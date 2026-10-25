//! The `list_lumps` tool as a client sees it: the merged view's rows with the
//! layer that won each name and the offsets one `save_wad` would write, the
//! `layer:N` scope reading one archive with its own layout, the `working`
//! scope reading the editable archive, the out-of-range layer and the bogus
//! scope, namespace, kind and limit refusals, the namespace, kind and map
//! filters, a graphics region keeping a weakly-guessed payload its own kind,
//! and the offset/limit/total paging.

use serde_json::{Value, json};

use crate::fixtures::{iwad, pwad};
use crate::lumps::{
    COLORMAP_BYTES, FLAT_BYTES, HEADER_BYTES, MAP_BYTES, PLAYPAL_BYTES, iwad_plus_pwad, names,
    row_named,
};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn list_lumps_merges_layers_and_reports_save_layout_offsets() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let report = call(&client, "list_lumps", json!({ "limit": 100_u64 })).await;
    assert_eq!(report["scope"], json!("merged"), "the default scope");
    assert_eq!(report["total"].as_u64(), Some(45), "iwad 33 + 12 additions");
    assert_eq!(report["offset"].as_u64(), Some(0), "echoed offset");
    assert_eq!(report["count"].as_u64(), Some(45), "one page");
    let floor = row_named(&report, "FLOOR1");
    assert_eq!(
        floor["index"].as_u64(),
        Some(14),
        "in place in the flats region, after the full MAP01 block"
    );
    assert_eq!(floor["size"].as_u64(), Some(FLAT_BYTES), "the PWAD's copy");
    assert_eq!(floor["kind"], json!("flat"), "classified in its region");
    assert_eq!(floor["namespace"], json!("flats"), "flats namespace");
    assert_eq!(floor["map"], Value::Null, "not map data");
    assert_eq!(
        floor["source"],
        json!({ "layer": 1_u64 }),
        "the PWAD's FLOOR1 won the region",
    );
    // The PWAD lays out: 12-byte header, empty FF_START, then FLOOR1.
    assert_eq!(
        floor["offset"],
        json!(HEADER_BYTES),
        "data right after the header"
    );
    let dehacked = row_named(&report, "DEHACKED");
    assert_eq!(
        dehacked["source"],
        json!({ "layer": 1_u64 }),
        "the PWAD's global"
    );
    assert_eq!(
        dehacked["offset"].as_u64(),
        Some(HEADER_BYTES.checked_add(FLAT_BYTES).expect("small sum")),
        "after the header and the 4096-byte flat the empty markers surround",
    );
    assert_eq!(
        row_named(&report, "PLAYPAL")["source"],
        json!({"layer": 0_u64}),
        "base copy"
    );
    assert_eq!(
        row_named(&report, "ENDOOM")["kind"],
        json!("endoom"),
        "size-matched kind"
    );
}

#[tokio::test]
async fn list_lumps_layer_scope_reads_one_archive_with_its_own_offsets() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad(dir.path());
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    let report = call(&client, "list_lumps", json!({ "scope": "layer:0" })).await;
    assert_eq!(report["scope"], json!({ "layer": 0_u64 }), "echoed scope");
    assert_eq!(report["total"].as_u64(), Some(33), "the IWAD fixture alone");
    assert_eq!(
        row_named(&report, "FLOOR1")["offset"].as_u64(),
        Some(
            HEADER_BYTES
                .checked_add(PLAYPAL_BYTES)
                .and_then(|sum| sum.checked_add(COLORMAP_BYTES))
                .and_then(|sum| sum.checked_add(MAP_BYTES))
                .expect("small sums")
        ),
        "header + PLAYPAL + COLORMAP + the MAP01 block's member data",
    );
    assert_eq!(
        row_named(&report, "FLOOR1")["source"],
        json!({ "layer": 0_u64 }),
        "layer rows carry the layer source",
    );
}

#[tokio::test]
async fn list_lumps_working_scope_reads_the_editable_archive() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let extra = pwad(dir.path());
    call(
        &client,
        "open_wad",
        json!({ "path": extra.display().to_string() }),
    )
    .await;
    let report = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(report["scope"], json!("working"), "echoed scope");
    assert_eq!(report["total"].as_u64(), Some(15), "the PWAD fixture alone");
    assert_eq!(
        names(&report),
        vec![
            "FF_START", "FLOOR1", "FF_END", "DEHACKED", "MAP02", "THINGS", "LINEDEFS", "SIDEDEFS",
            "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT", "BLOCKMAP",
        ],
        "the working archive keeps its own markers and order",
    );
    assert_eq!(
        row_named(&report, "FLOOR1")["source"],
        json!("working"),
        "working source"
    );
}

#[tokio::test]
async fn list_lumps_reports_an_out_of_range_layer_as_not_found() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let (code, message, _) = call_err(&client, "list_lumps", json!({ "scope": "layer:9" })).await;
    assert_eq!(code, "not_found", "no layer 9");
    assert!(
        message.contains("layer 9"),
        "the message names the missing layer: {message}"
    );
}

