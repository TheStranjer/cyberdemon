//! WAD format building blocks: lump names, little-endian byte access and
//! the in-memory archive.
//!
//! [`name::LumpName`] is the eight-character, case-insensitive identity a
//! WAD directory entry carries; [`bytes::Reader`], [`bytes::Writer`] and the
//! free helpers there are the bounds-checked little-endian cursor and sink
//! every format parser composes. [`archive::Wad`] ties both into a whole
//! archive: [`Wad::parse`], [`Wad::to_bytes`] and [`Wad::read_file`] read
//! and write real files. [`classify::analyze`] walks a directory and reports
//! every lump's namespace, map membership and type hint, and
//! [`game::detect_game`] names the game a directory of lump names
//! identifies. [`map`] holds the typed, format-neutral map model and its
//! binary `Doom` codec. Nothing in this module knows about MCP or sessions;
//! it is pure data.

pub mod archive;
pub mod bytes;
pub mod classify;
pub mod game;
pub mod map;
pub mod name;
pub mod textures;

pub use archive::{Lump, Wad, WadKind};
pub use classify::{
    Analysis, LumpInfo, LumpKind, MapBlock, MapFormat, Namespace, analyze, is_doom_patch,
};
pub use game::{Game, detect_game};
pub use name::LumpName;
pub use textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, dropped_references, parse_pnames,
    parse_texture_lump, remap_patches, write_pnames, write_texture_lump,
};
