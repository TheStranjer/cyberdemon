//! The pure per-element field engine behind the `bulk_edit` tool.
//!
//! [`apply`] takes one [`FieldEdits`] request (a `set` map, an `add` map
//! and flag-name lists) to a selection of [`Sector`], [`Sidedef`] or
//! [`Vertex`] rows of one decoded [`Map`] and reports what moved.
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
//!
//! Any other field name is refused with [`CyberError::InvalidArgument`]
//! listing the kind's valid fields; `flags_set` and `flags_clear` belong
//! to linedefs and things, whose [`apply`] arms refuse with a message
//! naming their fields until the next slice wires those engines in.
//!
//! Nothing is clamped and nothing is half-applied: every matched element's
//! resulting values are computed and range-checked first, and only when
//! the whole selection passes are the rows written. Text fields must name
//! a stock entry (the scope's sets), except `"-"`, always legal on the
//! three sidedef faces; a missing name is [`CyberError::NotFound`] unless
//! `allow_missing`. Numeric results stay in `light`'s `0..=255` in every
//! format and inside the binary record's columns the scope's format
//! stores them in (`i16` heights, offsets and coordinates, `u16`
//! `special` and `tag`); `UDMF` has no such caps beyond the model's
//! `i32` columns, and a sector's ceiling must end at or above its floor.
//!
//! [`Sector`]: crate::wad::map::Sector
//! [`Sidedef`]: crate::wad::map::Sidedef
//! [`Vertex`]: crate::wad::map::Vertex

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use super::super::flags::FlagFamily;
use super::super::model::Map;
use super::super::query::ElementKind;
use crate::error::CyberError;
use crate::wad::classify::MapFormat;

/// The texture name meaning "nothing here", always legal on a face.
const NO_TEXTURE: &str = "-";
/// The lowest light level the vanilla range allows.
const LIGHT_MIN: i64 = 0_i64;
/// The highest light level the vanilla range allows (`u8::MAX`).
const LIGHT_MAX: i64 = 255_i64;
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
    /// The flag family the flag names read from, once the kinds with
    /// flags are wired in.
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
    /// A height, offset or coordinate: an `i16` column on binary maps.
    Small,
    /// A `special` or `tag`: a `u16` column on binary maps.
    Unsigned,
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

/// Sector fields: the heights, light, flats, effect and tag.
const SECTOR_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "ceiling",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "ceiling_flat",
        addable: false,
        kind: FieldKind::Text(Stock::Flats),
    },
    FieldSpec {
        name: "floor",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "floor_flat",
        addable: false,
        kind: FieldKind::Text(Stock::Flats),
    },
    FieldSpec {
        name: "light",
        addable: true,
        kind: FieldKind::Num(NumRange::Light),
    },
    FieldSpec {
        name: "special",
        addable: false,
        kind: FieldKind::Num(NumRange::Unsigned),
    },
    FieldSpec {
        name: "tag",
        addable: true,
        kind: FieldKind::Num(NumRange::Unsigned),
    },
];

/// Sidedef fields: the three texture faces and the offsets.
const SIDEDEF_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "lower",
        addable: false,
        kind: FieldKind::Text(Stock::Walls),
    },
    FieldSpec {
        name: "middle",
        addable: false,
        kind: FieldKind::Text(Stock::Walls),
    },
    FieldSpec {
        name: "upper",
        addable: false,
        kind: FieldKind::Text(Stock::Walls),
    },
    FieldSpec {
        name: "x_offset",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "y_offset",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
];

/// Vertex fields: the coordinates, `add` only.
const VERTEX_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "x",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
    FieldSpec {
        name: "y",
        addable: true,
        kind: FieldKind::Num(NumRange::Small),
    },
];

/// Reads one numeric field of the row at an index.
type NumReader = fn(&Map, usize, &str) -> Option<i32>;
/// Reads one text field of the row at an index.
type TextReader = fn(&Map, usize, &str) -> Option<String>;
/// Writes one planned integer onto the row at an index.
type NumWriter = fn(&mut Map, usize, &str, i32);
/// Writes one planned name onto the row at an index.
type TextWriter = fn(&mut Map, usize, &str, &str);

/// The block-specific plumbing one [`apply`] run drives.
struct KindPlan {
    /// Singular element name for the messages ("sector").
    thing: &'static str,
    /// The fields this kind edits.
    fields: &'static [FieldSpec],
    /// Reads a numeric field of the row at an index.
    get_num: NumReader,
    /// Reads a text field of the row at an index.
    get_text: TextReader,
    /// Writes one planned integer onto the row at its index.
    write_num: NumWriter,
    /// Writes one planned name onto the row at its index.
    write_text: TextWriter,
}

/// The sector block's plan.
const SECTOR_PLAN: KindPlan = KindPlan {
    thing: "sector",
    fields: SECTOR_FIELDS,
    get_num: sector_num,
    get_text: sector_text,
    write_num: write_sector_num,
    write_text: write_sector_text,
};

/// The sidedef block's plan.
const SIDEDEF_PLAN: KindPlan = KindPlan {
    thing: "sidedef",
    fields: SIDEDEF_FIELDS,
    get_num: side_num,
    get_text: side_text,
    write_num: write_side_num,
    write_text: write_side_text,
};

/// The vertex block's plan.
const VERTEX_PLAN: KindPlan = KindPlan {
    thing: "vertex",
    fields: VERTEX_FIELDS,
    get_num: vertex_num,
    get_text: vertex_text,
    write_num: write_vertex_num,
    write_text: write_vertex_text,
};

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
/// these flagless kinds, a field the kind does not carry or cannot
/// `add`, a `set` on vertices, the same field in both `set` and `add`, a
/// value of the wrong type for its field, or a resulting value outside
/// the field's range or the sector's `ceiling >= floor` rule; each names
/// the offending field and the rule. [`CyberError::NotFound`] for a
/// texture/flat name the scope's stock lacks without `allow_missing`, or
/// for an index past the block.
#[expect(
    clippy::too_many_arguments,
    reason = "the CD-108 slice fixes this engine entry point: the next slice's bulk_edit tool calls apply(map, kind, indices, edits, scope) verbatim"
)]
#[inline]
pub fn apply(
    map: &mut Map,
    kind: ElementKind,
    indices: &[usize],
    edits: &FieldEdits,
    scope: &EditScope<'_>,
) -> Result<Applied, CyberError> {
    let plan: &'static KindPlan = match kind {
        ElementKind::Sectors => &SECTOR_PLAN,
        ElementKind::Sidedefs => &SIDEDEF_PLAN,
        ElementKind::Vertices => &VERTEX_PLAN,
        ElementKind::Linedefs => return apply_linedefs(),
        ElementKind::Things => return apply_things(),
    };
    run(
        &Job {
            kind,
            plan,
            edits,
            scope,
        },
        map,
        indices,
    )
}

