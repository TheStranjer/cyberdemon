//! Multi-kind geometry editing: `delete_geometry` and
//! `transform_selection` (see the `edit_geometry` router methods on
//! [`crate::server::Cyberdemon`]).
//!
//! `transform_selection` moves a mixed selection of vertices and things
//! through one scale/rotate/translate. The math and the selection
//! expansion live in the pure engine [`crate::wad::map::transform`]; this
//! file parses the request, expands the selection against the map as it
//! now stands (so a bad index or an empty selection is refused before
//! anything is copied in) and runs the whole transform inside exactly one
//! [`crate::session::Session::edit_map`] step, which rolls the map back
//! untouched when the engine refuses an out-of-range coordinate or a
//! collapsing linedef.
//!
//! One call deletes a mixed selection of vertices, linedefs, sidedefs,
//! sectors and things with full reference cleanup. The file is a thin
//! wrapper around the pure removal engine
//! ([`crate::wad::map::remove`]): it validates the selection, builds a
//! [`Removal`], grows it with [`expand_cascade`] — the cascade always runs,
//! unlike the per-kind tools' opt-in `cascade` — and hands the grown set to
//! [`apply_removal`] inside exactly one
//! [`crate::session::Session::edit_map`] step, so one undo entry moves the
//! whole call. `dry_run` computes the same cascade on a copy and only
//! reports: nothing is written, no undo entry is made.
//!
//! The geometry rule: deleting any vertex, line, side or sector changes
//! geometry, so the generated node lumps are emptied (and the trailing
//! unreferenced ones, split points a `build_nodes` left behind, are
//! stripped with them), the block's `GL` run is dropped and the result
//! reports `nodes_invalidated: true`. Run `build_nodes` before playing such
//! a map in vanilla `Doom`; `ZDoom`-family ports rebuild the missing node
//! data themselves. A selection of things alone changes no buildable
//! geometry and keeps the nodes.

