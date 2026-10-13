//! WAD format building blocks: lump names and little-endian byte access.
//!
//! [`name::LumpName`] is the eight-character, case-insensitive identity a
//! WAD directory entry carries; [`bytes::Reader`], [`bytes::Writer`] and the
//! free helpers there are the bounds-checked little-endian cursor and sink
//! every format parser composes. Nothing in this module knows about MCP,
//! sessions or the filesystem; it is pure data.

pub mod bytes;
pub mod name;
