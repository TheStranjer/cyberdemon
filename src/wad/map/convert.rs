//! Map format conversion, within and across the flag families: the pure
//! step behind the `convert_map_format` tool.
//!
//! A [`Map`] carries its layout in [`Map::format`] and, for `UDMF`, in
//! [`Map::udmf_namespace`]; its five element blocks are format-neutral, so
//! converting a map means rewriting those two fields plus the handful of
//! fields one layout can express and the other cannot. Inside one
//! [`FlagFamily`] (see [`Map::flag_family`], the tables the `flags` columns
//! are stored in) the bits keep their meaning and nothing needs
//! translating; a request naming the layout the map is already in refuses
//! [`CyberError::InvalidArgument`]. Across the families the [`super::xlat`]
//! stages translate best-effort through the embedded `ZDoom` table in
//! whichever direction the request asks: `Doom` family → `Hexen` family
//! rewrites every linedef special, thing flag set and sector type into its
//! generalized shape, and `Hexen` family → `Doom` family runs the table in
//! reverse, matching each line against the rows and restoring the legacy
//! special and trigger tag. Specials no row carries come back as 0, named
//! in [`Conversion::untranslated`].
//!
//! What each direction carries over, within one family:
//!
//! - binary `Doom` → `UDMF` (`doom`, `heretic` or `strife`): nothing to
//!   translate. The line `tag` stays the line `tag`; [`super::udmf_map`]
//!   writes it as both `id` and `arg0` and folds them back on read, so the
//!   conversion is lossless.
//! - `UDMF` (`Doom` family) → binary `Doom`: a binary record has no room
//!   for what the model does not name, so every `extra` entry goes, counted
//!   per element kind in [`Conversion::dropped_fields`]; the elements whose
//!   `extra` held the written text of a number the loader had already
//!   rounded are counted in [`Conversion::fractional_coordinates`]. Values
//!   outside the stored field types are [`super::doom`]'s refusal, i.e.
//!   [`CyberError::LimitExceeded`] naming element and field.
//! - binary `Hexen` → `UDMF` (a `Hexen` family namespace): `UDMF` has no
//!   `Line_SetIdentification`, so each line's `args[0]` becomes its line id
//!   (the model's [`Linedef::tag`]) and that special goes. `BEHAVIOR` and
//!   `SCRIPTS` ride along in [`Map::other`].
//! - `UDMF` (`Hexen` family) → binary `Hexen`: the reverse. A line with a
//!   non-zero id and no special becomes `Line_SetIdentification`
//!   (`special` 121) carrying the id in `args[0]`; an id over a byte, or on
//!   a line that already has a special, has nowhere to live and drops with
//!   a warning naming the line. [`super::hexen`] synthesizes the minimal
//!   `BEHAVIOR` the format needs when [`Map::other`] carries none.
//!
//! Across the families the translation stages run first, then exactly the
//! mechanical tail above for the target layout; `map.format` and
//! `map.udmf_namespace` are rewritten last, so the binary-`Hexen` 121 fold
//! and the `Line_SetIdentification` store never fire twice. Forward
//! (`Doom` family → `Hexen` family): a `Doom` `passuse` bit moves to the
//! line's `extra` bag (bit 512 is `repeatspecial` in the `Hexen` family),
//! the line tag moves into the table row's `tag` argument slot, thing
//! presence bits invert to their positive `Hexen` twins and Boom-style
//! sector types sit behind 64. Back (`Hexen` family → `Doom` family): a
//! line matching a row restores its `Doom` special and the trigger tag
//! from the row's `tag` slot (the lowest `Doom` special wins an ambiguous
//! match) and a plain line's id becomes its `Doom` trigger tag;
//! `Line_SetIdentification` from a binary source folds to its id first,
//! while a literal special 121 from a `UDMF` source has no table row, so
//! it clears to 0 with its id kept as the trigger tag. `Hexen`-only thing
//! bits and the `blockplayers`/`blockeverything` line bits drop,
//! `Hexen`-native sector types clear to 0, and every loss is counted or
//! warned in [`Conversion`].
//!
//! Every conversion removes the block's generated node lumps: their layout
//! is tied to the map format, so [`Map::other`] keeps none of `SEGS`,
//! `SSECTORS`, `NODES`, `REJECT`, `BLOCKMAP` or `ZNODES`, and the caller
//! rebuilds them. A block rewritten as binary `Doom` additionally loses a
//! `BEHAVIOR` lump (its presence is what
//! [`crate::wad::classify::analyze`] reads as the `Hexen` format) with a
//! warning.

use alloc::collections::BTreeMap;

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use crate::wad::map::write_map;
use crate::wad::map::xlat::{self, FieldReport, LinesReport, Untranslated};
use crate::wad::udmf::{decimal_to_rounded_int, has_fraction};

/// The `Hexen` line special that stores a line id: `Line_SetIdentification`.
const SET_IDENTIFICATION: i32 = 121_i32;
/// The largest value a binary `Hexen` byte field stores.
const MAX_BYTE: i32 = 255_i32;
/// The `UDMF` namespace a `Doom` family map declares when the caller names none.
const DOOM_NAMESPACE: &str = "doom";
/// The `UDMF` namespace a `Hexen` family map declares when the caller names none.
const HEXEN_NAMESPACE: &str = "zdoom";
/// The `BEHAVIOR` lump, which only a binary `Hexen` block may carry.
const BEHAVIOR: &[u8] = b"BEHAVIOR";
/// Longest accepted `UDMF` namespace name, in characters.
const MAX_NAMESPACE_LEN: usize = 32;
/// The most [`Untranslated`] lines one result lists; past it one warning
/// counts what was omitted.
const UNTRANSLATED_LIMIT: usize = 100;

/// The layout one conversion writes: a [`MapFormat`] and, for `UDMF`, the
/// namespace its `TEXTMAP` declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The format to write.
    pub format: MapFormat,
    /// The lowercased `UDMF` namespace to declare; `None` asks for the
    /// source's family default and is always `None` for a binary format.
    pub namespace: Option<String>,
}

impl Target {
    /// Parses a `to` word (`"doom"`, `"hexen"` or `"udmf"`, any case) with
    /// its optional `UDMF` namespace.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when `to` names no format, when a
    /// namespace comes with a binary format, or when it is not 1 to 32
    /// `ASCII` letters, digits or underscores.
    #[inline]
    pub fn parse(to: &str, namespace: Option<&str>) -> Result<Self, CyberError> {
        let format = parse_format(to)?;
        let wanted = namespace.map(namespace_name).transpose()?;
        if wanted.is_some() && format != MapFormat::Udmf {
            return Err(CyberError::InvalidArgument(format!(
                "namespace {namespace:?} only applies to UDMF maps: drop it to convert to \"{}\" \
                 (a binary format declares no namespace)",
                format_name(format)
            )));
        }
        Ok(Self {
            format,
            namespace: wanted,
        })
    }
}

