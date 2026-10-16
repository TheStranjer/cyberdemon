//! The `maps` tool group as seen by a real MCP client: listing the map
//! blocks of every scope with formats, namespaces and node presence,
//! reading one block's raw lump set (with optionally published bytes), and
//! the active-map selection the map tools' optional `map` parameter
//! defaults to.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::wad::LumpName;
    use cyberdemon::wad::map::doom::write_doom_map;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad, pwad_map_formats, two_room_map};
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
}
