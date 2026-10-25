//! Plan features: the game structures a compiled plan installs.
//!
//! [`apply_features`] runs right after [`compile`](fn@crate::plan::compile)
//! and turns the compiled bookkeeping into live map behaviour. It walks
//! a fixed-order pipeline — `doors`, `switches`, `lifts`, `teleporters`,
//! `exits`, `lights` and `secrets` — so a later step always sees what
//! the earlier ones allocated; the private `FeatureRun` carries that
//! state (the tag and band sector every connector got, the rows applied
//! so far) between them.
//!
//! The door step walks the plan's placed door connectors in connection
//! order: a fresh [`next_free_tag`] is stamped on the band sector
//! ([`DoorStyle::Remote`] included — the remote's sector tag is its
//! handle), non-remote styles write their [`door_specials`] action onto
//! the band's room-facing lines with [`face_room_line`], turned so each
//! room's sidedef faces the geometric front, and a locked band's jamb
//! faces are re-dressed in the coloured track [`FeatureDress`] names for
//! the style's [`track_role_name`]; plain styles keep what
//! [`apply_textures`](crate::plan::textures::apply_textures) dressed
//! there. The switch step then mounts each `switch` row's panel on its
//! room's wall (`choose_switch_wall`: among the one-sided room walls
//! facing the row's `wall`, the row's plan `at` point picks the nearest
//! one and centres the panel there, and without it the longest wall
//! wins — the longest outright when `wall` is unset), mounted with the
//! [`build_switch`] core at the default 32 units, and fires the `opens`
//! door's own tag — refused unless the row names a wired
//! [`DoorStyle::Remote`] door. The lift step runs the
//! [`build_lift`] core on each `lift` row's room sector — refused unless
//! every connection from the lift room to a room with a lower floor is
//! a plain opening — stamping a fresh tag, turning the platform's
//! lower-facing edges onto the players below with the family's
//! down-wait-up action and dressing those faces in the theme's `step`
//! texture. The teleporter step runs [`build_teleporter`] on each
//! `teleporter` row: the pad and destination squares (default: the room
//! centres, explicit: transformed plan points) must fit their rooms
//! with 8 units of clearance at the fixed 64-unit size, the pad borders
//! carry the family's walk-on teleport to the fresh-tagged destination
//! and the builder appends its own type-14 destination things straight
//! into the map before the plan's things are placed. The exit step ends
//! the level: its `switch` style mounts the panel the same way, reading
//! the row's `wall` and `at` and carrying the exit action, and its
//! `walk_over` style dresses every two-sided boundary of the room's
//! sectors with [`build_walk_over_exit`]. The
//! lights step paints every room's `light_effect` [`LightEffect`] onto
//! all its sectors with [`light_special`] (keeping the light level and
//! every bit above the family's basic-type field), and the secret step
//! — always after the lights, so a lit secret keeps both — flags every
//! `secret` room's sectors with [`secret_edit`], passing each
//! [`SecretEdit::boom_warning`] through [`boom_note`] into the compiled
//! plan's warnings. Every installed feature reports one [`FeatureRow`],
//! whose `lines` the run's last pass pairs with the finished map's
//! vertex coordinates so a caller sees where each line actually stands.
//! After the pipeline, [`playable_checks`] turns the judgment gaps the
//! wiring cannot see into advisory warnings — never errors: every locked
//! door whose key colour stands nowhere among the finished map's things
//! (matched through the thing definitions: the `Keys` category and the
//! colour word in the name, skull keys included, since their names carry
//! the colour), a plan carrying no `exit` row, and every `remote` door
//! whose id no `switch` row names in its `opens`. The notes come out in
//! plan order — key warnings per door in connection order, then the exit
//! note, then the remote notes in connection order — and the tool
//! appends them after the compiled plan's existing warnings.
//! Nothing here touches archives, sessions or `MCP` types; the theme
//! names arrive already resolved.
//!
//! [`DoorStyle::Remote`]: crate::wad::map::features::DoorStyle::Remote
//! [`SecretEdit::boom_warning`]: crate::wad::map::features::SecretEdit::boom_warning
//! [`door_specials`]: crate::wad::map::features::door_specials
//! [`face_room_line`]: crate::wad::map::features::face_room_line
//! [`next_free_tag`]: crate::wad::map::features::next_free_tag
//! [`track_role_name`]: crate::wad::map::features::DoorStyle::track_role_name
//! [`build_switch`]: crate::wad::map::features::build_switch
//! [`build_lift`]: crate::wad::map::features::build_lift
//! [`build_teleporter`]: crate::wad::map::features::build_teleporter
//! [`build_walk_over_exit`]: crate::wad::map::features::build_walk_over_exit
//! [`LightEffect`]: crate::wad::map::features::LightEffect
//! [`light_special`]: crate::wad::map::features::light_special
//! [`secret_edit`]: crate::wad::map::features::secret_edit
//! [`boom_note`]: crate::wad::map::features::boom_note
//! [`playable_checks`]: checks::playable_checks

