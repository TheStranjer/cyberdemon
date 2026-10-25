//! `build_nodes`: rebuilding a map's `BSP` tree in each of the layouts.
//!
//! [`Variant`] is the caller's choice, [`NodeBuildPlan`] that choice with
//! the builder's two knobs. [`apply_build_nodes`] is the edit closure every
//! node build shares and the step [`super::build_all`] reuses: it rebuilds
//! the tree through [`build_tree`] and installs the planned layout, the
//! classic run with [`write_classic_run`] (the vanilla records from
//! [`to_vanilla`], the split vertices appended to `VERTEXES`), the
//! `ZDoom` extended lump with [`install_extended_nodes`] (one
//! [`to_extended`] lump in `NODES` of a binary block or in `ZNODES` of a
//! `UDMF` one) or the `gl` run through [`super::build_gl`]'s
//! [`install_gl_nodes`]. [`NodesBuilt`] carries whichever of the three was
//! written into [`NodesBuildBody`]; [`nodes_build_run`] resolves the
//! parameters and runs one undoable edit.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::bsp::{BspTree, BuildOptions, build_tree, to_extended, to_vanilla};
use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::edit_result_fragment;
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::name::LumpName;

use super::build_gl::{GlBuild, gl_edit_step, install_gl_nodes};
use super::build_tables::{NODES_AFTER, SEGS_AFTER, SSECTORS_AFTER, ZNODES_AFTER, install_lump};

/// Arguments for [`Cyberdemon::build_nodes`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct BuildNodesParams {
    /// The map to edit; defaults like [`GetNodesParams::map`].
    #[serde(default)]
    pub(super) map: Option<String>,
    /// The node layout to write: `"classic"` (the default: the vanilla
    /// `SEGS`/`SSECTORS`/`NODES` lumps, binary maps only), `"extended"`
    /// (`ZDoom`'s single `XNOD`/`ZNOD` lump, stored in `NODES` of a
    /// binary map or in `ZNODES` of a `UDMF` one) or `"gl"` (the classic
    /// lumps plus the `glBSP` v2 `GL_*` run for `OpenGL` ports, binary
    /// maps only).
    #[serde(default)]
    pub(super) variant: Option<String>,
    /// Cost added per seg a candidate partition splits (default `8`);
    /// raise it to avoid splitting segs, lower it to prefer it.
    #[serde(default)]
    pub(super) split_cost: Option<i64>,
    /// Compress the extended lump with `zlib` behind the `ZNOD` magic
    /// (the default, `true`); `false` stores it raw behind `XNOD`. Only
    /// meaningful for variant `"extended"`.
    #[serde(default)]
    pub(super) compress: Option<bool>,
}

/// The `build_nodes` default for the per-split cost.
pub(super) const DEFAULT_SPLIT_COST: i64 = 8_i64;

/// Which node layout [`Cyberdemon::build_nodes`] should write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Variant {
    /// The vanilla `SEGS`/`SSECTORS`/`NODES` record lumps.
    Classic,
    /// `ZDoom`'s extended lump, in `NODES` or `ZNODES`.
    Extended,
    /// The `glBSP` `GL_*` run, written alongside the classic lumps.
    Gl,
}

/// The node part of one builder plan: the layout to write and the
/// builder's two knobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NodeBuildPlan {
    /// The node layout to write.
    pub(super) variant: Variant,
    /// Cost added per seg a candidate partition splits.
    pub(super) split_cost: i64,
    /// Whether an extended lump is `zlib`-compressed behind the `ZNOD`
    /// magic (`false` stores it raw behind `XNOD`).
    pub(super) compress: bool,
}

/// A node layout this build can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Implemented {
    /// The vanilla `SEGS`/`SSECTORS`/`NODES` record lumps.
    Classic,
    /// The `ZDoom` extended lump.
    Extended,
    /// The classic lumps plus the `glBSP` `GL_*` run.
    Gl,
}

