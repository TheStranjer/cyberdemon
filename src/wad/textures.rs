//! `PNAMES` and `TEXTURE1`/`TEXTURE2` codecs: the composite texture definitions.
//!
//! Patch names live in `PNAMES` as an ordered list of eight-byte entries;
//! the position in that list is the number a texture refers to. `TEXTURE1`
//! and `TEXTURE2` hold the texture records: an `i32` count, that many
//! `i32` record offsets from the lump start, then one record per texture.
//! A record is the name, the four-byte `masked`/`flags` word (kept raw
//! because `ZDoom` packs flags and two scales into it), `width` and
//! `height` as `u16`, the patch count as `u16`, and the mappatch list. The
//! two layouts differ in exactly two places ([`TextureLayout`]): the
//! `Doom` record also carries the obsolete column-directory `i32` before
//! the patch count and stores `stepdir` and `colormap` after each mappatch
//! origin, so a `Doom` record is `22 + 10 * patchcount` bytes and a
//! `Strife` record `18 + 6 * patchcount`.
//!
//! [`parse_texture_lump`] tries the `Doom` layout first and falls back to
//! `Strife` when a record overruns the lump, overruns the next record's
//! offset, or names a patch outside `PNAMES`; [`write_texture_lump`] then
//! emits the layout the lump was read in, writing `0` column directories,
//! `stepdir` of `1` and `colormap` of `0`. [`remap_patches`] and
//! [`dropped_references`] are the renumbering primitives the `PNAMES`
//! editing tools use to keep texture references truthful.

use crate::error::CyberError;
use crate::wad::bytes::{Reader, Writer};
use crate::wad::name::LumpName;

/// Width in bytes of one `PNAMES` or texture record name.
const NAME_WIDTH: usize = 8;
/// Bytes of the `numtextures` field and of one record offset.
const WORD_LEN: usize = 4;
/// Bytes of a `Doom` texture record before its mappatch list.
const DOOM_RECORD_HEAD_LEN: usize = 22;
/// Bytes of a `Strife` texture record before its mappatch list.
const STRIFE_RECORD_HEAD_LEN: usize = 18;
/// Bytes of one `Doom` mappatch: origins, patch, `stepdir`, `colormap`.
const DOOM_PATCH_LEN: usize = 10;
/// Bytes of one `Strife` mappatch: origins and patch.
const STRIFE_PATCH_LEN: usize = 6;
/// The `stepdir` the vanilla engines expect, written to every `Doom` patch.
const DEFAULT_STEP_DIR: u16 = 1;

/// One patch placement inside a texture: a `PNAMES` index and its origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchRef {
    /// Index into the `PNAMES` list.
    pub patch: u16,
    /// Horizontal placement in pixels from the texture's left edge.
    pub x: i16,
    /// Vertical placement in pixels from the texture's top edge.
    pub y: i16,
}

/// One texture definition: name, size and patch list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureDef {
    /// Texture name, uppercased and `NUL`-trimmed like a lump name.
    pub name: String,
    /// The `masked`/`flags` word kept raw: vanilla `Doom` stores `1` for a
    /// masked texture, `ZDoom` packs `flags u16` plus `scale x u8` and
    /// `scale y u8` into the same four bytes.
    pub flags_raw: u32,
    /// Texture width in pixels.
    pub width: u16,
    /// Texture height in pixels.
    pub height: u16,
    /// The patches making up this texture, in record order.
    pub patches: Vec<PatchRef>,
}

/// The record layout one texture lump uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureLayout {
    /// Vanilla `Doom`, Heretic and Hexen: a column-directory word in the
    /// record head and a five-field mappatch.
    Doom,
    /// `Strife`: no column directory and a three-field mappatch.
    Strife,
}

impl TextureLayout {
    /// Bytes of one record head, before the mappatch list.
    const fn head_len(self) -> usize {
        match self {
            Self::Doom => DOOM_RECORD_HEAD_LEN,
            Self::Strife => STRIFE_RECORD_HEAD_LEN,
        }
    }

    /// Bytes of one mappatch.
    const fn patch_len(self) -> usize {
        match self {
            Self::Doom => DOOM_PATCH_LEN,
            Self::Strife => STRIFE_PATCH_LEN,
        }
    }
}

