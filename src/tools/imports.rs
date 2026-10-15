//! Image import tools: whole pictures in, lumps out.
//!
//! [`Cyberdemon::import_image_as_patch`] (and its alias
//! [`Cyberdemon::import_png_as_patch`]) runs the shared pipeline of
//! [`image_pipeline`] - fetch, decode, quantize to the session's
//! `PLAYPAL`, resolve drawing offsets, encode a `Doom` patch - and drops
//! the result into the working archive's patches region as one undoable
//! edit, optionally listing the name in `PNAMES` (copy-on-write).
//! [`Cyberdemon::import_image_as_raw`] validates that the bytes are a
//! readable image and stores them unchanged, for `ZDoom`-family ports
//! that read `PNG` lumps directly. Flat, sprite and patch-replacement
//! tools grow on the same pipeline pieces.

mod image_pipeline;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::codec::{decode_image, format_name};
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::fail;
use crate::tools::imports::image_pipeline::{
    ImageImportParams, ImportReport, LumpWrite, OffsetDefault, Patched, build_patch, chosen_name,
    parse_import, put_lump,
};
use crate::tools::ok;
use crate::tools::textures::put_global;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;
use crate::wad::textures::{parse_pnames, write_pnames};

/// Raw directory bytes of the patch-name lump.
const PNAMES_RAW: [u8; 8] = *b"PNAMES\0\0";

/// Arguments for [`Cyberdemon::import_image_as_patch`] and its alias
/// [`Cyberdemon::import_png_as_patch`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportPatchParams {
    /// The shared image-import parameters: `source`, `name`, `palette`,
    /// `transparency`, `offsets` and `replace` (see
    /// `skill/reference/imports.md`).
    #[serde(flatten)]
    common: ImageImportParams,
    /// Also list the name in `PNAMES`, appending it copy-on-write when
    /// missing (default `true`).
    #[serde(default)]
    add_to_pnames: Option<bool>,
}

/// Arguments for [`Cyberdemon::import_image_as_raw`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ImportRawParams {
    /// File to import: absolute path, `file://` URI, `http(s)://` URL or
    /// this server's asset/upload URL. Its bytes are stored unchanged
    /// after a decode check.
    source: String,
    /// Lump name to store under: 1 to 8 printable `ASCII` characters,
    /// uppercased. Defaults to the fetched file name's stem uppercased and
    /// trimmed to 8 characters; when nothing usable remains the call fails
    /// asking for this name.
    #[serde(default)]
    name: Option<String>,
    /// Region to place the lump in (`sprites`, `flats`, `patches`,
    /// `colormaps`, `acs`, `voices`, `hires` or `textures`); without it
    /// the lump is stored loose, in the global area.
    #[serde(default)]
    namespace: Option<String>,
    /// Overwrite a working lump of this name in the target region in place
    /// (default `false`).
    #[serde(default)]
    replace: bool,
}

/// Structured result of [`Cyberdemon::import_image_as_patch`] and its
/// alias: the shared pipeline report with the placement facts.
#[derive(Serialize)]
struct PatchReport {
    /// The name the patch was stored under.
    name: LumpName,
    /// Its directory position in the working archive.
    lump_index: usize,
    /// Its `PNAMES` entry index, `null` when the name is not listed.
    pnames_index: Option<usize>,
    /// The shared `width`, `height`, `left_offset`, `top_offset`,
    /// `transparent_pixels`, `colors_used` and `max_color_error`.
    #[serde(flatten)]
    pipeline: ImportReport,
}

/// Structured result of [`Cyberdemon::import_image_as_raw`].
#[derive(Serialize)]
struct RawReport {
    /// The name the bytes were stored under.
    name: LumpName,
    /// Its directory position in the working archive.
    lump_index: usize,
    /// The sniffed image format: `"png"`, `"jpeg"`, `"gif"` or `"bmp"`.
    format: &'static str,
    /// Width in pixels, from the decode check.
    width: u32,
    /// Height in pixels.
    height: u32,
}

