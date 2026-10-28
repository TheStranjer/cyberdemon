//! The one family-aware classification of linedef specials.
//!
//! A special number means something different in every flag family: `1` is
//! the `Doom` family's `DR` door and the `Hexen` family's
//! `Polyobj_StartLine`, and `12` is nobody's action in the `Doom` family's
//! table and `Door_Raise` in the `Hexen` one. Every door detection, action
//! walk and exit reading therefore classifies a line through the map's own
//! [`FlagFamily`], never through the session game's definition tables:
//! [`special_class`] answers what one number does, [`is_door_action`] adds
//! the `Doom` family's `Boom` generalized doors to that answer,
//! [`doom_row`] is the playability walk's lookup that decodes a generalized
//! door, locked door or lift into the row its bits describe, and
//! [`exit_for`] resolves one line's exit rule with its trigger read off
//! the line itself.
//!
//! How a player sets a line off is the [`Trigger`] the rows carry. The
//! `Hexen` family keeps that answer in its activation field, and only some
//! of its six values name a player action: `playercross` reaches both
//! sides, `playeruse` the front with a press, `impact`, `playerpush` and
//! `missilecross` the front with a shot or a shove ([`Trigger::Shoot`]),
//! and `monstercross` no player at all ([`Trigger::Monster`]). `UDMF`'s
//! `monsteruse` and `monsterpush` booleans arrive through this crate's
//! `UDMF` reader as their player counterpart plus the `monsteractivate`
//! bit ([`crate::wad::map::udmf_map`]), so they read here as the player's
//! own use and push: the flag space has no third encoding, and only
//! `monstercross` stays a monster's own activation.
//!
//! The rows are the [`tables`] module: hand-written engine vocabulary,
//! shared with the playability walk
//! ([`crate::wad::map::playability`]) that folds them into a map's feature
//! inventory. The `auto_texture` engine ([`crate::themes::auto`]) takes its
//! door lines from a caller that asks [`is_door_action`] of every linedef;
//! nothing else re-reads the numbers.

pub mod tables;

use crate::defs::boom::{Generalized, decode_generalized_linedef, is_generalized};
use crate::wad::map::flags::{
    ACTIVATION_MASK, ACTIVATION_SHIFT, FlagFamily, IMPACT, MISSILE_CROSS, MONSTER_CROSS,
    PLAYER_PUSH, PLAYER_USE,
};
use crate::wad::map::model::Linedef;
use crate::wad::map::playability::KeyColor;
use tables::{
    DOOM_EXITS, DOOM_ROWS, DoomRow, ExitRow, HEXEN_EXITS, HEXEN_ROWS, RowEffect, Trigger,
};

/// What a table-carried line action does, read in one flag family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecialClass {
    /// A door action: opens, raises, locks or closes a door sector.
    Door,
    /// A platform lift action.
    Lift,
    /// A teleport action.
    Teleport,
    /// A level end. `secret` marks the exit that leads to the secret map;
    /// `trigger` is the `Doom` family's fixed one, and `None` for the
    /// `Hexen` family, whose activation field decides per line - read a
    /// concrete line through [`exit_for`].
    Exit {
        /// Whether it ends the level through the secret map.
        secret: bool,
        /// How the player reaches it, `None` when the line's own
        /// activation field says.
        trigger: Option<Trigger>,
    },
}

/// The `Boom` generalized door kind, as [`decode_generalized_linedef`]
/// spells it.
const GENERALIZED_DOOR: &str = "Door";

/// The `Boom` generalized locked-door kind, as
/// [`decode_generalized_linedef`] spells it.
const GENERALIZED_LOCKED_DOOR: &str = "Locked Door";

/// The `Boom` generalized lift kind, as [`decode_generalized_linedef`]
/// spells it.
const GENERALIZED_LIFT: &str = "Lift";

/// The `Doom` family's door kinds among the `Boom` generalized bit
/// patterns, as [`decode_generalized_linedef`] spells them.
const GENERALIZED_DOOR_KINDS: [&str; 2] = [GENERALIZED_DOOR, GENERALIZED_LOCKED_DOOR];

