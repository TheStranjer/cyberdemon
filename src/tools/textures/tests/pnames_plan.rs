//! `edit_pnames`' plan and its parts: [`plan_edit`]'s `rename` and
//! `replace_all` outcomes, [`name_mapping`]'s renumbering, the
//! [`EditPnamesParams`] reader [`pick_operation`], the [`Wad::put_global`] lump
//! write and the merged rows the set is read through.

use crate::session::{Scope, Session};
use crate::tools::textures::list::texture_rows;
use crate::tools::textures::pnames::{
    EditPnamesParams, Operation, RenamePair, RenameRow, name_mapping, plan_edit,
};
use crate::tools::textures::pnames_rewrite::{edit_pnames_run, pick_operation};
use crate::wad::archive::{Lump, Wad, WadKind};

use super::{code, name, run, session, set_of, texture1_lump};

#[test]
fn edit_pnames_rename_keeps_indices_and_textures() {
    let mut session = session();
    let pairs = vec![RenamePair {
        from: "wallp1".to_owned(),
        to: "WALLPX".to_owned(),
    }];
    let report =
        edit_pnames_run(&mut session, Operation::Rename(&pairs), false).expect("the rename runs");
    assert_eq!(report.pnames, 2, "the count stands");
    assert_eq!(
        report.renamed,
        vec![RenameRow {
            from: "WALLP1".to_owned(),
            to: "WALLPX".to_owned()
        }],
        "the uppercased pair is reported"
    );
    assert_eq!(report.textures_changed, 0, "indices did not move");
    let loaded = set_of(&session);
    assert_eq!(loaded.pnames[0], "WALLPX", "renamed in place");
    assert_eq!(
        texture_rows(&loaded)[0].patches,
        3,
        "WALL1 still has its refs"
    );
    let working: Vec<String> = session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect();
    assert_eq!(working, vec!["PNAMES"], "only PNAMES was copied in");
}

#[test]
fn rename_reports_missing_sources_and_taken_targets() {
    let missing = vec![RenamePair {
        from: "NOPE".to_owned(),
        to: "WALLPX".to_owned(),
    }];
    let missing_error = run(Operation::Rename(&missing), false).expect_err("no such entry");
    assert_eq!(code(&missing_error), "not_found", "missing from code");
    let taken = vec![RenamePair {
        from: "WALLP1".to_owned(),
        to: "STEPP1".to_owned(),
    }];
    let taken_error = run(Operation::Rename(&taken), false).expect_err("target is taken");
    assert_eq!(code(&taken_error), "conflict", "taken to code");
    let bad = vec![RenamePair {
        from: "WALLP1".to_owned(),
        to: "bad name".to_owned(),
    }];
    let bad_error = run(Operation::Rename(&bad), false).expect_err("bad name");
    assert_eq!(code(&bad_error), "invalid_argument", "bad name code");
    let empty: Vec<RenamePair> = Vec::new();
    let empty_error = run(Operation::Rename(&empty), false).expect_err("no pairs");
    assert_eq!(code(&empty_error), "invalid_argument", "empty rename code");
}

#[test]
fn edit_pnames_replace_all_reindexes_by_name() {
    let names = ["STEPP1".to_owned(), "WALLPX".to_owned()];
    let error = run(Operation::ReplaceAll(&names), false)
        .expect_err("WALLP1 vanishes while WALL1 still refers to it");
    assert_eq!(code(&error), "conflict", "conflict without force");
    let mut session = session();
    let report =
        edit_pnames_run(&mut session, Operation::ReplaceAll(&names), true).expect("the force runs");
    assert_eq!(report.pnames, 2, "the new list");
    assert_eq!(
        report.removed,
        vec!["WALLP1".to_owned()],
        "the vanished entry"
    );
    assert_eq!(
        report.added,
        vec!["WALLPX".to_owned()],
        "the brought-in entry"
    );
    assert_eq!(
        report.textures_changed, 2,
        "STEP1 moved 1 to 0, WALL1 pruned"
    );
    let loaded = set_of(&session);
    assert_eq!(
        loaded.pnames,
        vec!["STEPP1".to_owned(), "WALLPX".to_owned()],
        "the replacement stands"
    );
}

#[test]
fn replace_all_validates_its_list_and_replaces_nothing_when_there_is_no_list() {
    let dupes = ["A".to_owned(), "a".to_owned()];
    let dupe_error = run(Operation::ReplaceAll(&dupes), false).expect_err("case-folded dupe");
    assert_eq!(code(&dupe_error), "invalid_argument", "duplicate code");
    assert!(
        dupe_error.to_string().contains("twice"),
        "the message names the problem: {dupe_error}"
    );
    let bad = ["TOO LONG".to_owned()];
    let bad_error = run(Operation::ReplaceAll(&bad), false).expect_err("bad name");
    assert_eq!(code(&bad_error), "invalid_argument", "bad name code");
    // A session without PNAMES but with textures: no old names vanish,
    // so the empty replacement just creates the lump.
    let mut bare = Session::default();
    bare.edit("add TEXTURE1", |wad| {
        wad.lumps.push(texture1_lump());
        Ok(())
    })
    .expect("plain append");
    let empty: Vec<String> = Vec::new();
    let report = edit_pnames_run(&mut bare, Operation::ReplaceAll(&empty), false)
        .expect("nothing was removed, so nothing conflicts");
    assert_eq!(report.pnames, 0, "the fresh list is empty");
    assert_eq!(
        report.textures_changed, 0,
        "the unbounded lump is left alone"
    );
}

