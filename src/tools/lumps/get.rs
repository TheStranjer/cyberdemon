//! `get_lump`: its arguments, the format they ask for and the body that
//! format delivers.
//!
//! [`GetLumpParams`] is the wire form; [`parse_get_options`] reads it into
//! [`GetOptions`], with [`parse_format`] picking the [`Requested`] delivery
//! and [`parse_hex_length`] the `hex` window. [`run_get_lump`] resolves the
//! shared selector, [`decide`] settles which [`Applied`] format `auto` means
//! ([`is_text_kind`] for the kind, [`asset_naming`] over [`extension_for`]
//! for the published name) and [`text_body`], [`hex_body`] over [`hex_of`]
//! or the [`Pending`] publish job builds the [`Body`]. [`deliver`] finishes
//! that job outside the session lock and assembles the [`GetReport`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::assets::store::mime_for_extension;
use crate::error::CyberError;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::lumps::rows::LumpRow;
use crate::tools::lumps::select::{LumpSelector, resolve};
use crate::wad::classify::LumpKind;
use crate::wad::name::LumpName;

use super::{DEFAULT_HEX_LENGTH, DEFAULT_MAX_CHARS, HEX_PREVIEW_BYTES, MAX_HEX_LENGTH};

/// Arguments for [`Cyberdemon::get_lump`]: the shared selector plus the
/// output format and its windowing.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetLumpParams {
    /// The lump selector: `name` plus optional `scope`, `namespace` and
    /// `occurrence` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    pub(super) selector: LumpSelector,
    /// How to deliver the payload: `"auto"` (the default), `"info"`,
    /// `"text"`, `"hex"` or `"url"`.
    #[serde(default)]
    pub(super) format: Option<String>,
    /// First payload byte of the `hex` window (default 0).
    #[serde(default)]
    pub(super) start: Option<usize>,
    /// Bytes requested by the `hex` window (default 256, at most 4096);
    /// the window is clipped to the payload.
    #[serde(default)]
    pub(super) length: Option<usize>,
    /// Largest character count of a `text` body (default 100000).
    #[serde(default)]
    pub(super) max_chars: Option<usize>,
}

/// The `get_lump` `format` parameter, as requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Requested {
    /// Let the lump's kind and size pick the delivery.
    Auto,
    /// Row fields only.
    Info,
    /// UTF-8 text with flags.
    Text,
    /// A windowed hex dump.
    Hex,
    /// Publish the raw bytes as an asset.
    Url,
}

/// The `get_lump` format that actually ran, after resolving `auto`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Applied {
    /// Row fields only.
    Info,
    /// UTF-8 text with flags.
    Text,
    /// A windowed hex dump.
    Hex,
    /// A published asset reference.
    Url,
}

/// Kinds the `auto` format delivers as text.
const fn is_text_kind(kind: LumpKind) -> bool {
    matches!(
        kind,
        LumpKind::Text
            | LumpKind::Dehacked
            | LumpKind::Decorate
            | LumpKind::Zscript
            | LumpKind::Mapinfo
    )
}

/// The file extension `get_lump` picks for a published lump of `kind`
/// (without the dot).
const fn extension_for(kind: LumpKind) -> &'static str {
    match kind {
        LumpKind::Png => "png",
        LumpKind::Jpeg => "jpg",
        LumpKind::Gif => "gif",
        LumpKind::Wav => "wav",
        LumpKind::Ogg => "ogg",
        LumpKind::Mp3 => "mp3",
        LumpKind::Flac => "flac",
        LumpKind::Midi => "mid",
        LumpKind::Mus => "mus",
        LumpKind::Text
        | LumpKind::Dehacked
        | LumpKind::Decorate
        | LumpKind::Zscript
        | LumpKind::Mapinfo => "txt",
        LumpKind::Marker
        | LumpKind::MapMarker
        | LumpKind::MapData
        | LumpKind::Palette
        | LumpKind::Colormap
        | LumpKind::Pnames
        | LumpKind::TextureDefs
        | LumpKind::Endoom
        | LumpKind::Demo
        | LumpKind::DoomSound
        | LumpKind::PcSpeaker
        | LumpKind::Flat
        | LumpKind::Sprite
        | LumpKind::Patch
        | LumpKind::Empty
        | LumpKind::Unknown => "lmp",
    }
}

