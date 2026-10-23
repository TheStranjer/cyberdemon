//! The shared preview-pipeline helper: fetched picture bytes in, processed
//! picture (and optionally its indexed form) out.
//!
//! [`ProcessParams`] is the parameter block the `process_image` and
//! `create_texture_from_image` tools flatten into their arguments: the
//! ordered [`ImageOp`] list from [`crate::gfx::ops`], the palette
//! [`Dither`] mode and the `PLAYPAL` entry. [`process`] decodes the fetched
//! bytes with [`decode_image`] and runs [`apply_ops`], keeping the picture
//! and the per-op [`OpsReport`]; [`to_indexed`] folds a [`Processed`]
//! picture onto a session palette through [`quantize_dithered`] and
//! measures it with [`quantize_stats`]. The fetch and the palette read
//! happen in the calling tool: nothing here touches a session or a file,
//! so the whole helper is unit-testable.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assets::fetch::derive_lump_name;
use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::{decode_image, encode_png};
use crate::gfx::image::{IndexedImage, RgbaImage};
use crate::gfx::ops::dither::{Dither, quantize_dithered};
use crate::gfx::ops::{ImageOp, OpsReport, apply_ops};
use crate::gfx::palette::Palette;
use crate::gfx::quantize::{
    DEFAULT_ALPHA_THRESHOLD, QuantizeOptions, Transparency, nearest_index, quantize_stats,
};
use crate::gfx::{BYTES_PER_PIXEL, to_pixel};
use crate::state::AppState;
use crate::tools::fail;
use crate::tools::imports::image_pipeline::{
    DEFAULT_PALETTE, TransparencyParams, parse_transparency,
};
use crate::tools::ok;
use crate::tools::palette::load_palette;

use rmcp::ErrorData;
use rmcp::model::CallToolResult;

/// Media type of the published preview pictures.
const PNG_MIME: &str = "image/png";
/// Asset-name fallback when the fetched file names nothing usable.
const FALLBACK_STEM: &str = "IMAGE";

/// The dither mode every tool that quantizes settles on when its `dither`
/// argument is unset: serpentine Floyd-Steinberg error diffusion.
pub(in crate::tools) const DEFAULT_DITHER: Dither = Dither::FloydSteinberg;

/// The effective mode of one `dither` argument: the given one or
/// [`DEFAULT_DITHER`], which is also [`ProcessParams::dither_choice`].
#[must_use]
pub(in crate::tools) const fn dither_or_default(value: Option<Dither>) -> Dither {
    match value {
        Some(mode) => mode,
        None => DEFAULT_DITHER,
    }
}

/// The full documentation of [`crate::server::Cyberdemon::process_image`].
pub(in crate::tools) const PROCESS_TOOL_DESCRIPTION: &str = "Previews the image pipeline without touching a WAD: decodes source, runs your ordered ops (crop, fit, rotate, flip, auto_levels, adjust, color_key, trim, remove_background, make_tileable, threshold) and publishes the processed PNG plus a PLAYPAL-quantized preview with colors_used and max_color_error measured. Pass the returned URL as source to any import tool; see skill/reference/imports.md.";

/// The shared processing parameters, flattened into the arguments of every
/// tool that runs [`process`] before it stores or publishes a picture.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct ProcessParams {
    /// Image operations applied in order (see `skill/reference/imports.md`
    /// for the op catalogue). Default: none.
    #[serde(default)]
    pub(in crate::tools) ops: Vec<ImageOp>,
    /// Palette dithering for quantization: `"none"`, `"floyd_steinberg"`
    /// or `"ordered"`. Default `"floyd_steinberg"`.
    #[serde(default)]
    pub(in crate::tools) dither: Option<Dither>,
    /// Which `PLAYPAL` entry to quantize against (default 0). Past the
    /// entries the lump holds is `not_found`, as is a session with no
    /// `PLAYPAL`.
    #[serde(default)]
    pub(in crate::tools) palette: Option<usize>,
}

