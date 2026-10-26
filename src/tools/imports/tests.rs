//! Unit tests of the imports group: the shared name and region helpers the
//! patch and raw imports parse with, the `PNAMES` plan both the imports
//! and the patch lifecycle tools build their writes from and that the
//! patch import's single-lock `land_patch` step recomputes at the commit,
//! the one working-archive write with its overwrite rule and region
//! wording, the [`apply_extra`] fold every create's automatic ops run
//! through and the [`apply_role`] step both creates share, and the flat
//! import's argument parsing, shape fitting and alpha counting.

use super::flat::{
    ImportFlatParams, ResizeChoice, fit_flat_shape, parse_flat, parse_flat_size, parse_resize,
    transparent_alpha_pixels,
};
use super::image_pipeline::{ImportReport, Patched};
use super::patch::{ImportRawParams, PatchReport, land_patch};
use super::{
    LumpWrite, apply_extra, apply_role, chosen, current_pnames, namespace_names, parse_namespace,
    pnames_plan, position_of, put_lump, region_text, region_write,
};
use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::dither::Dither;
use crate::gfx::ops::{FitMode, ImageOp, OpsReport, apply_ops};
use crate::session::Session;
use crate::themes::Role;
use crate::tools::imports::processed::Processed;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;
use crate::wad::textures::write_pnames;

/// The error code of a [`CyberError`], for compact asserts.
fn code(error: &CyberError) -> &str {
    error.code()
}

/// A parsed test name.
fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A `PNAMES` payload listing `texts`.
fn pnames_bytes(texts: &[&str]) -> Vec<u8> {
    write_pnames(
        &texts
            .iter()
            .map(|text| (*text).to_owned())
            .collect::<Vec<String>>(),
    )
    .expect("test names are valid")
}

#[test]
fn chosen_takes_the_explicit_name_or_derives_one() {
    assert_eq!(
        chosen(Some("given"), "art.png").expect("explicit"),
        name("GIVEN")
    );
    assert_eq!(chosen(None, "art.png").expect("derived"), name("ART"));
    assert_eq!(
        code(&chosen(Some("too long"), "art.png").expect_err("bad name")),
        "invalid_argument"
    );
    let error = chosen(None, "###.png").expect_err("nothing derivable");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("name"),
        "names the parameter: {error}"
    );
}

#[test]
fn namespaces_parse_case_insensitively_and_refuse_else() {
    assert_eq!(parse_namespace(None).expect("absent"), None);
    assert_eq!(
        parse_namespace(Some("Patches")).expect("region"),
        Some(Namespace::Patches)
    );
    let error = parse_namespace(Some("nope")).expect_err("not a region");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        namespace_names()
            .split(", ")
            .all(|region| error.to_string().contains(region)),
        "the message lists every region: {error}"
    );
}

#[test]
fn position_matching_ignores_case() {
    let list = ["WALLP1".to_owned(), "STEPP1".to_owned()];
    assert_eq!(position_of(&list, "stepp1"), Some(1));
    assert_eq!(position_of(&list, "GONE"), None);
}

#[test]
fn current_pnames_reads_the_merged_lump_or_nothing() {
    let mut session = Session::default();
    assert!(
        current_pnames(&session).expect("no lump").is_empty(),
        "without a PNAMES lump the list is empty"
    );
    seed_pnames_session(&mut session, &["WALLP1"]);
    assert_eq!(
        current_pnames(&session).expect("the lump lists"),
        vec!["WALLP1".to_owned()]
    );
}

/// Appends a `PNAMES` lump listing `texts` to a session's working archive.
fn seed_pnames_session(session: &mut Session, texts: &[&str]) {
    let bytes = pnames_bytes(texts);
    session
        .edit("seed", move |wad| {
            wad.lumps.push(Lump::new(name("PNAMES"), bytes));
            Ok(())
        })
        .expect("plain append");
}

