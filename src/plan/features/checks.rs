//! The playability checks beside the pipeline: the judgment gaps the
//! wiring cannot see, reported as advisory warnings — never errors.
//!
//! [`playable_checks`] reports every locked door whose key colour stands
//! nowhere among the finished map's things (matched through the thing
//! definitions: the [`KEYS_CATEGORY`] and the colour word in the name,
//! skull keys included, since their names carry the colour), a plan
//! carrying no `exit` row, and every `remote` door whose id no `switch`
//! row names in its `opens`. The notes come out in plan order — key
//! warnings per door in connection order, then the exit note, then the
//! remote notes in connection order — and the tool appends them after
//! the compiled plan's existing warnings. [`remote_door`] answers the
//! switch step's lookup of the wired door a row names.

use alloc::collections::BTreeSet;

use crate::defs::Definitions;
use crate::error::CyberError;
use crate::plan::model::{Connection, ConnectionKind, DoorStyle, Feature, KeyColor, Plan};
use crate::wad::map::Map;
use crate::wad::map::model::Thing;

use super::lifts::connection_name;
use super::{FeatureRun, invalid};

/// The `Keys` thing-definition category the key check matches
/// (`data/defs/doom_things.tsv` files the cards and skull keys under it).
const KEYS_CATEGORY: &str = "Keys";

/// The lower-case colour word the key check finds in a key definition's
/// name; skull keys carry the same word, so they count alike.
const fn color_word(color: KeyColor) -> &'static str {
    match color {
        KeyColor::Blue => "blue",
        KeyColor::Yellow => "yellow",
        KeyColor::Red => "red",
    }
}

/// The never-error playability checks beside the pipeline: the judgment
/// gaps a wired plan can still carry, reported as advisory warnings.
///
/// Three checks, in this order: every locked door connector whose colour
/// no thing of `map` answers as a key (matched through `definitions`: a
/// `Keys`-category row whose name carries the colour word), one warning
/// per door in connection order; the plan's whole set of features, which
/// earns `"the map has no exit"` when it carries no `exit` row; and
/// every `remote` door whose id no `switch` row names in its `opens`,
/// one warning per door in connection order. `map` is the finished map —
/// the check reads the things that actually stand in it, planned rows
/// and the teleporter's own alike, since only their doomednums are
/// known. Nothing here refuses: the warnings join the compiled plan's
/// list beside [`apply_features`](super::apply_features)' notes, and what to do about each is
/// the caller's judgment.
#[must_use]
#[inline]
pub fn playable_checks(plan: &Plan, map: &Map, definitions: &Definitions) -> Vec<String> {
    let mut notes = missing_keys(plan, map, definitions);
    if !has_exit(plan) {
        notes.push("the map has no exit".to_owned());
    }
    notes.extend(unswitched_remotes(plan));
    notes
}

/// The locked-door warnings: one per locked connector with no key of its
/// colour among the map's things, in connection order.
fn missing_keys(plan: &Plan, map: &Map, definitions: &Definitions) -> Vec<String> {
    let mut notes = Vec::new();
    for connection in &plan.connections {
        let Some(color) = locked_color(connection) else {
            continue;
        };
        if map
            .things
            .iter()
            .any(|spawn| definition_is_key(spawn, definitions, color))
        {
            continue;
        }
        let word = color_word(color);
        notes.push(format!(
            "{word} door at {} but no {word} key in the plan",
            connection_name(connection)
        ));
    }
    notes
}

/// The key colour of a locked-door connection, `None` for every other
/// row.
const fn locked_color(connection: &Connection) -> Option<KeyColor> {
    match connection.kind {
        ConnectionKind::Door {
            style: DoorStyle::Locked(color),
        } => Some(color),
        ConnectionKind::Opening
        | ConnectionKind::Window { .. }
        | ConnectionKind::Stairs { .. }
        | ConnectionKind::Door { .. } => None,
    }
}

