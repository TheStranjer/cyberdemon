//! The read half: [`Fields`], one `UDMF` block's fields under modelled
//! reading, and the five element readers that fold a block's values into
//! the model's rows.
//!
//! Each reader takes the [`Fields`] of one block beside the `at` label
//! naming it in errors, pulls out the modelled keys — marking every one
//! taken so it does not also land in `extra` — and closes with
//! [`Fields::finish`], which renders what the model did not take back to
//! `UDMF` source text. A modelled number written with a fraction keeps
//! that text under its own key. Flag booleans decode through the family
//! tables named by [`FlagKind`] and [`FlagFamily`], the `Hexen`
//! activation field through [`ACTIVATIONS`] and [`MONSTER_PAIRS`], and
//! the `Doom` thing presence through [`DOOM_THING_BITS`] and
//! [`DOOM_THING_INVERSES`]. The shape readings and the `Format` builders
//! — [`whole`], [`as_index`], [`missing`], [`not_number`] and
//! [`not_text`] — are this half's own; [`is_decimal`], [`rounds_to`] and
//! [`too_big`] are shared with the write half.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::wad::map::flags::{
    self, ACTIVATION_SHIFT, FlagFamily, FlagKind, MONSTER_ACTIVATE, is_activation_name,
};
use crate::wad::map::model::{Linedef, Sector, Sidedef, Thing, Vertex};
use crate::wad::udmf::{UdmfValue, decimal_to_rounded_int, has_fraction};

use super::{
    ACTIVATIONS, DEFAULT_LIGHT, DOOM_THING_BITS, DOOM_THING_INVERSES, MONSTER_PAIRS, NO_REFERENCE,
    NO_TEXTURE,
};

/// One `UDMF` block's fields under modelled reading: values by name,
/// the keys taken into the model and the `extra` texts built along the
/// way.
pub(super) struct Fields<'doc> {
    /// The block's fields by lowercased name; a repeated name keeps its
    /// last value.
    values: BTreeMap<&'doc str, &'doc UdmfValue>,
    /// Keys this reader consumed into model columns.
    taken: BTreeSet<&'doc str>,
    /// Written texts already recorded for the result's `extra`.
    extra: BTreeMap<String, String>,
}

impl<'doc> Fields<'doc> {
    /// A reader over one block's fields.
    pub(super) fn new(fields: &'doc [(String, UdmfValue)]) -> Self {
        Self {
            values: fields
                .iter()
                .map(|entry| (entry.0.as_str(), &entry.1))
                .collect(),
            taken: BTreeSet::new(),
            extra: BTreeMap::new(),
        }
    }

    /// Records the written text of a modelled field's fraction.
    fn fraction_text(&mut self, key: &str, text: &str) {
        if has_fraction(text) {
            self.extra.insert(key.to_owned(), text.to_owned());
        }
    }

    /// Marks a key consumed by the model.
    fn take(&mut self, key: &'doc str) {
        self.taken.insert(key);
    }

    /// The integer a numeric field holds, or `None` when the field is
    /// absent or holds a value that is no number (which then stays for
    /// `extra`). A fraction keeps its written text there too.
    fn number_value(&mut self, key: &'static str, at: &str) -> Result<Option<i32>, CyberError> {
        let Some(value) = self.values.get(key).map(|found| (*found).clone()) else {
            return Ok(None);
        };
        match value {
            UdmfValue::Int(number) => {
                self.take(key);
                whole(number, key, at).map(Some)
            }
            UdmfValue::Float(text) => {
                self.take(key);
                let rounded = decimal_to_rounded_int(&text)?;
                self.fraction_text(key, &text);
                whole(rounded, key, at).map(Some)
            }
            UdmfValue::Bool(_) | UdmfValue::Keyword(_) | UdmfValue::Str(_) => Ok(None),
        }
    }

    /// The modelled integer of a numeric field, `default` when absent
    /// or unmodelled.
    fn number(&mut self, key: &'static str, default: i32, at: &str) -> Result<i32, CyberError> {
        Ok(self.number_value(key, at)?.unwrap_or(default))
    }

