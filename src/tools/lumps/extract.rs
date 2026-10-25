//! `extract_lump`: its arguments, the destination it names and the read
//! that hands the bytes over.
//!
//! [`parse_target`] checks the absolute path and [`extract_run`] the
//! selector read through [`crate::tools::lumps::select`]; [`path_text`] and
//! [`ExtractReport`] are what the call reports once the router has written
//! the file with [`crate::wad::archive::write_file_atomic`]. The overwrite
//! rule is not a separate existence check: the atomic write itself refuses
//! to land on a name that exists (or appeared) when `overwrite` is `false`,
//! so there is no window between check and write to lose a file to.

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

/// The shared path-as-text helper, re-exported at the group path the
/// extract and `get_lump` rows import from.
pub(super) use crate::tools::session::path_text;

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

/// Resolve the selector and copy out the raw bytes. Whether the target may
/// be replaced is decided by the atomic write that follows this read, not
/// by a check here that a concurrent writer could invalidate before the
/// write lands.
///
/// # Errors
///
/// [`resolve`]'s errors.
pub(super) fn extract_run(
    session: &Session,
    selector: &LumpSelector,
) -> Result<Vec<u8>, CyberError> {
    let row = resolve(session, selector)?;
    Ok(row.lump.data.to_vec())
}
