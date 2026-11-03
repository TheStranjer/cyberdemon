//! The map management tools' machinery: `create_map`, `copy_map`,
//! `rename_map` and `remove_map`.
//!
//! Every entry point here is one undoable [`crate::session::Session::edit`]
//! over the working archive: append a freshly encoded empty block (with the
//! player 1 start `create_map` was asked to stand in it), append a
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

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::session::Scope;
use crate::tools::edit_things::default_bits;
use crate::tools::maps::common::Directory;
use crate::tools::maps::params::fits_coord;
use crate::tools::maps::{CopyMapParams, CreateMapParams, CreateStartParam};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Analysis, GL_LEVEL, MapBlock, MapFormat, analyze, gl_marker_for};
use crate::wad::map::consts::PLAYER1_START;
use crate::wad::map::{Map, Thing, write_map};
use crate::wad::name::LumpName;

use super::{parse_format, resolve_namespace};

pub(super) use runs::{
    CopyReport, CreateReport, RemoveReport, RenameReport, copy_run, create_run, remove_run,
    rename_run,
};

#[cfg(test)]
mod tests;

/// Half-open directory range one lump run covers.
type Run = (usize, usize);

/// Whether `name` has one of the two shapes engines load without a
/// `MAPINFO`: `ExMy` (digits `x` and `y`) or `MAPxx` (two digits).
fn is_loadable_name(name: LumpName) -> bool {
    match *name.as_bytes() {
        [b'E', episode, b'M', map] => episode.is_ascii_digit() && map.is_ascii_digit(),
        [b'M', b'A', b'P', tens, ones] => tens.is_ascii_digit() && ones.is_ascii_digit(),
        _ => false,
    }
}

/// Degrees in a full circle; a requested start facing normalises into it.
const DEGREES: i32 = 360_i32;
/// The notice [`super::Cyberdemon::create_map`] carries when the fresh
/// block holds no player 1 start: nobody can spawn in an empty map, and a
/// start nobody remembers adding is the usual reason a finished level
/// fails its verification pass.
const NO_START_WARNING: &str = "the new map holds no player 1 start, so nothing can spawn: stand \
                                one in a sector (add_thing type 1) once the map has geometry, or \
                                pass player_start to create_map";

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

/// Every non-fatal notice of one `create_map`: the odd-name warning and,
/// when no start was asked for, the missing player 1 start.
fn create_warnings(name: LumpName, start: Option<CreateStart>) -> Vec<String> {
    let mut warnings = name_warning(name);
    if start.is_none() {
        warnings.push(NO_START_WARNING.to_owned());
    }
    warnings
}

/// The player 1 start [`super::Cyberdemon::create_map`] stands in the
/// fresh block: a map point with a facing, already range-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CreateStart {
    /// `X` in map units.
    pub(super) x: i32,
    /// `Y` in map units.
    pub(super) y: i32,
    /// Facing in degrees, normalised into `0..=359`.
    pub(super) angle: i32,
}

/// The validated start of one `create_map` call: the point the caller
/// passed, its facing normalised and its coordinates checked against the
/// layout that stores them.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an `x` or `y` outside the `i16`
/// range a binary `THINGS` record holds (`UDMF` stores full integers).
fn plan_start(
    given: Option<&CreateStartParam>,
    format: MapFormat,
) -> Result<Option<CreateStart>, CyberError> {
    let Some(start) = given else {
        return Ok(None);
    };
    if format != MapFormat::Udmf {
        fits_coord("x", start.x)?;
        fits_coord("y", start.y)?;
    }
    Ok(Some(CreateStart {
        x: start.x,
        y: start.y,
        angle: start.angle.unwrap_or_default().rem_euclid(DEGREES),
    }))
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
    /// The player 1 start to stand in the empty block; `None` leaves
    /// `THINGS` empty and the warnings say so.
    pub(super) start: Option<CreateStart>,
    /// Non-fatal notices (the odd name and the missing start).
    pub(super) warnings: Vec<String>,
}