use alloc::collections::BTreeMap;

use serde::Serialize;

use crate::error::CyberError;
use crate::plan::emit::CompiledPlan;
use crate::plan::model::{DoorStyle, KeyColor, Plan};
use crate::wad::map::Map;
use crate::wad::map::features::DoorStyle as WadDoorStyle;

mod checks;
mod doors;
mod lifts;
mod rooms;
mod switches;
mod teleporters;

use doors::doors;
use lifts::lifts;
use rooms::{lights, secrets};
use switches::{exits, switches};
use teleporters::teleporters;

pub use checks::playable_checks;

#[cfg(test)]
mod tests;

/// The model's name for "draw nothing here" on a texture face: what an
/// unresolved theme role arrives as, leaving faces as the compiler
/// dressed them.
const NO_NAME: &str = "-";

/// The theme-resolved texture names the feature steps dress with.
///
/// The tool resolves these against the session's theme before the run;
/// a role the theme could not answer arrives as `"-"` and its faces
/// keep what [`apply_textures`](crate::plan::textures::apply_textures)
/// dressed them. The door step reads the coloured tracks; the switch
/// and exit steps read [`switch`](FeatureDress::switch) and
/// [`exit_switch`](FeatureDress::exit_switch); the lift step reads
/// [`step`](FeatureDress::step) (falling back to
/// [`wall`](FeatureDress::wall), the name the compiler's fill pass
/// already hung on the room's height gaps); the teleporter step reads
/// [`wall`](FeatureDress::wall) for the texture bare new edges fall
/// back to, and the plain [`door`](FeatureDress::door) face and
/// [`track`](FeatureDress::track) names ride on with the door styles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeatureDress {
    /// The moving door face (theme role `door`).
    pub door: String,
    /// The plain door track (theme role `door_track`).
    pub track: String,
    /// The track beside a red-key door (role `door_track_red`).
    pub track_red: String,
    /// The track beside a blue-key door (role `door_track_blue`).
    pub track_blue: String,
    /// The track beside a yellow-key door (role `door_track_yellow`).
    pub track_yellow: String,
    /// The wall-switch panel (theme role `switch`).
    pub switch: String,
    /// The exit-switch panel (theme role `exit_switch`).
    pub exit_switch: String,
    /// The lift platform's side: the `lower` band its activation edges
    /// show the lower area (theme role `step`).
    pub step: String,
    /// The plain wall a teleporter square's bare new edges fall back to
    /// (theme role `wall`).
    pub wall: String,
}

impl FeatureDress {
    /// The resolved texture standing for the theme role name `role`
    /// ([`track_role_name`](WadDoorStyle::track_role_name) hands out the
    /// track roles), or `None` when no theme answers it and the faces it
    /// would dress keep their compiled names.
    #[must_use]
    #[inline]
    pub fn texture(&self, role: &str) -> Option<&str> {
        let name = match role {
            "door" => self.door.as_str(),
            "door_track" => self.track.as_str(),
            "door_track_red" => self.track_red.as_str(),
            "door_track_blue" => self.track_blue.as_str(),
            "door_track_yellow" => self.track_yellow.as_str(),
            "switch" => self.switch.as_str(),
            "exit_switch" => self.exit_switch.as_str(),
            "step" => self.step.as_str(),
            "wall" => self.wall.as_str(),
            _ => "",
        };
        (!name.is_empty() && name != NO_NAME).then_some(name)
    }
}

/// One game structure the run installed, as the result reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeatureRow {
    /// The feature class: `door`, `switch`, `lift`, `teleporter`,
    /// `exit`, `light` or `secret`, in pipeline order.
    pub kind: &'static str,
    /// The room the feature stands in; `None` for a connector feature
    /// like a door, which belongs to the wall between two rooms.
    pub room: Option<String>,
    /// The sectors the feature dresses (a door: its band sector; a
    /// switch: the door band it fires; a lift: its platform; a
    /// teleporter: its pad then destination; a light or secret: the
    /// room's painted sectors).
    pub sectors: Vec<usize>,
    /// The linedefs the feature carries its action on (a door: the
    /// room-facing trigger lines; a reported empty list with a tag is a
    /// remote's handle for the switch slice; a light or secret: none).
    pub lines: Vec<usize>,
    /// The tag the feature's sector and lines share, when it has one.
    pub tag: Option<i32>,
    /// Every [`lines`](Self::lines) entry beside the map-unit
    /// coordinates of its two endpoints, in the same order: where the
    /// feature stands in the finished map, so a wall-mounted panel can
    /// be checked against the drawing that asked for it. Read after the
    /// whole pipeline, so a teleporter rebuild's renumbering and the
    /// re-facing pass's swapped vertices are both already the numbers
    /// `lines` names; a line whose vertices the map does not resolve is
    /// left out.
    pub line_ends: Vec<FeatureLineEnds>,
}

