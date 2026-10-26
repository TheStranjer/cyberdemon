//! Unit tests of the patch group: the three tools' wire forms folded into
//! the shared import parameters, the passthrough staging of bytes that
//! already are a `Doom` patch, the merged-view scans that decide where a
//! replacement lands and which working lump a removal takes, the removal's
//! guarded slot deletion, and the single-lock lands that re-read those
//! scans, plan the `PNAMES` side and commit together.
//!
//! The fixtures are built with the library's own types: a session whose
//! working archive holds the region markers, a decodable 2x3 patch and, for
//! the `PNAMES` cases, a written list lump.

use serde_json::json;

use super::remove::{
    RemovalFlags, land_removal, remove_lump_at, working_patch_slot, working_removal_missing,
};
use super::write::{holds_patch, land_add_patch, land_replacement, located_patch};
use super::{
    AddPatchParams, PatchSourceParams, RemovePatchParams, ReplacePatchParams, Staged,
    looks_like_patch, passthrough_staged,
};
use crate::error::CyberError;
use crate::gfx::image::IndexedImage;
use crate::gfx::patch::{Patch, encode_patch};
use crate::session::{Layer, LayerRole, Session};
use crate::tools::imports::current_pnames;
use crate::tools::imports::image_pipeline::{
    DEFAULT_PALETTE, OffsetMode, OffsetsParam, TransparencyParams, parse_import,
};
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

/// The bytes of a 2x3 patch with header offsets `(-3, -2)`.
fn tiny_patch() -> Vec<u8> {
    let image = IndexedImage::new(2_u32, 3_u32, vec![1_u8; 6_usize], vec![true; 6_usize])
        .expect("2x3 is a valid shape");
    let patch = Patch {
        left_offset: -3_i16,
        top_offset: -2_i16,
        image,
    };
    encode_patch(&patch).expect("a 2x3 patch encodes")
}

/// A header-classified patch whose single column is a truncated post:
/// [`crate::wad::classify::is_doom_patch`] accepts it, decoding does
/// not.
fn corrupt_patch() -> Vec<u8> {
    vec![1_u8, 0, 1, 0, 0, 0, 0, 0, 12, 0, 0, 0, 0, 5]
}

/// A session whose working archive holds the lumps `names`, markers
/// included.
fn session_with(names: &[&str]) -> Session {
    let mut session = Session::default();
    let lumps: Vec<Lump> = names.iter().copied().map(region_lump).collect();
    session
        .edit("seed", move |wad| {
            wad.lumps.extend(lumps);
            Ok(())
        })
        .expect("plain append");
    session
}

/// The seed lump for one `text`: a marker for the region markers, a
/// decodable patch for everything else.
fn region_lump(text: &str) -> Lump {
    if text.starts_with("P_") || text.starts_with("F_") {
        return Lump::marker(name(text));
    }
    Lump::new(name(text), tiny_patch())
}

/// Arguments over a required name and one source path.
fn args_for(lump: &str, source: &str) -> AddPatchParams {
    AddPatchParams {
        name: lump.to_owned(),
        picture: PatchSourceParams {
            source: source.to_owned(),
            offsets: None,
            palette: None,
            transparency: None,
        },
        add_to_pnames: None,
    }
}

#[test]
fn the_wire_form_flattens_into_the_shared_import_parameters() {
    let args: AddPatchParams = serde_json::from_value(json!({
        "name": "myart",
        "source": "/x/y.png",
        "offsets": {"x": 3_i64, "y": -4_i64},
        "transparency": {"mode": "color", "color": "#FF0000"},
        "add_to_pnames": false,
    }))
    .expect("the wire form");
    assert_eq!(args.name, "myart", "the name stays as typed");
    assert_eq!(args.add_to_pnames, Some(false), "the flag carries");
    let import = parse_import(&args.common()).expect("valid");
    assert_eq!(
        import.name,
        Some(name("MYART")),
        "uppercased like any lump name"
    );
    assert_eq!(import.source, "/x/y.png", "the source carries");
    assert!(!import.replace, "add_patch never overwrites");
    assert_eq!(
        import.offsets,
        Some(OffsetMode::Explicit {
            left: 3_i32,
            top: -4_i32
        })
    );
}

