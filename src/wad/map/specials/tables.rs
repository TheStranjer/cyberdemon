//! The hand-written special tables the whole server classifies actions
//! through.
//!
//! Every `Doom` family door, lift and teleport line and every `Hexen`
//! family action is one row here, with the argument slot or key colour it
//! carries spelled out; [`KEY_THINGS`] names the thing types that hold a
//! key. The rows are the engine's own, not a definition table's: no
//! `DeHackEd` rename and no custom `DECORATE` title can move what this
//! classification believes the game does. The playability walk folds them
//! into a map's feature inventory through the lookups in
//! [`super`](crate::wad::map::specials); [`special_class`](super::special_class)
//! is the one classification every door detection and role naming reads.

use crate::wad::map::playability::KeyColor;

/// What a table row says its special does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowEffect {
    /// Raises the ceiling of the sector behind the line: a manual door.
    ManualDoor,
    /// Raises the ceiling of every sector carrying the line's tag.
    TaggedDoor,
    /// Shuts every door the line addresses: a door action the reachability
    /// walk never needs to open, so it is known and ignored in silence.
    CloseDoor,
    /// Drops every sector carrying the line's tag to its lowest neighbour.
    Lift,
    /// Moves the player to a destination sector.
    Teleport,
}

/// How a player sets a line off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A use line: only the sector on its front side works it.
    Use,
    /// A walk line: either side works it.
    Walk,
    /// A shoot or bump line: the `Doom` family's `G1` gun specials and the
    /// `Hexen` family's `impact`, `playerpush` and `missilecross` fields.
    /// The player works it without crossing it - with a shot, a shove, or a
    /// missile sent across - so this model gives its front side only: a
    /// player behind such a line has no documented way to set it off.
    Shoot,
    /// An activation no player sets off: the `Hexen` family's
    /// `monstercross` field, worked only by a monster crossing. No sector
    /// works the line, so a walk over its tables never fires it.
    Monster,
}

/// One row of the `Doom` family's line-special table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DoomRow {
    /// The [`Linedef::special`](crate::wad::map::model::Linedef::special) the row answers for.
    pub special: i32,
    /// What the special does.
    pub effect: RowEffect,
    /// How the player sets it off.
    pub trigger: Trigger,
    /// The key colour the line demands.
    pub lock: Option<KeyColor>,
}

/// A `Doom` row: an unlocked special of `effect` worked `trigger`.
const fn doom(special: i32, effect: RowEffect, trigger: Trigger) -> DoomRow {
    DoomRow {
        special,
        effect,
        trigger,
        lock: None,
    }
}

/// A `Doom` row for a line locked to `lock`.
const fn doom_locked(special: i32, effect: RowEffect, trigger: Trigger, lock: KeyColor) -> DoomRow {
    DoomRow {
        special,
        effect,
        trigger,
        lock: Some(lock),
    }
}

/// One row of the `Doom` family's exit table, reused for the `Hexen` portals
/// whose numbers say whether they are the secret end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitRow {
    /// The [`Linedef::special`](crate::wad::map::model::Linedef::special) that ends the level.
    pub special: i32,
    /// Whether it ends the level through the secret map.
    pub secret: bool,
    /// How the player reaches it.
    pub trigger: Trigger,
}

/// One row of the `Hexen` family's action table.
#[derive(Clone, Copy, Debug)]
pub struct HexenRow {
    /// The [`Linedef::special`](crate::wad::map::model::Linedef::special) the row answers for.
    pub special: i32,
    /// What the special does.
    pub effect: RowEffect,
    /// The `args` slot holding the sector tag, or the destination `TID`.
    pub tag_arg: usize,
    /// The `args` slot holding a lock code, for a lockable action.
    pub lock_arg: Option<usize>,
}

/// A `Hexen` row: an unlocked action addressing `args[tag_arg]`.
const fn hexen(special: i32, effect: RowEffect, tag_arg: usize) -> HexenRow {
    HexenRow {
        special,
        effect,
        tag_arg,
        lock_arg: None,
    }
}

