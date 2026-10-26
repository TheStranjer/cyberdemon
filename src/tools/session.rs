//! Session tools: loading archives into the editing session and describing
//! what the session holds.
//!
//! [`Cyberdemon::open_iwad`] makes the file at a path the session's
//! read-only base game layer, [`Cyberdemon::open_wad`] adopts one as the
//! editable working archive and [`Cyberdemon::new_wad`] stands up an empty
//! one. [`Cyberdemon::merge_wad`] brings another file in as a resource layer
//! or folds it into the working archive, [`Cyberdemon::save_wad`] writes the
//! working archive or the whole merged view back to a file, and
//! [`Cyberdemon::get_session_info`] and [`Cyberdemon::get_wad_info`]
//! describe the session and the view an engine would load.
//!
//! The group is split by tool family into the private modules [`mod@open`]
//! (the three load tools' parameters, reports and runs),
//! [`mod@merge_save`] (`merge_wad` and `save_wad`, their modes and the
//! target and overwrite rules), [`mod@info`] (the two description tools'
//! report shapes and builders) and [`mod@undo`] (`undo` and `redo`, serving
//! their own `history_router` apart from this group's; named for the tool,
//! apart from [`crate::session::history`], which is the stack itself). This
//! root keeps what the four families share:
//! [`UNSAVED_WORKING`], the helpers they read paths, archives, the session
//! and the result channel through ([`path_text`], [`read_archive`],
//! [`read_archive_at`], [`ensure_clean`] and [`chronological`]) and
//! the `#[tool_router]` block that hands every tool to its family's run. The
//! group's unit tests live in `session/tests.rs`.

mod info;
mod merge_save;
mod open;
mod undo;

use std::path::{Path, PathBuf};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::tools::reply;
use crate::tools::session::info::{describe_session, describe_wad};
use crate::tools::session::merge_save::{
    MergeWadParams, SaveWadParams, merge_wad_report, save_wad_report,
};
use crate::tools::session::open::{
    NewWadParams, OpenIwadParams, OpenWadParams, new_wad_report, open_iwad_report, open_wad_report,
};
use crate::wad::archive::Wad;

/// The placeholder the `pwads` list shows for an unsaved working archive.
const UNSAVED_WORKING: &str = "(unsaved working archive)";

/// A path as report text, shared by every tool that quotes a path in a
/// report or a refusal.
pub(in crate::tools) fn path_text(path: &Path) -> String {
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
pub(in crate::tools) fn read_archive(text: &str) -> Result<(PathBuf, Wad), CyberError> {
    let path = Path::new(text);
    if !path.is_absolute() {
        return Err(CyberError::InvalidArgument(format!(
            "path {text:?} is not absolute; pass the full filesystem path of the WAD file"
        )));
    }
    let wad = read_archive_at(path)?;
    Ok((path.to_path_buf(), wad))
}

/// Read the archive at `path`, mapping a missing file to `not_found`.
///
/// # Errors
///
/// [`read_archive_at`]'s [`CyberError::NotFound`] / [`CyberError::Io`] and
/// whatever [`Wad::read_file`] reports for contents that are not a valid
/// `WAD`.
pub(in crate::tools) fn read_archive_at(path: &Path) -> Result<Wad, CyberError> {
    Wad::read_file(path).map_err(|error| open_error(path, error))
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

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = session_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Loads a file as the session's base game (`IWAD`) layer.
    #[tool(
        description = "Reads the WAD at an absolute path and makes it the session's read-only base game layer, replacing any previous IWAD while keeping the resource layers; the game is detected from the archive. Call it first: palettes, flats, definitions and maps all come from the game data. undo does not step over an archive swap, and a running run_batch refuses the swap."
    )]
    fn open_iwad(
        &self,
        Parameters(args): Parameters<OpenIwadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(open_iwad_report(self.state(), &args.path))
    }

    /// Opens a file as the editable working archive.
    #[tool(
        description = "Reads the WAD at an absolute path and adopts it as the editable working archive, forgetting the previous archive's undo history. Refuses with conflict while the current working archive is dirty unless discard_changes: true, or while a run_batch is open. Use it to continue work on an existing mod; merge_wad stacks extra read-only layers instead."
    )]
    fn open_wad(
        &self,
        Parameters(args): Parameters<OpenWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(open_wad_report(
            self.state(),
            &args.path,
            args.discard_changes,
        ))
    }

    /// Starts a fresh empty working archive.
    #[tool(
        description = "Replaces the working archive with an empty PWAD that has no file behind it yet (save_wad gives it one). Refuses with conflict while unsaved edits exist unless discard_changes: true, or while a run_batch is open; clears the undo history and active map. Use it to start a fresh mod once open_iwad has loaded the game data."
    )]
    fn new_wad(
        &self,
        Parameters(args): Parameters<NewWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(new_wad_report(self.state(), args.discard_changes))
    }

    /// Merges a file in as a resource layer or into the working archive.
    #[tool(
        description = "Reads the WAD at an absolute path and folds it into the session: mode layer (default) stacks it as a read-only resource layer below the working archive, refusing while a run_batch is open; mode working folds its content into the working archive as one undoable edit under vanilla override rules. Use it to add a texture or sound pack; get_session_info shows the layer stack."
    )]
    fn merge_wad(
        &self,
        Parameters(args): Parameters<MergeWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(merge_wad_report(
            self.state(),
            &args.path,
            args.mode.as_deref(),
        ))
    }

    /// Writes the working archive or the whole merged view to a file.
    #[tool(
        description = "Saves the session to a file: mode pwad (default) writes the working archive to path (or the path it was opened from) and marks it saved, refusing while a run_batch is open; mode full flattens the IWAD, resource layers and working overrides into one archive and runs inside a batch. Paths must be absolute and the write is atomic. Reports path and bytes; undo history survives a pwad save."
    )]
    fn save_wad(
        &self,
        Parameters(args): Parameters<SaveWadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(save_wad_report(self.state(), &args))
    }

    /// Describes the session's archives, history and state.
    #[tool(
        description = "Reports the whole session: the base IWAD layer (or null), every read-only layer with its index and role, the working archive with its path and dirty flag, the active map and where undo and redo stand. Use it to reload the state after a conflict or to check what is loaded before editing."
    )]
    fn get_session_info(&self) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| Ok(describe_session(session)));
        reply(report)
    }

    /// Summarizes the merged view an engine would load.
    #[tool(
        description = "Reports the merged view an engine would see: detected game, load-ordered archive list, total lump count, every map with its format and element counts, and the resource summary (flats, patches, sprites, textures, sounds, music). Use it for the whole-project picture beside get_session_info's editing state."
    )]
    fn get_wad_info(&self) -> Result<CallToolResult, ErrorData> {
        let report = self
            .state()
            .with_session(|session| Ok(describe_wad(session)));
        reply(report)
    }
}

#[cfg(test)]
mod tests;
