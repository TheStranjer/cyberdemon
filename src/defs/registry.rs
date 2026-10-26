//! The definitions registry: embedded game tables and how to select them.
//!
//! [`definitions_for`] answers one question for a [`Game`]: what can an
//! editor offer for it? The DOOM / DOOM II, the Heretic and the Hexen thing,
//! linedef-special and sector-type tables are embedded in the binary with
//! `include_str!`, parsed once on first use behind a `LazyLock` through the
//! shared [`crate::defs::tsv`] reader, and assembled into a [`Definitions`]
//! whose lookups run through indexes built once at assembly. Doom
//! thing and special selection follows the `game` column, which names the
//! first game that has each entry, while the Doom sector table covers both
//! DOOM games. The Heretic and Hexen tables carry only their own rows; the
//! Hexen specials name their script arguments (`arg1=Sector Tag;...`) in
//! [`SpecialDef::args`]. `Strife` and [`Game::Unknown`] fall back to the
//! DOOM II sets with a warning.

use std::sync::LazyLock;

use alloc::collections::BTreeMap;

use crate::defs::table::{
    SectorTypeDef, SpecialDef, ThingDef, parse_sector_table, parse_special_table, parse_thing_table,
};
use crate::defs::tsv::embedded;
use crate::error::CyberError;
use crate::wad::game::Game;

/// The DOOM / DOOM II thing table, compiled into the binary from
/// `data/defs/doom_things.tsv`.
pub(crate) const DOOM_THINGS_TSV: &str = include_str!("../../data/defs/doom_things.tsv");

/// The DOOM / DOOM II linedef-special table, compiled into the binary from
/// `data/defs/doom_specials.tsv`.
pub(crate) const DOOM_SPECIALS_TSV: &str = include_str!("../../data/defs/doom_specials.tsv");

/// The Doom sector-type table, compiled into the binary from
/// `data/defs/doom_sectors.tsv`.
pub(crate) const DOOM_SECTORS_TSV: &str = include_str!("../../data/defs/doom_sectors.tsv");

/// The Heretic thing table, compiled into the binary from
/// `data/defs/heretic_things.tsv`.
pub(crate) const HERETIC_THINGS_TSV: &str = include_str!("../../data/defs/heretic_things.tsv");

/// The Heretic linedef-special table, compiled into the binary from
/// `data/defs/heretic_specials.tsv`.
pub(crate) const HERETIC_SPECIALS_TSV: &str = include_str!("../../data/defs/heretic_specials.tsv");

/// The Heretic sector-type table, compiled into the binary from
/// `data/defs/heretic_sectors.tsv`.
pub(crate) const HERETIC_SECTORS_TSV: &str = include_str!("../../data/defs/heretic_sectors.tsv");

/// The Hexen thing table, compiled into the binary from
/// `data/defs/hexen_things.tsv`.
pub(crate) const HEXEN_THINGS_TSV: &str = include_str!("../../data/defs/hexen_things.tsv");

/// The Hexen linedef-special table, compiled into the binary from
/// `data/defs/hexen_specials.tsv`. Its rows name their script arguments.
pub(crate) const HEXEN_SPECIALS_TSV: &str = include_str!("../../data/defs/hexen_specials.tsv");

/// The Hexen sector-type table, compiled into the binary from
/// `data/defs/hexen_sectors.tsv`.
pub(crate) const HEXEN_SECTORS_TSV: &str = include_str!("../../data/defs/hexen_sectors.tsv");

/// The `game` column value of entries DOOM already has.
const DOOM_ROW: &str = "doom";

/// The `game` column value of entries first added in DOOM II.
const DOOM2_ROW: &str = "doom2";

/// The parsed [`DOOM_THINGS_TSV`], produced on first use. A parse failure
/// means the embedded data is broken: it is kept as an error string and
/// reported as [`CyberError::Internal`] by every accessor, never a panic.
static DOOM_THINGS: LazyLock<Result<Vec<ThingDef>, String>> =
    LazyLock::new(|| parse_thing_table(DOOM_THINGS_TSV));

/// The parsed [`DOOM_SPECIALS_TSV`], with the same broken-data policy as
/// [`DOOM_THINGS`].
static DOOM_SPECIALS: LazyLock<Result<Vec<SpecialDef>, String>> =
    LazyLock::new(|| parse_special_table(DOOM_SPECIALS_TSV));