/// The `Doom` family's feature row for one special number: its
/// [`DOOM_ROWS`] entry, or, for a `Boom` generalized bit pattern, the door,
/// locked-door or lift row its bits decode to.
///
/// This is the playability walk's lookup. [`special_class`] stays the pure
/// table read; a generalized number is one of its rows here, so a `Boom`
/// map whose route runs through a generalized door walks like any other. A
/// generalized `Floor`, `Ceiling`, `Stairs` or `Crusher` answers `None`
/// like any other untabled number, and so does a locked door keyed to
/// `"any"` or `"all six"`: the walk names locks one colour at a time, and
/// counts what it cannot name instead of guessing it open.
#[must_use]
#[inline]
pub fn doom_row(special: i32) -> Option<DoomRow> {
    if let Some(row) = DOOM_ROWS.iter().find(|entry| entry.special == special) {
        return Some(*row);
    }
    u32::try_from(special).ok().and_then(generalized_row)
}

/// What line special `special` does in flag family `family`, `None` when
/// no table of that family carries it.
///
/// This is the pure table read: the `Doom` family's `Boom` generalized bit
/// patterns are no action here (see [`is_door_action`] for the
/// door-including-everything answer a texture pass wants, and
/// [`doom_row`] for the playability walk's door/locked-door/lift reading),
/// and a `Hexen` family exit's trigger rides the line's activation field,
/// so the class names it `None`.
#[must_use]
#[inline]
pub fn special_class(family: FlagFamily, special: i32) -> Option<SpecialClass> {
    match family {
        FlagFamily::Doom => doom_class(special),
        FlagFamily::Hexen => hexen_class(special),
    }
}

/// Whether `special` works a door in flag family `family`: any door row of
/// the family's table - open, raise, locked raise or close - or, in the
/// `Doom` family, a `Boom` generalized door bit pattern.
#[must_use]
#[inline]
pub fn is_door_action(special: i32, family: FlagFamily) -> bool {
    if special_class(family, special) == Some(SpecialClass::Door) {
        return true;
    }
    family == FlagFamily::Doom && u32::try_from(special).is_ok_and(generalized_door)
}

/// The exit row `line` carries, `None` when its special ends no level.
///
/// The `Doom` family's row brings its fixed trigger; the `Hexen`
/// family's portals take their side from the line's own activation field.
#[must_use]
#[inline]
pub fn exit_for(family: FlagFamily, line: &Linedef) -> Option<ExitRow> {
    match family {
        FlagFamily::Doom => DOOM_EXITS
            .iter()
            .copied()
            .find(|row| row.special == line.special),
        FlagFamily::Hexen => HEXEN_EXITS
            .iter()
            .find(|entry| entry.0 == line.special)
            .map(|entry| ExitRow {
                special: entry.0,
                secret: entry.1,
                trigger: hexen_trigger(line.flags),
            }),
    }
}

/// How a `Hexen` family line is worked, read off its activation field.
///
/// `playeruse` needs the player on the front side with a press;
/// `monstercross` is only ever set off by a monster ([`Trigger::Monster`]);
/// `impact`, `playerpush` and `missilecross` are worked with a shot, a
/// shove or a missile sent across, which this model reads as the front
/// side only ([`Trigger::Shoot`], see [`tables::Trigger`]); and
/// `playercross` - like the two field values the binary format leaves
/// unassigned - is the walk-over answer that reaches either side.
#[must_use]
#[inline]
pub fn hexen_trigger(flags: u32) -> Trigger {
    let field = (flags & ACTIVATION_MASK)
        .checked_shr(ACTIVATION_SHIFT)
        .unwrap_or(0_u32);
    match field {
        PLAYER_USE => Trigger::Use,
        MONSTER_CROSS => Trigger::Monster,
        IMPACT | PLAYER_PUSH | MISSILE_CROSS => Trigger::Shoot,
        _ => Trigger::Walk,
    }
}

/// The colour a `Hexen` lock code answers for, `None` for a code beyond the
/// six vanilla keys and the three any-colour numbers.
#[must_use]
#[inline]
pub const fn lock_color(code: i32) -> Option<KeyColor> {
    match code {
        1_i32 | 4_i32 | 129_i32 => Some(KeyColor::Red),
        2_i32 | 5_i32 | 130_i32 => Some(KeyColor::Blue),
        3_i32 | 6_i32 | 131_i32 => Some(KeyColor::Yellow),
        _ => None,
    }
}

