//! The picture side: the automatic fit and tiling steps at each square
//! side, the collision rule over a working twin and an `IWAD` layer, the
//! one-edit commit with its undo, the opaque quantization and the decoded
//! preview, the whole run's echo, the theme override outside the history
//! and the refusal that writes nothing.

use alloc::collections::BTreeSet;

use crate::assets::fetch::Fetched;
use crate::error::CyberError;
use crate::gfx::codec::{decode_image, encode_png};
use crate::gfx::flat::decode_flat;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::{FitMode, dither::Dither};
use crate::gfx::palette::Palette;
use crate::session::{Layer, LayerRole, Session};
use crate::state::AppState;
use crate::themes::Role;
use crate::tools::imports::apply_role;
use crate::tools::imports::create_flat::build::{
    BuiltFlat, build_flat, check_flat_clash, commit_flat, create_flat_build, land_flat, shape_flat,
};
use crate::tools::imports::create_flat::parse_create_flat;
use crate::tools::imports::processed::ProcessParams;
use crate::tools::imports::processed::{Processed, process};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::classify::Namespace;

use super::{args, code, name, png};

/// The step kinds of a shaped report, in order.
fn step_kinds(shaped: &Processed) -> Vec<&str> {
    shaped
        .report
        .steps
        .iter()
        .map(|step| step["op"].as_str().unwrap_or_default())
        .collect()
}

/// A processed picture straight from an encoded test PNG.
fn processed(width: u32, height: u32) -> Processed {
    process(
        &png(width, height),
        &ProcessParams {
            ops: Vec::new(),
            dither: None,
            palette: None,
        },
    )
    .expect("no ops")
}

#[test]
fn shape_fits_to_the_square_and_appends_the_tileable_step() {
    let shaped =
        shape_flat(processed(100, 80), FitMode::Cover, true, 64_u32).expect("both ops run");
    assert_eq!((shaped.rgba.width, shaped.rgba.height), (64_u32, 64_u32));
    assert_eq!(
        step_kinds(&shaped),
        vec!["fit".to_owned(), "make_tileable".to_owned()],
        "fit then tileable"
    );
    let flat = shape_flat(processed(64, 64), FitMode::Stretch, false, 64_u32).expect("fit runs");
    assert_eq!(
        step_kinds(&flat),
        vec!["fit".to_owned()],
        "the fit always runs"
    );
}

#[test]
fn shape_fits_to_the_bigger_squares_a_zdoom_port_reads() {
    for side in [128_u32, 256] {
        let shaped =
            shape_flat(processed(100, 80), FitMode::Cover, true, side).expect("both ops run");
        assert_eq!(
            (shaped.rgba.width, shaped.rgba.height),
            (side, side),
            "the requested square lands"
        );
    }
}

/// A session whose working archive holds a flats region with `FLOOR1`.
fn working_session() -> Session {
    let mut session = Session::default();
    session
        .edit("seed", |wad| {
            wad.lumps.push(Lump::marker(name("F_START")));
            wad.lumps.push(Lump::new(name("FLOOR1"), vec![1_u8; 4096]));
            wad.lumps.push(Lump::marker(name("F_END")));
            Ok(())
        })
        .expect("plain insert");
    session
}

/// A session on an `IWAD` layer holding a flats region with `FLOOR1`.
fn layer_session() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            "base.wad".into(),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::marker(name("F_START")),
                    Lump::new(name("FLOOR1"), vec![1_u8; 4096]),
                    Lump::marker(name("F_END")),
                ],
            },
        ))
        .expect("no batch is open");
    session
}

#[test]
fn a_free_flat_name_reports_no_override() {
    let session = working_session();
    assert!(
        !check_flat_clash(&session, name("NEWFLAT"), false).expect("free"),
        "nothing was there"
    );
}