#[test]
fn pnames_plans_keep_existing_entries_and_append_new_ones() {
    let mut session = Session::default();
    seed_pnames_session(&mut session, &["WALLP1", "STEPP1"]);
    assert!(
        pnames_plan(&session, name("NEW"), Some(false))
            .expect("off stays off")
            .is_none(),
        "add_to_pnames false writes nothing"
    );
    let kept = pnames_plan(&session, name("wallp1"), None)
        .expect("existing")
        .expect("planned");
    assert_eq!(
        kept.index, 0_usize,
        "the existing entry answers with its index"
    );
    assert!(kept.bytes.is_none(), "the list does not change");
    let added = pnames_plan(&session, name("NEWART"), None)
        .expect("appended")
        .expect("planned");
    assert_eq!(added.index, 2_usize, "the new entry lands last");
    assert!(added.bytes.is_some(), "the grown list gets written");
}

/// A staged patch import: fixed bytes and the pipeline facts a landing
/// reports, no real picture behind it.
fn patched(text: &str) -> Patched {
    Patched {
        name: name(text),
        data: vec![1_u8, 2, 3],
        report: ImportReport {
            width: 2_u32,
            height: 3_u32,
            left_offset: 0_i32,
            top_offset: 0_i32,
            transparent_pixels: 0_usize,
            colors_used: 1_usize,
            max_color_error: 0_i32,
        },
    }
}

/// One [`PatchReport`]'s `name` and `pnames_index`, read through the
/// result's own serialization.
fn landed_report(report: &PatchReport) -> (String, Option<usize>) {
    let value = serde_json::to_value(report).expect("a report serializes");
    (
        value["name"].as_str().unwrap_or_default().to_owned(),
        value["pnames_index"]
            .as_u64()
            .map(|index| usize::try_from(index).expect("test indices fit usize")),
    )
}

#[test]
fn two_patch_lands_after_both_stages_keep_both_pnames_entries() {
    // The runs stage their pictures - fetch, decode, quantize - outside
    // every lock and land them through one `with_session` closure. This
    // test freezes that seam: both pictures are staged first, then the
    // second lands - mutating the session between the first call's staging
    // and its commit - and only then the first lands. Because plan and
    // commit share the one [`land_patch`] lock, each plan reads the list
    // exactly as its commit finds it, and both entries survive.
    let mut session = Session::default();
    seed_pnames_session(&mut session, &["WALLP1", "STEPP1"]);
    let first = patched("MYA");
    let second = patched("MYB");
    let landed_b = landed_report(&land_patch(&mut session, second, None, false).expect("B lands"));
    let landed_a = landed_report(&land_patch(&mut session, first, None, false).expect("A lands"));
    assert_eq!(
        landed_b,
        ("MYB".to_owned(), Some(2_usize)),
        "B appended first"
    );
    assert_eq!(landed_a, ("MYA".to_owned(), Some(3_usize)), "A after B");
    assert_eq!(
        current_pnames(&session).expect("the list decodes"),
        vec![
            "WALLP1".to_owned(),
            "STEPP1".to_owned(),
            "MYB".to_owned(),
            "MYA".to_owned(),
        ],
        "neither land overwrote the other's entry"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["import_patch MYA", "import_patch MYB", "seed"],
        "two separate undoable steps"
    );
}

/// The raw arguments' wire shape deserializes with the documented flags.
#[test]
fn raw_arguments_deserialize() {
    let args: ImportRawParams = serde_json::from_value(serde_json::json!({
        "source": "/x/y.png",
        "namespace": "patches",
        "replace": true,
    }))
    .expect("the wire form");
    assert_eq!(args.namespace.as_deref(), Some("patches"));
    assert!(args.replace, "the flag carries");
}

/// Flat arguments over `source` with everything else unset.
fn flat_params(source: &str) -> ImportFlatParams {
    ImportFlatParams {
        source: source.to_owned(),
        name: None,
        palette: None,
        resize: None,
        size: None,
        dither: None,
        replace: false,
    }
}

/// A `width` by `height` test image whose every pixel carries `alpha`.
fn rgba_image(width: u32, height: u32, alpha: u8) -> RgbaImage {
    let count = usize::try_from(width)
        .expect("test width")
        .checked_mul(usize::try_from(height).expect("test height"))
        .expect("test pixels fit");
    let mut pixels: Vec<u8> = Vec::new();
    for _pixel in 0..count {
        pixels.extend_from_slice(&[1_u8, 2, 3, alpha]);
    }
    RgbaImage::new(width, height, pixels).expect("test shape is valid")
}

