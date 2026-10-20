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
//! room's wall (`choose_switch_wall`: the longest one-sided room wall
//! facing the row's `wall`, or the longest outright when `wall` is
//! unset), centred with the [`build_switch`] core at the default 32
//! units, and fires the `opens` door's own tag — refused unless the
//! row names a wired [`DoorStyle::Remote`] door. The lift step runs the
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
//! the level: its `switch` style mounts the panel the same way carrying
//! the exit action, and its `walk_over` style dresses every two-sided
//! boundary of the room's sectors with [`build_walk_over_exit`]. The
//! lights step paints every room's `light_effect` [`LightEffect`] onto
//! all its sectors with [`light_special`] (keeping the light level and
//! every bit above the family's basic-type field), and the secret step
//! — always after the lights, so a lit secret keeps both — flags every
//! `secret` room's sectors with [`secret_edit`], passing each
//! [`SecretEdit::boom_warning`] through [`boom_note`] into the compiled
//! plan's warnings. Every installed feature reports one [`FeatureRow`].
//! Nothing here touches archives, sessions or `MCP` types; the theme
//! names arrive already resolved.
//!
//! [`DoorStyle::Remote`]: crate::wad::map::features::DoorStyle::Remote
//! [`SecretEdit::boom_warning`]: crate::wad::map::features::SecretEdit::boom_warning
//! [`door_specials`]: crate::wad::map::features::door_specials
//! [`face_room_line`]: crate::wad::map::features::face_room_line
//! [`next_free_tag`]: crate::wad::map::features::next_free_tag
//! [`track_role_name`]: crate::wad::map::features::DoorStyle::track_role_name

use alloc::collections::BTreeMap;

use serde::Serialize;

use crate::error::CyberError;
use crate::geom::Pt;
use crate::plan::emit::{CompiledPlan, ConnectorInfo};
use crate::plan::model::{
    Connection, ConnectionKind, DoorStyle, ExitStyle, Feature, KeyColor, Plan, Side,
};
use crate::wad::map::Map;
use crate::wad::map::features::DoorStyle as WadDoorStyle;
use crate::wad::map::features::{
    DEFAULT_PAD_SIZE, DEFAULT_SWITCH_WIDTH, LiftPlatform, LiftSpec, LightEffect, PlacedSwitch,
    SecretEdit, SwitchAction, SwitchSpec, TeleportSpec, boom_note, build_lift, build_switch,
    build_teleporter, build_walk_over_exit, door_specials, face_room_line, light_special,
    next_free_tag, secret_edit,
};
use crate::wad::map::geometry::{point_in_sector, sector_bbox};
use crate::wad::map::model::Linedef;
use crate::wad::map::ops::Point;

/// The model's name for "draw nothing here" on a texture face: what an
/// unresolved theme role arrives as, leaving faces as the compiler
/// dressed them.
const NO_NAME: &str = "-";

/// The units by which the plan teleporter's fit rule insets the square's
/// corners: every corner of the 64-unit square shrunk by this much must
/// resolve into the room that owns the square.
const TELEPORT_CLEARANCE: i32 = 8_i32;

/// Divisor of the two-bound axis sum that gives the box centre.
const AXIS_DIVISOR: i64 = 2_i64;

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
}

/// Install `plan`'s game features onto the `compiled` map and report
/// one [`FeatureRow`] per installed feature, in pipeline order.
///
/// The pipeline is fixed-order — `doors -> switches -> lifts ->
/// teleporters -> exits -> lights -> secrets` — so a later step can
/// find what an earlier one allocated (the switch step looks a door's
/// tag up in the run state; the secret step runs after the lights so a
/// room with both keeps its effect and its flag). Every step acts on
/// the [`Feature`] rows of the plan or, for lights and secrets, the
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
/// no wired [`DoorStyle::Remote`] door and a room with no wall to mount
/// on; the lift step refuses a room that owns no sector (painted over),
/// a room that owns several, and one whose lower-neighbour connections
/// are not plain openings, and passes through [`build_lift`]'s refusals
/// for a platform that stands no higher than its low floor or edges no
/// lower sector; the teleporter step refuses the same way, a square
/// that does not fit its room with 8 units of clearance, a transformed
/// centre past the map coordinate range, and
/// [`build_teleporter`]'s refusals; the exit step refuses the same way,
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
    Ok(run.rows)
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

/// The door step: wire every placed door connector, in connection
/// order.
fn doors(
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for info in &compiled.connector_sectors {
        wire_door(&mut compiled.map, info, dress, run)?;
    }
    Ok(())
}

/// The switch step: mount every `switch` row's panel on its room's wall
/// firing the remote door's tag, in plan order.
fn switches(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for feature in plan.features.clone() {
        let Feature::Switch {
            room,
            opens,
            wall,
            repeatable,
        } = feature
        else {
            continue;
        };
        let door = remote_door(plan, run, &opens)?;
        let sectors = room_sectors_of(compiled, &room)?;
        let mount = SwitchMount {
            action: SwitchAction::OpenDoor,
            role: "switch",
            target: Some(door.band),
            repeatable,
            dress,
            room: &room,
        };
        let placed = mount_panel(&mut compiled.map, &sectors, wall, &mount)?;
        run.rows.push(FeatureRow {
            kind: "switch",
            room: Some(room),
            sectors: vec![door.band],
            lines: vec![placed.linedef],
            tag: Some(placed.tag),
        });
    }
    Ok(())
}

/// One wall-mounted panel to place: what it does, which dressed role it
/// wears, and the room it stands in for the refusal wording.
struct SwitchMount<'mount> {
    /// The action the [`build_switch`] core installs.
    action: SwitchAction,
    /// The [`FeatureDress`] role naming the panel texture.
    role: &'mount str,
    /// The sector the action fires on (tagged actions only).
    target: Option<usize>,
    /// Whether the line keeps working after use.
    repeatable: bool,
    /// The theme-resolved names the run dresses with.
    dress: &'mount FeatureDress,
    /// The room the panel is mounted in, for the wall-refusal wording.
    room: &'mount str,
}

/// Choose the room's switch wall and mount `mount`'s panel centred on
/// it.
fn mount_panel(
    map: &mut Map,
    sectors: &[usize],
    wall: Option<Side>,
    mount: &SwitchMount<'_>,
) -> Result<PlacedSwitch, CyberError> {
    let linedef = choose_switch_wall(map, sectors, wall, mount.room)?;
    place_switch(map, linedef, mount)
}

/// Run the [`build_switch`] core for one mount: the default-width panel
/// centred on `linedef`'s midpoint, carrying the door's own tag.
fn place_switch(
    map: &mut Map,
    linedef: usize,
    mount: &SwitchMount<'_>,
) -> Result<PlacedSwitch, CyberError> {
    let texture = panel_texture(map, linedef, mount);
    let spec = SwitchSpec {
        linedef,
        at: None,
        width: DEFAULT_SWITCH_WIDTH,
        action: mount.action,
        target_sector: mount.target,
        repeatable: mount.repeatable,
        texture,
    };
    build_switch(map, &spec)
}

/// The panel texture: the theme's answer for the mount's role, or the
/// wall's own current middle when the theme left the role unnamed —
/// those faces keep what the compiler dressed, door-slice style.
fn panel_texture(map: &Map, linedef: usize, mount: &SwitchMount<'_>) -> String {
    mount
        .dress
        .texture(mount.role)
        .map_or_else(|| wall_middle(map, linedef), str::to_owned)
}

/// The chosen wall's front-side `middle` texture, `"-"` when the line
/// wears nothing.
fn wall_middle(map: &Map, linedef: usize) -> String {
    map.linedefs
        .get(linedef)
        .and_then(|line| line.front)
        .and_then(|side| map.sidedefs.get(side))
        .map_or_else(|| NO_NAME.to_owned(), |side| side.middle.clone())
}

/// The lift step: dress every `lift` row's room sector as a platform,
/// in plan order.
fn lifts(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for feature in plan.features.clone() {
        let Feature::Lift {
            room,
            low_floor,
            style,
            repeatable,
        } = feature
        else {
            continue;
        };
        let sectors = room_sectors_of(compiled, &room)?;
        lift_join_rule(plan, &room)?;
        let platform = single_platform_sector(&sectors, &room)?;
        let spec = LiftSpec {
            platform: LiftPlatform::Existing(platform),
            low_floor,
            style,
            repeatable,
            side: lift_side(dress),
            floor_flat: NO_NAME.to_owned(),
        };
        let built = build_lift(&mut compiled.map, &spec)?;
        compiled.warnings.extend(built.notes.iter().cloned());
        run.rows.push(FeatureRow {
            kind: "lift",
            room: Some(room),
            sectors: vec![built.platform_sector],
            lines: built.lines,
            tag: Some(built.tag),
        });
    }
    Ok(())
}

/// The single sector of room `room`, which the plan lift dresses as
/// its whole platform: emission gives every plan room exactly one
/// sector today, and a room that ever owns several is refused by name
/// rather than silently lifting one of them.
fn single_platform_sector(sectors: &[usize], room: &str) -> Result<usize, CyberError> {
    match sectors {
        &[only] => Ok(only),
        many => Err(invalid(&format!(
            "lift room '{room}' owns {} sectors: the plan lift dresses a whole room as \
             one platform; give the room a single sector, or lift one of them directly \
             with add_lift",
            many.len()
        ))),
    }
}

/// The lift room's geometry rule: every connection from `room` to a
/// room whose floor is lower must be a plain opening, or the ride down
/// would dead-end in a closed band, a glass sill or a step no trigger
/// reaches.
fn lift_join_rule(plan: &Plan, room: &str) -> Result<(), CyberError> {
    let Some(lift) = plan.rooms.iter().find(|entry| entry.id == room) else {
        return Ok(());
    };
    for connection in &plan.connections {
        let Some(neighbour) = joined_room(connection, room) else {
            continue;
        };
        if !joins_lower_room(plan, neighbour, lift.floor) {
            continue;
        }
        if !matches!(connection.kind, ConnectionKind::Opening) {
            return Err(lift_join_refusal(room, neighbour, connection));
        }
    }
    Ok(())
}

/// The room across `connection` from `room`, or `None` when the
/// connection does not touch `room`.
fn joined_room<'conn>(connection: &'conn Connection, room: &str) -> Option<&'conn str> {
    if connection.from == room {
        Some(connection.to.as_str())
    } else if connection.to == room {
        Some(connection.from.as_str())
    } else {
        None
    }
}

/// Whether room `neighbour` sits below `floor`.
fn joins_lower_room(plan: &Plan, neighbour: &str, floor: i32) -> bool {
    plan.rooms
        .iter()
        .find(|entry| entry.id == neighbour)
        .is_some_and(|entry| entry.floor < floor)
}

/// The refusal for a lift room joined to a lower room by anything but
/// an opening.
fn lift_join_refusal(room: &str, neighbour: &str, connection: &Connection) -> CyberError {
    invalid(&format!(
        "lift room '{room}' joins lower room '{neighbour}' through connection '{}' of \
         kind {}: a lift needs openings to the lower rooms; make the connection an \
         opening, raise the lower room's floor, or move the lift",
        connection_name(connection),
        connection_kind_name(connection.kind),
    ))
}

/// The label `connection` answers to in messages: its `id`, or
/// `from->to` when unnamed.
fn connection_name(connection: &Connection) -> String {
    connection
        .id
        .clone()
        .unwrap_or_else(|| format!("{}->{}", connection.from, connection.to))
}

