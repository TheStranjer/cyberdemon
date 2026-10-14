//! Audio tools: bringing external sound files into the working archive and
//! previewing the sounds a session holds.
//!
//! The group's first tool is [`Cyberdemon::import_sound`]: fetch a
//! `WAV`/`OGG`/`MP3`/`FLAC` file through [`AppState::fetch_source`], decode
//! it with [`crate::audio::decode_any`] and either convert it to a vanilla
//! `DMX` `DS*` lump or store the original bytes unchanged.
//! [`Cyberdemon::preview_sound`] goes the other way: it reads one sound
//! lump of the session and publishes it as a playable asset, converting
//! `DMX` and `PC` speaker lumps to `WAV`. The conversion itself lives in
//! the crate-visible [`sound_to_playable`] so `get_asset_url` can reuse it.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::fetch::derive_lump_name;
use crate::assets::server::AssetRef;
use crate::assets::store::mime_for_extension;
use crate::audio::{
    Pcm, decode_any, decode_dmx, decode_wav, encode_dmx, encode_wav, sniff_format,
    synthesize_pc_speaker,
};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::fail;
use crate::tools::lumps::select::{LumpSelector, resolve};
use crate::tools::ok;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::LumpKind;
use crate::wad::name::{LumpName, MAX_NAME_LEN};

/// Prefix `Doom`'s sound table puts in front of every built-in sound name;
/// vanilla engines look sounds up as `DS<name>` with no `SNDINFO`.
const SOUND_PREFIX: &str = "DS";
/// Characters left for the derived stem once the prefix took its share of
/// the eight-byte lump-name field.
const SOUND_HEAD: usize = 6;
/// Default `DMX` sample rate, the vanilla engine's own.
const DEFAULT_RATE: u32 = 11025;
/// Lowest `sample_rate` the tool accepts.
const MIN_RATE: u32 = 4000;
/// Highest `sample_rate` the tool accepts, and the source rate `keep_rate`
/// is willing to preserve.
const MAX_RATE: u32 = 48000;
/// Default `max_seconds`.
const DEFAULT_MAX_SECONDS: u32 = 60;
/// Largest `max_seconds` the tool accepts; matches the decode cap in
/// [`crate::audio::decode_any`].
const MAX_MAX_SECONDS: u32 = 600;

/// Arguments for [`Cyberdemon::import_sound`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportSoundParams {
    /// Sound file to import: absolute path, `file://` URI, `http(s)://`
    /// URL or an uploaded asset URL. `WAV`, `OGG`, `MP3` and `FLAC` decode.
    source: String,
    /// Lump name (1 to 8 printable ASCII characters, uppercased). Defaults
    /// to `DS` plus the file stem uppercased and trimmed to 8 characters:
    /// `pistol.wav` becomes `DSPISTOL`.
    #[serde(default)]
    name: Option<String>,
    /// `"doom"` (the default) converts to a vanilla `DMX` sound lump;
    /// `"passthrough"` stores the original bytes unchanged, for
    /// `ZDoom`-family ports that play `WAV`/`OGG`/`FLAC`/`MP3` lumps.
    #[serde(default)]
    format: Option<String>,
    /// Target sample rate for the `doom` format, `4000..=48000` (default
    /// 11025). Ignored by `passthrough`.
    #[serde(default)]
    sample_rate: Option<u32>,
    /// `doom` format only: keep the source sample rate instead of
    /// resampling, when it is at most 48000 Hz (default `false`).
    #[serde(default)]
    keep_rate: bool,
    /// Longest accepted sound in seconds, `1..=600` (default 60); longer
    /// inputs fail with `limit_exceeded` before anything is stored.
    #[serde(default)]
    max_seconds: Option<u32>,
    /// Overwrite an existing working-archive lump with the same name
    /// (default `false`).
    #[serde(default)]
    replace: bool,
}

/// The validated arguments of one `import_sound` call.
struct Plan {
    /// Explicit lump name, `None` when it is derived from the file name.
    name: Option<LumpName>,
    /// What to store.
    format: SoundFormat,
    /// Target rate for the `doom` format.
    sample_rate: u32,
    /// Keep a source rate of at most [`MAX_RATE`] instead of resampling.
    keep_rate: bool,
    /// Length cap in seconds.
    max_seconds: u32,
    /// Whether a same-name working lump may be overwritten.
    replace: bool,
}

