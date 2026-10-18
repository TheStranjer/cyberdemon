//! Patch lifecycle tools: whole patch lumps in, over, and (as the group
//! grows) out again.
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
//! `remove_patch` will grow on the same plumbing.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::patch::{decode_patch, patch_header, set_patch_offsets};
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::state::AppState;
use crate::tools::fail;
use crate::tools::imports::image_pipeline::{
    ImageImportParams, LumpWrite, OffsetDefault, OffsetMode, OffsetsParam, ParsedImport, Patched,
    TransparencyParams, build_patch, parse_import, put_lump,
};
use crate::tools::imports::{PnamesPlan, current_pnames, pnames_plan};
use crate::tools::ok;
use crate::tools::textures::put_global;
use crate::wad::archive::Lump;
use crate::wad::classify::{LumpKind, Namespace, analyze, is_doom_patch};
use crate::wad::name::LumpName;

/// Raw directory bytes of the patch-name lump.
const PNAMES_RAW: [u8; 8] = *b"PNAMES\0\0";

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
    /// [`located_patch`], not from a parameter.
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

/// Whether the working archive's `patches` region already holds `name`.
///
/// # Errors
///
/// [`Session::lumps_in`]' errors (none for the working scope).
fn holds_patch(session: &Session, name: LumpName) -> Result<bool, CyberError> {
    let entries = session.lumps_in(Scope::Working)?;
    let lumps: Vec<Lump> = entries.iter().map(|entry| entry.lump.clone()).collect();
    let holds = analyze(&lumps)
        .lumps
        .iter()
        .any(|info| info.name == name && info.namespace == Some(Namespace::Patches));
    Ok(holds)
}

/// The duplicate refusal, before any work: `put_lump`' own hint says
/// `replace=true`, which this tool has no parameter for, so the message
/// points at `replace_patch` instead.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] for a working `patches`-region twin, and
/// [`Session::lumps_in`]' read errors.
fn refuse_working_twin(state: &AppState, name: LumpName) -> Result<(), CyberError> {
    if state.with_session(|session| holds_patch(session, name))? {
        return Err(CyberError::AlreadyExists(format!(
            "lump {name} already exists in the patches region of the working archive: use \
             replace_patch to overwrite it in place, or add the picture under another name"
        )));
    }
    Ok(())
}

/// The name's unchanged entry index in the merged `PNAMES` list, `None`
/// when no `PNAMES` answers or does not list it.
///
/// # Errors
///
/// [`current_pnames`]' [`CyberError::Format`] for a malformed lump.
fn pnames_position(session: &Session, name: LumpName) -> Result<Option<usize>, CyberError> {
    let text = name.to_string();
    Ok(current_pnames(session)?
        .iter()
        .position(|entry| entry.eq_ignore_ascii_case(&text)))
}

/// Whether the merged view holds a non-marker lump named `name` outside
/// the flats and sprites regions: `list_patches`' candidate rule for a
/// `PNAMES`-named global living outside the `patches` region. (A
/// `patches`-region occurrence also matches here, which is harmless: this
/// only ever decides the fallback after the region lookup came up empty.)
fn holds_global_patch(session: &Session, name: LumpName) -> bool {
    let view = session.merged();
    let lumps: Vec<Lump> = view.iter().map(|entry| entry.lump.clone()).collect();
    analyze(&lumps).lumps.iter().any(|info| {
        info.name == name
            && info.kind != LumpKind::Marker
            && !matches!(info.namespace, Some(Namespace::Flats | Namespace::Sprites))
    })
}

/// What the merged view stands on for a `replace_patch` target, decided
/// before any work: the name must stand as a patch the way `list_patches`
/// reads one - inside a `patches` region, or as a `PNAMES`-named global -
/// and the answer carries how the write must land (overwrite in place, or
/// override a lower layer) with the name's unchanged `PNAMES` index.
///
/// # Errors
///
/// [`current_pnames`]' [`CyberError::Format`] for a malformed `PNAMES` and
/// [`CyberError::NotFound`] naming `add_patch` for a name no layer holds
/// as a patch.
fn located_patch(session: &Session, name: LumpName) -> Result<(bool, Option<usize>), CyberError> {
    let pnames_index = pnames_position(session, name)?;
    let in_region = session.find(name, Some(Namespace::Patches)).is_some();
    let stands = in_region || (pnames_index.is_some() && holds_global_patch(session, name));
    if !stands {
        return Err(CyberError::NotFound(format!(
            "no patch named {name} in the merged view: pass a name list_patches shows, or \
             store a new one with add_patch"
        )));
    }
    holds_patch(session, name).map(|in_working| (in_working, pnames_index))
}

