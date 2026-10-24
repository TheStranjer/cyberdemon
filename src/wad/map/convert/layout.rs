//! The mechanical tail of one [`convert_map`](super::convert_map): what each
//! target layout stores, what it has no room for and what happens to it.
//! [`family_of`] and [`default_namespace`] read a layout as the flag family
//! its `flags` live in, [`into_udmf`] and [`into_binary`] set the layout
//! fields last — after [`as_line_id`] has folded each binary `Hexen`
//! `Line_SetIdentification` into the model's line id and [`store_line_ids`]
//! has carried the ids the other way — and [`strip_node_lumps`],
//! [`strip_behavior`] and [`drop_extras`] remove what the target cannot
//! carry, counting the dropped `UDMF` fields and the elements that held a
//! rounded original through [`ExtraBag`]. [`parse_format`] and
//! [`namespace_name`] validate the two words of a request before any of it
//! runs.

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use crate::wad::udmf::{decimal_to_rounded_int, has_fraction};

use super::plan::Plan;
use super::{
    BEHAVIOR, Conversion, DOOM_NAMESPACE, HEXEN_NAMESPACE, KindCounts, MAX_BYTE, MAX_NAMESPACE_LEN,
    SET_IDENTIFICATION, is_node_lump,
};

/// The flag family one target layout stores its `flags` in; a `UDMF` target
/// that names no namespace reads as the `Hexen` family, the way
/// [`crate::wad::map::udmf_map`] writes an unset namespace out.
pub(super) fn family_of(format: MapFormat, namespace: Option<&str>) -> FlagFamily {
    match format {
        MapFormat::Doom => FlagFamily::Doom,
        MapFormat::Hexen => FlagFamily::Hexen,
        MapFormat::Udmf => FlagFamily::from_udmf_namespace(namespace.unwrap_or(HEXEN_NAMESPACE)),
    }
}

/// The namespace a `UDMF` target of `family` declares when the caller names
/// none.
pub(super) fn default_namespace(family: FlagFamily) -> String {
    match family {
        FlagFamily::Doom => DOOM_NAMESPACE.to_owned(),
        FlagFamily::Hexen => HEXEN_NAMESPACE.to_owned(),
    }
}

/// Rewrites `map` as a `UDMF` block in the plan's namespace, folding each
/// binary `Hexen` `Line_SetIdentification` into the line id `UDMF` stores.
/// A cross-family step has already folded every one of them; the layout
/// fields are set last precisely so this fold never fires a second time.
pub(super) fn into_udmf(map: &mut Map, plan: &Plan) {
    if map.format == MapFormat::Hexen {
        for line in &mut map.linedefs {
            as_line_id(line);
        }
    }
    map.format = MapFormat::Udmf;
    map.udmf_namespace.clone_from(&plan.namespace);
}

/// Rewrites `map` as a binary block in the plan's format: the line ids of a
/// `Hexen` family source move into `Line_SetIdentification`, every `extra`
/// bag goes, and a `BEHAVIOR` lump cannot stay in a `Doom` block.
pub(super) fn into_binary(map: &mut Map, plan: &Plan, conversion: &mut Conversion) {
    if plan.format == MapFormat::Hexen && map.format == MapFormat::Udmf {
        store_line_ids(map, &mut conversion.warnings);
    }
    drop_extras(map, conversion);
    if plan.format == MapFormat::Doom {
        strip_behavior(map, conversion);
    }
    map.format = plan.format;
    map.udmf_namespace = None;
}

/// Removes every generated node lump from `map`'s block lumps.
pub(super) fn strip_node_lumps(map: &mut Map) {
    map.other.retain(|lump| !is_node_lump(lump.name.as_bytes()));
}

