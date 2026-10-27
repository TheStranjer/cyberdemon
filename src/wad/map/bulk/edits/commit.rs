//! The request checks, row plans and report of one
//! [`apply`](super::apply) run.
//!
//! [`Job`]'s methods ask this module for the steps that read the field
//! tables and the map rows: [`check_add_fields`], [`check_overlap`] and
//! [`check_values`] refuse a request no row could satisfy, [`plan_set`]
//! and [`plan_add`] queue one row's planned numbers and names,
//! [`check_numbers`] range-checks every resulting value,
//! [`commit`] writes one validated [`Pending`] row and [`report`] folds
//! the whole selection into the [`Applied`] answer — reading each row's
//! before/after pair off the still-untouched map, so a selected element
//! the request leaves as it found it counts as matched, not changed.
//! [`find_field`] and [`names_of`] are the table lookups the refusal
//! messages read.

use alloc::string::String;
use alloc::vec::Vec;

use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::wad::map::model::Map;

use super::access::{number_of, text_value};
use super::job::{Job, Pending};
use super::{Applied, Change, FieldKind, FieldSpec};

/// Refuses every `add` name the table does not accept amounts for.
pub(super) fn check_add_fields(job: &Job<'_>) -> Result<(), CyberError> {
    for key in job.edits.add.keys() {
        let spec = job.field(key)?;
        if spec.addable {
            continue;
        }
        let addable = names_of(job.plan.fields, true);
        if addable.is_empty() {
            return Err(CyberError::InvalidArgument(format!(
                "field '{key}' is not addable on {}: {} numbers are set, never \
                 added to",
                job.kind.as_str(),
                job.plan.thing
            )));
        }
        return Err(CyberError::InvalidArgument(format!(
            "field '{key}' is not addable on {}: addable fields are {addable}",
            job.kind.as_str()
        )));
    }
    Ok(())
}

/// Refuses the same field carried by both `set` and `add`.
pub(super) fn check_overlap(job: &Job<'_>) -> Result<(), CyberError> {
    for key in job.edits.add.keys() {
        if job.edits.set.contains_key(key) {
            return Err(CyberError::InvalidArgument(format!(
                "field '{key}' carries both a set value and an add amount: pass \
                 one of them"
            )));
        }
    }
    Ok(())
}

/// Checks every value's type and every text name's stock up front.
pub(super) fn check_values(job: &Job<'_>) -> Result<(), CyberError> {
    for (key, value) in &job.edits.set {
        let spec = job.field(key)?;
        match spec.kind {
            FieldKind::Num(_) => {
                number_of(job, spec.name, value)?;
            }
            FieldKind::Text(stock) => {
                job.check_name(stock, spec.name, value)?;
            }
        }
    }
    Ok(())
}

/// Plans every `set` value onto `pending`.
pub(super) fn plan_set(job: &Job<'_>, pending: &mut Pending) -> Result<(), CyberError> {
    for (key, value) in &job.edits.set {
        let spec = job.field(key)?;
        match spec.kind {
            FieldKind::Num(_) => queue_number(job, pending, spec, value)?,
            FieldKind::Text(_) => queue_text(job, pending, spec, value)?,
        }
    }
    Ok(())
}

/// Plans one `set` integer onto `pending`.
fn queue_number(
    job: &Job<'_>,
    pending: &mut Pending,
    spec: &FieldSpec,
    value: &Value,
) -> Result<(), CyberError> {
    let planned = number_of(job, spec.name, value)?;
    pending.nums.insert(spec.name, planned);
    Ok(())
}

/// Plans one `set` name (uppercased, stock-checked) onto `pending`.
fn queue_text(
    job: &Job<'_>,
    pending: &mut Pending,
    spec: &FieldSpec,
    value: &Value,
) -> Result<(), CyberError> {
    let name = text_value(job, spec.name, value)?;
    pending.texts.insert(spec.name, name);
    Ok(())
}

/// Plans every `add` amount onto the row's current value.
pub(super) fn plan_add(job: &Job<'_>, map: &Map, pending: &mut Pending) -> Result<(), CyberError> {
    let label = job.plan.thing;
    for (key, delta) in &job.edits.add {
        let spec = job.field(key)?;
        let field = spec.name;
        let Some(current) = job.current(map, pending.index, field) else {
            return Err(job.vanished(field));
        };
        let Some(planned) = i64::from(current).checked_add(*delta) else {
            return Err(CyberError::InvalidArgument(format!(
                "{label} {field} {current} plus {delta} leaves the range of \
                 integers: pass a smaller amount or set the value"
            )));
        };
        pending.nums.insert(field, planned);
    }
    Ok(())
}