/// The synchronous half of an `add_patch`: one undoable edit placing the
/// staged lump in the working `patches` region and, when the effective
/// `PNAMES` lacked the name, writing the grown list as a working
/// copy-on-write - the same shape as the import tools' commit step. The
/// `put_lump` call carries `replace: false` as the final twin guard behind
/// [`refuse_working_twin`].
///
/// # Errors
///
/// [`put_lump`]' placement and `already_exists` errors and
/// [`put_global`]' stale-index [`CyberError::Internal`].
fn commit_patch(
    session: &mut Session,
    staged: Staged,
    plan: Option<PnamesPlan>,
) -> Result<PatchOutcome, CyberError> {
    let label = format!("add_patch {}", staged.name);
    session.edit(&label, move |wad| {
        let write = LumpWrite {
            name: staged.name,
            data: &staged.data,
            namespace: Some(Namespace::Patches),
            replace: false,
        };
        put_lump(wad, &write)?;
        if let Some(list) = plan.as_ref().and_then(|entry| entry.bytes.as_ref()) {
            put_global(wad, LumpName::from_raw(PNAMES_RAW), list.clone())?;
        }
        Ok(PatchOutcome {
            name: staged.name,
            converted: staged.converted,
            width: staged.width,
            height: staged.height,
            left_offset: staged.left_offset,
            top_offset: staged.top_offset,
            pnames_index: plan.map(|entry| entry.index),
        })
    })
}

/// `add_patch`' work: validate, refuse a working twin before any fetch,
/// stage the bytes, plan the `PNAMES` entry, then commit patch and list as
/// one undoable edit.
///
/// # Errors
///
/// [`parse_import`]' argument errors, [`refuse_working_twin`]'
/// [`CyberError::AlreadyExists`], whatever [`resolve_staged`] reports for
/// the source and picture, and [`pnames_plan`]' and [`commit_patch`]'
/// errors; a refusal leaves the working archive untouched.
async fn add_patch_run(
    state: &AppState,
    args: &AddPatchParams,
) -> Result<PatchOutcome, CyberError> {
    let import = parse_import(&args.common())?;
    let Some(name) = import.name else {
        return Err(CyberError::Internal(
            "add_patch always carries its lump name".to_owned(),
        ));
    };
    refuse_working_twin(state, name)?;
    let staged = resolve_staged(state, name, &import).await?;
    let plan = pnames_plan(state, staged.name, args.add_to_pnames)?;
    state.with_session(move |session| commit_patch(session, staged, plan))
}

/// The synchronous half of a `replace_patch`: one undoable edit writing the
/// staged bytes over the named patch. `in_working` is the decision made
/// before the edit from the read-only working scan: `true` overwrites the
/// working `patches`-region lump in place at its own index, `false` lets
/// [`put_lump`] insert the working override (creating the region's markers
/// on the working archive when it has none). `PNAMES` is never written.
///
/// # Errors
///
/// [`put_lump`]' placement and stale-index errors - the `already_exists`
/// half is unreachable here because the scan chose `replace` to fit the
/// directory.
fn commit_replacement(
    session: &mut Session,
    staged: Staged,
    pnames_index: Option<usize>,
    in_working: bool,
) -> Result<ReplaceOutcome, CyberError> {
    let label = format!("replace_patch {}", staged.name);
    session.edit(&label, move |wad| {
        let write = LumpWrite {
            name: staged.name,
            data: &staged.data,
            namespace: Some(Namespace::Patches),
            replace: in_working,
        };
        let lump_index = put_lump(wad, &write)?;
        Ok(ReplaceOutcome {
            patch: PatchOutcome {
                name: staged.name,
                converted: staged.converted,
                width: staged.width,
                height: staged.height,
                left_offset: staged.left_offset,
                top_offset: staged.top_offset,
                pnames_index,
            },
            created_override: !in_working,
            lump_index,
        })
    })
}

