//! The `DeHackEd` topic: the patch pair's defaults and refusals, the
//! directory lookup the read builds on, the mode parsing, the line
//! operations, the `Latin-1` byte-preserving codecs and the one labelled
//! edit both commit modes make.

use serde_json::json;

use super::{args, carrying, code, loaded, name};
use crate::session::{Layer, LayerRole, Scope, Session};
use crate::tools::lumps::rows::build_rows;
use crate::tools::scripts::edit_dehacked::{
    Body, Draft, EditDehackedParams, Op, RemoveEntry, SetEntry, apply_ops, edit_run, plan,
};
use crate::tools::scripts::get_dehacked::{GetDehackedParams, get_run};
use crate::tools::scripts::{decode_patch, encode_patch, find_lump, lump_name};
use crate::wad::archive::{Lump, Wad, WadKind};
use std::path::PathBuf;

#[test]
fn the_default_name_is_dehacked_and_bad_names_are_refused() {
    assert_eq!(
        lump_name(None).expect("the default"),
        name("DEHACKED"),
        "the documented default"
    );
    assert_eq!(
        code(&lump_name(Some("TOO LONG")).expect_err("nine characters")),
        "invalid_argument",
        "bad name code"
    );
}

#[test]
fn scope_defaults_to_merged_and_refuses_other_names() {
    assert_eq!(
        Scope::parse_or_merged(None).expect("default"),
        Scope::Merged,
        "merged"
    );
    assert_eq!(
        Scope::parse_or_merged(Some("working")).expect("working"),
        Scope::Working,
        "working"
    );
    assert_eq!(
        code(&Scope::parse_or_merged(Some("nope")).expect_err("not a scope")),
        "invalid_argument",
        "bad scope code"
    );
}

#[test]
fn find_lump_picks_the_winning_copy_and_names_a_miss() {
    let session = loaded();
    let row = find_lump(&session, name("DEHACKED"), Scope::Merged).expect("the layer copy");
    assert_eq!(
        row.source,
        crate::session::Source::Layer(0),
        "from the base"
    );
    let error = find_lump(&session, name("DEHACKED"), Scope::Working)
        .expect_err("the working archive is empty");
    assert_eq!(code(&error), "not_found", "missing lump code");
    assert!(
        error.to_string().contains("edit_dehacked"),
        "the message names the tool that creates it: {error}"
    );
}

#[test]
fn rows_carry_the_classifications_the_report_builds_on() {
    let session = loaded();
    let rows = build_rows(&session, Scope::Merged).expect("merged rows");
    assert_eq!(rows.len(), 1, "one lump");
}

#[test]
fn plan_requires_exactly_one_mode() {
    assert_eq!(
        code(&plan(&args()).expect_err("no mode")),
        "invalid_argument",
        "none"
    );
    let both = EditDehackedParams {
        text: Some("x".to_owned()),
        source: Some("/x/y.deh".to_owned()),
        ..args()
    };
    assert_eq!(
        code(&plan(&both).expect_err("two modes")),
        "invalid_argument",
        "two"
    );
    let body = EditDehackedParams {
        text: Some("Patch File for DeHackEd v3.0\n".to_owned()),
        ..args()
    };
    assert!(
        matches!(plan(&body).expect("text mode"), Draft::Replace(text) if text.starts_with("Patch")),
        "the text mode is taken as-is"
    );
    let fetched = EditDehackedParams {
        source: Some("/x/y.deh".to_owned()),
        ..args()
    };
    assert!(
        matches!(plan(&fetched).expect("source mode"), Draft::Fetch(raw) if raw == "/x/y.deh"),
        "the source mode defers the fetch"
    );
}

#[test]
fn plan_validates_set_entries_up_front() {
    let empty = EditDehackedParams {
        set: Some(Vec::new()),
        ..args()
    };
    assert_eq!(
        code(&plan(&empty).expect_err("empty set")),
        "invalid_argument",
        "empty list"
    );
    let bad = EditDehackedParams {
        set: Some(vec![SetEntry {
            block: "Gong".to_owned(),
            key: "X".to_owned(),
            value: "1".to_owned(),
        }]),
        ..args()
    };
    assert_eq!(
        code(&plan(&bad).expect_err("bad block")),
        "invalid_argument",
        "bad block"
    );
    let multiline = EditDehackedParams {
        set: Some(vec![SetEntry {
            block: "Misc".to_owned(),
            key: "X".to_owned(),
            value: "1\n2".to_owned(),
        }]),
        ..args()
    };
    assert_eq!(
        code(&plan(&multiline).expect_err("newline value")),
        "invalid_argument",
        "newline value"
    );
}

