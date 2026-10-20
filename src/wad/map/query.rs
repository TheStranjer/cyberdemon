//! The pure query engine: filter a [`Map`]'s sectors, linedefs, sidedefs,
//! things and vertices by stored fields, derived values and a bounding
//! region, and answer with the sorted element indices.
//!
//! A [`Query`] names an [`ElementKind`], a list of [`Filter`]s that are
//! `ANDed` together, and an optional [`Within`] region. Every filter reads one
//! field of one element, compares it with an [`Op`] and a `JSON` value, and
//! either keeps or drops the element. The answer is the surviving indices
//! in ascending order; bulk-edit tools (CD-108) apply to exactly that list.
//!
//! Like [`describe`] and [`geometry`](super::geometry) this is a function of
//! the map model and the passed [`QueryContext`] alone: no sessions, no
//! `MCP`, no editing. The context carries the flag family (so `flags has`
//! reads the [`flags`] tables) and the game definitions (so thing `name`
//! and `category` resolve, and the derived sector fields reuse
//! [`Describe`]). Derived fields that do not resolve — the back side of a
//! one-sided line, a thing standing in the void, a line with a dangling
//! vertex — match no filter, `ne`, `not_in` and `has_not` included.
//!
//! The field tables:
//!
//! | kind | fields |
//! |------|--------|
//! | `sectors` | `index, floor, ceiling, height*, light, floor_flat, ceiling_flat, special, tag, area*, roles*` |
//! | `linedefs` | `index, v1, v2, flags, special, front_sector*, back_sector*, two_sided*, length*, front_upper*, front_middle*, front_lower*, back_upper*, back_middle*, back_lower*` plus `tag` (`Doom` family) or `arg0..arg4` (`Hexen` family) |
//! | `sidedefs` | `index, sector, upper, middle, lower, x_offset, y_offset, linedef*` |
//! | `things` | `index, type, x, y, angle, flags, name*, category*, sector*` plus `tid, special` (`Hexen` family) |
//! | `vertices` | `index, x, y` |
//!
//! (`*` marks derived fields.) Numeric fields take `eq ne lt le gt ge in
//! not_in`; text fields take `eq ne in not_in glob` (`glob` is
//! case-insensitive `*`/`?` matching); `two_sided` takes `eq ne in not_in`;
//! `flags` and `roles` take only `has` and `has_not`. A comparison value
//! must fit its field: an integer for numbers, a string for text, a boolean
//! for `two_sided`, an array for `in`/`not_in` and a flag or role name for
//! `has`/`has_not`. Anything else — and any unknown field — fails with
//! [`CyberError::InvalidArgument`] naming the valid options.
//!
//! `within` restricts by position or membership: a `bbox`
//! `[min_x, min_y, max_x, max_y]` (inclusive) keeps things and vertices at
//! points inside the box, sectors whose `center` lies inside it, linedefs
//! with **both** vertices inside it, and sidedefs whose owning line meets
//! that rule; a `sectors` index set keeps things and vertices standing in
//! one of them ([`point_in_sector`]), sectors by index, linedefs with a
//! resolvable side in one of them, and sidedefs opening into one of them.
//! Indices outside the sector block simply match nothing.
//!
//! [`flags`]: super::flags
//! [`describe`]: super::describe
//! [`Describe`]: super::describe::Describe

use alloc::borrow::Cow;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::describe::{Describe, ROLE_NAMES, SectorFacts};
use super::flags::{FlagFamily, FlagKind, names, to_names};
use super::geometry::{linedef_length, point_in_sector};
use super::model::{Linedef, Map, Thing};
use crate::defs::{Definitions, ThingDef};
use crate::error::CyberError;
use crate::wad::name::glob_match;

/// The element block a [`Query`] runs over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    /// Rows of [`Map::sectors`].
    Sectors,
    /// Rows of [`Map::linedefs`].
    Linedefs,
    /// Rows of [`Map::sidedefs`].
    Sidedefs,
    /// Rows of [`Map::things`].
    Things,
    /// Rows of [`Map::vertices`].
    Vertices,
}

impl ElementKind {
    /// Every kind, in report order.
    pub const ALL: [Self; 5] = [
        Self::Sectors,
        Self::Linedefs,
        Self::Sidedefs,
        Self::Things,
        Self::Vertices,
    ];

    /// The lowercase wire name of this kind.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sectors => "sectors",
            Self::Linedefs => "linedefs",
            Self::Sidedefs => "sidedefs",
            Self::Things => "things",
            Self::Vertices => "vertices",
        }
    }

    /// How many rows of this kind `map` holds.
    #[must_use]
    #[inline]
    pub fn count_on(self, map: &Map) -> usize {
        match self {
            Self::Sectors => map.sectors.len(),
            Self::Linedefs => map.linedefs.len(),
            Self::Sidedefs => map.sidedefs.len(),
            Self::Things => map.things.len(),
            Self::Vertices => map.vertices.len(),
        }
    }
}

/// How a [`Filter`] compares its field with its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// The field equals the value.
    Eq,
    /// The field differs from the value.
    Ne,
    /// The (numeric) field is below the value.
    Lt,
    /// The (numeric) field is at or below the value.
    Le,
    /// The (numeric) field is above the value.
    Gt,
    /// The (numeric) field is at or above the value.
    Ge,
    /// The field equals one value of the array.
    In,
    /// The field equals none of the values of the array.
    NotIn,
    /// The (text) field matches the case-insensitive `*`/`?` pattern.
    Glob,
    /// The flags field carries the named flag (or `roles` the named role).
    Has,
    /// The flags field does not carry the named flag (or `roles` the role).
    HasNot,
}

/// One field comparison of a [`Query`].
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct Filter {
    /// The field to read; the valid names depend on the query's kind.
    pub field: String,
    /// The comparison to make.
    pub op: Op,
    /// The value to compare against: an integer for numeric fields, a
    /// string for text fields and `glob`, a boolean for `two_sided`, an
    /// array for `in` and `not_in`, a flag or role name for `has`.
    pub value: Value,
}

/// The region a [`Query`] keeps elements from.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", untagged)]
pub enum Within {
    /// Keep elements inside an inclusive bounding box.
    Bbox {
        /// The box as `[min_x, min_y, max_x, max_y]` in map units.
        bbox: [i64; 4],
    },
    /// Keep elements belonging to, or standing in, one of these sectors.
    Sectors {
        /// The sector indices to bound by.
        sectors: Vec<usize>,
    },
}

/// A whole query: which elements, filtered how, bounded where.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct Query {
    /// The element block to run over.
    pub kind: ElementKind,
    /// The filters to combine with `AND`; an empty list keeps every bounded
    /// element.
    #[serde(default, rename = "where")]
    pub r#where: Vec<Filter>,
    /// The region to bound the hits by; `None` bounds nothing.
    #[serde(default)]
    pub within: Option<Within>,
}

/// A query's `within` clause, resolved for repeated element tests.
struct Bounds {
    /// The positional box, when the clause is a `bbox`.
    bbox: Option<[i64; 4]>,
    /// Whether the clause bounds by sector membership at all.
    sectors: bool,
    /// The membership set, built when `sectors`.
    set: BTreeSet<usize>,
}

/// Resolve the `within` clause once per query; the clone of an index list
/// bounded by the sector count is what keeps this cheap and the match plain.
fn resolve_bounds(within: Option<&Within>) -> Bounds {
    match within.cloned() {
        None => Bounds {
            bbox: None,
            sectors: false,
            set: BTreeSet::new(),
        },
        Some(Within::Bbox { bbox }) => Bounds {
            bbox: Some(bbox),
            sectors: false,
            set: BTreeSet::new(),
        },
        Some(Within::Sectors { sectors }) => Bounds {
            bbox: None,
            sectors: true,
            set: sectors.into_iter().collect(),
        },
    }
}

/// What a pure query run reads beyond the map itself.
#[derive(Clone, Debug)]
pub struct QueryContext<'defs> {
    /// The flag family the map's lines and things store their bits in.
    pub family: FlagFamily,
    /// The game definitions naming thing types for `name` and `category`.
    pub defs: &'defs Definitions,
}

impl<'defs> QueryContext<'defs> {
    /// A context reading flags in the `family`'s tables and things through
    /// the game definitions `defs`.
    #[must_use]
    #[inline]
    pub const fn new(family: FlagFamily, defs: &'defs Definitions) -> Self {
        Self { family, defs }
    }