/// The parsed [`DOOM_SECTORS_TSV`], with the same broken-data policy as
/// [`DOOM_THINGS`].
static DOOM_SECTORS: LazyLock<Result<Vec<SectorTypeDef>, String>> =
    LazyLock::new(|| parse_sector_table(DOOM_SECTORS_TSV));

/// The parsed [`HERETIC_THINGS_TSV`], with the same broken-data policy as
/// [`DOOM_THINGS`].
static HERETIC_THINGS: LazyLock<Result<Vec<ThingDef>, String>> =
    LazyLock::new(|| parse_thing_table(HERETIC_THINGS_TSV));

/// The parsed [`HERETIC_SPECIALS_TSV`], with the same broken-data policy as
/// [`DOOM_THINGS`].
static HERETIC_SPECIALS: LazyLock<Result<Vec<SpecialDef>, String>> =
    LazyLock::new(|| parse_special_table(HERETIC_SPECIALS_TSV));

/// The parsed [`HERETIC_SECTORS_TSV`], with the same broken-data policy as
/// [`DOOM_THINGS`].
static HERETIC_SECTORS: LazyLock<Result<Vec<SectorTypeDef>, String>> =
    LazyLock::new(|| parse_sector_table(HERETIC_SECTORS_TSV));

/// The parsed [`HEXEN_THINGS_TSV`], with the same broken-data policy as
/// [`DOOM_THINGS`].
static HEXEN_THINGS: LazyLock<Result<Vec<ThingDef>, String>> =
    LazyLock::new(|| parse_thing_table(HEXEN_THINGS_TSV));

/// The parsed [`HEXEN_SPECIALS_TSV`], with the same broken-data policy as
/// [`DOOM_THINGS`].
static HEXEN_SPECIALS: LazyLock<Result<Vec<SpecialDef>, String>> =
    LazyLock::new(|| parse_special_table(HEXEN_SPECIALS_TSV));

/// The parsed [`HEXEN_SECTORS_TSV`], with the same broken-data policy as
/// [`DOOM_THINGS`].
static HEXEN_SECTORS: LazyLock<Result<Vec<SectorTypeDef>, String>> =
    LazyLock::new(|| parse_sector_table(HEXEN_SECTORS_TSV));

/// Everything the server knows about one game's editing definitions.
///
/// The three tables answer by number, and by name for things, through
/// private index maps built once when the value is assembled; the overlay
/// mutators `put_thing`, `remove_thing` and `reindex_things` keep those
/// indexes true, so a [`Definitions::thing`], [`Definitions::thing_by_name`],
/// [`Definitions::special`] or [`Definitions::sector_type`] lookup is a map
/// read wherever it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definitions {
    /// The game these definitions were selected for.
    pub(crate) game: Game,
    /// The thing types the game offers, sorted by type number.
    pub(crate) things: Vec<ThingDef>,
    /// The special-table linedef actions the game offers, sorted by
    /// special number. Generalized values are not rows; see
    /// [`crate::defs::boom`].
    pub(crate) specials: Vec<SpecialDef>,
    /// The sector types the game offers, sorted by type number.
    pub(crate) sector_types: Vec<SectorTypeDef>,
    /// Non-fatal notes: fallback games taken.
    pub(crate) warnings: Vec<String>,
    /// Thing type number to its row's index; the first row of a number wins.
    things_by_kind: BTreeMap<u32, usize>,
    /// Lowercased thing name to its row's index; the first row of a name wins.
    things_by_name: BTreeMap<String, usize>,
    /// Special number to its row's index; the first row of a number wins.
    specials_by_number: BTreeMap<u32, usize>,
    /// Sector type value to its row's index; the first row of a number wins.
    sector_types_by_number: BTreeMap<u32, usize>,
}

/// The three tables and the notes one [`Definitions`] value is assembled
/// from, in the shape [`Definitions::new`] takes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DefinitionTables {
    /// The thing rows.
    pub(crate) things: Vec<ThingDef>,
    /// The special rows.
    pub(crate) specials: Vec<SpecialDef>,
    /// The sector-type rows.
    pub(crate) sector_types: Vec<SectorTypeDef>,
    /// The non-fatal notes to carry.
    pub(crate) warnings: Vec<String>,
}

impl Definitions {
    /// The definitions for `game` over `tables`: the rows stand as given
    /// (the builders hand them sorted by number), with the lookup indexes
    /// built once over them. A number or name carried twice keeps its
    /// first row, exactly as a linear first-match lookup did.
    #[must_use]
    pub(crate) fn new(game: Game, tables: DefinitionTables) -> Self {
        let mut defs = Self {
            game,
            things: tables.things,
            specials: tables.specials,
            sector_types: tables.sector_types,
            warnings: tables.warnings,
            things_by_kind: BTreeMap::new(),
            things_by_name: BTreeMap::new(),
            specials_by_number: BTreeMap::new(),
            sector_types_by_number: BTreeMap::new(),
        };
        defs.reindex();
        defs
    }

