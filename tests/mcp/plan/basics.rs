//! The two-room plan and the block it writes: a `door` and an `opening`
//! compiling into one written block whose sector and geometry counts match the
//! reported bookkeeping - the door landing closed, tagged and wired with its
//! style's specials, its fronts facing their rooms and a `features` row
//! reporting the wiring - and the two opening widths: an `opening` spanning a
//! whole shared edge joining the rooms with no band sector of its own (two
//! sectors, one open line, no stub walls, nothing wearing `"-"` flats), while a
//! narrower one keeps its band and inherits the adjoining rooms' flats and
//! light. The door wiring: a blue-key plan's `features` row matching the
//! written block, its lines carrying special `26` and the band's tag with
//! fronts facing their rooms, validation staying error-free and one `undo`
//! taking it all back. The raw `SIDEDEFS` bytes: every texture field of the
//! written block stores a name, an untextured face carrying `"-"`
//! (`2D 00 00 00 00 00 00 00`) rather than the all-`NUL` field a vanilla
//! engine quits on, each field matching what the model reads back. And the
//! block's own lifecycle: the active map adoption,
//! `already_exists` for a working-archive block without `replace` and its
//! overwrite keeping one block, the one-step `undo` that takes the whole block
//! back, and the plan-path-named refusals - a flat rectangle, both or neither
//! footprint, unknown kind and style names - down to rooms that do not touch
//! refusing with the connection path, the duplicate connection id naming both
//! indices, and the window-locked room warned unreachable.

use serde_json::{Value, json};

use crate::fixtures::iwad_hexen;
use crate::plan::{
    Client, features_loaded, path_arg, plan_for, refuse_with, reported_sector_count, working_lines,
    working_sector,
};
use crate::support::{call, call_err, connect};

/// The structural checks a compiled plan must pass; thing checks
/// run separately because only the thing tests place a start.
const STRUCTURAL_CODES: [&str; 7] = [
    "crossing_lines",
    "overlapping_lines",
    "two_sided_flag_mismatch",
    "zero_length_line",
    "duplicate_line",
    "missing_texture",
    "missing_flat",
];

#[tokio::test]
async fn two_rooms_with_a_door_and_an_opening_report_their_bookkeeping() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(&client, "build_map_from_plan", plan_for("MAP05")).await;
    assert_eq!(result["map"], json!("MAP05"), "the block written");
    assert_eq!(result["format"], json!("doom"), "the default layout");
    assert_eq!(
        result["rooms"],
        json!({"a": [0_u64], "b": [1_u64]}),
        "rooms keep plan order"
    );
    assert_eq!(
        result["connections"],
        json!([
            {"id": "d1", "kind": "door", "sectors": [2_u64]},
            {"id": "a->b", "kind": "opening", "sectors": [3_u64]}
        ]),
        "the door keeps its id, the opening earns the derived label"
    );
    assert_eq!(
        result["bounds"],
        json!({"x1": 0_i64, "y1": 0_i64, "x2": 256_i64, "y2": 128_i64}),
        "the plan's outer corners"
    );
    assert_eq!(result["nodes"], json!(null), "null without finalize");
    assert_eq!(
        result["things_placed"],
        json!(1_u64),
        "just the player start in room a"
    );
    assert_eq!(result["validation"], json!(null), "null without finalize");
    assert_eq!(
        result["next_steps"],
        json!([
            "render_topdown to compare with the sketch",
            "render_view from the player start"
        ]),
        "what to look at next"
    );
}

