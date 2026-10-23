//! The top-level plan compiler: a [`Plan`] and a theme in, a dressed
//! [`CompiledPlan`] out.
//!
//! [`compile`] runs the pipeline in this order, over the pure modules of
//! this crate's `plan` tree:
//!
//! 1. [`Plan::validate`] — structural checks on rooms and connections
//!    ([`plan::model`](crate::plan::model)).
//! 2. [`build_walls`] — arrange the rooms and grow the thick shared-wall
//!    void layers between them ([`plan::walls`](crate::plan::walls)).
//! 3. [`plan_connectors`] — place every connection on a shared boundary
//!    and build its band layers; an opening that spans a whole shared run
//!    takes its wall out of the stack instead of painting a band, so the
//!    two rooms join directly ([`plan::connectors`](crate::plan::connectors)).
//! 4. [`emit`] — one final arrangement of rooms, standing walls and bands,
//!    cut into the fresh [`Map`](crate::wad::map::Map)
//!    ([`plan::emit`](crate::plan::emit)).
//! 5. [`apply_textures`] — dress the emission in the theme's
//!    [`TextureRole`](crate::plan::textures::TextureRole) names
//!    ([`plan::textures`](crate::plan::textures)).
//! 6. The reachability, painted-over-room and floor-gap warnings of
//!    this module.
//! 7. Assemble the [`CompiledPlan`]: map, bookkeeping and warnings.
//!
//! The arrangement of step 4 is the only one: steps 5 to 7 read its
//! [`Emitted::pieces`](crate::plan::emit::Emitted::pieces) and label
//! index, never re-arrange. Nothing here touches archives, sessions or
//! `MCP` types.

use alloc::collections::VecDeque;

use crate::error::CyberError;
use crate::plan::connectors::{ConnectorGeometry, plan_connectors};
use crate::plan::emit::{CompiledPlan, MapIdentity, compiled_plan, emit};
use crate::plan::model::{ConnectionKind, Plan};
use crate::plan::textures::{Scene, TextureRoles, apply_textures};
use crate::plan::walls::build_walls;
use crate::wad::classify::MapFormat;
use crate::wad::name::LumpName;

/// The largest floor gap an opening, door or window may join; taller
/// transitions should be stair runs, and [`compile`] warns about them.
const MAX_STEPLESS_DIFF: i64 = 24_i64;

/// Compile `plan` into a dressed [`CompiledPlan`].
///
/// Runs the seven pipeline steps of this module's documentation:
/// [`Plan::validate`], [`build_walls`], [`plan_connectors`], [`emit`]
/// into a map named `name` in layout `format`, [`apply_textures`] with
/// `roles`, the reachability / coverage / floor-gap warnings of this
/// module, and the [`CompiledPlan`] bookkeeping. Every failure aborts
/// the whole compile: nothing outside the returned value is touched, so
/// a refused plan leaves no half-dressed map behind.
///
/// # Errors
///
/// [`Plan::validate`]'s refusals first, then
/// [`arrange`](fn@crate::geom::arrange)'s [`CyberError::InvalidArgument`] /
/// [`CyberError::LimitExceeded`] from the wall and final arrangements,
/// [`plan_connectors`]' refusals for unplaceable connections (unknown
/// rooms, rooms that "do not touch", boundaries too short, window spans
/// with too little glass, level stair runs), [`emit`]'s limit refusals
/// for out-of-range coordinates, and [`apply_textures`]' internal
/// contract refusals.
#[inline]
pub fn compile(
    plan: &Plan,
    name: LumpName,
    format: MapFormat,
    roles: &dyn TextureRoles,
) -> Result<CompiledPlan, CyberError> {
    plan.validate()?;
    let walls = build_walls(plan)?;
    let connectors = plan_connectors(plan, &walls)?;
    let identity = MapIdentity { name, format };
    let mut emitted = emit(plan, &walls, &connectors, identity)?;
    let mut map = core::mem::take(&mut emitted.map);
    let scene = Scene {
        plan,
        connectors: &connectors,
        emitted: &emitted,
    };
    apply_textures(&mut map, &scene, roles)?;
    emitted.map = map;
    let mut compiled = compiled_plan(plan, &connectors, emitted);
    compiled.warnings = warnings(plan, &connectors, &compiled);
    Ok(compiled)
}

/// The advisory notes of a compiled plan: quirks worth telling the
/// caller about that are not errors, in the order unreachable rooms,
/// painted-over rooms, too-tall floor gaps.
fn warnings(plan: &Plan, connectors: &[ConnectorGeometry], compiled: &CompiledPlan) -> Vec<String> {
    let mut notes = Vec::new();
    unreachable_rooms(plan, connectors, &mut notes);
    covered_rooms(plan, compiled, &mut notes);
    tall_steps(plan, connectors, &mut notes);
    notes
}

/// Warn about every room the connection graph cannot carry you to from
/// the plan's first room.
fn unreachable_rooms(plan: &Plan, connectors: &[ConnectorGeometry], notes: &mut Vec<String>) {
    let reached = reachable_from_first(plan, connectors);
    let Some(root) = plan.rooms.first() else {
        return;
    };
    for (index, room) in plan.rooms.iter().enumerate() {
        if !reached.get(index).copied().unwrap_or(false) {
            notes.push(format!(
                "room '{}' cannot be reached from room '{}' through the plan's connections; connect it or drop it",
                room.id, root.id
            ));
        }
    }
}

