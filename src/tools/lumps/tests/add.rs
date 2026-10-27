//! The editing calls: the plans' up-front validation, the name an import
//! derives after its fetch, the clash message and the single undoable
//! insert with its duplicate rule.

use crate::session::Session;
use crate::tools::lumps::add::{
    AddLumpParams, Prepared, apply, chosen_name, clash_message, plan_add, plan_import,
};
use crate::tools::lumps::payload::Payload;
use crate::tools::lumps::place::{Placement, PlacementParams};
use crate::wad::classify::Namespace;

use super::{add_args, code, import_args, name, prepared};

#[test]
fn plan_add_validates_name_payload_and_placement_up_front() {
    let plan = plan_add(&add_args("note")).expect("valid");
    assert_eq!(plan.name, Some(name("NOTE")), "the name uppercases");
    assert_eq!(plan.verb, "add_lump", "the undo verb");
    assert!(
        matches!(plan.payload, Payload::Empty),
        "no data field is an empty marker"
    );
    assert!(plan.source.is_none(), "add_lump echoes no source");
    let long = plan_add(&add_args("TOO LONG FOR WAD")).expect_err("over eight bytes");
    assert_eq!(code(&long), "invalid_argument", "bad name code");
    let twin_fields = AddLumpParams {
        text: Some("t".to_owned()),
        hex: Some("4d".to_owned()),
        ..add_args("NOTE")
    };
    let payload_error = plan_add(&twin_fields).expect_err("two payload fields");
    assert_eq!(
        code(&payload_error),
        "invalid_argument",
        "payload conflict code"
    );
    assert!(
        payload_error.to_string().contains("at most one"),
        "the shared picker states the rule: {payload_error}"
    );
    let conflicted = AddLumpParams {
        placement: PlacementParams {
            index: Some(1),
            after: Some("X".to_owned()),
            ..PlacementParams::default()
        },
        ..add_args("NOTE")
    };
    let error = plan_add(&conflicted).expect_err("two placement fields");
    assert_eq!(code(&error), "invalid_argument", "placement conflict code");
}

#[test]
fn plan_import_defers_name_and_fetch_to_prepare() {
    let plan = plan_import(&import_args("/x/y.png", None)).expect("valid");
    assert_eq!(plan.name, None, "the name is derived after the fetch");
    assert_eq!(plan.verb, "import", "the undo verb");
    assert_eq!(
        plan.source.as_deref(),
        Some("/x/y.png"),
        "the source echoes"
    );
    let explicit = plan_import(&import_args("/x/y.png", Some("PIC"))).expect("named");
    assert_eq!(explicit.name, Some(name("PIC")), "an explicit name parses");
    let bad = plan_import(&import_args("/x/y.png", Some("too long name here")))
        .expect_err("over eight bytes");
    assert_eq!(code(&bad), "invalid_argument", "bad name code");
}

#[test]
fn an_import_name_falls_back_to_the_derived_file_stem() {
    assert_eq!(
        chosen_name(Some(name("GIVEN")), Some("x.png")).expect("explicit wins"),
        name("GIVEN"),
        "the caller's name is kept"
    );
    assert_eq!(
        chosen_name(None, Some("D_RUNNIN.mid")).expect("derived"),
        name("D_RUNNIN"),
        "the stem uppercases"
    );
    let error = chosen_name(None, Some(".png")).expect_err("nothing to derive");
    assert_eq!(code(&error), "invalid_argument", "underivable code");
    assert!(
        error.to_string().contains("pass name"),
        "the message asks for name: {error}"
    );
    let nofile = chosen_name(None, None).expect_err("no filename at all");
    assert_eq!(code(&nofile), "invalid_argument", "absent filename code");
}

#[test]
fn clash_messages_name_the_region_and_both_exits() {
    let global = clash_message(name("TWIN"), None);
    assert!(
        global.contains("global area")
            && global.contains("allow_duplicate")
            && global.contains("replace_lump"),
        "the global text names both exits: {global}"
    );
    let flats = clash_message(name("TWIN"), Some(Namespace::Flats));
    assert!(
        flats.contains("the flats region"),
        "the region is named: {flats}"
    );
}

#[test]
fn apply_inserts_once_labels_the_history_and_refuses_twins() {
    let mut session = Session::default();
    let edit = prepared("NOTE", b"hello");
    let report = apply(&mut session, &edit).expect("the first add lands");
    assert_eq!((report.index, report.size), (0, 5), "appended at 0");
    assert_eq!(report.namespace, None, "a loose lump sits in no region");
    assert_eq!(
        session.undo_labels(),
        vec!["add_lump NOTE"],
        "the undo label"
    );
    assert!(session.is_dirty(), "an unsaved edit");
    let error = apply(&mut session, &edit).expect_err("the twin is refused");
    assert_eq!(code(&error), "already_exists", "duplicate code");
    assert_eq!(session.working().len(), 1, "the refused edit rolled back");
    assert_eq!(session.undo_labels().len(), 1, "and left no history");
    let allowed = Prepared {
        allow_duplicate: true,
        ..prepared("NOTE", b"again")
    };
    let twice = apply(&mut session, &allowed).expect("the twin is allowed");
    assert_eq!(twice.index, 1, "appended beside it");
    assert_eq!(session.working().len(), 2, "both copies stand");
}

#[test]
fn apply_reports_the_region_a_namespace_insert_joins() {
    let mut session = Session::default();
    let edit = Prepared {
        placement: Placement {
            namespace: Some(Namespace::Flats),
            ..Placement::default()
        },
        ..prepared("FLATA", b"\0".as_slice())
    };
    let report = apply(&mut session, &edit).expect("the region is created");
    assert_eq!(
        (report.index, report.namespace),
        (1, Some(Namespace::Flats)),
        "the lump sits between the new markers"
    );
    assert_eq!(session.working().len(), 3, "start, lump, end");
    let again = prepared("FLATA", b"\0".as_slice());
    let loose = apply(&mut session, &again).expect("a global twin is no clash");
    assert_eq!(loose.namespace, None, "the loose copy is global");
    assert_eq!(loose.index, 3, "appended past the region");
}