impl Variant {
    /// The accepted `variant` text, `classic` being the default.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the three accepted values.
    pub(super) fn parse(text: Option<&str>) -> Result<Self, CyberError> {
        let lowered = match text {
            None => return Ok(Self::Classic),
            Some(given) => given.to_ascii_lowercase(),
        };
        match lowered.as_str() {
            "" | "classic" => Ok(Self::Classic),
            "extended" => Ok(Self::Extended),
            "gl" => Ok(Self::Gl),
            other => Err(CyberError::InvalidArgument(format!(
                "variant {other:?} is neither \"classic\", \"extended\" nor \"gl\"; \
                 pass one of those three or leave the parameter out"
            ))),
        }
    }

    /// The layout this build writes.
    pub(super) const fn check(self) -> Implemented {
        match self {
            Self::Classic => Implemented::Classic,
            Self::Extended => Implemented::Extended,
            Self::Gl => Implemented::Gl,
        }
    }
}

/// What one classic node build wrote, before the edit fragment joins it.
#[derive(Debug)]
pub(super) struct ClassicBuild {
    /// Segs stored.
    segs: usize,
    /// Subsectors stored.
    subsectors: usize,
    /// Interior nodes stored.
    nodes: usize,
    /// Vertices appended to `VERTEXES` for seg splits.
    split_vertices: usize,
    /// The builder's per-leaf warnings.
    warnings: Vec<String>,
}

/// Refuse the layouts a `UDMF` block cannot carry.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] on a `UDMF` block.
pub(super) fn require_binary(map: &Map) -> Result<(), CyberError> {
    if map.format == MapFormat::Udmf {
        return Err(CyberError::InvalidArgument(
            "UDMF maps use extended nodes: the classic SEGS/SSECTORS/NODES run \
             has no place in a TEXTMAP block; build variant \"extended\""
                .to_owned(),
        ));
    }
    Ok(())
}

/// Install a finished tree as the vanilla run: the three record lumps and
/// the split vertices its seg endpoints need, appended after the map's.
pub(super) fn write_classic_run(map: &mut Map, tree: &BspTree) -> Result<ClassicBuild, CyberError> {
    let run = to_vanilla(map, tree)?;
    let built = ClassicBuild {
        segs: tree.segs.len(),
        subsectors: tree.subsectors.len(),
        nodes: tree.nodes.len(),
        split_vertices: run.new_vertices.len(),
        warnings: tree.warnings.clone(),
    };
    map.vertices.extend(run.new_vertices);
    install_lump(
        map,
        LumpName::from_raw(*b"SEGS\0\0\0\0"),
        &run.segs,
        &SEGS_AFTER,
    );
    install_lump(
        map,
        LumpName::from_raw(*b"SSECTORS"),
        &run.ssectors,
        &SSECTORS_AFTER,
    );
    install_lump(
        map,
        LumpName::from_raw(*b"NODES\0\0\0"),
        &run.nodes,
        &NODES_AFTER,
    );
    Ok(built)
}

/// The edit closure of [`Cyberdemon::build_nodes`] variant `"classic"`:
/// rebuild the tree and install the three vanilla lumps, appending the
/// split vertices the seg endpoints need after the map's own.
fn install_classic_nodes(map: &mut Map, split_cost: i64) -> Result<ClassicBuild, CyberError> {
    require_binary(map)?;
    let tree = build_tree(
        map,
        BuildOptions {
            split_cost,
            ..BuildOptions::default()
        },
    )?;
    write_classic_run(map, &tree)
}

/// What one [`apply_build_nodes`] wrote, in whichever layout was chosen.
pub(super) enum NodesBuilt {
    /// The vanilla run's counts.
    Classic(ClassicBuild),
    /// The extended lump's counts.
    Extended(ExtendedBuild),
    /// The classic counts plus the `GL_*` lumps waiting for the archive.
    Gl(Box<GlBuild>),
}

impl NodesBuilt {
    /// Fold the build's counts into the report body.
    pub(super) fn into_body(self) -> NodesBuildBody {
        match self {
            Self::Classic(built) => classic_body(built),
            Self::Extended(built) => extended_body(built),
            Self::Gl(built) => gl_body(*built),
        }
    }

