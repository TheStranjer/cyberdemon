//! The map management tools' machinery: `create_map`, `copy_map`,
//! `rename_map` and `remove_map`.
//!
//! Every entry point here is one undoable [`crate::session::Session::edit`]
//! over the working archive: append a freshly encoded empty block, append a
//! renamed copy of a scope's block, rename a block's header (and `GL`
//! marker) in place, or drop a block (and its `GL` run). The read-only
//! layers never change; a name they alone hold is overridden by the working
//! archive, which is exactly what `create_map` and `copy_map` do on
//! purpose. The active map follows management outside the history: `create`
//! adopts it when none was set, `rename` moves it, `remove` clears it —
//! `undo` never replays any of that.
//!
//! The four runs and their report shapes live in the private submodule
//! [`runs`], re-exported here so the router methods keep reaching them
//! through `manage::`; the unit tests split by topic in [`tests`].

mod runs;

use alloc::sync::Arc;

use crate::error::CyberError;
use crate::session::Scope;
use crate::tools::maps::common::Directory;
use crate::tools::maps::{CopyMapParams, CreateMapParams};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Analysis, MapBlock, MapFormat, analyze};
use crate::wad::map::{Map, write_map};
use crate::wad::name::{LumpName, NAME_LEN};

use super::{parse_format, resolve_namespace};

pub(super) use runs::{
    CopyReport, CreateReport, RemoveReport, RenameReport, copy_run, create_run, remove_run,
    rename_run,
};

#[cfg(test)]
mod tests;

/// The prefix of a map's `GL` node marker lump.
const GL_PREFIX: &[u8] = b"GL_";
/// Longest map name whose `GL_<name>` marker still fits a directory name.
const GL_NAME_MAX: usize = NAME_LEN - GL_PREFIX.len();
/// The `GL` marker that attaches to the map block just above it.
const GL_LEVEL: &[u8] = b"GL_LEVEL";

/// Half-open directory range one lump run covers.
type Run = (usize, usize);

/// What a rename remembers about one block: its header position and the
/// `GL` run attached to it.
type BlockHead = (usize, Option<Run>);

/// What management does with a block's `GL` node run when the map is
/// copied or renamed to `to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GlPlan {
    /// The marker is renamed to this name.
    Rename(LumpName),
    /// The marker is `GL_LEVEL`: it stays and attaches to the block written
    /// just above it in the new position.
    Keep,
    /// The run is dropped: `GL_<to>` would not fit eight bytes.
    Drop,
}

/// The plan for the `GL` run whose marker lump is named `head`, once the
/// map itself answers to `to`.
fn gl_plan(head: LumpName, to: LumpName) -> GlPlan {
    if head.as_bytes() == GL_LEVEL {
        return GlPlan::Keep;
    }
    gl_marker(to).map_or(GlPlan::Drop, GlPlan::Rename)
}

/// The `GL_<to>` marker name, `None` when `to` is too long to fit behind
/// the prefix.
fn gl_marker(to: LumpName) -> Option<LumpName> {
    if to.len() > GL_NAME_MAX {
        return None;
    }
    let mut raw = [0_u8; NAME_LEN];
    for (slot, byte) in raw.iter_mut().zip(GL_PREFIX.iter().chain(to.raw().iter())) {
        *slot = *byte;
    }
    Some(LumpName::from_raw(raw))
}

/// Whether `name` has one of the two shapes engines load without a
/// `MAPINFO`: `ExMy` (digits `x` and `y`) or `MAPxx` (two digits).
fn is_loadable_name(name: LumpName) -> bool {
    match *name.as_bytes() {
        [b'E', episode, b'M', map] => episode.is_ascii_digit() && map.is_ascii_digit(),
        [b'M', b'A', b'P', tens, ones] => tens.is_ascii_digit() && ones.is_ascii_digit(),
        _ => false,
    }
}

/// The warning [`super::Cyberdemon::create_map`] carries for a name no
/// engine loads by default; empty for `ExMy`/`MAPxx` names.
fn name_warning(name: LumpName) -> Vec<String> {
    if is_loadable_name(name) {
        return Vec::new();
    }
    vec![format!(
        "{name} is neither ExMy nor MAPxx: engines only load ExMy/MAPxx unless MAPINFO defines \
         it, so add a MAPINFO/UMAPINFO entry or rename the map"
    )]
}