#[test]
fn a_working_twin_refuses_until_replace() {
    let session = working_session();
    let error = check_flat_clash(&session, name("FLOOR1"), false).expect_err("the twin stands");
    assert_eq!(code(&error), "already_exists");
    assert!(
        error.to_string().contains("working archive"),
        "the message names the archive: {error}"
    );
    assert!(
        check_flat_clash(&session, name("FLOOR1"), true).expect("replace"),
        "override"
    );
}

#[test]
fn a_layer_flat_refuses_with_the_everywhere_hint() {
    let session = layer_session();
    let error = check_flat_clash(&session, name("FLOOR1"), false).expect_err("the IWAD stands");
    assert_eq!(code(&error), "already_exists");
    let message = error.to_string();
    assert!(
        message.contains("this would override FLOOR1 everywhere it is used"),
        "the hint names the consequence: {message}"
    );
    assert!(
        message.contains("pass replace=true to do that or choose another name"),
        "and the way out: {message}"
    );
    assert!(message.contains("the IWAD"), "and the layer: {message}");
    assert!(
        check_flat_clash(&session, name("FLOOR1"), true).expect("replace"),
        "override"
    );
}

#[test]
fn one_edit_places_the_flat_and_undo_takes_it_away() {
    let mut session = Session::default();
    session
        .edit("seed", |wad| {
            wad.lumps.push(Lump::new(name("PLAYPAL"), vec![0_u8; 768]));
            Ok(())
        })
        .expect("plain insert");
    let request = parse_create_flat(&args()).expect("valid");
    let built = BuiltFlat {
        data: vec![7_u8; 4096],
        preview_png: Vec::new(),
        steps: Vec::new(),
        colors_used: 1,
        max_color_error: 0,
        warnings: Vec::new(),
    };
    commit_flat(&mut session, &request, &built).expect("the commit lands");
    assert_eq!(
        session.undo_labels(),
        vec!["create_flat MYFLAT", "seed"],
        "one undoable step"
    );
    assert!(
        session
            .find(name("MYFLAT"), Some(Namespace::Flats))
            .is_some(),
        "the flats region took the lump"
    );
    session.undo().expect("one step back");
    assert!(
        session
            .find(name("MYFLAT"), Some(Namespace::Flats))
            .is_none(),
        "the flat left with its edit"
    );
}

/// Fixture palette bytes: two entries of `(i, 255 - i, i)`.
fn playpal() -> Vec<u8> {
    (0_u32..512_u32)
        .map(|position| {
            u8::try_from(position.checked_rem(256_u32).expect("non-zero"))
                .expect("remainders fit u8")
        })
        .flat_map(|index| [index, 255_u8.wrapping_sub(index), index])
        .collect()
}

/// Appends a `PLAYPAL` lump to a session's working archive.
fn seed_playpal(session: &mut Session) -> Result<(), CyberError> {
    session.edit("seed", |wad| {
        wad.lumps.push(Lump::new(name("PLAYPAL"), playpal()));
        Ok(())
    })
}

/// A state whose session carries a `PLAYPAL`.
fn state() -> AppState {
    let state = AppState::default();
    state
        .with_session(seed_playpal)
        .expect("the seed edit runs");
    state
}

/// A fetched test picture over the fixture raster.
fn fetched(width: u32, height: u32) -> Fetched {
    Fetched {
        bytes: png(width, height),
        filename: "art.png".to_owned(),
        mime: Some("image/png".to_owned()),
    }
}

#[tokio::test]
async fn build_quantizes_opaquely_and_reads_the_flat_back() {
    let state = state();
    let request = parse_create_flat(&args()).expect("valid");
    let built = build_flat(&state, &request, &fetched(100, 80)).expect("the build runs");
    assert_eq!(built.data.len(), 4096_usize, "a vanilla 64x64 flat");
    assert_eq!(
        decode_flat(&built.data).expect("the bytes decode").width,
        64_u32,
        "at the square shape"
    );
    assert_eq!(built.steps.len(), 2_usize, "fit and make_tileable");
    assert!(built.warnings.is_empty(), "the picture is opaque");
}

