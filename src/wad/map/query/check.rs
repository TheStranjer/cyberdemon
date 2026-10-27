//! The checks and the comparisons: validating one filter against the field
//! table, then running a validated filter against one element's value.
//!
//! [`prepare_filter`] turns a [`Filter`] into the [`Prepared`] row the engine
//! keeps: [`field_spec`]'s row for the name it spells, [`check_op`] against
//! the ops that field's [`FieldType`] takes and [`check_value`] against the
//! shape the op needs — [`check_scalar`] and [`check_list`] for the direct
//! and `in`/`not_in` comparisons, [`check_glob`] for a pattern and
//! [`check_name`] for the flag and role names, which end in [`check_flag`]
//! and [`check_role`] reading the tables that own them. Every refusal is a
//! [`CyberError::InvalidArgument`] naming what is valid, worded by
//! [`type_word`], [`op_name`] and [`ops_word`], or by [`unknown_field`]
//! listing the kind's whole field table.
//!
//! [`compare`] runs one checked filter: the [`FValue`] shape picks the
//! family — [`compare_num`], [`compare_text`], [`compare_bool`] or
//! [`compare_names`] over a flag bitset or a role list — and [`array_of`]
//! reads an `in`/`not_in` value.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde_json::Value;

use crate::error::CyberError;
use crate::wad::map::describe::ROLE_NAMES;
use crate::wad::map::flags::{FlagFamily, FlagKind, check_flag, to_names};
use crate::wad::name::glob_match;

use super::engine::Prepared;
use super::fields::{FValue, FieldSpec, FieldType, field_spec, valid_fields};
use super::{ElementKind, Filter, Op};

/// Check one filter against the field table: a known field, an op its type
/// takes, and a value of the shape that op needs.
pub(super) fn prepare_filter(
    kind: ElementKind,
    family: FlagFamily,
    filter: &Filter,
) -> Result<Prepared<'_>, CyberError> {
    let spec = field_spec(kind, family, &filter.field)
        .ok_or_else(|| unknown_field(kind, family, filter))?;
    check_op(kind, spec, filter)?;
    check_value(family, spec, filter)?;
    Ok(Prepared {
        spec,
        op: filter.op,
        want: &filter.value,
    })
}

/// The `invalid_argument` for a field the kind does not carry.
fn unknown_field(kind: ElementKind, family: FlagFamily, filter: &Filter) -> CyberError {
    CyberError::InvalidArgument(format!(
        "unknown field '{}' for {}: valid fields are {}",
        filter.field,
        kind.as_str(),
        valid_fields(kind, family)
    ))
}

/// The word naming one type's comparison values.
const fn type_word(ty: FieldType) -> &'static str {
    match ty {
        FieldType::Number => "a number",
        FieldType::Text => "a string",
        FieldType::Boolean => "a boolean",
        FieldType::LineFlags | FieldType::ThingFlags | FieldType::Roles => "a name",
    }
}

/// The op list a field type accepts, comma-joined for error messages.
fn ops_word(ty: FieldType) -> String {
    [
        Op::Eq,
        Op::Ne,
        Op::Lt,
        Op::Le,
        Op::Gt,
        Op::Ge,
        Op::In,
        Op::NotIn,
        Op::Glob,
        Op::Has,
        Op::HasNot,
    ]
    .into_iter()
    .filter(|op| op_ok(*op, ty))
    .map(op_name)
    .collect::<Vec<&str>>()
    .join(", ")
}

/// The wire name of one op.
const fn op_name(op: Op) -> &'static str {
    match op {
        Op::Eq => "eq",
        Op::Ne => "ne",
        Op::Lt => "lt",
        Op::Le => "le",
        Op::Gt => "gt",
        Op::Ge => "ge",
        Op::In => "in",
        Op::NotIn => "not_in",
        Op::Glob => "glob",
        Op::Has => "has",
        Op::HasNot => "has_not",
    }
}

/// Whether `op` applies to a field of type `ty`.
const fn op_ok(op: Op, ty: FieldType) -> bool {
    match ty {
        FieldType::Number => !matches!(op, Op::Glob | Op::Has | Op::HasNot),
        FieldType::Text => !matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Has | Op::HasNot),
        FieldType::Boolean => matches!(op, Op::Eq | Op::Ne | Op::In | Op::NotIn),
        FieldType::LineFlags | FieldType::ThingFlags | FieldType::Roles => {
            matches!(op, Op::Has | Op::HasNot)
        }
    }
}

/// Refuse an op the field's type does not take.
fn check_op(kind: ElementKind, spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    if op_ok(filter.op, spec.ty) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "op '{}' does not apply to {} field '{}': {} fields take {}",
        op_name(filter.op),
        kind.as_str(),
        spec.name,
        type_word(spec.ty),
        ops_word(spec.ty),
    )))
}

/// Check the comparison value's shape against the field's type and op.
fn check_value(family: FlagFamily, spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    match filter.op {
        Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => check_scalar(spec, filter),
        Op::In | Op::NotIn => check_list(spec, filter),
        Op::Glob => check_glob(spec, filter),
        Op::Has | Op::HasNot => check_name(family, spec, filter),
    }
}

/// The scalar shape one type's direct comparisons need.
fn type_matches(ty: FieldType, want: &Value) -> bool {
    match ty {
        FieldType::Number => want.is_i64(),
        FieldType::Text => want.is_string(),
        FieldType::Boolean => want.is_boolean(),
        FieldType::LineFlags | FieldType::ThingFlags | FieldType::Roles => false,
    }
}

