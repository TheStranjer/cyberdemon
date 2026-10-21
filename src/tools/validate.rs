//! Validate tools: [`Cyberdemon::validate_map`], the map health report.
//!
//! The checks themselves are the pure [`crate::wad::map::validate`]
//! module's; this file only resolves the parameters, reads the map, the
//! session's game definitions and the merged view's resource index under
//! one lock, and applies the severity filter, the paging and the counting
//! the client asked about. Later validation tickets add checks to
//! [`crate::wad::map::validate::CHECKS`] without touching anything here.
//! The [`Tables`], [`resource_index`] and [`Counts`] helpers are shared
//! with `build_map_from_plan`'s `finalize` pass.

use alloc::collections::BTreeSet;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::defs::{Definitions, definitions_for, session_definitions};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::defs::collect;
use crate::tools::maps::common;
use crate::tools::textures::TextureSet;
use crate::tools::{fail, ok};
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::game::Game;
use crate::wad::map::validate::{self, Context, Issue, ResourceIndex, Severity};
use crate::wad::name::LumpName;

/// Default row cap for the issues list.
const DEFAULT_LIMIT: usize = 500;
/// Largest row cap the issues list accepts.
const MAX_LIMIT: usize = 5000;

/// Arguments for [`Cyberdemon::validate_map`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ValidateMapParams {
    /// The map to validate, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// Which archive to read: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
    /// Keep issues of this severity or higher: `"info"` (the default),
    /// `"warning"` or `"error"`, case-insensitive.
    #[serde(default)]
    min_severity: Option<String>,
    /// Run only these checks, by snake-case code (e.g.
    /// `["no_player1_start", "duplicate_line"]`); the default runs every
    /// check. An unknown code fails with `invalid_argument` listing the
    /// valid ones.
    #[serde(default)]
    codes: Option<Vec<String>>,
    /// Maximum issues returned (default 500, at most 5000); `truncated`
    /// says whether the filter left more behind. `counts` always totals
    /// everything the checks found.
    #[serde(default)]
    limit: Option<usize>,
}

/// The definition tables one validation run consults, owned so the
/// [`Context`] can borrow them all. Shared with `build_map_from_plan`'s
/// `finalize` pass.
pub(in crate::tools) struct Tables {
    /// The session's definitions: the detected game's tables with the
    /// loaded archives' custom things and specials overlaid (what the
    /// thing and `Doom`-family special checks consult).
    definitions: Definitions,
    /// The `Hexen` specials table: what `Hexen`-family maps check their
    /// specials against whatever game the session detected.
    hexen_specials: Definitions,
}

impl Tables {
    /// The validation context borrowing both tables and the resource
    /// index the caller read from the merged view.
    pub(in crate::tools) const fn context<'res>(
        &'res self,
        resources: &'res ResourceIndex,
    ) -> Context<'res> {
        Context {
            definitions: &self.definitions,
            hexen_specials: &self.hexen_specials,
            resources,
        }
    }
}

/// The game a validation run treats the session as: the detected one, with
/// an unidentified session standing for DOOM II (as the defs tools do).
fn detected_game(session: &Session) -> Game {
    let found = session.game();
    if found == Game::Unknown {
        return Game::Doom2;
    }
    found
}

/// Read `session`'s validation tables: the detected game's definitions
/// overlaid with the merged view's DEHACKED, DECORATE, `ZScript` and
/// MAPINFO customs, plus the built-in `Hexen` specials. Shared with
/// `build_map_from_plan`'s `finalize` pass.
///
/// # Errors
///
/// [`collect::custom_sources`]'s read errors and [`session_definitions`]'
/// and [`definitions_for`]' refusals for malformed definition data.
pub(in crate::tools) fn session_tables(session: &Session) -> Result<Tables, CyberError> {
    let (sources, _notes) = collect::custom_sources(session)?;
    Ok(Tables {
        definitions: session_definitions(detected_game(session), &sources)?,
        hexen_specials: definitions_for(Game::Hexen)?,
    })
}

/// Adds the texture name a `TX_` lump stands for (`TX_WALL1` names
/// `WALL1`) and records that the merged view defines textures.
fn push_tx_lump(index: &mut ResourceIndex, lump: &str) {
    if let Some(texture) = lump.strip_prefix("TX_")
        && !texture.is_empty()
    {
        index.textures.insert(texture.to_owned());
        index.has_texture_defs = true;
    }
}

