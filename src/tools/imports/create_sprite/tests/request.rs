//! The call's arguments: the three naming forms and their refusals, the
//! up-front `height` and `background_color` checks, the default cleanup
//! chain, the height rule and its fit step, the emptied-picture hint, the
//! frame dedupe, the offset presets and the documented wire form.

use serde_json::json;

use crate::gfx::ops::ImageOp;
use crate::tools::imports::create_sprite::build::{
    apply_extra, background_ops, choose_height, emptied, fit_step, scaled_width,
};
use crate::tools::imports::create_sprite::{
    CREATE_SPRITE_TOOL_DESCRIPTION, CreateSpriteParams, check_background_color, check_height,
    dedupe, parse_sprite_names,
};
use crate::tools::imports::image_pipeline::OffsetMode;

use super::{
    args, by_name, by_sprite, code, names_of, processed, request, request_for_height, trim_error,
};

#[test]
fn the_name_form_serves_one_lump() {
    assert_eq!(names_of(&by_name("trooa2a8")), vec!["TROOA2A8"]);
    assert_eq!(names_of(&by_name("PLAYA0")), vec!["PLAYA0"]);
}

#[test]
fn the_parts_form_names_one_lump_with_defaults() {
    assert_eq!(names_of(&by_sprite("boba")), vec!["BOBAA0"]);
    let mut views = by_sprite("TROO");
    views.frame = Some("a".to_owned());
    views.rotation = Some(2_u8);
    views.mirror_rotation = Some(8_u8);
    assert_eq!(names_of(&views), vec!["TROOA2A8"], "the mirror pair form");
}

#[test]
fn the_frames_form_names_the_whole_list() {
    let mut call = by_sprite("TROO");
    call.frames = Some(
        ["A", "B", "C", "D"]
            .iter()
            .map(|letter| (*letter).to_owned())
            .collect(),
    );
    assert_eq!(
        names_of(&call),
        vec!["TROOA0", "TROOB0", "TROOC0", "TROOD0"],
        "one lump per frame at the rotation"
    );
    call.rotation = Some(1_u8);
    call.mirror_rotation = Some(5_u8);
    assert_eq!(
        names_of(&call),
        vec!["TROOA1A5", "TROOB1B5", "TROOC1C5", "TROOD1D5"],
        "every frame keeps the mirror pair"
    );
}

#[test]
fn duplicate_frames_write_the_lump_once() {
    let mut call = by_sprite("BOBA");
    call.frames = Some(
        ["A", "B", "A"]
            .iter()
            .map(|letter| (*letter).to_owned())
            .collect(),
    );
    assert_eq!(
        names_of(&call),
        vec!["BOBAA0", "BOBAB0"],
        "the repeat folds away"
    );
}

#[test]
fn selector_combinations_refuse() {
    let mut both = by_name("TROOA1");
    both.sprite = Some("TROO".to_owned());
    let error = parse_sprite_names(&both).expect_err("two selectors");
    assert_eq!(code(&error), "invalid_argument");
    assert!(error.to_string().contains("not both"), "{error}");
    let mut name_frames = by_name("TROOA1");
    name_frames.frames = Some(vec!["A".to_owned()]);
    assert_eq!(
        code(&parse_sprite_names(&name_frames).expect_err("frames beside name")),
        "invalid_argument"
    );
    let mut frame_frames = by_sprite("TROO");
    frame_frames.frame = Some("A".to_owned());
    frame_frames.frames = Some(vec!["B".to_owned()]);
    let pair_error = parse_sprite_names(&frame_frames).expect_err("frame beside frames");
    assert_eq!(code(&pair_error), "invalid_argument");
    assert!(
        pair_error.to_string().contains("frame and frames"),
        "the message names the pair: {pair_error}"
    );
    let mut alone = args();
    alone.frames = Some(vec!["A".to_owned()]);
    assert_eq!(
        code(&parse_sprite_names(&alone).expect_err("frames without sprite")),
        "invalid_argument"
    );
    alone.frames = Some(Vec::new());
    assert_eq!(
        code(&parse_sprite_names(&alone).expect_err("an empty list")),
        "invalid_argument"
    );
    assert_eq!(
        code(&parse_sprite_names(&args()).expect_err("no selector at all")),
        "invalid_argument"
    );
    let mut bad = by_sprite("TROO");
    bad.frames = Some(vec!["9".to_owned()]);
    assert_eq!(
        code(&parse_sprite_names(&bad).expect_err("a digit is no frame")),
        "invalid_argument"
    );
}