/// A `size x size` fetched picture of one uniform grey: off every fixture
/// palette colour, so the default diffusion really scatters indices.
fn grey_fetched(size: u32, shade: u8) -> Fetched {
    let pixels: Vec<u8> = (0..size.saturating_mul(size))
        .flat_map(|_| [shade, shade, shade, 255_u8])
        .collect();
    let image = RgbaImage::new(size, size, pixels).expect("square is valid");
    Fetched {
        bytes: encode_png(&image, None).expect("test PNG encodes"),
        filename: "grey.png".to_owned(),
        mime: Some("image/png".to_owned()),
    }
}

#[tokio::test]
async fn the_preview_paints_the_index_plane_the_flat_stored() {
    // One quantization feeds both halves: `build_flat` keeps the single
    // indexed image it measured and hands it to the flat encoder and to the
    // preview alike. A preview built from its own pass over the source - the
    // way a second quantize would do it - could not answer with the stored
    // plane, which is what the diffusion of an off-palette grey makes
    // visible here: the plane is mixed, not one nearest colour.
    let state = state();
    let mut call = args();
    call.tileable = false;
    let request = parse_create_flat(&call).expect("valid");
    let built =
        build_flat(&state, &request, &grey_fetched(64_u32, 200_u8)).expect("the build runs");
    let palette = Palette::from_playpal(&playpal(), 0).expect("the fixture palette");
    let stored = decode_flat(&built.data).expect("the stored flat decodes");
    let distinct = stored.indices.iter().collect::<BTreeSet<&u8>>().len();
    assert!(
        distinct > 1_usize,
        "the default floyd_steinberg diffusion mixed {distinct} indices"
    );
    let preview = decode_image(&built.preview_png).expect("the preview decodes");
    assert_eq!((preview.width, preview.height), (64_u32, 64_u32));
    assert_eq!(
        preview.pixels,
        stored.to_rgba(&palette).pixels,
        "every preview pixel is the palette colour of the index the lump stores"
    );
}

#[tokio::test]
async fn build_at_a_bigger_side_stores_that_square_and_warns() {
    let state = state();
    let mut call = args();
    call.size = Some(256_u32);
    let request = parse_create_flat(&call).expect("a hi-res side is valid");
    let built = build_flat(&state, &request, &fetched(100, 80)).expect("the build runs");
    assert_eq!(built.data.len(), 0x1_0000_usize, "a 256x256 flat");
    assert_eq!(
        decode_flat(&built.data).expect("the bytes decode").width,
        256_u32,
        "at the requested square"
    );
    assert!(
        built
            .warnings
            .iter()
            .any(|warning| warning.contains("256x256") && warning.contains("vanilla")),
        "the size warns: {:?}",
        built.warnings
    );
}

#[tokio::test]
async fn the_run_echoes_the_dither_and_size_it_used() {
    let state = state();
    let dir = tempfile::tempdir().expect("a scratch directory");
    let file = dir.path().join("art.png");
    std::fs::write(&file, png(100, 80)).expect("test file writes");
    let mut call = args();
    call.source = file.display().to_string();
    call.size = Some(128_u32);
    call.shared.dither = Some(Dither::Ordered);
    let report = create_flat_build(&state, &call)
        .await
        .expect("the run lands");
    assert_eq!(report.size, 128_u32, "the echo carries the side");
    assert_eq!(report.dither, Dither::Ordered, "the echo carries the mode");
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("128x128")),
        "the bigger side warns: {:?}",
        report.warnings
    );
}

/// The session's `Floor` override and its undo depth.
fn floor_theme(session: &Session) -> (Option<String>, usize) {
    (
        session.texture_theme().overrides.get(&Role::Floor).cloned(),
        session.undo_labels().len(),
    )
}

