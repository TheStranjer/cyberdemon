//! Selecting and applying: the `maps` wire shapes, `resolve`'s default,
//! `"all"` and listed-name paths and refusals, and the one-undo-step,
//! dry-run and empty-selection shapes of `apply_to_maps` over several
//! map blocks at once.

use serde_json::{from_value, json};

use crate::session::Session;
use crate::tools::query::replace::{AllMaps, ApplyOptions, MapsSelector, apply_to_maps, resolve};
use crate::wad::map::bulk::{replace_flats, replace_textures, replace_thing_types};

use super::{all_maps, base_session, blocks, merged_map, name, names_of, repaint};

#[test]
fn maps_selector_reads_its_documented_wire_shapes() {
    let all: MapsSelector = from_value(json!("all")).expect("\"all\" is the whole view");
    assert!(
        matches!(all, MapsSelector::All(AllMaps)),
        "the literal string binds All"
    );
    let listed: MapsSelector = from_value(json!(["MAP01", "MAP02"])).expect("a list is a name set");
    let MapsSelector::Names(texts) = listed else {
        panic!("a list must read as Names: {listed:?}");
    };
    assert_eq!(texts, vec!["MAP01".to_owned(), "MAP02".to_owned()]);
    let stray: MapsSelector = from_value(json!("MAP01")).expect("a bare string deserializes");
    assert!(matches!(stray, MapsSelector::Other(_)), "not a selection");
    let number: MapsSelector = from_value(json!(7_i64)).expect("a number deserializes");
    assert!(
        matches!(number, MapsSelector::Other(_)),
        "also not a selection"
    );
}

#[test]
fn resolve_none_takes_the_active_or_only_map() {
    let only = base_session(blocks(&["MAP01"]));
    assert_eq!(
        resolve(&only, None).expect("the single map is the default"),
        vec![name("MAP01")],
        "one block needs no selection"
    );
    let mut busy = base_session(blocks(&["MAP01", "MAP02"]));
    let error = resolve(&busy, None).expect_err("two blocks, no default");
    assert_eq!(error.code(), "invalid_argument", "no-default code");
    busy.set_active_map(Some(name("MAP02")));
    assert_eq!(
        resolve(&busy, None).expect("the active map is the default"),
        vec![name("MAP02")],
        "and an active map resolves it"
    );
}

#[test]
fn resolve_all_lists_the_merged_blocks_once_each_in_order() {
    let session = base_session(blocks(&["MAP01", "MAP01", "MAP02"]));
    let all = resolve(&session, Some(&all_maps())).expect("the merged view reads");
    assert_eq!(
        all,
        vec![name("MAP01"), name("MAP02")],
        "a double-named block appears once, in directory order"
    );
    let empty = Session::default();
    assert!(
        resolve(&empty, Some(&all_maps()))
            .expect("no blocks is no names")
            .is_empty(),
        "an empty merged view selects nothing"
    );
}

#[test]
fn resolve_names_folds_case_dedupes_and_checks_the_merged_view() {
    let session = base_session(blocks(&["MAP01", "MAP02"]));
    let listed = resolve(&session, Some(&names_of(&["map02", "MAP01", "MAP02"])))
        .expect("listed names resolve");
    assert_eq!(
        listed,
        vec![name("MAP02"), name("MAP01")],
        "given order, uppercased, deduplicated"
    );
    let error = resolve(&session, Some(&names_of(&["MAP09"]))).expect_err("no MAP09 here");
    assert_eq!(error.code(), "not_found", "a missing map is not_found");
    let message = error.message();
    assert!(message.contains("MAP09"), "the message names it: {message}");
    let long =
        resolve(&session, Some(&names_of(&["MAP012345"]))).expect_err("nine characters is over");
    assert_eq!(long.code(), "invalid_argument", "unparsable name code");
    let empty_list = resolve(&session, Some(&names_of(&[]))).expect_err("no names given");
    assert_eq!(empty_list.code(), "invalid_argument", "empty list code");
    assert!(
        empty_list.to_string().contains("\"all\""),
        "the message names the alternative: {empty_list}"
    );
}

#[test]
fn resolve_refuses_a_stray_maps_value_naming_the_two_shapes() {
    let session = base_session(blocks(&["MAP01"]));
    let stray = from_value::<MapsSelector>(json!("ALL")).expect("a wrong-case string reads");
    let error = resolve(&session, Some(&stray)).expect_err("not a selection");
    assert_eq!(error.code(), "invalid_argument", "stray selector code");
    let message = error.to_string();
    assert!(
        message.contains("\"all\"") && message.contains("list_maps"),
        "the message names \"all\" and list_maps: {message}"
    );
}