/// The merged view's resource index: what the texture and flat checks
/// look names up against. The textures are the merged `TEXTURE1` and
/// `TEXTURE2` entries plus the `TX_` names of the textures namespace,
/// and `has_texture_defs` records whether either source exists; the
/// flats are the merged flats-namespace lump names.
///
/// # Errors
///
/// [`common::directory`]'s errors and [`TextureSet::load`]'s
/// [`CyberError::Format`] for a malformed texture lump. Shared with
/// `build_map_from_plan`'s `finalize` pass.
pub(in crate::tools) fn resource_index(session: &Session) -> Result<ResourceIndex, CyberError> {
    let dir = common::directory(session, Scope::Merged)?;
    let mut index = ResourceIndex::default();
    for (entry, info) in dir.entries.iter().zip(dir.analysis.lumps.iter()) {
        let raw = entry.lump.name.as_bytes();
        if raw == b"TEXTURE1" || raw == b"TEXTURE2" {
            index.has_texture_defs = true;
        }
        if info.kind == LumpKind::Marker {
            continue;
        }
        let text = entry.lump.name.to_string();
        match info.namespace {
            Some(Namespace::Flats) => {
                index.flats.insert(text);
            }
            Some(Namespace::Textures) => push_tx_lump(&mut index, &text),
            Some(_) | None => {}
        }
    }
    index
        .textures
        .extend(TextureSet::load(session)?.widths().into_keys());
    Ok(index)
}

/// Which archive to read: the parsed `scope`, the merged view by default.
///
/// # Errors
///
/// [`Scope::parse`]'s [`CyberError::InvalidArgument`] for any other name.
fn parse_scope(text: Option<&str>) -> Result<Scope, CyberError> {
    text.map_or(Ok(Scope::Merged), Scope::parse)
}

/// The severity floor `min_severity` names, [`Severity::Info`] (keep
/// everything) when unset.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the three accepted values.
fn parse_min_severity(text: Option<&str>) -> Result<Severity, CyberError> {
    let lowered = match text {
        None => return Ok(Severity::Info),
        Some(given) => given.to_ascii_lowercase(),
    };
    match lowered.as_str() {
        "" | "info" => Ok(Severity::Info),
        "warning" => Ok(Severity::Warning),
        "error" => Ok(Severity::Error),
        other => Err(CyberError::InvalidArgument(format!(
            "min_severity {other:?} is not a severity: pass \"error\", \"warning\" or \
             \"info\", or leave the parameter out to keep every issue"
        ))),
    }
}

/// The checks `codes` selects: `None` (run everything) when unset.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] quoting every unknown code and listing
/// the valid ones.
fn parse_codes(wanted: Option<&[String]>) -> Result<Option<BTreeSet<&'static str>>, CyberError> {
    let Some(texts) = wanted else {
        return Ok(None);
    };
    let known = validate::codes();
    let mut selected: BTreeSet<&'static str> = BTreeSet::new();
    let mut unknown: Vec<&str> = Vec::new();
    for text in texts {
        match known
            .iter()
            .find(|code| code.eq_ignore_ascii_case(text.as_str()))
        {
            Some(code) => {
                selected.insert(*code);
            }
            None => unknown.push(text.as_str()),
        }
    }
    if !unknown.is_empty() {
        return Err(CyberError::InvalidArgument(format!(
            "unknown check code(s) {} for validate_map: valid codes are {}, or leave codes \
             out to run every check",
            unknown.join(", "),
            known.join(", "),
        )));
    }
    Ok(Some(selected))
}

/// The row cap `limit` asks for, [`DEFAULT_LIMIT`] when unset.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `0` or anything above
/// [`MAX_LIMIT`].
fn parse_limit(value: Option<usize>) -> Result<usize, CyberError> {
    let Some(limit) = value else {
        return Ok(DEFAULT_LIMIT);
    };
    if limit == 0 || limit > MAX_LIMIT {
        return Err(CyberError::InvalidArgument(format!(
            "limit {limit} is out of range: pass a value between 1 and {MAX_LIMIT}"
        )));
    }
    Ok(limit)
}

