//! Plan coordinates to map units: integer scaling, mirroring, snapping.
//!
//! A plan is drawn in whatever units the caller thinks in: pixels of a
//! sketch, half-tiles, centimetres. [`Units`] maps such a plan coordinate
//! into `Doom` map units: `map_x = origin_x + x * scale_num / scale_den`
//! and `map_y = origin_y + y * scale_num / scale_den`, the `y` term
//! subtracted instead when [`flip_y`](Units::flip_y) mirrors the drawing
//! vertically. Every coordinate rounds through [`mul_div_round`] (halves
//! away from zero) and is then pulled to the nearest multiple of the snap
//! grid, so no float is involved anywhere and a plan lands on whole map
//! units.
//!
//! [`transform_plan`] applies that to a whole [`Plan`] and returns a new
//! one. It runs *before* [`compile`](fn@crate::plan::compile): it is the
//! stage that turns a drawing into map-sized geometry, so walls,
//! connectors and emission all see final coordinates. [`Units::point`]
//! exposes the same transform for a single point; [`Units::to_map`] and
//! [`Units::to_image`] are its unsnapped leg and its exact inverse, the
//! pair a reference image is placed and sampled with. The transformed
//! fields are the room shapes, each [`Connection::at`] placement and each
//! [`Feature::Teleporter`]'s `pad` and `destination` squares.
//!
//! Sizes are **not** transformed. [`Plan::wall_thickness`],
//! [`Connection::width`], the heights and depths inside
//! [`ConnectionKind`](crate::plan::model::ConnectionKind) (`sill`,
//! `lintel`, `step_height`, `step_depth`) and the
//! [`Feature::Lift`] `low_floor` height are already map units: they are
//! the wall the compiler grows and the openings it cuts, so scaling them
//! with the coordinates would apply the scale twice. They pass through
//! [`transform_plan`] untouched.

use crate::error::CyberError;
use crate::fixed::mul_div_round;
use crate::geom::Pt;
use crate::plan::model::{Connection, Feature, Plan, Room, Shape};

/// How plan coordinates become map units: an integer scale, an origin, an
/// optional vertical mirror and a snap grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Units {
    /// Numerator of the scale: with `scale_den` `1`, `4` makes one plan
    /// unit four map units.
    pub scale_num: i64,
    /// Denominator of the scale; `0` is refused.
    pub scale_den: i64,
    /// Map `x` the plan origin lands on.
    pub origin_x: i64,
    /// Map `y` the plan origin lands on, and the line
    /// [`flip_y`](Self::flip_y) mirrors about.
    pub origin_y: i64,
    /// The plan's snap grid in map units, validated by [`Units::check`].
    /// Callers pass it to [`Units::point`] and [`transform_plan`], which
    /// take the grid to apply as an argument so one set of units can snap
    /// different coordinate sets differently.
    pub grid: i64,
    /// Mirror `y` about [`origin_y`](Self::origin_y): plan `y` grows up,
    /// map `y` grows down.
    pub flip_y: bool,
}

impl Default for Units {
    /// The identity: scale `1/1`, both origins `0`,
    /// [`grid`](Self::grid) `1` (no snapping) and no mirror.
    #[inline]
    fn default() -> Self {
        Self {
            scale_num: 1_i64,
            scale_den: 1_i64,
            origin_x: 0_i64,
            origin_y: 0_i64,
            grid: 1_i64,
            flip_y: false,
        }
    }
}

impl Units {
    /// Checks the numbers of this transform can run: a non-zero
    /// [`scale_den`](Self::scale_den) (it divides), a non-zero
    /// [`scale_num`](Self::scale_num) (`0` would collapse the whole plan
    /// onto the origin) and a [`grid`](Self::grid) of at least `1`.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the offending field.
    #[inline]
    pub fn check(&self) -> Result<(), CyberError> {
        if self.scale_den == 0_i64 {
            return Err(CyberError::InvalidArgument(
                "scale_den is 0; the scale needs a non-zero denominator".to_owned(),
            ));
        }
        if self.scale_num == 0_i64 {
            return Err(CyberError::InvalidArgument(
                "scale_num is 0; every coordinate would collapse onto the origin".to_owned(),
            ));
        }
        validate_grid(self.grid)
    }

