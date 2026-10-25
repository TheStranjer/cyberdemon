//! `mark_secret` and `set_light_effect`: the two sector-special steps.
//!
//! The two sector-special tools stay on this side of the `features`
//! group because they answer the same question the feature tables
//! answer: what number makes a plain map element do something. Both are
//! one [`crate::session::Session::edit_map`] step over
//! [`crate::wad::map::Sector::special`] (plus
//! [`crate::wad::map::Sector::light`] for `set_light_effect`) with no
//! drawing at all: [`crate::wad::map::features::secret_edit`] is the
//! per-family secret bit behind `mark_secret`,
//! [`crate::wad::map::features::light_special`] the per-family basic
//! type behind `set_light_effect`, and each refused index is named
//! before anything is written. A Boom secret bit on a vanilla `Doom`
//! type earns [`crate::wad::map::features::boom_note`]'s warning, shared
//! with the plan feature engine's secret step so both paths say the same
//! thing.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::edit_result_fragment;
use crate::wad::map::Map;
use crate::wad::map::features::{LightEffect, SecretEdit, boom_note, light_special, secret_edit};

use super::{invalid, out_of_range};

/// The lowest light level the vanilla range allows.
pub(super) const LIGHT_MIN: i32 = 0_i32;

/// The highest light level the vanilla range allows (`u8::MAX`).
pub(super) const LIGHT_MAX: i32 = 255_i32;

/// Arguments for [`Cyberdemon::mark_secret`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct MarkSecretParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The sectors to change, as a non-empty list of indices (duplicates
    /// collapse, the edit runs in ascending order).
    pub(super) sectors: Vec<usize>,
    /// Whether the sectors are secret (default true); `false` clears the
    /// flag instead of setting it.
    #[serde(default = "default_secret")]
    pub(super) secret: bool,
}

/// Default [`MarkSecretParams::secret`]: marking, not clearing.
const fn default_secret() -> bool {
    true
}

/// Arguments for [`Cyberdemon::set_light_effect`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SetLightParams {
    /// The map to edit; defaults like [`MarkSecretParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The sectors to change, as a non-empty list of indices (duplicates
    /// collapse, the edit runs in ascending order).
    pub(super) sectors: Vec<usize>,
    /// The light effect: `none`, `flicker`, `strobe_fast`, `strobe_slow`,
    /// `glow` or `random_off`.
    pub(super) effect: String,
    /// The static light level to give the sectors, `0..=255` (optional:
    /// without it only the effect changes).
    #[serde(default)]
    pub(super) light: Option<i32>,
}

/// The sectors a special tool was asked to change: ascending, without
/// duplicates and never empty.
pub(super) fn sector_selection(list: &[usize]) -> Result<Vec<usize>, CyberError> {
    if list.is_empty() {
        return Err(invalid(
            "sectors is empty: pass at least one sector index (get_map_summary lists the \
             map's sectors)",
        ));
    }
    let mut indices = list.to_vec();
    indices.sort_unstable();
    indices.dedup();
    Ok(indices)
}

/// Refuses a light level outside the vanilla `0..=255`, every format alike.
pub(super) fn check_light_level(level: i32) -> Result<(), CyberError> {
    if (LIGHT_MIN..=LIGHT_MAX).contains(&level) {
        return Ok(());
    }
    Err(invalid(&format!(
        "light {level} is out of range: the light level holds {LIGHT_MIN}..={LIGHT_MAX}"
    )))
}

/// The [`LightEffect`] `text` names (case-insensitive), or the refusal
/// listing every valid name.
pub(super) fn effect_of(text: &str) -> Result<LightEffect, CyberError> {
    let wanted = text.to_ascii_lowercase();
    LightEffect::all()
        .into_iter()
        .find(|effect| effect.name() == wanted)
        .ok_or_else(|| {
            invalid(&format!(
                "effect {text:?} is not a light effect: pass one of {}",
                effect_names().join(", ")
            ))
        })
}

