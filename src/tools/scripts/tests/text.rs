//! The script-tool topic: the two tools' per-tool defaults and name
//! folding, the create-then-edit-in-place copy-on-write run, the line
//! operations' refusal against an absent lump, the brace warning, the
//! non-`UTF-8` refusal that leaves no history and the result helpers.

use serde_json::json;

use super::{code, name, with_decorate};
use crate::session::Session;
use crate::state::AppState;
use crate::tools::scripts::text_edit::TextOp;
use crate::tools::scripts::text_tools::{
    EDIT_DECORATE, EDIT_ZSCRIPT, run_text_tool, text_edit_run, text_lump_name,
};
use crate::wad::archive::Lump;

#[test]
fn text_lump_names_default_per_tool_and_refuse_junk() {
    assert_eq!(
        text_lump_name(&EDIT_DECORATE, None).expect("the default"),
        name("DECORATE"),
        "the decorate default"
    );
    assert_eq!(
        text_lump_name(&EDIT_ZSCRIPT, None).expect("the default"),
        name("ZSCRIPT"),
        "the zscript default"
    );
    assert_eq!(
        text_lump_name(&EDIT_ZSCRIPT, Some("zscript2")).expect("an include file"),
        name("ZSCRIPT2"),
        "case folds"
    );
    assert_eq!(
        code(&text_lump_name(&EDIT_DECORATE, Some("TOO LONG")).expect_err("nine characters")),
        "invalid_argument",
        "bad name code"
    );
}

#[test]
fn text_edit_run_creates_a_missing_lump_by_appending() {
    let mut session = Session::default();
    let op = TextOp::Append {
        text: "actor Test 20001 { }\n".to_owned(),
    };
    let report =
        text_edit_run(&mut session, &EDIT_DECORATE, name("DECORATE"), &op).expect("a create");
    assert!(report.created, "the lump is new");
    assert_eq!(report.lines, 1, "one line");
    assert_eq!(report.changed_range, (1, 1), "the added line");
    assert!(
        report.warnings.is_empty(),
        "balanced: {:?}",
        report.warnings
    );
    assert_eq!(
        session.undo_labels(),
        vec!["edit_decorate DECORATE"],
        "the label carries the lump"
    );
    session.undo().expect("undo the create");
    assert!(session.working().is_empty(), "the lump went back");
}

#[test]
fn text_edit_run_refuses_line_ops_on_an_absent_lump() {
    let mut session = Session::default();
    let op = TextOp::DeleteLines { start: 1, end: 1 };
    let error = text_edit_run(&mut session, &EDIT_ZSCRIPT, name("ZSCRIPT"), &op)
        .expect_err("nothing to delete");
    assert_eq!(code(&error), "not_found", "absent lump code");
    assert!(
        error.to_string().contains("replace_all"),
        "the message names the creating ops: {error}"
    );
    assert!(session.undo_labels().is_empty(), "no history was made");
}

#[test]
fn text_edit_run_copies_on_write_then_edits_in_place() {
    let mut session = with_decorate("actor A {\n}\n");
    let op = TextOp::InsertLines {
        line: 2,
        text: "  radius 30".to_owned(),
    };
    let first =
        text_edit_run(&mut session, &EDIT_DECORATE, name("DECORATE"), &op).expect("a write");
    assert!(first.created, "the layer copy was not editable");
    assert_eq!(first.lines, 3, "the insert added a line");
    assert_eq!(first.changed_range, (2, 2), "its line");
    assert!(first.warnings.is_empty(), "still balanced");
    let second = text_edit_run(
        &mut session,
        &EDIT_DECORATE,
        name("DECORATE"),
        &TextOp::ReplaceAll {
            text: "actor A {\n}\n".to_owned(),
        },
    )
    .expect("an in-place edit");
    assert!(!second.created, "the working copy won the merged view");
    assert_eq!(session.working().len(), 1, "one override only");
    assert_eq!(
        session.undo_labels(),
        vec!["edit_decorate DECORATE", "edit_decorate DECORATE"],
        "two labelled steps"
    );
}

#[test]
fn text_edit_run_reports_unbalanced_braces_after_a_replace() {
    let mut session = with_decorate("actor A { }\n");
    let report = text_edit_run(
        &mut session,
        &EDIT_DECORATE,
        name("DECORATE"),
        &TextOp::ReplaceAll {
            text: "actor A {\n".to_owned(),
        },
    )
    .expect("a write");
    assert_eq!(report.warnings.len(), 1, "one note: {:?}", report.warnings);
    assert!(
        report
            .warnings
            .first()
            .is_some_and(|w| w.contains("unbalanced")),
        "the brace note: {:?}",
        report.warnings
    );
}

#[test]
fn a_non_utf8_lump_is_a_format_error_and_changes_nothing() {
    let mut session = Session::default();
    session
        .edit("seed", |wad| {
            wad.lumps
                .push(Lump::new(name("DECORATE"), vec![0x61_u8, 0xFF_u8]));
            Ok(())
        })
        .expect("a plain append");
    let error = text_edit_run(
        &mut session,
        &EDIT_DECORATE,
        name("DECORATE"),
        &TextOp::Append {
            text: "b".to_owned(),
        },
    )
    .expect_err("not UTF-8");
    assert_eq!(code(&error), "format_error", "non-UTF-8 code");
    assert!(
        error.to_string().contains("get_lump format=hex"),
        "the message points at the byte reader: {error}"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["seed"],
        "the edit left no trace"
    );
}

#[test]
fn run_text_tool_answers_through_the_result_helpers() {
    let state = AppState::default();
    let good = run_text_tool(
        &state,
        &EDIT_DECORATE,
        None,
        &TextOp::ReplaceAll {
            text: "actor A { }\n".to_owned(),
        },
    )
    .expect("a protocol-level success");
    assert_ne!(good.is_error, Some(true), "the edit ran");
    let content = good.structured_content.expect("structured content");
    assert_eq!(content["lump"], json!("DECORATE"), "the lump field");
    assert_eq!(content["created"], json!(true), "it created the lump");
    let bad = run_text_tool(
        &state,
        &EDIT_DECORATE,
        Some("TOO LONG"),
        &TextOp::Append {
            text: "x".to_owned(),
        },
    )
    .expect("a protocol-level answer");
    assert_eq!(bad.is_error, Some(true), "a bad name is a domain error");
    assert_eq!(
        bad.structured_content.expect("error content")["error"],
        json!("invalid_argument"),
        "the code field"
    );
}
