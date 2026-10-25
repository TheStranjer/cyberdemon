//! The patch and raw sides of the image imports: a picture in, a `Doom`
//! patch lump out, or a validated image's bytes stored unchanged.
//!
//! [`import_patch_run`] runs the shared pipeline of [`super::image_pipeline`]
//! and adds the `PNAMES` half outside any lock, then lands everything
//! through [`land_patch`]: one [`crate::state::AppState::with_session`]
//! closure in which [`pnames_plan`] works out from the session as it stands
//! whether the effective list grows and [`commit_patch`] places the patch
//! and, when it does, the written list, in one undoable edit - no other
//! tool's commit can slip between the plan and the write.
//! [`import_raw_run`] skips the conversion: it decodes the bytes only to
//! prove they are a readable image and [`commit_raw`] stores them exactly
//! as they arrived, in the region `namespace` names or loose in the global
//! area.

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PnamesPlan, chosen, parse_namespace, pnames_plan};
use crate::error::CyberError;
use crate::gfx::codec::{decode_image, format_name};
use crate::session::Session;
use crate::state::AppState;
use crate::tools::imports::image_pipeline::{
    ImageImportParams, ImportReport, LumpWrite, OffsetDefault, Patched, build_patch, parse_import,
    put_lump,
};
use crate::tools::reply;
use crate::wad::classify::Namespace;
use crate::wad::lumps;
use crate::wad::name::LumpName;

/// Arguments for [`crate::server::Cyberdemon::import_image_as_patch`] and its alias
/// [`crate::server::Cyberdemon::import_png_as_patch`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ImportPatchParams {
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

/// Arguments for [`crate::server::Cyberdemon::import_image_as_raw`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ImportRawParams {
    /// File to import: absolute path, `file://` URI, `http(s)://` URL or
    /// this server's asset/upload URL. Its bytes are stored unchanged
    /// after a decode check.
    pub(super) source: String,
    /// Lump name to store under: 1 to 8 printable `ASCII` characters,
    /// uppercased. Defaults to the fetched file name's stem uppercased and
    /// trimmed to 8 characters; when nothing usable remains the call fails
    /// asking for this name.
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Region to place the lump in (`sprites`, `flats`, `patches`,
    /// `colormaps`, `acs`, `voices`, `hires` or `textures`); without it
    /// the lump is stored loose, in the global area.
    #[serde(default)]
    pub(super) namespace: Option<String>,
    /// Overwrite a working lump of this name in the target region in place
    /// (default `false`).
    #[serde(default)]
    pub(super) replace: bool,
}

/// Structured result of [`crate::server::Cyberdemon::import_image_as_patch`]
/// and its alias: the shared pipeline report with the placement facts. The
/// fields read back through [`serde_json`] in tests of the landing step.
#[derive(Serialize)]
pub(super) struct PatchReport {
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

/// Structured result of [`crate::server::Cyberdemon::import_image_as_raw`].
#[derive(Serialize)]
pub(super) struct RawReport {
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

/// `import_image_as_patch`' work: run the pipeline (fetch, decode,
/// quantize) outside any lock, then plan and commit the `PNAMES` entry and
/// the patch under one session lock.
///
/// # Errors
///
/// Whatever [`build_patch`] reports for the source and picture, plus
/// [`land_patch`]' errors; a refusal leaves the working archive untouched.
async fn import_patch_run(
    state: &AppState,
    args: &ImportPatchParams,
) -> Result<PatchReport, CyberError> {
    let import = parse_import(&args.common)?;
    let patched = build_patch(state, &import, OffsetDefault::Zero).await?;
    let add_to_pnames = args.add_to_pnames;
    let replace = import.replace;
    state.with_session(move |session| land_patch(session, patched, add_to_pnames, replace))
}

/// The whole session half of a patch import under one lock: [`pnames_plan`]
/// reads the effective `PNAMES` as they stand at this moment and
/// [`commit_patch`] writes what it planned, so a concurrent commit cannot
/// wedge itself between the two and lose either side's entry.
///
/// # Errors
///
/// [`pnames_plan`]' format and store refusals and [`commit_patch`]'
/// placement, `already_exists` and stale-index errors.
pub(super) fn land_patch(
    session: &mut Session,
    patched: Patched,
    add_to_pnames: Option<bool>,
    replace: bool,
) -> Result<PatchReport, CyberError> {
    let plan = pnames_plan(session, patched.name, add_to_pnames)?;
    commit_patch(session, patched, plan, replace)
}

/// The synchronous half of a patch import: one undoable edit that places
/// the patch lump and, when the effective `PNAMES` lacked the name, writes
/// the grown list as a working copy-on-write.
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors and
/// [`crate::wad::archive::Wad::put_global`]' stale-index [`CyberError::Internal`].
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
            wad.put_global(lumps::PNAMES, list.clone())?;
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
pub(super) async fn import_raw_run(
    state: &AppState,
    args: &ImportRawParams,
) -> Result<RawReport, CyberError> {
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

/// The shared body of [`crate::server::Cyberdemon::import_image_as_patch`] and
/// [`crate::server::Cyberdemon::import_png_as_patch`].
pub(super) async fn patch_imported(
    state: &AppState,
    args: &ImportPatchParams,
) -> Result<CallToolResult, ErrorData> {
    reply(import_patch_run(state, args).await)
}
