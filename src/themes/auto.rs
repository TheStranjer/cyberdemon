//! The pure `auto_texture` engine: dress a [`Map`] from theme textures.
//!
//! [`apply`] takes the already-resolved texture names and a selected set
//! of sectors and either fixes what would render wrong
//! ([`AutoMode::FillMissing`]) or repaints everything in the theme
//! ([`AutoMode::Retexture`]). It never reads a theme table or a session:
//! the tool in [`crate::tools::themes`] resolves the roles once, computes
//! which linedefs carry a door special (only the game tables can tell) and
//! hands the verdict to this module.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::themes::Available;
use crate::wad::map::consts::{MAX_STEP_HEIGHT, NO_TEXTURE, is_sky_flat};
use crate::wad::map::{Linedef, Map, Sector, Sidedef};

/// What an `auto_texture` call should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoMode {
    /// Only change what would render wrong: missing wall textures and
    /// flats that do not exist.
    FillMissing,
    /// Repaint every selected wall and flat with the theme's textures,
    /// keeping door, switch and other special-carrying lines.
    Retexture,
}

/// The theme textures the engine paints with, already resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeTextures {
    /// The `wall` role's texture.
    pub wall: String,
    /// The `door` role's texture.
    pub door: String,
    /// The `door_track` role's texture.
    pub door_track: String,
    /// The `step` role's texture.
    pub step: String,
    /// The `floor` role's flat.
    pub floor: String,
    /// The `ceiling` role's flat.
    pub ceiling: String,
}

/// Everything one `auto_texture` pass needs besides the map.
#[derive(Debug)]
pub struct AutoPlan<'plan> {
    /// Fill or repaint.
    pub mode: AutoMode,
    /// Whether sector flats participate.
    pub include_flats: bool,
    /// The sectors to dress.
    pub selected: BTreeSet<usize>,
    /// Linedef indices whose special is a door action.
    pub door_lines: BTreeSet<usize>,
    /// The resolved theme textures.
    pub textures: ThemeTextures,
    /// What exists in the session (for fill checks and `F_SKY1`).
    pub available: &'plan Available,
}

/// What one `auto_texture` pass changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutoChange {
    /// Sidedefs whose texture fields changed.
    pub changed_sidedefs: usize,
    /// Sectors whose flats changed.
    pub changed_sectors: usize,
}

/// One textured slot of a sidedef.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    /// The upper texture.
    Upper,
    /// The middle texture.
    Middle,
    /// The lower texture.
    Lower,
}

/// One pending sidedef edit: `(sidedef, slot) -> texture`.
type Edits = BTreeMap<(usize, Slot), String>;

/// One selected side of a linedef with both sectors resolved.
struct Side<'map> {
    /// The linedef index.
    line: usize,
    /// The sidedef index.
    index: usize,
    /// The sidedef itself.
    data: &'map Sidedef,
    /// The sector this side opens into.
    this: &'map Sector,
    /// The sector across the line (`None` on a one-sided line).
    other: Option<&'map Sector>,
    /// Whether the line has a back side at all.
    two_sided: bool,
    /// The line's script special.
    special: i32,
}

/// Apply `plan` to `map` and report the counts.
#[inline]
pub fn apply(map: &mut Map, plan: &AutoPlan<'_>) -> AutoChange {
    let mut edits: Edits = BTreeMap::new();
    match plan.mode {
        AutoMode::FillMissing => fill_missing(map, plan, &mut edits),
        AutoMode::Retexture => retexture(map, plan, &mut edits),
    }
    let changed_sidedefs = apply_edits(map, &edits);
    let changed_sectors = if plan.include_flats {
        apply_flats(map, plan)
    } else {
        0
    };
    AutoChange {
        changed_sidedefs,
        changed_sectors,
    }
}

/// Every selected side of every line, front and back alike.
fn selected_sides<'map>(map: &'map Map, selected: &BTreeSet<usize>) -> Vec<Side<'map>> {
    map.linedefs
        .iter()
        .enumerate()
        .flat_map(|(line, def)| {
            [def.front, def.back]
                .into_iter()
                .flatten()
                .map(move |index| (line, def, index))
        })
        .filter_map(|slot| record(map, selected, slot))
        .collect()
}

/// One side slot of one line: `(linedef, definition, sidedef)`.
type SlotRef<'lines> = (usize, &'lines Linedef, usize);

