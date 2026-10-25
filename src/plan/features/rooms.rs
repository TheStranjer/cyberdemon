//! The room-property steps of the pipeline: the lights and the secrets.
//!
//! The lights step paints every room's `light_effect`
//! [`LightEffect`](crate::wad::map::features::LightEffect) onto all its
//! sectors with [`light_special`](crate::wad::map::features::light_special)
//! (keeping the light level and every bit above the family's basic-type
//! field), and the secret step — always after the lights, so a lit
//! secret keeps both — flags every `secret` room's sectors with
//! [`secret_edit`](crate::wad::map::features::secret_edit), passing each
//! [`SecretEdit::boom_warning`] through
//! [`boom_note`](crate::wad::map::features::boom_note) into the compiled
//! plan's warnings.

use crate::plan::emit::CompiledPlan;
use crate::plan::model::Plan;
use crate::wad::map::features::{LightEffect, SecretEdit, boom_note, light_special, secret_edit};

use super::{FeatureRow, FeatureRun, owned_sectors};

/// The lights step: paint every room's `light_effect` onto all of the
/// room's sectors, in plan order — always before the secret step, so a
/// room that is both lit and hidden ends up carrying both.
pub(super) fn lights(plan: &Plan, compiled: &mut CompiledPlan, run: &mut FeatureRun) {
    for room in &plan.rooms {
        let Some(effect) = room.light_effect else {
            continue;
        };
        light_room(compiled, &room.id, effect, run);
    }
}

/// Paint `effect` onto every sector of `room`: the family's basic type
/// goes into the type field, the bits above it and the light level
/// stay (the plan-then-write pattern of the `set_light_effect` tool).
/// A room the emission kept no sector of (fully painted over, already
/// warned about) installs nothing and reports no row.
fn light_room(compiled: &mut CompiledPlan, room: &str, effect: LightEffect, run: &mut FeatureRun) {
    let sectors = owned_sectors(compiled, room);
    let family = compiled.map.flag_family();
    let edits: Vec<(usize, i32)> = sectors
        .iter()
        .filter_map(|&index| {
            compiled
                .map
                .sectors
                .get(index)
                .map(|row| (index, light_special(family, row.special, effect)))
        })
        .collect();
    for (index, special) in edits {
        if let Some(row) = compiled.map.sectors.get_mut(index) {
            row.special = special;
        }
    }
    if !sectors.is_empty() {
        run.rows.push(FeatureRow {
            kind: "light",
            room: Some(room.to_owned()),
            sectors,
            lines: Vec::new(),
            tag: None,
            line_ends: Vec::new(),
        });
    }
}

/// The secrets step, always after the lights: flag every room the plan
/// marks `secret`, so the room's sectors carry the family's secret flag
/// over whatever special the earlier steps left there.
pub(super) fn secrets(plan: &Plan, compiled: &mut CompiledPlan, run: &mut FeatureRun) {
    for room in &plan.rooms {
        if room.secret {
            secret_room(compiled, &room.id, run);
        }
    }
}

/// Set the family's secret flag on every sector of `room`, keeping
/// every bit the flag does not own and passing each
/// [`SecretEdit::boom_warning`] through the shared [`boom_note`] into
/// the compiled plan's warnings. A room the emission kept no sector of
/// installs nothing and reports no row.
///
/// [`SecretEdit::boom_warning`]: crate::wad::map::features::SecretEdit::boom_warning
fn secret_room(compiled: &mut CompiledPlan, room: &str, run: &mut FeatureRun) {
    let sectors = owned_sectors(compiled, room);
    let family = compiled.map.flag_family();
    let edits: Vec<(usize, SecretEdit)> = sectors
        .iter()
        .filter_map(|&index| {
            compiled
                .map
                .sectors
                .get(index)
                .map(|row| (index, secret_edit(family, row.special, true)))
        })
        .collect();
    for (index, edit) in edits {
        if edit.boom_warning {
            compiled.warnings.push(boom_note(index, edit.special));
        }
        if let Some(row) = compiled.map.sectors.get_mut(index) {
            row.special = edit.special;
        }
    }
    if !sectors.is_empty() {
        run.rows.push(FeatureRow {
            kind: "secret",
            room: Some(room.to_owned()),
            sectors,
            lines: Vec::new(),
            tag: None,
            line_ends: Vec::new(),
        });
    }
}
