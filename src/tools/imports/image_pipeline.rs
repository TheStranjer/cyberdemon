//! The shared image-import pipeline: fetched bytes in, picture lump out.
//!
//! Every patch import flattens [`ImageImportParams`] into its arguments,
//! validates it with [`parse_import`] and runs [`build_patch`]: fetch the
//! `source` (never holding the session lock), [`decode_image`] it, quantize
//! against the session's `PLAYPAL` entry under the call's `transparency`
//! rule, resolve the drawing `offsets` (a `PNG`'s `grAb` chunk, a preset or
//! an explicit pair) and [`encode_patch`] the result. The quantization
//! happens once: [`build_patch`] folds the indexed image and the reported
//! colour measures off [`quantize_measured`]' single walk and hands that
//! one image to the encoder. The flat importer reaches around
//! [`ImageImportParams`] for the same pieces (`source` fetch,
//! [`decode_image`], quantization, [`chosen_name`]). Landing the finished
//! bytes is the group root's work: [`crate::tools::imports::put_lump`]
//! drops them into a namespace of the working archive through
//! [`crate::tools::lumps::place`], refusing a same-name twin unless
//! `replace` overwrites it in place. Everything here is visible to the
//! whole tools tree, so the patch lifecycle tools share the same pipeline
//! and [`chosen_name`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::fetch::derive_lump_name;
use crate::error::CyberError;
use crate::gfx::codec::{decode_image, is_png};
use crate::gfx::grab::{GrabOffsets, read_grab};
use crate::gfx::image::RgbaImage;
use crate::gfx::ops::parse_hex_colour;
use crate::gfx::patch::{Patch, encode_patch};
use crate::gfx::quantize::{
    DEFAULT_ALPHA_THRESHOLD, QuantizeOptions, Transparency, quantize_measured,
};
use crate::state::AppState;
use crate::tools::palette::load_palette;
use crate::wad::name::LumpName;

/// Default `PLAYPAL` entry image imports quantize against.
pub(in crate::tools) const DEFAULT_PALETTE: usize = 0_usize;
/// Default colour-key when `transparency.mode` is `"color"` without a
/// `color`: the cyan Doom sprite sources are keyed against.
const DEFAULT_KEY_COLOR: &str = "#00FFFF";
/// Default `transparency.mode` when the object (or the whole parameter) is
/// omitted.
const DEFAULT_TRANSPARENCY_MODE: &str = "alpha";

/// The image-import parameters shared with the patch lifecycle tools,
/// flattened into each tool's arguments.
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct ImageImportParams {
    /// File to import: absolute path, `file://` URI, `http(s)://` URL or
    /// this server's asset/upload URL.
    pub(in crate::tools) source: String,
    /// Lump name to store under: 1 to 8 printable `ASCII` characters,
    /// uppercased. Defaults to the fetched file name's stem uppercased and
    /// trimmed to 8 characters; when nothing usable remains the call fails
    /// asking for this name.
    #[serde(default)]
    pub(in crate::tools) name: Option<String>,
    /// Which `PLAYPAL` entry the picture is quantized against (default 0:
    /// the normal palette). Past the entries the lump holds is `not_found`,
    /// as is a session with no `PLAYPAL`.
    #[serde(default)]
    pub(in crate::tools) palette: Option<usize>,
    /// Which source pixels become transparent (see
    /// `skill/reference/imports.md`); default `{mode: "alpha", threshold:
    /// 128}`.
    #[serde(default)]
    pub(in crate::tools) transparency: Option<TransparencyParams>,
    /// Drawing offsets: `"zero"`, `"grab"`, `"center"`, `"center_bottom"`
    /// or an explicit `{"x": n, "y": n}` pair. Omitted means `"grab"` when
    /// the `PNG` carries a `grAb` chunk, else the tool's own default.
    /// `center` is `(width/2, height/2)`, `center_bottom` is
    /// `(width/2, height)` in the `Doom` convention: a positive left offset
    /// shifts the image left and the top offset is the distance from the
    /// image's top edge to the origin.
    #[serde(default)]
    pub(in crate::tools) offsets: Option<OffsetsParam>,
    /// Overwrite a same-name lump of the working archive in place (default
    /// `false`; without it such a lump is `already_exists`).
    #[serde(default)]
    pub(in crate::tools) replace: bool,
}