    /// The definition display name of thing type `kind`, `Unknown (n)` for
    /// a type the game does not define.
    #[must_use]
    #[inline]
    pub fn thing_name(&self, kind: i32) -> String {
        self.thing_row(kind)
            .map_or_else(|| format!("Unknown ({kind})"), |row| row.name.clone())
    }

    /// The definition category of thing type `kind`, `Unknown` for a type
    /// the game does not define.
    #[must_use]
    #[inline]
    pub fn thing_category(&self, kind: i32) -> String {
        self.thing_row(kind)
            .map_or_else(|| "Unknown".to_owned(), |row| row.category.clone())
    }

    /// The definition row of thing type `kind`, `None` for a negative type
    /// or one the game does not define.
    fn thing_row(&self, kind: i32) -> Option<&'defs ThingDef> {
        u32::try_from(kind)
            .ok()
            .and_then(|number| self.defs.thing(number))
    }
}

/// The sector index the side `side` opens into: `None` when the side is
/// absent, its sidedef index dangles, or that sidedef's sector index
/// dangles. The `find_elements` rows and later bulk edits share this read.
#[must_use]
#[inline]
pub fn side_sector(map: &Map, side: Option<usize>) -> Option<usize> {
    let row = side.and_then(|index| map.sidedefs.get(index))?;
    map.sectors.get(row.sector).map(|_| row.sector)
}

/// Run `query` over `map`: the ascending indices of every element of the
/// queried kind that lies within the bound and passes every filter.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a field the kind does not carry (the
/// message lists the valid fields), an op the field's type does not take, a
/// comparison value of the wrong shape, or an unknown flag or role name.
/// Filters are checked before any element is read, so a bad query fails the
/// same way over an empty map.
#[inline]
pub fn run_query<'map>(
    map: &'map Map,
    ctx: &QueryContext<'map>,
    query: &Query,
) -> Result<Vec<usize>, CyberError> {
    let engine = Engine::new(map, ctx, query)?;
    let mut found = Vec::new();
    for index in 0..engine.count() {
        if engine.matches(index) {
            found.push(index);
        }
    }
    Ok(found)
}

/// The comparable shape one field reads as.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldType {
    /// An integer: a stored number, an index or a derived measure.
    Number,
    /// A name: a flat, texture, definition name or category.
    Text,
    /// A derived boolean.
    Boolean,
    /// A raw bitset read through the linedef flag table.
    LineFlags,
    /// A raw bitset read through the thing flag table.
    ThingFlags,
    /// A sector's role names.
    Roles,
}

/// One queryable field: its wire name and comparable shape.
struct FieldSpec {
    /// The name a filter spells to read this field.
    name: &'static str,
    /// How its value reads.
    ty: FieldType,
}

/// One table row, for writing the tables below.
const fn field(name: &'static str, ty: FieldType) -> FieldSpec {
    FieldSpec { name, ty }
}

/// The fields of every sector.
const SECTOR_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("floor", FieldType::Number),
    field("ceiling", FieldType::Number),
    field("height", FieldType::Number),
    field("light", FieldType::Number),
    field("floor_flat", FieldType::Text),
    field("ceiling_flat", FieldType::Text),
    field("special", FieldType::Number),
    field("tag", FieldType::Number),
    field("area", FieldType::Number),
    field("roles", FieldType::Roles),
];

/// The linedef fields both families share.
const LINEDEF_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("v1", FieldType::Number),
    field("v2", FieldType::Number),
    field("flags", FieldType::LineFlags),
    field("special", FieldType::Number),
    field("front_sector", FieldType::Number),
    field("back_sector", FieldType::Number),
    field("two_sided", FieldType::Boolean),
    field("length", FieldType::Number),
    field("front_upper", FieldType::Text),
    field("front_middle", FieldType::Text),
    field("front_lower", FieldType::Text),
    field("back_upper", FieldType::Text),
    field("back_middle", FieldType::Text),
    field("back_lower", FieldType::Text),
];

/// The `Doom` family's extra linedef field: the trigger tag.
const LINEDEF_DOOM_EXTRA: &[FieldSpec] = &[field("tag", FieldType::Number)];

/// The `Hexen` family's extra linedef fields: the script arguments.
const LINEDEF_HEXEN_EXTRA: &[FieldSpec] = &[
    field("arg0", FieldType::Number),
    field("arg1", FieldType::Number),
    field("arg2", FieldType::Number),
    field("arg3", FieldType::Number),
    field("arg4", FieldType::Number),
];

/// The fields of every sidedef.
const SIDEDEF_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("sector", FieldType::Number),
    field("upper", FieldType::Text),
    field("middle", FieldType::Text),
    field("lower", FieldType::Text),
    field("x_offset", FieldType::Number),
    field("y_offset", FieldType::Number),
    field("linedef", FieldType::Number),
];

/// The thing fields both families share.
const THING_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("type", FieldType::Number),
    field("name", FieldType::Text),
    field("category", FieldType::Text),
    field("x", FieldType::Number),
    field("y", FieldType::Number),
    field("angle", FieldType::Number),
    field("flags", FieldType::ThingFlags),
    field("sector", FieldType::Number),
];

/// The `Hexen` family's extra thing fields.
const THING_HEXEN_EXTRA: &[FieldSpec] = &[
    field("tid", FieldType::Number),
    field("special", FieldType::Number),
];

/// The fields of every vertex.
const VERTEX_FIELDS: &[FieldSpec] = &[
    field("index", FieldType::Number),
    field("x", FieldType::Number),
    field("y", FieldType::Number),
];

/// The kind's own fields, the ones every family carries.
const fn base_fields(kind: ElementKind) -> &'static [FieldSpec] {
    match kind {
        ElementKind::Sectors => SECTOR_FIELDS,
        ElementKind::Linedefs => LINEDEF_FIELDS,
        ElementKind::Sidedefs => SIDEDEF_FIELDS,
        ElementKind::Things => THING_FIELDS,
        ElementKind::Vertices => VERTEX_FIELDS,
    }
}

/// The `Doom`/`Hexen`-only fields one kind carries.
const fn extra_fields(kind: ElementKind, family: FlagFamily) -> &'static [FieldSpec] {
    match (kind, family) {
        (ElementKind::Linedefs, FlagFamily::Doom) => LINEDEF_DOOM_EXTRA,
        (ElementKind::Linedefs, FlagFamily::Hexen) => LINEDEF_HEXEN_EXTRA,
        (ElementKind::Things, FlagFamily::Hexen) => THING_HEXEN_EXTRA,
        (ElementKind::Things, FlagFamily::Doom)
        | (
            ElementKind::Sectors | ElementKind::Sidedefs | ElementKind::Vertices,
            FlagFamily::Doom | FlagFamily::Hexen,
        ) => &[],
    }
}

/// The field named `name` in the kind's table, `None` when it has none.
fn field_spec(kind: ElementKind, family: FlagFamily, name: &str) -> Option<&'static FieldSpec> {
    base_fields(kind)
        .iter()
        .chain(extra_fields(kind, family))
        .find(|spec| spec.name == name)
}

/// The valid field names of one kind and family, comma-joined for errors.
fn valid_fields(kind: ElementKind, family: FlagFamily) -> String {
    base_fields(kind)
        .iter()
        .chain(extra_fields(kind, family))
        .map(|spec| spec.name)
        .collect::<Vec<&str>>()
        .join(", ")
}

/// A field value read off one element, widened to comparable shapes.
enum FValue<'map> {
    /// A number.
    Num(i64),
    /// A name.
    Text(Cow<'map, str>),
    /// A derived boolean.
    Bool(bool),
    /// A flag bitset with the kind whose table names its bits.
    Bits(u32, FlagKind),
    /// A sector's role names.
    Roles(Vec<String>),
}

/// One filter checked against the field table, ready to compare.
struct Prepared<'req> {
    /// The field this filter reads.
    spec: &'static FieldSpec,
    /// The comparison to make.
    op: Op,
    /// The value to compare against.
    want: &'req Value,
}

/// The query's engine: the map, the checked filters and the derived reads
/// (the [`Describe`] facts, the sidedef owners) built only when asked.
struct Engine<'map, 'req> {
    /// The element block being queried.
    kind: ElementKind,
    /// The flag family the map stores its bits in.
    family: FlagFamily,
    /// The definitions context naming things.
    ctx: &'req QueryContext<'map>,
    /// The map being queried.
    map: &'map Map,
    /// The combined filters, already validated.
    filters: Vec<Prepared<'req>>,
    /// The resolved `within` bound.
    bounds: Bounds,
    /// The per-sector describe read, built only when derived sector fields
    /// or a sector `bbox` ask for it.
    describe: Option<Describe<'map>>,
    /// The sidedef-to-linedef owners, built only when the `linedef` field
    /// or a sidedef `bbox` ask for them.
    owners: Option<Vec<Option<usize>>>,
}

