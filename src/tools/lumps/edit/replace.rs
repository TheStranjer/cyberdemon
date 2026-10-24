//! `replace_lump`: the copy-on-write payload swap in the working archive.
//!
//! [`replace_lump`] validates the selector and the payload fields,
//! [`fetch_data`] resolves the new bytes *before* the session lock is taken
//! and [`run_replace`] commits exactly one [`crate::session::Session::edit`]:
//! an overwrite of the lump the working archive holds, or the copy-in of an
//! override for a lump only the `IWAD` or a resource layer owns, which is
//! what [`Target`] and [`Replacement`] spell out. [`not_working_message`] is
//! the refusal text the `remove` and `rename` halves report.

use alloc::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{EditSelector, edit_selector, region_text, working_match};
use crate::error::CyberError;
use crate::session::cow;
use crate::session::{MergedLump, Scope, Session};
use crate::state::AppState;
use crate::tools::lumps::hex_bytes;
use crate::tools::lumps::place::{self, Placement};
use crate::tools::lumps::rows::build_rows;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::replace_lump`](crate::server::Cyberdemon).
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools::lumps) struct ReplaceLumpParams {
    /// Lump name to replace: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively (it is uppercased).
    pub(super) name: String,
    /// Keep only matches inside this namespace region (`sprites`, `flats`,
    /// `patches`, `colormaps`, `acs`, `voices`, `hires`, `textures`); without
    /// it any region and the loose lumps match.
    #[serde(default)]
    pub(super) namespace: Option<String>,
    /// 0-based index among the **working archive's** same-name matches; the
    /// default is the last one. Ignored when the name is only in a read-only
    /// layer, where the winning (last) copy is the one overridden.
    #[serde(default)]
    pub(super) occurrence: Option<usize>,
    /// New payload as UTF-8 text, written as-is. Exactly one of `text`, `hex`
    /// or `source` is required.
    #[serde(default)]
    pub(super) text: Option<String>,
    /// New payload as hex bytes, spaces allowed (e.g. `"4D 55 53 1A"`).
    /// Exactly one of `text`, `hex` or `source` is required.
    #[serde(default)]
    pub(super) hex: Option<String>,
    /// Absolute path, `file://` URI, `http(s)://` URL or uploaded asset URL
    /// whose bytes become the new payload. Exactly one of `text`, `hex` or
    /// `source` is required.
    #[serde(default)]
    pub(super) source: Option<String>,
}

/// Where a `replace_lump` payload goes in the working archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// The lump is already there: overwrite it at this working index.
    Working {
        /// Its position in the working archive.
        index: usize,
    },
    /// The lump sits in this map block of a read-only archive: copy the whole
    /// block in, then overwrite the copy.
    Map {
        /// The block to copy.
        map: LumpName,
    },
    /// The lump sits in this namespace region of a read-only archive: insert
    /// the override into the working archive's region of the same name.
    Region {
        /// The region to join.
        namespace: Namespace,
    },
    /// The lump is a loose global of a read-only archive: append the override.
    Append,
}

impl Target {
    /// Whether the working archive's directory grows to reach this target.
    pub(super) const fn creates_an_override(self) -> bool {
        !matches!(self, Self::Working { .. })
    }

    /// Whether reaching this target needs the merged view to copy from.
    pub(super) const fn needs_merged_view(self) -> bool {
        matches!(self, Self::Map { .. })
    }
}

/// One `replace_lump` write: the bytes, where they go and what has to be
/// copied in first.
pub(super) struct Replacement<'edit> {
    /// The lump to write.
    pub(super) name: LumpName,
    /// The new payload.
    pub(super) data: &'edit [u8],
    /// Where it lives, or how to bring it in.
    pub(super) target: Target,
    /// The merged view, needed to copy a map block.
    pub(super) merged: &'edit [MergedLump],
}

impl Replacement<'_> {
    /// Performs the working-archive mutation this write describes, reporting
    /// the lump's final index in that archive.
    ///
    /// # Errors
    ///
    /// [`crate::session::cow::copy_map_block`]'s and [`place::insert`]'s
    /// errors, [`CyberError::NotFound`] when a copied block holds no such
    /// lump, [`CyberError::Internal`] for a stale [`Target::Working`] index.
    pub(super) fn apply(&self, wad: &mut Wad) -> Result<usize, CyberError> {
        match self.target {
            Target::Working { index } => overwrite(wad, index, self.name, self.data),
            Target::Map { map } => self.copy_block_then_overwrite(wad, map),
            Target::Region { namespace } => self.insert(wad, Some(namespace)),
            Target::Append => self.insert(wad, None),
        }
    }

    /// Copies `map` into the working archive and overwrites the copy whose
    /// name matches, reporting its index.
    fn copy_block_then_overwrite(&self, wad: &mut Wad, map: LumpName) -> Result<usize, CyberError> {
        cow::copy_map_block(wad, self.merged, map)?;
        let index = wad.find_last(self.name).ok_or_else(|| {
            CyberError::NotFound(format!(
                "the copied {map} block holds no lump named {}: the merged view and the working \
                 archive disagree, so list_lumps map={map} shows what was actually copied",
                self.name
            ))
        })?;
        overwrite(wad, index, self.name, self.data)
    }

    /// Inserts the new lump as an override, into `namespace`'s region when one
    /// is given, and reports its index.
    fn insert(&self, wad: &mut Wad, namespace: Option<Namespace>) -> Result<usize, CyberError> {
        let placement = Placement {
            namespace,
            ..Placement::default()
        };
        place::insert(wad, &placement, Lump::new(self.name, self.data.to_vec()))
    }
}

