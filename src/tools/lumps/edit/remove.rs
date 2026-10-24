//! `remove_lump`: deleting named lumps from the working archive.
//!
//! [`plan_remove`] validates the arguments into a [`RemovePlan`],
//! [`apply_remove`] resolves every name against the working directory — a
//! name that is not there aborts the whole edit, so nothing goes — and
//! [`remove_lump`] commits the selected positions as one
//! [`crate::session::Session::edit`] labelled with the names in the order
//! they were given. The read-only layers are never touched: their lumps can
//! only be overridden with `replace_lump`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::in_region;
use super::replace::not_working_message;
use crate::error::CyberError;
use crate::session::Session;
use crate::tools::lumps::parse_namespace;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{LumpInfo, Namespace, analyze};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::remove_lump`](crate::server::Cyberdemon).
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools::lumps) struct RemoveLumpParams {
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

/// A validated `remove_lump` call: the names and how many copies of each go.
#[derive(Debug)]
pub(super) struct RemovePlan {
    /// The names to remove, in the order given.
    pub(super) names: Vec<LumpName>,
    /// Only matches inside this region count.
    pub(super) namespace: Option<Namespace>,
    /// The occurrence to take; only ever set for a single name.
    pub(super) occurrence: Option<usize>,
    /// Whether every match of each name goes.
    pub(super) all: bool,
}

/// Validate `remove_lump`'s arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty `names` list, a name that
/// does not parse, an unknown `namespace`, or `occurrence` beside more than
/// one name.
pub(super) fn plan_remove(args: &RemoveLumpParams) -> Result<RemovePlan, CyberError> {
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
        namespace: parse_namespace(args.namespace.as_deref())?,
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
pub(super) fn allowed_occurrence(
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
pub(super) struct RemovedLump {
    /// The lump's name.
    pub(super) name: LumpName,
    /// Its position in the working archive before it was removed.
    pub(super) index: usize,
}

/// Structured result of [`Cyberdemon::remove_lump`](crate::server::Cyberdemon).
#[derive(Debug, Serialize)]
pub(in crate::tools::lumps) struct RemoveReport {
    /// What went, in directory order.
    removed: Vec<RemovedLump>,
}

/// The working-archive positions of `name` inside the plan's region.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the working archive holds no such lump, or
/// when `occurrence` reaches past its matches.
pub(super) fn matched_positions(
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
pub(super) fn apply_remove(
    wad: &mut Wad,
    plan: &RemovePlan,
) -> Result<Vec<RemovedLump>, CyberError> {
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
pub(super) fn remove_label(names: &[LumpName]) -> String {
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
pub(in crate::tools::lumps) fn remove_lump(
    session: &mut Session,
    args: &RemoveLumpParams,
) -> Result<RemoveReport, CyberError> {
    let plan = plan_remove(args)?;
    let label = remove_label(&plan.names);
    let removed = session.edit(&label, |wad| apply_remove(wad, &plan))?;
    Ok(RemoveReport { removed })
}
