//! The `nodes` tool group as seen by a real MCP client: the three
//! inspection tools over the node-builder output (`get_nodes` for the
//! `SEGS`/`SSECTORS`/`NODES` tree in its vanilla and extended layouts,
//! `get_reject` for the visibility matrix, `get_blockmap` for the spatial
//! index), their empty-lump tolerance, their `format_error` refusals and
//! the `JSON` assets the `full` detail publishes, plus the two builders
//! (`build_blockmap`, `build_reject`) whose written lumps the inspection
//! tools then read back.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad_empty_map, pwad_nodes};
    use crate::support::{call, call_err, connect, http_get};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// Load the `IWAD` fixture (empty node lumps on `MAP01`) as layer 0.
    async fn plain_iwad(dir: &Path) -> Client {
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

    /// Load the nodes fixture (`MAP05` valid, `MAP06` malformed, `MAP07`
    /// `XNOD` in `ZNODES`) as layer 0.
    async fn nodes_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let extra = pwad_nodes(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": extra.display().to_string() }),
        )
        .await;
        client
    }

    /// Split an asset `URL` into the origin and the request path for
    /// [`http_get`].
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

    /// The `asset.url` of a full-detail report, as a parsed value plus
    /// the fetched `JSON` body.
    async fn fetch_json(report: &Value) -> Value {
        let url = report["asset"]["url"].as_str().expect("a published url");
        let body = fetch_asset(url).await;
        serde_json::from_slice(&body).expect("the asset is json")
    }

    #[tokio::test]
    async fn empty_node_lumps_report_none_rather_than_failing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(report["format"], json!("none"), "the empty block");
        assert_eq!(report["bytes"], json!(0_u64), "no node bytes");
        assert_eq!(report["segs"], json!(0_u64), "counts read zero");
        assert_eq!(report["nodes"], json!(0_u64), "and zero nodes");
        assert_eq!(report["extra_vertices"], json!(0_u64), "no added vertices");
        assert_eq!(report["root"], Value::Null, "no tree to root");
        assert_eq!(report["depth"], json!(0_u64), "and no depth");
        assert_eq!(report["gl"]["present"], json!(false), "no GL run");

        let reject = call(&client, "get_reject", json!({ "map": "MAP01" })).await;
        assert_eq!(reject["bytes"], json!(0_u64), "no reject bytes");
        assert_eq!(reject["sectors"], json!(2_u64), "the two fixture sectors");
        assert_eq!(reject["expected_bytes"], json!(1_u64), "four bits");
        assert_eq!(reject["all_zero"], json!(true), "nothing rejected");

        let blockmap = call(&client, "get_blockmap", json!({ "map": "MAP01" })).await;
        assert_eq!(blockmap["bytes"], json!(0_u64), "no blockmap bytes");
        assert_eq!(blockmap["columns"], json!(0_u64), "no grid");
        assert_eq!(blockmap["blocks"], json!(0_u64), "no cells");
        assert_eq!(blockmap["origin"]["x"], json!(0_i64), "zeroed origin");
    }

    #[tokio::test]
    async fn get_nodes_reads_the_hand_built_vanilla_tree_and_gl_run() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        let report = call(&client, "get_nodes", json!({ "map": "MAP05" })).await;
        assert_eq!(report["format"], json!("vanilla"), "the record lumps");
        assert_eq!(report["bytes"], json!(28_u64), "one 28-byte node");
        assert_eq!(report["segs"], json!(4_u64), "four segs");
        assert_eq!(report["subsectors"], json!(2_u64), "two subsectors");
        assert_eq!(report["nodes"], json!(1_u64), "one node");
        assert_eq!(report["extra_vertices"], json!(0_u64), "vanilla adds none");
        assert_eq!(report["root"], json!(0_u64), "the only node is the root");
        assert_eq!(report["depth"], json!(1_u64), "one level");
        assert_eq!(report["gl"]["present"], json!(true), "a GL run is attached");
        assert_eq!(report["gl"]["magic"], json!("gNd2"), "the GL_VERT magic");
        assert_eq!(
            report["gl"]["lumps"],
            json!({ "GL_MAP05": 0_u64, "GL_PVS": 2_u64, "GL_SEGS": 4_u64, "GL_VERT": 6_u64 }),
            "the run's sizes",
        );
        assert!(
            report.get("asset").is_none(),
            "summary detail publishes nothing: {report}"
        );
    }

    #[tokio::test]
    async fn get_nodes_full_publishes_the_tree_as_a_json_asset() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        let report = call(
            &client,
            "get_nodes",
            json!({ "map": "MAP05", "detail": "full" }),
        )
        .await;
        assert_eq!(
            report["asset"]["filename"],
            json!("MAP05_nodes.json"),
            "the asset name",
        );
        assert_eq!(
            report["asset"]["mime"],
            json!("application/json"),
            "the media type",
        );
        let body = fetch_json(&report).await;
        assert_eq!(body["format"], json!("vanilla"), "the layout travels");
        assert_eq!(body["segs"].as_array().expect("segs").len(), 4, "four segs");
        assert_eq!(
            body["segs"][3]["angle"],
            json!(-0x4000_i64),
            "record fields keep their signed values",
        );
        assert_eq!(
            body["subsectors"],
            json!([
                { "seg_count": 2_u64, "first_seg": 0_u64 },
                { "seg_count": 2_u64, "first_seg": 2_u64 },
            ]),
            "the subsector runs",
        );
        assert_eq!(
            body["nodes"][0]["left_child"],
            json!({ "subsector": 0_u64 }),
            "tagged children",
        );
        assert_eq!(body["extra_vertices"], json!([]), "none in vanilla");
    }

    #[tokio::test]
    async fn extended_xnod_nodes_parse_with_their_added_vertices() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        let report = call(
            &client,
            "get_nodes",
            json!({ "map": "MAP07", "detail": "full" }),
        )
        .await;
        assert_eq!(report["format"], json!("xnod"), "the ZNODES magic");
        assert_eq!(report["segs"], json!(2_u64), "two extended segs");
        assert_eq!(report["subsectors"], json!(2_u64), "two subsectors");
        assert_eq!(report["nodes"], json!(1_u64), "one node");
        assert_eq!(report["extra_vertices"], json!(1_u64), "one added vertex");
        assert_eq!(report["root"], json!(0_u64), "the last node is the root");
        assert_eq!(report["depth"], json!(1_u64), "one level");
        let body = fetch_json(&report).await;
        assert_eq!(body["format"], json!("xnod"), "the asset layout");
        assert_eq!(
            body["extra_vertices"],
            json!([{ "x": 0x1_0000_i64, "y": -0x4_0000_i64 }]),
            "the raw 16.16 pair",
        );
        assert_eq!(body["segs"][0]["v1"], json!(6_u64), "segs reach the vertex");
    }

    #[tokio::test]
    async fn get_reject_measures_the_matrix_and_full_publishes_rows() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        let report = call(&client, "get_reject", json!({ "map": "MAP05" })).await;
        assert_eq!(report["bytes"], json!(1_u64), "one byte stored");
        assert_eq!(
            report["expected_bytes"],
            json!(1_u64),
            "two sectors need one"
        );
        assert_eq!(report["size_ok"], json!(true), "the sizes agree");
        assert_eq!(report["all_zero"], json!(false), "two bits are set");
        assert_eq!(report["rejected_pairs"], json!(2_u64), "bit 1 and bit 3");
        let full = call(
            &client,
            "get_reject",
            json!({ "map": "MAP05", "detail": "full" }),
        )
        .await;
        assert_eq!(
            full["asset"]["filename"],
            json!("MAP05_reject.json"),
            "the asset name",
        );
        let body = fetch_json(&full).await;
        assert_eq!(body["sectors"], json!(2_u64), "the matrix width");
        assert_eq!(body["rows"], json!(["01", "01"]), "one row per sector");
        let wrong = call(&client, "get_reject", json!({ "map": "MAP06" })).await;
        assert_eq!(wrong["bytes"], json!(4_u64), "four stored bytes");
        assert_eq!(wrong["expected_bytes"], json!(1_u64), "one was right");
        assert_eq!(wrong["size_ok"], json!(false), "a wrong size is data");
        assert_eq!(wrong["all_zero"], json!(true), "and its bits are clear");
    }

    #[tokio::test]
    async fn get_blockmap_measures_the_grid_and_full_publishes_lists() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        let report = call(&client, "get_blockmap", json!({ "map": "MAP05" })).await;
        assert_eq!(report["bytes"], json!(22_u64), "the lump size");
        assert_eq!(report["origin"]["x"], json!(-128_i64), "the origin");
        assert_eq!(report["origin"]["y"], json!(-256_i64), "on both axes");
        assert_eq!(report["columns"], json!(1_u64), "one column");
        assert_eq!(report["rows"], json!(2_u64), "two rows");
        assert_eq!(report["blocks"], json!(2_u64), "two cells");
        assert_eq!(report["total_entries"], json!(1_u64), "one reference");
        assert_eq!(
            report["max_lines_per_block"],
            json!(1_u64),
            "the wider cell"
        );
        assert_eq!(report["empty_blocks"], json!(1_u64), "beside an empty one");
        let full = call(
            &client,
            "get_blockmap",
            json!({ "map": "MAP05", "detail": "full" }),
        )
        .await;
        assert_eq!(
            full["asset"]["filename"],
            json!("MAP05_blockmap.json"),
            "the asset name",
        );
        let body = fetch_json(&full).await;
        assert_eq!(body["origin_x"], json!(-128_i64), "the header");
        assert_eq!(
            body["blocks"],
            json!([[3_u64], []]),
            "the lists in row order"
        );
    }

    #[tokio::test]
    async fn malformed_node_lumps_fail_with_format_error() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        let (nodes_code, nodes_message) =
            call_err(&client, "get_nodes", json!({ "map": "MAP06" })).await;
        assert_eq!(nodes_code, "format_error", "a broken record size");
        assert!(
            nodes_message.contains("NODES"),
            "the lump is named: {nodes_message}"
        );
        let (map_code, map_message) =
            call_err(&client, "get_blockmap", json!({ "map": "MAP06" })).await;
        assert_eq!(map_code, "format_error", "an offset leaves the lump");
        assert!(
            map_message.contains("BLOCKMAP"),
            "the lump is named: {map_message}"
        );
        assert_eq!(
            call(&client, "get_reject", json!({ "map": "MAP06" })).await["size_ok"],
            json!(false),
            "the wrong reject size stays data, not an error",
        );
    }

    #[tokio::test]
    async fn the_nodes_tools_validate_their_shared_parameters() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        let (missing_code, _) = call_err(&client, "get_nodes", json!({ "map": "MAP99" })).await;
        assert_eq!(missing_code, "not_found", "no such block");
        let (scope_code, _) = call_err(&client, "get_reject", json!({ "scope": "nope" })).await;
        assert_eq!(scope_code, "invalid_argument", "not a scope");
        let (detail_code, detail_message) = call_err(
            &client,
            "get_blockmap",
            json!({ "map": "MAP05", "detail": "everything" }),
        )
        .await;
        assert_eq!(detail_code, "invalid_argument", "not a detail level");
        assert!(
            detail_message.contains("summary") && detail_message.contains("full"),
            "the message names both values: {detail_message}"
        );
        let (layer_code, _) = call_err(
            &client,
            "get_nodes",
            json!({ "map": "MAP05", "scope": "layer:9" }),
        )
        .await;
        assert_eq!(
            layer_code, "not_found",
            "an unloaded layer reports not_found"
        );
    }

    #[tokio::test]
    async fn the_active_map_selection_drives_the_default() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        call(&client, "set_active_map", json!({ "map": "MAP05" })).await;
        let report = call(&client, "get_nodes", json!({})).await;
        assert_eq!(report["map"], json!("MAP05"), "the active map");
        assert_eq!(report["format"], json!("vanilla"), "and its tree");
        call(&client, "set_active_map", json!({ "map": "MAP07" })).await;
        let reject = call(&client, "get_reject", json!({})).await;
        assert_eq!(reject["map"], json!("MAP07"), "the selection moved");
    }

    #[tokio::test]
    async fn build_blockmap_writes_a_grid_the_inspector_reads_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let built = call(&client, "build_blockmap", json!({ "map": "MAP01" })).await;
        assert_eq!(built["map"], json!("MAP01"), "the fragment");
        assert_eq!(
            built["copied_to_working"],
            json!(true),
            "the map came in to edit"
        );
        assert_eq!(
            built["nodes_invalidated"],
            json!(false),
            "a lump rewrite is not geometry"
        );
        assert_eq!(
            built["origin"]["x"],
            json!(-8_i64),
            "8 below the lowest vertex"
        );
        assert_eq!(built["origin"]["y"], json!(-8_i64), "on both axes");
        assert_eq!(built["columns"], json!(5_u64), "512 units span 5 cells");
        assert_eq!(built["rows"], json!(3_u64), "256 span 3");
        assert_eq!(built["vanilla_safe"], json!(true), "a toy map is tiny");
        let read = call(&client, "get_blockmap", json!({ "map": "MAP01" })).await;
        assert_eq!(
            read["bytes"], built["bytes"],
            "the merged view shows the build"
        );
        assert_eq!(read["columns"], json!(5_u64), "and agrees on the grid");
        assert_eq!(read["rows"], json!(3_u64));
        assert_eq!(read["blocks"], json!(15_u64), "5x3 cells");
        assert_eq!(
            read["total_entries"],
            json!(21_u64),
            "the two-room wall span"
        );
        assert_eq!(
            read["empty_blocks"],
            json!(2_u64),
            "the two interior corners"
        );
        let (code, _) = call_err(&client, "build_blockmap", json!({ "map": "MAP99" })).await;
        assert_eq!(code, "not_found", "no such block to build");
    }

    #[tokio::test]
    async fn blockmap_compression_only_changes_the_lump_size() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let packed = call(&client, "build_blockmap", json!({ "map": "MAP01" })).await;
        let plain = call(
            &client,
            "build_blockmap",
            json!({ "map": "MAP01", "compress": false }),
        )
        .await;
        assert_eq!(
            plain["copied_to_working"],
            json!(false),
            "already in the working copy"
        );
        assert!(
            plain["bytes"].as_u64().expect("a byte count")
                > packed["bytes"].as_u64().expect("a byte count"),
            "uncompressed copies every list: {} vs {}",
            plain["bytes"],
            packed["bytes"],
        );
        let working = call(
            &client,
            "get_blockmap",
            json!({ "map": "MAP01", "scope": "working" }),
        )
        .await;
        assert_eq!(working["bytes"], plain["bytes"], "the last build stands");
        assert_eq!(
            working["total_entries"],
            json!(21_u64),
            "with the same contents"
        );
    }

    #[tokio::test]
    async fn build_reject_writes_matrices_the_inspector_agrees_with() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let zero = call(&client, "build_reject", json!({ "map": "MAP01" })).await;
        assert_eq!(zero["mode"], json!("zero"), "the default mode");
        assert_eq!(zero["bytes"], json!(1_u64), "two sectors need one byte");
        assert_eq!(zero["sectors"], json!(2_u64), "the decoded count");
        assert_eq!(zero["rejected_pairs"], json!(0_u64), "nothing rejected");
        assert_eq!(
            zero["nodes_invalidated"],
            json!(false),
            "reject data is not geometry"
        );
        let read = call(&client, "get_reject", json!({ "map": "MAP01" })).await;
        assert_eq!(read["bytes"], json!(1_u64), "the written lump");
        assert_eq!(read["expected_bytes"], json!(1_u64), "at its expected size");
        assert_eq!(read["size_ok"], json!(true), "and the inspector agrees");
        assert_eq!(
            read["all_zero"],
            json!(true),
            "the clear matrix came back clear"
        );
        let parts = call(
            &client,
            "build_reject",
            json!({ "map": "MAP01", "mode": "COMPONENTS" }),
        )
        .await;
        assert_eq!(parts["mode"], json!("components"), "the echo");
        assert_eq!(
            parts["rejected_pairs"],
            json!(0_u64),
            "the step line joins both sectors, so nothing is rejected",
        );
        let connected = call(&client, "get_reject", json!({ "map": "MAP01" })).await;
        assert_eq!(connected["all_zero"], json!(true), "and the lump says so");
    }

    #[tokio::test]
    async fn builders_refuse_an_empty_map_and_an_unknown_mode() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let empty = pwad_empty_map(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": empty.display().to_string() }),
        )
        .await;
        let (code, message) = call_err(&client, "build_blockmap", json!({ "map": "MAP01" })).await;
        assert_eq!(code, "invalid_argument", "no vertices to grid");
        assert!(message.contains("vertices"), "the hint: {message}");
        let (mode_code, mode_message) = call_err(
            &client,
            "build_reject",
            json!({ "map": "MAP01", "mode": "visibility" }),
        )
        .await;
        assert_eq!(mode_code, "invalid_argument", "not a mode");
        assert!(
            mode_message.contains("zero") && mode_message.contains("components"),
            "the message names both values: {mode_message}"
        );
        let bare = call(&client, "build_reject", json!({ "map": "MAP01" })).await;
        assert_eq!(bare["sectors"], json!(0_u64), "no sectors");
        assert_eq!(bare["bytes"], json!(0_u64), "no bits to write");
        assert_eq!(bare["rejected_pairs"], json!(0_u64), "and nothing rejected");
    }
}
