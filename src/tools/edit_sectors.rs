//! Sector editing tools: `add_sector`, `edit_sector`, `remove_sector`,
//! `create_sector_from_loop` and `carve_void` (see the `edit_sectors`
//! router methods on
//! [`crate::server::Cyberdemon`]).
//!
//! Every call is exactly one [`crate::session::Session::edit_map`] step, so
//! one undo entry moves the whole call. The parameter shapes
//! ([`IndexSelector`]) and validators ([`check_index`],
//! [`validate_texture_name`]) come from [`crate::tools::maps::params`];
//! removal runs through [`crate::wad::map::remove`] and refuses the
//! `conflict` of a sector that sidedefs still use unless `cascade` is set.
//! `create_sector_from_loop` and `carve_void` wrap the pure overlay
//! engine [`crate::wad::map::overlay::apply_overlay`]: this module turns
//! the wire parameters into a validated
//! [`crate::wad::map::overlay::OverlayRequest`]
//! (sector fields checked like [`crate::session::Session::edit_map`] hands
//! in, textures and coordinates checked for the map's format, the outer
//! rule parsed) and reports the engine's
//! [`crate::wad::map::overlay::OverlayResult`]
//! with the standard edit fragment.
//!
//! The geometry rule: the sector count is part of the geometry signature,
//! so appending or removing sectors reports `nodes_invalidated: true` (the
//! generated lumps are emptied and the `GL` run dropped), while property
//! edits of existing sectors (heights, flats, light, special, tag) keep the
//! nodes. The binary formats store the heights and the light as `i16` and
//! `special`/`tag` as `u16`, so values outside those ranges are refused as
//! `invalid_argument` before anything is written; `UDMF` stores full
//! integers and has no such caps. The light level stays inside the vanilla
//! `0..=255` in every format. A flat may not be `"-"`: that names no
//! texture, not a floor or ceiling surface.

use alloc::collections::{BTreeMap, BTreeSet};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::geom::Pt;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::themes::Role;
use crate::tools::maps::common;
use crate::tools::maps::params::{
    IndexSelector, check_index, edit_result_fragment, validate_texture_name,
};
use crate::tools::themes::theme_texture;
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::overlay::{self, Outside, OverlayRequest, OverlayResult, Paint, WallTextures};
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, dependents, expand_cascade,
};
use crate::wad::map::{Map, Sector};

/// The `add_sector` default floor height, in map units.
const DEFAULT_FLOOR_HEIGHT: i32 = 0_i32;
/// The `add_sector` default ceiling height, in map units.
const DEFAULT_CEILING_HEIGHT: i32 = 128_i32;
/// The `add_sector` default floor flat name.
const DEFAULT_FLOOR_FLAT: &str = "FLOOR4_8";
/// The `add_sector` default ceiling flat name.
const DEFAULT_CEILING_FLAT: &str = "CEIL3_5";
/// The `add_sector` default light level.
const DEFAULT_LIGHT: i32 = 160_i32;
/// The texture name meaning "nothing here", never a valid flat.
const NO_TEXTURE: &str = "-";
/// The lowest light level the vanilla range allows.
const LIGHT_MIN: i32 = 0_i32;
/// The highest light level the vanilla range allows (`u8::MAX`).
const LIGHT_MAX: i32 = 255_i32;
/// The lowest height an `i16` binary `SECTORS` column holds (`i16::MIN`).
const HEIGHT_MIN: i32 = -0x8000_i32;
/// The highest height an `i16` binary `SECTORS` column holds (`i16::MAX`).
const HEIGHT_MAX: i32 = 0x7FFF_i32;
/// The lowest value a `u16` binary `SECTORS` column holds.
const FIELD_MIN: i32 = 0_i32;
/// The highest value a `u16` binary `SECTORS` column holds (`u16::MAX`).
const FIELD_MAX: i32 = 0xFFFF_i32;
/// The `create_sector_from_loop` default middle texture of a new wall.
const DEFAULT_WALL_TEXTURE: &str = "STARTAN3";
/// The `outer_sector` mode that probes the sector outside each new edge.
const OUTER_AUTO: &str = "auto";
/// The `outer_sector` mode that keeps every new wall one-sided.
const OUTER_NONE: &str = "none";
/// The lowest coordinate an `i16` binary `VERTEXES` column holds (`i16::MIN`).
const COORD_MIN: i32 = -0x8000_i32;
/// The highest coordinate an `i16` binary `VERTEXES` column holds (`i16::MAX`).
const COORD_MAX: i32 = 0x7FFF_i32;

/// Arguments for [`Cyberdemon::add_sector`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddSectorParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// Floor height in map units (default `0`).
    #[serde(default)]
    floor_height: Option<i32>,
    /// Ceiling height in map units (default `128`).
    #[serde(default)]
    ceiling_height: Option<i32>,
    /// Floor flat name (default `"FLOOR4_8"`).
    #[serde(default)]
    floor_flat: Option<String>,
    /// Ceiling flat name (default `"CEIL3_5"`).
    #[serde(default)]
    ceiling_flat: Option<String>,
    /// Light level, `0..=255` (default `160`).
    #[serde(default)]
    light: Option<i32>,
    /// Sector effect number (default `0`).
    #[serde(default)]
    special: Option<i32>,
    /// Tag matched by linedef actions (default `0`).
    #[serde(default)]
    tag: Option<i32>,
}

/// Arguments for [`Cyberdemon::edit_sector`].
#[derive(Debug, Deserialize, JsonSchema)]
struct EditSectorParams {
    /// The map to edit; defaults like [`AddSectorParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The sector to change: one index or a list of them (duplicates
    /// collapse); the shared changes below apply to every selected sector.
    index: IndexSelector,
    /// Replace the floor height in map units.
    #[serde(default)]
    floor_height: Option<i32>,
    /// Replace the ceiling height in map units.
    #[serde(default)]
    ceiling_height: Option<i32>,
    /// Raise or lower every selected sector's floor by this much.
    #[serde(default)]
    floor_delta: Option<i32>,
    /// Raise or lower every selected sector's ceiling by this much.
    #[serde(default)]
    ceiling_delta: Option<i32>,
    /// Replace the floor flat name.
    #[serde(default)]
    floor_flat: Option<String>,
    /// Replace the ceiling flat name.
    #[serde(default)]
    ceiling_flat: Option<String>,
    /// Replace the light level; it must stay in `0..=255`.
    #[serde(default)]
    light: Option<i32>,
    /// Replace the sector effect number.
    #[serde(default)]
    special: Option<i32>,
    /// Replace the tag.
    #[serde(default)]
    tag: Option<i32>,
}

/// Arguments for [`Cyberdemon::remove_sector`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveSectorParams {
    /// The map to edit; defaults like [`AddSectorParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The sector to remove: one index or a list of them.
    index: IndexSelector,
    /// Remove the sidedefs and geometry that use these sectors with them
    /// (default `false`: a referenced sector is refused with `conflict`).
    #[serde(default)]
    cascade: bool,
    /// Within a cascade, also remove sidedefs no surviving linedef
    /// references (default `true` when absent).
    #[serde(default)]
    prune_sidedefs: Option<bool>,
    /// Within a cascade, also remove vertices no surviving linedef
    /// references (default `true` when absent).
    #[serde(default)]
    prune_vertices: Option<bool>,
}

