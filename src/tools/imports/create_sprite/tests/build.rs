//! The picture side: the automatic chain measured on a fixture drawing,
//! the fully-background refusal, the one edit that writes every named lump
//! and undoes together, the working twin that refuses it, the rotation
//! warnings read for every lump and the first name that labels the edit.

use crate::gfx::image::RgbaImage;
use crate::session::Session;
use crate::tools::imports::create_sprite::build::{
    BuiltSprite, build_sprite, collect_warnings, commit_sprite, first_name,
};
use crate::tools::imports::create_sprite::parse_sprite_names;
use crate::wad::archive::Lump;
use crate::wad::classify::Namespace;

use super::{by_name, by_sprite, code, fetched, name, request, state, white_with_red};

#[tokio::test]
async fn build_removes_the_background_and_anchors_the_feet() {
    let state = state();
    let call = request(&by_name("TROOA1"));
    let built =
        build_sprite(&state, &call, &fetched(&white_with_red())).expect("the pipeline runs");
    assert_eq!(
        (built.width, built.height),
        (10_u32, 20_u32),
        "trimmed to the figure"
    );
    assert_eq!(built.left_offset, 5_i32, "half the final width");
    assert_eq!(built.top_offset, 20_i32, "the full final height");
    assert_eq!(
        built.transparent_pixels, 0_usize,
        "the trim cropped to the opaque figure"
    );
    let steps: Vec<&str> = built
        .steps
        .iter()
        .map(|step| step["op"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        steps,
        vec!["remove_background", "trim"],
        "the automatic chain"
    );
    assert_eq!(built.notes, Vec::<String>::new(), "nothing scaled");
}

#[tokio::test]
async fn a_fully_background_picture_refuses_with_the_hint() {
    let state = state();
    let call = request(&by_name("TROOA1"));
    let side = 8_u32;
    let count = usize::try_from(side)
        .expect("test side")
        .checked_mul(usize::try_from(side).expect("test side"))
        .expect("test pixels")
        .checked_mul(4_usize)
        .expect("test bytes");
    let white = RgbaImage::new(side, side, vec![255_u8; count]).expect("8x8 is valid");
    let error = build_sprite(&state, &call, &fetched(&white))
        .map(|_| ())
        .expect_err("the whole picture is background");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error
            .to_string()
            .contains("the whole image was removed as background"),
        "the hint: {error}"
    );
}

#[test]
fn one_edit_writes_every_named_lump_and_undoes_together() {
    let mut session = Session::default();
    let mut call = by_sprite("BOBA");
    call.frames = Some(
        ["A", "B"]
            .iter()
            .map(|letter| (*letter).to_owned())
            .collect(),
    );
    call.trim = Some(false);
    call.remove_background = Some(false);
    let built = BuiltSprite {
        data: vec![0_u8; 12],
        preview_png: Vec::new(),
        width: 2,
        height: 3,
        left_offset: 1,
        top_offset: 3,
        steps: Vec::new(),
        colors_used: 1,
        transparent_pixels: 4,
        notes: Vec::new(),
    };
    let request = request(&call);
    commit_sprite(&mut session, &request, &built).expect("the commit lands");
    assert_eq!(
        session.undo_labels(),
        vec!["create_sprite BOBAA0"],
        "one step labelled by the first lump"
    );
    for text in ["BOBAA0", "BOBAB0"] {
        assert!(
            session.find(name(text), Some(Namespace::Sprites)).is_some(),
            "{text} serves the merged view"
        );
    }
    session.undo().expect("one step back");
    assert!(
        session
            .find(name("BOBAA0"), Some(Namespace::Sprites))
            .is_none(),
        "both lumps left with the edit"
    );
}

#[test]
fn a_working_twin_refuses_the_edit_without_writing_anything() {
    let mut session = Session::default();
    session
        .edit("seed", |wad| {
            wad.lumps.push(Lump::marker(name("S_START")));
            wad.lumps.push(Lump::new(name("BOBAB0"), vec![0_u8; 8]));
            wad.lumps.push(Lump::marker(name("S_END")));
            Ok(())
        })
        .expect("plain insert");
    let mut call = by_sprite("BOBA");
    call.frames = Some(
        ["A", "B"]
            .iter()
            .map(|letter| (*letter).to_owned())
            .collect(),
    );
    let built = BuiltSprite {
        data: vec![0_u8; 12],
        preview_png: Vec::new(),
        width: 2,
        height: 3,
        left_offset: 1,
        top_offset: 3,
        steps: Vec::new(),
        colors_used: 1,
        transparent_pixels: 4,
        notes: Vec::new(),
    };
    let request = request(&call);
    let error = commit_sprite(&mut session, &request, &built).expect_err("the second twin stands");
    assert_eq!(code(&error), "already_exists");
    assert!(session.undo_labels().len() == 1_usize, "the seed edit only");
    assert!(
        session
            .find(name("BOBAA0"), Some(Namespace::Sprites))
            .is_none(),
        "the first lump was not written either"
    );
}

#[test]
fn the_rotation_warnings_are_read_for_every_lump() {
    let mut session = Session::default();
    session
        .edit("seed", |wad| {
            wad.lumps.push(Lump::marker(name("S_START")));
            wad.lumps.push(Lump::new(name("BOBAA2A8"), vec![0_u8; 8]));
            wad.lumps.push(Lump::new(name("BOBAB2A8"), vec![0_u8; 8]));
            wad.lumps.push(Lump::marker(name("S_END")));
            Ok(())
        })
        .expect("plain insert");
    let mut call = by_sprite("BOBA");
    call.frames = Some(
        ["A", "B"]
            .iter()
            .map(|letter| (*letter).to_owned())
            .collect(),
    );
    call.rotation = Some(2_u8);
    let warnings = collect_warnings(&session, &request(&call).names).expect("the read runs");
    assert_eq!(
        warnings.len(),
        2_usize,
        "one per covered frame: {warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .all(|warning| warning.contains("rotation 2")),
        "both name the rotation: {warnings:?}"
    );
}

#[test]
fn the_first_name_labels_the_edit_and_premiers_the_preview() {
    let mut call = by_sprite("BOBA");
    call.frames = Some(vec!["C".to_owned(), "A".to_owned()]);
    let names = parse_sprite_names(&call).expect("valid");
    assert_eq!(first_name(&names).expect("first"), name("BOBAC0"));
    assert_eq!(
        code(&first_name(&[]).expect_err("nothing")),
        "internal",
        "an empty naming is an internal contradiction"
    );
}
