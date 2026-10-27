//! The wire shapes: the documented `JSON` op lists with every serde
//! default, the new pixel ops' defaults, the grayscale and blur runs,
//! the hairline filter and the adaptive threshold reaching their
//! modules, and the radius errors each one answers with.

use crate::error::CyberError;
use crate::gfx::ops::{Filter, FitMode, ImageOp, apply_ops};

use super::quad;

#[test]
fn json_op_lists_deserialize_with_defaults() {
    let text = r##"[
        {"op":"crop","x":1,"y":2,"width":3,"height":4},
        {"op":"fit","width":8,"height":8,"mode":"contain"},
        {"op":"rotate","quarter_turns":3},
        {"op":"flip","horizontal":false,"vertical":true},
        {"op":"auto_levels"},
        {"op":"adjust"},
        {"op":"color_key","color":"#ff0000"},
        {"op":"trim"}
    ]"##;
    let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
    assert_eq!(ops.len(), 8, "all eight ops deserialize");
    let ImageOp::Fit {
        filter,
        background,
        mode,
        ..
    } = ops[1].clone()
    else {
        panic!("expected Fit at index 1");
    };
    assert_eq!(filter, Filter::Area, "filter defaults to area");
    assert_eq!(background, None, "background defaults to absent");
    assert_eq!(mode, FitMode::Contain, "explicit mode parses");
    let ImageOp::AutoLevels { clip_percent } = ops[4].clone() else {
        panic!("expected AutoLevels at index 4");
    };
    assert_eq!(clip_percent, 1, "clip defaults to one percent");
    let ImageOp::Adjust {
        brightness,
        contrast,
        saturation,
    } = ops[5].clone()
    else {
        panic!("expected Adjust at index 5");
    };
    assert_eq!(
        (brightness, contrast, saturation),
        (0_i32, 100_i32, 100_i32),
        "adjust defaults"
    );
    let ImageOp::Trim {
        alpha_threshold,
        padding,
    } = ops[7].clone()
    else {
        panic!("expected Trim at index 7");
    };
    assert_eq!((alpha_threshold, padding), (128, 0), "trim defaults");
}

#[test]
fn new_pixel_ops_deserialize_with_documented_defaults() {
    let text = r##"[
        {"op":"remove_background"},
        {"op":"remove_background","tolerance":8,"color":"#102030"},
        {"op":"make_tileable"},
        {"op":"make_tileable","band":3,"vertical":false},
        {"op":"threshold"},
        {"op":"threshold","level":128,"invert":true}
    ]"##;
    let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
    assert_eq!(ops.len(), 6, "all six entries parse");
    assert_eq!(background_of(&ops[0]), (32, None), "background defaults");
    assert_eq!(
        background_of(&ops[1]),
        (8, Some("#102030".to_owned())),
        "explicit background"
    );
    assert_eq!(tile_of(&ops[2]), (None, true, true), "tile defaults");
    assert_eq!(tile_of(&ops[3]), (Some(3), true, false), "explicit tile");
    assert_eq!(threshold_of(&ops[4]), (None, false), "threshold defaults");
    assert_eq!(
        threshold_of(&ops[5]),
        (Some(128), true),
        "explicit threshold"
    );
}

/// The `RemoveBackground` fields of an op that must be one.
fn background_of(op: &ImageOp) -> (u8, Option<String>) {
    let ImageOp::RemoveBackground { tolerance, color } = op.clone() else {
        panic!("expected a RemoveBackground op");
    };
    (tolerance, color)
}

/// The `MakeTileable` fields of an op that must be one.
fn tile_of(op: &ImageOp) -> (Option<u32>, bool, bool) {
    let ImageOp::MakeTileable {
        band,
        horizontal,
        vertical,
    } = op.clone()
    else {
        panic!("expected a MakeTileable op");
    };
    (band, horizontal, vertical)
}

/// The `Threshold` fields of an op that must be one.
fn threshold_of(op: &ImageOp) -> (Option<u8>, bool) {
    let ImageOp::Threshold { level, invert } = op.clone() else {
        panic!("expected a Threshold op");
    };
    (level, invert)
}

