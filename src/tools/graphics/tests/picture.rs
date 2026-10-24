//! The image tools' own halves over the fixture session in [`super`]: the
//! picture options and their defaults, the frame, rotation and prefix
//! parsers, the `get_sprite` and `get_thing_sprite` selectors, the scaled
//! offsets and the `grAb` chunk, what a [`Prepared`] reports, the
//! fetched file's name and the thing definitions and preparation with their
//! errors.

use crate::session::Session;
use crate::tools::graphics::decode::display_name;
use crate::tools::graphics::get::{SpriteRequest, parse_frame, parse_rotation, sprite_request};
use crate::tools::graphics::list_sprites::{parse_prefix, parse_sprite};
use crate::tools::graphics::picture::{
    AssetFormat, ImageParams, Prepared, grab_of, parse_options, scale_offset,
};
use crate::tools::graphics::thing_sprite::{
    missing_thing_sprite, parse_thing_rotation, prepare_thing_sprite, session_game, thing_request,
    thing_sprite_def,
};
use crate::wad::archive::Lump;
use crate::wad::game::Game;

use super::{code, fixture, name, params, rendered, sprite_params, thing_params, thing_selection};

#[test]
fn picture_options_default_and_reject_a_bad_format_or_scale() {
    let plain = parse_options(&params(None, None, None)).expect("defaults are valid");
    assert_eq!(
        (plain.palette, plain.format, plain.scale),
        (0_usize, AssetFormat::Png, 1_u32),
        "the documented defaults"
    );
    let picked = parse_options(&params(Some(13), Some("rgba"), Some(8))).expect("in range");
    assert_eq!(
        (picked.palette, picked.format, picked.scale),
        (13_usize, AssetFormat::Rgba, 8_u32),
        "everything as asked"
    );
    let odd_format = parse_options(&params(None, Some("jpg"), None)).expect_err("no such encoding");
    assert_eq!(code(&odd_format), "invalid_argument", "bad format code");
    for factor in [0_u32, 9_u32] {
        let odd = parse_options(&params(None, None, Some(factor))).expect_err("out of range");
        assert_eq!(code(&odd), "invalid_argument", "scale {factor} code");
    }
    assert_eq!(
        (AssetFormat::Png.extension(), AssetFormat::Rgba.extension()),
        (("png", "image/png"), ("rgba", "application/octet-stream")),
        "the published extensions and media types"
    );
}

#[test]
fn frames_rotations_and_prefixes_are_normalised_and_checked() {
    assert_eq!(parse_frame(None).expect("the default"), 'A', "frame A");
    for (text, want) in [("b", 'B'), ("[", '['), ("\\", '\\'), ("]", ']')] {
        assert_eq!(
            parse_frame(Some(text)).expect("a frame letter"),
            want,
            "{text:?} is a frame"
        );
    }
    for bad in ["", "AB", "1", "-"] {
        let error = parse_frame(Some(bad)).expect_err("not a frame letter");
        assert_eq!(code(&error), "invalid_argument", "frame {bad:?} code");
    }
    assert_eq!(parse_rotation(None).expect("the default"), 1, "rotation 1");
    for allowed in [0_u8, 4, 8] {
        assert_eq!(
            parse_rotation(Some(allowed)).expect("in range"),
            allowed,
            "rotation {allowed}"
        );
    }
    let high = parse_rotation(Some(9)).expect_err("past eight");
    assert_eq!(code(&high), "invalid_argument", "rotation 9 code");
    assert_eq!(
        parse_sprite("poss").expect("four letters"),
        "POSS",
        "the prefix is uppercased"
    );
    assert_eq!(
        parse_prefix(Some("troo")).expect("four letters"),
        Some("TROO".to_owned()),
        "the listing shares the check"
    );
    for bad in ["", "POS", "PO!S", "TROO1"] {
        let error = parse_sprite(bad).expect_err("not four letters or digits");
        assert_eq!(code(&error), "invalid_argument", "prefix {bad:?} code");
    }
}

