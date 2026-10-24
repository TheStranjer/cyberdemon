//! Unit tests of the sketch group: the trace parameters' parsing, the
//! frame's exact route back onto the source picture, the report rows and
//! the floor, ranking and caps that thin the two long lists.
//!
//! [`paper`], [`frame`] and [`args`] are the fixtures the topics share.
//! [`params`] checks the wire parsing and the crop refusals, [`mapping`]
//! the scale and the frame's mapping (including one downscaled drawing
//! measured end to end), [`report`] the id-tagged rows and the report's
//! next steps and [`thin`] the noise floor, the ranking, the caps and the
//! warnings naming what stayed out.

mod mapping;
mod params;
mod report;
mod thin;

use crate::gfx::image::RgbaImage;
use crate::tools::sketch::request::Frame;
use crate::tools::sketch::{TraceSketchParams, default_grid, default_orthogonal};

/// A `width` by `height` white test image.
fn paper(width: u32, height: u32) -> RgbaImage {
    let mut pixels = Vec::new();
    for _ in 0..width.saturating_mul(height) {
        pixels.extend_from_slice(&[255, 255, 255, 255]);
    }
    RgbaImage::new(width, height, pixels).expect("fixture")
}

/// A frame at scale `num / den` with no crop and no units.
fn frame(num: u32, den: u32) -> Frame {
    Frame {
        num,
        den,
        crop_x: 0_i64,
        crop_y: 0_i64,
        source_width: 800_u32,
        source_height: 600_u32,
        units: None,
    }
}

/// The trace parameters with only the source set.
fn args(source: &str) -> TraceSketchParams {
    TraceSketchParams {
        source: source.to_owned(),
        crop: None,
        max_size: None,
        ink: None,
        close_gaps: None,
        min_region_area: None,
        mark_max_size: None,
        simplify: None,
        orthogonal: default_orthogonal(),
        weld: None,
        units: None,
        grid: default_grid(),
        max_gaps: None,
        max_marks: None,
        noise_floor_px: None,
        verbose: false,
    }
}
