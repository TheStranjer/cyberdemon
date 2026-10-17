//! The `BLOCKMAP` lump: the map's spatial index of linedefs.
//!
//! A blockmap divides the plane into 128×128 map-unit blocks and stores,
//! per block, the linedef indices that touch it. The lump starts with an
//! 8-byte header (`X` origin, `Y origin` as `i16`s, then `columns` and
//! `rows` as `u16`s), followed by `columns × rows` `u16` offsets counted
//! in 16-bit words from the lump start, each pointing at a blocklist: a
//! leading `0x0000`, the block's linedef indices and a `0xFFFF`
//! terminator. Block `(col, row)` covers `x` from `origin_x + 128·col`
//! up to the next block's origin, `y` likewise, and its offset sits at
//! table position `row × columns + col`. [`Blockmap::parse`] validates
//! that every offset stays inside the lump and every list terminates;
//! anything else is a `Format` error. Writing goes the other way:
//! [`Blockmap::to_bytes`] serializes a grid back into lump bytes,
//! optionally sharing one copy of identical blocklists the way the
//! builders do. Nothing here knows about sessions or MCP.

use alloc::collections::BTreeMap;

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::bytes::{Reader, Writer, u16_at};

/// Bytes in the blockmap header: two origins and the grid size.
const HEADER_LEN: usize = 8;
/// The word every blocklist starts with.
const LIST_START: u16 = 0x0000_u16;
/// The word every blocklist ends with.
const LIST_END: u16 = 0xFFFF_u16;
/// Bytes in one offset-table entry.
const OFFSET_ENTRY_LEN: usize = 2;
/// Bytes in one blocklist word.
const WORD_LEN: usize = 2;
/// The empty list: start and end adjacent.
const MIN_LIST_LEN: usize = 4;
/// Words in the blockmap header.
const HEADER_WORDS: usize = 4;
/// Words in a blocklist beyond its linedef entries (`0x0000` and `0xFFFF`).
const LIST_OVERHEAD: usize = 2;
/// The largest `BLOCKMAP` lump its `u16` word offsets can address.
const MAX_LUMP_WORDS: usize = 0xFFFF_usize;
/// Side of one blockmap block in map units.
pub const BLOCK_SIZE: i32 = 128;

/// The parsed `BLOCKMAP`: header and one linedef list per block, in
/// row-major order (`row × columns + col`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Blockmap {
    /// `X` of the bottom-left corner of block `(0, 0)`.
    pub origin_x: i16,
    /// `Y` of the bottom-left corner of block `(0, 0)`.
    pub origin_y: i16,
    /// Blocks per row.
    pub columns: u16,
    /// Rows in the grid.
    pub rows: u16,
    /// The linedef indices of every block, row-major.
    pub blocks: Vec<Vec<u16>>,
}

impl Blockmap {
    /// The world-space `X` of block `col`'s left edge.
    #[must_use]
    #[inline]
    pub fn block_x(&self, col: usize) -> Option<i32> {
        let col_units = i32::try_from(col).ok()?;
        let width = col_units.checked_mul(BLOCK_SIZE)?;
        i32::from(self.origin_x).checked_add(width)
    }

    /// The world-space `Y` of block `row`'s bottom edge.
    #[must_use]
    #[inline]
    pub fn block_y(&self, row: usize) -> Option<i32> {
        let row_units = i32::try_from(row).ok()?;
        let height = row_units.checked_mul(BLOCK_SIZE)?;
        i32::from(self.origin_y).checked_add(height)
    }

