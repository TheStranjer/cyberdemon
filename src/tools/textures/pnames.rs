//! The planning half of `edit_pnames`: the wire arguments, the operation
//! they carry and the [`Plan`] computed before anything is written.
//!
//! [`plan_edit`] runs the matching planner - [`plan_add`], [`plan_remove`],
//! [`plan_rename`] or [`plan_replace_all`] - over the current `PNAMES` list
//! and then [`check_dropped`] over the [`name_mapping`] renumbering it
//! derived, so a call that would silently drop a texture's patch reference
//! fails with a `conflict` naming the textures. [`EditReport`] is what the
//! finished plan answers with.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::wad::name::LumpName;
use crate::wad::textures::{PatchMapping, TextureLump, dropped_references};

/// One `from`/`to` pair of [`EditPnamesParams::rename`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RenamePair {
    /// The current `PNAMES` entry to change: 1 to 8 printable `ASCII`
    /// characters, matched case-insensitively.
    pub(super) from: String,
    /// Its new name; must not already be in `PNAMES` (indices stay put, so
    /// texture references are untouched).
    pub(super) to: String,
}

/// Arguments for [`Cyberdemon::edit_pnames`]: exactly one operation.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct EditPnamesParams {
    /// Patch names to append. Names already present are skipped and
    /// reported in `skipped`.
    #[serde(default)]
    pub(super) add: Option<Vec<String>>,
    /// Patch names to remove. Without `force`, removing a name a texture
    /// still refers to fails with `conflict`; `force: true` drops those
    /// patch references from the textures.
    #[serde(default)]
    pub(super) remove: Option<Vec<String>>,
    /// Entries to rename in place: `[{from, to}]`; indices stay put.
    #[serde(default)]
    pub(super) rename: Option<Vec<RenamePair>>,
    /// The complete new list, replacing every entry. Indexing is by name,
    /// so surviving entries are renumbered in the textures and missing
    /// ones follow the same `conflict`/`force` rule as `remove`; an empty
    /// list clears `PNAMES`.
    #[serde(default)]
    pub(super) replace_all: Option<Vec<String>>,
    /// Proceed despite dropped references (default `false`). Applies to
    /// `remove` and `replace_all`.
    #[serde(default)]
    pub(super) force: Option<bool>,
}

/// The one operation an `edit_pnames` call carries.
#[derive(Clone, Copy, Debug)]
pub(super) enum Operation<'op> {
    /// Append names.
    Add(&'op [String]),
    /// Remove names.
    Remove(&'op [String]),
    /// Rename entries in place.
    Rename(&'op [RenamePair]),
    /// Replace the whole list.
    ReplaceAll(&'op [String]),
}

impl Operation<'_> {
    /// The operation's name, used in the undo label.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Add(_) => "add",
            Self::Remove(_) => "remove",
            Self::Rename(_) => "rename",
            Self::ReplaceAll(_) => "replace_all",
        }
    }
}

/// One renamed pair in [`EditReport::renamed`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(super) struct RenameRow {
    /// The old entry name.
    pub(super) from: String,
    /// The new entry name.
    pub(super) to: String,
}

/// Structured result of [`Cyberdemon::edit_pnames`].
#[derive(Debug, Serialize)]
pub(super) struct EditReport {
    /// `PNAMES` entries after the edit.
    pub(super) pnames: usize,
    /// Names appended by `add` or brought in by `replace_all`.
    pub(super) added: Vec<String>,
    /// Requested names that needed no change (already present for `add`,
    /// absent for `remove`).
    pub(super) skipped: Vec<String>,
    /// Names `remove` deleted or `replace_all` dropped.
    pub(super) removed: Vec<String>,
    /// Pairs `rename` applied.
    pub(super) renamed: Vec<RenameRow>,
    /// Textures whose patch references were renumbered or pruned by the
    /// edit (always 0 for `add` and `rename`).
    pub(super) textures_changed: usize,
}

