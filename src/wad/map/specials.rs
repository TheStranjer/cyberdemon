//! The one family-aware classification of linedef specials.
//!
//! A special number means something different in every flag family: `1` is
//! the `Doom` family's `DR` door and the `Hexen` family's
//! `Polyobj_StartLine`, and `12` is nobody's action in the `Doom` family's
//! table and `Door_Raise` in the `Hexen` one. Every door detection, action
//! walk and exit reading therefore classifies a line through the map's own
//! [`FlagFamily`], never through the session game's definition tables:
//! [`special_class`] answers what one number does, [`is_door_action`] adds
//! the `Doom` family's `Boom` generalized doors to that answer, and
//! [`exit_for`] resolves one line's exit rule with its trigger read off
//! the line itself.
//!
//! The rows are the [`tables`] module: hand-written engine vocabulary,
//! shared with the playability walk
//! ([`crate::wad::map::playability`]) that folds them into a map's feature
//! inventory. The `auto_texture` engine ([`crate::themes::auto`]) takes its
//! door lines from a caller that asks [`is_door_action`] of every linedef;
//! nothing else re-reads the numbers.

pub mod tables;

use crate::defs::boom::{decode_generalized_linedef, is_generalized};
use crate::wad::map::flags::{ACTIVATION_MASK, ACTIVATION_SHIFT, FlagFamily, PLAYER_USE};
use crate::wad::map::model::Linedef;
use crate::wad::map::playability::KeyColor;
use tables::{DOOM_EXITS, DOOM_ROWS, ExitRow, HEXEN_EXITS, HEXEN_ROWS, RowEffect, Trigger};

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

/// The `Doom` family's door kinds among the `Boom` generalized bit
/// patterns, as [`decode_generalized_linedef`] spells them.
const GENERALIZED_DOOR_KINDS: [&str; 2] = ["Door", "Locked Door"];

/// What line special `special` does in flag family `family`, `None` when
/// no table of that family carries it.
///
/// This is the pure table read: the `Doom` family's `Boom` generalized bit
/// patterns are no action here (see [`is_door_action`] for the
/// door-including-everything answer a texture pass wants), and a `Hexen`
/// family exit's trigger rides the line's activation field, so the class
/// names it `None`.
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

/// How a `Hexen` family line is worked: only `playeruse` needs the player on
/// a particular side, every other activation reaches from either side.
#[must_use]
#[inline]
pub fn hexen_trigger(flags: u32) -> Trigger {
    let field = (flags & ACTIVATION_MASK)
        .checked_shr(ACTIVATION_SHIFT)
        .unwrap_or(0_u32);
    if field == PLAYER_USE {
        return Trigger::Use;
    }
    Trigger::Walk
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

/// Whether the generalized number `value` decodes to a door or locked door.
fn generalized_door(value: u32) -> bool {
    is_generalized(value)
        && decode_generalized_linedef(value)
            .is_some_and(|row| GENERALIZED_DOOR_KINDS.contains(&row.kind.as_str()))
}

#[cfg(test)]
mod tests;
