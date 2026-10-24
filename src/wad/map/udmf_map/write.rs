//! The write half: [`Out`], one `UDMF` block being written, and the five
//! element writers that render the model's rows back into blocks.
//!
//! Each writer opens an [`Out`] over the element's `extra` and puts its
//! fields in emit order. A modelled field whose `extra` still holds a
//! text denoting the same value — see [`rounds_to`] — is written as that
//! text, so an untouched fraction survives verbatim; the rest go out as
//! integers, as `"<int>.000"` floats for the coordinate fields, or not at
//! all when they equal the `UDMF` default. [`Out::finish`] closes the
//! block with the `extra` texts the model never consumed. Flag booleans
//! come from [`to_names`] over the family's table, the `Doom` thing
//! presence from [`doom_thing_booleans`], and every reference index
//! passes through [`store_index`].

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::wad::map::flags::{FlagFamily, FlagKind, to_names};
use crate::wad::map::model::{Linedef, Sector, Sidedef, Thing, Vertex};
use crate::wad::udmf::{UdmfBlock, UdmfValue};

use super::read::{is_decimal, rounds_to, too_big};
use super::{DEFAULT_LIGHT, NO_REFERENCE, NO_TEXTURE};

/// A block being written: its fields in emit order and the element's
/// side-bag of texts, each consumed as it is emitted.
struct Out {
    /// The fields gathered so far.
    fields: Vec<(String, UdmfValue)>,
    /// The element's `extra`, with modelled keys consulted first.
    extra: BTreeMap<String, String>,
}

impl Out {
    /// A writer over one element's `extra` map.
    fn new(extra: &BTreeMap<String, String>) -> Self {
        Self {
            fields: Vec::new(),
            extra: extra.clone(),
        }
    }

    /// Appends one field.
    fn put(&mut self, key: &str, value: UdmfValue) {
        self.fields.push((key.to_owned(), value));
    }

    /// Emits a modelled field's preserved text when its `extra` holds
    /// one: any text that is no decimal (a value the reader left
    /// unmodelled) or a number that still rounds to `value`; reports
    /// whether the field is complete.
    fn preserved(&mut self, key: &str, value: i32) -> bool {
        let Some(text) = self.extra.remove(key) else {
            return false;
        };
        let keep = !is_decimal(&text) || rounds_to(&text, value);
        if keep {
            self.put(key, UdmfValue::Keyword(text));
        }
        keep
    }

    /// One optional numeric field: preserved text, else the integer
    /// only when it differs from `default`.
    fn number(&mut self, key: &str, value: i32, default: i32) {
        if self.preserved(key, value) || value == default {
            return;
        }
        self.put(key, UdmfValue::Int(i64::from(value)));
    }

    /// One required numeric field: preserved text, else the integer.
    fn required_number(&mut self, key: &str, value: i32) {
        if self.preserved(key, value) {
            return;
        }
        self.put(key, UdmfValue::Int(i64::from(value)));
    }

    /// One optional numeric field written as a `"<int>.000"` float when
    /// it is no default.
    fn float(&mut self, key: &str, value: i32, default: i32) {
        if self.preserved(key, value) || value == default {
            return;
        }
        self.put(key, UdmfValue::Float(format!("{value}.000")));
    }

    /// One required numeric field written as a `"<int>.000"` float.
    fn required_float(&mut self, key: &str, value: i32) {
        if self.preserved(key, value) {
            return;
        }
        self.put(key, UdmfValue::Float(format!("{value}.000")));
    }

    /// One optional texture name: preserved text, else the quoted name
    /// only when it differs from `default`.
    fn text(&mut self, key: &str, value: &str, default: &str) {
        if let Some(original) = self.extra.remove(key) {
            self.put(key, UdmfValue::Keyword(original));
            return;
        }
        if value != default {
            self.put(key, UdmfValue::Str(value.to_owned()));
        }
    }

    /// One required texture name: preserved text, else the quoted name.
    fn required_text(&mut self, key: &str, value: &str) {
        if let Some(original) = self.extra.remove(key) {
            self.put(key, UdmfValue::Keyword(original));
            return;
        }
        self.put(key, UdmfValue::Str(value.to_owned()));
    }

    /// One boolean, emitted only when true.
    fn boolean(&mut self, key: &str, value: bool) {
        if value {
            self.put(key, UdmfValue::Bool(true));
        }
    }

    /// The flag booleans `bits` carries; `playercross` names the
    /// all-clear field and is never written (an absent activation is
    /// `playercross`).
    fn flags(&mut self, bits: u32, kind: FlagKind, family: FlagFamily) {
        for name in to_names(bits, kind, family) {
            self.flag_boolean(name);
        }
    }

    /// One flag boolean, excepting the name of the all-clear activation
    /// field, which stays implicit.
    fn flag_boolean(&mut self, name: &str) {
        if name != "playercross" {
            self.put(name, UdmfValue::Bool(true));
        }
    }

    /// Closes the block: the `extra` entries the model did not consume
    /// are emitted verbatim, in key order.
    fn finish(mut self, kind: &str) -> UdmfBlock {
        for (key, text) in core::mem::take(&mut self.extra) {
            self.fields.push((key, UdmfValue::Keyword(text)));
        }
        UdmfBlock {
            kind: kind.to_owned(),
            fields: self.fields,
        }
    }
}