    /// The build's counts plus the standard edit fragment.
    fn into_report(self, edit: JsonMap<String, Value>) -> NodesBuildReport {
        NodesBuildReport {
            body: self.into_body(),
            edit,
        }
    }
}

/// The node edit step every node build shares: rebuild the tree from the
/// map's current geometry and install the planned layout. A `gl` build's
/// `GL_*` lumps come out with the value: the block lives outside the map,
/// so the caller splices it into the archive with [`install_gl_run`] in
/// the same [`Session::edit`].
pub(super) fn apply_build_nodes(
    map: &mut Map,
    plan: NodeBuildPlan,
) -> Result<NodesBuilt, CyberError> {
    match plan.variant.check() {
        Implemented::Classic => {
            install_classic_nodes(map, plan.split_cost).map(NodesBuilt::Classic)
        }
        Implemented::Extended => {
            install_extended_nodes(map, plan.split_cost, plan.compress).map(NodesBuilt::Extended)
        }
        Implemented::Gl => {
            install_gl_nodes(map, plan.split_cost).map(|built| NodesBuilt::Gl(Box::new(built)))
        }
    }
}

/// What one extended node build wrote, before the edit fragment joins it.
#[derive(Debug)]
pub(super) struct ExtendedBuild {
    /// The lump magic written: `"ZNOD"` or `"XNOD"`.
    format: &'static str,
    /// Bytes of the written lump.
    bytes: usize,
    /// Segs stored.
    segs: usize,
    /// Subsectors stored.
    subsectors: usize,
    /// Interior nodes stored.
    nodes: usize,
    /// Split vertices stored inside the lump, beyond the untouched
    /// `VERTEXES`.
    new_vertices: usize,
    /// The builder's per-leaf warnings.
    warnings: Vec<String>,
}

/// The edit closure of [`Cyberdemon::build_nodes`] variant `"extended"`:
/// rebuild the tree and install one `ZDoom` extended lump — into `NODES`
/// of a binary block, with `SEGS` and `SSECTORS` emptied (`ZDoom`
/// ignores them when `NODES` is extended), or into a `ZNODES` lump of a
/// `UDMF` block, dropping a stale `NODES`/`SEGS`/`SSECTORS` run. The
/// map's vertices are never modified.
pub(super) fn install_extended_nodes(
    map: &mut Map,
    split_cost: i64,
    compress: bool,
) -> Result<ExtendedBuild, CyberError> {
    let tree = build_tree(
        map,
        BuildOptions {
            split_cost,
            ..BuildOptions::default()
        },
    )?;
    let run = to_extended(map, &tree, compress)?;
    let built = ExtendedBuild {
        format: run.format,
        bytes: run.bytes.len(),
        segs: tree.segs.len(),
        subsectors: tree.subsectors.len(),
        nodes: tree.nodes.len(),
        new_vertices: run.new_vertices,
        warnings: tree.warnings,
    };
    if map.format == MapFormat::Udmf {
        map.other.retain(|lump| {
            !matches!(
                lump.name.as_bytes(),
                b"SEGS" | b"SSECTORS" | b"NODES" | b"ZNODES"
            )
        });
        install_lump(
            map,
            LumpName::from_raw(*b"ZNODES\0\0"),
            &run.bytes,
            &ZNODES_AFTER,
        );
    } else {
        install_lump(
            map,
            LumpName::from_raw(*b"NODES\0\0\0"),
            &run.bytes,
            &NODES_AFTER,
        );
        install_lump(map, LumpName::from_raw(*b"SEGS\0\0\0\0"), b"", &SEGS_AFTER);
        install_lump(map, LumpName::from_raw(*b"SSECTORS"), b"", &SSECTORS_AFTER);
    }
    Ok(built)
}

