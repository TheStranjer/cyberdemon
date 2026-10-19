//! Map features: complete game structures built from plain model rows.
//!
//! A feature starts as ordinary [`crate::wad::map::model::Map`] elements;
//! the modules here turn explicit parameters into the edits that make
//! those rows do something. [`doors`] is the door core: the per-family
//! action table of [`door_specials`] and the doorway dressing of
//! [`apply_door`], shared by the door tools. Like the rest of [`crate::wad`]
//! this is pure model arithmetic and table lookup: no sessions, no
//! archives, no MCP.

pub mod doors;

pub use doors::{
    DoorLineSetup, DoorSides, DoorStyle, DoorTextures, apply_door, door_specials, next_free_tag,
};
