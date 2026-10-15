//! `UDMF` text maps: the [`Map`] model read from and written to a
//! `TEXTMAP`/`ENDMAP` block.
//!
//! [`read_udmf_map`] decodes the `TEXTMAP` lump of a block (whose grammar
//! lives in [`crate::wad::udmf`]) into the map model: the five element
//! kinds map onto the model's rows, the `namespace` global is required
//! and kept in [`Map::udmf_namespace`], and every other member of the
//! block is carried verbatim in [`Map::other`] (`ENDMAP` itself is
//! regenerated on write). Fields the model does not name — unknown ones
//! like `comment` or `alpha`, unmodelled ones like `lightfloor`, and
//! modelled keys written in a shape this reader does not consume, such
//! as string `arg` values — land in the element's `extra` as rendered
//! `UDMF` source text, so the writer can emit them verbatim. A modelled
//! numeric field that holds a fraction keeps the rounded integer in the
//! model and its written text in `extra` under its own key;
//! [`write_udmf_map`] emits that text again while the model value still
//! rounds from it, and drops it once the element moved.
//!
//! Flag columns decode through the family tables in [`super::flags`]:
//! `Doom` namespace `TEXTMAP`s spell thing presence as `skill1`..`skill5`
//! plus the inverted `single`/`dm`/`coop`, `Hexen` family linedefs carry
//! the trigger booleans in the 3-bit activation field (the first true
//! name in table order wins; a monster name true together with its
//! player counterpart also sets `monsteractivate`), and every other
//! flag is a plain boolean bit. Blocks of kinds other than the five
//! elements and global assignments other than `namespace` have no
//! modelled home and are dropped.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::classify::MapFormat;
use crate::wad::name::LumpName;
use crate::wad::udmf::{
    UdmfBlock, UdmfDoc, UdmfValue, decimal_to_rounded_int, has_fraction, parse_udmf, write_udmf,
};

use super::flags::{
    self, ACTIVATION_SHIFT, FlagFamily, FlagKind, MONSTER_ACTIVATE, is_activation_name, to_names,
};
use super::model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};

/// The namespace [`write_udmf_map`] emits for a map that carries none.
const DEFAULT_NAMESPACE: &str = "zdoom";
/// The name of the global that declares a `TEXTMAP`'s namespace.
const NAMESPACE: &str = "namespace";
/// The `lightlevel` of a sector that names none.
const DEFAULT_LIGHT: i32 = 160_i32;
/// The `sideback` and `id` number that means "absent".
const NO_REFERENCE: i32 = -1_i32;
/// The texture and flat name that means "none".
const NO_TEXTURE: &str = "-";

/// `UDMF` linedef trigger booleans of the `Hexen` family: the name, the
/// activation field value it stands for, and whether the name also
/// implies the [`MONSTER_ACTIVATE`] bit. In table order: the first true
/// name picks the field.
const ACTIVATIONS: [(&str, u32, bool); 8] = [
    ("playercross", 0_u32, false),
    ("playeruse", 1_u32, false),
    ("monstercross", 2_u32, false),
    ("impact", 3_u32, false),
    ("playerpush", 4_u32, false),
    ("missilecross", 5_u32, false),
    ("monsteruse", 1_u32, true),
    ("monsterpush", 4_u32, true),
];

/// Pairs of player and monster trigger booleans that, both true, also
/// set the [`MONSTER_ACTIVATE`] bit.
const MONSTER_PAIRS: [(&str, &str); 3] = [
    ("playercross", "monstercross"),
    ("playeruse", "monsteruse"),
    ("playerpush", "monsterpush"),
];

/// `UDMF` thing booleans of the `Doom` family: the flag bit and the
/// names that, any of them true, set it.
const DOOM_THING_BITS: [(u32, &[&str]); 5] = [
    (1_u32, &["skill1", "skill2"]),
    (2_u32, &["skill3"]),
    (4_u32, &["skill4", "skill5"]),
    (8_u32, &["ambush"]),
    (128_u32, &["friend"]),
];

/// `Doom` family thing presence bits: the flag bit and the boolean name
/// that, not true, sets it.
const DOOM_THING_INVERSES: [(u32, &str); 3] =
    [(16_u32, "single"), (32_u32, "dm"), (64_u32, "coop")];

