//! `set_light_effect` landing every effect in its family's basic-type field
//! with the bits above it intact, the optional light level riding along, a
//! `Hexen` secret surviving an effect write, and the one-step undo each of
//! the two special tools answers with. The shared refusal pass covers both
//! tools' bad-effect, bad-level and out-of-range-sector arguments. The
//! `secret` module's `pub(super)` case machinery - `EffectCase`,
//! `doom_cases`, `hexen_cases`, `set_special` and `refuse_secret` - drives
//! the effect tables and that pass; `refuse_light` is this module's own.

use serde_json::{Value, json};

use crate::features::secret::{
    EffectCase, doom_cases, hexen_cases, refuse_missing_secret, refuse_secret, set_special,
};
use crate::features::{
    Client, expect_rooms, features_loaded, formats_loaded, validate_clean, working_sector,
    working_sectors,
};
use crate::support::{call, call_err};

/// Requires `set_light_effect` to reject `args` with `invalid_argument`
/// naming `needle`.
async fn refuse_light(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "set_light_effect", args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {note}");
    assert!(
        message.contains(needle),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
}

/// Runs one [`EffectCase`] against sector 0 of its map, which starts at
/// `from`: a bit above each family's basic-type field plus a vanilla
/// type inside it, so a correct write shows on both sides of the edge.
async fn expect_effect(client: &Client, case: &EffectCase, from: i32) {
    set_special(client, case.map, 0_usize, from).await;
    let result = call(
        client,
        "set_light_effect",
        json!({"map": case.map, "sectors": [0_i32], "effect": case.effect}),
    )
    .await;
    let name = case.effect;
    assert_eq!(result["effect"], json!(name), "{name} echoes canonically");
    assert_eq!(
        result["family"],
        json!(case.family),
        "{name} reads its family"
    );
    assert_eq!(
        result["special"],
        json!(case.basic),
        "{name} is the family's type"
    );
    assert_eq!(result["light"], Value::Null, "{name} asked for no level");
    assert_eq!(result["sectors"], json!([0_i32]), "{name} moved one row");
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "{name} is a property edit"
    );
    let row = working_sector(client, case.map, 0_usize).await;
    assert_eq!(
        row["special"],
        json!(case.stored),
        "{name} wrote only the type field of {from}"
    );
    assert_eq!(row["light"], json!(160_i32), "{name} left the level alone");
}

/// Runs every case of a family's table against its map's sector 0,
/// resetting that sector to `from` before each call.
async fn expect_effects(client: &Client, cases: &[EffectCase], from: i32) {
    for case in cases {
        expect_effect(client, case, from).await;
    }
}

#[tokio::test]
async fn every_light_effect_lands_in_the_doom_type_field() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    expect_effects(&client, &doom_cases(), 4_105_i32).await;
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn every_light_effect_lands_in_the_hexen_type_field() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    expect_effects(&client, &hexen_cases(), 1_289_i32).await;
}

#[tokio::test]
async fn a_light_effect_keeps_a_hexen_sector_secret() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    call(
        &client,
        "mark_secret",
        json!({"map": "MAP04", "sectors": [0_i32]}),
    )
    .await;
    call(
        &client,
        "set_light_effect",
        json!({"map": "MAP04", "sectors": [0_i32], "effect": "flicker"}),
    )
    .await;
    assert_eq!(
        working_sector(&client, "MAP04", 0_usize).await["special"],
        json!(1_105_i32),
        "1024 | 81: the light repaint left the secret bit at 1024 alone"
    );
    let zdoom = call(
        &client,
        "set_light_effect",
        json!({"map": "MAP03", "sectors": [1_i32], "effect": "strobe_slow", "light": 64_i32}),
    )
    .await;
    assert_eq!(
        zdoom["family"],
        json!("hexen"),
        "a zdoom UDMF map takes the Hexen numbers too"
    );
    assert_eq!(zdoom["special"], json!(67_i32), "3 plus 64");
    assert_eq!(zdoom["sectors"], json!([1_i32]), "the row moved");
    let room = working_sector(&client, "MAP03", 1_usize).await;
    assert_eq!(room["special"], json!(67_i32), "the stored number");
    assert_eq!(room["light"], json!(64_i32), "and the level rode along");
}