/// Puts `data` into the lump at `index`, keeping its name and position.
///
/// # Errors
///
/// [`CyberError::Internal`] when `index` is past the directory's end.
pub(super) fn overwrite(
    wad: &mut Wad,
    index: usize,
    name: LumpName,
    data: &[u8],
) -> Result<usize, CyberError> {
    let Some(lump) = wad.lumps.get_mut(index) else {
        return Err(CyberError::Internal(format!(
            "no lump at index {index} of the working archive: {name} was expected there"
        )));
    };
    lump.data = Arc::from(data);
    Ok(index)
}

/// Where an editing call's new bytes come from.
#[derive(Debug)]
pub(super) enum NewData {
    /// The `text` argument, kept as UTF-8 bytes.
    Text(String),
    /// The `hex` argument, waiting to be decoded.
    Hex(String),
    /// A `source` to fetch.
    Source(String),
}

/// The one payload field `replace_lump` was given, refusing none and many.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when no payload field or more than one is
/// present.
pub(super) fn pick_new_data(args: &ReplaceLumpParams) -> Result<NewData, CyberError> {
    match (
        args.text.as_deref(),
        args.hex.as_deref(),
        args.source.as_deref(),
    ) {
        (Some(body), None, None) => Ok(NewData::Text(body.to_owned())),
        (None, Some(digits), None) => Ok(NewData::Hex(digits.to_owned())),
        (None, None, Some(raw)) => Ok(NewData::Source(raw.to_owned())),
        (None, None, None) => Err(CyberError::InvalidArgument(
            "replace_lump needs a new payload: pass exactly one of text, hex and source".to_owned(),
        )),
        _ => Err(CyberError::InvalidArgument(
            "replace_lump takes exactly one of text, hex and source: pick one payload form"
                .to_owned(),
        )),
    }
}

/// The new bytes: text as-is, hex decoded, or the fetched source's bytes.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for malformed hex; [`AppState::fetch_source`]
/// reports every other fetch failure.
pub(super) async fn fetch_data(state: &AppState, data: NewData) -> Result<Vec<u8>, CyberError> {
    match data {
        NewData::Text(body) => Ok(body.into_bytes()),
        NewData::Hex(digits) => hex_bytes("hex", &digits),
        NewData::Source(raw) => state.fetch_source(&raw).await.map(|got| got.bytes),
    }
}

/// The `not_found` text for a name no loaded archive holds.
fn nowhere_message(selector: &EditSelector) -> String {
    let region = selector.namespace.map_or_else(String::new, |wanted| {
        format!(" in the {} region", wanted.as_str())
    });
    format!(
        "lump {}{region} is in none of the loaded archives: add_lump creates a new lump in the \
         working archive and list_lumps shows what is already there",
        selector.name
    )
}

/// The `not_found` text for a name the working archive does not hold.
pub(super) fn not_working_message(name: LumpName, namespace: Option<Namespace>) -> String {
    format!(
        "lump {name} is not in {}: the IWAD and resource layers are read-only, so their lumps \
         cannot be removed or renamed, only overridden with replace_lump",
        region_text(namespace)
    )
}

/// Structured result of [`Cyberdemon::replace_lump`](crate::server::Cyberdemon).
#[derive(Debug, Serialize)]
pub(in crate::tools::lumps) struct ReplaceReport {
    /// The lump that was written.
    pub(super) name: LumpName,
    /// Its position in the working archive.
    pub(super) index: usize,
    /// New payload bytes.
    pub(super) size: usize,
    /// Whether the lump had to be brought into the working archive first.
    pub(super) created_override: bool,
}

/// The working-archive half of `replace_lump`: pick the target and commit one
/// undoable write.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no archive holds the name (or `occurrence`
/// misses), and [`place::insert`]'s errors while an override is being placed.
pub(super) fn run_replace(
    session: &mut Session,
    selector: &EditSelector,
    data: &[u8],
) -> Result<ReplaceReport, CyberError> {
    let target = match working_match(session, selector)? {
        Some(index) => Target::Working { index },
        None => merged_target(session, selector)?,
    };
    let created_override = target.creates_an_override();
    let copied = target.needs_merged_view().then(|| session.merged());
    let label = format!("replace_lump {}", selector.name);
    let write = Replacement {
        name: selector.name,
        data,
        target,
        merged: copied.as_deref().unwrap_or(&[]),
    };
    let index = session.edit(&label, |wad| write.apply(wad))?;
    Ok(ReplaceReport {
        name: selector.name,
        index,
        size: data.len(),
        created_override,
    })
}

/// Where the merged view says this lump lives, for the copy-on-write path.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the merged view holds no match.
fn merged_target(session: &Session, selector: &EditSelector) -> Result<Target, CyberError> {
    let found = build_rows(session, Scope::Merged)?
        .into_iter()
        .rev()
        .find(|row| selector.accepts(row));
    let Some(row) = found else {
        return Err(CyberError::NotFound(nowhere_message(selector)));
    };
    Ok(match (row.map, row.namespace) {
        (Some(map), _) => Target::Map { map },
        (None, Some(namespace)) => Target::Region { namespace },
        (None, None) => Target::Append,
    })
}

/// Runs `replace_lump` end to end: validate, fetch the payload (never holding
/// the session lock), then commit the undoable edit.
///
/// # Errors
///
/// Whatever [`edit_selector`], [`pick_new_data`], [`fetch_data`] or
/// [`run_replace`] reports.
pub(in crate::tools::lumps) async fn replace_lump(
    state: &AppState,
    args: &ReplaceLumpParams,
) -> Result<ReplaceReport, CyberError> {
    let selector = edit_selector(&args.name, args.namespace.as_deref(), args.occurrence)?;
    let data = fetch_data(state, pick_new_data(args)?).await?;
    state.with_session(|session| run_replace(session, &selector, &data))
}