/// Reads the `UDMF` map named `name` out of a block's `members` (the
/// block's lumps after its header, `TEXTMAP` through `ENDMAP`).
///
/// The `TEXTMAP` parses through [`crate::wad::udmf`], the `namespace`
/// global is required, and every other member but `ENDMAP` is carried
/// into [`Map::other`]; blocks of kinds other than the five model
/// elements are ignored.
///
/// # Errors
///
/// [`CyberError::Format`] when the block holds no `TEXTMAP` or its bytes
/// are no `UTF-8`, when the grammar rejects the text, when no
/// `namespace` is declared, or when a block misses a required field or
/// carries a value its shape excludes; the message names the block kind
/// and index.
#[inline]
pub fn read_udmf_map(name: LumpName, members: &[Lump]) -> Result<Map, CyberError> {
    let source = textmap_source(members)?;
    let doc = parse_udmf(&source)?;
    let mut map = Map::new_empty(name, MapFormat::Udmf);
    map.udmf_namespace = Some(namespace_of(&doc)?);
    let family = map.flag_family();
    collect_blocks(&doc, family, &mut map)?;
    map.other = carried_members(members);
    Ok(map)
}

/// Writes `map` as its block's lumps: the zero-size header, the
/// `TEXTMAP` rendering of the model, every [`Map::other`] lump in order
/// and a fresh `ENDMAP` marker.
///
/// Required fields are always written, other fields only when they
/// differ from the `UDMF` default, booleans only when true and `extra`
/// texts verbatim; a modelled field whose `extra` text still rounds to
/// the model value is written as that text, and coordinate fields
/// written as plain numbers use the `"<int>.000"` float form.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a reference index leaves the
/// `i32` range.
#[inline]
pub fn write_udmf_map(map: &Map) -> Result<Vec<Lump>, CyberError> {
    let text = write_udmf(&document(map)?).into_bytes();
    let mut lumps = Vec::with_capacity(map.other.len().saturating_add(3_usize));
    lumps.push(Lump::marker(map.name));
    lumps.push(Lump::new(LumpName::from_raw(*b"TEXTMAP\0"), text));
    lumps.extend(map.other.iter().cloned());
    lumps.push(Lump::marker(LumpName::from_raw(*b"ENDMAP\0\0")));
    Ok(lumps)
}

/// The `TEXTMAP` payload as text, or a `Format` error when the block
/// holds no `TEXTMAP` or its bytes are no `UTF-8`.
fn textmap_source(members: &[Lump]) -> Result<String, CyberError> {
    let lump = members
        .iter()
        .find(|lump| lump.name.as_bytes() == b"TEXTMAP")
        .ok_or_else(|| {
            CyberError::Format(
                "the UDMF map block holds no TEXTMAP lump: a TEXTMAP map block runs \
                 from its TEXTMAP member to its ENDMAP member"
                    .to_owned(),
            )
        })?;
    core::str::from_utf8(&lump.data)
        .map(str::to_owned)
        .map_err(|_error| {
            CyberError::Format(
                "the TEXTMAP lump is no valid UTF-8: re-save the map as plain text".to_owned(),
            )
        })
}

/// The block members that are neither `TEXTMAP` nor `ENDMAP`, in
/// original order.
fn carried_members(members: &[Lump]) -> Vec<Lump> {
    members
        .iter()
        .filter(|lump| !matches!(lump.name.as_bytes(), b"TEXTMAP" | b"ENDMAP"))
        .cloned()
        .collect()
}

/// The declared namespace, or a `Format` error when the document holds
/// no `namespace` global or one of a non-string shape.
fn namespace_of(doc: &UdmfDoc) -> Result<String, CyberError> {
    let found = doc
        .globals
        .iter()
        .find(|entry| entry.0 == NAMESPACE)
        .map(|entry| entry.1.clone());
    match found {
        Some(UdmfValue::Str(namespace)) => Ok(namespace),
        Some(
            UdmfValue::Bool(_) | UdmfValue::Float(_) | UdmfValue::Int(_) | UdmfValue::Keyword(_),
        ) => Err(CyberError::Format(
            "the TEXTMAP namespace must be a quoted string: write `namespace = \"zdoom\";`"
                .to_owned(),
        )),
        None => Err(CyberError::Format(
            "the TEXTMAP declares no namespace: add a global like \
             `namespace = \"zdoom\";`"
                .to_owned(),
        )),
    }
}

