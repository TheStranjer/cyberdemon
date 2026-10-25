//! Patch lifecycle tools: whole patch lumps in, over, and out again.
//!
//! [`Cyberdemon::add_patch`] puts one patch lump into the working
//! archive's `patches` region as a single undoable edit. A source that
//! already is a `Doom` patch is stored unchanged - only an explicit
//! `offsets` pair rewrites its two header fields - while any other picture
//! runs the shared image-import pipeline of
//! [`crate::tools::imports::image_pipeline`], the very one
//! `import_image_as_patch` uses, so `palette`, `transparency` and
//! `offsets` behave exactly as [`crate::tools::imports`] documents them.
//! [`Cyberdemon::replace_patch`] swaps the bytes of a patch the session
//! already has - the working archive's own copy overwritten in place, or a
//! working override inserted over an `IWAD` or resource-layer lump - on
//! the same staged-bytes plumbing and never touching `PNAMES`.
//! [`Cyberdemon::remove_patch`] deletes a working patch lump and, on
//! request, its `PNAMES` entry - renumbering every texture patch reference
//! through the shared [`crate::tools::textures`] plan, lump and list in one
//! undoable edit.
//!
//! The two runs live in the private submodules: [`mod@write`] holds the
//! `add_patch` and `replace_patch` work with the merged-view scans that
//! decide where a patch is written, [`mod@remove`] the `remove_patch` work
//! with the working scan and the guarded lump deletion. This root keeps
//! what the three share - the parameters, the staged bytes and their
//! staging, the result reports and the router
//! methods - and the group's unit tests live in `patches/tests.rs`.

mod remove;
mod write;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::patch::{decode_patch, patch_header, set_patch_offsets};
use crate::server::Cyberdemon;
use crate::state::AppState;
use crate::tools::fail;
use crate::tools::imports::image_pipeline::{
    ImageImportParams, OffsetDefault, OffsetMode, OffsetsParam, ParsedImport, Patched,
    TransparencyParams, build_patch,
};
use crate::tools::ok;
use crate::tools::patches::remove::remove_patch_run;
use crate::tools::patches::write::{add_patch_run, replace_patch_run};
use crate::wad::classify::is_doom_patch;
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::add_patch`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddPatchParams {
    /// Lump name to store under: 1 to 8 printable `ASCII` characters,
    /// uppercased. Required: `add_patch` never derives a name from the
    /// file.
    name: String,
    /// The picture side: `source`, `offsets`, `palette` and
    /// `transparency`, the shared import parameters of
    /// `skill/reference/imports.md` without `name` and `replace`.
    #[serde(flatten)]
    picture: PatchSourceParams,
    /// Also list the name in `PNAMES`, appending it copy-on-write when
    /// missing (default `true`).
    #[serde(default)]
    add_to_pnames: Option<bool>,
}

/// The [`ImageImportParams`] fields [`Cyberdemon::add_patch`] and
/// [`Cyberdemon::replace_patch`] take: the shared picture parameters minus
/// `name` (required at each tool's top level) and minus `replace` (the
/// tools decide the write themselves - `add_patch` never overwrites,
/// `replace_patch` always does).
#[derive(Debug, Deserialize, JsonSchema)]
struct PatchSourceParams {
    /// File to import: absolute path, `file://` URI, `http(s)://` URL or
    /// this server's asset/upload URL.
    source: String,
    /// Drawing offsets: `"zero"`, `"grab"`, `"center"`, `"center_bottom"`
    /// or an explicit `{"x": n, "y": n}` pair. Defaults to `"zero"`: a
    /// converted `PNG`'s `grAb` chunk still wins over that default, and on
    /// a passthrough patch only an explicit pair rewrites the header.
    #[serde(default)]
    offsets: Option<OffsetsParam>,
    /// Which `PLAYPAL` entry a converted picture is quantized against
    /// (default 0; the passthrough half never reads a palette).
    #[serde(default)]
    palette: Option<usize>,
    /// Which source pixels become transparent (see
    /// `skill/reference/imports.md`; default `{mode: "alpha", threshold:
    /// 128}`; the passthrough half never quantizes).
    #[serde(default)]
    transparency: Option<TransparencyParams>,
}

