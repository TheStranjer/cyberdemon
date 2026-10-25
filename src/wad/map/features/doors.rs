//! The pure door core: the action a door style installs and the map
//! edits that dress a doorway with it.
//!
//! [`door_specials`] turns a flag family, a [`DoorStyle`] and a tag into
//! the [`DoorLineSetup`] the room-facing use lines carry; [`apply_door`]
//! writes one such setup and its [`DoorTextures`] onto a doorway the
//! caller has already drawn and describes with [`DoorSides`]: the door
//! sector is closed, every use line is faced at the room with the door
//! face on its upper, and the jamb lines become one-sided walls wearing
//! the track. [`face_room_line`] is that room-line treatment on its
//! own — the facing flip and the action write [`apply_door`]'s loop
//! runs — for callers (the `plan` feature engine) whose doorway is
//! already textured. [`next_free_tag`] mints the tag a new door's sector
//! and use lines share.
//!
//! Door-sector addressing follows each family's convention: `Doom` use
//! lines carry the tag and the door sector answers with the same one,
//! while `Hexen` lines carry no tag at all (binary `Hexen` linedefs have
//! no such field) and reach the door through `args[0] = 0`, the sector
//! behind the line. Nothing here reads a texture theme:
//! [`DoorStyle::track_role_name`] hands out the theme role *name* of the
//! track so the caller resolves the texture. Like the sibling engines in
//! [`crate::wad::map::ops`] and [`crate::wad::map::overlay`], a refused
//! [`apply_door`] leaves the map untouched: every index and adjacency
//! check runs before any row is written.

mod apply;

pub use apply::{apply_door, face_room_line};

use crate::wad::map::flags::{FlagFamily, MONSTER_ACTIVATE, PLAYER_USE_BITS, REPEAT_SPECIAL};
use crate::wad::map::model::Map;

/// The model's name for "no texture here" on a [`Sidedef`] texture face,
/// shared from [`consts`](crate::wad::map::consts).
///
/// [`Sidedef`]: crate::wad::map::Sidedef
pub(super) use crate::wad::map::consts::NO_TEXTURE;
/// `playeruse` plus `repeatspecial`: the line keeps working after use.
pub(super) const USE_REPEAT: u32 = PLAYER_USE_BITS | REPEAT_SPECIAL;
/// `playeruse`, `repeatspecial` and `monsteractivate`: monsters, too.
const USE_REPEAT_MONSTER: u32 = PLAYER_USE_BITS | REPEAT_SPECIAL | MONSTER_ACTIVATE;

/// The way a door is meant to be opened, as the room-facing lines see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorStyle {
    /// The plain use door: it opens, waits and closes again.
    Normal,
    /// The plain use door at double speed.
    Fast,
    /// A use door that opens once and stays open (the `D1`/`Door_Open`
    /// family).
    StayOpen,
    /// A door that opens only for the red key.
    LockedRed,
    /// A door that opens only for the blue key.
    LockedBlue,
    /// A door that opens only for the yellow key.
    LockedYellow,
    /// A door with no action on its own lines, raised by a remote
    /// trigger (or an `ACS` script): the door sector carries the tag.
    Remote,
}

impl DoorStyle {
    /// Every style, in table order.
    #[must_use]
    #[inline]
    pub const fn all() -> [Self; 7] {
        [
            Self::Normal,
            Self::Fast,
            Self::StayOpen,
            Self::LockedRed,
            Self::LockedBlue,
            Self::LockedYellow,
            Self::Remote,
        ]
    }

    /// The texture-theme role name of the track this style's jambs
    /// wear: the coloured tracks for the locked doors, the plain one
    /// for every other style. The strings match the theme tables; the
    /// `themes` module is not a dependency of `wad`, so the caller
    /// resolves the name to a texture.
    #[must_use]
    #[inline]
    pub const fn track_role_name(self) -> &'static str {
        match self {
            Self::LockedRed => "door_track_red",
            Self::LockedBlue => "door_track_blue",
            Self::LockedYellow => "door_track_yellow",
            Self::Normal | Self::Fast | Self::StayOpen | Self::Remote => "door_track",
        }
    }
}

