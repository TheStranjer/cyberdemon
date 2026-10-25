//! The two listings over the fixture set: what [`TextureSet::load`] decodes
//! from the winning lumps, what [`TextureSet::find`] answers with, the rows
//! [`list_textures_run`] and [`list_pnames_run`] filter and page, the
//! [`Page`] bounds [`parse_page`] refuses and the lumps
//! [`crate::session::Session::find_global`] locates.

use crate::session::{Session, Source};
use crate::tools::textures::list::{PnamesReport, list_pnames_run, list_textures_run};
use crate::tools::textures::{TextureSet, WhichLump, parse_page};
use crate::wad::archive::Lump;
use crate::wad::lumps;
use crate::wad::textures::{TextureLayout, TextureLump, write_texture_lump};

use super::{code, def, name, page, pnames_lump, session, set_of, texture1_lump};

#[test]
fn the_load_decodes_the_winning_lumps_and_finds_by_name() {
    let loaded = set_of(&session());
    assert_eq!(
        loaded.pnames,
        vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
        "PNAMES decoded"
    );
    let (wall, which) = loaded.find("wall1").expect("case-insensitive hit");
    assert_eq!(wall.width, 64, "the fixture width");
    assert_eq!(which, WhichLump::Texture1, "came from TEXTURE1");
    assert!(loaded.find("NOPE").is_none(), "an unknown name misses");
}

#[test]
fn texture2_entries_override_texture1_names_in_find() {
    let override_lump = TextureLump {
        layout: TextureLayout::Doom,
        textures: vec![def("WALL1", 8, 8, &[0])],
    };
    let mut session = session();
    session
        .edit("add TEXTURE2", |wad| {
            let bytes = write_texture_lump(&override_lump).expect("serializable");
            wad.lumps.push(Lump::new(name("TEXTURE2"), bytes));
            Ok(())
        })
        .expect("plain append");
    let loaded = set_of(&session);
    let (wall, which) = loaded.find("WALL1").expect("found");
    assert_eq!(which, WhichLump::Texture2, "TEXTURE2 wins");
    assert_eq!(wall.height, 8, "and it is the override definition");
}

#[test]
fn the_listing_rows_carry_lump_and_source() {
    let rows = set_of(&session()).texture_rows();
    assert_eq!(rows.len(), 2, "both textures");
    assert_eq!(rows[0].lump, "TEXTURE1", "TEXTURE1 rows first");
    assert_eq!(rows[1].patches, 1, "the mappatch count");
    assert_eq!(rows[0].source, Source::Layer(0), "read from the layer");
}

#[test]
fn find_global_returns_the_last_copy_of_the_merged_view() {
    let mut session = session();
    session
        .edit("override PNAMES", |wad| {
            wad.lumps.push(pnames_lump(&["ONLY"]));
            Ok(())
        })
        .expect("plain append");
    let found = session.find_global(lumps::PNAMES).expect("found");
    assert_eq!(found.source, Source::Working, "the working copy wins");
    let loaded = set_of(&session);
    assert_eq!(loaded.pnames, vec!["ONLY".to_owned()], "loaded the winner");
}

#[test]
fn pages_default_and_reject_out_of_range_limits() {
    let fresh = parse_page(None, None).expect("defaults");
    assert_eq!((fresh.offset, fresh.limit), (0, 200), "documented defaults");
    assert_eq!(
        parse_page(Some(3), Some(2000)).expect("max").limit,
        2000,
        "the cap"
    );
    for bad in [0_usize, 2001] {
        let error = parse_page(None, Some(bad)).expect_err("out of range");
        assert_eq!(code(&error), "invalid_argument", "limit {bad} code");
        assert!(
            error.to_string().contains("between 1 and 2000"),
            "the message states the range: {error}"
        );
    }
}

#[test]
fn listing_pages_filters_and_counts_the_total() {
    let session = session();
    let all = list_textures_run(&session, &page(0, 10), None)
        .expect("runs")
        .textures;
    assert_eq!(all.len(), 2, "both rows");
    let filtered = list_textures_run(&session, &page(0, 10), Some("wall*")).expect("runs");
    assert_eq!(filtered.total, 1, "the pattern counts");
    assert_eq!(filtered.textures.len(), 1, "one row");
    let report = list_textures_run(&session, &page(1, 1), None).expect("runs");
    assert_eq!(
        (report.total, report.textures.len()),
        (2, 1),
        "the second page"
    );
    assert_eq!(
        report.textures.first().expect("one row").name,
        "STEP1",
        "in lump order"
    );
    let past = list_textures_run(&session, &page(9, 1), None).expect("runs");
    assert!(
        past.textures.is_empty() && past.total == 2,
        "past the end is empty"
    );
    let names: PnamesReport = list_pnames_run(&session, &page(0, 10), None).expect("runs");
    assert_eq!(names.total, 2, "both entries");
    assert!(
        names
            .pnames
            .iter()
            .any(|row| row.name == "WALLP1" && row.index == 0 && row.lump_found),
        "WALLP1 has a lump outside the flats and sprites regions"
    );
    assert!(
        names
            .pnames
            .iter()
            .any(|row| row.name == "STEPP1" && !row.lump_found),
        "STEPP1 has no lump in this fixture"
    );
    let one = list_pnames_run(&session, &page(0, 10), Some("s*")).expect("runs");
    assert_eq!(one.total, 1, "the pattern counts");
    assert_eq!(
        one.pnames.first().expect("one row").name,
        "STEPP1",
        "glob match"
    );
}

#[test]
fn a_missing_pnames_lump_leaves_the_index_unbounded() {
    let mut bare = Session::default();
    bare.edit("add TEXTURE1 only", |wad| {
        wad.lumps.push(texture1_lump());
        Ok(())
    })
    .expect("plain append");
    let loaded = set_of(&bare);
    assert!(loaded.pnames.is_empty(), "no names");
    assert_eq!(loaded.texture_rows().len(), 2, "the lump still decodes");
    let report = list_pnames_run(&bare, &page(0, 10), None).expect("runs");
    assert_eq!(report.total, 0, "an empty list is not an error");
}

#[test]
fn a_malformed_pnames_lump_surfaces_as_a_format_error() {
    let mut session = session();
    session
        .edit("junk PNAMES", |wad| {
            wad.lumps.push(Lump::new(name("PNAMES"), vec![0_u8; 3]));
            Ok(())
        })
        .expect("plain append");
    let error = TextureSet::load(&session).expect_err("the count claims more names than fit");
    assert_eq!(code(&error), "format_error", "malformed PNAMES code");
}
