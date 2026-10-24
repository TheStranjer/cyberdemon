//! The bulk-editing machinery the `replace_*` tools are built on: the
//! `maps` selector, the one-undoable-step-over-many-map-blocks runner
//! and the merged view's replaceable-texture stock.
//!
//! [`MapsSelector`] reads the wire shape every bulk edit documents
//! (`"all"`, a list of map names, or absent for the session's default
//! map) and [`resolve`] turns it into the concrete [`LumpName`] list to
//! act on, exactly as the merged view resolves them. [`apply_to_maps`]
//! then runs one pure per-map pass (a [`crate::wad::map::bulk`]
//! replacement, or anything else shaped `Fn(&mut Map) -> T`) over every
//! named block inside a single [`Session::edit`], the one-step pattern
//! `build_all_nodes` uses in the nodes tool group: the copies into the
//! working archive and all rewrites undo together, and a refusal
//! mid-loop leaves the session exactly as it was. A
//! [`ApplyOptions::dry_run`] instead applies the pass to merged-view
//! clones, reporting what would change while writing nothing and
//! recording no history.
//!
//! [`Stock`] answers "does this texture exist?" for replacement
//! targets. It loads the merged view through the theme tools'
//! inventory, so it holds exactly what the theme roles resolve
//! against: the composite `TEXTURE1`/`TEXTURE2` names, the `TX_` lump
//! names, and the flats namespace plus `F_SKY1`.
//!
//! [`ThingRef`] is `replace_thing_type`'s number-or-name side:
//! `resolve_thing_ref` turns a doomednum straight into the stored
//! type number and a definition name through
//! [`crate::plan::things::resolve_thing_kind`] over the session's
//! definitions (exact case-insensitive name first, then a unique
//! case-insensitive substring), and `require_defined_thing` checks a
//! resolved target against the same tables unless the caller passes
//! the tool's `allow_unknown` escape.
//!
//! The module sits under the query tools because CD-108's bulk edits
//! act on exactly the element indices `find_elements` reports. The
//! `replace_texture`, `replace_flat` and `replace_thing_type` `#[tool]`
//! methods live on the parent and keep their heavy lifting here:
//! [`parse_terms`] and [`require_target`] shape the wire parameters,
//! [`run_replacement`] folds a pure per-map pass over the [`resolve`]d
//! selection into one [`ReplaceReport`].
//!
//! The selector and [`resolve`] stay here; the runner, its [`ReplaceReport`]
//! and the folded counts live in the private `apply` module, the
//! [`Stock`] checks, [`parse_terms`], [`require_target`] and [`ThingRef`]
//! resolution in the private `stock` module, both re-exported below;
//! the tests are the private `tests` module, its fixture builders
//! shared, the selecting-and-applying cases and the stock-and-thing
//! cases one topic file apiece.

mod apply;
mod stock;

pub use apply::{
    ApplyOptions, MapReplaceRun, ReplaceReport, Replacement, apply_to_maps, run_replacement,
};
pub use stock::{Stock, ThingRef, parse_terms, require_target};
pub(in crate::tools) use stock::{require_defined_thing, resolve_thing_ref, thing_definitions};

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use core::fmt::{self, Formatter};

use schemars::json_schema;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use serde::de::{self, Deserializer, Unexpected, Visitor};
use serde_json::Value;

use crate::error::CyberError;
use crate::session::{Scope, Session};
use crate::tools::maps::common;
use crate::wad::name::LumpName;

/// The marker payload of [`MapsSelector::All`]: deserializes exactly
/// the literal string `"all"` and nothing else.
///
/// Serde's untagged matching cannot bind a unit variant to a string at
/// all, so this one-string type is what makes [`MapsSelector::All`] the
/// `"all"` literal; every other bare string falls through to
/// [`MapsSelector::Other`] for [`resolve`]'s instructive refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllMaps;

impl JsonSchema for AllMaps {
    /// The wire schema of the `"all"` literal: that one string.
    #[inline]
    fn schema_name() -> Cow<'static, str> {
        "AllMaps".into()
    }

    /// The string-constant schema of the `"all"` literal.
    #[inline]
    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "const": "all"
        })
    }
}

impl<'de> Deserialize<'de> for AllMaps {
    /// Reads through the `AllMapsVisitor`: the literal `"all"` or a
    /// deserialization error.
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(AllMapsVisitor)
    }
}