#[tokio::test]
async fn the_written_block_matches_the_report_and_stays_clean() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(&client, "build_map_from_plan", plan_for("MAP05")).await;
    let summary = call(&client, "get_map_summary", json!({"map": "MAP05"})).await;
    assert_eq!(
        summary["name"],
        json!("MAP05"),
        "the built map became the active one"
    );
    assert_eq!(
        summary["counts"]["sectors"],
        json!(reported_sector_count(&result)),
        "two rooms and two bands own the whole map"
    );
    assert_eq!(
        summary["lines"]["one_sided"]
            .as_u64()
            .expect("a count")
            .saturating_add(summary["lines"]["two_sided"].as_u64().expect("a count")),
        summary["counts"]["linedefs"].as_u64().expect("a count"),
        "every linedef is exactly one-sided or two-sided"
    );
    assert_eq!(summary["bounds"]["min_x"], json!(0_i64), "the box");
    assert_eq!(summary["bounds"]["max_x"], json!(256_i64));
    assert_eq!(summary["bounds"]["min_y"], json!(0_i64));
    assert_eq!(summary["bounds"]["max_y"], json!(128_i64));
    let door = working_sector(&client, "MAP05", 2).await;
    assert_eq!(
        door["special"],
        json!(0_i32),
        "a Doom door acts from its lines: the sector keeps special 0"
    );
    assert_eq!(
        door["tag"],
        json!(1_i32),
        "the wired band answers the tag its lines carry"
    );
    assert_eq!(
        door["floor_height"], door["ceiling_height"],
        "the door sector is collapsed"
    );
    expect_door_feature_row(&result);
    let report = call(
        &client,
        "validate_map",
        json!({"map": "MAP05", "scope": "working", "codes": STRUCTURAL_CODES}),
    )
    .await;
    assert_eq!(
        report["counts"]["error"],
        json!(0_u64),
        "the compiled geometry is clean: {:?}",
        report["issues"]
    );
}

/// The working sector rows of the assembled graph.
async fn sector_rows(client: &Client, map: &str) -> Vec<Value> {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["sectors"]}),
    )
    .await;
    report["sectors"]
        .as_array()
        .expect("sectors is an array")
        .clone()
}

/// Two `256` wide rooms stacked on the line `y = 128`, each wearing
/// the fixture's own `FLOOR1` and `FLAT10`, joined by `connections`
/// and finalized so the build reports what validation sees.
fn touching_rooms_plan(map: &str, connections: &Value) -> Value {
    json!({
        "map": map,
        "rooms": [
            {"id": "north",
             "rect": {"x": 0_i64, "y": 128_i64, "w": 256_i64, "h": 128_i64},
             "floor_flat": "FLOOR1", "ceiling_flat": "FLAT10"},
            {"id": "south",
             "rect": {"x": 0_i64, "y": 0_i64, "w": 256_i64, "h": 128_i64},
             "floor_flat": "FLOOR1", "ceiling_flat": "FLAT10"}
        ],
        "connections": connections,
        "player_start": {"room": "south"},
        "finalize": true
    })
}

/// Asserts no sector row of `map` stands in bare `"-"` flats.
async fn assert_no_bare_flats(client: &Client, map: &str) {
    for row in sector_rows(client, map).await {
        let index = row["index"].as_u64().expect("an index");
        assert_ne!(
            row["floor_flat"],
            json!("-"),
            "sector {index} wears a floor"
        );
        assert_ne!(
            row["ceiling_flat"],
            json!("-"),
            "sector {index} wears a ceiling"
        );
    }
}

#[tokio::test]
async fn an_opening_across_the_whole_shared_edge_joins_the_two_rooms() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let plan = touching_rooms_plan(
        "MAP07",
        &json!([{"id": "wide", "from": "north", "to": "south",
                 "kind": "opening", "width": 256_i64}]),
    );
    let result = call(&client, "build_map_from_plan", plan).await;
    assert_eq!(
        result["rooms"],
        json!({"north": [0_u64], "south": [1_u64]}),
        "the two rooms are the whole map",
    );
    assert_eq!(
        result["connections"][0]["sectors"],
        json!([]),
        "a full-width opening keeps no band sector of its own",
    );
    let summary = call(&client, "get_map_summary", json!({"map": "MAP07"})).await;
    assert_eq!(
        summary["counts"]["sectors"],
        json!(2_u64),
        "no sliver sector stands between the rooms",
    );
    assert_eq!(
        summary["lines"]["two_sided"],
        json!(1_u64),
        "the rooms share their whole boundary as one open line",
    );
    assert_eq!(
        result["validation"]["errors"],
        json!([]),
        "the joined map validates clean: {:?}",
        result["validation"],
    );
    assert_no_bare_flats(&client, "MAP07").await;
}

