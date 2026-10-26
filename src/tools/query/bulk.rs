//! `bulk_edit`: the wire parameters that carry a `find_elements`
//! selection beside one [`FieldEdits`] request, the run that applies it as
//! one [`crate::session::Session::edit_map`] step, and the paged
//! [`BulkReport`].
//!
//! [`bulk_in_session`] refuses an empty request through
//! [`nothing_to_change`] before anything reads, selects with the same pure
//! query engine [`selected`] guards behind `max`, folds a `type` name into
//! a doomednum through [`resolve_thing_type`], checks texture and flat
//! names against [`Stock`] and hands the set to [`apply`]: as a preview
//! over the merged copy with [`dry_fragment`], or as one labeled
//! `edit_map` step whose [`crate::wad::map::bulk::edits::Applied`] folds
//! into [`bulk_report`] with [`bulk_next_steps`].
//!
//! The read the selection comes from is the `find` module's; the swaps
//! that rewrite whole maps and the machinery they share across maps are
//! the `replace_tools` and public `replace` modules'.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::defs::Definitions;
use crate::error::CyberError;
use crate::plan::things::resolve_thing_kind;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::maps::common;
use crate::tools::maps::params::edit_result_fragment;
use crate::tools::query::replace::Stock;
use crate::wad::map::Map;
use crate::wad::map::bulk::edits::{Applied, Change, EditScope, FieldEdits, apply};
use crate::wad::map::query::{ElementKind, Filter, Query, QueryContext, Within, run_query};
use crate::wad::name::LumpName;

use super::{DEFAULT_MAX, INDEX_PAGE, PREVIEW_PAGE};

/// Arguments for [`Cyberdemon::bulk_edit`]: the [`Query`] fields beside
/// one [`FieldEdits`] request and the run's knobs.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct BulkEditParams {
    /// The map to edit, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The element block to edit: "sectors", "linedefs", "sidedefs",
    /// "things" or "vertices".
    pub(super) kind: ElementKind,
    /// The filters selecting the elements, exactly as `find_elements`'
    /// `where`: an empty list edits everything of the kind.
    #[serde(default, rename = "where")]
    pub(super) r#where: Vec<Filter>,
    /// Bound the selection by region, exactly as `find_elements`'
    /// `within`.
    #[serde(default)]
    pub(super) within: Option<Within>,
    /// New values by field name: integers, texture/flat names, and for
    /// things a doomednum integer or a definition name.
    #[serde(default)]
    pub(super) set: Option<BTreeMap<String, Value>>,
    /// Integer amounts shifted onto each matched element's current value.
    #[serde(default)]
    pub(super) add: Option<BTreeMap<String, i64>>,
    /// Flag names to set on each matched linedef or thing.
    #[serde(default)]
    pub(super) flags_set: Option<Vec<String>>,
    /// Flag names to clear on each matched linedef or thing; a bit named
    /// on both ends ends up set.
    #[serde(default)]
    pub(super) flags_clear: Option<Vec<String>>,
    /// Write texture/flat names the merged view lacks without the
    /// existence check (default `false`). Numbers are never clamped
    /// either way.
    #[serde(default)]
    pub(super) allow_missing: Option<bool>,
    /// Run the whole check and report the same numbers over a copy of the
    /// map, writing nothing and recording no undo step (default `false`).
    #[serde(default)]
    pub(super) dry_run: Option<bool>,
    /// Refuse (and write nothing) when more than this many elements match
    /// (default 5000); `max: 0` therefore refuses any non-empty match.
    #[serde(default)]
    pub(super) max: Option<usize>,
}

