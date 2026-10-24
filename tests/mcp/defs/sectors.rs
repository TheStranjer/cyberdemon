//! The sector type tables as an MCP client sees them: the shipped `DOOM` rows,
//! the Heretic table standing on its own without decoding, the Boom decoding of
//! a sector value with its base, damage, secret, friction and push flags, and
//! the other games refusing to decode as Boom bits.

use serde_json::json;

use crate::support::{call, call_err, connect};

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
    let (code, message, _) =
        call_err(&client, "get_sector_specials", json!({ "game": "zdaemon" })).await;
    assert_eq!(code, "invalid_argument", "unknown game code");
    assert!(
        message.contains("game out"),
        "the message says how to fix it: {message}"
    );
}
