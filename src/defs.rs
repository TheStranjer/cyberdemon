//! Game definitions: the reference tables the server ships with.
//!
//! [`table`] parses the tab-separated files that live under `data/defs/`;
//! [`registry`] embeds them in the binary, parses each one once, and answers
//! [`registry::definitions_for`]: the thing types (and, in later tickets,
//! linedef specials and sector types) a given [`crate::wad::game::Game`]
//! can use.

pub mod registry;
pub mod table;

pub use registry::{Definitions, definitions_for};
pub use table::ThingDef;
