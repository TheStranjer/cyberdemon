//! The three `replace_*` swaps through the shared fixture: the faces and
//! surfaces the wire lists narrow to, the `"-"` clearing and the stock-
//! checked refusals, the one labeled undo step and its preview, and
//! `replace_thing_type`'s resolved numbers, named parameters and
//! `allow_unknown` escape.

use serde_json::json;

use crate::tools::query::replace_tools::{flat_run, texture_run, thing_type_run};

use super::{bulk_session, flat_params, name, number, texture_params, thing_params};

#[test]
fn texture_run_swaps_faces_as_one_labeled_step() {
    let mut session = bulk_session();
    let mut args = texture_params("wall1", "step1");
    args.dry_run = Some(true);
    let preview = texture_run(&mut session, &args).expect("the preview runs");
    assert_eq!(preview.total, 3_usize, "two middles and one upper");
    assert!(
        session.undo_labels().is_empty(),
        "a preview records nothing"
    );
    args.dry_run = None;
    let report = texture_run(&mut session, &args).expect("the swap runs");
    let shown = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(
        shown["maps"],
        json!({"MAP01": 3_u64}),
        "the three faces folded onto the block name"
    );
    assert_eq!(shown["total"], json!(3_u64), "the sum");
    assert_eq!(
        shown["copied_maps"],
        json!(["MAP01"]),
        "the map came from the IWAD layer"
    );
    assert_eq!(shown["dry_run"], json!(false), "the swap wrote");
    assert!(
        !report.next_steps.is_empty(),
        "the report names a next step"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["replace_texture WALL1 -> STEP1"],
        "one step, uppercase label from the folded names"
    );
}

#[test]
fn texture_run_narrows_to_the_listed_faces_and_clears_with_the_dash() {
    let mut middles = texture_params("WALL1", "STEP1");
    middles.parts = Some(vec!["middle".to_owned()]);
    let mut session = bulk_session();
    let report = texture_run(&mut session, &middles).expect("one face listed");
    assert_eq!(report.total, 2_usize, "only the two middles");
    let mut clear = texture_params("WALL1", "-");
    clear.parts = Some(vec!["UPPER".to_owned()]);
    let mut blank = bulk_session();
    let cleared = texture_run(&mut blank, &clear).expect("\"-\" clears without a stock check");
    assert_eq!(cleared.total, 1_usize, "only the upper, emptied");
}

#[test]
fn texture_run_refuses_stray_faces_and_unknown_targets() {
    let mut session = bulk_session();
    let mut stray = texture_params("WALL1", "STEP1");
    stray.parts = Some(vec!["roof".to_owned()]);
    let bad_face = texture_run(&mut session, &stray).expect_err("roof is no face");
    assert_eq!(bad_face.code(), "invalid_argument", "stray face code");
    let empty = texture_run(&mut session, &texture_params("WALL1", ""))
        .expect_err("an empty target is refused");
    assert_eq!(empty.code(), "invalid_argument", "empty target code");
    let missing =
        texture_run(&mut session, &texture_params("WALL1", "NOSUCH")).expect_err("no such wall");
    assert_eq!(missing.code(), "not_found", "missing target code");
    assert!(
        missing.to_string().contains("list_textures"),
        "the fix is named: {missing}"
    );
    assert!(
        session.undo_labels().is_empty(),
        "refusals write nothing and record nothing"
    );
}

#[test]
fn flat_run_swaps_surfaces_and_refuses_the_documented_strays() {
    let mut session = bulk_session();
    let mut floors = flat_params("floor1", "nukage1");
    floors.surfaces = Some(vec!["floor".to_owned()]);
    let report = flat_run(&mut session, &floors).expect("the swap runs");
    assert_eq!(report.total, 2_usize, "the two floors, ceilings untouched");
    assert_eq!(
        session.undo_labels(),
        vec!["replace_flat FLOOR1 -> NUKAGE1"],
        "one labeled step"
    );
    let mut blank = bulk_session();
    let all = flat_run(&mut blank, &flat_params("FLOOR1", "NUKAGE1")).expect("both surfaces");
    assert_eq!(all.total, 3_usize, "F_SKY1 is not FLOOR1 and stays");
    let stray = flat_run(&mut blank, &flat_params("FLOOR1", "NUKAGE1"))
        .expect("the swap runs again, changing nothing");
    assert_eq!(stray.total, 0_usize, "already swapped");
    let mut bad_surface = flat_params("FLOOR1", "NUKAGE1");
    bad_surface.surfaces = Some(vec!["roof".to_owned()]);
    let error = flat_run(&mut blank, &bad_surface).expect_err("roof is no surface");
    assert_eq!(error.code(), "invalid_argument", "stray surface code");
    assert!(
        error.to_string().contains("ceiling"),
        "the message names the two: {error}"
    );
    let dash = flat_run(&mut blank, &flat_params("FLOOR1", "-")).expect_err("no dash flat");
    assert_eq!(
        dash.code(),
        "not_found",
        "a flat \"-\" must exist and none does"
    );
    let missing = flat_run(&mut blank, &flat_params("FLOOR1", "NOSUCH")).expect_err("no such flat");
    assert_eq!(missing.code(), "not_found", "missing flat code");
}

