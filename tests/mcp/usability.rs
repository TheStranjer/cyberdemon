//! The CD-111 usability pass as seen by a real MCP client: every specific
//! error hint site answers with its documented `hint`, a bare error falls
//! back to the per-code default, the low-level tools' descriptions name the
//! high-level tools to prefer, and the server instructions carry the
//! workflow for a non-expert's agent.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::model::Tool;
    use rmcp::service::RunningService;
    use serde_json::json;

    use crate::fixtures::{iwad, iwad_features};
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// The documented hint of a tool refusal about a missing resource the
    /// listing tools know about.
    const INDEX_HINT: &str = "describe_map or find_elements returns indices";

    /// Open the plain `IWAD` fixture (textured two-room `MAP01`) as the
    /// session's base layer.
    async fn game_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        client
    }

    /// Open the features `IWAD` fixture (textured `MAP01`, `FLAT10`, the
    /// door wardrobe) and add a fresh empty working archive.
    async fn editable(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad_features(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        call(&client, "new_wad", json!({})).await;
        client
    }

    #[tokio::test]
    async fn a_session_without_the_game_points_at_open_iwad() {
        let client = connect().await;
        let (code, message, hint) = call_err(&client, "get_palette", json!({})).await;
        assert_eq!(code, "not_found", "no PLAYPAL loaded");
        assert!(
            message.contains("PLAYPAL"),
            "the message names it: {message}"
        );
        assert_eq!(
            hint, "call open_iwad (game data) first, then new_wad or open_wad",
            "the no-iwad site's hint"
        );
    }

    #[tokio::test]
    async fn an_unknown_map_points_at_list_maps_and_create_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = game_loaded(dir.path()).await;
        let (code, message, hint) =
            call_err(&client, "get_map_summary", json!({"map": "MAP99"})).await;
        assert_eq!(code, "not_found", "no such map");
        assert!(message.contains("MAP99"), "the message names it: {message}");
        assert_eq!(
            hint, "list_maps shows the maps; create_map makes a new one",
            "the unknown-map site's hint"
        );
    }

    #[tokio::test]
    async fn an_index_past_the_end_states_the_range_and_names_the_readers() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = game_loaded(dir.path()).await;
        let (code, message, hint) = call_err(
            &client,
            "edit_sector",
            json!({"index": 999_i32, "light": 100_i32}),
        )
        .await;
        assert_eq!(code, "not_found", "index past the block");
        assert!(
            message.contains("does not exist") && message.contains("valid indices are 0..="),
            "the message states the valid range: {message}"
        );
        assert_eq!(hint, INDEX_HINT, "the out-of-range site's hint");
    }

    #[tokio::test]
    async fn an_unknown_lump_points_at_search_lumps() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = game_loaded(dir.path()).await;
        let (code, message, hint) = call_err(&client, "get_lump", json!({"name": "NOPE"})).await;
        assert_eq!(code, "not_found", "no such lump");
        assert!(message.contains("NOPE"), "the message names it: {message}");
        assert_eq!(
            hint, "search_lumps finds lumps by pattern",
            "the unknown-lump site's hint"
        );
    }

    #[tokio::test]
    async fn an_unknown_flat_points_at_the_listers_and_the_creators() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = game_loaded(dir.path()).await;
        let (code, message, hint) = call_err(&client, "get_flat", json!({"name": "NOFLAT"})).await;
        assert_eq!(code, "not_found", "no such flat");
        assert!(
            message.contains("NOFLAT"),
            "the message names it: {message}"
        );
        assert!(
            hint.contains("list_flats")
                && hint.contains("list_textures")
                && hint.contains("create_flat_from_image")
                && hint.contains("create_texture_from_image"),
            "the flat/texture site's hint: {hint}"
        );
    }

    #[tokio::test]
    async fn an_unknown_wall_texture_points_at_the_same_way_out() {
        let client = connect().await;
        let (code, _, hint) = call_err(
            &client,
            "replace_texture",
            json!({"from": "STARTAN", "to": "NOTEX"}),
        )
        .await;
        assert_eq!(code, "not_found", "the merged view lacks the target");
        assert!(
            hint.contains("list_textures") && hint.contains("create_texture_from_image"),
            "the flat/texture site's hint: {hint}"
        );
    }

    #[tokio::test]
    async fn an_unknown_thing_name_points_at_get_thing_types() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = game_loaded(dir.path()).await;
        let (code, message, hint) = call_err(
            &client,
            "place_things",
            json!({"thing": "platypus", "bbox": [0_i32, 0_i32, 128_i32, 128_i32], "count": 1_i32}),
        )
        .await;
        assert_eq!(code, "invalid_argument", "no thing named platypus");
        assert!(
            message.contains("no thing named 'platypus'"),
            "the message names the lookup: {message}"
        );
        assert_eq!(
            hint, "get_thing_types with a search term",
            "the thing-name site's hint"
        );
    }

    #[tokio::test]
    async fn an_ambiguous_thing_name_points_at_get_thing_types_too() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = game_loaded(dir.path()).await;
        let (code, message, hint) = call_err(
            &client,
            "place_things",
            json!({"thing": "emon", "bbox": [0_i32, 0_i32, 128_i32, 128_i32], "count": 1_i32}),
        )
        .await;
        assert_eq!(
            code, "invalid_argument",
            "several types carry the substring"
        );
        assert!(
            message.contains("ambiguous"),
            "the message says so: {message}"
        );
        assert_eq!(hint, "get_thing_types with a search term", "same hint");
    }

    #[tokio::test]
    async fn rooms_that_do_not_touch_ask_for_a_shared_edge_or_a_corridor() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = editable(dir.path()).await;
        let (code, message, hint) = call_err(
            &client,
            "build_map_from_plan",
            json!({
                "map": "MAP05",
                "rooms": [
                    {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                    {"id": "b", "rect": {"x": 1024_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                ],
                "connections": [
                    {"from": "a", "to": "b", "kind": "opening"},
                ],
                "player_start": {"room": "a"},
            }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "the rooms cannot be joined");
        assert!(
            message.contains("do not touch") && message.contains('a') && message.contains('b'),
            "the message names the rooms: {message}"
        );
        assert_eq!(
            hint, "move the rooms so they share an edge, or add a corridor room",
            "the plan site's hint"
        );
    }

    #[tokio::test]
    async fn a_connection_wider_than_its_boundary_shares_the_plan_hint() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = editable(dir.path()).await;
        let (code, message, hint) = call_err(
            &client,
            "build_map_from_plan",
            json!({
                "map": "MAP05",
                "rooms": [
                    {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                    {"id": "b", "rect": {"x": 128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                ],
                "connections": [
                    {"from": "a", "to": "b", "kind": "opening", "width": 400_i32},
                ],
                "player_start": {"room": "a"},
            }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "the band cannot fit the boundary");
        assert!(
            message.contains("units wide") && message.contains("units long"),
            "the message names the width and the boundary: {message}"
        );
        assert_eq!(
            hint, "move the rooms so they share an edge, or add a corridor room",
            "the plan site's hint"
        );
    }

    #[tokio::test]
    async fn invalid_geometry_points_at_validate_map_and_undo() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = game_loaded(dir.path()).await;
        let (code, message, hint) = call_err(
            &client,
            "create_sector_from_loop",
            json!({
                "points": [
                    {"x": 0_i32, "y": 0_i32},
                    {"x": 128_i32, "y": 128_i32},
                    {"x": 128_i32, "y": 0_i32},
                    {"x": 0_i32, "y": 64_i32},
                ],
            }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "a crossing ring is refused");
        assert!(
            message.contains("self-intersecting"),
            "the message names the defect: {message}"
        );
        assert_eq!(
            hint, "validate_map shows details; undo reverts the last change",
            "the geometry site's hint"
        );
    }

    #[tokio::test]
    async fn a_bare_conflict_falls_back_to_its_code_default_hint() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let client = connect().await;
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        call(&client, "add_lump", json!({"name": "SCRATCH"})).await;
        let (code, _, hint) = call_err(&client, "new_wad", json!({})).await;
        assert_eq!(code, "conflict", "the working archive is dirty");
        assert_eq!(
            hint, "reload the state (get_session_info) and retry",
            "no site attached a hint: the conflict default answers"
        );
    }

    #[tokio::test]
    async fn a_bare_already_exists_falls_back_to_its_code_default_hint() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = editable(dir.path()).await;
        call(&client, "create_map", json!({"name": "USAB01"})).await;
        let (code, _, hint) = call_err(&client, "create_map", json!({"name": "USAB01"})).await;
        assert_eq!(code, "already_exists", "the name is taken");
        assert_eq!(
            hint, "pass replace: true or choose another name",
            "no site attached a hint: the already_exists default answers"
        );
    }

    /// The ticket's Prefer table, flattened to (tool, must-mention) pairs:
    /// every low-level tool's description must name the high-level tools to
    /// prefer.
    const PREFER_PAIRS: [(&str, &str); 24] = [
        ("add_vertex", "create_sector_from_loop"),
        ("add_vertex", "build_map_from_plan"),
        ("add_linedef", "create_sector_from_loop"),
        ("add_linedef", "build_map_from_plan"),
        ("add_sidedef", "create_sector_from_loop"),
        ("add_sidedef", "build_map_from_plan"),
        ("add_sector", "create_sector_from_loop"),
        ("add_sector", "build_map_from_plan"),
        ("create_sector_from_loop", "build_map_from_plan"),
        ("edit_linedef", "add_door"),
        ("edit_linedef", "add_switch"),
        ("edit_linedef", "add_exit"),
        ("edit_sector", "mark_secret"),
        ("edit_sector", "set_light_effect"),
        ("edit_sector", "bulk_edit"),
        ("edit_sector", "replace_texture"),
        ("add_thing", "place_things"),
        ("import_image_as_patch", "create_texture_from_image"),
        ("build_texture", "create_texture_from_image"),
        ("import_image_as_flat", "create_flat_from_image"),
        ("import_image_as_sprite", "create_sprite_from_image"),
        ("edit_sidedef", "bulk_edit"),
        ("edit_sidedef", "replace_texture"),
        ("build_nodes", "build_all_nodes"),
    ];

    /// The longest description a tool may carry, in characters.
    const DESCRIPTION_LIMIT: usize = 600;

    /// The tools the instructions must name by hand.
    const INSTRUCTION_MENTIONS: [&str; 5] = [
        "open_iwad",
        "build_map_from_plan",
        "trace_sketch",
        "check_playability",
        "run_batch",
    ];

    /// The listed tool named `wanted`.
    fn tool_named<'tools>(tools: &'tools [Tool], wanted: &str) -> &'tools Tool {
        tools
            .iter()
            .find(|tool| tool.name == wanted)
            .unwrap_or_else(|| panic!("{wanted} is listed"))
    }

    /// One tool's description text.
    fn description_of(tool: &Tool) -> &str {
        tool.description
            .as_deref()
            .unwrap_or_else(|| panic!("{} carries a description", tool.name))
    }

    /// Asserts every `PREFER_PAIRS` mention stands in the listed tools'
    /// descriptions.
    fn assert_prefer_table(tools: &[Tool]) {
        for (name, wanted) in PREFER_PAIRS {
            let text = description_of(tool_named(tools, name));
            assert!(
                text.contains(wanted),
                "{name}'s description must mention {wanted}: {text}"
            );
        }
    }

    /// Asserts one description is at most `DESCRIPTION_LIMIT` characters and
    /// carries a full stop.
    fn assert_description_shape(tool: &Tool) {
        let text = description_of(tool);
        let length = text.chars().count();
        assert!(
            length <= DESCRIPTION_LIMIT,
            "{}'s description is {length} characters, over {DESCRIPTION_LIMIT}: {text}",
            tool.name
        );
        assert!(
            text.contains('.'),
            "{}'s description has no full stop: {text}",
            tool.name
        );
    }

    /// Asserts the instructions name every required workflow tool.
    fn assert_instruction_mentions(instructions: &str) {
        for wanted in INSTRUCTION_MENTIONS {
            assert!(
                instructions.contains(wanted),
                "the instructions must mention {wanted}: {instructions}"
            );
        }
    }

    /// The underscore-joined lowercase words in `text`: each names a tool
    /// and must exist as one.
    fn tool_tokens(text: &str) -> Vec<&str> {
        text.split(|c: char| !c.is_ascii_lowercase() && c != '_')
            .filter(|word| word.contains('_'))
            .collect()
    }

    /// Asserts every tool-shaped word in the instructions names a listed
    /// tool.
    fn assert_instruction_tools_exist(instructions: &str, tools: &[Tool]) {
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        for token in tool_tokens(instructions) {
            assert!(
                names.contains(&token),
                "the instructions name {token:?}, which is not a listed tool"
            );
        }
    }

    #[tokio::test]
    async fn every_low_level_description_names_its_high_level_replacement() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        assert_prefer_table(&tools);
    }

    #[tokio::test]
    async fn every_description_is_short_and_ends_in_sentences() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        assert!(!tools.is_empty(), "the server lists tools");
        tools.iter().for_each(assert_description_shape);
    }

    #[tokio::test]
    async fn the_server_instructions_answer_for_a_non_expert_and_exist() {
        let client = connect().await;
        let tools = client.list_all_tools().await.expect("list tools");
        let info = client.peer_info().expect("server info after initialize");
        let instructions = info
            .instructions
            .as_deref()
            .expect("instructions in the initialize result");
        assert_instruction_mentions(instructions);
        assert!(
            instructions.lines().count() <= 25,
            "the instructions are {} lines, at most 25",
            instructions.lines().count()
        );
        assert_instruction_tools_exist(instructions, &tools);
    }
}
