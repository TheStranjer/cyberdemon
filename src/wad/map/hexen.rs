//! The binary `Hexen` map format: reading and writing `BEHAVIOR`-carrying
//! map blocks.
//!
//! A binary `Hexen` map is the vanilla block plus a `BEHAVIOR` lump (that
//! is what [`analyze`] reports as [`MapFormat::Hexen`]) and optionally
//! `SCRIPTS`. Every field is little-endian: `THINGS` records are 20 bytes
//! (`tid`, `x`, `y`, `z`, `angle` as `i16`, `type` and `flags` as `u16`,
//! `special` and five `args` as `u8`), `LINEDEFS` records are 16 bytes
//! (`v1`, `v2`, `flags` as `u16`, `special` and five `args` as `u8`, then
//! the right and left sidedef numbers with the `0xFFFF` "no side"
//! sentinel). `SIDEDEFS`, `VERTEXES` and `SECTORS` are byte-identical to
//! `Doom` and decode through the private `common` module.
//!
//! `Hexen` linedefs carry no tag: [`read_hexen_map`] leaves
//! [`Linedef::tag`] at zero, and [`write_hexen_map`] rejects a non-zero
//! one as [`CyberError::InvalidArgument`] because specials take their
//! parameters from `args`. Thing `tid`, `z`, `special` and `args` round
//! trip through the model fields `Doom` leaves empty.
//!
//! [`analyze`]: crate::wad::classify::analyze

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::bytes::{Reader, Writer};
use crate::wad::classify::MapFormat;
use crate::wad::lumps;
use crate::wad::map::common::{
    Element, carried, member_data, other_lumps, read_sectors, read_side, read_sidedefs,
    read_vertices, records, write_sectors, write_sidedefs, write_vertexes,
};
use crate::wad::map::model::{Linedef, Map, Thing};
use crate::wad::name::LumpName;

/// Bytes in one `Hexen` `THINGS` record.
const THING_LEN: usize = 20;
/// Bytes in one `Hexen` `LINEDEFS` record.
const LINEDEF_LEN: usize = 16;
/// Script arguments stored in one `Hexen` record.
const ARG_COUNT: usize = 5;
/// Byte offset of the script table in the minimal empty ACS object.
const EMPTY_ACS_SCRIPT_OFFSET: u32 = 8_u32;

/// Reads a whole binary `Hexen` map.
///
/// `name` is the block header's name; `members` are the block's lumps
/// after the header, in directory order. The five editable lumps decode
/// into the model with their `Hexen` fields (`tid`, `z`, thing and line
/// `special`/`args`) filled and [`Linedef::tag`] left at zero; every other
/// member, `BEHAVIOR` and `SCRIPTS` included, is carried into
/// [`Map::other`] verbatim.
///
/// # Errors
///
/// [`CyberError::Format`] when a required lump (THINGS, LINEDEFS,
/// SIDEDEFS, VERTEXES, SECTORS) is absent or when one holds a size that
/// is not a whole number of its records.
///
/// [`Map::other`]: crate::wad::map::Map::other
#[inline]
pub fn read_hexen_map(name: LumpName, members: &[Lump]) -> Result<Map, CyberError> {
    Ok(Map {
        name,
        format: MapFormat::Hexen,
        udmf_namespace: None,
        extra_globals: Vec::new(),
        extra_blocks: Vec::new(),
        things: read_things(member_data(members, lumps::THINGS, "Hexen")?)?,
        vertices: read_vertices(member_data(members, lumps::VERTEXES, "Hexen")?)?,
        linedefs: read_linedefs(member_data(members, lumps::LINEDEFS, "Hexen")?)?,
        sidedefs: read_sidedefs(member_data(members, lumps::SIDEDEFS, "Hexen")?)?,
        sectors: read_sectors(member_data(members, lumps::SECTORS, "Hexen")?)?,
        other: other_lumps(members),
    })
}

