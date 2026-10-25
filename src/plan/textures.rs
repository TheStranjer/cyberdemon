//! Texture roles: dressing an emitted plan map in the theme's names.
//!
//! [`emit`](crate::plan::emit::emit) leaves every side texture "-" and
//! every un-overridden flat "-". This slice names the eight
//! [`TextureRole`] slots a plan map needs and paints them onto the faces
//! the arrangement says they belong to: room walls and flats, door
//! tracks and leaves, window sills and lintels, stair risers, and a
//! final fill pass that leaves no height gap undressed. The theme is the
//! [`TextureRoles`] trait, so the caller (a tool over the session's
//! [`themes`](crate::themes) layer) picks the names; a role the theme
//! cannot answer keeps its "-".
//!
//! [`apply_textures`] is a pure mutation of an already-emitted [`Map`]:
//! it reads [`Emitted::pieces`] for the region label behind every face
//! and [`ConnectorGeometry`] for each band's kind, and never arranges
//! anything again. A non-door band wears the flats of the rooms it stands
//! between rather than a role of its own, so a passage continues the room
//! it leads out of instead of standing in bare "-" flats. Nothing here
//! touches archives, sessions or `MCP` types.
//!
//! The work is split: this root keeps the roles the dressing answers
//! from - [`TextureRole`], the [`TextureRoles`] theme and the [`Scene`]
//! borrow bag - beside [`apply_textures`] itself, while the private `dress`
//! module holds the faces it resolves a piece into, the `Dresser` that owns
//! one pass' names and band index, and every rule that writes a sidedef or
//! sector row.

mod dress;

use crate::error::CyberError;
use crate::plan::connectors::ConnectorGeometry;
use crate::plan::emit::Emitted;
use crate::plan::model::Plan;
use crate::wad::map::Map;

/// A named slot in the theme a plan map is dressed from.
///
/// Every role is a face class the plan compiler can identify from the
/// arrangement alone; which lump name answers a role is the theme's
/// decision, made through [`TextureRoles`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureRole {
    /// A room's floor flat, when the plan gives the room none.
    Floor,
    /// A room's ceiling flat, when the plan gives the room none and the
    /// room is not open to the sky.
    Ceiling,
    /// A plain wall face: a room's one-sided walls, an opening's jamb
    /// faces inside the void wall, and every two-sided band the fill
    /// pass exposes.
    Wall,
    /// The `upper` band on the room side of a door line: the face the
    /// closed leaf covers, over the collapsed door sector.
    Door,
    /// A door sector's floor and ceiling flats: the track the leaf
    /// slides along, seen from both ends of the band.
    DoorFloor,
    /// The one-sided `middle` of a door band's jamb faces inside the
    /// void wall: the track the leaf disappears into.
    DoorTrack,
    /// The `lower` band on the room side of a window line: the sill
    /// under the glass.
    WindowSill,
    /// The `lower` band of a stair riser, on the side of the lower
    /// sector the step rises above.
    Step,
}

/// The caller's theme: lump names for the [`TextureRole`] slots.
///
/// [`apply_textures`] consults it for every face that needs a name. A
/// role the theme does not answer leaves its faces `"-"` and is not an
/// error: the plan core never invents names, and in practice the
/// session's theme layer guarantees a name for every slot a plan can
/// ask about.
pub trait TextureRoles {
    /// The texture or flat name answering `role`, or `None` to leave
    /// the face `"-"`.
    fn texture(&self, role: TextureRole) -> Option<&str>;
}

/// The plan, connectors and emission one dressing pass reads together.
///
/// The three always travel as a unit — the [rules](apply_textures) name
/// a face by its region label, and only the plan and the placed
/// connectors say what that label is — so they ride in one borrow bag.
/// They must be the exact trio [`emit`](crate::plan::emit::emit)
/// produced: pieces and linedefs share one index, and foreign inputs
/// would dress the wrong faces.
#[derive(Clone, Copy, Debug)]
pub struct Scene<'plan> {
    /// The plan whose rooms and connections describe the map.
    pub plan: &'plan Plan,
    /// The placed connectors [`emit`](crate::plan::emit::emit) cut the
    /// map with, in plan order.
    pub connectors: &'plan [ConnectorGeometry],
    /// The emission being dressed: its [`Emitted::pieces`] label every
    /// face and its [`Emitted::sector_of_label`] owns every sector.
    pub emitted: &'plan Emitted,
}