/// A `Hexen` row whose action is locked to the code in `args[lock_arg]`.
const fn hexen_locked(
    special: i32,
    effect: RowEffect,
    tag_arg: usize,
    lock_arg: usize,
) -> HexenRow {
    HexenRow {
        special,
        effect,
        tag_arg,
        lock_arg: Some(lock_arg),
    }
}

/// The `Doom` family's door, lift and teleport specials.
///
/// The `DR`/`D1` rows at the top open the sector behind the line; the rest
/// answer a tag. `W1`/`WR` lines work from either side, `S1`/`SR` lines from
/// the front only, and the `G1` gun specials (line 46, the [`DOOM_EXITS`]
/// 197/198) from the front only as [`Trigger::Shoot`]: the player shoots the
/// face they see, never the side behind. The locked walkovers (99, 133 to
/// 137) are tagged doors whose key the row names. The
/// [`RowEffect::CloseDoor`] rows are the door shutters (`W1`/`S1`/`SR`/`WR`
/// Door Close, and the DOOM II fast pairs that close, or close and reopen):
/// a door action to a texture pass, and known but never opened by a player
/// who wants through. The perpetual plats (87, 89) drop their tagged sector
/// like the down-wait-up ones and stay down.
pub const DOOM_ROWS: &[DoomRow] = &[
    // Press the wall: the sector behind it is the doorway.
    doom(1, RowEffect::ManualDoor, Trigger::Use),
    doom(31, RowEffect::ManualDoor, Trigger::Use),
    doom(117, RowEffect::ManualDoor, Trigger::Use),
    doom(118, RowEffect::ManualDoor, Trigger::Use),
    doom_locked(26, RowEffect::ManualDoor, Trigger::Use, KeyColor::Blue),
    doom_locked(32, RowEffect::ManualDoor, Trigger::Use, KeyColor::Blue),
    doom_locked(27, RowEffect::ManualDoor, Trigger::Use, KeyColor::Yellow),
    doom_locked(34, RowEffect::ManualDoor, Trigger::Use, KeyColor::Yellow),
    doom_locked(28, RowEffect::ManualDoor, Trigger::Use, KeyColor::Red),
    doom_locked(33, RowEffect::ManualDoor, Trigger::Use, KeyColor::Red),
    // Open or raise every sector carrying the line's tag.
    doom(2, RowEffect::TaggedDoor, Trigger::Walk),
    doom(4, RowEffect::TaggedDoor, Trigger::Walk),
    doom(86, RowEffect::TaggedDoor, Trigger::Walk),
    doom(90, RowEffect::TaggedDoor, Trigger::Walk),
    doom(105, RowEffect::TaggedDoor, Trigger::Walk),
    doom(106, RowEffect::TaggedDoor, Trigger::Walk),
    doom(108, RowEffect::TaggedDoor, Trigger::Walk),
    doom(109, RowEffect::TaggedDoor, Trigger::Walk),
    doom(29, RowEffect::TaggedDoor, Trigger::Use),
    doom(61, RowEffect::TaggedDoor, Trigger::Use),
    doom(63, RowEffect::TaggedDoor, Trigger::Use),
    doom(103, RowEffect::TaggedDoor, Trigger::Use),
    doom(111, RowEffect::TaggedDoor, Trigger::Use),
    doom(112, RowEffect::TaggedDoor, Trigger::Use),
    doom(114, RowEffect::TaggedDoor, Trigger::Use),
    doom(115, RowEffect::TaggedDoor, Trigger::Use),
    doom(46, RowEffect::TaggedDoor, Trigger::Shoot),
    doom_locked(99, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Blue),
    doom_locked(133, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Blue),
    doom_locked(134, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Red),
    doom_locked(135, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Red),
    doom_locked(136, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Yellow),
    doom_locked(137, RowEffect::TaggedDoor, Trigger::Walk, KeyColor::Yellow),
    // Shut every tagged door: a door action, never the way through.
    doom(3, RowEffect::CloseDoor, Trigger::Walk),
    doom(16, RowEffect::CloseDoor, Trigger::Walk),
    doom(42, RowEffect::CloseDoor, Trigger::Use),
    doom(50, RowEffect::CloseDoor, Trigger::Use),
    doom(75, RowEffect::CloseDoor, Trigger::Walk),
    doom(76, RowEffect::CloseDoor, Trigger::Walk),
    doom(107, RowEffect::CloseDoor, Trigger::Walk),
    doom(110, RowEffect::CloseDoor, Trigger::Walk),
    doom(113, RowEffect::CloseDoor, Trigger::Use),
    doom(116, RowEffect::CloseDoor, Trigger::Use),
    // Drop every tagged sector to its lowest neighbouring floor.
    doom(10, RowEffect::Lift, Trigger::Walk),
    doom(21, RowEffect::Lift, Trigger::Use),
    doom(62, RowEffect::Lift, Trigger::Use),
    doom(87, RowEffect::Lift, Trigger::Walk),
    doom(88, RowEffect::Lift, Trigger::Walk),
    doom(89, RowEffect::Lift, Trigger::Use),
    doom(120, RowEffect::Lift, Trigger::Walk),
    doom(121, RowEffect::Lift, Trigger::Walk),
    doom(122, RowEffect::Lift, Trigger::Use),
    doom(123, RowEffect::Lift, Trigger::Use),
    // Move the player to the first destination thing in a tagged sector.
    doom(39, RowEffect::Teleport, Trigger::Use),
    doom(97, RowEffect::Teleport, Trigger::Walk),
];

