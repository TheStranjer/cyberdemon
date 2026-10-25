//! `list_flats` and `list_patches`: the two directory listings of picture
//! lumps, names and measurements only.
//!
//! Both read the merged view's rows through [`merged_rows`], keep one row
//! per name with [`last_wins`], filter with [`passes`] and cut the page with
//! [`page_rows`]. [`flat_row`] measures a flat as its `PNG` `IHDR` shape or
//! the shape its raw byte count implies; [`patch_row`] sniffs `PNG` first
//! and the `Doom` header second, so [`png_patch_shape`] and
//! [`doom_patch_shape`] split those two branches and [`shape_pair`] and
//! [`offset_pair`] turn a measurement into the optional row fields.
//! [`is_patch_candidate`] is the region-plus-`PNAMES` rule the patch listing
//! filters with, over the names [`pnames_of`] reads.

use alloc::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::codec::{is_png, png_dimensions};
use crate::gfx::flat::flat_shape;
use crate::gfx::grab::read_grab;
use crate::gfx::patch::{PatchHeader, patch_header};
use crate::session::{Session, Source};
use crate::tools::graphics::{Page, in_namespace, last_wins, merged_rows, page_rows, passes};
use crate::tools::lumps::rows::LumpRow;
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::lumps;
use crate::wad::name::LumpName;
use crate::wad::textures::parse_pnames;

/// Arguments for [`Cyberdemon::list_flats`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListFlatsParams {
    /// Case-insensitive flat-name glob: `*` matches any run of
    /// characters, `?` exactly one, e.g. `"LAVA*"`.
    #[serde(default)]
    pub(super) pattern: Option<String>,
    /// Matching flats to skip before the page (default 0).
    #[serde(default)]
    pub(super) offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

/// Arguments for [`Cyberdemon::list_patches`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListPatchesParams {
    /// Case-insensitive patch-name glob: `*` matches any run of
    /// characters, `?` exactly one, e.g. `"WALL*"`.
    #[serde(default)]
    pub(super) pattern: Option<String>,
    /// Matching patches to skip before the page (default 0).
    #[serde(default)]
    pub(super) offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    pub(super) limit: Option<usize>,
    /// Only report patches `PNAMES` names (default `false`).
    #[serde(default)]
    pub(super) in_pnames_only: Option<bool>,
}

/// One `list_flats` row.
#[derive(Debug, Serialize)]
pub(super) struct FlatRow {
    /// The flat's lump name.
    pub(super) name: String,
    /// Payload size in bytes.
    pub(super) size: usize,
    /// Width in pixels, `null` for a raw flat of an unsupported size or
    /// an unreadable `PNG` header.
    pub(super) width: Option<u32>,
    /// Height in pixels, `null` like `width`.
    pub(super) height: Option<u32>,
    /// `"png"` when the bytes start with the `PNG` signature, `"raw"`
    /// otherwise.
    pub(super) format: &'static str,
    /// The archive the winning lump came from.
    pub(super) source: Source,
}

/// Structured result of [`Cyberdemon::list_flats`].
#[derive(Debug, Serialize)]
pub(super) struct FlatsReport {
    /// Flats matching `pattern` (before paging).
    pub(super) total: usize,
    /// The matching flats, in directory order, on this page.
    pub(super) flats: Vec<FlatRow>,
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
pub(super) fn list_flats_run(
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
pub(super) struct PatchRow {
    /// The patch's lump name.
    pub(super) name: String,
    /// Payload size in bytes.
    pub(super) size: usize,
    /// Width in pixels, `null` for an unknown format or an unreadable
    /// `PNG` header.
    pub(super) width: Option<u32>,
    /// Height in pixels, `null` like `width`.
    pub(super) height: Option<u32>,
    /// Horizontal drawing offset, `null` when the format stores none.
    pub(super) left_offset: Option<i32>,
    /// Vertical drawing offset, `null` like `left_offset`.
    pub(super) top_offset: Option<i32>,
    /// `"doom"` for a readable `Doom` patch header, `"png"` for `PNG`
    /// bytes, `"unknown"` for anything else.
    pub(super) format: &'static str,
    /// Whether the merged `PNAMES` lists this name.
    pub(super) in_pnames: bool,
    /// `"patches"` inside a `P_START` region, `null` elsewhere.
    pub(super) namespace: Option<&'static str>,
    /// The archive the winning lump came from.
    pub(super) source: Source,
}

/// Structured result of [`Cyberdemon::list_patches`].
#[derive(Debug, Serialize)]
pub(super) struct PatchesReport {
    /// Patches matching the filters (before paging).
    pub(super) total: usize,
    /// The matching patches, in directory order, on this page.
    pub(super) patches: Vec<PatchRow>,
}

/// The merged view's `PNAMES` names as lump names; no `PNAMES` is the
/// empty set, and an entry that is not a valid lump name never matches.
///
/// # Errors
///
/// [`parse_pnames`]' [`CyberError::Format`] for a malformed lump.
pub(super) fn pnames_of(session: &Session) -> Result<BTreeSet<LumpName>, CyberError> {
    let Some(entry) = session.find_global(lumps::PNAMES) else {
        return Ok(BTreeSet::new());
    };
    Ok(parse_pnames(&entry.lump.data)?
        .into_iter()
        .filter_map(|text| LumpName::parse(&text).ok())
        .collect())
}

/// Whether `row` is listed as a patch: inside a `P_START` region, or
/// named by `PNAMES` from outside the flats and sprites regions.
pub(super) fn is_patch_candidate(row: &LumpRow, pnames: &BTreeSet<LumpName>) -> bool {
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
pub(super) fn patch_row(row: &LumpRow, pnames: &BTreeSet<LumpName>) -> PatchRow {
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
pub(super) fn list_patches_run(
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
