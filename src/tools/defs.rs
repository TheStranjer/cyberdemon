//! Definitions tools: the game reference tables the server ships with,
//! overlaid with the custom thing definitions of the loaded archives
//! ([`crate::session::Session::custom_sources`] collects the lump texts).

use alloc::collections::BTreeMap;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::defs::registry::game_label;
use crate::defs::{
    Generalized, SectorDecoded, SectorFamily, SectorFlag, SectorTypeDef, SpecialDef, ThingDef,
    decode_generalized_linedef, decode_sector_special, definitions_for, is_generalized,
    session_definitions,
};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::state::AppState;
use crate::tools::reply;
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
    /// Include the thing types the loaded archives define or move:
    /// DEHACKED `Thing` blocks, DECORATE actors, `ZScript` classes and
    /// MAPINFO `DoomEdNums` entries, read from the merged view. Defaults
    /// to true; false answers from the built-in tables alone.
    #[serde(default)]
    include_custom: Option<bool>,
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
    /// Where the row came from: `"builtin"`, `"dehacked"`, `"decorate"`
    /// or `"zscript"`.
    source: String,
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
            source: def.source.clone(),
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
    /// Non-fatal notes: tables not shipped yet, fallback games and what
    /// the custom-definition parsers refused to read.
    warnings: Vec<String>,
}

/// Arguments for [`Cyberdemon::get_linedef_specials`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetLinedefSpecialsParams {
    /// The game to list: `"doom"`, `"doom2"`, `"heretic"` or `"hexen"`.
    /// Defaults to the game detected from the session (`"doom2"` when the
    /// session identifies no game).
    #[serde(default)]
    game: Option<String>,
    /// Which specials table to list: `"doom"` or `"hexen"`. For maps that
    /// use the Hexen action format (binary Hexen maps, UDMF `zdoom`-
    /// namespace maps) inside a DOOM-family session, `"hexen"` requests the
    /// Hexen table with its five named script arguments. Defaults to the
    /// resolved game's own table; `"doom"` on a game that is not DOOM-family
    /// is refused.
    #[serde(default)]
    family: Option<String>,
    /// Keep only specials whose category starts with this text, matched
    /// case-insensitively (e.g. `"Doors"`, or `"Floors/"` for both floor
    /// directions).
    #[serde(default)]
    category: Option<String>,
    /// Keep only specials whose name contains this text, matched
    /// case-insensitively.
    #[serde(default)]
    search: Option<String>,
    /// Look up this special number. Numbers `0x2F80..=0x7FFF` on a
    /// DOOM-family game answer with the decoded Boom generalized action
    /// instead of table rows; other exact numbers filter the table.
    #[serde(default)]
    special: Option<u32>,
}

/// One linedef special as the tool reports it.
#[derive(Serialize)]
struct SpecialEntry {
    /// The special number.
    special: u32,
    /// The editor display name.
    name: String,
    /// The editor category path.
    category: String,
    /// What it acts on: a free-form table hint such as `"sector"`,
    /// `"sector_back"`, `"sector_or_back"`, `"thing"`, `"line"`,
    /// `"ex_5thing"`, `"no"` or `"-"`.
    target: String,
    /// The argument names in order (index 0 is `arg1`); empty for the
    /// Doom-family tables and for Hexen specials that take no arguments.
    args: Vec<String>,
}

impl SpecialEntry {
    /// The report row for one definition row.
    fn of(def: &SpecialDef) -> Self {
        Self {
            special: def.number,
            name: def.name.clone(),
            category: def.category.clone(),
            target: def.target.clone(),
            args: def.args.clone(),
        }
    }
}

