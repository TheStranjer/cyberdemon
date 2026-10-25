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
//!
//! The two directions are implementation details of these entry points:
//! the field reader and the five element readers live in the private
//! `read` module, the block writer and the five element writers in the
//! private `write` one, and this root holds what both share — the
//! defaults and the flag-name tables — beside the block walk that
//! dispatches a document's blocks and the document builder that orders
//! the element blocks.

mod read;
mod write;

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::classify::MapFormat;
use crate::wad::name::LumpName;
use crate::wad::udmf::{UdmfDoc, UdmfValue, parse_udmf, write_udmf};

use super::flags::{
    FlagFamily, IMPACT, MISSILE_CROSS, MONSTER_CROSS, PLAYER_CROSS, PLAYER_PUSH, PLAYER_USE,
    THING_AMBUSH, THING_DOOM_FRIEND, THING_DOOM_NOT_COOP, THING_DOOM_NOT_DM, THING_DOOM_NOT_SINGLE,
    THING_EASY, THING_HARD, THING_MEDIUM,
};
use super::model::Map;

use read::{Fields, read_linedef, read_sector, read_sidedef, read_thing, read_vertex};
use write::{write_linedef, write_sector, write_sidedef, write_thing, write_vertex};

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
    ("playercross", PLAYER_CROSS, false),
    ("playeruse", PLAYER_USE, false),
    ("monstercross", MONSTER_CROSS, false),
    ("impact", IMPACT, false),
    ("playerpush", PLAYER_PUSH, false),
    ("missilecross", MISSILE_CROSS, false),
    ("monsteruse", PLAYER_USE, true),
    ("monsterpush", PLAYER_PUSH, true),
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
    (THING_EASY, &["skill1", "skill2"]),
    (THING_MEDIUM, &["skill3"]),
    (THING_HARD, &["skill4", "skill5"]),
    (THING_AMBUSH, &["ambush"]),
    (THING_DOOM_FRIEND, &["friend"]),
];

/// `Doom` family thing presence bits: the flag bit and the boolean name
/// that, not true, sets it.
const DOOM_THING_INVERSES: [(u32, &str); 3] = [
    (THING_DOOM_NOT_SINGLE, "single"),
    (THING_DOOM_NOT_DM, "dm"),
    (THING_DOOM_NOT_COOP, "coop"),
];

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

#[cfg(test)]
mod tests;
