//! `describe_map`'s side of the playability group: the measured per-sector
//! picture of a whole map.
//!
//! [`load`](super::load) reads the map and the session's definitions under one
//! lock exactly like [`check_run`](super::check::check_run) does, the pure
//! [`crate::wad::map::describe`] module answers the facts and this module
//! deduplicates and filters the requested sectors, pages the rows by
//! `offset`/`limit`, keeps the always-whole [`MapOverview`] summary beside them
//! whatever the paging says and publishes the labelled overview with plain
//! marks.
//!
//! [`MapOverview`]: crate::wad::map::describe::MapOverview

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::state::AppState;
use crate::tools::maps::params::{ElementKind, check_index};
use crate::tools::render::{OverviewMarks, playability_overview};
use crate::tools::{Page, parse_limit};
use crate::wad::map::describe::{Describe, MapOverview, SectorFacts};
use crate::wad::name::LumpName;

use super::load;

/// Default sector-row cap for [`crate::server::Cyberdemon::describe_map`].
pub(super) const DESCRIBE_DEFAULT_LIMIT: usize = 150;
/// Largest sector-row cap [`crate::server::Cyberdemon::describe_map`] accepts.
pub(super) const DESCRIBE_MAX_LIMIT: usize = 500;

/// Arguments for [`crate::server::Cyberdemon::describe_map`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct DescribeMapParams {
    /// The map to describe, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// Describe only these sectors, by index; unset describes every sector.
    /// The list is deduplicated and read in ascending order; an index at or
    /// above the sector count fails with `not_found` naming the valid range.
    #[serde(default)]
    pub(super) sectors: Option<Vec<usize>>,
    /// Skip this many rows of the (possibly filtered) sector list before
    /// returning (default 0). An offset past the end is never an error: the
    /// rows simply come back empty.
    #[serde(default)]
    pub(super) offset: Option<usize>,
    /// Maximum sector rows returned (default 150, at most 500);
    /// `next_offset` names the offset that continues where this page
    /// stopped.
    #[serde(default)]
    pub(super) limit: Option<usize>,
    /// Also publish the labelled top-down overview PNG (default `true`):
    /// sector fills, sector numbers and things drawn, the picture
    /// `check_playability`'s `render` draws.
    #[serde(default)]
    pub(super) render: Option<bool>,
}

/// The sectors `describe_map` will report, ascending and deduplicated:
/// every stored index when `wanted` is unset, else `wanted` with each
/// index checked against the `count` of sectors `map` holds.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the offending index, the valid range
/// and the `sectors` parameter.
pub(super) fn sector_selection(
    wanted: Option<&[usize]>,
    count: usize,
    map: LumpName,
) -> Result<Vec<usize>, CyberError> {
    let Some(indices) = wanted else {
        return Ok((0..count).collect());
    };
    let mut chosen: BTreeSet<usize> = BTreeSet::new();
    for index in indices {
        check_index(ElementKind::Sectors, *index, count).map_err(|error| {
            error.map_message(|message| format!("`sectors` for map {map}: {message}"))
        })?;
        chosen.insert(*index);
    }
    Ok(chosen.into_iter().collect())
}

/// Structured result of [`crate::server::Cyberdemon::describe_map`].
#[derive(Debug, Serialize)]
pub(super) struct DescribeReport {
    /// The map that was described.
    map: LumpName,
    /// The map-wide overview, computed in full whatever the `sectors`
    /// filter and the paging say.
    summary: MapOverview,
    /// The described sector rows, in ascending index order, one page.
    sectors: Vec<SectorFacts>,
    /// The `offset` that returns the rows this page left behind, `null`
    /// when the page took everything.
    next_offset: Option<usize>,
    /// The labelled overview image, `null` unless `render` was asked for.
    image: Option<AssetRef>,
}

/// `describe_map`'s work: check the limit before the session is touched,
/// read the map and the session's definitions under one lock exactly like
/// [`check_run`](super::check::check_run) does, select and page the sector
/// rows, fold the always-whole map summary, and publish the labelled overview
/// through the render group's own machinery.
///
/// # Errors
///
/// [`crate::tools::parse_limit`]'s and [`sector_selection`]'s refusals, [`load`]'s
/// errors and - with `render` - the [`playability_overview`] refusals.
pub(super) async fn describe_run(
    state: &AppState,
    args: &DescribeMapParams,
) -> Result<DescribeReport, CyberError> {
    let limit = parse_limit(
        args.limit,
        DESCRIBE_DEFAULT_LIMIT,
        DESCRIBE_MAX_LIMIT,
        "limit",
    )?;
    let prepared = load(state, args.map.as_deref())?;
    let selected = sector_selection(
        args.sectors.as_deref(),
        prepared.map.sectors.len(),
        prepared.map.name,
    )?;
    let (page, next_offset) = Page::new(args.offset.unwrap_or(0), limit).window(&selected);
    let describe = Describe::new(&prepared.map, &prepared.defs);
    let sectors: Vec<SectorFacts> = page
        .iter()
        .filter_map(|index| describe.sector(*index))
        .collect();
    let summary = describe.summary();
    let image = if args.render.unwrap_or(true) {
        Some(playability_overview(state, args.map.as_deref(), OverviewMarks::default()).await?)
    } else {
        None
    };
    Ok(DescribeReport {
        map: prepared.map.name,
        summary,
        sectors,
        next_offset,
        image,
    })
}