    /// One plan point in map units: scale, mirror about
    /// [`origin_y`](Self::origin_y) when [`flip_y`](Self::flip_y), then
    /// snap to `grid`. This is what [`transform_plan`] does to every
    /// coordinate, exposed for a caller placing a single thing.
    ///
    /// `grid` is the snapping applied here; it is normally
    /// [`Units::grid`]. `None` when a coordinate leaves the supported
    /// range or `grid` is below `1` ([`Units::check`] rejects such a
    /// grid).
    #[must_use]
    #[inline]
    pub fn point(&self, grid: i64, x: i64, y: i64) -> Option<Pt> {
        let map_x = self.axis(grid, x, false)?;
        let map_y = self.axis(grid, y, true)?;
        Some((map_x, map_y))
    }

    /// One coordinate of the transform: `origin + value * num / den`,
    /// subtracted from [`origin_y`](Self::origin_y) on a mirrored
    /// vertical axis, then snapped to `grid`.
    #[inline]
    fn axis(&self, grid: i64, value: i64, vertical: bool) -> Option<i64> {
        let scaled = mul_div_round(value, self.scale_num, self.scale_den)?;
        let placed = if vertical && self.flip_y {
            self.origin_y.checked_sub(scaled)?
        } else if vertical {
            self.origin_y.checked_add(scaled)?
        } else {
            self.origin_x.checked_add(scaled)?
        };
        snap(placed, grid)
    }

    /// One drawing pixel in map units with no snapping: [`point`](Self::point)
    /// at grid `1`. This is the image-pixel to map-unit direction a
    /// reference picture travels; the inverse is [`to_image`](Self::to_image).
    ///
    /// A sketch pixel `(px, py)` lands at `x = origin_x + px * num / den`
    /// and `y = origin_y - py * num / den` when [`flip_y`](Self::flip_y)
    /// mirrors the drawing vertically, `y = origin_y + py * num / den`
    /// without the mirror (both [`mul_div_round`]-rounded).
    #[must_use]
    #[inline]
    pub fn to_map(self, x: i64, y: i64) -> Option<Pt> {
        self.point(1_i64, x, y)
    }

    /// One map point back in the drawing's pixel space: the inverse of
    /// [`to_map`](Self::to_map), so `px = (x - origin_x) * den / num` and
    /// `py = (origin_y - y) * den / num` when [`flip_y`](Self::flip_y)
    /// mirrors, `py = (y - origin_y) * den / num` without the mirror.
    /// Divisions round through [`mul_div_round`]; `None` when a value
    /// leaves the supported range or the scale is degenerate (zero
    /// [`scale_num`](Self::scale_num) or [`scale_den`](Self::scale_den),
    /// which [`check`](Self::check) refuses).
    #[must_use]
    #[inline]
    pub fn to_image(self, x: i64, y: i64) -> Option<Pt> {
        let px = self.unaxis(x, self.origin_x, false)?;
        let py = self.unaxis(y, self.origin_y, true)?;
        Some((px, py))
    }

    /// The inverse of one [`axis`](Self::axis) leg: the distance from the
    /// origin, measured downward on a mirrored vertical axis, divided by
    /// the scale.
    #[inline]
    fn unaxis(self, placed: i64, origin: i64, vertical: bool) -> Option<i64> {
        let offset = if vertical && self.flip_y {
            origin.checked_sub(placed)?
        } else {
            placed.checked_sub(origin)?
        };
        mul_div_round(offset, self.scale_den, self.scale_num)
    }

    /// [`Units::point`] as a `Result`, reporting a failure at the plan
    /// `path`. The caller has checked the units and the grid, so `None`
    /// here can only be a coordinate leaving the range.
    #[inline]
    fn mapped(&self, grid: i64, path: &str, point: Pt) -> Result<Pt, CyberError> {
        let (x, y) = point;
        let map_x = self.axis(grid, x, false).ok_or_else(|| range_error(path))?;
        let map_y = self.axis(grid, y, true).ok_or_else(|| range_error(path))?;
        Ok((map_x, map_y))
    }

