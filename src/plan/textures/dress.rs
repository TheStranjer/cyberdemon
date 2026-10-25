//! The dressing passes [`apply_textures`](super::apply_textures) runs: the
//! faces every emitted piece resolves into, the [`Dresser`] that owns one
//! pass' resolved role names and band index, and the rules that write a
//! sidedef or sector row: room flats, the bands' inherited flats, the
//! one-sided walls and door tracks, the two-sided doors, windows and stair
//! risers, and the final gap fill that leaves no exposed height bare.
//!
//! The scene, the theme and the eight roles it dresses from live in the
//! [`textures`](super) root. Nothing here arranges or reads an archive.

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::geom::Piece;
use crate::geom::label::Label;
use crate::plan::connectors::ConnectorGeometry;
use crate::plan::model::ConnectionKind;
use crate::plan::textures::{Scene, TextureRole, TextureRoles};
use crate::wad::map::flags::{BLOCKING, DONT_PEG_BOTTOM, TWO_SIDED};
use crate::wad::map::{Map, Sector};

/// The model's name for "draw nothing here" on a texture or flat face.
const NO_NAME: &str = "-";

/// The ceiling flat an `outdoor` room's sector already carries: the sky
/// draws itself, so no role texture goes there.
const SKY_FLAT: &str = "F_SKY1";

/// Which plan layer owns a piece face's region label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    /// The plan room at this index into [`crate::plan::model::Plan::rooms`].
    Room(usize),
    /// The `step`-th band layer of the `connector`-th placed connector.
    Band {
        /// Index into [`Scene::connectors`].
        connector: usize,
        /// Index into [`ConnectorGeometry::layers`], the lower stair
        /// step first; `0` for every single-band kind.
        step: usize,
    },
}

/// One face of an emitted line: the sidedef row to dress, the plan
/// layer owning the sector behind it, and that sector's shape.
#[derive(Clone, Copy, Debug)]
struct Face {
    /// Index of the [`Sidedef`](crate::wad::map::Sidedef) row in
    /// [`Map::sidedefs`].
    side: usize,
    /// The plan layer owning the face's sector.
    owner: Owner,
    /// The owning sector's floor height.
    floor: i32,
    /// The owning sector's ceiling height.
    ceiling: i32,
    /// Whether the owning sector keeps the sky as its ceiling flat.
    sky: bool,
}

/// Both faces of one emitted line, resolved for the dressing passes.
#[derive(Clone, Copy, Debug)]
struct Line {
    /// The [`Map::linedefs`] index, equal to the piece's index.
    index: usize,
    /// The front (`Doom` right) face.
    front: Face,
    /// The back face of a two-sided line; `None` when the line is
    /// one-sided against the void.
    back: Option<Face>,
}

impl Line {
    /// The two-sided line as a face pair, `None` when one-sided.
    const fn pair(&self) -> Option<(Face, Face)> {
        match self.back {
            Some(back) => Some((self.front, back)),
            None => None,
        }
    }
}

/// The eight resolved role names, uppercased once per pass.
///
/// A role the theme left unnamed resolves to `"-"`, so assigning a name
/// is always safe and a missing role leaves a face exactly as
/// [`emit`](crate::plan::emit::emit) set it.
#[derive(Clone, Debug)]
struct RoleNames {
    /// The room floor flat.
    floor: String,
    /// The room ceiling flat.
    ceiling: String,
    /// Plain walls and exposed bands.
    wall: String,
    /// The door face.
    door: String,
    /// The door band's flats.
    door_floor: String,
    /// The door jamb track.
    door_track: String,
    /// The window sill.
    sill: String,
    /// The stair risers.
    step: String,
}

impl RoleNames {
    /// Resolve every [`TextureRole`] through `roles`.
    fn resolve(roles: &dyn TextureRoles) -> Self {
        Self {
            floor: role_name(roles, TextureRole::Floor),
            ceiling: role_name(roles, TextureRole::Ceiling),
            wall: role_name(roles, TextureRole::Wall),
            door: role_name(roles, TextureRole::Door),
            door_floor: role_name(roles, TextureRole::DoorFloor),
            door_track: role_name(roles, TextureRole::DoorTrack),
            sill: role_name(roles, TextureRole::WindowSill),
            step: role_name(roles, TextureRole::Step),
        }
    }
}