/// The `transparency` object of [`ImageImportParams`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct TransparencyParams {
    /// Which rule drops pixels: `"alpha"` (default, alpha below
    /// `threshold`), `"color"` (the pixel equals `color` exactly) or
    /// `"none"` (everything stays opaque).
    #[serde(default)]
    pub(in crate::tools) mode: Option<String>,
    /// Exclusive alpha cutoff for `"alpha"` mode (default 128).
    #[serde(default)]
    pub(in crate::tools) threshold: Option<u8>,
    /// The key colour for `"color"` mode, as `#RRGGBB` or `#RRGGBBAA`
    /// (default `"#00FFFF"`, `Doom` cyan).
    #[serde(default)]
    pub(in crate::tools) color: Option<String>,
}

/// The `offsets` wire form: a preset word or an explicit pair.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub(in crate::tools) enum OffsetsParam {
    /// One of `"zero"`, `"grab"`, `"center"` or `"center_bottom"`.
    Preset(
        /// The preset name, matched exactly.
        String,
    ),
    /// A literal drawing offset pair.
    Point {
        /// Left (horizontal) offset in pixels.
        x: i32,
        /// Top (vertical) offset in pixels.
        y: i32,
    },
}

/// A validated `offsets` choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tools) enum OffsetMode {
    /// `(0, 0)`.
    Zero,
    /// A `PNG`'s `grAb` chunk, falling back to the tool's default when the
    /// picture carries none.
    Grab,
    /// `(width/2, height/2)`.
    Center,
    /// `(width/2, height)`.
    CenterBottom,
    /// A literal pair.
    Explicit {
        /// Left offset in pixels.
        left: i32,
        /// Top offset in pixels.
        top: i32,
    },
}

/// An import tool's own offset default, used when no `grAb` chunk answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tools) enum OffsetDefault {
    /// `(0, 0)`: what wall patches are stored with.
    Zero,
    /// `(width/2, height/2)`.
    Center,
    /// `(width/2, height)`: what floor-standing sprites are drawn with.
    CenterBottom,
}

/// The validated shared parameters of one image import.
#[derive(Debug)]
pub(in crate::tools) struct ParsedImport {
    /// The `source` to fetch.
    pub(in crate::tools) source: String,
    /// The explicit lump name, `None` when it is derived from the file.
    pub(in crate::tools) name: Option<LumpName>,
    /// `PLAYPAL` entry to quantize against.
    pub(in crate::tools) palette: usize,
    /// Which pixels count as transparent.
    pub(in crate::tools) transparency: Transparency,
    /// The requested offsets, `None` for the grab-or-default rule.
    pub(in crate::tools) offsets: Option<OffsetMode>,
    /// Whether a same-name working lump may be overwritten.
    pub(in crate::tools) replace: bool,
}

/// Validate the flattened shared parameters.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name that does not parse, an
/// unknown `transparency.mode`, a bad `color` hex or an unknown `offsets`
/// preset.
pub(in crate::tools) fn parse_import(
    params: &ImageImportParams,
) -> Result<ParsedImport, CyberError> {
    Ok(ParsedImport {
        source: params.source.clone(),
        name: params.name.as_deref().map(LumpName::parse).transpose()?,
        palette: params.palette.unwrap_or(DEFAULT_PALETTE),
        transparency: parse_transparency(params.transparency.as_ref())?,
        offsets: params.offsets.clone().map(parse_offset_mode).transpose()?,
        replace: params.replace,
    })
}

