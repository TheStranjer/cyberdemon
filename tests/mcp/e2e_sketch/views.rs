//! The chain's looking slice: the `render_topdown` overview with the sketch
//! as `underlay` and the sector labels on lands the built door on the drawn
//! wall - the break's own measured centre falls between the two pixels the
//! door's lines stand on, half a wall thickness either side of it, and each of
//! those pixels carries an automap line colour, as many line pixels as the
//! door has linedefs across the band - and the `render_view` frame from the
//! player start's own `x`, `y` and `angle` answers the camera as that start
//! thing, standing in the plan's `start` sector above its floor, with a centre
//! column that is geometry and not ground. The fitted `transform` reader, the
//! chain's own `Units`, the pixel readers, the picture sizes and the line
//! colours live here with these two tests. [`door_feature`] and [`door_lines`]
//! are `pub(super)` because the `save_and_lock` slice reads the same door's
//! linedefs.

use cyberdemon::gfx::image::RgbaImage;
use cyberdemon::gfx::ops::parse_hex_color;
use cyberdemon::plan::Units;
use cyberdemon::render::{AutomapColors, Pt, Rgba, scale_down};
use serde_json::{Value, json};

use crate::e2e_sketch::{
    Client, GRID, MAP, SCALE_NUM, build_the_traced_plan, gap_between, player_start, point_of,
    rooms_with_source_ids, units,
};
use crate::support::{asset_png, call};

/// Overview width: wide enough for the built door band to be ten
/// output pixels across, one column per line.
const OVERVIEW_WIDTH: u32 = 800_u32;

/// Overview height.
const OVERVIEW_HEIGHT: u32 = 600_u32;

/// The plan's default `wall_thickness`, map units: the depth of the
/// band the door is cut through, so its two lines stand half this far
/// either side of the break's snapped centre.
const WALL_THICKNESS: i64 = 16_i64;

/// How far a built line may stand off the pixel its map coordinate
/// lands on: every endpoint is floor-rounded into the picture.
const PIXEL_SLACK: i64 = 1_i64;

/// The automap line colours a black-and-white drawing can never blend
/// into: [`AutomapColors::DEFAULT`]'s wall red, floor-change brown and
/// ceiling-change yellow. Its fourth class, the `other` grey, is the
/// very colour the washed paper reaches under the default 50% blend,
/// so a grey pixel proves nothing and is never counted.
const LINE_CLASSES: [Rgba; 3] = [
    AutomapColors::DEFAULT.wall,
    AutomapColors::DEFAULT.floor_change,
    AutomapColors::DEFAULT.ceiling_change,
];

/// The `transform` a top-down report answers with, read the way the
/// tool documents it: `px = offset_x + x * scale_num / scale_den` and
/// `py = offset_y - y * scale_num / scale_den`, floored.
struct Fit {
    /// Pixel span of the fitted axis.
    num: i64,
    /// World span of the fitted axis.
    den: i64,
    /// Pixel column of world `x = 0`.
    offset_x: i64,
    /// Pixel row of world `y = 0`.
    offset_y: i64,
}

impl Fit {
    /// Reads the fitted mapping out of a `render_topdown` report.
    fn of(report: &Value) -> Self {
        let transform = &report["transform"];
        Self {
            num: transform["scale_num"].as_i64().expect("a scale_num"),
            den: transform["scale_den"].as_i64().expect("a scale_den"),
            offset_x: transform["offset_x"].as_i64().expect("an offset_x"),
            offset_y: transform["offset_y"].as_i64().expect("an offset_y"),
        }
    }

    /// The output pixel of the map point `x`/`y`, through the very
    /// [`scale_down`] the renderer places its own points with.
    fn pixel(&self, x: i64, y: i64) -> Pt {
        (
            self.offset_x
                .saturating_add(scale_down(x, self.num, self.den)),
            self.offset_y
                .saturating_sub(scale_down(y, self.num, self.den)),
        )
    }

