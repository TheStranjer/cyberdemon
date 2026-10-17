//! The embedded theme tables: `data/themes/*.tsv`, parsed once.
//!
//! Each file carries one game family's themes as tab-separated rows of
//! `theme`, `role`, `candidates` (`|`-separated) and `fallback_role`
//! (`-` for none). [`theme_table`] selects a theme for a [`Game`]; a
//! theme that omits a role inherits the game's default theme row for it
//! (reported as [`crate::themes::Matched::Fallback`] by
//! [`super::resolve`]). A broken
//! embedded file is a build bug: every accessor reports it as
//! [`CyberError::Internal`], never a panic.

use std::sync::LazyLock;

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::themes::{Role, ThemeRow, ThemeTable};
use crate::wad::game::Game;

/// The `DOOM` / `DOOM II` theme table, compiled into the binary from
/// `data/themes/doom.tsv`.
const DOOM_TSV: &str = include_str!("../../data/themes/doom.tsv");

/// The Heretic theme table, compiled into the binary from
/// `data/themes/heretic.tsv`.
const HERETIC_TSV: &str = include_str!("../../data/themes/heretic.tsv");

/// The Hexen theme table, compiled into the binary from
/// `data/themes/hexen.tsv`.
const HEXEN_TSV: &str = include_str!("../../data/themes/hexen.tsv");

/// The Strife theme table, compiled into the binary from
/// `data/themes/strife.tsv`.
const STRIFE_TSV: &str = include_str!("../../data/themes/strife.tsv");

/// The header every theme file starts with.
const HEADER: [&str; 4] = ["theme", "role", "candidates", "fallback_role"];

/// The marker column that means "no candidates" / "no fallback".
const NONE: &str = "-";

/// One game's themes: name to rows, rows to their content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameThemes {
    /// Every theme of the file, keyed by its lowercased name.
    pub themes: BTreeMap<String, BTreeMap<Role, ThemeRow>>,
}

/// The parsed [`DOOM_TSV`], with the broken-data policy of
/// [`crate::defs::registry`]: kept as an error string, reported later.
static DOOM: LazyLock<Result<GameThemes, String>> =
    LazyLock::new(|| parse_themes(DOOM_TSV).map_err(|error| error.to_string()));

/// The parsed [`HERETIC_TSV`].
static HERETIC: LazyLock<Result<GameThemes, String>> =
    LazyLock::new(|| parse_themes(HERETIC_TSV).map_err(|error| error.to_string()));

/// The parsed [`HEXEN_TSV`].
static HEXEN: LazyLock<Result<GameThemes, String>> =
    LazyLock::new(|| parse_themes(HEXEN_TSV).map_err(|error| error.to_string()));

/// The parsed [`STRIFE_TSV`].
static STRIFE: LazyLock<Result<GameThemes, String>> =
    LazyLock::new(|| parse_themes(STRIFE_TSV).map_err(|error| error.to_string()));

/// The theme file `game` reads its tables from (`DOOM`, `DOOM II` and the
/// undetected session share one file).
#[must_use]
#[inline]
pub const fn theme_file_game(game: Game) -> Game {
    match game {
        Game::Heretic => Game::Heretic,
        Game::Hexen => Game::Hexen,
        Game::Strife => Game::Strife,
        Game::Doom | Game::Doom2 | Game::Unknown => Game::Doom2,
    }
}

/// The theme a session starts on for `game`.
#[must_use]
#[inline]
pub const fn default_theme_name(game: Game) -> &'static str {
    match theme_file_game(game) {
        Game::Heretic => "castle",
        Game::Hexen => "forest",
        Game::Strife => "urban",
        Game::Doom | Game::Doom2 | Game::Unknown => "tech",
    }
}

/// The parsed tables for `game`.
///
/// # Errors
///
/// [`CyberError::Internal`] when the embedded file is malformed (a build
/// bug).
#[inline]
pub fn game_themes(game: Game) -> Result<&'static GameThemes, CyberError> {
    let parsed = match theme_file_game(game) {
        Game::Heretic => &*HERETIC,
        Game::Hexen => &*HEXEN,
        Game::Strife => &*STRIFE,
        Game::Doom | Game::Doom2 | Game::Unknown => &*DOOM,
    };
    parsed
        .as_ref()
        .map_err(|reason| CyberError::Internal(format!("embedded theme table is broken: {reason}")))
}