#[tokio::test]
async fn a_narrower_opening_keeps_its_band_and_inherits_the_rooms_flats() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let plan = touching_rooms_plan(
        "MAP08",
        &json!([{"id": "narrow", "from": "north", "to": "south",
                 "kind": "opening", "width": 128_i64}]),
    );
    let result = call(&client, "build_map_from_plan", plan).await;
    assert_eq!(
        result["connections"][0]["sectors"],
        json!([2_u64]),
        "a band the standing wall still needs",
    );
    assert_eq!(
        result["validation"]["errors"],
        json!([]),
        "the banded map validates clean: {:?}",
        result["validation"],
    );
    let band = working_sector(&client, "MAP08", 2).await;
    assert_eq!(
        band["floor_flat"],
        json!("FLOOR1"),
        "the passage continues the room's floor",
    );
    assert_eq!(
        band["ceiling_flat"],
        json!("FLAT10"),
        "and the room's ceiling",
    );
    assert_eq!(band["light"], json!(160_i64), "and the dimmer room light");
    assert_no_bare_flats(&client, "MAP08").await;
}

/// The `MAP05` plan's one door reports a `features` row: the door
/// class, no room, band sector 2 and the fresh tag 1.
fn expect_door_feature_row(result: &Value) {
    let rows = result["features"].as_array().expect("a feature list");
    assert_eq!(rows.len(), 1, "the door is the only feature: {result}");
    let row = rows.first().expect("the door row");
    assert_eq!(row["kind"], json!("door"), "the wired connector class");
    assert_eq!(row["room"], json!(null), "a door belongs to the wall");
    assert_eq!(row["sectors"], json!([2_u64]), "the band sector");
    assert_eq!(row["tag"], json!(1_u64), "the fresh shared tag");
}

/// Assert every row of `lines` whose index is in `indices` is a
/// blue-key use line: special 26, tag 1, the band behind and a room
/// in front.
fn assert_locked_use_lines(lines: &[Value], indices: &[u64]) {
    let wired: Vec<&Value> = lines
        .iter()
        .filter(|line| indices.contains(&line["index"].as_u64().expect("an index")))
        .collect();
    assert_eq!(
        wired.len(),
        indices.len(),
        "every reported door line is in the block"
    );
    for line in wired {
        let index = line["index"].as_u64().expect("an index");
        assert_eq!(
            line["special"],
            json!(26_i64),
            "DR blue key on line {index}"
        );
        assert_eq!(line["tag"], json!(1_i64), "the shared tag on line {index}");
        assert_eq!(
            line["back"]["sector"],
            json!(2_i64),
            "the band sits behind line {index}"
        );
        let front = line["front"]["sector"].as_i64().expect("a front sector");
        assert!(
            front == 0_i64 || front == 1_i64,
            "the front faces its room on line {index}: {line}"
        );
    }
}

#[tokio::test]
async fn a_blue_locked_door_lands_wired_and_reports_its_feature_row() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["connections"][0]["style"] = json!("locked_blue");
    let result = call(&client, "build_map_from_plan", request).await;
    expect_door_feature_row(&result);
    let indices: Vec<u64> = result["features"][0]["lines"]
        .as_array()
        .expect("two door lines")
        .iter()
        .map(|slot| slot.as_u64().expect("a line index"))
        .collect();
    let lines = working_lines(&client, "MAP05").await;
    assert_locked_use_lines(&lines, &indices);
    let jambs: Vec<&Value> = lines
        .iter()
        .filter(|line| line["back"] == json!(null) && line["front"]["sector"] == json!(2_i64))
        .collect();
    assert_eq!(
        jambs.len(),
        2,
        "the band's north reveal edges, split where the room's line ran; its south \
         edge stands on the map border"
    );
    assert!(
        jambs
            .iter()
            .all(|line| line["front"]["middle"] == json!("DOORBLU")),
        "the locked jambs wear the blue track: {jambs:?}"
    );
    let band = working_sector(&client, "MAP05", 2).await;
    assert_eq!(band["tag"], json!(1_i32), "the sector answers the tag");
    let report = call(
        &client,
        "validate_map",
        json!({"map": "MAP05", "scope": "working", "codes": STRUCTURAL_CODES}),
    )
    .await;
    assert_eq!(
        report["counts"]["error"],
        json!(0_u64),
        "the wired map stays error-free: {:?}",
        report["issues"]
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["build_map_from_plan MAP05"]),
        "one labelled step takes the wiring too"
    );
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "the whole block went back");
}

