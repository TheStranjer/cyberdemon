//! The three listings over the fixture session: [`Page::parse`]'s
//! defaults and refusals, [`super::last_wins`] on the repeated flat, the
//! shapes, formats, offsets, `PNAMES` flags and namespaces the rows report,
//! the sprite groups with their mirrored rotations and invalid names, the
//! prefix validation, the empty session and the working-archive override.

use alloc::collections::BTreeSet;

use crate::gfx::sprite_name::parse_sprite_name;
use crate::session::{Session, Source};
use crate::tools::Page;
use crate::tools::graphics::list::{list_flats_run, list_patches_run, patch_row};
use crate::tools::graphics::list_sprites::{
    Groups, collect_sprite, list_sprites_run, parse_prefix, sprite_rows,
};
use crate::tools::graphics::{DEFAULT_LIMIT, MAX_LIMIT, in_namespace, last_wins};
use crate::tools::lumps::rows::LumpRow;
use crate::wad::archive::Lump;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

use super::{candidates, code, fixture, header, name, page, row_named, rows};

#[test]
fn pages_default_and_reject_out_of_range_limits() {
    let fresh = Page::parse(None, None, DEFAULT_LIMIT, MAX_LIMIT).expect("defaults");
    assert_eq!((fresh.offset, fresh.limit), (0, 200), "documented defaults");
    assert_eq!(
        Page::parse(Some(3), Some(2000), DEFAULT_LIMIT, MAX_LIMIT)
            .expect("max")
            .limit,
        2000,
        "the cap"
    );
    for bad in [0_usize, 2001] {
        let error =
            Page::parse(None, Some(bad), DEFAULT_LIMIT, MAX_LIMIT).expect_err("out of range");
        assert_eq!(code(&error), "invalid_argument", "limit {bad} code");
    }
}

#[test]
fn last_wins_keeps_the_final_occurrence_in_order() {
    let rows = rows(&fixture());
    let flats: Vec<&LumpRow> = rows
        .iter()
        .filter(|row| in_namespace(row, Namespace::Flats))
        .collect();
    let deduped = last_wins(flats.clone());
    let floor1 = deduped
        .iter()
        .find(|row| row.name == name("FLOOR1"))
        .expect("FLOOR1 stays once");
    assert_eq!(floor1.size, 8_192, "the second copy wins");
    assert_eq!(
        deduped.len().saturating_add(1),
        flats.len(),
        "exactly one row was dropped"
    );
    let names: Vec<String> = deduped.iter().map(|row| row.name.to_string()).collect();
    assert_eq!(
        names,
        vec![
            "FLOOR1".to_owned(),
            "PNGFLAT".to_owned(),
            "ODDFLET".to_owned()
        ],
        "directory order is kept"
    );
}

#[test]
fn flats_report_raw_shapes_png_and_unknown_sizes() {
    let session = fixture();
    let report = list_flats_run(&session, &page(0, 10), None).expect("runs");
    assert_eq!(report.total, 3, "FLOOR1 (deduped), PNGFLAT, ODDFLET");
    let floor = report.flats.first().expect("FLOOR1");
    assert_eq!(
        (floor.width, floor.height, floor.format),
        (Some(64), Some(128), "raw"),
        "the winning 8192-byte copy"
    );
    let pngflat = report.flats.get(1).expect("PNGFLAT");
    assert_eq!(
        (pngflat.width, pngflat.height, pngflat.format),
        (Some(64), Some(64), "png"),
        "dimensions from the IHDR"
    );
    let odd = report.flats.get(2).expect("ODDFLET");
    assert_eq!(
        (odd.width, odd.height, odd.format),
        (None, None, "raw"),
        "100 bytes imply no vanilla shape"
    );
    assert_eq!(odd.source, Source::Layer(0), "read from the layer");
    let filtered = list_flats_run(&session, &page(0, 10), Some("floo*")).expect("runs");
    assert_eq!(filtered.total, 1, "the glob applies");
    let paged = list_flats_run(&session, &page(1, 1), None).expect("runs");
    assert_eq!(
        (paged.total, paged.flats.len()),
        (3, 1),
        "the middle of three"
    );
    assert_eq!(paged.flats.first().expect("one row").name, "PNGFLAT");
}

