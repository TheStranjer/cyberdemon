//! Shared fixtures for the texture tools' tests: the `PNAMES` and
//! `TEXTURE1` lumps the fixture layer carries, the session over it, the
//! [`Page`] and [`Operation`] builders the runners take and the readers of
//! a finished session.
//!
//! [`list`] reads the two listings, [`pnames_edit`] the committed
//! `edit_pnames` runs and the reusable removal, [`pnames_plan`] the planner,
//! the operation picker and `put_global`, [`build`] the two
//! single-texture tools.

mod build;
mod list;
mod pnames_edit;
mod pnames_plan;

use std::path::PathBuf;

use crate::error::CyberError;
use crate::session::{Layer, LayerRole, Session};
use crate::tools::textures::build::{BuildTextureParams, PatchPlacement};
use crate::tools::textures::pnames::{EditReport, Operation};
use crate::tools::textures::pnames_rewrite::edit_pnames_run;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;
use crate::wad::textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
};

use super::{Page, TextureSet};

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A `PNAMES` lump carrying `names`.
pub(super) fn pnames_lump(names: &[&str]) -> Lump {
    let owned: Vec<String> = names.iter().map(|text| (*text).to_owned()).collect();
    Lump::new(
        name("PNAMES"),
        write_pnames(&owned).expect("test names are valid"),
    )
}

/// One texture definition referring to `patches` at the origin zero.
pub(super) fn def(text: &str, width: u16, height: u16, patches: &[u16]) -> TextureDef {
    TextureDef {
        name: text.to_owned(),
        flags_raw: 0,
        width,
        height,
        patches: patches
            .iter()
            .map(|&patch| PatchRef { patch, x: 0, y: 0 })
            .collect(),
    }
}

/// A `TEXTURE1` lump: WALL1 (three refs) and STEP1 (one ref).
pub(super) fn texture1_lump() -> Lump {
    let lump = TextureLump {
        layout: TextureLayout::Doom,
        textures: vec![
            def("WALL1", 64, 128, &[0, 0, 1]),
            def("STEP1", 64, 32, &[1]),
        ],
    };
    Lump::new(
        name("TEXTURE1"),
        write_texture_lump(&lump).expect("serializable"),
    )
}

/// A session on one `IWAD` layer holding the fixture texture set.
pub(super) fn session() -> Session {
    let mut session = Session::default();
    session.set_iwad(Layer::new(
        PathBuf::from("base.wad"),
        LayerRole::Iwad,
        Wad {
            kind: WadKind::Iwad,
            lumps: vec![
                pnames_lump(&["WALLP1", "STEPP1"]),
                texture1_lump(),
                Lump::new(name("WALLP1"), vec![b'P']),
            ],
        },
    ));
    session
}

/// The set of an explicit session.
pub(super) fn set_of(session: &Session) -> TextureSet {
    TextureSet::load(session).expect("the fixture lumps decode")
}

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// A page for the listing calls.
pub(super) fn page(offset: usize, limit: usize) -> Page {
    Page { offset, limit }
}

/// Run one operation end to end against the fixture session.
pub(super) fn run(operation: Operation<'_>, force: bool) -> Result<EditReport, CyberError> {
    let mut session = session();
    edit_pnames_run(&mut session, operation, force)
}

/// A session like [`session`] whose `PNAMES` gained a third name no
/// texture refers to.
pub(super) fn session_with_spare() -> Session {
    let mut session = Session::default();
    session.set_iwad(Layer::new(
        PathBuf::from("base.wad"),
        LayerRole::Iwad,
        Wad {
            kind: WadKind::Iwad,
            lumps: vec![
                pnames_lump(&["WALLP1", "STEPP1", "EXTRAP"]),
                texture1_lump(),
                Lump::new(name("EXTRAP"), vec![b'E']),
            ],
        },
    ));
    session
}

/// A `build_texture` request over `patches` as `(name, x, y)` triples.
pub(super) fn build_args(
    texture: &str,
    width: u16,
    height: u16,
    patches: &[(&str, i16, i16)],
) -> BuildTextureParams {
    BuildTextureParams {
        name: texture.to_owned(),
        width,
        height,
        patches: patches
            .iter()
            .map(|&(patch, x, y)| PatchPlacement {
                patch: patch.to_owned(),
                x,
                y,
            })
            .collect(),
        target: None,
        replace: None,
        allow_missing_patches: None,
    }
}

/// The working archive's lump names, in order.
pub(super) fn working_names(session: &Session) -> Vec<String> {
    session
        .working()
        .lumps
        .iter()
        .map(|lump| lump.name.to_string())
        .collect()
}

/// A `TEXTURE2` also defining `WALL1` (8x8, one patch), shadowing the
/// fixture `TEXTURE1`.
pub(super) fn session_with_shadow() -> Session {
    let mut session = session();
    let shadow = TextureLump {
        layout: TextureLayout::Doom,
        textures: vec![def("WALL1", 8, 8, &[0])],
    };
    session
        .edit("add TEXTURE2", |wad| {
            let bytes = write_texture_lump(&shadow).expect("serializable");
            wad.lumps.push(Lump::new(name("TEXTURE2"), bytes));
            Ok(())
        })
        .expect("plain append");
    session
}
