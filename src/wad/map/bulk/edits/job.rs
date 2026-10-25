//! The [`Job`] of one [`apply`](super::apply) run: its request checks
//! and its row plans.
//!
//! A [`Job`] bundles one [`FieldEdits`] request with its kind's
//! [`KindPlan`], the [`EditScope`] and the bit masks the request's flag
//! names resolved into; [`new_job`] refuses every request the kind cannot
//! carry before any map is read. [`Job::propose`] then turns one selected
//! row into a [`Pending`] of planned values — the `set` values, the `add`
//! amounts shifted onto what the row holds and the resulting flag bits —
//! with every resulting number range-checked and a sector's ceiling kept
//! at or above its floor, so the whole selection is known good before
//! [`commit`](super::commit::commit) writes any of it.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde_json::Value;

use crate::error::CyberError;
use crate::wad::map::flags::from_names;
use crate::wad::map::model::Map;
use crate::wad::map::query::ElementKind;

use super::access::{bounds, wrong_family, wrong_family_error};
use super::commit::{
    check_add_fields, check_numbers, check_overlap, check_values, find_field, names_of, plan_add,
    plan_set,
};
use super::tables::{KindPlan, plan_for};
use super::{EditScope, FieldEdits, FieldKind, FieldSpec, NO_TEXTURE, Stock};

/// One [`apply`](super::apply) run: the request, its kind's plan, the
/// scope and the resolved flag masks.
pub(super) struct Job<'req> {
    /// The selected element block.
    pub(super) kind: ElementKind,
    /// The block's table and accessors.
    pub(super) plan: &'static KindPlan,
    /// The requested field changes.
    pub(super) edits: &'req FieldEdits,
    /// Format, flag family and stock sets.
    pub(super) scope: &'req EditScope<'req>,
    /// Bits the `flags_set` names carry; `0` without a kind or a name.
    pub(super) set_mask: u32,
    /// Bits the `flags_clear` names carry; `0` without a kind or a name.
    pub(super) clear_mask: u32,
}

/// One selected row's planned values, computed before anything is written.
pub(super) struct Pending {
    /// Row position in the block.
    pub(super) index: usize,
    /// Planned integers by field name.
    pub(super) nums: Vec<(&'static str, i64)>,
    /// Planned names (uppercase, stock-checked) by field name.
    pub(super) texts: Vec<(&'static str, String)>,
    /// Planned flag bits, `None` when this request leaves flags alone.
    pub(super) flags: Option<u32>,
}

/// Builds the run and refuses every request it cannot carry, with the
/// flag names resolved into the masks [`Job::plan_flags`] applies.
pub(super) fn new_job<'req>(
    kind: ElementKind,
    edits: &'req FieldEdits,
    scope: &'req EditScope<'req>,
) -> Result<Job<'req>, CyberError> {
    let mut job = Job {
        kind,
        plan: plan_for(kind, scope.family),
        edits,
        scope,
        set_mask: 0_u32,
        clear_mask: 0_u32,
    };
    job.check_request()?;
    Ok(job)
}