    /// The transformed rectangle at `path`, from its plan `sides`
    /// `(x, y, w, h)`: both corners are scaled, mirrored and snapped, and
    /// the extents are re-derived from the snapped corners, so the
    /// rectangle stays axis-aligned and grid-aligned.
    #[inline]
    fn rectangle(
        &self,
        grid: i64,
        path: &str,
        sides: (i64, i64, i64, i64),
    ) -> Result<Shape, CyberError> {
        let (left, bottom, width, height) = sides;
        let right = left
            .checked_add(width)
            .ok_or_else(|| range_error(&field(path, "w")))?;
        let top = bottom
            .checked_add(height)
            .ok_or_else(|| range_error(&field(path, "h")))?;
        let near = (
            self.axis(grid, left, false)
                .ok_or_else(|| range_error(&field(path, "x")))?,
            self.axis(grid, bottom, true)
                .ok_or_else(|| range_error(&field(path, "y")))?,
        );
        let far = (
            self.axis(grid, right, false)
                .ok_or_else(|| range_error(&field(path, "w")))?,
            self.axis(grid, top, true)
                .ok_or_else(|| range_error(&field(path, "h")))?,
        );
        rectangle_from(path, near, far)
    }
}

/// Rewrites `plan`'s coordinates into map units under `units`, snapping
/// every transformed coordinate to `grid` (`1` disables snapping).
///
/// Rectangles have both corners transformed and their `w`/`h` re-derived
/// from the snapped corners, so they stay grid-aligned; polygon rings,
/// the [`at`](Connection::at) position of each connection and the `pad`
/// and `destination` squares of each [`Feature::Teleporter`] are
/// transformed point by point. Sector properties, [`Plan::wall_thickness`]
/// and every size (`width`, `sill`, `lintel`, `step_height`,
/// `step_depth`, a lift's `low_floor`) are map units already and pass
/// through unchanged, as this module's documentation explains. The plan's
/// structure is not re-checked here; [`Plan::validate`] still runs on the
/// result.
///
/// # Errors
///
/// [`Units::check`]'s refusal naming the field, the same refusal for a
/// `grid` below `1`, and [`CyberError::InvalidArgument`] naming the plan
/// path (`rooms[2].rect.x`, `connections[0].at`) of a coordinate that
/// leaves the supported range or of a rectangle the grid collapses.
#[inline]
pub fn transform_plan(plan: &Plan, units: &Units, grid: i64) -> Result<Plan, CyberError> {
    units.check()?;
    validate_grid(grid)?;
    let rooms = transform_rooms(plan, units, grid)?;
    let connections = transform_connections(plan, units, grid)?;
    let features = transform_features(plan, units, grid)?;
    Ok(Plan {
        rooms,
        connections,
        features,
        wall_thickness: plan.wall_thickness,
    })
}

/// Every room with its shape transformed; the id, the heights, the light
/// and the flat and texture names are not coordinates and pass through.
#[inline]
fn transform_rooms(plan: &Plan, units: &Units, grid: i64) -> Result<Vec<Room>, CyberError> {
    plan.rooms
        .iter()
        .enumerate()
        .map(|(index, room)| {
            let path = format!("rooms[{index}]");
            Ok(Room {
                shape: transform_shape(units, grid, &path, &room.shape)?,
                ..room.clone()
            })
        })
        .collect()
}

/// One shape under the plan `path`.
#[inline]
fn transform_shape(
    units: &Units,
    grid: i64,
    path: &str,
    shape: &Shape,
) -> Result<Shape, CyberError> {
    match shape.clone() {
        Shape::Rect { x, y, w, h } => units.rectangle(grid, path, (x, y, w, h)),
        Shape::Polygon { points } => Ok(Shape::Polygon {
            points: transform_points(units, grid, path, &points)?,
        }),
    }
}

/// A polygon ring point by point, each reported at
/// `rooms[i].polygon[j]`.
#[inline]
fn transform_points(
    units: &Units,
    grid: i64,
    path: &str,
    points: &[Pt],
) -> Result<Vec<Pt>, CyberError> {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let name = format!("{path}.polygon[{index}]");
            units.mapped(grid, &name, *point)
        })
        .collect()
}