/// Arguments for [`Cyberdemon::replace_patch`]: an existing patch name and
/// the same picture side [`AddPatchParams`] takes. No `add_to_pnames`: the
/// name list is never touched, and every `PNAMES` index stays put.
#[derive(Debug, Deserialize, JsonSchema)]
struct ReplacePatchParams {
    /// Name of the patch to replace: the merged view must hold it, in the
    /// `patches` region or as a `PNAMES`-named global outside any region
    /// the way `list_patches` reads one.
    name: String,
    /// The picture side: `source`, `offsets`, `palette` and
    /// `transparency`, the shared import parameters of
    /// `skill/reference/imports.md` without `name` and `replace`.
    #[serde(flatten)]
    picture: PatchSourceParams,
}

/// Arguments for [`Cyberdemon::remove_patch`]: one patch name and the two
/// flags deciding how far the removal reaches. No picture side: the bytes
/// leave the archive, nothing comes in.
#[derive(Debug, Deserialize, JsonSchema)]
struct RemovePatchParams {
    /// Name of the patch to remove: 1 to 8 printable `ASCII` characters,
    /// uppercased. Only the working archive is searched; `IWAD` and
    /// resource-layer lumps cannot be deleted.
    name: String,
    /// Also delete the name's `PNAMES` entry, renumbering every texture
    /// patch reference in the same undoable edit (default `false`: the
    /// list is left as it stands).
    #[serde(default)]
    remove_from_pnames: Option<bool>,
    /// Let a `PNAMES` removal drop texture patch references that still
    /// refer to the removed entry (default `false`: such a removal is a
    /// `conflict` and nothing is removed).
    #[serde(default)]
    force: Option<bool>,
}

impl AddPatchParams {
    /// The shared import parameters this call stands for: the picture
    /// fields with the required name pinned in and `replace` off.
    fn common(&self) -> ImageImportParams {
        self.picture.common(&self.name)
    }
}

impl ReplacePatchParams {
    /// The shared import parameters this call stands for: the picture
    /// fields with the required name pinned in. `replace` stays off here
    /// because this tool's write mode is decided from the working scan in
    /// [`write::located_patch`], not from a parameter.
    fn common(&self) -> ImageImportParams {
        self.picture.common(&self.name)
    }
}

impl PatchSourceParams {
    /// The [`ImageImportParams`] these picture fields stand for under the
    /// explicit lump name `name`, with `replace` off: both patch tools
    /// decide the overwrite themselves.
    fn common(&self, name: &str) -> ImageImportParams {
        ImageImportParams {
            source: self.source.clone(),
            name: Some(name.to_owned()),
            palette: self.palette,
            transparency: self.transparency.as_ref().map(copy_transparency),
            offsets: self.offsets.clone(),
            replace: false,
        }
    }
}

/// A copy of one transparency parameter set (the shared pipeline type
/// carries no `Clone`).
fn copy_transparency(params: &TransparencyParams) -> TransparencyParams {
    TransparencyParams {
        mode: params.mode.clone(),
        threshold: params.threshold,
        color: params.color.clone(),
    }
}

/// One staged patch: the bytes ready for insertion and the header facts
/// the result reports.
#[derive(Debug)]
struct Staged {
    /// The lump name to store under.
    name: LumpName,
    /// The `Doom` patch bytes to store.
    data: Vec<u8>,
    /// Whether the image pipeline produced the bytes (`true`) or the
    /// source already was a `Doom` patch (`false`).
    converted: bool,
    /// The stored patch's width in pixels.
    width: u32,
    /// The stored patch's height in pixels.
    height: u32,
    /// The horizontal drawing offset as stored.
    left_offset: i32,
    /// The vertical drawing offset as stored.
    top_offset: i32,
}

