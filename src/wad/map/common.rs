//! Codecs and plumbing shared by the binary map formats.
//!
//! `Doom` and `Hexen` store `SIDEDEFS`, `VERTEXES` and `SECTORS` with
//! byte-identical little-endian records, so their readers and writers live
//! here once. The module also holds the lump plumbing both formats use:
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
use crate::wad::map::model::{Sector, Sidedef, Vertex};
use crate::wad::name::{LumpName, NAME_LEN};

/// Bytes in one `SIDEDEFS` record.
pub(super) const SIDEDEF_LEN: usize = 30;
/// Bytes in one `VERTEXES` record.
pub(super) const VERTEX_LEN: usize = 4;
/// Bytes in one `SECTORS` record.
pub(super) const SECTOR_LEN: usize = 26;
/// The stored sidedef number meaning "this side does not exist".
pub(super) const NO_SIDEEDEF: u16 = 0xFFFF;

/// The model's `"-"` (or empty) name for one eight-byte texture field.
const NONE_NAME: &str = "-";

/// The payload of member `wanted`, or a `Format` error naming it; `format`
/// names the map layout that needs the member.
pub(super) fn member_data<'members>(
    members: &'members [Lump],
    wanted: &str,
    format: &str,
) -> Result<&'members [u8], CyberError> {
    members
        .iter()
        .find(|lump| lump.name.as_bytes() == wanted.as_bytes())
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

/// Whether `name` is one of the five editable binary map lumps.
pub(super) const fn is_editable(name: &[u8]) -> bool {
    matches!(
        name,
        b"THINGS" | b"LINEDEFS" | b"SIDEDEFS" | b"VERTEXES" | b"SECTORS"
    )
}

/// The members that are not editable lumps, in original order.
pub(super) fn other_lumps(members: &[Lump]) -> Vec<Lump> {
    members
        .iter()
        .filter(|lump| !is_editable(lump.name.as_bytes()))
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

/// Decodes one eight-byte texture/flat field: `NUL`-trimmed and uppercase,
/// with an empty field reading as the model's `"-"`.
fn decode_name(bytes: &[u8]) -> String {
    let text: String = bytes
        .iter()
        .copied()
        .take_while(|&byte| byte != 0_u8)
        .map(|byte| char::from(byte).to_ascii_uppercase())
        .collect();
    if text.is_empty() {
        NONE_NAME.to_owned()
    } else {
        text
    }
}

/// Appends one model texture/flat name as its eight-byte field; `"-"` and
/// the empty string write as all-`NUL`.
fn put_name(writer: &mut Writer, name: &str) -> Result<(), CyberError> {
    if name == NONE_NAME || name.is_empty() {
        writer.put_bytes(&[0_u8; NAME_LEN]);
        return Ok(());
    }
    writer.put_padded_str(name, NAME_LEN)
}

/// The stored side number for a model side reference: `None` becomes the
/// `0xFFFF` sentinel.
pub(super) fn read_side(raw: u16) -> Option<usize> {
    (raw != NO_SIDEEDEF).then_some(usize::from(raw))
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
        if small == NO_SIDEEDEF {
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
        value.map_or(Ok(NO_SIDEEDEF), |index| self.index(field, index))
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
