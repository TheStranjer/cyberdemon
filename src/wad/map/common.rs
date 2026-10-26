//! Codecs and plumbing shared by the binary map formats.
//!
//! `Doom` and `Hexen` store `SIDEDEFS`, `VERTEXES` and `SECTORS` with
//! byte-identical little-endian records, so their readers and writers live
//! here once. So do the whole-block passes: both formats emit the same lump
//! run under the same zero-size header marker and carry every member they do
//! not build themselves, so [`read_binary_map`] and [`write_binary_map`] hold
//! that run once and take a [`BinaryLayout`] for the parts the two formats do
//! not share (their `THINGS` and `LINEDEFS` codecs, the label read errors name
//! and the `ACS` pair only `Hexen` writes).
//!
//! The module also holds the lump plumbing both formats use:
//! [`member_data`] looks a required member up by name, [`records`] slices a
//! payload into whole records, [`other_lumps`] and [`carried`] move the
//! non-editable block lumps through [`Map::other`] untouched, and [`Element`]
//! names the map element a write-time range error points at.
//!
//! [`Map::other`]: crate::wad::map::Map::other

use core::fmt::Display;
use core::slice::ChunksExact;

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::bytes::{Reader, Writer};
use crate::wad::classify::MapFormat;
use crate::wad::lumps;
use crate::wad::map::consts::NO_TEXTURE;
use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use crate::wad::name::{LumpName, NAME_LEN};

/// Bytes in one `SIDEDEFS` record.
pub(super) const SIDEDEF_LEN: usize = 30;
/// Bytes in one `VERTEXES` record.
pub(super) const VERTEX_LEN: usize = 4;
/// Bytes in one `SECTORS` record.
pub(super) const SECTOR_LEN: usize = 26;
/// The stored sidedef number meaning "this side does not exist".
pub(super) const NO_SIDEDEF: u16 = 0xFFFF;

/// The eight stored bytes of "no texture": the one-character name `"-"`
/// with `NUL` padding. Vanilla engines short-circuit a leading `-` in
/// `R_CheckTextureNumForName`, while an all-`NUL` field reaches the linear
/// name search, matches nothing and aborts the load, so an untextured face
/// has to write as this.
const NONE_FIELD: [u8; NAME_LEN] = *b"-\0\0\0\0\0\0\0";

/// The payload of member `wanted`, or a `Format` error naming it; `format`
/// names the map layout that needs the member.
pub(super) fn member_data<'members>(
    members: &'members [Lump],
    wanted: LumpName,
    format: &str,
) -> Result<&'members [u8], CyberError> {
    members
        .iter()
        .find(|lump| lump.name == wanted)
        .map(|lump| lump.data.as_ref())
        .ok_or_else(|| {
            CyberError::Format(format!(
                "the map block holds no {wanted} lump: a binary {format} map \
                 needs THINGS, LINEDEFS, SIDEDEFS, VERTEXES and SECTORS"
            ))
        })
}

/// The whole `size`-byte records of the `lump` payload, in order.
pub(super) fn records<'data>(
    lump: &str,
    data: &'data [u8],
    size: usize,
) -> Result<ChunksExact<'data, u8>, CyberError> {
    let chunks = data.chunks_exact(size);
    if chunks.remainder().is_empty() {
        return Ok(chunks);
    }
    Err(CyberError::Format(format!(
        "{lump} lump holds {} bytes, which is not a multiple of the \
         {size}-byte record size: rebuild the lump from whole records",
        data.len()
    )))
}

/// Whether `name` is one of the five editable binary map lumps,
/// [`lumps::EDITABLE_MAP_LUMPS`].
pub(super) fn is_editable(name: LumpName) -> bool {
    lumps::EDITABLE_MAP_LUMPS.contains(&name)
}

/// The members that are not editable lumps, in original order.
pub(super) fn other_lumps(members: &[Lump]) -> Vec<Lump> {
    members
        .iter()
        .filter(|lump| !is_editable(lump.name))
        .cloned()
        .collect()
}

