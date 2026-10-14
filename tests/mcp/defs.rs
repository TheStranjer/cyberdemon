//! The `defs` tool group as seen by a real MCP client: the embedded game
//! definition tables, the session-detected default game, the filters, and
//! the Boom generalized and sector decodings.

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, iwad_heretic, iwad_hexen};
    use crate::support::{call, call_err, connect};

    /// A generalized floor value: `0x6000 | WR | fast | up | lowest
    /// neighbour ceiling` (bits `1 | (2 << 3) | (1 << 6) | (3 << 7)`),
    /// combined without bare operators.
    fn generalized_floor() -> u64 {
        0x6000_u64
            .checked_add(1)
            .and_then(|v| v.checked_add(2_u64.checked_shl(3).expect("shift fits")))
            .and_then(|v| v.checked_add(1_u64.checked_shl(6).expect("shift fits")))
            .and_then(|v| v.checked_add(3_u64.checked_shl(7).expect("shift fits")))
            .expect("the fixture value fits")
    }

    /// The type numbers of a report's `things` array.
    fn types(report: &Value) -> Vec<u64> {
        report["things"]
            .as_array()
            .expect("things is an array")
            .iter()
            .map(|entry| {
                entry["type"]
                    .as_u64()
                    .expect("every entry has a type number")
            })
            .collect()
    }

    #[tokio::test]
    async fn an_empty_session_defaults_to_the_doom_ii_table() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({})).await;
        assert_eq!(
            report["game"],
            json!("doom2"),
            "unidentified session defaults"
        );
        assert_eq!(report["count"].as_u64(), Some(123), "the whole DOOM II set");
        assert_eq!(types(&report).len(), 123, "count mirrors the list");
        assert_eq!(report["warnings"], json!([]), "DOOM II has its own table");
        let kinds = types(&report);
        let mut sorted = kinds.clone();
        sorted.sort_unstable();
        assert_eq!(kinds, sorted, "rows arrive sorted by type");
    }

    #[tokio::test]
    async fn the_default_game_follows_the_loaded_iwad() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let client = connect().await;
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let report = call(&client, "get_thing_types", json!({})).await;
        assert_eq!(
            report["game"],
            json!("doom2"),
            "the fixture IWAD identifies DOOM II"
        );
        assert_eq!(
            report["count"].as_u64(),
            Some(123),
            "and picks the DOOM II table"
        );
        assert!(types(&report).contains(&65), "the Chaingunner is in it");
    }

    #[tokio::test]
    async fn a_heretic_iwad_defaults_to_the_heretic_tables() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad_heretic(dir.path());
        let client = connect().await;
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let things = call(&client, "get_thing_types", json!({})).await;
        assert_eq!(
            things["game"],
            json!("heretic"),
            "the fixture IWAD identifies Heretic"
        );
        assert_eq!(
            things["count"].as_u64(),
            Some(84),
            "and picks the heretic thing table"
        );
        assert!(types(&things).contains(&66), "the Gargoyle is in it");
        assert_eq!(things["warnings"], json!([]), "no fallback warning");
        let specials = call(&client, "get_linedef_specials", json!({})).await;
        assert_eq!(specials["game"], json!("heretic"), "specials follow too");
        assert_eq!(
            specials["count"].as_u64(),
            Some(105),
            "the heretic special table"
        );
        let sectors = call(&client, "get_sector_specials", json!({})).await;
        assert_eq!(
            sectors["count"].as_u64(),
            Some(47),
            "the heretic sector table"
        );
    }

    #[tokio::test]
    async fn an_explicit_doom_game_excludes_doom_ii_things() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({ "game": "doom" })).await;
        assert_eq!(report["game"], json!("doom"), "echoed game");
        assert_eq!(report["count"].as_u64(), Some(97), "the DOOM-only set");
        assert!(
            !types(&report).contains(&65),
            "Chaingunner is a DOOM II thing"
        );
        assert!(types(&report).contains(&3001), "the Imp is a DOOM thing");
    }

    #[tokio::test]
    async fn an_unknown_game_string_is_an_invalid_argument() {
        let client = connect().await;
        let (code, message) =
            call_err(&client, "get_thing_types", json!({ "game": "strife" })).await;
        assert_eq!(code, "invalid_argument", "unsupported game code");
        assert!(
            message.contains("\"doom\"") && message.contains("\"hexen\""),
            "the message names the accepted games: {message}"
        );
    }

    #[tokio::test]
    async fn heretic_lists_its_own_thing_table() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({ "game": "heretic" })).await;
        assert_eq!(report["game"], json!("heretic"), "echoed game");
        assert_eq!(report["count"].as_u64(), Some(84), "the whole heretic set");
        let kinds = types(&report);
        assert_eq!(kinds.len(), 84, "count mirrors the list");
        let mut sorted = kinds.clone();
        sorted.sort_unstable();
        assert_eq!(kinds, sorted, "rows arrive sorted by type");
        assert_eq!(report["warnings"], json!([]), "heretic has its own table");
        assert!(kinds.contains(&66), "the Gargoyle is a heretic thing");
        assert!(!kinds.contains(&3001), "the Imp is not");
        let gargoyle = report["things"]
            .as_array()
            .expect("things is an array")
            .iter()
            .find(|entry| entry["type"] == json!(66_i64))
            .cloned()
            .expect("type 66 is listed");
        assert_eq!(
            gargoyle,
            json!({
                "type": 66_i64,
                "name": "Gargoyle",
                "sprite": "IMPX",
                "frame": "A",
                "radius": 20_i64,
                "category": "Monsters",
                "hanging": false,
            }),
            "the heretic row reads as ticketed"
        );
    }

    #[tokio::test]
    async fn heretic_things_filter_by_category_search_and_type() {
        let client = connect().await;
        let monsters = call(
            &client,
            "get_thing_types",
            json!({ "game": "heretic", "category": "monsters" }),
        )
        .await;
        assert_eq!(monsters["count"].as_u64(), Some(16), "the heretic monsters");
        let keys = call(
            &client,
            "get_thing_types",
            json!({ "game": "heretic", "category": "Keys" }),
        )
        .await;
        assert_eq!(
            types(&keys),
            vec![73_u64, 79_u64, 80_u64],
            "the three heretic keys in type order"
        );
        let gargoyle = call(
            &client,
            "get_thing_types",
            json!({ "game": "heretic", "search": "gargoyle" }),
        )
        .await;
        assert_eq!(
            gargoyle["count"].as_u64(),
            Some(2),
            "Gargoyle and Fire Gargoyle"
        );
        let hanging = call(
            &client,
            "get_thing_types",
            json!({ "game": "heretic", "type": 51_i64 }),
        )
        .await;
        let corpse = hanging["things"].get(0).expect("one row").clone();
        assert_eq!(corpse["name"], json!("Hanging Corpse"), "name");
        assert_eq!(corpse["hanging"], json!(true), "the flag came through");
        assert_eq!(
            corpse["category"],
            json!("Gore/Hanging Skulls & Bodies"),
            "category"
        );
    }

    #[tokio::test]
    async fn hexen_lists_its_own_thing_table() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({ "game": "hexen" })).await;
        assert_eq!(report["game"], json!("hexen"), "echoed game");
        assert_eq!(report["count"].as_u64(), Some(233), "the whole hexen set");
        assert_eq!(types(&report).len(), 233, "count mirrors the list");
        assert_eq!(report["warnings"], json!([]), "hexen has its own table");
        let monsters = call(
            &client,
            "get_thing_types",
            json!({ "game": "hexen", "category": "Monsters" }),
        )
        .await;
        assert_eq!(monsters["count"].as_u64(), Some(18), "the hexen monsters");
        let ettin = monsters["things"]
            .as_array()
            .expect("things is an array")
            .iter()
            .find(|entry| entry["type"] == json!(10030_i64))
            .cloned()
            .expect("the Ettin is listed");
        assert_eq!(
            ettin,
            json!({
                "type": 10030_i64,
                "name": "Ettin",
                "sprite": "ETTN",
                "frame": "A",
                "radius": 25_i64,
                "category": "Monsters",
                "hanging": false,
            }),
            "the hexen monster row reads as ticketed"
        );
    }

    #[tokio::test]
    async fn the_category_filter_is_a_case_insensitive_prefix() {
        let client = connect().await;
        let report = call(
            &client,
            "get_thing_types",
            json!({ "game": "doom", "category": "monster" }),
        )
        .await;
        assert_eq!(report["count"].as_u64(), Some(10), "the DOOM monsters");
        let entries = report["things"].as_array().expect("things is an array");
        assert!(
            entries
                .iter()
                .all(|entry| entry["category"] == json!("Monsters")),
            "every row matches the category: {entries:?}"
        );
        let gore = call(
            &client,
            "get_thing_types",
            json!({ "game": "doom", "category": "Gore/" }),
        )
        .await;
        assert_eq!(
            gore["count"].as_u64(),
            Some(26),
            "every gore category, three of them"
        );
    }

    #[tokio::test]
    async fn the_search_filter_finds_names_as_a_substring() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({ "search": "baron" })).await;
        assert_eq!(report["count"].as_u64(), Some(1), "exactly the Baron");
        let baron = report["things"].get(0).expect("one row").clone();
        assert_eq!(baron["type"], json!(3_003_i64), "type number");
        assert_eq!(baron["name"], json!("Baron Of Hell"), "name");
    }

    #[tokio::test]
    async fn the_type_filter_returns_one_full_entry() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({ "type": 3_001_i64 })).await;
        assert_eq!(report["count"].as_u64(), Some(1), "a single row");
        assert_eq!(
            report["things"],
            json!([{
                "type": 3_001_i64,
                "name": "Imp",
                "sprite": "TROO",
                "frame": "A",
                "radius": 20_i64,
                "category": "Monsters",
                "hanging": false,
            }]),
            "the full entry"
        );
    }

    #[tokio::test]
    async fn an_unknown_type_number_is_an_empty_list_not_an_error() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({ "type": 424_242_i64 })).await;
        assert_eq!(report["count"].as_u64(), Some(0), "nothing matches");
        assert_eq!(report["things"], json!([]), "an empty list");
        assert_eq!(
            report["game"],
            json!("doom2"),
            "the default game still answers"
        );
    }

    #[tokio::test]
    async fn hanging_things_report_the_flag() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({ "type": 63_i64 })).await;
        let keen = report["things"].get(0).expect("one row").clone();
        assert_eq!(
            keen["hanging"],
            json!(true),
            "the hanging flag came through"
        );
        assert_eq!(keen["sprite"], json!("GOR1"), "sprite");
        assert_eq!(keen["frame"], json!("A"), "frame");
    }

    #[tokio::test]
    async fn filters_combine_with_the_exact_type() {
        let client = connect().await;
        let report = call(
            &client,
            "get_thing_types",
            json!({ "type": 3_001_i64, "category": "Weapons" }),
        )
        .await;
        assert_eq!(report["count"].as_u64(), Some(0), "the Imp is no weapon");
    }

    /// The special numbers of a report's `specials` array.
    fn specials_of(report: &Value) -> Vec<u64> {
        report["specials"]
            .as_array()
            .expect("specials is an array")
            .iter()
            .map(|entry| {
                entry["special"]
                    .as_u64()
                    .expect("every entry has a special number")
            })
            .collect()
    }

    #[tokio::test]
    async fn specials_list_the_doom_ii_set_by_default() {
        let client = connect().await;
        let report = call(&client, "get_linedef_specials", json!({})).await;
        assert_eq!(report["game"], json!("doom2"), "default game");
        assert_eq!(report["count"].as_u64(), Some(139), "the whole DOOM II set");
        assert_eq!(specials_of(&report).len(), 139, "count mirrors the list");
        let numbers = specials_of(&report);
        let mut sorted = numbers.clone();
        sorted.sort_unstable();
        assert_eq!(numbers, sorted, "rows arrive sorted by special");
        assert!(numbers.contains(&117), "DR Door Fast is a DOOM II special");
        assert_eq!(
            report["generalized"],
            json!(null),
            "a listing decodes nothing"
        );
        assert_eq!(report["warnings"], json!([]), "DOOM II has its own tables");
    }

    #[tokio::test]
    async fn doom_specials_exclude_the_doom_ii_additions() {
        let client = connect().await;
        let report = call(&client, "get_linedef_specials", json!({ "game": "doom" })).await;
        assert_eq!(report["count"].as_u64(), Some(100), "the DOOM-only set");
        assert!(
            !specials_of(&report).contains(&117),
            "special 117 is DOOM II only"
        );
        assert!(
            specials_of(&report).contains(&1),
            "DR Door is a DOOM special"
        );
    }

    #[tokio::test]
    async fn specials_filter_by_category_search_and_number() {
        let client = connect().await;
        let local = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "doom", "category": "doors/local" }),
        )
        .await;
        assert_eq!(local["count"].as_u64(), Some(8), "the DOOM local doors");
        let crusher = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "doom", "search": "crusher" }),
        )
        .await;
        assert_eq!(crusher["count"].as_u64(), Some(7), "the DOOM crushers");
        let one = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "doom", "special": 1_i64 }),
        )
        .await;
        assert_eq!(
            one["specials"],
            json!([{
                "special": 1_i64,
                "name": "DR Door",
                "category": "Doors/Local",
                "target": "sector_back",
                "args": [],
            }]),
            "the exact number gives the full row"
        );
    }

    #[tokio::test]
    async fn an_unknown_plain_special_is_an_empty_list_not_an_error() {
        let client = connect().await;
        let report = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "doom", "special": 999_i64 }),
        )
        .await;
        assert_eq!(report["count"].as_u64(), Some(0), "nothing matches");
        assert_eq!(report["specials"], json!([]), "an empty list");
        assert_eq!(
            report["generalized"],
            json!(null),
            "0x3E7 is not generalized"
        );
    }

    #[tokio::test]
    async fn a_generalized_number_decodes_without_table_rows() {
        let client = connect().await;
        let report = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "doom2", "special": generalized_floor() }),
        )
        .await;
        assert_eq!(report["count"].as_u64(), Some(0), "no table rows");
        assert_eq!(
            report["specials"],
            json!([]),
            "the generalized answer replaces them"
        );
        let decoded = report["generalized"].clone();
        assert_eq!(decoded["kind"], json!("Floor"), "kind");
        assert_eq!(decoded["trigger"], json!("WR"), "trigger bits 0-2");
        assert_eq!(decoded["speed"], json!("fast"), "speed bits 3-4");
        assert_eq!(decoded["fields"]["direction"], json!("up"), "bit 6");
        assert_eq!(
            decoded["fields"]["target"],
            json!("lowest neighbour ceiling"),
            "target bits 7-9"
        );
        assert_eq!(
            decoded["summary"],
            json!("WR Floor up to lowest neighbour ceiling, fast"),
            "summary text"
        );
    }

    #[tokio::test]
    async fn a_heretic_special_number_gives_the_heretic_row() {
        let client = connect().await;
        let report = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "heretic", "special": 100_i64 }),
        )
        .await;
        assert_eq!(report["game"], json!("heretic"), "echoed game");
        assert_eq!(
            report["specials"],
            json!([{
                "special": 100_i64,
                "name": "WR Door Triple Speed",
                "category": "Doors/Remote",
                "target": "sector",
                "args": [],
            }]),
            "100 means the heretic door, not the DOOM II stair builder"
        );
        assert_eq!(report["count"].as_u64(), Some(1), "one row");
        assert_eq!(
            report["generalized"],
            json!(null),
            "100 is a plain table number"
        );
        assert_eq!(report["warnings"], json!([]), "heretic has its own tables");
    }

    #[tokio::test]
    async fn heretic_specials_filter_by_category_and_search() {
        let client = connect().await;
        let crushers = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "heretic", "category": "crushers" }),
        )
        .await;
        assert_eq!(crushers["count"].as_u64(), Some(6), "the heretic crushers");
        let doors = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "heretic", "category": "doors/" }),
        )
        .await;
        assert_eq!(doors["count"].as_u64(), Some(24), "local plus remote doors");
        let donut = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "heretic", "search": "donut" }),
        )
        .await;
        assert_eq!(donut["count"].as_u64(), Some(1), "S1 Floor Donut");
        let exit = donut["specials"]
            .get(0)
            .expect("one row")
            .get("target")
            .cloned()
            .expect("a target field");
        assert_eq!(exit, json!("sector"), "the donut acts on tagged sectors");
    }

    #[tokio::test]
    async fn generalized_numbers_are_not_decoded_on_heretic() {
        let client = connect().await;
        let report = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "heretic", "special": generalized_floor() }),
        )
        .await;
        assert_eq!(
            report["generalized"],
            json!(null),
            "no Boom decoding for heretic"
        );
        assert_eq!(report["count"].as_u64(), Some(0), "and no such heretic row");
        assert_eq!(report["specials"], json!([]), "an empty list stands");
        assert_eq!(
            report["warnings"],
            json!([]),
            "heretic has its own tables now"
        );
    }

    /// The row with this `special` number in a report, or panic.
    fn special_row(report: &Value, number: i64) -> Value {
        report["specials"]
            .as_array()
            .expect("specials is an array")
            .iter()
            .find(|entry| entry["special"] == json!(number))
            .cloned()
            .unwrap_or_else(|| panic!("special {number} is listed"))
    }

    #[tokio::test]
    async fn hexen_specials_name_their_script_arguments() {
        let client = connect().await;
        let report = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "hexen", "special": 80_i64 }),
        )
        .await;
        assert_eq!(report["game"], json!("hexen"), "echoed game");
        assert_eq!(report["count"].as_u64(), Some(1), "one row");
        assert_eq!(
            report["specials"],
            json!([{
                "special": 80_i64,
                "name": "ACS_Execute",
                "category": "Scripting",
                "target": "-",
                "args": ["Script Number", "Map Levelnum", "First Arg", "Second Arg", "Third Arg"],
            }]),
            "five argument names in order, arg1 first"
        );
        assert_eq!(report["generalized"], json!(null), "hexen is not Boom");
        assert_eq!(report["warnings"], json!([]), "hexen has its own tables");
        let endgame = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "hexen", "special": 75_i64 }),
        )
        .await;
        let row = special_row(&endgame, 75);
        assert_eq!(
            row["args"],
            json!([]),
            "Teleport_EndGame takes no arguments"
        );
        let quake = call(
            &client,
            "get_linedef_specials",
            json!({ "game": "hexen", "special": 120_i64 }),
        )
        .await;
        assert_eq!(
            special_row(&quake, 120)["target"],
            json!("ex_5thing"),
            "the free-form target passes through"
        );
    }

    #[tokio::test]
    async fn family_hexen_requests_the_hexen_table_from_a_doom_session() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let client = connect().await;
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let report = call(
            &client,
            "get_linedef_specials",
            json!({ "family": "hexen", "special": 13_i64 }),
        )
        .await;
        assert_eq!(report["game"], json!("doom2"), "the session game is echoed");
        assert_eq!(
            special_row(&report, 13)["args"],
            json!(["Sector Tag", "Speed", "Delay", "Lock"]),
            "the Hexen table answers with its argument names"
        );
        let warnings = report["warnings"].as_array().expect("warnings is an array");
        assert!(
            warnings
                .iter()
                .filter_map(Value::as_str)
                .any(|text| text.contains("family \"hexen\"")),
            "the override is explained: {warnings:?}"
        );
        let all = call(
            &client,
            "get_linedef_specials",
            json!({ "family": "Hexen" }),
        )
        .await;
        assert_eq!(all["count"].as_u64(), Some(86), "the whole Hexen table");
        let plain = call(
            &client,
            "get_linedef_specials",
            json!({ "family": "doom", "special": 1_i64 }),
        )
        .await;
        assert_eq!(
            plain["specials"],
            json!([{
                "special": 1_i64,
                "name": "DR Door",
                "category": "Doors/Local",
                "target": "sector_back",
                "args": [],
            }]),
            "family doom keeps the session's DOOM-family rows"
        );
        assert_eq!(plain["warnings"], json!([]), "no override note for doom");
    }

    #[tokio::test]
    async fn generalized_numbers_are_not_decoded_under_the_hexen_family() {
        let client = connect().await;
        let report = call(
            &client,
            "get_linedef_specials",
            json!({ "family": "hexen", "special": generalized_floor() }),
        )
        .await;
        assert_eq!(
            report["generalized"],
            json!(null),
            "the Hexen table is not Boom-packed"
        );
        assert_eq!(report["count"].as_u64(), Some(0), "no such hexen row");
    }

    #[tokio::test]
    async fn the_family_parameter_refuses_bad_names_and_contradictions() {
        let client = connect().await;
        let (code, message) = call_err(
            &client,
            "get_linedef_specials",
            json!({ "family": "zdoom" }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "unknown family code");
        assert!(
            message.contains("\"hexen\"") && message.contains("leave family out"),
            "the message names the values and the default: {message}"
        );
        let (second_code, second_message) = call_err(
            &client,
            "get_linedef_specials",
            json!({ "game": "heretic", "family": "doom" }),
        )
        .await;
        assert_eq!(
            second_code, "invalid_argument",
            "family doom needs a DOOM-family game"
        );
        assert!(
            second_message.contains("DOOM-family"),
            "the message says what is required: {second_message}"
        );
    }

    #[tokio::test]
    async fn a_hexen_iwad_defaults_to_the_hexen_tables() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad_hexen(dir.path());
        let client = connect().await;
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let things = call(&client, "get_thing_types", json!({})).await;
        assert_eq!(
            things["game"],
            json!("hexen"),
            "TITLE plus WINNOWR detect Hexen"
        );
        assert_eq!(things["count"].as_u64(), Some(233), "the hexen thing table");
        assert_eq!(things["warnings"], json!([]), "no missing-table warning");
        let specials = call(&client, "get_linedef_specials", json!({})).await;
        assert_eq!(specials["game"], json!("hexen"), "specials follow too");
        assert_eq!(
            specials["count"].as_u64(),
            Some(86),
            "the hexen special table"
        );
        assert_eq!(
            special_row(&specials, 11)["target"],
            json!("sector_or_back"),
            "Door_Open may act on either side"
        );
        let sectors = call(&client, "get_sector_specials", json!({})).await;
        assert_eq!(
            sectors["count"].as_u64(),
            Some(46),
            "the hexen sector table"
        );
        assert_eq!(sectors["warnings"], json!([]), "hexen has its own tables");
    }

    #[tokio::test]
    async fn specials_refuse_an_unknown_game() {
        let client = connect().await;
        let (code, message) =
            call_err(&client, "get_linedef_specials", json!({ "game": "strife" })).await;
        assert_eq!(code, "invalid_argument", "unknown game code");
        assert!(
            message.contains("\"doom2\""),
            "the message lists games: {message}"
        );
    }

    #[tokio::test]
    async fn sector_types_list_the_full_doom_table() {
        let client = connect().await;
        let report = call(&client, "get_sector_specials", json!({ "game": "doom" })).await;
        assert_eq!(report["game"], json!("doom"), "echoed game");
        assert_eq!(report["count"].as_u64(), Some(16), "the shipped rows");
        assert_eq!(report["special"], json!(null), "no value echoed");
        assert_eq!(report["decoded"], json!(null), "nothing decoded");
        let rows = report["sector_types"]
            .as_array()
            .expect("sector_types is an array");
        assert!(
            rows.iter()
                .any(|row| row["type"] == json!(16_i64) && row["name"] == json!("20% Damage")),
            "type 16 is in the table: {rows:?}"
        );
        assert_eq!(report["warnings"], json!([]), "DOOM has its own table");
    }

    #[tokio::test]
    async fn heretic_sector_types_list_without_decoding() {
        let client = connect().await;
        let report = call(&client, "get_sector_specials", json!({ "game": "heretic" })).await;
        assert_eq!(report["game"], json!("heretic"), "echoed game");
        assert_eq!(report["count"].as_u64(), Some(47), "the heretic rows");
        assert_eq!(report["decoded"], json!(null), "nothing asked for");
        let rows = report["sector_types"]
            .as_array()
            .expect("sector_types is an array");
        assert_eq!(rows.len(), 47, "count mirrors the list");
        assert!(
            rows.iter()
                .any(|row| row["type"] == json!(5_i64) && row["name"] == json!("5% Lava Damage")),
            "the heretic damage type is in the table: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row["type"] == json!(51_i64) && row["name"] == json!("Wind West (25)")),
            "the last heretic row is there: {rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row["name"] == json!("20% Damage")),
            "no Doom sector rows leaked in: {rows:?}"
        );
        assert_eq!(report["warnings"], json!([]), "heretic has its own table");
        let decoded = call(
            &client,
            "get_sector_specials",
            json!({ "game": "heretic", "special": 16_i64 }),
        )
        .await;
        assert_eq!(decoded["special"], json!(16_i64), "the number is echoed");
        assert_eq!(
            decoded["decoded"],
            json!(null),
            "heretic sector values are not Boom bits"
        );
        assert_eq!(decoded["count"].as_u64(), Some(47), "the table still ships");
    }

    #[tokio::test]
    async fn a_sector_value_decodes_with_table_and_flags() {
        let client = connect().await;
        // base 9, damage bits 2 (10%), secret bit (9 | (2 << 5) | (1 << 7)).
        let report = call(
            &client,
            "get_sector_specials",
            json!({ "special": 201_i64 }),
        )
        .await;
        assert_eq!(report["game"], json!("doom2"), "the session default");
        assert_eq!(report["special"], json!(201_i64), "the number is echoed");
        assert_eq!(report["count"].as_u64(), Some(16), "the table still ships");
        assert_eq!(
            report["decoded"],
            json!({
                "base": 9_i64,
                "base_name": "Secret",
                "damage": "10%",
                "flags": ["secret"],
            }),
            "the Boom decoding"
        );
    }

    #[tokio::test]
    async fn sector_decoding_covers_friction_and_push() {
        let client = connect().await;
        // base 16 with friction (bit 8) and push (bit 9): 16 | (1 << 8) | (1 << 9).
        let report = call(
            &client,
            "get_sector_specials",
            json!({ "special": 784_i64 }),
        )
        .await;
        assert_eq!(
            report["decoded"],
            json!({
                "base": 16_i64,
                "base_name": "20% Damage",
                "damage": "none",
                "flags": ["friction", "push"],
            }),
            "friction and push flags decode"
        );
    }

    #[tokio::test]
    async fn sector_values_are_not_decoded_for_other_games() {
        let client = connect().await;
        let report = call(
            &client,
            "get_sector_specials",
            json!({ "game": "hexen", "special": 16_i64 }),
        )
        .await;
        assert_eq!(report["decoded"], json!(null), "no Boom bits for hexen");
        assert_eq!(report["count"].as_u64(), Some(46), "the hexen sector table");
        assert_eq!(report["warnings"], json!([]), "hexen ships its own tables");
        let sky = report["sector_types"]
            .as_array()
            .expect("sector_types is an array")
            .iter()
            .find(|row| row["type"] == json!(200_i64))
            .cloned()
            .expect("type 200 is listed");
        assert_eq!(
            sky["name"],
            json!("Sky 2 (MAPINFO)"),
            "the hexen sky row reads as ticketed"
        );
    }

    #[tokio::test]
    async fn sectors_refuse_an_unknown_game() {
        let client = connect().await;
        let (code, message) =
            call_err(&client, "get_sector_specials", json!({ "game": "zdaemon" })).await;
        assert_eq!(code, "invalid_argument", "unknown game code");
        assert!(
            message.contains("game out"),
            "the message says how to fix it: {message}"
        );
    }
}