/// The visitor behind [`AllMaps`]: the string `"all"` or an
/// invalid-value error.
struct AllMapsVisitor;

impl Visitor<'_> for AllMapsVisitor {
    type Value = AllMaps;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("the literal string \"all\"")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if v == "all" {
            return Ok(AllMaps);
        }
        Err(E::invalid_value(Unexpected::Str(v), &self))
    }
}

/// The `maps` parameter of a bulk edit: the literal string `"all"`, a
/// list of map names, or anything else.
///
/// The absent form (`None`, which is a parameter's own default for this
/// type) means the session's default map: the active map, else the
/// merged view's only block. An unrecognized value is recorded, not
/// rejected at the serde boundary, so [`resolve`] can refuse it with
/// the documented instruction.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MapsSelector {
    /// Every distinct map block of the merged view: the literal
    /// string `"all"`, read through [`AllMaps`].
    All(AllMaps),
    /// Exactly these maps, named as `list_maps` shows them.
    Names(Vec<String>),
    /// Anything that is neither `"all"` nor a list of names (a bare
    /// string, a number, an object, a mixed list): kept verbatim so
    /// [`resolve`] can quote it in its refusal.
    Other(Value),
}

/// The map blocks one bulk call acts on.
///
/// The `None` selector takes the session's default map, resolved
/// exactly like every other map tool's `map` parameter.
///
/// [`MapsSelector::All`] answers with every distinct block name of the
/// merged view, in directory order. A name carried by two blocks
/// appears once; later steps resolve it to the last block, exactly as
/// the shared `Directory` resolves a double-named map.
///
/// [`MapsSelector::Names`] answers with the parsed names, deduplicated
/// in the order given. [`MapsSelector::Other`] is refused with a
/// [`CyberError::InvalidArgument`] naming `"all"` and `list_maps`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a selector that is neither
/// `"all"` nor a list, for an empty list, for a name that does not
/// parse, and for the no-default case of `None` (no active map and no
/// single map in the merged view); [`CyberError::NotFound`] for a named
/// map the merged view does not hold.
#[inline]
pub fn resolve(
    session: &Session,
    maps: Option<&MapsSelector>,
) -> Result<Vec<LumpName>, CyberError> {
    let Some(selector) = maps else {
        return common::resolve_map_name(session, None).map(|name| vec![name]);
    };
    // Matching the owned clone keeps every pattern's shape explicit;
    // `IndexSelector::indices` in the shared params follows the same
    // consume-a-clone style for the same reason.
    match selector.clone() {
        MapsSelector::All(..) => every_map(session),
        MapsSelector::Names(names) => listed_maps(session, &names),
        MapsSelector::Other(value) => Err(stray_selector(&value)),
    }
}

/// The refusal for a `maps` value that is neither `"all"` nor a list.
fn stray_selector(value: &Value) -> CyberError {
    CyberError::InvalidArgument(format!(
        "maps {value} is not a map selection: pass \"all\" for every map of the \
         merged view, a list of map names, or leave the parameter out for the \
         active map (list_maps shows the names)"
    ))
}

/// Every distinct block name of the merged view, in directory order.
fn every_map(session: &Session) -> Result<Vec<LumpName>, CyberError> {
    let dir = common::directory(session, Scope::Merged)?;
    let mut names = Vec::new();
    for block in &dir.analysis.maps {
        push_distinct(&mut names, block.name);
    }
    Ok(names)
}

/// The listed names, parsed, deduplicated and checked against the
/// merged view.
fn listed_maps(session: &Session, texts: &[String]) -> Result<Vec<LumpName>, CyberError> {
    if texts.is_empty() {
        return Err(CyberError::InvalidArgument(
            "the maps list is empty: pass at least one map name or \"all\"".to_owned(),
        ));
    }
    let dir = common::directory(session, Scope::Merged)?;
    let mut names = Vec::new();
    for text in texts {
        let name = LumpName::parse(text)?;
        if dir.find_block(name).is_none() {
            return Err(common::missing_map(name, Scope::Merged));
        }
        push_distinct(&mut names, name);
    }
    Ok(names)
}

/// Appends `name` unless the list already carries it.
fn push_distinct(names: &mut Vec<LumpName>, name: LumpName) {
    if !names.contains(&name) {
        names.push(name);
    }
}

#[cfg(test)]
mod tests;