/// The validated `transparency` object, or the default rule.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an unknown mode or a malformed
/// `color`; fields irrelevant to the mode are ignored.
pub(in crate::tools) fn parse_transparency(
    value: Option<&TransparencyParams>,
) -> Result<Transparency, CyberError> {
    let Some(params) = value else {
        return Ok(Transparency::default());
    };
    let mode = params.mode.as_deref().unwrap_or(DEFAULT_TRANSPARENCY_MODE);
    match mode {
        "alpha" => Ok(Transparency::Alpha {
            threshold: params.threshold.unwrap_or(DEFAULT_ALPHA_THRESHOLD),
        }),
        "color" => Ok(Transparency::ColorKey {
            rgb: key_color(params.color.as_deref())?,
        }),
        "none" => Ok(Transparency::None),
        other => Err(CyberError::InvalidArgument(format!(
            "transparency mode {other:?} is unknown: pass \"alpha\", \"color\" or \"none\""
        ))),
    }
}

/// The colour-key as an `RGB` triple, defaulting to [`DEFAULT_KEY_COLOR`].
///
/// # Errors
///
/// [`parse_hex_colour`]' [`CyberError::InvalidArgument`] for anything that
/// is not a hex colour.
fn key_color(text: Option<&str>) -> Result<[u8; 3], CyberError> {
    let [red, green, blue, _] = parse_hex_colour(text.unwrap_or(DEFAULT_KEY_COLOR), true)?;
    Ok([red, green, blue])
}

/// The validated `offsets` parameter.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a preset outside the four names.
pub(in crate::tools) fn parse_offset_mode(value: OffsetsParam) -> Result<OffsetMode, CyberError> {
    match value {
        OffsetsParam::Preset(text) => parse_offset_preset(&text),
        OffsetsParam::Point { x, y } => Ok(OffsetMode::Explicit { left: x, top: y }),
    }
}

/// One validated preset word.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
fn parse_offset_preset(text: &str) -> Result<OffsetMode, CyberError> {
    match text {
        "zero" => Ok(OffsetMode::Zero),
        "grab" => Ok(OffsetMode::Grab),
        "center" => Ok(OffsetMode::Center),
        "center_bottom" => Ok(OffsetMode::CenterBottom),
        other => Err(CyberError::InvalidArgument(format!(
            "offsets {other:?} is unknown: pass \"zero\", \"grab\", \"center\", \
             \"center_bottom\" or an explicit {{\"x\": n, \"y\": n}} pair"
        ))),
    }
}

/// The shared report one finished import answers with, flattened into each
/// tool's result.
#[derive(Debug, Serialize)]
pub(in crate::tools) struct ImportReport {
    /// Patch width in pixels.
    pub(in crate::tools) width: u32,
    /// Patch height in pixels.
    pub(in crate::tools) height: u32,
    /// Horizontal drawing offset as stored.
    pub(in crate::tools) left_offset: i32,
    /// Vertical drawing offset as stored.
    pub(in crate::tools) top_offset: i32,
    /// Pixels the quantization dropped.
    pub(in crate::tools) transparent_pixels: usize,
    /// Distinct colours among the opaque pixels.
    pub(in crate::tools) colors_used: usize,
    /// Worst squared `RGB` distance from an opaque colour to its palette
    /// entry.
    pub(in crate::tools) max_color_error: i32,
}

/// An encoded patch ready for insertion.
#[derive(Debug)]
pub(in crate::tools) struct Patched {
    /// The lump name: the explicit one or the derived one.
    pub(in crate::tools) name: LumpName,
    /// The `Doom` patch bytes to store.
    pub(in crate::tools) data: Vec<u8>,
    /// The shared report fields.
    pub(in crate::tools) report: ImportReport,
}

