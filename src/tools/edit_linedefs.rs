//! Linedef editing tools: `add_linedef`, `edit_linedef`, `remove_linedef`,
//! `split_linedef` and `join_linedefs` (see the `edit_linedefs` router
//! methods on [`crate::server::Cyberdemon`]).
//!
//! This file keeps what the five tools share: the constants, the reported
//! [`LinedefDetail`], the flag adjustments [`EditBits`] resolves, the checks
//! every resulting linedef faces, the outcome wrapper and the router. The
//! tools' own wire parameters, report rows and one-step work live beside
//! them: [`add`] is `add_linedef` with its endpoints resolved in
//! [`endpoints`], [`edit`] is `edit_linedef` with its inline texture changes
//! handled by [`side_edit`], and [`remove`] is `remove_linedef`,
//! `split_linedef` and `join_linedefs`.
//!
//! Every call is exactly one [`crate::session::Session::edit_map`] step, so
//! one undo entry moves the whole call. The parameter shapes
//! ([`IndexSelector`], [`FlagsInput`]) and validators ([`check_index`],
//! [`validate_texture_name`], [`edit_result_fragment`]) come from
//! [`crate::tools::maps::params`]; removal runs through
//! [`crate::wad::map::remove`] with the linedef set seeded and the pruning
//! [`crate::wad::map::remove::CascadeOptions`] from the call. The split and
//! join geometry itself lives in the pure [`crate::wad::map::ops`].
//!
//! The orientation rule: walk from `v1` to `v2`, the front sidedef is on
//! your right, the back sidedef on your left. `edit_linedef`'s `flip` keeps
//! the rule: it swaps the endpoints *and* the sides, so the physical faces
//! of the line stay where they are.
//!
//! The format rules: endpoints given as points are coordinates, so binary
//! maps keep them inside `-32768..=32767` (`invalid_argument` outside);
//! `UDMF` stores full integers. `tag` exists only in the `Doom` family: a
//! non-zero tag on a `Hexen`-family map (binary `Hexen`, `UDMF` outside the
//! `doom`/`heretic`/`strife` namespace) is refused because `Hexen` specials
//! take their parameters from `args`. `args` exist only in the `Hexen`
//! family and the binary `Hexen` record stores each one as `u8`; passing
//! them on a `UDMF` map in a `Doom` namespace refuses with advice to run
//! `convert_map_format` to the `zdoom` namespace, since the map already is
//! `UDMF`. Binary `Doom` stores `special` and `tag` as `u16`, binary
//! `Hexen` `special` as `u8`.
//!
//! [`IndexSelector`]: crate::tools::maps::params::IndexSelector
//! [`check_index`]: crate::tools::maps::params::check_index
//! [`edit_result_fragment`]: crate::tools::maps::params::edit_result_fragment

mod add;
mod edit;
mod endpoints;
mod remove;
mod side_edit;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use serde::Serialize;

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::tools::edit_linedefs::add::{AddLinedefParams, add_run};
use crate::tools::edit_linedefs::edit::{EditLinedefParams, edit_run};
use crate::tools::edit_linedefs::remove::{
    JoinLinedefsParams, RemoveLinedefParams, SplitLinedefParams, join_run, remove_run, split_run,
};
use crate::tools::maps::params::{FlagsInput, validate_texture_name};
use crate::tools::reply;
use crate::wad::classify::MapFormat;
use crate::wad::map::consts::NO_TEXTURE;
use crate::wad::map::flags::{self, BLOCKING, TWO_SIDED};
use crate::wad::map::{FlagFamily, FlagKind, Linedef, Sidedef};

/// The all-zero script argument list of a `Doom`-family linedef.
const ZERO_ARGS: [i32; 5] = [0_i32; 5];
/// The lowest value any unsigned binary record column holds.
const FIELD_MIN: i32 = 0_i32;
/// The highest value a binary `Doom` `special` or `tag` column holds
/// (`u16::MAX`).
const U16_MAX: i32 = 0xFFFF_i32;
/// The highest value a binary `Hexen` `special` or `arg` column holds
/// (`u8::MAX`).
const SCRIPT_MAX: i32 = 0xFF_i32;
/// The lowest coordinate an `i16` binary `VERTEXES` column holds (`i16::MIN`).
const COORD_MIN: i32 = -0x8000_i32;
/// The highest coordinate an `i16` binary `VERTEXES` column holds (`i16::MAX`).
const COORD_MAX: i32 = 0x7FFF_i32;
/// The lowest offset an `i16` binary `SIDEDEFS` column holds (`i16::MIN`).
const OFFSET_MIN: i32 = -0x8000_i32;
/// The highest offset an `i16` binary `SIDEDEFS` column holds (`i16::MAX`).
const OFFSET_MAX: i32 = 0x7FFF_i32;