/// The `format` parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum SoundFormat {
    /// Convert to a vanilla `DMX` `DS*` sound lump.
    Doom,
    /// Store the original file bytes unchanged.
    Passthrough,
}

/// Structured result of [`Cyberdemon::import_sound`].
#[derive(Serialize)]
struct ImportSoundReport {
    /// The lump name the sound was stored under.
    name: LumpName,
    /// The format that was stored.
    format: SoundFormat,
    /// The decoded source container: `wav`, `ogg`, `mp3` or `flac`.
    source_format: &'static str,
    /// Sample rate of the stored sound in Hz.
    sample_rate: u32,
    /// Channel count the decoder read from the source file.
    channels_in: u16,
    /// Length of the stored sound in whole milliseconds.
    duration_ms: u64,
    /// Bytes in the stored lump.
    bytes: usize,
}

/// The sound file's lowercase extension, for the probe hint. A suffix is
/// only usable as a hint when it is `1..=5` ASCII letters.
fn extension_of(filename: &str) -> Option<String> {
    let (_, ext) = filename.rsplit_once('.')?;
    let usable =
        !ext.is_empty() && ext.len() <= 5 && ext.bytes().all(|byte| byte.is_ascii_alphabetic());
    usable.then(|| ext.to_ascii_lowercase())
}

/// The `format` parameter, defaulting to [`SoundFormat::Doom`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the accepted values.
fn parse_format(text: Option<&str>) -> Result<SoundFormat, CyberError> {
    let Some(raw) = text else {
        return Ok(SoundFormat::Doom);
    };
    match raw.to_ascii_lowercase().as_str() {
        "doom" => Ok(SoundFormat::Doom),
        "passthrough" => Ok(SoundFormat::Passthrough),
        other => Err(CyberError::InvalidArgument(format!(
            "unknown format {other:?}: use \"doom\" for a vanilla DMX DS* lump or \
             \"passthrough\" to store the original bytes"
        ))),
    }
}

/// The `sample_rate` parameter, defaulting to [`DEFAULT_RATE`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] outside `4000..=48000`.
fn parse_rate(value: Option<u32>) -> Result<u32, CyberError> {
    let Some(rate) = value else {
        return Ok(DEFAULT_RATE);
    };
    if !(MIN_RATE..=MAX_RATE).contains(&rate) {
        return Err(CyberError::InvalidArgument(format!(
            "sample rate {rate} is outside {MIN_RATE}..={MAX_RATE}; pass a rate in that range"
        )));
    }
    Ok(rate)
}

/// The `max_seconds` parameter, defaulting to [`DEFAULT_MAX_SECONDS`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] outside `1..=600`.
fn parse_max_seconds(value: Option<u32>) -> Result<u32, CyberError> {
    let Some(seconds) = value else {
        return Ok(DEFAULT_MAX_SECONDS);
    };
    if seconds == 0 || seconds > MAX_MAX_SECONDS {
        return Err(CyberError::InvalidArgument(format!(
            "max_seconds {seconds} is outside 1..={MAX_MAX_SECONDS}; pass a cap in that range \
             (decoding itself never returns more than 10 minutes of audio)"
        )));
    }
    Ok(seconds)
}

/// Validate [`Cyberdemon::import_sound`]'s arguments before any fetching.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad name, `format`, `sample_rate`
/// or `max_seconds`.
fn plan(args: &ImportSoundParams) -> Result<Plan, CyberError> {
    Ok(Plan {
        name: args.name.as_deref().map(LumpName::parse).transpose()?,
        format: parse_format(args.format.as_deref())?,
        sample_rate: parse_rate(args.sample_rate)?,
        keep_rate: args.keep_rate,
        max_seconds: parse_max_seconds(args.max_seconds)?,
        replace: args.replace,
    })
}

