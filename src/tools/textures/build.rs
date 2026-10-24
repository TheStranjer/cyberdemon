//! The two single-texture tools: `build_texture` defines one composite
//! record, `remove_texture` undefines it.
//!
//! [`build_texture_run`] validates the [`BuildTextureParams`] request
//! ([`checked_name`], [`checked_size`], [`build_patches`], [`target_lump`],
//! [`check_patch_lumps`]), grows `PNAMES` with [`missing_names`], resolves
//! every [`Placement`] into a [`crate::wad::textures::TextureDef`] through
//! [`definition`] and writes it with [`place_definition`];
//! [`remove_texture_run`] strips the name from both texture lumps. Each
//! commits its [`LumpRewrite`] rows as one edit. [`target_lump`],
//! [`lump_copy`], [`missing_names`], [`definition`], [`place_definition`],
//! [`Placement`] and [`checked_name`] are the same steps the
//! `create_texture_from_image` import runs, kept at the
//! [`crate::tools::textures`] paths its group imports.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::Session;
use crate::wad::name::LumpName;
use crate::wad::textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, write_pnames, write_texture_lump,
};

use super::list::present_lump_names;
use super::pnames::{position_of, usize_to_u16};
use super::{MAX_TEXTURE_SIZE, PNAMES_RAW, TextureSet, WhichLump, put_global};

/// One patch placement of [`BuildTextureParams::patches`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct PatchPlacement {
    /// Patch name: 1 to 8 printable `ASCII` characters, stored uppercase;
    /// appended to `PNAMES` when it is not listed yet.
    pub(super) patch: String,
    /// Horizontal placement in pixels from the texture's left edge.
    pub(super) x: i16,
    /// Vertical placement in pixels from the texture's top edge.
    pub(super) y: i16,
}

/// One validated patch placement: the parsed patch name and its origin.
pub(in crate::tools) struct Placement {
    /// The patch name, uppercased through [`LumpName::parse`].
    pub(in crate::tools) name: String,
    /// Horizontal placement in pixels from the texture's left edge.
    pub(in crate::tools) x: i16,
    /// Vertical placement in pixels from the texture's top edge.
    pub(in crate::tools) y: i16,
}

/// One texture lump one edit rewrites: the raw directory name and the new
/// bytes.
type LumpRewrite = ([u8; 8], Vec<u8>);

/// Arguments for [`Cyberdemon::build_texture`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct BuildTextureParams {
    /// The new texture's name: 1 to 8 printable `ASCII` characters,
    /// stored uppercase.
    pub(super) name: String,
    /// Texture width in pixels (1 to 4096). A width that is not a power
    /// of two builds but is reported in `warnings`: vanilla tiling lines
    /// up best at powers of two.
    pub(super) width: u16,
    /// Texture height in pixels (1 to 4096).
    pub(super) height: u16,
    /// The patch placements making up the texture, drawn in the order
    /// given; at least one.
    pub(super) patches: Vec<PatchPlacement>,
    /// Which lump defines the texture: `"TEXTURE1"` or `"TEXTURE2"`,
    /// case-insensitive. Unset means the lump that already defines
    /// `name`, else `TEXTURE1`.
    #[serde(default)]
    pub(super) target: Option<String>,
    /// Update an existing definition of this name in the target lump in
    /// place instead of failing (default false: a taken name is
    /// `already_exists`).
    #[serde(default)]
    pub(super) replace: Option<bool>,
    /// Accept patch names no lump in the merged view carries (default
    /// false: they fail with `not_found`).
    #[serde(default)]
    pub(super) allow_missing_patches: Option<bool>,
}

/// Arguments for [`Cyberdemon::remove_texture`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RemoveTextureParams {
    /// The texture to undefine: 1 to 8 printable `ASCII` characters,
    /// matched case-insensitively.
    pub(super) name: String,
}