/// The asset file name and media type for publishing `row`'s lump.
pub(super) fn asset_naming(row: &LumpRow) -> (String, &'static str) {
    let extension = extension_for(row.kind);
    let name = row.name;
    (format!("{name}.{extension}"), mime_for_extension(extension))
}

/// `get_lump`'s format and windowing, parsed from its arguments.
pub(super) struct GetOptions {
    /// The requested format.
    pub(super) requested: Requested,
    /// First byte of the `hex` window.
    pub(super) start: usize,
    /// Requested `hex` window length.
    pub(super) length: usize,
    /// Character cap for a `text` body.
    pub(super) max_chars: usize,
}

/// The payload half of a `get_lump` result, flattened after the row.
///
/// One struct with every format's fields optional: an unapplied format
/// leaves its fields `None` and out of the result, `Info` adds nothing at
/// all.
#[derive(Debug, Default, Serialize)]
pub(super) struct Body {
    /// The UTF-8 text, invalid sequences replaced (`text`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) text: Option<String>,
    /// Whether any invalid sequence was replaced (`text`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) lossy: Option<bool>,
    /// Whether `max_chars` cut the text (`text`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) truncated: Option<bool>,
    /// The window's bytes as space-separated uppercase hex (`hex`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hex: Option<String>,
    /// First byte of the hex window (`hex`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) start: Option<usize>,
    /// Bytes actually in the hex window (`hex`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) length: Option<usize>,
    /// The published raw bytes (`url`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) asset: Option<AssetRef>,
    /// First [`HEX_PREVIEW_BYTES`] bytes as hex; only the `auto` format
    /// attaches it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) preview_hex: Option<String>,
}

/// What [`run_get_lump`] settled on: a finished body, or bytes still
/// waiting to be published once the session lock is released.
pub(super) enum Pending {
    /// The body needs no publishing.
    Done(Body),
    /// Publish these bytes as `filename` with `mime`, attaching a
    /// `preview_hex` when `preview` says so.
    Publish {
        /// The raw payload.
        data: Vec<u8>,
        /// File name to publish under.
        filename: String,
        /// Media type for the file extension.
        mime: &'static str,
        /// Whether this came from `auto` and needs a preview.
        preview: bool,
    },
}

/// A resolved lump plus the body (or publish job) its format asks for.
pub(super) struct GetOutcome {
    /// The selected row.
    row: LumpRow,
    /// The format that ran.
    format: Applied,
    /// The finished body or the pending publish.
    pending: Pending,
}

/// Structured result of [`Cyberdemon::get_lump`]: the shared row, the
/// applied format and the payload fields.
#[derive(Serialize)]
pub(super) struct GetReport {
    /// The `list_lumps` row, flattened into the object.
    #[serde(flatten)]
    row: LumpRow,
    /// Which format ran.
    format: Applied,
    /// The format's extra fields, flattened into the object.
    #[serde(flatten)]
    body: Body,
}

/// The `get_lump` `format` parameter, defaulting to [`Requested::Auto`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the accepted forms.
pub(super) fn parse_format(text: Option<&str>) -> Result<Requested, CyberError> {
    let Some(raw) = text else {
        return Ok(Requested::Auto);
    };
    match raw.to_ascii_lowercase().as_str() {
        "auto" => Ok(Requested::Auto),
        "info" => Ok(Requested::Info),
        "text" => Ok(Requested::Text),
        "hex" => Ok(Requested::Hex),
        "url" => Ok(Requested::Url),
        other => Err(CyberError::InvalidArgument(format!(
            "unknown format {other:?}: use \"info\", \"text\", \"hex\" or \"url\", or \"auto\" \
             to let the lump's kind pick"
        ))),
    }
}

/// The `hex` window length, the default being [`DEFAULT_HEX_LENGTH`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for anything above [`MAX_HEX_LENGTH`].
pub(super) fn parse_hex_length(value: Option<usize>) -> Result<usize, CyberError> {
    let Some(length) = value else {
        return Ok(DEFAULT_HEX_LENGTH);
    };
    if length > MAX_HEX_LENGTH {
        return Err(CyberError::InvalidArgument(format!(
            "hex length {length} exceeds the maximum of {MAX_HEX_LENGTH}: pass a smaller \
             length (the window is clipped to the payload anyway)"
        )));
    }
    Ok(length)
}

