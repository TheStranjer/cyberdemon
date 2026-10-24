//! The mapping back onto the source picture: the exact reduced scale, the
//! frame's points, lengths and areas, the crop offsets, the map mirror, the
//! area-filter downscale and one downscaled three-room drawing measured end
//! to end.

use crate::gfx::image::RgbaImage;
use crate::plan::Units;
use crate::sketch::{InkMode, analyze, vectorize};
use crate::tools::sketch::request::{
    Frame, Request, analyze_options, downscale, fit_edge, scale_area, scale_for, scale_length,
    vectorize_options,
};

use super::{frame, paper};

#[test]
fn the_scale_reduces_to_an_exact_fraction() {
    assert_eq!(
        scale_for(400_u32, 300_u32, 1_024_u32),
        (1_u32, 1_u32),
        "fits"
    );
    assert_eq!(scale_for(2_400_u32, 1_800_u32, 600_u32), (1_u32, 4_u32));
    assert_eq!(scale_for(1_200_u32, 900_u32, 1_024_u32), (64_u32, 75_u32));
    assert_eq!(
        scale_for(1_024_u32, 10_u32, 1_024_u32),
        (1_u32, 1_u32),
        "edge"
    );
}

#[test]
fn the_frame_maps_points_lengths_and_areas_back_exactly() {
    let shrunk = frame(1_u32, 4_u32);
    assert_eq!(shrunk.point(10_i64, 20_i64), Some((40_i64, 80_i64)));
    assert_eq!(shrunk.length(6_u32), 24_u32, "a 6 px stroke is 24 at 1/4");
    assert_eq!(shrunk.area(100_u32), 1_600_u32, "areas square the scale");
    assert_eq!(shrunk.to_analysis(96_u32), 24_u32, "lengths go in");
    assert_eq!(shrunk.to_analysis_area(1_600_u32), 100_u32, "areas go in");
    let halved = frame(1_u32, 2_u32);
    assert_eq!(halved.point(21_i64, 21_i64), Some((42_i64, 42_i64)));
    assert_eq!(halved.point(21_i64, 22_i64), Some((42_i64, 44_i64)));
    assert_eq!(fit_edge(901_u32, 1_u32, 2_u32), 451_u32, "halves round up");
    assert_eq!(scale_length(3_u32, 1_u32, 2_u32), 2_u32, "1.5 rounds to 2");
    assert_eq!(scale_area(1_u32, 2_u32, 1_u32), 4_u32, "a px is 4 at 2x");
}

#[test]
fn a_crop_offset_shifts_every_mapped_point() {
    let mut shifted = frame(1_u32, 1_u32);
    shifted.crop_x = 10_i64;
    shifted.crop_y = 20_i64;
    assert_eq!(shifted.point(5_i64, 5_i64), Some((15_i64, 25_i64)));
    assert_eq!(
        shifted
            .bbox((5_u32, 5_u32, 9_u32, 9_u32), "x")
            .expect("in range"),
        (15_i64, 25_i64, 19_i64, 29_i64),
        "the inclusive far edge stays inclusive"
    );
}

#[test]
fn units_add_map_points_and_none_stays_null() {
    let plain = frame(1_u32, 1_u32);
    assert_eq!(
        plain.map_points(&[(8_i64, 16_i64)]).expect("no units"),
        None
    );
    let mapped = Frame {
        units: Some(Units {
            scale_num: 4_i64,
            flip_y: true,
            ..Units::default()
        }),
        ..plain
    };
    let rows = mapped
        .map_points(&[(8_i64, 16_i64)])
        .expect("in range")
        .expect("a row list");
    assert_eq!(rows, vec![(32_i64, -64_i64)], "scale 4, mirrored");
    assert_eq!(
        mapped.map_centre(8_u32, 16_u32).expect("in range"),
        Some((32_i64, -64_i64)),
        "centres follow the same transform"
    );
}

#[test]
fn downscaling_keeps_the_identity_at_1_to_1() {
    let kept = downscale(paper(20_u32, 10_u32), 1_u32, 1_u32);
    assert_eq!((kept.width, kept.height), (20_u32, 10_u32), "no resample");
    let shrunk = downscale(paper(20_u32, 10_u32), 1_u32, 2_u32);
    assert_eq!((shrunk.width, shrunk.height), (10_u32, 5_u32));
    assert_eq!(fit_edge(1_u32, 1_u32, 4_096_u32), 1_u32, "never zero");
}

/// Paints the solid half-open rectangle `rect` scaled by `k`.
fn scaled_bar(sheet: &mut RgbaImage, k: u32, rect: (u32, u32, u32, u32), colour: [u8; 4]) {
    let (x0, y0, x1, y1) = rect;
    let points = (y0.checked_mul(k).expect("t")..y1.checked_mul(k).expect("t")).flat_map(|y| {
        (x0.checked_mul(k).expect("t")..x1.checked_mul(k).expect("t")).map(move |x| (x, y))
    });
    for (column, row) in points {
        assert!(sheet.set(column, row, colour), "fixture in bounds");
    }
}

/// The three-room fixture's walls drawn at `k` times.
fn rooms_at(k: u32) -> RgbaImage {
    let mut sheet = paper(
        k.checked_mul(400).expect("t"),
        k.checked_mul(300).expect("t"),
    );
    let walls: [(u32, u32, u32, u32); 12] = [
        (20, 20, 24, 164),
        (20, 20, 94, 24),
        (90, 20, 94, 94),
        (90, 90, 154, 94),
        (150, 90, 154, 164),
        (20, 160, 154, 164),
        (240, 20, 244, 281),
        (357, 20, 361, 281),
        (240, 20, 361, 24),
        (240, 277, 361, 281),
        (240, 157, 281, 161),
        (311, 157, 361, 161),
    ];
    for wall in walls {
        scaled_bar(&mut sheet, k, wall, [0, 0, 0, 255]);
    }
    sheet
}

#[test]
fn a_downscaled_fixture_still_measures_three_regions() {
    let shrunk_image = rooms_at(6);
    let (num, den) = scale_for(shrunk_image.width, shrunk_image.height, 600_u32);
    assert_eq!((num, den), (1_u32, 4_u32), "2400 into 600 is a quarter");
    let picture = downscale(shrunk_image, num, den);
    assert_eq!((picture.width, picture.height), (600_u32, 450_u32));
    let frame = Frame {
        num,
        den,
        crop_x: 0_i64,
        crop_y: 0_i64,
        source_width: 2_400_u32,
        source_height: 1_800_u32,
        units: None,
    };
    let request = Request {
        source: String::new(),
        crop: None,
        max_size: 600_u32,
        ink: InkMode::Dark { level: None },
        close_gaps: Some(100_u32),
        min_region_area: None,
        mark_max_size: None,
        simplify: None,
        orthogonal: true,
        weld: None,
        units: None,
        max_gaps: 50_usize,
        max_marks: 50_usize,
        noise_floor_px: None,
        verbose: false,
    };
    let options = analyze_options(&request, &frame);
    let result = analyze(&picture, &options).expect("analysable");
    assert_eq!(result.close_radius, 25_u32, "100 original px at 1/4");
    assert_eq!(result.region_info.len(), 3, "three rooms survive the blur");
    let shapes = vectorize(&result, &vectorize_options(&request, &frame)).expect("vectorizable");
    assert_eq!(shapes.regions.len(), 3, "three vectorised rings");
}