#[test]
fn parse_flat_applies_defaults_and_validates_the_name() {
    let parsed = parse_flat(&flat_params("/x/y.png")).expect("a source is enough");
    assert_eq!(parsed.source, "/x/y.png", "the source carries");
    assert_eq!(parsed.name, None, "no name was given");
    assert_eq!(parsed.palette, 0_usize, "the normal palette by default");
    assert_eq!(parsed.resize, ResizeChoice::Keep, "no resizing by default");
    assert_eq!(parsed.size, 64_u32, "the vanilla side by default");
    assert_eq!(
        parsed.dither,
        Dither::FloydSteinberg,
        "the same dither default process_image documents"
    );
    assert!(!parsed.replace, "no silent overwrite");
    let mut args = flat_params("/x/y.png");
    args.name = Some("too long for wad".to_owned());
    assert_eq!(
        code(&parse_flat(&args).expect_err("bad name")),
        "invalid_argument"
    );
}

#[test]
fn parse_flat_keeps_an_explicit_dither_mode() {
    for mode in [Dither::None, Dither::Ordered, Dither::FloydSteinberg] {
        let mut args = flat_params("/x/y.png");
        args.dither = Some(mode);
        assert_eq!(
            parse_flat(&args).expect("a parsed mode carries").dither,
            mode,
            "the mode passes through untouched"
        );
    }
}

#[test]
fn resize_words_parse_and_name_the_rejections() {
    assert_eq!(parse_resize(None).expect("absent"), ResizeChoice::Keep);
    assert_eq!(
        parse_resize(Some("none")).expect("keep"),
        ResizeChoice::Keep
    );
    assert_eq!(
        parse_resize(Some("nearest")).expect("scale"),
        ResizeChoice::Nearest
    );
    let error = parse_resize(Some("linear")).expect_err("not a word");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("\"nearest\""),
        "the message lists the words: {error}"
    );
}

#[test]
fn flat_sizes_take_the_three_sides_and_refuse_else() {
    assert_eq!(parse_flat_size(None).expect("default"), 64_u32);
    for side in [64_u32, 128, 256] {
        assert_eq!(parse_flat_size(Some(side)).expect("a square"), side);
    }
    let error = parse_flat_size(Some(32_u32)).expect_err("no 32 side");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("64, 128, 256"),
        "the message lists the sides: {error}"
    );
}

#[test]
fn fit_flat_shape_keeps_supported_shapes_and_scales_the_rest() {
    for (width, height) in [(64_u32, 64_u32), (64, 128), (128, 128), (256, 256)] {
        let picture = rgba_image(width, height, 255_u8);
        let (kept, resized) =
            fit_flat_shape(&picture, ResizeChoice::Nearest, 256_u32).expect("a flat shape");
        assert!(!resized, "{width}x{height} is kept as it is");
        assert_eq!((kept.width, kept.height), (width, height));
    }
    let odd = rgba_image(100_u32, 50_u32, 255_u8);
    let (scaled, resized) =
        fit_flat_shape(&odd, ResizeChoice::Nearest, 128_u32).expect("nearest scales");
    assert!(resized, "the odd shape was scaled");
    assert_eq!((scaled.width, scaled.height), (128_u32, 128_u32));
}

#[test]
fn an_unsupported_shape_without_resize_names_the_way_out() {
    let odd = rgba_image(100_u32, 50_u32, 255_u8);
    let error = fit_flat_shape(&odd, ResizeChoice::Keep, 64_u32).expect_err("keep refuses");
    assert_eq!(code(&error), "invalid_argument");
    let message = error.to_string();
    assert!(message.contains("256x256"), "names the shapes: {message}");
    assert!(
        message.contains("resize=\"nearest\""),
        "names the option: {message}"
    );
}