#[tokio::test]
async fn list_lumps_rejects_a_bogus_scope_and_filters() {
    let client = connect().await;
    let (code, message, _) = call_err(&client, "list_lumps", json!({ "scope": "bogus" })).await;
    assert_eq!(code, "invalid_argument", "unknown scope");
    assert!(
        message.contains("merged"),
        "the hint lists the forms: {message}"
    );
    let (ns_code, ns_message, _) =
        call_err(&client, "list_lumps", json!({ "namespace": "nope" })).await;
    assert_eq!(ns_code, "invalid_argument", "unknown namespace");
    assert!(
        ns_message.contains("flats"),
        "the hint lists the namespaces: {ns_message}"
    );
    let (kind_code, kind_message, _) =
        call_err(&client, "list_lumps", json!({ "kind": "nope" })).await;
    assert_eq!(kind_code, "invalid_argument", "unknown kind");
    assert!(
        kind_message.contains("map_marker"),
        "the hint lists the kinds: {kind_message}"
    );
}

#[tokio::test]
async fn list_lumps_filters_by_namespace_kind_and_map() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let flats = call(&client, "list_lumps", json!({ "namespace": "flats" })).await;
    assert_eq!(
        names(&flats),
        vec!["F_START", "FLOOR1", "PNGFLAT", "F_END"],
        "the flats region of the merged view",
    );
    assert_eq!(
        row_named(&flats, "FLOOR1")["source"],
        json!({ "layer": 1_u64 }),
        "the filtered rows keep their sources",
    );
    let markers = call(&client, "list_lumps", json!({ "kind": "map_marker" })).await;
    assert_eq!(names(&markers), vec!["MAP01", "MAP02"], "both map headers");
    let map = call(&client, "list_lumps", json!({ "map": "MAP01" })).await;
    assert_eq!(
        names(&map),
        vec![
            "MAP01", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
            "SECTORS", "REJECT", "BLOCKMAP",
        ],
        "the MAP01 block only",
    );
    assert!(
        map["lumps"]
            .as_array()
            .expect("array")
            .iter()
            .all(|row| row["map"] == json!("MAP01")),
        "every row names the filtered map",
    );
}

#[tokio::test]
async fn list_lumps_keeps_a_frame_sync_flat_in_the_flats_region() {
    let client = connect().await;
    let hex = format!("FFE5{}", "AA".repeat(4_094));
    call(
        &client,
        "add_lump",
        json!({ "name": "FEE5FLAT", "hex": hex, "namespace": "flats" }),
    )
    .await;
    call(
        &client,
        "add_lump",
        json!({ "name": "FEE5FREE", "hex": hex }),
    )
    .await;
    let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
    assert_eq!(
        row_named(&listing, "FEE5FLAT")["kind"],
        json!("flat"),
        "the region beats the frame-sync guess"
    );
    assert_eq!(
        row_named(&listing, "FEE5FLAT")["namespace"],
        json!("flats"),
        "and the region is still reported"
    );
    assert_eq!(
        row_named(&listing, "FEE5FREE")["kind"],
        json!("mp3"),
        "the same bytes outside a region still read as a frame sync"
    );
}

#[tokio::test]
async fn list_lumps_pages_with_offset_limit_and_total() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let all = call(&client, "list_lumps", json!({ "limit": 100_u64 })).await;
    assert_eq!(all["total"].as_u64(), Some(45), "everything");
    let page = call(
        &client,
        "list_lumps",
        json!({ "offset": 5_u64, "limit": 4_u64 }),
    )
    .await;
    assert_eq!(page["total"].as_u64(), Some(45), "total counts all matches");
    assert_eq!(page["offset"].as_u64(), Some(5), "echoed offset");
    assert_eq!(page["count"].as_u64(), Some(4), "one page");
    assert_eq!(
        names(&page),
        names(&all).into_iter().skip(5).take(4).collect::<Vec<_>>(),
        "the page is the same slice of the full listing",
    );
    assert_eq!(
        page["lumps"][0]["index"].as_u64(),
        Some(5),
        "index counts positions in the scope, not the page",
    );
    let past = call(
        &client,
        "list_lumps",
        json!({ "offset": 100_u64, "limit": 4_u64 }),
    )
    .await;
    assert_eq!(past["count"].as_u64(), Some(0), "nothing past the end");
    assert_eq!(
        past["total"].as_u64(),
        Some(45),
        "total still reports the matches"
    );
}

#[tokio::test]
async fn list_lumps_rejects_limits_outside_one_to_two_thousand() {
    let client = connect().await;
    let (zero_code, zero_message, _) =
        call_err(&client, "list_lumps", json!({ "limit": 0_u64 })).await;
    assert_eq!(zero_code, "invalid_argument", "limit 0 code");
    assert!(
        zero_message.contains("between 1 and 2000"),
        "the message states the range: {zero_message}"
    );
    let (big_code, big_message, _) =
        call_err(&client, "list_lumps", json!({ "limit": 2001_u64 })).await;
    assert_eq!(big_code, "invalid_argument", "limit 2001 code");
    assert!(
        big_message.contains("between 1 and 2000"),
        "the message states the range: {big_message}"
    );
}