/// The rooms reachable from room `0` over the placed connectors'
/// undirected room pairs.
fn reachable_from_first(plan: &Plan, connectors: &[ConnectorGeometry]) -> Vec<bool> {
    let adjacency = adjacency(plan, connectors);
    let mut reached = vec![false; plan.rooms.len()];
    let Some((first, _)) = reached.split_first_mut() else {
        return reached;
    };
    *first = true;
    let mut queue = VecDeque::from([0_usize]);
    while let Some(index) = queue.pop_front() {
        for neighbour in adjacency.get(index).into_iter().flatten() {
            visit_room(*neighbour, &mut reached, &mut queue);
        }
    }
    reached
}

/// Mark `neighbour` reached and queue it when it is new.
fn visit_room(neighbour: usize, reached: &mut [bool], queue: &mut VecDeque<usize>) {
    let Some(slot) = reached.get_mut(neighbour) else {
        return;
    };
    if !*slot {
        *slot = true;
        queue.push_back(neighbour);
    }
}

/// The undirected room-pair adjacency of the placed connectors.
fn adjacency(plan: &Plan, connectors: &[ConnectorGeometry]) -> Vec<Vec<usize>> {
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); plan.rooms.len()];
    for connector in connectors {
        if let Some(from) = neighbours.get_mut(connector.to) {
            from.push(connector.from);
        }
        if let Some(to) = neighbours.get_mut(connector.from) {
            to.push(connector.to);
        }
    }
    neighbours
}

/// Warn about every room a later room painted over completely: its
/// region label survives on no piece, so it owns no sector.
fn covered_rooms(plan: &Plan, compiled: &CompiledPlan, notes: &mut Vec<String>) {
    for room in &plan.rooms {
        if !compiled.room_sectors.contains_key(&room.id) {
            notes.push(format!(
                "room '{}' is fully painted over by later rooms and owns no sector",
                room.id
            ));
        }
    }
}

