//! The one-shot floor/ceiling flat: one picture in, one `64x64` flat
//! lump out, in one undoable edit.
//!
//! [`create_flat_run`] flattens [`ProcessParams`] into its arguments:
//! [`process`] runs the user's ops, an automatic `Fit` brings the picture
//! to the vanilla flat shape (`64x64` - every flat this tool writes has
//! that size), a `MakeTileable` op blends opposite edges when `tileable`
//! (the default), [`to_indexed`] quantizes the result opaquely against
//! the session palette, [`encode_flat`] writes the raw index plane and
//! [`decode_flat`] reads it back for the preview. The collision rules
//! ([`check_flat_clash`]) run before anything is written - flats are
//! referenced by name everywhere, so a layer-held name refuses until
//! `replace` - and [`commit_flat`] places the lump as one
//! [`crate::session::Session::edit`] labelled `create_flat NAME`. An
//! `assign_role` request afterwards points a theme flat role at the new
//! flat; theme state is session state and is not part of the undo
//! history, which the result warnings say.

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::assets::fetch::Fetched;
use crate::assets::server::AssetRef;
use crate::error::CyberError;
use crate::gfx::codec::encode_png;
use crate::gfx::flat::{decode_flat, encode_flat};
use crate::gfx::ops::{Filter, FitMode, ImageOp, OpsReport, apply_ops};
use crate::gfx::quantize::Transparency;
use crate::session::{Session, Source};
use crate::state::AppState;
use crate::themes::Role;
use crate::tools::fail;
use crate::tools::imports::image_pipeline::{LumpWrite, put_lump};
use crate::tools::imports::processed::{ProcessParams, Processed, process, to_indexed};
use crate::tools::imports::texture::layer_label;
use crate::tools::imports::transparent_alpha_pixels;
use crate::tools::ok;
use crate::tools::palette::load_palette;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

/// The vanilla flat edge: every flat this tool writes is this square.
const FLAT_SIDE: u32 = 64_u32;
/// The blend band (pixels) of the automatic `make_tileable` step.
const TILE_BAND: u32 = 16_u32;
/// Media type of the published preview.
const PNG_MIME: &str = "image/png";

/// What every successful create reports as the way forward.
const NEXT_STEPS: [&str; 2] = [
    "edit_sector floor_flat/ceiling_flat",
    "auto_texture mode retexture after assign_role",
];

/// The full documentation of [`crate::server::Cyberdemon::create_flat_from_image`].
pub(in crate::tools) const CREATE_FLAT_TOOL_DESCRIPTION: &str = "One call from a picture file to a working Doom floor/ceiling flat: it processes the image (the same ops/dither/palette parameters the preview pipeline takes, see skill/reference/imports.md), fits it to 64x64, optionally blends opposite edges so it tiles seamlessly, quantizes it opaquely to the session PLAYPAL and stores the raw index plane in the working archive's flats region (F_START/F_END markers created there when the working archive has none) - all as ONE undoable edit labelled \"create_flat NAME\" (save_wad writes the result). Flats are always 64x64: vanilla engines tile floors and ceilings at that size, so the processed picture is always fit to it (import_image_as_flat keeps other shapes when you need them). source: absolute path, file:// URI, http(s):// URL or this server's asset/upload URL (the processed URL of an earlier process_image call is a valid source). name: the flat lump name (1 to 8 printable ASCII characters, uppercased). fit maps the picture onto the 64x64 square: \"cover\" (default) fills it and crops the overflow, \"contain\" letterboxes the whole picture with transparent (quantized: near-black) padding, \"stretch\" distorts it to the square. tileable=true (the default) appends a make_tileable op blending a 16-pixel band across opposite edges so neighbouring tiles meet without seams; pass tileable=false to keep the edges exactly as drawn. ops, dither and palette behave exactly as in process_image; your ops run before the automatic fit. Flats store no transparency: pixels whose alpha is below 128 stay opaque and a warning counts them. Collisions refuse before anything is written: a working-archive flat of this name is already_exists unless replace=true overwrites it in place, and a flat of this name a lower (IWAD or resource) layer holds is already_exists too - a new flat of an existing name replaces it EVERYWHERE it is used, so pass replace=true to accept that (then overrides_existing answers true) or choose another name. assign_role: a theme flat role (floor, ceiling, light_ceiling, outdoor_ceiling, door_floor, lift_floor, teleport_floor or liquid_floor) to point at the new flat afterwards; a wall role or unknown name is invalid_argument and nothing is written. Theme state is session state: undo does not roll an override back (the result warns); clear it with set_texture_theme overrides. Returns {flat, steps, colors_used, max_color_error, overrides_existing, preview, role, warnings, next_steps}: steps is one measurement object per applied op (yours first, then the automatic fit and tiling), colors_used and max_color_error measure the quantization (0 error means the source was palette-exact), overrides_existing says whether a same-name flat was overwritten or overridden, preview is the stored flat's picture as the flat decoder reads it back, role echoes an assign_role that landed. Then assign it with edit_sector (floor_flat/ceiling_flat) or dress a whole map with auto_texture mode retexture after assign_role; list_flats and get_flat read the stored lump back and one undo removes it.";