/// Structured result of [`Cyberdemon::build_texture`].
#[derive(Debug, Serialize)]
pub(super) struct BuildReport {
    /// The texture name, uppercased.
    pub(super) name: String,
    /// The lump the definition was written to: `"TEXTURE1"` or `"TEXTURE2"`.
    pub(super) lump: &'static str,
    /// Whether the working archive gained the target lump (it was absent,
    /// or held only by a read-only layer that was copied on write).
    pub(super) created: bool,
    /// Patch names this call appended to `PNAMES`.
    pub(super) pnames_added: Vec<String>,
    /// Non-fatal notes (e.g. a width that is not a power of two).
    pub(super) warnings: Vec<String>,
}

/// Structured result of [`Cyberdemon::remove_texture`].
#[derive(Debug, Serialize)]
pub(super) struct RemoveReport {
    /// The texture name, uppercased.
    pub(super) name: String,
    /// The lump that held the winning definition.
    pub(super) lump: &'static str,
}

/// Parses one texture-tool name through [`LumpName::parse`], echoing
/// `what` in the refusal.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected text.
pub(in crate::tools) fn checked_name(what: &str, text: &str) -> Result<String, CyberError> {
    LumpName::parse(text)
        .map(|name| name.to_string())
        .map_err(|error| {
            CyberError::InvalidArgument(format!("{what} name {text:?} is not valid: {error}"))
        })
}

/// Refuses a texture dimension outside `1..=4096`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the dimension and the range.
fn checked_size(what: &str, value: u16) -> Result<(), CyberError> {
    if value == 0 || value > MAX_TEXTURE_SIZE {
        return Err(CyberError::InvalidArgument(format!(
            "{what} {value} is out of range: pass a value between 1 and {MAX_TEXTURE_SIZE}"
        )));
    }
    Ok(())
}

/// The placements of one build, patch names parsed and uppercased.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty list or a bad patch name.
fn build_patches(args: &BuildTextureParams) -> Result<Vec<Placement>, CyberError> {
    if args.patches.is_empty() {
        return Err(CyberError::InvalidArgument(
            "build_texture needs at least one patch placement: pass patches as a non-empty \
             list of {patch, x, y}"
                .to_owned(),
        ));
    }
    args.patches
        .iter()
        .map(|placement| {
            Ok(Placement {
                name: checked_name("patch", &placement.patch)?,
                x: placement.x,
                y: placement.y,
            })
        })
        .collect()
}

/// The lump one build writes: the explicit `target` argument, else the
/// lump that already defines `name`, else `TEXTURE1`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a `target` outside `TEXTURE1` and
/// `TEXTURE2`.
pub(in crate::tools) fn target_lump(
    set: &TextureSet,
    target: Option<&str>,
    name: &str,
) -> Result<WhichLump, CyberError> {
    let Some(text) = target else {
        return Ok(set
            .find(name)
            .map_or(WhichLump::Texture1, |(_def, which)| which));
    };
    if text.eq_ignore_ascii_case(WhichLump::Texture1.as_str()) {
        return Ok(WhichLump::Texture1);
    }
    if text.eq_ignore_ascii_case(WhichLump::Texture2.as_str()) {
        return Ok(WhichLump::Texture2);
    }
    Err(CyberError::InvalidArgument(format!(
        "target {text:?} is not a texture lump: pass \"TEXTURE1\" or \"TEXTURE2\", or leave \
         target out to use the lump that already defines {name}"
    )))
}

/// The merged copy of one texture lump, or a fresh empty `Doom`-layout
/// list when no archive holds it.
pub(in crate::tools) fn lump_copy(set: &TextureSet, target: WhichLump) -> TextureLump {
    target.decoded(set).map_or_else(
        || TextureLump {
            layout: TextureLayout::Doom,
            textures: Vec::new(),
        },
        |held| held.0.clone(),
    )
}

