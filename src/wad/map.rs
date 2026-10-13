//! The map layer: a typed, format-neutral map model and its binary
//! `Doom` codec.
//!
//! [`model`] defines the rows every map tool edits ([`Map`] and friends,
//! re-exported here); [`doom`] reads and writes the binary `Doom` lump
//! format. [`read_map`] and [`write_map`] dispatch on a block's
//! [`MapFormat`]: `Hexen` and `UDMF` share this model and gain their codecs
//! in follow-up tickets, so they currently report
//! [`CyberError::Unsupported`].

pub mod doom;
pub mod model;

pub use crate::wad::classify::MapFormat;
pub use model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::classify::MapBlock;

/// Reads the map whose block `block` describes out of the directory
/// `block_lumps`.
///
/// # Errors
///
/// [`CyberError::Format`] when the block's range leaves `block_lumps` or
/// the binary `Doom` decoder rejects the members (see
/// [`doom::read_doom_map`]); [`CyberError::Unsupported`] for `Hexen` and
/// `UDMF` blocks.
#[inline]
pub fn read_map(block_lumps: &[Lump], block: &MapBlock) -> Result<Map, CyberError> {
    match block.format {
        MapFormat::Doom => {
            let start = block.header.saturating_add(1);
            let members = block_lumps.get(start..block.end).ok_or_else(|| {
                CyberError::Format(format!(
                    "map block {} spans lumps {start}..{} of a {}-lump directory: \
                     pass the archive that holds the whole block",
                    block.name,
                    block.end,
                    block_lumps.len()
                ))
            })?;
            doom::read_doom_map(block.name, members)
        }
        MapFormat::Hexen => Err(not_implemented("reading", "Hexen")),
        MapFormat::Udmf => Err(not_implemented("reading", "UDMF")),
    }
}

/// Writes `map` back out as its block's lumps, header first.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] or [`CyberError::InvalidArgument`] from
/// the binary `Doom` encoder (see [`doom::write_doom_map`]);
/// [`CyberError::Unsupported`] for `Hexen` and `UDMF` maps.
#[inline]
pub fn write_map(map: &Map) -> Result<Vec<Lump>, CyberError> {
    match map.format {
        MapFormat::Doom => doom::write_doom_map(map),
        MapFormat::Hexen => Err(not_implemented("writing", "Hexen")),
        MapFormat::Udmf => Err(not_implemented("writing", "UDMF")),
    }
}

/// The `Unsupported` error saying `operation` of `format` maps is a
/// follow-up ticket.
fn not_implemented(operation: &str, format: &str) -> CyberError {
    CyberError::Unsupported(format!(
        "{operation} {format} format maps is not implemented yet: only binary \
         Doom blocks are decoded and encoded for now"
    ))
}

#[cfg(test)]
mod tests {
    use crate::error::CyberError;
    use crate::wad::archive::Lump;
    use crate::wad::classify::{MapBlock, MapFormat, analyze};
    use crate::wad::map::{Linedef, Map, read_map, write_map};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A `MAP01` block whose header sits at zero and whose members end at
    /// `end` in `format`.
    fn block(end: usize, format: MapFormat) -> MapBlock {
        MapBlock {
            name: name("MAP01"),
            header: 0,
            end,
            format,
            gl: None,
        }
    }

    /// A minimal valid `Doom` block: header plus the five empty members.
    fn doom_lumps() -> Vec<Lump> {
        let mut lumps = vec![Lump::marker(name("MAP01"))];
        for member in ["THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SECTORS"] {
            lumps.push(Lump::marker(name(member)));
        }
        lumps
    }

    #[test]
    fn the_dispatcher_reads_and_writes_doom_blocks() {
        let lumps = doom_lumps();
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("analyze found the block");
        let map = read_map(&lumps, block).expect("a Doom block reads");
        assert_eq!(map.name, name("MAP01"), "the header names the map");
        assert_eq!(map.format, MapFormat::Doom, "and its format");
        assert!(map.things.is_empty(), "the members are empty");
        let written = write_map(&map).expect("the read map writes back");
        assert_eq!(written.len(), 11, "header plus ten vanilla members");
        assert_eq!(written.first().map(|lump| lump.name), Some(name("MAP01")));
    }

    #[test]
    fn the_dispatcher_refuses_blocks_outside_the_directory() {
        let lumps = doom_lumps();
        let wild = block(lumps.len().checked_add(1).expect("small"), MapFormat::Doom);
        let CyberError::Format(message) =
            read_map(&lumps, &wild).expect_err("the range leaves the directory")
        else {
            panic!("expected Format for an out-of-range block");
        };
        assert!(
            message.contains("MAP01") && message.contains("spans lumps"),
            "the block is named: {message}"
        );
    }

    /// The `Unsupported` message from reading a `MAP01` block ending at
    /// `end` in `format`.
    fn read_unsupported(end: usize, format: MapFormat) -> String {
        let lumps = doom_lumps();
        match read_map(&lumps, &block(end, format)) {
            Err(CyberError::Unsupported(message)) => message,
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(map) => panic!("expected a refusal, read {} linedefs", map.linedefs.len()),
        }
    }

    /// The `Unsupported` message from writing a map in `format`.
    fn write_unsupported(format: MapFormat) -> String {
        let map = Map {
            format,
            ..Map::default()
        };
        match write_map(&map) {
            Err(CyberError::Unsupported(message)) => message,
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(lumps) => panic!("expected a refusal, wrote {} lumps", lumps.len()),
        }
    }

    #[test]
    fn the_dispatcher_reports_hexen_and_udmf_as_unsupported() {
        for format in [MapFormat::Hexen, MapFormat::Udmf] {
            let read_message = read_unsupported(doom_lumps().len(), format);
            assert!(
                read_message.contains("not implemented yet"),
                "the read refusal is explicit: {read_message}"
            );
            let write_message = write_unsupported(format);
            assert!(
                write_message.contains("not implemented yet"),
                "the write refusal is explicit: {write_message}"
            );
        }
    }

    #[test]
    fn doom_maps_route_through_the_model_defaults() {
        let lumps = doom_lumps();
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("one block");
        let map = read_map(&lumps, block).expect("the empty block reads");
        assert!(map.linedefs.is_empty(), "no linedefs in an empty block");
        let mut one = map;
        one.linedefs.push(Linedef::default());
        let written = write_map(&one).expect("one empty linedef writes");
        let things_and_lines = written.get(2).expect("the LINEDEFS lump");
        assert_eq!(things_and_lines.size(), 14, "one 14-byte record");
    }
}