/// Writes a whole binary `Hexen` map.
///
/// The result is the zero-size header (named after [`Map::name`]) followed
/// by `THINGS LINEDEFS SIDEDEFS VERTEXES SEGS SSECTORS NODES SECTORS
/// REJECT BLOCKMAP BEHAVIOR`, then `SCRIPTS` when [`other`] carries it,
/// then any remaining `other` lumps in order. `SEGS`, `SSECTORS`, `NODES`,
/// `REJECT` and `BLOCKMAP` are taken from `other` when present and emit as
/// empty lumps otherwise. A `Hexen` map must carry a `BEHAVIOR`: when
/// `other` holds none the write emits a minimal empty `ACS` object
/// instead, the 16 bytes `ACS\0`, a `u32` script offset of 8, a `u32`
/// script count of 0 and a `u32` string count of 0.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a value does not fit its stored
/// field type or an index reaches the reserved `0xFFFF`;
/// [`CyberError::InvalidArgument`] when a linedef carries a non-zero
/// `tag` or a texture or flat name is longer than eight bytes.
///
/// [`Map::name`]: crate::wad::map::Map::name
/// [`other`]: Map::other
#[inline]
pub fn write_hexen_map(map: &Map) -> Result<Vec<Lump>, CyberError> {
    let mut lumps = Vec::with_capacity(12_usize.saturating_add(map.other.len()));
    lumps.push(Lump::marker(map.name));
    lumps.push(Lump::new(lumps::THINGS, write_things(&map.things)?));
    lumps.push(Lump::new(lumps::LINEDEFS, write_linedefs(&map.linedefs)?));
    lumps.push(Lump::new(lumps::SIDEDEFS, write_sidedefs(&map.sidedefs)?));
    lumps.push(Lump::new(lumps::VERTEXES, write_vertexes(&map.vertices)?));
    for node in lumps::CLASSIC_NODE_RUN_LUMPS {
        lumps.push(carried(&map.other, node));
    }
    lumps.push(Lump::new(lumps::SECTORS, write_sectors(&map.sectors)?));
    for trailing in [lumps::REJECT, lumps::BLOCKMAP] {
        lumps.push(carried(&map.other, trailing));
    }
    lumps.push(behavior(&map.other));
    lumps.extend(scripts(&map.other));
    lumps.extend(
        map.other
            .iter()
            .filter(|lump| !is_standard(lump.name))
            .cloned(),
    );
    Ok(lumps)
}

/// Whether `name` is one of the twelve lumps [`write_hexen_map`] emits
/// itself, [`lumps::STANDARD_HEXEN_MAP_LUMPS`], `BEHAVIOR` and `SCRIPTS`
/// included.
fn is_standard(name: LumpName) -> bool {
    lumps::STANDARD_HEXEN_MAP_LUMPS.contains(&name)
}

/// The `BEHAVIOR` lump to write: the one `other` carries, or a minimal
/// empty `ACS` object when it carries none.
fn behavior(other: &[Lump]) -> Lump {
    other
        .iter()
        .find(|lump| lump.name == lumps::BEHAVIOR)
        .cloned()
        .unwrap_or_else(|| Lump::new(lumps::BEHAVIOR, empty_behavior()))
}

/// The `SCRIPTS` lump when `other` carries one.
fn scripts(other: &[Lump]) -> Option<Lump> {
    other
        .iter()
        .find(|lump| lump.name == lumps::SCRIPTS)
        .cloned()
}

/// The payload of a minimal empty `ACS` object: the `ACS\0` magic, a
/// `u32` script table offset of 8 (right behind the written header), a
/// `u32` script count of 0 and a `u32` string count of 0.
fn empty_behavior() -> Vec<u8> {
    let mut writer = Writer::new();
    writer.put_bytes(b"ACS\0");
    writer.put_u32(EMPTY_ACS_SCRIPT_OFFSET);
    writer.put_u32(0_u32);
    writer.put_u32(0_u32);
    writer.into_inner()
}

/// Reads the five `args` bytes of one record.
fn read_args(reader: &mut Reader<'_>) -> Result<[i32; ARG_COUNT], CyberError> {
    let mut args = [0_i32; ARG_COUNT];
    for slot in &mut args {
        *slot = i32::from(reader.read_u8()?);
    }
    Ok(args)
}