/// `replace_patch`' work: validate, locate the patch in the merged view
/// and decide the write from a read-only working scan before any fetch,
/// stage the bytes on the shared `add_patch` plumbing, then commit the one
/// edit - no `PNAMES` write.
///
/// # Errors
///
/// [`parse_import`]' argument errors, [`located_patch`]'
/// [`CyberError::NotFound`] and `format_error`, whatever [`resolve_staged`]
/// reports for the source and picture, and [`commit_replacement`]' errors;
/// a refusal leaves the working archive untouched.
async fn replace_patch_run(
    state: &AppState,
    args: &ReplacePatchParams,
) -> Result<ReplaceOutcome, CyberError> {
    let import = parse_import(&args.common())?;
    let Some(name) = import.name else {
        return Err(CyberError::Internal(
            "replace_patch always carries its lump name".to_owned(),
        ));
    };
    let (in_working, pnames_index) = state.with_session(|session| located_patch(session, name))?;
    let staged = resolve_staged(state, name, &import).await?;
    state.with_session(move |session| commit_replacement(session, staged, pnames_index, in_working))
}

/// The full documentation of [`Cyberdemon::add_patch`].
const ADD_PATCH_TOOL_DESCRIPTION: &str = "Adds one patch lump to the working archive's patches region (P_START/P_END markers created there when the working archive has none) as a single undoable edit labelled \"add_patch NAME\" (save_wad writes the result; undo steps it back, redo replays it). name (required: 1 to 8 printable ASCII characters, uppercased) is the lump name - nothing is derived from the file. source: absolute path, file:// URI, http(s):// URL or this server's asset/upload URL. Passthrough rule: source bytes that already are a Doom patch (a valid header and column table, and they decode) are stored unchanged - converted reports false and the column bodies stay byte-identical - while any other source runs the import_image_as_patch pipeline (decode, quantize to the session's PLAYPAL, encode the classic column/post format), reported as converted: true. palette (default 0), transparency ({mode: \"alpha\"} with threshold default 128, {mode: \"color\"} with color default \"#00FFFF\", {mode: \"none\"}) and offsets are the import_image_as_patch parameters (see skill/reference/imports.md) and act on the converted half only. offsets default to \"zero\" here: a converted PNG's grAb chunk still wins over that default, while on a passthrough patch only an explicit {\"x\": n, \"y\": n} pair rewrites the two header fields - presets and an omitted offsets leave the stored header as it is - and either way an offset outside the i16 range a patch header stores is invalid_argument. add_to_pnames (default true) also lists the name in PNAMES, copy-on-write: an already-listed name keeps its index and nothing is rewritten. Errors: invalid_argument (a bad name or offsets preset or transparency mode, an offset outside the i16 range), not_found (missing source; a missing PLAYPAL or a palette index it lacks on the converted half), format_error (bytes that are neither a Doom patch nor a readable image), already_exists when the working archive's patches region already holds this name - add_patch never overwrites, that is replace_patch's job. Returns {name, converted, width, height, left_offset, top_offset, pnames_index}: the dimensions and offsets are the stored patch header's, pnames_index is the PNAMES entry or null when the name is not listed. list_patches shows the region's lumps, get_patch publishes this one back, list_pnames shows the name list, remove_patch deletes a patch, and import_image_as_patch converts a picture under a name derived from the file with a replace=true option.";

