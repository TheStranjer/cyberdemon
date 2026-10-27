//! Plan things: name resolution, in-room grid placement and presence
//! flags (pure).
//!
//! The compiler emits geometry; this module turns validated wire thing
//! rows into [`Thing`] records for the compiled [`Map`].
//!
//! Names resolve against the session [`Definitions`]: a case-insensitive
//! exact [`crate::defs::ThingDef::name`] match wins, else a
//! case-insensitive substring must identify exactly one distinct thing
//! type, with the ambiguous candidates listed for the caller to choose
//! between. Pinned plan points travel through the same [`Units`]
//! transform (and snap grid) the rooms did. Room placement fills a
//! sector deterministically: row-major on a grid spaced by twice the
//! thing's clearance, starting at the sector bounding box's minimum
//! corner raised by the clearance, keeping every point at least that
//! clearance inside the sector through the same containment and boundary
//! rules [`crate::wad::map::placement`] gives the `place_things` tool's
//! distribution — but read from one [`SectorIndex`] a [`place`] run
//! builds once for the compiled map, so a fill of a thousand grid points
//! never rescans the linedefs. [`presence_flags`]
//! folds the `skills` and `multiplayer_only` parameters into the `Thing`
//! flag bits of the map's flag family, reading the tables in
//! [`crate::wad::map::flags`]. Nothing here touches archives, sessions
//! or `MCP` types.

use alloc::collections::BTreeMap;

use crate::defs::Definitions;
use crate::error::CyberError;
use crate::fixed::midpoint;
use crate::geom::Pt;
use crate::plan::units::{Units, range_error};
/// A difficulty skill a planned thing can be limited to: the flag tables'
/// own [`Skill`], re-exported where the plan writers read it.
pub use crate::wad::map::flags::Skill;
use crate::wad::map::flags::{FlagFamily, FlagKind, names as flag_names};
use crate::wad::map::geometry::SectorIndex;
use crate::wad::map::placement::{self, DEFAULT_RADIUS, axis_points};
use crate::wad::map::{Map, Thing};

/// The thing type of the player 1 start, shared from
/// [`consts`](crate::wad::map::consts) in the plan's `u32` shape.
pub use crate::wad::map::consts::PLAYER1_START_U32 as PLAYER_ONE_START;

/// Where one planned thing goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThingSpot {
    /// A pinned plan-space point, transformed with the rooms.
    Point(Pt),
    /// A room to fill on the clearance grid, up to `count` copies.
    Room {
        /// The plan id of the room to fill.
        id: String,
    },
    /// The bounding-box centre of a room's sector, exactly one thing.
    RoomCentre {
        /// The plan id of the room to centre in.
        id: String,
    },
}

/// One resolved wire row: doomednum, spot and flags, ready for
/// [`place`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedThing {
    /// The thing type number to write.
    pub kind: u32,
    /// Where the copies go.
    pub spot: ThingSpot,
    /// How many copies a [`ThingSpot::Room`] fill may place; other spots
    /// hold exactly one thing.
    pub count: u32,
    /// Facing in degrees, validated to `0..=359` by the wire layer.
    pub angle: i32,
    /// Presence flag bits for the map's flag family.
    pub flags: u32,
    /// The wire path (`things[0]`, `player_start`) error messages name.
    pub path: String,
}

/// The compiled geometry and reference tables one [`place`] run reads.
pub struct PlacementContext<'ctx> {
    /// The compiled map, before any planned thing was appended.
    pub map: &'ctx Map,
    /// Room id to the sectors that room owns, from the compiled plan.
    pub room_sectors: &'ctx BTreeMap<String, Vec<usize>>,
    /// The session definitions: name resolution and blocking radii.
    pub definitions: &'ctx Definitions,
    /// The transform pinned points travel with the rooms.
    pub units: Units,
    /// The snap grid the transform applies, map units.
    pub grid: i64,
}

