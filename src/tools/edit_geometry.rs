//! Multi-kind geometry editing: `delete_geometry` (see the `edit_geometry`
//! router methods on [`crate::server::Cyberdemon`]).
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
            return Err(CyberError::NotFound(format!(
                "{kind} {index} does not exist (map has {count} {plural})"
            )));
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
        Err(CyberError::NotFound(_)) => common::load_map(session, name, Scope::Merged),
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
        description = "Deletes a mixed selection of a map's elements (vertices, linedefs, sidedefs, sectors, things) in a single undoable edit, with full reference cleanup. map defaults to the active map (set_active_map, else the merged view's only map). Pass any of vertices, linedefs, sidedefs, sectors and things as arrays of 0-based indices into that map's blocks; at least one array must hold an index or the call fails invalid_argument, and an index the map does not hold fails not_found naming the kind and index with nothing changed. The removal always cascades: every sidedef of a removed sector goes; every linedef touching a removed vertex goes; a linedef that loses its front side is flipped (v1/v2 swap, the back becomes the front); a linedef that loses its back becomes one-sided (two-sided bit cleared, blocking bit set, an otherwise invisible lower or upper texture of the surviving side moved to the middle); a linedef that loses both sides goes with them; prune_sidedefs (default true) also drops sidedefs no surviving linedef references and prune_vertices (default true) also drops vertices no surviving linedef references, so a deletion leaves no orphans unless you switch a pruning off. The cascade never adds sectors or things: those go only when you name them. dry_run (default false) computes the whole cascade on a copy and reports it without editing: no undo entry, copied_to_working false, nodes_invalidated false. Deleting anything but things alone is geometry: the node lumps are invalidated (run build_nodes before playing the map in vanilla Doom). Returns {removed: {vertices, linedefs, sidedefs, sectors, things (removed original indices, sorted), flipped_linedefs, one_sided_linedefs (new indices of the normalised linedefs)}, dry_run, map, nodes_invalidated, copied_to_working}. See skill/reference/geometry.md."
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
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, from_str};

    use super::{
        CascadeOptions, DeleteGeometryParams, Map, RemovalReport, cascade_options, check_selection,
        delete_in_place, has_selection, removal_size, seed_removal, selected_lists,
        untouched_fragment,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
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
                "vertex 99 does not exist (map has 6 vertices)",
            ),
            (
                r#"{"linedefs": [7]}"#,
                "linedef 7 does not exist (map has 7 linedefs)",
            ),
            (
                r#"{"sidedefs": [8]}"#,
                "sidedef 8 does not exist (map has 8 sidedefs)",
            ),
            (
                r#"{"sectors": [2]}"#,
                "sector 2 does not exist (map has 2 sectors)",
            ),
            (
                r#"{"things": [2]}"#,
                "thing 2 does not exist (map has 2 things)",
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
}
