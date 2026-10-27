//! `edit_linedef` as an MCP client calls it: replacing and adjusting the flag
//! bits, `MBF21`'s two `Doom`-family blocks stored by name with the `Hexen`
//! names refused, setting `special` and `tag` on a list of lines, the
//! nothing-to-change
//! empty, conflicting and null requests refused with the missing references as
//! `not_found` and the collapsed line refused, the family refusals with their
//! format-aware advice, the flip verified through
//! `assemble_map`, the
//! `back: null` closing a line one-sided, the in-place `front_side`
//! texture-offset edit, and moving the vertex and sidedef references.

use serde_json::{Value, json};

use crate::edit_linedefs::{doom_loaded, merged_linedef, miss, refuse, working_linedef};
use crate::support::{call, call_err};

#[tokio::test]
async fn edit_linedef_replaces_and_adjusts_the_flag_bits() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let replaced = call(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "flags": ["blocking", "mapped"]}),
    )
    .await;
    assert_eq!(replaced["linedefs"][0]["flags"], json!(257_i32), "1 | 256");
    assert_eq!(
        replaced["linedefs"][0]["flag_names"],
        json!(["blocking", "mapped"]),
        "named back"
    );
    assert_eq!(
        replaced["nodes_invalidated"],
        json!(false),
        "flags are a property edit"
    );
    let set = call(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "set_flags": ["secret"]}),
    )
    .await;
    assert_eq!(set["linedefs"][0]["flags"], json!(289_i32), "257 | 32");
    let cleared = call(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "clear_flags": ["mapped", "secret"]}),
    )
    .await;
    assert_eq!(
        cleared["linedefs"][0]["flags"],
        json!(1_i32),
        "back to blocking"
    );
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "flags": 1_i32, "set_flags": ["secret"]}),
    )
    .await;
    let stored = working_linedef(&client, "MAP01", 0).await;
    assert_eq!(stored["flag_names"], json!(["blocking"]), "read back");
}

#[tokio::test]
async fn edit_linedef_stores_the_mbf21_bits_on_a_doom_map() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let set = call(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "set_flags": ["blockplayers", "blocklandmonsters"]}),
    )
    .await;
    assert_eq!(
        set["linedefs"][0]["flags"],
        json!(1_i32 + 4_096_i32 + 8_192_i32),
        "blocking plus the two MBF21 bits"
    );
    assert_eq!(
        set["linedefs"][0]["flag_names"],
        json!(["blocking", "blocklandmonsters", "blockplayers"]),
        "named back in the Doom table's order"
    );
    let stored = working_linedef(&client, "MAP01", 0).await;
    assert_eq!(
        stored["flags"],
        json!(12_289_i32),
        "the binary u16 column holds both"
    );
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "set_flags": ["blockeverything"]}),
    )
    .await;
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "set_flags": ["repeatspecial"]}),
    )
    .await;
    let intact = working_linedef(&client, "MAP01", 0).await;
    assert_eq!(
        intact["flags"],
        json!(12_289_i32),
        "the two refusals changed nothing"
    );
}

#[tokio::test]
async fn edit_linedef_sets_special_and_tag_on_lists() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "edit_linedef",
        json!({"index": [4_i32, 5_i32, 6_i32], "special": 12_i32, "tag": 1_i32}),
    )
    .await;
    let rows = result["linedefs"].as_array().expect("a linedefs array");
    assert_eq!(rows.len(), 3, "three rows reported");
    assert_eq!(rows[0]["index"], json!(4_i32), "ascending");
    assert_eq!(rows[2]["index"], json!(6_i32));
    assert_eq!(rows[0]["special"], json!(12_i32), "the special landed");
    assert_eq!(rows[2]["tag"], json!(1_i32), "on every selected line");
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "specials and tags are not geometry"
    );
    let stored = working_linedef(&client, "MAP01", 6).await;
    assert_eq!(stored["special"], json!(12_i32), "written and read back");
    assert_eq!(stored["front"]["sidedef"], json!(7_i32), "the sides stand");
}

#[tokio::test]
async fn edit_linedef_refuses_empty_conflicting_and_null_requests() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    refuse(&client, "edit_linedef", json!({"index": 0_i32})).await;
    refuse(&client, "edit_linedef", json!({"index": []})).await;
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "front": null}),
    )
    .await;
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "back": null, "back_side": {"middle": "X"}}),
    )
    .await;
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "front_side": {}}),
    )
    .await;
    miss(
        &client,
        "edit_linedef",
        json!({"index": 99_i32, "special": 1_i32}),
    )
    .await;
    miss(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "v2": 99_i32}),
    )
    .await;
    miss(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "back": 99_i32}),
    )
    .await;
    let (zero_code, zero_message, _) = call_err(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "v2": 0_i32}),
    )
    .await;
    assert_eq!(zero_code, "invalid_argument", "zero-length refusal");
    assert!(
        zero_message.contains("zero-length"),
        "the message names the rule: {zero_message}"
    );
    let stored = merged_linedef(&client, "MAP01", 0).await;
    assert_eq!(stored["special"], json!(0_i32), "nothing changed");
    assert_eq!(stored["v2"]["index"], json!(5_i32), "the endpoint stands");
}

