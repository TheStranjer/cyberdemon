//! `edit_linedef`: its arguments, the side references it takes, the rows it
//! reports and the one step that rewrites the selected linedefs.
//!
//! The [`crate::server::Cyberdemon`] router method hands its parsed
//! [`EditLinedefParams`] to [`edit_run`], which first refuses a contradictory
//! or empty request with [`validate_edit`] ([`has_change`] is the "nothing to
//! change" half) and then runs [`edit_in_place`] as one
//! [`crate::session::Session::edit_map`] step. Inside that step [`EditPlan`]
//! holds the request beside the targets and the parent's [`EditBits`],
//! [`edited_line`] computes what each selected linedef becomes (its fields
//! replaced, [`adjusted_for_back`] keeping two-sidedness honest after a `back`
//! change, a flip swapping ends and sides together, [`moves_endpoints`] and
//! [`zero_line`] refusing a collapsed one) and the parent's [`check_line`]
//! keeps the result storable. The inline `front_side`/`back_side` changes go
//! through [`apply_side_edit`] beside the [`SideTarget`] [`side_target`]
//! builds, and [`check_requested_references`] refuses references the map does
//! not hold before anything is written.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{FlagsInput, IndexSelector, check_index, edit_result_fragment};
use crate::wad::classify::MapFormat;
use crate::wad::map::{FlagFamily, Linedef, Map};

use super::side_edit::{SideEdit, apply_side_edit, side_target};
use super::{
    BLOCKING_BITS, EditBits, LinedefDetail, TWO_SIDED_BITS, check_line, detail_of, flags_given,
};

/// The value of an [`Cyberdemon::edit_linedef`] side reference: an existing
/// sidedef index, or an explicit `null` dropping the reference.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(untagged)]
pub(super) enum SideRef {
    /// Point the side at this existing sidedef index.
    Index(
        /// The sidedef index.
        usize,
    ),
    /// The explicit `null`: remove the reference (`front` refuses it).
    Clear,
}

/// `serde` helper for the two side-reference parameters: a present value
/// is wrapped in `Some` even when it is an explicit `null` (which plain
/// `Option` deserialization swallows into the outer `None`); combined with
/// `#[serde(default)]` an absent field stays `None`, meaning "unchanged".
fn present_side<'de, D>(deserializer: D) -> Result<Option<SideRef>, D::Error>
where
    D: Deserializer<'de>,
{
    SideRef::deserialize(deserializer).map(Some)
}

/// Arguments for [`Cyberdemon::edit_linedef`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct EditLinedefParams {
    /// The map to edit; defaults like [`AddLinedefParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The linedef to change: one index or a list of them (duplicates
    /// collapse); the shared changes below apply to every selected linedef.
    pub(super) index: IndexSelector,
    /// Replace the whole flag bitset: a raw number or names. Conflicts
    /// with `set_flags`/`clear_flags`.
    #[serde(default)]
    pub(super) flags: Option<FlagsInput>,
    /// Flag bits to add on top of the current flags: a raw number or
    /// names.
    #[serde(default)]
    pub(super) set_flags: Option<FlagsInput>,
    /// Flag bits to remove from the current flags: a raw number or names;
    /// `set_flags` wins when both name the same bit.
    #[serde(default)]
    pub(super) clear_flags: Option<FlagsInput>,
    /// Replace the line special number.
    #[serde(default)]
    pub(super) special: Option<i32>,
    /// Replace the trigger tag (`Doom` family only).
    #[serde(default)]
    pub(super) tag: Option<i32>,
    /// Replace the five script arguments (`Hexen` family only).
    #[serde(default)]
    pub(super) args: Option<[i32; 5]>,
    /// Replace the start vertex; it must exist.
    #[serde(default)]
    pub(super) v1: Option<usize>,
    /// Replace the end vertex; it must exist.
    #[serde(default)]
    pub(super) v2: Option<usize>,
    /// Point the front at another sidedef; it must exist. `null` is
    /// refused (use `flip` or `remove_linedef`).
    #[serde(default, deserialize_with = "present_side")]
    pub(super) front: Option<SideRef>,
    /// Point the back at another sidedef (it must exist), or `null` to
    /// remove the back reference and close the line into one side.
    #[serde(default, deserialize_with = "present_side")]
    pub(super) back: Option<SideRef>,
    /// `true` swaps `v1`/`v2` and with them `front`/`back`, keeping the
    /// front on the right of the new direction.
    #[serde(default)]
    pub(super) flip: Option<bool>,
    /// Texture and offset changes for the front side's sidedef, edited in
    /// place; the linedef must have a front side.
    #[serde(default)]
    pub(super) front_side: Option<SideEdit>,
    /// Texture and offset changes for the back side's sidedef, edited in
    /// place; the linedef must have a back side.
    #[serde(default)]
    pub(super) back_side: Option<SideEdit>,
}