/// Whether `spawn` stands as a key of `color`'s colour in `definitions`:
/// its row in [`KEYS_CATEGORY`] with the [`color_word`] in its name.
fn definition_is_key(spawn: &Thing, definitions: &Definitions, color: KeyColor) -> bool {
    let word = color_word(color);
    u32::try_from(spawn.kind)
        .ok()
        .and_then(|kind| definitions.thing(kind))
        .is_some_and(|row| {
            row.category == KEYS_CATEGORY && row.name.to_ascii_lowercase().contains(word)
        })
}

/// Whether the plan carries any `exit` feature at all.
fn has_exit(plan: &Plan) -> bool {
    plan.features
        .iter()
        .any(|feature| matches!(feature, Feature::Exit { .. }))
}

/// The remote-door warnings: one per remote connector no `switch` row
/// names in its `opens`, in connection order. An unnamed connection can
/// never be referenced — a switch aimed at its `from->to` label is
/// refused — so its note asks for the `id` first.
fn unswitched_remotes(plan: &Plan) -> Vec<String> {
    let mut switched: BTreeSet<String> = BTreeSet::new();
    for feature in &plan.features {
        if !matches!(feature, Feature::Switch { .. }) {
            continue;
        }
        // A `&Feature` cannot be destructured with bindings under this
        // crate's denied `pattern_type_mismatch` and `ref_patterns`, so
        // only the matching row is cloned, never the whole vector.
        let Feature::Switch { opens, .. } = feature.clone() else {
            continue;
        };
        switched.insert(opens);
    }
    let mut notes = Vec::new();
    for connection in &plan.connections {
        if !matches!(
            connection.kind,
            ConnectionKind::Door {
                style: DoorStyle::Remote
            }
        ) {
            continue;
        }
        let label = connection_name(connection);
        if !switched.contains(label.as_str()) {
            notes.push(remote_hint(connection, &label));
        }
    }
    notes
}

/// The note for one unswitched remote door: a named door points the
/// caller at a `switch` row whose `opens` is its id; an unnamed one has
/// no handle to aim a switch at, so the note asks for the id first.
fn remote_hint(connection: &Connection, label: &str) -> String {
    if connection.id.is_some() {
        format!(
            "remote door at {label} is opened by no switch: add a switch feature whose \
                 opens is {label}"
        )
    } else {
        format!(
            "remote door at {label} is opened by no switch: give the connection an id \
                 and reference it in a switch feature's opens"
        )
    }
}

/// A remote door a switch row may fire: its tagged band sector.
#[derive(Debug)]
pub(super) struct RemoteDoor {
    /// The band sector `target_sector` points at; the door step already
    /// stamped its tag, so [`build_switch`](crate::wad::map::features::build_switch) keeps it instead of minting
    /// a second one.
    pub(super) band: usize,
}

/// Find the wired remote door `opens` names: the plan row must carry
/// the id, be a [`DoorStyle::Remote`] door, and have been tagged by the
/// door step (a band a later room painted over allocated nothing).
pub(super) fn remote_door(
    plan: &Plan,
    run: &FeatureRun,
    opens: &str,
) -> Result<RemoteDoor, CyberError> {
    let styled = plan.connections.iter().any(|row| {
        row.id.as_deref() == Some(opens)
            && matches!(
                row.kind,
                ConnectionKind::Door {
                    style: DoorStyle::Remote
                }
            )
    });
    let wired = styled
        .then(|| {
            Some(RemoteDoor {
                band: *run.bands.get(opens)?,
            })
        })
        .flatten();
    wired.ok_or_else(|| not_remote(opens))
}

/// The refusal for a switch whose `opens` names no wired remote door.
fn not_remote(opens: &str) -> CyberError {
    invalid(&format!(
        "switch opens {opens} which is not a remote door: point it at a door connection \
         of style remote, or leave the door for its own trigger"
    ))
}
