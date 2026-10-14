//! The definitions registry: embedded game tables and how to select them.
//!
//! [`definitions_for`] answers one question for a [`Game`]: what can an
//! editor offer for it? The DOOM / DOOM II, the Heretic and the Hexen thing,
//! linedef-special and sector-type tables are embedded in the binary with
//! `include_str!` and parsed once on first use behind a `LazyLock`; Doom
//! thing and special selection follows the `game` column, which names the
//! first game that has each entry, while the Doom sector table covers both
//! DOOM games. The Heretic and Hexen tables carry only their own rows; the
//! Hexen specials name their script arguments (`arg1=Sector Tag;...`) in
//! [`SpecialDef::args`]. `Strife` and [`Game::Unknown`] fall back to the
//! DOOM II sets with a warning.

use std::sync::LazyLock;

use crate::defs::table::{
    SectorTypeDef, SpecialDef, ThingDef, parse_sector_table, parse_special_table, parse_thing_table,
};
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definitions {
    /// The game these definitions were selected for.
    pub game: Game,
    /// The thing types the game offers, sorted by type number.
    pub things: Vec<ThingDef>,
    /// The special-table linedef actions the game offers, sorted by
    /// special number. Generalized values are not rows; see
    /// [`crate::defs::boom`].
    pub specials: Vec<SpecialDef>,
    /// The sector types the game offers, sorted by type number.
    pub sector_types: Vec<SectorTypeDef>,
    /// Non-fatal notes: fallback games taken.
    pub warnings: Vec<String>,
}

impl Definitions {
    /// The definition of thing type `kind`, when this game has one.
    #[must_use]
    #[inline]
    pub fn thing(&self, kind: u32) -> Option<&ThingDef> {
        self.things.iter().find(|row| row.kind == kind)
    }

    /// The definition of special-table linedef action `number`, when this
    /// game has one.
    #[must_use]
    #[inline]
    pub fn special(&self, number: u32) -> Option<&SpecialDef> {
        self.specials.iter().find(|row| row.number == number)
    }