/// Run the shared pipeline: fetch, decode, quantize, offset and encode.
///
/// The `source` is fetched before any lock is taken; the `PLAYPAL` read is
/// the only session access here. One [`quantize_measured`] pass stores the
/// indexed image the patch encodes and measures the colour facts the report
/// carries, so the picture is never quantized twice for one import.
///
/// # Errors
///
/// [`AppState::fetch_source`]' errors for the `source`,
/// [`decode_image`]' [`CyberError::Format`] and [`CyberError::Unsupported`]
/// for bytes that are no readable image,
/// [`CyberError::InvalidArgument`] for an underivable name or an offset
/// outside the `i16` range a patch header stores, [`load_palette`]'
/// [`CyberError::NotFound`] for the palette and [`encode_patch`]' errors
/// for an image past the patch codec's limits.
pub(in crate::tools) async fn build_patch(
    state: &AppState,
    import: &ParsedImport,
    default: OffsetDefault,
) -> Result<Patched, CyberError> {
    let fetched = state.fetch_source(&import.source).await?;
    let name = chosen_name(import.name, &fetched.filename)?;
    let image = decode_image(&fetched.bytes)?;
    let pair = resolve_offsets(
        import.offsets.as_ref(),
        grab_of(&fetched.bytes)?,
        default,
        &image,
    );
    let (left_offset, top_offset) = to_i16_offsets(pair)?;
    let options = QuantizeOptions {
        transparency: import.transparency,
        reserved: Vec::new(),
    };
    let palette = state.with_session(|session| load_palette(session, import.palette))?;
    let measured = quantize_measured(&image, &palette, &options);
    let patch = Patch {
        left_offset,
        top_offset,
        image: measured.image,
    };
    let data = encode_patch(&patch)?;
    Ok(Patched {
        name,
        data,
        report: ImportReport {
            width: image.width,
            height: image.height,
            left_offset: i32::from(left_offset),
            top_offset: i32::from(top_offset),
            transparent_pixels: transparent_pixels(&patch),
            colors_used: measured.colors_used,
            max_color_error: measured.max_color_error,
        },
    })
}

/// The name to import under: the explicit one, else one derived from the
/// fetched file name.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] asking for `name` when nothing usable
/// can be derived.
pub(in crate::tools) fn chosen_name(
    explicit: Option<LumpName>,
    filename: &str,
) -> Result<LumpName, CyberError> {
    explicit
        .or_else(|| derive_lump_name(filename))
        .ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "cannot derive a lump name from {filename:?}: pass name (1 to 8 printable ASCII \
             characters)"
            ))
        })
}

/// The `grAb` offsets of `PNG` bytes, `None` for any other format or no
/// chunk.
///
/// # Errors
///
/// [`read_grab`]' [`CyberError::Format`] for a chunk stream that does not
/// parse.
fn grab_of(bytes: &[u8]) -> Result<Option<GrabOffsets>, CyberError> {
    if is_png(bytes) {
        return read_grab(bytes);
    }
    Ok(None)
}

/// The drawing offsets the parameters ask for, before the `i16` check.
pub(in crate::tools) fn resolve_offsets(
    mode: Option<&OffsetMode>,
    grab: Option<GrabOffsets>,
    default: OffsetDefault,
    image: &RgbaImage,
) -> GrabOffsets {
    match mode.copied() {
        None | Some(OffsetMode::Grab) => grab.unwrap_or_else(|| preset_offsets(default, image)),
        Some(OffsetMode::Zero) => (0_i32, 0_i32),
        Some(OffsetMode::Center) => preset_offsets(OffsetDefault::Center, image),
        Some(OffsetMode::CenterBottom) => preset_offsets(OffsetDefault::CenterBottom, image),
        Some(OffsetMode::Explicit { left, top }) => (left, top),
    }
}

/// One offset preset for a picture's shape.
fn preset_offsets(preset: OffsetDefault, image: &RgbaImage) -> GrabOffsets {
    let half_width = half(image.width);
    match preset {
        OffsetDefault::Zero => (0_i32, 0_i32),
        OffsetDefault::Center => (half_width, half(image.height)),
        OffsetDefault::CenterBottom => (half_width, as_offset(image.height)),
    }
}

/// Half a dimension as an offset, saturating past the `i32` range.
fn half(value: u32) -> i32 {
    value.checked_div(2).map_or(i32::MAX, as_offset)
}

