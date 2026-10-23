//! Graphics tools: the flats, patch and sprite listings of the merged view
//! and the image tools that publish one picture as an asset.
//!
//! Every tool reads [`crate::session::Scope::Merged`] through the shared
//! rows in `crate::tools::lumps::rows`: the lumps of one namespace, last
//! lump with a name wins. `list_flats` describes the floor and ceiling
//! planes of the `F_START` region, `list_patches` the texture components
//! of the `P_START` region (plus any lump `PNAMES` names from outside the
//! flats and sprites regions), and `list_sprites` groups the `S_START`
//! region by the sprite name grammar of
//! [`crate::gfx::sprite_name::parse_sprite_name`]. The listings decode no
//! pixels: sizes, dimensions, offsets and names only.
//!
//! `get_flat`, `get_patch`, `get_sprite`, `get_thing_sprite` and
//! `decode_patch` do read pixels, through the shared resolvers in
//! [`resolve`], and never inline them: each publishes a `PNG` or a raw
//! `RGBA` asset and returns the [`AssetRef`] with the dimensions and
//! drawing offsets it encoded. `get_thing_sprite` starts from a thing
//! type number and reads its sprite through the session-aware definitions
//! of [`crate::defs::session_definitions`].

pub mod resolve;

use alloc::collections::{BTreeMap, BTreeSet};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::fetch::derive_lump_name;
use crate::assets::server::AssetRef;
use crate::defs::{CustomSources, Definitions, ThingDef, session_definitions};
use crate::error::CyberError;
use crate::gfx::codec::{encode_png, is_png, png_dimensions};
use crate::gfx::flat::flat_shape;
use crate::gfx::grab::{GrabOffsets, read_grab};
use crate::gfx::image::{IndexedImage, RgbaImage, scale_integer};
use crate::gfx::patch::{Patch, PatchHeader, decode_patch, patch_header};
use crate::gfx::sprite_name::{SpriteName, parse_sprite_name};
use crate::server::Cyberdemon;
use crate::session::{Scope, Session, Source};
use crate::state::AppState;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::resolve::{
    load_flat, load_patch, load_sprite, load_sprite_lump, lump_bytes,
};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::palette::load_palette;
use crate::tools::{fail, ok};
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::game::Game;
use crate::wad::name::{LumpName, glob_match};
use crate::wad::textures::parse_pnames;

/// Default page size for the listing tools.
const DEFAULT_LIMIT: usize = 200;
/// Largest page any listing tool will return.
const MAX_LIMIT: usize = 2000;
/// Raw directory bytes of the patch-name lump.
const PNAMES_RAW: [u8; 8] = *b"PNAMES\0\0";
/// Characters of the four-character sprite prefix `list_sprites` filters by.
const SPRITE_PREFIX_LEN: usize = 4;
/// Default `PLAYPAL` entry the image tools colour a picture with.
const DEFAULT_PALETTE: usize = 0_usize;
/// Default published-image scale: the stored size.
const DEFAULT_SCALE: u32 = 1_u32;
/// Largest whole-pixel upscale the image tools accept.
const MAX_SCALE: u32 = 8_u32;
/// Frame letter `get_sprite` assumes without one.
const DEFAULT_FRAME: char = 'A';
/// Rotation `get_sprite` assumes without one: the first of the eight views.
const DEFAULT_ROTATION: u8 = 1_u8;
/// Largest rotation number a sprite frame serves.
const MAX_ROTATION: u8 = 8_u8;
/// Name `decode_patch` publishes under when a `source` names no file.
const FALLBACK_NAME: &str = "PATCH";
/// Media type of the raw `RGBA8` assets.
const RGBA_MIME: &str = "application/octet-stream";

/// Arguments for [`Cyberdemon::list_flats`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ListFlatsParams {
    /// Case-insensitive flat-name glob: `*` matches any run of
    /// characters, `?` exactly one, e.g. `"LAVA*"`.
    #[serde(default)]
    pattern: Option<String>,
    /// Matching flats to skip before the page (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    limit: Option<usize>,
}

/// Arguments for [`Cyberdemon::list_patches`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ListPatchesParams {
    /// Case-insensitive patch-name glob: `*` matches any run of
    /// characters, `?` exactly one, e.g. `"WALL*"`.
    #[serde(default)]
    pattern: Option<String>,
    /// Matching patches to skip before the page (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    limit: Option<usize>,
    /// Only report patches `PNAMES` names (default `false`).
    #[serde(default)]
    in_pnames_only: Option<bool>,
}

/// Arguments for [`Cyberdemon::list_sprites`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ListSpritesParams {
    /// Four-character sprite prefix to list, e.g. `"POSS"` (case-
    /// insensitive); without it every sprite of the session is grouped.
    #[serde(default)]
    prefix: Option<String>,
    /// Matching sprites to skip before the page (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum sprite groups in the result (default 200, at most 2000).
    #[serde(default)]
    limit: Option<usize>,
}

/// The validated page request of the listing tools.
#[derive(Debug)]
struct Page {
    /// Rows to skip before the page.
    offset: usize,
    /// Maximum rows on the page.
    limit: usize,
}

/// The `{offset, limit}` pair a listing tool was given.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a `limit` outside `1..=2000`.
fn parse_page(offset: Option<usize>, limit: Option<usize>) -> Result<Page, CyberError> {
    let value = match limit {
        None => DEFAULT_LIMIT,
        Some(value) if value == 0 || value > MAX_LIMIT => {
            return Err(CyberError::InvalidArgument(format!(
                "limit {value} is out of range: pass a value between 1 and {MAX_LIMIT}"
            )));
        }
        Some(value) => value,
    };
    Ok(Page {
        offset: offset.unwrap_or(0_usize),
        limit: value,
    })
}

/// Whether `name` passes the optional glob filter.
fn passes(name: LumpName, pattern: Option<&str>) -> bool {
    pattern.is_none_or(|glob| glob_match(name.as_bytes(), glob))
}

/// Keeps the matching rows of `matched` on `page` and counts all matches.
fn page_rows<T>(matched: Vec<T>, page: &Page) -> (usize, Vec<T>) {
    let total = matched.len();
    (
        total,
        matched
            .into_iter()
            .skip(page.offset)
            .take(page.limit)
            .collect(),
    )
}

/// Keeps the last occurrence of every lump name: a name lookup over the
/// merged view resolves to the last lump, so a listing shows one row per
/// name even when one archive repeats it.
fn last_wins(rows: Vec<&LumpRow>) -> Vec<&LumpRow> {
    let mut seen: BTreeSet<LumpName> = BTreeSet::new();
    let mut kept: Vec<&LumpRow> = rows
        .into_iter()
        .rev()
        .filter(|row| seen.insert(row.name))
        .collect();
    kept.reverse();
    kept
}

/// The merged directory's rows, for a namespace listing.
///
/// # Errors
///
/// [`build_rows`]' errors for the merged scope.
fn merged_rows(session: &Session) -> Result<Vec<LumpRow>, CyberError> {
    build_rows(session, Scope::Merged)
}

/// Whether `row` is a data lump of `namespace` (markers are not listed).
fn in_namespace(row: &LumpRow, namespace: Namespace) -> bool {
    row.kind != LumpKind::Marker && row.namespace == Some(namespace)
}

/// One `list_flats` row.
#[derive(Debug, Serialize)]
struct FlatRow {
    /// The flat's lump name.
    name: String,
    /// Payload size in bytes.
    size: usize,
    /// Width in pixels, `null` for a raw flat of an unsupported size or
    /// an unreadable `PNG` header.
    width: Option<u32>,
    /// Height in pixels, `null` like `width`.
    height: Option<u32>,
    /// `"png"` when the bytes start with the `PNG` signature, `"raw"`
    /// otherwise.
    format: &'static str,
    /// The archive the winning lump came from.
    source: Source,
}

/// Structured result of [`Cyberdemon::list_flats`].
#[derive(Debug, Serialize)]
struct FlatsReport {
    /// Flats matching `pattern` (before paging).
    total: usize,
    /// The matching flats, in directory order, on this page.
    flats: Vec<FlatRow>,
}

/// The dimensions of a flat lump: the `IHDR` shape of a `PNG`, or the
/// shape the raw byte count implies (`4096`-`64x64`, `8192`-`64x128`,
/// `16384`-`128x128`, `65536`-`256x256`), `null`s for an unknown size.
fn flat_row(row: &LumpRow) -> FlatRow {
    let data = &row.lump.data;
    let (format, shape) = if is_png(data) {
        ("png", png_dimensions(data))
    } else {
        ("raw", flat_shape(data.len()))
    };
    let (width, height) = shape_pair(shape);
    FlatRow {
        name: row.name.to_string(),
        size: row.size,
        width,
        height,
        format,
        source: row.source,
    }
}

/// `list_flats`' work over the session.
///
/// # Errors
///
/// [`merged_rows`]' errors for the merged scope.
fn list_flats_run(
    session: &Session,
    page: &Page,
    pattern: Option<&str>,
) -> Result<FlatsReport, CyberError> {
    let rows = merged_rows(session)?;
    let matched: Vec<FlatRow> = last_wins(
        rows.iter()
            .filter(|row| in_namespace(row, Namespace::Flats))
            .collect(),
    )
    .into_iter()
    .filter(|row| passes(row.name, pattern))
    .map(flat_row)
    .collect();
    let (total, flats) = page_rows(matched, page);
    Ok(FlatsReport { total, flats })
}

/// One `list_patches` row.
#[derive(Debug, Serialize)]
struct PatchRow {
    /// The patch's lump name.
    name: String,
    /// Payload size in bytes.
    size: usize,
    /// Width in pixels, `null` for an unknown format or an unreadable
    /// `PNG` header.
    width: Option<u32>,
    /// Height in pixels, `null` like `width`.
    height: Option<u32>,
    /// Horizontal drawing offset, `null` when the format stores none.
    left_offset: Option<i32>,
    /// Vertical drawing offset, `null` like `left_offset`.
    top_offset: Option<i32>,
    /// `"doom"` for a readable `Doom` patch header, `"png"` for `PNG`
    /// bytes, `"unknown"` for anything else.
    format: &'static str,
    /// Whether the merged `PNAMES` lists this name.
    in_pnames: bool,
    /// `"patches"` inside a `P_START` region, `null` elsewhere.
    namespace: Option<&'static str>,
    /// The archive the winning lump came from.
    source: Source,
}