#[test]
fn the_theme_override_lands_in_session_state_outside_history() {
    let mut session = Session::default();
    let mut request = parse_create_flat(&args()).expect("valid");
    request.role = Some(Role::Floor);
    let role = apply_role(&mut session, request.role, &request.name.to_string());
    assert_eq!(role, Some("floor"), "the role reports back");
    let (texture, undo_len) = floor_theme(&session);
    assert_eq!(texture.as_deref(), Some("MYFLAT"), "the theme points at it");
    assert_eq!(undo_len, 0_usize, "the override records no edit");
    let plain = parse_create_flat(&args()).expect("valid");
    assert_eq!(
        apply_role(&mut session, plain.role, &plain.name.to_string()),
        None,
        "no role, no word"
    );
}

/// A built flat with dummy bytes for the land tests.
fn built_flat() -> BuiltFlat {
    BuiltFlat {
        data: vec![7_u8; 4096],
        preview_png: Vec::new(),
        steps: Vec::new(),
        colors_used: 1,
        max_color_error: 0,
        warnings: Vec::new(),
    }
}

#[test]
fn one_land_sees_the_flat_the_previous_land_placed() {
    // Two create-flat calls over one name interleave the way concurrency
    // puts them: both pictures are built (the runs' outside-the-lock half)
    // first, the first lands whole, and only then does the second land.
    // The clash check runs inside the same lock as its commit, so the
    // second land reads the first's lump: with `replace` it reports the
    // override it could only guess at from a stale pre-fetch scan.
    let mut session = Session::default();
    let built = built_flat();
    let first = parse_create_flat(&args()).expect("valid");
    let landed_a = land_flat(&mut session, &first, &built).expect("A lands");
    assert!(
        !landed_a.overrides_existing,
        "the name was free when A landed"
    );
    let mut second = parse_create_flat(&args()).expect("valid");
    second.replace = true;
    let landed_b = land_flat(&mut session, &second, &built).expect("B overrides");
    assert!(
        landed_b.overrides_existing,
        "B's check reads A's landed flat from the one lock"
    );
    assert_eq!(
        session.undo_labels(),
        vec![
            "create_flat MYFLAT".to_owned(),
            "create_flat MYFLAT".to_owned()
        ],
        "two undoable steps"
    );
    let mut clash = parse_create_flat(&args()).expect("valid");
    clash.role = Some(Role::Ceiling);
    let refused = land_flat(&mut session, &clash, &built).expect_err("C meets a twin");
    assert_eq!(code(&refused), "already_exists", "the clash check moved in");
    let (theme, edits) = floor_theme(&session);
    assert!(
        theme.is_none() && edits == 2_usize,
        "C wrote nothing anywhere: {theme:?} {edits}"
    );
}

/// The session's undo labels.
fn undo_labels(state: &AppState) -> Vec<String> {
    state
        .with_session(|session| {
            Ok(session
                .undo_labels()
                .iter()
                .copied()
                .map(str::to_owned)
                .collect())
        })
        .expect("the read runs")
}

/// A session on a `PLAYPAL`-carrying `IWAD` layer with a `FLOOR1` flat.
fn layer_session_with_palette() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            "base.wad".into(),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::new(name("PLAYPAL"), playpal()),
                    Lump::marker(name("F_START")),
                    Lump::new(name("FLOOR1"), vec![1_u8; 4096]),
                    Lump::marker(name("F_END")),
                ],
            },
        ))
        .expect("no batch is open");
    session
}

#[tokio::test]
async fn a_layer_flat_refuses_the_whole_run_without_writing() {
    let state = AppState::default();
    state
        .with_session(|session| Ok(core::mem::replace(session, layer_session_with_palette())))
        .expect("the seed runs");
    let dir = tempfile::tempdir().expect("a scratch directory");
    let file = dir.path().join("art.png");
    std::fs::write(&file, png(20, 20)).expect("test file writes");
    let mut call = args();
    call.source = file.display().to_string();
    call.name = "FLOOR1".to_owned();
    let error = create_flat_build(&state, &call)
        .await
        .expect_err("the IWAD flat stands");
    assert_eq!(code(&error), "already_exists");
    assert!(undo_labels(&state).is_empty(), "nothing was written");
}