/// Validate `create_map`'s arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an unparsable name, format,
/// namespace or a start outside the coordinates the layout stores.
pub(super) fn plan_create(args: &CreateMapParams) -> Result<CreatePlan, CyberError> {
    let name = LumpName::parse(&args.name)?;
    let format = parse_format(args.format.as_deref())?;
    let namespace = resolve_namespace(args.namespace.as_deref(), format)?;
    let start = plan_start(args.player_start.as_ref(), format)?;
    Ok(CreatePlan {
        name,
        format,
        namespace,
        replace: args.replace,
        warnings: create_warnings(name, start),
        start,
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

/// The type-1 [`Thing`] `spawn` describes in `map`'s flag family: the same
/// default presence bits [`crate::tools::edit_things`] gives an `add_thing`
/// row naming no flags, every other column zero.
fn start_thing(map: &Map, spawn: CreateStart) -> Thing {
    Thing {
        tid: 0,
        x: spawn.x,
        y: spawn.y,
        z: 0,
        angle: spawn.angle,
        kind: PLAYER1_START,
        flags: default_bits(map.flag_family()),
        special: 0,
        args: [0; 5],
        extra: BTreeMap::new(),
    }
}

/// The block for `name`: [`Map::new_empty`] written through [`write_map`],
/// with the `UDMF` namespace declared when one was resolved and the player
/// 1 start stood in `THINGS` when one was asked for.
fn empty_block(
    name: LumpName,
    format: MapFormat,
    namespace: Option<&str>,
    start: Option<CreateStart>,
) -> Result<Vec<Lump>, CyberError> {
    let mut map = Map::new_empty(name, format);
    map.udmf_namespace = namespace.map(str::to_owned);
    if let Some(spawn) = start {
        map.things.push(start_thing(&map, spawn));
    }
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

/// The copied `GL` run: every lump of `gl` with its marker renamed to the
/// name `to`'s block gets (see [`gl_marker_for`]). The run is appended
/// right after the copied block, so even the `GL_LEVEL` fallback attaches
/// to the block above it. Empty run with a warning when the directory no
/// longer holds the marker lump where the analysis read it.
fn gl_copies(dir: &Directory, gl: Run, to: LumpName, warnings: &mut Vec<String>) -> Vec<Lump> {
    let (start, end) = gl;
    let Some(head) = dir.lumps.get(start) else {
        warnings.push(format!(
            "the GL node run at {start}..{end} could not be copied: its marker lump is gone, so \
             the copy holds no node data - run build_nodes variant gl on {to} to rebuild it"
        ));
        return Vec::new();
    };
    let mut copies = vec![renamed(head, gl_marker_for(to))];
    let rest = start.saturating_add(1);
    if let Some(tail) = dir.lumps.get(rest..end) {
        copies.extend(tail.iter().cloned());
    }
    copies
}

/// The lumps to append for a copy of `block` under header name `to`: the
/// block with its header renamed, plus its `GL` run per [`gl_copies`].
fn copied_block(
    dir: &Directory,
    block: &MapBlock,
    to: LumpName,
    warnings: &mut Vec<String>,
) -> Vec<Lump> {
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
        copies.extend(gl_copies(dir, gl, to, warnings));
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

/// Renames `block`'s `GL` marker to the name `to`'s block gets (see
/// [`gl_marker_for`]). The run keeps its place unless that is impossible:
/// the `GL_LEVEL` fallback only attaches to the block right above it, so
/// a marker that does not sit there cannot carry a too-long name — that
/// run is reported for removal, with a warning naming the fix.
///
/// # Errors
///
/// [`set_name`]'s refusal for a stale marker position.
fn rename_gl(
    wad: &mut Wad,
    block: &MapBlock,
    to: LumpName,
    warnings: &mut Vec<String>,
) -> Result<Option<Run>, CyberError> {
    let Some((start, end)) = block.gl else {
        return Ok(None);
    };
    let Some(head) = wad.lumps.get(start).map(|lump| lump.name) else {
        return Ok(None);
    };
    let wanted = gl_marker_for(to);
    if head == wanted {
        return Ok(None);
    }
    if wanted == GL_LEVEL && start != block.end {
        warnings.push(format!(
            "the GL node run at {start}..{end} of {to} is dropped: the new name leaves no room \
             for a GL_<name> marker and the run does not sit right after the block, where \
             GL_LEVEL would attach - run build_nodes variant gl on {to} to rebuild it"
        ));
        return Ok(Some((start, end)));
    }
    set_name(wad, start, wanted)?;
    Ok(None)
}

/// Renames every working block named `from` to `to`, following each with
/// its `GL` marker (see [`rename_gl`]).
///
/// # Errors
///
/// [`CyberError::NotFound`] when the working archive holds no such block,
/// [`CyberError::AlreadyExists`] when it already holds a block named `to`;
/// nothing changes in either case.
fn apply_rename(
    wad: &mut Wad,
    from: LumpName,
    to: LumpName,
    warnings: &mut Vec<String>,
) -> Result<(), CyberError> {
    let analysis = analyze(&wad.lumps);
    let blocks: Vec<&MapBlock> = analysis
        .maps
        .iter()
        .filter(|block| block.name == from)
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
    for block in blocks {
        set_name(wad, block.header, to)?;
        if let Some((start, end)) = rename_gl(wad, block, to, warnings)? {
            doomed.extend(start..end);
        }
    }
    if !doomed.is_empty() {
        drop_positions(wad, &doomed);
    }
    Ok(())
}
