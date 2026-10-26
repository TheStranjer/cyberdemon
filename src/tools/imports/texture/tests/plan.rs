//! The write side of one call: the plan that grows `PNAMES` and the
//! target lump, the layer patch name and the taken texture name that
//! refuse it until `replace`, the one undoable step that lands patch,
//! `PNAMES` and definition together and takes them all back, the explicit
//! target word and the oversized edge, the automatic shape ops and the
//! refusal they name, the theme override that stays outside the history,
//! the composited preview, the order of the argument checks and the two
//! whole builds.

use alloc::collections::BTreeSet;
use std::path::PathBuf;

use crate::gfx::codec::encode_png;
use crate::gfx::image::IndexedImage;
use crate::gfx::ops::{FitMode, ImageOp};
use crate::gfx::palette::Palette;
use crate::gfx::patch::decode_patch;
use crate::session::{Layer, LayerRole, Session};
use crate::state::AppState;
use crate::themes::Role;
use crate::tools::imports::apply_role;
use crate::tools::imports::current_pnames;
use crate::tools::imports::processed::Processed;
use crate::tools::imports::texture::build::{build_picture, composite_png, shape_picture};
use crate::tools::imports::texture::commit::{
    commit_create, land_create, plan_create, texture_size,
};
use crate::tools::imports::texture::size::WallSize;
use crate::tools::imports::texture::{NEXT_STEPS, create_texture_build};
use crate::tools::textures::{TextureSet, WhichLump};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::classify::Namespace;
use crate::wad::textures::write_pnames;

use super::{
    args, code, make_built, name, picture, playpal, pnames_lump, request_for, seed_palette_layer,
    session, texture1_lump, theme_state,
};

#[test]
fn a_plan_over_a_free_name_grows_pnames_and_the_target_lump() {
    let session = session();
    let request = request_for("MYTEX", "MYTEX");
    let plan =
        plan_create(&session, &request, &make_built(64_u32, 128_u32)).expect("the plan runs");
    assert_eq!(plan.lump, "TEXTURE1", "the default target");
    let grown = write_pnames(&["WALLP1".to_owned(), "STEPP1".to_owned(), "MYTEX".to_owned()])
        .expect("valid");
    assert_eq!(
        plan.pnames_bytes.as_ref(),
        Some(&grown),
        "the new patch name grows PNAMES"
    );
}

#[test]
fn a_layer_patch_name_clashes_with_a_hint() {
    let session = session();
    let request = request_for("MYTEX", "WALLP1");
    let error = plan_create(&session, &request, &make_built(64_u32, 64_u32))
        .expect_err("the IWAD patch stands");
    assert_eq!(code(&error), "already_exists");
    let message = error.to_string();
    assert!(
        message.contains("a patch named WALLP1 already exists in the IWAD"),
        "the message names the layer: {message}"
    );
    assert!(
        message.contains("pass patch_name"),
        "and the way out: {message}"
    );
}

/// An `IWAD` layer whose fixture texture set lives in a `patches` region,
/// optionally with a working flats-region lump of the patch's own name.
fn region_iwad_session(working_flat: bool) -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::new(name("PLAYPAL"), playpal()),
                    pnames_lump(),
                    texture1_lump(),
                    Lump::marker(name("P_START")),
                    Lump::new(name("WALLP1"), vec![b'P']),
                    Lump::marker(name("P_END")),
                ],
            },
        ))
        .expect("no batch is open");
    if working_flat {
        session
            .edit("seed-flat", |wad| {
                wad.lumps.push(Lump::marker(name("F_START")));
                wad.lumps.push(Lump::new(name("WALLP1"), vec![1_u8; 4096]));
                wad.lumps.push(Lump::marker(name("F_END")));
                Ok(())
            })
            .expect("plain append");
    }
    session
}

#[test]
fn a_working_flat_named_like_the_layer_patch_still_clashes() {
    // The old skip only asked whether the working archive held the name at
    // all, so a flats-region twin hid the layer's patch and the create
    // overrode it without `replace`. A working lump counts as a twin only
    // in the patches namespace.
    let session = region_iwad_session(true);
    let request = request_for("MYTEX", "WALLP1");
    let error = plan_create(&session, &request, &make_built(64_u32, 64_u32))
        .expect_err("the IWAD region patch stands");
    assert_eq!(code(&error), "already_exists");
    let message = error.to_string();
    assert!(
        message.contains("a patch named WALLP1 already exists in the IWAD"),
        "the message names the layer: {message}"
    );
}

