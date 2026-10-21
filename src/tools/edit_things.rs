//! Thing editing tools: `add_thing`, `edit_thing` and `remove_thing` (see
//! the `edit_things` router methods on [`crate::server::Cyberdemon`]).
//!
//! Every call is exactly one [`crate::session::Session::edit_map`] step, so
//! one undo entry moves the whole call, and a thing edit never changes
//! geometry: `nodes_invalidated` is always `false`. The parameter shapes
//! ([`IndexSelector`], [`FlagsInput`]) and validators come from
//! [`crate::tools::maps::params`]; the flag names resolve through the map's
//! own [`FlagFamily`] thing table, and removal runs through
//! [`crate::wad::map::remove::apply_removal`] with only the thing set.
//!
//! The format rules: `Doom`-family maps (binary `Doom`, and `UDMF` in the
//! `doom`/`heretic`/`strife` namespace) store no `z`, `tid`, `special` or
//! `args` on things, so those must stay `0` there; the binary formats store
//! `type` as `u16`, `x`/`y`/`z`/`tid` as `i16` and (on `Hexen`) `special`
//! and the five `args` as `u8`, so values outside those ranges are refused
//! as `invalid_argument` before anything is written. `UDMF` has none of
//! those limits.

use alloc::collections::BTreeMap;

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
use crate::tools::maps::params::{FlagsInput, IndexSelector, check_index, edit_result_fragment};
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::flags;
use crate::wad::map::remove::{Removal, apply_removal};
use crate::wad::map::{FlagFamily, FlagKind, Map, Thing};

/// Degrees in a full circle; thing angles normalise into `0..=359`.
const DEGREES: i32 = 360_i32;
/// `Doom` family default thing flags: every skill in every mode
/// (`easy | medium | hard`).
const DOOM_DEFAULT_FLAGS: u32 = 7_u32;
/// `Hexen` family default thing flags: every skill, class and game mode
/// (`easy | medium | hard | class1 | class2 | class3 | single | coop | dm`).
const HEXEN_DEFAULT_FLAGS: u32 = 2_023_u32;
/// The all-zero script argument list of a `Doom`-format thing.
const ZERO_ARGS: [i32; 5] = [0_i32; 5];
/// The lowest value any unsigned binary record column holds.
const FIELD_MIN: i32 = 0_i32;
/// The highest value the `u16` `type` column holds (`u16::MAX`).
const TYPE_MAX: i32 = 0xFFFF_i32;
/// The lowest value an `i16` coordinate column holds (`i16::MIN`).
const COORD_MIN: i32 = -0x8000_i32;
/// The highest value an `i16` coordinate column holds (`i16::MAX`).
const COORD_MAX: i32 = 0x7FFF_i32;
/// The highest value a `Hexen` `u8` script column holds (`u8::MAX`).
const SCRIPT_MAX: i32 = 0xFF_i32;

/// Arguments for [`Cyberdemon::add_thing`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddThingParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// Thing number (`doomednum`), matched against the game's definitions.
    #[serde(rename = "type")]
    kind: i32,
    /// `X` coordinate in map units.
    x: i32,
    /// `Y` coordinate in map units.
    y: i32,
    /// Facing direction in degrees, normalised into `0..=359` (default `0`).
    #[serde(default)]
    angle: Option<i32>,
    /// Spawn height in map units (default `0`; `Hexen`/`UDMF` only).
    #[serde(default)]
    z: Option<i32>,
    /// Presence and behaviour flags: the raw bitset, or names from the map's
    /// thing flag table. Defaults to present in all skills and modes.
    #[serde(default)]
    flags: Option<FlagsInput>,
    /// Thing id for scripts (default `0`; `Hexen`/`UDMF` only).
    #[serde(default)]
    tid: Option<i32>,
    /// Thing special (default `0`; `Hexen`/`UDMF` only).
    #[serde(default)]
    special: Option<i32>,
    /// The five script arguments (default zeros; `Hexen`/`UDMF` only).
    #[serde(default)]
    args: Option<[i32; 5]>,
}

