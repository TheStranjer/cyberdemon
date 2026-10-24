//! The existence checks behind the `replace_*` wire parameters: the
//! merged view's texture [`Stock`], the target and term validators that
//! read it, and `replace_thing_type`'s doomednum-or-name [`ThingRef`]
//! side.
//!
//! [`Stock`] holds the wall and flat names the merged view offers,
//! loaded through the theme tools' inventory, so a replacement target
//! is checked against exactly what the theme roles resolve against;
//! [`Stock::require_wall`] and [`Stock::require_flat`] turn a miss into
//! the instructive [`CyberError::NotFound`] the tools document.
//! [`require_target`] adds the shared name validation around that check
//! and [`parse_terms`] folds a `parts` or `surfaces` list through the
//! pure passes' parsers. `resolve_thing_ref` reads a [`ThingRef`] into
//! the stored thing-type number through the session's definitions and
//! `require_defined_thing` checks a resolved target against the same
//! tables.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::defs::{Definitions, session_definitions};
use crate::error::CyberError;
use crate::plan::things::resolve_thing_kind;
use crate::session::Session;
use crate::tools::defs::collect::custom_sources;
use crate::tools::graphics::session_game;
use crate::tools::maps::params::validate_texture_name;
use crate::tools::themes::inventory;
use crate::wad::classify::MapFormat;

/// The merged view's replaceable texture stock: the wall and flat names
/// a replacement target may use.
#[derive(Clone, Debug, Default)]
pub struct Stock {
    /// Existing wall texture names.
    pub walls: BTreeSet<String>,
    /// Existing flat names.
    pub flats: BTreeSet<String>,
}

impl Stock {
    /// The merged view's current stock, loaded through the theme tools'
    /// inventory: composite `TEXTURE1`/`TEXTURE2` names, `TX_` lump
    /// names, the flats namespace and `F_SKY1`.
    ///
    /// # Errors
    ///
    /// The inventory load's refusals: the merged directory read's
    /// [`CyberError::NotFound`] and the texture-definition read's
    /// [`CyberError::Format`] for a malformed texture lump.
    #[inline]
    pub fn load(session: &Session) -> Result<Self, CyberError> {
        let available = inventory(session)?.available;
        Ok(Self {
            walls: available.walls,
            flats: available.flats,
        })
    }

    /// Whether `name` exists as a wall texture.
    #[must_use]
    #[inline]
    pub fn has_wall(&self, name: &str) -> bool {
        self.walls.contains(name)
    }

    /// Whether `name` exists as a flat.
    #[must_use]
    #[inline]
    pub fn has_flat(&self, name: &str) -> bool {
        self.flats.contains(name)
    }

    /// Refuse a wall name the merged view lacks.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] naming `list_textures` as the way to
    /// see what exists.
    #[inline]
    pub fn require_wall(&self, name: &str) -> Result<(), CyberError> {
        if self.has_wall(name) {
            return Ok(());
        }
        Err(CyberError::NotFound(format!(
            "wall texture {name} does not exist in the merged view: list_textures shows \
             the composites, build_texture and the import tools add more"
        ))
        .with_hint(
            "list_textures / list_flats with a glob pattern; or create one with \
                    create_texture_from_image / create_flat_from_image",
        ))
    }

    /// Refuse a flat name the merged view lacks.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] naming `list_flats` and
    /// `import_image_as_flat` as the fixes.
    #[inline]
    pub fn require_flat(&self, name: &str) -> Result<(), CyberError> {
        if self.has_flat(name) {
            return Ok(());
        }
        Err(CyberError::NotFound(format!(
            "flat {name} does not exist in the merged view: list_flats shows the flats, \
             import_image_as_flat adds one"
        ))
        .with_hint(
            "list_textures / list_flats with a glob pattern; or create one with \
                    create_texture_from_image / create_flat_from_image",
        ))
    }
}

/// One `replace_*` name list (`parts` or `surfaces`) run through `parse`.
///
/// Absent or empty means the tool's documented default of every entry,
/// which the pure passes read from an empty list.
///
/// # Errors
///
/// The first [`Face::parse`]/[`Surface::parse`]-style refusal `parse`
/// reports, naming the accepted values.
///
/// [`Face::parse`]: crate::wad::map::bulk::Face::parse
/// [`Surface::parse`]: crate::wad::map::bulk::Surface::parse
#[inline]
pub fn parse_terms<T, F>(values: Option<&[String]>, parse: F) -> Result<Vec<T>, CyberError>
where
    F: Fn(&str) -> Result<T, CyberError>,
{
    let Some(list) = values else {
        return Ok(Vec::new());
    };
    list.iter().map(|text| parse(text)).collect()
}

