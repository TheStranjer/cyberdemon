//! The multi-map half of the bulk-edit machinery: one pure per-map pass
//! over a [`super::MapsSelector`] selection, run as a single
//! [`Session::edit`] step and folded into the [`ReplaceReport`] the
//! `replace_*` tools answer with.
//!
//! [`apply_to_maps`] is the step: every named block's copy into the
//! working archive and every rewrite share one history entry, a refusal
//! mid-loop commits nothing and records nothing, and
//! [`ApplyOptions::dry_run`] previews the same pass over merged-view
//! clones. [`run_replacement`] is the tool-shaped wrapper above it: it
//! [`resolve`]s the selection, runs the pass and folds the per-map
//! counts, the copied maps and the mode-dependent `next_steps` into one
//! [`Replacement`]-described report.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use serde::Serialize;

use super::{MapsSelector, resolve};
use crate::error::CyberError;
use crate::session::map_edit;
use crate::session::{MergedLump, Scope, Session};
use crate::tools::maps::common;
use crate::wad::archive::Wad;
use crate::wad::map::Map;
use crate::wad::name::LumpName;

/// How one [`apply_to_maps`] run goes: the label of the single undo
/// step and whether the run touches the session at all.
#[derive(Clone, Copy, Debug)]
pub struct ApplyOptions<'lbl> {
    /// The label recorded as the one undo entry (a [`Self::dry_run`]
    /// records no entry at all).
    pub label: &'lbl str,
    /// Apply the pass to merged-view clones: write nothing, record
    /// nothing, copy nothing.
    pub dry_run: bool,
}

/// What one [`apply_to_maps`] run answered, map by map.
#[derive(Debug)]
pub struct MapReplaceRun<T> {
    /// The pass's result for every named map, in the order [`resolve`]
    /// produced it.
    pub per_map: Vec<(LumpName, T)>,
    /// The maps that had to be copied from the read-only layers into
    /// the working archive before they could be edited, one name per
    /// [`map_edit::MapEditReport::copied_to_working`]; always empty for
    /// a [`ApplyOptions::dry_run`].
    pub copied_maps: Vec<String>,
}

/// Runs `f` over every named map block as one undoable step: the same
/// [`Session::edit`] wrapper the whole-set node builds use.
///
/// The shared single-map edit step runs once per block, so every
/// block's copy-in and rewrite share one history entry, and a refusal
/// anywhere leaves the session exactly as it was: nothing is committed
/// and no entry is recorded. With [`ApplyOptions::dry_run`] nothing is
/// written: `f` runs on the merged-view clone of each map instead, and
/// a map the merged view lacks refuses just like the editing path. An
/// empty `names` list writes nothing and records no entry either.
///
/// # Errors
///
/// The single-map edit step's refusals ([`CyberError::NotFound`] for a
/// map no scope holds, and the map writer's encoder errors) and the
/// merged-view read's, unchanged.
#[inline]
pub fn apply_to_maps<T, F>(
    session: &mut Session,
    names: &[LumpName],
    options: &ApplyOptions<'_>,
    f: F,
) -> Result<MapReplaceRun<T>, CyberError>
where
    F: Fn(&mut Map) -> T + Copy,
{
    if options.dry_run {
        return dry_run(session, names, f);
    }
    if names.is_empty() {
        return Ok(MapReplaceRun {
            per_map: Vec::new(),
            copied_maps: Vec::new(),
        });
    }
    let merged = session.merged();
    let answered = session.edit(options.label, |wad| edit_each(wad, &merged, names, f))?;
    Ok(assembled(answered))
}

/// One named map's answer: the pass's value plus whether the block
/// had to be copied into the working archive.
type Answered<T> = Vec<(LumpName, T, bool)>;

/// The [`Session::edit`] body: one `edit_in_place` per named block,
/// each answer recorded beside whether its block had to be copied in.
fn edit_each<T, F>(
    wad: &mut Wad,
    merged: &[MergedLump],
    names: &[LumpName],
    f: F,
) -> Result<Answered<T>, CyberError>
where
    F: Fn(&mut Map) -> T + Copy,
{
    let mut answered = Vec::new();
    for name in names {
        let (value, report) = map_edit::edit_in_place(wad, merged, *name, |map| Ok(f(map)))?;
        answered.push((*name, value, report.copied_to_working));
    }
    Ok(answered)
}

/// Folds one run's per-map answers into the report.
fn assembled<T>(answered: Answered<T>) -> MapReplaceRun<T> {
    let mut run = MapReplaceRun {
        per_map: Vec::new(),
        copied_maps: Vec::new(),
    };
    for (name, value, copied) in answered {
        if copied {
            run.copied_maps.push(name.to_string());
        }
        run.per_map.push((name, value));
    }
    run
}

