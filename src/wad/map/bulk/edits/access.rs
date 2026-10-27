//! The value bounds and the typed accessors of each element block.
//!
//! [`bounds`] turns a [`NumRange`] into the inclusive pair one
//! [`MapFormat`] enforces: the vanilla light and angle caps in every
//! format, the binary record's `i16`/`u16`/`u8` columns on `Doom` and
//! `Hexen` maps and nothing beyond the model's own `i32` reach on `UDMF`.
//! [`wrong_family`] and [`wrong_family_error`] gate the linedef fields the
//! other flag family stores, and [`number_of`] and [`text_value`] read
//! one request value for its field. The rest are the readers and writers
//! the [`KindPlan`](super::tables::KindPlan) tables run under:
//! [`read_num`], [`read_text`], [`write_num`] and [`write_text`] dispatch
//! a [`Field`] onto its block's accessor, and each block's accessor
//! matches its own field enum arm by arm — every table-listed field
//! reaches the column it names, and no name can answer `None` or write
//! nothing without the match saying so. [`read_flags`] and
//! [`write_flags`] are the same story for the two kinds' `flags` column,
//! keyed by the plan's [`FlagKind`].

use alloc::format;
use alloc::string::String;

use serde_json::Value;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::flags::{FlagFamily, FlagKind};
use crate::wad::map::model::{Linedef, Map};

use super::fields::{Field, LinedefField, SectorField, SidedefField, ThingField, VertexField};
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

