//! The pure door core: the action a door style installs and the map
//! edits that dress a doorway with it.
//!
//! [`door_specials`] turns a flag family, a [`DoorStyle`] and a tag into
//! the [`DoorLineSetup`] the room-facing use lines carry; [`apply_door`]
//! writes one such setup and its [`DoorTextures`] onto a doorway the
//! caller has already drawn and describes with [`DoorSides`]: the door
//! sector is closed, every use line is faced at the room with the door
//! face on its upper, and the jamb lines become one-sided walls wearing
//! the track. [`next_free_tag`] mints the tag a new door's sector and
//! use lines share.
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

use crate::error::CyberError;

use crate::wad::map::flags::{ACTIVATION_MASK, FlagFamily, MONSTER_ACTIVATE};
use crate::wad::map::model::{Linedef, Map};

/// The `blocking` linedef flag bit: the flags of the one-sided jamb wall.
const BLOCKING_BIT: u32 = 1_u32;
/// The `twosided` linedef flag bit, the same value in both flag families.
const TWO_SIDED_BIT: u32 = 4_u32;
/// The `dontpegtop` (upper-unpegged) linedef bit, value 8 in both tables.
const DONTPEG_TOP_BIT: u32 = 8_u32;
/// The `dontpegbottom` (lower-unpegged) linedef bit, value 16 in both.
const DONTPEG_BOTTOM_BIT: u32 = 16_u32;
/// The model's name for "no texture here" on a [`Sidedef`] texture face.
///
/// [`Sidedef`]: crate::wad::map::Sidedef
const NO_TEXTURE: &str = "-";
/// The `Hexen` `repeatspecial` linedef bit (see the `flags` tables).
const REPEAT_SPECIAL_BIT: u32 = 0x200;
/// The `Hexen` `playeruse` activation: field value 1 at bit 10.
const PLAYER_USE_BITS: u32 = 0x400;
/// The activation bits of a `Hexen` use line: `playeruse` alone.
pub(super) const USE: u32 = PLAYER_USE_BITS;
/// `playeruse` plus `repeatspecial`: the line keeps working after use.
pub(super) const USE_REPEAT: u32 = PLAYER_USE_BITS | REPEAT_SPECIAL_BIT;
/// `playeruse`, `repeatspecial` and `monsteractivate`: monsters, too.
const USE_REPEAT_MONSTER: u32 = PLAYER_USE_BITS | REPEAT_SPECIAL_BIT | MONSTER_ACTIVATE;

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
        DoorStyle::StayOpen => (11_i32, [0_i32, 16_i32, 0_i32, 0_i32, 0_i32], USE),
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

/// A validated room line: the two sides and whether the line must turn
/// around so the room ends up on its geometric front (right of `v1` to
/// `v2`).
#[derive(Clone, Copy, Debug)]
struct RoomPlan {
    /// Index in [`Map::linedefs`] of the line.
    line: usize,
    /// Index in [`Map::sidedefs`] of the side facing the room.
    room_side: usize,
    /// Index in [`Map::sidedefs`] of the side facing the door sector.
    door_side: usize,
    /// Whether the front sidedef is the door's and the line must be
    /// turned around: `v1`/`v2` swap with `front`/`back` so the room's
    /// sidedef keeps the Doom convention (front on the line's right).
    flip: bool,
}

/// A validated jamb line and the door-facing side that stays as front.
#[derive(Clone, Copy, Debug)]
struct JambPlan {
    /// Index in [`Map::linedefs`] of the line.
    line: usize,
    /// Index in [`Map::sidedefs`] of the side facing the door sector.
    door_side: usize,
    /// Whether that side is the back and the line's vertices must swap
    /// with it, keeping the door sector on the geometric front.
    flip: bool,
}

/// Every [`DoorSides`] reference of one [`apply_door`] call, checked and
/// resolved against the map before anything is written.
struct DoorPlan {
    /// The room-facing use lines.
    rooms: Vec<RoomPlan>,
    /// The jamb walls.
    jambs: Vec<JambPlan>,
}

/// The writes of one [`apply_door`] call, bundled so the per-line
/// helpers stay inside the parameter budget.
struct DoorAction<'dress> {
    /// The line action for every room line.
    setup: &'dress DoorLineSetup,
    /// The door face, track and recess floor.
    textures: &'dress DoorTextures,
    /// The map's flag family: only `Hexen` takes activation bits.
    family: FlagFamily,
}

