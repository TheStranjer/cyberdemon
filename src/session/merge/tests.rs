//! Shared builders and readers for the merge tests: the test lump names,
//! the `PWAD` builder and the merged-directory views both topics assert
//! on. `rules` runs the vanilla override rules through `merge` and
//! `merge_all`; `entry_points` the whole-archive forms and the helpers.

mod entry_points;
mod rules;

use crate::session::{MergedLump, Source};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// An empty marker lump named `text`.
pub(super) fn marker(text: &str) -> Lump {
    Lump::marker(name(text))
}

/// A lump named `text` whose payload is `text`, so copies are tellable.
pub(super) fn lump(text: &str) -> Lump {
    Lump::new(name(text), text.as_bytes().to_vec())
}

/// A lump named `text` carrying `payload`, so overrides are tellable.
pub(super) fn payload_lump(text: &str, payload: &str) -> Lump {
    Lump::new(name(text), payload.as_bytes().to_vec())
}

/// The payload a named lump in `directory` carries, by name.
pub(super) fn payload_of(directory: &Wad, text: &str) -> String {
    let lump = directory
        .lumps
        .iter()
        .find(|lump| lump.name == name(text))
        .expect("the lump is there");
    String::from_utf8(lump.data.to_vec()).expect("test payloads are text")
}

/// A `PWAD` holding `lumps`.
pub(super) fn pwad(lumps: Vec<Lump>) -> Wad {
    Wad {
        kind: WadKind::Pwad,
        lumps,
    }
}

/// The merged directory's names, in order.
pub(super) fn names(directory: &[MergedLump]) -> Vec<String> {
    directory
        .iter()
        .map(|entry| entry.lump.name.to_string())
        .collect()
}

/// The merged directory as `NAME@IWAD`, `NAME@R1` or `NAME@W`, so both the
/// order and the winners are visible in one assertion.
pub(super) fn tagged(directory: &[MergedLump]) -> Vec<String> {
    directory
        .iter()
        .map(|entry| {
            let source = match entry.source {
                Source::Layer(0) => "IWAD".to_owned(),
                Source::Layer(index) => format!("R{index}"),
                Source::Working => "W".to_owned(),
            };
            format!("{}@{source}", entry.lump.name)
        })
        .collect()
}