#[test]
fn omitted_picture_options_take_the_pipeline_defaults() {
    let args = args_for("X", "/x/y.png");
    let common = args.common();
    assert_eq!(common.name.as_deref(), Some("X"), "the name pins in");
    assert!(!common.replace, "no overwrite parameter exists");
    let import = parse_import(&common).expect("valid");
    assert!(import.offsets.is_none(), "omitted means grab-then-zero");
    assert_eq!(import.palette, DEFAULT_PALETTE, "the normal palette");
    assert!(
        args.add_to_pnames.is_none(),
        "pnames_plan defaults the flag on"
    );
}

#[test]
fn a_bad_name_preset_or_mode_is_refused_before_any_work() {
    let long = args_for("TOOLONGNM", "/x/y.png");
    let name_error = parse_import(&long.common()).expect_err("nine characters");
    assert_eq!(code(&name_error), "invalid_argument");
    assert!(
        name_error.to_string().contains("name"),
        "the message names the parameter: {name_error}"
    );
    let mut preset = args_for("X", "/x/y.png");
    preset.picture.offsets = Some(OffsetsParam::Preset("middle".to_owned()));
    let preset_error = parse_import(&preset.common()).expect_err("not a preset");
    assert_eq!(code(&preset_error), "invalid_argument");
    assert!(
        preset_error.to_string().contains("center_bottom"),
        "the pipeline's message: {preset_error}"
    );
    let mut keyed = args_for("X", "/x/y.png");
    keyed.picture.transparency = Some(TransparencyParams {
        mode: Some("blur".to_owned()),
        threshold: None,
        color: None,
    });
    assert_eq!(
        code(&parse_import(&keyed.common()).expect_err("not a mode")),
        "invalid_argument"
    );
}

#[test]
fn holds_patch_reads_the_working_patches_region_only() {
    let region = session_with(&["P_START", "MYART", "P_END"]);
    assert!(
        holds_patch(&region, name("MYART")).expect("the scan runs"),
        "a region twin is held"
    );
    let loose = session_with(&["MYART"]);
    assert!(
        !holds_patch(&loose, name("MYART")).expect("the scan runs"),
        "a loose lump is not a region twin"
    );
    let flats = session_with(&["F_START", "MYART", "F_END"]);
    assert!(
        !holds_patch(&flats, name("MYART")).expect("the scan runs"),
        "another region is no twin either"
    );
    assert!(
        !holds_patch(&Session::default(), name("MYART")).expect("the scan runs"),
        "a fresh archive holds nothing"
    );
}

#[test]
fn only_decodable_doom_patch_bytes_pass_through() {
    assert!(
        looks_like_patch(&tiny_patch()),
        "a real patch passes through"
    );
    assert!(
        !looks_like_patch(b"this is a text file, not an image"),
        "junk runs the pipeline"
    );
    assert!(
        !looks_like_patch(&corrupt_patch()),
        "a header that will not decode runs the pipeline too"
    );
}

#[test]
fn passthrough_keeps_the_bytes_unless_the_offsets_are_explicit() {
    let bytes = tiny_patch();
    let kept = passthrough_staged(name("KEPT"), &bytes, None).expect("a real patch stages");
    assert_eq!(kept.data, bytes, "stored unchanged");
    assert!(!kept.converted, "the passthrough half ran");
    assert_eq!(
        (kept.width, kept.height),
        (2_u32, 3_u32),
        "the header's own shape"
    );
    assert_eq!(
        (kept.left_offset, kept.top_offset),
        (-3_i32, -2_i32),
        "the header's own offsets"
    );
    let centered =
        passthrough_staged(name("KEPT"), &bytes, Some(OffsetMode::Center)).expect("stages");
    assert_eq!(centered.data, bytes, "a preset never rewrites");
    let explicit = Some(OffsetMode::Explicit {
        left: 4_i32,
        top: -6_i32,
    });
    let moved =
        passthrough_staged(name("MOVED"), &bytes, explicit).expect("an explicit pair stages");
    assert_eq!(
        (moved.left_offset, moved.top_offset),
        (4_i32, -6_i32),
        "the rewritten pair reports itself"
    );
    assert_eq!(
        moved.data.get(..4),
        bytes.get(..4),
        "the header start survives"
    );
    assert_eq!(moved.data.get(8..), bytes.get(8..), "so does the payload");
    let far = passthrough_staged(
        name("FAR"),
        &bytes,
        Some(OffsetMode::Explicit {
            left: 40000_i32,
            top: 0_i32,
        }),
    )
    .expect_err("past the i16 range");
    assert_eq!(code(&far), "invalid_argument", "the range code");
    assert!(
        far.to_string().contains("does not fit"),
        "the pipeline's message: {far}"
    );
}