/// Arguments for [`crate::server::Cyberdemon::create_flat_from_image`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(in crate::tools) struct CreateFlatParams {
    /// Picture to turn into a flat: absolute path, `file://` URI,
    /// `http(s)://` URL or this server's asset/upload URL.
    pub(in crate::tools) source: String,
    /// Flat lump name: 1 to 8 printable `ASCII` characters, uppercased.
    pub(in crate::tools) name: String,
    /// How the picture maps onto the `64x64` square: `"cover"` (default),
    /// `"contain"` or `"stretch"`.
    #[serde(default)]
    pub(in crate::tools) fit: Option<FitMode>,
    /// Append a `make_tileable` op (band 16) blending opposite edges so
    /// the flat tiles seamlessly (default `true`).
    #[serde(default = "default_tileable")]
    pub(in crate::tools) tileable: bool,
    /// Theme flat role to point at the new flat afterwards (e.g.
    /// `"floor"`); a wall or unknown role is `invalid_argument`.
    #[serde(default)]
    pub(in crate::tools) assign_role: Option<String>,
    /// Overwrite or override an existing flat of this name (default
    /// `false`: such clashes are `already_exists`).
    #[serde(default)]
    pub(in crate::tools) replace: bool,
    /// The shared processing parameters: `ops`, `dither` and `palette`
    /// (see `skill/reference/imports.md`).
    #[serde(flatten)]
    pub(in crate::tools) shared: ProcessParams,
}

/// The documented `tileable` default.
const fn default_tileable() -> bool {
    true
}

/// The validated arguments of one create-flat call.
#[derive(Debug)]
struct FlatRequest {
    /// The `source` to fetch.
    source: String,
    /// The flat lump name, uppercased.
    name: LumpName,
    /// How the picture maps onto the square.
    fit: FitMode,
    /// Whether a `make_tileable` op follows the fit.
    tileable: bool,
    /// The theme flat role to assign afterwards, if any.
    role: Option<Role>,
    /// Whether an existing same-name flat may be overwritten.
    replace: bool,
    /// The shared processing parameters.
    shared: ProcessParams,
}

/// Validate the create-flat arguments: the name and the theme role. Every
/// refusal here happens before the `source` is fetched.
///
/// # Errors
///
/// [`LumpName::parse`]' [`CyberError::InvalidArgument`] for a bad name and
/// [`parse_flat_role`]' error for a wall or unknown `assign_role`.
fn parse_create_flat(args: &CreateFlatParams) -> Result<FlatRequest, CyberError> {
    Ok(FlatRequest {
        source: args.source.clone(),
        name: LumpName::parse(&args.name)?,
        fit: args.fit.unwrap_or(FitMode::Cover),
        tileable: args.tileable,
        role: parse_flat_role(args.assign_role.as_deref())?,
        replace: args.replace,
        shared: args.shared.clone(),
    })
}

