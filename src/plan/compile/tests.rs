//! Shared helpers for the plan compiler's tests: the [`TestRoles`] theme,
//! the rectangle-room, [`opening`] and [`connection`] builders behind every
//! plan the topics compile, the [`compiled`] and [`assert_valid`] readers,
//! the [`issues_for`] gate on the pure validation pass and the sidedef,
//! band and riser lookups the dressing asserts run through.
//!
//! The topics are the pipeline's own claims: [`openings`] the room-to-room
//! openings, the full-width joins with no band sector and the two repro
//! plans; [`dressing`] the bands' inherited flats, the doors, windows,
//! stair risers and the theme's overrides; [`warnings`] the advisory notes
//! a compile answers with.

mod dressing;
mod openings;
mod warnings;

use super::{CompiledPlan, compile};
use crate::defs::definitions_for;
use crate::plan::model::{Connection, ConnectionKind, Plan, Room, Shape};
use crate::plan::textures::{TextureRole, TextureRoles};
use crate::wad::classify::MapFormat;
use crate::wad::game::Game;
use crate::wad::map::validate::{Context, Issue, ResourceIndex, run};
use crate::wad::map::{Linedef, Map, Sector, Sidedef};
use crate::wad::name::LumpName;
use alloc::collections::BTreeSet;

/// A theme handing every role a distinct uppercase eight-character name.
pub(super) struct TestRoles {
    /// When `false` no role answers and every face stays `"-"`.
    pub(super) complete: bool,
}

impl TestRoles {
    /// Roles answering every [`TextureRole`].
    pub(super) fn full() -> Self {
        Self { complete: true }
    }

    /// Roles answering none.
    pub(super) fn missing() -> Self {
        Self { complete: false }
    }
}

impl TextureRoles for TestRoles {
    fn texture(&self, role: TextureRole) -> Option<&str> {
        self.complete.then_some(role_name(role))
    }
}

/// The fixture name for one role, distinct per slot.
pub(super) fn role_name(role: TextureRole) -> &'static str {
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
pub(super) fn map_name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test map names are valid")
}

/// A rectangular room with default sector properties.
pub(super) fn rect_room(id: &str, left_bottom: (i64, i64), width_height: (i64, i64)) -> Room {
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
pub(super) fn opening(from: &str, to: &str) -> Connection {
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
pub(super) fn connection(from: &str, to: &str, kind: ConnectionKind) -> Connection {
    let mut passage = opening(from, to);
    passage.kind = kind;
    passage
}

/// Two 128 x 128 rooms sharing the line x = 128, `thickness` apart.
pub(super) fn two_rooms(thickness: i64, connections: Vec<Connection>) -> Plan {
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

/// The identity every compile test emits into: `MAP01`, `Doom` layout,
/// the vanilla sky flat.
pub(super) fn identity() -> crate::plan::emit::MapIdentity {
    crate::plan::emit::MapIdentity {
        name: map_name("MAP01"),
        format: MapFormat::Doom,
        sky_flat: crate::wad::map::consts::SKY_FLAT,
    }
}

/// Compile a scenario plan with full roles.
#[track_caller]
pub(super) fn compiled(plan: &Plan) -> CompiledPlan {
    compile(plan, identity(), &TestRoles::full()).expect("the scenario compiles")
}

/// The codes [`assert_valid`] runs: geometry plus the resource lookups.
pub(super) const VALIDATED_CODES: [&str; 7] = [
    "crossing_lines",
    "overlapping_lines",
    "two_sided_flag_mismatch",
    "zero_length_line",
    "duplicate_line",
    "missing_texture",
    "missing_flat",
];

/// The issues of the `codes` checks over `map`, on the built-in tables
/// and a resource index carrying nothing.
#[track_caller]
pub(super) fn issues_for(map: &Map, codes: &[&'static str]) -> Vec<Issue> {
    let definitions = definitions_for(Game::Doom).expect("the embedded tables parse");
    let resources = ResourceIndex::default();
    let context = Context {
        definitions: &definitions,
        hexen_specials: &definitions,
        resources: &resources,
        sky_flat: crate::wad::map::consts::SKY_FLAT,
    };
    let enabled: BTreeSet<&'static str> = codes.iter().copied().collect();
    run(map, &context, Some(&enabled)).issues
}

/// Run the geometry checks plus the resource checks over a compiled
/// map: a dressed plan must report nothing.
#[track_caller]
pub(super) fn assert_valid(map: &Map) {
    let issues = issues_for(map, &VALIDATED_CODES);
    assert!(issues.is_empty(), "the compiled map validates: {issues:?}");
}

/// Every sector of the finished map wears both its flats: the rooms
/// from the theme and every band from the rooms it stands between.
#[track_caller]
pub(super) fn assert_no_bare_flats(map: &Map) {
    for (index, space) in map.sectors.iter().enumerate() {
        assert_ne!(space.floor_flat, "-", "sector {index} keeps a floor flat");
        assert_ne!(
            space.ceiling_flat, "-",
            "sector {index} keeps a ceiling flat"
        );
    }
}

/// The two [`Sidedef`] rows of a two-sided line.
pub(super) fn rows<'map>(map: &'map Map, line: &Linedef) -> Option<(&'map Sidedef, &'map Sidedef)> {
    let front = map.sidedefs.get(line.front?)?;
    let back = map.sidedefs.get(line.back?)?;
    Some((front, back))
}

/// The front [`Sidedef`] row of a line, one- or two-sided.
pub(super) fn row_of<'map>(map: &'map Map, line: &Linedef) -> &'map Sidedef {
    let front = line.front.expect("emitted lines have a front");
    map.sidedefs.get(front).expect("test rows")
}

/// A two-sided line: both rows and the sectors they face.
pub(super) type TwoSided<'map> = (&'map Sidedef, &'map Sidedef, usize, usize);

/// Every two-sided line's rows and the sectors they face.
pub(super) fn faces(map: &Map) -> Vec<TwoSided<'_>> {
    map.linedefs
        .iter()
        .filter_map(|line| {
            rows(map, line).map(|(front, back)| (front, back, front.sector, back.sector))
        })
        .collect()
}

