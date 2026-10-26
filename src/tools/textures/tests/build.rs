//! `build_texture` and `remove_texture` over the fixture set: the record
//! [`build_texture_run`] appends, the `PNAMES` names it brings in, the
//! refusals it answers with, what [`remove_texture_run`] strips from both
//! lumps and the warnings a non-power-of-two width earns.

use crate::session::Session;
use crate::tools::textures::WhichLump;
use crate::tools::textures::build::{
    BuildTextureParams, RemoveTextureParams, build_texture_run, remove_texture_run,
};
use crate::tools::textures::list::texture_rows;
use crate::wad::archive::Lump;

use super::{build_args, code, name, session, session_with_shadow, set_of, working_names};

#[test]
fn a_build_appends_the_texture_and_records_one_edit() {
    let mut session = session();
    let args = build_args("testx", 64, 32, &[("WALLP1", 0, 0)]);
    let report = build_texture_run(&mut session, &args).expect("the build lands");
    assert_eq!(report.name, "TESTX", "uppercased");
    assert_eq!(report.lump, "TEXTURE1", "the default target");
    assert!(report.created, "the working archive gained TEXTURE1");
    assert!(report.pnames_added.is_empty(), "WALLP1 is already listed");
    assert!(report.warnings.is_empty(), "64 is a power of two");
    assert_eq!(working_names(&session), vec!["TEXTURE1"], "one override");
    assert_eq!(
        session.undo_labels(),
        vec!["build_texture TESTX"],
        "one labelled edit"
    );
    assert!(session.is_dirty(), "and unsaved work");
    let loaded = set_of(&session);
    assert_eq!(
        loaded.pnames,
        vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
        "PNAMES stands in the merged view"
    );
    let (found, which) = loaded.find("TESTX").expect("the new texture");
    assert_eq!(which, WhichLump::Texture1, "in the first lump");
    assert_eq!((found.width, found.height), (64, 32), "the requested size");
    assert_eq!(found.patches.len(), 1, "the placement");
    assert_eq!(
        found.patches.first().expect("one").patch,
        0,
        "WALLP1's index"
    );
    session.undo().expect("undo the whole build");
    assert!(set_of(&session).find("TESTX").is_none(), "gone again");
    assert!(session.working().lumps.is_empty(), "no override left");
}

#[test]
fn a_build_appends_unlisted_patch_names_to_pnames() {
    let mut session = session();
    session
        .edit("add NEWP1", |wad| {
            wad.lumps.push(Lump::new(name("NEWP1"), vec![b'N']));
            Ok(())
        })
        .expect("a patch lump arrives");
    let args = build_args("MIXED", 32, 32, &[("NEWP1", 0, 0), ("WALLP1", 16, 0)]);
    let report = build_texture_run(&mut session, &args).expect("the build lands");
    assert_eq!(report.pnames_added, vec!["NEWP1".to_owned()], "one append");
    assert_eq!(
        working_names(&session),
        vec!["NEWP1", "PNAMES", "TEXTURE1"],
        "PNAMES and TEXTURE1 were copied in"
    );
    let loaded = set_of(&session);
    assert_eq!(
        loaded.pnames,
        vec!["WALLP1".to_owned(), "STEPP1".to_owned(), "NEWP1".to_owned()],
        "the list grew at the end"
    );
    let (found, _which) = loaded.find("MIXED").expect("the new texture");
    assert_eq!(
        found.patches.iter().map(|p| p.patch).collect::<Vec<u16>>(),
        vec![2_u16, 0],
        "the new name is the last index, the old one stands"
    );
}

#[test]
fn an_unbacked_patch_is_not_found_unless_allowed() {
    let mut session = session();
    let args = build_args("GHOST", 16, 16, &[("GHOSTP", 0, 0)]);
    let error = build_texture_run(&mut session, &args).expect_err("no GHOSTP lump");
    assert_eq!(code(&error), "not_found", "missing patch code");
    assert!(
        error.to_string().contains("GHOSTP"),
        "the message names the patch: {error}"
    );
    assert!(
        session.working().lumps.is_empty(),
        "the refusal wrote nothing"
    );
    assert!(session.undo_labels().is_empty(), "and recorded nothing");
    let allowed = BuildTextureParams {
        allow_missing_patches: Some(true),
        ..args
    };
    let report = build_texture_run(&mut session, &allowed).expect("the escape runs");
    assert_eq!(
        report.pnames_added,
        vec!["GHOSTP".to_owned()],
        "the name still enters PNAMES"
    );
    assert!(
        set_of(&session).find("GHOST").is_some(),
        "the texture is defined"
    );
}

