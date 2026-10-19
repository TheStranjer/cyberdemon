//! Plan compiler core: rooms + thick walls + connectors → map geometry (pure).
//!
//! A [`Plan`] describes a map declaratively: [`Room`]s with rectangular
//! or polygonal footprints and sector properties, [`Connection`]s
//! between them (openings, doors, windows, stairs) and a global wall
//! thickness. The later slices of the plan compiler grow rooms into
//! thick wall polygons, cut the connectors through those walls and
//! emit `Doom` map geometry from the arrangement. Nothing in this
//! module touches archives, sessions or `MCP` types; it is data,
//! measurement and validation only.

pub mod connectors;
pub mod model;
pub mod walls;

pub use model::*;