    /// Reads a whole `BLOCKMAP` lump.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when the lump is shorter than the header or
    /// its offset table, an offset leaves the lump, a blocklist does not
    /// start with `0x0000` or a list is not terminated by `0xFFFF`.
    #[inline]
    pub fn parse(data: &[u8]) -> Result<Self, CyberError> {
        let mut reader = Reader::new(data);
        if reader.remaining() < HEADER_LEN {
            return Err(CyberError::Format(format!(
                "BLOCKMAP lump holds {} bytes, too short for the {HEADER_LEN}-byte \
                 header; rebuild the blockmap",
                reader.remaining()
            )));
        }
        let origin_x = reader.read_i16()?;
        let origin_y = reader.read_i16()?;
        let columns = reader.read_u16()?;
        let rows = reader.read_u16()?;
        let cells = usize::from(columns)
            .checked_mul(usize::from(rows))
            .ok_or_else(|| {
                CyberError::Format(format!(
                    "BLOCKMAP grid {columns}x{rows} overflows: rebuild the blockmap"
                ))
            })?;
        let table_len = HEADER_LEN.saturating_add(cells.saturating_mul(OFFSET_ENTRY_LEN));
        if data.len() < table_len {
            return Err(CyberError::Format(format!(
                "BLOCKMAP lump holds {} bytes, too short for the offset table of \
                 {cells} blocks ({table_len} bytes); rebuild the blockmap",
                data.len()
            )));
        }
        let mut blocks = Vec::with_capacity(cells);
        let mut table = Reader::at(data, HEADER_LEN);
        for cell in 0..cells {
            let word = table.read_u16()?;
            blocks.push(read_list(data, cell, word)?);
        }
        Ok(Self {
            origin_x,
            origin_y,
            columns,
            rows,
            blocks,
        })
    }

    /// Serializes this grid into `BLOCKMAP` lump bytes in exactly the
    /// shape [`Blockmap::parse`] reads. With `compress`, identical
    /// blocklists share one copy; without it every block writes its own.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] ("blockmap too large") when the
    /// offset table or the finished lump leaves the 65535 words a `u16`
    /// offset can address; [`CyberError::InvalidArgument`] when the list
    /// count does not match the grid or a list holds the `0xFFFF`
    /// terminator word.
    #[inline]
    pub fn to_bytes(&self, compress: bool) -> Result<Vec<u8>, CyberError> {
        let table_end = self.check_shape()?;
        let mut plan = Plan::new(table_end, compress);
        let mut offsets: Vec<u16> = Vec::with_capacity(self.blocks.len());
        for list in &self.blocks {
            offsets.push(plan.place(list)?);
        }
        plan.finish()?;
        Ok(write_lump(self, &offsets, &plan.placed))
    }

    /// Checks that the list count matches the grid and returns the first
    /// free word after the header and offset table.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] on a count mismatch;
    /// [`CyberError::LimitExceeded`] when the offset table alone leaves
    /// the lump.
    fn check_shape(&self) -> Result<usize, CyberError> {
        let cells = usize::from(self.columns)
            .checked_mul(usize::from(self.rows))
            .ok_or_else(too_large)?;
        if self.blocks.len() != cells {
            return Err(CyberError::InvalidArgument(format!(
                "the BLOCKMAP holds {} lists for a {}x{} grid ({cells} blocks): rebuild the \
                 blockmap",
                self.blocks.len(),
                self.columns,
                self.rows
            )));
        }
        HEADER_WORDS
            .checked_add(cells)
            .filter(|end| *end <= MAX_LUMP_WORDS)
            .ok_or_else(too_large)
    }
}

/// Offset assignment in progress: the next free word, the shared-copy
/// table (in `compress` mode) and the bodies in ascending-offset order.
struct Plan<'list> {
    /// The next free word.
    next: usize,
    /// First-use offsets by list content, `compress` mode only.
    shared: BTreeMap<&'list [u16], u16>,
    /// The bodies to write, in placement order.
    placed: Vec<&'list [u16]>,
    /// Whether identical lists share one copy.
    compress: bool,
}

impl<'list> Plan<'list> {
    /// A plan whose first blocklist starts at word `next`.
    #[must_use]
    const fn new(next: usize, compress: bool) -> Self {
        Self {
            next,
            shared: BTreeMap::new(),
            placed: Vec::new(),
            compress,
        }
    }

    /// The offset word `list` lands on, placing its body (a first-use
    /// copy in `compress` mode) at the current end of the lump.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when the list holds the `0xFFFF`
    /// terminator word, [`CyberError::LimitExceeded`] when the growing
    /// lump leaves the addressable size.
    fn place(&mut self, list: &'list [u16]) -> Result<u16, CyberError> {
        if list.contains(&LIST_END) {
            return Err(CyberError::InvalidArgument(format!(
                "a BLOCKMAP blocklist holds {LIST_END:#06X}, the blocklist terminator: \
                 rebuild the blockmap"
            )));
        }
        let reuse = self
            .compress
            .then_some(self.shared.get(list).copied())
            .flatten();
        if let Some(word) = reuse {
            return Ok(word);
        }
        let word = u16::try_from(self.next).map_err(|_error| too_large())?;
        if self.compress {
            self.shared.insert(list, word);
        }
        self.placed.push(list);
        self.next = list_words(self.next, list)?;
        Ok(word)
    }

