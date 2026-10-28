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
//! reads the [`flags`] tables), the game definitions (so thing `name`
//! and `category` resolve, and the derived sector fields reuse
//! [`Describe`]) and the game's sky flat (so a `roles has sky` filter
//! reads the right ceiling name). Derived fields that do not resolve —
//! the back side of a one-sided line, a thing standing in the void, a
//! line with a dangling vertex — match no filter, `ne`, `not_in` and
//! `has_not` included.
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
//! one of them ([`SectorIndex::point_in_sector`](super::geometry::SectorIndex)),
//! sectors by index, linedefs with a resolvable side in one of them, and
//! sidedefs opening into one of them. Indices outside the sector block
//! simply match nothing.
//!
//! This file keeps the shapes a query is phrased in, the bound its `within`
//! clause resolves to, the context [`run_query`] reads through, that entry
//! point itself, and the shared element-index guard [`check_index`] every
//! tool and pure engine taking element indices runs through. The field
//! tables and the widened value one read
//! answers with are the private `fields` module; the checked filters, the
//! lazily built derived reads and the per-element matching are `engine`; and
//! the field, op and value checks and the comparisons they promise are
//! `check`.
//!
//! [`flags`]: super::flags
//! [`describe`]: super::describe
//! [`Describe`]: super::describe::Describe

mod check;
mod engine;
mod fields;

#[cfg(test)]
mod tests;

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::flags::FlagFamily;
use super::model::Map;
use crate::defs::{Definitions, ThingDef};
use crate::error::CyberError;

use engine::Engine;

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

    /// The lowercase name of one row of this kind (`"vertex"`).
    #[must_use]
    #[inline]
    pub const fn singular(self) -> &'static str {
        match self {
            Self::Sectors => "sector",
            Self::Linedefs => "linedef",
            Self::Sidedefs => "sidedef",
            Self::Things => "thing",
            Self::Vertices => "vertex",
        }
    }

    /// The lowercase name of several rows of this kind (`"vertices"`).
    /// Every plural is spelled per kind, never by appending an `s`, and
    /// equals the [`Self::as_str`] wire name.
    #[must_use]
    #[inline]
    pub const fn plural(self) -> &'static str {
        self.as_str()
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

impl crate::named::Named for ElementKind {
    /// Every kind, in report order.
    const ALL: &'static [Self] = &[
        Self::Sectors,
        Self::Linedefs,
        Self::Sidedefs,
        Self::Things,
        Self::Vertices,
    ];

    #[inline]
    fn name(self) -> &'static str {
        self.as_str()
    }
}

/// The hint [`missing_index`] attaches: the tools that answer with real
/// element indices.
const INDEX_HINT: &str = "describe_map or find_elements returns indices";

/// The [`CyberError::NotFound`] saying `index` is at or past a block of
/// `len` rows of `kind`, stating the valid range and hinting at the
/// index-listing tools.
#[must_use]
#[inline]
pub fn missing_index(kind: ElementKind, index: usize, len: usize) -> CyberError {
    let range = len.checked_sub(1).map_or_else(
        || format!("the map has no {}", kind.plural()),
        |last| {
            format!(
                "the map has {len} {}, valid indices are 0..={last}",
                kind.plural()
            )
        },
    );
    CyberError::NotFound(format!(
        "{} {index} does not exist ({range})",
        kind.singular()
    ))
    .with_hint(INDEX_HINT)
}

/// Refuses an element `index` outside `0..len` of one `kind` of map
/// element.
///
/// The message states the valid range `0..=N-1` with the kind's spelled
/// plural (`"vertices"`, never `"vertexs"`); the hint names the tools
/// that answer with real indices. Callers keep tool-specific context by
/// rebuilding this error with its message text run through a change
/// closure.
///
/// # Errors
///
/// [`CyberError::NotFound`] phrased for `kind` (e.g. `"linedef 12 does
/// not exist (the map has 7 linedefs, valid indices are 0..=6)"`).
#[inline]
pub fn check_index(kind: ElementKind, index: usize, len: usize) -> Result<(), CyberError> {
    if index < len {
        return Ok(());
    }
    Err(missing_index(kind, index, len))
}

/// Refuses the first of `indices` at or past the `kind` block of `map`.
///
/// # Errors
///
/// The [`check_index`] refusal for the first offending index, in the
/// order `indices` is iterated.
#[inline]
pub fn check_indices<'index, List>(
    kind: ElementKind,
    indices: List,
    map: &Map,
) -> Result<(), CyberError>
where
    List: IntoIterator<Item = &'index usize>,
{
    let count = kind.count_on(map);
    indices
        .into_iter()
        .try_for_each(|index| check_index(kind, *index, count))
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
pub(super) struct Bounds {
    /// The positional box, when the clause is a `bbox`.
    bbox: Option<[i64; 4]>,
    /// Whether the clause bounds by sector membership at all.
    sectors: bool,
    /// The membership set, built when `sectors`.
    set: BTreeSet<usize>,
}

/// Resolve the `within` clause once per query; the clone of an index list
/// bounded by the sector count is what keeps this cheap and the match plain.
pub(super) fn resolve_bounds(within: Option<&Within>) -> Bounds {
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
    /// The sky flat of the caller's game ([`crate::wad::game::sky_flat`]):
    /// what the derived `roles` read matches a `sky` ceiling against.
    pub sky_flat: &'static str,
}

impl<'defs> QueryContext<'defs> {
    /// A context reading flags in the `family`'s tables, things through
    /// the game definitions `defs` and `sky` ceilings against the sky
    /// flat `sky_flat`.
    #[must_use]
    #[inline]
    pub const fn new(family: FlagFamily, defs: &'defs Definitions, sky_flat: &'static str) -> Self {
        Self {
            family,
            defs,
            sky_flat,
        }
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
