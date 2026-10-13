//! Session tools: loading archives into the editing session and describing
//! what the session holds.

use alloc::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::merge::{MergeCounts, merge_counts, merge_into};
use crate::session::{Layer, LayerRole, MergedLump, Session, Source};
use crate::state::AppState;
use crate::tools::{fail, ok};
use crate::wad::archive::{Wad, write_file_atomic};
use crate::wad::classify::{Analysis, LumpKind, MapFormat, analyze};
use crate::wad::game::Game;
use crate::wad::{Lump, LumpName, WadKind};

/// The placeholder the `pwads` list shows for an unsaved working archive.
const UNSAVED_WORKING: &str = "(unsaved working archive)";

/// Arguments for [`Cyberdemon::open_iwad`].
#[derive(Debug, Deserialize, JsonSchema)]
struct OpenIwadParams {
    /// Absolute filesystem path of the archive to load as the base game
    /// layer; either `IWAD` or `PWAD` magic is accepted.
    path: String,
}

/// Arguments for [`Cyberdemon::open_wad`].
#[derive(Debug, Deserialize, JsonSchema)]
struct OpenWadParams {
    /// Absolute filesystem path of the archive to open as the working
    /// archive.
    path: String,
    /// Replace the working archive even when it holds unsaved changes;
    /// without this a dirty working archive refuses the open.
    #[serde(default)]
    discard_changes: bool,
}

/// Arguments for [`Cyberdemon::new_wad`].
#[derive(Debug, Deserialize, JsonSchema)]
struct NewWadParams {
    /// Replace the working archive even when it holds unsaved changes.
    #[serde(default)]
    discard_changes: bool,
}

/// Arguments for [`Cyberdemon::merge_wad`].
#[derive(Debug, Deserialize, JsonSchema)]
struct MergeWadParams {
    /// Absolute filesystem path of the archive to merge.
    path: String,
    /// `"layer"` stacks the file as a read-only resource layer (the
    /// default); `"working"` folds its lumps into the working archive as
    /// one undoable edit.
    #[serde(default)]
    mode: Option<String>,
}

/// How [`Cyberdemon::merge_wad`] combines the file with the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum MergeMode {
    /// Stack the archive as a read-only resource layer.
    #[default]
    Layer,
    /// Fold the archive's lumps into the working archive.
    Working,
}

/// The mode names the `merge_wad` `mode` parameter accepts, `None` for the
/// default. Anything else is refused with a hint naming both forms.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any text other than `layer` or
/// `working`, in any case.
fn parse_mode(text: Option<&str>) -> Result<MergeMode, CyberError> {
    let Some(raw) = text else {
        return Ok(MergeMode::Layer);
    };
    let lowered = raw.to_ascii_lowercase();
    if lowered == "layer" {
        return Ok(MergeMode::Layer);
    }
    if lowered == "working" {
        return Ok(MergeMode::Working);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown merge mode {raw:?}: use \"layer\" to stack the file read-only or \"working\" \
         to merge its lumps into the working archive"
    )))
}

/// Arguments for [`Cyberdemon::save_wad`].
#[derive(Debug, Deserialize, JsonSchema)]
struct SaveWadParams {
    /// Absolute filesystem path to write. Defaults to the working archive's
    /// path (set by `open_wad` or a previous `save_wad`); without either the
    /// call fails with `invalid_argument`.
    #[serde(default)]
    path: Option<String>,
    /// `"pwad"` (the default) writes the working archive and marks it saved;
    /// `"full"` exports the entire merged view as one standalone WAD without
    /// touching the working path or the dirty flag.
    #[serde(default)]
    mode: Option<String>,
    /// Header magic to write, `"PWAD"` or `"IWAD"`. Defaults to the working
    /// archive's kind for mode `pwad`; for `full`, `IWAD` when an IWAD layer
    /// is loaded, else `PWAD`.
    #[serde(default)]
    kind: Option<String>,
}

/// How [`Cyberdemon::save_wad`] decides what to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum SaveMode {
    /// Write the working archive and record it as saved.
    Pwad,
    /// Export the merged view; the session keeps its working path and dirty
    /// state untouched.
    Full,
}

/// The mode names the `save_wad` `mode` parameter accepts, `None` for the
/// default. Anything else is refused with a hint naming both forms.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any text other than `pwad` or
/// `full`, in any case.
fn parse_save_mode(text: Option<&str>) -> Result<SaveMode, CyberError> {
    let Some(raw) = text else {
        return Ok(SaveMode::Pwad);
    };
    let lowered = raw.to_ascii_lowercase();
    if lowered == "pwad" {
        return Ok(SaveMode::Pwad);
    }
    if lowered == "full" {
        return Ok(SaveMode::Full);
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown save mode {raw:?}: use \"pwad\" to save the working archive or \"full\" to \
         export the merged view as one standalone WAD"
    )))
}