impl<'map, 'req> Engine<'map, 'req> {
    /// Check every filter against the kind's field table and prebuild the
    /// derived reads the query needs.
    fn new(
        map: &'map Map,
        ctx: &'req QueryContext<'map>,
        query: &'req Query,
    ) -> Result<Self, CyberError> {
        let mut filters = Vec::with_capacity(query.r#where.len());
        for filter in &query.r#where {
            filters.push(prepare_filter(query.kind, ctx.family, filter)?);
        }
        let bounds = resolve_bounds(query.within.as_ref());
        let bounded_by_box = bounds.bbox.is_some();
        Ok(Self {
            kind: query.kind,
            family: ctx.family,
            ctx,
            map,
            filters,
            bounds,
            describe: (query.kind == ElementKind::Sectors
                && (bounded_by_box || mentions(&query.r#where, &["area", "roles"])))
            .then(|| Describe::new(map, ctx.defs)),
            owners: (query.kind == ElementKind::Sidedefs
                && (bounded_by_box || mentions(&query.r#where, &["linedef"])))
            .then(|| build_owners(map)),
        })
    }

    /// How many elements of the queried kind the map holds.
    fn count(&self) -> usize {
        self.kind.count_on(self.map)
    }

    /// Whether element `index` lies within the bound and passes every
    /// filter.
    fn matches(&self, index: usize) -> bool {
        self.bounded(index)
            && self
                .filters
                .iter()
                .all(|filter| self.filter_hits(filter, index))
    }

    /// Whether element `index` passes one filter; a derived field that does
    /// not resolve matches nothing.
    fn filter_hits(&self, filter: &Prepared<'_>, index: usize) -> bool {
        self.value(index, filter.spec.name)
            .is_some_and(|value| compare(filter.op, self.family, value, filter.want))
    }

    /// Read one field of element `index`.
    fn value(&self, index: usize, name: &str) -> Option<FValue<'map>> {
        match self.kind {
            ElementKind::Sectors => self.sector_value(index, name),
            ElementKind::Linedefs => self.linedef_value(index, name),
            ElementKind::Sidedefs => self.sidedef_value(index, name),
            ElementKind::Things => self.thing_value(index, name),
            ElementKind::Vertices => self.vertex_value(index, name),
        }
    }

    /// One sector field; `area` and `roles` read the [`Describe`] this
    /// query built.
    fn sector_value(&self, index: usize, name: &str) -> Option<FValue<'map>> {
        let row = self.map.sectors.get(index)?;
        match name {
            "index" => Some(FValue::Num(wide_usize(index))),
            "floor" => Some(FValue::Num(i64::from(row.floor_height))),
            "ceiling" => Some(FValue::Num(i64::from(row.ceiling_height))),
            "height" => Some(FValue::Num(i64::from(
                row.ceiling_height.saturating_sub(row.floor_height),
            ))),
            "light" => Some(FValue::Num(i64::from(row.light))),
            "floor_flat" => Some(FValue::Text(Cow::Borrowed(row.floor_flat.as_str()))),
            "ceiling_flat" => Some(FValue::Text(Cow::Borrowed(row.ceiling_flat.as_str()))),
            "special" => Some(FValue::Num(i64::from(row.special))),
            "tag" => Some(FValue::Num(i64::from(row.tag))),
            "area" => self
                .facts(index)
                .map(|facts| FValue::Num(wide_u64(facts.area))),
            "roles" => self.facts(index).map(|facts| FValue::Roles(facts.roles)),
            _ => None,
        }
    }

    /// The described facts of sector `index`, when the query asked for a
    /// derived sector field.
    fn facts(&self, index: usize) -> Option<SectorFacts> {
        self.describe.as_ref()?.sector(index)
    }

    /// One linedef field; sides, length and faces resolve through the
    /// sidedefs and vertices.
    fn linedef_value(&self, index: usize, name: &str) -> Option<FValue<'map>> {
        let line = self.map.linedefs.get(index)?;
        match name {
            "index" => Some(FValue::Num(wide_usize(index))),
            "v1" => Some(FValue::Num(wide_usize(line.v1))),
            "v2" => Some(FValue::Num(wide_usize(line.v2))),
            "flags" => Some(FValue::Bits(line.flags, FlagKind::Linedef)),
            "special" => Some(FValue::Num(i64::from(line.special))),
            "tag" => Some(FValue::Num(i64::from(line.tag))),
            "front_sector" => self
                .side_sector(line.front)
                .map(wide_usize)
                .map(FValue::Num),
            "back_sector" => self.side_sector(line.back).map(wide_usize).map(FValue::Num),
            "two_sided" => Some(FValue::Bool(line.front.is_some() && line.back.is_some())),
            "length" => linedef_length(self.map, index)
                .map(wide_u64)
                .map(FValue::Num),
            "front_upper" => self.face(line.front, "upper"),
            "front_middle" => self.face(line.front, "middle"),
            "front_lower" => self.face(line.front, "lower"),
            "back_upper" => self.face(line.back, "upper"),
            "back_middle" => self.face(line.back, "middle"),
            "back_lower" => self.face(line.back, "lower"),
            other => line_arg(line, other).map(FValue::Num),
        }
    }

    /// The sector a resolvable side opens into.
    fn side_sector(&self, side: Option<usize>) -> Option<usize> {
        side_sector(self.map, side)
    }

    /// One texture face of the side `side`, `None` when the side is absent
    /// or its sidedef dangles.
    fn face(&self, side: Option<usize>, face: &str) -> Option<FValue<'map>> {
        let row = self.map.sidedefs.get(side?)?;
        let name = match face {
            "upper" => row.upper.as_str(),
            "middle" => row.middle.as_str(),
            "lower" => row.lower.as_str(),
            _ => return None,
        };
        Some(FValue::Text(Cow::Borrowed(name)))
    }

    /// One sidedef field.
    fn sidedef_value(&self, index: usize, name: &str) -> Option<FValue<'map>> {
        let row = self.map.sidedefs.get(index)?;
        match name {
            "index" => Some(FValue::Num(wide_usize(index))),
            "sector" => Some(FValue::Num(wide_usize(row.sector))),
            "upper" => Some(FValue::Text(Cow::Borrowed(row.upper.as_str()))),
            "middle" => Some(FValue::Text(Cow::Borrowed(row.middle.as_str()))),
            "lower" => Some(FValue::Text(Cow::Borrowed(row.lower.as_str()))),
            "x_offset" => Some(FValue::Num(i64::from(row.x_offset))),
            "y_offset" => Some(FValue::Num(i64::from(row.y_offset))),
            "linedef" => self.owner(index).map(wide_usize).map(FValue::Num),
            _ => None,
        }
    }

    /// The linedef referencing sidedef `index`, when the owners were built.
    fn owner(&self, index: usize) -> Option<usize> {
        self.owners.as_ref()?.get(index).copied().flatten()
    }

    /// One thing field; `name` and `category` read the definitions, and the
    /// sector resolves through the geometry.
    fn thing_value(&self, index: usize, name: &str) -> Option<FValue<'map>> {
        let row = self.map.things.get(index)?;
        match name {
            "index" => Some(FValue::Num(wide_usize(index))),
            "type" => Some(FValue::Num(i64::from(row.kind))),
            "x" => Some(FValue::Num(i64::from(row.x))),
            "y" => Some(FValue::Num(i64::from(row.y))),
            "angle" => Some(FValue::Num(i64::from(row.angle))),
            "flags" => Some(FValue::Bits(row.flags, FlagKind::Thing)),
            "name" => Some(FValue::Text(Cow::Owned(self.ctx.thing_name(row.kind)))),
            "category" => Some(FValue::Text(Cow::Owned(self.ctx.thing_category(row.kind)))),
            "sector" => self.thing_sector(row).map(wide_usize).map(FValue::Num),
            "tid" => Some(FValue::Num(i64::from(row.tid))),
            "special" => Some(FValue::Num(i64::from(row.special))),
            _ => None,
        }
    }

