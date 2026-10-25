//! Texture-theme tools: `get_texture_theme`, `set_texture_theme`,
//! `auto_texture` and `align_textures` (see the `themes` router on
//! [`crate::server::Cyberdemon`]).
//!
//! A theme answers *roles* (`wall`, `door_track`, `liquid_floor`, ...)
//! with textures the merged view actually holds; the pure resolution order
//! lives in [`crate::themes`]. [`theme_texture`] is the session-level
//! helper every later high-level tool (drawing, doors, stairs, the plan
//! builder) asks when it needs "a wall texture" without naming one.
//! `auto_texture` and `align_textures` run their pure engines
//! ([`crate::themes::auto`] and [`crate::themes::align`]) as exactly one
//! [`crate::session::Session::edit_map`] step each.

use alloc::collections::{BTreeMap, BTreeSet};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::defs::{decode_generalized_linedef, definitions_for, is_generalized};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::map_edit::MapEditReport;
use crate::session::{Scope, Session};
use crate::themes::align::{self, AlignPlan};
use crate::themes::auto::{self, AutoChange, AutoMode, AutoPlan, ThemeTextures};
use crate::themes::table;
use crate::themes::{self, Available, Matched, Resolved, Role};
use crate::tools::maps::common;
use crate::tools::maps::params::{
    IndexSelector, check_index, edit_result_fragment, validate_texture_name,
};
use crate::tools::reply;
use crate::tools::textures::TextureSet;
use crate::wad::classify::{LumpKind, MapFormat, Namespace};
use crate::wad::game::Game;
use crate::wad::map::Map;
use crate::wad::map::consts::{NO_TEXTURE, SKY_FLAT};
use crate::wad::map::flags::FlagFamily;
use crate::wad::name::LumpName;

/// The `auto_texture` step label prefix.
const AUTO_LABEL: &str = "auto_texture";
/// The `align_textures` step label prefix.
const ALIGN_LABEL: &str = "align_textures";

/// What the merged view provides, precomputed for role resolution. The
/// bulk-edit machinery (`crate::tools::query::replace::Stock`) reads
/// the same two availability sets through [`inventory`].
pub(in crate::tools) struct Inventory {
    /// Existing wall texture and flat names.
    pub(in crate::tools) available: Available,
    /// Wall texture name to width in pixels, from `TEXTURE1`/`TEXTURE2`.
    pub(in crate::tools) widths: BTreeMap<String, u32>,
}

/// The merged view's texture inventory: `TEXTURE1`/`TEXTURE2` names and
/// widths, `TX_` lump names, the flats namespace, plus `F_SKY1`.
/// Shared with the bulk-edit machinery in
/// `crate::tools::query::replace`.
///
/// # Errors
///
/// [`common::directory`]'s `not_found` and [`TextureSet::load`]'s
/// [`CyberError::Format`] for a malformed texture lump.
pub(in crate::tools) fn inventory(session: &Session) -> Result<Inventory, CyberError> {
    let dir = common::directory(session, Scope::Merged)?;
    let mut available = Available::default();
    for (entry, info) in dir.entries.iter().zip(dir.analysis.lumps.iter()) {
        collect_lump(&mut available, info.namespace, info.kind, entry.lump.name);
    }
    available.flats.insert(SKY_FLAT.to_owned());
    let widths = TextureSet::load(session)?
        .widths()
        .into_iter()
        .map(|(name, width)| {
            available.walls.insert(name.clone());
            (name, u32::from(width))
        })
        .collect();
    Ok(Inventory { available, widths })
}

/// Files one merged lump into the availability sets by namespace.
fn collect_lump(
    available: &mut Available,
    namespace: Option<Namespace>,
    kind: LumpKind,
    name: LumpName,
) {
    if kind == LumpKind::Marker {
        return;
    }
    match namespace {
        Some(Namespace::Flats) => {
            available.flats.insert(name.to_string());
        }
        Some(Namespace::Textures) => {
            push_tx_name(&mut available.walls, &name.to_string());
        }
        Some(_) | None => {}
    }
}

