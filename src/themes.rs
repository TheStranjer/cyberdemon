//! Texture themes: role-based texture resolution over the merged view.
//!
//! A theme answers one question for a *role* (`wall`, `door_track`,
//! `liquid_floor`, ...): which texture in the session should stand for it?
//! [`table`] embeds the per-game candidate lists (`data/themes/*.tsv`);
//! [`resolve`] walks one theme in a deterministic order — exact candidate
//! names first, then glob patterns, then the role's `fallback_role` chain,
//! then the game's default texture and finally the alphabetically first
//! existing texture or flat of the right kind. [`auto`] and [`align`] are
//! the pure map engines behind the `auto_texture` and `align_textures`
//! tools. The outward API (the `themes` tool group) lives in
//! [`crate::tools::themes`]; [`crate::session::Session`] carries the
//! selected [`TextureTheme`] as session state, outside the undo history.
//!
//! Wall roles resolve against the merged `TEXTURE1`/`TEXTURE2` definitions
//! plus `TX_` texture lumps; flat roles against the flats namespace, with
//! `F_SKY1` always available.

pub mod align;
pub mod auto;
pub mod table;

use core::fmt::{self, Display, Formatter};

use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::wad::game::Game;
use crate::wad::name::glob_match;

/// The number of `fallback_role` hops one resolution may walk before it is
/// treated as broken data (real tables chain at most two deep; the budget
/// only exists to make a cycle in the data a miss rather than a hang).
pub(crate) const FALLBACK_BUDGET: usize = 8;

/// What a high-level tool can ask a theme for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The primary wall surface.
    Wall,
    /// A contrasting wall surface.
    WallAlt,
    /// The default floor flat.
    Floor,
    /// The default ceiling flat.
    Ceiling,
    /// A ceiling that reads as lit.
    LightCeiling,
    /// The ceiling of outdoor spaces.
    OutdoorCeiling,
    /// A door's moving middle texture.
    Door,
    /// The static track beside a door.
    DoorTrack,
    /// The track beside a red-key door.
    DoorTrackRed,
    /// The track beside a blue-key door.
    DoorTrackBlue,
    /// The track beside a yellow-key door.
    DoorTrackYellow,
    /// The flat inside a doorway.
    DoorFloor,
    /// A switch panel texture.
    Switch,
    /// The switch that ends the map.
    ExitSwitch,
    /// A step or short wall band.
    Step,
    /// The visible side of a lift.
    LiftSide,
    /// The flat on top of a lift.
    LiftFloor,
    /// The sill under a window.
    WindowSill,
    /// The flat of a teleporter pad.
    TeleportFloor,
    /// The flat of a liquid (slime, water, lava, blood).
    LiquidFloor,
}

impl Role {
    /// Every role, in theme-table order.
    pub const ALL: [Self; 20] = [
        Self::Wall,
        Self::WallAlt,
        Self::Floor,
        Self::Ceiling,
        Self::LightCeiling,
        Self::OutdoorCeiling,
        Self::Door,
        Self::DoorTrack,
        Self::DoorTrackRed,
        Self::DoorTrackBlue,
        Self::DoorTrackYellow,
        Self::DoorFloor,
        Self::Switch,
        Self::ExitSwitch,
        Self::Step,
        Self::LiftSide,
        Self::LiftFloor,
        Self::WindowSill,
        Self::TeleportFloor,
        Self::LiquidFloor,
    ];

