//! The painted overlays: the underlay blend under the map, the grid
//! colours and frame, the sector labels and annotations, and the resolved
//! request carrying them.

use serde_json::json;

use crate::defs::definitions_for;
use crate::render::{Canvas, Pt, RenderSettings, Rgba, Switch};
use crate::wad::game::Game;

use crate::tools::render::frame::{View, bounds_plane, fit};
use crate::tools::render::marks::LABEL_COLOUR;
use crate::tools::render::paint::{
    GRID_MAJOR, GRID_MINOR, GridLine, Scene, grid_frame, paint_grid_line,
};
use crate::tools::render::params::{Annotation, Bounds, Labels, RenderTopdownParams};
use crate::tools::render::resolve::resolve_request;
use crate::tools::render::topdown::paint_map;

use super::overlays::test_underlay;
use super::{request_for, rgb, two_rooms};

/// The pixels of `canvas` within `radius` of `centre`, off-canvas
/// coordinates dropped.
fn sample_window(canvas: &Canvas, centre: Pt, radius: i64) -> Vec<Rgba> {
    let rows = (centre.1.saturating_sub(radius)..=centre.1.saturating_add(radius))
        .filter_map(|row| u32::try_from(row).ok());
    let columns = (centre.0.saturating_sub(radius)..=centre.0.saturating_add(radius))
        .filter_map(|col| u32::try_from(col).ok());
    rows.flat_map(move |deep| columns.clone().map(move |col| (col, deep)))
        .filter_map(|(column, deep)| canvas.pixel(column, deep))
        .collect()
}

#[test]
fn the_underlay_blends_under_the_map_at_its_opacity() {
    let map = two_rooms();
    let mut request = request_for(&map);
    request.width = 200;
    request.height = 160;
    request.bounds = Bounds {
        min_x: -32,
        min_y: -352,
        max_x: 432,
        max_y: 32,
    };
    request.underlay = Some(test_underlay(100));
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let canvas = paint_map(&map, &defs, &request).canvas;
    let view = fit(&bounds_plane(&request.bounds), 200, 160);
    let at = |point: Pt| {
        canvas.pixel(
            u32::try_from(point.0).expect("a fitted column"),
            u32::try_from(point.1).expect("a fitted row"),
        )
    };
    assert_eq!(
        at(view.point(100, -100)),
        Some(rgb(255, 0, 0)),
        "the red left half stands behind the map"
    );
    assert_eq!(
        at(view.point(300, -100)),
        Some(rgb(0, 0, 255)),
        "the blue right half too"
    );
    assert_eq!(
        at(view.point(-24, -100)),
        Some([0, 0, 0, 255]),
        "west of the picture the background stays"
    );
    request.underlay = Some(test_underlay(0));
    let clear = paint_map(&map, &defs, &request).canvas;
    let px = view.point(100, -100);
    assert_eq!(
        clear.pixel(
            u32::try_from(px.0).expect("a fitted column"),
            u32::try_from(px.1).expect("a fitted row"),
        ),
        Some([0, 0, 0, 255]),
        "zero opacity draws no underlay"
    );
}

#[test]
fn grid_lines_pick_their_colour_across_the_frame() {
    let map = two_rooms();
    let request = request_for(&map);
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let mut scene = Scene {
        map: &map,
        defs: &defs,
        view: fit(&bounds_plane(&request.bounds), 200, 120),
        canvas: Canvas::new(200, 120, [0_u8, 0, 0, 255]),
        notes: Vec::new(),
    };
    paint_grid_line(
        &mut scene,
        &GridLine {
            at: 64,
            vertical: true,
            low: -32,
            high: 288,
        },
        1_i64,
    );
    paint_grid_line(
        &mut scene,
        &GridLine {
            at: 0,
            vertical: true,
            low: -32,
            high: 288,
        },
        8_i64,
    );
    assert_eq!(
        scene.canvas.pixel(47, 60),
        Some(GRID_MINOR),
        "the k=1 line at x=64 is minor grey"
    );
    assert_eq!(
        scene.canvas.pixel(30, 60),
        Some(GRID_MAJOR),
        "the k=8 line at x=0 is major grey"
    );
    assert_eq!(
        scene.canvas.pixel(38, 60),
        Some([0, 0, 0, 255]),
        "between the lines stays background"
    );
}