/// Arguments for [`Cyberdemon::create_sector_from_loop`].
#[derive(Debug, Deserialize, JsonSchema)]
struct CreateSectorFromLoopParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// The polygon's corners as points `{x, y}` (at least 3, implicitly
    /// closed): exactly one of `points` and `vertices`.
    #[serde(default)]
    points: Option<Vec<LoopPointParam>>,
    /// The polygon's corners as existing vertex indices (at least 3):
    /// exactly one of `points` and `vertices`.
    #[serde(default)]
    vertices: Option<Vec<usize>>,
    /// Floor height in map units (default `0`).
    #[serde(default)]
    floor_height: Option<i32>,
    /// Ceiling height in map units (default `128`).
    #[serde(default)]
    ceiling_height: Option<i32>,
    /// Floor flat name (default `"FLOOR4_8"`).
    #[serde(default)]
    floor_flat: Option<String>,
    /// Ceiling flat name (default `"CEIL3_5"`).
    #[serde(default)]
    ceiling_flat: Option<String>,
    /// Light level, `0..=255` (default `160`).
    #[serde(default)]
    light: Option<i32>,
    /// Sector effect number (default `0`).
    #[serde(default)]
    special: Option<i32>,
    /// Tag matched by linedef actions (default `0`).
    #[serde(default)]
    tag: Option<i32>,
    /// Middle texture of new one-sided walls (default `"STARTAN3"`).
    #[serde(default)]
    wall_texture: Option<String>,
    /// Upper texture of new two-sided transitions (default: `wall_texture`).
    #[serde(default)]
    upper_texture: Option<String>,
    /// Lower texture of new two-sided transitions (default: `wall_texture`).
    #[serde(default)]
    lower_texture: Option<String>,
    /// The sector across the new walls: an existing sector index, `"auto"`
    /// (default: the sector found just outside each edge on the original
    /// map), or `"none"` (new walls stay one-sided).
    #[serde(default)]
    outer_sector: Option<OuterSectorParam>,
    /// Keep existing sectors that sit strictly inside the polygon (a
    /// pillar, an inner room): only the area of the sectors the polygon's
    /// boundary touches is repainted (default `false`: everything inside
    /// is overwritten).
    #[serde(default)]
    keep_interior: bool,
}

/// Arguments for [`Cyberdemon::carve_void`].
#[derive(Debug, Deserialize, JsonSchema)]
struct CarveVoidParams {
    /// The map to edit; defaults like [`CreateSectorFromLoopParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The polygon's corners as points `{x, y}` (at least 3, implicitly
    /// closed): exactly one of `points` and `vertices`.
    #[serde(default)]
    points: Option<Vec<LoopPointParam>>,
    /// The polygon's corners as existing vertex indices (at least 3):
    /// exactly one of `points` and `vertices`.
    #[serde(default)]
    vertices: Option<Vec<usize>>,
}

/// One loop corner of [`Cyberdemon::create_sector_from_loop`].
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
struct LoopPointParam {
    /// `X` coordinate in map units.
    x: i32,
    /// `Y` coordinate in map units.
    y: i32,
}

/// The `outer_sector` parameter: an existing sector index or a mode name.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
enum OuterSectorParam {
    /// Face every new wall's outer side at this existing sector.
    Index(
        /// The sector index.
        usize,
    ),
    /// `"auto"` (the default) or `"none"`.
    Mode(
        /// The mode name.
        String,
    ),
}

/// One sector's fields as the tools report them.
#[derive(Debug, Serialize)]
struct SectorDetail {
    /// Floor height in map units.
    floor_height: i32,
    /// Ceiling height in map units.
    ceiling_height: i32,
    /// Floor flat name.
    floor_flat: String,
    /// Ceiling flat name.
    ceiling_flat: String,
    /// Light level, `0..=255`.
    light: i32,
    /// Sector effect number.
    special: i32,
    /// Tag matched by linedef actions.
    tag: i32,
}

/// One edited sector: its position plus its fields.
#[derive(Debug, Serialize)]
struct EditRow {
    /// Position in [`Map::sectors`].
    index: usize,
    /// The sector's fields, inlined next to the index.
    #[serde(flatten)]
    detail: SectorDetail,
}

/// Structured result of [`Cyberdemon::add_sector`].
#[derive(Serialize)]
struct AddReport {
    /// Index of the created sector.
    index: usize,
    /// The created sector's fields.
    sector: SectorDetail,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::edit_sector`].
#[derive(Serialize)]
struct EditReport {
    /// The edited sectors, ascending by index.
    sectors: Vec<EditRow>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::remove_sector`].
#[derive(Serialize)]
struct RemoveToolReport {
    /// What the removal engine dropped, flipped and normalised.
    removed: RemovalReport,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::create_sector_from_loop`].
#[derive(Serialize)]
struct LoopToolReport {
    /// What the overlay engine built, inlined next to the edit fragment.
    #[serde(flatten)]
    result: OverlayResult,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::carve_void`].
#[derive(Serialize)]
struct CarveToolReport {
    /// What the overlay engine built, inlined next to the fragment.
    #[serde(flatten)]
    result: OverlayResult,
    /// Indices of the sectors the carve left without a single sidedef
    /// (the same list `empty_sectors` reports; they stay in the map).
    removed_sectors_empty: Vec<usize>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The reported fields of `sector`.
fn detail_of(sector: &Sector) -> SectorDetail {
    SectorDetail {
        floor_height: sector.floor_height,
        ceiling_height: sector.ceiling_height,
        floor_flat: sector.floor_flat.clone(),
        ceiling_flat: sector.ceiling_flat.clone(),
        light: sector.light,
        special: sector.special,
        tag: sector.tag,
    }
}

/// A flat name validated for `format`; `"-"` names no texture and is
/// never a flat.
fn validate_flat(name: &str, format: MapFormat) -> Result<String, CyberError> {
    let checked = validate_texture_name(name, format)?;
    if checked == NO_TEXTURE {
        return Err(CyberError::InvalidArgument(
            "a flat cannot be \"-\": pass a flat lump name such as FLOOR4_8".to_owned(),
        ));
    }
    Ok(checked)
}

/// The flat the `add_sector` parameters ask for, `fallback` when absent.
fn flat_or_default(
    given: Option<&String>,
    fallback: &str,
    format: MapFormat,
) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(fallback.to_owned()),
        |name| validate_flat(name, format),
    )
}

/// The flat after an edit: the validated new name, or `current`.
fn edited_flat(
    current: &str,
    given: Option<&String>,
    format: MapFormat,
) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(current.to_owned()),
        |name| validate_flat(name, format),
    )
}

/// Refuses unless `value` sits inside the binary record field's
/// `min..=max` range.
fn fits_field(name: &str, value: i32, min: i32, max: i32) -> Result<(), CyberError> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{name} {value} is out of range for the binary map format: the field holds \
         {min}..={max}"
    )))
}

/// Range-checks `sector` against the binary `SECTORS` record: heights are
/// `i16` columns and `special`/`tag` are `u16` ones; a `UDMF` map stores
/// full integers and needs no check.
fn check_binary_ranges(format: MapFormat, sector: &Sector) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom | MapFormat::Hexen => {
            fits_field("floor_height", sector.floor_height, HEIGHT_MIN, HEIGHT_MAX)?;
            fits_field(
                "ceiling_height",
                sector.ceiling_height,
                HEIGHT_MIN,
                HEIGHT_MAX,
            )?;
            fits_field("special", sector.special, FIELD_MIN, FIELD_MAX)?;
            fits_field("tag", sector.tag, FIELD_MIN, FIELD_MAX)?;
        }
        MapFormat::Udmf => {}
    }
    Ok(())
}

/// Refuses a light level outside the vanilla `0..=255`, every format alike.
fn check_light(light: i32) -> Result<(), CyberError> {
    if (LIGHT_MIN..=LIGHT_MAX).contains(&light) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "light {light} is out of range: the light level holds {LIGHT_MIN}..={LIGHT_MAX}"
    )))
}