#[test]
fn alpha_pixels_below_the_cutoff_are_counted() {
    let pixels = vec![1_u8, 2, 3, 0, 1, 2, 3, 128, 1, 2, 3, 127, 1, 2, 3, 255];
    let img = RgbaImage::new(4_u32, 1_u32, pixels).expect("4x1 is a valid shape");
    assert_eq!(
        transparent_alpha_pixels(&img),
        2_usize,
        "0 and 127 are below 128, 128 and 255 are not"
    );
}

/// The flat arguments' wire shape deserializes with the documented flags.
#[test]
fn flat_arguments_deserialize() {
    let args: ImportFlatParams = serde_json::from_value(serde_json::json!({
        "source": "/x/y.png",
        "name": "FLAT",
        "palette": 3_u64,
        "resize": "nearest",
        "size": 128_u64,
        "dither": "ordered",
        "replace": true,
    }))
    .expect("the wire form");
    assert_eq!(args.name.as_deref(), Some("FLAT"));
    assert_eq!(args.palette, Some(3_usize));
    assert_eq!(args.resize.as_deref(), Some("nearest"));
    assert_eq!(args.size, Some(128_u32));
    assert_eq!(args.dither, Some(Dither::Ordered));
    assert!(args.replace, "the flag carries");
}

/// The `op` words of a report, in order.
fn step_ops(report: &OpsReport) -> Vec<&str> {
    report
        .steps
        .iter()
        .map(|step| step["op"].as_str().unwrap_or_default())
        .collect()
}

/// A `width` x `height` picture with `ops` already run over it: what a
/// tool hands its automatic step to [`apply_extra`].
fn processed_with(width: u32, height: u32, ops: &[ImageOp]) -> Processed {
    let decoded = rgba_image(width, height, 255_u8);
    let (rgba, report) = apply_ops(decoded, ops).expect("test ops run");
    Processed { rgba, report }
}

#[test]
fn apply_extra_without_ops_is_the_identity() {
    let picture = processed_with(4_u32, 4_u32, &[]);
    let same = apply_extra(picture, &[]).expect("nothing runs");
    assert_eq!((same.rgba.width, same.rgba.height), (4_u32, 4_u32));
    assert!(same.report.steps.is_empty(), "and the report stands");
}

#[test]
fn apply_extra_appends_the_automatic_steps_after_the_caller_s() {
    let crop = [ImageOp::Crop {
        x: 1,
        y: 1,
        width: 2,
        height: 2,
    }];
    let first = processed_with(4_u32, 4_u32, &crop);
    assert_eq!(
        step_ops(&first.report),
        vec!["crop"],
        "the caller's own step"
    );
    let shaped = apply_extra(
        first,
        &[ImageOp::Fit {
            width: 8,
            height: 8,
            mode: FitMode::Stretch,
            filter: crate::gfx::ops::Filter::Nearest,
            background: None,
        }],
    )
    .expect("the automatic step runs");
    assert_eq!(
        step_ops(&shaped.report),
        vec!["crop", "fit"],
        "the automatic step is appended, never prepended"
    );
    assert_eq!((shaped.rgba.width, shaped.rgba.height), (8_u32, 8_u32));
}

#[test]
fn apply_extra_passes_a_failing_step_through_untouched() {
    let error = apply_extra(
        processed_with(4_u32, 4_u32, &[]),
        &[ImageOp::Crop {
            x: 0,
            y: 0,
            width: 99,
            height: 1,
        }],
    )
    .expect_err("the crop leaves the picture");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("ops[0]"),
        "the shared fold keeps the op index for the caller to reword: {error}"
    );
}

#[test]
fn apply_role_records_the_name_outside_the_history() {
    let mut session = Session::default();
    assert_eq!(
        apply_role(&mut session, None, "MYTEX"),
        None,
        "a call that asked for no role answers nothing"
    );
    assert!(
        session.texture_theme().overrides.is_empty(),
        "and nothing was written"
    );
    assert_eq!(
        apply_role(&mut session, Some(Role::Switch), "MYTEX"),
        Some("switch"),
        "the role's wire word comes back"
    );
    assert_eq!(
        session
            .texture_theme()
            .overrides
            .get(&Role::Switch)
            .map(String::as_str),
        Some("MYTEX"),
        "the theme points at the name it was handed"
    );
    assert!(session.undo_labels().is_empty(), "no undo step records it");
}

