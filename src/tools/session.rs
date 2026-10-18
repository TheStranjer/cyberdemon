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
use crate::session::history::HISTORY_LIMIT;
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

/// Arguments for [`Cyberdemon::undo`].
#[derive(Debug, Deserialize, JsonSchema)]
struct UndoParams {
    /// Number of edits to step back over, `1..=100`; default `1`. When
    /// fewer edits are on the undo stack, undo steps over what exists.
    #[serde(default)]
    steps: Option<usize>,
}

/// Arguments for [`Cyberdemon::redo`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RedoParams {
    /// Number of undone edits to replay, `1..=100`; default `1`. When
    /// fewer edits are on the redo stack, redo replays what exists.
    #[serde(default)]
    steps: Option<usize>,
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
    /// Write each distinct non-empty lump payload once, with the duplicate
    /// directory entries sharing the first copy's offset (default `false`).
    /// Engines read shared offsets fine; the result then reports how many
    /// bytes the sharing saved.
    #[serde(default)]
    dedupe: bool,
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
    /// Payload bytes saved by writing duplicate lump data once, `0` unless
    /// the call passed `dedupe`.
    bytes_saved_by_dedupe: usize,
    /// Whether the working archive still holds unsaved changes (`false`
    /// after a `pwad` save, unchanged after a `full` export).
    dirty: bool,
}

/// Structured result of [`Cyberdemon::undo`].
#[derive(Debug, Serialize)]
struct UndoReport {
    /// Labels of the edits stepped back over, most recent first.
    undone: Vec<String>,
    /// Entries the undo stack still holds.
    remaining_undo: usize,
    /// Entries on the redo stack, how far `redo` can replay.
    redo_available: usize,
    /// Whether the working archive now differs from its last saved state.
    dirty: bool,
    /// Whether the active map was cleared because the merged view no
    /// longer holds it.
    active_map_cleared: bool,
}