/// The `PNAMES` side of one patch import.
struct PnamesPlan {
    /// The index the name answers with in the effective list.
    index: usize,
    /// The new `PNAMES` bytes when the list grew, `None` when the name was
    /// already listed and nothing needs writing.
    bytes: Option<Vec<u8>>,
}

/// The name to import under: the explicit one, else one derived from the
/// fetched file name.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] asking for `name` when nothing usable
/// can be derived.
fn chosen(explicit: Option<&str>, filename: &str) -> Result<LumpName, CyberError> {
    let parsed = explicit.map(LumpName::parse).transpose()?;
    chosen_name(parsed, filename)
}

/// The region a placement keeps, `None` staying the global area.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
fn parse_namespace(text: Option<&str>) -> Result<Option<Namespace>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    Namespace::parse(raw).map(Some).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "unknown namespace {raw:?}: expected one of {}",
            namespace_names()
        ))
    })
}

/// The comma-separated namespace region names.
fn namespace_names() -> String {
    Namespace::ALL
        .iter()
        .map(|namespace| namespace.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The merged view's effective `PNAMES` names; no `PNAMES` lump is the
/// empty list.
///
/// # Errors
///
/// [`parse_pnames`]' [`CyberError::Format`] for a malformed lump.
fn current_pnames(session: &Session) -> Result<Vec<String>, CyberError> {
    let Some(entry) = session.find(LumpName::from_raw(PNAMES_RAW), None) else {
        return Ok(Vec::new());
    };
    parse_pnames(&entry.lump.data)
}

/// The position of `name` in `list`, matched case-insensitively.
fn position_of(list: &[String], name: &str) -> Option<usize> {
    list.iter()
        .position(|entry| entry.eq_ignore_ascii_case(name))
}

/// What an import with `add_to_pnames` answers and writes: the entry index
/// (existing or freshly appended) and, only when the effective list grew,
/// the new lump bytes.
///
/// # Errors
///
/// [`parse_pnames`]' [`CyberError::Format`] for a malformed `PNAMES` and
/// [`write_pnames`]' [`CyberError::InvalidArgument`] for a name it cannot
/// store.
fn pnames_plan(
    state: &AppState,
    name: LumpName,
    add: Option<bool>,
) -> Result<Option<PnamesPlan>, CyberError> {
    if !add.unwrap_or(true) {
        return Ok(None);
    }
    let current = state.with_session(|session| current_pnames(session))?;
    let text = name.to_string();
    if let Some(index) = position_of(&current, &text) {
        return Ok(Some(PnamesPlan { index, bytes: None }));
    }
    let mut names = current;
    let index = names.len();
    names.push(text);
    Ok(Some(PnamesPlan {
        index,
        bytes: Some(write_pnames(&names)?),
    }))
}

/// `import_image_as_patch`' work: run the pipeline, plan the `PNAMES`
/// entry, then commit patch and list as one undoable edit.
///
/// # Errors
///
/// Whatever [`build_patch`] reports for the source and picture, plus
/// [`pnames_plan`]' and [`put_lump`]' errors; a refusal leaves the working
/// archive untouched.
async fn import_patch_run(
    state: &AppState,
    args: &ImportPatchParams,
) -> Result<PatchReport, CyberError> {
    let import = parse_import(&args.common)?;
    let patched = build_patch(state, &import, OffsetDefault::Zero).await?;
    let plan = pnames_plan(state, patched.name, args.add_to_pnames)?;
    let replace = import.replace;
    state.with_session(move |session| commit_patch(session, patched, plan, replace))
}

/// The synchronous half of a patch import: one undoable edit that places
/// the patch lump and, when the effective `PNAMES` lacked the name, writes
/// the grown list as a working copy-on-write.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors and
/// [`crate::tools::textures::put_global`]' stale-index [`CyberError::Internal`].
fn commit_patch(
    session: &mut Session,
    patched: Patched,
    plan: Option<PnamesPlan>,
    replace: bool,
) -> Result<PatchReport, CyberError> {
    let label = format!("import_patch {}", patched.name);
    session.edit(&label, |wad| {
        let write = LumpWrite {
            name: patched.name,
            data: &patched.data,
            namespace: Some(Namespace::Patches),
            replace,
        };
        let lump_index = put_lump(wad, &write)?;
        if let Some(list) = plan.as_ref().and_then(|entry| entry.bytes.as_ref()) {
            put_global(wad, LumpName::from_raw(PNAMES_RAW), list.clone())?;
        }
        Ok(PatchReport {
            name: patched.name,
            lump_index,
            pnames_index: plan.map(|entry| entry.index),
            pipeline: patched.report,
        })
    })
}

/// `import_image_as_raw`' work: fetch, run the decode check, store the
/// original bytes as one undoable edit.
///
/// # Errors
///
/// [`AppState::fetch_source`]' errors for the `source`,
/// [`decode_image`]' errors for bytes that are no readable image and
/// [`put_lump`]' placement and `already_exists` errors.
async fn import_raw_run(state: &AppState, args: &ImportRawParams) -> Result<RawReport, CyberError> {
    let namespace = parse_namespace(args.namespace.as_deref())?;
    let fetched = state.fetch_source(&args.source).await?;
    let name = chosen(args.name.as_deref(), &fetched.filename)?;
    let image = decode_image(&fetched.bytes)?;
    let format = format_name(&fetched.bytes).ok_or_else(|| {
        CyberError::Internal("a decoded image answered no format name".to_owned())
    })?;
    let (width, height) = (image.width, image.height);
    let write = LumpWrite {
        name,
        data: &fetched.bytes,
        namespace,
        replace: args.replace,
    };
    let lump_index = state.with_session(|session| commit_raw(session, &write))?;
    Ok(RawReport {
        name,
        lump_index,
        format,
        width,
        height,
    })
}

/// The synchronous half of a raw import: one undoable byte-for-byte insert.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors.
fn commit_raw(session: &mut Session, write: &LumpWrite<'_>) -> Result<usize, CyberError> {
    let label = format!("import_raw {}", write.name);
    session.edit(&label, |wad| put_lump(wad, write))
}

/// The shared body of [`Cyberdemon::import_image_as_patch`] and
/// [`Cyberdemon::import_png_as_patch`].
async fn patch_imported(
    state: &AppState,
    args: &ImportPatchParams,
) -> Result<CallToolResult, ErrorData> {
    match import_patch_run(state, args).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}

/// The full documentation of the patch import, shared by the tool and its
/// alias; [`ALIASED_PATCH_TOOL_DESCRIPTION`] is the alias's variant.
const PATCH_TOOL_DESCRIPTION: &str = "Imports an image file (PNG, JPEG, GIF or BMP bytes) as a Doom patch lump: the picture is quantized to the session's PLAYPAL and stored in the classic column/post format in the working archive's patches region (P_START/P_END markers created there when the working archive has none), as one undoable edit labelled \"import_patch NAME\" (save_wad writes the result). source: absolute path, file:// URI, http(s):// URL or this server's asset/upload URL. name defaults to the file stem uppercased and trimmed to 8 characters; an underivable name fails with invalid_argument. palette (default 0) picks the PLAYPAL entry the picture is quantized against: a session without PLAYPAL, or an index it lacks, is not_found. transparency decides which source pixels become patch holes: {mode: \"alpha\"} (the default) drops alpha below threshold (default 128), {mode: \"color\"} drops pixels matching color exactly (#RRGGBB or #RRGGBBAA, default \"#00FFFF\": Doom's cyan), {mode: \"none\"} keeps everything opaque. offsets are the drawing offsets stored in the patch header: omitted or \"grab\" takes a PNG grAb chunk and falls back to this tool's default \"zero\" when the picture carries none; \"center\" is (width/2, height/2) and \"center_bottom\" is (width/2, height) in the Doom convention (a positive left offset shifts the image left, the top offset is the distance from the image's top edge to the origin); {\"x\": n, \"y\": n} states them literally. Offsets outside the i16 range a patch header stores are invalid_argument. A working patches lump of this name is already_exists unless replace=true overwrites it in place; a same-name lump only the IWAD or a resource layer holds is fine and becomes an override. add_to_pnames (default true) also lists the name in PNAMES, copy-on-write: an already-listed name keeps its index and nothing is rewritten. Returns {name, lump_index, pnames_index, width, height, left_offset, top_offset, transparent_pixels, colors_used, max_color_error}: lump_index is the position in the working archive, pnames_index the PNAMES entry (null when not listed), colors_used counts distinct opaque source colours and max_color_error is the worst squared RGB distance to the palette. list_patches shows the result and get_patch publishes it back; import_image_as_raw stores the original bytes unchanged.";

/// [`PATCH_TOOL_DESCRIPTION`] with the alias note on the front.
const ALIASED_PATCH_TOOL_DESCRIPTION: &str = "Alias of import_image_as_patch - it accepts any supported image, not just PNG; the name exists for PNG-fluent clients. Everything else is identical: same parameters, same patches-region placement and PNAMES listing, same undo label and result. See import_image_as_patch for the full documentation.";

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = imports_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Imports an image file as a Doom patch lump in the patches region.
    #[tool(description = PATCH_TOOL_DESCRIPTION)]
    async fn import_image_as_patch(
        &self,
        Parameters(args): Parameters<ImportPatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        patch_imported(self.state(), &args).await
    }

    /// Alias of [`Cyberdemon::import_image_as_patch`].
    #[tool(description = ALIASED_PATCH_TOOL_DESCRIPTION)]
    async fn import_png_as_patch(
        &self,
        Parameters(args): Parameters<ImportPatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        patch_imported(self.state(), &args).await
    }

    /// Stores a validated image file's bytes unchanged as a lump.
    #[tool(
        description = "Validates that source bytes are a readable image (PNG, JPEG, GIF or BMP) and inserts them into the working archive exactly as they arrived - no quantization, no re-encoding - as one undoable edit labelled \"import_raw NAME\" (save_wad writes the result). This is how ZDoom-family ports carry graphics: they read PNG and JPEG lumps directly, so the original file reaches the game untouched. source: absolute path, file:// URI, http(s):// URL or this server's asset/upload URL. name defaults to the file stem uppercased and trimmed to 8 characters (an underivable name fails with invalid_argument). namespace is optional: without it the lump is stored loose, in the global area; pass \"patches\", \"sprites\", \"flats\" or another region name to place it inside that region (its markers are created in the working archive when it has none). A working lump of this name in the same region is already_exists unless replace=true overwrites it in place; a same-name lump only a read-only layer holds becomes an override. Bytes that fail the decode check report format_error (or unsupported) and nothing is stored. Returns {name, lump_index, format, width, height}: format is \"png\", \"jpeg\", \"gif\" or \"bmp\" as sniffed from the bytes, width and height come from the decode check, lump_index is the position in the working archive. For a vanilla Doom column/post patch use import_image_as_patch; get_lump and extract_lump read stored lumps back."
    )]
    async fn import_image_as_raw(
        &self,
        Parameters(args): Parameters<ImportRawParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match import_raw_run(self.state(), &args).await {
            Ok(report) => ok(&report),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ImportRawParams, chosen, current_pnames, namespace_names, parse_namespace, pnames_plan,
        position_of,
    };
    use crate::error::CyberError;
    use crate::session::Session;
    use crate::state::AppState;
    use crate::wad::archive::Lump;
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;
    use crate::wad::textures::write_pnames;

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A `PNAMES` payload listing `texts`.
    fn pnames_bytes(texts: &[&str]) -> Vec<u8> {
        write_pnames(
            &texts
                .iter()
                .map(|text| (*text).to_owned())
                .collect::<Vec<String>>(),
        )
        .expect("test names are valid")
    }

    #[test]
    fn chosen_takes_the_explicit_name_or_derives_one() {
        assert_eq!(
            chosen(Some("given"), "art.png").expect("explicit"),
            name("GIVEN")
        );
        assert_eq!(chosen(None, "art.png").expect("derived"), name("ART"));
        assert_eq!(
            code(&chosen(Some("too long"), "art.png").expect_err("bad name")),
            "invalid_argument"
        );
        let error = chosen(None, "###.png").expect_err("nothing derivable");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            error.to_string().contains("name"),
            "names the parameter: {error}"
        );
    }

    #[test]
    fn namespaces_parse_case_insensitively_and_refuse_else() {
        assert_eq!(parse_namespace(None).expect("absent"), None);
        assert_eq!(
            parse_namespace(Some("Patches")).expect("region"),
            Some(Namespace::Patches)
        );
        let error = parse_namespace(Some("nope")).expect_err("not a region");
        assert_eq!(code(&error), "invalid_argument");
        assert!(
            namespace_names()
                .split(", ")
                .all(|region| error.to_string().contains(region)),
            "the message lists every region: {error}"
        );
    }

    #[test]
    fn position_matching_ignores_case() {
        let list = ["WALLP1".to_owned(), "STEPP1".to_owned()];
        assert_eq!(position_of(&list, "stepp1"), Some(1));
        assert_eq!(position_of(&list, "GONE"), None);
    }

    #[test]
    fn current_pnames_reads_the_merged_lump_or_nothing() {
        let mut session = Session::default();
        assert!(
            current_pnames(&session).expect("no lump").is_empty(),
            "without a PNAMES lump the list is empty"
        );
        seed_pnames_session(&mut session, &["WALLP1"]);
        assert_eq!(
            current_pnames(&session).expect("the lump lists"),
            vec!["WALLP1".to_owned()]
        );
    }

    /// Appends a `PNAMES` lump listing `texts` to a session's working archive.
    fn seed_pnames_session(session: &mut Session, texts: &[&str]) {
        let bytes = pnames_bytes(texts);
        session
            .edit("seed", move |wad| {
                wad.lumps.push(Lump::new(name("PNAMES"), bytes));
                Ok(())
            })
            .expect("plain append");
    }

    /// Seeds a state's working archive with a `PNAMES` lump.
    fn seed_pnames_state(state: &AppState, texts: &[&str]) {
        state
            .with_session(|session| {
                seed_pnames_session(session, texts);
                Ok(())
            })
            .expect("the seed edit runs");
    }

    #[tokio::test]
    async fn pnames_plans_keep_existing_entries_and_append_new_ones() {
        let state = AppState::default();
        seed_pnames_state(&state, &["WALLP1", "STEPP1"]);
        assert!(
            pnames_plan(&state, name("NEW"), Some(false))
                .expect("off stays off")
                .is_none(),
            "add_to_pnames false writes nothing"
        );
        let kept = pnames_plan(&state, name("wallp1"), None)
            .expect("existing")
            .expect("planned");
        assert_eq!(
            kept.index, 0_usize,
            "the existing entry answers with its index"
        );
        assert!(kept.bytes.is_none(), "the list does not change");
        let added = pnames_plan(&state, name("NEWART"), None)
            .expect("appended")
            .expect("planned");
        assert_eq!(added.index, 2_usize, "the new entry lands last");
        assert!(added.bytes.is_some(), "the grown list gets written");
    }

    /// The raw arguments' wire shape deserializes with the documented flags.
    #[test]
    fn raw_arguments_deserialize() {
        let args: ImportRawParams = serde_json::from_value(serde_json::json!({
            "source": "/x/y.png",
            "namespace": "patches",
            "replace": true,
        }))
        .expect("the wire form");
        assert_eq!(args.namespace.as_deref(), Some("patches"));
        assert!(args.replace, "the flag carries");
    }
}