#[test]
fn region_write_carries_the_payload_and_the_overwrite_rule() {
    let write = region_write(name("MYFLAT"), b"payload", Namespace::Flats, true);
    assert_eq!(write.name, name("MYFLAT"), "the name to store under");
    assert_eq!(write.data, b"payload", "the bytes to store");
    assert_eq!(
        write.namespace,
        Some(Namespace::Flats),
        "the region it lands in"
    );
    assert!(write.replace, "the rule the call asked for");
    let guarded = region_write(name("MYFLAT"), b"payload", Namespace::Flats, false);
    assert!(!guarded.replace, "without it a working twin refuses");
}

#[test]
fn region_text_names_the_region_or_the_global_area() {
    assert_eq!(
        region_text(Some(Namespace::Sprites)),
        "the sprites region",
        "a region reads as its own"
    );
    assert_eq!(
        region_text(None),
        "the global area",
        "and a loose lump has no region"
    );
}

/// The directory names of an archive, in order.
fn directory(wad: &Wad) -> Vec<String> {
    wad.lumps.iter().map(|lump| lump.name.to_string()).collect()
}

/// The payload of the lump at `index`.
fn payload(wad: &Wad, index: usize) -> Vec<u8> {
    wad.lumps
        .get(index)
        .map(|lump| lump.data.to_vec())
        .expect("test indices exist")
}

/// A `Pwad` carrying one loose lump named `text`.
fn wad_with(text: &str, payload: &[u8]) -> Wad {
    Wad {
        kind: WadKind::Pwad,
        lumps: vec![Lump::new(name(text), payload.to_vec())],
    }
}

#[test]
fn put_lump_creates_the_region_and_refuses_twins_until_replace() {
    let mut wad = wad_with("PLAYPAL", &[1_u8]);
    let fresh = region_write(name("MYPATCH"), b"one", Namespace::Patches, false);
    let first = put_lump(&mut wad, &fresh).expect("the region takes the lump");
    assert_eq!(first, 2_usize, "after the created P_START");
    assert_eq!(
        directory(&wad),
        vec!["PLAYPAL", "P_START", "MYPATCH", "P_END"]
    );
    let twin = region_write(name("MYPATCH"), b"two", Namespace::Patches, false);
    let clash = put_lump(&mut wad, &twin).expect_err("the twin stands");
    assert_eq!(code(&clash), "already_exists");
    assert!(
        clash.to_string().contains("replace=true"),
        "the message names the way out: {clash}"
    );
    assert!(
        clash.to_string().contains("the patches region"),
        "and where the twin stands: {clash}"
    );
    let over = region_write(name("MYPATCH"), b"two", Namespace::Patches, true);
    let again = put_lump(&mut wad, &over).expect("replace overwrites");
    assert_eq!(again, first, "the copy keeps its place");
    assert_eq!(payload(&wad, again), b"two".to_vec(), "in place");
    assert_eq!(
        directory(&wad),
        vec!["PLAYPAL", "P_START", "MYPATCH", "P_END"]
    );
}

#[test]
fn put_lump_scopes_twins_to_their_region_and_the_global_area_too() {
    let mut wad = wad_with("ART", &[1_u8]);
    let loose = LumpWrite {
        name: name("ART"),
        data: b"2",
        namespace: None,
        replace: false,
    };
    let refused = put_lump(&mut wad, &loose).expect_err("the global twin clashes");
    assert_eq!(code(&refused), "already_exists");
    assert!(
        refused.to_string().contains("global area"),
        "no region is named as such: {refused}"
    );
    let in_region = region_write(name("ART"), b"3", Namespace::Patches, false);
    let region =
        put_lump(&mut wad, &in_region).expect("a region copy beside a loose one is not a twin");
    assert_eq!(directory(&wad), vec!["ART", "P_START", "ART", "P_END"]);
    assert_eq!(payload(&wad, region), b"3".to_vec());
}