/// The seed helper for [`session_listed`]: a `PNAMES` lump instead of
/// a patch when the text is the list's own name.
fn listed_lump(text: &str, listed: &[&str]) -> Lump {
    if text != "PNAMES" {
        return region_lump(text);
    }
    let texts: Vec<String> = listed.iter().map(|entry| (*entry).to_owned()).collect();
    Lump::new(
        name("PNAMES"),
        write_pnames(&texts).expect("test names store"),
    )
}

/// A session whose working archive holds the lumps `names` (markers
/// included), with a `PNAMES` lump listing `listed` among them.
fn session_listed(names: &[&str], listed: &[&str]) -> Session {
    let mut session = Session::default();
    let lumps: Vec<Lump> = names
        .iter()
        .copied()
        .map(|text| listed_lump(text, listed))
        .collect();
    session
        .edit("seed", move |wad| {
            wad.lumps.extend(lumps);
            Ok(())
        })
        .expect("plain append");
    session
}

#[test]
fn located_patch_decides_the_write_from_the_merged_view() {
    let owned = session_listed(&["P_START", "MYART", "P_END", "PNAMES"], &["MYART"]);
    assert_eq!(
        located_patch(&owned, name("MYART")).expect("a region patch"),
        (true, Some(0_usize)),
        "the working archive owns it: in-place overwrite at the unchanged index"
    );
    let unlisted = session_listed(&["P_START", "MYART", "P_END"], &[]);
    assert_eq!(
        located_patch(&unlisted, name("MYART")).expect("a region patch"),
        (true, None),
        "the region copy is working-owned and an unlisted name answers no index"
    );
    let global = session_listed(&["MYART", "PNAMES"], &["MYART"]);
    assert_eq!(
        located_patch(&global, name("MYART")).expect("a PNAMES global"),
        (false, Some(0_usize)),
        "no region copy: the write inserts an override at the global's index"
    );
}

#[test]
fn an_unlocated_replace_target_refuses_naming_add_patch() {
    let empty = Session::default();
    let error = located_patch(&empty, name("NOPE")).expect_err("nothing stands");
    assert_eq!(code(&error), "not_found", "the documented code");
    assert!(
        error.to_string().contains("add_patch"),
        "the message names the way in: {error}"
    );
    let flats = session_listed(&["F_START", "MYART", "F_END", "PNAMES"], &["MYART"]);
    assert_eq!(
        code(&located_patch(&flats, name("MYART")).expect_err("a flat is no patch")),
        "not_found",
        "the flats and sprites regions never make a candidate"
    );
}

#[test]
fn the_replace_wire_form_flattens_the_picture_without_other_flags() {
    let args: ReplacePatchParams = serde_json::from_value(json!({
        "name": "wallp1",
        "source": "/x/y.png",
        "offsets": "grab",
        "palette": 3_usize,
    }))
    .expect("the wire form");
    assert_eq!(args.name, "wallp1", "the name stays as typed");
    let import = parse_import(&args.common()).expect("valid");
    assert_eq!(
        import.name,
        Some(name("WALLP1")),
        "uppercased like any lump name"
    );
    assert_eq!(import.source, "/x/y.png", "the source carries");
    assert_eq!(import.palette, 3_usize, "the palette carries");
    assert_eq!(import.offsets, Some(OffsetMode::Grab), "the preset carries");
    assert!(!import.replace, "the tool decides the write itself");
}

#[test]
fn the_remove_wire_form_carries_the_name_and_the_two_flags() {
    let plain: RemovePatchParams =
        serde_json::from_value(json!({ "name": "wallp1" })).expect("just the name");
    assert_eq!(plain.name, "wallp1", "the name stays as typed");
    assert_eq!(
        plain.remove_from_pnames, None,
        "the list flag is off by default"
    );
    assert_eq!(plain.force, None, "the force flag is off by default");
    let both: RemovePatchParams = serde_json::from_value(json!({
        "name": "WALLP1",
        "remove_from_pnames": true,
        "force": true,
    }))
    .expect("all three fields");
    assert_eq!(both.remove_from_pnames, Some(true), "the list flag carries");
    assert_eq!(both.force, Some(true), "the force flag carries");
    let long = LumpName::parse("TOOLONGNM").expect_err("nine characters");
    assert_eq!(
        code(&long),
        "invalid_argument",
        "the parse refusal run uses"
    );
}