/// One [`apply`] run: the request, its kind's plan and the scope.
struct Job<'req> {
    /// The selected element block.
    kind: ElementKind,
    /// The block's table and accessors.
    plan: &'static KindPlan,
    /// The requested field changes.
    edits: &'req FieldEdits,
    /// Format, flag family and stock sets.
    scope: &'req EditScope<'req>,
}

/// One selected row's planned values, computed before anything is written.
struct Pending {
    /// Row position in the block.
    index: usize,
    /// Planned integers by field name.
    nums: Vec<(&'static str, i64)>,
    /// Planned names (uppercase, stock-checked) by field name.
    texts: Vec<(&'static str, String)>,
}

/// The whole all-or-nothing pass over one map.
fn run(job: &Job<'_>, map: &mut Map, indices: &[usize]) -> Result<Applied, CyberError> {
    job.check_request()?;
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

impl Job<'_> {
    /// Refuses a request this kind cannot carry before any map reads.
    fn check_request(&self) -> Result<(), CyberError> {
        if self.edits.is_empty() {
            return Err(CyberError::InvalidArgument(
                "nothing to change: pass at least one of set, add, flags_set or \
                 flags_clear"
                    .to_owned(),
            ));
        }
        self.check_flags()?;
        self.check_set_fields()?;
        check_add_fields(self)?;
        check_overlap(self)?;
        check_values(self)
    }

    /// Refuses flag names, which these kinds do not store.
    fn check_flags(&self) -> Result<(), CyberError> {
        if self.edits.flags_set.is_empty() && self.edits.flags_clear.is_empty() {
            return Ok(());
        }
        Err(CyberError::InvalidArgument(format!(
            "{} have no flags field: flags_set and flags_clear apply to linedefs \
             and things",
            self.kind.as_str()
        )))
    }

    /// Refuses a `set` on vertices and every field name outside the table.
    fn check_set_fields(&self) -> Result<(), CyberError> {
        if self.kind == ElementKind::Vertices && !self.edits.set.is_empty() {
            return Err(CyberError::InvalidArgument(format!(
                "vertices support add on {} only, never set: move these values \
                 into add",
                names_of(self.plan.fields, true)
            )));
        }
        for key in self.edits.set.keys() {
            self.field(key)?;
        }
        Ok(())
    }

    /// Refuses a text value that is not a string or, unless
    /// `allow_missing`, a name the scope's stock does not hold.
    fn check_name(&self, stock: Stock, field: &str, value: &Value) -> Result<(), CyberError> {
        let Some(text) = value.as_str() else {
            return Err(CyberError::InvalidArgument(format!(
                "field '{field}' needs a name string, got {value}"
            )));
        };
        if matches!(stock, Stock::Walls) && text == NO_TEXTURE {
            return Ok(());
        }
        let wanted = text.to_ascii_uppercase();
        let held = match stock {
            Stock::Flats => self.scope.flats,
            Stock::Walls => self.scope.walls,
        };
        if held.contains(&wanted) || self.scope.allow_missing {
            return Ok(());
        }
        Err(CyberError::NotFound(format!(
            "{field} '{wanted}' is not part of the map's stock: pass \
             allow_missing: true to write the name anyway, or pick a stocked one"
        )))
    }

    /// The table entry a request name refers to.
    fn field(&self, key: &str) -> Result<&'static FieldSpec, CyberError> {
        find_field(self.plan.fields, key).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "unknown field '{key}' for {}: valid fields are {}",
                self.kind.as_str(),
                names_of(self.plan.fields, false)
            ))
        })
    }

    /// The sorted, deduplicated selection, refused past the block's end.
    fn selection(&self, map: &Map, indices: &[usize]) -> Result<BTreeSet<usize>, CyberError> {
        let count = self.kind.count_on(map);
        if let Some(past) = indices.iter().find(|index| **index >= count) {
            return Err(CyberError::NotFound(format!(
                "{} {past} does not exist (the map holds {count} {}): pass \
                 existing indices, as find_elements answers with",
                self.plan.thing,
                self.kind.as_str()
            )));
        }
        Ok(indices.iter().copied().collect())
    }

    /// The row's full planned result, range-checked and not yet written.
    fn propose(&self, map: &Map, index: usize) -> Result<Pending, CyberError> {
        let mut pending = Pending {
            index,
            nums: Vec::new(),
            texts: Vec::new(),
        };
        plan_set(self, &mut pending)?;
        plan_add(self, map, &mut pending)?;
        check_numbers(self, map, &pending)?;
        self.check_height_order(map, &pending)?;
        Ok(pending)
    }

    /// Refuses one resulting numeric value outside its field's range.
    fn check_number(&self, spec: &FieldSpec, index: usize, value: i64) -> Result<(), CyberError> {
        let FieldKind::Num(range) = spec.kind else {
            return Ok(());
        };
        let (low, high) = bounds(range, self.scope.format);
        if (low..=high).contains(&value) {
            return Ok(());
        }
        Err(CyberError::InvalidArgument(format!(
            "{} {index} {} would become {value}; {} must stay within {low}..={high}",
            self.plan.thing, spec.name, spec.name
        )))
    }

    /// Refuses a resulting sector whose ceiling ends below its floor.
    fn check_height_order(&self, map: &Map, pending: &Pending) -> Result<(), CyberError> {
        if self.kind != ElementKind::Sectors {
            return Ok(());
        }
        let Some(floor) = self.resulting(map, pending, "floor") else {
            return Err(self.vanished("floor"));
        };
        let Some(ceiling) = self.resulting(map, pending, "ceiling") else {
            return Err(self.vanished("ceiling"));
        };
        if ceiling >= floor {
            return Ok(());
        }
        Err(CyberError::InvalidArgument(format!(
            "{} {} would end with floor {floor} above ceiling {ceiling}: keep the \
             ceiling at or above the floor (equal is a closed door)",
            self.plan.thing, pending.index
        )))
    }

    /// The resulting row's value for a numeric field: planned or current.
    fn resulting(&self, map: &Map, pending: &Pending, field: &str) -> Option<i64> {
        let current = i64::from(self.current(map, pending.index, field)?);
        Some(
            pending
                .nums
                .iter()
                .find(|planned| planned.0 == field)
                .map_or(current, |planned| planned.1),
        )
    }

    /// The row's stored value for one numeric field.
    fn current(&self, map: &Map, index: usize, field: &str) -> Option<i32> {
        (self.plan.get_num)(map, index, field)
    }

    /// The refusal for a row that vanished between the bounds check and
    /// the read: an engine bug, never a user error.
    fn vanished(&self, field: &str) -> CyberError {
        CyberError::Internal(format!(
            "bulk edits: the {} field '{field}' disappeared under the selection",
            self.plan.thing
        ))
    }
}

