//! Bulk-editing cores: the pure per-map passes the bulk `replace_*`
//! tools and the `bulk_edit` engine apply to one decoded [`Map`](super::Map).
//!
//! [`replace`] is the replacement engine: glob texture and flat names
//! across the sidedef faces or sector surfaces of one map, rewrite one
//! thing type across its [`Map::things`](super::Map::things) block, and
//! count the fields that actually changed. Every pass is a function of
//! the map model and its parameters alone: no sessions, no archives,
//! no `MCP`. The session-facing machinery that folds these single-map
//! passes over several map blocks inside one undoable step lives in
//! the tools (`crate::tools::query::replace`).
//!
//! The generic per-element edit steps of the `bulk_edit` tool are
//! [`edits`]: a [`edits::FieldEdits`] of `set` and `add` values applied
//! to a selection of sector, sidedef or vertex rows, every resulting row
//! range-checked before any write lands, with the scope's stock sets
//! deciding which texture and flat names exist.

pub mod edits;
pub mod replace;

pub use edits::{Applied, Change, EditScope, FieldEdits, apply};
pub use replace::{Face, Surface, replace_flats, replace_textures, replace_thing_types};