/// What [`apply_door`] writes into [`Linedef::special`], [`Linedef::tag`],
/// [`Linedef::args`] and the `Hexen` activation bits of [`Linedef::flags`]
/// of every room-facing door line.
///
/// [`Linedef::args`]: crate::wad::map::Linedef::args
/// [`Linedef::flags`]: crate::wad::map::Linedef::flags
/// [`Linedef::special`]: crate::wad::map::Linedef::special
/// [`Linedef::tag`]: crate::wad::map::Linedef::tag
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DoorLineSetup {
    /// The line special number; 0 for a [`DoorStyle::Remote`] door.
    pub special: i32,
    /// The trigger tag the door sector answers with; 0 in the `Hexen`
    /// family, whose linedefs carry no tag.
    pub tag: i32,
    /// The script arguments; all zero in the `Doom` family.
    pub args: [i32; 5],
    /// The linedef flag bits the `Hexen` family merges into `flags`:
    /// the activation field, `repeatspecial` and `monsteractivate`.
    /// Zero in the `Doom` family.
    pub flags_activation: u32,
}

/// The doorway [`apply_door`] dresses: the already-drawn sector and
/// lines, named by index into the [`Map`] rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoorSides {
    /// The sector the door slides in: the recess between the rooms.
    pub door_sector: usize,
    /// The two-sided lines between a room and the door sector that carry
    /// the action: one per room the door connects.
    pub room_lines: Vec<usize>,
    /// The lines framing the recess, which become the one-sided track
    /// walls beside the door face.
    pub jamb_lines: Vec<usize>,
}

/// The textures [`apply_door`] installs, resolved by the caller (usually
/// from the session's theme; [`DoorStyle::track_role_name`] names the
/// role of the track).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoorTextures {
    /// The moving door face: the `upper` texture of every room-facing
    /// side of a [`DoorSides::room_lines`] line.
    pub door: String,
    /// The jamb track: the `middle` texture of the door-facing side of
    /// every [`DoorSides::jamb_lines`] line.
    pub track: String,
    /// The recess floor: the `floor_flat` of [`DoorSides::door_sector`].
    pub floor_flat: String,
}

/// The [`DoorLineSetup`] a door of `style` carries in flag family
/// `family`, for the shared tag `tag`.
///
/// The table:
///
/// | Style | `Doom` | `Hexen` |
/// |---|---|---|
/// | [`Normal`] | `1` (`DR` door) | `12` `Door_Raise`, args `[0, 16, 150]` |
/// | [`Fast`] | `117` (`DR` fast) | `12` `Door_Raise`, args `[0, 64, 150]` |
/// | [`StayOpen`] | `31` (`D1`, stays open) | `11` `Door_Open`, args `[0, 16]` |
/// | [`LockedRed`] | `28` (`DR` red key) | `13` `Door_LockedRaise`, args `[0, 16, 150, 129]` |
/// | [`LockedBlue`] | `26` (`DR` blue key) | `13` `Door_LockedRaise`, args `[0, 16, 150, 130]` |
/// | [`LockedYellow`] | `27` (`DR` yellow key) | `13` `Door_LockedRaise`, args `[0, 16, 150, 131]` |
/// | [`Remote`] | `0` | `0` |
///
/// `Doom` setups carry `tag` in [`DoorLineSetup::tag`] (the door sector
/// gets the same tag, so the use line finds the recess; tag 0 leaves the
/// door acting on the sector behind the line); their args and activation
/// stay zero. `Hexen` setups carry no line tag at all: `args[0] = 0`
/// targets the sector behind the line, and
/// [`DoorLineSetup::flags_activation`] holds `playeruse` for every local
/// style, plus `repeatspecial` for [`Fast`] and the locked styles, plus
/// [`monsteractivate`] for [`Normal`] and [`Fast`]. [`Remote`] installs
/// nothing on the door's own lines in either family: its trigger is a
/// separate `S1`/`W1` (or script-activated) line elsewhere, and the door
/// sector carries `tag` for it.
///
/// [`Normal`]: DoorStyle::Normal
/// [`Fast`]: DoorStyle::Fast
/// [`StayOpen`]: DoorStyle::StayOpen
/// [`LockedRed`]: DoorStyle::LockedRed
/// [`LockedBlue`]: DoorStyle::LockedBlue
/// [`LockedYellow`]: DoorStyle::LockedYellow
/// [`Remote`]: DoorStyle::Remote
/// [`monsteractivate`]: crate::wad::map::flags
#[must_use]
#[inline]
pub const fn door_specials(family: FlagFamily, style: DoorStyle, tag: i32) -> DoorLineSetup {
    match family {
        FlagFamily::Doom => doom_door(style, tag),
        FlagFamily::Hexen => hexen_door(style),
    }
}

