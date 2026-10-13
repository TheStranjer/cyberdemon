//! The definitions registry: embedded game tables and how to select them.
//!
//! [`definitions_for`] answers one question for a [`Game`]: which thing
//! types can an editor offer for it? The DOOM / DOOM II table is embedded
//! in the binary with `include_str!` and parsed once on first use behind a
//! `LazyLock`; selection then follows the `game` column, which names
//! the first game that has each thing. Heretic and Hexen tables arrive in
//! later tickets; until then those games report empty thing lists with a
//! warning, and `Strife` and [`Game::Unknown`] fall back to the DOOM II set
//! with a warning too.

use std::sync::LazyLock;

use crate::defs::table::{ThingDef, parse_thing_table};
use crate::error::CyberError;
use crate::wad::game::Game;

/// The DOOM / DOOM II thing table, compiled into the binary from
/// `data/defs/doom_things.tsv`.
pub(crate) const DOOM_THINGS_TSV: &str = include_str!("../../data/defs/doom_things.tsv");

/// The `game` column value of things DOOM already has.
const DOOM_ROW: &str = "doom";

/// The `game` column value of things first added in DOOM II.
const DOOM2_ROW: &str = "doom2";

/// The parsed [`DOOM_THINGS_TSV`], produced on first use. A parse failure
/// means the embedded data is broken: it is kept as an error string and
/// reported as [`CyberError::Internal`] by every accessor, never a panic.
static DOOM_THINGS: LazyLock<Result<Vec<ThingDef>, String>> =
    LazyLock::new(|| parse_thing_table(DOOM_THINGS_TSV));

/// Everything the server knows about one game's editing definitions.
///
/// Thing types today; linedef specials and sector types join as later
/// tickets add their tables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definitions {
    /// The game these definitions were selected for.
    pub game: Game,
    /// The thing types the game offers, sorted by type number.
    pub things: Vec<ThingDef>,
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
}

/// The editing definitions for `game`.
///
/// `DOOM` gets the rows whose first game is DOOM; `DOOM II` adds the rows
/// first seen in DOOM II. Heretic and Hexen report an empty thing list and
/// a `"not yet available"` warning; Strife and [`Game::Unknown`] get the
/// DOOM II set with a fallback warning.
///
/// # Errors
///
/// [`CyberError::Internal`] when the embedded table data is malformed,
/// which is a build bug rather than a client problem.
#[inline]
pub fn definitions_for(game: Game) -> Result<Definitions, CyberError> {
    let (things, warnings) = match game {
        Game::Doom => (doom_table(false)?, Vec::new()),
        Game::Doom2 => (doom_table(true)?, Vec::new()),
        Game::Heretic | Game::Hexen => (Vec::new(), vec![not_yet_available(game)]),
        Game::Strife | Game::Unknown => (doom_table(true)?, vec![doom2_fallback(game)]),
    };
    Ok(Definitions {
        game,
        things,
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
fn doom_table(with_doom2: bool) -> Result<Vec<ThingDef>, CyberError> {
    let mut rows: Vec<ThingDef> = embedded_doom_things()?
        .iter()
        .filter(|row| row.game == DOOM_ROW || (with_doom2 && row.game == DOOM2_ROW))
        .cloned()
        .collect();
    rows.sort_by_key(|row| row.kind);
    Ok(rows)
}

/// The parsed rows of [`DOOM_THINGS_TSV`].
///
/// # Errors
///
/// [`CyberError::Internal`] quoting the parse failure of the embedded data.
fn embedded_doom_things() -> Result<&'static [ThingDef], CyberError> {
    DOOM_THINGS.as_deref().map_err(|reason| {
        CyberError::Internal(format!(
            "embedded thing table is malformed (this is a server bug): {reason}"
        ))
    })
}

/// The warning for a game whose own tables have not shipped yet.
fn not_yet_available(game: Game) -> String {
    format!(
        "thing definitions for {} are not yet available; this game's table ships in a later \
         release",
        game_label(game)
    )
}

/// The warning for a game served by the DOOM II fallback table.
fn doom2_fallback(game: Game) -> String {
    format!(
        "no thing definition table for {} yet; showing the DOOM II definitions",
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
    use crate::wad::game::Game;

    /// Whether any row in `defs` has this type number.
    fn has_kind(defs: &Definitions, kind: u32) -> bool {
        defs.things.iter().any(|row| row.kind == kind)
    }

    #[test]
    fn doom_excludes_the_doom_ii_only_things() {
        let defs = definitions_for(Game::Doom).expect("the embedded table is valid");
        assert_eq!(defs.game, Game::Doom, "the game is echoed");
        assert_eq!(defs.things.len(), 97, "the doom rows of the table");
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
    fn heretic_and_hexen_are_empty_with_a_warning() {
        for game in [Game::Heretic, Game::Hexen] {
            let defs = definitions_for(game).expect("selection cannot fail here");
            assert!(defs.things.is_empty(), "{game:?} has no table yet");
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
            let defs = definitions_for(game).expect("the embedded table is valid");
            assert_eq!(defs.game, game, "the requested game is echoed");
            assert_eq!(defs.things.len(), 123, "the DOOM II set for {game:?}");
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
