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

use serde::Serialize;

use crate::wad::archive::Lump;
use crate::wad::bytes::{u16_at, u32_at};
use crate::wad::name::LumpName;

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

/// The member lump names of a binary `Doom`/`Hexen` map block.
const MAP_MEMBERS: [&[u8]; 15] = [
    b"THINGS",
    b"LINEDEFS",
    b"SIDEDEFS",
    b"VERTEXES",
    b"SEGS",
    b"SSECTORS",
    b"NODES",
    b"SECTORS",
    b"REJECT",
    b"BLOCKMAP",
    b"BEHAVIOR",
    b"SCRIPTS",
    b"LEAFS",
    b"LIGHTS",
    b"MACROS",
];

/// The member lump names of a `GL` node block.
const GL_MEMBERS: [&[u8]; 5] = [b"GL_VERT", b"GL_SEGS", b"GL_SSECT", b"GL_NODES", b"GL_PVS"];

/// The namespace a start marker opens, `None` for any other name.
const fn namespace_start(name: &[u8]) -> Option<Namespace> {
    match name {
        b"S_START" | b"SS_START" => Some(Namespace::Sprites),
        b"F_START" | b"FF_START" => Some(Namespace::Flats),
        b"P_START" | b"PP_START" => Some(Namespace::Patches),
        b"C_START" => Some(Namespace::Colormaps),
        b"A_START" => Some(Namespace::Acs),
        b"V_START" => Some(Namespace::Voices),
        b"HI_START" => Some(Namespace::Hires),
        b"TX_START" => Some(Namespace::Textures),
        _ => None,
    }
}

/// The namespace an end marker closes, `None` for any other name.
const fn namespace_end(name: &[u8]) -> Option<Namespace> {
    match name {
        b"S_END" | b"SS_END" => Some(Namespace::Sprites),
        b"F_END" | b"FF_END" => Some(Namespace::Flats),
        b"P_END" | b"PP_END" => Some(Namespace::Patches),
        b"C_END" => Some(Namespace::Colormaps),
        b"A_END" => Some(Namespace::Acs),
        b"V_END" => Some(Namespace::Voices),
        b"HI_END" => Some(Namespace::Hires),
        b"TX_END" => Some(Namespace::Textures),
        _ => None,
    }
}

/// Whether `name` is any marker: a namespace boundary or an inner marker
/// like `F1_START` ending in `_START`/`_END`.
fn is_marker_name(name: &[u8]) -> bool {
    namespace_start(name).is_some() || namespace_end(name).is_some() || is_inner_marker(name)
}

/// Whether `name` ends in `_START` or `_END` without being a namespace
/// boundary.
fn is_inner_marker(name: &[u8]) -> bool {
    name.ends_with(b"_START") || name.ends_with(b"_END")
}

/// Whether `name` is a member of a binary map block.
fn is_map_member(name: &[u8]) -> bool {
    MAP_MEMBERS.contains(&name)
}

/// Whether `name` is a member of a `GL` node block.
fn is_gl_member(name: &[u8]) -> bool {
    GL_MEMBERS.contains(&name)
}

/// Whether the lump after `index` is named exactly `wanted`.
fn matches_next(lumps: &[Lump], index: usize, wanted: &[u8]) -> bool {
    lumps
        .get(index.saturating_add(1))
        .is_some_and(|lump| lump.name.as_bytes() == wanted)
}

/// Whether the lump at `index` starts a binary or `UDMF` map block.
fn is_map_header(lumps: &[Lump], index: usize) -> bool {
    let Some(head) = lumps.get(index) else {
        return false;
    };
    if is_marker_name(head.name.as_bytes()) {
        return false;
    }
    matches_next(lumps, index, b"THINGS") || matches_next(lumps, index, b"TEXTMAP")
}

/// Whether the lump at `index` starts a `GL` node block: a `GL_`-prefixed
/// name that is not itself a `GL` member, followed by `GL_VERT`.
fn is_gl_header(lumps: &[Lump], index: usize) -> bool {
    let Some(head) = lumps.get(index) else {
        return false;
    };
    let name = head.name.as_bytes();
    name.starts_with(b"GL_") && !is_gl_member(name) && matches_next(lumps, index, b"GL_VERT")
}

/// Finds the map blocks in directory order, recording warnings.
fn find_maps(lumps: &[Lump], warnings: &mut Vec<String>) -> Vec<MapBlock> {
    let mut maps = Vec::new();
    let mut index = 0_usize;
    while let Some(head) = lumps.get(index) {
        if is_map_header(lumps, index) {
            let (end, format) = scan_map(lumps, index, warnings);
            maps.push(MapBlock {
                name: head.name,
                header: index,
                end,
                format,
                gl: None,
            });
            index = end;
        } else if is_gl_header(lumps, index) {
            let end = scan_gl(lumps, index);
            attach_gl(&mut maps, lumps, index, end);
            index = end;
        } else {
            index = index.saturating_add(1);
        }
    }
    maps
}

/// Scans the body of the map whose header sits at `header`.
fn scan_map(lumps: &[Lump], header: usize, warnings: &mut Vec<String>) -> (usize, MapFormat) {
    let start = header.saturating_add(1);
    if lumps
        .get(start)
        .is_some_and(|lump| lump.name.as_bytes() == b"TEXTMAP")
    {
        udmf_range(lumps, start, warnings)
    } else {
        binary_range(lumps, start)
    }
}

/// The end of a binary member run starting at `start`, plus its format.
fn binary_range(lumps: &[Lump], start: usize) -> (usize, MapFormat) {
    let mut end = start;
    while lumps
        .get(end)
        .is_some_and(|lump| is_map_member(lump.name.as_bytes()))
    {
        end = end.saturating_add(1);
    }
    let hexen = lumps
        .iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .any(|lump| lump.name.as_bytes() == b"BEHAVIOR");
    let format = if hexen {
        MapFormat::Hexen
    } else {
        MapFormat::Doom
    };
    (end, format)
}