impl ProcessParams {
    /// The dither mode this call settles on: [`DEFAULT_DITHER`] when the
    /// `dither` argument is unset.
    #[must_use]
    pub(in crate::tools) const fn dither_choice(&self) -> Dither {
        dither_or_default(self.dither)
    }

    /// The `PLAYPAL` entry index this call settles on.
    #[must_use]
    pub(in crate::tools) const fn palette_index(&self) -> usize {
        match self.palette {
            Some(index) => index,
            None => DEFAULT_PALETTE,
        }
    }
}

/// A fetched picture with the operations already run over it.
#[derive(Debug)]
pub(in crate::tools) struct Processed {
    /// The picture after `ops`.
    pub(in crate::tools) rgba: RgbaImage,
    /// One measurement object per applied op, in order.
    pub(in crate::tools) report: OpsReport,
}

/// The indexed quantization of a [`Processed`] picture with its measures.
#[derive(Debug)]
pub(in crate::tools) struct Indexed {
    /// The palette indices and opacity flags.
    pub(in crate::tools) image: IndexedImage,
    /// Distinct opaque source colours.
    pub(in crate::tools) colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    pub(in crate::tools) max_color_error: i32,
    /// Pixels the transparency rule dropped.
    pub(in crate::tools) transparent_pixels: usize,
}

/// Decode `bytes` and run `params.ops` over the picture.
///
/// # Errors
///
/// [`decode_image`]' [`CyberError::Format`] and [`CyberError::Unsupported`]
/// for bytes that are no readable image, and [`apply_ops`]'
/// [`CyberError::InvalidArgument`] naming the failing op index.
pub(in crate::tools) fn process(
    bytes: &[u8],
    params: &ProcessParams,
) -> Result<Processed, CyberError> {
    let decoded = decode_image(bytes)?;
    let (rgba, report) = apply_ops(decoded, &params.ops)?;
    Ok(Processed { rgba, report })
}

/// Quantize `processed` to `palette` under `transparency` and `dither`,
/// measuring the result.
#[must_use]
pub(in crate::tools) fn to_indexed(
    processed: &Processed,
    palette: &Palette,
    transparency: Transparency,
    dither: Dither,
) -> Indexed {
    let options = QuantizeOptions {
        transparency,
        reserved: Vec::new(),
    };
    let image = quantize_dithered(&processed.rgba, palette, &options, dither);
    let (colors_used, max_color_error) = quantize_stats(&processed.rgba, palette, &options);
    let transparent_pixels = image.opaque.iter().filter(|visible| !**visible).count();
    Indexed {
        image,
        colors_used,
        max_color_error,
        transparent_pixels,
    }
}

/// Repaint every source pixel whose alpha is below
/// [`DEFAULT_ALPHA_THRESHOLD`] with the palette colour nearest to black and
/// mark it opaque, returning how many pixels were repainted.
///
/// Wall textures are opaque: this is the create-side companion of
/// [`to_indexed`] for a picture that carries (semi-)transparent pixels
/// although the wall must not.
pub(in crate::tools) fn fill_low_alpha(
    indexed: &mut IndexedImage,
    source: &RgbaImage,
    palette: &Palette,
) -> usize {
    let black = nearest_index([0_u8, 0, 0], palette, &[]);
    let mut repainted = 0_usize;
    for ((slot, visible), chunk) in indexed
        .indices
        .iter_mut()
        .zip(indexed.opaque.iter_mut())
        .zip(source.pixels.chunks_exact(BYTES_PER_PIXEL))
    {
        let [_, _, _, alpha] = to_pixel(chunk);
        if alpha >= DEFAULT_ALPHA_THRESHOLD {
            continue;
        }
        *slot = black;
        *visible = true;
        repainted = repainted.saturating_add(1);
    }
    repainted
}

