//! Classification pass over a lump directory: namespaces, map blocks and
//! type hints.
//!
//! [`analyze`] walks a slice of [`Lump`]s and produces an [`Analysis`]:
//! one [`LumpInfo`] per directory entry saying its [`Namespace`], the map
//! it belongs to (if any) and a [`LumpKind`] type hint, plus the list of
//! [`MapBlock`]s with their [`MapFormat`] and any GL node blocks, plus
//! warnings for unclosed markers and malformed map runs. The pass is
//! pure: it never edits the directory and reports every judgment as an
//! explicit rule. [`is_doom_patch`] is the standalone `PNAMES`/patch
//! header check the graphics tools reuse.
//!
//! The work is split three ways below this root: the marker names and the
//! open-namespace tracker are the `namespaces` module, the map-block finding
//! and the one `gl_marker_for` naming rule the `GL` writers share the
//! `maps` one and the per-lump type hints the private `kinds`
//! one. The [`Namespace`], [`MapFormat`] and [`LumpKind`] enums, the shapes
//! above, the constants they measure against and the [`analyze`] entry point
//! stay here, with [`is_doom_patch`] re-exported from `kinds`, `gl_marker_for`
//! and `GL_LEVEL` from `maps`, and the two `namespace_start` and
//! `namespace_end` marker helpers from `namespaces`. Those two modules are
//! public for one reason: their re-exported items are `pub(crate)`, which
//! `redundant_pub_crate` only accepts inside a public module.

mod kinds;
pub mod maps;
pub mod namespaces;

use serde::Serialize;

use crate::wad::archive::Lump;
use crate::wad::classify::kinds::classify;
use crate::wad::classify::maps::{find_maps, map_at};
use crate::wad::classify::namespaces::NamespaceState;
use crate::wad::name::LumpName;

pub use crate::wad::classify::kinds::is_doom_patch;
pub(crate) use crate::wad::classify::maps::{GL_LEVEL, gl_marker_for};
pub(crate) use crate::wad::classify::namespaces::{namespace_end, namespace_start};

/// Exact byte size of an `ENDOOM` lump.
const ENDOOM_LEN: usize = 4000;
/// Bytes before a `Doom` sound lump's raw samples.
const SOUND_HEADER_LEN: usize = 8;
/// The `DS` format tag a `Doom` sound lump starts with.
const SOUND_FORMAT: u16 = 3;
/// Longest sprite, patch or flat edge this pass accepts as plausible.
const MAX_PATCH_DIM: u16 = 4096;
/// Bytes in one `PatchCol` column-offset entry.
const COLUMN_OFFSET_LEN: usize = 4;
/// A marker-delimited region of the lump directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Namespace {
    /// Between `S_START`/`SS_START` and `S_END`/`SS_END`.
    Sprites,
    /// Between `F_START`/`FF_START` and `F_END`/`FF_END`.
    Flats,
    /// Between `P_START`/`PP_START` and `P_END`/`PP_END`.
    Patches,
    /// Between `C_START` and `C_END`.
    Colormaps,
    /// Between `A_START` and `A_END`.
    Acs,
    /// Between `V_START` and `V_END`.
    Voices,
    /// Between `HI_START` and `HI_END`.
    Hires,
    /// Between `TX_START` and `TX_END`.
    Textures,
}

impl Namespace {
    /// Every namespace, in documentation order.
    pub const ALL: [Self; 8] = [
        Self::Sprites,
        Self::Flats,
        Self::Patches,
        Self::Colormaps,
        Self::Acs,
        Self::Voices,
        Self::Hires,
        Self::Textures,
    ];