/// Every connection with its `at` transformed; `id`, `from`, `to`,
/// `kind` and the map-unit `width` pass through.
#[inline]
fn transform_connections(
    plan: &Plan,
    units: &Units,
    grid: i64,
) -> Result<Vec<Connection>, CyberError> {
    plan.connections
        .iter()
        .enumerate()
        .map(|(index, connection)| {
            let path = format!("connections[{index}]");
            Ok(Connection {
                at: transform_at(units, grid, &path, connection.at)?,
                ..connection.clone()
            })
        })
        .collect()
}

/// One connection's placement, `None` centring staying `None`.
#[inline]
fn transform_at(
    units: &Units,
    grid: i64,
    path: &str,
    at: Option<Pt>,
) -> Result<Option<Pt>, CyberError> {
    at.map(|point| units.mapped(grid, &format!("{path}.at"), point))
        .transpose()
}

/// Every feature with its teleporter squares transformed; the other
/// kinds carry no coordinates and clone through.
#[inline]
fn transform_features(plan: &Plan, units: &Units, grid: i64) -> Result<Vec<Feature>, CyberError> {
    plan.features
        .iter()
        .enumerate()
        .map(|(index, feature)| transform_feature(units, grid, index, feature))
        .collect()
}

/// One feature: a teleporter's `pad` and `destination` travel the same
/// transform as a connection's `at`, reported under the
/// `features[index]` path.
fn transform_feature(
    units: &Units,
    grid: i64,
    index: usize,
    feature: &Feature,
) -> Result<Feature, CyberError> {
    let path = format!("features[{index}]");
    Ok(match feature.clone() {
        Feature::Teleporter {
            from_room,
            to_room,
            pad,
            destination,
            angle,
            two_way,
        } => Feature::Teleporter {
            from_room,
            to_room,
            pad: pad
                .map(|point| units.mapped(grid, &format!("{path}.pad"), point))
                .transpose()?,
            destination: destination
                .map(|point| units.mapped(grid, &format!("{path}.destination"), point))
                .transpose()?,
            angle,
            two_way,
        },
        other @ (Feature::Exit { .. } | Feature::Switch { .. } | Feature::Lift { .. }) => other,
    })
}

/// A rectangle from two opposite map-space corners, normalised to the
/// lower-left with the extents re-derived from the corners themselves.
#[inline]
fn rectangle_from(path: &str, first: Pt, second: Pt) -> Result<Shape, CyberError> {
    Ok(Shape::Rect {
        x: first.0.min(second.0),
        y: first.1.min(second.1),
        w: axis_gap(path, "w", first.0, second.0)?,
        h: axis_gap(path, "h", first.1, second.1)?,
    })
}

/// The snapped distance between two corners along one axis, refused when
/// it is zero: a rectangle the scale or the grid collapses cannot be a
/// room.
#[inline]
fn axis_gap(path: &str, name: &str, one: i64, other: i64) -> Result<i64, CyberError> {
    let field = field(path, name);
    let gap = one
        .max(other)
        .checked_sub(one.min(other))
        .ok_or_else(|| range_error(&field))?;
    if gap == 0_i64 {
        return Err(CyberError::InvalidArgument(format!(
            "{field} collapses to 0 under the plan transform; use a smaller grid or scale"
        )));
    }
    Ok(gap)
}

/// `value` pulled to the nearest multiple of `grid`, halves away from
/// zero like [`mul_div_round`]. `None` when `grid` is below `1` or the
/// snapped value leaves `i64`.
#[inline]
fn snap(value: i64, grid: i64) -> Option<i64> {
    if grid < 1_i64 {
        return None;
    }
    let magnitude = i128::from(value).checked_abs()?;
    let step = i128::from(grid);
    let cells = magnitude.checked_div(step)?;
    let rest = magnitude.checked_rem(step)?;
    let carried = if rest.checked_mul(2_i128)? >= step {
        cells.checked_add(1_i128)?
    } else {
        cells
    };
    let snapped = carried.checked_mul(step)?;
    let signed = if value >= 0 {
        snapped
    } else {
        snapped.checked_neg()?
    };
    i64::try_from(signed).ok()
}

/// Refuses a snap grid below `1`; `1` snaps every coordinate to itself.
#[inline]
fn validate_grid(grid: i64) -> Result<(), CyberError> {
    if grid >= 1_i64 {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "grid {grid} is below 1; use 1 to switch snapping off"
    )))
}