#[tokio::test]
async fn a_working_block_needs_replace_and_overwrite_keeps_one_block() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(&client, "create_map", json!({"name": "MAP01"})).await;
    let (code, message, _) = call_err(&client, "build_map_from_plan", plan_for("MAP01")).await;
    assert_eq!(code, "already_exists", "the working block stands");
    assert!(
        message.contains("replace"),
        "the refusal names the way out: {message}"
    );
    let untouched = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP01", "scope": "working"}),
    )
    .await;
    assert_eq!(
        untouched["counts"]["sectors"],
        json!(0_u64),
        "the refusal wrote nothing"
    );
    let mut request = plan_for("MAP01");
    request["replace"] = json!(true);
    call(&client, "build_map_from_plan", request).await;
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    let rows = maps["maps"].as_array().expect("a map list");
    assert_eq!(rows.len(), 1, "one working block after the replace");
    assert_eq!(rows.first().expect("the row")["name"], json!("MAP01"));
    let summary = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP01", "scope": "working"}),
    )
    .await;
    assert_eq!(
        summary["counts"]["sectors"],
        json!(4_u64),
        "the planned map overwrote the empty one"
    );
}

#[tokio::test]
async fn undo_takes_the_whole_block_back() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(&client, "build_map_from_plan", plan_for("MAP05")).await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["build_map_from_plan MAP05"]),
        "one labelled step"
    );
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "the whole block went back");
}

#[tokio::test]
async fn plan_paths_name_every_refused_element() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut flat = plan_for("MAP05");
    flat["rooms"][0]["rect"]["w"] = json!(0_i64);
    refuse_with(&client, flat, "rooms[0]").await;
    let mut both = plan_for("MAP05");
    both["rooms"][1]["points"] = json!([[0_i64, 0_i64], [64_i64, 0_i64], [0_i64, 64_i64]]);
    refuse_with(&client, both, "rooms[1]").await;
    let mut neither = plan_for("MAP05");
    neither["rooms"][0].as_object_mut().expect("a room")["rect"] = json!(null);
    refuse_with(&client, neither, "rooms[0]").await;
    let mut portal = plan_for("MAP05");
    portal["connections"][0]["kind"] = json!("portal");
    refuse_with(&client, portal, "connections[0]").await;
    let mut slide = plan_for("MAP05");
    slide["connections"][0]["style"] = json!("slide");
    refuse_with(&client, slide, "normal, fast, stay_open").await;
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "no refusal wrote anything");
}

#[tokio::test]
async fn rooms_that_do_not_touch_refuse_with_the_connection_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let far = json!({
        "map": "MAP05",
        "rooms": [
            {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 64_i64, "h": 64_i64}},
            {"id": "b", "rect": {"x": 400_i64, "y": 0_i64, "w": 64_i64, "h": 64_i64}}
        ],
        "connections": [{"id": "d1", "from": "a", "to": "b", "kind": "opening"}]
    });
    let (code, message, _) = call_err(&client, "build_map_from_plan", far).await;
    assert_eq!(code, "invalid_argument", "rooms that miss");
    assert!(
        message.contains("connections[0]") && message.contains("do not touch"),
        "the path and the compiler's words: {message}"
    );
}

#[tokio::test]
async fn a_room_joined_only_by_a_window_warns_it_cannot_be_reached() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let glass_ward = json!({
        "map": "MAP09",
        "rooms": [
            {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
            {"id": "b", "rect": {"x": 128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
            {"id": "tower", "rect": {"x": 128_i64, "y": 128_i64, "w": 128_i64, "h": 128_i64}}
        ],
        "connections": [
            {"from": "a", "to": "b", "kind": "opening"},
            {"from": "b", "to": "tower", "kind": "window"}
        ],
        "player_start": {"room": "a"}
    });
    let result = call(&client, "build_map_from_plan", glass_ward).await;
    let warnings = result["warnings"].as_array().expect("a warning list");
    assert!(
        warnings.iter().any(|note| note
            .as_str()
            .is_some_and(|text| text.contains("room 'tower'")
                && text.contains("cannot be reached")
                && text.contains("window"))),
        "the window-locked tower reports as unreachable: {warnings:?}"
    );
}

#[tokio::test]
async fn duplicate_connection_ids_refuse_naming_the_path_and_both_indices() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut twin = plan_for("MAP05");
    twin["connections"][1]["id"] = json!("d1");
    let (code, message, _) = call_err(&client, "build_map_from_plan", twin).await;
    assert_eq!(code, "invalid_argument", "one id, two rows");
    assert!(
        message.starts_with("connections[0]")
            && message.contains("connections[1]")
            && message.contains("its own id"),
        "the plan path, both indices and the way out: {message}"
    );
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "the refusal wrote nothing");
}