/// Refuse a scalar comparison of the wrong shape.
fn check_scalar(spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    if type_matches(spec.ty, &filter.value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{} of field '{}' needs {}; got {}",
        op_name(filter.op),
        spec.name,
        type_word(spec.ty),
        filter.value,
    )))
}

/// Refuse an `in`/`not_in` whose array holds a wrongly shaped value.
fn check_list(spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    let Some(items) = filter.value.as_array() else {
        return Err(CyberError::InvalidArgument(format!(
            "{} of field '{}' needs an array of {}; got {}",
            op_name(filter.op),
            spec.name,
            type_word(spec.ty),
            filter.value,
        )));
    };
    for item in items {
        if !type_matches(spec.ty, item) {
            return Err(CyberError::InvalidArgument(format!(
                "{} of field '{}' needs an array of {}; {item} does not fit",
                op_name(filter.op),
                spec.name,
                type_word(spec.ty),
            )));
        }
    }
    Ok(())
}

/// Refuse a `glob` whose pattern is not a string.
fn check_glob(spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    if filter.value.is_string() {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "glob of field '{}' needs a string pattern; got {}",
        spec.name, filter.value,
    )))
}

/// Refuse a `has`/`has_not` whose name is unknown or wrongly shaped.
fn check_name(family: FlagFamily, spec: &FieldSpec, filter: &Filter) -> Result<(), CyberError> {
    let Some(wanted) = filter.value.as_str() else {
        return Err(CyberError::InvalidArgument(format!(
            "{} of field '{}' needs {}; got {}",
            op_name(filter.op),
            spec.name,
            type_word(spec.ty),
            filter.value,
        )));
    };
    match spec.ty {
        FieldType::LineFlags => check_flag(FlagKind::Linedef, family, wanted),
        FieldType::ThingFlags => check_flag(FlagKind::Thing, family, wanted),
        FieldType::Roles => check_role(spec, wanted),
        FieldType::Number | FieldType::Text | FieldType::Boolean => Ok(()),
    }
}

/// Refuse a role name the describe classification never answers with.
fn check_role(spec: &FieldSpec, wanted: &str) -> Result<(), CyberError> {
    if ROLE_NAMES.contains(&wanted) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "unknown sector role '{wanted}' for field '{}': valid roles are {}",
        spec.name,
        ROLE_NAMES.join(", "),
    )))
}

/// Run one checked comparison against one element's field value: the
/// value's shape picks the comparison family, exactly as `prepare_filter`
/// promised when it checked the op against the field type.
pub(super) fn compare(op: Op, family: FlagFamily, value: FValue<'_>, want: &Value) -> bool {
    match value {
        FValue::Num(stored) => compare_num(op, want, stored),
        FValue::Text(text) => compare_text(op, want, text.as_ref()),
        FValue::Bool(flag) => compare_bool(op, want, flag),
        FValue::Bits(bits, kind) => compare_names(op, want, &to_names(bits, kind, family)),
        FValue::Roles(roles) => {
            let names: Vec<&str> = roles.iter().map(String::as_str).collect();
            compare_names(op, want, &names)
        }
    }
}

/// Compare a numeric field against its value.
fn compare_num(op: Op, want: &Value, stored: i64) -> bool {
    match op {
        Op::Eq => want.as_i64() == Some(stored),
        Op::Ne => want.as_i64() != Some(stored),
        Op::In | Op::NotIn => {
            let in_list = array_of(want)
                .is_some_and(|items| items.iter().any(|item| item.as_i64() == Some(stored)));
            in_list == (op == Op::In)
        }
        Op::Lt => want.as_i64().is_some_and(|wanted| stored < wanted),
        Op::Le => want.as_i64().is_some_and(|wanted| stored <= wanted),
        Op::Gt => want.as_i64().is_some_and(|wanted| stored > wanted),
        Op::Ge => want.as_i64().is_some_and(|wanted| stored >= wanted),
        Op::Glob | Op::Has | Op::HasNot => false,
    }
}

/// Compare a text field against its value.
fn compare_text(op: Op, want: &Value, stored: &str) -> bool {
    match op {
        Op::Eq => want.as_str() == Some(stored),
        Op::Ne => want.as_str() != Some(stored),
        Op::In | Op::NotIn => {
            let in_list = array_of(want)
                .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(stored)));
            in_list == (op == Op::In)
        }
        Op::Glob => want
            .as_str()
            .is_some_and(|pattern| glob_match(stored.as_bytes(), pattern)),
        Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Has | Op::HasNot => false,
    }
}

/// Compare the `two_sided` flag against its boolean.
fn compare_bool(op: Op, want: &Value, stored: bool) -> bool {
    match op {
        Op::Eq => want.as_bool() == Some(stored),
        Op::Ne => want.as_bool() != Some(stored),
        Op::In | Op::NotIn => {
            let in_list = array_of(want)
                .is_some_and(|items| items.iter().any(|item| item.as_bool() == Some(stored)));
            in_list == (op == Op::In)
        }
        Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Glob | Op::Has | Op::HasNot => false,
    }
}

/// Compare a flag bitset or role list against one named entry.
fn compare_names(op: Op, want: &Value, names: &[&str]) -> bool {
    let found = want.as_str().is_some_and(|wanted| names.contains(&wanted));
    found == (op == Op::Has)
}

/// The array of an `in`/`not_in` comparison value.
fn array_of(want: &Value) -> Option<&[Value]> {
    want.as_array().map(Vec::as_slice)
}
