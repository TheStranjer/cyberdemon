//! The drawing topics: the two corner forms of a loop, the request
//! `create_sector_from_loop` builds from them with its theme defaults,
//! the solid paint of a carve and the fields refused before anything is
//! drawn.

use super::{carve_args, code, loop_args, test_map, three_points_json, vanilla_surfaces};
use crate::tools::edit_sectors::draw::{
    build_request, build_void_request, outer_sector_of, polygon_of,
};
use crate::tools::edit_sectors::{DEFAULT_CEILING_HEIGHT, DEFAULT_LIGHT};
use crate::tools::themes::Surfaces;
use crate::wad::classify::MapFormat;
use crate::wad::map::overlay::{Outside, Paint};

#[test]
fn the_loop_needs_exactly_one_corner_form() {
    let map = test_map();
    let points_args = loop_args(&three_points_json(""));
    let points = polygon_of(
        points_args.points.as_ref(),
        points_args.vertices.as_ref(),
        &map,
    )
    .expect("the points form is accepted");
    assert_eq!(points.len(), 3, "three corners stay three");
    assert_eq!(points.first().copied(), Some((0_i64, 0_i64)), "widened");
    let indices =
        polygon_of(None, Some(&vec![0_usize, 1, 2]), &map).expect("the index form is accepted");
    assert_eq!(indices.len(), 3, "three indices stay three");
    assert_eq!(indices.first().copied(), Some((0_i64, 0_i64)), "resolved");
    let both = three_points_json(", \"vertices\": [0, 1, 2]");
    for json in [
        "{}",
        r#"{"points": [], "vertices": [0, 1, 2]}"#,
        both.as_str(),
    ] {
        let args = loop_args(json);
        let error = polygon_of(args.points.as_ref(), args.vertices.as_ref(), &map)
            .expect_err("one form is required");
        assert_eq!(code(&error), "invalid_argument", "form code for {json}");
    }
    let past =
        polygon_of(None, Some(&vec![0_usize, 1, 99]), &map).expect_err("vertex 99 is not there");
    assert_eq!(code(&past), "not_found", "vertex code");
    assert!(
        past.to_string().contains("map has 4 vertices"),
        "the message counts the vertices: {past}"
    );
}

#[test]
fn the_built_request_carries_the_documented_defaults() {
    let map = test_map();
    let args = loop_args(&three_points_json(""));
    let request = build_request(&args, &map, &vanilla_surfaces()).expect("the minimal loop builds");
    assert_eq!(
        request.textures.wall, "STARTAN3",
        "the wall texture default"
    );
    assert_eq!(
        request.textures.upper, "STARTAN3",
        "the faces fall back to the wall"
    );
    assert_eq!(request.textures.lower, "STARTAN3");
    assert_eq!(
        request.outside,
        Outside::Original,
        "auto is the outer default"
    );
    assert!(
        !request.keep_interior,
        "the interior is overwritten by default"
    );
    let Paint::Sector(sector) = request.paint else {
        panic!("a room paints a sector");
    };
    assert_eq!(sector.floor_height, 0_i32);
    assert_eq!(sector.ceiling_height, DEFAULT_CEILING_HEIGHT);
    assert_eq!(
        sector.light, DEFAULT_LIGHT,
        "the sector defaults are add_sector's"
    );
    assert_eq!(sector.floor_flat, "FLOOR4_8");
    let full = loop_args(
        r#"{"points": [{"x": 0, "y": 0}, {"x": 128, "y": 0}, {"x": 64, "y": 128}],
            "ceiling_height": 192, "tag": 7, "wall_texture": "brick11",
            "upper_texture": "-", "lower_texture": "step1", "outer_sector": "none",
            "keep_interior": true}"#,
    );
    let built = build_request(&full, &map, &vanilla_surfaces()).expect("the full loop builds");
    assert_eq!(built.textures.wall, "BRICK11", "names are uppercased");
    assert_eq!(built.textures.upper, "-", "a wall texture may be the dash");
    assert_eq!(built.textures.lower, "STEP1");
    assert_eq!(built.outside, Outside::Solid, "none closes the outer");
    assert!(built.keep_interior, "the flag comes through");
    let Paint::Sector(deep) = built.paint else {
        panic!("a room paints a sector");
    };
    assert_eq!(deep.ceiling_height, 192_i32);
    assert_eq!(deep.tag, 7_i32);
    let indexed = loop_args(r#"{"vertices": [0, 1, 2, 3], "outer_sector": 1}"#);
    let explicit =
        build_request(&indexed, &map, &vanilla_surfaces()).expect("the indexed loop builds");
    assert_eq!(explicit.outside, Outside::Face(1), "an index is taken");
    assert_eq!(
        explicit.polygon,
        vec![(0_i64, 0_i64), (128, 0), (128, 128), (0, 128)],
        "the indices resolve to their vertices' coordinates"
    );
}