/// The `get_lump` format and windowing, parsed from its arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad `format` or `length`.
pub(super) fn parse_get_options(args: &GetLumpParams) -> Result<GetOptions, CyberError> {
    Ok(GetOptions {
        requested: parse_format(args.format.as_deref())?,
        start: args.start.unwrap_or(0_usize),
        length: parse_hex_length(args.length)?,
        max_chars: args.max_chars.unwrap_or(DEFAULT_MAX_CHARS),
    })
}

/// The format that runs for `requested` on this row: `auto` delivers
/// text-like kinds as text, empty lumps as info, everything else as a
/// published url.
pub(super) const fn decide(requested: Requested, row: &LumpRow) -> Applied {
    match requested {
        Requested::Auto => {
            if is_text_kind(row.kind) {
                Applied::Text
            } else if row.size == 0 {
                Applied::Info
            } else {
                Applied::Url
            }
        }
        Requested::Info => Applied::Info,
        Requested::Text => Applied::Text,
        Requested::Hex => Applied::Hex,
        Requested::Url => Applied::Url,
    }
}

/// `data` as space-separated uppercase hex, e.g. `"4D 55 53 1A"`.
fn hex_of(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `text` body: `data` decoded as UTF-8 with invalid sequences
/// replaced, cut at `max_chars`.
pub(super) fn text_body(data: &[u8], max_chars: usize) -> Body {
    let decoded = String::from_utf8_lossy(data);
    let text: String = decoded.chars().take(max_chars).collect();
    Body {
        lossy: Some(core::str::from_utf8(data).is_err()),
        truncated: Some(decoded.chars().count() > max_chars),
        text: Some(text),
        ..Body::default()
    }
}

/// The `hex` body: the `start..start + length` window of `data`, clipped
/// to the payload.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `start` is past the end of `data`.
pub(super) fn hex_body(
    data: &[u8],
    name: LumpName,
    start: usize,
    length: usize,
) -> Result<Body, CyberError> {
    if start > data.len() {
        return Err(CyberError::InvalidArgument(format!(
            "hex start {start} is past the end of lump {name} ({} bytes): pass a start within \
             the payload",
            data.len()
        )));
    }
    let end = start
        .checked_add(length)
        .map_or(data.len(), |wanted| wanted.min(data.len()));
    let window = data.get(start..end).unwrap_or(&[]);
    Ok(Body {
        hex: Some(hex_of(window)),
        start: Some(start),
        length: Some(window.len()),
        ..Body::default()
    })
}

/// Resolve the selector and build the body (or the pending publish job)
/// its format asks for.
///
/// # Errors
///
/// [`resolve`]'s errors, [`hex_body`]'s for a `start` past the payload.
pub(super) fn run_get_lump(
    session: &Session,
    selector: &LumpSelector,
    opts: &GetOptions,
) -> Result<GetOutcome, CyberError> {
    let row = resolve(session, selector)?;
    let format = decide(opts.requested, &row);
    let data = &row.lump.data;
    let pending = match format {
        Applied::Info => Pending::Done(Body::default()),
        Applied::Text => Pending::Done(text_body(data, opts.max_chars)),
        Applied::Hex => Pending::Done(hex_body(data, row.name, opts.start, opts.length)?),
        Applied::Url => {
            let (filename, mime) = asset_naming(&row);
            Pending::Publish {
                data: data.to_vec(),
                filename,
                mime,
                preview: opts.requested == Requested::Auto,
            }
        }
    };
    Ok(GetOutcome {
        row,
        format,
        pending,
    })
}

/// Finish an outcome outside the session lock: publish pending bytes and
/// assemble the report.
///
/// # Errors
///
/// [`AppState::publish`]'s errors when the asset cannot be stored.
pub(super) fn deliver(state: &AppState, outcome: GetOutcome) -> Result<GetReport, CyberError> {
    let GetOutcome {
        row,
        format,
        pending,
    } = outcome;
    let body = match pending {
        Pending::Done(body) => body,
        Pending::Publish {
            data,
            filename,
            mime,
            preview,
        } => {
            let head = data.get(..HEX_PREVIEW_BYTES).unwrap_or(&data);
            let preview_hex = preview.then(|| hex_of(head));
            let asset = state.publish(&filename, mime, data)?;
            Body {
                asset: Some(asset),
                preview_hex,
                ..Body::default()
            }
        }
    };
    Ok(GetReport { row, format, body })
}
