//! Pure per-sector facts over a decoded [`Map`]: the geometry-and-identity
//! half of the future `describe_map` core.
//!
//! [`sector_facts`] turns one sector into a [`SectorFacts`] row: what it
//! measures (an integer shoelace [`SectorFacts::area`] over its directed
//! boundary, a bounding box and a label point the even-odd rule places
//! inside), how it looks (heights, light, flats, sky, the definitions'
//! name for its sector special, its tag, and a tally of the textures its
//! sides wear) and what stands in it (things counted by definition name).
//! Roles and neighbours join these rows in the next slice.
//!
//! Like [`geometry`], this is a function of the map model and the passed
//! [`Definitions`] alone: no sessions, no `MCP`, no editing. References
//! that do not resolve simply do not take part in the answer, so a sector
//! with no usable boundary reports zero area and no box or centre.
//!
//! [`geometry`]: crate::wad::map::geometry

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString as _;
use alloc::vec::Vec;

use serde::Serialize;

use super::geometry::{point_in_sector, sector_bbox, sector_boundary};
use super::model::Map;
use crate::defs::Definitions;

/// The ceiling flat that means open sky, the name the renderer switches on.
const SKY_FLAT: &str = "F_SKY1";
/// The model's name for "no texture on this side".
const NO_TEXTURE: &str = "-";
/// Half of the shoelace sum: the doubled area divides by it.
const AREA_HALF_OF: u128 = 2_u128;

/// One world-space map point, coordinates widened to `i64`.
type Point = (i64, i64);

/// One directed boundary edge: a `(from, to)` pair of [`Point`]s.
type Edge = (Point, Point);

/// The number of a sector effect and the name the game's table gives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SpecialInfo {
    /// The stored [`crate::wad::map::Sector::special`] value.
    pub value: i32,
    /// The display name from [`Definitions::sector_type`], or the plain
    /// number as text when the table has no row for it.
    pub name: String,
}

/// One kind of thing standing in a sector, and how many of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ThingCount {
    /// The display name from [`Definitions::thing`], or `Unknown <kind>`
    /// for a type the game does not define.
    pub name: String,
    /// How many things of this kind stand in the sector.
    pub count: u32,
}

/// One sector described: geometry, look, contents (no roles/neighbours yet).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SectorFacts {
    /// The index into [`Map::sectors`] this row describes.
    pub sector: usize,
    /// The even-odd floor area in square map units: the integer shoelace
    /// over the sector's directed boundary, `|sum| / 2` in exact `i128`.
    pub area: u64,
    /// The boundary's `(min_x, min_y, max_x, max_y)` box, `None` when the
    /// sector bounds no resolvable vertex.
    pub bbox: Option<[i32; 4]>,
    /// A world point the even-odd scan places inside the sector, `None`
    /// with no boundary.
    pub center: Option<[i64; 2]>,
    /// The floor height in map units.
    pub floor: i32,
    /// The ceiling height in map units.
    pub ceiling: i32,
    /// `ceiling` minus `floor`, saturating.
    pub height: i32,
    /// The light level as stored.
    pub light: i32,
    /// The floor flat name.
    pub floor_flat: String,
    /// The ceiling flat name.
    pub ceiling_flat: String,
    /// Whether the ceiling is the `F_SKY1` sky flat.
    pub sky: bool,
    /// The sector effect named through the definitions, `None` for
    /// special `0`.
    pub special: Option<SpecialInfo>,
    /// The sector tag matched by linedef and thing triggers.
    pub tag: i32,
    /// How often each texture name is worn by this sector's wall sides
    /// (`"-"` never counted), ordered by name.
    pub wall_textures: BTreeMap<String, u32>,
    /// The things standing here, counted by definition name, sorted by
    /// count descending then name ascending.
    pub things: Vec<ThingCount>,
}

