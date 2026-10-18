//! Linedef editing tools: `add_linedef`, `edit_linedef`, `remove_linedef`,
//! `split_linedef` and `join_linedefs` (see the `edit_linedefs` router
//! methods on [`crate::server::Cyberdemon`]).
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
//! family and the binary `Hexen` record stores each one as `u8`. Binary
//! `Doom` stores `special` and `tag` as `u16`, binary `Hexen` `special` as
//! `u8`.

use alloc::collections::BTreeMap;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{
    FlagsInput, IndexSelector, check_index, edit_result_fragment, validate_texture_name,
};
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::flags;
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, expand_cascade,
};
use crate::wad::map::{FlagFamily, FlagKind, Linedef, Map, Sidedef, Vertex, ops};

/// The `blocking` linedef flag bit: the default flags of a one-sided line.
const BLOCKING_BITS: u32 = 1_u32;
/// The `twosided` linedef flag bit: the default flags of a two-sided line.
const TWO_SIDED_BITS: u32 = 4_u32;
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
/// The name of an empty texture field, `"-"` for no texture.
const NO_TEXTURE: &str = "-";

/// Arguments for [`Cyberdemon::add_linedef`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddLinedefParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// Start vertex index (with `v2`, one of the two endpoint forms).
    #[serde(default)]
    v1: Option<usize>,
    /// End vertex index (with `v1`).
    #[serde(default)]
    v2: Option<usize>,
    /// Start point `{x, y}` (with `to`, the other endpoint form; reuses a
    /// vertex already at exactly these coordinates, else creates one).
    #[serde(default)]
    from: Option<PointParam>,
    /// End point `{x, y}` (with `from`).
    #[serde(default)]
    to: Option<PointParam>,
    /// Front-side sidedef index (with `front_side`, one of the two front
    /// forms; one of them is required).
    #[serde(default)]
    front: Option<usize>,
    /// Front side to create: `{sector, upper?, middle?, lower?, x_offset?,
    /// y_offset?}` (with `front`).
    #[serde(default)]
    front_side: Option<NewSide>,
    /// Back-side sidedef index (with `back_side`, at most one form; no back
    /// keeps the line one-sided).
    #[serde(default)]
    back: Option<usize>,
    /// Back side to create (with `back`).
    #[serde(default)]
    back_side: Option<NewSide>,
    /// Flags: the raw bitset or names from the map's linedef flag table.
    /// Defaults to `blocking` (1) for a one-sided line, `twosided` (4)
    /// when a back side is given.
    #[serde(default)]
    flags: Option<FlagsInput>,
    /// Line special number (default `0`).
    #[serde(default)]
    special: Option<i32>,
    /// Trigger tag (default `0`; `Doom` family only).
    #[serde(default)]
    tag: Option<i32>,
    /// The five script arguments (default zeros; `Hexen` family only).
    #[serde(default)]
    args: Option<[i32; 5]>,
}

/// The value of an [`Cyberdemon::edit_linedef`] side reference: an existing
/// sidedef index, or an explicit `null` dropping the reference.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(untagged)]
enum SideRef {
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
struct EditLinedefParams {
    /// The map to edit; defaults like [`AddLinedefParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The linedef to change: one index or a list of them (duplicates
    /// collapse); the shared changes below apply to every selected linedef.
    index: IndexSelector,
    /// Replace the whole flag bitset: a raw number or names. Conflicts
    /// with `set_flags`/`clear_flags`.
    #[serde(default)]
    flags: Option<FlagsInput>,
    /// Flag bits to add on top of the current flags: a raw number or
    /// names.
    #[serde(default)]
    set_flags: Option<FlagsInput>,
    /// Flag bits to remove from the current flags: a raw number or names;
    /// `set_flags` wins when both name the same bit.
    #[serde(default)]
    clear_flags: Option<FlagsInput>,
    /// Replace the line special number.
    #[serde(default)]
    special: Option<i32>,
    /// Replace the trigger tag (`Doom` family only).
    #[serde(default)]
    tag: Option<i32>,
    /// Replace the five script arguments (`Hexen` family only).
    #[serde(default)]
    args: Option<[i32; 5]>,
    /// Replace the start vertex; it must exist.
    #[serde(default)]
    v1: Option<usize>,
    /// Replace the end vertex; it must exist.
    #[serde(default)]
    v2: Option<usize>,
    /// Point the front at another sidedef; it must exist. `null` is
    /// refused (use `flip` or `remove_linedef`).
    #[serde(default, deserialize_with = "present_side")]
    front: Option<SideRef>,
    /// Point the back at another sidedef (it must exist), or `null` to
    /// remove the back reference and close the line into one side.
    #[serde(default, deserialize_with = "present_side")]
    back: Option<SideRef>,
    /// `true` swaps `v1`/`v2` and with them `front`/`back`, keeping the
    /// front on the right of the new direction.
    #[serde(default)]
    flip: Option<bool>,
    /// Texture and offset changes for the front side's sidedef, edited in
    /// place; the linedef must have a front side.
    #[serde(default)]
    front_side: Option<SideEdit>,
    /// Texture and offset changes for the back side's sidedef, edited in
    /// place; the linedef must have a back side.
    #[serde(default)]
    back_side: Option<SideEdit>,
}

/// Arguments for [`Cyberdemon::remove_linedef`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveLinedefParams {
    /// The map to edit; defaults like [`AddLinedefParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The linedef to remove: one index or a list of them.
    index: IndexSelector,
    /// Within the cascade, also remove sidedefs no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    prune_sidedefs: Option<bool>,
    /// Within the cascade, also remove vertices no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    prune_vertices: Option<bool>,
}

/// An `X`/`Y` point parameter, one endpoint of the `from`/`to` form.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
struct PointParam {
    /// `X` coordinate in map units.
    x: i32,
    /// `Y` coordinate in map units.
    y: i32,
}

/// Arguments for [`Cyberdemon::split_linedef`].
#[derive(Debug, Deserialize, JsonSchema)]
struct SplitLinedefParams {
    /// The map to edit; defaults like [`AddLinedefParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The linedef to cut.
    index: usize,
    /// The cut point `{x, y}`: on the segment (within one map unit of the
    /// line, strictly between the endpoints). Defaults to the midpoint,
    /// rounded to nearest.
    #[serde(default)]
    at: Option<PointParam>,
}

/// Arguments for [`Cyberdemon::join_linedefs`].
#[derive(Debug, Deserialize, JsonSchema)]
struct JoinLinedefsParams {
    /// The map to edit; defaults like [`AddLinedefParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The linedef that survives the join (its properties win under `force`).
    first: usize,
    /// The linedef merged into the first and removed.
    second: usize,
    /// Join even though the two lines' properties or side textures differ,
    /// keeping the first linedef's (default `false`).
    #[serde(default)]
    force: Option<bool>,
}

/// A sidedef to create with a new linedef, mirroring `add_sidedef`.
#[derive(Debug, Deserialize, JsonSchema)]
struct NewSide {
    /// Index of the sector this side opens into; it must exist.
    sector: usize,
    /// Upper texture name (default `"-"`, no texture).
    #[serde(default)]
    upper: Option<String>,
    /// Middle texture name (default `"-"`, no texture).
    #[serde(default)]
    middle: Option<String>,
    /// Lower texture name (default `"-"`, no texture).
    #[serde(default)]
    lower: Option<String>,
    /// Texture shift along the line in map units (default `0`).
    #[serde(default)]
    x_offset: Option<i32>,
    /// Texture shift across the line in map units (default `0`).
    #[serde(default)]
    y_offset: Option<i32>,
}

/// Texture and offset changes for the sidedef on one side of a linedef;
/// fields left out stay as they are.
#[derive(Debug, Deserialize, JsonSchema)]
struct SideEdit {
    /// Replace the upper texture name.
    #[serde(default)]
    upper: Option<String>,
    /// Replace the middle texture name.
    #[serde(default)]
    middle: Option<String>,
    /// Replace the lower texture name.
    #[serde(default)]
    lower: Option<String>,
    /// Absolute texture shift along the line.
    #[serde(default)]
    x_offset: Option<i32>,
    /// Absolute texture shift across the line.
    #[serde(default)]
    y_offset: Option<i32>,
}