/// The end of a `UDMF` block whose `TEXTMAP` sits at `textmap`, plus the
/// fixed `Udmf` format. A missing `ENDMAP` warns and ends at the directory.
fn udmf_range(lumps: &[Lump], textmap: usize, warnings: &mut Vec<String>) -> (usize, MapFormat) {
    let body = textmap.saturating_add(1);
    let found = lumps
        .iter()
        .skip(body)
        .position(|lump| lump.name.as_bytes() == b"ENDMAP");
    let end = found.map_or_else(
        || {
            warnings.push(format!(
                "map lump at index {textmap} is a TEXTMAP without a following ENDMAP: \
                 the block runs to the end of the directory"
            ));
            lumps.len()
        },
        |offset| body.saturating_add(offset).saturating_add(1),
    );
    (end, MapFormat::Udmf)
}

/// The end of the `GL` member run whose header sits at `header`.
fn scan_gl(lumps: &[Lump], header: usize) -> usize {
    let mut end = header.saturating_add(1);
    while lumps
        .get(end)
        .is_some_and(|lump| is_gl_member(lump.name.as_bytes()))
    {
        end = end.saturating_add(1);
    }
    end
}

/// Attaches the `GL` block spanning `start..end` to its map: by name for
/// `GL_<mapname>`, to the preceding map for `GL_LEVEL`.
fn attach_gl(maps: &mut [MapBlock], lumps: &[Lump], start: usize, end: usize) {
    let Some(head) = lumps.get(start).map(|lump| lump.name) else {
        return;
    };
    let target = if head.as_bytes() == b"GL_LEVEL" {
        maps.last_mut()
    } else {
        let wanted = head.as_bytes().strip_prefix(b"GL_").unwrap_or_default();
        maps.iter_mut()
            .find(|block| block.name.as_bytes() == wanted)
    };
    if let Some(block) = target {
        block.gl = Some((start, end));
    }
}

/// The map a lump at `index` belongs to, with whether it is the header.
fn map_at(maps: &[MapBlock], index: usize) -> Option<(LumpName, bool)> {
    maps.iter().find_map(|block| {
        if index == block.header {
            Some((block.name, true))
        } else if (block.header < index && index < block.end)
            || block
                .gl
                .is_some_and(|(start, end)| start <= index && index < end)
        {
            Some((block.name, false))
        } else {
            None
        }
    })
}

/// Tracks the namespace currently open while walking the directory.
#[derive(Default)]
struct NamespaceState {
    /// The open namespace with its start marker's index and name.
    open: Option<(Namespace, usize, LumpName)>,
}

impl NamespaceState {
    /// Classifies `name` at `index`: returns the namespace it sits in and
    /// updates the open state, warning about unclosed starts and stray ends.
    fn visit(
        &mut self,
        name: LumpName,
        index: usize,
        warnings: &mut Vec<String>,
    ) -> Option<Namespace> {
        let bytes = name.as_bytes();
        if let Some(namespace) = namespace_start(bytes) {
            self.close(warnings);
            self.open = Some((namespace, index, name));
            return Some(namespace);
        }
        let Some(namespace) = namespace_end(bytes) else {
            return self.open.map(|(opened, _, _)| opened);
        };
        if self.open.is_some_and(|(opened, _, _)| opened == namespace) {
            self.open = None;
        } else {
            warnings.push(format!(
                "stray end marker {name} at index {index}: no {} namespace is open",
                namespace.as_str()
            ));
        }
        Some(namespace)
    }

    /// Ends the walk: an unclosed start runs to the directory end and warns.
    fn finish(&mut self, warnings: &mut Vec<String>) {
        self.close(warnings);
    }

    /// Warns if a namespace is still open and closes it.
    fn close(&mut self, warnings: &mut Vec<String>) {
        if let Some((namespace, at, marker)) = self.open.take() {
            warnings.push(format!(
                "start marker {marker} at index {at} was never closed: the {} namespace \
                 runs to the end of the directory",
                namespace.as_str()
            ));
        }
    }
}

/// Classifies one lump with its namespace and map membership settled.
fn classify(
    lump: &Lump,
    namespace: Option<Namespace>,
    owner: Option<(LumpName, bool)>,
) -> LumpKind {
    let bytes = lump.name.as_bytes();
    if is_marker_name(bytes) {
        return LumpKind::Marker;
    }
    if let Some((_, header)) = owner {
        return if header {
            LumpKind::MapMarker
        } else {
            LumpKind::MapData
        };
    }
    if let Some(kind) = kind_by_name(bytes, lump.size()) {
        return kind;
    }
    if namespace == Some(Namespace::Colormaps) {
        return LumpKind::Colormap;
    }
    if let Some(kind) = kind_by_magic(lump.name, &lump.data) {
        return kind;
    }
    fallback_kind(lump, namespace)
}

/// The kind implied by a well-known lump name, `None` for anything else.
const fn kind_by_name(name: &[u8], size: usize) -> Option<LumpKind> {
    match name {
        b"PLAYPAL" => Some(LumpKind::Palette),
        b"COLORMAP" => Some(LumpKind::Colormap),
        b"PNAMES" => Some(LumpKind::Pnames),
        b"TEXTURE1" | b"TEXTURE2" => Some(LumpKind::TextureDefs),
        b"ENDOOM" if size == ENDOOM_LEN => Some(LumpKind::Endoom),
        b"DEMO1" | b"DEMO2" | b"DEMO3" | b"DEMO4" => Some(LumpKind::Demo),
        b"DEHACKED" => Some(LumpKind::Dehacked),
        b"DECORATE" => Some(LumpKind::Decorate),
        b"ZSCRIPT" => Some(LumpKind::Zscript),
        b"MAPINFO" | b"ZMAPINFO" | b"UMAPINFO" | b"EMAPINFO" => Some(LumpKind::Mapinfo),
        _ => None,
    }
}

