//! Thick walls between plan rooms, built on [`arrange`](fn@crate::geom::arrange).
//!
//! [`room_layers`] turns a [`Plan`]'s rooms into one [`Layer`] each;
//! [`build_walls`] arranges those layers, finds the pieces two different
//! rooms share ([`shared_boundaries`]) and grows a [`Label::Void`]
//! rectangle straddling each one ([`wall_void_layer`]) so the finished
//! stack paints solid wall of [`Plan::wall_thickness`] between the rooms.
//! Each rectangle is recorded with the [`WallKey`] of the piece it fills,
//! so a caller that opens a whole shared run can drop exactly that wall
//! and leave the rest standing. Nothing here touches archives, sessions
//! or `MCP` types; the caller stacks the layers and arranges again.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::fixed::{isqrt, mul_div_round_sat, narrow_i64};
use crate::geom::label::{Label, LayerLabeler};
use crate::geom::{Layer, Piece, Pt, arrange};
use crate::plan::model::Plan;

/// One [`Layer`] per room, in plan order, labelled
/// [`Label::Region`] by its index in [`Plan::rooms`].
///
/// Rooms are painter's-algorithm layers, bottom first: where two rooms
/// overlap, the room later in the plan overwrites the earlier one,
/// because [`LayerLabeler`] always reports the topmost containing layer
/// and [`arrange`](fn@crate::geom::arrange) drops pieces whose two sides
/// carry the same label.
#[must_use]
#[inline]
pub fn room_layers(plan: &Plan) -> Vec<Layer> {
    plan.rooms
        .iter()
        .enumerate()
        .map(|(index, room)| Layer {
            label: Label::region(index),
            polygon: room.polygon(),
        })
        .collect()
}

/// The pieces of `pieces` that two different rooms share, keyed by the
/// room pair `(lower, higher)`.
///
/// A shared boundary has [`Label::Region`] values of two different ids on
/// its left and right sides. Pieces touching [`Label::Void`] are room
/// outer edges, not shared boundaries: they become one-sided map walls
/// later. Pieces whose sides carry the same label (which
/// [`arrange`](fn@crate::geom::arrange) already drops, but hand-made
/// input may hold) are not shared either.
#[must_use]
#[inline]
pub fn shared_boundaries(pieces: &[Piece]) -> BTreeMap<(u32, u32), Vec<Piece>> {
    let mut shared: BTreeMap<(u32, u32), Vec<Piece>> = BTreeMap::new();
    for piece in pieces {
        if let Some(key) = shared_pair(piece) {
            shared.entry(key).or_default().push(piece.clone());
        }
    }
    shared
}

/// The room pair `(lower, higher)` a piece separates, `None` when either
/// side is not a region or both sides are the same region.
#[inline]
const fn shared_pair(piece: &Piece) -> Option<(u32, u32)> {
    let (Label::Region(first), Label::Region(second)) = (piece.left, piece.right) else {
        return None;
    };
    if first == second {
        return None;
    }
    Some(if first < second {
        (first, second)
    } else {
        (second, first)
    })
}

/// Which ends of a shared piece another wall meets at.
///
/// A wall rectangle only reaches past the piece it stands on where a
/// second wall abuts that end: see [`wall_void_layer`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WallEnds {
    /// Whether another shared piece ends at the piece's `a`.
    pub at_a: bool,
    /// Whether another shared piece ends at the piece's `b`.
    pub at_b: bool,
}

