//! Definitions tools: the game reference tables the server ships with.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::defs::{ThingDef, definitions_for};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::state::AppState;
use crate::tools::{fail, ok};
use crate::wad::game::Game;

/// Arguments for [`Cyberdemon::get_thing_types`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetThingTypesParams {
    /// The game to list: `"doom"`, `"doom2"`, `"heretic"` or `"hexen"`.
    /// Defaults to the game detected from the session (`"doom2"` when the
    /// session identifies no game).
    #[serde(default)]
    game: Option<String>,
    /// Keep only things whose category starts with this text, matched
    /// case-insensitively (e.g. `"Monsters"`, or `"Gore/"` for every gore
    /// category).
    #[serde(default)]
    category: Option<String>,
    /// Keep only things whose name contains this text, matched
    /// case-insensitively.
    #[serde(default)]
    search: Option<String>,
    /// Keep only this thing type number. An unknown type returns an empty
    /// list, not an error.
    #[serde(default)]
    r#type: Option<u32>,
}

/// One thing type as the tool reports it.
#[derive(Serialize)]
struct ThingEntry {
    /// The thing type number.
    r#type: u32,
    /// The editor display name.
    name: String,
    /// The four-character sprite name, empty when the thing has none.
    sprite: String,
    /// The sprite frame shown by default.
    frame: char,
    /// The blocking radius in map units.
    radius: u32,
    /// The editor category path.
    category: String,
    /// Whether the thing hangs from the ceiling.
    hanging: bool,
}

impl ThingEntry {
    /// The report row for one definition row.
    fn of(def: &ThingDef) -> Self {
        Self {
            r#type: def.kind,
            name: def.name.clone(),
            sprite: def.sprite.clone(),
            frame: def.frame,
            radius: def.radius,
            category: def.category.clone(),
            hanging: def.hanging,
        }
    }
}

/// Structured result of [`Cyberdemon::get_thing_types`].
#[derive(Serialize)]
struct ThingTypesReport {
    /// The game the rows were selected for.
    game: Game,
    /// The number of things returned after filtering.
    count: usize,
    /// The matching thing types, sorted by `type` ascending.
    things: Vec<ThingEntry>,
    /// Non-fatal notes: tables not shipped yet, fallback games.
    warnings: Vec<String>,
}

/// The game named by the `game` parameter, matched case-insensitively;
/// anything outside the four supported names is refused with a hint.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the accepted values and the
/// session-default escape.
fn parse_game(text: &str) -> Result<Game, CyberError> {
    let lowered = text.to_ascii_lowercase();
    if lowered == "doom" {
        return Ok(Game::Doom);
    }
    if lowered == "doom2" {
        return Ok(Game::Doom2);
    }
    if lowered == "heretic" {
        return Ok(Game::Heretic);
    }
    if lowered == "hexen" {
        return Ok(Game::Hexen);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown game {text:?}: pass \"doom\", \"doom2\", \"heretic\" or \"hexen\", or leave \
         game out to use the game detected from the session"
    )))
}

/// The game to default to: the session's detected game, with an
/// unidentified session standing for DOOM II.
///
/// # Errors
///
/// [`AppState::with_session`]'s [`CyberError::Internal`] on a poisoned
/// session lock.
fn detected_game(state: &AppState) -> Result<Game, CyberError> {
    let found = state.with_session(|session| Ok(session.game()))?;
    Ok(if found == Game::Unknown {
        Game::Doom2
    } else {
        found
    })
}

/// The game to serve: the parsed `game` parameter, or the session default.
///
/// # Errors
///
/// [`parse_game`]'s or [`detected_game`]'s errors.
fn resolve_game(state: &AppState, text: Option<&str>) -> Result<Game, CyberError> {
    let Some(raw) = text else {
        return detected_game(state);
    };
    parse_game(raw)
}

/// Whether `def` passes the filters: exact type number `kind`, `category`
/// prefix and `search` name substring, both already lowercased; an absent
/// filter always passes.
fn keeps(def: &ThingDef, kind: Option<u32>, category: Option<&str>, search: Option<&str>) -> bool {
    let type_ok = kind.is_none_or(|wanted| def.kind == wanted);
    let category_ok =
        category.is_none_or(|prefix| def.category.to_ascii_lowercase().starts_with(prefix));
    let search_ok = search.is_none_or(|needle| def.name.to_ascii_lowercase().contains(needle));
    type_ok && category_ok && search_ok
}

/// `get_thing_types`' work: resolve the game, take its embedded table and
/// keep the rows the filters select, in ascending type order.
///
/// # Errors
///
/// [`resolve_game`]'s errors and [`definitions_for`]'s
/// [`CyberError::Internal`] for malformed embedded data.
fn thing_types_report(
    state: &AppState,
    args: &GetThingTypesParams,
) -> Result<ThingTypesReport, CyberError> {
    let game = resolve_game(state, args.game.as_deref())?;
    let defs = definitions_for(game)?;
    let category = args.category.as_deref().map(str::to_ascii_lowercase);
    let search = args.search.as_deref().map(str::to_ascii_lowercase);
    let things: Vec<ThingEntry> = defs
        .things
        .iter()
        .filter(|def| keeps(def, args.r#type, category.as_deref(), search.as_deref()))
        .map(ThingEntry::of)
        .collect();
    let count = things.len();
    Ok(ThingTypesReport {
        game,
        count,
        things,
        warnings: defs.warnings,
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = defs_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists the thing types known for a game.
    #[tool(
        description = "Lists the game's thing types (editor numbers, names, sprites, frames, radii, categories, hanging flags) sorted by type, from the definition tables embedded in the server. game is \"doom\"|\"doom2\"|\"heretic\"|\"hexen\", defaulting to the session's detected game (DOOM II when none is identified). Optional filters: category (case-insensitive prefix, e.g. \"Monsters\"), search (case-insensitive name substring) and type (exact number; unknown types return an empty list, not an error). Returns {game, count, things, warnings}; the heretic and hexen tables are not shipped yet and report empty with a warning."
    )]
    fn get_thing_types(
        &self,
        Parameters(args): Parameters<GetThingTypesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match thing_types_report(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{detected_game, parse_game};
    use crate::state::AppState;
    use crate::wad::game::Game;

    #[test]
    fn parse_game_accepts_the_four_supported_names_any_case() {
        for (text, wanted) in [
            ("doom", Game::Doom),
            ("DOOM2", Game::Doom2),
            ("Heretic", Game::Heretic),
            ("hexen", Game::Hexen),
        ] {
            assert_eq!(
                parse_game(text).expect("a supported name"),
                wanted,
                "{text} maps to its game"
            );
        }
    }

    #[test]
    fn parse_game_refuses_anything_else_with_a_hint() {
        let error = parse_game("strife").expect_err("strife is not a supported game yet");
        assert_eq!(error.code(), "invalid_argument", "unknown game code");
        let message = error.to_string();
        assert!(
            message.contains("\"doom\"") && message.contains("\"hexen\""),
            "the message names the accepted values: {message}"
        );
    }

    #[test]
    fn detected_game_defaults_an_empty_session_to_doom_ii() {
        let state = AppState::default();
        assert_eq!(
            detected_game(&state).expect("a fresh lock is never poisoned"),
            Game::Doom2,
            "an unidentified session stands for DOOM II"
        );
    }
}