/// Refuses any numeric field of the resulting row the format cannot hold:
/// the light's `0..=255` everywhere, the binary columns' ranges, the
/// `i32` model storage on `UDMF`.
pub(super) fn check_numbers(job: &Job<'_>, map: &Map, pending: &Pending) -> Result<(), CyberError> {
    for spec in job.plan.fields {
        if let Some(value) = job.resulting(map, pending, spec.name) {
            job.check_number(spec, pending.index, value)?;
        }
    }
    Ok(())
}

/// Writes one fully validated pending row back onto the map.
///
/// The `set` and `add` field names are disjoint (`check_overlap` refuses a
/// clash), so the keyed plans' field order never decides what a column ends
/// holding.
pub(super) fn commit(job: &Job<'_>, map: &mut Map, pending: &Pending) -> Result<(), CyberError> {
    for (field, value) in &pending.nums {
        let number = i32::try_from(*value).map_err(|error| {
            CyberError::Internal(format!(
                "bulk edits: the planned value {value} for '{field}' left the column's \
                 range: {error}",
            ))
        })?;
        (job.plan.write_num)(map, pending.index, field, number);
    }
    for (field, name) in &pending.texts {
        (job.plan.write_text)(map, pending.index, field, name);
    }
    if let Some(bits) = pending.flags {
        (job.plan.write_flags)(map, pending.index, bits);
    }
    Ok(())
}

/// The [`Applied`] of a validated pass, read off the still-untouched map.
pub(super) fn report(job: &Job<'_>, map: &Map, matched: usize, planned: &[Pending]) -> Applied {
    let mut indices: Vec<usize> = Vec::new();
    let mut preview: Vec<Change> = Vec::new();
    for pending in planned {
        if let Some(change) = diffs(job, map, pending) {
            indices.push(change.index);
            preview.push(change);
        }
    }
    let changed = indices.len();
    Applied {
        matched,
        changed,
        indices,
        preview,
    }
}

/// The change row of one pending row, `None` when nothing differs.
fn diffs(job: &Job<'_>, map: &Map, pending: &Pending) -> Option<Change> {
    let mut before: Vec<(String, Value)> = Vec::new();
    let mut after: Vec<(String, Value)> = Vec::new();
    let mut changed = false;
    for spec in job.plan.fields {
        let pair = match spec.kind {
            FieldKind::Num(_) => number_pair(job, map, pending, spec.name),
            FieldKind::Text(_) => text_pair(job, map, pending, spec.name),
        };
        if let Some((old, new)) = pair {
            changed |= old != new;
            before.push((spec.name.to_owned(), old));
            after.push((spec.name.to_owned(), new));
        }
    }
    if let Some((old, new)) = flags_pair(job, map, pending) {
        changed |= old != new;
        before.push(("flags".to_owned(), old));
        after.push(("flags".to_owned(), new));
    }
    if !changed {
        return None;
    }
    Some(Change {
        index: pending.index,
        before: into_object(before),
        after: into_object(after),
    })
}

/// The before/after pair of the planned flag bits, if this row plans them.
fn flags_pair(job: &Job<'_>, map: &Map, pending: &Pending) -> Option<(Value, Value)> {
    let planned = pending.flags?;
    let current = (job.plan.get_flags)(map, pending.index)?;
    Some((
        Value::from(u64::from(current)),
        Value::from(u64::from(planned)),
    ))
}

/// The before/after pair of a planned numeric field, if this row plans one.
fn number_pair(job: &Job<'_>, map: &Map, pending: &Pending, field: &str) -> Option<(Value, Value)> {
    let planned = pending.nums.get(field).copied()?;
    let current = i64::from(job.current(map, pending.index, field)?);
    Some((Value::from(current), Value::from(planned)))
}

/// The before/after pair of a planned text field, if this row plans one.
fn text_pair(job: &Job<'_>, map: &Map, pending: &Pending, field: &str) -> Option<(Value, Value)> {
    let planned = pending.texts.get(field)?;
    let current = (job.plan.get_text)(map, pending.index, field)?;
    Some((Value::String(current), Value::String(planned.clone())))
}

/// The `(name, value)` pairs as one `JSON` object.
fn into_object(pairs: Vec<(String, Value)>) -> Value {
    let collected: JsonMap<String, Value> = pairs.into_iter().collect();
    Value::Object(collected)
}

/// The table entry named `name`, if the table carries it.
pub(super) fn find_field(fields: &'static [FieldSpec], name: &str) -> Option<&'static FieldSpec> {
    fields.iter().find(|spec| spec.name == name)
}

/// The table's field names, all or only the addable ones, comma-joined.
pub(super) fn names_of(fields: &[FieldSpec], addable_only: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for spec in fields {
        if !addable_only || spec.addable {
            parts.push(spec.name);
        }
    }
    parts.join(", ")
}