/// Structured result of [`Cyberdemon::add_patch`], and the flattened heart
/// of [`Cyberdemon::replace_patch`]'s: what stands in the working archive
/// now. The passthrough half has no quantization stats to add, so this
/// report is its own shape, not a flattened
/// [`crate::tools::imports::image_pipeline::ImportReport`].
#[derive(Serialize)]
struct PatchOutcome {
    /// The name the patch was stored under.
    name: LumpName,
    /// Whether a picture was converted (`true`) or the source bytes were
    /// already a `Doom` patch and stand unchanged (`false`).
    converted: bool,
    /// The stored patch's width in pixels.
    width: u32,
    /// The stored patch's height in pixels.
    height: u32,
    /// The horizontal drawing offset as stored.
    left_offset: i32,
    /// The vertical drawing offset as stored.
    top_offset: i32,
    /// Its `PNAMES` entry index, `null` when the name is not listed.
    pnames_index: Option<usize>,
}

/// Structured result of [`Cyberdemon::replace_patch`]: the stored patch's
/// [`PatchOutcome`] facts flattened together with where the write landed.
/// `PNAMES` is never touched by this tool, so `pnames_index` is the
/// name's unchanged entry.
#[derive(Serialize)]
struct ReplaceOutcome {
    /// The stored patch's facts, flattened into this report.
    #[serde(flatten)]
    patch: PatchOutcome,
    /// Whether the working archive gained a new `patches`-region override
    /// (`true`: only a lower layer held the name) or its own lump was
    /// overwritten in place (`false`).
    created_override: bool,
    /// The replaced lump's directory index in the working archive.
    lump_index: usize,
}

/// Structured result of [`Cyberdemon::remove_patch`]: what the one edit
/// took out of the working archive.
#[derive(Serialize)]
struct RemoveOutcome {
    /// The name the removal was asked for, uppercased.
    name: LumpName,
    /// Whether a working lump was deleted. `false` only when the name had
    /// a `PNAMES` entry but no working lump of its own - still a
    /// legitimate edit when `remove_from_pnames` was asked for.
    removed_lump: bool,
    /// Whether the name left the `PNAMES` list (`false` when
    /// `remove_from_pnames` was not asked for or the list did not hold
    /// the name; then no `PNAMES` write happened).
    removed_from_pnames: bool,
    /// Textures whose patch references the renumbering pruned or shifted;
    /// `0` without a `PNAMES` write.
    textures_changed: usize,
}

/// Whether fetched bytes are a `Doom` patch to pass through: the cheap
/// structural check of [`is_doom_patch`] plus a full [`decode_patch`], so
/// corrupt columns never land in the archive untouched.
fn looks_like_patch(bytes: &[u8]) -> bool {
    is_doom_patch(bytes) && decode_patch(bytes).is_ok()
}

/// Stage passthrough bytes: unchanged unless the call states an explicit
/// offset pair, which [`set_patch_offsets`] writes into the header.
///
/// # Errors
///
/// [`set_patch_offsets`]' [`CyberError::InvalidArgument`] for an offset
/// outside the `i16` range the header stores.
fn passthrough_staged(
    name: LumpName,
    bytes: &[u8],
    offsets: Option<OffsetMode>,
) -> Result<Staged, CyberError> {
    let data = match offsets {
        Some(OffsetMode::Explicit { left, top }) => set_patch_offsets(bytes, left, top)?,
        Some(
            OffsetMode::Zero | OffsetMode::Grab | OffsetMode::Center | OffsetMode::CenterBottom,
        )
        | None => bytes.to_vec(),
    };
    let header = patch_header(&data).ok_or_else(|| {
        CyberError::Internal(format!(
            "the staged bytes for {name} answer no patch header"
        ))
    })?;
    Ok(Staged {
        name,
        data,
        converted: false,
        width: u32::from(header.width),
        height: u32::from(header.height),
        left_offset: i32::from(header.left_offset),
        top_offset: i32::from(header.top_offset),
    })
}