impl Job<'_> {
    /// Refuses a request this kind cannot carry before any map reads.
    fn check_request(&mut self) -> Result<(), CyberError> {
        if self.edits.is_empty() {
            return Err(CyberError::InvalidArgument(
                "nothing to change: pass at least one of set, add, flags_set or \
                 flags_clear"
                    .to_owned(),
            ));
        }
        self.check_flags()?;
        self.check_set_fields()?;
        check_add_fields(self)?;
        check_overlap(self)?;
        check_values(self)
    }

    /// Refuses flag names on the kinds without a `flags` column; resolves
    /// them into this run's set and clear masks on the kinds with one.
    fn check_flags(&mut self) -> Result<(), CyberError> {
        match self.plan.flag_kind {
            None if self.edits.flags_set.is_empty() && self.edits.flags_clear.is_empty() => Ok(()),
            None => Err(CyberError::InvalidArgument(format!(
                "{} have no flags field: flags_set and flags_clear apply to linedefs \
                 and things",
                self.kind.as_str()
            ))),
            Some(flag_kind) => {
                self.set_mask = from_names(&self.edits.flags_set, flag_kind, self.scope.family)?;
                self.clear_mask =
                    from_names(&self.edits.flags_clear, flag_kind, self.scope.family)?;
                Ok(())
            }
        }
    }

    /// Whether this request touches the row's `flags` column.
    fn carries_flags(&self) -> bool {
        self.plan.flag_kind.is_some()
            && !(self.edits.flags_set.is_empty() && self.edits.flags_clear.is_empty())
    }

    /// Refuses a `set` on vertices and every field name outside the table.
    fn check_set_fields(&self) -> Result<(), CyberError> {
        if self.kind == ElementKind::Vertices && !self.edits.set.is_empty() {
            return Err(CyberError::InvalidArgument(format!(
                "vertices support add on {} only, never set: move these values \
                 into add",
                names_of(self.plan.fields, true)
            )));
        }
        for key in self.edits.set.keys() {
            self.field(key)?;
        }
        Ok(())
    }

    /// Refuses a text value that is not a string or, unless
    /// `allow_missing`, a name the scope's stock does not hold.
    pub(super) fn check_name(
        &self,
        stock: Stock,
        field: &str,
        value: &Value,
    ) -> Result<(), CyberError> {
        let Some(text) = value.as_str() else {
            return Err(CyberError::InvalidArgument(format!(
                "field '{field}' needs a name string, got {value}"
            )));
        };
        if matches!(stock, Stock::Walls) && text == NO_TEXTURE {
            return Ok(());
        }
        let wanted = text.to_ascii_uppercase();
        let held = match stock {
            Stock::Flats => self.scope.flats,
            Stock::Walls => self.scope.walls,
        };
        if held.contains(&wanted) || self.scope.allow_missing {
            return Ok(());
        }
        Err(CyberError::NotFound(format!(
            "{field} '{wanted}' is not part of the map's stock: pass \
             allow_missing: true to write the name anyway, or pick a stocked one"
        )))
    }

    /// The table entry a request name refers to. The other flag family's
    /// linedef names refuse with their own message.
    pub(super) fn field(&self, key: &str) -> Result<&'static FieldSpec, CyberError> {
        if let Some(spec) = find_field(self.plan.fields, key) {
            return Ok(spec);
        }
        if self.kind == ElementKind::Linedefs && wrong_family(self.scope.family, key) {
            return Err(wrong_family_error(self.scope.family, key));
        }
        Err(CyberError::InvalidArgument(format!(
            "unknown field '{key}' for {}: valid fields are {}",
            self.kind.as_str(),
            names_of(self.plan.fields, false)
        )))
    }

    /// The sorted, deduplicated selection, refused past the block's end.
    pub(super) fn selection(
        &self,
        map: &Map,
        indices: &[usize],
    ) -> Result<BTreeSet<usize>, CyberError> {
        let count = self.kind.count_on(map);
        if let Some(past) = indices.iter().find(|index| **index >= count) {
            return Err(CyberError::NotFound(format!(
                "{} {past} does not exist (the map holds {count} {}): pass \
                 existing indices, as find_elements answers with",
                self.plan.thing,
                self.kind.as_str()
            )));
        }
        Ok(indices.iter().copied().collect())
    }

    /// The row's full planned result, range-checked and not yet written.
    pub(super) fn propose(&self, map: &Map, index: usize) -> Result<Pending, CyberError> {
        let mut pending = Pending {
            index,
            nums: Vec::new(),
            texts: Vec::new(),
            flags: None,
        };
        plan_set(self, &mut pending)?;
        plan_add(self, map, &mut pending)?;
        self.plan_flags(map, &mut pending)?;
        check_numbers(self, map, &pending)?;
        self.check_height_order(map, &pending)?;
        Ok(pending)
    }

    /// Plans the row's resulting flag bits: the clear mask off first, the
    /// set mask on last, so a bit named on both ends comes out set.
    fn plan_flags(&self, map: &Map, pending: &mut Pending) -> Result<(), CyberError> {
        if !self.carries_flags() {
            return Ok(());
        }
        let Some(current) = (self.plan.get_flags)(map, pending.index) else {
            return Err(self.vanished("flags"));
        };
        pending.flags = Some(self.set_mask | (current & !self.clear_mask));
        Ok(())
    }

    /// Refuses one resulting numeric value outside its field's range.
    pub(super) fn check_number(
        &self,
        spec: &FieldSpec,
        index: usize,
        value: i64,
    ) -> Result<(), CyberError> {
        let FieldKind::Num(range) = spec.kind else {
            return Ok(());
        };
        let (low, high) = bounds(range, self.scope.format);
        if (low..=high).contains(&value) {
            return Ok(());
        }
        Err(CyberError::InvalidArgument(format!(
            "{} {index} {} would become {value}; {} must stay within {low}..={high}",
            self.plan.thing, spec.name, spec.name
        )))
    }

    /// Refuses a resulting sector whose ceiling ends below its floor.
    fn check_height_order(&self, map: &Map, pending: &Pending) -> Result<(), CyberError> {
        if self.kind != ElementKind::Sectors {
            return Ok(());
        }
        let Some(floor) = self.resulting(map, pending, "floor") else {
            return Err(self.vanished("floor"));
        };
        let Some(ceiling) = self.resulting(map, pending, "ceiling") else {
            return Err(self.vanished("ceiling"));
        };
        if ceiling >= floor {
            return Ok(());
        }
        Err(CyberError::InvalidArgument(format!(
            "{} {} would end with floor {floor} above ceiling {ceiling}: keep the \
             ceiling at or above the floor (equal is a closed door)",
            self.plan.thing, pending.index
        )))
    }

    /// The resulting row's value for a numeric field: planned or current.
    pub(super) fn resulting(&self, map: &Map, pending: &Pending, field: &str) -> Option<i64> {
        let current = i64::from(self.current(map, pending.index, field)?);
        Some(
            pending
                .nums
                .iter()
                .find(|planned| planned.0 == field)
                .map_or(current, |planned| planned.1),
        )
    }

    /// The row's stored value for one numeric field.
    pub(super) fn current(&self, map: &Map, index: usize, field: &str) -> Option<i32> {
        (self.plan.get_num)(map, index, field)
    }

    /// The refusal for a row that vanished between the bounds check and
    /// the read: an engine bug, never a user error.
    pub(super) fn vanished(&self, field: &str) -> CyberError {
        CyberError::Internal(format!(
            "bulk edits: the {} field '{field}' disappeared under the selection",
            self.plan.thing
        ))
    }
}
