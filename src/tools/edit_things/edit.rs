//! `edit_thing`'s arguments, report and working steps: one request resolved
//! against the edited map and applied to every selected thing inside a
//! single [`crate::session::Session::edit_map`] step.
//!
//! The [`crate::server::Cyberdemon`] router method hands its parsed
//! [`EditThingParams`] to [`edit_run`]. That validates the request with
//! [`validate_edit`] ([`check_conflicts`] and [`has_change`]), resolves the
//! three flag parameters against the map's family into [`EditBits`] and the
//! target indices into [`EditPlan`], and runs [`edit_in_place`]: every
//! selected thing is rebuilt by [`edited_thing`] with [`new_coord`] and
//! [`new_angle`], and the result faces the same `Doom`-family and
//! binary-range checks the other two tools share, which live in the parent
//! module beside the [`EditRow`] they report.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{
    EditBits, ElementKind, FlagInputs, FlagsInput, IndexSelector, check_index, edit_result_fragment,
};
use crate::wad::map::{FlagFamily, FlagKind, Map, Thing};

use super::{DEGREES, EditRow, check_binary_ranges, check_doom_plain, edit_row};

/// Arguments for [`Cyberdemon::edit_thing`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct EditThingParams {
    /// The map to edit; defaults like [`AddThingParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The thing to change: one index or a list of them (duplicates
    /// collapse); the shared changes below apply to every selected thing.
    index: IndexSelector,
    /// Replace the thing number.
    #[serde(default)]
    #[serde(rename = "type")]
    kind: Option<i32>,
    /// Absolute `X` (single index only; use `dx` for a list).
    #[serde(default)]
    x: Option<i32>,
    /// Absolute `Y` (single index only; use `dy` for a list).
    #[serde(default)]
    y: Option<i32>,
    /// Move every selected thing by this much in `X`.
    #[serde(default)]
    dx: Option<i32>,
    /// Move every selected thing by this much in `Y`.
    #[serde(default)]
    dy: Option<i32>,
    /// Absolute facing angle in degrees, normalised into `0..=359`.
    #[serde(default)]
    angle: Option<i32>,
    /// Turn every selected thing by this many degrees, normalised.
    #[serde(default)]
    angle_delta: Option<i32>,
    /// Absolute spawn height (`Hexen`/`UDMF` only).
    #[serde(default)]
    z: Option<i32>,
    /// Replace the whole flag bitset: a raw number or names. Conflicts with
    /// `set_flags`/`clear_flags`.
    #[serde(default)]
    flags: Option<FlagsInput>,
    /// Flag bits to add on top of the current flags: a raw number or names.
    #[serde(default)]
    set_flags: Option<FlagsInput>,
    /// Flag bits to remove from the current flags: a raw number or names;
    /// `set_flags` wins when both name the same bit.
    #[serde(default)]
    clear_flags: Option<FlagsInput>,
    /// Thing id (`Hexen`/`UDMF` only).
    #[serde(default)]
    tid: Option<i32>,
    /// Thing special (`Hexen`/`UDMF` only).
    #[serde(default)]
    special: Option<i32>,
    /// The five script arguments (`Hexen`/`UDMF` only).
    #[serde(default)]
    args: Option<[i32; 5]>,
}

/// Structured result of [`Cyberdemon::edit_thing`].
#[derive(Serialize)]
pub(super) struct EditReport {
    /// The edited things, ascending by index.
    things: Vec<EditRow>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// An [`Cyberdemon::edit_thing`] call resolved against the edited map: the
/// target indices and the flag adjustments.
pub(super) struct EditPlan<'req> {
    /// The requested change parameters.
    args: &'req EditThingParams,
    /// The targets, sorted ascending without duplicates.
    indices: Vec<usize>,
    /// The resolved flag adjustments.
    bits: EditBits,
}

impl<'req> EditPlan<'req> {
    /// Resolves `args` for a map whose flags are in `family`.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a `flags`/`set_flags`/
    /// `clear_flags` name is not in the family's thing table.
    pub(super) fn new(
        args: &'req EditThingParams,
        indices: Vec<usize>,
        family: FlagFamily,
    ) -> Result<Self, CyberError> {
        Ok(Self {
            args,
            indices,
            bits: EditBits::resolve(
                FlagInputs {
                    flags: args.flags.clone(),
                    set_flags: args.set_flags.clone(),
                    clear_flags: args.clear_flags.clone(),
                },
                FlagKind::Thing,
                family,
            )?,
        })
    }

    /// Whether `index` is one of this edit's targets.
    fn holds(&self, index: usize) -> bool {
        self.indices.contains(&index)
    }
}

/// The coordinate after the edit: `absolute` wins, `delta` shifts
/// `current`, neither keeps it.
pub(super) fn new_coord(
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
                "{name} {current} plus {shift} leaves the range of map coordinates"
            ))
        })
    })
}