#[test]
fn a_taken_name_conflicts_until_replaced_in_place() {
    let mut session = session();
    let args = build_args("wall1", 8, 8, &[("WALLP1", 0, 0)]);
    let error = build_texture_run(&mut session, &args).expect_err("WALL1 stands");
    assert_eq!(code(&error), "already_exists", "duplicate code");
    assert!(
        error.to_string().contains("replace=true"),
        "the message names the escape: {error}"
    );
    let replace = BuildTextureParams {
        replace: Some(true),
        ..args
    };
    let report = build_texture_run(&mut session, &replace).expect("the replace runs");
    assert!(
        report.created,
        "TEXTURE1 was not in the working archive yet"
    );
    let loaded = set_of(&session);
    let (wall, which) = loaded.find("WALL1").expect("still defined");
    assert_eq!(which, WhichLump::Texture1, "the same lump");
    assert_eq!((wall.width, wall.height), (8, 8), "the new size");
    let rows = texture_rows(&loaded);
    assert_eq!(rows.len(), 2, "no third record");
    assert_eq!(rows[0].name, "WALL1", "the position was kept");
    let again = build_texture_run(&mut session, &replace).expect("a second replace");
    assert!(!again.created, "TEXTURE1 is already in the working archive");
}

#[test]
fn an_explicit_texture2_target_creates_the_lump() {
    let mut session = session();
    let args = BuildTextureParams {
        target: Some("TEXTURE2".to_owned()),
        ..build_args("NEWONE", 32, 32, &[("WALLP1", 0, 0)])
    };
    let report = build_texture_run(&mut session, &args).expect("the build lands");
    assert_eq!(report.lump, "TEXTURE2", "the explicit target");
    assert!(report.created, "no archive held TEXTURE2");
    assert_eq!(working_names(&session), vec!["TEXTURE2"], "one new lump");
    let loaded = set_of(&session);
    assert_eq!(
        texture_rows(&loaded).len(),
        3,
        "the fixture pair plus the newcomer"
    );
    let (_wall, which) = loaded.find("NEWONE").expect("defined");
    assert_eq!(which, WhichLump::Texture2, "in the new lump");
}

#[test]
fn a_target_lump_can_shadow_a_name_the_other_lump_holds() {
    let mut session = session();
    let args = BuildTextureParams {
        target: Some("TEXTURE2".to_owned()),
        ..build_args("WALL1", 16, 16, &[("WALLP1", 0, 0)])
    };
    let report = build_texture_run(&mut session, &args).expect("TEXTURE2 holds no WALL1");
    assert_eq!(report.lump, "TEXTURE2", "the explicit target");
    assert!(report.created, "TEXTURE2 did not exist yet");
    let loaded = set_of(&session);
    let (wall, which) = loaded.find("WALL1").expect("still defined");
    assert_eq!(which, WhichLump::Texture2, "the new entry wins the name");
    assert_eq!((wall.width, wall.height), (16, 16), "the shadowing size");
    // Removal then strips the shadowed copy out of TEXTURE1 too.
    remove_texture_run(
        &mut session,
        &RemoveTextureParams {
            name: "WALL1".to_owned(),
        },
    )
    .expect("strips both lumps");
    let after = set_of(&session);
    assert!(after.find("WALL1").is_none(), "gone from TEXTURE1 as well");
    assert_eq!(
        texture_rows(&after).len(),
        1,
        "only STEP1 stands in TEXTURE1"
    );
}

#[test]
fn removing_a_shadowed_name_strips_every_lump_that_holds_it() {
    let mut session = session_with_shadow();
    let report = remove_texture_run(
        &mut session,
        &RemoveTextureParams {
            name: "WALL1".to_owned(),
        },
    )
    .expect("the remove lands");
    assert_eq!(report.name, "WALL1", "the uppercased name");
    assert_eq!(report.lump, "TEXTURE2", "the winning lump is reported");
    assert_eq!(
        working_names(&session),
        vec!["TEXTURE2", "TEXTURE1"],
        "TEXTURE2 was rewritten in place, TEXTURE1 was copied in"
    );
    assert_eq!(
        session.undo_labels(),
        vec!["remove_texture WALL1", "add TEXTURE2"],
        "one labelled edit"
    );
    let loaded = set_of(&session);
    assert!(loaded.find("WALL1").is_none(), "gone from both lumps");
    assert_eq!(
        loaded.pnames,
        vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
        "PNAMES is not touched"
    );
    assert_eq!(texture_rows(&loaded).len(), 1, "only STEP1 stands");
}