/// Appends every element block of `doc` to its kind in `map`, in
/// document order; blocks of other kinds are ignored.
fn collect_blocks(doc: &UdmfDoc, family: FlagFamily, map: &mut Map) -> Result<(), CyberError> {
    for block in &doc.blocks {
        let mut fields = Fields::new(&block.fields);
        match block.kind.as_str() {
            "thing" => {
                let at = element("thing", map.things.len());
                map.things.push(read_thing(&mut fields, &at, family)?);
            }
            "vertex" => {
                let at = element("vertex", map.vertices.len());
                map.vertices.push(read_vertex(&mut fields, &at)?);
            }
            "linedef" => {
                let at = element("linedef", map.linedefs.len());
                map.linedefs.push(read_linedef(&mut fields, &at, family)?);
            }
            "sidedef" => {
                let at = element("sidedef", map.sidedefs.len());
                map.sidedefs.push(read_sidedef(&mut fields, &at)?);
            }
            "sector" => {
                let at = element("sector", map.sectors.len());
                map.sectors.push(read_sector(&mut fields, &at)?);
            }
            _ => {}
        }
    }
    Ok(())
}

/// The block label that names one element in read errors.
fn element(kind: &str, index: usize) -> String {
    format!("TEXTMAP {kind} {index}")
}

/// The element blocks of `map`, `sector` first, under its namespace
/// global.
fn document(map: &Map) -> Result<UdmfDoc, CyberError> {
    let family = map.flag_family();
    let namespace = map
        .udmf_namespace
        .clone()
        .unwrap_or_else(|| DEFAULT_NAMESPACE.to_owned());
    let mut blocks = Vec::new();
    for sector in &map.sectors {
        blocks.push(write_sector(sector));
    }
    for (index, sidedef) in map.sidedefs.iter().enumerate() {
        let at = element("sidedef", index);
        blocks.push(write_sidedef(sidedef, &at)?);
    }
    for vertex in &map.vertices {
        blocks.push(write_vertex(vertex));
    }
    for (index, linedef) in map.linedefs.iter().enumerate() {
        let at = element("linedef", index);
        blocks.push(write_linedef(linedef, &at, family)?);
    }
    for spawn in &map.things {
        blocks.push(write_thing(spawn, family));
    }
    let globals = vec![(NAMESPACE.to_owned(), UdmfValue::Str(namespace))];
    Ok(UdmfDoc { globals, blocks })
}