#[test]
fn working_patch_slot_scans_the_region_and_the_pnames_globals() {
    let region = session_listed(&["P_START", "MYART", "P_END", "PNAMES"], &[]);
    assert_eq!(
        working_patch_slot(&region, name("MYART")).expect("the scan runs"),
        Some(1_usize),
        "the region lump at its directory index"
    );
    let global = session_listed(&["MYART", "PNAMES"], &["MYART"]);
    assert_eq!(
        working_patch_slot(&global, name("MYART")).expect("the scan runs"),
        Some(0_usize),
        "a PNAMES-listed global outside any region is a candidate"
    );
    let unlisted = session_listed(&["MYART"], &[]);
    assert_eq!(
        working_patch_slot(&unlisted, name("MYART")).expect("the scan runs"),
        None,
        "an unlisted loose lump is no candidate"
    );
    let flats = session_listed(&["F_START", "MYART", "F_END", "PNAMES"], &["MYART"]);
    assert_eq!(
        working_patch_slot(&flats, name("MYART")).expect("the scan runs"),
        None,
        "the flats and sprites regions never offer a patch"
    );
    let markers = session_listed(&["P_START", "P_END", "PNAMES"], &["P_START"]);
    assert_eq!(
        working_patch_slot(&markers, name("P_START")).expect("the scan runs"),
        None,
        "a marker is no candidate even when PNAMES oddly lists one"
    );
    let twice = session_listed(
        &["MYART", "P_START", "MYART", "P_END", "PNAMES"],
        &["MYART"],
    );
    assert_eq!(
        working_patch_slot(&twice, name("MYART")).expect("the scan runs"),
        Some(2_usize),
        "the last working copy wins"
    );
}

#[test]
fn remove_lump_at_takes_the_named_slot_and_refuses_others() {
    let mut session = session_with(&["P_START", "MYART", "P_END"]);
    session
        .edit("cut", |wad| remove_lump_at(wad, name("MYART"), 1))
        .expect("the named slot leaves");
    let names: Vec<String> = session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect();
    assert_eq!(
        names,
        vec!["P_START".to_owned(), "P_END".to_owned()],
        "the markers stay, so the region stays closed"
    );
    let stale = session
        .edit("bad cut", |wad| remove_lump_at(wad, name("MYART"), 1))
        .expect_err("P_END stands at index 1 now");
    assert_eq!(code(&stale), "internal", "the stale-index guard bites");
    assert_eq!(
        session.undo_labels(),
        vec!["cut", "seed"],
        "the refused cut left no step"
    );
}

#[test]
fn the_missing_working_removal_hint_names_replace_patch() {
    let error = working_removal_missing(name("WALLP1"));
    assert_eq!(code(&error), "not_found", "the documented code");
    let message = error.to_string();
    assert!(
        message.contains("replace_patch") && message.contains("working"),
        "the hint says what only works on working lumps: {message}"
    );
}

/// Lump and `PNAMES` entry both go, without overriding a `conflict`.
fn list_flags() -> RemovalFlags {
    RemovalFlags {
        from_pnames: true,
        force: false,
    }
}

/// The working lump only: the list is left as it stands.
fn lump_only_flags() -> RemovalFlags {
    RemovalFlags {
        from_pnames: false,
        force: false,
    }
}

/// Staged bytes over `text`: the tiny fixture patch with its header facts.
fn staged(text: &str) -> Staged {
    Staged {
        name: name(text),
        data: tiny_patch(),
        converted: true,
        width: 2_u32,
        height: 3_u32,
        left_offset: -3_i32,
        top_offset: -2_i32,
    }
}

#[test]
fn two_add_lands_after_both_stagings_keep_both_pnames_entries() {
    // Both pictures stage (fetch, decode) outside every lock first; B's
    // whole land then mutates the session between A's staging and A's
    // commit. Plan and commit share the one land lock, so A reads the list
    // B grew and neither entry is written over.
    let mut session = session_listed(&["PNAMES"], &["WALLP1"]);
    let landed_b = land_add_patch(&mut session, staged("MYARTB"), None).expect("B lands");
    let landed_a = land_add_patch(&mut session, staged("MYARTA"), None).expect("A lands");
    assert_eq!(
        landed_b.pnames_index,
        Some(1_usize),
        "B appended after the fixture name"
    );
    assert_eq!(landed_a.pnames_index, Some(2_usize), "A appended after B");
    assert_eq!(
        current_pnames(&session).expect("the grown list decodes"),
        vec![
            "WALLP1".to_owned(),
            "MYARTB".to_owned(),
            "MYARTA".to_owned()
        ],
        "neither land overwrote the other's entry"
    );
}