use alloc::collections::BTreeSet;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::map_edit::MapEditReport;
use crate::session::{Scope, Session};
use crate::tools::maps::common;
use crate::tools::maps::params::edit_result_fragment;
use crate::tools::{fail, ok};
use crate::wad::map::Map;
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, expand_cascade,
};
use crate::wad::map::transform::{
    Operations, Point, Rotation, Scale, SelectionRequest, TransformReport, apply_transform,
    expand_selection,
};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::delete_geometry`].
#[derive(Debug, Deserialize, JsonSchema)]
struct DeleteGeometryParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// Indices into the map's vertex block to delete.
    #[serde(default)]
    vertices: Option<Vec<usize>>,
    /// Indices into the map's linedef block to delete.
    #[serde(default)]
    linedefs: Option<Vec<usize>>,
    /// Indices into the map's sidedef block to delete.
    #[serde(default)]
    sidedefs: Option<Vec<usize>>,
    /// Indices into the map's sector block to delete.
    #[serde(default)]
    sectors: Option<Vec<usize>>,
    /// Indices into the map's thing block to delete.
    #[serde(default)]
    things: Option<Vec<usize>>,
    /// Within the cascade, also remove sidedefs no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    prune_sidedefs: Option<bool>,
    /// Within the cascade, also remove vertices no surviving linedef
    /// references any more (default `true`).
    #[serde(default)]
    prune_vertices: Option<bool>,
    /// Compute the whole cascade on a copy and report it, touching neither
    /// the working archive nor the undo stack (default `false`).
    #[serde(default)]
    dry_run: bool,
}

/// Structured result of [`Cyberdemon::delete_geometry`].
#[derive(Serialize)]
struct DeleteReport {
    /// What the removal engine dropped, flipped and normalised.
    removed: RemovalReport,
    /// Whether nothing changed because the call only reported.
    dry_run: bool,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The request's five index lists as slices, absent ones empty, in the
/// order the blocks are checked.
fn selected_lists(args: &DeleteGeometryParams) -> [&[usize]; 5] {
    [
        args.vertices.as_deref().unwrap_or_default(),
        args.linedefs.as_deref().unwrap_or_default(),
        args.sidedefs.as_deref().unwrap_or_default(),
        args.sectors.as_deref().unwrap_or_default(),
        args.things.as_deref().unwrap_or_default(),
    ]
}

/// Whether the request names at least one index in any of its lists.
fn has_selection(args: &DeleteGeometryParams) -> bool {
    selected_lists(args)
        .into_iter()
        .any(|list| !list.is_empty())
}

/// The requested indices of one list as a set; duplicates collapse.
fn index_set(indices: Option<&Vec<usize>>) -> BTreeSet<usize> {
    indices.map_or_else(BTreeSet::new, |list| list.iter().copied().collect())
}

/// The seed removal of exactly the selected indices.
fn seed_removal(args: &DeleteGeometryParams) -> Removal {
    Removal {
        vertices: index_set(args.vertices.as_ref()),
        linedefs: index_set(args.linedefs.as_ref()),
        sidedefs: index_set(args.sidedefs.as_ref()),
        sectors: index_set(args.sectors.as_ref()),
        things: index_set(args.things.as_ref()),
    }
}

/// The number of seeded indices across all five kinds.
fn removal_size(removal: &Removal) -> usize {
    removal
        .vertices
        .len()
        .saturating_add(removal.linedefs.len())
        .saturating_add(removal.sidedefs.len())
        .saturating_add(removal.sectors.len())
        .saturating_add(removal.things.len())
}

/// The pruning flags of a call, the cascading default when either is
/// absent.
fn cascade_options(args: &DeleteGeometryParams) -> CascadeOptions {
    CascadeOptions {
        prune_sidedefs: args.prune_sidedefs.unwrap_or(true),
        prune_vertices: args.prune_vertices.unwrap_or(true),
    }
}

/// Refuses unless every index of one kind exists, naming the first that
/// does not the way the shared validator phrases it (`"vertex 9 does not
/// exist (map has 6 vertices)"`). `plural` is spelled per kind because
/// [`check_index`] only ever appends an `s`, which "vertex" does not take.
///
/// [`check_index`]: crate::tools::maps::params::check_index
fn check_kind(
    kind: &str,
    plural: &str,
    indices: &BTreeSet<usize>,
    count: usize,
) -> Result<(), CyberError> {
    for index in indices {
        if *index >= count {
            let range = count.checked_sub(1).map_or_else(
                || format!("the map has no {plural}"),
                |last| format!("the map has {count} {plural}, valid indices are 0..={last}"),
            );
            return Err(
                CyberError::NotFound(format!("{kind} {index} does not exist ({range})"))
                    .with_hint("describe_map or find_elements returns indices"),
            );
        }
    }
    Ok(())
}

/// Refuses unless every selected index exists in its block of `map`,
/// kinds in vertex, linedef, sidedef, sector, thing order.
fn check_selection(map: &Map, removal: &Removal) -> Result<(), CyberError> {
    check_kind("vertex", "vertices", &removal.vertices, map.vertices.len())?;
    check_kind("linedef", "linedefs", &removal.linedefs, map.linedefs.len())?;
    check_kind("sidedef", "sidedefs", &removal.sidedefs, map.sidedefs.len())?;
    check_kind("sector", "sectors", &removal.sectors, map.sectors.len())?;
    check_kind("thing", "things", &removal.things, map.things.len())
}

/// The deletion step: check the selection, grow it with the always-on
/// cascade and run the removal, returning what the engine did.
fn delete_in_place(
    map: &mut Map,
    opts: CascadeOptions,
    removal: &Removal,
) -> Result<RemovalReport, CyberError> {
    check_selection(map, removal)?;
    let grown = expand_cascade(map, removal, opts);
    apply_removal(map, &grown)
}

/// The map as the next edit sees it: the working archive's block when it
/// holds one, else the merged view's (which is what the edit copies in).
///
/// # Errors
///
/// [`common::load_map`]'s errors for the merged scope when the working
/// archive holds no such block.
fn current_map(session: &Session, name: LumpName) -> Result<Map, CyberError> {
    match common::load_map(session, name, Scope::Working) {
        Ok(map) => Ok(map),
        Err(error) if error.code() == "not_found" => common::load_map(session, name, Scope::Merged),
        Err(error) => Err(error),
    }
}

