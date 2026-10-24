//! Texturing: the flat and sky texels a column samples, the texture tops
//! the pegging rules pick, the `u` a hit maps to, the wrapped texel
//! coordinates, the texture keys and the wall directions fake contrast
//! splits the cardinals on.

use super::{frac, raw, solid, unit};
use crate::fixed::Angle;
use crate::render::light::WallDir;
use crate::render::view::ViewCamera;
use crate::render::view::projection::Projection;
use crate::render::view::texels::{
    flat_texel, masked_tex_top, sky_column, tex_top, texture_key, texture_u16, wall_dir, wrap16,
};
use crate::render::view::walk::{Edge, FacePlan, Plane, Side, Slot, Visit};

#[test]
fn flat_texels_wrap_the_world_grid() {
    assert_eq!(
        flat_texel(raw(3), raw(-5), 64, 64),
        Some((3, 5)),
        "a plain point, its row measured from the negated y"
    );
    assert_eq!(
        flat_texel(raw(67), raw(0), 64, 64),
        Some((3, 0)),
        "one lap east"
    );
    assert_eq!(
        flat_texel(raw(-1), raw(0), 64, 64),
        Some((63, 0)),
        "west of zero wraps back"
    );
    assert_eq!(
        flat_texel(raw(-64), raw(-64), 64, 64),
        Some((0, 0)),
        "exact laps land on the origin texel"
    );
    assert_eq!(
        flat_texel(raw(-1), raw(64), 64, 128),
        Some((63, 64)),
        "rows wrap into a 64x128 flat"
    );
    let under = raw(-1).checked_add(1).expect("test sum");
    assert_eq!(
        flat_texel(under, under, 64, 64),
        Some((63, 0)),
        "floor of -0.000015 is -1, of its negation 0"
    );
}

#[test]
fn sky_columns_bucket_the_ray_angle() {
    assert_eq!(sky_column(unit(Angle::EAST), 256), 0, "due east");
    assert_eq!(
        sky_column(unit(Angle::from_degrees(3)), 256),
        8,
        "3 degrees sits in slice 8"
    );
    assert_eq!(
        sky_column(unit(Angle::from_degrees(89)), 256),
        253,
        "just short of north"
    );
    assert_eq!(
        sky_column(unit(Angle::from_degrees(91)), 256),
        2,
        "slice 258 wraps the 256-wide sky, which repeats four times a turn"
    );
    assert_eq!(
        sky_column(unit(Angle::from_degrees(359)), 256),
        253,
        "one degree short of the turn"
    );
    assert_eq!(
        sky_column(unit(Angle::from_degrees(359)), 128),
        125,
        "a 128-wide sky repeats eight times"
    );
}

#[test]
fn sky_rows_use_dooms_placement_and_clamp() {
    let camera = ViewCamera {
        x: 0,
        y: 0,
        z: 0,
        angle: 0,
        pitch: 0,
        fov: 90,
    };
    let proj = Projection::new(&camera, 160, 100);
    assert_eq!(proj.sky_row(0, 128), 0, "the frame top is the sky top");
    assert_eq!(proj.sky_row(49, 128), 98, "one row over the horizon");
    assert_eq!(proj.sky_row(50, 128), 100, "the horizon row reads mid-sky");
    assert_eq!(proj.sky_row(63, 128), 126, "still inside the 128 rows");
    assert_eq!(
        proj.sky_row(64, 128),
        127,
        "the placement saturates at the sky's last row"
    );
    assert_eq!(proj.sky_row(99, 64), 63, "a short sky clamps early");
    let up = ViewCamera {
        pitch: 45,
        ..camera
    };
    let raised = Projection::new(&up, 160, 100);
    assert_eq!(
        raised.sky_row(0, 128),
        0,
        "looking up pushes the top row past the sky's top, clamped"
    );
}

#[test]
fn texture_tops_follow_the_pegging_rules() {
    let image = solid(64, 32, 7);
    let cases = [
        (Slot::Upper, false, 128, "pegged to the far ceiling"),
        (
            Slot::Upper,
            true,
            128,
            "unpegged measures from the near ceiling",
        ),
        (
            Slot::Middle,
            false,
            128,
            "a solid middle hangs from the ceiling",
        ),
        (Slot::Middle, true, 32, "unpegged stands on the floor"),
        (
            Slot::Lower,
            false,
            32,
            "a pegged lower tops out at the far floor",
        ),
        (Slot::Lower, true, 128, "unpegged scrolls from the ceiling"),
    ];
    for (slot, unpegged, expected, why) in cases {
        let (edge, near, far) = step_parts(unpegged);
        let visit = Visit {
            edge: &edge,
            side_index: 0,
            side: &near,
            far_side: Some(&far),
            t: frac(12, 10),
            s: frac(1, 2),
            front: true,
            y_ceil: 0,
            y_floor: 0,
            y_far_ceil: None,
            y_far_floor: None,
        };
        let plan = FacePlan {
            visit: &visit,
            slot,
            image: &image,
            span: (0, 10),
            depth16: raw(100),
        };
        assert_eq!(tex_top(&plan), expected, "{why}");
    }
}