/// One `TEXTURE1`/`TEXTURE2` lump decoded, with the layout it was read in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureLump {
    /// The layout this lump was parsed with, and written back in.
    pub layout: TextureLayout,
    /// The texture definitions, in lump order.
    pub textures: Vec<TextureDef>,
}

/// A `PNAMES` index renumbering: `mapping[old]` is the new index of the
/// old entry, or `None` when that entry was removed.
pub type PatchMapping = Vec<Option<u16>>;

/// The name of one record, uppercased and `NUL`-trimmed exactly like a
/// directory name.
fn record_name(bytes: [u8; NAME_WIDTH]) -> String {
    LumpName::from_raw(bytes).to_string()
}

/// The `usize` form of a declared count, refusing a negative one.
///
/// # Errors
///
/// [`CyberError::Format`] naming `what` when `raw` is negative.
fn entry_count(raw: i32, what: &str) -> Result<usize, CyberError> {
    usize::try_from(raw).map_err(|error| {
        CyberError::Format(format!(
            "{what} declares {raw} entries, which is not a count: {error}"
        ))
    })
}

/// The `i32` form of a count, for the stored field.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] above `i32::MAX`.
fn count_field(count: usize, what: &str) -> Result<i32, CyberError> {
    i32::try_from(count).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "{what} holds {count} entries, past the i32 field limit: {error}"
        ))
    })
}

/// Reads `PNAMES`: an `i32` count followed by that many eight-byte,
/// `NUL`-padded patch names. The position of a name in the result is the
/// number texture records refer to it by.
///
/// # Errors
///
/// [`CyberError::Format`] for a negative count or a name list that runs
/// past the end of `data`.
///
/// # Examples
///
/// ```
/// use cyberdemon::wad::textures::{parse_pnames, write_pnames};
///
/// let names = vec!["WALLP1".to_owned(), "STEPP1".to_owned()];
/// let bytes = write_pnames(&names).expect("valid names");
/// assert_eq!(parse_pnames(&bytes).expect("a well-formed lump"), names);
/// ```
#[inline]
pub fn parse_pnames(data: &[u8]) -> Result<Vec<String>, CyberError> {
    let mut reader = Reader::new(data);
    let count = entry_count(reader.read_i32()?, "PNAMES")?;
    let mut names = Vec::new();
    for _entry in 0..count {
        names.push(record_name(reader.read_array::<NAME_WIDTH>()?));
    }
    Ok(names)
}

/// Writes a `PNAMES` lump for `names`, each validated with
/// [`LumpName::parse`] so it is 1 to 8 `ASCII` graphic characters, stored
/// uppercase.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name that does not parse, naming
/// the offending position; [`CyberError::LimitExceeded`] when the count
/// does not fit the `i32` field.
#[inline]
pub fn write_pnames(names: &[String]) -> Result<Vec<u8>, CyberError> {
    let mut writer = Writer::new();
    writer.put_i32(count_field(names.len(), "PNAMES")?);
    for (index, text) in names.iter().enumerate() {
        let name = LumpName::parse(text).map_err(|error| {
            CyberError::InvalidArgument(format!(
                "PNAMES entry {index} is not a valid name: {error}"
            ))
        })?;
        writer.put_bytes(&name.raw());
    }
    Ok(writer.into_inner())
}

/// Parses a whole `TEXTURE1`/`TEXTURE2` lump, detecting its record layout.
///
/// The `Doom` layout is tried first; a record that overruns the lump or
/// the next record's offset, or a patch index at or past `pnames_len`,
/// falls back to `Strife`. Pass [`usize::MAX`] when no `PNAMES` bounds the
/// indices.
///
/// # Errors
///
/// [`CyberError::Format`] quoting both attempts when neither layout reads
/// `data` cleanly.
#[inline]
pub fn parse_texture_lump(data: &[u8], pnames_len: usize) -> Result<TextureLump, CyberError> {
    match parse_with_layout(data, pnames_len, TextureLayout::Doom) {
        Ok(lump) => Ok(lump),
        Err(doom_error) => {
            parse_with_layout(data, pnames_len, TextureLayout::Strife).map_err(|strife_error| {
                CyberError::Format(format!(
                    "texture lump matches neither the Doom layout ({doom_error}) nor the Strife \
                     layout ({strife_error})"
                ))
            })
        }
    }
}