/// One linedef's fields as the tools report them, flags named for the
/// map's family.
#[derive(Debug, Serialize)]
struct LinedefDetail {
    /// Start vertex index.
    v1: usize,
    /// End vertex index.
    v2: usize,
    /// Raw flag bits as the map's format stores them.
    flags: u32,
    /// The flag names from the map's flag family.
    flag_names: Vec<&'static str>,
    /// Line special number.
    special: i32,
    /// Trigger tag (`Doom` family).
    tag: i32,
    /// The five script arguments (`Hexen` family).
    args: [i32; 5],
    /// Front-side sidedef index, `null` when absent.
    front: Option<usize>,
    /// Back-side sidedef index, `null` when absent.
    back: Option<usize>,
}

/// The flag adjustments of an [`Cyberdemon::edit_linedef`] call.
struct EditBits {
    /// Whole-replacement bits from `flags`, `None` when it was absent.
    absolute: Option<u32>,
    /// Bits from `set_flags`.
    set: u32,
    /// Bits from `clear_flags`.
    clear: u32,
    /// Whether any of the three flag parameters was given: the two-sided
    /// and blocking auto-adjustments stand down when it was.
    explicit: bool,
}

impl EditBits {
    /// Resolves the three flag parameters against `family`'s linedef table.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a name is not in the table.
    fn resolve(args: &EditLinedefParams, family: FlagFamily) -> Result<Self, CyberError> {
        let given = args.flags.clone();
        Ok(Self {
            absolute: given
                .map(|input| input.resolve(FlagKind::Linedef, family))
                .transpose()?,
            set: resolve_bits(args.set_flags.as_ref(), family)?,
            clear: resolve_bits(args.clear_flags.as_ref(), family)?,
            explicit: flags_given(args),
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

/// Whether any of the three flag parameters was given.
fn flags_given(args: &EditLinedefParams) -> bool {
    [
        args.flags.as_ref(),
        args.set_flags.as_ref(),
        args.clear_flags.as_ref(),
    ]
    .iter()
    .any(Option::is_some)
}

/// The bits of one optional flag parameter, `0` when absent.
fn resolve_bits(input: Option<&FlagsInput>, family: FlagFamily) -> Result<u32, CyberError> {
    input.cloned().map_or_else(
        || Ok(0_u32),
        |given| given.resolve(FlagKind::Linedef, family),
    )
}

/// The flags a new linedef starts with when the caller named none: the
/// one-sided default blocks, the two-sided default just is two-sided.
#[must_use]
const fn default_line_flags(two_sided: bool) -> u32 {
    if two_sided { TWO_SIDED } else { BLOCKING }
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

/// Refuses unless `value` sits inside the `i16` range of a binary
/// `VERTEXES` column.
fn fits_coord(name: &str, value: i32) -> Result<(), CyberError> {
    fits_field(name, value, COORD_MIN, COORD_MAX)
}

/// Refuses unless `value` sits inside the `i16` range of a binary
/// `SIDEDEFS` offset column.
fn fits_offset(name: &str, value: i32) -> Result<(), CyberError> {
    fits_field(name, value, OFFSET_MIN, OFFSET_MAX)
}

/// Range-checks one point against the binary `VERTEXES` record; a `UDMF`
/// map stores full integers and needs no check.
fn check_binary_coords(format: MapFormat, x: i32, y: i32) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom | MapFormat::Hexen => {
            fits_coord("x", x)?;
            fits_coord("y", y)?;
        }
        MapFormat::Udmf => {}
    }
    Ok(())
}

/// Range-checks one sidedef's offsets against the binary `SIDEDEFS`
/// record; a `UDMF` map stores full integers and needs no check.
fn check_binary_offsets(format: MapFormat, side: &Sidedef) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom | MapFormat::Hexen => {
            fits_offset("x_offset", side.x_offset)?;
            fits_offset("y_offset", side.y_offset)?;
        }
        MapFormat::Udmf => {}
    }
    Ok(())
}

/// The family rules: `tag` is `Doom` family only, `args` are `Hexen`
/// family only; the args refusal of a `UDMF` map in a `Doom` namespace
/// points at the namespace conversion, since the map already is `UDMF`.
fn check_family_fields(
    format: MapFormat,
    family: FlagFamily,
    line: &Linedef,
) -> Result<(), CyberError> {
    if family == FlagFamily::Hexen && line.tag != 0_i32 {
        return Err(CyberError::InvalidArgument(
            "Hexen-family linedefs have no tag: Hexen specials take their parameters from \
             args, not a tag"
                .to_owned(),
        ));
    }
    if family == FlagFamily::Doom && line.args != ZERO_ARGS {
        return Err(CyberError::InvalidArgument(match format {
            MapFormat::Udmf => "UDMF linedefs in a Doom namespace have no args: run \
                                convert_map_format to the UDMF zdoom namespace (Hexen family) \
                                to store them"
                .to_owned(),
            MapFormat::Doom | MapFormat::Hexen => "Doom-format linedefs have no args; use a \
                                                   Hexen or UDMF map"
                .to_owned(),
        }));
    }
    Ok(())
}

/// The binary record ranges: `Doom` stores `special`/`tag` as `u16`,
/// `Hexen` stores `special` and every `arg` as `u8`; `UDMF` is unbounded.
fn check_binary_fields(format: MapFormat, line: &Linedef) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom => {
            fits_field("special", line.special, FIELD_MIN, U16_MAX)?;
            fits_field("tag", line.tag, FIELD_MIN, U16_MAX)?;
        }
        MapFormat::Hexen => {
            fits_field("special", line.special, FIELD_MIN, SCRIPT_MAX)?;
            for value in line.args {
                fits_field("arg", value, FIELD_MIN, SCRIPT_MAX)?;
            }
        }
        MapFormat::Udmf => {}
    }
    Ok(())
}

