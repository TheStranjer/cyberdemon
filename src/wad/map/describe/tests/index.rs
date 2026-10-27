//! The one [`SectorIndex`] a [`Describe`] folds: every geometry read of a
//! described row — the shoelace area, the box, the label centre, the
//! memoized thing homes and the neighbour groups — compared sector by
//! sector against the thin free-function wrappers of
//! [`crate::wad::map::geometry`], on the hand-drawn fixtures and a
//! plan-built map with things stood in the void, on a shared wall and
//! across a nested-sector tie; and the read of a sector the index bounds
//! nowhere, which still describes.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::defs::Definitions;
use crate::wad::classify::MapFormat;
use crate::wad::map::common::fixtures::{room, spawn};
use crate::wad::map::consts::SKY_FLAT;
use crate::wad::map::describe::shape::{label_point, shoelace_area};
use crate::wad::map::describe::{Describe, Edge, ThingCount};
use crate::wad::map::geometry::{point_in_sector, sector_bbox, sector_boundary};
use crate::wad::map::model::Map;

use super::plans::crewed_plan;
use super::{
    doom2_defs, elbow_map, nested_exit_map, split_divider_map, square_map, stand, switch_map,
};

/// The square room with one thing dead centre and one standing far off in
/// the void.
fn voided_map() -> Map {
    let mut map = square_map();
    stand(&mut map, spawn(3001, 128, 128, 0));
    stand(&mut map, spawn(3001, 4_000, 4_000, 0));
    map
}

/// The two split-wall rooms with a barrel stood on each shared divider
/// line and an imp inside room `A`: the on-boundary front rule's points.
fn walled_map() -> Map {
    let mut map = split_divider_map(0, 0);
    stand(&mut map, spawn(2035, 256, 64, 0));
    stand(&mut map, spawn(2035, 256, 192, 0));
    stand(&mut map, spawn(3001, 128, 128, 0));
    map
}

/// The nested squares with a barrel in the inner square (the nested-
/// sector tie), one in the ring outside it, and an imp in the void.
fn nested_stood_map() -> Map {
    let mut map = nested_exit_map(MapFormat::Doom, 0, 0);
    stand(&mut map, spawn(2035, 450, 450, 0));
    stand(&mut map, spawn(2035, 200, 200, 0));
    stand(&mut map, spawn(3001, 1_100, 500, 0));
    map
}

/// The square room with a second sector that bounds nothing and holds no
/// line, and a thing standing in the first one.
fn boundless_map() -> Map {
    let mut map = square_map();
    map.sectors.push(room(0, 0, 0, 0));
    stand(&mut map, spawn(3001, 128, 128, 0));
    map
}

/// `sector`'s directed boundary edges read through the free
/// [`sector_boundary`] wrapper: the reference for what the row's stored
/// index answers.
fn wrapper_edges(map: &Map, sector: usize) -> Vec<Edge> {
    sector_boundary(map, sector)
        .into_iter()
        .filter_map(|(line, is_front)| {
            let ((sx, sy), (ex, ey)) = map
                .linedefs
                .get(line)
                .and_then(|definition| map.line_endpoints(definition))?;
            let from = (i64::from(sx), i64::from(sy));
            let to = (i64::from(ex), i64::from(ey));
            Some(if is_front { (from, to) } else { (to, from) })
        })
        .collect()
}

/// `sector`'s neighbour groups read through the free [`sector_boundary`]
/// wrapper.
fn wrapper_groups(map: &Map, sector: usize) -> Vec<(usize, Vec<usize>)> {
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (line, is_front) in sector_boundary(map, sector) {
        let definition = map.linedefs.get(line);
        let across = definition
            .and_then(|def| map.side_sector(if is_front { def.back } else { def.front }))
            .filter(|other| *other != sector);
        if let Some(other) = across {
            groups.entry(other).or_default().push(line);
        }
    }
    groups.into_iter().collect()
}

/// The display name of thing type `kind`: the definitions' name for a
/// defined type, `Unknown <kind>` for anything else.
fn wrapper_name(defs: &Definitions, kind: i32) -> String {
    let known = u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number))
        .map(|def| def.name.clone());
    known.unwrap_or_else(|| format!("Unknown {kind}"))
}

