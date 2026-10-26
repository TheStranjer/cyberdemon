//! Shared builders and readers for the lump-editing tests: the test lump
//! names, the `PWAD` builder, the seeded session and the argument sets and
//! plans every topic asserts on. `replace` runs the copy-on-write tool,
//! `remove` the working-archive deletions and `rename` the in-place
//! renames.

mod remove;
mod rename;
mod replace;

use std::path::PathBuf;

use super::{EditSelector, edit_selector};
use crate::error::CyberError;
use crate::session::{Layer, LayerRole, Session};
use crate::tools::lumps::edit::remove::{RemoveLumpParams, RemovePlan, plan_remove};
use crate::tools::lumps::edit::rename::RenameLumpParams;
use crate::tools::lumps::edit::replace::ReplaceLumpParams;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A lump named `text` carrying `payload`.
pub(super) fn carrying(text: &str, payload: &str) -> Lump {
    Lump::new(name(text), payload.as_bytes().to_vec())
}

/// An empty marker lump named `text`.
pub(super) fn marker(text: &str) -> Lump {
    Lump::marker(name(text))
}

/// A `PWAD` holding `lumps`.
pub(super) fn pwad(lumps: Vec<Lump>) -> Wad {
    Wad {
        kind: WadKind::Pwad,
        lumps,
    }
}

/// The directory names of an archive, in order.
pub(super) fn names(wad: &Wad) -> Vec<String> {
    wad.lumps.iter().map(|lump| lump.name.to_string()).collect()
}

/// The payloads of an archive as `NAME(payload)`, in order.
pub(super) fn with_payloads(wad: &Wad) -> Vec<String> {
    wad.lumps
        .iter()
        .map(|lump| {
            let text = String::from_utf8(lump.data.to_vec()).expect("test payloads are text");
            format!("{}({text})", lump.name)
        })
        .collect()
}

/// The error code of a [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// An [`EditSelector`] for `text` with no refinements.
pub(super) fn selector_for(text: &str) -> EditSelector {
    edit_selector(text, None, None).expect("test selectors are valid")
}

/// A session: an `IWAD` with a loose global, a flats region and one `MAP01`
/// block with its `GL` run, over an empty working archive.
pub(super) fn loaded() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    carrying("PLAYPAL", "base-palette"),
                    marker("F_START"),
                    carrying("FLATA", "base-flat"),
                    marker("F_END"),
                    marker("MAP01"),
                    carrying("THINGS", "base-things"),
                    carrying("SECTORS", "base-sectors"),
                    marker("GL_MAP01"),
                    carrying("GL_VERT", "base-gl"),
                ],
            },
        ))
        .expect("no batch is open");
    session
}

/// Appends `lumps` to the working archive as one edit.
pub(super) fn seed(session: &mut Session, lumps: Vec<Lump>) {
    session
        .edit("seed", move |wad| {
            wad.lumps.extend(lumps);
            Ok(())
        })
        .expect("plain appends");
}

/// A `replace_lump` argument set carrying `text` and `digits`.
pub(super) fn replace_args(text: Option<&str>, digits: Option<&str>) -> ReplaceLumpParams {
    ReplaceLumpParams {
        name: "NOTE".to_owned(),
        namespace: None,
        occurrence: None,
        text: text.map(str::to_owned),
        hex: digits.map(str::to_owned),
        source: None,
    }
}

/// A `remove_lump` argument set.
pub(super) fn remove_args(
    listed: &[&str],
    namespace: Option<&str>,
    occurrence: Option<usize>,
    all: bool,
) -> RemoveLumpParams {
    RemoveLumpParams {
        names: listed.iter().map(|text| (*text).to_owned()).collect(),
        namespace: namespace.map(str::to_owned),
        occurrence,
        all_occurrences: all,
    }
}

/// A `remove_lump` plan for `listed` names, no region filter.
pub(super) fn remove_plan(listed: &[&str], all: bool) -> RemovePlan {
    plan_remove(&remove_args(listed, None, None, all)).expect("test plans are valid")
}

/// A `remove_lump` plan for `TWIN` with an explicit occurrence.
pub(super) fn twin_plan(occurrence: Option<usize>) -> RemovePlan {
    RemovePlan {
        names: vec![name("TWIN")],
        namespace: None,
        occurrence,
        all: false,
    }
}

/// A `rename_lump` argument set.
pub(super) fn rename_args(from: &str, to: &str, allow: bool) -> RenameLumpParams {
    RenameLumpParams {
        name: from.to_owned(),
        new_name: to.to_owned(),
        namespace: None,
        occurrence: None,
        allow_duplicate: allow,
    }
}

/// The payload of the merged view's `FLATA`, as text.
pub(super) fn report_of_flat(session: &Session) -> String {
    let flat = session
        .merged()
        .into_iter()
        .find(|entry| entry.lump.name == name("FLATA"))
        .expect("the view holds a FLATA");
    String::from_utf8(flat.lump.data.to_vec()).expect("test payloads are text")
}