/// The `other` lump named `wanted`, or an empty marker of that name.
pub(super) fn carried(other: &[Lump], wanted: LumpName) -> Lump {
    other
        .iter()
        .find(|lump| lump.name == wanted)
        .cloned()
        .unwrap_or_else(|| Lump::marker(wanted))
}

/// The lump run a binary format writes between `BLOCKMAP` and the remaining
/// `other` lumps, which [`write_binary_map`] appends; the empty run of the
/// layout that owns no lump beyond the vanilla block.
pub(super) const fn no_tail(_: &[Lump]) -> Vec<Lump> {
    Vec::new()
}

/// One layout's `THINGS` decoder.
type ThingReader = fn(&[u8]) -> Result<Vec<Thing>, CyberError>;
/// One layout's `LINEDEFS` decoder.
type LinedefReader = fn(&[u8]) -> Result<Vec<Linedef>, CyberError>;
/// One layout's `THINGS` encoder.
type ThingWriter = fn(&[Thing]) -> Result<Vec<u8>, CyberError>;
/// One layout's `LINEDEFS` encoder.
type LinedefWriter = fn(&[Linedef]) -> Result<Vec<u8>, CyberError>;
/// One layout's own lumps after the vanilla block.
type TailWriter = fn(&[Lump]) -> Vec<Lump>;

/// One binary map layout's own rules: everything the `Doom` and `Hexen`
/// block readers and writers do not share. Both formats write the same lump
/// run under the same zero-size header marker, decode `SIDEDEFS`, `VERTEXES`
/// and `SECTORS` with the same records and carry every other member through
/// [`Map::other`]; only the `THINGS` and `LINEDEFS` codecs, the word their
/// errors speak and the tail lumps they own differ, so [`read_binary_map`]
/// and [`write_binary_map`] hold the run once and take these rules for the
/// rest.
pub(super) struct BinaryLayout {
    /// The word read errors name the layout by: `Doom` or `Hexen`.
    pub(super) label: &'static str,
    /// The format the reader records on the model.
    pub(super) format: MapFormat,
    /// The lumps the writer emits itself, in emit order; a copy of one of
    /// these in [`Map::other`] is not appended again after the block.
    pub(super) standard: &'static [LumpName],
    /// Decodes the `THINGS` records into the model.
    pub(super) read_things: ThingReader,
    /// Decodes the `LINEDEFS` records into the model.
    pub(super) read_linedefs: LinedefReader,
    /// Encodes the model's things into the `THINGS` records.
    pub(super) write_things: ThingWriter,
    /// Encodes the model's linedefs into the `LINEDEFS` records.
    pub(super) write_linedefs: LinedefWriter,
    /// The lumps this layout owns beyond the vanilla block, written after
    /// `BLOCKMAP` from whatever [`Map::other`] carries.
    pub(super) tail: TailWriter,
}

/// Reads a whole binary map block under `layout`'s rules.
///
/// `name` is the block header's name; `members` are the block's lumps after
/// the header, in directory order. The five editable lumps decode into the
/// model, every other member is carried into [`Map::other`] verbatim, and the
/// fields the layout does not store keep their zero values.
///
/// # Errors
///
/// [`CyberError::Format`] when a required lump (THINGS, LINEDEFS, SIDEDEFS,
/// VERTEXES, SECTORS) is absent or when one holds a size that is not a whole
/// number of its records.
pub(super) fn read_binary_map(
    layout: &BinaryLayout,
    name: LumpName,
    members: &[Lump],
) -> Result<Map, CyberError> {
    Ok(Map {
        name,
        format: layout.format,
        udmf_namespace: None,
        extra_globals: Vec::new(),
        extra_blocks: Vec::new(),
        things: (layout.read_things)(member_data(members, lumps::THINGS, layout.label)?)?,
        vertices: read_vertices(member_data(members, lumps::VERTEXES, layout.label)?)?,
        linedefs: (layout.read_linedefs)(member_data(members, lumps::LINEDEFS, layout.label)?)?,
        sidedefs: read_sidedefs(member_data(members, lumps::SIDEDEFS, layout.label)?)?,
        sectors: read_sectors(member_data(members, lumps::SECTORS, layout.label)?)?,
        other: other_lumps(members),
    })
}

