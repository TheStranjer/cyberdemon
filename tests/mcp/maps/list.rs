//! The `list_maps` scopes and the `get_map_lumps` raw block view: both
//! fixtures listed with formats, sources, namespaces, node presence and
//! members in directory order, the `layer:N` and `working` scopes, the
//! `UDMF` and `Hexen` block shapes, the raw lump rows with sizes and kinds,
//! the `urls` payload publication read back over `HTTP` and the map,
//! active-map and scope refusals.
//!
//! Every helper here comes from [`crate::maps`].

use cyberdemon::wad::LumpName;
use cyberdemon::wad::map::doom::write_doom_map;
use serde_json::{Value, json};

use crate::fixtures::{iwad, two_room_map};
use crate::maps::{
    DOOM_MEMBERS, fetch_asset, formats_loaded, iwad_plus_pwad, lump_named, lump_names, map_named,
};
use crate::support::{call, call_err, connect};

#[tokio::test]
async fn list_maps_merged_lists_both_fixtures_with_formats_and_sources() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let report = call(&client, "list_maps", json!({})).await;
    let maps = report["maps"].as_array().expect("maps is an array");
    assert_eq!(maps.len(), 2, "one block per fixture archive");
    let first = map_named(&report, "MAP01");
    assert_eq!(first["format"], json!("doom"), "the binary block");
    assert_eq!(first["udmf_namespace"], Value::Null, "no UDMF here");
    assert_eq!(
        first["source"],
        json!({ "layer": 0_u64 }),
        "the IWAD's copy",
    );
    assert_eq!(
        first["lumps"],
        json!(DOOM_MEMBERS),
        "members in directory order, header excluded",
    );
    assert_eq!(
        first["has_nodes"],
        json!(false),
        "the fixture NODES are empty"
    );
    assert_eq!(first["has_gl_nodes"], json!(false), "no GL run");
    let second = map_named(&report, "MAP02");
    assert_eq!(second["format"], json!("doom"), "the PWAD block too");
    assert_eq!(
        second["source"],
        json!({ "layer": 1_u64 }),
        "the PWAD's copy"
    );
}

#[tokio::test]
async fn list_maps_layer_scopes_read_one_archive_and_working_starts_empty() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let base = call(&client, "list_maps", json!({ "scope": "layer:0" })).await;
    assert_eq!(
        base["maps"].as_array().expect("array").len(),
        1,
        "MAP01 only"
    );
    assert_eq!(map_named(&base, "MAP01")["name"], json!("MAP01"));
    let extra = call(&client, "list_maps", json!({ "scope": "layer:1" })).await;
    assert_eq!(
        extra["maps"].as_array().expect("array").len(),
        1,
        "MAP02 only"
    );
    assert_eq!(map_named(&extra, "MAP02")["name"], json!("MAP02"));
    call(&client, "new_wad", json!({})).await;
    let working = call(&client, "list_maps", json!({ "scope": "working" })).await;
    assert_eq!(
        working["maps"],
        json!([]),
        "the fresh working archive holds no map",
    );
}

#[tokio::test]
async fn list_maps_reports_scope_errors_like_the_lumps_tools() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let (code, message, _) = call_err(&client, "list_maps", json!({ "scope": "nope" })).await;
    assert_eq!(code, "invalid_argument", "unknown scope");
    assert!(
        message.contains("merged") && message.contains("layer:"),
        "the error lists the forms: {message}"
    );
    let (layer_code, _, _) = call_err(&client, "list_maps", json!({ "scope": "layer:5" })).await;
    assert_eq!(layer_code, "not_found", "layer 5 is not loaded");
}

#[tokio::test]
async fn list_maps_and_get_map_lumps_report_the_udmf_and_hexen_formats() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = formats_loaded(dir.path()).await;
    let report = call(&client, "list_maps", json!({ "scope": "layer:0" })).await;
    let udmf = map_named(&report, "MAP03");
    assert_eq!(udmf["format"], json!("udmf"), "the TEXTMAP block");
    assert_eq!(
        udmf["udmf_namespace"],
        json!("zdoom"),
        "the namespace the TEXTMAP declares",
    );
    assert_eq!(udmf["has_nodes"], json!(true), "the ZNODES lump counts");
    assert_eq!(udmf["has_gl_nodes"], json!(true), "the GL_MAP03 run");
    assert_eq!(
        udmf["lumps"],
        json!([
            "TEXTMAP", "ZNODES", "ENDMAP", "GL_MAP03", "GL_VERT", "GL_NODES",
        ]),
        "the block members and the GL run",
    );
    let hexen = map_named(&report, "MAP04");
    assert_eq!(hexen["format"], json!("hexen"), "the BEHAVIOR marks it");
    assert_eq!(
        hexen["udmf_namespace"],
        Value::Null,
        "binary has no namespace"
    );
    assert_eq!(hexen["has_nodes"], json!(true), "the NODES carry bytes");
    assert_eq!(hexen["has_gl_nodes"], json!(false));
    let raw = call(&client, "get_map_lumps", json!({ "map": "MAP04" })).await;
    assert_eq!(
        raw["format"],
        json!("hexen"),
        "the raw set carries the format"
    );
    assert!(
        lump_names(&raw).contains(&"BEHAVIOR".to_owned()),
        "the generated BEHAVIOR is a member: {:?}",
        lump_names(&raw)
    );
}

