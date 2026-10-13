//! Pure decoding of Boom's generalized specials and of Doom sector values.
//!
//! Boom packed endless linedef actions into the `special` field instead of
//! enumerating them: values `0x2F80..=0x7FFF` are bit fields, not table
//! numbers ([`decode_generalized_linedef`] unfolds them). Sector `special`
//! values are a small bit field of their own on top of the vanilla type
//! ([`decode_sector_special`]). This module holds no state and reads no
//! tables; the display names it returns mirror the shipped
//! `data/defs/doom_sectors.tsv`, kept honest by a registry test.

use alloc::collections::BTreeMap;

use serde::Serialize;

/// The lowest generalized linedef special value.
pub const GENERALIZED_MIN: u32 = 0x2F80;

/// The highest generalized linedef special value.
pub const GENERALIZED_MAX: u32 = 0x7FFF;

/// The one-bit field mask.
const ONE_MASK: u32 = 1;

/// The two-bit field mask.
const TWO_MASK: u32 = 0b11;

/// The three-bit field mask.
const THREE_MASK: u32 = 0b111;

/// The five-bit field mask: the basic sector type, bits 0-4.
const FIVE_MASK: u32 = 0b1_1111;

/// Trigger names indexed by bits 0-2.
const TRIGGERS: [&str; 8] = ["W1", "WR", "S1", "SR", "G1", "GR", "D1", "DR"];

/// Speed names indexed by bits 3-4.
const SPEEDS: [&str; 4] = ["slow", "normal", "fast", "turbo"];

/// Damage-per-touch names indexed by sector bits 5-6.
const DAMAGE_LEVELS: [&str; 4] = ["none", "5%", "10%", "20%"];

/// Generalized floor targets indexed by bits 7-9.
const FLOOR_TARGETS: [&str; 8] = [
    "highest neighbour floor",
    "lowest neighbour floor",
    "next neighbour floor",
    "lowest neighbour ceiling",
    "ceiling",
    "shortest lower texture",
    "24 units",
    "32 units",
];

/// Generalized ceiling targets indexed by bits 7-9.
const CEILING_TARGETS: [&str; 8] = [
    "highest neighbour ceiling",
    "lowest neighbour ceiling",
    "next neighbour ceiling",
    "highest neighbour floor",
    "floor",
    "shortest upper texture",
    "24 units",
    "32 units",
];

/// Texture and type change modes indexed by bits 10-11 of floors and
/// ceilings.
const CHANGE_MODES: [&str; 4] = ["none", "zero type", "texture", "texture and type"];

/// Generalized door kinds indexed by bits 5-6.
const DOOR_KINDS: [&str; 4] = [
    "open, wait, close",
    "open and stay",
    "close, wait, open",
    "close and stay",
];

/// Generalized door wait times indexed by bits 8-9.
const DOOR_DELAYS: [&str; 4] = ["1 s", "4 s", "9 s", "30 s"];

/// Keys for generalized locked doors, indexed by bits 6-8.
const LOCK_KEYS: [&str; 8] = [
    "any",
    "red card",
    "blue card",
    "yellow card",
    "red skull",
    "blue skull",
    "yellow skull",
    "all six",
];

/// Generalized lift wait times indexed by bits 6-7.
const LIFT_DELAYS: [&str; 4] = ["1 s", "3 s", "5 s", "10 s"];

/// Generalized lift targets indexed by bits 8-9.
const LIFT_TARGETS: [&str; 4] = [
    "lowest neighbour floor",
    "next neighbour floor",
    "lowest neighbour ceiling",
    "perpetual",
];

/// Generalized stair step heights indexed by bits 6-7.
const STAIR_STEPS: [&str; 4] = ["4", "8", "16", "24"];

/// Yes/no text for one-bit flags, indexed by the bit.
const YES_NO: [&str; 2] = ["no", "yes"];

/// The trigger and speed pair shared by every generalized kind, owned for
/// reuse across the fields map and the summary.
type Common = (String, String);