/// The `assign_role` word: a parseable flat role.
///
/// # Errors
///
/// [`Role::parse`]' [`CyberError::InvalidArgument`] for an unknown name,
/// and [`CyberError::InvalidArgument`] naming the wall tools for a wall
/// role.
fn parse_flat_role(text: Option<&str>) -> Result<Option<Role>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    let role = Role::parse(raw)?;
    if !role.is_flat() {
        return Err(CyberError::InvalidArgument(format!(
            "role {raw:?} is a wall role: create_flat_from_image stores flats - build a wall \
             texture with create_texture_from_image and point wall roles at it there or with \
             set_texture_theme"
        )));
    }
    Ok(Some(role))
}

/// The automatic `Fit` to the vanilla square and `MakeTileable` (when
/// asked) ops, appended to the processed picture and its report.
///
/// # Errors
///
/// [`apply_ops`]' [`CyberError::InvalidArgument`] widened to say the
/// failure came from the automatic step, not the user's ops.
fn shape_flat(processed: Processed, fit: FitMode, tileable: bool) -> Result<Processed, CyberError> {
    let mut extra: Vec<ImageOp> = vec![ImageOp::Fit {
        width: FLAT_SIDE,
        height: FLAT_SIDE,
        mode: fit,
        filter: Filter::Area,
        background: None,
    }];
    if tileable {
        extra.push(ImageOp::MakeTileable {
            band: Some(TILE_BAND),
            horizontal: true,
            vertical: true,
        });
    }
    let (rgba, second) = apply_ops(processed.rgba, &extra).map_err(|error| {
        CyberError::InvalidArgument(format!(
            "the automatic flat sizing step failed: {error}: adjust fit or pass tileable=false"
        ))
    })?;
    let mut steps = processed.report.steps;
    steps.extend(second.steps);
    Ok(Processed {
        rgba,
        report: OpsReport { steps },
    })
}

/// The shaped, quantized and encoded flat, ready to plan and commit.
struct BuiltFlat {
    /// The raw `PLAYPAL` index bytes of the stored flat.
    data: Vec<u8>,
    /// The stored flat's `PNG`, read back through the flat decoder.
    preview_png: Vec<u8>,
    /// One measurement object per applied op, in order.
    steps: Vec<Value>,
    /// Distinct source colours.
    colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    max_color_error: i32,
    /// Non-fatal picture facts.
    warnings: Vec<String>,
}

/// Run the whole picture side: user ops, the automatic square fit and
/// tiling, opaque quantization, flat encoding and the decoded preview.
/// The palette read is the only session access here.
///
/// # Errors
///
/// [`process`]' errors for the bytes and the user ops, [`shape_flat`]'
/// errors, [`load_palette`]' [`CyberError::NotFound`], [`encode_flat`],
/// [`decode_flat`] and [`encode_png`]' errors.
fn build_flat(
    state: &AppState,
    request: &FlatRequest,
    fetched: &Fetched,
) -> Result<BuiltFlat, CyberError> {
    let processed = process(&fetched.bytes, &request.shared)?;
    let shaped = shape_flat(processed, request.fit, request.tileable)?;
    let ignored = transparent_alpha_pixels(&shaped.rgba);
    let palette =
        state.with_session(|session| load_palette(session, request.shared.palette_index()))?;
    let indexed = to_indexed(
        &shaped,
        &palette,
        Transparency::None,
        request.shared.dither_choice(),
    );
    let (data, _flattened) = encode_flat(&indexed.image)?;
    let stored = decode_flat(&data)?;
    let preview_png = encode_png(&stored.to_rgba(&palette), None)?;
    Ok(BuiltFlat {
        data,
        preview_png,
        steps: shaped.report.steps,
        colors_used: indexed.colors_used,
        max_color_error: indexed.max_color_error,
        warnings: opaque_warning(ignored),
    })
}