/// `role`'s uppercased name from `roles`, `"-"` when it names none.
fn role_name(roles: &dyn TextureRoles, role: TextureRole) -> String {
    roles
        .texture(role)
        .map_or_else(|| NO_NAME.to_owned(), str::to_uppercase)
}

/// Dress `map` - the emission behind `scene` - in the names `roles`
/// answers, in place: the flats, the one-sided walls, the two-sided kind
/// rules and the gap fill, over the lines the scene's pieces resolve to.
///
/// # Errors
///
/// [`super::apply_textures`]' [`CyberError::Internal`] refusals for a map
/// that is not the one behind `scene`.
pub(super) fn apply(
    map: &mut Map,
    scene: &Scene<'_>,
    roles: &dyn TextureRoles,
) -> Result<(), CyberError> {
    let dress = Dresser::new(*scene, roles);
    let lines = dress.lines(map)?;
    dress_flats(&dress, map);
    dress_one_sided(&dress, map, &lines);
    dress_two_sided(&dress, map, &lines);
    fill_gaps(map, &lines, &dress.names.wall);
    Ok(())
}

/// One [`apply_textures`] pass: the scene, the band label index and the
/// resolved names every rule reads.
struct Dresser<'scene> {
    /// The plan, connectors and emission being dressed.
    scene: Scene<'scene>,
    /// Connector band labels to their `(connector, step)` owners.
    bands: BTreeMap<u32, (usize, usize)>,
    /// The theme's names for the eight roles.
    names: RoleNames,
}

impl<'scene> Dresser<'scene> {
    /// A pass over `scene` with the names `roles` gives.
    fn new(scene: Scene<'scene>, roles: &dyn TextureRoles) -> Self {
        Self {
            bands: band_labels(scene.connectors),
            names: RoleNames::resolve(roles),
            scene,
        }
    }

    /// Every piece's faces resolved into the map's rows and sectors.
    ///
    /// # Errors
    ///
    /// [`apply_textures`]'s `Internal` refusals.
    fn lines(&self, map: &Map) -> Result<Vec<Line>, CyberError> {
        let pieces = &self.scene.emitted.pieces;
        if pieces.len() != map.linedefs.len() {
            return Err(CyberError::Internal(format!(
                "the emission carries {} pieces but the map {} linedefs; dress the map this emission produced",
                pieces.len(),
                map.linedefs.len()
            )));
        }
        pieces
            .iter()
            .enumerate()
            .map(|(index, piece)| self.one_line(map, index, piece))
            .collect()
    }

    /// Resolve one piece's two faces into a [`Line`].
    fn one_line(&self, map: &Map, index: usize, piece: &Piece) -> Result<Line, CyberError> {
        let line = map
            .linedefs
            .get(index)
            .ok_or_else(|| foreign_map(index, piece))?;
        let (front_label, back_label) = face_labels(piece);
        let front_side = line.front.ok_or_else(|| foreign_map(index, piece))?;
        let front = self.face(map, front_side, front_label)?;
        let back = match (line.back, back_label) {
            (Some(side), Some(label)) => Some(self.face(map, side, label)?),
            (None, None) => None,
            (Some(_), None) | (None, Some(_)) => {
                return Err(foreign_map(index, piece));
            }
        };
        Ok(Line { index, front, back })
    }

