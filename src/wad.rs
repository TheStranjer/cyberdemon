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
//! identifies. [`map`] holds the typed, format-neutral map model with its
//! binary `Doom` and `Hexen` codecs and its `UDMF` mapping, [`mod@diff`]
//! compares two directories lump by lump and map block by map block,
//! [`optimize`] plans the working archive's redundant-lump cleanup over
//! the base and merged snapshots, and
//! [`udmf`] the `UDMF` `TEXTMAP` grammar as a
//! document of blocks and key/value pairs. Nothing in this module knows
//! about MCP or sessions; it is pure data.

pub mod archive;
pub mod bytes;
pub mod classify;
pub mod diff;
pub mod game;
pub mod map;
pub mod name;
pub mod optimize;
pub mod textures;
pub mod udmf;

pub use archive::{Lump, Wad, WadKind};
pub use classify::{
    Analysis, LumpInfo, LumpKind, MapBlock, MapFormat, Namespace, analyze, is_doom_patch,
};
pub use diff::{
    Diff, LumpDiff, MapCounts, MapDiff, Mode, Status, Summary, diff, fnv1a64, same_content,
};
pub use game::{Game, detect_game};
pub use name::LumpName;
pub use textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, dropped_references, parse_pnames,
    parse_texture_lump, remap_patches, write_pnames, write_texture_lump,
};
pub use udmf::{
    UdmfBlock, UdmfDoc, UdmfValue, decimal_to_rounded_int, has_fraction, parse_udmf, write_udmf,
};