/// Refuse a source longer than the `max_seconds` cap.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] naming the cap and the fix.
fn check_duration(pcm: &Pcm, max_seconds: u32) -> Result<(), CyberError> {
    let frames = u64::try_from(pcm.frames()).unwrap_or(u64::MAX);
    let allowed = u64::from(max_seconds).saturating_mul(u64::from(pcm.sample_rate));
    if frames > allowed {
        return Err(CyberError::LimitExceeded(format!(
            "the sound is {} ms at {} Hz, over the max_seconds cap of {max_seconds}; trim the \
             file or raise max_seconds (at most {MAX_MAX_SECONDS})",
            pcm.duration_ms(),
            pcm.sample_rate
        )));
    }
    Ok(())
}

/// The payload to store plus the numbers that describe it.
struct Stored {
    /// The lump bytes.
    data: Vec<u8>,
    /// Sample rate of the stored sound in Hz.
    sample_rate: u32,
    /// Length of the stored sound in whole milliseconds.
    duration_ms: u64,
}

/// The `doom` payload: mono, resampled unless `keep_rate` preserves the
/// source rate, padded `DMX` bytes.
///
/// # Errors
///
/// [`Pcm::resample`] and [`encode_dmx`] errors pass through.
fn build_dmx(pcm: &Pcm, plan: &Plan) -> Result<Stored, CyberError> {
    let mono = pcm.to_mono();
    let keep = plan.keep_rate && pcm.sample_rate <= MAX_RATE;
    let sound = if keep {
        mono
    } else {
        mono.resample(plan.sample_rate)?
    };
    let sample_rate = sound.sample_rate;
    let duration_ms = sound.duration_ms();
    Ok(Stored {
        data: encode_dmx(&sound, true)?,
        sample_rate,
        duration_ms,
    })
}

/// The name to store under: the explicit one, else `DS` plus the derived,
/// trimmed file stem.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] asking for `name` when nothing usable
/// can be derived.
fn chosen_name(plan: &Plan, filename: &str) -> Result<LumpName, CyberError> {
    if let Some(name) = plan.name {
        return Ok(name);
    }
    let stem = derive_lump_name(filename).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "cannot derive a sound name from {filename:?}: pass name (1 to {MAX_NAME_LEN} \
             printable ASCII characters)"
        ))
    })?;
    let head: String = stem.to_string().chars().take(SOUND_HEAD).collect();
    LumpName::parse(&format!("{SOUND_PREFIX}{head}"))
}

/// The synchronous half: one undoable insert (or in-place replace) of the
/// sound lump in the global area of the working archive.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] for a working lump of the same name when
/// `replace` is `false`.
fn store(
    session: &mut Session,
    name: LumpName,
    data: Vec<u8>,
    replace: bool,
) -> Result<(), CyberError> {
    let label = format!("import_sound {name}");
    session.edit(&label, |wad| place_sound(wad, name, data, replace))
}

/// Put the sound at the end of `wad`, replacing the last same-name lump
/// when one exists and `replace` allows it. Sounds have no namespace.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] as by [`store`].
fn place_sound(
    wad: &mut Wad,
    name: LumpName,
    data: Vec<u8>,
    replace: bool,
) -> Result<(), CyberError> {
    if wad.find_last(name).is_none() {
        wad.lumps.push(Lump::new(name, data));
        return Ok(());
    }
    if !replace {
        return Err(CyberError::AlreadyExists(format!(
            "lump {name} already exists in the working archive: pass replace true to overwrite \
             it or choose another name"
        )));
    }
    if let Some(slot) = wad.lumps.iter_mut().rev().find(|lump| lump.name == name) {
        *slot = Lump::new(name, data);
    }
    Ok(())
}

