//! Shared builders for the `scripts` tests: the parsed name and lump
//! helpers, the error-code shorthand, the empty `edit_dehacked` argument
//! frame and the two sessions the topics run through.
//!
//! [`dehacked`] checks the patch pair's defaults, the scope and directory
//! lookups, the mode parsing, the line operations and the one-edit commit;
//! [`text`] the two script tools' names, their copy-on-write runs, the
//! `UTF-8` refusal and the result helpers.

mod dehacked;
mod text;

use std::path::PathBuf;

use crate::error::CyberError;
use crate::session::{Layer, LayerRole, Session};
use crate::tools::scripts::edit_dehacked::EditDehackedParams;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A lump named `text` carrying `body`.
pub(super) fn carrying(text: &str, body: &str) -> Lump {
    Lump::new(name(text), body.as_bytes().to_vec())
}

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// An empty `edit_dehacked` argument set.
pub(super) fn args() -> EditDehackedParams {
    EditDehackedParams {
        name: None,
        text: None,
        source: None,
        set: None,
        remove: None,
    }
}

/// A session: a base layer holding a `DEHACKED` global.
pub(super) fn loaded() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![carrying("DEHACKED", "Thing 1 (Troop)\n")],
            },
        ))
        .expect("no batch is open");
    session
}

/// An `IWAD`-layer session holding one `DECORATE` lump.
pub(super) fn with_decorate(body: &str) -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![carrying("DECORATE", body)],
            },
        ))
        .expect("no batch is open");
    session
}