#[test]
fn patch_candidates_are_the_region_plus_pnames_named_globals() {
    let listed = candidates();
    assert!(
        listed.contains("WALLP1") && listed.contains("JUNKP1"),
        "every patches-region lump is listed"
    );
    assert!(listed.contains("PNGPAT"), "PNAMES pulls in the global");
    assert!(
        !listed.contains("TROOA1"),
        "a sprites lump named by PNAMES stays out"
    );
    assert!(!listed.contains("FLOOR1"), "flats stay out");
    assert!(
        !listed.iter().any(|text| text.ends_with("_START")),
        "markers are never listed"
    );
}

#[test]
fn patches_report_formats_offsets_and_the_in_pnames_flag() {
    let session = fixture();
    let report = list_patches_run(&session, &page(0, 10), None, false).expect("runs");
    assert_eq!(report.total, 3, "two region patches plus the global PNG");
    let wall = &report.patches[0];
    assert_eq!(wall.format, "doom", "the header reads");
    assert_eq!(
        (wall.width, wall.height, wall.left_offset, wall.top_offset),
        (Some(16_u32), Some(16_u32), Some(-2_i32), Some(-14_i32)),
        "header fields"
    );
    assert_eq!(wall.namespace, Some("patches"), "in the region");
    assert!(wall.in_pnames, "WALLP1 is named");
    let junk = &report.patches[1];
    assert_eq!(junk.format, "unknown", "40 x's are neither format");
    assert_eq!(
        (junk.width, junk.height, junk.left_offset, junk.top_offset),
        (None, None, None, None),
        "nothing measured"
    );
    assert!(!junk.in_pnames, "JUNKP1 is not in PNAMES");
    assert_eq!(junk.namespace, Some("patches"), "still a region lump");
    let pngpat = &report.patches[2];
    assert_eq!(pngpat.format, "png", "the global PNG");
    assert_eq!(
        (pngpat.width, pngpat.height),
        (Some(4), Some(2)),
        "from the IHDR"
    );
    assert_eq!(
        (pngpat.left_offset, pngpat.top_offset),
        (Some(3_i32), Some(-4_i32)),
        "from grAb"
    );
    assert_eq!(pngpat.namespace, None, "outside any patches region");
    let only_named = list_patches_run(&session, &page(0, 10), None, true).expect("runs");
    assert_eq!(only_named.total, 2, "WALLP1 and PNGPAT");
    assert_eq!(
        list_patches_run(&session, &page(0, 10), Some("*P1"), false)
            .expect("runs")
            .total,
        2,
        "the glob keeps the region pair"
    );
}

#[test]
fn a_malformed_pnames_surfaces_as_a_format_error() {
    let mut session = fixture();
    session
        .edit("junk PNAMES", |wad| {
            wad.lumps.push(Lump::new(name("PNAMES"), vec![0_u8; 3]));
            Ok(())
        })
        .expect("plain append");
    let error = list_patches_run(&session, &page(0, 10), None, false).expect_err("no names fit");
    assert_eq!(code(&error), "format_error", "malformed PNAMES code");
}