/// The validated arguments of one [`super::Cyberdemon::create_map`] call.
#[derive(Debug)]
pub(super) struct CreatePlan {
    /// The header name the new block gets.
    pub(super) name: LumpName,
    /// The layout to write.
    pub(super) format: MapFormat,
    /// The `UDMF` namespace to declare; `None` for binary formats.
    pub(super) namespace: Option<String>,
    /// Whether an existing working block may be discarded.
    pub(super) replace: bool,
    /// Non-fatal notices (the odd-name warning).
    pub(super) warnings: Vec<String>,
}

/// Validate `create_map`'s arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an unparsable name, format or
/// namespace.
pub(super) fn plan_create(args: &CreateMapParams) -> Result<CreatePlan, CyberError> {
    let name = LumpName::parse(&args.name)?;
    let format = parse_format(args.format.as_deref())?;
    let namespace = resolve_namespace(args.namespace.as_deref(), format)?;
    Ok(CreatePlan {
        name,
        format,
        namespace,
        replace: args.replace,
        warnings: name_warning(name),
    })
}

/// The validated arguments of one [`super::Cyberdemon::copy_map`] call.
#[derive(Debug)]
pub(super) struct CopyPlan {
    /// The map to read.
    pub(super) from: LumpName,
    /// The name the copy gets in the working archive.
    pub(super) to: LumpName,
    /// The archive the source block is read from.
    pub(super) scope: Scope,
    /// Whether an existing working target block may be discarded.
    pub(super) replace: bool,
}

/// Validate `copy_map`'s arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an unparsable name or scope.
pub(super) fn plan_copy(args: &CopyMapParams) -> Result<CopyPlan, CyberError> {
    Ok(CopyPlan {
        from: LumpName::parse(&args.from)?,
        to: LumpName::parse(&args.to)?,
        scope: Scope::parse_or_merged(args.scope.as_deref())?,
        replace: args.replace,
    })
}

/// The empty block for `name`: [`Map::new_empty`] written through
/// [`write_map`], with the `UDMF` namespace declared when one was resolved.
fn empty_block(
    name: LumpName,
    format: MapFormat,
    namespace: Option<&str>,
) -> Result<Vec<Lump>, CyberError> {
    let mut map = Map::new_empty(name, format);
    map.udmf_namespace = namespace.map(str::to_owned);
    write_map(&map)
}

/// The directory positions of every block named `name` in `analysis` and
/// of the `GL` runs attached to them, in directory order.
fn doomed_positions(analysis: &Analysis, name: LumpName) -> Vec<usize> {
    let mut positions = Vec::new();
    for block in analysis.maps.iter().filter(|block| block.name == name) {
        positions.extend(block.header..block.end);
        if let Some((start, end)) = block.gl {
            positions.extend(start..end);
        }
    }
    positions
}

/// Drops the lumps at `positions` from `wad`, returning how many went.
fn drop_positions(wad: &mut Wad, positions: &[usize]) -> usize {
    let before = wad.lumps.len();
    let kept: Vec<Lump> = core::mem::take(&mut wad.lumps)
        .into_iter()
        .enumerate()
        .filter(|&(index, _)| !positions.contains(&index))
        .map(|(_, lump)| lump)
        .collect();
    wad.lumps = kept;
    before.saturating_sub(wad.lumps.len())
}

/// A copy of `lump` under the name `to`, sharing its payload.
fn renamed(lump: &Lump, to: LumpName) -> Lump {
    Lump {
        name: to,
        data: Arc::clone(&lump.data),
    }
}

/// The copied `GL` run: dropped whole when the marker cannot carry the new
/// name, kept verbatim for `GL_LEVEL`, and renamed at its header otherwise.
fn gl_copies(dir: &Directory, gl: Run, to: LumpName) -> Vec<Lump> {
    let (start, end) = gl;
    let Some(head) = dir.lumps.get(start) else {
        return Vec::new();
    };
    match gl_plan(head.name, to) {
        GlPlan::Drop => Vec::new(),
        GlPlan::Keep => dir.lumps.get(start..end).unwrap_or(&[]).to_vec(),
        GlPlan::Rename(name) => {
            let mut copies = vec![renamed(head, name)];
            let rest = start.saturating_add(1);
            if let Some(tail) = dir.lumps.get(rest..end) {
                copies.extend(tail.iter().cloned());
            }
            copies
        }
    }
}