/// The edit fragment of a call that changed nothing: the map's name with
/// both flags `false`.
fn untouched_fragment(name: LumpName) -> JsonMap<String, Value> {
    edit_result_fragment(&MapEditReport {
        map: name,
        copied_to_working: false,
        nodes_invalidated: false,
    })
}

/// The `invalid_argument` when no list carries an index.
fn empty_selection() -> CyberError {
    CyberError::InvalidArgument(
        "nothing selected: pass at least one non-empty list among vertices, linedefs, \
         sidedefs, sectors and things"
            .to_owned(),
    )
}

/// `delete_geometry`'s work: validate the selection, then run the cascade
/// as one undoable step, or compute it on a copy for a dry run.
fn delete_run(
    session: &mut Session,
    args: &DeleteGeometryParams,
) -> Result<DeleteReport, CyberError> {
    if !has_selection(args) {
        return Err(empty_selection());
    }
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let removal = seed_removal(args);
    let opts = cascade_options(args);
    if args.dry_run {
        return dry_delete(session, name, opts, &removal);
    }
    let label = format!("delete {} map elements", removal_size(&removal));
    let (removed, report) =
        session.edit_map(name, &label, |map| delete_in_place(map, opts, &removal))?;
    Ok(DeleteReport {
        removed,
        dry_run: false,
        edit: edit_result_fragment(&report),
    })
}

/// The dry run: the same step computed on a loaded copy, reported with an
/// untouched fragment and no write.
fn dry_delete(
    session: &Session,
    name: LumpName,
    opts: CascadeOptions,
    removal: &Removal,
) -> Result<DeleteReport, CyberError> {
    let mut copy = current_map(session, name)?;
    let removed = delete_in_place(&mut copy, opts, removal)?;
    Ok(DeleteReport {
        removed,
        dry_run: true,
        edit: untouched_fragment(name),
    })
}

/// The `selection` parameter of [`Cyberdemon::transform_selection`]: the
/// index lists and expansion switches, before anything is resolved
/// against a map.
#[derive(Debug, Deserialize, JsonSchema)]
struct SelectionInput {
    /// Indices into the map's vertex block to move.
    #[serde(default)]
    vertices: Option<Vec<usize>>,
    /// Indices into the map's linedef block; each drags in its two
    /// endpoints.
    #[serde(default)]
    linedefs: Option<Vec<usize>>,
    /// Indices into the map's sector block; each drags in the vertices of
    /// all its boundary linedefs and, by default, its things.
    #[serde(default)]
    sectors: Option<Vec<usize>>,
    /// Indices into the map's thing block to move.
    #[serde(default)]
    things: Option<Vec<usize>>,
    /// Select every vertex and thing of the map; with it set the index
    /// lists add nothing (default `false`).
    #[serde(default)]
    all: Option<bool>,
    /// Whether things standing in a listed sector join the selection,
    /// judged before anything moves (default `true`).
    #[serde(default)]
    include_things_in_sectors: Option<bool>,
}

/// The `scale` parameter: the exact fraction `num / den` (both above
/// zero), e.g. 3/2 for 150%.
#[derive(Debug, Deserialize, JsonSchema)]
struct ScaleInput {
    /// Numerator; above zero.
    num: i32,
    /// Denominator; above zero.
    den: i32,
}

/// The `rotate` parameter: exactly one of the two fields,
/// counter-clockwise.
#[derive(Debug, Deserialize, JsonSchema)]
struct RotateInput {
    /// Whole degrees; give exactly one of this and `millidegrees`.
    #[serde(default)]
    degrees: Option<i64>,
    /// Thousandths of a degree; give exactly one of this and `degrees`.
    /// Thing facings take the nearest whole degree.
    #[serde(default)]
    millidegrees: Option<i64>,
}

/// The `translate` parameter: the whole-unit shift applied last.
#[derive(Debug, Deserialize, JsonSchema)]
struct TranslateInput {
    /// `X` shift in map units.
    dx: i32,
    /// `Y` shift in map units.
    dy: i32,
}

