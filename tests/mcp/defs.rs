//! The `defs` tool group as seen by a real MCP client: the embedded game
//! definition tables, the session-detected default game, and the filters.

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::fixtures::iwad;
    use crate::support::{call, call_err, connect};

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
    async fn heretic_reports_empty_with_a_not_yet_available_warning() {
        let client = connect().await;
        let report = call(&client, "get_thing_types", json!({ "game": "heretic" })).await;
        assert_eq!(report["game"], json!("heretic"), "echoed game");
        assert_eq!(report["count"].as_u64(), Some(0), "no heretic table yet");
        assert_eq!(report["things"], json!([]), "and an empty list");
        let warnings = report["warnings"].as_array().expect("warnings is an array");
        assert_eq!(warnings.len(), 1, "one warning");
        assert!(
            warnings
                .first()
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("not yet available")),
            "the warning says what is missing: {warnings:?}"
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
}
