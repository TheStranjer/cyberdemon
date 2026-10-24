//! The filesystem half of the in-memory archive: reading a path into bytes
//! and writing them back so a crash never leaves a truncated `WAD`.
//!
//! [`write_file_atomic`] stages into a `<name>.<pid>.tmp` sibling, `sync_all`s
//! it and `rename`s it over the target, removing the staging file when any
//! step after its creation fails. [`read_bytes`] is the matching load. Every
//! failure is reported as a [`CyberError`] naming the path and the action that
//! was attempted, so a caller never has to guess which file it lost. Parsing
//! the bytes is the `directory` module's job, not this one's.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process;

use crate::error::CyberError;

/// Writes `bytes` to `path` atomically via a sibling staging file.
///
/// The bytes land in `<name>.<pid>.tmp`, are `sync_all`-ed, then the
/// staging file is `rename`d over the target, so readers see either the
/// old file or the complete new one. The staging file is removed when
/// any step after creation fails. The parent directory of `path` must
/// exist.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the parent directory of `path` does not
/// exist; [`CyberError::Io`] naming the path for write, sync or rename
/// failures; [`CyberError::InvalidArgument`] when `path` has no file name.
#[inline]
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<(), CyberError> {
    let staging = staging_path(path)?;
    let mut file = File::create(&staging).map_err(|error| staging_error(path, &error))?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error(path, "write", &error));
    drop(file);
    let renamed = written.and_then(|()| {
        fs::rename(&staging, path).map_err(|error| io_error(path, "replace", &error))
    });
    if renamed.is_err() {
        remove_staging(&staging);
    }
    renamed
}

/// Reads `path` into memory for [`Wad::read_file`](super::Wad::read_file) to
/// parse.
///
/// # Errors
///
/// [`CyberError::Io`] naming the path when the file cannot be read; a
/// missing file is an I/O failure too.
pub(super) fn read_bytes(path: &Path) -> Result<Vec<u8>, CyberError> {
    fs::read(path).map_err(|error| io_error(path, "read", &error))
}

/// The sibling path `<name>.<pid>.tmp` an atomic write stages through.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `path` has no file name to extend.
fn staging_path(path: &Path) -> Result<PathBuf, CyberError> {
    let name = path.file_name().ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "{} has no file name to stage a sibling write beside",
            path.display()
        ))
    })?;
    let mut staging = name.to_os_string();
    staging.push(".");
    staging.push(process::id().to_string());
    staging.push(".tmp");
    Ok(path.with_file_name(staging))
}

/// An `Io` failure naming the path and what was attempted on it.
fn io_error(path: &Path, action: &str, error: &io::Error) -> CyberError {
    CyberError::Io(format!("cannot {action} {}: {error}", path.display()))
}

/// A staging-file creation failure: a missing parent directory is
/// `NotFound`, anything else `Io`.
fn staging_error(path: &Path, error: &io::Error) -> CyberError {
    if error.kind() == io::ErrorKind::NotFound {
        return CyberError::NotFound(format!(
            "cannot write {}: its parent directory does not exist ({error})",
            path.display()
        ));
    }
    io_error(path, "create a temporary file beside", error)
}

/// Removes a staging file after a failed write. Cleanup cannot add
/// information: the operation already decided its error.
fn remove_staging(staging: &Path) {
    let removal = fs::remove_file(staging);
    drop(removal);
}
