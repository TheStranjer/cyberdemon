//! The `render` tool group as seen by a real MCP client: the session-level
//! render defaults, the validation each setter applies, how long the
//! settings live, `render_topdown`' pictures: the fitted transform,
//! the style colours, sector fills, thing markers, isometric wireframe,
//! highlights and error codes, `render_minimap`' compact automap
//! square: its window fit, palette colours, zoom, player arrow and
//! errors, and `render_view`' first-person frame: camera echo, textured
//! walls, textured planes, the sky texture, the masked-middle and sprite
//! transparent pass, pitch, lighting and error codes. The `build_scene`
//! preparations the 3D view uses are checked here too.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use cyberdemon::error::CyberError;
    use cyberdemon::gfx::codec::{decode_image, encode_png};
    use cyberdemon::gfx::image::RgbaImage;
    use cyberdemon::render::{Lighting, RenderSettings};
    use cyberdemon::wad::map::doom::write_doom_map;
    use cyberdemon::wad::{Lump, LumpName, Wad};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{
        iwad, iwad_heretic, iwad_stock, iwad_view_textures, pwad, pwad_empty_map,
        two_room_textured_map,
    };
    use crate::support::{call, call_err, connect, http_get};

    /// The settings a fresh connection starts on.
    fn defaults() -> Value {
        json!({
            "width": 640_u64,
            "height": 400_u64,
            "lighting": "doom",
            "show_things": true,
            "show_sprites": true,
            "sky": true,
            "fov": 90_u64,
            "gamma": 0_u64,
            "topdown_style": "editor",
            "background": "#000000",
        })
    }

    /// Reads the settings back through `get_render_settings`.
    async fn settings(client: &RunningService<RoleClient, ()>) -> Value {
        call(client, "get_render_settings", Value::Null).await
    }

    /// The `{"path": ...}` argument object for `file`.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    /// Requires an error `message` to mention every one of `names`.
    fn assert_lists(message: &str, names: &[&str]) {
        let missing: Vec<&&str> = names
            .iter()
            .filter(|name| !message.contains(**name))
            .collect();
        assert!(
            missing.is_empty(),
            "the refusal should list all of {names:?}, misses {missing:?}: {message}"
        );
    }

    /// Refuses one option value through `set_render_options` and requires the
    /// message to show the valid `wanted` range or shape.
    async fn refused(client: &RunningService<RoleClient, ()>, args: Value, wanted: &str) {
        let label = args.to_string();
        let (code, message, _) = call_err(client, "set_render_options", args).await;
        assert_eq!(code, "invalid_argument", "{label} is refused");
        assert!(
            message.contains(wanted),
            "the refusal for {label} shows {wanted}: {message}"
        );
    }

    #[tokio::test]
    async fn get_render_settings_reports_the_vanilla_defaults() {
        let client = connect().await;
        assert_eq!(settings(&client).await, defaults(), "the default settings");
    }

    #[tokio::test]
    async fn set_render_resolution_reports_and_keeps_the_size() {
        let client = connect().await;
        let report = call(
            &client,
            "set_render_resolution",
            json!({ "width": 1_024_u64, "height": 768_u64 }),
        )
        .await;
        let mut expected = defaults();
        expected["width"] = json!(1_024_u64);
        expected["height"] = json!(768_u64);
        assert_eq!(report, expected, "the result is the full settings");
        assert_eq!(settings(&client).await, report, "and it persists");
    }

    #[tokio::test]
    async fn the_largest_square_is_exactly_the_pixel_cap() {
        let client = connect().await;
        let report = call(
            &client,
            "set_render_resolution",
            json!({ "width": 4_096_u64, "height": 4_096_u64 }),
        )
        .await;
        assert_eq!(report["width"], json!(4_096_u64), "the widest edge");
        assert_eq!(report["height"], json!(4_096_u64), "the tallest edge");
        let small = call(
            &client,
            "set_render_resolution",
            json!({ "width": 16_u64, "height": 16_u64 }),
        )
        .await;
        assert_eq!(small["width"], json!(16_u64), "the smallest square");
    }

    #[tokio::test]
    async fn a_resolution_outside_the_range_is_an_invalid_argument() {
        let client = connect().await;
        let (code, message, _) = call_err(
            &client,
            "set_render_resolution",
            json!({ "width": 8_u64, "height": 400_u64 }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "an edge below the floor");
        assert!(
            message.contains("width") && message.contains("16..=4096"),
            "the message names the field and the range: {message}"
        );
        let (tall_code, tall_message, _) = call_err(
            &client,
            "set_render_resolution",
            json!({ "width": 640_u64, "height": 4_097_u64 }),
        )
        .await;
        assert_eq!(tall_code, "invalid_argument", "an edge above the ceiling");
        assert!(
            tall_message.contains("height") && tall_message.contains("4097"),
            "the message names the field and the value: {tall_message}"
        );
        assert_eq!(
            settings(&client).await,
            defaults(),
            "refused sizes change nothing"
        );
    }

    #[tokio::test]
    async fn a_resolution_over_the_pixel_cap_is_an_invalid_argument() {
        let client = connect().await;
        let (code, message, _) = call_err(
            &client,
            "set_render_resolution",
            json!({ "width": 4_096_u64, "height": 4_097_u64 }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "one row past the cap");
        assert!(
            message.contains("16777216") && message.contains("lower"),
            "the message names the cap and the fix: {message}"
        );
        assert_eq!(
            settings(&client).await,
            defaults(),
            "the cap refusal changes nothing"
        );
    }

    #[tokio::test]
    async fn set_render_options_changes_only_the_fields_it_is_given() {
        let client = connect().await;
        let report = call(
            &client,
            "set_render_options",
            json!({
                "lighting": "fullbright",
                "gamma": 2_u64,
                "show_things": false,
                "background": "#1a2b3c",
            }),
        )
        .await;
        assert_eq!(report["lighting"], json!("fullbright"), "lighting set");
        assert_eq!(report["gamma"], json!(2_u64), "gamma set");
        assert_eq!(report["show_things"], json!(false), "markers off");
        assert_eq!(report["background"], json!("#1a2b3c"), "colour set");
        assert_eq!(report["show_sprites"], json!(true), "sprites untouched");
        assert_eq!(report["sky"], json!(true), "sky untouched");
        assert_eq!(report["fov"], json!(90_u64), "fov untouched");
        assert_eq!(report["width"], json!(640_u64), "resolution untouched");
        assert_eq!(settings(&client).await, report, "and it persists");
        let second = call(
            &client,
            "set_render_options",
            json!({ "topdown_style": "automap", "sky": false }),
        )
        .await;
        assert_eq!(
            second["topdown_style"],
            json!("automap"),
            "the new option applied"
        );
        assert_eq!(second["sky"], json!(false), "the new switch applied");
        assert_eq!(
            second["lighting"],
            json!("fullbright"),
            "the earlier lighting survives"
        );
        assert_eq!(
            second["background"],
            json!("#1a2b3c"),
            "the earlier colour survives"
        );
    }

    #[tokio::test]
    async fn an_unknown_enum_value_is_refused_listing_the_names() {
        let client = connect().await;
        let (code, message, _) = call_err(
            &client,
            "set_render_options",
            json!({ "lighting": "cinematic" }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "unknown lighting");
        assert_lists(&message, &["\"doom\"", "\"flat\"", "\"fullbright\""]);
        let (style_code, style_message, _) = call_err(
            &client,
            "set_render_options",
            json!({ "topdown_style": "wireframe" }),
        )
        .await;
        assert_eq!(style_code, "invalid_argument", "unknown style");
        assert!(
            style_message.contains("\"editor\"") && style_message.contains("\"automap\""),
            "the style refusal lists both names: {style_message}"
        );
        assert_eq!(
            settings(&client).await,
            defaults(),
            "refused values change nothing"
        );
    }

    #[tokio::test]
    async fn a_number_or_colour_outside_its_range_is_refused() {
        let client = connect().await;
        refused(&client, json!({ "fov": 200_u64 }), "10..=170").await;
        refused(&client, json!({ "fov": 5_u64 }), "10..=170").await;
        refused(&client, json!({ "gamma": 9_u64 }), "0..=4").await;
        refused(&client, json!({ "background": "blue" }), "#RRGGBB").await;
        refused(&client, json!({ "background": "#12345" }), "#RRGGBB").await;
        assert_eq!(
            settings(&client).await,
            defaults(),
            "no refused option landed"
        );
    }

    #[tokio::test]
    async fn one_bad_option_in_a_call_changes_nothing() {
        let client = connect().await;
        let (code, message, _) = call_err(
            &client,
            "set_render_options",
            json!({
                "lighting": "flat",
                "sky": false,
                "gamma": 7_u64,
            }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "the bad gamma fails the call");
        assert!(
            message.contains("0..=4"),
            "the message points at gamma: {message}"
        );
        assert_eq!(
            settings(&client).await,
            defaults(),
            "the valid lighting and sky did not land either"
        );
    }

    #[tokio::test]
    async fn set_render_options_without_any_field_is_an_invalid_argument() {
        let client = connect().await;
        let (code, message, _) = call_err(&client, "set_render_options", json!({})).await;
        assert_eq!(code, "invalid_argument", "an empty call does nothing");
        assert_lists(
            &message,
            &[
                "lighting",
                "show_things",
                "show_sprites",
                "sky",
                "fov",
                "gamma",
                "topdown_style",
                "background",
            ],
        );
    }

    #[tokio::test]
    async fn new_wad_and_open_wad_leave_the_render_settings_alone() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        call(
            &client,
            "set_render_options",
            json!({ "fov": 110_u64, "topdown_style": "automap" }),
        )
        .await;
        let before = settings(&client).await;
        call(&client, "new_wad", json!({})).await;
        assert_eq!(settings(&client).await, before, "new_wad keeps settings");
        let extra = pwad(dir.path());
        call(&client, "open_wad", path_arg(&extra)).await;
        assert_eq!(
            settings(&client).await,
            before,
            "opening an archive keeps settings"
        );
        call(&client, "merge_wad", path_arg(&extra)).await;
        assert_eq!(
            settings(&client).await,
            before,
            "stacking a layer keeps settings"
        );
    }

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// Load the fixture `IWAD` (the two-room `MAP01`) as the base.
    async fn iwad_client(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(&client, "open_iwad", path_arg(&base)).await;
        client
    }

    /// The fitted `transform` of a top-down report.
    struct Transform {
        /// Scale numerator.
        num: i64,
        /// Scale denominator.
        den: i64,
        /// Pixel column of world `x = 0`.
        offset_x: i64,
        /// Pixel row of world `y = 0`.
        offset_y: i64,
    }

    impl Transform {
        /// Read the transform out of a report.
        fn of(report: &Value) -> Self {
            let object = &report["transform"];
            Self {
                num: object["scale_num"].as_i64().expect("scale_num"),
                den: object["scale_den"].as_i64().expect("scale_den"),
                offset_x: object["offset_x"].as_i64().expect("offset_x"),
                offset_y: object["offset_y"].as_i64().expect("offset_y"),
            }
        }

        /// The documented pixel mapping: `px = offset_x + x*num/den`,
        /// `py = offset_y - y*num/den` (non-negative test points).
        fn project(&self, x: i64, y: i64) -> (u32, u32) {
            let column = self.offset_x.saturating_add(
                x.checked_mul(self.num)
                    .expect("test product")
                    .checked_div(self.den)
                    .expect("non-zero scale"),
            );
            let row = self.offset_y.saturating_sub(
                y.checked_mul(self.num)
                    .expect("test product")
                    .checked_div(self.den)
                    .expect("non-zero scale"),
            );
            (
                u32::try_from(column).expect("a fitted column"),
                u32::try_from(row).expect("a fitted row"),
            )
        }
    }

    /// Fetch a published asset's bytes over `HTTP`.
    async fn fetch_asset(url: &str) -> Vec<u8> {
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
        assert_eq!(status, 200, "the asset serves: {url}");
        body
    }

    /// Fetch and decode the overview asset of a report.
    async fn rendered_image(report: &Value) -> RgbaImage {
        let url = report["asset"]["url"].as_str().expect("an asset url");
        decode_image(&fetch_asset(url).await).expect("the overview is a readable PNG")
    }

    /// The number of pixels of `image` that are not the plain black
    /// background the renders start from.
    fn non_background(image: &RgbaImage) -> usize {
        (0..image.height)
            .flat_map(|row| (0..image.width).map(move |col| (col, row)))
            .filter(|&(col, row)| image.get(col, row) != Some([0, 0, 0, 255]))
            .count()
    }

    #[tokio::test]
    async fn render_topdown_reports_a_fitted_transform_and_paints_the_fixture() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "render_topdown",
            json!({ "map": "MAP01", "width": 200_u64, "height": 120_u64 }),
        )
        .await;
        assert_eq!(report["map"], json!("MAP01"), "the rendered map");
        assert_eq!(report["projection"], json!("topdown"), "the projection");
        assert_eq!(report["width"], json!(200_u64), "the width");
        assert_eq!(report["height"], json!(120_u64), "the height");
        let asset = &report["asset"];
        assert_eq!(
            asset["filename"],
            json!("MAP01_topdown.png"),
            "the published name"
        );
        assert_eq!(asset["mime"], json!("image/png"), "the media type");
        assert!(
            asset["size"].as_u64().expect("a size") > 0,
            "the PNG is not empty"
        );
        let image = rendered_image(&report).await;
        assert_eq!((image.width, image.height), (200, 120), "the PNG shape");
        let view = Transform::of(&report);
        let (line_x, line_y) = view.project(256, 128);
        assert_eq!(
            image.get(line_x, line_y).expect("a fitted pixel"),
            [112, 112, 112, 255],
            "the two-sided step line at its midpoint"
        );
        let (player_x, player_y) = view.project(64, 128);
        assert_eq!(
            image.get(player_x, player_y).expect("a fitted pixel"),
            [64, 224, 64, 255],
            "the player start marker"
        );
    }

    #[tokio::test]
    async fn the_automap_style_colours_the_outer_wall_red() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "render_topdown",
            json!({
                "map": "MAP01", "width": 200_u64, "height": 120_u64, "style": "automap",
            }),
        )
        .await;
        let image = rendered_image(&report).await;
        let view = Transform::of(&report);
        let (x, y) = view.project(0, 128);
        assert_eq!(
            image.get(x, y).expect("a fitted pixel"),
            [252, 0, 0, 255],
            "the one-sided west wall in automap red"
        );
    }

    #[tokio::test]
    async fn fill_sectors_paints_each_sector_with_half_its_light() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "render_topdown",
            json!({
                "map": "MAP01", "width": 200_u64, "height": 120_u64,
                "fill_sectors": true, "show_things": false,
            }),
        )
        .await;
        let image = rendered_image(&report).await;
        let view = Transform::of(&report);
        let (hall_x, hall_y) = view.project(64, 200);
        let (room_x, room_y) = view.project(400, 200);
        assert_eq!(
            image.get(hall_x, hall_y).expect("a fitted pixel"),
            [80, 80, 80, 255],
            "the hall at light 160 fills grey 80"
        );
        assert_eq!(
            image.get(room_x, room_y).expect("a fitted pixel"),
            [96, 96, 96, 255],
            "the back room at light 192 fills grey 96"
        );
    }

    #[tokio::test]
    async fn the_isometric_projection_omits_the_transform_and_draws_a_wireframe() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "render_topdown",
            json!({
                "map": "MAP01", "width": 200_u64, "height": 120_u64,
                "projection": "isometric",
            }),
        )
        .await;
        assert_eq!(
            report["projection"],
            json!("isometric"),
            "the projection name"
        );
        assert_eq!(report["transform"], Value::Null, "no transform reported");
        assert_eq!(report["width"], json!(200_u64), "the width");
        assert_eq!(report["height"], json!(120_u64), "the height");
        let image = rendered_image(&report).await;
        assert_eq!((image.width, image.height), (200, 120), "the PNG shape");
        assert!(non_background(&image) > 0, "the wireframe drew");
    }

    #[tokio::test]
    async fn highlighting_a_linedef_draws_it_red_and_three_pixels_wide() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "render_topdown",
            json!({
                "map": "MAP01", "width": 200_u64, "height": 120_u64,
                "show_things": false, "highlight": { "linedefs": [2_u64] },
            }),
        )
        .await;
        let image = rendered_image(&report).await;
        let view = Transform::of(&report);
        let (centre_x, line_y) = view.project(256, 128);
        let beside = centre_x.checked_sub(1_u32).expect("a fitted column");
        assert_eq!(
            image.get(beside, line_y).expect("a fitted pixel"),
            [255, 64, 64, 255],
            "highlight red left of the line centre"
        );
        assert_eq!(
            image.get(centre_x, line_y).expect("a fitted pixel"),
            [255, 64, 64, 255],
            "highlight red on the line centre"
        );
    }

    /// Four `render_topdown` calls, one per highlight list, each naming an
    /// index past the end of its table.
    fn out_of_range_highlights() -> Vec<(&'static str, Value)> {
        vec![
            (
                "linedefs",
                json!({ "highlight": { "linedefs": [999_u64] } }),
            ),
            ("sectors", json!({ "highlight": { "sectors": [9_u64] } })),
            ("things", json!({ "highlight": { "things": [9_u64] } })),
            ("vertices", json!({ "highlight": { "vertices": [99_u64] } })),
        ]
    }

    /// Assert that each out-of-range highlight call is refused `not_found`.
    async fn refuse_out_of_range(client: &Client, cases: &[(&'static str, Value)]) {
        let owned = cases.to_vec();
        for (field, args) in owned {
            let (index_code, _, _) = call_err(client, "render_topdown", args).await;
            assert_eq!(index_code, "not_found", "{field} index past the end");
        }
    }

    #[tokio::test]
    async fn render_topdown_refuses_unknown_maps_bad_bounds_and_indexes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (missing_code, missing_message, _) =
            call_err(&client, "render_topdown", json!({ "map": "MAP99" })).await;
        assert_eq!(missing_code, "not_found", "no such map");
        assert!(
            missing_message.contains("MAP99"),
            "the message names it: {missing_message}"
        );
        let (flat_code, flat_message, _) = call_err(
            &client,
            "render_topdown",
            json!({ "bounds": { "min_x": 100_i64, "min_y": 0_i64, "max_x": 100_i64, "max_y": 64_i64 } }),
        )
        .await;
        assert_eq!(flat_code, "invalid_argument", "flat bounds");
        assert!(
            flat_message.contains("min_x < max_x"),
            "the message states the rule: {flat_message}"
        );
        refuse_out_of_range(&client, &out_of_range_highlights()).await;
        let (proj_code, proj_message, _) = call_err(
            &client,
            "render_topdown",
            json!({ "projection": "birdseye" }),
        )
        .await;
        assert_eq!(proj_code, "invalid_argument", "not a projection");
        assert!(
            proj_message.contains("\"isometric\""),
            "the message lists the projections: {proj_message}"
        );
        let (size_code, _, _) =
            call_err(&client, "render_topdown", json!({ "width": 8_u64 })).await;
        assert_eq!(size_code, "invalid_argument", "a width below the floor");
    }

    #[tokio::test]
    async fn an_empty_map_needs_explicit_bounds_and_then_frames_them() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let extra = pwad_empty_map(dir.path());
        call(&client, "open_wad", path_arg(&extra)).await;
        let (code, message, _) = call_err(&client, "render_topdown", json!({})).await;
        assert_eq!(code, "invalid_argument", "nothing to frame");
        assert!(
            message.contains("bounds"),
            "the message names the fix: {message}"
        );
        let report = call(
            &client,
            "render_topdown",
            json!({
                "width": 64_u64, "height": 64_u64,
                "bounds": { "min_x": -128_i64, "min_y": -128_i64, "max_x": 128_i64, "max_y": 128_i64 },
            }),
        )
        .await;
        assert_eq!(report["width"], json!(64_u64), "the requested size");
        let image = rendered_image(&report).await;
        assert_eq!(non_background(&image), 0, "an empty map is pure background");
    }

    /// A `50x40` reference `PNG`: opaque red on the left half, blue on the
    /// right, written into `dir`.
    fn sketch_png(dir: &Path) -> PathBuf {
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
        let image = RgbaImage::new(50, 40, pixels).expect("the buffer matches the shape");
        let png = encode_png(&image, None).expect("the test picture encodes");
        let path = dir.join("sketch.png");
        fs::write(&path, png).expect("the sketch file writes");
        path
    }

    /// An `underlay` argument object for the sketch file: eight map units
    /// per image pixel with the `y` mirror, exactly the plan builder's
    /// image-space recipe.
    fn sketch_underlay(file: &Path) -> Value {
        json!({
            "source": file.display().to_string(),
            "units": { "scale_num": 8_i64, "flip_y": true },
        })
    }

    /// Whether any pixel within six of `at` equals `colour`.
    fn painted_near(image: &RgbaImage, at: (u32, u32), colour: [u8; 4]) -> bool {
        let columns = (i64::from(at.0).saturating_sub(6)..=i64::from(at.0).saturating_add(6))
            .filter_map(|col| u32::try_from(col).ok());
        let rows = (i64::from(at.1).saturating_sub(6)..=i64::from(at.1).saturating_add(6))
            .filter_map(|row| u32::try_from(row).ok());
        columns
            .flat_map(move |col| {
                rows.clone()
                    .map(move |row| image.get(col, row) == Some(colour))
            })
            .any(|hit| hit)
    }

    #[tokio::test]
    async fn an_underlay_frames_an_empty_map_and_shows_through_the_pixels() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        call(&client, "create_map", json!({ "name": "MAP01" })).await;
        let file = sketch_png(dir.path());
        let report = call(
            &client,
            "render_topdown",
            json!({
                "width": 200_u64, "height": 160_u64,
                "underlay": sketch_underlay(&file),
            }),
        )
        .await;
        let underlay = &report["underlay"];
        assert_eq!(underlay["width"], json!(50_u64), "the picture width");
        assert_eq!(underlay["height"], json!(40_u64), "the picture height");
        assert_eq!(
            underlay["map_bounds"],
            json!({ "min_x": 0_i64, "min_y": -320_i64, "max_x": 400_i64, "max_y": 0_i64 }),
            "eight units per pixel, mirrored about y = 0"
        );
        assert_eq!(report["notes"], json!([]), "nothing was skipped");
        let image = rendered_image(&report).await;
        let view = Transform::of(&report);
        let (red_x, red_y) = view.project(100, -100);
        let [red, _, blue, alpha] = image.get(red_x, red_y).expect("a fitted pixel");
        assert_eq!(alpha, 255_u8, "the background stays opaque");
        assert!(
            red > blue.saturating_add(40) && red > 64,
            "map (100, -100) is red half's pixel 13: {red},{blue}"
        );
        let (blue_x, blue_y) = view.project(300, -100);
        let [far_red, _, far_blue, _] = image.get(blue_x, blue_y).expect("a fitted pixel");
        assert!(
            far_blue > far_red.saturating_add(40) && far_blue > 64,
            "map (300, -100) is blue half's pixel 38: {far_red},{far_blue}"
        );
    }

    #[tokio::test]
    async fn a_zero_opacity_underlay_draws_nothing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        call(&client, "create_map", json!({ "name": "MAP01" })).await;
        let file = sketch_png(dir.path());
        let mut args = sketch_underlay(&file);
        args["opacity"] = json!(0_u64);
        let report = call(
            &client,
            "render_topdown",
            json!({
                "width": 64_u64, "height": 64_u64,
                "underlay": args,
            }),
        )
        .await;
        assert_eq!(
            report["underlay"]["width"],
            json!(50_u64),
            "the picture still reports"
        );
        let image = rendered_image(&report).await;
        assert_eq!(
            non_background(&image),
            0,
            "an empty map with a clear underlay is pure background"
        );
    }

    #[tokio::test]
    async fn an_underlay_source_that_is_no_picture_is_a_format_error() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = dir.path().join("not-a-picture.bin");
        fs::write(&file, b"definitely not an image").expect("the file writes");
        let client = connect().await;
        call(&client, "create_map", json!({ "name": "MAP01" })).await;
        let (code, message, _) = call_err(
            &client,
            "render_topdown",
            json!({ "underlay": { "source": file.display().to_string() } }),
        )
        .await;
        assert_eq!(code, "format_error", "the bytes are no image");
        assert!(
            message.contains("image") || message.contains("format"),
            "the message says what failed: {message}"
        );
    }

    #[tokio::test]
    async fn overlays_are_refused_on_the_isometric_projection() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        call(&client, "create_map", json!({ "name": "MAP01" })).await;
        let file = sketch_png(dir.path());
        let (code, message, _) = call_err(
            &client,
            "render_topdown",
            json!({ "projection": "isometric", "underlay": sketch_underlay(&file) }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "an overlay on isometric");
        assert!(
            message.contains("overlays need the topdown projection"),
            "the message names the projection: {message}"
        );
        let (grid_code, _, _) = call_err(
            &client,
            "render_topdown",
            json!({ "projection": "isometric", "grid": 64_i64 }),
        )
        .await;
        assert_eq!(grid_code, "invalid_argument", "a grid on isometric");
        let (label_code, _, _) = call_err(
            &client,
            "render_topdown",
            json!({ "projection": "isometric", "labels": { "sectors": true } }),
        )
        .await;
        assert_eq!(label_code, "invalid_argument", "labels on isometric");
    }

    #[tokio::test]
    async fn grid_lines_land_on_map_unit_multiples_and_a_fine_grid_notes_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "render_topdown",
            json!({
                "map": "MAP01", "width": 200_u64, "height": 120_u64,
                "grid": 64_i64, "show_things": false,
            }),
        )
        .await;
        assert_eq!(report["notes"], json!([]), "the grid drew");
        let image = rendered_image(&report).await;
        let view = Transform::of(&report);
        let (minor_x, minor_y) = view.project(64, -16);
        assert_eq!(
            image.get(minor_x, minor_y).expect("a fitted pixel"),
            [42, 42, 42, 255],
            "the x = 64 minor line below the map"
        );
        let (major_x, major_y) = view.project(512, -16);
        assert_eq!(
            image.get(major_x, major_y).expect("a fitted pixel"),
            [60, 60, 60, 255],
            "the eighth line at x = 512 draws major"
        );
        let (free_x, free_y) = view.project(96, -16);
        assert_eq!(
            image.get(free_x, free_y).expect("a fitted pixel"),
            [0, 0, 0, 255],
            "between the lines stays background"
        );
        let (cross_x, cross_y) = view.project(96, 128);
        assert_eq!(
            image.get(cross_x, cross_y).expect("a fitted pixel"),
            [42, 42, 42, 255],
            "the y = 128 line crosses the middle"
        );
        let skipped = call(
            &client,
            "render_topdown",
            json!({
                "map": "MAP01", "width": 64_u64, "height": 64_u64, "grid": 8_i64,
            }),
        )
        .await;
        let notes = skipped["notes"].as_array().expect("one note");
        assert_eq!(notes.len(), 1, "the fine grid reports its skip: {notes:?}");
        assert!(
            notes
                .first()
                .and_then(Value::as_str)
                .is_some_and(|note| note.contains("grid skipped")),
            "the note names the skip: {notes:?}"
        );
    }

    #[tokio::test]
    async fn sector_labels_sit_inside_their_sectors() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "render_topdown",
            json!({
                "map": "MAP01", "width": 200_u64, "height": 120_u64,
                "show_things": false, "labels": { "sectors": true },
            }),
        )
        .await;
        let image = rendered_image(&report).await;
        let view = Transform::of(&report);
        let label = [255_u8, 255, 128, 255];
        let hall = view.project(128, 128);
        let room = view.project(384, 128);
        assert!(
            painted_near(&image, hall, label),
            "the hall numbers itself at its inside point {hall:?}"
        );
        assert!(
            painted_near(&image, room, label),
            "the back room numbers itself at its inside point {room:?}"
        );
    }

    #[tokio::test]
    async fn annotation_rows_beyond_the_limits_are_refused_and_one_draws() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let long_text = "X".repeat(33_usize);
        let (code, message, _) = call_err(
            &client,
            "render_topdown",
            json!({ "annotations": [{ "x": 0_i64, "y": 0_i64, "text": long_text }] }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "33 characters");
        assert!(
            message.contains("32"),
            "the message names the character limit: {message}"
        );
        let many: Vec<Value> = (0_u32..65_u32)
            .map(|index| json!({ "x": 0_i64, "y": 0_i64, "text": format!("N{index}") }))
            .collect();
        let (count_code, count_message, _) =
            call_err(&client, "render_topdown", json!({ "annotations": many })).await;
        assert_eq!(count_code, "invalid_argument", "65 rows");
        assert!(
            count_message.contains("64"),
            "the message names the row limit: {count_message}"
        );
        let (colour_code, _, _) = call_err(
            &client,
            "render_topdown",
            json!({ "annotations": [{ "x": 0_i64, "y": 0_i64, "text": "A", "color": "green" }] }),
        )
        .await;
        assert_eq!(colour_code, "invalid_argument", "not a hex colour");
        let report = call(
            &client,
            "render_topdown",
            json!({
                "map": "MAP01", "width": 200_u64, "height": 120_u64, "show_things": false,
                "background": "#101010",
                "annotations": [{ "x": 128_i64, "y": 128_i64, "text": "hall", "color": "#FF00FF" }],
            }),
        )
        .await;
        let image = rendered_image(&report).await;
        let view = Transform::of(&report);
        assert!(
            painted_near(&image, view.project(128, 128), [255, 0, 255, 255]),
            "the magenta note stands at its world point"
        );
    }

    #[tokio::test]
    async fn render_topdown_defaults_follow_the_session_settings() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        call(
            &client,
            "set_render_resolution",
            json!({ "width": 320_u64, "height": 200_u64 }),
        )
        .await;
        call(
            &client,
            "set_render_options",
            json!({ "topdown_style": "automap", "show_things": false }),
        )
        .await;
        let report = call(&client, "render_topdown", json!({ "map": "MAP01" })).await;
        assert_eq!(report["width"], json!(320_u64), "the settings size");
        assert_eq!(report["height"], json!(200_u64), "the settings size");
        let image = rendered_image(&report).await;
        let view = Transform::of(&report);
        let (x, y) = view.project(0, 128);
        assert_eq!(
            image.get(x, y).expect("a fitted pixel"),
            [252, 0, 0, 255],
            "the settings' automap style drew the outer wall red"
        );
        let (px, py) = view.project(64, 128);
        assert_eq!(
            image.get(px, py).expect("a fitted pixel"),
            [0, 0, 0, 255],
            "show_things off left the player spot dark"
        );
    }

    /// The pixel of world `x`/`y` in a minimap report: the documented
    /// window mapping, `mid ± (v - centre) * size / (2 * radius)`
    /// floor-rounded.
    fn minimap_pixel(report: &Value, x: i64, y: i64) -> (u32, u32) {
        let size = report["size"].as_i64().expect("size");
        let centre_x = report["center"]["x"].as_i64().expect("centre x");
        let centre_y = report["center"]["y"].as_i64().expect("centre y");
        let radius = report["radius"].as_i64().expect("radius");
        let mid = size.checked_div(2).expect("a test size");
        let den = radius.checked_mul(2).expect("a test radius");
        let column = x
            .saturating_sub(centre_x)
            .checked_mul(size)
            .expect("a test product")
            .div_euclid(den);
        let row = y
            .saturating_sub(centre_y)
            .checked_mul(size)
            .expect("a test product")
            .div_euclid(den);
        (
            u32::try_from(mid.saturating_add(column)).expect("a fitted column"),
            u32::try_from(mid.saturating_sub(row)).expect("a fitted row"),
        )
    }

    /// The number of pixels of `image` that are exactly `color`.
    fn count_pixels(image: &RgbaImage, color: [u8; 4]) -> usize {
        (0..image.height)
            .flat_map(|row| (0..image.width).map(move |col| (col, row)))
            .filter(|&(col, row)| image.get(col, row) == Some(color))
            .count()
    }

    /// One default-framed minimap of the fixture, in the given extra args.
    async fn minimap_of(client: &Client, extra: Value) -> Value {
        let mut args = extra.as_object().cloned().unwrap_or_default();
        args.insert("map".to_owned(), json!("MAP01"));
        call(client, "render_minimap", Value::Object(args)).await
    }

    #[tokio::test]
    async fn render_minimap_frames_the_whole_map_in_automap_colours() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = minimap_of(&client, json!({ "palette_colors": false })).await;
        assert_eq!(report["map"], json!("MAP01"), "the rendered map");
        assert_eq!(report["size"], json!(256_u64), "the default edge");
        assert_eq!(
            report["center"],
            json!({ "x": 256_i64, "y": 128_i64 }),
            "the map centre"
        );
        assert_eq!(
            report["radius"],
            json!(257_i64),
            "half the long span, padded"
        );
        assert_eq!(
            report["asset"]["filename"],
            json!("MAP01_minimap.png"),
            "the published name"
        );
        let image = rendered_image(&report).await;
        assert_eq!((image.width, image.height), (256, 256), "the square PNG");
        let (west_x, west_y) = minimap_pixel(&report, 0, 128);
        assert_eq!(
            image.get(west_x, west_y).expect("a fitted pixel"),
            [252, 0, 0, 255],
            "the one-sided west wall in automap red"
        );
        let (step_x, step_y) = minimap_pixel(&report, 256, 120);
        assert_eq!(
            image.get(step_x, step_y).expect("a fitted pixel"),
            [188, 120, 72, 255],
            "the shared step line is brown for its floor change"
        );
        assert!(non_background(&image) > 0, "the line map drew");
        let (dot_x, dot_y) = minimap_pixel(&report, 64, 128);
        assert_eq!(
            image.get(dot_x, dot_y).expect("a fitted pixel"),
            [0, 0, 0, 255],
            "things are unmarked by default"
        );
    }

    #[tokio::test]
    async fn render_minimap_marks_things_with_category_dots_when_asked() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = minimap_of(
            &client,
            json!({ "palette_colors": false, "show_things": true }),
        )
        .await;
        let image = rendered_image(&report).await;
        let (start_x, start_y) = minimap_pixel(&report, 64, 128);
        assert_eq!(
            image.get(start_x, start_y).expect("a fitted pixel"),
            [64, 224, 64, 255],
            "the player start dot"
        );
        let next = start_x.checked_add(1_u32).expect("a fitted column");
        let below = start_y.checked_add(1_u32).expect("a fitted row");
        assert_eq!(
            image.get(next, below).expect("a fitted pixel"),
            [64, 224, 64, 255],
            "the dot is 2x2"
        );
        let (imp_x, imp_y) = minimap_pixel(&report, 384, 128);
        assert_eq!(
            image.get(imp_x, imp_y).expect("a fitted pixel"),
            [224, 64, 64, 255],
            "the imp dot"
        );
    }

    #[tokio::test]
    async fn palette_colours_come_from_the_fixture_playpal_automap_indices() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = minimap_of(&client, Value::Null).await;
        let image = rendered_image(&report).await;
        let (west_x, west_y) = minimap_pixel(&report, 0, 128);
        assert_eq!(
            image.get(west_x, west_y).expect("a fitted pixel"),
            [176, 79, 176, 255],
            "palette index 176 is the wall colour"
        );
        let (step_x, step_y) = minimap_pixel(&report, 256, 120);
        assert_eq!(
            image.get(step_x, step_y).expect("a fitted pixel"),
            [64, 191, 64, 255],
            "palette index 64 is the floor-change colour"
        );
    }

    #[tokio::test]
    async fn a_small_radius_zooms_and_moves_the_shared_line_away() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = minimap_of(
            &client,
            json!({
                "palette_colors": false,
                "center": { "x": 64_i64, "y": 128_i64 },
                "radius": 64_u64,
            }),
        )
        .await;
        assert_eq!(report["center"]["x"], json!(64_i64), "the given centre");
        assert_eq!(report["radius"], json!(64_u64), "the given radius");
        let image = rendered_image(&report).await;
        assert_eq!(
            count_pixels(&image, [188, 120, 72, 255]),
            0,
            "the step line at x 256 is outside this window"
        );
        assert!(
            count_pixels(&image, [252, 0, 0, 255]) > 0,
            "the west wall still crosses the zoomed square"
        );
        let (hall_x, hall_y) = minimap_pixel(&report, 64, 128);
        assert_eq!(
            image.get(hall_x, hall_y).expect("a fitted pixel"),
            [0, 0, 0, 255],
            "the shared line no longer runs through the centre"
        );
    }

    #[tokio::test]
    async fn the_player_arrow_is_white_from_its_position_to_its_tip() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = minimap_of(
            &client,
            json!({
                "palette_colors": false,
                "center": { "x": 64_i64, "y": 128_i64 },
                "radius": 64_u64,
                "player": { "x": 64_i64, "y": 128_i64, "angle": 0_i64 },
            }),
        )
        .await;
        let image = rendered_image(&report).await;
        let (at_x, at_y) = minimap_pixel(&report, 64, 128);
        assert_eq!(
            image.get(at_x, at_y).expect("a fitted pixel"),
            [255, 255, 255, 255],
            "the arrow starts at the player position"
        );
        let tip_x = at_x.checked_add(16_u32).expect("a fitted column");
        assert_eq!(
            image.get(tip_x, at_y).expect("a fitted pixel"),
            [255, 255, 255, 255],
            "the shaft runs 16 pixels east"
        );
        let behind = at_x.checked_sub(8_u32).expect("a fitted column");
        assert_eq!(
            image.get(behind, at_y).expect("a fitted pixel"),
            [0, 0, 0, 255],
            "nothing trails the tail"
        );
    }

    #[tokio::test]
    async fn minimap_sizes_and_radii_outside_their_ranges_are_refused() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (small_code, small_message, _) =
            call_err(&client, "render_minimap", json!({ "size": 16_u64 })).await;
        assert_eq!(small_code, "invalid_argument", "an edge below the floor");
        assert!(
            small_message.contains("32..=1024") && small_message.contains("16"),
            "the message shows the range and the value: {small_message}"
        );
        let (big_code, _, _) =
            call_err(&client, "render_minimap", json!({ "size": 2_048_u64 })).await;
        assert_eq!(big_code, "invalid_argument", "an edge above the ceiling");
        let (zero_code, zero_message, _) =
            call_err(&client, "render_minimap", json!({ "radius": 0_u64 })).await;
        assert_eq!(
            zero_code, "invalid_argument",
            "a zero radius frames nothing"
        );
        assert!(
            zero_message.contains("1..=65536") && zero_message.contains("radius"),
            "the message names the field and the range: {zero_message}"
        );
        let (far_code, _, _) =
            call_err(&client, "render_minimap", json!({ "radius": 100_000_u64 })).await;
        assert_eq!(far_code, "invalid_argument", "a radius past the ceiling");
    }

    #[tokio::test]
    async fn render_minimap_refuses_unknown_maps_and_windowless_empty_maps() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (missing_code, missing_message, _) =
            call_err(&client, "render_minimap", json!({ "map": "MAP99" })).await;
        assert_eq!(missing_code, "not_found", "no such map");
        assert!(
            missing_message.contains("MAP99"),
            "the message names it: {missing_message}"
        );
        let extra = pwad_empty_map(dir.path());
        call(&client, "open_wad", path_arg(&extra)).await;
        let (empty_code, empty_message, _) = call_err(&client, "render_minimap", json!({})).await;
        assert_eq!(empty_code, "invalid_argument", "nothing to frame");
        assert!(
            empty_message.contains("center") && empty_message.contains("radius"),
            "the message names both fixes: {empty_message}"
        );
        let report = call(
            &client,
            "render_minimap",
            json!({
                "center": { "x": 0_i64, "y": 0_i64 },
                "radius": 32_u64,
            }),
        )
        .await;
        assert_eq!(report["radius"], json!(32_u64), "the given radius");
        let image = rendered_image(&report).await;
        assert_eq!(non_background(&image), 0, "an empty map is pure black");
    }

    /// The `render_view` arguments over the fixture camera in the hall,
    /// with `extra` merged in.
    fn view_args(extra: &Value) -> Value {
        let mut args = json!({
            "map": "MAP01", "x": 64_i64, "y": 128_i64,
            "width": 160_u64, "height": 100_u64,
        });
        let Some(base) = args.as_object_mut() else {
            return args;
        };
        let Some(over) = extra.as_object() else {
            return args;
        };
        for (key, value) in over {
            base.insert(key.clone(), value.clone());
        }
        args
    }

    /// Refuses one `render_view` camera argument and requires the message
    /// to show `wanted`.
    async fn view_refused(client: &Client, field: &str, value: Value, wanted: &str) {
        let mut args = view_args(&Value::Null);
        args[field] = value;
        let (code, message, _) = call_err(client, "render_view", args).await;
        assert_eq!(code, "invalid_argument", "{field} out of range");
        assert!(
            message.contains(wanted),
            "the {field} refusal shows {wanted}: {message}"
        );
    }

    /// Light everything fully so palette colours land unchanged.
    async fn fullbright(client: &Client) {
        call(
            client,
            "set_render_options",
            json!({ "lighting": "fullbright" }),
        )
        .await;
    }

    #[tokio::test]
    async fn render_view_reports_the_camera_and_paints_the_hall_view() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        fullbright(&client).await;
        let report = call(&client, "render_view", view_args(&Value::Null)).await;
        assert_eq!(report["map"], json!("MAP01"), "the rendered map");
        assert_eq!(
            report["masked_columns"],
            json!(0_u64),
            "the plain fixture map masks nothing"
        );
        assert_eq!(
            report["camera"],
            json!({
                "x": 64_i64, "y": 128_i64, "z": 41_i64, "angle": 0_i64,
                "pitch": 0_i64, "fov": 90_u64, "sector": 0_u64,
            }),
            "the effective camera, standing on the hall floor"
        );
        assert_eq!(
            report["asset"]["filename"],
            json!("MAP01_view.png"),
            "the published name"
        );
        assert_eq!(
            report["problems"],
            json!([]),
            "an indoor map whose flats, textures and sprites the session has reports \
             nothing: {}",
            report["problems"]
        );
        let image = rendered_image(&report).await;
        assert_eq!((image.width, image.height), (160, 100), "the PNG shape");
        assert_eq!(
            image.get(80, 50).expect("a pixel"),
            [108_u8, 147, 108, 255],
            "the centre at the horizon is room 1's far WALL1 wall through the open step \
             line, a fake-contrast step darker on its north-south line"
        );
        assert_eq!(
            image.get(80, 60).expect("a pixel"),
            [158_u8, 97, 158, 255],
            "low in the centre column the STEP1 lower texture faces the step"
        );
        assert_eq!(
            image.get(0, 99).expect("a pixel"),
            [1_u8, 254, 1, 255],
            "the bottom-left pixel is the FLOOR1 average as floor"
        );
        assert_eq!(
            image.get(20, 0).expect("a pixel"),
            [1_u8, 254, 1, 255],
            "near the top-left the hall's own ceiling still shows"
        );
    }

    #[tokio::test]
    async fn render_view_pitch_moves_the_horizon_down_the_frame() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        fullbright(&client).await;
        let report = call(
            &client,
            "render_view",
            view_args(&json!({ "pitch": 30_i64 })),
        )
        .await;
        assert_eq!(report["camera"]["pitch"], json!(30_i64), "echoed back");
        let image = rendered_image(&report).await;
        assert_eq!(
            image.get(80, 84).expect("a pixel"),
            [108_u8, 147, 108, 255],
            "the far wall slid down with the horizon: its rows are now around 80 to 97"
        );
        assert_eq!(
            image.get(80, 50).expect("a pixel"),
            [1_u8, 254, 1, 255],
            "the old centre row now looks over the hall's ceiling"
        );
    }

    #[tokio::test]
    async fn render_view_takes_the_room_side_camera_and_an_explicit_eye_height() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "render_view",
            json!({ "map": "MAP01", "x": 384_i64, "y": 128_i64, "z": 100_i64,
                    "angle": 180_i64, "width": 160_u64, "height": 100_u64 }),
        )
        .await;
        assert_eq!(report["camera"]["sector"], json!(1_u64), "in room 1");
        assert_eq!(
            report["camera"]["z"],
            json!(100_i64),
            "the given eye height"
        );
        assert_eq!(
            report["camera"]["angle"],
            json!(180_i64),
            "looking back west into the hall"
        );
    }

    #[tokio::test]
    async fn render_view_doom_lighting_darkens_the_far_wall() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "render_view", view_args(&Value::Null)).await;
        let image = rendered_image(&report).await;
        assert_eq!(
            image.get(80, 50).expect("a pixel"),
            [156_u8, 99, 156, 255],
            "light 192 at depth 448 lands on table 7 of the fixture ramp: \
             index 100 + 56, darker than the fullbright wall's (108, 147, 108)"
        );
    }

    /// The number of pixels of `image`'s rows `top..` (the rest of the
    /// frame) that are exactly `color`.
    fn count_pixels_in_rows(image: &RgbaImage, top: u32, color: [u8; 4]) -> usize {
        (top..image.height)
            .flat_map(|row| (0..image.width).map(move |col| (col, row)))
            .filter(|&(col, row)| image.get(col, row) == Some(color))
            .count()
    }

    #[tokio::test]
    async fn render_view_textures_the_floor_with_its_flat_on_the_world_grid() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad_view_textures(dir.path());
        call(&client, "open_iwad", path_arg(&base)).await;
        fullbright(&client).await;
        let report = call(
            &client,
            "render_view",
            view_args(&json!({ "pitch": -45_i64 })),
        )
        .await;
        let image = rendered_image(&report).await;
        let left = count_pixels_in_rows(&image, 50, [10_u8, 245, 10, 255]);
        let right = count_pixels_in_rows(&image, 50, [20_u8, 235, 20, 255]);
        assert!(
            left > 0,
            "the CHECKER's index-10 half shows in the lower half: texel columns 0..32 of \
             the CHECKER 64x64 grid"
        );
        assert!(
            right > 0,
            "and the index-20 half too: {left}/{right} split by texel column 32"
        );
    }

    /// A fullbright client on the textured-view fixture `IWAD`.
    async fn sky_room_client(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad_view_textures(dir);
        call(&client, "open_iwad", path_arg(&base)).await;
        fullbright(&client).await;
        client
    }

    /// Asserts the sky band at the frame top of a room-1 view reads the
    /// `SKY1` texture colour: room 1 (`x 256..512`) has the `F_SKY1`
    /// ceiling and its sky rows start at the frame top, well above the
    /// horizon at row 50.
    fn assert_sky_band(image: &RgbaImage) {
        for (column, row) in [
            (40_u32, 0_u32),
            (40, 10),
            (80, 0),
            (80, 10),
            (120, 0),
            (120, 10),
        ] {
            assert_eq!(
                image.get(column, row).expect("a pixel"),
                [200_u8, 55, 200, 255],
                "the F_SKY1 ceiling draws the SKY1 texture at column {column} row {row}"
            );
        }
    }

    #[tokio::test]
    async fn render_view_draws_the_sky_texture_over_a_sky_ceiling() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = sky_room_client(dir.path()).await;
        let report = call(
            &client,
            "render_view",
            json!({ "map": "MAP01", "x": 384_i64, "y": 128_i64, "angle": 180_i64,
                    "width": 160_u64, "height": 100_u64 }),
        )
        .await;
        assert_eq!(
            report["camera"]["sector"],
            json!(1_u64),
            "standing in the sky room"
        );
        let image = rendered_image(&report).await;
        assert_sky_band(&image);
        assert!(
            !report["problems"]
                .as_array()
                .expect("problems is a list")
                .iter()
                .any(|problem| problem.as_str().is_some_and(|text| text.contains("SKY1"))),
            "the fixture SKY1 loaded: {}",
            report["problems"]
        );
        call(&client, "set_render_options", json!({ "sky": false })).await;
        let off = call(
            &client,
            "render_view",
            json!({ "map": "MAP01", "x": 384_i64, "y": 128_i64, "angle": 180_i64,
                    "width": 160_u64, "height": 100_u64 }),
        )
        .await;
        let dark = rendered_image(&off).await;
        assert_eq!(
            dark.get(80, 0).expect("a pixel"),
            [0_u8, 0, 0, 255],
            "with the sky switch off the sky ceiling reads the background colour"
        );
    }

    #[tokio::test]
    async fn render_view_paints_a_masked_grate_and_shows_the_far_flat_through_its_hole() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = sky_room_client(dir.path()).await;
        let report = call(
            &client,
            "render_view",
            view_args(&json!({ "map": "MAP02" })),
        )
        .await;
        assert_eq!(
            report["camera"]["sector"],
            json!(0_u64),
            "the camera stands in the hall before the grate"
        );
        let masked = report["masked_columns"]
            .as_u64()
            .expect("masked_columns is a number");
        assert!(masked >= 1, "the transparent pass painted {masked} columns");
        let image = rendered_image(&report).await;
        assert_eq!(
            image.get(70, 10).expect("a pixel"),
            [70_u8, 185, 70, 255],
            "the grate's texel left of the hole paints palette 70, fullbright"
        );
        assert_eq!(
            image.get(72, 10).expect("a pixel"),
            [70_u8, 185, 70, 255],
            "and the texel right of it"
        );
        assert_eq!(
            image.get(71, 9).expect("a pixel"),
            [70_u8, 185, 70, 255],
            "and one above"
        );
        assert_eq!(
            image.get(71, 11).expect("a pixel"),
            [70_u8, 185, 70, 255],
            "and one below"
        );
        assert_eq!(
            image.get(71, 10).expect("a pixel"),
            [1_u8, 254, 1, 255],
            "the GRATEP hole lets the FLOOR1 flat of the far ceiling show through"
        );
    }

    /// Palette index 50 of the fixture `PLAYPAL`: the colour of every
    /// texel of the view fixture's `TROOA1` under full bright.
    const IMP_GREEN: [u8; 4] = [50_u8, 205, 50, 255];

    #[tokio::test]
    async fn render_view_draws_the_imp_sprite_near_the_view_centre() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = sky_room_client(dir.path()).await;
        let report = call(&client, "render_view", view_args(&Value::Null)).await;
        let drawn = report["sprites_drawn"]
            .as_u64()
            .expect("sprites_drawn is a number");
        assert!(drawn >= 1, "the imp's sprite painted: {drawn}");
        assert_eq!(
            report["masked_columns"],
            json!(0_u64),
            "a sprite paints no masked column: MAP01 masks nothing"
        );
        let image = rendered_image(&report).await;
        assert_eq!(
            image.get(80, 50).expect("a pixel"),
            IMP_GREEN,
            "the sprite stands in front of the far wall, on the horizon row"
        );
        assert_eq!(
            image.get(80, 52).expect("a pixel"),
            IMP_GREEN,
            "and below the horizon, where it hid the hall floor"
        );
        let green = count_pixels(&image, IMP_GREEN);
        assert!(
            green >= 4,
            "the patch covers a small block at the centre: {green}"
        );
    }

    #[tokio::test]
    async fn show_sprites_off_drops_the_imp_from_the_view() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = sky_room_client(dir.path()).await;
        call(
            &client,
            "set_render_options",
            json!({ "show_sprites": false }),
        )
        .await;
        let report = call(&client, "render_view", view_args(&Value::Null)).await;
        assert_eq!(
            report["sprites_drawn"],
            json!(0_u64),
            "the switch draws no sprite at all"
        );
        let image = rendered_image(&report).await;
        assert_eq!(
            count_pixels(&image, IMP_GREEN),
            0,
            "no sprite texel lands anywhere"
        );
        assert_eq!(
            image.get(80, 50).expect("a pixel"),
            [108_u8, 147, 108, 255],
            "the far wall keeps the centre column where the sprite stood"
        );
    }

    #[tokio::test]
    async fn a_wall_between_camera_and_thing_hides_the_sprite_behind_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = sky_room_client(dir.path()).await;
        let report = call(
            &client,
            "render_view",
            view_args(&json!({ "map": "MAP03" })),
        )
        .await;
        assert_eq!(
            report["sprites_drawn"],
            json!(0_u64),
            "the sealed hall depth-tests every sprite out"
        );
        let image = rendered_image(&report).await;
        assert_eq!(
            count_pixels(&image, IMP_GREEN),
            0,
            "the imp paints no pixel from behind the wall"
        );
        assert_eq!(
            image.get(80, 50).expect("a pixel"),
            [108_u8, 147, 108, 255],
            "the WALL1 face that seals the hall paints the centre column it hides behind"
        );
    }

    #[tokio::test]
    async fn absent_sprite_lumps_are_reported_once_per_sprite_name() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = sky_room_client(dir.path()).await;
        let report = call(
            &client,
            "render_view",
            view_args(&json!({ "map": "MAP03" })),
        )
        .await;
        let problems = report["problems"].as_array().expect("problems is a list");
        let sprite_lines: Vec<&str> = problems
            .iter()
            .filter_map(Value::as_str)
            .filter(|line| line.ends_with("POSS") || line.ends_with("TROO"))
            .collect();
        assert_eq!(
            sprite_lines,
            ["missing sprite POSS"],
            "the two POSS things report one line and the present TROO none: {problems:?}"
        );
    }

    /// The `masked_columns` count of a `render_view` report.
    fn masked_of(report: &Value) -> u64 {
        report["masked_columns"]
            .as_u64()
            .expect("masked_columns is a number")
    }

    /// A fullbright client on the `DOOM II`-shaped fixture `IWAD`.
    async fn stock_client(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad_stock(dir);
        call(&client, "open_iwad", path_arg(&base)).await;
        fullbright(&client).await;
        client
    }

    /// The stock fixture's hall camera: the middle of its `512x512` room,
    /// facing `angle` degrees, over `map`.
    fn hall_view(map: &str, angle: i64) -> Value {
        json!({
            "map": map, "x": 256_i64, "y": 256_i64, "angle": angle,
            "width": 160_u64, "height": 100_u64,
        })
    }

    /// Every compass view from the middle of a stock-fixture hall is quiet:
    /// one of the eight ringed imps stands at each of the engine's rotation
    /// buckets, and each bucket resolves against the fixture's pair-named
    /// lumps, so nothing is reported and the imp dead ahead paints.
    async fn every_ring_angle_is_quiet(client: &Client, map: &str) {
        for angle in [0_i64, 45, 90, 135, 180, 225, 270, 315] {
            let report = call(client, "render_view", hall_view(map, angle)).await;
            assert_eq!(
                report["problems"],
                json!([]),
                "the {angle}-degree view of the indoor ring reports nothing"
            );
            let drawn = report["sprites_drawn"]
                .as_u64()
                .expect("sprites_drawn is a number");
            assert!(
                drawn >= 1,
                "the imp straight ahead paints at {angle} degrees: {drawn}"
            );
        }
    }

    /// The stock fixture stores the sky as a `TEXTURE1` record and the
    /// `TROO` frame over the mirror-pair names a real `IWAD` writes, so an
    /// indoor hall ringed by eight imps — one per engine rotation — has
    /// nothing to report from any of the eight compass views.
    #[tokio::test]
    async fn render_view_reports_nothing_for_a_stock_iwads_indoor_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = stock_client(dir.path()).await;
        every_ring_angle_is_quiet(&client, "MAP01").await;
    }

    /// Asserts the frame top of a stock-fixture sky-room view reads the
    /// `RSKY1` patch's colour: the hall's whole ceiling is `F_SKY1` and its
    /// sky rows start at the frame top, well above the horizon.
    fn assert_stock_sky_band(image: &RgbaImage) {
        for column in [40_u32, 80_u32, 120_u32] {
            assert_eq!(
                image.get(column, 0).expect("a pixel"),
                [200_u8, 55, 200, 255],
                "the F_SKY1 ceiling draws the RSKY1 patch at column {column}"
            );
        }
    }

    /// The stock fixture has no lump named `SKY1`: its sky is a texture
    /// definition, which is where the check has to look once a map actually
    /// shows a sky plane.
    #[tokio::test]
    async fn render_view_draws_a_sky_it_found_in_the_texture_definitions() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = stock_client(dir.path()).await;
        let report = call(&client, "render_view", hall_view("MAP02", 0)).await;
        assert_eq!(
            report["problems"],
            json!([]),
            "the SKY1 record is no missing sky"
        );
        let image = rendered_image(&report).await;
        assert_stock_sky_band(&image);
    }

    /// The other half of the same fix: a sprite the session really does not
    /// carry still costs exactly one line, and nothing else in the frame of
    /// the stock-fixture hall does.
    #[tokio::test]
    async fn render_view_reports_a_genuinely_missing_sprite_once() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = stock_client(dir.path()).await;
        let report = call(&client, "render_view", hall_view("MAP03", 0)).await;
        assert_eq!(
            report["problems"],
            json!(["missing sprite POSS"]),
            "the two POSS things are one line and the ring of imps is no line at all"
        );
        let drawn = report["sprites_drawn"]
            .as_u64()
            .expect("sprites_drawn is a number");
        assert!(
            drawn >= 1,
            "the imps the session has dress and draw: {drawn}"
        );
    }

    #[tokio::test]
    async fn masked_columns_stay_independent_of_the_sprite_switch() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = sky_room_client(dir.path()).await;
        let on = call(
            &client,
            "render_view",
            view_args(&json!({ "map": "MAP02" })),
        )
        .await;
        call(
            &client,
            "set_render_options",
            json!({ "show_sprites": false }),
        )
        .await;
        let off = call(
            &client,
            "render_view",
            view_args(&json!({ "map": "MAP02" })),
        )
        .await;
        assert!(masked_of(&on) >= 1, "the grate masks columns");
        assert_eq!(
            masked_of(&on),
            masked_of(&off),
            "the sprite switch moves no masked column"
        );
        assert_eq!(
            on["sprites_drawn"],
            json!(0_u64),
            "MAP02's only thing stands on the camera and draws nothing"
        );
    }

    #[tokio::test]
    async fn render_view_refuses_a_camera_outside_the_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (code, message, _) = call_err(
            &client,
            "render_view",
            json!({ "map": "MAP01", "x": -100_i64, "y": -100_i64 }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "no sector holds the camera");
        assert!(
            message.contains("outside"),
            "the message says the camera is outside: {message}"
        );
    }

    #[tokio::test]
    async fn render_view_refuses_out_of_range_camera_arguments() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        view_refused(&client, "pitch", json!(90_i64), "-60..=60").await;
        view_refused(&client, "fov", json!(200_u64), "10..=170").await;
        view_refused(&client, "width", json!(8_u64), "16..=4096").await;
        view_refused(&client, "lighting", json!("cinematic"), "\"fullbright\"").await;
    }

    #[tokio::test]
    async fn render_view_without_a_playpal_is_not_found() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad(dir.path());
        let bytes = fs::read(&base).expect("the fixture WAD is readable");
        let mut wad = Wad::parse(&bytes).expect("the fixture WAD parses");
        wad.lumps.retain(|lump| lump.name.to_string() != "PLAYPAL");
        let stripped = dir.path().join("no-playpal.wad");
        let out = wad.to_bytes().expect("the stripped archive serializes");
        fs::write(&stripped, out).expect("the stripped archive writes");
        call(&client, "open_iwad", path_arg(&stripped)).await;
        let (code, message, _) = call_err(
            &client,
            "render_view",
            json!({ "map": "MAP01", "x": 64_i64, "y": 128_i64 }),
        )
        .await;
        assert_eq!(code, "not_found", "a view needs a palette");
        assert!(
            message.contains("PLAYPAL"),
            "the refusal names the missing lump: {message}"
        );
    }

    /// A parsed test lump name.
    fn parsed_name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A session over the fixture archive `file`, with every lump named in
    /// `drop` removed and `extra` appended last.
    fn fixture_session(
        file: &Path,
        drop: &[&str],
        extra: Vec<Lump>,
    ) -> cyberdemon::session::Session {
        let bytes = fs::read(file).expect("the fixture WAD is readable");
        let mut wad = Wad::parse(&bytes).expect("the fixture WAD parses");
        wad.lumps
            .retain(|lump| !drop.contains(&lump.name.to_string().as_str()));
        wad.lumps.extend(extra);
        let mut session = cyberdemon::session::Session::default();
        session.set_iwad(cyberdemon::session::Layer::new(
            file.to_path_buf(),
            cyberdemon::session::LayerRole::Iwad,
            wad,
        ));
        session
    }

    /// Builds a scene over `session` with the default settings.
    fn scene_of(
        session: &cyberdemon::session::Session,
        map: &str,
    ) -> Result<cyberdemon::render::Scene, CyberError> {
        cyberdemon::render::build_scene(session, parsed_name(map), &RenderSettings::default())
    }

    /// The `code` of a [`CyberError`], for compact asserts.
    fn error_code(error: &CyberError) -> &str {
        error.code()
    }

    /// The identity lighting table, for `COLORMAP` assertions.
    fn identity_table() -> [u8; 256] {
        let mut table = [0_u8; 256];
        for (index, entry) in table.iter_mut().enumerate() {
            *entry = u8::try_from(index).expect("the table covers exactly the u8 range");
        }
        table
    }

    /// The fixture map's two composed wall textures.
    fn assert_wall_textures(scene: &cyberdemon::render::Scene) {
        let textures: Vec<&String> = scene.textures.keys().collect();
        assert_eq!(textures, ["STEP1", "WALL1"], "both mapped textures");
        assert_eq!(
            scene.textures["WALL1"].width, 64,
            "the WALL1 record's width"
        );
        assert_eq!(
            scene.textures["WALL1"].height, 128,
            "the WALL1 record's height"
        );
        assert_eq!(
            scene.textures["WALL1"].get(0, 0),
            Some((100, true)),
            "the tiled WALLP1 patch is opaque"
        );
        assert_eq!(
            scene.textures["STEP1"].height, 32,
            "the STEP1 record's height"
        );
        assert_eq!(
            scene.textures["STEP1"].get(0, 0),
            Some((150, true)),
            "its STEPP1 tiles drew too"
        );
    }

    /// The fallback sky a map that names no sky plane carries: it is never
    /// sampled, and it is reported as no problem.
    fn assert_fallback_sky(scene: &cyberdemon::render::Scene) {
        assert_eq!(scene.sky_flat, "F_SKY1", "the sky ceiling marker");
        assert_eq!(
            (scene.sky.width, scene.sky.height),
            (64, 64),
            "the fallback sky"
        );
        assert_eq!(scene.sky.get(0, 0), Some((0, true)), "brightest corner");
        assert_eq!(
            scene.sky.get(8, 0),
            Some((252, true)),
            "the darkest cell beside it (252..=255 tie at the lowest luminance)"
        );
    }

    #[test]
    fn build_scene_folds_the_fixture_iwad_into_a_scene() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let session = fixture_session(&iwad(dir.path()), &[], Vec::new());
        let settings = RenderSettings {
            gamma: 2,
            lighting: Lighting::Fullbright,
            ..RenderSettings::default()
        };
        let scene = cyberdemon::render::build_scene(&session, parsed_name("MAP01"), &settings)
            .expect("the fixture scene builds");
        assert_eq!(scene.map.name.to_string(), "MAP01", "the map");
        assert_eq!(scene.settings, settings, "the settings ride along");
        assert_eq!(
            scene.palette.color(3),
            Some([3_u8, 252_u8, 3_u8]),
            "the palette"
        );
        assert_wall_textures(&scene);
        let flats: Vec<&String> = scene.flats.keys().collect();
        assert_eq!(flats, ["FLOOR1"], "the one flat both rooms use");
        assert_eq!(
            scene.flats["FLOOR1"].get(0, 0),
            Some((1, true)),
            "its pixels"
        );
        assert_eq!(scene.colormaps.len(), 34, "the 34 lighting tables");
        assert_eq!(scene.colormaps[0], identity_table(), "the fixture's map");
        assert_fallback_sky(&scene);
        assert_eq!(scene.problems, Vec::<String>::new(), "nothing is missing");
    }

    #[test]
    fn build_scene_reports_a_sky_the_map_shows_and_the_session_lacks() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let session = fixture_session(&iwad_view_textures(dir.path()), &["SKY1"], Vec::new());
        let scene = scene_of(&session, "MAP01").expect("the sky room builds");
        assert!(
            scene.problems.contains(&"missing sky SKY1".to_owned()),
            "room 1's F_SKY1 ceiling asks for the sky the session has not: {:?}",
            scene.problems
        );
        assert_eq!(
            scene.sky.width, 64,
            "the sky falls back to the checkerboard"
        );
    }

    #[test]
    fn build_scene_falls_back_for_textures_the_session_cannot_compose() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let session = fixture_session(&iwad(dir.path()), &["TEXTURE1"], Vec::new());
        let scene = scene_of(&session, "MAP01").expect("the scene still builds without records");
        assert!(
            scene.problems.contains(&"missing texture WALL1".to_owned())
                && scene.problems.contains(&"missing texture STEP1".to_owned()),
            "both names are reported: {:?}",
            scene.problems
        );
        let wall = &scene.textures["WALL1"];
        assert_eq!((wall.width, wall.height), (64, 64), "the checkerboard size");
        assert_eq!(wall.get(0, 0), Some((0, true)), "brightest cell");
        assert_eq!(wall.get(8, 8), Some((0, true)), "the diagonal cell matches");
        assert_eq!(
            wall.get(8, 0),
            Some((252, true)),
            "the orthogonal cell differs"
        );
        assert_eq!(
            scene.flats["FLOOR1"].get(0, 0),
            Some((1, true)),
            "the flats are untouched"
        );
    }

    #[test]
    fn build_scene_refuses_a_session_without_a_playpal() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let session = fixture_session(&iwad(dir.path()), &["PLAYPAL"], Vec::new());
        let error = scene_of(&session, "MAP01").expect_err("rendering needs a palette");
        assert_eq!(error_code(&error), "not_found", "missing PLAYPAL code");
        assert!(
            error.to_string().contains("PLAYPAL"),
            "the refusal names the lump: {error}"
        );
    }

    #[test]
    fn build_scene_refuses_a_map_the_session_lacks() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let session = fixture_session(&iwad(dir.path()), &[], Vec::new());
        let error = scene_of(&session, "MAP99").expect_err("the fixture has no MAP99");
        assert_eq!(error_code(&error), "not_found", "missing map code");
        assert!(
            error.to_string().contains("MAP99"),
            "the refusal names the map: {error}"
        );
    }

    #[test]
    fn build_scene_generates_a_colormap_when_the_lump_is_missing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let session = fixture_session(&iwad(dir.path()), &["COLORMAP"], Vec::new());
        let scene = scene_of(&session, "MAP01").expect("the scene builds");
        assert_eq!(scene.colormaps.len(), 34, "the generated 34 tables");
        assert_eq!(scene.colormaps[0], identity_table(), "map 0 is identity");
        assert_ne!(scene.colormaps[1][255], 255, "map 1 darkens the top colour");
        assert!(
            scene.problems.contains(&"generated COLORMAP".to_owned()),
            "the generation is reported: {:?}",
            scene.problems
        );
    }

    #[test]
    fn build_scene_regenerates_a_too_short_colormap() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let stub = vec![Lump::new(parsed_name("COLORMAP"), vec![0_u8; 10])];
        let session = fixture_session(&base, &["COLORMAP"], stub);
        let scene = scene_of(&session, "MAP01").expect("a stub COLORMAP still builds a scene");
        assert_eq!(scene.colormaps.len(), 34, "the replacement has 34 tables");
        assert!(
            scene.problems.contains(&"generated COLORMAP".to_owned()),
            "the 10-byte lump is refused and regenerated: {:?}",
            scene.problems
        );
    }

    #[test]
    fn a_heretic_session_picks_its_episode_sky_and_reports_missing_pictures() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let mut outdoors = two_room_textured_map();
        outdoors.name = parsed_name("E2M1");
        let mut extra = write_doom_map(&outdoors).expect("the sky map serializes");
        extra.push(Lump::new(parsed_name("SKY2"), vec![7_u8; 4_096]));
        let session = fixture_session(&iwad_heretic(dir.path()), &[], extra);
        let scene = scene_of(&session, "E2M1").expect("the Heretic scene builds");
        assert_eq!(scene.sky.get(0, 0), Some((7, true)), "E2M1 skies over SKY2");
        assert!(
            !scene
                .problems
                .iter()
                .any(|problem| problem.starts_with("missing sky")),
            "the episode sky loaded: {:?}",
            scene.problems
        );
        assert!(
            scene.problems.contains(&"missing flat FLOOR1".to_owned())
                && scene.problems.contains(&"missing texture WALL1".to_owned()),
            "the Heretic fixture's pictures fall back and report: {:?}",
            scene.problems
        );
    }
}