/// The [`Side`] for one slot, `None` when its sector is unselected or the
/// sidedef/sector references do not resolve.
fn record<'map>(
    map: &'map Map,
    selected: &BTreeSet<usize>,
    slot: SlotRef<'_>,
) -> Option<Side<'map>> {
    let (line, def, index) = slot;
    let side = map.sidedefs.get(index)?;
    if !selected.contains(&side.sector) {
        return None;
    }
    let this = map.sectors.get(side.sector)?;
    let across = if def.front == Some(index) {
        def.back
    } else {
        def.front
    };
    Some(Side {
        line,
        index,
        data: side,
        this,
        other: far_sector(map, across, index),
        two_sided: def.back.is_some(),
        special: def.special,
    })
}

/// The sector behind the far side, unless both sides are the same sidedef.
fn far_sector(map: &Map, across: Option<usize>, index: usize) -> Option<&Sector> {
    across
        .filter(|&far| far != index)
        .and_then(|far| map.sidedefs.get(far))
        .and_then(|sidedef| map.sectors.get(sidedef.sector))
}

/// The fill rule: missing one-sided middles, upper gaps and lower steps.
fn fill_missing(map: &Map, plan: &AutoPlan<'_>, edits: &mut Edits) {
    for side in selected_sides(map, &plan.selected) {
        fill_side(&side, plan, edits);
    }
}

/// The missing-texture verdict for one side.
fn fill_side(side: &Side<'_>, plan: &AutoPlan<'_>, edits: &mut Edits) {
    if !side.two_sided {
        if side.data.middle == NO_TEXTURE {
            edits.insert((side.index, Slot::Middle), plan.textures.wall.clone());
        }
        return;
    }
    let Some(other) = side.other else {
        return;
    };
    if let Some(texture) = upper_choice(side, other, plan) {
        edits.insert((side.index, Slot::Upper), texture);
    }
    if let Some(texture) = lower_choice(side, other, plan) {
        edits.insert((side.index, Slot::Lower), texture);
    }
}

/// The upper texture for a visible ceiling gap, if one must be filled.
fn upper_choice(side: &Side<'_>, other: &Sector, plan: &AutoPlan<'_>) -> Option<String> {
    let this = side.this;
    let shows_gap = side.data.upper == NO_TEXTURE
        && other.ceiling_height < this.ceiling_height
        && !(is_sky_flat(&this.ceiling_flat) && is_sky_flat(&other.ceiling_flat));
    if !shows_gap {
        return None;
    }
    if other.ceiling_height == other.floor_height {
        Some(plan.textures.door.clone())
    } else {
        Some(plan.textures.wall.clone())
    }
}

/// The lower texture for a visible floor step, if one must be filled.
fn lower_choice(side: &Side<'_>, other: &Sector, plan: &AutoPlan<'_>) -> Option<String> {
    if side.data.lower != NO_TEXTURE {
        return None;
    }
    let gap = gap_up(other, side.this)?;
    if gap <= MAX_STEP_HEIGHT {
        Some(plan.textures.step.clone())
    } else {
        Some(plan.textures.wall.clone())
    }
}

/// The positive floor step from `other` up to `this`.
fn gap_up(other: &Sector, this: &Sector) -> Option<i32> {
    this.floor_height
        .checked_sub(other.floor_height)
        .filter(|gap| *gap > 0_i32)
}

/// The repaint rule: theme everything except special-carrying lines.
fn retexture(map: &Map, plan: &AutoPlan<'_>, edits: &mut Edits) {
    let jambs = jamb_lines(map, &plan.door_lines);
    for side in selected_sides(map, &plan.selected) {
        retexture_side(&side, plan, &jambs, edits);
    }
}

/// The repaint verdict for one selected side.
fn retexture_side(
    side: &Side<'_>,
    plan: &AutoPlan<'_>,
    jambs: &BTreeSet<usize>,
    edits: &mut Edits,
) {
    if plan.door_lines.contains(&side.line) {
        let door = (side.data.middle != NO_TEXTURE).then(|| plan.textures.door.clone());
        insert_opt(edits, side.index, Slot::Middle, door);
        return;
    }
    if side.special != 0_i32 {
        return;
    }
    if !side.two_sided {
        edits.insert((side.index, Slot::Middle), plan.textures.wall.clone());
        return;
    }
    let track = jambs.contains(&side.line);
    let texture = if track {
        &plan.textures.door_track
    } else {
        &plan.textures.wall
    };
    let middle = (side.data.middle != NO_TEXTURE).then(|| texture.clone());
    insert_opt(edits, side.index, Slot::Middle, middle);
    let upper = (side.data.upper != NO_TEXTURE).then(|| plan.textures.wall.clone());
    insert_opt(edits, side.index, Slot::Upper, upper);
    insert_opt(edits, side.index, Slot::Lower, lower_band(side, plan));
}