/// The header magic the `save_wad` `kind` parameter selects, `None` for the
/// mode's default.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for any text other than `pwad` or
/// `iwad`, in any case.
fn parse_kind(text: Option<&str>) -> Result<Option<WadKind>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    let lowered = raw.to_ascii_lowercase();
    if lowered == "pwad" {
        return Ok(Some(WadKind::Pwad));
    }
    if lowered == "iwad" {
        return Ok(Some(WadKind::Iwad));
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown kind {raw:?}: use \"PWAD\" for a patch archive or \"IWAD\" for a full game \
         archive, or leave the parameter out for the default magic"
    )))
}

/// A whole archive as the load tools report it.
struct ArchiveSummary {
    /// The path it was read from, as text.
    path: String,
    /// The header magic found in the file.
    kind: WadKind,
    /// Number of directory entries.
    lumps: usize,
    /// Number of map blocks it declares.
    maps: usize,
}

impl ArchiveSummary {
    /// Classify `wad` at `path` into the fields the load reports carry.
    fn of(path: &Path, wad: &Wad) -> Self {
        Self {
            path: path_text(path),
            kind: wad.kind,
            lumps: wad.len(),
            maps: analyze(&wad.lumps).maps.len(),
        }
    }
}

/// Structured result of [`Cyberdemon::open_iwad`].
#[derive(Serialize)]
struct IwadReport {
    /// The file that became the base layer.
    path: String,
    /// The header magic found in the file.
    kind: WadKind,
    /// Directory entries the archive holds.
    lumps: usize,
    /// Map blocks in the archive.
    maps: usize,
    /// The game the session reports after the load.
    game: Game,
}

/// Structured result of [`Cyberdemon::open_wad`].
#[derive(Serialize)]
struct OpenWadReport {
    /// The file that became the working archive.
    path: String,
    /// The header magic found in the file.
    kind: WadKind,
    /// Directory entries the archive holds.
    lumps: usize,
    /// Map blocks in the archive.
    maps: usize,
    /// The game the session reports after the open.
    game: Game,
    /// Whether the working archive now holds unsaved changes (never does
    /// right after an open).
    dirty: bool,
}

/// Structured result of [`Cyberdemon::new_wad`].
#[derive(Serialize)]
struct NewWadReport {
    /// The kind of the fresh archive (always `PWAD`).
    kind: WadKind,
    /// Directory entries it holds (always `0`).
    lumps: usize,
}

/// Structured result of [`Cyberdemon::merge_wad`], one shape per mode.
#[derive(Serialize)]
#[serde(untagged)]
enum MergeReport {
    /// The file went onto the read-only layer stack.
    Layer {
        /// Which mode ran.
        mode: MergeMode,
        /// Index the new layer took in the session's layer numbering.
        layer_index: usize,
        /// Directory entries the layered archive holds.
        lumps: usize,
    },
    /// The file was folded into the working archive.
    Working {
        /// Which mode ran.
        mode: MergeMode,
        /// Base entries the overlay replaced (a whole map block counts
        /// as one).
        replaced: usize,
        /// Entries the overlay brought that the working archive lacked
        /// (a whole new map block counts as one).
        added: usize,
    },
}

/// Structured result of [`Cyberdemon::save_wad`].
#[derive(Serialize)]
struct SaveReport {
    /// The file that was written.
    path: String,
    /// Which mode ran.
    mode: SaveMode,
    /// The header magic written into the file.
    kind: WadKind,
    /// Directory entries in the file.
    lumps: usize,
    /// Bytes in the file.
    bytes: usize,
    /// Whether the working archive still holds unsaved changes (`false`
    /// after a `pwad` save, unchanged after a `full` export).
    dirty: bool,
}