/// Writes a [`Vertex`] as its `vertex` block.
pub(super) fn write_vertex(vertex: &Vertex) -> UdmfBlock {
    let mut out = Out::new(&vertex.extra);
    out.required_float("x", vertex.x);
    out.required_float("y", vertex.y);
    out.finish("vertex")
}

/// Writes a [`Sector`] as its `sector` block.
pub(super) fn write_sector(sector: &Sector) -> UdmfBlock {
    let mut out = Out::new(&sector.extra);
    out.number("heightfloor", sector.floor_height, 0_i32);
    out.number("heightceiling", sector.ceiling_height, 0_i32);
    out.required_text("texturefloor", &sector.floor_flat);
    out.required_text("textureceiling", &sector.ceiling_flat);
    out.number("lightlevel", sector.light, DEFAULT_LIGHT);
    out.number("special", sector.special, 0_i32);
    out.number("id", sector.tag, 0_i32);
    out.finish("sector")
}

/// Writes a [`Sidedef`] as its `sidedef` block.
pub(super) fn write_sidedef(sidedef: &Sidedef, at: &str) -> Result<UdmfBlock, CyberError> {
    let mut out = Out::new(&sidedef.extra);
    out.number("offsetx", sidedef.x_offset, 0_i32);
    out.number("offsety", sidedef.y_offset, 0_i32);
    out.text("texturetop", &sidedef.upper, NO_TEXTURE);
    out.text("texturebottom", &sidedef.lower, NO_TEXTURE);
    out.text("texturemiddle", &sidedef.middle, NO_TEXTURE);
    out.required_number("sector", store_index(sidedef.sector, "sector", at)?);
    Ok(out.finish("sidedef"))
}

/// Writes a [`Linedef`] as its `linedef` block; the `Doom` family
/// carries its tag in both `id` and `arg0`, the `Hexen` family in `id`.
/// A `Doom` family line writes no `arg0` of its own: the tag owns it.
pub(super) fn write_linedef(
    linedef: &Linedef,
    at: &str,
    family: FlagFamily,
) -> Result<UdmfBlock, CyberError> {
    let mut out = Out::new(&linedef.extra);
    out.required_number("v1", store_index(linedef.v1, "v1", at)?);
    out.required_number("v2", store_index(linedef.v2, "v2", at)?);
    let front = linedef
        .front
        .map_or(Ok(NO_REFERENCE), |side| store_index(side, "sidefront", at))?;
    out.required_number("sidefront", front);
    if let Some(back) = linedef.back {
        out.number("sideback", store_index(back, "sideback", at)?, NO_REFERENCE);
    }
    out.number("special", linedef.special, 0_i32);
    for (position, value) in linedef.args.iter().enumerate() {
        if family == FlagFamily::Doom && position == 0 {
            continue;
        }
        out.number(&format!("arg{position}"), *value, 0_i32);
    }
    if linedef.tag != 0_i32 {
        out.number("id", linedef.tag, 0_i32);
        if family == FlagFamily::Doom {
            out.required_number("arg0", linedef.tag);
        }
    }
    out.flags(linedef.flags, FlagKind::Linedef, family);
    Ok(out.finish("linedef"))
}

/// Writes a [`Thing`] as its `thing` block.
pub(super) fn write_thing(spawn: &Thing, family: FlagFamily) -> UdmfBlock {
    let mut out = Out::new(&spawn.extra);
    out.number("id", spawn.tid, 0_i32);
    out.required_float("x", spawn.x);
    out.required_float("y", spawn.y);
    out.float("height", spawn.z, 0_i32);
    out.number("angle", spawn.angle, 0_i32);
    out.required_number("type", spawn.kind);
    out.number("special", spawn.special, 0_i32);
    for (position, value) in spawn.args.iter().enumerate() {
        out.number(&format!("arg{position}"), *value, 0_i32);
    }
    match family {
        FlagFamily::Doom => doom_thing_booleans(&mut out, spawn.flags),
        FlagFamily::Hexen => out.flags(spawn.flags, FlagKind::Thing, FlagFamily::Hexen),
    }
    out.finish("thing")
}

/// The index a field can store.
fn store_index(value: usize, key: &str, at: &str) -> Result<i32, CyberError> {
    i32::try_from(value).map_err(|_error| too_big(at, key, value))
}

/// The `Doom` family thing booleans for `bits`: skill names carry the
/// presence bits, and `single`, `dm` and `coop` are written true exactly
/// when their inverse bit is clear.
fn doom_thing_booleans(out: &mut Out, bits: u32) {
    let easy = bits & 1_u32 != 0_u32;
    let hard = bits & 4_u32 != 0_u32;
    out.boolean("skill1", easy);
    out.boolean("skill2", easy);
    out.boolean("skill3", bits & 2_u32 != 0_u32);
    out.boolean("skill4", hard);
    out.boolean("skill5", hard);
    out.boolean("ambush", bits & 8_u32 != 0_u32);
    out.boolean("single", bits & 16_u32 == 0_u32);
    out.boolean("dm", bits & 32_u32 == 0_u32);
    out.boolean("coop", bits & 64_u32 == 0_u32);
    out.boolean("friend", bits & 128_u32 != 0_u32);
}