    /// `units` map units as output pixels, never narrower than one.
    fn span(&self, units: i64) -> i64 {
        scale_down(units, self.num, self.den).max(1_i64)
    }
}

/// The chain's `units` as the library type: [`units`]' scale and
/// vertical mirror, with the shared [`GRID`] for the snapped readings
/// and grid `1` for the unsnapped underlay placement every
/// `render_topdown` uses. [`break_centre`] and
/// [`assert_underlay_placement`] check their reports against it, so
/// the test's copy of the transform cannot drift from the tools'.
const fn plan_units() -> Units {
    Units {
        scale_num: SCALE_NUM,
        scale_den: 1_i64,
        origin_x: 0_i64,
        origin_y: 0_i64,
        grid: GRID,
        flip_y: true,
    }
}

/// The `A|B` break's centre in map units, read off the trace report's
/// drawing-pixel `center` through [`plan_units`]: unsnapped, the way
/// the underlay is sampled, and snapped to the shared grid, the way
/// the plan's door is placed. The snapped pair is checked against the
/// report's own `map_center`, never assumed.
fn break_centre(gap: &Value) -> (Pt, Pt) {
    let drawn = point_of(&gap["center"]);
    let units = plan_units();
    let unsnapped = units.to_map(drawn.0, drawn.1).expect("the centre maps");
    let snapped = units
        .point(GRID, drawn.0, drawn.1)
        .expect("the centre snaps");
    assert_eq!(
        snapped,
        point_of(&gap["map_center"]),
        "the report's map_center is the shared units over the shared grid"
    );
    (unsnapped, snapped)
}

/// The `render_topdown` request the verification guide draws a
/// sketch-built map with: the sketch itself as `underlay` through the
/// very `units` the plan was built on (no `grid`, an underlay never
/// snaps), the `automap` line colours and the sector indices.
fn topdown_over_sketch(sketch: &str) -> Value {
    json!({
        "map": MAP,
        "width": OVERVIEW_WIDTH,
        "height": OVERVIEW_HEIGHT,
        "style": "automap",
        "labels": { "sectors": true },
        "underlay": { "source": sketch, "units": units() },
    })
}

/// The published asset's `URL` text.
fn asset_url(report: &Value) -> &str {
    report["asset"]["url"].as_str().expect("an asset url")
}

/// The one pixel `image` draws at world-pixel `x`/`y`, `None` off the
/// picture.
fn pixel_at(image: &RgbaImage, x: i64, y: i64) -> Option<Rgba> {
    image.get(u32::try_from(x).ok()?, u32::try_from(y).ok()?)
}

/// Whether `image`'s row `at.1` carries one of the automap line
/// colours within [`PIXEL_SLACK`] columns of `at.0`.
fn a_line_stands(image: &RgbaImage, at: Pt) -> bool {
    let low = at.0.saturating_sub(PIXEL_SLACK);
    let high = at.0.saturating_add(PIXEL_SLACK);
    (low..=high).any(|column| {
        pixel_at(image, column, at.1).is_some_and(|pixel| LINE_CLASSES.contains(&pixel))
    })
}

/// The two output pixels the built door's lines stand on: half the
/// wall thickness either side of the break's snapped centre, the pair
/// of lines the plan's `door` connector is wired on.
fn door_track_pixels(fit: &Fit, centre: Pt) -> [Pt; 2] {
    let half = WALL_THICKNESS.checked_div(2).expect("a wall above 1");
    [
        fit.pixel(centre.0.saturating_sub(half), centre.1),
        fit.pixel(centre.0.saturating_add(half), centre.1),
    ]
}

/// The line-coloured pixels of the row `at.1` across `reach` columns
/// either side of `at.0`: the built lines standing on the drawn wall
/// band.
fn band_line_pixels(image: &RgbaImage, at: Pt, reach: i64) -> usize {
    let low = at.0.saturating_sub(reach);
    let high = at.0.saturating_add(reach);
    (low..=high)
        .filter(|column| {
            pixel_at(image, *column, at.1).is_some_and(|pixel| LINE_CLASSES.contains(&pixel))
        })
        .count()
}