/// The kind implied by the payload's leading bytes, checked image-first
/// then audio then `Doom` sound formats.
fn kind_by_magic(name: LumpName, data: &[u8]) -> Option<LumpKind> {
    image_magic(data)
        .or_else(|| audio_magic(data))
        .or_else(|| sound_magic(name, data))
}

/// The image container magics.
fn image_magic(data: &[u8]) -> Option<LumpKind> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(LumpKind::Png)
    } else if data.starts_with(b"\xFF\xD8\xFF") {
        Some(LumpKind::Jpeg)
    } else if data.starts_with(b"GIF8") {
        Some(LumpKind::Gif)
    } else {
        None
    }
}

/// The audio container magics.
fn audio_magic(data: &[u8]) -> Option<LumpKind> {
    if data.starts_with(b"MUS\x1A") {
        Some(LumpKind::Mus)
    } else if data.starts_with(b"MThd") {
        Some(LumpKind::Midi)
    } else if is_wav(data) {
        Some(LumpKind::Wav)
    } else if data.starts_with(b"OggS") {
        Some(LumpKind::Ogg)
    } else if data.starts_with(b"fLaC") {
        Some(LumpKind::Flac)
    } else if is_mp3(data) {
        Some(LumpKind::Mp3)
    } else {
        None
    }
}

/// The `DMX` `DS` sound and `DP` PC speaker formats.
fn sound_magic(name: LumpName, data: &[u8]) -> Option<LumpKind> {
    if is_doom_sound(data) {
        Some(LumpKind::DoomSound)
    } else if is_pc_speaker(name, data) {
        Some(LumpKind::PcSpeaker)
    } else {
        None
    }
}

/// Whether `data` is a `RIFF` container announcing `WAVE`.
fn is_wav(data: &[u8]) -> bool {
    data.starts_with(b"RIFF") && data.get(8_usize..12).is_some_and(|tag| tag == b"WAVE")
}

/// Whether `data` starts with an `ID3` tag or an `MPEG` frame sync word.
fn is_mp3(data: &[u8]) -> bool {
    if data.starts_with(b"ID3") {
        return true;
    }
    match (data.first(), data.get(1_usize)) {
        (Some(&first), Some(&second)) => first == 0xFF_u8 && second >= 0xE0_u8,
        _ => false,
    }
}

/// Whether `data` is a `DMX` sound: format word `3` and a sample count
/// that fits the payload.
fn is_doom_sound(data: &[u8]) -> bool {
    u16_at(data, 0).is_ok_and(|format| format == SOUND_FORMAT)
        && u32_at(data, 4).is_ok_and(|count| {
            usize::try_from(count).is_ok_and(|samples| {
                SOUND_HEADER_LEN
                    .checked_add(samples)
                    .is_some_and(|need| data.len() >= need)
            })
        })
}

/// Whether `name` is a `DP*` PC speaker lump: zero first word.
fn is_pc_speaker(name: LumpName, data: &[u8]) -> bool {
    name.as_bytes().starts_with(b"DP") && u16_at(data, 0).is_ok_and(|head| head == 0_u16)
}

/// The kind implied by sitting inside a graphics namespace.
const fn kind_by_namespace(namespace: Option<Namespace>) -> Option<LumpKind> {
    match namespace {
        Some(Namespace::Flats) => Some(LumpKind::Flat),
        Some(Namespace::Sprites) => Some(LumpKind::Sprite),
        Some(Namespace::Patches) => Some(LumpKind::Patch),
        Some(_) | None => None,
    }
}

/// The last-resort kinds: patch headers, then text, then emptiness.
fn fallback_kind(lump: &Lump, namespace: Option<Namespace>) -> LumpKind {
    if let Some(kind) = kind_by_namespace(namespace) {
        return kind;
    }
    if is_doom_patch(&lump.data) {
        return LumpKind::Patch;
    }
    if is_text(&lump.data) {
        return LumpKind::Text;
    }
    if lump.size() == 0 {
        return LumpKind::Empty;
    }
    LumpKind::Unknown
}

/// Whether every byte of `data` is printable `ASCII` or whitespace and
/// the payload is not empty.
fn is_text(data: &[u8]) -> bool {
    !data.is_empty() && data.iter().all(|&byte| is_text_byte(byte))
}