/// Refuses a sector whose ceiling ends below its floor; equal heights are
/// allowed, the closed shell of a raised door.
fn check_height_order(sector: &Sector) -> Result<(), CyberError> {
    if sector.ceiling_height >= sector.floor_height {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "ceiling_height {} is below floor_height {}: keep the ceiling at or above \
         the floor (equal is a closed door)",
        sector.ceiling_height, sector.floor_height
    )))
}

/// Everything one resulting sector must satisfy before anything is written.
fn check_sector(format: MapFormat, sector: &Sector) -> Result<(), CyberError> {
    check_light(sector.light)?;
    check_binary_ranges(format, sector)?;
    check_height_order(sector)
}

/// The height after the edit: `absolute` wins, `delta` shifts `current`,
/// neither keeps it.
fn new_height(
    name: &str,
    current: i32,
    absolute: Option<i32>,
    delta: Option<i32>,
) -> Result<i32, CyberError> {
    if let Some(value) = absolute {
        return Ok(value);
    }
    delta.map_or(Ok(current), |shift| {
        current.checked_add(shift).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "{name} {current} plus {shift} leaves the range of sector heights"
            ))
        })
    })
}

/// The sector the `add_sector` parameters describe for a map of `format`.
fn new_sector(args: &AddSectorParams, format: MapFormat) -> Result<Sector, CyberError> {
    Ok(Sector {
        floor_height: args.floor_height.unwrap_or(DEFAULT_FLOOR_HEIGHT),
        ceiling_height: args.ceiling_height.unwrap_or(DEFAULT_CEILING_HEIGHT),
        floor_flat: flat_or_default(args.floor_flat.as_ref(), DEFAULT_FLOOR_FLAT, format)?,
        ceiling_flat: flat_or_default(args.ceiling_flat.as_ref(), DEFAULT_CEILING_FLAT, format)?,
        light: args.light.unwrap_or(DEFAULT_LIGHT),
        special: args.special.unwrap_or_default(),
        tag: args.tag.unwrap_or_default(),
        extra: BTreeMap::new(),
    })
}

/// The sector `current` becomes under the fields `args` gives; unasked
/// fields and the `UDMF` extras stand.
fn edited_sector(
    current: &Sector,
    args: &EditSectorParams,
    format: MapFormat,
) -> Result<Sector, CyberError> {
    Ok(Sector {
        floor_height: new_height(
            "floor_height",
            current.floor_height,
            args.floor_height,
            args.floor_delta,
        )?,
        ceiling_height: new_height(
            "ceiling_height",
            current.ceiling_height,
            args.ceiling_height,
            args.ceiling_delta,
        )?,
        floor_flat: edited_flat(&current.floor_flat, args.floor_flat.as_ref(), format)?,
        ceiling_flat: edited_flat(&current.ceiling_flat, args.ceiling_flat.as_ref(), format)?,
        light: args.light.unwrap_or(current.light),
        special: args.special.unwrap_or(current.special),
        tag: args.tag.unwrap_or(current.tag),
        extra: current.extra.clone(),
    })
}

/// `add_sector`'s step: build, validate and append the described sector.
fn add_in_place(map: &mut Map, args: &AddSectorParams) -> Result<EditRow, CyberError> {
    let sector = new_sector(args, map.format)?;
    check_sector(map.format, &sector)?;
    let index = map.sectors.len();
    map.sectors.push(sector.clone());
    Ok(EditRow {
        index,
        detail: detail_of(&sector),
    })
}