    /// The wire name of this role, as the tables and JSON spell it.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wall => "wall",
            Self::WallAlt => "wall_alt",
            Self::Floor => "floor",
            Self::Ceiling => "ceiling",
            Self::LightCeiling => "light_ceiling",
            Self::OutdoorCeiling => "outdoor_ceiling",
            Self::Door => "door",
            Self::DoorTrack => "door_track",
            Self::DoorTrackRed => "door_track_red",
            Self::DoorTrackBlue => "door_track_blue",
            Self::DoorTrackYellow => "door_track_yellow",
            Self::DoorFloor => "door_floor",
            Self::Switch => "switch",
            Self::ExitSwitch => "exit_switch",
            Self::Step => "step",
            Self::LiftSide => "lift_side",
            Self::LiftFloor => "lift_floor",
            Self::WindowSill => "window_sill",
            Self::TeleportFloor => "teleport_floor",
            Self::LiquidFloor => "liquid_floor",
        }
    }

    /// Parses a role name case-insensitively.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] listing the valid roles when `text`
    /// names none of them.
    #[inline]
    pub fn parse(text: &str) -> Result<Self, CyberError> {
        Self::ALL
            .iter()
            .copied()
            .find(|role| role.as_str().eq_ignore_ascii_case(text))
            .ok_or_else(|| unknown_role(text))
    }

    /// Whether this role wants a flat rather than a wall texture.
    #[must_use]
    #[inline]
    pub const fn is_flat(self) -> bool {
        matches!(
            self,
            Self::Floor
                | Self::Ceiling
                | Self::LightCeiling
                | Self::OutdoorCeiling
                | Self::DoorFloor
                | Self::LiftFloor
                | Self::TeleportFloor
                | Self::LiquidFloor
        )
    }

    /// Whether this role names a ceiling surface (which the game default
    /// resolves differently from a floor surface).
    #[must_use]
    #[inline]
    pub const fn is_ceiling(self) -> bool {
        matches!(
            self,
            Self::Ceiling | Self::LightCeiling | Self::OutdoorCeiling
        )
    }

    /// Every role name, for error messages.
    #[must_use]
    #[inline]
    pub fn names() -> Vec<&'static str> {
        Self::ALL.iter().copied().map(Self::as_str).collect()
    }
}

impl Display for Role {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The refusal naming the roles a parameter accepts.
fn unknown_role(text: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "unknown texture role {text:?}: expected one of {}",
        Role::names().join(", ")
    ))
}

/// How a resolved texture name was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Matched {
    /// An exact candidate name existed in the session.
    Exact,
    /// A glob candidate matched (the alphabetically first match wins).
    Glob,
    /// The name came from another role's row (a `fallback_role` chain or
    /// the game's default theme).
    Fallback,
    /// The game's default texture, the alphabetically first existing one,
    /// or `-` when the session holds none.
    Default,
}

/// One resolution answer: the texture name and how it was found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Resolved {
    /// The texture or flat name to use (`"-"` when nothing exists).
    pub name: String,
    /// Which rule produced `name`.
    pub matched: Matched,
}

/// One row of a theme table: the candidates for a role and its fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeRow {
    /// Candidate patterns in table order: exact names and globs (`*`, `?`).
    pub candidates: Vec<String>,
    /// The role to consult when every candidate misses.
    pub fallback_role: Option<Role>,
}

/// A theme selected from one game's table, ready to resolve against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeTable {
    /// The game whose default textures finish a failed resolution.
    game: Game,
    /// The selected theme's name.
    name: String,
    /// This theme's rows.
    rows: BTreeMap<Role, ThemeRow>,
    /// The game's default theme rows, consulted for roles this theme does
    /// not carry (`None` for the default theme itself).
    base: Option<BTreeMap<Role, ThemeRow>>,
}

impl ThemeTable {
    /// Builds a table: `game` for the final defaults, `name` for reporting,
    /// `rows` for this theme, and `base` for roles `rows` does not carry.
    #[must_use]
    #[inline]
    pub const fn build(
        game: Game,
        name: String,
        rows: BTreeMap<Role, ThemeRow>,
        base: Option<BTreeMap<Role, ThemeRow>>,
    ) -> Self {
        Self {
            game,
            name,
            rows,
            base,
        }
    }

    /// The game this table belongs to.
    #[must_use]
    #[inline]
    pub const fn game(&self) -> Game {
        self.game
    }

    /// The theme's name.
    #[must_use]
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The texture names a session actually provides, split by kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Available {
    /// Composite texture names (`TEXTURE1`/`TEXTURE2` and `TX_` lumps).
    pub walls: BTreeSet<String>,
    /// Flat names (the flats namespace plus `F_SKY1`).
    pub flats: BTreeSet<String>,
}

impl Available {
    /// The set a role resolves against.
    #[must_use]
    #[inline]
    pub const fn names_for(&self, role: Role) -> &BTreeSet<String> {
        if role.is_flat() {
            &self.flats
        } else {
            &self.walls
        }
    }

