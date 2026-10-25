//! The pure per-element field engine behind the `bulk_edit` tool.
//!
//! [`apply`] takes one [`FieldEdits`] request (a `set` map, an `add` map
//! and flag-name lists) to a selection of [`Sector`], [`Sidedef`],
//! [`Vertex`], [`Linedef`] or [`Thing`] rows of one decoded [`Map`] and
//! reports what moved.
//!
//! Like [`replace`](super::replace) this is a function of the map model
//! and the passed [`EditScope`] alone: the scope carries the map's
//! format, the flag family and the stock wall and flat name sets, so no
//! session or `MCP` type reaches in here. The tool slice feeds those in,
//! resolves the selection with
//! [`run_query`](crate::wad::map::query::run_query), and pages
//! [`Applied::indices`] and [`Applied::preview`] out.
//!
//! The field tables:
//!
//! | kind | settable | `add`able |
//! |------|----------|-----------|
//! | sectors | `floor`, `ceiling`, `light`, `floor_flat`, `ceiling_flat`, `special`, `tag` | `floor`, `ceiling`, `light`, `tag` |
//! | sidedefs | `upper`, `middle`, `lower`, `x_offset`, `y_offset` | `x_offset`, `y_offset` |
//! | vertices | (none: `set` is refused, `add` only) | `x`, `y` |
//! | linedefs | `special` and `tag` in the `Doom` family, `special` and `arg0..arg4` in the `Hexen` one ([`EditScope::family`]) | (none: linedef numbers are set, never added) |
//! | things | `type`, `angle`, `x`, `y` | `angle`, `x`, `y` |
//!
//! Any other field name is refused with [`CyberError::InvalidArgument`]
//! listing the kind's valid fields; the other family's linedef names (`tag`
//! on a `Hexen`-family map, `arg0..arg4` on a `Doom`-family one) refuse
//! naming the map's family and `convert_map_format`. `flags_set` and
//! `flags_clear` belong to linedefs and things: their names resolve
//! through [`from_names`](crate::wad::map::flags::from_names) in the
//! scope's family table for the kind, the resulting bits are the current
//! ones with the clear mask off and the set mask on (a bit named on both
//! ends up set), and the preview reports the raw bitset under `flags`.
//! The three flagless kinds keep refusing flag lists with a message
//! naming the kinds that carry them.
//!
//! Nothing is clamped and nothing is half-applied: every matched element's
//! resulting values are computed and range-checked first, and only when
//! the whole selection passes are the rows written. Text fields must name
//! a stock entry (the scope's sets), except `"-"`, always legal on the
//! three sidedef faces; a missing name is [`CyberError::NotFound`] unless
//! `allow_missing`. Numeric results stay in `light`'s `0..=255` and
//! `angle`'s `0..=359` in every format and inside the binary record's
//! columns the scope's format stores them in (`i16` heights, offsets and
//! coordinates; `u16` `special` and `tag` on `Doom`-format lines and all
//! thing types; `u8` `special` and args on `Hexen`-format lines); `UDMF`
//! has no such caps beyond the model's `i32` columns, and a sector's
//! ceiling must end at or above its floor.
//!
//! This root holds the request and answer shapes, the range vocabulary
//! (`NumRange`, `Stock`, `FieldKind` and `FieldSpec`), the [`apply`]
//! entry point and its whole-selection `run` pass. Four private modules
//! carry the rest: `tables` is every element kind's field table with the
//! `KindPlan` of accessors it drives; `job` is the `Job` of one run, its
//! request checks and its per-row `Pending` plans; `commit` the check,
//! plan, write and report steps that drive a plan; and `access` the
//! value bounds beside the typed readers and writers of the five element
//! blocks.
//!
//! [`Sector`]: crate::wad::map::Sector
//! [`Sidedef`]: crate::wad::map::Sidedef
//! [`Vertex`]: crate::wad::map::Vertex
//! [`Linedef`]: crate::wad::map::Linedef
//! [`Thing`]: crate::wad::map::Thing

