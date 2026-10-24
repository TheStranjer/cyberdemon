//! The plan-built fixtures: the rectangular [`Room`] and [`Connection`]
//! builders, the [`FlatRoles`] theme and [`built`] — a compiled plan with
//! its features wired — beside the four-room [`keyed_exit_plan`] the
//! overview readings stand a crew, a blue keycard and a walk-over exit in.
//!
//! The topics that need a real door band, lift, teleporter or stair run
//! build it here rather than hand-drawing the records.

use alloc::vec;
use alloc::vec::Vec;

use crate::plan::FeatureDress;
use crate::plan::compile::compile;
use crate::plan::features::{FeatureRow, apply_features};
use crate::plan::model::{
    Connection, ConnectionKind, DoorStyle, ExitStyle, Feature as PlanFeature,
    KeyColor as PlanKeyColor, Plan, Room, Shape,
};
use crate::plan::textures::{TextureRole, TextureRoles};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::common::fixtures::{name, spawn};

use super::{cell, room_sector, stand};

/// A theme answering every dressing role with a fixed name.
struct FlatRoles;

impl TextureRoles for FlatRoles {
    fn texture(&self, role: TextureRole) -> Option<&str> {
        Some(match role {
            TextureRole::Floor => "FLATROL",
            TextureRole::Ceiling => "CEIROL",
            TextureRole::Wall => "WALROL",
            TextureRole::Door => "DOORROL",
            TextureRole::DoorFloor => "DFLAROL",
            TextureRole::DoorTrack => "DTRROL",
            TextureRole::WindowSill => "WSIROL",
            TextureRole::Step => "STEPRO",
        })
    }
}

/// A rectangular plan room, 256 units tall, spanning `span` on `x`
/// and standing at `heights`' `(floor, ceiling)`.
pub(super) fn plan_room(id: &str, span: (i64, i64), heights: (i32, i32)) -> Room {
    Room {
        id: id.to_owned(),
        shape: Shape::Rect {
            x: span.0,
            y: 0,
            w: span.1,
            h: 256,
        },
        floor: heights.0,
        ceiling: heights.1,
        light: 160,
        floor_flat: None,
        ceiling_flat: None,
        wall_texture: None,
        outdoor: false,
        secret: false,
        light_effect: None,
    }
}

/// A connection of `kind` between the rooms of `pair`, `width` clear.
pub(super) fn plan_link(
    id: &str,
    pair: (&str, &str),
    kind: ConnectionKind,
    width: i64,
) -> Connection {
    Connection {
        id: Some(id.to_owned()),
        from: pair.0.to_owned(),
        to: pair.1.to_owned(),
        kind,
        at: None,
        width,
    }
}

/// The plan every [`built`] test wraps: `rooms` joined by `links`,
/// `features` wired and `thickness` of wall grown between the rooms.
pub(super) fn plan_of(
    rooms: Vec<Room>,
    links: Vec<Connection>,
    features: Vec<PlanFeature>,
    thickness: i64,
) -> Plan {
    Plan {
        rooms,
        connections: links,
        features,
        wall_thickness: thickness,
    }
}

/// A compiled plan in `format` with its features wired.
pub(super) fn built(plan: &Plan, format: MapFormat) -> (Map, Vec<FeatureRow>) {
    let mut compiled =
        compile(plan, name("MAP01"), format, &FlatRoles).expect("the test plans compile");
    let dress = FeatureDress {
        door: "DOORROL".to_owned(),
        track: "DTRROL".to_owned(),
        track_red: "REDTRAK".to_owned(),
        track_blue: "BLUETRAK".to_owned(),
        track_yellow: "YELTRAK".to_owned(),
        switch: "SWITROL".to_owned(),
        exit_switch: "EXITROL".to_owned(),
        step: "STEPRO".to_owned(),
        wall: "WALROL".to_owned(),
    };
    let rows = apply_features(plan, &mut compiled, &dress).expect("the test features install");
    (compiled.map, rows)
}

/// The four-room plan the overview tests build: `a | b | c` down the
/// `x` axis, a blue-locked door between `a` and `b`, openings from
/// `b` to `c` and from `c` to the secret room `s`, and a walk-over
/// exit on `c`'s boundaries.
pub(super) fn keyed_exit_plan() -> Plan {
    let mut hidden = plan_room("s", (768, 256), cell(0_i32));
    hidden.secret = true;
    plan_of(
        vec![
            plan_room("a", (0, 256), cell(0_i32)),
            plan_room("b", (256, 256), cell(0_i32)),
            plan_room("c", (512, 256), cell(0_i32)),
            hidden,
        ],
        vec![
            plan_link(
                "d1",
                ("a", "b"),
                ConnectionKind::Door {
                    style: DoorStyle::Locked(PlanKeyColor::Blue),
                },
                64,
            ),
            plan_link("c1", ("b", "c"), ConnectionKind::Opening, 64),
            plan_link("s1", ("c", "s"), ConnectionKind::Opening, 64),
        ],
        vec![PlanFeature::Exit {
            room: "c".to_owned(),
            style: ExitStyle::WalkOver,
            secret: false,
            wall: None,
            at: None,
        }],
        16_i64,
    )
}

/// The room sectors of a [`keyed_exit_plan`] build, in plan order.
pub(super) fn plan_rooms(map: &Map) -> [usize; 4] {
    [
        room_sector(map, 0),
        room_sector(map, 1),
        room_sector(map, 2),
        room_sector(map, 3),
    ]
}

/// The [`keyed_exit_plan`] built with a player 1 start, a blue keycard
/// and a small crew (two monsters, three items) standing in the start
/// room `a`.
pub(super) fn crewed_plan() -> (Map, Vec<FeatureRow>) {
    let (mut map, rows) = built(&keyed_exit_plan(), MapFormat::Doom);
    stand(&mut map, spawn(1, 128, 128, 0));
    stand(&mut map, spawn(5, 64, 192, 0));
    stand(&mut map, spawn(3001, 192, 64, 0));
    stand(&mut map, spawn(3004, 64, 64, 0));
    stand(&mut map, spawn(2001, 192, 192, 0));
    stand(&mut map, spawn(8, 160, 128, 0));
    stand(&mut map, spawn(2011, 224, 128, 0));
    (map, rows)
}
