//! Thing editing tools: `add_thing`, `edit_thing` and `remove_thing` (see
//! the `edit_things` router methods on [`crate::server::Cyberdemon`]).
//!
//! `edit_thing`'s parameter and report types, its resolved plan and its
//! working steps live in [`edit`]: that file holds the one tool's request
//! handling beside the edit itself. This root keeps the shapes the three
//! tools share, the format rules all of them check their results against,
//! `add_thing` and `remove_thing`, and the router.
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

mod edit;

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
use crate::tools::edit_things::edit::{EditThingParams, edit_run};
use crate::tools::maps::common;
use crate::tools::maps::params::{
    ElementKind, FlagsInput, IndexSelector, check_index, edit_result_fragment,
};
use crate::tools::reply;
use crate::wad::classify::MapFormat;
use crate::wad::map::flags;
use crate::wad::map::flags::{
    THING_HEXEN_CLASS1, THING_HEXEN_CLASS2, THING_HEXEN_CLASS3, THING_HEXEN_COOP, THING_HEXEN_DM,
    THING_HEXEN_SINGLE, THING_SKILL_BITS,
};
use crate::wad::map::remove::{Removal, apply_removal};
use crate::wad::map::{FlagFamily, FlagKind, Map, Thing};

/// Degrees in a full circle; thing angles normalise into `0..=359`.
const DEGREES: i32 = 360_i32;
/// `Doom` family default thing flags: every skill in every mode
/// (`easy | medium | hard`).
const DOOM_DEFAULT_FLAGS: u32 = THING_SKILL_BITS;
/// `Hexen` family default thing flags: every skill, class and game mode
/// (`easy | medium | hard | class1 | class2 | class3 | single | coop | dm`).
const HEXEN_DEFAULT_FLAGS: u32 = THING_SKILL_BITS
    | THING_HEXEN_CLASS1
    | THING_HEXEN_CLASS2
    | THING_HEXEN_CLASS3
    | THING_HEXEN_SINGLE
    | THING_HEXEN_COOP
    | THING_HEXEN_DM;
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
            check_index(ElementKind::Things, *index, map.things.len())?;
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
        reply(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Changes one or several things in a single undoable edit.
    #[tool(
        description = "Changes one or several things of a map in a single undoable edit: type, x/y (absolute, single index) or dx/dy, angle or angle_delta, flags by bitset or names, tid, special and args; the Doom-family and binary range rules apply to the results. Prefer place_things to add many things, replace_thing_type to change one type into another across maps, and bulk_edit for a query-wide sweep. Thing edits never invalidate nodes."
    )]
    fn edit_thing(
        &self,
        Parameters(args): Parameters<EditThingParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
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
        reply(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests;