#[tokio::test]
async fn edit_linedef_args_advice_follows_the_map_format() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let (code, binary_message, _) = call_err(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "args": [7_i32, 0_i32, 0_i32, 0_i32, 0_i32]}),
    )
    .await;
    assert_eq!(code, "invalid_argument", "binary Doom has no args");
    assert!(
        binary_message.contains("Doom-format linedefs have no args; use a Hexen or UDMF map"),
        "the binary advice names the other formats: {binary_message}"
    );
    call(
        &client,
        "convert_map_format",
        json!({"map": "MAP01", "to": "udmf"}),
    )
    .await;
    let (udmf_code, udmf_message, _) = call_err(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "args": [7_i32, 0_i32, 0_i32, 0_i32, 0_i32]}),
    )
    .await;
    assert_eq!(
        udmf_code, "invalid_argument",
        "UDMF doom namespace has no args"
    );
    assert!(
        udmf_message.contains("convert_map_format")
            && udmf_message.contains("zdoom")
            && udmf_message.contains("Hexen family"),
        "the UDMF advice names the namespace conversion: {udmf_message}"
    );
    assert!(
        !udmf_message.contains("use a Hexen or UDMF map"),
        "it never tells a UDMF map to become UDMF: {udmf_message}"
    );
    let stored = merged_linedef(&client, "MAP01", 0).await;
    assert_eq!(
        stored["args"],
        json!([0_i32, 0_i32, 0_i32, 0_i32, 0_i32]),
        "the refusals wrote nothing"
    );
}

#[tokio::test]
async fn edit_linedef_flip_keeps_the_front_on_the_right() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "edit_linedef",
        json!({"index": 2_i32, "flip": true}),
    )
    .await;
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "a flipped line is geometry"
    );
    assert_eq!(
        result["linedefs"][0]["v1"],
        json!(1_i32),
        "the ends swapped"
    );
    assert_eq!(result["linedefs"][0]["v2"], json!(4_i32));
    assert_eq!(
        result["linedefs"][0]["front"],
        json!(3_i32),
        "the old back fronts now"
    );
    assert_eq!(result["linedefs"][0]["back"], json!(2_i32));
    let row = working_linedef(&client, "MAP01", 2).await;
    assert_eq!(row["v1"]["x"], json!(256_i32), "v1 at the old end");
    assert_eq!(row["v1"]["y"], json!(0_i32), "the hall floor corner");
    assert_eq!(row["v2"]["y"], json!(256_i32), "v2 at the old start");
    assert_eq!(
        row["front"]["sector"],
        json!(1_i32),
        "the raised room's side stayed the physical left, now the right"
    );
    assert_eq!(row["back"]["sector"], json!(0_i32), "the hall side trails");
    assert_eq!(row["flag_names"], json!(["twosided"]), "flags untouched");
}

#[tokio::test]
async fn edit_linedef_back_null_closes_the_line_one_sided() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "edit_linedef",
        json!({"index": 2_i32, "back": null}),
    )
    .await;
    assert_eq!(
        result["linedefs"][0]["back"],
        Value::Null,
        "the reference went"
    );
    assert_eq!(result["linedefs"][0]["flags"], json!(1_i32), "blocking on");
    assert_eq!(
        result["linedefs"][0]["flag_names"],
        json!(["blocking"]),
        "twosided cleared"
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "losing a side is geometry"
    );
    let row = working_linedef(&client, "MAP01", 2).await;
    assert_eq!(row["back"], Value::Null, "and the map agrees");
    assert_eq!(row["front"]["sidedef"], json!(2_i32), "the front stands");
}

#[tokio::test]
async fn edit_linedef_front_side_edits_the_referenced_sidedef_offsets() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "edit_linedef",
        json!({"index": 2_i32, "front_side": {"x_offset": 16_i32, "upper": "brick"}}),
    )
    .await;
    assert_eq!(
        result["linedefs"][0]["front"],
        json!(2_i32),
        "sidedef 2 fronts"
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "textures and offsets are not geometry"
    );
    let row = working_linedef(&client, "MAP01", 2).await;
    assert_eq!(row["front"]["x_offset"], json!(16_i32), "the shift landed");
    assert_eq!(
        row["front"]["upper"],
        json!("BRICK"),
        "the texture uppercased"
    );
    assert_eq!(
        row["front"]["lower"],
        json!("STEP1"),
        "unasked fields stand"
    );
    assert_eq!(
        row["front"]["middle"],
        json!("-"),
        "as the fixture had them"
    );
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 0_i32, "back_side": {"middle": "X"}}),
    )
    .await;
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 2_i32, "front_side": {"x_offset": 40_000_i32}}),
    )
    .await;
}

#[tokio::test]
async fn edit_linedef_moves_references_and_rejects_a_collapsed_line() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "edit_linedef",
        json!({"index": 3_i32, "v1": 2_i32, "front": 4_i32}),
    )
    .await;
    assert_eq!(result["linedefs"][0]["v1"], json!(2_i32), "the start moved");
    assert_eq!(
        result["linedefs"][0]["front"],
        json!(4_i32),
        "the side moved"
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "moving an endpoint is geometry"
    );
    refuse(
        &client,
        "edit_linedef",
        json!({"index": 3_i32, "v2": 2_i32}),
    )
    .await;
    let stored = working_linedef(&client, "MAP01", 3).await;
    assert_eq!(
        stored["v1"]["x"],
        json!(512_i32),
        "the line now starts at 512,0"
    );
    assert_eq!(
        stored["front"]["sidedef"],
        json!(4_i32),
        "and the new front read back"
    );
}