/// The plan path of one rectangle field: `rooms[2].rect.w`.
#[inline]
fn field(path: &str, name: &str) -> String {
    format!("{path}.rect.{name}")
}

/// The refusal of a plan coordinate the transform cannot carry, naming
/// where in the plan it sits.
#[inline]
fn range_error(path: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{path} leaves the supported range under the plan transform; \
         reduce scale_num, the origin or the coordinates"
    ))
}

#[cfg(test)]
mod tests {
    use super::{Units, snap, transform_plan};
    use crate::error::CyberError;
    use crate::geom::Pt;
    use crate::plan::model::{Connection, ConnectionKind, Feature, Plan, Room, Shape};
    use crate::wad::map::features::LiftStyle;

    /// A rectangular room with default sector properties, based at `at`
    /// and `size` wide.
    fn rect_room(id: &str, at: Pt, size: Pt) -> Room {
        Room {
            id: id.to_owned(),
            shape: Shape::Rect {
                x: at.0,
                y: at.1,
                w: size.0,
                h: size.1,
            },
            floor: 0_i32,
            ceiling: 128_i32,
            light: 160_i32,
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
            floor: 0_i32,
            ceiling: 128_i32,
            light: 160_i32,
            floor_flat: None,
            ceiling_flat: None,
            wall_texture: None,
            outdoor: false,
            secret: false,
            light_effect: None,
        }
    }

    /// An opening between two rooms, placed at `at` when given.
    fn opening(from: &str, to: &str, at: Option<Pt>) -> Connection {
        Connection {
            id: None,
            from: from.to_owned(),
            to: to.to_owned(),
            kind: ConnectionKind::Opening,
            at,
            width: 48_i64,
        }
    }

    /// A plan over `rooms` and `connections` with a 16-unit wall.
    fn plan_with(rooms: Vec<Room>, connections: Vec<Connection>) -> Plan {
        Plan {
            rooms,
            connections,
            features: Vec::new(),
            wall_thickness: 16_i64,
        }
    }

    /// The sample plan's rooms and opening carrying `features`.
    fn plan_features(features: Vec<Feature>) -> Plan {
        Plan {
            features,
            ..sample_plan()
        }
    }

    /// A teleporter from room `a` to room `b` with the given squares.
    fn teleporter(pad: Option<Pt>, destination: Option<Pt>) -> Feature {
        Feature::Teleporter {
            from_room: "a".to_owned(),
            to_room: "b".to_owned(),
            pad,
            destination,
            angle: 90_i32,
            two_way: true,
        }
    }

    /// The two-room, one-opening plan the refusals share.
    fn sample_plan() -> Plan {
        plan_with(
            vec![
                rect_room("a", (0, 0), (64, 64)),
                rect_room("b", (96, 0), (64, 64)),
            ],
            vec![opening("a", "b", Some((80, 32)))],
        )
    }

    /// Asserts `error` is an `invalid_argument` whose message names
    /// `needle`.
    #[track_caller]
    fn assert_argument(error: &CyberError, needle: &str) {
        assert_eq!(error.code(), "invalid_argument", "code for: {error}");
        assert!(
            error.to_string().contains(needle),
            "message '{error}' must name '{needle}'"
        );
    }

    /// Asserts `units` is refused by [`Units::check`] and by
    /// [`transform_plan`], each time naming `needle`.
    #[track_caller]
    fn assert_refused(units: &Units, needle: &str) {
        let checked = units.check().expect_err("the units must be refused");
        assert_argument(&checked, needle);
        let moved = transform_plan(&sample_plan(), units, 1_i64)
            .expect_err("the transform must be refused");
        assert_argument(&moved, needle);
    }

    /// Asserts a `plan` transform under `units` and `grid` is refused
    /// naming `needle`.
    #[track_caller]
    fn assert_plan_refused(plan: &Plan, units: &Units, grid: i64, needle: &str) {
        let refused = transform_plan(plan, units, grid).expect_err("the coordinate must fail");
        assert_argument(&refused, needle);
    }