/// Structured result of [`Cyberdemon::redo`].
#[derive(Debug, Serialize)]
struct RedoReport {
    /// Labels of the edits replayed, in the order they were re-applied.
    redone: Vec<String>,
    /// Entries the redo stack still holds.
    remaining_redo: usize,
    /// Entries on the undo stack, how far `undo` can step back.
    undo_available: usize,
    /// Whether the working archive now differs from its last saved state.
    dirty: bool,
    /// Whether the active map was cleared because the merged view no
    /// longer holds it.
    active_map_cleared: bool,
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
/// mode `pwad`, record it as the working archive's saved state. With
/// `dedupe` the write shares duplicate payloads and the report carries the
/// bytes saved.
fn save_wad_report(state: &AppState, args: &SaveWadParams) -> Result<SaveReport, CyberError> {
    let wanted_mode = parse_save_mode(args.mode.as_deref())?;
    let wanted_kind = parse_kind(args.kind.as_deref())?;
    let dedupe = args.dedupe;
    state.with_session(|session| {
        let target = resolve_target(session, args.path.as_deref())?;
        ensure_not_layer_file(session, &target)?;
        let wad = match wanted_mode {
            SaveMode::Pwad => working_copy(session, wanted_kind),
            SaveMode::Full => merged_export(session, wanted_kind),
        };
        let (bytes, bytes_saved_by_dedupe) = if dedupe {
            wad.to_bytes_dedup()?
        } else {
            (wad.to_bytes()?, 0_usize)
        };
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
            bytes_saved_by_dedupe,
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

/// The `steps` argument [`Cyberdemon::undo`] and [`Cyberdemon::redo`]
/// accept: absent means `1`, and only counts up to [`HISTORY_LIMIT`] pass.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the accepted range for `0` or a
/// count past the history limit.
fn parse_steps(steps: Option<usize>) -> Result<usize, CyberError> {
    let Some(count) = steps else {
        return Ok(1_usize);
    };
    if count == 0 || count > HISTORY_LIMIT {
        return Err(CyberError::InvalidArgument(format!(
            "steps {count} is outside 1..={HISTORY_LIMIT}: pass the number of edits to step \
             through, or leave steps out for 1"
        )));
    }
    Ok(count)
}

/// One step of history: [`Session::undo`] or [`Session::redo`].
type HistoryStep = fn(&mut Session) -> Result<String, CyberError>;

/// Runs `step` (`Session::undo` or `Session::redo`) up to `count` times and
/// collects the labels it reports, in the order the steps ran. Reaching the
/// bottom of the stack is a normal stop; the refusal only becomes an error
/// when not a single step landed, so the caller reports `not_found` exactly
/// when there was nothing to undo/redo at all.
///
/// # Errors
///
/// Whatever `step` reports on its very first call; later refusals end the
/// walk and the collected labels are returned.
fn walk_history(
    session: &mut Session,
    count: usize,
    step: HistoryStep,
) -> Result<Vec<String>, CyberError> {
    let mut labels: Vec<String> = Vec::new();
    for _step in 0..count {
        match step(session) {
            Ok(label) => labels.push(label),
            Err(error) if labels.is_empty() => return Err(error),
            Err(_) => break,
        }
    }
    Ok(labels)
}

/// Whether the merged view still declares a map block named `map`.
fn merged_holds_map(session: &Session, map: LumpName) -> bool {
    let view = session.merged();
    let lumps: Vec<Lump> = view.iter().map(|entry| entry.lump.clone()).collect();
    analyze(&lumps).maps.iter().any(|block| block.name == map)
}

/// Drops the active-map selection when the stepped-back working archive no
/// longer leaves its map in the merged view. Layers cannot be undone, so a
/// map a read-only layer provides always survives the check.
///
/// Reports whether the selection was cleared.
fn clear_missing_active_map(session: &mut Session) -> bool {
    let Some(map) = session.active_map() else {
        return false;
    };
    if merged_holds_map(session, map) {
        return false;
    }
    session.set_active_map(None);
    true
}

/// `undo`'s work: step back over history, then report the stacks, the
/// dirty state and whether the active map had to go.
fn undo_run(session: &mut Session, steps: usize) -> Result<UndoReport, CyberError> {
    let undone = walk_history(session, steps, Session::undo)?;
    let active_map_cleared = clear_missing_active_map(session);
    Ok(UndoReport {
        undone,
        remaining_undo: session.undo_labels().len(),
        redo_available: session.redo_labels().len(),
        dirty: session.is_dirty(),
        active_map_cleared,
    })
}

/// `redo`'s work: replay undone steps, then report the stacks, the dirty
/// state and whether the active map had to go.
fn redo_run(session: &mut Session, steps: usize) -> Result<RedoReport, CyberError> {
    let redone = walk_history(session, steps, Session::redo)?;
    let active_map_cleared = clear_missing_active_map(session);
    Ok(RedoReport {
        redone,
        remaining_redo: session.redo_labels().len(),
        undo_available: session.undo_labels().len(),
        dirty: session.is_dirty(),
        active_map_cleared,
    })
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
        description = "Saves the session to a file. mode \"pwad\" (the default) writes the working archive and marks it saved (the working path becomes the file written); mode \"full\" exports the entire merged view (IWAD layer, resource layers and working archive under the override rules) as one standalone WAD without changing the working path or the dirty flag. path is absolute and defaults to the working archive's own path; kind (\"PWAD\"|\"IWAD\") overrides the header magic. dedupe (default false) writes each distinct non-empty lump payload once, later copies sharing the first one's offset, and reports the bytes saved; engines read such files fine, but a naive byte-comparing tool may not. Refuses with conflict to overwrite a loaded layer's file. Returns {path, mode, kind, lumps, bytes, bytes_saved_by_dedupe, dirty}."
    )]
    fn save_wad(
        &self,
        Parameters(args): Parameters<SaveWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(save_wad_report(self.state(), &args))
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

    /// Steps back over recent edits of the working archive.
    #[tool(
        description = "Undoes the most recent edits to the working archive: steps back over up to steps entries of the undo stack (default 1, 1..=100; the history keeps only the newest 100 edits and open_wad/new_wad clear it). Fewer edits than steps simply undoes what exists; an empty undo stack fails with not_found \"nothing to undo\". Only the working archive changes: layers, the active map and the render settings are not part of the history, and an active map whose block is gone from the merged view is cleared (active_map_cleared true). Returns {undone: [labels, most recent first], remaining_undo, redo_available, dirty, active_map_cleared}."
    )]
    fn undo(&self, Parameters(args): Parameters<UndoParams>) -> Result<CallToolResult, ErrorData> {
        let outcome = parse_steps(args.steps).and_then(|steps| {
            self.state()
                .with_session(|session| undo_run(session, steps))
        });
        reported(outcome)
    }

    /// Replays recently undone edits onto the working archive.
    #[tool(
        description = "Re-applies edits that undo stepped back over: replays up to steps entries of the redo stack (default 1, 1..=100). Fewer entries than steps simply replays what exists; an empty redo stack fails with not_found \"nothing to redo\", and steps 0 or past 100 is invalid_argument. A new edit after an undo drops the redo branch. Mirrors undo in every other way: only the working archive changes and a vanished active map is cleared. Returns {redone: [labels, in the order replayed], remaining_redo, undo_available, dirty, active_map_cleared}."
    )]
    fn redo(&self, Parameters(args): Parameters<RedoParams>) -> Result<CallToolResult, ErrorData> {
        let outcome = parse_steps(args.steps).and_then(|steps| {
            self.state()
                .with_session(|session| redo_run(session, steps))
        });
        reported(outcome)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        SaveWadParams, clear_missing_active_map, merged_holds_map, parse_steps, redo_run,
        save_wad_report, undo_run,
    };
    use crate::session::history::HISTORY_LIMIT;
    use crate::session::{Layer, LayerRole, Session};
    use crate::state::AppState;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::name::LumpName;

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A lump named `text` carrying `body`.
    fn carrying(text: &str, body: &str) -> Lump {
        Lump::new(name(text), body.as_bytes().to_vec())
    }

    /// A `PWAD` holding `lumps`.
    fn pwad(lumps: Vec<Lump>) -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps,
        }
    }

    #[test]
    fn save_wad_report_dedupes_only_when_asked_and_reports_the_saving() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let state = AppState::default();
        state
            .with_session(|session| {
                session.replace_working(
                    pwad(vec![
                        carrying("FIRST", "same-bytes"),
                        carrying("SECOND", "same-bytes"),
                        marker("NOTE"),
                    ]),
                    None,
                );
                Ok(())
            })
            .expect("seeding the session works");
        let plain_path = dir.path().join("plain.wad");
        let plain_args = SaveWadParams {
            path: Some(plain_path.display().to_string()),
            mode: None,
            kind: None,
            dedupe: false,
        };
        let plain = save_wad_report(&state, &plain_args).expect("the plain save lands");
        assert_eq!(
            plain.bytes_saved_by_dedupe, 0,
            "nothing shared, nothing saved"
        );
        assert!(!plain.dirty, "a pwad save marks the archive saved");
        let dedup_path = dir.path().join("dedup.wad");
        let dedup_args = SaveWadParams {
            path: Some(dedup_path.display().to_string()),
            mode: None,
            kind: None,
            dedupe: true,
        };
        let deduped = save_wad_report(&state, &dedup_args).expect("the dedupe save lands");
        assert_eq!(deduped.bytes_saved_by_dedupe, 10, "the twin's payload");
        assert_eq!(
            plain.bytes.saturating_sub(deduped.bytes),
            10,
            "the file really is smaller by the saved bytes"
        );
        assert_eq!(
            Wad::read_file(&dedup_path).expect("it re-opens"),
            Wad::read_file(&plain_path).expect("so does the plain save"),
            "dedupe changes the layout, never the archive"
        );
    }

    /// Appends an empty marker named `text` as one `add_lump TEXT` edit.
    fn add_marker(session: &mut Session, text: &str) {
        let lump = marker(text);
        session
            .edit(&format!("add_lump {text}"), move |wad| {
                wad.lumps.push(lump);
                Ok(())
            })
            .expect("appends never fail");
    }

    /// Appends marker lumps named `texts`, each as its own edit.
    fn add_markers(session: &mut Session, texts: &[&str]) {
        for text in texts {
            add_marker(session, text);
        }
    }

    /// Appends a minimal `MAP01` block (header plus `THINGS`) as one edit.
    fn build_map01(session: &mut Session) {
        session
            .edit("build MAP01", |wad| {
                wad.lumps.push(marker("MAP01"));
                wad.lumps.push(marker("THINGS"));
                Ok(())
            })
            .expect("appends never fail");
    }

    #[test]
    fn parse_steps_defaults_to_one_and_bounds_at_the_history_limit() {
        assert_eq!(parse_steps(None).expect("absent"), 1, "absent means one");
        assert_eq!(
            parse_steps(Some(1)).expect("lowest"),
            1,
            "the lowest accepted count"
        );
        assert_eq!(
            parse_steps(Some(HISTORY_LIMIT)).expect("highest"),
            HISTORY_LIMIT,
            "the highest accepted count"
        );
    }

    #[test]
    fn parse_steps_refuses_zero_and_counts_past_the_limit() {
        let zero = parse_steps(Some(0)).expect_err("0 is refused");
        assert_eq!(zero.code(), "invalid_argument", "steps 0 code");
        let past = HISTORY_LIMIT
            .checked_add(1)
            .expect("a count past the limit");
        let error = parse_steps(Some(past)).expect_err("past the limit is refused");
        assert_eq!(error.code(), "invalid_argument", "steps > 100 code");
        assert!(
            error.to_string().contains("1..=100"),
            "the message names the accepted range: {error}"
        );
    }

    #[test]
    fn undo_run_steps_back_over_the_stack_and_reports_it() {
        let mut session = Session::default();
        add_markers(&mut session, &["A", "B", "C"]);
        session.mark_saved(PathBuf::from("out.wad"));
        let report = undo_run(&mut session, 5).expect("three steps are available");
        assert_eq!(
            report.undone,
            vec!["add_lump C", "add_lump B", "add_lump A"],
            "newest first, and only three of the asked five ran"
        );
        assert_eq!(report.remaining_undo, 0, "the stack is empty now");
        assert_eq!(report.redo_available, 3, "all three can be replayed");
        assert!(report.dirty, "back to a revision before the saved one");
        assert!(!report.active_map_cleared, "no map was ever selected");
    }

    #[test]
    fn undo_run_on_an_empty_stack_reports_not_found() {
        let mut session = Session::default();
        let error = undo_run(&mut session, 1).expect_err("nothing was edited");
        assert_eq!(error.code(), "not_found", "empty undo code");
        assert_eq!(error.to_string(), "nothing to undo", "empty undo message");
    }

    #[test]
    fn redo_run_replays_the_undone_steps_in_order() {
        let mut session = Session::default();
        add_markers(&mut session, &["A", "B", "C"]);
        session.mark_saved(PathBuf::from("out.wad"));
        undo_run(&mut session, 3).expect("undo all three");
        let report = redo_run(&mut session, 2).expect("two steps are available");
        assert_eq!(
            report.redone,
            vec!["add_lump A", "add_lump B"],
            "the replay order, oldest undone first"
        );
        assert_eq!(report.remaining_redo, 1, "C still waits");
        assert_eq!(report.undo_available, 2, "A and B can be undone again");
        assert!(report.dirty, "not yet back at the saved revision");
        assert_eq!(session.working().len(), 2, "A and B are in the archive");
    }

    #[test]
    fn redo_run_on_an_empty_stack_reports_not_found() {
        let mut session = Session::default();
        let error = redo_run(&mut session, 1).expect_err("nothing was undone");
        assert_eq!(error.code(), "not_found", "empty redo code");
        assert_eq!(error.to_string(), "nothing to redo", "empty redo message");
    }

    #[test]
    fn undo_run_clears_an_active_map_the_history_removed() {
        let mut session = Session::default();
        build_map01(&mut session);
        session.set_active_map(Some(name("MAP01")));
        let report = undo_run(&mut session, 1).expect("the build is one step back");
        assert!(report.active_map_cleared, "the block left the merged view");
        assert_eq!(session.active_map(), None, "and the selection was dropped");
    }

    #[test]
    fn undo_run_keeps_an_active_map_a_layer_still_provides() {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("doom.wad"),
            LayerRole::Iwad,
            pwad(vec![marker("MAP01"), marker("THINGS")]),
        ));
        session.set_active_map(Some(name("MAP01")));
        add_markers(&mut session, &["NOTE"]);
        let report = undo_run(&mut session, 1).expect("undo the note");
        assert!(!report.active_map_cleared, "the layer still holds the map");
        assert_eq!(
            session.active_map(),
            Some(name("MAP01")),
            "the selection stands"
        );
    }

    #[test]
    fn merged_holds_map_reads_the_header_plus_members() {
        let mut session = Session::default();
        assert!(
            !merged_holds_map(&session, name("MAP01")),
            "nothing is loaded"
        );
        build_map01(&mut session);
        assert!(
            merged_holds_map(&session, name("MAP01")),
            "the working block counts"
        );
        assert!(
            !merged_holds_map(&session, name("MAP02")),
            "only MAP01 is there"
        );
    }

    #[test]
    fn clear_missing_active_map_needs_a_selection_to_drop() {
        let mut session = Session::default();
        assert!(
            !clear_missing_active_map(&mut session),
            "no selection, nothing to clear"
        );
        session.set_active_map(Some(name("MAP07")));
        assert!(
            clear_missing_active_map(&mut session),
            "MAP07 is nowhere in the view"
        );
        assert_eq!(session.active_map(), None, "and it is gone");
    }
}