/// Structured result of [`Cyberdemon::list_patches`].
#[derive(Debug, Serialize)]
struct PatchesReport {
    /// Patches matching the filters (before paging).
    total: usize,
    /// The matching patches, in directory order, on this page.
    patches: Vec<PatchRow>,
}

/// The merged view's `PNAMES` names as lump names; no `PNAMES` is the
/// empty set, and an entry that is not a valid lump name never matches.
///
/// # Errors
///
/// [`parse_pnames`]' [`CyberError::Format`] for a malformed lump.
fn pnames_of(session: &Session) -> Result<BTreeSet<LumpName>, CyberError> {
    let Some(entry) = session.find(LumpName::from_raw(PNAMES_RAW), None) else {
        return Ok(BTreeSet::new());
    };
    Ok(parse_pnames(&entry.lump.data)?
        .into_iter()
        .filter_map(|text| LumpName::parse(&text).ok())
        .collect())
}

/// Whether `row` is listed as a patch: inside a `P_START` region, or
/// named by `PNAMES` from outside the flats and sprites regions.
fn is_patch_candidate(row: &LumpRow, pnames: &BTreeSet<LumpName>) -> bool {
    if row.kind == LumpKind::Marker {
        return false;
    }
    match row.namespace {
        Some(Namespace::Patches) => true,
        Some(Namespace::Flats | Namespace::Sprites) => false,
        Some(_) | None => pnames.contains(&row.name),
    }
}

/// A patch measurement: the `format` label with dimensions and drawing
/// offsets, `None` wherever the format stores nothing readable.
type PatchShape = (
    &'static str,
    Option<u32>,
    Option<u32>,
    Option<i32>,
    Option<i32>,
);

/// The report for a lump neither probe recognizes.
const SHAPE_UNKNOWN: PatchShape = ("unknown", None, None, None, None);

/// Splits a measured `(width, height)` into the optional row fields.
fn shape_pair(shape: Option<(u32, u32)>) -> (Option<u32>, Option<u32>) {
    shape.map_or((None, None), |(width, height)| (Some(width), Some(height)))
}

/// Splits a paired offset into the optional row fields.
const fn offset_pair(offsets: (i32, i32)) -> (Option<i32>, Option<i32>) {
    let (left, top) = offsets;
    (Some(left), Some(top))
}

/// The `PNG` branch of [`patch_row`]: `IHDR` dimensions and the offsets
/// of a `grAb` chunk when the lump carries one.
fn png_patch_shape(data: &[u8]) -> PatchShape {
    let (width, height) = shape_pair(png_dimensions(data));
    let (left_offset, top_offset) = read_grab(data)
        .ok()
        .flatten()
        .map_or((None, None), offset_pair);
    ("png", width, height, left_offset, top_offset)
}

/// The `Doom` branch of [`patch_row`]: the header's dimensions and
/// drawing offsets; the column bodies are never touched.
fn doom_patch_shape(header: PatchHeader) -> PatchShape {
    (
        "doom",
        Some(u32::from(header.width)),
        Some(u32::from(header.height)),
        Some(i32::from(header.left_offset)),
        Some(i32::from(header.top_offset)),
    )
}

/// One patch row: format sniffing first `PNG`, then the `Doom` header.
fn patch_row(row: &LumpRow, pnames: &BTreeSet<LumpName>) -> PatchRow {
    let data = &row.lump.data;
    let (format, width, height, left_offset, top_offset) = if is_png(data) {
        png_patch_shape(data)
    } else if let Some(header) = patch_header(data) {
        doom_patch_shape(header)
    } else {
        SHAPE_UNKNOWN
    };
    PatchRow {
        name: row.name.to_string(),
        size: row.size,
        width,
        height,
        left_offset,
        top_offset,
        format,
        in_pnames: pnames.contains(&row.name),
        namespace: in_namespace(row, Namespace::Patches).then_some("patches"),
        source: row.source,
    }
}

/// `list_patches`' work over the session.
///
/// # Errors
///
/// [`merged_rows`]' errors and [`pnames_of`]' [`CyberError::Format`] for
/// a malformed `PNAMES`.
fn list_patches_run(
    session: &Session,
    page: &Page,
    pattern: Option<&str>,
    in_pnames_only: bool,
) -> Result<PatchesReport, CyberError> {
    let pnames = pnames_of(session)?;
    let rows = merged_rows(session)?;
    let matched: Vec<PatchRow> = last_wins(
        rows.iter()
            .filter(|row| is_patch_candidate(row, &pnames))
            .collect(),
    )
    .into_iter()
    .filter(|row| passes(row.name, pattern))
    .filter(|row| !in_pnames_only || pnames.contains(&row.name))
    .map(|row| patch_row(row, &pnames))
    .collect();
    let (total, patches) = page_rows(matched, page);
    Ok(PatchesReport { total, patches })
}

/// One `list_sprites` rotation entry: which lump serves this frame and
/// rotation, and whether the image is flipped to do it.
#[derive(Debug, Serialize)]
struct RotationRow {
    /// Rotation `0`-`8` this entry serves.
    rotation: u8,
    /// The lump providing the image.
    lump: String,
    /// Whether the engine must flip the image horizontally for this
    /// rotation.
    mirrored: bool,
}

/// One frame of a sprite: a frame letter and the rotations it serves.
#[derive(Debug, Serialize)]
struct FrameRow {
    /// The frame letter.
    frame: char,
    /// The rotations this frame serves, by number then lump name.
    rotations: Vec<RotationRow>,
}

/// One sprite group: a four-character prefix and its frames.
#[derive(Debug, Serialize)]
struct SpriteRow {
    /// The four-character sprite prefix.
    sprite: String,
    /// The frames, in letter order.
    frames: Vec<FrameRow>,
}

/// Structured result of [`Cyberdemon::list_sprites`].
#[derive(Debug, Serialize)]
struct SpritesReport {
    /// Sprite groups matching `prefix` (before paging).
    total: usize,
    /// The matching sprite groups, by name, on this page.
    sprites: Vec<SpriteRow>,
    /// Sprites-region lumps whose names match no sprite grammar, in
    /// directory order.
    invalid: Vec<String>,
}

/// Sprite name to frames to rotations, built over the directory walk.
type Groups = BTreeMap<String, BTreeMap<char, Vec<RotationRow>>>;

/// The four-character sprite prefix to filter by, uppercased; anything
/// that is not four `ASCII` letters or digits is refused.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected text.
fn parse_prefix(text: Option<&str>) -> Result<Option<String>, CyberError> {
    text.map(parse_sprite).transpose()
}

/// One validated sprite prefix: four `ASCII` letters or digits,
/// uppercased.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected text.
fn parse_sprite(text: &str) -> Result<String, CyberError> {
    let upper = text.to_ascii_uppercase();
    let well_formed = upper.len() == SPRITE_PREFIX_LEN
        && upper
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte.is_ascii_digit());
    if well_formed {
        return Ok(upper);
    }
    Err(CyberError::InvalidArgument(format!(
        "prefix {text:?} is not four ASCII letters or digits: pass the sprite prefix, e.g. \
         \"POSS\"; leave it out to list every sprite"
    )))
}

/// Records the one or two rotations a parsed sprite name contributes.
fn push_rotation(groups: &mut Groups, parsed: &SpriteName, mirror: Option<(char, u8)>, lump: &str) {
    let (frame, rotation, mirrored) = match mirror {
        None => (parsed.frame, parsed.rotation, false),
        Some((frame, rotation)) => (frame, rotation, true),
    };
    groups
        .entry(parsed.sprite.clone())
        .or_default()
        .entry(frame)
        .or_default()
        .push(RotationRow {
            rotation,
            lump: lump.to_owned(),
            mirrored,
        });
}

/// Whether an unparseable region name belongs in the `invalid` list:
/// without a prefix filter every one does, with one only names starting
/// with it.
fn shows_invalid(text: &str, prefix: Option<&str>) -> bool {
    prefix.is_none_or(|wanted| text.starts_with(wanted))
}

/// Collects one sprite lump's rotations into `groups`, honouring the
/// prefix filter.
fn collect_sprite(groups: &mut Groups, parsed: &SpriteName, lump: &str, prefix: Option<&str>) {
    if prefix.is_some_and(|wanted| wanted != parsed.sprite) {
        return;
    }
    push_rotation(groups, parsed, None, lump);
    if let Some(mirror) = parsed.mirror {
        push_rotation(groups, parsed, Some(mirror), lump);
    }
}

/// Orders the collected groups into rows: frames by letter (the
/// [`BTreeMap`] order), rotations by number then lump name.
fn sprite_rows(groups: Groups) -> Vec<SpriteRow> {
    groups
        .into_iter()
        .map(|(sprite, frames)| SpriteRow {
            sprite,
            frames: frames
                .into_iter()
                .map(|(frame, mut rotations)| {
                    rotations.sort_by(|first, second| {
                        first
                            .rotation
                            .cmp(&second.rotation)
                            .then_with(|| first.lump.cmp(&second.lump))
                    });
                    FrameRow { frame, rotations }
                })
                .collect(),
        })
        .collect()
}

/// `list_sprites`' work over the session. `prefix` filters by four
/// characters and must already be uppercased (`parse_prefix` normalises
/// the client's text).
///
/// # Errors
///
/// [`merged_rows`]' errors for the merged scope.
fn list_sprites_run(
    session: &Session,
    page: &Page,
    prefix: Option<&str>,
) -> Result<SpritesReport, CyberError> {
    let rows = merged_rows(session)?;
    let mut groups: Groups = BTreeMap::new();
    let mut invalid: Vec<String> = Vec::new();
    for row in last_wins(
        rows.iter()
            .filter(|row| in_namespace(row, Namespace::Sprites))
            .collect(),
    ) {
        let text = row.name.to_string();
        match parse_sprite_name(&text) {
            Some(parsed) => collect_sprite(&mut groups, &parsed, &text, prefix),
            None if shows_invalid(&text, prefix) => invalid.push(text),
            None => {}
        }
    }
    let (total, sprites) = page_rows(sprite_rows(groups), page);
    Ok(SpritesReport {
        total,
        sprites,
        invalid,
    })
}

/// The shared picture options of the image tools, flattened into each
/// tool's arguments.
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct ImageParams {
    /// Which `PLAYPAL` entry colours the picture (default 0: the normal
    /// palette). Past the entries the lump holds is `not_found`, as is a
    /// session with no `PLAYPAL`.
    #[serde(default)]
    palette: Option<usize>,
    /// How the asset is encoded: `"png"` (default) or `"rgba"` for raw
    /// row-major `RGBA8` bytes, `width*height*4` of them.
    #[serde(default)]
    format: Option<String>,
    /// Whole-pixel nearest-neighbour upscale applied before publishing,
    /// `1..=8` (default 1); it multiplies the reported dimensions and
    /// drawing offsets too.
    #[serde(default)]
    scale: Option<u32>,
}