/// Structured result of [`Cyberdemon::bulk_edit`]: the engine's [`Applied`]
/// paged to the documented windows, beside the standard edit fragment.
#[derive(Debug, Serialize)]
pub(super) struct BulkReport {
    /// Elements the query selected.
    pub(super) matched: usize,
    /// Elements whose values actually changed.
    pub(super) changed: usize,
    /// Ascending indices of the changed elements, at most [`INDEX_PAGE`]
    /// of them.
    pub(super) indices: Vec<usize>,
    /// Whether [`Self::indices`] left changed indices out.
    pub(super) truncated: bool,
    /// Whether the run only previewed.
    pub(super) dry_run: bool,
    /// Up to [`PREVIEW_PAGE`] before/after change rows.
    pub(super) preview: Vec<Change>,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    pub(super) edit: JsonMap<String, Value>,
    /// What to do or look at next.
    pub(super) next_steps: Vec<&'static str>,
}

/// The `next_steps` every written `bulk_edit` report carries.
const BULK_NEXT_STEPS: [&str; 3] = [
    "undo takes the whole edit back as one step",
    "find_elements over the same query reads the new values",
    "render_topdown looks at the result",
];

/// The `next_steps` a previewed `bulk_edit` report carries.
const BULK_DRY_NEXT_STEPS: [&str; 2] = [
    "nothing was written: run again with dry_run false to apply the edit",
    "the preview rows list every element that would change, before and after",
];

/// The extra step a geometry edit's report carries.
pub(super) const BULK_NODES_STEP: &str =
    "nodes_invalidated is true: run build_nodes before playing the map in vanilla Doom";

/// The four request parts of [`BulkEditParams`] as one engine
/// [`FieldEdits`]. The rows move in rather than being cloned: the tool's run
/// owns its parameters and needs them nowhere else afterwards.
pub(super) fn requested_edits(
    set: Option<BTreeMap<String, Value>>,
    add: Option<BTreeMap<String, i64>>,
    flags_set: Option<Vec<String>>,
    flags_clear: Option<Vec<String>>,
) -> FieldEdits {
    FieldEdits {
        set: set.unwrap_or_default(),
        add: add.unwrap_or_default(),
        flags_set: flags_set.unwrap_or_default(),
        flags_clear: flags_clear.unwrap_or_default(),
    }
}

/// The refusal for a call that asks for no field change at all.
fn nothing_to_change() -> CyberError {
    CyberError::InvalidArgument(
        "nothing to change: pass at least one of set, add, flags_set or flags_clear".to_owned(),
    )
}

/// The query engine's selection for this call, behind the `max` guard.
///
/// # Errors
///
/// [`run_query`]'s refusals for a bad field, op or value, and
/// [`CyberError::LimitExceeded`] when the hits outnumber `max`.
fn selected(
    map: &Map,
    defs: &Definitions,
    query: &Query,
    cap: Option<usize>,
) -> Result<Vec<usize>, CyberError> {
    let ctx = QueryContext::new(map.flag_family(), defs);
    let hits = run_query(map, &ctx, query)?;
    let max = cap.unwrap_or(DEFAULT_MAX);
    if hits.len() > max {
        return Err(CyberError::LimitExceeded(format!(
            "{} {} match this query, above max {max}: nothing was written, \
             dry_run included. Narrow the selection with where/within, or \
             raise max to at least {}",
            hits.len(),
            query.kind.as_str(),
            hits.len()
        )));
    }
    Ok(hits)
}

/// Fold a `type` name on a `things` request into the doomednum the engine
/// writes; integers and every other kind pass through untouched.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the candidates when the
/// session's definitions hold no thing with that name, or when one
/// substring matches several distinct types.
fn resolve_thing_type(
    mut edits: FieldEdits,
    kind: ElementKind,
    defs: &Definitions,
) -> Result<FieldEdits, CyberError> {
    if kind != ElementKind::Things {
        return Ok(edits);
    }
    let Some(text) = edits.set.get("type").and_then(Value::as_str) else {
        return Ok(edits);
    };
    let resolved = resolve_thing_kind(defs, text).map_err(CyberError::InvalidArgument)?;
    edits
        .set
        .insert("type".to_owned(), Value::from(u64::from(resolved)));
    Ok(edits)
}