/// Reads the `THINGS` records: tid, x, y, z, angle, the u16 type/flags and
/// the u8 special and args.
fn read_things(data: &[u8]) -> Result<Vec<Thing>, CyberError> {
    let mut things = Vec::new();
    for record in records("THINGS", data, THING_LEN)? {
        let mut reader = Reader::new(record);
        let tid = reader.read_i16()?;
        let x = reader.read_i16()?;
        let y = reader.read_i16()?;
        let z = reader.read_i16()?;
        let angle = reader.read_i16()?;
        let kind = reader.read_u16()?;
        let flags = reader.read_u16()?;
        let special = reader.read_u8()?;
        things.push(Thing {
            tid: i32::from(tid),
            x: i32::from(x),
            y: i32::from(y),
            z: i32::from(z),
            angle: i32::from(angle),
            kind: i32::from(kind),
            flags: u32::from(flags),
            special: i32::from(special),
            args: read_args(&mut reader)?,
            ..Default::default()
        });
    }
    Ok(things)
}

/// Reads the `LINEDEFS` records: vertex indices, flags, the u8 special and
/// args and the two sidedef numbers; `tag` has no stored field and reads
/// as zero.
fn read_linedefs(data: &[u8]) -> Result<Vec<Linedef>, CyberError> {
    let mut linedefs = Vec::new();
    for record in records("LINEDEFS", data, LINEDEF_LEN)? {
        let mut reader = Reader::new(record);
        let v1 = reader.read_u16()?;
        let v2 = reader.read_u16()?;
        let flags = reader.read_u16()?;
        let special = reader.read_u8()?;
        let args = read_args(&mut reader)?;
        let right = reader.read_u16()?;
        let left = reader.read_u16()?;
        linedefs.push(Linedef {
            v1: usize::from(v1),
            v2: usize::from(v2),
            flags: u32::from(flags),
            special: i32::from(special),
            tag: 0,
            args,
            front: read_side(right),
            back: read_side(left),
            ..Default::default()
        });
    }
    Ok(linedefs)
}

/// Writes the `THINGS` records: tid, x, y, z and angle as `i16`, type and
/// flags as `u16`, special and args as `u8`.
fn write_things(things: &[Thing]) -> Result<Vec<u8>, CyberError> {
    let mut writer = Writer::new();
    for (index, spawn) in things.iter().enumerate() {
        let at = Element {
            kind: "thing",
            index,
        };
        writer.put_i16(at.i16("tid", spawn.tid)?);
        writer.put_i16(at.i16("x", spawn.x)?);
        writer.put_i16(at.i16("y", spawn.y)?);
        writer.put_i16(at.i16("z", spawn.z)?);
        writer.put_i16(at.i16("angle", spawn.angle)?);
        writer.put_u16(at.u16("type", spawn.kind)?);
        writer.put_u16(at.u16_bits("flags", spawn.flags)?);
        writer.put_u8(at.u8("special", spawn.special)?);
        at.put_args(&mut writer, &spawn.args)?;
    }
    Ok(writer.into_inner())
}

/// Writes the `LINEDEFS` records, front side as the right sidedef; a
/// non-zero `tag` is a model-only field and an error here.
fn write_linedefs(linedefs: &[Linedef]) -> Result<Vec<u8>, CyberError> {
    let mut writer = Writer::new();
    for (index, linedef) in linedefs.iter().enumerate() {
        let at = Element {
            kind: "linedef",
            index,
        };
        if linedef.tag != 0_i32 {
            return Err(CyberError::InvalidArgument(format!(
                "linedef {index} has tag {}, but Hexen linedefs have no tag; \
                 use args to carry script parameters",
                linedef.tag
            )));
        }
        writer.put_u16(at.index("v1", linedef.v1)?);
        writer.put_u16(at.index("v2", linedef.v2)?);
        writer.put_u16(at.u16_bits("flags", linedef.flags)?);
        writer.put_u8(at.u8("special", linedef.special)?);
        at.put_args(&mut writer, &linedef.args)?;
        writer.put_u16(at.side("front", linedef.front)?);
        writer.put_u16(at.side("back", linedef.back)?);
    }
    Ok(writer.into_inner())
}