    /// The snake-case name of this namespace.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sprites => "sprites",
            Self::Flats => "flats",
            Self::Patches => "patches",
            Self::Colormaps => "colormaps",
            Self::Acs => "acs",
            Self::Voices => "voices",
            Self::Hires => "hires",
            Self::Textures => "textures",
        }
    }

    /// The canonical marker that opens this namespace.
    #[must_use]
    #[inline]
    pub const fn start_marker(self) -> &'static str {
        match self {
            Self::Sprites => "S_START",
            Self::Flats => "F_START",
            Self::Patches => "P_START",
            Self::Colormaps => "C_START",
            Self::Acs => "A_START",
            Self::Voices => "V_START",
            Self::Hires => "HI_START",
            Self::Textures => "TX_START",
        }
    }

    /// The canonical marker that closes this namespace.
    #[must_use]
    #[inline]
    pub const fn end_marker(self) -> &'static str {
        match self {
            Self::Sprites => "S_END",
            Self::Flats => "F_END",
            Self::Patches => "P_END",
            Self::Colormaps => "C_END",
            Self::Acs => "A_END",
            Self::Voices => "V_END",
            Self::Hires => "HI_END",
            Self::Textures => "TX_END",
        }
    }

    /// Parses a namespace from its name, case-insensitively; `None` for
    /// anything else.
    #[must_use]
    #[inline]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|namespace| namespace.as_str().eq_ignore_ascii_case(text))
    }
}

/// The layout a map block's data lumps use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MapFormat {
    /// The vanilla `Doom`/Heretic lump set.
    Doom,
    /// A `Doom` block that carries a `BEHAVIOR` lump.
    Hexen,
    /// A `TEXTMAP`/`ENDMAP` block.
    Udmf,
}

impl MapFormat {
    /// The wire name of this format, as the map tools spell it.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Doom => "doom",
            Self::Hexen => "hexen",
            Self::Udmf => "udmf",
        }
    }

    /// Parses a format from its wire name, case-insensitively; `None` for
    /// anything else.
    #[must_use]
    #[inline]
    pub fn parse(text: &str) -> Option<Self> {
        [Self::Doom, Self::Hexen, Self::Udmf]
            .into_iter()
            .find(|format| format.as_str().eq_ignore_ascii_case(text))
    }
}

/// The type hint [`analyze`] proposes for one lump.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LumpKind {
    /// A namespace marker, `S_START` and friends, inner ones included.
    Marker,
    /// The zero-size header lump of a map block.
    MapMarker,
    /// A data lump inside a map block.
    MapData,
    /// `PLAYPAL`.
    Palette,
    /// `COLORMAP` or any lump in the `colormaps` namespace.
    Colormap,
    /// `PNAMES`.
    Pnames,
    /// `TEXTURE1` or `TEXTURE2`.
    TextureDefs,
    /// `ENDOOM`, exactly 4000 bytes.
    Endoom,
    /// `DEMO1` to `DEMO4`.
    Demo,
    /// `DEHACKED`.
    Dehacked,
    /// `DECORATE`.
    Decorate,
    /// `ZSCRIPT`.
    Zscript,
    /// `MAPINFO`, `ZMAPINFO`, `UMAPINFO` or `EMAPINFO`.
    Mapinfo,
    /// Payload starts with the `PNG` signature.
    Png,
    /// Payload starts with the `JPEG` signature.
    Jpeg,
    /// Payload starts with the `GIF` signature.
    Gif,
    /// Payload starts with the `MUS` signature.
    Mus,
    /// Payload starts with the `MThd` signature.
    Midi,
    /// Payload is a `RIFF`/`WAVE` file.
    Wav,
    /// Payload starts with the `OggS` capture pattern.
    Ogg,
    /// Payload starts with the `fLaC` magic.
    Flac,
    /// Payload starts with an `ID3` tag or an `MPEG` frame sync.
    Mp3,
    /// Payload is a `DMX` `DS` sound with a consistent sample count.
    DoomSound,
    /// A `DP*` lump starting with a zero word: PC speaker data.
    PcSpeaker,
    /// A lump in the `flats` namespace.
    Flat,
    /// A lump in the `sprites` namespace.
    Sprite,
    /// A lump in the `patches` namespace, or with a valid patch header.
    Patch,
    /// Payload is non-empty printable `ASCII` and whitespace.
    Text,
    /// Non-marker lump with an empty payload.
    Empty,
    /// Nothing matched.
    Unknown,
}