/// The whole change one `edit_pnames` call wants, before anything is
/// written.
#[derive(Default)]
pub(super) struct Plan {
    /// The new complete `PNAMES` list.
    pub(super) names: Vec<String>,
    /// `mapping[old]` renumbers texture references; `None` when no index
    /// moves.
    pub(super) mapping: Option<PatchMapping>,
    /// Names appended.
    pub(super) added: Vec<String>,
    /// Requested names that needed no change.
    pub(super) skipped: Vec<String>,
    /// Names deleted.
    pub(super) removed: Vec<String>,
    /// Pairs applied by `rename`.
    pub(super) renamed: Vec<RenameRow>,
}

/// Computes the plan for one operation: the new list, the renumbering
/// mapping and the report's name lists.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad name, an empty `add`/`remove`/
/// `rename` list or a `replace_all` list with duplicates;
/// [`CyberError::NotFound`] for a `rename` whose `from` is absent;
/// [`CyberError::Conflict`] when entries a texture still refers to would
/// vanish without `force`; [`CyberError::LimitExceeded`] over the `u16`
/// index range.
pub(super) fn plan_edit(
    current: &[String],
    operation: Operation<'_>,
    force: bool,
    textures: &[(&TextureLump, &'static str)],
) -> Result<Plan, CyberError> {
    let plan = match operation {
        Operation::Add(list) => plan_add(current, list)?,
        Operation::Remove(list) => plan_remove(current, list)?,
        Operation::Rename(pairs) => plan_rename(current, pairs)?,
        Operation::ReplaceAll(list) => plan_replace_all(current, list)?,
    };
    if let Some(mapping) = plan.mapping.as_ref() {
        check_dropped(mapping, force, textures)?;
    }
    Ok(plan)
}

/// Parses `list` into uppercased names for `operation`, refusing empties.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty list or an unparsable name.
fn name_list(operation: &str, list: &[String]) -> Result<Vec<String>, CyberError> {
    if list.is_empty() {
        return Err(CyberError::InvalidArgument(format!(
            "{operation} needs at least one name: pass a non-empty list"
        )));
    }
    list.iter().map(|text| parsed_name(text)).collect()
}

/// Uppercases one name through [`LumpName::parse`], echoing it in errors.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected text.
fn parsed_name(text: &str) -> Result<String, CyberError> {
    LumpName::parse(text)
        .map(|name| name.to_string())
        .map_err(|error| {
            CyberError::InvalidArgument(format!("PNAMES name {text:?} is not valid: {error}"))
        })
}

/// The position of `name` in `list`, matched case-insensitively.
pub(super) fn position_of(list: &[String], name: &str) -> Option<usize> {
    list.iter()
        .position(|entry| entry.eq_ignore_ascii_case(name))
}

/// `add`: append the names not present yet.
fn plan_add(current: &[String], list: &[String]) -> Result<Plan, CyberError> {
    let wanted = name_list("add", list)?;
    let mut names = current.to_vec();
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    for name in wanted {
        if position_of(&names, &name).is_some() {
            skipped.push(name);
        } else {
            names.push(name.clone());
            added.push(name);
        }
    }
    Ok(Plan {
        names,
        added,
        skipped,
        ..Plan::default()
    })
}

/// `remove`: drop the named entries; the mapping renumbers what is left.
fn plan_remove(current: &[String], list: &[String]) -> Result<Plan, CyberError> {
    let wanted = name_list("remove", list)?;
    let names: Vec<String> = current
        .iter()
        .filter(|entry| !wanted.iter().any(|name| name == *entry))
        .cloned()
        .collect();
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    for name in wanted {
        let present = position_of(current, &name).is_some();
        if present && !removed.contains(&name) {
            removed.push(name);
        } else if !present && !skipped.contains(&name) {
            skipped.push(name);
        } else {
            // A repeated request for the same outcome: report it once.
        }
    }
    Ok(Plan {
        mapping: name_mapping(current, &names)?,
        names,
        removed,
        skipped,
        ..Plan::default()
    })
}

/// `rename`: change entries in place; indices (and textures) do not move.
fn plan_rename(current: &[String], pairs: &[RenamePair]) -> Result<Plan, CyberError> {
    if pairs.is_empty() {
        return Err(CyberError::InvalidArgument(
            "rename needs at least one {from, to} pair".to_owned(),
        ));
    }
    let mut names = current.to_vec();
    let mut renamed = Vec::new();
    for pair in pairs {
        let from = parsed_name(&pair.from)?;
        let to = parsed_name(&pair.to)?;
        let at = position_of(&names, &from).ok_or_else(|| {
            CyberError::NotFound(format!(
                "PNAMES holds no entry {from}: list_pnames shows the current names"
            ))
        })?;
        if to != from && position_of(&names, &to).is_some() {
            return Err(CyberError::Conflict(format!(
                "PNAMES already holds {to}: rename to a free name (a later pair may free one)"
            )));
        }
        if let Some(slot) = names.get_mut(at) {
            slot.clone_from(&to);
        }
        renamed.push(RenameRow { from, to });
    }
    Ok(Plan {
        names,
        renamed,
        ..Plan::default()
    })
}

/// `replace_all`: the new complete list; indexing is by name, so the
/// mapping renumbers every surviving reference.
fn plan_replace_all(current: &[String], list: &[String]) -> Result<Plan, CyberError> {
    let names: Vec<String> = list
        .iter()
        .map(|text| parsed_name(text))
        .collect::<Result<_, _>>()?;
    let mut sorted = names.clone();
    sorted.sort_unstable();
    let duplicated = sorted
        .windows(2)
        .find(|pair| {
            pair.first()
                .zip(pair.get(1))
                .is_some_and(|(left, right)| left == right)
        })
        .and_then(|pair| pair.first().cloned());
    if let Some(dup) = duplicated {
        return Err(CyberError::InvalidArgument(format!(
            "replace_all lists {dup} twice: every PNAMES name must be unique"
        )));
    }
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for entry in current {
        if position_of(&names, entry).is_none() {
            removed.push(entry.clone());
        }
    }
    for name in &names {
        if position_of(current, name).is_none() {
            added.push(name.clone());
        }
    }
    Ok(Plan {
        mapping: name_mapping(current, &names)?,
        names,
        added,
        removed,
        ..Plan::default()
    })
}

/// The name-based `old -> new` mapping of two lists, `None` when no index
/// moves.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a surviving name lands past the
/// `u16` range a patch reference can store.
pub(super) fn name_mapping(
    current: &[String],
    names: &[String],
) -> Result<Option<PatchMapping>, CyberError> {
    let mut mapping = Vec::with_capacity(current.len());
    let mut identity = true;
    for (old, entry) in current.iter().enumerate() {
        let new = position_of(names, entry).map(usize_to_u16).transpose()?;
        let old_field = u16::try_from(old).ok();
        if new != old_field {
            identity = false;
        }
        mapping.push(new);
    }
    Ok((!identity).then_some(mapping))
}

/// The `u16` form of a new index position.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] above `u16::MAX`.
pub(super) fn usize_to_u16(value: usize) -> Result<u16, CyberError> {
    u16::try_from(value).map_err(|error| {
        CyberError::LimitExceeded(format!(
            "the new PNAMES puts an entry at index {value}, past the u16 range textures store: \
             {error}"
        ))
    })
}

/// Refuses a plan whose mapping drops texture patch references, unless
/// `force` was given.
///
/// # Errors
///
/// [`CyberError::Conflict`] listing the affected textures.
fn check_dropped(
    mapping: &PatchMapping,
    force: bool,
    textures: &[(&TextureLump, &'static str)],
) -> Result<(), CyberError> {
    if force {
        return Ok(());
    }
    let offenders: Vec<String> = textures
        .iter()
        .flat_map(|&(lump, label)| {
            dropped_references(lump, mapping)
                .into_iter()
                .map(move |name| format!("{name} ({label})"))
        })
        .collect();
    if offenders.is_empty() {
        return Ok(());
    }
    Err(CyberError::Conflict(format!(
        "textures still refer to PNAMES names this edit removes: {}; pass force=true to drop \
         those patch references",
        offenders.join(", ")
    )))
}