/// The lower band of a plain two-sided line: a small floor step takes the
/// step texture, anything taller the wall.
fn lower_band(side: &Side<'_>, plan: &AutoPlan<'_>) -> Option<String> {
    if side.data.lower == NO_TEXTURE {
        return None;
    }
    let step = side
        .other
        .and_then(|other| gap_up(other, side.this))
        .is_some_and(|gap| gap <= MAX_STEP_HEIGHT);
    if step {
        Some(plan.textures.step.clone())
    } else {
        Some(plan.textures.wall.clone())
    }
}

/// Inserts `texture` when it is `Some`.
fn insert_opt(edits: &mut Edits, index: usize, slot: Slot, pending: Option<String>) {
    if let Some(texture) = pending {
        edits.insert((index, slot), texture);
    }
}

/// The two-sided, special-free lines touching a door line: the jambs.
fn jamb_lines(map: &Map, doors: &BTreeSet<usize>) -> BTreeSet<usize> {
    if doors.is_empty() {
        return BTreeSet::new();
    }
    let ends: Vec<(usize, usize)> = doors
        .iter()
        .filter_map(|index| map.linedefs.get(*index))
        .map(|line| (line.v1, line.v2))
        .collect();
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(index, line)| {
            !doors.contains(&index)
                && line.special == 0_i32
                && line.front.is_some()
                && line.back.is_some()
                && ends
                    .iter()
                    .any(|&(a, b)| a == line.v1 || a == line.v2 || b == line.v1 || b == line.v2)
        })
        .map(|(index, _line)| index)
        .collect()
}

/// Writes the queued edits, returning how many sidedefs changed.
fn apply_edits(map: &mut Map, edits: &Edits) -> usize {
    let mut changed: BTreeSet<usize> = BTreeSet::new();
    for (&(index, slot), texture) in edits {
        let Some(side) = map.sidedefs.get_mut(index) else {
            continue;
        };
        let field = match slot {
            Slot::Upper => &mut side.upper,
            Slot::Middle => &mut side.middle,
            Slot::Lower => &mut side.lower,
        };
        if field != texture {
            field.clone_from(texture);
            changed.insert(index);
        }
    }
    changed.len()
}

/// Repairs or repaints the selected sectors' flats; the changed count.
fn apply_flats(map: &mut Map, plan: &AutoPlan<'_>) -> usize {
    let mut changed: BTreeSet<usize> = BTreeSet::new();
    for index in &plan.selected {
        let Some(sector) = map.sectors.get_mut(*index) else {
            continue;
        };
        let floor = flat_wanted(plan, &sector.floor_flat, &plan.textures.floor, false);
        let ceiling = flat_wanted(plan, &sector.ceiling_flat, &plan.textures.ceiling, true);
        if let Some(name) = floor {
            sector.floor_flat = name;
            changed.insert(*index);
        }
        if let Some(name) = ceiling {
            sector.ceiling_flat = name;
            changed.insert(*index);
        }
    }
    changed.len()
}

