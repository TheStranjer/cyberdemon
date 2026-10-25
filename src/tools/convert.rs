//! Convert tools: [`Cyberdemon::convert_map_format`], the map format
//! conversion within and across the flag families (see the `convert` router
//! on [`crate::server::Cyberdemon`]).
//!
//! The call is exactly one [`crate::session::Session::convert_map`] step, so
//! it copies a layer-owned block in, rewrites it and drops its node data as
//! a single undo entry; `dry_run` converts a copy of the merged view's block
//! and only reports. Inside one flag family the step is mechanical; across
//! the families it translates linedef specials, thing flags and sector
//! types best-effort through the embedded `ZDoom` table in whichever
//! direction the request asks. What a conversion carries over, drops and
//! warns about is the pure [`crate::wad::map::convert`] module's decision;
//! this file only resolves the map, turns the parameters into a
//! [`crate::wad::map::convert::Target`] and assembles the result.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::tools::maps::common;
use crate::tools::{fail, reply};
use crate::wad::classify::MapFormat;
use crate::wad::map::convert::{self, Conversion};
use crate::wad::name::LumpName;

/// The note every conversion carries: node data is written in the map's own
/// format, so it never survives a rewrite and has to be rebuilt.
const NODES_NOTE: &str = "the node data went with the old format (SEGS, SSECTORS, NODES, REJECT, \
                          BLOCKMAP, ZNODES and the GL run): run build_nodes before playing the \
                          map in vanilla Doom";

/// Arguments for [`Cyberdemon::convert_map_format`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ConvertMapFormatParams {
    /// The map to convert, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map when it holds
    /// exactly one. Without either the call fails with `invalid_argument`.
    #[serde(default)]
    map: Option<String>,
    /// The layout to write: `"doom"`, `"hexen"` or `"udmf"`, case-insensitive.
    to: String,
    /// The namespace the `UDMF` `TEXTMAP` declares (1 to 32 ASCII letters,
    /// digits or underscores, lowercased). `UDMF` only: the default is
    /// `"doom"` when the source is a Doom family map and `"zdoom"` when it is
    /// a Hexen family one. Passing it with a binary `to` fails with
    /// `invalid_argument`.
    #[serde(default)]
    namespace: Option<String>,
    /// Convert a copy and report what would change, touching neither the
    /// working archive nor the undo stack (default `false`).
    #[serde(default)]
    dry_run: bool,
}

/// What one conversion step did, before the result is assembled.
struct Step {
    /// The layout the block was read in.
    from: MapFormat,
    /// The namespace the converted block declares; `None` for a binary one.
    namespace: Option<String>,
    /// What the step dropped, and everything it warns about.
    conversion: Conversion,
}

/// Structured result of [`Cyberdemon::convert_map_format`].
#[derive(Serialize)]
struct ConvertReport {
    /// The map block that was (or, in a dry run, would be) rewritten.
    map: LumpName,
    /// The layout it was read in.
    from: MapFormat,
    /// The layout it was written in.
    to: MapFormat,
    /// The namespace the `UDMF` `TEXTMAP` declares; `null` for a binary one.
    namespace: Option<String>,
    /// The translation and dropped-field counts and the warnings,
    /// serialized into this object.
    #[serde(flatten)]
    conversion: Conversion,
    /// Whether the block's generated node data was removed.
    nodes_removed: bool,
    /// Whether nothing changed because the call only reported.
    dry_run: bool,
}

/// `convert_map_format`'s work: the dry run converts a copy of the merged
/// view's block, the real run rewrites the block as one undoable step.
fn convert_run(
    session: &mut Session,
    map: LumpName,
    target: &convert::Target,
    dry_run: bool,
) -> Result<ConvertReport, CyberError> {
    let step = if dry_run {
        dry_step(session, map, target)?
    } else {
        apply_step(session, map, target)?
    };
    Ok(assembled(map, target, step, dry_run))
}

/// Rewrites the block in the working archive as one undoable step.
///
/// # Errors
///
/// [`Session::convert_map`]'s errors: `not_found` for a map no archive holds,
/// `format_error` for a block that does not decode, `invalid_argument` from
/// the conversion itself and `limit_exceeded` from the target format's
/// writer.
fn apply_step(
    session: &mut Session,
    map: LumpName,
    target: &convert::Target,
) -> Result<Step, CyberError> {
    let label = format!("convert {map} to {}", convert::format_name(target.format));
    let (step, _report) = session.convert_map(map, &label, |model| {
        let from = model.format;
        let conversion = convert::convert_map(model, target)?;
        Ok(Step {
            from,
            namespace: model.udmf_namespace.clone(),
            conversion,
        })
    })?;
    Ok(step)
}

/// Converts a copy of the merged view's block and checks that it writes, so a
/// dry run reports the refusals the real one would have hit.
///
/// # Errors
///
/// [`common::load_map`]'s and [`convert::convert_map`]'s errors, plus the
/// target writer's refusal for a value it cannot store.
fn dry_step(
    session: &Session,
    map: LumpName,
    target: &convert::Target,
) -> Result<Step, CyberError> {
    let mut copy = common::load_map(session, map, Scope::Merged)?;
    let from = copy.format;
    let conversion = convert::convert_map(&mut copy, target)?;
    convert::encodes(&copy)?;
    Ok(Step {
        from,
        namespace: copy.udmf_namespace,
        conversion,
    })
}

/// Assembles the result: the step's counts and notices, with the node-data
/// hint every conversion earns.
fn assembled(map: LumpName, target: &convert::Target, step: Step, dry_run: bool) -> ConvertReport {
    let mut conversion = step.conversion;
    conversion.warnings.push(NODES_NOTE.to_owned());
    ConvertReport {
        map,
        from: step.from,
        to: target.format,
        namespace: step.namespace,
        conversion,
        nodes_removed: true,
        dry_run,
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = convert_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Rewrites a map block in another format, translating across flag
    /// families best-effort.
    #[tool(
        description = "Rewrites one map block in another format (doom, hexen, udmf, or the extended ZDoom node layouts) as a single undoable edit; within one flag family the step is mechanical and across the Doom and Hexen families specials and fields translate through the embedded tables, every translation reported. dry_run converts a copy and only reports. Use when a port needs another format."
    )]
    fn convert_map_format(
        &self,
        Parameters(args): Parameters<ConvertMapFormatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let target = match convert::Target::parse(&args.to, args.namespace.as_deref()) {
            Ok(target) => target,
            Err(error) => return fail(&error),
        };
        reply(self.state().with_session(|session| {
            let name = common::resolve_map_name(session, args.map.as_deref())?;
            convert_run(session, name, &target, args.dry_run)
        }))
    }
}
