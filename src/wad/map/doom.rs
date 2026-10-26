//! The binary `Doom` map format: reading and writing vanilla map blocks.
//!
//! A binary `Doom` map is ten member lumps between the map header and the
//! next non-member: the five editable records ([`Thing`], [`Linedef`],
//! [`Sidedef`], [`Vertex`] and [`Sector`] rows) plus the generated
//! `SEGS`/`SSECTORS`/`NODES` block, `REJECT` and `BLOCKMAP`, which this
//! module copies verbatim.
//! Every field is little-endian: `THINGS` records are 10 bytes,
//! `LINEDEFS` 14, `SIDEDEFS` 30, `VERTEXES` 4 and `SECTORS` 26, with the
//! `0xFFFF` sidedef field meaning "no side". What `Doom` does not share is
//! its own `THINGS` and `LINEDEFS` codec pair below and the `LAYOUT` that
//! hands them to the block passes; the `SIDEDEFS`, `VERTEXES` and `SECTORS`
//! codecs, the whole-block lump run and the range-check helpers are shared
//! with the `Hexen` format through the private `common` module.
//! [`read_doom_map`] refuses a block
//! whose editable lump is missing or whose size is not a whole number of
//! records; [`write_doom_map`] refuses values outside the stored type and
//! names the offending element and field. Texture and flat fields hold
//! `NUL`-padded names: the model's `"-"` writes as `"-"` plus padding and an
//! all-`NUL` field reads back as `"-"`, so a name the model did not change
//! keeps its stored bytes.
//!
//! [`Sidedef`]: crate::wad::map::Sidedef
//! [`Sector`]: crate::wad::map::Sector
//! [`Vertex`]: crate::wad::map::Vertex

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::bytes::{Reader, Writer};
use crate::wad::classify::MapFormat;
use crate::wad::lumps;
use crate::wad::map::common::{
    BinaryLayout, Element, no_tail, read_binary_map, read_side, records, write_binary_map,
};
use crate::wad::map::model::{Linedef, Map, Thing};
use crate::wad::name::LumpName;

/// Bytes in one `THINGS` record.
const THING_LEN: usize = 10;
/// Bytes in one `LINEDEFS` record.
const LINEDEF_LEN: usize = 14;

/// The `Doom` block's rules: its own [`THINGS`] and [`LINEDEFS`] widths and
/// codecs, the vanilla ten-lump run the writer emits itself (so a copy of one
/// of those in [`Map::other`] is not appended twice) and no tail lump beyond
/// that run.
///
/// [`THINGS`]: read_things
/// [`LINEDEFS`]: read_linedefs
const LAYOUT: BinaryLayout = BinaryLayout {
    label: "Doom",
    format: MapFormat::Doom,
    standard: &lumps::STANDARD_DOOM_MAP_LUMPS,
    read_things,
    read_linedefs,
    write_things,
    write_linedefs,
    tail: no_tail,
};

/// Reads a whole binary `Doom` map.
///
/// `name` is the block header's name; `members` are the block's lumps after
/// the header, in directory order. The five editable lumps decode into the
/// model, every other member is carried into [`Map::other`] verbatim, and
/// `Doom` leaves the `Hexen`/`UDMF`-only fields (`tid`, `z`, thing
/// `special`/`args`, [`Sidedef::extra`] and friends) at their zero values.
/// The block run and the `SIDEDEFS`, `VERTEXES` and `SECTORS` records come
/// from the private `common` module, shared with the `Hexen` format.
///
/// # Errors
///
/// [`CyberError::Format`] when a required lump (THINGS, LINEDEFS, SIDEDEFS,
/// VERTEXES, SECTORS) is absent or when one holds a size that is not a
/// whole number of its records.
///
/// [`Map::other`]: crate::wad::map::Map::other
/// [`Sidedef::extra`]: crate::wad::map::Sidedef::extra
#[inline]
pub fn read_doom_map(name: LumpName, members: &[Lump]) -> Result<Map, CyberError> {
    read_binary_map(&LAYOUT, name, members)
}