/// Whether `byte` may appear in a text lump.
const fn is_text_byte(byte: u8) -> bool {
    byte.is_ascii_graphic() || matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

/// Whether `data` is a `Doom` patch: sane dimensions, a complete column
/// offset table and every offset pointing inside the payload.
#[must_use]
#[inline]
pub fn is_doom_patch(data: &[u8]) -> bool {
    let (Ok(width), Ok(height)) = (u16_at(data, 0), u16_at(data, 2)) else {
        return false;
    };
    if !(1_u16..=MAX_PATCH_DIM).contains(&width) || !(1_u16..=MAX_PATCH_DIM).contains(&height) {
        return false;
    }
    patch_offsets_in_range(data, usize::from(width))
}

/// Checks a header-validated patch's column offset table of `width`
/// entries against the payload bounds.
fn patch_offsets_in_range(data: &[u8], width: usize) -> bool {
    let Some(table_end) = COLUMN_OFFSET_LEN
        .checked_mul(width)
        .and_then(|table| 8_usize.checked_add(table))
    else {
        return false;
    };
    let Some(table) = data.get(8_usize..table_end) else {
        return false;
    };
    table.chunks_exact(COLUMN_OFFSET_LEN).all(|entry| {
        <[u8; COLUMN_OFFSET_LEN]>::try_from(entry)
            .map(u32::from_le_bytes)
            .ok()
            .and_then(|offset| usize::try_from(offset).ok())
            .is_some_and(|offset| table_end <= offset && offset < data.len())
    })
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
mod tests {
    use serde_json::json;

    use super::{LumpInfo, LumpKind, MapBlock, MapFormat, Namespace, analyze, is_doom_patch};
    use crate::wad::archive::Lump;
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A lump named `text` holding `data`.
    fn lump(text: &str, data: &[u8]) -> Lump {
        Lump::new(name(text), data.to_vec())
    }

    /// The kind the analysis gave the lump at `index`.
    fn kind_at(analysis: &super::Analysis, index: usize) -> LumpKind {
        analysis.lumps.get(index).expect("index present").kind
    }

    /// The namespace the analysis gave the lump at `index`.
    fn namespace_at(analysis: &super::Analysis, index: usize) -> Option<Namespace> {
        analysis.lumps.get(index).and_then(|entry| entry.namespace)
    }

    /// The map the analysis gave the lump at `index`.
    fn map_at(analysis: &super::Analysis, index: usize) -> Option<LumpName> {
        analysis.lumps.get(index).and_then(|entry| entry.map)
    }

    /// A binary-ish payload that matches no magic and no text rule.
    fn junk() -> Vec<u8> {
        vec![0_u8, 0xFF, 0x7F, 3, 90, 200, 0x10]
    }

    #[test]
    fn namespace_helpers_and_serialization() {
        assert_eq!(Namespace::Sprites.as_str(), "sprites", "snake-case name");
        assert_eq!(
            Namespace::Sprites.start_marker(),
            "S_START",
            "canonical start"
        );
        assert_eq!(Namespace::Sprites.end_marker(), "S_END", "canonical end");
        assert_eq!(Namespace::Flats.start_marker(), "F_START", "flats start");
        assert_eq!(Namespace::Patches.end_marker(), "P_END", "patches end");
        assert_eq!(
            Namespace::Colormaps.start_marker(),
            "C_START",
            "colormaps start"
        );
        assert_eq!(Namespace::Acs.end_marker(), "A_END", "acs end");
        assert_eq!(Namespace::Voices.start_marker(), "V_START", "voices start");
        assert_eq!(Namespace::Hires.end_marker(), "HI_END", "hires end");
        assert_eq!(
            Namespace::Textures.start_marker(),
            "TX_START",
            "textures start"
        );
        assert_eq!(
            Namespace::parse("flats"),
            Some(Namespace::Flats),
            "exact parse"
        );
        assert_eq!(
            Namespace::parse("Hires"),
            Some(Namespace::Hires),
            "case-insensitive"
        );
        assert_eq!(Namespace::parse("nope"), None, "unknown name");
        assert_eq!(
            serde_json::to_value(Namespace::Colormaps).expect("serializes"),
            json!("colormaps"),
            "snake-case wire name"
        );
    }

    #[test]
    fn lump_kind_names_mirror_serialization_and_parse() {
        assert_eq!(LumpKind::ALL.len(), 30, "the documented kind count");
        for kind in LumpKind::ALL {
            let serialized = serde_json::to_value(kind).expect("kinds serialize");
            let text = serialized
                .as_str()
                .expect("kinds serialize as snake-case strings");
            assert_eq!(kind.as_str(), text, "as_str mirrors Serialize for {kind:?}");
            assert_eq!(LumpKind::parse(text), Some(kind), "exact parse of {text}");
            assert_eq!(
                LumpKind::parse(&text.to_ascii_uppercase()),
                Some(kind),
                "case-insensitive parse of {text}"
            );
        }
        assert_eq!(LumpKind::parse("nope"), None, "unknown kind name");
    }

    #[test]
    fn every_namespace_pair_delimits_its_region() {
        for namespace in Namespace::ALL {
            let lumps = vec![
                marker(namespace.start_marker()),
                lump("XMEMBER", &junk()),
                marker(namespace.end_marker()),
                marker("AFTER"),
            ];
            let analysis = analyze(&lumps);
            assert_eq!(
                namespace_at(&analysis, 0),
                Some(namespace),
                "{} start marker knows its namespace",
                namespace.as_str()
            );
            assert_eq!(
                namespace_at(&analysis, 1),
                Some(namespace),
                "member sits inside {}",
                namespace.as_str()
            );
            assert_eq!(
                namespace_at(&analysis, 2),
                Some(namespace),
                "{} end marker knows its namespace",
                namespace.as_str()
            );
            assert_eq!(
                namespace_at(&analysis, 3),
                None,
                "after the pair is outside"
            );
            assert_eq!(kind_at(&analysis, 0), LumpKind::Marker, "start is a marker");
            assert_eq!(kind_at(&analysis, 2), LumpKind::Marker, "end is a marker");
            assert!(analysis.warnings.is_empty(), "a clean pair must not warn");
        }
    }

    #[test]
    fn alias_markers_open_and_close_their_namespace() {
        let lumps = vec![
            marker("SS_START"),
            marker("S_END"),
            marker("FF_START"),
            marker("F_END"),
            marker("PP_START"),
            marker("P_END"),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(
            analysis
                .lumps
                .iter()
                .map(|entry| entry.namespace)
                .collect::<Vec<_>>(),
            vec![
                Some(Namespace::Sprites),
                Some(Namespace::Sprites),
                Some(Namespace::Flats),
                Some(Namespace::Flats),
                Some(Namespace::Patches),
                Some(Namespace::Patches),
            ],
            "aliases carry their namespace and close on the canonical end"
        );
        assert!(analysis.warnings.is_empty(), "mixed pairs are legal");
    }

    #[test]
    fn inner_markers_stay_inside_the_namespace() {
        let lumps = vec![
            marker("F_START"),
            marker("F1_START"),
            lump("FLATA", &junk()),
            marker("F2_END"),
            lump("FLATB", &junk()),
            marker("F_END"),
        ];
        let analysis = analyze(&lumps);
        for (index, expected) in [
            LumpKind::Marker,
            LumpKind::Marker,
            LumpKind::Flat,
            LumpKind::Marker,
            LumpKind::Flat,
            LumpKind::Marker,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(kind_at(&analysis, index), expected, "kind at {index}");
        }
        assert!(
            (0..6).all(|at| namespace_at(&analysis, at) == Some(Namespace::Flats)),
            "inner markers do not close the enclosing namespace"
        );
        assert!(
            analysis.warnings.is_empty(),
            "inner markers are not warnings"
        );
    }

    #[test]
    fn unclosed_namespace_runs_to_the_end_and_warns() {
        let lumps = vec![
            marker("S_START"),
            lump("SPRITE01", &junk()),
            marker("AFTER"),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(
            analysis.warnings.len(),
            1,
            "exactly one warning: {analysis:?}"
        );
        assert!(
            analysis.warnings[0].contains("never closed")
                && analysis.warnings[0].contains("sprites"),
            "the warning explains the fallback: {}",
            analysis.warnings[0]
        );
        assert_eq!(
            namespace_at(&analysis, 1),
            Some(Namespace::Sprites),
            "runs on"
        );
        assert_eq!(
            namespace_at(&analysis, 2),
            Some(Namespace::Sprites),
            "to the end"
        );
    }

    #[test]
    fn stray_end_marker_warns_without_closing_another_namespace() {
        let lumps = vec![
            marker("F_START"),
            marker("P_END"),
            lump("FLATA", &junk()),
            marker("F_END"),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(
            analysis.warnings.len(),
            1,
            "only the stray warns: {analysis:?}"
        );
        assert!(
            analysis.warnings[0].contains("stray end marker")
                && analysis.warnings[0].contains("patches"),
            "the warning names the stray: {}",
            analysis.warnings[0]
        );
        assert_eq!(
            namespace_at(&analysis, 1),
            Some(Namespace::Patches),
            "the marker still reports the namespace it closes"
        );
        assert_eq!(
            kind_at(&analysis, 1),
            LumpKind::Marker,
            "a stray is a marker"
        );
        assert_eq!(
            namespace_at(&analysis, 2),
            Some(Namespace::Flats),
            "the open namespace survives a stray end"
        );
    }

    #[test]
    fn a_new_start_closes_the_previous_namespace_with_a_warning() {
        let lumps = vec![
            marker("F_START"),
            marker("S_START"),
            lump("SPRITE01", &junk()),
            marker("S_END"),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(
            analysis.warnings.len(),
            1,
            "one unclosed start: {analysis:?}"
        );
        assert!(
            analysis.warnings[0].contains("F_START") && analysis.warnings[0].contains("flats"),
            "the warning names the opener: {}",
            analysis.warnings[0]
        );
        assert_eq!(
            namespace_at(&analysis, 2),
            Some(Namespace::Sprites),
            "the later start wins"
        );
    }

    #[test]
    fn markers_win_over_content_rules() {
        let lumps = vec![
            lump("S_START", b"not really empty"),
            lump("TX_END", b"tail"),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(
            kind_at(&analysis, 0),
            LumpKind::Marker,
            "a filled start is a marker"
        );
        assert_eq!(
            namespace_at(&analysis, 0),
            Some(Namespace::Sprites),
            "and opens sprites"
        );
        assert_eq!(
            kind_at(&analysis, 1),
            LumpKind::Marker,
            "a filled end is a marker"
        );
        assert_eq!(
            namespace_at(&analysis, 1),
            Some(Namespace::Textures),
            "and reports textures"
        );
    }

    #[test]
    fn binary_map_block_is_doom_format() {
        let lumps = vec![
            marker("MAP01"),
            lump("THINGS", &junk()),
            lump("LINEDEFS", &junk()),
            lump("SIDEDEFS", &junk()),
            lump("VERTEXES", &junk()),
            lump("SEGS", &junk()),
            lump("SSECTORS", &junk()),
            lump("NODES", &junk()),
            lump("SECTORS", &junk()),
            lump("REJECT", &junk()),
            lump("BLOCKMAP", &junk()),
            lump("PLAYPAL", b"palettes"),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(analysis.maps.len(), 1, "one map: {:?}", analysis.maps);
        let block = analysis.maps.first().expect("the block");
        assert_eq!(block.name, name("MAP01"), "the header names the map");
        assert_eq!(
            (block.header, block.end),
            (0, 11),
            "the run spans the members"
        );
        assert_eq!(block.format, MapFormat::Doom, "no BEHAVIOR means Doom");
        assert_eq!(block.gl, None, "no GL block");
        assert_eq!(kind_at(&analysis, 0), LumpKind::MapMarker, "header kind");
        assert_eq!(kind_at(&analysis, 1), LumpKind::MapData, "member kind");
        assert_eq!(
            map_at(&analysis, 0),
            Some(name("MAP01")),
            "header belongs to the map"
        );
        assert_eq!(
            map_at(&analysis, 10),
            Some(name("MAP01")),
            "last member too"
        );
        assert_eq!(
            kind_at(&analysis, 11),
            LumpKind::Palette,
            "the first non-member ends it"
        );
        assert_eq!(map_at(&analysis, 11), None, "and is not map data");
        assert!(
            analysis.warnings.is_empty(),
            "clean block: {:?}",
            analysis.warnings
        );
    }

    #[test]
    fn behavior_lump_marks_the_hexen_format() {
        let lumps = vec![
            marker("E1M1"),
            lump("THINGS", &junk()),
            lump("SECTORS", &junk()),
            lump("BEHAVIOR", &junk()),
            lump("SPECIALS", &junk()),
        ];
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("one map");
        assert_eq!(block.format, MapFormat::Hexen, "BEHAVIOR means Hexen");
        assert_eq!(block.end, 4, "SPECIALS is not a binary member");
        assert_eq!(
            kind_at(&analysis, 3),
            LumpKind::MapData,
            "BEHAVIOR is map data"
        );
    }

    #[test]
    fn udmf_block_spans_textmap_through_endmap() {
        let lumps = vec![
            marker("MAP01"),
            lump("TEXTMAP", b"map \"Doom\";"),
            lump("ZNODES", &junk()),
            lump("BEHAVIOR", &junk()),
            lump("DIALOGUE", &junk()),
            lump("REJECT", &junk()),
            lump("BLOCKMAP", &junk()),
            marker("ENDMAP"),
            lump("FLATA", &junk()),
        ];
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("one map");
        assert_eq!(block.format, MapFormat::Udmf, "TEXTMAP wins over BEHAVIOR");
        assert_eq!((block.header, block.end), (0, 8), "ENDMAP is included");
        assert_eq!(
            kind_at(&analysis, 2),
            LumpKind::MapData,
            "unlisted lumps are members"
        );
        assert_eq!(
            kind_at(&analysis, 7),
            LumpKind::MapData,
            "ENDMAP is map data too"
        );
        assert_eq!(map_at(&analysis, 8), None, "the run ended at ENDMAP");
        assert!(
            analysis.warnings.is_empty(),
            "clean UDMF: {:?}",
            analysis.warnings
        );
    }

    #[test]
    fn udmf_without_endmap_runs_on_and_warns() {
        let lumps = vec![
            marker("E1M1"),
            lump("TEXTMAP", b"x"),
            lump("ZNODES", &junk()),
        ];
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("one map");
        assert_eq!(block.end, 3, "the block runs to the directory end");
        assert_eq!(analysis.warnings.len(), 1, "missing ENDMAP warns");
        assert!(
            analysis.warnings[0].contains("ENDMAP"),
            "the warning names ENDMAP: {}",
            analysis.warnings[0]
        );
    }

    #[test]
    fn two_maps_back_to_back() {
        let lumps = vec![
            marker("MAP01"),
            lump("THINGS", &junk()),
            lump("SECTORS", &junk()),
            marker("MAP02"),
            lump("THINGS", &junk()),
            lump("SECTORS", &junk()),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(analysis.maps.len(), 2, "both maps found");
        let (first, second) = (
            analysis.maps.first().expect("first map"),
            analysis.maps.get(1).expect("second map"),
        );
        assert_eq!((first.header, first.end), (0, 3), "first block");
        assert_eq!((second.header, second.end), (3, 6), "second block");
        assert_eq!(map_at(&analysis, 4), Some(name("MAP02")), "second THINGS");
        assert_eq!(kind_at(&analysis, 3), LumpKind::MapMarker, "second header");
    }

    #[test]
    fn gl_block_attaches_by_name() {
        let mut lumps = vec![
            marker("MAP01"),
            lump("THINGS", &junk()),
            lump("SECTORS", &junk()),
        ];
        lumps.extend([
            marker("GL_MAP01"),
            lump("GL_VERT", &junk()),
            lump("GL_SEGS", &junk()),
            lump("GL_SSECT", &junk()),
            lump("GL_NODES", &junk()),
            lump("GL_PVS", &junk()),
            lump("PLAYPAL", b"palettes"),
        ]);
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("one map");
        assert_eq!(block.gl, Some((3, 9)), "GL block attached to MAP01");
        assert_eq!(
            kind_at(&analysis, 3),
            LumpKind::MapData,
            "GL header is map data"
        );
        assert_eq!(
            map_at(&analysis, 8),
            Some(name("MAP01")),
            "GL_PVS belongs to the map"
        );
        assert_eq!(map_at(&analysis, 9), None, "PLAYPAL does not");
        assert!(
            analysis.warnings.is_empty(),
            "clean GL: {:?}",
            analysis.warnings
        );
    }

    #[test]
    fn gl_level_attaches_to_the_preceding_map() {
        let lumps = vec![
            marker("E1M1"),
            lump("THINGS", &junk()),
            marker("GL_LEVEL"),
            lump("GL_VERT", &junk()),
            lump("GL_NODES", &junk()),
            lump("SECTORS", &junk()),
        ];
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("one map");
        assert_eq!(block.gl, Some((2, 5)), "GL_LEVEL took the preceding map");
        assert_eq!(
            kind_at(&analysis, 4),
            LumpKind::MapData,
            "GL_NODES is map data"
        );
        assert_eq!(
            kind_at(&analysis, 5),
            LumpKind::Unknown,
            "SECTORS after the run is loose"
        );
    }

    #[test]
    fn gl_block_without_a_matching_map_is_ignored() {
        let lumps = vec![
            marker("GL_MAP09"),
            lump("GL_VERT", &junk()),
            lump("GL_SEGS", &junk()),
            lump("END", &junk()),
        ];
        let analysis = analyze(&lumps);
        assert!(
            analysis.maps.is_empty(),
            "nothing to attach to: {:?}",
            analysis.maps
        );
        assert_eq!(
            kind_at(&analysis, 0),
            LumpKind::Empty,
            "the empty GL header falls back"
        );
        assert_eq!(
            kind_at(&analysis, 1),
            LumpKind::Unknown,
            "a loose GL lump is not map data"
        );
        assert_eq!(
            map_at(&analysis, 1),
            None,
            "no map ownership without a block"
        );
    }

    #[test]
    fn things_without_a_header_is_not_a_map() {
        let lumps = vec![lump("THINGS", &junk()), lump("LINEDEFS", &junk())];
        let analysis = analyze(&lumps);
        assert!(analysis.maps.is_empty(), "no preceding header means no map");
        assert_eq!(
            kind_at(&analysis, 0),
            LumpKind::Unknown,
            "THINGS at index 0 is a loose lump"
        );
        assert_eq!(map_at(&analysis, 0), None, "and owns no map");
    }

    #[test]
    fn fixed_names_get_their_kinds() {
        let cases: [(&str, LumpKind); 16] = [
            ("PLAYPAL", LumpKind::Palette),
            ("COLORMAP", LumpKind::Colormap),
            ("PNAMES", LumpKind::Pnames),
            ("TEXTURE1", LumpKind::TextureDefs),
            ("TEXTURE2", LumpKind::TextureDefs),
            ("DEHACKED", LumpKind::Dehacked),
            ("DECORATE", LumpKind::Decorate),
            ("ZSCRIPT", LumpKind::Zscript),
            ("MAPINFO", LumpKind::Mapinfo),
            ("ZMAPINFO", LumpKind::Mapinfo),
            ("UMAPINFO", LumpKind::Mapinfo),
            ("EMAPINFO", LumpKind::Mapinfo),
            ("DEMO1", LumpKind::Demo),
            ("DEMO2", LumpKind::Demo),
            ("DEMO3", LumpKind::Demo),
            ("DEMO4", LumpKind::Demo),
        ];
        let lumps: Vec<Lump> = cases.iter().map(|case| lump(case.0, b"hi")).collect();
        let analysis = analyze(&lumps);
        for (at, case) in cases.iter().enumerate() {
            assert_eq!(kind_at(&analysis, at), case.1, "kind of {}", case.0);
        }
    }

    #[test]
    fn endoom_needs_the_exact_size() {
        let full = vec![lump("ENDOOM", &vec![0xA0_u8; 4000])];
        assert_eq!(kind_at(&analyze(&full), 0), LumpKind::Endoom, "4000 bytes");
        let short = vec![lump("ENDOOM", &vec![0xA0_u8; 3999])];
        assert_eq!(
            kind_at(&analyze(&short), 0),
            LumpKind::Unknown,
            "3999 bytes is not ENDOOM"
        );
    }

    #[test]
    fn colormaps_namespace_lumps_are_colormaps() {
        let lumps = vec![marker("C_START"), lump("GAMMA2", &junk()), marker("C_END")];
        let analysis = analyze(&lumps);
        assert_eq!(
            kind_at(&analysis, 1),
            LumpKind::Colormap,
            "any lump in the namespace"
        );
        assert_eq!(
            kind_at(&analysis, 0),
            LumpKind::Marker,
            "the markers stay markers"
        );
        assert_eq!(
            namespace_at(&analysis, 1),
            Some(Namespace::Colormaps),
            "namespace set"
        );
    }

    /// One magic-detection case: name, payload, expected kind.
    type MagicCase = (&'static str, Vec<u8>, LumpKind);

    #[test]
    fn content_magics_get_their_kinds() {
        let cases: Vec<MagicCase> = vec![
            ("PNGD", b"\x89PNG\r\n\x1a\n\x00\x01".to_vec(), LumpKind::Png),
            ("JPEGD", vec![0xFF, 0xD8, 0xFF, 0x00], LumpKind::Jpeg),
            ("GIFD", b"GIF89a".to_vec(), LumpKind::Gif),
            ("MUSD", b"MUS\x1Arest".to_vec(), LumpKind::Mus),
            ("MIDID", b"MThd\x00\x06".to_vec(), LumpKind::Midi),
            ("WAVD", b"RIFF$\0\0\0WAVEfmt".to_vec(), LumpKind::Wav),
            ("OGGD", b"OggS\x00".to_vec(), LumpKind::Ogg),
            ("FLACD", b"fLaC\x00".to_vec(), LumpKind::Flac),
            ("MP3D", b"ID3\x04\x00".to_vec(), LumpKind::Mp3),
            ("MP3E", vec![0xFF, 0xFB, 0x10], LumpKind::Mp3),
        ];
        let lumps: Vec<Lump> = cases.iter().map(|case| lump(case.0, &case.1)).collect();
        let analysis = analyze(&lumps);
        for (at, case) in cases.iter().enumerate() {
            assert_eq!(kind_at(&analysis, at), case.2, "magic of {}", case.0);
        }
        let riff_only = vec![lump("RIFFX", b"RIFF\x24\x00\x00\x00AIFFdata")];
        assert_eq!(
            kind_at(&analyze(&riff_only), 0),
            LumpKind::Unknown,
            "a RIFF that is not WAVE does not match"
        );
    }

    #[test]
    fn doom_sound_and_pc_speaker_detection() {
        let lumps = vec![
            lump("DSSNARE", &[3, 0, 0x80, 0x2B, 4, 0, 0, 0, 1, 2, 3, 4]),
            lump("DPSONG", &[0, 0, 0x91, 0x8A]),
            lump("DSBAD", &[3, 0, 0, 0, 0x10, 0x27, 0, 0, 1]),
            lump("DPBAD", &[1, 0, 0x91, 0x8A]),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(
            kind_at(&analysis, 0),
            LumpKind::DoomSound,
            "format 3 with a fitting count"
        );
        assert_eq!(
            kind_at(&analysis, 1),
            LumpKind::PcSpeaker,
            "DP name, zero head"
        );
        assert_eq!(
            kind_at(&analysis, 2),
            LumpKind::Unknown,
            "count past the end"
        );
        assert_eq!(
            kind_at(&analysis, 3),
            LumpKind::Unknown,
            "DP name but nonzero head"
        );
    }

    #[test]
    fn namespaces_imply_the_graphic_kinds() {
        let cases = [
            ("F_START", "F_END", Namespace::Flats, LumpKind::Flat),
            ("S_START", "S_END", Namespace::Sprites, LumpKind::Sprite),
            ("P_START", "P_END", Namespace::Patches, LumpKind::Patch),
        ];
        for (start, end, ns, expected) in cases {
            let lumps = vec![marker(start), lump("MEMBER", &junk()), marker(end)];
            let analysis = analyze(&lumps);
            assert_eq!(
                kind_at(&analysis, 1),
                expected,
                "a loose lump inside {start}"
            );
            assert_eq!(namespace_at(&analysis, 1), Some(ns), "namespace of {start}");
        }
    }

    /// A patch payload: header, `width` column offsets and `tail` spare
    /// bytes behind the table.
    fn patch_bytes(width: u16, height: u16, offsets: &[u32], tail: usize) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&width.to_le_bytes());
        data.extend_from_slice(&height.to_le_bytes());
        data.extend_from_slice(&0_i16.to_le_bytes());
        data.extend_from_slice(&0_i16.to_le_bytes());
        for offset in offsets {
            data.extend_from_slice(&offset.to_le_bytes());
        }
        data.append(&mut vec![0xAA_u8; tail]);
        data
    }

    #[test]
    fn is_doom_patch_accepts_well_formed_headers() {
        assert!(
            is_doom_patch(&patch_bytes(1, 1, &[12], 1)),
            "one column pointing past the table"
        );
        assert!(
            is_doom_patch(&patch_bytes(2, 3, &[16, 16], 4)),
            "both columns at the table end"
        );
        let loose = vec![lump("TWOD", &patch_bytes(2, 2, &[16, 19], 5))];
        assert_eq!(
            kind_at(&analyze(&loose), 0),
            LumpKind::Patch,
            "the pass uses the check too"
        );
    }

    #[test]
    fn is_doom_patch_rejects_bad_headers() {
        assert!(!is_doom_patch(&patch_bytes(0, 1, &[12], 1)), "zero width");
        assert!(
            !is_doom_patch(&patch_bytes(4097, 1, &[1], 1)),
            "width past the cap"
        );
        assert!(!is_doom_patch(&patch_bytes(1, 0, &[12], 1)), "zero height");
        assert!(
            !is_doom_patch(&patch_bytes(1, 4097, &[1], 1)),
            "height past the cap"
        );
        assert!(
            !is_doom_patch(&patch_bytes(2, 1, &[16], 0)),
            "table truncated"
        );
        assert!(
            !is_doom_patch(&patch_bytes(1, 1, &[8], 4)),
            "offset inside the table"
        );
        assert!(
            !is_doom_patch(&patch_bytes(1, 1, &[13], 0)),
            "offset at the very end"
        );
        assert!(!is_doom_patch(&[1_u8, 0, 1]), "shorter than the header");
        assert!(!is_doom_patch(&[]), "empty data");
    }

    #[test]
    fn text_binary_and_empty_fallbacks() {
        let lumps = vec![
            lump("HELLO", b"hello world\r\n\t!"),
            lump("BINARY", &junk()),
            marker("NOTHING"),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(kind_at(&analysis, 0), LumpKind::Text, "printable payload");
        assert_eq!(
            kind_at(&analysis, 1),
            LumpKind::Unknown,
            "high bytes are not text"
        );
        assert_eq!(kind_at(&analysis, 2), LumpKind::Empty, "empty non-marker");
        assert_eq!(
            analysis.lumps.first().expect("first").size,
            15,
            "size reported"
        );
        assert_eq!(
            analysis.lumps.get(2).expect("third").index,
            2,
            "index reported"
        );
    }

    #[test]
    fn serialized_names_use_the_contract_casing() {
        let lumps = vec![marker("MAP01"), lump("THINGS", &junk()), lump("X", &[])];
        let analysis = analyze(&lumps);
        let value = serde_json::to_value(&analysis).expect("an Analysis serializes");
        assert_eq!(
            serde_json::to_value(analysis.lumps.first().expect("header info")).expect("serializes"),
            json!({
                "index": 0_u64,
                "name": "MAP01",
                "size": 0_u64,
                "kind": "map_marker",
                "namespace": null,
                "map": "MAP01",
            }),
            "header entry"
        );
        assert_eq!(
            serde_json::to_value(analysis.lumps.get(2).expect("loose")).expect("serializes"),
            json!({
                "index": 2_u64,
                "name": "X",
                "size": 0_u64,
                "kind": "empty",
                "namespace": null,
                "map": null,
            }),
            "loose empty entry"
        );
        assert_eq!(
            value["maps"][0]["format"],
            json!("doom"),
            "format is lowercase"
        );
        assert_eq!(value["maps"][0]["gl"], json!(null), "absent GL is null");
        assert_eq!(
            serde_json::to_value(MapFormat::Udmf).expect("serializes"),
            json!("udmf"),
            "udmf"
        );
        for (kind, text) in [
            (LumpKind::DoomSound, "doom_sound"),
            (LumpKind::PcSpeaker, "pc_speaker"),
            (LumpKind::TextureDefs, "texture_defs"),
            (LumpKind::MapData, "map_data"),
        ] {
            assert_eq!(
                serde_json::to_value(kind).expect("serializes"),
                json!(text),
                "serialized kind"
            );
        }
    }

    #[test]
    fn empty_directory_analyzes_cleanly() {
        let analysis = analyze(&[]);
        assert!(analysis.lumps.is_empty(), "no entries");
        assert!(analysis.maps.is_empty(), "no maps");
        assert!(analysis.warnings.is_empty(), "no warnings");
        let empty_maps: Vec<MapBlock> = Vec::new();
        assert!(empty_maps.is_empty(), "type anchor for MapBlock");
        let first_info: Option<&LumpInfo> = analysis.lumps.first();
        assert!(first_info.is_none(), "type anchor for LumpInfo");
    }
}
