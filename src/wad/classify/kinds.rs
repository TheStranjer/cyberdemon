//! The type hint for one lump: [`classify`]'s order of checks — markers
//! first, then map membership, then well-known names, then the `colormaps`
//! namespace, then payload magics — and [`fallback_kind`]'s last resorts of
//! patch headers, printable `ASCII` and emptiness. Inside the graphics
//! namespaces only the unmistakable magics are consulted: the weak sound
//! tests match raw indexed pixels too often to be trusted there.
//!
//! [`is_doom_patch`] is the standalone `PNAMES`/patch header check the
//! graphics tools reuse, re-exported from the root. [`super::analyze`]
//! calls [`classify`] once per lump with its namespace and map owner
//! settled; the [`LumpKind`] variants and the size and format constants the
//! checks measure against stay in the root.

use crate::wad::archive::Lump;
use crate::wad::bytes::{u16_at, u32_at};
use crate::wad::lumps;
use crate::wad::name::LumpName;

use super::namespaces::is_marker_name;
use super::{
    COLUMN_OFFSET_LEN, ENDOOM_LEN, LumpKind, MAX_PATCH_DIM, Namespace, SOUND_FORMAT,
    SOUND_HEADER_LEN,
};

/// Classifies one lump with its namespace and map membership settled.
pub(super) fn classify(
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
    if let Some(kind) = kind_by_name(lump.name, lump.size()) {
        return kind;
    }
    if namespace == Some(Namespace::Colormaps) {
        return LumpKind::Colormap;
    }
    if let Some(kind) = kind_by_magic(lump.name, &lump.data, namespace) {
        return kind;
    }
    fallback_kind(lump, namespace)
}

/// The kind implied by a well-known lump name, `None` for anything else.
/// Every name is the [`crate::wad::lumps`] constant for it.
const fn kind_by_name(name: LumpName, size: usize) -> Option<LumpKind> {
    match name {
        lumps::PLAYPAL => Some(LumpKind::Palette),
        lumps::COLORMAP => Some(LumpKind::Colormap),
        lumps::PNAMES => Some(LumpKind::Pnames),
        lumps::TEXTURE1 | lumps::TEXTURE2 => Some(LumpKind::TextureDefs),
        lumps::ENDOOM if size == ENDOOM_LEN => Some(LumpKind::Endoom),
        lumps::DEMO1 | lumps::DEMO2 | lumps::DEMO3 | lumps::DEMO4 => Some(LumpKind::Demo),
        lumps::DEHACKED => Some(LumpKind::Dehacked),
        lumps::DECORATE => Some(LumpKind::Decorate),
        lumps::ZSCRIPT => Some(LumpKind::Zscript),
        lumps::MAPINFO | lumps::ZMAPINFO | lumps::UMAPINFO | lumps::EMAPINFO => {
            Some(LumpKind::Mapinfo)
        }
        _ => None,
    }
}

/// The kind implied by the payload's leading bytes, checked image-first
/// then audio then, outside a graphics namespace, the weak sound formats.
fn kind_by_magic(name: LumpName, data: &[u8], namespace: Option<Namespace>) -> Option<LumpKind> {
    image_magic(data)
        .or_else(|| audio_magic(data))
        .or_else(|| sound_magic(name, data, namespace))
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
    } else if data.starts_with(b"ID3") {
        Some(LumpKind::Mp3)
    } else {
        None
    }
}

/// The bare `MPEG` frame sync and the `DMX` `DS` sound and `DP` PC speaker
/// formats: three tests a run of indexed pixels satisfies by accident, so
/// they are skipped wherever [`kind_by_namespace`] already names the payload.
fn sound_magic(name: LumpName, data: &[u8], namespace: Option<Namespace>) -> Option<LumpKind> {
    if kind_by_namespace(namespace).is_some() {
        return None;
    }
    if is_mp3_frame(data) {
        Some(LumpKind::Mp3)
    } else if is_doom_sound(data) {
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

/// Whether `data` starts with an `MPEG` frame sync word.
fn is_mp3_frame(data: &[u8]) -> bool {
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
