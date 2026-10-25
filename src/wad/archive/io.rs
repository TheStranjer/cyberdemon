//! The filesystem half of the in-memory archive: reading a path into bytes
//! and writing them back so a crash never leaves a truncated `WAD`.
//!
//! [`write_file_atomic`] stages into a unique `<name>.<pid>.<seq>.tmp`
//! sibling opened with `create_new`, `sync_all`s it and lands it: `rename`d
//! over the target when replacing, `hard_link`ed to it when not (a link
//! onto an existing name fails, so a no-replace write is refused at the
//! final path with no check-then-write gap). The staging file is removed
//! when any step after its creation fails, and the parent directory is
//! `sync_all`-ed after a successful landing so the new name itself is
//! durable. [`read_bytes`] is the matching load. Every failure is reported
//! as a [`CyberError`] naming the path and the action that was attempted,
//! so a caller never has to guess which file it lost. Parsing the bytes is
//! the `directory` module's job, not this one's.

use core::sync::atomic::{AtomicU64, Ordering};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process;

use crate::error::CyberError;

/// Process-wide counter making each staging name unique among this
/// process's concurrent writes.
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Tries made at opening a staging file before giving up, for the
/// vanishingly unlikely case of a name another writer already took.
const STAGING_ATTEMPTS: usize = 8;

/// Writes `bytes` to `path` atomically via a unique sibling staging file.
///
/// The bytes land in a freshly created `<name>.<pid>.<seq>.tmp` sibling,
/// are `sync_all`-ed, then the staging file is landed on the target:
/// `rename`d over it when `replace` is `true`, `hard_link`ed to it when
/// `false` so a target that exists, or appears, is refused with
/// [`CyberError::AlreadyExists`] instead of replaced. Readers of `path`
/// see either the old file or the complete new one, never a mix, and two
/// concurrent writes to the same target never share a staging file. The
/// staging file is removed when any step after its creation fails, and
/// the parent directory is `sync_all`-ed (best effort) after a successful
/// landing. The parent directory of `path` must exist.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the parent directory of `path` does not
/// exist; [`CyberError::AlreadyExists`] when `replace` is `false` and
/// `path` names an existing file; [`CyberError::Io`] naming the path for
/// write, sync or landing failures; [`CyberError::InvalidArgument`] when
/// `path` has no file name.
#[inline]
pub fn write_file_atomic(path: &Path, bytes: &[u8], replace: bool) -> Result<(), CyberError> {
    let (mut file, staging) = create_staging(path)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error(path, "write", &error));
    drop(file);
    let landed = written.and_then(|()| land(&staging, path, replace));
    if landed.is_err() || !replace {
        remove_staging(&staging);
    }
    if landed.is_ok() {
        sync_parent(path);
    }
    landed
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

/// Opens a fresh staging file beside `path`, drawing a new sequence number
/// per try, and returns the handle with the path it was created at.
///
/// # Errors
///
/// [`staging_error`]'s, and [`CyberError::Io`] when every candidate name
/// is already taken.
fn create_staging(path: &Path) -> Result<(File, PathBuf), CyberError> {
    for _ in 0..STAGING_ATTEMPTS {
        let staging = staging_path(path)?;
        let opened = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging);
        match opened {
            Ok(file) => return Ok((file, staging)),
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
                return Err(staging_error(path, &error));
            }
            Err(_) => {}
        }
    }
    Err(io_error(
        path,
        "create a free temporary file beside (every candidate name is taken)",
        &io::Error::from(io::ErrorKind::AlreadyExists),
    ))
}

/// The sibling path `<name>.<pid>.<seq>.tmp` an atomic write stages
/// through, with `seq` a process-wide counter no two calls ever see twice,
/// so concurrent writes never open the same staging file.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `path` has no file name to extend.
pub(super) fn staging_path(path: &Path) -> Result<PathBuf, CyberError> {
    let name = path.file_name().ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "{} has no file name to stage a sibling write beside",
            path.display()
        ))
    })?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut staging = name.to_os_string();
    staging.push(".");
    staging.push(process::id().to_string());
    staging.push(".");
    staging.push(sequence.to_string());
    staging.push(".tmp");
    Ok(path.with_file_name(staging))
}

/// Lands the finished staging file on `path`: a `rename` over it when
/// `replace` is `true`, else a `hard_link` that atomically refuses a name
/// already taken.
fn land(staging: &Path, path: &Path, replace: bool) -> Result<(), CyberError> {
    if replace {
        return fs::rename(staging, path).map_err(|error| io_error(path, "replace", &error));
    }
    fs::hard_link(staging, path).map_err(|error| land_error(path, &error))
}

/// A failed landing: a taken target under no-replace is `AlreadyExists`
/// naming the way out, anything else `Io`.
fn land_error(path: &Path, error: &io::Error) -> CyberError {
    if error.kind() == io::ErrorKind::AlreadyExists {
        return CyberError::AlreadyExists(format!(
            "{} already exists: pass overwrite true to replace it",
            path.display()
        ));
    }
    io_error(path, "write without replacing", error)
}

/// `sync_all`s `path`'s parent directory so a completed landing survives a
/// crash. Best effort: a filesystem that will not open the directory has
/// still taken the file, and the write itself already succeeded.
fn sync_parent(path: &Path) {
    let synced = path.parent().map_or(Ok(()), |parent| {
        File::open(parent).and_then(|dir| dir.sync_all())
    });
    drop(synced);
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

/// Removes a staging file after a failed write, or after a no-replace
/// landing whose inode the target now shares. Cleanup cannot add
/// information: the operation already decided its error.
fn remove_staging(staging: &Path) {
    let removal = fs::remove_file(staging);
    drop(removal);
}