/// The theme names `game` offers, in table order.
///
/// # Errors
///
/// [`game_themes`]' [`CyberError::Internal`] for malformed embedded data.
#[inline]
pub fn theme_names(game: Game) -> Result<Vec<String>, CyberError> {
    Ok(game_themes(game)?.themes.keys().cloned().collect())
}

/// The selected theme's table: `wanted` by name (case-insensitively) or
/// the game's default theme, and a name the file does not carry falls back
/// to that default.
///
/// # Errors
///
/// [`game_themes`]' [`CyberError::Internal`] for malformed embedded data.
#[inline]
pub fn theme_table(game: Game, wanted: Option<&str>) -> Result<ThemeTable, CyberError> {
    let themes = game_themes(game)?;
    let default = default_theme_name(game);
    let chosen = wanted
        .filter(|name| {
            themes
                .themes
                .keys()
                .any(|theme| theme.eq_ignore_ascii_case(name))
        })
        .unwrap_or(default)
        .to_ascii_lowercase();
    let rows = themes
        .themes
        .get(&chosen)
        .or_else(|| themes.themes.get(default))
        .cloned()
        .unwrap_or_default();
    let base = if chosen == default || !themes.themes.contains_key(&chosen) {
        None
    } else {
        themes.themes.get(default).cloned()
    };
    Ok(ThemeTable::build(game, chosen, rows, base))
}

/// Parse one theme file.
///
/// # Errors
///
/// [`CyberError::Format`] for a wrong header, a row that is not four
/// columns, an unknown role name, a duplicate `(theme, role)` pair, a role
/// that falls back to itself or a file without themes.
#[inline]
pub fn parse_themes(text: &str) -> Result<GameThemes, CyberError> {
    let mut themes: BTreeMap<String, BTreeMap<Role, ThemeRow>> = BTreeMap::new();
    let mut seen_header = false;
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if !seen_header {
            check_header(line)?;
            seen_header = true;
            continue;
        }
        let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
        if columns.len() != HEADER.len() {
            return Err(CyberError::Format(format!(
                "theme row {number} has {} columns, expected {}: {line}",
                columns.len(),
                HEADER.len()
            )));
        }
        insert_row(&mut themes, number, &columns)?;
    }
    if themes.is_empty() {
        return Err(CyberError::Format(
            "the theme table carries no rows".to_owned(),
        ));
    }
    Ok(GameThemes { themes })
}

/// Rejects a header that is not the documented four columns.
fn check_header(line: &str) -> Result<(), CyberError> {
    let columns: Vec<&str> = line.split('\t').map(str::trim).collect();
    if columns == HEADER {
        return Ok(());
    }
    Err(CyberError::Format(format!(
        "theme table header is {line:?}, expected {:?}",
        HEADER.join("\t")
    )))
}

/// Adds one validated row to the theme map. `columns` holds the four
/// trimmed fields: theme, role, candidates, fallback role.
fn insert_row(
    themes: &mut BTreeMap<String, BTreeMap<Role, ThemeRow>>,
    number: usize,
    columns: &[&str],
) -> Result<(), CyberError> {
    let &[theme, role, candidates, fallback] = columns else {
        return Err(CyberError::Format(format!(
            "theme row {number} is not four columns"
        )));
    };
    if theme.is_empty() {
        return Err(CyberError::Format(format!(
            "theme row {number} has an empty theme"
        )));
    }
    let parsed_role = Role::parse(role)
        .map_err(|error| CyberError::Format(format!("theme row {number}: {error}")))?;
    let fallback_role = if fallback == NONE {
        None
    } else {
        let target = Role::parse(fallback)
            .map_err(|error| CyberError::Format(format!("theme row {number}: {error}")))?;
        if target == parsed_role {
            return Err(CyberError::Format(format!(
                "theme row {number}: role {target} falls back to itself"
            )));
        }
        Some(target)
    };
    let row = ThemeRow {
        candidates: split_candidates(candidates),
        fallback_role,
    };
    let rows = themes.entry(theme.to_ascii_lowercase()).or_default();
    if rows.contains_key(&parsed_role) {
        return Err(CyberError::Format(format!(
            "theme {theme} carries role {parsed_role} twice (row {number})"
        )));
    }
    rows.insert(parsed_role, row);
    Ok(())
}