#[test]
fn a_themed_request_dresses_the_wall_and_flats_with_the_theme() {
    let map = test_map();
    let args = loop_args(&three_points_json(""));
    let themed = Surfaces {
        wall: "CSTLRCK".to_owned(),
        floor: "FLOOR00".to_owned(),
        ceiling: "FLAT513".to_owned(),
    };
    let dressed = build_request(&args, &map, &themed).expect("a themed loop builds");
    assert_eq!(dressed.textures.wall, "CSTLRCK", "the theme wall dresses");
    assert_eq!(
        dressed.textures.upper, "CSTLRCK",
        "and transitions follow it"
    );
    assert_eq!(dressed.textures.lower, "CSTLRCK");
    let Paint::Sector(room) = dressed.paint else {
        panic!("a room paints a sector");
    };
    assert_eq!(room.floor_flat, "FLOOR00", "the theme flats land");
    assert_eq!(room.ceiling_flat, "FLAT513");
}

#[test]
fn a_void_request_carves_with_the_given_wall_texture() {
    let map = test_map();
    let carve =
        carve_args(r#"{"points": [{"x": 8, "y": 8}, {"x": 64, "y": 8}, {"x": 8, "y": 64}]}"#);
    let request = build_void_request(&carve, &map, "THEME1").expect("the carve builds");
    assert_eq!(request.paint, Paint::Void, "the interior becomes solid");
    assert!(!request.keep_interior, "a carve never keeps the interior");
    assert_eq!(
        request.outside,
        Outside::Original,
        "neighbours stay as they are"
    );
    assert_eq!(request.textures.wall, "THEME1", "the theme wall stands in");
    assert_eq!(request.textures.upper, "THEME1");
    assert_eq!(request.polygon.len(), 3, "the triangle comes through");
}

#[test]
fn bad_loop_fields_are_refused_before_anything_draws() {
    let map = test_map();
    let refusals = [
        three_points_json(", \"light\": 300"),
        three_points_json(", \"floor_height\": 200"),
        three_points_json(", \"ceiling_flat\": \"-\""),
        three_points_json(", \"wall_texture\": \"TOOLONG12\""),
        three_points_json(", \"outer_sector\": \"sideways\""),
        r#"{"points": [{"x": 0, "y": 0}, {"x": 40000, "y": 0}, {"x": 64, "y": 128}]}"#.to_owned(),
    ];
    for json in &refusals {
        let args = loop_args(json);
        let error = build_request(&args, &map, &vanilla_surfaces())
            .expect_err("the field must refuse on a binary map");
        assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
    }
    let mode = outer_sector_of(loop_args(r#"{"outer_sector": "AUTO"}"#).outer_sector)
        .expect("case-insensitive auto");
    assert_eq!(mode, Outside::Original, "the mode name uppercases away");
    let mut udmf = test_map();
    udmf.format = MapFormat::Udmf;
    let wide = loop_args(
        r#"{"points": [{"x": 0, "y": 0}, {"x": 40000, "y": 0}, {"x": 64, "y": 128}],
            "wall_texture": "a_long_udmf_wall_texture", "tag": 70000}"#,
    );
    build_request(&wide, &udmf, &vanilla_surfaces())
        .expect("UDMF takes the wide coordinates, name and tag");
}