#[test]
fn an_empty_add_or_remove_list_is_invalid_argument() {
    let empty: Vec<String> = Vec::new();
    let add_error = run(Operation::Add(&empty), false).expect_err("empty add");
    assert_eq!(code(&add_error), "invalid_argument", "empty add code");
    let remove_error = run(Operation::Remove(&empty), false).expect_err("empty remove");
    assert_eq!(code(&remove_error), "invalid_argument", "empty remove code");
    assert!(
        add_error.to_string().contains("at least one"),
        "the message states the rule: {add_error}"
    );
}

#[test]
fn put_global_overwrites_the_last_working_copy_in_place() {
    let mut wad = Wad {
        kind: WadKind::Pwad,
        lumps: vec![
            Lump::new(name("PNAMES"), vec![1_u8]),
            Lump::new(name("X"), vec![2_u8]),
            Lump::new(name("PNAMES"), vec![3_u8]),
        ],
    };
    wad.put_global(name("PNAMES"), vec![9_u8])
        .expect("the overwrite runs");
    assert_eq!(wad.lumps.len(), 3, "no lump grew in");
    assert_eq!(
        wad.lumps.last().expect("the last PNAMES").data.to_vec(),
        vec![9_u8],
        "the last copy changed"
    );
    assert_eq!(
        wad.lumps.first().expect("the first PNAMES").data.to_vec(),
        vec![1_u8],
        "the earlier copy stands"
    );
}

#[test]
fn put_global_appends_a_lump_the_archive_lacks() {
    let mut wad = Wad::default();
    wad.put_global(name("TEXTURE1"), vec![7_u8])
        .expect("the append runs");
    assert_eq!(wad.lumps.len(), 1, "one lump arrived");
    assert_eq!(
        wad.lumps.first().expect("one lump").name,
        name("TEXTURE1"),
        "its name"
    );
}

#[test]
fn the_operation_picker_accepts_exactly_one_field() {
    let args = EditPnamesParams {
        add: Some(vec!["A".to_owned()]),
        remove: None,
        rename: None,
        replace_all: None,
        force: None,
    };
    assert!(
        matches!(pick_operation(&args), Ok(Operation::Add(_))),
        "add is picked"
    );
    let two = EditPnamesParams {
        remove: Some(vec!["A".to_owned()]),
        ..args
    };
    let two_error = pick_operation(&two).expect_err("two operations");
    assert_eq!(code(&two_error), "invalid_argument", "two-op code");
    assert!(
        two_error.to_string().contains("exactly one"),
        "the message states the rule: {two_error}"
    );
    let none = EditPnamesParams {
        add: None,
        remove: None,
        rename: None,
        replace_all: None,
        force: None,
    };
    let none_error = pick_operation(&none).expect_err("no operation");
    assert_eq!(code(&none_error), "invalid_argument", "zero-op code");
    let renamed = EditPnamesParams {
        rename: Some(Vec::new()),
        ..none
    };
    assert!(
        matches!(pick_operation(&renamed), Ok(Operation::Rename(_))),
        "rename is picked"
    );
}

#[test]
fn the_name_mapping_tracks_moves_vanishing_and_standing_still() {
    let current = ["A".to_owned(), "B".to_owned(), "C".to_owned()];
    let names = ["C".to_owned(), "A".to_owned()];
    let mapping = name_mapping(&current, &names)
        .expect("small lists")
        .expect("names moved");
    assert_eq!(
        mapping,
        vec![Some(1_u16), None, Some(0_u16)],
        "B vanished, A moved to 1, C to 0"
    );
    assert!(
        name_mapping(&current, &current)
            .expect("small lists")
            .is_none(),
        "an unchanged list needs no mapping"
    );
    let appended = [
        "A".to_owned(),
        "B".to_owned(),
        "C".to_owned(),
        "D".to_owned(),
    ];
    assert!(
        name_mapping(&current, &appended)
            .expect("small lists")
            .is_none(),
        "a pure append keeps every old index"
    );
}

#[test]
fn a_plan_without_a_mapping_skips_the_conflict_scan() {
    let current = ["A".to_owned(), "B".to_owned()];
    let list = ["C".to_owned()];
    let plan = plan_edit(&current, Operation::Add(&list), false, &[]).expect("a trivial plan");
    assert!(plan.mapping.is_none(), "an append never renumbers");
    assert_eq!(
        plan.names,
        vec!["A".to_owned(), "B".to_owned(), "C".to_owned()],
        "the list grew"
    );
    assert_eq!(plan.added, vec!["C".to_owned()], "one addition");
}

#[test]
fn scope_rows_stay_visible_through_the_set() {
    let rows =
        crate::tools::lumps::rows::build_rows(&session(), Scope::Merged).expect("merged rows");
    assert!(
        rows.iter().any(|row| row.name == name("WALLP1")),
        "the fixture lump is visible"
    );
}