/// Arguments for [`Cyberdemon::edit_thing`].
#[derive(Debug, Deserialize, JsonSchema)]
struct EditThingParams {
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

/// Arguments for [`Cyberdemon::remove_thing`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveThingParams {
    /// The map to edit; defaults like [`AddThingParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The thing to remove: one index or a list of them.
    index: IndexSelector,
}

/// One thing's fields as the tools report them, flags named for the map's
/// family.
#[derive(Debug, Serialize)]
struct ThingDetail {
    /// Thing number (`doomednum`).
    #[serde(rename = "type")]
    kind: i32,
    /// `X` in map units.
    x: i32,
    /// `Y` in map units.
    y: i32,
    /// Spawn height in map units.
    z: i32,
    /// Facing direction in degrees, `0..=359`.
    angle: i32,
    /// Raw flag bits as the map's format stores them.
    flags: u32,
    /// The flag names from the map's flag family.
    flag_names: Vec<&'static str>,
    /// Thing id (`Hexen`/`UDMF`).
    tid: i32,
    /// Thing special (`Hexen`/`UDMF`).
    special: i32,
    /// The five script arguments (`Hexen`/`UDMF`).
    args: [i32; 5],
}

/// One edited thing: its position plus its fields.
#[derive(Debug, Serialize)]
struct EditRow {
    /// Position in [`Map::things`].
    index: usize,
    /// The thing's fields, inlined next to the index.
    #[serde(flatten)]
    detail: ThingDetail,
}

/// Structured result of [`Cyberdemon::add_thing`].
#[derive(Serialize)]
struct AddReport {
    /// Index of the created thing.
    index: usize,
    /// The created thing's fields.
    thing: ThingDetail,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::edit_thing`].
#[derive(Serialize)]
struct EditReport {
    /// The edited things, ascending by index.
    things: Vec<EditRow>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::remove_thing`].
#[derive(Serialize)]
struct RemoveReport {
    /// The removed thing indices (original numbering, sorted).
    removed: Vec<usize>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The flag bits a thing starts with when the caller named none: all
/// skills in all modes, per the map's family.
#[must_use]
const fn default_bits(family: FlagFamily) -> u32 {
    match family {
        FlagFamily::Doom => DOOM_DEFAULT_FLAGS,
        FlagFamily::Hexen => HEXEN_DEFAULT_FLAGS,
    }
}

/// The error refusing `Hexen`-only fields on a `Doom`-family map.
fn doom_field_error() -> CyberError {
    CyberError::InvalidArgument(
        "Doom-format things have no tid/z/special/args; use a Hexen or UDMF map".to_owned(),
    )
}

/// Whether `spawn` leaves the `Doom`-format columns at zero.
fn is_doom_plain(spawn: &Thing) -> bool {
    spawn.z == 0_i32 && spawn.tid == 0_i32 && spawn.special == 0_i32 && spawn.args == ZERO_ARGS
}

/// Refuses a nonzero `z`, `tid`, `special` or `args` on a `Doom`-family
/// map: binary `Doom` blocks and `UDMF` maps in the `doom`, `heretic` or
/// `strife` namespace have no columns for them.
pub(in crate::tools) fn check_doom_plain(
    family: FlagFamily,
    spawn: &Thing,
) -> Result<(), CyberError> {
    if family == FlagFamily::Doom && !is_doom_plain(spawn) {
        return Err(doom_field_error());
    }
    Ok(())
}

/// Refuses unless `value` sits inside the binary record field's
/// `min..=max` range.
fn fits_field(name: &str, value: i32, min: i32, max: i32) -> Result<(), CyberError> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{name} {value} is out of range for the binary map format: the field holds {min}..={max}"
    )))
}

/// The `Hexen`-only `special` and `args` of a binary `Hexen` record fit
/// `u8`.
fn check_hexen_script(spawn: &Thing) -> Result<(), CyberError> {
    fits_field("special", spawn.special, FIELD_MIN, SCRIPT_MAX)?;
    for value in spawn.args {
        fits_field("arg", value, FIELD_MIN, SCRIPT_MAX)?;
    }
    Ok(())
}