/// Arguments for [`crate::server::Cyberdemon::process_image`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct ProcessImageParams {
    /// Picture to process: absolute path, `file://` URI, `http(s)://` URL
    /// or this server's asset/upload URL (a previously published asset
    /// `URL` is a valid `source`, so one call can feed the next).
    pub(in crate::tools) source: String,
    /// Which source pixels are transparent in the quantized preview (the
    /// same object the import pipeline takes, default `{mode: "alpha",
    /// threshold: 128}`; see `skill/reference/imports.md`).
    #[serde(default)]
    pub(in crate::tools) transparency: Option<TransparencyParams>,
    /// Also quantize the picture against the session palette and publish
    /// the result (default `true`).
    #[serde(default)]
    pub(in crate::tools) quantize: Option<bool>,
    /// The shared processing parameters: `ops`, `dither` and `palette`
    /// (see `skill/reference/imports.md`).
    #[serde(flatten)]
    pub(in crate::tools) shared: ProcessParams,
}

/// Structured result of [`crate::server::Cyberdemon::process_image`].
#[derive(Serialize)]
pub(in crate::tools) struct ProcessReport {
    /// Width of both published pictures, in pixels.
    width: u32,
    /// Height of both published pictures, in pixels.
    height: u32,
    /// One measurement object per applied op, in order.
    steps: Vec<Value>,
    /// The `PNG` of the picture after `ops` (full `RGBA`).
    processed: AssetRef,
    /// The `PNG` of the palette-quantized picture rendered back through
    /// the palette, transparency as alpha; `null` with `quantize: false`.
    quantized: Option<AssetRef>,
    /// Distinct opaque source colours; `null` without quantization.
    colors_used: Option<usize>,
    /// Worst squared `RGB` distance to the palette; `null` without
    /// quantization.
    max_color_error: Option<i32>,
    /// Pixels the transparency rule dropped from the quantized preview;
    /// `null` without quantization.
    transparent_pixels: Option<usize>,
}