/// The lumps to append for a copy of `block` under header name `to`: the
/// block with its header renamed, plus its `GL` run per [`gl_copies`].
fn copied_block(dir: &Directory, block: &MapBlock, to: LumpName) -> Vec<Lump> {
    let mut copies: Vec<Lump> = Vec::new();
    for position in block.header..block.end {
        let Some(lump) = dir.lumps.get(position) else {
            continue;
        };
        if position == block.header {
            copies.push(renamed(lump, to));
        } else {
            copies.push(lump.clone());
        }
    }
    if let Some(gl) = block.gl {
        copies.extend(gl_copies(dir, gl, to));
    }
    copies
}

/// Appends `lumps` as the working archive's new block for `name`, first
/// discarding the old block of that name when `replace` allows it.
///
/// `build_map_from_plan` writes its compiled block through the same door
/// (see the `write_block` wrapper one level up).
///
/// # Errors
///
/// [`CyberError::AlreadyExists`] when the working archive holds a block
/// named `name` and `replace` is `false`.
pub(super) fn append_block(
    wad: &mut Wad,
    name: LumpName,
    replace: bool,
    lumps: Vec<Lump>,
) -> Result<(), CyberError> {
    let existing = doomed_positions(&analyze(&wad.lumps), name);
    if !existing.is_empty() && !replace {
        return Err(CyberError::AlreadyExists(format!(
            "the working archive already holds a map block named {name}: pass replace true to \
             discard the old block first, or create under another name"
        )));
    }
    drop_positions(wad, &existing);
    wad.lumps.extend(lumps);
    Ok(())
}

/// Writes `to` over the name of the lump at `index`.
///
/// # Errors
///
/// [`CyberError::Internal`] when the directory holds no lump there.
fn set_name(wad: &mut Wad, index: usize, to: LumpName) -> Result<(), CyberError> {
    let Some(lump) = wad.lumps.get_mut(index) else {
        return Err(CyberError::Internal(format!(
            "the working archive holds no lump at index {index} to name {to}"
        )));
    };
    lump.name = to;
    Ok(())
}

/// Renames `block`'s `GL` marker to match `to`, reporting the run's range
/// when the marker cannot carry the new name and the whole run goes.
///
/// # Errors
///
/// [`set_name`]'s refusal for a stale marker position.
fn rename_gl(wad: &mut Wad, gl: Option<Run>, to: LumpName) -> Result<Option<Run>, CyberError> {
    let Some((start, end)) = gl else {
        return Ok(None);
    };
    let Some(head) = wad.lumps.get(start) else {
        return Ok(None);
    };
    match gl_plan(head.name, to) {
        GlPlan::Keep => Ok(None),
        GlPlan::Drop => Ok(Some((start, end))),
        GlPlan::Rename(name) => set_name(wad, start, name).map(|()| None),
    }
}

/// Renames every working block named `from` to `to`, following each with
/// its `GL` marker (see [`rename_gl`]).
///
/// # Errors
///
/// [`CyberError::NotFound`] when the working archive holds no such block,
/// [`CyberError::AlreadyExists`] when it already holds a block named `to`;
/// nothing changes in either case.
fn apply_rename(wad: &mut Wad, from: LumpName, to: LumpName) -> Result<(), CyberError> {
    let analysis = analyze(&wad.lumps);
    let blocks: Vec<BlockHead> = analysis
        .maps
        .iter()
        .filter(|block| block.name == from)
        .map(|block| (block.header, block.gl))
        .collect();
    if blocks.is_empty() {
        return Err(CyberError::NotFound(format!(
            "no map block named {from} in the working archive: the IWAD and resource layers are \
             read-only, so copy_map {from} {to} brings that map in under the new name instead"
        )));
    }
    if analysis.maps.iter().any(|block| block.name == to) {
        return Err(CyberError::AlreadyExists(format!(
            "the working archive already holds a map block named {to}: remove_map it first, or \
             rename {from} to another name"
        )));
    }
    let mut doomed: Vec<usize> = Vec::new();
    for (header, gl) in blocks {
        set_name(wad, header, to)?;
        if let Some((start, end)) = rename_gl(wad, gl, to)? {
            doomed.extend(start..end);
        }
    }
    if !doomed.is_empty() {
        drop_positions(wad, &doomed);
    }
    Ok(())
}
