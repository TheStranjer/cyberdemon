//! Game definitions: the reference tables the server ships with.
//!
//! [`table`] parses the tab-separated files that live under `data/defs/`;
//! [`registry`] embeds them in the binary, parses each one once, and answers
//! [`registry::definitions_for`]: the thing types, linedef specials and
//! sector types a given [`crate::wad::game::Game`] can use. [`boom`]
//! decodes the bit-packed Boom generalized specials and the Boom sector
//! special bits that no table enumerates. [`custom`] overlays the built-in
//! tables with the custom thing definitions of the loaded archives, read
//! through the tolerant subset parsers [`dehacked`], [`decorate`],
//! [`zscript`] and [`mapinfo`]. [`dehacked_edit`] is the line-oriented
//! DeHackEd/BEX reader and editor behind the scripts tools.

pub mod boom;
pub mod custom;
pub mod decorate;
pub mod dehacked;
pub mod dehacked_edit;
pub mod mapinfo;
pub mod registry;
pub mod table;
pub mod zscript;

pub use boom::{
    GENERALIZED_MAX, GENERALIZED_MIN, Generalized, SectorDecoded, SectorFamily, SectorFlag,
    decode_generalized_linedef, decode_sector_special, is_generalized,
};
pub use custom::{CustomSources, session_definitions};
pub use registry::{Definitions, definitions_for};
pub use table::{SectorTypeDef, SpecialDef, ThingDef};