/// Drops the `BEHAVIOR` lump of a block rewritten as binary `Doom`: that
/// lump is what makes [`crate::wad::classify`] read a block as `Hexen`, so
/// keeping it would convert the map to a layout nobody asked for.
fn strip_behavior(map: &mut Map, conversion: &mut Conversion) {
    let before = map.other.len();
    map.other.retain(|lump| lump.name.as_bytes() != BEHAVIOR);
    if map.other.len() != before {
        conversion.warnings.push(
            "the BEHAVIOR lump was dropped: a binary Doom block that carries one reads back as \
             a Hexen map; convert to UDMF to keep the script"
                .to_owned(),
        );
    }
}

/// Moves one binary `Hexen` `Line_SetIdentification` onto the `UDMF` line id
/// the format uses for it: `args[0]` becomes the id and the special, which
/// `UDMF` has no room for, goes.
const fn as_line_id(line: &mut Linedef) {
    if line.special != SET_IDENTIFICATION {
        return;
    }
    let [id, arg1, arg2, arg3, arg4] = line.args;
    line.tag = id;
    line.special = 0_i32;
    line.args = [0_i32, arg1, arg2, arg3, arg4];
}

/// Folds every `UDMF` line id of `map` into what binary `Hexen` can store.
/// A cross-family step's forward stage has already settled every id; the
/// layout fields are set last precisely so this store never fires a second
/// time.
fn store_line_ids(map: &mut Map, warnings: &mut Vec<String>) {
    for (index, line) in map.linedefs.iter_mut().enumerate() {
        store_line_id(line, index, warnings);
    }
}

/// Folds one line's id (the model's [`Linedef::tag`]) into its binary
/// `Hexen` record: an otherwise plain line becomes
/// `Line_SetIdentification` carrying the id in `args[0]`; an id over a
/// byte's range, or on a line that already has a special, cannot be stored
/// and drops with a warning naming the line.
fn store_line_id(line: &mut Linedef, index: usize, warnings: &mut Vec<String>) {
    let id = line.tag;
    if id == 0_i32 {
        return;
    }
    line.tag = 0_i32;
    if line.special != 0_i32 {
        warnings.push(format!(
            "linedef {index} line id {id} was dropped: the line already uses special {}, and \
             Hexen stores a line id only in Line_SetIdentification (special {SET_IDENTIFICATION})",
            line.special
        ));
        return;
    }
    if id > MAX_BYTE {
        warnings.push(format!(
            "linedef {index} line id {id} was dropped: Hexen keeps it in one byte (0 to \
             {MAX_BYTE}); lower the id or stay on a UDMF map"
        ));
        return;
    }
    let [_, arg1, arg2, arg3, arg4] = line.args;
    line.args = [id, arg1, arg2, arg3, arg4];
    line.special = SET_IDENTIFICATION;
}

/// Drops every `extra` bag of `map`, counting the fields and the elements
/// that held a rounded original per element kind.
fn drop_extras(map: &mut Map, conversion: &mut Conversion) {
    let things = drop_bags(&mut map.things);
    let linedefs = drop_bags(&mut map.linedefs);
    let sidedefs = drop_bags(&mut map.sidedefs);
    let sectors = drop_bags(&mut map.sectors);
    let vertices = drop_bags(&mut map.vertices);
    conversion.dropped_fields = KindCounts {
        things: things.dropped,
        linedefs: linedefs.dropped,
        sidedefs: sidedefs.dropped,
        sectors: sectors.dropped,
        vertices: vertices.dropped,
    };
    conversion.fractional_coordinates = KindCounts {
        things: things.rounded,
        linedefs: linedefs.rounded,
        sidedefs: sidedefs.rounded,
        sectors: sectors.rounded,
        vertices: vertices.rounded,
    };
}

/// The `UDMF` fields one element kind holds, plus how many of them are the
/// preserved text of a rounded number.
struct Dropped {
    /// The `extra` entries that went.
    dropped: usize,
    /// The elements whose `extra` held a rounded original.
    rounded: usize,
}

/// One element row's `extra` bag and the keys its kind's model reads as
/// numbers, so the whole-map passes can treat the five kinds alike.
trait ExtraBag {
    /// The `UDMF` keys this kind reads into modelled number columns: an
    /// `extra` entry under one of them holds the written text of a value
    /// the loader rounded.
    const NUMBERS: &'static [&'static str];

    /// The row's unmodelled fields.
    fn bag(&mut self) -> &mut BTreeMap<String, String>;
}