    /// The definition of sector type `value`, when this game has one.
    #[must_use]
    #[inline]
    pub fn sector_type(&self, value: u32) -> Option<&SectorTypeDef> {
        self.sector_types.iter().find(|row| row.number == value)
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
    Ok(Definitions {
        game,
        things,
        specials,
        sector_types,
        warnings,
    })
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
    let mut rows: Vec<SectorTypeDef> = embedded(&DOOM_SECTORS, "sector table")?.to_vec();
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
    let mut rows: Vec<ThingDef> = embedded(&HERETIC_THINGS, "heretic thing table")?.to_vec();
    rows.sort_by_key(|row| row.kind);
    Ok(rows)
}

/// The embedded Heretic linedef-special rows, sorted by special number.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn heretic_specials() -> Result<Vec<SpecialDef>, CyberError> {
    let mut rows: Vec<SpecialDef> = embedded(&HERETIC_SPECIALS, "heretic special table")?.to_vec();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The embedded Heretic sector-type rows, sorted by type number.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn heretic_sectors() -> Result<Vec<SectorTypeDef>, CyberError> {
    let mut rows: Vec<SectorTypeDef> = embedded(&HERETIC_SECTORS, "heretic sector table")?.to_vec();
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
    let mut rows: Vec<ThingDef> = embedded(&HEXEN_THINGS, "hexen thing table")?.to_vec();
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
    let mut rows: Vec<SpecialDef> = embedded(&HEXEN_SPECIALS, "hexen special table")?.to_vec();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The embedded Hexen sector-type rows, sorted by type number.
///
/// # Errors
///
/// [`embedded`]'s [`CyberError::Internal`] for malformed embedded data.
fn hexen_sectors() -> Result<Vec<SectorTypeDef>, CyberError> {
    let mut rows: Vec<SectorTypeDef> = embedded(&HEXEN_SECTORS, "hexen sector table")?.to_vec();
    rows.sort_by_key(|row| row.number);
    Ok(rows)
}

/// The parsed rows of an embedded table, or the internal error quoting its
/// parse failure; `label` names the table in the message.
///
/// # Errors
///
/// [`CyberError::Internal`] when the embedded data is malformed.
fn embedded<T>(
    parsed: &'static LazyLock<Result<Vec<T>, String>>,
    label: &str,
) -> Result<&'static [T], CyberError> {
    parsed.as_deref().map_err(|reason| {
        CyberError::Internal(format!(
            "embedded {label} is malformed (this is a server bug): {reason}"
        ))
    })
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
mod tests {
    use super::{Definitions, definitions_for, game_label};
    use crate::defs::boom::{SectorFamily, decode_sector_special};
    use crate::wad::game::Game;

    /// Whether any row in `defs` has this type number.
    fn has_kind(defs: &Definitions, kind: u32) -> bool {
        defs.things.iter().any(|row| row.kind == kind)
    }

    /// Whether `numbers` are in ascending order.
    fn is_ascending(numbers: &[u32]) -> bool {
        let mut sorted = numbers.to_vec();
        sorted.sort_unstable();
        sorted == numbers
    }

    #[test]
    fn doom_excludes_the_doom_ii_only_things() {
        let defs = definitions_for(Game::Doom).expect("the embedded table is valid");
        assert_eq!(defs.game, Game::Doom, "the game is echoed");
        assert_eq!(defs.things.len(), 97, "the doom rows of the thing table");
        assert!(!has_kind(&defs, 65), "Chaingunner is a DOOM II thing");
        assert!(has_kind(&defs, 3001), "Imp is a DOOM thing");
        assert!(defs.warnings.is_empty(), "DOOM has its own table");
    }

    #[test]
    fn doom2_adds_the_doom_ii_only_things() {
        let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
        assert_eq!(defs.things.len(), 123, "both games' rows");
        assert!(has_kind(&defs, 65), "Chaingunner is there");
        assert!(has_kind(&defs, 3001), "and the DOOM things as well");
        assert!(defs.warnings.is_empty(), "DOOM II has its own table");
    }

    #[test]
    fn things_are_sorted_by_type_number() {
        let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
        let sorts: Vec<u32> = defs.things.iter().map(|row| row.kind).collect();
        let mut sorted = sorts.clone();
        sorted.sort_unstable();
        assert_eq!(sorts, sorted, "type numbers ascend");
    }

    #[test]
    fn doom_excludes_the_doom_ii_only_specials() {
        let defs = definitions_for(Game::Doom).expect("the embedded table is valid");
        assert_eq!(
            defs.specials.len(),
            100,
            "the doom rows of the special table"
        );
        assert!(defs.special(1).is_some(), "DR Door is a DOOM special");
        assert!(defs.special(117).is_none(), "DR Door Fast is DOOM II only");
        let numbers: Vec<u32> = defs.specials.iter().map(|row| row.number).collect();
        assert!(is_ascending(&numbers), "special numbers ascend");
    }

    #[test]
    fn doom2_adds_the_doom_ii_only_specials() {
        let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
        assert_eq!(defs.specials.len(), 139, "both games' rows");
        let fast = defs.special(117).expect("DR Door Fast exists");
        assert_eq!(fast.name, "DR Door Fast", "name");
        assert_eq!(fast.category, "Doors/Local", "category");
        assert_eq!(fast.target, "sector_back", "target column");
        assert!(
            fast.args.is_empty(),
            "Doom-family specials take no arguments"
        );
        assert_eq!(fast.game, "doom2", "first game");
        assert!(defs.special(1).is_some(), "and the DOOM specials as well");
    }

    #[test]
    fn sector_types_are_shared_by_the_doom_games() {
        let doom = definitions_for(Game::Doom).expect("the embedded table is valid");
        let doom2 = definitions_for(Game::Doom2).expect("the embedded table is valid");
        assert_eq!(doom.sector_types.len(), 16, "the shipped sector rows");
        assert_eq!(
            doom.sector_types, doom2.sector_types,
            "both games use the same table"
        );
        let damage = doom.sector_type(16).expect("type 16 exists");
        assert_eq!(damage.name, "20% Damage", "name lookup");
        assert!(doom.sector_type(6).is_none(), "type 6 is unassigned");
        let numbers: Vec<u32> = doom.sector_types.iter().map(|row| row.number).collect();
        assert!(is_ascending(&numbers), "type numbers ascend");
    }

    #[test]
    fn the_boom_decoder_names_every_sector_table_row() {
        let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
        for row in &defs.sector_types {
            let decoded = decode_sector_special(row.number, SectorFamily::Doom)
                .expect("the Doom family decodes");
            assert_eq!(decoded.base, row.number, "the base is the row itself");
            assert_eq!(
                decoded.base_name, row.name,
                "boom and the table agree on type {}",
                row.number
            );
        }
    }

    #[test]
    fn heretic_ships_its_own_tables() {
        let defs = definitions_for(Game::Heretic).expect("the embedded tables are valid");
        assert_eq!(defs.game, Game::Heretic, "the game is echoed");
        assert_eq!(defs.things.len(), 84, "the heretic thing rows");
        assert_eq!(defs.specials.len(), 105, "the heretic special rows");
        assert_eq!(defs.sector_types.len(), 47, "the heretic sector rows");
        assert!(defs.warnings.is_empty(), "heretic has its own tables");
        let kinds: Vec<u32> = defs.things.iter().map(|row| row.kind).collect();
        assert!(is_ascending(&kinds), "thing types ascend");
        let numbers: Vec<u32> = defs.specials.iter().map(|row| row.number).collect();
        assert!(is_ascending(&numbers), "special numbers ascend");
        let types: Vec<u32> = defs.sector_types.iter().map(|row| row.number).collect();
        assert!(is_ascending(&types), "sector types ascend");
    }

    #[test]
    fn heretic_lookups_name_the_heretic_rows() {
        let defs = definitions_for(Game::Heretic).expect("the embedded tables are valid");
        let gargoyle = defs.thing(66).expect("type 66 is the Gargoyle");
        assert_eq!(gargoyle.name, "Gargoyle", "name");
        assert_eq!(gargoyle.sprite, "IMPX", "sprite");
        assert_eq!(gargoyle.frame, 'A', "frame");
        assert_eq!(gargoyle.radius, 20, "radius");
        assert_eq!(gargoyle.category, "Monsters", "category");
        assert_eq!(gargoyle.game, "heretic", "game column");
        let lich = defs.thing(6).expect("type 6 is the Iron Lich");
        assert_eq!(lich.radius, 40, "a wide boss radius");
        let corpse = defs.thing(51).expect("type 51 is the Hanging Corpse");
        assert!(corpse.hanging, "the gore rows carry the hanging flag");
        let door = defs.special(100).expect("special 100 is a heretic door");
        assert_eq!(door.name, "WR Door Triple Speed", "name");
        assert_eq!(door.category, "Doors/Remote", "category");
        assert_eq!(door.target, "sector", "target column");
        assert!(door.args.is_empty(), "heretic specials take no arguments");
        let exit = defs.special(11).expect("special 11 is S1 Exit");
        assert_eq!(exit.target, "-", "an exit acts on no sector");
        let scroll = defs.sector_type(20).expect("sector type 20 exists");
        assert_eq!(scroll.name, "Scroll East (5)", "name");
        let damage = defs.sector_type(5).expect("sector type 5 exists");
        assert_eq!(damage.name, "5% Lava Damage", "heretic damage naming");
        assert!(defs.sector_type(6).is_none(), "type 6 is unassigned");
        assert!(
            defs.sector_type(17).is_none(),
            "types 17 to 19 are unassigned"
        );
        assert!(defs.sector_type(51).is_some(), "the table ends at type 51");
        assert!(defs.thing(3_001).is_none(), "no Doom monsters in here");
    }

    #[test]
    fn heretic_tables_hold_no_doom_rows() {
        let defs = definitions_for(Game::Heretic).expect("the embedded tables are valid");
        assert!(
            defs.things.iter().all(|row| row.game == "heretic"),
            "no Doom thing rows leaked in"
        );
        assert!(
            defs.specials.iter().all(|row| row.game == "heretic"),
            "no Doom special rows leaked in"
        );
        assert!(!has_kind(&defs, 3001), "the Imp is not a heretic thing");
        assert!(
            defs.special(117).is_none(),
            "DR Door Fast is a Doom II special"
        );
        let start = defs.thing(5).expect("heretic has a type 5");
        assert_eq!(start.name, "Fire Gargoyle", "not the Doom Blue Keycard");
        let pod = defs.thing(2035).expect("heretic has a type 2035");
        assert_eq!(pod.name, "Gas Pod", "not the Doom Barrel");
        assert_eq!(pod.sprite, "PPOD", "and its own sprite");
        let doom2 = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        let stairs = doom2.special(100).expect("DOOM II has a special 100");
        assert_eq!(stairs.name, "W1 Build Stairs 16 + Crush", "its own meaning");
    }

    #[test]
    fn the_doom_tables_hold_no_heretic_rows() {
        let doom2 = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        assert!(
            doom2.things.iter().all(|row| row.game != "heretic"),
            "the Doom thing sets stay as they were"
        );
        assert_eq!(doom2.things.len(), 123, "DOOM II keeps its own count");
        assert_eq!(doom2.specials.len(), 139, "DOOM II keeps its own count");
        assert_eq!(
            doom2.sector_types.len(),
            16,
            "the Doom sector table is untouched"
        );
        assert_eq!(
            doom2.thing(5).expect("DOOM II has a type 5").name,
            "Blue Keycard",
            "type 5 still means the Doom thing"
        );
    }

    #[test]
    fn hexen_ships_its_own_tables() {
        let defs = definitions_for(Game::Hexen).expect("the embedded tables are valid");
        assert_eq!(defs.game, Game::Hexen, "the game is echoed");
        assert_eq!(defs.things.len(), 233, "the hexen thing rows");
        assert_eq!(defs.specials.len(), 86, "the hexen special rows");
        assert_eq!(defs.sector_types.len(), 46, "the hexen sector rows");
        assert!(defs.warnings.is_empty(), "hexen has its own tables");
        let kinds: Vec<u32> = defs.things.iter().map(|row| row.kind).collect();
        assert!(is_ascending(&kinds), "thing types ascend");
        let numbers: Vec<u32> = defs.specials.iter().map(|row| row.number).collect();
        assert!(is_ascending(&numbers), "special numbers ascend");
        let types: Vec<u32> = defs.sector_types.iter().map(|row| row.number).collect();
        assert!(is_ascending(&types), "sector types ascend");
    }

    #[test]
    fn hexen_lookups_name_the_hexen_rows() {
        let defs = definitions_for(Game::Hexen).expect("the embedded tables are valid");
        let ettin = defs.thing(10030).expect("type 10030 is the Ettin");
        assert_eq!(ettin.name, "Ettin", "name");
        assert_eq!(ettin.sprite, "ETTN", "sprite");
        assert_eq!(ettin.frame, 'A', "frame");
        assert_eq!(ettin.radius, 25, "radius");
        assert_eq!(ettin.category, "Monsters", "category");
        assert_eq!(ettin.game, "hexen", "game column");
        let chandelier = defs.thing(17).expect("type 17 is the lit Chandelier");
        assert!(chandelier.hanging, "the gore and chandelier rows hang");
        let locked = defs.special(13).expect("special 13 is Door_LockedRaise");
        assert_eq!(locked.name, "Door_LockedRaise", "name");
        assert_eq!(locked.category, "Doors", "category");
        assert_eq!(locked.target, "sector_or_back", "free-form target column");
        let args: Vec<&str> = locked.args.iter().map(String::as_str).collect();
        assert_eq!(
            args,
            vec!["Sector Tag", "Speed", "Delay", "Lock"],
            "argument names in order, index 0 is arg1"
        );
        let acs = defs.special(80).expect("special 80 is ACS_Execute");
        assert_eq!(acs.args.len(), 5, "five script arguments");
        let endgame = defs.special(75).expect("special 75 is Teleport_EndGame");
        assert!(endgame.args.is_empty(), "a no-argument special stays empty");
        assert_eq!(endgame.target, "-", "an exit acts on nothing");
        let quake = defs.special(120).expect("special 120 is Radius_Quake");
        assert_eq!(quake.target, "ex_5thing", "ex_5thing passes through");
        let sky = defs.sector_type(200).expect("sector type 200 exists");
        assert_eq!(sky.name, "Sky 2 (MAPINFO)", "name");
        assert!(
            defs.sector_type(5).is_none(),
            "Doom damage types are not rows"
        );
        assert!(defs.thing(3_001).is_none(), "no Doom monsters in here");
    }

    #[test]
    fn hexen_tables_hold_no_doom_or_heretic_rows() {
        let defs = definitions_for(Game::Hexen).expect("the embedded tables are valid");
        assert!(
            defs.things.iter().all(|row| row.game == "hexen"),
            "no Doom or Heretic thing rows leaked in"
        );
        assert!(
            defs.specials.iter().all(|row| row.game == "hexen"),
            "no Doom or Heretic special rows leaked in"
        );
        assert!(!has_kind(&defs, 3_001), "the Imp is not a hexen thing");
        assert!(
            !defs.things.iter().any(|row| row.name == "Gargoyle"),
            "the heretic Gargoyle is not in the hexen table"
        );
        let doom2 = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        assert!(
            doom2.things.iter().all(|row| row.game != "hexen")
                && doom2.specials.iter().all(|row| row.game != "hexen"),
            "the Doom sets stay free of hexen rows"
        );
        let heretic = definitions_for(Game::Heretic).expect("the embedded tables are valid");
        assert!(
            heretic.things.iter().all(|row| row.game != "hexen")
                && heretic.specials.iter().all(|row| row.game != "hexen"),
            "the heretic sets stay free of hexen rows"
        );
    }

    #[test]
    fn strife_and_unknown_fall_back_to_the_doom2_table() {
        for game in [Game::Strife, Game::Unknown] {
            let defs = definitions_for(game).expect("the embedded tables are valid");
            assert_eq!(defs.game, game, "the requested game is echoed");
            assert_eq!(defs.things.len(), 123, "the DOOM II things for {game:?}");
            assert_eq!(
                defs.specials.len(),
                139,
                "the DOOM II specials for {game:?}"
            );
            assert_eq!(defs.sector_types.len(), 16, "the sector table for {game:?}");
            assert!(
                defs.warnings.first().is_some_and(|w| w.contains("DOOM II")),
                "the warning names the fallback: {:?}",
                defs.warnings
            );
        }
    }

    #[test]
    fn thing_returns_the_matching_definition() {
        let defs = definitions_for(Game::Doom).expect("the embedded table is valid");
        let imp = defs.thing(3001).expect("DOOM has the Imp");
        assert_eq!(imp.name, "Imp", "name");
        assert_eq!(imp.sprite, "TROO", "sprite");
        assert_eq!(imp.frame, 'A', "frame");
        assert_eq!(imp.radius, 20, "radius");
        assert_eq!(imp.category, "Monsters", "category");
        assert!(!imp.hanging, "the Imp does not hang");
        assert_eq!(imp.game, "doom", "first game");
        assert!(
            defs.thing(65).is_none(),
            "Chaingunner is not in the DOOM set"
        );
        assert!(defs.thing(0).is_none(), "type 0 exists in no game");
    }

    #[test]
    fn the_hanging_flag_survives_parsing() {
        let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
        let keen = defs.thing(72).expect("Hanging Keen exists");
        assert!(keen.hanging, "the flags column marked it hanging");
        assert_eq!(keen.game, "doom2", "it is a DOOM II thing");
    }

    #[test]
    fn game_labels_match_the_wire_names() {
        for game in [
            Game::Doom,
            Game::Doom2,
            Game::Heretic,
            Game::Hexen,
            Game::Strife,
            Game::Unknown,
        ] {
            assert_eq!(
                serde_json::to_value(game).expect("games serialize"),
                serde_json::Value::String(game_label(game).to_owned()),
                "warning text and wire name agree for {game:?}"
            );
        }
    }
}
