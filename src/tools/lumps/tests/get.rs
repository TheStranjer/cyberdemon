//! The `get_lump` read: the accepted `format` names, the `hex` window and
//! its cap, the format `auto` picks, the `text` and `hex` bodies, the
//! published asset's name and the parsed options' defaults, beside the
//! `extract_lump` path checks that ride on the same selector.

use std::path::Path;

use crate::tools::lumps::extract::{parse_target, path_text, refuse_existing};
use crate::tools::lumps::get::{
    Applied, GetLumpParams, Requested, asset_naming, decide, hex_body, parse_format,
    parse_get_options, parse_hex_length, text_body,
};
use crate::wad::classify::{LumpKind, Namespace};

use super::{code, name, row};

#[test]
fn get_format_defaults_to_auto_and_accepts_every_name() {
    assert_eq!(parse_format(None).expect("default"), Requested::Auto);
    for (text, wanted) in [
        ("auto", Requested::Auto),
        ("INFO", Requested::Info),
        ("Text", Requested::Text),
        ("hex", Requested::Hex),
        ("url", Requested::Url),
    ] {
        assert_eq!(
            parse_format(Some(text)).expect(text),
            wanted,
            "format {text}"
        );
    }
    let error = parse_format(Some("base64")).expect_err("not a format");
    assert_eq!(code(&error), "invalid_argument", "bad format code");
    assert!(
        error.to_string().contains("\"auto\""),
        "the hint names the default: {error}"
    );
}

#[test]
fn hex_length_defaults_and_stays_under_the_cap() {
    assert_eq!(
        parse_hex_length(None).expect("default"),
        256,
        "the documented default"
    );
    assert_eq!(
        parse_hex_length(Some(4_096)).expect("cap"),
        4_096,
        "the cap"
    );
    assert_eq!(
        parse_hex_length(Some(0)).expect("empty"),
        0,
        "zero is empty"
    );
    let error = parse_hex_length(Some(4_097)).expect_err("over the cap");
    assert_eq!(code(&error), "invalid_argument", "over-cap code");
    assert!(
        error.to_string().contains("4096"),
        "the message names the cap: {error}"
    );
}

#[test]
fn auto_picks_text_for_text_like_kinds_and_info_for_empty() {
    for kind in [
        LumpKind::Text,
        LumpKind::Dehacked,
        LumpKind::Decorate,
        LumpKind::Zscript,
        LumpKind::Mapinfo,
    ] {
        let row = row("ANY", kind, None, b"words");
        assert_eq!(
            decide(Requested::Auto, &row),
            Applied::Text,
            "auto over {kind:?}"
        );
    }
    let empty = row("F_START", LumpKind::Marker, None, b"");
    assert_eq!(
        decide(Requested::Auto, &empty),
        Applied::Info,
        "auto over empty"
    );
    let flat = row("FLOOR1", LumpKind::Flat, Some(Namespace::Flats), b"\0\x01");
    assert_eq!(
        decide(Requested::Auto, &flat),
        Applied::Url,
        "auto over a flat"
    );
    for (requested, applied) in [
        (Requested::Info, Applied::Info),
        (Requested::Text, Applied::Text),
        (Requested::Hex, Applied::Hex),
        (Requested::Url, Applied::Url),
    ] {
        assert_eq!(decide(requested, &flat), applied, "explicit {requested:?}");
    }
}

#[test]
fn text_bodies_report_lossiness_and_truncation() {
    let clean = text_body(b"Thing 1 (Troop)", 100_000);
    assert_eq!(
        clean.text.as_deref(),
        Some("Thing 1 (Troop)"),
        "clean bytes survive"
    );
    assert_eq!(clean.lossy, Some(false), "clean bytes are not lossy");
    assert_eq!(clean.truncated, Some(false), "well under the cap");
    assert_eq!(clean.hex, None, "a text body carries no hex fields");
    let ragged = text_body(b"ab\xe9\xffcd", 100_000);
    assert_eq!(ragged.lossy, Some(true), "invalid sequences were replaced");
    assert_eq!(ragged.truncated, Some(false), "short input");
    assert_eq!(
        ragged.text.as_deref(),
        Some("ab\u{FFFD}\u{FFFD}cd"),
        "the replacement characters"
    );
    let cut = text_body(b"Thing 1 (Troop)", 5);
    assert_eq!(
        cut.text.as_deref(),
        Some("Thing"),
        "cut at the character cap"
    );
    assert_eq!(cut.truncated, Some(true), "the cap was hit");
}