/// The full documentation of [`Cyberdemon::replace_patch`].
const REPLACE_PATCH_TOOL_DESCRIPTION: &str = "Replaces the bytes of one patch lump the session already has, whether the working archive owns it or it comes from the IWAD or a resource layer, as a single undoable edit labelled \"replace_patch NAME\" (save_wad writes the result; undo steps it back, redo replays it). name (required: 1 to 8 printable ASCII characters, uppercased) is resolved against the merged view the way list_patches reads one: the last lump of that name inside a P_START region, or, when PNAMES lists the name, an occurrence living outside the flats and sprites regions as well. Nothing answers is not_found and points at add_patch, the way in for a patch that does not exist yet. source, palette, transparency and offsets are add_patch's shared picture parameters unchanged (see skill/reference/patches.md): source bytes that already are a Doom patch are stored as they are - converted reports false, only an explicit {\"x\": n, \"y\": n} pair rewrites the two header fields, and an offset outside the i16 range a patch header stores is invalid_argument - while any other picture runs the import_image_as_patch pipeline, reported as converted: true; a missing source or PLAYPAL trouble is not_found and bytes that are neither a Doom patch nor a readable image are format_error. The write is decided before the edit: the working archive's patches region already holds the name, so the lump is overwritten in place at its own directory index (created_override: false); only a lower layer holds it, so the bytes land as a working override in the patches region, whose P_START/P_END markers are created there when the working archive has none (created_override: true) - the lower layer is never touched. PNAMES is never written: a listed name keeps its index, so TEXTURE1/TEXTURE2 patch references keep working and list_pnames answers unchanged. Returns add_patch's facts (name, converted, width, height, left_offset, top_offset) plus the unchanged pnames_index (null for an unlisted name), created_override, and lump_index, the replaced lump's position in the working archive. Check the swap with get_patch or list_patches (the row shows source: \"working\"); undoing a created override hands the merged view back to the lower layer.";

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
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AddPatchParams, PatchSourceParams, ReplacePatchParams, holds_patch, located_patch,
        looks_like_patch, passthrough_staged,
    };
    use crate::error::CyberError;
    use crate::gfx::image::IndexedImage;
    use crate::gfx::patch::{Patch, encode_patch};
    use crate::session::Session;
    use crate::tools::imports::image_pipeline::{
        DEFAULT_PALETTE, OffsetMode, OffsetsParam, TransparencyParams, parse_import,
    };
    use crate::wad::archive::Lump;
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

    /// The bytes of a 2x3 patch with header offsets `(-3, -2)`.
    fn tiny_patch() -> Vec<u8> {
        let image = IndexedImage::new(2_u32, 3_u32, vec![1_u8; 6_usize], vec![true; 6_usize])
            .expect("2x3 is a valid shape");
        let patch = Patch {
            left_offset: -3_i16,
            top_offset: -2_i16,
            image,
        };
        encode_patch(&patch).expect("a 2x3 patch encodes")
    }

    /// A header-classified patch whose single column is a truncated post:
    /// [`crate::wad::classify::is_doom_patch`] accepts it, decoding does
    /// not.
    fn corrupt_patch() -> Vec<u8> {
        vec![1_u8, 0, 1, 0, 0, 0, 0, 0, 12, 0, 0, 0, 0, 5]
    }

    /// A session whose working archive holds the lumps `names`, markers
    /// included.
    fn session_with(names: &[&str]) -> Session {
        let mut session = Session::default();
        let lumps: Vec<Lump> = names.iter().copied().map(region_lump).collect();
        session
            .edit("seed", move |wad| {
                wad.lumps.extend(lumps);
                Ok(())
            })
            .expect("plain append");
        session
    }

    /// The seed lump for one `text`: a marker for the region markers, a
    /// decodable patch for everything else.
    fn region_lump(text: &str) -> Lump {
        if text.starts_with("P_") || text.starts_with("F_") {
            return Lump::marker(name(text));
        }
        Lump::new(name(text), tiny_patch())
    }

    /// Arguments over a required name and one source path.
    fn args_for(lump: &str, source: &str) -> AddPatchParams {
        AddPatchParams {
            name: lump.to_owned(),
            picture: PatchSourceParams {
                source: source.to_owned(),
                offsets: None,
                palette: None,
                transparency: None,
            },
            add_to_pnames: None,
        }
    }

    #[test]
    fn the_wire_form_flattens_into_the_shared_import_parameters() {
        let args: AddPatchParams = serde_json::from_value(json!({
            "name": "myart",
            "source": "/x/y.png",
            "offsets": {"x": 3_i64, "y": -4_i64},
            "transparency": {"mode": "color", "color": "#FF0000"},
            "add_to_pnames": false,
        }))
        .expect("the wire form");
        assert_eq!(args.name, "myart", "the name stays as typed");
        assert_eq!(args.add_to_pnames, Some(false), "the flag carries");
        let import = parse_import(&args.common()).expect("valid");
        assert_eq!(
            import.name,
            Some(name("MYART")),
            "uppercased like any lump name"
        );
        assert_eq!(import.source, "/x/y.png", "the source carries");
        assert!(!import.replace, "add_patch never overwrites");
        assert_eq!(
            import.offsets,
            Some(OffsetMode::Explicit {
                left: 3_i32,
                top: -4_i32
            })
        );
    }

    #[test]
    fn omitted_picture_options_take_the_pipeline_defaults() {
        let args = args_for("X", "/x/y.png");
        let common = args.common();
        assert_eq!(common.name.as_deref(), Some("X"), "the name pins in");
        assert!(!common.replace, "no overwrite parameter exists");
        let import = parse_import(&common).expect("valid");
        assert!(import.offsets.is_none(), "omitted means grab-then-zero");
        assert_eq!(import.palette, DEFAULT_PALETTE, "the normal palette");
        assert!(
            args.add_to_pnames.is_none(),
            "pnames_plan defaults the flag on"
        );
    }

    #[test]
    fn a_bad_name_preset_or_mode_is_refused_before_any_work() {
        let long = args_for("TOOLONGNM", "/x/y.png");
        let name_error = parse_import(&long.common()).expect_err("nine characters");
        assert_eq!(code(&name_error), "invalid_argument");
        assert!(
            name_error.to_string().contains("name"),
            "the message names the parameter: {name_error}"
        );
        let mut preset = args_for("X", "/x/y.png");
        preset.picture.offsets = Some(OffsetsParam::Preset("middle".to_owned()));
        let preset_error = parse_import(&preset.common()).expect_err("not a preset");
        assert_eq!(code(&preset_error), "invalid_argument");
        assert!(
            preset_error.to_string().contains("center_bottom"),
            "the pipeline's message: {preset_error}"
        );
        let mut keyed = args_for("X", "/x/y.png");
        keyed.picture.transparency = Some(TransparencyParams {
            mode: Some("blur".to_owned()),
            threshold: None,
            color: None,
        });
        assert_eq!(
            code(&parse_import(&keyed.common()).expect_err("not a mode")),
            "invalid_argument"
        );
    }

    #[test]
    fn holds_patch_reads_the_working_patches_region_only() {
        let region = session_with(&["P_START", "MYART", "P_END"]);
        assert!(
            holds_patch(&region, name("MYART")).expect("the scan runs"),
            "a region twin is held"
        );
        let loose = session_with(&["MYART"]);
        assert!(
            !holds_patch(&loose, name("MYART")).expect("the scan runs"),
            "a loose lump is not a region twin"
        );
        let flats = session_with(&["F_START", "MYART", "F_END"]);
        assert!(
            !holds_patch(&flats, name("MYART")).expect("the scan runs"),
            "another region is no twin either"
        );
        assert!(
            !holds_patch(&Session::default(), name("MYART")).expect("the scan runs"),
            "a fresh archive holds nothing"
        );
    }

    #[test]
    fn only_decodable_doom_patch_bytes_pass_through() {
        assert!(
            looks_like_patch(&tiny_patch()),
            "a real patch passes through"
        );
        assert!(
            !looks_like_patch(b"this is a text file, not an image"),
            "junk runs the pipeline"
        );
        assert!(
            !looks_like_patch(&corrupt_patch()),
            "a header that will not decode runs the pipeline too"
        );
    }

    #[test]
    fn passthrough_keeps_the_bytes_unless_the_offsets_are_explicit() {
        let bytes = tiny_patch();
        let kept = passthrough_staged(name("KEPT"), &bytes, None).expect("a real patch stages");
        assert_eq!(kept.data, bytes, "stored unchanged");
        assert!(!kept.converted, "the passthrough half ran");
        assert_eq!(
            (kept.width, kept.height),
            (2_u32, 3_u32),
            "the header's own shape"
        );
        assert_eq!(
            (kept.left_offset, kept.top_offset),
            (-3_i32, -2_i32),
            "the header's own offsets"
        );
        let centered =
            passthrough_staged(name("KEPT"), &bytes, Some(OffsetMode::Center)).expect("stages");
        assert_eq!(centered.data, bytes, "a preset never rewrites");
        let explicit = Some(OffsetMode::Explicit {
            left: 4_i32,
            top: -6_i32,
        });
        let moved =
            passthrough_staged(name("MOVED"), &bytes, explicit).expect("an explicit pair stages");
        assert_eq!(
            (moved.left_offset, moved.top_offset),
            (4_i32, -6_i32),
            "the rewritten pair reports itself"
        );
        assert_eq!(
            moved.data.get(..4),
            bytes.get(..4),
            "the header start survives"
        );
        assert_eq!(moved.data.get(8..), bytes.get(8..), "so does the payload");
        let far = passthrough_staged(
            name("FAR"),
            &bytes,
            Some(OffsetMode::Explicit {
                left: 40000_i32,
                top: 0_i32,
            }),
        )
        .expect_err("past the i16 range");
        assert_eq!(code(&far), "invalid_argument", "the range code");
        assert!(
            far.to_string().contains("does not fit"),
            "the pipeline's message: {far}"
        );
    }

    /// The seed helper for [`session_listed`]: a `PNAMES` lump instead of
    /// a patch when the text is the list's own name.
    fn listed_lump(text: &str, listed: &[&str]) -> Lump {
        if text != "PNAMES" {
            return region_lump(text);
        }
        let texts: Vec<String> = listed.iter().map(|entry| (*entry).to_owned()).collect();
        Lump::new(
            name("PNAMES"),
            write_pnames(&texts).expect("test names store"),
        )
    }

    /// A session whose working archive holds the lumps `names` (markers
    /// included), with a `PNAMES` lump listing `listed` among them.
    fn session_listed(names: &[&str], listed: &[&str]) -> Session {
        let mut session = Session::default();
        let lumps: Vec<Lump> = names
            .iter()
            .copied()
            .map(|text| listed_lump(text, listed))
            .collect();
        session
            .edit("seed", move |wad| {
                wad.lumps.extend(lumps);
                Ok(())
            })
            .expect("plain append");
        session
    }

    #[test]
    fn located_patch_decides_the_write_from_the_merged_view() {
        let owned = session_listed(&["P_START", "MYART", "P_END", "PNAMES"], &["MYART"]);
        assert_eq!(
            located_patch(&owned, name("MYART")).expect("a region patch"),
            (true, Some(0_usize)),
            "the working archive owns it: in-place overwrite at the unchanged index"
        );
        let unlisted = session_listed(&["P_START", "MYART", "P_END"], &[]);
        assert_eq!(
            located_patch(&unlisted, name("MYART")).expect("a region patch"),
            (true, None),
            "the region copy is working-owned and an unlisted name answers no index"
        );
        let global = session_listed(&["MYART", "PNAMES"], &["MYART"]);
        assert_eq!(
            located_patch(&global, name("MYART")).expect("a PNAMES global"),
            (false, Some(0_usize)),
            "no region copy: the write inserts an override at the global's index"
        );
    }

    #[test]
    fn an_unlocated_replace_target_refuses_naming_add_patch() {
        let empty = Session::default();
        let error = located_patch(&empty, name("NOPE")).expect_err("nothing stands");
        assert_eq!(code(&error), "not_found", "the documented code");
        assert!(
            error.to_string().contains("add_patch"),
            "the message names the way in: {error}"
        );
        let flats = session_listed(&["F_START", "MYART", "F_END", "PNAMES"], &["MYART"]);
        assert_eq!(
            code(&located_patch(&flats, name("MYART")).expect_err("a flat is no patch")),
            "not_found",
            "the flats and sprites regions never make a candidate"
        );
    }

    #[test]
    fn the_replace_wire_form_flattens_the_picture_without_other_flags() {
        let args: ReplacePatchParams = serde_json::from_value(json!({
            "name": "wallp1",
            "source": "/x/y.png",
            "offsets": "grab",
            "palette": 3_usize,
        }))
        .expect("the wire form");
        assert_eq!(args.name, "wallp1", "the name stays as typed");
        let import = parse_import(&args.common()).expect("valid");
        assert_eq!(
            import.name,
            Some(name("WALLP1")),
            "uppercased like any lump name"
        );
        assert_eq!(import.source, "/x/y.png", "the source carries");
        assert_eq!(import.palette, 3_usize, "the palette carries");
        assert_eq!(import.offsets, Some(OffsetMode::Grab), "the preset carries");
        assert!(!import.replace, "the tool decides the write itself");
    }
}
