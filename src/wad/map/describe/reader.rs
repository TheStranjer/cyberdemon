//! The per-map read: the one `impl` of [`Describe`]. [`Describe::new`]
//! folds the map's flag family, its [`FeatureTargets`] and its
//! [`Skill::Medium`] [`Playability`] walk once, [`Describe::sector`]
//! answers one [`SectorFacts`] row from them and [`Describe::summary`] the
//! whole [`MapOverview`].
//!
//! Nothing here measures or classifies on its own: the area and the label
//! point come from [`super::shape`], the boundary, neighbour-group and
//! special-role reads from [`super::roles`], the counts, tallies and the
//! playable sentences from [`super::tally`]. What stays here is only what
//! the folded walks can answer: the `door` and `lift` roles of a
//! neighbour across a boundary, the `via` precedence over them, the closed
//! band, the window gap and the walk-over exit sweeping in the sector its
//! midpoint stands in.
//!
//! [`FeatureTargets`]: crate::wad::map::playability::FeatureTargets
//! [`Playability`]: crate::wad::map::playability::Playability
//! [`Skill::Medium`]: crate::wad::map::flags::Skill::Medium

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use crate::defs::Definitions;
use crate::wad::map::flags::{BLOCKING, Skill};
use crate::wad::map::geometry::{point_in_sector, sector_bbox};
use crate::wad::map::model::{Linedef, Map};
use crate::wad::map::playability::{
    analyze_playability, feature_targets, start_thing, stuck_things,
};

use super::roles::{
    boundary_edges, carries_flag, height_via, is_stair_step, line_midpoint, neighbour_groups,
    side_sector, special_roles, walk_over_exit,
};
use super::shape::{label_point, shoelace_area};
use super::tally::{
    category_counts, element_counts, exit_reachable, flat_tally, overview_summary, player_starts,
    special_info, start_stuck, thing_counts, top_rows, wall_tally, wall_textures,
};
use super::{
    Describe, MapOverview, NAME_DOOR, NAME_EXIT_ROOM, NAME_IMPASSABLE, NAME_LIFT, NAME_SECRET,
    NAME_SKY, NAME_STAIR_STEP, NAME_TELEPORT_DESTINATION, NAME_WINDOW, Neighbour, PlayableSummary,
    SectorFacts,
};

/// The ceiling flat that means open sky, the name the renderer switches on.
const SKY_FLAT: &str = "F_SKY1";
/// The greatest headroom of a closed door band, map units.
const CLOSED_BAND_HEIGHT: i32 = 8_i32;
/// The smallest vertical light gap across a blocking boundary that reads
/// as a window, map units.
const LIGHT_GAP: i32 = 16_i32;

/// The rows `MapOverview::largest_sectors` answers with.
const LARGEST_ROWS: usize = 5_usize;
/// The definition category a monster stands under.
const MONSTER_CATEGORY: &str = "Monsters";
/// The definition categories an item counts under.
const ITEM_CATEGORIES: [&str; 5] = [
    "Weapons",
    "Ammunition",
    "Powerups",
    "Keys",
    "Health And Armour",
];

impl<'map> Describe<'map> {
    /// Fold `map` for describing: read its own flag family once, then the
    /// feature targets and the [`Skill::Medium`] playability walk over it.
    #[must_use]
    #[inline]
    pub fn new(map: &'map Map, defs: &'map Definitions) -> Self {
        let family = map.flag_family();
        Self {
            map,
            defs,
            family,
            targets: feature_targets(map, family),
            play: analyze_playability(map, family, Skill::Medium),
        }
    }