/// Describe sector `sector` of `map`, naming its sector special and its
/// things' types through the game definitions `defs`.
///
/// `None` when the map holds no sector with that index. A sector with no
/// usable boundary still describes: zero area, no box, no centre, no
/// textures and no things.
#[must_use]
#[inline]
pub fn sector_facts(map: &Map, defs: &Definitions, sector: usize) -> Option<SectorFacts> {
    let row = map.sectors.get(sector)?;
    let edges = boundary_edges(map, sector);
    Some(SectorFacts {
        sector,
        area: shoelace_area(&edges),
        bbox: sector_bbox(map, sector).map(<[i32; 4]>::from),
        center: label_point(&edges),
        floor: row.floor_height,
        ceiling: row.ceiling_height,
        height: row.ceiling_height.saturating_sub(row.floor_height),
        light: row.light,
        floor_flat: row.floor_flat.clone(),
        ceiling_flat: row.ceiling_flat.clone(),
        sky: row.ceiling_flat == SKY_FLAT,
        special: special_info(row.special, defs),
        tag: row.tag,
        wall_textures: wall_textures(map, sector),
        things: thing_counts(map, defs, sector),
    })
}

/// `sector`'s boundary as directed world-space edges: front sides walk
/// `v1` to `v2`, back sides `v2` to `v1`, linedef order; lines whose
/// vertices do not resolve are skipped.
fn boundary_edges(map: &Map, sector: usize) -> Vec<Edge> {
    sector_boundary(map, sector)
        .into_iter()
        .filter_map(|(index, is_front)| {
            let line = map.linedefs.get(index)?;
            let start = map.vertices.get(line.v1)?;
            let end = map.vertices.get(line.v2)?;
            let from = (i64::from(start.x), i64::from(start.y));
            let to = (i64::from(end.x), i64::from(end.y));
            Some(if is_front { (from, to) } else { (to, from) })
        })
        .collect()
}

/// The `u64` floor area of the directed `edges` loop set: the shoelace
/// sum of every edge's vertex cross product in exact `i128`, halved on its
/// absolute value. One consistent direction makes a hole's loop subtract
/// itself, so a sector with an island reports the even-odd sum.
fn shoelace_area(edges: &[Edge]) -> u64 {
    let mut sum = 0_i128;
    for edge in edges {
        let left = i128::from(edge.0.0).saturating_mul(i128::from(edge.1.1));
        let right = i128::from(edge.1.0).saturating_mul(i128::from(edge.0.1));
        sum = sum.saturating_add(left.saturating_sub(right));
    }
    let half = sum.unsigned_abs().checked_div(AREA_HALF_OF);
    u64::try_from(half.unwrap_or(u128::MAX)).unwrap_or(u64::MAX)
}

/// The world-space port of the renderer's sector label point: the middle
/// row of the boundary's box is scanned with the even-odd rule, the
/// longest inside interval wins and its midpoint is the centre; a polygon
/// the row misses falls back to its first boundary vertex. No edges, no
/// centre.
fn label_point(edges: &[Edge]) -> Option<[i64; 2]> {
    let (min_y, max_y) = edge_rows(edges)?;
    let row = midpoint(min_y, max_y);
    let mut crossings: Vec<i64> = edges
        .iter()
        .filter(|edge| crosses_row(edge, row))
        .map(|edge| intersect_x(edge.0, edge.1, row))
        .collect();
    crossings.sort_unstable();
    longest_interval(&crossings)
        .map(|(lo, hi)| [midpoint(lo, hi), row])
        .or_else(|| edges.first().map(|edge| <[i64; 2]>::from(edge.0)))
}

/// Whether `edge` straddles row `row` under the half-open even-odd rule.
const fn crosses_row(edge: &Edge, row: i64) -> bool {
    (edge.0.1 > row) != (edge.1.1 > row)
}

