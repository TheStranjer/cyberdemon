//! The `nodes` tool group as seen by a real MCP client: the three
//! inspection tools over the node-builder output (`get_nodes` for the
//! `SEGS`/`SSECTORS`/`NODES` tree in its vanilla and extended layouts,
//! `get_reject` for the visibility matrix, `get_blockmap` for the spatial
//! index), their empty-lump tolerance, their `format_error` refusals and
//! the `JSON` assets the `full` detail publishes, plus the three builders
//! (`build_blockmap`, `build_reject`, `build_nodes`) whose written lumps
//! the inspection tools then read back — `build_nodes` writing the whole
//! tree either as the vanilla run (its split vertices appended to
//! `VERTEXES` and stripped again by the next geometry edit), as one
//! `ZDoom` extended lump in `NODES`/`ZNODES` with the splits kept inside
//! the lump, or as the `glBSP` v2 `GL_*` block (classic run plus
//! miniseg-closed subsectors) attached right after the map block.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, iwad_l_room, pwad_empty_map, pwad_nodes};
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

    /// Load the L-room fixture (a concave one-sector room whose node
    /// build appends one split vertex) as layer 0.
    async fn l_room_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad_l_room(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
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

    /// Requires `get_nodes` to read `MAP01`'s fresh extended lump back:
    /// the given format, every count of the build report and the lump
    /// size it echoes.
    async fn assert_extended_read(client: &Client, built: &Value, format: &str) {
        let read = call(client, "get_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(read["format"], json!(format), "NODES carries the run");
        assert_eq!(read["segs"], built["segs"], "the reader agrees on segs");
        assert_eq!(read["subsectors"], built["subsectors"], "and subsectors");
        assert_eq!(read["nodes"], built["nodes"], "and nodes");
        assert_eq!(read["extra_vertices"], json!(0_u64), "nothing added here");
        assert_eq!(read["bytes"], built["bytes"], "the same lump");
        assert_eq!(read["root"], json!(0_u64), "the only node is the root");
        assert_eq!(read["depth"], json!(1_u64), "one level deep");
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

    #[tokio::test]
    async fn build_nodes_writes_vanilla_lumps_the_readers_confirm() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let built = call(&client, "build_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(built["variant"], json!("classic"), "the default variant");
        assert_eq!(
            built["segs"],
            json!(8_u64),
            "seven fronts and the shared back"
        );
        assert_eq!(built["subsectors"], json!(2_u64), "one leaf per room");
        assert_eq!(built["nodes"], json!(1_u64), "the shared line cuts once");
        assert_eq!(built["split_vertices"], json!(0_u64), "flush corners");
        assert_eq!(built["warnings"], json!([]), "each leaf bounds one sector");
        assert_eq!(built["map"], json!("MAP01"), "the fragment");
        assert_eq!(
            built["nodes_invalidated"],
            json!(false),
            "a build is a product, not a geometry change"
        );
        assert_eq!(
            built["copied_to_working"],
            json!(true),
            "the map came in to edit"
        );
        let read = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(read["format"], json!("vanilla"), "the record lumps");
        assert_eq!(read["segs"], built["segs"], "the reader agrees on segs");
        assert_eq!(read["subsectors"], built["subsectors"], "and subsectors");
        assert_eq!(read["nodes"], built["nodes"], "and nodes");
        assert_eq!(read["root"], json!(0_u64), "the only node is the root");
        assert_eq!(read["depth"], json!(1_u64), "one level deep");
        let summary = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
        assert_eq!(summary["nodes"]["present"], json!(true), "nodes are there");
        assert_eq!(summary["nodes"]["segs"], json!(8_u64), "SEGS over 12");
        assert_eq!(summary["nodes"]["nodes"], json!(1_u64), "NODES over 28");
        assert_eq!(
            summary["counts"]["vertices"],
            json!(6_u64),
            "this map splits nothing"
        );
    }

    #[tokio::test]
    async fn split_vertices_arrive_with_the_build_and_leave_with_the_nodes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = l_room_loaded(dir.path()).await;
        let built = call(&client, "build_nodes", json!({})).await;
        assert_eq!(built["map"], json!("MAP01"), "the only map is default");
        assert_eq!(
            built["segs"],
            json!(7_u64),
            "the cut doubles the bottom seg"
        );
        assert_eq!(built["subsectors"], json!(2_u64), "the arms are leaves");
        assert_eq!(built["nodes"], json!(1_u64), "one cut suffices");
        assert_eq!(
            built["split_vertices"],
            json!(1_u64),
            "the cut point had no vertex"
        );
        let summary = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
        assert_eq!(
            summary["counts"]["vertices"],
            json!(7_u64),
            "the split vertex joined VERTEXES"
        );
        assert_eq!(
            summary["nodes"]["present"],
            json!(true),
            "and the tree landed"
        );
    }

    #[tokio::test]
    async fn a_geometry_edit_strips_split_vertices_and_undo_restores_them() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = l_room_loaded(dir.path()).await;
        call(&client, "build_nodes", json!({})).await;
        let moved = call(
            &client,
            "move_vertex",
            json!({ "map": "MAP01", "index": 0_i64, "dx": 16_i64 }),
        )
        .await;
        assert_eq!(
            moved["nodes_invalidated"],
            json!(true),
            "moving a used vertex is geometry"
        );
        let after = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
        assert_eq!(
            after["counts"]["vertices"],
            json!(6_u64),
            "the split vertex was stripped with the invalidated nodes"
        );
        assert_eq!(after["nodes"]["present"], json!(false), "the run emptied");
        call(&client, "undo", json!({})).await;
        let restored = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
        assert_eq!(
            restored["counts"]["vertices"],
            json!(7_u64),
            "undo brings the split vertex back"
        );
        assert_eq!(restored["nodes"]["present"], json!(true), "and the tree");
        let nodes = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(nodes["format"], json!("vanilla"), "the undone build reads");
        assert_eq!(nodes["segs"], json!(7_u64), "with the cut segs");
    }

    #[tokio::test]
    async fn build_nodes_refuses_unknown_variants_unknown_maps_and_empty_geometry() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "znodes" }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "not a variant");
        assert!(
            message.contains("classic") && message.contains("extended") && message.contains("gl"),
            "the message names all three: {message}"
        );
        let (missing_code, _) = call_err(&client, "build_nodes", json!({ "map": "MAP99" })).await;
        assert_eq!(missing_code, "not_found", "no such block");
        let empty_dir = tempfile::tempdir().expect("a scratch directory");
        let empty_client = connect().await;
        let empty = pwad_empty_map(empty_dir.path());
        call(
            &empty_client,
            "open_iwad",
            json!({ "path": empty.display().to_string() }),
        )
        .await;
        let (no_code, no_message) =
            call_err(&empty_client, "build_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(no_code, "invalid_argument", "nothing to tree");
        assert!(
            no_message.contains("sidedefs"),
            "the hint names sides: {no_message}"
        );
        let (gl_code, gl_message) = call_err(
            &empty_client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "gl" }),
        )
        .await;
        assert_eq!(gl_code, "invalid_argument", "gl needs sides too");
        assert!(
            gl_message.contains("sidedefs"),
            "the hint names sides: {gl_message}"
        );
        let (scope_code, _) = call_err(
            &empty_client,
            "get_map_summary",
            json!({ "map": "MAP01", "scope": "working" }),
        )
        .await;
        assert_eq!(scope_code, "not_found", "the refusal wrote nothing");
    }

    #[tokio::test]
    async fn build_nodes_extended_writes_a_znod_lump_the_reader_confirms() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let built = call(
            &client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "EXTENDED" }),
        )
        .await;
        assert_eq!(built["variant"], json!("extended"), "the echo");
        assert_eq!(built["format"], json!("ZNOD"), "compressed by default");
        assert_eq!(
            built["segs"],
            json!(8_u64),
            "seven fronts and the shared back"
        );
        assert_eq!(built["subsectors"], json!(2_u64), "one leaf per room");
        assert_eq!(built["nodes"], json!(1_u64), "the shared line cuts once");
        assert_eq!(built["new_vertices"], json!(0_u64), "flush corners");
        assert_eq!(built["warnings"], json!([]), "each leaf bounds one sector");
        assert_eq!(built["map"], json!("MAP01"), "the fragment");
        assert_eq!(
            built["nodes_invalidated"],
            json!(false),
            "a build is a product, not a geometry change"
        );
        assert_eq!(
            built["copied_to_working"],
            json!(true),
            "the map came in to edit"
        );
        assert!(
            built.get("split_vertices").is_none(),
            "the classic-only key stays out of the extended result: {built}"
        );
        let bytes = built["bytes"].as_u64().expect("a lump size");
        assert!(bytes > 4, "magic plus a payload: {bytes}");
        assert_extended_read(&client, &built, "znod").await;
        let summary = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
        assert_eq!(
            summary["counts"]["vertices"],
            json!(6_u64),
            "an extended build never touches VERTEXES"
        );
        assert_eq!(summary["nodes"]["present"], json!(true), "nodes are there");
        assert_eq!(
            summary["nodes"]["nodes_format"],
            json!("ZNOD"),
            "the summary reports the magic",
        );
        assert!(
            summary["nodes"].get("segs").is_none(),
            "the vanilla counts give way to it: {summary}"
        );
    }

    #[tokio::test]
    async fn extended_compression_only_changes_the_lump_not_the_records() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        call(
            &client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "extended" }),
        )
        .await;
        let packed = call(
            &client,
            "get_nodes",
            json!({ "map": "MAP01", "detail": "full" }),
        )
        .await;
        assert_eq!(packed["format"], json!("znod"), "the compressed lump");
        let packed_body = fetch_json(&packed).await;
        let raw = call(
            &client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "extended", "compress": false }),
        )
        .await;
        assert_eq!(raw["format"], json!("XNOD"), "compress false stores raw");
        assert_eq!(
            raw["copied_to_working"],
            json!(false),
            "already in the working copy"
        );
        let open = call(
            &client,
            "get_nodes",
            json!({ "map": "MAP01", "detail": "full" }),
        )
        .await;
        assert_eq!(open["format"], json!("xnod"), "the raw magic reads back");
        let open_body = fetch_json(&open).await;
        assert_eq!(open_body["format"], json!("xnod"), "the asset layout");
        assert_eq!(
            packed_body["format"],
            json!("znod"),
            "and its compressed twin"
        );
        assert!(
            ["extra_vertices", "segs", "subsectors", "nodes"]
                .iter()
                .all(|field| open_body[*field] == packed_body[*field]),
            "compressed and uncompressed decode to the same records"
        );
    }

    #[tokio::test]
    async fn extended_build_on_udmf_writes_znodes_and_classic_refuses_there() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = nodes_loaded(dir.path()).await;
        let built = call(
            &client,
            "build_nodes",
            json!({ "map": "MAP07", "variant": "extended" }),
        )
        .await;
        assert_eq!(built["variant"], json!("extended"), "the echo");
        assert_eq!(built["format"], json!("ZNOD"), "compressed by default");
        assert_eq!(built["segs"], json!(8_u64), "the two-room map splits once");
        assert_eq!(built["subsectors"], json!(2_u64), "one leaf per room");
        assert_eq!(built["new_vertices"], json!(0_u64), "flush corners");
        let working = call(
            &client,
            "get_nodes",
            json!({ "map": "MAP07", "scope": "working" }),
        )
        .await;
        assert_eq!(
            working["format"],
            json!("znod"),
            "ZNODES holds the fresh run"
        );
        assert_eq!(working["segs"], built["segs"], "the reader agrees");
        assert_eq!(
            working["bytes"], built["bytes"],
            "the block's ZNODES replaced the fixture's XNOD"
        );
        let merged = call(&client, "get_nodes", json!({ "map": "MAP07" })).await;
        assert_eq!(
            merged["format"],
            json!("znod"),
            "the merged view shows the build"
        );
        let (code, message) = call_err(&client, "build_nodes", json!({ "map": "MAP07" })).await;
        assert_eq!(code, "invalid_argument", "classic has no place in UDMF");
        assert!(
            message.contains("UDMF maps use extended nodes"),
            "the message names the rule: {message}"
        );
        let (gl_code, gl_message) = call_err(
            &client,
            "build_nodes",
            json!({ "map": "MAP07", "variant": "gl" }),
        )
        .await;
        assert_eq!(gl_code, "invalid_argument", "gl writes the classic run too");
        assert!(
            gl_message.contains("UDMF maps use extended nodes"),
            "the same rule: {gl_message}"
        );
        let after = call(
            &client,
            "get_nodes",
            json!({ "map": "MAP07", "scope": "working" }),
        )
        .await;
        assert_eq!(after["format"], json!("znod"), "the refusal wrote nothing");
    }

    /// The name column of one scope's directory.
    async fn directory_names(client: &Client, scope: &str) -> Vec<String> {
        let listing = call(
            client,
            "list_lumps",
            json!({ "scope": scope, "limit": 2000_i64 }),
        )
        .await;
        listing["lumps"]
            .as_array()
            .expect("a lump list")
            .iter()
            .map(|row| row["name"].as_str().expect("a name").to_owned())
            .collect()
    }

    #[tokio::test]
    async fn build_nodes_gl_writes_classic_lumps_and_the_readers_agree() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let built = call(
            &client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "gl" }),
        )
        .await;
        assert_eq!(built["variant"], json!("gl"), "the echo");
        assert_eq!(
            built["segs"],
            json!(8_u64),
            "the classic run is written as well"
        );
        assert_eq!(built["subsectors"], json!(2_u64), "one leaf per room");
        assert_eq!(built["nodes"], json!(1_u64), "the shared line cuts once");
        assert_eq!(built["split_vertices"], json!(0_u64), "flush corners");
        assert_eq!(
            built["gl_segs"],
            json!(8_u64),
            "every edge holds a real seg"
        );
        assert_eq!(
            built["minisegs"],
            json!(0_u64),
            "the shared line is covered both sides"
        );
        assert_eq!(
            built["gl_vertices"],
            json!(0_u64),
            "the GL run needs no fraction points"
        );
        assert_eq!(
            built["nodes_invalidated"],
            json!(false),
            "a build is a product"
        );
        assert_eq!(
            built["copied_to_working"],
            json!(true),
            "the map came in to edit"
        );
        let maps = call(&client, "list_maps", json!({ "scope": "working" })).await;
        assert_eq!(
            maps["maps"][0]["has_gl_nodes"],
            json!(true),
            "list_maps sees the run"
        );
        assert_gl_read_back(&client, &built).await;
    }

    /// Requires `get_nodes` on `MAP01` to read the fresh `glBSP` v2 run:
    /// the classic records still parsed and the GL block counted.
    async fn assert_gl_read_back(client: &Client, built: &Value) {
        let read = call(client, "get_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(
            read["format"],
            json!("vanilla"),
            "the classic run still reads"
        );
        assert_eq!(
            read["gl"]["present"],
            json!(true),
            "and the GL block rides along"
        );
        assert_eq!(read["gl"]["magic"], json!("gNd2"), "the v2 magic");
        assert_eq!(read["gl"]["segs"], built["gl_segs"], "GL_SEGS rows counted");
        assert_eq!(read["gl"]["subsectors"], json!(2_u64), "GL_SSECT rows");
        assert_eq!(read["gl"]["nodes"], json!(1_u64), "GL_NODES rows");
        assert_eq!(read["gl"]["vertices"], built["gl_vertices"], "GL_VERT rows");
    }

    #[tokio::test]
    async fn the_gl_block_sits_after_the_map_and_survives_rebuilds_and_undo() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        call(
            &client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "gl" }),
        )
        .await;
        let names = directory_names(&client, "working").await;
        let map_at = names.iter().position(|n| n == "MAP01").expect("the block");
        let gl_at = names
            .iter()
            .position(|n| n == "GL_MAP01")
            .expect("the marker");
        let members = map_at
            .checked_add(11)
            .expect("ten members after the header");
        assert_eq!(gl_at, members, "the run sits right after the map block");
        let end = gl_at.checked_add(6).expect("six run lumps");
        assert_eq!(
            names[gl_at..end],
            [
                "GL_MAP01", "GL_VERT", "GL_SEGS", "GL_SSECT", "GL_NODES", "GL_PVS"
            ],
            "the run in glBSP order"
        );
        let again = call(
            &client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "gl" }),
        )
        .await;
        assert_eq!(again["copied_to_working"], json!(false), "already editable");
        assert_eq!(
            directory_names(&client, "working").await,
            names,
            "a rebuild replaces, never duplicates"
        );
        call(&client, "undo", json!({})).await;
        assert_eq!(
            directory_names(&client, "working").await,
            names,
            "the first build stands"
        );
        call(&client, "undo", json!({})).await;
        let gone = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(
            gone["gl"]["present"],
            json!(false),
            "undo took the block away"
        );
        assert_eq!(gone["format"], json!("none"), "and the classic run with it");
    }

    #[tokio::test]
    async fn gl_minisegs_close_the_uncovered_partition_of_an_l_room() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = l_room_loaded(dir.path()).await;
        let built = call(
            &client,
            "build_nodes",
            json!({ "map": "MAP01", "variant": "gl" }),
        )
        .await;
        assert_eq!(built["segs"], json!(7_u64), "the classic count");
        assert_eq!(
            built["gl_segs"],
            json!(9_u64),
            "seven real segs plus two minisegs"
        );
        assert_eq!(
            built["minisegs"],
            json!(2_u64),
            "one per side of the doorway plane"
        );
        assert_eq!(
            built["gl_vertices"],
            json!(0_u64),
            "the classic run's split vertex is reused"
        );
        let read = call(
            &client,
            "get_nodes",
            json!({ "map": "MAP01", "scope": "working" }),
        )
        .await;
        assert_eq!(
            read["gl"]["segs"],
            json!(9_u64),
            "the reader counts them back"
        );
        assert_eq!(read["gl"]["subsectors"], json!(2_u64), "two closed leaves");
    }

    #[tokio::test]
    async fn a_built_map_survives_property_edits_and_reports_itself_undoable() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        call(&client, "build_nodes", json!({ "map": "MAP01" })).await;
        let lighted = call(
            &client,
            "edit_sector",
            json!({ "map": "MAP01", "index": 0_i64, "light": 200_i64 }),
        )
        .await;
        assert_eq!(
            lighted["nodes_invalidated"],
            json!(false),
            "a property edit keeps the fresh tree"
        );
        let kept = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(kept["format"], json!("vanilla"), "still vanilla records");
        assert_eq!(kept["segs"], json!(8_u64), "with every seg");
        call(&client, "undo", json!({})).await;
        let undone = call(&client, "undo", json!({})).await;
        let gone = call(&client, "get_nodes", json!({ "map": "MAP01" })).await;
        assert_eq!(
            gone["format"],
            json!("none"),
            "undoing the build erases the run"
        );
        assert_eq!(undone["redo_available"], json!(2_u64), "two steps to redo");
    }
}