#[test]
fn height_and_background_colour_are_validated_up_front() {
    assert_eq!(check_height(1_u32).expect("one"), 1_u32);
    assert_eq!(check_height(512_u32).expect("the top"), 512_u32);
    for value in [0_u32, 513_u32] {
        let error = check_height(value).expect_err("out of range");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("1..=512"),
            "the message names the range: {error}"
        );
    }
    assert_eq!(
        check_background_color("#00FF00").expect("a colour"),
        "#00FF00"
    );
    let error = check_background_color("green").expect_err("no colour");
    assert_eq!(code(&error), "invalid_argument");
    assert!(
        error.to_string().contains("background_color"),
        "the message names the field: {error}"
    );
}

#[test]
fn the_default_request_is_removal_trim_and_feet_on_origin() {
    let built = request(&by_name("TROOA1"));
    assert!(built.cleanup.remove, "background removal is on by default");
    assert_eq!(built.cleanup.tolerance, 32_u8, "the documented tolerance");
    assert!(built.cleanup.trim, "trim is on by default");
    assert!(
        built.cleanup.color.is_none(),
        "the border colour is detected"
    );
    assert!(built.height.is_none(), "no explicit height");
    assert!(built.offsets.is_none(), "the preset default");
    assert!(!built.replace, "no silent overwrite");
    let ops = background_ops(&built.cleanup);
    assert_eq!(ops.len(), 2_usize, "the documented chain");
    assert!(
        matches!(ops.first(), Some(ImageOp::RemoveBackground { tolerance, .. }) if *tolerance == 32_u8),
        "the flood runs first at the default tolerance"
    );
    assert!(
        matches!(
            ops.get(1_usize),
            Some(ImageOp::Trim { alpha_threshold, padding })
                if *alpha_threshold == 128_u8 && *padding == 0_u32
        ),
        "the trim runs at alpha 128 with no padding"
    );
    let mut off = by_name("TROOA1");
    off.remove_background = Some(false);
    off.trim = Some(false);
    assert!(
        background_ops(&request(&off).cleanup).is_empty(),
        "both switches off, no ops"
    );
}

#[test]
fn the_height_rule_scales_explicitly_and_catches_tall_pictures() {
    let call = request_for_height(Some(40_u32));
    let plan = choose_height(&call, &processed(10_u32, 20_u32).rgba).expect("a fit");
    assert_eq!((plan.width, plan.height), (20_u32, 40_u32), "aspect kept");
    assert!(plan.note.is_none(), "an explicit height adds no note");
    let equal = choose_height(&call, &processed(20_u32, 40_u32).rgba);
    assert!(equal.is_none(), "an already-exact size does not fit again");
    let tall = request(&by_name("TROOA1"));
    let auto = choose_height(&tall, &processed(60_u32, 300_u32).rgba).expect("the rule fires");
    assert_eq!((auto.width, auto.height), (26_u32, 128_u32), "128 tall");
    assert_eq!(
        auto.note.as_deref(),
        Some("scaled to 128 px tall; pass height to choose"),
        "the documented note"
    );
    let short = choose_height(&tall, &processed(60_u32, 256_u32).rgba);
    assert!(short.is_none(), "256 is not too tall");
    assert_eq!(
        scaled_width(&processed(3_u32, 900_u32).rgba, 1_u32),
        1_u32,
        "never zero"
    );
}

#[test]
fn fit_appends_the_step_and_keeps_the_report_in_order() {
    let call = request_for_height(Some(40_u32));
    let picture = processed(10_u32, 20_u32);
    let (shaped, note) = fit_step(picture, &call).expect("the fit runs");
    assert_eq!((shaped.rgba.width, shaped.rgba.height), (20_u32, 40_u32));
    assert!(note.is_none(), "explicit height stays quiet");
    assert_eq!(shaped.report.steps.len(), 1_usize, "one automatic step");
    let kept = fit_step(processed(20_u32, 40_u32), &call).expect("nothing to do");
    assert!(kept.0.report.steps.is_empty(), "an equal size adds no step");
}