#[test]
fn a_sprite_call_names_one_lump_as_stored_or_one_rotation() {
    let stored = sprite_request(&sprite_params(Some("trooa2a8"), None)).expect("a name");
    assert!(
        matches!(stored, SpriteRequest::Lump(text) if text == name("TROOA2A8")),
        "the whole lump name is served as stored"
    );
    let parts = sprite_request(&sprite_params(None, Some("troo"))).expect("a sprite");
    let SpriteRequest::Rotation {
        name: wanted,
        sprite,
        frame,
        rotation,
    } = parts
    else {
        panic!("a sprite/frame/rotation request was asked for: {parts:?}");
    };
    assert_eq!(wanted, name("TROOA1"), "the defaults name the lump");
    assert_eq!(sprite, "TROO", "uppercased prefix");
    assert_eq!((frame, rotation), ('A', 1), "the documented defaults");
    let both =
        sprite_request(&sprite_params(Some("TROOA1"), Some("TROO"))).expect_err("two selectors");
    assert_eq!(code(&both), "invalid_argument", "both selectors code");
    let neither = sprite_request(&sprite_params(None, None)).expect_err("no selector");
    assert_eq!(code(&neither), "invalid_argument", "no selector code");
}

#[test]
fn offsets_scale_with_the_picture_and_only_a_grab_writes_grab() {
    assert_eq!(scale_offset(-2, 4), -8_i32, "the published offset");
    assert_eq!(scale_offset(8, 1), 8_i32, "an unscaled offset");
    assert_eq!(
        grab_of(&rendered(None)),
        None,
        "a flat carries no grAb chunk"
    );
    assert_eq!(
        grab_of(&rendered(Some((3_i32, -4_i32)))),
        Some((3_i32, -4_i32)),
        "both offsets or neither"
    );
}

#[test]
fn a_prepared_picture_reports_the_sprite_extras_only_for_a_sprite() {
    let flat = Prepared::plain(name("FLOOR1"), rendered(None));
    assert!(
        flat.lump.is_none() && flat.mirrored.is_none(),
        "a flat answers with its own name"
    );
    let sprite = Prepared::served(name("TROOA8"), name("TROOA2A8"), true, rendered(None));
    assert_eq!(
        sprite.name,
        name("TROOA8"),
        "the rotation that was asked for"
    );
    assert_eq!(
        sprite.lump,
        Some(name("TROOA2A8")),
        "the lump that served it"
    );
    assert_eq!(sprite.mirrored, Some(true), "and the flip it needed");
}

#[test]
fn a_fetched_file_is_named_from_its_stem() {
    assert_eq!(display_name("WALLP1.png"), "WALLP1", "a lump-name stem");
    assert_eq!(
        display_name("my-drawing.png"),
        "MYDRAWIN",
        "filtered and trimmed to eight characters"
    );
    assert_eq!(display_name("notes"), "NOTES", "no extension to strip");
    assert_eq!(
        display_name("?!"),
        "?!".to_owned(),
        "no lump name in it: the filename"
    );
    assert_eq!(display_name(""), "PATCH", "a source naming no file at all");
}

#[test]
fn thing_sprite_arguments_default_and_refuse_odd_selections() {
    let plain = thing_request(&thing_params(3001, None, None)).expect("a type alone");
    assert_eq!(
        (plain.kind, plain.frame, plain.rotation),
        (3001_u32, None, 1_u8),
        "the definition's frame and rotation 1"
    );
    let picked = thing_request(&thing_params(3001, Some("b"), Some(8))).expect("given parts");
    assert_eq!(
        (picked.frame, picked.rotation),
        (Some('B'), 8_u8),
        "the frame is uppercased"
    );
    let odd_frame = thing_request(&thing_params(3001, Some("1"), None)).expect_err("no frame");
    assert_eq!(code(&odd_frame), "invalid_argument", "frame code");
    assert!(
        thing_request(&thing_params(3001, None, Some(0)))
            .expect_err("rotation 0 is get_sprite's")
            .to_string()
            .contains("rotation"),
        "the message names the parameter"
    );
    for rotation in [0_u8, 9_u8] {
        let error = parse_thing_rotation(Some(rotation)).expect_err("outside the eight views");
        assert_eq!(code(&error), "invalid_argument", "rotation {rotation} code");
    }
    assert_eq!(
        parse_thing_rotation(None).expect("the default"),
        1,
        "rotation 1"
    );
}

