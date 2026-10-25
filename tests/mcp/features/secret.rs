//! `mark_secret` writing the vanilla `9`, the `Boom` bit 128 (with its
//! warning over a vanilla type) or the `Hexen` bit 1024, and clearing each
//! again. The two special tools' shared machinery lives `pub(super)` beside
//! these tests for the `light` module: the `set_special` fixture step, the
//! `mark_secret` refusal and the `EffectCase` light-effect case tables
//! (`doom_cases`, `hexen_cases`).

use serde_json::{Value, json};

use crate::features::{
    Client, features_loaded, formats_loaded, validate_clean, working_sector, working_sectors,
};
use crate::support::{call, call_err};

/// Gives sector `index` of `map` the special `value`, so a special tool
/// has something to keep (or warn) about.
pub(super) async fn set_special(client: &Client, map: &str, index: usize, value: i32) {
    call(
        client,
        "edit_sector",
        json!({"map": map, "index": index, "special": value}),
    )
    .await;
}

/// Requires `mark_secret` to reject `args` with `invalid_argument`
/// naming `needle`.
pub(super) async fn refuse_secret(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "mark_secret", args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {note}");
    assert!(
        message.contains(needle),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
}

#[tokio::test]
async fn a_secret_mark_follows_the_doom_flag_family() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let marked = call(&client, "mark_secret", json!({"sectors": [0_i32]})).await;
    assert_eq!(marked["map"], json!("MAP01"), "the only map is default");
    assert_eq!(marked["secret"], json!(true), "marking is the default");
    assert_eq!(marked["family"], json!("doom"), "the vanilla family secret");
    assert_eq!(marked["sectors"], json!([0_i32]), "one row moved");
    assert_eq!(
        marked["warnings"],
        json!([]),
        "the vanilla type needs no note"
    );
    assert_eq!(
        marked["nodes_invalidated"],
        json!(false),
        "a sector property is not geometry"
    );
    assert_eq!(marked["copied_to_working"], json!(true));
    assert_eq!(
        working_sector(&client, "MAP01", 0_usize).await["special"],
        json!(9_i32),
        "the vanilla secret type"
    );
    let again = call(&client, "mark_secret", json!({"sectors": [0_i32]})).await;
    assert_eq!(
        again["sectors"],
        json!([]),
        "already secret: nothing changed"
    );
    let plain = call(
        &client,
        "mark_secret",
        json!({"sectors": [0_i32], "secret": false}),
    )
    .await;
    assert_eq!(plain["secret"], json!(false), "the clear is echoed");
    assert_eq!(
        plain["sectors"],
        json!([0_i32]),
        "and it moved the row back"
    );
    assert_eq!(
        working_sector(&client, "MAP01", 0_usize).await["special"],
        json!(0_i32),
        "the vanilla 9 clears to plain"
    );
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn a_boom_secret_mark_keeps_every_other_bit() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    set_special(&client, "MAP01", 0_usize, 32_i32).await;
    let boom = call(&client, "mark_secret", json!({"sectors": [0_i32]})).await;
    assert_eq!(boom["sectors"], json!([0_i32]), "the Boom bit landed");
    assert_eq!(
        boom["warnings"],
        json!([]),
        "a generalized type is Boom's own"
    );
    assert_eq!(
        working_sector(&client, "MAP01", 0_usize).await["special"],
        json!(160_i32),
        "32 | 128, the generalized field intact"
    );
    let unmarked = call(
        &client,
        "mark_secret",
        json!({"sectors": [0_i32], "secret": false}),
    )
    .await;
    assert_eq!(unmarked["sectors"], json!([0_i32]));
    assert_eq!(
        working_sector(&client, "MAP01", 0_usize).await["special"],
        json!(32_i32),
        "exactly the flag came back off"
    );
}

#[tokio::test]
async fn a_vanilla_secret_mark_carries_a_boom_warning() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    set_special(&client, "MAP01", 1_usize, 17_i32).await;
    let warned = call(&client, "mark_secret", json!({"sectors": [1_i32]})).await;
    assert_eq!(
        warned["sectors"],
        json!([1_i32]),
        "the vanilla type took the bit too"
    );
    let notes = warned["warnings"].as_array().expect("warnings is an array");
    assert_eq!(notes.len(), 1_usize, "one note: {notes:#?}");
    let note = notes.first().and_then(Value::as_str).expect("a string");
    assert!(
        note.contains("vanilla Doom ignores Boom bits"),
        "the note says what vanilla Doom does not read: {note}"
    );
    assert!(note.contains("sector 1"), "and names the sector: {note}");
    assert_eq!(
        working_sector(&client, "MAP01", 1_usize).await["special"],
        json!(145_i32),
        "17 | 128"
    );
    let back = call(
        &client,
        "mark_secret",
        json!({"sectors": [1_i32], "secret": false}),
    )
    .await;
    assert_eq!(
        back["warnings"],
        json!([]),
        "clearing has nothing to warn about"
    );
    assert_eq!(
        working_sector(&client, "MAP01", 1_usize).await["special"],
        json!(17_i32),
        "the flicker is the plain number again"
    );
}