#[test]
fn remove_texture_refuses_names_no_lump_defines() {
    let mut session = session();
    let error = remove_texture_run(
        &mut session,
        &RemoveTextureParams {
            name: "NOPE".to_owned(),
        },
    )
    .expect_err("no such texture");
    assert_eq!(code(&error), "not_found", "unknown name code");
    assert!(
        error.to_string().contains("list_textures"),
        "the message points at the listing: {error}"
    );
    assert!(
        session.working().lumps.is_empty(),
        "the refusal wrote nothing"
    );
    assert!(session.undo_labels().is_empty(), "and recorded nothing");
}

#[test]
fn the_build_validates_names_sizes_and_the_patch_list() {
    let mut session = session();
    let cases: Vec<BuildTextureParams> = Vec::from([
        build_args("TOO LONG", 8, 8, &[("WALLP1", 0, 0)]),
        build_args("", 8, 8, &[("WALLP1", 0, 0)]),
        build_args("TESTX", 0, 8, &[("WALLP1", 0, 0)]),
        build_args("TESTX", 8, 4097, &[("WALLP1", 0, 0)]),
        build_args("TESTX", 8, 8, &[("bad name", 0, 0)]),
        BuildTextureParams {
            target: Some("TEXTURES".to_owned()),
            ..build_args("TESTX", 8, 8, &[("WALLP1", 0, 0)])
        },
    ]);
    for args in cases {
        let error = build_texture_run(&mut session, &args).expect_err("invalid call");
        assert_eq!(code(&error), "invalid_argument", "code for {error}");
    }
    let empty = BuildTextureParams {
        patches: Vec::new(),
        ..build_args("TESTX", 8, 8, &[("WALLP1", 0, 0)])
    };
    let empty_error = build_texture_run(&mut session, &empty).expect_err("no patches");
    assert_eq!(code(&empty_error), "invalid_argument", "empty patches code");
    assert!(
        empty_error.to_string().contains("at least one"),
        "the message states the rule: {empty_error}"
    );
    assert!(
        session.working().lumps.is_empty(),
        "no refusal wrote anything"
    );
    assert!(
        session.undo_labels().is_empty(),
        "and none recorded history"
    );
}

#[test]
fn a_non_power_of_two_width_warns_but_builds() {
    let mut session = session();
    let args = build_args("ODD", 48, 16, &[("WALLP1", 0, 0)]);
    let report = build_texture_run(&mut session, &args).expect("a warning is not a refusal");
    assert_eq!(report.warnings.len(), 1, "one note: {:?}", report.warnings);
    assert!(
        report
            .warnings
            .first()
            .expect("one")
            .contains("power of two"),
        "the warning explains the tiling: {}",
        report.warnings.join("; ")
    );
    assert!(
        set_of(&session).find("ODD").is_some(),
        "the texture was built"
    );
}

#[test]
fn a_build_creates_pnames_and_texture1_from_nothing() {
    let mut bare = Session::default();
    bare.edit("add PATCH1", |wad| {
        wad.lumps.push(Lump::new(name("PATCH1"), vec![b'P']));
        Ok(())
    })
    .expect("a bare patch lump");
    let args = build_args("FRESH", 16, 16, &[("PATCH1", 0, 0)]);
    let report = build_texture_run(&mut bare, &args).expect("both lumps are created");
    assert_eq!(
        working_names(&bare),
        vec!["PATCH1", "PNAMES", "TEXTURE1"],
        "PNAMES and TEXTURE1 arrived as fresh Doom lumps"
    );
    assert_eq!(report.pnames_added, vec!["PATCH1".to_owned()], "listed now");
    let loaded = set_of(&bare);
    assert_eq!(loaded.pnames, vec!["PATCH1".to_owned()], "the new list");
    let (found, which) = loaded.find("FRESH").expect("defined");
    assert_eq!(which, WhichLump::Texture1, "in the new lump");
    assert_eq!(found.patches.first().expect("one").patch, 0, "index zero");
}
