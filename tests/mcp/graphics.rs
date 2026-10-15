//! The `graphics` tool group as seen by a real MCP client: the flats,
//! patch and sprite listings over the fixture `IWAD` (dimensions from the
//! raw shapes, the `PNG` headers and the `Doom` patch headers, the sprite
//! grouping with its mirrored rotations and its invalid list), the
//! pattern/prefix and paging rules, the merged-view last-wins resolution,
//! and the image tools that publish a flat, a patch or one sprite rotation
//! as a `PNG` or raw `RGBA` asset - with their quantization, `grAb`
//! offsets, palette and scale handling and their error codes.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::codec::{decode_image, encode_png};
    use cyberdemon::gfx::grab::read_grab;
    use cyberdemon::gfx::image::{IndexedImage, RgbaImage};
    use cyberdemon::gfx::patch::{Patch, encode_patch};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, iwad_duplicated};
    use crate::support::{call, call_err, connect, http_get};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// The `{"path": ...}` argument object for `file`.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    /// Load the fixture `IWAD` (flats, patches, sprites, textures) as the
    /// base.
    async fn iwad_client(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(&client, "open_iwad", path_arg(&base)).await;
        client
    }

    /// The space-separated uppercase hex of `data`, for `add_lump` inputs.
    fn hex_of(data: &[u8]) -> String {
        data.iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A flat `PNG` of `width` by `height`, optionally with `grAb`
    /// offsets, for `add_lump` inputs.
    fn png(width: u32, height: u32, offsets: Option<(i32, i32)>) -> String {
        let pixels = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit")
            .checked_mul(4)
            .expect("test pixel bytes fit");
        let img = RgbaImage::new(width, height, vec![0x10_u8; pixels]).expect("test shapes fit");
        hex_of(&encode_png(&img, offsets).expect("test PNGs encode"))
    }

    /// One opaque `RGBA` pixel of `color`.
    fn pixel(color: [u8; 3]) -> [u8; 4] {
        let [red, green, blue] = color;
        [red, green, blue, 255_u8]
    }

    /// A fully opaque `PNG` of one colour with optional `grAb` offsets, as
    /// the hex `add_lump` takes: a port's `PNG` patch or flat.
    fn image_png(width: u32, height: u32, color: [u8; 3], offsets: Option<(i32, i32)>) -> String {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let pixels: Vec<u8> = vec![pixel(color); count].concat();
        let img = RgbaImage::new(width, height, pixels).expect("test shapes fit");
        hex_of(&encode_png(&img, offsets).expect("test PNGs encode"))
    }

    /// A `Doom` patch `width` by 2 whose only opaque pixels are column
    /// `column`, as the hex `add_lump` takes: an asymmetric sprite image,
    /// so a horizontal flip is visible.
    fn stripe_hex(width: u32, column: u32, offsets: (i16, i16)) -> String {
        hex_of(&stripe_bytes(width, column, offsets))
    }

    /// The asset object a report carries under `field`.
    fn asset_of<'report>(report: &'report Value, field: &str) -> &'report Value {
        &report[field]
    }

    /// The `url` of the asset a report carries under `field`.
    fn asset_url(report: &Value, field: &str) -> String {
        asset_of(report, field)["url"]
            .as_str()
            .expect("the report carries an asset url")
            .to_owned()
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

    /// Assert the published shape of a picture report.
    fn check_dims(report: &Value, width: u64, height: u64) {
        assert_eq!(
            (report["width"].as_u64(), report["height"].as_u64()),
            (Some(width), Some(height)),
            "the published dimensions"
        );
    }

    /// Assert the drawing offsets a picture report answers with.
    fn check_offsets(report: &Value, left: i64, top: i64) {
        assert_eq!(
            (
                report["left_offset"].as_i64(),
                report["top_offset"].as_i64()
            ),
            (Some(left), Some(top)),
            "the drawing offsets in published pixels"
        );
    }

    /// Add one working-archive lump under `name` from `hex`.
    async fn add(client: &Client, name: &str, hex: &str) {
        call(client, "add_lump", json!({ "name": name, "hex": hex })).await;
    }

    /// Add one working-archive lump inside a namespace region.
    async fn add_in(client: &Client, name: &str, hex: &str, namespace: &str) {
        call(
            client,
            "add_lump",
            json!({ "name": name, "hex": hex, "namespace": namespace }),
        )
        .await;
    }

    /// The bytes of a `Doom` patch `width` by 2 whose only opaque pixels
    /// are column `column`, for a file fixture.
    fn stripe_bytes(width: u32, column: u32, offsets: (i16, i16)) -> Vec<u8> {
        let (left, top) = offsets;
        let columns = usize::try_from(width).expect("test width");
        let total = columns.checked_mul(2).expect("test pixels fit");
        let mut indices = vec![0_u8; total];
        let mut opaque = vec![false; total];
        for row in 0_usize..2 {
            let slot = row
                .checked_mul(columns)
                .and_then(|start| start.checked_add(usize::try_from(column).expect("test column")))
                .expect("test index fits");
            indices[slot] = 5_u8;
            opaque[slot] = true;
        }
        let image = IndexedImage::new(width, 2, indices, opaque).expect("test shapes fit");
        encode_patch(&Patch {
            left_offset: left,
            top_offset: top,
            image,
        })
        .expect("test patches encode")
    }

    /// Add `TROOA2A8` to the working archive's sprite region as an
    /// asymmetric 4x2 patch (opaque column 0, offsets `(1, 2)`),
    /// overriding the fixture's plain copy so a mirrored rotation is
    /// visible in the published pixels.
    async fn add_stripe_mirror(client: &Client) {
        add_in(
            client,
            "TROOA2A8",
            &hex_of(&stripe_bytes(4, 0, (1_i16, 2_i16))),
            "sprites",
        )
        .await;
    }

    /// The `name` fields of a report list, in order.
    fn listed(report: &Value, key: &str) -> Vec<String> {
        report[key]
            .as_array()
            .expect("the list is an array")
            .iter()
            .map(|row| row["name"].as_str().expect("names are strings").to_owned())
            .collect()
    }

    /// The sprite-group `sprite` fields of a report, in order.
    fn grouped(report: &Value) -> Vec<String> {
        report["sprites"]
            .as_array()
            .expect("sprites is an array")
            .iter()
            .map(|row| {
                let text = row["sprite"].as_str().expect("sprite names are strings");
                text.to_owned()
            })
            .collect()
    }

    /// Assert the shared shape of a listing `row`: name, size and source.
    fn check_row(row: &Value, name: &str, size: u64, source: &Value) {
        assert_eq!(row["name"], json!(name), "row name");
        assert_eq!(row["size"], json!(size), "{name} size");
        assert_eq!(&row["source"], source, "{name} source");
    }

    /// Assert that one listing tool refuses `limit` with the documented
    /// range message.
    async fn refuse_limit(client: &Client, tool: &str, limit: u64) {
        let (code, message) = call_err(client, tool, json!({ "limit": limit })).await;
        assert_eq!(code, "invalid_argument", "{tool} limit {limit} code");
        assert!(
            message.contains("between 1 and 2000"),
            "{tool}: the message states the range: {message}"
        );
    }

    #[tokio::test]
    async fn an_empty_session_lists_no_graphics() {
        let client = connect().await;
        let flats = call(&client, "list_flats", json!({})).await;
        assert_eq!(flats["total"], json!(0_u64), "no flats");
        assert_eq!(flats["flats"], json!([]), "an empty list, not an error");
        let patches = call(&client, "list_patches", json!({})).await;
        assert_eq!(patches["total"], json!(0_u64), "no patches");
        assert_eq!(patches["patches"], json!([]), "an empty list, not an error");
        let sprites = call(&client, "list_sprites", json!({})).await;
        assert_eq!(sprites["total"], json!(0_u64), "no sprites");
        assert_eq!(sprites["sprites"], json!([]), "an empty list, not an error");
        assert_eq!(sprites["invalid"], json!([]), "and nothing malformed");
    }

    #[tokio::test]
    async fn list_flats_reports_raw_and_png_flats() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "list_flats", json!({})).await;
        assert_eq!(report["total"], json!(2_u64), "FLOOR1 and PNGFLAT");
        assert_eq!(
            listed(&report, "flats"),
            vec!["FLOOR1", "PNGFLAT"],
            "markers excluded"
        );
        let floor = &report["flats"][0];
        check_row(floor, "FLOOR1", 4_096_u64, &json!({"layer": 0_u64}));
        assert_eq!(
            (floor["width"].as_u64(), floor["height"].as_u64()),
            (Some(64), Some(64)),
            "the 4096-byte shape"
        );
        assert_eq!(floor["format"], json!("raw"), "indexed bytes");
        let pngflat = &report["flats"][1];
        assert!(
            pngflat["size"].as_u64().expect("a size") < 4_096,
            "a compressed PNG"
        );
        assert_eq!(pngflat["format"], json!("png"), "the signature decides");
        assert_eq!(
            (pngflat["width"].as_u64(), pngflat["height"].as_u64()),
            (Some(64), Some(64)),
            "dimensions from the IHDR"
        );
        let filtered = call(&client, "list_flats", json!({ "pattern": "p?gflat" })).await;
        assert_eq!(filtered["total"], json!(1_u64), "case-insensitive glob");
        assert_eq!(listed(&filtered, "flats"), vec!["PNGFLAT"]);
    }

    #[tokio::test]
    async fn list_flats_resolves_duplicates_and_unknown_sizes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad_duplicated(dir.path());
        call(&client, "open_iwad", path_arg(&base)).await;
        let report = call(&client, "list_flats", json!({})).await;
        assert_eq!(
            listed(&report, "flats"),
            vec!["FLOOR1", "PNGFLAT"],
            "one row per name, the last copy wins"
        );
        let odd_hex = hex_of(&[7_u8; 100]);
        call(
            &client,
            "add_lump",
            json!({ "name": "ODDFLET", "hex": odd_hex, "namespace": "flats" }),
        )
        .await;
        let grown = call(&client, "list_flats", json!({ "pattern": "ODD*" })).await;
        assert_eq!(
            grown["total"],
            json!(1_u64),
            "the new flat is in the region"
        );
        let odd = &grown["flats"][0];
        check_row(odd, "ODDFLET", 100_u64, &json!("working"));
        assert!(
            odd["width"].is_null() && odd["height"].is_null(),
            "100 bytes imply no shape"
        );
        assert_eq!(odd["format"], json!("raw"), "not a PNG");
    }

    #[tokio::test]
    async fn the_listing_tools_validate_their_page_bounds() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        refuse_page_bounds(&client).await;
        let paged = call(
            &client,
            "list_flats",
            json!({ "offset": 1_u64, "limit": 1_u64 }),
        )
        .await;
        assert_eq!(paged["total"], json!(2_u64), "total counts all matches");
        assert_eq!(listed(&paged, "flats"), vec!["PNGFLAT"], "the second row");
        let patches = call(
            &client,
            "list_patches",
            json!({ "offset": 1_u64, "limit": 1_u64 }),
        )
        .await;
        assert_eq!(
            listed(&patches, "patches"),
            vec!["STEPP1"],
            "one row per page"
        );
        let sprites = call(
            &client,
            "list_sprites",
            json!({ "offset": 1_u64, "limit": 1_u64 }),
        )
        .await;
        assert_eq!(grouped(&sprites), vec!["TROO"], "paging is over groups");
    }

    #[tokio::test]
    async fn list_patches_reports_the_region_with_parsed_headers() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "list_patches", json!({})).await;
        assert_eq!(report["total"], json!(2_u64), "the two region patches");
        assert_eq!(
            listed(&report, "patches"),
            vec!["WALLP1", "STEPP1"],
            "directory order"
        );
        let wall = &report["patches"][0];
        check_row(
            wall,
            "WALLP1",
            wall["size"].as_u64().expect("a size"),
            &json!({"layer": 0_u64}),
        );
        assert_eq!(wall["format"], json!("doom"), "the header reads");
        assert_eq!(
            (wall["width"].as_u64(), wall["height"].as_u64()),
            (Some(16), Some(16)),
            "16x16 fixture"
        );
        assert_eq!(
            (wall["left_offset"].as_i64(), wall["top_offset"].as_i64()),
            (Some(-2), Some(-14)),
            "the fixture's header offsets"
        );
        assert_eq!(wall["in_pnames"], json!(true), "named by PNAMES");
        assert_eq!(wall["namespace"], json!("patches"), "inside P_START");
        let step = &report["patches"][1];
        assert_eq!(
            (step["width"].as_u64(), step["height"].as_u64()),
            (Some(16), Some(8)),
            "16x8 fixture"
        );
        let filtered = call(&client, "list_patches", json!({ "pattern": "wall*" })).await;
        assert_eq!(filtered["total"], json!(1_u64), "the glob applies");
        assert_eq!(listed(&filtered, "patches"), vec!["WALLP1"]);
    }

    #[tokio::test]
    async fn list_patches_names_named_globals_and_reads_png_shapes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add_global_lumps(&client).await;
        let loose = call(&client, "list_patches", json!({})).await;
        assert_eq!(loose["total"], json!(2_u64), "un-named globals stay out");
        name_globals(&client).await;
        let report = call(&client, "list_patches", json!({})).await;
        assert_eq!(
            report["total"],
            json!(5_u64),
            "the region pair plus the named globals; FLOOR1 (flats) and TROOA1 (sprites) stay out"
        );
        let pngpat = &report["patches"][2];
        check_row(
            pngpat,
            "PNGPAT",
            pngpat["size"].as_u64().expect("a size"),
            &json!("working"),
        );
        assert_eq!(pngpat["format"], json!("png"), "the signature decides");
        assert_eq!(
            (pngpat["width"].as_u64(), pngpat["height"].as_u64()),
            (Some(4), Some(2)),
            "dimensions from the IHDR"
        );
        assert_eq!(
            (
                pngpat["left_offset"].as_i64(),
                pngpat["top_offset"].as_i64()
            ),
            (Some(3), Some(-4)),
            "offsets from grAb"
        );
        assert_eq!(pngpat["namespace"], Value::Null, "a global lump");
        assert_eq!(pngpat["in_pnames"], json!(true), "just named");
    }

    #[tokio::test]
    async fn list_patches_reports_bare_pngs_unknowns_and_in_pnames_only() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add_named_globals(&client).await;
        let report = call(&client, "list_patches", json!({})).await;
        let bare = &report["patches"][3];
        assert_eq!(bare["name"], json!("PNGPA2"), "the PNG without grAb");
        assert!(
            bare["left_offset"].is_null() && bare["top_offset"].is_null(),
            "no offsets to report"
        );
        assert_eq!(
            (bare["width"].as_u64(), bare["height"].as_u64()),
            (Some(2), Some(3)),
            "still measured"
        );
        let junk = &report["patches"][4];
        assert_eq!(
            junk["format"],
            json!("unknown"),
            "40 x's are neither format"
        );
        assert!(
            junk["width"].is_null() && junk["left_offset"].is_null(),
            "nothing measured"
        );
        let only_named = call(&client, "list_patches", json!({ "in_pnames_only": true })).await;
        assert_eq!(only_named["total"], json!(5_u64), "everything named now");
    }

    #[tokio::test]
    async fn list_patches_tracks_pnames_membership_of_region_lumps() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add_named_globals(&client).await;
        call(
            &client,
            "edit_pnames",
            json!({ "remove": ["WALLP1"], "force": true }),
        )
        .await;
        let after = call(&client, "list_patches", json!({ "in_pnames_only": true })).await;
        assert_eq!(
            after["total"],
            json!(4_u64),
            "the un-named region lump drops out"
        );
        let all = call(&client, "list_patches", json!({})).await;
        assert_eq!(all["total"], json!(5_u64), "WALLP1 is still a region lump");
        let wall = &all["patches"][0];
        assert_eq!(wall["in_pnames"], json!(false), "its name left PNAMES");
    }

    /// Add a `grAb`-offset `PNG` patch, a plain `PNG` patch and a junk
    /// global to the working archive.
    async fn add_global_lumps(client: &Client) {
        call(
            client,
            "add_lump",
            json!({ "name": "PNGPAT", "hex": png(4, 2, Some((3_i32, -4_i32))) }),
        )
        .await;
        call(
            client,
            "add_lump",
            json!({ "name": "PNGPA2", "hex": png(2, 3, None) }),
        )
        .await;
        call(
            client,
            "add_lump",
            json!({ "name": "JUNKPL", "hex": hex_of(&[b'x'; 40]) }),
        )
        .await;
    }

    /// Name the added globals in `PNAMES`, `FLOOR1` and one sprite lump
    /// included: the region rules must keep those two out of the list.
    async fn name_globals(client: &Client) {
        call(
            client,
            "edit_pnames",
            json!({ "add": ["PNGPAT", "PNGPA2", "JUNKPL", "FLOOR1", "TROOA1"] }),
        )
        .await;
    }

    /// Both steps: the three global lumps added and named.
    async fn add_named_globals(client: &Client) {
        add_global_lumps(client).await;
        name_globals(client).await;
    }

    #[tokio::test]
    async fn list_sprites_groups_the_fixture_frames_with_mirrors() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "list_sprites", json!({})).await;
        assert_eq!(report["total"], json!(2_u64), "PLAY and TROO");
        assert_eq!(grouped(&report), vec!["PLAY", "TROO"], "grouped by prefix");
        assert_eq!(
            report["invalid"],
            json!(["BADNAME"]),
            "the odd name is reported"
        );
        let play = &report["sprites"][0];
        assert_eq!(play["frames"][0]["frame"], json!("A"), "one frame");
        assert_eq!(
            play["frames"][0]["rotations"],
            json!([{"lump": "PLAYA0", "mirrored": false, "rotation": 0_u64}]),
            "rotation 0 serves every angle"
        );
        let troo = &report["sprites"][1];
        assert_eq!(
            troo["frames"].as_array().expect("frames").len(),
            1,
            "frame A only"
        );
        assert_eq!(
            troo["frames"][0]["rotations"],
            json!([
                {"lump": "TROOA1", "mirrored": false, "rotation": 1_u64},
                {"lump": "TROOA2A8", "mirrored": false, "rotation": 2_u64},
                {"lump": "TROOA3A7", "mirrored": false, "rotation": 3_u64},
                {"lump": "TROOA4A6", "mirrored": false, "rotation": 4_u64},
                {"lump": "TROOA5", "mirrored": false, "rotation": 5_u64},
                {"lump": "TROOA4A6", "mirrored": true, "rotation": 6_u64},
                {"lump": "TROOA3A7", "mirrored": true, "rotation": 7_u64},
                {"lump": "TROOA2A8", "mirrored": true, "rotation": 8_u64},
            ]),
            "five lumps serve the eight rotations, the pair entries flipped"
        );
    }

    #[tokio::test]
    async fn list_sprites_filters_by_prefix_and_validates_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let one = call(&client, "list_sprites", json!({ "prefix": "troo" })).await;
        assert_eq!(one["total"], json!(1_u64), "one group, case-insensitive");
        assert_eq!(grouped(&one), vec!["TROO"]);
        assert_eq!(one["invalid"], json!([]), "BADNAME is not a TROO");
        let odd = call(&client, "list_sprites", json!({ "prefix": "BADN" })).await;
        assert_eq!(odd["total"], json!(0_u64), "no sprite named BADN");
        assert_eq!(
            odd["invalid"],
            json!(["BADNAME"]),
            "its malformed names are kept"
        );
        refuse_bad_prefixes(&client).await;
        let miss = call(&client, "list_sprites", json!({ "prefix": "ZZZZ" })).await;
        assert_eq!(miss["total"], json!(0_u64), "a prefix with no lumps");
        assert_eq!(miss["sprites"], json!([]), "an empty list, not an error");
    }

    /// Assert that `list_sprites` rejects every malformed `prefix` form.
    async fn refuse_bad_prefixes(client: &Client) {
        for bad in ["", "POSSS", "PO!S", "P1"] {
            refuse_prefix(client, bad).await;
        }
    }

    /// Assert that all three listing tools refuse the out-of-range page
    /// limits with the documented range message.
    async fn refuse_page_bounds(client: &Client) {
        for tool in ["list_flats", "list_patches", "list_sprites"] {
            refuse_limit(client, tool, 0_u64).await;
            refuse_limit(client, tool, 2001_u64).await;
        }
    }

    /// Assert that `list_sprites` refuses a malformed `prefix`.
    async fn refuse_prefix(client: &Client, bad: &str) {
        let (code, message) = call_err(client, "list_sprites", json!({ "prefix": bad })).await;
        assert_eq!(code, "invalid_argument", "prefix {bad:?} code");
        assert!(
            message.contains("four"),
            "the message states the rule: {message}"
        );
    }

    #[tokio::test]
    async fn listings_follow_the_merged_view() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        call(
            &client,
            "replace_lump",
            json!({ "name": "PNGFLAT", "hex": png(8, 8, None) }),
        )
        .await;
        let flats = call(&client, "list_flats", json!({ "pattern": "PNGFLAT" })).await;
        assert_eq!(flats["total"], json!(1_u64), "still one row per name");
        assert_eq!(
            flats["flats"][0]["source"],
            json!("working"),
            "the working override wins the merge"
        );
        assert_eq!(
            (
                flats["flats"][0]["width"].as_u64(),
                flats["flats"][0]["height"].as_u64()
            ),
            (Some(8), Some(8)),
            "the override's IHDR is what is listed"
        );
    }

    #[tokio::test]
    async fn get_flat_publishes_a_raw_flat_as_a_png_asset() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_flat", json!({ "name": "floor1" })).await;
        assert_eq!(report["name"], json!("FLOOR1"), "the name is uppercased");
        check_dims(&report, 64_u64, 64_u64);
        assert!(
            report["left_offset"].is_null() && report["top_offset"].is_null(),
            "a flat stores no drawing offsets"
        );
        let asset = asset_of(&report, "asset");
        assert_eq!(asset["filename"], json!("FLOOR1.png"), "the published name");
        assert_eq!(asset["mime"], json!("image/png"), "the media type");
        assert!(
            asset["size"].as_u64().expect("a size") > 0,
            "bytes were stored"
        );
        let url = asset_url(&report, "asset");
        let image = fetch_image(&url).await;
        assert_eq!(
            image.get(0, 0),
            Some([1_u8, 254, 1, 255]),
            "every pixel is palette colour 1"
        );
        assert_eq!(
            image.get(63, 63),
            Some([1_u8, 254, 1, 255]),
            "the far corner too"
        );
        assert_eq!(
            read_grab(&fetch_asset(&url).await).expect("a chunked PNG"),
            None,
            "a flat is published without a grAb chunk"
        );
    }

    #[tokio::test]
    async fn get_flat_quantizes_a_png_flat_to_palette_zero() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_flat", json!({ "name": "PNGFLAT" })).await;
        check_dims(&report, 64_u64, 64_u64);
        assert_eq!(report["asset"]["mime"], json!("image/png"), "a PNG asset");
        let image = fetch_image(&asset_url(&report, "asset")).await;
        assert_eq!(
            image.get(32, 32),
            Some([64_u8, 191, 64, 255]),
            "the PNG colour is quantized to index 64 and recoloured from it"
        );
    }

    /// `width * height * 4`, the `RGBA8` byte count a picture's asset
    /// carries.
    const fn rgba_len(width: usize, height: usize) -> usize {
        width.saturating_mul(height).saturating_mul(4)
    }

    #[tokio::test]
    async fn get_flat_scales_the_picture_and_repaints_it_from_another_palette() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let plain = call(&client, "get_flat", json!({ "name": "FLOOR1" })).await;
        let doubled = call(
            &client,
            "get_flat",
            json!({ "name": "FLOOR1", "scale": 2_u64 }),
        )
        .await;
        check_dims(&doubled, 128_u64, 128_u64);
        assert_eq!(
            doubled["asset"]["filename"],
            json!("FLOOR1.png"),
            "the same name, another asset"
        );
        let image = fetch_image(&asset_url(&doubled, "asset")).await;
        assert_eq!(
            image.get(1, 1),
            Some([1_u8, 254, 1, 255]),
            "nearest-neighbour upscaling keeps the colour"
        );
        let coloured = call(
            &client,
            "get_flat",
            json!({ "name": "FLOOR1", "palette": 13_u64 }),
        )
        .await;
        assert_eq!(
            coloured["name"],
            json!("FLOOR1"),
            "the palette entry is not a new image"
        );
        assert_eq!(
            fetch_asset(&asset_url(&plain, "asset")).await,
            fetch_asset(&asset_url(&coloured, "asset")).await,
            "the fixture palettes are copies, so palette 13 paints the same picture"
        );
    }

    #[tokio::test]
    async fn get_flat_publishes_raw_rgba_bytes_at_the_scaled_size() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let rgba = call(
            &client,
            "get_flat",
            json!({ "name": "FLOOR1", "format": "rgba", "scale": 2_u64 }),
        )
        .await;
        assert_eq!(
            rgba["asset"]["filename"],
            json!("FLOOR1.rgba"),
            "the raw asset name"
        );
        assert_eq!(
            rgba["asset"]["mime"],
            json!("application/octet-stream"),
            "the raw asset media type"
        );
        check_dims(&rgba, 128_u64, 128_u64);
        let bytes = fetch_asset(&asset_url(&rgba, "asset")).await;
        assert_eq!(bytes.len(), rgba_len(128, 128), "row-major RGBA8 bytes");
        assert_eq!(
            bytes.first().copied(),
            Some(1_u8),
            "the first channel is palette colour 1"
        );
    }

    /// Assert the image tools refuse `args` with the error `code` and a
    /// `hint` in the message.
    async fn refuse(client: &Client, tool: &str, args: Value, expect: (&str, &str)) {
        let (code, hint) = expect;
        let (got, message) = call_err(client, tool, args).await;
        assert_eq!(got, code, "{tool} code: {message}");
        assert!(
            message.contains(hint),
            "{tool} should mention {hint:?}: {message}"
        );
    }

    /// One refusal a picture tool answers with: `(tool, arguments, code,
    /// message hint)`.
    type Refusal = (&'static str, Value, &'static str, &'static str);

    /// Assert each `(tool, args, code, hint)` refusal in `cases`.
    async fn refuse_all(client: &Client, cases: &[Refusal]) {
        for case in cases {
            refuse(client, case.0, case.1.clone(), (case.2, case.3)).await;
        }
    }

    /// The refusals for pictures that are missing or that no decoder reads.
    fn missing_picture_cases() -> Vec<Refusal> {
        vec![
            (
                "get_flat",
                json!({ "name": "NOPE" }),
                "not_found",
                "list_flats",
            ),
            (
                "get_patch",
                json!({ "name": "NOPE" }),
                "not_found",
                "list_patches",
            ),
            (
                "get_sprite",
                json!({ "sprite": "NOPE" }),
                "not_found",
                "list_sprites",
            ),
            (
                "get_sprite",
                json!({ "name": "NOPEA1" }),
                "not_found",
                "list_sprites",
            ),
            (
                "get_patch",
                json!({ "name": "ENDOOM" }),
                "format_error",
                "patch",
            ),
            (
                "get_flat",
                json!({ "name": "ODDFLAT" }),
                "format_error",
                "flat",
            ),
            (
                "get_sprite",
                json!({ "name": "BADNAME" }),
                "format_error",
                "end of data",
            ),
            (
                "decode_patch",
                json!({ "name": "PNGFLAT" }),
                "format_error",
                "patch",
            ),
            (
                "get_flat",
                json!({ "name": "FLOOR1", "palette": 20_u64 }),
                "not_found",
                "PLAYPAL",
            ),
        ]
    }

    #[tokio::test]
    async fn the_image_tools_report_missing_and_undecodable_pictures() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add_in(&client, "ODDFLAT", &hex_of(&[7_u8; 100]), "flats").await;
        refuse_all(&client, &missing_picture_cases()).await;
        let empty = connect().await;
        refuse(
            &empty,
            "get_flat",
            json!({ "name": "FLOOR1" }),
            ("not_found", "list_flats"),
        )
        .await;
    }

    /// The refusals for malformed arguments of the picture tools.
    fn bad_argument_cases() -> Vec<Refusal> {
        vec![
            (
                "get_sprite",
                json!({ "name": "TROOA1", "sprite": "TROO" }),
                "invalid_argument",
                "not both",
            ),
            (
                "get_sprite",
                json!({ "format": "png" }),
                "invalid_argument",
                "needs a sprite",
            ),
            (
                "get_flat",
                json!({ "name": "FLOOR1", "format": "jpg" }),
                "invalid_argument",
                "rgba",
            ),
            (
                "get_flat",
                json!({ "name": "TOOLONGNAME" }),
                "invalid_argument",
                "lump name",
            ),
            (
                "get_patch",
                json!({ "name": "WALLP1", "scale": 0_u64 }),
                "invalid_argument",
                "scale",
            ),
            (
                "get_patch",
                json!({ "name": "WALLP1", "scale": 9_u64 }),
                "invalid_argument",
                "scale",
            ),
            (
                "get_sprite",
                json!({ "sprite": "TROO", "frame": "1" }),
                "invalid_argument",
                "frame",
            ),
            (
                "get_sprite",
                json!({ "sprite": "TROO", "rotation": 9_u64 }),
                "invalid_argument",
                "rotation",
            ),
            (
                "get_sprite",
                json!({ "sprite": "TROO1" }),
                "invalid_argument",
                "four",
            ),
        ]
    }

    #[tokio::test]
    async fn the_image_tools_validate_their_arguments() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        refuse_all(&client, &bad_argument_cases()).await;
    }

    #[tokio::test]
    async fn get_patch_publishes_a_doom_patch_with_grab_offsets() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "get_patch", json!({ "name": "wallp1" })).await;
        assert_eq!(report["name"], json!("WALLP1"), "the lump it read");
        check_dims(&report, 16_u64, 16_u64);
        check_offsets(&report, -2_i64, -14_i64);
        let url = asset_url(&report, "asset");
        let bytes = fetch_asset(&url).await;
        assert_eq!(
            read_grab(&bytes).expect("a chunked PNG"),
            Some((-2_i32, -14_i32)),
            "the drawing offsets survive in a grAb chunk"
        );
        let image = decode_image(&bytes).expect("the asset decodes");
        assert_eq!(
            image.get(0, 0),
            Some([17_u8, 238, 17, 255]),
            "the fill index 0x11 in palette colour 17"
        );
        let doubled = call(
            &client,
            "get_patch",
            json!({ "name": "WALLP1", "scale": 2_u64 }),
        )
        .await;
        check_dims(&doubled, 32_u64, 32_u64);
        check_offsets(&doubled, -4_i64, -28_i64);
        assert_eq!(
            read_grab(&fetch_asset(&asset_url(&doubled, "asset")).await).expect("a chunked PNG"),
            Some((-4_i32, -28_i32)),
            "the chunk scales along with the picture"
        );
        let rgba = call(
            &client,
            "get_patch",
            json!({ "name": "WALLP1", "format": "rgba" }),
        )
        .await;
        assert_eq!(
            rgba["asset"]["filename"],
            json!("WALLP1.rgba"),
            "the raw asset"
        );
        assert_eq!(
            fetch_asset(&asset_url(&rgba, "asset")).await.len(),
            16_usize
                .checked_mul(16)
                .expect("16x16 fits")
                .checked_mul(4)
                .expect("RGBA8 fits"),
            "row-major RGBA8 bytes"
        );
    }

    #[tokio::test]
    async fn get_patch_resolves_a_png_lump_and_a_working_override() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add(
            &client,
            "PNGPAT",
            &image_png(3, 2, [17_u8, 238, 17], Some((3_i32, -4_i32))),
        )
        .await;
        let report = call(&client, "get_patch", json!({ "name": "PNGPAT" })).await;
        check_dims(&report, 3_u64, 2_u64);
        check_offsets(&report, 3_i64, -4_i64);
        let image = fetch_image(&asset_url(&report, "asset")).await;
        assert_eq!(
            image.get(0, 0),
            Some([17_u8, 238, 17, 255]),
            "a PNG patch is quantized to palette 0 and recoloured from it"
        );
        let loose = call(&client, "list_patches", json!({ "pattern": "PNGPAT" })).await;
        assert_eq!(
            loose["total"],
            json!(0_u64),
            "an unnamed global is no patch"
        );
        call(
            &client,
            "replace_lump",
            json!({ "name": "WALLP1", "hex": stripe_hex(6, 1, (0_i16, 0_i16)) }),
        )
        .await;
        let override_report = call(&client, "get_patch", json!({ "name": "WALLP1" })).await;
        check_dims(&override_report, 6_u64, 2_u64);
        assert_eq!(
            override_report["name"],
            json!("WALLP1"),
            "the working copy wins the merged view"
        );
    }

    #[tokio::test]
    async fn get_sprite_serves_a_named_lump_as_stored() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add_stripe_mirror(&client).await;
        let report = call(&client, "get_sprite", json!({ "name": "trooa1" })).await;
        assert_eq!(report["name"], json!("TROOA1"), "the requested name");
        assert_eq!(
            report["lump"],
            json!("TROOA1"),
            "and the lump that served it"
        );
        assert_eq!(report["mirrored"], json!(false), "served as stored");
        check_dims(&report, 8_u64, 8_u64);
        check_offsets(&report, 0_i64, 0_i64);
        let pair = call(&client, "get_sprite", json!({ "name": "TROOA2A8" })).await;
        assert_eq!(pair["lump"], json!("TROOA2A8"), "the whole name is read");
        assert_eq!(pair["mirrored"], json!(false), "as stored, not flipped");
        check_dims(&pair, 4_u64, 2_u64);
        check_offsets(&pair, 1_i64, 2_i64);
        let image = fetch_image(&asset_url(&pair, "asset")).await;
        assert_eq!(
            image.get(0, 0).map(|[_, _, _, alpha]| alpha),
            Some(255),
            "the stored image keeps its opaque column at the left"
        );
        assert_eq!(
            read_grab(&fetch_asset(&asset_url(&pair, "asset")).await).expect("a chunked PNG"),
            Some((1_i32, 2_i32)),
            "grAb carries the stored offsets"
        );
    }

    #[tokio::test]
    async fn get_sprite_serves_a_mirrored_pair_rotation_flipped() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        add_stripe_mirror(&client).await;
        let direct = call(
            &client,
            "get_sprite",
            json!({ "sprite": "troo", "frame": "a", "rotation": 1_u64 }),
        )
        .await;
        assert_eq!(direct["lump"], json!("TROOA1"), "case-insensitive parts");
        assert_eq!(
            direct["mirrored"],
            json!(false),
            "the rotation has its own lump"
        );
        check_dims(&direct, 8_u64, 8_u64);
        let eight = call(
            &client,
            "get_sprite",
            json!({ "sprite": "TROO", "frame": "A", "rotation": 8_u64 }),
        )
        .await;
        assert_eq!(
            eight["name"],
            json!("TROOA8"),
            "the rotation that was asked for"
        );
        assert_eq!(
            eight["lump"],
            json!("TROOA2A8"),
            "served by the mirror pair"
        );
        assert_eq!(eight["mirrored"], json!(true), "flipped horizontally");
        check_dims(&eight, 4_u64, 2_u64);
        check_offsets(&eight, 3_i64, 2_i64);
        let url = asset_url(&eight, "asset");
        let image = fetch_image(&url).await;
        assert_eq!(
            image.get(3, 0).map(|[_, _, _, alpha]| alpha),
            Some(255),
            "the opaque column moved to the right edge"
        );
        assert_eq!(
            image.get(0, 1).map(|[_, _, _, alpha]| alpha),
            Some(0),
            "and the left edge is a hole"
        );
        assert_eq!(
            read_grab(&fetch_asset(&url).await).expect("a chunked PNG"),
            Some((3_i32, 2_i32)),
            "the mirrored left offset is what gets stored"
        );
    }

    #[tokio::test]
    async fn get_sprite_falls_back_to_the_all_angles_image() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let angles = call(
            &client,
            "get_sprite",
            json!({ "sprite": "PLAY", "rotation": 5_u64 }),
        )
        .await;
        assert_eq!(
            angles["lump"],
            json!("PLAYA0"),
            "the all-angles image serves any rotation"
        );
        assert_eq!(angles["mirrored"], json!(false), "used as stored");
        check_dims(&angles, 16_u64, 16_u64);
        assert_eq!(
            angles["name"],
            json!("PLAYA5"),
            "the frame defaults to A and the report keeps the request"
        );
        let zero = call(
            &client,
            "get_sprite",
            json!({ "sprite": "PLAY", "rotation": 0_u64, "scale": 3_u64 }),
        )
        .await;
        check_dims(&zero, 48_u64, 48_u64);
        assert_eq!(zero["lump"], json!("PLAYA0"), "rotation 0 is itself");
    }

    #[tokio::test]
    async fn decode_patch_publishes_both_encodings_of_a_lump() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let report = call(&client, "decode_patch", json!({ "name": "stepp1" })).await;
        assert_eq!(report["name"], json!("STEPP1"), "the lump it read");
        check_dims(&report, 16_u64, 8_u64);
        check_offsets(&report, 0_i64, 0_i64);
        assert_eq!(report["palette"], json!(0_u64), "the default palette");
        let png_url = asset_url(&report, "png");
        assert_eq!(
            report["png"]["filename"],
            json!("STEPP1.png"),
            "the PNG asset name"
        );
        assert_eq!(
            report["rgba"]["filename"],
            json!("STEPP1.rgba"),
            "the raw asset name"
        );
        let bytes = fetch_asset(&png_url).await;
        assert_eq!(
            read_grab(&bytes).expect("a chunked PNG"),
            Some((0_i32, 0_i32)),
            "the offsets are published even when they are zero"
        );
        let image = decode_image(&bytes).expect("the PNG decodes");
        assert_eq!(
            image.get(15, 7),
            Some([34_u8, 221, 34, 255]),
            "the fill index 0x22 in colour"
        );
        let raw = fetch_asset(&asset_url(&report, "rgba")).await;
        assert_eq!(
            raw.len(),
            16_usize
                .checked_mul(8)
                .expect("16x8 fits")
                .checked_mul(4)
                .expect("RGBA8 fits"),
            "width*height*4 raw bytes"
        );
        assert_eq!(&raw[..4], &[34_u8, 221, 34, 255], "row-major RGBA8");
        let coloured = call(
            &client,
            "decode_patch",
            json!({ "name": "STEPP1", "palette": 13_u64 }),
        )
        .await;
        assert_eq!(
            coloured["palette"],
            json!(13_u64),
            "the palette entry the caller asked for"
        );
    }

    #[tokio::test]
    async fn decode_patch_reads_a_file_and_refuses_bad_inputs() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let patch = dir.path().join("MYPATCH.dat");
        std::fs::write(&patch, stripe_bytes(5, 4, (-1_i16, -2_i16))).expect("write the file");
        let report = call(
            &client,
            "decode_patch",
            json!({ "source": patch.display().to_string() }),
        )
        .await;
        assert_eq!(
            report["name"],
            json!("MYPATCH"),
            "the file stem names the patch"
        );
        check_dims(&report, 5_u64, 2_u64);
        check_offsets(&report, -1_i64, -2_i64);
        let junk = dir.path().join("notes.txt");
        std::fs::write(&junk, b"this is a text file, not a patch").expect("write the file");
        refuse_all(
            &client,
            &[
                (
                    "decode_patch",
                    json!({ "source": junk.display().to_string() }),
                    "format_error",
                    "patch",
                ),
                (
                    "decode_patch",
                    json!({ "name": "WALLP1", "source": junk.display().to_string() }),
                    "invalid_argument",
                    "not both",
                ),
                (
                    "decode_patch",
                    json!({ "palette": 0_u64 }),
                    "invalid_argument",
                    "needs a patch",
                ),
                (
                    "decode_patch",
                    json!({ "name": "NOPE" }),
                    "not_found",
                    "NOPE",
                ),
                (
                    "decode_patch",
                    json!({ "source": dir.path().join("absent.dat").display().to_string() }),
                    "not_found",
                    "absent.dat",
                ),
            ],
        )
        .await;
    }
}