/// Warn about every opening, door or window joining floors more than
/// [`MAX_STEPLESS_DIFF`] apart: the player cannot step over that.
fn tall_steps(plan: &Plan, connectors: &[ConnectorGeometry], notes: &mut Vec<String>) {
    for connector in connectors {
        if !matches!(
            connector.kind,
            ConnectionKind::Opening | ConnectionKind::Door { .. } | ConnectionKind::Window { .. }
        ) {
            continue;
        }
        let (Some(from), Some(to)) = (plan.rooms.get(connector.from), plan.rooms.get(connector.to))
        else {
            continue;
        };
        let gap = i64::from(from.floor)
            .saturating_sub(i64::from(to.floor))
            .abs();
        if gap > MAX_STEPLESS_DIFF {
            notes.push(format!(
                "step taller than 24 between {} and {}; use stairs",
                from.id, to.id
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use super::{CompiledPlan, compile};
    use crate::defs::definitions_for;
    use crate::geom::label::{Label, Labeler as _, LayerLabeler, SAMPLE_SCALE};
    use crate::plan::model::{Connection, ConnectionKind, DoorStyle, Plan, Room, Shape};
    use crate::plan::textures::{TextureRole, TextureRoles};
    use crate::plan::units::{Units, transform_plan};
    use crate::plan::walls::room_layers;
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::validate::{Context, ResourceIndex, run};
    use crate::wad::map::{Linedef, Map, Sector, Sidedef};
    use crate::wad::name::LumpName;

    /// A theme handing every role a distinct uppercase eight-character name.
    struct TestRoles {
        /// When `false` no role answers and every face stays `"-"`.
        complete: bool,
    }

    impl TestRoles {
        /// Roles answering every [`TextureRole`].
        fn full() -> Self {
            Self { complete: true }
        }

        /// Roles answering none.
        fn missing() -> Self {
            Self { complete: false }
        }
    }

    impl TextureRoles for TestRoles {
        fn texture(&self, role: TextureRole) -> Option<&str> {
            self.complete.then_some(role_name(role))
        }
    }

    /// The fixture name for one role, distinct per slot.
    fn role_name(role: TextureRole) -> &'static str {
        match role {
            TextureRole::Floor => "FLOROLE",
            TextureRole::Ceiling => "CEIROLE",
            TextureRole::Wall => "WALROLE",
            TextureRole::Door => "DOOROLE",
            TextureRole::DoorFloor => "DFOROLE",
            TextureRole::DoorTrack => "DTRROLE",
            TextureRole::WindowSill => "WSIROLE",
            TextureRole::Step => "STEPROLE",
        }
    }

    /// A parsed map name for test data.
    fn map_name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test map names are valid")
    }

    /// A rectangular room with default sector properties.
    fn rect_room(id: &str, left_bottom: (i64, i64), width_height: (i64, i64)) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Rect {
                x: left_bottom.0,
                y: left_bottom.1,
                w: width_height.0,
                h: width_height.1,
            },
            floor: 0,
            ceiling: 128,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// A default-width opening between two rooms by id.
    fn opening(from: &str, to: &str) -> Connection {
        Connection {
            id: None,
            from: from.to_owned(),
            to: to.to_owned(),
            kind: ConnectionKind::Opening,
            at: None,
            width: 64,
        }
    }

    /// A connection of the given kind between two rooms by id.
    fn connection(from: &str, to: &str, kind: ConnectionKind) -> Connection {
        let mut passage = opening(from, to);
        passage.kind = kind;
        passage
    }

    /// Two 128 x 128 rooms sharing the line x = 128, `thickness` apart.
    fn two_rooms(thickness: i64, connections: Vec<Connection>) -> Plan {
        Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (128, 0), (128, 128)),
            ],
            connections,
            features: Vec::new(),
            wall_thickness: thickness,
        }
    }

    /// Compile a scenario plan with full roles.
    #[track_caller]
    fn compiled(plan: &Plan) -> CompiledPlan {
        compile(plan, map_name("MAP01"), MapFormat::Doom, &TestRoles::full())
            .expect("the scenario compiles")
    }

    /// Run the geometry checks plus the resource checks over a compiled
    /// map: a dressed plan must report nothing.
    #[track_caller]
    fn assert_valid(map: &Map) {
        let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
        let resources = ResourceIndex::default();
        let context = Context {
            definitions: &definitions,
            hexen_specials: &definitions,
            resources: &resources,
        };
        let enabled: BTreeSet<&'static str> = [
            "crossing_lines",
            "overlapping_lines",
            "two_sided_flag_mismatch",
            "zero_length_line",
            "duplicate_line",
            "missing_texture",
            "missing_flat",
        ]
        .into_iter()
        .collect();
        let issues = run(map, &context, Some(&enabled)).issues;
        assert!(issues.is_empty(), "the compiled map validates: {issues:?}");
    }

    /// Every sector of the finished map wears both its flats: the rooms
    /// from the theme and every band from the rooms it stands between.
    #[track_caller]
    fn assert_no_bare_flats(map: &Map) {
        for (index, space) in map.sectors.iter().enumerate() {
            assert_ne!(space.floor_flat, "-", "sector {index} keeps a floor flat");
            assert_ne!(
                space.ceiling_flat, "-",
                "sector {index} keeps a ceiling flat"
            );
        }
    }

    /// The two [`Sidedef`] rows of a two-sided line.
    fn rows<'map>(map: &'map Map, line: &Linedef) -> Option<(&'map Sidedef, &'map Sidedef)> {
        let front = map.sidedefs.get(line.front?)?;
        let back = map.sidedefs.get(line.back?)?;
        Some((front, back))
    }

    /// The front [`Sidedef`] row of a line, one- or two-sided.
    fn row_of<'map>(map: &'map Map, line: &Linedef) -> &'map Sidedef {
        let front = line.front.expect("emitted lines have a front");
        map.sidedefs.get(front).expect("test rows")
    }

    /// A two-sided line: both rows and the sectors they face.
    type TwoSided<'map> = (&'map Sidedef, &'map Sidedef, usize, usize);

    /// Every two-sided line's rows and the sectors they face.
    fn faces(map: &Map) -> Vec<TwoSided<'_>> {
        map.linedefs
            .iter()
            .filter_map(|line| {
                rows(map, line).map(|(front, back)| (front, back, front.sector, back.sector))
            })
            .collect()
    }

    /// The front rows of every one-sided line with the lines' flags.
    fn wall_faces(map: &Map) -> Vec<(&Sidedef, u32)> {
        map.linedefs
            .iter()
            .filter(|line| line.back.is_none())
            .map(|line| (row_of(map, line), line.flags))
            .collect()
    }

    /// Whether a side row belongs to one of the two room sectors.
    const fn is_room_side(row: &Sidedef) -> bool {
        row.sector == 0 || row.sector == 1
    }

    /// Whether sectors `one` and `two` form one of the stair run's
    /// risers: the lowest step against the lower room, or two
    /// consecutive steps.
    const fn is_riser(one: usize, two: usize) -> bool {
        let (low, high) = if one < two { (one, two) } else { (two, one) };
        matches!((low, high), (0, 2) | (2, 3) | (3, 4) | (4, 5))
    }

    /// Every face the neighbour's heights demand is dressed, and no
    /// `middle` on a two-sided line was painted: the fill pass must
    /// prove this for the whole map.
    #[track_caller]
    fn assert_no_gaps(map: &Map) {
        for (front, back, front_sector, back_sector) in faces(map) {
            let front_space = map.sectors.get(front_sector).expect("test indices");
            let back_space = map.sectors.get(back_sector).expect("test indices");
            assert_eq!(front.middle, "-", "two-sided middles stay bare");
            assert_eq!(back.middle, "-", "on both faces");
            assert_side_dressed(front, back_space, front_space, "front");
            assert_side_dressed(back, front_space, back_space, "back");
        }
    }

    /// One two-sided face: any band the sector across exposes must name
    /// a texture (sky ceilings excepted, which the engine draws).
    #[track_caller]
    fn assert_side_dressed(row: &Sidedef, across: &Sector, own: &Sector, which: &str) {
        let sky = |space: &Sector| space.ceiling_flat.eq_ignore_ascii_case("F_SKY1");
        if across.floor_height > own.floor_height {
            assert_ne!(
                row.lower, "-",
                "the {which} side's exposed floor needs a lower texture"
            );
        }
        if across.ceiling_height < own.ceiling_height && !(sky(own) && sky(across)) {
            assert_ne!(
                row.upper, "-",
                "the {which} side's exposed ceiling needs an upper texture"
            );
        }
    }

    /// The indices of the two-sided lines joining sectors `one` and
    /// `two`, in either order.
    fn lines_between(map: &Map, one: usize, two: usize) -> Vec<usize> {
        map.linedefs
            .iter()
            .enumerate()
            .filter_map(|(index, line)| joined(map, line, one, two).then_some(index))
            .collect()
    }

    /// Whether `line` is two-sided between sectors `one` and `two`.
    fn joined(map: &Map, line: &Linedef, one: usize, two: usize) -> bool {
        let Some((front, back)) = rows(map, line) else {
            return false;
        };
        (front.sector == one && back.sector == two) || (front.sector == two && back.sector == one)
    }

    /// Count the sides of the whole map whose `lower` texture is exactly
    /// `name`.
    fn lower_sides_with(map: &Map, name: &str) -> usize {
        map.sidedefs.iter().filter(|row| row.lower == name).count()
    }

    /// The row of the pair that faces the lower of the two sectors.
    fn lower_row<'map>(map: &Map, front: &'map Sidedef, back: &'map Sidedef) -> &'map Sidedef {
        let floor_of = |side: &Sidedef| {
            map.sectors
                .get(side.sector)
                .expect("test indices")
                .floor_height
        };
        if floor_of(front) < floor_of(back) {
            front
        } else {
            back
        }
    }

    /// A door line shows the leaf above the collapsed band on the
    /// room's side and nothing on the band's own side.
    #[track_caller]
    fn assert_door_line(map: &Map, line: &Linedef) {
        let (front, back) = rows(map, line).expect("door lines are two-sided");
        for row in [front, back].into_iter().filter(|row| is_room_side(row)) {
            assert_eq!(row.upper, "DOOROLE", "the leaf sits on the room's side");
        }
        for row in [front, back].into_iter().filter(|row| !is_room_side(row)) {
            assert_eq!(row.upper, "-", "the band side faces the open doorway");
        }
    }

    /// A window line blocks, and its room sides carry sill below and
    /// lintel above the glass box.
    #[track_caller]
    fn assert_window_line(map: &Map, line: &Linedef) {
        assert_eq!(
            line.flags & (1_u32 | 4_u32),
            1_u32 | 4_u32,
            "blocking and two-sided"
        );
        let (front, back) = rows(map, line).expect("window lines are two-sided");
        for row in [front, back].into_iter().filter(|row| is_room_side(row)) {
            assert_eq!(row.lower, "WSIROLE", "the room side gets the sill");
            assert_eq!(row.upper, "WALROLE", "and the lintel");
        }
        for row in [front, back].into_iter().filter(|row| !is_room_side(row)) {
            assert_eq!(row.lower, "-", "the glass box needs nothing of its own");
            assert_eq!(row.upper, "-", "it is open to the wall on both bands");
        }
    }

    #[test]
    fn an_opening_between_two_rooms_dresses_every_wall_and_stays_clean() {
        let plan = two_rooms(16, vec![opening("a", "b")]);
        let out = compiled(&plan);
        let map = &out.map;
        assert_eq!(map.sectors.len(), 3, "two rooms plus the band");
        assert_eq!(
            out.room_sectors.get("a"),
            Some(&vec![0_usize]),
            "rooms keep plan order"
        );
        assert_eq!(out.room_sectors.get("b"), Some(&vec![1_usize]));
        let info = out.connector_sectors.first().expect("the opening");
        assert_eq!(info.id, "a->b", "the unnamed connection label");
        assert_eq!(info.kind, ConnectionKind::Opening, "kind rides along");
        assert_eq!(info.sectors, vec![2], "one band sector");
        assert_eq!(info.door_lines.len(), 2, "the band meets each room once");
        assert_eq!(
            one_sided_at(map, 120),
            2,
            "the void wall stands at x=120, split below and above the opening"
        );
        assert_eq!(one_sided_at(map, 136), 2, "and at x=136 likewise");
        for (row, flags) in wall_faces(map) {
            assert_eq!(
                row.middle, "WALROLE",
                "every face to the void (room walls and the opening's jambs) is walled"
            );
            assert_eq!(flags, 1_u32, "one-sided lines just block");
        }
        for index in [0_usize, 1] {
            let space = map.sectors.get(index).expect("a room sector");
            assert_eq!(
                space.floor_flat, "FLOROLE",
                "unset room floors take the role"
            );
            assert_eq!(space.ceiling_flat, "CEIROLE", "and so do the ceilings");
        }
        let band = map.sectors.get(2).expect("the band sector");
        assert_eq!(
            band.floor_flat, "FLOROLE",
            "an opening's band inherits the room standing at its floor"
        );
        assert_eq!(
            band.ceiling_flat, "CEIROLE",
            "and the room standing at its ceiling"
        );
        assert_no_bare_flats(map);
        assert_no_gaps(map);
        assert!(
            out.warnings.is_empty(),
            "the plan is sound: {:?}",
            out.warnings
        );
        assert_valid(map);
    }

    /// The shared `x` of a vertical line, `None` otherwise.
    fn vertical_x(map: &Map, line: &Linedef) -> Option<i32> {
        let first = map.vertices.get(line.v1)?;
        let second = map.vertices.get(line.v2)?;
        (first.x == second.x).then_some(first.x)
    }

    /// The one-sided lines standing on the vertical grid line `x`.
    fn one_sided_at(map: &Map, x: i32) -> usize {
        map.linedefs
            .iter()
            .filter(|line| line.back.is_none() && vertical_x(map, line) == Some(x))
            .count()
    }

    #[test]
    fn a_full_width_opening_joins_two_touching_rooms_with_no_extra_sector() {
        let mut arch = opening("a", "b");
        arch.id = Some("arch".to_owned());
        arch.width = 128_i64;
        let out = compiled(&two_rooms(16, vec![arch]));
        let map = &out.map;
        assert_eq!(map.sectors.len(), 2, "the two rooms and nothing between");
        let info = out.connector_sectors.first().expect("the arch");
        assert_eq!(info.id, "arch", "the connection still reports under its id");
        assert!(
            info.sectors.is_empty(),
            "an opening across the whole run owns no sector"
        );
        assert!(
            info.door_lines.is_empty(),
            "and no line to hang a special on"
        );
        let shared = lines_between(map, 0, 1);
        assert_eq!(
            shared.len(),
            1,
            "the rooms share their whole boundary as one line"
        );
        assert_eq!(
            wall_faces(map).len(),
            6,
            "the outer walls of the two rooms are all that stands"
        );
        assert_eq!(one_sided_at(map, 120), 0, "no stub wall at the inner face");
        assert_eq!(one_sided_at(map, 136), 0, "and none at the other one");
        assert_eq!(
            out.room_sectors.get("a"),
            Some(&vec![0_usize]),
            "both rooms keep their own sector"
        );
        assert_eq!(out.room_sectors.get("b"), Some(&vec![1_usize]));
        assert_no_bare_flats(map);
        assert_no_gaps(map);
        assert!(
            out.warnings.is_empty(),
            "the join needs no note: {:?}",
            out.warnings
        );
        assert_valid(map);
    }

    #[test]
    fn a_full_width_opening_at_zero_thickness_joins_the_rooms_too() {
        let mut arch = opening("a", "b");
        arch.width = 128_i64;
        let out = compiled(&two_rooms(0, vec![arch]));
        let map = &out.map;
        assert_eq!(
            map.sectors.len(),
            2,
            "the band sector that used to sit here is gone"
        );
        assert_eq!(
            lines_between(map, 0, 1).len(),
            1,
            "the rooms share their line whole"
        );
        assert_no_bare_flats(map);
        assert_valid(map);
    }

    /// The reported repro: an `L`-shaped `north` room and a `south` room
    /// touching it along one edge, both drawn in image units and carried
    /// into map units by the caller's scale, vertical mirror and `16`
    /// snap grid. The shared run lands `304` map units long; `width` is
    /// the opening the caller asked for.
    fn repro_plan(width: i64) -> Plan {
        let north = Room {
            id: "north".to_owned(),
            shape: Shape::Polygon {
                points: vec![
                    (100, 140),
                    (725, 140),
                    (725, 610),
                    (585, 610),
                    (585, 335),
                    (100, 335),
                ],
            },
            floor: 0,
            ceiling: 128,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        };
        let mut plan = Plan {
            rooms: vec![north, rect_room("south", (64, 410), (521, 200))],
            connections: vec![opening("north", "south")],
            features: Vec::new(),
            wall_thickness: 16_i64,
        };
        if let Some(connection) = plan.connections.first_mut() {
            connection.width = width;
        }
        let units = Units {
            scale_num: 3_i64,
            scale_den: 2_i64,
            origin_x: 0_i64,
            origin_y: 0_i64,
            grid: 16_i64,
            flip_y: true,
        };
        transform_plan(&plan, &units, units.grid).expect("the repro transform runs")
    }

    #[test]
    fn a_repro_opening_across_the_whole_snapped_edge_joins_the_rooms() {
        let out = compiled(&repro_plan(304_i64));
        assert_eq!(
            out.map.sectors.len(),
            2,
            "no 16x304 sliver stands between the rooms"
        );
        assert!(
            out.connector_sectors
                .first()
                .expect("the opening")
                .sectors
                .is_empty(),
            "the full-width opening owns no sector"
        );
        assert_eq!(
            lines_between(&out.map, 0, 1).len(),
            1,
            "the rooms share their snapped edge as one open line"
        );
        assert_no_bare_flats(&out.map);
        assert_no_gaps(&out.map);
        assert!(
            out.warnings.is_empty(),
            "the join needs no note: {:?}",
            out.warnings
        );
        assert_valid(&out.map);
    }

    /// The endpoints of every one-sided vertical line standing on one of
    /// the `x` coordinates in `faces`.
    fn wall_face_points(map: &Map, faces: &[i32]) -> Vec<(i32, i32)> {
        map.linedefs
            .iter()
            .filter(|row| row.back.is_none())
            .filter(|row| vertical_x(map, row).is_some_and(|x| faces.contains(&x)))
            .flat_map(|row| [row.v1, row.v2])
            .map(|index| {
                let vertex = map.vertices.get(index).expect("test indices");
                (vertex.x, vertex.y)
            })
            .collect()
    }

    #[test]
    fn a_repro_opening_short_of_the_edge_keeps_one_dressed_band() {
        let out = compiled(&repro_plan(288_i64));
        assert_eq!(
            out.map.sectors.len(),
            3,
            "two rooms plus the one band the wall still needs"
        );
        assert_eq!(
            out.connector_sectors.first().expect("the opening").sectors,
            vec![2_usize],
            "the narrower opening keeps its sector"
        );
        let band = out.map.sectors.get(2).expect("the band sector");
        assert_eq!(band.floor_flat, "FLOROLE", "the band inherits the rooms");
        for (x, y) in wall_face_points(&out.map, &[872_i32, 888_i32]) {
            assert!(
                (-912_i32..=(-608_i32)).contains(&y),
                "the wall face at x={x} reaches past the shared run's ends"
            );
        }
        assert_no_bare_flats(&out.map);
        assert_no_gaps(&out.map);
        assert_valid(&out.map);
    }

    #[test]
    fn a_narrow_band_inherits_the_room_standing_at_each_of_its_surfaces() {
        let mut plan = two_rooms(16, vec![opening("a", "b")]);
        if let Some(room) = plan.rooms.get_mut(0) {
            room.floor_flat = Some("floor4_8".to_owned());
            room.ceiling_flat = Some("ceil5_1".to_owned());
        }
        if let Some(room) = plan.rooms.get_mut(1) {
            room.floor = 16_i32;
            room.floor_flat = Some("FLAT10".to_owned());
        }
        let out = compiled(&plan);
        let band = out.map.sectors.get(2).expect("the band sector");
        assert_eq!(band.floor_height, 16_i32, "the higher floor spans the band");
        assert_eq!(
            band.floor_flat, "FLAT10",
            "so room b's floor continues through it"
        );
        assert_eq!(
            band.ceiling_flat, "CEIL5_1",
            "and room a owns the ceiling both rooms stand at"
        );
        assert_eq!(band.light, 160_i32, "the dimmer of the two room lights");
        assert_no_bare_flats(&out.map);
        assert_no_gaps(&out.map);
        assert_valid(&out.map);
    }

    #[test]
    fn a_band_between_two_outdoor_rooms_keeps_the_sky_it_reaches() {
        let mut plan = two_rooms(16, vec![opening("a", "b")]);
        for room in &mut plan.rooms {
            room.outdoor = true;
        }
        if let Some(room) = plan.rooms.get_mut(1) {
            room.ceiling = 256_i32;
        }
        let out = compiled(&plan);
        let band = out.map.sectors.get(2).expect("the band sector");
        assert_eq!(band.ceiling_height, 128_i32, "the lower ceiling spans it");
        assert_eq!(
            band.ceiling_flat, "F_SKY1",
            "and the room standing at that ceiling opens to the sky"
        );
        assert_no_bare_flats(&out.map);
        assert_valid(&out.map);
    }

    #[test]
    fn a_room_wall_override_beats_the_theme() {
        let mut plan = two_rooms(16, vec![opening("a", "b")]);
        if let Some(room) = plan.rooms.get_mut(1) {
            room.wall_texture = Some("brick".to_owned());
        }
        let out = compiled(&plan);
        let overridden = out
            .map
            .sidedefs
            .iter()
            .filter(|row| row.middle == "BRICK")
            .count();
        assert!(overridden > 0, "room b's walls take its override");
        let themed = out
            .map
            .sidedefs
            .iter()
            .filter(|row| row.middle == "WALROLE")
            .count();
        assert!(themed > 0, "room a and the jambs still take the role");
        assert_valid(&out.map);
    }

    #[test]
    fn a_door_dresses_its_leaf_track_and_flats() {
        let kind = ConnectionKind::Door {
            style: DoorStyle::Normal,
        };
        let plan = two_rooms(16, vec![connection("a", "b", kind)]);
        let out = compiled(&plan);
        let map = &out.map;
        let info = out.connector_sectors.first().expect("the door");
        assert_eq!(
            info.kind,
            ConnectionKind::Door {
                style: DoorStyle::Normal
            },
            "the door kind survives placement"
        );
        assert_eq!(info.sectors, vec![2], "one closed band sector");
        let band = map.sectors.get(2).expect("the band sector");
        assert_eq!(band.floor_height, band.ceiling_height, "the door is closed");
        assert_eq!(band.floor_flat, "DFOROLE", "the track floor");
        assert_eq!(band.ceiling_flat, "DFOROLE", "and above it");
        assert_eq!(
            info.door_lines.len(),
            2,
            "the band meets each wall face once"
        );
        for &index in &info.door_lines {
            assert_door_line(map, map.linedefs.get(index).expect("recorded index"));
        }
        let jambs: Vec<(&Sidedef, u32)> = wall_faces(map)
            .into_iter()
            .filter(|&(row, _)| row.sector == 2)
            .collect();
        assert_eq!(
            jambs.len(),
            4,
            "the band's four reveal edges inside the wall"
        );
        for (row, flags) in jambs {
            assert_eq!(row.middle, "DTRROLE", "the jamb shows the track");
            assert_ne!(flags & 16_u32, 0, "the track is lower-unpegged");
        }
        let plain_walls = wall_faces(map)
            .into_iter()
            .filter(|&(row, _)| row.sector != 2);
        for (row, flags) in plain_walls {
            assert_eq!(row.middle, "WALROLE", "room walls keep the plain role");
            assert_eq!(flags & 16_u32, 0, "plain walls carry no track bit");
        }
        assert_no_gaps(map);
        assert_valid(map);
    }

    #[test]
    fn a_window_dresses_sill_and_lintel_and_blocks_the_way_through() {
        let kind = ConnectionKind::Window {
            sill: 32_i32,
            lintel: 24_i32,
        };
        let plan = two_rooms(16, vec![connection("a", "b", kind)]);
        let out = compiled(&plan);
        let map = &out.map;
        let info = out.connector_sectors.first().expect("the window");
        assert_eq!(info.sectors, vec![2]);
        let glass = map.sectors.get(2).expect("the glass box");
        assert_eq!(glass.floor_height, 32_i32, "the sill lifts the floor");
        assert_eq!(
            glass.ceiling_height, 104_i32,
            "the lintel drops the ceiling"
        );
        assert_eq!(
            info.door_lines.len(),
            2,
            "the band meets each wall face once"
        );
        for &index in &info.door_lines {
            assert_window_line(map, map.linedefs.get(index).expect("recorded index"));
        }
        assert_no_gaps(map);
        assert_valid(map);
    }

    #[test]
    fn a_window_too_tall_for_the_gap_fails_the_compile() {
        let kind = ConnectionKind::Window {
            sill: 100_i32,
            lintel: 100_i32,
        };
        let plan = two_rooms(16, vec![connection("a", "b", kind)]);
        let error = compile(
            &plan,
            map_name("MAP01"),
            MapFormat::Doom,
            &TestRoles::full(),
        )
        .expect_err("there is no glass left");
        assert_eq!(error.code(), "invalid_argument", "the span is an argument");
        assert!(
            error.to_string().contains("glass"),
            "the message explains the squeeze: {error}"
        );
    }

    #[test]
    fn stairs_dress_every_riser_on_its_low_side() {
        let kind = ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 24_i32,
        };
        let mut plan = two_rooms(16, vec![connection("a", "b", kind)]);
        if let Some(room) = plan.rooms.get_mut(1) {
            room.floor = 64_i32;
            room.ceiling = 160_i32;
        }
        let out = compiled(&plan);
        let map = &out.map;
        assert_eq!(map.sectors.len(), 6, "two rooms plus four steps");
        let info = out.connector_sectors.first().expect("the stair run");
        assert_eq!(info.sectors, vec![2, 3, 4, 5], "steps in plan order");
        let floors: Vec<i32> = (2_usize..6_usize)
            .map(|index| map.sectors.get(index).expect("a step sector").floor_height)
            .collect();
        assert_eq!(
            floors,
            vec![16_i32, 32_i32, 48_i32, 64_i32],
            "four rising steps"
        );
        let risered = faces(map)
            .into_iter()
            .filter(|&(_, _, one, two)| is_riser(one, two));
        let mut dressed = 0_usize;
        for (front, back, _, _) in risered {
            dressed = dressed.saturating_add(1);
            assert_eq!(
                lower_row(map, front, back).lower,
                "STEPROLE",
                "the riser faces the lower sector"
            );
        }
        assert_eq!(dressed, 6, "three room edges plus three step joins");
        assert_eq!(
            lower_sides_with(map, "STEPROLE"),
            6,
            "and the step texture dresses no other face"
        );
        assert_no_gaps(map);
        assert_valid(map);
    }

    #[test]
    fn zero_thickness_rooms_share_their_walls_and_the_opening_still_works() {
        let plan = two_rooms(0, vec![opening("a", "b")]);
        let out = compiled(&plan);
        let map = &out.map;
        assert_eq!(map.sectors.len(), 3, "the band still gets its own sector");
        let shared = lines_between(map, 0, 1);
        assert_eq!(
            shared.len(),
            2,
            "the rooms meet directly on x=128 below and above the band"
        );
        assert!(
            !lines_between(map, 0, 2).is_empty() && !lines_between(map, 2, 1).is_empty(),
            "the depth-16 band still cuts both rooms"
        );
        assert_eq!(
            out.connector_sectors.first().expect("the opening").sectors,
            vec![2]
        );
        assert_no_gaps(map);
        assert_valid(map);
    }

    #[test]
    fn a_later_room_paints_over_the_l_shaped_one() {
        let court = Room {
            id: "court".to_owned(),
            shape: Shape::Polygon {
                points: vec![(0, 0), (192, 0), (192, 64), (64, 64), (64, 192), (0, 192)],
            },
            floor: 0,
            ceiling: 128,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        };
        let lab = rect_room("lab", (96, 0), (96, 96));
        let mut passage = opening("court", "lab");
        passage.width = 32_i64;
        let plan = Plan {
            rooms: vec![court, lab],
            connections: vec![passage],
            features: Vec::new(),
            wall_thickness: 16_i64,
        };
        plan.validate().expect("the plan validates");
        let layers = room_layers(&plan);
        let labeler = LayerLabeler::new(&layers);
        let scale = |x: i64, y: i64| {
            (
                i128::from(x).saturating_mul(SAMPLE_SCALE),
                i128::from(y).saturating_mul(SAMPLE_SCALE),
            )
        };
        assert_eq!(
            labeler.label(scale(120, 40)),
            Label::Region(1),
            "the later rectangle wins where they overlap"
        );
        assert_eq!(
            labeler.label(scale(20, 120)),
            Label::Region(0),
            "the L survives uncovered"
        );
        let out = compiled(&plan);
        assert_eq!(out.map.sectors.len(), 3, "two rooms plus the band");
        assert!(
            out.room_sectors.contains_key("court") && out.room_sectors.contains_key("lab"),
            "both rooms own sectors: {:?}",
            out.room_sectors
        );
        assert_no_gaps(&out.map);
        assert_valid(&out.map);
    }

    #[test]
    fn rooms_that_do_not_touch_cannot_be_connected() {
        let plan = Plan {
            rooms: vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (400, 0), (128, 128)),
            ],
            connections: vec![opening("a", "b")],
            features: Vec::new(),
            wall_thickness: 16_i64,
        };
        let error = compile(
            &plan,
            map_name("MAP01"),
            MapFormat::Doom,
            &TestRoles::full(),
        )
        .expect_err("nothing joins the two rooms");
        assert_eq!(error.code(), "invalid_argument", "placement is an argument");
        assert!(
            error.to_string().contains("do not touch"),
            "the message names the fault: {error}"
        );
    }

    #[test]
    fn an_unreachable_room_is_warned_about_by_name() {
        let mut plan = two_rooms(16, vec![opening("a", "b")]);
        plan.rooms.push(rect_room("attic", (0, 200), (128, 128)));
        let out = compiled(&plan);
        assert_eq!(out.warnings.len(), 1, "only the orphan reports");
        assert!(
            out.warnings
                .first()
                .is_some_and(|note| note.contains("attic") && note.contains("'a'")),
            "the note names the room and the root: {:?}",
            out.warnings
        );
        assert_valid(&out.map);
    }

    #[test]
    fn a_hundred_unit_floor_gap_through_an_opening_warns() {
        let mut plan = two_rooms(16, vec![opening("a", "b")]);
        if let Some(room) = plan.rooms.get_mut(1) {
            room.floor = 100_i32;
        }
        let out = compiled(&plan);
        assert_eq!(
            out.warnings,
            vec!["step taller than 24 between a and b; use stairs".to_owned()],
            "the exact height warning"
        );
        assert_no_gaps(&out.map);
        assert_valid(&out.map);
    }

    #[test]
    fn a_fully_covered_room_earns_a_warning() {
        let plan = Plan {
            rooms: vec![
                rect_room("under", (0, 0), (64, 64)),
                rect_room("over", (0, 0), (128, 128)),
            ],
            connections: Vec::new(),
            features: Vec::new(),
            wall_thickness: 16_i64,
        };
        let out = compiled(&plan);
        assert_eq!(out.map.sectors.len(), 1, "only the covering room survives");
        assert_eq!(out.room_sectors.len(), 1, "one owner");
        assert!(
            out.room_sectors.contains_key("over"),
            "the buried room owns no sector"
        );
        assert!(
            out.warnings
                .iter()
                .any(|note| note.contains("under") && note.contains("painted over")),
            "the note names the buried room: {:?}",
            out.warnings
        );
        assert_valid(&out.map);
    }

    #[test]
    fn missing_roles_leave_the_dressed_map_bare_without_erroring() {
        let kind = ConnectionKind::Door {
            style: DoorStyle::Fast,
        };
        let plan = two_rooms(16, vec![connection("a", "b", kind)]);
        let out = compile(
            &plan,
            map_name("MAP01"),
            MapFormat::Doom,
            &TestRoles::missing(),
        )
        .expect("a theme that answers nothing is not an error");
        for row in &out.map.sidedefs {
            assert_eq!(
                (row.upper.as_str(), row.lower.as_str(), row.middle.as_str()),
                ("-", "-", "-"),
                "every face keeps the emission's placeholder"
            );
        }
        for space in &out.map.sectors {
            assert_eq!(
                (space.floor_flat.as_str(), space.ceiling_flat.as_str()),
                ("-", "-")
            );
        }
        assert_valid(&out.map);
    }
}