#[test]
fn grayscale_and_blur_deserialize_from_their_documented_json() {
    let text = r#"[
        {"op":"grayscale"},
        {"op":"blur","radius":2}
    ]"#;
    let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
    assert_eq!(ops.len(), 2, "both new ops parse");
    assert_eq!(
        ops[0],
        ImageOp::Grayscale,
        "grayscale carries no parameters"
    );
    let ImageOp::Blur { radius } = ops[1].clone() else {
        panic!("expected Blur at index 1");
    };
    assert_eq!(radius, 2_u32, "the blur radius carries through");
}

#[test]
fn grayscale_and_blur_run_through_apply_ops() {
    let ops = [ImageOp::Grayscale, ImageOp::Blur { radius: 1 }];
    let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
    assert_eq!(
        report
            .steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["grayscale", "blur"],
        "one named entry per op, in order"
    );
    assert_eq!(
        report.steps[0]["distinct_colors_before"], 4_i32,
        "the quad's four opaque colours"
    );
    assert_eq!(
        report.steps[0]["distinct_colors_after"], 4_i32,
        "76, 149, 29 and 255 greys stay distinct"
    );
    assert_eq!(report.steps[1]["radius"], 1_i32, "the radius echoes");
    let uniform = out.get(0, 0).expect("in bounds");
    for (x, y) in [(0_u32, 0_u32), (1, 0), (0, 1), (1, 1)] {
        assert_eq!(out.get(x, y), Some(uniform), "({x},{y}) joins the blur");
    }
    assert_eq!(
        [uniform[0], uniform[1], uniform[2]],
        [128_u8, 128, 128],
        "the whole 2x2 window is every pixel's window: the grey mean rounds to 128"
    );
    assert_eq!(uniform[3], 255, "opaque stays opaque");
}

#[test]
fn blur_radius_errors_name_the_op_index_and_field() {
    let error = apply_ops(quad(), &[ImageOp::Grayscale, ImageOp::Blur { radius: 0 }])
        .expect_err("radius 0 must fail");
    let CyberError::InvalidArgument(message) = error else {
        panic!("expected InvalidArgument");
    };
    assert!(
        message.contains("ops[1].radius") && message.contains("1..=256"),
        "the parameter error names the field and range: {message}"
    );
}

#[test]
fn thin_stroke_filter_deserializes_with_documented_defaults() {
    let text = r#"[
        {"op":"thin_stroke_filter","radius":1},
        {"op":"thin_stroke_filter","radius":16,"level":128,"invert":true}
    ]"#;
    let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
    assert_eq!(
        thin_stroke_fields(&ops[0]),
        (1_u32, None, false),
        "level defaults to Otsu and invert to false"
    );
    assert_eq!(
        thin_stroke_fields(&ops[1]),
        (16_u32, Some(128), true),
        "every explicit field carries through"
    );
}

/// The `ThinStrokeFilter` fields of an op that must be one.
fn thin_stroke_fields(op: &ImageOp) -> (u32, Option<u8>, bool) {
    let ImageOp::ThinStrokeFilter {
        radius,
        level,
        invert,
    } = op.clone()
    else {
        panic!("expected a ThinStrokeFilter op");
    };
    (radius, level, invert)
}

#[test]
fn thin_stroke_filter_runs_through_apply_ops() {
    let ops = [
        ImageOp::Threshold {
            level: Some(128),
            invert: false,
        },
        ImageOp::ThinStrokeFilter {
            radius: 1,
            level: None,
            invert: false,
        },
    ];
    let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
    assert_eq!(
        report.steps[1]["op"], "thin_stroke_filter",
        "the documented step name"
    );
    assert_eq!(report.steps[1]["radius"], 1_i32, "the radius echoes");
    assert_eq!(report.steps[1]["otsu"], true, "level None is Otsu's");
    assert!(report.steps[1]["level"].is_number(), "the level reports");
    assert_eq!(out.width, 2, "the op never changes the shape");
}

#[test]
fn thin_stroke_radius_errors_name_the_op_index_and_field() {
    assert_thin_radius_rejected(0_u32);
    assert_thin_radius_rejected(17_u32);
}

