//! Map features: complete game structures built from plain model rows.
//!
//! A feature starts as ordinary [`crate::wad::map::model::Map`] elements;
//! the modules here turn explicit parameters into the edits that make
//! those rows do something. [`doors`] is the door core: the per-family
//! action table of [`door_specials`] and the doorway dressing of
//! [`apply_door`], shared by the door tools; [`face_room_line`] is that
//! room-line treatment on its own, for callers (the `plan` feature
//! engine) that dress the faces themselves. [`place`] is the door
//! placement geometry: wall selection, width fit and the outward depth
//! raycast that turn a room and a desire into a concrete [`DoorWall`]
//! cut and the [`BoundaryStrip`] and [`door_rect`] polygons to draw.
//! [`switches`] is the pure switch core: the specials a wall switch
//! installs per flag family is the [`switch_special`] table, and
//! [`switches::build_switch`] mounts one on a named wall — fit the panel
//! chunk, isolate it as its own linedef and dress the line with the
//! action — while [`switches::build_walk_over_exit`] ends the level on
//! every plain two-sided boundary of a sector, leaving a boundary line
//! another feature already dresses to its own action. [`window`] is the
//! pure window core: [`window::window_record`] computes the strip
//! sector's sill/lintel heights and [`window::apply_window`] closes off
//! the drawn strip with two-sided blocking view lines. [`stairs`] is
//! the pure stair core: [`stairs::plan_stairs`] divides a straight path
//! into [`stairs::StairStep`] rectangles whose floors rise evenly
//! between the endpoint sectors (headroom tested before anything is
//! drawn) and [`stairs::build_stairs`] paints them bottom-up through the
//! overlay engine. [`lift`] is the pure lift core: [`lift::lift_specials`]
//! is the per-family down-wait-up action of a [`lift::LiftStyle`], and
//! [`lift::build_lift`] finds the edges of a platform (an existing
//! sector or a polygon folded over the map) that face lower neighbours,
//! turns them to face the player below and dresses them with the action.
//! [`specials`] is the sector-special side of the same
//! idea: [`secret_edit`] folds the secret flag of each flag family into
//! a sector's current special and [`light_special`] does the same for a
//! [`LightEffect`], keeping every bit outside the family's basic-type
//! field; [`boom_note`] is the shared vanilla-Doom warning a flagged
//! vanilla type earns, emitted by both the `mark_secret` tool and the
//! plan feature engine's secret step. [`teleport`] is the pure teleporter core:
//! [`teleport::teleport_specials`] is the walk-on action of a pad border
//! per flag family, and [`teleport::build_teleporter`] draws the pad and
//! destination squares through the overlay engine, places the type-14
//! destination things (a `Doom` one found through the tagged
//! destination sector, a `Hexen` one carrying its own TID) and dresses
//! the squares' outward-facing borders with it — [`TeleportSpec::two_way`]
//! running the same pattern back with its own tag/TID. Like the rest of
//! [`crate::wad`] this is pure model arithmetic and table lookup: no
//! sessions, no archives, no MCP.

pub mod doors;
pub mod lift;
pub mod place;
pub mod specials;
pub mod stairs;
pub mod switches;
pub mod teleport;
pub mod window;

pub use doors::{
    DoorLineSetup, DoorSides, DoorStyle, DoorTextures, apply_door, door_specials, face_room_line,
    next_free_tag,
};
pub use lift::{
    LiftBuild, LiftPlatform, LiftSpec, LiftStyle, MIN_LIFT_DROP, build_lift, lift_specials,
};
pub use place::{
    BoundaryStrip, DoorWall, VertexPair, door_rect, door_segment, door_wall, endpoints,
    longest_boundary, nearest_boundary, outward_depth, project_onto_line, void_strip,
};
pub use specials::{
    BOOM_SECRET_BIT, DOOM_SECRET_SPECIAL, GENERALIZED_SECTOR_MIN, HEXEN_SECRET_BIT, LightEffect,
    SecretEdit, boom_note, light_special, secret_edit,
};
pub use stairs::{
    DEPTH_TOLERANCE, MAX_STEP_HEIGHT, MIN_HEADROOM, MIN_STEP_HEIGHT, PaintedStep, StairBuild,
    StairPlan, StairSpec, StairStep, build_stairs, plan_stairs,
};
pub use switches::{
    Chunk, DEFAULT_SWITCH_WIDTH, PlacedSwitch, SkippedLine, SwitchAction, SwitchSpec, WalkOverExit,
    action_name, build_switch, build_walk_over_exit, dress_switch, is_level_end, isolate_chunk,
    switch_cut, switch_span, switch_special, switch_tag, walk_over_special,
};
pub use teleport::{
    DEFAULT_PAD_SIZE, TELEPORT_DESTINATION_THING, TeleportBuild, TeleportSpec, build_teleporter,
    teleport_specials,
};
pub use window::{
    MIN_OPENING, WindowHeights, WindowSides, WindowTextures, apply_window, boundary_lines,
    window_heights, window_record,
};