/// Dress `map` — the emission behind `scene` — in the textures `roles`
/// names, in place.
///
/// The pass walks [`Emitted::pieces`] (one per linedef, in the same
/// order) so every face is classified by the region label the
/// arrangement left on it, and dresses: room sectors (wall override
/// else [`TextureRole::Wall`] on the one-sided faces, unset flats from
/// [`TextureRole::Floor`] and [`TextureRole::Ceiling`], never over an
/// `outdoor` room's sky); an opening's void-facing jambs with
/// [`TextureRole::Wall`]; door bands ([`TextureRole::DoorFloor`] flats,
/// [`TextureRole::Door`] `upper` on the room side of the door lines,
/// [`TextureRole::DoorTrack`] and the lower-unpegged bit on the
/// one-sided jambs); window bands ([`TextureRole::WindowSill`] `lower`
/// and [`TextureRole::Wall`] `upper` on the room side, blocking and
/// two-sided flags on the line); stair risers
/// ([`TextureRole::Step`] `lower` on the lower sector's side); every
/// other band's flats, inherited from the adjoining rooms it stands
/// between; and a general fill that gives every remaining two-sided face
/// its neighbour heights demand [`TextureRole::Wall`], leaving `middle`
/// faces of two-sided lines alone. A stricter rule's name is never
/// overwritten by the fill, and a role the theme left `None` keeps its
/// face `"-"`.
///
/// `map` must be the map `scene.emitted` holds: linedef order is matched
/// against [`Emitted::pieces`].
///
/// # Errors
///
/// [`CyberError::Internal`] when a piece has no linedef or sidedef row
/// behind it, or a face's region label owns no sector: the signature of
/// a `map` that is not `scene.emitted`'s own or was rebuilt after the
/// emission.
#[inline]
pub fn apply_textures(
    map: &mut Map,
    scene: &Scene<'_>,
    roles: &dyn TextureRoles,
) -> Result<(), CyberError> {
    dress::apply(map, scene, roles)
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use super::{Scene, TextureRole, TextureRoles, apply_textures};
    use crate::geom::label::Label;
    use crate::plan::connectors::plan_connectors;
    use crate::plan::emit::{Emitted, MapIdentity, emit};
    use crate::plan::model::{Connection, ConnectionKind, Plan, Room, Shape};
    use crate::plan::walls::build_walls;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{Linedef, Map};
    use crate::wad::name::LumpName;

    /// A theme answering every role with a lowercase name: the pass must
    /// uppercase it.
    struct FixedRoles;

    impl TextureRoles for FixedRoles {
        fn texture(&self, role: TextureRole) -> Option<&str> {
            match role {
                TextureRole::Floor => Some("florole"),
                TextureRole::Ceiling => Some("ceirole"),
                TextureRole::Wall => Some("walrole"),
                TextureRole::Door => Some("doorole"),
                TextureRole::DoorFloor => Some("dforole"),
                TextureRole::DoorTrack => Some("dtrrole"),
                TextureRole::WindowSill => Some("wsirole"),
                TextureRole::Step => Some("steptol"),
            }
        }
    }

    /// A theme answering nothing: every face stays `"-"`.
    struct NoRoles;

    impl TextureRoles for NoRoles {
        fn texture(&self, _role: TextureRole) -> Option<&str> {
            None
        }
    }

    /// The fixed emission identity.
    fn identity() -> MapIdentity {
        MapIdentity {
            name: LumpName::parse("MAP01").expect("test names are valid"),
            format: MapFormat::Doom,
        }
    }

    /// A default rectangular room with an optional wall override.
    fn room(id: &str, wall_texture: Option<&str>) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Rect {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
            },
            floor: 0,
            ceiling: 128,
            light: 160,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: wall_texture.map(str::to_owned),
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// Emit a single-room plan and dress the map through a hand-built
    /// [`Scene`].
    #[track_caller]
    fn dress(plan: &Plan, roles: &dyn TextureRoles) -> Map {
        let walls = build_walls(plan).expect("the fixture walls build");
        let connectors = plan_connectors(plan, &walls).expect("the fixture connectors place");
        let mut emitted = emit(plan, &walls, &connectors, identity()).expect("the fixture emits");
        let mut map = core::mem::take(&mut emitted.map);
        let scene = Scene {
            plan,
            connectors: &connectors,
            emitted: &emitted,
        };
        apply_textures(&mut map, &scene, roles).expect("dressing succeeds");
        map
    }

    /// A plan of just the given rooms.
    fn solo(rooms: Vec<Room>) -> Plan {
        Plan {
            rooms,
            connections: Vec::new(),
            features: Vec::new(),
            wall_thickness: 16_i64,
        }
    }

    #[test]
    fn a_room_wall_override_beats_the_role_and_names_are_uppercased() {
        let map = dress(&solo(vec![room("cellar", Some("brick"))]), &FixedRoles);
        assert_eq!(map.linedefs.len(), 4, "the lone room draws its four walls");
        for line in &map.linedefs {
            assert!(line.back.is_none(), "every wall faces the void");
            let front = line.front.expect("one-sided lines have a front");
            let row = map.sidedefs.get(front).expect("test indices");
            assert_eq!(row.middle, "BRICK", "the override wins, uppercased");
        }
        let space = map.sectors.first().expect("one sector");
        assert_eq!(space.floor_flat, "FLOROLE", "the floor role fills in");
        assert_eq!(
            space.ceiling_flat, "CEIROLE",
            "and so does the ceiling role"
        );
    }

    #[test]
    fn missing_roles_leave_every_face_bare() {
        let map = dress(&solo(vec![room("cellar", None)]), &NoRoles);
        for row in &map.sidedefs {
            assert_eq!(
                (row.upper.as_str(), row.lower.as_str(), row.middle.as_str()),
                ("-", "-", "-"),
                "no theme, no names"
            );
        }
        let space = map.sectors.first().expect("one sector");
        assert_eq!(
            (space.floor_flat.as_str(), space.ceiling_flat.as_str()),
            ("-", "-")
        );
    }

    #[test]
    fn an_outdoor_rooms_sky_ceiling_wears_no_role() {
        let mut yard = room("yard", None);
        yard.outdoor = true;
        let map = dress(&solo(vec![yard]), &FixedRoles);
        let space = map.sectors.first().expect("one sector");
        assert_eq!(space.ceiling_flat, "F_SKY1", "the sky stays the sky");
        assert_eq!(space.floor_flat, "FLOROLE", "the floor still gets its role");
    }

    /// Two `64 x 64` rooms sharing the line `x = 64` behind a `16` wall,
    /// joined by `connections`.
    fn pair(connections: Vec<Connection>) -> Plan {
        Plan {
            rooms: vec![
                room("west", None),
                Room {
                    shape: Shape::Rect {
                        x: 64,
                        y: 0,
                        w: 64,
                        h: 64,
                    },
                    ..room("east", None)
                },
            ],
            connections,
            features: Vec::new(),
            wall_thickness: 16_i64,
        }
    }

    /// A plain west-to-east opening `width` map units across.
    fn gap(width: i64) -> Connection {
        Connection {
            id: None,
            from: "west".to_owned(),
            to: "east".to_owned(),
            kind: ConnectionKind::Opening,
            at: None,
            width,
        }
    }

    #[test]
    fn an_opening_band_inherits_the_adjoining_rooms_flats() {
        let mut plan = pair(vec![gap(32_i64)]);
        if let Some(west) = plan.rooms.get_mut(0) {
            west.floor_flat = Some("floor4_8".to_owned());
            west.ceiling_flat = Some("CEIL5_1".to_owned());
        }
        if let Some(east) = plan.rooms.get_mut(1) {
            east.floor = 32_i32;
            east.floor_flat = Some("FLAT10".to_owned());
        }
        let map = dress(&plan, &FixedRoles);
        let band = map.sectors.get(2).expect("the band sector");
        assert_eq!(band.floor_height, 32_i32, "the higher floor spans the band");
        assert_eq!(
            band.floor_flat, "FLAT10",
            "so the east room's floor continues through it"
        );
        assert_eq!(
            band.ceiling_flat, "CEIL5_1",
            "and the west room owns the ceiling both stand at"
        );
    }

    #[test]
    fn an_outdoor_band_inherits_the_sky_only_where_it_reaches_the_sky() {
        let mut plan = pair(vec![gap(32_i64)]);
        for entry in &mut plan.rooms {
            entry.outdoor = true;
        }
        if let Some(east) = plan.rooms.get_mut(1) {
            east.ceiling = 256_i32;
        }
        let map = dress(&plan, &FixedRoles);
        let band = map.sectors.get(2).expect("the band sector");
        assert_eq!(band.ceiling_height, 128_i32, "the lower ceiling spans it");
        assert_eq!(
            band.ceiling_flat, "F_SKY1",
            "the room standing at that ceiling opens to the sky, and so does the band"
        );
        assert_eq!(band.floor_flat, "FLOROLE", "its floor takes the role");
    }

    #[test]
    fn a_step_band_between_the_rooms_heights_falls_back_to_the_room_it_leads_from() {
        let mut climb = gap(32_i64);
        climb.kind = ConnectionKind::Stairs {
            step_height: 16_i32,
            step_depth: 16_i32,
        };
        let mut plan = pair(vec![climb]);
        if let Some(west) = plan.rooms.get_mut(0) {
            west.floor_flat = Some("FLOOR4_8".to_owned());
        }
        if let Some(east) = plan.rooms.get_mut(1) {
            east.floor = 48_i32;
            east.floor_flat = Some("FLAT10".to_owned());
        }
        let map = dress(&plan, &FixedRoles);
        assert_eq!(map.sectors.len(), 5, "two rooms plus three steps");
        for index in 2_usize..4_usize {
            let step = map.sectors.get(index).expect("a step sector");
            let floor = step.floor_height;
            assert_eq!(
                step.floor_flat, "FLOOR4_8",
                "the step at floor {floor} continues the west room it climbs from"
            );
        }
        let top = map.sectors.get(4).expect("the top step");
        assert_eq!(
            top.floor_flat, "FLAT10",
            "and the step flush with the east room wears that room's floor"
        );
    }

    #[test]
    fn a_full_width_opening_leaves_no_band_to_dress() {
        let map = dress(&pair(vec![gap(64_i64)]), &FixedRoles);
        assert_eq!(map.sectors.len(), 2, "the rooms join with no band between");
        let open: Vec<&Linedef> = map
            .linedefs
            .iter()
            .filter(|line| line.back.is_some())
            .collect();
        assert_eq!(
            open.len(),
            1,
            "one open line, the shared run, and nothing else"
        );
        for line in open {
            let front = map
                .sidedefs
                .get(line.front.expect("test rows"))
                .expect("row");
            let behind = map
                .sidedefs
                .get(line.back.expect("test rows"))
                .expect("row");
            let faces = (front.sector, behind.sector);
            assert!(
                matches!(faces, (0_usize, 1_usize) | (1_usize, 0_usize)),
                "the shared line faces the two rooms: {line:?}"
            );
            assert_eq!(front.lower, "-", "nothing stands exposed across it");
            assert_eq!(behind.lower, "-", "on either face");
        }
    }

    #[test]
    fn a_map_the_pieces_do_not_back_is_refused() {
        let plan = solo(vec![room("cellar", None)]);
        let walls = build_walls(&plan).expect("the fixture walls build");
        let connectors = plan_connectors(&plan, &walls).expect("the fixture connectors place");
        let emitted = emit(&plan, &walls, &connectors, identity()).expect("the fixture emits");
        let foreign = Emitted {
            map: Map::default(),
            sector_of_label: BTreeMap::new(),
            pieces: emitted.pieces,
        };
        let scene = Scene {
            plan: &plan,
            connectors: &connectors,
            emitted: &foreign,
        };
        let error = apply_textures(&mut Map::default(), &scene, &NoRoles)
            .expect_err("four pieces cannot back an empty map");
        assert_eq!(error.code(), "internal", "a contract break is internal");
        assert!(
            error.to_string().contains("pieces"),
            "the message explains the mix-up: {error}"
        );
    }

    #[test]
    fn a_face_label_without_a_sector_is_refused() {
        let plan = solo(vec![room("cellar", None)]);
        let walls = build_walls(&plan).expect("the fixture walls build");
        let connectors = plan_connectors(&plan, &walls).expect("the fixture connectors place");
        let mut emitted = emit(&plan, &walls, &connectors, identity()).expect("the fixture emits");
        let map = core::mem::take(&mut emitted.map);
        let labels = BTreeMap::from([(Label::Region(9), 0_usize)]);
        let foreign = Emitted {
            map,
            sector_of_label: labels,
            pieces: emitted.pieces,
        };
        let scene = Scene {
            plan: &plan,
            connectors: &connectors,
            emitted: &foreign,
        };
        let mut dressed = Map::default();
        let error = apply_textures(&mut dressed, &scene, &NoRoles)
            .expect_err("the room label owns no sector");
        assert_eq!(error.code(), "internal", "the label index is the contract");
    }
}