/// How many issues of each severity the enabled checks found. Shared
/// with `build_map_from_plan`'s `finalize` pass.
#[derive(Debug, Default, Serialize)]
pub(in crate::tools) struct Counts {
    /// The [`Severity::Error`] rows: the map is broken.
    error: usize,
    /// The [`Severity::Warning`] rows: the map is probably wrong.
    warning: usize,
    /// The [`Severity::Info`] rows: the map deserves a look.
    info: usize,
}

impl Counts {
    /// The totals of `issues` by severity.
    pub(in crate::tools) fn of(issues: &[Issue]) -> Self {
        let count = |severity: Severity| {
            issues
                .iter()
                .filter(|issue| issue.severity == severity)
                .count()
        };
        Self {
            error: count(Severity::Error),
            warning: count(Severity::Warning),
            info: count(Severity::Info),
        }
    }
}

/// Structured result of [`Cyberdemon::validate_map`].
#[derive(Debug, Serialize)]
struct ValidationReport {
    /// The map that was validated.
    map: LumpName,
    /// How many issues of each severity the checks found, before the
    /// severity filter and the limit.
    counts: Counts,
    /// The issues at or above `min_severity`, at most `limit` of them, in
    /// check order.
    issues: Vec<Issue>,
    /// Whether the limit dropped issues the severity filter kept.
    truncated: bool,
    /// The codes of the checks that ran, in report order.
    checks_run: Vec<&'static str>,
}

/// Assembles the client-facing report: totals over everything the checks
/// found, the issue list filtered by severity and paged at the limit.
fn viewed(
    map: LumpName,
    outcome: validate::Outcome,
    min_severity: Severity,
    limit: usize,
) -> ValidationReport {
    let counts = Counts::of(&outcome.issues);
    let checks_run = outcome.checks_run;
    let mut issues: Vec<Issue> = outcome
        .issues
        .into_iter()
        .filter(|issue| issue.severity.passes(min_severity))
        .collect();
    let truncated = issues.len() > limit;
    issues.truncate(limit);
    ValidationReport {
        map,
        counts,
        issues,
        truncated,
        checks_run,
    }
}