/// Writes a whole binary map block under `layout`'s rules.
///
/// The result is the zero-size header (named after [`Map::name`]) followed by
/// `THINGS LINEDEFS SIDEDEFS VERTEXES`, the classic node run, `SECTORS`,
/// `REJECT` and `BLOCKMAP`, then the layout's own [`tail`]. `SEGS`,
/// `SSECTORS`, `NODES`, `REJECT` and `BLOCKMAP` come from [`Map::other`] when
/// present and emit as empty markers otherwise; every `other` lump the layout
/// does not emit itself follows in directory order.
///
/// # Errors
///
/// Whatever the layout's `THINGS` and `LINEDEFS` encoders report, plus
/// [`CyberError::LimitExceeded`] or [`CyberError::InvalidArgument`] from the
/// shared record writers.
///
/// [`tail`]: BinaryLayout::tail
pub(super) fn write_binary_map(layout: &BinaryLayout, map: &Map) -> Result<Vec<Lump>, CyberError> {
    let owned = layout.standard.len().saturating_add(1);
    let mut lumps = Vec::with_capacity(owned.saturating_add(map.other.len()));
    lumps.push(Lump::marker(map.name));
    lumps.push(Lump::new(
        lumps::THINGS,
        (layout.write_things)(&map.things)?,
    ));
    lumps.push(Lump::new(
        lumps::LINEDEFS,
        (layout.write_linedefs)(&map.linedefs)?,
    ));
    lumps.push(Lump::new(lumps::SIDEDEFS, write_sidedefs(&map.sidedefs)?));
    lumps.push(Lump::new(lumps::VERTEXES, write_vertexes(&map.vertices)?));
    for node in lumps::CLASSIC_NODE_RUN_LUMPS {
        lumps.push(carried(&map.other, node));
    }
    lumps.push(Lump::new(lumps::SECTORS, write_sectors(&map.sectors)?));
    for trailing in [lumps::REJECT, lumps::BLOCKMAP] {
        lumps.push(carried(&map.other, trailing));
    }
    lumps.extend((layout.tail)(&map.other));
    lumps.extend(
        map.other
            .iter()
            .filter(|lump| !layout.standard.contains(&lump.name))
            .cloned(),
    );
    Ok(lumps)
}

/// Decodes one eight-byte texture/flat field: `NUL`-trimmed and uppercase,
/// with an empty field reading as the model's `"-"`. Every other byte is
/// read as the `Latin-1` character it is, which [`put_name`] writes back as
/// that same byte.
fn decode_name(bytes: &[u8]) -> String {
    let text: String = bytes
        .iter()
        .copied()
        .take_while(|&byte| byte != 0_u8)
        .map(|byte| char::from(byte).to_ascii_uppercase())
        .collect();
    if text.is_empty() {
        NO_TEXTURE.to_owned()
    } else {
        text
    }
}