#[cfg(test)]
mod tests {
    use crate::error::CyberError;
    use crate::wad::archive::Lump;
    use crate::wad::classify::{MapFormat, analyze};
    use crate::wad::map::common::fixtures::{lump, marker, name, two_room_map};
    use crate::wad::map::hexen::{
        read_hexen_map, read_linedefs, read_things, write_hexen_map, write_linedefs, write_things,
    };
    use crate::wad::map::model::{Linedef, Map, Thing};
    use crate::wad::map::{read_map, write_map};
    use crate::wad::name::LumpName;

    /// The `ACS\0` script offset stored by [`empty_behavior`], in bytes.
    const EMPTY_ACS: [u8; 16] = [b'A', b'C', b'S', 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    /// The two-room map as `Hexen`: `tid`s, `z`, thing and line specials
    /// and args set, and a `BEHAVIOR` lump carried in [`Map::other`].
    fn hexen_two_room_map() -> Map {
        let mut map = two_room_map();
        map.format = MapFormat::Hexen;
        if let Some(first) = map.things.first_mut() {
            first.tid = 5_i32;
            first.z = 128_i32;
            first.special = 3_i32;
            first.args = [1_i32, 2_i32, 3_i32, 4_i32, 5_i32];
        }
        if let Some(second) = map.things.get_mut(1) {
            second.tid = -2_i32;
            second.z = -16_i32;
            second.args = [8_i32, 0_i32, 0_i32, 0_i32, 0_i32];
        }
        if let Some(portal) = map.linedefs.get_mut(2) {
            portal.special = 125_i32;
            portal.args = [3_i32, 0_i32, 0_i32, 2_i32, 7_i32];
        }
        map.other
            .push(lump("BEHAVIOR", b"ACS\0some compiled script"));
        map
    }

    /// The `LimitExceeded` message the given things produce at write time.
    fn thing_error(thing: Thing) -> String {
        match write_things(&[thing]) {
            Err(CyberError::LimitExceeded(message)) => message,
            Err(other) => panic!("expected LimitExceeded, got {other:?}"),
            Ok(bytes) => panic!("expected a rejection, wrote {} bytes", bytes.len()),
        }
    }

    /// The `LimitExceeded` message the given linedefs produce at write time.
    fn linedef_error(linedef: Linedef) -> String {
        match write_linedefs(&[linedef]) {
            Err(CyberError::LimitExceeded(message)) => message,
            Err(other) => panic!("expected LimitExceeded, got {other:?}"),
            Ok(bytes) => panic!("expected a rejection, wrote {} bytes", bytes.len()),
        }
    }

    #[test]
    fn the_hexen_two_room_map_round_trips_through_the_binary_format() {
        let map = hexen_two_room_map();
        let lumps = write_hexen_map(&map).expect("the test map is writable");
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
                name("BEHAVIOR"),
            ],
            "header, the ten vanilla members and BEHAVIOR, in order"
        );
        assert_eq!(lumps.get(1).map(Lump::size), Some(40), "two 20-byte things");
        assert_eq!(
            lumps.get(2).map(Lump::size),
            Some(112),
            "seven 16-byte linedefs"
        );
        let read = read_hexen_map(map.name, lumps.get(1..).expect("members"))
            .expect("the written block reads back");
        assert_eq!(read, map, "read(write(map)) == map");
    }

    #[test]
    fn scripts_and_extras_follow_the_block_in_order() {
        let mut map = hexen_two_room_map();
        map.other.push(lump("SCRIPTS", b"ANIMATED things"));
        map.other.push(lump("ZNODES", &[9_u8; 4]));
        let lumps = write_hexen_map(&map).expect("the test map is writable");
        let extras: Vec<String> = lumps
            .iter()
            .skip(12)
            .map(|lump| lump.name.to_string())
            .collect();
        assert_eq!(extras, vec!["SCRIPTS", "ZNODES"], "SCRIPTS then extras");
        let read = read_hexen_map(map.name, lumps.get(1..).expect("members"))
            .expect("the extended block reads back");
        assert_eq!(read, map, "SCRIPTS and ZNODES survive the round trip");
    }

    #[test]
    fn a_missing_behavior_writes_a_minimal_empty_acs_object() {
        let mut map = hexen_two_room_map();
        map.other.retain(|lump| lump.name.as_bytes() != b"BEHAVIOR");
        let lumps = write_hexen_map(&map).expect("a BEHAVIOR-less map is writable");
        let behavior = lumps.get(11).expect("BEHAVIOR sits after BLOCKMAP");
        assert_eq!(behavior.name, name("BEHAVIOR"), "the lump is named");
        assert_eq!(
            behavior.data.as_ref(),
            EMPTY_ACS.as_slice(),
            "the exact 16 bytes"
        );
        let read = read_hexen_map(map.name, lumps.get(1..).expect("members"))
            .expect("the synthesized block reads back");
        let carried = read
            .other
            .iter()
            .find(|lump| lump.name.as_bytes() == b"BEHAVIOR");
        assert!(
            carried.is_some_and(|lump| lump.data.as_ref() == EMPTY_ACS.as_slice()),
            "the synthesized lump is a plain carry on read"
        );
    }

    #[test]
    fn things_decode_the_twenty_byte_record() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&(-3_i16).to_le_bytes());
        buffer.extend_from_slice(&(-24_i16).to_le_bytes());
        buffer.extend_from_slice(&300_i16.to_le_bytes());
        buffer.extend_from_slice(&128_i16.to_le_bytes());
        buffer.extend_from_slice(&(-90_i16).to_le_bytes());
        buffer.extend_from_slice(&60000_u16.to_le_bytes());
        buffer.extend_from_slice(&0xFFFF_u16.to_le_bytes());
        buffer.extend_from_slice(&[200_u8, 1, 2, 3, 4, 5]);
        let things = read_things(&buffer).expect("one complete record");
        assert_eq!(
            things,
            vec![Thing {
                tid: -3,
                x: -24,
                y: 300,
                z: 128,
                angle: -90,
                kind: 60000,
                flags: 0xFFFF,
                special: 200,
                args: [1_i32, 2_i32, 3_i32, 4_i32, 5_i32],
                ..Default::default()
            }],
            "tid, x, y, z, angle, type, flags, special and five args in order"
        );
    }

    #[test]
    fn linedefs_decode_the_sixteen_byte_record_and_zero_the_tag() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&0xFFFE_u16.to_le_bytes());
        buffer.extend_from_slice(&3_u16.to_le_bytes());
        buffer.extend_from_slice(&13_u16.to_le_bytes());
        buffer.extend_from_slice(&[160_u8, 9, 8, 7, 6, 5]);
        buffer.extend_from_slice(&0xFFFF_u16.to_le_bytes());
        buffer.extend_from_slice(&5_u16.to_le_bytes());
        let linedefs = read_linedefs(&buffer).expect("one complete record");
        assert_eq!(
            linedefs,
            vec![Linedef {
                v1: 0xFFFE,
                v2: 3,
                flags: 13,
                special: 160,
                tag: 0,
                args: [9_i32, 8_i32, 7_i32, 6_i32, 5_i32],
                front: None,
                back: Some(5),
                ..Default::default()
            }],
            "the 0xFFFF sidedef is None, tag stays zero, and the u8 special \
             and args land on the model"
        );
    }

    #[test]
    fn wrong_record_sizes_are_format_errors_naming_the_lump() {
        let CyberError::Format(things_message) =
            read_things(&[0_u8; 19]).expect_err("19 bytes are no Hexen thing")
        else {
            panic!("expected Format for a short THINGS record");
        };
        assert!(
            things_message.contains("THINGS") && things_message.contains("20-byte"),
            "the lump and its record size are named: {things_message}"
        );
        let CyberError::Format(line_message) =
            read_linedefs(&[0_u8; 14]).expect_err("14 bytes are no Hexen linedef")
        else {
            panic!("expected Format for a short LINEDEFS record");
        };
        assert!(
            line_message.contains("LINEDEFS") && line_message.contains("16-byte"),
            "the lump and its record size are named: {line_message}"
        );
    }

    #[test]
    fn a_missing_editable_lump_is_a_format_error_naming_the_format() {
        let members = vec![
            marker("THINGS"),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            marker("VERTEXES"),
        ];
        let CyberError::Format(message) =
            read_hexen_map(name("MAP01"), &members).expect_err("no SECTORS lump")
        else {
            panic!("expected Format for a missing lump");
        };
        assert!(message.contains("SECTORS"), "the lump is named: {message}");
        assert!(message.contains("Hexen"), "the format is named: {message}");
    }

    #[test]
    fn the_writer_rejects_u8_overflows_naming_element_and_field() {
        let special_message = thing_error(Thing {
            special: 300,
            ..Default::default()
        });
        assert_eq!(
            special_message, "thing 0 special 300 does not fit u8",
            "the ticket's example shape"
        );
        let mut arg_thing = Thing::default();
        if let Some(slot) = arg_thing.args.get_mut(2) {
            *slot = 256_i32;
        }
        let arg_message = thing_error(arg_thing);
        assert!(
            arg_message.contains("thing 0 args[2] 256 does not fit u8"),
            "the argument position is named: {arg_message}"
        );
        let line_special = linedef_error(Linedef {
            special: 256,
            ..Default::default()
        });
        assert!(
            line_special.contains("linedef 0 special 256 does not fit u8"),
            "linedef specials are checked too: {line_special}"
        );
    }

    #[test]
    fn the_writer_rejects_i16_overflows_on_the_hexen_thing_fields() {
        let tid_message = thing_error(Thing {
            tid: 40000,
            ..Default::default()
        });
        assert_eq!(
            tid_message, "thing 0 tid 40000 does not fit i16",
            "the ticket's example shape"
        );
        let z_message = thing_error(Thing {
            z: i32::from(i16::MIN).saturating_sub(1),
            ..Default::default()
        });
        assert!(
            z_message.contains("thing 0 z -32769 does not fit i16"),
            "z is checked too: {z_message}"
        );
    }

    #[test]
    fn a_tagged_linedef_on_a_hexen_map_is_an_invalid_argument() {
        let CyberError::InvalidArgument(message) = write_linedefs(&[Linedef {
            tag: 4,
            ..Default::default()
        }])
        .expect_err("Hexen has no linedef tag") else {
            panic!("expected InvalidArgument for a non-zero tag");
        };
        assert!(
            message.contains("Hexen linedefs have no tag; use args"),
            "the fix is named: {message}"
        );
        assert!(
            message.contains("linedef 0"),
            "the element is named: {message}"
        );
        let CyberError::InvalidArgument(map_message) = write_hexen_map(&Map {
            linedefs: vec![Linedef {
                tag: 1,
                ..Default::default()
            }],
            format: MapFormat::Hexen,
            ..Map::default()
        })
        .expect_err("the whole-map write refuses tags too") else {
            panic!("expected InvalidArgument through write_hexen_map");
        };
        assert!(
            map_message.contains("tag 1"),
            "the offending value is named: {map_message}"
        );
    }

    #[test]
    fn the_dispatcher_reads_and_writes_hexen_blocks() {
        let map = hexen_two_room_map();
        let lumps = write_map(&map).expect("write_map routes Hexen");
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("analyze finds the block");
        assert_eq!(block.format, MapFormat::Hexen, "BEHAVIOR means Hexen");
        let read = read_map(&lumps, block).expect("read_map routes Hexen");
        assert_eq!(read, map, "the block round trips through the dispatcher");
    }

    #[test]
    fn the_dispatcher_reads_a_bare_marker_hexen_block() {
        let lumps = vec![
            marker("MAP01"),
            marker("THINGS"),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            marker("VERTEXES"),
            marker("SECTORS"),
            marker("BEHAVIOR"),
        ];
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("analyze finds the block");
        assert_eq!(block.format, MapFormat::Hexen, "BEHAVIOR means Hexen");
        let read = read_map(&lumps, block).expect("the block's members read");
        assert_eq!(read.format, MapFormat::Hexen, "the model carries it");
        assert!(read.things.is_empty(), "no records in the markers");
        assert_eq!(
            read.other.len(),
            1,
            "only BEHAVIOR is a non-editable member"
        );
    }
}
