//! Music tools: bringing external music files into the working archive and
//! previewing the music lumps a session holds.
//!
//! The group's first tool is [`Cyberdemon::import_music`]: fetch a
//! music file through [`AppState::fetch_source`], identify its container
//! with [`crate::music::sniff_music`] and either store a vanilla `MUS`
//! `D_*` lump (converting `MIDI` with [`midi_to_mus`], validating `MUS`
//! with [`mus_to_midi`]) or keep the original bytes unchanged. Naming and
//! placement run through the shared [`crate::tools::audio_common::MUSIC`]
//! family rule. [`Cyberdemon::preview_music`] goes the other way: it reads
//! one music lump of the session and publishes it as a playable asset,
//! converting `MUS` to `MIDI`. The conversion itself lives in the
//! crate-visible [`music_to_playable`] so `get_asset_url` can reuse it.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::music::{MusicKind, midi_to_mus, mus_to_midi, sniff_music};
use crate::server::Cyberdemon;
use crate::state::AppState;
use crate::tools::audio_common::{MUSIC, MediaLump, publish_playable};
use crate::tools::lumps::select::{LumpSelector, resolve};
use crate::tools::reply;
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::import_music`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportMusicParams {
    /// Music file to import: absolute path, `file://` URI, `http(s)://`
    /// URL or an uploaded asset URL. `MUS`, `MIDI`, `OGG`, `MP3`, `FLAC`,
    /// `WAV`, `XM`, `IT`, `S3M` and `MOD` are recognised from their bytes.
    source: String,
    /// Lump name (1 to 8 printable ASCII characters, uppercased). Defaults
    /// to `D_` plus the file stem uppercased and trimmed to 8 characters:
    /// `runnin.mid` becomes `D_RUNNIN`. A stem that already starts with
    /// `D_` is kept whole: `D_RUNNIN.mid` becomes `D_RUNNIN`, not
    /// `D_D_RUNN`.
    #[serde(default)]
    name: Option<String>,
    /// `"mus"` stores a vanilla `MUS` lump (`MIDI` is converted, `MUS`
    /// validated as-is; any other input is refused); `"passthrough"` stores
    /// the original bytes unchanged. Default: `"mus"` when the input is
    /// `MIDI` or `MUS`, `"passthrough"` otherwise.
    #[serde(default)]
    format: Option<String>,
    /// Overwrite an existing working-archive lump with the same name
    /// (default `false`).
    #[serde(default)]
    replace: bool,
}

/// The validated arguments of one `import_music` call.
#[derive(Debug)]
struct Plan {
    /// Explicit lump name, `None` when it is derived from the file name.
    name: Option<LumpName>,
    /// Requested storage format, `None` to default from the sniffed kind.
    format: Option<StorageFormat>,
    /// Whether a same-name working lump may be overwritten.
    replace: bool,
}

/// The `format` parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum StorageFormat {
    /// Store a vanilla `MUS` `D_*` lump.
    Mus,
    /// Store the original file bytes unchanged.
    Passthrough,
}

/// Structured result of [`Cyberdemon::import_music`].
#[derive(Serialize)]
struct ImportMusicReport {
    /// The lump name the music was stored under.
    name: LumpName,
    /// The container the input bytes were sniffed as.
    input_format: MusicKind,
    /// What was stored.
    stored_format: StorageFormat,
    /// Bytes in the stored lump.
    bytes: usize,
    /// One line per kind of `MIDI` event a conversion dropped.
    warnings: Vec<String>,
}

/// The `format` parameter, `None` when it is left to the sniffed kind.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the accepted values.
fn parse_format(text: Option<&str>) -> Result<Option<StorageFormat>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    match raw.to_ascii_lowercase().as_str() {
        "mus" => Ok(Some(StorageFormat::Mus)),
        "passthrough" => Ok(Some(StorageFormat::Passthrough)),
        other => Err(CyberError::InvalidArgument(format!(
            "unknown format {other:?}: use \"mus\" for a vanilla Doom MUS D_* lump or \
             \"passthrough\" to store the original bytes"
        ))),
    }
}

/// Validate [`Cyberdemon::import_music`]'s arguments before any fetching.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad name or `format`.
fn plan(args: &ImportMusicParams) -> Result<Plan, CyberError> {
    Ok(Plan {
        name: args.name.as_deref().map(LumpName::parse).transpose()?,
        format: parse_format(args.format.as_deref())?,
        replace: args.replace,
    })
}