/// Adds the texture name a `TX_` lump stands for (`TX_WALL1` names
/// `WALL1`).
fn push_tx_name(walls: &mut BTreeSet<String>, lump: &str) {
    if let Some(texture) = lump.strip_prefix("TX_")
        && !texture.is_empty()
    {
        walls.insert(texture.to_owned());
    }
}

/// The session's theme fully resolved against the merged view.
struct ThemeView {
    /// The effective theme name.
    theme: String,
    /// The theme names the session's game offers.
    available_themes: Vec<String>,
    /// Every role's texture and how it was matched.
    roles: BTreeMap<Role, Resolved>,
}

/// Resolve every role for the session's current theme, with an optional
/// one-call theme-name override (`None` follows the session's choice).
///
/// # Errors
///
/// [`inventory`]'s errors and [`table::theme_names`]' /
/// [`table::theme_table`]' [`CyberError::Internal`] for broken embedded
/// tables.
fn theme_view(session: &Session, theme_override: Option<&str>) -> Result<ThemeView, CyberError> {
    let game = session.game();
    let available_themes = table::theme_names(game)?;
    let wanted = theme_override.or_else(|| session.texture_theme().name.as_deref());
    let selected = table::theme_table(game, wanted)?;
    let stock = inventory(session)?;
    let overrides = &session.texture_theme().overrides;
    let mut roles = BTreeMap::new();
    for role in Role::ALL {
        let resolved = overrides.get(&role).map_or_else(
            || themes::resolve(&selected, role, &stock.available),
            |name| Resolved {
                name: name.clone(),
                matched: Matched::Exact,
            },
        );
        roles.insert(role, resolved);
    }
    Ok(ThemeView {
        theme: selected.name().to_owned(),
        available_themes,
        roles,
    })
}

/// The texture that currently stands for `role` in the session's theme.
///
/// This is the helper high-level tools (drawing, doors, stairs, the plan
/// builder) ask instead of hard-coding a texture name; it never fails on
/// an empty session, it answers `"-"`.
///
/// # Errors
///
/// `theme_view`'s errors: the session-level helper high-level tools
/// use.
#[inline]
pub fn theme_texture(session: &Session, role: Role) -> Result<String, CyberError> {
    let view = theme_view(session, None)?;
    Ok(view
        .roles
        .get(&role)
        .map_or_else(|| "-".to_owned(), |hit| hit.name.clone()))
}

/// The textures standing for `wanted` roles, resolved in one pass against
/// the session's theme or a one-call `theme_override`.
///
/// This is what a tool that needs the whole picture for a single build
/// (`build_map_from_plan` and its [`crate::plan::TextureRoles`] adapter)
/// asks instead of one [`theme_texture`] call per role: the candidate
/// table is resolved once, and a named `theme_override` dresses only
/// this call — the session's theme state never changes. The session's
/// per-role overrides keep winning over the override theme, exactly as
/// they do for [`theme_texture`]. A role the theme cannot answer maps to
/// `"-"`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `theme_override` names a theme the
/// session's game does not offer (listing the ones it does), plus
/// `theme_view`'s errors.
#[inline]
pub fn theme_roles(
    session: &Session,
    theme_override: Option<&str>,
    wanted: &[Role],
) -> Result<BTreeMap<Role, String>, CyberError> {
    if let Some(text) = theme_override {
        check_theme_name(session.game(), text)?;
    }
    let view = theme_view(session, theme_override)?;
    Ok(wanted
        .iter()
        .map(|role| {
            (
                *role,
                view.roles
                    .get(role)
                    .map_or_else(|| "-".to_owned(), |hit| hit.name.clone()),
            )
        })
        .collect())
}

/// One `get_texture_theme` role row.
#[derive(Serialize)]
struct RoleReport {
    /// The texture or flat standing for the role.
    texture: String,
    /// How it was found: `exact`, `glob`, `fallback` or `default`.
    matched: Matched,
}

