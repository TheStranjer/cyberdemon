//! Inspect tools: comparing what two WAD states hold.
//!
//! [`Cyberdemon::diff_wads`] answers "what does my `PWAD` override?" by
//! default (`base` against `working`) and any other question of the same
//! shape by naming two sides: session scopes, a single layer or an archive
//! file on disk. The comparison itself is the pure [`crate::wad::diff`];
//! this module only resolves side strings to directories, runs the diff
//! under the session lock and pages the result.

use std::path::{Path, PathBuf};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{MergedLump, Scope, Session};
use crate::state::AppState;
use crate::tools::{fail, ok};
use crate::wad::Lump;
use crate::wad::archive::Wad;
use crate::wad::diff::{Diff, LumpDiff, MapDiff, Mode, Status, Summary, diff};

/// Default side `a`: the layers without the working archive.
const DEFAULT_A: &str = "base";
/// Default side `b`: the editable archive.
const DEFAULT_B: &str = "working";
/// Default row cap for both result lists.
const DEFAULT_LIMIT: usize = 500;
/// Largest row cap either list accepts.
const MAX_LIMIT: usize = 5000;

/// Arguments for [`Cyberdemon::diff_wads`].
#[derive(Debug, Deserialize, JsonSchema)]
struct DiffWadsParams {
    /// The left side: `"working"`, `"merged"`, `"base"` (the read-only
    /// layers without the working archive, the default), `"layer:N"` (0 is
    /// the IWAD) or an absolute path to a WAD file on disk.
    #[serde(default)]
    a: Option<String>,
    /// The right side, same forms as `a`; default `"working"`.
    #[serde(default)]
    b: Option<String>,
    /// List unchanged units too (default `false`: only what differs).
    #[serde(default)]
    include_unchanged: bool,
    /// Maximum rows per list (default 500, at most 5000); `truncated`
    /// says whether either list held more.
    #[serde(default)]
    limit: Option<usize>,
}

/// A side string after parsing, before its directory is at hand.
#[derive(Clone, Debug)]
enum Side {
    /// The merged view of the read-only layers alone.
    Base,
    /// A session scope: the working archive, the merged view or one layer.
    Scope(Scope),
    /// An absolute path to a `WAD` file, read fresh and never loaded into
    /// the session.
    File(PathBuf),
}

/// A side prepared for one call: its parsed form and the text to echo.
#[derive(Debug)]
struct Prepared {
    /// The side text the client sees in the result.
    text: String,
    /// What the text names.
    side: Side,
}

/// The echo strings and view settings of one report.
struct ReportArgs {
    /// Side `a` as given.
    a: String,
    /// Side `b` as given.
    b: String,
    /// Whether unchanged units stay in the lists.
    include_unchanged: bool,
    /// Row cap for both lists.
    limit: usize,
}

/// Structured result of [`Cyberdemon::diff_wads`].
#[derive(Debug, Serialize)]
struct DiffReport {
    /// Side `a` as given.
    a: String,
    /// Side `b` as given.
    b: String,
    /// Counts over the whole comparison, before filtering and paging.
    summary: Summary,
    /// The compared lumps, `a` order then `b` arrivals.
    lumps: Vec<LumpDiff>,
    /// The compared map blocks in the same order.
    maps: Vec<MapDiff>,
    /// Whether either list held more rows than `limit` allows.
    truncated: bool,
}

/// Whether a unit of this status reaches the result lists.
fn shows(status: Status, include_unchanged: bool) -> bool {
    include_unchanged || status != Status::Unchanged
}

/// Parse one side string: a scope keyword, `base` or an absolute path.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a keyword [`Scope::parse`] rejects,
/// for a bad `layer:` index and for any relative path.
fn parse_side(text: &str) -> Result<Side, CyberError> {
    if Path::new(text).is_absolute() {
        return Ok(Side::File(PathBuf::from(text)));
    }
    let lowered = text.to_ascii_lowercase();
    if lowered == "base" {
        return Ok(Side::Base);
    }
    if lowered == "working" || lowered == "merged" || lowered.starts_with("layer:") {
        return Scope::parse(text).map(Side::Scope);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown side {text:?}: expected \"working\", \"merged\", \"base\", \"layer:N\" (N is \
         zero-based, 0 is the IWAD) or an absolute path to a WAD file"
    )))
}

