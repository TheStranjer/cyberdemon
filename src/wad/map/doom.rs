//! The binary `Doom` map format: reading and writing vanilla map blocks.
//!
//! A binary `Doom` map is ten member lumps between the map header and the
//! next non-member: the five editable records ([`Thing`], [`Linedef`],
//! [`Sidedef`], [`Vertex`] and [`Sector`] rows) plus the generated
//! `SEGS`/`SSECTORS`/`NODES` block, `REJECT` and `BLOCKMAP`, which this
//! module copies verbatim.
//! Every field is little-endian: `THINGS` records are 10 bytes,
//! `LINEDEFS` 14, `SIDEDEFS` 30, `VERTEXES` 4 and `SECTORS` 26, with the
//! `0xFFFF` sidedef field meaning "no side". The `SIDEDEFS`, `VERTEXES` and
//! `SECTORS` codecs and the range-check helpers are shared with the `Hexen`
//! format through the private `common` module. [`read_doom_map`] refuses a
//! block
//! whose editable lump is missing or whose size is not a whole number of
//! records; [`write_doom_map`] refuses values outside the stored type and
//! names the offending element and field. Texture and flat fields hold
//! `NUL`-padded names: the model's `"-"` writes as an all-`NUL` field and
//! an all-`NUL` field reads back as `"-"`.
//!
//! [`Sidedef`]: crate::wad::map::Sidedef
//! [`Sector`]: crate::wad::map::Sector
//! [`Vertex`]: crate::wad::map::Vertex

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::bytes::{Reader, Writer};
use crate::wad::classify::MapFormat;
use crate::wad::map::common::{
    Element, carried, is_editable, member_data, other_lumps, read_sectors, read_side,
    read_sidedefs, read_vertices, records, write_sectors, write_sidedefs, write_vertexes,
};
use crate::wad::map::model::{Linedef, Map, Thing};
use crate::wad::name::LumpName;

/// Bytes in one `THINGS` record.
const THING_LEN: usize = 10;
/// Bytes in one `LINEDEFS` record.
const LINEDEF_LEN: usize = 14;

/// Reads a whole binary `Doom` map.
///
/// `name` is the block header's name; `members` are the block's lumps after
/// the header, in directory order. The five editable lumps decode into the
/// model, every other member is carried into [`Map::other`] verbatim, and
/// `Doom` leaves the `Hexen`/`UDMF`-only fields (`tid`, `z`, thing
/// `special`/`args`, [`Sidedef::extra`] and friends) at their zero values.
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
    Ok(Map {
        name,
        format: MapFormat::Doom,
        udmf_namespace: None,
        things: read_things(member_data(members, "THINGS", "Doom")?)?,
        vertices: read_vertices(member_data(members, "VERTEXES", "Doom")?)?,
        linedefs: read_linedefs(member_data(members, "LINEDEFS", "Doom")?)?,
        sidedefs: read_sidedefs(member_data(members, "SIDEDEFS", "Doom")?)?,
        sectors: read_sectors(member_data(members, "SECTORS", "Doom")?)?,
        other: other_lumps(members),
    })
}

/// Writes a whole binary `Doom` map.
///
/// The result is the zero-size header (named after [`Map::name`]) followed
/// by `THINGS LINEDEFS SIDEDEFS VERTEXES SEGS SSECTORS NODES SECTORS
/// REJECT BLOCKMAP`, taking `SEGS`, `SSECTORS`, `NODES`, `REJECT` and
/// `BLOCKMAP` from [`Map::other`] when present and emitting an empty lump
/// otherwise; any remaining [`other`] lumps follow in order. The `Doom`
/// format stores no `tid`, `z`, thing `special`/`args`, so those model
/// fields are not written.
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
    let mut lumps = Vec::with_capacity(11_usize.saturating_add(map.other.len()));
    lumps.push(Lump::marker(map.name));
    lumps.push(Lump::new(
        LumpName::from_raw(*b"THINGS\0\0"),
        write_things(&map.things)?,
    ));
    lumps.push(Lump::new(
        LumpName::from_raw(*b"LINEDEFS"),
        write_linedefs(&map.linedefs)?,
    ));
    lumps.push(Lump::new(
        LumpName::from_raw(*b"SIDEDEFS"),
        write_sidedefs(&map.sidedefs)?,
    ));
    lumps.push(Lump::new(
        LumpName::from_raw(*b"VERTEXES"),
        write_vertexes(&map.vertices)?,
    ));
    for raw in [*b"SEGS\0\0\0\0", *b"SSECTORS", *b"NODES\0\0\0"] {
        lumps.push(carried(&map.other, LumpName::from_raw(raw)));
    }
    lumps.push(Lump::new(
        LumpName::from_raw(*b"SECTORS\0"),
        write_sectors(&map.sectors)?,
    ));
    for raw in [*b"REJECT\0\0", *b"BLOCKMAP"] {
        lumps.push(carried(&map.other, LumpName::from_raw(raw)));
    }
    lumps.extend(
        map.other
            .iter()
            .filter(|lump| !is_standard(lump.name.as_bytes()))
            .cloned(),
    );
    Ok(lumps)
}

/// Whether `name` is one of the ten lumps [`write_doom_map`] emits itself.
const fn is_standard(name: &[u8]) -> bool {
    is_editable(name)
        || matches!(
            name,
            b"SEGS" | b"SSECTORS" | b"NODES" | b"REJECT" | b"BLOCKMAP"
        )
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
    use crate::wad::map::common::fixtures::{lump, marker, name, room, side, two_room_map};
    use crate::wad::map::common::{NO_SIDEEDEF, read_sidedefs, write_sectors, write_sidedefs};
    use crate::wad::map::doom::{
        read_doom_map, read_linedefs, read_things, write_doom_map, write_linedefs, write_things,
    };
    use crate::wad::map::model::{Linedef, Sector, Sidedef, Thing};
    use crate::wad::name::LumpName;

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
        buffer.extend_from_slice(&NO_SIDEEDEF.to_le_bytes());
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
        for value in [0xFFFE_u16, 3, 13, 0xFFFF, 257, NO_SIDEEDEF, 5] {
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
    }

    #[test]
    fn the_dash_name_writes_as_the_empty_field_and_back() {
        let buffer = write_sectors(&[Sector {
            floor_flat: "-".to_owned(),
            ceiling_flat: String::new(),
            ..Default::default()
        }])
        .expect("dashes and empties are writable");
        assert!(
            buffer[4_usize..20].iter().all(|&byte| byte == 0_u8),
            "both flat fields store all NUL"
        );
        let map = read_doom_map(
            name("MAP01"),
            &[
                marker("THINGS"),
                marker("LINEDEFS"),
                marker("SIDEDEFS"),
                marker("VERTEXES"),
                lump("SECTORS", &buffer),
            ],
        )
        .expect("the record reads back");
        let sector = map.sectors.first().expect("one sector");
        assert_eq!(sector.floor_flat, "-", "an empty field reads as dash");
        assert_eq!(sector.ceiling_flat, "-", "the empty name normalises too");
    }
}