/// `process_image`'s work: fetch, process, publish the previews. The
/// working archive is never touched.
///
/// # Errors
///
/// [`AppState::fetch_source`]' errors for the `source`, [`process`]' errors
/// for the bytes and the ops, the transparency parse's
/// [`CyberError::InvalidArgument`], [`load_palette`]'
/// [`CyberError::NotFound`] for the palette and [`AppState::publish`]'
/// errors for the store.
pub(in crate::tools) async fn process_image_run(
    state: &AppState,
    args: &ProcessImageParams,
) -> Result<CallToolResult, ErrorData> {
    match process_image_build(state, args).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

/// The measured body of [`process_image_run`].
async fn process_image_build(
    state: &AppState,
    args: &ProcessImageParams,
) -> Result<ProcessReport, CyberError> {
    let transparency = parse_transparency(args.transparency.as_ref())?;
    let fetched = state.fetch_source(&args.source).await?;
    let processed = process(&fetched.bytes, &args.shared)?;
    let stem = preview_stem(&fetched.filename);
    let (width, height) = (processed.rgba.width, processed.rgba.height);
    let steps = processed.report.steps.clone();
    let png = encode_png(&processed.rgba, None)?;
    let published = state.publish(&format!("{stem}-processed.png"), PNG_MIME, png)?;
    if !args.quantize.unwrap_or(true) {
        return Ok(ProcessReport {
            width,
            height,
            steps,
            processed: published,
            quantized: None,
            colors_used: None,
            max_color_error: None,
            transparent_pixels: None,
        });
    }
    let palette =
        state.with_session(|session| load_palette(session, args.shared.palette_index()))?;
    let indexed = to_indexed(
        &processed,
        &palette,
        transparency,
        args.shared.dither_choice(),
    );
    let rgba = indexed.image.to_rgba(&palette);
    let quantized_png = encode_png(&rgba, None)?;
    let quantized = state.publish(&format!("{stem}-quantized.png"), PNG_MIME, quantized_png)?;
    Ok(ProcessReport {
        width,
        height,
        steps,
        processed: published,
        quantized: Some(quantized),
        colors_used: Some(indexed.colors_used),
        max_color_error: Some(indexed.max_color_error),
        transparent_pixels: Some(indexed.transparent_pixels),
    })
}

/// The asset-name stem: the processed file's derived lump name, uppercased.
fn preview_stem(filename: &str) -> String {
    derive_lump_name(filename).map_or_else(|| FALLBACK_STEM.to_owned(), |name| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::{ProcessParams, fill_low_alpha, preview_stem, process, to_indexed};
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::ops::dither::Dither;
    use crate::gfx::ops::{ImageOp, apply_ops};
    use crate::gfx::palette::Palette;
    use crate::gfx::quantize::Transparency;

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A palette whose colour `i` is `(i, 255 - i, 0)`.
    fn ramp() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        for (index, color) in colors.iter_mut().enumerate() {
            let slot = u8::try_from(index).expect("palette indices fit u8");
            *color = [slot, 255_u8.wrapping_sub(slot), 0_u8];
        }
        Palette::from(colors)
    }

    /// A `width` x `height` opaque mid-grey test picture.
    fn grey(width: u32, height: u32) -> RgbaImage {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let pixels: Vec<u8> = (0..count).flat_map(|_| [90_u8, 90, 90, 255]).collect();
        RgbaImage::new(width, height, pixels).expect("test shape is valid")
    }

    /// Parameters running one op list.
    fn params(ops: Vec<ImageOp>) -> ProcessParams {
        ProcessParams {
            ops,
            dither: None,
            palette: None,
        }
    }

    #[test]
    fn process_defaults_are_documented() {
        let fresh = ProcessParams {
            ops: Vec::new(),
            dither: None,
            palette: None,
        };
        assert_eq!(
            fresh.dither_choice(),
            Dither::FloydSteinberg,
            "the default dither"
        );
        assert_eq!(fresh.palette_index(), 0_usize, "the normal palette");
        let keyed = ProcessParams {
            ops: Vec::new(),
            dither: Some(Dither::Ordered),
            palette: Some(3_usize),
        };
        assert_eq!(keyed.dither_choice(), Dither::Ordered, "explicit dither");
        assert_eq!(keyed.palette_index(), 3_usize, "explicit palette");
    }

    #[tokio::test]
    async fn process_decodes_and_runs_the_ops_in_order() {
        let png = encode_png(&grey(8_u32, 8_u32), None).expect("test PNG encodes");
        let ops = vec![
            ImageOp::Crop {
                x: 1,
                y: 1,
                width: 6,
                height: 6,
            },
            ImageOp::Fit {
                width: 4,
                height: 4,
                mode: crate::gfx::ops::FitMode::Stretch,
                filter: crate::gfx::ops::Filter::Nearest,
                background: None,
            },
        ];
        let processed = process(&png, &params(ops)).expect("both ops are valid");
        assert_eq!(
            (processed.rgba.width, processed.rgba.height),
            (4_u32, 4_u32)
        );
        assert_eq!(processed.report.steps.len(), 2, "one report entry per op");
    }

    #[test]
    fn a_bad_op_names_its_index_through_process() {
        let png = encode_png(&grey(4_u32, 4_u32), None).expect("test PNG encodes");
        let ops = vec![ImageOp::Crop {
            x: 0,
            y: 0,
            width: 99,
            height: 1,
        }];
        let error = process(&png, &params(ops)).expect_err("the crop leaves the picture");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("ops[0]"),
            "the message names the op index: {error}"
        );
    }

    /// A 4x4 opaque picture of the ramp's exact colour 40.
    fn ramp_colour() -> RgbaImage {
        let pixels: Vec<u8> = (0..16_usize).flat_map(|_| [40_u8, 215, 0, 255]).collect();
        RgbaImage::new(4_u32, 4_u32, pixels).expect("test shape is valid")
    }

    #[test]
    fn to_indexed_measures_colors_and_transparency() {
        let exact = ramp_colour();
        let processed = super::Processed {
            rgba: exact.clone(),
            report: apply_ops(exact, &[]).expect("no ops").1,
        };
        let indexed = to_indexed(&processed, &ramp(), Transparency::None, Dither::None);
        assert_eq!(indexed.colors_used, 1_usize, "one distinct colour");
        assert_eq!(indexed.transparent_pixels, 0_usize, "nothing dropped");
        assert!(
            indexed.image.indices.iter().all(|value| *value == 40_u8),
            "the exact palette colour keeps its index"
        );
        assert_eq!(
            indexed.max_color_error, 0_i32,
            "an exact source has no error"
        );
        let grey_processed = super::Processed {
            rgba: grey(4_u32, 4_u32),
            report: apply_ops(grey(4_u32, 4_u32), &[]).expect("no ops").1,
        };
        let off = to_indexed(&grey_processed, &ramp(), Transparency::None, Dither::None);
        assert!(
            off.max_color_error > 0_i32,
            "an off-palette grey accrues error"
        );
        let dropped = to_indexed(
            &grey_processed,
            &ramp(),
            Transparency::ColorKey {
                rgb: [90_u8, 90, 90],
            },
            Dither::None,
        );
        assert_eq!(
            dropped.transparent_pixels, 16_usize,
            "the key colour drops all"
        );
        assert_eq!(dropped.colors_used, 0_usize, "nothing opaque is measured");
    }

    #[test]
    fn fill_low_alpha_repaints_only_the_faint_pixels() {
        let pixels = vec![
            200_u8, 30, 20, 255, // opaque: untouched
            10, 200, 30, 0, // dropped alpha: repainted
            40, 40, 200, 127, // below the threshold: repainted
            50, 50, 50, 128, // at the threshold: untouched
        ];
        let source = RgbaImage::new(4_u32, 1_u32, pixels).expect("4x1 is valid");
        let palette = ramp();
        let processed = super::Processed {
            rgba: source.clone(),
            report: apply_ops(source.clone(), &[]).expect("no ops").1,
        };
        let mut indexed = to_indexed(&processed, &palette, Transparency::None, Dither::None);
        let before = indexed.image.indices.clone();
        let repainted = fill_low_alpha(&mut indexed.image, &source, &palette);
        assert_eq!(repainted, 2_usize, "alpha 0 and 127 fall below 128");
        let black = crate::gfx::quantize::nearest_index([0_u8, 0, 0], &palette, &[]);
        assert_eq!(
            indexed.image.indices.get(1).copied(),
            Some(black),
            "the dropped pixel took black"
        );
        assert_eq!(
            indexed.image.indices.get(2).copied(),
            Some(black),
            "the faint pixel took black"
        );
        assert_eq!(
            (
                indexed.image.indices.first(),
                indexed.image.indices.get(3_usize),
            ),
            (before.first(), before.get(3_usize)),
            "opaque pixels keep their indices"
        );
        assert!(
            indexed.image.opaque.iter().all(|visible| *visible),
            "the whole picture is opaque"
        );
    }

    #[test]
    fn fill_low_alpha_touches_nothing_in_an_opaque_picture() {
        let source = grey(2_u32, 2_u32);
        let palette = ramp();
        let processed = super::Processed {
            rgba: source.clone(),
            report: apply_ops(source.clone(), &[]).expect("no ops").1,
        };
        let mut indexed = to_indexed(&processed, &palette, Transparency::None, Dither::None);
        let before = indexed.image.indices.clone();
        assert_eq!(
            fill_low_alpha(&mut indexed.image, &source, &palette),
            0_usize,
            "an opaque picture repaints nothing"
        );
        assert_eq!(indexed.image.indices, before, "the indices stand");
    }

    #[test]
    fn stems_name_the_assets_or_fall_back() {
        assert_eq!(preview_stem("wall.png"), "WALL");
        assert_eq!(preview_stem("###.png"), "IMAGE", "nothing derivable");
    }
}
