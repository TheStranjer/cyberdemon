//! `render_topdown`' resolution core: the fitted [`View`], the projection,
//! bounds and highlight parsing, the element colour rules and the painted
//! two-room fixture.

use serde_json::json;

use crate::defs::{Category, definitions_for};
use crate::render::{
    Canvas, RenderSettings, Rgba, Switch, TopdownStyle, category_color, scale_down, thing_visual,
};
use crate::wad::game::Game;
use crate::wad::map::geometry::SectorIndex;
use crate::wad::map::{Linedef, Map};

use crate::tools::render::frame::{Plane, bounds_plane, fit, height_span, iso_plane};
use crate::tools::render::paint::{Scene, editor_line_color, light_grey};
use crate::tools::render::params::{
    BoundsParams, Call, HighlightParams, Projection, RenderTopdownParams,
};
use crate::tools::render::resolve::{
    check_overlays, parse_projection, resolve_bounds, resolve_highlight, resolve_request,
};
use crate::tools::render::topdown::paint_map;

use super::{edge, pixel_of, request_for, rgb, two_rooms};

/// The automap colour of `map`'s linedef at `index`, with `with`
/// standing in for it when given.
fn automap_colour(map: &Map, index: usize, with: Option<&Linedef>) -> Option<Rgba> {
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let bounds = resolve_bounds(None, map, None).expect("the fixture map bounds itself");
    let view = fit(&bounds_plane(&bounds), 64, 64);
    let scene = Scene {
        map,
        geometry: SectorIndex::new(map),
        defs: &defs,
        view,
        canvas: Canvas::new(8, 8, [0_u8, 0, 0, 255]),
        notes: Vec::new(),
    };
    let line = match with {
        Some(replacement) => replacement,
        None => map.linedefs.get(index)?,
    };
    scene.automap_line_color(line)
}

#[test]
fn fit_picks_the_constraining_axis_and_centres_the_view() {
    let plane = Plane {
        min_u: -32,
        max_u: 544,
        min_v: -32,
        max_v: 288,
    };
    let view = fit(&plane, 200, 120);
    assert_eq!((view.num, view.den), (88, 320), "the height fits exactly");
    assert_eq!(
        (view.offset_x, view.offset_y),
        (30, 95),
        "centred horizontally, the full height vertically"
    );
    assert_eq!(view.point(256, 128), (100, 60), "the step line midpoint");
    assert_eq!(view.point(64, 128), (47, 60), "the player start");
    assert_eq!(view.point(-32, -32), (21, 104), "the south-west corner");
}

#[test]
fn fit_centres_a_square_plane_and_holds_the_margins() {
    let plane = Plane {
        min_u: -256,
        max_u: 256,
        min_v: -256,
        max_v: 256,
    };
    let view = fit(&plane, 200, 200);
    assert_eq!(view.point(0, 0), (100, 100), "the origin centres");
    let west = view.point(-256, 0).0;
    let east = view.point(256, 0).0;
    assert!(
        west >= 16_i64 && east <= 184_i64,
        "the 16-pixel margins hold: {west}..{east}"
    );
}

#[test]
fn scale_down_floors_towards_negative_infinity() {
    assert_eq!(scale_down(-32, 88, 320), -9, "-8.8 floors to -9");
    assert_eq!(scale_down(31, 88, 320), 8, "8.52 floors to 8");
    assert_eq!(scale_down(0, 88, 320), 0, "zero stays zero");
}

#[test]
fn projection_parses_its_two_names_case_insensitively() {
    assert_eq!(
        parse_projection(None).expect("absent is top-down"),
        Projection::Topdown
    );
    assert_eq!(
        parse_projection(Some("IsoMetric"))
            .expect("a known name")
            .as_str(),
        "isometric"
    );
    assert_eq!(Projection::Topdown.as_str(), "topdown", "the report name");
    let error = parse_projection(Some("birdseye")).expect_err("not a projection");
    assert_eq!(error.code(), "invalid_argument", "unknown projection code");
    let message = error.to_string();
    assert!(
        message.contains("\"topdown\"") && message.contains("\"isometric\""),
        "the message lists both names: {message}"
    );
}

