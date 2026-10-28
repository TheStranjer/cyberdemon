//! Composite texture resolution tests: definition lookup and patch
//! composition, the `TEXTURE2` precedence, `PNAMES` reporting, the `TX_`
//! patch fallback and the refusals.

use super::{
    code, definition, doom_patch, lump, name, place, pnames_lump, session, texture_bytes,
    with_extra,
};
use crate::session::Session;
use crate::session::resolve::{PictureIndex, load_texture};
use crate::wad::archive::Lump;

/// The fixture session's picture index.
fn index_of(session: &Session) -> PictureIndex<'_> {
    PictureIndex::new(session).expect("the fixture indexes")
}

#[test]
fn textures_composite_definitions_and_report_missing_patches() {
    let texture1 = texture_bytes(vec![definition(
        "TWALL",
        8,
        4,
        vec![place(0, 0, 0), place(1, 4, 0)],
    )]);
    let session = with_extra(vec![
        pnames_lump(&["WALLP1", "GONE"]),
        lump("TEXTURE1", texture1),
    ]);
    let composite = load_texture(&index_of(&session), name("TWALL")).expect("TWALL composes");
    assert_eq!(
        (composite.image.width, composite.image.height),
        (8, 4),
        "the record's size"
    );
    assert_eq!(
        composite.image.get(3, 3),
        Some((0x11, true)),
        "the present patch drew"
    );
    assert_eq!(
        composite.image.get(5, 1),
        Some((0, false)),
        "the missing patch leaves a hole"
    );
    assert_eq!(composite.missing, ["GONE"], "the hole is named");
    assert_eq!(
        composite.problems,
        ["texture TWALL is missing patch GONE".to_owned()],
        "the problem names the texture"
    );
}

#[test]
fn texture2_definitions_win_over_their_texture1_namesakes() {
    let step = doom_patch(2, 2, (0, 0), 0x22);
    let texture1 = texture_bytes(vec![definition("AWIN", 4, 4, vec![place(0, 0, 0)])]);
    let texture2 = texture_bytes(vec![definition("AWIN", 2, 2, vec![place(1, 0, 0)])]);
    let session = with_extra(vec![
        pnames_lump(&["WALLP1", "STEPP1"]),
        lump("STEPP1", step),
        lump("TEXTURE1", texture1),
        lump("TEXTURE2", texture2),
    ]);
    let composite = load_texture(&index_of(&session), name("AWIN")).expect("AWIN composes");
    assert_eq!(
        (composite.image.width, composite.image.height),
        (2, 2),
        "the TEXTURE2 record's size"
    );
    assert_eq!(
        composite.image.get(0, 0),
        Some((0x22, true)),
        "the TEXTURE2 component drew"
    );
    assert!(
        composite.missing.is_empty() && composite.problems.is_empty(),
        "a full composite reports nothing"
    );
}

#[test]
fn without_a_pnames_lump_indices_stay_unnamed_and_missing() {
    let texture1 = texture_bytes(vec![definition("TPTR", 2, 2, vec![place(0, 0, 0)])]);
    let session = with_extra(vec![lump("TEXTURE1", texture1)]);
    let composite = load_texture(&index_of(&session), name("TPTR")).expect("TPTR composes");
    assert_eq!(
        composite.missing,
        ["PNAMES index 0"],
        "the raw index names the hole"
    );
    assert_eq!(
        composite.image.get(1, 1),
        Some((0, false)),
        "the whole texture is a hole"
    );
}

#[test]
fn a_tx_region_patch_serves_a_texture_without_definitions() {
    let session = with_extra(vec![
        Lump::marker(name("TX_START")),
        lump("TXONE", doom_patch(3, 5, (0, 0), 0x33)),
        Lump::marker(name("TX_END")),
    ]);
    let composite = load_texture(&index_of(&session), name("TXONE")).expect("the TX patch answers");
    assert_eq!(
        (composite.image.width, composite.image.height),
        (3, 5),
        "stored as a patch"
    );
    assert_eq!(composite.image.get(0, 0), Some((0x33, true)));
    assert!(
        composite.missing.is_empty() && composite.problems.is_empty(),
        "a plain patch has nothing missing"
    );
}

#[test]
fn unknown_texture_names_are_not_found_and_broken_lumps_are_format_errors() {
    let session = session();
    let missing = load_texture(&index_of(&session), name("NOPE")).expect_err("no such texture");
    assert_eq!(code(&missing), "not_found", "missing texture code");
    assert!(
        missing.to_string().contains("list_textures"),
        "the message names the listing: {missing}"
    );
    let broken = with_extra(vec![
        pnames_lump(&["WALLP1"]),
        lump("TEXTURE1", vec![b'?'; 40]),
    ]);
    let error = load_texture(&index_of(&broken), name("ANY")).expect_err("junk is no texture lump");
    assert_eq!(code(&error), "format_error", "undecodable lump code");
}

#[test]
fn a_component_that_is_no_picture_stays_a_reported_hole() {
    let texture1 = texture_bytes(vec![definition("AWRY", 2, 2, vec![place(0, 0, 0)])]);
    let session = with_extra(vec![
        pnames_lump(&["JUNK"]),
        lump("TEXTURE1", texture1),
        lump("JUNK", vec![b'?'; 40]),
    ]);
    let composite = load_texture(&index_of(&session), name("AWRY")).expect("AWRY composes");
    assert_eq!(
        composite.missing,
        ["JUNK"],
        "the undecodable patch is named"
    );
    assert_eq!(
        composite.problems,
        ["texture AWRY is missing patch JUNK".to_owned()],
        "the problem names the texture"
    );
    assert_eq!(
        composite.image.get(1, 1),
        Some((0, false)),
        "its place stays a hole"
    );
}
