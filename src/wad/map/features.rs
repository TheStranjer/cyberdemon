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
//! [`switches`] is the pure switch action table of [`switch_special`]:
//! the specials a wall switch installs per flag family, in the same
//! [`DoorLineSetup`] shape the door engine writes. [`window`] is the
//! pure window core: [`window::window_record`] computes the strip
//! sector's sill/lintel heights and [`window::apply_window`] closes off
//! the drawn strip with two-sided blocking view lines. [`specials`] is
//! the sector-special side of the same idea: [`secret_edit`] folds the
//! secret flag of each flag family into a sector's current special and
//! [`light_special`] does the same for a [`LightEffect`], keeping every bit
//! outside the family's basic-type field. Like the rest of
//! [`crate::wad`] this is pure model arithmetic and
//! table lookup: no sessions, no archives, no MCP.

pub mod doors;
pub mod place;
pub mod specials;
pub mod switches;
pub mod window;

pub use doors::{
    DoorLineSetup, DoorSides, DoorStyle, DoorTextures, apply_door, door_specials, next_free_tag,
};
pub use place::{
    BoundaryStrip, DoorWall, VertexPair, door_rect, door_segment, door_wall, longest_boundary,
    nearest_boundary, outward_depth, project_onto_line, void_strip,
};
pub use specials::{
    BOOM_SECRET_BIT, DOOM_SECRET_SPECIAL, GENERALIZED_SECTOR_MIN, HEXEN_SECRET_BIT, LightEffect,
    SecretEdit, light_special, secret_edit,
};
pub use switches::{SwitchAction, switch_special};
pub use window::{
    MIN_OPENING, WindowHeights, WindowSides, WindowTextures, apply_window, boundary_lines,
    window_heights, window_record,
};