#[test]
fn a_working_region_patch_twin_defers_to_the_placement_refusal() {
    let mut session = region_iwad_session(false);
    session
        .edit("seed-patch", |wad| {
            wad.lumps.push(Lump::marker(name("P_START")));
            wad.lumps.push(Lump::new(name("WALLP1"), vec![b'W']));
            wad.lumps.push(Lump::marker(name("P_END")));
            Ok(())
        })
        .expect("plain region append");
    let request = request_for("MYTEX", "WALLP1");
    let built = make_built(64_u32, 64_u32);
    let error = land_create(&mut session, &request, &built).expect_err("the twin stands");
    assert_eq!(code(&error), "already_exists");
    assert!(
        error.to_string().contains("patches region"),
        "put_lump's own twin message: {error}"
    );
    assert_eq!(
        session.undo_labels(),
        ["seed-patch"],
        "the refused create recorded nothing"
    );
}

#[test]
fn two_lands_after_both_pictures_keep_both_entries_and_definitions() {
    // Both pictures are built (the runs' fetch/decode/quantize half) before
    // either lands, exactly as two concurrent calls interleave; B's whole
    // land then mutates the session between A's picture and A's commit.
    // Because plan and commit share the one `land_create` lock, each plan
    // grows the list as its own commit finds it and nothing is overwritten.
    let mut session = session();
    let request_a = request_for("MYTEXA", "PATCHA");
    let request_b = request_for("MYTEXB", "PATCHB");
    let built = make_built(64_u32, 128_u32);
    let landed_b = land_create(&mut session, &request_b, &built).expect("B lands");
    let landed_a = land_create(&mut session, &request_a, &built).expect("A lands");
    assert_eq!(landed_a.lump, "TEXTURE1", "both defined in TEXTURE1");
    assert_eq!(landed_b.lump, "TEXTURE1");
    assert_eq!(landed_a.role, None, "no theme role asked");
    assert_eq!(
        current_pnames(&session).expect("the grown list decodes"),
        vec![
            "WALLP1".to_owned(),
            "STEPP1".to_owned(),
            "PATCHB".to_owned(),
            "PATCHA".to_owned(),
        ],
        "neither land dropped the other's entry"
    );
    let set = TextureSet::load(&session).expect("both definitions decode");
    assert!(
        set.find("MYTEXA").is_some() && set.find("MYTEXB").is_some(),
        "both textures stand",
    );
    assert_eq!(
        session.undo_labels(),
        ["create_texture MYTEXA", "create_texture MYTEXB"],
        "one undoable step each"
    );
}

#[test]
fn a_taken_texture_name_clashes_until_replace() {
    let session = session();
    let taken = request_for("WALL1", "MYTEX");
    let error =
        plan_create(&session, &taken, &make_built(64_u32, 64_u32)).expect_err("the texture stands");
    assert_eq!(code(&error), "already_exists");
    assert!(
        error.to_string().contains("replace=true"),
        "the message names the way out: {error}"
    );
}

#[test]
fn replace_overrides_the_layer_patch_and_redefines_the_texture() {
    let mut session = session();
    let mut request = request_for("WALL1", "WALLP1");
    request.replace = true;
    let built = make_built(64_u32, 128_u32);
    let plan = plan_create(&session, &request, &built).expect("replace runs");
    assert!(plan.pnames_bytes.is_none(), "WALLP1 is already listed");
    commit_create(&mut session, &request, &built, &plan).expect("the commit lands");
    let set = TextureSet::load(&session).expect("the working override decodes");
    let (def, which) = set.find("WALL1").expect("still defined");
    assert_eq!(which, WhichLump::Texture1, "updated in place");
    assert_eq!(def.patches.len(), 1, "the single patch replaced the tile");
    assert_eq!(
        session.undo_labels(),
        vec!["create_texture WALL1"],
        "one undoable step"
    );
}

#[test]
fn an_undo_step_takes_patch_pnames_and_texture_together() {
    let mut session = session();
    let request = request_for("MYTEX", "MYTEX");
    let built = make_built(64_u32, 128_u32);
    let plan = plan_create(&session, &request, &built).expect("the plan runs");
    commit_create(&mut session, &request, &built, &plan).expect("the commit lands");
    assert!(
        session
            .find(name("MYTEX"), Some(Namespace::Patches))
            .is_some(),
        "the patch serves the merged view"
    );
    session.undo().expect("one step back");
    assert!(
        session
            .find(name("MYTEX"), Some(Namespace::Patches))
            .is_none(),
        "the patch left with its edit"
    );
    assert!(
        TextureSet::load(&session)
            .expect("the fixture lumps decode")
            .find("MYTEX")
            .is_none(),
        "the definition left too"
    );
    assert_eq!(
        current_pnames(&session).expect("the fixture PNAMES decode"),
        vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
        "and the PNAMES append"
    );
}