#[tokio::test]
async fn a_light_level_travels_with_the_effect() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "set_light_effect",
        json!({"sectors": [0_i32, 1_i32], "effect": "glow", "light": 96_i32}),
    )
    .await;
    assert_eq!(result["light"], json!(96_i32), "the level the call applied");
    assert_eq!(result["sectors"], json!([0_i32, 1_i32]), "both rooms");
    let rooms = working_sectors(&client, "MAP01").await;
    expect_rooms(&rooms, 96_i32, 8_i32);
    let again = call(
        &client,
        "set_light_effect",
        json!({"sectors": [0_i32, 1_i32], "effect": "glow", "light": 96_i32}),
    )
    .await;
    assert_eq!(
        again["sectors"],
        json!([]),
        "the same request changed nothing"
    );
    let level = call(
        &client,
        "set_light_effect",
        json!({"sectors": [1_i32], "effect": "glow", "light": 128_i32}),
    )
    .await;
    assert_eq!(
        level["sectors"],
        json!([1_i32]),
        "a level alone counts as a change, the effect already standing"
    );
    assert_eq!(
        working_sector(&client, "MAP01", 1_usize).await["light"],
        json!(128_i32)
    );
}

#[tokio::test]
async fn a_secret_mark_and_a_light_effect_undo_as_one_step_each() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(&client, "mark_secret", json!({"sectors": [0_i32]})).await;
    call(
        &client,
        "set_light_effect",
        json!({"sectors": [1_i32], "effect": "flicker"}),
    )
    .await;
    let undone = call(&client, "undo", json!({"steps": 2_usize})).await;
    assert_eq!(
        undone["undone"],
        json!(["set light effect", "mark secret"]),
        "each call is its own undo entry, rolled back newest first"
    );
    let merged = call(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "merged", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        merged["sectors"]
            .as_array()
            .expect("sectors is an array")
            .iter()
            .map(|row| row["special"].clone())
            .collect::<Vec<Value>>(),
        vec![json!(0_i32), json!(0_i32)],
        "the layer was never written"
    );
}

#[tokio::test]
async fn the_two_special_tools_validate_their_arguments() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_secret(&client, json!({"sectors": []}), "sectors is empty").await;
    refuse_missing_secret(
        &client,
        json!({"sectors": [99_i32]}),
        "sector 99 does not exist",
    )
    .await;
    refuse_light(
        &client,
        json!({"sectors": [], "effect": "glow"}),
        "sectors is empty",
    )
    .await;
    refuse_light(
        &client,
        json!({"sectors": [0_i32], "effect": "pulsate"}),
        "none, flicker, strobe_fast, strobe_slow, glow, random_off",
    )
    .await;
    refuse_light(
        &client,
        json!({"sectors": [0_i32], "effect": "glow", "light": 256_i32}),
        "light 256 is out of range",
    )
    .await;
    refuse_light(
        &client,
        json!({"sectors": [0_i32], "effect": "glow", "light": -1_i32}),
        "light -1 is out of range",
    )
    .await;
    let (light_code, light_message, light_hint) = call_err(
        &client,
        "set_light_effect",
        json!({"sectors": [42_i32], "effect": "glow"}),
    )
    .await;
    assert_eq!(light_code, "not_found", "a missing sector is not_found");
    assert!(
        light_message.contains("sector 42 does not exist"),
        "the refusal names the sector: {light_message}"
    );
    assert_eq!(
        light_hint, "describe_map or find_elements returns indices",
        "the shared index hint names the readers"
    );
    let (code, _, _) = call_err(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        code, "not_found",
        "no refused special even copied the block into the working archive"
    );
}