/// Refuses every `add` name the table does not accept amounts for.
fn check_add_fields(job: &Job<'_>) -> Result<(), CyberError> {
    for key in job.edits.add.keys() {
        let spec = job.field(key)?;
        if !spec.addable {
            return Err(CyberError::InvalidArgument(format!(
                "field '{key}' is not addable on {}: addable fields are {}",
                job.kind.as_str(),
                names_of(job.plan.fields, true)
            )));
        }
    }
    Ok(())
}

/// Refuses the same field carried by both `set` and `add`.
fn check_overlap(job: &Job<'_>) -> Result<(), CyberError> {
    for key in job.edits.add.keys() {
        if job.edits.set.contains_key(key) {
            return Err(CyberError::InvalidArgument(format!(
                "field '{key}' carries both a set value and an add amount: pass \
                 one of them"
            )));
        }
    }
    Ok(())
}

/// Checks every value's type and every text name's stock up front.
fn check_values(job: &Job<'_>) -> Result<(), CyberError> {
    for (key, value) in &job.edits.set {
        let spec = job.field(key)?;
        match spec.kind {
            FieldKind::Num(_) => {
                number_of(job, spec.name, value)?;
            }
            FieldKind::Text(stock) => {
                job.check_name(stock, spec.name, value)?;
            }
        }
    }
    Ok(())
}

/// Plans every `set` value onto `pending`.
fn plan_set(job: &Job<'_>, pending: &mut Pending) -> Result<(), CyberError> {
    for (key, value) in &job.edits.set {
        let spec = job.field(key)?;
        match spec.kind {
            FieldKind::Num(_) => queue_number(job, pending, spec, value)?,
            FieldKind::Text(_) => queue_text(job, pending, spec, value)?,
        }
    }
    Ok(())
}

/// Plans one `set` integer onto `pending`.
fn queue_number(
    job: &Job<'_>,
    pending: &mut Pending,
    spec: &FieldSpec,
    value: &Value,
) -> Result<(), CyberError> {
    let planned = number_of(job, spec.name, value)?;
    pending.nums.push((spec.name, planned));
    Ok(())
}

/// Plans one `set` name (uppercased, stock-checked) onto `pending`.
fn queue_text(
    job: &Job<'_>,
    pending: &mut Pending,
    spec: &FieldSpec,
    value: &Value,
) -> Result<(), CyberError> {
    let name = text_value(job, spec.name, value)?;
    pending.texts.push((spec.name, name));
    Ok(())
}

/// Plans every `add` amount onto the row's current value.
fn plan_add(job: &Job<'_>, map: &Map, pending: &mut Pending) -> Result<(), CyberError> {
    let label = job.plan.thing;
    for (key, delta) in &job.edits.add {
        let spec = job.field(key)?;
        let field = spec.name;
        let Some(current) = job.current(map, pending.index, field) else {
            return Err(job.vanished(field));
        };
        let Some(planned) = i64::from(current).checked_add(*delta) else {
            return Err(CyberError::InvalidArgument(format!(
                "{label} {field} {current} plus {delta} leaves the range of \
                 integers: pass a smaller amount or set the value"
            )));
        };
        pending.nums.push((field, planned));
    }
    Ok(())
}

/// Refuses any numeric field of the resulting row the format cannot hold:
/// the light's `0..=255` everywhere, the binary columns' ranges, the
/// `i32` model storage on `UDMF`.
fn check_numbers(job: &Job<'_>, map: &Map, pending: &Pending) -> Result<(), CyberError> {
    for spec in job.plan.fields {
        if let Some(value) = job.resulting(map, pending, spec.name) {
            job.check_number(spec, pending.index, value)?;
        }
    }
    Ok(())
}

/// Writes one fully validated pending row back onto the map.
fn commit(job: &Job<'_>, map: &mut Map, pending: &Pending) -> Result<(), CyberError> {
    for planned in &pending.nums {
        let value = planned.1;
        let number = i32::try_from(value).map_err(|error| {
            CyberError::Internal(format!(
                "bulk edits: the planned value {value} for '{}' left the column's \
                 range: {error}",
                planned.0
            ))
        })?;
        (job.plan.write_num)(map, pending.index, planned.0, number);
    }
    for planned in &pending.texts {
        (job.plan.write_text)(map, pending.index, planned.0, &planned.1);
    }
    Ok(())
}

/// The [`Applied`] of a validated pass, read off the still-untouched map.
fn report(job: &Job<'_>, map: &Map, matched: usize, planned: &[Pending]) -> Applied {
    let mut indices: Vec<usize> = Vec::new();
    let mut preview: Vec<Change> = Vec::new();
    for pending in planned {
        if let Some(change) = diffs(job, map, pending) {
            indices.push(change.index);
            preview.push(change);
        }
    }
    let changed = indices.len();
    Applied {
        matched,
        changed,
        indices,
        preview,
    }
}

/// The change row of one pending row, `None` when nothing differs.
fn diffs(job: &Job<'_>, map: &Map, pending: &Pending) -> Option<Change> {
    let mut before: Vec<(String, Value)> = Vec::new();
    let mut after: Vec<(String, Value)> = Vec::new();
    let mut changed = false;
    for spec in job.plan.fields {
        let pair = match spec.kind {
            FieldKind::Num(_) => number_pair(job, map, pending, spec.name),
            FieldKind::Text(_) => text_pair(job, map, pending, spec.name),
        };
        if let Some((old, new)) = pair {
            changed |= old != new;
            before.push((spec.name.to_owned(), old));
            after.push((spec.name.to_owned(), new));
        }
    }
    if !changed {
        return None;
    }
    Some(Change {
        index: pending.index,
        before: into_object(before),
        after: into_object(after),
    })
}

/// The before/after pair of a planned numeric field, if this row plans one.
fn number_pair(job: &Job<'_>, map: &Map, pending: &Pending, field: &str) -> Option<(Value, Value)> {
    let planned = pending.nums.iter().find(|planned| planned.0 == field)?.1;
    let current = i64::from(job.current(map, pending.index, field)?);
    Some((Value::from(current), Value::from(planned)))
}

/// The before/after pair of a planned text field, if this row plans one.
fn text_pair(job: &Job<'_>, map: &Map, pending: &Pending, field: &str) -> Option<(Value, Value)> {
    let planned = &pending.texts.iter().find(|planned| planned.0 == field)?.1;
    let current = (job.plan.get_text)(map, pending.index, field)?;
    Some((Value::String(current), Value::String(planned.clone())))
}

