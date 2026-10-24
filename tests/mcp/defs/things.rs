//! The thing tables as an MCP client sees them: the `DOOM II` set a bare
//! session answers with, the default game following the loaded `IWAD` with the
//! Heretic tables standing up too, the explicit `game` picks and their
//! refusals, and the `category`, `search` and `type` filters over each table
//! with the sorted rows, the hanging flag, the full entry shape and the empty
//! list a filter combination can narrow to.

use serde_json::json;

use crate::defs::types;
use crate::fixtures::{iwad, iwad_heretic};
use crate::support::{call, call_err, connect};

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
    let (code, message, _) =
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
            "source": "builtin",
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
            "source": "builtin",
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
            "source": "builtin",
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