#[test]
fn plan_validates_remove_entries_and_keeps_absent_keys() {
    let empty_remove = EditDehackedParams {
        remove: Some(Vec::new()),
        ..args()
    };
    assert_eq!(
        code(&plan(&empty_remove).expect_err("empty remove")),
        "invalid_argument",
        "empty remove list"
    );
    let empty_key = EditDehackedParams {
        remove: Some(vec![RemoveEntry {
            block: "Misc".to_owned(),
            key: Some(" ".to_owned()),
        }]),
        ..args()
    };
    assert_eq!(
        code(&plan(&empty_key).expect_err("blank key")),
        "invalid_argument",
        "blank key"
    );
    let ok = EditDehackedParams {
        remove: Some(vec![RemoveEntry {
            block: "[STRINGS]".to_owned(),
            key: None,
        }]),
        ..args()
    };
    match plan(&ok).expect("a whole-section removal") {
        Draft::Ops(ops) => assert!(
            matches!(ops.first(), Some(Op::Remove { key: None, .. })),
            "the key stays absent"
        ),
        Draft::Replace(_) | Draft::Fetch(_) => panic!("the remove mode plans operations"),
    }
}

#[test]
fn apply_ops_creates_a_header_bearing_patch_for_a_set_on_nothing() {
    let ops = vec![Op::Set {
        block: "Thing 12".to_owned(),
        key: "ID #".to_owned(),
        value: "20000".to_owned(),
    }];
    let text = apply_ops(None, &ops, name("DEHACKED")).expect("a create");
    assert!(
        text.starts_with("Patch File for DeHackEd v3.0\n"),
        "the canonical header: {text}"
    );
    assert!(
        text.contains("Thing 12\nID # = 20000\n"),
        "the block landed: {text}"
    );
}

#[test]
fn apply_ops_refuses_removals_from_an_absent_lump() {
    let ops = vec![Op::Remove {
        block: "Thing 12".to_owned(),
        key: None,
    }];
    let error = apply_ops(None, &ops, name("DEHACKED")).expect_err("nothing to remove");
    assert_eq!(code(&error), "not_found", "absent lump code");
    assert!(
        error.to_string().contains("none of the loaded"),
        "the message: {error}"
    );
}

#[test]
fn apply_ops_chains_and_aborts_at_the_first_missing_key() {
    let base = "Misc\nfoo = 1\n";
    let ops = vec![
        Op::Set {
            block: "Misc".to_owned(),
            key: "bar".to_owned(),
            value: "2".to_owned(),
        },
        Op::Remove {
            block: "Misc".to_owned(),
            key: Some("nope".to_owned()),
        },
    ];
    let error = apply_ops(Some(base), &ops, name("DEHACKED")).expect_err("key missing");
    assert_eq!(code(&error), "not_found", "missing key code");
}

#[test]
fn edit_run_appends_the_override_and_undo_takes_it_back() {
    let mut session = loaded();
    let body = Body::Replace("Thing 12\nID # = 20000\n".to_owned());
    let report = edit_run(&mut session, name("DEHACKED"), body).expect("an override");
    assert!(report.created, "the layer copy was copied-on-write");
    assert_eq!(report.lines, 2, "two lines");
    assert_eq!(report.changed, 1, "the header line rewritten");
    assert_eq!(report.added, 1, "the key line added");
    assert_eq!(report.removed, 0, "nothing dropped");
    assert_eq!(
        session.undo_labels(),
        vec!["edit_dehacked"],
        "the one labelled edit"
    );
    assert_eq!(
        session.working().len(),
        1,
        "the override sits in the working archive"
    );
    session.undo().expect("undo the edit");
    assert!(session.working().is_empty(), "the override is gone");
}

#[test]
fn edit_run_overwrites_an_existing_working_copy_in_place() {
    let mut session = loaded();
    session
        .edit("seed", |wad| {
            wad.lumps.push(carrying("DEHACKED", "Misc\nfoo = 1\n"));
            Ok(())
        })
        .expect("a plain append");
    let body = Body::Ops(vec![Op::Set {
        block: "Misc".to_owned(),
        key: "foo".to_owned(),
        value: "2".to_owned(),
    }]);
    let report = edit_run(&mut session, name("DEHACKED"), body).expect("an overwrite");
    assert!(!report.created, "the working copy was edited in place");
    assert_eq!(report.changed, 1, "one line rewritten");
    assert_eq!(report.added, 0, "nothing inserted");
    assert_eq!(report.removed, 0, "nothing dropped");
    assert_eq!(report.lines, 2, "still two lines");
    let text = String::from_utf8(
        session
            .working()
            .lumps
            .last()
            .expect("the lump")
            .data
            .to_vec(),
    )
    .expect("text");
    assert!(text.contains("foo = 2\n"), "the new value: {text}");
}

