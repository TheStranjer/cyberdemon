//! The value bounds and the typed accessors of each element block.
//!
//! [`bounds`] turns a [`NumRange`] into the inclusive pair one
//! [`MapFormat`] enforces: the vanilla light and angle caps in every
//! format, the binary record's `i16`/`u16`/`u8` columns on `Doom` and
//! `Hexen` maps and nothing beyond the model's own `i32` reach on `UDMF`.
//! [`wrong_family`] and [`wrong_family_error`] gate the linedef fields the
//! other flag family stores, and [`number_of`] and [`text_value`] read
//! one request value for its field. The rest are the readers and writers
//! the [`KindPlan`](super::tables::KindPlan) tables point at: one pair per
//! field type per element block, keyed by the exact table field name,
//! each answering `None` for a row or a field the block does not hold.

use alloc::format;
use alloc::string::String;

use serde_json::Value;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::model::{Linedef, Map};

use super::job::Job;
use super::{
    ANGLE_MAX, ANGLE_MIN, I16_MAX, I16_MIN, I32_MAX, I32_MIN, LIGHT_MAX, LIGHT_MIN, NumRange,
    U8_MAX, U8_MIN, U16_MAX, U16_MIN,
};

/// The inclusive bounds `range` enforces on a map stored in `format`.
pub(super) fn bounds(range: NumRange, format: MapFormat) -> (i64, i64) {
    if format == MapFormat::Udmf {
        return udmf_bounds(range);
    }
    match range {
        NumRange::Light => (LIGHT_MIN, LIGHT_MAX),
        NumRange::Angle => (ANGLE_MIN, ANGLE_MAX),
        NumRange::Small => (I16_MIN, I16_MAX),
        NumRange::Unsigned => (U16_MIN, U16_MAX),
        NumRange::LineSpecial => line_special_bounds(format),
        NumRange::Byte => (U8_MIN, U8_MAX),
    }
}

/// `UDMF` keeps only the format-independent caps; everything else lives
/// in the model's `i32` columns.
const fn udmf_bounds(range: NumRange) -> (i64, i64) {
    match range {
        NumRange::Light => (LIGHT_MIN, LIGHT_MAX),
        NumRange::Angle => (ANGLE_MIN, ANGLE_MAX),
        NumRange::Small | NumRange::Unsigned | NumRange::LineSpecial | NumRange::Byte => {
            (I32_MIN, I32_MAX)
        }
    }
}

/// A `Doom` record stores the linedef `special` as `u16`, a `Hexen` one
/// as `u8` (the `UDMF` case never reaches here).
const fn line_special_bounds(format: MapFormat) -> (i64, i64) {
    match format {
        MapFormat::Doom => (U16_MIN, U16_MAX),
        MapFormat::Hexen | MapFormat::Udmf => (U8_MIN, U8_MAX),
    }
}

/// Whether `key` names a linedef field only the other flag family stores.
pub(super) fn wrong_family(family: FlagFamily, key: &str) -> bool {
    match family {
        FlagFamily::Doom => matches!(key, "arg0" | "arg1" | "arg2" | "arg3" | "arg4"),
        FlagFamily::Hexen => key == "tag",
    }
}

/// The refusal naming the map's family, the fields it does store and the
/// tool that changes families.
pub(super) fn wrong_family_error(family: FlagFamily, key: &str) -> CyberError {
    let (wanted, stored) = match family {
        FlagFamily::Doom => ("Hexen", "special and tag"),
        FlagFamily::Hexen => ("Doom", "special, arg0, arg1, arg2, arg3 and arg4"),
    };
    CyberError::InvalidArgument(format!(
        "linedef field '{key}' belongs to the {wanted} family: this map stores the \
         {} family, whose settable linedef fields are {stored}; run convert_map_format \
         to move the map between families",
        family.as_str()
    ))
}

/// The integer a numeric field's set value carries.
pub(super) fn number_of(job: &Job<'_>, field: &str, value: &Value) -> Result<i64, CyberError> {
    value.as_i64().ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "{} field '{field}' needs an integer value, got {value}",
            job.plan.thing
        ))
    })
}