    /// Describe sector `sector`: the measured and cosmetic facts, the
    /// role names and the neighbour rows this module's rules define.
    ///
    /// `None` when the map holds no sector with that index. A sector with
    /// no usable boundary still describes: zero area, no box, no centre,
    /// no textures, no things and no neighbours.
    #[must_use]
    #[inline]
    pub fn sector(&self, sector: usize) -> Option<SectorFacts> {
        let row = self.map.sectors.get(sector)?;
        let edges = boundary_edges(self.map, sector);
        let area = shoelace_area(&edges);
        let neighbours = self.build_neighbours(sector, row.floor_height);
        Some(SectorFacts {
            sector,
            area,
            bbox: sector_bbox(self.map, sector).map(<[i32; 4]>::from),
            center: label_point(&edges),
            floor: row.floor_height,
            ceiling: row.ceiling_height,
            height: row.ceiling_height.saturating_sub(row.floor_height),
            light: row.light,
            floor_flat: row.floor_flat.clone(),
            ceiling_flat: row.ceiling_flat.clone(),
            sky: row.ceiling_flat == SKY_FLAT,
            special: special_info(row.special, self.defs),
            tag: row.tag,
            wall_textures: wall_textures(self.map, sector),
            things: thing_counts(self.map, self.defs, sector),
            roles: self.collect_roles(sector, area, &neighbours),
            neighbours,
        })
    }

    /// The neighbour rows of `sector`: its groups of shared two-sided
    /// lines, in sector order, each with the `via` its boundary and the
    /// sector across it answer with.
    fn build_neighbours(&self, sector: usize, own_floor: i32) -> Vec<Neighbour> {
        neighbour_groups(self.map, sector)
            .into_iter()
            .map(|(other, lines)| {
                let their_floor = self
                    .map
                    .sectors
                    .get(other)
                    .map_or(own_floor, |row| row.floor_height);
                let floor_delta = their_floor.saturating_sub(own_floor);
                let draft = Neighbour {
                    sector: other,
                    via: String::new(),
                    lines,
                    floor_delta,
                };
                let via = self.via_for(sector, &draft);
                Neighbour { via, ..draft }
            })
            .collect()
    }

    /// The way through the boundary `across` describes between `own` and
    /// its neighbour: the neighbour's `door` and `lift` roles first, then
    /// the blocking rules, then the heights.
    fn via_for(&self, own: usize, across: &Neighbour) -> String {
        if self.has_door_role(across.sector) {
            return NAME_DOOR.to_owned();
        }
        if self.targets.lifts.contains(&across.sector) {
            return NAME_LIFT.to_owned();
        }
        let blocking = across
            .lines
            .iter()
            .copied()
            .filter(|line| carries_flag(self.map, *line, BLOCKING))
            .count();
        if blocking == 0 {
            return height_via(across.floor_delta).to_owned();
        }
        if blocking == across.lines.len() && self.window_gap(own, across.sector) >= LIGHT_GAP {
            NAME_WINDOW.to_owned()
        } else {
            NAME_IMPASSABLE.to_owned()
        }
    }

    /// Whether `sector` wears the `door` role: a door action raises its
    /// ceiling, or it is a closed band with neighbours on both sides.
    fn has_door_role(&self, sector: usize) -> bool {
        self.targets.doors.contains(&sector) || self.closed_band(sector)
    }

    /// Whether `sector` is a closed door band: no more than
    /// [`CLOSED_BAND_HEIGHT`] of headroom and at least two distinct
    /// sectors across its two-sided lines.
    fn closed_band(&self, sector: usize) -> bool {
        let Some(row) = self.map.sectors.get(sector) else {
            return false;
        };
        row.ceiling_height.saturating_sub(row.floor_height) <= CLOSED_BAND_HEIGHT
            && neighbour_groups(self.map, sector).len() >= 2
    }

    /// The light across the blocking boundary between `own` and `other`:
    /// the lower ceiling over the higher floor, map units.
    fn window_gap(&self, own: usize, other: usize) -> i32 {
        let (Some(a), Some(b)) = (self.map.sectors.get(own), self.map.sectors.get(other)) else {
            return 0_i32;
        };
        a.ceiling_height
            .min(b.ceiling_height)
            .saturating_sub(a.floor_height.max(b.floor_height))
    }

    /// Whether a teleport action lands a player in `sector`.
    fn lands_teleports_in(&self, sector: usize) -> bool {
        self.targets
            .teleporters
            .iter()
            .any(|pad| pad.destination == Some(sector))
    }

    /// Whether an exit line of the playability read touches `sector` on
    /// either resolved side, or walks over into it at its midpoint.
    fn exits_here(&self, sector: usize) -> bool {
        self.play.exits.iter().any(|exit| {
            self.map
                .linedefs
                .get(exit.line)
                .is_some_and(|line| self.exit_side_or_sweep(line, sector))
        })
    }

