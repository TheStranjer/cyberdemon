//! The description side of the session group: what `get_session_info` and
//! `get_wad_info` report.
//!
//! [`describe_session`] folds the live session into [`SessionInfo`]: the
//! layer stack, the working archive with its path and dirty flag, the active
//! map, the detected game and both history stacks. [`describe_wad`] folds
//! the merged view an engine would load into [`WadInfo`]: the load-ordered
//! archive list, every map with the archive its block came from, the
//! namespace counts and the structural warnings [`crate::wad::classify`]
//! raises over the merged directory.
//!
//! The builders of both reports are private steps here; the shared
//! [`super::path_text`] and [`super::chronological`] readers come from the
//! root.

use alloc::collections::BTreeMap;

use serde::Serialize;

use crate::session::{Layer, LayerRole, MergedLump, Session, Source};
use crate::wad::classify::{Analysis, LumpKind, MapFormat};
use crate::wad::game::Game;
use crate::wad::{LumpName, WadKind};

use super::{UNSAVED_WORKING, chronological, path_text};

/// One read-only layer, as `get_session_info` lists the stack.
#[derive(Serialize)]
pub(super) struct LayerInfo {
    /// Position in the layer numbering: `0` is the base.
    index: usize,
    /// Whether this layer is the base game archive or a stacked resource.
    role: LayerRole,
    /// Where the archive was read from.
    path: String,
    /// The header magic of the file.
    kind: WadKind,
    /// Directory entries the archive holds.
    lumps: usize,
}

/// The base layer without its index and role, which `iwad` fixes anyway.
#[derive(Serialize)]
pub(super) struct IwadInfo {
    /// Where the archive was read from.
    path: String,
    /// The header magic of the file.
    kind: WadKind,
    /// Directory entries the archive holds.
    lumps: usize,
}

/// The working archive as `get_session_info` reports it.
#[derive(Serialize)]
pub(super) struct WorkingInfo {
    /// Where it was opened from or last saved, `null` when never saved.
    path: Option<String>,
    /// The header magic the archive carries.
    kind: WadKind,
    /// Directory entries it holds.
    lumps: usize,
    /// Whether it holds changes not yet saved to a file.
    dirty: bool,
}

/// Structured result of [`Cyberdemon::get_session_info`].
#[derive(Serialize)]
pub(super) struct SessionInfo {
    /// The base game layer, `null` while none is loaded.
    iwad: Option<IwadInfo>,
    /// Every read-only layer, index 0 first: the `IWAD` while loaded,
    /// then the resources in load order.
    layers: Vec<LayerInfo>,
    /// The editable working archive.
    working: WorkingInfo,
    /// The map editing tools should default to, `null` when none was picked.
    active_map: Option<LumpName>,
    /// The game the session's archives identify.
    game: Game,
    /// Labels of the edits undo would take back, oldest first, newest last.
    undo: Vec<String>,
    /// Labels of the edits redo would replay, oldest first, newest last.
    redo: Vec<String>,
}

/// One map of the merged view and where its winning copy came from.
#[derive(Serialize)]
pub(super) struct MapEntry {
    /// The map's lump name.
    name: LumpName,
    /// The layout its data lumps use.
    format: MapFormat,
    /// Which archive the merged view took the block from.
    source: Source,
}

/// Structured result of [`Cyberdemon::get_wad_info`].
#[derive(Serialize)]
pub(super) struct WadInfo {
    /// The game the session's archives identify.
    game: Game,
    /// The base layer's path, `null` while none is loaded.
    iwad: Option<String>,
    /// The resource layer paths in load order, then the working archive's
    /// path or [`UNSAVED_WORKING`].
    pwads: Vec<String>,
    /// Directory entries of the merged view.
    total_lumps: usize,
    /// Maps of the merged view, in directory order.
    maps: Vec<MapEntry>,
    /// Member-lump counts per namespace present in the merged view.
    namespaces: BTreeMap<&'static str, usize>,
    /// Structural warnings the merged directory raises.
    warnings: Vec<String>,
}

/// One layer of the stack as `get_session_info` shows it.
fn layer_info(index: usize, layer: &Layer) -> LayerInfo {
    LayerInfo {
        index,
        role: layer.role,
        path: path_text(&layer.path),
        kind: layer.wad.kind,
        lumps: layer.wad.len(),
    }
}

/// The whole session as `get_session_info` reports it.
pub(super) fn describe_session(session: &Session) -> SessionInfo {
    SessionInfo {
        iwad: session.iwad().map(|layer| IwadInfo {
            path: path_text(&layer.path),
            kind: layer.wad.kind,
            lumps: layer.wad.len(),
        }),
        layers: session
            .layers()
            .iter()
            .enumerate()
            .map(|(index, layer)| layer_info(index, layer))
            .collect(),
        working: WorkingInfo {
            path: session.working_path().map(path_text),
            kind: session.working().kind,
            lumps: session.working().len(),
            dirty: session.is_dirty(),
        },
        active_map: session.active_map(),
        game: session.game(),
        undo: chronological(session.undo_labels()),
        redo: chronological(session.redo_labels()),
    }
}

/// The archives an engine loads on top of the base, working archive last.
fn merged_pwads(session: &Session) -> Vec<String> {
    let mut paths: Vec<String> = session
        .layers()
        .into_iter()
        .filter(|layer| layer.role == LayerRole::Resource)
        .map(|layer| path_text(&layer.path))
        .collect();
    paths.push(
        session
            .working_path()
            .map_or_else(|| UNSAVED_WORKING.to_owned(), path_text),
    );
    paths
}

/// The map blocks of the merged view with the archive each came from.
fn merged_maps(view: &[MergedLump], analysis: &Analysis) -> Vec<MapEntry> {
    analysis
        .maps
        .iter()
        .filter_map(|block| {
            view.get(block.header).map(|entry| MapEntry {
                name: block.name,
                format: block.format,
                source: entry.source,
            })
        })
        .collect()
}

/// Member-lump counts per namespace, skipping the region markers.
fn namespace_counts(analysis: &Analysis) -> BTreeMap<&'static str, usize> {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for info in &analysis.lumps {
        let Some(namespace) = info.namespace else {
            continue;
        };
        if info.kind == LumpKind::Marker {
            continue;
        }
        let slot = counts.entry(namespace.as_str()).or_insert(0_usize);
        *slot = slot.saturating_add(1);
    }
    counts
}

/// The merged view as `get_wad_info` reports it.
pub(super) fn describe_wad(session: &Session) -> WadInfo {
    let view = session.merged();
    let analysis = session.analysis();
    WadInfo {
        game: session.game(),
        iwad: session.iwad().map(|layer| path_text(&layer.path)),
        pwads: merged_pwads(session),
        total_lumps: view.len(),
        maps: merged_maps(&view, &analysis),
        namespaces: namespace_counts(&analysis),
        warnings: analysis.warnings,
    }
}
