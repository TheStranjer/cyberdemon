//! Shared builders and assertions for the `plan::features` tests: the
//! plan and map fixtures every topic compiles and wires, the theme that
//! answers every dressing role, the wired-run entry points and the
//! line, sector and refusal readers the topics share.
//!
//! [`doors`] wires the connector doors, [`switches`] and its [`wall`]
//! child mount and choose panels, [`lifts`] rides, [`teleporters`]
//! rebuild, [`rooms`] paints lights and secrets and [`pipeline`] runs
//! the rich plan and the playable checks.

mod doors;
mod lifts;
mod pipeline;
mod rooms;
mod switches;
mod teleporters;

use super::{CompiledPlan, FeatureDress, FeatureRow, apply_features};
use crate::plan::compile::compile;
use crate::plan::model::{Connection, ConnectionKind, DoorStyle, Plan, Room, Shape};
use crate::plan::textures::{TextureRole, TextureRoles};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::model::Linedef;
use crate::wad::name::LumpName;

/// The band sector of every single-door fixture map: the two rooms
/// take sectors 0 and 1, the connector band follows.
pub(super) const BAND: usize = 2_usize;

/// The tag a fresh map's first door takes.
pub(super) const TAG_ONE: i32 = 1_i32;

/// The door lines of the two-room fixture.
pub(super) const TWO_DOOR_LINES: usize = 2_usize;

/// The band's jamb faces inside the wall, in the fixture.
pub(super) const FOUR_JAMBS: usize = 4_usize;

/// The `Hexen` `playeruse` activation bit.
pub(super) use crate::wad::map::flags::PLAYER_USE_BITS as PLAYER_USE;

/// The `Hexen` `repeatspecial` bit.
pub(super) use crate::wad::map::flags::REPEAT_SPECIAL as REPEAT;

/// The `Hexen` `monsteractivate` bit.
pub(super) use crate::wad::map::flags::MONSTER_ACTIVATE as MONSTER;

/// A theme answering every dressing role with a distinct name.
pub(super) struct TestRoles;

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
pub(super) fn map_name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test map names are valid")
}

/// A rectangular room with default sector properties.
pub(super) fn rect_room(id: &str, left_bottom: (i64, i64), width_height: (i64, i64)) -> Room {
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
pub(super) fn door(id: &str, style: DoorStyle, at: (i64, i64)) -> Connection {
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
pub(super) fn two_rooms(connections: Vec<Connection>) -> Plan {
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
pub(super) fn door_plan(style: DoorStyle) -> Plan {
    two_rooms(vec![door("d1", style, (128_i64, 32_i64))])
}

/// The feature dress every test applies, colours distinct.
pub(super) fn dress() -> FeatureDress {
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
pub(super) fn wired(plan: &Plan) -> (CompiledPlan, Vec<FeatureRow>) {
    wired_dressed(plan, MapFormat::Doom, &dress())
}

/// Compile a plan in `format` and run the features dressed by
/// `themed`.
#[track_caller]
pub(super) fn wired_dressed(
    plan: &Plan,
    format: MapFormat,
    themed: &FeatureDress,
) -> (CompiledPlan, Vec<FeatureRow>) {
    let mut compiled = compile(plan, identity(format), &TestRoles).expect("the fixture compiles");
    let rows = apply_features(plan, &mut compiled, themed).expect("features apply");
    (compiled, rows)
}

/// The identity a feature test compiles into: `MAP01` in `format`, the
/// vanilla sky flat.
pub(super) fn identity(format: MapFormat) -> crate::plan::emit::MapIdentity {
    crate::plan::emit::MapIdentity {
        name: map_name("MAP01"),
        format,
        sky_flat: crate::wad::map::consts::SKY_FLAT,
    }
}

/// The line `index` of the map.
pub(super) fn line_at(map: &Map, index: usize) -> &Linedef {
    map.linedefs.get(index).expect("test indices")
}

/// The sector behind a line's front sidedef.
pub(super) fn front_sector(map: &Map, line: &Linedef) -> usize {
    let front = line.front.expect("emitted lines have a front");
    map.sidedefs.get(front).expect("test rows").sector
}

/// The sector behind a line's back sidedef, `None` when one-sided.
pub(super) fn back_sector(map: &Map, line: &Linedef) -> Option<usize> {
    let back = line.back?;
    Some(map.sidedefs.get(back)?.sector)
}

/// The action a wired use line must carry: `special`, `tag` and
/// `args`.
pub(super) type UseAction = (i32, i32, [i32; 5]);

/// Assert the two-sided `index`th line is a use line facing its
/// room — the front side belongs to a room sector, the back to the
/// band — and carries `action`.
#[track_caller]
pub(super) fn assert_use_line(map: &Map, index: usize, action: UseAction) {
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

/// A named plain opening between rooms `from` and `to`.
pub(super) fn opening(id: &str, from: &str, to: &str) -> Connection {
    Connection {
        id: Some(id.to_owned()),
        from: from.to_owned(),
        to: to.to_owned(),
        kind: ConnectionKind::Opening,
        at: None,
        width: 48_i64,
    }
}

/// Compile `plan` and require the feature run to refuse.
#[track_caller]
pub(super) fn refuse_features(plan: &Plan) -> crate::error::CyberError {
    let mut compiled = compile(plan, identity(MapFormat::Doom), &TestRoles).expect("it compiles");
    let error = apply_features(plan, &mut compiled, &dress());
    error.expect_err("the feature must be refused")
}