#[test]
fn thing_definitions_follow_the_session_game_and_custom_sources() {
    assert_eq!(
        session_game(&Session::default()),
        Game::Doom2,
        "an unidentified session stands for DOOM II"
    );
    let session = fixture();
    let imp = thing_sprite_def(&session, 3001).expect("the built-in Imp");
    assert_eq!(
        (imp.sprite.as_str(), imp.frame, imp.name.as_str()),
        ("TROO", 'A', "Imp"),
        "the table row"
    );
    let unknown = thing_sprite_def(&session, 999_999).expect_err("no such type");
    assert_eq!(code(&unknown), "not_found", "unknown type code");
    assert!(
        unknown.to_string().contains("no thing type 999999"),
        "the message names the type: {unknown}"
    );
    let mut grown = fixture();
    grown
        .edit("decorate", |wad| {
            wad.lumps.push(Lump::new(
                name("DECORATE"),
                b"actor LampSprite 20010 {\n States {\n Spawn: PLSM A 10\n }\n}\n\
                  actor LampPlain 20014 { radius 10 }\n"
                    .to_vec(),
            ));
            Ok(())
        })
        .expect("plain append");
    let custom = thing_sprite_def(&grown, 20_010).expect("the DECORATE type");
    assert_eq!(
        (custom.sprite.as_str(), custom.frame, custom.source.as_str()),
        ("PLSM", 'A', "decorate"),
        "the session-aware overlay reaches the tool"
    );
    let no_sprite = thing_sprite_def(&grown, 20_014).expect_err("no sprite column");
    assert_eq!(code(&no_sprite), "not_found", "no-sprite code");
    assert!(
        no_sprite
            .to_string()
            .contains("thing type 20014 has no sprite"),
        "the message names the type: {no_sprite}"
    );
}

#[test]
fn thing_sprite_prep_reports_definition_lump_and_picture_errors() {
    let session = fixture();
    let options = parse_options(&ImageParams::default()).expect("the defaults");
    let unknown = prepare_thing_sprite(&session, &thing_selection(999_999, None, 1), &options)
        .err()
        .expect("no such type");
    assert_eq!(code(&unknown), "not_found", "unknown type code");
    let no_lumps = prepare_thing_sprite(&session, &thing_selection(3004, None, 1), &options)
        .err()
        .expect("the fixture has no POSS lumps");
    assert_eq!(code(&no_lumps), "not_found", "missing lumps code");
    assert!(
        no_lumps.to_string().contains("POSSA1 / POSSA0"),
        "the message names the lump pattern: {no_lumps}"
    );
    let broken = prepare_thing_sprite(&session, &thing_selection(3001, None, 1), &options)
        .err()
        .expect("the fixture's TROOA1 is a bare header");
    assert_eq!(
        code(&broken),
        "format_error",
        "an undecodable sprite stays a format error"
    );
}

#[test]
fn a_missing_thing_sprite_names_the_expected_lumps() {
    let error = missing_thing_sprite("TROO", 'A', 1);
    assert_eq!(code(&error), "not_found", "the helper's code");
    let message = error.to_string();
    assert!(
        message.contains("TROOA1 / TROOA0") && message.contains("list_sprites"),
        "the message names the lumps and the listing: {message}"
    );
}