/// Arguments for [`Cyberdemon::get_flat`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetFlatParams {
    /// The flat's lump name: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively.
    name: String,
    /// The shared `palette`, `format` and `scale` options.
    #[serde(flatten)]
    common: ImageParams,
}

/// Arguments for [`Cyberdemon::get_patch`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetPatchParams {
    /// The patch's lump name: 1 to 8 printable `ASCII` characters,
    /// matched case-insensitively.
    name: String,
    /// The shared `palette`, `format` and `scale` options.
    #[serde(flatten)]
    common: ImageParams,
}

/// Arguments for [`Cyberdemon::get_sprite`]: exactly one of `name` and
/// `sprite` says which picture to read.
#[derive(Debug, Deserialize, JsonSchema)]
struct GetSpriteParams {
    /// The whole sprite lump name to serve as stored, e.g. `"TROOA1"` (1
    /// to 8 printable `ASCII` characters, case-insensitive). Give this or
    /// `sprite`, never both.
    #[serde(default)]
    name: Option<String>,
    /// The four-character sprite name to resolve, e.g. `"TROO"` (case-
    /// insensitive). Give this with `frame`/`rotation` instead of `name`.
    #[serde(default)]
    sprite: Option<String>,
    /// Frame letter of the rotation to read: one character of `A`-`Z` then
    /// `[`, `\`, `]` (default `"A"`). With `sprite`.
    #[serde(default)]
    frame: Option<String>,
    /// Rotation to read, `0`-`8` (default 1): `0` is the all-angles image,
    /// `1`..`8` the eight views, served by the pair name that carries the
    /// rotation itself or, for its second rotation, flipped, when no lump
    /// names the rotation on its own. With `sprite`.
    #[serde(default)]
    rotation: Option<u8>,
    /// The shared `palette`, `format` and `scale` options.
    #[serde(flatten)]
    common: ImageParams,
}

/// Arguments for [`Cyberdemon::get_thing_sprite`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetThingSpriteParams {
    /// The thing type number to preview, as editors write it into
    /// `THINGS`, e.g. 3001 (the Imp). Looked up in the session's
    /// definitions: the game's built-in table plus its custom
    /// DEHACKED/DECORATE/ZScript/MAPINFO types.
    r#type: u32,
    /// Sprite frame letter to read: one character of `A`-`Z` then `[`,
    /// `\`, `]` (case-insensitive). Default: the definition's own frame.
    #[serde(default)]
    frame: Option<String>,
    /// Rotation to read, `1`-`8` (default 1): the eight views, `1` the
    /// front a head-on viewer sees, `5` the back. Resolved by the engine's
    /// pair-name and rotation-`0` fallback rules: a frame's rotation `2`
    /// lives in the lump named `TROOA2A8`, unflipped.
    #[serde(default)]
    rotation: Option<u8>,
    /// The shared `palette`, `format` and `scale` options.
    #[serde(flatten)]
    common: ImageParams,
}

/// Arguments for [`Cyberdemon::decode_patch`]: exactly one of `name` and
/// `source` says where the patch bytes come from.
#[derive(Debug, Default, Deserialize, JsonSchema)]
struct DecodePatchParams {
    /// The lump to decode, `Doom` patch format only: 1 to 8 printable
    /// `ASCII` characters, case-insensitive, resolved anywhere in the
    /// merged view. Give this or `source`, never both.
    #[serde(default)]
    name: Option<String>,
    /// Absolute path, `file://` URI, `http(s)://` `URL` or this server's
    /// asset `URL` of the patch file to decode. Give this or `name`, never
    /// both.
    #[serde(default)]
    source: Option<String>,
    /// Which `PLAYPAL` entry colours the picture (default 0).
    #[serde(default)]
    palette: Option<usize>,
}

/// How an image tool encodes the asset it publishes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssetFormat {
    /// A `PNG`, with a `grAb` chunk carrying the drawing offsets when the
    /// source has any.
    Png,
    /// Raw row-major `RGBA8` bytes.
    Rgba,
}

impl AssetFormat {
    /// The client's `format` word, defaulting to `png`.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] for any other name.
    fn parse(text: Option<&str>) -> Result<Self, CyberError> {
        match text {
            None | Some("png") => Ok(Self::Png),
            Some("rgba") => Ok(Self::Rgba),
            Some(other) => Err(CyberError::InvalidArgument(format!(
                "format {other:?} is unknown: pass \"png\" for a PNG asset or \"rgba\" for raw \
                 RGBA8 bytes"
            ))),
        }
    }

    /// This encoding's filename extension and media type.
    const fn extension(self) -> (&'static str, &'static str) {
        match self {
            Self::Png => ("png", "image/png"),
            Self::Rgba => ("rgba", RGBA_MIME),
        }
    }
}

/// The validated `palette`, `format` and `scale` of one image call.
#[derive(Debug)]
struct ImageOptions {
    /// `PLAYPAL` entry to colour with.
    palette: usize,
    /// How to encode the asset.
    format: AssetFormat,
    /// Whole-pixel upscale factor.
    scale: u32,
}

/// The validated options of one image call.
///
/// # Errors
///
/// [`AssetFormat::parse`] and [`parse_scale`]' [`CyberError::InvalidArgument`].
fn parse_options(common: &ImageParams) -> Result<ImageOptions, CyberError> {
    Ok(ImageOptions {
        palette: common.palette.unwrap_or(DEFAULT_PALETTE),
        format: AssetFormat::parse(common.format.as_deref())?,
        scale: parse_scale(common.scale)?,
    })
}

/// The `scale` factor: a whole number in `1..=8`, defaulting to 1.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected factor.
fn parse_scale(value: Option<u32>) -> Result<u32, CyberError> {
    match value {
        None => Ok(DEFAULT_SCALE),
        Some(factor @ (DEFAULT_SCALE..=MAX_SCALE)) => Ok(factor),
        Some(factor) => Err(CyberError::InvalidArgument(format!(
            "scale {factor} is out of range: pass a whole factor between {DEFAULT_SCALE} and \
             {MAX_SCALE}"
        ))),
    }
}

/// A picture's pixels and drawing offsets, rendered and ready to publish.
struct Rendered {
    /// The scaled pixels.
    image: RgbaImage,
    /// Horizontal drawing offset in published pixels; `None` for a flat.
    left_offset: Option<i32>,
    /// Vertical drawing offset in published pixels; `None` for a flat.
    top_offset: Option<i32>,
}

/// What one image call answers with: the picture plus the sprite extras
/// the plain image tools leave out.
struct Prepared {
    /// The name the report answers with.
    name: LumpName,
    /// The lump that actually served the image (`get_sprite` only).
    lump: Option<LumpName>,
    /// Whether a stored sprite image was flipped (`get_sprite` only).
    mirrored: Option<bool>,
    /// The pixels and offsets to publish.
    rendered: Rendered,
}

impl Prepared {
    /// The answer for a flat or a patch: the requested name, no extras.
    const fn plain(name: LumpName, rendered: Rendered) -> Self {
        Self {
            name,
            lump: None,
            mirrored: None,
            rendered,
        }
    }

    /// The answer for a sprite call, which also names the lump that served
    /// the rotation and whether its stored image was flipped.
    const fn served(name: LumpName, lump: LumpName, mirrored: bool, rendered: Rendered) -> Self {
        Self {
            name,
            lump: Some(lump),
            mirrored: Some(mirrored),
            rendered,
        }
    }
}

/// Structured result of `get_flat`, `get_patch` and `get_sprite`.
#[derive(Serialize)]
struct ImageReport {
    /// The picture's name: the flat or patch lump, or the sprite name the
    /// call asked for (`TROOA8`).
    name: LumpName,
    /// Width in published pixels: `scale` times the stored image.
    width: u32,
    /// Height in published pixels.
    height: u32,
    /// Horizontal drawing offset in published pixels; absent for flats,
    /// which have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    left_offset: Option<i32>,
    /// Vertical drawing offset in published pixels, like `left_offset`.
    #[serde(skip_serializing_if = "Option::is_none")]
    top_offset: Option<i32>,
    /// The lump whose pixels served the call; `get_sprite` only, because a
    /// mirror-pair or all-angles lump can stand in for the requested name.
    #[serde(skip_serializing_if = "Option::is_none")]
    lump: Option<LumpName>,
    /// Whether the stored sprite image was flipped horizontally to serve
    /// the rotation; `get_sprite` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    mirrored: Option<bool>,
    /// The published picture.
    asset: AssetRef,
}

/// Colours `image` with the session's requested palette, upscales it and
/// keeps the drawing offsets scaled along.
///
/// # Errors
///
/// [`load_palette`]' [`CyberError::NotFound`] for a palette index the
/// session's `PLAYPAL` does not hold.
fn render_indexed(
    session: &Session,
    image: &IndexedImage,
    offsets: Option<(i16, i16)>,
    options: &ImageOptions,
) -> Result<Rendered, CyberError> {
    let palette = load_palette(session, options.palette)?;
    Ok(Rendered {
        image: scale_integer(&image.to_rgba(&palette), options.scale),
        left_offset: offsets.map(|(left, _)| scale_offset(left, options.scale)),
        top_offset: offsets.map(|(_, top)| scale_offset(top, options.scale)),
    })
}

/// The render of a patch or sprite: its pixels plus both drawing offsets.
///
/// # Errors
///
/// [`render_indexed`]' errors.
fn render_patch(
    session: &Session,
    patch: &Patch,
    options: &ImageOptions,
) -> Result<Rendered, CyberError> {
    let offsets = (patch.left_offset, patch.top_offset);
    render_indexed(session, &patch.image, Some(offsets), options)
}

/// One drawing offset in published pixels: `scale` times the stored
/// offset, keeping it as is when the product would not fit.
fn scale_offset(value: i16, scale: u32) -> i32 {
    let factor = i32::try_from(scale).unwrap_or(1_i32);
    let scaled = i32::from(value);
    scaled.checked_mul(factor).unwrap_or(scaled)
}

/// The `grAb` offsets a published `PNG` carries: a render's drawing
/// offsets, and none for a flat.
fn grab_of(rendered: &Rendered) -> Option<GrabOffsets> {
    let (left, top) = (rendered.left_offset?, rendered.top_offset?);
    Some((left, top))
}

