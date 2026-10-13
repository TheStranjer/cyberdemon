//! WAD format building blocks: lump names, little-endian byte access and
//! the in-memory archive.
//!
//! [`name::LumpName`] is the eight-character, case-insensitive identity a
//! WAD directory entry carries; [`bytes::Reader`], [`bytes::Writer`] and the
//! free helpers there are the bounds-checked little-endian cursor and sink
//! every format parser composes. [`archive::Wad`] ties both into a whole
//! archive: [`Wad::parse`], [`Wad::to_bytes`] and [`Wad::read_file`] read
//! and write real files. [`classify::analyze`] walks a directory and reports
//! every lump's namespace, map membership and type hint. Nothing in this
//! module knows about MCP or sessions; it is pure data.

pub mod archive;
pub mod bytes;
pub mod classify;
pub mod name;

pub use archive::{Lump, Wad, WadKind};
pub use classify::{
    Analysis, LumpInfo, LumpKind, MapBlock, MapFormat, Namespace, analyze, is_doom_patch,
};
