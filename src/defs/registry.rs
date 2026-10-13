//! The definitions registry: embedded game tables and how to select them.
//!
//! [`definitions_for`] answers one question for a [`Game`]: what can an
//! editor offer for it? The DOOM / DOOM II thing, linedef-special and
//! sector-type tables are embedded in the binary with `include_str!` and
//! parsed once on first use behind a `LazyLock`; thing and special
//! selection then follows the `game` column, which names the first game
//! that has each entry, while the Doom sector table covers both. Heretic
//! and Hexen tables arrive in later tickets; until then those games report
//! empty lists with a warning, and `Strife` and [`Game::Unknown`] fall back
//! to the DOOM II sets with a warning too.

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
    /// Non-fatal notes: tables that are not available yet, fallbacks taken.
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
/// first seen in DOOM II and shares the sector table. Heretic and Hexen
/// report empty lists and a `"not yet available"` warning; Strife and
/// [`Game::Unknown`] get the DOOM II sets with a fallback warning.
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
        Game::Heretic | Game::Hexen => (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![not_yet_available(game)],
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

/// The warning for a game whose own tables have not shipped yet.
fn not_yet_available(game: Game) -> String {
    format!(
        "definition tables for {} are not yet available; they ship in a later release",
        game_label(game)
    )
}

/// The warning for a game served by the DOOM II fallback tables.
fn doom2_fallback(game: Game) -> String {
    format!(
        "no definition tables for {} yet; showing the DOOM II definitions",
        game_label(game)
    )
}

/// The lowercase game name used in warning text.
const fn game_label(game: Game) -> &'static str {
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
    fn heretic_and_hexen_are_empty_with_a_warning() {
        for game in [Game::Heretic, Game::Hexen] {
            let defs = definitions_for(game).expect("selection cannot fail here");
            assert!(defs.things.is_empty(), "{game:?} has no table yet");
            assert!(defs.specials.is_empty(), "{game:?} has no specials yet");
            assert!(
                defs.sector_types.is_empty(),
                "{game:?} has no sector table yet"
            );
            assert_eq!(defs.warnings.len(), 1, "one warning for {game:?}");
            assert!(
                defs.warnings
                    .first()
                    .is_some_and(|w| w.contains("not yet available")),
                "the warning says what is missing: {:?}",
                defs.warnings
            );
        }
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
