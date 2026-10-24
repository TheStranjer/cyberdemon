//! Unit tests of the palette group: the shared `PLAYPAL` reader and the
//! swatch grid, then the `set_palette` half - the vanilla tint ramps, the
//! source decoding, the entry writes and the commits that copy-on-write
//! into the working archive as one undoable edit.
//!
//! The fixtures are built with the library's own types: a session whose
//! `IWAD` layer holds the ramp palette whose colour `i` is
//! `(i, 255 - i, i)`.

use std::path::PathBuf;

use super::set::{
    Payload, Request, SetPaletteParams, SourceArg, apply_payload, commit_palette, interpret_bytes,
    palette_from_image, pick_source, rebuild_tints, set_palette_run, tint_channel, tint_of,
};
use super::{OPAQUE, PALETTE_BYTES, PALETTE_COLORS, VANILLA_PALETTES, load_palette, swatch_image};
use crate::gfx::codec::encode_png;
use crate::gfx::colormap;
use crate::gfx::image::RgbaImage;
use crate::gfx::palette::Palette;
use crate::session::{Layer, LayerRole, Session};
use crate::state::AppState;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;
use alloc::collections::BTreeSet;

/// A valid parsed name for test data.
fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// `palettes` copies of the fixture palette (colour `i` is
/// `(i, 255 - i, i)`) as raw `PLAYPAL` bytes.
fn fixture_bytes(palettes: usize) -> Vec<u8> {
    let mut first = Vec::new();
    for index in 0_u8..=255 {
        first.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index]);
    }
    first.repeat(palettes)
}

/// The fixture palette.
fn fixture_palette() -> Palette {
    Palette::from_playpal(&fixture_bytes(1), 0).expect("one palette was built")
}

/// A session whose `IWAD` layer holds the fixture `PLAYPAL` (and a
/// zeroed `COLORMAP` when `with_colormap` is set).
fn session_with(palettes: usize, with_colormap: bool) -> Session {
    let mut lumps = vec![Lump::new(name("PLAYPAL"), fixture_bytes(palettes))];
    if with_colormap {
        lumps.push(Lump::new(name("COLORMAP"), vec![0_u8; colormap::MIN_BYTES]));
    }
    let mut session = Session::default();
    session.set_iwad(Layer::new(
        PathBuf::from("base.wad"),
        LayerRole::Iwad,
        Wad {
            kind: WadKind::Iwad,
            lumps,
        },
    ));
    session
}

/// A 16x16 image of the fixture palette, fully opaque.
fn fixture_grid() -> RgbaImage {
    let mut pixels = Vec::new();
    for index in 0_u8..=255 {
        pixels.extend_from_slice(&[index, 255_u8.wrapping_sub(index), index, OPAQUE]);
    }
    RgbaImage::new(16, 16, pixels).expect("256 RGBA8 pixels fit a 16x16 image")
}

/// The fixture palette with entry 5 changed.
fn changed_palette() -> Palette {
    let mut colors = fixture_palette().0;
    colors[5] = [1_u8, 2_u8, 3_u8];
    Palette::from(colors)
}

/// A colours payload of the changed fixture palette.
fn changed_payload() -> Payload {
    Payload::Colors(Box::new(changed_palette()))
}

/// A commit request over a payload, with `(tints, colormap)` flags.
fn request(payload: Payload, index: usize, flags: (bool, bool)) -> Request {
    Request {
        payload,
        index,
        tints: flags.0,
        colormap: flags.1,
    }
}

#[test]
fn load_palette_reads_the_named_entry_of_the_merged_playpal() {
    let session = session_with(2, false);
    let palette = load_palette(&session, 1).expect("palette 1 exists");
    assert_eq!(
        palette.color(3),
        Some([3_u8, 252_u8, 3_u8]),
        "fixture colour 3"
    );
    assert_eq!(palette, fixture_palette(), "the copies hold the fixture");
}