/// Applies a `thin_stroke_filter` with the out-of-range `radius` after a
/// leading op and checks the error names `ops[1].radius` and `1..=16`.
fn assert_thin_radius_rejected(radius: u32) {
    let error = apply_ops(
        quad(),
        &[
            ImageOp::Flip {
                horizontal: true,
                vertical: false,
            },
            ImageOp::ThinStrokeFilter {
                radius,
                level: Some(128),
                invert: false,
            },
        ],
    )
    .expect_err("a radius outside 1..=16 must fail");
    let CyberError::InvalidArgument(message) = error else {
        panic!("expected InvalidArgument");
    };
    assert!(
        message.contains("ops[1].radius") && message.contains("1..=16"),
        "the parameter error names the field and range: {message}"
    );
}

#[test]
fn adaptive_threshold_deserializes_with_documented_defaults() {
    let text = r#"[
        {"op":"adaptive_threshold","radius":4},
        {"op":"adaptive_threshold","radius":128,"offset":200,"invert":true}
    ]"#;
    let ops: Vec<ImageOp> = serde_json::from_str(text).expect("the documented JSON layout");
    assert_eq!(
        adaptive_fields(&ops[0]),
        (4_u32, 15_u8, false),
        "offset defaults to 15 and invert to false"
    );
    assert_eq!(
        adaptive_fields(&ops[1]),
        (128_u32, 200_u8, true),
        "every explicit field carries through"
    );
}

/// The `AdaptiveThreshold` fields of an op that must be one.
fn adaptive_fields(op: &ImageOp) -> (u32, u8, bool) {
    let ImageOp::AdaptiveThreshold {
        radius,
        offset,
        invert,
    } = op.clone()
    else {
        panic!("expected an AdaptiveThreshold op");
    };
    (radius, offset, invert)
}

#[test]
fn adaptive_threshold_runs_through_apply_ops() {
    let ops = [
        ImageOp::Threshold {
            level: Some(128),
            invert: false,
        },
        ImageOp::AdaptiveThreshold {
            radius: 1,
            offset: 0,
            invert: false,
        },
    ];
    let (out, report) = apply_ops(quad(), &ops).expect("chain applies");
    assert_eq!(
        report.steps[1]["op"], "adaptive_threshold",
        "the documented step name"
    );
    assert_eq!(report.steps[1]["radius"], 1_i32, "the radius echoes");
    assert_eq!(report.steps[1]["offset"], 0_i32, "the offset echoes");
    assert_eq!(
        report.steps[1]["otsu"], false,
        "the local test never uses Otsu"
    );
    assert_eq!(
        report.steps[1]["background_min"], 127_i32,
        "every window is the whole 2x2 of the binarized corners: mean(0, 255, 0, 255) floors to 127"
    );
    assert_eq!(report.steps[1]["background_max"], 127_i32, "one mean");
    assert_eq!(
        report.steps[1]["ink_pixels"], 2_i32,
        "the two black corners sit at or below the mean, the two white ones above"
    );
    assert_eq!(
        out.get(0, 0),
        Some([0, 0, 0, 255]),
        "the black corner re-inks to opaque black"
    );
    assert_eq!(
        out.get(1, 1),
        Some([255, 255, 255, 255]),
        "the white corner stays opaque paper"
    );
    assert_eq!(
        (out.width, out.height),
        (2, 2),
        "the op never changes the shape"
    );
}

#[test]
fn adaptive_radius_errors_name_the_op_index_and_field() {
    for radius in [0_u32, 129_u32] {
        let error = apply_ops(
            quad(),
            &[
                ImageOp::Flip {
                    horizontal: true,
                    vertical: false,
                },
                ImageOp::AdaptiveThreshold {
                    radius,
                    offset: 15,
                    invert: false,
                },
            ],
        )
        .expect_err("a radius outside 1..=128 must fail");
        assert_eq!(
            error.code(),
            "invalid_argument",
            "radius {radius} refuses by code"
        );
        let message = error.to_string();
        assert!(
            message.contains("ops[1].radius") && message.contains("1..=128"),
            "the parameter error names the field and range: {message}"
        );
    }
}
