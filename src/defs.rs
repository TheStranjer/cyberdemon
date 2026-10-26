//! Game definitions: the reference tables the server ships with.
//!
//! [`tsv`] is the shared tab-separated reader every embedded table (the
//! definition, theme and translation files alike) parses through, with the
//! one `LazyLock` accessor for parsed-once embedded data. [`table`] turns
//! the thing, linedef-special and sector-type files into rows — classifying
//! each row's editor category into a [`table::Category`] once — and
//! [`registry`] embeds them in the binary and answers
//! [`registry::definitions_for`]: the thing types, linedef specials and
//! sector types a given [`crate::wad::game::Game`] can use, looked up
//! through indexes the [`Definitions`] build once over their tables.
//! [`boom`] decodes the bit-packed Boom generalized specials and the Boom
//! sector special bits that no table enumerates. [`custom`] overlays the
//! built-in tables with the custom thing definitions of the loaded archives:
//! DEHACKED patches through the shared line reader [`dehacked_edit`] — the
//! same blocks the scripts tools list and edit — and custom scripts through
//! the tolerant subset parsers [`decorate`], [`zscript`] and [`mapinfo`].
//! [`dehacked_edit`] is also the line-oriented DeHackEd/BEX reader and
//! editor behind the scripts tools.

pub mod boom;
pub mod custom;
pub mod decorate;
pub mod dehacked;
pub mod dehacked_edit;
pub mod mapinfo;
pub mod registry;
pub mod table;
pub mod tsv;
pub mod zscript;

pub use boom::{
    GENERALIZED_MAX, GENERALIZED_MIN, Generalized, SectorDecoded, SectorFamily, SectorFlag,
    decode_generalized_linedef, decode_sector_special, is_generalized,
};
pub use custom::{CustomSources, session_definitions};
pub use registry::{Definitions, definitions_for};
pub use table::{Category, SectorTypeDef, SpecialDef, ThingDef};
