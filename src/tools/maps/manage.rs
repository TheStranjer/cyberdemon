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

use alloc::sync::Arc;

use serde::Serialize;

use crate::error::CyberError;
use crate::session::{Scope, Session};
use crate::tools::maps::common::{self, Directory};
use crate::tools::maps::{CopyMapParams, CreateMapParams};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Analysis, MapBlock, MapFormat, analyze};
use crate::wad::map::{Map, write_map};
use crate::wad::name::{LumpName, NAME_LEN};

use super::{parse_format, parse_scope, resolve_namespace};

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
        scope: parse_scope(args.scope.as_deref())?,
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

/// Structured result of [`super::Cyberdemon::create_map`].
#[derive(Debug, Serialize)]
pub(super) struct CreateReport {
    /// The map that was created.
    map: LumpName,
    /// The layout its block uses.
    format: MapFormat,
    /// The namespace a `UDMF` block declares; `null` for binary formats.
    namespace: Option<String>,
    /// Non-fatal notices: the odd-name warning.
    warnings: Vec<String>,
    /// The session's active map after the call.
    active_map: Option<LumpName>,
}

/// Runs one `create_map`: append the freshly encoded empty block, then
/// adopt the new map as the active selection when none was set.
///
/// # Errors
///
/// [`empty_block`]'s encoder refusals and [`CyberError::AlreadyExists`]
/// for a working block of that name without `replace`; the session does
/// not change when the edit fails.
pub(super) fn create_run(
    session: &mut Session,
    plan: CreatePlan,
) -> Result<CreateReport, CyberError> {
    let CreatePlan {
        name,
        format,
        namespace,
        replace,
        warnings,
    } = plan;
    let lumps = empty_block(name, format, namespace.as_deref())?;
    let label = format!("create_map {name}");
    session.edit(&label, move |wad| append_block(wad, name, replace, lumps))?;
    if session.active_map().is_none() {
        session.set_active_map(Some(name));
    }
    Ok(CreateReport {
        map: name,
        format,
        namespace,
        warnings,
        active_map: session.active_map(),
    })
}

/// Structured result of [`super::Cyberdemon::copy_map`].
#[derive(Debug, Serialize)]
pub(super) struct CopyReport {
    /// The map that was read.
    from: LumpName,
    /// The name the copy holds in the working archive.
    to: LumpName,
    /// Lumps appended (header and members, the kept GL run included).
    lumps: usize,
}

/// Runs one `copy_map`: resolve the source block in its scope and append
/// the renamed copy as one undoable edit.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `scope` holds no block named `from`,
/// [`CyberError::AlreadyExists`] when the working archive holds `to`
/// without `replace`; a failed call changes nothing.
pub(super) fn copy_run(session: &mut Session, plan: &CopyPlan) -> Result<CopyReport, CyberError> {
    let dir = common::directory(session, plan.scope)?;
    let block = dir
        .find_block(plan.from)
        .ok_or_else(|| common::missing_map(plan.from, plan.scope))?;
    let copies = copied_block(&dir, block, plan.to);
    let count = copies.len();
    let label = format!("copy_map {} -> {}", plan.from, plan.to);
    let to = plan.to;
    let replace = plan.replace;
    session.edit(&label, move |wad| {
        let existing = doomed_positions(&analyze(&wad.lumps), to);
        if !existing.is_empty() && !replace {
            return Err(CyberError::AlreadyExists(format!(
                "the working archive already holds a map block named {to}: pass replace true to \
                 discard it first, or copy to another name"
            )));
        }
        drop_positions(wad, &existing);
        wad.lumps.extend(copies);
        Ok(())
    })?;
    Ok(CopyReport {
        from: plan.from,
        to: plan.to,
        lumps: count,
    })
}

/// Structured result of [`super::Cyberdemon::rename_map`].
#[derive(Debug, Serialize)]
pub(super) struct RenameReport {
    /// The name the map had.
    from: LumpName,
    /// The name it has now.
    to: LumpName,
}

