//! `extract_lump`: its arguments, the destination it names and the read
//! that hands the bytes over.
//!
//! [`parse_target`] checks the absolute path, [`refuse_existing`] the
//! overwrite rule and [`extract_run`] the selector read through
//! [`crate::tools::lumps::select`]; [`path_text`] and [`ExtractReport`] are
//! what the call reports once the router has written the file with
//! [`crate::wad::archive::write_file_atomic`].

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::lumps::select::{LumpSelector, resolve};

/// Arguments for [`Cyberdemon::extract_lump`]: the shared selector plus the
/// destination file.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ExtractLumpParams {
    /// The lump selector: `name` plus optional `scope`, `namespace` and
    /// `occurrence` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    pub(super) selector: LumpSelector,
    /// Absolute filesystem path to write the lump's raw bytes to.
    pub(super) path: String,
    /// Replace the target file when it already exists (default `false`).
    #[serde(default)]
    pub(super) overwrite: bool,
}

/// Structured result of [`Cyberdemon::extract_lump`].
#[derive(Serialize)]
pub(super) struct ExtractReport {
    /// The file that was written.
    pub(super) path: String,
    /// Bytes in the file.
    pub(super) bytes: usize,
}

/// A filesystem path as display text, for reports.
pub(super) fn path_text(path: &Path) -> String {
    path.display().to_string()
}

/// The destination of an `extract_lump` call: its `path` argument as a
/// [`PathBuf`], which must be absolute.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a relative path.
pub(super) fn parse_target(raw: &str) -> Result<PathBuf, CyberError> {
    let path = Path::new(raw);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Err(CyberError::InvalidArgument(format!(
        "path {raw:?} is not absolute; pass the full filesystem path to write"
    )))
}

/// Refuse to replace an existing target unless `overwrite` allows it.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] when the target exists and `overwrite` is
/// `false`.
pub(super) fn refuse_existing(target: &Path, overwrite: bool) -> Result<(), CyberError> {
    if !overwrite && target.try_exists().is_ok_and(|exists| exists) {
        return Err(CyberError::AlreadyExists(format!(
            "{} already exists: pass overwrite true to replace it",
            target.display()
        )));
    }
    Ok(())
}

/// Resolve the selector and copy out the raw bytes, refusing an existing
/// target unless `overwrite` says to replace it.
///
/// # Errors
///
/// [`resolve`]'s errors and [`refuse_existing`]'s.
pub(super) fn extract_run(
    session: &Session,
    selector: &LumpSelector,
    target: &Path,
    overwrite: bool,
) -> Result<Vec<u8>, CyberError> {
    let row = resolve(session, selector)?;
    refuse_existing(target, overwrite)?;
    Ok(row.lump.data.to_vec())
}