#[test]
fn sprites_group_frames_rotations_and_mirrors() {
    let session = fixture();
    let report = list_sprites_run(&session, &page(0, 10), None).expect("runs");
    assert_eq!(report.total, 2, "PLAY and TROO");
    assert_eq!(report.invalid, vec!["BADNAME".to_owned()], "the odd name");
    let troo = report.sprites.get(1).expect("TROO sorts after PLAY");
    assert_eq!(troo.frames.len(), 1, "frame A only");
    let rotations: Vec<(u8, &str, bool)> = troo.frames[0]
        .rotations
        .iter()
        .map(|row| (row.rotation, row.lump.as_str(), row.mirrored))
        .collect();
    assert_eq!(
        rotations,
        vec![
            (1, "TROOA1", false),
            (2, "TROOA2A8", false),
            (8, "TROOA2A8", true),
        ],
        "by rotation, the mirror entry flagged"
    );
    let play = report.sprites.first().expect("PLAY");
    assert_eq!(play.frames[0].rotations[0].rotation, 0, "the all-angles");
    let filtered = list_sprites_run(&session, &page(0, 10), Some("TROO")).expect("runs");
    assert_eq!(filtered.total, 1, "one group survives the prefix");
    assert!(filtered.invalid.is_empty(), "BADNAME is not a TROO");
    assert_eq!(
        list_sprites_run(&session, &page(0, 10), Some("BADN"))
            .expect("runs")
            .invalid,
        vec!["BADNAME".to_owned()],
        "the prefix keeps its invalid names"
    );
    assert_eq!(
        list_sprites_run(&session, &page(1, 1), None)
            .expect("runs")
            .sprites
            .first()
            .expect("the second group")
            .sprite,
        "TROO",
        "paging skips PLAY"
    );
    let past = list_sprites_run(&session, &page(2, 1), None).expect("runs");
    assert_eq!((past.total, past.sprites.len()), (2, 0), "past the end");
}

#[test]
fn prefixes_are_uppercased_and_validated() {
    assert_eq!(
        parse_prefix(Some("poss")).expect("four letters"),
        Some("POSS".to_owned()),
        "case is normalised"
    );
    assert_eq!(parse_prefix(None).expect("absent"), None, "no filter");
    for bad in ["", "POSSS", "PO!S", "P1"] {
        let error = parse_prefix(Some(bad)).expect_err("not four letters or digits");
        assert_eq!(code(&error), "invalid_argument", "prefix {bad:?} code");
    }
}

#[test]
fn empty_sessions_list_nothing() {
    let session = Session::default();
    let flats = list_flats_run(&session, &page(0, 10), None).expect("runs");
    assert_eq!(flats.total, 0, "no flats");
    let patches = list_patches_run(&session, &page(0, 10), None, false).expect("runs");
    assert_eq!(patches.total, 0, "no patches and no PNAMES is fine");
    let sprites = list_sprites_run(&session, &page(0, 10), None).expect("runs");
    assert!(
        sprites.sprites.is_empty() && sprites.invalid.is_empty() && sprites.total == 0,
        "empty lists are not an error"
    );
}

#[test]
fn a_working_override_replaces_the_layer_row_of_the_same_name() {
    let mut session = fixture();
    session
        .edit("override WALLP1", |wad| {
            wad.lumps
                .push(Lump::new(name("WALLP1"), header(8, 8, 0, 0)));
            Ok(())
        })
        .expect("plain append");
    let report = list_patches_run(&session, &page(0, 10), Some("WALLP1"), false).expect("runs");
    assert_eq!(report.total, 1, "one row per name");
    let wall = report.patches.first().expect("the winner");
    assert_eq!(wall.width, Some(8), "the working copy's header");
    assert_eq!(wall.source, Source::Working, "and its origin");
}

#[test]
fn sprite_helpers_group_only_grammar_matching_names() {
    let pnames: BTreeSet<LumpName> = BTreeSet::new();
    let directory = rows(&fixture());
    let troo = row_named(&directory, "TROOA2A8");
    assert_eq!(
        patch_row(troo, &pnames).format,
        "doom",
        "a sprite is a patch"
    );
    let bad = row_named(&directory, "BADNAME");
    assert_eq!(patch_row(bad, &pnames).format, "unknown", "five bytes");
    assert!(parse_sprite_name("BADNAME").is_none(), "not a sprite name");
    let parsed = parse_sprite_name("TROOA2A8").expect("this one is");
    let mut groups = Groups::new();
    collect_sprite(&mut groups, &parsed, "TROOA2A8", Some("PLAY"));
    assert!(groups.is_empty(), "the prefix filter drops it");
    collect_sprite(&mut groups, &parsed, "TROOA2A8", None);
    let sprites = sprite_rows(groups);
    let frames = sprites
        .first()
        .expect("one sprite")
        .frames
        .first()
        .expect("one frame");
    assert_eq!(frames.rotations.len(), 2, "direct and mirrored entries");
    assert!(frames.rotations[1].mirrored, "the second one is mirrored");
}