/// The raw bytes of the working archive's `SIDEDEFS` lump, read back through
/// `get_lump`'s `hex` window.
async fn working_sidedef_bytes(client: &Client) -> Vec<u8> {
    let read = call(
        client,
        "get_lump",
        json!({
            "name": "SIDEDEFS",
            "scope": "working",
            "format": "hex",
            "start": 0_u64,
            "length": 4096_u64
        }),
    )
    .await;
    read["hex"]
        .as_str()
        .expect("a hex window")
        .split_whitespace()
        .map(|pair| u8::from_str_radix(pair, 16).expect("a whole hex pair"))
        .collect()
}

/// The upper, lower and middle texture fields of one 30-byte `SIDEDEFS`
/// record, in stored order, each with the `assemble_map` field that names
/// it.
fn texture_fields(record: &[u8]) -> [(&[u8], &str); 3] {
    assert!(record.len() >= 28, "one record holds three name fields");
    [
        (&record[4..12], "upper"),
        (&record[12..20], "lower"),
        (&record[20..28], "middle"),
    ]
}

/// Checks one written `SIDEDEFS` record against the model row read back
/// from it: no texture field is all-`NUL`, every `2D`-padded field reads
/// as the model's `"-"` and every other field reads as a dressed name.
/// Whether the record's middle is the untextured one.
fn assert_fields_match_model(record: &[u8], row: &Value) -> bool {
    let index = row["index"].as_u64().expect("a sidedef index");
    let blank = [0_u8; 8];
    let dash = *b"-\0\0\0\0\0\0\0";
    let mut untextured_middle = false;
    for (field, name) in texture_fields(record) {
        assert_ne!(
            field,
            &blank[..],
            "sidedef {index} {name}: an all-NUL field makes vanilla \
             R_CheckTextureNumForName search for the empty name and quit the load"
        );
        let model = row[name].as_str().expect("a texture name");
        if field == dash {
            assert_eq!(
                model, "-",
                "sidedef {index} {name}: a 2D-padded field reads as the model's \"-\""
            );
            untextured_middle = name == "middle";
        } else {
            assert_ne!(
                model, "-",
                "sidedef {index} {name} reads {model:?} from a dressed field"
            );
        }
    }
    untextured_middle
}

#[tokio::test]
async fn an_untextured_face_stores_dash_for_the_vanilla_texture_lookup() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(&client, "build_map_from_plan", plan_for("MAP05")).await;
    let data = working_sidedef_bytes(&client).await;
    let records: Vec<&[u8]> = data.chunks_exact(30).collect();
    assert!(
        data.chunks_exact(30).remainder().is_empty() && records.len() > 4,
        "the block holds several whole sidedef records, not {} bytes",
        data.len()
    );
    let graph = call(
        &client,
        "assemble_map",
        json!({"map": "MAP05", "scope": "working", "include": ["sidedefs"]}),
    )
    .await;
    let rows = graph["sidedefs"].as_array().expect("sidedefs is an array");
    assert_eq!(rows.len(), records.len(), "every record reads back");
    let untextured_middles = records
        .iter()
        .zip(rows)
        .filter(|&(record, row)| assert_fields_match_model(record, row))
        .count();
    assert!(
        untextured_middles > 0,
        "the plan leaves the two-sided lines' middles untextured, and each of those \
         {} faces stores 2D 00 00 00 00 00 00 00, not an empty field",
        records.len()
    );
}

#[tokio::test]
async fn an_outdoor_room_in_a_hexen_session_wears_f_sky() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad_hexen(dir.path());
    call(&client, "open_iwad", path_arg(&base)).await;
    let built = call(
        &client,
        "build_map_from_plan",
        json!({
            "map": "MAP05",
            "rooms": [
                {"id": "quad", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64},
                 "outdoor": true}
            ],
            "connections": [],
            "player_start": {"room": "quad"}
        }),
    )
    .await;
    assert_eq!(
        built["rooms"]["quad"],
        json!([0_u64]),
        "the courtyard owns sector 0: {built}"
    );
    let yard = working_sector(&client, "MAP05", 0).await;
    assert_eq!(
        yard["ceiling_flat"],
        json!("F_SKY"),
        "an outdoor room of a Hexen session sees the Hexen sky"
    );
}