/// The `Doom`-family row of [`door_specials`]: a vanilla special, the
/// caller's tag, no arguments and no activation bits.
const fn doom_door(style: DoorStyle, tag: i32) -> DoorLineSetup {
    let special = match style {
        DoorStyle::Normal => 1_i32,
        DoorStyle::Fast => 117_i32,
        DoorStyle::StayOpen => 31_i32,
        DoorStyle::LockedRed => 28_i32,
        DoorStyle::LockedBlue => 26_i32,
        DoorStyle::LockedYellow => 27_i32,
        DoorStyle::Remote => 0_i32,
    };
    DoorLineSetup {
        special,
        tag,
        args: [0_i32; 5],
        flags_activation: 0_u32,
    }
}

/// The `Hexen`-family row of [`door_specials`]: a `Door_*` script
/// special targeting the sector behind the line (`args[0] = 0`), with
/// the activation bits the style's trigger needs and no line tag.
const fn hexen_door(style: DoorStyle) -> DoorLineSetup {
    let (special, args, flags_activation) = match style {
        DoorStyle::Normal => (
            12_i32,
            [0_i32, 16_i32, 150_i32, 0_i32, 0_i32],
            USE_REPEAT_MONSTER,
        ),
        DoorStyle::Fast => (
            12_i32,
            [0_i32, 64_i32, 150_i32, 0_i32, 0_i32],
            USE_REPEAT_MONSTER,
        ),
        DoorStyle::StayOpen => (
            11_i32,
            [0_i32, 16_i32, 0_i32, 0_i32, 0_i32],
            PLAYER_USE_BITS,
        ),
        DoorStyle::LockedRed => (13_i32, [0_i32, 16_i32, 150_i32, 129_i32, 0_i32], USE_REPEAT),
        DoorStyle::LockedBlue => (13_i32, [0_i32, 16_i32, 150_i32, 130_i32, 0_i32], USE_REPEAT),
        DoorStyle::LockedYellow => (13_i32, [0_i32, 16_i32, 150_i32, 131_i32, 0_i32], USE_REPEAT),
        DoorStyle::Remote => (0_i32, [0_i32; 5], 0_u32),
    };
    DoorLineSetup {
        special,
        tag: 0_i32,
        args,
        flags_activation,
    }
}

/// The next unused sector tag: one above every tag the map already
/// carries, so a new door's sector and lines cannot collide with an
/// existing one.
///
/// An empty map starts at `1`. Counted: every [`Sector::tag`], every
/// [`Linedef::tag`] of a linedef carrying a special (the `Doom` way),
/// and, in the `Hexen` family, every `args[0]` of a linedef carrying a
/// special (the `Door_*`-style tag argument). Negative tags are left
/// where they are; when the map already carries `i32::MAX` there is no
/// free tag above it and `i32::MAX` is reported unchanged.
///
/// [`Linedef::tag`]: crate::wad::map::Linedef::tag
/// [`Sector::tag`]: crate::wad::map::Sector::tag
#[must_use]
#[inline]
pub fn next_free_tag(map: &Map) -> i32 {
    let hexen = map.flag_family() == FlagFamily::Hexen;
    let mut highest = 0_i32;
    for sector in &map.sectors {
        highest = highest.max(sector.tag);
    }
    for line in &map.linedefs {
        if line.special == 0_i32 {
            continue;
        }
        highest = highest.max(line.tag);
        if hexen {
            let [arg0, ..] = line.args;
            highest = highest.max(arg0);
        }
    }
    highest.saturating_add(1)
}

#[cfg(test)]
mod tests;
