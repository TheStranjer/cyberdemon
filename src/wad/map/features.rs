//! Map features: complete game structures built from plain model rows.
//!
//! A feature starts as ordinary [`crate::wad::map::model::Map`] elements;
//! the modules here turn explicit parameters into the edits that make
//! those rows do something. [`doors`] is the door core: the per-family
//! action table of [`door_specials`] and the doorway dressing of
//! [`apply_door`], shared by the door tools. [`place`] is the door
//! placement geometry: wall selection, width fit and the outward depth
//! raycast that turn a room and a desire into a concrete [`DoorWall`]
//! cut and the [`BoundaryStrip`] and [`door_rect`] polygons to draw.
//! Like the rest of [`crate::wad`] this is pure model arithmetic and
//! table lookup: no sessions, no archives, no MCP.

pub mod doors;
pub mod place;

pub use doors::{
    DoorLineSetup, DoorSides, DoorStyle, DoorTextures, apply_door, door_specials, next_free_tag,
};
pub use place::{
    BoundaryStrip, DoorWall, VertexPair, door_rect, door_segment, door_wall, longest_boundary,
    nearest_boundary, outward_depth, project_onto_line, void_strip,
};