/// What a [`place`] run produced: the rows to append and the notes.
#[derive(Debug, Default)]
pub struct Placement {
    /// The [`Thing`] records, in request order; room fills stay
    /// row-major within their row.
    pub things: Vec<Thing>,
    /// One note per room whose grid held fewer points than requested.
    pub warnings: Vec<String>,
}

/// The doomednum of a thing `name` in `definitions`: an exact
/// (case-insensitive) name match wins, else a case-insensitive
/// substring must identify exactly one distinct thing type.
///
/// # Errors
///
/// A `String` refusal naming the search: no thing carries that name, or
/// an ambiguous substring matched several distinct types and its
/// candidate names (sorted, deduplicated by type) follow.
#[inline]
pub fn resolve_thing_kind(definitions: &Definitions, name: &str) -> Result<u32, String> {
    if let Some(row) = definitions.thing_by_name(name) {
        return Ok(row.kind);
    }
    let matched = substring_matches(definitions, &name.to_ascii_lowercase());
    match (matched.first(), matched.get(1)) {
        (None, _) => Err(format!(
            "no thing named '{name}' in the session's definitions; check get_thing_types for the \
             names or pass a doomednum via type"
        )),
        (Some(entry), None) => Ok(entry.1),
        (Some(_), Some(_)) => Err(format!(
            "name '{name}' is ambiguous: it matches {}; give the exact name or pass a doomednum \
             via type",
            candidate_names(&matched)
        )),
    }
}

/// The rows whose name contains the already-lowered `needle`, one per
/// distinct thing type in table order.
fn substring_matches(definitions: &Definitions, needle: &str) -> Vec<(String, u32)> {
    let mut found: Vec<(String, u32)> = Vec::new();
    for row in &definitions.things {
        let distinct = !found.iter().any(|entry| entry.1 == row.kind);
        if distinct && row.name.to_ascii_lowercase().contains(needle) {
            found.push((row.name.clone(), row.kind));
        }
    }
    found
}

/// The matched candidate names, sorted, as one comma-separated list.
fn candidate_names(matched: &[(String, u32)]) -> String {
    let mut texts: Vec<&str> = matched.iter().map(|entry| entry.0.as_str()).collect();
    texts.sort_unstable();
    texts.join(", ")
}

/// The bit `wanted` holds in the family's `Thing` flag table, `0` when
/// the table carries no such name.
fn thing_bit(family: FlagFamily, wanted: &str) -> u32 {
    flag_names(FlagKind::Thing, family)
        .iter()
        .find(|entry| entry.0 == wanted)
        .map_or(0_u32, |entry| entry.1)
}

/// The presence flag bits of a planned thing: the named `skills`, all
/// three when unset.
///
/// With `multiplayer_only` the `Doom` family gains its `notsingle` bit
/// and the `Hexen` family the clearing equivalent for `single`: the
/// `coop` and `dm` mode bits, which confine the thing to the
/// multiplayer modes. No mode bits at all means present in every game
/// mode.
#[must_use]
#[inline]
pub fn presence_flags(family: FlagFamily, skills: Option<&[Skill]>, multiplayer_only: bool) -> u32 {
    let wanted = skills.unwrap_or(Skill::ALL.as_slice());
    let mut bits = wanted
        .iter()
        .fold(0_u32, |acc, skill| acc | skill.bit(family));
    if multiplayer_only {
        match family {
            FlagFamily::Doom => bits |= thing_bit(family, "notsingle"),
            FlagFamily::Hexen => bits |= thing_bit(family, "coop") | thing_bit(family, "dm"),
        }
    }
    bits
}

/// Turn resolved rows into [`Thing`] records for `context`'s compiled
/// map. The whole run reads the map through one [`SectorIndex`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the row's wire path for an
/// unknown room id, a pinned point the plan transform cannot carry or a
/// room whose sector bounds nothing; [`CyberError::LimitExceeded`] for
/// coordinates or a thing type the map formats cannot store.
#[inline]
pub fn place(
    context: &PlacementContext<'_>,
    specs: &[PlannedThing],
) -> Result<Placement, CyberError> {
    let mut pass = Pass::new(context);
    pass.run(specs)?;
    Ok(pass.out)
}

