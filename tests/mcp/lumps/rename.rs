//! `rename_lump`: a working lump moving to its new name with its payload and
//! position kept, the taken name refused until `allow_duplicate`, the twin it
//! then allows and the label history records, the over-long new name and the
//! missing lump, the read-only `IWAD` lump and the copy-on-write block an
//! override stood in the working archive.

use serde_json::json;

use crate::lumps::{
    add, assert_untouched, iwad_only, names, replace_hex, undo_labels, working_names,
};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn rename_lump_moves_a_working_lump_and_keeps_its_bytes() {
    let client = connect().await;
    add(&client, "OLD", "payload").await;
    let report = call(
        &client,
        "rename_lump",
        json!({ "name": "old", "new_name": "new" }),
    )
    .await;
    assert_eq!(
        report,
        json!({ "old_name": "OLD", "new_name": "NEW", "index": 0_u64 }),
        "the documented result shape, both names uppercased"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["NEW"],
        "renamed in place"
    );
    let read = call(
        &client,
        "get_lump",
        json!({ "name": "NEW", "scope": "working", "format": "text" }),
    )
    .await;
    assert_eq!(read["text"], json!("payload"), "the payload survived");
    assert_eq!(
        undo_labels(&client).await,
        vec!["add_lump OLD", "rename_lump OLD -> NEW"],
        "the label shows both names"
    );
}

#[tokio::test]
async fn rename_lump_refuses_a_taken_name_unless_told_otherwise() {
    let client = connect().await;
    add(&client, "A", "1").await;
    add(&client, "B", "2").await;
    let (code, message, _) = call_err(
        &client,
        "rename_lump",
        json!({ "name": "A", "new_name": "B" }),
    )
    .await;
    assert_eq!(code, "already_exists", "B is taken in the same region");
    assert!(
        message.contains("allow_duplicate"),
        "the message names the fix: {message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["A", "B"],
        "the refusal renamed nothing"
    );
}

#[tokio::test]
async fn rename_lump_allows_a_twin_when_asked_and_labels_the_edit() {
    let client = connect().await;
    add(&client, "A", "1").await;
    add(&client, "B", "2").await;
    let forced = call(
        &client,
        "rename_lump",
        json!({ "name": "A", "new_name": "B", "allow_duplicate": true }),
    )
    .await;
    assert_eq!(forced["new_name"], json!("B"), "the twin was allowed");
    assert_eq!(forced["index"], json!(0_u64), "still in place");
    assert_eq!(
        working_names(&client).await,
        vec!["B", "B"],
        "both copies now carry the name"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["add_lump A", "add_lump B", "rename_lump A -> B"],
        "the accepted rename is the newest step"
    );
}

#[tokio::test]
async fn rename_lump_rejects_a_bad_new_name_and_a_missing_lump() {
    let client = connect().await;
    add(&client, "MINE", "m").await;
    let (long_code, long_message, _) = call_err(
        &client,
        "rename_lump",
        json!({ "name": "MINE", "new_name": "TOO LONG FOR WAD" }),
    )
    .await;
    assert_eq!(
        long_code, "invalid_argument",
        "the new name is over eight bytes"
    );
    assert!(
        long_message.contains("limit is 8"),
        "the message states the limit: {long_message}"
    );
    let (bad_code, _, _) = call_err(
        &client,
        "rename_lump",
        json!({ "name": "GHOST", "new_name": "OK" }),
    )
    .await;
    assert_eq!(
        bad_code, "not_found",
        "the working archive has no such lump"
    );
    let (region_code, _, _) = call_err(
        &client,
        "rename_lump",
        json!({ "name": "MINE", "new_name": "OK", "namespace": "nope" }),
    )
    .await;
    assert_eq!(region_code, "invalid_argument", "not a namespace");
    assert_eq!(
        working_names(&client).await,
        vec!["MINE"],
        "no refusal changed anything"
    );
}

#[tokio::test]
async fn rename_lump_cannot_touch_an_iwad_lump() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    let (code, message, _) = call_err(
        &client,
        "rename_lump",
        json!({ "name": "FLOOR1", "new_name": "FLOORX" }),
    )
    .await;
    assert_eq!(code, "not_found", "an IWAD lump cannot be renamed");
    assert!(
        message.contains("replace_lump"),
        "the message names the override tool: {message}"
    );
    assert_untouched(&client).await;
}

#[tokio::test]
async fn rename_lump_reaches_the_copy_on_write_block_it_was_given() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    call(&client, "replace_lump", replace_hex("THINGS", "AA")).await;
    let report = call(
        &client,
        "rename_lump",
        json!({ "name": "THINGS", "new_name": "STUFF" }),
    )
    .await;
    assert_eq!(report["index"], json!(1_u64), "the copied member");
    assert_eq!(
        working_names(&client).await.get(1),
        Some(&"STUFF".to_owned()),
        "the working copy was renamed, the IWAD's was not"
    );
    let layer = call(
        &client,
        "list_lumps",
        json!({ "scope": "layer:0", "map": "MAP01" }),
    )
    .await;
    assert_eq!(
        names(&layer).get(1),
        Some(&"THINGS".to_owned()),
        "the read-only block still spells THINGS"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["replace_lump THINGS", "rename_lump THINGS -> STUFF"],
        "both edits are undoable steps"
    );
}