/// Run `import_sound` end to end: fetch, decode, convert and commit.
///
/// # Errors
///
/// [`plan`], [`AppState::fetch_source`], [`decode_any`], [`check_duration`],
/// [`build_dmx`], [`chosen_name`] and [`store`] errors pass through.
async fn import(
    state: &AppState,
    args: &ImportSoundParams,
) -> Result<ImportSoundReport, CyberError> {
    let plan = plan(args)?;
    let fetched = state.fetch_source(&args.source).await?;
    let hint = extension_of(&fetched.filename);
    let pcm = decode_any(&fetched.bytes, hint.as_deref())?;
    let source_format = sniff_format(&fetched.bytes).ok_or_else(|| {
        CyberError::Unsupported(format!(
            "the decoder accepted {} but its container cannot be named; pass a WAV, OGG, FLAC \
             or MP3 file",
            fetched.filename
        ))
    })?;
    check_duration(&pcm, plan.max_seconds)?;
    let channels_in = pcm.channels;
    let stored = match plan.format {
        SoundFormat::Doom => build_dmx(&pcm, &plan)?,
        SoundFormat::Passthrough => Stored {
            sample_rate: pcm.sample_rate,
            duration_ms: pcm.duration_ms(),
            data: fetched.bytes,
        },
    };
    let name = chosen_name(&plan, &fetched.filename)?;
    let Stored {
        data,
        sample_rate,
        duration_ms,
    } = stored;
    let bytes = data.len();
    state.with_session(|session| store(session, name, data, plan.replace))?;
    Ok(ImportSoundReport {
        name,
        format: plan.format,
        source_format,
        sample_rate,
        channels_in,
        duration_ms,
        bytes,
    })
}

/// Sample width of the generated `WAV` files unless `bits` says otherwise.
const DEFAULT_BITS: u16 = 16_u16;

/// Arguments for [`Cyberdemon::preview_sound`].
#[derive(Debug, Deserialize, JsonSchema)]
struct PreviewSoundParams {
    /// The lump selector: `name` plus optional `scope`, `namespace` and
    /// `occurrence` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    selector: LumpSelector,
    /// Sample width of a generated `WAV`: `8` or `16` (default `16`). It
    /// applies to the conversions (`doom_sound` and `pc_speaker` lumps);
    /// container lumps (`wav`, `ogg`, `mp3`, `flac`) pass through unchanged.
    #[serde(default)]
    bits: Option<u16>,
}

/// What a playable conversion knows about the file it produced.
#[derive(Clone, Copy, Debug)]
pub(in crate::tools) struct SoundInfo {
    /// Sample rate of the playable audio in Hz.
    pub(in crate::tools) sample_rate: u32,
    /// Length of the playable audio in whole milliseconds.
    pub(in crate::tools) duration_ms: u64,
}

impl SoundInfo {
    /// The facts of one decoded [`Pcm`].
    fn of_pcm(pcm: &Pcm) -> Self {
        Self {
            sample_rate: pcm.sample_rate,
            duration_ms: pcm.duration_ms(),
        }
    }
}

/// Structured result of [`Cyberdemon::preview_sound`].
#[derive(Serialize)]
struct PreviewSoundReport {
    /// The previewed lump name.
    name: LumpName,
    /// The lump's detected kind.
    kind: LumpKind,
    /// The playable container that was published: `wav`, `ogg`, `mp3` or
    /// `flac`.
    format: &'static str,
    /// Sample rate in Hz, when the format names one.
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_rate: Option<u32>,
    /// Length in whole milliseconds, when it is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    /// The published playable file.
    asset: AssetRef,
}

/// The `bits` parameter, defaulting to [`DEFAULT_BITS`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] unless the value is `8` or `16`.
fn parse_bits(value: Option<u16>) -> Result<u16, CyberError> {
    match value {
        None => Ok(DEFAULT_BITS),
        Some(bits @ (8_u16 | 16_u16)) => Ok(bits),
        Some(bits) => Err(CyberError::InvalidArgument(format!(
            "bits {bits} is neither 8 nor 16: pass bits 8 or bits 16"
        ))),
    }
}

/// Whether a classification names a sound [`sound_to_playable`] converts.
const fn is_sound_kind(kind: LumpKind) -> bool {
    matches!(
        kind,
        LumpKind::DoomSound
            | LumpKind::PcSpeaker
            | LumpKind::Wav
            | LumpKind::Ogg
            | LumpKind::Mp3
            | LumpKind::Flac
    )
}