#[test]
fn an_explicit_target_defines_the_other_lump() {
    let mut session = session();
    let mut request = request_for("MYTEX", "MYTEX");
    request.target = Some("texture2".to_owned());
    let built = make_built(64_u32, 64_u32);
    let plan = plan_create(&session, &request, &built).expect("the plan runs");
    assert_eq!(plan.lump, "TEXTURE2", "case-insensitive target");
    commit_create(&mut session, &request, &built, &plan).expect("the commit lands");
    let set = TextureSet::load(&session).expect("the override decodes");
    assert_eq!(
        set.find("MYTEX").expect("defined").1,
        WhichLump::Texture2,
        "TEXTURE2 holds it"
    );
}

#[test]
fn a_bad_target_word_refuses_the_plan() {
    let session = session();
    let mut request = request_for("MYTEX", "MYTEX");
    request.target = Some("TEXTURE3".to_owned());
    let error =
        plan_create(&session, &request, &make_built(64_u32, 64_u32)).expect_err("no such lump");
    assert_eq!(code(&error), "invalid_argument");
}

#[test]
fn an_oversized_edge_refuses_the_record() {
    let huge = make_built(70_000_u32, 64_u32);
    let error = texture_size(&huge).expect_err("past u16");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("width 70000"),
        "the message names the edge: {error}"
    );
    assert_eq!(
        texture_size(&make_built(64_u32, 128_u32)).expect("a stored size"),
        (64_u16, 128_u16)
    );
}