#[test]
fn a_twin_placed_after_the_precheck_refuses_the_add_land() {
    // The pre-fetch twin scan cannot see what a concurrent add placed in
    // between; the land's own guard catches it before anything is written.
    let mut session = Session::default();
    land_add_patch(&mut session, staged("MYART"), None).expect("the first lands");
    let error = land_add_patch(&mut session, staged("MYART"), None).expect_err("the twin stands");
    assert_eq!(code(&error), "already_exists");
    assert!(
        error.to_string().contains("replace_patch"),
        "the message names the way out: {error}"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["add_patch MYART"],
        "the second land wrote nothing"
    );
}

/// A session on an `IWAD` layer whose `patches` region holds `MYART` with
/// a `PNAMES` entry, for the replacement lands.
fn layer_patch_session() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            "base.wad".into(),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::marker(name("P_START")),
                    Lump::new(name("MYART"), tiny_patch()),
                    Lump::marker(name("P_END")),
                    Lump::new(
                        name("PNAMES"),
                        write_pnames(&["MYART".to_owned()]).expect("test names store"),
                    ),
                ],
            },
        ))
        .expect("no batch is open");
    session
}

#[test]
fn a_replace_land_overrides_the_layer_then_overwrites_its_own_copy() {
    // The write's decision (in place or as an override, at which unchanged
    // `PNAMES` index) is re-read inside the land's own lock, so the second
    // land sees the working copy the first one placed.
    let mut session = layer_patch_session();
    let first = land_replacement(&mut session, staged("MYART")).expect("the override lands");
    assert!(
        first.created_override,
        "the working archive had no copy yet"
    );
    assert_eq!(
        first.patch.pnames_index,
        Some(0_usize),
        "the unchanged entry rides along"
    );
    let second = land_replacement(&mut session, staged("MYART")).expect("the overwrite lands");
    assert!(
        !second.created_override,
        "the land re-read its own working copy"
    );
    assert_eq!(second.lump_index, first.lump_index, "overwritten in place");
    assert_eq!(
        session.undo_labels(),
        vec![
            "replace_patch MYART".to_owned(),
            "replace_patch MYART".to_owned()
        ],
        "one undoable step each"
    );
}

#[test]
fn one_removal_land_takes_the_slot_and_the_list_together() {
    // Scan, plan and commit run under one lock, so the index the guarded
    // deletion takes is the entry the scan found in the directory as the
    // commit finds it.
    let mut session = session_listed(&["P_START", "MYART", "P_END", "PNAMES"], &["MYART"]);
    let outcome =
        land_removal(&mut session, name("MYART"), list_flags()).expect("the removal lands");
    assert!(
        outcome.removed_lump && outcome.removed_from_pnames,
        "lump and entry went together"
    );
    assert_eq!(
        outcome.textures_changed, 0_usize,
        "no texture referred to it"
    );
    assert!(
        current_pnames(&session)
            .expect("the emptied list decodes")
            .is_empty(),
        "the entry left the list"
    );
    assert!(
        session
            .find(name("MYART"), Some(Namespace::Patches))
            .is_none(),
        "the lump left the merged view"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["remove_patch MYART", "seed"],
        "one undoable step"
    );
}

#[test]
fn a_pnames_only_removal_land_still_removes_the_entry() {
    let mut session = session_listed(&["PNAMES"], &["MYART"]);
    let outcome = land_removal(&mut session, name("MYART"), list_flags()).expect("the entry goes");
    assert!(!outcome.removed_lump, "no working lump stood there");
    assert!(outcome.removed_from_pnames, "the entry went alone");
}

#[test]
fn a_removal_land_refuses_a_name_nothing_holds() {
    let mut session = Session::default();
    let error =
        land_removal(&mut session, name("MYART"), lump_only_flags()).expect_err("nothing there");
    assert_eq!(code(&error), "not_found", "the documented code");
    assert!(
        session.undo_labels().is_empty(),
        "the refused land recorded nothing"
    );
}