/// A near/far pair split by a 32-unit step under a 96-unit far
/// ceiling, on an edge carrying the asked-for unpegged bits. Near
/// floor 0 ceiling 128, far floor 32 ceiling 96, texture 32 tall:
/// the pegged cases differ from the unpegged ones by the table above.
fn step_parts(unpegged: bool) -> (Edge, Side<'static>, Side<'static>) {
    let near = Plane {
        floor: 0,
        ceil: 128,
        light: 255,
        floor_surface: None,
        ceil_surface: None,
        ceil_sky: false,
    };
    let far = Plane {
        floor: 32,
        ceil: 96,
        ..near
    };
    (
        Edge {
            line: 0,
            x0: 0,
            y0: 0,
            ex: 0,
            ey: -256,
            len16: raw(256),
            dir: WallDir::NorthSouth,
            front: 0,
            back: Some(1),
            upper_unpegged: unpegged,
            lower_unpegged: unpegged,
        },
        Side {
            plane: near,
            upper: Some("WALL1".to_owned()),
            lower: Some("STEP1".to_owned()),
            middle: None,
            x_offset: 0,
            y_offset: 0,
        },
        Side {
            plane: far,
            upper: None,
            lower: None,
            middle: None,
            x_offset: 0,
            y_offset: 0,
        },
    )
}

/// A [`Visit`] over `parts` at `s = 1/4`, facing the asked-for
/// side, for the mapping helper.
fn step_visit<'fix>(parts: &'fix (Edge, Side<'fix>, Side<'fix>), front: bool) -> Visit<'fix> {
    Visit {
        edge: &parts.0,
        side_index: 0,
        side: &parts.1,
        far_side: Some(&parts.2),
        t: frac(12, 10),
        s: frac(1, 4),
        front,
        y_ceil: 0,
        y_floor: 0,
        y_far_ceil: None,
        y_far_floor: None,
    }
}

#[test]
fn texture_u_measures_from_the_faced_end_and_adds_the_shift() {
    let parts = step_parts(false);
    let front = texture_u16(&step_visit(&parts, true));
    assert_eq!(front, raw(64), "a quarter along the 256-long line");
    let back = texture_u16(&step_visit(&parts, false));
    assert_eq!(back, raw(192), "a quarter measured from v2 backwards");
    let mut shifted = parts;
    shifted.1.x_offset = 10;
    assert_eq!(
        texture_u16(&step_visit(&shifted, true)),
        raw(74),
        "the sidedef's x shift rides along"
    );
}

#[test]
fn masked_tops_hang_from_the_low_ceiling_and_stand_on_the_high_floor() {
    let (pegged, near, far) = step_parts(false);
    assert_eq!(
        masked_tex_top(&near.plane, &far.plane, 32_i64, pegged.lower_unpegged),
        96,
        "pegged hangs from the lower ceiling: near 0/128, far 32/96"
    );
    let (unpegged, near_base, far_base) = step_parts(true);
    assert_eq!(
        masked_tex_top(
            &near_base.plane,
            &far_base.plane,
            32_i64,
            unpegged.lower_unpegged
        ),
        64,
        "lower-unpegged stands on the higher floor 32 plus the 32-tall texture"
    );
}

#[test]
fn wrapped_texture_coordinates_stay_in_the_image() {
    assert_eq!(wrap16(raw(3), 64), 3, "a plain column");
    assert_eq!(wrap16(raw(67), 64), 3, "one lap");
    assert_eq!(wrap16(-raw(1), 64), 63, "below zero wraps up");
    assert_eq!(wrap16(-raw(64), 64), 0, "exactly one lap back");
    assert_eq!(wrap16(raw(1), 0), 0, "an empty texture reads column zero");
}

#[test]
fn texture_keys_trim_uppercase_and_drop_the_none_marker() {
    assert_eq!(texture_key(" wall1 "), Some("WALL1".to_owned()));
    assert_eq!(texture_key("-"), None, "the none-marker");
    assert_eq!(texture_key("  "), None, "an empty name");
}

#[test]
fn wall_directions_split_the_cardinals() {
    assert_eq!(wall_dir(64, 0), WallDir::EastWest);
    assert_eq!(wall_dir(0, -64), WallDir::NorthSouth);
    assert_eq!(wall_dir(3, 4), WallDir::Other);
}