#[test]
fn bounds_default_pads_and_explicit_refuses_flattened_rectangles() {
    let map = two_rooms();
    let padded = resolve_bounds(None, &map, None).expect("the default frames the map");
    assert_eq!(
        (padded.min_x, padded.max_x, padded.min_y, padded.max_y),
        (-32_i64, 544, -32, 288),
        "32 units of slack on every side"
    );
    let flat = BoundsParams {
        min_x: 0,
        min_y: 0,
        max_x: 0,
        max_y: 100,
    };
    let error = resolve_bounds(Some(&flat), &map, None).expect_err("min_x == max_x");
    assert_eq!(error.code(), "invalid_argument", "flat bounds code");
    assert!(
        error.to_string().contains("min_x < max_x"),
        "the message states the rule: {error}"
    );
    let empty = Map::default();
    let missing = resolve_bounds(None, &empty, None).expect_err("nothing to frame");
    assert_eq!(missing.code(), "invalid_argument", "empty map code");
    assert!(
        missing.to_string().contains("bounds"),
        "the message names the fix: {missing}"
    );
}

#[test]
fn highlight_indices_must_exist_and_the_refusal_names_them() {
    let map = two_rooms();
    let good = HighlightParams {
        linedefs: Some(vec![2_usize]),
        sectors: Some(vec![1_usize]),
        things: Some(vec![0_usize]),
        vertices: Some(vec![5_usize]),
    };
    assert!(
        resolve_highlight(Some(&good), &map).is_ok(),
        "in-range indices pass"
    );
    for (field, kind, value) in [
        ("linedefs", "linedef", 7_usize),
        ("sectors", "sector", 2_usize),
        ("things", "thing", 2_usize),
        ("vertices", "vertex", 6_usize),
    ] {
        let mut object = serde_json::Map::new();
        object.insert(field.to_owned(), json!([value]));
        let params: HighlightParams = serde_json::from_value(json!(object)).expect("one list");
        let error =
            resolve_highlight(Some(&params), &map).expect_err("an index past the end of the table");
        assert_eq!(error.code(), "not_found", "{field} code");
        let message = error.to_string();
        assert!(
            message.contains("highlight on map")
                && message.contains(kind)
                && message.contains(&value.to_string())
                && message.contains("does not exist"),
            "the message names the parameter, element, index and rule: {message}"
        );
    }
}

#[test]
fn categories_pick_the_documented_marker_colours() {
    assert_eq!(
        category_color(Category::parse("Monsters")),
        rgb(224, 64, 64)
    );
    assert_eq!(
        category_color(Category::parse("Player Starts")),
        rgb(64, 224, 64)
    );
    assert_eq!(
        category_color(Category::parse("Weapons")),
        rgb(224, 160, 64)
    );
    assert_eq!(
        category_color(Category::parse("Ammunition")),
        rgb(224, 160, 64)
    );
    assert_eq!(
        category_color(Category::parse("Health And Armour")),
        rgb(64, 160, 224)
    );
    assert_eq!(
        category_color(Category::parse("Powerups")),
        rgb(64, 160, 224)
    );
    assert_eq!(category_color(Category::parse("Keys")), rgb(224, 224, 64));
    assert_eq!(
        category_color(Category::parse("Decorations/Tech")),
        rgb(176, 176, 176)
    );
}

#[test]
fn thing_visuals_use_the_definitions_or_the_fallback() {
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    assert_eq!(thing_visual(&defs, 1), (rgb(64, 224, 64), 16), "player 1");
    assert_eq!(thing_visual(&defs, 3001), (rgb(224, 64, 64), 20), "the imp");
    assert_eq!(
        thing_visual(&defs, 2012),
        (rgb(64, 160, 224), 12),
        "the medikit is Health And Armour"
    );
    assert_eq!(
        thing_visual(&defs, 13),
        (rgb(224, 224, 64), 12),
        "the keycard"
    );
    assert_eq!(
        thing_visual(&defs, 4_000_000),
        (rgb(176, 176, 176), 16),
        "an unknown type falls back to grey and radius 16"
    );
}