#[tokio::test]
async fn a_secret_mark_follows_the_hexen_flag_family() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    set_special(&client, "MAP04", 0_usize, 81_i32).await;
    let marked = call(
        &client,
        "mark_secret",
        json!({"map": "MAP04", "sectors": [0_i32, 1_i32]}),
    )
    .await;
    assert_eq!(
        marked["family"],
        json!("hexen"),
        "binary Hexen is its own family"
    );
    assert_eq!(marked["warnings"], json!([]), "the Hexen bit never warns");
    assert_eq!(marked["sectors"], json!([0_i32, 1_i32]), "both rows moved");
    let rooms = working_sectors(&client, "MAP04").await;
    assert_eq!(
        rooms.first().expect("sector 0")["special"],
        json!(1_105_i32),
        "81 | 1024 keeps the sector's own effect"
    );
    assert_eq!(
        rooms.get(1).expect("sector 1")["special"],
        json!(1_024_i32),
        "a plain sector takes the bare bit"
    );
    let cleared = call(
        &client,
        "mark_secret",
        json!({"map": "MAP04", "sectors": [0_i32, 1_i32], "secret": false}),
    )
    .await;
    assert_eq!(cleared["sectors"], json!([0_i32, 1_i32]), "both cleared");
    let back = working_sectors(&client, "MAP04").await;
    assert_eq!(back.first().expect("sector 0")["special"], json!(81_i32));
    assert_eq!(back.get(1).expect("sector 1")["special"], json!(0_i32));
    let udmf = call(
        &client,
        "mark_secret",
        json!({"map": "MAP03", "sectors": [1_i32]}),
    )
    .await;
    assert_eq!(
        udmf["family"],
        json!("hexen"),
        "a zdoom UDMF map is Hexen-family too"
    );
    assert_eq!(
        working_sector(&client, "MAP03", 1_usize).await["special"],
        json!(1_024_i32),
        "a plain sector of a zdoom map takes the bare bit, its tag untouched"
    );
    assert_eq!(
        working_sector(&client, "MAP03", 1_usize).await["tag"],
        json!(1_i32),
        "the tool writes the special column only"
    );
}

/// One `set_light_effect` case: the map to edit, the family it reports,
/// the effect to send, the basic type that family stores and the whole
/// special the sector must end with.
pub(super) struct EffectCase {
    /// The map to edit.
    pub(super) map: &'static str,
    /// The flag family the tool reports.
    pub(super) family: &'static str,
    /// The effect name to send.
    pub(super) effect: &'static str,
    /// The basic type this family stores for it.
    pub(super) basic: i32,
    /// The whole special after the effect lands: every bit outside the
    /// family's type field kept.
    pub(super) stored: i32,
}

/// The `Doom` family's table: each effect's vanilla type under the
/// generalized bit (bit 12) the sector carried all along.
pub(super) fn doom_cases() -> [EffectCase; 6] {
    let rows = [
        ("flicker", 17_i32, 4_113_i32),
        ("strobe_fast", 2_i32, 4_098_i32),
        ("strobe_slow", 3_i32, 4_099_i32),
        ("glow", 8_i32, 4_104_i32),
        ("random_off", 1_i32, 4_097_i32),
        ("none", 0_i32, 4_096_i32),
    ];
    rows.map(|(effect, basic, stored)| EffectCase {
        map: "MAP01",
        family: "doom",
        effect,
        basic,
        stored,
    })
}

/// The `Hexen` family's table: the same effects `64` higher, under the
/// flag at 256 and the secret bit at 1024 the sector carried.
pub(super) fn hexen_cases() -> [EffectCase; 6] {
    let rows = [
        ("flicker", 81_i32, 1_361_i32),
        ("strobe_fast", 66_i32, 1_346_i32),
        ("strobe_slow", 67_i32, 1_347_i32),
        ("glow", 72_i32, 1_352_i32),
        ("random_off", 65_i32, 1_345_i32),
        ("none", 0_i32, 1_280_i32),
    ];
    rows.map(|(effect, basic, stored)| EffectCase {
        map: "MAP04",
        family: "hexen",
        effect,
        basic,
        stored,
    })
}