/// A decoded Boom generalized linedef special as the tool reports it.
#[derive(Serialize)]
struct GeneralizedEntry {
    /// The action family: `"Floor"`, `"Ceiling"`, `"Door"`,
    /// `"Locked Door"`, `"Lift"`, `"Stairs"` or `"Crusher"`.
    kind: String,
    /// The trigger prefix: `W1`, `WR`, `S1`, `SR`, `G1`, `GR`, `D1`, `DR`.
    trigger: String,
    /// The movement speed: `slow`, `normal`, `fast`, `turbo`.
    speed: String,
    /// The kind-specific bit fields with human-readable values.
    fields: BTreeMap<String, String>,
    /// A one-line description of the action.
    summary: String,
}

impl GeneralizedEntry {
    /// The report row for one decoded generalized value.
    fn of(decoded: Generalized) -> Self {
        Self {
            kind: decoded.kind,
            trigger: decoded.trigger,
            speed: decoded.speed,
            fields: decoded.fields,
            summary: decoded.summary,
        }
    }
}

/// Structured result of [`Cyberdemon::get_linedef_specials`].
#[derive(Serialize)]
struct LinedefSpecialsReport {
    /// The game the rows were selected for.
    game: Game,
    /// The number of table rows returned after filtering.
    count: usize,
    /// The matching specials, sorted by `special` ascending; empty when
    /// `generalized` answers instead.
    specials: Vec<SpecialEntry>,
    /// The decoded Boom generalized action, when the looked-up number is
    /// in the generalized range on a DOOM-family game.
    generalized: Option<GeneralizedEntry>,
    /// Non-fatal notes: tables not shipped yet, fallback games.
    warnings: Vec<String>,
}

/// Arguments for [`Cyberdemon::get_sector_specials`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetSectorSpecialsParams {
    /// The game to list: `"doom"`, `"doom2"`, `"heretic"` or `"hexen"`.
    /// Defaults to the game detected from the session (`"doom2"` when the
    /// session identifies no game).
    #[serde(default)]
    game: Option<String>,
    /// Decode this sector `special` value with the Boom bit layout
    /// (basic type, damage, secret/friction/push flags) instead of only
    /// listing the table.
    #[serde(default)]
    special: Option<u32>,
}

/// One sector type as the tool reports it.
#[derive(Serialize)]
struct SectorTypeEntry {
    /// The sector type value.
    r#type: u32,
    /// The editor display name.
    name: String,
}

impl SectorTypeEntry {
    /// The report row for one definition row.
    fn of(def: &SectorTypeDef) -> Self {
        Self {
            r#type: def.number,
            name: def.name.clone(),
        }
    }
}

/// A decoded Boom sector special value as the tool reports it.
#[derive(Serialize)]
struct SectorDecodedEntry {
    /// The basic type, `special & 31`.
    base: u32,
    /// The display name of the basic type, `"Unknown"` when unassigned.
    base_name: String,
    /// Damage per touch: `"none"`, `"5%"`, `"10%"` or `"20%"`.
    damage: String,
    /// The effect flags set: some of `"secret"`, `"friction"`, `"push"`.
    flags: Vec<SectorFlag>,
}

impl SectorDecodedEntry {
    /// The report row for one decoded sector value.
    fn of(decoded: SectorDecoded) -> Self {
        Self {
            base: decoded.base,
            base_name: decoded.base_name,
            damage: decoded.damage,
            flags: decoded.flags,
        }
    }
}

/// Structured result of [`Cyberdemon::get_sector_specials`].
#[derive(Serialize)]
struct SectorSpecialsReport {
    /// The game the value was selected and decoded for.
    game: Game,
    /// The number of table rows returned.
    count: usize,
    /// The echoed `special` parameter, if any.
    special: Option<u32>,
    /// The Boom decoding of `special`, or null when no `special` was given
    /// or the game is not DOOM-family.
    decoded: Option<SectorDecodedEntry>,
    /// The sector types of the game, sorted by `type` ascending.
    sector_types: Vec<SectorTypeEntry>,
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
    state.with_session(|session| Ok(session.effective_game()))
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

/// The specials table named by the `family` parameter, matched case-
/// insensitively: `true` for the Hexen table, `false` for the DOOM-family
/// one.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the accepted values and the
/// by-game default.
fn parse_family(text: &str) -> Result<bool, CyberError> {
    let lowered = text.to_ascii_lowercase();
    if lowered == "doom" {
        return Ok(false);
    }
    if lowered == "hexen" {
        return Ok(true);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown family {text:?}: pass \"doom\" for the DOOM-family special table or \"hexen\" \
         for the Hexen one, or leave family out to select the table by game"
    )))
}