/// The warning for kept-opaque holes, empty for a fully opaque picture.
fn opaque_warning(ignored: usize) -> Vec<String> {
    if ignored == 0 {
        return Vec::new();
    }
    vec![format!(
        "{ignored} pixels below alpha 128 were kept opaque: flats store no transparency - the \
         colour they show is the source's own"
    )]
}

/// Refuse a flat name that already serves, unless `replace` was given,
/// and answer whether this call will overwrite or override.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] for a working or layer-held twin without
/// `replace`; the layer message names the everywhere-used override.
fn check_flat_clash(session: &Session, name: LumpName, replace: bool) -> Result<bool, CyberError> {
    let Some(entry) = session.find(name, Some(Namespace::Flats)) else {
        return Ok(false);
    };
    if replace {
        return Ok(true);
    }
    match entry.source {
        Source::Working => Err(CyberError::AlreadyExists(format!(
            "flat {name} already exists in the working archive's flats region: pass replace=true \
             to overwrite it in place, or another name"
        ))),
        Source::Layer(index) => Err(CyberError::AlreadyExists(format!(
            "flat {name} already exists in {}: this would override {name} everywhere it is \
             used; pass replace=true to do that or choose another name",
            layer_label(session, index),
        ))),
    }
}

/// The synchronous half: one undoable edit placing the flat bytes in the
/// working archive's flats region.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors.
fn commit_flat(
    session: &mut Session,
    request: &FlatRequest,
    built: &BuiltFlat,
) -> Result<(), CyberError> {
    let label = format!("create_flat {}", request.name);
    let write = LumpWrite {
        name: request.name,
        data: &built.data,
        namespace: Some(Namespace::Flats),
        replace: request.replace,
    };
    session.edit(&label, |wad| put_lump(wad, &write).map(|_| ()))?;
    Ok(())
}

/// Point the requested theme role at the new flat. Theme state is session
/// state: no undo step records it.
///
/// # Errors
///
/// [`AppState::with_session`]' lock errors.
fn apply_role(state: &AppState, request: &FlatRequest) -> Result<Option<&'static str>, CyberError> {
    let Some(role) = request.role else {
        return Ok(None);
    };
    let name = request.name.to_string();
    state.with_session(move |session| {
        session.texture_theme_mut().overrides.insert(role, name);
        Ok(Some(role.as_str()))
    })
}

/// Structured result of [`crate::server::Cyberdemon::create_flat_from_image`].
#[derive(Debug, Serialize)]
struct CreatedFlatReport {
    /// The flat name, uppercased.
    flat: String,
    /// One measurement object per applied op, in order.
    steps: Vec<Value>,
    /// Distinct source colours.
    colors_used: usize,
    /// Worst squared `RGB` distance to the palette.
    max_color_error: i32,
    /// Whether this call overwrote or overrode a same-name flat.
    overrides_existing: bool,
    /// The stored flat's `PNG`, read back through the flat decoder.
    preview: AssetRef,
    /// The theme role now standing for this flat, if any.
    role: Option<&'static str>,
    /// Non-fatal picture facts and what the theme override means.
    warnings: Vec<String>,
    /// What to do next.
    next_steps: &'static [&'static str],
}

/// The committed facts a report is assembled from.
struct Landed {
    /// Whether this call overwrote or overrode a same-name flat.
    overrides_existing: bool,
    /// The theme role now standing for this flat, if any.
    role: Option<&'static str>,
    /// The published preview.
    preview: AssetRef,
}

