//! The CD-111 usability pass as seen by a real MCP client: every specific
//! error hint site answers with its documented `hint`, a bare error falls
//! back to the per-code default, the low-level tools' descriptions name the
//! high-level tools to prefer, and the server instructions carry the
//! workflow for a non-expert's agent. The CD-279 check adds the
//! description/schema contract: a tool description may only name parameters
//! its input schema really has, and may only enumerate mode words the tool
//! really accepts.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::model::Tool;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

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

    // ---- The CD-279 description/schema contract ----
    //
    // An outward-facing description must not lie about the wire. Three
    // simple, deliberate rules, all reading the description token stream
    // against the tool's own input schema:
    //
    // 1. `name: value` - a lowercase snake_case word followed by `:` and a
    //    literal value (`true`, `false`, `null`, a number, or a quoted or
    //    backticked word) claims a parameter: `name` must be a property of
    //    the input schema at any depth (so a nested `selection.all` counts).
    // 2. A backtick-wrapped lowercase snake_case identifier claims a
    //    parameter too, unless it names a listed tool or a `mode`/`kind`
    //    label sits right before it (rule 3 then judges it as a value).
    // 3. A mode word - the word right after the label `mode` or `kind`
    //    (past any colons and quotes) on a tool that really has such a
    //    parameter - must be accepted by it: equal to one of the
    //    parameter's schema `enum` strings, or quoted (`"` or backtick) in
    //    the parameter's own description. A label in prose on a tool
    //    without that parameter is no claim, and a claimed word that is a
    //    plain function word ("dither mode and stores") is skipped so
    //    English stays writable.

    /// The parameter labels whose following word a description claims as
    /// one of that parameter's accepted values (`mode pwad`, `kind switch`).
    const VALUE_LABELS: [&str; 2] = ["mode", "kind"];

    /// Function words that can follow `mode`/`kind` in prose but are never
    /// mode values; rule 3 skips a claim of one of these.
    const PROSE_WORDS: [&str; 6] = ["and", "or", "is", "the", "of", "that"];

    /// The words that are literal values after a colon (numbers start with
    /// a digit, checked apart).
    const LITERAL_WORDS: [&str; 3] = ["true", "false", "null"];

    /// One token of a scanned description, borrowing from the text.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Tok<'text> {
        /// A run of ASCII letters, digits and underscores.
        Word(&'text str),
        /// Any other single non-space character.
        Punct(char),
    }

    /// Splits a description into tokens: identifier words separated by
    /// single-character punctuation tokens; spaces are dropped.
    fn tokenize(text: &str) -> Vec<Tok<'_>> {
        let mut acc: (Option<usize>, Vec<Tok<'_>>) = (None, Vec::new());
        for (end, ch) in text.char_indices() {
            acc = step_char(text, acc, end, ch);
        }
        step_char(text, acc, text.len(), ' ').1
    }

    /// Consumes one character: a word character extends the open word, any
    /// other closes it and contributes one `Punct` token (spaces only
    /// separate).
    fn step_char<'text>(
        text: &'text str,
        acc: (Option<usize>, Vec<Tok<'text>>),
        end: usize,
        ch: char,
    ) -> (Option<usize>, Vec<Tok<'text>>) {
        let (mut start, mut out) = acc;
        if ch.is_ascii_alphanumeric() || ch == '_' {
            start.get_or_insert(end);
            return (start, out);
        }
        flush_word(text, start, end, &mut out);
        if !ch.is_ascii_whitespace() {
            out.push(Tok::Punct(ch));
        }
        (None, out)
    }

    /// Pushes the word spanning `start..end` when there is one.
    fn flush_word<'text>(
        text: &'text str,
        start: Option<usize>,
        end: usize,
        out: &mut Vec<Tok<'text>>,
    ) {
        let Some(word) = start.and_then(|from| text.get(from..end)) else {
            return;
        };
        out.push(Tok::Word(word));
    }

    /// The word at token `at`, when that token is one.
    fn word_at<'text>(tokens: &[Tok<'text>], at: usize) -> Option<&'text str> {
        match tokens.get(at) {
            Some(&Tok::Word(text)) => Some(text),
            Some(&Tok::Punct(_)) | None => None,
        }
    }

    /// Whether a word is shaped like a JSON parameter name: lowercase
    /// letters, digits and underscores, starting with a letter.
    fn is_param_shape(word: &str) -> bool {
        let Some(first) = word.chars().next() else {
            return false;
        };
        first.is_ascii_lowercase()
            && word
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    }

    /// The names under every `properties` object of a schema, at any depth,
    /// so nested fields (`selection.all`) count as real parameters.
    fn collect_field_names(schema: &Value, names: &mut Vec<String>) {
        names.extend(declared_props(schema));
        for child in children(schema) {
            collect_field_names(child, names);
        }
    }

    /// The `properties` keys of one schema node.
    fn declared_props(schema: &Value) -> Vec<String> {
        schema
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|props| props.keys().cloned())
            .collect()
    }

    /// The schema nodes directly below one node: object values or array items.
    fn children(schema: &Value) -> Vec<&Value> {
        if let Some(map) = schema.as_object() {
            return map.values().collect();
        }
        if let Some(items) = schema.as_array() {
            return items.iter().collect();
        }
        Vec::new()
    }

    /// Whether the token after a colon is a literal value, which is what
    /// turns the word before it into a parameter mention.
    fn is_literal_value(tok: Option<&Tok<'_>>) -> bool {
        match tok {
            Some(&Tok::Punct('"' | '`')) => true,
            Some(&Tok::Word(text)) => {
                LITERAL_WORDS.contains(&text)
                    || text.chars().next().is_some_and(|ch| ch.is_ascii_digit())
            }
            Some(&Tok::Punct(_)) | None => false,
        }
    }

    /// The parameter name a `[word, ':', value]` token window claims, when
    /// the word is param-shaped and the value is a literal.
    fn claimed_field(window: &[Tok<'_>]) -> Option<String> {
        let (Some(&Tok::Word(name)), Some(&Tok::Punct(':')), Some(value)) =
            (window.first(), window.get(1), window.get(2))
        else {
            return None;
        };
        (is_param_shape(name) && is_literal_value(Some(value))).then(|| name.to_owned())
    }

    /// Rule 1: every `name: value` mention with a literal value names an
    /// input field of the tool's schema.
    fn assert_colon_mentions(tool: &str, tokens: &[Tok<'_>], fields: &[String]) {
        let claims: Vec<String> = tokens.windows(3).filter_map(claimed_field).collect();
        for name in claims {
            assert!(
                fields.contains(&name),
                "{tool}'s description mentions `{name}: ...` but the tool has no \
                 such input field; name a real parameter or reword the mention",
            );
        }
    }

    /// The lone word a backtick at `at` wraps between two backticks.
    fn backticked_word<'text>(tokens: &[Tok<'text>], at: usize) -> Option<&'text str> {
        let word = word_at(tokens, at.saturating_add(1))?;
        let closed = matches!(tokens.get(at.saturating_add(2)), Some(&Tok::Punct('`')));
        closed.then_some(word)
    }

    /// The `(word, position)` pair when `token` is a backtick that opens a
    /// lone-word span; `None` otherwise.
    fn opened_word<'text>(
        at: usize,
        token: Tok<'_>,
        tokens: &[Tok<'text>],
    ) -> Option<(&'text str, usize)> {
        let is_open = token == Tok::Punct('`');
        is_open
            .then(|| backticked_word(tokens, at))
            .flatten()
            .map(|word| (word, at))
    }

    /// Whether the token at `at` is one of the `mode`/`kind` label words.
    fn word_is_label(tokens: &[Tok<'_>], at: usize) -> bool {
        word_at(tokens, at).is_some_and(|word| VALUE_LABELS.contains(&word))
    }

    /// Whether a `mode`/`kind` label sits right before the backtick at `at`
    /// (past at most a colon), which hands that word to rule 3 instead.
    fn preceded_by_label(tokens: &[Tok<'_>], at: usize) -> bool {
        if word_is_label(tokens, at.saturating_sub(1)) {
            return true;
        }
        tokens.get(at.saturating_sub(1)) == Some(&Tok::Punct(':'))
            && word_is_label(tokens, at.saturating_sub(2))
    }

    /// Rule 2: every backtick-wrapped lowercase identifier names a listed
    /// tool or an input field of the tool.
    fn assert_backticked_names(tool: &str, tokens: &[Tok<'_>], fields: &[String], tools: &[&str]) {
        let claims: Vec<&str> = tokens
            .iter()
            .enumerate()
            .filter_map(|(at, token)| opened_word(at, *token, tokens))
            .filter(|&(word, at)| {
                is_param_shape(word) && !tools.contains(&word) && !preceded_by_label(tokens, at)
            })
            .map(|(word, _at)| word)
            .collect();
        for word in claims {
            assert!(
                fields.iter().any(|field| field == word),
                "{tool}'s description backticks `{word}`, which is neither a listed \
                 tool nor an input field; name a real parameter or reword",
            );
        }
    }

    /// Whether a token is one of the `:`, `"` and backtick separators a mode
    /// claim may hide behind.
    fn is_claim_punct(token: Tok<'_>) -> bool {
        matches!(token, Tok::Punct(':' | '"' | '`'))
    }

    /// The word a label at `at` claims as its value: the first token past
    /// any run of colon and quote punctuation, when it is a param-shaped
    /// word.
    fn claimed_value<'text>(tokens: &[Tok<'text>], at: usize) -> Option<&'text str> {
        let first = tokens[at.saturating_add(1)..]
            .iter()
            .find(|token| !is_claim_punct(**token))?;
        match first {
            &Tok::Word(text) if is_param_shape(text) => Some(text),
            &Tok::Word(_) | &Tok::Punct(_) => None,
        }
    }

    /// The label, claimed value and parameter schema node of a `mode X` /
    /// `kind X` claim at token `at`, when the tool really has that
    /// parameter and the claimed word is not prose.
    fn mode_claim<'desc, 'schema>(
        tokens: &[Tok<'desc>],
        at: usize,
        props: Option<&'schema Value>,
    ) -> Option<(&'static str, &'desc str, &'schema Value)> {
        let word = word_at(tokens, at)?;
        let label = VALUE_LABELS.iter().copied().find(|name| *name == word)?;
        let value = claimed_value(tokens, at)?;
        if PROSE_WORDS.contains(&value) {
            return None;
        }
        let node = props?.get(label)?;
        Some((label, value, node))
    }

    /// The values a parameter accepts: its schema `enum` strings plus every
    /// param-shaped word quoted (double quotes or backticks) in its own
    /// description.
    fn accepted_values(node: &Value) -> Vec<String> {
        let mut out: Vec<String> = node
            .get("enum")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(text) = node.get("description").and_then(Value::as_str) {
            out.extend(quoted_words(text));
        }
        out.sort();
        out
    }

    /// The param-shaped words quoted in `text` with `"` or with backticks.
    fn quoted_words(text: &str) -> Vec<String> {
        ['"', '`']
            .iter()
            .flat_map(|quote| quoted_between(text, *quote))
            .collect()
    }

    /// The param-shaped words quoted between `quote` characters in `text`,
    /// read as every other piece of the `quote` split.
    fn quoted_between(text: &str, quote: char) -> Vec<String> {
        let mut pieces = text.split(quote);
        pieces.next();
        pieces
            .step_by(2)
            .map(str::trim)
            .filter(|inner| is_param_shape(inner))
            .map(str::to_ascii_lowercase)
            .collect()
    }

    /// Rule 3: every `mode X` / `kind X` claim lists X among the accepted
    /// values of the tool's parameter of that name.
    fn assert_mode_claims(tool: &str, tokens: &[Tok<'_>], schema: &Value) {
        let props = schema.get("properties");
        let claims: Vec<(&str, &str, &Value)> = (0..tokens.len())
            .filter_map(|at| mode_claim(tokens, at, props))
            .collect();
        for (label, value, node) in claims {
            let accepted = accepted_values(node);
            assert!(
                accepted.contains(&value.to_owned()),
                "{tool}'s description claims mode `{label} {value}`, which its {label} \
                 parameter does not accept (it names {accepted:?}); use an accepted word",
            );
        }
    }

    /// Checks one tool description against its input schema by the three
    /// documented rules.
    fn check_description(tool: &str, text: &str, schema: &Value, tools: &[&str]) {
        let mut fields: Vec<String> = Vec::new();
        collect_field_names(schema, &mut fields);
        let tokens = tokenize(text);
        assert_colon_mentions(tool, &tokens, &fields);
        assert_backticked_names(tool, &tokens, &fields, tools);
        assert_mode_claims(tool, &tokens, schema);
    }

    /// Checks every listed tool's description against its input schema.
    fn check_all_descriptions(listed: &[Tool], names: &[&str]) {
        for tool in listed {
            let schema = serde_json::to_value(&tool.input_schema).expect("a serializable schema");
            check_description(&tool.name, description_of(tool), &schema, names);
        }
    }

    #[tokio::test]
    async fn every_description_names_real_parameters_and_accepted_modes() {
        let client = connect().await;
        let listed = client.list_all_tools().await.expect("list tools");
        assert!(!listed.is_empty(), "the server lists tools");
        let names: Vec<&str> = listed.iter().map(|tool| tool.name.as_ref()).collect();
        check_all_descriptions(&listed, &names);
    }

    /// The `open_wad`/`save_wad` shape the synthetic checks run against:
    /// `discard_changes` (never `discard_dirty`) and a `mode` that quotes
    /// only `pwad` and `full`, plus a nested `selection.all`.
    fn synthetic_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "discard_changes": {"type": "boolean", "description": "Replace even when dirty."},
                "mode": {
                    "type": ["string", "null"],
                    "description": "\"pwad\" (the default) saves; \"full\" exports the merged view.",
                },
                "selection": {
                    "type": "object",
                    "properties": {"all": {"type": "boolean", "description": "Take the whole map."}},
                },
            },
        })
    }

    /// The buggy `open_wad` wording the CD-279 ticket reports.
    const OLD_DISCARD_WAD: &str =
        "Refuses with conflict while the archive is dirty unless discard_dirty: true.";

    /// The buggy `save_wad` wording the CD-279 ticket reports.
    const OLD_SAVE_MODE: &str = "mode pwad (default) writes it; mode merged flattens everything.";

    /// The fixed wording of both, plus the nested `all: true` form.
    const FIXED_WORDING: [&str; 2] = [
        "Refuses with conflict while the archive is dirty unless discard_changes: true.",
        "mode pwad (default) writes it; mode full flattens everything; or all: true takes the map.",
    ];

    #[test]
    #[should_panic(expected = "discard_dirty")]
    fn the_check_refuses_a_parameter_the_tool_lacks() {
        check_description(
            "open_wad",
            OLD_DISCARD_WAD,
            &synthetic_schema(),
            &["open_wad"],
        );
    }

    #[test]
    #[should_panic(expected = "merged")]
    fn the_check_refuses_a_mode_word_the_parameter_never_offers() {
        check_description(
            "save_wad",
            OLD_SAVE_MODE,
            &synthetic_schema(),
            &["save_wad"],
        );
    }

    #[test]
    fn the_check_passes_the_fixed_wording_with_nested_and_quoted_fields() {
        for text in FIXED_WORDING {
            check_description("open_wad", text, &synthetic_schema(), &["open_wad"]);
        }
    }

    #[test]
    #[should_panic(expected = "not_a_field")]
    fn the_check_refuses_a_backticked_name_that_is_neither_tool_nor_field() {
        check_description(
            "open_wad",
            "Pass `not_a_field` to choose.",
            &synthetic_schema(),
            &["open_wad"],
        );
    }

    #[test]
    fn the_check_allows_backticked_tools_and_quoted_mode_values() {
        check_description(
            "save_wad",
            "Call `undo` after; mode `full` exports.",
            &synthetic_schema(),
            &["undo"],
        );
    }

    #[test]
    fn the_check_ignores_prose_colons_and_possessives() {
        check_description(
            "open_wad",
            "Call it first: the map's and file's reads; a DS lump's data stays put.",
            &synthetic_schema(),
            &["open_wad"],
        );
    }

    #[test]
    fn the_check_skips_mode_prose_without_a_matching_parameter() {
        check_description(
            "create_flat_from_image",
            "Quantizes in your dither mode and stores the bytes.",
            &synthetic_schema(),
            &["save_wad"],
        );
        check_description(
            "save_wad",
            "The chosen mode is remembered; mode full exports.",
            &synthetic_schema(),
            &["save_wad"],
        );
    }
}
