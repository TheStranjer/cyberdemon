//! Convert tools: [`Cyberdemon::convert_map_format`], the within-flag-family
//! map format conversion (see the `convert` router on
//! [`crate::server::Cyberdemon`]).
//!
//! The call is exactly one [`crate::session::Session::convert_map`] step, so
//! it copies a layer-owned block in, rewrites it and drops its node data as
//! a single undo entry; `dry_run` converts a copy of the merged view's block
//! and only reports. What a conversion carries over, drops and warns about is
//! the pure [`crate::wad::map::convert`] module's decision; this file only
//! resolves the map, turns the parameters into a
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
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::convert::{self, Conversion};
use crate::wad::name::LumpName;

/// The note every conversion carries: node data is written in the map's own
/// format, so it never survives a rewrite and has to be rebuilt.
const NODES_NOTE: &str = "the node data went with the old format (SEGS, SSECTORS, NODES, REJECT, \
                          BLOCKMAP, ZNODES and the GL run): run build_all_nodes before playing the \
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
    /// The dropped-field counts and the warnings, serialized into this object.
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
/// `format_error` for a block that does not decode, `unsupported` and
/// `invalid_argument` from the conversion itself, and `limit_exceeded` from
/// the target format's writer.
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

/// Hand an outcome to the client as the documented structured result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = convert_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Rewrites a map block in another format inside the same flag family.
    #[tool(
        description = "Rewrites one map block in another map format, as a single undoable edit, inside one flag family: binary Doom <-> UDMF, or binary Hexen <-> UDMF. map defaults to the active map (set_active_map, else the merged view's only map). to is \"doom\", \"hexen\" or \"udmf\" (case-insensitive). namespace is the namespace the UDMF TEXTMAP declares (1 to 32 ASCII letters, digits or underscores, lowercased); UDMF only, and its default follows the source's family: \"doom\" for a Doom family map, \"zdoom\" for a Hexen family one. A binary to with a namespace is invalid_argument. The Doom family is binary Doom and the UDMF namespaces doom, heretic and strife; the Hexen family is binary Hexen and hexen, zdoom, gzdoom, eternity and friends. Flags keep their meaning inside a family, so a request that changes family is unsupported (cross-family conversion needs the linedef and thing special translation, and that is a separate step) and a request naming the layout the map already has (same format and namespace, compared case-insensitively) is invalid_argument. What carries over: Doom <-> UDMF is lossless (the line tag rides on both id and arg0); Hexen -> UDMF turns each Line_SetIdentification (special 121) into a line id, moving args[0] into it and clearing the special, and keeps BEHAVIOR and SCRIPTS; UDMF -> Hexen does the reverse and drops an id that cannot be stored (above 255, or on a line that already has a special) with a warning, and the Hexen writer synthesizes the minimal BEHAVIOR the format needs when the block carries none; UDMF -> any binary format drops every unmodelled field, counted per element kind in dropped_fields, and reports how many elements had a fractional number already rounded on load in fractional_coordinates (a BEHAVIOR lump in a block converted to binary Doom drops with a warning, since that lump is what makes a block read as Hexen). Values outside the target format's ranges are limit_exceeded naming element and field, and nothing changes. Converting a UDMF map to another namespace of its own family rewrites the declaration only. Node data is written in the map's own format, so every conversion removes SEGS, SSECTORS, NODES, REJECT, BLOCKMAP, ZNODES and the block's GL run (nodes_removed true; run build_all_nodes before playing the map in vanilla Doom). dry_run true converts a copy and reports without touching the working archive or the undo stack (it still checks that the result writes). Returns {map, from, to, namespace (null for a binary layout), dropped_fields {things, linedefs, sidedefs, sectors, vertices}, fractional_coordinates {same keys}, warnings, nodes_removed, dry_run}. Unknown map is not_found, unparsable map or a bad to/namespace pair is invalid_argument, an undecodable block is format_error. See skill/reference/convert.md."
    )]
    fn convert_map_format(
        &self,
        Parameters(args): Parameters<ConvertMapFormatParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let target = match convert::Target::parse(&args.to, args.namespace.as_deref()) {
            Ok(target) => target,
            Err(error) => return fail(&error),
        };
        reported(self.state().with_session(|session| {
            let name = common::resolve_map_name(session, args.map.as_deref())?;
            convert_run(session, name, &target, args.dry_run)
        }))
    }
}