    #[test]
    fn to_map_and_to_image_invert_each_other() {
        let units = Units {
            scale_num: 4_i64,
            origin_x: 10_i64,
            origin_y: -20_i64,
            flip_y: true,
            ..Units::default()
        };
        for (px, py) in [(0_i64, 0_i64), (12_i64, 30_i64), (-8_i64, 7_i64)] {
            let (x, y) = units
                .to_map(px, py)
                .expect("small scaled coordinates stay in range");
            assert_eq!(
                units.to_image(x, y),
                Some((px, py)),
                "an exact pixel round-trips at scale 4 with the mirror"
            );
        }
        assert_eq!(
            units.to_map(0_i64, 4_i64),
            Some((10_i64, -36_i64)),
            "the unsnapped direction matches point(1, x, y)"
        );
        assert_eq!(
            units.to_image(12_i64, -20_i64),
            Some((1_i64, 0_i64)),
            "a map point between pixels rounds to the nearest, halves away from zero"
        );
        let shrink = Units {
            scale_num: 1_i64,
            scale_den: 8_i64,
            ..Units::default()
        };
        assert_eq!(
            shrink.to_image(7_i64, -7_i64),
            Some((56_i64, -56_i64)),
            "a fractional scale divides out by eight"
        );
        assert_eq!(
            Units::default().to_map(3_i64, -3_i64),
            Some((3_i64, -3_i64)),
            "the identity carries pixels through"
        );
    }

    #[test]
    fn the_default_units_are_the_identity() {
        let units = Units::default();
        units.check().expect("the identity is usable");
        assert_eq!(
            units.point(1_i64, -7_i64, 23_i64),
            Some((-7_i64, 23_i64)),
            "a point passes through"
        );
        let plan = sample_plan();
        let moved = transform_plan(&plan, &units, 1_i64).expect("the identity runs");
        assert_eq!(plan, moved, "an identity transform changes nothing");
    }

    #[test]
    fn a_scale_of_four_turns_a_hundred_pixels_into_four_hundred_units() {
        let units = Units {
            scale_num: 4_i64,
            ..Units::default()
        };
        let plan = plan_with(vec![rect_room("hall", (10, 20), (100, 50))], vec![]);
        let moved = transform_plan(&plan, &units, 1_i64).expect("scale 4 runs");
        assert_eq!(
            moved.rooms.first().expect("one room").shape,
            Shape::Rect {
                x: 40,
                y: 80,
                w: 400,
                h: 200
            },
            "the 100x50 rect becomes 400x200"
        );
    }

    #[test]
    fn flip_y_mirrors_about_the_origin() {
        let units = Units {
            origin_y: 500_i64,
            flip_y: true,
            ..Units::default()
        };
        assert_eq!(
            units.point(1_i64, 6_i64, 100_i64),
            Some((6_i64, 400_i64)),
            "y is measured down from the origin"
        );
        let plan = plan_with(vec![rect_room("attic", (0, 100), (64, 32))], vec![]);
        let moved = transform_plan(&plan, &units, 1_i64).expect("a mirror runs");
        assert_eq!(
            moved.rooms.first().expect("one room").shape,
            Shape::Rect {
                x: 0,
                y: 368,
                w: 64,
                h: 32
            },
            "the mirrored rect keeps a positive extent below its old top"
        );
    }

    #[test]
    fn snapping_pulls_every_coordinate_to_the_nearest_grid_line() {
        let units = Units {
            grid: 16_i64,
            ..Units::default()
        };
        let plan = plan_with(vec![rect_room("a", (10, 6), (100, 50))], vec![]);
        let moved = transform_plan(&plan, &units, 16_i64).expect("a 16 grid runs");
        assert_eq!(
            moved.rooms.first().expect("one room").shape,
            Shape::Rect {
                x: 16,
                y: 0,
                w: 96,
                h: 64
            },
            "10 goes to 16, 6 to 0, 110 to 112 and the 56 tie to 64"
        );
        let Shape::Rect {
            x: left,
            y: bottom,
            w: width,
            h: height,
        } = moved.rooms.first().expect("one room").shape
        else {
            panic!("a rect room stays a rect");
        };
        for value in [left, bottom, width, height] {
            assert_eq!(snap(value, 16_i64), Some(value), "{value} is grid-aligned");
        }
    }