/// The build report's `door` feature row: the lines and band sectors
/// the plan's locked door was wired onto.
pub(super) fn door_feature(built: &Value) -> &Value {
    built["features"]
        .as_array()
        .expect("a feature list")
        .iter()
        .find(|row| row["kind"] == json!("door"))
        .expect("the plan's blue gate is a door feature")
}

/// The linedef indices the built door stands on.
pub(super) fn door_lines(built: &Value) -> &[Value] {
    door_feature(built)["lines"]
        .as_array()
        .expect("a line list")
        .as_slice()
}

/// The overview's `underlay` row is the sketch where the shared
/// `units` put it: the picture's own size and the map rectangle its
/// four corners land in.
fn assert_underlay_placement(overview: &Value, trace: &Value) {
    let row = &overview["underlay"];
    assert_eq!(row["width"], trace["image"]["width"], "the sketch's width");
    assert_eq!(
        row["height"], trace["image"]["height"],
        "the sketch's height"
    );
    let wide = row["width"].as_i64().expect("a width");
    let high = row["height"].as_i64().expect("a height");
    let units = plan_units();
    let (west, north) = units.to_map(0_i64, 0_i64).expect("the origin maps");
    let (east, south) = units.to_map(wide, high).expect("the far corner maps");
    let bounds = &row["map_bounds"];
    assert_eq!(bounds["min_x"], json!(west), "the underlay's west edge");
    assert_eq!(bounds["max_x"], json!(east), "and its east edge");
    assert_eq!(bounds["min_y"], json!(south), "its south edge");
    assert_eq!(bounds["max_y"], json!(north), "and its north edge");
}

/// The built door lands on the drawn wall: the break's own measured
/// centre falls between the two pixels the door's lines stand on,
/// half a wall thickness either side of it, and each of those pixels
/// carries an automap line colour — as many line pixels as the door
/// has linedefs across the band. The exact centre pixel is the band's
/// own sector label, which is why the reading is the pair of tracks.
fn assert_door_on_the_wall(image: &RgbaImage, overview: &Value, trace: &Value, built: &Value) {
    let rooms = rooms_with_source_ids(trace);
    let gap = gap_between(trace, &rooms, "start", "hall");
    let (measured, snapped) = break_centre(gap);
    let limit = GRID.checked_div(2).expect("a grid above 1");
    let drift = measured.0.saturating_sub(snapped.0).abs();
    assert!(
        drift <= limit,
        "the grid snap moves the centre by at most half a cell: {measured:?} / {snapped:?}"
    );
    let fit = Fit::of(overview);
    let tracks = door_track_pixels(&fit, snapped);
    let centre = fit.pixel(measured.0, measured.1);
    assert!(
        centre >= tracks[0] && centre <= tracks[1],
        "the drawn break's centre {centre:?} lies between the door's lines {tracks:?}"
    );
    for track in tracks {
        assert!(
            a_line_stands(image, track),
            "the door line at {track:?} draws in an automap line colour, not {:?}",
            pixel_at(image, track.0, track.1)
        );
    }
    let drawn = band_line_pixels(
        image,
        centre,
        fit.span(WALL_THICKNESS).saturating_add(PIXEL_SLACK),
    );
    let lines = door_lines(built).len();
    assert!(
        drawn >= lines,
        "the door's {lines} lines land on the drawn wall, {drawn} line pixels were counted"
    );
}