/// Refuses an absolute/delta pair carried together on one axis.
fn check_conflicts(args: &EditSectorParams) -> Result<(), CyberError> {
    let moves = [
        (args.floor_height, args.floor_delta),
        (args.ceiling_height, args.ceiling_delta),
    ];
    if moves
        .into_iter()
        .any(|(absolute, delta)| absolute.is_some() && delta.is_some())
    {
        return Err(CyberError::InvalidArgument(
            "an absolute height conflicts with its delta: pass one of floor_height/\
             floor_delta or ceiling_height/ceiling_delta"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Whether the request changes at least one field.
#[must_use]
const fn has_change(args: &EditSectorParams) -> bool {
    args.floor_height.is_some()
        || args.ceiling_height.is_some()
        || args.floor_delta.is_some()
        || args.ceiling_delta.is_some()
        || args.floor_flat.is_some()
        || args.ceiling_flat.is_some()
        || args.light.is_some()
        || args.special.is_some()
        || args.tag.is_some()
}

/// Validates an `edit_sector` request up front and returns its sorted,
/// deduplicated target indices.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty index list, an
/// absolute/delta conflict or a request that changes nothing.
fn validate_edit(args: &EditSectorParams) -> Result<Vec<usize>, CyberError> {
    let indices = args.index.clone().indices()?;
    check_conflicts(args)?;
    if !has_change(args) {
        return Err(CyberError::InvalidArgument(
            "nothing to change: pass at least one of floor_height, ceiling_height, \
             floor_delta, ceiling_delta, floor_flat, ceiling_flat, light, special, tag"
                .to_owned(),
        ));
    }
    Ok(indices)
}

/// One planned sector change: the target index and the full result.
type PlannedEdit = (usize, Sector);

/// The validated `(index, new sector)` pairs of an `edit_sector` call;
/// nothing is stored until the whole selection computes.
fn planned_edits(
    map: &Map,
    args: &EditSectorParams,
    indices: &[usize],
) -> Result<Vec<PlannedEdit>, CyberError> {
    for index in indices {
        check_index("sector", *index, map.sectors.len())?;
    }
    let format = map.format;
    let mut planned = Vec::with_capacity(indices.len());
    for (index, slot) in map.sectors.iter().enumerate() {
        if !indices.contains(&index) {
            continue;
        }
        let updated = edited_sector(slot, args, format)?;
        check_sector(format, &updated)?;
        planned.push((index, updated));
    }
    Ok(planned)
}

/// `edit_sector`'s step: rewrite every selected sector and report the rows.
fn edit_in_place(
    map: &mut Map,
    args: &EditSectorParams,
    indices: &[usize],
) -> Result<Vec<EditRow>, CyberError> {
    let planned = planned_edits(map, args, indices)?;
    let mut rows = Vec::with_capacity(planned.len());
    for (index, updated) in planned {
        rows.push(EditRow {
            index,
            detail: detail_of(&updated),
        });
        if let Some(slot) = map.sectors.get_mut(index) {
            *slot = updated;
        }
    }
    Ok(rows)
}

/// The `conflict` naming every sidedef that still uses a selected sector.
fn dependent_refusal(uses: &BTreeMap<usize, Vec<usize>>) -> CyberError {
    let parts: Vec<String> = uses
        .iter()
        .map(|(sector, sides)| format!("sector {sector} is used by sidedef(s) {}", join(sides)))
        .collect();
    CyberError::Conflict(format!(
        "{}; pass cascade: true to remove the referencing sidedefs with them",
        parts.join("; ")
    ))
}

/// The indices as one comma-separated list.
fn join(indices: &[usize]) -> String {
    indices
        .iter()
        .map(usize::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

/// The seed removal of exactly the selected sectors.
fn sector_removal(indices: &[usize]) -> Removal {
    Removal {
        sectors: indices.iter().copied().collect::<BTreeSet<usize>>(),
        ..Default::default()
    }
}

/// The pruning flags of the `remove_sector` parameters; an absent flag
/// prunes, the cascading default.
fn cascade_options(args: &RemoveSectorParams) -> CascadeOptions {
    CascadeOptions {
        prune_sidedefs: args.prune_sidedefs.unwrap_or(true),
        prune_vertices: args.prune_vertices.unwrap_or(true),
    }
}

/// `remove_sector`'s step: check the indices, refuse a referenced sector
/// without `cascade`, then run the removal (grown by the cascade when set).
fn remove_in_place(
    map: &mut Map,
    args: &RemoveSectorParams,
    indices: &[usize],
) -> Result<RemovalReport, CyberError> {
    for index in indices {
        check_index("sector", *index, map.sectors.len())?;
    }
    let removal = sector_removal(indices);
    if !args.cascade {
        let uses = dependents(map, &removal).sidedefs_using_sectors;
        if !uses.is_empty() {
            return Err(dependent_refusal(&uses));
        }
        return apply_removal(map, &removal);
    }
    let grown = expand_cascade(map, &removal, cascade_options(args));
    apply_removal(map, &grown)
}

/// `add_sector`'s work: resolve the map, run the one-step append.
fn add_run(session: &mut Session, args: &AddSectorParams) -> Result<AddReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = "add sector".to_owned();
    let (row, report) = session.edit_map(name, &label, |map| add_in_place(map, args))?;
    Ok(AddReport {
        index: row.index,
        sector: row.detail,
        edit: edit_result_fragment(&report),
    })
}

/// `edit_sector`'s work: validate, resolve the map, run the one-step edit.
fn edit_run(session: &mut Session, args: &EditSectorParams) -> Result<EditReport, CyberError> {
    let indices = validate_edit(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("edit {} sectors", indices.len());
    let (rows, report) =
        session.edit_map(name, &label, |map| edit_in_place(map, args, &indices))?;
    Ok(EditReport {
        sectors: rows,
        edit: edit_result_fragment(&report),
    })
}

/// `remove_sector`'s work: resolve the map and run the removal.
fn remove_run(
    session: &mut Session,
    args: &RemoveSectorParams,
) -> Result<RemoveToolReport, CyberError> {
    let indices = args.index.clone().indices()?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("remove {} sectors", indices.len());
    let (removed, report) =
        session.edit_map(name, &label, |map| remove_in_place(map, args, &indices))?;
    Ok(RemoveToolReport {
        removed,
        edit: edit_result_fragment(&report),
    })
}

/// The refusal for a call giving the polygon in both forms or in neither.
fn loop_form_refusal() -> CyberError {
    CyberError::InvalidArgument(
        "the polygon needs its corners in exactly one form: points [{x, y}, ...] \
         (at least 3 corners, implicitly closed) or vertices [index, ...] (at least 3 \
         existing vertex indices)"
            .to_owned(),
    )
}

/// The polygon's corners the two optional parameters give, in exactly
/// one of the two forms: points checked against the map's format, or
/// existing vertex indices resolved to their coordinates.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when both `points` and `vertices` or
/// neither is passed, or when a point coordinate leaves the binary `i16`
/// range; [`CyberError::NotFound`] for a vertex index the map does not
/// hold.
fn polygon_of(
    points: Option<&Vec<LoopPointParam>>,
    vertices: Option<&Vec<usize>>,
    map: &Map,
) -> Result<Vec<Pt>, CyberError> {
    match (points, vertices) {
        (Some(list), None) => points_of(map.format, list),
        (None, Some(indices)) => vertices_of(map, indices),
        _ => Err(loop_form_refusal()),
    }
}

/// The point form: coordinates checked for the format, widened to `i64`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] through [`check_points`] when a
/// coordinate leaves the binary `i16` range.
fn points_of(format: MapFormat, points: &[LoopPointParam]) -> Result<Vec<Pt>, CyberError> {
    if format != MapFormat::Udmf {
        check_points(points)?;
    }
    Ok(points
        .iter()
        .map(|corner| (i64::from(corner.x), i64::from(corner.y)))
        .collect())
}

/// The index form: every corner resolved to its vertex' coordinates.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the first index past [`Map::vertices`].
fn vertices_of(map: &Map, indices: &[usize]) -> Result<Vec<Pt>, CyberError> {
    let mut corners: Vec<Pt> = Vec::with_capacity(indices.len());
    for index in indices {
        let vertex = map.vertices.get(*index).ok_or_else(|| {
            let count = map.vertices.len();
            let range = count.checked_sub(1).map_or_else(
                || "the map has no vertices".to_owned(),
                |last| format!("the map has {count} vertices, valid indices are 0..={last}"),
            );
            CyberError::NotFound(format!(
                "vertex {index} does not exist ({range}): pass existing indices, or draw with \
                 points"
            ))
            .with_hint("describe_map or find_elements returns indices")
        })?;
        corners.push((i64::from(vertex.x), i64::from(vertex.y)));
    }
    Ok(corners)
}

/// Refuses every point corner an `i16` binary `VERTEXES` column cannot hold.
fn check_points(points: &[LoopPointParam]) -> Result<(), CyberError> {
    for corner in points {
        fits_coord("x", corner.x)?;
        fits_coord("y", corner.y)?;
    }
    Ok(())
}

/// Refuses a loop coordinate an `i16` binary `VERTEXES` column cannot hold.
fn fits_coord(field: &str, value: i32) -> Result<(), CyberError> {
    if (COORD_MIN..=COORD_MAX).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "the loop point {field} {value} is out of range for the binary map format: \
         coordinates hold {COORD_MIN}..={COORD_MAX}"
    )))
}

/// The sector the loop parameters describe, checked like `add_sector`'s.
fn loop_sector(args: &CreateSectorFromLoopParams, format: MapFormat) -> Result<Sector, CyberError> {
    let sector = Sector {
        floor_height: args.floor_height.unwrap_or(DEFAULT_FLOOR_HEIGHT),
        ceiling_height: args.ceiling_height.unwrap_or(DEFAULT_CEILING_HEIGHT),
        floor_flat: flat_or_default(args.floor_flat.as_ref(), DEFAULT_FLOOR_FLAT, format)?,
        ceiling_flat: flat_or_default(args.ceiling_flat.as_ref(), DEFAULT_CEILING_FLAT, format)?,
        light: args.light.unwrap_or(DEFAULT_LIGHT),
        special: args.special.unwrap_or_default(),
        tag: args.tag.unwrap_or_default(),
        extra: BTreeMap::new(),
    };
    check_sector(format, &sector)?;
    Ok(sector)
}

/// A texture parameter validated and uppercased, `fallback` when absent.
fn texture_or_default(
    given: Option<&String>,
    fallback: &str,
    format: MapFormat,
) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(fallback.to_owned()),
        |name| validate_texture_name(name, format),
    )
}

/// The three wall face textures: the one-sided wall and the two transition
/// faces, each defaulting to `wall_texture`.
fn wall_faces(
    args: &CreateSectorFromLoopParams,
    format: MapFormat,
) -> Result<(String, String, String), CyberError> {
    let wall = texture_or_default(args.wall_texture.as_ref(), DEFAULT_WALL_TEXTURE, format)?;
    let upper = args.upper_texture.as_ref().map_or_else(
        || Ok(wall.clone()),
        |name| validate_texture_name(name, format),
    )?;
    let lower = args.lower_texture.as_ref().map_or_else(
        || Ok(wall.clone()),
        |name| validate_texture_name(name, format),
    )?;
    Ok((wall, upper, lower))
}

/// The outer rule the `outer_sector` parameter gives, `"auto"` when absent.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a mode name other than `"auto"` or
/// `"none"`.
fn outer_sector_of(given: Option<OuterSectorParam>) -> Result<Outside, CyberError> {
    match given {
        None => Ok(Outside::Original),
        Some(OuterSectorParam::Index(index)) => Ok(Outside::Face(index)),
        Some(OuterSectorParam::Mode(text)) => match text.to_ascii_lowercase().as_str() {
            OUTER_AUTO => Ok(Outside::Original),
            OUTER_NONE => Ok(Outside::Solid),
            other => Err(CyberError::InvalidArgument(format!(
                "outer_sector {other:?} is not a mode: pass an existing sector index, \
                 \"{OUTER_AUTO}\" to open each new wall onto the sector found outside it, \
                 or \"{OUTER_NONE}\" to keep new walls one-sided"
            ))),
        },
    }
}

/// The validated [`OverlayRequest`] the parameters and the map describe.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad corner form, an out-of-range
/// coordinate, a sector the format cannot store or a bad texture/mode;
/// [`CyberError::NotFound`] for an unknown vertex or outer-sector index.
fn build_request(
    args: &CreateSectorFromLoopParams,
    map: &Map,
) -> Result<OverlayRequest, CyberError> {
    let (wall, upper, lower) = wall_faces(args, map.format)?;
    Ok(OverlayRequest {
        polygon: polygon_of(args.points.as_ref(), args.vertices.as_ref(), map)?,
        paint: Paint::Sector(loop_sector(args, map.format)?),
        keep_interior: args.keep_interior,
        textures: WallTextures { wall, upper, lower },
        outside: outer_sector_of(args.outer_sector.clone())?,
    })
}

/// The [`OverlayRequest`] the `carve_void` parameters and the map
/// describe: solid paint, the theme's wall texture standing in for walls
/// the surrounding sectors cannot dress from their own.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad corner form or an
/// out-of-range coordinate; [`CyberError::NotFound`] for an unknown
/// vertex index.
fn build_void_request(
    args: &CarveVoidParams,
    map: &Map,
    wall: &str,
) -> Result<OverlayRequest, CyberError> {
    Ok(OverlayRequest {
        polygon: polygon_of(args.points.as_ref(), args.vertices.as_ref(), map)?,
        paint: Paint::Void,
        keep_interior: false,
        textures: WallTextures {
            wall: wall.to_owned(),
            upper: wall.to_owned(),
            lower: wall.to_owned(),
        },
        outside: Outside::Original,
    })
}

/// `create_sector_from_loop`'s work: resolve the map, then run the
/// one-step painting on it.
fn create_run(
    session: &mut Session,
    args: &CreateSectorFromLoopParams,
) -> Result<LoopToolReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = "create sector from loop".to_owned();
    let (result, report) = session.edit_map(name, &label, |map| {
        let request = build_request(args, map)?;
        overlay::apply_overlay(map, &request)
    })?;
    Ok(LoopToolReport {
        result,
        edit: edit_result_fragment(&report),
    })
}

