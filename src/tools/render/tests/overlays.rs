//! The overlay parameters: the underlay transform and rectangle, the
//! bounds union, the isometric refusal, the grid, label and annotation
//! ranges, the opacity blend and the sector-label anchor search.

use serde_json::json;

use crate::gfx::image::RgbaImage;
use crate::plan::Units;
use crate::render::canvas::intersect_x;
use crate::render::{Edge, Pt};
use crate::wad::map::Map;

use crate::tools::plan::params::UnitsParam;
use crate::tools::render::marks::sector_label_point;
use crate::tools::render::paint::scale_alpha;
use crate::tools::render::params::{
    AnnotationParams, BoundsParams, LabelsParams, Projection, RenderTopdownParams, Underlay,
};
use crate::tools::render::resolve::{
    check_overlays, resolve_annotations, resolve_bounds, resolve_grid, resolve_label_scale,
    resolve_labels, resolve_opacity, to_overlay_units, underlay_rect,
};

use super::{rgb, two_rooms};

/// An `underlay.units` wire with `scale_num` `8` and the `y` mirror.
fn sketch_units() -> UnitsParam {
    UnitsParam {
        scale_num: 8_i64,
        flip_y: true,
        ..UnitsParam::default()
    }
}

/// A `50x40` opaque picture: the left half red, the right half blue.
fn half_half_image() -> RgbaImage {
    let mut row = Vec::new();
    for _col in 0..25_u32 {
        row.extend_from_slice(&[255_u8, 0, 0, 255]);
    }
    for _col in 0..25_u32 {
        row.extend_from_slice(&[0, 0, 255, 255]);
    }
    let mut pixels = Vec::new();
    for _row in 0..40_u32 {
        pixels.extend_from_slice(&row);
    }
    RgbaImage::new(50, 40, pixels).expect("the buffer matches the shape")
}

/// The placed `50x40` underlay at `opacity` percent.
pub(super) fn test_underlay(opacity: u8) -> Underlay {
    Underlay {
        image: half_half_image(),
        units: to_overlay_units(Some(sketch_units())),
        opacity,
        rect: (0_i64, -320_i64, 400_i64, 0_i64),
    }
}

#[test]
fn the_overlay_units_transform_is_unsnapped_and_the_rect_frames_the_image() {
    let units = to_overlay_units(Some(sketch_units()));
    assert_eq!(units.grid, 1_i64, "picture sampling never snaps");
    let image = half_half_image();
    assert_eq!(
        underlay_rect(&units, &image).expect("the corners fit"),
        (0_i64, -320, 400, 0),
        "8 units per pixel with the mirror runs south"
    );
    assert_eq!(
        underlay_rect(&Units::default(), &image).expect("identity fits"),
        (0_i64, 0, 50, 40),
        "the identity maps the raw pixel box"
    );
    let wild = Units {
        scale_num: i64::MAX,
        ..Units::default()
    };
    assert_eq!(
        underlay_rect(&wild, &image)
            .expect_err("a corner leaves the range")
            .code(),
        "invalid_argument",
        "an overflowing rectangle is refused"
    );
}

#[test]
fn bounds_union_the_map_with_the_underlay_and_an_underlay_frames_an_empty_map() {
    let map = two_rooms();
    let rect = (0_i64, -320, 400, 0);
    let unioned = resolve_bounds(None, &map, Some(rect)).expect("the union frames");
    assert_eq!(
        (unioned.min_x, unioned.max_x, unioned.min_y, unioned.max_y),
        (-32_i64, 544, -352, 288),
        "the map reaches east, the picture reaches south"
    );
    let empty = Map::default();
    let picture = resolve_bounds(None, &empty, Some(rect)).expect("an underlay frames");
    assert_eq!(
        (picture.min_x, picture.min_y, picture.max_x, picture.max_y),
        (-32_i64, -352, 432, 32),
        "the padded picture rectangle alone is enough"
    );
    let chosen = BoundsParams {
        min_x: 0,
        min_y: 0,
        max_x: 100,
        max_y: 100,
    };
    let explicit = resolve_bounds(Some(&chosen), &empty, Some(rect)).expect("bounds win");
    assert_eq!(
        (explicit.min_x, explicit.max_y),
        (0_i64, 100),
        "an explicit box is not unioned"
    );
}

