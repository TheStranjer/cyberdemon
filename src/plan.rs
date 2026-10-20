//! Plan compiler core: rooms + thick walls + connectors → map geometry (pure).
//!
//! A [`Plan`] describes a map declaratively: [`Room`]s with rectangular
//! or polygonal footprints and sector properties, [`Connection`]s
//! between them (openings, doors, windows, stairs) and a global wall
//! thickness. [`fn@compile`] runs the whole pipeline: thick walls, connector
//! placement, map emission and theme dressing, and reports the result as
//! a [`CompiledPlan`]. Before it, [`transform_plan`] carries a drawn plan
//! from its own units into map units: an integer scale, an optional
//! vertical mirror and a snap grid. Nothing in this module touches
//! archives, sessions or `MCP` types; it is data, measurement and
//! validation only.

pub mod compile;
pub mod connectors;
pub mod emit;
pub mod model;
pub mod textures;
pub mod units;
pub mod walls;

pub use compile::compile;
pub use emit::{CompiledPlan, ConnectorInfo};
pub use model::*;
pub use textures::{TextureRole, TextureRoles};
pub use units::{Units, transform_plan};
