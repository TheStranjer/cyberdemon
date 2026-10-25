//! The content-reading sweeps and the commit: trimming the working
//! `PNAMES` and renumbering the texture lumps, the patches and flats no
//! merged reference uses, the malformed `PNAMES` and map refusals, what
//! [`super::apply`] leaves in the archive and the wire shape of an
//! [`crate::wad::optimize::Action`] row.

use serde_json::json;

use super::{
    dropped, flat_map, lump, marker, name, patch_region, planned, pnames, pnames_of, rows, targets,
    texture1,
};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::optimize::{Action, Inputs, Operation, Target, apply, plan};
use crate::wad::textures::parse_texture_lump;

#[test]
fn unused_pnames_prunes_the_list_and_renumbers_the_texture_lump() {
    let base = vec![pnames(&["WALLP1", "STEPP1"]), texture1(&[0])];
    let working = vec![pnames(&["WALLP1", "TALK", "STEPP1"]), texture1(&[0, 2])];
    let outcome = planned(&working, &base, &Operation::SAFE);
    assert_eq!(
        rows(&outcome),
        vec![(Operation::UnusedPnames, "TALK".to_owned())],
        "the unreferenced middle entry"
    );
    assert_eq!(outcome.lumps_removed(), 0, "no directory entry goes");
    assert_eq!(outcome.bytes_saved(), 8, "one PNAMES slot");
    assert_eq!(
        dropped(&outcome, Operation::UnusedPnames),
        vec![1],
        "the row carries the PNAMES list index"
    );
    let rewrite = outcome.rewrite.as_ref().expect("the rewrite");
    assert_eq!(
        pnames_of(&rewrite.pnames),
        vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
        "the list shrinks"
    );
    let bytes = rewrite.texture1.as_ref().expect("TEXTURE1 renumbered");
    let parsed = parse_texture_lump(bytes, 2).expect("the re-encoded lump parses");
    let patches = &parsed.textures.first().expect("WALL1").patches;
    assert_eq!(patches.len(), 2, "both references survive");
    assert_eq!(patches[1].patch, 1, "STEPP1 moved from index 2 to 1");
    assert!(
        rewrite.texture2.is_none(),
        "there is no TEXTURE2 to rewrite"
    );
}

#[test]
fn unused_pnames_skips_a_working_copy_the_plan_removes_as_redundant() {
    let shared = pnames(&["WALLP1", "TALK"]);
    let textures = texture1(&[0]);
    let base = vec![shared.clone(), textures];
    let working = vec![shared];
    let outcome = planned(&working, &base, &Operation::SAFE);
    assert_eq!(
        rows(&outcome),
        vec![(Operation::IdenticalOverrides, "PNAMES".to_owned())],
        "the byte-identical override goes whole instead of being trimmed"
    );
    assert!(outcome.rewrite.is_none(), "and nothing is rewritten");
    let empty = vec![lump("MISC", b"m")];
    let second = planned(&empty, &base, &Operation::SAFE);
    assert!(
        second.rewrite.is_none() && second.actions.is_empty(),
        "no working PNAMES means the sweep does nothing"
    );
}

#[test]
fn a_fully_referenced_pnames_plans_nothing() {
    let base: Vec<Lump> = Vec::new();
    let working = vec![pnames(&["WALLP1"]), texture1(&[0])];
    let outcome = planned(&working, &base, &Operation::SAFE);
    assert!(
        outcome.rewrite.is_none() && outcome.actions.is_empty(),
        "every entry is referenced"
    );
}

#[test]
fn a_malformed_working_pnames_fails_the_plan_with_format() {
    let base: Vec<Lump> = Vec::new();
    let working = vec![lump("PNAMES", b"\x05\x00\x00\x00short")];
    let inputs = Inputs {
        working: &working,
        base: &base,
        merged: &working,
    };
    let error = plan(&inputs, &[Operation::UnusedPnames]).expect_err("truncated list");
    assert_eq!(error.code(), "format_error", "malformed PNAMES code");
}

#[test]
fn unused_patches_drops_only_what_no_texture_references_by_name() {
    let base = vec![pnames(&["WALLP1", "STEPP1"]), texture1(&[0])];
    let mut working = patch_region(&["WALLP1", "STEPP1", "EXTRA"]);
    working.push(lump("WALLP1", b"loose-is-not-a-patch"));
    let outcome = planned(&working, &base, &[Operation::UnusedPatches]);
    let names = targets(&outcome);
    assert_eq!(
        names,
        vec!["STEPP1".to_owned(), "EXTRA".to_owned()],
        "the referenced patch and the loose copy stay, the region's unreferenced names go"
    );
}

#[test]
fn unused_patches_without_a_pnames_touches_nothing() {
    let base: Vec<Lump> = Vec::new();
    let working = patch_region(&["ORPHAN"]);
    let outcome = planned(&working, &base, &[Operation::UnusedPatches]);
    assert!(
        outcome.actions.is_empty(),
        "no name list means no reference to check"
    );
}

#[test]
fn unused_flats_drops_only_what_no_merged_sector_uses() {
    let base = flat_map("MAP01", "FLOOR1", "F_SKY1");
    let working = vec![
        marker("F_START"),
        lump("FLOOR1", b"used"),
        lump("F_SKY1", b"sky"),
        lump("GONE", b"nobody"),
        marker("F_END"),
    ];
    let outcome = planned(&working, &base, &[Operation::UnusedFlats]);
    let names = targets(&outcome);
    assert_eq!(
        names,
        vec!["GONE".to_owned()],
        "only the flat no sector names"
    );
}