/// The wall texture a carve dresses bare sectors with: the session
/// theme's `Wall`, else the `create_sector_from_loop` default.
fn theme_wall(session: &Session) -> Result<String, CyberError> {
    let themed = theme_texture(session, Role::Wall)?;
    if themed == NO_TEXTURE {
        return Ok(DEFAULT_WALL_TEXTURE.to_owned());
    }
    Ok(themed)
}

/// `carve_void`'s work: resolve the map and the theme wall, then run the
/// one-step carving on it.
fn carve_run(session: &mut Session, args: &CarveVoidParams) -> Result<CarveToolReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let wall = theme_wall(session)?;
    let label = "carve void".to_owned();
    let (result, report) = session.edit_map(name, &label, |map| {
        let request = build_void_request(args, map, &wall)?;
        overlay::apply_overlay(map, &request)
    })?;
    Ok(CarveToolReport {
        removed_sectors_empty: result.empty_sectors.clone(),
        result,
        edit: edit_result_fragment(&report),
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

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = edit_sectors_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Appends one sector to a map in a single undoable edit.
    #[tool(
        description = "Adds one sector (a region sharing floor, ceiling, flats, light and effect) to a map in a single undoable edit with vanilla-flavoured defaults; the appended record stays dead space until sidedefs reference it. Prefer create_sector_from_loop to grow a whole room with its walls in one call, or build_map_from_plan for a finished map; pair this with add_sidedef only for exact low-level control."
    )]
    fn add_sector(
        &self,
        Parameters(args): Parameters<AddSectorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Changes one or several sectors in a single undoable edit.
    #[tool(
        description = "Changes one or several sectors of a map in a single undoable edit: floor_height/ceiling_height or deltas, floor_flat/ceiling_flat, light, special and tag, every change applied to each selected index; a ceiling below its floor is refused. Prefer mark_secret and set_light_effect for those specials, and bulk_edit plus replace_texture for sweeping values or flats across many sectors by query; undo steps it all back."
    )]
    fn edit_sector(
        &self,
        Parameters(args): Parameters<EditSectorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| edit_run(session, &args)),
        )
    }

    /// Removes one or several sectors in a single undoable edit.
    #[tool(
        description = "Removes one sector or a list of them from a map in a single undoable edit; the remaining sectors renumber and surviving sidedef references follow. Without cascade a sector a sidedef still uses is refused with conflict naming them; cascade: true takes the room with it - its sides go, lines flip or close, orphans prune. Sector count changes invalidate nodes: rebuild before vanilla play."
    )]
    fn remove_sector(
        &self,
        Parameters(args): Parameters<RemoveSectorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }

    /// Draws a closed corner loop as one new sector with its walls.
    #[tool(
        description = "Draws a closed loop of corners as one new sector with its boundary walls, in a single undoable edit: the draw-a-room tool. points: [{x, y}, ...] (at least 3, implicitly closed) or vertices: [i, ...]; sector properties as in add_sector. The geometry engine splits existing walls at T-junctions and crossings and merges painted-over sides, so the result never crosses itself. Prefer build_map_from_plan for a whole fresh level; use this to add rooms to a map."
    )]
    fn create_sector_from_loop(
        &self,
        Parameters(args): Parameters<CreateSectorFromLoopParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| create_run(session, &args)),
        )
    }

    /// Carves a polygon of solid rock into the map.
    #[tool(
        description = "Carves a closed polygon of solid rock out of a map in a single undoable edit: the area holds no sector and the surrounding sectors get one-sided walls along the cut. The ring comes as points: [{x, y}, ...] or existing vertices: [i, ...]; zero-area or self-intersecting rings are refused by the geometry engine. Use it to pull a boundary in or delete leftover space; create_sector_from_loop does the opposite."
    )]
    fn carve_void(
        &self,
        Parameters(args): Parameters<CarveVoidParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| carve_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use serde_json::from_str;

    use super::{
        AddSectorParams, CarveVoidParams, CreateSectorFromLoopParams, DEFAULT_CEILING_FLAT,
        DEFAULT_CEILING_HEIGHT, DEFAULT_FLOOR_FLAT, DEFAULT_LIGHT, EditSectorParams, FIELD_MAX,
        HEIGHT_MAX, HEIGHT_MIN, LIGHT_MAX, LIGHT_MIN, add_in_place, build_request,
        build_void_request, cascade_options, check_binary_ranges, check_height_order, check_light,
        edit_in_place, edited_flat, edited_sector, flat_or_default, has_change, join, new_height,
        new_sector, outer_sector_of, planned_edits, polygon_of, remove_in_place, validate_edit,
        validate_flat,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::overlay::{Outside, Paint};
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
    use crate::wad::name::LumpName;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// An add request parsed from `json`.
    fn add_args(json: &str) -> AddSectorParams {
        from_str(json).expect("test add requests parse")
    }

    /// An edit request parsed from `json`.
    fn edit_args(json: &str) -> EditSectorParams {
        from_str(json).expect("test edit requests parse")
    }

    /// A remove request parsed from `json`.
    fn remove_args(json: &str) -> super::RemoveSectorParams {
        from_str(json).expect("test remove requests parse")
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A sidedef opening into `sector`.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Default::default()
        }
    }

    /// A one-sided line from `v1` to `v2` over front sidedef `front`.
    fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-sided line from `v1` to `v2` over sidedefs `front`/`back`.
    fn open_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// A sector with the given heights, light and tag.
    fn room(floor_height: i32, ceiling_height: i32, light: i32, tag: i32) -> Sector {
        Sector {
            floor_height,
            ceiling_height,
            floor_flat: "FLOOR1".to_owned(),
            ceiling_flat: "FLOOR1".to_owned(),
            light,
            tag,
            ..Default::default()
        }
    }

    /// The shared test map: a hall (sector 0, sidedefs 0/1), a raised room
    /// (sector 1, sidedefs 2/3) split by the two-sided line `1`, and an
    /// unreferenced sector `2`.
    fn test_map() -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("a valid name"),
            format: MapFormat::Doom,
            vertices: vec![
                vertex(0, 0),
                vertex(128, 0),
                vertex(128, 128),
                vertex(0, 128),
            ],
            linedefs: vec![
                solid_line(0, 1, 0),
                open_line(1, 2, 1, 2),
                solid_line(2, 3, 3),
            ],
            sidedefs: vec![side(0), side(0), side(1), side(1)],
            sectors: vec![
                room(0, 128, 160, 0),
                room(32, 128, 192, 1),
                room(0, 128, 160, 0),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn added_sectors_take_the_vanilla_defaults() {
        let plain = new_sector(&add_args("{}"), MapFormat::Doom).expect("defaults build");
        assert_eq!(
            (plain.floor_height, plain.ceiling_height),
            (0_i32, DEFAULT_CEILING_HEIGHT),
            "the heights default to 0 and 128"
        );
        assert_eq!(plain.floor_flat, DEFAULT_FLOOR_FLAT, "the flat defaults");
        assert_eq!(plain.ceiling_flat, DEFAULT_CEILING_FLAT);
        assert_eq!(plain.light, DEFAULT_LIGHT, "and the light to 160");
        assert_eq!(
            (plain.special, plain.tag),
            (0_i32, 0_i32),
            "special and tag start at 0"
        );
        let named = add_args(
            r#"{"floor_height": -8, "ceiling_height": 200, "floor_flat": "floor1",
                "light": 255, "special": 11, "tag": 3000}"#,
        );
        let custom = new_sector(&named, MapFormat::Doom).expect("a full request builds");
        assert_eq!(custom.floor_height, -8_i32, "the given heights land");
        assert_eq!(custom.ceiling_height, 200_i32);
        assert_eq!(custom.floor_flat, "FLOOR1", "the name is uppercased");
        assert_eq!(custom.light, 255_i32, "the top of the vanilla range fits");
        assert_eq!((custom.special, custom.tag), (11_i32, 3_000_i32));
    }

    #[test]
    fn flat_names_validate_uppercase_and_refuse_the_dash() {
        assert_eq!(
            flat_or_default(None, DEFAULT_FLOOR_FLAT, MapFormat::Doom)
                .expect("absent is the fallback"),
            "FLOOR4_8"
        );
        let dashed = validate_flat("-", MapFormat::Doom).expect_err("a dash is no flat");
        assert_eq!(code(&dashed), "invalid_argument", "dash code");
        assert!(
            dashed.to_string().contains("FLOOR4_8"),
            "the message names a real flat: {dashed}"
        );
        assert_eq!(
            validate_flat("floor4_8", MapFormat::Doom).expect("a vanilla flat"),
            "FLOOR4_8",
            "names are uppercased"
        );
        assert!(
            validate_flat("123456789", MapFormat::Doom).is_err(),
            "nine characters refuse on binary"
        );
        assert!(
            validate_flat("123456789", MapFormat::Udmf).is_ok(),
            "UDMF takes 64"
        );
        assert_eq!(
            edited_flat("FLOOR1", None, MapFormat::Doom).expect("absent keeps the current"),
            "FLOOR1"
        );
        assert_eq!(
            edited_flat("FLOOR1", Some(&"ceil5".to_owned()), MapFormat::Doom).expect("a new flat"),
            "CEIL5",
            "replacement wins"
        );
    }

    #[test]
    fn heights_shift_by_delta_and_absolute_wins() {
        assert_eq!(
            new_height("floor_height", 32_i32, Some(48_i32), None).expect("absolute"),
            48_i32
        );
        assert_eq!(
            new_height("ceiling_height", 128_i32, None, Some(-16_i32)).expect("delta shifts"),
            112_i32
        );
        assert_eq!(
            new_height("floor_height", 32_i32, None, None).expect("neither keeps"),
            32_i32
        );
        let overflow =
            new_height("floor_height", i32::MAX, None, Some(1_i32)).expect_err("above i32 refuses");
        assert_eq!(code(&overflow), "invalid_argument", "overflow code");
        assert!(
            overflow.to_string().contains("floor_height"),
            "the field is named: {overflow}"
        );
    }

    #[test]
    fn ranges_light_and_height_order_are_checked() {
        let edges = room(HEIGHT_MIN, HEIGHT_MAX, LIGHT_MAX, FIELD_MAX);
        assert!(
            check_binary_ranges(MapFormat::Doom, &edges).is_ok(),
            "the i16/u16 edges fit"
        );
        let low = Sector {
            ceiling_height: HEIGHT_MAX.saturating_add(1),
            ..edges.clone()
        };
        let height_error =
            check_binary_ranges(MapFormat::Hexen, &low).expect_err("above i16 refuses");
        assert_eq!(code(&height_error), "invalid_argument", "height range code");
        let wide_tag = Sector {
            tag: FIELD_MAX.checked_add(1).expect("edge plus one"),
            ..edges
        };
        assert!(
            check_binary_ranges(MapFormat::Doom, &wide_tag).is_err(),
            "a tag above u16 refuses on binary"
        );
        assert!(
            check_binary_ranges(MapFormat::Udmf, &wide_tag).is_ok(),
            "UDMF stores full integers"
        );
        assert!(
            check_light(LIGHT_MIN).is_ok() && check_light(255_i32).is_ok(),
            "0 and 255 fit"
        );
        assert_eq!(
            code(&check_light(300_i32).expect_err("300 refuses")),
            "invalid_argument"
        );
        assert_eq!(
            code(&check_light(-1_i32).expect_err("negatives refuse")),
            "invalid_argument"
        );
        let closed = room(64, 64, 160, 0);
        assert!(
            check_height_order(&closed).is_ok(),
            "equal heights are a closed door"
        );
        let inverted = room(200, 128, 160, 0);
        let order_error = check_height_order(&inverted).expect_err("ceiling below floor refuses");
        assert_eq!(code(&order_error), "invalid_argument", "order code");
        assert!(
            order_error.to_string().contains("ceiling_height 128")
                && order_error.to_string().contains("floor_height 200"),
            "the message shows both heights: {order_error}"
        );
    }

    #[test]
    fn edited_sectors_combine_unchanged_fields_with_the_request() {
        let current = room(32, 128, 192, 1);
        let args = edit_args(r#"{"index": 0, "floor_delta": 16, "light": 255}"#);
        let edited = edited_sector(&current, &args, MapFormat::Doom).expect("the edit computes");
        assert_eq!(edited.floor_height, 48_i32, "the delta raised the floor");
        assert_eq!(edited.ceiling_height, 128_i32, "the unasked height stands");
        assert_eq!(edited.light, 255_i32, "the asked light replaced");
        assert_eq!(edited.floor_flat, "FLOOR1", "and the flats survive");
        assert_eq!(edited.tag, 1_i32, "with the tag");
        let special = edit_args(r#"{"index": 0, "special": 0}"#);
        assert!(
            has_change(&special),
            "an explicit zero still counts as a change"
        );
    }

    #[test]
    fn edit_validation_names_conflicts_and_nothings() {
        let sorted = validate_edit(&edit_args(r#"{"index": [3, 1], "floor_delta": 8}"#))
            .expect("a list with deltas is fine");
        assert_eq!(sorted, vec![1_usize, 3], "sorted and deduplicated");
        let refusals = [
            r#"{"index": 0, "floor_height": 8, "floor_delta": 8}"#,
            r#"{"index": 0, "ceiling_height": 8, "ceiling_delta": 8}"#,
            r#"{"index": 0}"#,
            r#"{"index": []}"#,
        ];
        for json in refusals {
            let error = validate_edit(&edit_args(json)).expect_err("the request must refuse");
            assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
        }
    }

    #[test]
    fn adding_appends_only_after_every_check() {
        let mut map = test_map();
        let bad_flat = add_args(r#"{"floor_flat": "-"}"#);
        let flat_error = add_in_place(&mut map, &bad_flat).expect_err("the dash refuses");
        assert_eq!(code(&flat_error), "invalid_argument", "flat code");
        let bright = add_args(r#"{"light": 300}"#);
        assert_eq!(
            code(&add_in_place(&mut map, &bright).expect_err("light 300 refuses")),
            "invalid_argument",
            "light code"
        );
        let sunken = add_args(r#"{"floor_height": 200}"#);
        assert_eq!(
            code(&add_in_place(&mut map, &sunken).expect_err("ceiling below floor refuses")),
            "invalid_argument",
            "order code"
        );
        let wide = add_args(r#"{"tag": 70000}"#);
        assert_eq!(
            code(&add_in_place(&mut map, &wide).expect_err("a wide tag refuses on binary")),
            "invalid_argument",
            "range code"
        );
        assert_eq!(map.sectors.len(), 3, "none of the refusals appended");
        map.format = MapFormat::Udmf;
        let row = add_in_place(&mut map, &wide).expect("UDMF stores the wide tag");
        assert_eq!(row.index, 3, "after the existing three");
        assert_eq!(row.detail.tag, 70_000_i32, "and reports it");
        assert_eq!(map.sectors.len(), 4, "the list grew by one");
    }

    #[test]
    fn editing_touches_only_the_selection() {
        let mut map = test_map();
        let past = edit_args(r#"{"index": 9, "light": 200}"#);
        let error = edit_in_place(&mut map, &past, &[9]).expect_err("sector 9 is not there");
        assert_eq!(code(&error), "not_found", "a missing index refuses");
        assert!(
            error.to_string().contains("map has 3 sectors"),
            "the message names the count: {error}"
        );
        let inverted = edit_args(r#"{"index": 0, "floor_height": 200}"#);
        assert_eq!(
            code(
                &edit_in_place(&mut map, &inverted, &[0]).expect_err("ceiling below floor refuses")
            ),
            "invalid_argument",
            "order code"
        );
        assert_eq!(
            map.sectors[0].floor_height, 0_i32,
            "the refusal changed nothing"
        );
        let rows = edit_in_place(
            &mut map,
            &edit_args(r#"{"index": [0, 2], "light": 96}"#),
            &[0_usize, 2],
        )
        .expect("both sectors darken");
        assert_eq!(rows.len(), 2, "one row per selection");
        assert_eq!(rows[0].index, 0, "the rows come sorted");
        assert_eq!(map.sectors[0].light, 96_i32, "sector 0 changed");
        assert_eq!(map.sectors[2].light, 96_i32, "sector 2 changed");
        assert_eq!(
            map.sectors[1].light, 192_i32,
            "the unselected sector stands"
        );
    }

    #[test]
    fn the_planning_pass_validates_the_whole_selection_first() {
        let map = test_map();
        let args = edit_args(r#"{"index": [0, 1], "floor_height": 200}"#);
        let error = planned_edits(&map, &args, &[0_usize, 1])
            .expect_err("sector 0's ceiling cannot go under its new floor");
        assert_eq!(code(&error), "invalid_argument", "order code");
        let light_refusal = edit_args(r#"{"index": [0, 1], "light": 300}"#);
        assert!(
            planned_edits(&map, &light_refusal, &[0_usize, 1]).is_err(),
            "the light range refuses before any store"
        );
    }

    #[test]
    fn removal_refuses_used_sectors_and_cuts_orphans() {
        let mut map = test_map();
        let used = remove_in_place(&mut map, &remove_args(r#"{"index": 0}"#), &[0]);
        let error = used.expect_err("sidedefs 0/1 use sector 0");
        assert_eq!(code(&error), "conflict", "referenced without cascade");
        assert!(
            error
                .to_string()
                .contains("sector 0 is used by sidedef(s) 0, 1")
                && error.to_string().contains("cascade"),
            "the message names the users and the fix: {error}"
        );
        assert_eq!(map.sectors.len(), 3, "the conflict changed nothing");
        let past = remove_in_place(&mut map, &remove_args(r#"{"index": 9}"#), &[9]);
        assert_eq!(
            code(&past.expect_err("sector 9 is not there")),
            "not_found",
            "a missing index refuses"
        );
        let report = remove_in_place(&mut map, &remove_args(r#"{"index": 2}"#), &[2])
            .expect("the unreferenced sector goes without a cascade");
        assert_eq!(report.sectors, vec![2_usize], "the removed original index");
        assert!(
            report.sidedefs.is_empty() && report.linedefs.is_empty(),
            "nothing else"
        );
        assert_eq!(map.sectors.len(), 2, "and the list shrank");
    }

    #[test]
    fn a_cascaded_removal_closes_the_line_and_prunes_the_room() {
        let mut map = test_map();
        let report = remove_in_place(
            &mut map,
            &remove_args(r#"{"index": 1, "cascade": true}"#),
            &[1],
        )
        .expect("the cascade lifts the conflict");
        assert_eq!(report.sectors, vec![1_usize], "the seeded sector goes");
        assert_eq!(report.sidedefs, vec![2_usize, 3], "both of its sides go");
        assert_eq!(
            report.linedefs,
            vec![2_usize],
            "the wall only they held goes"
        );
        assert_eq!(report.vertices, vec![3_usize], "and its orphaned corner");
        assert_eq!(
            report.one_sided_linedefs,
            vec![1_usize],
            "the shared line closed"
        );
        assert!(report.flipped_linedefs.is_empty(), "nothing had to flip");
        assert_eq!(map.sectors.len(), 2, "the hall and the spare survive");
        assert_eq!(map.sidedefs.len(), 2, "only the hall's sides survive");
        assert_eq!(map.linedefs.len(), 2, "two linedefs survive");
        assert_eq!(map.linedefs[1].back, None, "the shared line is one-sided");
        assert_eq!(map.linedefs[1].flags, 1, "two-sided cleared, blocking set");
    }

    #[test]
    fn the_pruning_flags_reach_the_cascade() {
        let pruning = remove_args(r#"{"index": 1, "cascade": true}"#);
        assert!(
            cascade_options(&pruning).prune_sidedefs && cascade_options(&pruning).prune_vertices,
            "both prunings default on"
        );
        let let_alone = remove_args(
            r#"{"index": 1, "cascade": true, "prune_sidedefs": false, "prune_vertices": false}"#,
        );
        let options = cascade_options(&let_alone);
        assert!(
            !options.prune_sidedefs && !options.prune_vertices,
            "both switch off"
        );
        let mut map = test_map();
        let report =
            remove_in_place(&mut map, &let_alone, &[1]).expect("the unpruned cascade applies");
        assert_eq!(
            report.linedefs,
            vec![2_usize],
            "the sideless line still dies"
        );
        assert!(report.vertices.is_empty(), "the orphaned vertex stays");
        assert_eq!(map.vertices.len(), 4, "and the list kept its length");
        let used: BTreeSet<usize> = map
            .linedefs
            .iter()
            .flat_map(|line| [line.v1, line.v2])
            .collect();
        assert!(!used.contains(&3), "no surviving line uses vertex 3");
    }

    #[test]
    fn the_conflict_message_joins_every_user() {
        let uses: alloc::collections::BTreeMap<usize, Vec<usize>> =
            [(0_usize, vec![0_usize, 1]), (1_usize, vec![2, 3])]
                .into_iter()
                .collect();
        let message = super::dependent_refusal(&uses).to_string();
        assert!(
            message.contains("sector 0 is used by sidedef(s) 0, 1")
                && message.contains("sector 1 is used by sidedef(s) 2, 3"),
            "both sectors are listed with their sides: {message}"
        );
        assert!(
            join(&[4_usize, 7]) == "4, 7",
            "the joiner spells the list out"
        );
    }

    /// A loop request parsed from `json`.
    fn loop_args(json: &str) -> CreateSectorFromLoopParams {
        from_str(json).expect("test loop requests parse")
    }

    /// A carve request parsed from `json`.
    fn carve_args(json: &str) -> CarveVoidParams {
        from_str(json).expect("test carve requests parse")
    }

    /// The three-corner points parameter list, as JSON.
    const THREE_POINTS: &str =
        r#""points": [{"x": 0, "y": 0}, {"x": 128, "y": 0}, {"x": 64, "y": 128}]"#;

    /// The points form over [`THREE_POINTS`], as a JSON object.
    fn three_points_json(tail: &str) -> String {
        format!("{{{THREE_POINTS}{tail}}}")
    }

    #[test]
    fn the_loop_needs_exactly_one_corner_form() {
        let map = test_map();
        let points_args = loop_args(&three_points_json(""));
        let points = polygon_of(
            points_args.points.as_ref(),
            points_args.vertices.as_ref(),
            &map,
        )
        .expect("the points form is accepted");
        assert_eq!(points.len(), 3, "three corners stay three");
        assert_eq!(points.first().copied(), Some((0_i64, 0_i64)), "widened");
        let indices =
            polygon_of(None, Some(&vec![0_usize, 1, 2]), &map).expect("the index form is accepted");
        assert_eq!(indices.len(), 3, "three indices stay three");
        assert_eq!(indices.first().copied(), Some((0_i64, 0_i64)), "resolved");
        let both = three_points_json(", \"vertices\": [0, 1, 2]");
        for json in [
            "{}",
            r#"{"points": [], "vertices": [0, 1, 2]}"#,
            both.as_str(),
        ] {
            let args = loop_args(json);
            let error = polygon_of(args.points.as_ref(), args.vertices.as_ref(), &map)
                .expect_err("one form is required");
            assert_eq!(code(&error), "invalid_argument", "form code for {json}");
        }
        let past = polygon_of(None, Some(&vec![0_usize, 1, 99]), &map)
            .expect_err("vertex 99 is not there");
        assert_eq!(code(&past), "not_found", "vertex code");
        assert!(
            past.to_string().contains("map has 4 vertices"),
            "the message counts the vertices: {past}"
        );
    }

    #[test]
    fn the_built_request_carries_the_documented_defaults() {
        let map = test_map();
        let args = loop_args(&three_points_json(""));
        let request = build_request(&args, &map).expect("the minimal loop builds");
        assert_eq!(
            request.textures.wall, "STARTAN3",
            "the wall texture default"
        );
        assert_eq!(
            request.textures.upper, "STARTAN3",
            "the faces fall back to the wall"
        );
        assert_eq!(request.textures.lower, "STARTAN3");
        assert_eq!(
            request.outside,
            Outside::Original,
            "auto is the outer default"
        );
        assert!(
            !request.keep_interior,
            "the interior is overwritten by default"
        );
        let Paint::Sector(sector) = request.paint else {
            panic!("a room paints a sector");
        };
        assert_eq!(
            (sector.floor_height, sector.ceiling_height, sector.light),
            (0_i32, DEFAULT_CEILING_HEIGHT, DEFAULT_LIGHT),
            "the sector defaults are add_sector's"
        );
        assert_eq!(sector.floor_flat, DEFAULT_FLOOR_FLAT);
        let full = loop_args(
            r#"{"points": [{"x": 0, "y": 0}, {"x": 128, "y": 0}, {"x": 64, "y": 128}],
                "ceiling_height": 192, "tag": 7, "wall_texture": "brick11",
                "upper_texture": "-", "lower_texture": "step1", "outer_sector": "none",
                "keep_interior": true}"#,
        );
        let built = build_request(&full, &map).expect("the full loop builds");
        assert_eq!(built.textures.wall, "BRICK11", "names are uppercased");
        assert_eq!(built.textures.upper, "-", "a wall texture may be the dash");
        assert_eq!(built.textures.lower, "STEP1");
        assert_eq!(built.outside, Outside::Solid, "none closes the outer");
        assert!(built.keep_interior, "the flag comes through");
        let Paint::Sector(deep) = built.paint else {
            panic!("a room paints a sector");
        };
        assert_eq!(deep.ceiling_height, 192_i32);
        assert_eq!(deep.tag, 7_i32);
        let indexed = loop_args(r#"{"vertices": [0, 1, 2, 3], "outer_sector": 1}"#);
        let explicit = build_request(&indexed, &map).expect("the indexed loop builds");
        assert_eq!(explicit.outside, Outside::Face(1), "an index is taken");
        assert_eq!(
            explicit.polygon,
            vec![(0_i64, 0_i64), (128, 0), (128, 128), (0, 128)],
            "the indices resolve to their vertices' coordinates"
        );
    }

    #[test]
    fn a_void_request_carves_with_the_given_wall_texture() {
        let map = test_map();
        let carve =
            carve_args(r#"{"points": [{"x": 8, "y": 8}, {"x": 64, "y": 8}, {"x": 8, "y": 64}]}"#);
        let request = build_void_request(&carve, &map, "THEME1").expect("the carve builds");
        assert_eq!(request.paint, Paint::Void, "the interior becomes solid");
        assert!(!request.keep_interior, "a carve never keeps the interior");
        assert_eq!(
            request.outside,
            Outside::Original,
            "neighbours stay as they are"
        );
        assert_eq!(request.textures.wall, "THEME1", "the theme wall stands in");
        assert_eq!(request.textures.upper, "THEME1");
        assert_eq!(request.polygon.len(), 3, "the triangle comes through");
    }

    #[test]
    fn bad_loop_fields_are_refused_before_anything_draws() {
        let map = test_map();
        let refusals = [
            three_points_json(", \"light\": 300"),
            three_points_json(", \"floor_height\": 200"),
            three_points_json(", \"ceiling_flat\": \"-\""),
            three_points_json(", \"wall_texture\": \"TOOLONG12\""),
            three_points_json(", \"outer_sector\": \"sideways\""),
            r#"{"points": [{"x": 0, "y": 0}, {"x": 40000, "y": 0}, {"x": 64, "y": 128}]}"#
                .to_owned(),
        ];
        for json in &refusals {
            let args = loop_args(json);
            let error =
                build_request(&args, &map).expect_err("the field must refuse on a binary map");
            assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
        }
        let mode = outer_sector_of(loop_args(r#"{"outer_sector": "AUTO"}"#).outer_sector)
            .expect("case-insensitive auto");
        assert_eq!(mode, Outside::Original, "the mode name uppercases away");
        let mut udmf = test_map();
        udmf.format = MapFormat::Udmf;
        let wide = loop_args(
            r#"{"points": [{"x": 0, "y": 0}, {"x": 40000, "y": 0}, {"x": 64, "y": 128}],
                "wall_texture": "a_long_udmf_wall_texture", "tag": 70000}"#,
        );
        build_request(&wide, &udmf).expect("UDMF takes the wide coordinates, name and tag");
    }
}