/// Validate one `replace_*` target name (uppercased, printable, within
/// the formats' longest cap) and require it to exist in the merged view
/// through `require`.
///
/// When `clearing` is `true`, `"-"` is the accepted no-texture name and
/// skips the existence check: clearing faces is what a wall replacement
/// means by `-`, while no sector may hold `-` as a flat.
///
/// # Errors
///
/// [`validate_texture_name`]'s [`CyberError::InvalidArgument`] for a bad
/// name and `require`'s [`CyberError::NotFound`] for a name the merged
/// view lacks.
#[inline]
pub fn require_target<F>(
    session: &Session,
    to: &str,
    clearing: bool,
    require: F,
) -> Result<String, CyberError>
where
    F: Fn(&Stock, &str) -> Result<(), CyberError>,
{
    // `UDMF` is the loosest cap the shared validator offers; the stock
    // check that follows admits exactly the names the merged view holds,
    // so no over-long name can reach a binary block.
    let name = validate_texture_name(to, MapFormat::Udmf)?;
    if clearing && name == "-" {
        return Ok(name);
    }
    require(&Stock::load(session)?, &name)?;
    Ok(name)
}

/// A `from` or `to` thing reference on the `replace_thing_type` wire:
/// the doomednum as stored, or a definition name.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ThingRef {
    /// The thing type number, exactly as `THINGS` stores it.
    Number(u32),
    /// A definition name, resolved through the session's definitions
    /// by `resolve_thing_ref`.
    Name(String),
}

/// The session's thing definitions: the tables of [`session_game`]'s
/// game overlaid with the merged view's `DEHACKED`/`DECORATE`/`ZScript`
/// custom sources — the same one-lock read as `tools/query.rs::load`.
///
/// # Errors
///
/// [`custom_sources`]' merged-scope read errors and
/// [`session_definitions`]' refusal for malformed embedded tables.
#[inline]
pub(in crate::tools) fn thing_definitions(session: &Session) -> Result<Definitions, CyberError> {
    let (sources, _notes) = custom_sources(session)?;
    session_definitions(session_game(session), &sources)
}

/// The doomednum as a stored thing-type number, or the refusal for one
/// above the signed 32-bit range the map models carry.
fn stored_kind(number: u32) -> Result<i32, CyberError> {
    let Ok(kind) = i32::try_from(number) else {
        return Err(CyberError::InvalidArgument(format!(
            "thing type {number} is above the signed 32-bit range a map can store: pass a \
             doomednum as the game saves it, or a definition name"
        )));
    };
    Ok(kind)
}

/// Resolve one [`ThingRef`] to the stored type number: a doomednum
/// passes through after a range check, a name resolves through the
/// session's definitions — an exact case-insensitive name first, then
/// a unique case-insensitive substring.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a doomednum above [`i32::MAX`],
/// for a name no definition carries, and for a substring several
/// distinct types share; the two name refusals list the candidate
/// names, and the caller prefixes them with the failing parameter.
#[inline]
pub(in crate::tools) fn resolve_thing_ref(
    session: &Session,
    value: &ThingRef,
) -> Result<i32, CyberError> {
    match value.clone() {
        ThingRef::Number(number) => stored_kind(number),
        ThingRef::Name(text) => {
            let definitions = thing_definitions(session)?;
            let resolved = resolve_thing_kind(&definitions, &text).map_err(|problem| {
                CyberError::InvalidArgument(problem).with_hint("get_thing_types with a search term")
            })?;
            stored_kind(resolved)
        }
    }
}

/// Refuse a resolved `to` doomednum no session definition carries.
/// `replace_thing_type` skips the check when the caller passes
/// `allow_unknown: true`, and then writes and reports the number as-is.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming `get_thing_types` and the
/// `allow_unknown` escape hatch.
#[inline]
pub(in crate::tools) fn require_defined_thing(
    definitions: &Definitions,
    to: i32,
) -> Result<(), CyberError> {
    let known = u32::try_from(to).is_ok_and(|kind| definitions.thing(kind).is_some());
    if known {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "thing type {to} is not defined in the session's tables: get_thing_types lists the \
         known numbers and names, or pass allow_unknown: true to write it anyway"
    )))
}