/// One reported [`FeatureRow::lines`] entry as the finished map holds
/// it: the linedef index beside the coordinates of its two vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FeatureLineEnds {
    /// The index into the finished map's `LINEDEFS`.
    pub index: usize,
    /// The `x` of the line's `v1` vertex, map units.
    pub x1: i32,
    /// The `y` of the line's `v1` vertex, map units.
    pub y1: i32,
    /// The `x` of the line's `v2` vertex, map units.
    pub x2: i32,
    /// The `y` of the line's `v2` vertex, map units.
    pub y2: i32,
}

/// Install `plan`'s game features onto the `compiled` map and report
/// one [`FeatureRow`] per installed feature, in pipeline order.
///
/// The pipeline is fixed-order — `doors -> switches -> lifts ->
/// teleporters -> exits -> lights -> secrets` — so a later step can
/// find what an earlier one allocated (the switch step looks a door's
/// tag up in the run state; the secret step runs after the lights so a
/// room with both keeps its effect and its flag). Every step acts on
/// the [`Feature`](crate::plan::model::Feature) rows of the plan or, for lights and secrets, the
/// [`Room`](crate::plan::model::Room) fields; `dress` carries the
/// theme-resolved texture names, and a role answered `"-"` leaves its
/// faces as [`compile`](fn@crate::plan::compile) dressed them.
/// `compiled` must be the plan's own compilation: the steps act on the
/// connector and room bookkeeping, never re-derive it. Lift and
/// teleporter build notes (a drop too short for a ride, an overlay
/// warning) join `compiled`'s warnings like every other advisory.
///
/// # Errors
///
/// [`CyberError::Internal`] when a door connector names a band sector
/// the compiled map does not hold — the signature of a `compiled` from
/// a different plan. The switch step refuses a row whose `opens` names
/// no wired [`DoorStyle::Remote`] door, a room with no wall to mount
/// on, and a row whose `at` point leaves the map coordinate range; the
/// lift step refuses a room that owns no sector (painted over),
/// a room that owns several, and one whose lower-neighbour connections
/// are not plain openings, and passes through [`build_lift`](crate::wad::map::features::build_lift)'s refusals
/// for a platform that stands no higher than its low floor or edges no
/// lower sector; the teleporter step refuses the same way, a square
/// that does not fit its room with 8 units of clearance, a transformed
/// centre past the map coordinate range, and
/// [`build_teleporter`](crate::wad::map::features::build_teleporter)'s refusals; the exit step refuses the same way,
/// and a `walk_over` room whose sectors share no two-sided boundary
/// with anything.
#[inline]
pub fn apply_features(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
) -> Result<Vec<FeatureRow>, CyberError> {
    let mut run = FeatureRun::default();
    doors(compiled, dress, &mut run)?;
    switches(plan, compiled, dress, &mut run)?;
    lifts(plan, compiled, dress, &mut run)?;
    teleporters(plan, compiled, dress, &mut run)?;
    exits(plan, compiled, dress, &mut run)?;
    lights(plan, compiled, &mut run);
    secrets(plan, compiled, &mut run);
    reface_action_lines(&mut compiled.map, &run.rows);
    report_line_ends(&mut run.rows, &compiled.map);
    Ok(run.rows)
}

/// Put the wired action lines back facing their players after the run:
/// a teleporter square rebuilds the whole map through
/// [`apply_overlay`](crate::wad::map::overlay::apply_overlay), and the
/// rebuild re-labels line sides by its own convention, so a door or lift
/// line the earlier steps turned to face its room can come out with the
/// moving sector — the row's band or platform — on the front instead of
/// behind. This last pass swaps the sides (and the vertices with them)
/// of exactly those lines, restoring the engine-facing rule the `doors`
/// and `lifts` steps installed: room or lower area in front, moving
/// sector behind. One-sided faces (a switch panel's wall), lines whose
/// orientation survived and lines a rebuild dropped (already out of the
/// rows) are untouched.
fn reface_action_lines(map: &mut Map, rows: &[FeatureRow]) {
    for row in rows {
        reface_row(map, row);
    }
}