    /// The modelled integer of a required numeric field.
    fn required_number(&mut self, key: &'static str, at: &str) -> Result<i32, CyberError> {
        if !self.values.contains_key(key) {
            return Err(missing(at, key));
        }
        self.number_value(key, at)?
            .map_or_else(|| Err(not_number(at, key)), Ok)
    }

    /// A required reference index into one of the model's element
    /// columns.
    fn index(&mut self, key: &'static str, at: &str) -> Result<usize, CyberError> {
        let value = self.required_number(key, at)?;
        as_index(value, key, at)
    }

    /// An optional reference index; a negative number means "absent".
    fn opt_index(&mut self, key: &'static str, at: &str) -> Result<Option<usize>, CyberError> {
        let value = self.number(key, NO_REFERENCE, at)?;
        if value < 0_i32 {
            return Ok(None);
        }
        as_index(value, key, at).map(Some)
    }

    /// The modelled text of a string field, uppercased to the model's
    /// texture-name convention; `default` when absent or unmodelled.
    fn text(&mut self, key: &'static str, default: &str) -> String {
        self.text_value(key).unwrap_or_else(|| default.to_owned())
    }

    /// The modelled text of a required string field.
    fn required_text(&mut self, key: &'static str, at: &str) -> Result<String, CyberError> {
        if !self.values.contains_key(key) {
            return Err(missing(at, key));
        }
        self.text_value(key)
            .map_or_else(|| Err(not_text(at, key)), Ok)
    }

    /// The uppercased contents of a string field, `None` for an absent
    /// or non-string value.
    fn text_value(&mut self, key: &'static str) -> Option<String> {
        let content = self.values.get(key).and_then(|value| value.as_str())?;
        self.take(key);
        Some(content.to_ascii_uppercase())
    }

    /// Whether a flag boolean reads true; any boolean value is consumed.
    fn boolean(&mut self, key: &'static str) -> bool {
        let Some(value) = self.values.get(key).map(|found| (*found).clone()) else {
            return false;
        };
        match value {
            UdmfValue::Bool(yes) => {
                self.take(key);
                yes
            }
            UdmfValue::Float(_) | UdmfValue::Int(_) | UdmfValue::Keyword(_) | UdmfValue::Str(_) => {
                false
            }
        }
    }

    /// The element's `extra`: the recorded fraction texts plus the
    /// written text of every field the model did not take.
    fn finish(&mut self) -> BTreeMap<String, String> {
        let mut extra = core::mem::take(&mut self.extra);
        let taken = core::mem::take(&mut self.taken);
        extra.extend(
            self.values
                .iter()
                .filter(|entry| !taken.contains(*entry.0))
                .map(|entry| ((*entry.0).to_owned(), entry.1.to_source())),
        );
        extra
    }
}

/// Reads a `vertex { x, y }` block.
pub(super) fn read_vertex(fields: &mut Fields<'_>, at: &str) -> Result<Vertex, CyberError> {
    let x = fields.required_number("x", at)?;
    let y = fields.required_number("y", at)?;
    Ok(Vertex {
        x,
        y,
        extra: fields.finish(),
    })
}

/// Reads a `sector` block: heights, flats, light, special and tag.
pub(super) fn read_sector(fields: &mut Fields<'_>, at: &str) -> Result<Sector, CyberError> {
    let floor_height = fields.number("heightfloor", 0_i32, at)?;
    let ceiling_height = fields.number("heightceiling", 0_i32, at)?;
    let floor_flat = fields.required_text("texturefloor", at)?;
    let ceiling_flat = fields.required_text("textureceiling", at)?;
    let light = fields.number("lightlevel", DEFAULT_LIGHT, at)?;
    let special = fields.number("special", 0_i32, at)?;
    let tag = fields.number("id", 0_i32, at)?;
    Ok(Sector {
        floor_height,
        ceiling_height,
        floor_flat,
        ceiling_flat,
        light,
        special,
        tag,
        extra: fields.finish(),
    })
}

