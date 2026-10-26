//! The `add_linedef` topic: the endpoint forms, the side, flag, family and
//! binary record rules, the sides a new line creates and the append step with
//! its refusals.

use super::{add_args, code, fixture, solid_line, spec_of};
use crate::tools::edit_linedefs::add::{NewSide, add_in_place, new_sidedef, validate_add};
use crate::tools::edit_linedefs::endpoints::{EndpointSpec, endpoint_spec, place_endpoints};
use crate::tools::edit_linedefs::{check_binary_fields, check_family_fields, default_line_flags};
use crate::wad::classify::MapFormat;
use crate::wad::map::{FlagFamily, Linedef};

#[test]
fn the_endpoint_forms_are_exactly_one_of_indices_or_points() {
    let indices = add_args(r#"{"v1": 0, "v2": 5, "front": 0}"#);
    assert!(
        matches!(spec_of(&indices), EndpointSpec::Indices { v1: 0, v2: 5 }),
        "the index form reads through"
    );
    let points = add_args(r#"{"from": {"x": 0, "y": 0}, "to": {"x": 8, "y": 8}, "front": 0}"#);
    assert!(
        matches!(spec_of(&points), EndpointSpec::Points { .. }),
        "the point form reads through"
    );
    let refusals = [
        r#"{"front": 0}"#,
        r#"{"v1": 0, "front": 0}"#,
        r#"{"v1": 0, "v2": 1, "from": {"x": 0, "y": 0}, "front": 0}"#,
        r#"{"from": {"x": 0, "y": 0}, "to": {"x": 1, "y": 1}, "v2": 1, "front": 0}"#,
    ];
    for json in refusals {
        let error = endpoint_spec(&add_args(json)).expect_err("the request must refuse");
        assert_eq!(code(&error), "invalid_argument", "endpoint code for {json}");
    }
}

#[test]
fn add_validation_reads_side_forms_and_the_required_front() {
    let both =
        add_args(r#"{"v1": 0, "v2": 1, "front_side": {"sector": 0}, "back_side": {"sector": 1}}"#);
    let good = validate_add(&both).expect("both sides created");
    assert!(
        matches!(good, EndpointSpec::Indices { v1: 0, v2: 1 }),
        "validation hands the spec through"
    );
    let double_front = r#"{"v1": 0, "v2": 1, "front": 0, "front_side": {"sector": 0}}"#;
    let double_back = r#"{"v1": 0, "v2": 1, "front": 0, "back": 1, "back_side": {"sector": 1}}"#;
    let no_front = r#"{"v1": 0, "v2": 1}"#;
    let only_back = r#"{"v1": 0, "v2": 1, "back": 3}"#;
    for json in [double_front, double_back, no_front, only_back] {
        let error = validate_add(&add_args(json)).expect_err("the request must refuse");
        assert_eq!(code(&error), "invalid_argument", "side code for {json}");
    }
}

#[test]
fn the_flag_defaults_follow_one_or_two_sidedness() {
    assert_eq!(default_line_flags(false), 1_u32, "one-sided blocks");
    assert_eq!(default_line_flags(true), 4_u32, "two-sided just is");
}

#[test]
fn family_rules_move_tag_and_args_apart() {
    let tagged = Linedef {
        tag: 5_i32,
        ..solid_line(0, 1, 0)
    };
    assert!(
        check_family_fields(MapFormat::Doom, FlagFamily::Doom, &tagged).is_ok(),
        "the Doom family stores tags"
    );
    let error = check_family_fields(MapFormat::Hexen, FlagFamily::Hexen, &tagged)
        .expect_err("Hexen has no tag");
    assert_eq!(code(&error), "invalid_argument", "family rule code");
    assert!(
        error.to_string().contains("have no tag"),
        "the message names the field: {error}"
    );
    let arged = Linedef {
        args: [0_i32, 0_i32, 3_i32, 0_i32, 0_i32],
        ..solid_line(0, 1, 0)
    };
    assert!(
        check_family_fields(MapFormat::Hexen, FlagFamily::Hexen, &arged).is_ok(),
        "the Hexen family stores args"
    );
    let binary = check_family_fields(MapFormat::Doom, FlagFamily::Doom, &arged)
        .expect_err("the Doom family refuses them");
    assert!(
        binary
            .to_string()
            .contains("Doom-format linedefs have no args; use a Hexen or UDMF map"),
        "the binary advice names the other formats: {binary}"
    );
    let udmf_doom = check_family_fields(MapFormat::Udmf, FlagFamily::Doom, &arged)
        .expect_err("a UDMF map in the Doom namespace refuses them too");
    let message = udmf_doom.to_string();
    assert!(
        message.contains("convert_map_format")
            && message.contains("zdoom namespace")
            && message.contains("(Hexen family)"),
        "the UDMF advice names the namespace conversion: {message}"
    );
    assert!(
        !message.contains("use a Hexen or UDMF map"),
        "it never says the map is not already UDMF: {message}"
    );
    assert!(
        check_family_fields(MapFormat::Doom, FlagFamily::Doom, &solid_line(0, 1, 0)).is_ok(),
        "zeros are plain"
    );
}

#[test]
fn binary_record_ranges_are_checked_and_udmf_is_unbounded() {
    let wide = Linedef {
        special: 300_i32,
        tag: 70_000_i32,
        ..solid_line(0, 1, 0)
    };
    assert!(
        check_binary_fields(MapFormat::Doom, &wide).is_err(),
        "a tag above u16 refuses on binary Doom"
    );
    let big_special = Linedef {
        special: 70_000_i32,
        ..solid_line(0, 1, 0)
    };
    assert!(
        check_binary_fields(MapFormat::Doom, &big_special).is_err(),
        "so does a special above u16"
    );
    let hexen = Linedef {
        special: 300_i32,
        ..solid_line(0, 1, 0)
    };
    assert!(
        check_binary_fields(MapFormat::Hexen, &hexen).is_err(),
        "a Hexen special above u8 refuses"
    );
    let big_arg = Linedef {
        args: [0_i32, 0_i32, 0_i32, 256_i32, 0_i32],
        ..solid_line(0, 1, 0)
    };
    assert!(
        check_binary_fields(MapFormat::Hexen, &big_arg).is_err(),
        "a Hexen arg above u8 refuses"
    );
    assert!(
        check_binary_fields(MapFormat::Udmf, &big_arg).is_ok(),
        "UDMF stores full integers"
    );
    assert!(
        check_binary_fields(MapFormat::Doom, &solid_line(0, 1, 0)).is_ok(),
        "the vanilla zeros fit everywhere"
    );
}

#[test]
fn a_new_linedef_appends_reusing_vertices_and_creating_sides() {
    let mut map = fixture(MapFormat::Doom);
    let args = add_args(
        r#"{"from": {"x": 0, "y": 0}, "to": {"x": 256, "y": 128},
                "front_side": {"sector": 1, "middle": "brick01", "x_offset": 8},
                "back": 3}"#,
    );
    let spec = spec_of(&args);
    let row = add_in_place(&mut map, &args, &spec).expect("the line lands");
    assert_eq!(row.index, 7, "after the existing seven");
    assert_eq!(
        row.created_vertices,
        vec![6_usize],
        "only the far point is new"
    );
    assert_eq!(
        row.created_sidedefs,
        vec![8_usize],
        "the front side was created"
    );
    assert_eq!(row.detail.front, Some(8), "and it fronts the line");
    assert_eq!(row.detail.back, Some(3), "the referenced back stands");
    assert_eq!(
        row.detail.flag_names,
        vec!["twosided"],
        "a back makes it two-sided"
    );
    assert_eq!(
        row.detail.args, [0_i32; 5],
        "the Doom family keeps zero args"
    );
    let created = map.vertices.get(6).expect("the new vertex");
    assert_eq!((created.x, created.y), (256_i32, 128_i32), "at its point");
    let side = map.sidedefs.get(8).expect("the new sidedef");
    assert_eq!((side.middle.as_str(), side.x_offset), ("BRICK01", 8_i32));
    assert_eq!(map.linedefs.len(), 8, "and the block grew by one line");
}

#[test]
fn add_resolves_existing_indices_and_one_sided_defaults() {
    let mut map = fixture(MapFormat::Doom);
    let args = add_args(r#"{"v1": 2, "v2": 4, "front": 6}"#);
    let row = add_in_place(&mut map, &args, &spec_of(&args)).expect("an existing pair");
    assert!(row.created_vertices.is_empty(), "nothing was created");
    assert!(row.created_sidedefs.is_empty(), "no side either");
    assert_eq!(row.detail.flags, 1_u32, "one-sided defaults to blocking");
    assert_eq!(
        row.detail.front,
        Some(6),
        "the named sidedef became the front"
    );
}

#[test]
fn add_refuses_degenerate_missing_and_doubled_lines() {
    let mut map = fixture(MapFormat::Doom);
    let same = add_args(r#"{"v1": 4, "v2": 4, "front": 0}"#);
    let error = add_in_place(&mut map, &same, &spec_of(&same)).expect_err("one point");
    assert_eq!(code(&error), "invalid_argument", "degenerate code");
    let twin =
        add_args(r#"{"from": {"x": 256, "y": 256}, "to": {"x": 256, "y": 256}, "front": 0}"#);
    let twin_error =
        add_in_place(&mut map, &twin, &spec_of(&twin)).expect_err("the same point twice");
    assert_eq!(
        code(&twin_error),
        "invalid_argument",
        "point degeneracy code"
    );
    let flipped = add_args(r#"{"v1": 5, "v2": 0, "front": 0}"#);
    let duplicate = add_in_place(&mut map, &flipped, &spec_of(&flipped))
        .expect_err("linedef 0 runs 0 to 5 already");
    assert_eq!(code(&duplicate), "already_exists", "duplicate code");
    assert!(
        duplicate.to_string().contains("linedef 0"),
        "the message names the existing line: {duplicate}"
    );
    let no_vertex = add_args(r#"{"v1": 0, "v2": 9, "front": 0}"#);
    assert_eq!(
        code(&add_in_place(&mut map, &no_vertex, &spec_of(&no_vertex)).expect_err("gone")),
        "not_found",
        "a missing vertex is not_found"
    );
    let no_side = add_args(r#"{"v1": 0, "v2": 2, "front": 99}"#);
    assert_eq!(
        code(&add_in_place(&mut map, &no_side, &spec_of(&no_side)).expect_err("gone")),
        "not_found",
        "a missing sidedef is not_found"
    );
    let no_sector = add_args(r#"{"v1": 0, "v2": 2, "front_side": {"sector": 9}}"#);
    assert_eq!(
        code(&add_in_place(&mut map, &no_sector, &spec_of(&no_sector)).expect_err("gone")),
        "not_found",
        "a missing sector is not_found"
    );
    assert_eq!(map.linedefs.len(), 7, "every refusal appended nothing");
    assert_eq!(map.vertices.len(), 6, "and no vertex came in");
}

#[test]
fn the_hexen_family_moves_tag_into_args() {
    let mut map = fixture(MapFormat::Udmf);
    map.udmf_namespace = Some("zdoom".to_owned());
    let tagged = add_args(r#"{"v1": 0, "v2": 2, "front": 0, "tag": 5}"#);
    let error = add_in_place(&mut map, &tagged, &spec_of(&tagged)).expect_err("no tag here");
    assert_eq!(code(&error), "invalid_argument", "Hexen tag code");
    assert!(
        error.to_string().contains("have no tag"),
        "the message names the field: {error}"
    );
    let arged =
        add_args(r#"{"v1": 0, "v2": 2, "front": 0, "special": 80, "args": [7, 0, 0, 0, 0]}"#);
    let row = add_in_place(&mut map, &arged, &spec_of(&arged)).expect("Hexen takes args");
    assert_eq!(
        row.detail.args,
        [7_i32, 0_i32, 0_i32, 0_i32, 0_i32],
        "and stores them"
    );
    let mut binary_doom = fixture(MapFormat::Doom);
    let arged_doom = add_args(r#"{"v1": 0, "v2": 2, "front": 0, "args": [7, 0, 0, 0, 0]}"#);
    let refusal = add_in_place(&mut binary_doom, &arged_doom, &spec_of(&arged_doom))
        .expect_err("no args here");
    assert_eq!(
        code(&refusal),
        "invalid_argument",
        "the Doom family refuses args"
    );
}

#[test]
fn binary_points_cap_at_i16_and_created_sides_at_i16_offsets() {
    let mut map = fixture(MapFormat::Doom);
    let far = add_args(r#"{"from": {"x": 40000, "y": 0}, "to": {"x": 0, "y": 0}, "front": 0}"#);
    let error = add_in_place(&mut map, &far, &spec_of(&far)).expect_err("above i16");
    assert_eq!(code(&error), "invalid_argument", "coordinate code");
    let wide_side =
        add_args(r#"{"v1": 0, "v2": 2, "front_side": {"sector": 0, "x_offset": 40000}}"#);
    let offsetless =
        add_in_place(&mut map, &wide_side, &spec_of(&wide_side)).expect_err("above i16");
    assert_eq!(code(&offsetless), "invalid_argument", "offset code");
    map.format = MapFormat::Udmf;
    let row = add_in_place(&mut map, &far, &spec_of(&far)).expect("UDMF stores the wide point");
    assert_eq!(row.detail.v1, 6, "the vertex was created anyway");
}

#[test]
fn new_sides_take_dash_and_zero_defaults_and_validated_names() {
    let over_long = NewSide {
        sector: 2,
        upper: Some("toolongtexture".to_owned()),
        middle: None,
        lower: None,
        x_offset: None,
        y_offset: None,
    };
    let error =
        new_sidedef(&over_long, MapFormat::Doom).expect_err("ten characters is over the cap");
    assert_eq!(code(&error), "invalid_argument", "texture names validate");
    let built = new_sidedef(
        &NewSide {
            sector: 1,
            upper: None,
            middle: Some("step1".to_owned()),
            lower: None,
            x_offset: Some(-4_i32),
            y_offset: None,
        },
        MapFormat::Doom,
    )
    .expect("a plain side builds");
    assert_eq!(built.upper, "-", "absent textures are the dash");
    assert_eq!(built.middle, "STEP1", "given ones uppercase");
    assert_eq!(
        (built.x_offset, built.y_offset),
        (-4_i32, 0_i32),
        "shifts land"
    );
    assert_eq!(built.sector, 1_usize, "the sector passes through");
}

#[test]
fn endpoint_resolution_reports_created_vertices_in_order() {
    let mut map = fixture(MapFormat::Doom);
    let both_new = add_args(
        r#"{"from": {"x": 64, "y": 64}, "to": {"x": -64, "y": 64}, "front_side": {"sector": 0}}"#,
    );
    let ends = place_endpoints(&mut map, &spec_of(&both_new)).expect("both points create");
    assert_eq!(
        (ends.v1, ends.v2),
        (6_usize, 7_usize),
        "appended in call order"
    );
    assert_eq!(ends.created, vec![6_usize, 7], "and reported as created");
}