/// The result of making one sound lump playable: the file bytes, its
/// extension (without the dot) and, where the format names them, the
/// sample rate and length.
type Playable = (Vec<u8>, &'static str, Option<SoundInfo>);

/// Render one decoded [`Pcm`] as the `WAV` file plus its facts.
///
/// # Errors
///
/// [`encode_wav`]'s errors pass through.
fn playable_wav(pcm: &Pcm, bits: u16) -> Result<Playable, CyberError> {
    let info = SoundInfo::of_pcm(pcm);
    Ok((encode_wav(pcm, bits)?, "wav", Some(info)))
}

/// Convert one sound lump's bytes into a playable file: the file bytes,
/// its extension (without the dot) and, where the format names them, the
/// sample rate and length.
///
/// `doom_sound` lumps are decoded and re-encoded as `WAV` at `bits`;
/// `pc_speaker` lumps are synthesized (mono 11025 Hz square wave, pitch
/// approximate, see [`synthesize_pc_speaker`]) and encoded as `WAV` at
/// `bits`; the four container kinds pass through unchanged.
///
/// # Errors
///
/// [`decode_dmx`], [`synthesize_pc_speaker`] and [`encode_wav`]'s errors
/// for malformed sound data; [`CyberError::Unsupported`] for a kind that
/// is not a sound.
pub(in crate::tools) fn sound_to_playable(
    bytes: &[u8],
    kind: LumpKind,
    bits: u16,
) -> Result<Playable, CyberError> {
    match kind {
        LumpKind::DoomSound => playable_wav(&decode_dmx(bytes)?, bits),
        LumpKind::PcSpeaker => playable_wav(&synthesize_pc_speaker(bytes)?, bits),
        LumpKind::Wav => {
            // The bytes publish unchanged; the rate and length are extras
            // when the header still decodes.
            let info = decode_wav(bytes).ok().as_ref().map(SoundInfo::of_pcm);
            Ok((bytes.to_vec(), "wav", info))
        }
        LumpKind::Ogg => Ok((bytes.to_vec(), "ogg", None)),
        LumpKind::Mp3 => Ok((bytes.to_vec(), "mp3", None)),
        LumpKind::Flac => Ok((bytes.to_vec(), "flac", None)),
        other @ (LumpKind::Marker
        | LumpKind::MapMarker
        | LumpKind::MapData
        | LumpKind::Palette
        | LumpKind::Colormap
        | LumpKind::Pnames
        | LumpKind::TextureDefs
        | LumpKind::Endoom
        | LumpKind::Demo
        | LumpKind::Dehacked
        | LumpKind::Decorate
        | LumpKind::Zscript
        | LumpKind::Mapinfo
        | LumpKind::Png
        | LumpKind::Jpeg
        | LumpKind::Gif
        | LumpKind::Mus
        | LumpKind::Midi
        | LumpKind::Flat
        | LumpKind::Sprite
        | LumpKind::Patch
        | LumpKind::Text
        | LumpKind::Empty
        | LumpKind::Unknown) => Err(CyberError::Unsupported(format!(
            "a {} lump is not a recognised sound; use get_lump format=url \
             for raw bytes",
            other.as_str()
        ))),
    }
}

/// Run `preview_sound` end to end: resolve the lump, convert it and
/// publish the playable file.
///
/// # Errors
///
/// [`parse_bits`] and [`resolve`] errors for the arguments;
/// [`CyberError::Unsupported`] naming the lump when its kind is not a
/// sound; [`sound_to_playable`] errors for the bytes;
/// [`AppState::publish`] errors for the asset store.
fn preview(state: &AppState, args: &PreviewSoundParams) -> Result<PreviewSoundReport, CyberError> {
    let bits = parse_bits(args.bits)?;
    let row = state.with_session(|session| resolve(session, &args.selector))?;
    if !is_sound_kind(row.kind) {
        let name = row.name;
        return Err(CyberError::Unsupported(format!(
            "lump {name} is not a recognised sound; use get_lump format=url for raw bytes"
        )));
    }
    let (data, extension, info) = sound_to_playable(&row.lump.data, row.kind, bits)?;
    let filename = format!("{}.{extension}", row.name);
    let mime = mime_for_extension(extension);
    let asset = state.publish(&filename, mime, data)?;
    Ok(PreviewSoundReport {
        name: row.name,
        kind: row.kind,
        format: extension,
        sample_rate: info.map(|facts| facts.sample_rate),
        duration_ms: info.map(|facts| facts.duration_ms),
        asset,
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = audio_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Imports a sound file as a lump in the working archive.
    #[tool(
        description = "Imports a WAV, OGG, MP3 or FLAC sound file as a lump in the working archive, as one undoable edit labelled \"import_sound NAME\" (save_wad writes the result). source: absolute path, file:// URI, http(s):// URL or this server's asset URL. format \"doom\" (default) decodes the file, downmixes to mono and stores a vanilla DMX DS* lump (unsigned 8-bit, padded): sample_rate is the target rate, 4000..=48000 default 11025, and keep_rate=true stores the source rate instead when it is at most 48000 Hz. format \"passthrough\" validates that the bytes decode, then stores the original file unchanged - for ZDoom-family ports, which play WAV/OGG/FLAC/MP3 lumps directly. name defaults to \"DS\" plus the file stem uppercased and trimmed to 8 characters (pistol.wav -> DSPISTOL): vanilla Doom needs SNDINFO-free DS* names that match its built-in sound names (DSPISTOL, DSDOORSW ...), while ZDoom-family ports can map any lump name through SNDINFO. max_seconds (1..=600, default 60) rejects longer sounds with limit_exceeded before anything is stored; decoding itself never returns more than 10 minutes of audio. A working lump of the same name is already_exists unless replace=true (replace overwrites the last copy in place). Returns {name, format, source_format, sample_rate, channels_in, duration_ms, bytes}."
    )]
    async fn import_sound(
        &self,
        Parameters(args): Parameters<ImportSoundParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match import(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Publishes one sound lump as a playable audio asset.
    #[tool(
        description = "Publishes one sound lump of the session as a playable file on the localhost asset server; the archives are not modified. Takes the shared lump selector: name plus optional scope (\"merged\" default, \"working\", \"layer:N\"), namespace and occurrence (default the last, winning match). What runs depends on the lump's detected kind: a doom_sound (DMX DS* lump) is decoded and re-encoded as a WAV - bits 8 or 16 (default 16) is the sample width of generated WAVs; a pc_speaker (DP* tone-list lump) is synthesized into a mono 11025 Hz square-wave WAV (each 1/140 s tick keeps its exact share of samples so timing does not drift; tone n plays at 110 + 13n Hz, an approximation of the vanilla tone table, so the pitch is approximate); wav, ogg, mp3 and flac lumps are published unchanged with their own extension. Any other kind is unsupported (use get_lump format=url for raw bytes); a name with no match is not_found; a bad bits value is invalid_argument. Returns {name, kind, format, sample_rate, duration_ms (the last two when the format names them: DMX, WAV, PC speaker), asset: {url, filename, mime, size}}."
    )]
    fn preview_sound(
        &self,
        Parameters(args): Parameters<PreviewSoundParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match preview(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SoundFormat, extension_of, is_sound_kind, parse_bits, parse_format, parse_max_seconds,
        parse_rate, sound_to_playable,
    };
    use crate::audio::{Pcm, decode_wav, encode_dmx, encode_wav};
    use crate::wad::classify::LumpKind;

    /// One frame of the small square wave the fixtures use.
    fn square_at(frame: usize) -> i16 {
        if frame.checked_rem(2).unwrap_or(0) == 0 {
            8000_i16
        } else {
            -8000_i16
        }
    }

    /// Mono `Pcm` of `frames` samples of a small square-ish wave.
    fn tone(rate: u32, frames: usize) -> Pcm {
        Pcm {
            sample_rate: rate,
            channels: 1,
            samples: (0..frames).map(square_at).collect(),
        }
    }

    #[test]
    fn format_parses_the_two_ways_case_insensitively() {
        assert_eq!(
            parse_format(None).expect("default"),
            SoundFormat::Doom,
            "doom is the default"
        );
        assert_eq!(
            parse_format(Some("PassThrough")).expect("case folds"),
            SoundFormat::Passthrough,
            "names fold"
        );
        let error = parse_format(Some("mp3")).expect_err("not a storage format");
        assert_eq!(error.code(), "invalid_argument", "bad format code");
        assert!(
            error.to_string().contains("passthrough"),
            "the message names the fix: {error}"
        );
    }

    #[test]
    fn rates_and_caps_are_checked_before_any_fetch() {
        assert_eq!(
            parse_rate(None).expect("default"),
            11025_u32,
            "vanilla rate"
        );
        assert_eq!(
            parse_rate(Some(4000)).expect("low bound"),
            4000_u32,
            "bound ok"
        );
        assert_eq!(
            parse_rate(Some(48000)).expect("high bound"),
            48000_u32,
            "bound ok"
        );
        assert_eq!(
            parse_rate(Some(3999)).expect_err("under range").code(),
            "invalid_argument",
            "low rate code"
        );
        assert_eq!(
            parse_rate(Some(48001)).expect_err("over range").code(),
            "invalid_argument",
            "high rate code"
        );
        assert_eq!(
            parse_max_seconds(None).expect("default"),
            60_u32,
            "one minute default"
        );
        assert_eq!(
            parse_max_seconds(Some(600)).expect("bound"),
            600_u32,
            "the cap is inclusive"
        );
        assert_eq!(
            parse_max_seconds(Some(0)).expect_err("zero cap").code(),
            "invalid_argument",
            "zero max_seconds code"
        );
        assert_eq!(
            parse_max_seconds(Some(601)).expect_err("over cap").code(),
            "invalid_argument",
            "raised max_seconds stays bounded"
        );
    }

    #[test]
    fn extension_hints_come_from_short_alphabetic_suffixes_only() {
        assert_eq!(extension_of("pistol.wav").as_deref(), Some("wav"), "plain");
        assert_eq!(
            extension_of("Gun.CUT").as_deref(),
            Some("cut"),
            "lowercased for the hint"
        );
        assert_eq!(extension_of("noextension"), None, "no dot");
        assert_eq!(extension_of("trailing."), None, "empty suffix");
        assert_eq!(
            extension_of("archive.tar.gz").as_deref(),
            Some("gz"),
            "last suffix wins; a wrong hint only misleads the probe's search order, never the result"
        );
        assert_eq!(
            extension_of("sound.toolongext"),
            None,
            "over five characters"
        );
    }

    #[test]
    fn bits_defaults_to_16_and_accepts_only_8_and_16() {
        assert_eq!(parse_bits(None).expect("default"), 16_u16, "16-bit default");
        assert_eq!(parse_bits(Some(8)).expect("low width"), 8_u16, "8 ok");
        assert_eq!(parse_bits(Some(16)).expect("high width"), 16_u16, "16 ok");
        let error = parse_bits(Some(12)).expect_err("not a WAV width");
        assert_eq!(error.code(), "invalid_argument", "bad bits code");
        assert!(
            error.to_string().contains("pass bits 8"),
            "the message names the fix: {error}"
        );
        assert_eq!(
            parse_bits(Some(0)).expect_err("zero width").code(),
            "invalid_argument",
            "zero bits code"
        );
    }

    #[test]
    fn sound_kinds_are_exactly_the_six_playable_ones() {
        let playable = [
            LumpKind::DoomSound,
            LumpKind::PcSpeaker,
            LumpKind::Wav,
            LumpKind::Ogg,
            LumpKind::Mp3,
            LumpKind::Flac,
        ];
        for kind in LumpKind::ALL {
            assert_eq!(
                is_sound_kind(kind),
                playable.contains(&kind),
                "{kind:?} playability",
            );
        }
    }

    #[test]
    fn dmx_lump_bytes_become_a_wav_with_their_facts() {
        let bytes = encode_dmx(&tone(22050, 100), true).expect("mono encodes");
        let (wav, extension, maybe_facts) =
            sound_to_playable(&bytes, LumpKind::DoomSound, 16).expect("a DMX sound converts");
        assert_eq!(extension, "wav", "a WAV is produced");
        assert_eq!(wav[..4], *b"RIFF", "RIFF magic");
        let decoded = decode_wav(&wav).expect("the WAV we just encoded");
        assert_eq!(decoded.sample_rate, 22050, "the lump's rate");
        assert_eq!(decoded.frames(), 100, "the pads are gone");
        let facts = maybe_facts.expect("DMX names its rate and length");
        assert_eq!(facts.sample_rate, 22050, "reported rate");
        assert_eq!(facts.duration_ms, 4, "100 frames at 22050 Hz floor to 4 ms");
    }

    #[test]
    fn bits_8_writes_the_narrower_wav_data() {
        let bytes = encode_dmx(&tone(11025, 100), true).expect("mono encodes");
        let (wide, _, _) = sound_to_playable(&bytes, LumpKind::DoomSound, 16).expect("16-bit");
        let (narrow, _, _) = sound_to_playable(&bytes, LumpKind::DoomSound, 8).expect("8-bit");
        assert_eq!(
            wide.len(),
            44_usize.checked_add(200).expect("test sizes fit"),
            "canonical header plus i16 frames"
        );
        assert_eq!(
            narrow.len(),
            44_usize.checked_add(100).expect("test sizes fit"),
            "canonical header plus byte frames"
        );
    }

    #[test]
    fn pc_speaker_lumps_synthesize_and_broken_ones_refuse() {
        let lump = [0_u8, 0, 3, 0, 0, 20, 40];
        let (wav, extension, maybe_facts) =
            sound_to_playable(&lump, LumpKind::PcSpeaker, 16).expect("a tone list synthesizes");
        assert_eq!(extension, "wav", "the synthesis is a WAV");
        let decoded = decode_wav(&wav).expect("the WAV we just encoded");
        assert_eq!(decoded.sample_rate, 11025, "the engine's rate");
        assert_eq!(decoded.frames(), 236, "3 * 11025 / 140 floored");
        let facts = maybe_facts.expect("the synthesis names its rate and length");
        assert_eq!(
            facts.duration_ms, 21,
            "236 frames at 11025 Hz floor to 21 ms"
        );
        let broken = [1_u8, 0, 1, 0, 0];
        let error = sound_to_playable(&broken, LumpKind::PcSpeaker, 16)
            .expect_err("format 1 is not a tone list");
        assert_eq!(error.code(), "format_error", "bad DP format code");
    }

    #[test]
    fn containers_pass_through_and_only_wav_reports_facts() {
        let wav = encode_wav(&tone(8000, 30), 16).expect("the fixture encodes");
        let (kept, extension, facts) =
            sound_to_playable(&wav, LumpKind::Wav, 16).expect("a WAV passes through");
        assert_eq!(extension, "wav", "kept as a WAV");
        assert_eq!(kept, wav, "byte for byte unchanged");
        let header = facts.expect("the WAV header decodes");
        assert_eq!(header.sample_rate, 8000, "rate from the header");
        assert_eq!(header.duration_ms, 3, "30 frames at 8000 Hz floor to 3 ms");
        for (kind, wanted) in [
            (LumpKind::Ogg, "ogg"),
            (LumpKind::Mp3, "mp3"),
            (LumpKind::Flac, "flac"),
        ] {
            let payload = b"\x00\x01\x02\x03";
            let (data, got, info) = sound_to_playable(payload, kind, 16).expect("bytes pass");
            assert_eq!(got, wanted, "extension for {kind:?}");
            assert_eq!(data, payload, "unchanged for {kind:?}");
            assert!(info.is_none(), "no facts for {kind:?}");
        }
    }

    #[test]
    fn a_broken_wav_still_passes_through_without_facts() {
        let not_a_real_wav = b"RIFF\x00\x00\x00\x00WAVEjunk";
        let (bytes, extension, info) =
            sound_to_playable(not_a_real_wav, LumpKind::Wav, 16).expect("the bytes pass");
        assert_eq!(extension, "wav", "the kind picks the extension");
        assert_eq!(bytes, not_a_real_wav, "unchanged");
        assert!(info.is_none(), "an undecodable header reports nothing");
    }

    #[test]
    fn non_sound_kinds_are_unsupported_with_a_way_out() {
        let error =
            sound_to_playable(b"xx", LumpKind::Palette, 16).expect_err("a palette is not a sound");
        assert_eq!(error.code(), "unsupported", "palette code");
        assert!(
            error.to_string().contains("get_lump format=url"),
            "the message names the way out: {error}"
        );
    }
}