/// The game whose specials table [`Cyberdemon::get_linedef_specials`]
/// should answer with: the resolved `game`, replaced by `Game::Hexen` when
/// `family` asked for the Hexen table.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `family = "doom"` is requested for
/// a game that is not DOOM-family, since that game's specials are not the
/// DOOM-family ones.
fn specials_table_game(game: Game, family: Option<bool>) -> Result<Game, CyberError> {
    if family == Some(true) {
        return Ok(Game::Hexen);
    }
    if family == Some(false) && !is_doom_family(game) {
        return Err(CyberError::InvalidArgument(format!(
            "family \"doom\" needs a DOOM-family game, not {}: pass game=\"doom\" or \
             game=\"doom2\", use family \"hexen\" for the Hexen table, or leave family out to \
             select by game",
            game_label(game)
        )));
    }
    Ok(game)
}

/// The warning explaining a `family` override: present only when the Hexen
/// table answers for a game that is not Hexen itself.
fn family_note(game: Game, family: Option<bool>) -> Option<String> {
    if family == Some(true) && game != Game::Hexen {
        return Some(format!(
            "family \"hexen\": showing the Hexen linedef special table for the {} game",
            game_label(game)
        ));
    }
    None
}

/// Whether `game` follows the DOOM-family (Boom-compatible) special and
/// sector bit layouts: `DOOM`, `DOOM II`, `Strife` and unidentified
/// sessions. Heretic and Hexen carry their own special sets and are never
/// bit-decoded, even though they are `Doom`-engine games.
const fn is_doom_family(game: Game) -> bool {
    match game {
        Game::Doom | Game::Doom2 | Game::Strife | Game::Unknown => true,
        Game::Heretic | Game::Hexen => false,
    }
}