    /// One face's row, owner and sector shape.
    ///
    /// # Errors
    ///
    /// [`apply_textures`]'s `Internal` refusals.
    fn face(&self, map: &Map, side: usize, label: Label) -> Result<Face, CyberError> {
        let owner = self.owner(label).ok_or_else(|| unmapped(label))?;
        let sector = self
            .scene
            .emitted
            .sector_of_label
            .get(&label)
            .copied()
            .ok_or_else(|| unmapped(label))?;
        let row = map.sidedefs.get(side).ok_or_else(|| unmapped(label))?;
        let space = map.sectors.get(sector).ok_or_else(|| unmapped(label))?;
        if row.sector != sector {
            return Err(unmapped(label));
        }
        Ok(Face {
            side,
            owner,
            floor: space.floor_height,
            ceiling: space.ceiling_height,
            sky: space.ceiling_flat.eq_ignore_ascii_case(SKY_FLAT),
        })
    }

    /// The plan layer owning a region label: rooms answer first (labels
    /// below the room count), then the connector band index.
    fn owner(&self, label: Label) -> Option<Owner> {
        let Label::Region(id) = label else {
            return None;
        };
        let room = usize::try_from(id)
            .ok()
            .filter(|index| *index < self.scene.plan.rooms.len());
        room.map(Owner::Room).or_else(|| {
            self.bands
                .get(&id)
                .copied()
                .map(|(connector, step)| Owner::Band { connector, step })
        })
    }
}

/// The `(connector, step)` owner of every band layer label.
fn band_labels(connectors: &[ConnectorGeometry]) -> BTreeMap<u32, (usize, usize)> {
    connectors
        .iter()
        .enumerate()
        .flat_map(|(connector, geometry)| {
            geometry
                .layers
                .iter()
                .enumerate()
                .map(move |(step, layer)| (layer.label, (connector, step)))
        })
        .filter_map(|(label, owner)| region_id(label).map(|id| (id, owner)))
        .collect()
}

/// The `u32` of a [`Label::Region`], `None` for [`Label::Void`].
const fn region_id(label: Label) -> Option<u32> {
    match label {
        Label::Region(id) => Some(id),
        Label::Void => None,
    }
}

/// The piece's face labels in linedef order: the front face's region
/// first, the back's `None` on one-sided pieces, mirroring
/// [`emit`](crate::plan::emit::emit)'s flip rule.
const fn face_labels(piece: &Piece) -> (Label, Option<Label>) {
    match (piece.left, piece.right) {
        (Label::Void, Label::Region(_)) => (piece.right, None),
        (Label::Region(_) | Label::Void, Label::Void) => (piece.left, None),
        (Label::Region(_), Label::Region(_)) => (piece.right, Some(piece.left)),
    }
}

/// Sector dressings: room flats from the theme, then the bands' flats.
fn dress_flats(dress: &Dresser<'_>, map: &mut Map) {
    for index in 0..dress.scene.plan.rooms.len() {
        dress_room_flats(dress, map, index);
    }
    for connector in dress.scene.connectors {
        dress_band_flats(dress, map, connector);
    }
}

/// Unset room flats take the `Floor` and `Ceiling` roles; an `outdoor`
/// room's `F_SKY1` ceiling is set, so it is never touched.
fn dress_room_flats(dress: &Dresser<'_>, map: &mut Map, index: usize) {
    let Some(sector) = dress
        .scene
        .emitted
        .sector_of_label
        .get(&region_label(index))
        .copied()
    else {
        return;
    };
    let Some(space) = map.sectors.get_mut(sector) else {
        return;
    };
    if space.floor_flat == NO_NAME {
        space.floor_flat.clone_from(&dress.names.floor);
    }
    if space.ceiling_flat == NO_NAME {
        space.ceiling_flat.clone_from(&dress.names.ceiling);
    }
}

/// The region label of plan index `index`, matching the room numbering of
/// [`room_layers`](crate::plan::walls::room_layers).
fn region_label(index: usize) -> Label {
    Label::Region(u32::try_from(index).unwrap_or(u32::MAX))
}

/// A door band dresses floor and ceiling in the `DoorFloor` role: the
/// track the leaf slides along. Every other kind continues the rooms it
/// joins, so a passage, a glass box or a step never stands in bare flats.
fn dress_band_flats(dress: &Dresser<'_>, map: &mut Map, connector: &ConnectorGeometry) {
    if matches!(connector.kind, ConnectionKind::Door { .. }) {
        for layer in &connector.layers {
            dress_door_band(dress, map, layer.label);
        }
        return;
    }
    dress_joined_bands(dress, map, connector);
}