/// Fold the request, the built flat and the landed facts into the
/// documented result.
fn report_of(request: &FlatRequest, built: &BuiltFlat, landed: Landed) -> CreatedFlatReport {
    let mut warnings = built.warnings.clone();
    if let Some(assigned) = landed.role {
        warnings.push(format!(
            "theme override {assigned} -> {name} set in session state: undo does not roll it \
             back, clear it with set_texture_theme overrides",
            name = request.name,
        ));
    }
    CreatedFlatReport {
        flat: request.name.to_string(),
        steps: built.steps.clone(),
        colors_used: built.colors_used,
        max_color_error: built.max_color_error,
        overrides_existing: landed.overrides_existing,
        preview: landed.preview,
        role: landed.role,
        warnings,
        next_steps: &NEXT_STEPS,
    }
}

/// `create_flat_from_image`' work: validate, fetch and process, check the
/// collisions, commit as one edit, then the theme override and the
/// preview publish.
pub(in crate::tools) async fn create_flat_run(
    state: &AppState,
    args: &CreateFlatParams,
) -> Result<CallToolResult, ErrorData> {
    match create_flat_build(state, args).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

/// The measured body of [`create_flat_run`].
///
/// # Errors
///
/// [`parse_create_flat`]' argument errors, [`AppState::fetch_source`]' and
/// [`build_flat`]' errors, [`check_flat_clash`]' and [`commit_flat`]'
/// refusals, [`apply_role`]' and [`AppState::publish`]' errors; a refusal
/// leaves the working archive untouched.
async fn create_flat_build(
    state: &AppState,
    args: &CreateFlatParams,
) -> Result<CreatedFlatReport, CyberError> {
    let request = parse_create_flat(args)?;
    let fetched = state.fetch_source(&request.source).await?;
    let built = build_flat(state, &request, &fetched)?;
    let overrides_existing =
        state.with_session(|session| check_flat_clash(session, request.name, request.replace))?;
    state.with_session(|session| commit_flat(session, &request, &built))?;
    let role = apply_role(state, &request)?;
    let preview = state.publish(
        &format!("{}_flat.png", request.name),
        PNG_MIME,
        built.preview_png.clone(),
    )?;
    Ok(report_of(
        &request,
        &built,
        Landed {
            overrides_existing,
            role,
            preview,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        CREATE_FLAT_TOOL_DESCRIPTION, CreateFlatParams, apply_role, build_flat, check_flat_clash,
        commit_flat, create_flat_build, opaque_warning, parse_create_flat, parse_flat_role,
        shape_flat,
    };
    use crate::assets::fetch::Fetched;
    use crate::error::CyberError;
    use crate::gfx::codec::encode_png;
    use crate::gfx::image::RgbaImage;
    use crate::gfx::ops::FitMode;
    use crate::session::{Layer, LayerRole, Session};
    use crate::state::AppState;
    use crate::themes::Role;
    use crate::tools::imports::processed::ProcessParams;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// Parameters over `source`/`name` with everything else default.
    fn args() -> CreateFlatParams {
        CreateFlatParams {
            source: "/x/y.png".to_owned(),
            name: "MYFLAT".to_owned(),
            fit: None,
            tileable: true,
            assign_role: None,
            replace: false,
            shared: ProcessParams {
                ops: Vec::new(),
                dither: None,
                palette: None,
            },
        }
    }

    /// A `width` x `height` opaque test picture of ramp colours.
    fn picture(width: u32, height: u32) -> RgbaImage {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let pixels: Vec<u8> = (0..count)
            .flat_map(|pixel| {
                let slot = u8::try_from(pixel.checked_rem(256_usize).expect("non-zero"))
                    .expect("remainders fit u8");
                [slot, 255_u8.wrapping_sub(slot), slot, 255_u8]
            })
            .collect();
        RgbaImage::new(width, height, pixels).expect("test shape is valid")
    }

    #[test]
    fn parse_applies_the_documented_defaults() {
        let request = parse_create_flat(&args()).expect("a source and name are enough");
        assert_eq!(request.name, name("MYFLAT"), "the name carries");
        assert_eq!(request.fit, FitMode::Cover, "cover by default");
        assert!(request.tileable, "tileable is on unless passed false");
        assert!(request.role.is_none(), "no role asked for");
        assert!(!request.replace, "no silent overwrite");
        let mut lower = args();
        lower.name = "myflat".to_owned();
        assert_eq!(
            parse_create_flat(&lower).expect("lowercase").name,
            name("MYFLAT"),
            "the name uppercases"
        );
        let mut bad = args();
        bad.name = "way too long".to_owned();
        assert_eq!(
            code(&parse_create_flat(&bad).expect_err("no lump name")),
            "invalid_argument"
        );
    }

    #[test]
    fn roles_must_parse_and_stay_on_the_flats() {
        assert!(parse_flat_role(None).expect("absent").is_none());
        assert_eq!(
            parse_flat_role(Some("LIGHT_CEILING")).expect("a flat role"),
            Some(Role::LightCeiling)
        );
        let error = parse_flat_role(Some("wall")).expect_err("a wall role");
        assert_eq!(code(&error), "invalid_argument");
        let message = error.to_string();
        assert!(
            message.contains("wall role"),
            "the message says why: {message}"
        );
        assert!(
            message.contains("create_texture_from_image"),
            "and names the way out: {message}"
        );
        assert_eq!(
            code(&parse_flat_role(Some("roof")).expect_err("no such role")),
            "invalid_argument",
            "an unknown role refuses too"
        );
    }

    /// The step kinds of a shaped report, in order.
    fn step_kinds(shaped: &super::Processed) -> Vec<&str> {
        shaped
            .report
            .steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect()
    }

    /// A processed picture straight from an encoded test PNG.
    fn processed(width: u32, height: u32) -> super::Processed {
        let png = encode_png(&picture(width, height), None).expect("test PNG encodes");
        super::process(
            &png,
            &ProcessParams {
                ops: Vec::new(),
                dither: None,
                palette: None,
            },
        )
        .expect("no ops")
    }

    #[test]
    fn shape_fits_to_the_square_and_appends_the_tileable_step() {
        let shaped = shape_flat(processed(100, 80), FitMode::Cover, true).expect("both ops run");
        assert_eq!((shaped.rgba.width, shaped.rgba.height), (64_u32, 64_u32));
        assert_eq!(
            step_kinds(&shaped),
            vec!["fit".to_owned(), "make_tileable".to_owned()],
            "fit then tileable"
        );
        let flat = shape_flat(processed(64, 64), FitMode::Stretch, false).expect("fit runs");
        assert_eq!(
            step_kinds(&flat),
            vec!["fit".to_owned()],
            "the fit always runs"
        );
    }

    #[test]
    fn the_opaque_warning_counts_kept_holes_only() {
        assert!(
            opaque_warning(0_usize).is_empty(),
            "an opaque flat is quiet"
        );
        let warnings = opaque_warning(12_usize);
        assert_eq!(warnings.len(), 1_usize, "one warning");
        assert!(
            warnings[0].contains("12 pixels") && warnings[0].contains("no transparency"),
            "the warning explains the keep: {}",
            warnings[0]
        );
    }

    /// A session whose working archive holds a flats region with `FLOOR1`.
    fn working_session() -> Session {
        let mut session = Session::default();
        session
            .edit("seed", |wad| {
                wad.lumps.push(Lump::marker(name("F_START")));
                wad.lumps.push(Lump::new(name("FLOOR1"), vec![1_u8; 4096]));
                wad.lumps.push(Lump::marker(name("F_END")));
                Ok(())
            })
            .expect("plain insert");
        session
    }

    /// A session on an `IWAD` layer holding a flats region with `FLOOR1`.
    fn layer_session() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            "base.wad".into(),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::marker(name("F_START")),
                    Lump::new(name("FLOOR1"), vec![1_u8; 4096]),
                    Lump::marker(name("F_END")),
                ],
            },
        ));
        session
    }

    #[test]
    fn a_free_flat_name_reports_no_override() {
        let session = working_session();
        assert!(
            !check_flat_clash(&session, name("NEWFLAT"), false).expect("free"),
            "nothing was there"
        );
    }

    #[test]
    fn a_working_twin_refuses_until_replace() {
        let session = working_session();
        let error = check_flat_clash(&session, name("FLOOR1"), false).expect_err("the twin stands");
        assert_eq!(code(&error), "already_exists");
        assert!(
            error.to_string().contains("working archive"),
            "the message names the archive: {error}"
        );
        assert!(
            check_flat_clash(&session, name("FLOOR1"), true).expect("replace"),
            "override"
        );
    }

    #[test]
    fn a_layer_flat_refuses_with_the_everywhere_hint() {
        let session = layer_session();
        let error = check_flat_clash(&session, name("FLOOR1"), false).expect_err("the IWAD stands");
        assert_eq!(code(&error), "already_exists");
        let message = error.to_string();
        assert!(
            message.contains("this would override FLOOR1 everywhere it is used"),
            "the hint names the consequence: {message}"
        );
        assert!(
            message.contains("pass replace=true to do that or choose another name"),
            "and the way out: {message}"
        );
        assert!(message.contains("the IWAD"), "and the layer: {message}");
        assert!(
            check_flat_clash(&session, name("FLOOR1"), true).expect("replace"),
            "override"
        );
    }

    #[test]
    fn one_edit_places_the_flat_and_undo_takes_it_away() {
        let mut session = Session::default();
        session
            .edit("seed", |wad| {
                wad.lumps.push(Lump::new(name("PLAYPAL"), vec![0_u8; 768]));
                Ok(())
            })
            .expect("plain insert");
        let request = parse_create_flat(&args()).expect("valid");
        let built = super::BuiltFlat {
            data: vec![7_u8; 4096],
            preview_png: Vec::new(),
            steps: Vec::new(),
            colors_used: 1,
            max_color_error: 0,
            warnings: Vec::new(),
        };
        commit_flat(&mut session, &request, &built).expect("the commit lands");
        assert_eq!(
            session.undo_labels(),
            vec!["create_flat MYFLAT", "seed"],
            "one undoable step"
        );
        assert!(
            session
                .find(name("MYFLAT"), Some(Namespace::Flats))
                .is_some(),
            "the flats region took the lump"
        );
        session.undo().expect("one step back");
        assert!(
            session
                .find(name("MYFLAT"), Some(Namespace::Flats))
                .is_none(),
            "the flat left with its edit"
        );
    }

    /// Fixture palette bytes: two entries of `(i, 255 - i, i)`.
    fn playpal() -> Vec<u8> {
        (0_u32..512_u32)
            .map(|position| {
                u8::try_from(position.checked_rem(256_u32).expect("non-zero"))
                    .expect("remainders fit u8")
            })
            .flat_map(|index| [index, 255_u8.wrapping_sub(index), index])
            .collect()
    }

    /// Appends a `PLAYPAL` lump to a session's working archive.
    fn seed_playpal(session: &mut Session) -> Result<(), CyberError> {
        session.edit("seed", |wad| {
            wad.lumps.push(Lump::new(name("PLAYPAL"), playpal()));
            Ok(())
        })
    }

    /// A state whose session carries a `PLAYPAL`.
    fn state() -> AppState {
        let state = AppState::default();
        state
            .with_session(seed_playpal)
            .expect("the seed edit runs");
        state
    }

    #[tokio::test]
    async fn build_quantizes_opaquely_and_reads_the_flat_back() {
        let state = state();
        let request = parse_create_flat(&args()).expect("valid");
        let png = encode_png(&picture(100, 80), None).expect("test PNG encodes");
        let fetched = Fetched {
            bytes: png,
            filename: "art.png".to_owned(),
            mime: Some("image/png".to_owned()),
        };
        let built = build_flat(&state, &request, &fetched).expect("the build runs");
        assert_eq!(built.data.len(), 4096_usize, "a vanilla 64x64 flat");
        assert_eq!(
            super::decode_flat(&built.data)
                .expect("the bytes decode")
                .width,
            64_u32,
            "at the square shape"
        );
        assert_eq!(built.steps.len(), 2_usize, "fit and make_tileable");
        assert!(built.warnings.is_empty(), "the picture is opaque");
    }

    /// The session's `Floor` override and its undo depth.
    fn floor_theme(state: &AppState) -> (Option<String>, usize) {
        state
            .with_session(|session| {
                Ok((
                    session.texture_theme().overrides.get(&Role::Floor).cloned(),
                    session.undo_labels().len(),
                ))
            })
            .expect("the read runs")
    }

    #[tokio::test]
    async fn the_theme_override_lands_in_session_state_outside_history() {
        let state = state();
        let mut request = parse_create_flat(&args()).expect("valid");
        request.role = Some(Role::Floor);
        let role = apply_role(&state, &request).expect("the override sets");
        assert_eq!(role, Some("floor"), "the role reports back");
        let (texture, undo_len) = floor_theme(&state);
        assert_eq!(texture.as_deref(), Some("MYFLAT"), "the theme points at it");
        assert_eq!(undo_len, 1_usize, "only the seed edit stands");
    }

    /// The session's undo labels.
    fn undo_labels(state: &AppState) -> Vec<String> {
        state
            .with_session(|session| {
                Ok(session
                    .undo_labels()
                    .iter()
                    .copied()
                    .map(str::to_owned)
                    .collect())
            })
            .expect("the read runs")
    }

    #[tokio::test]
    async fn a_layer_flat_refuses_the_whole_run_without_writing() {
        let state = AppState::default();
        state
            .with_session(|session| Ok(core::mem::replace(session, layer_session_with_palette())))
            .expect("the seed runs");
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = dir.path().join("art.png");
        std::fs::write(
            &file,
            encode_png(&picture(20, 20), None).expect("test PNG encodes"),
        )
        .expect("test file writes");
        let mut call = args();
        call.source = file.display().to_string();
        call.name = "FLOOR1".to_owned();
        let error = create_flat_build(&state, &call)
            .await
            .expect_err("the IWAD flat stands");
        assert_eq!(code(&error), "already_exists");
        assert!(undo_labels(&state).is_empty(), "nothing was written");
    }

    /// A session on a `PLAYPAL`-carrying `IWAD` layer with a `FLOOR1` flat.
    fn layer_session_with_palette() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            "base.wad".into(),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    Lump::new(name("PLAYPAL"), playpal()),
                    Lump::marker(name("F_START")),
                    Lump::new(name("FLOOR1"), vec![1_u8; 4096]),
                    Lump::marker(name("F_END")),
                ],
            },
        ));
        session
    }

    #[test]
    fn the_wire_form_flattens_the_shared_fields() {
        let call: CreateFlatParams = serde_json::from_value(serde_json::json!({
            "source": "/x/y.png",
            "name": "MYFLAT",
            "fit": "contain",
            "tileable": false,
            "ops": [{"op": "flip", "horizontal": true, "vertical": false}],
            "dither": "ordered",
            "palette": 2_u64,
            "assign_role": "ceiling",
            "replace": true,
        }))
        .expect("the documented wire form");
        assert_eq!(call.fit, Some(FitMode::Contain));
        assert!(!call.tileable, "the flag carries");
        assert_eq!(call.shared.ops.len(), 1_usize, "the op list carries");
        assert_eq!(call.shared.palette, Some(2_usize));
        assert_eq!(call.assign_role.as_deref(), Some("ceiling"));
        assert!(call.replace);
        assert!(
            CREATE_FLAT_TOOL_DESCRIPTION.contains("create_flat NAME"),
            "the description names the undo label"
        );
    }
}
