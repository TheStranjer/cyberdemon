//! Shared builders for the `lumps` tests: the parsed name and the row a
//! search judges, the one-glob [`Search`] it judges with, the error-code
//! shorthand and the editing arguments and committed edit the add topic runs.
//!
//! [`parse_search`] checks the shared argument parsers and a search's name
//! and content criteria; [`get`] runs the `get_lump` formats, bodies and
//! asset names beside the `extract_lump` path checks; [`add`] runs the
//! payload rules, the plans, the derived import name and the insert itself.

mod add;
mod get;
mod parse_search;

use crate::error::CyberError;
use crate::session::Source;
use crate::tools::lumps::add::{AddLumpParams, ImportParams, Prepared};
use crate::tools::lumps::list::Search;
use crate::tools::lumps::place::{Placement, PlacementParams};
use crate::tools::lumps::rows::LumpRow;
use crate::wad::archive::Lump;
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::LumpName;

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A row carrying `data` for judging, everything else filled in.
pub(super) fn row(
    text: &str,
    kind: LumpKind,
    namespace: Option<Namespace>,
    data: &[u8],
) -> LumpRow {
    LumpRow {
        index: 0,
        name: name(text),
        size: data.len(),
        offset: 12,
        kind,
        namespace,
        map: None,
        source: Source::Working,
        lump: Lump::new(name(text), data.to_vec()),
    }
}

/// A `Search` with only a name glob.
pub(super) fn glob_search(pattern: &str) -> Search {
    Search {
        pattern: Some(pattern.to_owned()),
        kind: None,
        namespace: None,
        text: None,
        hex: None,
    }
}

/// The error code of a failing [`CyberError`], for compact asserts.
pub(super) fn code(error: &CyberError) -> &str {
    error.code()
}

/// `add_lump` arguments carrying only the required name.
pub(super) fn add_args(text: &str) -> AddLumpParams {
    AddLumpParams {
        name: text.to_owned(),
        text: None,
        hex: None,
        source: None,
        placement: PlacementParams::default(),
        allow_duplicate: false,
    }
}

/// `import` arguments for `source` with an optional explicit name.
pub(super) fn import_args(source: &str, wanted: Option<&str>) -> ImportParams {
    ImportParams {
        source: source.to_owned(),
        name: wanted.map(str::to_owned),
        placement: PlacementParams::default(),
        allow_duplicate: false,
    }
}

/// A committed edit of `data` bytes under `text`, appending.
pub(super) fn prepared(text: &str, data: &[u8]) -> Prepared {
    Prepared {
        verb: "add_lump",
        name: name(text),
        data: data.to_vec(),
        placement: Placement::default(),
        allow_duplicate: false,
        source: None,
    }
}
