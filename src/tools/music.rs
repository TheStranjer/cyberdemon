//! Music tools: bringing external music files into the working archive.
//!
//! The first tool of the group is [`Cyberdemon::import_music`]: fetch a
//! music file through [`AppState::fetch_source`], identify its container
//! with [`crate::music::sniff_music`] and either store a vanilla `MUS`
//! `D_*` lump (converting `MIDI` with [`midi_to_mus`], validating `MUS`
//! with [`mus_to_midi`]) or keep the original bytes unchanged.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::fetch::derive_lump_name;
use crate::error::CyberError;
use crate::music::{MusicKind, midi_to_mus, mus_to_midi, sniff_music};
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::{fail, ok};
use crate::wad::archive::{Lump, Wad};
use crate::wad::name::{LumpName, MAX_NAME_LEN};

/// Prefix vanilla `Doom`'s music player puts in front of every song name;
/// the engine looks songs up as `D_<name>` with no `MAPINFO` entry.
const MUSIC_PREFIX: &str = "D_";
/// Characters left for the derived stem once the prefix took its share of
/// the eight-byte lump-name field.
const MUSIC_HEAD: usize = 6;

/// Arguments for [`Cyberdemon::import_music`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportMusicParams {
    /// Music file to import: absolute path, `file://` URI, `http(s)://`
    /// URL or an uploaded asset URL. `MUS`, `MIDI`, `OGG`, `MP3`, `FLAC`,
    /// `WAV`, `XM`, `IT`, `S3M` and `MOD` are recognised from their bytes.
    source: String,
    /// Lump name (1 to 8 printable ASCII characters, uppercased). Defaults
    /// to `D_` plus the file stem uppercased and trimmed to 8 characters:
    /// `runnin.mid` becomes `D_RUNNIN`.
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

/// The name to store under: the explicit one, else `D_` plus the derived,
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
            "cannot derive a music name from {filename:?}: pass name (1 to {MAX_NAME_LEN} \
             printable ASCII characters)"
        ))
    })?;
    let head: String = stem.to_string().chars().take(MUSIC_HEAD).collect();
    LumpName::parse(&format!("{MUSIC_PREFIX}{head}"))
}

/// The synchronous half: one undoable insert (or in-place replace) of the
/// music lump in the global area of the working archive.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] for a working lump of the same name when
/// `replace` is `false`.
fn commit(
    session: &mut Session,
    name: LumpName,
    data: Vec<u8>,
    replace: bool,
) -> Result<(), CyberError> {
    let label = format!("import_music {name}");
    session.edit(&label, |wad| place_music(wad, name, data, replace))
}

/// Put the song at the end of `wad`, replacing the last same-name lump
/// when one exists and `replace` allows it. Music has no namespace.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] as by [`commit`].
fn place_music(
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

/// Run `import_music` end to end: fetch, sniff, convert and commit.
///
/// # Errors
///
/// [`plan`], [`AppState::fetch_source`], [`store_payload`], [`chosen_name`]
/// and [`commit`] errors pass through.
async fn import(
    state: &AppState,
    args: &ImportMusicParams,
) -> Result<ImportMusicReport, CyberError> {
    let plan = plan(args)?;
    let fetched = state.fetch_source(&args.source).await?;
    let kind = sniff_music(&fetched.bytes);
    let format = chosen_format(&plan, kind);
    let stored = store_payload(format, kind, fetched.bytes)?;
    let name = chosen_name(&plan, &fetched.filename)?;
    let bytes = stored.data.len();
    let warnings = stored.warnings;
    state.with_session(|session| commit(session, name, stored.data, plan.replace))?;
    Ok(ImportMusicReport {
        name,
        input_format: kind,
        stored_format: format,
        bytes,
        warnings,
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
        description = "Imports a music file as a lump in the working archive, as one undoable edit labelled \"import_music NAME\" (save_wad writes the result). source: absolute path, file:// URI, http(s):// URL or this server's asset URL. The input container is sniffed from the bytes: MUS, MIDI, OGG, MP3, FLAC, WAV, XM, IT, S3M and MOD are recognised; anything else is a format_error. format \"mus\" stores a vanilla Doom MUS lump: MIDI is converted (the result's warnings list the MIDI events MUS cannot carry), MUS is validated and stored unchanged; any other input is invalid_argument. format \"passthrough\" stores the original file bytes unchanged. Unset format defaults to \"mus\" for MIDI or MUS input and \"passthrough\" otherwise. Vanilla Doom plays only MUS from D_* lumps (Heretic uses MUS_* names; Hexen map music names come from MAPINFO), while ZDoom-family ports also play MIDI, OGG, MP3, FLAC and tracker module lumps. name defaults to \"D_\" plus the file stem uppercased and trimmed to 8 characters (runnin.mid -> D_RUNNIN); when nothing usable can be derived the call fails asking for this name. A working lump of the same name is already_exists unless replace=true (replace overwrites the last copy in place). Returns {name, input_format, stored_format, bytes, warnings}."
    )]
    async fn import_music(
        &self,
        Parameters(args): Parameters<ImportMusicParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match import(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ImportMusicParams, MusicKind, Plan, StorageFormat, sniff_music};
    use super::{chosen_format, chosen_name, parse_format, plan, store_payload};

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

    #[test]
    fn names_default_to_the_prefix_plus_a_trimmed_stem() {
        let blank = Plan {
            name: None,
            format: None,
            replace: false,
        };
        assert_eq!(
            chosen_name(&blank, "runnin.mid")
                .expect("derivable")
                .to_string(),
            "D_RUNNIN",
            "the ticket's example"
        );
        assert_eq!(
            chosen_name(&blank, "E1M2 theme.mid")
                .expect("derivable")
                .to_string(),
            "D_E1M2TH",
            "space dropped, stem trimmed to six"
        );
        let explicit = Plan {
            name: super::LumpName::parse("MYTUNE").ok(),
            format: None,
            replace: false,
        };
        assert_eq!(
            chosen_name(&explicit, "runnin.mid")
                .expect("explicit")
                .to_string(),
            "MYTUNE",
            "an explicit name wins over derivation"
        );
        let error = chosen_name(&blank, ".mid").expect_err("nothing usable");
        assert_eq!(error.code(), "invalid_argument", "unnameable source code");
        assert!(
            error.to_string().contains("pass name"),
            "the message asks for the name: {error}"
        );
    }
}