/// A [`Label::Void`] rectangle standing for the thick wall along one
/// shared-boundary `piece`.
///
/// The segment `a`→`b` is offset by `± thickness / 2` along the integer
/// unit normal `(-dy / len, dx / len)`, every component rounded through
/// [`mul_div_round_sat`] (exact for axis-aligned pieces). At each end named
/// by `ends` the segment is additionally extended by `thickness / 2`
/// past the piece, so the rectangles of abutting walls overlap at their
/// shared corner and the final stack never leaves an unwalled gap piece
/// between two wall voids; at a free end the rectangle stops with the
/// shared run, because wall there would stand in space no two rooms
/// share.
///
/// Zero-length pieces cannot come from [`arrange`](fn@crate::geom::arrange),
/// whose every piece has positive length; should one be passed in, the
/// guard below returns the piece segment doubled as the ring, which
/// [`arrange`](fn@crate::geom::arrange) will refuse as a degenerate
/// layer. A `thickness` of `0` or `1` offsets both sides onto the centre
/// line and the same doubled-segment guard applies; wall plans want at
/// least `2`.
#[must_use]
#[inline]
pub fn wall_void_layer(piece: &Piece, thickness: i64, ends: WallEnds) -> Layer {
    let dx = narrow_i64(i128::from(piece.b.0).saturating_sub(i128::from(piece.a.0)));
    let dy = narrow_i64(i128::from(piece.b.1).saturating_sub(i128::from(piece.a.1)));
    let length_squared = i128::from(dx)
        .saturating_mul(i128::from(dx))
        .saturating_add(i128::from(dy).saturating_mul(i128::from(dy)));
    let length =
        i64::try_from(isqrt(u64::try_from(length_squared).unwrap_or(u64::MAX))).unwrap_or(i64::MAX);
    let degenerate = || Layer {
        label: Label::Void,
        polygon: vec![piece.a, piece.b, piece.a, piece.b],
    };
    if length == 0 {
        return degenerate();
    }
    let half = thickness.saturating_div(2_i64);
    let extension = (
        mul_div_round_sat(half, dx, length),
        mul_div_round_sat(half, dy, length),
    );
    let normal = (
        mul_div_round_sat(half, dy.saturating_neg(), length),
        mul_div_round_sat(half, dx, length),
    );
    let back = reaching(extension, ends.at_a);
    let forward = reaching(extension, ends.at_b);
    let start = shifted(piece.a, back.0.saturating_neg(), back.1.saturating_neg());
    let stop = shifted(piece.b, forward.0, forward.1);
    if normal == (0, 0) {
        return degenerate();
    }
    let side = negated(normal);
    Layer {
        label: Label::Void,
        polygon: vec![
            shifted(start, normal.0, normal.1),
            shifted(stop, normal.0, normal.1),
            shifted(stop, side.0, side.1),
            shifted(start, side.0, side.1),
        ],
    }
}

/// The along-piece extension of one end: the full `extension` where
/// another wall abuts the end, nothing where the end stands free.
#[must_use]
#[inline]
const fn reaching(extension: Pt, abutted: bool) -> Pt {
    if abutted { extension } else { (0_i64, 0_i64) }
}

/// `point` moved by `(dx, dy)`, saturating per component.
#[inline]
const fn shifted(point: Pt, dx: i64, dy: i64) -> Pt {
    (point.0.saturating_add(dx), point.1.saturating_add(dy))
}

/// Both components negated, saturating at the `i64` extremes.
#[inline]
const fn negated(offset: Pt) -> Pt {
    (offset.0.saturating_neg(), offset.1.saturating_neg())
}

/// Which shared-boundary piece one built wall rectangle fills.
///
/// The pair and the piece's endpoints name the rectangle exactly: a
/// caller that opens a whole shared run drops the wall of that key and
/// leaves every other rectangle standing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WallKey {
    /// The room pair the wall stands between, lower plan index first, as
    /// [`shared_boundaries`] keys them.
    pub pair: (u32, u32),
    /// The shared piece's endpoints, in the order the arrangement has them.
    pub piece: (Pt, Pt),
}

/// One built wall rectangle with the shared boundary it fills.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallLayer {
    /// Which shared piece this rectangle stands on.
    pub key: WallKey,
    /// The [`Label::Void`] rectangle grown along the piece.
    pub layer: Layer,
}

/// The wall layers of a [`Plan`] and the shared room boundaries they
/// fill, as built by [`build_walls`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walls {
    /// One [`WallLayer`] per shared-boundary piece, ordered by room pair;
    /// empty when the plan's wall thickness is `0`. Each rectangle reaches
    /// past its piece only at the ends another wall abuts.
    pub layers: Vec<WallLayer>,
    /// Every piece the rooms share, keyed by the room pair
    /// `(lower, higher)` as in [`shared_boundaries`].
    pub shared: BTreeMap<(u32, u32), Vec<Piece>>,
}