/// Structured result of [`Cyberdemon::get_texture_theme`] and
/// [`Cyberdemon::set_texture_theme`].
#[derive(Serialize)]
struct ThemeReport {
    /// The effective theme name.
    theme: String,
    /// The themes the session's game offers.
    available_themes: Vec<String>,
    /// Every role's current texture.
    roles: BTreeMap<Role, RoleReport>,
}

/// Fold a view into its wire form.
fn reported_view(view: &ThemeView) -> ThemeReport {
    ThemeReport {
        theme: view.theme.clone(),
        available_themes: view.available_themes.clone(),
        roles: view
            .roles
            .iter()
            .map(|(role, hit)| {
                (
                    *role,
                    RoleReport {
                        texture: hit.name.clone(),
                        matched: hit.matched,
                    },
                )
            })
            .collect(),
    }
}

/// Arguments for [`Cyberdemon::set_texture_theme`].
#[derive(Debug, Deserialize, JsonSchema)]
struct SetThemeParams {
    /// The theme to switch to (case-insensitive); unset keeps the current
    /// choice. An unknown name fails with `invalid_argument` listing the
    /// available themes.
    #[serde(default)]
    theme: Option<String>,
    /// Per-role texture names that win over the theme's candidates. Each
    /// name must exist in the merged view (`not_found` otherwise); `"-"`
    /// removes that role's override.
    #[serde(default)]
    overrides: Option<BTreeMap<String, String>>,
}

/// Validate the request, then apply it to the session's theme.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when neither field is given, the theme
/// is unknown or a role name is unknown; [`CyberError::NotFound`] when an
/// override texture does not exist in the merged view; [`inventory`]'s
/// errors.
fn set_run(session: &mut Session, args: &SetThemeParams) -> Result<ThemeReport, CyberError> {
    if args.theme.is_none() && args.overrides.is_none() {
        return Err(CyberError::InvalidArgument(
            "nothing to change: pass theme, overrides, or both".to_owned(),
        ));
    }
    let game = session.game();
    if let Some(text) = args.theme.as_ref() {
        check_theme_name(game, text)?;
    }
    let stock = inventory(session)?;
    let mut planned: Vec<(Role, Option<String>)> = Vec::new();
    for (key, value) in args.overrides.clone().unwrap_or_default() {
        let role = Role::parse(&key)?;
        let name = validate_texture_name(&value, MapFormat::Doom)?;
        if name != NO_TEXTURE && !stock.available.names_for(role).contains(&name) {
            return Err(CyberError::NotFound(format!(
                "override {role} = {name} does not exist in the merged view: list_textures / \
                 list_flats show what is there (build_texture or the import tools add more); \
                 pass \"-\" to clear this override instead"
            )));
        }
        planned.push((role, (name != NO_TEXTURE).then_some(name)));
    }
    let theme = session.texture_theme_mut();
    if let Some(text) = args.theme.as_ref() {
        theme.name = Some(text.to_ascii_lowercase());
    }
    for (role, value) in planned {
        match value {
            Some(name) => {
                theme.overrides.insert(role, name);
            }
            None => {
                theme.overrides.remove(&role);
            }
        }
    }
    Ok(reported_view(&theme_view(session, None)?))
}

/// Refuse a theme the session's game does not offer.
fn check_theme_name(game: Game, text: &str) -> Result<(), CyberError> {
    let names = table::theme_names(game)?;
    if names.iter().any(|name| name.eq_ignore_ascii_case(text)) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown texture theme {text:?} for this game: available themes are {}",
        names.join(", ")
    )))
}

/// Arguments for [`Cyberdemon::auto_texture`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AutoTextureParams {
    /// The map to dress. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// Sectors to dress (default: every sector of the map). An index the
    /// map does not hold fails with `not_found`.
    #[serde(default)]
    sectors: Option<Vec<usize>>,
    /// `"fill_missing"` (default) only fixes what would render wrong;
    /// `"retexture"` repaints everything in the theme.
    #[serde(default)]
    mode: Option<String>,
    /// Whether sector flats participate (default `true`).
    #[serde(default)]
    include_flats: Option<bool>,
}

