//! The three `replace_*` swaps as the `find_elements` group runs them:
//! `replace_texture`, `replace_flat` and `replace_thing_type`, each one's
//! wire parameters beside the run that folds a pure
//! [`crate::wad::map::bulk`] pass over the selected map blocks.
//!
//! [`texture_run`] and [`flat_run`] shape the wire list with
//! [`parse_terms`], check the target against the merged view's stock with
//! [`require_target`] and [`Stock`], and hand the pass to
//! [`run_replacement`] as one undoable step. [`thing_type_run`] resolves
//! both sides through [`resolve_side`] (each refusal naming the parameter
//! that carried it), admits an undefined target only through
//! `allow_unknown`, and reports its two resolved numbers beside the shared
//! [`ReplaceReport`] fields.
//!
//! The session-facing machinery all three share, the `maps` selector, the
//! many-map runner and the stock and thing-name checks, is the public
//! `replace` module's; `bulk_edit`, the other tool this group reports
//! through, is the `bulk` module's.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::query::replace::{
    MapsSelector, ReplaceReport, Replacement, Stock, ThingRef, parse_terms, require_defined_thing,
    require_target, resolve_thing_ref, run_replacement,
};
use crate::wad::map::bulk::{Face, Surface, replace_flats, replace_textures, replace_thing_types};

/// Arguments for [`Cyberdemon::replace_texture`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ReplaceTextureParams {
    /// The case-insensitive glob (`*` any run, `?` one character) of
    /// texture names to replace; `"WALL1"` names one texture, `"WALL*"`
    /// sweeps its family and `"*"` takes every face, `"-"` and the empty
    /// ones included.
    pub(super) from: String,
    /// The texture every matched face gets: a wall texture the merged
    /// view holds (`list_textures` shows them; uppercased) or `"-"`,
    /// which clears the face. An unknown name fails `not_found`.
    pub(super) to: String,
    /// The sidedef faces to rewrite: any of `"upper"`, `"middle"`,
    /// `"lower"` (case-insensitive). Absent or empty means all three.
    #[serde(default)]
    pub(super) parts: Option<Vec<String>>,
    /// The map blocks to edit: absent takes the session's default map
    /// (the active map, else the merged view's only map), `"all"` every
    /// distinct block of the merged view, a list exactly those names.
    /// A map a read-only layer holds is copied into the working archive
    /// first, inside the same undo step.
    #[serde(default)]
    pub(super) maps: Option<MapsSelector>,
    /// Report the per-map counts without writing anything or touching
    /// the undo stack (default `false`).
    #[serde(default)]
    pub(super) dry_run: Option<bool>,
}

/// Arguments for [`Cyberdemon::replace_flat`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ReplaceFlatParams {
    /// The case-insensitive glob of flat names to replace, matched
    /// against every targeted sector surface.
    pub(super) from: String,
    /// The flat every matched surface gets: a flat the merged view holds
    /// (`list_flats` shows them, `F_SKY1` included; uppercased). An
    /// unknown name fails `not_found`; unlike a wall swap there is no
    /// clearing `"-"` for a surface.
    pub(super) to: String,
    /// The sector surfaces to rewrite: any of `"floor"`, `"ceiling"`
    /// (case-insensitive). Absent or empty means both.
    #[serde(default)]
    pub(super) surfaces: Option<Vec<String>>,
    /// The map blocks to edit, exactly as in [`ReplaceTextureParams::maps`].
    #[serde(default)]
    pub(super) maps: Option<MapsSelector>,
    /// Report the per-map counts without writing anything or touching
    /// the undo stack (default `false`).
    #[serde(default)]
    pub(super) dry_run: Option<bool>,
}

/// `replace_texture`'s work: parse the face list, check the target
/// against the merged view's wall stock (the clearing `"-"` excepted)
/// and run [`replace_textures`] over the selected maps as one step.
///
/// # Errors
///
/// [`parse_terms`]'s and [`require_target`]'s refusals for a stray face
/// or target name, [`Stock`]'s `not_found` for a texture that does not
/// exist and [`run_replacement`]'s for a map no scope holds.
pub(super) fn texture_run(
    session: &mut Session,
    args: &ReplaceTextureParams,
) -> Result<ReplaceReport, CyberError> {
    let faces = parse_terms(args.parts.as_deref(), Face::parse)?;
    let to = require_target(session, &args.to, true, Stock::require_wall)?;
    let label = format!("replace_texture {} -> {to}", args.from.to_ascii_uppercase());
    let replacement = Replacement {
        label: &label,
        maps: args.maps.as_ref(),
        dry_run: args.dry_run.unwrap_or(false),
    };
    let from = args.from.as_str();
    run_replacement(session, &replacement, |map| {
        replace_textures(map, from, &to, &faces)
    })
}

