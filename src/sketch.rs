//! Raster sketch measurement: ink, strokes, morphology, components, regions.
//!
//! This is the pure raster engine behind sketch tracing (the `trace_sketch`
//! tool family). Vision decides *what* a sketch shows; this module only
//! *measures* it in pixels: [`ink_mask`] turns a drawing into ink and paper,
//! [`stroke_width`] estimates the pen width, [`dilate`]/[`erode`]/[`close`]
//! run square-structuring-element morphology, [`label_components`] numbers
//! the connected pieces, and [`fn@analyze`] composes them into the
//! measurable picture of one sketch: [`Analysis`] carries the wall mask,
//! its closed form, the enclosed [`Component`] regions, the [`Gap`]s where
//! the user left breaks in the walls and the small [`Mark`] strokes (dots,
//! crosses, letters) that were filtered out of the walls.
//!
//! Everything here is integer-only, deterministic and side-effect free:
//! no floats, no `MCP`, no filesystem. Component ids follow raster scan
//! order so repeated runs label the same sketch identically.
//! Vectorisation and lump writing build on these measurements; they never
//! re-interpret them.

pub mod analyze;
pub mod components;
pub mod mask;
pub mod morph;

pub use analyze::{Analysis, AnalyzeOptions, Gap, Mark, analyze};
pub use components::{Component, Connectivity, Labels, label_components};
pub use mask::{InkMode, Mask, ink_mask, stroke_width};
pub use morph::{close, dilate, erode};