/// The bit layouts a sector special value can follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectorFamily {
    /// The Doom family (`DOOM`, `DOOM II`, `Strife` and Boom-compatible
    /// games): the basic type plus the Boom damage and effect bits.
    Doom,
    /// Any other game: this decoder knows no bit layout for its sector
    /// values.
    Other,
}

/// One Boom effect flag of a decoded sector special value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SectorFlag {
    /// Bit 7: the sector counts as secret.
    Secret,
    /// Bit 8: friction and current effects are active in the sector.
    Friction,
    /// Bit 9: wind and pusher effects are active in the sector.
    Push,
}

/// A decoded Boom generalized linedef special.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generalized {
    /// The action family: `"Floor"`, `"Ceiling"`, `"Door"`,
    /// `"Locked Door"`, `"Lift"`, `"Stairs"` or `"Crusher"`.
    pub kind: String,
    /// How the special fires, as the editor prefix: `W1`, `WR`, `S1`, `SR`,
    /// `G1`, `GR`, `D1` or `DR`.
    pub trigger: String,
    /// Movement speed: `slow`, `normal`, `fast` or `turbo`.
    pub speed: String,
    /// The kind-specific bit fields with human-readable values, ordered by
    /// field name.
    pub fields: BTreeMap<String, String>,
    /// A one-line description, e.g. `"WR Floor up to lowest neighbour
    /// ceiling, fast, change texture"`.
    pub summary: String,
}

/// A decoded sector special value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectorDecoded {
    /// The basic type, bits 0-4 (`value & 31`).
    pub base: u32,
    /// The display name of the basic type, `"Unknown"` outside the shipped
    /// table.
    pub base_name: String,
    /// Damage per touch from bits 5-6: `"none"`, `"5%"`, `"10%"` or
    /// `"20%"`.
    pub damage: String,
    /// The effect flags set in bits 7-9, in bit order.
    pub flags: Vec<SectorFlag>,
}

/// Whether `special` lies in the generalized range at all.
#[must_use]
#[inline]
pub const fn is_generalized(special: u32) -> bool {
    special >= GENERALIZED_MIN && special <= GENERALIZED_MAX
}

/// Decode a Boom generalized linedef special.
///
/// Returns [`None`] unless `special` lies in `0x2F80..=0x7FFF`.
#[must_use]
#[inline]
pub fn decode_generalized_linedef(special: u32) -> Option<Generalized> {
    if !is_generalized(special) {
        return None;
    }
    let common: Common = (
        named(&TRIGGERS, field(special, 0, THREE_MASK)).to_owned(),
        named(&SPEEDS, field(special, 3, TWO_MASK)).to_owned(),
    );
    if (0x6000..=0x7FFF).contains(&special) {
        return Some(decode_floor(special, common));
    }
    if (0x4000..=0x5FFF).contains(&special) {
        return Some(decode_ceiling(special, common));
    }
    if (0x3C00..=0x3FFF).contains(&special) {
        return Some(decode_door(special, common));
    }
    if (0x3800..=0x3BFF).contains(&special) {
        return Some(decode_locked_door(special, common));
    }
    if (0x3400..=0x37FF).contains(&special) {
        return Some(decode_lift(special, common));
    }
    if (0x3000..=0x33FF).contains(&special) {
        return Some(decode_stairs(special, common));
    }
    Some(decode_crusher(special, common))
}

/// Decode a sector special value for the given game family.
///
/// Bits above 9 are ignored; [`None`] means `family` follows no layout
/// this decoder knows.
#[must_use]
#[inline]
pub fn decode_sector_special(value: u32, family: SectorFamily) -> Option<SectorDecoded> {
    if family != SectorFamily::Doom {
        return None;
    }
    let base = value & FIVE_MASK;
    Some(SectorDecoded {
        base,
        base_name: doom_base_name(base).to_owned(),
        damage: named(&DAMAGE_LEVELS, field(value, 5, TWO_MASK)).to_owned(),
        flags: decode_flags(value),
    })
}