/// Writes a whole binary `Doom` map.
///
/// The result is the zero-size header (named after [`Map::name`]) followed
/// by `THINGS LINEDEFS SIDEDEFS VERTEXES SEGS SSECTORS NODES SECTORS
/// REJECT BLOCKMAP`, taking `SEGS`, `SSECTORS`, `NODES`, `REJECT` and
/// `BLOCKMAP` from [`Map::other`] when present and emitting an empty lump
/// otherwise; any remaining [`other`] lumps follow in order. The `Doom`
/// format stores no `tid`, `z`, thing `special`/`args`, so those model
/// fields are not written. The lump run itself is shared with the `Hexen`
/// writer through the private `common` module.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a value does not fit its stored
/// field type or an index reaches the reserved `0xFFFF`;
/// [`CyberError::InvalidArgument`] when a texture or flat name is longer
/// than eight bytes.
///
/// [`Map::name`]: crate::wad::map::Map::name
/// [`Map::other`]: crate::wad::map::Map::other
/// [`other`]: Map::other
#[inline]
pub fn write_doom_map(map: &Map) -> Result<Vec<Lump>, CyberError> {
    write_binary_map(&LAYOUT, map)
}

/// Reads the `THINGS` records: x, y, angle and the u16 type/flags.
fn read_things(data: &[u8]) -> Result<Vec<Thing>, CyberError> {
    let mut things = Vec::new();
    for record in records("THINGS", data, THING_LEN)? {
        let mut reader = Reader::new(record);
        things.push(Thing {
            x: i32::from(reader.read_i16()?),
            y: i32::from(reader.read_i16()?),
            angle: i32::from(reader.read_i16()?),
            kind: i32::from(reader.read_u16()?),
            flags: u32::from(reader.read_u16()?),
            ..Default::default()
        });
    }
    Ok(things)
}

/// Reads the `LINEDEFS` records: vertex indices, flags, special, tag and
/// the two sidedef numbers.
fn read_linedefs(data: &[u8]) -> Result<Vec<Linedef>, CyberError> {
    let mut linedefs = Vec::new();
    for record in records("LINEDEFS", data, LINEDEF_LEN)? {
        let mut reader = Reader::new(record);
        let v1 = reader.read_u16()?;
        let v2 = reader.read_u16()?;
        let flags = reader.read_u16()?;
        let special = reader.read_u16()?;
        let tag = reader.read_u16()?;
        let right = reader.read_u16()?;
        let left = reader.read_u16()?;
        linedefs.push(Linedef {
            v1: usize::from(v1),
            v2: usize::from(v2),
            flags: u32::from(flags),
            special: i32::from(special),
            tag: i32::from(tag),
            front: read_side(right),
            back: read_side(left),
            ..Default::default()
        });
    }
    Ok(linedefs)
}

/// Writes the `THINGS` records; `tid`, `z`, `special` and `args` have no
/// place in the binary `Doom` format and are not written.
fn write_things(things: &[Thing]) -> Result<Vec<u8>, CyberError> {
    let mut writer = Writer::new();
    for (index, spawn) in things.iter().enumerate() {
        let at = Element {
            kind: "thing",
            index,
        };
        writer.put_i16(at.i16("x", spawn.x)?);
        writer.put_i16(at.i16("y", spawn.y)?);
        writer.put_i16(at.i16("angle", spawn.angle)?);
        writer.put_u16(at.u16("type", spawn.kind)?);
        writer.put_u16(at.u16_bits("flags", spawn.flags)?);
    }
    Ok(writer.into_inner())
}

/// Writes the `LINEDEFS` records, front side as the right sidedef.
fn write_linedefs(linedefs: &[Linedef]) -> Result<Vec<u8>, CyberError> {
    let mut writer = Writer::new();
    for (index, linedef) in linedefs.iter().enumerate() {
        let at = Element {
            kind: "linedef",
            index,
        };
        writer.put_u16(at.index("v1", linedef.v1)?);
        writer.put_u16(at.index("v2", linedef.v2)?);
        writer.put_u16(at.u16_bits("flags", linedef.flags)?);
        writer.put_u16(at.u16("special", linedef.special)?);
        writer.put_u16(at.u16("tag", linedef.tag)?);
        writer.put_u16(at.side("front", linedef.front)?);
        writer.put_u16(at.side("back", linedef.back)?);
    }
    Ok(writer.into_inner())
}

