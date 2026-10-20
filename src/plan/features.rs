//! Plan features: the game structures a compiled plan installs.
//!
//! [`apply_features`] runs right after [`compile`](fn@crate::plan::compile)
//! and turns the compiled bookkeeping into live map behaviour. It walks
//! a fixed-order pipeline — `doors`, `switches`, `lifts`, `teleporters`,
//! `exits`, `lights` and `secrets` — so a later step always sees what
//! the earlier ones allocated; the private `FeatureRun` carries that
//! state (the tag and band sector every connector got, the rows applied
//! so far) between them. The door, switch and exit steps have
//! behaviour on this branch; the lift and teleporter wiring slices fill
//! the rest.
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
//! row names a wired [`DoorStyle::Remote`] door. The exit step ends the
//! level: its `switch` style mounts the panel the same way carrying the
//! exit action, and its `walk_over` style dresses every two-sided
//! boundary of the room's sectors with [`build_walk_over_exit`]. Every
//! installed feature reports one [`FeatureRow`]. Nothing here touches
//! archives, sessions or `MCP` types; the theme names arrive already
//! resolved.
//!
//! [`DoorStyle::Remote`]: crate::wad::map::features::DoorStyle::Remote
//! [`door_specials`]: crate::wad::map::features::door_specials
//! [`face_room_line`]: crate::wad::map::features::face_room_line
//! [`next_free_tag`]: crate::wad::map::features::next_free_tag
//! [`track_role_name`]: crate::wad::map::features::DoorStyle::track_role_name

use alloc::collections::BTreeMap;

use serde::Serialize;

use crate::error::CyberError;
use crate::plan::emit::{CompiledPlan, ConnectorInfo};
use crate::plan::model::{ConnectionKind, DoorStyle, ExitStyle, Feature, KeyColor, Plan, Side};
use crate::wad::map::Map;
use crate::wad::map::features::DoorStyle as WadDoorStyle;
use crate::wad::map::features::{
    DEFAULT_SWITCH_WIDTH, PlacedSwitch, SwitchAction, SwitchSpec, build_switch,
    build_walk_over_exit, door_specials, face_room_line, next_free_tag,
};
use crate::wad::map::model::Linedef;

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
/// [`exit_switch`](FeatureDress::exit_switch), and the plain
/// [`door`](FeatureDress::door) face and [`track`](FeatureDress::track)
/// names serve the follow-up slices.
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
            _ => "",
        };
        (!name.is_empty() && name != NO_NAME).then_some(name)
    }
}

/// One game structure the run installed, as the result reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FeatureRow {
    /// The feature class: `door`, `switch` and `exit` today; `lift`,
    /// `teleporter`, `light` and `secret` come with the follow-up slices.
    pub kind: &'static str,
    /// The room the feature stands in; `None` for a connector feature
    /// like a door, which belongs to the wall between two rooms.
    pub room: Option<String>,
    /// The sectors the feature dresses (a door: its band sector; a
    /// switch: the door band it fires; an exit: its room's sectors).
    pub sectors: Vec<usize>,
    /// The linedefs the feature carries its action on (a door: the
    /// room-facing trigger lines; a reported empty list with a tag is a
    /// remote's handle for the switch slice).
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
/// tag up in the run state); the door, switch and exit steps act on the
/// [`Feature`] rows of the plan, and the
/// lift, teleporter, light and secret steps stay placeholders until
/// their wiring slices land. `dress` carries the theme-resolved texture
/// names; a role answered `"-"` leaves its faces as
/// [`compile`](fn@crate::plan::compile) dressed them. `compiled` must
/// be the plan's own compilation: the steps act on the connector
/// bookkeeping, never re-derive it.
///
/// # Errors
///
/// [`CyberError::Internal`] when a door connector names a band sector
/// the compiled map does not hold — the signature of a `compiled` from
/// a different plan. The switch step refuses a row whose `opens` names
/// no wired [`DoorStyle::Remote`] door and a room with no wall to mount
/// on; the exit step refuses the same way, and a `walk_over` room whose
/// sectors share no two-sided boundary with anything.
#[inline]
pub fn apply_features(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
) -> Result<Vec<FeatureRow>, CyberError> {
    let mut run = FeatureRun::default();
    doors(compiled, dress, &mut run)?;
    switches(plan, compiled, dress, &mut run)?;
    lifts(plan, &run);
    teleporters(plan, &run);
    exits(plan, compiled, dress, &mut run)?;
    lights(plan, &run);
    secrets(plan, &run);
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

/// The lift step: empty until the follow-up slice lowers the platforms
/// the plan's `lift` rows ask for.
const fn lifts(_plan: &Plan, _run: &FeatureRun) {}

/// The teleporter step: empty until the follow-up slice stands the pads
/// and destinations of the plan's `teleporter` rows.
const fn teleporters(_plan: &Plan, _run: &FeatureRun) {}

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

/// The lights step: empty until the follow-up slice installs the rooms'
/// `light_effect` sector specials.
const fn lights(_plan: &Plan, _run: &FeatureRun) {}

/// The secrets step: empty until the follow-up slice flags the rooms
/// the plan marks `secret` (and the `exit` rows carried `secret`).
const fn secrets(_plan: &Plan, _run: &FeatureRun) {}

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
        choose_switch_wall, doors, remote_door, wad_style,
    };
    use crate::plan::compile::compile;
    use crate::plan::model::{
        Connection, ConnectionKind, DoorStyle, ExitStyle, Feature, KeyColor, Room, Shape, Side,
    };
    use crate::plan::textures::{TextureRole, TextureRoles};
    use crate::wad::classify::MapFormat;
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
        assert_eq!(themed.texture("wall"), None, "no such dressing field");
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
}