/// Dress a drawn doorway as one undo-free map edit.
///
/// The door sector ([`DoorSides::door_sector`]) gets its ceiling down at
/// its floor — a closed door — and [`DoorTextures::floor_flat`] as its
/// floor flat; its ceiling flat stays. Every room line of
/// [`DoorSides::room_lines`] must be two-sided between a room sector and
/// the door sector: its front side is flipped to face the room (`Doom`
/// use specials fire from the front only), becomes two-sided,
/// non-blocking and top-pegged (the `dontpegtop` bit cleared, so the
/// door face rides the rising ceiling), wears [`DoorTextures::door`] as
/// its room-side `upper` with the door-side `upper` cleared, and takes
/// [`DoorLineSetup::special`], [`DoorLineSetup::tag`],
/// [`DoorLineSetup::args`] and — in the `Hexen` family — the activation
/// bits of [`DoorLineSetup::flags_activation`] (the old activation field
/// is replaced, never merged). A flipped line swaps `v1`/`v2` with the
/// sides, so the room's sidedef keeps the Doom convention of being on
/// the line's right (what the renderer draws and the `Doom` use lines
/// fire from). Every jamb line of
/// [`DoorSides::jamb_lines`] is made one-sided with its front facing the
/// door sector (its vertices swap when the door's side was the back):
/// blocking, lower-unpegged (the `dontpegbottom` bit, which
/// pins the track to the floor) and wearing [`DoorTextures::track`] as
/// its `middle`.
///
/// A refused call leaves `map` untouched.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `sides.door_sector` is not a
/// sector of the map, when a line index is not a linedef, when a line
/// references a sidedef the map does not hold, when a room line is not
/// two-sided or does not separate a room sector from the door sector, or
/// when a jamb line has no side facing the door sector. Build the
/// doorway with [`crate::wad::map::overlay`] first and pass the indices
/// it reported.
#[inline]
pub fn apply_door(
    map: &mut Map,
    sides: &DoorSides,
    setup: &DoorLineSetup,
    textures: &DoorTextures,
) -> Result<(), CyberError> {
    let plan = plan_door(map, sides)?;
    if let Some(sector) = map.sectors.get_mut(sides.door_sector) {
        sector.ceiling_height = sector.floor_height;
        textures.floor_flat.clone_into(&mut sector.floor_flat);
    }
    let action = DoorAction {
        setup,
        textures,
        family: map.flag_family(),
    };
    for room in &plan.rooms {
        dress_room_line(map, room, &action);
    }
    for jamb in &plan.jambs {
        dress_jamb_line(map, *jamb, textures);
    }
    Ok(())
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

/// Check every [`DoorSides`] reference against `map` and resolve each
/// line into its plan, before any mutation.
fn plan_door(map: &Map, sides: &DoorSides) -> Result<DoorPlan, CyberError> {
    if map.sectors.get(sides.door_sector).is_none() {
        return Err(range_error(
            "door sector",
            sides.door_sector,
            map.sectors.len(),
        ));
    }
    let rooms = sides
        .room_lines
        .iter()
        .copied()
        .map(|line| plan_room_line(map, line, sides.door_sector))
        .collect::<Result<Vec<RoomPlan>, CyberError>>()?;
    let jambs = sides
        .jamb_lines
        .iter()
        .copied()
        .map(|line| plan_jamb_line(map, line, sides.door_sector))
        .collect::<Result<Vec<JambPlan>, CyberError>>()?;
    Ok(DoorPlan { rooms, jambs })
}

/// Resolve one room line: both sides exist, exactly one faces the door
/// sector, and the flip needed to face the front at the room is noted.
fn plan_room_line(map: &Map, line: usize, door_sector: usize) -> Result<RoomPlan, CyberError> {
    let linedef = line_at(map, line, "room")?;
    let (front, back) = both_sides(line, linedef)?;
    let at_door = (
        side_sector(map, line, front)? == door_sector,
        side_sector(map, line, back)? == door_sector,
    );
    let (room_side, door_side, flip) = match at_door {
        (false, true) => (front, back, false),
        (true, false) => (back, front, true),
        _ => {
            return Err(CyberError::InvalidArgument(format!(
                "room linedef {line} does not separate a room sector from door sector \
                 {door_sector}: one of its sides must belong to the door sector"
            )));
        }
    };
    Ok(RoomPlan {
        line,
        room_side,
        door_side,
        flip,
    })
}

/// Resolve one jamb line: the side that stays as front, which is the
/// door-facing one of a two-sided line or the front of an already
/// one-sided one, and whether the line must turn around so that side
/// keeps the Doom convention of being on the line's right.
fn plan_jamb_line(map: &Map, line: usize, door_sector: usize) -> Result<JambPlan, CyberError> {
    let linedef = line_at(map, line, "jamb")?;
    match (linedef.front, linedef.back) {
        (Some(front), Some(back)) => {
            let at_door = (
                side_sector(map, line, front)? == door_sector,
                side_sector(map, line, back)? == door_sector,
            );
            let (door_side, flip) = match at_door {
                (true, false) => (front, false),
                (false, true) => (back, true),
                _ => return Err(jamb_not_at_door(line, door_sector)),
            };
            Ok(JambPlan {
                line,
                door_side,
                flip,
            })
        }
        (Some(front), None) => {
            if side_sector(map, line, front)? == door_sector {
                Ok(JambPlan {
                    line,
                    door_side: front,
                    flip: false,
                })
            } else {
                Err(jamb_not_at_door(line, door_sector))
            }
        }
        (None, _) => Err(CyberError::InvalidArgument(format!(
            "jamb linedef {line} has no front sidedef: give it a side over the door sector \
             first (see add_sidedef)"
        ))),
    }
}

/// The linedef `index` of `map`, named by its door role `role`.
fn line_at<'held>(map: &'held Map, index: usize, role: &str) -> Result<&'held Linedef, CyberError> {
    map.linedefs
        .get(index)
        .ok_or_else(|| range_error(&format!("{role} linedef"), index, map.linedefs.len()))
}