#[test]
fn flat_run_previews_without_writing_or_recording() {
    let mut session = bulk_session();
    let mut preview = flat_params("FLOOR1", "NUKAGE1");
    preview.dry_run = Some(true);
    let report = flat_run(&mut session, &preview).expect("the preview runs");
    let shown = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(shown["maps"], json!({"MAP01": 3_u64}), "counts reported");
    assert_eq!(shown["dry_run"], json!(true), "flagged as a preview");
    assert_eq!(shown["copied_maps"], json!([]), "nothing copied");
    assert!(session.working().is_empty(), "the working archive stands");
    assert!(session.undo_labels().is_empty(), "no history entry");
}

#[test]
fn thing_type_run_labels_the_step_with_the_resolved_numbers() {
    let mut session = bulk_session();
    let mut preview = thing_params(name("imp"), name("demon"));
    preview.dry_run = Some(true);
    let previewed = thing_type_run(&mut session, &preview).expect("the preview runs");
    let shown = serde_json::to_value(&previewed).expect("the report serializes");
    assert_eq!(shown["maps"], json!({"MAP01": 1_u64}), "counts reported");
    assert_eq!(shown["dry_run"], json!(true), "flagged as a preview");
    assert_eq!(shown["copied_maps"], json!([]), "a preview copies nothing");
    assert_eq!(shown["from"], json!(3001_u64), "the resolved doomednum");
    assert_eq!(shown["to"], json!(3002_u64), "and the resolved target");
    assert!(
        session.undo_labels().is_empty(),
        "the preview records nothing"
    );
    let report = thing_type_run(&mut session, &thing_params(name("Imp"), number(3002_u32)))
        .expect("the swap runs");
    let written = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(written["total"], json!(1_u64), "the fixture imp moved");
    assert_eq!(
        report.swap.copied_maps,
        vec!["MAP01".to_owned()],
        "copied in"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["replace_thing_type 3001 -> 3002"],
        "one labeled step with the resolved numbers"
    );
}

#[test]
fn thing_type_run_names_the_failing_parameter_on_both_sides() {
    let mut session = bulk_session();
    let bad_from = thing_type_run(&mut session, &thing_params(name("platypus"), name("imp")))
        .expect_err("no thing named platypus");
    assert_eq!(bad_from.code(), "invalid_argument", "unknown from code");
    assert!(
        bad_from.to_string().starts_with("from:"),
        "the refusal names from: {bad_from}"
    );
    let bad_to = thing_type_run(&mut session, &thing_params(name("imp"), name("emon")))
        .expect_err("several types share that substring");
    assert_eq!(bad_to.code(), "invalid_argument", "ambiguous to code");
    assert!(
        bad_to.to_string().starts_with("to:"),
        "the refusal names to: {bad_to}"
    );
    assert!(
        session.undo_labels().is_empty(),
        "refusals write nothing and record nothing"
    );
}

/// An undefined doomednum the binary `Doom` records can still store.
const UNDEFINED_KIND: u32 = 9999_u32;

#[test]
fn thing_type_run_checks_the_target_and_allow_unknown_escapes_it() {
    let mut session = bulk_session();
    let refused = thing_type_run(
        &mut session,
        &thing_params(number(3001_u32), number(UNDEFINED_KIND)),
    )
    .expect_err("9999 is undefined");
    assert_eq!(refused.code(), "invalid_argument", "undefined target code");
    let message = refused.to_string();
    assert!(
        message.contains("get_thing_types") && message.contains("allow_unknown"),
        "the reference and the escape hatch are named: {message}"
    );
    assert!(
        session.undo_labels().is_empty(),
        "the refusal records nothing"
    );
    let mut allowed = thing_params(number(3001_u32), number(UNDEFINED_KIND));
    allowed.allow_unknown = Some(true);
    let report = thing_type_run(&mut session, &allowed).expect("the escape admits it");
    let shown = serde_json::to_value(&report).expect("the report serializes");
    assert_eq!(
        shown["to"],
        json!(i64::from(UNDEFINED_KIND)),
        "the written number is reported as-is"
    );
    assert_eq!(
        shown["total"],
        json!(1_u64),
        "the imp took the undefined type"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["replace_thing_type 3001 -> 9999"],
        "the escape hatch writes like any other run"
    );
}