#[test]
fn a_dry_run_reports_the_maps_without_touching_the_session() {
    let mut session = base_session(blocks(&["MAP01", "MAP02"]));
    let targets = resolve(&session, Some(&all_maps())).expect("both maps");
    let options = ApplyOptions {
        label: "replace flats",
        dry_run: true,
    };
    let run = apply_to_maps(&mut session, &targets, &options, |map| {
        replace_flats(map, "FLOOR4", "FLOOR9", &[])
    })
    .expect("the dry pass runs");
    assert_eq!(
        run.per_map,
        vec![(name("MAP01"), 2_usize), (name("MAP02"), 2_usize)],
        "two flat fields per map"
    );
    assert!(run.copied_maps.is_empty(), "a dry run copies nothing");
    assert!(session.undo_labels().is_empty(), "and records no history");
    assert!(!session.is_dirty(), "the session stayed clean");
    assert!(
        session.working().is_empty(),
        "the working archive stayed empty"
    );
    let kept = merged_map(&session, "MAP01");
    assert_eq!(
        kept.sectors.first().expect("sector 0").floor_flat.as_str(),
        "FLOOR4",
        "the merged view still reads the layer's block"
    );
}

#[test]
fn one_apply_step_covers_every_named_map_and_undoes_as_one() {
    let mut session = base_session(blocks(&["MAP01", "MAP02"]));
    let targets = resolve(&session, Some(&all_maps())).expect("both maps");
    let options = ApplyOptions {
        label: "bulk flats",
        dry_run: false,
    };
    let run = apply_to_maps(&mut session, &targets, &options, |map| {
        replace_flats(map, "*", "FLOOR9", &[])
    })
    .expect("the step runs");
    assert_eq!(
        run.per_map,
        vec![(name("MAP01"), 2_usize), (name("MAP02"), 2_usize)],
        "both maps answered"
    );
    assert_eq!(
        run.copied_maps,
        vec!["MAP01".to_owned(), "MAP02".to_owned()],
        "both maps came from the layer alone"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["bulk flats"],
        "two maps, one undo entry"
    );
    assert!(session.is_dirty(), "and unsaved work");
    for text in ["MAP01", "MAP02"] {
        let written = merged_map(&session, text);
        assert_eq!(
            written
                .sectors
                .first()
                .expect("sector 0")
                .floor_flat
                .as_str(),
            "FLOOR9",
            "{text} was rewritten"
        );
    }
    assert_eq!(session.undo().expect("undo the step"), "bulk flats");
    assert!(
        session.working().is_empty(),
        "both copies rolled back with the edits"
    );
}

#[test]
fn a_reapplied_map_is_already_editable_and_already_replaced() {
    let mut session = base_session(blocks(&["MAP01"]));
    let targets = resolve(&session, Some(&all_maps())).expect("the one map");
    let options = ApplyOptions {
        label: "bulk things",
        dry_run: false,
    };
    let first = apply_to_maps(&mut session, &targets, &options, |map| {
        replace_thing_types(map, 3004, 9)
    })
    .expect("the first run");
    assert_eq!(
        first.per_map,
        vec![(name("MAP01"), 1_usize)],
        "one thing moved"
    );
    assert_eq!(first.copied_maps, vec!["MAP01".to_owned()], "copied in");
    let second = apply_to_maps(&mut session, &targets, &options, |map| {
        replace_thing_types(map, 3004, 9)
    })
    .expect("the second run");
    assert_eq!(
        second.per_map,
        vec![(name("MAP01"), 0_usize)],
        "nothing left to move"
    );
    assert!(second.copied_maps.is_empty(), "already editable");
    assert_eq!(session.undo_labels().len(), 2, "two steps, two labels");
}

#[test]
fn apply_refuses_a_map_no_archive_holds_and_changes_nothing() {
    let mut session = base_session(blocks(&["MAP01"]));
    let write = ApplyOptions {
        label: "bulk walls",
        dry_run: false,
    };
    let dry = ApplyOptions {
        label: "bulk walls",
        dry_run: true,
    };
    let missing = name("MAP09");
    let error = apply_to_maps(&mut session, &[name("MAP01"), missing], &write, |map| {
        replace_textures(map, "WALL*", "BRICK1", &[])
    })
    .expect_err("MAP09 is nowhere");
    assert_eq!(error.code(), "not_found", "missing map code");
    assert!(session.working().is_empty(), "the whole step was discarded");
    assert!(session.undo_labels().is_empty(), "and recorded nothing");
    let dry_error = apply_to_maps(&mut session, &[missing], &dry, |map| {
        replace_textures(map, "WALL*", "BRICK1", &[])
    })
    .expect_err("the dry read refuses too");
    assert_eq!(dry_error.code(), "not_found", "dry missing map code");
}

#[test]
fn empty_selections_apply_and_record_nothing() {
    let mut session = base_session(blocks(&["MAP01"]));
    let dry = ApplyOptions {
        label: "bulk nothing",
        dry_run: true,
    };
    let write = ApplyOptions {
        label: "bulk nothing",
        dry_run: false,
    };
    let preview = apply_to_maps(&mut session, &[], &dry, repaint).expect("dry no-op");
    let done = apply_to_maps(&mut session, &[], &write, repaint).expect("write no-op");
    assert!(
        preview.per_map.is_empty() && preview.copied_maps.is_empty(),
        "the dry no-op answered nothing"
    );
    assert!(
        done.per_map.is_empty() && done.copied_maps.is_empty(),
        "the write no-op answered nothing"
    );
    assert!(session.working().is_empty(), "nothing written");
    assert!(
        session.undo_labels().is_empty(),
        "no history entry for nothing"
    );
    assert!(!session.is_dirty(), "and the session stayed clean");
}
