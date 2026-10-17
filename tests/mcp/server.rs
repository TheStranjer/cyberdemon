//! Server behavior as seen by a real MCP client: identity, tool listing,
//! the `version` tool, protocol errors, and the real binary over stdio.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::process::Stdio;

    use rmcp::ServiceExt as _;
    use rmcp::model::{CallToolRequestParams, ErrorCode, Tool};
    use rmcp::service::ServiceError;
    use rmcp::transport::TokioChildProcess;
    use serde_json::{Value, json};
    use tokio::process::Command;

    use crate::support::{call, call_err, connect, connect_probe};

    const EXPECTED_VERSION: &str = concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"));

    #[tokio::test]
    async fn initialize_reports_identity_and_tools_capability() {
        let client = connect().await;
        let info = client.peer_info().expect("server info after initialize");
        let server_info = info.server_info.as_ref().expect("server implementation");
        assert_eq!(server_info.name, "cyberdemon", "server name");
        assert_eq!(
            server_info.version,
            env!("CARGO_PKG_VERSION"),
            "server version"
        );
        assert!(info.capabilities.tools.is_some(), "tools capability");
        assert_eq!(
            info.instructions.as_deref(),
            Some("Cyberdemon reads and edits Doom WAD files."),
            "instructions"
        );
    }

    /// The listed tool named `wanted`.
    fn tool_named<'tools>(tools: &'tools [Tool], wanted: &str) -> &'tools Tool {
        tools
            .iter()
            .find(|tool| tool.name == wanted)
            .unwrap_or_else(|| panic!("{wanted} is listed"))
    }

    /// One tool's input schema as a JSON value.
    fn schema_of(tool: &Tool) -> Value {
        serde_json::to_value(&tool.input_schema).expect("a serializable schema")
    }

    /// Requires a schema to flatten every one of `fields` into its
    /// properties object.
    fn assert_flattens(schema: &Value, fields: &[&str]) {
        let properties = schema.get("properties").expect("a properties object");
        let missing: Vec<&str> = fields
            .iter()
            .filter(|field| properties.get(**field).is_none())
            .copied()
            .collect();
        assert!(missing.is_empty(), "the schema lacks {missing:?}: {schema}");
    }

    /// Requires a schema to keep `field` in its required list.
    fn assert_required(schema: &Value, field: &str) {
        let holds = schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|fields| fields.iter().any(|named| named == field));
        assert!(holds, "{field:?} stays required: {schema}");
    }

    /// Requires a schema to list no required field at all: the tool decides
    /// which combination of its optional arguments is valid.
    fn assert_nothing_required(schema: &Value) {
        assert!(
            schema.get("required").is_none(),
            "every argument is optional and validated together: {schema}"
        );
    }

    /// Every tool the server lists, sorted by name.
    const EXPECTED_TOOLS: [&str; 91] = [
        "add_lump",
        "add_sector",
        "add_sidedef",
        "add_thing",
        "align_textures",
        "assemble_map",
        "auto_texture",
        "build_blockmap",
        "build_reject",
        "build_texture",
        "convert_map_format",
        "copy_map",
        "create_map",
        "decode_patch",
        "diff_wads",
        "edit_decorate",
        "edit_dehacked",
        "edit_pnames",
        "edit_sector",
        "edit_sidedef",
        "edit_thing",
        "edit_zscript",
        "extract_lump",
        "get_asset_server_info",
        "get_asset_url",
        "get_blockmap",
        "get_dehacked",
        "get_flat",
        "get_linedef_specials",
        "get_lump",
        "get_map_lumps",
        "get_map_summary",
        "get_nodes",
        "get_palette",
        "get_patch",
        "get_reject",
        "get_render_settings",
        "get_sector_specials",
        "get_session_info",
        "get_sprite",
        "get_texture_theme",
        "get_thing_sprite",
        "get_thing_types",
        "get_wad_info",
        "import_asset",
        "import_file_as_lump",
        "import_image_as_flat",
        "import_image_as_patch",
        "import_image_as_raw",
        "import_image_as_sprite",
        "import_music",
        "import_png_as_flat",
        "import_png_as_patch",
        "import_png_as_sprite",
        "import_sound",
        "list_flats",
        "list_lumps",
        "list_maps",
        "list_patches",
        "list_pnames",
        "list_sprites",
        "list_textures",
        "merge_wad",
        "new_wad",
        "open_iwad",
        "open_wad",
        "preview_music",
        "preview_sound",
        "redo",
        "remove_lump",
        "remove_map",
        "remove_sector",
        "remove_sidedef",
        "remove_texture",
        "remove_thing",
        "rename_lump",
        "rename_map",
        "render_topdown",
        "reorder_lumps",
        "replace_lump",
        "run_batch",
        "save_wad",
        "search_lumps",
        "set_active_map",
        "set_asset_capacity",
        "set_palette",
        "set_render_options",
        "set_render_resolution",
        "set_texture_theme",
        "undo",
        "version",
    ];

    #[tokio::test]
    async fn lists_the_grouped_tools_of_every_router() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        names.sort_unstable();
        assert_eq!(names, EXPECTED_TOOLS.to_vec(), "sorted tool names");
    }

    #[tokio::test]
    async fn the_lump_reader_schemas_flatten_their_selector_fields() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let get_lump = schema_of(tool_named(&tools, "get_lump"));
        assert_flattens(
            &get_lump,
            &[
                "name",
                "scope",
                "namespace",
                "occurrence",
                "format",
                "start",
                "length",
                "max_chars",
            ],
        );
        assert_required(&get_lump, "name");
        let extract_lump = schema_of(tool_named(&tools, "extract_lump"));
        assert_flattens(
            &extract_lump,
            &[
                "name",
                "scope",
                "namespace",
                "occurrence",
                "path",
                "overwrite",
            ],
        );
        assert_required(&extract_lump, "name");
        assert_required(&extract_lump, "path");
        let preview_sound = schema_of(tool_named(&tools, "preview_sound"));
        assert_flattens(
            &preview_sound,
            &["name", "scope", "namespace", "occurrence", "bits"],
        );
        assert_required(&preview_sound, "name");
        let preview_music = schema_of(tool_named(&tools, "preview_music"));
        assert_flattens(
            &preview_music,
            &["name", "scope", "namespace", "occurrence"],
        );
        assert_required(&preview_music, "name");
    }

    /// Requires a picture tool's schema to flatten the shared `palette`,
    /// `format` and `scale` options and still require its `name`.
    fn assert_picture_options(tool: &Tool) {
        let schema = schema_of(tool);
        assert_flattens(&schema, &["name", "palette", "format", "scale"]);
        assert_required(&schema, "name");
    }

    #[tokio::test]
    async fn the_picture_tool_schemas_flatten_their_shared_options() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        assert_picture_options(tool_named(&tools, "get_flat"));
        assert_picture_options(tool_named(&tools, "get_patch"));
        let sprite = schema_of(tool_named(&tools, "get_sprite"));
        assert_flattens(
            &sprite,
            &[
                "name", "sprite", "frame", "rotation", "palette", "format", "scale",
            ],
        );
        assert_nothing_required(&sprite);
        let thing = schema_of(tool_named(&tools, "get_thing_sprite"));
        assert_flattens(
            &thing,
            &["type", "frame", "rotation", "palette", "format", "scale"],
        );
        assert_required(&thing, "type");
        let decoded = schema_of(tool_named(&tools, "decode_patch"));
        assert_flattens(&decoded, &["name", "source", "palette"]);
        assert_nothing_required(&decoded);
    }

    /// The image imports flatten their parameters into the tool schema, and
    /// each alias carries its tool's schema exactly.
    #[tokio::test]
    async fn the_image_import_schemas_flatten_their_shared_parameters() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let patch = schema_of(tool_named(&tools, "import_image_as_patch"));
        assert_flattens(
            &patch,
            &[
                "source",
                "name",
                "palette",
                "transparency",
                "offsets",
                "replace",
                "add_to_pnames",
            ],
        );
        assert_required(&patch, "source");
        assert_eq!(
            schema_of(tool_named(&tools, "import_png_as_patch")),
            patch,
            "the alias takes the same arguments"
        );
        let flat = schema_of(tool_named(&tools, "import_image_as_flat"));
        assert_flattens(
            &flat,
            &["source", "name", "palette", "resize", "size", "replace"],
        );
        assert_required(&flat, "source");
        assert_eq!(
            schema_of(tool_named(&tools, "import_png_as_flat")),
            flat,
            "the flat alias takes the same arguments"
        );
        let sprite = schema_of(tool_named(&tools, "import_image_as_sprite"));
        assert_flattens(
            &sprite,
            &[
                "source",
                "name",
                "palette",
                "transparency",
                "offsets",
                "replace",
                "sprite",
                "frame",
                "rotation",
                "mirror_rotation",
            ],
        );
        assert_required(&sprite, "source");
        assert_eq!(
            schema_of(tool_named(&tools, "import_png_as_sprite")),
            sprite,
            "the sprite alias takes the same arguments"
        );
        let raw = schema_of(tool_named(&tools, "import_image_as_raw"));
        assert_flattens(&raw, &["source", "name", "namespace", "replace"]);
        assert_required(&raw, "source");
    }

    #[tokio::test]
    async fn add_lump_flattens_its_payload_and_placement_fields() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let add_lump = schema_of(tool_named(&tools, "add_lump"));
        assert_flattens(
            &add_lump,
            &[
                "name",
                "text",
                "hex",
                "source",
                "index",
                "before",
                "after",
                "namespace",
                "allow_duplicate",
            ],
        );
        assert_required(&add_lump, "name");
    }

    #[tokio::test]
    async fn get_asset_url_flattens_the_selector_and_carries_its_two_choices() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let schema = schema_of(tool_named(&tools, "get_asset_url"));
        assert_flattens(
            &schema,
            &[
                "name",
                "scope",
                "namespace",
                "occurrence",
                "palette",
                "prefer",
            ],
        );
        assert_required(&schema, "name");
    }

    #[tokio::test]
    async fn the_three_lump_editors_require_their_own_fields() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let replace = schema_of(tool_named(&tools, "replace_lump"));
        assert_flattens(
            &replace,
            &["name", "namespace", "occurrence", "text", "hex", "source"],
        );
        assert_required(&replace, "name");
        let remove = schema_of(tool_named(&tools, "remove_lump"));
        assert_flattens(
            &remove,
            &["names", "namespace", "occurrence", "all_occurrences"],
        );
        assert_required(&remove, "names");
        let rename = schema_of(tool_named(&tools, "rename_lump"));
        assert_flattens(
            &rename,
            &[
                "name",
                "new_name",
                "namespace",
                "occurrence",
                "allow_duplicate",
            ],
        );
        assert_required(&rename, "name");
        assert_required(&rename, "new_name");
    }

    #[tokio::test]
    async fn reorder_lumps_flattens_its_run_and_placement_fields() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let reorder = schema_of(tool_named(&tools, "reorder_lumps"));
        assert_flattens(
            &reorder,
            &[
                "name",
                "from_namespace",
                "occurrence",
                "from_index",
                "count",
                "index",
                "before",
                "after",
                "namespace",
            ],
        );
        assert_nothing_required(&reorder);
    }

    #[tokio::test]
    async fn the_two_import_tools_share_one_schema_requiring_source() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let import_asset = schema_of(tool_named(&tools, "import_asset"));
        assert_flattens(
            &import_asset,
            &[
                "source",
                "name",
                "index",
                "before",
                "after",
                "namespace",
                "allow_duplicate",
            ],
        );
        assert_required(&import_asset, "source");
        assert_eq!(
            schema_of(tool_named(&tools, "import_file_as_lump")),
            import_asset,
            "the two import names share one contract"
        );
    }

    #[tokio::test]
    async fn version_reports_name_and_version() {
        let client = connect().await;
        let result = client
            .call_tool(CallToolRequestParams::new("version"))
            .await
            .expect("call version");
        assert_ne!(result.is_error, Some(true), "is_error");
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str());
        assert_eq!(text, Some(EXPECTED_VERSION), "version text");
    }

    #[tokio::test]
    async fn unknown_tool_is_a_protocol_error() {
        let client = connect().await;
        let err = client
            .call_tool(CallToolRequestParams::new("no_such_tool"))
            .await
            .expect_err("unknown tool must fail");
        assert!(
            matches!(&err, ServiceError::McpError(e) if e.code == ErrorCode::INVALID_PARAMS),
            "expected INVALID_PARAMS McpError, got {err:?}"
        );
    }

    /// Exercises `main.rs`: spawns the real binary and speaks MCP over its stdio.
    #[tokio::test]
    async fn binary_serves_mcp_over_stdio() {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyberdemon"));
        command.stderr(Stdio::null());
        let transport = TokioChildProcess::new(command).expect("spawn binary");
        let mut client = tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
            .await
            .expect("initialize timed out")
            .expect("initialize");
        let info = client.peer_info().expect("server info");
        let name = info.server_info.as_ref().map(|i| i.name.as_str());
        assert_eq!(name, Some("cyberdemon"), "server name over stdio");
        let result = client
            .call_tool(CallToolRequestParams::new("version"))
            .await
            .expect("call version");
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str());
        assert_eq!(text, Some(EXPECTED_VERSION), "version text over stdio");
        client.close().await.expect("close");
    }

    #[tokio::test]
    async fn call_returns_structured_success_content() {
        let client = connect_probe().await;
        let value = call(&client, "echo", json!({ "message": "E1M1" })).await;
        assert_eq!(value, json!({ "echo": "E1M1" }), "echo structured content");
    }

    #[tokio::test]
    async fn call_err_returns_the_documented_error_pair() {
        let client = connect_probe().await;
        let (code, message) = call_err(&client, "missing", Value::Null).await;
        assert_eq!(code, "not_found", "error code field");
        assert_eq!(message, "probe lump does not exist", "message field");
    }
}