    /// The sector thing `row` stands in, `None` in the void.
    fn thing_sector(&self, row: &Thing) -> Option<usize> {
        point_in_sector(self.map, i64::from(row.x), i64::from(row.y))
    }

    /// One vertex field.
    fn vertex_value(&self, index: usize, name: &str) -> Option<FValue<'map>> {
        let row = self.map.vertices.get(index)?;
        match name {
            "index" => Some(FValue::Num(wide_usize(index))),
            "x" => Some(FValue::Num(i64::from(row.x))),
            "y" => Some(FValue::Num(i64::from(row.y))),
            _ => None,
        }
    }

    /// Whether element `index` lies within the query's bound.
    fn bounded(&self, index: usize) -> bool {
        self.bounds.bbox.map_or_else(
            || !self.bounds.sectors || self.in_sector_set(index),
            |bbox| self.in_bbox(bbox, index),
        )
    }

    /// The `bbox` rule of the queried kind.
    fn in_bbox(&self, bbox: [i64; 4], index: usize) -> bool {
        match self.kind {
            ElementKind::Sectors => self
                .facts(index)
                .and_then(|facts| facts.center)
                .is_some_and(|[x, y]| in_box(bbox, x, y)),
            ElementKind::Things => self
                .map
                .things
                .get(index)
                .is_some_and(|row| in_box(bbox, i64::from(row.x), i64::from(row.y))),
            ElementKind::Vertices => self
                .map
                .vertices
                .get(index)
                .is_some_and(|row| in_box(bbox, i64::from(row.x), i64::from(row.y))),
            ElementKind::Linedefs => self
                .map
                .linedefs
                .get(index)
                .is_some_and(|line| self.line_in_box(bbox, line)),
            ElementKind::Sidedefs => self.owner(index).is_some_and(|line| {
                self.map
                    .linedefs
                    .get(line)
                    .is_some_and(|row| self.line_in_box(bbox, row))
            }),
        }
    }

    /// Whether both vertices of `line` lie inside `bbox`.
    fn line_in_box(&self, bbox: [i64; 4], line: &Linedef) -> bool {
        self.vertex_in_box(bbox, line.v1) && self.vertex_in_box(bbox, line.v2)
    }

    /// Whether vertex `vertex` exists and lies inside `bbox`.
    fn vertex_in_box(&self, bbox: [i64; 4], vertex: usize) -> bool {
        self.map
            .vertices
            .get(vertex)
            .is_some_and(|row| in_box(bbox, i64::from(row.x), i64::from(row.y)))
    }

    /// The `sectors` rule of the queried kind.
    fn in_sector_set(&self, index: usize) -> bool {
        let set = &self.bounds.set;
        match self.kind {
            ElementKind::Sectors => set.contains(&index),
            ElementKind::Things => self
                .map
                .things
                .get(index)
                .is_some_and(|row| self.point_in_set(i64::from(row.x), i64::from(row.y), set)),
            ElementKind::Vertices => self
                .map
                .vertices
                .get(index)
                .is_some_and(|row| self.point_in_set(i64::from(row.x), i64::from(row.y), set)),
            ElementKind::Linedefs => self.map.linedefs.get(index).is_some_and(|line| {
                self.side_in_set(line.front, set) || self.side_in_set(line.back, set)
            }),
            ElementKind::Sidedefs => self
                .map
                .sidedefs
                .get(index)
                .is_some_and(|row| set.contains(&row.sector)),
        }
    }

    /// Whether the point `x`/`y` stands in one of `set`'s sectors.
    fn point_in_set(&self, x: i64, y: i64, set: &BTreeSet<usize>) -> bool {
        point_in_sector(self.map, x, y).is_some_and(|sector| set.contains(&sector))
    }

    /// Whether the resolvable side `side` opens into one of `set`'s sectors.
    fn side_in_set(&self, side: Option<usize>, set: &BTreeSet<usize>) -> bool {
        self.side_sector(side)
            .is_some_and(|sector| set.contains(&sector))
    }
}

/// Whether any filter reads one of the `wanted` fields.
fn mentions(filters: &[Filter], wanted: &[&str]) -> bool {
    filters
        .iter()
        .any(|filter| wanted.contains(&filter.field.as_str()))
}

/// The first linedef referencing each sidedef, in linedef order; a side no
/// line owns reads as `None`.
fn build_owners(map: &Map) -> Vec<Option<usize>> {
    let mut owners: Vec<Option<usize>> = core::iter::repeat_n(None, map.sidedefs.len()).collect();
    for (index, line) in map.linedefs.iter().enumerate() {
        claim_owner(&mut owners, line.front, index);
        claim_owner(&mut owners, line.back, index);
    }
    owners
}

/// Let linedef `index` own side `side` while no earlier line owns it.
fn claim_owner(owners: &mut [Option<usize>], side: Option<usize>, index: usize) {
    let Some(slot) = side.and_then(|which| owners.get_mut(which)) else {
        return;
    };
    if slot.is_none() {
        *slot = Some(index);
    }
}

/// The `Hexen` linedef argument one `arg0..arg4` field reads.
fn line_arg(line: &Linedef, name: &str) -> Option<i64> {
    let slot = match name {
        "arg0" => 0_usize,
        "arg1" => 1_usize,
        "arg2" => 2_usize,
        "arg3" => 3_usize,
        "arg4" => 4_usize,
        _ => return None,
    };
    line.args.iter().copied().nth(slot).map(i64::from)
}

/// A `u64` measure (area, length) widened, saturating at the top.
fn wide_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// An element index widened, saturating at the top.
fn wide_usize(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Whether the point `x`/`y` lies inside the inclusive box.
const fn in_box(bbox: [i64; 4], x: i64, y: i64) -> bool {
    let [min_x, min_y, max_x, max_y] = bbox;
    x >= min_x && x <= max_x && y >= min_y && y <= max_y
}

/// Check one filter against the field table: a known field, an op its type
/// takes, and a value of the shape that op needs.
fn prepare_filter(
    kind: ElementKind,
    family: FlagFamily,
    filter: &Filter,
) -> Result<Prepared<'_>, CyberError> {
    let spec = field_spec(kind, family, &filter.field)
        .ok_or_else(|| unknown_field(kind, family, filter))?;
    check_op(kind, spec, filter)?;
    check_value(family, spec, filter)?;
    Ok(Prepared {
        spec,
        op: filter.op,
        want: &filter.value,
    })
}

/// The `invalid_argument` for a field the kind does not carry.
fn unknown_field(kind: ElementKind, family: FlagFamily, filter: &Filter) -> CyberError {
    CyberError::InvalidArgument(format!(
        "unknown field '{}' for {}: valid fields are {}",
        filter.field,
        kind.as_str(),
        valid_fields(kind, family)
    ))
}

/// The word naming one type's comparison values.
const fn type_word(ty: FieldType) -> &'static str {
    match ty {
        FieldType::Number => "a number",
        FieldType::Text => "a string",
        FieldType::Boolean => "a boolean",
        FieldType::LineFlags | FieldType::ThingFlags | FieldType::Roles => "a name",
    }
}

/// The op list a field type accepts, comma-joined for error messages.
fn ops_word(ty: FieldType) -> String {
    [
        Op::Eq,
        Op::Ne,
        Op::Lt,
        Op::Le,
        Op::Gt,
        Op::Ge,
        Op::In,
        Op::NotIn,
        Op::Glob,
        Op::Has,
        Op::HasNot,
    ]
    .into_iter()
    .filter(|op| op_ok(*op, ty))
    .map(op_name)
    .collect::<Vec<&str>>()
    .join(", ")
}

/// The wire name of one op.
const fn op_name(op: Op) -> &'static str {
    match op {
        Op::Eq => "eq",
        Op::Ne => "ne",
        Op::Lt => "lt",
        Op::Le => "le",
        Op::Gt => "gt",
        Op::Ge => "ge",
        Op::In => "in",
        Op::NotIn => "not_in",
        Op::Glob => "glob",
        Op::Has => "has",
        Op::HasNot => "has_not",
    }
}

/// Whether `op` applies to a field of type `ty`.
const fn op_ok(op: Op, ty: FieldType) -> bool {
    match ty {
        FieldType::Number => !matches!(op, Op::Glob | Op::Has | Op::HasNot),
        FieldType::Text => !matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Has | Op::HasNot),
        FieldType::Boolean => matches!(op, Op::Eq | Op::Ne | Op::In | Op::NotIn),
        FieldType::LineFlags | FieldType::ThingFlags | FieldType::Roles => {
            matches!(op, Op::Has | Op::HasNot)
        }
    }
}