/// Both flats of the sector behind a door band label, if it survived.
fn dress_door_band(dress: &Dresser<'_>, map: &mut Map, label: Label) {
    let Some(sector) = dress.scene.emitted.sector_of_label.get(&label).copied() else {
        return;
    };
    let Some(space) = map.sectors.get_mut(sector) else {
        return;
    };
    space.floor_flat.clone_from(&dress.names.door_floor);
    space.ceiling_flat.clone_from(&dress.names.door_floor);
}

/// The bands of one non-door connector: each surviving layer's sector
/// takes its flats from the adjoining rooms it stands between.
fn dress_joined_bands(dress: &Dresser<'_>, map: &mut Map, connector: &ConnectorGeometry) {
    let rooms = adjoining_sectors(dress, connector);
    for layer in &connector.layers {
        let Some(band) = dress
            .scene
            .emitted
            .sector_of_label
            .get(&layer.label)
            .copied()
        else {
            continue;
        };
        let Some((floor_flat, ceiling_flat)) = continued_flats(map, &rooms, band) else {
            continue;
        };
        if let Some(space) = map.sectors.get_mut(band) {
            floor_flat.clone_into(&mut space.floor_flat);
            ceiling_flat.clone_into(&mut space.ceiling_flat);
        }
    }
}

/// The sectors of the two rooms a connector joins, the room it leads from
/// first, skipping a room the emission kept nothing of.
fn adjoining_sectors(dress: &Dresser<'_>, connector: &ConnectorGeometry) -> Vec<usize> {
    [connector.from, connector.to]
        .into_iter()
        .filter_map(|index| {
            dress
                .scene
                .emitted
                .sector_of_label
                .get(&region_label(index))
                .copied()
        })
        .collect()
}

/// The flats the band `band` continues from `rooms`: each of the band's
/// two surfaces wears the flat of the adjoining room standing at the same
/// height, so a passage flush with a room carries that room's flat and an
/// `outdoor` room's sky only carries over where the band really reaches
/// the sky. A band standing between the rooms' heights (a stair step, a
/// window box) falls back to the first adjoining room; a connector with
/// no adjoining sector at all keeps what [`emit`] left.
///
/// [`emit`]: crate::plan::emit::emit
fn continued_flats(map: &Map, rooms: &[usize], band: usize) -> Option<(String, String)> {
    let space = map.sectors.get(band)?;
    let (floor_height, ceiling_height) = (space.floor_height, space.ceiling_height);
    let own = *rooms.first()?;
    let floor_from = level_room(map, rooms, floor_height, floor_of).unwrap_or(own);
    let ceiling_from = level_room(map, rooms, ceiling_height, ceiling_of).unwrap_or(own);
    let floor_flat = map.sectors.get(floor_from)?.floor_flat.clone();
    let ceiling_flat = map.sectors.get(ceiling_from)?.ceiling_flat.clone();
    Some((floor_flat, ceiling_flat))
}

/// The first of `rooms` whose `height_of` surface stands at `height`.
fn level_room(
    map: &Map,
    rooms: &[usize],
    height: i32,
    height_of: fn(&Sector) -> i32,
) -> Option<usize> {
    rooms.iter().copied().find(|&index| {
        map.sectors
            .get(index)
            .is_some_and(|space| height_of(space) == height)
    })
}

/// The floor of a sector row.
const fn floor_of(space: &Sector) -> i32 {
    space.floor_height
}

/// The ceiling of a sector row.
const fn ceiling_of(space: &Sector) -> i32 {
    space.ceiling_height
}

