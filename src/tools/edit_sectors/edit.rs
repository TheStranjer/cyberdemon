//! `add_sector`, `edit_sector` and `remove_sector`: their arguments, report
//! shapes and one-step working steps.
//!
//! The [`crate::server::Cyberdemon`] router methods hand their parsed
//! [`AddSectorParams`], [`EditSectorParams`] and [`RemoveSectorParams`] to
//! [`add_run`], [`edit_run`] and [`remove_run`]. Each resolves the map (the
//! add also resolving the session theme's [`Surfaces`] once) and
//! then runs [`add_in_place`], [`edit_in_place`] or [`remove_in_place`] as
//! one [`crate::session::Session::edit_map`] step, so a refused call never
//! writes: an `edit_sector` request is validated up front by
//! [`validate_edit`] ([`check_conflicts`] and [`has_change`]) and every
//! selected sector is computed by [`planned_edits`] before any of them is
//! stored; a `remove_sector` call without `cascade` refuses the `conflict`
//! [`dependent_refusal`] spells out, and with it hands the grown set to
//! [`crate::wad::map::remove::apply_removal`]. Every resulting sector faces
//! [`check_sector`] in the parent module beside the [`SectorDetail`] these
//! tools report.

use alloc::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{ElementKind, IndexSelector, check_index, edit_result_fragment};
// The comma-joined index list of the `conflict` refusals, re-exported from
// `crate::tools::maps::params` so this module's tests keep reaching it here.
pub(super) use crate::tools::maps::params::join;
use crate::tools::themes::{Surfaces, surfaces};
use crate::wad::classify::MapFormat;
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, dependents, expand_cascade,
};
use crate::wad::map::{Map, Sector};

use super::{
    DEFAULT_CEILING_HEIGHT, DEFAULT_FLOOR_HEIGHT, DEFAULT_LIGHT, SectorDetail, check_sector,
    detail_of, edited_flat, flat_or_default,
};

