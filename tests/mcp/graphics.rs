//! The `graphics` tool group as seen by a real MCP client: the flats,
//! patch and sprite listings over the fixture `IWAD` (dimensions from the
//! raw shapes, the `PNG` headers and the `Doom` patch headers, the sprite
//! grouping with its mirrored rotations and its invalid list), the
//! pattern/prefix and paging rules, and the merged-view last-wins
//! resolution.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::codec::encode_png;
    use cyberdemon::gfx::image::RgbaImage;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, iwad_duplicated};
    use crate::support::{call, call_err, connect};

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
            (Some(0), Some(0)),
            "the fixture's zero offsets"
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
}