/// The effect flags set in bits 7-9 of a sector value, in bit order.
fn decode_flags(value: u32) -> Vec<SectorFlag> {
    let mut flags: Vec<SectorFlag> = Vec::new();
    if bit(value, 7) {
        flags.push(SectorFlag::Secret);
    }
    if bit(value, 8) {
        flags.push(SectorFlag::Friction);
    }
    if bit(value, 9) {
        flags.push(SectorFlag::Push);
    }
    flags
}

/// The display name of a Doom basic sector type (`0..=31`), mirroring
/// `data/defs/doom_sectors.tsv`; values the table leaves unassigned answer
/// `"Unknown"`.
const fn doom_base_name(base: u32) -> &'static str {
    match base {
        0 => "Normal",
        1 => "Light On+Off Randomly",
        2 => "Light Strobe 0.5 Sec",
        3 => "Light Strobe 1 Sec",
        4 => "20% Damage + Strobe 0.5 Sec",
        5 => "10% Damage",
        7 => "5% Damage",
        8 => "Light Glow",
        9 => "Secret",
        10 => "Door Close After 30 Sec",
        11 => "20% Damage + Exit < 10",
        12 => "Light Strobe 1.0 Sec In Sync",
        13 => "Light Strobe 0.5 Sec In Sync",
        14 => "Door Open After 5 Min",
        16 => "20% Damage",
        17 => "Light Flicker",
        _ => "Unknown",
    }
}

/// Decode a generalized floor (`0x6000..=0x7FFF`).
fn decode_floor(value: u32, common: Common) -> Generalized {
    let direction = if bit(value, 6) { "up" } else { "down" };
    let target = named(&FLOOR_TARGETS, field(value, 7, THREE_MASK));
    let change = field(value, 10, TWO_MASK);
    let crush = bit(value, 12);
    let mut fields = common_fields(&common);
    fields.insert(
        "model".to_owned(),
        flag_text(bit(value, 5), "numeric", "trigger"),
    );
    fields.insert("direction".to_owned(), direction.to_owned());
    fields.insert("target".to_owned(), target.to_owned());
    fields.insert("change".to_owned(), named(&CHANGE_MODES, change).to_owned());
    fields.insert("crush".to_owned(), yes_no(crush));
    let summary = format!(
        "{} Floor {direction} to {target}{}",
        common.0,
        speed_tail(&common.1, change, crush)
    );
    generalized("Floor", common, fields, summary)
}

/// Decode a generalized ceiling (`0x4000..=0x5FFF`).
fn decode_ceiling(value: u32, common: Common) -> Generalized {
    let direction = if bit(value, 6) { "down" } else { "up" };
    let target = named(&CEILING_TARGETS, field(value, 7, THREE_MASK));
    let change = field(value, 10, TWO_MASK);
    let mut fields = common_fields(&common);
    fields.insert(
        "model".to_owned(),
        flag_text(bit(value, 5), "numeric", "trigger"),
    );
    fields.insert("direction".to_owned(), direction.to_owned());
    fields.insert("target".to_owned(), target.to_owned());
    fields.insert("change".to_owned(), named(&CHANGE_MODES, change).to_owned());
    let summary = format!(
        "{} Ceiling {direction} to {target}{}",
        common.0,
        speed_tail(&common.1, change, false)
    );
    generalized("Ceiling", common, fields, summary)
}

/// Decode a generalized door (`0x3C00..=0x3FFF`).
fn decode_door(value: u32, common: Common) -> Generalized {
    let kind = named(&DOOR_KINDS, field(value, 5, TWO_MASK));
    let monsters = bit(value, 7);
    let delay = named(&DOOR_DELAYS, field(value, 8, TWO_MASK));
    let mut fields = common_fields(&common);
    fields.insert("kind".to_owned(), kind.to_owned());
    fields.insert("monsters".to_owned(), yes_no(monsters));
    fields.insert("delay".to_owned(), delay.to_owned());
    let summary = format!(
        "{} Door {kind}, {}, {delay} delay{}",
        common.0,
        common.1,
        monsters_tail(monsters)
    );
    generalized("Door", common, fields, summary)
}