/// One-sided lines: a room's walls take its override else the `Wall`
/// role; band faces inside the void wall take the `Wall` role for an
/// opening and the unpegged `DoorTrack` role for a door. Window and
/// stair bands dress no void-facing face: those reveals sit inside
/// solid rock the player never reaches.
fn dress_one_sided(dress: &Dresser<'_>, map: &mut Map, lines: &[Line]) {
    for line in lines.iter().filter(|line| line.back.is_none()) {
        match line.front.owner {
            Owner::Room(index) => {
                set_middle(map, line.front.side, &room_wall(dress, index));
            }
            Owner::Band { connector, .. } => dress_band_jamb(dress, map, line, connector),
        }
    }
}

/// The `middle` name for room `index`'s walls: its own override,
/// uppercased, else the `Wall` role.
fn room_wall(dress: &Dresser<'_>, index: usize) -> String {
    dress
        .scene
        .plan
        .rooms
        .get(index)
        .and_then(|entry| entry.wall_texture.as_deref())
        .map_or_else(|| dress.names.wall.clone(), str::to_uppercase)
}

/// The void-facing middle of one band line, by connector kind.
fn dress_band_jamb(dress: &Dresser<'_>, map: &mut Map, line: &Line, connector: usize) {
    let Some(geometry) = dress.scene.connectors.get(connector) else {
        return;
    };
    match geometry.kind {
        ConnectionKind::Opening => set_middle(map, line.front.side, &dress.names.wall),
        ConnectionKind::Door { .. } => {
            set_middle(map, line.front.side, &dress.names.door_track);
            // The lower texture is pinned to the jamb's own floor: the
            // `dontpegbottom` bit keeps the track put while the leaf slides.
            add_flags(map, line.index, DONT_PEG_BOTTOM);
        }
        ConnectionKind::Window { .. } | ConnectionKind::Stairs { .. } => {}
    }
}

/// Two-sided lines whose faces need a kind rule; the fill pass catches
/// every height gap these rules leave.
fn dress_two_sided(dress: &Dresser<'_>, map: &mut Map, lines: &[Line]) {
    for line in lines {
        dress_pair(dress, map, line);
    }
}

/// One two-sided line by its face owners.
fn dress_pair(dress: &Dresser<'_>, map: &mut Map, line: &Line) {
    let Some((front, back)) = line.pair() else {
        return;
    };
    match (front.owner, back.owner) {
        (Owner::Room(_), Owner::Band { .. }) | (Owner::Band { .. }, Owner::Room(_)) => {
            dress_room_band(dress, map, line);
        }
        (Owner::Band { .. }, Owner::Band { .. }) => {
            dress_band_band(dress, map, &front, &back);
        }
        // Room-to-room lines (zero thickness, or where a band ends)
        // expose nothing a kind rule names: the fill pass dresses their
        // height gaps.
        (Owner::Room(_), Owner::Room(_)) => {}
    }
}

/// The room-facing side of a band line: doors get their leaf above the
/// collapsed band, windows their sill and lintel and blocking, and the
/// lowest step of a stair run its riser against the lower room.
fn dress_room_band(dress: &Dresser<'_>, map: &mut Map, line: &Line) {
    let Some((front, back)) = line.pair() else {
        return;
    };
    let (room, band) = pick_room_band(&front, &back);
    let (Owner::Band { connector, step }, Owner::Room(room_index)) = (band.owner, room.owner)
    else {
        return;
    };
    let Some(geometry) = dress.scene.connectors.get(connector) else {
        return;
    };
    match geometry.kind {
        ConnectionKind::Window { .. } => {
            set_lower(map, room.side, &dress.names.sill);
            set_upper(map, room.side, &dress.names.wall);
            add_flags(map, line.index, BLOCKING | TWO_SIDED);
        }
        ConnectionKind::Door { .. } => set_upper(map, room.side, &dress.names.door),
        ConnectionKind::Stairs { .. }
            if step == 0_usize && geometry.lower_room == Some(room_index) =>
        {
            dress_riser(map, room, band, &dress.names.step);
        }
        ConnectionKind::Opening | ConnectionKind::Stairs { .. } => {}
    }
}