/// `replace_flat`'s work: parse the surface list, check the target
/// against the merged view's flats and run [`replace_flats`] over the
/// selected maps as one step.
///
/// # Errors
///
/// [`parse_terms`]'s and [`require_target`]'s refusals for a stray
/// surface or flat name and [`run_replacement`]'s for a map no scope
/// holds.
pub(super) fn flat_run(
    session: &mut Session,
    args: &ReplaceFlatParams,
) -> Result<ReplaceReport, CyberError> {
    let surfaces = parse_terms(args.surfaces.as_deref(), Surface::parse)?;
    let to = require_target(session, &args.to, false, Stock::require_flat)?;
    let label = format!("replace_flat {} -> {to}", args.from.to_ascii_uppercase());
    let replacement = Replacement {
        label: &label,
        maps: args.maps.as_ref(),
        dry_run: args.dry_run.unwrap_or(false),
    };
    let from = args.from.as_str();
    run_replacement(session, &replacement, |map| {
        replace_flats(map, from, &to, &surfaces)
    })
}

/// Arguments for [`Cyberdemon::replace_thing_type`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ReplaceThingTypeParams {
    /// The things to rewrite: the doomednum as stored, or a definition
    /// name (`get_thing_types` shows them; an exact case-insensitive
    /// name wins, else a unique case-insensitive substring). A name
    /// nothing holds or several types share fails with `invalid_argument`
    /// naming `from`.
    pub(super) from: ThingRef,
    /// What every matched thing gets: a doomednum or a definition name,
    /// resolved exactly as [`Self::from`]. The resolved number must
    /// exist in the session's definitions unless `allow_unknown` is set.
    pub(super) to: ThingRef,
    /// The map blocks to edit, exactly as in [`ReplaceTextureParams::maps`].
    #[serde(default)]
    pub(super) maps: Option<MapsSelector>,
    /// Report the per-map counts without writing anything or touching
    /// the undo stack (default `false`).
    #[serde(default)]
    pub(super) dry_run: Option<bool>,
    /// Write a `to` number the session does not define without the
    /// existence check, and report it as-is (default `false`). The
    /// number must still fit each targeted map's format.
    #[serde(default)]
    pub(super) allow_unknown: Option<bool>,
}

/// Structured result of [`Cyberdemon::replace_thing_type`]: the shared
/// [`ReplaceReport`] fold beside the two resolved numbers.
#[derive(Debug, Serialize)]
pub(super) struct ThingSwapReport {
    /// The resolved thing type the call rewrote.
    pub(super) from: i32,
    /// The resolved thing type the call wrote.
    pub(super) to: i32,
    /// The glob swaps' shared fields: `maps`, `total`, `copied_maps`,
    /// `dry_run` and `next_steps`.
    #[serde(flatten)]
    pub(super) swap: ReplaceReport,
}

/// [`resolve_thing_ref`]'s answer with an `invalid_argument` refusal
/// attributed to the `parameter` that carried the bad reference.
fn resolve_side(session: &Session, value: &ThingRef, parameter: &str) -> Result<i32, CyberError> {
    match resolve_thing_ref(session, value) {
        Err(error) if error.is_invalid_argument() => {
            Err(error.map_message(|problem| format!("{parameter}: {problem}")))
        }
        answered => answered,
    }
}

/// `replace_thing_type`'s work: resolve both sides (each failure naming
/// its parameter), refuse an undefined `to` unless `allow_unknown` lets
/// it through, and run [`replace_thing_types`] over the selected maps
/// as one step.
///
/// # Errors
///
/// [`resolve_side`]'s and [`require_defined_thing`]'s
/// [`CyberError::InvalidArgument`] refusals and [`run_replacement`]'s
/// for a map no scope holds.
pub(super) fn thing_type_run(
    session: &mut Session,
    args: &ReplaceThingTypeParams,
) -> Result<ThingSwapReport, CyberError> {
    let from = resolve_side(session, &args.from, "from")?;
    let to = resolve_side(session, &args.to, "to")?;
    if !args.allow_unknown.unwrap_or(false) {
        require_defined_thing(&session.definitions()?, to)?;
    }
    let label = format!("replace_thing_type {from} -> {to}");
    let replacement = Replacement {
        label: &label,
        maps: args.maps.as_ref(),
        dry_run: args.dry_run.unwrap_or(false),
    };
    let swap = run_replacement(session, &replacement, |map| {
        replace_thing_types(map, from, to)
    })?;
    Ok(ThingSwapReport { from, to, swap })
}
