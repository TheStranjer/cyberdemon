//! Directory browsing tools: [`Cyberdemon::list_lumps`] and
//! [`Cyberdemon::search_lumps`], built on the shared rows in
//! [`crate::tools::lumps::rows`], the single-lump readers
//! [`Cyberdemon::get_lump`] and [`Cyberdemon::extract_lump`], built on the
//! shared selector in [`crate::tools::lumps::select`], and the editing
//! tools [`Cyberdemon::add_lump`], [`Cyberdemon::import_asset`],
//! [`Cyberdemon::import_file_as_lump`], [`Cyberdemon::replace_lump`],
//! [`Cyberdemon::remove_lump`] and [`Cyberdemon::rename_lump`], built on the
//! shared placement in [`crate::tools::lumps::place`] and the argument and
//! mutation logic in [`crate::tools::lumps::edit`].

mod edit;
mod place;
mod rows;
mod select;

use std::path::{Path, PathBuf};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::fetch::derive_lump_name;
use crate::assets::server::AssetRef;
use crate::assets::store::mime_for_extension;
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::lumps::edit::{RemoveLumpParams, RenameLumpParams, ReplaceLumpParams};
use crate::tools::lumps::place::{Placement, PlacementParams, parse_placement};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::lumps::select::{LumpSelector, resolve};
use crate::tools::{fail, ok};
use crate::wad::archive::{Lump, write_file_atomic};
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::LumpName;

/// Default page size for both tools.
const DEFAULT_LIMIT: usize = 200;
/// Largest page either tool will return.
const MAX_LIMIT: usize = 2000;
/// Default byte count of a `get_lump` `hex` window.
const DEFAULT_HEX_LENGTH: usize = 256;
/// Largest byte count a `get_lump` `hex` window accepts.
const MAX_HEX_LENGTH: usize = 4_096;
/// Bytes of `preview_hex` the `auto` format attaches to published lumps.
const HEX_PREVIEW_BYTES: usize = 64;
/// Default character cap for a `get_lump` `text` body.
const DEFAULT_MAX_CHARS: usize = 100_000;

/// Arguments for [`Cyberdemon::list_lumps`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ListLumpsParams {
    /// Which archive to list: `"merged"` (the default: the directory an
    /// engine sees), `"working"` (the editable archive alone) or
    /// `"layer:N"` (read-only layer `N`, 0 is the IWAD).
    #[serde(default)]
    scope: Option<String>,
    /// Keep only lumps inside this namespace region: `sprites`, `flats`,
    /// `patches`, `colormaps`, `acs`, `voices`, `hires` or `textures`.
    #[serde(default)]
    namespace: Option<String>,
    /// Keep only lumps with this type hint, e.g. `map_marker`, `map_data`,
    /// `flat`, `sprite`, `patch`, `palette`, `doom_sound` (every value is
    /// listed in `skill/reference/lumps.md`).
    #[serde(default)]
    kind: Option<String>,
    /// Keep only the header and data lumps of this map block, e.g.
    /// `"MAP01"`.
    #[serde(default)]
    map: Option<String>,
    /// Matching lumps to skip before the page (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    limit: Option<usize>,
}

/// Arguments for [`Cyberdemon::search_lumps`]. At least one of `pattern`,
/// `kind`, `namespace`, `contains_text` or `contains_hex` is required.
#[derive(Debug, Deserialize, JsonSchema)]
struct SearchLumpsParams {
    /// Case-insensitive lump-name glob: `*` matches any run of characters,
    /// `?` exactly one, e.g. `"FLOOR*"` or `"?LOOR1"`.
    #[serde(default)]
    pattern: Option<String>,
    /// Keep only lumps with this type hint, e.g. `palette`, `flat`,
    /// `doom_sound` (every value is listed in
    /// `skill/reference/lumps.md`).
    #[serde(default)]
    kind: Option<String>,
    /// Keep only lumps inside this namespace region: `sprites`, `flats`,
    /// `patches`, `colormaps`, `acs`, `voices`, `hires` or `textures`.
    #[serde(default)]
    namespace: Option<String>,
    /// Keep only lumps whose payload contains this text, matched
    /// case-insensitively on `ASCII` bytes.
    #[serde(default)]
    contains_text: Option<String>,
    /// Keep only lumps whose payload contains these bytes: a hex string
    /// with optional spaces, e.g. `"4D 55 53 1A"`.
    #[serde(default)]
    contains_hex: Option<String>,
    /// Which archive to search: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
    /// Maximum rows in the result (default 200, at most 2000); `truncated`
    /// says whether more matched.
    #[serde(default)]
    limit: Option<usize>,
}

/// Arguments for [`Cyberdemon::get_lump`]: the shared selector plus the
/// output format and its windowing.
#[derive(Debug, Deserialize, JsonSchema)]
struct GetLumpParams {
    /// The lump selector: `name` plus optional `scope`, `namespace` and
    /// `occurrence` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    selector: LumpSelector,
    /// How to deliver the payload: `"auto"` (the default), `"info"`,
    /// `"text"`, `"hex"` or `"url"`.
    #[serde(default)]
    format: Option<String>,
    /// First payload byte of the `hex` window (default 0).
    #[serde(default)]
    start: Option<usize>,
    /// Bytes requested by the `hex` window (default 256, at most 4096);
    /// the window is clipped to the payload.
    #[serde(default)]
    length: Option<usize>,
    /// Largest character count of a `text` body (default 100000).
    #[serde(default)]
    max_chars: Option<usize>,
}

/// Arguments for [`Cyberdemon::extract_lump`]: the shared selector plus the
/// destination file.
#[derive(Debug, Deserialize, JsonSchema)]
struct ExtractLumpParams {
    /// The lump selector: `name` plus optional `scope`, `namespace` and
    /// `occurrence` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    selector: LumpSelector,
    /// Absolute filesystem path to write the lump's raw bytes to.
    path: String,
    /// Replace the target file when it already exists (default `false`).
    #[serde(default)]
    overwrite: bool,
}

/// Arguments for [`Cyberdemon::add_lump`]: the name, at most one payload
/// field, the shared placement and the duplicate rule.
#[derive(Debug, Deserialize, JsonSchema)]
struct AddLumpParams {
    /// Name for the new lump: 1 to 8 printable ASCII characters, matched
    /// case-insensitively (it is uppercased).
    name: String,
    /// Payload as UTF-8 text, written as-is. At most one of `text`, `hex`
    /// or `source`.
    #[serde(default)]
    text: Option<String>,
    /// Payload as hex bytes, spaces allowed (e.g. `"4D 55 53 1A"`). At
    /// most one of `text`, `hex` or `source`.
    #[serde(default)]
    hex: Option<String>,
    /// Absolute path, `file://` URI, `http(s)://` URL or uploaded asset
    /// URL whose bytes become the payload. At most one of `text`, `hex`
    /// or `source`.
    #[serde(default)]
    source: Option<String>,
    /// Where the lump goes: at most one of `index`, `before`, `after`,
    /// `namespace` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    placement: PlacementParams,
    /// Add the lump even when the working archive already holds one with
    /// this name in the same namespace (default `false`).
    #[serde(default)]
    allow_duplicate: bool,
}

/// Arguments for [`Cyberdemon::import_asset`] and
/// [`Cyberdemon::import_file_as_lump`]: the source to fetch, an optional
/// name, the shared placement and the duplicate rule.
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportParams {
    /// Source to fetch: absolute path, `file://` URI, `http(s)://` URL or
    /// an uploaded asset URL. Its bytes are inserted without conversion.
    source: String,
    /// Lump name to store the bytes under; defaults to the fetched file
    /// name's stem uppercased and trimmed to 8 characters. When nothing
    /// usable remains the call fails asking for this name.
    #[serde(default)]
    name: Option<String>,
    /// Where the lump goes: at most one of `index`, `before`, `after`,
    /// `namespace` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    placement: PlacementParams,
    /// Import even when the working archive already holds one with this
    /// name in the same namespace (default `false`).
    #[serde(default)]
    allow_duplicate: bool,
}

/// The parsed filters and paging of one `list_lumps` call.
struct Listing {
    /// Archive to list.
    scope: Scope,
    /// Namespace to keep, if any.
    namespace: Option<Namespace>,
    /// Type hint to keep, if any.
    kind: Option<LumpKind>,
    /// Map block to keep, if any.
    map: Option<LumpName>,
    /// Matching rows to skip.
    offset: usize,
    /// Largest page to return.
    limit: usize,
}