/// The outcome of one [`convert_map`] step: what it translated, dropped
/// and every notice the caller should see.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Conversion {
    /// Lines whose special moved through the embedded `ZDoom` table; zero
    /// for a same-family conversion.
    pub translated_specials: usize,
    /// Lines carrying a special the table could not translate, in line
    /// order; each one ends with `special` 0. At most one hundred entries
    /// are listed; past them one warning counts the omitted ones.
    pub untranslated: Vec<Untranslated>,
    /// Things whose presence flags the cross-family stage rewrote.
    pub thing_flags_changed: usize,
    /// Sectors whose `special` the cross-family stage rewrote.
    pub sector_types_changed: usize,
    /// `UDMF` fields the target format cannot store, counted per kind.
    pub dropped_fields: KindCounts,
    /// Elements whose `extra` held the written text of a number the loader
    /// had already rounded, counted per kind.
    pub fractional_coordinates: KindCounts,
    /// Lossy steps the caller should know about.
    pub warnings: Vec<String>,
}

/// One count per map element kind, in the order map results list them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct KindCounts {
    /// The [`Thing`] count.
    pub things: usize,
    /// The [`Linedef`] count.
    pub linedefs: usize,
    /// The [`Sidedef`] count.
    pub sidedefs: usize,
    /// The [`Sector`] count.
    pub sectors: usize,
    /// The [`Vertex`] count.
    pub vertices: usize,
}

/// The lowercase name of one map layout, spelled as the tool results and
/// `list_maps` spell it.
#[must_use]
#[inline]
pub const fn format_name(format: MapFormat) -> &'static str {
    match format {
        MapFormat::Doom => "doom",
        MapFormat::Hexen => "hexen",
        MapFormat::Udmf => "udmf",
    }
}

/// Whether `name` is a generated node lump, whose layout is tied to the map
/// format: the vanilla `SEGS`/`SSECTORS`/`NODES`/`REJECT`/`BLOCKMAP` run or
/// the `UDMF` `ZNODES` run.
#[must_use]
#[inline]
pub const fn is_node_lump(name: &[u8]) -> bool {
    matches!(
        name,
        b"SEGS" | b"SSECTORS" | b"NODES" | b"REJECT" | b"BLOCKMAP" | b"ZNODES"
    )
}

/// Converts `map` in place into `target` and reports what that cost.
///
/// The map keeps its elements, indices and references: only the layout
/// fields, the translated specials, flags and sector types, the `Hexen`
/// line ids and the fields the target cannot store change. The node lumps
/// of [`Map::other`] are removed.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `target` names the layout the map
/// already uses; `map` stands unchanged behind it.
/// [`CyberError::Internal`] when the embedded translation table is
/// malformed, which is a build bug.
#[inline]
pub fn convert_map(map: &mut Map, target: &Target) -> Result<Conversion, CyberError> {
    let plan = Plan::resolve(map, target)?;
    let mut conversion = Conversion::default();
    strip_node_lumps(map);
    match plan.direction {
        Direction::SameFamily => {}
        Direction::ToHexen => to_hexen(map, &plan, &mut conversion)?,
        Direction::FromHexen => from_hexen(map, &mut conversion)?,
    }
    if plan.format == MapFormat::Udmf {
        into_udmf(map, &plan);
    } else {
        into_binary(map, &plan, &mut conversion);
    }
    Ok(conversion)
}

/// Whether `map` writes in the layout it now claims.
///
/// The target format's range checks belong to its writer, so a conversion
/// that only reports runs this to surface the refusals the real write would
/// have hit.
///
/// # Errors
///
/// [`write_map`]'s errors: [`CyberError::LimitExceeded`] or
/// [`CyberError::InvalidArgument`] naming the element and field the target
/// cannot store, and [`CyberError::Internal`] for a size the format
/// excludes.
#[inline]
pub fn encodes(map: &Map) -> Result<(), CyberError> {
    write_map(map).map(drop)
}

/// The destination of one request, resolved against the map it applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Plan {
    /// The format to write.
    format: MapFormat,
    /// The namespace to declare; `Some` exactly for a `UDMF` target, and
    /// then always the family default when the caller named none.
    namespace: Option<String>,
    /// Which way the field translation runs between the map's family and
    /// the target's.
    direction: Direction,
}

/// Whether one conversion stays inside the map's flag family, and if not,
/// which way the [`super::xlat`] stages translate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    /// Source and target store their `flags` in the same family.
    SameFamily,
    /// `Doom` family to `Hexen` family: the table runs forward.
    ToHexen,
    /// `Hexen` family to `Doom` family: the table runs in reverse.
    FromHexen,
}

impl Plan {
    /// Resolves `target` against `map`: the namespace to declare, the
    /// direction the step takes and the check that it means anything (not
    /// the layout the map is already in).
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] as [`convert_map`] reports it.
    fn resolve(map: &Map, target: &Target) -> Result<Self, CyberError> {
        let namespace = target.namespace.clone().or_else(|| {
            (target.format == MapFormat::Udmf).then(|| default_namespace(map.flag_family()))
        });
        let plan = Self {
            format: target.format,
            direction: direction_between(map.flag_family(), target.format, namespace.as_deref()),
            namespace,
        };
        plan.require_change(map)?;
        Ok(plan)
    }

    /// Refuses a target that names the layout and namespace the map uses.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] saying the map already is in that
    /// format.
    fn require_change(&self, map: &Map) -> Result<(), CyberError> {
        if !self.names_the_same_layout(map) {
            return Ok(());
        }
        Err(CyberError::InvalidArgument(format!(
            "{} already is a {} map: nothing to convert; pass a different to (and, for UDMF, \
             namespace) pair",
            map.name,
            self.label()
        )))
    }

    /// Whether this plan rewrites nothing at all.
    fn names_the_same_layout(&self, map: &Map) -> bool {
        if map.format != self.format {
            return false;
        }
        match (self.namespace.as_deref(), map.udmf_namespace.as_deref()) {
            (Some(wanted), Some(current)) => wanted.eq_ignore_ascii_case(current),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }

    /// The plan's layout as text: `hexen`, or `udmf (zdoom)`.
    fn label(&self) -> String {
        let format = format_name(self.format);
        self.namespace.as_deref().map_or_else(
            || format.to_owned(),
            |namespace| format!("{format} ({namespace})"),
        )
    }
}

