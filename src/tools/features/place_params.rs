//! The `place_things` wire surface: its parameter shapes and the
//! map-free half of resolving them into one [`PlaceRequest`].
//!
//! Everything here parses and checks the call's `JSON` - the thing (a
//! doomednum or a definition name), the exactly-one area (`sectors`,
//! `bbox` or `near`), the exactly-one demand (`count` or `per_sector`),
//! the clearance, the skills, the presence flags and the snapped facing,
//! and refuses with the instructions the tool documents. The map-backed
//! half, the distribution run, lives in [`super::place`].
//!
//! [`place_on`]: super::place::place_on

use schemars::JsonSchema;
use serde::Deserialize;

use crate::defs::Definitions;
use crate::error::CyberError;
use crate::named::parse_named;
use crate::plan::things::resolve_thing_kind;
use crate::wad::map::flags::{self, FlagFamily, FlagKind, Skill};
use crate::wad::map::placement::{Area, Facing, snap_angle};

use super::specials::sector_selection;

/// The `place_things` count ceiling per call.
const COPIES_MAX: u32 = 256_u32;

/// The blocking radius `place_things` assumes for a thing type the
/// definitions do not list, map units: the shared
/// [`UNKNOWN_THING_RADIUS`](crate::wad::map::consts::UNKNOWN_THING_RADIUS).
pub(super) use crate::wad::map::consts::UNKNOWN_THING_RADIUS as PLACE_UNKNOWN_RADIUS;

/// The all-zero script argument list of a thing the call dressed.
const PLACE_ZERO_ARGS: [i32; 5] = [0_i32; 5];

/// The `next_steps` every `place_things` result carries.
pub(super) const PLACE_NEXT_STEPS: [&str; 1] =
    ["check_playability (stuck_things) and render_topdown"];

/// Arguments for [`Cyberdemon::place_things`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct PlaceThingsParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    pub(super) map: Option<String>,
    /// What to place: a doomednum or a definition name resolved against
    /// the session's definitions (an exact case-insensitive name, else a
    /// unique substring; ambiguity is `invalid_argument` listing the
    /// candidates).
    pub(super) thing: ThingKindParam,
    /// The sectors to fill, as a list of indices (duplicates collapse):
    /// exactly one of `sectors`, `bbox` and `near`.
    #[serde(default)]
    pub(super) sectors: Option<Vec<usize>>,
    /// The box to fill, as `[x0, y0, x1, y1]` in map units: exactly one
    /// of `sectors`, `bbox` and `near`.
    #[serde(default)]
    pub(super) bbox: Option<[i32; 4]>,
    /// The circle to fill, `{x, y, radius}` in map units: exactly one of
    /// `sectors`, `bbox` and `near`.
    #[serde(default)]
    pub(super) near: Option<NearParam>,
    /// How many things to place in total, `1..=256`, split across the
    /// sectors by area: exactly one of `count` and `per_sector`.
    #[serde(default)]
    pub(super) count: Option<u32>,
    /// How many things to place in every area: exactly one of `count`
    /// and `per_sector`.
    #[serde(default)]
    pub(super) per_sector: Option<u32>,
    /// The distance every placed thing keeps from walls and other things,
    /// positive map units (default: the thing's definition radius, else
    /// 20, plus 8).
    #[serde(default)]
    pub(super) clearance: Option<i32>,
    /// Skills the things appear on: some of `"easy"`, `"medium"`,
    /// `"hard"` (default all three).
    #[serde(default)]
    pub(super) skills: Option<Vec<String>>,
    /// Confine the things to the multiplayer modes (default false).
    #[serde(default)]
    pub(super) multiplayer_only: bool,
    /// Spawn the things deaf, ambushing whatever they can see
    /// (default false).
    #[serde(default)]
    pub(super) ambush: bool,
    /// The facing: a number of degrees, or `"face_player_start"`,
    /// `"face_center"` or `"random_seeded"` (default 0). Angles snap to
    /// multiples of 45.
    #[serde(default)]
    pub(super) angle: Option<AngleParam>,
    /// The seed of `angle: "random_seeded"` (default 1); the only use of
    /// randomness, deterministic from it.
    #[serde(default = "default_place_seed")]
    pub(super) seed: u32,
    /// Thing id copied onto every placed thing (`Hexen` family only).
    #[serde(default)]
    pub(super) tid: Option<i32>,
    /// Thing special copied onto every placed thing, `{number, args}`
    /// (`Hexen` family only).
    #[serde(default)]
    pub(super) special: Option<ThingSpecialParam>,
    /// Compute and report the placement without writing anything
    /// (default false).
    #[serde(default)]
    pub(super) dry_run: WireBool,
}

/// A boolean wire parameter kept out of the parameter struct's bool
/// budget: it deserializes as a plain `true`/`false` `JSON` boolean.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[serde(transparent)]
pub(super) struct WireBool {
    /// The flag as the client sent it.
    flag: bool,
}

impl WireBool {
    /// The wrapped state.
    pub(super) const fn get(self) -> bool {
        self.flag
    }
}