/// The `(name, value)` pairs as one `JSON` object.
fn into_object(pairs: Vec<(String, Value)>) -> Value {
    let collected: JsonMap<String, Value> = pairs.into_iter().collect();
    Value::Object(collected)
}

/// The table entry named `name`, if the table carries it.
fn find_field(fields: &'static [FieldSpec], name: &str) -> Option<&'static FieldSpec> {
    fields.iter().find(|spec| spec.name == name)
}

/// The table's field names, all or only the addable ones, comma-joined.
fn names_of(fields: &[FieldSpec], addable_only: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for spec in fields {
        if !addable_only || spec.addable {
            parts.push(spec.name);
        }
    }
    parts.join(", ")
}

/// The inclusive bounds `range` enforces on a map stored in `format`.
fn bounds(range: NumRange, format: MapFormat) -> (i64, i64) {
    if format == MapFormat::Udmf {
        let storage = matches!(range, NumRange::Small | NumRange::Unsigned);
        if storage {
            return (I32_MIN, I32_MAX);
        }
        return (LIGHT_MIN, LIGHT_MAX);
    }
    match range {
        NumRange::Light => (LIGHT_MIN, LIGHT_MAX),
        NumRange::Small => (I16_MIN, I16_MAX),
        NumRange::Unsigned => (U16_MIN, U16_MAX),
    }
}

/// The integer a numeric field's set value carries.
fn number_of(job: &Job<'_>, field: &str, value: &Value) -> Result<i64, CyberError> {
    value.as_i64().ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "{} field '{field}' needs an integer value, got {value}",
            job.plan.thing
        ))
    })
}

/// The uppercased name a text field's set value carries.
fn text_value(job: &Job<'_>, field: &str, value: &Value) -> Result<String, CyberError> {
    value.as_str().map(str::to_ascii_uppercase).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "{} field '{field}' needs a name string, got {value}",
            job.plan.thing
        ))
    })
}

/// Reads the numeric sector field `field` of `map`'s row `index`.
fn sector_num(map: &Map, index: usize, field: &str) -> Option<i32> {
    let row = map.sectors.get(index)?;
    match field {
        "ceiling" => Some(row.ceiling_height),
        "floor" => Some(row.floor_height),
        "light" => Some(row.light),
        "special" => Some(row.special),
        "tag" => Some(row.tag),
        _ => None,
    }
}

/// Reads the text sector field `field` of `map`'s row `index`.
fn sector_text(map: &Map, index: usize, field: &str) -> Option<String> {
    let row = map.sectors.get(index)?;
    match field {
        "ceiling_flat" => Some(row.ceiling_flat.clone()),
        "floor_flat" => Some(row.floor_flat.clone()),
        _ => None,
    }
}

/// Writes one planned integer onto a sector row.
fn write_sector_num(map: &mut Map, index: usize, field: &str, number: i32) {
    let Some(row) = map.sectors.get_mut(index) else {
        return;
    };
    match field {
        "ceiling" => row.ceiling_height = number,
        "floor" => row.floor_height = number,
        "light" => row.light = number,
        "special" => row.special = number,
        "tag" => row.tag = number,
        _ => {}
    }
}

/// Writes one planned name onto a sector row.
fn write_sector_text(map: &mut Map, index: usize, field: &str, name: &str) {
    let Some(row) = map.sectors.get_mut(index) else {
        return;
    };
    match field {
        "ceiling_flat" => name.clone_into(&mut row.ceiling_flat),
        "floor_flat" => name.clone_into(&mut row.floor_flat),
        _ => {}
    }
}

/// Reads the numeric sidedef field `field` of `map`'s row `index`.
fn side_num(map: &Map, index: usize, field: &str) -> Option<i32> {
    let row = map.sidedefs.get(index)?;
    match field {
        "x_offset" => Some(row.x_offset),
        "y_offset" => Some(row.y_offset),
        _ => None,
    }
}

/// Reads the text sidedef field `field` of `map`'s row `index`.
fn side_text(map: &Map, index: usize, field: &str) -> Option<String> {
    let row = map.sidedefs.get(index)?;
    match field {
        "lower" => Some(row.lower.clone()),
        "middle" => Some(row.middle.clone()),
        "upper" => Some(row.upper.clone()),
        _ => None,
    }
}

/// Writes one planned integer onto a sidedef row.
fn write_side_num(map: &mut Map, index: usize, field: &str, number: i32) {
    let Some(row) = map.sidedefs.get_mut(index) else {
        return;
    };
    match field {
        "x_offset" => row.x_offset = number,
        "y_offset" => row.y_offset = number,
        _ => {}
    }
}

/// Writes one planned name onto a sidedef row.
fn write_side_text(map: &mut Map, index: usize, field: &str, name: &str) {
    let Some(row) = map.sidedefs.get_mut(index) else {
        return;
    };
    match field {
        "lower" => name.clone_into(&mut row.lower),
        "middle" => name.clone_into(&mut row.middle),
        "upper" => name.clone_into(&mut row.upper),
        _ => {}
    }
}

/// Reads the numeric vertex field `field` of `map`'s row `index`.
fn vertex_num(map: &Map, index: usize, field: &str) -> Option<i32> {
    let row = map.vertices.get(index)?;
    match field {
        "x" => Some(row.x),
        "y" => Some(row.y),
        _ => None,
    }
}

/// Vertices store no text fields, so this reads none.
const fn vertex_text(_map: &Map, _index: usize, _field: &str) -> Option<String> {
    None
}

/// Writes one planned integer onto a vertex row.
fn write_vertex_num(map: &mut Map, index: usize, field: &str, number: i32) {
    let Some(row) = map.vertices.get_mut(index) else {
        return;
    };
    match field {
        "x" => row.x = number,
        "y" => row.y = number,
        _ => {}
    }
}

/// Vertices store no text fields, so this writes nothing.
const fn write_vertex_text(_map: &mut Map, _index: usize, _field: &str, _name: &str) {}

/// The refusal for a linedef selection: `bulk_edit` covers sectors,
/// sidedefs and vertices here, and the linedef fields (`flags`,
/// `special`, tag or `args`, `v1` and `v2`) land with the next bulk
/// slice's engine; until then this names the way forward.
fn apply_linedefs() -> Result<Applied, CyberError> {
    Err(CyberError::InvalidArgument(
        "linedef bulk editing is not wired yet: it will cover flags, special, tag, \
         args, v1 and v2; until then use edit_linedef per line"
            .to_owned(),
    ))
}

