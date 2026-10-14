//! The lump-editing tools' arguments and working-archive mutations:
//! `replace_lump`, `remove_lump` and `rename_lump`.
//!
//! Every [`crate::tools::lumps`] router method hands its arguments to this
//! module. The shared shape is: parse and validate everything, fetch any
//! `source` *before* the session lock is taken, then run exactly one
//! [`crate::session::Session::edit`] whose closure either finishes the whole
//! change or reports an error that rolls it back. The undo label is the tool's
//! name followed by the lump names it touched.
//!
//! `replace_lump` is the copy-on-write tool. A lump the working archive holds
//! is overwritten in place; one that only the `IWAD` or a resource layer holds
//! is brought in first — a whole map block through
//! [`crate::session::cow::copy_map_block`], a region member through the shared
//! placement in [`crate::tools::lumps::place`], anything else appended — and
//! the report says so with `"created_override": true`. The read-only layers
//! never change, so `remove_lump` and `rename_lump` work on the working
//! archive alone and refuse a name it does not hold.

use alloc::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::cow;
use crate::session::{MergedLump, Scope, Session};
use crate::state::AppState;
use crate::tools::lumps::place::{self, Placement};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{LumpInfo, Namespace, analyze};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::replace_lump`](crate::server::Cyberdemon).
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ReplaceLumpParams {
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

/// Arguments for [`Cyberdemon::remove_lump`](crate::server::Cyberdemon).
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RemoveLumpParams {
    /// Lump names to remove from the working archive: at least one, each 1 to
    /// 8 printable `ASCII` characters, matched case-insensitively.
    pub(super) names: Vec<String>,
    /// Keep only matches inside this namespace region (`sprites`, `flats`,
    /// ...); it applies to every name given.
    #[serde(default)]
    pub(super) namespace: Option<String>,
    /// 0-based index among the working archive's matches of the named lump
    /// (default the last one). Only allowed when `names` holds exactly one
    /// name.
    #[serde(default)]
    pub(super) occurrence: Option<usize>,
    /// Remove **every** working copy of each name instead of just the
    /// selected one (default `false`).
    #[serde(default)]
    pub(super) all_occurrences: bool,
}

/// Arguments for [`Cyberdemon::rename_lump`](crate::server::Cyberdemon).
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RenameLumpParams {
    /// Lump name to rename, from the working archive only: 1 to 8 printable
    /// `ASCII` characters, matched case-insensitively.
    pub(super) name: String,
    /// The new name: 1 to 8 printable `ASCII` characters, uppercased.
    pub(super) new_name: String,
    /// Keep only matches inside this namespace region (`sprites`, `flats`,
    /// ...).
    #[serde(default)]
    pub(super) namespace: Option<String>,
    /// 0-based index among the working archive's same-name matches; the
    /// default is the last one.
    #[serde(default)]
    pub(super) occurrence: Option<usize>,
    /// Rename even when the target region already holds a lump with
    /// `new_name` (default `false`).
    #[serde(default)]
    pub(super) allow_duplicate: bool,
}

/// The name, region and occurrence the lump-editing tools select on.
#[derive(Debug)]
struct EditSelector {
    /// The lump name to edit.
    name: LumpName,
    /// Only matches inside this region count; `None` is any region.
    namespace: Option<Namespace>,
    /// 0-based index among the working archive's matches; `None` is the last.
    occurrence: Option<usize>,
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
fn edit_selector(
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
fn region_text(namespace: Option<Namespace>) -> String {
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
fn working_match(session: &Session, selector: &EditSelector) -> Result<Option<usize>, CyberError> {
    let matched: Vec<usize> = build_rows(session, Scope::Working)?
        .into_iter()
        .filter(|row| selector.accepts(row))
        .map(|row| row.index)
        .collect();
    pick_occurrence(&matched, selector)
}

/// The one match `occurrence` selects, the last when it is `None`; `None`
/// only when nothing matched.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming how many copies exist.
fn pick_occurrence(
    matched: &[usize],
    selector: &EditSelector,
) -> Result<Option<usize>, CyberError> {
    if matched.is_empty() {
        return Ok(None);
    }
    let count = matched.len();
    let last = count.saturating_sub(1);
    let wanted = selector.occurrence.unwrap_or(last);
    let Some(&index) = matched.get(wanted) else {
        return Err(CyberError::NotFound(format!(
            "only {count} lumps named {} in the working archive: occurrence {wanted} is out of \
             range (0 to {last})",
            selector.name
        )));
    };
    Ok(Some(index))
}

/// Where a `replace_lump` payload goes in the working archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
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
    const fn creates_an_override(self) -> bool {
        !matches!(self, Self::Working { .. })
    }

    /// Whether reaching this target needs the merged view to copy from.
    const fn needs_merged_view(self) -> bool {
        matches!(self, Self::Map { .. })
    }
}

/// One `replace_lump` write: the bytes, where they go and what has to be
/// copied in first.
struct Replacement<'edit> {
    /// The lump to write.
    name: LumpName,
    /// The new payload.
    data: &'edit [u8],
    /// Where it lives, or how to bring it in.
    target: Target,
    /// The merged view, needed to copy a map block.
    merged: &'edit [MergedLump],
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
    fn apply(&self, wad: &mut Wad) -> Result<usize, CyberError> {
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
fn overwrite(
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
enum NewData {
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
fn pick_new_data(args: &ReplaceLumpParams) -> Result<NewData, CyberError> {
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
async fn fetch_data(state: &AppState, data: NewData) -> Result<Vec<u8>, CyberError> {
    match data {
        NewData::Text(body) => Ok(body.into_bytes()),
        NewData::Hex(digits) => super::hex_bytes("hex", &digits),
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
fn not_working_message(name: LumpName, namespace: Option<Namespace>) -> String {
    format!(
        "lump {name} is not in {}: the IWAD and resource layers are read-only, so their lumps \
         cannot be removed or renamed, only overridden with replace_lump",
        region_text(namespace)
    )
}

/// Structured result of [`Cyberdemon::replace_lump`](crate::server::Cyberdemon).
#[derive(Debug, Serialize)]
pub(super) struct ReplaceReport {
    /// The lump that was written.
    name: LumpName,
    /// Its position in the working archive.
    index: usize,
    /// New payload bytes.
    size: usize,
    /// Whether the lump had to be brought into the working archive first.
    created_override: bool,
}

/// The working-archive half of `replace_lump`: pick the target and commit one
/// undoable write.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no archive holds the name (or `occurrence`
/// misses), and [`place::insert`]'s errors while an override is being placed.
fn run_replace(
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
pub(super) async fn replace_lump(
    state: &AppState,
    args: &ReplaceLumpParams,
) -> Result<ReplaceReport, CyberError> {
    let selector = edit_selector(&args.name, args.namespace.as_deref(), args.occurrence)?;
    let data = fetch_data(state, pick_new_data(args)?).await?;
    state.with_session(|session| run_replace(session, &selector, &data))
}

/// A validated `remove_lump` call: the names and how many copies of each go.
#[derive(Debug)]
struct RemovePlan {
    /// The names to remove, in the order given.
    names: Vec<LumpName>,
    /// Only matches inside this region count.
    namespace: Option<Namespace>,
    /// The occurrence to take; only ever set for a single name.
    occurrence: Option<usize>,
    /// Whether every match of each name goes.
    all: bool,
}

/// Validate `remove_lump`'s arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty `names` list, a name that
/// does not parse, an unknown `namespace`, or `occurrence` beside more than
/// one name.
fn plan_remove(args: &RemoveLumpParams) -> Result<RemovePlan, CyberError> {
    if args.names.is_empty() {
        return Err(CyberError::InvalidArgument(
            "remove_lump needs at least one name: pass names as a list of lump names".to_owned(),
        ));
    }
    let names: Vec<LumpName> = args
        .names
        .iter()
        .map(|text| LumpName::parse(text))
        .collect::<Result<Vec<LumpName>, CyberError>>()?;
    Ok(RemovePlan {
        namespace: super::parse_namespace(args.namespace.as_deref())?,
        occurrence: allowed_occurrence(args.occurrence, names.len())?,
        all: args.all_occurrences,
        names,
    })
}

/// The `occurrence` argument, which only makes sense for one name.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when several names share an occurrence.
fn allowed_occurrence(
    occurrence: Option<usize>,
    count: usize,
) -> Result<Option<usize>, CyberError> {
    match (occurrence, count) {
        (Some(wanted), 1) => Ok(Some(wanted)),
        (Some(_), _) => Err(CyberError::InvalidArgument(
            "occurrence picks one copy of one lump: it cannot be combined with several names in \
             remove_lump (drop it, or pass all_occurrences)"
                .to_owned(),
        )),
        (None, _) => Ok(None),
    }
}

/// One removed lump: its name and where it stood before the removal.
#[derive(Debug, Serialize)]
struct RemovedLump {
    /// The lump's name.
    name: LumpName,
    /// Its position in the working archive before it was removed.
    index: usize,
}

/// Structured result of [`Cyberdemon::remove_lump`](crate::server::Cyberdemon).
#[derive(Debug, Serialize)]
pub(super) struct RemoveReport {
    /// What went, in directory order.
    removed: Vec<RemovedLump>,
}

/// The working-archive positions of `name` inside the plan's region.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the working archive holds no such lump, or
/// when `occurrence` reaches past its matches.
fn matched_positions(
    infos: &[LumpInfo],
    name: LumpName,
    plan: &RemovePlan,
) -> Result<Vec<usize>, CyberError> {
    let matched: Vec<usize> = infos
        .iter()
        .filter(|info| info.name == name && in_region(info.namespace, plan.namespace))
        .map(|info| info.index)
        .collect();
    if matched.is_empty() {
        return Err(CyberError::NotFound(not_working_message(
            name,
            plan.namespace,
        )));
    }
    if plan.all {
        return Ok(matched);
    }
    let count = matched.len();
    let last = count.saturating_sub(1);
    let wanted = plan.occurrence.unwrap_or(last);
    let Some(&index) = matched.get(wanted) else {
        return Err(CyberError::NotFound(format!(
            "only {count} lumps named {name} in the working archive: occurrence {wanted} is out \
             of range (0 to {last})"
        )));
    };
    Ok(vec![index])
}

/// The whole `remove_lump` step: resolve every name against the working
/// archive — a name that is not there aborts the edit, so nothing is removed —
/// then delete the selected positions.
///
/// # Errors
///
/// [`CyberError::NotFound`] for the first name the working archive lacks.
fn apply_remove(wad: &mut Wad, plan: &RemovePlan) -> Result<Vec<RemovedLump>, CyberError> {
    let analysis = analyze(&wad.lumps);
    let mut doomed: Vec<usize> = Vec::new();
    for name in &plan.names {
        let found = matched_positions(&analysis.lumps, *name, plan)?;
        doomed.extend(found);
    }
    let removed: Vec<RemovedLump> = analysis
        .lumps
        .iter()
        .filter(|info| doomed.contains(&info.index))
        .map(|info| RemovedLump {
            name: info.name,
            index: info.index,
        })
        .collect();
    let lumps: Vec<Lump> = core::mem::take(&mut wad.lumps)
        .into_iter()
        .enumerate()
        .filter(|&(index, _)| !doomed.contains(&index))
        .map(|(_, lump)| lump)
        .collect();
    wad.lumps = lumps;
    Ok(removed)
}

/// The undo label for a removal: `remove_lump A, B, ...`.
fn remove_label(names: &[LumpName]) -> String {
    let listed: Vec<String> = names.iter().map(ToString::to_string).collect();
    format!("remove_lump {}", listed.join(", "))
}

/// Runs `remove_lump`: one undoable deletion of every named lump from the
/// working archive.
///
/// # Errors
///
/// [`plan_remove`]'s refusals, and [`CyberError::NotFound`] when any name is
/// missing from the working archive, in which case nothing is removed.
pub(super) fn remove_lump(
    session: &mut Session,
    args: &RemoveLumpParams,
) -> Result<RemoveReport, CyberError> {
    let plan = plan_remove(args)?;
    let label = remove_label(&plan.names);
    let removed = session.edit(&label, |wad| apply_remove(wad, &plan))?;
    Ok(RemoveReport { removed })
}

/// Structured result of [`Cyberdemon::rename_lump`](crate::server::Cyberdemon).
#[derive(Debug, Serialize)]
pub(super) struct RenameReport {
    /// The name the lump had.
    old_name: LumpName,
    /// The name it has now.
    new_name: LumpName,
    /// Its position in the working archive.
    index: usize,
}

/// Renames the lump at `index`, refusing an unasked-for same-name twin in the
/// region that lump sits in.
///
/// # Errors
///
/// [`CyberError::Internal`] for a stale index, [`CyberError::AlreadyExists`]
/// when another lump already carries `to` in the same region and
/// `allow_duplicate` is `false`. Nothing changes in either case.
fn apply_rename(
    wad: &mut Wad,
    index: usize,
    to: LumpName,
    allow_duplicate: bool,
) -> Result<(), CyberError> {
    let analysis = analyze(&wad.lumps);
    let Some(info) = analysis.lumps.get(index) else {
        return Err(CyberError::Internal(format!(
            "no lump at index {index} of the working archive: it was expected to be renamed to \
             {to}"
        )));
    };
    let namespace = info.namespace;
    if !allow_duplicate && holds_name(&analysis.lumps, index, to, namespace) {
        return Err(CyberError::AlreadyExists(format!(
            "a lump named {to} already stands in {}: pass allow_duplicate true to keep both \
             copies, or rename to another name",
            region_text(namespace)
        )));
    }
    rename_in_place(wad, index, to)
}

/// Whether any lump but `index` already carries `name` inside `namespace`.
fn holds_name(
    infos: &[LumpInfo],
    index: usize,
    name: LumpName,
    namespace: Option<Namespace>,
) -> bool {
    infos
        .iter()
        .any(|other| other.index != index && other.name == name && other.namespace == namespace)
}

/// Writes `to` over the name of the lump at `index`.
///
/// # Errors
///
/// [`CyberError::Internal`] when the directory holds no lump there.
fn rename_in_place(wad: &mut Wad, index: usize, to: LumpName) -> Result<(), CyberError> {
    let Some(lump) = wad.lumps.get_mut(index) else {
        return Err(CyberError::Internal(format!(
            "the working archive holds no lump at index {index} to rename to {to}"
        )));
    };
    lump.name = to;
    Ok(())
}

/// Runs `rename_lump`: one undoable rename inside the working archive.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name or `new_name` that does not
/// parse, [`CyberError::NotFound`] when the working archive holds no match,
/// [`CyberError::AlreadyExists`] for a twin without `allow_duplicate`.
pub(super) fn rename_lump(
    session: &mut Session,
    args: &RenameLumpParams,
) -> Result<RenameReport, CyberError> {
    let selector = edit_selector(&args.name, args.namespace.as_deref(), args.occurrence)?;
    let to = LumpName::parse(&args.new_name)?;
    let index = working_match(session, &selector)?.ok_or_else(|| {
        CyberError::NotFound(not_working_message(selector.name, selector.namespace))
    })?;
    let label = format!("rename_lump {} -> {to}", selector.name);
    session.edit(&label, |wad| {
        apply_rename(wad, index, to, args.allow_duplicate)
    })?;
    Ok(RenameReport {
        old_name: selector.name,
        new_name: to,
        index,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        EditSelector, NewData, RemoveLumpParams, RemovePlan, RenameLumpParams, ReplaceLumpParams,
        Replacement, Target, allowed_occurrence, apply_remove, apply_rename, edit_selector,
        fetch_data, in_region, matched_positions, not_working_message, overwrite, pick_new_data,
        plan_remove, region_text, remove_label, remove_lump, rename_in_place, rename_lump,
        run_replace,
    };
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::state::AppState;
    use crate::tools::lumps::rows::build_rows;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::{Namespace, analyze};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` carrying `payload`.
    fn carrying(text: &str, payload: &str) -> Lump {
        Lump::new(name(text), payload.as_bytes().to_vec())
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A `PWAD` holding `lumps`.
    fn pwad(lumps: Vec<Lump>) -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps,
        }
    }

    /// The directory names of an archive, in order.
    fn names(wad: &Wad) -> Vec<String> {
        wad.lumps.iter().map(|lump| lump.name.to_string()).collect()
    }

    /// The payloads of an archive as `NAME(payload)`, in order.
    fn with_payloads(wad: &Wad) -> Vec<String> {
        wad.lumps
            .iter()
            .map(|lump| {
                let text = String::from_utf8(lump.data.to_vec()).expect("test payloads are text");
                format!("{}({text})", lump.name)
            })
            .collect()
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// An [`EditSelector`] for `text` with no refinements.
    fn selector_for(text: &str) -> EditSelector {
        edit_selector(text, None, None).expect("test selectors are valid")
    }

    /// A session: an `IWAD` with a loose global, a flats region and one `MAP01`
    /// block with its `GL` run, over an empty working archive.
    fn loaded() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    carrying("PLAYPAL", "base-palette"),
                    marker("F_START"),
                    carrying("FLATA", "base-flat"),
                    marker("F_END"),
                    marker("MAP01"),
                    carrying("THINGS", "base-things"),
                    carrying("SECTORS", "base-sectors"),
                    marker("GL_MAP01"),
                    carrying("GL_VERT", "base-gl"),
                ],
            },
        ));
        session
    }

    /// Appends `lumps` to the working archive as one edit.
    fn seed(session: &mut Session, lumps: Vec<Lump>) {
        session
            .edit("seed", move |wad| {
                wad.lumps.extend(lumps);
                Ok(())
            })
            .expect("plain appends");
    }

    /// A `replace_lump` argument set carrying `text` and `digits`.
    fn replace_args(text: Option<&str>, digits: Option<&str>) -> ReplaceLumpParams {
        ReplaceLumpParams {
            name: "NOTE".to_owned(),
            namespace: None,
            occurrence: None,
            text: text.map(str::to_owned),
            hex: digits.map(str::to_owned),
            source: None,
        }
    }

    /// A `remove_lump` argument set.
    fn remove_args(
        listed: &[&str],
        namespace: Option<&str>,
        occurrence: Option<usize>,
        all: bool,
    ) -> RemoveLumpParams {
        RemoveLumpParams {
            names: listed.iter().map(|text| (*text).to_owned()).collect(),
            namespace: namespace.map(str::to_owned),
            occurrence,
            all_occurrences: all,
        }
    }

    /// A `remove_lump` plan for `listed` names, no region filter.
    fn remove_plan(listed: &[&str], all: bool) -> RemovePlan {
        plan_remove(&remove_args(listed, None, None, all)).expect("test plans are valid")
    }

    /// A `remove_lump` plan for `TWIN` with an explicit occurrence.
    fn twin_plan(occurrence: Option<usize>) -> RemovePlan {
        RemovePlan {
            names: vec![name("TWIN")],
            namespace: None,
            occurrence,
            all: false,
        }
    }

    /// A `rename_lump` argument set.
    fn rename_args(from: &str, to: &str, allow: bool) -> RenameLumpParams {
        RenameLumpParams {
            name: from.to_owned(),
            new_name: to.to_owned(),
            namespace: None,
            occurrence: None,
            allow_duplicate: allow,
        }
    }

    #[test]
    fn selectors_parse_names_and_regions_or_refuse() {
        let plain = edit_selector("flat", Some("flats"), Some(2)).expect("valid");
        assert_eq!(plain.name, name("FLAT"), "the name uppercases");
        assert_eq!(plain.namespace, Some(Namespace::Flats), "the region parses");
        assert_eq!(plain.occurrence, Some(2), "the occurrence carries");
        let loose = selector_for("NOTE");
        assert_eq!(loose.namespace, None, "no region means any match");
        assert_eq!(loose.occurrence, None, "no occurrence means the last one");
        let long = edit_selector("TOO LONG FOR WAD", None, None).expect_err("over eight bytes");
        assert_eq!(code(&long), "invalid_argument", "bad name code");
        let region = edit_selector("NOTE", Some("nope"), None).expect_err("not a region");
        assert_eq!(code(&region), "invalid_argument", "bad namespace code");
    }

    #[test]
    fn a_selector_accepts_only_its_own_name_and_region() {
        let rows = build_rows(&loaded(), Scope::Merged).expect("the view lists");
        let flat = rows
            .iter()
            .find(|row| row.name == name("FLATA"))
            .expect("the base flat");
        let palette = rows
            .iter()
            .find(|row| row.name == name("PLAYPAL"))
            .expect("the base palette");
        assert!(
            edit_selector("flata", Some("flats"), None)
                .expect("valid")
                .accepts(flat),
            "the region copy matches its own selector"
        );
        assert!(
            !selector_for("PLAYPAL").accepts(flat),
            "another name never matches"
        );
        assert!(
            !edit_selector("FLATA", Some("sprites"), None)
                .expect("valid")
                .accepts(flat),
            "the wrong region does not match"
        );
        assert!(
            selector_for("PLAYPAL").accepts(palette),
            "a loose lump matches with no region filter"
        );
    }

    #[test]
    fn region_filters_match_the_same_region_only() {
        let flats = Some(Namespace::Flats);
        assert!(in_region(None, None), "no filter matches a loose lump");
        assert!(in_region(flats, None), "no filter matches a region member");
        assert!(in_region(flats, flats), "the same region matches");
        assert!(
            !in_region(None, flats),
            "a loose lump is not in the flats region"
        );
        assert!(
            !in_region(flats, Some(Namespace::Sprites)),
            "a flat is no sprite"
        );
        assert_eq!(
            region_text(flats),
            "the flats region of the working archive",
            "the region text names the region"
        );
        assert_eq!(
            region_text(None),
            "the working archive",
            "a loose lump's region text"
        );
    }

    #[test]
    fn replace_needs_exactly_one_payload_field() {
        assert!(
            matches!(pick_new_data(&replace_args(Some("t"), None)).expect("text"), NewData::Text(body) if body == "t"),
            "text alone"
        );
        assert!(
            matches!(
                pick_new_data(&replace_args(None, Some("4D55"))).expect("hex"),
                NewData::Hex(_)
            ),
            "hex alone"
        );
        let mut sourced = replace_args(None, None);
        sourced.source = Some("/x/y.bin".to_owned());
        assert!(
            matches!(pick_new_data(&sourced).expect("source"), NewData::Source(raw) if raw == "/x/y.bin"),
            "source alone"
        );
        let none = pick_new_data(&replace_args(None, None)).expect_err("no payload");
        assert_eq!(code(&none), "invalid_argument", "no payload code");
        assert!(
            none.to_string().contains("exactly one"),
            "the message states the rule: {none}"
        );
        let both = pick_new_data(&replace_args(Some("t"), Some("4D55"))).expect_err("two fields");
        assert_eq!(code(&both), "invalid_argument", "two payload fields code");
        assert!(
            both.to_string().contains("exactly one"),
            "the message states the rule: {both}"
        );
        let mut all_three = replace_args(Some("t"), Some("4D55"));
        all_three.source = Some("/x/y.bin".to_owned());
        assert_eq!(
            code(&pick_new_data(&all_three).expect_err("three fields")),
            "invalid_argument",
            "three payload fields code"
        );
    }

    #[tokio::test]
    async fn payload_bytes_come_from_text_or_hex_and_report_bad_hex() {
        let state = AppState::default();
        assert_eq!(
            fetch_data(&state, NewData::Text("hi".to_owned()))
                .await
                .expect("text is taken as-is"),
            vec![b'h', b'i'],
            "the text's bytes"
        );
        assert_eq!(
            fetch_data(&state, NewData::Hex("4D 55".to_owned()))
                .await
                .expect("hex decodes"),
            vec![0x4D_u8, 0x55],
            "spaces ignored"
        );
        let error = fetch_data(&state, NewData::Hex("4Z".to_owned()))
            .await
            .expect_err("not hex");
        assert_eq!(code(&error), "invalid_argument", "bad hex code");
        assert!(
            error.to_string().starts_with("hex "),
            "the message names the field: {error}"
        );
    }

    #[test]
    fn a_working_match_is_the_last_one_unless_occurrence_says_otherwise() {
        let mut session = loaded();
        seed(
            &mut session,
            vec![carrying("TWIN", "first"), carrying("TWIN", "second")],
        );
        assert_eq!(
            super::working_match(&session, &selector_for("TWIN")).expect("matches exist"),
            Some(1),
            "the default is the last copy"
        );
        let first = edit_selector("TWIN", None, Some(0)).expect("valid");
        assert_eq!(
            super::working_match(&session, &first).expect("occurrence 0 exists"),
            Some(0),
            "an explicit occurrence picks by directory order"
        );
        let past = edit_selector("TWIN", None, Some(2)).expect("valid");
        let error = super::working_match(&session, &past).expect_err("only two copies");
        assert_eq!(code(&error), "not_found", "past the end code");
        assert!(
            error.to_string().contains("only 2 lumps named TWIN"),
            "the message counts the copies: {error}"
        );
        assert_eq!(
            super::working_match(&session, &selector_for("ABSENT")).expect("no matches"),
            None,
            "an unknown name is no match at all"
        );
    }

    #[test]
    fn targets_know_when_they_grow_the_directory_and_when_they_copy() {
        let in_place = Target::Working { index: 3 };
        let copied = Target::Map { map: name("MAP01") };
        let region = Target::Region {
            namespace: Namespace::Flats,
        };
        assert!(
            !in_place.creates_an_override(),
            "an overwrite is no override"
        );
        assert!(!in_place.needs_merged_view(), "nothing to copy");
        assert!(
            copied.creates_an_override(),
            "a copied block is an override"
        );
        assert!(copied.needs_merged_view(), "the block comes from the view");
        assert!(
            region.creates_an_override(),
            "a region insert is an override"
        );
        assert!(!region.needs_merged_view(), "but no block copy");
        assert!(
            Target::Append.creates_an_override(),
            "an append is an override"
        );
        assert!(!Target::Append.needs_merged_view(), "and no block copy");
    }

    #[test]
    fn replacing_a_working_lump_overwrites_it_in_place() {
        let mut session = loaded();
        seed(&mut session, vec![carrying("NOTE", "old")]);
        let report = run_replace(&mut session, &selector_for("NOTE"), b"new").expect("replaced");
        assert_eq!(
            serde_json::to_value(&report).expect("serializable"),
            json!({ "name": "NOTE", "index": 0_u64, "size": 3_u64, "created_override": false }),
            "the documented result shape"
        );
        assert_eq!(
            with_payloads(session.working()),
            vec!["NOTE(new)"],
            "same place, new bytes"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["replace_lump NOTE", "seed"],
            "the undo label on top of the seeded lump"
        );
    }

    #[test]
    fn replacing_a_layer_flat_creates_an_override_inside_the_region() {
        let mut session = loaded();
        let report = run_replace(&mut session, &selector_for("FLATA"), b"mine").expect("override");
        assert!(report.created_override, "the flat was brought in");
        assert_eq!(report.index, 1, "between the created region markers");
        assert_eq!(
            names(session.working()),
            vec!["F_START", "FLATA", "F_END"],
            "a flats region was created in the working archive"
        );
        assert_eq!(
            report_of_flat(&session),
            "mine",
            "the override won the merged view"
        );
        assert_eq!(
            session.layers().first().expect("the base").wad.len(),
            9,
            "the layer is untouched"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["replace_lump FLATA"],
            "one undoable edit"
        );
    }

    /// The payload of the merged view's `FLATA`, as text.
    fn report_of_flat(session: &Session) -> String {
        let flat = session
            .merged()
            .into_iter()
            .find(|entry| entry.lump.name == name("FLATA"))
            .expect("the view holds a FLATA");
        String::from_utf8(flat.lump.data.to_vec()).expect("test payloads are text")
    }

    #[test]
    fn an_existing_region_receives_the_override_at_its_end() {
        let mut session = loaded();
        seed(
            &mut session,
            vec![
                marker("F_START"),
                carrying("FLATB", "mine"),
                marker("F_END"),
            ],
        );
        let report = run_replace(&mut session, &selector_for("FLATA"), b"new")
            .expect("the override joins the region");
        assert_eq!(report.index, 2, "just before the working archive's F_END");
        assert_eq!(
            names(session.working()),
            vec!["F_START", "FLATB", "FLATA", "F_END"],
            "the existing region grew instead of a second one appearing"
        );
    }

    #[test]
    fn replacing_iwad_map_data_copies_the_whole_block_first() {
        let mut session = loaded();
        let report = run_replace(&mut session, &selector_for("THINGS"), b"mine")
            .expect("the block was copied");
        assert!(report.created_override, "the block came from the layer");
        assert_eq!(report.index, 1, "the copied THINGS");
        assert_eq!(report.size, 4, "the new bytes");
        assert_eq!(
            names(session.working()),
            vec!["MAP01", "THINGS", "SECTORS", "GL_MAP01", "GL_VERT"],
            "the whole MAP01 block including its GL run, in view order"
        );
        assert_eq!(
            with_payloads(session.working()).get(1).cloned(),
            Some("THINGS(mine)".to_owned()),
            "the copied member carries the new bytes"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["replace_lump THINGS"],
            "copy and write are one undoable edit"
        );
    }

    #[test]
    fn replacing_a_loose_layer_global_appends_the_override() {
        let mut session = loaded();
        let report = run_replace(&mut session, &selector_for("PLAYPAL"), b"new")
            .expect("the global was overridden");
        assert!(report.created_override, "an override arrived");
        assert_eq!(report.index, 0, "appended to the empty working archive");
        assert_eq!(names(session.working()), vec!["PLAYPAL"], "one loose lump");
        assert_eq!(
            session
                .find(name("PLAYPAL"), None)
                .expect("the view still has one")
                .source,
            crate::session::Source::Working,
            "the working copy wins the merged view"
        );
    }

    #[test]
    fn replacing_a_name_no_archive_holds_is_not_found() {
        let mut session = loaded();
        let error = run_replace(&mut session, &selector_for("NOPE"), b"x")
            .expect_err("nothing is named NOPE");
        assert_eq!(code(&error), "not_found", "missing lump code");
        assert!(
            error.to_string().contains("add_lump"),
            "the message names the tool that creates one: {error}"
        );
        assert!(session.working().is_empty(), "nothing was created");
        assert!(session.undo_labels().is_empty(), "and nothing recorded");
        assert!(!session.is_dirty(), "so the session is still clean");
    }

    #[test]
    fn a_namespace_selector_only_overrides_that_regions_copy() {
        let mut session = loaded();
        let in_sprites = edit_selector("FLATA", Some("sprites"), None).expect("valid");
        let error = run_replace(&mut session, &in_sprites, b"x").expect_err("no sprite FLATA");
        assert_eq!(code(&error), "not_found", "wrong region");
        assert!(
            error.to_string().contains("in the sprites region"),
            "the message names the region: {error}"
        );
        assert!(session.working().is_empty(), "nothing was created");
    }

    #[test]
    fn overwriting_a_stale_index_reports_internal() {
        let mut wad = pwad(vec![carrying("NOTE", "old")]);
        let error = overwrite(&mut wad, 9, name("NOTE"), b"new").expect_err("only one lump");
        assert_eq!(code(&error), "internal", "stale index code");
        assert!(
            error.to_string().contains("index 9"),
            "the message names the index: {error}"
        );
        assert_eq!(with_payloads(&wad), vec!["NOTE(old)"], "nothing changed");
        assert_eq!(
            overwrite(&mut wad, 0, name("NOTE"), b"new").expect("a real index"),
            0,
            "and the same call works when the lump is there"
        );
        assert_eq!(with_payloads(&wad), vec!["NOTE(new)"], "written in place");
    }

    #[test]
    fn a_copied_block_without_the_named_lump_is_not_found() {
        let merged = loaded().merged();
        let mut wad = Wad::default();
        let write = Replacement {
            name: name("SIDEDEFS"),
            data: b"x".as_slice(),
            target: Target::Map { map: name("MAP01") },
            merged: &merged,
        };
        let error = write.apply(&mut wad).expect_err("MAP01 has no SIDEDEFS");
        assert_eq!(code(&error), "not_found", "missing member code");
        assert!(
            error.to_string().contains("the copied MAP01 block"),
            "the message says what disagrees: {error}"
        );
        assert_eq!(
            names(&wad),
            vec!["MAP01", "THINGS", "SECTORS", "GL_MAP01", "GL_VERT"],
            "the copy happened before the lookup failed, and the caller rolls it back"
        );
        assert!(
            run_replace(&mut loaded(), &selector_for("THINGS"), b"x")
                .is_ok_and(|report| report.created_override),
            "the same write through a session succeeds"
        );
    }

    #[test]
    fn remove_plans_refuse_an_empty_list_bad_names_and_shared_occurrence() {
        let empty = plan_remove(&remove_args(&[], None, None, false)).expect_err("no names");
        assert_eq!(code(&empty), "invalid_argument", "empty list code");
        assert!(
            empty.to_string().contains("at least one"),
            "the message states the rule: {empty}"
        );
        let bad =
            plan_remove(&remove_args(&["OK", "TOO LONG"], None, None, false)).expect_err("one bad");
        assert_eq!(code(&bad), "invalid_argument", "bad name code");
        let shared = remove_args(&["A", "B"], None, Some(1), false);
        let error = plan_remove(&shared).expect_err("occurrence with two names");
        assert_eq!(code(&error), "invalid_argument", "occurrence conflict code");
        assert!(
            error.to_string().contains("occurrence"),
            "the message names the field: {error}"
        );
        let region = remove_args(&["A"], Some("nope"), None, false);
        assert_eq!(
            code(&plan_remove(&region).expect_err("not a region")),
            "invalid_argument",
            "bad namespace code"
        );
        let once =
            plan_remove(&remove_args(&["a", "B"], Some("flats"), None, true)).expect("valid");
        assert_eq!(once.names, vec![name("A"), name("B")], "names uppercase");
        assert_eq!(once.namespace, Some(Namespace::Flats), "the region carries");
        assert!(once.all, "and the all flag");
        assert_eq!(once.occurrence, None, "no occurrence was given");
    }

    #[test]
    fn allowed_occurrence_only_ever_accompanies_one_name() {
        assert_eq!(
            allowed_occurrence(Some(3), 1).expect("one name"),
            Some(3),
            "kept"
        );
        assert_eq!(
            allowed_occurrence(None, 4).expect("unset"),
            None,
            "absent is fine with any count"
        );
        let error = allowed_occurrence(Some(1), 2).expect_err("two names");
        assert_eq!(code(&error), "invalid_argument", "conflict code");
    }

    #[test]
    fn removing_takes_the_last_copy_unless_asked_for_more() {
        let mut wad = pwad(vec![
            carrying("TWIN", "one"),
            carrying("OTHER", "x"),
            carrying("TWIN", "two"),
        ]);
        let removed = apply_remove(&mut wad, &remove_plan(&["TWIN"], false)).expect("one goes");
        assert_eq!(
            removed
                .iter()
                .map(|entry| format!("{}:{}", entry.name, entry.index))
                .collect::<Vec<String>>(),
            vec!["TWIN:2"],
            "the last match, reported at its old index"
        );
        assert_eq!(names(&wad), vec!["TWIN", "OTHER"], "and the first stays");
        let again = apply_remove(&mut wad, &remove_plan(&["TWIN"], true)).expect("the last goes");
        assert_eq!(again.len(), 1, "one position was left to remove");
        assert_eq!(names(&wad), vec!["OTHER"], "both twins are gone");
    }

    #[test]
    fn removing_several_names_reports_directory_order_and_labels_the_edit() {
        let mut wad = pwad(vec![
            carrying("B", "b"),
            carrying("A", "a"),
            marker("F_START"),
            carrying("C", "c"),
            marker("F_END"),
        ]);
        let plan = remove_plan(&["C", "A"], false);
        let removed = apply_remove(&mut wad, &plan).expect("both names exist");
        assert_eq!(
            removed
                .iter()
                .map(|entry| format!("{}:{}", entry.name, entry.index))
                .collect::<Vec<String>>(),
            vec!["A:1", "C:3"],
            "reported in directory order with pre-removal indices"
        );
        assert_eq!(names(&wad), vec!["B", "F_START", "F_END"], "two went");
        assert_eq!(
            remove_label(&plan.names),
            "remove_lump C, A",
            "the label keeps the caller's order"
        );
    }

    #[test]
    fn a_missing_name_aborts_a_multi_name_removal_entirely() {
        let mut wad = pwad(vec![carrying("A", "a"), carrying("B", "b")]);
        let before = names(&wad);
        let error = apply_remove(&mut wad, &remove_plan(&["A", "GHOST"], false))
            .expect_err("GHOST is absent");
        assert_eq!(code(&error), "not_found", "missing name code");
        assert!(
            error.to_string().contains("read-only"),
            "the message explains the layers: {error}"
        );
        assert!(
            error.to_string().contains("replace_lump"),
            "the message names the way to override: {error}"
        );
        assert_eq!(names(&wad), before, "nothing was removed");
    }

    #[test]
    fn removals_honour_the_region_and_the_selected_occurrence() {
        let mut wad = pwad(vec![
            carrying("FLATA", "loose"),
            marker("F_START"),
            carrying("FLATA", "region"),
            marker("F_END"),
        ]);
        let flats = plan_remove(&remove_args(&["FLATA"], Some("flats"), None, false))
            .expect("a valid plan");
        let removed = apply_remove(&mut wad, &flats).expect("the region copy goes");
        assert_eq!(
            removed
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<usize>>(),
            vec![2_usize],
            "only the region member matched"
        );
        assert_eq!(
            names(&wad),
            vec!["FLATA", "F_START", "F_END"],
            "the loose copy survives"
        );
        let twins = pwad(vec![
            carrying("TWIN", "one"),
            carrying("TWIN", "two"),
            carrying("TWIN", "three"),
        ]);
        let infos = analyze(&twins.lumps);
        assert_eq!(
            matched_positions(&infos.lumps, name("TWIN"), &twin_plan(Some(0)))
                .expect("the first copy"),
            vec![0_usize],
            "occurrence 0 is the first match"
        );
        let past = matched_positions(&infos.lumps, name("TWIN"), &twin_plan(Some(3)))
            .expect_err("past the end");
        assert_eq!(code(&past), "not_found", "an out of range occurrence");
        assert!(
            past.to_string().contains("0 to 2"),
            "the message names the range: {past}"
        );
        assert_eq!(
            matched_positions(&infos.lumps, name("TWIN"), &twin_plan(None)).expect("the last copy"),
            vec![2_usize],
            "the default occurrence is the last match"
        );
        assert_eq!(
            matched_positions(&infos.lumps, name("ABSENT"), &twin_plan(None))
                .err()
                .map(|error| error.to_string()),
            Some(not_working_message(name("ABSENT"), None)),
            "the documented message for an absent name"
        );
    }

    #[test]
    fn remove_lump_edits_the_working_archive_once_and_rolls_a_refusal_back() {
        let mut session = loaded();
        seed(
            &mut session,
            vec![carrying("PLAYPAL", "mine"), carrying("NOTE", "n")],
        );
        let report = remove_lump(&mut session, &remove_args(&["NOTE"], None, None, false))
            .expect("the working archive holds NOTE");
        assert_eq!(
            serde_json::to_value(&report).expect("serializable"),
            json!({ "removed": [{ "name": "NOTE", "index": 1_u64 }] }),
            "the documented result shape"
        );
        assert_eq!(names(session.working()), vec!["PLAYPAL"], "one lump went");
        assert_eq!(
            session.undo_labels(),
            vec!["remove_lump NOTE", "seed"],
            "the undo label on top of the seeded lumps"
        );
        let error = remove_lump(&mut session, &remove_args(&["FLATA"], None, None, false))
            .expect_err("FLATA is only in the layer");
        assert_eq!(code(&error), "not_found", "layer lump code");
        assert_eq!(
            names(session.working()),
            vec!["PLAYPAL"],
            "nothing else went"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["remove_lump NOTE", "seed"],
            "and no history for the refusal"
        );
    }

    #[test]
    fn renaming_moves_a_working_lump_and_labels_the_edit() {
        let mut session = loaded();
        seed(&mut session, vec![carrying("OLD", "x")]);
        let report =
            rename_lump(&mut session, &rename_args("old", "NEW", false)).expect("OLD is working");
        assert_eq!(
            serde_json::to_value(&report).expect("serializable"),
            json!({ "old_name": "OLD", "new_name": "NEW", "index": 0_u64 }),
            "the documented result shape"
        );
        assert_eq!(names(session.working()), vec!["NEW"], "renamed in place");
        assert_eq!(
            session.undo_labels(),
            vec!["rename_lump OLD -> NEW", "seed"],
            "the undo label on top of the seeded lump"
        );
        let error = rename_lump(&mut session, &rename_args("FLATA", "WHATEVER", false))
            .expect_err("FLATA is only in the layer");
        assert_eq!(code(&error), "not_found", "layer lump code");
        assert!(
            error.to_string().contains("cannot be removed or renamed"),
            "the message explains the layers: {error}"
        );
    }

    #[test]
    fn renaming_onto_a_taken_name_needs_allow_duplicate() {
        let mut wad = pwad(vec![
            carrying("A", "a"),
            carrying("TAKEN", "t"),
            carrying("C", "c"),
        ]);
        let clash = apply_rename(&mut wad, 0, name("TAKEN"), false)
            .expect_err("TAKEN already stands there");
        assert_eq!(code(&clash), "already_exists", "twin code");
        assert!(
            clash.to_string().contains("allow_duplicate"),
            "the message names the fix: {clash}"
        );
        assert_eq!(
            names(&wad),
            vec!["A", "TAKEN", "C"],
            "the refusal changes nothing"
        );
        apply_rename(&mut wad, 0, name("TAKEN"), true).expect("the twin is allowed");
        assert_eq!(
            names(&wad),
            vec!["TAKEN", "TAKEN", "C"],
            "both copies stand"
        );
        let stale = apply_rename(&mut wad, 42, name("OTHER"), false).expect_err("no lump at 42");
        assert_eq!(code(&stale), "internal", "stale index code");
    }

    #[test]
    fn a_rename_onto_another_regions_name_is_no_clash() {
        let mut wad = pwad(vec![
            carrying("WALL1", "wall"),
            marker("F_START"),
            carrying("FLATA", "flat"),
            marker("F_END"),
        ]);
        apply_rename(&mut wad, 0, name("FLATA"), false).expect("a loose twin is no region clash");
        assert_eq!(
            names(&wad),
            vec!["FLATA", "F_START", "FLATA", "F_END"],
            "two FLATA copies, one loose and one in flats"
        );
        assert_eq!(
            with_payloads(&wad).first().cloned(),
            Some("FLATA(wall)".to_owned()),
            "the renamed lump kept its bytes"
        );
        let stale = apply_rename(&mut wad, 42, name("FLATA"), false).expect_err("no lump at 42");
        assert_eq!(code(&stale), "internal", "stale index code");
    }

    #[test]
    fn renaming_in_place_reports_a_stale_index() {
        let mut wad = pwad(vec![carrying("A", "a")]);
        rename_in_place(&mut wad, 0, name("B")).expect("the only lump");
        assert_eq!(names(&wad), vec!["B"], "renamed");
        let error = rename_in_place(&mut wad, 7, name("C")).expect_err("nothing at 7");
        assert_eq!(code(&error), "internal", "stale index code");
        assert!(
            error.to_string().contains("index 7"),
            "the message names the index: {error}"
        );
        assert_eq!(names(&wad), vec!["B"], "the failed call changed nothing");
    }

    #[test]
    fn rename_validates_both_names_before_editing() {
        let mut session = loaded();
        let error = rename_lump(&mut session, &rename_args("PLAYPAL", "TOO LONG", false))
            .expect_err("a bad new_name");
        assert_eq!(code(&error), "invalid_argument", "bad new name code");
        let bad_old = rename_lump(&mut session, &rename_args("TOO LONG", "OK", false))
            .expect_err("a bad name");
        assert_eq!(code(&bad_old), "invalid_argument", "bad name code");
        let region = RenameLumpParams {
            namespace: Some("nope".to_owned()),
            ..rename_args("PLAYPAL", "OK", false)
        };
        assert_eq!(
            code(&rename_lump(&mut session, &region).expect_err("not a region")),
            "invalid_argument",
            "bad namespace code"
        );
        assert!(session.undo_labels().is_empty(), "no edit was attempted");
        assert!(session.working().is_empty(), "and nothing changed");
    }
}