/// The wire class of `connection`'s kind.
const fn connection_kind_name(kind: ConnectionKind) -> &'static str {
    match kind {
        ConnectionKind::Opening => "opening",
        ConnectionKind::Door { .. } => "door",
        ConnectionKind::Window { .. } => "window",
        ConnectionKind::Stairs { .. } => "stairs",
    }
}

/// The texture a plan lift's platform side wears: the theme's `step`
/// name, else the `wall` name the compiler's fill pass already dressed
/// the room's height gaps in, else `"-"` (no theme answer at all).
fn lift_side(dress: &FeatureDress) -> String {
    dress
        .texture("step")
        .or_else(|| dress.texture("wall"))
        .unwrap_or(NO_NAME)
        .to_owned()
}

/// The teleporter step: stand every `teleporter` row's pad and
/// destination squares, in plan order.
fn teleporters(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for feature in plan.features.clone() {
        let Feature::Teleporter {
            from_room,
            to_room,
            pad,
            destination,
            angle,
            two_way,
        } = feature
        else {
            continue;
        };
        let row = TeleporterRow {
            pad: teleport_square("pad", &from_room, pad, compiled)?,
            destination: teleport_square("destination", &to_room, destination, compiled)?,
            angle,
            two_way,
        };
        let spec = teleport_spec(compiled, dress, &row)?;
        let built = build_teleporter(&mut compiled.map, &spec)?;
        remap_rows(&mut run.rows, &built.line_remap);
        compiled.warnings.extend(built.notes.iter().cloned());
        run.rows.push(FeatureRow {
            kind: "teleporter",
            room: Some(from_room),
            sectors: vec![built.pad_sector, built.destination_sector],
            lines: built.pad_lines,
            tag: Some(built.tag),
        });
    }
    Ok(())
}

/// Follow every line index reported so far through one teleporter's
/// square rebuilds: the arrangement re-sorts the map's lines, so a
/// row must name its lines as the finished map holds them, and a line
/// a square painted over drops out of the row.
fn remap_rows(rows: &mut [FeatureRow], remap: &[Option<usize>]) {
    for row in rows {
        row.lines = row
            .lines
            .iter()
            .copied()
            .filter_map(|old| remap.get(old).copied().flatten())
            .collect();
    }
}

/// One square of a plan teleporter: the room it stands in, the room's
/// sectors in the compiled map, and the transformed centre the wire
/// gave, if any.
struct TeleportSquare<'plan> {
    /// The word naming the square in messages: `pad` or `destination`.
    role: &'static str,
    /// The id of the room the square stands in.
    room: &'plan str,
    /// The wire's transformed, snapped centre, if it gave one.
    given: Option<Pt>,
    /// The indices into the compiled map's sectors that `room` owns.
    sectors: Vec<usize>,
}

/// One square resolved against `compiled`: its room must own sectors
/// the emission kept.
fn teleport_square<'plan>(
    role: &'static str,
    room: &'plan str,
    given: Option<Pt>,
    compiled: &CompiledPlan,
) -> Result<TeleportSquare<'plan>, CyberError> {
    Ok(TeleportSquare {
        role,
        room,
        given,
        sectors: room_sectors_of(compiled, room)?,
    })
}

/// The two squares and the facing of one `teleporter` row.
struct TeleporterRow<'plan> {
    /// The pad square, standing in `from_room`.
    pad: TeleportSquare<'plan>,
    /// The destination square, standing in `to_room`.
    destination: TeleportSquare<'plan>,
    /// The arriving player's facing, degrees (the builder normalises).
    angle: i32,
    /// Whether the destination teleports back to the pad as well.
    two_way: bool,
}

/// The centre of `square`: the wire's point, else the centre of the
/// room sectors' merged bounding box.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a wire point past the map
/// coordinate range, [`CyberError::Internal`] for a room the compiled
/// map bounds nowhere.
fn square_centre(map: &Map, square: &TeleportSquare<'_>) -> Result<Point, CyberError> {
    let given = match square.given {
        Some(point) => map_point(point, square.role)?,
        None => merged_centre(map, &square.sectors).ok_or_else(|| unbounded_room(square))?,
    };
    Ok(given)
}

/// The `Internal` refusal for a room whose sectors bound nothing.
fn unbounded_room(square: &TeleportSquare<'_>) -> CyberError {
    CyberError::Internal(format!(
        "the teleporter {} room '{}' owns no sector the compiled map can bound; pass an \
         explicit {} point",
        square.role, square.room, square.role
    ))
}

/// Require every [`TELEPORT_CLEARANCE`]-inset corner of the 64-unit
/// square at `centre` to resolve through [`point_in_sector`] into one
/// of the square's room sectors.
fn assert_square_fits(
    map: &Map,
    centre: Point,
    square: &TeleportSquare<'_>,
) -> Result<(), CyberError> {
    for (x, y) in square_corners(centre) {
        let inside = point_in_sector(map, i64::from(x), i64::from(y))
            .is_some_and(|sector| square.sectors.contains(&sector));
        if !inside {
            return Err(square_too_big(square, x, y));
        }
    }
    Ok(())
}

/// The [`TeleportSpec`] behind one row: both centres resolved and
/// fitted, the fixed [`DEFAULT_PAD_SIZE`] side, the rooms' own floor
/// flats and the theme's `wall` for bare new edges.
///
/// # Errors
///
/// [`square_centre`]'s and [`assert_square_fits`]' refusals for either
/// square.
fn teleport_spec(
    compiled: &CompiledPlan,
    dress: &FeatureDress,
    row: &TeleporterRow<'_>,
) -> Result<TeleportSpec, CyberError> {
    let map = &compiled.map;
    let pad = square_centre(map, &row.pad)?;
    assert_square_fits(map, pad, &row.pad)?;
    let destination = square_centre(map, &row.destination)?;
    assert_square_fits(map, destination, &row.destination)?;
    Ok(TeleportSpec {
        pad,
        destination,
        angle: row.angle,
        pad_size: DEFAULT_PAD_SIZE,
        two_way: row.two_way,
        floor_flat: NO_NAME.to_owned(),
        wall: teleport_wall(dress),
    })
}

/// The texture a plan teleporter's bare new edges fall back to: the
/// theme's `wall` name, `"-"` when the theme left the role unnamed.
fn teleport_wall(dress: &FeatureDress) -> String {
    dress.texture("wall").unwrap_or(NO_NAME).to_owned()
}

/// The four corners of the [`DEFAULT_PAD_SIZE`] square centred on
/// `centre`, inset by [`TELEPORT_CLEARANCE`] units.
const fn square_corners(centre: Point) -> [(i32, i32); 4] {
    let reach = DEFAULT_PAD_SIZE
        .saturating_div(2)
        .saturating_sub(TELEPORT_CLEARANCE);
    let (x, y) = (centre.x, centre.y);
    [
        (x.saturating_sub(reach), y.saturating_sub(reach)),
        (x.saturating_add(reach), y.saturating_sub(reach)),
        (x.saturating_add(reach), y.saturating_add(reach)),
        (x.saturating_sub(reach), y.saturating_add(reach)),
    ]
}

/// The centre of the merged bounding box of `sectors`, or `None` when
/// no one of them bounds a resolvable vertex.
fn merged_centre(map: &Map, sectors: &[usize]) -> Option<Point> {
    let (min_x, min_y, max_x, max_y) = merged_bbox(map, sectors)?;
    Some(Point {
        x: mid_axis(min_x, max_x),
        y: mid_axis(min_y, max_y),
    })
}

/// The box covering `sectors`, or `None` when no one of them bounds a
/// resolvable vertex.
fn merged_bbox(map: &Map, sectors: &[usize]) -> Option<(i32, i32, i32, i32)> {
    sectors
        .iter()
        .filter_map(|&index| sector_bbox(map, index))
        .reduce(|(a1, b1, a2, b2), (c1, d1, c2, d2)| {
            (a1.min(c1), b1.min(d1), a2.max(c2), b2.max(d2))
        })
}

/// The centre axis of two box edges, halved in `i64` so the sum never
/// wraps.
fn mid_axis(low: i32, high: i32) -> i32 {
    let sum = i64::from(low).saturating_add(i64::from(high));
    let half = sum.div_euclid(AXIS_DIVISOR);
    i32::try_from(half).unwrap_or(if half < 0_i64 { i32::MIN } else { i32::MAX })
}

/// The map-unit [`Point`] of a transformed plan coordinate, refused
/// when an axis does not fit the map's `i32` grid.
fn map_point(point: Pt, role: &str) -> Result<Point, CyberError> {
    let (x, y) = point;
    let fitted = i32::try_from(x).ok().zip(i32::try_from(y).ok());
    fitted.map_or_else(
        || Err(centre_out_of_range(point, role)),
        |(fx, fy)| Ok(Point { x: fx, y: fy }),
    )
}

/// The refusal for a wire centre past the map coordinate range.
fn centre_out_of_range(point: Pt, role: &str) -> CyberError {
    invalid(&format!(
        "the teleporter {role} point {}, {} is outside the map coordinate range: keep \
         plan points and the units transform inside the map grid",
        point.0, point.1
    ))
}

/// The refusal for a teleporter square its room cannot hold.
fn square_too_big(square: &TeleportSquare<'_>, x: i32, y: i32) -> CyberError {
    invalid(&format!(
        "the teleporter {} does not fit inside room '{}' with {TELEPORT_CLEARANCE} units \
         of clearance: the square corner at {x}, {y} lands outside the room; pass an \
         explicit {} point nearer the room's middle or move the rooms apart",
        square.role, square.room, square.role,
    ))
}

/// The exit step: end the level where every `exit` row stands, in plan
/// order — its `switch` style through the same wall-mounted panel as
/// the switch step, its `walk_over` style on every two-sided boundary
/// of the room's sectors.
fn exits(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for feature in plan.features.clone() {
        let Feature::Exit {
            room,
            style,
            secret,
            wall,
        } = feature
        else {
            continue;
        };
        let sectors = room_sectors_of(compiled, &room)?;
        let lines = match style {
            ExitStyle::Switch => {
                let mount = exit_mount(dress, &room, secret);
                vec![mount_panel(&mut compiled.map, &sectors, wall, &mount)?.linedef]
            }
            ExitStyle::WalkOver => walk_over_exit(&mut compiled.map, &sectors, secret)?,
        };
        run.rows.push(FeatureRow {
            kind: "exit",
            room: Some(room),
            sectors,
            lines,
            tag: None,
        });
    }
    Ok(())
}

/// The `switch`-style exit mount: the (secret) exit action on the
/// room's chosen wall, wearing the theme's `exit_switch` panel.
const fn exit_mount<'mount>(
    dress: &'mount FeatureDress,
    room: &'mount str,
    secret: bool,
) -> SwitchMount<'mount> {
    let action = if secret {
        SwitchAction::SecretExit
    } else {
        SwitchAction::Exit
    };
    SwitchMount {
        action,
        role: "exit_switch",
        target: None,
        repeatable: false,
        dress,
        room,
    }
}

/// Dress every two-sided boundary of `sectors` with the family's
/// walk-over level end, keeping the tags those lines carry — a plan
/// room's lines hold tag `0`, which in the `Doom` family ends the game.
fn walk_over_exit(
    map: &mut Map,
    sectors: &[usize],
    secret: bool,
) -> Result<Vec<usize>, CyberError> {
    let mut lines = Vec::new();
    for &sector in sectors {
        lines.extend(build_walk_over_exit(map, sector, secret)?.lines);
    }
    lines.sort_unstable();
    lines.dedup();
    Ok(lines)
}