#[cfg(test)]
mod tests {
    use crate::error::CyberError;
    use crate::wad::archive::Lump;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::common::fixtures::{
        GOLDEN_SECTORS, GOLDEN_SIDEDEFS, GOLDEN_VERTEXES, lump, marker, name, one_of_each_map,
        payload, room, side, two_room_map,
    };
    use crate::wad::map::common::{
        NO_SIDEDEF, read_sectors, read_sidedefs, write_sectors, write_sidedefs,
    };
    use crate::wad::map::doom::{
        read_doom_map, read_linedefs, read_things, write_doom_map, write_linedefs, write_things,
    };
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Thing};
    use crate::wad::name::{LumpName, NAME_LEN};

    /// The `THINGS` record [`one_of_each_map`]'s thing writes: `x`, `y` and
    /// `angle` as `i16` then the `u16` type and flags. `tid`, `z`, `special`
    /// and `args` have no field in this format.
    const GOLDEN_THINGS: [u8; 10] = [
        0xe8_u8, 0xff_u8, 0x2c_u8, 0x01_u8, 0x5a_u8, 0x00_u8, 0xb9_u8, 0x0b_u8, 0x07_u8, 0x00_u8,
    ];
    /// The `LINEDEFS` record: the two vertex indices, flags, special and tag
    /// as `u16`, then the front side as the right sidedef and the `0xFFFF`
    /// no-side sentinel for the left.
    const GOLDEN_LINEDEFS: [u8; 14] = [
        0x00_u8, 0x00_u8, 0x00_u8, 0x00_u8, 0x01_u8, 0x00_u8, 0x0c_u8, 0x00_u8, 0x00_u8, 0x00_u8,
        0x00_u8, 0x00_u8, 0xff_u8, 0xff_u8,
    ];

    /// The block [`write_doom_map`] emits for [`one_of_each_map`]: the
    /// carried `SEGS` bytes, the empty generated members and one extra lump
    /// after the block.
    fn golden_map() -> Map {
        let mut map = one_of_each_map();
        map.other = vec![
            lump("SEGS", b"segs"),
            marker("SSECTORS"),
            marker("NODES"),
            marker("REJECT"),
            marker("BLOCKMAP"),
            lump("ZNODES", &[9_u8; 4]),
        ];
        map
    }

    #[test]
    fn the_doom_block_writes_its_golden_lump_run() {
        let map = golden_map();
        let lumps = write_doom_map(&map).expect("the golden map is writable");
        let layout: Vec<(String, usize)> = lumps
            .iter()
            .map(|lump| (lump.name.to_string(), lump.size()))
            .collect();
        assert_eq!(
            layout,
            vec![
                ("MAP01".to_owned(), 0),
                ("THINGS".to_owned(), 10),
                ("LINEDEFS".to_owned(), 14),
                ("SIDEDEFS".to_owned(), 30),
                ("VERTEXES".to_owned(), 4),
                ("SEGS".to_owned(), 4),
                ("SSECTORS".to_owned(), 0),
                ("NODES".to_owned(), 0),
                ("SECTORS".to_owned(), 26),
                ("REJECT".to_owned(), 0),
                ("BLOCKMAP".to_owned(), 0),
                ("ZNODES".to_owned(), 4),
            ],
            "the zero-size header, the ten vanilla members with the node bytes \
             `other` carries, then the extras"
        );
        let read = read_doom_map(map.name, lumps.get(1..).expect("members"))
            .expect("the golden block reads back");
        let mut stored = map;
        if let Some(spawn) = stored.things.first_mut() {
            spawn.tid = 0_i32;
            spawn.z = 0_i32;
            spawn.special = 0_i32;
            spawn.args = [0_i32; 5];
        }
        if let Some(line) = stored.linedefs.first_mut() {
            line.args = [0_i32; 5];
        }
        assert_eq!(
            read, stored,
            "the golden block is the map it came from, minus the fields Doom \
             has no place for"
        );
    }

    #[test]
    fn the_doom_records_hold_their_golden_bytes() {
        let lumps = write_doom_map(&golden_map()).expect("the golden map is writable");
        assert_eq!(
            payload(&lumps, "THINGS"),
            GOLDEN_THINGS,
            "the ten-byte Doom thing record"
        );
        assert_eq!(
            payload(&lumps, "LINEDEFS"),
            GOLDEN_LINEDEFS,
            "the fourteen-byte Doom linedef record"
        );
        assert_eq!(
            payload(&lumps, "SIDEDEFS"),
            GOLDEN_SIDEDEFS,
            "the record the two binary formats share"
        );
        assert_eq!(
            payload(&lumps, "VERTEXES"),
            GOLDEN_VERTEXES,
            "the record the two binary formats share"
        );
        assert_eq!(
            payload(&lumps, "SECTORS"),
            GOLDEN_SECTORS,
            "the record the two binary formats share"
        );
    }

    /// The `LimitExceeded` message one linedef row produces at write time.
    fn linedef_error(linedef: Linedef) -> String {
        match write_linedefs(&[linedef]) {
            Err(CyberError::LimitExceeded(message)) => message,
            Err(other) => panic!("expected LimitExceeded, got {other:?}"),
            Ok(bytes) => panic!("expected a rejection, wrote {} bytes", bytes.len()),
        }
    }

    /// The `LimitExceeded` message one sidedef row produces at write time.
    fn sidedef_error(sidedef: &Sidedef) -> String {
        match write_sidedefs(core::slice::from_ref(sidedef)) {
            Err(CyberError::LimitExceeded(message)) => message,
            Err(other) => panic!("expected LimitExceeded, got {other:?}"),
            Ok(bytes) => panic!("expected a rejection, wrote {} bytes", bytes.len()),
        }
    }

    /// The eight-byte name fields of one record that start at each of
    /// `offsets`, in order.
    fn name_fields(record: &[u8], offsets: &[usize]) -> Vec<[u8; NAME_LEN]> {
        offsets
            .iter()
            .map(|&start| {
                let end = start.checked_add(NAME_LEN).expect("a field fits");
                let mut field = [0_u8; NAME_LEN];
                field.copy_from_slice(record.get(start..end).expect("a whole name field"));
                field
            })
            .collect()
    }

    #[test]
    fn the_two_room_map_round_trips_through_the_binary_format() {
        let map = two_room_map();
        let lumps = write_doom_map(&map).expect("the test map is writable");
        let names: Vec<LumpName> = lumps.iter().map(|lump| lump.name).collect();
        assert_eq!(
            names,
            vec![
                name("MAP01"),
                name("THINGS"),
                name("LINEDEFS"),
                name("SIDEDEFS"),
                name("VERTEXES"),
                name("SEGS"),
                name("SSECTORS"),
                name("NODES"),
                name("SECTORS"),
                name("REJECT"),
                name("BLOCKMAP"),
            ],
            "header plus the ten vanilla members, in order"
        );
        assert!(
            lumps.first().is_some_and(|lump| lump.size() == 0),
            "the header is zero-size"
        );
        assert_eq!(lumps.get(1).map(Lump::size), Some(20), "two things");
        assert_eq!(lumps.get(2).map(Lump::size), Some(98), "seven linedefs");
        assert_eq!(lumps.get(3).map(Lump::size), Some(240), "eight sidedefs");
        assert_eq!(lumps.get(4).map(Lump::size), Some(24), "six vertices");
        assert_eq!(lumps.get(8).map(Lump::size), Some(52), "two sectors");
        let read = read_doom_map(map.name, lumps.get(1..).expect("members"))
            .expect("the written block reads back");
        assert_eq!(read, map, "read(write(map)) == map");
    }

    #[test]
    fn remaining_other_lumps_are_appended_after_the_block() {
        let mut map = two_room_map();
        map.other.push(lump("ZNODES", &[9_u8; 4]));
        map.other.push(lump("BEHAVIOR", &[8_u8; 3]));
        let lumps = write_doom_map(&map).expect("the test map is writable");
        let extras: Vec<String> = lumps
            .iter()
            .skip(11)
            .map(|lump| lump.name.to_string())
            .collect();
        assert_eq!(extras, vec!["ZNODES", "BEHAVIOR"], "extras keep order");
        let read = read_doom_map(map.name, lumps.get(1..).expect("members"))
            .expect("the extended block reads back");
        assert_eq!(read, map, "extras survive the round trip");
    }

    #[test]
    fn missing_node_lumps_write_as_empty_markers() {
        let mut map = two_room_map();
        map.other.clear();
        let lumps = write_doom_map(&map).expect("the test map is writable");
        for lump in lumps.iter().skip(5).take(3) {
            assert_eq!(lump.size(), 0, "the node block is empty");
        }
        let read = read_doom_map(map.name, lumps.get(1..).expect("members"))
            .expect("the block reads back");
        assert_eq!(read.other.len(), 5, "the five empty members are other");
    }

    #[test]
    fn things_decode_every_field_including_signed_and_high_u16() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&(-24_i16).to_le_bytes());
        buffer.extend_from_slice(&300_i16.to_le_bytes());
        buffer.extend_from_slice(&(-90_i16).to_le_bytes());
        buffer.extend_from_slice(&60000_u16.to_le_bytes());
        buffer.extend_from_slice(&NO_SIDEDEF.to_le_bytes());
        let things = read_things(&buffer).expect("one complete record");
        assert_eq!(
            things,
            vec![Thing {
                x: -24,
                y: 300,
                angle: -90,
                kind: 60000,
                flags: 0xFFFF,
                ..Default::default()
            }],
            "x, y, angle, type and flags decode in order, above-32767 u16 \
             values and negatives included"
        );
    }

    #[test]
    fn linedefs_decode_indices_high_u16_and_the_no_side_sentinel() {
        let mut buffer = Vec::new();
        for value in [0xFFFE_u16, 3, 13, 0xFFFF, 257, NO_SIDEDEF, 5] {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        let linedefs = read_linedefs(&buffer).expect("one complete record");
        assert_eq!(
            linedefs,
            vec![Linedef {
                v1: 0xFFFE,
                v2: 3,
                flags: 13,
                special: 0xFFFF,
                tag: 257,
                front: None,
                back: Some(5),
                ..Default::default()
            }],
            "the 0xFFFF sidedef is None, the other side survives, and u16 \
             values above 32767 keep their full width"
        );
    }

    #[test]
    fn sidedefs_decode_offsets_names_and_the_empty_name() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&(-4_i16).to_le_bytes());
        buffer.extend_from_slice(&8_i16.to_le_bytes());
        buffer.extend_from_slice(&[0_u8; 8]);
        buffer.extend_from_slice(b"step1\0\0\0");
        buffer.extend_from_slice(b"-\0\0\0\0\0\0\0");
        buffer.extend_from_slice(&3_u16.to_le_bytes());
        let sidedefs = read_sidedefs(&buffer).expect("one complete record");
        assert_eq!(
            sidedefs,
            vec![Sidedef {
                x_offset: -4,
                y_offset: 8,
                ..side("-", "STEP1", "-", 3)
            }],
            "empty reads as \"-\", lowercase uppercases, and offsets and the \
             sector index land on the model"
        );
    }

    #[test]
    fn an_empty_editable_lump_reads_as_no_elements() {
        let mut members = vec![
            marker("THINGS"),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            marker("VERTEXES"),
            marker("SECTORS"),
        ];
        members.push(lump("ZNODES", &[7_u8; 2]));
        let map = read_doom_map(name("E1M1"), &members).expect("empty records parse");
        assert_eq!(map.name, name("E1M1"), "the header name is kept");
        assert_eq!(map.format, MapFormat::Doom, "a Doom read gives Doom");
        assert!(map.things.is_empty(), "no things");
        assert!(map.linedefs.is_empty(), "no linedefs");
        assert_eq!(map.other.len(), 1, "the ZNODES lump is carried");
    }

    #[test]
    fn a_truncated_lump_is_a_format_error_naming_the_lump() {
        let truncated_things = vec![
            lump("THINGS", &[0_u8; 13]),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            marker("VERTEXES"),
            marker("SECTORS"),
        ];
        let CyberError::Format(things_message) = read_doom_map(name("MAP01"), &truncated_things)
            .expect_err("13 bytes are no whole thing")
        else {
            panic!("expected Format for a truncated record");
        };
        assert!(
            things_message.contains("THINGS"),
            "the lump is named: {things_message}"
        );
        assert!(
            things_message.contains("multiple"),
            "the reason is stated: {things_message}"
        );
        let truncated_vertices = vec![
            lump("THINGS", &[0_u8; 10]),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            lump("VERTEXES", &[1_u8; 5]),
            marker("SECTORS"),
        ];
        let CyberError::Format(vertex_message) = read_doom_map(name("MAP01"), &truncated_vertices)
            .expect_err("5 bytes are no vertex pair")
        else {
            panic!("expected Format for a truncated VERTEXES");
        };
        assert!(
            vertex_message.contains("VERTEXES") && vertex_message.contains("4-byte"),
            "the lump and its record size are named: {vertex_message}"
        );
        assert!(
            vertex_message.contains("Doom") || vertex_message.contains("record"),
            "the message stays actionable: {vertex_message}"
        );
    }

    #[test]
    fn a_missing_editable_lump_is_a_format_error_naming_it() {
        let members = vec![
            marker("THINGS"),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            marker("VERTEXES"),
        ];
        let CyberError::Format(message) =
            read_doom_map(name("MAP01"), &members).expect_err("no SECTORS lump")
        else {
            panic!("expected Format for a missing lump");
        };
        assert!(message.contains("SECTORS"), "the lump is named: {message}");
        assert!(
            message.contains("LINEDEFS"),
            "the need is explained: {message}"
        );
        assert!(message.contains("Doom"), "the format is named: {message}");
    }

    #[test]
    fn the_writer_rejects_i16_overflows_naming_element_and_field() {
        let CyberError::LimitExceeded(x_message) = write_things(&[Thing {
            x: 40000,
            ..Default::default()
        }])
        .expect_err("40000 is no i16") else {
            panic!("expected LimitExceeded for an oversized x");
        };
        assert_eq!(x_message, "thing 0 x 40000 does not fit i16", "exact shape");
        let low_ceiling = room(0, i32::from(i16::MIN).saturating_sub(1), 160, 0);
        let CyberError::LimitExceeded(height_message) =
            write_sectors(&[low_ceiling]).expect_err("one below i16::MIN is unwritable")
        else {
            panic!("expected LimitExceeded for an undersized height");
        };
        assert!(
            height_message.contains("sector 0 ceiling_height -32769"),
            "the element and field are named: {height_message}"
        );
    }

    #[test]
    fn the_writer_rejects_u16_overflows_naming_element_and_field() {
        let tag_message = linedef_error(Linedef {
            tag: 70000,
            ..Default::default()
        });
        assert_eq!(
            tag_message, "linedef 0 tag 70000 does not fit u16",
            "the ticket's example shape, element and field included"
        );
        let special_message = linedef_error(Linedef {
            special: -1,
            ..Default::default()
        });
        assert!(
            special_message.contains("linedef 0 special -1 does not fit u16"),
            "negatives are rejected too: {special_message}"
        );
    }

    #[test]
    fn the_writer_rejects_flag_and_type_overflows() {
        let CyberError::LimitExceeded(type_message) = write_things(&[Thing {
            kind: -5,
            ..Default::default()
        }])
        .expect_err("a negative type is unwritable") else {
            panic!("expected LimitExceeded for a negative type");
        };
        assert!(
            type_message.contains("thing 0 type -5 does not fit u16"),
            "the type field is named: {type_message}"
        );
        let CyberError::LimitExceeded(flags_message) = write_things(&[Thing {
            flags: 0x1_0000,
            ..Default::default()
        }])
        .expect_err("bit 16 is unwritable") else {
            panic!("expected LimitExceeded for oversized flags");
        };
        assert!(
            flags_message.contains("thing 0 flags 65536 does not fit u16"),
            "the flags field is named: {flags_message}"
        );
    }

    #[test]
    fn the_writer_rejects_reference_indices_at_and_above_the_sentinel() {
        for index in [0xFFFF_usize, 70000] {
            let message = linedef_error(Linedef {
                v1: index,
                ..Default::default()
            });
            assert!(
                message.contains(&format!("linedef 0 v1 {index}")),
                "the element and value are named: {message}"
            );
        }
        let reserved = linedef_error(Linedef {
            back: Some(0xFFFF),
            ..Default::default()
        });
        assert!(
            reserved.contains("linedef 0 back 65535"),
            "a reserved sentinel is named: {reserved}"
        );
        let sector_message = sidedef_error(&side("-", "-", "-", 70000));
        assert!(
            sector_message.contains("sidedef 0 sector 70000 does not fit u16"),
            "the sector field is named: {sector_message}"
        );
    }

    #[test]
    fn a_name_longer_than_eight_bytes_is_an_invalid_argument() {
        let CyberError::InvalidArgument(texture_message) =
            write_sidedefs(&[side("-", "-", "WALL_TOOLONG", 0)])
                .expect_err("twelve bytes are no name")
        else {
            panic!("expected InvalidArgument for a long name");
        };
        assert!(
            texture_message.contains("WALL_TOOLONG"),
            "the text is named: {texture_message}"
        );
        let CyberError::InvalidArgument(flat_message) = write_sectors(&[Sector {
            floor_flat: "TOOLONGFLAT".to_owned(),
            ..Default::default()
        }])
        .expect_err("eleven bytes are no flat name") else {
            panic!("expected InvalidArgument for a long flat");
        };
        assert!(
            flat_message.contains("TOOLONGFLAT"),
            "the flat is named: {flat_message}"
        );
        let wide = format!("CAF{}", "\u{e9}".repeat(8_usize));
        let CyberError::InvalidArgument(high_message) = write_sectors(&[Sector {
            ceiling_flat: wide,
            ..Default::default()
        }])
        .expect_err("eleven Latin-1 characters are eleven bytes") else {
            panic!("expected InvalidArgument for an over-wide high-byte name");
        };
        assert!(
            high_message.contains("is 11 bytes"),
            "the stored width, not the character count, is what overflows: \
             {high_message}"
        );
    }

    #[test]
    fn an_untextured_face_writes_as_dash_not_as_an_empty_field() {
        let stored = *b"-\0\0\0\0\0\0\0";
        let buffer = write_sidedefs(&[side("", "-", "-", 7)]).expect("empties are writable");
        assert_eq!(
            name_fields(&buffer, &[4_usize, 12, 20]),
            vec![stored, stored, stored],
            "the model's \"-\" and the empty string both store the one-character \
             name a vanilla engine reads as no texture"
        );
        let flats = write_sectors(&[Sector {
            floor_flat: "-".to_owned(),
            ceiling_flat: String::new(),
            ..Default::default()
        }])
        .expect("dashes and empties are writable");
        assert_eq!(
            name_fields(&flats, &[4_usize, 12]),
            vec![stored, stored],
            "the same rule holds for sector flats"
        );
        let read = read_sectors(&flats).expect("the record reads back");
        let sector = read.first().expect("one sector");
        assert_eq!(sector.floor_flat, "-", "a dash field reads as dash");
        assert_eq!(sector.ceiling_flat, "-", "the empty name normalises too");
    }

    #[test]
    fn an_all_nul_name_field_still_reads_as_dash() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&0_i16.to_le_bytes());
        buffer.extend_from_slice(&128_i16.to_le_bytes());
        buffer.extend_from_slice(&[0_u8; 8]);
        buffer.extend_from_slice(&[0_u8; 8]);
        buffer.extend_from_slice(&200_i16.to_le_bytes());
        buffer.extend_from_slice(&0_u16.to_le_bytes());
        buffer.extend_from_slice(&3_u16.to_le_bytes());
        let sectors = read_sectors(&buffer).expect("one complete record");
        let sector = sectors.first().expect("one sector");
        assert_eq!(sector.floor_flat, "-", "an all-NUL flat reads as dash");
        assert_eq!(
            sector.ceiling_flat, "-",
            "the other all-NUL flat reads as dash too"
        );
    }

    #[test]
    fn a_texture_name_with_a_high_byte_round_trips_byte_for_byte() {
        let mut record = Vec::new();
        record.extend_from_slice(&(-4_i16).to_le_bytes());
        record.extend_from_slice(&8_i16.to_le_bytes());
        record.extend_from_slice(b"FLOOR9\0\0");
        record.extend_from_slice(&[b'-', 0, 0, 0, 0, 0, 0, 0]);
        record.extend_from_slice(b"CA");
        record.push(0xE9_u8);
        record.extend_from_slice(b"LF\0\0\0");
        record.extend_from_slice(&3_u16.to_le_bytes());
        let sidedefs = read_sidedefs(&record).expect("one complete record");
        assert_eq!(
            sidedefs.first().map(|row| row.middle.as_str()),
            Some("CA\u{e9}LF"),
            "the byte above 0x7F reads as the Latin-1 character it stands for"
        );
        let written = write_sidedefs(&sidedefs).expect("the decoded name is writable");
        assert_eq!(
            written, record,
            "a name the model did not change keeps its eight raw bytes"
        );
    }

    #[test]
    fn a_name_beyond_latin1_writes_its_whole_utf8_encoding() {
        let buffer = write_sidedefs(&[side("-", "-", "A\u{20ac}B", 0)]).expect("five bytes fit");
        assert_eq!(
            name_fields(&buffer, &[20_usize]),
            vec![[b'A', 0xE2, 0x82, 0xAC, b'B', 0, 0, 0]],
            "a character above U+00FF contributes every UTF-8 byte of itself, so the \
             field still comes out eight bytes wide"
        );
    }
}