/// The format an unprefixed choice defaults to: `mus` for the two kinds
/// that can become a `MUS` lump, `passthrough` for everything else.
const fn default_format(kind: MusicKind) -> StorageFormat {
    if kind.convertible_to_mus() {
        StorageFormat::Mus
    } else {
        StorageFormat::Passthrough
    }
}

/// The format to store in: the explicit one, else [`default_format`].
fn chosen_format(plan: &Plan, kind: MusicKind) -> StorageFormat {
    plan.format.unwrap_or_else(|| default_format(kind))
}

/// The payload to store plus what the conversion had to say.
#[derive(Debug)]
struct Stored {
    /// The lump bytes.
    data: Vec<u8>,
    /// One line per kind of `MIDI` event a conversion dropped.
    warnings: Vec<String>,
}

/// The `mus` payload: converted `MIDI`, validated-and-copied `MUS`, or a
/// refusal naming the way out.
///
/// # Errors
///
/// [`midi_to_mus`] and [`mus_to_midi`] errors pass through;
/// [`CyberError::InvalidArgument`] for a kind that cannot become `MUS`.
fn build_mus(kind: MusicKind, bytes: &[u8]) -> Result<Stored, CyberError> {
    match kind {
        MusicKind::Midi => {
            let conversion = midi_to_mus(bytes)?;
            Ok(Stored {
                data: conversion.mus,
                warnings: conversion.warnings,
            })
        }
        MusicKind::Mus => {
            mus_to_midi(bytes)?;
            Ok(Stored {
                data: bytes.to_vec(),
                warnings: Vec::new(),
            })
        }
        other @ (MusicKind::Ogg
        | MusicKind::Mp3
        | MusicKind::Flac
        | MusicKind::Wav
        | MusicKind::Xm
        | MusicKind::It
        | MusicKind::S3m
        | MusicKind::Mod
        | MusicKind::Unknown) => Err(CyberError::InvalidArgument(format!(
            "only MIDI or MUS can be stored as MUS; {other:?} input cannot: use format \
             \"passthrough\" to store it unchanged (ZDoom-family ports also play OGG, MP3, \
             FLAC, WAV and tracker module lumps)"
        ))),
    }
}

/// The payload for the chosen format; the `passthrough` path keeps the
/// original bytes of any recognised container.
///
/// # Errors
///
/// [`build_mus`] errors pass through; [`CyberError::Format`] for bytes of
/// an unrecognised container.
fn store_payload(
    format: StorageFormat,
    kind: MusicKind,
    bytes: Vec<u8>,
) -> Result<Stored, CyberError> {
    match format {
        StorageFormat::Mus => build_mus(kind, &bytes),
        StorageFormat::Passthrough => {
            if kind == MusicKind::Unknown {
                return Err(CyberError::Format(
                    "cannot identify the music file: recognised formats are MUS, MIDI, OGG, \
                     MP3, FLAC, WAV, XM, IT, S3M and MOD; pass a file in one of them"
                        .to_owned(),
                ));
            }
            Ok(Stored {
                data: bytes,
                warnings: Vec::new(),
            })
        }
    }
}

/// Run `import_music` end to end: fetch, sniff, convert and commit.
///
/// # Errors
///
/// [`plan`], [`AppState::fetch_source`], [`store_payload`],
/// [`MUSIC`](crate::tools::audio_common::MUSIC)'s name and store errors
/// pass through.
async fn import(
    state: &AppState,
    args: &ImportMusicParams,
) -> Result<ImportMusicReport, CyberError> {
    let plan = plan(args)?;
    let fetched = state.fetch_source(&args.source).await?;
    let kind = sniff_music(&fetched.bytes);
    let format = chosen_format(&plan, kind);
    let stored = store_payload(format, kind, fetched.bytes)?;
    let name = MUSIC.choose_name(plan.name, &fetched.filename)?;
    let warnings = stored.warnings;
    let bytes = stored.data.len();
    let lump = MediaLump {
        name,
        data: stored.data,
    };
    state.with_session(|session| MUSIC.store(session, lump, plan.replace))?;
    Ok(ImportMusicReport {
        name,
        input_format: kind,
        stored_format: format,
        bytes,
        warnings,
    })
}

/// Arguments for [`Cyberdemon::preview_music`].
#[derive(Debug, Deserialize, JsonSchema)]
struct PreviewMusicParams {
    /// The lump selector: `name` plus optional `scope`, `namespace` and
    /// `occurrence` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    selector: LumpSelector,
}

/// Structured result of [`Cyberdemon::preview_music`].
#[derive(Serialize)]
struct PreviewMusicReport {
    /// The previewed lump name.
    name: LumpName,
    /// The container the lump's bytes were sniffed as.
    input_format: MusicKind,
    /// The playable container that was published: `mid`, `ogg`, `mp3`,
    /// `flac`, `wav` or one of the tracker-module extensions.
    output_format: &'static str,
    /// The published playable file.
    asset: AssetRef,
}