/// Writes `lump` back in its recorded layout, with `0` column directories,
/// a `stepdir` of `1` and `colormap` of `0`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name longer than eight bytes;
/// [`CyberError::LimitExceeded`] when a count or an offset outgrows the
/// stored `i32`/`u16` fields.
#[inline]
pub fn write_texture_lump(lump: &TextureLump) -> Result<Vec<u8>, CyberError> {
    let count = count_field(lump.textures.len(), "texture lump")?;
    let planned = plan_records(lump)?;
    let mut writer = Writer::with_capacity(planned.total_len);
    writer.put_i32(count);
    for offset in &planned.offsets {
        writer.put_i32(*offset);
    }
    for (index, texture) in lump.textures.iter().enumerate() {
        write_texture(&mut writer, texture, lump.layout, index)?;
    }
    Ok(writer.into_inner())
}

/// The planned offset table of one lump, plus its final byte length.
struct Planned {
    /// `i32` offsets from the lump start, one per texture.
    offsets: Vec<i32>,
    /// Total bytes of the finished lump.
    total_len: usize,
}

/// Lays out the records: the count, the offset table, then every record
/// back to back.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a patch count or an offset does not
/// fit its field.
fn plan_records(lump: &TextureLump) -> Result<Planned, CyberError> {
    let table_len = WORD_LEN.saturating_mul(lump.textures.len().saturating_add(1));
    let mut offsets = Vec::with_capacity(lump.textures.len());
    let mut position = table_len;
    for (index, texture) in lump.textures.iter().enumerate() {
        let at = i32::try_from(position).map_err(|error| {
            CyberError::LimitExceeded(format!(
                "texture {index} offset {position} does not fit an i32 field: {error}"
            ))
        })?;
        offsets.push(at);
        let patches = usize::from(patch_count(texture, index)?);
        position = position
            .checked_add(lump.layout.head_len())
            .and_then(|head| head.checked_add(lump.layout.patch_len().saturating_mul(patches)))
            .ok_or_else(|| {
                CyberError::LimitExceeded(format!(
                    "texture {index} record does not fit the addressable range"
                ))
            })?;
    }
    Ok(Planned {
        offsets,
        total_len: position,
    })
}

/// The patch count of one texture as the `u16` the format stores.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] above `u16::MAX`.
fn patch_count(texture: &TextureDef, index: usize) -> Result<u16, CyberError> {
    u16::try_from(texture.patches.len()).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "texture {index} ({}) holds {} patches, the format stores at most {}: {error}",
            texture.name,
            texture.patches.len(),
            u16::MAX
        ))
    })
}

/// One texture record: the head fields and the mappatch list.
fn write_texture(
    writer: &mut Writer,
    texture: &TextureDef,
    layout: TextureLayout,
    index: usize,
) -> Result<(), CyberError> {
    writer.put_padded_str(&texture.name, NAME_WIDTH)?;
    writer.put_u32(texture.flags_raw);
    writer.put_u16(texture.width);
    writer.put_u16(texture.height);
    if layout == TextureLayout::Doom {
        writer.put_i32(0_i32);
    }
    writer.put_u16(patch_count(texture, index)?);
    for patch in &texture.patches {
        writer.put_i16(patch.x);
        writer.put_i16(patch.y);
        writer.put_u16(patch.patch);
        if layout == TextureLayout::Doom {
            writer.put_u16(DEFAULT_STEP_DIR);
            writer.put_u16(0_u16);
        }
    }
    Ok(())
}

/// Reads the whole lump in one fixed layout, checking every record stays
/// inside the lump and inside the next record's start.
fn parse_with_layout(
    data: &[u8],
    pnames_len: usize,
    layout: TextureLayout,
) -> Result<TextureLump, CyberError> {
    let mut header = Reader::new(data);
    let count = entry_count(header.read_i32()?, "texture lump")?;
    let mut offsets = Vec::new();
    for _entry in 0..count {
        offsets.push(header.read_i32()?);
    }
    let mut textures = Vec::new();
    for (index, &start_raw) in offsets.iter().enumerate() {
        let limit = record_limit(&offsets, index, data.len())?;
        let mut reader = Reader::at(data, record_start(start_raw, index)?);
        let texture = read_texture(&mut reader, layout, pnames_len, index)?;
        if reader.pos() > limit {
            return Err(CyberError::Format(format!(
                "texture {index} record ends at {}, past the limit {limit} set by the lump or \
                 the next record offset",
                reader.pos()
            )));
        }
        textures.push(texture);
    }
    Ok(TextureLump { layout, textures })
}

