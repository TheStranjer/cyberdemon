//! The keyed read cache: one merge per session state, invalidation on every
//! mutation path (`edit`, `undo`, `redo`, layer add, base swap, working
//! replacement, the batch scope's commit and rollback), the definitions
//! parse cached the same way, and the cache transparent to clone, equality
//! and `Debug`.

use super::{add_marker, layer, loaded, lump, marker, name, pwad, tagged};
use crate::session::cache::{Key, Slot};
use crate::session::history::BatchOutcome;
use crate::session::{LayerRole, Scope, Session, merge};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::maps::map_owners;
use crate::wad::classify::{MapBlock, MapFormat, analyze};
use crate::wad::game::Game;
use crate::wad::name::LumpName;

/// A lump named `label` carrying `body`, so stream changes are tellable.
fn carrying(label: &str, body: &str) -> Lump {
    Lump::new(name(label), body.as_bytes().to_vec())
}

#[test]
fn repeated_reads_build_the_merged_directory_once() {
    let session = loaded();
    assert_eq!(session.merge_count(), 0, "nothing has been read yet");
    let first = tagged(&session.merged());
    let second = tagged(&session.merged());
    let looked = session
        .find(name("FLOOR_1"), None)
        .expect("the base flat is there");
    let (entries, analysis) = session.analyzed(Scope::Merged).expect("merged analyzes");
    assert_eq!(first, second, "the repeated view stands");
    assert_eq!(looked.lump.name, name("FLOOR_1"), "the lookup answered");
    assert_eq!(tagged(&entries), first, "analyzed pairs with merged");
    assert_eq!(
        analysis.lumps.len(),
        entries.len(),
        "one classification entry per view lump"
    );
    assert_eq!(
        session.merge_count(),
        1,
        "four reads, one merge: the cache served the rest"
    );
}

#[test]
fn the_cached_view_and_analysis_equal_a_fresh_fold_of_the_same_state() {
    let session = loaded();
    let layers: Vec<&Wad> = session
        .layers()
        .into_iter()
        .map(|layer| &layer.wad)
        .collect();
    let fresh = merge::merge_all(&layers, session.working());
    assert_eq!(session.merged(), fresh, "the cached directory is the fold");
    let (entries, analysis) = session.analyzed(Scope::Merged).expect("merged reads");
    assert_eq!(
        analysis,
        merge::analyze_directory(&fresh),
        "the cached classification matches a fresh analyze of the same directory"
    );
    assert_eq!(entries, fresh, "the pairing carries the very view");
}

#[test]
fn game_detection_follows_the_layers_without_touching_the_cache() {
    let session = loaded();
    assert_eq!(session.game(), Game::Doom2, "the base's rows answer");
    assert_eq!(session.merge_count(), 0, "the IWAD branch reads no view");
}

#[test]
fn an_edit_invalidates_the_view_and_the_next_read_sees_the_change() {
    let mut session = loaded();
    let before = tagged(&session.merged());
    assert_eq!(session.merge_count(), 1, "the first read built once");
    add_marker(&mut session, "NEWWIN").expect("plain append");
    let after = tagged(&session.merged());
    assert_eq!(
        session.merge_count(),
        2,
        "the edit forced exactly one rebuild"
    );
    assert_ne!(before, after, "the view moved on");
    assert!(
        after.iter().any(|entry| entry == "NEWWIN@W"),
        "the working lump stands in the fresh view: {after:?}"
    );
}

#[test]
fn undo_and_redo_invalidate_the_view() {
    let mut session = loaded();
    add_marker(&mut session, "TEMP").expect("plain append");
    let edited = tagged(&session.merged());
    assert!(edited.iter().any(|e| e == "TEMP@W"), "the edit is in");
    session.undo().expect("one step back");
    let undone = tagged(&session.merged());
    assert!(
        !undone.iter().any(|e| e == "TEMP@W"),
        "undo rebuilt the view without the edit: {undone:?}"
    );
    session.redo().expect("replay");
    let redone = tagged(&session.merged());
    assert_eq!(redone, edited, "redo rebuilt the view back onto the edit");
    assert_eq!(
        session.merge_count(),
        3,
        "the edit's read, the undo's and the redo's: one rebuild each"
    );
}

