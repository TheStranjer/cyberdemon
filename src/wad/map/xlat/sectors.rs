//! The [`Sector`](crate::wad::map::Sector) special stage of the
//! cross-family `Doom` ↔ `Hexen` conversion: the `Doom` Boom-style bit
//! layout and its `Hexen` generalized twin, the field folds
//! [`hexen_sector_bits`] and [`boom_sector_bits`] between them, and the two
//! in-place stages [`sectors_to_hexen`] and [`sectors_from_hexen`].
//!
//! Each stage answers with a [`FieldReport`](super::FieldReport) of the
//! sectors whose `special` moved, and clears any value the fold cannot read
//! to 0, naming every such sector in one grouped [`cleared_sectors`]
//! warning. [`Sector::tag`](crate::wad::map::Sector::tag) is never touched.

use crate::wad::map::model::{Map, Sector};

use super::{FIELD_ONE, FIELD_TWO, FieldReport, count_if, move_field, show_indices};

/// The highest Boom-style sector special: the `Doom` family packs its
/// sector effects into bits 0 to 9.
const SECTOR_BOOM_MAX: u32 = 0x3FF;
/// The five-bit field of a Boom-style sector's basic type.
const SECTOR_TYPE_MASK: u32 = 0x1F;
/// The `Doom` basic sector type that marks the sector secret.
const SECTOR_SECRET_TYPE: u32 = 9_u32;
/// The offset a translated generalized `Hexen` sector type sits behind.
const SECTOR_HEXEN_BASE: u32 = 64_u32;
/// The lowest `Hexen` type that reads back as translated (base 1).
const SECTOR_HEXEN_FLOOR: u32 = 65_u32;
/// The highest `Hexen` type that reads back as translated (base 17).
const SECTOR_HEXEN_TOP: u32 = 81_u32;
/// The `Hexen` secret bit a translated type-9 sector carries.
const SECTOR_HEXEN_SECRET: u32 = 1024_u32;
/// The `Hexen` bits 8 to 12: the moved damage, secret, friction and push
/// fields of a translated sector special.
const SECTOR_HEXEN_FIELDS: u32 = 0x1F00;
/// The seven low bits of a `Hexen` sector special: the type field.
const SECTOR_HEXEN_CORE: u32 = 0x7F;
/// The only bits a translated `Hexen` sector special may carry.
const SECTOR_HEXEN_ALLOWED: u32 = SECTOR_HEXEN_CORE | SECTOR_HEXEN_FIELDS;

/// The `Hexen` generalized sector special the Boom-style `value` becomes:
/// the basic type behind [`SECTOR_HEXEN_BASE`] (the secret bit for type 9,
/// nothing for type 0) with the damage, secret, friction and push fields
/// moved to their `Hexen` bits.
fn hexen_sector_bits(value: u32) -> u32 {
    let base = value & SECTOR_TYPE_MASK;
    let mut out = match base {
        0_u32 => 0_u32,
        SECTOR_SECRET_TYPE => SECTOR_HEXEN_SECRET,
        _ => SECTOR_HEXEN_BASE.saturating_add(base),
    };
    out |= move_field(value, 5_u32, FIELD_TWO, 8_u32);
    out |= move_field(value, 7_u32, FIELD_ONE, 10_u32);
    out |= move_field(value, 8_u32, FIELD_ONE, 11_u32);
    out |= move_field(value, 9_u32, FIELD_ONE, 12_u32);
    out
}

/// The Boom-style value a translated `Hexen` special `value` decodes to,
/// or `None` when it is no type this table translates: a native `Hexen`
/// effect, a bare field or a number the translation never writes.
fn boom_sector_bits(value: u32) -> Option<u32> {
    if value & !SECTOR_HEXEN_ALLOWED != 0 {
        return None;
    }
    let core = value & SECTOR_HEXEN_CORE;
    if !(SECTOR_HEXEN_FLOOR..=SECTOR_HEXEN_TOP).contains(&core) {
        return None;
    }
    let mut out = core.saturating_sub(SECTOR_HEXEN_BASE);
    out |= move_field(value, 8_u32, FIELD_TWO, 5_u32);
    out |= move_field(value, 10_u32, FIELD_ONE, 7_u32);
    out |= move_field(value, 11_u32, FIELD_ONE, 8_u32);
    out |= move_field(value, 12_u32, FIELD_ONE, 9_u32);
    Some(out)
}