impl LumpKind {
    /// Every kind, in documentation order.
    pub const ALL: [Self; 30] = [
        Self::Marker,
        Self::MapMarker,
        Self::MapData,
        Self::Palette,
        Self::Colormap,
        Self::Pnames,
        Self::TextureDefs,
        Self::Endoom,
        Self::Demo,
        Self::Dehacked,
        Self::Decorate,
        Self::Zscript,
        Self::Mapinfo,
        Self::Png,
        Self::Jpeg,
        Self::Gif,
        Self::Mus,
        Self::Midi,
        Self::Wav,
        Self::Ogg,
        Self::Flac,
        Self::Mp3,
        Self::DoomSound,
        Self::PcSpeaker,
        Self::Flat,
        Self::Sprite,
        Self::Patch,
        Self::Text,
        Self::Empty,
        Self::Unknown,
    ];

    /// The snake-case name of this kind, exactly what `Serialize` writes.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Marker => "marker",
            Self::MapMarker => "map_marker",
            Self::MapData => "map_data",
            Self::Palette => "palette",
            Self::Colormap => "colormap",
            Self::Pnames => "pnames",
            Self::TextureDefs => "texture_defs",
            Self::Endoom => "endoom",
            Self::Demo => "demo",
            Self::Dehacked => "dehacked",
            Self::Decorate => "decorate",
            Self::Zscript => "zscript",
            Self::Mapinfo => "mapinfo",
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
            Self::Mus => "mus",
            Self::Midi => "midi",
            Self::Wav => "wav",
            Self::Ogg => "ogg",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::DoomSound => "doom_sound",
            Self::PcSpeaker => "pc_speaker",
            Self::Flat => "flat",
            Self::Sprite => "sprite",
            Self::Patch => "patch",
            Self::Text => "text",
            Self::Empty => "empty",
            Self::Unknown => "unknown",
        }
    }

    /// Parses a kind from its snake-case name, case-insensitively; `None`
    /// for anything else.
    #[must_use]
    #[inline]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(text))
    }
}

/// One directory entry with its namespace, map and type hint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LumpInfo {
    /// Position in the directory.
    pub index: usize,
    /// Normalised lump name.
    pub name: LumpName,
    /// Payload size in bytes.
    pub size: usize,
    /// Type hint.
    pub kind: LumpKind,
    /// Namespace the lump sits in, if any.
    pub namespace: Option<Namespace>,
    /// Name of the map this lump belongs to, if any.
    pub map: Option<LumpName>,
}

/// A contiguous map definition in the directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MapBlock {
    /// Name of the map, taken from the header lump.
    pub name: LumpName,
    /// Index of the header lump.
    pub header: usize,
    /// One past the last data lump of the block.
    pub end: usize,
    /// Layout the block uses.
    pub format: MapFormat,
    /// Start and end of the attached `GL` node block, if present.
    pub gl: Option<(usize, usize)>,
}

/// The whole classification of one directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Analysis {
    /// One entry per lump, in directory order.
    pub lumps: Vec<LumpInfo>,
    /// Map blocks, in directory order.
    pub maps: Vec<MapBlock>,
    /// Structural problems found on the way.
    pub warnings: Vec<String>,
}

/// Classifies a whole lump directory: namespaces, map blocks and type
/// hints, with warnings for everything structurally odd.
#[must_use]
#[inline]
pub fn analyze(lumps: &[Lump]) -> Analysis {
    let mut warnings = Vec::new();
    let maps = find_maps(lumps, &mut warnings);
    let mut state = NamespaceState::default();
    let mut infos = Vec::with_capacity(lumps.len());
    for (index, lump) in lumps.iter().enumerate() {
        let namespace = state.visit(lump.name, index, &mut warnings);
        let owner = map_at(&maps, index);
        infos.push(LumpInfo {
            index,
            name: lump.name,
            size: lump.size(),
            kind: classify(lump, namespace, owner),
            namespace,
            map: owner.map(|(name, _)| name),
        });
    }
    state.finish(&mut warnings);
    Analysis {
        lumps: infos,
        maps,
        warnings,
    }
}

#[cfg(test)]
mod tests;