/// Whether this flat slot should move to `wanted` under the current mode.
///
/// `is_ceiling` guards `F_SKY1`: the sky is never painted over.
fn flat_wanted(
    plan: &AutoPlan<'_>,
    current: &str,
    wanted: &str,
    is_ceiling: bool,
) -> Option<String> {
    if is_ceiling && is_sky_flat(current) {
        return None;
    }
    match plan.mode {
        AutoMode::FillMissing if !plan.available.has_flat(current) && current != wanted => {
            Some(wanted.to_owned())
        }
        AutoMode::FillMissing => None,
        AutoMode::Retexture => Some(wanted.to_owned()).filter(|next| next.as_str() != current),
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use super::{AutoMode, AutoPlan, ThemeTextures, apply};
    use crate::themes::Available;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::consts::SKY_FLAT;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};

    /// The resolved theme names used across these tests.
    fn textures() -> ThemeTextures {
        ThemeTextures {
            wall: "WALL".to_owned(),
            door: "DOOR".to_owned(),
            door_track: "TRACK".to_owned(),
            step: "STEP".to_owned(),
            floor: "FLR".to_owned(),
            ceiling: "CEL".to_owned(),
        }
    }

    /// An availability set matching the theme names plus the sky.
    fn available() -> Available {
        let mut walls = BTreeSet::new();
        walls.insert("WALL".to_owned());
        Available {
            walls,
            flats: ["FLR", "CEL", SKY_FLAT]
                .iter()
                .map(|f| (*f).to_owned())
                .collect(),
        }
    }

    /// A plan in `mode` over every sector, with `doors` as door lines.
    fn plan(mode: AutoMode, doors: BTreeSet<usize>, available: &Available) -> AutoPlan<'_> {
        AutoPlan {
            mode,
            include_flats: true,
            selected: BTreeSet::from([0_usize, 1, 2]),
            door_lines: doors,
            textures: textures(),
            available,
        }
    }

    /// A sector with the given heights and flats.
    fn sector(floor: i32, ceil: i32, floor_flat: &str, ceil_flat: &str) -> Sector {
        Sector {
            floor_height: floor,
            ceiling_height: ceil,
            floor_flat: floor_flat.to_owned(),
            ceiling_flat: ceil_flat.to_owned(),
            ..Default::default()
        }
    }

    /// A sidedef with the given upper/middle/lower over `sec`.
    fn side(upper: &str, middle: &str, lower: &str, sec: usize) -> Sidedef {
        Sidedef {
            upper: upper.to_owned(),
            middle: middle.to_owned(),
            lower: lower.to_owned(),
            sector: sec,
            ..Default::default()
        }
    }

    /// A line from `v1` to `v2` with the given sides and no special.
    fn line(v1: usize, v2: usize, front: usize, back: Option<usize>) -> Linedef {
        Linedef {
            v1,
            v2,
            front: Some(front),
            back,
            ..Default::default()
        }
    }

    /// The test lines: line 1 carries the door special 1.
    fn lines() -> Vec<Linedef> {
        let mut all = vec![
            line(0, 1, 0, None),
            line(1, 2, 1, Some(2)),
            line(1, 2, 3, Some(4)),
            line(2, 3, 5, Some(6)),
        ];
        all[1].special = 1_i32;
        all
    }

    /// The test hall:
    ///
    /// - sector 0: the hall (floor 0, ceiling 128, sky);
    /// - sector 1: behind the door line, ceiling 32 (an open gap of 96);
    /// - sector 2: the stepped room (floor 16, ceiling 96, unknown flat);
    /// - line 0: one-sided with an empty middle;
    /// - line 1: the door line (special 1) with a middle, front side 1,
    ///   back side 2;
    /// - line 2: a special-free copy of line 1's span, so it reads as a
    ///   jamb, its front middle is `TRACKX`;
    /// - line 3: hall over the stepped room, back side 6 with an `OLD`
    ///   upper and an empty lower.
    fn hall() -> Map {
        Map {
            format: MapFormat::Doom,
            vertices: (0..4_usize)
                .map(|i| Vertex {
                    x: i32::try_from(i)
                        .expect("test indices fit")
                        .checked_mul(64_i32)
                        .expect("test positions fit"),
                    ..Default::default()
                })
                .collect(),
            linedefs: lines(),
            sidedefs: vec![
                side("-", "-", "-", 0),
                side("-", "DOORX", "-", 0),
                side("-", "-", "-", 1),
                side("-", "TRACKX", "-", 0),
                side("-", "-", "-", 1),
                side("-", "-", "-", 0),
                side("OLD", "-", "-", 2),
            ],
            sectors: vec![
                sector(0, 128, "FLR", SKY_FLAT),
                sector(0, 32, "FLR", "CEL"),
                sector(16, 96, "GONE", "CEL"),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn fill_missing_fixes_middle_upper_and_lower_and_a_bad_flat() {
        let avail = available();
        let mut map = hall();
        let change = apply(
            &mut map,
            &plan(AutoMode::FillMissing, BTreeSet::new(), &avail),
        );
        assert_eq!(map.sidedefs[0].middle, "WALL", "the empty one-sided middle");
        assert_eq!(
            map.sidedefs[1].upper, "WALL",
            "the 96-unit gap over the door sector"
        );
        assert_eq!(
            map.sidedefs[3].upper, "WALL",
            "the same gap on the jamb line"
        );
        assert_eq!(
            map.sidedefs[5].upper, "WALL",
            "the gap over the stepped room"
        );
        assert_eq!(
            map.sidedefs[6].lower, "STEP",
            "the 16-unit step up into sector 2"
        );
        assert_eq!(
            map.sidedefs[6].upper, "OLD",
            "an existing upper is left alone"
        );
        assert_eq!(
            map.sectors[2].floor_flat, "FLR",
            "the unknown flat is replaced"
        );
        assert_eq!(
            map.sectors[0].ceiling_flat, SKY_FLAT,
            "the sky is never touched"
        );
        assert_eq!(change.changed_sidedefs, 5, "the five sides above");
        assert_eq!(change.changed_sectors, 1, "one unknown flat");
    }

    #[test]
    fn retexture_repaints_plain_lines_and_doors_with_their_jambs() {
        let avail = available();
        let mut map = hall();
        let change = apply(
            &mut map,
            &plan(AutoMode::Retexture, BTreeSet::from([1_usize]), &avail),
        );
        assert_eq!(
            map.sidedefs[0].middle, "WALL",
            "the one-sided wall repaints"
        );
        assert_eq!(
            map.sidedefs[1].middle, "DOOR",
            "the door line gets the door role"
        );
        assert_eq!(map.sidedefs[3].middle, "TRACK", "the jamb gets the track");
        assert_eq!(map.sidedefs[6].upper, "WALL", "the existing upper repaints");
        assert_eq!(
            map.sidedefs[5].upper, "-",
            "fill does not happen in retexture"
        );
        assert_eq!(map.sectors[2].floor_flat, "FLR", "floors follow the theme");
        assert_eq!(map.sectors[2].ceiling_flat, "CEL", "the theme ceiling");
        assert_eq!(map.sectors[0].ceiling_flat, SKY_FLAT, "F_SKY1 is kept");
        assert_eq!(change.changed_sidedefs, 4, "the four sides above");
        assert_eq!(change.changed_sectors, 1, "sector 2's floor was unknown");
    }

    #[test]
    fn retexture_keeps_other_specials_and_follows_the_selector() {
        let avail = available();
        let mut map = hall();
        map.linedefs[0].special = 19_i32;
        let mut sector_only = plan(AutoMode::Retexture, BTreeSet::new(), &avail);
        sector_only.selected = BTreeSet::from([0_usize]);
        let change = apply(&mut map, &sector_only);
        assert_eq!(
            map.sidedefs[0].middle, "-",
            "the special-carrying line keeps its look"
        );
        assert_eq!(
            map.sidedefs[1].middle, "DOORX",
            "so does the door line without a door verdict"
        );
        assert_eq!(
            map.sidedefs[3].middle, "WALL",
            "the plain two-sided middle repaints"
        );
        assert_eq!(
            map.sectors[0].ceiling_flat, SKY_FLAT,
            "the hall ceiling is outside the theme's reach"
        );
        assert_eq!(change.changed_sidedefs, 1, "only that middle");
        assert_eq!(
            change.changed_sectors, 0,
            "the hall's flats were already theme flats"
        );
    }

    #[test]
    fn a_closed_back_sector_behind_an_upper_gap_reads_as_a_door() {
        let avail = available();
        let mut map = hall();
        map.sectors[1].ceiling_height = 0_i32;
        apply(
            &mut map,
            &plan(AutoMode::FillMissing, BTreeSet::new(), &avail),
        );
        assert_eq!(
            map.sidedefs[1].upper, "DOOR",
            "ceiling equals floor behind the gap"
        );
        assert_eq!(
            map.sidedefs[3].upper, "DOOR",
            "the jamb line sees the same door"
        );
        assert_eq!(
            map.sidedefs[5].upper, "WALL",
            "the stepped room is not closed"
        );
    }

    #[test]
    fn include_flats_false_leaves_the_planes_alone() {
        let avail = available();
        let mut map = hall();
        let mut no_flats = plan(AutoMode::Retexture, BTreeSet::new(), &avail);
        no_flats.include_flats = false;
        let change = apply(&mut map, &no_flats);
        assert_eq!(
            map.sectors[2].floor_flat, "GONE",
            "the unknown flat survives"
        );
        assert_eq!(change.changed_sectors, 0, "nothing counted");
    }

    #[test]
    fn a_big_step_gets_the_wall_not_the_step() {
        let avail = available();
        let mut map = hall();
        map.sectors[2].floor_height = 64_i32;
        apply(
            &mut map,
            &plan(AutoMode::FillMissing, BTreeSet::new(), &avail),
        );
        assert_eq!(map.sidedefs[6].lower, "WALL", "a 64-unit ledge is a wall");
    }
}
