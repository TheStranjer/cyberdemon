//! The minimap arguments: size, radius and centre resolution, the fully
//! resolved request and the painted automap colours.

use serde_json::json;

use crate::defs::definitions_for;
use crate::render::{AutomapColors, PlayerMark};
use crate::wad::game::Game;
use crate::wad::map::Map;

use crate::tools::maps::params::PointParam;
use crate::tools::render::minimap::{
    MINIMAP_FIT_PAD, RenderMinimapParams, paint_minimap, resolve_centre, resolve_minimap,
    resolve_radius, resolve_size,
};

use super::{fixture_palette, two_rooms};

#[test]
fn minimap_size_defaults_and_refuses_edges_outside_the_range() {
    assert_eq!(
        resolve_size(None).expect("the default"),
        256,
        "256 by default"
    );
    assert_eq!(resolve_size(Some(32)).expect("the floor"), 32, "the floor");
    assert_eq!(
        resolve_size(Some(1_024)).expect("the ceiling"),
        1_024,
        "the ceiling"
    );
    for bad in [16_u32, 2_048_u32] {
        let error = resolve_size(Some(bad)).expect_err("outside the range");
        assert_eq!(error.code(), "invalid_argument", "size {bad} code");
        assert!(
            error.to_string().contains("32..=1024"),
            "the message shows the range: {error}"
        );
    }
}

#[test]
fn minimap_radius_validates_and_fits_the_whole_map() {
    let map = two_rooms();
    assert_eq!(
        resolve_radius(Some(64), &map).expect("an explicit radius"),
        64,
        "the given radius"
    );
    assert_eq!(
        resolve_radius(None, &map).expect("the whole map fits"),
        257,
        "half the 512-span rounded up plus the fit padding"
    );
    assert_eq!(MINIMAP_FIT_PAD, 1, "the documented slack");
    for bad in [0_u32, 100_000_u32] {
        let error = resolve_radius(Some(bad), &map).expect_err("outside the range");
        assert_eq!(error.code(), "invalid_argument", "radius {bad} code");
        assert!(
            error.to_string().contains("1..=65536"),
            "the message shows the range: {error}"
        );
    }
    let empty = Map::default();
    let error = resolve_radius(None, &empty).expect_err("nothing to fit");
    assert_eq!(error.code(), "invalid_argument", "empty map code");
    let message = error.to_string();
    assert!(
        message.contains("center") && message.contains("radius"),
        "the message names both fixes: {message}"
    );
}

#[test]
fn minimap_centre_defaults_to_the_map_midpoint() {
    let map = two_rooms();
    assert_eq!(
        resolve_centre(None, &map).expect("the map centre"),
        (256_i64, 128),
        "midpoint of both spans"
    );
    let point = PointParam { x: -100, y: 40 };
    assert_eq!(
        resolve_centre(Some(&point), &map).expect("an explicit centre"),
        (-100_i64, 40),
        "the given centre wins"
    );
    let empty = Map::default();
    assert_eq!(
        resolve_centre(None, &empty)
            .expect_err("nothing to centre on")
            .code(),
        "invalid_argument",
        "empty map code"
    );
}

#[test]
fn the_minimap_request_resolves_every_argument() {
    let map = two_rooms();
    let plain: RenderMinimapParams = serde_json::from_value(json!({})).expect("all optional");
    let request = resolve_minimap(&plain, &map, None).expect("defaults resolve");
    assert_eq!(request.size, 256, "the size default");
    assert_eq!(request.window.centre(), (256, 128), "the map centre");
    assert_eq!(request.window.radius(), 257, "the fitting radius");
    assert_eq!(
        request.colors,
        AutomapColors::DEFAULT,
        "no palette means the defaults"
    );
    assert!(!request.show_things, "thing dots are opt-in");
    assert!(request.player.is_none(), "no arrow by default");

    let palette = fixture_palette();
    let detailed: RenderMinimapParams = serde_json::from_value(json!({
        "size": 64_u64,
        "radius": 32_u64,
        "center": { "x": 64_i64, "y": 128_i64 },
        "player": { "x": 64_i64, "y": 128_i64, "angle": 90_i64 },
        "show_things": true,
    }))
    .expect("the full object");
    let seen = resolve_minimap(&detailed, &map, Some(&palette)).expect("valid arguments");
    assert_eq!(seen.size, 64, "size set");
    assert_eq!(seen.window.centre(), (64, 128), "centre set");
    assert_eq!(seen.window.radius(), 32, "radius set");
    assert_eq!(seen.colors.wall, [176, 79, 176, 255], "palette colours");
    assert!(seen.show_things, "dots on");
    assert_eq!(
        seen.player,
        Some(PlayerMark {
            x: 64,
            y: 128,
            angle: 90
        }),
        "the arrow carries position and facing"
    );

    let declined: RenderMinimapParams =
        serde_json::from_value(json!({ "palette_colors": false })).expect("one field");
    assert_eq!(
        resolve_minimap(&declined, &map, Some(&palette))
            .expect("valid argument")
            .colors,
        AutomapColors::DEFAULT,
        "the palette is declined"
    );
    let bad: RenderMinimapParams =
        serde_json::from_value(json!({ "size": 8_u64 })).expect("one field");
    assert_eq!(
        resolve_minimap(&bad, &map, None)
            .expect_err("below the floor")
            .code(),
        "invalid_argument",
        "tiny size code"
    );
}

#[test]
fn painting_the_minimap_lands_automap_colours_on_black() {
    let map = two_rooms();
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let args: RenderMinimapParams = serde_json::from_value(json!({
        "size": 256_u64,
        "palette_colors": false,
        "show_things": true,
        "player": { "x": 256_i64, "y": 128_i64, "angle": 0_i64 },
    }))
    .expect("the picture arguments");
    let request = resolve_minimap(&args, &map, None).expect("valid arguments");
    let canvas = paint_minimap(&map, &defs, &request);
    assert_eq!(
        (canvas.width(), canvas.height()),
        (256_u32, 256),
        "the square output"
    );
    assert_eq!(
        canvas.pixel(0, 128),
        Some([252, 0, 0, 255]),
        "the west wall"
    );
    assert_eq!(
        canvas.pixel(128, 120),
        Some([188, 120, 72, 255]),
        "the step line brown"
    );
    assert_eq!(
        canvas.pixel(128, 128),
        Some([255, 255, 255, 255]),
        "the arrow at its position"
    );
    assert_eq!(
        canvas.pixel(144, 128),
        Some([255, 255, 255, 255]),
        "the arrow tip sixteen pixels on"
    );
    assert_eq!(
        canvas.pixel(32, 128),
        Some([64, 224, 64, 255]),
        "the player-start dot"
    );
    assert_eq!(canvas.pixel(5, 5), Some([0, 0, 0, 255]), "black ground");
}