/// The refusal for a thing selection: `bulk_edit` covers sectors,
/// sidedefs and vertices here, and the thing fields (position, facing,
/// `type`, `flags`, `tid` and friends) land with the next bulk slice's
/// engine; until then this names the way forward.
fn apply_things() -> Result<Applied, CyberError> {
    Err(CyberError::InvalidArgument(
        "thing bulk editing is not wired yet: it will cover x, y, z, angle, type, \
         flags, tid, special and args; until then use edit_thing per thing"
            .to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use alloc::string::String;

    use serde::Serialize;
    use serde_json::{Value, from_str, json};

    use super::{Applied, Change, EditScope, FieldEdits, apply};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::map::query::ElementKind;
    use crate::wad::map::{Map, Sector, Sidedef, Vertex};

    /// The test stock: the names the sample map's textures and flats
    /// come from, and the scopes the engine runs under.
    struct Stock {
        /// The walls the sidedef faces may wear.
        walls: BTreeSet<String>,
        /// The flats the sector surfaces may wear.
        flats: BTreeSet<String>,
    }

    impl Stock {
        /// The stock the sample map wears.
        fn new() -> Self {
            Self {
                walls: set_of(&["BRICK1", "STARTAN1"]),
                flats: set_of(&["CEIL5", "FLAT1", "FLOOR4_8"]),
            }
        }

        /// The binary `Doom` scope over this stock.
        fn doom(&self) -> EditScope<'_> {
            EditScope {
                format: MapFormat::Doom,
                family: FlagFamily::Doom,
                walls: &self.walls,
                flats: &self.flats,
                allow_missing: false,
            }
        }

        /// The binary `Doom` scope that lets unstocked names through.
        fn permissive(&self) -> EditScope<'_> {
            EditScope {
                allow_missing: true,
                ..self.doom()
            }
        }

        /// The `UDMF` scope over this stock.
        fn udmf(&self) -> EditScope<'_> {
            EditScope {
                format: MapFormat::Udmf,
                ..self.doom()
            }
        }
    }

    /// A stock set from `items`.
    fn set_of(items: &[&str]) -> BTreeSet<String> {
        items.iter().copied().map(str::to_owned).collect()
    }

    /// A `set`-only request.
    fn set_request(pairs: &[(&str, Value)]) -> FieldEdits {
        FieldEdits {
            set: pairs
                .iter()
                .map(|entry| (entry.0.to_owned(), entry.1.clone()))
                .collect(),
            ..Default::default()
        }
    }

    /// An `add`-only request.
    fn add_request(pairs: &[(&str, i64)]) -> FieldEdits {
        FieldEdits {
            add: pairs
                .iter()
                .map(|entry| (entry.0.to_owned(), entry.1))
                .collect(),
            ..Default::default()
        }
    }

    /// A request carrying both `set` and `add` fields.
    fn mixed(set_pairs: &[(&str, Value)], add_pairs: &[(&str, i64)]) -> FieldEdits {
        FieldEdits {
            set: set_request(set_pairs).set,
            add: add_request(add_pairs).add,
            ..Default::default()
        }
    }

    /// The sample map: two vertices, two sidedefs, two sectors.
    fn sample_map() -> Map {
        Map {
            vertices: vec![
                Vertex {
                    x: 0_i32,
                    y: 0_i32,
                    ..Default::default()
                },
                Vertex {
                    x: 128_i32,
                    y: 0_i32,
                    ..Default::default()
                },
            ],
            sidedefs: vec![
                Sidedef {
                    upper: "-".to_owned(),
                    middle: "STARTAN1".to_owned(),
                    lower: "BRICK1".to_owned(),
                    ..Default::default()
                },
                Sidedef {
                    upper: "-".to_owned(),
                    middle: "STARTAN1".to_owned(),
                    lower: "-".to_owned(),
                    x_offset: 8_i32,
                    y_offset: -8_i32,
                    ..Default::default()
                },
            ],
            sectors: vec![
                Sector {
                    floor_height: 0_i32,
                    ceiling_height: 128_i32,
                    floor_flat: "FLOOR4_8".to_owned(),
                    ceiling_flat: "CEIL5".to_owned(),
                    light: 160_i32,
                    ..Default::default()
                },
                Sector {
                    floor_height: 32_i32,
                    ceiling_height: 128_i32,
                    floor_flat: "FLAT1".to_owned(),
                    ceiling_flat: "FLAT1".to_owned(),
                    light: 192_i32,
                    special: 9_i32,
                    tag: 1_i32,
                    ..Default::default()
                },
            ],
            ..Map::default()
        }
    }

    /// The stored value of `key` on `row`, read through its serialization.
    fn stored<R: Serialize>(row: &R, key: &str) -> Value {
        serde_json::to_value(row)
            .expect("model rows serialize")
            .get(key)
            .expect("test rows carry the field")
            .clone()
    }

    /// Every settable sector field, on a fresh copy of the map.
    #[test]
    fn every_settable_sector_field_applies() {
        let stock = Stock::new();
        let scope = stock.doom();
        for (field, key, before, value) in [
            ("ceiling", "ceiling_height", json!(128_i64), json!(200_i64)),
            (
                "ceiling_flat",
                "ceiling_flat",
                json!("CEIL5"),
                json!("FLAT1"),
            ),
            ("floor", "floor_height", json!(0_i64), json!(8_i64)),
            (
                "floor_flat",
                "floor_flat",
                json!("FLOOR4_8"),
                json!("CEIL5"),
            ),
            ("light", "light", json!(160_i64), json!(64_i64)),
            ("special", "special", json!(0_i64), json!(11_i64)),
            ("tag", "tag", json!(0_i64), json!(5_i64)),
        ] {
            let mut map = sample_map();
            let applied = apply(
                &mut map,
                ElementKind::Sectors,
                &[0],
                &set_request(&[(field, value.clone())]),
                &scope,
            )
            .expect("a table field sets");
            assert_eq!(applied.matched, 1_usize, "{field} matched the row");
            assert_eq!(applied.changed, 1_usize, "{field} changed the row");
            assert_eq!(applied.indices, vec![0_usize], "{field} is listed");
            assert_eq!(stored(&map.sectors[0], key), value, "{field} landed");
            let row = applied.preview.first().expect("one preview row");
            assert_eq!(
                row.before.get(field).expect("the old value"),
                &before,
                "{field} previews its past"
            );
            assert_eq!(
                row.after.get(field).expect("the new value"),
                &value,
                "{field} previews its future"
            );
        }
    }

    /// Every settable sidedef field, on a fresh copy of the map.
    #[test]
    fn every_settable_sidedef_field_applies() {
        let stock = Stock::new();
        let scope = stock.doom();
        for (field, before, value) in [
            ("lower", json!("BRICK1"), json!("-")),
            ("middle", json!("STARTAN1"), json!("BRICK1")),
            ("upper", json!("-"), json!("STARTAN1")),
            ("x_offset", json!(0_i64), json!(8_i64)),
            ("y_offset", json!(0_i64), json!(-8_i64)),
        ] {
            let mut map = sample_map();
            let applied = apply(
                &mut map,
                ElementKind::Sidedefs,
                &[0],
                &set_request(&[(field, value.clone())]),
                &scope,
            )
            .expect("a table field sets");
            assert_eq!(applied.changed, 1_usize, "{field} changed the row");
            assert_eq!(stored(&map.sidedefs[0], field), value, "{field} landed");
            let row = applied.preview.first().expect("one preview row");
            assert_eq!(
                row.before.get(field).expect("the old value"),
                &before,
                "{field} previews its past"
            );
        }
    }

    /// `add` shifts both coordinates of every selected vertex.
    #[test]
    fn vertices_shift_only_by_add() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Vertices,
            &[1, 0],
            &add_request(&[("x", 16_i64), ("y", -16_i64)]),
            &scope,
        )
        .expect("adds slide the sheet");
        assert_eq!(applied.matched, 2_usize, "both vertices");
        assert_eq!(applied.changed, 2_usize, "both moved");
        assert_eq!(applied.indices, vec![0_usize, 1], "reported ascending");
        assert_eq!(
            stored(&map.vertices[0], "x"),
            json!(16_i64),
            "the first slid"
        );
        assert_eq!(stored(&map.vertices[0], "y"), json!(-16_i64), "and down");
        assert_eq!(
            stored(&map.vertices[1], "x"),
            json!(144_i64),
            "the second slid too"
        );
        assert_eq!(
            applied.preview[1].before,
            json!({"x": 128_i64, "y": 0_i64}),
            "the preview shows the past"
        );
        assert_eq!(
            applied.preview[1].after,
            json!({"x": 144_i64, "y": -16_i64}),
            "and the future"
        );
    }

    /// `set` and `add` ride one pass over the same sector.
    #[test]
    fn add_and_set_mix_on_one_sector() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &mixed(
                &[("light", json!(96_i64))],
                &[("floor", 16_i64), ("tag", 2_i64)],
            ),
            &scope,
        )
        .expect("set and add mix");
        assert_eq!(applied.changed, 1_usize, "one element changed");
        assert_eq!(applied.matched, 1_usize, "one element matched");
        let sector = &map.sectors[0];
        assert_eq!(
            (
                sector.floor_height,
                sector.ceiling_height,
                sector.light,
                sector.tag
            ),
            (16_i32, 128_i32, 96_i32, 2_i32),
            "the set light and the added floor and tag all landed"
        );
        let row = applied.preview.first().expect("one preview row");
        assert_eq!(
            row.before,
            json!({"floor": 0_i64, "light": 160_i64, "tag": 0_i64}),
            "every touched field previews"
        );
        assert_eq!(
            row.after,
            json!({"floor": 16_i64, "light": 96_i64, "tag": 2_i64}),
            "with its future"
        );
    }

    /// Faces take stock names and always `"-"`; unstocked names refuse.
    #[test]
    fn face_names_need_stock_or_the_dash() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Sidedefs,
            &[0],
            &set_request(&[("upper", json!("-")), ("middle", json!("BRICK1"))]),
            &scope,
        )
        .expect("\"-\" is always legal on a face");
        assert_eq!(applied.changed, 1_usize, "only the moved face counts");
        assert_eq!(map.sidedefs[0].upper, "-", "the face already \"-\" stayed");
        assert_eq!(map.sidedefs[0].middle, "BRICK1", "the stock name landed");
        let mut other = sample_map();
        let error = apply(
            &mut other,
            ElementKind::Sidedefs,
            &[0],
            &set_request(&[("lower", json!("NOPE1"))]),
            &scope,
        )
        .expect_err("an unstocked face name is missing");
        assert_eq!(error.code(), "not_found", "missing stock is not_found");
        assert!(
            error.to_string().contains("allow_missing"),
            "the refusal names the escape hatch: {error}"
        );
        assert_eq!(other, sample_map(), "nothing was written");
    }

    /// `"-"` names no flat: it is not exempt on the sector surfaces.
    #[test]
    fn flat_dash_is_not_exempt() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &set_request(&[("floor_flat", json!("-"))]),
            &scope,
        )
        .expect_err("\"-\" is no floor");
        assert_eq!(error.code(), "not_found", "the missing flat is not_found");
        assert!(
            error.to_string().contains("floor_flat"),
            "and named: {error}"
        );
        assert_eq!(map, sample_map(), "nothing was written");
    }

    /// `allow_missing` writes names the stock does not hold, uppercased.
    #[test]
    fn allow_missing_writes_unknown_names() {
        let stock = Stock::new();
        let scope = stock.permissive();
        let mut map = sample_map();
        apply(
            &mut map,
            ElementKind::Sectors,
            &[1],
            &set_request(&[("floor_flat", json!("slime13"))]),
            &scope,
        )
        .expect("the escape hatch lets the name through");
        assert_eq!(map.sectors[1].floor_flat, "SLIME13", "stored uppercase");
        let mut side = sample_map();
        apply(
            &mut side,
            ElementKind::Sidedefs,
            &[0],
            &set_request(&[("upper", json!("nope1"))]),
            &scope,
        )
        .expect("and so for a face");
        assert_eq!(side.sidedefs[0].upper, "NOPE1", "the face too");
        assert_eq!(
            stored(&map.sectors[1], "floor_flat"),
            json!("SLIME13"),
            "the preview-relevant row value is the stored one"
        );
    }

    /// Out-of-range results refuse with the rule, writing nothing.
    #[test]
    fn out_of_range_values_refuse_and_write_nothing() {
        let stock = Stock::new();
        let scope = stock.doom();
        for (request, index, needle) in [
            (
                set_request(&[("light", json!(-40_i64))]),
                1_usize,
                "sector 1 light would become -40; light must stay within 0..=255",
            ),
            (
                add_request(&[("light", 100_i64)]),
                0_usize,
                "sector 0 light would become 260; light must stay within 0..=255",
            ),
            (
                set_request(&[("ceiling", json!(0x8000_i64))]),
                0_usize,
                "sector 0 ceiling would become 32768; ceiling must stay within -32768..=32767",
            ),
            (
                set_request(&[("floor", json!(200_i64)), ("ceiling", json!(64_i64))]),
                0_usize,
                "sector 0 would end with floor 200 above ceiling 64",
            ),
        ] {
            let mut map = sample_map();
            let error = apply(&mut map, ElementKind::Sectors, &[index], &request, &scope)
                .expect_err("the range rule bites before any write");
            assert_eq!(error.code(), "invalid_argument", "code for {needle}");
            assert!(
                error.to_string().contains(needle),
                "the message says \"{needle}\": {error}"
            );
            assert_eq!(map, sample_map(), "the map is untouched: {needle}");
        }
    }

    /// A vertex coordinate leaving the binary `i16` range refuses.
    #[test]
    fn vertex_coordinates_keep_the_binary_range() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let error = apply(
            &mut map,
            ElementKind::Vertices,
            &[1],
            &add_request(&[("x", 32700_i64)]),
            &scope,
        )
        .expect_err("128 + 32700 leaves i16");
        assert_eq!(error.code(), "invalid_argument", "out of i16");
        assert!(
            error
                .to_string()
                .contains("vertex 1 x would become 32828; x must stay within -32768..=32767"),
            "the refusal names row, field and range: {error}"
        );
        assert_eq!(map, sample_map(), "nothing was written");
    }

    /// One bad row holds back the whole selection, even good fields.
    #[test]
    fn one_bad_row_holds_back_the_whole_selection() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0, 1],
            &mixed(&[("light", json!(32_i64))], &[("floor", 100_i64)]),
            &scope,
        )
        .expect_err("sector 1 would end 132 over 128");
        assert!(
            error
                .to_string()
                .contains("sector 1 would end with floor 132 above ceiling 128"),
            "the refusal names the offending row: {error}"
        );
        assert_eq!(
            map,
            sample_map(),
            "the legal row 0 and the light set stand untouched"
        );
    }

    /// `set` on vertices is refused, naming the `add`-able fields.
    #[test]
    fn vertices_support_add_only_never_set() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let error = apply(
            &mut map,
            ElementKind::Vertices,
            &[0],
            &set_request(&[("x", json!(5_i64))]),
            &scope,
        )
        .expect_err("a vertex is moved, not set");
        assert_eq!(error.code(), "invalid_argument", "a vertex set is refused");
        let message = error.to_string();
        assert!(
            message.contains("add") && message.contains("x, y"),
            "the refusal lists what vertices take: {message}"
        );
        assert_eq!(map, sample_map(), "nothing was written");
    }

    /// Unknown and un-addable field names come back as the kind's table.
    #[test]
    fn unknown_fields_list_the_valid_ones() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let sector_error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &set_request(&[("roof", json!(1_i64))]),
            &scope,
        )
        .expect_err("sectors have no roof");
        assert!(
            sector_error.to_string().contains(
                "valid fields are ceiling, ceiling_flat, floor, floor_flat, light, \
                 special, tag"
            ),
            "the sector table is listed: {sector_error}"
        );
        let side_error = apply(
            &mut map,
            ElementKind::Sidedefs,
            &[0],
            &add_request(&[("sector", 1_i64)]),
            &scope,
        )
        .expect_err("sidedef fields are not add targets here");
        assert!(
            side_error
                .to_string()
                .contains("valid fields are lower, middle, upper, x_offset, y_offset"),
            "the sidedef table is listed: {side_error}"
        );
        let special_error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &add_request(&[("special", 1_i64)]),
            &scope,
        )
        .expect_err("special takes no amount");
        assert!(
            special_error
                .to_string()
                .contains("not addable on sectors: addable fields are ceiling, floor, light, tag"),
            "the addable table is listed: {special_error}"
        );
        assert_eq!(map, sample_map(), "nothing was written");
    }

    /// One field cannot carry a `set` value and an `add` amount at once.
    #[test]
    fn a_field_cannot_be_set_and_added_at_once() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &mixed(&[("light", json!(64_i64))], &[("light", 8_i64)]),
            &scope,
        )
        .expect_err("one field, two fates");
        assert_eq!(error.code(), "invalid_argument", "the clash refuses");
        assert!(
            error
                .to_string()
                .contains("carries both a set value and an add amount"),
            "and says so: {error}"
        );
        assert_eq!(map, sample_map(), "nothing was written");
    }

    /// These three kinds store no flags: flag names refuse with a pointer.
    #[test]
    fn flags_belong_to_linedefs_and_things() {
        let stock = Stock::new();
        let scope = stock.doom();
        let request = FieldEdits {
            flags_set: vec!["blocking".to_owned()],
            ..Default::default()
        };
        for kind in [
            ElementKind::Sectors,
            ElementKind::Sidedefs,
            ElementKind::Vertices,
        ] {
            let mut map = sample_map();
            let error = apply(&mut map, kind, &[0], &request, &scope)
                .expect_err("these kinds store no flags");
            assert_eq!(
                error.code(),
                "invalid_argument",
                "flag refusal is invalid_argument"
            );
            assert!(
                error.to_string().contains("no flags field"),
                "the refusal says so for {}: {error}",
                kind.as_str()
            );
            assert_eq!(map, sample_map(), "nothing was written");
        }
    }

    /// Linedefs and things refuse by name until the next slice's engines.
    #[test]
    fn linedefs_and_things_refuse_with_their_coming_fields() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let before = map.clone();
        let request = set_request(&[("light", json!(1_i64))]);
        let line_error = apply(&mut map, ElementKind::Linedefs, &[0], &request, &scope)
            .expect_err("linedef support lands next");
        assert_eq!(
            line_error.code(),
            "invalid_argument",
            "a linedef selection refuses"
        );
        let line_message = line_error.to_string();
        assert!(
            line_message.contains("flags")
                && line_message.contains("args")
                && line_message.contains("edit_linedef"),
            "the refusal names the coming fields and the way forward: {line_message}"
        );
        let spawn_error = apply(&mut map, ElementKind::Things, &[0], &request, &scope)
            .expect_err("thing support lands next");
        let spawn_message = spawn_error.to_string();
        assert!(
            spawn_message.contains("angle") && spawn_message.contains("edit_thing"),
            "and the same for things: {spawn_message}"
        );
        assert_eq!(map, before, "nothing was written");
    }

    /// The report: matched counts the selection, changed counts moves.
    #[test]
    fn the_report_counts_matches_and_previews_only_changes() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Sectors,
            &[1, 0, 1],
            &set_request(&[("light", json!(160_i64))]),
            &scope,
        )
        .expect("an in-range set applies");
        assert_eq!(
            applied.matched, 2_usize,
            "duplicates collapse into the two rows"
        );
        assert_eq!(
            applied.changed, 1_usize,
            "a no-op counts as matched, not changed"
        );
        assert_eq!(
            applied.indices,
            vec![1_usize],
            "only the moved row is listed"
        );
        assert_eq!(
            applied.preview,
            vec![Change {
                index: 1_usize,
                before: json!({"light": 192_i64}),
                after: json!({"light": 160_i64}),
            }],
            "the preview names the field it moved"
        );
        assert_eq!(
            map.sectors[0].light, 160_i32,
            "the no-op row kept its light"
        );
        assert_eq!(
            map.sectors[1].light, 160_i32,
            "the changed row took the new one"
        );
    }

    /// An empty selection succeeds with zeroes; an empty request refuses.
    #[test]
    fn empty_selections_and_requests_are_answered_at_the_gate() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Sectors,
            &[],
            &set_request(&[("light", json!(96_i64))]),
            &scope,
        )
        .expect("an empty selection is a successful nothing");
        assert_eq!(applied, Applied::default(), "zeroes all round");
        assert_eq!(map, sample_map(), "nothing was written");
        let error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &FieldEdits::default(),
            &scope,
        )
        .expect_err("an empty request is no edit");
        assert!(
            error.to_string().contains("nothing to change"),
            "and says so: {error}"
        );
    }

    /// `is_empty` reads all four parts of the request.
    #[test]
    fn is_empty_reads_every_part_of_the_request() {
        assert!(
            FieldEdits::default().is_empty(),
            "the default asks for nothing"
        );
        assert!(
            !set_request(&[("light", json!(0_i64))]).is_empty(),
            "even a zero set asks"
        );
        assert!(
            !add_request(&[("light", 0_i64)]).is_empty(),
            "and a zero add asks"
        );
        let flags = FieldEdits {
            flags_clear: vec!["secret".to_owned()],
            ..Default::default()
        };
        assert!(!flags.is_empty(), "one flag name is a request");
    }

    /// An index past the block is `not_found`, naming the block's size.
    #[test]
    fn indices_the_blocks_do_not_hold_are_not_found() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let error = apply(
            &mut map,
            ElementKind::Sectors,
            &[2],
            &set_request(&[("light", json!(160_i64))]),
            &scope,
        )
        .expect_err("there is no third sector");
        assert_eq!(error.code(), "not_found", "past the end is not_found");
        assert!(
            error
                .to_string()
                .contains("sector 2 does not exist (the map holds 2 sectors)"),
            "the message counts the block: {error}"
        );
        let vertex_error = apply(
            &mut map,
            ElementKind::Vertices,
            &[5],
            &add_request(&[("x", 1_i64)]),
            &scope,
        )
        .expect_err("there is no sixth vertex");
        assert!(
            vertex_error.to_string().contains("vertex 5"),
            "and names the row: {vertex_error}"
        );
        assert_eq!(map, sample_map(), "nothing was written");
    }

    /// Wrong value types refuse before the selection is even read.
    #[test]
    fn value_types_are_checked_before_the_selection_runs() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let light_error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &set_request(&[("light", json!("160"))]),
            &scope,
        )
        .expect_err("a string is no light level");
        assert_eq!(
            light_error.code(),
            "invalid_argument",
            "a wrong type refuses"
        );
        assert!(
            light_error.to_string().contains("needs an integer value"),
            "the refusal says so: {light_error}"
        );
        let fraction_error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &set_request(&[("floor", json!(1.5_f64))]),
            &scope,
        )
        .expect_err("a fraction is no height");
        assert!(
            fraction_error.to_string().contains("'floor'"),
            "the field is named: {fraction_error}"
        );
        let name_error = apply(
            &mut map,
            ElementKind::Sidedefs,
            &[0],
            &set_request(&[("middle", json!(7_i64))]),
            &scope,
        )
        .expect_err("a number is no texture name");
        assert!(
            name_error.to_string().contains("needs a name string"),
            "faces want names: {name_error}"
        );
        let gate_error = apply(
            &mut map,
            ElementKind::Sectors,
            &[],
            &set_request(&[("light", json!("160"))]),
            &scope,
        )
        .expect_err("bad types refuse even with nothing selected");
        assert!(
            gate_error.to_string().contains("'light'"),
            "and the field again: {gate_error}"
        );
        assert_eq!(map, sample_map(), "nothing was ever written");
    }

    /// An `add` amount that leaves the integers refuses.
    #[test]
    fn an_add_that_leaves_the_integers_is_refused() {
        let stock = Stock::new();
        let scope = stock.doom();
        let mut map = sample_map();
        let error = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &add_request(&[("light", i64::MAX)]),
            &scope,
        )
        .expect_err("160 + i64::MAX has no integer answer");
        assert_eq!(error.code(), "invalid_argument", "overflow refuses");
        assert!(
            error.to_string().contains("leaves the range of integers"),
            "the refusal says so: {error}"
        );
        assert_eq!(map, sample_map(), "nothing was written");
    }

    /// `UDMF` carries no binary caps; the light rule stays universal.
    #[test]
    fn udmf_maps_ignore_the_binary_ranges() {
        let stock = Stock::new();
        let scope = stock.udmf();
        let mut tall = sample_map();
        tall.format = MapFormat::Udmf;
        let applied = apply(
            &mut tall,
            ElementKind::Sectors,
            &[0],
            &set_request(&[("ceiling", json!(40000_i64)), ("tag", json!(-5_i64))]),
            &scope,
        )
        .expect("UDMF holds what an i16 column cannot");
        assert_eq!(applied.changed, 1_usize, "both fields rode one pass");
        assert_eq!(
            tall.sectors[0].ceiling_height, 40000_i32,
            "the height landed"
        );
        assert_eq!(tall.sectors[0].tag, -5_i32, "and the negative tag");
        let mut bright = sample_map();
        bright.format = MapFormat::Udmf;
        let error = apply(
            &mut bright,
            ElementKind::Sectors,
            &[0],
            &set_request(&[("light", json!(300_i64))]),
            &scope,
        )
        .expect_err("the light level stays vanilla everywhere");
        assert!(
            error.to_string().contains("light must stay within 0..=255"),
            "the light rule is universal: {error}"
        );
        let mut standing = sample_map();
        standing.format = MapFormat::Udmf;
        assert_eq!(bright, standing, "the map stands");
    }

    /// The wire shape of [`FieldEdits`], as the next slice's tool sends it.
    #[test]
    fn field_edits_deserialize_from_the_tool_shape() {
        let parsed: FieldEdits = from_str(
            r#"{"set": {"light": 64, "floor_flat": "FLAT1"}, "add": {"tag": 2},
                "flags_set": ["mapped"], "flags_clear": []}"#,
        )
        .expect("the wire shape parses");
        assert_eq!(parsed.set.len(), 2_usize, "two set fields");
        assert_eq!(
            parsed.set.get("light"),
            Some(&json!(64_i64)),
            "numbers ride as values"
        );
        assert_eq!(parsed.add.get("tag"), Some(&2_i64), "amounts land in add");
        assert_eq!(
            parsed.flags_set,
            vec!["mapped".to_owned()],
            "names in flags_set"
        );
        assert!(!parsed.is_empty(), "a full request is not empty");
        let bare: FieldEdits = from_str("{}").expect("every part defaults");
        assert!(bare.is_empty(), "and so the empty object is empty");
    }
}