/// Reads a `sidedef` block: offsets, the three textures and the sector.
pub(super) fn read_sidedef(fields: &mut Fields<'_>, at: &str) -> Result<Sidedef, CyberError> {
    let x_offset = fields.number("offsetx", 0_i32, at)?;
    let y_offset = fields.number("offsety", 0_i32, at)?;
    let upper = fields.text("texturetop", NO_TEXTURE);
    let lower = fields.text("texturebottom", NO_TEXTURE);
    let middle = fields.text("texturemiddle", NO_TEXTURE);
    let sector = fields.index("sector", at)?;
    Ok(Sidedef {
        x_offset,
        y_offset,
        upper,
        lower,
        middle,
        sector,
        extra: fields.finish(),
    })
}

/// Reads a `linedef` block; the `Doom` family takes its tag from
/// `arg0`, the `Hexen` family from the line `id`.
pub(super) fn read_linedef(
    fields: &mut Fields<'_>,
    at: &str,
    family: FlagFamily,
) -> Result<Linedef, CyberError> {
    let v1 = fields.index("v1", at)?;
    let v2 = fields.index("v2", at)?;
    let front = Some(fields.index("sidefront", at)?);
    let back = fields.opt_index("sideback", at)?;
    let special = fields.number("special", 0_i32, at)?;
    let args = read_args(fields, at)?;
    let id = fields.number("id", NO_REFERENCE, at)?;
    let flags = linedef_flags(fields, family);
    let tag = match family {
        FlagFamily::Doom => args.first().copied().unwrap_or(0_i32),
        FlagFamily::Hexen if id < 0_i32 => 0_i32,
        FlagFamily::Hexen => id,
    };
    Ok(Linedef {
        v1,
        v2,
        flags,
        special,
        tag,
        args,
        front,
        back,
        extra: fields.finish(),
    })
}

/// Reads a `thing` block: `id` becomes the model's `tid` and `height`
/// its `z`.
pub(super) fn read_thing(
    fields: &mut Fields<'_>,
    at: &str,
    family: FlagFamily,
) -> Result<Thing, CyberError> {
    let tid = fields.number("id", 0_i32, at)?;
    let x = fields.required_number("x", at)?;
    let y = fields.required_number("y", at)?;
    let z = fields.number("height", 0_i32, at)?;
    let angle = fields.number("angle", 0_i32, at)?;
    let kind = fields.required_number("type", at)?;
    let special = fields.number("special", 0_i32, at)?;
    let args = read_args(fields, at)?;
    let flags = thing_flags(fields, family);
    Ok(Thing {
        tid,
        x,
        y,
        z,
        angle,
        kind,
        flags,
        special,
        args,
        extra: fields.finish(),
    })
}

/// Reads the five script arguments of a `linedef` or `thing` block.
fn read_args(fields: &mut Fields<'_>, at: &str) -> Result<[i32; 5], CyberError> {
    Ok([
        fields.number("arg0", 0_i32, at)?,
        fields.number("arg1", 0_i32, at)?,
        fields.number("arg2", 0_i32, at)?,
        fields.number("arg3", 0_i32, at)?,
        fields.number("arg4", 0_i32, at)?,
    ])
}

/// The `flags` of a `linedef` block: the family's bit booleans plus,
/// for the `Hexen` family, the activation field.
fn linedef_flags(fields: &mut Fields<'_>, family: FlagFamily) -> u32 {
    let mut bits = bit_flags(fields, FlagKind::Linedef, family);
    if family == FlagFamily::Hexen {
        bits |= activation_bits(fields);
    }
    bits
}

/// The `flags` of a `thing` block, read through the family's spelling.
fn thing_flags(fields: &mut Fields<'_>, family: FlagFamily) -> u32 {
    match family {
        FlagFamily::Doom => doom_thing_flags(fields),
        FlagFamily::Hexen => bit_flags(fields, FlagKind::Thing, FlagFamily::Hexen),
    }
}

/// The bits a block's plain boolean flag names carry; activation names
/// hold a field value, not a bit, and are read by [`activation_bits`].
fn bit_flags(fields: &mut Fields<'_>, kind: FlagKind, family: FlagFamily) -> u32 {
    let mut bits = 0_u32;
    for entry in flags::names(kind, family) {
        if is_activation_name(entry.0) {
            continue;
        }
        if fields.boolean(entry.0) {
            bits |= entry.1;
        }
    }
    bits
}