/// The asset bytes of one render in one encoding.
///
/// # Errors
///
/// [`encode_png`]' errors for a `PNG`; raw `RGBA8` bytes cannot fail.
fn encode_rendered(rendered: &Rendered, format: AssetFormat) -> Result<Vec<u8>, CyberError> {
    match format {
        AssetFormat::Png => encode_png(&rendered.image, grab_of(rendered)),
        AssetFormat::Rgba => Ok(rendered.image.pixels.clone()),
    }
}

/// Publishes one render as `NAME.png` or `NAME.rgba`.
///
/// # Errors
///
/// [`encode_rendered`]' errors and [`AppState::publish`]' for the store.
fn publish_image(
    state: &AppState,
    name: &str,
    rendered: &Rendered,
    options: &ImageOptions,
) -> Result<AssetRef, CyberError> {
    let (extension, mime) = options.format.extension();
    let data = encode_rendered(rendered, options.format)?;
    state.publish(&format!("{name}.{extension}"), mime, data)
}

/// Publishes a prepared picture and assembles the shared report.
///
/// # Errors
///
/// [`publish_image`]' errors.
fn finish(
    state: &AppState,
    prepared: Prepared,
    options: &ImageOptions,
) -> Result<ImageReport, CyberError> {
    let Prepared {
        name,
        lump,
        mirrored,
        rendered,
    } = prepared;
    let width = rendered.image.width;
    let height = rendered.image.height;
    let left_offset = rendered.left_offset;
    let top_offset = rendered.top_offset;
    let asset = publish_image(state, &name.to_string(), &rendered, options)?;
    Ok(ImageReport {
        name,
        width,
        height,
        left_offset,
        top_offset,
        lump,
        mirrored,
        asset,
    })
}

/// `get_flat`'s work: resolve and render under the session lock, then
/// publish outside it.
///
/// # Errors
///
/// [`load_flat`]' errors for the lump and [`finish`]' for the asset.
fn get_flat_run(
    state: &AppState,
    name: LumpName,
    options: &ImageOptions,
) -> Result<ImageReport, CyberError> {
    let prepared = state.with_session(|session| {
        let image = load_flat(session, name)?;
        Ok(Prepared::plain(
            name,
            render_indexed(session, &image, None, options)?,
        ))
    })?;
    finish(state, prepared, options)
}

/// `get_patch`'s work: resolve and render under the session lock, then
/// publish outside it.
///
/// # Errors
///
/// [`load_patch`]' errors for the lump and [`finish`]' for the asset.
fn get_patch_run(
    state: &AppState,
    name: LumpName,
    options: &ImageOptions,
) -> Result<ImageReport, CyberError> {
    let prepared = state.with_session(|session| {
        let patch = load_patch(session, name)?;
        Ok(Prepared::plain(
            name,
            render_patch(session, &patch, options)?,
        ))
    })?;
    finish(state, prepared, options)
}

/// What a `get_sprite` call names: one lump exactly, or one rotation.
#[derive(Debug)]
enum SpriteRequest {
    /// The whole lump name to serve as stored (`TROOA2A8`).
    Lump(LumpName),
    /// One rotation of a sprite, resolved by the engine's rules.
    Rotation {
        /// The name this rotation would carry: what the report answers
        /// with (`TROOA8`).
        name: LumpName,
        /// Four-character sprite prefix, uppercased.
        sprite: String,
        /// Frame letter, uppercased.
        frame: char,
        /// Rotation `0`-`8`.
        rotation: u8,
    },
}

/// Which sprite one `get_sprite` call asks for: exactly one of `name` and
/// `sprite`, with `frame` (default `A`) and `rotation` (default 1).
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for both or neither selector, or for a
/// malformed `sprite`, `frame` or `rotation`.
fn sprite_request(args: &GetSpriteParams) -> Result<SpriteRequest, CyberError> {
    match (args.name.as_deref(), args.sprite.as_deref()) {
        (Some(text), None) => Ok(SpriteRequest::Lump(LumpName::parse(text)?)),
        (None, Some(text)) => by_rotation(text, args.frame.as_deref(), args.rotation),
        (Some(_), Some(_)) => Err(CyberError::InvalidArgument(
            "get_sprite takes either name or sprite, not both: name one lump like \"TROOA1\", \
             or give sprite with frame and rotation"
                .to_owned(),
        )),
        (None, None) => Err(CyberError::InvalidArgument(
            "get_sprite needs a sprite to read: pass the full lump name, or the four-character \
             sprite with an optional frame and rotation"
                .to_owned(),
        )),
    }
}

/// The rotation request behind a `sprite` with its `frame` and `rotation`.
///
/// # Errors
///
/// [`parse_sprite`], [`parse_frame`] and [`parse_rotation`]'
/// [`CyberError::InvalidArgument`] errors.
fn by_rotation(
    sprite: &str,
    frame: Option<&str>,
    rotation: Option<u8>,
) -> Result<SpriteRequest, CyberError> {
    let prefix = parse_sprite(sprite)?;
    let letter = parse_frame(frame)?;
    let number = parse_rotation(rotation)?;
    Ok(SpriteRequest::Rotation {
        name: LumpName::parse(&format!("{prefix}{letter}{number}"))?,
        sprite: prefix,
        frame: letter,
        rotation: number,
    })
}

/// The frame letter: one character of `A`-`Z` then `[`, `\`, `]`,
/// uppercased, defaulting to [`DEFAULT_FRAME`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any other text.
fn parse_frame(text: Option<&str>) -> Result<char, CyberError> {
    let Some(raw) = text else {
        return Ok(DEFAULT_FRAME);
    };
    let upper = raw.to_ascii_uppercase();
    let single = upper.chars().count() == 1;
    if let Some(letter) = upper.chars().next()
        && single
        && is_frame_letter(letter)
    {
        return Ok(letter);
    }
    Err(CyberError::InvalidArgument(format!(
        "frame {text:?} is not one sprite frame letter: pass a single character A-Z, [, \\ or ]"
    )))
}

/// Whether `letter` is a sprite frame letter: `A`-`Z`, then the three
/// characters after `Z`.
const fn is_frame_letter(letter: char) -> bool {
    matches!(letter, 'A'..='Z' | '[' | '\\' | ']')
}

/// The rotation number: `0`-`8`, defaulting to [`DEFAULT_ROTATION`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected number.
fn parse_rotation(value: Option<u8>) -> Result<u8, CyberError> {
    match value {
        None => Ok(DEFAULT_ROTATION),
        Some(rotation @ (0_u8..=MAX_ROTATION)) => Ok(rotation),
        Some(rotation) => Err(CyberError::InvalidArgument(format!(
            "rotation {rotation} is out of range: pass 0 for the all-angles image, or 1 to \
             {MAX_ROTATION} for the eight views"
        ))),
    }
}

/// `get_sprite`'s work: resolve and render under the session lock, then
/// publish outside it.
///
/// # Errors
///
/// [`prepare_sprite`]' errors and [`finish`]' for the asset.
fn get_sprite_run(
    state: &AppState,
    request: SpriteRequest,
    options: &ImageOptions,
) -> Result<ImageReport, CyberError> {
    let prepared = state.with_session(|session| prepare_sprite(session, request, options))?;
    finish(state, prepared, options)
}

/// Resolves one sprite request and renders the picture it names: a whole
/// lump as stored, or a rotation by the engine's rules.
///
/// # Errors
///
/// [`load_sprite_lump`] and [`load_sprite`]' errors, and
/// [`render_patch`]' for the pixels.
fn prepare_sprite(
    session: &Session,
    request: SpriteRequest,
    options: &ImageOptions,
) -> Result<Prepared, CyberError> {
    match request {
        SpriteRequest::Lump(name) => {
            let patch = load_sprite_lump(session, name)?;
            let rendered = render_patch(session, &patch, options)?;
            Ok(Prepared::served(name, name, false, rendered))
        }
        SpriteRequest::Rotation {
            name,
            sprite,
            frame,
            rotation,
        } => {
            let found = load_sprite(session, &sprite, frame, rotation)?;
            let rendered = render_patch(session, &found.patch, options)?;
            Ok(Prepared::served(name, found.lump, found.mirrored, rendered))
        }
    }
}

/// The validated selection of one `get_thing_sprite` call.
#[derive(Debug)]
struct ThingRequest {
    /// Thing type number, as the client passed it.
    kind: u32,
    /// Requested frame letter, `None` for the definition's own.
    frame: Option<char>,
    /// Rotation number `1..=8`.
    rotation: u8,
}

/// The selection behind a `get_thing_sprite` call: the type number with
/// its optional `frame` and `rotation` (default 1).
///
/// # Errors
///
/// [`parse_frame`] and [`parse_thing_rotation`]' [`CyberError::InvalidArgument`]
/// errors.
fn thing_request(args: &GetThingSpriteParams) -> Result<ThingRequest, CyberError> {
    Ok(ThingRequest {
        kind: args.r#type,
        frame: args
            .frame
            .as_deref()
            .map(|text| parse_frame(Some(text)))
            .transpose()?,
        rotation: parse_thing_rotation(args.rotation)?,
    })
}

/// The `get_thing_sprite` rotation number: `1..=8`, defaulting to
/// [`DEFAULT_ROTATION`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected number.
fn parse_thing_rotation(value: Option<u8>) -> Result<u8, CyberError> {
    match value {
        None => Ok(DEFAULT_ROTATION),
        Some(rotation @ 1_u8..=MAX_ROTATION) => Ok(rotation),
        Some(rotation) => Err(CyberError::InvalidArgument(format!(
            "rotation {rotation} is out of range: get_thing_sprite serves the eight views 1 to \
             {MAX_ROTATION}; get_sprite also takes the all-angles 0"
        ))),
    }
}

/// The game whose definition tables the session selects: the detected
/// game, and DOOM II for an unidentified session, as `get_thing_types`
/// defaults.
pub(in crate::tools) fn session_game(session: &Session) -> Game {
    let detected = session.game();
    if detected == Game::Unknown {
        Game::Doom2
    } else {
        detected
    }
}

/// The session's thing definitions: the built-in tables of the session's
/// game overlaid with the custom sources of the merged view.
///
/// # Errors
///
/// [`session_definitions`]' errors for the tables; collection errors for
/// the merged scope.
fn session_things(session: &Session) -> Result<Definitions, CyberError> {
    let (sources, _notes): (CustomSources, Vec<String>) = custom_sources(session)?;
    session_definitions(session_game(session), &sources)
}

