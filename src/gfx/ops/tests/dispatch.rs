//! The runner: an empty list, the chained report, every op reaching its
//! own module, the two dispatchers refusing the variants they were not
//! routed for, the `JSON`-deserialized `PerspectiveCorrect` and the size
//! and parameter refusals that name `ops[i].field`.

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::perspective::Corner;
use crate::gfx::ops::{Filter, FitMode, ImageOp, MAX_OP_DIMENSION, OpsReport, apply_ops};

use crate::gfx::ops::apply::{geometry_op, pixel_op, restoration_op};

use super::quad;

#[test]
fn empty_op_list_returns_the_image_and_no_steps() {
    let (out, report) = apply_ops(quad(), &[]).expect("no ops cannot fail");
    assert_eq!(out.width, 2, "image passes through");
    assert_eq!(report, OpsReport::default(), "no steps for no ops");
}

#[test]
fn apply_ops_chains_ops_with_one_entry_per_op() {
    let ops = [
        ImageOp::Flip {
            horizontal: true,
            vertical: false,
        },
        ImageOp::Crop {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        },
        ImageOp::Trim {
            alpha_threshold: 1,
            padding: 0,
        },
    ];
    let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
    assert_eq!(
        report
            .steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["flip", "crop", "trim"],
        "one named entry per op, in order"
    );
    assert_eq!((out.width, out.height), (2, 1), "crop sized the result");
}

#[test]
fn the_new_ops_reach_their_modules_through_apply_ops() {
    let ops = [
        ImageOp::RemoveBackground {
            tolerance: 8,
            color: Some("#0000ff".to_owned()),
        },
        ImageOp::MakeTileable {
            band: Some(1),
            horizontal: true,
            vertical: false,
        },
        ImageOp::Threshold {
            level: Some(128),
            invert: false,
        },
    ];
    let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
    assert_eq!(
        report
            .steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["remove_background", "make_tileable", "threshold"],
        "each new op reports under its documented name"
    );
    assert_eq!(
        report.steps[0]["background"], "#0000FF",
        "the requested colour is reported"
    );
    assert_eq!(
        report.steps[0]["removed_pixels"], 1_i32,
        "only the blue border pixel floods"
    );
    assert_eq!(report.steps[1]["band_x"], 1_i32, "explicit horizontal band");
    assert!(
        report.steps[1]["band_y"].is_null(),
        "vertical axis disabled"
    );
    assert_eq!(
        report.steps[2]["level"], 128_i32,
        "explicit threshold level"
    );
    assert_eq!(
        out.get(0, 0),
        Some([255, 255, 255, 255]),
        "green's luminance 149 exceeds the level and binarizes to white"
    );
    assert_eq!(
        out.get(1, 0),
        Some([255, 255, 255, 255]),
        "the tiling pass overwrote blue with the white column, everything ends opaque"
    );
}

#[test]
fn pixel_op_rejects_geometric_variants_it_was_not_routed_for() {
    let error = pixel_op(
        quad(),
        3,
        ImageOp::Flip {
            horizontal: false,
            vertical: false,
        },
    )
    .expect_err("a flip never belongs to the pixel dispatcher");
    assert_eq!(error.code(), "internal", "dispatch bugs are internal");
}

#[test]
fn restoration_op_rejects_variants_it_was_not_routed_for() {
    let error = restoration_op(
        quad(),
        5,
        ImageOp::Trim {
            alpha_threshold: 128,
            padding: 0,
        },
    )
    .expect_err("a trim never belongs to the restoration dispatcher");
    let CyberError::Internal(message) = error else {
        panic!("expected Internal");
    };
    assert!(
        message.contains("ops[5]") && message.contains("wrong pixel dispatcher"),
        "misdispatch names the op and the fault: {message}"
    );
}