impl Listing {
    /// Whether a row passes every filter that was set.
    fn accepts(&self, row: &LumpRow) -> bool {
        self.namespace
            .is_none_or(|wanted| row.namespace == Some(wanted))
            && self.kind.is_none_or(|wanted| row.kind == wanted)
            && self.map.is_none_or(|wanted| row.map == Some(wanted))
    }
}

/// One content needle of a [`Search`], with its case rule.
struct Needle<'crit> {
    /// The bytes to look for.
    bytes: &'crit [u8],
    /// Whether the comparison folds `ASCII` case on both sides.
    ignore_case: bool,
}

/// The parsed criteria of one `search_lumps` call.
struct Search {
    /// Case-insensitive name glob, if given.
    pattern: Option<String>,
    /// Type hint to keep, if given.
    kind: Option<LumpKind>,
    /// Namespace to keep, if given.
    namespace: Option<Namespace>,
    /// Upper-cased text needle, if given.
    text: Option<Vec<u8>>,
    /// Needle bytes decoded from the hex string, if given.
    hex: Option<Vec<u8>>,
}

/// How [`Search::judge`] decided about one row.
enum Match {
    /// The row matches; the payload offset of the first content match,
    /// `None` when no content criterion was given.
    Hit(Option<usize>),
    /// The row fails some criterion and is dropped.
    Miss,
}

impl Search {
    /// Whether the caller gave no criterion at all.
    const fn is_empty(&self) -> bool {
        self.pattern.is_none()
            && self.kind.is_none()
            && self.namespace.is_none()
            && self.text.is_none()
            && self.hex.is_none()
    }

    /// Judge one row against every criterion of this search.
    fn judge(&self, row: &LumpRow) -> Match {
        if self
            .pattern
            .as_deref()
            .is_some_and(|glob| !row.name.matches_glob(glob))
        {
            return Match::Miss;
        }
        if self.kind.is_some_and(|kind| row.kind != kind)
            || self
                .namespace
                .is_some_and(|namespace| row.namespace != Some(namespace))
        {
            return Match::Miss;
        }
        self.content_match(row)
    }

    /// The verdict of the content criteria for one row's payload: every
    /// needle must occur, and the lowest offset among them wins.
    fn content_match(&self, row: &LumpRow) -> Match {
        let offsets: Option<Vec<usize>> = self
            .needles()
            .map(|needle| find_needle(&row.lump.data, &needle))
            .collect();
        offsets.map_or(Match::Miss, |positions| {
            Match::Hit(positions.into_iter().min())
        })
    }

    /// The content needles to look for, text first, each a borrow of this.
    fn needles(&self) -> impl Iterator<Item = Needle<'_>> {
        let text = self.text.as_deref().map(|bytes| Needle {
            bytes,
            ignore_case: true,
        });
        let hex = self.hex.as_deref().map(|bytes| Needle {
            bytes,
            ignore_case: false,
        });
        text.into_iter().chain(hex)
    }
}

/// The first offset in `data` where `needle` occurs, `None` when it does
/// not. An empty needle matches at offset 0.
fn find_needle(data: &[u8], needle: &Needle<'_>) -> Option<usize> {
    if needle.bytes.is_empty() {
        return Some(0);
    }
    if needle.ignore_case {
        let folded = data.to_ascii_uppercase();
        return folded
            .windows(needle.bytes.len())
            .position(|window| window == needle.bytes);
    }
    data.windows(needle.bytes.len())
        .position(|window| window == needle.bytes)
}

/// The value of one hex digit, `None` for any other character.
fn hex_digit(ch: char) -> Option<u8> {
    ch.to_digit(16).and_then(|value| u8::try_from(value).ok())
}

/// The byte two hex characters spell, `None` unless the slice is exactly
/// two characters and both are hex digits.
fn parse_pair(pair: &[char]) -> Option<u8> {
    let &[high, low] = pair else {
        return None;
    };
    let (Some(h), Some(l)) = (hex_digit(high), hex_digit(low)) else {
        return None;
    };
    h.checked_mul(16)?.checked_add(l)
}

/// Structured result of [`Cyberdemon::list_lumps`].
#[derive(Serialize)]
struct ListReport {
    /// The scope that was listed.
    scope: Scope,
    /// Lumps matching the filters, before paging.
    total: usize,
    /// Rows skipped before this page.
    offset: usize,
    /// Rows in this page.
    count: usize,
    /// The page, in directory order.
    lumps: Vec<LumpRow>,
}

/// One `search_lumps` hit: the shared row plus the content match offset.
#[derive(Serialize)]
struct SearchRow {
    /// The directory row, flattened into the object.
    #[serde(flatten)]
    row: LumpRow,
    /// Byte offset of the first `contains_text`/`contains_hex` match
    /// (lowest across both needles), `null` when neither was given.
    match_offset: Option<usize>,
}

/// Structured result of [`Cyberdemon::search_lumps`].
#[derive(Serialize)]
struct SearchReport {
    /// The scope that was searched.
    scope: Scope,
    /// Rows in the result.
    count: usize,
    /// Whether more lumps matched than `limit` allowed.
    truncated: bool,
    /// The hits, in directory order, capped at `limit`.
    lumps: Vec<SearchRow>,
}

/// The `get_lump` `format` parameter, as requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Requested {
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
enum Applied {
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
fn asset_naming(row: &LumpRow) -> (String, &'static str) {
    let extension = extension_for(row.kind);
    let name = row.name;
    (format!("{name}.{extension}"), mime_for_extension(extension))
}

/// `get_lump`'s format and windowing, parsed from its arguments.
struct GetOptions {
    /// The requested format.
    requested: Requested,
    /// First byte of the `hex` window.
    start: usize,
    /// Requested `hex` window length.
    length: usize,
    /// Character cap for a `text` body.
    max_chars: usize,
}

/// The payload half of a `get_lump` result, flattened after the row.
///
/// One struct with every format's fields optional: an unapplied format
/// leaves its fields `None` and out of the result, `Info` adds nothing at
/// all.
#[derive(Debug, Default, Serialize)]
struct Body {
    /// The UTF-8 text, invalid sequences replaced (`text`).
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// Whether any invalid sequence was replaced (`text`).
    #[serde(skip_serializing_if = "Option::is_none")]
    lossy: Option<bool>,
    /// Whether `max_chars` cut the text (`text`).
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    /// The window's bytes as space-separated uppercase hex (`hex`).
    #[serde(skip_serializing_if = "Option::is_none")]
    hex: Option<String>,
    /// First byte of the hex window (`hex`).
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<usize>,
    /// Bytes actually in the hex window (`hex`).
    #[serde(skip_serializing_if = "Option::is_none")]
    length: Option<usize>,
    /// The published raw bytes (`url`).
    #[serde(skip_serializing_if = "Option::is_none")]
    asset: Option<AssetRef>,
    /// First [`HEX_PREVIEW_BYTES`] bytes as hex; only the `auto` format
    /// attaches it.
    #[serde(skip_serializing_if = "Option::is_none")]
    preview_hex: Option<String>,
}

/// What [`run_get_lump`] settled on: a finished body, or bytes still
/// waiting to be published once the session lock is released.
enum Pending {
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
struct GetOutcome {
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
struct GetReport {
    /// The `list_lumps` row, flattened into the object.
    #[serde(flatten)]
    row: LumpRow,
    /// Which format ran.
    format: Applied,
    /// The format's extra fields, flattened into the object.
    #[serde(flatten)]
    body: Body,
}

/// Structured result of [`Cyberdemon::extract_lump`].
#[derive(Serialize)]
struct ExtractReport {
    /// The file that was written.
    path: String,
    /// Bytes in the file.
    bytes: usize,
}

/// The accepted `scope` text, the merged view being the default.
///
/// # Errors
///
/// [`Scope::parse`]'s [`CyberError::InvalidArgument`] for any other name.
fn parse_scope(text: Option<&str>) -> Result<Scope, CyberError> {
    text.map_or(Ok(Scope::Merged), Scope::parse)
}

/// Every `Namespace` name, for error hints.
fn namespace_names() -> String {
    Namespace::ALL
        .iter()
        .map(|namespace| namespace.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every `LumpKind` name, for error hints.
fn kind_names() -> String {
    LumpKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The namespace a filter keeps, [`Namespace::parse`] being
/// case-insensitive.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
fn parse_namespace(text: Option<&str>) -> Result<Option<Namespace>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    Namespace::parse(raw).map(Some).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "unknown namespace {raw:?}: expected one of {names}",
            names = namespace_names()
        ))
    })
}

/// The type hint a filter keeps, matched case-insensitively against
/// [`LumpKind::as_str`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
fn parse_kind(text: Option<&str>) -> Result<Option<LumpKind>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    LumpKind::parse(raw).map(Some).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "unknown kind {raw:?}: expected one of {names}",
            names = kind_names()
        ))
    })
}