/// The shared state of one [`place`] run: the caller's context, the
/// compiled map indexed once for the whole pass, and the rows gathered
/// so far. Every box, containment and boundary read of the pass answers
/// from that stored [`SectorIndex`], never a fresh scan per point.
struct Pass<'ctx> {
    /// The compiled map and reference tables the rows are read against.
    context: &'ctx PlacementContext<'ctx>,
    /// The compiled map's per-sector geometry, built once for the run.
    geometry: SectorIndex,
    /// The rows and notes accumulated so far.
    out: Placement,
}

impl<'ctx> Pass<'ctx> {
    /// Index the context's map once and open an empty placement.
    fn new(context: &'ctx PlacementContext<'ctx>) -> Self {
        Self {
            context,
            geometry: SectorIndex::new(context.map),
            out: Placement::default(),
        }
    }

    /// Place every resolved row, in order; the first refusal stops the
    /// run with its rows and notes dropped.
    fn run(&mut self, specs: &[PlannedThing]) -> Result<(), CyberError> {
        for spec in specs {
            self.place_row(spec)?;
        }
        Ok(())
    }

    /// Place one resolved row: its pinned point, its room fill or the
    /// room centre, appending records and warnings.
    fn place_row(&mut self, spec: &PlannedThing) -> Result<(), CyberError> {
        match spec.spot.clone() {
            ThingSpot::Point(point) => {
                let mapped = transform_point(self.context, spec, point)?;
                self.out.things.push(thing_row(spec, mapped)?);
                Ok(())
            }
            ThingSpot::Room { id } => self.room_fill(spec, &id),
            ThingSpot::RoomCentre { id } => {
                let centre = self.room_centre(spec, &id)?;
                self.out.things.push(thing_row(spec, centre)?);
                Ok(())
            }
        }
    }

    /// The first sector the plan room `id` owns, refused with the row's
    /// `path` when the compiled plan holds nothing for it.
    fn room_sector(&self, path: &str, id: &str) -> Result<usize, CyberError> {
        self.context
            .room_sectors
            .get(id)
            .and_then(|sectors| sectors.first().copied())
            .ok_or_else(|| {
                CyberError::InvalidArgument(format!(
                    "{path}: room '{id}' is not a room of the compiled plan (or it was painted \
                     over entirely); check the rooms list"
                ))
            })
    }