#[test]
fn an_absent_lump_still_takes_a_full_replace() {
    let mut session = Session::default();
    let report = edit_run(
        &mut session,
        name("MYPATCH"),
        Body::Replace("Misc\na = 1\n".to_owned()),
    )
    .expect("a create");
    assert!(report.created, "the lump is new");
    assert_eq!(report.added, 2, "all lines are new");
    assert_eq!(report.changed, 0, "nothing to rewrite against");
    assert_eq!(session.working().len(), 1, "one lump was appended");
}

#[test]
fn edit_run_reports_a_one_line_insert_as_one_added() {
    let mut session = loaded();
    let body = Body::Replace("z\nThing 1 (Troop)\n".to_owned());
    let report = edit_run(&mut session, name("DEHACKED"), body).expect("a replace");
    assert_eq!(
        (report.added, report.removed, report.changed),
        (1_usize, 0_usize, 0_usize),
        "inserting at line 1 counts the one new line, not the file"
    );
}

/// A session whose `IWAD` layer holds a `DEHACKED` global as raw bytes.
fn holding(raw: Vec<u8>) -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("legacy.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![Lump::new(name("DEHACKED"), raw)],
            },
        ))
        .expect("no batch is open");
    session
}

#[test]
fn a_latin1_set_round_trips_untouched_bytes_exactly() {
    let mut session = holding(b"Misc\nname = Caf\xE9\n".to_vec());
    let body = Body::Ops(vec![Op::Set {
        block: "Misc".to_owned(),
        key: "speed".to_owned(),
        value: "2".to_owned(),
    }]);
    let report = edit_run(&mut session, name("DEHACKED"), body).expect("a set");
    assert_eq!(report.added, 1, "the inserted key line");
    assert_eq!(report.changed, 0, "no line rewritten");
    let data = session
        .working()
        .lumps
        .last()
        .expect("the override")
        .data
        .to_vec();
    assert_eq!(
        data,
        b"Misc\nname = Caf\xE9\nspeed = 2\n".to_vec(),
        "the 0xE9 byte survived the edit untouched"
    );
}

#[test]
fn a_latin1_value_written_by_set_becomes_its_single_byte() {
    let mut session = holding(b"Misc\nfoo = 1\n".to_vec());
    let body = Body::Ops(vec![Op::Set {
        block: "Misc".to_owned(),
        key: "name".to_owned(),
        value: "Caf\u{E9}".to_owned(),
    }]);
    edit_run(&mut session, name("DEHACKED"), body).expect("a set");
    let data = session
        .working()
        .lumps
        .last()
        .expect("the override")
        .data
        .to_vec();
    assert_eq!(
        data,
        b"Misc\nfoo = 1\nname = Caf\xE9\n".to_vec(),
        "one byte"
    );
}

#[test]
fn get_run_reads_latin1_text_without_replacing_bytes() {
    let session = holding(b"Misc\nname = Caf\xE9\n".to_vec());
    let args: GetDehackedParams =
        serde_json::from_value(json!({ "include_text": true })).expect("the defaults parse");
    let report = serde_json::to_value(get_run(&session, &args).expect("the lump reads"))
        .expect("a report serializes");
    assert_eq!(
        report["text"],
        json!("Misc\nname = Caf\u{E9}\n"),
        "the Latin-1 view, no U+FFFD anywhere"
    );
    assert_eq!(
        report["blocks"][0]["fields"]["name"],
        json!("Caf\u{E9}"),
        "the parsed value round trips"
    );
}

#[test]
fn every_byte_value_round_trips_through_the_patch_codecs() {
    let bytes = (0_u8..=u8::MAX).collect::<Vec<u8>>();
    let text = decode_patch(&bytes);
    assert_eq!(text.chars().count(), 256, "one character per byte");
    assert_eq!(encode_patch(&text), bytes, "the bytes come back exactly");
}

#[test]
fn encode_patch_keeps_wide_characters_as_their_utf8_bytes() {
    let data = encode_patch("caf\u{E9} \u{2020}");
    assert_eq!(
        data,
        vec![
            0x63_u8, 0x61_u8, 0x66_u8, 0xE9_u8, 0x20_u8, 0xE2_u8, 0x80_u8, 0xA0_u8
        ],
        "Latin-1 as its one byte, wider characters as UTF-8, never U+FFFD"
    );
}
