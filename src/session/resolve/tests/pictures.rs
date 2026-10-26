//! Picture resolution tests: which lump wins a flat, patch or sprite
//! lookup, how `Doom` and `PNG` bytes decode, how rotations serve, flip
//! and cache, and what the not-found refusals report.

use std::collections::HashMap;

use super::{
    code, doom_patch, first_index, flat_png, lump, name, png, request, session, stripe, with_extra,
};
use crate::gfx::image::IndexedImage;
use crate::gfx::patch::Patch;
use crate::session::Session;
use crate::session::resolve::Entry;
use crate::session::resolve::sprite::{clamp_offset, find_rotation, mirror_patch, sprite_entries};
use crate::session::resolve::{
    Place, Serving, decode_patch_bytes, load_flat, load_patch, load_sprite, load_sprite_cached,
    load_sprite_lump, locate, lump_bytes, merged_rows, not_found,
};
use crate::wad::archive::Lump;
use crate::wad::classify::Namespace;

/// The sprite entries of the fixture session.
fn rows(session: &Session) -> Vec<Entry> {
    sprite_entries(session).expect("the fixture lists")
}

#[test]
fn flats_resolve_the_region_copy_first_and_the_last_of_a_name() {
    let session = session();
    assert_eq!(
        first_index(&load_flat(&session, name("FLOOR1")).expect("FLOOR1 is a flat")),
        2,
        "the last FLOOR1 wins"
    );
    assert_eq!(
        load_flat(&session, name("LOOSE1"))
            .expect("a loose flat is the second chance")
            .width,
        64,
        "4096 bytes is 64x64"
    );
    let missing = load_flat(&session, name("NOPE")).expect_err("no such flat");
    assert_eq!(code(&missing), "not_found", "missing flat code");
    assert!(
        missing.to_string().contains("list_flats"),
        "the message names the listing: {missing}"
    );
}

#[test]
fn a_png_flat_is_quantized_to_palette_zero() {
    let session = session();
    let flat = load_flat(&session, name("PNGFLAT")).expect("the PNG flat decodes");
    assert_eq!((flat.width, flat.height), (2, 2), "the PNG shape");
    assert_eq!(first_index(&flat), 64, "(64,191,64) is colour 64");
    assert!(
        flat.opaque.iter().all(|visible| *visible),
        "opaque pixels stay opaque"
    );
}

#[test]
fn a_png_pixel_below_the_alpha_threshold_stays_a_hole() {
    let hole = flat_png(1, 1, [0_u8, 0, 0, 0], None);
    let session = with_extra(vec![lump("HOLE", hole)]);
    let image = load_flat(&session, name("HOLE")).expect("the PNG flat decodes");
    assert_eq!(
        image.get(0, 0).map(|(_, visible)| visible),
        Some(false),
        "alpha 0 is a hole"
    );
}

#[test]
fn patches_prefer_the_region_and_reject_unreadable_bytes() {
    let session = session();
    let wall = load_patch(&session, name("WALLP1")).expect("the region patch");
    assert_eq!(
        (wall.left_offset, wall.top_offset),
        (-2, -14),
        "header offsets survive"
    );
    assert_eq!(wall.image.width, 4, "the stored shape");
    let junk = load_patch(&session, name("JUNK")).expect_err("40 x's are no patch");
    assert_eq!(code(&junk), "format_error", "undecodable patch code");
    let missing = load_patch(&session, name("NOPE")).expect_err("no such patch");
    assert!(
        missing.to_string().contains("list_patches"),
        "the message names the listing: {missing}"
    );
}

#[test]
fn loose_png_patches_quantize_and_carry_their_grab_offsets() {
    let session = session();
    let loose = load_patch(&session, name("PNGPAT")).expect("the loose PNG");
    assert_eq!((loose.left_offset, loose.top_offset), (3, -4), "from grAb");
    assert_eq!(
        (loose.image.width, loose.image.height),
        (3, 2),
        "the PNG shape"
    );
    let no_grab = png(2, 1, [0x10, 0x20, 0x30], None);
    let grown = with_extra(vec![lump("NOGRAB", no_grab)]);
    assert_eq!(
        load_patch(&grown, name("NOGRAB"))
            .expect("the PNG decodes")
            .left_offset,
        0,
        "no grAb is offset 0"
    );
}