/// One edited linedef: its position plus its fields.
#[derive(Debug, Serialize)]
pub(super) struct EditRow {
    /// Position in [`Map::linedefs`].
    pub(super) index: usize,
    /// The linedef's fields, inlined next to the index.
    #[serde(flatten)]
    pub(super) detail: LinedefDetail,
}

/// Structured result of [`Cyberdemon::edit_linedef`].
#[derive(Serialize)]
pub(super) struct EditReport {
    /// The edited linedefs, ascending by index.
    linedefs: Vec<EditRow>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The reported row of `line` at `index` in a map of `family`.
pub(super) fn edit_row(index: usize, line: &Linedef, family: FlagFamily) -> EditRow {
    EditRow {
        index,
        detail: detail_of(line, family),
    }
}

/// Whether the request changes at least one field (`flip` counts only when
/// it is `true`, a side edit only when it names at least one field).
pub(super) fn has_change(args: &EditLinedefParams) -> bool {
    let line_fields = flags_given(args)
        || args.special.is_some()
        || args.tag.is_some()
        || args.args.is_some()
        || args.v1.is_some()
        || args.v2.is_some()
        || args.front.is_some()
        || args.back.is_some()
        || args.flip == Some(true);
    let side_fields = [args.front_side.as_ref(), args.back_side.as_ref()]
        .into_iter()
        .flatten()
        .any(SideEdit::has_fields);
    line_fields || side_fields
}

/// Refuses an `edit_linedef` request whose parameters contradict each other
/// or change nothing, and returns its sorted target indices.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty index list, `flags`
/// together with `set_flags`/`clear_flags`, `front: null`, `back: null`
/// together with `back_side`, or no change at all.
pub(super) fn validate_edit(args: &EditLinedefParams) -> Result<Vec<usize>, CyberError> {
    let indices = args.index.clone().indices()?;
    if args.flags.is_some() && (args.set_flags.is_some() || args.clear_flags.is_some()) {
        return Err(CyberError::InvalidArgument(
            "flags conflicts with set_flags/clear_flags: replace the bitset or adjust it, not \
             both"
                .to_owned(),
        ));
    }
    if args.front == Some(SideRef::Clear) {
        return Err(CyberError::InvalidArgument(
            "a linedef must keep a front side: to turn the line around use flip: true, to drop \
             the far side use back: null, to drop the line use remove_linedef"
                .to_owned(),
        ));
    }
    if args.back == Some(SideRef::Clear)
        && args.back_side.as_ref().is_some_and(SideEdit::has_fields)
    {
        return Err(CyberError::InvalidArgument(
            "back: null removes the back side while back_side edits it: pass one of them"
                .to_owned(),
        ));
    }
    if !has_change(args) {
        return Err(CyberError::InvalidArgument(
            "nothing to change: pass at least one of flags, set_flags, clear_flags, special, \
             tag, args, v1, v2, front, back, flip: true, front_side, back_side"
                .to_owned(),
        ));
    }
    Ok(indices)
}

/// An [`Cyberdemon::edit_linedef`] call resolved against the edited map:
/// the target indices and the flag adjustments.
pub(super) struct EditPlan<'req> {
    /// The requested change parameters.
    args: &'req EditLinedefParams,
    /// The targets, sorted ascending without duplicates.
    indices: Vec<usize>,
    /// The resolved flag adjustments.
    bits: EditBits,
    /// The map's binary storage layout.
    format: MapFormat,
    /// The map's flag family.
    family: FlagFamily,
}

impl<'req> EditPlan<'req> {
    /// Resolves `args` against `map`'s family and format.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a `flags`/`set_flags`/
    /// `clear_flags` name is not in the family's linedef table.
    pub(super) fn new(
        args: &'req EditLinedefParams,
        indices: Vec<usize>,
        map: &Map,
    ) -> Result<Self, CyberError> {
        Ok(Self {
            args,
            indices,
            bits: EditBits::resolve(args, map.flag_family())?,
            format: map.format,
            family: map.flag_family(),
        })
    }

    /// Whether `index` is one of this edit's targets.
    fn holds(&self, index: usize) -> bool {
        self.indices.contains(&index)
    }
}

/// Whether this call moves the endpoints (`v1`/`v2` given or a flip).
fn moves_endpoints(args: &EditLinedefParams) -> bool {
    args.v1.is_some() || args.v2.is_some() || args.flip == Some(true)
}