    /// Whether `name` exists as a wall texture.
    #[must_use]
    #[inline]
    pub fn has_wall(&self, name: &str) -> bool {
        self.walls.contains(name)
    }

    /// Whether `name` exists as a flat.
    #[must_use]
    #[inline]
    pub fn has_flat(&self, name: &str) -> bool {
        self.flats.contains(name)
    }
}

/// Resolve `role` in `table` against the textures `available` provides.
///
/// The order is fixed: the role's exact candidates in table order, then
/// its glob candidates (each taking the alphabetically first match), then
/// the `fallback_role` chain, then the game's default theme rows for the
/// same role, then the game's default texture for the role's kind
/// (wall/floor/ceiling), then the alphabetically first existing texture or
/// flat. When the session has nothing of the right kind at all the name is
/// `"-"`.
#[must_use]
#[inline]
pub fn resolve(table: &ThemeTable, role: Role, available: &Available) -> Resolved {
    if let Some(hit) = resolve_in(&table.rows, role, available) {
        return hit;
    }
    if let Some(base) = table.base.as_ref() {
        if let Some(hit) = resolve_in(base, role, available) {
            return Resolved {
                name: hit.name,
                matched: Matched::Fallback,
            };
        }
    }
    default_resolution(table.game, role, available)
}

/// The theme's own rows for `role`, walking the `fallback_role` chain.
pub(crate) fn resolve_in(
    rows: &BTreeMap<Role, ThemeRow>,
    role: Role,
    available: &Available,
) -> Option<Resolved> {
    let mut current = role;
    let mut stepped = false;
    for _step in 0..=FALLBACK_BUDGET {
        let row = rows.get(&current)?;
        if let Some((name, matched)) = candidates_of(row, current, available) {
            return Some(Resolved {
                name,
                matched: fell_back(matched, stepped),
            });
        }
        current = row.fallback_role?;
        stepped = true;
    }
    None
}

/// A candidate found after at least one fallback hop always reports as a
/// fallback, whatever the row itself matched on.
#[must_use]
const fn fell_back(matched: Matched, stepped: bool) -> Matched {
    if stepped { Matched::Fallback } else { matched }
}

/// The exact-then-glob candidate pass over one row.
fn candidates_of(row: &ThemeRow, role: Role, available: &Available) -> Option<(String, Matched)> {
    let names = available.names_for(role);
    if let Some(hit) = row
        .candidates
        .iter()
        .filter(|pattern| !is_glob(pattern))
        .find(|candidate| names.contains(candidate.as_str()))
    {
        return Some((hit.clone(), Matched::Exact));
    }
    row.candidates
        .iter()
        .filter(|pattern| is_glob(pattern))
        .find_map(|pattern| {
            names
                .iter()
                .find(|name| glob_match(name.as_bytes(), pattern))
                .cloned()
        })
        .map(|name| (name, Matched::Glob))
}

/// Whether a candidate pattern carries glob characters.
#[must_use]
pub(crate) fn is_glob(pattern: &str) -> bool {
    pattern
        .as_bytes()
        .iter()
        .any(|&byte| byte == b'*' || byte == b'?')
}

/// The game's last-resort texture names: `(wall, floor, ceiling)`.
#[must_use]
#[inline]
pub const fn game_default_names(game: Game) -> (&'static str, &'static str, &'static str) {
    match game {
        Game::Heretic => ("GRSTNPB", "FLOOR03", "FLAT506"),
        Game::Hexen => ("FOREST01", "F_010", "F_011"),
        Game::Strife => ("BRKGRY17", "F_NOLINE", "F_DECK"),
        Game::Doom | Game::Doom2 | Game::Unknown => ("STARTAN", "FLOOR0_1", "CEIL1_1"),
    }
}

/// The game's default texture name for one role.
#[must_use]
#[inline]
pub const fn game_default_name(game: Game, role: Role) -> &'static str {
    let (wall, floor, ceiling) = game_default_names(game);
    if !role.is_flat() {
        return wall;
    }
    if role.is_ceiling() { ceiling } else { floor }
}