#[test]
fn rotations_find_their_exact_mirrored_or_all_angles_lump() {
    let directory = rows(&session());
    let one = find_rotation(&directory, &request("TROO", 'A', 1)).expect("A1 is stored");
    assert_eq!((one.0.name, one.1), (name("TROOA1"), Serving::Direct));
    let eight = find_rotation(&directory, &request("TROO", 'A', 8)).expect("A2A8 mirrors it");
    assert_eq!(
        (eight.0.name, eight.1),
        (name("TROOA2A8"), Serving::Mirrored),
        "the pair name serves rotation 8"
    );
    let seven = find_rotation(&directory, &request("TROO", 'A', 7));
    assert!(seven.is_none(), "nothing serves rotation 7");
    let angles = find_rotation(&directory, &request("PLAY", 'A', 5)).expect("PLAYA0");
    assert_eq!(angles.0.name, name("PLAYA0"), "the all-angles image");
    let none = find_rotation(&directory, &request("PLAY", 'A', 0)).expect("PLAYA0 itself");
    assert_eq!(none.0.name, name("PLAYA0"), "the exact name is rotation 0");
}

/// An `IWAD` writes a frame's rotations `2`, `3` and `4` into its
/// mirror-pair lumps only, so those rotations have no three-character
/// lump to find: the pair name has to serve them as its own image.
#[test]
fn a_pair_name_serves_its_first_rotation_without_flipping() {
    let session = with_extra(vec![
        Lump::marker(name("S_START")),
        lump("TROOA3A7", stripe(4, 1, (1, 2))),
        Lump::marker(name("S_END")),
    ]);
    let directory = rows(&session);
    let two = find_rotation(&directory, &request("TROO", 'A', 2)).expect("A2A8 serves rotation 2");
    assert_eq!(
        (two.0.name, two.1),
        (name("TROOA2A8"), Serving::Direct),
        "the pair's own rotation is used as stored"
    );
    let three =
        find_rotation(&directory, &request("TROO", 'A', 3)).expect("A3A7 serves rotation 3");
    assert_eq!((three.0.name, three.1), (name("TROOA3A7"), Serving::Direct));
    let seven =
        find_rotation(&directory, &request("TROO", 'A', 7)).expect("A3A7 mirrors rotation 7");
    assert_eq!(
        (seven.0.name, seven.1),
        (name("TROOA3A7"), Serving::Mirrored)
    );
    let sprite = load_sprite(&session, "TROO", 'A', 2).expect("rotation 2 loads");
    assert_eq!(sprite.lump, name("TROOA2A8"), "served by the pair lump");
    assert!(!sprite.mirrored, "as stored, not flipped");
}

#[test]
fn a_frame_letter_outside_a_lump_name_is_an_invalid_argument() {
    let session = session();
    let error = load_sprite(&session, "TROO", ' ', 1).expect_err("a space is no name");
    assert_eq!(code(&error), "invalid_argument", "bad frame code");
    let missing = load_sprite(&session, "BOGUS12", 'A', 1).expect_err("not four characters");
    assert_eq!(
        code(&missing),
        "invalid_argument",
        "a nine-character name is refused"
    );
}

#[test]
fn sprites_decode_flip_and_report_their_lump() {
    let session = session();
    let direct = load_sprite(&session, "troo", 'a', 1).expect("TROOA1 serves rotation 1");
    assert!(
        !direct.mirrored && direct.lump == name("TROOA1"),
        "case-insensitive, stored as is"
    );
    assert_eq!(
        (direct.patch.left_offset, direct.patch.top_offset),
        (4, 0),
        "the header's offsets"
    );
    let flipped = load_sprite(&session, "TROO", 'A', 8).expect("the mirror pair answers");
    assert!(flipped.mirrored, "the pair is flipped");
    assert_eq!(flipped.lump, name("TROOA2A8"), "named for rotation 2");
    assert_eq!(
        (flipped.patch.left_offset, flipped.patch.top_offset),
        (3, 2),
        "left' = width - left = 4 - 1"
    );
    assert_eq!(
        flipped.patch.image.get(3, 0),
        Some((5, true)),
        "the only opaque column moved from 0 to the right edge"
    );
    let all = load_sprite(&session, "PLAY", 'A', 3).expect("the all-angles image");
    assert_eq!(all.lump, name("PLAYA0"), "rotation 0 serves every angle");
    assert!(!all.mirrored, "used as stored");
    let missing = load_sprite(&session, "PLAY", 'B', 1).expect_err("no frame B");
    assert_eq!(code(&missing), "not_found", "missing sprite code");
    assert!(
        missing.to_string().contains("list_sprites"),
        "the message names the listing: {missing}"
    );
}