    /// The bounding-box centre of the room's sector, map units, the box
    /// read from the run's index.
    fn room_centre(&self, spec: &PlannedThing, id: &str) -> Result<Pt, CyberError> {
        let path = format!("{}.room", spec.path);
        let sector = self.room_sector(&path, id)?;
        let bbox = self.geometry.bbox(sector).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "{path}: room '{id}' bounds nothing, so it has no centre to place at"
            ))
        })?;
        let (min_x, min_y, max_x, max_y) = bbox;
        Ok((
            midpoint(i64::from(min_x), i64::from(max_x)),
            midpoint(i64::from(min_y), i64::from(max_y)),
        ))
    }

    /// Fill one room row-major, append what fits and note what did not.
    fn room_fill(&mut self, spec: &PlannedThing, id: &str) -> Result<(), CyberError> {
        let path = format!("{}.room", spec.path);
        let sector = self.room_sector(&path, id)?;
        let clearance = thing_clearance(self.context.definitions, spec.kind);
        let wanted = usize::try_from(spec.count).unwrap_or(usize::MAX);
        let slots = self.slots(sector, clearance, wanted);
        if slots.len() < wanted {
            self.out.warnings.push(format!(
                "room '{id}' fits only {} of {} things",
                slots.len(),
                spec.count
            ));
        }
        for point in slots {
            self.out.things.push(thing_row(spec, point)?);
        }
        Ok(())
    }

    /// The first `wanted` grid points that [`Self::fits`]: row-major
    /// (the `x` axis runs fastest), from the sector bounding box's
    /// minimum corner raised by `clearance`, twice the clearance apart.
    /// The box comes from the run's index.
    fn slots(&self, sector: usize, clearance: i64, wanted: usize) -> Vec<Pt> {
        let Some(bbox) = self.geometry.bbox(sector) else {
            return Vec::new();
        };
        let (min_x, min_y, max_x, max_y) = bbox;
        let step = clearance.saturating_mul(2_i64);
        let rows = axis_points(
            i64::from(min_y).saturating_add(clearance),
            i64::from(max_y),
            step,
        );
        let columns = axis_points(
            i64::from(min_x).saturating_add(clearance),
            i64::from(max_x),
            step,
        );
        rows.iter()
            .flat_map(|&y| columns.iter().map(move |&x| (x, y)))
            .filter(|at| self.fits(sector, *at, clearance))
            .take(wanted)
            .collect()
    }

    /// [`placement::fits`]' answer read from the run's one index: the
    /// same containment winner and the same boundary rows the single
    /// helper folds for its one point, shared across the whole fill.
    fn fits(&self, sector: usize, at: Pt, clearance: i64) -> bool {
        let (x, y) = at;
        self.geometry.point_in_sector(x, y) == Some(sector)
            && self
                .geometry
                .boundary(sector)
                .iter()
                .all(|&(line, _)| placement::line_clear(self.context.map, line, at, clearance))
    }
}

/// A pinned plan point through the room transform and snap grid.
fn transform_point(
    context: &PlacementContext<'_>,
    spec: &PlannedThing,
    point: Pt,
) -> Result<Pt, CyberError> {
    let (x, y) = point;
    context
        .units
        .point(context.grid, x, y)
        .ok_or_else(|| range_error(&format!("{}: x/y", spec.path)))
}

/// The wall clearance of thing type `kind`: its definition's blocking
/// radius (or [`DEFAULT_RADIUS`] when the definitions do not list the
/// type) plus the shared clearance margin.
fn thing_clearance(definitions: &Definitions, kind: u32) -> i64 {
    placement::thing_clearance(definitions, kind, DEFAULT_RADIUS)
}

/// The [`Thing`] record for one copy of `spec` standing at map `point`.
fn thing_row(spec: &PlannedThing, point: Pt) -> Result<Thing, CyberError> {
    let Ok(kind) = i32::try_from(spec.kind) else {
        return Err(CyberError::LimitExceeded(format!(
            "{}: type {} is above the storable thing-type range; use a doomednum the game defines",
            spec.path, spec.kind
        )));
    };
    let Ok(map_x) = i32::try_from(point.0) else {
        return Err(axis_refusal(spec, "x", point.0));
    };
    let Ok(map_y) = i32::try_from(point.1) else {
        return Err(axis_refusal(spec, "y", point.1));
    };
    Ok(Thing {
        x: map_x,
        y: map_y,
        angle: spec.angle,
        kind,
        flags: spec.flags,
        ..Default::default()
    })
}