/// The definition one `get_thing_sprite` call previews: a known type with
/// a non-empty sprite.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a type the session definitions do not
/// hold, and for a definition whose sprite is empty.
fn thing_sprite_def(session: &Session, kind: u32) -> Result<ThingDef, CyberError> {
    let def = session_things(session)?
        .thing(kind)
        .cloned()
        .ok_or_else(|| {
            CyberError::NotFound(format!(
                "no thing type {kind} in the session definitions; get_thing_types lists what the \
                 game has, and custom types need their archive loaded"
            ))
            .with_hint("get_thing_types with a search term")
        })?;
    if def.sprite.is_empty() {
        return Err(CyberError::NotFound(format!(
            "thing type {kind} has no sprite; get_thing_types shows which types define one"
        )));
    }
    Ok(def)
}

/// The `not_found` for a rotation no sprite lump serves: names the
/// expected lump pattern, the rotation's own image and the frame's
/// all-angles image.
fn missing_thing_sprite(sprite: &str, frame: char, rotation: u8) -> CyberError {
    CyberError::NotFound(format!(
        "no {sprite}{frame}{rotation} / {sprite}{frame}0 sprite lump in the merged view; \
         list_sprites with prefix \"{sprite}\" lists what the session has"
    ))
}

/// What the session-side half of `get_thing_sprite` prepared.
struct PreparedThing {
    /// The definition row the picture came from.
    def: ThingDef,
    /// The frame letter that was served.
    frame: char,
    /// The rotation name the call resolves to (`TROOA1`), published under.
    name: LumpName,
    /// The lump whose pixels served the rotation.
    lump: LumpName,
    /// Whether the stored image was flipped to serve it.
    mirrored: bool,
    /// The rendered picture.
    rendered: Rendered,
}

/// `get_thing_sprite`' work under the session lock: definition lookup,
/// rotation resolution and rendering.
///
/// # Errors
///
/// [`thing_sprite_def`]' and [`missing_thing_sprite`]' [`CyberError::NotFound`]
/// errors, [`load_sprite`]' and [`render_patch`]' errors.
fn prepare_thing_sprite(
    session: &Session,
    request: &ThingRequest,
    options: &ImageOptions,
) -> Result<PreparedThing, CyberError> {
    let def = thing_sprite_def(session, request.kind)?;
    let frame = request.frame.unwrap_or(def.frame);
    let sprite = def.sprite.clone();
    let found = match load_sprite(session, &sprite, frame, request.rotation) {
        Ok(found) => found,
        Err(error) if error.code() == "not_found" => {
            return Err(missing_thing_sprite(&sprite, frame, request.rotation));
        }
        Err(error) => return Err(error),
    };
    let rotation = request.rotation;
    let name = LumpName::parse(&format!("{sprite}{frame}{rotation}"))?;
    let rendered = render_patch(session, &found.patch, options)?;
    Ok(PreparedThing {
        def,
        frame,
        name,
        lump: found.lump,
        mirrored: found.mirrored,
        rendered,
    })
}

/// Structured result of [`Cyberdemon::get_thing_sprite`].
#[derive(Serialize)]
struct ThingSpriteReport {
    /// The thing type number that was asked for.
    r#type: u32,
    /// The definition's editor display name.
    name: String,
    /// The four-character sprite name.
    sprite: String,
    /// The frame letter the picture came from.
    frame: char,
    /// The rotation number `1..=8`.
    rotation: u8,
    /// The lump whose pixels served the rotation.
    lump: LumpName,
    /// Whether the stored image was flipped horizontally to serve it.
    mirrored: bool,
    /// Width in published pixels: `scale` times the stored image.
    width: u32,
    /// Height in published pixels.
    height: u32,
    /// Horizontal drawing offset in published pixels.
    left_offset: i32,
    /// Vertical drawing offset in published pixels.
    top_offset: i32,
    /// Whether the thing hangs from the ceiling.
    hanging: bool,
    /// The published picture.
    asset: AssetRef,
}

/// `get_thing_sprite`'s work: resolve and render under the session lock,
/// then publish outside it.
///
/// # Errors
///
/// [`prepare_thing_sprite`]' errors and [`publish_image`]' for the asset.
fn get_thing_sprite_run(
    state: &AppState,
    request: &ThingRequest,
    options: &ImageOptions,
) -> Result<ThingSpriteReport, CyberError> {
    let prepared = state.with_session(|session| prepare_thing_sprite(session, request, options))?;
    let PreparedThing {
        def,
        frame,
        name,
        lump,
        mirrored,
        rendered,
    } = prepared;
    let (width, height) = (rendered.image.width, rendered.image.height);
    let left_offset = rendered.left_offset.unwrap_or(0_i32);
    let top_offset = rendered.top_offset.unwrap_or(0_i32);
    let asset = publish_image(state, &name.to_string(), &rendered, options)?;
    Ok(ThingSpriteReport {
        r#type: def.kind,
        name: def.name,
        sprite: def.sprite,
        frame,
        rotation: request.rotation,
        lump,
        mirrored,
        width,
        height,
        left_offset,
        top_offset,
        hanging: def.hanging,
        asset,
    })
}

/// The name and bytes of the patch one `decode_patch` call reads, from the
/// merged view or from a fetched file: exactly one of the two.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for both `name` and `source` or for
/// neither; [`lump_bytes`]' and [`AppState::fetch_source`]' errors for the
/// chosen one.
async fn patch_payload(
    state: &AppState,
    args: &DecodePatchParams,
) -> Result<(String, Vec<u8>), CyberError> {
    match (args.name.as_deref(), args.source.as_deref()) {
        (Some(text), None) => {
            let name = LumpName::parse(text)?;
            let data = state.with_session(|session| lump_bytes(session, name))?;
            Ok((name.to_string(), data))
        }
        (None, Some(uri)) => {
            let fetched = state.fetch_source(uri).await?;
            Ok((display_name(&fetched.filename), fetched.bytes))
        }
        (Some(_), Some(_)) => Err(CyberError::InvalidArgument(
            "decode_patch takes either name or source, not both: name one session lump or one \
             external file"
                .to_owned(),
        )),
        (None, None) => Err(CyberError::InvalidArgument(
            "decode_patch needs a patch to read: pass the name of a session lump or the source \
             of a file"
                .to_owned(),
        )),
    }
}

/// The name a fetched file is reported and published under: the lump name
/// its stem allows, else its filename, else [`FALLBACK_NAME`].
fn display_name(filename: &str) -> String {
    derive_lump_name(filename)
        .map(|name| name.to_string())
        .or_else(|| (!filename.is_empty()).then(|| filename.to_owned()))
        .unwrap_or_else(|| FALLBACK_NAME.to_owned())
}

/// Structured result of [`Cyberdemon::decode_patch`]: one patch as both
/// encodings.
#[derive(Serialize)]
struct DecodedPatchReport {
    /// The patch's name: the lump, or the fetched file's name.
    name: String,
    /// Width in pixels.
    width: u32,
    /// Height in pixels.
    height: u32,
    /// Horizontal drawing offset in pixels.
    left_offset: i32,
    /// Vertical drawing offset in pixels.
    top_offset: i32,
    /// The `PLAYPAL` entry both published pictures are coloured with.
    palette: usize,
    /// The published `PNG`, drawing offsets in a `grAb` chunk.
    png: AssetRef,
    /// The same picture as raw `RGBA8` bytes.
    rgba: AssetRef,
}

