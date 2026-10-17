//! The `assets` tool group as seen by a real MCP client: server discovery,
//! uploads through the asset server, and runtime capacity changes.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use cyberdemon::assets::store::MAX_ASSET_BYTES;
    use cyberdemon::audio::{Pcm, encode_dmx};
    use cyberdemon::gfx::codec::{decode_image, encode_png};
    use cyberdemon::gfx::grab::read_grab;
    use cyberdemon::gfx::image::RgbaImage;
    use cyberdemon::music::{MidiWriter, midi_to_mus};

    use crate::fixtures::iwad;
    use crate::support::{call, call_err, connect, http_get, http_put};

    /// The client handle these tests call through.
    type Client = RunningService<RoleClient, ()>;

    /// The port out of a `http://localhost:<port>` base `URL`.
    fn port_of(base_url: &str) -> u16 {
        let text = base_url
            .strip_prefix("http://localhost:")
            .expect("a localhost origin");
        text.parse().expect("a numeric port")
    }

    /// Call `get_asset_server_info` and return its structured content.
    async fn info(client: &RunningService<RoleClient, ()>) -> Value {
        call(client, "get_asset_server_info", Value::Null).await
    }

    /// Upload `body` to the server behind `base_url` and require acceptance.
    async fn upload(base_url: &str, filename: &str, body: &[u8]) {
        let path = format!("/upload/{filename}");
        let (status, receipt) = http_put(base_url, &path, body).await;
        assert_eq!(status, 201, "upload of {filename} rejected: {receipt:?}");
    }

    #[tokio::test]
    async fn asset_server_info_describes_a_fresh_server() {
        let client = connect().await;
        let report = info(&client).await;
        let base_url = report["base_url"].as_str().expect("a base url");
        let port = port_of(base_url);
        assert!(
            (3_000_u16..=60_000_u16).contains(&port),
            "port {port} is outside 3000..=60000"
        );
        assert_eq!(
            report["upload_url_template"].as_str(),
            Some(format!("{base_url}/upload/{{filename}}").as_str()),
            "the upload template sits on the same origin"
        );
        assert_eq!(report["capacity"].as_u64(), Some(50), "default capacity");
        assert_eq!(report["stored"].as_u64(), Some(0), "a fresh store is empty");
        assert_eq!(
            report["stored_bytes"].as_u64(),
            Some(0),
            "an empty store holds no bytes"
        );
        assert_eq!(
            report["max_asset_bytes"].as_u64(),
            Some(u64::try_from(MAX_ASSET_BYTES).expect("a usize fits u64")),
            "the advertised upload cap"
        );
        let again = info(&client).await;
        assert_eq!(
            again["base_url"].as_str(),
            Some(base_url),
            "the second call reuses one server"
        );
    }

    #[tokio::test]
    async fn an_upload_shows_up_in_the_stored_counts() {
        let client = connect().await;
        let report = info(&client).await;
        let base_url = report["base_url"].as_str().expect("a base url");
        let template = report["upload_url_template"]
            .as_str()
            .expect("an upload template");
        let url = template.replace("{filename}", "pic.png");
        let path = url
            .strip_prefix(base_url)
            .expect("the template origin matches");
        let body = b"not really a png".to_vec();
        let expected_bytes = body.len();
        let (status, _receipt) = http_put(base_url, path, &body).await;
        assert_eq!(status, 201, "the upload is stored");
        let after = info(&client).await;
        assert_eq!(after["stored"].as_u64(), Some(1), "one asset stored");
        assert_eq!(
            after["stored_bytes"].as_u64(),
            Some(u64::try_from(expected_bytes).expect("a small body fits u64")),
            "the uploaded bytes are counted"
        );
    }

    #[tokio::test]
    async fn shrinking_the_capacity_evicts_the_oldest() {
        let client = connect().await;
        let report = info(&client).await;
        let base_url = report["base_url"].as_str().expect("a base url").to_owned();
        upload(&base_url, "first.bin", b"1").await;
        upload(&base_url, "second.bin", b"2").await;
        let result = call(
            &client,
            "set_asset_capacity",
            json!({ "capacity": 1_usize }),
        )
        .await;
        assert_eq!(result["capacity"].as_u64(), Some(1), "the new capacity");
        assert_eq!(
            result["evicted"].as_u64(),
            Some(1),
            "the shrink dropped the oldest asset"
        );
        let after = info(&client).await;
        assert_eq!(after["capacity"].as_u64(), Some(1), "the capacity persists");
        assert_eq!(
            after["stored"].as_u64(),
            Some(1),
            "only the newest asset remains"
        );
    }

    #[tokio::test]
    async fn capacity_outside_the_range_is_an_invalid_argument() {
        let client = connect().await;
        let zero = call_err(
            &client,
            "set_asset_capacity",
            json!({ "capacity": 0_usize }),
        )
        .await;
        assert_eq!(zero.0, "invalid_argument", "zero capacity: {}", zero.1);
        let above = call_err(
            &client,
            "set_asset_capacity",
            json!({ "capacity": 10_001_usize }),
        )
        .await;
        assert_eq!(
            above.0, "invalid_argument",
            "above the maximum: {}",
            above.1
        );
        let report = info(&client).await;
        assert_eq!(
            report["capacity"].as_u64(),
            Some(50),
            "rejected calls leave the store untouched"
        );
    }

    /// A tool argument naming one path.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    /// The fixture `IWAD` open as the session's base layer: flats, patches,
    /// sprites, textures, `PNAMES`, `PLAYPAL` and `COLORMAP`.
    async fn iwad_client(dir: &Path) -> Client {
        let client = connect().await;
        call(&client, "open_iwad", path_arg(&iwad(dir))).await;
        client
    }

    /// The hex spelling `add_lump` takes: byte values separated by spaces.
    fn hex_of(bytes: &[u8]) -> String {
        bytes
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Adds one working-archive lump of `bytes`, optionally inside a region.
    async fn add(client: &Client, name: &str, bytes: &[u8], namespace: Option<&str>) {
        let report = call(
            client,
            "add_lump",
            json!({ "name": name, "hex": hex_of(bytes), "namespace": namespace }),
        )
        .await;
        assert_eq!(
            report["name"],
            json!(name),
            "the lump is in the working archive"
        );
    }

    /// The `URL` one report published.
    fn url_of(report: &Value) -> &str {
        report["asset"]["url"]
            .as_str()
            .expect("a published asset carries a url")
    }

    /// Asserts the identity a report answers with: the family it resolved
    /// to, its media type and the name it is stored under.
    fn check(report: &Value, resolved_as: &str, mime: &str, filename: &str) {
        assert_eq!(
            report["resolved_as"],
            json!(resolved_as),
            "the family: {report}"
        );
        assert_eq!(report["mime"], json!(mime), "the media type");
        assert_eq!(report["asset"]["mime"], json!(mime), "stored with the type");
        assert_eq!(
            report["asset"]["filename"],
            json!(filename),
            "the file name"
        );
    }

    /// Asserts the published pixel size of a picture report.
    fn check_dims(report: &Value, width: u64, height: u64) {
        assert_eq!(
            (report["width"].as_u64(), report["height"].as_u64()),
            (Some(width), Some(height)),
            "the published dimensions: {report}"
        );
    }

    /// Asserts a sound or music report carries no pixel size.
    fn check_no_dims(report: &Value) {
        assert!(
            report.get("width").is_none() && report.get("height").is_none(),
            "no dimensions for a playable file: {report}"
        );
    }

    /// GET a published asset and require the bytes it serves.
    async fn fetch_asset(url: &str) -> Vec<u8> {
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
        assert_eq!(status, 200, "the asset serves: {url}");
        body
    }

    /// GET a published asset and decode it as an image.
    async fn fetch_image(url: &str) -> RgbaImage {
        let body = fetch_asset(url).await;
        decode_image(&body).expect("the asset is a readable PNG")
    }

    /// A mono `DMX` sound lump of a short sawtooth.
    fn dmx_lump() -> Vec<u8> {
        let samples: Vec<u8> = (0_u32..96)
            .map(|step| u8::try_from(step).expect("96 fits"))
            .collect();
        let pcm = Pcm::from_u8_unsigned(&samples, 11_025);
        encode_dmx(&pcm, true).expect("a mono sound encodes as DMX")
    }

    /// A `Doom` `MUS` lump: the one-note score `MidiWriter` can express.
    fn mus_lump() -> Vec<u8> {
        let mut midi = MidiWriter::new();
        midi.put_tempo(0_u32, 500_000_u32);
        midi.put_event(0_u32, 0x90_u8, &[0x3C_u8, 0x64_u8]);
        midi.put_event(70_u32, 0x80_u8, &[0x3C_u8, 0x00_u8]);
        let bytes = midi.into_type0(70_u16, 0_u32).expect("the score encodes");
        midi_to_mus(&bytes).expect("the score converts").mus
    }

    /// A 14-entry `PLAYPAL` whose entry `n` colours every index `(n, n, n)`,
    /// so the palette a call asks for shows up in the published pixels.
    fn grey_playpal() -> Vec<u8> {
        (0_u8..14)
            .flat_map(|entry| (0_u16..256).flat_map(move |_index| [entry, entry, entry]))
            .collect()
    }

    #[tokio::test]
    async fn a_flat_publishes_a_png_of_its_indexed_plane() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_asset_url", json!({ "name": "floor1" })).await;
        assert_eq!(report["name"], json!("FLOOR1"), "the name is normalised");
        check(&report, "flat", "image/png", "FLOOR1.png");
        check_dims(&report, 64, 64);
        let image = fetch_image(url_of(&report)).await;
        assert_eq!(
            image.get(0, 0),
            Some([1_u8, 254, 1, 255]),
            "the plane's index through palette 0"
        );
        assert_eq!(
            image.get(63, 63),
            Some([1_u8, 254, 1, 255]),
            "all one colour"
        );
    }

    #[tokio::test]
    async fn a_png_in_the_flats_region_publishes_as_a_flat() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_asset_url", json!({ "name": "PNGFLAT" })).await;
        check(&report, "flat", "image/png", "PNGFLAT.png");
        check_dims(&report, 64, 64);
        let image = fetch_image(url_of(&report)).await;
        assert_eq!(
            image.get(32, 32),
            Some([64_u8, 191, 64, 255]),
            "the stored PNG recoloured through palette 0"
        );
    }

    #[tokio::test]
    async fn a_patch_publishes_its_drawing_offsets_in_the_png() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_asset_url", json!({ "name": "WALLP1" })).await;
        check(&report, "patch", "image/png", "WALLP1.png");
        check_dims(&report, 16, 16);
        let body = fetch_asset(url_of(&report)).await;
        assert_eq!(
            read_grab(&body).expect("the published PNG's chunks parse"),
            Some((-2_i32, -14_i32)),
            "the grAb chunk carries the stored offsets"
        );
    }

    #[tokio::test]
    async fn a_sprite_picture_reports_itself_as_a_sprite() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_asset_url", json!({ "name": "TROOA1" })).await;
        check(&report, "sprite", "image/png", "TROOA1.png");
        check_dims(&report, 8, 8);
        let image = fetch_image(url_of(&report)).await;
        assert_eq!(image.get(0, 0), Some([51_u8, 204, 51, 255]), "0x33 in grey");
    }

    #[tokio::test]
    async fn a_texture_name_no_lump_carries_still_composites() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_asset_url", json!({ "name": "WALL1" })).await;
        check(&report, "texture", "image/png", "WALL1.png");
        check_dims(&report, 64, 128);
        let image = fetch_image(url_of(&report)).await;
        assert_eq!(
            image.get(0, 0),
            Some([17_u8, 238, 17, 255]),
            "the tiled mappatch's index"
        );
        assert_eq!(
            image.get(63, 127),
            Some([17_u8, 238, 17, 255]),
            "the far corner is covered too"
        );
    }

    #[tokio::test]
    async fn prefer_chooses_between_the_lump_and_the_definition() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add(&client, "STEP1", &vec![3_u8; 4_096], Some("flats")).await;
        let lump = call(&client, "get_asset_url", json!({ "name": "STEP1" })).await;
        check(&lump, "flat", "image/png", "STEP1.png");
        check_dims(&lump, 64, 64);
        let texture = call(
            &client,
            "get_asset_url",
            json!({ "name": "STEP1", "prefer": "texture" }),
        )
        .await;
        check(&texture, "texture", "image/png", "STEP1.png");
        check_dims(&texture, 64, 32);
        let image = fetch_image(url_of(&texture)).await;
        assert_eq!(
            image.get(0, 0),
            Some([34_u8, 221, 34, 255]),
            "STEPP1 tiled, not the flat's index"
        );
        let no_definition = call(
            &client,
            "get_asset_url",
            json!({ "name": "FLOOR1", "prefer": "texture" }),
        )
        .await;
        check(&no_definition, "flat", "image/png", "FLOOR1.png");
    }

    /// A 2x2 `PNG` of one colour, the payload of an embedded-picture lump.
    fn png_bytes() -> Vec<u8> {
        let mut pixels = Vec::new();
        for _pixel in 0_u32..4_u32 {
            pixels.extend_from_slice(&[64_u8, 191, 64, 255]);
        }
        let image = RgbaImage::new(2, 2, pixels).expect("a 2x2 shape is valid");
        encode_png(&image, None).expect("the fixture PNG encodes")
    }

    #[tokio::test]
    async fn an_embedded_picture_lump_reexports_as_a_png() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add(&client, "PHOTO", &png_bytes(), None).await;
        let report = call(&client, "get_asset_url", json!({ "name": "PHOTO" })).await;
        check(&report, "image", "image/png", "PHOTO.png");
        check_dims(&report, 2, 2);
        let body = fetch_asset(url_of(&report)).await;
        assert_eq!(
            read_grab(&body).expect("the published PNG's chunks parse"),
            None,
            "a picture with no drawing offsets publishes none"
        );
        let image = decode_image(&body).expect("the asset is a readable PNG");
        assert_eq!(
            image.get(1, 1),
            Some([64_u8, 191, 64, 255]),
            "the stored colour survives the re-encode"
        );
    }

    #[tokio::test]
    async fn a_sound_lump_publishes_the_wav_preview_sound_makes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add(&client, "DSPISTOL", &dmx_lump(), None).await;
        let report = call(&client, "get_asset_url", json!({ "name": "DSPISTOL" })).await;
        check(&report, "sound", "audio/wav", "DSPISTOL.wav");
        check_no_dims(&report);
        let body = fetch_asset(url_of(&report)).await;
        assert_eq!(&body[..4], b"RIFF", "a RIFF/WAV body");
        assert_eq!(&body[8..12], b"WAVE", "of the WAV form");
    }

    #[tokio::test]
    async fn a_music_lump_publishes_the_midi_preview_music_makes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add(&client, "D_VICTOR", &mus_lump(), None).await;
        let report = call(&client, "get_asset_url", json!({ "name": "D_VICTOR" })).await;
        check(&report, "music", "audio/midi", "D_VICTOR.mid");
        check_no_dims(&report);
        let body = fetch_asset(url_of(&report)).await;
        assert_eq!(&body[..4], b"MThd", "a Standard MIDI File body");
    }

    #[tokio::test]
    async fn the_palette_lump_publishes_its_swatch() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_asset_url", json!({ "name": "PLAYPAL" })).await;
        check(&report, "palette", "image/png", "PLAYPAL.png");
        check_dims(&report, 256, 256);
        let image = fetch_image(url_of(&report)).await;
        assert_eq!(image.get(0, 0), Some([0_u8, 255, 0, 255]), "entry 0");
        assert_eq!(image.get(16, 0), Some([1_u8, 254, 1, 255]), "entry 1");
        assert_eq!(image.get(0, 16), Some([16_u8, 239, 16, 255]), "entry 16");
    }

    #[tokio::test]
    async fn the_colormap_lump_publishes_one_row_per_table() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_asset_url", json!({ "name": "COLORMAP" })).await;
        check(&report, "colormap", "image/png", "COLORMAP.png");
        check_dims(&report, 256, 34);
        let image = fetch_image(url_of(&report)).await;
        assert_eq!(
            image.get(0, 0),
            Some([0_u8, 255, 0, 255]),
            "identity entry 0"
        );
        assert_eq!(
            image.get(17, 33),
            Some([17_u8, 238, 17, 255]),
            "the last table's entry 17"
        );
    }

    /// The top-left pixel of the `FLOOR1` preview published through
    /// palette entry `entry`.
    async fn flat_pixel(client: &Client, entry: usize) -> [u8; 4] {
        let report = call(
            client,
            "get_asset_url",
            json!({ "name": "FLOOR1", "palette": entry }),
        )
        .await;
        check_dims(&report, 64, 64);
        let image = fetch_image(url_of(&report)).await;
        image.get(0, 0).expect("a 64x64 flat has a first pixel")
    }

    #[tokio::test]
    async fn the_palette_argument_reaches_the_colours() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add(&client, "PLAYPAL", &grey_playpal(), None).await;
        assert_eq!(
            flat_pixel(&client, 0).await,
            [0_u8, 0, 0, 255],
            "the plane's index 1 through grey entry 0"
        );
        assert_eq!(
            flat_pixel(&client, 7).await,
            [7_u8, 7, 7, 255],
            "a different entry is a different grey"
        );
    }

    #[tokio::test]
    async fn a_lump_with_nothing_to_show_is_refused_with_the_way_out() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add(
            &client,
            "DEHACKED",
            b"Thing 1
spawnhealth 20
",
            None,
        )
        .await;
        let (code, message) =
            call_err(&client, "get_asset_url", json!({ "name": "DEHACKED" })).await;
        assert_eq!(code, "unsupported", "a text lump: {message}");
        assert!(
            message.contains("dehacked") && message.contains("get_lump format=url"),
            "the message names the kind and the raw way out: {message}"
        );
    }

    #[tokio::test]
    async fn a_name_that_is_neither_a_lump_nor_a_texture_is_not_found() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (code, message) = call_err(&client, "get_asset_url", json!({ "name": "NOWAY" })).await;
        assert_eq!(code, "not_found", "nothing behind the name: {message}");
        assert!(
            message.contains("list_textures"),
            "the message says the definitions were tried too: {message}"
        );
    }

    #[tokio::test]
    async fn the_selectors_scope_and_bad_choices_are_reported() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (scope, scope_text) = call_err(
            &client,
            "get_asset_url",
            json!({ "name": "FLOOR1", "scope": "working" }),
        )
        .await;
        assert_eq!(
            scope, "not_found",
            "the base layer is out of scope: {scope_text}"
        );
        assert!(
            scope_text.contains("not a composite texture either"),
            "the widened message: {scope_text}"
        );
        let (word, word_text) = call_err(
            &client,
            "get_asset_url",
            json!({ "name": "FLOOR1", "prefer": "sprites" }),
        )
        .await;
        assert_eq!(word, "invalid_argument", "no such choice: {word_text}");
        let (entry, entry_text) = call_err(
            &client,
            "get_asset_url",
            json!({ "name": "FLOOR1", "palette": 99_usize }),
        )
        .await;
        assert_eq!(entry, "not_found", "past the last palette: {entry_text}");
        let (namespace, _) = call_err(
            &client,
            "get_asset_url",
            json!({ "name": "FLOOR1", "namespace": "sprites" }),
        )
        .await;
        assert_eq!(namespace, "not_found", "the flat is not a sprite");
        let (scope_word, scope_message) = call_err(
            &client,
            "get_asset_url",
            json!({ "name": "FLOOR1", "scope": "sideways" }),
        )
        .await;
        assert_eq!(
            scope_word, "invalid_argument",
            "a selector that does not parse is not turned into a miss: {scope_message}"
        );
    }

    /// Publishes one resource and requires a stored file on `origin`.
    async fn published(client: &Client, name: &str, origin: &str) -> Value {
        let report = call(client, "get_asset_url", json!({ "name": name })).await;
        let url = url_of(&report);
        assert!(
            url.starts_with(origin),
            "{name} publishes on the session's own origin: {url}"
        );
        let bytes = report["asset"]["size"].as_u64().expect("a stored size");
        assert!(bytes > 0, "{name} stores bytes");
        report
    }

    #[tokio::test]
    async fn every_family_shares_one_asset_server_and_leaves_the_wads_alone() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add(&client, "DSPISTOL", &dmx_lump(), None).await;
        let base = call(&client, "get_asset_server_info", Value::Null).await;
        let origin = base["base_url"].as_str().expect("an origin");
        let names = ["FLOOR1", "WALLP1", "WALL1", "DSPISTOL", "PLAYPAL"];
        let mut urls = vec![
            url_of(&published(&client, "FLOOR1", origin).await).to_owned(),
            url_of(&published(&client, "WALLP1", origin).await).to_owned(),
            url_of(&published(&client, "WALL1", origin).await).to_owned(),
            url_of(&published(&client, "DSPISTOL", origin).await).to_owned(),
            url_of(&published(&client, "PLAYPAL", origin).await).to_owned(),
        ];
        urls.sort();
        let kept = urls.len();
        urls.dedup();
        assert_eq!(urls.len(), kept, "one fresh asset per call");
        let after = call(&client, "get_asset_server_info", Value::Null).await;
        assert_eq!(
            after["stored"].as_u64(),
            Some(u64::try_from(names.len()).expect("a few assets fit")),
            "the store counts them"
        );
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["undo"],
            json!(["add_lump DSPISTOL"]),
            "the lump add is the only edit: five publishes changed nothing"
        );
    }
}
