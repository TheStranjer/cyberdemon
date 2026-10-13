//! The `render` tool group as seen by a real MCP client: the session-level
//! render defaults, the validation each setter applies, and how long the
//! settings live.

#[cfg(test)]
mod tests {
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::pwad;
    use crate::support::{call, call_err, connect};

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
    fn path_arg(file: &std::path::Path) -> Value {
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
}