/// The map block a filter keeps, parsed strictly as a lump name.
///
/// # Errors
///
/// [`LumpName::parse`]'s [`CyberError::InvalidArgument`] for a name that is
/// not 1 to 8 printable `ASCII` characters.
fn parse_map(text: Option<&str>) -> Result<Option<LumpName>, CyberError> {
    text.map(LumpName::parse).transpose()
}

/// The page size to apply, the default being [`DEFAULT_LIMIT`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `0` or anything above
/// [`MAX_LIMIT`].
fn parse_limit(value: Option<usize>) -> Result<usize, CyberError> {
    let Some(limit) = value else {
        return Ok(DEFAULT_LIMIT);
    };
    if limit == 0 || limit > MAX_LIMIT {
        return Err(CyberError::InvalidArgument(format!(
            "limit {limit} is out of range: pass a value between 1 and {MAX_LIMIT}"
        )));
    }
    Ok(limit)
}

/// The needle bytes of a `contains_hex` string: hex digits in pairs, with
/// any whitespace between them ignored.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a non-hex character or an odd
/// number of digits.
fn parse_hex(text: Option<&str>) -> Result<Option<Vec<u8>>, CyberError> {
    text.map(|raw| hex_bytes("contains_hex", raw)).transpose()
}

/// The bytes a hex argument spells: hex digits in pairs, whitespace
/// between them ignored; `field` names the argument in the errors.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a non-hex character or an odd
/// number of digits.
fn hex_bytes(field: &str, raw: &str) -> Result<Vec<u8>, CyberError> {
    let digits: Vec<char> = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
    let mut bytes = Vec::new();
    let mut pairs = digits.chunks_exact(2);
    for pair in pairs.by_ref() {
        let Some(byte) = parse_pair(pair) else {
            return Err(CyberError::InvalidArgument(format!(
                "{field} {raw:?} holds {pair:?}, which is not a hex pair (use 0-9 and a-f)"
            )));
        };
        bytes.push(byte);
    }
    if let Some(left) = pairs.remainder().first() {
        return Err(CyberError::InvalidArgument(format!(
            "{field} {raw:?} holds an odd number of hex digits ({left:?} is left over): write \
             whole bytes such as \"4D 55 53 1A\""
        )));
    }
    Ok(bytes)
}