/// The uppercased name a text field's set value carries.
pub(super) fn text_value(job: &Job<'_>, field: &str, value: &Value) -> Result<String, CyberError> {
    value.as_str().map(str::to_ascii_uppercase).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "{} field '{field}' needs a name string, got {value}",
            job.plan.thing
        ))
    })
}

/// Reads the numeric sector field `field` of `map`'s row `index`.
pub(super) fn sector_num(map: &Map, index: usize, field: &str) -> Option<i32> {
    let row = map.sectors.get(index)?;
    match field {
        "ceiling" => Some(row.ceiling_height),
        "floor" => Some(row.floor_height),
        "light" => Some(row.light),
        "special" => Some(row.special),
        "tag" => Some(row.tag),
        _ => None,
    }
}

/// Reads the text sector field `field` of `map`'s row `index`.
pub(super) fn sector_text(map: &Map, index: usize, field: &str) -> Option<String> {
    let row = map.sectors.get(index)?;
    match field {
        "ceiling_flat" => Some(row.ceiling_flat.clone()),
        "floor_flat" => Some(row.floor_flat.clone()),
        _ => None,
    }
}

/// Writes one planned integer onto a sector row.
pub(super) fn write_sector_num(map: &mut Map, index: usize, field: &str, number: i32) {
    let Some(row) = map.sectors.get_mut(index) else {
        return;
    };
    match field {
        "ceiling" => row.ceiling_height = number,
        "floor" => row.floor_height = number,
        "light" => row.light = number,
        "special" => row.special = number,
        "tag" => row.tag = number,
        _ => {}
    }
}

/// Writes one planned name onto a sector row.
pub(super) fn write_sector_text(map: &mut Map, index: usize, field: &str, name: &str) {
    let Some(row) = map.sectors.get_mut(index) else {
        return;
    };
    match field {
        "ceiling_flat" => name.clone_into(&mut row.ceiling_flat),
        "floor_flat" => name.clone_into(&mut row.floor_flat),
        _ => {}
    }
}

/// Reads the numeric sidedef field `field` of `map`'s row `index`.
pub(super) fn side_num(map: &Map, index: usize, field: &str) -> Option<i32> {
    let row = map.sidedefs.get(index)?;
    match field {
        "x_offset" => Some(row.x_offset),
        "y_offset" => Some(row.y_offset),
        _ => None,
    }
}

/// Reads the text sidedef field `field` of `map`'s row `index`.
pub(super) fn side_text(map: &Map, index: usize, field: &str) -> Option<String> {
    let row = map.sidedefs.get(index)?;
    match field {
        "lower" => Some(row.lower.clone()),
        "middle" => Some(row.middle.clone()),
        "upper" => Some(row.upper.clone()),
        _ => None,
    }
}

/// Writes one planned integer onto a sidedef row.
pub(super) fn write_side_num(map: &mut Map, index: usize, field: &str, number: i32) {
    let Some(row) = map.sidedefs.get_mut(index) else {
        return;
    };
    match field {
        "x_offset" => row.x_offset = number,
        "y_offset" => row.y_offset = number,
        _ => {}
    }
}

/// Writes one planned name onto a sidedef row.
pub(super) fn write_side_text(map: &mut Map, index: usize, field: &str, name: &str) {
    let Some(row) = map.sidedefs.get_mut(index) else {
        return;
    };
    match field {
        "lower" => name.clone_into(&mut row.lower),
        "middle" => name.clone_into(&mut row.middle),
        "upper" => name.clone_into(&mut row.upper),
        _ => {}
    }
}

/// Reads the numeric vertex field `field` of `map`'s row `index`.
pub(super) fn vertex_num(map: &Map, index: usize, field: &str) -> Option<i32> {
    let row = map.vertices.get(index)?;
    match field {
        "x" => Some(row.x),
        "y" => Some(row.y),
        _ => None,
    }
}

/// Vertices store no text fields, so this reads none.
pub(super) const fn vertex_text(_map: &Map, _index: usize, _field: &str) -> Option<String> {
    None
}

/// Writes one planned integer onto a vertex row.
pub(super) fn write_vertex_num(map: &mut Map, index: usize, field: &str, number: i32) {
    let Some(row) = map.vertices.get_mut(index) else {
        return;
    };
    match field {
        "x" => row.x = number,
        "y" => row.y = number,
        _ => {}
    }
}

