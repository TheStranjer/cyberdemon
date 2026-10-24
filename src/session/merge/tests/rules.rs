//! The vanilla override rules: what an overlay replaces in place, what it
//! appends, how map blocks, namespace regions and markers move, and which
//! archives win the names they share.

use super::{lump, marker, names, pwad, tagged};
use crate::session::Source;
use crate::session::merge::{merge, merge_all, tag};

#[test]
fn tag_keeps_the_archives_own_order_and_positions() {
    let wad = pwad(vec![lump("PLAYPAL"), marker("F_START")]);
    let view = tag(&wad, Source::Working);
    assert_eq!(
        tagged(&view),
        vec!["PLAYPAL@W", "F_START@W"],
        "every entry keeps its archive"
    );
    assert_eq!(
        view.iter()
            .map(|entry| entry.source_index)
            .collect::<Vec<_>>(),
        vec![0_usize, 1],
        "directory positions survive"
    );
}

#[test]
fn an_empty_directory_takes_the_overlay_verbatim() {
    let overlay = pwad(vec![
        lump("PLAYPAL"),
        marker("F_START"),
        lump("FLOOR_1"),
        marker("F_END"),
    ]);
    let view = merge(Vec::new(), &overlay, Source::Layer(0));
    assert_eq!(
        names(&view),
        vec!["PLAYPAL", "F_START", "FLOOR_1", "F_END"],
        "nothing to override, so nothing moved or was dropped"
    );
    assert!(
        view.iter().all(|entry| entry.source == Source::Layer(0)),
        "every entry reports the overlay"
    );
}

