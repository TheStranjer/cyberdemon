//! WAD parsing and writing.
//!
//! This module tree is pure data: it carries no `MCP` or `rmcp` concerns
//! and knows nothing about sessions, tools or asset `URL`s. [`name`]
//! defines the short lump names every WAD structure spells with.

pub mod name;