/// The `(lowest, highest)` endpoint rows over `edges`, `None` when empty.
fn edge_rows(edges: &[Edge]) -> Option<(i64, i64)> {
    let rows = edges.iter().flat_map(|edge| [edge.0.1, edge.1.1]);
    let min = rows.clone().min()?;
    Some((min, rows.max()?))
}

/// The midpoint of two world coordinates, rounded down.
const fn midpoint(low: i64, high: i64) -> i64 {
    low.saturating_add(high).div_euclid(2)
}

/// The widest `(lo, hi)` interval between consecutive pairs of the sorted
/// `crossings`, `None` when fewer than two of them exist.
fn longest_interval(crossings: &[i64]) -> Option<(i64, i64)> {
    let mut best: Option<(i64, i64)> = None;
    for pair in crossings.chunks_exact(2) {
        let Ok(&[lo, hi]) = <&[i64; 2]>::try_from(pair) else {
            continue;
        };
        if wider_than((lo, hi), best) {
            best = Some((lo, hi));
        }
    }
    best
}

/// Whether the interval `one` is wider than `best` (or `best` is unset).
const fn wider_than(one: (i64, i64), best: Option<(i64, i64)>) -> bool {
    match best {
        None => true,
        Some((b_lo, b_hi)) => one.1.saturating_sub(one.0) > b_hi.saturating_sub(b_lo),
    }
}

/// The column where the `start` to `end` line crosses row `y`,
/// floor-rounded in exact `i128`; a horizontal edge answers `start`'s
/// column (the even-odd filter never asks about one).
fn intersect_x(start: Point, end: Point, y: i64) -> i64 {
    let rise = i128::from(y).saturating_sub(i128::from(start.1));
    let span = i128::from(end.1).saturating_sub(i128::from(start.1));
    scale_down(start.0, end.0.saturating_sub(start.0), rise, span)
}

/// `base + delta * num / den` in exact `i128`, floor-rounded and
/// saturating back to `i64`; a zero `den` answers `base`.
fn scale_down(base: i64, delta: i64, num: i128, den: i128) -> i64 {
    if den == 0_i128 {
        return base;
    }
    let (n, d) = if den < 0_i128 {
        (num.saturating_neg(), den.saturating_neg())
    } else {
        (num, den)
    };
    let drawn = i128::from(base)
        .saturating_mul(d)
        .saturating_add(i128::from(delta).saturating_mul(n))
        .div_euclid(d);
    narrow(drawn)
}

/// The `i64` nearest an `i128` value, saturating at the bounds.
fn narrow(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0_i128 { i64::MIN } else { i64::MAX })
}

/// The [`SpecialInfo`] of sector special `value`: `None` for the plain `0`
/// that means "no effect", the table's name for a number
/// [`Definitions::sector_type`] knows, and the plain number as text for
/// everything else.
fn special_info(value: i32, defs: &Definitions) -> Option<SpecialInfo> {
    if value == 0_i32 {
        return None;
    }
    let name = u32::try_from(value)
        .ok()
        .and_then(|number| defs.sector_type(number))
        .map_or_else(|| value.to_string(), |row| row.name.clone());
    Some(SpecialInfo { value, name })
}

/// The texture tally of the sides opening into `sector`: for every
/// boundary line, each of its two sides whose own sector resolves to
/// `sector` contributes its upper, middle and lower names; `"-"` never
/// counts and a dangling side reference contributes nothing.
fn wall_textures(map: &Map, sector: usize) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for (index, _) in sector_boundary(map, sector) {
        let Some(line) = map.linedefs.get(index) else {
            continue;
        };
        for side in [line.front, line.back].into_iter().flatten() {
            count_side(&mut counts, map, side, sector);
        }
    }
    counts
}

/// Record the three texture names of `map`'s sidedef `side` in `counts`,
/// when that side opens into `sector`.
fn count_side(counts: &mut BTreeMap<String, u32>, map: &Map, side: usize, sector: usize) {
    let Some(row) = map.sidedefs.get(side) else {
        return;
    };
    if row.sector != sector {
        return;
    }
    for texture in [row.upper.as_str(), row.lower.as_str(), row.middle.as_str()] {
        if texture != NO_TEXTURE {
            bump(counts, texture);
        }
    }
}