/// Decode a generalized locked door (`0x3800..=0x3BFF`).
fn decode_locked_door(value: u32, common: Common) -> Generalized {
    let kind = flag_text(bit(value, 5), "open and stay", "open, wait, close");
    let key = named(&LOCK_KEYS, field(value, 6, THREE_MASK));
    let skull = bit(value, 9);
    let skull_tail = flag_text(skull, ", skull acts as card", "");
    let mut fields = common_fields(&common);
    fields.insert("kind".to_owned(), kind.clone());
    fields.insert("key".to_owned(), key.to_owned());
    fields.insert("skull acts as card".to_owned(), yes_no(skull));
    let summary = format!(
        "{} Locked Door {kind}, {} {key} key{skull_tail}",
        common.0, common.1
    );
    generalized("Locked Door", common, fields, summary)
}

/// Decode a generalized lift (`0x3400..=0x37FF`).
fn decode_lift(value: u32, common: Common) -> Generalized {
    let monsters = bit(value, 5);
    let delay = named(&LIFT_DELAYS, field(value, 6, TWO_MASK));
    let target = named(&LIFT_TARGETS, field(value, 8, TWO_MASK));
    let mut fields = common_fields(&common);
    fields.insert("monsters".to_owned(), yes_no(monsters));
    fields.insert("delay".to_owned(), delay.to_owned());
    fields.insert("target".to_owned(), target.to_owned());
    let summary = format!(
        "{} Lift to {target}, {}, {delay} delay{}",
        common.0,
        common.1,
        monsters_tail(monsters)
    );
    generalized("Lift", common, fields, summary)
}

/// Decode a generalized stair builder (`0x3000..=0x33FF`).
fn decode_stairs(value: u32, common: Common) -> Generalized {
    let monsters = bit(value, 5);
    let step = named(&STAIR_STEPS, field(value, 6, TWO_MASK));
    let direction = if bit(value, 8) { "up" } else { "down" };
    let ignore = bit(value, 9);
    let mut fields = common_fields(&common);
    fields.insert("monsters".to_owned(), yes_no(monsters));
    fields.insert("step".to_owned(), format!("{step} units"));
    fields.insert("direction".to_owned(), direction.to_owned());
    fields.insert("ignore textures".to_owned(), yes_no(ignore));
    let ignore_tail = flag_text(ignore, ", ignore textures", "");
    let summary = format!(
        "{} Stairs {direction} {step}-unit steps, {}{ignore_tail}{}",
        common.0,
        common.1,
        monsters_tail(monsters)
    );
    generalized("Stairs", common, fields, summary)
}

/// Decode a generalized crusher (`0x2F80..=0x2FFF`).
fn decode_crusher(value: u32, common: Common) -> Generalized {
    let monsters = bit(value, 5);
    let silent = bit(value, 6);
    let mut fields = common_fields(&common);
    fields.insert("monsters".to_owned(), yes_no(monsters));
    fields.insert("silent".to_owned(), yes_no(silent));
    let silent_tail = flag_text(silent, ", silent", "");
    let summary = format!(
        "{} Crusher, {}{silent_tail}{}",
        common.0,
        common.1,
        monsters_tail(monsters)
    );
    generalized("Crusher", common, fields, summary)
}

/// Assemble a decoded value of the given kind from its parts.
fn generalized(
    kind: &str,
    common: Common,
    fields: BTreeMap<String, String>,
    summary: String,
) -> Generalized {
    Generalized {
        kind: kind.to_owned(),
        trigger: common.0,
        speed: common.1,
        fields,
        summary,
    }
}