/// Every effect name, in [`LightEffect::all`] order.
pub(super) fn effect_names() -> Vec<&'static str> {
    LightEffect::all().map(LightEffect::name).to_vec()
}

/// Everything [`mark_secrets`] needs once the wire parameters resolved.
#[derive(Debug)]
pub(super) struct SecretSpec {
    /// The sectors to change, ascending and deduplicated.
    pub(super) indices: Vec<usize>,
    /// Whether they become secret or plain again.
    pub(super) secret: bool,
}

/// What one [`mark_secrets`] step wrote.
#[derive(Debug)]
pub(super) struct SecretStep {
    /// Indices of the sectors whose `special` the step changed.
    pub(super) changed: Vec<usize>,
    /// The Boom-bit notes for sectors a vanilla engine cannot read.
    pub(super) warnings: Vec<String>,
    /// The flag family whose bits were written.
    pub(super) family: &'static str,
}

/// Everything [`set_lights`] needs once the wire parameters resolved.
#[derive(Debug)]
pub(super) struct LightSpec {
    /// The sectors to change, ascending and deduplicated.
    pub(super) indices: Vec<usize>,
    /// The effect to install.
    pub(super) effect: LightEffect,
    /// The light level to give them, when one was asked for.
    pub(super) light: Option<i32>,
}

/// What one [`set_lights`] step wrote.
#[derive(Debug)]
pub(super) struct LightStep {
    /// Indices of the sectors whose special or light the step changed.
    pub(super) changed: Vec<usize>,
    /// The effect the step installed.
    pub(super) effect: LightEffect,
    /// The basic sector type the effect installs in this family.
    pub(super) special: i32,
    /// The flag family that wrote it.
    pub(super) family: &'static str,
}

/// The `(special, light)` pair of sector `index`, or the `invalid_argument`
/// naming the index past the map's sectors.
fn sector_state(map: &Map, index: usize) -> Result<(i32, i32), CyberError> {
    map.sectors
        .get(index)
        .map(|row| (row.special, row.light))
        .ok_or_else(|| out_of_range("sector", index, map.sectors.len()))
}

/// Store `special` in sector `index`, reporting whether the row changed.
fn write_special(map: &mut Map, index: usize, special: i32) -> bool {
    let Some(row) = map.sectors.get_mut(index) else {
        return false;
    };
    let changed = row.special != special;
    row.special = special;
    changed
}

/// Store the special `effect` and the level `light` in sector `index`,
/// reporting whether the row changed.
fn write_light(map: &mut Map, index: usize, effect: i32, light: i32) -> bool {
    let Some(row) = map.sectors.get_mut(index) else {
        return false;
    };
    let changed = row.special != effect || row.light != light;
    row.special = effect;
    row.light = light;
    changed
}

/// The checked `(index, edit)` pair of one sector a [`mark_secrets`] call
/// names.
type SecretPlan = (usize, SecretEdit);

/// The planned rows of a [`mark_secrets`] call: every selected sector's
/// secret edit, all computed before any of them is written.
fn planned_secrets(map: &Map, spec: &SecretSpec) -> Result<Vec<SecretPlan>, CyberError> {
    let family = map.flag_family();
    let mut plans = Vec::with_capacity(spec.indices.len());
    for index in &spec.indices {
        let (before, _) = sector_state(map, *index)?;
        plans.push((*index, secret_edit(family, before, spec.secret)));
    }
    Ok(plans)
}

/// The checked `(index, special, light)` triple of every sector a
/// [`set_lights`] call names, computed before any row is written.
type LightPlan = (usize, i32, i32);

/// The planned rows of a [`set_lights`] call, all computed up front.
fn planned_lights(map: &Map, spec: &LightSpec) -> Result<Vec<LightPlan>, CyberError> {
    let family = map.flag_family();
    let mut plans = Vec::with_capacity(spec.indices.len());
    for index in &spec.indices {
        let (before, level) = sector_state(map, *index)?;
        plans.push((
            *index,
            light_special(family, before, spec.effect),
            spec.light.unwrap_or(level),
        ));
    }
    Ok(plans)
}