/// Re-face the one wired row's action lines: doors and lifts only, each
/// with its moving sector (band or platform, the row's first sector)
/// kept behind the line so the room or the lower area stays the front.
fn reface_row(map: &mut Map, row: &FeatureRow) {
    if row.kind != "door" && row.kind != "lift" {
        return;
    }
    let Some(behind) = row.sectors.first().copied() else {
        return;
    };
    let flipped: Vec<usize> = row
        .lines
        .iter()
        .copied()
        .filter(|&line| has_sector_in_front(map, line, behind))
        .collect();
    for line in flipped {
        if let Some(def) = map.linedefs.get_mut(line) {
            core::mem::swap(&mut def.front, &mut def.back);
            core::mem::swap(&mut def.v1, &mut def.v2);
        }
    }
}

/// Pair every reported line of `rows` with the endpoints `map` holds for
/// it: the run's last pass, after the rebuilds that renumbered lines and
/// the re-facing that swapped vertices, so the coordinates a caller reads
/// are the ones the finished map carries.
fn report_line_ends(rows: &mut [FeatureRow], map: &Map) {
    for row in rows {
        row.line_ends = row
            .lines
            .iter()
            .copied()
            .filter_map(|index| line_end_of(map, index))
            .collect();
    }
}

/// The reported endpoints of linedef `index`, `None` when the map holds
/// no such line or no such vertex.
fn line_end_of(map: &Map, index: usize) -> Option<FeatureLineEnds> {
    let line = map.linedefs.get(index)?;
    let first = map.vertices.get(line.v1)?;
    let second = map.vertices.get(line.v2)?;
    Some(FeatureLineEnds {
        index,
        x1: first.x,
        y1: first.y,
        x2: second.x,
        y2: second.y,
    })
}

/// Whether `line` is two-sided with sector `sector` on its front side.
fn has_sector_in_front(map: &Map, line: usize, sector: usize) -> bool {
    map.linedefs.get(line).is_some_and(|def| {
        def.back.is_some()
            && def
                .front
                .and_then(|side| map.sidedefs.get(side))
                .is_some_and(|row| row.sector == sector)
    })
}

/// The state one [`apply_features`] run threads through the pipeline:
/// the tags and band sectors allocated so far and the rows applied. The
/// switch step reads [`FeatureRun::tags`] and [`FeatureRun::bands`] to
/// find the door an `opens` id names.
#[derive(Default)]
struct FeatureRun {
    /// Connection id to the tag its door band carries.
    tags: BTreeMap<String, i32>,
    /// Connection id to the band sector the door step tagged.
    bands: BTreeMap<String, usize>,
    /// The rows applied so far, in pipeline order.
    rows: Vec<FeatureRow>,
}

/// The sectors `room` owns in the compiled map, refusing a room the
/// emission kept none of (fully painted over by a later one).
fn room_sectors_of(compiled: &CompiledPlan, room: &str) -> Result<Vec<usize>, CyberError> {
    compiled.room_sectors.get(room).cloned().ok_or_else(|| {
        invalid(&format!(
            "feature names room '{room}', which the compiled map holds no sector for: it \
             was painted over by a later room; move the rooms apart or drop the feature"
        ))
    })
}

/// The sectors `room` owns in the compiled map, empty for a room the
/// emission kept none of (fully painted over by a later room, which
/// [`compile`](fn@crate::plan::compile) already warned about). Room
/// properties that paint sectors — the lights and secrets steps —
/// install nothing for such a room instead of refusing.
fn owned_sectors(compiled: &CompiledPlan, room: &str) -> Vec<usize> {
    compiled.room_sectors.get(room).cloned().unwrap_or_default()
}

/// The `InvalidArgument` with an actionable `message`.
fn invalid(message: &str) -> CyberError {
    CyberError::InvalidArgument(message.to_owned())
}

/// The door core's style behind a plan [`DoorStyle`]: the locked
/// variants pick their colour, everything else carries over.
const fn wad_style(style: DoorStyle) -> WadDoorStyle {
    match style {
        DoorStyle::Normal => WadDoorStyle::Normal,
        DoorStyle::Fast => WadDoorStyle::Fast,
        DoorStyle::StayOpen => WadDoorStyle::StayOpen,
        DoorStyle::Locked(KeyColor::Red) => WadDoorStyle::LockedRed,
        DoorStyle::Locked(KeyColor::Blue) => WadDoorStyle::LockedBlue,
        DoorStyle::Locked(KeyColor::Yellow) => WadDoorStyle::LockedYellow,
        DoorStyle::Remote => WadDoorStyle::Remote,
    }
}