/// One read-only layer, as `get_session_info` lists the stack.
#[derive(Serialize)]
struct LayerInfo {
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
struct IwadInfo {
    /// Where the archive was read from.
    path: String,
    /// The header magic of the file.
    kind: WadKind,
    /// Directory entries the archive holds.
    lumps: usize,
}

/// The working archive as `get_session_info` reports it.
#[derive(Serialize)]
struct WorkingInfo {
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
struct SessionInfo {
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
struct MapEntry {
    /// The map's lump name.
    name: LumpName,
    /// The layout its data lumps use.
    format: MapFormat,
    /// Which archive the merged view took the block from.
    source: Source,
}

/// Structured result of [`Cyberdemon::get_wad_info`].
#[derive(Serialize)]
struct WadInfo {
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

/// A path as report text.
fn path_text(path: &Path) -> String {
    path.display().to_string()
}

/// Validate an absolute path argument and read the archive it names.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a relative path,
/// [`CyberError::NotFound`] for a missing file, [`CyberError::Io`] for any
/// other read failure and whatever [`Wad::read_file`] reports for contents
/// that are not a valid `WAD`.
fn read_archive(text: &str) -> Result<(PathBuf, Wad), CyberError> {
    let path = Path::new(text);
    if !path.is_absolute() {
        return Err(CyberError::InvalidArgument(format!(
            "path {text:?} is not absolute; pass the full filesystem path of the WAD file"
        )));
    }
    let wad = Wad::read_file(path).map_err(|error| open_error(path, error))?;
    Ok((path.to_path_buf(), wad))
}

/// A failed archive read: a missing file is `not_found`, other I/O stays as
/// the `io_error` [`Wad::read_file`] reports, and format failures pass
/// through untouched.
fn open_error(path: &Path, error: CyberError) -> CyberError {
    let missing = matches!(error, CyberError::Io(_)) && matches!(path.try_exists(), Ok(false));
    if missing {
        return CyberError::NotFound(format!("no file at {}", path.display()));
    }
    error
}

/// Refuse to drop unsaved work unless the caller passed `discard_changes`.
///
/// # Errors
///
/// [`CyberError::Conflict`] naming both escapes when the working archive is
/// dirty and `discard_changes` is `false`.
fn ensure_clean(session: &Session, discard_changes: bool) -> Result<(), CyberError> {
    if session.is_dirty() && !discard_changes {
        return Err(CyberError::Conflict(
            "unsaved changes in the working archive; save_wad first or pass discard_changes: \
             true to replace it anyway"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The history labels in chronological order: the session stacks report
/// them newest first, the tools show them newest last.
fn chronological(labels: Vec<&str>) -> Vec<String> {
    labels.into_iter().rev().map(str::to_owned).collect()
}

/// Hand an outcome to the client as the documented structured result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

/// `open_iwad`'s work: load the file and make it the session's base layer.
fn open_iwad_report(state: &AppState, text: &str) -> Result<IwadReport, CyberError> {
    let (path, wad) = read_archive(text)?;
    let summary = ArchiveSummary::of(&path, &wad);
    state.with_session(|session| {
        session.set_iwad(Layer::new(path, LayerRole::Iwad, wad));
        Ok(IwadReport {
            path: summary.path,
            kind: summary.kind,
            lumps: summary.lumps,
            maps: summary.maps,
            game: session.game(),
        })
    })
}

/// `open_wad`'s work: load the file and adopt it as the working archive.
fn open_wad_report(
    state: &AppState,
    text: &str,
    discard_changes: bool,
) -> Result<OpenWadReport, CyberError> {
    let (path, wad) = read_archive(text)?;
    let summary = ArchiveSummary::of(&path, &wad);
    state.with_session(|session| {
        ensure_clean(session, discard_changes)?;
        session.replace_working(wad, Some(path));
        Ok(OpenWadReport {
            path: summary.path,
            kind: summary.kind,
            lumps: summary.lumps,
            maps: summary.maps,
            game: session.game(),
            dirty: session.is_dirty(),
        })
    })
}

/// `new_wad`'s work: adopt a fresh empty `PWAD` as the working archive.
fn new_wad_report(state: &AppState, discard_changes: bool) -> Result<NewWadReport, CyberError> {
    state.with_session(|session| {
        ensure_clean(session, discard_changes)?;
        session.replace_working(Wad::default(), None);
        Ok(NewWadReport {
            kind: session.working().kind,
            lumps: session.working().len(),
        })
    })
}

/// `merge_wad` mode `layer`: stack the file above the existing layers.
fn merge_layer_report(
    state: &AppState,
    path: PathBuf,
    wad: &Wad,
) -> Result<MergeReport, CyberError> {
    let lumps = wad.len();
    state.with_session(|session| {
        let layer_index = session.layers().len();
        session.add_resource(Layer::new(path, LayerRole::Resource, wad.clone()));
        Ok(MergeReport::Layer {
            mode: MergeMode::Layer,
            layer_index,
            lumps,
        })
    })
}

/// `merge_wad` mode `working`: fold the file into the working archive as
/// one undoable edit.
fn merge_working_report(
    state: &AppState,
    path: &Path,
    overlay: &Wad,
) -> Result<MergeReport, CyberError> {
    let file_name = path.file_name().map_or_else(
        || path_text(path),
        |name| name.to_string_lossy().into_owned(),
    );
    let label = format!("merge_wad {file_name}");
    state.with_session(|session| {
        let base = session.working();
        let MergeCounts { replaced, added } = merge_counts(base, overlay);
        let merged = merge_into(base, overlay);
        session.edit(&label, |working| {
            *working = merged;
            Ok(())
        })?;
        Ok(MergeReport::Working {
            mode: MergeMode::Working,
            replaced,
            added,
        })
    })
}

/// `merge_wad`'s work: parse the mode, read the file, then run one mode.
fn merge_wad_report(
    state: &AppState,
    text: &str,
    mode: Option<&str>,
) -> Result<MergeReport, CyberError> {
    let wanted = parse_mode(mode)?;
    let (path, wad) = read_archive(text)?;
    match wanted {
        MergeMode::Layer => merge_layer_report(state, path, &wad),
        MergeMode::Working => merge_working_report(state, &path, &wad),
    }
}

/// The file `save_wad` will write: the `path` argument when given (it must
/// be absolute), else the working archive's remembered path.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a relative `path`, and for no `path`
/// on a session that never opened or saved to a file.
fn resolve_target(session: &Session, text: Option<&str>) -> Result<PathBuf, CyberError> {
    let Some(raw) = text else {
        return session
            .working_path()
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                CyberError::InvalidArgument(
                    "no path: pass path, the working archive was never opened from or saved to \
                     a file"
                        .to_owned(),
                )
            });
    };
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(CyberError::InvalidArgument(format!(
            "path {raw:?} is not absolute; pass the full filesystem path to save to"
        )));
    }
    Ok(path.to_path_buf())
}

/// The canonical form of `path` when a file stands there, for same-file
/// comparisons; `None` when it is absent or cannot be resolved.
fn canonical_existing(path: &Path) -> Option<PathBuf> {
    if !path.try_exists().is_ok_and(|exists| exists) {
        return None;
    }
    path.canonicalize().ok()
}

/// The layer's role as text, for the overwrite refusal.
const fn role_label(role: LayerRole) -> &'static str {
    match role {
        LayerRole::Iwad => "IWAD",
        LayerRole::Resource => "resource",
    }
}

/// Refuse to overwrite the file one of the session's read-only layers reads
/// from, compared by canonical path whenever the target exists.
///
/// # Errors
///
/// [`CyberError::Conflict`] naming the layer and the escape hatch when
/// `target` is a loaded layer's file.
fn ensure_not_layer_file(session: &Session, target: &Path) -> Result<(), CyberError> {
    let Some(resolved) = canonical_existing(target) else {
        return Ok(());
    };
    for layer in session.layers() {
        let same_file = canonical_existing(&layer.path).is_some_and(|file| file == resolved);
        if same_file {
            return Err(CyberError::Conflict(format!(
                "{} is the session's {} layer at {}; pass a different path",
                target.display(),
                role_label(layer.role),
                layer.path.display(),
            )));
        }
    }
    Ok(())
}

/// The archive `save_wad` mode `pwad` writes: the working archive, its magic
/// overridden when `kind` says so.
fn working_copy(session: &Session, kind: Option<WadKind>) -> Wad {
    let mut wad = session.working().clone();
    if let Some(wanted) = kind {
        wad.kind = wanted;
    }
    wad
}

/// The archive `save_wad` mode `full` writes: the whole merged view as one
/// standalone `Wad`, `IWAD`-magic by default while a base layer is loaded.
fn merged_export(session: &Session, kind: Option<WadKind>) -> Wad {
    let view = session.merged();
    let default = session.iwad().map_or(WadKind::Pwad, |_| WadKind::Iwad);
    Wad {
        kind: kind.unwrap_or(default),
        lumps: view.into_iter().map(|entry| entry.lump).collect(),
    }
}

/// `save_wad`'s work: resolve path, mode and kind, write the file and, in
/// mode `pwad`, record it as the working archive's saved state.
fn save_wad_report(
    state: &AppState,
    text: Option<&str>,
    mode: Option<&str>,
    kind: Option<&str>,
) -> Result<SaveReport, CyberError> {
    let wanted_mode = parse_save_mode(mode)?;
    let wanted_kind = parse_kind(kind)?;
    state.with_session(|session| {
        let target = resolve_target(session, text)?;
        ensure_not_layer_file(session, &target)?;
        let wad = match wanted_mode {
            SaveMode::Pwad => working_copy(session, wanted_kind),
            SaveMode::Full => merged_export(session, wanted_kind),
        };
        let bytes = wad.to_bytes()?;
        write_file_atomic(&target, &bytes)?;
        if wanted_mode == SaveMode::Pwad {
            session.mark_saved(target.clone());
        }
        Ok(SaveReport {
            path: path_text(&target),
            mode: wanted_mode,
            kind: wad.kind,
            lumps: wad.len(),
            bytes: bytes.len(),
            dirty: session.is_dirty(),
        })
    })
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
fn describe_session(session: &Session) -> SessionInfo {
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
fn describe_wad(session: &Session) -> WadInfo {
    let view = session.merged();
    let lumps: Vec<Lump> = view.iter().map(|entry| entry.lump.clone()).collect();
    let analysis = analyze(&lumps);
    WadInfo {
        game: session.game(),
        iwad: session.iwad().map(|layer| path_text(&layer.path)),
        pwads: merged_pwads(session),
        total_lumps: view.len(),
        maps: merged_maps(&view, &analysis),
        namespaces: namespace_counts(&analysis),
        warnings: analysis.warnings.clone(),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = session_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Loads a file as the session's base game (`IWAD`) layer.
    #[tool(
        description = "Reads the WAD at an absolute path and makes it the session's read-only base game layer, replacing any previous IWAD while keeping the resource layers and the working archive. Accepts IWAD or PWAD magic. Returns {path, kind, lumps, maps, game}."
    )]
    fn open_iwad(
        &self,
        Parameters(args): Parameters<OpenIwadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(open_iwad_report(self.state(), &args.path))
    }

    /// Opens a file as the editable working archive.
    #[tool(
        description = "Reads the WAD at an absolute path and adopts it as the editable working archive, forgetting the previous archive's undo history. Refuses with a conflict while the working archive has unsaved changes unless discard_changes is true. Returns {path, kind, lumps, maps, game, dirty}."
    )]
    fn open_wad(
        &self,
        Parameters(args): Parameters<OpenWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(open_wad_report(
            self.state(),
            &args.path,
            args.discard_changes,
        ))
    }