/// Whether `field` is a linedef field only the other flag family stores.
pub(super) const fn wrong_family(family: FlagFamily, field: Field) -> bool {
    match family {
        FlagFamily::Doom => matches!(
            field,
            Field::Linedef(
                LinedefField::Arg0
                    | LinedefField::Arg1
                    | LinedefField::Arg2
                    | LinedefField::Arg3
                    | LinedefField::Arg4,
            )
        ),
        FlagFamily::Hexen => matches!(field, Field::Linedef(LinedefField::Tag)),
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

/// Reads the stored integer of one numeric field, `None` for a row past
/// the block and for a field the block holds as a name.
pub(super) fn read_num(map: &Map, index: usize, key: Field) -> Option<i32> {
    match key {
        Field::Sector(field) => sector_num(map, index, field),
        Field::Sidedef(field) => side_num(map, index, field),
        Field::Vertex(field) => vertex_num(map, index, field),
        Field::Linedef(field) => line_num(map, index, field),
        Field::Thing(field) => thing_num(map, index, field),
    }
}

/// Writes one planned integer onto the field's column.
pub(super) fn write_num(map: &mut Map, index: usize, key: Field, number: i32) {
    match key {
        Field::Sector(field) => write_sector_num(map, index, field, number),
        Field::Sidedef(field) => write_side_num(map, index, field, number),
        Field::Vertex(field) => write_vertex_num(map, index, field, number),
        Field::Linedef(field) => write_line_num(map, index, field, number),
        Field::Thing(field) => write_thing_num(map, index, field, number),
    }
}

/// Reads the stored name of one text field, `None` for a row past the
/// block and for every field of a block that stores no names.
pub(super) fn read_text(map: &Map, index: usize, key: Field) -> Option<String> {
    match key {
        Field::Sector(field) => sector_text(map, index, field),
        Field::Sidedef(field) => side_text(map, index, field),
        Field::Vertex(..) | Field::Linedef(..) | Field::Thing(..) => None,
    }
}

/// Writes one planned name onto the field's column.
pub(super) fn write_text(map: &mut Map, index: usize, key: Field, name: &str) {
    match key {
        Field::Sector(field) => write_sector_text(map, index, field, name),
        Field::Sidedef(field) => write_side_text(map, index, field, name),
        Field::Vertex(..) | Field::Linedef(..) | Field::Thing(..) => {}
    }
}

/// Reads the flag bits of the row the plan's flag kind points at.
pub(super) fn read_flags(kind: FlagKind, map: &Map, index: usize) -> Option<u32> {
    match kind {
        FlagKind::Linedef => map.linedefs.get(index).map(|row| row.flags),
        FlagKind::Thing => map.things.get(index).map(|row| row.flags),
    }
}

/// Writes planned flag bits onto the row the flag kind points at.
pub(super) fn write_flags(kind: FlagKind, map: &mut Map, index: usize, bits: u32) {
    match kind {
        FlagKind::Linedef => {
            if let Some(row) = map.linedefs.get_mut(index) {
                row.flags = bits;
            }
        }
        FlagKind::Thing => {
            if let Some(row) = map.things.get_mut(index) {
                row.flags = bits;
            }
        }
    }
}

/// Reads one sector row's numeric field.
fn sector_num(map: &Map, index: usize, field: SectorField) -> Option<i32> {
    let row = map.sectors.get(index)?;
    match field {
        SectorField::Ceiling => Some(row.ceiling_height),
        SectorField::CeilingFlat | SectorField::FloorFlat => None,
        SectorField::Floor => Some(row.floor_height),
        SectorField::Light => Some(row.light),
        SectorField::Special => Some(row.special),
        SectorField::Tag => Some(row.tag),
    }
}

/// Writes one planned integer onto a sector row.
fn write_sector_num(map: &mut Map, index: usize, field: SectorField, number: i32) {
    let Some(row) = map.sectors.get_mut(index) else {
        return;
    };
    match field {
        SectorField::Ceiling => row.ceiling_height = number,
        SectorField::CeilingFlat | SectorField::FloorFlat => {}
        SectorField::Floor => row.floor_height = number,
        SectorField::Light => row.light = number,
        SectorField::Special => row.special = number,
        SectorField::Tag => row.tag = number,
    }
}

/// Reads one sector row's text field.
fn sector_text(map: &Map, index: usize, field: SectorField) -> Option<String> {
    let row = map.sectors.get(index)?;
    match field {
        SectorField::Ceiling
        | SectorField::Floor
        | SectorField::Light
        | SectorField::Special
        | SectorField::Tag => None,
        SectorField::CeilingFlat => Some(row.ceiling_flat.clone()),
        SectorField::FloorFlat => Some(row.floor_flat.clone()),
    }
}

/// Writes one planned name onto a sector row.
fn write_sector_text(map: &mut Map, index: usize, field: SectorField, name: &str) {
    let Some(row) = map.sectors.get_mut(index) else {
        return;
    };
    match field {
        SectorField::Ceiling
        | SectorField::Floor
        | SectorField::Light
        | SectorField::Special
        | SectorField::Tag => {}
        SectorField::CeilingFlat => name.clone_into(&mut row.ceiling_flat),
        SectorField::FloorFlat => name.clone_into(&mut row.floor_flat),
    }
}

/// Reads one sidedef row's numeric field.
fn side_num(map: &Map, index: usize, field: SidedefField) -> Option<i32> {
    let row = map.sidedefs.get(index)?;
    match field {
        SidedefField::Lower | SidedefField::Middle | SidedefField::Upper => None,
        SidedefField::XOffset => Some(row.x_offset),
        SidedefField::YOffset => Some(row.y_offset),
    }
}

/// Writes one planned integer onto a sidedef row.
fn write_side_num(map: &mut Map, index: usize, field: SidedefField, number: i32) {
    let Some(row) = map.sidedefs.get_mut(index) else {
        return;
    };
    match field {
        SidedefField::Lower | SidedefField::Middle | SidedefField::Upper => {}
        SidedefField::XOffset => row.x_offset = number,
        SidedefField::YOffset => row.y_offset = number,
    }
}

/// Reads one sidedef row's texture face.
fn side_text(map: &Map, index: usize, field: SidedefField) -> Option<String> {
    let row = map.sidedefs.get(index)?;
    match field {
        SidedefField::Lower => Some(row.lower.clone()),
        SidedefField::Middle => Some(row.middle.clone()),
        SidedefField::Upper => Some(row.upper.clone()),
        SidedefField::XOffset | SidedefField::YOffset => None,
    }
}

/// Writes one planned name onto a sidedef row's texture face.
fn write_side_text(map: &mut Map, index: usize, field: SidedefField, name: &str) {
    let Some(row) = map.sidedefs.get_mut(index) else {
        return;
    };
    match field {
        SidedefField::Lower => name.clone_into(&mut row.lower),
        SidedefField::Middle => name.clone_into(&mut row.middle),
        SidedefField::Upper => name.clone_into(&mut row.upper),
        SidedefField::XOffset | SidedefField::YOffset => {}
    }
}

/// Reads one vertex row's coordinate.
fn vertex_num(map: &Map, index: usize, field: VertexField) -> Option<i32> {
    let row = map.vertices.get(index)?;
    match field {
        VertexField::X => Some(row.x),
        VertexField::Y => Some(row.y),
    }
}

/// Writes one planned integer onto a vertex row.
fn write_vertex_num(map: &mut Map, index: usize, field: VertexField, number: i32) {
    let Some(row) = map.vertices.get_mut(index) else {
        return;
    };
    match field {
        VertexField::X => row.x = number,
        VertexField::Y => row.y = number,
    }
}

/// Reads one linedef row's action, tag or script argument.
fn line_num(map: &Map, index: usize, field: LinedefField) -> Option<i32> {
    let row = map.linedefs.get(index)?;
    match field {
        LinedefField::Arg0 => arg_of(row, 0_usize),
        LinedefField::Arg1 => arg_of(row, 1_usize),
        LinedefField::Arg2 => arg_of(row, 2_usize),
        LinedefField::Arg3 => arg_of(row, 3_usize),
        LinedefField::Arg4 => arg_of(row, 4_usize),
        LinedefField::Special => Some(row.special),
        LinedefField::Tag => Some(row.tag),
    }
}

/// Writes one planned integer onto a linedef row.
fn write_line_num(map: &mut Map, index: usize, field: LinedefField, number: i32) {
    let Some(row) = map.linedefs.get_mut(index) else {
        return;
    };
    match field {
        LinedefField::Arg0 => put_arg(row, 0_usize, number),
        LinedefField::Arg1 => put_arg(row, 1_usize, number),
        LinedefField::Arg2 => put_arg(row, 2_usize, number),
        LinedefField::Arg3 => put_arg(row, 3_usize, number),
        LinedefField::Arg4 => put_arg(row, 4_usize, number),
        LinedefField::Special => row.special = number,
        LinedefField::Tag => row.tag = number,
    }
}

/// The `args` cell at `slot`, `None` past the five.
fn arg_of(row: &Linedef, slot: usize) -> Option<i32> {
    row.args.get(slot).copied()
}

/// Writes one planned integer onto the `args` cell at `slot`.
fn put_arg(row: &mut Linedef, slot: usize, number: i32) {
    if let Some(cell) = row.args.get_mut(slot) {
        *cell = number;
    }
}

/// Reads one thing row's doomednum, facing or position.
fn thing_num(map: &Map, index: usize, field: ThingField) -> Option<i32> {
    let row = map.things.get(index)?;
    match field {
        ThingField::Angle => Some(row.angle),
        ThingField::Type => Some(row.kind),
        ThingField::X => Some(row.x),
        ThingField::Y => Some(row.y),
    }
}

/// Writes one planned integer onto a thing row.
fn write_thing_num(map: &mut Map, index: usize, field: ThingField, number: i32) {
    let Some(row) = map.things.get_mut(index) else {
        return;
    };
    match field {
        ThingField::Angle => row.angle = number,
        ThingField::Type => row.kind = number,
        ThingField::X => row.x = number,
        ThingField::Y => row.y = number,
    }
}