/// The lights step: paint every room's `light_effect` onto all of the
/// room's sectors, in plan order — always before the secret step, so a
/// room that is both lit and hidden ends up carrying both.
fn lights(plan: &Plan, compiled: &mut CompiledPlan, run: &mut FeatureRun) {
    for room in &plan.rooms {
        let Some(effect) = room.light_effect else {
            continue;
        };
        light_room(compiled, &room.id, effect, run);
    }
}

/// Paint `effect` onto every sector of `room`: the family's basic type
/// goes into the type field, the bits above it and the light level
/// stay (the plan-then-write pattern of the `set_light_effect` tool).
/// A room the emission kept no sector of (fully painted over, already
/// warned about) installs nothing and reports no row.
fn light_room(compiled: &mut CompiledPlan, room: &str, effect: LightEffect, run: &mut FeatureRun) {
    let sectors = owned_sectors(compiled, room);
    let family = compiled.map.flag_family();
    let edits: Vec<(usize, i32)> = sectors
        .iter()
        .filter_map(|&index| {
            compiled
                .map
                .sectors
                .get(index)
                .map(|row| (index, light_special(family, row.special, effect)))
        })
        .collect();
    for (index, special) in edits {
        if let Some(row) = compiled.map.sectors.get_mut(index) {
            row.special = special;
        }
    }
    if !sectors.is_empty() {
        run.rows.push(FeatureRow {
            kind: "light",
            room: Some(room.to_owned()),
            sectors,
            lines: Vec::new(),
            tag: None,
        });
    }
}

/// The secrets step, always after the lights: flag every room the plan
/// marks `secret`, so the room's sectors carry the family's secret flag
/// over whatever special the earlier steps left there.
fn secrets(plan: &Plan, compiled: &mut CompiledPlan, run: &mut FeatureRun) {
    for room in &plan.rooms {
        if room.secret {
            secret_room(compiled, &room.id, run);
        }
    }
}

/// Set the family's secret flag on every sector of `room`, keeping
/// every bit the flag does not own and passing each
/// [`SecretEdit::boom_warning`] through the shared [`boom_note`] into
/// the compiled plan's warnings. A room the emission kept no sector of
/// installs nothing and reports no row.
///
/// [`SecretEdit::boom_warning`]: crate::wad::map::features::SecretEdit::boom_warning
fn secret_room(compiled: &mut CompiledPlan, room: &str, run: &mut FeatureRun) {
    let sectors = owned_sectors(compiled, room);
    let family = compiled.map.flag_family();
    let edits: Vec<(usize, SecretEdit)> = sectors
        .iter()
        .filter_map(|&index| {
            compiled
                .map
                .sectors
                .get(index)
                .map(|row| (index, secret_edit(family, row.special, true)))
        })
        .collect();
    for (index, edit) in edits {
        if edit.boom_warning {
            compiled.warnings.push(boom_note(index, edit.special));
        }
        if let Some(row) = compiled.map.sectors.get_mut(index) {
            row.special = edit.special;
        }
    }
    if !sectors.is_empty() {
        run.rows.push(FeatureRow {
            kind: "secret",
            room: Some(room.to_owned()),
            sectors,
            lines: Vec::new(),
            tag: None,
        });
    }
}

/// A remote door a switch row may fire: its tagged band sector.
#[derive(Debug)]
struct RemoteDoor {
    /// The band sector `target_sector` points at; the door step already
    /// stamped its tag, so [`build_switch`] keeps it instead of minting
    /// a second one.
    band: usize,
}

/// Find the wired remote door `opens` names: the plan row must carry
/// the id, be a [`DoorStyle::Remote`] door, and have been tagged by the
/// door step (a band a later room painted over allocated nothing).
fn remote_door(plan: &Plan, run: &FeatureRun, opens: &str) -> Result<RemoteDoor, CyberError> {
    let styled = plan.connections.iter().any(|row| {
        row.id.as_deref() == Some(opens)
            && matches!(
                row.kind,
                ConnectionKind::Door {
                    style: DoorStyle::Remote
                }
            )
    });
    let wired = styled
        .then(|| {
            Some(RemoteDoor {
                band: *run.bands.get(opens)?,
            })
        })
        .flatten();
    wired.ok_or_else(|| not_remote(opens))
}

/// The refusal for a switch whose `opens` names no wired remote door.
fn not_remote(opens: &str) -> CyberError {
    invalid(&format!(
        "switch opens {opens} which is not a remote door: point it at a door connection \
         of style remote, or leave the door for its own trigger"
    ))
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

/// The linedef a wall-mounted feature takes in the room owning
/// `room_sectors`: the longest of the room's one-sided walls — no back,
/// front side in one of those sectors — whose outward normal points at
/// `wall` ([`Side`]'s compass axis), or, with `wall: None`, the room's
/// longest one-sided wall outright. Ties go to the lowest linedef
/// index, and lines whose vertices do not resolve never win. All
/// integer: the facing test is the sign of the outward normal's
/// (`-dy`, `dx`) dot product with the side's axis vector, the length
/// comparison that of the squared run.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming `room` and, when given, the
/// side, when the room holds no such wall.
fn choose_switch_wall(
    map: &Map,
    room_sectors: &[usize],
    wall: Option<Side>,
    room: &str,
) -> Result<usize, CyberError> {
    let mut best: Option<(i128, usize)> = None;
    for (index, line) in map.linedefs.iter().enumerate() {
        let Some((dx, dy)) = wall_vector(map, line, room_sectors) else {
            continue;
        };
        if wall.is_some_and(|side| !faces_toward(side, dx, dy)) {
            continue;
        }
        let squared = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
        if best.is_none_or(|(known, _)| squared > known) {
            best = Some((squared, index));
        }
    }
    best.map(|(_, index)| index)
        .ok_or_else(|| no_switch_wall(room, wall))
}

/// The `v1` to `v2` vector of `line` when it is one-sided, its front
/// side belongs to one of `room_sectors`, and both vertices resolve.
fn wall_vector(map: &Map, line: &Linedef, room_sectors: &[usize]) -> Option<(i128, i128)> {
    if line.back.is_some() {
        return None;
    }
    let sector = map.sidedefs.get(line.front?)?.sector;
    if !room_sectors.contains(&sector) {
        return None;
    }
    let first = map.vertices.get(line.v1)?;
    let second = map.vertices.get(line.v2)?;
    Some((
        i128::from(second.x).saturating_sub(i128::from(first.x)),
        i128::from(second.y).saturating_sub(i128::from(first.y)),
    ))
}

/// Whether the outward normal (`-dy`, `dx`) of a line running `(dx, dy)`
/// points toward `side`: a positive dot product with the side's axis.
/// The room sits on the front (right) side of a one-sided wall, so the
/// void behind it is to the line's left.
const fn faces_toward(side: Side, dx: i128, dy: i128) -> bool {
    match side {
        Side::North => dx > 0_i128,
        Side::South => dx < 0_i128,
        Side::East => dy < 0_i128,
        Side::West => dy > 0_i128,
    }
}

/// The refusal for a room with no wall to mount on, naming the asked
/// side when there is one.
fn no_switch_wall(room: &str, wall: Option<Side>) -> CyberError {
    let facing = wall.map_or_else(
        || "no one-sided wall".to_owned(),
        |side| format!("no one-sided wall facing {}", side_name(side)),
    );
    invalid(&format!(
        "room '{room}' has {facing} to mount a switch on: every boundary line is shared \
         with another sector; pick another room, leave `wall` unset, or give the plan a \
         wall thickness"
    ))
}

/// The wire name of a compass `side`.
const fn side_name(side: Side) -> &'static str {
    match side {
        Side::North => "north",
        Side::South => "south",
        Side::East => "east",
        Side::West => "west",
    }
}

/// Install one door connector: stamp a fresh tag on its band sector,
/// give the band's room-facing lines the style's action, dress a locked
/// band's jamb faces in their coloured track, and record the row and
/// the tag under the connector id. A [`DoorStyle::Remote`] band keeps
/// its lines bare — the tagged sector is the handle a remote trigger
/// (the switch step's panel, or the caller's own line) fires on.
///
/// A band a later connector painted over owns no sector: there is
/// nothing to tag and no line to face, so the door reports no row.
///
/// # Errors
///
/// [`wire_door`]'s and [`apply_features`]'s `Internal` refusal for a
/// band sector the map does not hold.
fn wire_door(
    map: &mut Map,
    info: &ConnectorInfo,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    let ConnectionKind::Door { style } = info.kind else {
        return Ok(());
    };
    let Some(band) = info.sectors.first().copied() else {
        return Ok(());
    };
    let core = wad_style(style);
    let tag = next_free_tag(map);
    stamp_band_tag(map, band, tag, info)?;
    if core != WadDoorStyle::Remote {
        let setup = door_specials(map.flag_family(), core, tag);
        for &line in &info.door_lines {
            face_room_line(map, line, band, &setup);
        }
    }
    dress_locked_jambs(map, band, core, dress);
    run.tags.insert(info.id.clone(), tag);
    run.bands.insert(info.id.clone(), band);
    run.rows.push(FeatureRow {
        kind: "door",
        room: None,
        sectors: vec![band],
        lines: info.door_lines.clone(),
        tag: Some(tag),
    });
    Ok(())
}

/// Stamp `tag` on the band sector `band`, refusing bookkeeping the map
/// does not back: [`emit`](crate::plan::emit::emit) allocated every
/// recorded sector, so a missing row means `compiled` did not come from
/// this plan.
fn stamp_band_tag(
    map: &mut Map,
    band: usize,
    tag: i32,
    info: &ConnectorInfo,
) -> Result<(), CyberError> {
    match map.sectors.get_mut(band) {
        Some(space) => {
            space.tag = tag;
            Ok(())
        }
        None => Err(CyberError::Internal(format!(
            "door connector '{}' names band sector {band}, which the compiled map does \
             not hold; apply features to the plan's own compiled result",
            info.id
        ))),
    }
}

/// Re-dress a locked door band's jamb faces — the one-sided lines whose
/// front faces the band sector — in the coloured track the style's role
/// names. Plain and remote styles keep what the compiler dressed, and
/// so does a locked style whose colour the theme left unnamed.
fn dress_locked_jambs(map: &mut Map, band: usize, style: WadDoorStyle, dress: &FeatureDress) {
    let locked = matches!(
        style,
        WadDoorStyle::LockedRed | WadDoorStyle::LockedBlue | WadDoorStyle::LockedYellow
    );
    if !locked {
        return;
    }
    let Some(track) = dress.texture(style.track_role_name()) else {
        return;
    };
    paint_band_jambs(map, band, track);
}