impl SideEdit {
    /// Whether this edit asks for at least one field.
    fn has_fields(&self) -> bool {
        let textures = [
            self.upper.as_ref(),
            self.middle.as_ref(),
            self.lower.as_ref(),
        ]
        .iter()
        .any(Option::is_some);
        let offsets = [self.x_offset, self.y_offset].iter().any(Option::is_some);
        textures || offsets
    }
}

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

/// One edited linedef: its position plus its fields.
#[derive(Debug, Serialize)]
struct EditRow {
    /// Position in [`Map::linedefs`].
    index: usize,
    /// The linedef's fields, inlined next to the index.
    #[serde(flatten)]
    detail: LinedefDetail,
}

/// The result of `add_linedef`'s step: the new row plus what the call
/// appended to get there.
#[derive(Debug)]
struct AddRow {
    /// Position in [`Map::linedefs`] after the append.
    index: usize,
    /// The created linedef's fields.
    detail: LinedefDetail,
    /// Indices of the vertices this call appended, ascending.
    created_vertices: Vec<usize>,
    /// Indices of the sidedefs this call appended, ascending.
    created_sidedefs: Vec<usize>,
}

/// Structured result of [`Cyberdemon::add_linedef`].
#[derive(Serialize)]
struct AddReport {
    /// Index of the created linedef.
    index: usize,
    /// The created linedef's fields.
    linedef: LinedefDetail,
    /// Vertices created for the `from`/`to` endpoints.
    created_vertices: Vec<usize>,
    /// Sidedefs created from `front_side`/`back_side`.
    created_sidedefs: Vec<usize>,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::edit_linedef`].
#[derive(Serialize)]
struct EditReport {
    /// The edited linedefs, ascending by index.
    linedefs: Vec<EditRow>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::remove_linedef`].
#[derive(Serialize)]
struct RemoveToolReport {
    /// What the removal engine dropped, flipped and normalised.
    removed: RemovalReport,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::split_linedef`].
#[derive(Serialize)]
struct SplitToolReport {
    /// What the split engine built, inlined next to the edit fragment.
    #[serde(flatten)]
    result: ops::SplitResult,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::join_linedefs`].
#[derive(Serialize)]
struct JoinToolReport {
    /// What the join engine welded and removed, inlined next to the edit
    /// fragment.
    #[serde(flatten)]
    result: ops::JoinResult,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The two endpoint forms of an [`Cyberdemon::add_linedef`] call, checked
/// to be exactly one of them.
#[derive(Debug)]
enum EndpointSpec<'req> {
    /// Existing vertex indices, as given.
    Indices {
        /// The start vertex.
        v1: usize,
        /// The end vertex.
        v2: usize,
    },
    /// Endpoints as points: existing vertices are reused, missing ones
    /// are created.
    Points {
        /// The start point.
        from: &'req PointParam,
        /// The end point.
        to: &'req PointParam,
    },
}

/// The endpoints after resolving them against the map.
struct Ends {
    /// The start vertex index.
    v1: usize,
    /// The end vertex index.
    v2: usize,
    /// Indices of the vertices this call appended, ascending.
    created: Vec<usize>,
}

/// The sides after resolving them against the map.
struct Sides {
    /// The front sidedef index.
    front: usize,
    /// The back sidedef index, `None` for a one-sided line.
    back: Option<usize>,
    /// Indices of the sidedefs this call appended, ascending.
    created: Vec<usize>,
}

/// One side of an [`Cyberdemon::edit_linedef`]: the linedef it belongs to,
/// the side reference to edit and the changes asked for.
struct SideTarget<'req> {
    /// Index of the linedef whose side this is (for messages).
    line: usize,
    /// Which side, for messages (`"front"` or `"back"`).
    which: &'static str,
    /// The side's sidedef index after the edit, `None` when absent.
    side: Option<usize>,
    /// The requested field changes, `None` when this side was not named.
    edit: Option<&'req SideEdit>,
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
    if two_sided {
        TWO_SIDED_BITS
    } else {
        BLOCKING_BITS
    }
}

/// The index of the first vertex sitting exactly on `x`/`y`.
fn find_vertex(map: &Map, x: i32, y: i32) -> Option<usize> {
    map.vertices
        .iter()
        .position(|vertex| vertex.x == x && vertex.y == y)
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
/// family only.
fn check_family_fields(family: FlagFamily, line: &Linedef) -> Result<(), CyberError> {
    if family == FlagFamily::Hexen && line.tag != 0_i32 {
        return Err(CyberError::InvalidArgument(
            "Hexen-family linedefs have no tag: Hexen specials take their parameters from \
             args, not a tag"
                .to_owned(),
        ));
    }
    if family == FlagFamily::Doom && line.args != ZERO_ARGS {
        return Err(CyberError::InvalidArgument(
            "Doom-format linedefs have no args; use a Hexen or UDMF map".to_owned(),
        ));
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
    check_family_fields(family, line)?;
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

/// The reported row of `line` at `index` in a map of `family`.
fn edit_row(index: usize, line: &Linedef, family: FlagFamily) -> EditRow {
    EditRow {
        index,
        detail: detail_of(line, family),
    }
}

/// One texture parameter validated and uppercased, `NO_TEXTURE` when absent.
fn new_texture(given: Option<&String>, format: MapFormat) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(NO_TEXTURE.to_owned()),
        |name| validate_texture_name(name, format),
    )
}

/// The texture after a side edit: the validated new name, or `current`.
fn edited_texture(
    current: &str,
    given: Option<&String>,
    format: MapFormat,
) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(current.to_owned()),
        |name| validate_texture_name(name, format),
    )
}

/// The sidedef the `front_side`/`back_side` parameter describes for a map
/// of `format`.
fn new_sidedef(side: &NewSide, format: MapFormat) -> Result<Sidedef, CyberError> {
    Ok(Sidedef {
        x_offset: side.x_offset.unwrap_or_default(),
        y_offset: side.y_offset.unwrap_or_default(),
        upper: new_texture(side.upper.as_ref(), format)?,
        lower: new_texture(side.lower.as_ref(), format)?,
        middle: new_texture(side.middle.as_ref(), format)?,
        sector: side.sector,
        extra: BTreeMap::new(),
    })
}

/// The sidedef `current` becomes under the side edit: its textures and
/// offsets change, its sector and `UDMF` extras stand.
fn side_edited(
    current: &Sidedef,
    edit: &SideEdit,
    format: MapFormat,
) -> Result<Sidedef, CyberError> {
    Ok(Sidedef {
        x_offset: edit.x_offset.unwrap_or(current.x_offset),
        y_offset: edit.y_offset.unwrap_or(current.y_offset),
        upper: edited_texture(&current.upper, edit.upper.as_ref(), format)?,
        lower: edited_texture(&current.lower, edit.lower.as_ref(), format)?,
        middle: edited_texture(&current.middle, edit.middle.as_ref(), format)?,
        sector: current.sector,
        extra: current.extra.clone(),
    })
}

/// The refusal for endpoints given in both forms or in neither.
fn endpoint_refusal() -> CyberError {
    CyberError::InvalidArgument(
        "add_linedef needs its two endpoints in exactly one form: pass v1 and v2 (existing \
         vertex indices) or from and to (points {x, y})"
            .to_owned(),
    )
}

/// The add call's endpoint form: exactly one of `v1`/`v2` or `from`/`to`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when both forms or neither is given,
/// or when one form is incomplete.
fn endpoint_spec(args: &AddLinedefParams) -> Result<EndpointSpec<'_>, CyberError> {
    let points = args.from.is_some() || args.to.is_some();
    match (args.v1, args.v2) {
        (Some(_), Some(_)) if points => Err(endpoint_refusal()),
        (Some(v1), Some(v2)) => Ok(EndpointSpec::Indices { v1, v2 }),
        (None, None) => match (args.from.as_ref(), args.to.as_ref()) {
            (Some(from), Some(to)) => Ok(EndpointSpec::Points { from, to }),
            _ => Err(endpoint_refusal()),
        },
        _ => Err(endpoint_refusal()),
    }
}