/// What one nodes build wrote, before the edit fragment joins it.
#[derive(Debug, Serialize)]
pub(super) struct NodesBuildBody {
    /// The layout that was built: `"classic"`, `"extended"` or `"gl"`.
    pub(super) variant: &'static str,
    /// The magic of the lump an extended build wrote (`"ZNOD"` or
    /// `"XNOD"`); only in the extended result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) format: Option<&'static str>,
    /// Bytes of the lump an extended build wrote; only in the extended
    /// result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) bytes: Option<usize>,
    /// Segs written.
    pub(super) segs: usize,
    /// Subsectors written.
    pub(super) subsectors: usize,
    /// Interior nodes written; `0` when the whole map is one subsector.
    pub(super) nodes: usize,
    /// Vertices appended to `VERTEXES` for seg splits; only in the
    /// classic result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) split_vertices: Option<usize>,
    /// Split vertices stored inside the extended lump, beyond the
    /// untouched `VERTEXES`; only in the extended result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) new_vertices: Option<usize>,
    /// Segs in the `GL_SEGS` lump, real and mini; only in the `gl` result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) gl_segs: Option<usize>,
    /// Minisegs among the `GL_SEGS` records; only in the `gl` result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) minisegs: Option<usize>,
    /// Fraction-precise vertices written to `GL_VERT`; only in the `gl`
    /// result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) gl_vertices: Option<usize>,
    /// The builder's warnings, one per subsector spanning several sectors.
    pub(super) warnings: Vec<String>,
}

/// The classic body: the build's counts.
fn classic_body(built: ClassicBuild) -> NodesBuildBody {
    NodesBuildBody {
        variant: "classic",
        format: None,
        bytes: None,
        segs: built.segs,
        subsectors: built.subsectors,
        nodes: built.nodes,
        split_vertices: Some(built.split_vertices),
        new_vertices: None,
        gl_segs: None,
        minisegs: None,
        gl_vertices: None,
        warnings: built.warnings,
    }
}

/// The extended body: the build's counts.
fn extended_body(built: ExtendedBuild) -> NodesBuildBody {
    NodesBuildBody {
        variant: "extended",
        format: Some(built.format),
        bytes: Some(built.bytes),
        segs: built.segs,
        subsectors: built.subsectors,
        nodes: built.nodes,
        split_vertices: None,
        new_vertices: Some(built.new_vertices),
        gl_segs: None,
        minisegs: None,
        gl_vertices: None,
        warnings: built.warnings,
    }
}

/// The `gl` body: the classic counts and the `GL_*` lump counts.
fn gl_body(built: GlBuild) -> NodesBuildBody {
    NodesBuildBody {
        variant: "gl",
        format: None,
        bytes: None,
        segs: built.classic.segs,
        subsectors: built.classic.subsectors,
        nodes: built.classic.nodes,
        split_vertices: Some(built.classic.split_vertices),
        new_vertices: None,
        gl_segs: Some(built.run.seg_count),
        minisegs: Some(built.run.minisegs),
        gl_vertices: Some(built.run.new_vertices),
        warnings: built.classic.warnings,
    }
}

/// Structured result of [`Cyberdemon::build_nodes`].
#[derive(Debug, Serialize)]
pub(super) struct NodesBuildReport {
    /// What the build wrote, in the chosen layout.
    #[serde(flatten)]
    pub(super) body: NodesBuildBody,
    /// The standard edit fragment.
    #[serde(flatten)]
    pub(super) edit: JsonMap<String, Value>,
}

/// `build_nodes`' work: resolve the variant, the map and the cost, run
/// the one edit step in the layout the caller chose.
pub(super) fn nodes_build_run(
    session: &mut Session,
    args: &BuildNodesParams,
) -> Result<NodesBuildReport, CyberError> {
    let variant = Variant::parse(args.variant.as_deref())?;
    let plan = NodeBuildPlan {
        variant,
        split_cost: args.split_cost.unwrap_or(DEFAULT_SPLIT_COST),
        compress: args.compress.unwrap_or(true),
    };
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let (built, report) = if variant == Variant::Gl {
        let merged = session.merged();
        session.edit("build nodes", |wad| gl_edit_step(wad, &merged, name, plan))?
    } else {
        session.edit_map(name, "build nodes", |map| apply_build_nodes(map, plan))?
    };
    Ok(built.into_report(edit_result_fragment(&report)))
}