/// One `UDMF` block's fields under modelled reading: values by name,
/// the keys taken into the model and the `extra` texts built along the
/// way.
struct Fields<'doc> {
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
    fn new(fields: &'doc [(String, UdmfValue)]) -> Self {
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
fn read_vertex(fields: &mut Fields<'_>, at: &str) -> Result<Vertex, CyberError> {
    let x = fields.required_number("x", at)?;
    let y = fields.required_number("y", at)?;
    Ok(Vertex {
        x,
        y,
        extra: fields.finish(),
    })
}

/// Reads a `sector` block: heights, flats, light, special and tag.
fn read_sector(fields: &mut Fields<'_>, at: &str) -> Result<Sector, CyberError> {
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
fn read_sidedef(fields: &mut Fields<'_>, at: &str) -> Result<Sidedef, CyberError> {
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
fn read_linedef(
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
fn read_thing(fields: &mut Fields<'_>, at: &str, family: FlagFamily) -> Result<Thing, CyberError> {
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
fn is_decimal(text: &str) -> bool {
    decimal_to_rounded_int(text).is_ok()
}

/// Whether preserved text still denotes `value`.
fn rounds_to(text: &str, value: i32) -> bool {
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
fn too_big(at: &str, key: &str, value: usize) -> CyberError {
    CyberError::LimitExceeded(format!(
        "{at} field '{key}' index {value} does not fit into an i32"
    ))
}

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
fn write_vertex(vertex: &Vertex) -> UdmfBlock {
    let mut out = Out::new(&vertex.extra);
    out.required_float("x", vertex.x);
    out.required_float("y", vertex.y);
    out.finish("vertex")
}

/// Writes a [`Sector`] as its `sector` block.
fn write_sector(sector: &Sector) -> UdmfBlock {
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
fn write_sidedef(sidedef: &Sidedef, at: &str) -> Result<UdmfBlock, CyberError> {
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
fn write_linedef(linedef: &Linedef, at: &str, family: FlagFamily) -> Result<UdmfBlock, CyberError> {
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
fn write_thing(spawn: &Thing, family: FlagFamily) -> UdmfBlock {
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

#[cfg(test)]
mod tests {
    use crate::error::CyberError;
    use crate::wad::archive::Lump;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::Map;
    use crate::wad::map::common::fixtures::{lump, marker, name, two_room_map};
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::map::udmf_map::{read_udmf_map, write_udmf_map};

    /// The model built from one `TEXTMAP` text plus a closing `ENDMAP`.
    fn read_ok(text: &str) -> Map {
        let members = [lump("TEXTMAP", text.as_bytes()), marker("ENDMAP")];
        read_udmf_map(name("MAP01"), &members).expect("the test TEXTMAP decodes")
    }

    /// The `Format` message one `TEXTMAP` text fails with.
    fn error_of(text: &str) -> String {
        let members = [lump("TEXTMAP", text.as_bytes()), marker("ENDMAP")];
        match read_udmf_map(name("MAP01"), &members) {
            Err(CyberError::Format(message)) => message,
            Err(other) => panic!("expected Format, got {other:?}"),
            Ok(map) => panic!("expected a refusal, read {} vertices", map.vertices.len()),
        }
    }

    /// The `TEXTMAP` payload of written lumps, as text.
    fn textmap_of(lumps: &[Lump]) -> String {
        let lump = lumps
            .iter()
            .find(|lump| lump.name.as_bytes() == b"TEXTMAP")
            .expect("the writer emits TEXTMAP");
        String::from_utf8(lump.data.to_vec()).expect("the writer emits UTF-8")
    }

    /// The two-room fixture as a `UDMF` map in `namespace`.
    fn udmf_two_rooms(namespace: &str) -> Map {
        let mut map = two_room_map();
        map.format = MapFormat::Udmf;
        map.udmf_namespace = Some(namespace.to_owned());
        map.other = vec![
            lump("ZNODES", b"gz"),
            lump("BEHAVIOR", b"bh"),
            lump("REJECT", b"rj"),
            lump("BLOCKMAP", b"bm"),
        ];
        map
    }

    /// The decoded `flags` of a `zdoom` linedef carrying `booleans`.
    fn linedef_bits(booleans: &[&str]) -> u32 {
        let flags: Vec<String> = booleans
            .iter()
            .map(|name| format!(" {name} = true;"))
            .collect();
        let text = format!(
            "namespace = \"zdoom\"; linedef {{ v1 = 0; v2 = 0; sidefront = 0;{} }}",
            flags.join("")
        );
        read_ok(&text).linedefs.first().expect("the linedef").flags
    }

    #[test]
    fn the_two_room_map_round_trips_as_udmf_in_both_families() {
        for (namespace, spelling) in [("doom", "skill1 = true;"), ("zdoom", "easy = true;")] {
            let map = udmf_two_rooms(namespace);
            let lumps = write_udmf_map(&map).expect("the fixture is writable");
            assert_eq!(
                lumps.first().map(|lump| lump.name),
                Some(name("MAP01")),
                "the header leads"
            );
            assert_eq!(
                lumps.get(1).map(|lump| lump.name),
                Some(name("TEXTMAP")),
                "then TEXTMAP"
            );
            assert_eq!(
                lumps.last().map(|lump| lump.name),
                Some(name("ENDMAP")),
                "and a fresh ENDMAP closes the block"
            );
            assert_eq!(lumps.len(), 7, "header, TEXTMAP, four others, ENDMAP");
            let text = textmap_of(&lumps);
            assert!(
                text.contains(spelling),
                "the {namespace} family spells thing presence as {spelling}: {text}"
            );
            assert!(
                text.contains("twosided = true;"),
                "the step line keeps its flag"
            );
            assert!(
                text.contains("heightceiling = 128;"),
                "sector heights are written"
            );
            assert!(!text.contains("heightfloor = 0;"), "defaults stay away");
            assert!(
                text.contains("x = 64.000;"),
                "coordinates are written as floats"
            );
            let read = read_udmf_map(map.name, lumps.get(1..).expect("members"))
                .expect("the written block reads back");
            assert_eq!(read, map, "read(write(map)) == map in {namespace}");
        }
    }

    #[test]
    fn a_map_without_a_namespace_writes_as_zdoom() {
        let mut map = udmf_two_rooms("doom");
        map.udmf_namespace = None;
        map.things.clear();
        map.linedefs.clear();
        map.sidedefs.clear();
        map.sectors.clear();
        map.vertices.clear();
        map.other.clear();
        let text = textmap_of(&write_udmf_map(&map).expect("an empty map is writable"));
        assert_eq!(
            text, "namespace = \"zdoom\";\n",
            "the unset namespace falls back and nothing else is emitted"
        );
        let read = read_ok(&text);
        assert_eq!(
            read.udmf_namespace.as_deref(),
            Some("zdoom"),
            "it reads back"
        );
        assert_eq!(
            read.flag_family(),
            FlagFamily::Hexen,
            "and lands in the Hexen family"
        );
    }

    #[test]
    fn the_doom_family_decodes_every_linedef_and_thing_boolean() {
        let map = read_ok(concat!(
            "namespace = \"doom\";",
            "linedef { v1 = 0; v2 = 1; sidefront = 2; sideback = 3; special = 1;",
            " id = 66; arg0 = 66;",
            " blocking = true; blockmonsters = true; twosided = true; dontpegtop = true;",
            " dontpegbottom = true; secret = true; blocksound = true; dontdraw = true;",
            " mapped = true; passuse = true; }",
            "thing { x = 8; y = 16; type = 5; height = 24; angle = 90; id = 7; special = 2;",
            " arg1 = 3; skill1 = true; skill3 = true; skill5 = true; ambush = true;",
            " friend = true; }",
        ));
        let linedef = map.linedefs.first().expect("one linedef");
        assert_eq!(linedef.flags, 0x3FF, "the ten Doom line bits");
        assert_eq!(linedef.tag, 66_i32, "the Doom tag rides on arg0");
        assert_eq!(
            linedef.args.first().copied(),
            Some(66_i32),
            "arg0 keeps the tag"
        );
        assert_eq!(linedef.front, Some(2), "the front side");
        assert_eq!(linedef.back, Some(3), "and the back side");
        assert_eq!(linedef.special, 1_i32, "the special");
        let spawn = map.things.first().expect("one thing");
        assert_eq!(
            spawn.flags, 255_u32,
            "skill1 gives easy, skill3 medium, skill5 hard, ambush and friend set their \
             bits, and absent single, dm and coop invert into notsingle, notdm and notcoop"
        );
        assert_eq!(spawn.tid, 7_i32, "the thing id is the tid");
        assert_eq!(spawn.z, 24_i32, "height is the model z");
        assert_eq!(spawn.args.get(1).copied(), Some(3_i32), "thing args decode");
        let again = read_ok(&textmap_of(&write_udmf_map(&map).expect("writable")));
        assert_eq!(again, map, "the spelled flags survive a write cycle");
    }

    #[test]
    fn the_hexen_family_decodes_every_linedef_and_thing_boolean() {
        let map = read_ok(concat!(
            "namespace = \"zdoom\";",
            "linedef { v1 = 0; v2 = 1; sidefront = 0; blocking = true; blockmonsters = true;",
            " twosided = true; dontpegtop = true; dontpegbottom = true; secret = true;",
            " blocksound = true; dontdraw = true; mapped = true; repeatspecial = true;",
            " playeruse = true; monsteractivate = true; blockplayers = true;",
            " blockeverything = true; }",
            "thing { x = 0; y = 0; type = 1; easy = true; medium = true; hard = true;",
            " ambush = true; dormant = true; class1 = true; class2 = true; class3 = true;",
            " single = true; coop = true; dm = true; translucent = true; invisible = true;",
            " friend = true; standing = true; }",
        ));
        let linedef = map.linedefs.first().expect("one linedef");
        assert_eq!(
            linedef.flags,
            0x1FF_u32 | 0x200 | 0x400 | 0x2000 | 0x4000 | 0x8000,
            "the shared bits, repeatspecial, the playeruse field and the Hexen bits"
        );
        let spawn = map.things.first().expect("one thing");
        assert_eq!(spawn.flags, 0x7FFF, "all fifteen Hexen thing bits");
        let again = read_ok(&textmap_of(&write_udmf_map(&map).expect("writable")));
        assert_eq!(again, map, "the direct spelling survives a write cycle");
    }

    #[test]
    fn the_activation_field_takes_the_first_true_boolean() {
        for (booleans, expected) in [
            (&[][..], 0_u32),
            (&["playercross"][..], 0),
            (&["playeruse"][..], 0x400),
            (&["monstercross"][..], 0x800),
            (&["impact"][..], 0xC00),
            (&["playerpush"][..], 0x1000),
            (&["missilecross"][..], 0x1400),
            (&["monsteruse"][..], 0x2400),
            (&["monsterpush"][..], 0x3000),
            (&["monsterpush", "monsteruse"][..], 0x2400),
            (&["playercross", "impact"][..], 0),
            (&["playeruse", "monstercross"][..], 0x400),
            (&["playercross", "monstercross"][..], 0x2000),
            (&["playeruse", "monsteruse"][..], 0x2400),
            (&["playerpush", "monsterpush"][..], 0x3000),
        ] {
            assert_eq!(
                linedef_bits(booleans),
                expected,
                "activation from {booleans:?}"
            );
        }
    }

    #[test]
    fn a_fractional_vertex_survives_until_it_moves() {
        let map = read_ok("namespace = \"zdoom\"; vertex { x = 64.5; y = -20.25; }");
        let vertex = map.vertices.first().expect("one vertex");
        assert_eq!(vertex.x, 65_i32, "the model keeps the rounded integer");
        assert_eq!(vertex.y, -20_i32, "a fraction rounds half away from zero");
        assert_eq!(vertex.extra.get("x").map(String::as_str), Some("64.5"));
        assert_eq!(vertex.extra.get("y").map(String::as_str), Some("-20.25"));
        let text = textmap_of(&write_udmf_map(&map).expect("writable"));
        assert!(
            text.contains("x = 64.5;") && text.contains("y = -20.25;"),
            "untouched fractions are re-emitted verbatim: {text}"
        );
        assert_eq!(read_ok(&text), map, "and read back identically");
        let mut moved = map.clone();
        let first = moved.vertices.first_mut().expect("one vertex");
        first.x = 70_i32;
        assert!(
            first.extra.remove("x").is_some(),
            "the stale text leaves with the move"
        );
        let moved_text = textmap_of(&write_udmf_map(&moved).expect("writable"));
        assert!(
            moved_text.contains("x = 70.000;") && !moved_text.contains("64.5"),
            "a moved vertex emits the new integer: {moved_text}"
        );
        assert_eq!(read_ok(&moved_text), moved, "the moved vertex round trips");
    }

    #[test]
    fn unmodelled_fields_and_shapes_survive_verbatim() {
        let map = read_ok(concat!(
            "namespace = \"zdoom\";",
            "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\";",
            " comment = \"hello\"; lightfloor = 1.5; scalex_top = 2; skyfloor = Sky1; }",
            "linedef { v1 = 0; v2 = 0; sidefront = 0; arg1 = \"nope\"; alpha = 0.5;",
            " blocking = 7; }",
        ));
        let sector = map.sectors.first().expect("one sector");
        assert_eq!(
            sector.light, 160_i32,
            "an unmodelled lightfloor changes nothing"
        );
        assert_eq!(
            sector.extra.get("comment").map(String::as_str),
            Some("\"hello\"")
        );
        assert_eq!(
            sector.extra.get("lightfloor").map(String::as_str),
            Some("1.5")
        );
        assert_eq!(
            sector.extra.get("scalex_top").map(String::as_str),
            Some("2")
        );
        assert_eq!(
            sector.extra.get("skyfloor").map(String::as_str),
            Some("Sky1")
        );
        let linedef = map.linedefs.first().expect("one linedef");
        assert_eq!(linedef.flags, 0, "a non-boolean blocking sets no bit");
        assert_eq!(
            linedef.args.get(1).copied(),
            Some(0_i32),
            "a string arg stays unmodelled"
        );
        assert_eq!(
            linedef.extra.get("arg1").map(String::as_str),
            Some("\"nope\"")
        );
        assert_eq!(linedef.extra.get("alpha").map(String::as_str), Some("0.5"));
        assert_eq!(linedef.extra.get("blocking").map(String::as_str), Some("7"));
        let text = textmap_of(&write_udmf_map(&map).expect("writable"));
        for wanted in [
            "comment = \"hello\";",
            "lightfloor = 1.5;",
            "scalex_top = 2;",
            "skyfloor = Sky1;",
            "arg1 = \"nope\";",
            "alpha = 0.5;",
            "blocking = 7;",
        ] {
            assert!(text.contains(wanted), "the text keeps `{wanted}`: {text}");
        }
        assert_eq!(read_ok(&text), map, "and reads back identically");
    }

    #[test]
    fn a_missing_namespace_is_refused() {
        let message = error_of("vertex { x = 0; y = 0; }");
        assert!(
            message.contains("namespace") && message.contains("zdoom"),
            "the error names the missing global and a fix: {message}"
        );
        let shape = error_of("namespace = 5; vertex { x = 0; y = 0; }");
        assert!(
            shape.contains("quoted string"),
            "a bare namespace value is refused: {shape}"
        );
    }

    #[test]
    fn a_missing_required_field_names_the_block_and_index() {
        let cases = [
            ("vertex { x = 0; }", "vertex 0", "y"),
            ("linedef { v1 = 0; v2 = 0; }", "linedef 0", "sidefront"),
            ("sidedef { offsetx = 4; }", "sidedef 0", "sector"),
            (
                "sector { texturefloor = \"A\"; }",
                "sector 0",
                "textureceiling",
            ),
            ("thing { x = 0; y = 0; }", "thing 0", "type"),
        ];
        for (body, block, field) in cases {
            let message = error_of(&format!("namespace = \"doom\"; {body}"));
            assert!(
                message.contains(block) && message.contains(field),
                "'{body}' must name {block} and '{field}': {message}"
            );
        }
        let second = error_of("namespace = \"doom\"; vertex { x = 0; y = 0; } vertex { x = 1; }");
        assert!(
            second.contains("vertex 1") && second.contains("'y'"),
            "the index counts the blocks of the kind: {second}"
        );
        let wrong_shape = error_of("namespace = \"doom\"; vertex { x = \"a\"; y = 0; }");
        assert!(
            wrong_shape.contains("vertex 0") && wrong_shape.contains("'x'"),
            "a required field of the wrong shape is refused: {wrong_shape}"
        );
    }

    #[test]
    fn a_block_without_textmap_and_binary_text_are_refused() {
        let members = [marker("THINGS"), marker("ENDMAP")];
        let CyberError::Format(message) =
            read_udmf_map(name("MAP01"), &members).expect_err("no TEXTMAP here")
        else {
            panic!("expected Format for a block without TEXTMAP");
        };
        assert!(message.contains("TEXTMAP"), "the lump is named: {message}");
        let binary = [
            lump("TEXTMAP", &[0xFF_u8, 0xFE, 0x00, 0x80]),
            marker("ENDMAP"),
        ];
        let CyberError::Format(utf8_message) =
            read_udmf_map(name("MAP01"), &binary).expect_err("no UTF-8 here")
        else {
            panic!("expected Format for a binary TEXTMAP");
        };
        assert!(
            utf8_message.contains("UTF-8"),
            "the reason is named: {utf8_message}"
        );
    }

    #[test]
    fn other_lumps_between_the_markers_are_carried() {
        let members = [
            lump("TEXTMAP", b"namespace = \"zdoom\";"),
            lump("BEHAVIOR", b"acs"),
            lump("ZNODES", b"zn"),
            marker("ENDMAP"),
        ];
        let map = read_udmf_map(name("E1M1"), &members).expect("the block reads");
        let carried: Vec<String> = map.other.iter().map(|lump| lump.name.to_string()).collect();
        assert_eq!(
            carried,
            ["BEHAVIOR", "ZNODES"],
            "TEXTMAP and ENDMAP stay out"
        );
        let written = write_udmf_map(&map).expect("the map writes");
        let order: Vec<String> = written.iter().map(|lump| lump.name.to_string()).collect();
        assert_eq!(
            order,
            ["E1M1", "TEXTMAP", "BEHAVIOR", "ZNODES", "ENDMAP"],
            "the carried lumps sit between the markers, in order"
        );
        let read = read_udmf_map(map.name, written.get(1..).expect("members")).expect("re-read");
        assert_eq!(read, map, "the carried lumps round trip");
    }
}