/// Structured result of [`Cyberdemon::auto_texture`].
#[derive(Serialize)]
struct AutoReport {
    /// The mode that ran.
    mode: &'static str,
    /// Sidedefs whose texture fields changed.
    changed_sidedefs: usize,
    /// Sectors whose flats changed.
    changed_sectors: usize,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Parse the `mode` parameter.
fn parse_mode(text: Option<&str>) -> Result<AutoMode, CyberError> {
    match text.unwrap_or("fill_missing").to_ascii_lowercase().as_str() {
        "fill_missing" => Ok(AutoMode::FillMissing),
        "retexture" => Ok(AutoMode::Retexture),
        other => Err(CyberError::InvalidArgument(format!(
            "unknown auto_texture mode {other:?}: \"fill_missing\" fixes what would render \
             wrong, \"retexture\" repaints the selection with the theme"
        ))),
    }
}

/// The sectors to dress: the request's list or every sector of the map.
fn selected_sectors(map: &Map, requested: Option<&[usize]>) -> Result<BTreeSet<usize>, CyberError> {
    let Some(indices) = requested else {
        return Ok((0..map.sectors.len()).collect());
    };
    if indices.is_empty() {
        return Err(CyberError::InvalidArgument(
            "the sectors list is empty: pass at least one index or omit it to dress every \
             sector"
                .to_owned(),
        ));
    }
    for index in indices {
        check_index("sector", *index, map.sectors.len())?;
    }
    Ok(indices.iter().copied().collect())
}

/// Resolve the theme textures the engine paints with.
///
/// # Errors
///
/// [`theme_texture`]'s errors.
fn paint_set(session: &Session) -> Result<ThemeTextures, CyberError> {
    Ok(ThemeTextures {
        wall: theme_texture(session, Role::Wall)?,
        door: theme_texture(session, Role::Door)?,
        door_track: theme_texture(session, Role::DoorTrack)?,
        step: theme_texture(session, Role::Step)?,
        floor: theme_texture(session, Role::Floor)?,
        ceiling: theme_texture(session, Role::Ceiling)?,
    })
}

/// The linedefs of `map` carrying a door action of the session's game.
///
/// # Errors
///
/// [`definitions_for`]' [`CyberError::Internal`] for a broken embedded
/// specials table.
fn door_lines(session: &Session, map: &Map) -> Result<BTreeSet<usize>, CyberError> {
    let specials = door_specials(session.game())?;
    Ok(map
        .linedefs
        .iter()
        .enumerate()
        .filter(|&(_index, line)| is_door_action(line.special, map.flag_family(), &specials))
        .map(|(index, _line)| index)
        .collect())
}

/// The special numbers the game's tables categorise as doors.
fn door_specials(game: Game) -> Result<BTreeSet<u32>, CyberError> {
    let defs = definitions_for(game)?;
    Ok(defs
        .specials
        .iter()
        .filter(|row| row.category.starts_with("Doors"))
        .map(|row| row.number)
        .collect())
}

/// Whether `special` moves a door: a table row, or (in the `Doom` family)
/// a Boom generalized door.
fn is_door_action(special: i32, family: FlagFamily, table: &BTreeSet<u32>) -> bool {
    let Ok(value) = u32::try_from(special) else {
        return false;
    };
    if value == 0 {
        return false;
    }
    if table.contains(&value) {
        return true;
    }
    family == FlagFamily::Doom
        && is_generalized(value)
        && decode_generalized_linedef(value)
            .is_some_and(|row| row.kind == "Door" || row.kind == "Locked Door")
}

/// `auto_texture`'s work: resolve, plan, run one undoable edit.
///
/// # Errors
///
/// [`common::resolve_map_name`] / [`common::load_map`] refusals,
/// `invalid_argument` for a bad mode or empty sector list, `not_found` for
/// a sector the map lacks, and [`Session::edit_map`]'s errors.
fn auto_run(session: &mut Session, args: &AutoTextureParams) -> Result<AutoReport, CyberError> {
    let mode = parse_mode(args.mode.as_deref())?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let loaded = common::load_map(session, name, Scope::Merged)?;
    let stock = inventory(session)?;
    let plan = AutoPlan {
        mode,
        include_flats: args.include_flats.unwrap_or(true),
        selected: selected_sectors(&loaded, args.sectors.as_deref())?,
        door_lines: door_lines(session, &loaded)?,
        textures: paint_set(session)?,
        available: &stock.available,
    };
    let label = format!("{AUTO_LABEL} {name}");
    let (change, report) = session.edit_map(name, &label, |map| Ok(auto::apply(map, &plan)))?;
    Ok(assembled_auto(mode, change, &report))
}

/// Fold the engine's counts and the edit fragment into the wire report.
fn assembled_auto(mode: AutoMode, change: AutoChange, report: &MapEditReport) -> AutoReport {
    AutoReport {
        mode: mode_label(mode),
        changed_sidedefs: change.changed_sidedefs,
        changed_sectors: change.changed_sectors,
        edit: edit_result_fragment(report),
    }
}

/// The mode's wire name.
const fn mode_label(mode: AutoMode) -> &'static str {
    match mode {
        AutoMode::FillMissing => "fill_missing",
        AutoMode::Retexture => "retexture",
    }
}

