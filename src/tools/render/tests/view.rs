//! The `render_view` sprite pass: the camera-relative projection, the
//! depth skip line, the definition-driven dressing and the problem-line
//! dedupe.

use crate::defs::{Definitions, definitions_for};
use crate::render::ViewCamera;
use crate::render::view::ONE_16;
use crate::wad::game::Game;
use crate::wad::map::geometry::SectorIndex;

use crate::tools::render::view::{SPRITE_MIN_TZ16, SpriteSpot, SpriteWiring};

use super::{spawn, two_rooms};

/// A camera standing at `x`/`y`, facing `angle` degrees.
fn view_camera(x: i32, y: i32, angle: i32) -> ViewCamera {
    ViewCamera {
        x,
        y,
        z: 41_i32,
        angle,
        pitch: 0_i32,
        fov: 90_u32,
    }
}

/// `value` map units in raw 16.16.
fn raw16(value: i64) -> i128 {
    i128::from(value).saturating_mul(ONE_16)
}

/// The wiring over the embedded `Doom II` tables, due east from
/// `(64, 128)` like the view fixture camera, reading `geometry` for the
/// home sectors.
fn east_wiring<'ctx>(defs: &'ctx Definitions, geometry: &'ctx SectorIndex) -> SpriteWiring<'ctx> {
    SpriteWiring::new(defs, &view_camera(64, 128, 0), geometry)
}

#[test]
fn sprite_projection_measures_forward_and_right_from_the_camera() {
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let map = two_rooms();
    let geometry = SectorIndex::new(&map);
    let east = east_wiring(&defs, &geometry);
    let imp = SpriteSpot {
        x: 384,
        y: 128,
        z: 0,
        angle: 180,
    };
    assert_eq!(
        east.project(&imp),
        (0_i128, raw16(320)),
        "straight east of the east-facing camera"
    );
    let south_side = SpriteSpot { y: 64, ..imp };
    assert_eq!(
        east.project(&south_side),
        (raw16(64), raw16(320)),
        "south of the view axis is positive right when facing east"
    );
    let north = SpriteWiring::new(&defs, &view_camera(64, 128, 90), &geometry);
    assert_eq!(
        north.project(&imp),
        (raw16(320), 0_i128),
        "due east of a north-facing camera is all right-offset and no depth"
    );
}

#[test]
fn the_skip_line_is_four_map_units_of_forward_depth() {
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let geometry = SectorIndex::new(&two_rooms());
    let east = east_wiring(&defs, &geometry);
    let near = SpriteSpot {
        x: 67,
        y: 128,
        z: 0,
        angle: 0,
    };
    let far = SpriteSpot { x: 68, ..near };
    assert!(
        east.project(&near).1 < SPRITE_MIN_TZ16,
        "three units ahead is under the line"
    );
    assert!(
        east.project(&far).1 >= SPRITE_MIN_TZ16,
        "four units ahead is not"
    );
}

#[test]
fn only_a_known_kind_with_a_sprite_dresses_a_thing() {
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let geometry = SectorIndex::new(&two_rooms());
    let east = east_wiring(&defs, &geometry);
    let imp = east
        .definition(&spawn(3001, 384, 128, 180))
        .expect("the imp carries a sprite");
    assert_eq!(
        (imp.sprite.as_str(), imp.frame, imp.hanging),
        ("TROO", 'A', false)
    );
    assert!(
        east.definition(&spawn(-1, 0, 0, 0)).is_none(),
        "a negative kind dresses nothing"
    );
    assert!(
        east.definition(&spawn(4_000_000, 0, 0, 0)).is_none(),
        "an unknown kind dresses nothing"
    );
}

#[test]
fn sprite_problem_lines_dedup_per_sprite_name() {
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let geometry = SectorIndex::new(&two_rooms());
    let mut wiring = east_wiring(&defs, &geometry);
    wiring.report("POSS", true);
    wiring.report("POSS", true);
    wiring.report("POSS", false);
    wiring.report("HEAD", false);
    assert_eq!(
        wiring.problems,
        [
            "missing sprite POSS".to_owned(),
            "unreadable sprite HEAD".to_owned(),
        ],
        "one line per name and the first failure's word wins"
    );
}
