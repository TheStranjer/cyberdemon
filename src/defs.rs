//! Game definitions: the reference tables the server ships with.
//!
//! [`table`] parses the tab-separated files that live under `data/defs/`;
//! [`registry`] embeds them in the binary, parses each one once, and answers
//! [`registry::definitions_for`]: the thing types, linedef specials and
//! sector types a given [`crate::wad::game::Game`] can use. [`boom`]
//! decodes the bit-packed Boom generalized specials and the Boom sector
//! special bits that no table enumerates.

pub mod boom;
pub mod registry;
pub mod table;

pub use boom::{
    GENERALIZED_MAX, GENERALIZED_MIN, Generalized, SectorDecoded, SectorFamily, SectorFlag,
    decode_generalized_linedef, decode_sector_special, is_generalized,
};
pub use registry::{Definitions, definitions_for};
pub use table::{SectorTypeDef, SpecialDef, ThingDef};