    /// Whether `line` resolves to `sector` on either side, or is a
    /// walk-over end whose midpoint stands in `sector`.
    fn exit_side_or_sweep(&self, line: &Linedef, sector: usize) -> bool {
        side_sector(self.map, line.front) == Some(sector)
            || side_sector(self.map, line.back) == Some(sector)
            || (walk_over_exit(self.family, line)
                && line_midpoint(self.map, line)
                    .is_some_and(|(x, y)| point_in_sector(self.map, x, y) == Some(sector)))
    }

    /// The sector's role names: lowercase, sorted and deduplicated, as
    /// the module-doc rules state them for `area`, its `neighbours` and
    /// its own row.
    fn collect_roles(&self, sector: usize, area: u64, neighbours: &[Neighbour]) -> Vec<String> {
        let Some(row) = self.map.sectors.get(sector) else {
            return Vec::new();
        };
        let mut found: BTreeSet<String> = BTreeSet::new();
        if self.has_door_role(sector) {
            found.insert(NAME_DOOR.to_owned());
        }
        if self.targets.lifts.contains(&sector) {
            found.insert(NAME_LIFT.to_owned());
        }
        if self.lands_teleports_in(sector) {
            found.insert(NAME_TELEPORT_DESTINATION.to_owned());
        }
        if self.exits_here(sector) {
            found.insert(NAME_EXIT_ROOM.to_owned());
        }
        if row.ceiling_flat == SKY_FLAT {
            found.insert(NAME_SKY.to_owned());
        }
        if is_stair_step(area, neighbours) {
            found.insert(NAME_STAIR_STEP.to_owned());
        }
        found.extend(
            special_roles(row.special, self.family)
                .into_iter()
                .map(str::to_owned),
        );
        found.into_iter().collect()
    }

    /// The sectors wearing the `secret` role, ascending: the per-sector
    /// role read, asked straight of every stored special rather than
    /// re-testing the family's bits here.
    fn secret_sectors(&self) -> Vec<usize> {
        self.map
            .sectors
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                special_roles(row.special, self.family)
                    .contains(&NAME_SECRET)
                    .then_some(index)
            })
            .collect()
    }

    /// The [`LARGEST_ROWS`] biggest sectors by the facts' own shoelace
    /// area, descending, ties by index.
    fn largest_sectors(&self) -> Vec<usize> {
        let mut sized: Vec<(usize, u64)> = (0..self.map.sectors.len())
            .map(|index| (index, shoelace_area(&boundary_edges(self.map, index))))
            .collect();
        sized.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        sized
            .into_iter()
            .take(LARGEST_ROWS)
            .map(|(index, _)| index)
            .collect()
    }

    /// The map-wide [`MapOverview`]: this read's folded format, counts,
    /// walk rows and feature targets, the secret and largest-sector
    /// readings, the thing and texture tallies and the playable answer,
    /// which adds one [`Skill::Medium`] stuck test over the map.
    #[must_use]
    #[inline]
    pub fn summary(&self) -> MapOverview {
        let stuck = stuck_things(self.map, self.defs, Skill::Medium);
        let start = start_thing(self.map, self.family, Skill::Medium);
        let exit_open = exit_reachable(&self.play);
        MapOverview {
            format: self.map.format,
            bounds: self.map.bounds().map(<[i32; 4]>::from),
            counts: element_counts(self.map),
            player_starts: player_starts(self.map),
            exits: self.play.exits.clone(),
            keys: self.play.keys.clone(),
            locked_doors: self.play.locks.clone(),
            teleporters: self.targets.teleporters.clone(),
            secret_sectors: self.secret_sectors(),
            monsters: category_counts(self.map, self.defs, &[MONSTER_CATEGORY]),
            items: category_counts(self.map, self.defs, &ITEM_CATEGORIES),
            top_wall_textures: top_rows(wall_tally(self.map)),
            top_flats: top_rows(flat_tally(self.map)),
            largest_sectors: self.largest_sectors(),
            playable: PlayableSummary {
                ok: self.play.start_sector.is_some() && exit_open && !start_stuck(start, &stuck),
                summary: overview_summary(&self.play, self.map.sectors.len(), exit_open),
            },
        }
    }
}