    /// The definition of thing type `kind`, when this game has one.
    #[must_use]
    #[inline]
    pub fn thing(&self, kind: u32) -> Option<&ThingDef> {
        self.things_by_kind
            .get(&kind)
            .and_then(|index| self.things.get(*index))
    }

    /// The definition whose thing name is `name`, compared
    /// case-insensitively on the ASCII case fold; the first such row wins.
    #[must_use]
    #[inline]
    pub fn thing_by_name(&self, name: &str) -> Option<&ThingDef> {
        self.things_by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|index| self.things.get(*index))
    }

    /// The definition of special-table linedef action `number`, when this
    /// game has one.
    #[must_use]
    #[inline]
    pub fn special(&self, number: u32) -> Option<&SpecialDef> {
        self.specials_by_number
            .get(&number)
            .and_then(|index| self.specials.get(*index))
    }

    /// The definition of sector type `value`, when this game has one.
    #[must_use]
    #[inline]
    pub fn sector_type(&self, value: u32) -> Option<&SectorTypeDef> {
        self.sector_types_by_number
            .get(&value)
            .and_then(|index| self.sector_types.get(*index))
    }

    /// Inserts or replaces the row of thing type `def.kind`, dropping any
    /// row that already uses the number, and keeps the thing indexes true.
    /// The new row stands at the table's end until the next sort.
    pub(crate) fn put_thing(&mut self, def: ThingDef) {
        self.things.retain(|row| row.kind != def.kind);
        self.things.push(def);
        self.reindex_things();
    }

    /// Drops the row of thing type `kind`, if this table has one, keeping
    /// the thing indexes true.
    pub(crate) fn remove_thing(&mut self, kind: u32) {
        self.things.retain(|row| row.kind != kind);
        self.reindex_things();
    }

    /// The mutable row of thing type `kind`, when this game has one.
    pub(crate) fn thing_mut(&mut self, kind: u32) -> Option<&mut ThingDef> {
        let index = self.things_by_kind.get(&kind).copied();
        index.and_then(|at| self.things.get_mut(at))
    }

    /// Rebuilds the thing indexes after a direct edit of [`Self::things`],
    /// e.g. the sort that closes a session overlay.
    pub(crate) fn reindex_things(&mut self) {
        let mut kinds = BTreeMap::new();
        let mut names = BTreeMap::new();
        for (index, row) in self.things.iter().enumerate() {
            kinds.entry(row.kind).or_insert(index);
            names.entry(row.name.to_ascii_lowercase()).or_insert(index);
        }
        self.things_by_kind = kinds;
        self.things_by_name = names;
    }

    /// Builds every lookup index over the tables as they stand.
    fn reindex(&mut self) {
        self.reindex_things();
        let mut specials = BTreeMap::new();
        for (index, row) in self.specials.iter().enumerate() {
            specials.entry(row.number).or_insert(index);
        }
        self.specials_by_number = specials;
        let mut sectors = BTreeMap::new();
        for (index, row) in self.sector_types.iter().enumerate() {
            sectors.entry(row.number).or_insert(index);
        }
        self.sector_types_by_number = sectors;
    }
}

/// The editing definitions for `game`.
///
/// `DOOM` gets the rows whose first game is DOOM; `DOOM II` adds the rows
/// first seen in DOOM II and shares the sector table. Heretic and Hexen get
/// their own three tables; Hexen specials carry their script argument names.
/// Strife and [`Game::Unknown`] get the DOOM II sets with a fallback
/// warning.
///
/// # Errors
///
/// [`CyberError::Internal`] when an embedded table is malformed, which is a
/// build bug rather than a client problem.
#[inline]
pub fn definitions_for(game: Game) -> Result<Definitions, CyberError> {
    let (things, specials, sector_types, warnings) = match game {
        Game::Doom => (
            doom_things(false)?,
            doom_specials(false)?,
            doom_sectors()?,
            Vec::new(),
        ),
        Game::Doom2 => (
            doom_things(true)?,
            doom_specials(true)?,
            doom_sectors()?,
            Vec::new(),
        ),
        Game::Heretic => (
            heretic_things()?,
            heretic_specials()?,
            heretic_sectors()?,
            Vec::new(),
        ),
        Game::Hexen => (
            hexen_things()?,
            hexen_specials()?,
            hexen_sectors()?,
            Vec::new(),
        ),
        Game::Strife | Game::Unknown => (
            doom_things(true)?,
            doom_specials(true)?,
            doom_sectors()?,
            vec![doom2_fallback(game)],
        ),
    };
    Ok(Definitions::new(
        game,
        DefinitionTables {
            things,
            specials,
            sector_types,
            warnings,
        },
    ))
}

