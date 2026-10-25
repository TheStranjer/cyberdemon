//! The [`Thing`](crate::wad::map::Thing) presence-flag stage of the
//! cross-family `Doom` ↔ `Hexen` conversion: the two families' thing bits,
//! the bit folds [`hexen_thing_flags`] and [`doom_thing_flags`] that carry
//! them across, [`clear_thing_script`], which drops the `Hexen`-only fields
//! `Doom` has no room for, and the two in-place stages [`things_to_hexen`]
//! and [`things_from_hexen`].
//!
//! Both stages answer with a [`FieldReport`](super::FieldReport) of the
//! things whose fields moved; the bits with no `Doom` equivalent are counted
//! by [`HexenDrops`](super::HexenDrops) into the one grouped warning the
//! reverse stage reports.

use crate::wad::map::model::{Map, Thing};

use super::{FieldReport, HexenDrops, count_if};

/// The low nibble the `Doom` and `Hexen` thing flags share: `easy`,
/// `medium`, `hard` and `ambush` keep their bit in both families.
const THING_SKILL_BITS: u32 = 0x0F;
/// The `Doom` thing bit 16: `notsingle`.
const THING_DOOM_NOT_SINGLE: u32 = 16_u32;
/// The `Doom` thing bit 32: `notdm`.
const THING_DOOM_NOT_DM: u32 = 32_u32;
/// The `Doom` thing bit 64: `notcoop`.
const THING_DOOM_NOT_COOP: u32 = 64_u32;
/// The `Doom` thing bit 128: `friend`.
const THING_DOOM_FRIEND: u32 = 128_u32;
/// The `Hexen` thing bit 16: `dormant`.
pub(super) const THING_HEXEN_DORMANT: u32 = 16_u32;
/// The `Hexen` thing bits 32|64|128: `class1`, `class2` and `class3`
/// combined.
const THING_HEXEN_CLASSES: u32 = 0xE0;
/// The `Hexen` thing bit 256: `single`.
const THING_HEXEN_SINGLE: u32 = 256_u32;
/// The `Hexen` thing bit 512: `coop`.
const THING_HEXEN_COOP: u32 = 512_u32;
/// The `Hexen` thing bit 1024: `dm`.
const THING_HEXEN_DM: u32 = 1024_u32;
/// The `Hexen` thing bit 2048: `translucent`.
pub(super) const THING_HEXEN_TRANSLUCENT: u32 = 2048_u32;
/// The `Hexen` thing bit 4096: `invisible`.
pub(super) const THING_HEXEN_INVISIBLE: u32 = 4096_u32;
/// The `Hexen` thing bit 8192: `friend`.
const THING_HEXEN_FRIEND: u32 = 8192_u32;
/// The `Hexen` thing bit 0x4000: `standing`.
pub(super) const THING_HEXEN_STANDING: u32 = 0x4000;

/// The `Hexen` thing flags the `Doom` family bits of `flags` become: the
/// shared skill bits, all three class bits, a presence bit for every game
/// mode the thing was not excluded from, and `friend` at its `Hexen` bit.
const fn hexen_thing_flags(flags: u32) -> u32 {
    let mut out = (flags & THING_SKILL_BITS) | THING_HEXEN_CLASSES;
    if flags & THING_DOOM_NOT_SINGLE == 0 {
        out |= THING_HEXEN_SINGLE;
    }
    if flags & THING_DOOM_NOT_COOP == 0 {
        out |= THING_HEXEN_COOP;
    }
    if flags & THING_DOOM_NOT_DM == 0 {
        out |= THING_HEXEN_DM;
    }
    if flags & THING_DOOM_FRIEND != 0 {
        out |= THING_HEXEN_FRIEND;
    }
    out
}

/// The `Doom` thing flags the `Hexen` family bits of `flags` become: the
/// shared skill bits, an exclusion for every game mode or class the thing
/// may not appear in, and `friend` back at its `Doom` bit. The bits `Doom`
/// has no room for drop; the caller reports them.
const fn doom_thing_flags(flags: u32) -> u32 {
    let mut out = flags & THING_SKILL_BITS;
    let all_classes = flags & THING_HEXEN_CLASSES == THING_HEXEN_CLASSES;
    if flags & THING_HEXEN_SINGLE == 0 || !all_classes {
        out |= THING_DOOM_NOT_SINGLE;
    }
    if flags & THING_HEXEN_COOP == 0 {
        out |= THING_DOOM_NOT_COOP;
    }
    if flags & THING_HEXEN_DM == 0 {
        out |= THING_DOOM_NOT_DM;
    }
    if flags & THING_HEXEN_FRIEND != 0 {
        out |= THING_DOOM_FRIEND;
    }
    out
}

/// Drops the script fields one cross-family thing translation clears:
/// `tid`, `special` and `args` carry data only in the `Hexen` family
/// storage.
const fn clear_thing_script(spawned: &mut Thing) {
    spawned.tid = 0_i32;
    spawned.special = 0_i32;
    spawned.args = [0_i32; 5];
}

/// Translate every [`Thing`] of `map` in place from `Doom` to `Hexen`
/// family flags.
///
/// The presence bits invert to their positive `Hexen` twins, every thing
/// joins all three classes, `friend` moves to its `Hexen` bit, and the
/// `tid`, `special` and `args` fields `Doom` has no room for are zeroed.
/// Coordinates, `angle`, `kind` and `z` stand untouched; the translation
/// drops nothing, so the report carries no warnings. `changed` counts the
/// things whose flags moved or whose script fields were not already zero.
#[must_use]
#[inline]
pub fn things_to_hexen(map: &mut Map) -> FieldReport {
    let mut report = FieldReport::default();
    for spawned in &mut map.things {
        let original = spawned.flags;
        let scripted =
            spawned.tid != 0_i32 || spawned.special != 0_i32 || spawned.args != [0_i32; 5];
        spawned.flags = hexen_thing_flags(original);
        clear_thing_script(spawned);
        report.changed = count_if(spawned.flags != original || scripted, report.changed);
    }
    report
}

/// Translate every [`Thing`] of `map` in place from `Hexen` to `Doom`
/// family flags.
///
/// A thing keeps its skill and ambush bits; it gains `notsingle` unless
/// its `single` bit and all three class bits are set, and `notcoop` and
/// `notdm` unless the matching presence bit is set; `friend` returns to
/// its `Doom` bit. The `dormant`, `translucent`, `invisible` and
/// `standing` bits have no `Doom` equivalent and drop with one grouped
/// warning; the `tid`, `special` and `args` fields are zeroed. `changed`
/// counts the things whose flags moved.
#[must_use]
#[inline]
pub fn things_from_hexen(map: &mut Map) -> FieldReport {
    let mut report = FieldReport::default();
    let mut dropped = HexenDrops::default();
    for spawned in &mut map.things {
        let original = spawned.flags;
        dropped.note(original);
        spawned.flags = doom_thing_flags(original);
        clear_thing_script(spawned);
        report.changed = count_if(spawned.flags != original, report.changed);
    }
    if let Some(warning) = dropped.warning() {
        report.warnings.push(warning);
    }
    report
}
