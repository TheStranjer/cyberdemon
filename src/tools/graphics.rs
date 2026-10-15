//! Graphics browsing tools: the flats, patch and sprite listings of the
//! merged view.
//!
//! Every tool reads [`crate::session::Scope::Merged`] through the shared
//! rows in [`crate::tools::lumps::rows`]: the lumps of one namespace, last
//! lump with a name wins. `list_flats` describes the floor and ceiling
//! planes of the `F_START` region, `list_patches` the texture components
//! of the `P_START` region (plus any lump `PNAMES` names from outside the
//! flats and sprites regions), and `list_sprites` groups the `S_START`
//! region by the sprite name grammar of
//! [`crate::gfx::sprite_name::parse_sprite_name`]. Nothing here decodes
//! pixels or edits lumps: sizes, dimensions, offsets and names only.

use alloc::collections::{BTreeMap, BTreeSet};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::codec::{is_png, png_dimensions};
use crate::gfx::flat::flat_shape;
use crate::gfx::grab::read_grab;
use crate::gfx::patch::{PatchHeader, patch_header};
use crate::gfx::sprite_name::{SpriteName, parse_sprite_name};
use crate::server::Cyberdemon;
use crate::session::{Scope, Session, Source};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::{fail, ok};
use crate::wad::classify::{LumpKind, Namespace};
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
    let Some(raw) = text else {
        return Ok(None);
    };
    let upper = raw.to_ascii_uppercase();
    let well_formed = upper.len() == SPRITE_PREFIX_LEN
        && upper
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte.is_ascii_digit());
    if well_formed {
        return Ok(Some(upper));
    }
    Err(CyberError::InvalidArgument(format!(
        "prefix {raw:?} is not four ASCII letters or digits: pass the sprite prefix, e.g. \
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

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = graphics_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists the flat textures of the session.
    #[tool(
        description = "Lists the flat (wall/floor/ceiling) lumps of the merged view's flats regions (between F_START/F_END or FF_START/FF_END; markers excluded, and when a name repeats the last lump wins): {name, size, width, height, format, source}. format is `\"png\"` when the bytes carry a PNG signature - then width/height come from the IHDR - and `\"raw\"` otherwise, where the vanilla shapes answer: 4096 bytes is 64x64, 8192 is 64x128, 16384 is 128x128, 65536 is 256x256; an unsupported size answers width/height null. source is list_lumps' archive origin (\"working\" or {\"layer\": N}). A session without flats answers {total: 0, flats: []}, not an error. Optional pattern is a case-insensitive name glob (* and ?); paging is offset (default 0) and limit (default 200, max 2000). Returns {total, flats} where total counts pattern matches before paging. Patch lumps (texture components) are listed by list_patches and sprite graphics by list_sprites; read a flat's bytes with get_lump or extract_lump."
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
        description = "Lists the patch lumps of the merged view: every lump in a patches region (between P_START/P_END or PP_START/PP_END; markers excluded, last lump with a name wins) plus every lump PNAMES names from outside the flats and sprites regions. Rows are {name, size, width, height, left_offset, top_offset, format, in_pnames, namespace, source}: format is `\"doom\"` when the first eight bytes read as a Doom patch header (dimensions and drawing offsets from it; the column bodies are never touched), `\"png\"` for a PNG lump (dimensions from the IHDR, offsets from a grAb chunk when present) and `\"unknown\"` otherwise, where every measurement is null. in_pnames says whether PNAMES lists the name; namespace is `\"patches\"` inside a patches region and null elsewhere; source is list_lumps' archive origin. A session without patches answers {total: 0, patches: []}, not an error; a malformed PNAMES fails with format_error. Optional pattern is a case-insensitive name glob (* and ?), in_pnames_only (default false) restricts the list to PNAMES entries, and paging is offset (default 0) and limit (default 200, max 2000). Returns {total, patches} where total counts matches before paging. See list_textures and list_pnames for the textures these patches compose, list_flats and list_sprites for the other graphic lumps."
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
        description = "Groups the merged view's sprite-region lumps (between S_START/S_END or SS_START/SS_END; markers excluded, last lump with a name wins) by the Doom sprite name grammar: four characters of sprite name, a frame letter (A-Z, then [, \\, ]), a rotation digit 0-8 - and optionally a second frame+rotation served by the same image horizontally mirrored, e.g. POSSA2A8 serves rotation 2 directly and rotation 8 flipped; rotation 0 serves every angle with the one image. Answers {total, sprites, invalid}: sprites is [{sprite, frames: [{frame, rotations: [{rotation, lump, mirrored}]}]}] grouped by prefix, frames in letter order, rotations by number then lump name; invalid lists the region lumps whose names match no grammar, in directory order. total counts sprite groups before paging (the prefix filter applied); paging is offset (default 0) and limit (default 200, max 2000) over groups. Optional prefix keeps one four-character sprite (case-insensitive letters or digits) - any other text fails with invalid_argument. A session without sprites answers {total: 0, sprites: [], invalid: []}, not an error. Flats are listed by list_flats, patches by list_patches."
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
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use std::path::PathBuf;

    use super::{
        Groups, Page, collect_sprite, in_namespace, is_patch_candidate, last_wins, list_flats_run,
        list_patches_run, list_sprites_run, parse_page, parse_prefix, patch_row, pnames_of,
        sprite_rows,
    };
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::sprite_name::parse_sprite_name;
    use crate::session::{Layer, LayerRole, Scope, Session, Source};
    use crate::tools::lumps::rows::{LumpRow, build_rows};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
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
}