#[tokio::test]
async fn the_topdown_overview_lands_the_built_door_on_the_drawn_wall() {
    let dir = tempfile::tempdir().expect("temp dir");
    let chain = build_the_traced_plan(dir.path()).await;
    let overview = call(
        &chain.client,
        "render_topdown",
        topdown_over_sketch(&chain.sketch),
    )
    .await;
    assert_eq!(overview["map"], json!(MAP), "the built map drew");
    assert_eq!(overview["projection"], json!("topdown"), "the flat fit");
    assert_eq!(
        overview["notes"],
        json!([]),
        "the overview skipped nothing: {}",
        overview["notes"]
    );
    assert_underlay_placement(&overview, &chain.trace);
    let image = asset_png(asset_url(&overview)).await;
    assert_eq!(
        i64::from(image.width),
        overview["width"].as_i64().expect("a width"),
        "the asked width stands"
    );
    assert_eq!(
        i64::from(image.height),
        overview["height"].as_i64().expect("a height"),
        "and the asked height"
    );
    assert_door_on_the_wall(&image, &overview, &chain.trace, &chain.built);
}

/// The `render_view` request the camera at the plan's own start stands
/// on: the thing's reported `x`, `y` and `angle`, which faces down the
/// sketch's west-to-east axis at the blue door.
fn view_from(start: &Value) -> Value {
    json!({
        "map": MAP,
        "x": start["x"],
        "y": start["y"],
        "angle": start["angle"],
    })
}

/// The camera the frame answers with is the start thing itself,
/// standing in the plan's `start` sector with an eye above its floor.
fn assert_camera_is_the_start(view: &Value, start: &Value, built: &Value) {
    let camera = &view["camera"];
    for field in ["x", "y", "angle"] {
        assert_eq!(camera[field], start[field], "the camera echoes the {field}");
    }
    assert_eq!(
        camera["sector"], built["rooms"]["start"][0],
        "the camera stands in the start room: {camera}"
    );
    assert!(
        camera["z"].as_i64().expect("an eye height") > 0_i64,
        "the eye is above the floor it stands on"
    );
}

/// The pixel the session's own render settings fill the ground with.
async fn ground_pixel(client: &Client) -> Rgba {
    let settings = call(client, "get_render_settings", Value::Null).await;
    let text = settings["background"]
        .as_str()
        .expect("a background colour");
    parse_hex_color(text).expect("the settings' own colour parses")
}

/// The pixels of the frame's centre column that are not the ground:
/// straight ahead of the camera stands the blue door.
fn painted_centre_column(image: &RgbaImage, ground: Rgba) -> usize {
    let column = image.width.checked_div(2).expect("a frame above 1 wide");
    (0..image.height)
        .filter(|row| image.get(column, *row) != Some(ground))
        .count()
}

#[tokio::test]
async fn the_view_from_the_player_start_draws_the_door_ahead() {
    let dir = tempfile::tempdir().expect("temp dir");
    let chain = build_the_traced_plan(dir.path()).await;
    let start = player_start(&chain.client).await;
    let view = call(&chain.client, "render_view", view_from(&start)).await;
    assert_eq!(view["map"], json!(MAP), "the built map drew");
    assert_camera_is_the_start(&view, &start, &chain.built);
    let image = asset_png(asset_url(&view)).await;
    let ground = ground_pixel(&chain.client).await;
    let painted = painted_centre_column(&image, ground);
    let height = usize::try_from(image.height).expect("a test frame");
    assert_eq!(
        usize::try_from(view["height"].as_u64().expect("a height")).expect("a test frame"),
        height,
        "the frame is the size the report answers"
    );
    let half = height.checked_div(2).expect("a frame above 1 tall");
    assert!(painted > 0, "the centre column carries something drawn");
    assert!(
        painted >= half,
        "the middle of the frame is geometry, not ground: {painted} of {height} pixels"
    );
    let ahead = (
        image.width.checked_div(2).expect("a frame above 1 wide"),
        image.height.checked_div(2).expect("a frame above 1 tall"),
    );
    assert_ne!(
        image.get(ahead.0, ahead.1).expect("the frame's centre"),
        ground,
        "dead ahead, at the horizon, stands the blue door, not bare ground"
    );
}