#[test]
fn hex_windows_format_clip_and_refuse_a_start_past_the_end() {
    let head = hex_body(b"Mus\x1Arest", name("D_E1M1"), 0, 4).expect("a fitting window");
    assert_eq!(
        head.hex.as_deref(),
        Some("4D 75 73 1A"),
        "space-separated uppercase bytes"
    );
    assert_eq!(
        (head.start, head.length),
        (Some(0), Some(4)),
        "echoed window"
    );
    let clipped = hex_body(b"abc", name("X"), 1, 4_096).expect("clipped window");
    assert_eq!(
        clipped.hex.as_deref(),
        Some("62 63"),
        "the window clips at the payload end"
    );
    assert_eq!(
        clipped.length,
        Some(2),
        "the reported length is the real one"
    );
    let tail = hex_body(b"abc", name("X"), 3, 16).expect("an empty tail window");
    assert_eq!(tail.hex.as_deref(), Some(""), "a start at the end is empty");
    assert_eq!(tail.length, Some(0), "no bytes");
    let error = hex_body(b"abc", name("X"), 4, 16).expect_err("past the end");
    assert_eq!(code(&error), "invalid_argument", "start code");
    assert!(
        error
            .to_string()
            .contains("4 is past the end of lump X (3 bytes)"),
        "the message shows the bounds: {error}"
    );
}

#[test]
fn asset_names_follow_the_kind_table() {
    let cases = [
        (LumpKind::Png, "png"),
        (LumpKind::Jpeg, "jpg"),
        (LumpKind::Gif, "gif"),
        (LumpKind::Wav, "wav"),
        (LumpKind::Ogg, "ogg"),
        (LumpKind::Mp3, "mp3"),
        (LumpKind::Flac, "flac"),
        (LumpKind::Midi, "mid"),
        (LumpKind::Mus, "mus"),
        (LumpKind::Text, "txt"),
        (LumpKind::Dehacked, "txt"),
        (LumpKind::Mapinfo, "txt"),
        (LumpKind::Palette, "lmp"),
        (LumpKind::Flat, "lmp"),
        (LumpKind::Unknown, "lmp"),
    ];
    for (kind, extension) in cases {
        let row = row("LOMB", kind, None, b"x");
        let (filename, mime) = asset_naming(&row);
        assert_eq!(
            filename,
            format!("LOMB.{extension}"),
            "file name over {kind:?}"
        );
        assert_eq!(
            mime,
            crate::assets::store::mime_for_extension(extension),
            "mime"
        );
    }
}

#[test]
fn extract_targets_must_be_absolute() {
    let path = parse_target("/tmp/opencode/nowhere.lmp").expect("an absolute path");
    assert!(path.is_absolute(), "kept as given");
    let error = parse_target("out.bin").expect_err("a relative path");
    assert_eq!(code(&error), "invalid_argument", "relative code");
    assert!(
        error.to_string().contains("not absolute"),
        "the message says what to pass: {error}"
    );
}

#[test]
fn extract_refuses_an_existing_target_unless_overwriting() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let target = dir.path().join("lump.bin");
    refuse_existing(&target, false).expect("nothing stands there");
    std::fs::write(&target, b"old").expect("the scratch file lands");
    let error = refuse_existing(&target, false).expect_err("the file exists");
    assert_eq!(code(&error), "already_exists", "existing target code");
    assert!(
        error.to_string().contains("overwrite true"),
        "the message names the fix: {error}"
    );
    refuse_existing(&target, true).expect("overwrite was allowed");
}

#[test]
fn path_text_renders_a_path_for_reports() {
    assert_eq!(
        path_text(Path::new("/tmp/opencode/x.lmp")),
        "/tmp/opencode/x.lmp"
    );
}

#[test]
fn get_options_default_every_window() {
    let args = GetLumpParams {
        selector: crate::tools::lumps::select::LumpSelector {
            name: "X".to_owned(),
            scope: None,
            namespace: None,
            occurrence: None,
        },
        format: None,
        start: None,
        length: None,
        max_chars: None,
    };
    let opts = parse_get_options(&args).expect("defaults parse");
    assert_eq!(opts.requested, Requested::Auto, "the default format");
    assert_eq!(
        (opts.start, opts.length),
        (0, 256),
        "the default hex window"
    );
    assert_eq!(opts.max_chars, 100_000, "the default text cap");
}