    /// Starts a fresh empty working archive.
    #[tool(
        description = "Replaces the working archive with an empty PWAD that has no file behind it yet (save_wad gives it one). Refuses with a conflict while the working archive has unsaved changes unless discard_changes is true. Returns {kind, lumps}."
    )]
    fn new_wad(
        &self,
        Parameters(args): Parameters<NewWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(new_wad_report(self.state(), args.discard_changes))
    }

    /// Merges a file in as a resource layer or into the working archive.
    #[tool(
        description = "Reads the WAD at an absolute path and combines it with the session. mode \"layer\" (the default) stacks it as a read-only resource layer below the working archive and returns {mode, layer_index, lumps}; mode \"working\" folds its lumps into the working archive under the merged view's override rules as one undoable edit and returns {mode, replaced, added}. Paths are absolute or the call fails with invalid_argument."
    )]
    fn merge_wad(
        &self,
        Parameters(args): Parameters<MergeWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(merge_wad_report(
            self.state(),
            &args.path,
            args.mode.as_deref(),
        ))
    }

    /// Writes the working archive or the whole merged view to a file.
    #[tool(
        description = "Saves the session to a file. mode \"pwad\" (the default) writes the working archive and marks it saved (the working path becomes the file written); mode \"full\" exports the entire merged view (IWAD layer, resource layers and working archive under the override rules) as one standalone WAD without changing the working path or the dirty flag. path is absolute and defaults to the working archive's own path; kind (\"PWAD\"|\"IWAD\") overrides the header magic. Refuses with conflict to overwrite a loaded layer's file. Returns {path, mode, kind, lumps, bytes, dirty}."
    )]
    fn save_wad(
        &self,
        Parameters(args): Parameters<SaveWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(save_wad_report(
            self.state(),
            args.path.as_deref(),
            args.mode.as_deref(),
            args.kind.as_deref(),
        ))
    }

    /// Describes the session's archives, history and state.
    #[tool(
        description = "Reports the whole session: the base IWAD layer (or null), every read-only layer with its index and role, the working archive with its path and dirty flag, the active map, the detected game, and the undo/redo label stacks oldest first (newest last). Takes no arguments."
    )]
    fn get_session_info(&self) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| Ok(describe_session(session)));
        reported(report)
    }

    /// Summarizes the merged view an engine would load.
    #[tool(
        description = "Reports the merged view of the whole session: detected game, base path, the load-ordered archive list, total lump count, every map with its format and winning source, per-namespace member counts and structural warnings. Takes no arguments."
    )]
    fn get_wad_info(&self) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| Ok(describe_wad(session)));
        reported(report)
    }
}