#[test]
fn a_sector_wearing_the_dash_placeholder_marks_no_flat_as_used() {
    // The sector's floor is the `"-"` none-marker, so `add_flat_name` skips
    // it: the `used` set holds only `FLOOR1`, and a flat lump of either name
    // is dropped even though a byte-for-byte `-` flat sits in the archive.
    let base = flat_map("MAP01", "-", "FLOOR1");
    let working = vec![
        marker("F_START"),
        lump("FLOOR1", b"used"),
        lump("-", b"placeholder"),
        marker("F_END"),
    ];
    let outcome = planned(&working, &base, &[Operation::UnusedFlats]);
    assert_eq!(
        targets(&outcome),
        vec!["-".to_owned()],
        "the `\"-\"` flat is a placeholder, never a used name"
    );
}

#[test]
fn unused_flats_refuses_when_a_merged_map_cannot_decode() {
    let mut base = flat_map("MAP01", "FLOOR1", "FLOOR1");
    if let Some(slot) = base.iter_mut().find(|entry| entry.name == name("THINGS")) {
        *slot = lump("THINGS", b"\x01\x02\x03");
    }
    let working = vec![marker("F_START"), lump("GONE", b"nobody"), marker("F_END")];
    let mut merged: Vec<Lump> = base.clone();
    merged.extend_from_slice(&working);
    let inputs = Inputs {
        working: &working,
        base: &base,
        merged: &merged,
    };
    let error = plan(&inputs, &[Operation::UnusedFlats]).expect_err("broken map");
    assert_eq!(error.code(), "format_error", "undecodable map code");
    assert!(
        error.to_string().contains("MAP01"),
        "the message names the map: {error}"
    );
}

#[test]
fn requested_subsets_run_only_their_own_sweep_in_canonical_order() {
    let base = vec![lump("PLAYPAL", b"same")];
    let working = vec![
        lump("PLAYPAL", b"same"),
        lump("NOTE", b"a"),
        lump("NOTE", b"b"),
    ];
    let subset = planned(&working, &base, &[Operation::ShadowedDuplicates]);
    assert_eq!(
        rows(&subset),
        vec![(Operation::ShadowedDuplicates, "NOTE".to_owned())],
        "only the asked-for sweep ran"
    );
    let both = planned(
        &working,
        &base,
        &[Operation::ShadowedDuplicates, Operation::IdenticalOverrides],
    );
    assert_eq!(
        rows(&both),
        vec![
            (Operation::IdenticalOverrides, "PLAYPAL".to_owned()),
            (Operation::ShadowedDuplicates, "NOTE".to_owned()),
        ],
        "the request order never changes the plan"
    );
}

#[test]
fn apply_drops_the_planned_entries_and_writes_the_rewrite_in_place() {
    let base = vec![lump("PLAYPAL", b"same")];
    let working = vec![
        lump("PLAYPAL", b"same"),
        pnames(&["WALLP1", "TALK"]),
        texture1(&[0]),
    ];
    let outcome = planned(&working, &base, &Operation::ALL);
    assert_eq!(
        outcome.bytes_saved(),
        12,
        "PLAYPAL bytes plus one PNAMES slot"
    );
    let mut wad = Wad {
        kind: WadKind::Pwad,
        lumps: working,
    };
    apply(&mut wad, &outcome);
    assert_eq!(wad.lumps.len(), 2, "the identical override went");
    assert_eq!(wad.find_first(name("PLAYPAL")), None, "by name");
    let kept = wad.lumps.first().expect("PNAMES is first now");
    assert_eq!(
        pnames_of(&kept.data),
        vec!["WALLP1".to_owned()],
        "the list was trimmed in place, not appended"
    );
    assert_eq!(
        wad.find_last(name("TEXTURE1")).expect("TEXTURE1 stays"),
        1,
        "and it was not re-added"
    );
}

#[test]
fn actions_serialize_with_the_documented_keys() {
    let lump_action = Action {
        operation: Operation::UnusedPnames,
        target: Target::Lump("TALK".to_owned()),
        index: 1,
        bytes: 8,
    };
    assert_eq!(
        serde_json::to_value(&lump_action).expect("serializable"),
        json!({
            "operation": "unused_pnames",
            "lump": "TALK",
            "index": 1_u64,
            "bytes": 8_u64,
        }),
        "a lump row"
    );
    let map_action = Action {
        operation: Operation::IdenticalOverrides,
        target: Target::Map(name("MAP01")),
        index: 0,
        bytes: 100,
    };
    assert_eq!(
        serde_json::to_value(&map_action).expect("serializable"),
        json!({
            "operation": "identical_overrides",
            "map": "MAP01",
            "index": 0_u64,
            "bytes": 100_u64,
        }),
        "a map row"
    );
}

#[test]
fn no_operations_requested_plans_an_empty_result() {
    let working = vec![lump("PLAYPAL", b"x")];
    let outcome = planned(&working, &[], &[]);
    assert!(outcome.is_empty(), "nothing requested, nothing planned");
    assert_eq!(outcome.bytes_saved(), 0, "no bytes");
    assert_eq!(outcome.lumps_removed(), 0, "no lumps");
}