#[test]
fn stacking_a_resource_and_swapping_the_base_invalidate_the_view() {
    let mut session = loaded();
    assert!(
        !session.merged().is_empty(),
        "the first read folds the base"
    );
    session
        .add_resource(layer(
            LayerRole::Resource,
            "more",
            vec![carrying("SKYADD", "sky")],
        ))
        .expect("no batch is open");
    let with_resource = tagged(&session.merged());
    assert!(
        with_resource.iter().any(|e| e == "SKYADD@L2"),
        "the new layer folded in: {with_resource:?}"
    );
    session
        .set_iwad(layer(LayerRole::Iwad, "swap", vec![marker("BAREONLY")]))
        .expect("no batch is open");
    let swapped = tagged(&session.merged());
    assert!(
        swapped.iter().any(|e| e == "BAREONLY@L0"),
        "the swapped base stands at layer 0: {swapped:?}"
    );
    assert!(
        !swapped.iter().any(|e| e == "FLOOR_1@L0"),
        "the replaced base's flat is gone: {swapped:?}"
    );
    assert_eq!(
        session.merge_count(),
        3,
        "read, resource push and base swap: one rebuild each"
    );
}

#[test]
fn replacing_the_working_archive_invalidates_the_view() {
    let mut session = loaded();
    assert!(
        !session.merged().is_empty(),
        "the first read folds the base"
    );
    session
        .replace_working(pwad(vec![carrying("BRANDNEW", "fresh")]), None)
        .expect("no batch is open");
    let after = tagged(&session.merged());
    assert!(
        after.iter().any(|e| e == "BRANDNEW@W"),
        "the adopted archive is the one merged: {after:?}"
    );
    assert_eq!(session.merge_count(), 2, "the swap forced one rebuild");
}

#[test]
fn a_rolled_back_batch_forces_exactly_one_rebuild_back_to_the_pre_batch_view() {
    let mut session = loaded();
    let before = tagged(&session.merged());
    let token = session.begin_batch("batched").expect("no batch is open");
    add_marker(&mut session, "GONE").expect("plain append");
    let left = session
        .end_batch(token, BatchOutcome::Rollback)
        .expect("the token names the open batch");
    assert!(!left, "a rolled-back batch leaves no entry");
    let after = tagged(&session.merged());
    assert_eq!(after, before, "the rollback's read is the pre-batch view");
    assert_eq!(
        session.merge_count(),
        2,
        "one pre-batch fold, one post-rollback fold"
    );
}

#[test]
fn a_committed_batch_reads_fold_the_moment_they_happen_and_the_land_rebuilds() {
    let mut session = loaded();
    let token = session.begin_batch("kept").expect("no batch is open");
    add_marker(&mut session, "KEPT").expect("plain append");
    let mid = tagged(&session.merged());
    assert!(
        mid.iter().any(|e| e == "KEPT@W"),
        "mid-batch reads see the folded edits"
    );
    let left = session
        .end_batch(token, BatchOutcome::Commit)
        .expect("the token names the open batch");
    assert!(left, "the fold stands as one entry");
    let landed = tagged(&session.merged());
    assert_eq!(landed, mid, "the committed view is the folded one");
    session.undo().expect("one undo takes the whole batch back");
    let undone = tagged(&session.merged());
    assert!(
        !undone.iter().any(|e| e == "KEPT@W"),
        "and the next read rebuilt without it"
    );
}