/// Order a two-sided band line's faces room-first.
const fn pick_room_band<'faces>(
    front: &'faces Face,
    back: &'faces Face,
) -> (&'faces Face, &'faces Face) {
    if matches!(front.owner, Owner::Room(_)) {
        (front, back)
    } else {
        (back, front)
    }
}

/// The riser between two consecutive step bands of one stair run.
fn dress_band_band(dress: &Dresser<'_>, map: &mut Map, one: &Face, two: &Face) {
    let (
        Owner::Band { connector, step },
        Owner::Band {
            connector: other,
            step: other_step,
        },
    ) = (one.owner, two.owner)
    else {
        return;
    };
    if connector != other || step.abs_diff(other_step) != 1_usize {
        return;
    }
    let Some(geometry) = dress.scene.connectors.get(connector) else {
        return;
    };
    if matches!(geometry.kind, ConnectionKind::Stairs { .. }) {
        dress_riser(map, one, two, &dress.names.step);
    }
}

/// The `Step` role on the lower sector's side of a riser; equal floors
/// make no riser at all.
fn dress_riser(map: &mut Map, one: &Face, two: &Face, texture: &str) {
    match (one.floor, two.floor) {
        (front, back) if front < back => set_lower(map, one.side, texture),
        (front, back) if back < front => set_lower(map, two.side, texture),
        _ => {}
    }
}

/// The general fill: every exposed two-sided face no kind rule dressed
/// takes the plain `wall` role. Middle faces of two-sided lines are
/// never touched.
fn fill_gaps(map: &mut Map, lines: &[Line], wall: &str) {
    for line in lines {
        let Some((front, back)) = line.pair() else {
            continue;
        };
        fill_side(map, &front, &back, wall);
        fill_side(map, &back, &front, wall);
    }
}

/// One side's fill: a higher neighbour floor exposes a `lower`, a lower
/// neighbour ceiling an `upper` — unless both sectors keep the sky,
/// where the engine draws the gap itself.
fn fill_side(map: &mut Map, this: &Face, other: &Face, wall: &str) {
    let Some(row) = map.sidedefs.get_mut(this.side) else {
        return;
    };
    if other.floor > this.floor && row.lower == NO_NAME {
        wall.clone_into(&mut row.lower);
    }
    if other.ceiling < this.ceiling && !(this.sky && other.sky) && row.upper == NO_NAME {
        wall.clone_into(&mut row.upper);
    }
}

/// Write `texture` into one sidedef row's `middle` face.
fn set_middle(map: &mut Map, side: usize, texture: &str) {
    if let Some(row) = map.sidedefs.get_mut(side) {
        texture.clone_into(&mut row.middle);
    }
}

/// Write `texture` into one sidedef row's `lower` face.
fn set_lower(map: &mut Map, side: usize, texture: &str) {
    if let Some(row) = map.sidedefs.get_mut(side) {
        texture.clone_into(&mut row.lower);
    }
}

/// Write `texture` into one sidedef row's `upper` face.
fn set_upper(map: &mut Map, side: usize, texture: &str) {
    if let Some(row) = map.sidedefs.get_mut(side) {
        texture.clone_into(&mut row.upper);
    }
}

/// Light `bits` into the `index`th linedef's flags.
fn add_flags(map: &mut Map, index: usize, bits: u32) {
    if let Some(line) = map.linedefs.get_mut(index) {
        line.flags |= bits;
    }
}

/// The `Internal` for a piece the `map`'s linedef rows do not back.
fn foreign_map(index: usize, piece: &Piece) -> CyberError {
    CyberError::Internal(format!(
        "piece {index} ({} / {}) has no matching sidedef rows in this map; dress the map the emission produced",
        piece.left.describe(),
        piece.right.describe()
    ))
}

/// The `Internal` for a face label no plan layer or sector answers.
fn unmapped(label: Label) -> CyberError {
    CyberError::Internal(format!(
        "no dressed sector answers the face label {}; dress the emission's own map",
        label.describe()
    ))
}