/// The facing angle after the edit, normalised into `0..=359`.
pub(super) fn new_angle(
    current: i32,
    absolute: Option<i32>,
    delta: Option<i32>,
) -> Result<i32, CyberError> {
    new_coord("angle", current, absolute, delta).map(|degrees| degrees.rem_euclid(DEGREES))
}

/// The thing `current` becomes under `plan`.
pub(super) fn edited_thing(current: &Thing, plan: &EditPlan<'_>) -> Result<Thing, CyberError> {
    let args = plan.args;
    Ok(Thing {
        tid: args.tid.unwrap_or(current.tid),
        x: new_coord("x", current.x, args.x, args.dx)?,
        y: new_coord("y", current.y, args.y, args.dy)?,
        z: args.z.unwrap_or(current.z),
        angle: new_angle(current.angle, args.angle, args.angle_delta)?,
        kind: args.kind.unwrap_or(current.kind),
        flags: plan.bits.apply(current.flags),
        special: args.special.unwrap_or(current.special),
        args: args.args.unwrap_or(current.args),
        extra: current.extra.clone(),
    })
}

/// `edit_thing`'s step: rewrite every selected thing and report the rows.
fn edit_in_place(map: &mut Map, plan: &EditPlan<'_>) -> Result<Vec<EditRow>, CyberError> {
    for index in &plan.indices {
        check_index(ElementKind::Things, *index, map.things.len())?;
    }
    let family = map.flag_family();
    let format = map.format;
    let mut rows = Vec::new();
    for (index, slot) in map.things.iter_mut().enumerate() {
        if !plan.holds(index) {
            continue;
        }
        let updated = edited_thing(slot, plan)?;
        check_doom_plain(family, &updated)?;
        check_binary_ranges(format, &updated)?;
        rows.push(edit_row(index, &updated, family));
        *slot = updated;
    }
    Ok(rows)
}

/// Refuses an absolute/delta pair or `flags` plus adjustments carried
/// together.
fn check_conflicts(args: &EditThingParams) -> Result<(), CyberError> {
    let moves = [
        (args.x, args.dx),
        (args.y, args.dy),
        (args.angle, args.angle_delta),
    ];
    if moves
        .into_iter()
        .any(|(absolute, delta)| absolute.is_some() && delta.is_some())
    {
        return Err(CyberError::InvalidArgument(
            "an absolute parameter conflicts with its delta: pass one of x/dx, y/dy or \
             angle/angle_delta"
                .to_owned(),
        ));
    }
    if args.flags.is_some() && (args.set_flags.is_some() || args.clear_flags.is_some()) {
        return Err(CyberError::InvalidArgument(
            "flags conflicts with set_flags/clear_flags: replace the bitset or adjust it, not \
             both"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Whether the request changes at least one field.
pub(super) fn has_change(args: &EditThingParams) -> bool {
    let scalars = [
        args.kind,
        args.x,
        args.y,
        args.dx,
        args.dy,
        args.angle,
        args.angle_delta,
        args.z,
        args.tid,
        args.special,
    ]
    .iter()
    .any(Option::is_some);
    let flag_lists = [
        args.flags.as_ref(),
        args.set_flags.as_ref(),
        args.clear_flags.as_ref(),
    ]
    .iter()
    .any(Option::is_some);
    scalars || flag_lists || args.args.is_some()
}

/// Validates an `edit_thing` request up front and returns its sorted,
/// deduplicated target indices.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty index list, an
/// absolute/delta or `flags`/`set_flags` conflict, an absolute `x`/`y`
/// asked for more than one thing, or a request that changes nothing.
pub(super) fn validate_edit(args: &EditThingParams) -> Result<Vec<usize>, CyberError> {
    let indices = args.index.indices()?;
    check_conflicts(args)?;
    if (args.x.is_some() || args.y.is_some()) && indices.len() > 1 {
        return Err(CyberError::InvalidArgument(
            "absolute x/y apply to a single thing: pass one index, or move a list with dx/dy"
                .to_owned(),
        ));
    }
    if !has_change(args) {
        return Err(CyberError::InvalidArgument(
            "nothing to change: pass at least one of type, x, y, dx, dy, angle, angle_delta, z, \
             flags, set_flags, clear_flags, tid, special, args"
                .to_owned(),
        ));
    }
    Ok(indices)
}

/// `edit_thing`'s work: validate, resolve the map, run the one-step edit.
pub(super) fn edit_run(
    session: &mut Session,
    args: &EditThingParams,
) -> Result<EditReport, CyberError> {
    let indices = validate_edit(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("edit {} things", indices.len());
    let (rows, report) = session.edit_map(name, &label, |map| {
        let plan = EditPlan::new(args, indices, map.flag_family())?;
        edit_in_place(map, &plan)
    })?;
    Ok(EditReport {
        things: rows,
        edit: edit_result_fragment(&report),
    })
}