#[test]
fn definitions_parse_once_and_reparse_on_the_mutation_paths() {
    let mut session = Session::default();
    session
        .set_iwad(layer(
            LayerRole::Iwad,
            "doom2",
            vec![
                marker("MAP01"),
                carrying("DECORATE", "actor CacheOne 9101 { }"),
            ],
        ))
        .expect("no batch is open");
    let first = session.definitions().expect("the overlay parses");
    session.definitions().expect("cached");
    session.definitions().expect("cached");
    assert_eq!(session.definition_count(), 1, "three reads, one parse");
    assert!(first.thing(9101).is_some(), "the DECORATE thing reads");

    session
        .edit("override the DECORATE", |wad| {
            wad.lumps
                .push(carrying("DECORATE", "actor CacheTwo 9102 { }"));
            Ok(())
        })
        .expect("a plain append");
    let second = session.definitions().expect("the edit reparsed");
    assert_eq!(
        session.definition_count(),
        2,
        "the edit forced exactly one reparse"
    );
    assert!(second.thing(9102).is_some(), "the new custom reads through");
    assert!(
        second.thing(9101).is_none(),
        "the working override replaced the base's DECORATE in the merged view"
    );

    session.undo().expect("one step back");
    let third = session.definitions().expect("the undo reparsed");
    assert_eq!(session.definition_count(), 3, "undo reparsed once");
    assert!(
        third.thing(9102).is_none(),
        "the undone custom is gone from the fresh definitions"
    );
    assert!(third.thing(9101).is_some(), "the base custom reads again");
}

#[test]
fn a_layer_swap_invalidates_the_definitions_too() {
    let mut session = Session::default();
    session
        .set_iwad(layer(
            LayerRole::Iwad,
            "one",
            vec![carrying("DECORATE", "actor LayerOne 9111 { }")],
        ))
        .expect("no batch is open");
    assert!(
        session.definitions().expect("parses").thing(9111).is_some(),
        "the first layer's custom reads"
    );
    session
        .set_iwad(layer(
            LayerRole::Iwad,
            "two",
            vec![carrying("DECORATE", "actor LayerTwo 9112 { }")],
        ))
        .expect("no batch is open");
    let swapped = session.definitions().expect("reparses");
    assert_eq!(
        session.definition_count(),
        2,
        "the base swap forced a reparse"
    );
    assert!(swapped.thing(9112).is_some(), "the new base custom reads");
    assert!(
        swapped.thing(9111).is_none(),
        "the vanished base custom is gone"
    );
}

#[test]
fn a_clone_caches_cold_and_keeps_the_originals_content() {
    let mut session = loaded();
    assert!(
        !session.merged().is_empty(),
        "the first read folds the base"
    );
    let snapshot = session.clone();
    add_marker(&mut session, "LATER").expect("plain append");
    let snapshot_view = tagged(&snapshot.merged());
    assert!(
        !snapshot_view.iter().any(|e| e == "LATER@W"),
        "the clone holds the pre-edit archives, merged fresh"
    );
    assert_eq!(snapshot.merge_count(), 1, "the clone started cold");
    assert_eq!(
        session.merge_count(),
        1,
        "the original has not read since its edit"
    );
    let live = tagged(&session.merged());
    assert_ne!(live, snapshot_view, "the original's read rebuilt its view");
    assert!(
        live.iter().any(|e| e == "LATER@W"),
        "and shows its own edit"
    );
    assert_eq!(
        session.merge_count(),
        2,
        "the original rebuilt after its edit"
    );
}

#[test]
fn equality_ignores_the_cache_and_debug_shows_it_as_machinery() {
    let cold = loaded();
    let warm = loaded();
    assert!(!warm.merged().is_empty(), "the warm read folds the base");
    warm.definitions().expect("the merged scope parses");
    assert_eq!(cold, warm, "a warm cache is not a state difference");
    let shown = format!("{warm:?}");
    assert!(shown.contains("ViewCache"), "the cache shows up as itself");
    assert!(
        shown.contains("Slot(warm)") && shown.contains("Slot(cold)"),
        "and reports only its slots' warmth: {shown}"
    );
}

#[test]
fn base_and_merged_fold_once_each_under_the_shared_key() {
    let session = loaded();
    let first = tagged(&session.base_lumps());
    let second = tagged(&session.base_lumps());
    assert_eq!(first, second, "the layer-only view stands");
    assert_eq!(session.merge_count(), 1, "the base folded once");
    let merged_view = tagged(&session.merged());
    let merged_again = tagged(&session.merged());
    assert_eq!(merged_view, merged_again, "and so does the merged one");
    assert_eq!(
        session.merge_count(),
        2,
        "base and merged are two folds, then every read is served"
    );
}