mod access;
mod commit;
mod job;
mod tables;

#[cfg(test)]
mod tests;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::flags::FlagFamily;
use super::super::model::Map;
use super::super::query::ElementKind;
use crate::error::CyberError;
use crate::wad::classify::MapFormat;

use commit::{commit, report};
use job::{Job, Pending, new_job};

/// The texture name meaning "nothing here", always legal on a face,
/// shared from [`consts`](crate::wad::map::consts).
pub(super) use crate::wad::map::consts::NO_TEXTURE;
/// The lowest light level the vanilla range allows.
const LIGHT_MIN: i64 = 0_i64;
/// The highest light level the vanilla range allows (`u8::MAX`).
const LIGHT_MAX: i64 = 255_i64;
/// The lowest facing angle a thing may end at.
const ANGLE_MIN: i64 = 0_i64;
/// The highest facing angle a thing may end at, one below a full circle.
const ANGLE_MAX: i64 = 359_i64;
/// The lowest value a `u8` binary column holds.
const U8_MIN: i64 = 0_i64;
/// The highest value a `u8` binary column holds (`u8::MAX`).
const U8_MAX: i64 = 0xFF_i64;
/// The lowest value an `i16` binary column holds (`i16::MIN`).
const I16_MIN: i64 = -0x8000_i64;
/// The highest value an `i16` binary column holds (`i16::MAX`).
const I16_MAX: i64 = 0x7FFF_i64;
/// The lowest value a `u16` binary column holds.
const U16_MIN: i64 = 0_i64;
/// The highest value a `u16` binary column holds (`u16::MAX`).
const U16_MAX: i64 = 0xFFFF_i64;
/// The lowest value the model's `i32` columns hold (`i32::MIN`).
const I32_MIN: i64 = -0x8000_0000_i64;
/// The highest value the model's `i32` columns hold (`i32::MAX`).
const I32_MAX: i64 = 0x7FFF_FFFF_i64;

/// The field changes one bulk edit asks every matched element to take.
///
/// This is the tool's `JSON` shape: [`FieldEdits::set`] maps field names
/// to the new values (integers or texture/flat name strings),
/// [`FieldEdits::add`] maps field names to integer amounts shifted onto
/// what each element currently holds, and the flag lists name bits to set
/// and clear. The names each kind accepts are the tables at the head of
/// this module's docs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct FieldEdits {
    /// New values by field name.
    #[serde(default)]
    pub set: BTreeMap<String, Value>,
    /// Integer amounts added to each selected element's current value.
    #[serde(default)]
    pub add: BTreeMap<String, i64>,
    /// Flag names to set on each selected element.
    #[serde(default)]
    pub flags_set: Vec<String>,
    /// Flag names to clear on each selected element.
    #[serde(default)]
    pub flags_clear: Vec<String>,
}

impl FieldEdits {
    /// Whether this request asks for no change at all.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
            && self.add.is_empty()
            && self.flags_set.is_empty()
            && self.flags_clear.is_empty()
    }
}

/// Everything the engine needs to know about the world outside the map.
///
/// The format and flag family come from the map block, the name sets
/// from the session's stock, and `allow_missing` from the tool's
/// parameter: bundling them here keeps `src/wad/` free of sessions and
/// `MCP` types.
#[derive(Clone, Debug)]
pub struct EditScope<'stock> {
    /// The map block's storage format, deciding the binary field ranges.
    pub format: MapFormat,
    /// The flag family the `flags_set`/`flags_clear` names of linedefs and
    /// things read from, and the family that gates the linedef fields.
    pub family: FlagFamily,
    /// The known texture names for the sidedef faces, uppercase.
    pub walls: &'stock BTreeSet<String>,
    /// The known flat names for the sector surfaces, uppercase.
    pub flats: &'stock BTreeSet<String>,
    /// Whether a name missing from the stock sets is written anyway.
    pub allow_missing: bool,
}