/// The embedded thing rows for the games whose tables exist, `with_doom2`
/// adding the DOOM II entries, sorted by type number.
///
/// # Errors
///
/// [`embedded_doom_things`]'s [`CyberError::Internal`] for malformed
/// embedded data.
fn doom_things(with_doom2: bool) -> Result<Vec<ThingDef>, CyberError> {
    let mut rows: Vec<ThingDef> = embedded(&DOOM_THINGS, "thing table")?
        .iter()
        .filter(|row| row.game == DOOM_ROW || (with_doom2 && row.game == DOOM2_ROW))
        .cloned()
        .collect();
    rows.sort_by_key(|row| row.kind);
    Ok(rows)
}

/// The embedded linedef-special rows, `with_doom2` adding the DOOM II
/// entries, sorted by special number.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn doom_specials(with_doom2: bool) -> Result<Vec<SpecialDef>, CyberError> {
    let mut rows: Vec<SpecialDef> = embedded(&DOOM_SPECIALS, "special table")?
        .iter()
        .filter(|row| row.game == DOOM_ROW || (with_doom2 && row.game == DOOM2_ROW))
        .cloned()
        .collect();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The embedded sector-type rows for the Doom family, sorted by type
/// number.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn doom_sectors() -> Result<Vec<SectorTypeDef>, CyberError> {
    let mut rows: Vec<SectorTypeDef> = embedded(&DOOM_SECTORS, "sector table")?.clone();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The embedded Heretic thing rows, sorted by type number. Every row of the
/// Heretic table is Heretic's own, so nothing filters them out; the tests
/// check that the table holds no Doom rows.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn heretic_things() -> Result<Vec<ThingDef>, CyberError> {
    let mut rows: Vec<ThingDef> = embedded(&HERETIC_THINGS, "heretic thing table")?.clone();
    rows.sort_by_key(|row| row.kind);
    Ok(rows)
}

/// The embedded Heretic linedef-special rows, sorted by special number.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn heretic_specials() -> Result<Vec<SpecialDef>, CyberError> {
    let mut rows: Vec<SpecialDef> = embedded(&HERETIC_SPECIALS, "heretic special table")?.clone();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The embedded Heretic sector-type rows, sorted by type number.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn heretic_sectors() -> Result<Vec<SectorTypeDef>, CyberError> {
    let mut rows: Vec<SectorTypeDef> = embedded(&HERETIC_SECTORS, "heretic sector table")?.clone();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The embedded Hexen thing rows, sorted by type number. Every row of the
/// Hexen table is Hexen's own, so nothing filters them out; the tests check
/// that the table holds no Doom or Heretic rows.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn hexen_things() -> Result<Vec<ThingDef>, CyberError> {
    let mut rows: Vec<ThingDef> = embedded(&HEXEN_THINGS, "hexen thing table")?.clone();
    rows.sort_by_key(|row| row.kind);
    Ok(rows)
}

/// The embedded Hexen linedef-special rows, sorted by special number; each
/// row keeps the argument names from its `args` column.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn hexen_specials() -> Result<Vec<SpecialDef>, CyberError> {
    let mut rows: Vec<SpecialDef> = embedded(&HEXEN_SPECIALS, "hexen special table")?.clone();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The embedded Hexen sector-type rows, sorted by type number.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn hexen_sectors() -> Result<Vec<SectorTypeDef>, CyberError> {
    let mut rows: Vec<SectorTypeDef> = embedded(&HEXEN_SECTORS, "hexen sector table")?.clone();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The warning for a game served by the DOOM II fallback tables.
fn doom2_fallback(game: Game) -> String {
    format!(
        "no definition tables for {} yet; showing the DOOM II definitions",
        game_label(game)
    )
}

/// The lowercase game name used in warning text.
pub(crate) const fn game_label(game: Game) -> &'static str {
    match game {
        Game::Doom => "doom",
        Game::Doom2 => "doom2",
        Game::Heretic => "heretic",
        Game::Hexen => "hexen",
        Game::Strife => "strife",
        Game::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests;