/// The activation field bits of a `Hexen` family `linedef` block: the
/// first true [`ACTIVATIONS`] name wins, and both halves of a
/// [`MONSTER_PAIRS`] entry together add the `monsteractivate` bit.
fn activation_bits(fields: &mut Fields<'_>) -> u32 {
    let mut bits = 0_u32;
    let mut chosen = false;
    for (name, value, with_monster) in ACTIVATIONS {
        if !fields.boolean(name) || chosen {
            continue;
        }
        chosen = true;
        bits = value.checked_shl(ACTIVATION_SHIFT).unwrap_or(0_u32);
        if with_monster {
            bits |= MONSTER_ACTIVATE;
        }
    }
    if monster_pairs(fields) {
        bits |= MONSTER_ACTIVATE;
    }
    bits
}

/// Whether both halves of any [`MONSTER_PAIRS`] entry are true.
fn monster_pairs(fields: &mut Fields<'_>) -> bool {
    let mut together = false;
    for (player, monster) in MONSTER_PAIRS {
        if fields.boolean(player) && fields.boolean(monster) {
            together = true;
        }
    }
    together
}

/// The `Doom` family thing flags from the skill and presence booleans:
/// a bit's names are all consumed and the bit holds when any is true;
/// `notsingle`, `notdm` and `notcoop` hold while `single`, `dm` and
/// `coop` is not true.
fn doom_thing_flags(fields: &mut Fields<'_>) -> u32 {
    let mut bits = 0_u32;
    for (bit, names) in DOOM_THING_BITS {
        let mut hot = false;
        for &name in names {
            hot |= fields.boolean(name);
        }
        if hot {
            bits |= bit;
        }
    }
    for (bit, name) in DOOM_THING_INVERSES {
        if !fields.boolean(name) {
            bits |= bit;
        }
    }
    bits
}

/// The `i32` a numeric field value fits, or a `Format` error.
fn whole(value: i64, key: &str, at: &str) -> Result<i32, CyberError> {
    i32::try_from(value).map_err(|_error| {
        CyberError::Format(format!(
            "{at} field '{key}' holds {value}, which is outside the stored i32 range"
        ))
    })
}

/// Whether `text` denotes a decimal number at all.
pub(super) fn is_decimal(text: &str) -> bool {
    decimal_to_rounded_int(text).is_ok()
}

/// Whether preserved text still denotes `value`.
pub(super) fn rounds_to(text: &str, value: i32) -> bool {
    let Ok(rounded) = decimal_to_rounded_int(text) else {
        return false;
    };
    i32::try_from(rounded).is_ok_and(|whole| whole == value)
}

/// The model index a non-negative field value names.
fn as_index(value: i32, key: &str, at: &str) -> Result<usize, CyberError> {
    usize::try_from(value).map_err(|_error| {
        CyberError::Format(format!(
            "{at} field '{key}' holds {value}, which is not a non-negative index"
        ))
    })
}

/// The `Format` saying block `at` misses the required field `key`.
fn missing(at: &str, key: &str) -> CyberError {
    CyberError::Format(format!("{at} lacks the required field '{key}'"))
}

/// The `Format` saying field `key` of block `at` holds no number.
fn not_number(at: &str, key: &str) -> CyberError {
    CyberError::Format(format!(
        "{at} field '{key}' is no number: give it an integer or decimal value"
    ))
}

/// The `Format` saying field `key` of block `at` holds no string.
fn not_text(at: &str, key: &str) -> CyberError {
    CyberError::Format(format!("{at} field '{key}' is no quoted string"))
}

/// The `LimitExceeded` saying an index of element `at` field `key`
/// leaves the `i32` range.
pub(super) fn too_big(at: &str, key: &str, value: usize) -> CyberError {
    CyberError::LimitExceeded(format!(
        "{at} field '{key}' index {value} does not fit into an i32"
    ))
}