/// Splits the `|`-separated candidate column (uppercased; `-` is empty).
fn split_candidates(candidates: &str) -> Vec<String> {
    if candidates == NONE {
        return Vec::new();
    }
    candidates
        .split('|')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(str::to_ascii_uppercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use super::{default_theme_name, parse_themes, theme_names, theme_table};
    use crate::themes::{Available, Matched, Role};
    use crate::wad::game::Game;

    /// A minimal valid file body for the failure checks.
    const MINI: &str = "theme\trole\tcandidates\tfallback_role\ntech\twall\tA|B*\t-\n";

    /// The default theme of every game parses and resolves the wall role.
    fn default_resolves_wall(game: Game) {
        let table = theme_table(game, None).expect("the default theme parses");
        assert_eq!(
            table.name(),
            default_theme_name(game),
            "the default by name"
        );
        let mut walls = BTreeSet::new();
        walls.insert("ONLYWALL".to_owned());
        let available = Available {
            walls,
            flats: BTreeSet::default(),
        };
        let hit = super::super::resolve(&table, Role::Wall, &available);
        assert_eq!(
            hit.name, "ONLYWALL",
            "no candidate or default name is in the set, so the first available wins"
        );
        assert_eq!(
            hit.matched,
            Matched::Default,
            "{game:?} falls through to the last resort"
        );
    }

    #[test]
    fn every_games_default_theme_parses() {
        for game in [
            Game::Doom,
            Game::Doom2,
            Game::Heretic,
            Game::Hexen,
            Game::Strife,
        ] {
            default_resolves_wall(game);
        }
    }

    #[test]
    fn the_shipped_files_list_their_documented_themes() {
        assert_eq!(
            theme_names(Game::Doom2).expect("doom parses"),
            ["brick", "hell", "rock", "tech"],
            "the doom themes, alphabetical by table order"
        );
        assert_eq!(
            theme_names(Game::Heretic).expect("heretic parses"),
            ["castle"]
        );
        assert_eq!(theme_names(Game::Hexen).expect("hexen parses"), ["forest"]);
        assert_eq!(theme_names(Game::Strife).expect("strife parses"), ["urban"]);
        assert_eq!(
            theme_names(Game::Unknown).expect("unknown reads doom"),
            ["brick", "hell", "rock", "tech"]
        );
    }

    #[test]
    fn unknown_theme_names_fall_back_to_the_default_theme() {
        let table = theme_table(Game::Doom2, Some("NO-SUCH")).expect("a table comes back");
        assert_eq!(table.name(), "tech", "the unknown name was not in the file");
        let hell = theme_table(Game::Doom2, Some("HELL")).expect("case-insensitive hit");
        assert_eq!(
            hell.name(),
            "hell",
            "and the chosen theme is reported lowercased"
        );
    }

    #[test]
    fn parse_accepts_the_mini_file_and_skips_blank_lines() {
        let parsed = parse_themes(&format!("\n{MINI}\n")).expect("the mini file parses");
        assert_eq!(parsed.themes.len(), 1, "one theme");
        let row = parsed.themes["tech"][&Role::Wall].clone();
        assert_eq!(row.candidates, ["A", "B*"], "the candidate column split");
        assert_eq!(row.fallback_role, None, "a dash is no fallback");
    }

    #[test]
    fn parse_rejects_broken_tables() {
        let three = MINI.replace("tech\twall\tA|B*\t-\n", "tech\twall\tA\n");
        let bad_role = MINI.replace("wall", "roof");
        let bad_fallback = MINI.replace("A|B*\t-", "A|B*\troof");
        let doubled = format!("{MINI}{MINI}");
        let cases = [
            (
                "role\ttheme\tcandidates\tfallback_role\nt\twall\tA\t-\n",
                "header",
            ),
            (three.as_str(), "three columns"),
            (bad_role.as_str(), "unknown role"),
            (bad_fallback.as_str(), "unknown fallback"),
            (doubled.as_str(), "duplicate pair"),
            ("theme\trole\tcandidates\tfallback_role\n", "no rows"),
            (
                "theme\trole\tcandidates\tfallback_role\ntech\twall\tA\twall\n",
                "self fallback",
            ),
            (
                "theme\trole\tcandidates\tfallback_role\textra\nt\tw\tc\tf\te\n",
                "five columns",
            ),
        ];
        for (text, why) in cases {
            let error = parse_themes(text).expect_err(why);
            assert_eq!(error.code(), "format_error", "code for {why}");
        }
    }
}