/// The `, speed[, change ...][, crush]` tail of floor and ceiling summaries.
fn speed_tail(speed: &str, change: usize, crush: bool) -> String {
    let mut tail = format!(", {speed}");
    if change > 0 {
        tail = format!("{tail}, change {}", named(&CHANGE_MODES, change));
    }
    if crush {
        tail = format!("{tail}, crush");
    }
    tail
}

/// The ", also monsters" tail when the monster-usable bit is set.
const fn monsters_tail(monsters: bool) -> &'static str {
    if monsters { ", also monsters" } else { "" }
}

/// The `trigger` and `speed` entries every kind carries in its fields.
fn common_fields(common: &Common) -> BTreeMap<String, String> {
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    fields.insert("trigger".to_owned(), common.0.clone());
    fields.insert("speed".to_owned(), common.1.clone());
    fields
}

/// The `(value >> offset) & mask` bit field as an index; out-of-range
/// extractions read as 0.
fn field(value: u32, offset: u32, mask: u32) -> usize {
    value
        .checked_shr(offset)
        .and_then(|shifted| usize::try_from(shifted & mask).ok())
        .unwrap_or(0)
}

/// Whether bit `offset` of `value` is set.
fn bit(value: u32, offset: u32) -> bool {
    field(value, offset, ONE_MASK) == 1
}