/// Runs one `rename_map`: rename the working block in place, then move the
/// active selection when it pointed at the old name.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `from` equal to `to`, plus
/// [`apply_rename`]'s refusals; a failed call changes nothing.
pub(super) fn rename_run(
    session: &mut Session,
    from: LumpName,
    to: LumpName,
) -> Result<RenameReport, CyberError> {
    if from == to {
        return Err(CyberError::InvalidArgument(format!(
            "rename_map names the same map twice ({from}): pass a different to, or no rename is \
             needed"
        )));
    }
    let label = format!("rename_map {from} -> {to}");
    session.edit(&label, |wad| apply_rename(wad, from, to))?;
    if session.active_map() == Some(from) {
        session.set_active_map(Some(to));
    }
    Ok(RenameReport { from, to })
}

/// Structured result of [`super::Cyberdemon::remove_map`].
#[derive(Debug, Serialize)]
pub(super) struct RemoveReport {
    /// The map that was removed.
    map: LumpName,
    /// Lumps removed (block and `GL` run together).
    lumps_removed: usize,
}

/// Runs one `remove_map`: drop the block and its `GL` run, then clear the
/// active selection when it pointed at the removed map.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the working archive holds no such block;
/// a failed call changes nothing.
pub(super) fn remove_run(
    session: &mut Session,
    name: LumpName,
) -> Result<RemoveReport, CyberError> {
    let label = format!("remove_map {name}");
    let removed = session.edit(&label, |wad| {
        let doomed = doomed_positions(&analyze(&wad.lumps), name);
        if doomed.is_empty() {
            return Err(CyberError::NotFound(format!(
                "no map block named {name} in the working archive: the IWAD and resource layers \
                 are read-only and never lose their maps, so list_maps shows what is editable"
            )));
        }
        Ok(drop_positions(wad, &doomed))
    })?;
    if session.active_map() == Some(name) {
        session.set_active_map(None);
    }
    Ok(RemoveReport {
        map: name,
        lumps_removed: removed,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{Value, json};

    use super::{
        CopyPlan, CreatePlan, GlPlan, copied_block, copy_run, create_run, doomed_positions,
        empty_block, gl_marker, gl_plan, is_loadable_name, name_warning, plan_copy, plan_create,
        remove_run, rename_gl, rename_run,
    };
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::tools::maps::common::directory;
    use crate::tools::maps::{CopyMapParams, CreateMapParams, parse_format, resolve_namespace};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::{MapFormat, analyze};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A lump named `text` carrying `payload` as its bytes.
    fn carrying(text: &str, payload: &str) -> Lump {
        Lump::new(name(text), payload.as_bytes().to_vec())
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A session whose only layer is a base archive of `lumps`.
    fn base_session(lumps: Vec<Lump>) -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        ));
        session
    }

    /// A base layer: a `MAP01` block with its `GL` run and loose lumps.
    fn layer_map() -> Vec<Lump> {
        vec![
            marker("PLAYPAL"),
            marker("MAP01"),
            carrying("THINGS", "things"),
            carrying("LINEDEFS", "lines"),
            marker("GL_MAP01"),
            carrying("GL_VERT", "gl"),
            marker("ENDPAK"),
        ]
    }

    /// Appends `lumps` to the working archive as one seeded edit.
    fn seed(session: &mut Session, lumps: Vec<Lump>) {
        session
            .edit("seed", move |wad| {
                wad.lumps.extend(lumps);
                Ok(())
            })
            .expect("plain appends");
    }

    /// A `create_map` argument set.
    fn create_args(text: &str, format: Option<&str>, namespace: Option<&str>) -> CreateMapParams {
        CreateMapParams {
            name: text.to_owned(),
            format: format.map(str::to_owned),
            namespace: namespace.map(str::to_owned),
            replace: false,
        }
    }

    /// A validated `create_map` plan for `text` in `format`.
    fn create_plan(text: &str, format: &str, namespace: Option<&str>) -> CreatePlan {
        plan_create(&create_args(text, Some(format), namespace)).expect("valid plan")
    }

    /// A validated `create_map` plan for `text` with the default format.
    fn doom_plan(text: &str) -> CreatePlan {
        plan_create(&create_args(text, None, None)).expect("valid plan")
    }

    /// A `copy_map` argument set.
    fn copy_args(from: &str, to: &str, scope: Option<&str>, replace: bool) -> CopyMapParams {
        CopyMapParams {
            from: from.to_owned(),
            to: to.to_owned(),
            scope: scope.map(str::to_owned),
            replace,
        }
    }

    /// A validated `copy_map` plan.
    fn copy_plan(from: &str, to: &str, scope: &str, replace: bool) -> CopyPlan {
        plan_copy(&copy_args(from, to, Some(scope), replace)).expect("valid plan")
    }

    /// Serialize a report for whole-shape asserts.
    fn value<T: serde::Serialize>(report: &T) -> Value {
        serde_json::to_value(report).expect("reports serialize")
    }

    /// The working archive's directory names, in order.
    fn working_names(session: &Session) -> Vec<String> {
        session
            .working()
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect()
    }

    /// How many working lumps carry the name `text`.
    fn count_named(names: &[String], text: &str) -> usize {
        names.iter().filter(|seen| *seen == text).count()
    }

    #[test]
    fn parse_format_defaults_to_doom_and_lists_the_choices() {
        assert_eq!(parse_format(None).expect("absent"), MapFormat::Doom);
        assert_eq!(parse_format(Some("Hexen")).expect("case"), MapFormat::Hexen);
        assert_eq!(parse_format(Some("udmf")).expect("lower"), MapFormat::Udmf);
        let error = parse_format(Some("zdoom")).expect_err("not a format");
        assert_eq!(code(&error), "invalid_argument", "bad format code");
        assert!(
            error.to_string().contains("doom") && error.to_string().contains("udmf"),
            "the message lists the choices: {error}"
        );
    }

    #[test]
    fn namespaces_default_validate_and_refuse_binaries() {
        assert_eq!(
            resolve_namespace(None, MapFormat::Udmf).expect("default"),
            Some("zdoom".to_owned()),
            "UDMF without a namespace gets the default",
        );
        assert_eq!(
            resolve_namespace(Some("GLBDOOM"), MapFormat::Udmf).expect("lowered"),
            Some("glbdoom".to_owned()),
            "names are stored lowercase",
        );
        assert_eq!(
            resolve_namespace(None, MapFormat::Doom).expect("binary default"),
            None,
            "binary maps declare no namespace",
        );
        let clash = resolve_namespace(Some("zdoom"), MapFormat::Hexen).expect_err("binary ns");
        assert_eq!(code(&clash), "invalid_argument", "binary namespace code");
        assert!(
            clash.to_string().contains("UDMF"),
            "the message names the format: {clash}"
        );
        let bad = resolve_namespace(Some("not a ns"), MapFormat::Udmf).expect_err("bad name");
        assert_eq!(code(&bad), "invalid_argument", "bad namespace code");
    }

    #[test]
    fn loadable_names_follow_the_two_engine_forms() {
        for text in ["MAP01", "map09", "MAP99", "E1M1", "E9M9"] {
            assert!(is_loadable_name(name(text)), "{text} is loadable");
        }
        for text in ["E12M3", "MAP0A", "HOUR01", "SLOTPAD", "E1N1"] {
            assert!(!is_loadable_name(name(text)), "{text} is not");
        }
    }

    #[test]
    fn odd_names_warn_with_the_engine_rule() {
        assert!(name_warning(name("MAP07")).is_empty(), "MAPxx is quiet");
        assert!(name_warning(name("E2M7")).is_empty(), "ExMy is quiet");
        let warnings = name_warning(name("SLOTPAD"));
        assert_eq!(warnings.len(), 1, "the odd name warns once");
        assert!(
            warnings[0].contains("SLOTPAD")
                && warnings[0].contains("ExMy")
                && warnings[0].contains("MAPINFO"),
            "the warning names the map and the fix: {}",
            warnings[0],
        );
    }

    #[test]
    fn gl_markers_fit_short_names_only() {
        assert_eq!(
            gl_marker(name("MAP07")).expect("five fit"),
            name("GL_MAP07")
        );
        assert_eq!(gl_marker(name("E1M1")).expect("four fit"), name("GL_E1M1"));
        assert_eq!(gl_marker(name("LONGNAME")), None, "GL_ plus eight is over");
        assert_eq!(
            gl_plan(name("GL_MAP01"), name("MAP09")),
            GlPlan::Rename(name("GL_MAP09"))
        );
        assert_eq!(gl_plan(name("GL_LEVEL"), name("MAP09")), GlPlan::Keep);
        assert_eq!(gl_plan(name("GL_MAP01"), name("LONGNAME")), GlPlan::Drop);
    }

    #[test]
    fn empty_blocks_match_the_documented_member_sets() {
        let doom = empty_block(name("MAP05"), MapFormat::Doom, None).expect("doom writes");
        let doom_names: Vec<String> = doom.iter().map(|lump| lump.name.to_string()).collect();
        assert_eq!(
            doom_names,
            vec![
                "MAP05", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
                "SECTORS", "REJECT", "BLOCKMAP",
            ],
            "header plus the ten vanilla members",
        );
        assert!(
            doom.iter().all(|lump| lump.size() == 0),
            "every member is empty"
        );
        let hexen = empty_block(name("MAP05"), MapFormat::Hexen, None).expect("hexen writes");
        assert_eq!(
            analyze(&hexen).maps.first().expect("one block").format,
            MapFormat::Hexen,
            "the generated BEHAVIOR marks it Hexen",
        );
        let udmf = empty_block(name("MAP05"), MapFormat::Udmf, Some("doom")).expect("udmf writes");
        let udmf_names: Vec<String> = udmf.iter().map(|lump| lump.name.to_string()).collect();
        assert_eq!(
            udmf_names,
            vec!["MAP05", "TEXTMAP", "ENDMAP"],
            "the text block"
        );
        assert_eq!(
            analyze(&udmf).maps.first().expect("one block").format,
            MapFormat::Udmf
        );
        let body = udmf.get(1).expect("TEXTMAP").data.as_ref().to_vec();
        assert_eq!(
            String::from_utf8(body).expect("UDMF is text").trim(),
            "namespace = \"doom\";",
            "the TEXTMAP declares only the namespace",
        );
    }

    #[test]
    fn a_copied_block_renames_header_and_gl_marker() {
        let session = base_session(layer_map());
        let dir = directory(&session, Scope::Merged).expect("the view reads");
        let block = dir
            .find_block(name("MAP01"))
            .expect("the layer map")
            .clone();
        let copies = copied_block(&dir, &block, name("MAP09"));
        let names: Vec<String> = copies.iter().map(|lump| lump.name.to_string()).collect();
        assert_eq!(
            names,
            vec!["MAP09", "THINGS", "LINEDEFS", "GL_MAP09", "GL_VERT"],
            "header and marker renamed, member and GL bytes kept",
        );
        let things = copies.get(1).expect("THINGS").data.as_ref().to_vec();
        assert_eq!(String::from_utf8(things).expect("text payload"), "things");
        let dropped = copied_block(&dir, &block, name("LONGNAME"));
        let dropped_names: Vec<String> = dropped.iter().map(|lump| lump.name.to_string()).collect();
        assert_eq!(
            dropped_names,
            vec!["LONGNAME", "THINGS", "LINEDEFS"],
            "the GL run goes when the marker would not fit",
        );
    }

    #[test]
    fn a_copied_gl_level_marker_stays_gl_level() {
        let session = base_session(vec![
            marker("E1M1"),
            carrying("THINGS", "t"),
            marker("GL_LEVEL"),
            carrying("GL_VERT", "g"),
        ]);
        let dir = directory(&session, Scope::Merged).expect("the view reads");
        let block = dir.find_block(name("E1M1")).expect("the block").clone();
        let copies = copied_block(&dir, &block, name("E2M1"));
        let names: Vec<String> = copies.iter().map(|lump| lump.name.to_string()).collect();
        assert_eq!(
            names,
            vec!["E2M1", "THINGS", "GL_LEVEL", "GL_VERT"],
            "GL_LEVEL keeps its name and attaches to the copy above it",
        );
    }

    #[test]
    fn doomed_positions_covers_every_same_named_block_and_gl_run() {
        let session = base_session(vec![
            marker("MAP01"),
            carrying("THINGS", "a"),
            marker("GL_MAP01"),
            carrying("GL_VERT", "g"),
            marker("MAP01"),
            carrying("THINGS", "b"),
            marker("MAP02"),
            carrying("THINGS", "c"),
        ]);
        let dir = directory(&session, Scope::Layer(0)).expect("layer 0 reads");
        assert_eq!(
            doomed_positions(&dir.analysis, name("MAP01")),
            vec![0_usize, 1, 2, 3, 4, 5],
            "both MAP01 blocks and the attached GL run",
        );
        assert_eq!(
            doomed_positions(&dir.analysis, name("MAP02")),
            vec![6_usize, 7]
        );
        assert!(
            doomed_positions(&dir.analysis, name("MAP09")).is_empty(),
            "an unknown map dooms nothing",
        );
    }

    #[test]
    fn create_run_appends_a_doom_block_and_adopts_the_active_map() {
        let mut session = base_session(layer_map());
        let report = create_run(&mut session, doom_plan("MAP05")).expect("fresh map");
        assert_eq!(
            value(&report),
            json!({
                "map": "MAP05", "format": "doom", "namespace": null, "warnings": [],
                "active_map": "MAP05",
            }),
            "the documented result",
        );
        assert_eq!(
            session.undo_labels(),
            vec!["create_map MAP05"],
            "one undoable edit",
        );
        assert!(session.is_dirty(), "and unsaved work");
        let appended = working_names(&session);
        let start = appended
            .iter()
            .position(|text| text == "MAP05")
            .expect("MAP05");
        assert_eq!(
            &appended[start..],
            [
                "MAP05", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
                "SECTORS", "REJECT", "BLOCKMAP",
            ],
            "the empty block sits at the end of the working archive",
        );
        session.undo().expect("undo the create");
        assert!(session.working().is_empty(), "the block went back");
    }

    #[test]
    fn create_run_refuses_working_duplicates_replaces_and_overrides_layers() {
        let mut session = base_session(layer_map());
        create_run(&mut session, doom_plan("MAP01")).expect("the layer copy is overridable");
        let error = create_run(&mut session, doom_plan("MAP01")).expect_err("now it is working");
        assert_eq!(code(&error), "already_exists", "working duplicate code");
        assert!(
            error.to_string().contains("replace"),
            "the message names the way out: {error}"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["create_map MAP01"],
            "a refusal leaves no history",
        );
        let mut replace = doom_plan("MAP01");
        replace.replace = true;
        create_run(&mut session, replace).expect("replace lands");
        let names = working_names(&session);
        assert_eq!(
            count_named(&names, "MAP01"),
            1,
            "the old block was discarded"
        );
        assert_eq!(session.undo_labels().len(), 2, "replace is its own step");
    }

    #[test]
    fn create_run_builds_hexen_and_udmf_blocks_and_keeps_the_active_map() {
        let mut session = Session::default();
        session.set_active_map(Some(name("MAP01")));
        let hexen = create_run(&mut session, create_plan("E1M1", "hexen", None)).expect("hexen");
        assert_eq!(value(&hexen)["format"], json!("hexen"), "the hexen report");
        assert_eq!(
            value(&hexen)["active_map"],
            json!("MAP01"),
            "a set active map is kept",
        );
        assert!(
            value(&hexen)["warnings"]
                .as_array()
                .expect("warnings")
                .is_empty(),
            "E1M1 is a loadable name",
        );
        assert!(
            working_names(&session).contains(&"BEHAVIOR".to_owned()),
            "the hexen block carries its BEHAVIOR",
        );
        let udmf =
            create_run(&mut session, create_plan("SLICK", "udmf", Some("GZDoom"))).expect("udmf");
        assert_eq!(
            value(&udmf)["namespace"],
            json!("gzdoom"),
            "lowered namespace"
        );
        let names = working_names(&session);
        let tail: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .skip(names.len().saturating_sub(3))
            .collect();
        assert_eq!(
            tail,
            vec!["SLICK", "TEXTMAP", "ENDMAP"],
            "the udmf block sits at the very end",
        );
    }

    #[test]
    fn copy_run_copies_a_layer_block_and_renames_the_gl_marker() {
        let mut session = base_session(layer_map());
        let report = copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", false))
            .expect("the copy lands");
        assert_eq!(
            value(&report),
            json!({"from": "MAP01", "to": "MAP09", "lumps": 5_u64}),
            "the documented result, GL run counted",
        );
        assert_eq!(
            working_names(&session),
            vec!["MAP09", "THINGS", "LINEDEFS", "GL_MAP09", "GL_VERT"],
            "block plus renamed GL run, header renamed",
        );
        assert_eq!(
            session.undo_labels(),
            vec!["copy_map MAP01 -> MAP09"],
            "label"
        );
    }

    #[test]
    fn copy_run_drops_the_gl_run_for_an_oversized_marker_name() {
        let mut session = base_session(layer_map());
        let report = copy_run(
            &mut session,
            &copy_plan("MAP01", "LONGNAME", "layer:0", false),
        )
        .expect("the copy lands");
        assert_eq!(
            value(&report)["lumps"],
            json!(3_u64),
            "the GL run is left out"
        );
        assert_eq!(
            working_names(&session),
            vec!["LONGNAME", "THINGS", "LINEDEFS"],
            "members only",
        );
    }

    #[test]
    fn copy_run_refuses_missing_sources_and_existing_targets_and_replaces() {
        let mut session = base_session(layer_map());
        let missing = copy_run(&mut session, &copy_plan("MAP07", "MAP09", "merged", false))
            .expect_err("no MAP07 anywhere");
        assert_eq!(code(&missing), "not_found", "missing source code");
        assert!(session.working().is_empty(), "nothing was copied");
        assert!(session.undo_labels().is_empty(), "and nothing recorded");
        copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", false)).expect("first copy");
        let clash = copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", false))
            .expect_err("MAP09 is working now");
        assert_eq!(code(&clash), "already_exists", "existing target code");
        let names = working_names(&session);
        assert_eq!(
            count_named(&names, "MAP09"),
            1,
            "the refusal appended nothing"
        );
        copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", true)).expect("replace");
        assert_eq!(
            working_names(&session),
            vec!["MAP09", "THINGS", "LINEDEFS", "GL_MAP09", "GL_VERT"],
            "the replaced copy stands alone at the end",
        );
        assert_eq!(
            session.undo_labels().len(),
            2,
            "copy and replace are two steps",
        );
    }

    #[test]
    fn copy_run_can_read_a_working_block_under_a_new_name() {
        let mut session = base_session(layer_map());
        copy_run(&mut session, &copy_plan("MAP01", "MAP09", "working", false))
            .expect_err("working holds no MAP01");
        copy_run(&mut session, &copy_plan("MAP01", "MAP09", "merged", false)).expect("layer copy");
        copy_run(&mut session, &copy_plan("MAP09", "MAP10", "working", false)).expect("re-copy");
        let names = working_names(&session);
        assert_eq!(count_named(&names, "MAP09"), 1, "the first copy stands");
        assert_eq!(count_named(&names, "MAP10"), 1, "the second arrived");
    }

    #[test]
    fn rename_run_renames_the_block_and_marker_and_follows_the_active_map() {
        let mut session = base_session(layer_map());
        copy_run(&mut session, &copy_plan("MAP01", "MAP08", "merged", false)).expect("copy in");
        session.set_active_map(Some(name("MAP08")));
        let report = rename_run(&mut session, name("MAP08"), name("MAP02")).expect("renamed");
        assert_eq!(
            value(&report),
            json!({"from": "MAP08", "to": "MAP02"}),
            "the documented result",
        );
        assert_eq!(
            working_names(&session),
            vec!["MAP02", "THINGS", "LINEDEFS", "GL_MAP02", "GL_VERT"],
            "header and marker renamed in place",
        );
        assert_eq!(
            session.active_map(),
            Some(name("MAP02")),
            "the selection follows"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["rename_map MAP08 -> MAP02", "copy_map MAP01 -> MAP08"],
            "one step per call",
        );
        session.undo().expect("undo the rename");
        assert_eq!(
            working_names(&session),
            vec!["MAP08", "THINGS", "LINEDEFS", "GL_MAP08", "GL_VERT"],
            "the old names stand again",
        );
        assert_eq!(
            session.active_map(),
            Some(name("MAP02")),
            "the selection is not part of the history",
        );
    }

    #[test]
    fn rename_run_keeps_gl_level_and_drops_oversized_markers() {
        let mut session = Session::default();
        seed(
            &mut session,
            vec![
                marker("E1M1"),
                carrying("THINGS", "t"),
                marker("GL_LEVEL"),
                carrying("GL_VERT", "g"),
            ],
        );
        rename_run(&mut session, name("E1M1"), name("ABCDEFGH")).expect("gl_level stays");
        assert_eq!(
            working_names(&session),
            vec!["ABCDEFGH", "THINGS", "GL_LEVEL", "GL_VERT"],
            "GL_LEVEL needs no rename, however long the new map name is",
        );
        seed(
            &mut session,
            vec![
                marker("MAP01"),
                carrying("THINGS", "x"),
                marker("GL_MAP01"),
                carrying("GL_VERT", "y"),
            ],
        );
        rename_run(&mut session, name("MAP01"), name("TOOLONG6")).expect("the run goes");
        assert_eq!(
            working_names(&session),
            vec![
                "ABCDEFGH", "THINGS", "GL_LEVEL", "GL_VERT", "TOOLONG6", "THINGS"
            ],
            "GL_<name> would not fit, so that block's GL run is dropped",
        );
    }

    #[test]
    fn rename_run_refuses_missing_colliding_and_self_renames() {
        let mut session = base_session(layer_map());
        let error = rename_run(&mut session, name("MAP01"), name("MAP09"))
            .expect_err("MAP01 is only in the layer");
        assert_eq!(code(&error), "not_found", "missing in working code");
        assert!(
            error.to_string().contains("copy_map MAP01 MAP09"),
            "the message names the tool and the fix: {error}"
        );
        seed(&mut session, vec![marker("MAP09"), carrying("THINGS", "x")]);
        let taken =
            rename_run(&mut session, name("MAP09"), name("MAP09")).expect_err("self rename");
        assert_eq!(code(&taken), "invalid_argument", "self rename code");
        seed(&mut session, vec![marker("MAP07"), carrying("THINGS", "y")]);
        assert_eq!(
            rename_run(&mut session, name("MAP09"), name("MAP07"))
                .expect_err("MAP07 exists")
                .code(),
            "already_exists",
            "existing target code",
        );
        assert_eq!(
            rename_run(&mut session, name("MAP07"), name("MAP06"))
                .expect("free name")
                .to,
            name("MAP06"),
            "the refusal above changed nothing that this rename builds on",
        );
    }

    #[test]
    fn rename_gl_renames_or_reports_the_run_it_drops() {
        let mut session = Session::default();
        seed(
            &mut session,
            vec![
                marker("MAP01"),
                carrying("THINGS", "t"),
                marker("GL_MAP01"),
                carrying("GL_VERT", "g"),
            ],
        );
        let analysis = analyze(&session.working().lumps);
        let gl = analysis.maps.first().expect("the block").gl;
        assert_eq!(gl, Some((2_usize, 4)), "the fixture run");
        let mut working = session.working().clone();
        let dropped = rename_gl(&mut working, gl, name("MAP02")).expect("renamed");
        assert_eq!(dropped, None, "GL_MAP02 fits");
        assert_eq!(
            working.lumps.get(2).expect("marker").name,
            name("GL_MAP02"),
            "the marker was renamed",
        );
        let mut again = session.working().clone();
        assert_eq!(
            rename_gl(&mut again, gl, name("LONGNAME")).expect("dropped"),
            Some((2_usize, 4)),
            "the oversized rename reports the run for removal",
        );
    }

    #[test]
    fn remove_run_removes_the_block_and_clears_the_active_map() {
        let mut session = base_session(layer_map());
        copy_run(&mut session, &copy_plan("MAP01", "MAP08", "merged", false)).expect("copy in");
        session.set_active_map(Some(name("MAP08")));
        let report = remove_run(&mut session, name("MAP08")).expect("removed");
        assert_eq!(
            value(&report),
            json!({"map": "MAP08", "lumps_removed": 5_u64}),
            "block plus GL run",
        );
        assert!(session.working().is_empty(), "everything went");
        assert_eq!(session.active_map(), None, "and the selection cleared");
        assert_eq!(
            session.undo().expect("undo the removal"),
            "remove_map MAP08",
            "the label comes back",
        );
        assert_eq!(
            working_names(&session),
            vec!["MAP08", "THINGS", "LINEDEFS", "GL_MAP08", "GL_VERT"],
            "undo brings the block back",
        );
        let layer =
            remove_run(&mut session, name("MAP01")).expect_err("the layer never loses maps");
        assert_eq!(code(&layer), "not_found", "layer map code");
        assert!(
            layer.to_string().contains("read-only"),
            "the message explains: {layer}"
        );
    }

    #[test]
    fn plans_validate_names_and_scopes() {
        let bad = plan_create(&create_args("", None, None)).expect_err("empty name");
        assert_eq!(code(&bad), "invalid_argument", "empty name code");
        let ns = plan_create(&create_args("MAP05", Some("doom"), Some("zdoom")))
            .expect_err("namespace on a binary map");
        assert_eq!(code(&ns), "invalid_argument", "namespace clash code");
        let plan = plan_create(&create_args("SLOTPAD", None, None)).expect("odd name");
        assert_eq!(plan.warnings.len(), 1, "the plan carries the warning");
        assert_eq!(plan.namespace, None, "binary default");
        let scope =
            plan_copy(&copy_args("MAP01", "MAP02", Some("nope"), false)).expect_err("bad scope");
        assert_eq!(code(&scope), "invalid_argument", "bad scope code");
        let copy = copy_plan("map01", "map02", "working", true);
        assert_eq!(
            (copy.from, copy.to, copy.scope),
            (name("MAP01"), name("MAP02"), Scope::Working),
            "names upper, scope parsed, flag carried",
        );
    }
}