/// The result of making one music lump playable: the file bytes, its
/// extension (without the dot) and the sniffed input kind.
type Playable = (Vec<u8>, &'static str, MusicKind);

/// Make one lump named `name`'s bytes playable: the file bytes, the
/// extension to publish them under (without the dot) and the sniffed input
/// kind. The bytes are sniffed exactly once, here.
///
/// `MUS` scores are converted to type-0 `SMF` with [`mus_to_midi`]; every
/// other recognised container is kept byte for byte. This is the tools-
/// visible conversion `preview_music` uses and `get_asset_url` can reuse.
///
/// # Errors
///
/// [`mus_to_midi`]' [`CyberError::Format`] for a corrupt `MUS` lump;
/// [`CyberError::Unsupported`] naming the lump when its bytes are not
/// recognised music.
pub(in crate::tools) fn music_to_playable(
    name: LumpName,
    bytes: &[u8],
) -> Result<Playable, CyberError> {
    let kind = sniff_music(bytes);
    match kind {
        MusicKind::Mus => Ok((mus_to_midi(bytes)?, "mid", kind)),
        MusicKind::Midi => Ok((bytes.to_vec(), "mid", kind)),
        MusicKind::Ogg => Ok((bytes.to_vec(), "ogg", kind)),
        MusicKind::Mp3 => Ok((bytes.to_vec(), "mp3", kind)),
        MusicKind::Flac => Ok((bytes.to_vec(), "flac", kind)),
        MusicKind::Wav => Ok((bytes.to_vec(), "wav", kind)),
        MusicKind::Xm => Ok((bytes.to_vec(), "xm", kind)),
        MusicKind::It => Ok((bytes.to_vec(), "it", kind)),
        MusicKind::S3m => Ok((bytes.to_vec(), "s3m", kind)),
        MusicKind::Mod => Ok((bytes.to_vec(), "mod", kind)),
        MusicKind::Unknown => Err(CyberError::Unsupported(format!(
            "lump {name} is not recognised music; use get_lump format=url for raw bytes"
        ))),
    }
}

/// Run `preview_music` end to end: resolve the lump, convert it and publish
/// the playable file.
///
/// # Errors
///
/// [`resolve`] errors for the selector; [`music_to_playable`] errors for
/// the bytes; [`publish_playable`] errors for the asset store.
fn preview(state: &AppState, args: &PreviewMusicParams) -> Result<PreviewMusicReport, CyberError> {
    let row = state.with_session(|session| resolve(session, &args.selector))?;
    let (data, extension, kind) = music_to_playable(row.name, &row.lump.data)?;
    let asset = publish_playable(state, row.name, extension, data)?;
    Ok(PreviewMusicReport {
        name: row.name,
        input_format: kind,
        output_format: extension,
        asset,
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = music_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Imports a music file as a lump in the working archive.
    #[tool(
        description = "Imports a music file as a lump in the working archive as one undoable edit: the container is sniffed from the bytes (MUS, MIDI, OGG, MP3, FLAC, XM, IT, S3M, MOD); format mus converts MIDI to a vanilla D_-lump and warns about events MUS cannot carry, while port formats keep their names. name defaults to the file stem under the D_ prefix, kept whole when it already starts with D_. preview_music publishes it back as a playable file."
    )]
    async fn import_music(
        &self,
        Parameters(args): Parameters<ImportMusicParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(import(self.state(), &args).await)
    }

    /// Publishes one music lump as a playable MIDI or original-file asset.
    #[tool(
        description = "Publishes one music lump of the session as a playable file on the localhost asset server; the archives are not modified. A MUS lump converts to a type-0 MIDI and a port format (OGG, MP3, FLAC, tracker modules) is served as stored. Takes the shared lump selector. Use it to hear a track before replacing or renaming it."
    )]
    fn preview_music(
        &self,
        Parameters(args): Parameters<PreviewMusicParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(preview(self.state(), &args))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ImportMusicParams, MusicKind, Plan, StorageFormat, music_to_playable, sniff_music,
    };
    use super::{chosen_format, parse_format, plan, store_payload};

    /// The arguments of a `source`-only call, everything else defaulted.
    fn args(source: &str) -> ImportMusicParams {
        ImportMusicParams {
            source: source.to_owned(),
            name: None,
            format: None,
            replace: false,
        }
    }

    #[test]
    fn format_parses_both_ways_case_insensitively_and_defaults_to_none() {
        assert_eq!(
            parse_format(None).expect("absent is fine"),
            None,
            "the default depends on the input"
        );
        assert_eq!(
            parse_format(Some("MUS")).expect("case folds"),
            Some(StorageFormat::Mus),
            "\"mus\" names the conversion"
        );
        assert_eq!(
            parse_format(Some("PassThrough")).expect("case folds"),
            Some(StorageFormat::Passthrough),
            "\"passthrough\" stores bytes"
        );
        let error = parse_format(Some("zdoom")).expect_err("not a storage format");
        assert_eq!(error.code(), "invalid_argument", "bad format code");
        assert!(
            error.to_string().contains("passthrough"),
            "the message names the fix: {error}"
        );
    }

    #[test]
    fn plan_validates_name_and_format_before_any_fetch() {
        let quiet = plan(&args("/x/y.mid")).expect("a bare source plans");
        assert!(quiet.name.is_none(), "name defaults to the derived stem");
        assert!(quiet.format.is_none(), "format defaults after sniffing");
        assert!(!quiet.replace, "replace defaults to false");
        let long = ImportMusicParams {
            name: Some("NINECHARS".to_owned()),
            ..args("/x/y.mid")
        };
        assert_eq!(
            plan(&long).expect_err("over eight characters").code(),
            "invalid_argument",
            "bad name code"
        );
        let bad_format = ImportMusicParams {
            format: Some("ogg".to_owned()),
            ..args("/x/y.mid")
        };
        assert_eq!(
            plan(&bad_format).expect_err("not a storage format").code(),
            "invalid_argument",
            "bad format code"
        );
    }

    #[test]
    fn an_unset_format_follows_the_sniffed_kind() {
        let blank = Plan {
            name: None,
            format: None,
            replace: false,
        };
        assert_eq!(
            chosen_format(&blank, MusicKind::Midi),
            StorageFormat::Mus,
            "MIDI defaults to the conversion"
        );
        assert_eq!(
            chosen_format(&blank, MusicKind::Mus),
            StorageFormat::Mus,
            "MUS defaults to staying MUS"
        );
        for kind in [MusicKind::Ogg, MusicKind::Mod, MusicKind::Unknown] {
            assert_eq!(
                chosen_format(&blank, kind),
                StorageFormat::Passthrough,
                "{kind:?} defaults to passthrough"
            );
        }
        let explicit = Plan {
            format: Some(StorageFormat::Passthrough),
            name: None,
            replace: false,
        };
        assert_eq!(
            chosen_format(&explicit, MusicKind::Midi),
            StorageFormat::Passthrough,
            "the explicit choice wins"
        );
    }

    #[test]
    fn passthrough_stores_recognised_bytes_and_refuses_unknown_ones() {
        let ogg = b"OggS\x00\x01\x02\x03".to_vec();
        let stored = store_payload(StorageFormat::Passthrough, MusicKind::Ogg, ogg.clone())
            .expect("a recognised container passes through");
        assert_eq!(stored.data, ogg, "byte for byte");
        assert!(stored.warnings.is_empty(), "nothing was converted");
        let error = store_payload(StorageFormat::Passthrough, MusicKind::Unknown, vec![9_u8])
            .expect_err("unknown bytes cannot be stored");
        assert_eq!(error.code(), "format_error", "unrecognised code");
        assert!(
            error.to_string().contains("MUS, MIDI, OGG"),
            "the message lists the formats: {error}"
        );
    }

    #[test]
    fn storing_as_mus_refuses_every_kind_but_midi_and_mus() {
        for kind in [
            MusicKind::Ogg,
            MusicKind::Mp3,
            MusicKind::Wav,
            MusicKind::Unknown,
        ] {
            let error = store_payload(StorageFormat::Mus, kind, b"data".to_vec())
                .expect_err("{kind:?} is not MUS-able");
            assert_eq!(error.code(), "invalid_argument", "code for {kind:?}");
            assert!(
                error.to_string().contains("passthrough"),
                "the message names the way out for {kind:?}: {error}"
            );
        }
        let corrupt_mus = sniff_music(b"MUS\x1A");
        assert_eq!(corrupt_mus, MusicKind::Mus, "the magic sniffs");
        let error = store_payload(StorageFormat::Mus, corrupt_mus, b"MUS\x1A".to_vec())
            .expect_err("four bytes are not a whole MUS lump");
        assert_eq!(
            error.code(),
            "format_error",
            "validation passes errors through"
        );
    }

    /// Bytes of the fixed `MUS` header in the fixture.
    const HEADER_LEN: u16 = 16_u16;

    /// The lump name every converter test reports failures under.
    fn song_name() -> super::LumpName {
        super::LumpName::parse("D_TEST").expect("a valid test name")
    }

    /// A minimal valid `MUS` lump: the 16-byte header plus a score holding
    /// one middle-C note and the end marker.
    fn mus_fixture() -> Vec<u8> {
        let score: [u8; 6] = [0x10_u8, 0x3C, 0x80, 0x3C, 0x0A, 0x60];
        let score_len = u16::try_from(score.len()).expect("the fixture score fits");
        let mut bytes = Vec::from(&b"MUS\x1A"[..]);
        bytes.extend_from_slice(&score_len.to_le_bytes());
        bytes.extend_from_slice(&HEADER_LEN.to_le_bytes());
        for _word in 0_usize..4 {
            bytes.extend_from_slice(&0_u16.to_le_bytes());
        }
        bytes.extend_from_slice(&score);
        bytes
    }

    #[test]
    fn playable_mus_becomes_a_midi_file_and_midi_passes_through() {
        let (midi, extension, kind) =
            music_to_playable(song_name(), &mus_fixture()).expect("the MUS fixture converts");
        assert_eq!(extension, "mid", "the playable container is SMF");
        assert_eq!(kind, MusicKind::Mus, "the input kind is reported");
        assert!(midi.starts_with(b"MThd"), "the conversion writes SMF");
        assert!(
            midi.windows(2).any(|pair| pair == [0x90_u8, 0x3C]),
            "the note-on event survives: {midi:02X?}"
        );
        let raw = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01".to_vec();
        let (through, smf_ext, smf_kind) =
            music_to_playable(song_name(), &raw).expect("MIDI is already playable");
        assert_eq!(through, raw, "the bytes are unchanged");
        assert_eq!(smf_ext, "mid", "the same container");
        assert_eq!(smf_kind, MusicKind::Midi, "reported as the input kind");
    }

    #[test]
    fn containers_and_modules_pass_through_under_their_own_extension() {
        let mut ogg = b"OggS\x00\x01".to_vec();
        ogg.extend_from_slice(&[0_u8; 20]);
        for (bytes, want) in [
            (ogg.clone(), ("ogg", MusicKind::Ogg)),
            (b"ID3\x04\0rest".to_vec(), ("mp3", MusicKind::Mp3)),
            (b"fLaC\0\0meta".to_vec(), ("flac", MusicKind::Flac)),
            (b"RIFF\0\0\0\0WAVEfmt ".to_vec(), ("wav", MusicKind::Wav)),
            (b"Extended Module: XModule".to_vec(), ("xm", MusicKind::Xm)),
            (b"IMPM\0\0\0\0body".to_vec(), ("it", MusicKind::It)),
        ] {
            let (data, extension, kind) =
                music_to_playable(song_name(), &bytes).expect("a recognised container plays");
            assert_eq!(data, bytes, "byte for byte for {want:?}");
            assert_eq!(extension, want.0, "extension for {want:?}");
            assert_eq!(kind, want.1, "kind for {want:?}");
        }
        let mut s3m = vec![0_u8; 48];
        s3m[44..48].copy_from_slice(b"SCRM");
        let mut mod_bytes = vec![0_u8; 1084];
        mod_bytes[1080..1084].copy_from_slice(b"M.K.");
        for (bytes, want) in [
            (s3m, ("s3m", MusicKind::S3m)),
            (mod_bytes, ("mod", MusicKind::Mod)),
        ] {
            let (data, extension, kind) =
                music_to_playable(song_name(), &bytes).expect("a module plays");
            assert_eq!(data, bytes, "byte for byte for {want:?}");
            assert_eq!(extension, want.0, "extension for {want:?}");
            assert_eq!(kind, want.1, "kind for {want:?}");
        }
    }

    #[test]
    fn unrecognised_music_refuses_naming_the_lump() {
        let error = music_to_playable(song_name(), b"total nonsense").expect_err("no magic here");
        assert_eq!(error.code(), "unsupported", "unrecognised code");
        let message = error.to_string();
        assert!(
            message.contains("D_TEST") && message.contains("not recognised music"),
            "the message names the lump and the way out: {message}"
        );
        assert!(
            message.contains("get_lump format=url"),
            "and the raw fallback: {message}"
        );
    }

    #[test]
    fn a_corrupt_mus_score_keeps_the_validator_s_message() {
        let corrupt_error =
            music_to_playable(song_name(), b"MUS\x1A").expect_err("a truncated MUS");
        assert_eq!(
            corrupt_error.code(),
            "format_error",
            "the converter's code survives"
        );
    }
}