/// Fetch the `source` before any session lock is taken and stage the
/// patch: the passthrough half when the bytes already are a `Doom` patch,
/// [`build_patch`] otherwise. The pipeline keeps its own `source` fetch,
/// exactly as the import tools run it today.
///
/// # Errors
///
/// [`AppState::fetch_source`]' errors for the `source`,
/// [`set_patch_offsets`]' range refusal, and whatever [`build_patch`]
/// reports for the picture and the palette.
async fn resolve_staged(
    state: &AppState,
    name: LumpName,
    import: &ParsedImport,
) -> Result<Staged, CyberError> {
    let fetched = state.fetch_source(&import.source).await?;
    if looks_like_patch(&fetched.bytes) {
        return passthrough_staged(name, &fetched.bytes, import.offsets);
    }
    let patched = build_patch(state, import, OffsetDefault::Zero).await?;
    let Patched {
        name: stored,
        data,
        report,
    } = patched;
    Ok(Staged {
        name: stored,
        data,
        converted: true,
        width: report.width,
        height: report.height,
        left_offset: report.left_offset,
        top_offset: report.top_offset,
    })
}

/// The full documentation of [`Cyberdemon::add_patch`].
const ADD_PATCH_TOOL_DESCRIPTION: &str = "Adds one patch lump to the working archive's patches region as a single undoable edit: name is required (nothing derives from the file), source bytes that already are a Doom patch store unchanged while any other picture runs the import pipeline, offsets default zero (a grAb chunk still wins for converted PNGs) and add_to_pnames (default true) also lists the name in PNAMES.";

/// The full documentation of [`Cyberdemon::replace_patch`].
const REPLACE_PATCH_TOOL_DESCRIPTION: &str = "Replaces the bytes of one patch lump the session already has - working, IWAD or resource layer - as a single undoable edit: a working copy is overwritten in place, a lower-layer one lands as a working override, and PNAMES never changes so TEXTURE references keep working. name resolves the way list_patches reads one; source rules are add_patch's.";

/// The full documentation of [`Cyberdemon::remove_patch`].
const REMOVE_PATCH_TOOL_DESCRIPTION: &str = "Removes one patch from the working archive as a single undoable edit (only working patches can go). remove_from_pnames (default false) also deletes the PNAMES entry and renumbers every TEXTURE1/TEXTURE2 reference in the SAME edit; a texture that still needs the entry refuses with conflict unless force: true accepts the dropped references.";

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = patches_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Adds one patch lump to the working archive's patches region.
    #[tool(description = ADD_PATCH_TOOL_DESCRIPTION)]
    async fn add_patch(
        &self,
        Parameters(args): Parameters<AddPatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match add_patch_run(self.state(), &args).await {
            Ok(outcome) => ok(&outcome),
            Err(error) => fail(&error),
        }
    }

    /// Replaces one patch lump the session already has.
    #[tool(description = REPLACE_PATCH_TOOL_DESCRIPTION)]
    async fn replace_patch(
        &self,
        Parameters(args): Parameters<ReplacePatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match replace_patch_run(self.state(), &args).await {
            Ok(outcome) => ok(&outcome),
            Err(error) => fail(&error),
        }
    }

    /// Removes one patch from the working archive, `PNAMES` entry included.
    #[tool(description = REMOVE_PATCH_TOOL_DESCRIPTION)]
    async fn remove_patch(
        &self,
        Parameters(args): Parameters<RemovePatchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        match remove_patch_run(self.state(), &args) {
            Ok(outcome) => ok(&outcome),
            Err(error) => fail(&error),
        }
    }
}

#[cfg(test)]
mod tests;