/// Default [`PlaceThingsParams::seed`].
const fn default_place_seed() -> u32 {
    1_u32
}

/// The `thing` parameter: a doomednum or a definition name.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(super) enum ThingKindParam {
    /// The thing type number to place.
    Type(
        /// The doomednum.
        u32,
    ),
    /// A definition name to resolve.
    Name(
        /// The name to look up.
        String,
    ),
}

/// The `angle` parameter: degrees or a facing mode.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(super) enum AngleParam {
    /// A fixed facing in degrees (snapped to a multiple of 45).
    Degrees(
        /// The facing in degrees.
        i32,
    ),
    /// One of `face_player_start`, `face_center`, `random_seeded`.
    Mode(
        /// The mode name.
        String,
    ),
}

/// The `near` circle parameter.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
pub(super) struct NearParam {
    /// The centre `X` in map units.
    pub(super) x: i32,
    /// The centre `Y` in map units.
    pub(super) y: i32,
    /// The radius in map units (positive).
    pub(super) radius: i32,
}

/// The `special` parameter: the thing action and its five arguments.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub(super) struct ThingSpecialParam {
    /// The thing special number.
    pub(super) number: i32,
    /// The five script arguments (default zeros).
    #[serde(default)]
    pub(super) args: Option<[i32; 5]>,
}

/// Where a resolved [`place_on`] call looks: exactly one wire shape.
#[derive(Clone, Debug)]
pub(super) enum WhereSpec {
    /// Whole sectors, ascending and deduplicated.
    Sectors(Vec<usize>),
    /// One open shape: a box or a circle.
    Area(Area),
}

/// How many things a resolved [`place_on`] call demands.
#[derive(Clone, Copy, Debug)]
pub(super) enum NeedSpec {
    /// A total, split across the areas by bounding-box area.
    Total(usize),
    /// This many in every area.
    PerSector(usize),
}

/// The `place_things` wire parameters resolved into one request.
#[derive(Debug)]
pub(super) struct PlaceRequest {
    /// The doomednum to place.
    pub(super) kind: u32,
    /// The one area kind the call named.
    pub(super) target: WhereSpec,
    /// How the count is demanded.
    pub(super) demand: NeedSpec,
    /// The explicit clearance in map units (`None`: radius plus 8).
    pub(super) clearance: Option<i64>,
    /// The named skills (`None`: every skill).
    pub(super) skills: Option<Vec<Skill>>,
    /// Whether the things are confined to the multiplayer modes.
    pub(super) multiplayer_only: bool,
    /// Whether the things spawn deaf.
    pub(super) ambush: bool,
    /// How the facings are chosen.
    pub(super) facing: Facing,
    /// The script tid copied onto each thing.
    pub(super) tid: i32,
    /// The script special copied onto each thing.
    pub(super) special: i32,
    /// The five script arguments copied onto each thing.
    pub(super) args: [i32; 5],
}

/// Resolve the wire parameters into a [`PlaceRequest`] (everything that
/// does not need the map).
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an unresolvable name, none or
/// several of the area parameters, neither or both of the count
/// parameters, a count outside `1..=256`, a non-positive `clearance`, an
/// empty `sectors` list, a backwards `bbox`, a non-positive `near`
/// radius, an unknown skill or angle mode.
pub(super) fn place_request(
    args: &PlaceThingsParams,
    definitions: &Definitions,
) -> Result<PlaceRequest, CyberError> {
    let kind = place_kind(&args.thing, definitions)?;
    Ok(PlaceRequest {
        kind,
        target: where_spec(args)?,
        demand: need_spec(args)?,
        clearance: clearance_of(args.clearance)?,
        skills: args.skills.as_deref().map(place_skills).transpose()?,
        multiplayer_only: args.multiplayer_only,
        ambush: args.ambush,
        facing: facing_of(args.angle.as_ref(), args.seed)?,
        tid: args.tid.unwrap_or_default(),
        special: args.special.as_ref().map_or(0_i32, |row| row.number),
        args: args
            .special
            .as_ref()
            .and_then(|row| row.args)
            .unwrap_or(PLACE_ZERO_ARGS),
    })
}

/// The doomednum to place: the number as given, the name resolved
/// against `definitions` (its refusal an `invalid_argument`).
pub(super) fn place_kind(
    param: &ThingKindParam,
    definitions: &Definitions,
) -> Result<u32, CyberError> {
    match param.clone() {
        ThingKindParam::Type(number) => Ok(number),
        ThingKindParam::Name(text) => resolve_thing_kind(definitions, &text).map_err(|problem| {
            CyberError::invalid(&problem).with_hint("get_thing_types with a search term")
        }),
    }
}