/// The grouped warning for the sectors whose `special` a stage cleared,
/// its index list phrased by [`show_indices`].
fn cleared_sectors(kind: &str, lost: &[usize]) -> String {
    format!(
        "{} sectors had specials that are not {kind} (indices {}): they were set to 0",
        lost.len(),
        show_indices(lost),
    )
}

/// Translate every [`Sector::special`] of `map` in place to the `Hexen`
/// generalized form.
///
/// A Boom-style value (bits 0-4 type, 5-6 damage, 7 secret, 8 friction,
/// 9 push) becomes the type behind 64, the secret type
/// as the `Hexen` secret bit, and the four effect fields at their `Hexen`
/// positions. A value outside the Boom-style bits is cleared to 0 and
/// named in one grouped warning; [`Sector::tag`] is never touched.
#[must_use]
#[inline]
pub fn sectors_to_hexen(map: &mut Map) -> FieldReport {
    let mut report = FieldReport::default();
    let mut foreign: Vec<usize> = Vec::new();
    for (index, area) in map.sectors.iter_mut().enumerate() {
        if sector_to_hexen(area, &mut report.changed) {
            foreign.push(index);
        }
    }
    if !foreign.is_empty() {
        report
            .warnings
            .push(cleared_sectors("Boom-style generalized values", &foreign));
    }
    report
}

/// Rewrites one sector's `special` to its `Hexen` form, answering whether
/// the value needed clearing instead of translating.
fn sector_to_hexen(area: &mut Sector, changed: &mut usize) -> bool {
    let boom = match u32::try_from(area.special) {
        Ok(value) if value <= SECTOR_BOOM_MAX => Some(hexen_sector_bits(value)),
        Ok(_) | Err(_) => None,
    };
    if let Some(bits) = boom {
        let stored = i32::try_from(bits).unwrap_or(0_i32);
        *changed = count_if(area.special != stored, *changed);
        area.special = stored;
        false
    } else {
        *changed = count_if(area.special != 0_i32, *changed);
        area.special = 0_i32;
        true
    }
}

/// Translate every [`Sector::special`] of `map` in place back from the
/// `Hexen` generalized form to the Boom-style bits.
///
/// Types 65 through 81 return to
/// their Boom-style value with the four effect fields back at their
/// `Doom` positions, and a plain 0 stays 0. Any other value is no
/// translated Boom type: a native `Hexen` effect (or a Boom base type the
/// reverse table does not cover) clears to 0 and is named in one grouped
/// warning. [`Sector::tag`] is never touched.
#[must_use]
#[inline]
pub fn sectors_from_hexen(map: &mut Map) -> FieldReport {
    let mut report = FieldReport::default();
    let mut foreign: Vec<usize> = Vec::new();
    for (index, area) in map.sectors.iter_mut().enumerate() {
        if sector_from_hexen(area, &mut report.changed) {
            foreign.push(index);
        }
    }
    if !foreign.is_empty() {
        report
            .warnings
            .push(cleared_sectors("translated Boom types", &foreign));
    }
    report
}

/// Rewrites one sector's `special` back to its Boom-style value,
/// answering whether the value needed clearing instead.
fn sector_from_hexen(area: &mut Sector, changed: &mut usize) -> bool {
    if area.special == 0_i32 {
        return false;
    }
    let restored = u32::try_from(area.special).ok().and_then(boom_sector_bits);
    if let Some(bits) = restored {
        let stored = i32::try_from(bits).unwrap_or(0_i32);
        *changed = count_if(area.special != stored, *changed);
        area.special = stored;
        false
    } else {
        *changed = changed.saturating_add(1_usize);
        area.special = 0_i32;
        true
    }
}