/// `validate_map`'s work: resolve and check the parameters before the
/// session is touched, then read the map, the definitions and the
/// resource index under one lock and run the selected checks.
///
/// # Errors
///
/// The parameter parsers' [`CyberError::InvalidArgument`] refusals, and
/// under the lock [`common::resolve_map_name`]'s, [`common::load_map`]'s,
/// [`session_tables`]' and [`resource_index`]' errors.
fn validate_run(
    state: &AppState,
    args: &ValidateMapParams,
) -> Result<ValidationReport, CyberError> {
    let min_severity = parse_min_severity(args.min_severity.as_deref())?;
    let enabled = parse_codes(args.codes.as_deref())?;
    let limit = parse_limit(args.limit)?;
    let scope = parse_scope(args.scope.as_deref())?;
    state.with_session(|session| {
        let name = common::resolve_map_name(session, args.map.as_deref())?;
        let map = common::load_map(session, name, scope)?;
        let tables = session_tables(session)?;
        let resources = resource_index(session)?;
        let outcome = validate::run(&map, &tables.context(&resources), enabled.as_ref());
        Ok(viewed(name, outcome, min_severity, limit))
    })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = validate_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reports a map's structural problems as severity-ranked issues.
    #[tool(
        description = "Checks one map for reference, structure, thing, special, geometry, resource and node-lump problems - dangling indices, zero-length and crossing lines, missing starts, unknown specials and types, ceiling below floor, missing textures and flats, stale nodes - each reported as an issue {severity (error|warning|info), code, message, element indices}. min_severity, codes and limit filter and page; counts answer before the filter. Run it after every meaningful change."
    )]
    fn validate_map(
        &self,
        Parameters(args): Parameters<ValidateMapParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match validate_run(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Counts, DEFAULT_LIMIT, MAX_LIMIT, ValidateMapParams, detected_game, parse_codes,
        parse_limit, parse_min_severity, parse_scope, resource_index, validate_run, viewed,
    };
    use crate::defs::definitions_for;
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::state::AppState;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::game::Game;
    use crate::wad::map::validate::{self, Context, ResourceIndex, Severity};
    use crate::wad::map::{Map, Sector};
    use crate::wad::name::LumpName;
    use crate::wad::textures::{TextureDef, TextureLayout, TextureLump, write_texture_lump};

    /// A `validate_map` call over just these parameter texts.
    fn args(map: Option<&str>, scope: Option<&str>) -> ValidateMapParams {
        ValidateMapParams {
            map: map.map(str::to_owned),
            scope: scope.map(str::to_owned),
            min_severity: None,
            codes: None,
            limit: None,
        }
    }

    #[test]
    fn parse_min_severity_accepts_the_three_names_and_defaults_to_info() {
        assert_eq!(
            parse_min_severity(None).expect("absent"),
            Severity::Info,
            "the default keeps everything"
        );
        assert_eq!(
            parse_min_severity(Some("")).expect("empty"),
            Severity::Info,
            "an empty name is the default"
        );
        assert_eq!(
            parse_min_severity(Some("Error")).expect("error"),
            Severity::Error,
            "case-insensitive"
        );
        assert_eq!(
            parse_min_severity(Some("warning")).expect("warning"),
            Severity::Warning,
            "and warning too"
        );
        let error = parse_min_severity(Some("fatal")).expect_err("not a severity");
        assert_eq!(error.code(), "invalid_argument", "bad severity code");
        let message = error.to_string();
        assert!(
            message.contains("\"error\"")
                && message.contains("\"warning\"")
                && message.contains("\"info\""),
            "the message names all three values: {message}"
        );
    }

    #[test]
    fn parse_codes_selects_the_named_checks_case_insensitively() {
        assert!(
            parse_codes(None).expect("absent").is_none(),
            "unset runs all"
        );
        let wanted = vec!["NO_PLAYER1_START".to_owned(), "duplicate_line".to_owned()];
        let selected = parse_codes(Some(&wanted))
            .expect("two known codes")
            .expect("a selection");
        assert_eq!(
            selected.into_iter().collect::<Vec<&'static str>>(),
            vec!["duplicate_line", "no_player1_start"],
            "the static codes, sorted by the set"
        );
        let empty = parse_codes(Some(&Vec::new()))
            .expect("an empty list selects nothing")
            .expect("still a selection");
        assert!(empty.is_empty(), "no codes, no checks");
    }

    #[test]
    fn parse_codes_refuses_unknown_names_with_the_valid_list() {
        let wanted = vec!["nope".to_owned(), "also_nope".to_owned()];
        let error = parse_codes(Some(&wanted)).expect_err("neither code exists");
        assert_eq!(error.code(), "invalid_argument", "unknown code");
        let message = error.to_string();
        assert!(
            message.contains("nope") && message.contains("also_nope"),
            "the offenders are quoted: {message}"
        );
        assert!(
            message.contains("vertex_out_of_range")
                && message.contains("tag_without_sector")
                && message.contains("leave codes out"),
            "the message lists valid codes and the default: {message}"
        );
    }

    #[test]
    fn parse_limits_pages_the_report_between_one_and_five_thousand() {
        assert_eq!(parse_limit(None).expect("absent"), DEFAULT_LIMIT);
        assert_eq!(parse_limit(Some(1)).expect("one"), 1);
        assert_eq!(parse_limit(Some(MAX_LIMIT)).expect("max"), MAX_LIMIT);
        for bad in [0, MAX_LIMIT.checked_add(1).expect("small")] {
            let error = parse_limit(Some(bad)).expect_err("this cap is out of range");
            assert_eq!(error.code(), "invalid_argument", "bad limit code");
            assert!(
                error.to_string().contains("between 1 and 5000"),
                "the message names the range: {error}"
            );
        }
    }

    #[test]
    fn parse_scope_defaults_to_the_merged_view() {
        assert_eq!(parse_scope(None).expect("absent"), Scope::Merged);
        assert_eq!(
            parse_scope(Some("WORKING")).expect("working"),
            Scope::Working,
            "case-insensitive like every scope"
        );
        let error = parse_scope(Some("nope")).expect_err("not a scope");
        assert_eq!(error.code(), "invalid_argument", "bad scope code");
    }

    #[test]
    fn counts_total_every_severity_of_the_found_issues() {
        let doom = definitions_for(Game::Doom2).expect("the embedded Doom II table is valid");
        let hexen = definitions_for(Game::Hexen).expect("the embedded Hexen table is valid");
        let resources = ResourceIndex::default();
        let context = Context {
            definitions: &doom,
            hexen_specials: &hexen,
            resources: &resources,
        };
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        let outcome = validate::run(&map, &context, None);
        let counts = Counts::of(&outcome.issues);
        assert_eq!(counts.error, 1, "only the missing player 1 start");
        assert_eq!(
            counts.warning, 2,
            "the orphaned sector and the absent node tree"
        );
        assert_eq!(
            counts.info, 3,
            "the coop note, the missing blockmap and the skipped texture check"
        );
        let none = Counts::of(&[]);
        assert_eq!((none.error, none.warning, none.info), (0, 0, 0), "empty");
    }

    #[test]
    fn detected_game_falls_back_to_doom_ii_only_when_unidentified() {
        let mut session = Session::default();
        assert_eq!(
            detected_game(&session),
            Game::Doom2,
            "no archives: DOOM II rows"
        );
        session.set_iwad(layer_named(&["PLAYPAL"]));
        assert_eq!(
            detected_game(&session),
            Game::Doom2,
            "an unidentifiable IWAD also stands for DOOM II"
        );
        session.set_iwad(layer_named(&["TITLE", "WINNOWR"]));
        assert_eq!(detected_game(&session), Game::Hexen, "a detected game wins");
    }

    /// A base layer whose directory holds exactly `names`.
    fn layer_named(names: &[&str]) -> Layer {
        let lumps: Vec<Lump> = names
            .iter()
            .map(|text| Lump::marker(LumpName::parse(text).expect("test names are valid")))
            .collect();
        Layer::new(
            std::path::PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        )
    }

    /// A base layer whose directory holds exactly `lumps`.
    fn layer_with(lumps: Vec<Lump>) -> Layer {
        Layer::new(
            std::path::PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        )
    }

    /// A parsed test name.
    fn parsed(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A session whose only layer holds `lumps`.
    fn session_over(lumps: Vec<Lump>) -> Session {
        let mut session = Session::default();
        session.set_iwad(layer_with(lumps));
        session
    }

    /// A valid `TEXTURE1` body defining exactly `names`.
    fn texture_lump(names: &[&str]) -> Vec<u8> {
        let lump = TextureLump {
            layout: TextureLayout::Doom,
            textures: names
                .iter()
                .map(|name| TextureDef {
                    name: (*name).to_owned(),
                    flags_raw: 0,
                    width: 64,
                    height: 64,
                    patches: Vec::new(),
                })
                .collect(),
        };
        write_texture_lump(&lump).expect("test texture lumps serialize")
    }

    #[test]
    fn resource_index_collects_flats_tx_names_and_defined_textures() {
        let session = session_over(vec![
            Lump::marker(parsed("F_START")),
            Lump::new(parsed("FLOOR1"), vec![1_u8; 64]),
            Lump::marker(parsed("F_END")),
            Lump::marker(parsed("TX_START")),
            Lump::new(parsed("TX_WIDE"), vec![2_u8; 8]),
            Lump::marker(parsed("TX_END")),
            Lump::new(parsed("TEXTURE1"), texture_lump(&["WALL1", "STEP1"])),
        ]);
        let index = resource_index(&session).expect("the fixture view indexes");
        assert!(index.has_texture_defs, "a TEXTURE1 lump defines textures");
        assert_eq!(
            index.flats.into_iter().collect::<Vec<String>>(),
            vec!["FLOOR1".to_owned()],
            "the flat region's one lump"
        );
        assert_eq!(
            index.textures.into_iter().collect::<Vec<String>>(),
            vec!["STEP1".to_owned(), "WALL1".to_owned(), "WIDE".to_owned()],
            "the defined records plus the TX_ name, sorted"
        );
    }

    #[test]
    fn resource_index_reports_no_texture_definitions_without_the_lumps() {
        let session = session_over(vec![
            Lump::marker(parsed("F_START")),
            Lump::new(parsed("FLOOR1"), vec![0_u8; 4]),
            Lump::marker(parsed("F_END")),
        ]);
        let index = resource_index(&session).expect("a flat-only view indexes");
        assert!(
            !index.has_texture_defs,
            "no TEXTURE1/TEXTURE2 and no textures region"
        );
        assert!(index.textures.is_empty(), "nothing to look names up in");
        assert_eq!(index.flats.len(), 1, "the flat is still collected");
    }

    #[test]
    fn resource_index_refuses_a_malformed_texture_lump() {
        let session = session_over(vec![Lump::new(parsed("TEXTURE1"), b"junk".to_vec())]);
        let error = resource_index(&session).expect_err("the lump cannot decode");
        assert_eq!(
            error.code(),
            "format_error",
            "a broken TEXTURE1 fails the run with the fix-hinting format error"
        );
    }

    #[test]
    fn viewed_totals_everything_then_filters_and_pages() {
        let doom = definitions_for(Game::Doom2).expect("the embedded Doom II table is valid");
        let hexen = definitions_for(Game::Hexen).expect("the embedded Hexen table is valid");
        let resources = ResourceIndex::default();
        let context = Context {
            definitions: &doom,
            hexen_specials: &hexen,
            resources: &resources,
        };
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        let name = LumpName::parse("MAP01").expect("test names are valid");
        let outcome = validate::run(&map, &context, None);
        let errors_only = viewed(name, outcome, Severity::Error, DEFAULT_LIMIT);
        assert_eq!(errors_only.map, name, "the map travels");
        assert_eq!(errors_only.issues.len(), 1, "only the start error stays");
        assert_eq!(errors_only.counts.warning, 2, "counts keep the whole truth");
        assert_eq!(errors_only.counts.info, 3, "at every severity");
        assert!(!errors_only.truncated, "one row fits any page");
        assert_eq!(
            errors_only.checks_run,
            validate::codes(),
            "with no code filter every check ran"
        );
    }

    #[test]
    fn viewed_pages_the_kept_rows_and_says_so() {
        let doom = definitions_for(Game::Doom2).expect("the embedded Doom II table is valid");
        let hexen = definitions_for(Game::Hexen).expect("the embedded Hexen table is valid");
        let resources = ResourceIndex::default();
        let context = Context {
            definitions: &doom,
            hexen_specials: &hexen,
            resources: &resources,
        };
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        let name = LumpName::parse("MAP01").expect("test names are valid");
        let outcome = validate::run(&map, &context, None);
        let paged = viewed(name, outcome, Severity::Info, 2);
        assert_eq!(paged.issues.len(), 2, "the page holds two of the six rows");
        assert!(paged.truncated, "four stayed behind");
        assert_eq!(paged.counts.error, 1, "counts still total everything");
        assert_eq!(paged.counts.warning, 2, "at every severity,");
        assert_eq!(paged.counts.info, 3, "filtered or paged out or not");
    }

    #[test]
    fn validate_run_refuses_bad_parameters_before_touching_a_session() {
        let state = AppState::default();
        let mut bad_severity = args(None, None);
        bad_severity.min_severity = Some("fatal".to_owned());
        assert_eq!(
            validate_run(&state, &bad_severity)
                .expect_err("not a severity")
                .code(),
            "invalid_argument",
            "a bad floor is refused even with no WAD open"
        );
        let mut bad_codes = args(None, None);
        bad_codes.codes = Some(vec!["nope".to_owned()]);
        assert_eq!(
            validate_run(&state, &bad_codes)
                .expect_err("not a code")
                .code(),
            "invalid_argument",
            "an unknown code is refused"
        );
        let mut bad_limit = args(None, None);
        bad_limit.limit = Some(0);
        assert_eq!(
            validate_run(&state, &bad_limit)
                .expect_err("zero rows")
                .code(),
            "invalid_argument",
            "a zero limit is refused"
        );
        assert_eq!(
            validate_run(&state, &args(None, Some("nope")))
                .expect_err("not a scope")
                .code(),
            "invalid_argument",
            "an unknown scope is refused"
        );
        assert_eq!(
            validate_run(&state, &args(None, None))
                .expect_err("no maps at all")
                .code(),
            "invalid_argument",
            "the empty-session no-map refusal comes from the shared resolver"
        );
    }
}
