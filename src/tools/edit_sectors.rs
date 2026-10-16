//! Sector editing tools: `add_sector`, `edit_sector` and `remove_sector`
//! (see the `edit_sectors` router methods on [`crate::server::Cyberdemon`]).
//!
//! Every call is exactly one [`crate::session::Session::edit_map`] step, so
//! one undo entry moves the whole call. The parameter shapes
//! ([`IndexSelector`]) and validators ([`check_index`],
//! [`validate_texture_name`]) come from [`crate::tools::maps::params`];
//! removal runs through [`crate::wad::map::remove`] and refuses the
//! `conflict` of a sector that sidedefs still use unless `cascade` is set.
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
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{
    IndexSelector, check_index, edit_result_fragment, validate_texture_name,
};
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
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
        description = "Adds one sector (a region sharing floor, ceiling, flats, light and effect) to a map in a single undoable edit. map defaults to the active map (set_active_map, else the merged view's only map). Every field is optional with vanilla-flavoured defaults: floor_height (0), ceiling_height (128), floor_flat (FLOOR4_8), ceiling_flat (CEIL3_5), light (160), special (0) and tag (0). A sector does nothing until sidedefs reference it: the appended record is dead space until sides point at it (add_sidedef or edit_sidedef) - to grow a whole room at once use create_sector_from_loop (not yet implemented). Flat names are uppercased and validated (non-empty, printable ASCII, at most 8 characters on binary maps and 64 on UDMF); \"-\" means no texture and is refused for a flat (invalid_argument). Light must sit in 0..=255 (invalid_argument outside). A ceiling below the floor is invalid_argument (equal heights are allowed: a closed door). Binary maps store the heights as i16 and special/tag as u16: out-of-range values are invalid_argument; UDMF is unbounded. Adding changes the sector count, so the node lumps are invalidated (nodes_invalidated true; run build_all_nodes before playing the map in vanilla Doom). Returns {index, sector: {floor_height, ceiling_height, floor_flat, ceiling_flat, light, special, tag}, map, nodes_invalidated, copied_to_working}. Unknown map is not_found."
    )]
    fn add_sector(
        &self,
        Parameters(args): Parameters<AddSectorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Changes one or several sectors in a single undoable edit.
    #[tool(
        description = "Changes one or several sectors of a map in a single undoable edit. index is one sector index or a list of them (duplicates collapse, sorted first); every change parameter applies to all selected sectors and only the given fields change. Parameters, all optional: floor_height and ceiling_height (absolute map units), floor_delta and ceiling_delta (shift each selected sector's current height by this much), floor_flat and ceiling_flat (new flat names, validated like add_sector's), light (0..=255), special and tag. Absolute and delta conflict on the same axis (invalid_argument): floor_height with floor_delta, ceiling_height with ceiling_delta. A call with no change parameter is invalid_argument. Each resulting sector must keep its ceiling at or above its floor (equal is allowed: a closed door), its light in 0..=255 and the format's ranges (binary heights i16, special/tag u16; UDMF unbounded), else invalid_argument. An index the map does not hold is not_found and nothing changes. Sector properties are not geometry: the node lumps stay trusted (nodes_invalidated false). Returns {sectors: [{index, floor_height, ceiling_height, floor_flat, ceiling_flat, light, special, tag}], map, nodes_invalidated, copied_to_working}."
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
        description = "Removes one sector or a list of them (index: one integer or an array) from a map in a single undoable edit; the remaining sectors renumber down and every surviving sidedef's sector reference follows. Without cascade, a sector a sidedef still uses is refused with conflict naming the referencing sidedefs and nothing changes; an unreferenced sector (e.g. one just added) removes fine. With cascade: true the removal takes the room with it: every sidedef of a removed sector goes; a linedef that loses its front side is flipped (its back becomes the front), one that loses its back becomes one-sided (two-sided bit cleared, blocking bit set, an otherwise invisible lower or upper texture of the surviving side moved to the middle), and a linedef that loses both sides is removed with them; prune_sidedefs (default true) also drops sidedefs no surviving linedef references and prune_vertices (default true) also drops orphaned vertices. Removing sectors changes the sector count, so the node lumps are invalidated (nodes_invalidated true; run build_all_nodes before playing the map in vanilla Doom). Returns {removed: {sectors, sidedefs, linedefs, vertices, things (removed original indices, sorted), flipped_linedefs, one_sided_linedefs (new indices of the normalised linedefs)}, map, nodes_invalidated, copied_to_working}. An index the map does not hold is not_found; an empty index list is invalid_argument."
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
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use serde_json::from_str;

    use super::{
        AddSectorParams, DEFAULT_CEILING_FLAT, DEFAULT_CEILING_HEIGHT, DEFAULT_FLOOR_FLAT,
        DEFAULT_LIGHT, EditSectorParams, FIELD_MAX, HEIGHT_MAX, HEIGHT_MIN, LIGHT_MAX, LIGHT_MIN,
        add_in_place, cascade_options, check_binary_ranges, check_height_order, check_light,
        edit_in_place, edited_flat, edited_sector, flat_or_default, has_change, join, new_height,
        new_sector, planned_edits, remove_in_place, validate_edit, validate_flat,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
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
}