/// `decode_patch`'s work: fetch or read the bytes, decode the `Doom`
/// format and publish both encodings.
///
/// # Errors
///
/// [`patch_payload`]' errors for the arguments, [`decode_patch`]'
/// [`CyberError::Format`] for bytes that are no patch,
/// [`load_palette`]' [`CyberError::NotFound`] for the palette and
/// [`AppState::publish`]' errors for the two assets.
async fn decode_patch_run(
    state: &AppState,
    args: &DecodePatchParams,
) -> Result<DecodedPatchReport, CyberError> {
    let index = args.palette.unwrap_or(DEFAULT_PALETTE);
    let (name, data) = patch_payload(state, args).await?;
    let patch = decode_patch(&data)?;
    let palette = state.with_session(|session| load_palette(session, index))?;
    let image = patch.image.to_rgba(&palette);
    let left_offset = i32::from(patch.left_offset);
    let top_offset = i32::from(patch.top_offset);
    let rgba = image.pixels.clone();
    let (width, height) = (image.width, image.height);
    let png_asset = state.publish(
        &format!("{name}.png"),
        "image/png",
        encode_png(&image, Some((left_offset, top_offset)))?,
    )?;
    let rgba_asset = state.publish(&format!("{name}.rgba"), RGBA_MIME, rgba)?;
    Ok(DecodedPatchReport {
        name,
        width,
        height,
        left_offset,
        top_offset,
        palette: index,
        png: png_asset,
        rgba: rgba_asset,
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = graphics_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists the flat textures of the session.
    #[tool(
        description = "Lists the flat lumps of the merged view's flats regions (F_START/F_END or FF_START/FF_END markers, last same-name copy winning as an engine reads), with name, size, format, dimensions and source archive. pattern is a case-insensitive glob. Use it to find a flat before edit_sector or replace_flat; get_flat publishes one as a picture."
    )]
    fn list_flats(
        &self,
        Parameters(args): Parameters<ListFlatsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match parse_page(args.offset, args.limit) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_flats_run(session, &page, args.pattern.as_deref()));
        match report {
            Ok(success) => ok(&success),
            Err(error) => fail(&error),
        }
    }

    /// Lists the patch lumps of the session.
    #[tool(
        description = "Lists the patch lumps of the merged view's patches regions (P_START/P_END or PP_START/PP_END, plus PNAMES-listed loose copies), last same-name winning, with dimensions, offsets and source. pattern filters names. Use it before add_patch or build_texture to see what exists; get_patch publishes one as an image."
    )]
    fn list_patches(
        &self,
        Parameters(args): Parameters<ListPatchesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match parse_page(args.offset, args.limit) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let report = self.state().with_session(|session| {
            list_patches_run(
                session,
                &page,
                args.pattern.as_deref(),
                args.in_pnames_only.unwrap_or(false),
            )
        });
        match report {
            Ok(success) => ok(&success),
            Err(error) => fail(&error),
        }
    }

    /// Lists the sprite lumps of the session, grouped by sprite.
    #[tool(
        description = "Groups the merged view's sprite-region lumps (S_START/S_END or SS_START/SS_END) by sprite, frame and the rotations their names serve, with an optional prefix filter. Use it before import_image_as_sprite or create_sprite_from_image to see which rotations exist; get_sprite publishes one rotation as a picture."
    )]
    fn list_sprites(
        &self,
        Parameters(args): Parameters<ListSpritesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let page = match parse_page(args.offset, args.limit) {
            Ok(page) => page,
            Err(error) => return fail(&error),
        };
        let prefix = match parse_prefix(args.prefix.as_deref()) {
            Ok(prefix) => prefix,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_sprites_run(session, &page, prefix.as_deref()));
        match report {
            Ok(success) => ok(&success),
            Err(error) => fail(&error),
        }
    }

    /// Publishes one flat as an image asset.
    #[tool(
        description = "Publishes one flat as an image asset on the localhost server and reports its dimensions; nothing is edited. name resolves the way an engine resolves it in the merged view (last flats-region match wins); raw vanilla sizes and ZDoom PNG flats decode, palette picks the PLAYPAL entry, scale upscales and format picks png or rgba. A name no archive holds is not_found - list_flats shows the names."
    )]
    fn get_flat(
        &self,
        Parameters(args): Parameters<GetFlatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let options = match parse_options(&args.common) {
            Ok(options) => options,
            Err(error) => return fail(&error),
        };
        let name = match LumpName::parse(&args.name) {
            Ok(name) => name,
            Err(error) => return fail(&error),
        };
        match get_flat_run(self.state(), name, &options) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Publishes one patch lump as an image asset.
    #[tool(
        description = "Publishes one patch lump (a texture component, HUD or port graphic) as a PNG image asset with its drawing offsets, or raw RGBA bytes with format \"rgba\"; nothing is edited. name resolves the merged view with the last patches-region match winning; palette and scale tune the picture. list_patches shows the names; decode_patch also reads external files."
    )]
    fn get_patch(
        &self,
        Parameters(args): Parameters<GetPatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let options = match parse_options(&args.common) {
            Ok(options) => options,
            Err(error) => return fail(&error),
        };
        let name = match LumpName::parse(&args.name) {
            Ok(name) => name,
            Err(error) => return fail(&error),
        };
        match get_patch_run(self.state(), name, &options) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Publishes one sprite lump or rotation as an image asset.
    #[tool(
        description = "Publishes one sprite rotation as an image asset; nothing is edited. Either name gives a whole sprite lump name served as stored, or sprite with optional frame (default A) and rotation (0..=8, default 1) resolves the way the engine's viewer-angle buckets do, mirrored frames included. Both or neither is invalid_argument. get_thing_sprite starts from a doomednum."
    )]
    fn get_sprite(
        &self,
        Parameters(args): Parameters<GetSpriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = match sprite_request(&args) {
            Ok(request) => request,
            Err(error) => return fail(&error),
        };
        let options = match parse_options(&args.common) {
            Ok(options) => options,
            Err(error) => return fail(&error),
        };
        match get_sprite_run(self.state(), request, &options) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Previews one thing type's sprite rotation as an image asset.
    #[tool(
        description = "Publishes what one thing type looks like in game as an image asset; nothing is edited. type is looked up in the session-aware definitions - the detected game's table overlaid with DEHACKED, DECORATE, ZScript and MAPINFO customs, so load the archives first; optional frame and rotation pick the view. An unknown type is not_found: get_thing_types lists the numbers."
    )]
    fn get_thing_sprite(
        &self,
        Parameters(args): Parameters<GetThingSpriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let request = match thing_request(&args) {
            Ok(request) => request,
            Err(error) => return fail(&error),
        };
        let options = match parse_options(&args.common) {
            Ok(options) => options,
            Err(error) => return fail(&error),
        };
        match get_thing_sprite_run(self.state(), &request, &options) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }

    /// Decodes a Doom-format patch from a lump or a file and publishes it
    /// as both a PNG and raw RGBA.
    #[tool(
        description = "Decodes one Doom-format patch - from a session lump (name) or an external file (source; exactly one) - and publishes it both ways: a PNG with the grAb offsets and raw RGBA8 bytes, so you can pick an encoding; nothing is edited. Tall DeePsea patches read; PNG bytes are not patch data, use get_asset_url for those."
    )]
    async fn decode_patch(
        &self,
        Parameters(args): Parameters<DecodePatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match decode_patch_run(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use std::path::PathBuf;

    use super::{
        AssetFormat, GetSpriteParams, GetThingSpriteParams, Groups, ImageParams, Page, Prepared,
        Rendered, SpriteRequest, ThingRequest, collect_sprite, display_name, grab_of, in_namespace,
        is_patch_candidate, last_wins, list_flats_run, list_patches_run, list_sprites_run,
        missing_thing_sprite, parse_frame, parse_options, parse_page, parse_prefix, parse_rotation,
        parse_sprite, parse_thing_rotation, patch_row, pnames_of, prepare_thing_sprite,
        scale_offset, session_game, sprite_request, sprite_rows, thing_request, thing_sprite_def,
    };
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::sprite_name::parse_sprite_name;
    use crate::session::{Layer, LayerRole, Scope, Session, Source};
    use crate::tools::lumps::rows::{LumpRow, build_rows};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::game::Game;
    use crate::wad::name::LumpName;
    use crate::wad::textures::write_pnames;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A page for the listing calls.
    fn page(offset: usize, limit: usize) -> Page {
        Page { offset, limit }
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A `Doom` patch header: dimensions and offsets, no columns.
    fn header(width: u16, height: u16, left: i16, top: i16) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&left.to_le_bytes());
        bytes.extend_from_slice(&top.to_le_bytes());
        bytes
    }

    /// A `PNG` lump of the given shape with optional `grAb` offsets.
    fn png(width: u32, height: u32, offsets: Option<(i32, i32)>) -> Vec<u8> {
        let pixels = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit")
            .checked_mul(4)
            .expect("test pixel bytes fit");
        let img = RgbaImage::new(width, height, vec![0x40_u8; pixels]).expect("test shapes fit");
        encode_png(&img, offsets).expect("test PNGs encode")
    }

    /// The fixture archive: two raw flats and a `PNG` flat (with a repeated
    /// `FLOOR1` to exercise last-wins), two region patches, a `PNG` patch
    /// global named by `PNAMES`, and a sprites region with the mirrored
    /// grammar plus one malformed name.
    fn fixture() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("gfx.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::marker(name("F_START")),
                    Lump::new(name("FLOOR1"), vec![1_u8; 4_096]),
                    Lump::new(name("FLOOR1"), vec![3_u8; 8_192]),
                    Lump::new(name("PNGFLAT"), png(64, 64, None)),
                    Lump::new(name("ODDFLET"), vec![7_u8; 100]),
                    Lump::marker(name("F_END")),
                    Lump::marker(name("P_START")),
                    Lump::new(name("WALLP1"), header(16, 16, -2, -14)),
                    Lump::new(name("JUNKP1"), vec![b'x'; 40]),
                    Lump::marker(name("P_END")),
                    Lump::marker(name("S_START")),
                    Lump::new(name("TROOA1"), header(8, 8, 4, 4)),
                    Lump::new(name("TROOA2A8"), header(8, 8, 4, 4)),
                    Lump::new(name("PLAYA0"), header(16, 16, 0, 0)),
                    Lump::new(name("BADNAME"), vec![b'?'; 5]),
                    Lump::marker(name("S_END")),
                    Lump::new(name("PNGPAT"), png(4, 2, Some((3_i32, -4_i32)))),
                    Lump::new(
                        name("PNAMES"),
                        write_pnames(&[
                            "WALLP1".to_owned(),
                            "PNGPAT".to_owned(),
                            "TROOA1".to_owned(),
                        ])
                        .expect("test names are valid"),
                    ),
                ],
            },
        ));
        session
    }

    /// The fixture rows of the merged view.
    fn rows(session: &Session) -> Vec<LumpRow> {
        build_rows(session, Scope::Merged).expect("the fixture lists")
    }

    /// The row named `text` of the fixture session.
    fn row_named<'rows>(rows: &'rows [LumpRow], text: &str) -> &'rows LumpRow {
        rows.iter()
            .find(|row| row.name == name(text))
            .unwrap_or_else(|| panic!("no fixture row {text}"))
    }

    #[test]
    fn pages_default_and_reject_out_of_range_limits() {
        let fresh = parse_page(None, None).expect("defaults");
        assert_eq!((fresh.offset, fresh.limit), (0, 200), "documented defaults");
        assert_eq!(
            parse_page(Some(3), Some(2000)).expect("max").limit,
            2000,
            "the cap"
        );
        for bad in [0_usize, 2001] {
            let error = parse_page(None, Some(bad)).expect_err("out of range");
            assert_eq!(code(&error), "invalid_argument", "limit {bad} code");
        }
    }

    #[test]
    fn last_wins_keeps_the_final_occurrence_in_order() {
        let rows = rows(&fixture());
        let flats: Vec<&LumpRow> = rows
            .iter()
            .filter(|row| in_namespace(row, Namespace::Flats))
            .collect();
        let deduped = last_wins(flats.clone());
        let floor1 = deduped
            .iter()
            .find(|row| row.name == name("FLOOR1"))
            .expect("FLOOR1 stays once");
        assert_eq!(floor1.size, 8_192, "the second copy wins");
        assert_eq!(
            deduped.len().saturating_add(1),
            flats.len(),
            "exactly one row was dropped"
        );
        let names: Vec<String> = deduped.iter().map(|row| row.name.to_string()).collect();
        assert_eq!(
            names,
            vec![
                "FLOOR1".to_owned(),
                "PNGFLAT".to_owned(),
                "ODDFLET".to_owned()
            ],
            "directory order is kept"
        );
    }

    #[test]
    fn flats_report_raw_shapes_png_and_unknown_sizes() {
        let session = fixture();
        let report = list_flats_run(&session, &page(0, 10), None).expect("runs");
        assert_eq!(report.total, 3, "FLOOR1 (deduped), PNGFLAT, ODDFLET");
        let floor = report.flats.first().expect("FLOOR1");
        assert_eq!(
            (floor.width, floor.height, floor.format),
            (Some(64), Some(128), "raw"),
            "the winning 8192-byte copy"
        );
        let pngflat = report.flats.get(1).expect("PNGFLAT");
        assert_eq!(
            (pngflat.width, pngflat.height, pngflat.format),
            (Some(64), Some(64), "png"),
            "dimensions from the IHDR"
        );
        let odd = report.flats.get(2).expect("ODDFLET");
        assert_eq!(
            (odd.width, odd.height, odd.format),
            (None, None, "raw"),
            "100 bytes imply no vanilla shape"
        );
        assert_eq!(odd.source, Source::Layer(0), "read from the layer");
        let filtered = list_flats_run(&session, &page(0, 10), Some("floo*")).expect("runs");
        assert_eq!(filtered.total, 1, "the glob applies");
        let paged = list_flats_run(&session, &page(1, 1), None).expect("runs");
        assert_eq!(
            (paged.total, paged.flats.len()),
            (3, 1),
            "the middle of three"
        );
        assert_eq!(paged.flats.first().expect("one row").name, "PNGFLAT");
    }

    /// The candidate flags of the fixture's patch listings.
    fn candidates() -> BTreeSet<String> {
        let session = fixture();
        let pnames = pnames_of(&session).expect("PNAMES parses");
        rows(&session)
            .iter()
            .filter(|row| is_patch_candidate(row, &pnames))
            .map(|row| row.name.to_string())
            .collect()
    }

    #[test]
    fn patch_candidates_are_the_region_plus_pnames_named_globals() {
        let listed = candidates();
        assert!(
            listed.contains("WALLP1") && listed.contains("JUNKP1"),
            "every patches-region lump is listed"
        );
        assert!(listed.contains("PNGPAT"), "PNAMES pulls in the global");
        assert!(
            !listed.contains("TROOA1"),
            "a sprites lump named by PNAMES stays out"
        );
        assert!(!listed.contains("FLOOR1"), "flats stay out");
        assert!(
            !listed.iter().any(|text| text.ends_with("_START")),
            "markers are never listed"
        );
    }

    #[test]
    fn patches_report_formats_offsets_and_the_in_pnames_flag() {
        let session = fixture();
        let report = list_patches_run(&session, &page(0, 10), None, false).expect("runs");
        assert_eq!(report.total, 3, "two region patches plus the global PNG");
        let wall = &report.patches[0];
        assert_eq!(wall.format, "doom", "the header reads");
        assert_eq!(
            (wall.width, wall.height, wall.left_offset, wall.top_offset),
            (Some(16_u32), Some(16_u32), Some(-2_i32), Some(-14_i32)),
            "header fields"
        );
        assert_eq!(wall.namespace, Some("patches"), "in the region");
        assert!(wall.in_pnames, "WALLP1 is named");
        let junk = &report.patches[1];
        assert_eq!(junk.format, "unknown", "40 x's are neither format");
        assert_eq!(
            (junk.width, junk.height, junk.left_offset, junk.top_offset),
            (None, None, None, None),
            "nothing measured"
        );
        assert!(!junk.in_pnames, "JUNKP1 is not in PNAMES");
        assert_eq!(junk.namespace, Some("patches"), "still a region lump");
        let pngpat = &report.patches[2];
        assert_eq!(pngpat.format, "png", "the global PNG");
        assert_eq!(
            (pngpat.width, pngpat.height),
            (Some(4), Some(2)),
            "from the IHDR"
        );
        assert_eq!(
            (pngpat.left_offset, pngpat.top_offset),
            (Some(3_i32), Some(-4_i32)),
            "from grAb"
        );
        assert_eq!(pngpat.namespace, None, "outside any patches region");
        let only_named = list_patches_run(&session, &page(0, 10), None, true).expect("runs");
        assert_eq!(only_named.total, 2, "WALLP1 and PNGPAT");
        assert_eq!(
            list_patches_run(&session, &page(0, 10), Some("*P1"), false)
                .expect("runs")
                .total,
            2,
            "the glob keeps the region pair"
        );
    }

    #[test]
    fn a_malformed_pnames_surfaces_as_a_format_error() {
        let mut session = fixture();
        session
            .edit("junk PNAMES", |wad| {
                wad.lumps.push(Lump::new(name("PNAMES"), vec![0_u8; 3]));
                Ok(())
            })
            .expect("plain append");
        let error =
            list_patches_run(&session, &page(0, 10), None, false).expect_err("no names fit");
        assert_eq!(code(&error), "format_error", "malformed PNAMES code");
    }

    #[test]
    fn sprites_group_frames_rotations_and_mirrors() {
        let session = fixture();
        let report = list_sprites_run(&session, &page(0, 10), None).expect("runs");
        assert_eq!(report.total, 2, "PLAY and TROO");
        assert_eq!(report.invalid, vec!["BADNAME".to_owned()], "the odd name");
        let troo = report.sprites.get(1).expect("TROO sorts after PLAY");
        assert_eq!(troo.frames.len(), 1, "frame A only");
        let rotations: Vec<(u8, &str, bool)> = troo.frames[0]
            .rotations
            .iter()
            .map(|row| (row.rotation, row.lump.as_str(), row.mirrored))
            .collect();
        assert_eq!(
            rotations,
            vec![
                (1, "TROOA1", false),
                (2, "TROOA2A8", false),
                (8, "TROOA2A8", true),
            ],
            "by rotation, the mirror entry flagged"
        );
        let play = report.sprites.first().expect("PLAY");
        assert_eq!(play.frames[0].rotations[0].rotation, 0, "the all-angles");
        let filtered = list_sprites_run(&session, &page(0, 10), Some("TROO")).expect("runs");
        assert_eq!(filtered.total, 1, "one group survives the prefix");
        assert!(filtered.invalid.is_empty(), "BADNAME is not a TROO");
        assert_eq!(
            list_sprites_run(&session, &page(0, 10), Some("BADN"))
                .expect("runs")
                .invalid,
            vec!["BADNAME".to_owned()],
            "the prefix keeps its invalid names"
        );
        assert_eq!(
            list_sprites_run(&session, &page(1, 1), None)
                .expect("runs")
                .sprites
                .first()
                .expect("the second group")
                .sprite,
            "TROO",
            "paging skips PLAY"
        );
        let past = list_sprites_run(&session, &page(2, 1), None).expect("runs");
        assert_eq!((past.total, past.sprites.len()), (2, 0), "past the end");
    }

    #[test]
    fn prefixes_are_uppercased_and_validated() {
        assert_eq!(
            parse_prefix(Some("poss")).expect("four letters"),
            Some("POSS".to_owned()),
            "case is normalised"
        );
        assert_eq!(parse_prefix(None).expect("absent"), None, "no filter");
        for bad in ["", "POSSS", "PO!S", "P1"] {
            let error = parse_prefix(Some(bad)).expect_err("not four letters or digits");
            assert_eq!(code(&error), "invalid_argument", "prefix {bad:?} code");
        }
    }

    #[test]
    fn empty_sessions_list_nothing() {
        let session = Session::default();
        let flats = list_flats_run(&session, &page(0, 10), None).expect("runs");
        assert_eq!(flats.total, 0, "no flats");
        let patches = list_patches_run(&session, &page(0, 10), None, false).expect("runs");
        assert_eq!(patches.total, 0, "no patches and no PNAMES is fine");
        let sprites = list_sprites_run(&session, &page(0, 10), None).expect("runs");
        assert!(
            sprites.sprites.is_empty() && sprites.invalid.is_empty() && sprites.total == 0,
            "empty lists are not an error"
        );
    }

    #[test]
    fn a_working_override_replaces_the_layer_row_of_the_same_name() {
        let mut session = fixture();
        session
            .edit("override WALLP1", |wad| {
                wad.lumps
                    .push(Lump::new(name("WALLP1"), header(8, 8, 0, 0)));
                Ok(())
            })
            .expect("plain append");
        let report = list_patches_run(&session, &page(0, 10), Some("WALLP1"), false).expect("runs");
        assert_eq!(report.total, 1, "one row per name");
        let wall = report.patches.first().expect("the winner");
        assert_eq!(wall.width, Some(8), "the working copy's header");
        assert_eq!(wall.source, Source::Working, "and its origin");
    }

    #[test]
    fn sprite_helpers_group_only_grammar_matching_names() {
        let pnames: BTreeSet<LumpName> = BTreeSet::new();
        let directory = rows(&fixture());
        let troo = row_named(&directory, "TROOA2A8");
        assert_eq!(
            patch_row(troo, &pnames).format,
            "doom",
            "a sprite is a patch"
        );
        let bad = row_named(&directory, "BADNAME");
        assert_eq!(patch_row(bad, &pnames).format, "unknown", "five bytes");
        assert!(parse_sprite_name("BADNAME").is_none(), "not a sprite name");
        let parsed = parse_sprite_name("TROOA2A8").expect("this one is");
        let mut groups = Groups::new();
        collect_sprite(&mut groups, &parsed, "TROOA2A8", Some("PLAY"));
        assert!(groups.is_empty(), "the prefix filter drops it");
        collect_sprite(&mut groups, &parsed, "TROOA2A8", None);
        let sprites = sprite_rows(groups);
        let frames = sprites
            .first()
            .expect("one sprite")
            .frames
            .first()
            .expect("one frame");
        assert_eq!(frames.rotations.len(), 2, "direct and mirrored entries");
        assert!(frames.rotations[1].mirrored, "the second one is mirrored");
    }

    /// The shared picture options of a test call.
    fn params(palette: Option<usize>, format: Option<&str>, scale: Option<u32>) -> ImageParams {
        ImageParams {
            palette,
            format: format.map(str::to_owned),
            scale,
        }
    }

    #[test]
    fn picture_options_default_and_reject_a_bad_format_or_scale() {
        let plain = parse_options(&params(None, None, None)).expect("defaults are valid");
        assert_eq!(
            (plain.palette, plain.format, plain.scale),
            (0_usize, AssetFormat::Png, 1_u32),
            "the documented defaults"
        );
        let picked = parse_options(&params(Some(13), Some("rgba"), Some(8))).expect("in range");
        assert_eq!(
            (picked.palette, picked.format, picked.scale),
            (13_usize, AssetFormat::Rgba, 8_u32),
            "everything as asked"
        );
        let odd_format =
            parse_options(&params(None, Some("jpg"), None)).expect_err("no such encoding");
        assert_eq!(code(&odd_format), "invalid_argument", "bad format code");
        for factor in [0_u32, 9_u32] {
            let odd = parse_options(&params(None, None, Some(factor))).expect_err("out of range");
            assert_eq!(code(&odd), "invalid_argument", "scale {factor} code");
        }
        assert_eq!(
            (AssetFormat::Png.extension(), AssetFormat::Rgba.extension()),
            (("png", "image/png"), ("rgba", "application/octet-stream")),
            "the published extensions and media types"
        );
    }

    #[test]
    fn frames_rotations_and_prefixes_are_normalised_and_checked() {
        assert_eq!(parse_frame(None).expect("the default"), 'A', "frame A");
        for (text, want) in [("b", 'B'), ("[", '['), ("\\", '\\'), ("]", ']')] {
            assert_eq!(
                parse_frame(Some(text)).expect("a frame letter"),
                want,
                "{text:?} is a frame"
            );
        }
        for bad in ["", "AB", "1", "-"] {
            let error = parse_frame(Some(bad)).expect_err("not a frame letter");
            assert_eq!(code(&error), "invalid_argument", "frame {bad:?} code");
        }
        assert_eq!(parse_rotation(None).expect("the default"), 1, "rotation 1");
        for allowed in [0_u8, 4, 8] {
            assert_eq!(
                parse_rotation(Some(allowed)).expect("in range"),
                allowed,
                "rotation {allowed}"
            );
        }
        let high = parse_rotation(Some(9)).expect_err("past eight");
        assert_eq!(code(&high), "invalid_argument", "rotation 9 code");
        assert_eq!(
            parse_sprite("poss").expect("four letters"),
            "POSS",
            "the prefix is uppercased"
        );
        assert_eq!(
            parse_prefix(Some("troo")).expect("four letters"),
            Some("TROO".to_owned()),
            "the listing shares the check"
        );
        for bad in ["", "POS", "PO!S", "TROO1"] {
            let error = parse_sprite(bad).expect_err("not four letters or digits");
            assert_eq!(code(&error), "invalid_argument", "prefix {bad:?} code");
        }
    }

    /// The `get_sprite` arguments naming one lump, one sprite, or both.
    fn sprite_params(name: Option<&str>, sprite: Option<&str>) -> GetSpriteParams {
        GetSpriteParams {
            name: name.map(str::to_owned),
            sprite: sprite.map(str::to_owned),
            frame: None,
            rotation: None,
            common: ImageParams::default(),
        }
    }

    #[test]
    fn a_sprite_call_names_one_lump_as_stored_or_one_rotation() {
        let stored = sprite_request(&sprite_params(Some("trooa2a8"), None)).expect("a name");
        assert!(
            matches!(stored, SpriteRequest::Lump(text) if text == name("TROOA2A8")),
            "the whole lump name is served as stored"
        );
        let parts = sprite_request(&sprite_params(None, Some("troo"))).expect("a sprite");
        let SpriteRequest::Rotation {
            name: wanted,
            sprite,
            frame,
            rotation,
        } = parts
        else {
            panic!("a sprite/frame/rotation request was asked for: {parts:?}");
        };
        assert_eq!(wanted, name("TROOA1"), "the defaults name the lump");
        assert_eq!(sprite, "TROO", "uppercased prefix");
        assert_eq!((frame, rotation), ('A', 1), "the documented defaults");
        let both = sprite_request(&sprite_params(Some("TROOA1"), Some("TROO")))
            .expect_err("two selectors");
        assert_eq!(code(&both), "invalid_argument", "both selectors code");
        let neither = sprite_request(&sprite_params(None, None)).expect_err("no selector");
        assert_eq!(code(&neither), "invalid_argument", "no selector code");
    }

    /// A one-pixel render with optional drawing offsets.
    fn rendered(offsets: Option<(i32, i32)>) -> Rendered {
        let image = RgbaImage::new(1, 1, vec![0_u8; 4]).expect("1x1 is a valid shape");
        match offsets {
            Some((left, top)) => Rendered {
                image,
                left_offset: Some(left),
                top_offset: Some(top),
            },
            None => Rendered {
                image,
                left_offset: None,
                top_offset: None,
            },
        }
    }

    #[test]
    fn offsets_scale_with_the_picture_and_only_a_grab_writes_grab() {
        assert_eq!(scale_offset(-2, 4), -8_i32, "the published offset");
        assert_eq!(scale_offset(8, 1), 8_i32, "an unscaled offset");
        assert_eq!(
            grab_of(&rendered(None)),
            None,
            "a flat carries no grAb chunk"
        );
        assert_eq!(
            grab_of(&rendered(Some((3_i32, -4_i32)))),
            Some((3_i32, -4_i32)),
            "both offsets or neither"
        );
    }

    #[test]
    fn a_prepared_picture_reports_the_sprite_extras_only_for_a_sprite() {
        let flat = Prepared::plain(name("FLOOR1"), rendered(None));
        assert!(
            flat.lump.is_none() && flat.mirrored.is_none(),
            "a flat answers with its own name"
        );
        let sprite = Prepared::served(name("TROOA8"), name("TROOA2A8"), true, rendered(None));
        assert_eq!(
            sprite.name,
            name("TROOA8"),
            "the rotation that was asked for"
        );
        assert_eq!(
            sprite.lump,
            Some(name("TROOA2A8")),
            "the lump that served it"
        );
        assert_eq!(sprite.mirrored, Some(true), "and the flip it needed");
    }

    #[test]
    fn a_fetched_file_is_named_from_its_stem() {
        assert_eq!(display_name("WALLP1.png"), "WALLP1", "a lump-name stem");
        assert_eq!(
            display_name("my-drawing.png"),
            "MYDRAWIN",
            "filtered and trimmed to eight characters"
        );
        assert_eq!(display_name("notes"), "NOTES", "no extension to strip");
        assert_eq!(
            display_name("?!"),
            "?!".to_owned(),
            "no lump name in it: the filename"
        );
        assert_eq!(display_name(""), "PATCH", "a source naming no file at all");
    }

    /// The `get_thing_sprite` arguments for one type and selection.
    fn thing_params(kind: u32, frame: Option<&str>, rotation: Option<u8>) -> GetThingSpriteParams {
        GetThingSpriteParams {
            r#type: kind,
            frame: frame.map(str::to_owned),
            rotation,
            common: ImageParams::default(),
        }
    }

    /// A direct `ThingRequest` for the session-side helpers.
    fn thing_selection(kind: u32, frame: Option<char>, rotation: u8) -> ThingRequest {
        ThingRequest {
            kind,
            frame,
            rotation,
        }
    }

    #[test]
    fn thing_sprite_arguments_default_and_refuse_odd_selections() {
        let plain = thing_request(&thing_params(3001, None, None)).expect("a type alone");
        assert_eq!(
            (plain.kind, plain.frame, plain.rotation),
            (3001_u32, None, 1_u8),
            "the definition's frame and rotation 1"
        );
        let picked = thing_request(&thing_params(3001, Some("b"), Some(8))).expect("given parts");
        assert_eq!(
            (picked.frame, picked.rotation),
            (Some('B'), 8_u8),
            "the frame is uppercased"
        );
        let odd_frame = thing_request(&thing_params(3001, Some("1"), None)).expect_err("no frame");
        assert_eq!(code(&odd_frame), "invalid_argument", "frame code");
        assert!(
            thing_request(&thing_params(3001, None, Some(0)))
                .expect_err("rotation 0 is get_sprite's")
                .to_string()
                .contains("rotation"),
            "the message names the parameter"
        );
        for rotation in [0_u8, 9_u8] {
            let error = parse_thing_rotation(Some(rotation)).expect_err("outside the eight views");
            assert_eq!(code(&error), "invalid_argument", "rotation {rotation} code");
        }
        assert_eq!(
            parse_thing_rotation(None).expect("the default"),
            1,
            "rotation 1"
        );
    }

    #[test]
    fn thing_definitions_follow_the_session_game_and_custom_sources() {
        assert_eq!(
            session_game(&Session::default()),
            Game::Doom2,
            "an unidentified session stands for DOOM II"
        );
        let session = fixture();
        let imp = thing_sprite_def(&session, 3001).expect("the built-in Imp");
        assert_eq!(
            (imp.sprite.as_str(), imp.frame, imp.name.as_str()),
            ("TROO", 'A', "Imp"),
            "the table row"
        );
        let unknown = thing_sprite_def(&session, 999_999).expect_err("no such type");
        assert_eq!(code(&unknown), "not_found", "unknown type code");
        assert!(
            unknown.to_string().contains("no thing type 999999"),
            "the message names the type: {unknown}"
        );
        let mut grown = fixture();
        grown
            .edit("decorate", |wad| {
                wad.lumps.push(Lump::new(
                    name("DECORATE"),
                    b"actor LampSprite 20010 {\n States {\n Spawn: PLSM A 10\n }\n}\n\
                      actor LampPlain 20014 { radius 10 }\n"
                        .to_vec(),
                ));
                Ok(())
            })
            .expect("plain append");
        let custom = thing_sprite_def(&grown, 20_010).expect("the DECORATE type");
        assert_eq!(
            (custom.sprite.as_str(), custom.frame, custom.source.as_str()),
            ("PLSM", 'A', "decorate"),
            "the session-aware overlay reaches the tool"
        );
        let no_sprite = thing_sprite_def(&grown, 20_014).expect_err("no sprite column");
        assert_eq!(code(&no_sprite), "not_found", "no-sprite code");
        assert!(
            no_sprite
                .to_string()
                .contains("thing type 20014 has no sprite"),
            "the message names the type: {no_sprite}"
        );
    }

    #[test]
    fn thing_sprite_prep_reports_definition_lump_and_picture_errors() {
        let session = fixture();
        let options = parse_options(&ImageParams::default()).expect("the defaults");
        let unknown = prepare_thing_sprite(&session, &thing_selection(999_999, None, 1), &options)
            .err()
            .expect("no such type");
        assert_eq!(code(&unknown), "not_found", "unknown type code");
        let no_lumps = prepare_thing_sprite(&session, &thing_selection(3004, None, 1), &options)
            .err()
            .expect("the fixture has no POSS lumps");
        assert_eq!(code(&no_lumps), "not_found", "missing lumps code");
        assert!(
            no_lumps.to_string().contains("POSSA1 / POSSA0"),
            "the message names the lump pattern: {no_lumps}"
        );
        let broken = prepare_thing_sprite(&session, &thing_selection(3001, None, 1), &options)
            .err()
            .expect("the fixture's TROOA1 is a bare header");
        assert_eq!(
            code(&broken),
            "format_error",
            "an undecodable sprite stays a format error"
        );
    }

    #[test]
    fn a_missing_thing_sprite_names_the_expected_lumps() {
        let error = missing_thing_sprite("TROO", 'A', 1);
        assert_eq!(code(&error), "not_found", "the helper's code");
        let message = error.to_string();
        assert!(
            message.contains("TROOA1 / TROOA0") && message.contains("list_sprites"),
            "the message names the lumps and the listing: {message}"
        );
    }
}