/// The byte a record starts at, refusing a negative offset.
///
/// # Errors
///
/// [`CyberError::Format`] when `raw` is negative or cannot name a byte
/// position on this target.
fn record_start(raw: i32, index: usize) -> Result<usize, CyberError> {
    usize::try_from(raw).map_err(|error| {
        CyberError::Format(format!(
            "texture {index} has the lump offset {raw}, which no reader can seek to: {error}"
        ))
    })
}

/// How far record `index` may reach: the next record's start clipped to the
/// lump, or the lump's end for the last record.
///
/// # Errors
///
/// [`CyberError::Format`] when the following offset is negative.
fn record_limit(offsets: &[i32], index: usize, data_len: usize) -> Result<usize, CyberError> {
    let next = index.saturating_add(1);
    let Some(&next_raw) = offsets.get(next) else {
        return Ok(data_len);
    };
    let start = record_start(next_raw, next)?;
    Ok(start.min(data_len))
}

/// What reading one mappatch needs to know about its record and its lump.
struct RecordRead<'name> {
    /// The layout being read.
    layout: TextureLayout,
    /// The `PNAMES` length that bounds the indices.
    pnames_len: usize,
    /// The record's name, for error messages.
    name: &'name str,
    /// The record's position in the lump, for error messages.
    index: usize,
}

/// One texture record, leaving `reader` positioned past its last field.
fn read_texture(
    reader: &mut Reader<'_>,
    layout: TextureLayout,
    pnames_len: usize,
    index: usize,
) -> Result<TextureDef, CyberError> {
    let name = record_name(reader.read_array::<NAME_WIDTH>()?);
    let flags_raw = reader.read_u32()?;
    let width = reader.read_u16()?;
    let height = reader.read_u16()?;
    if layout == TextureLayout::Doom {
        reader.read_i32().map(|_| ())?;
    }
    let count = usize::from(reader.read_u16()?);
    let context = RecordRead {
        layout,
        pnames_len,
        name: &name,
        index,
    };
    let mut patches = Vec::with_capacity(count);
    for slot in 0..count {
        patches.push(read_patch(reader, &context, slot)?);
    }
    Ok(TextureDef {
        name,
        flags_raw,
        width,
        height,
        patches,
    })
}

/// One mappatch: origins, the `PNAMES` index and, in the `Doom` layout,
/// the `stepdir` and `colormap` words the model does not keep.
///
/// # Errors
///
/// [`CyberError::Format`] for a truncated field or a patch index at or
/// past `pnames_len`, naming the texture and the slot.
fn read_patch(
    reader: &mut Reader<'_>,
    context: &RecordRead<'_>,
    slot: usize,
) -> Result<PatchRef, CyberError> {
    let x = reader.read_i16()?;
    let y = reader.read_i16()?;
    let patch = reader.read_u16()?;
    if context.layout == TextureLayout::Doom {
        reader.read_u16().map(|_| ())?;
        reader.read_u16().map(|_| ())?;
    }
    if usize::from(patch) >= context.pnames_len {
        return Err(CyberError::Format(format!(
            "texture {} ({}) patch {slot} refers to PNAMES index {patch}, but only {} names \
             are defined",
            context.index, context.name, context.pnames_len
        )));
    }
    Ok(PatchRef { patch, x, y })
}

/// Renumbers the patch references of `lump` through `mapping`.
///
/// `Some(new)` moves a reference, `None` drops it together with its patch,
/// and an index at or past the end of the mapping is dropped too. Returns
/// the number of textures that changed.
#[must_use]
#[inline]
pub fn remap_patches(lump: &mut TextureLump, mapping: &PatchMapping) -> usize {
    let mut changed = 0_usize;
    for texture in &mut lump.textures {
        let patches = remap_list(&texture.patches, mapping);
        if patches != texture.patches {
            changed = changed.saturating_add(1);
            texture.patches = patches;
        }
    }
    changed
}