/// The edit fragment a `dry_run` reports: the named map, nothing copied,
/// nothing invalidated.
fn dry_fragment(name: LumpName) -> JsonMap<String, Value> {
    [
        ("map".to_owned(), Value::String(name.to_string())),
        ("nodes_invalidated".to_owned(), Value::Bool(false)),
        ("copied_to_working".to_owned(), Value::Bool(false)),
    ]
    .into_iter()
    .collect()
}

/// The report's `next_steps`, by run mode and whether the step changed
/// geometry.
fn bulk_next_steps(dry_run: bool, edit: &JsonMap<String, Value>) -> Vec<&'static str> {
    let nodes_invalidated = edit
        .get("nodes_invalidated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut steps: Vec<&'static str> = if dry_run {
        BULK_DRY_NEXT_STEPS.to_vec()
    } else {
        BULK_NEXT_STEPS.to_vec()
    };
    if nodes_invalidated {
        steps.push(BULK_NODES_STEP);
    }
    steps
}

/// [`Applied`] folded into the tool's report, paged to the documented
/// `indices` and `preview` windows, beside the edit fragment.
pub(super) fn bulk_report(
    applied: Applied,
    dry_run: bool,
    edit: JsonMap<String, Value>,
) -> BulkReport {
    let truncated = applied.indices.len() > INDEX_PAGE;
    let next_steps = bulk_next_steps(dry_run, &edit);
    BulkReport {
        matched: applied.matched,
        changed: applied.changed,
        indices: applied.indices.iter().copied().take(INDEX_PAGE).collect(),
        truncated,
        dry_run,
        preview: applied.preview.into_iter().take(PREVIEW_PAGE).collect(),
        edit,
        next_steps,
    }
}

/// `bulk_edit`'s work inside the session lock: parse the request, select
/// with the query engine behind the `max` guard, resolve thing-type names,
/// and run the pure engine either as a preview over the merged copy or as
/// one undoable `Session::edit_map` step.
///
/// # Errors
///
/// [`nothing_to_change`]'s refusal, the map resolvers' errors,
/// [`selected`]'s and [`resolve_thing_type`]'s refusals, the engine's
/// (nothing is written behind any of them, and [`Session::edit_map`]
/// rolls the whole step back on any error inside it) and [`Stock::load`]'s
/// errors.
pub(super) fn bulk_in_session(
    session: &mut Session,
    args: BulkEditParams,
) -> Result<BulkReport, CyberError> {
    let kind = args.kind;
    let requested = requested_edits(args.set, args.add, args.flags_set, args.flags_clear);
    if requested.is_empty() {
        return Err(nothing_to_change());
    }
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let mut map = common::load_map(session, name, Scope::Merged)?;
    let defs = session.definitions()?;
    let query = Query {
        kind,
        r#where: args.r#where,
        within: args.within,
    };
    let hits = selected(&map, &defs, &query, args.max)?;
    let edits = resolve_thing_type(requested, kind, &defs)?;
    let stock = Stock::load(session)?;
    let scope = EditScope {
        format: map.format,
        family: map.flag_family(),
        walls: &stock.walls,
        flats: &stock.flats,
        allow_missing: args.allow_missing.unwrap_or(false),
    };
    if args.dry_run.unwrap_or(false) {
        let applied = apply(&mut map, kind, &hits, &edits, &scope)?;
        return Ok(bulk_report(applied, true, dry_fragment(name)));
    }
    let label = format!("bulk_edit {} ({})", kind.as_str(), hits.len());
    let (applied, report) = session.edit_map(name, &label, |target| {
        apply(target, kind, &hits, &edits, &scope)
    })?;
    Ok(bulk_report(applied, false, edit_result_fragment(&report)))
}

/// `bulk_edit`'s work: the whole request under one session lock.
///
/// # Errors
///
/// [`bulk_in_session`]'s errors and the poisoned-lock
/// [`CyberError::Internal`].
pub(super) fn bulk_run(state: &AppState, args: BulkEditParams) -> Result<BulkReport, CyberError> {
    state.with_session(|session| bulk_in_session(session, args))
}