#[test]
fn load_palette_without_a_playpal_points_at_open_iwad() {
    let session = Session::default();
    let error = load_palette(&session, 0).expect_err("no PLAYPAL is loaded");
    assert_eq!(error.code(), "not_found", "missing lump code");
    assert!(
        error.to_string().contains("open_iwad"),
        "the hint names the tool: {error}"
    );
}

#[test]
fn load_palette_refuses_an_index_past_the_lump() {
    let session = session_with(1, false);
    let error = load_palette(&session, 1).expect_err("one palette has no index 1");
    assert_eq!(error.code(), "not_found", "out-of-range code");
    assert!(
        error.to_string().contains("holds 1 palettes"),
        "the message states the count: {error}"
    );
}

#[test]
fn the_swatch_grids_the_palette_in_sixteen_pixel_cells() {
    let image = swatch_image(&fixture_palette()).expect("a 256x256 swatch");
    assert_eq!((image.width, image.height), (256, 256), "the shape");
    assert_eq!(
        image.get(15, 15),
        Some([0_u8, 255_u8, 0_u8, OPAQUE]),
        "cell 0 covers the first sixteen pixels"
    );
    assert_eq!(
        image.get(16, 0),
        Some([1_u8, 254_u8, 1_u8, OPAQUE]),
        "cell 1 starts at x 16"
    );
    assert_eq!(
        image.get(255, 255),
        Some([255_u8, 0_u8, 255_u8, OPAQUE]),
        "the last cell carries the last colour"
    );
}

#[test]
fn tint_targets_follow_the_vanilla_ramps() {
    assert_eq!(
        tint_of(1),
        ([255_u8, 0_u8, 0_u8], 1, 9),
        "first damage tint"
    );
    assert_eq!(tint_of(8), ([255_u8, 0_u8, 0_u8], 8, 9), "last damage tint");
    assert_eq!(
        tint_of(9),
        ([215_u8, 186_u8, 69_u8], 1, 8),
        "first pickup tint"
    );
    assert_eq!(
        tint_of(12),
        ([215_u8, 186_u8, 69_u8], 4, 8),
        "last pickup tint"
    );
    assert_eq!(
        tint_of(13),
        ([0_u8, 255_u8, 0_u8], 1, 8),
        "radiation-suit tint"
    );
}

#[test]
fn tint_channel_moves_colours_toward_the_target_rounded() {
    assert_eq!(tint_channel(0, 255, 1, 9), 28, "25.5 rounds to 28");
    assert_eq!(tint_channel(255, 0, 8, 9), 28, "255 - 226.7 rounds to 28");
    assert_eq!(tint_channel(128, 0, 1, 2), 64, "an exact half");
    assert_eq!(tint_channel(100, 100, 1, 8), 100, "no delta stays put");
}

#[test]
fn a_source_image_must_hold_exactly_256_pixels() {
    let wide = RgbaImage::new(2, 1, vec![0_u8; 8]).expect("a 2x1 image");
    let error = palette_from_image(&wide).expect_err("two pixels are not a palette");
    assert_eq!(error.code(), "invalid_argument", "pixel count code");
    assert!(
        error.to_string().contains("has 2 pixels"),
        "the message states the count: {error}"
    );
    let palette = palette_from_image(&fixture_grid()).expect("256 pixels read row-major");
    assert_eq!(palette, fixture_palette(), "row-major colour order");
}

#[test]
fn interpret_bytes_sorts_images_from_raw_palettes_and_refuses_junk() {
    let png = encode_png(&fixture_grid(), None).expect("the grid encodes");
    let from_image = interpret_bytes(&png).expect("a 256-pixel PNG is a palette");
    assert!(
        matches!(from_image, Payload::Colors(palette) if *palette == fixture_palette()),
        "the image branch yields the fixture palette"
    );
    let raw = fixture_bytes(2);
    let from_raw = interpret_bytes(&raw).expect("1536 bytes are two palettes");
    assert!(
        matches!(&from_raw, Payload::Playpal(bytes) if *bytes == raw),
        "the raw branch keeps every byte"
    );
    let odd = vec![7_u8; 10];
    assert_eq!(
        interpret_bytes(&odd)
            .expect_err("ten bytes are no palette")
            .code(),
        "invalid_argument",
        "odd length code"
    );
    assert_eq!(
        interpret_bytes(b"")
            .expect_err("empty is no palette")
            .code(),
        "invalid_argument",
        "empty code"
    );
}