/// The name at `index` of a complete table (indices are masked before
/// lookup, so the fallback cannot trigger in practice).
fn named(table: &[&'static str], index: usize) -> &'static str {
    table.get(index).copied().unwrap_or("unknown")
}

/// The `yes`/`no` text of a flag bit.
fn yes_no(flag: bool) -> String {
    named(&YES_NO, usize::from(flag)).to_owned()
}

/// One of two texts chosen by a flag bit.
fn flag_text(flag: bool, when_set: &str, when_clear: &str) -> String {
    if flag {
        when_set.to_owned()
    } else {
        when_clear.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SectorFamily, SectorFlag, decode_generalized_linedef, decode_sector_special, is_generalized,
    };

    /// Bits 0-2 trigger `t` plus bits 3-4 speed `s`.
    fn trigger_speed(trigger: u32, speed: u32) -> u32 {
        trigger | speed.checked_shl(3).expect("speed shift fits")
    }

    /// `value` shifted into bit `offset`.
    fn at(value: u32, offset: u32) -> u32 {
        value.checked_shl(offset).expect("test shift fits")
    }

    #[test]
    fn values_outside_the_generalized_range_are_not_decoded() {
        assert!(!is_generalized(0), "plain zero");
        assert!(!is_generalized(0x2F7F), "one below the range");
        assert!(is_generalized(0x2F80), "the first generalized value");
        assert!(is_generalized(0x7FFF), "the last generalized value");
        assert!(!is_generalized(0x8000), "one above the range");
        assert!(
            decode_generalized_linedef(0x2F7F).is_none(),
            "0x2F7F is a table value"
        );
        assert!(
            decode_generalized_linedef(0x8000).is_none(),
            "0x8000 is out of range"
        );
    }

    #[test]
    fn decodes_a_generalized_floor() {
        // 0x6000 | WR | fast | up | target lowest neighbour ceiling.
        let value = 0x6000 | trigger_speed(1, 2) | at(1, 6) | at(3, 7);
        let decoded = decode_generalized_linedef(value).expect("a floor value decodes");
        assert_eq!(decoded.kind, "Floor", "kind");
        assert_eq!(decoded.trigger, "WR", "trigger from bits 0-2");
        assert_eq!(decoded.speed, "fast", "speed from bits 3-4");
        assert_eq!(decoded.fields["direction"], "up", "direction bit 6");
        assert_eq!(
            decoded.fields["target"], "lowest neighbour ceiling",
            "target bits 7-9"
        );
        assert_eq!(decoded.fields["model"], "trigger", "model bit 5 unset");
        assert_eq!(decoded.fields["change"], "none", "change bits unset");
        assert_eq!(decoded.fields["crush"], "no", "crush bit unset");
        assert_eq!(
            decoded.summary, "WR Floor up to lowest neighbour ceiling, fast",
            "summary text"
        );
    }

    #[test]
    fn floor_summary_adds_change_and_crush() {
        // S1, normal, down, ceiling (target 4), change texture, crushing.
        let value = 0x6000 | trigger_speed(2, 1) | at(4, 7) | at(2, 10) | at(1, 12);
        let decoded = decode_generalized_linedef(value).expect("a floor value decodes");
        assert_eq!(
            decoded.summary, "S1 Floor down to ceiling, normal, change texture, crush",
            "change and crush show up"
        );
    }

    #[test]
    fn decodes_a_generalized_ceiling() {
        // DR, turbo, numeric model, down (bit 6 set), target 24 units.
        let value = 0x4000 | trigger_speed(7, 3) | at(1, 5) | at(1, 6) | at(6, 7);
        let decoded = decode_generalized_linedef(value).expect("a ceiling value decodes");
        assert_eq!(decoded.kind, "Ceiling", "kind");
        assert_eq!(decoded.trigger, "DR", "trigger");
        assert_eq!(decoded.speed, "turbo", "speed");
        assert_eq!(
            decoded.fields["direction"], "down",
            "ceiling bit 6 means down"
        );
        assert_eq!(
            decoded.fields["model"], "numeric",
            "bit 5 selects the numeric model"
        );
        assert_eq!(decoded.fields["target"], "24 units", "target bits 7-9");
        assert_eq!(
            decoded.summary, "DR Ceiling down to 24 units, turbo",
            "summary text"
        );
    }

    #[test]
    fn decodes_a_generalized_door() {
        // SR, normal, close-wait-open, monster-usable, 30 s delay.
        let value = 0x3C00 | trigger_speed(3, 1) | at(2, 5) | at(1, 7) | at(3, 8);
        let decoded = decode_generalized_linedef(value).expect("a door value decodes");
        assert_eq!(decoded.kind, "Door", "kind");
        assert_eq!(decoded.fields["kind"], "close, wait, open", "kind bits 5-6");
        assert_eq!(decoded.fields["monsters"], "yes", "monster bit 7");
        assert_eq!(decoded.fields["delay"], "30 s", "delay bits 8-9");
        assert_eq!(
            decoded.summary, "SR Door close, wait, open, normal, 30 s delay, also monsters",
            "summary text"
        );
    }

    #[test]
    fn decodes_a_generalized_locked_door() {
        // G1, slow, open-and-stay (bit 5), red skull (key 4), skull as card (bit 9).
        let value = 0x3800 | trigger_speed(4, 0) | at(1, 5) | at(4, 6) | at(1, 9);
        let decoded = decode_generalized_linedef(value).expect("a lock value decodes");
        assert_eq!(decoded.kind, "Locked Door", "kind");
        assert_eq!(decoded.fields["kind"], "open and stay", "kind bit 5");
        assert_eq!(decoded.fields["key"], "red skull", "key bits 6-8");
        assert_eq!(decoded.fields["skull acts as card"], "yes", "bit 9");
        assert_eq!(
            decoded.summary, "G1 Locked Door open and stay, slow red skull key, skull acts as card",
            "summary text"
        );
    }

    #[test]
    fn decodes_a_generalized_lift() {
        // W1, fast, monster-usable (bit 5), 5 s delay (2), perpetual (3).
        let value = 0x3400 | trigger_speed(0, 2) | at(1, 5) | at(2, 6) | at(3, 8);
        let decoded = decode_generalized_linedef(value).expect("a lift value decodes");
        assert_eq!(decoded.kind, "Lift", "kind");
        assert_eq!(decoded.trigger, "W1", "trigger");
        assert_eq!(decoded.fields["monsters"], "yes", "bit 5");
        assert_eq!(decoded.fields["delay"], "5 s", "delay bits 6-7");
        assert_eq!(decoded.fields["target"], "perpetual", "target bits 8-9");
        assert_eq!(
            decoded.summary, "W1 Lift to perpetual, fast, 5 s delay, also monsters",
            "summary text"
        );
    }

    #[test]
    fn decodes_a_generalized_stairs() {
        // S1, normal, up (bit 8), 16-unit steps (2), ignore textures (bit 9).
        let value = 0x3000 | trigger_speed(2, 1) | at(2, 6) | at(1, 8) | at(1, 9);
        let decoded = decode_generalized_linedef(value).expect("a stairs value decodes");
        assert_eq!(decoded.kind, "Stairs", "kind");
        assert_eq!(decoded.fields["step"], "16 units", "step bits 6-7");
        assert_eq!(decoded.fields["direction"], "up", "direction bit 8");
        assert_eq!(decoded.fields["ignore textures"], "yes", "bit 9");
        assert_eq!(decoded.fields["monsters"], "no", "bit 5 unset");
        assert_eq!(
            decoded.summary, "S1 Stairs up 16-unit steps, normal, ignore textures",
            "summary text"
        );
    }

    #[test]
    fn decodes_a_generalized_crusher() {
        // GR, slow, monster-usable (bit 5), silent (bit 6): the range floor.
        let value = 0x2F80 | trigger_speed(5, 0) | at(1, 5) | at(1, 6);
        let decoded = decode_generalized_linedef(value).expect("the range floor decodes");
        assert_eq!(decoded.kind, "Crusher", "kind");
        assert_eq!(decoded.trigger, "GR", "trigger");
        assert_eq!(decoded.speed, "slow", "speed");
        assert_eq!(decoded.fields["monsters"], "yes", "bit 5");
        assert_eq!(decoded.fields["silent"], "yes", "bit 6");
        assert_eq!(
            decoded.summary, "GR Crusher, slow, silent, also monsters",
            "summary text"
        );
    }

    #[test]
    fn the_range_top_still_decodes_as_a_floor() {
        let decoded =
            decode_generalized_linedef(0x7FFF).expect("the highest generalized value decodes");
        assert_eq!(decoded.kind, "Floor", "0x7FFF is in the floor block");
        assert_eq!(decoded.trigger, "DR", "every field saturated");
        assert_eq!(decoded.speed, "turbo", "speed bits saturated");
        assert_eq!(decoded.fields["crush"], "yes", "bit 12 set");
        assert_eq!(
            decoded.fields["target"], "32 units",
            "target bits saturated"
        );
        assert_eq!(
            decoded.fields["change"], "texture and type",
            "change bits saturated"
        );
    }

    #[test]
    fn decodes_a_boom_sector_value() {
        // base 9, 10% damage (bits 5-6 = 2), secret flag (bit 7).
        let value = 9 | at(2, 5) | at(1, 7);
        let decoded =
            decode_sector_special(value, SectorFamily::Doom).expect("the Doom family decodes");
        assert_eq!(decoded.base, 9, "basic type is value & 31");
        assert_eq!(decoded.base_name, "Secret", "table name of type 9");
        assert_eq!(decoded.damage, "10%", "damage from bits 5-6");
        assert_eq!(decoded.flags, vec![SectorFlag::Secret], "bit 7 sets secret");
    }

    #[test]
    fn sector_flags_and_unknown_bases_decode() {
        // friction (bit 8) and push (bit 9) on base 6, which the table leaves unassigned.
        let value = 6 | at(1, 8) | at(1, 9);
        let decoded =
            decode_sector_special(value, SectorFamily::Doom).expect("the Doom family decodes");
        assert_eq!(decoded.base_name, "Unknown", "unassigned base");
        assert_eq!(decoded.damage, "none", "damage bits clear");
        assert_eq!(
            decoded.flags,
            vec![SectorFlag::Friction, SectorFlag::Push],
            "bits 8 and 9 in order"
        );
    }

    #[test]
    fn other_families_report_nothing() {
        assert!(
            decode_sector_special(16, SectorFamily::Other).is_none(),
            "the Doom bit layout is not applied to other games"
        );
    }
}