/// The `LimitExceeded` for one plan coordinate the map format cannot
/// store.
fn axis_refusal(spec: &PlannedThing, axis: &str, value: i64) -> CyberError {
    CyberError::LimitExceeded(format!(
        "{}: {axis} {value} is outside the 32-bit map coordinate range; shrink the scale or move \
         the point",
        spec.path
    ))
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use super::{
        PLAYER_ONE_START, Pass, PlacementContext, PlannedThing, Skill, ThingSpot, candidate_names,
        place, presence_flags, resolve_thing_kind, substring_matches, thing_clearance,
    };
    use crate::defs::registry::DefinitionTables;
    use crate::defs::table::ThingDef;
    use crate::defs::{Category, Definitions, definitions_for};
    use crate::geom::Pt;
    use crate::plan::Units;
    use crate::wad::game::Game;
    use crate::wad::map::flags::{FlagFamily, FlagKind, to_names};
    use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Thing, Vertex};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn map_name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A map holding `squares` as separate one-sector rooms, each
    /// `(min, max)`-cornered box bounded by four blocking lines.
    fn square_rooms(squares: &[(i32, i32)]) -> Map {
        let mut map = Map::new_empty(map_name("MAP01"), MapFormat::Doom);
        for square in squares {
            add_square(&mut map, square.0, square.1);
        }
        map
    }

    /// Append one square sector from (`min`, `min`) to (`max`, `max`).
    fn add_square(map: &mut Map, min: i32, max: i32) {
        let sector = map.sectors.len();
        let base = map.vertices.len();
        map.sectors.push(Sector::default());
        let corners = [(min, min), (max, min), (max, max), (min, max)];
        for corner in corners {
            map.vertices.push(Vertex {
                x: corner.0,
                y: corner.1,
                extra: BTreeMap::new(),
            });
        }
        let sides: Vec<usize> = (0_usize..4_usize)
            .map(|_| {
                let side = map.sidedefs.len();
                map.sidedefs.push(Sidedef {
                    sector,
                    ..Default::default()
                });
                side
            })
            .collect();
        let edges = [(0_usize, 1_usize), (1, 2), (2, 3), (3, 0)];
        for edge in edges {
            map.linedefs.push(Linedef {
                v1: base.saturating_add(edge.0),
                v2: base.saturating_add(edge.1),
                flags: 1_u32,
                front: sides.get(edge.0).copied(),
                ..Default::default()
            });
        }
    }

    /// The session's DOOM definitions for the name and radius tests.
    fn doom_definitions() -> Definitions {
        definitions_for(Game::Doom).expect("the embedded tables parse")
    }

    /// A [`ThingDef`] row named `name` with type `kind`.
    fn thing_def(kind: u32, name: &str) -> ThingDef {
        ThingDef {
            kind,
            name: name.to_owned(),
            sprite: String::new(),
            frame: 'A',
            radius: 16_u32,
            category: String::new(),
            group: Category::Other,
            hanging: false,
            game: "doom".to_owned(),
            source: "test".to_owned(),
        }
    }

    /// A resolved row filling room `id` with `count` copies.
    fn room_row(kind: u32, id: &str, count: u32) -> PlannedThing {
        PlannedThing {
            kind,
            spot: ThingSpot::Room { id: id.to_owned() },
            count,
            angle: 90_i32,
            flags: 7_u32,
            path: "things[0]".to_owned(),
        }
    }

    /// The room bookkeeping the placement tests share: room `"a"` owns
    /// sector `0`.
    fn rooms_a() -> BTreeMap<String, Vec<usize>> {
        let mut room_sectors: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        room_sectors.insert("a".to_owned(), vec![0_usize]);
        room_sectors
    }

    /// A placement context at the identity transform.
    fn context<'ctx>(
        map: &'ctx Map,
        room_sectors: &'ctx BTreeMap<String, Vec<usize>>,
        definitions: &'ctx Definitions,
    ) -> PlacementContext<'ctx> {
        PlacementContext {
            map,
            room_sectors,
            definitions,
            units: Units::default(),
            grid: 1_i64,
        }
    }

    /// [`Pass::slots`] over a throwaway context: the grid reads only the
    /// map and the pass's one index.
    fn slots(map: &Map, sector: usize, clearance: i64, wanted: usize) -> Vec<Pt> {
        let definitions = doom_definitions();
        let rooms: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        let context = context(map, &rooms, &definitions);
        Pass::new(&context).slots(sector, clearance, wanted)
    }

    #[test]
    fn an_exact_name_wins_over_the_substrings_it_is_contained_in() {
        let definitions = doom_definitions();
        assert_eq!(
            resolve_thing_kind(&definitions, "Imp"),
            Ok(3001_u32),
            "exact, case-insensitive"
        );
        assert_eq!(
            resolve_thing_kind(&definitions, "PLAYER 1 START"),
            Ok(1_u32),
            "exact beats the ambiguous 'start' substring"
        );
        assert_eq!(
            resolve_thing_kind(&definitions, "barrel"),
            Ok(2035_u32),
            "'Barrel' matches exactly although 'Burning Barrel' contains it"
        );
    }

    #[test]
    fn a_substring_naming_one_type_resolves_to_its_doomednum() {
        let definitions = doom_definitions();
        assert_eq!(
            resolve_thing_kind(&definitions, "barre"),
            Ok(2035_u32),
            "only 'Barrel' contains 'barre'"
        );
    }

    #[test]
    fn an_ambiguous_substring_lists_the_candidates_and_advises() {
        let definitions = doom_definitions();
        let problem =
            resolve_thing_kind(&definitions, "skul").expect_err("'skul' matches several things");
        assert!(problem.contains("ambiguous"), "{problem}");
        assert!(problem.contains("Skull on Pole"), "{problem}");
        assert!(problem.contains("Pile of Skulls"), "{problem}");
        assert!(problem.contains("doomednum"), "the refusal instructs");
    }

    #[test]
    fn an_unknown_name_is_refused_naming_the_thing() {
        let definitions = doom_definitions();
        let problem = resolve_thing_kind(&definitions, "platypus").expect_err("no such thing");
        assert!(problem.contains("no thing named 'platypus'"), "{problem}");
    }

    #[test]
    fn substring_candidates_dedupe_by_type_and_sort_their_names() {
        let definitions = Definitions::new(
            Game::Doom2,
            DefinitionTables {
                things: vec![
                    thing_def(50, "zeta bot"),
                    thing_def(51, "alpha bot"),
                    thing_def(51, "alpha bot"),
                ],
                ..DefinitionTables::default()
            },
        );
        let matched = substring_matches(&definitions, "bot");
        assert_eq!(
            matched.iter().map(|entry| entry.1).collect::<Vec<u32>>(),
            vec![50_u32, 51_u32],
            "one entry per distinct type, in table order"
        );
        assert_eq!(candidate_names(&matched), "alpha bot, zeta bot", "sorted");
        assert_eq!(
            resolve_thing_kind(&definitions, "alpha bot"),
            Ok(51_u32),
            "exact again before any substring work"
        );
    }

    #[test]
    fn presence_flags_default_to_all_skills_and_extend_per_family() {
        assert_eq!(
            presence_flags(FlagFamily::Doom, None, false),
            7_u32,
            "present on every skill"
        );
        assert_eq!(presence_flags(FlagFamily::Hexen, None, false), 7_u32);
        assert_eq!(
            to_names(
                presence_flags(FlagFamily::Doom, None, true),
                FlagKind::Thing,
                FlagFamily::Doom
            ),
            vec!["easy", "medium", "hard", "notsingle"],
            "the Doom family sets notsingle"
        );
        assert_eq!(
            to_names(
                presence_flags(FlagFamily::Hexen, None, true),
                FlagKind::Thing,
                FlagFamily::Hexen
            ),
            vec!["easy", "medium", "hard", "coop", "dm"],
            "the Hexen single-clearing equivalent names the multiplayer modes"
        );
        assert_eq!(
            presence_flags(FlagFamily::Doom, Some(&[Skill::Easy]), true),
            1_u32.checked_add(16_u32).expect("easy + notsingle"),
            "skills replace the default set"
        );
        assert_eq!(
            presence_flags(FlagFamily::Hexen, Some(&[Skill::Hard]), false),
            4_u32
        );
    }

    #[test]
    fn skills_parse_case_insensitively_and_refuse_anything_else() {
        assert_eq!(Skill::parse("EASY"), Some(Skill::Easy));
        assert_eq!(Skill::parse("Hard"), Some(Skill::Hard));
        assert_eq!(Skill::parse("medium"), Some(Skill::Medium));
        assert_eq!(Skill::parse("nightmare"), None);
    }

    #[test]
    fn the_room_grid_runs_row_major_from_the_raised_minimum_corner() {
        let map = square_rooms(&[(0_i32, 100_i32)]);
        assert_eq!(
            slots(&map, 0_usize, 20_i64, 10_usize),
            vec![
                (20_i64, 20_i64),
                (60_i64, 20_i64),
                (20_i64, 60_i64),
                (60_i64, 60_i64)
            ],
            "step 40 from (20, 20); the 100-line is inside every clearance"
        );
        assert_eq!(
            slots(&map, 0_usize, 20_i64, 2_usize),
            vec![(20_i64, 20_i64), (60_i64, 20_i64)],
            "the fill stops at the requested count"
        );
        assert!(
            slots(&map, 0_usize, 200_i64, 4_usize).is_empty(),
            "nothing clears 200 units in a 100x100 room"
        );
    }

    #[test]
    fn clearance_uses_the_defined_radius_or_the_plan_default() {
        let definitions = doom_definitions();
        assert_eq!(
            thing_clearance(&definitions, 3001_u32),
            28_i64,
            "the Imp's 20-unit radius plus the 8-unit margin"
        );
        assert_eq!(
            thing_clearance(&definitions, 999_999_u32),
            24_i64,
            "an unlisted type gets 16"
        );
    }

    #[test]
    fn a_room_fill_places_row_major_with_the_row_s_shape() {
        let map = square_rooms(&[(0_i32, 300_i32)]);
        let definitions = doom_definitions();
        let room_sectors = rooms_a();
        let context = context(&map, &room_sectors, &definitions);
        let placement = place(&context, &[room_row(3001_u32, "a", 2_u32)]).expect("the fill runs");
        assert_eq!(
            placement.things.len(),
            2,
            "exactly the requested count from a room that holds more"
        );
        assert!(placement.warnings.is_empty(), "no shortage, no note");
        assert_eq!(
            placement.things.first().expect("two rows"),
            &Thing {
                x: 28_i32,
                y: 28_i32,
                angle: 90_i32,
                kind: 3001_i32,
                flags: 7_u32,
                ..Default::default()
            },
            "the first grid point carries the row's type, angle and flags"
        );
        assert_eq!(
            placement.things.get(1).expect("two rows").x,
            84_i32,
            "x runs fastest: clearance 28, step 56"
        );
    }

    #[test]
    fn a_short_room_warns_fits_only_and_keeps_what_landed() {
        let map = square_rooms(&[(0_i32, 64_i32)]);
        let definitions = doom_definitions();
        let room_sectors = rooms_a();
        let context = context(&map, &room_sectors, &definitions);
        let placement = place(&context, &[room_row(3001_u32, "a", 4_u32)]).expect("the fill runs");
        assert_eq!(placement.things.len(), 1, "one imp clears a 64-unit room");
        assert_eq!(
            placement.warnings,
            vec!["room 'a' fits only 1 of 4 things".to_owned()],
            "the documented note"
        );
    }

    #[test]
    fn a_pinned_point_travels_the_units_transform_and_grid() {
        let map = square_rooms(&[(0_i32, 100_i32)]);
        let definitions = doom_definitions();
        let room_sectors = rooms_a();
        let mut context = context(&map, &room_sectors, &definitions);
        context.units = Units {
            scale_num: 4_i64,
            ..Units::default()
        };
        context.grid = 8_i64;
        let pinned = PlannedThing {
            kind: 2035_u32,
            spot: ThingSpot::Point((30_i64, 32_i64)),
            count: 1_u32,
            angle: 0_i32,
            flags: 7_u32,
            path: "things[1]".to_owned(),
        };
        let placement = place(&context, &[pinned]).expect("the point transforms");
        assert_eq!(
            placement.things.first().expect("one row"),
            &Thing {
                x: 120_i32,
                y: 128_i32,
                kind: 2035_i32,
                flags: 7_u32,
                ..Default::default()
            },
            "30*4=120, 32*4=128, both on the 8 grid"
        );
    }

    #[test]
    fn an_untransformable_point_and_an_unknown_room_name_their_path() {
        let map = square_rooms(&[(0_i32, 100_i32)]);
        let definitions = doom_definitions();
        let room_sectors = rooms_a();
        let mut context = context(&map, &room_sectors, &definitions);
        context.units = Units {
            scale_num: i64::MAX,
            ..Units::default()
        };
        let far = PlannedThing {
            kind: 2035_u32,
            spot: ThingSpot::Point((8_i64, 8_i64)),
            count: 1_u32,
            angle: 0_i32,
            flags: 7_u32,
            path: "things[2]".to_owned(),
        };
        let refused = place(&context, &[far]).expect_err("the coordinate leaves the range");
        assert_eq!(refused.code(), "invalid_argument");
        assert!(refused.to_string().contains("things[2]"), "{refused}");
        let ghost = place(&context, &[room_row(3001_u32, "ghost", 1_u32)])
            .expect_err("the room is not in the plan");
        assert_eq!(ghost.code(), "invalid_argument", "unknown room");
        assert!(ghost.to_string().contains("things[0].room"), "{ghost}");
        assert!(ghost.to_string().contains("'ghost'"), "{ghost}");
    }

    #[test]
    fn the_room_centre_is_the_bbox_midpoint() {
        let map = square_rooms(&[(0_i32, 100_i32)]);
        let definitions = doom_definitions();
        let room_sectors = rooms_a();
        let context = context(&map, &room_sectors, &definitions);
        let start = PlannedThing {
            kind: PLAYER_ONE_START,
            spot: ThingSpot::RoomCentre { id: "a".to_owned() },
            count: 1_u32,
            angle: 270_i32,
            flags: 7_u32,
            path: "player_start".to_owned(),
        };
        let placement = place(&context, core::slice::from_ref(&start)).expect("the centre lands");
        let row = placement.things.first().expect("one start");
        assert_eq!(
            (row.x, row.y, row.kind, row.angle),
            (50_i32, 50_i32, 1_i32, 270_i32)
        );
        assert!(
            placement.warnings.is_empty(),
            "the centre never short-lands"
        );
        let ghost = PlannedThing {
            spot: ThingSpot::RoomCentre {
                id: "gone".to_owned(),
            },
            ..start
        };
        let refused = place(&context, &[ghost]).expect_err("no such room");
        assert!(
            refused.to_string().contains("player_start.room"),
            "{refused} names the parameter"
        );
    }

    #[test]
    fn oversized_coordinates_and_types_are_limits_naming_the_row() {
        let map = square_rooms(&[(0_i32, 100_i32)]);
        let definitions = doom_definitions();
        let room_sectors = rooms_a();
        let context = context(&map, &room_sectors, &definitions);
        let far = PlannedThing {
            kind: 2035_u32,
            spot: ThingSpot::Point((i64::from(i32::MAX).saturating_add(1_i64), 0_i64)),
            count: 1_u32,
            angle: 0_i32,
            flags: 7_u32,
            path: "things[3]".to_owned(),
        };
        let refused = place(&context, &[far]).expect_err("no i32 holds it");
        assert_eq!(refused.code(), "limit_exceeded", "a coordinate limit");
        assert!(refused.to_string().contains("things[3]"), "{refused}");
        let big = PlannedThing {
            kind: u32::MAX,
            ..room_row(3001_u32, "a", 1_u32)
        };
        let toowide = place(&context, &[big]).expect_err("no i32 holds the type");
        assert_eq!(toowide.code(), "limit_exceeded", "a type limit");
        assert!(toowide.to_string().contains("things[0]"), "{toowide}");
    }
}