#[test]
fn slot_serves_only_successes_and_serves_them_once() {
    let slot: Slot<u32> = Slot::default();
    let key = Key {
        revision: 0_u64,
        generation: 0_u64,
    };
    slot.get_or_try_init::<_, ()>(key, || Err(()))
        .expect_err("a failing init reports");
    let first = slot
        .get_or_try_init::<_, ()>(key, || Ok(7_u32))
        .expect("the retry succeeds");
    assert_eq!(first, 7_u32, "the success stands");
    let again = slot.get_or_init(key, || 99_u32);
    assert_eq!(again, 7_u32, "the cached answer wins over a new init");
    let next_key = Key {
        revision: 1_u64,
        generation: 0_u64,
    };
    let moved = slot.get_or_init(next_key, || 9_u32);
    assert_eq!(moved, 9_u32, "a new key rebuilds");
    slot.invalidate();
    assert_eq!(
        slot.get_or_init(next_key, || 42_u32),
        42_u32,
        "and so does an explicit invalidation"
    );
}

/// The overlapping synthetic pair: `MAP02`'s `GL` range covers `MAP01`'s
/// header and member, so first-claimant-wins and the scan's block order
/// must answer identically.
fn overlapping_blocks() -> Vec<MapBlock> {
    vec![
        MapBlock {
            name: name("MAP01"),
            header: 0_usize,
            end: 3_usize,
            format: MapFormat::Doom,
            gl: Some((6_usize, 8_usize)),
        },
        MapBlock {
            name: name("MAP02"),
            header: 3_usize,
            end: 6_usize,
            format: MapFormat::Doom,
            gl: Some((0_usize, 2_usize)),
        },
    ]
}

/// The original per-lump scan rule, spelled out as the tests' oracle: the
/// first block (in directory order) that owns `index`, its header position
/// answering with the `true` flag.
fn scanned_owner(blocks: &[MapBlock], index: usize) -> Option<(LumpName, bool)> {
    blocks.iter().find_map(|block| {
        if index == block.header {
            Some((block.name, true))
        } else if (block.header < index && index < block.end)
            || block
                .gl
                .is_some_and(|(start, end)| start <= index && index < end)
        {
            Some((block.name, false))
        } else {
            None
        }
    })
}

#[test]
fn the_owner_table_answers_exactly_what_the_scan_does() {
    let blocks = overlapping_blocks();
    let owners: Vec<Option<(LumpName, bool)>> = map_owners(&blocks, 9_usize);
    for index in 0_usize..9_usize {
        assert_eq!(
            owners.get(index).copied().flatten(),
            scanned_owner(&blocks, index),
            "position {index} of the overlapping synthetic blocks"
        );
    }
    assert_eq!(
        owners.first().copied().flatten(),
        Some((name("MAP01"), true)),
        "MAP01's header claim wins MAP02's later GL reach back"
    );
    assert_eq!(
        owners.get(8_usize).copied().flatten(),
        None,
        "one past every claim is unowned"
    );
    let empty: Vec<MapBlock> = Vec::new();
    assert!(
        map_owners(&empty, 0_usize).is_empty(),
        "an empty directory claims nothing"
    );
}

#[test]
fn an_analysis_from_a_real_directory_agrees_with_the_scan() {
    let lumps = vec![
        marker("MAP01"),
        lump("THINGS"),
        marker("GL_MAP01"),
        lump("GL_VERT"),
        lump("GL_SEGS"),
        lump("TAIL"),
    ];
    let analysis = analyze(&lumps);
    let owned: Vec<Option<LumpName>> = analysis.lumps.iter().map(|info| info.map).collect();
    assert_eq!(
        owned,
        vec![
            Some(name("MAP01")),
            Some(name("MAP01")),
            Some(name("MAP01")),
            Some(name("MAP01")),
            Some(name("MAP01")),
            None,
        ],
        "the header, its member and the whole GL run belong to the map, the tail does not"
    );
}