/// One element's touched fields, before the change and after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Change {
    /// Position in the element block.
    pub index: usize,
    /// Object keyed by the touched field names, holding the old values.
    pub before: Value,
    /// Object keyed by the touched field names, holding the new values.
    pub after: Value,
}

/// What one [`apply`] run did to one map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Applied {
    /// Selected elements, counted once each (duplicates collapse).
    pub matched: usize,
    /// Elements whose field values actually changed.
    pub changed: usize,
    /// Ascending indices of the elements that changed.
    pub indices: Vec<usize>,
    /// One [`Change`] preview per changed element, ascending by index.
    pub preview: Vec<Change>,
}

/// The bounds one editable numeric field's resulting value must obey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NumRange {
    /// The vanilla light level: `0..=255` in every format.
    Light,
    /// A thing's facing: `0..=359` in every format, never normalised.
    Angle,
    /// A height, offset or coordinate: an `i16` column on binary maps.
    Small,
    /// A `special` or `tag`: a `u16` column on binary maps.
    Unsigned,
    /// A linedef's `special`: a `u16` column on binary `Doom`, a `u8`
    /// column on binary `Hexen`.
    LineSpecial,
    /// A `Hexen` script argument: a `u8` column on binary maps.
    Byte,
}

/// The stock list one editable text field's name is checked against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stock {
    /// A sector flat: [`EditScope::flats`] must hold the name.
    Flats,
    /// A sidedef face: [`EditScope::walls`] must hold the name, and
    /// `"-"` is always legal.
    Walls,
}

/// What one editable field holds: a ranged integer or a stock name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKind {
    /// An integer column bounded by [`NumRange`].
    Num(NumRange),
    /// A texture or flat name checked against a [`Stock`] set.
    Text(Stock),
}

/// One editable field of one element kind.
struct FieldSpec {
    /// The table's exact (lowercase) field name.
    name: &'static str,
    /// Whether the field accepts an `add` amount.
    addable: bool,
    /// Whether the field holds a number or a name.
    kind: FieldKind,
}

/// Applies `edits` to the `indices` rows of one element `kind` of `map`.
///
/// The whole selection is computed and validated first, so an error
/// leaves the map exactly as it was; on success every planned value is
/// written and [`Applied`] counts and previews what moved. Duplicated
/// indices collapse, and a selected row whose new values all equal its
/// old ones counts as matched, not as changed.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty request, flag names on
/// the flagless kinds, a field the kind does not carry or cannot `add`,
/// a `set` on vertices, the other flag family's linedef names (naming the
/// map's family and `convert_map_format`), a flag name the family table
/// does not hold, the same field in both `set` and `add`, a value of the
/// wrong type for its field, or a resulting value outside the field's
/// range or the sector's `ceiling >= floor` rule; each names the offending
/// field and the rule. [`CyberError::NotFound`] for a texture/flat name
/// the scope's stock lacks without `allow_missing`, or for an index past
/// the block.
#[expect(
    clippy::too_many_arguments,
    reason = "the CD-108 slice fixes this engine entry point: the bulk_edit tool calls apply(map, kind, indices, edits, scope) verbatim"
)]
#[inline]
pub fn apply(
    map: &mut Map,
    kind: ElementKind,
    indices: &[usize],
    edits: &FieldEdits,
    scope: &EditScope<'_>,
) -> Result<Applied, CyberError> {
    let job = new_job(kind, edits, scope)?;
    run(&job, map, indices)
}

/// The whole all-or-nothing pass over one map. The job's request has
/// already passed [`new_job`]'s checks by the time this runs.
fn run(job: &Job<'_>, map: &mut Map, indices: &[usize]) -> Result<Applied, CyberError> {
    let targets = job.selection(map, indices)?;
    let matched = targets.len();
    let mut planned: Vec<Pending> = Vec::with_capacity(matched);
    for index in &targets {
        planned.push(job.propose(map, *index)?);
    }
    let report = report(job, map, matched, &planned);
    for row in &planned {
        commit(job, map, row)?;
    }
    Ok(report)
}