/// `sector`'s things counted by name through the free [`point_in_sector`]
/// wrapper — the per-thing rescan the row now memoizes — sorted as a row
/// sorts.
fn wrapper_things(map: &Map, defs: &Definitions, sector: usize) -> Vec<ThingCount> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in &map.things {
        let at = point_in_sector(map, i64::from(row.x), i64::from(row.y));
        if at != Some(sector) {
            continue;
        }
        let slot = counts.entry(wrapper_name(defs, row.kind)).or_insert(0_u32);
        *slot = slot.saturating_add(1);
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

#[test]
fn the_stored_index_answers_every_row_read_the_free_wrappers_do() {
    let (plan, _) = crewed_plan();
    let maps = vec![
        voided_map(),
        walled_map(),
        nested_stood_map(),
        boundless_map(),
        elbow_map(),
        switch_map(),
        plan,
    ];
    let defs = doom2_defs();
    for map in &maps {
        let describe = Describe::new(map, &defs, SKY_FLAT);
        for sector in 0..map.sectors.len() {
            let facts = describe
                .sector(sector)
                .expect("the fixture holds every sector");
            let edges = wrapper_edges(map, sector);
            assert_eq!(facts.area, shoelace_area(&edges), "sector {sector}'s area");
            assert_eq!(
                facts.bbox,
                sector_bbox(map, sector).map(<[i32; 4]>::from),
                "sector {sector}'s box"
            );
            assert_eq!(
                facts.center,
                label_point(&edges),
                "sector {sector}'s centre"
            );
            assert_eq!(
                facts.things,
                wrapper_things(map, &defs, sector),
                "sector {sector}'s memoized homes match the per-thing rescan"
            );
            let rows: Vec<(usize, Vec<usize>)> = facts
                .neighbours
                .iter()
                .map(|neighbour| (neighbour.sector, neighbour.lines.clone()))
                .collect();
            assert_eq!(
                rows,
                wrapper_groups(map, sector),
                "sector {sector}'s neighbour groups"
            );
        }
    }
}

#[test]
fn the_memoized_homes_answer_the_void_wall_and_tie_points() {
    let defs = doom2_defs();

    let manned_square = voided_map();
    let voided = Describe::new(&manned_square, &defs, SKY_FLAT);
    let square = voided.sector(0).expect("the square exists");
    assert_eq!(
        square.things,
        vec![ThingCount {
            name: "Imp".to_owned(),
            count: 1,
        }],
        "the centred imp counts and the void thing counts nowhere"
    );

    let stood_walls = walled_map();
    let walled = Describe::new(&stood_walls, &defs, SKY_FLAT);
    let hall = walled.sector(0).expect("room A exists");
    assert_eq!(
        hall.things,
        vec![
            ThingCount {
                name: "Barrel".to_owned(),
                count: 2,
            },
            ThingCount {
                name: "Imp".to_owned(),
                count: 1,
            },
        ],
        "both barrels stand on the divider's front side, A's"
    );
    let back = walled.sector(1).expect("room B exists");
    assert!(
        back.things.is_empty(),
        "the back side claims nothing that stands on its own wall"
    );

    let stood_nests = nested_stood_map();
    let nested = Describe::new(&stood_nests, &defs, SKY_FLAT);
    let outer = nested.sector(0).expect("the outer square exists");
    let inner = nested.sector(1).expect("the inner square exists");
    assert_eq!(
        inner.things,
        vec![ThingCount {
            name: "Barrel".to_owned(),
            count: 1,
        }],
        "of the nested holders the smaller box homes the inner barrel"
    );
    assert_eq!(
        outer.things,
        vec![ThingCount {
            name: "Barrel".to_owned(),
            count: 1,
        }],
        "the ring barrel stays outer and the void imp homes nowhere"
    );
}

#[test]
fn a_sector_the_index_bounds_nowhere_still_describes() {
    let defs = doom2_defs();
    let unbounded = boundless_map();
    let describe = Describe::new(&unbounded, &defs, SKY_FLAT);
    let bare = describe.sector(1).expect("sector 1 exists");
    assert_eq!(bare.sector, 1, "the index echoes");
    assert_eq!(bare.area, 0, "no boundary bounds no area");
    assert_eq!(bare.bbox, None, "and no box");
    assert_eq!(bare.center, None, "and no centre");
    assert!(bare.wall_textures.is_empty(), "and no walls");
    assert!(
        bare.things.is_empty(),
        "a sector with no box homes no thing"
    );
    assert!(bare.neighbours.is_empty(), "and nobody next to it");
    assert!(bare.roles.is_empty(), "and it states no role at all");
    let full = describe.sector(0).expect("sector 0 exists");
    assert_eq!(
        full.things,
        vec![ThingCount {
            name: "Imp".to_owned(),
            count: 1,
        }],
        "the bounded sector still counts the thing standing in it"
    );
}