    /// Checks the finished lump still fits the format.
    ///
    /// # Errors
    ///
    /// [`CyberError::LimitExceeded`] when the last list ran past word
    /// [`MAX_LUMP_WORDS`].
    fn finish(&self) -> Result<(), CyberError> {
        if self.next > MAX_LUMP_WORDS {
            return Err(too_large());
        }
        Ok(())
    }
}

/// Writes the lump: header, offset table, then every placed blocklist
/// once, in ascending-offset (first-use) order.
fn write_lump(map: &Blockmap, offsets: &[u16], placed: &[&[u16]]) -> Vec<u8> {
    let mut w = Writer::new();
    w.put_i16(map.origin_x);
    w.put_i16(map.origin_y);
    w.put_u16(map.columns);
    w.put_u16(map.rows);
    for offset in offsets {
        w.put_u16(*offset);
    }
    for list in placed {
        w.put_u16(LIST_START);
        for entry in *list {
            w.put_u16(*entry);
        }
        w.put_u16(LIST_END);
    }
    w.into_inner()
}

/// The next free word after a blocklist of `list` entries placed at
/// `after`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the words leave the addressable
/// lump.
fn list_words(after: usize, list: &[u16]) -> Result<usize, CyberError> {
    after
        .checked_add(list.len())
        .and_then(|used| used.checked_add(LIST_OVERHEAD))
        .ok_or_else(too_large)
}

/// The refusal of a grid or lump that leaves the words a `BLOCKMAP`
/// can hold.
fn too_large() -> CyberError {
    CyberError::LimitExceeded(
        "blockmap too large: the grid or its blocklists leave the 65535 words a \
         BLOCKMAP lump can hold"
            .to_owned(),
    )
}

/// The blocklist `offset_words` points at, validated in bounds and
/// terminated; `cell` names the block in errors.
fn read_list(data: &[u8], cell: usize, offset_words: u16) -> Result<Vec<u16>, CyberError> {
    let start = list_start(data, cell, offset_words)?;
    let head = u16_at(data, start)?;
    if head != LIST_START {
        return Err(CyberError::Format(format!(
            "BLOCKMAP block {cell} list does not start with 0x0000 (found {head:#06X}); \
             rebuild the blockmap"
        )));
    }
    let mut pos = start.saturating_add(WORD_LEN);
    let mut lines = Vec::new();
    loop {
        let Ok(word) = u16_at(data, pos) else {
            return Err(unterminated(cell, offset_words));
        };
        pos = pos.saturating_add(WORD_LEN);
        if word == LIST_END {
            return Ok(lines);
        }
        lines.push(word);
    }
}

/// The byte offset of cell `cell`'s blocklist, checked to hold at least
/// an empty list inside `data`.
fn list_start(data: &[u8], cell: usize, offset_words: u16) -> Result<usize, CyberError> {
    let outside = || {
        CyberError::Format(format!(
            "BLOCKMAP block {cell} points at word {offset_words}, outside the \
             {}-byte lump; rebuild the blockmap",
            data.len()
        ))
    };
    usize::from(offset_words)
        .checked_mul(WORD_LEN)
        .filter(|start| {
            data.get(*start..start.saturating_add(MIN_LIST_LEN))
                .is_some()
        })
        .ok_or_else(outside)
}

/// The error for a list that runs off the lump without its terminator.
fn unterminated(cell: usize, offset_words: u16) -> CyberError {
    CyberError::Format(format!(
        "BLOCKMAP block {cell} list at word {offset_words} is not terminated by \
         0xFFFF; rebuild the blockmap"
    ))
}

#[cfg(test)]
mod tests {
    use super::{Blockmap, MAX_LUMP_WORDS};
    use crate::wad::bytes::Writer;

    /// The three blocklists of [`build`], in placement order.
    type Lists = (Vec<u8>, Vec<u8>, Vec<u8>);