/// The stored bytes of one model texture/flat name: a character within
/// `U+0000..=U+00FF` becomes the single raw byte [`decode_name`] read it
/// from, so a name the model did not change round-trips byte-identically
/// even when it carries a byte above `0x7F`; a wider character contributes
/// its whole `UTF-8` encoding.
fn name_bytes(name: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(name.len());
    for ch in name.chars() {
        let Ok(byte) = u8::try_from(u32::from(ch)) else {
            let mut encoded = [0_u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
            continue;
        };
        bytes.push(byte);
    }
    bytes
}

/// Appends one model texture/flat name as its eight-byte field: the model's
/// `"-"` and the empty string as [`NONE_FIELD`], any other name as its
/// [`name_bytes`] with `NUL` padding.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the name needs more than `NAME_LEN`
/// bytes; nothing is appended in that case.
fn put_name(writer: &mut Writer, name: &str) -> Result<(), CyberError> {
    if name == NO_TEXTURE || name.is_empty() {
        writer.put_bytes(&NONE_FIELD);
        return Ok(());
    }
    let bytes = name_bytes(name);
    let count = bytes.len();
    if count > NAME_LEN {
        return Err(CyberError::InvalidArgument(format!(
            "texture name {name:?} is {count} bytes, longer than the \
             {NAME_LEN}-byte field; use at most {NAME_LEN} characters"
        )));
    }
    writer.put_padded_bytes(&bytes, NAME_LEN)
}

/// The stored side number for a model side reference: `None` becomes the
/// `0xFFFF` sentinel.
pub(super) fn read_side(raw: u16) -> Option<usize> {
    (raw != NO_SIDEDEF).then_some(usize::from(raw))
}

/// Reads the `VERTEXES` records.
pub(super) fn read_vertices(data: &[u8]) -> Result<Vec<Vertex>, CyberError> {
    let mut vertices = Vec::new();
    for record in records("VERTEXES", data, VERTEX_LEN)? {
        let mut reader = Reader::new(record);
        vertices.push(Vertex {
            x: i32::from(reader.read_i16()?),
            y: i32::from(reader.read_i16()?),
            ..Default::default()
        });
    }
    Ok(vertices)
}

/// Reads the `SIDEDEFS` records: offsets, the three texture names and the
/// sector number.
pub(super) fn read_sidedefs(data: &[u8]) -> Result<Vec<Sidedef>, CyberError> {
    let mut sidedefs = Vec::new();
    for record in records("SIDEDEFS", data, SIDEDEF_LEN)? {
        let mut reader = Reader::new(record);
        sidedefs.push(Sidedef {
            x_offset: i32::from(reader.read_i16()?),
            y_offset: i32::from(reader.read_i16()?),
            upper: decode_name(reader.read_bytes(NAME_LEN)?),
            lower: decode_name(reader.read_bytes(NAME_LEN)?),
            middle: decode_name(reader.read_bytes(NAME_LEN)?),
            sector: usize::from(reader.read_u16()?),
            ..Default::default()
        });
    }
    Ok(sidedefs)
}

/// Reads the `SECTORS` records: heights, flats, light, special and tag.
pub(super) fn read_sectors(data: &[u8]) -> Result<Vec<Sector>, CyberError> {
    let mut sectors = Vec::new();
    for record in records("SECTORS", data, SECTOR_LEN)? {
        let mut reader = Reader::new(record);
        sectors.push(Sector {
            floor_height: i32::from(reader.read_i16()?),
            ceiling_height: i32::from(reader.read_i16()?),
            floor_flat: decode_name(reader.read_bytes(NAME_LEN)?),
            ceiling_flat: decode_name(reader.read_bytes(NAME_LEN)?),
            light: i32::from(reader.read_i16()?),
            special: i32::from(reader.read_u16()?),
            tag: i32::from(reader.read_u16()?),
            ..Default::default()
        });
    }
    Ok(sectors)
}

/// Writes the `VERTEXES` records.
pub(super) fn write_vertexes(vertices: &[Vertex]) -> Result<Vec<u8>, CyberError> {
    let mut writer = Writer::new();
    for (index, vertex) in vertices.iter().enumerate() {
        let at = Element {
            kind: "vertex",
            index,
        };
        writer.put_i16(at.i16("x", vertex.x)?);
        writer.put_i16(at.i16("y", vertex.y)?);
    }
    Ok(writer.into_inner())
}

/// Writes the `SIDEDEFS` records.
pub(super) fn write_sidedefs(sidedefs: &[Sidedef]) -> Result<Vec<u8>, CyberError> {
    let mut writer = Writer::new();
    for (index, sidedef) in sidedefs.iter().enumerate() {
        let at = Element {
            kind: "sidedef",
            index,
        };
        writer.put_i16(at.i16("x_offset", sidedef.x_offset)?);
        writer.put_i16(at.i16("y_offset", sidedef.y_offset)?);
        put_name(&mut writer, &sidedef.upper)?;
        put_name(&mut writer, &sidedef.lower)?;
        put_name(&mut writer, &sidedef.middle)?;
        writer.put_u16(at.index("sector", sidedef.sector)?);
    }
    Ok(writer.into_inner())
}

/// Writes the `SECTORS` records.
pub(super) fn write_sectors(sectors: &[Sector]) -> Result<Vec<u8>, CyberError> {
    let mut writer = Writer::new();
    for (index, sector) in sectors.iter().enumerate() {
        let at = Element {
            kind: "sector",
            index,
        };
        writer.put_i16(at.i16("floor_height", sector.floor_height)?);
        writer.put_i16(at.i16("ceiling_height", sector.ceiling_height)?);
        put_name(&mut writer, &sector.floor_flat)?;
        put_name(&mut writer, &sector.ceiling_flat)?;
        writer.put_i16(at.i16("light", sector.light)?);
        writer.put_u16(at.u16("special", sector.special)?);
        writer.put_u16(at.u16("tag", sector.tag)?);
    }
    Ok(writer.into_inner())
}

/// One map element being written, for naming it in range errors.
#[derive(Clone, Copy)]
pub(super) struct Element {
    /// The element kind: `thing`, `linedef`, `sidedef`, `vertex` or
    /// `sector`.
    pub(super) kind: &'static str,
    /// The element's position in its block.
    pub(super) index: usize,
}

impl Element {
    /// The `LimitExceeded` saying this element's `field` value does not fit
    /// the stored type `ty`.
    fn overflow<V: Display>(self, field: &str, value: V, ty: &str) -> CyberError {
        CyberError::LimitExceeded(format!(
            "{} {} {field} {value} does not fit {ty}",
            self.kind, self.index
        ))
    }

    /// The `i16` form of an `i32` model field.
    pub(in crate::wad::map) fn i16(self, field: &str, value: i32) -> Result<i16, CyberError> {
        let Ok(small) = i16::try_from(value) else {
            return Err(self.overflow(field, value, "i16"));
        };
        Ok(small)
    }

    /// The `u16` form of a non-negative `i32` model field.
    pub(in crate::wad::map) fn u16(self, field: &str, value: i32) -> Result<u16, CyberError> {
        let Ok(small) = u16::try_from(value) else {
            return Err(self.overflow(field, value, "u16"));
        };
        Ok(small)
    }

    /// The `u8` form of a non-negative `i32` model field.
    pub(in crate::wad::map) fn u8(self, field: &str, value: i32) -> Result<u8, CyberError> {
        let Ok(small) = u8::try_from(value) else {
            return Err(self.overflow(field, value, "u8"));
        };
        Ok(small)
    }

    /// The `u16` form of a flag bitset.
    pub(in crate::wad::map) fn u16_bits(self, field: &str, value: u32) -> Result<u16, CyberError> {
        let Ok(small) = u16::try_from(value) else {
            return Err(self.overflow(field, value, "u16"));
        };
        Ok(small)
    }

    /// The `u16` form of a reference index; `0xFFFF` is reserved for the
    /// "no side" sentinel, so a real index must stay below it.
    pub(in crate::wad::map) fn index(self, field: &str, value: usize) -> Result<u16, CyberError> {
        let Ok(small) = u16::try_from(value) else {
            return Err(self.overflow(field, value, "u16"));
        };
        if small == NO_SIDEDEF {
            return Err(CyberError::LimitExceeded(format!(
                "{} {} {field} 65535 collides with the reserved no-sidedef \
                 marker: keep {field} below 65535",
                self.kind, self.index
            )));
        }
        Ok(small)
    }

    /// The stored side number: `None` becomes the `0xFFFF` sentinel.
    pub(in crate::wad::map) fn side(
        self,
        field: &str,
        value: Option<usize>,
    ) -> Result<u16, CyberError> {
        value.map_or(Ok(NO_SIDEDEF), |index| self.index(field, index))
    }

    /// Appends the five script arguments of one record as `u8` fields,
    /// naming the position in range errors.
    pub(in crate::wad::map) fn put_args(
        self,
        writer: &mut Writer,
        args: &[i32; 5],
    ) -> Result<(), CyberError> {
        for (position, value) in args.iter().enumerate() {
            let field = format!("args[{position}]");
            writer.put_u8(self.u8(&field, *value)?);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(super) mod fixtures {
    //! Element builders shared by the binary-format unit tests. The
    //! two-room map mirrors the `tests/mcp/fixtures.rs` WAD builder.

    use alloc::collections::BTreeMap;

    use crate::wad::archive::Lump;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    pub(in crate::wad::map) fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` holding `data`.
    pub(in crate::wad::map) fn lump(text: &str, data: &[u8]) -> Lump {
        Lump::new(name(text), data.to_vec())
    }

    /// An empty marker lump named `text`.
    pub(in crate::wad::map) fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A vertex at `x`/`y`.
    pub(in crate::wad::map) const fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            extra: BTreeMap::new(),
        }
    }

    /// A fully visible spawn of `kind` at `x`/`y` facing `angle`.
    pub(in crate::wad::map) fn spawn(kind: i32, x: i32, y: i32, angle: i32) -> Thing {
        Thing {
            kind,
            x,
            y,
            angle,
            flags: 7,
            ..Default::default()
        }
    }

    /// A blocking one-sided line from `v1` to `v2` with sidedef `front`.
    pub(in crate::wad::map) fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// The two-sided step line between the rooms: flags 4, sidedefs 2/3.
    pub(in crate::wad::map) fn step_line() -> Linedef {
        Linedef {
            v1: 4,
            v2: 1,
            flags: 4,
            front: Some(2),
            back: Some(3),
            ..Default::default()
        }
    }

    /// A sidedef with the given names over `sector`.
    pub(in crate::wad::map) fn side(
        upper: &str,
        lower: &str,
        middle: &str,
        sector: usize,
    ) -> Sidedef {
        Sidedef {
            upper: upper.to_owned(),
            lower: lower.to_owned(),
            middle: middle.to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// A `FLOOR1`-floored sector with the given heights, light and tag.
    pub(in crate::wad::map) fn room(
        floor_height: i32,
        ceiling_height: i32,
        light: i32,
        tag: i32,
    ) -> Sector {
        Sector {
            floor_height,
            ceiling_height,
            floor_flat: "FLOOR1".to_owned(),
            ceiling_flat: "FLOOR1".to_owned(),
            light,
            tag,
            ..Default::default()
        }
    }

    /// The empty generated node lumps a built map carries in [`Map::other`].
    pub(in crate::wad::map) fn node_lumps() -> Vec<Lump> {
        ["SEGS", "SSECTORS", "NODES", "REJECT", "BLOCKMAP"]
            .into_iter()
            .map(marker)
            .collect()
    }

    /// The bytes of the written lump named `wanted`, which the byte goldens
    /// read out of a written block.
    pub(in crate::wad::map) fn payload(lumps: &[Lump], wanted: &str) -> Vec<u8> {
        lumps
            .iter()
            .find(|lump| lump.name.to_string() == wanted)
            .map(|lump| lump.data.to_vec())
            .expect("the writer emits the named lump")
    }

    /// The `SIDEDEFS` record [`one_of_each_map`]'s sidedef writes: the two
    /// `i16` offsets, the none-texture above `STEP1` and `WALL1` and the
    /// sector index. Byte-identical in both binary formats.
    pub(in crate::wad::map) const GOLDEN_SIDEDEFS: [u8; 30] = [
        0xfc_u8, 0xff_u8, 0x08_u8, 0x00_u8, 0x2d_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8,
        0x00_u8, 0x00_u8, 0x53_u8, 0x54_u8, 0x45_u8, 0x50_u8, 0x31_u8, 0x00_u8, 0x00_u8, 0x00_u8,
        0x57_u8, 0x41_u8, 0x4c_u8, 0x4c_u8, 0x31_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8,
    ];
    /// The `VERTEXES` record [`one_of_each_map`]'s vertex writes: `x` `-256`
    /// and `y` `1024` as two `i16` fields.
    pub(in crate::wad::map) const GOLDEN_VERTEXES: [u8; 4] = [0x00_u8, 0xff_u8, 0x00_u8, 0x04_u8];
    /// The `SECTORS` record [`one_of_each_map`]'s sector writes: the two
    /// heights, the two flats, the light level and the special and tag.
    pub(in crate::wad::map) const GOLDEN_SECTORS: [u8; 26] = [
        0x00_u8, 0x00_u8, 0x80_u8, 0x00_u8, 0x46_u8, 0x4c_u8, 0x4f_u8, 0x4f_u8, 0x52_u8, 0x34_u8,
        0x5f_u8, 0x38_u8, 0x2d_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8,
        0xc0_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x01_u8, 0x00_u8,
    ];

    /// One element of every kind, the shape both binary writers' byte
    /// goldens are taken from: a thing carrying the `Hexen`-only `tid`, `z`,
    /// `special` and `args` that `Doom` has no field for, a line whose
    /// `special` fits both stored widths and whose `args` only `Hexen`
    /// stores, one sidedef with the none-texture above a real name and one
    /// sector with a full eight-byte flat and an empty ceiling.
    pub(in crate::wad::map) fn one_of_each_map() -> Map {
        Map {
            name: name("MAP01"),
            format: MapFormat::Doom,
            things: vec![Thing {
                tid: 5,
                x: -24,
                y: 300,
                z: 128,
                angle: 90,
                kind: 3001,
                flags: 7,
                special: 3,
                args: [1_i32, 2_i32, 3_i32, 4_i32, 5_i32],
                ..Default::default()
            }],
            vertices: vec![vertex(-256, 1024)],
            linedefs: vec![Linedef {
                v1: 0,
                v2: 0,
                flags: 1,
                special: 12,
                tag: 0,
                args: [9_i32, 8_i32, 7_i32, 6_i32, 5_i32],
                front: Some(0),
                back: None,
                ..Default::default()
            }],
            sidedefs: vec![Sidedef {
                x_offset: -4,
                y_offset: 8,
                upper: "-".to_owned(),
                lower: "STEP1".to_owned(),
                middle: "WALL1".to_owned(),
                sector: 0,
                ..Default::default()
            }],
            sectors: vec![Sector {
                floor_height: 0,
                ceiling_height: 128,
                floor_flat: "FLOOR4_8".to_owned(),
                ceiling_flat: "-".to_owned(),
                light: 192,
                tag: 1,
                ..Default::default()
            }],
            other: node_lumps(),
            ..Default::default()
        }
    }

    /// The two-room test map: a hall and a raised back room split by a
    /// two-sided step line.
    pub(in crate::wad::map) fn two_room_map() -> Map {
        Map {
            name: name("MAP01"),
            format: MapFormat::Doom,
            things: vec![spawn(1, 64, 128, 0), spawn(3001, 384, 128, 180)],
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(512, 0),
                vertex(512, 256),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                solid_line(0, 5, 0),
                solid_line(5, 4, 1),
                step_line(),
                solid_line(1, 0, 4),
                solid_line(4, 3, 5),
                solid_line(3, 2, 6),
                solid_line(2, 1, 7),
            ],
            sidedefs: vec![
                side("-", "-", "WALL1", 0),
                side("-", "-", "WALL1", 0),
                side("-", "STEP1", "-", 0),
                side("-", "-", "-", 1),
                side("-", "-", "WALL1", 0),
                side("-", "-", "WALL1", 1),
                side("-", "-", "WALL1", 1),
                side("-", "-", "WALL1", 1),
            ],
            sectors: vec![room(0, 128, 160, 0), room(32, 128, 192, 1)],
            other: node_lumps(),
            ..Default::default()
        }
    }
}