/// The `Doom` family's read of one special: an exit row first, then a
/// feature row.
fn doom_class(special: i32) -> Option<SpecialClass> {
    if let Some(exit) = DOOM_EXITS.iter().find(|row| row.special == special) {
        return Some(SpecialClass::Exit {
            secret: exit.secret,
            trigger: Some(exit.trigger),
        });
    }
    DOOM_ROWS
        .iter()
        .find(|row| row.special == special)
        .map(|row| effect_class(row.effect))
}

/// The `Hexen` family's read of one special: an exit pair first, then a
/// scripted-action row; the exit's trigger is the line's own, so `None`.
fn hexen_class(special: i32) -> Option<SpecialClass> {
    if let Some(exit) = HEXEN_EXITS.iter().find(|entry| entry.0 == special) {
        return Some(SpecialClass::Exit {
            secret: exit.1,
            trigger: None,
        });
    }
    HEXEN_ROWS
        .iter()
        .find(|row| row.special == special)
        .map(|row| effect_class(row.effect))
}

/// The class one feature row's effect belongs to.
const fn effect_class(effect: RowEffect) -> SpecialClass {
    match effect {
        RowEffect::ManualDoor | RowEffect::TaggedDoor | RowEffect::CloseDoor => SpecialClass::Door,
        RowEffect::Lift => SpecialClass::Lift,
        RowEffect::Teleport => SpecialClass::Teleport,
    }
}

/// The `Doom` family row one `Boom` generalized bit pattern `value`
/// describes: a door, locked door or lift with the trigger its prefix
/// letters carry, `None` for every kind [`doom_row`] does not model and for
/// a locked door no single colour answers.
fn generalized_row(value: u32) -> Option<DoomRow> {
    let decoded = decode_generalized_linedef(value)?;
    let trigger = generalized_trigger(&decoded)?;
    let effect = generalized_effect(&decoded)?;
    let lock = if decoded.kind == GENERALIZED_LOCKED_DOOR {
        let key = decoded.fields.get("key")?;
        Some(generalized_key(key.as_str())?)
    } else {
        None
    };
    Some(DoomRow {
        special: i32::try_from(value).ok()?,
        effect,
        trigger,
        lock,
    })
}

/// The effect the walk models for one decoded generalized pattern: a door
/// that opens (its `kind` field says the ceiling rises), a door that only
/// shuts, or a lift; `None` for the floor, ceiling, stair and crusher
/// kinds the walk leaves unmodelled.
fn generalized_effect(decoded: &Generalized) -> Option<RowEffect> {
    match decoded.kind.as_str() {
        GENERALIZED_DOOR => match decoded.fields.get("kind")?.as_str() {
            "open, wait, close" | "open and stay" => Some(RowEffect::TaggedDoor),
            _ => Some(RowEffect::CloseDoor),
        },
        GENERALIZED_LOCKED_DOOR => Some(RowEffect::TaggedDoor),
        GENERALIZED_LIFT => Some(RowEffect::Lift),
        _ => None,
    }
}

/// The [`Trigger`] one generalized prefix letter pair carries: the walk
/// specials (`W1`/`WR`) reach either side, the press specials (`S1`/`SR`)
/// and the manual doors (`D1`/`DR`) the front with a use, and the gun
/// specials (`G1`/`GR`) the front with a shot.
fn generalized_trigger(decoded: &Generalized) -> Option<Trigger> {
    match decoded.trigger.as_str() {
        "W1" | "WR" => Some(Trigger::Walk),
        "S1" | "SR" | "D1" | "DR" => Some(Trigger::Use),
        "G1" | "GR" => Some(Trigger::Shoot),
        _ => None,
    }
}

/// The colour a generalized lock key names, `None` for the `"any"` and
/// `"all six"` fields: no single key colour answers either, and the walk
/// counts the line unhandled rather than guess it open.
fn generalized_key(key: &str) -> Option<KeyColor> {
    match key {
        "red card" | "red skull" => Some(KeyColor::Red),
        "blue card" | "blue skull" => Some(KeyColor::Blue),
        "yellow card" | "yellow skull" => Some(KeyColor::Yellow),
        _ => None,
    }
}

/// Whether the generalized number `value` decodes to a door or locked door.
fn generalized_door(value: u32) -> bool {
    is_generalized(value)
        && decode_generalized_linedef(value)
            .is_some_and(|row| GENERALIZED_DOOR_KINDS.contains(&row.kind.as_str()))
}

#[cfg(test)]
mod tests;
