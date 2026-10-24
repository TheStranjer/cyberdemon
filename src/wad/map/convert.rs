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
//!
//! The [`Target`] and [`Conversion`] shapes the tool reports, the
//! [`convert_map`] entry point with its two [`super::xlat`] drivers and the
//! [`KindCounts`] and layout names every result is built from stay here;
//! resolving a request against the map it names is the private `plan`
//! module, and rewriting a map into the layout that request chose, with the
//! node lumps, `BEHAVIOR` lump and unmodellable `UDMF` fields it cannot
//! keep, is the private `layout` one.
//!
//! [`FlagFamily`]: crate::wad::map::flags::FlagFamily
//! [`Linedef::tag`]: crate::wad::map::model::Linedef::tag

mod layout;
mod plan;

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::model::Map;
use crate::wad::map::write_map;
use crate::wad::map::xlat::{self, FieldReport, LinesReport, Untranslated};
use layout::{into_binary, into_udmf, namespace_name, parse_format, strip_node_lumps};
use plan::{Direction, Plan};

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
    /// The [`Thing`](crate::wad::map::model::Thing) count.
    pub things: usize,
    /// The [`Linedef`](crate::wad::map::model::Linedef) count.
    pub linedefs: usize,
    /// The [`Sidedef`](crate::wad::map::model::Sidedef) count.
    pub sidedefs: usize,
    /// The [`Sector`](crate::wad::map::model::Sector) count.
    pub sectors: usize,
    /// The [`Vertex`](crate::wad::map::model::Vertex) count.
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

#[cfg(test)]
mod tests;