/// Refuses patch names that no lump in the merged view (outside the flats
/// and sprites namespaces) carries.
///
/// # Errors
///
/// [`present_lump_names`]' errors and [`CyberError::NotFound`] listing the
/// missing patches.
fn check_patch_lumps(session: &Session, placements: &[Placement]) -> Result<(), CyberError> {
    let present = present_lump_names(session)?;
    let mut missing: Vec<String> = Vec::new();
    for placement in placements {
        let found = LumpName::parse(&placement.name).is_ok_and(|parsed| present.contains(&parsed));
        if !found && !missing.contains(&placement.name) {
            missing.push(placement.name.clone());
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(CyberError::NotFound(format!(
        "no lump in the merged view carries {}: add the patch lumps (import_image_as_patch, or \
         add_lump into the patches namespace) or pass allow_missing_patches=true to build \
         anyway",
        missing.join(", ")
    )))
}

/// The placement patch names `PNAMES` does not list yet, in first-use
/// order.
pub(in crate::tools) fn missing_names(pnames: &[String], placements: &[Placement]) -> Vec<String> {
    let mut added: Vec<String> = Vec::new();
    for placement in placements {
        if position_of(pnames, &placement.name).is_none() && !added.contains(&placement.name) {
            added.push(placement.name.clone());
        }
    }
    added
}

/// The definition one build writes: every placement resolved to its index
/// in the new `PNAMES` list.
///
/// # Errors
///
/// [`usize_to_u16`]'s [`CyberError::LimitExceeded`] for an index past the
/// `u16` range and [`CyberError::Internal`] for a vanished patch name.
pub(in crate::tools) fn definition(
    name: &str,
    size: (u16, u16),
    placements: &[Placement],
    pnames: &[String],
) -> Result<TextureDef, CyberError> {
    let mut patches = Vec::with_capacity(placements.len());
    for placement in placements {
        let at = position_of(pnames, &placement.name).ok_or_else(|| {
            CyberError::Internal(format!(
                "patch {} vanished from the planned PNAMES list",
                placement.name
            ))
        })?;
        patches.push(PatchRef {
            patch: usize_to_u16(at)?,
            x: placement.x,
            y: placement.y,
        });
    }
    Ok(TextureDef {
        name: name.to_owned(),
        flags_raw: 0,
        width: size.0,
        height: size.1,
        patches,
    })
}

/// Writes the definition into its target lump: a same-name record is
/// updated in place (position and raw flags kept) when `replace` was
/// given, a taken name without it fails, and a free name grows the list.
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] for a name the lump holds without
/// `replace`.
pub(in crate::tools) fn place_definition(
    lump: &mut TextureLump,
    target: WhichLump,
    def: TextureDef,
    replace: bool,
) -> Result<(), CyberError> {
    let at = lump
        .textures
        .iter()
        .position(|existing| existing.name.eq_ignore_ascii_case(&def.name));
    let Some(index) = at else {
        lump.textures.push(def);
        return Ok(());
    };
    if !replace {
        return Err(CyberError::AlreadyExists(format!(
            "{} is already defined in {}: pass replace=true to update it in place, or \
             target the other lump to shadow it",
            def.name,
            target.as_str()
        )));
    }
    if let Some(slot) = lump.textures.get_mut(index) {
        slot.width = def.width;
        slot.height = def.height;
        slot.patches = def.patches;
    }
    Ok(())
}

/// The warning for a width vanilla engines cannot tile in step.
fn tiling_warning(width: u16) -> Vec<String> {
    if width.is_power_of_two() {
        return Vec::new();
    }
    vec![format!(
        "width {width} is not a power of two: vanilla texture tiling repeats the texture by its \
         width along the wall, which lines up best at powers of two"
    )]
}

/// `build_texture`'s work: validate, extend `PNAMES`, then commit
/// `PNAMES` and the target lump as one edit.
///
/// # Errors
///
/// [`checked_name`], [`checked_size`], [`build_patches`] and
/// [`target_lump`]'s [`CyberError::InvalidArgument`],
/// [`check_patch_lumps`]'s [`CyberError::NotFound`],
/// [`place_definition`]'s [`CyberError::AlreadyExists`],
/// [`TextureSet::load`]'s [`CyberError::Format`] and the codecs' errors.
pub(super) fn build_texture_run(
    session: &mut Session,
    args: &BuildTextureParams,
) -> Result<BuildReport, CyberError> {
    let name = checked_name("texture", &args.name)?;
    checked_size("width", args.width)?;
    checked_size("height", args.height)?;
    let placements = build_patches(args)?;
    let set = TextureSet::load(session)?;
    let target = target_lump(&set, args.target.as_deref(), &name)?;
    if !args.allow_missing_patches.unwrap_or(false) {
        check_patch_lumps(session, &placements)?;
    }
    let pnames_added = missing_names(&set.pnames, &placements);
    let mut pnames = set.pnames.clone();
    pnames.extend(pnames_added.iter().cloned());
    let def = definition(&name, (args.width, args.height), &placements, &pnames)?;
    let mut lump = lump_copy(&set, target);
    place_definition(&mut lump, target, def, args.replace.unwrap_or(false))?;
    let lump_bytes = write_texture_lump(&lump)?;
    let pnames_bytes = (!pnames_added.is_empty())
        .then(|| write_pnames(&pnames))
        .transpose()?;
    let created = session
        .working()
        .find_last(LumpName::from_raw(target.as_raw()))
        .is_none();
    let label = format!("build_texture {name}");
    session.edit(&label, |wad| {
        if let Some(bytes) = pnames_bytes.as_ref() {
            put_global(wad, LumpName::from_raw(PNAMES_RAW), bytes.clone())?;
        }
        put_global(wad, LumpName::from_raw(target.as_raw()), lump_bytes)?;
        Ok(())
    })?;
    Ok(BuildReport {
        name,
        lump: target.as_str(),
        created,
        pnames_added,
        warnings: tiling_warning(args.width),
    })
}

/// `remove_texture`'s work: strip the definition from every texture lump
/// of the merged view that holds it and commit the rewrites as one edit.
///
/// # Errors
///
/// [`checked_name`]'s [`CyberError::InvalidArgument`],
/// [`CyberError::NotFound`] for a name no texture lump defines,
/// [`TextureSet::load`]'s [`CyberError::Format`] and
/// [`write_texture_lump`]'s errors.
pub(super) fn remove_texture_run(
    session: &mut Session,
    args: &RemoveTextureParams,
) -> Result<RemoveReport, CyberError> {
    let name = checked_name("texture", &args.name)?;
    let set = TextureSet::load(session)?;
    let (_def, winner) = set.find(&name).ok_or_else(|| {
        CyberError::NotFound(format!(
            "no TEXTURE1 or TEXTURE2 definition named {name}: list_textures shows the names"
        ))
        .with_hint(
            "list_textures / list_flats with a glob pattern; or create one with \
                    create_texture_from_image / create_flat_from_image",
        )
    })?;
    let mut rewrites: Vec<LumpRewrite> = Vec::new();
    for which in [WhichLump::Texture1, WhichLump::Texture2] {
        let mut lump = lump_copy(&set, which);
        let before = lump.textures.len();
        lump.textures
            .retain(|existing| !existing.name.eq_ignore_ascii_case(&name));
        if lump.textures.len() == before {
            continue;
        }
        rewrites.push((which.as_raw(), write_texture_lump(&lump)?));
    }
    let label = format!("remove_texture {name}");
    session.edit(&label, |wad| {
        for rewrite in rewrites.as_slice() {
            put_global(wad, LumpName::from_raw(rewrite.0), rewrite.1.clone())?;
        }
        Ok(())
    })?;
    Ok(RemoveReport {
        name,
        lump: winner.as_str(),
    })
}