/// The one area parameter the call named.
fn where_spec(args: &PlaceThingsParams) -> Result<WhereSpec, CyberError> {
    match (args.sectors.clone(), args.bbox, args.near) {
        (Some(list), None, None) => Ok(WhereSpec::Sectors(sector_selection(&list)?)),
        (None, Some(edges), None) => Ok(WhereSpec::Area(rect_area(edges)?)),
        (None, None, Some(around)) => Ok(WhereSpec::Area(circle_area(around)?)),
        _ => Err(CyberError::invalid(
            "pass exactly one of sectors, bbox and near: the tool places into one kind of area",
        )),
    }
}

/// The `bbox` parameter as a [`Area::Rect`]; a backwards box is refused.
fn rect_area(edges: [i32; 4]) -> Result<Area, CyberError> {
    let [x0, y0, x1, y1] = edges;
    if x0 >= x1 || y0 >= y1 {
        return Err(CyberError::invalid(&format!(
            "bbox [{x0}, {y0}, {x1}, {y1}] is empty or backwards: pass [x0, y0, x1, y1] with \
             x0 < x1 and y0 < y1"
        )));
    }
    Ok(Area::Rect {
        min_x: i64::from(x0),
        min_y: i64::from(y0),
        max_x: i64::from(x1),
        max_y: i64::from(y1),
    })
}

/// The `near` parameter as a [`Area::Circle`]; a zero or negative radius
/// is refused.
fn circle_area(around: NearParam) -> Result<Area, CyberError> {
    if around.radius < 1_i32 {
        let radius = around.radius;
        return Err(CyberError::invalid(&format!(
            "near radius {radius} is not a circle: pass a positive radius in map units"
        )));
    }
    Ok(Area::Circle {
        centre: (i64::from(around.x), i64::from(around.y)),
        radius: i64::from(around.radius),
    })
}

/// The one demand parameter the call named, its count checked.
pub(super) fn need_spec(args: &PlaceThingsParams) -> Result<NeedSpec, CyberError> {
    match (args.count, args.per_sector) {
        (Some(total), None) => Ok(NeedSpec::Total(check_copies("count", total)?)),
        (None, Some(each)) => Ok(NeedSpec::PerSector(check_copies("per_sector", each)?)),
        (Some(_), Some(_)) => Err(CyberError::invalid(
            "pass one of count and per_sector, not both: count splits across the areas, \
             per_sector fills every one of them",
        )),
        (None, None) => Err(CyberError::invalid(
            "pass count (1..=256 things in total) or per_sector (that many in every area)",
        )),
    }
}

/// A requested copy count inside `1..=256`, as a `usize`.
fn check_copies(name: &str, value: u32) -> Result<usize, CyberError> {
    if value == 0_u32 || value > COPIES_MAX {
        return Err(CyberError::invalid(&format!(
            "{name} {value} is out of range: pass 1..={COPIES_MAX}"
        )));
    }
    usize::try_from(value).map_err(|conversion| {
        CyberError::invalid(&format!(
            "{name} {value} exceeds the supported count: {conversion}"
        ))
    })
}

/// The explicit `clearance`, positive; `None` means the thing's radius
/// plus the shared margin.
fn clearance_of(given: Option<i32>) -> Result<Option<i64>, CyberError> {
    let Some(value) = given else {
        return Ok(None);
    };
    if value < 1_i32 {
        return Err(CyberError::invalid(&format!(
            "clearance {value} is out of range: pass a positive distance in map units, or leave \
             it out for the thing's radius plus 8"
        )));
    }
    Ok(Some(i64::from(value)))
}

/// The `skills` list parsed to [`Skill`]s; an empty list would hide the
/// things on no skill at all, so it is refused.
fn place_skills(texts: &[String]) -> Result<Vec<Skill>, CyberError> {
    if texts.is_empty() {
        return Err(CyberError::invalid(
            "skills is empty: pass some of easy, medium and hard, or leave skills out for every \
             skill",
        ));
    }
    texts
        .iter()
        .map(|text| parse_named::<Skill>("skill", text))
        .collect()
}

/// The `angle` parameter resolved into a [`Facing`] (a fixed angle
/// already snapped).
pub(super) fn facing_of(param: Option<&AngleParam>, seed: u32) -> Result<Facing, CyberError> {
    match param.cloned() {
        None => Ok(Facing::Fixed(0_i32)),
        Some(AngleParam::Degrees(degrees)) => Ok(Facing::Fixed(snap_angle(degrees))),
        Some(AngleParam::Mode(text)) => match text.to_ascii_lowercase().as_str() {
            "face_player_start" => Ok(Facing::PlayerStart),
            "face_center" => Ok(Facing::Centre),
            "random_seeded" => Ok(Facing::Seeded(seed)),
            other => Err(CyberError::invalid(&format!(
                "angle {other:?} is not an angle: pass a number of degrees or one of \
                 face_player_start, face_center, random_seeded"
            ))),
        },
    }
}

/// The `Thing` flag bit `wanted` names in `family`' table, `0` when none.
pub(super) fn thing_bit(family: FlagFamily, wanted: &str) -> u32 {
    flags::names(FlagKind::Thing, family)
        .iter()
        .find(|entry| entry.0 == wanted)
        .map_or(0_u32, |entry| entry.1)
}
