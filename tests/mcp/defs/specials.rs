//! The linedef special tables as an MCP client sees them: the `DOOM II` set a
//! bare session answers with and the `DOOM`-only rows, the `category`, `search`
//! and `special` filters, the plain table number and the generalized one with
//! its Boom decoding, the Heretic and Hexen tables with their argument names,
//! the `family` override over a `DOOM` session with its bad-name and
//! contradiction refusals, and the Hexen `IWAD` defaulting every table.

use serde_json::{Value, json};

use crate::defs::{generalized_floor, special_row, specials_of};
use crate::fixtures::{iwad, iwad_hexen};
use crate::support::{call, call_err, connect};

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
    let (code, message, _) = call_err(
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
    let (second_code, second_message, _) = call_err(
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
    let (code, message, _) =
        call_err(&client, "get_linedef_specials", json!({ "game": "strife" })).await;
    assert_eq!(code, "invalid_argument", "unknown game code");
    assert!(
        message.contains("\"doom2\""),
        "the message lists games: {message}"
    );
}