/// The remapped mappatch list of one texture.
fn remap_list(patches: &[PatchRef], mapping: &PatchMapping) -> Vec<PatchRef> {
    patches
        .iter()
        .filter_map(|patch| {
            let new = (*mapping.get(usize::from(patch.patch))?)?;
            Some(PatchRef {
                patch: new,
                x: patch.x,
                y: patch.y,
            })
        })
        .collect()
}

/// The names of the textures in `lump` that `mapping` would drop a
/// reference from, for conflict reporting.
#[must_use]
#[inline]
pub fn dropped_references(lump: &TextureLump, mapping: &PatchMapping) -> Vec<String> {
    lump.textures
        .iter()
        .filter(|texture| {
            texture
                .patches
                .iter()
                .any(|patch| !matches!(mapping.get(usize::from(patch.patch)), Some(Some(_))))
        })
        .map(|texture| texture.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        PatchMapping, PatchRef, TextureDef, TextureLayout, TextureLump, dropped_references,
        parse_pnames, parse_texture_lump, remap_patches, write_pnames, write_texture_lump,
    };
    use crate::error::CyberError;
    use crate::wad::bytes::Writer;

    /// A texture referring to `patches`, each at its own horizontal step.
    fn texture(name: &str, width: u16, height: u16, patches: &[u16]) -> TextureDef {
        TextureDef {
            name: name.to_owned(),
            flags_raw: 0,
            width,
            height,
            patches: patches
                .iter()
                .enumerate()
                .map(|(slot, &patch)| PatchRef {
                    patch,
                    x: i16::try_from(slot).expect("test counts fit"),
                    y: 8_i16,
                })
                .collect(),
        }
    }

    /// A two-texture `Doom` lump: WALL1 refers to both names, STEP1 to the
    /// second alone.
    fn doom_lump() -> TextureLump {
        TextureLump {
            layout: TextureLayout::Doom,
            textures: vec![
                texture("WALL1", 64, 128, &[0, 0, 1]),
                texture("STEP1", 64, 32, &[1]),
            ],
        }
    }

    /// A `Strife` lump whose origins are read as a `Doom` patch count, so
    /// the `Doom` layout cannot accept it.
    fn strife_lump() -> TextureLump {
        TextureLump {
            layout: TextureLayout::Strife,
            textures: vec![TextureDef {
                name: "ASTRIFE".to_owned(),
                flags_raw: 1,
                width: 32,
                height: 32,
                patches: vec![
                    PatchRef {
                        patch: 0,
                        x: 0,
                        y: 8,
                    },
                    PatchRef {
                        patch: 0,
                        x: 16,
                        y: 8,
                    },
                ],
            }],
        }
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    #[test]
    fn pnames_round_trip_uppercases_and_trims() {
        let written =
            write_pnames(&["wallp1".to_owned(), "STEPP1".to_owned()]).expect("two valid names");
        assert_eq!(
            written.len(),
            4 + 8 + 8,
            "count plus two eight-byte entries"
        );
        assert_eq!(
            parse_pnames(&written).expect("the lump parses back"),
            vec!["WALLP1".to_owned(), "STEPP1".to_owned()],
            "names round-trip uppercased"
        );
    }

    #[test]
    fn empty_pnames_round_trips() {
        let written = write_pnames(&[]).expect("an empty list is writable");
        assert_eq!(written.len(), 4, "just the count");
        assert!(
            parse_pnames(&written).expect("empty parses").is_empty(),
            "no names"
        );
    }

    #[test]
    fn pnames_rejects_bad_names_and_malformed_counts() {
        let name_error =
            write_pnames(&["TOO LONG".to_owned()]).expect_err("a space is not part of a name");
        assert_eq!(code(&name_error), "invalid_argument", "bad name code");
        assert!(
            name_error.to_string().contains("entry 0"),
            "the message names the entry: {name_error}"
        );
        let mut negative = Writer::new();
        negative.put_i32(-1);
        let negative_error = parse_pnames(&negative.into_inner()).expect_err("negative count");
        assert_eq!(code(&negative_error), "format_error", "negative count code");
        let mut short = Writer::new();
        short.put_i32(3);
        short.put_bytes(b"WALLP1\0\0");
        let short_error = parse_pnames(&short.into_inner()).expect_err("list cut short");
        assert_eq!(code(&short_error), "format_error", "truncated list code");
        assert!(
            short_error.to_string().contains("end of data"),
            "the read error says where: {short_error}"
        );
    }

    #[test]
    fn doom_lump_round_trips_in_its_layout() {
        let lump = doom_lump();
        let bytes = write_texture_lump(&lump).expect("serializable");
        assert_eq!(
            bytes.len(),
            12 + 52 + 32,
            "count, two offsets, (22+30) and (22+10) records"
        );
        let parsed = parse_texture_lump(&bytes, 2).expect("a valid Doom lump");
        assert_eq!(parsed, lump, "layout and every field survive");
    }

    #[test]
    fn strife_lump_round_trips_and_is_detected() {
        let lump = strife_lump();
        let bytes = write_texture_lump(&lump).expect("serializable");
        assert_eq!(bytes.len(), 8 + 30, "count, one offset, an 18 + 2*6 record");
        let parsed = parse_texture_lump(&bytes, 1).expect("the Strife fallback reads it");
        assert_eq!(parsed.layout, TextureLayout::Strife, "layout detected");
        assert_eq!(parsed, lump, "every field survives the fallback");
        let back = write_texture_lump(&parsed).expect("re-serializable");
        assert_eq!(back, bytes, "the layout is written back unchanged");
    }

    #[test]
    fn out_of_range_patch_indices_fail_both_layouts() {
        let lump = TextureLump {
            layout: TextureLayout::Strife,
            textures: vec![texture("BADONE", 16, 16, &[3])],
        };
        let bytes = write_texture_lump(&lump).expect("serializable");
        let error = parse_texture_lump(&bytes, 3).expect_err("index 3 is not defined");
        assert_eq!(code(&error), "format_error", "both layouts fail");
        let message = error.to_string();
        assert!(
            message.contains("Doom layout")
                && message.contains("Strife layout")
                && message.contains("BADONE"),
            "the error names both attempts and the texture: {message}"
        );
    }

    #[test]
    fn a_doom_lump_beyond_its_names_falls_back_to_strife() {
        let bytes = write_texture_lump(&doom_lump()).expect("serializable");
        let fallback = parse_texture_lump(&bytes, 1).expect("the Strife read fits the bytes");
        assert_eq!(
            fallback.layout,
            TextureLayout::Strife,
            "documented fallback"
        );
        assert!(
            fallback.textures.iter().all(|def| def.patches.is_empty()),
            "the obsolete column directory reads as a zero patch count"
        );
    }

    #[test]
    fn malformed_lumps_fail_both_layouts_with_format() {
        let junk = [0_u8; 3];
        let short_error = parse_texture_lump(&junk, 4).expect_err("three bytes hold nothing");
        assert_eq!(code(&short_error), "format_error", "truncated header code");
        let mut wild = Writer::new();
        wild.put_i32(1);
        wild.put_i32(9999);
        let wild_error =
            parse_texture_lump(&wild.into_inner(), 4).expect_err("offset past the end");
        assert_eq!(code(&wild_error), "format_error", "wild offset code");
        assert!(
            wild_error.to_string().contains("9999"),
            "the message names the bad offset: {wild_error}"
        );
        let mut negative = Writer::new();
        negative.put_i32(-2);
        let negative_error = parse_texture_lump(&negative.into_inner(), 4)
            .expect_err("a negative count is no count");
        assert_eq!(code(&negative_error), "format_error", "negative count code");
        assert!(
            negative_error.to_string().contains("-2"),
            "the message names the count: {negative_error}"
        );
    }

    #[test]
    fn a_record_overrunning_the_next_offset_is_a_format_error() {
        let mut writer = Writer::new();
        writer.put_i32(2);
        writer.put_i32(12);
        writer.put_i32(13);
        writer.put_bytes(b"AA\0\0\0\0\0\0");
        writer.put_u32(0);
        writer.put_u16(4);
        writer.put_u16(4);
        writer.put_i32(0);
        writer.put_u16(0);
        let error = parse_texture_lump(&writer.into_inner(), 2).expect_err("records overlap");
        assert_eq!(code(&error), "format_error", "overrun code");
        assert!(
            error.to_string().contains("past the limit"),
            "the message names the overrun: {error}"
        );
    }

    #[test]
    fn a_negative_record_offset_is_a_format_error() {
        let mut writer = Writer::new();
        writer.put_i32(1);
        writer.put_i32(-4);
        let error = parse_texture_lump(&writer.into_inner(), 4).expect_err("negative offset");
        assert_eq!(code(&error), "format_error", "negative offset code");
        assert!(
            error.to_string().contains("lump offset -4"),
            "the message names the problem: {error}"
        );
    }

    #[test]
    fn zero_texture_lumps_parse_as_empty_doom_lumps() {
        let lump = TextureLump {
            layout: TextureLayout::Doom,
            textures: Vec::new(),
        };
        let bytes = write_texture_lump(&lump).expect("serializable");
        assert_eq!(bytes.len(), 4, "only the count");
        assert_eq!(
            parse_texture_lump(&bytes, 0).expect("empty"),
            lump,
            "round trip"
        );
    }

    #[test]
    fn oversized_fields_are_refused_on_the_way_out() {
        let long_name = write_texture_lump(&TextureLump {
            layout: TextureLayout::Doom,
            textures: vec![TextureDef {
                name: "TOOLONGNAME".to_owned(),
                flags_raw: 0,
                width: 1,
                height: 1,
                patches: Vec::new(),
            }],
        })
        .expect_err("eleven bytes do not fit the name field");
        assert_eq!(code(&long_name), "invalid_argument", "long name code");
        assert!(
            long_name.to_string().contains("TOOLONGNAME"),
            "the message names it: {long_name}"
        );
        let mut lump = doom_lump();
        let first = lump.textures.first_mut().expect("WALL1 is first");
        for _slot in 0..=u16::MAX {
            first.patches.push(PatchRef {
                patch: 0,
                x: 0,
                y: 0,
            });
        }
        let overflow = write_texture_lump(&lump).expect_err("65536 patches do not fit");
        assert_eq!(code(&overflow), "limit_exceeded", "patch overflow code");
        assert!(
            overflow.to_string().contains("WALL1"),
            "the message names the texture: {overflow}"
        );
    }

    #[test]
    fn remap_drops_and_renumbers_and_counts_changed_textures() {
        let mut lump = doom_lump();
        let mapping: PatchMapping = vec![None, Some(0)];
        assert_eq!(
            remap_patches(&mut lump, &mapping),
            2,
            "both textures change"
        );
        assert_eq!(
            lump.textures[0].patches,
            vec![PatchRef {
                patch: 0,
                x: 2,
                y: 8
            }],
            "WALL1's two index-0 refs dropped, its index-1 ref renumbered to 0"
        );
        assert_eq!(
            lump.textures[1].patches,
            vec![PatchRef {
                patch: 0,
                x: 0,
                y: 8
            }],
            "STEP1's ref moved from index 1 to 0"
        );
    }

    #[test]
    fn remap_with_an_identity_mapping_changes_nothing() {
        let mut lump = doom_lump();
        let mapping: PatchMapping = vec![Some(0), Some(1)];
        assert_eq!(remap_patches(&mut lump, &mapping), 0, "nothing moved");
        assert_eq!(lump, doom_lump(), "the lump is untouched");
    }

    #[test]
    fn dropped_references_lists_the_textures_at_risk() {
        let lump = doom_lump();
        let swapped: PatchMapping = vec![Some(1), Some(0)];
        assert!(
            dropped_references(&lump, &swapped).is_empty(),
            "a pure swap drops nothing"
        );
        let shrunk: PatchMapping = vec![Some(0)];
        assert_eq!(
            dropped_references(&lump, &shrunk),
            vec!["WALL1".to_owned(), "STEP1".to_owned()],
            "both refer to the dropped index 1"
        );
        let empty: PatchMapping = Vec::new();
        assert_eq!(
            dropped_references(&lump, &empty).len(),
            2,
            "an empty mapping leaves every reference nowhere to go"
        );
    }

    #[test]
    fn masked_and_zdoom_flag_words_survive_untouched() {
        let lump = TextureLump {
            layout: TextureLayout::Doom,
            textures: vec![TextureDef {
                name: "MASKED".to_owned(),
                flags_raw: 0x0101_0001,
                width: 8,
                height: 8,
                patches: Vec::new(),
            }],
        };
        let bytes = write_texture_lump(&lump).expect("serializable");
        assert_eq!(
            parse_texture_lump(&bytes, 0).expect("round trip"),
            lump,
            "the raw word is neither interpreted nor rewritten"
        );
    }
}