/// Refuses a resulting linedef the map's format and family cannot store.
fn check_line(format: MapFormat, family: FlagFamily, line: &Linedef) -> Result<(), CyberError> {
    check_family_fields(format, family, line)?;
    check_binary_fields(format, line)
}

/// The reported fields of `line` in a map of `family`.
fn detail_of(line: &Linedef, family: FlagFamily) -> LinedefDetail {
    LinedefDetail {
        v1: line.v1,
        v2: line.v2,
        flags: line.flags,
        flag_names: flags::to_names(line.flags, FlagKind::Linedef, family),
        special: line.special,
        tag: line.tag,
        args: line.args,
        front: line.front,
        back: line.back,
    }
}

/// One texture parameter validated and uppercased, `NO_TEXTURE` when absent.
fn new_texture(given: Option<&String>, format: MapFormat) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(NO_TEXTURE.to_owned()),
        |name| validate_texture_name(name, format),
    )
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = edit_linedefs_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Appends one linedef to a map in a single undoable edit.
    #[tool(
        description = "Adds one linedef to a map in a single undoable edit: endpoints as existing v1/v2 indices or from/to points, a required front side (an existing sidedef or an inline side spec that creates one), an optional back side (a line with a back is two-sided), plus flags, special, tag or Hexen args. Prefer create_sector_from_loop to draw a room's whole wall loop, or build_map_from_plan to build a level; use this for hand-placed single lines."
    )]
    fn add_linedef(
        &self,
        Parameters(args): Parameters<AddLinedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Changes one or several linedefs in a single undoable edit.
    #[tool(
        description = "Changes one or several linedefs of a map in a single undoable edit: flags (replace, or set_flags/clear_flags by name), special, tag, args, endpoints v1/v2, front/back sidedefs, flip, and inline front_side/back_side texture edits. Prefer add_door, add_switch and add_exit to give lines door, trigger and exit behaviour correctly in one step, bulk_edit to sweep a field across a find_elements selection, and replace_texture to rename faces everywhere."
    )]
    fn edit_linedef(
        &self,
        Parameters(args): Parameters<EditLinedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| edit_run(session, &args)),
        )
    }

    /// Removes one or several linedefs in a single undoable edit.
    #[tool(
        description = "Removes one linedef or a list of them from a map in a single undoable edit; the remaining linedefs renumber down and surviving references follow. The cascade prunes orphaned sidedefs and vertices unless switched off, and sectors are never touched. Geometry change - run build_all_nodes before vanilla play. For a mixed selection use delete_geometry."
    )]
    fn remove_linedef(
        &self,
        Parameters(args): Parameters<RemoveLinedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }

    /// Cuts one linedef into two at a point in a single undoable edit.
    #[tool(
        description = "Cuts one linedef into two at a single point in a single undoable edit: at {x, y} must lie on the segment (within one map unit), the midpoint when omitted; the new half copies flags, special, tag, args and sides with x_offsets shifted so textures stay put on the wall. The undo counterpart of join_linedefs, and the way add_switch isolates a panel. Geometry: rebuild nodes after."
    )]
    fn split_linedef(
        &self,
        Parameters(args): Parameters<SplitLinedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| split_run(session, &args)),
        )
    }

    /// Welds two collinear linedefs into one in a single undoable edit.
    #[tool(
        description = "Welds two collinear linedefs into the first of them in a single undoable edit, removing the second and the shared vertex: the pair must meet head to tail, run the same direction, share their sector pair and match in properties unless force: true lets the first win. Offsets shift so textures hold their place. The undo counterpart of split_linedef; rebuild nodes after."
    )]
    fn join_linedefs(
        &self,
        Parameters(args): Parameters<JoinLinedefsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| join_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests;