/// The sector bit-layout family of `game`.
const fn sector_family(game: Game) -> SectorFamily {
    if is_doom_family(game) {
        SectorFamily::Doom
    } else {
        SectorFamily::Other
    }
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

/// Whether `def` passes the special filters: exact number, `category`
/// prefix and `search` name substring, both already lowercased; an absent
/// filter always passes.
fn keeps_special(
    def: &SpecialDef,
    number: Option<u32>,
    category: Option<&str>,
    search: Option<&str>,
) -> bool {
    let number_ok = number.is_none_or(|wanted| def.number == wanted);
    let category_ok =
        category.is_none_or(|prefix| def.category.to_ascii_lowercase().starts_with(prefix));
    let search_ok = search.is_none_or(|needle| def.name.to_ascii_lowercase().contains(needle));
    number_ok && category_ok && search_ok
}

/// The decoded generalized action for a lookup: `Some` exactly when
/// `special` names a generalized number and `game` is DOOM-family.
fn generalized_for(game: Game, special: Option<u32>) -> Option<GeneralizedEntry> {
    let value = special?;
    if !is_doom_family(game) || !is_generalized(value) {
        return None;
    }
    decode_generalized_linedef(value).map(GeneralizedEntry::of)
}

/// `get_thing_types`' work: resolve the game, take its definitions — the
/// built-in tables overlaid with the session's custom sources unless
/// `include_custom` is false — and keep the rows the filters select, in
/// ascending type order.
///
/// # Errors
///
/// [`resolve_game`]'s errors, [`session_definitions`]'s collection error
/// for the merged scope, and their [`CyberError::Internal`] for malformed
/// embedded data.
fn thing_types_report(
    state: &AppState,
    args: &GetThingTypesParams,
) -> Result<ThingTypesReport, CyberError> {
    let game = resolve_game(state, args.game.as_deref())?;
    let defs = if args.include_custom.unwrap_or(true) {
        let (sources, notes) = state.with_session(|session| session.custom_sources())?;
        let mut overlaid = session_definitions(game, &sources)?;
        overlaid.warnings.extend(notes);
        overlaid
    } else {
        definitions_for(game)?
    };
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

/// `get_linedef_specials`' work: a generalized lookup when the number is
/// in the generalized range on a DOOM-family table, otherwise the filtered
/// table rows in ascending special order. The table follows the resolved
/// game unless `family` overrides it.
///
/// # Errors
///
/// [`resolve_game`]'s, [`parse_family`]'s and [`specials_table_game`]'s
/// errors, and [`definitions_for`]'s [`CyberError::Internal`] for malformed
/// embedded data.
fn linedef_specials_report(
    state: &AppState,
    args: &GetLinedefSpecialsParams,
) -> Result<LinedefSpecialsReport, CyberError> {
    let game = resolve_game(state, args.game.as_deref())?;
    let family = args.family.as_deref().map(parse_family).transpose()?;
    let table_game = specials_table_game(game, family)?;
    let defs = definitions_for(table_game)?;
    let generalized = generalized_for(table_game, args.special);
    let specials: Vec<SpecialEntry> = if generalized.is_some() {
        Vec::new()
    } else {
        let category = args.category.as_deref().map(str::to_ascii_lowercase);
        let search = args.search.as_deref().map(str::to_ascii_lowercase);
        defs.specials
            .iter()
            .filter(|def| keeps_special(def, args.special, category.as_deref(), search.as_deref()))
            .map(SpecialEntry::of)
            .collect()
    };
    let count = specials.len();
    let mut warnings = defs.warnings;
    warnings.extend(family_note(game, family));
    Ok(LinedefSpecialsReport {
        game,
        count,
        specials,
        generalized,
        warnings,
    })
}

/// `get_sector_specials`' work: the game's full sector-type table, plus the
/// Boom decoding of `special` when one was given for a DOOM-family game.
///
/// # Errors
///
/// [`resolve_game`]'s errors and [`definitions_for`]'s
/// [`CyberError::Internal`] for malformed embedded data.
fn sector_specials_report(
    state: &AppState,
    args: &GetSectorSpecialsParams,
) -> Result<SectorSpecialsReport, CyberError> {
    let game = resolve_game(state, args.game.as_deref())?;
    let defs = definitions_for(game)?;
    let decoded = args
        .special
        .and_then(|value| decode_sector_special(value, sector_family(game)))
        .map(SectorDecodedEntry::of);
    let sector_types: Vec<SectorTypeEntry> =
        defs.sector_types.iter().map(SectorTypeEntry::of).collect();
    let count = sector_types.len();
    Ok(SectorSpecialsReport {
        game,
        count,
        special: args.special,
        decoded,
        sector_types,
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
        description = "Lists the game's thing types - editor numbers, names, sprites, frames, radii, categories - from the tables embedded in the server, by default overlaid with the session's custom definitions (DEHACKED, DECORATE, ZScript, MAPINFO). Filters: search (name substring), category prefix, exact type. Use it whenever a tool wants a thing type or name: add_thing, place_things, replace_thing_type."
    )]
    fn get_thing_types(
        &self,
        Parameters(args): Parameters<GetThingTypesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(thing_types_report(self.state(), &args))
    }

    /// Lists the linedef specials known for a game, decoding Boom generalized values.
    #[tool(
        description = "Lists the game's special-table linedef actions sorted by special number: number, name, category, what the action targets and (Hexen family) its argument names, from the tables embedded in the server. game and family select the table and search filters names. Read it before writing a special with edit_linedef so the number means what you think."
    )]
    fn get_linedef_specials(
        &self,
        Parameters(args): Parameters<GetLinedefSpecialsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(linedef_specials_report(self.state(), &args))
    }

    /// Lists the sector types known for a game, decoding Boom sector values.
    #[tool(
        description = "Lists the game's sector types (values and names) from the embedded tables, sorted by type; game defaults to the session's detected game. On a Doom-family game, passing special also answers the Boom generalized decoding {base, damage, secret/friction/push flags}. Read it before edit_sector or set_light_effect sets a special."
    )]
    fn get_sector_specials(
        &self,
        Parameters(args): Parameters<GetSectorSpecialsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(sector_specials_report(self.state(), &args))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        detected_game, family_note, is_doom_family, parse_family, parse_game, sector_family,
        specials_table_game,
    };
    use crate::defs::SectorFamily;
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

    #[test]
    fn only_the_doom_family_gets_bit_decodings() {
        for game in [Game::Doom, Game::Doom2, Game::Strife, Game::Unknown] {
            assert!(is_doom_family(game), "{game:?} is DOOM-family");
            assert_eq!(
                sector_family(game),
                SectorFamily::Doom,
                "{game:?} sector bits"
            );
        }
        for game in [Game::Heretic, Game::Hexen] {
            assert!(!is_doom_family(game), "{game:?} is not DOOM-family");
            assert_eq!(
                sector_family(game),
                SectorFamily::Other,
                "{game:?} bits differ"
            );
        }
    }

    #[test]
    fn parse_family_accepts_the_two_names_any_case() {
        assert!(
            parse_family("Hexen").expect("hexen is a family"),
            "hexen names the Hexen table"
        );
        assert!(
            !parse_family("doom").expect("doom is a family"),
            "doom names the DOOM-family table"
        );
    }

    #[test]
    fn parse_family_refuses_anything_else_with_a_hint() {
        let error = parse_family("zdoom").expect_err("zdoom is not a family name");
        assert_eq!(error.code(), "invalid_argument", "unknown family code");
        let message = error.to_string();
        assert!(
            message.contains("\"doom\"")
                && message.contains("\"hexen\"")
                && message.contains("leave family out"),
            "the message names the values and the default: {message}"
        );
    }

    #[test]
    fn the_hexen_family_always_selects_the_hexen_table() {
        for game in [
            Game::Doom,
            Game::Doom2,
            Game::Heretic,
            Game::Hexen,
            Game::Strife,
            Game::Unknown,
        ] {
            assert_eq!(
                specials_table_game(game, Some(true)).expect("an explicit hexen request is served"),
                Game::Hexen,
                "family hexen serves the Hexen table for {game:?}"
            );
            assert_eq!(
                specials_table_game(game, None).expect("the game alone always selects"),
                game,
                "without family the game's own table answers for {game:?}"
            );
        }
    }

    #[test]
    fn family_doom_is_refused_outside_the_doom_family() {
        for game in [Game::Doom, Game::Doom2, Game::Strife, Game::Unknown] {
            assert_eq!(
                specials_table_game(game, Some(false)).expect("DOOM-family accepts family doom"),
                game,
                "family doom keeps the game's own table for {game:?}"
            );
        }
        for game in [Game::Heretic, Game::Hexen] {
            let error = specials_table_game(game, Some(false))
                .expect_err("a non-DOOM-family game has no DOOM-family default");
            assert_eq!(error.code(), "invalid_argument", "refusal code");
            assert!(
                error.to_string().contains("DOOM-family"),
                "the message names the requirement: {error}"
            );
        }
    }

    #[test]
    fn only_a_real_table_override_gets_a_note() {
        assert_eq!(
            family_note(Game::Doom2, Some(true)).expect("doom2 served from the Hexen table"),
            "family \"hexen\": showing the Hexen linedef special table for the doom2 game",
            "the note names the served table and the resolved game"
        );
        assert!(
            family_note(Game::Hexen, Some(true)).is_none(),
            "hexen needs no note"
        );
        assert!(
            family_note(Game::Doom2, None).is_none(),
            "no family, no note"
        );
        assert!(
            family_note(Game::Doom2, Some(false)).is_none(),
            "family doom is the DOOM-family default"
        );
    }
}
