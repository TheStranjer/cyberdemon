//! The doorway dressing side of the door core: the plan that checks every
//! [`DoorSides`] reference against the map and the writes that follow it.
//!
//! [`apply_door`] runs [`plan_door`] first, so each line is resolved into a
//! [`RoomPlan`] or a [`JambPlan`] — and every refusal raised — before a
//! single row changes; the dressings themselves are [`dress_room_line`] and
//! [`dress_jamb_line`], the first of the two running on
//! [`face_room_line`], the room-line treatment callers whose doorway is
//! already textured use on its own. The shapes these writes carry
//! ([`DoorLineSetup`], [`DoorSides`] and [`DoorTextures`]), the `DoorStyle`
//! action table behind them and the `door_specials` lookup that fills a
//! setup live in [`super`].

use crate::error::CyberError;
use crate::wad::map::flags::{
    ACTIVATION_MASK, BLOCKING, DONT_PEG_BOTTOM, DONT_PEG_TOP, FlagFamily, TWO_SIDED,
};
use crate::wad::map::model::{Linedef, Map};

use super::{DoorLineSetup, DoorSides, DoorTextures, NO_TEXTURE};

/// A validated room line: its two sides, room side first.
/// [`face_room_line`] decides at write time whether the line must turn
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
    /// The sector the door slides in: every room line separates it from
    /// a room.
    door_sector: usize,
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
        door_sector: sides.door_sector,
    };
    for room in &plan.rooms {
        dress_room_line(map, room, &action);
    }
    for jamb in &plan.jambs {
        dress_jamb_line(map, *jamb, textures);
    }
    Ok(())
}

/// Write a door's [`DoorLineSetup`] onto the two-sided room line `line`,
/// turning the line around first when its front faces the door sector
/// `door_sector`.
///
/// The flip swaps `v1`/`v2` with `front`/`back`, so the room's sidedef
/// keeps the `Doom` convention of sitting right of `v1` to `v2` (what
/// the renderer draws and the `Doom` use specials fire from); a line
/// already facing the room keeps its vertices. The line then takes
/// [`DoorLineSetup::special`], [`DoorLineSetup::tag`] and
/// [`DoorLineSetup::args`], comes out two-sided, non-blocking and
/// top-pegged (the `dontpegtop` bit cleared so a rising door face rides
/// the ceiling), and — in the `Hexen` family — its old activation field
/// is replaced by [`DoorLineSetup::flags_activation`], never merged.
///
/// This is [`apply_door`]'s room-line treatment in one place, for
/// callers whose doorway is already dressed: the `plan` feature engine
/// ([`crate::plan::features`]) wires the door lines a compiled plan
/// reports without touching their textures. `line` must be a two-sided
/// linedef separating a room sector from `door_sector`; [`apply_door`]
/// checks its own references before writing, so a missing row can only
/// come from a caller that skipped its validation — it is left
/// untouched.
#[inline]
pub fn face_room_line(map: &mut Map, line: usize, door_sector: usize, setup: &DoorLineSetup) {
    let hexen = map.flag_family() == FlagFamily::Hexen;
    let door_on_front = front_side_of(map, line).is_some_and(|front| {
        map.sidedefs
            .get(front)
            .is_some_and(|side| side.sector == door_sector)
    });
    let Some(linedef) = map.linedefs.get_mut(line) else {
        return;
    };
    if door_on_front {
        core::mem::swap(&mut linedef.front, &mut linedef.back);
        core::mem::swap(&mut linedef.v1, &mut linedef.v2);
    }
    linedef.special = setup.special;
    linedef.tag = setup.tag;
    linedef.args = setup.args;
    linedef.flags = (linedef.flags | TWO_SIDED) & !(BLOCKING | DONT_PEG_TOP);
    if hexen {
        linedef.flags = (linedef.flags & !ACTIVATION_MASK) | setup.flags_activation;
    }
}

/// The [`Linedef::front`] sidedef index of the `index`th line, `None`
/// when the line or its front is missing.
///
/// [`Linedef::front`]: crate::wad::map::Linedef::front
fn front_side_of(map: &Map, index: usize) -> Option<usize> {
    map.linedefs.get(index).and_then(|linedef| linedef.front)
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
/// sector, and the two sides are noted room-first. [`face_room_line`]
/// turns the line around at write time when the front is the door's.
fn plan_room_line(map: &Map, line: usize, door_sector: usize) -> Result<RoomPlan, CyberError> {
    let linedef = line_at(map, line, "room")?;
    let (front, back) = both_sides(line, linedef)?;
    let at_door = (
        map.required_side_sector(line, front)? == door_sector,
        map.required_side_sector(line, back)? == door_sector,
    );
    let (room_side, door_side) = match at_door {
        (false, true) => (front, back),
        (true, false) => (back, front),
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
                map.required_side_sector(line, front)? == door_sector,
                map.required_side_sector(line, back)? == door_sector,
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
            if map.required_side_sector(line, front)? == door_sector {
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

/// Write the door action onto one room line through [`face_room_line`]
/// and its door face onto the room side (the indices were checked by
/// [`plan_door`], so a row that has since vanished would be the only
/// reason to skip it).
fn dress_room_line(map: &mut Map, room: &RoomPlan, action: &DoorAction<'_>) {
    face_room_line(map, room.line, action.door_sector, action.setup);
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
        linedef.flags = (linedef.flags | BLOCKING | DONT_PEG_BOTTOM) & !TWO_SIDED;
    }
    if let Some(side) = map.sidedefs.get_mut(jamb.door_side) {
        textures.track.clone_into(&mut side.middle);
    }
}