    #[test]
    fn snapping_rounds_halves_away_from_zero_on_both_sides() {
        let units = Units::default();
        assert_eq!(
            units.point(16_i64, 10_i64, 6_i64),
            Some((16_i64, 0_i64)),
            "nearest line wins"
        );
        assert_eq!(
            units.point(16_i64, -10_i64, -24_i64),
            Some((-16_i64, -32_i64)),
            "negatives snap outward at the tie"
        );
        assert_eq!(
            units.point(8_i64, 12_i64, -4_i64),
            Some((16_i64, -8_i64)),
            "the ties 12 and -4 move away from zero"
        );
    }

    #[test]
    fn a_fractional_scale_rounds_once_per_coordinate() {
        let units = Units {
            scale_num: 2_i64,
            scale_den: 4_i64,
            origin_x: 10_i64,
            origin_y: 20_i64,
            flip_y: true,
            ..Units::default()
        };
        assert_eq!(
            units.point(8_i64, 30_i64, 30_i64),
            Some((24_i64, 8_i64)),
            "30 scaled by a half is 15, then 25 and 5, snapped to 24 and 8"
        );
    }

    #[test]
    fn polygon_points_and_connection_positions_are_transformed() {
        let units = Units {
            scale_num: 2_i64,
            ..Units::default()
        };
        let plan = plan_with(
            vec![
                polygon_room("court", vec![(0, 0), (64, 0), (32, 48)]),
                rect_room("lab", (96, 0), (64, 64)),
            ],
            vec![
                opening("court", "lab", Some((96, 32))),
                opening("lab", "court", None),
            ],
        );
        let moved = transform_plan(&plan, &units, 1_i64).expect("scale 2 runs");
        assert_eq!(
            moved.rooms.first().expect("one room").shape,
            Shape::Polygon {
                points: vec![(0, 0), (128, 0), (64, 96)]
            },
            "every polygon point is transformed"
        );
        assert_eq!(
            moved.connections.first().expect("one connection").at,
            Some((192, 64)),
            "the placement follows the rooms"
        );
        assert_eq!(
            moved.connections.get(1).expect("second connection").at,
            None,
            "an unplaced connection stays unplaced"
        );
        moved.validate().expect("the scaled plan still validates");
    }

    #[test]
    fn connection_sizes_and_wall_thickness_stay_in_map_units() {
        let units = Units {
            scale_num: 4_i64,
            grid: 8_i64,
            ..Units::default()
        };
        let window = Connection {
            kind: ConnectionKind::Window {
                sill: 32_i32,
                lintel: 24_i32,
            },
            ..opening("a", "b", Some((80, 32)))
        };
        let stairs = Connection {
            kind: ConnectionKind::Stairs {
                step_height: 16_i32,
                step_depth: 24_i32,
            },
            ..opening("b", "a", None)
        };
        let plan = plan_with(
            vec![
                rect_room("a", (0, 0), (64, 64)),
                rect_room("b", (96, 0), (64, 64)),
            ],
            vec![window, stairs],
        );
        let moved = transform_plan(&plan, &units, 8_i64).expect("the transform runs");
        assert_eq!(moved.wall_thickness, 16_i64, "the wall is a map-unit size");
        let glass = moved.connections.first().expect("the window");
        assert_eq!(glass.width, 48_i64, "an opening width is a map-unit size");
        assert_eq!(
            glass.kind,
            ConnectionKind::Window {
                sill: 32_i32,
                lintel: 24_i32
            },
            "sill and lintel are heights"
        );
        assert_eq!(
            moved.connections.get(1).expect("the stairs").kind,
            ConnectionKind::Stairs {
                step_height: 16_i32,
                step_depth: 24_i32
            },
            "step height and depth are sizes"
        );
    }

    #[test]
    fn teleporter_squares_scale_with_the_rooms() {
        let units = Units {
            scale_num: 4_i64,
            ..Units::default()
        };
        let plan = plan_features(vec![teleporter(Some((10, 12)), None)]);
        let moved = transform_plan(&plan, &units, 8_i64).expect("scale 4 runs");
        assert_eq!(
            moved.features.first().expect("one feature"),
            &teleporter(Some((40_i64, 48_i64)), None),
            "the pad travels like a placement, an absent square stays absent"
        );
    }

