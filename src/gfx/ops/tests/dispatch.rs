//! The runner: an empty list, the chained report, every op reaching its own
//! module through [`apply_ops`], the `JSON`-deserialized
//! `PerspectiveCorrect` and the size and parameter refusals that name
//! `ops[i].field`.

use crate::error::CyberError;
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::perspective::Corner;
use crate::gfx::ops::{Filter, FitMode, ImageOp, MAX_OP_DIMENSION, OpsReport, apply_ops};

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

/// The five variants that change the picture's shape, each with the step
/// name its module documents.
fn shape_ops() -> [(&'static str, ImageOp); 5] {
    [
        (
            "crop",
            ImageOp::Crop {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
        ),
        (
            "fit",
            ImageOp::Fit {
                width: 1,
                height: 1,
                mode: FitMode::Stretch,
                filter: Filter::Nearest,
                background: None,
            },
        ),
        ("rotate", ImageOp::Rotate { quarter_turns: 1 }),
        (
            "flip",
            ImageOp::Flip {
                horizontal: true,
                vertical: false,
            },
        ),
        ("perspective_correct", straighten()),
    ]
}

/// The five tonal and alpha variants, each with the step name its module
/// documents.
fn tone_ops() -> [(&'static str, ImageOp); 5] {
    [
        ("auto_levels", ImageOp::AutoLevels { clip_percent: 1 }),
        (
            "adjust",
            ImageOp::Adjust {
                brightness: 0,
                contrast: 100,
                saturation: 100,
            },
        ),
        (
            "color_key",
            ImageOp::ColorKey {
                color: "#ff0000".to_owned(),
                tolerance: 0,
            },
        ),
        (
            "trim",
            ImageOp::Trim {
                alpha_threshold: 128,
                padding: 0,
            },
        ),
        ("grayscale", ImageOp::Grayscale),
    ]
}

/// The six background, tiling, binarizing and blurring variants, each with
/// the step name its module documents.
fn cleaning_ops() -> [(&'static str, ImageOp); 6] {
    [
        (
            "remove_background",
            ImageOp::RemoveBackground {
                tolerance: 8,
                color: Some("#0000ff".to_owned()),
            },
        ),
        (
            "make_tileable",
            ImageOp::MakeTileable {
                band: Some(1),
                horizontal: true,
                vertical: false,
            },
        ),
        (
            "threshold",
            ImageOp::Threshold {
                level: Some(128),
                invert: false,
            },
        ),
        (
            "adaptive_threshold",
            ImageOp::AdaptiveThreshold {
                radius: 1,
                offset: 15,
                invert: false,
            },
        ),
        ("blur", ImageOp::Blur { radius: 1 }),
        (
            "thin_stroke_filter",
            ImageOp::ThinStrokeFilter {
                radius: 1,
                level: Some(128),
                invert: false,
            },
        ),
    ]
}

#[test]
fn every_op_reaches_its_module_through_apply_ops() {
    let ops: Vec<(&str, ImageOp)> = shape_ops()
        .into_iter()
        .chain(tone_ops())
        .chain(cleaning_ops())
        .collect();
    assert_eq!(ops.len(), 16_usize, "one table row per ImageOp variant");
    for (name, op) in ops {
        let (out, report) = apply_ops(quad(), &[op])
            .unwrap_or_else(|error| panic!("the dispatch runs {name}: {error}"));
        assert_eq!(report.steps.len(), 1_usize, "{name} answers one step");
        assert_eq!(report.steps[0]["op"], name, "{name} reports its name");
        assert!(!out.pixels.is_empty(), "{name} leaves a picture");
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
