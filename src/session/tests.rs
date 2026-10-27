//! Unit tests of one editing session: the scope names, the layer stack and
//! the merged view in `layers`, the labelled history and the batch scope in
//! `history`, the keyed read cache in `cache`, and the rest of the session's
//! own state — active map, render settings, theme, the layer-only view, game
//! detection — in `state`.
//!
//! [`name`], [`lump`], [`marker`], [`pwad`], [`layer`], [`tagged`],
//! [`loaded`], [`add_marker`], [`append_marker`] and [`payload`] are the
//! fixtures the topics share.

mod cache;
mod history;
mod layers;
mod state;

use std::path::PathBuf;

use super::{Layer, LayerRole, MergedLump, Session, Source};
use crate::error::CyberError;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A lump named `text` whose payload is `text`, so copies are tellable.
pub(super) fn lump(text: &str) -> Lump {
    Lump::new(name(text), text.as_bytes().to_vec())
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

/// A layer named after `file`, reading itself as its path.
pub(super) fn layer(role: LayerRole, file: &str, lumps: Vec<Lump>) -> Layer {
    Layer::new(PathBuf::from(format!("{file}.wad")), role, pwad(lumps))
}

/// The names and origins of a lump list, for one-glance assertions.
pub(super) fn tagged(list: &[MergedLump]) -> Vec<String> {
    list.iter()
        .map(|entry| {
            let source = match entry.source {
                Source::Layer(index) => format!("L{index}"),
                Source::Working => "W".to_owned(),
            };
            format!("{}@{source}", entry.lump.name)
        })
        .collect()
}

/// A session with a base archive, one resource and a working archive that
/// overrides a flat and adds a map.
pub(super) fn loaded() -> Session {
    let mut session = Session::default();
    session
        .set_iwad(layer(
            LayerRole::Iwad,
            "doom",
            vec![
                lump("PLAYPAL"),
                lump("GAMEINFO"),
                marker("F_START"),
                lump("FLOOR_1"),
                lump("FLOOR_2"),
                marker("F_END"),
                marker("MAP01"),
                lump("THINGS"),
            ],
        ))
        .expect("no batch is open");
    session
        .add_resource(layer(
            LayerRole::Resource,
            "extras",
            vec![
                lump("GAMEINFO"),
                marker("F_START"),
                lump("F_SKY1"),
                marker("F_END"),
            ],
        ))
        .expect("no batch is open");
    session
}

/// Appends one marker lump to the working archive, always succeeding.
pub(super) fn add_marker(session: &mut Session, text: &str) -> Result<usize, CyberError> {
    let owned = lump(text);
    session.edit(&format!("add {text}"), move |wad| {
        let index = wad.lumps.len();
        wad.lumps.push(owned);
        Ok(index)
    })
}

/// Appends a fixed marker to a working archive. Parsing the name is the
/// only step that can fail.
pub(super) fn append_marker(wad: &mut Wad) -> Result<(), CyberError> {
    let lump_name = LumpName::parse("MARKED")?;
    wad.lumps.push(Lump::marker(lump_name));
    Ok(())
}

/// A lump named `text` carrying `body`, so copies are tellable.
pub(super) fn payload(text: &str, body: &str) -> Lump {
    Lump::new(name(text), body.as_bytes().to_vec())
}
