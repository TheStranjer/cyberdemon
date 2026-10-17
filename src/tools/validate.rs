//! Validate tools: [`Cyberdemon::validate_map`], the map health report.
//!
//! The checks themselves are the pure [`crate::wad::map::validate`]
//! module's; this file only resolves the parameters, reads the map and the
//! session's game definitions under one lock, and applies the severity
//! filter, the paging and the counting the client asked about. Later
//! validation tickets add checks to [`crate::wad::map::validate::CHECKS`]
//! without touching anything here.

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
use crate::tools::{fail, ok};
use crate::wad::game::Game;
use crate::wad::map::validate::{self, Context, Issue, Severity};
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
/// [`Context`] can borrow both.
struct Tables {
    /// The session's definitions: the detected game's tables with the
    /// loaded archives' custom things and specials overlaid (what the
    /// thing and `Doom`-family special checks consult).
    definitions: Definitions,
    /// The `Hexen` specials table: what `Hexen`-family maps check their
    /// specials against whatever game the session detected.
    hexen_specials: Definitions,
}

impl Tables {
    /// The validation context borrowing both tables.
    const fn context(&self) -> Context<'_> {
        Context {
            definitions: &self.definitions,
            hexen_specials: &self.hexen_specials,
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
/// MAPINFO customs, plus the built-in `Hexen` specials.
///
/// # Errors
///
/// [`collect::custom_sources`]'s read errors and [`session_definitions`]'
/// and [`definitions_for`]' refusals for malformed definition data.
fn session_tables(session: &Session) -> Result<Tables, CyberError> {
    let (sources, _notes) = collect::custom_sources(session)?;
    Ok(Tables {
        definitions: session_definitions(detected_game(session), &sources)?,
        hexen_specials: definitions_for(Game::Hexen)?,
    })
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

/// How many issues of each severity the enabled checks found.
#[derive(Debug, Default, Serialize)]
struct Counts {
    /// The [`Severity::Error`] rows: the map is broken.
    error: usize,
    /// The [`Severity::Warning`] rows: the map is probably wrong.
    warning: usize,
    /// The [`Severity::Info`] rows: the map deserves a look.
    info: usize,
}

impl Counts {
    /// The totals of `issues` by severity.
    fn of(issues: &[Issue]) -> Self {
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
/// session is touched, then read the map and the definitions under one
/// lock and run the selected checks.
///
/// # Errors
///
/// The parameter parsers' [`CyberError::InvalidArgument`] refusals, and
/// under the lock [`common::resolve_map_name`]'s, [`common::load_map`]'s
/// and [`session_tables`]' errors.
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
        let outcome = validate::run(&map, &tables.context(), enabled.as_ref());
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
        description = "Checks one map for reference, structure, thing and special problems and reports every finding as an issue {severity (\"error\"|\"warning\"|\"info\"), code, message, vertices, linedefs, sidedefs, sectors, things (0-based index lists, present only when the issue has them), x, y (map coordinates, when the issue sits at a point)}. map defaults to the active map (set_active_map, else the merged view's only map); scope picks the archive (\"merged\" default, \"working\" or \"layer:N\"). min_severity (\"error\", \"warning\" or \"info\", default \"info\") keeps only that severity and above; codes runs only the named checks; limit (default 500, max 5000) pages the issue list. Returns {map, counts: {error, warning, info}, issues, truncated, checks_run}: counts totals everything the enabled checks found (before the severity filter and the limit), checks_run lists the codes that ran in report order, truncated says whether the limit dropped kept issues. The checks: vertex_out_of_range, sidedef_out_of_range, sector_out_of_range, missing_front_side, zero_length_line, duplicate_line (two linedefs connect the same vertex pair in either direction) and no_player1_start (no thing of type 1) are errors; two_sided_flag_mismatch (back side without the two-sided flag 4, or the flag without a back side), unused_sector (no sidedef references it), unknown_thing_type (the type is absent from the session's definitions, which include DEHACKED/DECORATE/ZScript/MAPINFO customs), unknown_special (outside the map's flag-family table; Doom-family maps also accept the Boom generalized range 0x2F80..=0x7FFF), special_without_tag (Doom-family sector special on an untagged line) and tag_without_sector (Doom-family nonzero tag no sector carries) are warnings; unused_vertex, unused_sidedef, shared_sidedef (several linedefs use one sidedef) and missing_coop_starts (no things of types 2 to 4) are info. The tag checks run only for Doom-family maps (Hexen-family lines trigger through arguments, which are not validated yet); node lumps and UDMF-only features are not checked. Unknown maps are not_found; bad scope, severity, code or limit values are invalid_argument."
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
        parse_limit, parse_min_severity, parse_scope, validate_run, viewed,
    };
    use crate::defs::definitions_for;
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::state::AppState;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::game::Game;
    use crate::wad::map::validate::{self, Context, Severity};
    use crate::wad::map::{Map, Sector};
    use crate::wad::name::LumpName;

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
        let context = Context {
            definitions: &doom,
            hexen_specials: &hexen,
        };
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        let outcome = validate::run(&map, &context, None);
        let counts = Counts::of(&outcome.issues);
        assert_eq!(counts.error, 1, "only the missing player 1 start");
        assert_eq!(counts.warning, 1, "only the orphaned sector");
        assert_eq!(counts.info, 1, "only the missing coop set");
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

    #[test]
    fn viewed_totals_everything_then_filters_and_pages() {
        let doom = definitions_for(Game::Doom2).expect("the embedded Doom II table is valid");
        let hexen = definitions_for(Game::Hexen).expect("the embedded Hexen table is valid");
        let context = Context {
            definitions: &doom,
            hexen_specials: &hexen,
        };
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        let name = LumpName::parse("MAP01").expect("test names are valid");
        let outcome = validate::run(&map, &context, None);
        let errors_only = viewed(name, outcome, Severity::Error, DEFAULT_LIMIT);
        assert_eq!(errors_only.map, name, "the map travels");
        assert_eq!(errors_only.issues.len(), 1, "only the start error stays");
        assert_eq!(errors_only.counts.warning, 1, "counts keep the whole truth");
        assert_eq!(errors_only.counts.info, 1, "at every severity");
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
        let context = Context {
            definitions: &doom,
            hexen_specials: &hexen,
        };
        let mut map = Map::default();
        map.sectors.push(Sector::default());
        let name = LumpName::parse("MAP01").expect("test names are valid");
        let outcome = validate::run(&map, &context, None);
        let paged = viewed(name, outcome, Severity::Info, 2);
        assert_eq!(
            paged.issues.len(),
            2,
            "the page holds two of the three rows"
        );
        assert!(paged.truncated, "and one stayed behind");
        assert_eq!(paged.counts.error, 1, "counts still total everything");
        assert_eq!(paged.counts.warning, 1, "at every severity,");
        assert_eq!(paged.counts.info, 1, "filtered or paged out or not");
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