/// Arrange a plan's [`room_layers`] and grow thick-wall void layers
/// along their [`shared_boundaries`].
///
/// When [`Plan::wall_thickness`] is positive, one [`wall_void_layer`]
/// rectangle is emitted along every shared piece, each recorded with the
/// [`WallKey`] of the piece it fills; a thickness of `0` means the rooms
/// touch: they share their boundary lines directly and no wall layers
/// are emitted.
///
/// The caller owns the final stacking: put the room layers at the
/// bottom, then `walls.layers`, then the connector layers, and arrange
/// that whole stack bottom-first with a [`LayerLabeler`] over it, so
/// openings cut through the wall voids. No connector layers are built
/// here.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a room polygon is degenerate or
/// self-crossing and [`CyberError::LimitExceeded`] past the arrangement
/// segment cap, both straight from [`arrange`](fn@crate::geom::arrange).
#[inline]
pub fn build_walls(plan: &Plan) -> Result<Walls, CyberError> {
    let rooms = room_layers(plan);
    let pieces = arrange(&rooms, &[], &LayerLabeler::new(&rooms))?;
    let shared = shared_boundaries(&pieces);
    let layers = wall_layers(&shared, plan.wall_thickness);
    Ok(Walls { layers, shared })
}

/// One [`wall_void_layer`] rectangle per shared piece, ordered by room
/// pair; none while the thickness is not positive.
fn wall_layers(shared: &BTreeMap<(u32, u32), Vec<Piece>>, thickness: i64) -> Vec<WallLayer> {
    if thickness <= 0 {
        return Vec::new();
    }
    let corners = corner_points(shared);
    shared
        .iter()
        .flat_map(|(pair, pieces)| {
            pieces
                .iter()
                .map(|piece| wall_layer(*pair, piece, thickness, &corners))
        })
        .collect()
}

/// The wall rectangle of one shared `piece` of room `pair`, extended at
/// the ends `corners` says another wall abuts.
#[must_use]
fn wall_layer(
    pair: (u32, u32),
    piece: &Piece,
    thickness: i64,
    corners: &BTreeSet<Pt>,
) -> WallLayer {
    WallLayer {
        key: WallKey {
            pair,
            piece: (piece.a, piece.b),
        },
        layer: wall_void_layer(
            piece,
            thickness,
            WallEnds {
                at_a: corners.contains(&piece.a),
                at_b: corners.contains(&piece.b),
            },
        ),
    }
}

/// The piece endpoints at least two shared runs meet at: the corners
/// whose wall rectangles must overlap so no unwalled gap piece survives
/// between them. A point only one piece ends at is a free end.
fn corner_points(shared: &BTreeMap<(u32, u32), Vec<Piece>>) -> BTreeSet<Pt> {
    let mut counts: BTreeMap<Pt, usize> = BTreeMap::new();
    for pieces in shared.values() {
        for piece in pieces {
            count_at(&mut counts, piece.a);
            count_at(&mut counts, piece.b);
        }
    }
    counts
        .into_iter()
        .filter_map(|(point, count)| (count > 1_usize).then_some(point))
        .collect()
}

/// Records one more shared piece ending at `point`.
fn count_at(counts: &mut BTreeMap<Pt, usize>, point: Pt) {
    let next = counts
        .get(&point)
        .copied()
        .unwrap_or(0_usize)
        .saturating_add(1);
    counts.insert(point, next);
}

#[cfg(test)]
mod tests {
    use super::{
        WallEnds, WallKey, Walls, build_walls, room_layers, shared_boundaries, wall_void_layer,
    };
    use crate::fixed::{mul_div_round_sat, narrow_i64};
    use crate::geom::label::{Labeler as _, contains_point};
    use crate::geom::{Label, Layer, LayerLabeler, Piece, Pt, SAMPLE_SCALE, arrange};
    use crate::plan::model::{Plan, Room, Shape};