/// Vertices store no text fields, so this writes nothing.
pub(super) const fn write_vertex_text(_map: &mut Map, _index: usize, _field: &str, _name: &str) {}

/// Reads the numeric linedef field `field` of `map`'s row `index`.
pub(super) fn line_num(map: &Map, index: usize, field: &str) -> Option<i32> {
    let row = map.linedefs.get(index)?;
    match field {
        "special" => Some(row.special),
        "tag" => Some(row.tag),
        other => arg_slot(other).and_then(|slot| row.args.get(slot).copied()),
    }
}

/// Linedefs store no text fields, so this reads none.
pub(super) const fn line_text(_map: &Map, _index: usize, _field: &str) -> Option<String> {
    None
}

/// Writes one planned integer onto a linedef row.
pub(super) fn write_line_num(map: &mut Map, index: usize, field: &str, number: i32) {
    let Some(row) = map.linedefs.get_mut(index) else {
        return;
    };
    match field {
        "special" => row.special = number,
        "tag" => row.tag = number,
        other => write_arg(row, other, number),
    }
}

/// Linedefs store no text fields, so this writes nothing.
pub(super) const fn write_line_text(_map: &mut Map, _index: usize, _field: &str, _name: &str) {}

/// The `arg0..arg4` slot one field name reads, `None` for any other name.
fn arg_slot(field: &str) -> Option<usize> {
    match field {
        "arg0" => Some(0_usize),
        "arg1" => Some(1_usize),
        "arg2" => Some(2_usize),
        "arg3" => Some(3_usize),
        "arg4" => Some(4_usize),
        _ => None,
    }
}

/// Writes one planned integer onto the argument slot `field` names.
fn write_arg(row: &mut Linedef, field: &str, number: i32) {
    if let Some(cell) = arg_slot(field).and_then(|slot| row.args.get_mut(slot)) {
        *cell = number;
    }
}

/// Reads the numeric thing field `field` of `map`'s row `index`.
pub(super) fn thing_num(map: &Map, index: usize, field: &str) -> Option<i32> {
    let row = map.things.get(index)?;
    match field {
        "angle" => Some(row.angle),
        "type" => Some(row.kind),
        "x" => Some(row.x),
        "y" => Some(row.y),
        _ => None,
    }
}

/// Things store no text fields, so this reads none.
pub(super) const fn thing_text(_map: &Map, _index: usize, _field: &str) -> Option<String> {
    None
}

/// Writes one planned integer onto a thing row.
pub(super) fn write_thing_num(map: &mut Map, index: usize, field: &str, number: i32) {
    let Some(row) = map.things.get_mut(index) else {
        return;
    };
    match field {
        "angle" => row.angle = number,
        "type" => row.kind = number,
        "x" => row.x = number,
        "y" => row.y = number,
        _ => {}
    }
}

/// Things store no text fields, so this writes nothing.
pub(super) const fn write_thing_text(_map: &mut Map, _index: usize, _field: &str, _name: &str) {}

/// Reads the flag bits of `map`'s linedef row `index`.
pub(super) fn line_flags(map: &Map, index: usize) -> Option<u32> {
    let row = map.linedefs.get(index)?;
    Some(row.flags)
}

/// Writes planned flag bits onto `map`'s linedef row `index`.
pub(super) fn write_line_flags(map: &mut Map, index: usize, bits: u32) {
    if let Some(row) = map.linedefs.get_mut(index) {
        row.flags = bits;
    }
}

/// Reads the flag bits of `map`'s thing row `index`.
pub(super) fn thing_flags(map: &Map, index: usize) -> Option<u32> {
    let row = map.things.get(index)?;
    Some(row.flags)
}

/// Writes planned flag bits onto `map`'s thing row `index`.
pub(super) fn write_thing_flags(map: &mut Map, index: usize, bits: u32) {
    if let Some(row) = map.things.get_mut(index) {
        row.flags = bits;
    }
}

/// The kinds without a `flags` column read no bits.
pub(super) const fn no_flags(_map: &Map, _index: usize) -> Option<u32> {
    None
}

/// The kinds without a `flags` column write no bits.
pub(super) const fn write_no_flags(_map: &mut Map, _index: usize, _bits: u32) {}