#[test]
fn geometry_op_rejects_pixel_variants_it_was_not_routed_for() {
    let error = geometry_op(
        &quad(),
        1,
        ImageOp::Threshold {
            level: Some(128),
            invert: false,
        },
    )
    .expect_err("a threshold never belongs to the geometric dispatcher");
    assert_eq!(error.code(), "internal", "dispatch bugs are internal");
}

/// The perspective warp's own square over the 2x2 test image.
fn straighten() -> ImageOp {
    ImageOp::PerspectiveCorrect {
        corners: [
            Corner { x: 0, y: 0 },
            Corner { x: 1, y: 0 },
            Corner { x: 1, y: 1 },
            Corner { x: 0, y: 1 },
        ],
        width: 2,
        height: 2,
    }
}

#[test]
fn perspective_correct_deserializes_and_runs_through_apply_ops() {
    let text = r#"[{"op":"perspective_correct",
        "corners":[{"x":0,"y":0},{"x":1,"y":0},{"x":1,"y":1},{"x":0,"y":1}],
        "width":2,"height":2}]"#;
    let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
    assert_eq!(ops[0], straighten(), "the tag, corners and size parse");
    let (out, report) = apply_ops(quad(), &ops).expect("the warp runs");
    assert_eq!(
        report.steps[0]["op"], "perspective_correct",
        "the documented step name"
    );
    assert_eq!((out.width, out.height), (2, 2), "the target shape");
    assert_eq!(
        out.get(1, 0),
        Some([0, 255, 0, 255]),
        "the identity warp keeps pixels"
    );
}

#[test]
fn the_pixel_dispatchers_reject_perspective_correct() {
    let to_pixel = pixel_op(quad(), 2, straighten())
        .expect_err("a warp never belongs to the pixel dispatcher");
    assert_eq!(to_pixel.code(), "internal", "dispatch bugs are internal");
    let to_restoration = restoration_op(quad(), 3, straighten())
        .expect_err("a warp never belongs to the restoration dispatcher");
    assert_eq!(
        to_restoration.code(),
        "internal",
        "dispatch bugs are internal"
    );
}

#[test]
fn invalid_parameters_name_the_op_index_and_field() {
    let ops = [
        ImageOp::Flip {
            horizontal: true,
            vertical: false,
        },
        ImageOp::Rotate { quarter_turns: 2 },
        ImageOp::Crop {
            x: 0,
            y: 0,
            width: 0,
            height: 1,
        },
    ];
    let error = apply_ops(quad(), &ops).expect_err("zero width must fail");
    let CyberError::InvalidArgument(message) = error else {
        panic!("expected InvalidArgument");
    };
    assert!(
        message.contains("ops[2].width") && message.contains("1..=4096"),
        "message names the failing op: {message}"
    );
}

#[test]
fn fit_parameters_are_capped_at_the_op_dimension() {
    let error = apply_ops(
        quad(),
        &[ImageOp::Fit {
            width: MAX_OP_DIMENSION + 1,
            height: 1,
            mode: FitMode::Stretch,
            filter: Filter::Nearest,
            background: None,
        }],
    )
    .expect_err("a 4097-wide target must fail");
    let CyberError::InvalidArgument(message) = error else {
        panic!("expected InvalidArgument");
    };
    assert!(
        message.contains("ops[0].width") && message.contains("1..=4096"),
        "parameter error names the field and range: {message}"
    );
}

#[test]
fn results_above_the_op_dimension_are_rejected() {
    let wide = RgbaImage::new(8_000, 1, vec![255_u8; 32_000_usize]).expect("8000x1 fits core");
    let error = apply_ops(
        wide,
        &[ImageOp::Flip {
            horizontal: true,
            vertical: false,
        }],
    )
    .expect_err("an 8000-wide result exceeds the op limit");
    let CyberError::InvalidArgument(message) = error else {
        panic!("expected InvalidArgument");
    };
    assert!(
        message.contains("ops[0]") && message.contains("exceeds 4096"),
        "result error names the op and limit: {message}"
    );
}