    /// A rectangular room with default sector properties, based at
    /// `left_bottom` and spanning `width_height`.
    fn rect_room(id: &str, left_bottom: Pt, width_height: Pt) -> Room {
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

    /// A polygon room with default sector properties.
    fn polygon_room(id: &str, points: Vec<Pt>) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Polygon { points },
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

    /// A plan of `rooms` with the given wall thickness and no connections.
    fn plan_with(rooms: Vec<Room>, wall_thickness: i64) -> Plan {
        Plan {
            rooms,
            connections: Vec::new(),
            features: Vec::new(),
            wall_thickness,
        }
    }

    /// A hand-made piece with the given endpoints and side labels.
    fn edge_piece(a: Pt, b: Pt, left: Label, right: Label) -> Piece {
        Piece {
            a,
            b,
            left,
            right,
            origin: Vec::new(),
        }
    }

    /// The piece between two endpoints, either direction.
    fn find_piece(pieces: &[Piece], one: Pt, two: Pt) -> Option<&Piece> {
        pieces
            .iter()
            .find(|piece| (piece.a == one && piece.b == two) || (piece.a == two && piece.b == one))
    }

    /// A side-sample point at whole integer coordinates.
    fn at(x: i64, y: i64) -> (i128, i128) {
        (
            i128::from(x).saturating_mul(SAMPLE_SCALE),
            i128::from(y).saturating_mul(SAMPLE_SCALE),
        )
    }

    /// The label the painter's stack puts on a whole-coordinate point.
    fn label_at(layers: &[Layer], x: i64, y: i64) -> Label {
        LayerLabeler::new(layers).label(at(x, y))
    }

    /// Two 128 x 128 rooms side by side, sharing the line x = 128.
    fn two_rooms(thickness: i64) -> Plan {
        plan_with(
            vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (128, 0), (128, 128)),
            ],
            thickness,
        )
    }

    /// Rooms `A | B` with a third room `C` above `B`, so the two shared
    /// boundaries meet in an L at `(128, 128)`.
    fn corner_rooms(thickness: i64) -> Plan {
        plan_with(
            vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (128, 0), (128, 128)),
                rect_room("c", (128, 128), (128, 128)),
            ],
            thickness,
        )
    }

    /// The wall rectangles of a build, in the order they were built.
    fn void_layers(walls: &Walls) -> Vec<Layer> {
        walls.layers.iter().map(|wall| wall.layer.clone()).collect()
    }

    /// Stack the room layers, then the wall layers, and arrange.
    fn stack_pieces(plan: &Plan, walls: &Walls) -> Vec<Piece> {
        let mut stack = room_layers(plan);
        stack.extend(void_layers(walls));
        arrange(&stack, &[], &LayerLabeler::new(&stack)).expect("the stack arranges")
    }

    /// The stack layer labels plus wall labels in the caller's order.
    fn stacked_layers(plan: &Plan, walls: &Walls) -> Vec<Layer> {
        let mut stack = room_layers(plan);
        stack.extend(void_layers(walls));
        stack
    }

    #[test]
    fn adjacent_rooms_get_one_shared_piece_and_a_centred_void_wall() {
        let plan = two_rooms(16);
        plan.validate().expect("two touching rooms validate");
        let walls = build_walls(&plan).expect("the walls build");
        assert_eq!(walls.shared.len(), 1, "exactly one shared pair");
        assert!(
            !walls.shared.contains_key(&(0, 2)),
            "no phantom key for non-touching labels"
        );
        let group = walls
            .shared
            .get(&(0, 1))
            .expect("rooms 0 and 1 share the line x=128");
        assert_eq!(group.len(), 1, "one shared piece on x=128");
        assert_eq!(group[0].a, (128, 0), "piece starts at the bottom");
        assert_eq!(group[0].b, (128, 128), "piece ends at the top");
        assert_eq!(group[0].left, Label::Region(0), "west side is room 0");
        assert_eq!(group[0].right, Label::Region(1), "east side is room 1");
        assert_eq!(walls.layers.len(), 1, "one wall void layer");
        assert_eq!(
            walls.layers[0].layer.polygon,
            vec![(120, 0), (120, 128), (136, 128), (136, 0)],
            "16 wide, centred on x=128 and stopping with the shared run: both ends stand free"
        );
        assert_eq!(
            walls.layers[0].key,
            WallKey {
                pair: (0, 1),
                piece: ((128, 0), (128, 128)),
            },
            "the layer carries the key of the piece it fills"
        );
        assert_eq!(walls.layers[0].layer.label, Label::Void, "walls are void");
        let stack = stacked_layers(&plan, &walls);
        assert_eq!(label_at(&stack, 124, 64), Label::Void, "west of x=128");
        assert_eq!(label_at(&stack, 128, 64), Label::Void, "the shared line");
        assert_eq!(label_at(&stack, 132, 64), Label::Void, "east of x=128");
        assert_eq!(label_at(&stack, 64, 64), Label::Region(0), "room 0 core");
        assert_eq!(label_at(&stack, 192, 64), Label::Region(1), "room 1 core");
        let pieces = stack_pieces(&plan, &walls);
        let west = find_piece(&pieces, (120, 0), (120, 128)).expect("inner wall line x=120");
        assert_eq!(west.left, Label::Region(0), "room 0 faces its wall");
        assert_eq!(west.right, Label::Void, "the wall is void");
        let east = find_piece(&pieces, (136, 0), (136, 128)).expect("inner wall line x=136");
        assert_eq!(east.left, Label::Void, "the wall is void");
        assert_eq!(east.right, Label::Region(1), "room 1 faces its wall");
        assert!(
            find_piece(&pieces, (128, 0), (128, 128)).is_none(),
            "the shared line sits inside the wall and is dropped"
        );
        let outer = find_piece(&pieces, (0, 0), (0, 128)).expect("room 0 outer edge");
        assert_eq!(outer.left, Label::Void, "outside room 0 is void");
        assert_eq!(
            outer.right,
            Label::Region(0),
            "room 0 outer edge stays region-vs-Void"
        );
    }

    #[test]
    fn zero_thickness_shares_room_room_lines_directly() {
        let plan = two_rooms(0);
        let walls = build_walls(&plan).expect("the walls build");
        assert!(walls.layers.is_empty(), "no wall layers at thickness 0");
        let group = walls
            .shared
            .get(&(0, 1))
            .expect("the shared piece survives");
        assert_eq!(group.len(), 1, "still one shared piece");
        assert_eq!(group[0].left, Label::Region(0), "room-to-room piece, left");
        assert_eq!(
            group[0].right,
            Label::Region(1),
            "room-to-room piece, right"
        );
        let rooms = room_layers(&plan);
        let pieces = arrange(&rooms, &[], &LayerLabeler::new(&rooms)).expect("rooms arrange");
        let shared = find_piece(&pieces, (128, 0), (128, 128)).expect("the centre line is a piece");
        assert!(
            matches!(
                (shared.left, shared.right),
                (Label::Region(_), Label::Region(_))
            ),
            "the shared run stays region-vs-region"
        );
    }

    #[test]
    fn rotated_shared_piece_gets_symmetric_rounded_corners() {
        let piece = edge_piece((0, 128), (64, 64), Label::Region(1), Label::Region(0));
        let abuts = WallEnds {
            at_a: true,
            at_b: true,
        };
        let wall = wall_void_layer(&piece, 20, abuts);
        assert_eq!(
            wall.polygon,
            vec![(0, 142), (78, 64), (64, 50), (-14, 128)],
            "length 90, half 10, every component mul_div_round-ed"
        );
        let sums: Vec<i64> = wall
            .polygon
            .iter()
            .map(|point| point.0.saturating_add(point.1))
            .collect();
        assert_eq!(
            sums,
            vec![142, 142, 114, 114],
            "corners sit 14 units each side of the centre line x+y=128"
        );
        assert_eq!(
            (
                wall.polygon[0].0.saturating_add(wall.polygon[2].0),
                wall.polygon[0].1.saturating_add(wall.polygon[2].1),
            ),
            (
                wall.polygon[1].0.saturating_add(wall.polygon[3].0),
                wall.polygon[1].1.saturating_add(wall.polygon[3].1),
            ),
            "the rectangle is symmetric about the piece midpoint"
        );
    }

    #[test]
    fn a_wall_with_free_ends_stops_with_its_shared_run() {
        let piece = edge_piece((0, 128), (64, 64), Label::Region(1), Label::Region(0));
        let wall = wall_void_layer(&piece, 20, WallEnds::default());
        assert_eq!(
            wall.polygon,
            vec![(7, 135), (71, 71), (57, 57), (-7, 121)],
            "the band still straddles the piece, but neither end reaches past it"
        );
        let sums: Vec<i64> = wall
            .polygon
            .iter()
            .map(|point| point.0.saturating_add(point.1))
            .collect();
        assert_eq!(
            sums,
            vec![142, 142, 114, 114],
            "the free-ended rectangle still sits 14 units each side of x+y=128"
        );
    }

    #[test]
    fn diamond_rooms_share_a_diagonal_boundary_and_wall() {
        let plan = plan_with(
            vec![
                polygon_room("a", vec![(0, 0), (64, 64), (0, 128), (-64, 64)]),
                polygon_room("b", vec![(64, 64), (128, 128), (64, 192), (0, 128)]),
            ],
            20,
        );
        plan.validate().expect("the diamonds validate");
        let walls = build_walls(&plan).expect("diamond walls build");
        let group = walls.shared.get(&(0, 1)).expect("the diamonds share");
        assert_eq!(group.len(), 1, "one shared diagonal");
        assert_eq!(group[0].a, (0, 128), "diagonal start");
        assert_eq!(group[0].b, (64, 64), "diagonal end");
        let wall = walls.layers.first().expect("one wall layer");
        assert_eq!(
            wall.key,
            WallKey {
                pair: (0, 1),
                piece: ((0, 128), (64, 64)),
            },
            "the key names the shared diagonal"
        );
        assert_eq!(
            wall.layer,
            wall_void_layer(&group[0], 20, WallEnds::default()),
            "the emitted layer is the helper's rectangle, free at both ends of the one run"
        );
        let stack = stacked_layers(&plan, &walls);
        assert_eq!(
            label_at(&stack, 26, 110),
            Label::Void,
            "a point inside room b near the shared edge is painted over by the wall"
        );
    }

    #[test]
    fn three_rooms_in_a_row_share_only_with_their_neighbours() {
        let plan = plan_with(
            vec![
                rect_room("a", (0, 0), (128, 128)),
                rect_room("b", (128, 0), (128, 128)),
                rect_room("c", (256, 0), (128, 128)),
            ],
            16,
        );
        let walls = build_walls(&plan).expect("the row builds");
        let keys: Vec<(u32, u32)> = walls.shared.keys().copied().collect();
        assert_eq!(
            keys,
            vec![(0, 1), (1, 2)],
            "neighbour pairs only, keyed min-max"
        );
        assert_eq!(walls.layers.len(), 2, "one wall layer per shared piece");
    }

    #[test]
    fn abutting_walls_overlap_at_their_corner() {
        let plan = corner_rooms(16);
        let walls = build_walls(&plan).expect("the corner plan builds");
        let keys: Vec<(u32, u32)> = walls.shared.keys().copied().collect();
        assert_eq!(keys, vec![(0, 1), (1, 2)], "the L shares two runs");
        assert_eq!(walls.layers.len(), 2, "one wall per run");
        let corner = at(132, 132);
        assert!(
            contains_point(&walls.layers[0].layer.polygon, corner),
            "the x=128 wall reaches y=136 past the corner"
        );
        assert!(
            contains_point(&walls.layers[1].layer.polygon, corner),
            "the y=128 wall reaches x=120 before the corner"
        );
        assert_eq!(
            walls.layers[0].layer.polygon,
            vec![(120, 0), (120, 136), (136, 136), (136, 0)],
            "the wall reaches y=136 past its abutted end and stops at y=0 at its free one"
        );
        let pieces = stack_pieces(&plan, &walls);
        assert!(
            !pieces.iter().any(|piece| matches!(
                (piece.left, piece.right),
                (Label::Region(_), Label::Region(_))
            )),
            "no region-vs-region gap piece survives between the voids"
        );
        assert_eq!(
            label_at(&stacked_layers(&plan, &walls), 124, 132),
            Label::Void,
            "the unwalled corner square closes over"
        );
    }

    #[test]
    fn a_later_room_paints_over_the_earlier_one() {
        let plan = plan_with(
            vec![
                rect_room("under", (0, 0), (128, 128)),
                rect_room("over", (64, 64), (128, 128)),
            ],
            16,
        );
        plan.validate().expect("overlapping rooms validate");
        let rooms = room_layers(&plan);
        let pieces = arrange(&rooms, &[], &LayerLabeler::new(&rooms)).expect("rooms arrange");
        assert_eq!(
            label_at(&rooms, 100, 100),
            Label::Region(1),
            "the later room wins where they overlap"
        );
        assert_eq!(
            label_at(&rooms, 30, 30),
            Label::Region(0),
            "the earlier room survives uncovered"
        );
        assert!(
            find_piece(&pieces, (128, 64), (128, 128)).is_none(),
            "the earlier room's edge buried in the later one vanishes"
        );
        assert!(
            find_piece(&pieces, (64, 128), (128, 128)).is_none(),
            "and its other buried edge too"
        );
        let shared = shared_boundaries(&pieces);
        let keys: Vec<(u32, u32)> = shared.keys().copied().collect();
        assert_eq!(keys, vec![(0, 1)], "the overlap boundary is shared");
        let group = shared.get(&(0, 1)).expect("the pair");
        let ends: Vec<(Pt, Pt)> = group.iter().map(|piece| (piece.a, piece.b)).collect();
        assert_eq!(
            ends,
            vec![((64, 64), (64, 128)), ((64, 64), (128, 64))],
            "the later room's two edges through the earlier room"
        );
    }

    #[test]
    fn shared_boundaries_skip_void_and_same_label_pieces() {
        let pieces = vec![
            edge_piece((0, 0), (10, 0), Label::Region(0), Label::Void),
            edge_piece((0, 0), (0, 10), Label::Void, Label::Region(3)),
            edge_piece((5, 5), (6, 6), Label::Region(2), Label::Region(2)),
            edge_piece((9, 9), (8, 8), Label::Region(4), Label::Region(1)),
        ];
        let shared = shared_boundaries(&pieces);
        let keys: Vec<(u32, u32)> = shared.keys().copied().collect();
        assert_eq!(keys, vec![(1, 4)], "only the region-vs-region piece counts");
        assert_eq!(shared[&(1, 4)].len(), 1, "kept once");
        assert_eq!(
            shared[&(1, 4)][0].a,
            (9, 9),
            "the piece passes through unchanged"
        );
    }

    #[test]
    fn degenerate_inputs_double_the_segment_instead_of_collapsing() {
        let zero_length = wall_void_layer(
            &edge_piece((5, 5), (5, 5), Label::Region(0), Label::Region(1)),
            16,
            WallEnds::default(),
        );
        assert_eq!(
            zero_length.polygon,
            vec![(5, 5), (5, 5), (5, 5), (5, 5)],
            "a zero-length piece becomes the doubled segment"
        );
        let one_thick = wall_void_layer(
            &edge_piece((0, 0), (128, 0), Label::Region(0), Label::Region(1)),
            1,
            WallEnds::default(),
        );
        assert_eq!(
            one_thick.polygon,
            vec![(0, 0), (128, 0), (0, 0), (128, 0)],
            "thickness 1 rounds both offsets to zero and doubles the segment"
        );
        let huge = wall_void_layer(
            &edge_piece(
                (0, 0),
                (i64::MAX, i64::MAX),
                Label::Region(0),
                Label::Region(1),
            ),
            16,
            WallEnds {
                at_a: true,
                at_b: true,
            },
        );
        assert_eq!(
            huge.polygon.len(),
            4,
            "an overlong piece still yields a ring"
        );
    }

    #[test]
    fn rounding_helpers_saturate_instead_of_failing() {
        assert_eq!(narrow_i64(i128::from(i64::MAX)), i64::MAX, "in range");
        assert_eq!(
            narrow_i64(i128::from(i64::MAX).saturating_mul(2)),
            i64::MAX,
            "above the top saturates"
        );
        assert_eq!(
            narrow_i64(i128::from(i64::MIN).saturating_sub(1)),
            i64::MIN,
            "below the bottom saturates"
        );
        assert_eq!(
            mul_div_round_sat(i64::MAX, i64::MAX, 1),
            i64::MAX,
            "a leaving result saturates high"
        );
        assert_eq!(
            mul_div_round_sat(i64::MIN, i64::MAX, 1),
            i64::MIN,
            "and low for negative halves"
        );
        assert_eq!(mul_div_round_sat(8, 128, 128), 8, "exact stays exact");
    }

    #[test]
    fn a_self_crossing_room_propagates_the_arrange_error() {
        let plan = plan_with(
            vec![
                polygon_room("court", vec![(0, 0), (100, 100), (100, 0), (0, 50)]),
                rect_room("yard", (200, 0), (64, 64)),
            ],
            16,
        );
        let error = build_walls(&plan).expect_err("the bowtie is refused");
        assert_eq!(error.code(), "invalid_argument", "arrange's code");
        assert!(
            error.to_string().contains("self-intersecting"),
            "the message explains the fault: {error}"
        );
    }

    #[test]
    fn room_layers_are_region_labelled_in_plan_order() {
        let plan = two_rooms(16);
        let layers = room_layers(&plan);
        assert_eq!(layers.len(), 2, "one layer per room");
        assert_eq!(layers[0].label, Label::Region(0), "first room, first label");
        assert_eq!(
            layers[1].label,
            Label::Region(1),
            "second room, second label"
        );
        assert_eq!(
            layers[0].polygon,
            vec![(0, 0), (128, 0), (128, 128), (0, 128)],
            "the room's own ring"
        );
    }
}
