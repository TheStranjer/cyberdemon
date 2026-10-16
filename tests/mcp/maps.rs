//! The `maps` tool group as seen by a real MCP client: listing the map
//! blocks of every scope with formats, namespaces and node presence,
//! reading one block's raw lump set (with optionally published bytes),
//! the cheap per-map overview, the typed whole-map graph with resolved
//! references (inline or published past the inline limit, dangling
//! references reported as problems), the active-map selection the map
//! tools' optional `map` parameter defaults to, and the management tools
//! that build, clone, rename and delete whole blocks of the working
//! archive (`create_map`, `copy_map`, `rename_map`, `remove_map`) with
//! their undo steps.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::wad::LumpName;
    use cyberdemon::wad::map::doom::write_doom_map;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, iwad_broken_map, pwad, pwad_map_formats, two_room_map};
    use crate::support::{call, call_err, connect, http_get};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// The ten vanilla member lumps of a built binary `Doom` map block.
    const DOOM_MEMBERS: [&str; 10] = [
        "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS",
        "REJECT", "BLOCKMAP",
    ];

    /// Load the `IWAD` fixture as layer 0 and stack the `PWAD` fixture as
    /// layer 1: two binary `Doom` maps from two archives.
    async fn iwad_plus_pwad(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        let extra = pwad(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        call(
            &client,
            "merge_wad",
            json!({ "path": extra.display().to_string() }),
        )
        .await;
        client
    }

    /// Load the map-format fixture (`UDMF` `MAP03`, `Hexen` `MAP04`) as
    /// the base layer.
    async fn formats_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let extra = pwad_map_formats(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": extra.display().to_string() }),
        )
        .await;
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

    /// The map row named `wanted` from a `list_maps` report.
    fn map_named<'rep>(report: &'rep Value, wanted: &str) -> &'rep Value {
        report["maps"]
            .as_array()
            .expect("maps is an array")
            .iter()
            .find(|row| row["name"] == json!(wanted))
            .unwrap_or_else(|| panic!("no map row named {wanted} in {:?}", report["maps"]))
    }

    /// The lump row named `wanted` from a `get_map_lumps` report.
    fn lump_named<'rep>(report: &'rep Value, wanted: &str) -> &'rep Value {
        report["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .find(|row| row["name"] == json!(wanted))
            .unwrap_or_else(|| panic!("no lump row named {wanted} in {:?}", report["lumps"]))
    }

    /// The row names of a `get_map_lumps` report, in result order.
    fn lump_names(report: &Value) -> Vec<String> {
        report["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .map(|row| row["name"].as_str().expect("names are strings").to_owned())
            .collect()
    }

    #[tokio::test]
    async fn list_maps_merged_lists_both_fixtures_with_formats_and_sources() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let report = call(&client, "list_maps", json!({})).await;
        let maps = report["maps"].as_array().expect("maps is an array");
        assert_eq!(maps.len(), 2, "one block per fixture archive");
        let first = map_named(&report, "MAP01");
        assert_eq!(first["format"], json!("doom"), "the binary block");
        assert_eq!(first["udmf_namespace"], Value::Null, "no UDMF here");
        assert_eq!(
            first["source"],
            json!({ "layer": 0_u64 }),
            "the IWAD's copy",
        );
        assert_eq!(
            first["lumps"],
            json!(DOOM_MEMBERS),
            "members in directory order, header excluded",
        );
        assert_eq!(
            first["has_nodes"],
            json!(false),
            "the fixture NODES are empty"
        );
        assert_eq!(first["has_gl_nodes"], json!(false), "no GL run");
        let second = map_named(&report, "MAP02");
        assert_eq!(second["format"], json!("doom"), "the PWAD block too");
        assert_eq!(
            second["source"],
            json!({ "layer": 1_u64 }),
            "the PWAD's copy"
        );
    }

    #[tokio::test]
    async fn list_maps_layer_scopes_read_one_archive_and_working_starts_empty() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let base = call(&client, "list_maps", json!({ "scope": "layer:0" })).await;
        assert_eq!(
            base["maps"].as_array().expect("array").len(),
            1,
            "MAP01 only"
        );
        assert_eq!(map_named(&base, "MAP01")["name"], json!("MAP01"));
        let extra = call(&client, "list_maps", json!({ "scope": "layer:1" })).await;
        assert_eq!(
            extra["maps"].as_array().expect("array").len(),
            1,
            "MAP02 only"
        );
        assert_eq!(map_named(&extra, "MAP02")["name"], json!("MAP02"));
        call(&client, "new_wad", json!({})).await;
        let working = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(
            working["maps"],
            json!([]),
            "the fresh working archive holds no map",
        );
    }

    #[tokio::test]
    async fn list_maps_reports_scope_errors_like_the_lumps_tools() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let (code, message) = call_err(&client, "list_maps", json!({ "scope": "nope" })).await;
        assert_eq!(code, "invalid_argument", "unknown scope");
        assert!(
            message.contains("merged") && message.contains("layer:"),
            "the error lists the forms: {message}"
        );
        let (layer_code, _) = call_err(&client, "list_maps", json!({ "scope": "layer:5" })).await;
        assert_eq!(layer_code, "not_found", "layer 5 is not loaded");
    }

    #[tokio::test]
    async fn list_maps_and_get_map_lumps_report_the_udmf_and_hexen_formats() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        let report = call(&client, "list_maps", json!({ "scope": "layer:0" })).await;
        let udmf = map_named(&report, "MAP03");
        assert_eq!(udmf["format"], json!("udmf"), "the TEXTMAP block");
        assert_eq!(
            udmf["udmf_namespace"],
            json!("zdoom"),
            "the namespace the TEXTMAP declares",
        );
        assert_eq!(udmf["has_nodes"], json!(true), "the ZNODES lump counts");
        assert_eq!(udmf["has_gl_nodes"], json!(true), "the GL_MAP03 run");
        assert_eq!(
            udmf["lumps"],
            json!([
                "TEXTMAP", "ZNODES", "ENDMAP", "GL_MAP03", "GL_VERT", "GL_NODES",
            ]),
            "the block members and the GL run",
        );
        let hexen = map_named(&report, "MAP04");
        assert_eq!(hexen["format"], json!("hexen"), "the BEHAVIOR marks it");
        assert_eq!(
            hexen["udmf_namespace"],
            Value::Null,
            "binary has no namespace"
        );
        assert_eq!(hexen["has_nodes"], json!(true), "the NODES carry bytes");
        assert_eq!(hexen["has_gl_nodes"], json!(false));
        let raw = call(&client, "get_map_lumps", json!({ "map": "MAP04" })).await;
        assert_eq!(
            raw["format"],
            json!("hexen"),
            "the raw set carries the format"
        );
        assert!(
            lump_names(&raw).contains(&"BEHAVIOR".to_owned()),
            "the generated BEHAVIOR is a member: {:?}",
            lump_names(&raw)
        );
    }

    #[tokio::test]
    async fn get_map_lumps_lists_the_raw_lump_set_with_sizes_and_kinds() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let report = call(&client, "get_map_lumps", json!({ "map": "MAP02" })).await;
        assert_eq!(report["map"], json!("MAP02"), "the map echoes");
        assert_eq!(report["format"], json!("doom"), "and its format");
        assert_eq!(lump_names(&report), DOOM_MEMBERS, "the raw set in order");
        let things = lump_named(&report, "THINGS");
        assert_eq!(things["size"], json!(20_u64), "two 10-byte things");
        assert_eq!(
            things["kind"],
            json!("map_data"),
            "classified like list_lumps"
        );
        assert!(things.get("asset").is_none(), "no asset key without urls");
        assert_eq!(
            lump_named(&report, "NODES")["size"],
            json!(0_u64),
            "empty nodes"
        );
        let (code, message) = call_err(
            &client,
            "get_map_lumps",
            json!({ "map": "MAP02", "scope": "layer:0" }),
        )
        .await;
        assert_eq!(code, "not_found", "MAP02 is not in layer 0");
        assert!(
            message.contains("layer:0"),
            "the message names the scope: {message}"
        );
    }

    #[tokio::test]
    async fn get_map_lumps_urls_publish_the_raw_bytes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let report = call(
            &client,
            "get_map_lumps",
            json!({ "map": "MAP01", "urls": true }),
        )
        .await;
        let things = lump_named(&report, "THINGS");
        assert_eq!(things["asset"]["filename"], json!("MAP01_THINGS.lmp"));
        assert_eq!(
            things["asset"]["mime"],
            json!("application/octet-stream"),
            "raw bytes",
        );
        assert_eq!(things["asset"]["size"], json!(20_u64), "the payload size");
        let url = things["asset"]["url"].as_str().expect("an asset url");
        assert!(
            url.starts_with("http://localhost:"),
            "a local origin: {url}"
        );
        let written = write_doom_map(&two_room_map()).expect("the fixture map serializes");
        let parsed = LumpName::parse("THINGS").expect("a valid name");
        let expected = written
            .iter()
            .find(|lump| lump.name == parsed)
            .expect("the THINGS lump")
            .data
            .to_vec();
        let fetched = fetch_asset(url).await;
        assert_eq!(fetched, expected, "the served bytes are the lump's own");
        assert!(
            lump_named(&report, "SEGS").get("asset").is_none(),
            "zero-size lumps are not published",
        );
    }

    #[tokio::test]
    async fn get_map_lumps_resolves_map_active_map_and_the_only_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let (code, message) = call_err(&client, "get_map_lumps", json!({})).await;
        assert_eq!(code, "invalid_argument", "two maps, no default");
        assert!(
            message.contains("pass map or call set_active_map"),
            "the error names the fixes: {message}"
        );
        call(&client, "set_active_map", json!({ "map": "MAP02" })).await;
        let report = call(&client, "get_map_lumps", json!({})).await;
        assert_eq!(
            report["map"],
            json!("MAP02"),
            "the active map is the default"
        );
        let solo = connect().await;
        let base = iwad(dir.path());
        call(
            &solo,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let only = call(&solo, "get_map_lumps", json!({})).await;
        assert_eq!(
            only["map"],
            json!("MAP01"),
            "a single map needs no selection"
        );
        let empty = connect().await;
        let (empty_code, _) = call_err(&empty, "get_map_lumps", json!({})).await;
        assert_eq!(
            empty_code, "invalid_argument",
            "no archives, no map to read"
        );
    }

    #[tokio::test]
    async fn get_map_lumps_rejects_unknown_maps_and_bad_names() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let (unknown_code, message) =
            call_err(&client, "get_map_lumps", json!({ "map": "MAP99" })).await;
        assert_eq!(unknown_code, "not_found", "no such map");
        assert!(
            message.contains("MAP99") && message.contains("list_maps"),
            "the message names the map and the way out: {message}"
        );
        let (name_code, _) = call_err(&client, "get_map_lumps", json!({ "map": "BAD NAME" })).await;
        assert_eq!(
            name_code, "invalid_argument",
            "spaces are not allowed in names"
        );
        let (scope_code, _) = call_err(&client, "get_map_lumps", json!({ "scope": "nope" })).await;
        assert_eq!(scope_code, "invalid_argument", "unknown scope");
    }

    /// The whole `get_map_summary` result for the two-room fixture map
    /// under the given `source` value.
    fn summary_json(source: &Value) -> Value {
        json!({
            "name": "MAP01", "format": "doom", "udmf_namespace": null, "source": source,
            "counts": {"things": 2_u64, "vertices": 6_u64, "linedefs": 7_u64, "sidedefs": 8_u64, "sectors": 2_u64},
            "bounds": {"min_x": 0_i64, "min_y": 0_i64, "max_x": 512_i64, "max_y": 256_i64, "width": 512_i64, "height": 256_i64},
            "lines": {"one_sided": 6_u64, "two_sided": 1_u64, "with_special": 0_u64},
            "heights": {"floor_min": 0_i64, "floor_max": 32_i64, "ceiling_min": 128_i64, "ceiling_max": 128_i64},
            "things_by_type": [{"type": 1_i64, "count": 1_u64}, {"type": 3001_i64, "count": 1_u64}],
            "player_starts": [1_i64],
            "textures_used": ["STEP1", "WALL1"], "flats_used": ["FLOOR1"],
            "nodes": {
                "present": false, "segs": 0_u64, "subsectors": 0_u64, "nodes": 0_u64,
                "reject_bytes": 0_u64, "blockmap_bytes": 0_u64, "znodes": false, "gl": false,
            },
        })
    }

    #[tokio::test]
    async fn get_map_summary_reports_the_fixture_map_then_the_working_copy() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let report = call(&client, "get_map_summary", json!({})).await;
        assert_eq!(
            report,
            summary_json(&json!({ "layer": 0_u64 })),
            "the whole summary of the only map, resolved without a name",
        );
        call(
            &client,
            "replace_lump",
            json!({ "name": "REJECT", "hex": "AA BB" }),
        )
        .await;
        let working = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
        assert_eq!(
            working["source"],
            json!("working"),
            "the copied block reads from the working archive",
        );
        assert_eq!(
            working["nodes"]["reject_bytes"],
            json!(2_u64),
            "the new REJECT",
        );
        assert_eq!(
            working["nodes"]["present"],
            json!(false),
            "REJECT bytes alone are no node data",
        );
        assert_eq!(
            working["counts"]["things"],
            json!(2_u64),
            "the map itself is unchanged",
        );
    }

    #[tokio::test]
    async fn get_map_summary_covers_the_udmf_and_hexen_node_shapes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        let udmf = call(&client, "get_map_summary", json!({ "map": "MAP03" })).await;
        assert_eq!(udmf["format"], json!("udmf"), "the TEXTMAP block");
        assert_eq!(
            udmf["udmf_namespace"],
            json!("zdoom"),
            "the namespace rides along",
        );
        assert_eq!(
            udmf["bounds"]["width"],
            json!(512_i64),
            "the decoded map bounds",
        );
        assert_eq!(
            udmf["nodes"],
            json!({
                "present": true, "segs": 0_u64, "subsectors": 0_u64, "nodes": 0_u64,
                "reject_bytes": 0_u64, "blockmap_bytes": 0_u64, "znodes": true, "gl": true,
            }),
            "bytes in ZNODES and an attached GL run",
        );
        let hexen = call(&client, "get_map_summary", json!({ "map": "MAP04" })).await;
        assert_eq!(hexen["format"], json!("hexen"), "the BEHAVIOR marks it");
        assert_eq!(
            hexen["udmf_namespace"],
            Value::Null,
            "binary has no namespace",
        );
        assert_eq!(
            hexen["nodes"],
            json!({
                "present": true, "segs": 0_u64, "subsectors": 0_u64, "nodes": 0_u64,
                "reject_bytes": 0_u64, "blockmap_bytes": 0_u64, "znodes": false, "gl": false,
            }),
            "8 NODES bytes are present but hold no whole vanilla record",
        );
    }

    #[tokio::test]
    async fn get_map_summary_reports_unknown_maps_and_missing_defaults() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let (code, message) = call_err(&client, "get_map_summary", json!({ "map": "MAP99" })).await;
        assert_eq!(code, "not_found", "no such map");
        assert!(
            message.contains("MAP99") && message.contains("list_maps"),
            "the message names the map and the way out: {message}",
        );
        let (default_code, default_message) = call_err(&client, "get_map_summary", json!({})).await;
        assert_eq!(default_code, "invalid_argument", "two maps, no default");
        assert!(
            default_message.contains("pass map or call set_active_map"),
            "the error names the fixes: {default_message}",
        );
        let (layer_code, _) = call_err(
            &client,
            "get_map_summary",
            json!({ "map": "MAP02", "scope": "layer:0" }),
        )
        .await;
        assert_eq!(layer_code, "not_found", "MAP02 is not in layer 0");
        let (scope_code, _) = call_err(
            &client,
            "get_map_summary",
            json!({ "map": "MAP02", "scope": "nope" }),
        )
        .await;
        assert_eq!(scope_code, "invalid_argument", "unknown scope");
        let (name_code, _) =
            call_err(&client, "get_map_summary", json!({ "map": "BAD NAME" })).await;
        assert_eq!(
            name_code, "invalid_argument",
            "spaces are not allowed in names"
        );
        let empty = connect().await;
        let (empty_code, _) = call_err(&empty, "get_map_summary", json!({ "map": "MAP01" })).await;
        assert_eq!(
            empty_code, "not_found",
            "nothing is loaded, MAP01 is nowhere"
        );
    }

    #[tokio::test]
    async fn set_active_map_selects_validates_clears_and_shows_in_session_info() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let (missing_code, message) =
            call_err(&client, "set_active_map", json!({ "map": "MAP99" })).await;
        assert_eq!(missing_code, "not_found", "MAP99 exists nowhere");
        assert!(message.contains("MAP99"), "the message names it: {message}");
        let (blank_code, _) = call_err(&client, "set_active_map", json!({ "map": "" })).await;
        assert_eq!(
            blank_code, "invalid_argument",
            "an empty name does not parse"
        );
        let set = call(&client, "set_active_map", json!({ "map": "map01" })).await;
        assert_eq!(set["active_map"], json!("MAP01"), "parsed and uppercased");
        let after_set = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            after_set["active_map"],
            json!("MAP01"),
            "the session reflects it"
        );
        assert_eq!(
            after_set["undo"],
            json!([]),
            "the selection is not an undoable edit"
        );
        let cleared = call(&client, "set_active_map", json!({ "map": null })).await;
        assert_eq!(cleared["active_map"], Value::Null, "null clears");
        let after_clear = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            after_clear["active_map"],
            Value::Null,
            "and the session shows that"
        );
        call(&client, "set_active_map", json!({ "map": "MAP02" })).await;
        let omitted = call(&client, "set_active_map", json!({})).await;
        assert_eq!(
            omitted["active_map"],
            Value::Null,
            "an omitted map clears too"
        );
    }

    /// Load the plain `IWAD` fixture (one two-room `MAP01`) as layer 0.
    async fn iwad_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        client
    }

    #[tokio::test]
    async fn assemble_map_resolves_the_lines_of_the_fixture_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        let report = call(&client, "assemble_map", json!({})).await;
        assert_eq!(report["map"], json!("MAP01"), "the map echoes");
        assert_eq!(report["problems"], json!([]), "the fixture map is sound");
        let step = &report["linedefs"][2];
        assert_eq!(
            step["front"]["sector"],
            json!(0_i64),
            "linedef 2 faces the hall",
        );
        assert_eq!(
            step["back"]["sector"],
            json!(1_i64),
            "and opens to the back room",
        );
        assert_eq!(
            step["flag_names"],
            json!(["twosided"]),
            "flags named in the Doom family",
        );
        assert_eq!(step["length"], json!(256_u64), "the shared wall measures");
        assert_eq!(
            step["v1"]["x"],
            json!(256_i64),
            "ends resolved to coordinates",
        );
        assert_eq!(step["v2"]["index"], json!(1_i64), "with their indices");
        assert_eq!(
            report["vertices"][1]["linedefs"],
            json!([2_i64, 3_i64, 6_i64]),
            "the shared corner lists its lines",
        );
        assert_eq!(
            report["sidedefs"][3]["side"],
            json!("back"),
            "sidedef 3 is the step line's back side",
        );
        assert_eq!(
            report["sidedefs"][3]["linedef"],
            json!(2_i64),
            "of the step line",
        );
    }

    #[tokio::test]
    async fn assemble_map_resolves_the_sectors_and_things_of_the_fixture_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        let report = call(&client, "assemble_map", json!({})).await;
        assert_eq!(
            report["sectors"][0]["neighbors"],
            json!([1_i64]),
            "the hall neighbours only the back room",
        );
        assert_eq!(
            report["sectors"][0]["linedefs"],
            json!([0_i64, 1_i64, 2_i64, 3_i64]),
            "the hall boundary",
        );
        assert_eq!(
            report["sectors"][0]["sidedefs"],
            json!([0_i64, 1_i64, 2_i64, 4_i64]),
            "the hall sides",
        );
        assert_eq!(
            report["sectors"][0]["bbox"],
            json!([0_i64, 0_i64, 256_i64, 256_i64]),
            "the hall box",
        );
        assert_eq!(
            report["things"][1]["sector"],
            json!(1_i64),
            "the imp (thing 1) stands in the back room",
        );
        assert_eq!(
            report["sectors"][1]["things"],
            json!([1_i64]),
            "and the back room lists it",
        );
        assert_eq!(
            report["sectors"][0]["things"],
            json!([0_i64]),
            "the player start stands in the hall",
        );
        assert_eq!(
            report["things"][0]["flag_names"],
            json!(["easy", "medium", "hard"]),
            "thing flags named too",
        );
    }

    #[tokio::test]
    async fn assemble_map_include_returns_only_the_requested_arrays() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        let report = call(&client, "assemble_map", json!({ "include": ["things"] })).await;
        assert_eq!(
            report["things"]
                .as_array()
                .expect("things is an array")
                .len(),
            2,
            "both things carry resolved sectors",
        );
        assert!(
            report.get("vertices").is_none(),
            "vertices were not requested"
        );
        assert!(
            report.get("linedefs").is_none(),
            "linedefs were not requested"
        );
        assert!(
            report.get("sidedefs").is_none(),
            "sidedefs were not requested"
        );
        assert!(
            report.get("sectors").is_none(),
            "sectors were not requested"
        );
        assert!(
            report.get("problems").is_some(),
            "problems always ride along"
        );
        let (code, message) =
            call_err(&client, "assemble_map", json!({ "include": ["nope"] })).await;
        assert_eq!(code, "invalid_argument", "unknown include name");
        assert!(
            message.contains("vertices") && message.contains("things"),
            "the error lists the valid names: {message}"
        );
    }

    /// Requires the fetched graph asset to carry `key` as an array of
    /// exactly `len` rows.
    fn assert_asset_array(graph: &Value, key: &str, len: usize) {
        let rows = graph[key].as_array().expect("the asset carries the array");
        assert_eq!(rows.len(), len, "the {key} array holds every row");
    }

    #[tokio::test]
    async fn assemble_map_publishes_an_asset_above_the_inline_limit() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        let report = call(&client, "assemble_map", json!({ "inline_limit": 3_u64 })).await;
        assert_eq!(report["map"], json!("MAP01"), "the map still echoes");
        assert!(report.get("linedefs").is_none(), "nothing inlines");
        assert_eq!(
            report["counts"],
            json!({
                "vertices": 6_u64, "linedefs": 7_u64, "sidedefs": 8_u64,
                "sectors": 2_u64, "things": 2_u64, "problems": 0_u64,
            }),
            "every kind counted",
        );
        assert_eq!(
            report["asset"]["filename"],
            json!("MAP01_graph.json"),
            "the documented name",
        );
        assert_eq!(
            report["asset"]["mime"],
            json!("application/json"),
            "the documented media type",
        );
        let url = report["asset"]["url"].as_str().expect("an asset url");
        let body = fetch_asset(url).await;
        let graph: Value = serde_json::from_slice(&body).expect("the asset is JSON");
        assert_eq!(graph["map"], json!("MAP01"), "the body names the map");
        assert_asset_array(&graph, "vertices", 6);
        assert_asset_array(&graph, "linedefs", 7);
        assert_asset_array(&graph, "sidedefs", 8);
        assert_asset_array(&graph, "sectors", 2);
        assert_asset_array(&graph, "things", 2);
        assert_eq!(graph["problems"], json!([]), "and the problem list");
    }

    #[tokio::test]
    async fn assemble_map_refuses_inline_limits_outside_the_range() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        let (zero_code, _) =
            call_err(&client, "assemble_map", json!({ "inline_limit": 0_u64 })).await;
        assert_eq!(zero_code, "invalid_argument", "zero inlines nothing");
        let (big_code, big_message) = call_err(
            &client,
            "assemble_map",
            json!({ "inline_limit": 20001_u64 }),
        )
        .await;
        assert_eq!(big_code, "invalid_argument", "above the maximum");
        assert!(
            big_message.contains("1..=20000"),
            "the message names the range: {big_message}"
        );
    }

    #[tokio::test]
    async fn assemble_map_reports_dangling_references_as_problems() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad_broken_map(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let report = call(&client, "assemble_map", json!({})).await;
        let problems = report["problems"].as_array().expect("problems is an array");
        assert!(!problems.is_empty(), "the break is reported, not fatal");
        assert!(
            problems.iter().any(|problem| problem
                .as_str()
                .is_some_and(|text| text.contains("sector 99"))),
            "the dangling sector is named: {problems:?}"
        );
        assert_eq!(
            report["sidedefs"][4]["sector"],
            Value::Null,
            "the row nulls it"
        );
        assert_eq!(
            report["linedefs"][3]["front"]["sector"],
            Value::Null,
            "and through the referencing line",
        );
        assert_eq!(
            report["sectors"][0]["sidedefs"],
            json!([0_i64, 1_i64, 2_i64]),
            "the hall lost its dangling side",
        );
        assert_eq!(
            report["things"][0]["sector"],
            json!(0_i64),
            "the rest of the graph still resolves",
        );
    }

    #[tokio::test]
    async fn assemble_map_shares_the_group_map_and_scope_errors() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let (code, message) = call_err(&client, "assemble_map", json!({ "map": "MAP99" })).await;
        assert_eq!(code, "not_found", "no such map");
        assert!(
            message.contains("MAP99"),
            "the message names the map: {message}"
        );
        let (default_code, _) = call_err(&client, "assemble_map", json!({})).await;
        assert_eq!(default_code, "invalid_argument", "two maps, no default");
        let (scope_code, _) = call_err(
            &client,
            "assemble_map",
            json!({ "map": "MAP01", "scope": "nope" }),
        )
        .await;
        assert_eq!(scope_code, "invalid_argument", "unknown scope");
    }

    /// The whole `create_map` result for a fresh `doom` block.
    fn created_doom_map() -> Value {
        json!({
            "map": "MAP05", "format": "doom", "namespace": null,
            "warnings": [], "active_map": "MAP05",
        })
    }

    #[tokio::test]
    async fn create_map_builds_empty_doom_and_hexen_blocks_and_sets_the_active_map() {
        let client = connect().await;
        let doom = call(&client, "create_map", json!({ "name": "map05" })).await;
        assert_eq!(doom, created_doom_map(), "the documented doom result");
        let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
        let row = map_named(&listing, "MAP05");
        assert_eq!(row["format"], json!("doom"), "the vanilla set");
        assert_eq!(row["lumps"], json!(DOOM_MEMBERS), "the ten empty members");
        assert_eq!(
            row["source"],
            json!("working"),
            "it is the working archive's"
        );
        let summary = call(
            &client,
            "get_map_summary",
            json!({ "map": "MAP05", "scope": "working" }),
        )
        .await;
        assert_eq!(
            summary["counts"],
            json!({"things": 0_u64, "vertices": 0_u64, "linedefs": 0_u64, "sidedefs": 0_u64, "sectors": 0_u64}),
            "an empty map holds nothing",
        );
        assert_eq!(summary["bounds"], Value::Null, "and has no bounds");
        let hexen = call(
            &client,
            "create_map",
            json!({ "name": "MAP06", "format": "hexen" }),
        )
        .await;
        assert_eq!(hexen["format"], json!("hexen"), "the hexen report");
        assert_eq!(
            hexen["active_map"],
            json!("MAP05"),
            "only an unset active map is adopted",
        );
        let hexen_listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(
            map_named(&hexen_listing, "MAP06")["format"],
            json!("hexen"),
            "the generated BEHAVIOR classifies it",
        );
        let undo = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo["undone"],
            json!(["create_map MAP06"]),
            "the hexen create is one undoable step",
        );
    }

    #[tokio::test]
    async fn create_map_writes_a_udmf_block_declaring_its_namespace() {
        let client = connect().await;
        let udmf = call(
            &client,
            "create_map",
            json!({ "name": "MAP07", "format": "udmf", "namespace": "GZDoom" }),
        )
        .await;
        assert_eq!(udmf["namespace"], json!("gzdoom"), "the lowered namespace");
        let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
        let row = map_named(&listing, "MAP07");
        assert_eq!(row["format"], json!("udmf"), "the TEXTMAP block");
        assert_eq!(row["udmf_namespace"], json!("gzdoom"), "which decodes");
        assert_eq!(
            row["lumps"],
            json!(["TEXTMAP", "ENDMAP"]),
            "and carries only its namespace so far",
        );
        let summary = call(
            &client,
            "get_map_summary",
            json!({ "map": "MAP07", "scope": "working" }),
        )
        .await;
        assert_eq!(udmf["map"], json!("MAP07"), "the create echoes the map");
        assert_eq!(summary["format"], json!("udmf"), "the empty UDMF map reads");
        assert_eq!(
            summary["counts"]["things"],
            json!(0_u64),
            "with no elements at all",
        );
    }

    #[tokio::test]
    async fn create_map_warns_about_odd_names_and_refuses_bad_arguments() {
        let client = connect().await;
        let report = call(&client, "create_map", json!({ "name": "SLICK01" })).await;
        assert_eq!(report["map"], json!("SLICK01"), "the odd name is created");
        let warnings = report["warnings"].as_array().expect("warnings is an array");
        assert_eq!(warnings.len(), 1, "one notice: {warnings:?}");
        assert!(
            warnings[0]
                .as_str()
                .is_some_and(|text| text.contains("SLICK01") && text.contains("MAPINFO")),
            "the warning names the map and the fix: {warnings:?}",
        );
        let (format_code, format_message) = call_err(
            &client,
            "create_map",
            json!({ "name": "MAP08", "format": "zdoom" }),
        )
        .await;
        assert_eq!(format_code, "invalid_argument", "not a map format");
        assert!(
            format_message.contains("doom") && format_message.contains("udmf"),
            "the message lists the formats: {format_message}",
        );
        let (ns_code, _) = call_err(
            &client,
            "create_map",
            json!({ "name": "MAP08", "namespace": "zdoom" }),
        )
        .await;
        assert_eq!(ns_code, "invalid_argument", "namespace on a binary map");
        let (name_code, _) = call_err(&client, "create_map", json!({ "name": "" })).await;
        assert_eq!(name_code, "invalid_argument", "an empty name");
        let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
        let maps = listing["maps"].as_array().expect("maps is an array");
        assert_eq!(maps.len(), 1, "only the warned creation landed");
        assert_eq!(
            maps.first().expect("one row")["name"],
            json!("SLICK01"),
            "by name"
        );
    }

    #[tokio::test]
    async fn create_map_refuses_working_duplicates_replaces_and_overrides_layers() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        call(&client, "create_map", json!({ "name": "MAP01" })).await;
        assert_eq!(
            call(&client, "get_map_summary", json!({ "map": "MAP01" })).await["source"],
            json!("working"),
            "the empty block overrides the IWAD's",
        );
        let (code, message) = call_err(&client, "create_map", json!({ "name": "MAP01" })).await;
        assert_eq!(code, "already_exists", "the working override clashes");
        assert!(
            message.contains("replace"),
            "the message names the way out: {message}",
        );
        call(
            &client,
            "add_thing",
            json!({ "map": "MAP01", "type": 1_i64, "x": 8_i64, "y": 8_i64 }),
        )
        .await;
        assert_eq!(
            call(&client, "get_map_summary", json!({ "map": "MAP01" })).await["counts"]["things"],
            json!(1_u64),
            "the block has a thing before the replace",
        );
        let replaced = call(
            &client,
            "create_map",
            json!({ "name": "MAP01", "replace": true }),
        )
        .await;
        assert_eq!(replaced["active_map"], json!("MAP01"), "the replace lands");
        let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(
            listing["maps"].as_array().expect("array").len(),
            1,
            "the old block was discarded, not trailed",
        );
        assert_eq!(
            call(&client, "get_map_summary", json!({ "map": "MAP01" })).await["counts"]["things"],
            json!(0_u64),
            "the replaced block is empty again",
        );
        let undo = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo["undone"],
            json!(["create_map MAP01"]),
            "the replace is one undoable step",
        );
        assert_eq!(
            call(&client, "get_map_summary", json!({ "map": "MAP01" })).await["counts"]["things"],
            json!(1_u64),
            "undo brings the thing back",
        );
    }

    /// The asset bytes of the lump row `wanted` of a `get_map_lumps urls` report.
    async fn lump_bytes(report: &Value, wanted: &str) -> Vec<u8> {
        let url = lump_named(report, wanted)["asset"]["url"]
            .as_str()
            .expect("a published lump");
        fetch_asset(url).await
    }

    /// The rows of a `get_map_lumps urls` report as `(name, size, kind)`
    /// facts, so two reports of the same bytes compare directly.
    fn lump_facts(report: &Value) -> Vec<(String, u64, String)> {
        report["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .map(|row| {
                (
                    row["name"].as_str().expect("a name").to_owned(),
                    row["size"].as_u64().expect("a size"),
                    row["kind"].as_str().expect("a kind").to_owned(),
                )
            })
            .collect()
    }

    /// A `copy_map` call the faithful-copy tests share.
    async fn copy_to_map07(client: &Client) -> Value {
        call(
            client,
            "copy_map",
            json!({ "from": "MAP01", "to": "map07" }),
        )
        .await
    }

    #[tokio::test]
    async fn copy_map_copies_the_iwad_map_byte_for_byte() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        let report = copy_to_map07(&client).await;
        assert_eq!(
            report,
            json!({"from": "MAP01", "to": "MAP07", "lumps": 11_u64}),
            "the documented copy result",
        );
        let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
        let row = map_named(&listing, "MAP07");
        assert_eq!(row["format"], json!("doom"), "the block arrived as doom");
        assert_eq!(row["lumps"], json!(DOOM_MEMBERS), "with every member");
        let source = call(
            &client,
            "get_map_lumps",
            json!({ "map": "MAP01", "scope": "layer:0", "urls": true }),
        )
        .await;
        let copy = call(
            &client,
            "get_map_lumps",
            json!({ "map": "MAP07", "scope": "working", "urls": true }),
        )
        .await;
        assert_eq!(lump_names(&source), lump_names(&copy), "the same lump set");
        assert_eq!(
            lump_facts(&source),
            lump_facts(&copy),
            "every row matches by name, size and kind",
        );
        assert_eq!(
            lump_bytes(&copy, "THINGS").await,
            lump_bytes(&source, "THINGS").await,
            "the THINGS bytes are identical",
        );
        assert_eq!(
            lump_bytes(&copy, "SIDEDEFS").await,
            lump_bytes(&source, "SIDEDEFS").await,
            "and the SIDEDEFS bytes are",
        );
        let summary = call(
            &client,
            "get_map_summary",
            json!({ "map": "MAP07", "scope": "working" }),
        )
        .await;
        assert_eq!(
            summary["counts"]["linedefs"],
            json!(7_u64),
            "the copy decodes whole"
        );
    }

    #[tokio::test]
    async fn copy_map_undoes_as_one_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        copy_to_map07(&client).await;
        let undo = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo["undone"],
            json!(["copy_map MAP01 -> MAP07"]),
            "one undoable step",
        );
        let after = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(after["maps"], json!([]), "the copy rolled back wholly");
    }

    #[tokio::test]
    async fn copy_map_renames_the_gl_marker_and_drops_it_for_long_names() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        call(
            &client,
            "copy_map",
            json!({ "from": "MAP03", "to": "MAP08" }),
        )
        .await;
        let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
        let row = map_named(&listing, "MAP08");
        assert_eq!(row["has_gl_nodes"], json!(true), "the GL run came along");
        assert_eq!(
            row["lumps"],
            json!([
                "TEXTMAP", "ZNODES", "ENDMAP", "GL_MAP08", "GL_VERT", "GL_NODES"
            ]),
            "its marker follows the new name",
        );
        let (code, message) = call_err(
            &client,
            "copy_map",
            json!({ "from": "MAP03", "to": "MAP08" }),
        )
        .await;
        assert_eq!(code, "already_exists", "MAP08 is working now");
        assert!(message.contains("replace"), "the hint is there: {message}");
        call(
            &client,
            "copy_map",
            json!({ "from": "MAP03", "to": "MAP08", "replace": true }),
        )
        .await;
        let (missing_code, missing_message) = call_err(
            &client,
            "copy_map",
            json!({ "from": "MAP99", "to": "MAP09" }),
        )
        .await;
        assert_eq!(missing_code, "not_found", "no MAP99 in the merged view");
        assert!(
            missing_message.contains("MAP99"),
            "the message names it: {missing_message}",
        );
        let long = call(
            &client,
            "copy_map",
            json!({ "from": "MAP03", "to": "LONGNAME" }),
        )
        .await;
        assert_eq!(
            long["lumps"],
            json!(4_u64),
            "header plus three members, the GL run dropped",
        );
        let after = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(
            map_named(&after, "LONGNAME")["has_gl_nodes"],
            json!(false),
            "no marker for an eight-character name fits",
        );
    }

    #[tokio::test]
    async fn rename_map_moves_working_blocks_and_follows_the_selection() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        call(
            &client,
            "copy_map",
            json!({ "from": "MAP01", "to": "MAP07" }),
        )
        .await;
        call(&client, "set_active_map", json!({ "map": "MAP07" })).await;
        let report = call(
            &client,
            "rename_map",
            json!({ "from": "MAP07", "to": "MAP08" }),
        )
        .await;
        assert_eq!(
            report,
            json!({"from": "MAP07", "to": "MAP08"}),
            "the documented rename result",
        );
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["active_map"],
            json!("MAP08"),
            "the selection followed the map",
        );
        let listing = call(&client, "list_maps", json!({ "scope": "working" })).await;
        let maps = listing["maps"].as_array().expect("maps is an array");
        assert_eq!(maps.len(), 1, "the only working block is the renamed one");
        assert_eq!(
            maps.first().expect("one row")["name"],
            json!("MAP08"),
            "by name"
        );
        let undo = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo["undone"],
            json!(["rename_map MAP07 -> MAP08"]),
            "the rename is one undoable step",
        );
        let after = call(
            &client,
            "get_map_summary",
            json!({ "map": "MAP07", "scope": "working" }),
        )
        .await;
        assert_eq!(after["name"], json!("MAP07"), "undo restores the old name");
    }

    #[tokio::test]
    async fn rename_map_refuses_layer_maps_self_renames_and_missing_names() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_loaded(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "rename_map",
            json!({ "from": "MAP01", "to": "MAP07" }),
        )
        .await;
        assert_eq!(code, "not_found", "MAP01 is only the IWAD's");
        assert!(
            message.contains("copy_map MAP01 MAP07"),
            "the message points at the tool that works: {message}",
        );
        call(
            &client,
            "copy_map",
            json!({ "from": "MAP01", "to": "MAP07" }),
        )
        .await;
        let (self_code, _) = call_err(
            &client,
            "rename_map",
            json!({ "from": "MAP07", "to": "MAP07" }),
        )
        .await;
        assert_eq!(self_code, "invalid_argument", "a self rename is a refusal");
        let (missing_code, _) = call_err(
            &client,
            "rename_map",
            json!({ "from": "MAP99", "to": "MAP08" }),
        )
        .await;
        assert_eq!(missing_code, "not_found", "MAP99 is not working");
        call(&client, "create_map", json!({ "name": "MAP08" })).await;
        let (taken_code, taken_message) = call_err(
            &client,
            "rename_map",
            json!({ "from": "MAP07", "to": "MAP08" }),
        )
        .await;
        assert_eq!(taken_code, "already_exists", "MAP08 is taken");
        assert!(
            taken_message.contains("remove_map"),
            "the message names the way out: {taken_message}",
        );
        let still = call(
            &client,
            "get_map_summary",
            json!({ "map": "MAP07", "scope": "working" }),
        )
        .await;
        assert_eq!(still["name"], json!("MAP07"), "the refusal renamed nothing");
    }

    #[tokio::test]
    async fn rename_and_remove_map_track_the_gl_run_and_the_selection() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = formats_loaded(dir.path()).await;
        call(
            &client,
            "copy_map",
            json!({ "from": "MAP03", "to": "MAP09" }),
        )
        .await;
        call(&client, "set_active_map", json!({ "map": "MAP09" })).await;
        call(
            &client,
            "rename_map",
            json!({ "from": "MAP09", "to": "E1M8" }),
        )
        .await;
        let renamed = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(
            map_named(&renamed, "E1M8")["lumps"],
            json!([
                "TEXTMAP", "ZNODES", "ENDMAP", "GL_E1M8", "GL_VERT", "GL_NODES"
            ]),
            "the marker followed the new name too",
        );
        let removed = call(&client, "remove_map", json!({ "name": "E1M8" })).await;
        assert_eq!(
            removed,
            json!({"map": "E1M8", "lumps_removed": 7_u64}),
            "the block and its GL run went together",
        );
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["active_map"],
            Value::Null,
            "and the selection was cleared",
        );
        let empty = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(empty["maps"], json!([]), "nothing is left in working");
        let (code, message) = call_err(&client, "remove_map", json!({ "name": "MAP03" })).await;
        assert_eq!(code, "not_found", "the layer's own MAP03 stays put");
        assert!(
            message.contains("read-only"),
            "the message explains why: {message}",
        );
        let undo = call(&client, "undo", json!({})).await;
        assert_eq!(
            undo["undone"],
            json!(["remove_map E1M8"]),
            "the removal is one undoable step",
        );
        let back = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(
            map_named(&back, "E1M8")["has_gl_nodes"],
            json!(true),
            "undo brings block and GL run back",
        );
    }
}