/// The `Doom` family's level ends: the `S1` pair needs a press, the `WR`
/// pair is walked onto, and the `G1` pair 197/198 is shot at - worked from
/// the front side like every other [`Trigger::Shoot`] row.
pub const DOOM_EXITS: &[ExitRow] = &[
    ExitRow {
        special: 11,
        secret: false,
        trigger: Trigger::Use,
    },
    ExitRow {
        special: 51,
        secret: true,
        trigger: Trigger::Use,
    },
    ExitRow {
        special: 52,
        secret: false,
        trigger: Trigger::Walk,
    },
    ExitRow {
        special: 124,
        secret: true,
        trigger: Trigger::Walk,
    },
    ExitRow {
        special: 197,
        secret: false,
        trigger: Trigger::Shoot,
    },
    ExitRow {
        special: 198,
        secret: true,
        trigger: Trigger::Shoot,
    },
];

/// The `Hexen` family's door, lift and teleport actions. `Door_Close` (10)
/// is a [`RowEffect::CloseDoor`] row: a door action to a texture pass, and
/// never a way through for a walk.
pub const HEXEN_ROWS: &[HexenRow] = &[
    hexen(10, RowEffect::CloseDoor, 0),
    hexen(11, RowEffect::TaggedDoor, 0),
    hexen(12, RowEffect::TaggedDoor, 0),
    hexen_locked(13, RowEffect::TaggedDoor, 0, 3),
    hexen_locked(14, RowEffect::TaggedDoor, 0, 3),
    hexen_locked(202, RowEffect::TaggedDoor, 0, 4),
    hexen(60, RowEffect::Lift, 0),
    hexen(62, RowEffect::Lift, 0),
    hexen(63, RowEffect::Lift, 0),
    hexen(64, RowEffect::Lift, 0),
    hexen(65, RowEffect::Lift, 0),
    hexen(172, RowEffect::Lift, 0),
    hexen(206, RowEffect::Lift, 0),
    hexen(207, RowEffect::Lift, 0),
    hexen(70, RowEffect::Teleport, 0),
    hexen(71, RowEffect::Teleport, 0),
];

/// The `Hexen` family's level ends: `Portal`, its instantaneous secret twin,
/// and `Teleport_NewMap`. Each is a `(special, secret)` pair; the side the
/// player reaches it from is the line's own activation field.
pub const HEXEN_EXITS: &[(i32, bool)] = &[(243_i32, false), (244_i32, true), (74_i32, false)];

/// The key items both families spawn: `ZDoom` keeps the `Doom` numbers. A
/// colour's card and skull answer for each other.
pub const KEY_THINGS: &[(i32, KeyColor)] = &[
    (5_i32, KeyColor::Blue),
    (40_i32, KeyColor::Blue),
    (13_i32, KeyColor::Red),
    (38_i32, KeyColor::Red),
    (6_i32, KeyColor::Yellow),
    (39_i32, KeyColor::Yellow),
];
