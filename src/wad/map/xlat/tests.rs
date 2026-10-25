//! Shared builders for the translation stages' tests: the embedded-row
//! assertion [`check_row`], the one-element-per-value maps [`thing_map`] and
//! [`sector_map`] with their read-backs [`flags_of`] and [`specials_of`], the
//! sector there-and-back trip [`trip_sector_base`], the linedef spec rows
//! [`LineSpec`] with [`line_map`] and [`only_line`], and the whole-table
//! there-and-back trip [`trip_table_row`].
//!
//! [`table`](self::table) measures the embedded table with its parsing
//! refusals, [`things_sectors`](self::things_sectors) the thing and sector
//! stages beside the report wire shapes,
//! [`lines_restore`](self::lines_restore) the linedef round trips,
//! [`lines_warnings`](self::lines_warnings) the grouped loss warnings and
//! [`lines_tags`](self::lines_tags) the tag and line-id rules with the
//! whole-table trip.

mod lines_restore;
mod lines_tags;
mod lines_warnings;
mod table;
mod things_sectors;

use crate::wad::map::model::{Linedef, Map, Sector, Thing};
use crate::wad::map::xlat::table::MAX_ARGS;
use crate::wad::map::xlat::{
    XlatRow, linedefs_from_hexen, linedefs_to_hexen, rows_for_doom, sectors_from_hexen,
    sectors_to_hexen,
};

/// Assert the embedded table has exactly one row for `expected.doom`
/// and it equals `expected` field by field.
pub(super) fn check_row(expected: &XlatRow) {
    let rows = rows_for_doom(expected.doom).expect("the embedded table is valid");
    assert_eq!(
        rows.len(),
        1,
        "exactly one row translates special {}",
        expected.doom
    );
    let found = rows.first().expect("the row exists");
    assert_eq!(
        *found, expected,
        "full field values of row {}",
        expected.doom
    );
}

/// A map with one thing per flag value.
pub(super) fn thing_map(flags: &[u32]) -> Map {
    Map {
        things: flags
            .iter()
            .copied()
            .map(|bits| Thing {
                flags: bits,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// A map with one sector per special value.
pub(super) fn sector_map(specials: &[i32]) -> Map {
    Map {
        sectors: specials
            .iter()
            .copied()
            .map(|special| Sector {
                special,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// The flags of `map`'s things.
pub(super) fn flags_of(map: &Map) -> Vec<u32> {
    map.things.iter().map(|spawned| spawned.flags).collect()
}

/// The specials of `map`'s sectors.
pub(super) fn specials_of(map: &Map) -> Vec<i32> {
    map.sectors.iter().map(|area| area.special).collect()
}

/// Trips `base` with each of `field_sets` through both sector stages.
pub(super) fn trip_sector_base(base: i32, field_sets: &[i32]) {
    for fields in field_sets {
        let original = base | fields;
        let mut map = sector_map(&[original]);
        let there = sectors_to_hexen(&mut map);
        let back = sectors_from_hexen(&mut map);
        assert_eq!(specials_of(&map), vec![original], "{original} comes home");
        assert!(
            there.warnings.is_empty() && back.warnings.is_empty(),
            "the trip warns nothing for {original}"
        );
    }
}

/// One test linedef: `(special, tag, args, flags)`.
pub(super) type LineSpec = (i32, i32, [i32; MAX_ARGS], u32);

/// The all-zero argument array.
pub(super) const ZERO_ARGS: [i32; MAX_ARGS] = [0_i32; MAX_ARGS];

/// A map with one linedef per spec row.
pub(super) fn line_map(lines: &[LineSpec]) -> Map {
    Map {
        linedefs: lines
            .iter()
            .map(|&(special, tag, args, flags)| Linedef {
                flags,
                special,
                tag,
                args,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// The one line of a one-line map.
pub(super) fn only_line(map: &Map) -> &Linedef {
    map.linedefs.first().expect("the map has one line")
}

/// One row of the embedded table there and back: Doom `special`
/// `row.doom` with trigger tag 13 forward through `UDMF`, then reverse.
/// The special always comes back; the tag returns from the row's `tag`
/// slot when the row has one and ends 0 when it has none (the homeless
/// line id drops on the way back).
pub(super) fn trip_table_row(row: &XlatRow) {
    let mut map = line_map(&[(row.doom, 13_i32, ZERO_ARGS, 0_u32)]);
    let there = linedefs_to_hexen(&mut map, false).expect("the table parses");
    assert_eq!(there.translated, 1, "doom {} translated", row.doom);
    assert!(
        there.untranslated.is_empty() && there.warnings.is_empty(),
        "the forward trip of row {} is lossless: {there:?}",
        row.doom
    );
    let back = linedefs_from_hexen(&mut map, true).expect("the table parses");
    assert_eq!(back.translated, 1, "doom {} came back", row.doom);
    assert!(back.untranslated.is_empty(), "row {} found", row.doom);
    let doom = only_line(&map);
    assert_eq!(
        doom.special, row.doom,
        "row {} restores its own special",
        row.doom
    );
    let wanted = if row.tag_slot.is_some() {
        13_i32
    } else {
        0_i32
    };
    assert_eq!(doom.tag, wanted, "the tag of row {}", row.doom);
    assert_eq!(doom.args, ZERO_ARGS, "the matched line zeroes its args");
    assert_eq!(doom.flags, 0_u32, "the trigger bits were consumed");
    let lost = back
        .warnings
        .iter()
        .filter(|note| note.contains("line id 13 was dropped"))
        .count();
    if row.tag_slot.is_some() {
        assert!(
            back.warnings.is_empty(),
            "lossless trip: {:?}",
            back.warnings
        );
    } else {
        assert_eq!(
            lost, 1_usize,
            "row {} carries its tag as a busy id: {:?}",
            row.doom, back.warnings
        );
    }
}
