//! Header tests: the eight header bytes read on their own, the rewrite of
//! the two drawing offsets and the refusals for bytes that carry no patch
//! header, plus the encoder's own dimension limits.

use super::{build, full, patch_lump, post};
use crate::error::CyberError;
use crate::gfx::image::IndexedImage;
use crate::gfx::patch::{Patch, decode_patch, encode_patch, patch_header, set_patch_offsets};

#[test]
fn patch_header_reads_the_eight_header_bytes_alone() {
    let bytes = patch_lump(2, 3, (-3, -2), &[&[0xFF_u8], &[0xFF_u8]]);
    let header = patch_header(&bytes).expect("a built patch starts with a header");
    assert_eq!((header.width, header.height), (2, 3), "dimensions");
    assert_eq!(
        (header.left_offset, header.top_offset),
        (-3, -2),
        "drawing offsets, negatives included"
    );
    let head_only: Vec<u8> = bytes.into_iter().take(8).collect();
    assert!(
        patch_header(&head_only).is_some(),
        "no column table is needed for the header"
    );
}

#[test]
fn patch_header_rejects_bytes_that_carry_no_patch_header() {
    assert!(
        patch_header(&[1_u8, 0, 0]).is_none(),
        "shorter than a header"
    );
    assert!(
        patch_header(&patch_lump(0, 1, (0, 0), &[])).is_none(),
        "zero width"
    );
    assert!(
        patch_header(&patch_lump(4_097, 1, (0, 0), &[&[0xFF_u8]])).is_none(),
        "width past the cap"
    );
    assert!(patch_header(b"png?").is_none(), "four bytes of text");
}

#[test]
fn set_patch_offsets_rewrites_only_the_two_header_fields() {
    let column0 = [post(0, &[5, 7]), vec![0xFF_u8]].concat();
    let bytes = patch_lump(2, 3, (-3, -2), &[&column0, &[0xFF_u8]]);
    let out = set_patch_offsets(&bytes, 5, -7).expect("a real header takes new offsets");
    assert_eq!(&out[..4], &bytes[..4], "width and height stay");
    assert_ne!(&out[4..8], &bytes[4..8], "the offset pair changed");
    assert_eq!(
        &out[8..],
        &bytes[8..],
        "table and bodies stay byte-identical"
    );
    let header = patch_header(&out).expect("the rewrite is still a header");
    assert_eq!(
        (header.left_offset, header.top_offset),
        (5, -7),
        "new offsets"
    );
    assert_eq!((header.width, header.height), (2, 3), "old dimensions");
    let before = decode_patch(&bytes).expect("the original decodes");
    let after = decode_patch(&out).expect("the rewrite round-trips through decode");
    assert_eq!(after.image, before.image, "the picture is untouched");
}

#[test]
fn set_patch_offsets_refuses_non_patches_and_offsets_past_the_header() {
    let short = set_patch_offsets(b"png?", 1, 1).expect_err("four bytes are no header");
    assert_eq!(short.code(), "invalid_argument", "non-patch code");
    let zero = set_patch_offsets(&patch_lump(0, 1, (0, 0), &[]), 1, 1)
        .expect_err("zero width answers no header");
    assert_eq!(zero.code(), "invalid_argument", "corrupt header code");
    let bytes = patch_lump(1, 1, (0, 0), &[&[0xFF_u8]]);
    let far = set_patch_offsets(&bytes, 40000, 0).expect_err("past the i16 range");
    assert_eq!(far.code(), "invalid_argument", "far left code");
    assert!(
        far.to_string().contains("left_offset 40000 does not fit"),
        "the message names the field: {far}"
    );
    let low = set_patch_offsets(&bytes, 0, -40000).expect_err("past the i16 range");
    assert!(
        low.to_string().contains("top_offset -40000"),
        "the other field is named too: {low}"
    );
}

#[test]
fn rejects_malformed_headers() {
    let zero = decode_patch(&patch_lump(0, 1, (0, 0), &[])).expect_err("zero width is no patch");
    assert_eq!(zero.code(), "format_error", "zero width code");
    assert!(
        zero.to_string().contains("width"),
        "names the field: {zero}"
    );
    let wide =
        decode_patch(&patch_lump(4_097, 1, (0, 0), &[&[0xFF_u8]])).expect_err("width past the cap");
    assert_eq!(wide.code(), "format_error", "oversized width code");
    let tall = decode_patch(&patch_lump(1, 4_097, (0, 0), &[&[0xFF_u8]]))
        .expect_err("height past the cap");
    assert_eq!(tall.code(), "format_error", "oversized height code");
    let stub = decode_patch(&[1_u8, 0, 0]).expect_err("three bytes are no header");
    assert_eq!(stub.code(), "format_error", "truncated header code");
}

#[test]
fn rejects_column_offsets_outside_the_lump() {
    let mut bytes = patch_lump(1, 1, (0, 0), &[&[0xFF_u8]]);
    bytes[8..12].copy_from_slice(&999_u32.to_le_bytes());
    let error = decode_patch(&bytes).expect_err("the offset points nowhere");
    assert_eq!(error.code(), "format_error", "bad offset code");
    assert!(
        error.to_string().contains("column 0"),
        "names the column: {error}"
    );
    let no_table = decode_patch(&[1_u8, 0, 1, 0, 0, 0, 0, 0]).expect_err("no table at all");
    assert_eq!(no_table.code(), "format_error", "missing table code");
    let half = decode_patch(&patch_lump(2, 1, (0, 0), &[&[0xFF_u8]]))
        .expect_err("width 2 needs two offsets");
    assert_eq!(half.code(), "format_error", "truncated table code");
}

#[test]
fn rejects_truncated_posts_and_missing_terminators() {
    let cut = patch_lump(1, 2, (0, 0), &[&[0_u8, 5, 0, 1, 2]]);
    let error = decode_patch(&cut).expect_err("the post promises five bytes");
    assert_eq!(error.code(), "format_error", "truncated post code");
    let unterminated = patch_lump(1, 1, (0, 0), &[&post(0, &[7])]);
    let missing = decode_patch(&unterminated).expect_err("no 0xFF before the end");
    assert_eq!(missing.code(), "format_error", "missing terminator code");
}

#[test]
fn encode_rejects_oversized_and_empty_images() {
    let wide = Patch {
        left_offset: 0,
        top_offset: 0,
        image: build(4_097, 1, full),
    };
    let CyberError::LimitExceeded(message) = encode_patch(&wide).expect_err("width past the cap")
    else {
        panic!("expected limit_exceeded for an oversized width");
    };
    assert!(message.contains("width"), "names the field: {message}");
    let tall = Patch {
        left_offset: 0,
        top_offset: 0,
        image: build(1, 4_097, full),
    };
    assert_eq!(
        encode_patch(&tall).expect_err("height past the cap").code(),
        "limit_exceeded",
        "oversized height code"
    );
    let empty = Patch {
        left_offset: 0,
        top_offset: 0,
        image: IndexedImage {
            width: 0,
            height: 1,
            indices: Vec::new(),
            opaque: Vec::new(),
        },
    };
    assert_eq!(
        encode_patch(&empty)
            .expect_err("no columns to encode")
            .code(),
        "invalid_argument",
        "zero width code"
    );
}