impl ExtraBag for Vertex {
    const NUMBERS: &'static [&'static str] = &["x", "y"];

    #[inline]
    fn bag(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.extra
    }
}

impl ExtraBag for Thing {
    const NUMBERS: &'static [&'static str] = &[
        "id", "x", "y", "height", "angle", "type", "special", "arg0", "arg1", "arg2", "arg3",
        "arg4",
    ];

    #[inline]
    fn bag(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.extra
    }
}

impl ExtraBag for Linedef {
    const NUMBERS: &'static [&'static str] = &[
        "v1",
        "v2",
        "sidefront",
        "sideback",
        "special",
        "id",
        "arg0",
        "arg1",
        "arg2",
        "arg3",
        "arg4",
    ];

    #[inline]
    fn bag(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.extra
    }
}

impl ExtraBag for Sidedef {
    const NUMBERS: &'static [&'static str] = &["offsetx", "offsety", "sector"];

    #[inline]
    fn bag(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.extra
    }
}

impl ExtraBag for Sector {
    const NUMBERS: &'static [&'static str] = &[
        "heightfloor",
        "heightceiling",
        "lightlevel",
        "special",
        "id",
    ];

    #[inline]
    fn bag(&mut self) -> &mut BTreeMap<String, String> {
        &mut self.extra
    }
}

/// Empties every row's `extra` bag in `rows`, counting the fields that went
/// and the rows that held a rounded original.
fn drop_bags<T: ExtraBag>(rows: &mut [T]) -> Dropped {
    let mut dropped = 0_usize;
    let mut rounded = 0_usize;
    for row in rows {
        let bag = row.bag();
        dropped = dropped.saturating_add(bag.len());
        if held_fraction(bag, T::NUMBERS) {
            rounded = rounded.saturating_add(1_usize);
        }
        bag.clear();
    }
    Dropped { dropped, rounded }
}

/// Whether `bag` holds the written text of a modelled number the loader had
/// to round: a decimal under one of `numbers` whose value is no whole
/// number.
fn held_fraction(bag: &BTreeMap<String, String>, numbers: &[&str]) -> bool {
    bag.iter().any(|(key, text)| {
        numbers.contains(&key.as_str()) && has_fraction(text) && is_decimal(text)
    })
}

/// Whether `text` names a decimal value at all (the reader keeps any other
/// shape verbatim, and no rounding happened to it).
fn is_decimal(text: &str) -> bool {
    decimal_to_rounded_int(text).is_ok()
}

/// The `MapFormat` one `to` word names, case-insensitively.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the three accepted spellings.
pub(super) fn parse_format(to: &str) -> Result<MapFormat, CyberError> {
    match to.to_ascii_lowercase().as_str() {
        "doom" => Ok(MapFormat::Doom),
        "hexen" => Ok(MapFormat::Hexen),
        "udmf" => Ok(MapFormat::Udmf),
        _ => Err(CyberError::InvalidArgument(format!(
            "unknown target format {to:?}: convert_map_format writes \"doom\", \"hexen\" or \
             \"udmf\""
        ))),
    }
}

/// The lowercased namespace, or a refusal when it is no `UDMF` namespace
/// name: 1 to [`MAX_NAMESPACE_LEN`] `ASCII` letters, digits or underscores.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] describing the accepted shape.
pub(super) fn namespace_name(text: &str) -> Result<String, CyberError> {
    let lowered = text.to_ascii_lowercase();
    let shaped = !lowered.is_empty()
        && lowered.len() <= MAX_NAMESPACE_LEN
        && lowered
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if shaped {
        return Ok(lowered);
    }
    Err(CyberError::InvalidArgument(format!(
        "namespace {text:?} is not a UDMF namespace name: use 1 to {MAX_NAMESPACE_LEN} ASCII \
         letters, digits or underscores"
    )))
}
