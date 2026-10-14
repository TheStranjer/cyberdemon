//! Audio tools: bringing external sound files into the working archive.
//!
//! The first tool of the group is [`Cyberdemon::import_sound`]: fetch a
//! `WAV`/`OGG`/`MP3`/`FLAC` file through [`AppState::fetch_source`], decode
//! it with [`crate::audio::decode_any`] and either convert it to a vanilla
//! `DMX` `DS*` lump or store the original bytes unchanged.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::fetch::derive_lump_name;
use crate::audio::{Pcm, decode_any, encode_dmx, sniff_format};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::{fail, ok};
use crate::wad::archive::{Lump, Wad};
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
}

#[cfg(test)]
mod tests {
    use super::{SoundFormat, extension_of, parse_format, parse_max_seconds, parse_rate};

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
}