/// Arguments for [`Cyberdemon::add_sector`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddSectorParams {
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
    /// Floor flat name (default: the session theme's `Floor` flat,
    /// `FLOOR4_8` when the theme names none).
    #[serde(default)]
    floor_flat: Option<String>,
    /// Ceiling flat name (default: the session theme's `Ceiling` flat,
    /// `CEIL3_5` when the theme names none).
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
pub(super) struct EditSectorParams {
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
pub(super) struct RemoveSectorParams {
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

/// One edited sector: its position plus its fields.
#[derive(Debug, Serialize)]
pub(super) struct EditRow {
    /// Position in [`Map::sectors`].
    pub(super) index: usize,
    /// The sector's fields, inlined next to the index.
    #[serde(flatten)]
    pub(super) detail: SectorDetail,
}

/// Structured result of [`Cyberdemon::add_sector`].
#[derive(Serialize)]
pub(super) struct AddReport {
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
pub(super) struct EditReport {
    /// The edited sectors, ascending by index.
    sectors: Vec<EditRow>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::remove_sector`].
#[derive(Serialize)]
pub(super) struct RemoveToolReport {
    /// What the removal engine dropped, flipped and normalised.
    removed: RemovalReport,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The height after the edit: `absolute` wins, `delta` shifts `current`,
/// neither keeps it.
pub(super) fn new_height(
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

/// The sector the `add_sector` parameters describe for a map of `format`,
/// defaulting unasked flats to the theme `defaults`.
pub(super) fn new_sector(
    args: &AddSectorParams,
    format: MapFormat,
    defaults: &Surfaces,
) -> Result<Sector, CyberError> {
    Ok(Sector {
        floor_height: args.floor_height.unwrap_or(DEFAULT_FLOOR_HEIGHT),
        ceiling_height: args.ceiling_height.unwrap_or(DEFAULT_CEILING_HEIGHT),
        floor_flat: flat_or_default(args.floor_flat.as_ref(), &defaults.floor, format)?,
        ceiling_flat: flat_or_default(args.ceiling_flat.as_ref(), &defaults.ceiling, format)?,
        light: args.light.unwrap_or(DEFAULT_LIGHT),
        special: args.special.unwrap_or_default(),
        tag: args.tag.unwrap_or_default(),
        extra: BTreeMap::new(),
    })
}

/// The sector `current` becomes under the fields `args` gives; unasked
/// fields and the `UDMF` extras stand.
pub(super) fn edited_sector(
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
pub(super) fn add_in_place(
    map: &mut Map,
    args: &AddSectorParams,
    defaults: &Surfaces,
) -> Result<EditRow, CyberError> {
    let sector = new_sector(args, map.format, defaults)?;
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
pub(super) const fn has_change(args: &EditSectorParams) -> bool {
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
pub(super) fn validate_edit(args: &EditSectorParams) -> Result<Vec<usize>, CyberError> {
    let indices = args.index.indices()?;
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
pub(super) type PlannedEdit = (usize, Sector);

/// The validated `(index, new sector)` pairs of an `edit_sector` call;
/// nothing is stored until the whole selection computes.
pub(super) fn planned_edits(
    map: &Map,
    args: &EditSectorParams,
    indices: &[usize],
) -> Result<Vec<PlannedEdit>, CyberError> {
    for index in indices {
        check_index(ElementKind::Sectors, *index, map.sectors.len())?;
    }
    let format = map.format;
    let wanted: BTreeSet<usize> = indices.iter().copied().collect();
    let mut planned = Vec::with_capacity(wanted.len());
    for (index, slot) in map.sectors.iter().enumerate() {
        if !wanted.contains(&index) {
            continue;
        }
        let updated = edited_sector(slot, args, format)?;
        check_sector(format, &updated)?;
        planned.push((index, updated));
    }
    Ok(planned)
}

/// `edit_sector`'s step: rewrite every selected sector and report the rows.
pub(super) fn edit_in_place(
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
pub(super) fn dependent_refusal(uses: &BTreeMap<usize, Vec<usize>>) -> CyberError {
    let parts: Vec<String> = uses
        .iter()
        .map(|(sector, sides)| format!("sector {sector} is used by sidedef(s) {}", join(sides)))
        .collect();
    CyberError::Conflict(format!(
        "{}; pass cascade: true to remove the referencing sidedefs with them",
        parts.join("; ")
    ))
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
pub(super) fn cascade_options(args: &RemoveSectorParams) -> CascadeOptions {
    CascadeOptions {
        prune_sidedefs: args.prune_sidedefs.unwrap_or(true),
        prune_vertices: args.prune_vertices.unwrap_or(true),
    }
}

/// `remove_sector`'s step: check the indices, refuse a referenced sector
/// without `cascade`, then run the removal (grown by the cascade when set).
pub(super) fn remove_in_place(
    map: &mut Map,
    args: &RemoveSectorParams,
    indices: &[usize],
) -> Result<RemovalReport, CyberError> {
    for index in indices {
        check_index(ElementKind::Sectors, *index, map.sectors.len())?;
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

/// `add_sector`'s work: resolve the map and the theme defaults, run the
/// one-step append.
pub(super) fn add_run(
    session: &mut Session,
    args: &AddSectorParams,
) -> Result<AddReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let defaults = surfaces(session)?;
    let label = "add sector".to_owned();
    let (row, report) = session.edit_map(name, &label, |map| add_in_place(map, args, &defaults))?;
    Ok(AddReport {
        index: row.index,
        sector: row.detail,
        edit: edit_result_fragment(&report),
    })
}

/// `edit_sector`'s work: validate, resolve the map, run the one-step edit.
pub(super) fn edit_run(
    session: &mut Session,
    args: &EditSectorParams,
) -> Result<EditReport, CyberError> {
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
pub(super) fn remove_run(
    session: &mut Session,
    args: &RemoveSectorParams,
) -> Result<RemoveToolReport, CyberError> {
    let indices = args.index.indices()?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("remove {} sectors", indices.len());
    let (removed, report) =
        session.edit_map(name, &label, |map| remove_in_place(map, args, &indices))?;
    Ok(RemoveToolReport {
        removed,
        edit: edit_result_fragment(&report),
    })
}
