//! The lump-editing tools' arguments and working-archive mutations:
//! `replace_lump`, `remove_lump` and `rename_lump`.
//!
//! Every [`crate::tools::lumps`] router method but
//! [`crate::tools::lumps::reorder`]'s hands its arguments to this module. The
//! shared shape is: parse and validate everything, fetch any `source` *before*
//! the session lock is taken, then run exactly one
//! [`crate::session::Session::edit`] whose closure either finishes the whole
//! change or reports an error that rolls it back. The undo label is the tool's
//! name followed by the lump names it touched. The working-archive selector
//! ([`EditSelector`], [`edit_selector`] and [`working_match`]) is shared with
//! `reorder_lumps`.
//!
//! `replace_lump` is the copy-on-write tool. A lump the working archive holds
//! is overwritten in place; one that only the `IWAD` or a resource layer holds
//! is brought in first — a whole map block through
//! [`crate::session::cow::copy_map_block`], a region member through the shared
//! placement in [`crate::tools::lumps::place`], anything else appended — and
//! the report says so with `"created_override": true`. The read-only layers
//! never change, so `remove_lump` and `rename_lump` work on the working
//! archive alone and refuse a name it does not hold.
//!
//! Each tool's arguments and mutations then live in their own module:
//! [`replace_lump`] and where its payload lands in `replace`, [`remove_lump`]
//! and its plan in `remove`, [`rename_lump`] and its clash check in `rename`.
//! The three entry points and their parameter types are re-exported here, so
//! the routers and [`crate::tools::lumps::reorder`] reach all of it through
//! `edit::`.

mod remove;
mod rename;
mod replace;

use crate::error::CyberError;
use crate::session::{Scope, Session};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::lumps::select::nth_match;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

pub(super) use remove::{RemoveLumpParams, remove_lump};
pub(super) use rename::{RenameLumpParams, rename_lump};
pub(super) use replace::{ReplaceLumpParams, replace_lump};

/// Where the editing tools' occurrence refusals say they found the
/// matches.
const WORKING_ARCHIVE: &str = "the working archive";

/// The name, region and occurrence the lump-editing tools select on.
#[derive(Clone, Copy, Debug)]
pub(super) struct EditSelector {
    /// The lump name to edit.
    pub(super) name: LumpName,
    /// Only matches inside this region count; `None` is any region.
    pub(super) namespace: Option<Namespace>,
    /// 0-based index among the working archive's matches; `None` is the last.
    pub(super) occurrence: Option<usize>,
}

impl EditSelector {
    /// Whether a directory row is one of this selector's matches.
    fn accepts(&self, row: &LumpRow) -> bool {
        row.name == self.name && in_region(row.namespace, self.namespace)
    }
}

/// The `names`, `namespace` and `occurrence` a tool's arguments spell out.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name that does not parse or a
/// `namespace` that names no region.
pub(super) fn edit_selector(
    name: &str,
    namespace: Option<&str>,
    occurrence: Option<usize>,
) -> Result<EditSelector, CyberError> {
    Ok(EditSelector {
        name: LumpName::parse(name)?,
        namespace: super::parse_namespace(namespace)?,
        occurrence,
    })
}

/// Whether a directory entry sits where a region filter looks; `None` is any
/// region, the loose lumps included.
fn in_region(found: Option<Namespace>, wanted: Option<Namespace>) -> bool {
    wanted.is_none_or(|namespace| found == Some(namespace))
}

/// The region a message names a lump's location by, for error text.
pub(super) fn region_text(namespace: Option<Namespace>) -> String {
    namespace.map_or_else(
        || "the working archive".to_owned(),
        |wanted| format!("the {} region of the working archive", wanted.as_str()),
    )
}

/// The working-archive index of the lump `selector` picks, `None` when the
/// working archive holds no match at all.
///
/// # Errors
///
/// [`build_rows`]' errors for the scope, and [`CyberError::NotFound`] when
/// `occurrence` reaches past the matches.
pub(super) fn working_match(
    session: &Session,
    selector: &EditSelector,
) -> Result<Option<usize>, CyberError> {
    let matched: Vec<usize> = build_rows(session, Scope::Working)?
        .into_iter()
        .filter(|row| selector.accepts(row))
        .map(|row| row.index)
        .collect();
    if matched.is_empty() {
        return Ok(None);
    }
    nth_match(matched, selector.occurrence, selector.name, WORKING_ARCHIVE).map(Some)
}

#[cfg(test)]
mod tests;