/// Which way the field translation runs from a `source` family into the
/// target layout; a UDMF target that names no namespace reads as the
/// source's own family default, so it never changes family.
fn direction_between(source: FlagFamily, format: MapFormat, namespace: Option<&str>) -> Direction {
    match (source, family_of(format, namespace)) {
        (FlagFamily::Doom, FlagFamily::Hexen) => Direction::ToHexen,
        (FlagFamily::Hexen, FlagFamily::Doom) => Direction::FromHexen,
        (FlagFamily::Doom, FlagFamily::Doom) | (FlagFamily::Hexen, FlagFamily::Hexen) => {
            Direction::SameFamily
        }
    }
}

/// Runs the three forward stages over `map`: the linedef specials through
/// the table (byte-limited when the target is binary `Hexen`), the thing
/// flags and the sector types, folding their reports into `conversion`.
///
/// # Errors
///
/// [`super::xlat::linedefs_to_hexen`]'s [`CyberError::Internal`] for
/// malformed embedded data.
fn to_hexen(map: &mut Map, plan: &Plan, conversion: &mut Conversion) -> Result<(), CyberError> {
    let byte_limited = plan.format == MapFormat::Hexen;
    let lines = xlat::linedefs_to_hexen(map, byte_limited)?;
    let things = xlat::things_to_hexen(map);
    let sectors = xlat::sectors_to_hexen(map);
    fold_reports(lines, things, sectors, conversion);
    Ok(())
}

/// Runs the three reverse stages over `map`: the linedef specials back
/// through the table (a `UDMF` source already keeps its line ids in the
/// model's tag field), the thing flags and the sector types, folding their
/// reports into `conversion`.
///
/// # Errors
///
/// [`super::xlat::linedefs_from_hexen`]'s [`CyberError::Internal`] for
/// malformed embedded data.
fn from_hexen(map: &mut Map, conversion: &mut Conversion) -> Result<(), CyberError> {
    let source_udmf = map.format == MapFormat::Udmf;
    let lines = xlat::linedefs_from_hexen(map, source_udmf)?;
    let things = xlat::things_from_hexen(map);
    let sectors = xlat::sectors_from_hexen(map);
    fold_reports(lines, things, sectors, conversion);
    Ok(())
}

/// Folds one cross-family step's three stage reports into the result: the
/// counts, the [`UNTRANSLATED_LIMIT`]-capped untranslated list with an
/// omitted-count warning, and every stage warning in stage order.
fn fold_reports(
    lines: LinesReport,
    things: FieldReport,
    sectors: FieldReport,
    conversion: &mut Conversion,
) {
    let missed = lines.untranslated.len();
    conversion.translated_specials = lines.translated;
    conversion.untranslated = lines
        .untranslated
        .into_iter()
        .take(UNTRANSLATED_LIMIT)
        .collect();
    conversion.thing_flags_changed = things.changed;
    conversion.sector_types_changed = sectors.changed;
    conversion.warnings.extend(lines.warnings);
    conversion.warnings.extend(things.warnings);
    conversion.warnings.extend(sectors.warnings);
    let omitted = missed.saturating_sub(UNTRANSLATED_LIMIT);
    if omitted > 0 {
        conversion.warnings.push(format!(
            "{omitted} further untranslated linedefs are not listed (the result names at most \
             {UNTRANSLATED_LIMIT})"
        ));
    }
}

/// The flag family one target layout stores its `flags` in; a `UDMF` target
/// that names no namespace reads as the `Hexen` family, the way
/// [`super::udmf_map`] writes an unset namespace out.
fn family_of(format: MapFormat, namespace: Option<&str>) -> FlagFamily {
    match format {
        MapFormat::Doom => FlagFamily::Doom,
        MapFormat::Hexen => FlagFamily::Hexen,
        MapFormat::Udmf => FlagFamily::from_udmf_namespace(namespace.unwrap_or(HEXEN_NAMESPACE)),
    }
}

/// The namespace a `UDMF` target of `family` declares when the caller names
/// none.
fn default_namespace(family: FlagFamily) -> String {
    match family {
        FlagFamily::Doom => DOOM_NAMESPACE.to_owned(),
        FlagFamily::Hexen => HEXEN_NAMESPACE.to_owned(),
    }
}