#[tokio::test]
async fn get_map_lumps_lists_the_raw_lump_set_with_sizes_and_kinds() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let report = call(&client, "get_map_lumps", json!({ "map": "MAP02" })).await;
    assert_eq!(report["map"], json!("MAP02"), "the map echoes");
    assert_eq!(report["format"], json!("doom"), "and its format");
    assert_eq!(lump_names(&report), DOOM_MEMBERS, "the raw set in order");
    let things = lump_named(&report, "THINGS");
    assert_eq!(things["size"], json!(20_u64), "two 10-byte things");
    assert_eq!(
        things["kind"],
        json!("map_data"),
        "classified like list_lumps"
    );
    assert!(things.get("asset").is_none(), "no asset key without urls");
    assert_eq!(
        lump_named(&report, "NODES")["size"],
        json!(0_u64),
        "empty nodes"
    );
    let (code, message, _) = call_err(
        &client,
        "get_map_lumps",
        json!({ "map": "MAP02", "scope": "layer:0" }),
    )
    .await;
    assert_eq!(code, "not_found", "MAP02 is not in layer 0");
    assert!(
        message.contains("layer:0"),
        "the message names the scope: {message}"
    );
}

#[tokio::test]
async fn get_map_lumps_urls_publish_the_raw_bytes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad(dir.path());
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    let report = call(
        &client,
        "get_map_lumps",
        json!({ "map": "MAP01", "urls": true }),
    )
    .await;
    let things = lump_named(&report, "THINGS");
    assert_eq!(things["asset"]["filename"], json!("MAP01_THINGS.lmp"));
    assert_eq!(
        things["asset"]["mime"],
        json!("application/octet-stream"),
        "raw bytes",
    );
    assert_eq!(things["asset"]["size"], json!(20_u64), "the payload size");
    let url = things["asset"]["url"].as_str().expect("an asset url");
    assert!(
        url.starts_with("http://localhost:"),
        "a local origin: {url}"
    );
    let written = write_doom_map(&two_room_map()).expect("the fixture map serializes");
    let parsed = LumpName::parse("THINGS").expect("a valid name");
    let expected = written
        .iter()
        .find(|lump| lump.name == parsed)
        .expect("the THINGS lump")
        .data
        .to_vec();
    let fetched = fetch_asset(url).await;
    assert_eq!(fetched, expected, "the served bytes are the lump's own");
    assert!(
        lump_named(&report, "SEGS").get("asset").is_none(),
        "zero-size lumps are not published",
    );
}

#[tokio::test]
async fn get_map_lumps_resolves_map_active_map_and_the_only_map() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let (code, message, _) = call_err(&client, "get_map_lumps", json!({})).await;
    assert_eq!(code, "invalid_argument", "two maps, no default");
    assert!(
        message.contains("pass map or call set_active_map"),
        "the error names the fixes: {message}"
    );
    call(&client, "set_active_map", json!({ "map": "MAP02" })).await;
    let report = call(&client, "get_map_lumps", json!({})).await;
    assert_eq!(
        report["map"],
        json!("MAP02"),
        "the active map is the default"
    );
    let solo = connect().await;
    let base = iwad(dir.path());
    call(
        &solo,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    let only = call(&solo, "get_map_lumps", json!({})).await;
    assert_eq!(
        only["map"],
        json!("MAP01"),
        "a single map needs no selection"
    );
    let empty = connect().await;
    let (empty_code, _, _) = call_err(&empty, "get_map_lumps", json!({})).await;
    assert_eq!(
        empty_code, "invalid_argument",
        "no archives, no map to read"
    );
}

#[tokio::test]
async fn get_map_lumps_rejects_unknown_maps_and_bad_names() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let (unknown_code, message, hint) =
        call_err(&client, "get_map_lumps", json!({ "map": "MAP99" })).await;
    assert_eq!(unknown_code, "not_found", "no such map");
    assert!(
        message.contains("MAP99"),
        "the message names the map: {message}"
    );
    assert!(
        hint.contains("list_maps") && hint.contains("create_map"),
        "the hint names the way out: {hint}"
    );
    let (name_code, _, _) = call_err(&client, "get_map_lumps", json!({ "map": "BAD NAME" })).await;
    assert_eq!(
        name_code, "invalid_argument",
        "spaces are not allowed in names"
    );
    let (scope_code, _, _) = call_err(&client, "get_map_lumps", json!({ "scope": "nope" })).await;
    assert_eq!(scope_code, "invalid_argument", "unknown scope");
}
