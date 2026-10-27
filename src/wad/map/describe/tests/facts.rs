//! The per-sector row: what [`sector_facts`] answers for a hand-drawn
//! room — the measured area, box and label centre, the cosmetic fields,
//! the named sector special, the wall tally, the counted things and the
//! empty answers for a missing and a boundary-less sector — and the
//! serialised shape of one row.

use alloc::vec;

use serde_json::{json, to_value};

use crate::wad::map::Thing;
use crate::wad::map::common::fixtures::{room, spawn};
use crate::wad::map::consts::{HEXEN_SKY_FLAT, SKY_FLAT};
use crate::wad::map::describe::{SpecialInfo, ThingCount, sector_facts};
use crate::wad::map::geometry::point_in_sector;

use super::{doom_defs, doom2_defs, elbow_map, retune, square_map, stand, switch_map};

#[test]
fn a_square_room_reports_area_box_centre_and_look() {
    let map = square_map();
    let facts = sector_facts(&map, &doom2_defs(), SKY_FLAT, 0).expect("sector 0 exists");
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
    assert!(facts.roles.is_empty(), "a plain room is nothing special");
    assert!(facts.neighbours.is_empty(), "and stands alone");
}

#[test]
fn a_concave_room_reports_its_true_area_and_a_centre_in_the_foot() {
    let map = elbow_map();
    let facts = sector_facts(&map, &doom2_defs(), SKY_FLAT, 0).expect("sector 0 exists");
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
fn a_sky_ceiling_is_flagged_and_wears_the_sky_role() {
    let mut map = square_map();
    let mut outdoor = room(0, 128, 160, 0);
    outdoor.ceiling_flat = "F_SKY1".to_owned();
    retune(&mut map, outdoor);
    let facts = sector_facts(&map, &doom2_defs(), SKY_FLAT, 0).expect("sector 0 exists");
    assert!(facts.sky, "an `F_SKY1` ceiling is the sky");
    assert_eq!(facts.ceiling_flat, "F_SKY1", "and the name rides along");
    assert_eq!(facts.roles, vec!["sky".to_owned()], "and the role too");

    let mut indoors = square_map();
    let mut floored = room(0, 128, 160, 0);
    floored.floor_flat = "F_SKY1".to_owned();
    retune(&mut indoors, floored);
    let under = sector_facts(&indoors, &doom2_defs(), SKY_FLAT, 0).expect("sector 0 exists");
    assert!(!under.sky, "an `F_SKY1` floor is nobody's sky");
    assert!(under.roles.is_empty(), "and it earns no role either");
}

#[test]
fn the_sky_field_follows_the_sky_flat_it_is_read_with() {
    let mut map = square_map();
    let mut hexen_sky = room(0, 128, 160, 0);
    hexen_sky.ceiling_flat = "F_SKY".to_owned();
    retune(&mut map, hexen_sky);
    let as_hexen = sector_facts(&map, &doom2_defs(), HEXEN_SKY_FLAT, 0)
        .expect("a Hexen read of the same ceiling");
    assert!(as_hexen.sky, "an `F_SKY` ceiling is the sky for `Hexen`");
    assert_eq!(as_hexen.roles, vec!["sky".to_owned()], "and earns the role");
    let as_vanilla =
        sector_facts(&map, &doom2_defs(), SKY_FLAT, 0).expect("a vanilla read of the same ceiling");
    assert!(
        !as_vanilla.sky,
        "`F_SKY` is just a flat name to a vanilla read"
    );
    assert!(as_vanilla.roles.is_empty(), "and it earns no role there");
}

#[test]
fn sector_specials_are_named_through_the_definitions() {
    let mut map = square_map();
    let mut secret = room(0, 128, 160, 0);
    secret.special = 9_i32;
    retune(&mut map, secret);
    let facts = sector_facts(&map, &doom_defs(), SKY_FLAT, 0).expect("sector 0 exists");
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
    let unnamed = sector_facts(&unassigned, &doom_defs(), SKY_FLAT, 0).expect("sector 0 exists");
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
    let off_table = sector_facts(&negative, &doom_defs(), SKY_FLAT, 0).expect("sector 0 exists");
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
    let facts = sector_facts(&map, &doom2_defs(), SKY_FLAT, 0).expect("sector 0 exists");
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
    let facts = sector_facts(&map, &doom2_defs(), SKY_FLAT, 0).expect("sector 0 exists");
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
        sector_facts(&map, &doom2_defs(), SKY_FLAT, 9).is_none(),
        "no sector 9 in a one-sector map"
    );
    let mut lonelier = square_map();
    lonelier.sectors.push(room(0, 0, 0, 0));
    let bare = sector_facts(&lonelier, &doom2_defs(), SKY_FLAT, 1).expect("sector 1 exists");
    assert_eq!(bare.area, 0, "a sector with no boundary has no area");
    assert_eq!(bare.bbox, None, "and no box");
    assert_eq!(bare.center, None, "and no centre");
    assert!(bare.wall_textures.is_empty(), "and no walls");
    assert!(bare.things.is_empty(), "and nothing stands in it");
    assert!(bare.neighbours.is_empty(), "and nobody next to it");
    assert!(bare.roles.is_empty(), "and it states no role at all");
}

#[test]
fn the_row_serialises_to_the_tickets_shape() {
    let value =
        to_value(sector_facts(&square_map(), &doom2_defs(), SKY_FLAT, 0).expect("sector 0"))
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
            "roles": [],
            "neighbours": [],
        }),
        "every field the two ticket halves named, in their names"
    );
}