/// Range-checks one thing against the binary `THINGS` record layouts; a
/// `UDMF` map stores full integers and needs none of it.
pub(in crate::tools) fn check_binary_ranges(
    format: MapFormat,
    spawn: &Thing,
) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom | MapFormat::Hexen => {
            fits_field("type", spawn.kind, FIELD_MIN, TYPE_MAX)?;
            fits_field("x", spawn.x, COORD_MIN, COORD_MAX)?;
            fits_field("y", spawn.y, COORD_MIN, COORD_MAX)?;
            fits_field("z", spawn.z, COORD_MIN, COORD_MAX)?;
            fits_field("tid", spawn.tid, COORD_MIN, COORD_MAX)?;
            if format == MapFormat::Hexen {
                check_hexen_script(spawn)?;
            }
            Ok(())
        }
        MapFormat::Udmf => Ok(()),
    }
}

/// The reported row of `spawn` at `index` in a map of `family`.
fn edit_row(index: usize, spawn: &Thing, family: FlagFamily) -> EditRow {
    EditRow {
        index,
        detail: ThingDetail {
            kind: spawn.kind,
            x: spawn.x,
            y: spawn.y,
            z: spawn.z,
            angle: spawn.angle,
            flags: spawn.flags,
            flag_names: flags::to_names(spawn.flags, FlagKind::Thing, family),
            tid: spawn.tid,
            special: spawn.special,
            args: spawn.args,
        },
    }
}

/// The thing the `add_thing` parameters describe, `family` deciding the
/// default flags and the name resolution.
fn new_thing(args: &AddThingParams, family: FlagFamily) -> Result<Thing, CyberError> {
    let given = args.flags.clone();
    Ok(Thing {
        tid: args.tid.unwrap_or_default(),
        x: args.x,
        y: args.y,
        z: args.z.unwrap_or_default(),
        angle: args.angle.unwrap_or_default().rem_euclid(DEGREES),
        kind: args.kind,
        flags: given.map_or_else(
            || Ok(default_bits(family)),
            |input| input.resolve(FlagKind::Thing, family),
        )?,
        special: args.special.unwrap_or_default(),
        args: args.args.unwrap_or(ZERO_ARGS),
        extra: BTreeMap::new(),
    })
}

/// `add_thing`'s step: append the described thing and report its row.
fn add_in_place(map: &mut Map, args: &AddThingParams) -> Result<EditRow, CyberError> {
    let family = map.flag_family();
    let format = map.format;
    let spawn = new_thing(args, family)?;
    check_doom_plain(family, &spawn)?;
    check_binary_ranges(format, &spawn)?;
    let index = map.things.len();
    map.things.push(spawn.clone());
    Ok(edit_row(index, &spawn, family))
}

/// The resolved flag adjustments of an [`Cyberdemon::edit_thing`] call.
struct EditBits {
    /// Whole-replacement bits from `flags`, `None` when it was absent.
    absolute: Option<u32>,
    /// Bits from `set_flags`.
    set: u32,
    /// Bits from `clear_flags`.
    clear: u32,
}

impl EditBits {
    /// Resolves the three flag parameters against `family`'s thing table.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a name is not in the table.
    fn resolve(args: &EditThingParams, family: FlagFamily) -> Result<Self, CyberError> {
        let given = args.flags.clone();
        Ok(Self {
            absolute: given
                .map(|input| input.resolve(FlagKind::Thing, family))
                .transpose()?,
            set: resolve_bits(args.set_flags.as_ref(), family)?,
            clear: resolve_bits(args.clear_flags.as_ref(), family)?,
        })
    }

    /// The flags after the edit: an absolute bitset replaces everything;
    /// otherwise the `clear` bits go and the `set` bits go on (a bit named
    /// for both ends up set).
    #[must_use]
    const fn apply(&self, current: u32) -> u32 {
        match self.absolute {
            Some(bits) => bits,
            None => (current & !self.clear) | self.set,
        }
    }
}

/// The bits of one optional flag parameter, `0` when absent.
fn resolve_bits(input: Option<&FlagsInput>, family: FlagFamily) -> Result<u32, CyberError> {
    input
        .cloned()
        .map_or_else(|| Ok(0_u32), |given| given.resolve(FlagKind::Thing, family))
}

