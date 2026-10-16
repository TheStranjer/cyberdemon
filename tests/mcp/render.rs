//! The `render` tool group as seen by a real MCP client: the session-level
//! render defaults, the validation each setter applies, how long the
//! settings live, and `render_topdown`' pictures: the fitted transform,
//! the style colours, sector fills, thing markers, isometric wireframe,
//! highlights and error codes.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::codec::decode_image;
    use cyberdemon::gfx::image::RgbaImage;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad, pwad_empty_map};
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
        let (code, message) = call_err(client, "set_render_options", args).await;
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
        let (code, message) = call_err(
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
        let (tall_code, tall_message) = call_err(
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
        let (code, message) = call_err(
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
        let (code, message) = call_err(
            &client,
            "set_render_options",
            json!({ "lighting": "cinematic" }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "unknown lighting");
        assert_lists(&message, &["\"doom\"", "\"flat\"", "\"fullbright\""]);
        let (style_code, style_message) = call_err(
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
        let (code, message) = call_err(
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
        let (code, message) = call_err(&client, "set_render_options", json!({})).await;
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
            let (index_code, _) = call_err(client, "render_topdown", args).await;
            assert_eq!(index_code, "not_found", "{field} index past the end");
        }
    }

    #[tokio::test]
    async fn render_topdown_refuses_unknown_maps_bad_bounds_and_indexes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (missing_code, missing_message) =
            call_err(&client, "render_topdown", json!({ "map": "MAP99" })).await;
        assert_eq!(missing_code, "not_found", "no such map");
        assert!(
            missing_message.contains("MAP99"),
            "the message names it: {missing_message}"
        );
        let (flat_code, flat_message) = call_err(
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
        let (proj_code, proj_message) = call_err(
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
        let (size_code, _) = call_err(&client, "render_topdown", json!({ "width": 8_u64 })).await;
        assert_eq!(size_code, "invalid_argument", "a width below the floor");
    }

    #[tokio::test]
    async fn an_empty_map_needs_explicit_bounds_and_then_frames_them() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let extra = pwad_empty_map(dir.path());
        call(&client, "open_wad", path_arg(&extra)).await;
        let (code, message) = call_err(&client, "render_topdown", json!({})).await;
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
}