/// The front rows of every one-sided line with the lines' flags.
pub(super) fn wall_faces(map: &Map) -> Vec<(&Sidedef, u32)> {
    map.linedefs
        .iter()
        .filter(|line| line.back.is_none())
        .map(|line| (row_of(map, line), line.flags))
        .collect()
}

/// Whether a side row belongs to one of the two room sectors.
pub(super) const fn is_room_side(row: &Sidedef) -> bool {
    row.sector == 0 || row.sector == 1
}

/// Whether sectors `one` and `two` form one of the stair run's
/// risers: the lowest step against the lower room, or two
/// consecutive steps.
pub(super) const fn is_riser(one: usize, two: usize) -> bool {
    let (low, high) = if one < two { (one, two) } else { (two, one) };
    matches!((low, high), (0, 2) | (2, 3) | (3, 4) | (4, 5))
}

/// Every face the neighbour's heights demand is dressed, and no
/// `middle` on a two-sided line was painted: the fill pass must
/// prove this for the whole map.
#[track_caller]
pub(super) fn assert_no_gaps(map: &Map) {
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
pub(super) fn assert_side_dressed(row: &Sidedef, across: &Sector, own: &Sector, which: &str) {
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
pub(super) fn lines_between(map: &Map, one: usize, two: usize) -> Vec<usize> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| joined(map, line, one, two).then_some(index))
        .collect()
}

/// Whether `line` is two-sided between sectors `one` and `two`.
pub(super) fn joined(map: &Map, line: &Linedef, one: usize, two: usize) -> bool {
    let Some((front, back)) = rows(map, line) else {
        return false;
    };
    (front.sector == one && back.sector == two) || (front.sector == two && back.sector == one)
}

/// Count the sides of the whole map whose `lower` texture is exactly
/// `name`.
pub(super) fn lower_sides_with(map: &Map, name: &str) -> usize {
    map.sidedefs.iter().filter(|row| row.lower == name).count()
}

/// The row of the pair that faces the lower of the two sectors.
pub(super) fn lower_row<'map>(
    map: &Map,
    front: &'map Sidedef,
    back: &'map Sidedef,
) -> &'map Sidedef {
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
pub(super) fn assert_door_line(map: &Map, line: &Linedef) {
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
pub(super) fn assert_window_line(map: &Map, line: &Linedef) {
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