/// The front and back sidedef indices of a two-sided line.
fn both_sides(line: usize, linedef: &Linedef) -> Result<(usize, usize), CyberError> {
    match (linedef.front, linedef.back) {
        (Some(front), Some(back)) => Ok((front, back)),
        _ => Err(CyberError::InvalidArgument(format!(
            "room linedef {line} is not two-sided: a door use line needs a front side \
             facing the room and a back side facing the door sector"
        ))),
    }
}

/// The sector behind sidedef `side` of linedef `line`.
fn side_sector(map: &Map, line: usize, side: usize) -> Result<usize, CyberError> {
    map.sidedefs
        .get(side)
        .map(|sidedef| sidedef.sector)
        .ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "linedef {line} references sidedef {side}, which the map does not hold \
                 (it has {} sidedefs)",
                map.sidedefs.len()
            ))
        })
}

/// `InvalidArgument` for a jamb line with no side over the door sector.
fn jamb_not_at_door(line: usize, door_sector: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "jamb linedef {line} does not border door sector {door_sector}: pass the wall \
         lines between the recess and its neighbours"
    ))
}

/// `InvalidArgument` naming the reference `kind`, its out-of-range
/// `index` and how many the map holds.
fn range_error(kind: &str, index: usize, len: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{kind} {index} is out of range: the map holds {len} of them; re-read the map \
         and pass current indices"
    ))
}

/// Write the door action onto one room line and its door face onto the
/// room side (the indices were checked by [`plan_door`], so a row that
/// has since vanished would be the only reason to skip it).
fn dress_room_line(map: &mut Map, room: &RoomPlan, action: &DoorAction<'_>) {
    if let Some(linedef) = map.linedefs.get_mut(room.line) {
        if room.flip {
            core::mem::swap(&mut linedef.front, &mut linedef.back);
            core::mem::swap(&mut linedef.v1, &mut linedef.v2);
        }
        linedef.special = action.setup.special;
        linedef.tag = action.setup.tag;
        linedef.args = action.setup.args;
        linedef.flags = (linedef.flags | TWO_SIDED_BIT) & !(BLOCKING_BIT | DONTPEG_TOP_BIT);
        if action.family == FlagFamily::Hexen {
            linedef.flags = (linedef.flags & !ACTIVATION_MASK) | action.setup.flags_activation;
        }
    }
    if let Some(side) = map.sidedefs.get_mut(room.room_side) {
        action.textures.door.clone_into(&mut side.upper);
    }
    if let Some(side) = map.sidedefs.get_mut(room.door_side) {
        NO_TEXTURE.clone_into(&mut side.upper);
    }
}