/// An [`Cyberdemon::edit_thing`] call resolved against the edited map: the
/// target indices and the flag adjustments.
struct EditPlan<'req> {
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
    fn new(
        args: &'req EditThingParams,
        indices: Vec<usize>,
        family: FlagFamily,
    ) -> Result<Self, CyberError> {
        Ok(Self {
            args,
            indices,
            bits: EditBits::resolve(args, family)?,
        })
    }

    /// Whether `index` is one of this edit's targets.
    fn holds(&self, index: usize) -> bool {
        self.indices.contains(&index)
    }
}

/// The coordinate after the edit: `absolute` wins, `delta` shifts
/// `current`, neither keeps it.
fn new_coord(
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
fn new_angle(current: i32, absolute: Option<i32>, delta: Option<i32>) -> Result<i32, CyberError> {
    new_coord("angle", current, absolute, delta).map(|degrees| degrees.rem_euclid(DEGREES))
}

/// The thing `current` becomes under `plan`.
fn edited_thing(current: &Thing, plan: &EditPlan<'_>) -> Result<Thing, CyberError> {
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
        check_index("thing", *index, map.things.len())?;
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
fn has_change(args: &EditThingParams) -> bool {
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
fn validate_edit(args: &EditThingParams) -> Result<Vec<usize>, CyberError> {
    let indices = args.index.clone().indices()?;
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

/// `add_thing`'s work: resolve the map, run the one-step append.
fn add_run(session: &mut Session, args: &AddThingParams) -> Result<AddReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("add thing {}", args.kind);
    let (row, report) = session.edit_map(name, &label, |map| add_in_place(map, args))?;
    Ok(AddReport {
        index: row.index,
        thing: row.detail,
        edit: edit_result_fragment(&report),
    })
}

/// `edit_thing`'s work: validate, resolve the map, run the one-step edit.
fn edit_run(session: &mut Session, args: &EditThingParams) -> Result<EditReport, CyberError> {
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

/// `remove_thing`'s work: resolve the map and run the removal with only
/// the thing set.
///
/// # Errors
///
/// [`IndexSelector::indices`]' and [`check_index`]' refusals (an index the
/// map does not hold is `not_found`), [`apply_removal`]'s errors and
/// whatever [`crate::session::Session::edit_map`] reports.
fn remove_run(session: &mut Session, args: &RemoveThingParams) -> Result<RemoveReport, CyberError> {
    let indices = args.index.clone().indices()?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("remove {} things", indices.len());
    let (removed, report) = session.edit_map(name, &label, |map| {
        for index in &indices {
            check_index("thing", *index, map.things.len())?;
        }
        let removal = Removal {
            things: indices.iter().copied().collect(),
            ..Default::default()
        };
        Ok(apply_removal(map, &removal)?.things)
    })?;
    Ok(RemoveReport {
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
#[tool_router(router = edit_things_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Appends one thing to a map in a single undoable edit.
    #[tool(
        description = "Adds one thing to a map in a single undoable edit: type (a doomednum - get_thing_types lists the game's numbers and names), x, y and optional angle, flags, and the Hexen-family tid, special and args. Prefer place_things to distribute monsters and items across sectors or areas in one call, or build_map_from_plan whose thing rows stand a whole population in with the map."
    )]
    fn add_thing(
        &self,
        Parameters(args): Parameters<AddThingParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Changes one or several things in a single undoable edit.
    #[tool(
        description = "Changes one or several things of a map in a single undoable edit: type, x/y (absolute, single index) or dx/dy, angle or angle_delta, flags by bitset or names, tid, special and args; the Doom-family and binary range rules apply to the results. Prefer place_things to add many things, replace_thing_type to change one type into another across maps, and bulk_edit for a query-wide sweep. Thing edits never invalidate nodes."
    )]
    fn edit_thing(
        &self,
        Parameters(args): Parameters<EditThingParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| edit_run(session, &args)),
        )
    }

    /// Removes one or several things in a single undoable edit.
    #[tool(
        description = "Removes one thing or a list of them from a map in a single undoable edit; the remaining things renumber down. Removing things never cascades into other element kinds and never invalidates the node lumps. An index the map lacks is not_found. To clear a whole set, take the indices from find_elements or use delete_geometry for a mixed selection."
    )]
    fn remove_thing(
        &self,
        Parameters(args): Parameters<RemoveThingParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::{
        AddThingParams, COORD_MAX, COORD_MIN, DEGREES, EditBits, EditPlan, EditThingParams,
        TYPE_MAX, add_in_place, check_binary_ranges, check_doom_plain, default_bits, edited_thing,
        fits_field, has_change, new_angle, new_coord, new_thing, validate_edit,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{FlagFamily, Map, Thing};
    use crate::wad::name::LumpName;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// An edit request parsed from `json`.
    fn edit_args(json: &str) -> EditThingParams {
        from_str(json).expect("test edit requests parse")
    }

    /// An add request parsed from `json`.
    fn add_args(json: &str) -> AddThingParams {
        from_str(json).expect("test add requests parse")
    }

    /// A thing with no `Hexen`-only fields for the range and family checks.
    fn plain(x: i32, y: i32, kind: i32) -> Thing {
        Thing {
            x,
            y,
            kind,
            ..Default::default()
        }
    }

    #[test]
    fn the_default_flags_are_the_all_present_bits_per_family() {
        assert_eq!(default_bits(FlagFamily::Doom), 7_u32, "easy|medium|hard");
        assert_eq!(
            default_bits(FlagFamily::Hexen),
            2_023_u32,
            "0x7E7: skills, classes and modes"
        );
    }

    #[test]
    fn angles_normalise_and_coordinates_shift() {
        assert_eq!(
            new_angle(0_i32, Some(450_i32), None).expect("450 is nameable"),
            90_i32,
            "the turn lands in 0..=359"
        );
        assert_eq!(
            new_angle(350_i32, Some(-90_i32), None).expect("-90 is nameable"),
            270_i32,
            "rem_euclid normalises negatives"
        );
        assert_eq!(
            new_angle(90_i32, None, Some(405_i32)).expect("a full turn plus a quarter"),
            135_i32,
            "deltas normalise too"
        );
        assert_eq!(
            new_coord("x", 32_000_i32, None, Some(1_000_i32)).expect("the sum fits"),
            33_000_i32,
            "a delta shifts the coordinate"
        );
        assert_eq!(DEGREES, 360_i32, "the full turn");
    }

    #[test]
    fn coordinate_overflow_is_refused_and_absolute_wins() {
        let error = new_coord("y", i32::MAX, None, Some(1_i32)).expect_err("above i32 refuses");
        assert_eq!(code(&error), "invalid_argument", "overflow code");
        assert!(
            error.to_string().contains('y'),
            "the field is named: {error}"
        );
        assert_eq!(
            new_coord("x", 10_i32, Some(-5_i32), None).expect("absolute wins"),
            -5_i32,
            "an absolute value passes through untouched"
        );
    }

    #[test]
    fn doom_family_things_stay_plain() {
        let with_tid = Thing {
            tid: 5_i32,
            ..plain(0_i32, 0_i32, 1_i32)
        };
        let error =
            check_doom_plain(FlagFamily::Doom, &with_tid).expect_err("a tid on a Doom map refuses");
        assert_eq!(code(&error), "invalid_argument", "family rule code");
        assert!(
            error
                .to_string()
                .contains("Doom-format things have no tid/z/special/args; use a Hexen or UDMF map"),
            "the documented message: {error}"
        );
        let extras = [
            Thing {
                z: 8_i32,
                ..plain(0_i32, 0_i32, 1_i32)
            },
            Thing {
                special: 1_i32,
                ..plain(0_i32, 0_i32, 1_i32)
            },
            Thing {
                args: [0_i32, 0_i32, 0_i32, 0_i32, 1_i32],
                ..plain(0_i32, 0_i32, 1_i32)
            },
        ];
        for extra in extras {
            assert!(
                check_doom_plain(FlagFamily::Doom, &extra).is_err(),
                "z/special/args refuse too"
            );
        }
        assert!(
            check_doom_plain(FlagFamily::Doom, &plain(0_i32, 0_i32, 1_i32)).is_ok(),
            "all zeros are plain"
        );
        let wild = Thing {
            tid: 9_i32,
            z: 64_i32,
            special: 2_i32,
            args: [1_i32, 2_i32, 3_i32, 4_i32, 5_i32],
            ..plain(0_i32, 0_i32, 1_i32)
        };
        assert!(
            check_doom_plain(FlagFamily::Hexen, &wild).is_ok(),
            "the Hexen family stores them all"
        );
    }

    #[test]
    fn binary_ranges_are_checked_and_udmf_is_unbounded() {
        let error = fits_field("type", 70_000_i32, 0_i32, TYPE_MAX).expect_err("above u16 refuses");
        assert_eq!(code(&error), "invalid_argument", "range code");
        assert!(
            error.to_string().contains("0..=65535"),
            "the message shows the field range: {error}"
        );
        assert!(
            check_binary_ranges(MapFormat::Doom, &plain(COORD_MAX, COORD_MIN, TYPE_MAX)).is_ok(),
            "the i16 edges fit"
        );
        let over = plain(
            COORD_MAX.checked_add(1).expect("edge plus one"),
            0_i32,
            1_i32,
        );
        assert!(
            check_binary_ranges(MapFormat::Doom, &over).is_err(),
            "x above i16 refuses on binary Doom"
        );
        let big_special = Thing {
            special: TYPE_MAX,
            ..plain(0_i32, 0_i32, 1_i32)
        };
        assert!(
            check_binary_ranges(MapFormat::Hexen, &big_special).is_err(),
            "a Hexen special above u8 refuses"
        );
        let big_arg = Thing {
            args: [0_i32, 0_i32, 300_i32, 0_i32, 0_i32],
            ..plain(0_i32, 0_i32, 1_i32)
        };
        assert!(
            check_binary_ranges(MapFormat::Hexen, &big_arg).is_err(),
            "a Hexen arg above u8 refuses"
        );
        assert!(
            check_binary_ranges(MapFormat::Udmf, &plain(i32::MAX, i32::MIN, i32::MAX)).is_ok(),
            "UDMF stores full integers"
        );
        let doom_special = Thing {
            special: 300_i32,
            ..plain(0_i32, 0_i32, 1_i32)
        };
        assert!(
            check_binary_ranges(MapFormat::Doom, &doom_special).is_ok(),
            "the u8 script checks are Hexen-only (the family rule zeroes these earlier)"
        );
    }

    #[test]
    fn new_things_take_defaults_and_normalised_angles() {
        let args = add_args(r#"{"type": 3001, "x": 8, "y": -8, "angle": 450}"#);
        let doom = new_thing(&args, FlagFamily::Doom).expect("a plain Doom thing");
        assert_eq!(
            (doom.kind, doom.x, doom.y, doom.angle),
            (3_001_i32, 8_i32, -8_i32, 90_i32),
            "the required fields and the normalised angle"
        );
        assert_eq!(doom.flags, 7_u32, "the Doom default");
        assert_eq!(doom.args, [0_i32; 5], "and zeros elsewhere");
        let named = add_args(
            r#"{"type": 1, "x": 0, "y": 0, "flags": ["ambush", "class1"], "z": 8, "tid": 3,
                "special": 1, "args": [1, 2, 3, 4, 5]}"#,
        );
        let hexen = new_thing(&named, FlagFamily::Hexen).expect("named flags");
        assert_eq!(hexen.flags, 8_u32 | 32_u32, "ambush|class1 from the table");
        assert_eq!(hexen.tid, 3_i32, "the parameters pass through");
        let plain_args = add_args(r#"{"type": 1, "x": 0, "y": 0}"#);
        let spawned = new_thing(&plain_args, FlagFamily::Hexen).expect("defaults resolve");
        assert_eq!(spawned.flags, 2_023_u32, "the Hexen default is 0x7E7");
    }

    #[test]
    fn edits_replace_or_adjust_the_flag_bits() {
        let adjusting = EditBits {
            absolute: None,
            set: 8_u32,
            clear: 1_u32,
        };
        assert_eq!(adjusting.apply(7_u32), 14_u32, "easy clears, ambush sets");
        let replacing = EditBits {
            absolute: Some(3_u32),
            set: 8_u32,
            clear: 1_u32,
        };
        assert_eq!(replacing.apply(7_u32), 3_u32, "the absolute bitset wins");
    }

    #[test]
    fn edited_things_combine_unchanged_fields_with_the_request() {
        let current = Thing {
            tid: 2_i32,
            x: 100_i32,
            y: -100_i32,
            z: 8_i32,
            angle: 90_i32,
            kind: 3_001_i32,
            flags: 7_u32,
            special: 0_i32,
            args: [0_i32; 5],
            extra: alloc::collections::BTreeMap::new(),
        };
        let args = edit_args(r#"{"index": 0, "dx": -30, "angle_delta": 45}"#);
        let plan = EditPlan::new(&args, vec![0_usize], FlagFamily::Hexen).expect("a flagless plan");
        let edited = edited_thing(&current, &plan).expect("the edit computes");
        assert_eq!((edited.x, edited.y), (70_i32, -100_i32), "dx moves x only");
        assert_eq!(edited.angle, 135_i32, "the turn stays in 0..=359");
        assert_eq!(edited.kind, 3_001_i32, "unasked fields stand");
        assert_eq!(edited.tid, 2_i32, "and the tid survives");
    }

    #[test]
    fn edit_validation_names_conflicts_and_nothings() {
        let sorted = validate_edit(&edit_args(r#"{"index": [3, 1], "dx": 8, "dy": -8}"#))
            .expect("a list with deltas is fine");
        assert_eq!(sorted, vec![1_usize, 3], "sorted and deduplicated");
        let refusals = [
            r#"{"index": 0, "x": 1, "dx": 2}"#,
            r#"{"index": 0, "y": 1, "dy": 2}"#,
            r#"{"index": 0, "angle": 1, "angle_delta": 2}"#,
            r#"{"index": 0, "flags": 3, "set_flags": ["easy"]}"#,
            r#"{"index": 0, "flags": 3, "clear_flags": ["easy"]}"#,
            r#"{"index": [0, 1], "x": 4}"#,
            r#"{"index": [0, 1], "y": 4}"#,
            r#"{"index": 0}"#,
            r#"{"index": []}"#,
        ];
        for json in refusals {
            let error = validate_edit(&edit_args(json)).expect_err("the request must refuse");
            assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
        }
        assert!(
            has_change(&edit_args(r#"{"index": 0, "tid": 0}"#)),
            "tid counts"
        );
        assert!(
            has_change(&edit_args(r#"{"index": 0, "args": [0, 0, 0, 0, 0]}"#)),
            "args count"
        );
    }

    #[test]
    fn an_added_thing_never_touches_the_geometry() {
        let mut map = Map::new_empty(
            LumpName::parse("MAP01").expect("a valid name"),
            MapFormat::Doom,
        );
        map.things.push(plain(0_i32, 0_i32, 1_i32));
        let args = add_args(r#"{"type": 3004, "x": 16, "y": 16}"#);
        let row = add_in_place(&mut map, &args).expect("the thing appends");
        assert_eq!(row.index, 1_usize, "after the existing one");
        assert_eq!(map.things.len(), 2, "and the list grew by one");
        assert!(map.vertices.is_empty(), "geometry stayed untouched");
    }

    #[test]
    fn a_doom_namespace_udmf_map_takes_the_plain_rule_without_binary_ranges() {
        let mut map = Map::new_empty(
            LumpName::parse("MAP01").expect("a valid name"),
            MapFormat::Udmf,
        );
        map.udmf_namespace = Some("doom".to_owned());
        let wide = add_args(r#"{"type": 70000, "x": 40000, "y": 0}"#);
        let row = add_in_place(&mut map, &wide).expect("UDMF stores wide values");
        assert_eq!(row.detail.kind, 70_000_i32, "no u16 cap in TEXTMAP");
        assert_eq!(
            row.detail.flags, 7_u32,
            "the doom namespace is the Doom family"
        );
        let tidied = add_args(r#"{"type": 1, "x": 0, "y": 0, "tid": 3}"#);
        let error = add_in_place(&mut map, &tidied).expect_err("the tid refuses there too");
        assert_eq!(code(&error), "invalid_argument", "namespace family code");
        assert_eq!(map.things.len(), 1, "and nothing appended");
    }
}