/// The display name of thing type `kind`: the table's name for a type
/// [`Definitions::thing`] knows, `Unknown <kind>` for anything else
/// (including a negative type number).
fn thing_name(defs: &Definitions, kind: i32) -> String {
    u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number))
        .map_or_else(|| format!("Unknown {kind}"), |row| row.name.clone())
}

/// Every thing that stands in `sector` - skill-independent, whatever its
/// presence bits say - counted by definition name and sorted by count
/// descending, then by name ascending.
fn thing_counts(map: &Map, defs: &Definitions, sector: usize) -> Vec<ThingCount> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in &map.things {
        let at = point_in_sector(map, i64::from(row.x), i64::from(row.y));
        if at != Some(sector) {
            continue;
        }
        bump(&mut counts, &thing_name(defs, row.kind));
    }
    let mut rows: Vec<ThingCount> = counts
        .into_iter()
        .map(|(name, count)| ThingCount { name, count })
        .collect();
    rows.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    rows
}

/// Record `name` once more in `counts`.
fn bump(counts: &mut BTreeMap<String, u32>, name: &str) {
    let slot = counts.entry(name.to_owned()).or_insert(0_u32);
    *slot = slot.saturating_add(1);
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use serde_json::{json, to_value};

    use super::{SpecialInfo, ThingCount, sector_facts};
    use crate::defs::{Definitions, definitions_for};
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::common::fixtures::{name, room, side, solid_line, spawn, vertex};
    use crate::wad::map::geometry::point_in_sector;
    use crate::wad::map::{Map, Sector, Thing};

    /// The embedded `DOOM` definitions (the sector table's home).
    fn doom_defs() -> Definitions {
        definitions_for(Game::Doom).expect("the embedded Doom table is valid")
    }

    /// The embedded `DOOM II` definitions (the thing table's superset).
    fn doom2_defs() -> Definitions {
        definitions_for(Game::Doom2).expect("the embedded Doom II table is valid")
    }

    /// The 256x256 square room: `WALL1` on all four one-sided walls,
    /// floor `0` under a 128-unit ceiling, light 160, tag 0, sector 0.
    fn square_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(256, 256),
            vertex(0, 256),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            solid_line(2, 3, 2),
            solid_line(3, 0, 3),
        ];
        map.sidedefs = (0_usize..4_usize)
            .map(|_| side("-", "-", "WALL1", 0_usize))
            .collect();
        map.sectors = vec![room(0, 128, 160, 0)];
        map
    }

    /// The concave room: a 512x256 foot with a 256x256 tower on its west
    /// end, six `WALL1` sides around sector 0.
    fn elbow_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(256, 512),
            vertex(0, 512),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            solid_line(2, 3, 2),
            solid_line(3, 4, 3),
            solid_line(4, 5, 4),
            solid_line(5, 0, 5),
        ];
        map.sidedefs = (0_usize..6_usize)
            .map(|_| side("-", "-", "WALL1", 0_usize))
            .collect();
        map.sectors = vec![room(0, 128, 160, 0)];
        map
    }

    /// The square room with its east side split in two and its north side
    /// in three: six `WALL1` sides and one `SWITCH1` middle around
    /// sector 0.
    fn switch_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(256, 128),
            vertex(256, 256),
            vertex(192, 256),
            vertex(64, 256),
            vertex(0, 256),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 1),
            solid_line(2, 3, 2),
            solid_line(3, 4, 3),
            solid_line(4, 5, 4),
            solid_line(5, 6, 5),
            solid_line(6, 0, 6),
        ];
        map.sidedefs = vec![
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "-", "SWITCH1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
        ];
        map.sectors = vec![room(0, 128, 160, 0)];
        map
    }

    /// Retune sector 0 of `map` in place.
    fn retune(map: &mut Map, row: Sector) {
        map.sectors = vec![row];
    }

    /// Stands `row` among `map`'s things.
    fn stand(map: &mut Map, row: Thing) {
        map.things.push(row);
    }

    #[test]
    fn a_square_room_reports_area_box_centre_and_look() {
        let map = square_map();
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert_eq!(facts.sector, 0, "the index echoes");
        assert_eq!(facts.area, 0x0001_0000_u64, "256x256 of floor");
        assert_eq!(
            facts.bbox,
            Some([0_i32, 0_i32, 256_i32, 256_i32]),
            "the box spans the walls"
        );
        assert_eq!(facts.center, Some([128_i64, 128_i64]), "the square's heart");
        let at = facts.center.expect("the asserted centre");
        assert_eq!(
            point_in_sector(&map, at[0], at[1]),
            Some(0),
            "and the point really stands in the sector"
        );
        assert_eq!(facts.floor, 0_i32, "floor height");
        assert_eq!(facts.ceiling, 128_i32, "ceiling height");
        assert_eq!(facts.height, 128_i32, "ceiling minus floor");
        assert_eq!(facts.light, 160_i32, "light level");
        assert_eq!(facts.floor_flat, "FLOOR1", "floor flat");
        assert_eq!(facts.ceiling_flat, "FLOOR1", "ceiling flat");
        assert!(!facts.sky, "a solid ceiling is no sky");
        assert_eq!(facts.special, None, "special 0 describes as nothing");
        assert_eq!(facts.tag, 0_i32, "tag passthrough");
        assert_eq!(
            to_value(&facts.wall_textures).expect("counts serialise"),
            json!({ "WALL1": 4_u64 }),
            "four `WALL1` sides"
        );
        assert!(facts.things.is_empty(), "nobody stood in");
    }

    #[test]
    fn a_concave_room_reports_its_true_area_and_a_centre_in_the_foot() {
        let map = elbow_map();
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert_eq!(facts.area, 196_608_u64, "512x256 plus 256x256");
        assert_eq!(
            facts.bbox,
            Some([0_i32, 0_i32, 512_i32, 512_i32]),
            "the box bounds the whole room, concavity included"
        );
        assert_eq!(
            facts.center,
            Some([128_i64, 256_i64]),
            "the middle row's widest inside interval is the tower's base"
        );
        assert_eq!(
            point_in_sector(&map, 128, 256),
            Some(0),
            "and the label point stands inside the sector"
        );
    }

    #[test]
    fn a_sky_ceiling_is_flagged_and_a_sky_floor_is_not() {
        let mut map = square_map();
        let mut outdoor = room(0, 128, 160, 0);
        outdoor.ceiling_flat = "F_SKY1".to_owned();
        retune(&mut map, outdoor);
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert!(facts.sky, "an `F_SKY1` ceiling is the sky");
        assert_eq!(facts.ceiling_flat, "F_SKY1", "and the name rides along");

        let mut indoors = square_map();
        let mut floored = room(0, 128, 160, 0);
        floored.floor_flat = "F_SKY1".to_owned();
        retune(&mut indoors, floored);
        let under = sector_facts(&indoors, &doom2_defs(), 0).expect("sector 0 exists");
        assert!(!under.sky, "an `F_SKY1` floor is nobody's sky");
    }

    #[test]
    fn sector_specials_are_named_through_the_definitions() {
        let mut map = square_map();
        let mut secret = room(0, 128, 160, 0);
        secret.special = 9_i32;
        retune(&mut map, secret);
        let facts = sector_facts(&map, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            facts.special,
            Some(SpecialInfo {
                value: 9,
                name: "Secret".to_owned(),
            }),
            "the Doom table names type 9"
        );

        let mut unassigned = square_map();
        let mut odd = room(0, 128, 160, 0);
        odd.special = 42_i32;
        retune(&mut unassigned, odd);
        let unnamed = sector_facts(&unassigned, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            unnamed.special,
            Some(SpecialInfo {
                value: 42,
                name: "42".to_owned(),
            }),
            "a number the table has no row for falls back to its own text"
        );

        let mut negative = square_map();
        let mut below = room(0, 128, 160, 0);
        below.special = -3_i32;
        retune(&mut negative, below);
        let off_table = sector_facts(&negative, &doom_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            off_table.special,
            Some(SpecialInfo {
                value: -3,
                name: "-3".to_owned(),
            }),
            "and a negative value reads as its own text too"
        );
    }

    #[test]
    fn wall_textures_count_both_faces_of_every_side_opening_here() {
        let map = switch_map();
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            to_value(&facts.wall_textures).expect("counts serialise"),
            json!({ "SWITCH1": 1_u64, "WALL1": 6_u64 }),
            "six walls and one switch panel, split sides folded per name"
        );
        assert_eq!(
            facts.area, 0x0001_0000_u64,
            "the split walls do not move the box"
        );
    }

    #[test]
    fn things_are_counted_by_name_sorted_by_size_and_all_skills_count() {
        let mut map = square_map();
        stand(&mut map, spawn(3001, 64, 64, 0));
        stand(&mut map, spawn(3001, 128, 64, 0));
        stand(
            &mut map,
            Thing {
                flags: 4,
                ..spawn(3001, 64, 192, 0)
            },
        );
        stand(&mut map, spawn(2001, 192, 192, 0));
        stand(&mut map, spawn(9999, 128, 192, 0));
        stand(&mut map, spawn(3001, 4_000, 4_000, 7));
        let facts = sector_facts(&map, &doom2_defs(), 0).expect("sector 0 exists");
        assert_eq!(
            facts.things,
            vec![
                ThingCount {
                    name: "Imp".to_owned(),
                    count: 3,
                },
                ThingCount {
                    name: "Shotgun".to_owned(),
                    count: 1,
                },
                ThingCount {
                    name: "Unknown 9999".to_owned(),
                    count: 1,
                },
            ],
            "count descending, ties by name ascending; the hard-skill imp \
             counts and the one in the void does not"
        );
    }

    #[test]
    fn a_missing_sector_describes_as_nothing_and_an_empty_one_as_zeroes() {
        let map = square_map();
        assert!(
            sector_facts(&map, &doom2_defs(), 9).is_none(),
            "no sector 9 in a one-sector map"
        );
        let mut lonelier = square_map();
        lonelier.sectors.push(room(0, 0, 0, 0));
        let bare = sector_facts(&lonelier, &doom2_defs(), 1).expect("sector 1 exists");
        assert_eq!(bare.area, 0, "a sector with no boundary has no area");
        assert_eq!(bare.bbox, None, "and no box");
        assert_eq!(bare.center, None, "and no centre");
        assert!(bare.wall_textures.is_empty(), "and no walls");
        assert!(bare.things.is_empty(), "and nothing stands in it");
    }

    #[test]
    fn the_row_serialises_to_the_tickets_shape() {
        let value = to_value(sector_facts(&square_map(), &doom2_defs(), 0).expect("sector 0"))
            .expect("facts serialise");
        assert_eq!(
            value,
            json!({
                "sector": 0_u64,
                "area": 0x0001_0000_u64,
                "bbox": [0_i64, 0_i64, 256_i64, 256_i64],
                "center": [128_i64, 128_i64],
                "floor": 0_i64,
                "ceiling": 128_i64,
                "height": 128_i64,
                "light": 160_i64,
                "floor_flat": "FLOOR1",
                "ceiling_flat": "FLOOR1",
                "sky": false,
                "special": null,
                "tag": 0_i64,
                "wall_textures": { "WALL1": 4_u64 },
                "things": [],
            }),
            "every field the parent ticket named, in its names"
        );
    }
}
