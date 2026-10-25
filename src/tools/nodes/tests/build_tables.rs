//! The two side-lump builders: [`crate::tools::nodes::build_tables::install_lump`]'s
//! in-place replacement and canonical insert position, the reject `mode`
//! parameter, and the `build_blockmap` and `build_reject` runs writing the
//! working archive as one undoable step - including the refusals that
//! write nothing at all.

use crate::session::Session;
use crate::tools::nodes::build_tables::{
    BLOCKMAP_AFTER, REJECT_AFTER, blockmap_build_run, install_lump, parse_mode, reject_build_run,
};
use crate::tools::nodes::{BuildBlockmapParams, BuildRejectParams};
use crate::wad::archive::Lump;
use crate::wad::map::build_reject::RejectMode;
use crate::wad::map::{Blockmap, Map};
use crate::wad::name::LumpName;
use serde_json::json;

use super::{block_session, build_session, lump, marker};

/// The first working-archive lump named `text`.
fn written<'lump>(session: &'lump Session, text: &[u8]) -> &'lump Lump {
    session
        .working()
        .lumps
        .iter()
        .find(|lump| lump.name.as_bytes() == text)
        .unwrap_or_else(|| panic!("no {text:?} lump was written"))
}

#[test]
fn install_lump_replaces_in_place_or_inserts_after_its_neighbour() {
    let mut map = Map {
        other: vec![
            marker("SEGS"),
            marker("SSECTORS"),
            lump("NODES", b"nd"),
            marker("REJECT"),
            lump("BLOCKMAP", b"old"),
            marker("BEHAVIOR"),
        ],
        ..Map::default()
    };
    install_lump(
        &mut map,
        LumpName::from_raw(*b"BLOCKMAP"),
        b"new",
        &BLOCKMAP_AFTER,
    );
    assert_eq!(map.other.len(), 6, "the replacement did not grow the block");
    let at = map
        .other
        .iter()
        .position(|lump| lump.name.as_bytes() == b"BLOCKMAP")
        .expect("BLOCKMAP is present");
    assert_eq!(at, 4, "it kept its position");
    assert_eq!(
        map.other.get(at).map(|lump| lump.data.as_ref()),
        Some(&b"new"[..]),
        "and took the new bytes"
    );
    let mut bare = Map {
        other: vec![marker("SEGS"), marker("NODES"), marker("BEHAVIOR")],
        ..Map::default()
    };
    install_lump(
        &mut bare,
        LumpName::from_raw(*b"REJECT\0\0"),
        b"r",
        &REJECT_AFTER,
    );
    let names: Vec<String> = bare
        .other
        .iter()
        .map(|lump| lump.name.to_string())
        .collect();
    assert_eq!(
        names,
        ["SEGS", "NODES", "REJECT", "BEHAVIOR"],
        "an absent lump lands after its first canonical neighbour"
    );
    let mut empty = Map::default();
    install_lump(
        &mut empty,
        LumpName::from_raw(*b"BLOCKMAP"),
        b"b",
        &BLOCKMAP_AFTER,
    );
    assert_eq!(empty.other.len(), 1, "with no neighbour it appends");
}

#[test]
fn parse_mode_defaults_to_zero_and_refuses_anything_else() {
    assert_eq!(parse_mode(None).expect("absent"), RejectMode::Zero);
    assert_eq!(
        parse_mode(Some("Components")).expect("components"),
        RejectMode::Components
    );
    let error = parse_mode(Some("sightlines")).expect_err("not a mode");
    assert_eq!(error.code(), "invalid_argument", "bad mode code");
    let message = error.to_string();
    assert!(
        message.contains("zero") && message.contains("components"),
        "the message names both values: {message}"
    );
}

#[test]
fn blockmap_build_run_writes_the_grid_into_the_working_archive() {
    let mut session = build_session();
    let report = blockmap_build_run(
        &mut session,
        &BuildBlockmapParams {
            map: Some("map01".to_owned()),
            compress: None,
        },
    )
    .expect("the fixture builds a blockmap");
    assert_eq!(report.body.columns, 2, "x 0..200 from -8 spans two cells");
    assert_eq!(report.body.rows, 1, "y 0..0 fits one row");
    assert_eq!(
        (report.body.origin.x, report.body.origin.y),
        (-8_i16, -8_i16),
        "origins"
    );
    assert_eq!(
        report.body.bytes, 18,
        "the two identical lists share one copy"
    );
    assert!(report.body.vanilla_safe, "a toy grid is tiny");
    assert_eq!(
        report.edit.get("map"),
        Some(&json!("MAP01")),
        "the fragment"
    );
    assert_eq!(
        report.edit.get("nodes_invalidated"),
        Some(&json!(false)),
        "a lump rewrite is not a geometry edit"
    );
    assert_eq!(report.edit.get("copied_to_working"), Some(&json!(true)));
    assert_eq!(
        session.undo_labels(),
        vec!["build blockmap".to_owned()],
        "one undo step"
    );
    let parsed = Blockmap::parse(written(&session, b"BLOCKMAP").data.as_ref())
        .expect("the written lump parses");
    assert_eq!(
        parsed.blocks,
        vec![vec![0_u16], vec![0]],
        "the line is listed in both cells"
    );
    let plain = blockmap_build_run(
        &mut session,
        &BuildBlockmapParams {
            map: None,
            compress: Some(false),
        },
    )
    .expect("the uncompressed build runs too");
    assert_eq!(plain.body.bytes, 24, "every cell writes its own list");
    assert_eq!(plain.edit.get("copied_to_working"), Some(&json!(false)));
}

#[test]
fn reject_build_run_writes_zero_then_components() {
    let mut session = build_session();
    let zero = reject_build_run(
        &mut session,
        &BuildRejectParams {
            map: None,
            mode: None,
        },
    )
    .expect("the default mode builds");
    assert_eq!(zero.body.bytes, 1, "two sectors need one byte");
    assert_eq!(zero.body.sectors, 2, "the decoded sector count");
    assert_eq!(zero.body.rejected_pairs, 0, "zero rejects nothing");
    assert_eq!(zero.body.mode, "zero", "the echo");
    let parts = reject_build_run(
        &mut session,
        &BuildRejectParams {
            map: None,
            mode: Some("COMPONENTS".to_owned()),
        },
    )
    .expect("components builds");
    assert_eq!(
        parts.body.rejected_pairs, 2,
        "the dangling side joins nothing"
    );
    assert_eq!(parts.body.mode, "components", "the echo");
    assert_eq!(
        written(&session, b"REJECT").data.as_ref(),
        &[0b0000_0110_u8],
        "bits (0,1) and (1,0) only"
    );
    assert_eq!(
        session.undo_labels().len(),
        2,
        "the two builds are two steps"
    );
}

#[test]
fn build_runs_refuse_without_writing_anything() {
    let mut session = block_session(Vec::new());
    let error = blockmap_build_run(
        &mut session,
        &BuildBlockmapParams {
            map: None,
            compress: None,
        },
    )
    .expect_err("the fixture has no vertices");
    assert_eq!(error.code(), "invalid_argument", "empty map code");
    assert!(session.working().is_empty(), "the copy rolled back");
    assert!(session.undo_labels().is_empty(), "and no history entry");
    let bad = reject_build_run(
        &mut session,
        &BuildRejectParams {
            map: None,
            mode: Some("opaque".to_owned()),
        },
    )
    .expect_err("not a mode");
    assert_eq!(bad.code(), "invalid_argument", "bad mode code");
    assert!(session.working().is_empty(), "still nothing written");
}