#[test]
fn shape_appends_the_automatic_ops_to_the_report() {
    let processed = Processed {
        rgba: picture(100_u32, 60_u32),
        report: crate::gfx::ops::OpsReport::default(),
    };
    let size = WallSize {
        width: 128_u32,
        height: 128_u32,
        note: None,
    };
    let shaped = shape_picture(processed, &size, FitMode::Cover, true).expect("both ops run");
    assert_eq!((shaped.rgba.width, shaped.rgba.height), (128_u32, 128_u32));
    let kinds: Vec<&str> = shaped
        .report
        .steps
        .iter()
        .map(|step| step["op"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(kinds, vec!["fit", "make_tileable"], "fit then tileable");
    let same = Processed {
        rgba: picture(128_u32, 128_u32),
        report: crate::gfx::ops::OpsReport::default(),
    };
    let untouched = shape_picture(same, &size, FitMode::Cover, false).expect("no ops needed");
    assert!(
        untouched.report.steps.is_empty(),
        "an equal size adds nothing"
    );
}

#[test]
fn a_one_pixel_axis_refuses_the_tileable_op_naming_the_automatic_step() {
    let processed = Processed {
        rgba: picture(16_u32, 1_u32),
        report: crate::gfx::ops::OpsReport::default(),
    };
    let size = WallSize {
        width: 16_u32,
        height: 1_u32,
        note: None,
    };
    let error = shape_picture(processed, &size, FitMode::Cover, true)
        .expect_err("one row cannot tile vertically");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("automatic resize/tiling step"),
        "the message says which step: {error}"
    );
}

#[test]
fn the_theme_override_lands_in_session_state_outside_history() {
    let mut session = session();
    let mut request = request_for("MYTEX", "MYTEX");
    request.role = Some(Role::Wall);
    let role = apply_role(&mut session, request.role, &request.name);
    assert_eq!(role, Some("wall"), "the role reports back");
    let clean = request_for("MYTEX", "MYTEX");
    assert_eq!(apply_role(&mut session, clean.role, &clean.name), None);
    let (override_name, edits) = theme_state(&session, Role::Wall);
    assert_eq!(
        override_name.as_deref(),
        Some("MYTEX"),
        "the override stands"
    );
    assert_eq!(edits, 0_usize, "no edit recorded");
}

#[test]
fn the_preview_is_the_composited_picture() {
    let palette = Palette::from_playpal(&playpal(), 0).expect("the fixture palette");
    let image =
        IndexedImage::new(2_u32, 1_u32, vec![4_u8, 9], vec![true, true]).expect("2x1 is valid");
    let png = composite_png("MYTEX", &image, &palette).expect("compositing works");
    let back = crate::gfx::codec::decode_image(&png).expect("the preview decodes");
    assert_eq!((back.width, back.height), (2_u32, 1_u32), "the shape");
    assert_eq!(
        back.get(0, 0).map(|[r, g, _, _]| [r, g]),
        Some([4_u8, 251]),
        "the palette colours carried"
    );
}

#[tokio::test]
async fn the_patch_and_the_preview_are_the_one_indexed_picture() {
    // `build_picture` quantizes once and makes both halves from what that
    // pass stored: the patch lump encodes the indexed image and the
    // published preview is the same image composited. Re-quantizing the
    // source for the preview - the duplicate pass this replaced - cannot
    // answer with the plane inside the stored patch, which the default
    // floyd_steinberg diffusion of an off-palette colour scatters.
    let state = AppState::default();
    seed_palette_layer(&state);
    let request = request_for("MYTEX", "MYTEX");
    let png = encode_png(&picture(64_u32, 128_u32), None).expect("a test PNG");
    let fetched = crate::assets::fetch::Fetched {
        bytes: png,
        filename: "art.png".to_owned(),
        mime: Some("image/png".to_owned()),
    };
    let built = build_picture(&state, &request, &fetched).expect("the build runs");
    let palette = Palette::from_playpal(&playpal(), 0).expect("the fixture palette");
    let stored = decode_patch(&built.patch_bytes).expect("the stored patch decodes");
    assert_eq!(
        (stored.image.width, stored.image.height),
        (64_u32, 128_u32),
        "the patch carries the texture size"
    );
    let from_stored = composite_png("MYTEX", &stored.image, &palette).expect("compositing works");
    let distinct = stored.image.indices.iter().collect::<BTreeSet<&u8>>().len();
    assert!(
        distinct > 1_usize,
        "the diffusion scattered {distinct} indices, so a plain second pass could not match them"
    );
    assert_eq!(
        built.preview_png, from_stored,
        "the preview is exactly what the lump stores"
    );
}

#[tokio::test]
async fn bad_arguments_refuse_before_the_source_is_fetched() {
    let state = AppState::default();
    let mut call = args(Some(100_u32), None);
    call.assign_role = Some("floor".to_owned());
    let role_error = create_texture_build(&state, &call)
        .await
        .expect_err("flat role");
    assert_eq!(
        code(&role_error),
        "invalid_argument",
        "the role answers first"
    );
    call.assign_role = None;
    let width_error = create_texture_build(&state, &call)
        .await
        .expect_err("bad width");
    assert_eq!(
        code(&width_error),
        "invalid_argument",
        "the width answers second"
    );
    call.size.width = None;
    let fetch_error = create_texture_build(&state, &call)
        .await
        .expect_err("missing file");
    assert_eq!(code(&fetch_error), "not_found", "only then the fetch");
}

#[tokio::test]
async fn the_full_build_quantizes_sizes_and_reports_a_real_picture() {
    let state = AppState::default();
    seed_palette_layer(&state);
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("ART.png");
    let png = encode_png(&picture(64_u32, 128_u32), None).expect("a test PNG");
    std::fs::write(&path, &png).expect("the test file writes");
    let mut call = args(None, None);
    call.source = path.display().to_string();
    call.shared.ops = vec![ImageOp::Flip {
        horizontal: true,
        vertical: false,
    }];
    let report = create_texture_build(&state, &call)
        .await
        .expect("the create runs");
    assert_eq!(
        (report.width, report.height),
        (64_u32, 128_u32),
        "the kept size"
    );
    assert_eq!(report.lump, "TEXTURE1", "created in TEXTURE1");
    assert_eq!(report.texture, "MYTEX", "the name echoes");
    assert_eq!(report.patch, "MYTEX", "the patch follows the name");
    assert_eq!(report.steps.len(), 1, "only the user's flip ran");
    assert!(report.notes.is_empty(), "a kept size adds no note");
    assert!(report.warnings.is_empty(), "the picture is opaque");
    assert_eq!(
        report.next_steps,
        NEXT_STEPS.as_slice(),
        "the documented next steps"
    );
}

#[tokio::test]
async fn an_odd_sized_picture_falls_back_to_the_square_with_a_note() {
    let state = AppState::default();
    seed_palette_layer(&state);
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("ODD.png");
    let png = encode_png(&picture(100_u32, 60_u32), None).expect("a test PNG");
    std::fs::write(&path, &png).expect("the test file writes");
    let mut call = args(None, None);
    call.source = path.display().to_string();
    let report = create_texture_build(&state, &call)
        .await
        .expect("the create runs");
    assert_eq!(
        (report.width, report.height),
        (128_u32, 128_u32),
        "the fallback"
    );
    assert_eq!(
        report.notes,
        vec!["resized to 128x128; pass width/height to choose".to_owned()],
        "the documented note"
    );
    let kinds: Vec<&str> = report
        .steps
        .iter()
        .map(|step| step["op"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(kinds, vec!["fit"], "the automatic fit step reports");
}