/// Set or clear the family's secret flag on every selected sector, keeping
/// every bit the flag does not own.
pub(super) fn mark_secrets(map: &mut Map, spec: &SecretSpec) -> Result<SecretStep, CyberError> {
    let plans = planned_secrets(map, spec)?;
    let mut step = SecretStep {
        changed: Vec::new(),
        warnings: Vec::new(),
        family: map.flag_family().as_str(),
    };
    for (index, edit) in plans {
        if edit.boom_warning {
            step.warnings.push(boom_note(index, edit.special));
        }
        if write_special(map, index, edit.special) {
            step.changed.push(index);
        }
    }
    Ok(step)
}

/// Install the effect (and the asked-for light level) on every selected
/// sector, keeping the bits above the family's basic-type field.
pub(super) fn set_lights(map: &mut Map, spec: &LightSpec) -> Result<LightStep, CyberError> {
    let plans = planned_lights(map, spec)?;
    let family = map.flag_family();
    let mut step = LightStep {
        changed: Vec::new(),
        effect: spec.effect,
        special: spec.effect.basic_special(family),
        family: family.as_str(),
    };
    for (index, effect, light) in plans {
        if write_light(map, index, effect, light) {
            step.changed.push(index);
        }
    }
    Ok(step)
}

/// Structured result of [`Cyberdemon::mark_secret`].
#[derive(Serialize)]
pub(super) struct SecretToolReport {
    /// Whether the sectors were marked (`true`) or cleared (`false`).
    secret: bool,
    /// The flag family whose secret bit was written: `doom` or `hexen`.
    family: &'static str,
    /// Indices of the sectors whose special actually changed, ascending.
    sectors: Vec<usize>,
    /// The Boom-bit notes, empty unless a vanilla type was flagged.
    warnings: Vec<String>,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::set_light_effect`].
#[derive(Serialize)]
pub(super) struct LightToolReport {
    /// The canonical effect name the sectors now carry.
    effect: &'static str,
    /// The basic sector type the effect installs in this family.
    special: i32,
    /// The flag family that wrote it: `doom` or `hexen`.
    family: &'static str,
    /// The light level applied (`null` when the call asked for none).
    light: Option<i32>,
    /// Indices of the sectors whose special or light actually changed.
    sectors: Vec<usize>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// `mark_secret`'s work: resolve the selection and run the whole flagging
/// as one [`crate::session::Session::edit_map`] step.
pub(super) fn mark_secret_run(
    session: &mut Session,
    args: &MarkSecretParams,
) -> Result<SecretToolReport, CyberError> {
    let spec = SecretSpec {
        indices: sector_selection(&args.sectors)?,
        secret: args.secret,
    };
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = if spec.secret {
        "mark secret"
    } else {
        "clear secret"
    };
    let (step, report) = session.edit_map(name, label, |map| mark_secrets(map, &spec))?;
    Ok(SecretToolReport {
        secret: spec.secret,
        family: step.family,
        sectors: step.changed,
        warnings: step.warnings,
        edit: edit_result_fragment(&report),
    })
}

/// `set_light_effect`'s work: resolve the effect and the level, then run
/// the whole repaint as one [`crate::session::Session::edit_map`] step.
pub(super) fn set_light_run(
    session: &mut Session,
    args: &SetLightParams,
) -> Result<LightToolReport, CyberError> {
    let effect = effect_of(&args.effect)?;
    args.light.map_or(Ok(()), check_light_level)?;
    let spec = LightSpec {
        indices: sector_selection(&args.sectors)?,
        effect,
        light: args.light,
    };
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = "set light effect";
    let (step, report) = session.edit_map(name, label, |map| set_lights(map, &spec))?;
    Ok(LightToolReport {
        effect: step.effect.name(),
        special: step.special,
        family: step.family,
        light: spec.light,
        sectors: step.changed,
        edit: edit_result_fragment(&report),
    })
}