/// The refusal when an endpoint change collapses the line to one vertex.
fn zero_line(index: usize, vertex: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "the edit makes linedef {index} zero-length: both endpoints would be vertex {vertex}; \
         a line needs two distinct endpoints"
    ))
}

/// The flags after the back side's reference changes: naming a sidedef
/// opens the line (`twosided` on, `blocking` off), `null` closes it
/// (`twosided` off, `blocking` on).
#[must_use]
const fn adjusted_for_back(flags: u32, target: SideRef) -> u32 {
    if matches!(target, SideRef::Clear) {
        (flags & !TWO_SIDED_BITS) | BLOCKING_BITS
    } else {
        (flags | TWO_SIDED_BITS) & !BLOCKING_BITS
    }
}

/// The linedef `current` becomes under `plan`: fields replaced, flags
/// applied, the back-side two-sidedness adjusted, the flip applied.
pub(super) fn edited_line(
    current: &Linedef,
    index: usize,
    plan: &EditPlan<'_>,
) -> Result<Linedef, CyberError> {
    let args = plan.args;
    let mut line = Linedef {
        v1: args.v1.unwrap_or(current.v1),
        v2: args.v2.unwrap_or(current.v2),
        flags: plan.bits.apply(current.flags),
        special: args.special.unwrap_or(current.special),
        tag: args.tag.unwrap_or(current.tag),
        args: args.args.unwrap_or(current.args),
        front: match args.front {
            Some(SideRef::Index(side)) => Some(side),
            Some(SideRef::Clear) | None => current.front,
        },
        back: match args.back {
            Some(SideRef::Index(side)) => Some(side),
            Some(SideRef::Clear) => None,
            None => current.back,
        },
        extra: current.extra.clone(),
    };
    if let Some(target) = args.back.filter(|_| !plan.bits.explicit) {
        line.flags = adjusted_for_back(line.flags, target);
    }
    if args.flip == Some(true) {
        core::mem::swap(&mut line.v1, &mut line.v2);
        core::mem::swap(&mut line.front, &mut line.back);
    }
    if moves_endpoints(args) && line.v1 == line.v2 {
        return Err(zero_line(index, line.v1));
    }
    check_line(plan.format, plan.family, &line)?;
    Ok(line)
}

/// `edit_linedef`'s step: check the references, rewrite every selected
/// linedef, apply its side edits and report the rows.
pub(super) fn edit_in_place(
    map: &mut Map,
    plan: &EditPlan<'_>,
) -> Result<Vec<EditRow>, CyberError> {
    for index in &plan.indices {
        check_index("linedef", *index, map.linedefs.len())?;
    }
    check_requested_references(map, plan.args)?;
    let format = map.format;
    let mut rows = Vec::new();
    for (index, slot) in map.linedefs.iter_mut().enumerate() {
        if !plan.holds(index) {
            continue;
        }
        let updated = edited_line(slot, index, plan)?;
        apply_side_edit(
            &mut map.sidedefs,
            &side_target(index, "front", updated.front, plan.args.front_side.as_ref()),
            format,
        )?;
        apply_side_edit(
            &mut map.sidedefs,
            &side_target(index, "back", updated.back, plan.args.back_side.as_ref()),
            format,
        )?;
        rows.push(edit_row(index, &updated, plan.family));
        *slot = updated;
    }
    Ok(rows)
}

/// Refuses vertex and sidedef indices the map does not hold, for every
/// reference the request replaces.
fn check_requested_references(map: &Map, args: &EditLinedefParams) -> Result<(), CyberError> {
    if let Some(vertex) = args.v1 {
        check_index("vertex", vertex, map.vertices.len())?;
    }
    if let Some(vertex) = args.v2 {
        check_index("vertex", vertex, map.vertices.len())?;
    }
    if let Some(SideRef::Index(side)) = args.front {
        check_index("sidedef", side, map.sidedefs.len())?;
    }
    if let Some(SideRef::Index(side)) = args.back {
        check_index("sidedef", side, map.sidedefs.len())?;
    }
    Ok(())
}

/// `edit_linedef`'s work: validate, resolve the map, run the one-step edit.
pub(super) fn edit_run(
    session: &mut Session,
    args: &EditLinedefParams,
) -> Result<EditReport, CyberError> {
    let indices = validate_edit(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("edit {} linedefs", indices.len());
    let (rows, report) = session.edit_map(name, &label, |map| {
        let plan = EditPlan::new(args, indices, map)?;
        edit_in_place(map, &plan)
    })?;
    Ok(EditReport {
        linedefs: rows,
        edit: edit_result_fragment(&report),
    })
}