/// Turn one jamb line into a one-sided, floor-pinned track wall (the
/// indices were checked by [`plan_door`]).
fn dress_jamb_line(map: &mut Map, jamb: JambPlan, textures: &DoorTextures) {
    if let Some(linedef) = map.linedefs.get_mut(jamb.line) {
        if jamb.flip {
            core::mem::swap(&mut linedef.v1, &mut linedef.v2);
        }
        linedef.front = Some(jamb.door_side);
        linedef.back = None;
        linedef.flags = (linedef.flags | BLOCKING_BIT | DONTPEG_BOTTOM_BIT) & !TWO_SIDED_BIT;
    }
    if let Some(side) = map.sidedefs.get_mut(jamb.door_side) {
        textures.track.clone_into(&mut side.middle);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DoorLineSetup, DoorSides, DoorStyle, DoorTextures, apply_door, door_specials, next_free_tag,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Vertex};

    /// The tag the table tests ask `door_specials` for.
    const SAMPLE_TAG: i32 = 42;

    /// A sidedef over `sector` with the model's empty-face names.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            upper: "-".to_owned(),
            lower: "-".to_owned(),
            middle: "-".to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// A two-sided line between sidedefs `front` and `back`, running
    /// from vertex 0 to vertex 1.
    fn two_sided(front: usize, back: usize) -> Linedef {
        Linedef {
            v1: 0_usize,
            v2: 1_usize,
            front: Some(front),
            back: Some(back),
            flags: 4_u32,
            ..Default::default()
        }
    }

    /// A blocking one-sided line with front sidedef `front`, running
    /// from vertex 0 to vertex 1.
    fn one_sided(front: usize) -> Linedef {
        Linedef {
            v1: 0_usize,
            v2: 1_usize,
            front: Some(front),
            flags: 1_u32,
            ..Default::default()
        }
    }

    /// A sector at the given heights, with the named flats and tag.
    fn sector(floor_height: i32, ceiling_height: i32, tag: i32) -> Sector {
        Sector {
            floor_height,
            ceiling_height,
            floor_flat: "FLOOR1".to_owned(),
            ceiling_flat: "CEIL1".to_owned(),
            light: 176_i32,
            tag,
            ..Default::default()
        }
    }

    /// A doorway: room sector 0, door sector 1, and
    /// - linedef 0: two-sided, front (sidedef 0) facing the room — correct;
    /// - linedef 1: two-sided, front (sidedef 2) facing the door — backwards;
    /// - linedef 2: two-sided jamb, front (sidedef 4) room, back (5) door;
    /// - linedef 3: jamb already one-sided with front (sidedef 6) over the door;
    /// - linedef 4: a plain wall of the room, not part of the door.
    ///
    /// Lines 0 and 1 start top-unpegged (bit 8) to check it is cleared.
    fn doorway(format: MapFormat) -> Map {
        Map {
            format,
            sectors: vec![sector(0_i32, 128_i32, 0_i32), sector(0_i32, 128_i32, 0_i32)],
            vertices: vec![Vertex::default(), Vertex::default()],
            linedefs: vec![
                Linedef {
                    flags: 12_u32,
                    ..two_sided(0_usize, 1_usize)
                },
                Linedef {
                    flags: 12_u32,
                    ..two_sided(2_usize, 3_usize)
                },
                two_sided(4_usize, 5_usize),
                one_sided(6_usize),
                one_sided(7_usize),
            ],
            sidedefs: vec![
                side(0_usize), // 0: line 0 front, room
                side(1_usize), // 1: line 0 back, door
                side(1_usize), // 2: line 1 front, door
                side(0_usize), // 3: line 1 back, room
                side(0_usize), // 4: line 2 front, room
                side(1_usize), // 5: line 2 back, door
                side(1_usize), // 6: line 3 front, door
                side(0_usize), // 7: line 4 front, room wall
            ],
            ..Map::default()
        }
    }

    /// The doorway indices of [`doorway`].
    fn sides() -> DoorSides {
        DoorSides {
            door_sector: 1_usize,
            room_lines: vec![0_usize, 1_usize],
            jamb_lines: vec![2_usize, 3_usize],
        }
    }

    /// Door textures with distinctive names.
    fn textures() -> DoorTextures {
        DoorTextures {
            door: "DOOR7".to_owned(),
            track: "DOORTRAK".to_owned(),
            floor_flat: "FLOOR6_8".to_owned(),
        }
    }

    /// The `Doom` use-door action for the doorway tests.
    fn doom_setup() -> DoorLineSetup {
        door_specials(FlagFamily::Doom, DoorStyle::Normal, SAMPLE_TAG)
    }

    /// Whether `error` is the `InvalidArgument` mentioning `needle`.
    fn refuses(error: CyberError, needle: &str) -> bool {
        let CyberError::InvalidArgument(message) = error else {
            panic!("not an InvalidArgument: {error:?}");
        };
        message.contains(needle)
    }

    #[test]
    fn door_specials_matches_the_doom_table() {
        let expected = [
            (DoorStyle::Normal, 1_i32),
            (DoorStyle::Fast, 117_i32),
            (DoorStyle::StayOpen, 31_i32),
            (DoorStyle::LockedRed, 28_i32),
            (DoorStyle::LockedBlue, 26_i32),
            (DoorStyle::LockedYellow, 27_i32),
            (DoorStyle::Remote, 0_i32),
        ];
        assert_eq!(
            expected.len(),
            DoorStyle::all().len(),
            "the table covers every style"
        );
        for (style, special) in expected {
            assert_eq!(
                door_specials(FlagFamily::Doom, style, SAMPLE_TAG),
                DoorLineSetup {
                    special,
                    tag: SAMPLE_TAG,
                    args: [0_i32; 5],
                    flags_activation: 0_u32,
                },
                "the Doom row of {style:?}"
            );
        }
    }

    #[test]
    fn door_specials_matches_the_hexen_table() {
        let expected = [
            (
                DoorStyle::Normal,
                12_i32,
                [0_i32, 16_i32, 150_i32, 0_i32, 0_i32],
                0x2600_u32,
            ),
            (
                DoorStyle::Fast,
                12_i32,
                [0_i32, 64_i32, 150_i32, 0_i32, 0_i32],
                0x2600_u32,
            ),
            (
                DoorStyle::StayOpen,
                11_i32,
                [0_i32, 16_i32, 0_i32, 0_i32, 0_i32],
                0x400_u32,
            ),
            (
                DoorStyle::LockedRed,
                13_i32,
                [0_i32, 16_i32, 150_i32, 129_i32, 0_i32],
                0x600_u32,
            ),
            (
                DoorStyle::LockedBlue,
                13_i32,
                [0_i32, 16_i32, 150_i32, 130_i32, 0_i32],
                0x600_u32,
            ),
            (
                DoorStyle::LockedYellow,
                13_i32,
                [0_i32, 16_i32, 150_i32, 131_i32, 0_i32],
                0x600_u32,
            ),
            (DoorStyle::Remote, 0_i32, [0_i32; 5], 0_u32),
        ];
        for (style, special, args, activation) in expected {
            assert_eq!(
                door_specials(FlagFamily::Hexen, style, SAMPLE_TAG),
                DoorLineSetup {
                    special,
                    tag: 0_i32,
                    args,
                    flags_activation: activation,
                },
                "the Hexen row of {style:?}: use 0x400, repeat 0x200, monster 0x2000"
            );
        }
    }

    #[test]
    fn apply_door_closes_the_recess_and_dresses_the_room_lines() {
        let mut map = doorway(MapFormat::Doom);
        apply_door(&mut map, &sides(), &doom_setup(), &textures()).expect("a sound doorway");
        let door = map.sectors.get(1).expect("door sector");
        assert_eq!(door.ceiling_height, door.floor_height, "the door is closed");
        assert_eq!(
            door.floor_flat, "FLOOR6_8",
            "the recess floor is re-dressed"
        );
        assert_eq!(door.ceiling_flat, "CEIL1", "the ceiling flat stays");
        let untouched = map.sectors.first().expect("room sector");
        assert_eq!(untouched.ceiling_height, 128_i32, "the room is not closed");
        for index in [0_usize, 1] {
            let line = map.linedefs.get(index).expect("room line");
            assert_eq!(line.special, 1_i32, "the DR action of linedef {index}");
            assert_eq!(line.tag, SAMPLE_TAG, "the shared tag of linedef {index}");
            assert_eq!(line.args, [0_i32; 5], "no arguments of linedef {index}");
            assert_eq!(line.flags, 4_u32, "two-sided, unblocked and top-pegged");
        }
        assert_eq!(
            map.linedefs.first().expect("line 0").front,
            Some(0_usize),
            "the correct front stays the front"
        );
        assert_eq!(
            (
                map.linedefs.first().expect("line 0").v1,
                map.linedefs.first().expect("line 0").v2
            ),
            (0_usize, 1_usize),
            "an unflipped line keeps its vertices"
        );
        assert_eq!(
            map.linedefs.get(1).expect("line 1").front,
            Some(3_usize),
            "the backwards line is flipped to face the room"
        );
        assert_eq!(
            (
                map.linedefs.get(1).expect("line 1").v1,
                map.linedefs.get(1).expect("line 1").v2
            ),
            (1_usize, 0_usize),
            "the flip turns the line around too, so the room sidedef keeps the Doom \
             convention of sitting right of v1 to v2"
        );
        assert_eq!(map.sidedefs.first().expect("side").upper, "DOOR7");
        assert_eq!(map.sidedefs.get(3).expect("side").upper, "DOOR7");
        assert_eq!(
            map.sidedefs.get(1).expect("side").upper,
            "-",
            "the door-side upper clears"
        );
        assert_eq!(map.sidedefs.get(2).expect("side").upper, "-");
        assert_eq!(
            map.sidedefs.get(7).expect("side").upper,
            "-",
            "the plain wall is untouched"
        );
        assert_eq!(map.linedefs.get(4).expect("plain wall").special, 0_i32);
    }

    #[test]
    fn apply_door_turns_the_jambs_into_one_sided_tracks() {
        let mut map = doorway(MapFormat::Doom);
        apply_door(&mut map, &sides(), &doom_setup(), &textures()).expect("a sound doorway");
        let swinging = map.linedefs.get(2).expect("jamb 2");
        assert_eq!(
            swinging.front,
            Some(5_usize),
            "the door-facing side becomes the front"
        );
        assert_eq!(swinging.back, None, "the room side is dropped");
        assert_eq!(
            (swinging.v1, swinging.v2),
            (1_usize, 0_usize),
            "the door side was the back, so the jamb turns around with it"
        );
        assert_eq!(
            swinging.flags,
            1_u32 | 16_u32,
            "one-sided blocking and lower-unpegged"
        );
        assert_eq!(map.sidedefs.get(5).expect("side").middle, "DOORTRAK");
        let standing = map.linedefs.get(3).expect("jamb 3");
        assert_eq!(
            standing.front,
            Some(6_usize),
            "an already correct jamb keeps its front"
        );
        assert_eq!(standing.back, None);
        assert_eq!(standing.flags, 1_u32 | 16_u32);
        assert_eq!(map.sidedefs.get(6).expect("side").middle, "DOORTRAK");
    }

    #[test]
    fn apply_door_writes_the_hexen_activation_over_the_old_field() {
        let mut map = doorway(MapFormat::Hexen);
        map.linedefs.first_mut().expect("room line").flags = 4_u32 | 0x1000_u32;
        let setup = door_specials(FlagFamily::Hexen, DoorStyle::Normal, SAMPLE_TAG);
        apply_door(&mut map, &sides(), &setup, &textures()).expect("a sound doorway");
        let line = map.linedefs.first().expect("room line");
        assert_eq!(line.special, 12_i32, "Door_Raise");
        assert_eq!(
            line.args,
            [0_i32, 16_i32, 150_i32, 0_i32, 0_i32],
            "speed and delay"
        );
        assert_eq!(line.tag, 0_i32, "a Hexen line carries no tag");
        assert_eq!(
            line.flags,
            4_u32 | 0x2600_u32,
            "the playerpush field is replaced by use, repeat and monsteractivate"
        );
    }

    #[test]
    fn locked_styles_name_their_coloured_track_role() {
        assert_eq!(DoorStyle::LockedRed.track_role_name(), "door_track_red");
        assert_eq!(DoorStyle::LockedBlue.track_role_name(), "door_track_blue");
        assert_eq!(
            DoorStyle::LockedYellow.track_role_name(),
            "door_track_yellow"
        );
        for style in [
            DoorStyle::Normal,
            DoorStyle::Fast,
            DoorStyle::StayOpen,
            DoorStyle::Remote,
        ] {
            assert_eq!(style.track_role_name(), "door_track", "{style:?}");
        }
    }

    #[test]
    fn apply_door_refuses_indices_the_map_does_not_hold() {
        let outside = |door_sides: DoorSides| {
            let mut map = doorway(MapFormat::Doom);
            let before = map.clone();
            let error = apply_door(&mut map, &door_sides, &doom_setup(), &textures())
                .expect_err("the index is out of range");
            assert_eq!(map, before, "a refused call leaves the map alone");
            error
        };
        let mut missing_sector = sides();
        missing_sector.door_sector = 9_usize;
        assert!(refuses(outside(missing_sector), "sector 9"), "the sector");
        let mut missing_line = sides();
        missing_line.room_lines = vec![0_usize, 77_usize];
        assert!(
            refuses(outside(missing_line), "room linedef 77"),
            "the room line"
        );
        let mut missing_jamb = sides();
        missing_jamb.jamb_lines = vec![88_usize];
        assert!(
            refuses(outside(missing_jamb), "jamb linedef 88"),
            "the jamb line"
        );
    }

    #[test]
    fn apply_door_refuses_lines_that_are_not_doorways() {
        let mut map = doorway(MapFormat::Doom);
        let before = map.clone();
        let mut one_sided_room = sides();
        one_sided_room.room_lines = vec![4_usize];
        let first_error = apply_door(&mut map, &one_sided_room, &doom_setup(), &textures())
            .expect_err("a one-sided line is no use line");
        assert!(refuses(first_error, "not two-sided"), "the hint says why");

        let mut plain_jamb = sides();
        plain_jamb.jamb_lines = vec![4_usize];
        let second_error = apply_door(&mut map, &plain_jamb, &doom_setup(), &textures())
            .expect_err("the wall does not border the recess");
        assert!(refuses(second_error, "jamb linedef 4"), "the jamb is named");
        assert_eq!(map, before, "both refused calls leave the map alone");

        let mut dangling = map.linedefs.remove(0_usize);
        dangling.front = Some(99_usize);
        map.linedefs.insert(0_usize, dangling);
        let third_error = apply_door(&mut map, &sides(), &doom_setup(), &textures())
            .expect_err("the sidedef reference dangles");
        assert!(
            refuses(third_error, "sidedef 99"),
            "the missing side is named"
        );
    }

    #[test]
    fn next_free_tag_starts_at_one_on_an_empty_map() {
        assert_eq!(
            next_free_tag(&Map::default()),
            1_i32,
            "nothing carries a tag yet"
        );
    }

    #[test]
    fn next_free_tag_counts_sectors_and_doom_line_tags_of_special_lines() {
        let map = Map {
            sectors: vec![sector(0_i32, 128_i32, 3_i32), sector(0_i32, 128_i32, 9_i32)],
            linedefs: vec![
                Linedef {
                    special: 1_i32,
                    tag: 7_i32,
                    ..two_sided(0_usize, 1_usize)
                },
                Linedef {
                    tag: 100_i32,
                    ..one_sided(0_usize)
                },
            ],
            ..Map::default()
        };
        assert_eq!(
            next_free_tag(&map),
            10_i32,
            "the special-carrying line tag 7 and the sector tags count, the \
             plain line's 100 does not"
        );
    }

    #[test]
    fn next_free_tag_counts_hexen_arguments_and_saturates_at_the_limit() {
        let hexen = Map {
            format: MapFormat::Hexen,
            sectors: vec![sector(0_i32, 128_i32, 2_i32)],
            linedefs: vec![
                Linedef {
                    special: 12_i32,
                    args: [6_i32, 16_i32, 150_i32, 0_i32, 0_i32],
                    ..two_sided(0_usize, 1_usize)
                },
                Linedef {
                    args: [50_i32, 0_i32, 0_i32, 0_i32, 0_i32],
                    ..one_sided(0_usize)
                },
            ],
            ..Map::default()
        };
        assert_eq!(
            next_free_tag(&hexen),
            7_i32,
            "args[0] of the tagged special counts, the plain line's does not"
        );
        let full = Map {
            sectors: vec![sector(0_i32, 128_i32, i32::MAX)],
            ..Map::default()
        };
        assert_eq!(
            next_free_tag(&full),
            i32::MAX,
            "nothing is above the maximum"
        );
    }
}