/// Give every one-sided line whose front faces sector `band` the `track`
/// texture on its `middle` face.
fn paint_band_jambs(map: &mut Map, band: usize, track: &str) {
    let faces: Vec<usize> = map
        .linedefs
        .iter()
        .filter(|line| line.back.is_none())
        .filter_map(|line| line.front)
        .filter(|&side| map.sidedefs.get(side).is_some_and(|row| row.sector == band))
        .collect();
    for side in faces {
        if let Some(row) = map.sidedefs.get_mut(side) {
            track.clone_into(&mut row.middle);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{
        CompiledPlan, FeatureDress, FeatureRow, FeatureRun, Plan, WadDoorStyle, apply_features,
        choose_switch_wall, doors, lift_join_rule, lift_side, remap_rows, remote_door,
        teleport_wall, wad_style,
    };
    use crate::geom::Pt;
    use crate::plan::compile::compile;
    use crate::plan::model::{
        Connection, ConnectionKind, DoorStyle, ExitStyle, Feature, KeyColor, Room, Shape, Side,
    };
    use crate::plan::textures::{TextureRole, TextureRoles};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::features::{LiftStyle, LightEffect, TELEPORT_DESTINATION_THING};
    use crate::wad::map::geometry::point_in_sector;
    use crate::wad::map::{Linedef, Map, Sidedef};
    use crate::wad::name::LumpName;

    /// The band sector of every single-door fixture map: the two rooms
    /// take sectors 0 and 1, the connector band follows.
    const BAND: usize = 2_usize;
    /// The tag a fresh map's first door takes.
    const TAG_ONE: i32 = 1_i32;
    /// The door lines of the two-room fixture.
    const TWO_DOOR_LINES: usize = 2_usize;
    /// The band's jamb faces inside the wall, in the fixture.
    const FOUR_JAMBS: usize = 4_usize;
    /// The `Hexen` `playeruse` activation bit.
    const PLAYER_USE: u32 = 0x400_u32;
    /// The `Hexen` `repeatspecial` bit.
    const REPEAT: u32 = 0x200_u32;
    /// The `Hexen` `monsteractivate` bit.
    const MONSTER: u32 = 0x2000_u32;

    /// A theme answering every dressing role with a distinct name.
    struct TestRoles;

    impl TextureRoles for TestRoles {
        fn texture(&self, role: TextureRole) -> Option<&str> {
            Some(match role {
                TextureRole::Floor => "FLOROLE",
                TextureRole::Ceiling => "CEIROLE",
                TextureRole::Wall => "WALROLE",
                TextureRole::Door => "DOOROLE",
                TextureRole::DoorFloor => "DFOROLE",
                TextureRole::DoorTrack => "DTRROLE",
                TextureRole::WindowSill => "WSIROLE",
                TextureRole::Step => "STEPROLE",
            })
        }
    }

    /// A parsed map name for test data.
    fn map_name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test map names are valid")
    }

    /// A rectangular room with default sector properties.
    fn rect_room(id: &str, left_bottom: (i64, i64), width_height: (i64, i64)) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Rect {
                x: left_bottom.0,
                y: left_bottom.1,
                w: width_height.0,
                h: width_height.1,
            },
            floor: 0,
            ceiling: 128,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// A named door connection from room `a` to room `b`, placed at
    /// `at`.
    fn door(id: &str, style: DoorStyle, at: (i64, i64)) -> Connection {
        Connection {
            id: Some(id.to_owned()),
            from: "a".to_owned(),
            to: "b".to_owned(),
            kind: ConnectionKind::Door { style },
            at: Some(at),
            width: 48_i64,
        }
    }

    /// Two 128 x 128 rooms sharing the line `x = 128`, joined by the
    /// given connections.
    fn two_rooms(connections: Vec<Connection>) -> Plan {
        Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (128, 0), (128, 128)),
            ],
            connections,
            features: Vec::new(),
            wall_thickness: 16_i64,
        }
    }

    /// A one-door plan: a `style` door low on the shared wall.
    fn door_plan(style: DoorStyle) -> Plan {
        two_rooms(vec![door("d1", style, (128_i64, 32_i64))])
    }

    /// The feature dress every test applies, colours distinct.
    fn dress() -> FeatureDress {
        FeatureDress {
            door: "DOOROLE".to_owned(),
            track: "DTRROLE".to_owned(),
            track_red: "REDTRAK".to_owned(),
            track_blue: "BLUETRAK".to_owned(),
            track_yellow: "YELTRAK".to_owned(),
            switch: "SWTROLE".to_owned(),
            exit_switch: "EXTPANE".to_owned(),
            step: "STEPROLE".to_owned(),
            wall: "WALROLE".to_owned(),
        }
    }

    /// Compile a plan in the `Doom` family and run the features: the
    /// wired plan and its rows.
    #[track_caller]
    fn wired(plan: &Plan) -> (CompiledPlan, Vec<FeatureRow>) {
        wired_dressed(plan, MapFormat::Doom, &dress())
    }

    /// Compile a plan in `format` and run the features dressed by
    /// `themed`.
    #[track_caller]
    fn wired_dressed(
        plan: &Plan,
        format: MapFormat,
        themed: &FeatureDress,
    ) -> (CompiledPlan, Vec<FeatureRow>) {
        let mut compiled =
            compile(plan, map_name("MAP01"), format, &TestRoles).expect("the fixture compiles");
        let rows = apply_features(plan, &mut compiled, themed).expect("features apply");
        (compiled, rows)
    }

    /// The line `index` of the map.
    fn line_at(map: &Map, index: usize) -> &Linedef {
        map.linedefs.get(index).expect("test indices")
    }

    /// The sector behind a line's front sidedef.
    fn front_sector(map: &Map, line: &Linedef) -> usize {
        let front = line.front.expect("emitted lines have a front");
        map.sidedefs.get(front).expect("test rows").sector
    }

    /// The sector behind a line's back sidedef, `None` when one-sided.
    fn back_sector(map: &Map, line: &Linedef) -> Option<usize> {
        let back = line.back?;
        Some(map.sidedefs.get(back)?.sector)
    }

    /// The action a wired use line must carry: `special`, `tag` and
    /// `args`.
    type UseAction = (i32, i32, [i32; 5]);

    /// Assert the two-sided `index`th line is a use line facing its
    /// room — the front side belongs to a room sector, the back to the
    /// band — and carries `action`.
    #[track_caller]
    fn assert_use_line(map: &Map, index: usize, action: UseAction) {
        let (special, tag, args) = action;
        let line = line_at(map, index);
        assert_eq!(line.special, special, "the door special of line {index}");
        assert_eq!(line.tag, tag, "the shared tag of line {index}");
        assert_eq!(line.args, args, "the arguments of line {index}");
        assert_eq!(back_sector(map, line), Some(BAND), "the band sits behind");
        assert!(
            matches!(front_sector(map, line), 0_usize | 1_usize),
            "the front faces the room on line {index}, not {}",
            front_sector(map, line)
        );
    }

    /// The band's one-sided jamb faces of `map`: their sidedef rows.
    fn band_jambs(map: &Map) -> Vec<&Sidedef> {
        map.linedefs
            .iter()
            .filter(|line| line.back.is_none() && front_sector(map, line) == BAND)
            .map(|line| {
                let front = line.front.expect("one-sided lines have a front");
                map.sidedefs.get(front).expect("test rows")
            })
            .collect()
    }

    /// Assert one `Doom`-family door wiring: the row, the tagged band
    /// and every door line facing its room with vanilla `special`.
    #[track_caller]
    fn assert_doom_door(style: DoorStyle, special: i32) {
        let (compiled, rows) = wired(&door_plan(style));
        let row = rows.first().expect("one door row");
        assert_eq!(row.kind, "door", "the class of {style:?}");
        assert_eq!(row.room, None, "a door belongs to the wall, not a room");
        assert_eq!(row.sectors, vec![BAND], "the band sector is reported");
        assert_eq!(row.tag, Some(TAG_ONE), "the shared tag of {style:?}");
        assert_eq!(
            row.lines.len(),
            TWO_DOOR_LINES,
            "the band meets each room once for {style:?}"
        );
        let band = compiled.map.sectors.get(BAND).expect("the band sector");
        assert_eq!(band.tag, TAG_ONE, "{style:?} stamps the shared tag");
        assert_eq!(
            band.special, 0_i32,
            "the Doom action lives on the lines, not the sector"
        );
        for &index in &row.lines {
            assert_use_line(&compiled.map, index, (special, TAG_ONE, [0_i32; 5]));
        }
    }

    #[test]
    fn every_doom_door_style_wires_its_special_and_shared_tag() {
        let table = [
            (DoorStyle::Normal, 1_i32),
            (DoorStyle::Fast, 117_i32),
            (DoorStyle::StayOpen, 31_i32),
            (DoorStyle::Locked(KeyColor::Red), 28_i32),
            (DoorStyle::Locked(KeyColor::Blue), 26_i32),
            (DoorStyle::Locked(KeyColor::Yellow), 27_i32),
        ];
        for (style, special) in table {
            assert_doom_door(style, special);
        }
    }

    #[test]
    fn a_remote_door_stamps_its_sector_and_writes_nothing_on_the_lines() {
        let (compiled, rows) = wired(&door_plan(DoorStyle::Remote));
        let row = rows.first().expect("one door row");
        assert_eq!(
            row.tag,
            Some(TAG_ONE),
            "the tagged sector is the remote's handle"
        );
        assert_eq!(row.lines.len(), TWO_DOOR_LINES, "the lines are reported");
        let band = compiled.map.sectors.get(BAND).expect("the band sector");
        assert_eq!(band.tag, TAG_ONE, "the sector carries the tag");
        for &index in &row.lines {
            let line = line_at(&compiled.map, index);
            assert_eq!(line.special, 0_i32, "a remote door keeps line {index} bare");
            assert_eq!(line.tag, 0_i32, "and untagged");
            assert_eq!(line.args, [0_i32; 5], "with no arguments");
        }
    }

    /// Assert one `Hexen`-family door wiring: every line carries the
    /// script `action` and the style's activation `bits`.
    #[track_caller]
    fn assert_hexen_door(style: DoorStyle, action: UseAction, bits: u32) {
        let (compiled, rows) = wired_dressed(&door_plan(style), MapFormat::Hexen, &dress());
        let row = rows.first().expect("one door row");
        for &index in &row.lines {
            assert_use_line(&compiled.map, index, action);
            let flags = line_at(&compiled.map, index).flags;
            assert_eq!(flags & PLAYER_USE, PLAYER_USE, "{style:?} fires on use");
            assert_eq!(
                flags & 0x1C00_u32,
                PLAYER_USE,
                "the activation field holds playeruse alone for {style:?}"
            );
            assert_eq!(flags & bits, bits, "the {style:?} activation bits");
        }
    }

    #[test]
    fn a_hexen_door_takes_the_script_action_and_playeruse_activation() {
        assert_hexen_door(
            DoorStyle::Normal,
            (12_i32, 0_i32, [0_i32, 16_i32, 150_i32, 0_i32, 0_i32]),
            PLAYER_USE | REPEAT | MONSTER,
        );
        assert_hexen_door(
            DoorStyle::Locked(KeyColor::Blue),
            (13_i32, 0_i32, [0_i32, 16_i32, 150_i32, 130_i32, 0_i32]),
            PLAYER_USE | REPEAT,
        );
    }

    /// Assert one row of the two-door fixture: its `tag`, its `band`
    /// sector answering the same tag and its lines sharing it.
    #[track_caller]
    fn assert_tagged_row(compiled: &CompiledPlan, row: &FeatureRow, band: usize, tag: i32) {
        assert_eq!(row.sectors, vec![band], "the {band}th sector is the band");
        assert_eq!(row.tag, Some(tag), "connection order tags {tag}");
        let space = compiled.map.sectors.get(band).expect("the band");
        assert_eq!(space.tag, tag, "the band answers its door's tag");
        assert!(
            row.lines
                .iter()
                .all(|&line| line_at(&compiled.map, line).tag == tag),
            "the lines share tag {tag}"
        );
    }

    #[test]
    fn two_doors_take_consecutive_tags_in_connection_order() {
        let plan = two_rooms(vec![
            door("first", DoorStyle::Normal, (128_i64, 32_i64)),
            door(
                "second",
                DoorStyle::Locked(KeyColor::Red),
                (128_i64, 96_i64),
            ),
        ]);
        let (compiled, rows) = wired(&plan);
        assert_eq!(rows.len(), 2, "both doors report a row");
        assert_tagged_row(&compiled, rows.first().expect("two rows"), BAND, 1_i32);
        assert_tagged_row(&compiled, rows.get(1).expect("two rows"), 3_usize, 2_i32);
    }

    #[test]
    fn the_run_records_each_doors_tag_under_its_connector_id() {
        let plan = two_rooms(vec![
            door("first", DoorStyle::Remote, (128_i64, 32_i64)),
            door("second", DoorStyle::Normal, (128_i64, 96_i64)),
        ]);
        let mut compiled =
            compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
        let mut run = FeatureRun::default();
        doors(&mut compiled, &dress(), &mut run).expect("the doors wire");
        assert_eq!(
            run.tags.get("first"),
            Some(&1_i32),
            "the switch slice's lookup"
        );
        assert_eq!(run.tags.get("second"), Some(&2_i32));
        assert_eq!(run.rows.len(), 2, "both rows landed");
    }

    /// Assert a locked door's band jamb middles wear the coloured
    /// `track` the dress names for its key.
    #[track_caller]
    fn assert_locked_jambs(style: DoorStyle, track: &str) {
        let (compiled, _) = wired(&door_plan(style));
        let jambs = band_jambs(&compiled.map);
        assert_eq!(jambs.len(), FOUR_JAMBS, "the band's four reveal edges");
        assert!(
            jambs.iter().all(|row| row.middle == track),
            "{style:?} jambs wear {track}: {:?}",
            jambs
                .iter()
                .map(|row| row.middle.as_str())
                .collect::<Vec<&str>>()
        );
    }

    #[test]
    fn a_locked_doors_jambs_wear_their_coloured_track() {
        assert_locked_jambs(DoorStyle::Locked(KeyColor::Red), "REDTRAK");
        assert_locked_jambs(DoorStyle::Locked(KeyColor::Blue), "BLUETRAK");
        assert_locked_jambs(DoorStyle::Locked(KeyColor::Yellow), "YELTRAK");
    }

    #[test]
    fn plain_doors_keep_the_compilers_track() {
        let plain = [
            DoorStyle::Normal,
            DoorStyle::Fast,
            DoorStyle::StayOpen,
            DoorStyle::Remote,
        ];
        for style in plain {
            let (compiled, _) = wired(&door_plan(style));
            let jambs = band_jambs(&compiled.map);
            assert_eq!(jambs.len(), FOUR_JAMBS, "{style:?} keeps its four jambs");
            assert!(
                jambs.iter().all(|row| row.middle == "DTRROLE"),
                "{style:?} keeps the plain track the compiler dressed"
            );
        }
    }

    #[test]
    fn an_unnamed_colour_leaves_the_locked_jambs_dressed_plain() {
        let themed = FeatureDress {
            track_blue: "-".to_owned(),
            ..dress()
        };
        let (compiled, _) = wired_dressed(
            &door_plan(DoorStyle::Locked(KeyColor::Blue)),
            MapFormat::Doom,
            &themed,
        );
        let jambs = band_jambs(&compiled.map);
        assert!(
            jambs.iter().all(|row| row.middle == "DTRROLE"),
            "the \"-\" role keeps the compiler's track"
        );
    }

    #[test]
    fn non_door_connectors_install_nothing() {
        let mut passage = door("d1", DoorStyle::Normal, (128_i64, 32_i64));
        passage.kind = ConnectionKind::Opening;
        let (compiled, rows) = wired(&two_rooms(vec![passage]));
        assert!(rows.is_empty(), "an opening is no feature");
        let info = compiled.connector_sectors.first().expect("the opening");
        assert!(
            info.door_lines
                .iter()
                .all(|&index| line_at(&compiled.map, index).special == 0_i32),
            "an opening's lines stay bare"
        );
    }

    #[test]
    fn a_door_band_painted_over_earns_no_row_and_stays_untouched() {
        let plan = door_plan(DoorStyle::Normal);
        let mut compiled =
            compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
        if let Some(info) = compiled.connector_sectors.first_mut() {
            info.sectors.clear();
        }
        let before = compiled.map.clone();
        let rows = apply_features(&plan, &mut compiled, &dress()).expect("nothing to wire");
        assert!(rows.is_empty(), "no sector, no row");
        assert_eq!(compiled.map, before, "the map stays as it was");
    }

    #[test]
    fn a_band_the_compiled_map_does_not_hold_refuses_the_run() {
        let plan = door_plan(DoorStyle::Fast);
        let mut compiled =
            compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
        if let Some(info) = compiled.connector_sectors.first_mut() {
            info.sectors = vec![99_usize];
        }
        let error =
            apply_features(&plan, &mut compiled, &dress()).expect_err("the band sector is foreign");
        assert_eq!(error.code(), "internal", "a contract break stays internal");
        let message = error.to_string();
        assert!(
            message.contains("d1") && message.contains("99"),
            "the connector and the sector are named: {message}"
        );
    }

    #[test]
    fn a_reclassified_connector_reports_no_row() {
        let plan = door_plan(DoorStyle::Normal);
        let (mut compiled, _) = wired(&plan);
        if let Some(info) = compiled.connector_sectors.first_mut() {
            info.kind = ConnectionKind::Opening;
        }
        let rows = apply_features(&plan, &mut compiled, &dress())
            .expect("a non-door connector is skipped");
        assert!(rows.is_empty(), "no door kind, no door row");
    }

    #[test]
    fn dress_resolves_tracks_by_the_doors_role_names() {
        let themed = dress();
        assert_eq!(themed.texture("door"), Some("DOOROLE"));
        assert_eq!(themed.texture("door_track"), Some("DTRROLE"));
        assert_eq!(themed.texture("door_track_red"), Some("REDTRAK"));
        assert_eq!(themed.texture("door_track_blue"), Some("BLUETRAK"));
        assert_eq!(themed.texture("door_track_yellow"), Some("YELTRAK"));
        assert_eq!(themed.texture("switch"), Some("SWTROLE"));
        assert_eq!(themed.texture("exit_switch"), Some("EXTPANE"));
        assert_eq!(themed.texture("step"), Some("STEPROLE"));
        assert_eq!(themed.texture("wall"), Some("WALROLE"));
        assert_eq!(
            themed.texture("teleport_floor"),
            None,
            "no such dressing field"
        );
        assert_eq!(themed.texture(""), None, "the empty name answers nothing");
        let bare = FeatureDress::default();
        assert_eq!(
            bare.texture("door_track"),
            None,
            "unset fields answer nothing"
        );
    }

    #[test]
    fn plan_styles_map_onto_the_door_core() {
        let table = [
            (DoorStyle::Normal, WadDoorStyle::Normal),
            (DoorStyle::Fast, WadDoorStyle::Fast),
            (DoorStyle::StayOpen, WadDoorStyle::StayOpen),
            (DoorStyle::Locked(KeyColor::Red), WadDoorStyle::LockedRed),
            (DoorStyle::Locked(KeyColor::Blue), WadDoorStyle::LockedBlue),
            (
                DoorStyle::Locked(KeyColor::Yellow),
                WadDoorStyle::LockedYellow,
            ),
            (DoorStyle::Remote, WadDoorStyle::Remote),
        ];
        for (style, core) in table {
            assert_eq!(wad_style(style), core, "{style:?}");
        }
    }

    #[test]
    fn the_wired_map_still_holds_the_reported_geometry() {
        let (compiled, rows) = wired(&door_plan(DoorStyle::Normal));
        let info = compiled.connector_sectors.first().expect("the door");
        assert_eq!(rows.first().expect("one row").lines, info.door_lines);
        assert!(
            info.door_lines
                .iter()
                .all(|&index| line_at(&compiled.map, index).flags & 4_u32 == 4_u32),
            "use lines stay two-sided"
        );
    }
    /// A named plain opening between rooms `from` and `to`.
    fn opening(id: &str, from: &str, to: &str) -> Connection {
        Connection {
            id: Some(id.to_owned()),
            from: from.to_owned(),
            to: to.to_owned(),
            kind: ConnectionKind::Opening,
            at: None,
            width: 48_i64,
        }
    }

    /// A `switch` feature row.
    fn switch_feature(room: &str, opens: &str, wall: Option<Side>, repeatable: bool) -> Feature {
        Feature::Switch {
            room: room.to_owned(),
            opens: opens.to_owned(),
            wall,
            repeatable,
        }
    }

    /// An `exit` feature row.
    fn exit_feature(room: &str, style: ExitStyle, secret: bool, wall: Option<Side>) -> Feature {
        Feature::Exit {
            room: room.to_owned(),
            style,
            secret,
            wall,
        }
    }

    /// The two-room remote-door plan carrying `features`.
    fn door_features(features: Vec<Feature>) -> Plan {
        let mut plan = two_rooms(vec![door("d1", DoorStyle::Remote, (128_i64, 32_i64))]);
        plan.features = features;
        plan
    }

    /// Two stacked 128 x 128 rooms joined by a remote door off-centre:
    /// the lower room's north wall comes out as two equal 36-unit
    /// segments either side of the band.
    fn stacked_doors() -> Plan {
        let band = Connection {
            id: Some("d1".to_owned()),
            from: "a".to_owned(),
            to: "b".to_owned(),
            kind: ConnectionKind::Door {
                style: DoorStyle::Remote,
            },
            at: Some((64_i64, 128_i64)),
            width: 48_i64,
        };
        Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (0, 128), (128, 128)),
            ],
            connections: vec![band],
            features: Vec::new(),
            wall_thickness: 16_i64,
        }
    }

    /// Three 128 x 128 rooms in a row at zero thickness, joined by
    /// openings: the middle room owns no one-sided east or west wall.
    fn three_in_row() -> Plan {
        Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (128, 0), (128, 128)),
                rect_room("c", (256, 0), (128, 128)),
            ],
            connections: vec![opening("o1", "a", "b"), opening("o2", "b", "c")],
            features: Vec::new(),
            wall_thickness: 0_i64,
        }
    }

    /// The two endpoint coordinates of a linedef.
    type Corners = ((i32, i32), (i32, i32));

    /// The `(x, y)` endpoints of linedef `index`.
    fn ends(map: &Map, index: usize) -> Corners {
        let line = map.linedefs.get(index).expect("test index");
        let first = map.vertices.get(line.v1).expect("test vertex");
        let second = map.vertices.get(line.v2).expect("test vertex");
        ((first.x, first.y), (second.x, second.y))
    }

    /// The front side's `middle` texture of linedef `line`.
    fn front_middle(map: &Map, line: usize) -> String {
        let side = map
            .linedefs
            .get(line)
            .expect("test index")
            .front
            .expect("mounted lines keep a front");
        map.sidedefs.get(side).expect("test row").middle.clone()
    }

    /// The `index`th line's special and tag.
    fn action_of_line(map: &Map, index: usize) -> (i32, i32) {
        let line = map.linedefs.get(index).expect("test index");
        (line.special, line.tag)
    }

    /// Room `a`'s chosen wall in the two-room remote-door fixture.
    fn a_wall(map: &Map, side: Option<Side>) -> usize {
        choose_switch_wall(map, &[0], side, "a").expect("a wall on the asked side")
    }

    #[test]
    fn the_wall_choice_follows_the_compass_side() {
        let plan = two_rooms(vec![door("d1", DoorStyle::Remote, (128_i64, 32_i64))]);
        let (compiled, _) = wired(&plan);
        let map = &compiled.map;
        assert_eq!(
            ends(map, a_wall(map, Some(Side::North))),
            ((0_i32, 128_i32), (120_i32, 128_i32)),
            "the north wall answers north"
        );
        assert_eq!(
            ends(map, a_wall(map, Some(Side::South))),
            ((120_i32, 0_i32), (0_i32, 0_i32)),
            "the south wall answers south"
        );
        assert_eq!(
            ends(map, a_wall(map, Some(Side::West))),
            ((0_i32, 0_i32), (0_i32, 128_i32)),
            "the west wall answers west"
        );
    }

    #[test]
    fn the_longest_line_wins_within_the_chosen_side() {
        let plan = two_rooms(vec![door("d1", DoorStyle::Remote, (128_i64, 32_i64))]);
        let (compiled, _) = wired(&plan);
        let map = &compiled.map;
        let east = choose_switch_wall(map, &[0], Some(Side::East), "a").expect("an east wall");
        let ((x1, y1), (x2, y2)) = ends(map, east);
        assert_eq!((x1, x2), (120_i32, 120_i32), "the band-cut east wall");
        assert_eq!(
            y1.abs_diff(y2),
            72_u32,
            "the long of the two segments (the door band left an 8-unit stub below)"
        );
    }

    #[test]
    fn without_a_side_the_longest_wall_wins() {
        let plan = two_rooms(vec![door("d1", DoorStyle::Remote, (128_i64, 32_i64))]);
        let (compiled, _) = wired(&plan);
        let map = &compiled.map;
        assert_eq!(
            ends(map, a_wall(map, None)),
            ((0_i32, 0_i32), (0_i32, 128_i32)),
            "the untouched 128-unit west wall outsides the 120-unit north and south"
        );
    }

    #[test]
    fn equal_walls_fall_to_the_lowest_index() {
        let (compiled, _) = wired(&stacked_doors());
        let map = &compiled.map;
        let tied: Vec<usize> = (0..map.linedefs.len())
            .filter(|&index| {
                let ((x1, y1), (x2, y2)) = ends(map, index);
                y1 == y2 && y1 == 120_i32 && x1.abs_diff(x2) == 40_u32
            })
            .collect();
        assert_eq!(tied.len(), 2, "the centred band leaves two equal segments");
        let chosen = choose_switch_wall(map, &[0], Some(Side::North), "a")
            .expect("two equal north segments");
        assert_eq!(
            chosen,
            *tied.iter().min().expect("two tied candidates"),
            "ties go to the lowest linedef index"
        );
    }

    #[test]
    fn a_side_without_a_wall_names_the_room_and_the_side() {
        let (compiled, _) = wired(&three_in_row());
        let error = choose_switch_wall(&compiled.map, &[1], Some(Side::East), "b")
            .expect_err("room b meets room c on its east side");
        assert_eq!(error.code(), "invalid_argument", "a wall-less side");
        let message = error.to_string();
        assert!(
            message.contains("'b'") && message.contains("east"),
            "the refusal names room and side: {message}"
        );
    }

    #[test]
    fn a_switch_on_a_plain_door_is_refused() {
        let mut plan = two_rooms(vec![door("d1", DoorStyle::Normal, (128_i64, 32_i64))]);
        plan.features = vec![switch_feature("a", "d1", Some(Side::West), false)];
        let error = refuse_features(&plan);
        assert!(
            error
                .to_string()
                .contains("switch opens d1 which is not a remote door"),
            "the refusal words: {error}"
        );
    }

    #[test]
    fn a_remote_door_the_wiring_never_tagged_is_no_switch_target() {
        let plan = door_features(Vec::new());
        let error = remote_door(&plan, &FeatureRun::default(), "d1")
            .expect_err("a remote row with no wired band");
        assert!(
            error
                .to_string()
                .contains("switch opens d1 which is not a remote door"),
            "unwired reads as not-a-target: {error}"
        );
    }

    /// Compile `plan` and require the feature run to refuse.
    #[track_caller]
    fn refuse_features(plan: &Plan) -> crate::error::CyberError {
        let mut compiled =
            compile(plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
        let error = apply_features(plan, &mut compiled, &dress());
        error.expect_err("the feature must be refused")
    }

    #[test]
    fn a_feature_in_a_painted_over_room_names_it() {
        let plan = Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (0, 0), (128, 128)),
            ],
            connections: Vec::new(),
            features: vec![exit_feature("a", ExitStyle::WalkOver, false, None)],
            wall_thickness: 16_i64,
        };
        plan.validate().expect("the plan validates");
        let message = refuse_features(&plan).to_string();
        assert!(
            message.contains("room 'a'") && message.contains("painted over"),
            "the refusal names the vanished room: {message}"
        );
    }

    #[test]
    fn the_switch_centers_on_the_wall_and_fires_the_doors_own_tag() {
        let plan = door_features(vec![switch_feature("a", "d1", Some(Side::West), false)]);
        let (compiled, rows) = wired(&plan);
        let map = &compiled.map;
        let row = rows
            .iter()
            .find(|r| r.kind == "switch")
            .expect("a switch row");
        assert_eq!(row.room.as_deref(), Some("a"), "the room it stands in");
        assert_eq!(row.sectors, vec![BAND], "the door band it fires");
        assert_eq!(
            row.tag,
            Some(TAG_ONE),
            "the door's tag answers, no second mint"
        );
        assert_eq!(row.lines.len(), 1, "one switch line");
        let line = row.lines.first().copied().expect("one line");
        assert_eq!(
            action_of_line(map, line),
            (103_i32, TAG_ONE),
            "S1 opens the tagged door"
        );
        assert_eq!(
            front_middle(map, line),
            "SWTROLE",
            "the theme's switch panel"
        );
        let ((x1, y1), (x2, y2)) = ends(map, line);
        assert_eq!((x1, x2), (0_i32, 0_i32), "still on the west wall");
        assert_eq!(
            (y1.min(y2), y1.max(y2)),
            (48_i32, 80_i32),
            "the 32-unit panel centred on the wall's midpoint"
        );
        assert_eq!(
            map.sectors
                .iter()
                .filter(|space| space.tag != 0_i32)
                .count(),
            1,
            "only the band carries a tag: the switch reused the door's"
        );
    }

    #[test]
    fn a_repeatable_switch_takes_the_sr_special() {
        let plan = door_features(vec![switch_feature("a", "d1", None, true)]);
        let (compiled, rows) = wired(&plan);
        let row = rows
            .iter()
            .find(|r| r.kind == "switch")
            .expect("a switch row");
        let line = row.lines.first().copied().expect("one line");
        assert_eq!(
            action_of_line(&compiled.map, line),
            (61_i32, TAG_ONE),
            "SR keeps working after use"
        );
    }

    #[test]
    fn an_unnamed_switch_panel_keeps_the_wall_texture() {
        let themed = FeatureDress {
            switch: "-".to_owned(),
            ..dress()
        };
        let plan = door_features(vec![switch_feature("a", "d1", Some(Side::West), false)]);
        let (compiled, rows) = wired_dressed(&plan, MapFormat::Doom, &themed);
        let row = rows
            .iter()
            .find(|r| r.kind == "switch")
            .expect("a switch row");
        let line = row.lines.first().copied().expect("one line");
        assert_eq!(
            front_middle(&compiled.map, line),
            "WALROLE",
            "the \"-\" answer keeps the wall's own middle"
        );
        assert_eq!(
            action_of_line(&compiled.map, line).0,
            103_i32,
            "the action still lands"
        );
    }

    #[test]
    fn the_exit_switch_carries_the_level_end() {
        let plan = door_features(vec![exit_feature(
            "b",
            ExitStyle::Switch,
            false,
            Some(Side::East),
        )]);
        let (compiled, rows) = wired(&plan);
        let map = &compiled.map;
        let row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
        assert_eq!(row.room.as_deref(), Some("b"));
        assert_eq!(row.sectors, vec![1_usize], "the room it stands in");
        assert_eq!(row.tag, None, "an exit acts on no tag");
        let line = row.lines.first().copied().expect("one line");
        assert_eq!(action_of_line(map, line), (11_i32, 0_i32), "the S1 exit");
        assert_eq!(front_middle(map, line), "EXTPANE", "the theme's exit panel");
    }

    #[test]
    fn a_secret_exit_switch_takes_the_s1_secret() {
        let plan = door_features(vec![exit_feature("b", ExitStyle::Switch, true, None)]);
        let (compiled, rows) = wired(&plan);
        let row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
        let line = row.lines.first().copied().expect("one line");
        assert_eq!(
            action_of_line(&compiled.map, line).0,
            51_i32,
            "the S1 secret exit"
        );
    }

    #[test]
    fn a_walk_over_exit_dresses_the_rooms_two_sided_boundaries() {
        let mut plan = three_in_row();
        plan.features = vec![exit_feature("b", ExitStyle::WalkOver, false, None)];
        let (compiled, rows) = wired(&plan);
        let map = &compiled.map;
        let expected: Vec<usize> = map
            .linedefs
            .iter()
            .enumerate()
            .filter_map(|(index, line)| boundary_of(map, line, 1_usize).then_some(index))
            .collect();
        assert!(!expected.is_empty(), "the fixture shares walls");
        let row = rows.iter().find(|r| r.kind == "exit").expect("an exit row");
        assert_eq!(row.lines, expected, "every two-sided boundary of the room");
        assert_eq!(row.sectors, vec![1_usize]);
        assert_eq!(row.tag, None);
        for &index in &expected {
            assert_eq!(
                action_of_line(map, index),
                (52_i32, 0_i32),
                "W1 exits keep the bare tags"
            );
        }
    }

    /// Whether `line` is two-sided with sector `sector` on one side.
    fn boundary_of(map: &Map, line: &Linedef, sector: usize) -> bool {
        let side = |slot: Option<usize>| {
            slot.and_then(|index| map.sidedefs.get(index))
                .map(|row| row.sector)
        };
        let (Some(front), Some(back)) = (side(line.front), side(line.back)) else {
            return false;
        };
        front == sector || back == sector
    }

    /// A named connection of `kind` between rooms `from` and `to`,
    /// centred on their shared boundary.
    fn join(id: &str, from: &str, to: &str, kind: ConnectionKind) -> Connection {
        Connection {
            id: Some(id.to_owned()),
            from: from.to_owned(),
            to: to.to_owned(),
            kind,
            at: None,
            width: 64_i64,
        }
    }

    /// A `lift` feature in room `room`: a use-wall ride, repeating when
    /// `repeatable`.
    fn lift_feature(room: &str, repeatable: bool) -> Feature {
        Feature::Lift {
            room: room.to_owned(),
            low_floor: None,
            style: LiftStyle::UseWall,
            repeatable,
        }
    }

    /// A `teleporter` feature from room `from` to room `to` on the
    /// rooms' default centres.
    fn teleporter_feature(from: &str, to: &str, angle: i32, two_way: bool) -> Feature {
        Feature::Teleporter {
            from_room: from.to_owned(),
            to_room: to.to_owned(),
            pad: None,
            destination: None,
            angle,
            two_way,
        }
    }

    /// A `teleporter` feature with explicit square centres.
    fn teleporter_at(from: &str, to: &str, pad: Option<Pt>, destination: Option<Pt>) -> Feature {
        Feature::Teleporter {
            from_room: from.to_owned(),
            to_room: to.to_owned(),
            pad,
            destination,
            angle: 0_i32,
            two_way: false,
        }
    }

    /// Two stacked 128 x 128 rooms touching at `y = 128` with no wall
    /// between them, joined by a `kind` connection, the upper room `b`
    /// standing 64 units above `a` and carrying a default use-wall lift.
    fn lift_fixture(kind: ConnectionKind) -> Plan {
        let mut upper = rect_room("b", (0, 128), (128, 128));
        upper.floor = 64_i32;
        Plan {
            rooms: vec![rect_room("a", (0, 0), (128, 128)), upper],
            connections: vec![join("c1", "b", "a", kind)],
            features: vec![lift_feature("b", true)],
            wall_thickness: 0_i64,
        }
    }

    /// The two-room fixture joined by a plain opening, wearing
    /// `features`.
    fn teleport_fixture(features: Vec<Feature>) -> Plan {
        let mut plan = two_rooms(vec![opening("o1", "a", "b")]);
        plan.features = features;
        plan
    }

    /// The `lower` band worn by `line`'s front sidedef.
    fn front_lower(map: &Map, line: usize) -> String {
        let side = map
            .linedefs
            .get(line)
            .expect("test index")
            .front
            .expect("a front side");
        map.sidedefs.get(side).expect("test row").lower.clone()
    }

    /// The indices of every line carrying `special` and line tag `tag`.
    fn lines_wearing(map: &Map, special: i32, tag: i32) -> Vec<usize> {
        map.linedefs
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                (line.special == special && line.tag == tag).then_some(index)
            })
            .collect()
    }

    /// The teleport-destination things (type 14) of the map.
    fn destination_things(map: &Map) -> Vec<&crate::wad::map::model::Thing> {
        map.things
            .iter()
            .filter(|thing| thing.kind == TELEPORT_DESTINATION_THING)
            .collect()
    }

    /// Assert the one lift row of `compiled`: room `b`'s sector tagged,
    /// both shared-wall segments turned front-down onto room `a` and
    /// carrying `special` with the row's tag.
    #[track_caller]
    fn assert_doom_lift(compiled: &CompiledPlan, row: &FeatureRow, special: i32) {
        assert_eq!(row.kind, "lift", "the lift row");
        assert_eq!(row.room.as_deref(), Some("b"), "the platform's room");
        let platform = row.sectors.first().copied().expect("one platform");
        assert_eq!(
            row.sectors,
            compiled.room_sectors.get("b").expect("room b").clone(),
            "the row names the room's own sector"
        );
        let tag = row.tag.expect("a fresh tag");
        let space = compiled.map.sectors.get(platform).expect("the platform");
        assert_eq!(space.tag, tag, "the platform answers with the tag");
        assert_eq!(
            space.floor_flat, "FLOROLE",
            "the ride keeps the room's flat"
        );
        assert_eq!(row.lines.len(), 2, "the two segments of the shared wall");
        let lower = compiled
            .room_sectors
            .get("a")
            .expect("room a")
            .first()
            .expect("one sector");
        for &line in &row.lines {
            let def = line_at(&compiled.map, line);
            assert_eq!(def.special, special, "the lift action on line {line}");
            assert_eq!(def.tag, tag, "line {line} answers with the platform tag");
            assert_eq!(back_sector(&compiled.map, def), Some(platform));
            assert_eq!(
                front_sector(&compiled.map, def),
                *lower,
                "the front looks down"
            );
            assert_eq!(
                front_lower(&compiled.map, line),
                "STEPROLE",
                "the side wears step"
            );
        }
    }

    #[test]
    fn a_plan_lift_tags_its_room_and_turns_the_down_facing_edges() {
        let (compiled, rows) = wired(&lift_fixture(ConnectionKind::Opening));
        assert_eq!(rows.len(), 1, "an opening wires no door");
        assert_doom_lift(&compiled, rows.first().expect("one row"), 62_i32);
    }

    #[test]
    fn the_once_only_doom_ride_takes_the_s1_special() {
        let mut plan = lift_fixture(ConnectionKind::Opening);
        plan.features = vec![lift_feature("b", false)];
        let (compiled, rows) = wired(&plan);
        assert_doom_lift(&compiled, rows.first().expect("one row"), 21_i32);
    }

    #[test]
    fn a_hexen_plan_lift_runs_the_scripted_lip_plat() {
        let (compiled, rows) = wired_dressed(
            &lift_fixture(ConnectionKind::Opening),
            MapFormat::Hexen,
            &dress(),
        );
        let row = rows.first().expect("one lift row");
        let tag = row.tag.expect("a fresh tag");
        let platform = row.sectors.first().copied().expect("one platform");
        let space = compiled.map.sectors.get(platform).expect("the platform");
        assert_eq!(space.tag, tag, "the sector keeps the tag the args carry");
        assert_eq!(row.lines.len(), 2, "both shared-wall segments");
        for &line in &row.lines {
            let def = line_at(&compiled.map, line);
            assert_eq!(def.special, 206_i32, "Plat_DownWaitUpStayLip");
            assert_eq!(
                def.args,
                [tag, 32_i32, 105_i32, 0_i32, 0_i32],
                "tag, speed, wait"
            );
            assert_eq!(def.tag, 0_i32, "a Hexen line carries no tag");
            assert_eq!(
                def.flags & 0x600_u32,
                0x600_u32,
                "playeruse and repeatspecial"
            );
        }
    }

    #[test]
    fn a_short_plan_lift_notes_that_stairs_will_do() {
        let mut plan = lift_fixture(ConnectionKind::Opening);
        if let Some(upper) = plan.rooms.get_mut(1) {
            upper.floor = 32_i32;
        }
        let (compiled, rows) = wired(&plan);
        assert_eq!(
            rows.first().expect("one lift row").lines.len(),
            2_usize,
            "both shared-wall segments ride"
        );
        assert!(
            compiled
                .warnings
                .iter()
                .any(|note| note.contains("a lift is not needed")),
            "the small-drop note passes through: {:?}",
            compiled.warnings
        );
    }

    #[test]
    fn a_lift_refuses_a_lower_room_joined_by_a_door() {
        let plan = lift_fixture(ConnectionKind::Door {
            style: DoorStyle::Normal,
        });
        let message = refuse_features(&plan).to_string();
        assert!(
            message.contains("a lift needs openings to the lower rooms"),
            "the refusal words: {message}"
        );
        assert!(
            message.contains("lift room 'b'")
                && message.contains("lower room 'a'")
                && message.contains("'c1'")
                && message.contains("kind door"),
            "room, neighbour, connection and kind are named: {message}"
        );
    }

    #[test]
    fn a_door_to_a_higher_room_passes_the_lift_rule() {
        let mut plan = lift_fixture(ConnectionKind::Opening);
        let mut tower = rect_room("c", (0, 256), (128, 128));
        tower.floor = 128_i32;
        tower.ceiling = 256_i32;
        plan.rooms.push(tower);
        plan.connections.push(join(
            "c2",
            "b",
            "c",
            ConnectionKind::Door {
                style: DoorStyle::Normal,
            },
        ));
        let (_, rows) = wired(&plan);
        let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            vec!["door", "lift"],
            "the door to a higher room joins fine: {kinds:?}"
        );
    }

    #[test]
    fn the_lift_rule_ignores_a_room_the_plan_does_not_hold() {
        let plan = lift_fixture(ConnectionKind::Opening);
        lift_join_rule(&plan, "ghost").expect("no row, no rule");
    }

    #[test]
    fn a_room_owning_several_sectors_refuses_the_plan_lift() {
        let plan = lift_fixture(ConnectionKind::Opening);
        for (count, owned) in [(2_usize, vec![1_usize, 2_usize]), (0_usize, Vec::new())] {
            let mut compiled = compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles)
                .expect("it compiles");
            compiled.room_sectors.insert("b".to_owned(), owned);
            let error = apply_features(&plan, &mut compiled, &dress())
                .expect_err("the platform must be one sector");
            assert_eq!(error.code(), "invalid_argument", "the guard refusal");
            let message = error.to_string();
            assert!(
                message.contains("'b'") && message.contains(&format!("owns {count} sectors")),
                "the room and the count are named: {message}"
            );
        }
    }

    #[test]
    fn an_unnamed_step_falls_the_lift_side_back_to_the_wall() {
        let themed = FeatureDress {
            step: "-".to_owned(),
            ..dress()
        };
        assert_eq!(
            lift_side(&themed),
            "WALROLE",
            "the fill pass's name rides on"
        );
        let bare = FeatureDress::default();
        assert_eq!(
            lift_side(&bare),
            "-",
            "no theme at all leaves the band bare"
        );
        assert_eq!(teleport_wall(&themed), "WALROLE");
        assert_eq!(teleport_wall(&bare), "-", "an unnamed wall stays bare");
    }

    #[test]
    fn the_plan_teleporter_dresses_its_pad_to_the_tagged_destination() {
        let plan = teleport_fixture(vec![teleporter_feature("a", "b", 45, false)]);
        let (compiled, rows) = wired(&plan);
        let row = rows.first().expect("one teleporter row");
        assert_eq!(row.kind, "teleporter");
        assert_eq!(row.room.as_deref(), Some("a"), "the pad's room");
        assert_eq!(row.sectors.len(), 2, "pad then destination");
        let pad = row.sectors.first().copied().expect("the pad sector");
        let dest = row.sectors.get(1).copied().expect("the destination sector");
        let tag = row.tag.expect("a fresh tag");
        assert_eq!(row.lines.len(), 4, "a border line on every side");
        for &line in &row.lines {
            let def = line_at(&compiled.map, line);
            assert_eq!(def.special, 97_i32, "the WR teleport on line {line}");
            assert_eq!(def.tag, tag, "answering with the destination tag");
        }
        let dest_space = compiled.map.sectors.get(dest).expect("the destination");
        assert_eq!(
            dest_space.tag, tag,
            "the Doom search reads the tagged sector"
        );
        let pad_space = compiled.map.sectors.get(pad).expect("the pad");
        assert_eq!(pad_space.tag, 0_i32, "a one-way pad carries no tag");
        assert_eq!(
            pad_space.floor_flat, "FLOROLE",
            "the pad keeps the room's flat"
        );
        let things = destination_things(&compiled.map);
        assert_eq!(things.len(), 1, "one-way stands one thing");
        let thing = things.first().expect("the destination thing");
        assert_eq!(thing.angle, 45_i32, "the arrival facing");
        assert_eq!(
            point_in_sector(&compiled.map, i64::from(thing.x), i64::from(thing.y)),
            Some(dest),
            "the type-14 thing stands inside the destination square"
        );
    }

    #[test]
    fn the_two_way_plan_teleporter_runs_the_return_leg() {
        let plan = teleport_fixture(vec![teleporter_feature("a", "b", 90, true)]);
        let (compiled, rows) = wired(&plan);
        let row = rows.first().expect("one teleporter row");
        let tag = row.tag.expect("the outward tag");
        let pad = row.sectors.first().copied().expect("the pad sector");
        assert_eq!(
            compiled.map.sectors.get(pad).expect("the pad").tag,
            tag.saturating_add(1),
            "the Doom pad answers with the return tag"
        );
        assert_eq!(
            lines_wearing(&compiled.map, 97_i32, tag).len(),
            row.lines.len(),
            "the outward leg dresses only the pad's borders"
        );
        assert_eq!(
            lines_wearing(&compiled.map, 97_i32, tag.saturating_add(1)).len(),
            4_usize,
            "the return leg dresses the destination's borders"
        );
        let things = destination_things(&compiled.map);
        assert_eq!(things.len(), 2, "a thing on each end");
        assert_eq!(
            things.iter().map(|thing| thing.angle).collect::<Vec<i32>>(),
            vec![90_i32, 270_i32],
            "the pair looks at each other"
        );
    }

    #[test]
    fn a_teleporter_square_that_does_not_fit_names_its_room() {
        for (feature, needle) in [
            (
                teleporter_at("a", "b", Some((8_i64, 8_i64)), None),
                "the teleporter pad does not fit inside room 'a' with 8 units of clearance",
            ),
            (
                teleporter_at("a", "b", None, Some((128_i64, 64_i64))),
                "the teleporter destination does not fit inside room 'b' with 8 units of clearance",
            ),
        ] {
            let message = refuse_features(&teleport_fixture(vec![feature])).to_string();
            assert!(message.contains(needle), "{needle} must be in: {message}");
        }
    }

    #[test]
    fn an_explicit_teleporter_centre_lands_where_the_wire_put_it() {
        let plan = teleport_fixture(vec![teleporter_at("a", "b", None, Some((200_i64, 64_i64)))]);
        let (compiled, rows) = wired(&plan);
        let row = rows.first().expect("one teleporter row");
        let dest = row.sectors.get(1).copied().expect("the destination");
        let thing = destination_things(&compiled.map)
            .first()
            .expect("the destination thing")
            .to_owned();
        assert_eq!(
            (thing.x, thing.y),
            (200_i32, 64_i32),
            "the wire point stands"
        );
        assert_eq!(
            point_in_sector(&compiled.map, i64::from(thing.x), i64::from(thing.y)),
            Some(dest),
            "and its square is the reported destination"
        );
    }

    #[test]
    fn a_teleporter_centre_past_the_map_range_is_refused_by_role() {
        let plan = teleport_fixture(vec![teleporter_at(
            "a",
            "b",
            Some((0x0001_0000_0000_i64, 0_i64)),
            None,
        )]);
        let message = refuse_features(&plan).to_string();
        assert!(
            message.contains(
                "the teleporter pad point 4294967296, 0 is outside the map coordinate range"
            ),
            "the refusal names the point and its role: {message}"
        );
    }

    #[test]
    fn a_room_the_compiled_map_bounds_nowhere_breaks_internally() {
        let plan = teleport_fixture(vec![teleporter_feature("a", "b", 0, false)]);
        let mut compiled =
            compile(&plan, map_name("MAP01"), MapFormat::Doom, &TestRoles).expect("it compiles");
        compiled.room_sectors.insert("a".to_owned(), vec![99_usize]);
        let error =
            apply_features(&plan, &mut compiled, &dress()).expect_err("sector 99 bounds nothing");
        assert_eq!(error.code(), "internal", "a contract break stays internal");
        let message = error.to_string();
        assert!(
            message.contains("pad") && message.contains("'a'"),
            "the role and room are named: {message}"
        );
    }

    #[test]
    fn remap_rows_follow_the_rebuild_and_drop_the_lost_lines() {
        let mut rows = vec![
            FeatureRow {
                kind: "door",
                room: None,
                sectors: vec![2_usize],
                lines: vec![0_usize, 1_usize, 2_usize],
                tag: Some(1_i32),
            },
            FeatureRow {
                kind: "lift",
                room: Some("b".to_owned()),
                sectors: vec![1_usize],
                lines: vec![3_usize],
                tag: Some(2_i32),
            },
        ];
        remap_rows(
            &mut rows,
            &[
                Some(5_usize),
                None,
                Some(6_usize),
                Some(7_usize),
                None,
                None,
                None,
                None,
            ],
        );
        assert_eq!(
            rows.first().expect("two rows").lines,
            vec![5_usize, 6_usize],
            "a painted-over line drops out"
        );
        assert_eq!(rows.get(1).expect("two rows").lines, vec![7_usize]);
        assert_eq!(
            rows.first().expect("two rows").sectors,
            vec![2_usize],
            "sectors never move"
        );
    }

    #[test]
    fn a_later_teleporter_keeps_earlier_rows_naming_their_real_lines() {
        let mut plan = two_rooms(vec![door("d1", DoorStyle::Normal, (128_i64, 32_i64))]);
        plan.features = vec![teleporter_feature("a", "b", 0, false)];
        let (compiled, rows) = wired(&plan);
        let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            vec!["door", "teleporter"],
            "the door leads: {kinds:?}"
        );
        let door_row = rows.first().expect("two rows");
        assert_eq!(door_row.lines.len(), 2_usize, "the door keeps its lines");
        for &line in &door_row.lines {
            let def = line_at(&compiled.map, line);
            assert_eq!(def.special, 1_i32, "line {line} is still the DR door line");
            assert_eq!(def.tag, door_row.tag.expect("the door tag"));
        }
        let teleporter_row = rows.get(1).expect("two rows");
        for &line in &teleporter_row.lines {
            assert_eq!(line_at(&compiled.map, line).special, 97_i32);
        }
    }

    /// A one-room plan with the given light effect and secret flag.
    fn lit_room_plan(effect: Option<LightEffect>, secret: bool) -> Plan {
        let mut room = rect_room("a", (0, 0), (128, 128));
        room.light_effect = effect;
        room.secret = secret;
        Plan {
            rooms: vec![room],
            connections: Vec::new(),
            features: Vec::new(),
            wall_thickness: 16_i64,
        }
    }

    #[test]
    fn a_lit_secret_room_keeps_the_light_under_its_secret_bit() {
        let (compiled, rows) = wired(&lit_room_plan(Some(LightEffect::StrobeSlow), true));
        let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(kinds, vec!["light", "secret"], "lights run before secrets");
        let space = compiled.map.sectors.first().expect("one sector");
        assert_eq!(space.special, 131_i32, "strobe 3 keeps the Boom bit 128");
        assert_eq!(space.light, 160_i32, "the level is kept");
        assert!(
            compiled
                .warnings
                .iter()
                .any(|note| note.contains("sector 0 now carries special 131")),
            "the vanilla warning passes through: {:?}",
            compiled.warnings
        );
        for row in &rows {
            assert_eq!(row.room.as_deref(), Some("a"), "the lit room");
            assert_eq!(row.sectors, vec![0_usize], "the room's one sector");
            assert!(row.lines.is_empty(), "a painted special owns no line");
            assert_eq!(row.tag, None, "and no tag");
        }
    }

    #[test]
    fn an_unlit_secret_room_takes_the_vanilla_type_in_silence() {
        let (compiled, rows) = wired(&lit_room_plan(None, true));
        let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(kinds, vec!["secret"], "no effect, no light row");
        assert_eq!(
            compiled.map.sectors.first().expect("one sector").special,
            9_i32,
            "the plain sector takes the vanilla secret type"
        );
        assert!(
            !compiled
                .warnings
                .iter()
                .any(|note| note.contains("Boom bits")),
            "the vanilla 9 needs no warning: {:?}",
            compiled.warnings
        );
    }

    #[test]
    fn an_explicit_none_effect_clears_the_type_like_the_tool_does() {
        let (compiled, rows) = wired(&lit_room_plan(Some(LightEffect::None), false));
        let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(kinds, vec!["light"], "Some(none) is still an effect");
        assert_eq!(
            compiled.map.sectors.first().expect("one sector").special,
            0_i32,
            "the basic type clears"
        );
    }

    #[test]
    fn a_plain_room_earns_neither_light_nor_secret_row() {
        let (compiled, rows) = wired(&lit_room_plan(None, false));
        assert!(rows.is_empty(), "nothing to paint: {rows:?}");
        assert_eq!(
            compiled.map.sectors.first().expect("one sector").special,
            0_i32
        );
    }

    #[test]
    fn a_painted_over_lit_room_installs_nothing_and_reports_no_row() {
        let mut lit = rect_room("a", (0, 0), (128, 128));
        lit.light_effect = Some(LightEffect::Glow);
        lit.secret = true;
        let plan = Plan {
            rooms: vec![lit, rect_room("b", (0, 0), (128, 128))],
            connections: Vec::new(),
            features: Vec::new(),
            wall_thickness: 16_i64,
        };
        let (compiled, rows) = wired(&plan);
        assert!(
            rows.is_empty(),
            "the vanished room paints nothing: {rows:?}"
        );
        assert!(
            compiled
                .warnings
                .iter()
                .any(|note| note.contains("painted over")),
            "the compiler already said so: {:?}",
            compiled.warnings
        );
    }

    /// A three-room plan asking for everything the pipeline installs:
    /// a remote door and its switch, a lift, a teleporter, an exit and
    /// a lit secret.
    fn rich_plan() -> Plan {
        let mut upper = rect_room("b", (0, 128), (128, 128));
        upper.floor = 64_i32;
        let mut crypt = rect_room("c", (128, 0), (128, 128));
        crypt.secret = true;
        crypt.light_effect = Some(LightEffect::StrobeSlow);
        Plan {
            rooms: vec![rect_room("a", (0, 0), (128, 128)), upper, crypt],
            connections: vec![
                join(
                    "d1",
                    "a",
                    "c",
                    ConnectionKind::Door {
                        style: DoorStyle::Remote,
                    },
                ),
                join("o1", "a", "b", ConnectionKind::Opening),
            ],
            features: vec![
                lift_feature("b", true),
                teleporter_feature("a", "b", 0, false),
                switch_feature("a", "d1", None, false),
                exit_feature("c", ExitStyle::Switch, false, Some(Side::East)),
            ],
            wall_thickness: 0_i64,
        }
    }

    /// The first row of class `kind`.
    #[track_caller]
    fn row<'rows>(rows: &'rows [FeatureRow], kind: &str) -> &'rows FeatureRow {
        rows.iter()
            .find(|entry| entry.kind == kind)
            .unwrap_or_else(|| panic!("a {kind} row"))
    }

    #[test]
    fn the_rows_come_back_in_the_fixed_pipeline_order() {
        let (_, rows) = wired(&rich_plan());
        let kinds: Vec<&str> = rows.iter().map(|entry| entry.kind).collect();
        assert_eq!(
            kinds,
            vec![
                "door",
                "switch",
                "lift",
                "teleporter",
                "exit",
                "light",
                "secret"
            ],
            "the pipeline order is fixed whatever the plan's order"
        );
        assert_eq!(row(&rows, "door").tag, Some(1_i32), "the door tags first");
        assert_eq!(
            row(&rows, "switch").tag,
            Some(1_i32),
            "the switch reuses it"
        );
        assert_eq!(row(&rows, "lift").tag, Some(2_i32), "the lift mints next");
        assert_eq!(
            row(&rows, "teleporter").tag,
            Some(3_i32),
            "the teleporter follows"
        );
        assert_eq!(row(&rows, "exit").tag, None, "the exit carries no tag");
        assert_eq!(row(&rows, "light").tag, None, "and so does the light");
        assert_eq!(row(&rows, "secret").tag, None, "or the secret");
    }

    #[test]
    fn the_rich_plan_survives_the_rebuild_lit_and_wired() {
        let (compiled, rows) = wired(&rich_plan());
        let crypt = compiled.room_sectors.get("c").expect("room c");
        let space = compiled
            .map
            .sectors
            .get(*crypt.first().expect("one sector"))
            .expect("the crypt");
        assert_eq!(
            space.special, 131_i32,
            "the lit secret survives in the order"
        );
        for &line in &row(&rows, "lift").lines {
            assert_eq!(
                line_at(&compiled.map, line).special,
                62_i32,
                "the lift lines still name the ride after the teleporter rebuild"
            );
        }
        let panel = row(&rows, "switch")
            .lines
            .first()
            .copied()
            .expect("one panel line");
        assert_eq!(
            line_at(&compiled.map, panel).special,
            103_i32,
            "the panel keeps its action"
        );
        let warnings = &compiled.warnings;
        assert!(
            warnings
                .iter()
                .any(|note| note.contains("sector") && note.contains("special 131")),
            "the boom note rides along: {warnings:?}"
        );
    }
}