#[test]
fn a_global_lump_replaces_the_last_copy_of_its_name() {
    let base = pwad(vec![lump("PLAYPAL"), lump("DOOMGI"), lump("PLAYPAL")]);
    let overlay = pwad(vec![lump("PLAYPAL")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        tagged(&view),
        vec!["PLAYPAL@IWAD", "DOOMGI@IWAD", "PLAYPAL@W"],
        "the last copy changed, in place"
    );
}

#[test]
fn an_unknown_global_lump_is_appended() {
    let base = pwad(vec![lump("PLAYPAL")]);
    let overlay = pwad(vec![lump("GAMEINFO")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(names(&view), vec!["PLAYPAL", "GAMEINFO"], "appended last");
}

#[test]
fn a_map_block_replaces_the_base_block_of_the_same_name() {
    let base = pwad(vec![
        marker("MAP01"),
        lump("THINGS"),
        lump("LINEDEFS"),
        lump("PLAYPAL"),
    ]);
    let overlay = pwad(vec![marker("MAP01"), lump("THINGS")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        tagged(&view),
        vec!["MAP01@W", "THINGS@W", "PLAYPAL@IWAD"],
        "the base block's remaining member was dropped in place"
    );
}

#[test]
fn every_base_map_affects_only_its_own_block() {
    let base = pwad(vec![
        marker("MAP01"),
        lump("THINGS"),
        marker("MAP02"),
        lump("THINGS"),
    ]);
    let overlay = pwad(vec![
        marker("MAP02"),
        lump("THINGS"),
        lump("LINEDEFS"),
        marker("MAP01"),
        lump("THINGS"),
        lump("SIDEDEFS"),
    ]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["MAP01", "THINGS", "SIDEDEFS", "MAP02", "THINGS", "LINEDEFS"],
        "each block was replaced where it stood, in base order"
    );
    assert!(
        view.iter().all(|entry| entry.source == Source::Working),
        "every entry comes from the overlay"
    );
}

#[test]
fn a_gl_block_moves_with_its_map() {
    let base = pwad(vec![
        marker("MAP01"),
        lump("THINGS"),
        lump("SECTORS"),
        marker("GL_MAP01"),
        lump("GL_VERT"),
        lump("GL_NODES"),
        lump("PLAYPAL"),
    ]);
    let overlay = pwad(vec![marker("MAP01"), lump("THINGS")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        tagged(&view),
        vec!["MAP01@W", "THINGS@W", "PLAYPAL@IWAD"],
        "the base block went as a whole, its `GL` nodes with it"
    );
}

#[test]
fn an_overlay_map_bringing_gl_nodes_replaces_the_base_block() {
    let base = pwad(vec![marker("MAP01"), lump("THINGS"), lump("PLAYPAL")]);
    let overlay = pwad(vec![
        marker("MAP01"),
        lump("THINGS"),
        marker("GL_MAP01"),
        lump("GL_VERT"),
    ]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        tagged(&view),
        vec![
            "MAP01@W",
            "THINGS@W",
            "GL_MAP01@W",
            "GL_VERT@W",
            "PLAYPAL@IWAD"
        ],
        "the whole block including the `GL` run arrived in place"
    );
}

#[test]
fn an_overlay_map_without_a_base_block_is_appended() {
    let base = pwad(vec![marker("MAP01"), lump("THINGS")]);
    let overlay = pwad(vec![marker("E1M1"), lump("THINGS")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["MAP01", "THINGS", "E1M1", "THINGS"],
        "the new map landed at the end"
    );
    assert_eq!(
        view.get(3).expect("the appended THINGS").source,
        Source::Working,
        "the new map's data is the overlay's"
    );
    assert_eq!(
        view.get(1).expect("the base THINGS").source,
        Source::Layer(0),
        "the old map is untouched"
    );
}

#[test]
fn two_overlay_maps_of_one_name_replace_the_base_only_once() {
    let base = pwad(vec![marker("MAP01"), lump("THINGS")]);
    let overlay = pwad(vec![
        marker("MAP01"),
        lump("THINGS"),
        marker("MAP01"),
        lump("LINEDEFS"),
    ]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["MAP01", "THINGS", "MAP01", "LINEDEFS"],
        "the second copy appends instead of claiming the base block again"
    );
}

#[test]
fn map_data_is_not_copied_twice_as_a_global() {
    let base = pwad(vec![lump("PLAYPAL")]);
    let overlay = pwad(vec![marker("MAP05"), lump("THINGS"), lump("SECTORS")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["PLAYPAL", "MAP05", "THINGS", "SECTORS"],
        "the block was appended once"
    );
}

#[test]
fn a_namespace_lump_replaces_the_base_copy() {
    let base = pwad(vec![
        marker("F_START"),
        lump("FLOOR_1"),
        lump("FLOOR_2"),
        marker("F_END"),
    ]);
    let overlay = pwad(vec![marker("F_START"), lump("FLOOR_2"), marker("F_END")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        tagged(&view),
        vec!["F_START@IWAD", "FLOOR_1@IWAD", "FLOOR_2@W", "F_END@IWAD"],
        "the region's content was replaced, its markers untouched"
    );
}

#[test]
fn a_new_namespace_lump_goes_before_the_end_marker() {
    let base = pwad(vec![marker("F_START"), lump("FLOOR_1"), marker("F_END")]);
    let overlay = pwad(vec![marker("F_START"), lump("F_NEW"), marker("F_END")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["F_START", "FLOOR_1", "F_NEW", "F_END"],
        "the new flat joined the region instead of trailing it"
    );
}

#[test]
fn a_namespace_the_base_lacks_arrives_as_a_region() {
    let base = pwad(vec![lump("PLAYPAL")]);
    let overlay = pwad(vec![
        marker("F_START"),
        lump("FLOOR_1"),
        lump("FLOOR_2"),
        marker("F_END"),
    ]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["PLAYPAL", "F_START", "FLOOR_1", "FLOOR_2", "F_END"],
        "the region arrived whole, markers included"
    );
    assert_eq!(
        view.get(1).expect("the start marker").source_index,
        1,
        "the synthesized marker reports the region member it opens"
    );
    assert_eq!(
        view.get(4).expect("the end marker").source_index,
        2,
        "the closing marker reports the region's last member"
    );
}

#[test]
fn alias_markers_open_and_close_the_same_region() {
    let base = pwad(vec![marker("SS_START"), lump("TROOA1"), marker("SS_END")]);
    let overlay = pwad(vec![marker("S_START"), lump("TROOB1"), marker("S_END")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["SS_START", "TROOA1", "TROOB1", "SS_END"],
        "the alias region was joined rather than duplicated"
    );
}

#[test]
fn a_region_lump_does_not_override_the_same_named_global() {
    let base = pwad(vec![lump("FLOOR_1")]);
    let overlay = pwad(vec![marker("F_START"), lump("FLOOR_1"), marker("F_END")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["FLOOR_1", "F_START", "FLOOR_1", "F_END"],
        "the global stays and the region gets its own copy"
    );
}

#[test]
fn overlay_markers_are_never_copied_on_their_own() {
    let base = pwad(vec![lump("PLAYPAL")]);
    let overlay = pwad(vec![
        marker("F_START"),
        marker("F1_START"),
        lump("FLOOR_1"),
        marker("F1_END"),
        marker("F_END"),
    ]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        names(&view),
        vec!["PLAYPAL", "F_START", "FLOOR_1", "F_END"],
        "one clean region, no inner markers carried over"
    );
}

#[test]
fn a_marker_only_overlay_contributes_nothing() {
    let base = pwad(vec![marker("F_START"), lump("FLOOR_1"), marker("F_END")]);
    let overlay = pwad(vec![marker("F_START"), marker("F_END")]);
    let view = merge(tag(&base, Source::Layer(0)), &overlay, Source::Working);
    assert_eq!(
        tagged(&view),
        vec!["F_START@IWAD", "FLOOR_1@IWAD", "F_END@IWAD"],
        "markers alone are not an override"
    );
}

#[test]
fn the_last_of_three_archives_wins_every_name() {
    let iwad = pwad(vec![
        lump("PLAYPAL"),
        marker("MAP01"),
        lump("THINGS"),
        marker("F_START"),
        lump("FLOOR_1"),
        marker("F_END"),
    ]);
    let resource = pwad(vec![
        lump("GAMEINFO"),
        marker("F_START"),
        lump("F_SKY1"),
        marker("F_END"),
        lump("MIDWARS"),
    ]);
    let working = pwad(vec![
        marker("MAP01"),
        lump("THINGS"),
        marker("F_START"),
        lump("FLOOR_1"),
        marker("F_END"),
    ]);
    let view = merge_all(&[&iwad, &resource], &working);
    assert_eq!(
        tagged(&view),
        vec![
            "PLAYPAL@IWAD",
            "MAP01@W",
            "THINGS@W",
            "F_START@IWAD",
            "FLOOR_1@W",
            "F_SKY1@R1",
            "F_END@IWAD",
            "GAMEINFO@R1",
            "MIDWARS@R1"
        ],
        "each name was overridden where it stood, by the latest archive"
    );
}