#[test]
fn light_greys_are_half_the_level_rounded() {
    assert_eq!(light_grey(160), rgb(80, 80, 80), "the hall");
    assert_eq!(light_grey(192), rgb(96, 96, 96), "the back room");
    assert_eq!(light_grey(255), rgb(128, 128, 128), "255/2 rounds to 128");
    assert_eq!(light_grey(-10), rgb(0, 0, 0), "a negative light clamps");
}

#[test]
fn editor_colours_split_special_two_sided_and_plain_lines() {
    assert_eq!(
        editor_line_color(&edge(0, 1, 0, None)),
        rgb(192, 192, 192),
        "one-sided"
    );
    assert_eq!(
        editor_line_color(&edge(0, 1, 0, Some(1))),
        rgb(112, 112, 112),
        "two-sided"
    );
    let mut special = edge(0, 1, 0, None);
    special.special = 1_i32;
    assert_eq!(editor_line_color(&special), rgb(106, 140, 255));
}

#[test]
fn automap_colours_follow_the_vanilla_line_rules() {
    let map = two_rooms();
    assert_eq!(
        automap_colour(&map, 0, None),
        Some(rgb(252, 0, 0)),
        "outer wall"
    );
    assert_eq!(
        automap_colour(&map, 2, None),
        Some(rgb(188, 120, 72)),
        "the step line changes floor height"
    );
    let mut drawn = map.linedefs.first().expect("line 0 exists").clone();
    drawn.flags = 129_u32;
    assert_eq!(
        automap_colour(&map, 0, Some(&drawn)),
        None,
        "dontdraw lines are skipped"
    );
    let mut secret = map.linedefs.get(2).expect("line 2 exists").clone();
    secret.flags = 4_u32 | 32_u32;
    assert_eq!(
        automap_colour(&map, 2, Some(&secret)),
        Some(rgb(252, 0, 0)),
        "a secret line reads as one-sided"
    );
}

#[test]
fn iso_planes_span_the_projected_bounds_and_heights() {
    let map = two_rooms();
    assert_eq!(height_span(&map), (0_i64, 128), "floor 0, ceiling 128");
    let bounds = resolve_bounds(None, &map, None).expect("the default bounds");
    let plane = iso_plane(&bounds, height_span(&map));
    assert_eq!(plane.min_u, -320, "west corner minus north");
    assert_eq!(plane.max_u, 576, "east corner minus south");
    assert_eq!(plane.min_v, -160, "south-west half-sum minus the top");
    assert_eq!(plane.max_v, 416, "north-east half-sum minus the floor");
    assert_eq!(
        height_span(&Map::default()),
        (0_i64, 0),
        "a sector-less map spans nothing"
    );
}

#[test]
fn painting_the_fixture_lands_lines_and_things_where_the_transform_says() {
    let map = two_rooms();
    let request = request_for(&map);
    assert_eq!(
        pixel_of(&map, &request, 100, 60),
        Some(rgb(112, 112, 112)),
        "the two-sided step line at its midpoint"
    );
    assert_eq!(
        pixel_of(&map, &request, 47, 60),
        Some(rgb(64, 224, 64)),
        "the player start centre"
    );
    assert_eq!(
        pixel_of(&map, &request, 30, 60),
        Some(rgb(192, 192, 192)),
        "the one-sided west wall"
    );
}