    #[test]
    fn teleporter_squares_mirror_and_snap_like_placements() {
        let units = Units {
            scale_num: 2_i64,
            origin_y: 100_i64,
            flip_y: true,
            ..Units::default()
        };
        let plan = plan_features(vec![teleporter(Some((5, 10)), Some((-14, 30)))]);
        let moved = transform_plan(&plan, &units, 8_i64).expect("the mirror runs");
        assert_eq!(
            moved.features.first().expect("one feature"),
            &teleporter(Some((8_i64, 80_i64)), Some((-32_i64, 40_i64))),
            "10 goes to 80 below the origin, -28 snaps away from zero to -32"
        );
    }

    #[test]
    fn a_lifts_low_floor_is_a_height_and_stays_put() {
        let units = Units {
            scale_num: 4_i64,
            ..Units::default()
        };
        let lift = Feature::Lift {
            room: "a".to_owned(),
            low_floor: Some(-64_i32),
            style: LiftStyle::WalkTrigger,
            repeatable: false,
        };
        let plan = plan_features(vec![lift.clone()]);
        let moved = transform_plan(&plan, &units, 8_i64).expect("scale 4 runs");
        assert_eq!(moved.features, vec![lift], "no coordinate, no transform");
    }

    #[test]
    fn an_overflowing_teleporter_square_names_its_path() {
        let units = Units {
            origin_x: i64::MAX - 1_000_i64,
            ..Units::default()
        };
        let plan = plan_features(vec![teleporter(Some((5_000, 32)), None)]);
        assert_plan_refused(&plan, &units, 1_i64, "features[0].pad");
    }

    #[test]
    fn a_zero_denominator_is_refused_naming_scale_den() {
        let units = Units {
            scale_den: 0_i64,
            ..Units::default()
        };
        assert_refused(&units, "scale_den");
    }

    #[test]
    fn a_zero_numerator_is_refused_naming_scale_num() {
        let units = Units {
            scale_num: 0_i64,
            ..Units::default()
        };
        assert_refused(&units, "scale_num");
    }

    #[test]
    fn a_zero_or_negative_grid_is_refused_naming_grid() {
        for grid in [0_i64, -8_i64] {
            assert_refused(
                &Units {
                    grid,
                    ..Units::default()
                },
                "grid",
            );
            let refused = transform_plan(&sample_plan(), &Units::default(), grid)
                .expect_err("snapping needs a grid");
            assert_argument(&refused, "grid");
        }
        assert_eq!(
            Units::default().point(0_i64, 4_i64, 4_i64),
            None,
            "a grid below 1 snaps nothing"
        );
    }

    #[test]
    fn an_overflowing_rectangle_corner_names_its_field() {
        let units = Units {
            scale_num: i64::MAX,
            ..Units::default()
        };
        let plan = plan_with(vec![rect_room("a", (100, 0), (64, 64))], vec![]);
        assert_plan_refused(&plan, &units, 1_i64, "rooms[0].rect.x");
    }

    #[test]
    fn an_overflowing_polygon_point_names_its_index() {
        let units = Units {
            scale_num: i64::MAX,
            ..Units::default()
        };
        let plan = plan_with(
            vec![polygon_room("a", vec![(0, 0), (64, 0), (0, 64)])],
            vec![],
        );
        assert_plan_refused(&plan, &units, 1_i64, "rooms[0].polygon[1]");
    }

    #[test]
    fn an_overflowing_connection_placement_names_its_path() {
        let units = Units {
            origin_x: i64::MAX - 1_000_i64,
            ..Units::default()
        };
        let plan = plan_with(
            vec![
                rect_room("a", (0, 0), (64, 64)),
                rect_room("b", (96, 0), (64, 64)),
            ],
            vec![opening("a", "b", Some((5_000, 32)))],
        );
        assert_plan_refused(&plan, &units, 1_i64, "connections[0].at");
    }

    #[test]
    fn a_rectangle_the_grid_collapses_is_refused_naming_its_extent() {
        let units = Units {
            grid: 4_096_i64,
            ..Units::default()
        };
        let plan = plan_with(vec![rect_room("a", (0, 0), (64, 64))], vec![]);
        assert_plan_refused(&plan, &units, 4_096_i64, "rooms[0].rect.w");
    }
}