/// Refuse an op the field's type does not take.
fn check_op(kind: ElementKind, spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    if op_ok(filter.op, spec.ty) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "op '{}' does not apply to {} field '{}': {} fields take {}",
        op_name(filter.op),
        kind.as_str(),
        spec.name,
        type_word(spec.ty),
        ops_word(spec.ty),
    )))
}

/// Check the comparison value's shape against the field's type and op.
fn check_value(family: FlagFamily, spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    match filter.op {
        Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => check_scalar(spec, filter),
        Op::In | Op::NotIn => check_list(spec, filter),
        Op::Glob => check_glob(spec, filter),
        Op::Has | Op::HasNot => check_name(family, spec, filter),
    }
}

/// The scalar shape one type's direct comparisons need.
fn type_matches(ty: FieldType, want: &Value) -> bool {
    match ty {
        FieldType::Number => want.is_i64(),
        FieldType::Text => want.is_string(),
        FieldType::Boolean => want.is_boolean(),
        FieldType::LineFlags | FieldType::ThingFlags | FieldType::Roles => false,
    }
}

/// Refuse a scalar comparison of the wrong shape.
fn check_scalar(spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    if type_matches(spec.ty, &filter.value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{} of field '{}' needs {}; got {}",
        op_name(filter.op),
        spec.name,
        type_word(spec.ty),
        filter.value,
    )))
}

/// Refuse an `in`/`not_in` whose array holds a wrongly shaped value.
fn check_list(spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    let Some(items) = filter.value.as_array() else {
        return Err(CyberError::InvalidArgument(format!(
            "{} of field '{}' needs an array of {}; got {}",
            op_name(filter.op),
            spec.name,
            type_word(spec.ty),
            filter.value,
        )));
    };
    for item in items {
        if !type_matches(spec.ty, item) {
            return Err(CyberError::InvalidArgument(format!(
                "{} of field '{}' needs an array of {}; {item} does not fit",
                op_name(filter.op),
                spec.name,
                type_word(spec.ty),
            )));
        }
    }
    Ok(())
}

/// Refuse a `glob` whose pattern is not a string.
fn check_glob(spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    if filter.value.is_string() {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "glob of field '{}' needs a string pattern; got {}",
        spec.name, filter.value,
    )))
}

/// Refuse a `has`/`has_not` whose name is unknown or wrongly shaped.
fn check_name(family: FlagFamily, spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    let Some(wanted) = filter.value.as_str() else {
        return Err(CyberError::InvalidArgument(format!(
            "{} of field '{}' needs {}; got {}",
            op_name(filter.op),
            spec.name,
            type_word(spec.ty),
            filter.value,
        )));
    };
    match spec.ty {
        FieldType::LineFlags => check_flag(FlagKind::Linedef, family, wanted),
        FieldType::ThingFlags => check_flag(FlagKind::Thing, family, wanted),
        FieldType::Roles => check_role(spec, wanted),
        FieldType::Number | FieldType::Text | FieldType::Boolean => Ok(()),
    }
}

/// Refuse a flag name the family's table does not carry.
fn check_flag(kind: FlagKind, family: FlagFamily, wanted: &str) -> Result<(), CyberError> {
    if names(kind, family).iter().any(|entry| entry.0 == wanted) {
        return Ok(());
    }
    let valid: Vec<&str> = names(kind, family).iter().map(|entry| entry.0).collect();
    Err(CyberError::InvalidArgument(format!(
        "unknown {} flag '{wanted}' for the {} family: valid names are {}",
        kind.as_str(),
        family.as_str(),
        valid.join(", "),
    )))
}

/// Refuse a role name the describe classification never answers with.
fn check_role(spec: &FieldSpec, wanted: &str) -> Result<(), CyberError> {
    if ROLE_NAMES.contains(&wanted) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown sector role '{wanted}' for field '{}': valid roles are {}",
        spec.name,
        ROLE_NAMES.join(", "),
    )))
}

/// Run one checked comparison against one element's field value: the
/// value's shape picks the comparison family, exactly as `prepare_filter`
/// promised when it checked the op against the field type.
fn compare(op: Op, family: FlagFamily, value: FValue<'_>, want: &Value) -> bool {
    match value {
        FValue::Num(stored) => compare_num(op, want, stored),
        FValue::Text(text) => compare_text(op, want, text.as_ref()),
        FValue::Bool(flag) => compare_bool(op, want, flag),
        FValue::Bits(bits, kind) => compare_names(op, want, &to_names(bits, kind, family)),
        FValue::Roles(roles) => {
            let names: Vec<&str> = roles.iter().map(String::as_str).collect();
            compare_names(op, want, &names)
        }
    }
}

/// Compare a numeric field against its value.
fn compare_num(op: Op, want: &Value, stored: i64) -> bool {
    match op {
        Op::Eq => want.as_i64() == Some(stored),
        Op::Ne => want.as_i64() != Some(stored),
        Op::In | Op::NotIn => {
            let in_list = array_of(want)
                .is_some_and(|items| items.iter().any(|item| item.as_i64() == Some(stored)));
            in_list == (op == Op::In)
        }
        Op::Lt => want.as_i64().is_some_and(|wanted| stored < wanted),
        Op::Le => want.as_i64().is_some_and(|wanted| stored <= wanted),
        Op::Gt => want.as_i64().is_some_and(|wanted| stored > wanted),
        Op::Ge => want.as_i64().is_some_and(|wanted| stored >= wanted),
        Op::Glob | Op::Has | Op::HasNot => false,
    }
}

/// Compare a text field against its value.
fn compare_text(op: Op, want: &Value, stored: &str) -> bool {
    match op {
        Op::Eq => want.as_str() == Some(stored),
        Op::Ne => want.as_str() != Some(stored),
        Op::In | Op::NotIn => {
            let in_list = array_of(want)
                .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(stored)));
            in_list == (op == Op::In)
        }
        Op::Glob => want
            .as_str()
            .is_some_and(|pattern| glob_match(stored.as_bytes(), pattern)),
        Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Has | Op::HasNot => false,
    }
}

/// Compare the `two_sided` flag against its boolean.
fn compare_bool(op: Op, want: &Value, stored: bool) -> bool {
    match op {
        Op::Eq => want.as_bool() == Some(stored),
        Op::Ne => want.as_bool() != Some(stored),
        Op::In | Op::NotIn => {
            let in_list = array_of(want)
                .is_some_and(|items| items.iter().any(|item| item.as_bool() == Some(stored)));
            in_list == (op == Op::In)
        }
        Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Glob | Op::Has | Op::HasNot => false,
    }
}

/// Compare a flag bitset or role list against one named entry.
fn compare_names(op: Op, want: &Value, names: &[&str]) -> bool {
    let found = want.as_str().is_some_and(|wanted| names.contains(&wanted));
    found == (op == Op::Has)
}