/// A bare `{x, y}` point parameter (`origin`).
#[derive(Debug, Deserialize, JsonSchema)]
struct PointInput {
    /// `X` in map units.
    x: i32,
    /// `Y` in map units.
    y: i32,
}

impl From<&PointInput> for Point {
    #[inline]
    fn from(input: &PointInput) -> Self {
        Self {
            x: i64::from(input.x),
            y: i64::from(input.y),
        }
    }
}

/// Arguments for [`Cyberdemon::transform_selection`].
#[derive(Debug, Deserialize, JsonSchema)]
struct TransformSelectionParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// Which elements to move (expanded into vertices and things).
    selection: SelectionInput,
    /// Exact-fraction scaling about the origin, applied first.
    #[serde(default)]
    scale: Option<ScaleInput>,
    /// Counter-clockwise rotation about the origin, applied second.
    #[serde(default)]
    rotate: Option<RotateInput>,
    /// Whole-unit shift, applied last.
    #[serde(default)]
    translate: Option<TranslateInput>,
    /// The pivot of `scale` and `rotate`; defaults to the centre of the
    /// selection's bounding box, rounded to nearest.
    #[serde(default)]
    origin: Option<PointInput>,
}

/// Structured result of [`Cyberdemon::transform_selection`].
#[derive(Serialize)]
struct TransformOutcome {
    /// What the transform engine did.
    #[serde(flatten)]
    transform: TransformReport,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The `selection` parameter as the engine's expansion request, defaults
/// applied.
fn selection_request(input: &SelectionInput) -> SelectionRequest {
    SelectionRequest {
        vertices: input.vertices.clone().unwrap_or_default(),
        linedefs: input.linedefs.clone().unwrap_or_default(),
        sectors: input.sectors.clone().unwrap_or_default(),
        things: input.things.clone().unwrap_or_default(),
        all: input.all.unwrap_or(false),
        include_things_in_sectors: input.include_things_in_sectors.unwrap_or(true),
    }
}

/// The `rotate` parameter as a rotation; it must carry exactly one of
/// `degrees` and `millidegrees`.
fn rotation_of(input: &RotateInput) -> Result<Rotation, CyberError> {
    match (input.degrees, input.millidegrees) {
        (Some(_), Some(_)) | (None, None) => Err(CyberError::InvalidArgument(
            "rotate needs exactly one of degrees and millidegrees; pass one number and drop \
             the other"
                .to_owned(),
        )),
        (Some(degrees), None) => Ok(Rotation::from_degrees(degrees)),
        (None, Some(millidegrees)) => Ok(Rotation::from_millidegrees(millidegrees)),
    }
}

/// The call's operations, validated and refusing the empty set: a
/// transform with nothing to do is never what the caller meant.
fn operations_of(args: &TransformSelectionParams) -> Result<Operations, CyberError> {
    let ops = Operations {
        scale: args
            .scale
            .as_ref()
            .map(|input| Scale::new(input.num, input.den))
            .transpose()?,
        rotation: args.rotate.as_ref().map(rotation_of).transpose()?,
        translate: args.translate.as_ref().map(|input| Point {
            x: i64::from(input.dx),
            y: i64::from(input.dy),
        }),
    };
    if ops.is_empty() {
        return Err(CyberError::InvalidArgument(
            "no operation: pass at least one of scale, rotate and translate".to_owned(),
        ));
    }
    Ok(ops)
}

/// `n` and its noun, singular at one and plural otherwise (`"vertex"`
/// pluralises irregularly, so both forms are given).
fn counted(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", if n == 1 { singular } else { plural })
}

/// `transform_selection`'s work: validate the operations, expand the
/// selection against the map as it now stands (an early, side-effect-free
/// refusal of bad indices and empty expansions), then run the whole
/// transform as one undoable step.
fn transform_run(
    session: &mut Session,
    args: &TransformSelectionParams,
) -> Result<TransformOutcome, CyberError> {
    let ops = operations_of(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let request = selection_request(&args.selection);
    let pivot = args.origin.as_ref().map(Point::from);
    let expanded = expand_selection(&current_map(session, name)?, &request)?;
    let label = format!(
        "transform {} and {}",
        counted(expanded.vertices.len(), "vertex", "vertices"),
        counted(expanded.things.len(), "thing", "things")
    );
    let (transform, report) = session.edit_map(name, &label, |map| {
        let moved = expand_selection(map, &request)?;
        apply_transform(map, &moved, &ops, pivot)
    })?;
    Ok(TransformOutcome {
        transform,
        edit: edit_result_fragment(&report),
    })
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
#[tool_router(router = edit_geometry_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Deletes a mixed selection of map elements with full cleanup.
    #[tool(
        description = "Deletes a mixed selection of vertices, linedefs, sidedefs, sectors and things in a single undoable edit with full reference cleanup: sidedefs of removed sectors go, linedefs on removed vertices go, losing a front flips a line, losing a back makes it one-sided, and prune_sidedefs/prune_vertices (both default true) leave no orphans. dry_run reports the whole cascade without editing anything."
    )]
    fn delete_geometry(
        &self,
        Parameters(args): Parameters<DeleteGeometryParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| delete_run(session, &args)),
        )
    }

    /// Moves a selection of vertices and things with one transform.
    #[tool(
        description = "Moves a selection of a map's vertices and things through one scale/rotate/translate transform in a single undoable edit; the math is integer-only and rounded. selection lists vertices, linedefs, sectors and things (or all: true - lines bring their endpoints, sectors their boundary), then scale (an exact fraction), rotate (degrees; things turn too) and translate run around origin, the selection's centre by default. Geometry: rebuild nodes after."
    )]
    fn transform_selection(
        &self,
        Parameters(args): Parameters<TransformSelectionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| transform_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, from_str};

    use super::{
        CascadeOptions, DeleteGeometryParams, Map, RemovalReport, RotateInput,
        TransformSelectionParams, cascade_options, check_selection, counted, delete_in_place,
        has_selection, operations_of, removal_size, rotation_of, seed_removal, selected_lists,
        selection_request, untouched_fragment,
    };
    use crate::error::CyberError;
    use crate::fixed::Angle;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::transform::{Point, Scale};
    use crate::wad::map::{Linedef, Sector, Sidedef, Thing, Vertex};
    use crate::wad::name::LumpName;
    use alloc::collections::BTreeSet;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A delete request parsed from `json`.
    fn args(json: &str) -> DeleteGeometryParams {
        from_str(json).expect("test delete requests parse")
    }

    /// A valid parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A fully visible spawn of `kind` at `x`/`y`.
    fn spawn(kind: i32, x: i32, y: i32) -> Thing {
        Thing {
            kind,
            x,
            y,
            flags: 7,
            ..Default::default()
        }
    }

    /// A blocking one-sided line from `v1` to `v2` over sidedef `front`.
    fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-sided step line from `v1` to `v2` over sidedefs `front`/`back`.
    fn step_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// A sidedef opening into `sector`.
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Default::default()
        }
    }

    /// The two-room map the test fixtures build: a `0,0` to `512,256` hall
    /// and raised back room, six vertices, seven lines (`2` the two-sided
    /// step), eight sides over two sectors, a start and an imp.
    fn two_room() -> Map {
        Map {
            name: name("MAP01"),
            format: MapFormat::Doom,
            things: vec![spawn(1, 64, 128), spawn(3_001, 384, 128)],
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(512, 0),
                vertex(512, 256),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                solid_line(0, 5, 0),
                solid_line(5, 4, 1),
                step_line(4, 1, 2, 3),
                solid_line(1, 0, 4),
                solid_line(4, 3, 5),
                solid_line(3, 2, 6),
                solid_line(2, 1, 7),
            ],
            sidedefs: vec![
                side(0),
                side(0),
                side(0),
                side(1),
                side(0),
                side(1),
                side(1),
                side(1),
            ],
            sectors: vec![Sector::default(), Sector::default()],
            ..Default::default()
        }
    }

    /// The step of `json`'s selection, checked and cascaded onto `map`.
    fn delete(map: &mut Map, json: &str) -> Result<RemovalReport, CyberError> {
        let request = args(json);
        let opts = cascade_options(&request);
        let removal = seed_removal(&request);
        delete_in_place(map, opts, &removal)
    }

    #[test]
    fn a_selection_needs_at_least_one_index() {
        assert!(!has_selection(&args("{}")), "no lists at all");
        assert!(
            !has_selection(&args(r#"{"vertices": [], "sectors": []}"#)),
            "empty lists select nothing"
        );
        assert!(
            has_selection(&args(r#"{"linedefs": [], "sectors": [1]}"#)),
            "one non-empty list is enough"
        );
        let mixed = args(r#"{"vertices": [4, 1, 4], "things": [0]}"#);
        assert_eq!(
            selected_lists(&mixed)[0],
            [4_usize, 1, 4],
            "the lists pass through as given"
        );
        let removal = seed_removal(&mixed);
        assert_eq!(
            removal.vertices,
            BTreeSet::from([1_usize, 4]),
            "duplicates collapse"
        );
        assert_eq!(removal.things, BTreeSet::from([0_usize]));
        assert!(
            removal.linedefs.is_empty()
                && removal.sidedefs.is_empty()
                && removal.sectors.is_empty(),
            "the absent lists stay empty"
        );
        assert_eq!(removal_size(&removal), 3, "the seed count labels the step");
    }

    #[test]
    fn the_pruning_flags_default_to_the_cascading_pair() {
        let defaults = cascade_options(&args(r#"{"sectors": [1]}"#));
        assert!(
            defaults.prune_sidedefs && defaults.prune_vertices,
            "pruning is on by default"
        );
        assert_eq!(
            cascade_options(&args(
                r#"{"sectors": [1], "prune_sidedefs": false, "prune_vertices": false}"#,
            )),
            CascadeOptions {
                prune_sidedefs: false,
                prune_vertices: false,
            },
            "both switches reach the options"
        );
    }

    #[test]
    fn check_selection_names_the_missing_index_and_its_kind() {
        let map = two_room();
        assert!(
            check_selection(
                &map,
                &seed_removal(&args(r#"{"sectors": [0, 1], "vertices": [5]}"#)),
            )
            .is_ok(),
            "every index of the fixture selection exists"
        );
        let cases = [
            (
                r#"{"vertices": [99]}"#,
                "vertex 99 does not exist (the map has 6 vertices, valid indices are 0..=5)",
            ),
            (
                r#"{"linedefs": [7]}"#,
                "linedef 7 does not exist (the map has 7 linedefs, valid indices are 0..=6)",
            ),
            (
                r#"{"sidedefs": [8]}"#,
                "sidedef 8 does not exist (the map has 8 sidedefs, valid indices are 0..=7)",
            ),
            (
                r#"{"sectors": [2]}"#,
                "sector 2 does not exist (the map has 2 sectors, valid indices are 0..=1)",
            ),
            (
                r#"{"things": [2]}"#,
                "thing 2 does not exist (the map has 2 things, valid indices are 0..=1)",
            ),
        ];
        for (json, phrasing) in cases {
            let error = check_selection(&map, &seed_removal(&args(json)))
                .expect_err("the named index is past the end of its block");
            assert_eq!(code(&error), "not_found", "out of range code for {json}");
            assert_eq!(error.to_string(), phrasing, "the documented phrasing");
        }
        let first_kind = check_selection(
            &map,
            &seed_removal(&args(r#"{"vertices": [9], "sectors": [9]}"#)),
        )
        .expect_err("both lists are out of range");
        assert!(
            first_kind.to_string().contains("vertex 9"),
            "the vertex list is checked first: {first_kind}"
        );
    }

    #[test]
    fn a_cascaded_sector_deletion_takes_the_room_apart() {
        let mut map = two_room();
        let report = delete(&mut map, r#"{"sectors": [1]}"#).expect("sector 1 goes");
        assert_eq!(report.sectors, vec![1], "the seeded sector");
        assert_eq!(report.sidedefs, vec![3, 5, 6, 7], "every side of it");
        assert_eq!(
            report.linedefs,
            vec![4, 5, 6],
            "the lines that only those sides held"
        );
        assert_eq!(report.vertices, vec![2, 3], "the orphaned corners follow");
        assert!(report.things.is_empty(), "the cascade never adds things");
        assert_eq!(report.one_sided_linedefs, vec![2], "the shared line closed");
        assert!(report.flipped_linedefs.is_empty(), "nothing had to flip");
        assert_eq!(map.sectors.len(), 1, "only the hall survives");
        assert_eq!(map.sidedefs.len(), 4, "its four sides survive");
        assert_eq!(map.linedefs.len(), 4, "four lines survive");
        assert_eq!(map.vertices.len(), 4, "the back room's corners went");
    }

    #[test]
    fn a_mixed_selection_deletes_things_and_geometry_together() {
        let mut map = two_room();
        let report = delete(&mut map, r#"{"things": [1], "vertices": [5]}"#)
            .expect("the imp and one corner go");
        assert_eq!(report.things, vec![1], "the seeded thing");
        assert_eq!(report.vertices, vec![5], "the seeded corner");
        assert_eq!(
            report.linedefs,
            vec![0, 1],
            "both lines ending at the corner go"
        );
        assert_eq!(
            report.sidedefs,
            vec![0, 1],
            "the sides only those lines had are pruned"
        );
        assert!(report.sectors.is_empty(), "no sector went");
        assert_eq!(map.things.len(), 1, "the start survives");
        assert_eq!(map.vertices.len(), 5, "one vertex dropped");
        assert_eq!(map.linedefs.len(), 5, "five lines survive");
    }

    #[test]
    fn pruning_off_leaves_the_orphans_behind() {
        let mut unpruned = two_room();
        let report = delete(
            &mut unpruned,
            r#"{"linedefs": [5, 6], "prune_sidedefs": false, "prune_vertices": false}"#,
        )
        .expect("the two named lines go");
        assert_eq!(report.linedefs, vec![5, 6], "exactly the seed");
        assert!(
            report.sidedefs.is_empty() && report.vertices.is_empty(),
            "nothing followed: {report:?}"
        );
        assert_eq!(unpruned.vertices.len(), 6, "the orphaned vertex stays");
        assert_eq!(unpruned.sidedefs.len(), 8, "the orphaned sides stay");
        let mut vertexless = two_room();
        let sector_report = delete(
            &mut vertexless,
            r#"{"sectors": [1], "prune_vertices": false}"#,
        )
        .expect("the sector rule still runs");
        assert_eq!(
            sector_report.sidedefs,
            vec![3, 5, 6, 7],
            "the sector rule is not a pruning"
        );
        assert_eq!(
            sector_report.linedefs,
            vec![4, 5, 6],
            "sideless lines still go"
        );
        assert!(
            sector_report.vertices.is_empty(),
            "the orphaned vertices stay: {sector_report:?}"
        );
        assert_eq!(
            vertexless.vertices.len(),
            6,
            "both back-room corners survive"
        );
    }

    #[test]
    fn a_missing_index_deletes_nothing() {
        let before = two_room();
        let mut map = two_room();
        let error = delete(&mut map, r#"{"things": [0], "sectors": [9]}"#)
            .expect_err("sector 9 is not there");
        assert_eq!(code(&error), "not_found", "out of range code");
        assert_eq!(map, before, "the whole map stands");
    }

    #[test]
    fn an_untouched_fragment_reports_the_map_by_name() {
        let fragment = untouched_fragment(name("MAP01"));
        assert_eq!(
            fragment.get("map").and_then(Value::as_str),
            Some("MAP01"),
            "named"
        );
        assert_eq!(
            fragment.get("nodes_invalidated").and_then(Value::as_bool),
            Some(false),
            "nothing was invalidated"
        );
        assert_eq!(
            fragment.get("copied_to_working").and_then(Value::as_bool),
            Some(false),
            "nothing was copied"
        );
    }

    /// A transform request parsed from `json`.
    fn transform_args(json: &str) -> TransformSelectionParams {
        from_str(json).expect("test transform requests parse")
    }

    /// The `rotate` object of a parsed request.
    fn rotate_of(json: &str) -> RotateInput {
        transform_args(&format!(r#"{{"selection": {{}}, "rotate": {json}}}"#))
            .rotate
            .expect("the rotate object parses")
    }

    #[test]
    fn a_transform_needs_at_least_one_operation() {
        let none = operations_of(&transform_args(r#"{"selection": {"all": true}}"#))
            .expect_err("no operation does nothing");
        assert_eq!(code(&none), "invalid_argument", "no-operation code");
        assert!(
            none.to_string().contains("scale, rotate and translate"),
            "the message names the three: {none}"
        );
        let ops = operations_of(&transform_args(
            r#"{"selection": {"all": true}, "translate": {"dx": 0, "dy": 0}}"#,
        ))
        .expect("even the zero shift is an operation");
        assert_eq!(
            ops.translate,
            Some(Point { x: 0, y: 0 }),
            "the shift reaches the operations"
        );
    }

    #[test]
    fn operations_validate_scale_fractions_and_rotate_shapes() {
        let bad_scale =
            transform_args(r#"{"selection": {"all": true}, "scale": {"num": 0, "den": 2}}"#);
        let zero = operations_of(&bad_scale).expect_err("0/2 is no factor");
        assert_eq!(code(&zero), "invalid_argument", "zero numerator code");
        let both = operations_of(&transform_args(
            r#"{"selection": {"all": true},
                "rotate": {"degrees": 90, "millidegrees": 90000}}"#,
        ))
        .expect_err("two rotation numbers are ambiguous");
        assert_eq!(code(&both), "invalid_argument", "two-of-one code");
        assert!(
            both.to_string().contains("exactly one"),
            "the message says what to pass: {both}"
        );
        let combined = operations_of(&transform_args(
            r#"{"selection": {"all": true},
                "scale": {"num": 3, "den": 2},
                "rotate": {"millidegrees": 45500},
                "translate": {"dx": 8, "dy": -8}}"#,
        ))
        .expect("all three operations parse");
        assert_eq!(
            combined.scale,
            Some(Scale { num: 3, den: 2 }),
            "the fraction widens intact"
        );
        assert_eq!(
            combined.rotation.map(|rotation| rotation.degrees),
            Some(46),
            "thing facings take the rounded degree"
        );
        assert_eq!(
            combined.translate,
            Some(Point { x: 8, y: -8 }),
            "the shift comes through"
        );
    }

    #[test]
    fn rotation_reads_its_one_number() {
        let degrees = rotation_of(&rotate_of(r#"{"degrees": -90}"#)).expect("whole degrees");
        assert_eq!(degrees.degrees, -90, "the facing delta passes through");
        assert_eq!(degrees.angle, Angle::from_degrees(-90), "and the angle");
        let fine = rotation_of(&rotate_of(r#"{"millidegrees": 45000}"#)).expect("millidegrees");
        assert_eq!(
            fine.angle,
            Angle::from_millidegrees(45_000),
            "the exact angle"
        );
        assert!(
            rotation_of(&rotate_of("{}")).is_err(),
            "neither number refuses"
        );
    }

    #[test]
    fn the_step_label_counts_each_kind_by_its_own_form() {
        assert_eq!(
            counted(0, "vertex", "vertices"),
            "0 vertices",
            "zero is plural"
        );
        assert_eq!(counted(1, "thing", "things"), "1 thing", "one is singular");
        assert_eq!(
            counted(4, "vertex", "vertices"),
            "4 vertices",
            "irregular plural"
        );
    }

    #[test]
    fn the_selection_defaults_to_including_sector_things() {
        let bare = selection_request(&transform_args(r#"{"selection": {}}"#).selection);
        assert!(!bare.all, "all is off by default");
        assert!(
            bare.include_things_in_sectors,
            "the include switch is on by default"
        );
        assert!(
            bare.vertices.is_empty()
                && bare.linedefs.is_empty()
                && bare.sectors.is_empty()
                && bare.things.is_empty(),
            "absent lists stay empty"
        );
        let switched = transform_args(
            r#"{"selection": {"sectors": [1], "things": [0], "all": false,
                "include_things_in_sectors": false}}"#,
        );
        let request = selection_request(&switched.selection);
        assert_eq!(request.sectors, vec![1_usize], "the lists pass through");
        assert_eq!(request.things, vec![0_usize]);
        assert!(
            !request.include_things_in_sectors,
            "the switch reaches the request"
        );
    }
}