/// The dry pass: `f` over the merged-view clone of each named map.
fn dry_run<T, F>(
    session: &Session,
    names: &[LumpName],
    f: F,
) -> Result<MapReplaceRun<T>, CyberError>
where
    F: Fn(&mut Map) -> T,
{
    let mut per_map = Vec::new();
    for name in names {
        let mut map = common::load_map(session, *name, Scope::Merged)?;
        per_map.push((*name, f(&mut map)));
    }
    Ok(MapReplaceRun {
        per_map,
        copied_maps: Vec::new(),
    })
}

/// The `next_steps` every written [`ReplaceReport`] carries.
const APPLIED_NEXT_STEPS: [&str; 3] = [
    "undo takes the whole swap back as one step",
    "find_elements over the old name finds any field the glob missed",
    "render_topdown looks at the rewritten maps",
];

/// The `next_steps` a previewed [`ReplaceReport`] carries.
const DRY_NEXT_STEPS: [&str; 2] = [
    "nothing was written: run again with dry_run false to apply the swap",
    "find_elements lists the matching elements one by one",
];

/// The per-tool knobs one [`run_replacement`] step needs beside the pass:
/// the label of the single undo entry (already interpolated by the tool)
/// and the run's shape.
#[derive(Clone, Copy, Debug)]
pub struct Replacement<'lbl> {
    /// The label recorded as the one undo entry (never recorded on a
    /// [`Self::dry_run`] preview).
    pub label: &'lbl str,
    /// The `maps` selector; `None` takes the session's default map,
    /// exactly as [`resolve`] documents it.
    pub maps: Option<&'lbl MapsSelector>,
    /// Report the counts without writing anything or touching history.
    pub dry_run: bool,
}

/// The structured result the `replace_*` tools report: the glob swaps
/// return it directly, and `replace_thing_type` folds it into its own
/// report beside the resolved numbers.
#[derive(Debug, Serialize)]
pub struct ReplaceReport {
    /// The fields the replacement changed, keyed by map block name as
    /// text and summed per block; every selected map holds an entry.
    pub maps: BTreeMap<String, usize>,
    /// The sum of [`Self::maps`]' values.
    pub total: usize,
    /// The maps the call had to copy from the read-only layers into
    /// the working archive; empty for a [`Self::dry_run`] report.
    pub copied_maps: Vec<String>,
    /// Whether the run only previewed.
    pub dry_run: bool,
    /// What to do or look at next, per [`Self::dry_run`].
    pub next_steps: Vec<&'static str>,
}

impl ReplaceReport {
    /// Fold one finished [`MapReplaceRun`] into the tools' report: the
    /// per-map counts become the `maps` object, `total` sums them, and
    /// the `next_steps` follow the run's mode.
    fn folded(run: MapReplaceRun<usize>, dry_run: bool) -> Self {
        let maps: BTreeMap<String, usize> = run
            .per_map
            .into_iter()
            .map(|(name, count)| (name.to_string(), count))
            .collect();
        let total = maps.values().sum();
        let steps: &[&'static str] = if dry_run {
            &DRY_NEXT_STEPS
        } else {
            &APPLIED_NEXT_STEPS
        };
        Self {
            maps,
            total,
            copied_maps: run.copied_maps,
            dry_run,
            next_steps: steps.to_vec(),
        }
    }
}

/// Run one pure per-map count (`bulk::replace_textures`,
/// `bulk::replace_flats` or any other `Fn(&mut Map) -> usize`) over the
/// [`Replacement::maps`] selection.
///
/// As [`apply_to_maps`] documents: one undoable step copying every
/// layer-only map into the working archive first, or — with
/// [`Replacement::dry_run`] — a preview over merged-view clones that
/// writes and records nothing.
///
/// # Errors
///
/// [`resolve`]'s refusals for a bad selection and [`apply_to_maps`]'
/// (through the single-map edit step: [`CyberError::NotFound`] for a map
/// no scope holds, the map writer's encoder errors).
#[inline]
pub fn run_replacement<F>(
    session: &mut Session,
    replacement: &Replacement<'_>,
    pass: F,
) -> Result<ReplaceReport, CyberError>
where
    F: Fn(&mut Map) -> usize + Copy,
{
    let names = resolve(session, replacement.maps)?;
    let options = ApplyOptions {
        label: replacement.label,
        dry_run: replacement.dry_run,
    };
    let run = apply_to_maps(session, &names, &options, pass)?;
    Ok(ReplaceReport::folded(run, replacement.dry_run))
}