/// The side pair that answers "what does my `PWAD` override?": `base`
/// against `working` reports in the override vocabulary.
const fn pick_mode(a: &Side, b: &Side) -> Mode {
    let base = matches!(a, Side::Base);
    let working = matches!(b, Side::Scope(Scope::Working));
    if base && working {
        Mode::Override
    } else {
        Mode::General
    }
}

/// Read a file side: a missing file is `not_found`, other read failures
/// keep [`Wad::read_file`]'s `io_error` and format failures pass through.
fn read_archive(path: &Path) -> Result<Wad, CyberError> {
    Wad::read_file(path).map_err(|error| open_error(path, error))
}

/// A failed archive read: a missing file is `not_found`, everything else
/// is what [`Wad::read_file`] reported.
fn open_error(path: &Path, error: CyberError) -> CyberError {
    let missing = matches!(error, CyberError::Io(_)) && matches!(path.try_exists(), Ok(false));
    if missing {
        return CyberError::NotFound(format!("no file at {}", path.display()));
    }
    error
}

/// The plain lumps of a session directory view.
fn owned(view: Vec<MergedLump>) -> Vec<Lump> {
    view.into_iter().map(|entry| entry.lump).collect()
}

/// The directory one side names.
///
/// # Errors
///
/// [`Side::Scope`]'s out-of-range layer and [`read_archive`]'s file errors.
fn side_lumps(session: &Session, side: Side) -> Result<Vec<Lump>, CyberError> {
    match side {
        Side::Base => Ok(owned(session.base_lumps())),
        Side::Scope(scope) => Ok(owned(session.lumps_in(scope)?)),
        Side::File(path) => Ok(read_archive(&path)?.lumps),
    }
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

/// Validate both side strings and the limit before the session is touched.
///
/// # Errors
///
/// [`parse_side`]'s and [`parse_limit`]'s errors.
fn plan(args: &DiffWadsParams) -> Result<(Prepared, Prepared, usize), CyberError> {
    let a_text = args.a.as_deref().unwrap_or(DEFAULT_A);
    let b_text = args.b.as_deref().unwrap_or(DEFAULT_B);
    Ok((
        Prepared {
            text: a_text.to_owned(),
            side: parse_side(a_text)?,
        },
        Prepared {
            text: b_text.to_owned(),
            side: parse_side(b_text)?,
        },
        parse_limit(args.limit)?,
    ))
}

/// Drop unchanged units unless asked for and page both lists at the limit.
fn viewed(args: ReportArgs, outcome: Diff) -> DiffReport {
    let ReportArgs {
        a,
        b,
        include_unchanged,
        limit,
    } = args;
    let summary = outcome.summary();
    let mut lumps: Vec<LumpDiff> = outcome
        .lumps
        .into_iter()
        .filter(|row| shows(row.status, include_unchanged))
        .collect();
    let mut maps: Vec<MapDiff> = outcome
        .maps
        .into_iter()
        .filter(|unit| shows(unit.status, include_unchanged))
        .collect();
    let truncated = lumps.len() > limit || maps.len() > limit;
    lumps.truncate(limit);
    maps.truncate(limit);
    DiffReport {
        a,
        b,
        summary,
        lumps,
        maps,
        truncated,
    }
}

/// Run one `diff_wads` call end to end.
///
/// # Errors
///
/// [`plan`]'s errors and whatever [`side_lumps`] reports under the session
/// lock.
fn diff_run(state: &AppState, args: &DiffWadsParams) -> Result<DiffReport, CyberError> {
    let (a, b, limit) = plan(args)?;
    let mode = pick_mode(&a.side, &b.side);
    let outcome = state.with_session(|session| {
        let left = side_lumps(session, a.side.clone())?;
        let right = side_lumps(session, b.side.clone())?;
        Ok(diff(&left, &right, mode))
    })?;
    Ok(viewed(
        ReportArgs {
            a: a.text,
            b: b.text,
            include_unchanged: args.include_unchanged,
            limit,
        },
        outcome,
    ))
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = inspect_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Compares two WAD states lump by lump and map block by map block.
    #[tool(
        description = "Diffs two WAD states. Each side is \"working\" (the editable archive), \"merged\" (what an engine sees), \"base\" (the read-only layers without the working archive), \"layer:N\" (read-only layer N, 0 is the IWAD) or an absolute path to a WAD file (read fresh, never loaded into the session). Defaults a=\"base\", b=\"working\": what does my PWAD override? In that default mode units only in b are \"new\", units in both sides are \"override\" (bytes differ) or \"identical_override\" (same bytes, usually a cleanup-worthy mistake) and \"removed\" cannot happen; other side pairs report \"added\" (only in b), \"removed\" (only in a), \"changed\" (bytes differ) and \"unchanged\". Map blocks compare as units by map name: a changed map lists its differing member lumps and, when both blocks parse, element counts {things, linedefs, sidedefs, vertices, sectors} per side (null when a block is absent or does not decode). include_unchanged (default false) also lists units that match; limit (default 500, max 5000) pages both lists and sets truncated. Returns {a, b, summary: {added, removed, changed, unchanged, override, identical_override, new}, lumps: [{name, namespace, status, size_a, size_b}], maps: [{name, status, lumps, counts_a, counts_b}], truncated}; summary counts every compared unit (maps included) before filtering and paging. Unknown side names and relative paths are invalid_argument, missing files and out-of-range layers not_found, unreadable WAD files format_error."
    )]
    fn diff_wads(
        &self,
        Parameters(args): Parameters<DiffWadsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match diff_run(self.state(), &args) {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_A, DEFAULT_B, DiffWadsParams, MAX_LIMIT, Mode, Prepared, Side, diff_run,
        parse_limit, parse_side, pick_mode, plan, shows,
    };
    use crate::server::Cyberdemon;
    use crate::session::Scope;
    use crate::wad::Status;

    /// `DiffWadsParams` diffing `base` against side `b`.
    fn args_for_b(b: &str) -> DiffWadsParams {
        DiffWadsParams {
            a: None,
            b: Some(b.to_owned()),
            include_unchanged: false,
            limit: None,
        }
    }

    /// `DiffWadsParams` with explicit sides and paging.
    fn args(a: Option<&str>, b: Option<&str>, limit: Option<usize>) -> DiffWadsParams {
        DiffWadsParams {
            a: a.map(str::to_owned),
            b: b.map(str::to_owned),
            include_unchanged: false,
            limit,
        }
    }

    #[test]
    fn parse_side_accepts_every_documented_form() {
        assert!(
            matches!(parse_side("base").expect("base"), Side::Base),
            "the base view"
        );
        assert!(
            matches!(
                parse_side("Working").expect("working"),
                Side::Scope(Scope::Working)
            ),
            "the working archive, any case"
        );
        assert!(
            matches!(
                parse_side("merged").expect("merged"),
                Side::Scope(Scope::Merged)
            ),
            "the merged view"
        );
        assert!(
            matches!(
                parse_side("LAYER:1").expect("layer"),
                Side::Scope(Scope::Layer(1))
            ),
            "a layer index, any case"
        );
        assert!(
            matches!(parse_side("/nonexistent/extra.wad"), Ok(Side::File(_))),
            "an absolute path parses whatever it names"
        );
    }

    #[test]
    fn parse_side_refuses_names_and_relative_paths() {
        let unknown = parse_side("sideways").expect_err("no such side");
        assert_eq!(unknown.code(), "invalid_argument", "unknown side code");
        assert!(
            unknown.to_string().contains("absolute path"),
            "the message lists the forms: {unknown}"
        );
        let relative = parse_side("wads/extra.wad").expect_err("not absolute");
        assert_eq!(relative.code(), "invalid_argument", "relative path code");
        let bad_layer = parse_side("layer:x").expect_err("not a number");
        assert_eq!(bad_layer.code(), "invalid_argument", "bad layer index code");
    }

    #[test]
    fn only_the_default_pair_gets_the_override_vocabulary() {
        let base = Side::Base;
        let working = Side::Scope(Scope::Working);
        let merged = Side::Scope(Scope::Merged);
        let file = Side::File("/tmp/extra.wad".into());
        assert_eq!(
            pick_mode(&base, &working),
            Mode::Override,
            "base to working"
        );
        assert_eq!(pick_mode(&base, &merged), Mode::General, "base to merged");
        assert_eq!(
            pick_mode(&working, &file),
            Mode::General,
            "working to a file"
        );
    }

    #[test]
    fn the_limit_defaults_pages_and_is_capped() {
        assert_eq!(
            parse_limit(None).expect("the default"),
            500,
            "default limit"
        );
        assert_eq!(parse_limit(Some(1)).expect("the floor"), 1, "the floor");
        assert_eq!(
            parse_limit(Some(MAX_LIMIT)).expect("the ceiling"),
            MAX_LIMIT,
            "the ceiling"
        );
        let zero = parse_limit(Some(0)).expect_err("zero rows is no page");
        assert_eq!(zero.code(), "invalid_argument", "zero limit code");
        let over = parse_limit(Some(MAX_LIMIT + 1)).expect_err("past the ceiling");
        assert_eq!(over.code(), "invalid_argument", "overshoot limit code");
    }

    #[test]
    fn unchanged_units_are_shown_only_when_asked() {
        assert!(!shows(Status::Unchanged, false), "the default drops them");
        assert!(
            shows(Status::Unchanged, true),
            "and the flag brings them back"
        );
        assert!(shows(Status::New, false), "every other status always stays");
        assert!(
            shows(Status::Removed, false),
            "including the one-sided ones"
        );
    }

    #[test]
    fn the_default_args_plan_against_the_documented_defaults() {
        let (a, b, limit) = plan(&args(None, None, None)).expect("plain arguments plan");
        let Prepared {
            text: a_text,
            side: a_side,
        } = a;
        assert_eq!(a_text, DEFAULT_A, "side a defaults");
        assert!(matches!(a_side, Side::Base), "and parses");
        assert_eq!(b.text, DEFAULT_B, "side b defaults");
        assert!(matches!(b.side, Side::Scope(Scope::Working)), "and parses");
        assert_eq!(limit, 500, "the default page size");
    }

    #[test]
    fn planning_rejects_a_bad_side_before_any_work() {
        let server = Cyberdemon::new();
        let error =
            diff_run(server.state(), &args(Some("nope"), None, Some(10))).expect_err("bad side");
        assert_eq!(error.code(), "invalid_argument", "bad side code");
    }

    #[test]
    fn a_fresh_session_diffs_two_empty_sides() {
        let server = Cyberdemon::new();
        let report = diff_run(server.state(), &args_for_b("merged")).expect("empty sides");
        assert_eq!(report.a, DEFAULT_A, "the defaults echo back");
        assert_eq!(report.b, "merged", "and the given side echoes");
        assert!(
            report.lumps.is_empty() && report.maps.is_empty(),
            "nothing to compare"
        );
        assert!(!report.truncated, "and nothing to page");
    }

    #[test]
    fn a_missing_file_side_is_not_found() {
        let server = Cyberdemon::new();
        let error = diff_run(
            server.state(),
            &args(Some("/nonexistent-directory/absent.wad"), None, None),
        )
        .expect_err("no such file");
        assert_eq!(error.code(), "not_found", "missing file code");
    }

    #[test]
    fn an_out_of_range_layer_side_is_not_found() {
        let server = Cyberdemon::new();
        let error = diff_run(server.state(), &args_for_b("layer:3")).expect_err("nothing stacked");
        assert_eq!(error.code(), "not_found", "out of range layer code");
    }
}
