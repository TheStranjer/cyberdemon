//! Plan compiler core: rooms + thick walls + connectors → map geometry (pure).
//!
//! A [`Plan`] describes a map declaratively: [`Room`]s with rectangular
//! or polygonal footprints and sector properties, [`Connection`]s
//! between them (openings, doors, windows, stairs) and a global wall
//! thickness. [`fn@compile`] runs the whole pipeline: thick walls, connector
//! placement, map emission and theme dressing, and reports the result as
//! a [`CompiledPlan`]. Before it, [`transform_plan`] carries a drawn plan
//! from its own units into map units: an integer scale, an optional
//! vertical mirror and a snap grid. After it, [`things`] fills the
//! compiled geometry: name resolution, room grids and presence flags,
//! and [`features`] installs the game behaviour: door connectors take
//! their tags, specials and facing, `switch` rows mount their panels on
//! room walls firing a remote door's tag, and `exit` rows end the level
//! by panel or walk-over; the lift, teleporter, light and secret
//! wiring comes with the follow-up slices. Nothing in this module touches
//! archives, sessions or `MCP` types; it is data, measurement and
//! validation only.

pub mod compile;
pub mod connectors;
pub mod emit;
pub mod features;
pub mod model;
pub mod textures;
pub mod things;
pub mod units;
pub mod walls;

pub use compile::compile;
pub use emit::{CompiledPlan, ConnectorInfo};
pub use features::{FeatureDress, FeatureRow, apply_features};
pub use model::*;
pub use textures::{TextureRole, TextureRoles};
pub use things::{
    PLAYER_ONE_START, Placement, PlacementContext, PlannedThing, Skill, ThingSpot, fits, place,
    presence_flags, resolve_thing_kind,
};
pub use units::{Units, transform_plan};