#[test]
fn the_grid_frame_is_the_unprojected_canvas_rectangle() {
    let map = two_rooms();
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    assert_eq!(
        grid_frame(&Scene {
            map: &map,
            defs: &defs,
            view: View {
                num: 128,
                den: 384,
                offset_x: 16,
                offset_y: 80,
            },
            canvas: Canvas::new(200, 160, [0_u8, 0, 0, 255]),
            notes: Vec::new(),
        }),
        (-48_i64, 240, 549, -237),
        "the canvas corners unproject to the covered world rectangle"
    );
}

#[test]
fn sector_labels_and_annotations_paint_their_text() {
    let map = two_rooms();
    let mut request = request_for(&map);
    request.show_things = Switch::new(false);
    request.labels = Labels {
        sectors: Switch::new(true),
        ..Labels::default()
    };
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    let canvas = paint_map(&map, &defs, &request).canvas;
    let hits = (55..76_u32)
        .flat_map(|row| (58..73_u32).map(move |col| (col, row)))
        .filter(|&(col, row)| canvas.pixel(col, row) == Some(LABEL_COLOUR))
        .count();
    assert!(
        hits > 4,
        "the hall's number sits at its anchor (65, 60): {hits} label pixels"
    );
    let far = (4..14_u32)
        .flat_map(|row| (4..14_u32).map(move |col| (col, row)))
        .filter(|&(col, row)| canvas.pixel(col, row) == Some(LABEL_COLOUR))
        .count();
    assert_eq!(far, 0, "no label colour in the far margin");

    let mut noted = request_for(&map);
    noted.show_things = Switch::new(false);
    noted.background = [10, 10, 10, 255];
    noted.annotations = vec![Annotation {
        x: -64,
        y: -64,
        text: "GO".to_owned(),
        colour: rgb(255, 255, 255),
    }];
    let noted_canvas = paint_map(&map, &defs, &noted).canvas;
    let view = fit(&bounds_plane(&noted.bounds), 200, 120);
    let colours = sample_window(&noted_canvas, view.point(-64, -64), 9);
    assert!(
        colours.contains(&rgb(255, 255, 255)),
        "the annotation draws its white text centred on the point"
    );
    assert!(
        colours.contains(&[0, 0, 0, 255]),
        "and its black outline against the lifted background"
    );
}

#[test]
fn resolve_request_carries_the_overlays_and_the_empty_map_gains_a_frame() {
    let map = two_rooms();
    let settings = RenderSettings::default();
    let args: RenderTopdownParams = serde_json::from_value(json!({
        "width": 200_u64,
        "height": 160_u64,
        "grid": 64_i64,
        "labels": {"sectors": true},
        "annotations": [{"x": 0_i64, "y": 0_i64, "text": "a"}],
    }))
    .expect("the overlay call");
    let request =
        resolve_request(&settings, &args, &map, Some(test_underlay(50))).expect("overlays resolve");
    assert_eq!(request.grid, Some(64_i64), "the grid carries");
    assert!(
        request.labels.sectors.is_on() && !request.labels.vertices.is_on(),
        "only the asked-for labels"
    );
    assert_eq!(
        request.annotations.first().expect("one note").text,
        "A",
        "the text arrives upper-cased"
    );
    assert_eq!(
        request
            .underlay
            .as_ref()
            .expect("the underlay carries through")
            .opacity,
        50_u8,
        "the default opacity"
    );
    assert_eq!(request.bounds.min_y, -352, "the underlay joined the frame");
    assert_eq!(request.label_scale, 1_u32, "the narrow default");

    let iso: RenderTopdownParams =
        serde_json::from_value(json!({"projection": "isometric", "grid": 64_i64}))
            .expect("one field");
    let error =
        resolve_request(&settings, &iso, &map, None).expect_err("isometric refuses the grid");
    assert!(
        error
            .to_string()
            .contains("overlays need the topdown projection"),
        "the message names the projection: {error}"
    );
}