#[test]
fn apply_extra_without_ops_is_the_identity() {
    let picture = processed(4_u32, 4_u32);
    let same = apply_extra(picture, &[]).expect("no ops run");
    assert_eq!((same.rgba.width, same.rgba.height), (4_u32, 4_u32));
}

#[test]
fn an_emptied_picture_names_the_way_out() {
    let with_removal = request(&by_name("TROOA1"));
    let error = emptied(
        &with_removal.cleanup,
        trim_error("ops[1] nothing left after trim: no pixel reaches alpha 128"),
    );
    assert_eq!(code(&error), "invalid_argument");
    let message = error.to_string();
    assert!(
        message.contains("the whole image was removed as background"),
        "the documented hint: {message}"
    );
    assert!(
        message.contains("lower background_tolerance")
            && message.contains("remove_background false"),
        "both ways out are named: {message}"
    );
    let mut trim_only = by_name("TROOA1");
    trim_only.remove_background = Some(false);
    let other = emptied(
        &request(&trim_only).cleanup,
        trim_error("ops[0] nothing left after trim: no pixel reaches alpha 128"),
    );
    assert!(
        other.to_string().contains("trim=false"),
        "without removal the trim owns the message: {other}"
    );
    let passed = emptied(
        &with_removal.cleanup,
        trim_error("ops[0].color is out of range"),
    );
    assert!(
        passed.to_string().contains("out of range"),
        "another error stands as it is: {passed}"
    );
}

#[test]
fn dedupe_keeps_the_first_occurrences_in_order() {
    let mut call = by_sprite("BOBA");
    call.frames = Some(
        ["B", "A", "B"]
            .iter()
            .map(|letter| (*letter).to_owned())
            .collect(),
    );
    let names = parse_sprite_names(&call).expect("valid");
    let folded = dedupe(names);
    let texts: Vec<String> = folded
        .iter()
        .map(|naming| naming.name.to_string())
        .collect();
    assert_eq!(texts, vec!["BOBAB0", "BOBAA0"], "first order, no repeats");
}

#[test]
fn offsets_presets_and_literals_reach_the_request() {
    let mut call = by_name("TROOA1");
    call.offsets = Some(crate::tools::imports::image_pipeline::OffsetsParam::Preset(
        "center".to_owned(),
    ));
    assert_eq!(
        request(&call).offsets,
        Some(OffsetMode::Center),
        "the preset parses"
    );
}

#[test]
fn the_wire_form_flattens_every_documented_field() {
    let call: CreateSpriteParams = serde_json::from_value(json!({
        "source": "/x/y.png",
        "sprite": "TROO",
        "frames": ["A", "B"],
        "rotation": 2_u64,
        "remove_background": false,
        "background_tolerance": 9_u64,
        "background_color": "#FFFFFF",
        "trim": false,
        "height": 40_u64,
        "offsets": {"x": 3_i64, "y": 8_i64},
        "ops": [{"op": "flip", "horizontal": true, "vertical": false}],
        "dither": "none",
        "palette": 1_u64,
        "replace": true,
    }))
    .expect("the documented wire form");
    assert_eq!(call.frames.as_ref().expect("list").len(), 2_usize);
    assert_eq!(call.rotation, Some(2_u8));
    assert_eq!(call.remove_background, Some(false));
    assert_eq!(call.background_tolerance, Some(9_u8));
    assert_eq!(call.height, Some(40_u32));
    assert!(call.shared.dither.is_some(), "the dither carried");
    let built = request(&call);
    assert!(
        !built.cleanup.remove && !built.cleanup.trim,
        "both switches carry"
    );
    assert_eq!(built.cleanup.tolerance, 9_u8);
    assert_eq!(built.cleanup.color.as_deref(), Some("#FFFFFF"));
    assert_eq!(built.height, Some(40_u32));
    assert_eq!(
        built.offsets,
        Some(OffsetMode::Explicit {
            left: 3_i32,
            top: 8_i32
        })
    );
    assert!(built.replace);
    assert_eq!(built.names.len(), 2_usize, "both frames named");
    assert!(
        CREATE_SPRITE_TOOL_DESCRIPTION.contains("ONE undoable edit"),
        "the description promises one undoable edit"
    );
}