    /// A `2×2` blockmap: header, four offsets and three lists. Blocks
    /// `(0,0)` and `(1,1)` share one list (linedefs 5 and 9), `(1,0)` is
    /// empty, `(0,1)` holds only linedef 2. Offsets are filled in by
    /// `build`.
    fn build() -> Vec<u8> {
        let lists: Lists = {
            let mut shared = Writer::new();
            shared.put_u16(0);
            shared.put_u16(5);
            shared.put_u16(9);
            shared.put_u16(0xFFFF);
            let mut empty = Writer::new();
            empty.put_u16(0);
            empty.put_u16(0xFFFF);
            let mut single = Writer::new();
            single.put_u16(0);
            single.put_u16(2);
            single.put_u16(0xFFFF);
            (shared.into_inner(), empty.into_inner(), single.into_inner())
        };
        let (shared, empty, single) = lists;
        let header_len = 8_usize;
        let table_len = 8_usize;
        let shared_at = header_len.checked_add(table_len).expect("small");
        let empty_at = shared_at.checked_add(shared.len()).expect("small");
        let single_at = empty_at.checked_add(empty.len()).expect("small");
        let word = |at: usize| -> u16 {
            u16::try_from(at.checked_div(2).expect("even")).expect("test offsets fit")
        };
        let mut w = Writer::new();
        w.put_i16(-128);
        w.put_i16(-256);
        w.put_u16(2);
        w.put_u16(2);
        // Row-major: (0,0), (1,0), (0,1), (1,1).
        w.put_u16(word(shared_at));
        w.put_u16(word(empty_at));
        w.put_u16(word(single_at));
        w.put_u16(word(shared_at));
        w.put_bytes(&shared);
        w.put_bytes(&empty);
        w.put_bytes(&single);
        w.into_inner()
    }

    #[test]
    fn header_grid_and_shared_or_empty_lists_parse() {
        let map = Blockmap::parse(&build()).expect("the fixture parses");
        assert_eq!(
            (map.origin_x, map.origin_y),
            (-128_i16, -256_i16),
            "origins"
        );
        assert_eq!((map.columns, map.rows), (2, 2), "grid");
        assert_eq!(map.blocks.len(), 4, "one list per cell");
        assert_eq!(map.blocks[0], vec![5, 9], "the first shared list");
        assert!(map.blocks[1].is_empty(), "the empty list");
        assert_eq!(map.blocks[2], vec![2], "the single-linedef list");
        assert_eq!(map.blocks[3], vec![5, 9], "the shared copy reads the same");
    }

    #[test]
    fn block_edges_step_by_128_map_units() {
        let map = Blockmap::parse(&build()).expect("the fixture parses");
        assert_eq!(map.block_x(0), Some(-128_i32), "origin column");
        assert_eq!(map.block_x(2), Some(128_i32), "two columns over");
        assert_eq!(map.block_y(1), Some(-128_i32), "one row over");
        assert_eq!(map.block_y(usize::MAX), None, "absurd row");
    }

    #[test]
    fn short_lumps_are_format_errors() {
        for bytes in [&[][..], b"1234567", b"1234"] {
            let error = Blockmap::parse(bytes).expect_err("no whole header or table");
            assert_eq!(error.code(), "format_error", "short lump code");
            assert!(
                error.to_string().contains("BLOCKMAP"),
                "the lump is named: {error}"
            );
        }
        let mut w = Writer::new();
        w.put_i16(0);
        w.put_i16(0);
        w.put_u16(2);
        w.put_u16(2);
        w.put_u16(4);
        let error = Blockmap::parse(&w.into_inner()).expect_err("the table is cut");
        assert!(
            error.to_string().contains("offset table"),
            "the table is named: {error}"
        );
    }

    #[test]
    fn wild_offsets_are_format_errors() {
        let mut bytes = build();
        let mut w = Writer::new();
        w.put_u16(u16::MAX);
        let wild = w.into_inner();
        bytes.splice(8_usize..10_usize, wild);
        let error = Blockmap::parse(&bytes).expect_err("an offset leaves the lump");
        assert_eq!(error.code(), "format_error", "wild offset code");
        assert!(
            error.to_string().contains("outside"),
            "the fault is named: {error}"
        );
    }

