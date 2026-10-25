//! `remove_lump`: the selected copy with the index it held, several names in
//! one edit reported in directory order, the last or the numbered occurrence,
//! `all_occurrences` clearing every twin at once, the whole edit rolling back
//! when any name is missing, the read-only `IWAD` lump, the empty list and the
//! occurrence shared between two names, the bad names, regions and
//! occurrences and the namespace filter.

use serde_json::json;

use crate::fixtures::pwad;
use crate::lumps::{
    add, add_twins, assert_untouched, iwad_only, path_arg, seed_three, undo_labels, working_names,
};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn remove_lump_takes_the_selected_copy_and_reports_its_old_index() {
    let client = connect().await;
    seed_three(&client).await;
    let report = call(&client, "remove_lump", json!({ "names": ["TWO"] })).await;
    assert_eq!(
        report,
        json!({ "removed": [{ "name": "TWO", "index": 1_u64 }] }),
        "the documented result shape"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["ONE", "THREE"],
        "the named lump is gone"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec![
            "add_lump ONE",
            "add_lump TWO",
            "add_lump THREE",
            "remove_lump TWO"
        ],
        "the removal is the newest history entry"
    );
}

#[tokio::test]
async fn remove_lump_takes_several_names_in_one_edit() {
    let client = connect().await;
    seed_three(&client).await;
    let report = call(&client, "remove_lump", json!({ "names": ["THREE", "ONE"] })).await;
    assert_eq!(
        report["removed"],
        json!([
            { "name": "ONE", "index": 0_u64 },
            { "name": "THREE", "index": 2_u64 }
        ]),
        "reported in directory order with the pre-removal indices"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["TWO"],
        "one edit, two gone"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec![
            "add_lump ONE",
            "add_lump TWO",
            "add_lump THREE",
            "remove_lump THREE, ONE"
        ],
        "the label lists every name in the order given"
    );
}

#[tokio::test]
async fn remove_lump_takes_the_last_copy_or_the_numbered_one() {
    let client = connect().await;
    add_twins(&client).await;
    call(
        &client,
        "add_lump",
        json!({ "name": "TWIN", "text": "third", "allow_duplicate": true }),
    )
    .await;
    let last = call(&client, "remove_lump", json!({ "names": ["TWIN"] })).await;
    assert_eq!(
        last["removed"],
        json!([{ "name": "TWIN", "index": 2_u64 }]),
        "the default occurrence is the last copy"
    );
    let first = call(
        &client,
        "remove_lump",
        json!({ "names": ["TWIN"], "occurrence": 0_u64 }),
    )
    .await;
    assert_eq!(
        first["removed"],
        json!([{ "name": "TWIN", "index": 0_u64 }]),
        "occurrence 0 is the first of what is left"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["TWIN"],
        "one twin remains, the middle one"
    );
}

#[tokio::test]
async fn remove_lump_all_occurrences_clears_every_twin_at_once() {
    let client = connect().await;
    add_twins(&client).await;
    let swept = call(
        &client,
        "remove_lump",
        json!({ "names": ["TWIN"], "all_occurrences": true }),
    )
    .await;
    assert_eq!(
        swept["removed"],
        json!([
            { "name": "TWIN", "index": 0_u64 },
            { "name": "TWIN", "index": 1_u64 }
        ]),
        "every copy went, each with the index it held"
    );
    assert_eq!(
        working_names(&client).await,
        Vec::<String>::new(),
        "the working archive is empty again"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["add_lump TWIN", "add_lump TWIN", "remove_lump TWIN"],
        "one removal for both copies"
    );
}

#[tokio::test]
async fn remove_lump_aborts_the_whole_edit_when_any_name_is_missing() {
    let client = connect().await;
    add(&client, "MINE", "m").await;
    let (code, message, _) = call_err(
        &client,
        "remove_lump",
        json!({ "names": ["MINE", "GHOST"] }),
    )
    .await;
    assert_eq!(code, "not_found", "GHOST is not in the working archive");
    assert!(
        message.contains("read-only"),
        "the message explains the layers: {message}"
    );
    assert!(
        message.contains("replace_lump"),
        "the message names the override tool: {message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["MINE"],
        "MINE stayed: the edit rolled back"
    );
    assert_eq!(
        undo_labels(&client).await,
        vec!["add_lump MINE"],
        "and no history was recorded"
    );
}

#[tokio::test]
async fn remove_lump_cannot_touch_an_iwad_lump() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_only(dir.path()).await;
    let (code, message, _) =
        call_err(&client, "remove_lump", json!({ "names": ["PLAYPAL"] })).await;
    assert_eq!(code, "not_found", "an IWAD lump cannot be removed");
    assert!(
        message.contains("read-only"),
        "the message explains the layers: {message}"
    );
    assert_untouched(&client).await;
}

#[tokio::test]
async fn remove_lump_rejects_an_empty_list_and_a_shared_occurrence() {
    let client = connect().await;
    seed_three(&client).await;
    let (empty_code, empty_message, _) =
        call_err(&client, "remove_lump", json!({ "names": [] })).await;
    assert_eq!(empty_code, "invalid_argument", "at least one name");
    assert!(
        empty_message.contains("at least one"),
        "the message states the rule: {empty_message}"
    );
    let (shared_code, shared_message, _) = call_err(
        &client,
        "remove_lump",
        json!({ "names": ["ONE", "TWO"], "occurrence": 0_u64 }),
    )
    .await;
    assert_eq!(shared_code, "invalid_argument", "occurrence needs one name");
    assert!(
        shared_message.contains("occurrence"),
        "the message names the field: {shared_message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["ONE", "TWO", "THREE"],
        "both refusals removed nothing"
    );
}

#[tokio::test]
async fn remove_lump_rejects_bad_names_regions_and_occurrences() {
    let client = connect().await;
    seed_three(&client).await;
    let (name_code, _, _) =
        call_err(&client, "remove_lump", json!({ "names": ["TOO LONG"] })).await;
    assert_eq!(name_code, "invalid_argument", "an unparsable name");
    let (region_code, _, _) = call_err(
        &client,
        "remove_lump",
        json!({ "names": ["ONE"], "namespace": "nope" }),
    )
    .await;
    assert_eq!(region_code, "invalid_argument", "not a namespace");
    let (past_code, past_message, _) = call_err(
        &client,
        "remove_lump",
        json!({ "names": ["ONE"], "occurrence": 5_u64 }),
    )
    .await;
    assert_eq!(past_code, "not_found", "no fifth copy of ONE");
    assert!(
        past_message.contains("0 to 0"),
        "the message names the range: {past_message}"
    );
    assert_eq!(
        working_names(&client).await,
        vec!["ONE", "TWO", "THREE"],
        "every refusal removed nothing"
    );
}

#[tokio::test]
async fn remove_lump_honours_the_namespace_filter() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let extra = pwad(dir.path());
    call(&client, "open_wad", path_arg(&extra)).await;
    let report = call(
        &client,
        "remove_lump",
        json!({ "names": ["FLOOR1"], "namespace": "flats" }),
    )
    .await;
    assert_eq!(
        report["removed"],
        json!([{ "name": "FLOOR1", "index": 1_u64 }]),
        "the region copy went"
    );
    assert_eq!(
        working_names(&client).await,
        vec![
            "FF_START", "FF_END", "DEHACKED", "MAP02", "THINGS", "LINEDEFS", "SIDEDEFS",
            "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT", "BLOCKMAP",
        ],
        "the region's markers stayed"
    );
}