/// Rewrites `map` as a `UDMF` block in the plan's namespace, folding each
/// binary `Hexen` `Line_SetIdentification` into the line id `UDMF` stores.
/// A cross-family step has already folded every one of them; the layout
/// fields are set last precisely so this fold never fires a second time.
fn into_udmf(map: &mut Map, plan: &Plan) {
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
fn into_binary(map: &mut Map, plan: &Plan, conversion: &mut Conversion) {
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
fn strip_node_lumps(map: &mut Map) {
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
fn parse_format(to: &str) -> Result<MapFormat, CyberError> {
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
fn namespace_name(text: &str) -> Result<String, CyberError> {
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

#[cfg(test)]
mod tests {
    use super::{
        Conversion, KindCounts, Target, UNTRANSLATED_LIMIT, Untranslated, convert_map, encodes,
        family_of, format_name, is_node_lump,
    };
    use crate::error::CyberError;
    use crate::wad::classify::{MapFormat, analyze};
    use crate::wad::map::common::fixtures::{lump, marker, name, two_room_map};
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::map::udmf_map::read_udmf_map;
    use crate::wad::map::{Map, read_map, write_map};

    /// The target for the binary layout `to`.
    fn binary(to: &str) -> Target {
        Target::parse(to, None).expect("the test targets are valid")
    }

    /// The parsed `UDMF` target, naming no namespace.
    fn udmf_default() -> Target {
        Target::parse("udmf", None).expect("udmf is a target")
    }

    /// The parsed `UDMF` target declaring `namespace`.
    fn udmf(namespace: &str) -> Target {
        Target::parse("udmf", Some(namespace)).expect("the test namespaces are valid")
    }

    /// Writes `map` and reads it back through the format dispatcher.
    fn cycle(map: &Map) -> Map {
        let lumps = write_map(map).expect("the test map writes");
        let analysis = analyze(&lumps);
        let block = analysis
            .maps
            .first()
            .expect("the written lumps are a block");
        read_map(&lumps, block).expect("and decodes back")
    }

    /// The `UDMF` two-room map in `namespace`, carrying node lumps.
    fn udmf_map(namespace: &str) -> Map {
        let mut map = two_room_map();
        map.format = MapFormat::Udmf;
        map.udmf_namespace = Some(namespace.to_owned());
        map.other = vec![
            lump("ZNODES", b"znod"),
            lump("NODES", b"nodes"),
            lump("BEHAVIOR", b"behavior"),
        ];
        map
    }

    /// The two-room map as binary `Hexen`: `tid`s, `z`, one
    /// `Line_SetIdentification` line and the two `Hexen` script lumps.
    fn hexen_map() -> Map {
        let mut map = two_room_map();
        map.format = MapFormat::Hexen;
        if let Some(first) = map.things.first_mut() {
            first.tid = 5_i32;
            first.z = 128_i32;
        }
        if let Some(portal) = map.linedefs.get_mut(2) {
            portal.special = 121_i32;
            portal.args = [7_i32, 0_i32, 0_i32, 0_i32, 0_i32];
        }
        map.other.push(lump("BEHAVIOR", b"ACS\0script"));
        map.other.push(lump("SCRIPTS", b"animated"));
        map
    }

    /// A `Doom` family `UDMF` map from one `TEXTMAP` body, whose block also
    /// carries a `BEHAVIOR` lump.
    fn doom_udmf(body: &str) -> Map {
        let text = format!("namespace = \"doom\"; {body}");
        let members = [
            lump("TEXTMAP", text.as_bytes()),
            lump("BEHAVIOR", b"acs"),
            marker("ENDMAP"),
        ];
        read_udmf_map(name("MAP01"), &members).expect("the test TEXTMAP decodes")
    }

    /// A `zdoom` map from one `TEXTMAP` body.
    fn zdoom_udmf(body: &str) -> Map {
        let text = format!("namespace = \"zdoom\"; {body}");
        let members = [lump("TEXTMAP", text.as_bytes()), marker("ENDMAP")];
        read_udmf_map(name("MAP01"), &members).expect("the test TEXTMAP decodes")
    }

    /// One line, side, sector and vertex per element kind, each carrying the
    /// fields a binary record has no room for.
    const ODD_FIELDS: &str = concat!(
        "vertex { x = 64.5; y = 0; }",
        "thing { x = 0; y = 0.25; type = 5; comment = \"hi\"; }",
        "linedef { v1 = 0; v2 = 0; sidefront = 0; alpha = 0.5; }",
        "sidedef { sector = 0; offsetx = 1.5; scalex_top = 2; }",
        "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\";",
        " heightfloor = 8.5; lightfloor = 0.5; }",
    );

    /// The block lump names of `map`, in order.
    fn carried(map: &Map) -> Vec<String> {
        map.other.iter().map(|lump| lump.name.to_string()).collect()
    }

    /// The refusal `convert_map` gives `map` for `target`, as `(code, text)`.
    fn refusal(map: &Map, target: &Target) -> (String, String) {
        let mut copy = map.clone();
        match convert_map(&mut copy, target) {
            Err(error) => (error.code().to_owned(), error.to_string()),
            Ok(conversion) => panic!("expected a refusal, converted into {conversion:?}"),
        }
    }

    /// One `KindCounts` row from its five numbers, in [`KindCounts`] field
    /// order: things, linedefs, sidedefs, sectors, vertices.
    fn counts(row: [usize; 5]) -> KindCounts {
        let [things, linedefs, sidedefs, sectors, vertices] = row;
        KindCounts {
            things,
            linedefs,
            sidedefs,
            sectors,
            vertices,
        }
    }

    #[test]
    fn a_doom_map_round_trips_through_udmf_without_loss() {
        let original = two_room_map();
        let mut as_udmf = original.clone();
        let out = convert_map(&mut as_udmf, &udmf("doom")).expect("the Doom family converts");
        assert_eq!(out, Conversion::default(), "a Doom line needs no note");
        assert_eq!(as_udmf.format, MapFormat::Udmf, "the layout moved");
        assert_eq!(
            as_udmf.udmf_namespace.as_deref(),
            Some("doom"),
            "and declares its own namespace"
        );
        assert!(
            as_udmf.other.is_empty(),
            "the node lumps left: {:?}",
            carried(&as_udmf)
        );
        assert_eq!(
            as_udmf.linedefs, original.linedefs,
            "the lines are untouched"
        );
        assert_eq!(as_udmf.things, original.things, "and the things");
        assert_eq!(cycle(&as_udmf), as_udmf, "the UDMF block round trips");
        let mut back = cycle(&as_udmf);
        convert_map(&mut back, &binary("doom")).expect("and converts back");
        assert_eq!(back.format, MapFormat::Doom, "the layout came home");
        assert_eq!(back.udmf_namespace, None, "and the namespace went");
        assert_eq!(cycle(&back), original, "modulo the empty node markers");
    }

    /// The cross-family report keys of `out` as serialized: all four at
    /// their `derive(Default)` zero, alongside the ever-present
    /// `dropped_fields`, the shape a same-family step always publishes.
    fn assert_cross_family_keys_are_zero(out: &Conversion) {
        let value = serde_json::to_value(out).expect("a Conversion serializes");
        assert_eq!(value["translated_specials"], 0_i32, "the table did not run");
        assert_eq!(
            value["untranslated"],
            serde_json::json!([]),
            "nothing failed to translate"
        );
        assert_eq!(value["thing_flags_changed"], 0_i32, "no thing flag moved");
        assert_eq!(value["sector_types_changed"], 0_i32, "no sector type moved");
        assert_eq!(
            value["dropped_fields"],
            serde_json::json!({
                "things": 0_i32,
                "linedefs": 0_i32,
                "sidedefs": 0_i32,
                "sectors": 0_i32,
                "vertices": 0_i32,
            }),
            "the pre-existing key rides on the same object"
        );
    }

    #[test]
    fn a_same_family_conversion_publishes_the_cross_family_keys_as_zero() {
        let mut as_udmf = two_room_map();
        let out = convert_map(&mut as_udmf, &udmf("doom")).expect("the Doom family converts");
        assert_cross_family_keys_are_zero(&out);
        let mut back = cycle(&as_udmf);
        let again = convert_map(&mut back, &binary("doom")).expect("and converts back");
        assert_cross_family_keys_are_zero(&again);
    }

    #[test]
    fn an_unnamed_udmf_target_takes_the_source_family_default_namespace() {
        let mut from_doom = two_room_map();
        convert_map(&mut from_doom, &udmf_default()).expect("a Doom map converts");
        assert_eq!(
            from_doom.udmf_namespace.as_deref(),
            Some("doom"),
            "a Doom family source defaults to the doom namespace"
        );
        let mut from_hexen = hexen_map();
        convert_map(&mut from_hexen, &udmf_default()).expect("a Hexen map converts");
        assert_eq!(
            from_hexen.udmf_namespace.as_deref(),
            Some("zdoom"),
            "a Hexen family source defaults to zdoom"
        );
    }

    #[test]
    fn a_udmf_doom_map_drops_its_extra_fields_and_counts_them() {
        let mut map = doom_udmf(ODD_FIELDS);
        let out = convert_map(&mut map, &binary("doom")).expect("the Doom family converts back");
        assert_eq!(
            out.dropped_fields,
            counts([2_usize, 1_usize, 2_usize, 2_usize, 1_usize]),
            "every unmodelled field counts, the preserved fraction texts included"
        );
        assert_eq!(
            out.fractional_coordinates,
            counts([1_usize, 0_usize, 1_usize, 1_usize, 1_usize]),
            "only a modelled number's text is a rounded original: `alpha` and \
             `lightfloor` are unmodelled fields that simply drop"
        );
        let thing = map.things.first().expect("one thing");
        assert_eq!(thing.y, 0_i32, "the model kept the rounded integer");
        assert!(thing.extra.is_empty(), "and its bag is empty now");
        assert!(
            map.vertices
                .first()
                .is_some_and(|vertex| vertex.extra.is_empty() && vertex.x == 65_i32),
            "64.5 was already stored as 65 on load"
        );
        assert!(
            out.warnings
                .iter()
                .any(|text| text.contains("BEHAVIOR") && text.contains("binary Doom")),
            "the carried BEHAVIOR cannot stay: {:?}",
            out.warnings
        );
        assert_eq!(
            carried(&map),
            Vec::<String>::new(),
            "no block lump survived"
        );
        assert_eq!(map.udmf_namespace, None, "the namespace declaration went");
        assert_eq!(
            cycle(&map).format,
            MapFormat::Doom,
            "and the block reads as Doom"
        );
    }

    #[test]
    fn a_hexen_line_identification_becomes_a_udmf_line_id_and_back() {
        let original = hexen_map();
        let mut as_udmf = original.clone();
        let out = convert_map(&mut as_udmf, &udmf("zdoom")).expect("the Hexen family converts");
        assert!(out.warnings.is_empty(), "a storable id needs no note");
        assert_eq!(
            out.dropped_fields,
            KindCounts::default(),
            "UDMF drops nothing"
        );
        let identified = as_udmf.linedefs.get(2).expect("the identified line");
        assert_eq!(identified.tag, 7_i32, "args[0] became the line id");
        assert_eq!(identified.special, 0_i32, "the special went");
        assert_eq!(identified.args, [0_i32; 5], "and its arg with it");
        assert_eq!(
            carried(&as_udmf),
            vec!["BEHAVIOR", "SCRIPTS"],
            "the script lumps ride along, the node lumps do not"
        );
        assert_eq!(cycle(&as_udmf), as_udmf, "the UDMF block round trips");
        let mut back = cycle(&as_udmf);
        let again = convert_map(&mut back, &binary("hexen")).expect("and converts back");
        assert!(
            again.warnings.is_empty(),
            "the id fits a byte and the line is plain"
        );
        let restored = back.linedefs.get(2).expect("the identified line");
        assert_eq!(restored.tag, 0_i32, "Hexen stores no line id");
        assert_eq!(
            restored.special, 121_i32,
            "it became Line_SetIdentification"
        );
        assert_eq!(restored.args.first().copied(), Some(7_i32), "in args[0]");
        assert_eq!(cycle(&back), original, "the Hexen block is what it was");
    }

    #[test]
    fn an_unstorable_line_id_drops_with_a_warning() {
        let body = concat!(
            "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
            "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
            "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 9; special = 10; }",
            "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 300; }",
            "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 4; }",
        );
        let mut map = zdoom_udmf(body);
        let out = convert_map(&mut map, &binary("hexen")).expect("the Hexen family converts");
        assert_eq!(out.warnings.len(), 2, "two ids had nowhere to go");
        let taken = out.warnings.first().expect("two warnings");
        assert!(
            taken.contains("linedef 0 line id 9") && taken.contains("already uses special 10"),
            "the busy line is named: {taken}"
        );
        let oversized = out.warnings.get(1).expect("two warnings");
        assert!(
            oversized.contains("linedef 1 line id 300") && oversized.contains("one byte"),
            "the oversized id is named: {oversized}"
        );
        let busy = map.linedefs.first().expect("the busy line");
        assert_eq!(busy.tag, 0_i32, "the id left the model either way");
        assert_eq!(busy.special, 10_i32, "and its own special stayed");
        let free = map.linedefs.get(2).expect("the plain line");
        assert_eq!(
            free.special, 121_i32,
            "the storable id became Line_SetIdentification"
        );
        assert_eq!(free.args.first().copied(), Some(4_i32), "in args[0]");
        encodes(&map).expect("what is left writes, and the Hexen writer adds BEHAVIOR");
    }

    #[test]
    fn values_a_binary_hexen_record_cannot_store_are_limit_exceeded() {
        let body = concat!(
            "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
            "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
            "linedef { v1 = 0; v2 = 0; sidefront = 0; arg1 = 300; }",
        );
        let mut wide_arg = zdoom_udmf(body);
        convert_map(&mut wide_arg, &binary("hexen")).expect("the conversion itself is fine");
        let CyberError::LimitExceeded(message) = encodes(&wide_arg).expect_err("300 is no byte")
        else {
            panic!("expected LimitExceeded for a wide arg");
        };
        assert!(
            message.contains("linedef 0 args[1] 300 does not fit u8"),
            "the element and field are named: {message}"
        );
        let mut wide_special = zdoom_udmf(&body.replace("arg1 = 300", "special = 300"));
        convert_map(&mut wide_special, &binary("hexen")).expect("a wide special converts");
        let CyberError::LimitExceeded(special_message) =
            encodes(&wide_special).expect_err("300 is no byte")
        else {
            panic!("expected LimitExceeded for a wide special");
        };
        assert!(
            special_message.contains("linedef 0 special 300"),
            "the special is named: {special_message}"
        );
    }

    /// The two-room map with a `Doom` door line: linedef 2 is special 1
    /// (`Door_Raise`, tag 5) and carries the `passuse` bit.
    fn door_map() -> Map {
        let mut map = two_room_map();
        if let Some(step) = map.linedefs.get_mut(2) {
            step.special = 1_i32;
            step.tag = 5_i32;
            step.flags = 0x201_u32;
        }
        map
    }

    /// The two-room map with the door line converted forward to `zdoom`:
    /// the model and the conversion report, for the forward and reverse
    /// tests to share.
    fn door_udmf_zdoom() -> (Map, Conversion) {
        let mut map = door_map();
        let out = convert_map(&mut map, &udmf("zdoom")).expect("the forward direction converts");
        (map, out)
    }

    #[test]
    fn a_doom_map_converts_forward_through_the_table_to_hexen() {
        let (map, out) = door_udmf_zdoom();
        assert_eq!(map.format, MapFormat::Udmf, "the layout moved");
        assert_eq!(
            map.udmf_namespace.as_deref(),
            Some("zdoom"),
            "and declares zdoom"
        );
        let door = map.linedefs.get(2).expect("the door line");
        assert_eq!(door.special, 12_i32, "Door_Raise");
        assert_eq!(door.args, [0_i32, 16_i32, 150_i32, 5_i32, 0_i32]);
        assert_eq!(door.tag, 0_i32, "the trigger tag moved into its slot");
        assert_eq!(
            door.flags, 0x2601_u32,
            "blocking plus the use field, repeat and monster bits; passuse left bit 512"
        );
        assert_eq!(
            door.extra.get("passuse").map(String::as_str),
            Some("true"),
            "the Doom passuse bit rides on as an extra key"
        );
        assert_eq!(out.translated_specials, 1, "the door line rode the table");
        assert!(out.untranslated.is_empty(), "row 1 translates");
        assert_eq!(out.thing_flags_changed, 2, "both things move to Hexen bits");
        assert_eq!(out.sector_types_changed, 0, "plain sectors stand");
        assert!(
            out.warnings.is_empty(),
            "UDMF loses nothing: {:?}",
            out.warnings
        );
    }

    #[test]
    fn a_forward_converted_map_comes_home_and_drops_its_passuse_extra() {
        let (mut back, _there) = door_udmf_zdoom();
        let again = convert_map(&mut back, &binary("doom")).expect("and converts back");
        assert_eq!(
            again.translated_specials, 1,
            "row 1 came back through the table"
        );
        assert!(
            again.untranslated.is_empty(),
            "nothing failed going back: {:?}",
            again.warnings
        );
        assert_eq!(again.thing_flags_changed, 2, "the things come home too");
        let restored = back.linedefs.get(2).expect("the door line");
        assert_eq!(restored.special, 1_i32, "the Doom special is restored");
        assert_eq!(restored.tag, 5_i32, "with its trigger tag");
        assert_eq!(restored.args, [0_i32; 5], "and the generalized args gone");
        assert_eq!(
            restored.flags, 0x1_u32,
            "the trigger bits were consumed, blocking kept"
        );
        assert!(
            restored.extra.is_empty(),
            "the binary target drops the extra key"
        );
        assert_eq!(
            again.dropped_fields.linedefs, 1,
            "the forwarded passuse key counts once"
        );
        assert_eq!(
            format_name(back.format),
            "doom",
            "the line is home: only the passuse bit was spent on the trip"
        );
    }

    #[test]
    fn a_forward_conversion_to_binary_hexen_needs_no_line_id_carrier() {
        let mut map = two_room_map();
        if let Some(step) = map.linedefs.get_mut(2) {
            step.special = 1_i32;
            step.tag = 13_i32;
        }
        let out = convert_map(&mut map, &binary("hexen")).expect("the forward direction converts");
        assert_eq!(out.translated_specials, 1, "row 1's tag took its slot");
        assert!(out.untranslated.is_empty(), "row 1 fits the bytes");
        assert!(
            !out.warnings.iter().any(|note| note.contains("line id")),
            "the tag left no homeless id: {:?}",
            out.warnings
        );
        let door = map.linedefs.get(2).expect("the door line");
        assert_eq!(door.special, 12_i32, "Door_Raise in one byte");
        assert_eq!(door.args, [0_i32, 16_i32, 150_i32, 13_i32, 0_i32]);
        assert_eq!(door.tag, 0_i32, "binary Hexen models never tag lines");
        encodes(&map).expect("a binary Hexen record writes");
    }

    #[test]
    fn a_udmf_doom_family_source_stores_its_line_ids_once() {
        let body = concat!(
            "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
            "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
            "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 9; arg0 = 9; }",
        );
        let mut map = doom_udmf(body);
        let out = convert_map(&mut map, &binary("hexen")).expect("the forward direction converts");
        assert!(out.untranslated.is_empty(), "a plain line reports nothing");
        assert_eq!(out.translated_specials, 0, "and translates nothing");
        assert!(
            out.warnings.is_empty(),
            "keep_line_id stored the id once; store_line_ids found none left: {:?}",
            out.warnings
        );
        let line = map.linedefs.first().expect("the plain line");
        assert_eq!(line.special, 121_i32, "Line_SetIdentification carries it");
        assert_eq!(line.args.first().copied(), Some(9_i32), "in args[0]");
        assert_eq!(line.tag, 0_i32, "and the model tag emptied once");
        encodes(&map).expect("and the record writes");
    }

    #[test]
    fn a_hexen_map_converts_back_to_a_binary_doom_layout() {
        let mut map = hexen_map();
        let out = convert_map(&mut map, &binary("doom")).expect("the reverse direction converts");
        assert_eq!(out.translated_specials, 0, "no line needed the table");
        assert!(out.untranslated.is_empty(), "nothing failed to translate");
        assert_eq!(
            out.thing_flags_changed, 2,
            "both things fold back to Doom bits"
        );
        assert_eq!(out.sector_types_changed, 0, "plain sectors stand");
        let portal = map.linedefs.get(2).expect("the identified line");
        assert_eq!(portal.special, 0_i32, "Line_SetIdentification folded away");
        assert_eq!(
            portal.tag, 7_i32,
            "its id came back as the Doom trigger tag"
        );
        assert_eq!(portal.args, [0_i32; 5], "and the id slot zeroed");
        let first = map.things.first().expect("the scripted thing");
        assert_eq!(first.tid, 0_i32, "the tid has no Doom home");
        assert_eq!(first.special, 0_i32, "nor does the thing special");
        assert_eq!(first.args, [0_i32; 5], "nor the thing args");
        assert_eq!(
            first.z, 128_i32,
            "the model keeps the height for a UDMF target"
        );
        assert_eq!(
            first.flags, 119_u32,
            "7 read as Hexen bits excludes every mode"
        );
        assert!(
            out.warnings
                .iter()
                .any(|text| text.contains("BEHAVIOR") && text.contains("binary Doom")),
            "the carried BEHAVIOR cannot stay: {:?}",
            out.warnings
        );
        assert_eq!(carried(&map), vec!["SCRIPTS"], "only SCRIPTS rode along");
        assert_eq!(map.format, MapFormat::Doom, "the layout moved");
        assert_eq!(map.udmf_namespace, None, "and no namespace claims Hexen");
        assert_eq!(
            cycle(&map).format,
            MapFormat::Doom,
            "the block reads as Doom"
        );
    }

    #[test]
    fn a_zdoom_udmf_map_converts_back_to_binary_doom() {
        let mut map = udmf_map("zdoom");
        let out = convert_map(&mut map, &binary("doom")).expect("the reverse direction converts");
        assert_eq!(out.translated_specials, 0, "plain lines translate nothing");
        assert!(out.untranslated.is_empty(), "and report nothing");
        assert_eq!(
            out.thing_flags_changed, 2,
            "the flags 7 things exclude every mode"
        );
        assert_eq!(out.sector_types_changed, 0, "the two plain sectors stand");
        assert_eq!(
            out.dropped_fields,
            KindCounts::default(),
            "the fixture model carries no extras"
        );
        assert!(out.warnings.iter().any(|text| text.contains("BEHAVIOR")));
        assert!(
            !out.warnings.iter().any(|text| text.contains("line id")),
            "store_line_ids never fires for a Doom target: {:?}",
            out.warnings
        );
        assert_eq!(carried(&map), Vec::<String>::new(), "every block lump went");
        assert_eq!(map.format, MapFormat::Doom, "the layout moved");
        assert_eq!(
            cycle(&map).format,
            MapFormat::Doom,
            "and reads back as Doom"
        );
    }

    #[test]
    fn a_hexen_map_converts_to_a_doom_family_udmf_namespace() {
        let mut map = hexen_map();
        let out = convert_map(&mut map, &udmf("heretic")).expect("UDMF keeps the scripts");
        assert_eq!(out.translated_specials, 0, "no line needed the table");
        let portal = map.linedefs.get(2).expect("the identified line");
        assert_eq!(portal.special, 0_i32, "the binary fold already ran");
        assert_eq!(
            portal.tag, 7_i32,
            "the id rides on as the Doom family line id"
        );
        assert_eq!(
            portal.args, [0_i32; 5],
            "and the 121 fold never fired again"
        );
        assert_eq!(map.format, MapFormat::Udmf, "the layout moved");
        assert_eq!(
            map.udmf_namespace.as_deref(),
            Some("heretic"),
            "best-effort spelling"
        );
        assert_eq!(
            carried(&map),
            vec!["BEHAVIOR", "SCRIPTS"],
            "the script lumps ride along, the node lumps do not"
        );
        let read_back = cycle(&map);
        assert_eq!(
            read_back.format,
            MapFormat::Udmf,
            "the block reads back as UDMF"
        );
        assert_eq!(
            read_back.udmf_namespace.as_deref(),
            Some("heretic"),
            "and keeps the namespace"
        );
        let folded = read_back.linedefs.get(2).expect("the identified line");
        assert_eq!(folded.special, 0_i32, "the line reads back plain");
        assert_eq!(
            folded.tag, 7_i32,
            "with its id where the Doom family keeps it"
        );
    }

    #[test]
    fn a_set_identification_from_a_udmf_source_is_untranslated() {
        let body = concat!(
            "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
            "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
            "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 7; special = 121; }",
        );
        let mut map = zdoom_udmf(body);
        let out = convert_map(&mut map, &binary("doom")).expect("121 has no row, not a refusal");
        assert_eq!(
            out.untranslated,
            vec![Untranslated {
                linedef: 0,
                special: 121
            }],
            "the literal 121 lands among the unmatched"
        );
        assert_eq!(out.translated_specials, 0, "and nothing translated");
        let line = map.linedefs.first().expect("the line");
        assert_eq!(line.special, 0_i32, "the special clears to 0");
        assert_eq!(line.tag, 7_i32, "the id is kept as the Doom trigger tag");
        assert!(
            out.warnings
                .iter()
                .any(|note| note.contains("no Doom translation")),
            "the grouped miss names it: {:?}",
            out.warnings
        );
        encodes(&map).expect("tag 7 fits a Doom i16 and writes");
    }

    #[test]
    fn a_restored_tag_beyond_the_doom_field_is_the_writer_refusal() {
        let body = concat!(
            "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
            "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
            "linedef { v1 = 0; v2 = 0; sidefront = 0; id = 70000; special = 121; }",
        );
        let mut map = zdoom_udmf(body);
        let out = convert_map(&mut map, &binary("doom")).expect("the conversion does not clamp");
        assert_eq!(out.untranslated.first().expect("one miss").special, 121_i32);
        assert_eq!(map.linedefs.first().expect("the line").tag, 70000_i32);
        let CyberError::LimitExceeded(message) = encodes(&map).expect_err("70000 is no u16") else {
            panic!("expected LimitExceeded for a wide restored tag");
        };
        assert!(
            message.contains("linedef 0 tag 70000"),
            "the writer names the restored tag: {message}"
        );
    }

    #[test]
    fn untranslated_lines_are_listed_up_to_the_limit_then_counted() {
        let mut body = concat!(
            "vertex { x = 0; y = 0; } sidedef { sector = 0; }",
            "sector { texturefloor = \"FLOOR1\"; textureceiling = \"CEIL1\"; }",
        )
        .to_owned();
        let total = UNTRANSLATED_LIMIT.saturating_add(5_usize);
        for _ in 0..total {
            body.push_str("linedef { v1 = 0; v2 = 0; sidefront = 0; special = 560; }");
        }
        let mut map = zdoom_udmf(&body);
        let out = convert_map(&mut map, &binary("doom")).expect("560 translates to nothing");
        assert_eq!(
            out.untranslated.len(),
            UNTRANSLATED_LIMIT,
            "the list is capped at the documented limit"
        );
        assert_eq!(
            out.untranslated.first().expect("one entry").special,
            560_i32,
            "the first lines are named"
        );
        assert!(
            out.warnings
                .iter()
                .any(|note| note.contains("5 further untranslated linedefs are not listed")),
            "the omitted count is warned: {:?}",
            out.warnings
        );
        assert_eq!(out.translated_specials, 0, "nothing made the trip");
    }

    #[test]
    fn cross_family_requests_refuse_nothing() {
        let cases = [
            (two_room_map(), binary("hexen")),
            (two_room_map(), udmf("zdoom")),
            (hexen_map(), binary("doom")),
            (udmf_map("zdoom"), binary("doom")),
            (hexen_map(), udmf("heretic")),
        ];
        for (map, target) in cases {
            let mut moved = map.clone();
            let before = map.clone();
            let out = convert_map(&mut moved, &target)
                .unwrap_or_else(|error| panic!("{target:?} now converts: {error}"));
            assert_ne!(moved, before, "{target:?} changes the map");
            assert_eq!(out.thing_flags_changed, 2, "every fixture thing moves");
        }
    }

    #[test]
    fn a_request_naming_the_layout_in_hand_is_invalid_argument() {
        let same_binary = refusal(&two_room_map(), &binary("doom"));
        assert_eq!(
            same_binary.0, "invalid_argument",
            "doom to doom changes nothing"
        );
        assert!(
            same_binary
                .1
                .contains("MAP01 already is a doom map: nothing to convert"),
            "the map and its layout are named: {}",
            same_binary.1
        );
        let same_hexen = refusal(&hexen_map(), &binary("hexen"));
        assert_eq!(
            same_hexen.0, "invalid_argument",
            "hexen to hexen changes nothing"
        );
        let same_udmf = refusal(&udmf_map("zdoom"), &udmf_default());
        assert_eq!(
            same_udmf.0, "invalid_argument",
            "the family default names the namespace in hand"
        );
        assert!(
            same_udmf.1.contains("udmf (zdoom)"),
            "the namespace is part of the label: {}",
            same_udmf.1
        );
        let spelled = refusal(&udmf_map("zdoom"), &udmf("ZDoom"));
        assert_eq!(
            spelled.0, "invalid_argument",
            "namespaces match case-insensitively"
        );
        let mut moved = udmf_map("heretic");
        convert_map(&mut moved, &udmf_default()).expect("heretic to the family default converts");
        assert_eq!(
            moved.udmf_namespace.as_deref(),
            Some("doom"),
            "a Doom family source rewrites the odd namespace to the family default"
        );
    }

    #[test]
    fn a_refused_conversion_changes_nothing() {
        let mut refused = two_room_map();
        let before = refused.clone();
        let error = convert_map(&mut refused, &binary("doom")).expect_err("the layout in hand");
        assert_eq!(error.code(), "invalid_argument", "same-layout code");
        assert_eq!(refused, before, "a refused conversion changes nothing");
    }

    #[test]
    fn a_namespace_change_inside_one_family_converts_the_declaration_only() {
        let original = udmf_map("zdoom");
        let mut renamed = original.clone();
        let out = convert_map(&mut renamed, &udmf("gzdoom")).expect("zdoom to gzdoom converts");
        assert_eq!(
            out,
            Conversion::default(),
            "UDMF keeps every field it wrote"
        );
        assert_eq!(
            renamed.udmf_namespace.as_deref(),
            Some("gzdoom"),
            "the namespace moved"
        );
        assert_eq!(renamed.format, MapFormat::Udmf, "and the layout stayed");
        assert_eq!(renamed.things, original.things, "the elements stand");
        assert_eq!(renamed.linedefs, original.linedefs, "all of them");
        assert_eq!(
            carried(&renamed),
            vec!["BEHAVIOR"],
            "only the node lumps left"
        );
    }

    #[test]
    fn targets_validate_their_words_and_namespace_shape() {
        assert_eq!(binary("Doom").format, MapFormat::Doom, "any case");
        assert_eq!(
            binary("hexen").namespace,
            None,
            "a binary target declares nothing"
        );
        let target = udmf("GZDoom");
        assert_eq!(target.format, MapFormat::Udmf, "udmf");
        assert_eq!(
            target.namespace.as_deref(),
            Some("gzdoom"),
            "and lowercases"
        );
        for bad in ["zmap", "", "doom2"] {
            let error = Target::parse(bad, None).expect_err("not a format");
            assert_eq!(error.code(), "invalid_argument", "unknown format code");
            assert!(
                error.to_string().contains("\"udmf\""),
                "the message lists the choices: {error}"
            );
        }
        let on_binary =
            Target::parse("hexen", Some("zdoom")).expect_err("a binary format has none");
        assert_eq!(
            on_binary.code(),
            "invalid_argument",
            "a namespace on a binary target is refused"
        );
        assert!(
            on_binary.to_string().contains("only applies to UDMF"),
            "the fix is named: {on_binary}"
        );
        for bad in ["", "has space", &"x".repeat(33)] {
            let error = Target::parse("udmf", Some(bad)).expect_err("not a namespace");
            assert_eq!(error.code(), "invalid_argument", "bad namespace code");
            assert!(
                error.to_string().contains("32 ASCII"),
                "the shape is named: {error}"
            );
        }
    }

    #[test]
    fn families_names_and_node_lumps_are_the_documented_ones() {
        assert_eq!(
            family_of(MapFormat::Doom, Some("zdoom")),
            FlagFamily::Doom,
            "a namespace means nothing to a binary target"
        );
        assert_eq!(
            family_of(MapFormat::Hexen, None),
            FlagFamily::Hexen,
            "binary Hexen"
        );
        assert_eq!(
            family_of(MapFormat::Udmf, Some("heretic")),
            FlagFamily::Doom,
            "heretic is a Doom family namespace"
        );
        assert_eq!(
            family_of(MapFormat::Udmf, None),
            FlagFamily::Hexen,
            "an unset namespace writes as zdoom"
        );
        assert_eq!(format_name(MapFormat::Udmf), "udmf", "the wire spelling");
        for lump_name in ["SEGS", "SSECTORS", "NODES", "REJECT", "BLOCKMAP", "ZNODES"] {
            assert!(
                is_node_lump(lump_name.as_bytes()),
                "{lump_name} is node data"
            );
        }
        for lump_name in ["THINGS", "BEHAVIOR", "SCRIPTS", "TEXTMAP", "ENDMAP"] {
            assert!(!is_node_lump(lump_name.as_bytes()), "{lump_name} is not");
        }
    }
}
