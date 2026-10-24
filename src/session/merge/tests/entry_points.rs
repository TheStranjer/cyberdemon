//! The entry points and helpers around the rules: empty and whole-archive
//! merges, the layer numbering, the merged-directory analysis, the edit
//! range flagging and the synthesized marker lump.

use super::{lump, marker, name, payload_lump, payload_of, pwad, tagged};
use crate::session::merge::{
    Base, analyze_directory, marker_lump, merge, merge_all, merge_counts, merge_into, tag,
};
use crate::session::{MergedLump, Source};
use crate::wad::archive::{Wad, WadKind};
use crate::wad::classify::Namespace;
use crate::wad::name::NAME_LEN;

#[test]
fn an_empty_overlay_changes_nothing() {
    let base = pwad(vec![lump("PLAYPAL"), marker("MAP01"), lump("THINGS")]);
    let view = merge(
        tag(&base, Source::Layer(0)),
        &Wad::default(),
        Source::Working,
    );
    assert_eq!(
        tagged(&view),
        vec!["PLAYPAL@IWAD", "MAP01@IWAD", "THINGS@IWAD"]
    );
}

#[test]
fn merge_all_numbers_layers_in_load_order() {
    let first = pwad(vec![lump("PLAYPAL")]);
    let second = pwad(vec![lump("ADVSND")]);
    let view = merge_all(&[&first, &second], &Wad::default());
    assert_eq!(
        tagged(&view),
        vec!["PLAYPAL@IWAD", "ADVSND@R1"],
        "layer 0 then layer 1 then nothing from the working archive"
    );
}

#[test]
fn analyze_reports_the_namespace_of_a_merged_region() {
    let iwad = pwad(vec![marker("F_START"), lump("FLOOR_1"), marker("F_END")]);
    let overlay = pwad(vec![marker("F_START"), lump("FLOOR_2"), marker("F_END")]);
    let view = merge(tag(&iwad, Source::Layer(0)), &overlay, Source::Working);
    let analysis = analyze_directory(&view);
    let flat = analysis
        .lumps
        .iter()
        .find(|info| info.name == name("FLOOR_2"))
        .expect("the new flat");
    assert_eq!(
        flat.namespace,
        Some(Namespace::Flats),
        "a lump inserted before the end marker stays in its region"
    );
}

#[test]
fn skipping_past_the_end_of_the_directory_is_harmless() {
    let mut base = Base::new(tag(&pwad(vec![lump("PLAYPAL")]), Source::Layer(0)));
    base.edits.skip((0, 99));
    assert!(base.finish().is_empty(), "the whole directory was dropped");
}

#[test]
fn a_marker_lump_pads_and_uppercases_its_name() {
    let behalf = MergedLump {
        lump: lump("FLOOR_1"),
        source: Source::Working,
        source_index: 7,
    };
    let start = marker_lump("f_start", &behalf);
    assert_eq!(start.lump.name, name("F_START"), "uppercased");
    assert_eq!(start.lump.size(), 0, "markers are empty");
    assert_eq!(start.source, Source::Working, "it reports its region");
    assert_eq!(start.source_index, 7, "and the region member's position");
    let long = marker_lump("TOOLONGNAME", &behalf);
    assert_eq!(
        long.lump.name.as_bytes().len(),
        NAME_LEN,
        "a name past the directory width is cut to eight bytes"
    );
}

#[test]
fn merge_into_an_empty_base_takes_the_overlay_whole() {
    let overlay = pwad(vec![
        lump("PLAYPAL"),
        marker("F_START"),
        lump("FLOOR_1"),
        marker("F_END"),
    ]);
    let merged = merge_into(&Wad::default(), &overlay);
    assert_eq!(merged.kind, WadKind::Pwad, "the base's kind persists");
    assert_eq!(
        merged
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect::<Vec<String>>(),
        vec!["PLAYPAL", "F_START", "FLOOR_1", "F_END"],
        "an empty base has nothing to override"
    );
}

#[test]
fn merge_into_applies_the_view_rules_under_the_base_kind() {
    let mut base = pwad(vec![
        lump("PLAYPAL"),
        marker("MAP01"),
        lump("THINGS"),
        marker("F_START"),
        lump("FLOOR_1"),
        marker("F_END"),
    ]);
    base.kind = WadKind::Iwad;
    let overlay = pwad(vec![
        payload_lump("PLAYPAL", "new palette"),
        marker("F_START"),
        payload_lump("FLOOR_1", "new flat"),
        marker("F_END"),
        marker("MAP01"),
        payload_lump("THINGS", "new things"),
    ]);
    let merged = merge_into(&base, &overlay);
    assert_eq!(merged.kind, WadKind::Iwad, "the base kind wins");
    assert_eq!(
        merged
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect::<Vec<String>>(),
        vec!["PLAYPAL", "MAP01", "THINGS", "F_START", "FLOOR_1", "F_END"],
        "every override landed where the base entry stood"
    );
    assert_eq!(
        payload_of(&merged, "PLAYPAL"),
        "new palette",
        "global override"
    );
    assert_eq!(
        payload_of(&merged, "THINGS"),
        "new things",
        "map block override"
    );
    assert_eq!(
        payload_of(&merged, "FLOOR_1"),
        "new flat",
        "region override"
    );
}

#[test]
fn merge_counts_sees_overrides_and_arrivals() {
    let base = pwad(vec![
        marker("MAP01"),
        lump("THINGS"),
        marker("F_START"),
        lump("FLOOR_1"),
        marker("F_END"),
    ]);
    let overlay = pwad(vec![
        lump("DEHACKED"),
        marker("MAP01"),
        lump("THINGS"),
        marker("MAP02"),
        lump("THINGS"),
        marker("FF_START"),
        lump("FLOOR_1"),
        lump("F_SKY1"),
        marker("FF_END"),
    ]);
    let counts = merge_counts(&base, &overlay);
    assert_eq!(counts.replaced, 2, "MAP01 as a block and the flat FLOOR_1");
    assert_eq!(
        counts.added, 3,
        "the DEHACKED global, map MAP02 and flat F_SKY1"
    );
}

#[test]
fn merge_counts_keeps_regions_and_globals_apart() {
    let base = pwad(vec![lump("FLOOR_1"), lump("GAMEINFO")]);
    let overlay = pwad(vec![
        marker("F_START"),
        lump("FLOOR_1"),
        marker("F_END"),
        lump("FLOOR_2"),
    ]);
    let counts = merge_counts(&base, &overlay);
    assert_eq!(
        counts.replaced, 0,
        "the region lump does not override the global"
    );
    assert_eq!(
        counts.added, 2,
        "the flat arrives as a new region and FLOOR_2 rides along with it"
    );
}

#[test]
fn merge_counts_of_a_marker_only_overlay_is_empty() {
    let base = pwad(vec![lump("PLAYPAL")]);
    let overlay = pwad(vec![marker("F_START"), marker("F_END")]);
    let counts = merge_counts(&base, &overlay);
    assert_eq!(
        (counts.replaced, counts.added),
        (0, 0),
        "markers alone are not units"
    );
}
