//! The `palette` tool group as seen by a real MCP client: `get_palette`
//! over the fixture `PLAYPAL` (colours, published swatch, `COLORMAP`
//! presence, error paths) and `set_palette` from hex lists, a 16x16 PNG
//! source and raw `PLAYPAL` bytes, including tint and `COLORMAP`
//! regeneration, copy-on-write into the working archive with the `IWAD`
//! layer left alone, and the refused calls.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use cyberdemon::gfx::codec::{decode_image, encode_png};
    use cyberdemon::gfx::image::RgbaImage;
    use cyberdemon::wad::archive::Wad;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad;
    use crate::support::{call, call_err, connect, http_get};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// The `{"path": ...}` argument object for `file`.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    /// Load the fixture `IWAD` (14 palettes, `COLORMAP`) as the base.
    async fn iwad_client(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(&client, "open_iwad", path_arg(&base)).await;
        client
    }

    /// Split an asset `URL` into the `http://localhost:<port>` origin and
    /// the request path for [`http_get`].
    fn split_asset_url(url: &str) -> (String, String) {
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        (base.to_owned(), format!("/assets/{rest}"))
    }

    /// Fetch a published asset with a raw `HTTP` `GET` and require success.
    async fn fetch_asset(url: &str) -> Vec<u8> {
        let (base, path) = split_asset_url(url);
        let (status, body) = http_get(&base, &path).await;
        assert_eq!(status, 200, "the asset serves: {url}");
        body
    }

    /// The `URL` of the swatch (or other asset) a report published.
    async fn asset_of(report: &Value) -> Vec<u8> {
        let url = report["swatch"]["url"]
            .as_str()
            .or_else(|| report["asset"]["url"].as_str())
            .expect("an asset url")
            .to_owned();
        fetch_asset(&url).await
    }

    /// The fixture palette: colour `i` is `(i, 255 - i, i)`, as hex.
    fn fixture_hex() -> Vec<String> {
        (0_u32..256)
            .map(|index| {
                let value = u8::try_from(index).expect("test colours fit u8");
                format!("#{value:02X}{:02X}{value:02X}", 255_u8.wrapping_sub(value))
            })
            .collect()
    }

    /// A red-ramp palette: colour `i` is `(i, 0, 0)`, as hex.
    fn red_hex() -> Vec<String> {
        (0_u32..256)
            .map(|index| {
                let value = u8::try_from(index).expect("test colours fit u8");
                format!("#{value:02X}0000")
            })
            .collect()
    }

    /// The `PLAYPAL` payload of the `IWAD` fixture, read from disk.
    fn disk_playpal(dir: &Path) -> Vec<u8> {
        let path = iwad(dir);
        let wad = Wad::read_file(&path).expect("the fixture reads");
        wad.lumps
            .iter()
            .find(|lump| lump.name.as_bytes() == b"PLAYPAL")
            .expect("the IWAD holds PLAYPAL")
            .data
            .to_vec()
    }

    /// A 16x16 `PNG` of the blue ramp (colour `i` is `(0, i, 255 - i)`).
    fn blue_png() -> Vec<u8> {
        let mut pixels = Vec::new();
        for index in 0_u8..=255 {
            pixels.extend_from_slice(&[0_u8, index, 255_u8.wrapping_sub(index), 255_u8]);
        }
        let image = RgbaImage::new(16, 16, pixels).expect("256 pixels fit a 16x16 image");
        encode_png(&image, None).expect("the test PNG encodes")
    }

    /// Raw `PLAYPAL` bytes: `palettes` copies of the inverted ramp
    /// (colour `i` is `(255 - i, i, 255 - i)`).
    fn raw_palettes(palettes: usize) -> Vec<u8> {
        let mut first = Vec::new();
        for index in 0_u8..=255 {
            first.extend_from_slice(&[
                255_u8.wrapping_sub(index),
                index,
                255_u8.wrapping_sub(index),
            ]);
        }
        first.repeat(palettes)
    }

    /// The `get_session_info` undo-label list.
    async fn undo_labels(client: &Client) -> Vec<String> {
        let info = call(client, "get_session_info", Value::Null).await;
        info["undo"]
            .as_array()
            .expect("undo is an array")
            .iter()
            .map(|entry| entry.as_str().expect("labels are strings").to_owned())
            .collect()
    }

    /// The `colors` array of a report as strings.
    fn colors_of(report: &Value) -> Vec<String> {
        report["colors"]
            .as_array()
            .expect("colors is an array")
            .iter()
            .map(|entry| entry.as_str().expect("colours are strings").to_owned())
            .collect()
    }

    #[tokio::test]
    async fn get_palette_reports_the_fixture_ramp_and_serves_a_swatch() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_palette", json!({})).await;
        assert_eq!(report["palettes"], json!(14_u64), "the fixture PLAYPAL");
        assert_eq!(report["index"], json!(0_u64), "the default index");
        assert_eq!(report["has_colormap"], json!(true), "the fixture COLORMAP");
        let colors = colors_of(&report);
        assert_eq!(colors.len(), 256, "256 colours");
        assert_eq!(colors[0], "#00FF00", "the fixture green");
        assert_eq!(colors[16], "#10EF10", "colour 16");
        assert_eq!(colors[255], "#FF00FF", "the fixture magenta");
        assert_eq!(colors, fixture_hex(), "the whole palette is the fixture");
        let url = report["swatch"]["url"].as_str().expect("a swatch url");
        assert!(
            url.ends_with("/PLAYPAL_0.png"),
            "the documented name: {url}"
        );
        assert_eq!(report["swatch"]["mime"], json!("image/png"), "a PNG");
        let body = asset_of(&report).await;
        let image = decode_image(&body).expect("the swatch is a PNG");
        assert_eq!(
            (image.width, image.height),
            (256, 256),
            "the documented shape"
        );
        assert_eq!(
            image.get(16, 0),
            Some([1_u8, 254_u8, 1_u8, 255_u8]),
            "cell 1 of the top row carries colour 1"
        );
        assert_eq!(
            image.get(0, 16),
            Some([16_u8, 239_u8, 16_u8, 255_u8]),
            "the second row starts at colour 16"
        );
        assert_eq!(
            image.get(255, 255),
            Some([255_u8, 0_u8, 255_u8, 255_u8]),
            "the last cell carries the last colour"
        );
    }

    #[tokio::test]
    async fn get_palette_reads_a_later_entry_by_index() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_palette", json!({ "index": 13_u64 })).await;
        assert_eq!(report["index"], json!(13_u64), "the requested index");
        assert_eq!(
            colors_of(&report),
            fixture_hex(),
            "the copies hold the fixture"
        );
        let url = report["swatch"]["url"].as_str().expect("a swatch url");
        assert!(
            url.ends_with("/PLAYPAL_13.png"),
            "the index names the file: {url}"
        );
    }

    #[tokio::test]
    async fn get_palette_refuses_an_index_past_the_playpal() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (code, message, _) = call_err(&client, "get_palette", json!({ "index": 14_u64 })).await;
        assert_eq!(code, "not_found", "there is no palette 14");
        assert!(
            message.contains("holds 14 palettes"),
            "the message states the count: {message}"
        );
    }

    #[tokio::test]
    async fn get_palette_without_an_iwad_points_at_open_iwad() {
        let client = connect().await;
        let (code, message, _) = call_err(&client, "get_palette", json!({})).await;
        assert_eq!(code, "not_found", "an empty session has no PLAYPAL");
        assert!(
            message.contains("open_iwad"),
            "the hint names the tool: {message}"
        );
    }

    #[tokio::test]
    async fn set_palette_from_hex_replaces_the_merged_copy_only() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let expected_disk = disk_playpal(dir.path());
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "set_palette", json!({ "colors": red_hex() })).await;
        assert_eq!(report["palettes"], json!(14_u64), "the count stands");
        assert_eq!(report["replaced"], json!([0_u64]), "entry 0 replaced");
        assert_eq!(
            report["colormap_regenerated"],
            json!(false),
            "no COLORMAP was asked for"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["set_palette"],
            "one labelled edit"
        );
        let after = call(&client, "get_palette", json!({})).await;
        let colors = colors_of(&after);
        assert_eq!(colors[16], "#100000", "the new red ramp");
        assert_eq!(colors[255], "#FF0000", "the ramp tops out pure red");
        let untouched = call(&client, "get_palette", json!({ "index": 1_u64 })).await;
        assert_eq!(
            colors_of(&untouched),
            fixture_hex(),
            "the other entries stand"
        );
        let on_disk = disk_playpal(dir.path());
        assert_eq!(
            on_disk, expected_disk,
            "the IWAD file on disk never changed"
        );
    }

    #[tokio::test]
    async fn set_palette_appends_a_new_entry_at_the_count() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "set_palette",
            json!({ "colors": red_hex(), "index": 14_u64 }),
        )
        .await;
        assert_eq!(report["palettes"], json!(15_u64), "one palette appended");
        assert_eq!(report["replaced"], json!([14_u64]), "the new entry");
        let fresh = call(&client, "get_palette", json!({ "index": 14_u64 })).await;
        assert_eq!(colors_of(&fresh)[255], "#FF0000", "the appended ramp");
        let zero = call(&client, "get_palette", json!({})).await;
        assert_eq!(colors_of(&zero), fixture_hex(), "palette 0 untouched");
    }

    #[tokio::test]
    async fn set_palette_reads_a_16x16_png_source() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let path = dir.path().join("blue-ramp.png");
        fs::write(&path, blue_png()).expect("the test file writes");
        let report = call(
            &client,
            "set_palette",
            json!({ "source": path.display().to_string() }),
        )
        .await;
        assert_eq!(
            report["replaced"],
            json!([0_u64]),
            "the image fills one entry"
        );
        let after = call(&client, "get_palette", json!({})).await;
        let colors = colors_of(&after);
        assert_eq!(colors[0], "#0000FF", "the blue ramp starts blue");
        assert_eq!(colors[16], "#0010EF", "row-major pixel 16");
        assert_eq!(colors[255], "#00FF00", "and ends green");
    }

    #[tokio::test]
    async fn set_palette_reads_raw_playpal_bytes_whole() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let path = dir.path().join("playpal.bin");
        fs::write(&path, raw_palettes(3)).expect("the test file writes");
        let report = call(
            &client,
            "set_palette",
            json!({ "source": path.display().to_string() }),
        )
        .await;
        assert_eq!(report["palettes"], json!(3_u64), "the PLAYPAL was replaced");
        assert_eq!(
            report["replaced"],
            json!([0_u64, 1_u64, 2_u64]),
            "every carried entry counts as written"
        );
        let after = call(&client, "get_palette", json!({ "index": 2_u64 })).await;
        assert_eq!(colors_of(&after)[254], "#01FE01", "the inverted ramp");
        let (code, _, _) = call_err(&client, "get_palette", json!({ "index": 3_u64 })).await;
        assert_eq!(code, "not_found", "the shortened lump has no entry 3");
    }

    #[tokio::test]
    async fn regenerate_tints_rebuilds_the_vanilla_ramp_from_palette_zero() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "set_palette",
            json!({ "colors": fixture_hex(), "regenerate_tints": true }),
        )
        .await;
        assert_eq!(report["palettes"], json!(14_u64), "the vanilla count");
        let red = call(&client, "get_palette", json!({ "index": 1_u64 })).await;
        assert_eq!(
            colors_of(&red)[0],
            "#1CE300",
            "the fixture green reddened by 1/9: palette 1 is redder than palette 0"
        );
        let gold = call(&client, "get_palette", json!({ "index": 9_u64 })).await;
        assert_ne!(
            colors_of(&gold),
            fixture_hex(),
            "the pickup tint differs too"
        );
        let suit = call(&client, "get_palette", json!({ "index": 13_u64 })).await;
        assert_eq!(
            colors_of(&suit)[255],
            "#DF20DF",
            "magenta lifted 1/8 toward green"
        );
        let labels = undo_labels(&client).await;
        assert_eq!(labels, vec!["set_palette"], "still ONE edit for all 13");
    }

    #[tokio::test]
    async fn regenerate_colormap_rebuilds_thirty_four_tables() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(
            &client,
            "set_palette",
            json!({ "colors": fixture_hex(), "regenerate_colormap": true }),
        )
        .await;
        assert_eq!(
            report["colormap_regenerated"],
            json!(true),
            "COLORMAP was rebuilt"
        );
        let lump = call(
            &client,
            "get_lump",
            json!({ "name": "COLORMAP", "scope": "working", "format": "url" }),
        )
        .await;
        let maps = asset_of(&lump).await;
        assert_eq!(
            maps.len(),
            34_usize.checked_mul(256).expect("test size fits"),
            "34 x 256"
        );
        let identity: Vec<u8> = (0_u8..=255).collect();
        assert_eq!(&maps[..256], identity.as_slice(), "map 0 is the identity");
        let start = 33_usize.checked_mul(256).expect("test size fits");
        let black = maps.get(start).copied().expect("map 33 exists");
        assert!(
            maps.get(start..)
                .expect("map 33")
                .iter()
                .all(|&value| value == black),
            "map 33 maps everything alike"
        );
        assert_eq!(black, 85, "the fixture's nearest colour to black");
        let info = call(&client, "get_palette", json!({})).await;
        assert_eq!(
            info["has_colormap"],
            json!(true),
            "the session still has a COLORMAP"
        );
    }

    #[tokio::test]
    async fn refusing_calls_changes_nothing_and_records_no_history() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (none_code, none_message, _) = call_err(&client, "set_palette", json!({})).await;
        assert_eq!(none_code, "invalid_argument", "no source given");
        assert!(
            none_message.contains("exactly one"),
            "the message says so: {none_message}"
        );
        call_err(
            &client,
            "set_palette",
            json!({ "colors": red_hex(), "source": "/palette.png" }),
        )
        .await;
        let mut broken = red_hex();
        broken
            .get_mut(3)
            .expect("256 colours")
            .replace_range(.., "nope");
        call_err(&client, "set_palette", json!({ "colors": broken })).await;
        let short = red_hex().into_iter().take(8).collect::<Vec<String>>();
        call_err(&client, "set_palette", json!({ "colors": short })).await;
        let (range_code, range_message, _) = call_err(
            &client,
            "set_palette",
            json!({ "colors": red_hex(), "index": 20_u64 }),
        )
        .await;
        assert_eq!(range_code, "invalid_argument", "index past the end");
        assert!(
            range_message.contains("past the end"),
            "the message points at the range: {range_message}"
        );
        let junk = dir.path().join("junk.bin");
        fs::write(&junk, vec![7_u8; 10]).expect("the test file writes");
        let junk_arg = json!({ "source": junk.display().to_string() });
        let missing_arg = json!({ "source": "/definitely-not-here/palette.png" });
        let (junk_code, _, _) = call_err(&client, "set_palette", junk_arg).await;
        assert_eq!(
            junk_code, "invalid_argument",
            "neither image nor whole palettes"
        );
        let (missing_code, _, _) = call_err(&client, "set_palette", missing_arg).await;
        assert_eq!(missing_code, "not_found", "a missing source file");
        assert!(
            undo_labels(&client).await.is_empty(),
            "no refused call edited"
        );
        let palette = call(&client, "get_palette", json!({})).await;
        assert_eq!(
            colors_of(&palette),
            fixture_hex(),
            "the palette still stands"
        );
    }

    #[tokio::test]
    async fn set_palette_creates_a_missing_playpal_in_a_fresh_wad() {
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        let report = call(&client, "set_palette", json!({ "colors": red_hex() })).await;
        assert_eq!(report["palettes"], json!(1_u64), "the PLAYPAL was created");
        assert_eq!(report["replaced"], json!([0_u64]), "with one entry");
        let after = call(&client, "get_palette", json!({})).await;
        assert_eq!(colors_of(&after)[16], "#100000", "the new ramp reads back");
        assert_eq!(
            after["has_colormap"],
            json!(false),
            "a fresh archive has no COLORMAP"
        );
    }
}