/// Refuses a side named by both its index parameter and its create
/// parameter.
fn check_side_pairs(args: &AddLinedefParams) -> Result<(), CyberError> {
    if args.front.is_some() && args.front_side.is_some() {
        return Err(CyberError::InvalidArgument(
            "front conflicts with front_side: reference an existing sidedef or create a new \
             one, not both"
                .to_owned(),
        ));
    }
    if args.back.is_some() && args.back_side.is_some() {
        return Err(CyberError::InvalidArgument(
            "back conflicts with back_side: reference an existing sidedef or create a new \
             one, not both"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The refusal for a call without any front side.
fn front_required() -> CyberError {
    CyberError::InvalidArgument(
        "a linedef needs a front side: pass front (an existing sidedef index) or front_side \
         {sector, ...} (create one)"
            .to_owned(),
    )
}

/// Validates an `add_linedef` request up front: the endpoint form, the
/// side forms and the required front.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a wrong or missing endpoint form,
/// a side named twice, or no front side.
fn validate_add(args: &AddLinedefParams) -> Result<EndpointSpec<'_>, CyberError> {
    let spec = endpoint_spec(args)?;
    check_side_pairs(args)?;
    if args.front.is_none() && args.front_side.is_none() {
        return Err(front_required());
    }
    Ok(spec)
}

/// The vertex `point` names for `map`: an existing one at exactly these
/// coordinates, else an appended one recorded in `created`.
fn place_point(
    map: &mut Map,
    point: PointParam,
    created: &mut Vec<usize>,
) -> Result<usize, CyberError> {
    check_binary_coords(map.format, point.x, point.y)?;
    find_vertex(map, point.x, point.y).map_or_else(
        || {
            let index = map.vertices.len();
            map.vertices.push(Vertex {
                x: point.x,
                y: point.y,
                extra: BTreeMap::new(),
            });
            created.push(index);
            Ok(index)
        },
        Ok,
    )
}

/// The `invalid_argument` for a pair of endpoints on one point.
fn zero_endpoint(v1: usize, v2: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "the two endpoints are one point (vertex {v1} = vertex {v2}): a linedef needs two \
         distinct endpoints; move one or name another vertex"
    ))
}

/// The resolved endpoints of an `add_linedef`: checked existing indices, or
/// points reused and created against `map`.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a vertex index the map does not hold,
/// [`CyberError::InvalidArgument`] for one endpoint or a coordinate above
/// the binary `i16` cap.
fn place_endpoints(map: &mut Map, spec: &EndpointSpec<'_>) -> Result<Ends, CyberError> {
    let mut ends = match *spec {
        EndpointSpec::Indices { v1, v2 } => {
            check_index("vertex", v1, map.vertices.len())?;
            check_index("vertex", v2, map.vertices.len())?;
            Ends {
                v1,
                v2,
                created: Vec::new(),
            }
        }
        EndpointSpec::Points { from, to } => {
            let mut created = Vec::new();
            let v1 = place_point(map, *from, &mut created)?;
            let v2 = place_point(map, *to, &mut created)?;
            Ends { v1, v2, created }
        }
    };
    if ends.v1 == ends.v2 {
        return Err(zero_endpoint(ends.v1, ends.v2));
    }
    ends.created.sort_unstable();
    Ok(ends)
}

/// The `already_exists` naming the linedef that runs between the two
/// vertices in either direction.
fn duplicate_line(index: usize, v1: usize, v2: usize) -> CyberError {
    CyberError::AlreadyExists(format!(
        "linedef {index} already runs between vertices {v1} and {v2} (in either direction): \
         edit it with edit_linedef or move an endpoint with move_vertex"
    ))
}

/// Refuses when a linedef between `v1` and `v2` already exists, either way
/// around.
fn check_duplicate(map: &Map, v1: usize, v2: usize) -> Result<(), CyberError> {
    map.linedefs
        .iter()
        .position(|line| (line.v1 == v1 && line.v2 == v2) || (line.v1 == v2 && line.v2 == v1))
        .map_or(Ok(()), |index| Err(duplicate_line(index, v1, v2)))
}

/// Resolves one side parameter pair: the referenced sidedef must exist;
/// the created one gets its sector checked, its textures validated and its
/// offsets range-checked, then it is appended and recorded in `created`.
fn place_side(
    map: &mut Map,
    reference: Option<usize>,
    side: Option<&NewSide>,
    created: &mut Vec<usize>,
) -> Result<Option<usize>, CyberError> {
    if let Some(existing) = reference {
        check_index("sidedef", existing, map.sidedefs.len())?;
        return Ok(Some(existing));
    }
    let Some(new) = side else {
        return Ok(None);
    };
    check_index("sector", new.sector, map.sectors.len())?;
    let created_side = new_sidedef(new, map.format)?;
    check_binary_offsets(map.format, &created_side)?;
    let index = map.sidedefs.len();
    map.sidedefs.push(created_side);
    created.push(index);
    Ok(Some(index))
}

/// The resolved sides of an `add_linedef`, front required.
///
/// # Errors
///
/// [`CyberError::NotFound`] for a sidedef or sector index the map does not
/// hold, [`CyberError::InvalidArgument`] for an out-of-range texture name,
/// offset or missing front side.
fn resolve_sides(map: &mut Map, args: &AddLinedefParams) -> Result<Sides, CyberError> {
    let mut created = Vec::new();
    let front = place_side(map, args.front, args.front_side.as_ref(), &mut created)?;
    let back = place_side(map, args.back, args.back_side.as_ref(), &mut created)?;
    let Some(front_index) = front else {
        return Err(front_required());
    };
    created.sort_unstable();
    Ok(Sides {
        front: front_index,
        back,
        created,
    })
}

/// The linedef the `add_linedef` call describes over the resolved `ends`
/// and `sides`, checked against the map's family and format ranges.
fn build_line(
    map: &Map,
    args: &AddLinedefParams,
    ends: &Ends,
    sides: &Sides,
) -> Result<Linedef, CyberError> {
    let family = map.flag_family();
    let given = args.flags.clone();
    let line = Linedef {
        v1: ends.v1,
        v2: ends.v2,
        flags: given.map_or_else(
            || Ok(default_line_flags(sides.back.is_some())),
            |input| input.resolve(FlagKind::Linedef, family),
        )?,
        special: args.special.unwrap_or_default(),
        tag: args.tag.unwrap_or_default(),
        args: args.args.unwrap_or(ZERO_ARGS),
        front: Some(sides.front),
        back: sides.back,
        extra: BTreeMap::new(),
    };
    check_line(map.format, family, &line)?;
    Ok(line)
}

/// `add_linedef`'s step: resolve endpoints and sides, refuse a duplicate,
/// append the linedef and report its row.
fn add_in_place(
    map: &mut Map,
    args: &AddLinedefParams,
    spec: &EndpointSpec<'_>,
) -> Result<AddRow, CyberError> {
    let ends = place_endpoints(map, spec)?;
    check_duplicate(map, ends.v1, ends.v2)?;
    let sides = resolve_sides(map, args)?;
    let line = build_line(map, args, &ends, &sides)?;
    let index = map.linedefs.len();
    let detail = edit_row(index, &line, map.flag_family());
    map.linedefs.push(line);
    Ok(AddRow {
        index: detail.index,
        detail: detail.detail,
        created_vertices: ends.created,
        created_sidedefs: sides.created,
    })
}

/// The `add_linedef` history label for the chosen endpoint form.
fn add_label(spec: &EndpointSpec<'_>) -> String {
    match *spec {
        EndpointSpec::Indices { v1, v2 } => format!("add linedef from vertex {v1} to {v2}"),
        EndpointSpec::Points { from, to } => format!(
            "add linedef from {}, {} to {}, {}",
            from.x, from.y, to.x, to.y
        ),
    }
}

/// `add_linedef`'s work: validate, resolve the map, run the one-step add.
fn add_run(session: &mut Session, args: &AddLinedefParams) -> Result<AddReport, CyberError> {
    let spec = validate_add(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = add_label(&spec);
    let (row, report) = session.edit_map(name, &label, |map| add_in_place(map, args, &spec))?;
    Ok(AddReport {
        index: row.index,
        linedef: row.detail,
        created_vertices: row.created_vertices,
        created_sidedefs: row.created_sidedefs,
        edit: edit_result_fragment(&report),
    })
}

/// Whether the request changes at least one field (`flip` counts only when
/// it is `true`, a side edit only when it names at least one field).
fn has_change(args: &EditLinedefParams) -> bool {
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
fn validate_edit(args: &EditLinedefParams) -> Result<Vec<usize>, CyberError> {
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
struct EditPlan<'req> {
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
    fn new(
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
fn edited_line(
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

/// The refusal when a side edit names a side the linedef does not have.
fn no_side(index: usize, which: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "linedef {index} has no {which} side to edit: create one (add_linedef with a back side, \
         or edit_linedef with back) or drop the {which}_side parameter"
    ))
}

/// Applies one side edit to the sidedef `target` names, refusing when the
/// side is absent. `sides` is [`Map::sidedefs`] on its own so the linedef
/// iteration can hold its own field borrow.
fn apply_side_edit(
    sides: &mut [Sidedef],
    target: &SideTarget<'_>,
    format: MapFormat,
) -> Result<(), CyberError> {
    let Some(edit) = target.edit.filter(|side| side.has_fields()) else {
        return Ok(());
    };
    let Some(side) = target.side else {
        return Err(no_side(target.line, target.which));
    };
    let slot = sides
        .get_mut(side)
        .ok_or_else(|| CyberError::NotFound(format!("sidedef {side} does not exist")))?;
    let updated = side_edited(slot, edit, format)?;
    check_binary_offsets(format, &updated)?;
    *slot = updated;
    Ok(())
}

/// The one side-reference edit for linedef `index`, resolved against the
/// updated line.
const fn side_target<'req>(
    index: usize,
    which: &'static str,
    side: Option<usize>,
    edit: Option<&'req SideEdit>,
) -> SideTarget<'req> {
    SideTarget {
        line: index,
        which,
        side,
        edit,
    }
}

/// `edit_linedef`'s step: check the references, rewrite every selected
/// linedef, apply its side edits and report the rows.
fn edit_in_place(map: &mut Map, plan: &EditPlan<'_>) -> Result<Vec<EditRow>, CyberError> {
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
fn edit_run(session: &mut Session, args: &EditLinedefParams) -> Result<EditReport, CyberError> {
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

/// The pruning flags of a `remove_linedef` call, the cascading default
/// when either is absent.
fn cascade_options(args: &RemoveLinedefParams) -> CascadeOptions {
    CascadeOptions {
        prune_sidedefs: args.prune_sidedefs.unwrap_or(true),
        prune_vertices: args.prune_vertices.unwrap_or(true),
    }
}

/// `remove_linedef`'s step: check the indices and run the cascade-grown
/// removal.
fn remove_in_place(
    map: &mut Map,
    args: &RemoveLinedefParams,
    indices: &[usize],
) -> Result<RemovalReport, CyberError> {
    for index in indices {
        check_index("linedef", *index, map.linedefs.len())?;
    }
    let removal = Removal {
        linedefs: indices.iter().copied().collect(),
        ..Default::default()
    };
    let grown = expand_cascade(map, &removal, cascade_options(args));
    apply_removal(map, &grown)
}

/// `remove_linedef`'s work: resolve the map and run the removal.
fn remove_run(
    session: &mut Session,
    args: &RemoveLinedefParams,
) -> Result<RemoveToolReport, CyberError> {
    let indices = args.index.clone().indices()?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("remove {} linedefs", indices.len());
    let (removed, report) =
        session.edit_map(name, &label, |map| remove_in_place(map, args, &indices))?;
    Ok(RemoveToolReport {
        removed,
        edit: edit_result_fragment(&report),
    })
}

/// `split_linedef`'s work: resolve the map and run the one-step split.
fn split_run(
    session: &mut Session,
    args: &SplitLinedefParams,
) -> Result<SplitToolReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("split linedef {}", args.index);
    let (result, report) = session.edit_map(name, &label, |map| {
        ops::split_linedef(
            map,
            args.index,
            args.at.map(|at| ops::Point { x: at.x, y: at.y }),
        )
    })?;
    Ok(SplitToolReport {
        result,
        edit: edit_result_fragment(&report),
    })
}

/// `join_linedefs`' work: resolve the map and run the one-step join.
fn join_run(
    session: &mut Session,
    args: &JoinLinedefsParams,
) -> Result<JoinToolReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("join linedefs {} and {}", args.first, args.second);
    let (result, report) = session.edit_map(name, &label, |map| {
        ops::join_linedefs(map, args.first, args.second, args.force.unwrap_or(false))
    })?;
    Ok(JoinToolReport {
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
#[tool_router(router = edit_linedefs_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Appends one linedef to a map in a single undoable edit.
    #[tool(
        description = "Adds one linedef to a map in a single undoable edit. map defaults to the active map (set_active_map, else the merged view's only map). Orientation: walk from v1 to v2, the front sidedef is on your right (the Doom right side), the back sidedef on your left. Endpoints, exactly one form: v1 and v2 (existing vertex indices) or from and to (points {x, y}; each point reuses the vertex already sitting at exactly those coordinates, else a vertex is created and reported in created_vertices). Front side, required, at most one form: front (an existing sidedef index) or front_side {sector, upper?, middle?, lower?, x_offset?, y_offset?} which creates a sidedef; the same two forms for the optional back side: back or back_side - a line with a back is two-sided. Both forms of one side, no front side at all, an incomplete endpoint pair or both endpoint forms are invalid_argument. Optional: flags as a raw bitset or names from the map's linedef flag table (default: blocking (1) for a one-sided line, twosided (4) when a back side is given), special (default 0), tag (default 0) and args, five integers (default zeros). Family and format rules: a non-zero tag on a Hexen-family map (binary Hexen, UDMF outside the doom/heretic/strife namespace) is invalid_argument - Hexen specials take args, not a tag; non-zero args on a Doom-family map are invalid_argument; binary maps store points as i16 and special/tag (Doom) as u16, Hexen special/args as u8 - out of range is invalid_argument, UDMF is unbounded. v1 == v2 (or from and to naming one point) is invalid_argument; a vertex, sidedef or sector index the map does not hold is not_found; when a linedef between the same two vertices already exists in either direction the call refuses already_exists naming it. Adding a linedef is geometry: nodes_invalidated true (run build_nodes before playing the map in vanilla Doom). Returns {index, linedef: {v1, v2, flags, flag_names, special, tag, args, front, back}, created_vertices, created_sidedefs, map, nodes_invalidated, copied_to_working}. See skill/reference/linedefs.md."
    )]
    fn add_linedef(
        &self,
        Parameters(args): Parameters<AddLinedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Changes one or several linedefs in a single undoable edit.
    #[tool(
        description = "Changes one or several linedefs of a map in a single undoable edit. index is one linedef index or a list of them (duplicates collapse, sorted first); every change below applies to each selected linedef. flags replaces the whole bitset (raw bits or names from the map's linedef flag table); set_flags/clear_flags adjust the current bits (a bit named in both ends up set); flags together with either is invalid_argument. special, tag, args (the add_linedef family and format rules apply to the results), v1/v2 (existing vertex indices; a result with v1 == v2 is invalid_argument) and front/back (existing sidedef indices) replace fields. front: null is invalid_argument (turn the line with flip, close it with back: null, or remove it); back: null removes the back reference. flip: true swaps v1/v2 and with them front/back, so the front stays on the right of the new v1 to v2 direction and both physical faces keep their sidedefs. Setting back also sets the twosided bit and clears blocking; clearing it (back: null) also clears twosided and sets blocking - unless you pass flags/set_flags/clear_flags, which leave the bits to you. front_side/back_side {upper?, middle?, lower?, x_offset?, y_offset?} edit the referenced sidedefs' textures and offsets in place, seen from the linedef (after flip: on what became the front); naming fields for a side the linedef does not have is invalid_argument, as is back: null together with back_side fields. A call that changes nothing is invalid_argument. Endpoint and side-reference changes are geometry (nodes_invalidated true); flags, special, tag, args and texture/offset edits keep the nodes. Returns {linedefs: [{index, v1, v2, flags, flag_names, special, tag, args, front, back}], map, nodes_invalidated, copied_to_working}. A linedef, vertex or sidedef index the map does not hold is not_found and nothing changes. See skill/reference/linedefs.md."
    )]
    fn edit_linedef(
        &self,
        Parameters(args): Parameters<EditLinedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| edit_run(session, &args)),
        )
    }

    /// Removes one or several linedefs in a single undoable edit.
    #[tool(
        description = "Removes one linedef or a list of them (index: one integer or an array) from a map in a single undoable edit; the remaining linedefs renumber down and the surviving references follow. The removal always runs the cascade, and the two pruning switches say how far it reaches: sidedefs no surviving linedef references are removed with the line (prune_sidedefs, default true) and vertices no surviving linedef ends at are removed too (prune_vertices, default true) - switch a pruning off to leave those orphans in the block. Sectors are never touched by removing a linedef. Removing linedefs is geometry: the node lumps are invalidated (nodes_invalidated true; run build_nodes before playing the map in vanilla Doom; that also strips trailing vertices no linedef references). Returns {removed: {linedefs, sidedefs, vertices, sectors, things (removed original indices, sorted), flipped_linedefs, one_sided_linedefs}, map, nodes_invalidated, copied_to_working}. An index the map does not hold is not_found; an empty index list is invalid_argument. See skill/reference/linedefs.md."
    )]
    fn remove_linedef(
        &self,
        Parameters(args): Parameters<RemoveLinedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }

    /// Cuts one linedef into two at a point in a single undoable edit.
    #[tool(
        description = "Cuts one linedef into two at a single point in a single undoable edit; the undo counterpart of join_linedefs. map defaults to the active map (set_active_map, else the merged view's only map); index is the linedef to cut. Optional at is the cut point {x, y}: it must lie on the segment (within one map unit of the line) and strictly between the endpoints, else invalid_argument; leave it out for the midpoint of the endpoints (each coordinate rounded to nearest). The vertex at the cut point is reused when one sits at exactly those coordinates, else a new one is created. The original linedef keeps its start and now ends at the cut point; a new linedef is appended to the block running from the cut point to the old end, copying the original's flags, special, tag, args and extra fields. Each existing side gets a copy for the new line with its x_offset shifted so the texture continues where it was (fronts measure from v1, backs from v2): the front copy shifts by the first piece's length and the surviving original back shifts by the second piece's, the new back copy keeps the old offset. Lengths are integer hypots of the coordinate deltas (floored). Binary maps store the cut point and every resulting x_offset as i16: a point or offset outside is invalid_argument; UDMF is unbounded. Cutting is geometry: the node lumps are invalidated (nodes_invalidated true; run build_nodes before playing the map in vanilla Doom). Returns {vertex: the cut point's index, new_linedef: the appended half's index, new_sidedefs: the copies' indices (front first, then back), map, nodes_invalidated, copied_to_working}. A missing linedef or vertex, or a side pointing at a missing sidedef, is not_found; the refusal changes nothing. See skill/reference/linedefs.md."
    )]
    fn split_linedef(
        &self,
        Parameters(args): Parameters<SplitLinedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| split_run(session, &args)),
        )
    }

    /// Welds two collinear linedefs into one in a single undoable edit.
    #[tool(
        description = "Welds two collinear linedefs into the first of them in a single undoable edit, removing the second and the vertex they shared; the undo counterpart of split_linedef. map defaults to the active map; first is the surviving linedef, second the one merged away. The pair must meet head to tail - first.v2 equals second.v1, or second.v2 equals first.v1 (then the second lines up as the head) - and sharing no vertex, both vertices, or only tails-to-tails is invalid_argument naming the fix. They must lie on one straight line (collinear: invalid_argument otherwise) and run in the same direction (flip one with edit_linedef's flip: true). Their front sides must open onto the same sector and their back sides too (both having none counts), else invalid_argument. The shared vertex must belong to no third linedef - conflict names the blocker. flags, special, tag, args and the front/back side textures must match, else conflict listing the differences; with force: true the first linedef's properties win. The survivor spans its free end through the shared vertex to the other free end, keeping its own sides; the side anchored at the absorbed vertex has its x_offset shifted back by the removed line's length so the texture stays put on the wall. The second linedef, its orphaned sidedefs and the shared vertex leave through the removal engine, the block renumbers and references follow (a survivor past the removed line shifts down; the reported linedef is the survivor's new index). Joining is geometry: the node lumps are invalidated (nodes_invalidated true; run build_nodes before playing the map in vanilla Doom). Returns {linedef: the survivor's new index, removed: {linedefs, sidedefs, vertices, sectors, things, flipped_linedefs, one_sided_linedefs}, map, nodes_invalidated, copied_to_working}. A missing linedef or vertex, or a side pointing at a missing sidedef, is not_found; the refusal changes nothing. See skill/reference/linedefs.md."
    )]
    fn join_linedefs(
        &self,
        Parameters(args): Parameters<JoinLinedefsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| join_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::{
        AddLinedefParams, EditLinedefParams, EndpointSpec, RemoveLinedefParams, add_in_place,
        check_binary_fields, check_family_fields, check_line, default_line_flags, edit_in_place,
        edited_line, endpoint_spec, has_change, new_sidedef, place_endpoints, side_edited,
        validate_add, validate_edit,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{FlagFamily, Linedef, Map, Sector, Sidedef, Vertex};
    use crate::wad::name::LumpName;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// An add request parsed from `json`.
    fn add_args(json: &str) -> AddLinedefParams {
        from_str(json).expect("test add requests parse")
    }

    /// An edit request parsed from `json`.
    fn edit_args(json: &str) -> EditLinedefParams {
        from_str(json).expect("test edit requests parse")
    }

    /// A remove request parsed from `json`.
    fn remove_args(json: &str) -> RemoveLinedefParams {
        from_str(json).expect("test remove requests parse")
    }

    /// A valid parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A blocking one-sided line from `v1` to `v2` over sidedef `front`.
    fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-sided step line from `v1` to `v2` over sidedefs `front`/`back`.
    fn step_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// A sidedef with the given textures over `sector`.
    fn side(upper: &str, middle: &str, lower: &str, sector: usize) -> Sidedef {
        Sidedef {
            upper: upper.to_owned(),
            middle: middle.to_owned(),
            lower: lower.to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// The two-room map the fixtures build: six vertices (`0,0` to
    /// `512,256`), seven lines (`2` the two-sided step from vertex 4 to
    /// vertex 1), eight sides over two sectors.
    fn fixture(format: MapFormat) -> Map {
        Map {
            name: name("MAP01"),
            format,
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(512, 0),
                vertex(512, 256),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                solid_line(0, 5, 0),
                solid_line(5, 4, 1),
                step_line(4, 1, 2, 3),
                solid_line(1, 0, 4),
                solid_line(4, 3, 5),
                solid_line(3, 2, 6),
                solid_line(2, 1, 7),
            ],
            sidedefs: vec![
                side("-", "WALL1", "-", 0),
                side("-", "WALL1", "-", 0),
                side("-", "STEP1", "-", 0),
                side("-", "-", "-", 1),
                side("-", "WALL1", "-", 0),
                side("-", "WALL1", "-", 1),
                side("-", "WALL1", "-", 1),
                side("-", "WALL1", "-", 1),
            ],
            sectors: vec![Sector::default(), Sector::default()],
            ..Default::default()
        }
    }

    /// The resolved endpoint form of a parsed add request.
    fn spec_of(args: &AddLinedefParams) -> EndpointSpec<'_> {
        endpoint_spec(args).expect("test endpoint forms resolve")
    }

    #[test]
    fn the_endpoint_forms_are_exactly_one_of_indices_or_points() {
        let indices = add_args(r#"{"v1": 0, "v2": 5, "front": 0}"#);
        assert!(
            matches!(spec_of(&indices), EndpointSpec::Indices { v1: 0, v2: 5 }),
            "the index form reads through"
        );
        let points = add_args(r#"{"from": {"x": 0, "y": 0}, "to": {"x": 8, "y": 8}, "front": 0}"#);
        assert!(
            matches!(spec_of(&points), EndpointSpec::Points { .. }),
            "the point form reads through"
        );
        let refusals = [
            r#"{"front": 0}"#,
            r#"{"v1": 0, "front": 0}"#,
            r#"{"v1": 0, "v2": 1, "from": {"x": 0, "y": 0}, "front": 0}"#,
            r#"{"from": {"x": 0, "y": 0}, "to": {"x": 1, "y": 1}, "v2": 1, "front": 0}"#,
        ];
        for json in refusals {
            let error = endpoint_spec(&add_args(json)).expect_err("the request must refuse");
            assert_eq!(code(&error), "invalid_argument", "endpoint code for {json}");
        }
    }

    #[test]
    fn add_validation_reads_side_forms_and_the_required_front() {
        let both = add_args(
            r#"{"v1": 0, "v2": 1, "front_side": {"sector": 0}, "back_side": {"sector": 1}}"#,
        );
        let good = validate_add(&both).expect("both sides created");
        assert!(
            matches!(good, EndpointSpec::Indices { v1: 0, v2: 1 }),
            "validation hands the spec through"
        );
        let double_front = r#"{"v1": 0, "v2": 1, "front": 0, "front_side": {"sector": 0}}"#;
        let double_back =
            r#"{"v1": 0, "v2": 1, "front": 0, "back": 1, "back_side": {"sector": 1}}"#;
        let no_front = r#"{"v1": 0, "v2": 1}"#;
        let only_back = r#"{"v1": 0, "v2": 1, "back": 3}"#;
        for json in [double_front, double_back, no_front, only_back] {
            let error = validate_add(&add_args(json)).expect_err("the request must refuse");
            assert_eq!(code(&error), "invalid_argument", "side code for {json}");
        }
    }

    #[test]
    fn the_flag_defaults_follow_one_or_two_sidedness() {
        assert_eq!(default_line_flags(false), 1_u32, "one-sided blocks");
        assert_eq!(default_line_flags(true), 4_u32, "two-sided just is");
    }

    #[test]
    fn family_rules_move_tag_and_args_apart() {
        let tagged = Linedef {
            tag: 5_i32,
            ..solid_line(0, 1, 0)
        };
        assert!(
            check_family_fields(FlagFamily::Doom, &tagged).is_ok(),
            "the Doom family stores tags"
        );
        let error = check_family_fields(FlagFamily::Hexen, &tagged).expect_err("Hexen has no tag");
        assert_eq!(code(&error), "invalid_argument", "family rule code");
        assert!(
            error.to_string().contains("have no tag"),
            "the message names the field: {error}"
        );
        let arged = Linedef {
            args: [0_i32, 0_i32, 3_i32, 0_i32, 0_i32],
            ..solid_line(0, 1, 0)
        };
        assert!(
            check_family_fields(FlagFamily::Hexen, &arged).is_ok(),
            "the Hexen family stores args"
        );
        assert!(
            check_family_fields(FlagFamily::Doom, &arged).is_err(),
            "the Doom family refuses them"
        );
        assert!(
            check_family_fields(FlagFamily::Doom, &solid_line(0, 1, 0)).is_ok(),
            "zeros are plain"
        );
    }

    #[test]
    fn binary_record_ranges_are_checked_and_udmf_is_unbounded() {
        let wide = Linedef {
            special: 300_i32,
            tag: 70_000_i32,
            ..solid_line(0, 1, 0)
        };
        assert!(
            check_binary_fields(MapFormat::Doom, &wide).is_err(),
            "a tag above u16 refuses on binary Doom"
        );
        let big_special = Linedef {
            special: 70_000_i32,
            ..solid_line(0, 1, 0)
        };
        assert!(
            check_binary_fields(MapFormat::Doom, &big_special).is_err(),
            "so does a special above u16"
        );
        let hexen = Linedef {
            special: 300_i32,
            ..solid_line(0, 1, 0)
        };
        assert!(
            check_binary_fields(MapFormat::Hexen, &hexen).is_err(),
            "a Hexen special above u8 refuses"
        );
        let big_arg = Linedef {
            args: [0_i32, 0_i32, 0_i32, 256_i32, 0_i32],
            ..solid_line(0, 1, 0)
        };
        assert!(
            check_binary_fields(MapFormat::Hexen, &big_arg).is_err(),
            "a Hexen arg above u8 refuses"
        );
        assert!(
            check_binary_fields(MapFormat::Udmf, &big_arg).is_ok(),
            "UDMF stores full integers"
        );
        assert!(
            check_binary_fields(MapFormat::Doom, &solid_line(0, 1, 0)).is_ok(),
            "the vanilla zeros fit everywhere"
        );
    }

    #[test]
    fn a_new_linedef_appends_reusing_vertices_and_creating_sides() {
        let mut map = fixture(MapFormat::Doom);
        let args = add_args(
            r#"{"from": {"x": 0, "y": 0}, "to": {"x": 256, "y": 128},
                "front_side": {"sector": 1, "middle": "brick01", "x_offset": 8},
                "back": 3}"#,
        );
        let spec = spec_of(&args);
        let row = add_in_place(&mut map, &args, &spec).expect("the line lands");
        assert_eq!(row.index, 7, "after the existing seven");
        assert_eq!(
            row.created_vertices,
            vec![6_usize],
            "only the far point is new"
        );
        assert_eq!(
            row.created_sidedefs,
            vec![8_usize],
            "the front side was created"
        );
        assert_eq!(row.detail.front, Some(8), "and it fronts the line");
        assert_eq!(row.detail.back, Some(3), "the referenced back stands");
        assert_eq!(
            row.detail.flag_names,
            vec!["twosided"],
            "a back makes it two-sided"
        );
        assert_eq!(
            row.detail.args, [0_i32; 5],
            "the Doom family keeps zero args"
        );
        let created = map.vertices.get(6).expect("the new vertex");
        assert_eq!((created.x, created.y), (256_i32, 128_i32), "at its point");
        let side = map.sidedefs.get(8).expect("the new sidedef");
        assert_eq!((side.middle.as_str(), side.x_offset), ("BRICK01", 8_i32));
        assert_eq!(map.linedefs.len(), 8, "and the block grew by one line");
    }

    #[test]
    fn add_resolves_existing_indices_and_one_sided_defaults() {
        let mut map = fixture(MapFormat::Doom);
        let args = add_args(r#"{"v1": 2, "v2": 4, "front": 6}"#);
        let row = add_in_place(&mut map, &args, &spec_of(&args)).expect("an existing pair");
        assert!(row.created_vertices.is_empty(), "nothing was created");
        assert!(row.created_sidedefs.is_empty(), "no side either");
        assert_eq!(row.detail.flags, 1_u32, "one-sided defaults to blocking");
        assert_eq!(
            row.detail.front,
            Some(6),
            "the named sidedef became the front"
        );
    }

    #[test]
    fn add_refuses_degenerate_missing_and_doubled_lines() {
        let mut map = fixture(MapFormat::Doom);
        let same = add_args(r#"{"v1": 4, "v2": 4, "front": 0}"#);
        let error = add_in_place(&mut map, &same, &spec_of(&same)).expect_err("one point");
        assert_eq!(code(&error), "invalid_argument", "degenerate code");
        let twin =
            add_args(r#"{"from": {"x": 256, "y": 256}, "to": {"x": 256, "y": 256}, "front": 0}"#);
        let twin_error =
            add_in_place(&mut map, &twin, &spec_of(&twin)).expect_err("the same point twice");
        assert_eq!(
            code(&twin_error),
            "invalid_argument",
            "point degeneracy code"
        );
        let flipped = add_args(r#"{"v1": 5, "v2": 0, "front": 0}"#);
        let duplicate = add_in_place(&mut map, &flipped, &spec_of(&flipped))
            .expect_err("linedef 0 runs 0 to 5 already");
        assert_eq!(code(&duplicate), "already_exists", "duplicate code");
        assert!(
            duplicate.to_string().contains("linedef 0"),
            "the message names the existing line: {duplicate}"
        );
        let no_vertex = add_args(r#"{"v1": 0, "v2": 9, "front": 0}"#);
        assert_eq!(
            code(&add_in_place(&mut map, &no_vertex, &spec_of(&no_vertex)).expect_err("gone")),
            "not_found",
            "a missing vertex is not_found"
        );
        let no_side = add_args(r#"{"v1": 0, "v2": 2, "front": 99}"#);
        assert_eq!(
            code(&add_in_place(&mut map, &no_side, &spec_of(&no_side)).expect_err("gone")),
            "not_found",
            "a missing sidedef is not_found"
        );
        let no_sector = add_args(r#"{"v1": 0, "v2": 2, "front_side": {"sector": 9}}"#);
        assert_eq!(
            code(&add_in_place(&mut map, &no_sector, &spec_of(&no_sector)).expect_err("gone")),
            "not_found",
            "a missing sector is not_found"
        );
        assert_eq!(map.linedefs.len(), 7, "every refusal appended nothing");
        assert_eq!(map.vertices.len(), 6, "and no vertex came in");
    }

    #[test]
    fn the_hexen_family_moves_tag_into_args() {
        let mut map = fixture(MapFormat::Udmf);
        map.udmf_namespace = Some("zdoom".to_owned());
        let tagged = add_args(r#"{"v1": 0, "v2": 2, "front": 0, "tag": 5}"#);
        let error = add_in_place(&mut map, &tagged, &spec_of(&tagged)).expect_err("no tag here");
        assert_eq!(code(&error), "invalid_argument", "Hexen tag code");
        assert!(
            error.to_string().contains("have no tag"),
            "the message names the field: {error}"
        );
        let arged =
            add_args(r#"{"v1": 0, "v2": 2, "front": 0, "special": 80, "args": [7, 0, 0, 0, 0]}"#);
        let row = add_in_place(&mut map, &arged, &spec_of(&arged)).expect("Hexen takes args");
        assert_eq!(
            row.detail.args,
            [7_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            "and stores them"
        );
        let mut binary_doom = fixture(MapFormat::Doom);
        let arged_doom = add_args(r#"{"v1": 0, "v2": 2, "front": 0, "args": [7, 0, 0, 0, 0]}"#);
        let refusal = add_in_place(&mut binary_doom, &arged_doom, &spec_of(&arged_doom))
            .expect_err("no args here");
        assert_eq!(
            code(&refusal),
            "invalid_argument",
            "the Doom family refuses args"
        );
    }

    #[test]
    fn binary_points_cap_at_i16_and_created_sides_at_i16_offsets() {
        let mut map = fixture(MapFormat::Doom);
        let far = add_args(r#"{"from": {"x": 40000, "y": 0}, "to": {"x": 0, "y": 0}, "front": 0}"#);
        let error = add_in_place(&mut map, &far, &spec_of(&far)).expect_err("above i16");
        assert_eq!(code(&error), "invalid_argument", "coordinate code");
        let wide_side =
            add_args(r#"{"v1": 0, "v2": 2, "front_side": {"sector": 0, "x_offset": 40000}}"#);
        let offsetless =
            add_in_place(&mut map, &wide_side, &spec_of(&wide_side)).expect_err("above i16");
        assert_eq!(code(&offsetless), "invalid_argument", "offset code");
        map.format = MapFormat::Udmf;
        let row = add_in_place(&mut map, &far, &spec_of(&far)).expect("UDMF stores the wide point");
        assert_eq!(row.detail.v1, 6, "the vertex was created anyway");
    }

    #[test]
    fn new_sides_take_dash_and_zero_defaults_and_validated_names() {
        let over_long = super::NewSide {
            sector: 2,
            upper: Some("toolongtexture".to_owned()),
            middle: None,
            lower: None,
            x_offset: None,
            y_offset: None,
        };
        let error =
            new_sidedef(&over_long, MapFormat::Doom).expect_err("ten characters is over the cap");
        assert_eq!(code(&error), "invalid_argument", "texture names validate");
        let built = new_sidedef(
            &super::NewSide {
                sector: 1,
                upper: None,
                middle: Some("step1".to_owned()),
                lower: None,
                x_offset: Some(-4_i32),
                y_offset: None,
            },
            MapFormat::Doom,
        )
        .expect("a plain side builds");
        assert_eq!(built.upper, "-", "absent textures are the dash");
        assert_eq!(built.middle, "STEP1", "given ones uppercase");
        assert_eq!(
            (built.x_offset, built.y_offset),
            (-4_i32, 0_i32),
            "shifts land"
        );
        assert_eq!(built.sector, 1_usize, "the sector passes through");
    }

    #[test]
    fn endpoint_resolution_reports_created_vertices_in_order() {
        let mut map = fixture(MapFormat::Doom);
        let both_new = add_args(
            r#"{"from": {"x": 64, "y": 64}, "to": {"x": -64, "y": 64}, "front_side": {"sector": 0}}"#,
        );
        let ends = place_endpoints(&mut map, &spec_of(&both_new)).expect("both points create");
        assert_eq!(
            (ends.v1, ends.v2),
            (6_usize, 7_usize),
            "appended in call order"
        );
        assert_eq!(ends.created, vec![6_usize, 7], "and reported as created");
    }

    #[test]
    fn edit_validation_names_every_conflict_and_nothing() {
        let good = validate_edit(&edit_args(r#"{"index": 0, "special": 1}"#))
            .expect("one field is a change");
        assert_eq!(good, vec![0_usize], "the target");
        let refusals = [
            r#"{"index": 0, "flags": 3, "set_flags": ["blocking"]}"#,
            r#"{"index": 0, "flags": 3, "clear_flags": ["blocking"]}"#,
            r#"{"index": 0, "front": null}"#,
            r#"{"index": 0, "back": null, "back_side": {"middle": "X"}}"#,
            r#"{"index": 0}"#,
            r#"{"index": []}"#,
            r#"{"index": 0, "flip": false}"#,
            r#"{"index": 0, "front_side": {}}"#,
        ];
        for json in refusals {
            let error = validate_edit(&edit_args(json)).expect_err("the request must refuse");
            assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
        }
        assert!(
            has_change(&edit_args(r#"{"index": 0, "back": null}"#)),
            "back: null is a change"
        );
        assert!(
            has_change(&edit_args(r#"{"index": 0, "flip": true}"#)),
            "flip: true is a change"
        );
    }

    #[test]
    fn a_flip_swaps_ends_and_sides_together() {
        let map = fixture(MapFormat::Doom);
        let args = edit_args(r#"{"index": 2, "flip": true}"#);
        let plan = super::EditPlan::new(&args, vec![2_usize], &map).expect("a flagless plan");
        let line = map.linedefs.get(2).expect("the step line");
        let flipped = edited_line(line, 2, &plan).expect("the flip computes");
        assert_eq!((flipped.v1, flipped.v2), (1_usize, 4), "the ends swapped");
        assert_eq!(flipped.front, Some(3), "the old back is the front now");
        assert_eq!(flipped.back, Some(2), "the old front trails");
        assert_eq!(flipped.flags, line.flags, "flags stand");
    }

    #[test]
    fn setting_and_dropping_the_back_adjusts_the_two_sided_bits() {
        let map = fixture(MapFormat::Doom);
        let opening = edit_args(r#"{"index": 0, "back": 3}"#);
        let plan = super::EditPlan::new(&opening, vec![0_usize], &map).expect("a flagless plan");
        let opened = edited_line(map.linedefs.first().expect("line 0"), 0, &plan).expect("opens");
        assert_eq!(opened.flags, 4_u32, "blocking clears, twosided sets");
        assert_eq!(opened.back, Some(3), "and the side lands");
        let closing = edit_args(r#"{"index": 2, "back": null}"#);
        let shut = super::EditPlan::new(&closing, vec![2_usize], &map).expect("a flagless plan");
        let closed = edited_line(map.linedefs.get(2).expect("the step"), 2, &shut).expect("closes");
        assert_eq!(closed.flags, 1_u32, "twosided clears, blocking sets");
        assert_eq!(closed.back, None, "and the side is gone");
        let held = edit_args(r#"{"index": 2, "back": null, "flags": 256}"#);
        let explicit = super::EditPlan::new(&held, vec![2_usize], &map).expect("an explicit plan");
        let kept =
            edited_line(map.linedefs.get(2).expect("the step"), 2, &explicit).expect("holds");
        assert_eq!(kept.flags, 256_u32, "given flags stand untouched");
        assert_eq!(kept.back, None, "and the side still goes");
    }

    #[test]
    fn an_edit_replaces_fields_or_refuses_a_collapsed_line() {
        let map = fixture(MapFormat::Doom);
        let args =
            edit_args(r#"{"index": 0, "flags": ["blocking", "mapped"], "special": 10, "tag": 1}"#);
        let plan = super::EditPlan::new(&args, vec![0_usize], &map).expect("a plain plan");
        let edited = edited_line(map.linedefs.first().expect("line 0"), 0, &plan).expect("edits");
        assert_eq!(edited.flags, 257_u32, "the names fold to 1 | 256");
        assert_eq!(
            (edited.special, edited.tag),
            (10_i32, 1_i32),
            "the fields land"
        );
        assert_eq!(edited.v1, 0, "unasked references stand");
        let collapsed = edit_args(r#"{"index": 0, "v2": 0}"#);
        let flat = super::EditPlan::new(&collapsed, vec![0_usize], &map).expect("a plan");
        let error = edited_line(map.linedefs.first().expect("line 0"), 0, &flat)
            .expect_err("0 to 0 collapses");
        assert_eq!(code(&error), "invalid_argument", "zero-length code");
        assert!(
            error.to_string().contains("linedef 0 zero-length"),
            "the message names the line: {error}"
        );
    }

    #[test]
    fn editing_touches_only_the_selection_and_applies_side_fields() {
        let mut map = fixture(MapFormat::Doom);
        let args = edit_args(
            r#"{"index": [2, 0], "special": 12, "front_side": {"x_offset": 8, "lower": "step1"}}"#,
        );
        let plan = super::EditPlan::new(&args, vec![0_usize, 2], &map).expect("a plain plan");
        let rows = edit_in_place(&mut map, &plan).expect("both lines edit");
        assert_eq!(
            rows.iter().map(|row| row.index).collect::<Vec<_>>(),
            vec![0_usize, 2],
            "ascending rows"
        );
        assert_eq!(rows[0].detail.special, 12_i32, "the special landed");
        assert_eq!(rows[0].detail.front, Some(0), "front 0");
        assert_eq!(rows[1].detail.front, Some(2), "front 2");
        assert_eq!(
            map.sidedefs.first().expect("side 0").x_offset,
            8_i32,
            "line 0's front shifted"
        );
        assert_eq!(
            (
                map.sidedefs.get(2).expect("side 2").x_offset,
                map.sidedefs.get(2).expect("side 2").lower.as_str()
            ),
            (8_i32, "STEP1"),
            "and line 2's front dressed"
        );
        assert_eq!(
            map.sidedefs.get(1).expect("side 1").x_offset,
            0_i32,
            "the unselected side stands"
        );
        assert_eq!(
            map.linedefs.get(1).expect("line 1").special,
            0_i32,
            "and the unselected line"
        );
    }

    #[test]
    fn a_side_edit_names_a_missing_side_and_bad_references_are_not_found() {
        let mut map = fixture(MapFormat::Doom);
        let args = edit_args(r#"{"index": 0, "back_side": {"middle": "X"}}"#);
        let plan = super::EditPlan::new(&args, vec![0_usize], &map).expect("a plain plan");
        let error = edit_in_place(&mut map, &plan).expect_err("line 0 has no back");
        assert_eq!(code(&error), "invalid_argument", "missing side code");
        assert!(
            error.to_string().contains("linedef 0 has no back side"),
            "the message names the line and side: {error}"
        );
        let bad_line = edit_args(r#"{"index": 9, "special": 1}"#);
        let gone = super::EditPlan::new(&bad_line, vec![9_usize], &map).expect("a plan");
        assert_eq!(
            code(&edit_in_place(&mut map, &gone).expect_err("linedef 9 is not there")),
            "not_found",
            "a missing linedef is not_found"
        );
        let bad_vertex = edit_args(r#"{"index": 0, "v2": 99}"#);
        let vertexless = super::EditPlan::new(&bad_vertex, vec![0_usize], &map).expect("a plan");
        assert_eq!(
            code(&edit_in_place(&mut map, &vertexless).expect_err("vertex 99 is not there")),
            "not_found",
            "a missing vertex is not_found"
        );
        let bad_side = edit_args(r#"{"index": 0, "back": 99}"#);
        let sidless = super::EditPlan::new(&bad_side, vec![0_usize], &map).expect("a plan");
        assert_eq!(
            code(&edit_in_place(&mut map, &sidless).expect_err("sidedef 99 is not there")),
            "not_found",
            "a missing sidedef is not_found"
        );
        assert_eq!(map, fixture(MapFormat::Doom), "no refusal changed anything");
    }

    #[test]
    fn side_edits_keep_the_sector_and_offsets_stay_in_range() {
        let current = side("UP1", "MID1", "LOW1", 4);
        let args = edit_args(r#"{"index": 0, "front_side": {"middle": "NEW1"}}"#);
        let edit = args.front_side.as_ref().expect("the parsed edit");
        let edited = side_edited(&current, edit, MapFormat::Doom).expect("the name computes");
        assert_eq!(edited.middle, "NEW1", "the asked field changed");
        assert_eq!(edited.upper, "UP1", "the unasked ones stand");
        assert_eq!(edited.lower, "LOW1", "on both remaining fields");
        assert_eq!(edited.sector, 4, "the sector is never touched");
        assert!(
            check_line(MapFormat::Doom, FlagFamily::Doom, &solid_line(0, 1, 0)).is_ok(),
            "a plain line passes the family and range checks"
        );
        let shifted = edit_args(r#"{"index": 0, "front_side": {"y_offset": 40000}}"#);
        let wide = shifted.front_side.as_ref().expect("the parsed edit");
        let moved = side_edited(&current, wide, MapFormat::Udmf).expect("UDMF-style storage");
        assert_eq!(moved.y_offset, 40_000_i32, "the edit itself builds");
        assert_eq!(
            code(
                &super::check_binary_offsets(MapFormat::Hexen, &moved).expect_err("binary i16 cap")
            ),
            "invalid_argument",
            "and the binary range check refuses it"
        );
    }

    /// Run `remove_in_place` on `map` with the request parsed from `json`.
    fn removal(json: &str, map: &mut Map) -> super::RemovalReport {
        let args = remove_args(json);
        let indices = args.index.clone().indices().expect("test indices parse");
        super::remove_in_place(map, &args, &indices).expect("the removal applies")
    }

    /// The error `remove_in_place` gives for the request parsed from
    /// `json`.
    fn removal_error(json: &str, map: &mut Map) -> CyberError {
        let args = remove_args(json);
        let indices = args.index.clone().indices().expect("test indices parse");
        super::remove_in_place(map, &args, &indices).expect_err("the request must refuse")
    }

    #[test]
    fn removal_prunes_orphans_with_the_cascade_by_default() {
        let mut map = fixture(MapFormat::Doom);
        let report = removal(r#"{"index": [5, 6]}"#, &mut map);
        assert_eq!(report.linedefs, vec![5_usize, 6], "the selected lines");
        assert_eq!(
            report.sidedefs,
            vec![6_usize, 7],
            "their sides went orphaned"
        );
        assert_eq!(
            report.vertices,
            vec![2_usize],
            "and so did their shared point"
        );
        assert!(report.sectors.is_empty(), "sectors never go");
        assert_eq!(map.linedefs.len(), 5, "five lines survive");
        assert_eq!(map.vertices.len(), 5, "the vertex count dropped");
        assert_eq!(map.sidedefs.len(), 6, "the orphaned sides dropped");
    }

    #[test]
    fn removal_pruning_can_be_switched_off() {
        let mut map = fixture(MapFormat::Doom);
        let report = removal(
            r#"{"index": [5, 6], "prune_sidedefs": false, "prune_vertices": false}"#,
            &mut map,
        );
        assert_eq!(report.linedefs, vec![5_usize, 6], "the selected lines went");
        assert!(report.sidedefs.is_empty(), "the sides stay orphaned");
        assert!(report.vertices.is_empty(), "the vertex stays too");
        assert_eq!(map.vertices.len(), 6, "nothing was pruned");
        assert_eq!(map.sidedefs.len(), 8, "in either block");
    }

    #[test]
    fn removing_a_missing_index_reports_not_found() {
        let mut map = fixture(MapFormat::Doom);
        let error = removal_error(r#"{"index": 40}"#, &mut map);
        assert_eq!(code(&error), "not_found", "out of range code");
        assert_eq!(map, fixture(MapFormat::Doom), "nothing removed");
    }
}