/// Arguments for [`Cyberdemon::align_textures`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AlignTexturesParams {
    /// The map to align. Defaults like `auto_texture`'s `map`.
    #[serde(default)]
    map: Option<String>,
    /// The linedefs to consider: one index or a list. Mutually exclusive
    /// with `sector`; unset considers the whole map.
    #[serde(default)]
    linedefs: Option<IndexSelector>,
    /// Consider every linedef touching this sector. Mutually exclusive
    /// with `linedefs`.
    #[serde(default)]
    sector: Option<usize>,
    /// Rewrite `x_offset` along the wall chains (default `true`).
    #[serde(default)]
    x: Option<bool>,
    /// Rewrite `y_offset` so the texture top pegs to the chain's highest
    /// ceiling (default `false`).
    #[serde(default)]
    y: Option<bool>,
}

/// Structured result of [`Cyberdemon::align_textures`].
#[derive(Serialize)]
struct AlignReport {
    /// Sidedefs whose offsets changed.
    aligned: usize,
    /// Chains the pass could not align (and why).
    warnings: Vec<String>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// `align_textures`' work: pick the lines, run one undoable edit.
///
/// # Errors
///
/// `invalid_argument` for both selectors, for neither axis, or an empty
/// `linedefs` list; [`common`]'s map refusals; `not_found` for a linedef
/// or sector index the map lacks; [`Session::edit_map`]'s errors.
fn align_run(session: &mut Session, args: &AlignTexturesParams) -> Result<AlignReport, CyberError> {
    let do_x = args.x.unwrap_or(true);
    let do_y = args.y.unwrap_or(false);
    if !do_x && !do_y {
        return Err(CyberError::InvalidArgument(
            "nothing to align: pass x and/or y".to_owned(),
        ));
    }
    if args.linedefs.is_some() && args.sector.is_some() {
        return Err(CyberError::InvalidArgument(
            "pass linedefs or sector, not both".to_owned(),
        ));
    }
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let loaded = common::load_map(session, name, Scope::Merged)?;
    let targets = target_lines(&loaded, args)?;
    let stock = inventory(session)?;
    let plan = AlignPlan {
        targets,
        widths: &stock.widths,
        do_x,
        do_y,
    };
    let label = format!("{ALIGN_LABEL} {name}");
    let (change, report) = session.edit_map(name, &label, |map| Ok(align::apply(map, &plan)))?;
    Ok(AlignReport {
        aligned: change.aligned,
        warnings: change.warnings,
        edit: edit_result_fragment(&report),
    })
}

/// The linedefs the pass considers, validated against `map`.
fn target_lines(map: &Map, args: &AlignTexturesParams) -> Result<BTreeSet<usize>, CyberError> {
    if let Some(selector) = args.linedefs.as_ref() {
        let indices = selector.clone().indices()?;
        for index in &indices {
            check_index("linedef", *index, map.linedefs.len())?;
        }
        return Ok(indices.into_iter().collect());
    }
    if let Some(sector) = args.sector {
        check_index("sector", sector, map.sectors.len())?;
        return Ok(side_lines(map, sector));
    }
    Ok((0..map.linedefs.len()).collect())
}

/// The linedefs with a side opening into `sector`.
fn side_lines(map: &Map, sector: usize) -> BTreeSet<usize> {
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(_index, line)| {
            [line.front, line.back]
                .into_iter()
                .flatten()
                .any(|side| map.sidedefs.get(side).is_some_and(|sd| sd.sector == sector))
        })
        .map(|(index, _line)| index)
        .collect()
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = themes_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reads the session's texture theme and every role's current texture.
    #[tool(
        description = "Reads the session's texture theme: the effective theme name, the themes the detected game offers, and every role (wall, floor, ceiling, door, door_track, switch, exit_switch, step, lift, window, teleport, liquid and the rest) with the texture standing for it now and how it matched. Use it before set_texture_theme; build_map_from_plan and auto_texture resolve roles against this table."
    )]
    fn get_texture_theme(&self) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| Ok(reported_view(&theme_view(session, None)?))),
        )
    }

    /// Switches the theme and/or edits the per-role overrides.
    #[tool(
        description = "Sets the session's texture theme, its per-role overrides, or both: theme names validate against get_texture_theme's available list (case-insensitive) and overrides {role: texture} answer a single role from now on (an empty value clears it). Theme state is session state - undo does not roll it back. Use it to steer what auto_texture, the feature tools and build_map_from_plan dress with."
    )]
    fn set_texture_theme(
        &self,
        Parameters(args): Parameters<SetThemeParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(self.state().with_session(|session| set_run(session, &args)))
    }

    /// Fills missing textures or repaints a map with the theme.
    #[tool(
        description = "Dresses a map (or the listed sectors) with the session's texture theme, as one undoable edit: mode fill_missing (default) gives untextured middles, visible upper gaps and lower steps the theme's wall, door or step texture; mode retexture redresses every face, keeping door and switch specials. Returns per-role change counts. Use it after building or swapping a theme; align_textures then removes the seams."
    )]
    fn auto_texture(
        &self,
        Parameters(args): Parameters<AutoTextureParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| auto_run(session, &args)),
        )
    }

    /// Runs wall textures continuously across corners.
    #[tool(
        description = "Aligns wall textures along chains of connected linedefs that carry the same texture on their front side, as one undoable edit: x (default true) rewrites each x_offset so the picture runs unbroken around corners, and y pegs every member's texture top to the chain's highest ceiling. Use it after auto_texture or a repaint leaves seams running along a wall."
    )]
    fn align_textures(
        &self,
        Parameters(args): Parameters<AlignTexturesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(
            self.state()
                .with_session(|session| align_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use serde_json::from_str;

    use super::{
        AlignTexturesParams, AutoTextureParams, SetThemeParams, is_door_action, mode_label,
        parse_mode, push_tx_name, theme_roles,
    };
    use crate::session::Session;
    use crate::themes::auto::AutoMode;
    use crate::themes::{Role, table};
    use crate::wad::map::FlagFamily;

    /// The plain `Doom` door specials for the action checks.
    fn specials() -> BTreeSet<u32> {
        [1_u32, 26, 27, 28].into_iter().collect()
    }

    #[test]
    fn modes_parse_case_insensitively_and_default_to_fill() {
        assert_eq!(parse_mode(None).expect("default"), AutoMode::FillMissing);
        assert_eq!(
            parse_mode(Some("Retexture")).expect("case fold"),
            AutoMode::Retexture
        );
        assert_eq!(
            parse_mode(Some("repaint"))
                .expect_err("no such mode")
                .code(),
            "invalid_argument",
            "a bad mode names the good ones"
        );
        assert_eq!(
            mode_label(AutoMode::FillMissing),
            "fill_missing",
            "wire name"
        );
    }

    #[test]
    fn door_actions_cover_table_rows_and_generalized_doors() {
        assert!(is_door_action(1, FlagFamily::Doom, &specials()), "DR Door");
        assert!(
            is_door_action(0x3C00_i32 + 16, FlagFamily::Doom, &specials()),
            "a generalized door bit pattern"
        );
        assert!(
            !is_door_action(18, FlagFamily::Doom, &specials()),
            "a pillar build is not a door"
        );
        assert!(
            !is_door_action(0, FlagFamily::Doom, &specials()),
            "no special, no action"
        );
        assert!(
            !is_door_action(0x3C00 + 16, FlagFamily::Hexen, &specials()),
            "the generalized layout belongs to the Doom family"
        );
    }

    #[test]
    fn tx_lumps_contribute_their_stripped_names() {
        let mut walls = BTreeSet::new();
        push_tx_name(&mut walls, "TX_WALL9");
        push_tx_name(&mut walls, "TX_");
        push_tx_name(&mut walls, "WALLP1");
        assert_eq!(walls.len(), 1, "only the real TX_ name lands");
        assert!(walls.contains("WALL9"), "stripped of its TX_ prefix");
    }

    #[test]
    fn parameter_json_matches_the_documented_shapes() {
        let auto: AutoTextureParams =
            from_str(r#"{"map": "MAP01", "sectors": [0, 3], "mode": "retexture"}"#)
                .expect("auto args");
        assert_eq!(auto.sectors.as_deref(), Some([0_usize, 3].as_slice()));
        let align: AlignTexturesParams =
            from_str(r#"{"linedefs": 7, "y": true}"#).expect("align args, one index");
        assert!(align.x.is_none(), "x defaults on");
        let theme: SetThemeParams =
            from_str(r#"{"overrides": {"wall": "WALL1", "step": "-"}}"#).expect("set args");
        assert_eq!(theme.overrides.expect("map").len(), 2, "two overrides");
    }

    #[test]
    fn theme_roles_answers_one_row_per_wanted_role() {
        let session = Session::default();
        let wanted = [Role::Wall, Role::Floor];
        let resolved = theme_roles(&session, None, &wanted).expect("an empty session answers");
        assert_eq!(resolved.len(), 2, "one row per role");
        assert_eq!(
            resolved.get(&Role::Wall).expect("wall"),
            "-",
            "an empty merged view resolves to the no-texture name"
        );
        assert_eq!(
            resolved.get(&Role::Floor).expect("floor"),
            "F_SKY1",
            "F_SKY1 is always in the flats inventory, so it is the only existing flat"
        );
        let none_wanted = theme_roles(&session, None, &[]).expect("nothing asked");
        assert!(none_wanted.is_empty(), "nothing answered");
    }

    #[test]
    fn a_theme_override_dresses_one_call_without_touching_the_session() {
        let session = Session::default();
        let game_themes = table::theme_names(session.game()).expect("the table reads");
        let other = game_themes
            .iter()
            .find(|name| name.as_str() != "tech")
            .expect("the doom table offers more than one theme");
        let wanted = [Role::Floor];
        let dressed = theme_roles(&session, Some(&other.to_ascii_uppercase()), &wanted)
            .expect("overrides are case-insensitive");
        assert_eq!(dressed.len(), 1, "the named theme resolves");
        assert_eq!(
            session.texture_theme().name,
            None,
            "the session never adopted the override theme"
        );
        let broken =
            theme_roles(&session, Some("nope"), &wanted).expect_err("an unknown theme is refused");
        assert_eq!(broken.code(), "invalid_argument", "unknown theme code");
        for name in &game_themes {
            assert!(broken.to_string().contains(name), "lists {name}");
        }
    }
}