#[test]
fn colour_payloads_replace_and_append_and_refuse_the_future() {
    let base = fixture_bytes(1);
    let mut touched = BTreeSet::new();
    let same = apply_payload(
        &base,
        Payload::Colors(Box::new(fixture_palette())),
        0,
        &mut touched,
    )
    .expect("entry 0 is replaced");
    assert_eq!(same, base, "the same palette re-written lands identically");
    assert_eq!(
        touched.into_iter().collect::<Vec<usize>>(),
        vec![0],
        "entry 0"
    );

    let mut appended = BTreeSet::new();
    let grown = apply_payload(&base, changed_payload(), 1, &mut appended).expect("index 1 appends");
    assert_eq!(
        grown.chunks_exact(PALETTE_BYTES).count(),
        2,
        "one palette more"
    );

    let mut refused = BTreeSet::new();
    let error = apply_payload(&base, changed_payload(), 2, &mut refused)
        .expect_err("index 2 is past the single palette");
    assert_eq!(error.code(), "invalid_argument", "future index code");
    assert!(refused.is_empty(), "the refusal touched nothing");
}

#[test]
fn a_raw_payload_replaces_every_entry_it_carries() {
    let mut replaced = BTreeSet::new();
    let data = apply_payload(
        &fixture_bytes(14),
        Payload::Playpal(fixture_bytes(3)),
        0,
        &mut replaced,
    )
    .expect("the whole lump is replaced");
    assert_eq!(
        data.chunks_exact(PALETTE_BYTES).count(),
        3,
        "three palettes stand in"
    );
    assert_eq!(
        replaced.into_iter().collect::<Vec<usize>>(),
        vec![0, 1, 2],
        "every entry was written"
    );
}

#[test]
fn tints_rebuild_the_vanilla_ramp_and_grow_short_data() {
    let mut data = fixture_bytes(1);
    let mut replaced = BTreeSet::new();
    rebuild_tints(&mut data, &mut replaced).expect("13 tints from palette 0");
    assert_eq!(
        data.chunks_exact(PALETTE_BYTES).count(),
        VANILLA_PALETTES,
        "grown to the vanilla 14"
    );
    assert_eq!(replaced.len(), 13, "entries 1..=13 rebuilt");
    let red = Palette::from_playpal(&data, 1).expect("tint 1 exists");
    assert_eq!(
        red.color(0),
        Some([28_u8, 227_u8, 0_u8]),
        "the fixture green tinted 1/9 toward red"
    );
    let suit = Palette::from_playpal(&data, 13).expect("tint 13 exists");
    assert_eq!(
        suit.color(255),
        Some([223_u8, 32_u8, 223_u8]),
        "magenta lifted 1/8 toward green"
    );
    rebuild_tints(&mut data, &mut replaced).expect("idempotent on full data");
    assert_eq!(
        data.chunks_exact(PALETTE_BYTES).count(),
        VANILLA_PALETTES,
        "no further growth"
    );
}