/// A dimension as an offset, saturating past the `i32` range.
fn as_offset(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Both offsets as the `i16` pair a patch header stores.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the field out of range.
pub(in crate::tools) fn to_i16_offsets(pair: GrabOffsets) -> Result<(i16, i16), CyberError> {
    let (left, top) = pair;
    Ok((
        fit_offset("left_offset", left)?,
        fit_offset("top_offset", top)?,
    ))
}

/// One offset narrowed to `i16`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the field and the stored range.
fn fit_offset(field: &str, value: i32) -> Result<i16, CyberError> {
    i16::try_from(value).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "{field} {value} does not fit the -32768..=32767 range a patch header stores: \
             {error}; pass a smaller offset"
        ))
    })
}

/// Pixels the quantized patch leaves transparent.
fn transparent_pixels(patch: &Patch) -> usize {
    patch
        .image
        .opaque
        .iter()
        .filter(|visible| !**visible)
        .count()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        ImageImportParams, OffsetDefault, OffsetMode, OffsetsParam, ParsedImport,
        TransparencyParams, build_patch, chosen_name, grab_of, parse_import, parse_offset_preset,
        preset_offsets, resolve_offsets, to_i16_offsets, transparent_pixels,
    };
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::grab::insert_grab;
    use crate::gfx::image::{IndexedImage, RgbaImage};
    use crate::gfx::patch::Patch;
    use crate::gfx::quantize::Transparency;
    use crate::state::AppState;
    use crate::wad::name::LumpName;

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// Shared parameters over `source` with everything else unset.
    fn params(source: &str) -> ImageImportParams {
        ImageImportParams {
            source: source.to_owned(),
            name: None,
            palette: None,
            transparency: None,
            offsets: None,
            replace: false,
        }
    }

    /// A `width` by `height` test image.
    fn image(width: u32, height: u32) -> RgbaImage {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        RgbaImage::new(
            width,
            height,
            vec![0_u8; count.checked_mul(4).expect("test bytes fit")],
        )
        .expect("test shape is valid")
    }

    /// A parsed offsets preset word.
    fn preset(text: &str) -> OffsetMode {
        parse_offset_preset(text).expect("test presets are valid")
    }

    /// Resolve with no explicit choice and no chunk, per tool default.
    fn defaulted(default: OffsetDefault, image: &RgbaImage) -> (i32, i32) {
        resolve_offsets(None, None, default, image)
    }

    #[test]
    fn parse_import_applies_the_documented_defaults() {
        let import = parse_import(&params("/x/y.png")).expect("a source is enough");
        assert_eq!(import.source, "/x/y.png", "the source carries");
        assert_eq!(import.name, None, "no name was given");
        assert_eq!(import.palette, 0_usize, "the normal palette by default");
        assert_eq!(
            import.transparency,
            Transparency::Alpha { threshold: 128_u8 },
            "alpha at the default cutoff"
        );
        assert_eq!(import.offsets, None, "the grab-or-tool-default rule");
        assert!(!import.replace, "no silent overwrite");
    }

    #[test]
    fn parse_import_validates_name_offsets_and_transparency() {
        let mut args = params("/x/y.png");
        args.name = Some("too long for wad".to_owned());
        assert_eq!(
            code(&parse_import(&args).expect_err("bad name")),
            "invalid_argument"
        );
        args.name = Some("ART".to_owned());
        assert_eq!(parse_import(&args).expect("valid").name, Some(name("ART")));
        args.offsets = Some(OffsetsParam::Preset("middle".to_owned()));
        assert_eq!(
            code(&parse_import(&args).expect_err("not a preset")),
            "invalid_argument"
        );
        args.offsets = Some(OffsetsParam::Point {
            x: -3_i32,
            y: 9_i32,
        });
        assert_eq!(
            parse_import(&args).expect("a pair parses").offsets,
            Some(OffsetMode::Explicit {
                left: -3_i32,
                top: 9_i32
            })
        );
        args.transparency = Some(TransparencyParams {
            mode: Some("magic".to_owned()),
            threshold: None,
            color: None,
        });
        assert_eq!(
            code(&parse_import(&args).expect_err("not a mode")),
            "invalid_argument"
        );
    }

    #[test]
    fn transparency_modes_take_their_defaults_and_options() {
        let keyed = TransparencyParams {
            mode: Some("color".to_owned()),
            threshold: Some(7_u8),
            color: None,
        };
        assert_eq!(
            parse_import_with_transparency(&keyed).transparency,
            Transparency::ColorKey {
                rgb: [0_u8, 255, 255]
            },
            "the default cyan; a threshold is irrelevant here"
        );
        let custom = TransparencyParams {
            mode: Some("color".to_owned()),
            threshold: None,
            color: Some("#102030".to_owned()),
        };
        assert_eq!(
            parse_import_with_transparency(&custom).transparency,
            Transparency::ColorKey {
                rgb: [0x10_u8, 0x20, 0x30]
            }
        );
        let none = TransparencyParams {
            mode: Some("none".to_owned()),
            threshold: None,
            color: Some("nope".to_owned()),
        };
        assert_eq!(
            parse_import_with_transparency(&none).transparency,
            Transparency::None,
            "an irrelevant bad colour is never parsed"
        );
        let cutoff = TransparencyParams {
            mode: Some("alpha".to_owned()),
            threshold: Some(10_u8),
            color: None,
        };
        assert_eq!(
            parse_import_with_transparency(&cutoff).transparency,
            Transparency::Alpha { threshold: 10_u8 }
        );
    }

    /// Parse shared parameters carrying one transparency object.
    fn parse_import_with_transparency(transparency: &TransparencyParams) -> ParsedImport {
        let mut args = params("/x/y.png");
        args.transparency = Some(TransparencyParams {
            mode: transparency.mode.clone(),
            threshold: transparency.threshold,
            color: transparency.color.clone(),
        });
        parse_import(&args).expect("test transparency is valid")
    }

    #[test]
    fn offset_presets_parse_and_name_the_rejections() {
        assert_eq!(preset("zero"), OffsetMode::Zero);
        assert_eq!(preset("grab"), OffsetMode::Grab);
        assert_eq!(preset("center"), OffsetMode::Center);
        assert_eq!(preset("center_bottom"), OffsetMode::CenterBottom);
        let error = parse_offset_preset("Corner").expect_err("case-sensitive");
        assert_eq!(code(&error), "invalid_argument");
        assert_eq!(
            error.message(),
            "offsets \"Corner\" is unknown: pass \"zero\", \"grab\", \"center\", \
             \"center_bottom\" or an explicit {\"x\": n, \"y\": n} pair",
            "every preset is named in plain words"
        );
        assert!(
            !error.message().contains('\\'),
            "the message carries no escape leftovers: {error}"
        );
    }

    #[test]
    fn presets_measure_the_picture_the_way_the_docs_say() {
        let wide = image(9_u32, 5_u32);
        assert_eq!(
            defaulted(OffsetDefault::Zero, &wide),
            (0_i32, 0_i32),
            "zero"
        );
        assert_eq!(
            defaulted(OffsetDefault::Center, &wide),
            (4_i32, 2_i32),
            "halves floor"
        );
        assert_eq!(
            defaulted(OffsetDefault::CenterBottom, &wide),
            (4_i32, 5_i32),
            "bottom keeps the full height"
        );
        assert_eq!(
            preset_offsets(OffsetDefault::CenterBottom, &image(8_u32, 8_u32)),
            (4_i32, 8_i32)
        );
    }

    #[test]
    fn resolution_prefers_the_chunk_then_follows_the_choice() {
        let picture = image(8_u32, 8_u32);
        assert_eq!(
            resolve_offsets(None, Some((7_i32, -9_i32)), OffsetDefault::Zero, &picture),
            (7_i32, -9_i32),
            "an omitted offsets parameter takes grAb"
        );
        assert_eq!(
            resolve_offsets(
                Some(&OffsetMode::Grab),
                None,
                OffsetDefault::Center,
                &picture
            ),
            (4_i32, 4_i32),
            "grab without a chunk falls back to the tool default"
        );
        let explicit = OffsetMode::Explicit {
            left: 0x9C40_i32,
            top: 0_i32,
        };
        assert_eq!(
            resolve_offsets(
                Some(&explicit),
                Some((7_i32, -9_i32)),
                OffsetDefault::Zero,
                &picture
            ),
            (0x9C40_i32, 0_i32),
            "explicit beats the chunk"
        );
    }

    #[test]
    fn offsets_narrow_to_i16_or_say_which_one_did_not() {
        let edges = (i32::from(i16::MIN), i32::from(i16::MAX));
        assert_eq!(
            to_i16_offsets(edges).expect("at the edges"),
            (i16::MIN, i16::MAX)
        );
        let error = to_i16_offsets((0x9C40_i32, 0_i32)).expect_err("past i16");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("left_offset 40000"),
            "the message names the field: {error}"
        );
        let top = to_i16_offsets((0_i32, -0x9C40_i32)).expect_err("past i16");
        assert!(
            top.to_string().contains("top_offset -40000"),
            "the other field is named too: {top}"
        );
    }

    #[test]
    fn grab_offsets_only_come_from_pngs() {
        let plain = encode_png(&image(2_u32, 2_u32), None).expect("test PNG encodes");
        assert_eq!(
            grab_of(&plain).expect("a chunked PNG parses"),
            None,
            "without grAb there is nothing"
        );
        let chunked = insert_grab(&plain, 4_i32, -6_i32).expect("test grAb inserts");
        assert_eq!(
            grab_of(&chunked).expect("a grAb PNG parses"),
            Some((4_i32, -6_i32)),
            "the chunk survives"
        );
        assert_eq!(
            grab_of(b"not a png").expect("other bytes carry no chunk"),
            None,
            "only PNGs are scanned"
        );
    }

    #[test]
    fn transparent_count_reads_the_flags() {
        let patch = Patch {
            left_offset: 0_i16,
            top_offset: 0_i16,
            image: IndexedImage::new(2_u32, 1_u32, vec![0_u8, 1], vec![true, false])
                .expect("2x1 is a valid shape"),
        };
        assert_eq!(transparent_pixels(&patch), 1_usize, "one hole");
    }

    #[test]
    fn chosen_name_falls_back_to_the_file_stem() {
        assert_eq!(
            chosen_name(Some(name("GIVEN")), "art.png").expect("explicit"),
            name("GIVEN")
        );
        assert_eq!(chosen_name(None, "art.png").expect("derived"), name("ART"));
        let error = chosen_name(None, "###.png").expect_err("nothing usable");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("name"),
            "the fix names the parameter: {error}"
        );
    }

    #[tokio::test]
    async fn build_patch_reports_the_fetch_errors_before_touching_a_session() {
        let state = AppState::default();
        let import = parse_import(&params("/nowhere/art.png")).expect("parses");
        let error = build_patch(&state, &import, OffsetDefault::Zero)
            .await
            .expect_err("no such file");
        assert_eq!(code(&error), "not_found", "the missing source code");
    }

    /// Shared-parameter JSON for one offsets/transparency wire value.
    #[test]
    fn the_wire_forms_deserialize() {
        let value = json!({
            "source": "/x/y.png",
            "offsets": {"x": 3_i64, "y": -4_i64},
            "transparency": {"mode": "color", "color": "#FF0000"},
            "replace": true,
        });
        let args: ImageImportParams = serde_json::from_value(value).expect("the wire form");
        let import = parse_import(&args).expect("valid");
        assert_eq!(
            import.offsets,
            Some(OffsetMode::Explicit {
                left: 3_i32,
                top: -4_i32
            })
        );
        assert_eq!(
            import.transparency,
            Transparency::ColorKey {
                rgb: [255_u8, 0, 0]
            },
            "the key colour"
        );
        assert!(import.replace, "the flag carries");
    }
}