#[test]
fn mirroring_clamps_offsets_a_stored_width_cannot_reach() {
    let stored = Patch {
        left_offset: i16::MIN,
        top_offset: -1,
        image: IndexedImage::new(4, 1, vec![0_u8; 4], vec![false; 4])
            .expect("4x1 is a valid shape"),
    };
    let flipped = mirror_patch(&stored);
    assert_eq!(flipped.left_offset, i16::MAX, "4 - (-32768) saturates");
    assert_eq!(flipped.top_offset, -1, "the top offset is untouched");
    assert_eq!(clamp_offset(-1_i32), -1, "an offset inside i16 survives");
}

#[test]
fn a_sprite_region_lump_that_is_no_picture_fails_as_a_format_error() {
    let session = with_extra(vec![
        Lump::marker(name("S_START")),
        lump("TROOA3", vec![b'?'; 60]),
        Lump::marker(name("S_END")),
    ]);
    let error = load_sprite(&session, "TROO", 'A', 3).expect_err("junk is not a patch");
    assert_eq!(code(&error), "format_error", "undecodable sprite code");
}

#[test]
fn the_render_cache_decodes_each_lump_once_and_stores_it_unflipped() {
    let session = session();
    let defs = crate::defs::definitions_for(crate::wad::game::Game::Doom2)
        .expect("the embedded tables are valid");
    let imp = defs.thing(3001).expect("the embedded imp");
    let mut cache = HashMap::new();
    let first = load_sprite_cached(&session, imp, 8, &mut cache).expect("A2A8 flips");
    let second = load_sprite_cached(&session, imp, 8, &mut cache).expect("from cache");
    assert!(
        first.mirrored && second.mirrored,
        "rotation 8 serves flipped"
    );
    assert_eq!(
        (second.patch.left_offset, second.patch.image.get(3, 0)),
        (3, Some((5, true))),
        "the cached copy flips the same way the decoded one did"
    );
    let stored = cache.get(&name("TROOA2A8")).expect("the lump is cached");
    assert_eq!(
        (stored.left_offset, stored.image.get(0, 0)),
        (1, Some((5, true))),
        "the cache holds the patch as stored, not as served"
    );
    assert_eq!(cache.len(), 1, "one decode serves both calls");
}

#[test]
fn decoders_read_the_doom_format_before_the_png_one() {
    let session = session();
    let bytes = doom_patch(2, 2, (1, 2), 0x22);
    let stored = decode_patch_bytes(&session, &bytes).expect("a Doom patch decodes");
    assert_eq!(
        (stored.left_offset, stored.top_offset),
        (1, 2),
        "header offsets"
    );
    let png_bytes = png(1, 2, [0x11, 0x11, 0x11], Some((-1_i32, 9_i32)));
    let as_png = decode_patch_bytes(&session, &png_bytes).expect("a PNG decodes");
    assert_eq!(as_png.left_offset, -1, "grAb offsets");
    assert_eq!(as_png.image.height, 2, "the PNG shape");
}

#[test]
fn locate_and_lump_bytes_answer_for_any_winning_lump() {
    let session = session();
    let missing = lump_bytes(&session, name("PNAMES")).expect_err("the fixture has none");
    assert_eq!(code(&missing), "not_found", "missing lump code");
    let bytes = lump_bytes(&session, name("PLAYPAL")).expect("a global resolves");
    assert_eq!(bytes.len(), 768_usize.saturating_mul(14), "the whole lump");
    let directory = merged_rows(&session).expect("the fixture lists");
    assert!(
        locate(&directory, name("PLAYPAL"), &[Place::Anywhere]).is_some(),
        "an anywhere place matches a global"
    );
    assert!(
        locate(&directory, name("PLAYPAL"), &[Place::In(Namespace::Flats)]).is_none(),
        "a wrong region matches nothing"
    );
    assert!(
        locate(&directory, name("S_START"), &[Place::Anywhere]).is_none(),
        "a marker is never a picture"
    );
    let error = not_found("flat", "list_flats", name("NOPE"));
    assert_eq!(code(&error), "not_found", "the helper's code");
}

#[test]
fn a_named_sprite_lump_is_served_exactly_as_stored() {
    let session = session();
    let stored = load_sprite_lump(&session, name("TROOA2A8")).expect("the name resolves");
    assert_eq!(
        (stored.left_offset, stored.top_offset),
        (1, 2),
        "not mirrored"
    );
    assert_eq!(
        stored.image.get(0, 0),
        Some((5, true)),
        "the opaque column stays at the left edge"
    );
    let missing = load_sprite_lump(&session, name("NOPEA1")).expect_err("no such lump");
    assert_eq!(code(&missing), "not_found", "missing name code");
    assert!(
        missing.to_string().contains("list_sprites"),
        "the message names the listing: {missing}"
    );
}