#[test]
fn overlays_are_refused_for_the_isometric_projection_only() {
    for value in [
        json!({"grid": 64_i64}),
        json!({"underlay": {"source": "/tmp/sketch.png"}}),
        json!({"labels": {"sectors": true}}),
        json!({"annotations": [{"x": 0_i64, "y": 0_i64, "text": "A"}]}),
        json!({"label_scale": 2_u64}),
    ] {
        let args: RenderTopdownParams = serde_json::from_value(value.clone()).expect("overlay");
        check_overlays(&args, Projection::Topdown).expect("topdown takes every overlay");
        let error = check_overlays(&args, Projection::Isometric)
            .expect_err("isometric refuses the overlay");
        assert_eq!(error.code(), "invalid_argument", "overlay isometric code");
        assert!(
            error
                .to_string()
                .contains("overlays need the topdown projection"),
            "the message names the projection: {error}"
        );
    }
    let bare: RenderTopdownParams =
        serde_json::from_value(json!({"annotations": [], "labels": null})).expect("empties");
    check_overlays(&bare, Projection::Isometric).expect("nothing asked for nothing");
}

#[test]
fn grid_spacing_and_label_scale_check_their_ranges() {
    assert_eq!(
        resolve_grid(None).expect("absent draws no grid"),
        None,
        "absent"
    );
    assert_eq!(
        resolve_grid(Some(8)).expect("the floor"),
        Some(8_i64),
        "the floor passes"
    );
    assert_eq!(
        resolve_grid(Some(1_024)).expect("the ceiling"),
        Some(1_024),
        "the ceiling passes"
    );
    for bad in [7_i64, 2_048_i64] {
        let error = resolve_grid(Some(bad)).expect_err("outside the range");
        assert_eq!(error.code(), "invalid_argument", "grid {bad} code");
        assert!(
            error.to_string().contains("8..=1024"),
            "the message shows the range: {error}"
        );
    }
    assert_eq!(
        resolve_label_scale(None, 640).expect("a narrow default"),
        1_u32,
        "narrow output labels at one"
    );
    assert_eq!(
        resolve_label_scale(None, 1_024).expect("a wide default"),
        2_u32,
        "wide output labels at two"
    );
    assert_eq!(
        resolve_label_scale(Some(4), 100).expect("an explicit scale"),
        4_u32,
        "the ceiling passes"
    );
    for bad in [0_u32, 5_u32] {
        let error = resolve_label_scale(Some(bad), 640).expect_err("outside the range");
        assert_eq!(error.code(), "invalid_argument", "scale {bad} code");
        assert!(
            error.to_string().contains("1..=4"),
            "the message shows the range: {error}"
        );
    }
    assert_eq!(
        resolve_label_scale(Some(2), 4_096).expect("an explicit wide scale"),
        2_u32,
        "an explicit scale beats the default"
    );
}

#[test]
fn opacity_defaults_to_half_and_refuses_the_over_hundred() {
    assert_eq!(resolve_opacity(None).expect("the default"), 50_u8, "half");
    assert_eq!(
        resolve_opacity(Some(0)).expect("clear"),
        0_u8,
        "clear passes"
    );
    assert_eq!(
        resolve_opacity(Some(100)).expect("solid"),
        100_u8,
        "solid passes"
    );
    let error = resolve_opacity(Some(101)).expect_err("over the top");
    assert_eq!(error.code(), "invalid_argument", "opacity code");
    assert!(
        error.to_string().contains("0..=100"),
        "the message shows the range: {error}"
    );
}