#[test]
fn painting_with_fills_and_highlights_tints_and_thickens() {
    let map = two_rooms();
    let mut filled = request_for(&map);
    filled.show_things = Switch::new(false);
    filled.fill_sectors = Switch::new(true);
    assert_eq!(
        pixel_of(&map, &filled, 47, 40),
        Some(rgb(80, 80, 80)),
        "the hall fills with half its light"
    );
    assert_eq!(
        pixel_of(&map, &filled, 140, 40),
        Some(rgb(96, 96, 96)),
        "the back room is brighter"
    );
    let mut marked = request_for(&map);
    marked.show_things = Switch::new(false);
    marked.highlight.linedefs.push(2_usize);
    assert_eq!(
        pixel_of(&map, &marked, 99, 60),
        Some(rgb(255, 64, 64)),
        "the highlight runs 3 pixels wide"
    );
    assert_eq!(
        pixel_of(&map, &marked, 100, 60),
        Some(rgb(255, 64, 64)),
        "including the line centre"
    );
}

#[test]
fn the_isometric_projection_paints_a_wireframe_and_reports_no_transform() {
    let map = two_rooms();
    let mut request = request_for(&map);
    request.projection = Projection::Isometric;
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let painted = paint_map(&map, &defs, &request);
    let canvas = painted.canvas;
    let view = painted.view;
    assert!(view.is_none(), "isometric reports no transform");
    let blank = [0_u8, 0, 0, 255];
    let marks: usize = (0..canvas.height())
        .flat_map(|row| (0..canvas.width()).map(move |col| (col, row)))
        .filter(|&(col, row)| canvas.pixel(col, row) != Some(blank))
        .count();
    assert!(marks > 0, "the wireframe drew {marks} pixels");
}

/// The [`Call`] [`topdown_report`] would hand [`resolve_request`]: the
/// arguments with their projection parsed and their overlays checked,
/// exactly as the run does once before any fetch.
fn checked(args: &RenderTopdownParams) -> Call<'_> {
    let projection = parse_projection(args.projection.as_deref()).expect("test projections");
    check_overlays(args, projection).expect("test overlays");
    Call { args, projection }
}

#[test]
fn resolve_request_falls_back_to_the_settings_and_validates_overrides() {
    let map = two_rooms();
    let settings = RenderSettings::default();
    let plain: RenderTopdownParams = serde_json::from_value(json!({})).expect("all optional");
    let request =
        resolve_request(&settings, &checked(&plain), &map, None).expect("defaults resolve");
    assert_eq!(
        (request.width, request.height),
        (640_u32, 400),
        "the size default"
    );
    assert!(request.show_things.is_on(), "the settings enable things");
    assert!(!request.show_vertices.is_on(), "vertices are opt-in");
    assert_eq!(request.style, TopdownStyle::Editor, "the style default");

    let over: RenderTopdownParams = serde_json::from_value(json!({
        "width": 64_u64,
        "height": 64_u64,
        "style": "Automap",
        "show_things": false,
        "show_vertices": true,
        "fill_sectors": true,
    }))
    .expect("the override object");
    let overridden =
        resolve_request(&settings, &checked(&over), &map, None).expect("valid overrides");
    assert_eq!(
        (overridden.width, overridden.height),
        (64_u32, 64),
        "size set"
    );
    assert_eq!(overridden.style, TopdownStyle::Automap, "style set");
    assert!(!overridden.show_things.is_on(), "things off");
    assert!(overridden.show_vertices.is_on(), "vertices on");
    assert!(overridden.fill_sectors.is_on(), "fills on");

    let bad_size: RenderTopdownParams =
        serde_json::from_value(json!({ "width": 8_u64 })).expect("one field");
    let size_error =
        resolve_request(&settings, &checked(&bad_size), &map, None).expect_err("below the floor");
    assert_eq!(size_error.code(), "invalid_argument", "tiny width code");
    let bad_style: RenderTopdownParams =
        serde_json::from_value(json!({ "style": "wireframe" })).expect("one field");
    let style_error =
        resolve_request(&settings, &checked(&bad_style), &map, None).expect_err("not a style");
    assert_eq!(style_error.code(), "invalid_argument", "bad style code");
}