/// The array of an `in`/`not_in` comparison value.
fn array_of(want: &Value) -> Option<&[Value]> {
    want.as_array().map(Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{ElementKind, Filter, Op, Query, QueryContext, Within, run_query, side_sector};
    use crate::defs::{Definitions, definitions_for};
    use crate::wad::game::Game;
    use crate::wad::map::common::fixtures::{room, side, solid_line, two_room_map, vertex};
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::map::{Linedef, Map, MapFormat};

    /// The built-in `DOOM II` rows, naming the fixture's things.
    fn defs() -> Definitions {
        definitions_for(Game::Doom2).expect("the embedded Doom II table is valid")
    }

    /// A `Doom`-family context over `defs`.
    fn doom_ctx(defs: &Definitions) -> QueryContext<'_> {
        QueryContext::new(FlagFamily::Doom, defs)
    }

    /// A `Hexen`-family context over `defs`.
    fn hexen_ctx(defs: &Definitions) -> QueryContext<'_> {
        QueryContext::new(FlagFamily::Hexen, defs)
    }

    /// One filter row.
    fn filter(field: &str, op: Op, value: Value) -> Filter {
        Filter {
            field: field.to_owned(),
            op,
            value,
        }
    }

    /// A query over just these filters and bound.
    fn query(kind: ElementKind, filters: Vec<Filter>, within: Option<Within>) -> Query {
        Query {
            kind,
            r#where: filters,
            within,
        }
    }

    /// A one-filter unbounded query.
    fn field_query(kind: ElementKind, f: Filter) -> Query {
        query(kind, vec![f], None)
    }

    /// One op-coverage row: field, op, value and the expected hits.
    type Case = (&'static str, Op, Value, Vec<usize>);

    /// Assert every `needles` appears in `message`.
    fn assert_mentions(message: &str, needles: &[&str]) {
        for needle in needles {
            assert!(message.contains(needle), "{needle} missing from: {message}");
        }
    }

    /// Run `q` over `map`, expecting success.
    fn hits(map: &Map, q: &Query) -> Vec<usize> {
        let defs = defs();
        run_query(map, &doom_ctx(&defs), q).expect("test queries are valid")
    }

    /// The two rooms plus a `secret`-special and `F_SKY1` role marker on
    /// sector 1, for the derived-field tests.
    fn roley_map() -> Map {
        let mut map = two_room_map();
        let sector = map.sectors.get_mut(1).expect("the fixture has 2 sectors");
        sector.special = 9_i32;
        sector.ceiling_flat = "F_SKY1".to_owned();
        map
    }

    /// The fixture plus a diagonal line 7 from `(0, 0)` to `(512, 256)`.
    fn diagonal_map() -> Map {
        let mut map = two_room_map();
        map.linedefs.push(solid_line(0, 3, 0));
        map
    }

    /// The fixture with an ambush-flagged start and a thing in the void.
    fn flaggy_map() -> Map {
        let mut map = two_room_map();
        map.things
            .get_mut(0)
            .expect("the fixture has a start")
            .flags |= 8;
        map.things
            .push(crate::wad::map::common::fixtures::spawn(99999, 600, 600, 0));
        map
    }

    #[test]
    fn every_numeric_op_filters_the_fixture_lights() {
        let map = two_room_map();
        let cases: [Case; 8] = [
            ("light", Op::Eq, json!(160_i64), vec![0]),
            ("light", Op::Ne, json!(160_i64), vec![1]),
            ("light", Op::Lt, json!(170_i64), vec![0]),
            ("light", Op::Le, json!(160_i64), vec![0]),
            ("light", Op::Gt, json!(170_i64), vec![1]),
            ("light", Op::Ge, json!(192_i64), vec![1]),
            ("light", Op::In, json!([160_i64, 192_i64]), vec![0, 1]),
            ("light", Op::NotIn, json!([160_i64]), vec![1]),
        ];
        for (field, op, value, want) in cases {
            let q = field_query(ElementKind::Sectors, filter(field, op, value.clone()));
            assert_eq!(hits(&map, &q), want, "{field} {op:?} {value}");
        }
    }

    #[test]
    fn every_text_op_filters_the_fixture_flats() {
        let map = two_room_map();
        let cases: [Case; 5] = [
            ("floor_flat", Op::Eq, json!("FLOOR1"), vec![0, 1]),
            ("floor_flat", Op::Ne, json!("FLOOR1"), vec![]),
            ("floor_flat", Op::In, json!(["FLOOR1"]), vec![0, 1]),
            ("floor_flat", Op::NotIn, json!(["FLOOR1"]), vec![]),
            ("floor_flat", Op::Glob, json!("floor*"), vec![0, 1]),
        ];
        for (field, op, value, want) in cases {
            let q = field_query(ElementKind::Sectors, filter(field, op, value.clone()));
            assert_eq!(hits(&map, &q), want, "{field} {op:?} {value}");
        }
    }

    #[test]
    fn glob_matches_case_insensitively_on_patterns_and_names() {
        let map = two_room_map();
        let lower = field_query(ElementKind::Things, filter("name", Op::Glob, json!("imp*")));
        assert_eq!(hits(&map, &lower), vec![1], "the pattern is lowercase");
        let question = field_query(
            ElementKind::Sectors,
            filter("floor_flat", Op::Glob, json!("FLOOR?")),
        );
        assert_eq!(hits(&map, &question), vec![0, 1], "? takes one character");
        let miss = field_query(
            ElementKind::Sectors,
            filter("floor_flat", Op::Glob, json!("NUKAGE*")),
        );
        assert!(hits(&map, &miss).is_empty(), "non-matching patterns drop");
    }

    #[test]
    fn has_tests_named_linedef_and_thing_flags() {
        let map = flaggy_map();
        let blocking = field_query(
            ElementKind::Linedefs,
            filter("flags", Op::Has, json!("blocking")),
        );
        assert_eq!(
            hits(&map, &blocking),
            vec![0, 1, 3, 4, 5, 6],
            "the step line does not block"
        );
        let twosided = field_query(
            ElementKind::Linedefs,
            filter("flags", Op::Has, json!("twosided")),
        );
        assert_eq!(
            hits(&map, &twosided),
            vec![2],
            "only the step line is two-sided"
        );
        let not_blocking = field_query(
            ElementKind::Linedefs,
            filter("flags", Op::HasNot, json!("blocking")),
        );
        assert_eq!(hits(&map, &not_blocking), vec![2], "has_not inverts");
        let ambush = field_query(
            ElementKind::Things,
            filter("flags", Op::Has, json!("ambush")),
        );
        assert_eq!(hits(&map, &ambush), vec![0], "the start carries the bit");
        let easy = field_query(ElementKind::Things, filter("flags", Op::Has, json!("easy")));
        assert_eq!(
            hits(&map, &easy),
            vec![0, 1, 2],
            "spawns carry the skill bits"
        );
    }

    #[test]
    fn has_tests_named_sector_roles() {
        let map = roley_map();
        let secret = field_query(
            ElementKind::Sectors,
            filter("roles", Op::Has, json!("secret")),
        );
        assert_eq!(hits(&map, &secret), vec![1], "special 9 hides sector 1");
        let sky = field_query(ElementKind::Sectors, filter("roles", Op::Has, json!("sky")));
        assert_eq!(
            hits(&map, &sky),
            vec![1],
            "the F_SKY1 ceiling opens sector 1"
        );
        let none = field_query(
            ElementKind::Sectors,
            filter("roles", Op::Has, json!("door")),
        );
        assert!(hits(&map, &none).is_empty(), "no door action here");
        let not_sky = field_query(
            ElementKind::Sectors,
            filter("roles", Op::HasNot, json!("sky")),
        );
        assert_eq!(hits(&map, &not_sky), vec![0], "sector 0 stays closed");
    }

    #[test]
    fn derived_sector_fields_measure_the_fixture_rooms() {
        let map = two_room_map();
        let height = field_query(
            ElementKind::Sectors,
            filter("height", Op::Eq, json!(96_i64)),
        );
        assert_eq!(
            hits(&map, &height),
            vec![1],
            "128 minus the raised 32-unit floor"
        );
        let area = field_query(
            ElementKind::Sectors,
            filter("area", Op::Ge, json!(0x0001_0000_i64)),
        );
        assert_eq!(hits(&map, &area), vec![0, 1], "both rooms span 256 x 256");
        let small = field_query(
            ElementKind::Sectors,
            filter("area", Op::Lt, json!(1000_i64)),
        );
        assert!(hits(&map, &small).is_empty(), "nothing is that small");
    }

    #[test]
    fn derived_linedef_fields_resolve_sides_lengths_and_faces() {
        let map = diagonal_map();
        let two_sided = field_query(
            ElementKind::Linedefs,
            filter("two_sided", Op::Eq, json!(true)),
        );
        assert_eq!(
            hits(&map, &two_sided),
            vec![2],
            "only the step line has two sides"
        );
        let either = field_query(
            ElementKind::Linedefs,
            filter("two_sided", Op::In, json!([true, false])),
        );
        assert_eq!(
            hits(&map, &either),
            (0..8).collect::<Vec<usize>>(),
            "in covers both"
        );
        let front = field_query(
            ElementKind::Linedefs,
            filter("front_sector", Op::Eq, json!(0_i64)),
        );
        assert_eq!(
            hits(&map, &front),
            vec![0, 1, 2, 3, 7],
            "the hall fronts, plus the reused diagonal"
        );
        let back = field_query(
            ElementKind::Linedefs,
            filter("back_sector", Op::Eq, json!(1_i64)),
        );
        assert_eq!(
            hits(&map, &back),
            vec![2],
            "only the step line opens backwards"
        );
        let long = field_query(
            ElementKind::Linedefs,
            filter("length", Op::Gt, json!(300_i64)),
        );
        assert_eq!(hits(&map, &long), vec![7], "hypot(512, 256) beats 300");
        let front_lower = field_query(
            ElementKind::Linedefs,
            filter("front_lower", Op::Eq, json!("STEP1")),
        );
        assert_eq!(hits(&map, &front_lower), vec![2], "the step's lower face");
        let back_middle = field_query(
            ElementKind::Linedefs,
            filter("back_middle", Op::Eq, json!("-")),
        );
        assert_eq!(
            hits(&map, &back_middle),
            vec![2],
            "one-sided lines have no back face"
        );
    }

    #[test]
    fn derived_thing_fields_read_names_categories_and_sectors() {
        let map = flaggy_map();
        let start = field_query(
            ElementKind::Things,
            filter("name", Op::Eq, json!("Player 1 Start")),
        );
        assert_eq!(hits(&map, &start), vec![0], "type 1 reads as the start");
        let monster = field_query(
            ElementKind::Things,
            filter("category", Op::Eq, json!("Monsters")),
        );
        assert_eq!(hits(&map, &monster), vec![1], "the imp is a monster");
        let unknown = field_query(
            ElementKind::Things,
            filter("name", Op::Glob, json!("unknown*")),
        );
        assert_eq!(
            hits(&map, &unknown),
            vec![2],
            "type 99999 reads Unknown (99999)"
        );
        let hall = field_query(ElementKind::Things, filter("sector", Op::Eq, json!(0_i64)));
        assert_eq!(hits(&map, &hall), vec![0], "the start stands in the hall");
        let voided = field_query(ElementKind::Things, filter("sector", Op::Ne, json!(0_i64)));
        assert_eq!(
            hits(&map, &voided),
            vec![1],
            "the imp is in a different sector and the void thing resolves to nothing, ne included"
        );
    }

    #[test]
    fn sidedef_fields_include_their_owning_line() {
        let map = two_room_map();
        let wall = field_query(
            ElementKind::Sidedefs,
            filter("middle", Op::Eq, json!("WALL1")),
        );
        assert_eq!(
            hits(&map, &wall),
            vec![0, 1, 4, 5, 6, 7],
            "the dressed faces"
        );
        let step = field_query(
            ElementKind::Sidedefs,
            filter("lower", Op::Eq, json!("STEP1")),
        );
        assert_eq!(hits(&map, &step), vec![2], "the step's lower face");
        let owner = field_query(
            ElementKind::Sidedefs,
            filter("linedef", Op::Eq, json!(2_i64)),
        );
        assert_eq!(
            hits(&map, &owner),
            vec![2, 3],
            "sidedefs 2 and 3 dress line 2"
        );
        let over = field_query(
            ElementKind::Sidedefs,
            filter("sector", Op::Eq, json!(1_i64)),
        );
        assert_eq!(hits(&map, &over), vec![3, 5, 6, 7], "the back room's sides");
    }

    /// A query for `kind` bounded by `bbox` alone.
    fn boxed(kind: ElementKind, bbox: [i64; 4]) -> Query {
        query(kind, vec![], Some(Within::Bbox { bbox }))
    }

    #[test]
    fn bbox_bounds_the_point_kinds() {
        let map = two_room_map();
        let things = query(
            ElementKind::Things,
            vec![filter("index", Op::Ge, json!(0_i64))],
            Some(Within::Bbox {
                bbox: [256, 0, 512, 256],
            }),
        );
        assert_eq!(
            hits(&map, &things),
            vec![1],
            "the imp stands right of x 256"
        );
        let vertices = boxed(ElementKind::Vertices, [0, 0, 256, 256]);
        assert_eq!(
            hits(&map, &vertices),
            vec![0, 1, 4, 5],
            "the hall box corners"
        );
    }

    #[test]
    fn bbox_bounds_the_shape_kinds_by_their_own_rules() {
        let map = two_room_map();
        let lines = boxed(ElementKind::Linedefs, [0, 0, 256, 256]);
        assert_eq!(
            hits(&map, &lines),
            vec![0, 1, 2, 3],
            "lines fully inside the box"
        );
        let sides = boxed(ElementKind::Sidedefs, [0, 0, 256, 256]);
        assert_eq!(
            hits(&map, &sides),
            vec![0, 1, 2, 3, 4],
            "sides of those lines"
        );
        let sectors = boxed(ElementKind::Sectors, [0, 0, 200, 255]);
        assert_eq!(
            hits(&map, &sectors),
            vec![0],
            "only the hall's centre is boxed in"
        );
    }

    #[test]
    fn sector_sets_bound_every_kind_by_membership() {
        let map = two_room_map();
        let one = Within::Sectors { sectors: vec![1] };
        let zero = Within::Sectors { sectors: vec![0] };
        let things = query(ElementKind::Things, vec![], Some(one.clone()));
        assert_eq!(hits(&map, &things), vec![1], "the imp's room");
        let vertices = query(ElementKind::Vertices, vec![], Some(zero.clone()));
        assert_eq!(
            hits(&map, &vertices),
            vec![0, 1, 4, 5],
            "the hall's own corners and the step line's, which face it"
        );
        let lines = query(ElementKind::Linedefs, vec![], Some(one.clone()));
        assert_eq!(
            hits(&map, &lines),
            vec![2, 4, 5, 6],
            "every line with a side in room 1"
        );
        let sides = query(ElementKind::Sidedefs, vec![], Some(one));
        assert_eq!(hits(&map, &sides), vec![3, 5, 6, 7], "the room 1 sides");
        let sectors = query(ElementKind::Sectors, vec![], Some(zero));
        assert_eq!(hits(&map, &sectors), vec![0], "sectors bound by index");
        let none = query(
            ElementKind::Vertices,
            vec![],
            Some(Within::Sectors {
                sectors: vec![99_usize],
            }),
        );
        assert!(
            hits(&map, &none).is_empty(),
            "an out-of-range bound matches nothing"
        );
    }

    #[test]
    fn filters_combine_with_and() {
        let map = two_room_map();
        let both = query(
            ElementKind::Sectors,
            vec![
                filter("light", Op::Gt, json!(100_i64)),
                filter("tag", Op::Eq, json!(1_i64)),
            ],
            None,
        );
        assert_eq!(hits(&map, &both), vec![1], "bright and tagged");
        let empty = query(
            ElementKind::Sectors,
            vec![
                filter("light", Op::Gt, json!(100_i64)),
                filter("tag", Op::Eq, json!(0_i64)),
            ],
            Some(Within::Sectors { sectors: vec![1] }),
        );
        assert!(
            hits(&map, &empty).is_empty(),
            "the third clause contradicts"
        );
        let bare = query(ElementKind::Vertices, vec![], None);
        assert_eq!(
            hits(&map, &bare),
            (0..6).collect::<Vec<usize>>(),
            "no filters keeps everything"
        );
    }

    #[test]
    fn unknown_fields_are_refused_with_the_valid_list() {
        let defs = defs();
        let map = two_room_map();
        let q = field_query(ElementKind::Sectors, filter("aro", Op::Eq, json!(1_i64)));
        let error = run_query(&map, &doom_ctx(&defs), &q).expect_err("no such field");
        assert_eq!(error.code(), "invalid_argument", "bad field code");
        let message = error.to_string();
        assert!(
            message.contains("unknown field 'aro' for sectors")
                && message.contains("floor_flat")
                && message.contains("roles")
                && message.contains("height"),
            "the valid fields are listed: {message}"
        );
    }

    #[test]
    fn family_tables_see_their_own_fields() {
        let defs = defs();
        let map = two_room_map();
        let tag_on_hexen = field_query(ElementKind::Linedefs, filter("tag", Op::Eq, json!(0_i64)));
        let error = run_query(&map, &hexen_ctx(&defs), &tag_on_hexen)
            .expect_err("the Hexen family carries arg0 instead");
        assert!(
            error.to_string().contains("arg0"),
            "the extras are listed: {error}"
        );
        let arg_on_doom = field_query(ElementKind::Linedefs, filter("arg0", Op::Eq, json!(0_i64)));
        assert_eq!(
            run_query(&map, &doom_ctx(&defs), &arg_on_doom)
                .expect_err("the Doom family carries tag instead")
                .code(),
            "invalid_argument",
            "arg0 is not a Doom field"
        );
        let mut hexen = Map::new_empty(map.name, MapFormat::Hexen);
        hexen.linedefs.push(Linedef {
            args: [7_i32, 0_i32, 0_i32, 0_i32, 0_i32],
            flags: 1_u32,
            ..Default::default()
        });
        let arg0 = field_query(ElementKind::Linedefs, filter("arg0", Op::Eq, json!(7_i64)));
        assert_eq!(
            run_query(&hexen, &hexen_ctx(&defs), &arg0).expect("arg0 reads"),
            vec![0],
            "the script argument filters"
        );
        let cross = field_query(
            ElementKind::Linedefs,
            filter("flags", Op::Has, json!("playercross")),
        );
        assert_eq!(
            run_query(&hexen, &hexen_ctx(&defs), &cross).expect("activation names"),
            vec![0],
            "field value 0 reads as playercross"
        );
        let tid_on_doom = field_query(ElementKind::Things, filter("tid", Op::Eq, json!(0_i64)));
        assert_eq!(
            run_query(&map, &doom_ctx(&defs), &tid_on_doom)
                .expect_err("Doom things carry no tid")
                .code(),
            "invalid_argument",
            "tid is a Hexen field"
        );
    }

    /// Refuse each query in `cases`, checking the code and message.
    fn refuse_all(cases: Vec<(Query, Vec<&str>)>) {
        let defs = defs();
        let map = two_room_map();
        for (q, needles) in cases {
            let error =
                run_query(&map, &doom_ctx(&defs), &q).expect_err("these filters must be refused");
            assert_eq!(error.code(), "invalid_argument", "{q:?} code");
            assert_mentions(&error.to_string(), &needles);
        }
    }

    #[test]
    fn wrong_ops_and_values_are_refused_with_instructions() {
        let cases = vec![
            (
                field_query(ElementKind::Sectors, filter("light", Op::Eq, json!("x"))),
                vec!["needs a number"],
            ),
            (
                field_query(
                    ElementKind::Sectors,
                    filter("floor_flat", Op::Gt, json!("A")),
                ),
                vec!["does not apply", "string", "eq"],
            ),
            (
                field_query(ElementKind::Linedefs, filter("flags", Op::Eq, json!(1_i64))),
                vec!["does not apply", "has"],
            ),
            (
                field_query(ElementKind::Sectors, filter("roles", Op::Eq, json!("door"))),
                vec!["does not apply"],
            ),
            (
                field_query(
                    ElementKind::Sectors,
                    filter("light", Op::In, json!([160_i64, "x"])),
                ),
                vec!["needs an array of a number", "does not fit"],
            ),
            (
                field_query(
                    ElementKind::Sectors,
                    filter("light", Op::In, json!(160_i64)),
                ),
                vec!["needs an array"],
            ),
            (
                field_query(
                    ElementKind::Sectors,
                    filter("floor_flat", Op::Glob, json!(5_i64)),
                ),
                vec!["needs a string pattern"],
            ),
        ];
        refuse_all(cases);
    }

    #[test]
    fn unknown_flag_and_role_names_list_the_valid_ones() {
        let cases = vec![
            (
                field_query(
                    ElementKind::Linedefs,
                    filter("flags", Op::Has, json!("impassable")),
                ),
                vec!["unknown linedef flag 'impassable'", "blocking", "twosided"],
            ),
            (
                field_query(
                    ElementKind::Things,
                    filter("flags", Op::Has, json!("twosided")),
                ),
                vec!["unknown thing flag 'twosided'", "ambush", "easy"],
            ),
            (
                field_query(
                    ElementKind::Sectors,
                    filter("roles", Op::Has, json!("hall")),
                ),
                vec!["unknown sector role 'hall'", "stair_step", "exit_room"],
            ),
        ];
        refuse_all(cases);
    }

    #[test]
    fn bad_queries_fail_before_any_element_is_read() {
        let defs = defs();
        let empty = Map::default();
        let q = field_query(ElementKind::Sectors, filter("aro", Op::Eq, json!(1_i64)));
        assert_eq!(
            run_query(&empty, &doom_ctx(&defs), &q)
                .expect_err("unknown fields fail even with no sectors")
                .code(),
            "invalid_argument",
            "checked eagerly"
        );
    }

    #[test]
    fn the_wire_shape_parses_and_documents_where() {
        let value = json!({
            "kind": "sectors",
            "where": [{ "field": "light", "op": "gt", "value": 170_i64 }],
            "within": { "sectors": [1_usize] },
        });
        let parsed: Query = serde_json::from_value(value).expect("the wire shape parses");
        assert_eq!(parsed.kind, ElementKind::Sectors, "the kind");
        assert_eq!(parsed.r#where.len(), 1, "one filter");
        assert_eq!(
            parsed.r#where.first().expect("the filter").op,
            Op::Gt,
            "snake-case op"
        );
        assert!(
            matches!(parsed.within, Some(Within::Sectors { .. })),
            "tagged within"
        );
        let schema = serde_json::to_value(schemars::schema_for!(Query)).expect("a schema");
        assert!(
            schema["properties"]["where"].is_object(),
            "the schema property is named where, not r#where: {schema}"
        );
    }

    #[test]
    fn context_names_things_through_the_definitions() {
        let defs = defs();
        let ctx = doom_ctx(&defs);
        assert_eq!(ctx.thing_name(3001), "Imp", "the table name");
        assert_eq!(ctx.thing_category(3001), "Monsters", "the table category");
        assert_eq!(ctx.thing_name(99999), "Unknown (99999)", "undefined type");
        assert_eq!(ctx.thing_category(-1), "Unknown", "negative type");
    }

    #[test]
    fn side_sector_reads_resolvable_sides_only() {
        let map = two_room_map();
        let line = map.linedefs.get(2).expect("the step line");
        assert_eq!(side_sector(&map, line.front), Some(0), "the hall side");
        assert_eq!(side_sector(&map, line.back), Some(1), "the room side");
        assert_eq!(side_sector(&map, None), None, "no side, no sector");
        let mut broken = two_room_map();
        broken.sidedefs.get_mut(3).expect("sidedef 3").sector = 42;
        let dangling = broken.linedefs.get(2).expect("the step line");
        assert_eq!(
            side_sector(&broken, dangling.back),
            None,
            "a sidedef pointing past the sector block resolves to nothing"
        );
    }

    #[test]
    fn unresolvable_derived_values_and_empty_lists_match_nothing() {
        let map = two_room_map();
        let one_sided = query(
            ElementKind::Linedefs,
            vec![filter("back_sector", Op::Ne, json!(9_i64))],
            None,
        );
        assert_eq!(
            hits(&map, &one_sided),
            vec![2],
            "ne does not rescue a missing side"
        );
        let empty_in = field_query(ElementKind::Sectors, filter("light", Op::In, json!([])));
        assert!(
            hits(&map, &empty_in).is_empty(),
            "an empty list matches nothing"
        );
        let empty_not_in = field_query(ElementKind::Sectors, filter("light", Op::NotIn, json!([])));
        assert_eq!(
            hits(&map, &empty_not_in),
            vec![0, 1],
            "an empty list excludes nothing"
        );
    }

    #[test]
    fn vertices_and_things_filter_by_plain_numbers() {
        let map = two_room_map();
        let east = field_query(ElementKind::Vertices, filter("x", Op::Ge, json!(256_i64)));
        assert_eq!(hits(&map, &east), vec![1, 2, 3, 4], "right-hand vertices");
        let facing = field_query(ElementKind::Things, filter("angle", Op::Eq, json!(180_i64)));
        assert_eq!(hits(&map, &facing), vec![1], "the imp faces south");
        let kind = field_query(ElementKind::Things, filter("type", Op::Eq, json!(1_i64)));
        assert_eq!(hits(&map, &kind), vec![0], "the player start");
    }

    #[test]
    fn extra_sector_rows_and_lines_stay_queryable() {
        let mut map = two_room_map();
        map.sectors.push(room(0, 128, 128, 2));
        map.sidedefs.push(side("-", "-", "STARTAN", 2));
        let mut lone = solid_line(0, 1, 8);
        lone.special = 1_i32;
        map.linedefs.push(lone);
        map.vertices.push(vertex(0, 128));
        map.vertices.push(vertex(256, 128));
        let dark = field_query(
            ElementKind::Sectors,
            filter("light", Op::Lt, json!(160_i64)),
        );
        assert_eq!(hits(&map, &dark), vec![2], "the added room is darkest");
        let tagged = field_query(ElementKind::Sectors, filter("tag", Op::Eq, json!(2_i64)));
        assert_eq!(hits(&map, &tagged), vec![2], "and tagged");
        let special = field_query(
            ElementKind::Linedefs,
            filter("special", Op::Eq, json!(1_i64)),
        );
        assert_eq!(
            hits(&map, &special),
            vec![7],
            "the new line wears the action"
        );
    }
}
