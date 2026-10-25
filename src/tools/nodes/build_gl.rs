//! The `gl` variant: the `glBSP` v2 run spliced around a map block.
//!
//! A `gl` build writes the classic lumps *and* a `GL_*` run, and the run
//! cannot live inside the map block, so it is the one node output an edit
//! closure cannot place on its own. [`install_gl_nodes`] builds the tree
//! once, installs the classic run through [`write_classic_run`] and
//! serializes the same tree with [`to_gl`]; [`gl_edit_step`] runs that
//! rewrite and [`splice_gl_run`] attaches the bytes, [`install_gl_run`]
//! replacing an existing run in place or inserting the six lumps named by
//! [`gl_lump_names`] right after the block at [`gl_insert_point`]'s
//! position — all of it inside one [`crate::session::Session::edit`] entry.

use crate::bsp::{BuildOptions, GlNodes, build_tree, to_gl};
use crate::error::CyberError;
use crate::session::MergedLump;
use crate::session::map_edit::{self, MapEditReport};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Analysis, analyze};
use crate::wad::map::Map;
use crate::wad::name::LumpName;

use super::build_nodes::{
    ClassicBuild, NodeBuildPlan, NodesBuilt, apply_build_nodes, require_binary, write_classic_run,
};

/// What one `gl` variant build wrote: the classic run's report and the
/// `glBSP` v2 lumps waiting for the archive.
#[derive(Debug)]
pub(super) struct GlBuild {
    /// The classic counts installed alongside (the map plays everywhere).
    pub(super) classic: ClassicBuild,
    /// The `GL_*` record lumps.
    pub(super) run: GlNodes,
}

/// The edit closure of [`Cyberdemon::build_nodes`] variant `"gl"`: build
/// the tree once, install the classic run (so vanilla `Doom` and every
/// port play the map) and serialize the same tree as the `glBSP` v2
/// lumps. The `GL_*` block itself lives outside the map block, so the
/// caller splices it into the archive around this edit.
pub(super) fn install_gl_nodes(map: &mut Map, split_cost: i64) -> Result<GlBuild, CyberError> {
    require_binary(map)?;
    let tree = build_tree(
        map,
        BuildOptions {
            split_cost,
            ..BuildOptions::default()
        },
    )?;
    let classic = write_classic_run(map, &tree)?;
    let run = to_gl(map, &tree)?;
    Ok(GlBuild { classic, run })
}

/// The six lump names of one map's `GL` run: the `GL_<MAP>` marker, or
/// `GL_LEVEL` when the map name leaves no room to embed it.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] should a derived name not parse.
pub(super) fn gl_lump_names(map: LumpName) -> Result<[LumpName; 6], CyberError> {
    let map_name = map.to_string();
    let head = if map.len() > 5 {
        "GL_LEVEL".to_owned()
    } else {
        format!("GL_{map_name}")
    };
    Ok([
        LumpName::parse(&head)?,
        LumpName::parse("GL_VERT")?,
        LumpName::parse("GL_SEGS")?,
        LumpName::parse("GL_SSECT")?,
        LumpName::parse("GL_NODES")?,
        LumpName::parse("GL_PVS")?,
    ])
}

/// The one edit step of a `gl` build: the classic-plus-GL map rewrite
/// through [`map_edit::edit_in_place`], then the `GL_*` block spliced in
/// behind the map block — one undo entry carrying both.
///
/// # Errors
///
/// [`map_edit::edit_in_place`] and [`install_gl_run`]'s refusals.
pub(super) fn gl_edit_step(
    wad: &mut Wad,
    merged: &[MergedLump],
    name: LumpName,
    plan: NodeBuildPlan,
) -> Result<(NodesBuilt, MapEditReport), CyberError> {
    let (built, report) =
        map_edit::edit_in_place(wad, merged, name, |map| apply_build_nodes(map, plan))?;
    let spliced = splice_gl_run(wad, name, built)?;
    Ok((spliced, report))
}

/// Splice the `GL_*` block of a `gl` build in behind `wad`'s map block
/// (the run lives outside the map) and hand the build back: the node
/// builders call this right after the map rewrite, inside the same
/// [`crate::session::Session::edit`].
///
/// # Errors
///
/// [`install_gl_run`]'s refusals.
pub(super) fn splice_gl_run(
    wad: &mut Wad,
    name: LumpName,
    built: NodesBuilt,
) -> Result<NodesBuilt, CyberError> {
    match built {
        NodesBuilt::Gl(gl) => {
            install_gl_run(wad, name, &gl.run)?;
            Ok(NodesBuilt::Gl(gl))
        }
        kept @ (NodesBuilt::Classic(_) | NodesBuilt::Extended(_)) => Ok(kept),
    }
}

/// Attach `run` to `wad`'s block named `map`: an existing `GL` run is
/// replaced in place, otherwise the block is inserted right after the
/// map block. The map block has just been re-encoded by the caller's
/// [`map_edit::edit_in_place`], so the directory is read as it now stands.
///
/// # Errors
///
/// [`gl_lump_names`]'s refusals and [`CyberError::NotFound`] when the
/// archive holds no such block (it was just edited: only a bug gets here).
fn install_gl_run(wad: &mut Wad, map: LumpName, run: &GlNodes) -> Result<(), CyberError> {
    let [head, vert, segs, ssect, nodes, pvs] = gl_lump_names(map)?;
    let lumps = vec![
        Lump::marker(head),
        Lump::new(vert, run.vertices.clone()),
        Lump::new(segs, run.segs.clone()),
        Lump::new(ssect, run.ssectors.clone()),
        Lump::new(nodes, run.nodes.clone()),
        Lump::new(pvs, run.pvs.clone()),
    ];
    let analysis = analyze(&wad.lumps);
    let (start, stop) = gl_insert_point(&analysis, map)?;
    wad.lumps.drain(start..stop);
    wad.lumps.splice(start..start, lumps);
    Ok(())
}

/// Where the map's `GL` run goes: its current range when it has one, the
/// position right after the block otherwise.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming `map` when the directory holds no
/// such block.
fn gl_insert_point(analysis: &Analysis, map: LumpName) -> Result<(usize, usize), CyberError> {
    let Some(block) = analysis.maps.iter().rev().find(|block| block.name == map) else {
        return Err(CyberError::NotFound(format!(
            "no map block named {map} in the working archive: build nodes \
             on a map the session holds"
        )));
    };
    Ok(block.gl.unwrap_or((block.end, block.end)))
}