/// The final fallbacks: the game default, then the alphabetically first
/// existing texture or flat of the right kind, then `-`.
fn default_resolution(game: Game, role: Role, available: &Available) -> Resolved {
    let wanted = game_default_name(game, role);
    let names = available.names_for(role);
    if names.contains(wanted) {
        return Resolved {
            name: wanted.to_owned(),
            matched: Matched::Default,
        };
    }
    Resolved {
        name: names
            .iter()
            .next()
            .cloned()
            .unwrap_or_else(|| "-".to_owned()),
        matched: Matched::Default,
    }
}

/// The session's texture theme: the selected table plus per-role overrides.
///
/// Session state like [`crate::render::RenderSettings`]: no edit, undo or
/// replaced archive changes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextureTheme {
    /// The chosen theme name; `None` follows the session game's default
    /// theme (`tech` for the `DOOM` games, `castle` for Heretic, ...).
    pub name: Option<String>,
    /// Names that win over any candidate, keyed by role.
    pub overrides: BTreeMap<Role, String>,
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use super::{
        Available, Matched, Resolved, Role, ThemeRow, ThemeTable, candidates_of, game_default_name,
        resolve,
    };
    use crate::wad::game::Game;

    /// A specification row for the test-table builder.
    type RowSpec = (Role, Vec<&'static str>, Option<Role>);

    /// Builds an availability set from name lists (all uppercased).
    fn available(walls: &[&str], flats: &[&str]) -> Available {
        Available {
            walls: walls.iter().map(|name| (*name).to_owned()).collect(),
            flats: flats.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    /// A one-theme table over the given rows.
    fn table(rows: Vec<RowSpec>) -> ThemeTable {
        ThemeTable::build(
            Game::Doom2,
            "tech".to_owned(),
            rows.into_iter()
                .map(|(role, candidates, fallback)| {
                    (
                        role,
                        ThemeRow {
                            candidates: candidates.into_iter().map(str::to_owned).collect(),
                            fallback_role: fallback,
                        },
                    )
                })
                .collect(),
            None,
        )
    }

    #[test]
    fn exact_candidates_win_over_globs_in_table_order() {
        let rows = vec![(Role::Wall, vec!["B_WALL", "*_WALL"], None)];
        let avail = available(&["A_WALL", "B_WALL"], &[]);
        assert_eq!(
            resolve(&table(rows), Role::Wall, &avail),
            Resolved {
                name: "B_WALL".to_owned(),
                matched: Matched::Exact,
            },
            "the listed exact name beats an earlier alphabetical glob hit"
        );
    }

    #[test]
    fn globs_take_the_alphabetically_first_available_name() {
        let rows = vec![(Role::Wall, vec!["NOPE", "*_WALL"], None)];
        let avail = available(&["Z_WALL", "C_WALL"], &[]);
        assert_eq!(
            resolve(&table(rows.clone()), Role::Wall, &avail).matched,
            Matched::Glob,
            "only the glob can hit"
        );
        assert_eq!(
            resolve(&table(rows.clone()), Role::Wall, &avail).name,
            "C_WALL",
            "the first match in order"
        );
    }

    #[test]
    fn fallback_role_reports_itself_as_a_fallback() {
        let rows = vec![
            (Role::Wall, vec!["WALLX"], None),
            (Role::DoorTrack, vec!["DOORTRAK"], None),
            (
                Role::DoorTrackYellow,
                vec!["DOORYEL"],
                Some(Role::DoorTrack),
            ),
        ];
        let avail = available(&["DOORTRAK"], &[]);
        assert_eq!(
            resolve(&table(rows), Role::DoorTrackYellow, &avail),
            Resolved {
                name: "DOORTRAK".to_owned(),
                matched: Matched::Fallback,
            },
            "the missing yellow name falls to the door_track row"
        );
    }

    #[test]
    fn a_missing_theme_row_uses_the_default_theme_as_a_fallback() {
        let own = table(vec![(Role::Wall, vec!["HELLWAL"], None)]).rows;
        let base = BTreeMap::from([(
            Role::Ceiling,
            ThemeRow {
                candidates: vec!["CEILA".to_owned()],
                fallback_role: None,
            },
        )]);
        let selected = ThemeTable::build(Game::Doom2, "hell".to_owned(), own, Some(base));
        let avail = available(&["HELLWAL"], &["CEILA"]);
        assert_eq!(
            resolve(&selected, Role::Wall, &avail).matched,
            Matched::Exact,
            "own rows come first"
        );
        assert_eq!(
            resolve(&selected, Role::Ceiling, &avail),
            Resolved {
                name: "CEILA".to_owned(),
                matched: Matched::Fallback,
            },
            "the default theme finishes what the theme omits"
        );
    }

    #[test]
    fn game_defaults_then_first_available_then_dash() {
        let rows = vec![(Role::Floor, vec!["NOTHING*"], None)];
        let with_default = available(&[], &["CEIL1_1", "FLOOR0_1"]);
        assert_eq!(
            resolve(&table(rows.clone()), Role::Floor, &with_default),
            Resolved {
                name: "FLOOR0_1".to_owned(),
                matched: Matched::Default,
            },
            "the doom floor default"
        );
        let without = available(&[], &["AAA", "ZZZ"]);
        assert_eq!(
            resolve(&table(rows.clone()), Role::Floor, &without).name,
            "AAA",
            "the alphabetically first flat when the default is missing"
        );
        let empty = Available::default();
        assert_eq!(
            resolve(&table(rows.clone()), Role::Wall, &empty),
            Resolved {
                name: "-".to_owned(),
                matched: Matched::Default,
            },
            "nothing available at all"
        );
    }

    #[test]
    fn default_names_follow_the_role_kind() {
        assert_eq!(game_default_name(Game::Doom2, Role::Wall), "STARTAN");
        assert_eq!(game_default_name(Game::Doom2, Role::LiftFloor), "FLOOR0_1");
        assert_eq!(
            game_default_name(Game::Heretic, Role::LightCeiling),
            "FLAT506"
        );
        assert_eq!(game_default_name(Game::Hexen, Role::Floor), "F_010");
        assert_eq!(game_default_name(Game::Strife, Role::Ceiling), "F_DECK");
        assert_eq!(game_default_name(Game::Unknown, Role::Wall), "STARTAN");
    }

    #[test]
    fn a_fallback_chain_past_its_budget_is_a_miss() {
        let rows = vec![
            (Role::Wall, vec!["NOPE"], Some(Role::WallAlt)),
            (Role::WallAlt, vec!["NOPE"], Some(Role::Wall)),
        ];
        let avail = available(&["ONLYWALL"], &[]);
        assert_eq!(
            resolve(&table(rows.clone()), Role::Wall, &avail).name,
            "ONLYWALL",
            "the wall<->wall_alt cycle ends and the default path runs"
        );
    }

    #[test]
    fn candidate_pass_is_public_for_the_tool_layer() {
        let row = ThemeRow {
            candidates: vec!["SW1*".to_owned()],
            fallback_role: None,
        };
        let avail = available(&["SW1GARG", "BARRIER"], &[]);
        assert_eq!(
            candidates_of(&row, Role::Switch, &avail),
            Some(("SW1GARG".to_owned(), Matched::Glob)),
            "the glob finds the switch lookalike"
        );
    }

    #[test]
    fn role_names_parse_case_insensitively_and_reject_noise() {
        assert_eq!(
            Role::parse("DOOR_TRACK_Red").expect("mixed case"),
            Role::DoorTrackRed
        );
        assert_eq!(Role::parse("floor").expect("flat role"), Role::Floor);
        assert_eq!(
            Role::parse("roof").expect_err("no roof role").code(),
            "invalid_argument",
            "an unknown role refuses"
        );
        assert_eq!(Role::Wall.as_str(), "wall", "wire names are snake_case");
        assert!(
            Role::LiquidFloor.is_flat() && !Role::Switch.is_flat(),
            "flat split"
        );
        assert!(
            Role::OutdoorCeiling.is_ceiling() && !Role::DoorFloor.is_ceiling(),
            "ceiling split"
        );
    }
}