#[test]
fn a_commit_copies_on_write_undoes_as_one_edit_and_leaves_the_layer() {
    let mut session = session_with(1, false);
    let layer_before = session
        .layers()
        .first()
        .expect("the IWAD layer")
        .wad
        .lumps
        .len();
    let report = commit_palette(&mut session, request(changed_payload(), 0, (false, false)))
        .expect("the edit lands");
    assert_eq!(report.palettes, 1, "the count stands");
    assert_eq!(report.replaced, vec![0], "entry 0 replaced");
    assert!(!report.colormap_regenerated, "no COLORMAP was asked for");
    assert_eq!(
        session.undo_labels(),
        vec!["set_palette"],
        "one labelled edit"
    );
    assert!(session.is_dirty(), "unsaved work");
    let working: Vec<String> = session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect();
    assert_eq!(working, vec!["PLAYPAL"], "the override was appended");
    assert_eq!(
        load_palette(&session, 0).expect("merged"),
        changed_palette(),
        "the override won"
    );
    assert_eq!(
        session
            .layers()
            .first()
            .expect("the IWAD layer")
            .wad
            .lumps
            .len(),
        layer_before,
        "the layer itself never changed"
    );
    assert!(
        load_palette(&session, 1).is_err(),
        "no tint was added without a flag"
    );
    session.undo().expect("undo steps back over the edit");
    assert_eq!(
        load_palette(&session, 0).expect("restored"),
        fixture_palette(),
        "the fixture is back"
    );
    assert!(session.working().lumps.is_empty(), "the override is gone");
}

#[test]
fn a_full_commit_grows_the_tints_and_writes_both_lumps() {
    let mut session = session_with(1, true);
    let report = commit_palette(
        &mut session,
        request(
            Payload::Colors(Box::new(fixture_palette())),
            0,
            (true, true),
        ),
    )
    .expect("palette, tints and maps land together");
    assert_eq!(report.palettes, VANILLA_PALETTES, "grown to the vanilla 14");
    assert!(report.colormap_regenerated, "COLORMAP was rebuilt");
    assert_eq!(
        report.replaced.first().copied(),
        Some(0),
        "entry 0 written first"
    );
    assert_eq!(
        report.replaced.last().copied(),
        Some(13),
        "up to the suit tint"
    );
    let working: Vec<String> = session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect();
    assert_eq!(
        working,
        vec!["PLAYPAL", "COLORMAP"],
        "both overrides appended"
    );
    let maps = session
        .find(name("COLORMAP"), None)
        .expect("the merged view serves it");
    assert_eq!(
        maps.lump.data.len(),
        colormap::MIN_BYTES,
        "34 tables of 256"
    );
}

#[test]
fn a_commit_creates_a_missing_playpal_at_index_zero() {
    let mut session = Session::default();
    let report = commit_palette(&mut session, request(changed_payload(), 0, (false, false)))
        .expect("the first palette is created");
    assert_eq!(report.palettes, 1, "one palette stands");
    assert_eq!(
        load_palette(&session, 0).expect("readable"),
        changed_palette(),
        "its colours"
    );
}

#[test]
fn zero_or_two_sources_are_refused() {
    let error = pick_source(&SetPaletteParams::default()).expect_err("no source given");
    assert_eq!(error.code(), "invalid_argument", "no source code");
    assert!(
        error.to_string().contains("exactly one"),
        "the message says so: {error}"
    );
    let both = SetPaletteParams {
        colors: Some(vec!["#000000".to_owned(); PALETTE_COLORS]),
        source: Some("/palette.png".to_owned()),
        ..SetPaletteParams::default()
    };
    assert_eq!(
        pick_source(&both).expect_err("two sources given").code(),
        "invalid_argument",
        "both sources code"
    );
    let only = SetPaletteParams {
        colors: Some(vec!["#000000".to_owned(); PALETTE_COLORS]),
        ..SetPaletteParams::default()
    };
    assert!(
        matches!(pick_source(&only), Ok(SourceArg::Colors(list)) if list.len() == PALETTE_COLORS),
        "one source is accepted"
    );
}

#[tokio::test]
async fn set_palette_run_reports_the_fetch_failure_of_a_missing_file() {
    let state = AppState::default();
    let args = SetPaletteParams {
        source: Some("/definitely-not-here/cyberdemon-palette.png".to_owned()),
        ..SetPaletteParams::default()
    };
    let error = set_palette_run(&state, &args)
        .await
        .expect_err("no such file exists");
    assert_eq!(error.code(), "not_found", "missing file code");
}
