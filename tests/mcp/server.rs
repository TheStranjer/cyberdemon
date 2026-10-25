//! Server behavior as seen by a real MCP client: identity, tool listing,
//! the `version` tool, protocol errors, the real binary over stdio, the
//! round trip of every `support` picture helper, and the raw structure of
//! the ruled photo sheet fixture.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::process::Stdio;

    use cyberdemon::gfx::codec::decode_image;
    use cyberdemon::gfx::image::RgbaImage;
    use rmcp::ServiceExt as _;
    use rmcp::model::{CallToolRequestParams, ErrorCode, Tool};
    use rmcp::service::ServiceError;
    use rmcp::transport::TokioChildProcess;
    use serde_json::{Value, json};
    use tokio::process::Command;

    use crate::fixtures::iwad;
    use crate::support::{
        PHOTO_INK, PHOTO_ROOM, PHOTO_RULING, PHOTO_RULING_PERIOD, PHOTO_RULING_PHASE,
        PHOTO_SHADOW_ROWS, PHOTO_TICK, PHOTO_WALL_PX, asset_png, call, call_err, connect,
        connect_probe, fill_rect, paper_sheet, ruled_photo_sheet, write_png,
    };

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
        let instructions = info
            .instructions
            .as_deref()
            .expect("server instructions after initialize");
        assert!(
            instructions.starts_with("Cyberdemon reads and edits Doom WAD files over MCP"),
            "instructions open with what the server does: {instructions}"
        );
        assert!(
            instructions.lines().count() <= 25,
            "the instructions stay at most 25 lines: {}",
            instructions.lines().count()
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
    const EXPECTED_TOOLS: [&str; 135] = [
        "add_door",
        "add_exit",
        "add_lift",
        "add_linedef",
        "add_lump",
        "add_patch",
        "add_sector",
        "add_sidedef",
        "add_stairs",
        "add_switch",
        "add_teleporter",
        "add_thing",
        "add_vertex",
        "add_window",
        "align_textures",
        "assemble_map",
        "auto_texture",
        "build_all_nodes",
        "build_blockmap",
        "build_map_from_plan",
        "build_nodes",
        "build_reject",
        "build_texture",
        "bulk_edit",
        "carve_void",
        "check_playability",
        "convert_map_format",
        "copy_map",
        "create_flat_from_image",
        "create_map",
        "create_sector_from_loop",
        "create_sprite_from_image",
        "create_texture_from_image",
        "decode_patch",
        "delete_geometry",
        "describe_map",
        "diff_wads",
        "edit_decorate",
        "edit_dehacked",
        "edit_linedef",
        "edit_pnames",
        "edit_sector",
        "edit_sidedef",
        "edit_thing",
        "edit_zscript",
        "extract_lump",
        "find_elements",
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
        "join_linedefs",
        "list_flats",
        "list_lumps",
        "list_maps",
        "list_patches",
        "list_pnames",
        "list_sprites",
        "list_textures",
        "mark_secret",
        "merge_wad",
        "move_vertex",
        "new_wad",
        "open_iwad",
        "open_wad",
        "optimize_wad",
        "place_things",
        "preview_music",
        "preview_sound",
        "process_image",
        "redo",
        "remove_linedef",
        "remove_lump",
        "remove_map",
        "remove_patch",
        "remove_sector",
        "remove_sidedef",
        "remove_texture",
        "remove_thing",
        "remove_vertex",
        "rename_lump",
        "rename_map",
        "render_minimap",
        "render_topdown",
        "render_view",
        "reorder_lumps",
        "replace_flat",
        "replace_lump",
        "replace_patch",
        "replace_texture",
        "replace_thing_type",
        "run_batch",
        "save_wad",
        "search_lumps",
        "set_active_map",
        "set_asset_capacity",
        "set_light_effect",
        "set_palette",
        "set_render_options",
        "set_render_resolution",
        "set_texture_theme",
        "split_linedef",
        "trace_sketch",
        "transform_selection",
        "undo",
        "validate_map",
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

    /// `create_flat_from_image`' schema carries its own `size` square-side
    /// selector beside the flattened `dither`/`ops`/`palette` trio and
    /// keeps `name` required.
    fn assert_flat_creator_schema(tool: &Tool) {
        let created = schema_of(tool);
        assert_flattens(
            &created,
            &[
                "source", "name", "fit", "size", "tileable", "ops", "dither", "palette",
            ],
        );
        assert_required(&created, "name");
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
            &[
                "source", "name", "palette", "resize", "size", "dither", "replace",
            ],
        );
        assert_required(&flat, "source");
        assert_eq!(
            schema_of(tool_named(&tools, "import_png_as_flat")),
            flat,
            "the flat alias takes the same arguments"
        );
        assert_flat_creator_schema(tool_named(&tools, "create_flat_from_image"));
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

    /// `add_patch` flattens the picture parameters into its schema and
    /// keeps both its own `name` and the flattened `source` required.
    #[tokio::test]
    async fn add_patch_flattens_the_picture_parameters_and_keeps_its_requireds() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let add_patch = schema_of(tool_named(&tools, "add_patch"));
        assert_flattens(
            &add_patch,
            &[
                "name",
                "source",
                "offsets",
                "palette",
                "transparency",
                "add_to_pnames",
            ],
        );
        assert_required(&add_patch, "name");
        assert_required(&add_patch, "source");
    }

    /// `replace_patch` flattens the same picture parameters, keeps both
    /// required fields - and carries neither `add_to_pnames` nor `replace`:
    /// the list is never touched and the overwrite is what the tool does.
    #[tokio::test]
    async fn replace_patch_flattens_the_picture_parameters_and_drops_the_other_flags() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let replace = schema_of(tool_named(&tools, "replace_patch"));
        assert_flattens(
            &replace,
            &["name", "source", "offsets", "palette", "transparency"],
        );
        assert_required(&replace, "name");
        assert_required(&replace, "source");
        let properties = replace.get("properties").expect("a properties object");
        assert!(
            properties.get("add_to_pnames").is_none(),
            "PNAMES is never written: {replace}"
        );
        assert!(
            properties.get("replace").is_none(),
            "the tool always replaces: {replace}"
        );
    }

    /// `remove_patch` takes exactly `name` (required), `remove_from_pnames`
    /// and `force`: no picture side (nothing comes in) and no `replace`.
    #[tokio::test]
    async fn remove_patch_takes_exactly_its_name_and_the_two_removal_flags() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let remove = schema_of(tool_named(&tools, "remove_patch"));
        assert_flattens(&remove, &["name", "remove_from_pnames", "force"]);
        assert_required(&remove, "name");
        let properties = remove.get("properties").expect("a properties object");
        assert_eq!(
            properties.as_object().expect("an object").len(),
            3_usize,
            "exactly the three documented fields: {remove}"
        );
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
    async fn the_two_sector_special_tools_require_their_selection() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let secret = schema_of(tool_named(&tools, "mark_secret"));
        assert_flattens(&secret, &["map", "sectors", "secret"]);
        assert_required(&secret, "sectors");
        let light = schema_of(tool_named(&tools, "set_light_effect"));
        assert_flattens(&light, &["map", "sectors", "effect", "light"]);
        assert_required(&light, "sectors");
        assert_required(&light, "effect");
        let warning = tool_named(&tools, "mark_secret")
            .description
            .as_deref()
            .expect("a description");
        assert!(
            warning.contains("vanilla Doom ignores Boom bits"),
            "the description carries the exact Boom warning: {warning}"
        );
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
        let (code, message, _) = call_err(&client, "missing", Value::Null).await;
        assert_eq!(code, "not_found", "error code field");
        assert_eq!(message, "probe lump does not exist", "message field");
    }

    /// Exercises every `support` picture helper in one round trip: build
    /// a paper sheet, paint a rectangle, write it as a `PNG`, decode the
    /// bytes back, and fetch a real published asset with `asset_png`.
    #[tokio::test]
    async fn the_picture_support_helpers_round_trip() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let mut sheet = paper_sheet(8_u32, 4_u32);
        fill_rect(&mut sheet, (2_u32, 1_u32, 6_u32, 3_u32), [9_u8, 8, 7, 255]);
        let source = write_png(dir.path(), "HELPER.png", &sheet);
        let bytes = std::fs::read(&source).expect("the helper PNG is on disk");
        let image = decode_image(&bytes).expect("the written PNG decodes");
        assert_eq!(
            (image.width, image.height),
            (8_u32, 4_u32),
            "the written shape"
        );
        assert_eq!(
            image.get(3, 2),
            Some([9_u8, 8, 7, 255]),
            "the painted block"
        );
        assert_eq!(
            image.get(0, 0),
            Some([255_u8, 255, 255, 255]),
            "the paper outside the block"
        );
        let client = connect().await;
        let base = iwad(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let flat = call(&client, "get_flat", json!({ "name": "FLOOR1" })).await;
        let url = flat["asset"]["url"]
            .as_str()
            .expect("get_flat publishes an asset");
        let fetched = asset_png(url).await;
        assert_eq!(
            (fetched.width, fetched.height),
            (64_u32, 64_u32),
            "the fixture flat's shape over HTTP"
        );
        assert_eq!(
            fetched.get(63, 63),
            Some([1_u8, 254, 1, 255]),
            "the fixture flat's colour over HTTP"
        );
    }

    /// The `(299 r + 587 g + 114 b) / 1000` luminance of a pixel.
    fn luminance(pixel: [u8; 4]) -> u32 {
        let red = 299_u32
            .checked_mul(u32::from(pixel[0]))
            .expect("a test product");
        let green = 587_u32
            .checked_mul(u32::from(pixel[1]))
            .expect("a test product");
        let blue = 114_u32
            .checked_mul(u32::from(pixel[2]))
            .expect("a test product");
        red.checked_add(green)
            .expect("a test sum")
            .checked_add(blue)
            .expect("a test sum")
            .checked_div(1_000_u32)
            .expect("the divisor is constant")
    }

    /// The sheet's shadow factor at `row`: `255 - (160 - row)` on the
    /// ramp and 255 below it.
    fn shadow_factor(row: u32) -> u32 {
        255_u32
            .checked_sub(PHOTO_SHADOW_ROWS.saturating_sub(row))
            .expect("the ramp fits")
    }

    /// `base` through the sheet's shadow at `row`: every colour channel
    /// times `shadow_factor(row) / 255`, the alpha byte kept.
    fn shaded(base: [u8; 4], row: u32) -> [u8; 4] {
        let factor = shadow_factor(row);
        let channel = |value: u8| {
            u8::try_from(
                u32::from(value)
                    .checked_mul(factor)
                    .expect("a test product")
                    .checked_div(255_u32)
                    .expect("the divisor is constant"),
            )
            .expect("a scaled channel fits a u8")
        };
        [
            channel(base[0]),
            channel(base[1]),
            channel(base[2]),
            base[3],
        ]
    }

    /// Down a paper column, the rows at the ruling's phase and every
    /// `PHOTO_RULING_PERIOD` after carry the shadowed blue exactly, and
    /// every row between carries shadowed paper exactly.
    fn assert_ruling(sheet: &RgbaImage, column: u32) {
        let paper: [u8; 4] = [255_u8, 255, 255, 255];
        let height = usize::try_from(sheet.height).expect("a test height");
        let phase = usize::try_from(PHOTO_RULING_PHASE).expect("a test phase");
        let period = usize::try_from(PHOTO_RULING_PERIOD).expect("a test period");
        assert!(phase < height, "the sheet carries ruled rows");
        let mut ruled = vec![false; height];
        for row in (phase..height).step_by(period) {
            ruled[row] = true;
        }
        for (row, was_ruled) in ruled.iter().enumerate() {
            let y = u32::try_from(row).expect("a test row");
            let pixel = sheet.get(column, y).expect("a sheet pixel");
            let base = [paper, PHOTO_RULING][usize::from(*was_ruled)];
            assert_eq!(pixel, shaded(base, y), "column {column} at row {y}");
        }
    }

    /// Whether the pixel at `(column, row)` is the blue of the ruling
    /// rather than the grey of paper: shadow and vignette scale the blue
    /// channel hardest, so blue over red tells them apart at any depth.
    fn is_ruled(sheet: &RgbaImage, column: u32, row: u32) -> bool {
        let pixel = sheet.get(column, row).expect("a sheet pixel");
        pixel[2] > pixel[0]
    }

    /// The paper's luminance never decreases down the sheet, every
    /// adjacent paper-row step stays at 2 levels or under, and row 0 is
    /// clearly grey rather than white.
    fn assert_shadow_ramp(sheet: &RgbaImage, column: u32) {
        let paper: Vec<(u32, u32)> = (0..sheet.height)
            .filter(|&row| !is_ruled(sheet, column, row))
            .map(|row| {
                (
                    row,
                    luminance(sheet.get(column, row).expect("a sheet pixel")),
                )
            })
            .collect();
        assert!(paper.len() >= 2, "the column carries paper rows");
        for pair in paper.windows(2) {
            assert!(pair.len() >= 2, "the windows have two rows");
            let (before_row, before) = pair[0];
            let (row, after) = pair[1];
            assert!(after >= before, "the ramp never darkens at row {row}");
            let step = after.saturating_sub(before);
            assert!(
                step <= 2_u32,
                "the ramp steps {step} levels between rows {before_row} and {row}"
            );
        }
        let top = luminance(sheet.get(column, 0_u32).expect("a sheet pixel"));
        assert!(
            (64_u32..=160_u32).contains(&top),
            "row 0 is a clear grey, neither white nor ink: {top}"
        );
    }

    /// Both top corner vignettes sit darker than the paper beside them,
    /// and by no more than 20 levels: two shallow steps, no hard edge.
    fn assert_vignettes(sheet: &RgbaImage) {
        let row = 10_u32; // paper, on the ramp, inside the 28-row blocks
        let beside = luminance(sheet.get(500_u32, row).expect("a sheet pixel"));
        let left = luminance(sheet.get(30_u32, row).expect("a sheet pixel"));
        let right_column = sheet.width.saturating_sub(31_u32);
        let right = luminance(sheet.get(right_column, row).expect("a sheet pixel"));
        for (corner, value) in [("left", left), ("right", right)] {
            assert!(value < beside, "the {corner} corner is shaded");
            let step = beside.saturating_sub(value);
            assert!(step <= 20_u32, "the {corner} vignette drops {step} levels");
        }
    }

    /// The four wall bars and the interior tick print the shadowed ink
    /// exactly, the top bar is exactly the wall width thick, the tick is
    /// 12x3 and the ink is near-black.
    fn assert_walls_and_tick(sheet: &RgbaImage) {
        let (x0, y0, x1, y1) = PHOTO_ROOM;
        let centre_x = x0
            .checked_add(x1)
            .expect("a test span")
            .checked_div(2_u32)
            .expect("halved");
        let centre_y = y0
            .checked_add(y1)
            .expect("a test span")
            .checked_div(2_u32)
            .expect("halved");
        let wall = |column: u32, row: u32| {
            let pixel = sheet.get(column, row).expect("a sheet pixel");
            assert_eq!(
                pixel,
                shaded(PHOTO_INK, row),
                "the ink at ({column}, {row})"
            );
        };
        wall(centre_x, y0.checked_add(1_u32).expect("a test row")); // top
        wall(centre_x, y1.saturating_sub(1_u32)); // bottom
        wall(x0.checked_add(1_u32).expect("a test column"), centre_y); // left
        wall(x1.saturating_sub(1_u32), centre_y); // right
        let (tx0, ty0, tx1, ty1) = PHOTO_TICK;
        wall(
            tx0.checked_add(tx1)
                .expect("a test span")
                .checked_div(2_u32)
                .expect("halved"),
            ty0.checked_add(ty1)
                .expect("a test span")
                .checked_div(2_u32)
                .expect("halved"),
        ); // the tick
        let outside = y0.checked_add(PHOTO_WALL_PX).expect("a test wall fits");
        assert_eq!(
            sheet.get(centre_x, outside).expect("a sheet pixel"),
            shaded([255_u8, 255, 255, 255], outside),
            "the top bar is exactly PHOTO_WALL_PX thick"
        );
        assert_eq!(
            (tx1.saturating_sub(tx0), ty1.saturating_sub(ty0)),
            (12_u32, 3_u32),
            "the tick is 12x3"
        );
        assert!(luminance(PHOTO_INK) <= 64_u32, "the ink is near-black");
    }

    /// No pixel darker than luminance 64 lies within 90 px of any frame
    /// edge: the drawing keeps the sheet's border clean.
    fn assert_margin_is_clean(sheet: &RgbaImage) {
        let inset = 90_u32;
        let right = sheet.width.saturating_sub(inset);
        let bottom = sheet.height.saturating_sub(inset);
        let count = sheet.width.checked_mul(sheet.height).expect("a test sheet");
        for index in 0_u32..count {
            let column = index.checked_rem(sheet.width).expect("a test width");
            let row = index.checked_div(sheet.width).expect("a test width");
            let margin = column < inset || column >= right || row < inset || row >= bottom;
            let pixel = sheet.get(column, row).expect("a sheet pixel");
            let light = luminance(pixel) >= 64_u32;
            assert!(
                !margin || light,
                "the margin stays light at ({column}, {row})"
            );
        }
    }

    /// Reads the raw `ruled_photo_sheet` structure without any client:
    /// the ruling against the paper, the shadow ramp, both corner
    /// vignettes, the wall bars with their tick and the clean frame
    /// margin, every one of them the contract the sketch fixtures rely
    /// on.
    #[test]
    fn ruled_photo_sheet_carries_its_documented_layout() {
        let sheet = ruled_photo_sheet(600_u32, 400_u32);
        assert_eq!(
            (sheet.width, sheet.height),
            (600_u32, 400_u32),
            "the built sheet's shape"
        );
        let column = 500_u32; // clear of the room box and both vignettes
        assert_ruling(&sheet, column);
        assert_shadow_ramp(&sheet, column);
        assert_vignettes(&sheet);
        assert_walls_and_tick(&sheet);
        assert_margin_is_clean(&sheet);
    }
}