#[test]
fn annotations_uppercase_and_both_limits_and_bad_colours_are_refused() {
    let rows: Vec<AnnotationParams> = serde_json::from_value(json!([
        {"x": 8_i64, "y": -8_i64, "text": " armoury "},
        {"x": 0_i64, "y": 0_i64, "text": "X", "color": "#102030"},
    ]))
    .expect("two rows");
    let notes = resolve_annotations(Some(&rows)).expect("valid rows");
    let first = notes.first().expect("two annotations");
    assert_eq!(first.text, " ARMOURY ", "the text upper-cases in place");
    assert_eq!(
        first.colour,
        rgb(255, 255, 255),
        "the default colour is white"
    );
    let second = notes.get(1).expect("two annotations");
    assert_eq!(second.colour, rgb(16, 32, 48), "the given colour");
    assert!(
        resolve_annotations(None).expect("absent").is_empty(),
        "none asked"
    );

    let long_text = "X".repeat(33_usize);
    let too_long: Vec<AnnotationParams> =
        serde_json::from_value(json!([{"x": 0_i64, "y": 0_i64, "text": long_text}]))
            .expect("one row");
    let length = resolve_annotations(Some(&too_long)).expect_err("33 characters");
    assert_eq!(length.code(), "invalid_argument", "long text code");
    assert!(
        length.to_string().contains("32"),
        "the message names the limit: {length}"
    );
    let many: Vec<AnnotationParams> = (0_u32..65_u32)
        .map(|index| AnnotationParams {
            x: 0,
            y: 0,
            text: format!("N{index}"),
            color: None,
        })
        .collect();
    let count = resolve_annotations(Some(&many)).expect_err("65 rows");
    assert_eq!(count.code(), "invalid_argument", "row count code");
    assert!(
        count.to_string().contains("64"),
        "the message names the limit: {count}"
    );
    let bad_colour: Vec<AnnotationParams> = serde_json::from_value(json!(
        [{"x": 0_i64, "y": 0_i64, "text": "A", "color": "green"}]
    ))
    .expect("one row");
    assert_eq!(
        resolve_annotations(Some(&bad_colour))
            .expect_err("not a hex colour")
            .code(),
        "invalid_argument",
        "bad colour code"
    );
}

#[test]
fn labels_default_off_and_switch_individually() {
    assert!(!resolve_labels(None).any(), "absent labels draw nothing");
    let wanted: LabelsParams =
        serde_json::from_value(json!({"sectors": true, "things": true})).expect("two switches");
    let some = resolve_labels(Some(&wanted));
    assert!(
        some.sectors.is_on() && some.things.is_on(),
        "the given switches land"
    );
    assert!(
        !some.linedefs.is_on() && !some.vertices.is_on(),
        "the left-out switches stay off"
    );
    assert!(some.any(), "two switches on is on");
}

#[test]
fn the_underlay_alpha_scales_by_the_opacity_percent() {
    assert_eq!(scale_alpha(255, 50), 128, "half of full alpha rounds up");
    assert_eq!(scale_alpha(255, 0), 0, "nothing stays nothing");
    assert_eq!(scale_alpha(255, 100), 255, "solid stays solid");
    assert_eq!(scale_alpha(100, 25), 25, "a quarter of a hundred");
}

/// The closed ring through `points` as edges.
fn ring(points: &[Pt]) -> Vec<Edge> {
    let tail = points.iter().skip(1).copied();
    let closing = core::iter::once(points.first().copied().expect("a ring has a first point"));
    tail.chain(closing).zip(points.iter().copied()).collect()
}

/// Whether `at` sits inside the even-odd polygon of `edges`.
fn inside_edges(edges: &[Edge], at: Pt) -> bool {
    let crossings = edges
        .iter()
        .filter(|edge| (edge.0.1 > at.1) != (edge.1.1 > at.1))
        .filter(|edge| intersect_x(edge.0, edge.1, at.1) < at.0)
        .count();
    crossings.checked_rem(2_usize) == Some(1_usize)
}

#[test]
fn sector_label_points_land_inside_concave_polygons() {
    let l_shape = ring(&[(0, 0), (60, 0), (60, 20), (20, 20), (20, 60), (0, 60)]);
    let at = sector_label_point(&l_shape).expect("an L has an anchor");
    assert_eq!(at, (10_i64, 30), "the middle row's only span, centred");
    assert!(inside_edges(&l_shape, at), "the L anchor is inside");
    let u_shape = ring(&[
        (0, 0),
        (60, 0),
        (60, 60),
        (40, 60),
        (40, 20),
        (20, 20),
        (20, 60),
        (0, 60),
    ]);
    let anchor = sector_label_point(&u_shape).expect("a U has an anchor");
    assert!(
        inside_edges(&u_shape, anchor),
        "the U anchor {anchor:?} is inside"
    );
    assert_eq!(
        sector_label_point(&[((0, 0), (0, 10))]).expect("a fallback exists"),
        (0_i64, 0),
        "no inside interval falls back to the first vertex"
    );
    assert_eq!(sector_label_point(&[]), None, "nothing to anchor on");
}