/// The `get_lump` `format` parameter, defaulting to [`Requested::Auto`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the accepted forms.
fn parse_format(text: Option<&str>) -> Result<Requested, CyberError> {
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
fn parse_hex_length(value: Option<usize>) -> Result<usize, CyberError> {
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
fn parse_get_options(args: &GetLumpParams) -> Result<GetOptions, CyberError> {
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
const fn decide(requested: Requested, row: &LumpRow) -> Applied {
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
fn text_body(data: &[u8], max_chars: usize) -> Body {
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
fn hex_body(data: &[u8], name: LumpName, start: usize, length: usize) -> Result<Body, CyberError> {
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
fn run_get_lump(
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
fn deliver(state: &AppState, outcome: GetOutcome) -> Result<GetReport, CyberError> {
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

/// A filesystem path as display text, for reports.
fn path_text(path: &Path) -> String {
    path.display().to_string()
}

/// The destination of an `extract_lump` call: its `path` argument as a
/// [`PathBuf`], which must be absolute.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a relative path.
fn parse_target(raw: &str) -> Result<PathBuf, CyberError> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Err(CyberError::InvalidArgument(format!(
        "path {raw:?} is not absolute; pass the full filesystem path to write"
    )))
}

/// Refuse to replace an existing target unless `overwrite` allows it.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] when the target exists and `overwrite` is
/// `false`.
fn refuse_existing(target: &Path, overwrite: bool) -> Result<(), CyberError> {
    if !overwrite && target.try_exists().is_ok_and(|exists| exists) {
        return Err(CyberError::AlreadyExists(format!(
            "{} already exists: pass overwrite true to replace it",
            target.display()
        )));
    }
    Ok(())
}

/// Resolve the selector and copy out the raw bytes, refusing an existing
/// target unless `overwrite` says to replace it.
///
/// # Errors
///
/// [`resolve`]'s errors and [`refuse_existing`]'s.
fn extract_run(
    session: &Session,
    selector: &LumpSelector,
    target: &Path,
    overwrite: bool,
) -> Result<Vec<u8>, CyberError> {
    let row = resolve(session, selector)?;
    refuse_existing(target, overwrite)?;
    Ok(row.lump.data.to_vec())
}

/// The `list_lumps` filters and paging, parsed from its arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad `scope`, `namespace`, `kind`,
/// `map` or `limit`.
fn parse_listing(args: &ListLumpsParams) -> Result<Listing, CyberError> {
    Ok(Listing {
        scope: parse_scope(args.scope.as_deref())?,
        namespace: parse_namespace(args.namespace.as_deref())?,
        kind: parse_kind(args.kind.as_deref())?,
        map: parse_map(args.map.as_deref())?,
        offset: args.offset.unwrap_or(0_usize),
        limit: parse_limit(args.limit)?,
    })
}

/// List, filter and page the scope's rows.
fn list_run(session: &Session, listing: &Listing) -> Result<ListReport, CyberError> {
    let rows = build_rows(session, listing.scope)?;
    let matched: Vec<LumpRow> = rows
        .into_iter()
        .filter(|row| listing.accepts(row))
        .collect();
    let total = matched.len();
    let lumps: Vec<LumpRow> = matched
        .into_iter()
        .skip(listing.offset)
        .take(listing.limit)
        .collect();
    Ok(ListReport {
        scope: listing.scope,
        total,
        offset: listing.offset,
        count: lumps.len(),
        lumps,
    })
}

/// The `search_lumps` scope, paging and criteria.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad `scope`, `kind`, `namespace`,
/// `contains_hex` or `limit`, and when no criterion was given.
fn parse_search(args: &SearchLumpsParams) -> Result<(Scope, usize, Search), CyberError> {
    let search = Search {
        pattern: args.pattern.clone(),
        kind: parse_kind(args.kind.as_deref())?,
        namespace: parse_namespace(args.namespace.as_deref())?,
        text: args
            .contains_text
            .as_deref()
            .map(|needle| needle.as_bytes().to_ascii_uppercase()),
        hex: parse_hex(args.contains_hex.as_deref())?,
    };
    if search.is_empty() {
        return Err(CyberError::InvalidArgument(
            "search_lumps needs at least one criterion: pass pattern, kind, namespace, \
             contains_text or contains_hex"
                .to_owned(),
        ));
    }
    Ok((
        parse_scope(args.scope.as_deref())?,
        parse_limit(args.limit)?,
        search,
    ))
}

/// Judge every row of the scope and keep up to `limit` hits.
fn search_run(
    session: &Session,
    scope: Scope,
    limit: usize,
    search: &Search,
) -> Result<SearchReport, CyberError> {
    let rows = build_rows(session, scope)?;
    let hits: Vec<SearchRow> = rows
        .into_iter()
        .filter_map(|row| match search.judge(&row) {
            Match::Hit(match_offset) => Some(SearchRow { row, match_offset }),
            Match::Miss => None,
        })
        .collect();
    let truncated = hits.len() > limit;
    let lumps: Vec<SearchRow> = hits.into_iter().take(limit).collect();
    Ok(SearchReport {
        scope,
        count: lumps.len(),
        truncated,
        lumps,
    })
}

/// Hand an outcome to the client as the documented structured result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

/// How the caller supplied a new lump's bytes.
#[derive(Debug)]
enum Payload {
    /// No data field: the lump will be an empty marker.
    Empty,
    /// The `text` argument, kept as UTF-8 bytes.
    Text(String),
    /// The `hex` argument, waiting to be decoded.
    Hex(String),
    /// A `source` to fetch.
    Source(String),
}

/// A validated editing call whose payload may still need fetching.
#[derive(Debug)]
struct PendingEdit {
    /// The undo-label verb: `add_lump` or `import`.
    verb: &'static str,
    /// The lump name when the arguments carried one; imports otherwise
    /// derive it from the fetched file name.
    name: Option<LumpName>,
    /// Where the payload bytes come from.
    payload: Payload,
    /// Where the lump goes in the working archive.
    placement: Placement,
    /// Whether a same-name twin in the same namespace is allowed.
    allow_duplicate: bool,
    /// The `source` to echo back (the import tools only).
    source: Option<String>,
}

/// An editing call with its payload bytes in hand, ready to commit.
struct Prepared {
    /// The undo-label verb (`add_lump` or `import`).
    verb: &'static str,
    /// The name to insert under.
    name: LumpName,
    /// The payload bytes.
    data: Vec<u8>,
    /// Where the lump goes in the working archive.
    placement: Placement,
    /// Whether a same-name twin in the same namespace is allowed.
    allow_duplicate: bool,
    /// The `source` to echo back (the import tools only).
    source: Option<String>,
}

/// Structured result of the three editing tools.
#[derive(Debug, Serialize)]
struct EditReport {
    /// The name the lump was added under.
    name: LumpName,
    /// Its directory position in the working archive.
    index: usize,
    /// Payload bytes.
    size: usize,
    /// The region it sits in, `null` for a loose lump.
    namespace: Option<Namespace>,
    /// The fetched source; only the import tools carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

/// The at-most-one-of rule for the three `add_lump` payload fields.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when more than one is given.
fn pick_payload(
    text: Option<&str>,
    hex: Option<&str>,
    source: Option<&str>,
) -> Result<Payload, CyberError> {
    match (text, hex, source) {
        (None, None, None) => Ok(Payload::Empty),
        (Some(body), None, None) => Ok(Payload::Text(body.to_owned())),
        (None, Some(digits), None) => Ok(Payload::Hex(digits.to_owned())),
        (None, None, Some(raw)) => Ok(Payload::Source(raw.to_owned())),
        _ => Err(CyberError::InvalidArgument(
            "add_lump takes at most one of text, hex and source: pick one payload form".to_owned(),
        )),
    }
}

/// Validate [`Cyberdemon::add_lump`]'s arguments before any fetching.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad name, more than one payload
/// field or more than one placement field.
fn plan_add(args: &AddLumpParams) -> Result<PendingEdit, CyberError> {
    Ok(PendingEdit {
        verb: "add_lump",
        name: Some(LumpName::parse(&args.name)?),
        payload: pick_payload(
            args.text.as_deref(),
            args.hex.as_deref(),
            args.source.as_deref(),
        )?,
        placement: parse_placement(&args.placement)?,
        allow_duplicate: args.allow_duplicate,
        source: None,
    })
}

/// Validate the import tools' arguments before any fetching.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an explicit name that does not
/// parse or more than one placement field.
fn plan_import(args: &ImportParams) -> Result<PendingEdit, CyberError> {
    Ok(PendingEdit {
        verb: "import",
        name: args.name.as_deref().map(LumpName::parse).transpose()?,
        payload: Payload::Source(args.source.clone()),
        placement: parse_placement(&args.placement)?,
        allow_duplicate: args.allow_duplicate,
        source: Some(args.source.clone()),
    })
}

/// The name to insert under: the explicit one, else one derived from the
/// fetched file name.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] asking for `name` when nothing usable
/// can be derived.
fn chosen_name(name: Option<LumpName>, filename: Option<&str>) -> Result<LumpName, CyberError> {
    if let Some(explicit) = name {
        return Ok(explicit);
    }
    let file = filename.unwrap_or_default();
    derive_lump_name(file).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "cannot derive a lump name from {file:?}: pass name (1 to 8 printable ASCII \
             characters)"
        ))
    })
}

/// Bring the payload bytes in, fetching a `source` before the session lock
/// is ever taken.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for bad hex or an underivable import
/// name; [`AppState::fetch_source`]'s errors pass through.
async fn prepare(state: &AppState, pending: PendingEdit) -> Result<Prepared, CyberError> {
    let PendingEdit {
        verb,
        name,
        payload,
        placement,
        allow_duplicate,
        source,
    } = pending;
    let (data, filename) = match payload {
        Payload::Empty => (Vec::new(), None),
        Payload::Text(body) => (body.into_bytes(), None),
        Payload::Hex(digits) => (hex_bytes("hex", &digits)?, None),
        Payload::Source(raw) => {
            let fetched = state.fetch_source(&raw).await?;
            (fetched.bytes, Some(fetched.filename))
        }
    };
    Ok(Prepared {
        verb,
        name: chosen_name(name, filename.as_deref())?,
        data,
        placement,
        allow_duplicate,
        source,
    })
}

/// The `already_exists` text for a same-name twin, naming its region.
fn clash_message(name: LumpName, namespace: Option<Namespace>) -> String {
    let region = namespace.map_or_else(
        || "the global area".to_owned(),
        |wanted| format!("the {} region", wanted.as_str()),
    );
    format!(
        "lump {name} already exists in {region} of the working archive: pass allow_duplicate \
         true to add a second copy beside it or use replace_lump to overwrite the existing one"
    )
}

/// The synchronous half of an edit: one undoable insert into the working
/// archive plus the duplicate refusal (a refusal rolls the insert back).
///
/// # Errors
///
/// [`place::insert`]'s errors and [`CyberError::AlreadyExists`] for a
/// same-name twin in the same namespace when `allow_duplicate` is `false`.
fn apply(session: &mut Session, edit: &Prepared) -> Result<EditReport, CyberError> {
    let label = format!("{} {}", edit.verb, edit.name);
    let size = edit.data.len();
    let (index, namespace) = session.edit(&label, |wad| {
        let lump = Lump::new(edit.name, edit.data.clone());
        let at = place::insert(wad, &edit.placement, lump)?;
        let here = place::inspect(wad, at);
        if !edit.allow_duplicate && here.clashes {
            return Err(CyberError::AlreadyExists(clash_message(
                edit.name,
                here.namespace,
            )));
        }
        Ok((at, here.namespace))
    })?;
    Ok(EditReport {
        name: edit.name,
        index,
        size,
        namespace,
        source: edit.source.clone(),
    })
}

/// Run one editing tool end to end: fetch the payload (never holding the
/// session lock), then commit the undoable edit.
///
/// # Errors
///
/// Whatever [`prepare`] or [`apply`] reports.
async fn commit(state: &AppState, pending: PendingEdit) -> Result<EditReport, CyberError> {
    let prepared = prepare(state, pending).await?;
    state.with_session(|session| apply(session, &prepared))
}

/// The shared body of [`Cyberdemon::import_asset`] and
/// [`Cyberdemon::import_file_as_lump`].
async fn imported(state: &AppState, args: &ImportParams) -> Result<CallToolResult, ErrorData> {
    let pending = match plan_import(args) {
        Ok(pending) => pending,
        Err(error) => return fail(&error),
    };
    match commit(state, pending).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = lumps_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists one scope's lump directory with filters and paging.
    #[tool(
        description = "Lists a scope's lumps in directory order. scope is \"merged\" (the default: what an engine sees), \"working\" or \"layer:N\" (0 is the IWAD). Optional filters: namespace (sprites, flats, patches, colormaps, acs, voices, hires, textures), kind (a LumpKind type hint such as map_marker, flat, sprite, patch, palette, doom_sound), map (only that map block's lumps, e.g. \"MAP01\"). Paging: offset (default 0), limit (default 200, max 2000). Returns {scope, total, offset, count, lumps:[{index, name, size, offset, kind, namespace, map, source}]} where index counts positions in the listed scope and offset is the byte position of the lump's data inside its source archive as save_wad would lay it out."
    )]
    fn list_lumps(
        &self,
        Parameters(args): Parameters<ListLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let listing = match parse_listing(&args) {
            Ok(listing) => listing,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_run(session, &listing));
        reported(report)
    }

    /// Searches a scope's directory by name, type, namespace or content.
    #[tool(
        description = "Searches a scope's lumps (scope: \"merged\" default, \"working\" or \"layer:N\"); give at least one criterion. pattern is a case-insensitive name glob (* and ?), kind and namespace filter like list_lumps, contains_text is a case-insensitive ASCII substring of the payload and contains_hex is a byte substring written as hex, spaces allowed (e.g. \"4D 55 53 1A\"); all criteria combine with AND. Returns {scope, count, truncated, lumps:[row + match_offset]} where row is list_lumps' shape and match_offset is the first content-match byte offset (null when only name/kind/namespace was searched). limit defaults to 200, max 2000; truncated says more matched."
    )]
    fn search_lumps(
        &self,
        Parameters(args): Parameters<SearchLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (scope, limit, search) = match parse_search(&args) {
            Ok(parsed) => parsed,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| search_run(session, scope, limit, &search));
        reported(report)
    }

    /// Reads one lump through the shared selector.
    #[tool(
        description = "Reads one lump chosen by the shared selector: name (1 to 8 printable ASCII characters, case-insensitive) plus optional scope (\"merged\" default, \"working\", \"layer:N\"), namespace (keep only matches inside that region, e.g. \"flats\") and occurrence (0-based index among the same-name matches in the scope's directory order; the default is the LAST one, the copy that wins). format: \"info\" returns just the list_lumps row fields (name, size, kind, namespace, source, ...); \"text\" decodes UTF-8 replacing invalid sequences (\"text\", \"lossy\", \"truncated\"; max_chars default 100000); \"hex\" returns \"hex\" (space-separated uppercase bytes), \"start\" (default 0) and \"length\" (default 256, max 4096, clipped to the payload; a start past the end is invalid_argument); \"url\" publishes the raw bytes to the asset server and returns \"asset\": {url, filename, mime, size} - no base64 crosses the stdio channel; \"auto\" (the default) picks text for text-like kinds (text, dehacked, decorate, zscript, mapinfo), info for empty lumps, and url plus \"preview_hex\" (first 64 bytes) for everything else. The result always carries the row fields plus \"format\" (what actually ran). Images and sounds have dedicated decoding tools (to be added later); until then, or for any whole-binary read, use format url. Unknown formats are invalid_argument; a name with no match, or an occurrence past the last match, is not_found."
    )]
    fn get_lump(
        &self,
        Parameters(args): Parameters<GetLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let opts = match parse_get_options(&args) {
            Ok(opts) => opts,
            Err(error) => return fail(&error),
        };
        let outcome = match self
            .state()
            .with_session(|session| run_get_lump(session, &args.selector, &opts))
        {
            Ok(outcome) => outcome,
            Err(error) => return fail(&error),
        };
        match deliver(self.state(), outcome) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Writes one lump's raw bytes to a file.
    #[tool(
        description = "Writes one lump's raw bytes to an absolute filesystem path, atomically (staged beside the target, then renamed). Takes the same selector as get_lump: name plus optional scope, namespace and occurrence (default occurrence is the last, winning match). A relative path is invalid_argument; an existing target is already_exists unless overwrite is true; a missing parent directory is not_found. Returns {path, bytes}."
    )]
    fn extract_lump(
        &self,
        Parameters(args): Parameters<ExtractLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let target = match parse_target(&args.path) {
            Ok(target) => target,
            Err(error) => return fail(&error),
        };
        let bytes = match self
            .state()
            .with_session(|session| extract_run(session, &args.selector, &target, args.overwrite))
        {
            Ok(bytes) => bytes,
            Err(error) => return fail(&error),
        };
        match write_file_atomic(&target, &bytes) {
            Ok(()) => ok(&ExtractReport {
                path: path_text(&target),
                bytes: bytes.len(),
            }),
            Err(error) => fail(&error),
        }
    }

    /// Adds a new lump to the working archive.
    #[tool(
        description = "Adds a new lump to the working archive as one undoable edit (read-only layers are never touched; save_wad writes the result). name is 1 to 8 printable ASCII characters, uppercased. Payload: at most one of text (UTF-8 written as-is), hex (bytes as hex digits, spaces allowed) or source (absolute path, file:// or http(s):// URL, or an uploaded asset URL, fetched for its bytes); with none the lump is empty - that is how a directory marker is made. Placement: at most one of index (0..=current working length), before/after (relative to the LAST lump with that name in the working archive), namespace (sprites, flats, patches, colormaps, acs, voices, hires, textures: insert just before that region's end marker, creating the region with its canonical start/end markers when the working archive has none); with none the lump is appended at the end. A lump of this name already in the target namespace (global counts as its own) is already_exists unless allow_duplicate is true. Returns {name, index, size, namespace} with index counting positions in the working archive; the undo label is \"add_lump NAME\"."
    )]
    async fn add_lump(
        &self,
        Parameters(args): Parameters<AddLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let pending = match plan_add(&args) {
            Ok(pending) => pending,
            Err(error) => return fail(&error),
        };
        match commit(self.state(), pending).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Fetches a source and inserts its bytes as a new lump.
    #[tool(
        description = "Fetches source (absolute path, file:// URI, http(s):// URL or this server's asset/upload URL) and inserts its bytes into the working archive without any conversion, as one undoable edit. Identical to import_file_as_lump; clients may call either name. name defaults to the fetched file name's stem uppercased and trimmed to 8 characters (letters, digits and underscore); when nothing usable remains the call fails with invalid_argument asking for an explicit name. Placement and allow_duplicate work exactly like add_lump. Returns {name, index, size, namespace, source}; the undo label is \"import NAME\"."
    )]
    async fn import_asset(
        &self,
        Parameters(args): Parameters<ImportParams>,
    ) -> Result<CallToolResult, ErrorData> {
        imported(self.state(), &args).await
    }

    /// Fetches a file and inserts its bytes as a new lump.
    #[tool(
        description = "Fetches source (absolute path, file:// URI, http(s):// URL or this server's asset/upload URL) and inserts its bytes into the working archive without any conversion, as one undoable edit. Identical to import_asset; clients may call either name. name defaults to the fetched file name's stem uppercased and trimmed to 8 characters (letters, digits and underscore); when nothing usable remains the call fails with invalid_argument asking for an explicit name. Placement and allow_duplicate work exactly like add_lump. Returns {name, index, size, namespace, source}; the undo label is \"import NAME\"."
    )]
    async fn import_file_as_lump(
        &self,
        Parameters(args): Parameters<ImportParams>,
    ) -> Result<CallToolResult, ErrorData> {
        imported(self.state(), &args).await
    }

    /// Overwrites one lump's payload, overriding a read-only copy when needed.
    #[tool(
        description = "Overwrites one lump's payload as a single undoable edit labelled \"replace_lump NAME\" (save_wad writes the result), and is how a lump the IWAD or a resource layer owns gets changed: read-only archives are never modified, an override is created in the working archive instead and the result says \"created_override\": true. Selector: name (1 to 8 printable ASCII characters, case-insensitive) plus optional namespace (keep only matches inside that region, e.g. \"flats\") and occurrence (0-based index among the WORKING archive's same-name matches, default the last one; ignored when the name only a read-only layer holds is overridden, where the winning copy is the one taken). Payload: exactly one of text (UTF-8 written as-is), hex (bytes as hex digits, spaces allowed) or source (absolute path, file:// URI, http(s):// URL or uploaded asset URL, fetched for its bytes); none or several is invalid_argument. A lump the working archive holds is rewritten in place, keeping its name and index. A lump only a layer holds is brought in first: map data copies that whole map block (header, members and GL nodes) into the working archive and rewrites the copy, a namespace member is inserted into that region of the working archive (its canonical markers are created when it has none), a loose global is appended. A name no loaded archive holds is not_found - use add_lump to create one. Returns {name, index, size, created_override} with index counting positions in the working archive."
    )]
    async fn replace_lump(
        &self,
        Parameters(args): Parameters<ReplaceLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match edit::replace_lump(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Removes lumps from the working archive, all or nothing.
    #[tool(
        description = "Removes lumps from the working archive as one undoable edit labelled \"remove_lump A, B, ...\" (save_wad writes the result). names is a list of one or more lump names (1 to 8 printable ASCII characters, case-insensitive); an empty list, an unparsable name or an unknown namespace is invalid_argument. namespace limits the matches to that region and applies to every name. occurrence picks one copy of a single named lump (default the last working match) and is invalid_argument together with more than one name; all_occurrences (default false) removes every working copy of each name instead of the selected one. Only the working archive is touched: if any name is missing from it the call is not_found and NOTHING is removed, because IWAD and resource-layer lumps cannot be deleted, only overridden with replace_lump. Returns {removed: [{name, index}]} in directory order, with the indices the lumps held before the removal."
    )]
    fn remove_lump(
        &self,
        Parameters(args): Parameters<RemoveLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| edit::remove_lump(session, &args));
        reported(report)
    }

    /// Renames one lump of the working archive.
    #[tool(
        description = "Renames one lump of the working archive as one undoable edit labelled \"rename_lump OLD -> NEW\" (save_wad writes the result), keeping its position and payload. name selects the lump, plus optional namespace and occurrence as in replace_lump; new_name is 1 to 8 printable ASCII characters and is uppercased - either name failing to parse is invalid_argument. The working archive only: a name it does not hold is not_found (IWAD and layer lumps cannot be renamed - override one with replace_lump, or add_lump a copy and remove_lump the original). Unless allow_duplicate is true, a working lump already named new_name in the same region (no region counts as its own) is already_exists and nothing changes. Returns {old_name, new_name, index} with index counting positions in the working archive."
    )]
    fn rename_lump(
        &self,
        Parameters(args): Parameters<RenameLumpParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| edit::rename_lump(session, &args));
        reported(report)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::error::CyberError;
    use crate::session::{Scope, Session, Source};
    use crate::tools::lumps::place::{Placement, PlacementParams};
    use crate::tools::lumps::rows::LumpRow;
    use crate::tools::lumps::{
        AddLumpParams, Applied, GetLumpParams, ImportParams, Match, Needle, Payload, Prepared,
        Requested, Search, apply, asset_naming, chosen_name, clash_message, decide, find_needle,
        hex_body, hex_bytes, kind_names, namespace_names, parse_format, parse_get_options,
        parse_hex, parse_hex_length, parse_kind, parse_limit, parse_map, parse_namespace,
        parse_pair, parse_scope, parse_target, path_text, pick_payload, plan_add, plan_import,
        refuse_existing, text_body,
    };
    use crate::wad::archive::Lump;
    use crate::wad::classify::{LumpKind, Namespace};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A row carrying `data` for judging, everything else filled in.
    fn row(text: &str, kind: LumpKind, namespace: Option<Namespace>, data: &[u8]) -> LumpRow {
        LumpRow {
            index: 0,
            name: name(text),
            size: data.len(),
            offset: 12,
            kind,
            namespace,
            map: None,
            source: Source::Working,
            lump: Lump::new(name(text), data.to_vec()),
        }
    }

    /// A `Search` with only a name glob.
    fn glob_search(pattern: &str) -> Search {
        Search {
            pattern: Some(pattern.to_owned()),
            kind: None,
            namespace: None,
            text: None,
            hex: None,
        }
    }

    /// The error code of a failing [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    #[test]
    fn scope_defaults_to_merged_and_parses_every_form() {
        assert_eq!(parse_scope(None).expect("default"), Scope::Merged);
        assert_eq!(
            parse_scope(Some("working")).expect("working"),
            Scope::Working
        );
        assert_eq!(
            parse_scope(Some("layer:1")).expect("layer"),
            Scope::Layer(1)
        );
        let error = parse_scope(Some("bogus")).expect_err("not a scope");
        assert_eq!(code(&error), "invalid_argument", "bad scope code");
    }

    #[test]
    fn namespace_and_kind_filters_parse_names_or_refuse() {
        assert_eq!(parse_namespace(None).expect("absent"), None);
        assert_eq!(
            parse_namespace(Some("Flats")).expect("flats"),
            Some(Namespace::Flats)
        );
        let error = parse_namespace(Some("nope")).expect_err("not a namespace");
        assert_eq!(code(&error), "invalid_argument", "bad namespace code");
        assert!(
            error.to_string().contains("sprites"),
            "the hint lists the names: {error}"
        );
        assert_eq!(
            parse_kind(Some("DOOM_SOUND")).expect("kind"),
            Some(LumpKind::DoomSound)
        );
        let kind = parse_kind(Some("nope")).expect_err("not a kind");
        assert_eq!(code(&kind), "invalid_argument", "bad kind code");
        assert!(
            kind.to_string().contains("map_marker"),
            "the hint lists the kinds: {kind}"
        );
    }

    #[test]
    fn map_filter_parses_lump_names_strictly() {
        assert_eq!(parse_map(None).expect("absent"), None);
        assert_eq!(
            parse_map(Some("map01")).expect("parsed"),
            Some(name("MAP01")),
            "the name parser uppercases"
        );
        let error = parse_map(Some("TOO LONG FOR A NAME")).expect_err("over eight bytes");
        assert_eq!(code(&error), "invalid_argument", "bad map name code");
    }

    #[test]
    fn limit_defaults_and_stays_in_range() {
        assert_eq!(
            parse_limit(None).expect("default"),
            200,
            "documented default"
        );
        assert_eq!(parse_limit(Some(2000)).expect("max"), 2000, "the cap");
        assert_eq!(parse_limit(Some(1)).expect("min"), 1, "the floor");
        for bad in [0_usize, 2001] {
            let error = parse_limit(Some(bad)).expect_err("out of range");
            assert_eq!(code(&error), "invalid_argument", "limit {bad} code");
        }
    }

    #[test]
    fn hex_needles_accept_digits_with_spaces_and_refuse_the_rest() {
        assert_eq!(parse_hex(None).expect("absent"), None);
        assert_eq!(
            parse_hex(Some("4D 55 53 1A")).expect("spaced"),
            Some(vec![0x4D_u8, 0x55, 0x53, 0x1A])
        );
        assert_eq!(
            parse_hex(Some("4d55")).expect("lowercase"),
            Some(vec![0x4D_u8, 0x55])
        );
        let odd = parse_hex(Some("4D5")).expect_err("odd digits");
        assert_eq!(code(&odd), "invalid_argument", "odd hex code");
        assert!(
            odd.to_string().contains("odd"),
            "the message says odd: {odd}"
        );
        let bad = parse_hex(Some("0G")).expect_err("not hex");
        assert_eq!(code(&bad), "invalid_argument", "non-hex code");
        assert!(
            bad.to_string().contains("'G'"),
            "the message names the digit: {bad}"
        );
    }

    #[test]
    fn pair_parsing_is_strict() {
        assert_eq!(parse_pair(&['4', 'd']), Some(0x4D), "mixed case digits");
        assert_eq!(parse_pair(&['F', 'F']), Some(0xFF), "top of the range");
        assert_eq!(parse_pair(&['Z', 'Q']), None, "non-hex characters");
        assert_eq!(parse_pair(&[]), None, "an empty pair is not a byte");
        assert_eq!(parse_pair(&['7']), None, "a lone digit is not a byte");
    }

    #[test]
    fn needles_match_with_the_right_case_rule() {
        let data = b"Thing 1 (Troop)".to_vec();
        let text = Needle {
            bytes: b"TROOP",
            ignore_case: true,
        };
        let hex = Needle {
            bytes: b"(T",
            ignore_case: false,
        };
        let lone = Needle {
            bytes: b"",
            ignore_case: false,
        };
        assert_eq!(find_needle(&data, &text), Some(9), "case folds both ways");
        assert_eq!(find_needle(&data, &hex), Some(8), "exact bytes");
        assert_eq!(
            find_needle(&data, &lone),
            Some(0),
            "an empty needle matches at 0"
        );
        let missing = Needle {
            bytes: b"(t",
            ignore_case: false,
        };
        assert_eq!(find_needle(&data, &missing), None, "hex stays case-exact");
        let absent = Needle {
            bytes: b"NOPE",
            ignore_case: true,
        };
        assert_eq!(find_needle(&data, &absent), None, "absent text");
    }

    #[test]
    fn judge_combines_every_criterion_with_and() {
        let empty = Search {
            pattern: None,
            kind: None,
            namespace: None,
            text: None,
            hex: None,
        };
        assert!(empty.is_empty(), "no criteria at all");
        let floor = row("FLOOR1", LumpKind::Flat, Some(Namespace::Flats), b"aa");
        let dehacked = row("DEHACKED", LumpKind::Dehacked, None, b"Thing 1 (Troop)");
        assert!(!glob_search("F*").is_empty(), "a pattern is a criterion");
        assert!(
            matches!(glob_search("F*").judge(&floor), Match::Hit(None)),
            "a name-only hit carries no offset"
        );
        assert!(
            matches!(glob_search("?LOOR1").judge(&floor), Match::Hit(None)),
            "question marks"
        );
        assert!(
            matches!(glob_search("F*").judge(&dehacked), Match::Miss),
            "a name miss drops the row"
        );
        let conflicting = Search {
            pattern: None,
            kind: Some(LumpKind::Dehacked),
            namespace: Some(Namespace::Flats),
            text: None,
            hex: None,
        };
        assert!(
            matches!(conflicting.judge(&dehacked), Match::Miss),
            "kind AND namespace must both hold"
        );
        let content = Search {
            pattern: None,
            kind: None,
            namespace: None,
            text: Some(b"TROOP".to_vec()),
            hex: Some(b"(T".to_vec()),
        };
        assert!(
            matches!(content.judge(&dehacked), Match::Hit(Some(8))),
            "the lowest content offset wins"
        );
        let missing_hex = Search {
            pattern: None,
            kind: None,
            namespace: None,
            text: Some(b"TROOP".to_vec()),
            hex: Some(b"ZZ".to_vec()),
        };
        assert!(
            matches!(missing_hex.judge(&dehacked), Match::Miss),
            "one absent needle drops the row"
        );
    }

    #[test]
    fn error_hints_list_every_documented_name() {
        assert_eq!(
            namespace_names(),
            "sprites, flats, patches, colormaps, acs, voices, hires, textures",
            "the namespace hint text"
        );
        assert!(
            kind_names().starts_with("marker, map_marker, map_data, palette"),
            "the kind hint leads with the directory kinds"
        );
        assert!(
            kind_names().contains("pc_speaker"),
            "the kind hint is complete"
        );
    }

    #[test]
    fn get_format_defaults_to_auto_and_accepts_every_name() {
        assert_eq!(parse_format(None).expect("default"), Requested::Auto);
        for (text, wanted) in [
            ("auto", Requested::Auto),
            ("INFO", Requested::Info),
            ("Text", Requested::Text),
            ("hex", Requested::Hex),
            ("url", Requested::Url),
        ] {
            assert_eq!(
                parse_format(Some(text)).expect(text),
                wanted,
                "format {text}"
            );
        }
        let error = parse_format(Some("base64")).expect_err("not a format");
        assert_eq!(code(&error), "invalid_argument", "bad format code");
        assert!(
            error.to_string().contains("\"auto\""),
            "the hint names the default: {error}"
        );
    }

    #[test]
    fn hex_length_defaults_and_stays_under_the_cap() {
        assert_eq!(
            parse_hex_length(None).expect("default"),
            256,
            "the documented default"
        );
        assert_eq!(
            parse_hex_length(Some(4_096)).expect("cap"),
            4_096,
            "the cap"
        );
        assert_eq!(
            parse_hex_length(Some(0)).expect("empty"),
            0,
            "zero is empty"
        );
        let error = parse_hex_length(Some(4_097)).expect_err("over the cap");
        assert_eq!(code(&error), "invalid_argument", "over-cap code");
        assert!(
            error.to_string().contains("4096"),
            "the message names the cap: {error}"
        );
    }

    #[test]
    fn auto_picks_text_for_text_like_kinds_and_info_for_empty() {
        for kind in [
            LumpKind::Text,
            LumpKind::Dehacked,
            LumpKind::Decorate,
            LumpKind::Zscript,
            LumpKind::Mapinfo,
        ] {
            let row = row("ANY", kind, None, b"words");
            assert_eq!(
                decide(Requested::Auto, &row),
                Applied::Text,
                "auto over {kind:?}"
            );
        }
        let empty = row("F_START", LumpKind::Marker, None, b"");
        assert_eq!(
            decide(Requested::Auto, &empty),
            Applied::Info,
            "auto over empty"
        );
        let flat = row("FLOOR1", LumpKind::Flat, Some(Namespace::Flats), b"\0\x01");
        assert_eq!(
            decide(Requested::Auto, &flat),
            Applied::Url,
            "auto over a flat"
        );
        for (requested, applied) in [
            (Requested::Info, Applied::Info),
            (Requested::Text, Applied::Text),
            (Requested::Hex, Applied::Hex),
            (Requested::Url, Applied::Url),
        ] {
            assert_eq!(decide(requested, &flat), applied, "explicit {requested:?}");
        }
    }

    #[test]
    fn text_bodies_report_lossiness_and_truncation() {
        let clean = text_body(b"Thing 1 (Troop)", 100_000);
        assert_eq!(
            clean.text.as_deref(),
            Some("Thing 1 (Troop)"),
            "clean bytes survive"
        );
        assert_eq!(clean.lossy, Some(false), "clean bytes are not lossy");
        assert_eq!(clean.truncated, Some(false), "well under the cap");
        assert_eq!(clean.hex, None, "a text body carries no hex fields");
        let ragged = text_body(b"ab\xe9\xffcd", 100_000);
        assert_eq!(ragged.lossy, Some(true), "invalid sequences were replaced");
        assert_eq!(ragged.truncated, Some(false), "short input");
        assert_eq!(
            ragged.text.as_deref(),
            Some("ab\u{FFFD}\u{FFFD}cd"),
            "the replacement characters"
        );
        let cut = text_body(b"Thing 1 (Troop)", 5);
        assert_eq!(
            cut.text.as_deref(),
            Some("Thing"),
            "cut at the character cap"
        );
        assert_eq!(cut.truncated, Some(true), "the cap was hit");
    }

    #[test]
    fn hex_windows_format_clip_and_refuse_a_start_past_the_end() {
        let head = hex_body(b"Mus\x1Arest", name("D_E1M1"), 0, 4).expect("a fitting window");
        assert_eq!(
            head.hex.as_deref(),
            Some("4D 75 73 1A"),
            "space-separated uppercase bytes"
        );
        assert_eq!(
            (head.start, head.length),
            (Some(0), Some(4)),
            "echoed window"
        );
        let clipped = hex_body(b"abc", name("X"), 1, 4_096).expect("clipped window");
        assert_eq!(
            clipped.hex.as_deref(),
            Some("62 63"),
            "the window clips at the payload end"
        );
        assert_eq!(
            clipped.length,
            Some(2),
            "the reported length is the real one"
        );
        let tail = hex_body(b"abc", name("X"), 3, 16).expect("an empty tail window");
        assert_eq!(tail.hex.as_deref(), Some(""), "a start at the end is empty");
        assert_eq!(tail.length, Some(0), "no bytes");
        let error = hex_body(b"abc", name("X"), 4, 16).expect_err("past the end");
        assert_eq!(code(&error), "invalid_argument", "start code");
        assert!(
            error
                .to_string()
                .contains("4 is past the end of lump X (3 bytes)"),
            "the message shows the bounds: {error}"
        );
    }

    #[test]
    fn asset_names_follow_the_kind_table() {
        let cases = [
            (LumpKind::Png, "png"),
            (LumpKind::Jpeg, "jpg"),
            (LumpKind::Gif, "gif"),
            (LumpKind::Wav, "wav"),
            (LumpKind::Ogg, "ogg"),
            (LumpKind::Mp3, "mp3"),
            (LumpKind::Flac, "flac"),
            (LumpKind::Midi, "mid"),
            (LumpKind::Mus, "mus"),
            (LumpKind::Text, "txt"),
            (LumpKind::Dehacked, "txt"),
            (LumpKind::Mapinfo, "txt"),
            (LumpKind::Palette, "lmp"),
            (LumpKind::Flat, "lmp"),
            (LumpKind::Unknown, "lmp"),
        ];
        for (kind, extension) in cases {
            let row = row("LOMB", kind, None, b"x");
            let (filename, mime) = asset_naming(&row);
            assert_eq!(
                filename,
                format!("LOMB.{extension}"),
                "file name over {kind:?}"
            );
            assert_eq!(
                mime,
                crate::assets::store::mime_for_extension(extension),
                "mime"
            );
        }
    }

    #[test]
    fn extract_targets_must_be_absolute() {
        let path = parse_target("/tmp/opencode/nowhere.lmp").expect("an absolute path");
        assert!(path.is_absolute(), "kept as given");
        let error = parse_target("out.bin").expect_err("a relative path");
        assert_eq!(code(&error), "invalid_argument", "relative code");
        assert!(
            error.to_string().contains("not absolute"),
            "the message says what to pass: {error}"
        );
    }

    #[test]
    fn extract_refuses_an_existing_target_unless_overwriting() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let target = dir.path().join("lump.bin");
        refuse_existing(&target, false).expect("nothing stands there");
        std::fs::write(&target, b"old").expect("the scratch file lands");
        let error = refuse_existing(&target, false).expect_err("the file exists");
        assert_eq!(code(&error), "already_exists", "existing target code");
        assert!(
            error.to_string().contains("overwrite true"),
            "the message names the fix: {error}"
        );
        refuse_existing(&target, true).expect("overwrite was allowed");
    }

    #[test]
    fn path_text_renders_a_path_for_reports() {
        assert_eq!(
            path_text(Path::new("/tmp/opencode/x.lmp")),
            "/tmp/opencode/x.lmp"
        );
    }

    #[test]
    fn get_options_default_every_window() {
        let args = GetLumpParams {
            selector: crate::tools::lumps::select::LumpSelector {
                name: "X".to_owned(),
                scope: None,
                namespace: None,
                occurrence: None,
            },
            format: None,
            start: None,
            length: None,
            max_chars: None,
        };
        let opts = parse_get_options(&args).expect("defaults parse");
        assert_eq!(opts.requested, Requested::Auto, "the default format");
        assert_eq!(
            (opts.start, opts.length),
            (0, 256),
            "the default hex window"
        );
        assert_eq!(opts.max_chars, 100_000, "the default text cap");
    }

    /// `add_lump` arguments carrying only the required name.
    fn add_args(text: &str) -> AddLumpParams {
        AddLumpParams {
            name: text.to_owned(),
            text: None,
            hex: None,
            source: None,
            placement: PlacementParams::default(),
            allow_duplicate: false,
        }
    }

    /// `import` arguments for `source` with an optional explicit name.
    fn import_args(source: &str, wanted: Option<&str>) -> ImportParams {
        ImportParams {
            source: source.to_owned(),
            name: wanted.map(str::to_owned),
            placement: PlacementParams::default(),
            allow_duplicate: false,
        }
    }

    /// A committed edit of `data` bytes under `text`, appending.
    fn prepared(text: &str, data: &[u8]) -> Prepared {
        Prepared {
            verb: "add_lump",
            name: name(text),
            data: data.to_vec(),
            placement: Placement::default(),
            allow_duplicate: false,
            source: None,
        }
    }

    #[test]
    fn payload_picking_takes_at_most_one_field() {
        assert!(
            matches!(
                pick_payload(None, None, None).expect("none"),
                Payload::Empty
            ),
            "no data field is a marker"
        );
        assert!(
            matches!(pick_payload(Some("t"), None, None), Ok(Payload::Text(_))),
            "text wins alone"
        );
        assert!(
            matches!(pick_payload(None, Some("4D55"), None), Ok(Payload::Hex(_))),
            "hex wins alone"
        );
        assert!(
            matches!(
                pick_payload(None, None, Some("/x/y.bin")),
                Ok(Payload::Source(_))
            ),
            "source wins alone"
        );
        for (text, digits, source) in [
            (Some("t"), Some("4d"), None),
            (Some("t"), None, Some("/x")),
            (None, Some("4d"), Some("/x")),
        ] {
            let error = pick_payload(text, digits, source).expect_err("two fields");
            assert_eq!(code(&error), "invalid_argument", "payload conflict code");
            assert!(
                error.to_string().contains("at most one"),
                "the message states the rule: {error}"
            );
        }
    }

    #[test]
    fn plan_add_validates_name_and_placement_up_front() {
        let plan = plan_add(&add_args("note")).expect("valid");
        assert_eq!(plan.name, Some(name("NOTE")), "the name uppercases");
        assert_eq!(plan.verb, "add_lump", "the undo verb");
        assert!(plan.source.is_none(), "add_lump echoes no source");
        let long = plan_add(&add_args("TOO LONG FOR WAD")).expect_err("over eight bytes");
        assert_eq!(code(&long), "invalid_argument", "bad name code");
        let conflicted = AddLumpParams {
            placement: PlacementParams {
                index: Some(1),
                after: Some("X".to_owned()),
                ..PlacementParams::default()
            },
            ..add_args("NOTE")
        };
        let error = plan_add(&conflicted).expect_err("two placement fields");
        assert_eq!(code(&error), "invalid_argument", "placement conflict code");
    }

    #[test]
    fn plan_import_defers_name_and_fetch_to_prepare() {
        let plan = plan_import(&import_args("/x/y.png", None)).expect("valid");
        assert_eq!(plan.name, None, "the name is derived after the fetch");
        assert_eq!(plan.verb, "import", "the undo verb");
        assert_eq!(
            plan.source.as_deref(),
            Some("/x/y.png"),
            "the source echoes"
        );
        let explicit = plan_import(&import_args("/x/y.png", Some("PIC"))).expect("named");
        assert_eq!(explicit.name, Some(name("PIC")), "an explicit name parses");
        let bad = plan_import(&import_args("/x/y.png", Some("too long name here")))
            .expect_err("over eight bytes");
        assert_eq!(code(&bad), "invalid_argument", "bad name code");
    }

    #[test]
    fn an_import_name_falls_back_to_the_derived_file_stem() {
        assert_eq!(
            chosen_name(Some(name("GIVEN")), Some("x.png")).expect("explicit wins"),
            name("GIVEN"),
            "the caller's name is kept"
        );
        assert_eq!(
            chosen_name(None, Some("D_RUNNIN.mid")).expect("derived"),
            name("D_RUNNIN"),
            "the stem uppercases"
        );
        let error = chosen_name(None, Some(".png")).expect_err("nothing to derive");
        assert_eq!(code(&error), "invalid_argument", "underivable code");
        assert!(
            error.to_string().contains("pass name"),
            "the message asks for name: {error}"
        );
        let nofile = chosen_name(None, None).expect_err("no filename at all");
        assert_eq!(code(&nofile), "invalid_argument", "absent filename code");
    }

    #[test]
    fn hex_payloads_report_under_their_own_field_name() {
        assert_eq!(
            hex_bytes("hex", "4d 5553").expect("decodes"),
            vec![0x4D_u8, 0x55, 0x53],
            "spaces are ignored"
        );
        assert!(
            hex_bytes("hex", "").expect("empty hex").is_empty(),
            "empty hex is an empty payload"
        );
        let odd = hex_bytes("hex", "4D5").expect_err("odd digits");
        assert_eq!(code(&odd), "invalid_argument", "odd hex code");
        assert!(
            odd.to_string().starts_with("hex "),
            "the field is named: {odd}"
        );
        let bad = hex_bytes("hex", "0G").expect_err("not hex");
        assert!(
            bad.to_string().starts_with("hex "),
            "the field is named: {bad}"
        );
    }

    #[test]
    fn clash_messages_name_the_region_and_both_exits() {
        let global = clash_message(name("TWIN"), None);
        assert!(
            global.contains("global area")
                && global.contains("allow_duplicate")
                && global.contains("replace_lump"),
            "the global text names both exits: {global}"
        );
        let flats = clash_message(name("TWIN"), Some(Namespace::Flats));
        assert!(
            flats.contains("the flats region"),
            "the region is named: {flats}"
        );
    }

    #[test]
    fn apply_inserts_once_labels_the_history_and_refuses_twins() {
        let mut session = Session::default();
        let edit = prepared("NOTE", b"hello");
        let report = apply(&mut session, &edit).expect("the first add lands");
        assert_eq!((report.index, report.size), (0, 5), "appended at 0");
        assert_eq!(report.namespace, None, "a loose lump sits in no region");
        assert_eq!(
            session.undo_labels(),
            vec!["add_lump NOTE"],
            "the undo label"
        );
        assert!(session.is_dirty(), "an unsaved edit");
        let error = apply(&mut session, &edit).expect_err("the twin is refused");
        assert_eq!(code(&error), "already_exists", "duplicate code");
        assert_eq!(session.working().len(), 1, "the refused edit rolled back");
        assert_eq!(session.undo_labels().len(), 1, "and left no history");
        let allowed = Prepared {
            allow_duplicate: true,
            ..prepared("NOTE", b"again")
        };
        let twice = apply(&mut session, &allowed).expect("the twin is allowed");
        assert_eq!(twice.index, 1, "appended beside it");
        assert_eq!(session.working().len(), 2, "both copies stand");
    }

    #[test]
    fn apply_reports_the_region_a_namespace_insert_joins() {
        let mut session = Session::default();
        let edit = Prepared {
            placement: Placement {
                namespace: Some(Namespace::Flats),
                ..Placement::default()
            },
            ..prepared("FLATA", b"\0".as_slice())
        };
        let report = apply(&mut session, &edit).expect("the region is created");
        assert_eq!(
            (report.index, report.namespace),
            (1, Some(Namespace::Flats)),
            "the lump sits between the new markers"
        );
        assert_eq!(session.working().len(), 3, "start, lump, end");
        let again = prepared("FLATA", b"\0".as_slice());
        let loose = apply(&mut session, &again).expect("a global twin is no clash");
        assert_eq!(loose.namespace, None, "the loose copy is global");
        assert_eq!(loose.index, 3, "appended past the region");
    }
}
