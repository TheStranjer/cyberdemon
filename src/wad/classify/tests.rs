//! Shared builders and readers for the classification tests: the parsed-name
//! maker, the lump and marker builders, the readers of one analysis entry's
//! kind and namespace, and the binary payload that matches no magic and no
//! text rule.
//!
//! `namespaces` runs the marker pairs, aliases, inner markers and the two
//! warning rules; `maps` the binary, `UDMF` and `GL` block finding, with its
//! own map-owner reader; `kinds` every name, magic and fallback hint, the
//! `is_doom_patch` header check and the wire names.

mod kinds;
mod maps;
mod namespaces;

use super::{LumpKind, Namespace};
use crate::wad::archive::Lump;
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// An empty marker lump named `text`.
pub(super) fn marker(text: &str) -> Lump {
    Lump::marker(name(text))
}

/// A lump named `text` holding `data`.
pub(super) fn lump(text: &str, data: &[u8]) -> Lump {
    Lump::new(name(text), data.to_vec())
}

/// The kind the analysis gave the lump at `index`.
pub(super) fn kind_at(analysis: &super::Analysis, index: usize) -> LumpKind {
    analysis.lumps.get(index).expect("index present").kind
}

/// The namespace the analysis gave the lump at `index`.
pub(super) fn namespace_at(analysis: &super::Analysis, index: usize) -> Option<Namespace> {
    analysis.lumps.get(index).and_then(|entry| entry.namespace)
}

/// A binary-ish payload that matches no magic and no text rule.
pub(super) fn junk() -> Vec<u8> {
    vec![0_u8, 0xFF, 0x7F, 3, 90, 200, 0x10]
}