    #[test]
    fn malformed_lists_are_format_errors() {
        let mut bytes = build();
        bytes[16] = 7;
        let head_error = Blockmap::parse(&bytes).expect_err("the list loses its 0x0000");
        assert!(
            head_error.to_string().contains("does not start"),
            "the fault is named: {head_error}"
        );
        let mut truncated = build();
        let cut = truncated.len().checked_sub(2).expect("long enough");
        truncated.truncate(cut);
        let end_error = Blockmap::parse(&truncated).expect_err("the last list loses 0xFFFF");
        assert!(
            end_error.to_string().contains("not terminated"),
            "the fault is named: {end_error}"
        );
    }

    #[test]
    fn blockmap_serializes_for_json_assets() {
        let map = Blockmap::parse(&build()).expect("the fixture parses");
        let json = serde_json::to_value(&map).expect("a blockmap serializes");
        assert_eq!(json["origin_x"], serde_json::json!(-128_i64), "origin");
        assert_eq!(json["columns"], serde_json::json!(2_u64), "grid width");
        assert_eq!(
            json["blocks"],
            serde_json::json!([[5_u64, 9_u64], [], [2_u64], [5_u64, 9_u64]]),
            "the block lists"
        );
    }

    #[test]
    fn to_bytes_round_trips_compressed_and_plain() {
        let map = Blockmap::parse(&build()).expect("the fixture parses");
        let plain = map.to_bytes(false).expect("the fixture serializes");
        let packed = map.to_bytes(true).expect("the fixture compresses");
        assert_eq!(
            Blockmap::parse(&plain).expect("the plain lump parses"),
            map,
            "uncompressed round trip"
        );
        assert_eq!(
            Blockmap::parse(&packed).expect("the compressed lump parses"),
            map,
            "compressed round trip"
        );
        assert!(
            packed.len() < plain.len(),
            "the shared [5, 9] list is written once: {packed:#?} vs {plain:#?}"
        );
        let empty = Blockmap {
            origin_x: 0,
            origin_y: 0,
            columns: 0,
            rows: 0,
            blocks: Vec::new(),
        };
        let bytes = empty.to_bytes(true).expect("an empty grid writes");
        assert_eq!(bytes.len(), 8, "header only");
        assert_eq!(
            Blockmap::parse(&bytes).expect("it parses"),
            empty,
            "and round trips"
        );
    }

    #[test]
    fn to_bytes_refuses_terminators_and_count_mismatches() {
        let terminator = Blockmap {
            origin_x: 0,
            origin_y: 0,
            columns: 1,
            rows: 1,
            blocks: vec![vec![0xFFFF_u16]],
        };
        let error = terminator
            .to_bytes(false)
            .expect_err("0xFFFF is the list terminator, not an entry");
        assert_eq!(error.code(), "invalid_argument", "terminator code");
        assert!(
            error.to_string().contains("terminator"),
            "the fault is named: {error}"
        );
        let mismatch = Blockmap {
            origin_x: 0,
            origin_y: 0,
            columns: 2,
            rows: 2,
            blocks: vec![Vec::new()],
        };
        let mismatch_error = mismatch
            .to_bytes(false)
            .expect_err("one list cannot serve a 2x2 grid");
        assert_eq!(
            mismatch_error.code(),
            "invalid_argument",
            "count mismatch code"
        );
        assert!(
            mismatch_error.to_string().contains("rebuild"),
            "the fix is named: {mismatch_error}"
        );
    }

    #[test]
    fn to_bytes_refuses_grids_and_lists_past_the_addressable_size() {
        let wild = Blockmap {
            origin_x: 0,
            origin_y: 0,
            columns: 300,
            rows: 300,
            blocks: vec![Vec::new(); 90_000_usize],
        };
        let wild_error = wild.to_bytes(true).expect_err("90000 offsets do not fit");
        assert_eq!(wild_error.code(), "limit_exceeded", "wild grid code");
        assert!(
            wild_error.to_string().contains("blockmap too large"),
            "the ticket phrase is kept: {wild_error}"
        );
        let mut huge = vec![7_u16; MAX_LUMP_WORDS.saturating_sub(5)];
        huge.push(8_u16);
        let heavy = Blockmap {
            origin_x: 0,
            origin_y: 0,
            columns: 1,
            rows: 1,
            blocks: vec![huge],
        };
        let heavy_error = heavy
            .to_bytes(true)
            .expect_err("one list of 65531 entries leaves the lump");
        assert_eq!(heavy_error.code(), "limit_exceeded", "oversized list code");
    }
}
