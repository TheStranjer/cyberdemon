//! The run: the checked filters, the lazily built derived reads and the
//! per-element matching that answers a query with its ascending indices.
//!
//! [`Engine`] snapshots one [`run_query`](super::run_query) call: the flag
//! family and definitions of the [`QueryContext`], the map, the [`Prepared`]
//! filters [`prepare_filter`] checked, the resolved `within` bound, and the
//! derived reads it builds only when the query asks for them — one geometry
//! read per run: the [`Describe`] light [`SectorShape`] rows behind `area`,
//! `roles` and the sector arm of a `bbox` (whose centre they carry), or a
//! plain [`SectorIndex`] behind the thing `sector` field and the point rule
//! of a `sectors` bound. The two gates exclude each other by kind, so no
//! run ever builds both. The sidedef-to-linedef owners behind `linedef` and
//! a sidedef `bbox` are the third, independent read.
//! [`Engine::matches`] bounds the element and then `AND`s every filter over
//! the field values [`Engine::value`] reads; a derived field that does not
//! resolve matches nothing, `ne`, `not_in` and `has_not` included.
//!
//! Around the engine are the reads it measures with: [`mentions`] answers
//! whether a query needs a derived read at all, [`build_owners`] finds the
//! first line referencing each side, [`line_arg`] the `Hexen` script argument
//! an `arg0..arg4` field names, [`wide_u64`] and [`wide_usize`] the saturating
//! widenings every index and measure passes through, and [`in_box`] the
//! inclusive point-in-`bbox` test of both bound rules.

use alloc::borrow::Cow;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use serde_json::Value;

use crate::error::CyberError;
use crate::wad::map::describe::{Describe, SectorShape};
use crate::wad::map::flags::{FlagFamily, FlagKind};
use crate::wad::map::geometry::{SectorIndex, linedef_length};
use crate::wad::map::model::{Linedef, Map, Thing};

use super::check::{compare, prepare_filter};
use super::fields::{FValue, FieldSpec};
use super::{Bounds, ElementKind, Filter, Op, Query, QueryContext, resolve_bounds};

/// One filter checked against the field table, ready to compare.
pub(super) struct Prepared<'req> {
    /// The field this filter reads.
    pub(super) spec: &'static FieldSpec,
    /// The comparison to make.
    pub(super) op: Op,
    /// The value to compare against.
    pub(super) want: &'req Value,
}

/// The query's engine: the map, the checked filters and the derived reads
/// (the geometry read and the sidedef owners) built only when asked.
pub(super) struct Engine<'map, 'req> {
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
    /// or a sector `bbox` ask for it; every sector `area`, `roles` and
    /// centre read answers from its [`Describe::sector_shape`].
    describe: Option<Describe<'map>>,
    /// The stored sector geometry, built only when the thing `sector`
    /// field or the point rule of a `sectors` bound asks for it. Its gate
    /// excludes [`Self::describe`]'s by kind, so one engine holds at most
    /// one of the two geometry reads, and never re-indexes per element.
    index: Option<SectorIndex>,
    /// The sidedef-to-linedef owners, built only when the `linedef` field
    /// or a sidedef `bbox` ask for them.
    owners: Option<Vec<Option<usize>>>,
}

impl<'map, 'req> Engine<'map, 'req> {
    /// Check every filter against the kind's field table and prebuild the
    /// derived reads the query needs.
    pub(super) fn new(
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
        let bounded_by_sectors = bounds.sectors;
        Ok(Self {
            kind: query.kind,
            family: ctx.family,
            ctx,
            map,
            filters,
            bounds,
            describe: wants_shapes(query, bounded_by_box)
                .then(|| Describe::new(map, ctx.defs, ctx.sky_flat)),
            index: wants_points(query, bounded_by_sectors).then(|| SectorIndex::new(map)),
            owners: (query.kind == ElementKind::Sidedefs
                && (bounded_by_box || mentions(&query.r#where, &["linedef"])))
            .then(|| build_owners(map)),
        })
    }

    /// How many elements of the queried kind the map holds.
    pub(super) fn count(&self) -> usize {
        self.kind.count_on(self.map)
    }

    /// Whether element `index` lies within the bound and passes every
    /// filter.
    pub(super) fn matches(&self, index: usize) -> bool {
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

    /// One sector field; `area` and `roles` read the [`SectorShape`] rows
    /// this query's [`Describe`] answers with.
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
                .shape(index)
                .map(|shape| FValue::Num(wide_u64(shape.area))),
            "roles" => self.shape(index).map(|shape| FValue::Roles(shape.roles)),
            _ => None,
        }
    }

    /// The light shape row of sector `index`, when the query asked for a
    /// derived sector field or a sector `bbox`.
    fn shape(&self, index: usize) -> Option<SectorShape> {
        self.describe.as_ref()?.sector_shape(index)
    }

    /// The sector holding the point `x`/`y` through the [`SectorIndex`]
    /// this query built, `None` in the void.
    fn holder(&self, x: i64, y: i64) -> Option<usize> {
        self.index.as_ref()?.point_in_sector(x, y)
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
                .map
                .side_sector(line.front)
                .map(wide_usize)
                .map(FValue::Num),
            "back_sector" => self
                .map
                .side_sector(line.back)
                .map(wide_usize)
                .map(FValue::Num),
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
        self.holder(i64::from(row.x), i64::from(row.y))
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
                .shape(index)
                .and_then(|shape| shape.center)
                .is_some_and(|(x, y)| in_box(bbox, x, y)),
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
        self.holder(x, y)
            .is_some_and(|sector| set.contains(&sector))
    }

    /// Whether the resolvable side `side` opens into one of `set`'s sectors.
    fn side_in_set(&self, side: Option<usize>, set: &BTreeSet<usize>) -> bool {
        self.map
            .side_sector(side)
            .is_some_and(|sector| set.contains(&sector))
    }
}

/// Whether any filter reads one of the `wanted` fields.
fn mentions(filters: &[Filter], wanted: &[&str]) -> bool {
    filters
        .iter()
        .any(|filter| wanted.contains(&filter.field.as_str()))
}

/// Whether the query reads the derived sector fields `area` or `roles`, or
/// bounds sectors by a box: the gate for the [`Describe`] shape read.
fn wants_shapes(query: &Query, bounded_by_box: bool) -> bool {
    query.kind == ElementKind::Sectors
        && (bounded_by_box || mentions(&query.r#where, &["area", "roles"]))
}

/// Whether the query reads the thing `sector` field or bounds things and
/// vertices by sector membership: the gate for the plain [`SectorIndex`]
/// point read. It excludes [`wants_shapes`]'s kind, so one engine builds
/// at most one geometry read.
fn wants_points(query: &Query, bounded_by_sectors: bool) -> bool {
    match query.kind {
        ElementKind::Things => bounded_by_sectors || mentions(&query.r#where, &["sector"]),
        ElementKind::Vertices => bounded_by_sectors,
        ElementKind::Sectors | ElementKind::Linedefs | ElementKind::Sidedefs => false,
    }
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
