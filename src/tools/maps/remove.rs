//! The element-removal scaffold every `remove_*` tool runs through.
//!
//! The five single-kind removal tools (`remove_thing`, `remove_linedef`,
//! `remove_sidedef`, `remove_sector` and `remove_vertex`) all walk the same
//! ladder: read the [`IndexSelector`], resolve the map name, seed a
//! single-kind [`Removal`] with `seed_removal`, range-check the indices
//! with `check_present`, then either refuse a referenced element with the
//! kind-worded `dependent_refusal` `conflict` or grow the seed with
//! `expand_cascade` and apply it through `remove_in_place`; `remove_run`
//! wraps one such step as a [`Session::edit_map`] call under the label
//! `remove {n} {kind plural}` and reports `RemoveToolReport`. The ladder
//! is parameterized by the [`ElementKind`] and, for the no-cascade arm,
//! which [`Dependents`] field the kind consults: vertices
//! and sidedefs by linedefs, sectors by sidedefs — and linedefs and things
//! by nothing, so their no-cascade arm is the plain apply. That is how
//! `remove_thing` runs (its `cascade` is always `None`) and how
//! `remove_linedef`, which always cascades, passes `Some(options)`.
//!
//! The pure engine — what references what, how a cascade grows and how the
//! survivors renumber — stays in [`crate::wad::map::remove`]; this module
//! only folds it into the one shape the tools call.
//!
//! [`IndexSelector`]: crate::tools::maps::params::IndexSelector
//! [`Session::edit_map`]: crate::session::Session::edit_map
//! [`ElementKind`]: crate::tools::maps::params::ElementKind
//! [`Dependents`]: crate::wad::map::remove::Dependents

use alloc::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{ElementKind, edit_result_fragment, join};
use crate::wad::map::Map;
use crate::wad::map::query::check_indices;
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, dependents, expand_cascade,
};

/// Structured result of the `remove_*` tools: what the removal engine
/// dropped beside the standard edit fragment.
#[derive(Serialize)]
pub(crate) struct RemoveToolReport {
    /// What the removal engine dropped, flipped and normalised.
    pub(crate) removed: RemovalReport,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    pub(crate) edit: JsonMap<String, Value>,
}

/// One `remove_*` call's shared plan: which kind of element, the already
/// read index selection and the call's cascade mode.
pub(crate) struct RemovePlan<'call> {
    /// The kind of element the tool removes.
    pub(crate) kind: ElementKind,
    /// The call's `map` parameter; `None` is the session default.
    pub(crate) map: Option<&'call str>,
    /// The selected indices, sorted and deduplicated by
    /// [`IndexSelector::indices`].
    ///
    /// [`IndexSelector::indices`]: crate::tools::maps::params::IndexSelector::indices
    pub(crate) indices: &'call [usize],
    /// `Some(options)` always runs the [`cascade_options`]-grown removal;
    /// `None` takes the refuse-without-cascade arm, which for the kinds
    /// nothing references (`linedef` and `thing`) is the plain apply.
    pub(crate) cascade: Option<CascadeOptions>,
}

/// A doomed-index-to-referencing-indices table: the shape of every
/// [`Dependents`] field, keyed by the removed element's original index.
pub(crate) type IndexUsers = BTreeMap<usize, Vec<usize>>;

/// The seed [`Removal`] of exactly `indices` of one element `kind`, all
/// other kinds empty.
#[must_use]
pub(crate) fn seed_removal(kind: ElementKind, indices: &[usize]) -> Removal {
    let set: BTreeSet<usize> = indices.iter().copied().collect();
    match kind {
        ElementKind::Vertices => Removal {
            vertices: set,
            ..Default::default()
        },
        ElementKind::Linedefs => Removal {
            linedefs: set,
            ..Default::default()
        },
        ElementKind::Sidedefs => Removal {
            sidedefs: set,
            ..Default::default()
        },
        ElementKind::Sectors => Removal {
            sectors: set,
            ..Default::default()
        },
        ElementKind::Things => Removal {
            things: set,
            ..Default::default()
        },
    }
}

/// Refuses unless every one of `indices` is an element of `kind` that
/// `map` holds, through the shared [`check_indices`] guard.
///
/// [`check_indices`]: crate::wad::map::query::check_indices
pub(crate) fn check_present(
    kind: ElementKind,
    indices: &[usize],
    map: &Map,
) -> Result<(), CyberError> {
    check_indices(kind, indices, map)
}

/// The pruning flags of a `remove_*` call, the cascading default when
/// either is absent.
#[must_use]
pub(crate) fn cascade_options(
    prune_sidedefs: Option<bool>,
    prune_vertices: Option<bool>,
) -> CascadeOptions {
    CascadeOptions {
        prune_sidedefs: prune_sidedefs.unwrap_or(true),
        prune_vertices: prune_vertices.unwrap_or(true),
    }
}

/// The `conflict` naming every element of `referencing` that still uses a
/// selected element of `kind`, and offering the `cascade` fix:
/// `vertex 4 is used by linedef(s) 1, 2`, `sidedef 1 is used by linedef(s)
/// 1`, `sector 0 is used by sidedef(s) 0, 1`, each pair joined by `"; "`
/// and the whole list ended by the pass-`cascade` advice.
#[must_use]
pub(crate) fn dependent_refusal(
    kind: ElementKind,
    referencing: ElementKind,
    uses: &IndexUsers,
) -> CyberError {
    let parts: Vec<String> = uses
        .iter()
        .map(|(index, users)| {
            format!(
                "{} {index} is used by {}(s) {}",
                kind.singular(),
                referencing.singular(),
                join(users),
            )
        })
        .collect();
    CyberError::Conflict(format!(
        "{}; pass cascade: true to remove the referencing {} with them",
        parts.join("; "),
        referencing.plural(),
    ))
}

/// The kind that references `kind`'s elements, with the uses of `removal`
/// keyed by doomed index: vertices and sidedefs are linedef references,
/// sectors are sidedef references, and linedefs and things are referenced
/// by nothing (`None`).
fn refused_uses(
    kind: ElementKind,
    map: &Map,
    removal: &Removal,
) -> Option<(ElementKind, IndexUsers)> {
    match kind {
        ElementKind::Vertices => {
            let uses = dependents(map, removal).linedefs_using_vertices;
            Some((ElementKind::Linedefs, uses))
        }
        ElementKind::Sidedefs => {
            let uses = dependents(map, removal).linedefs_using_sidedefs;
            Some((ElementKind::Linedefs, uses))
        }
        ElementKind::Sectors => {
            let uses = dependents(map, removal).sidedefs_using_sectors;
            Some((ElementKind::Sidedefs, uses))
        }
        ElementKind::Linedefs | ElementKind::Things => None,
    }
}

/// The no-cascade arm: refuse the `conflict` [`dependent_refusal`] spells
/// when anything still references the seed, else apply the seed as it is.
fn plain_or_refuse(
    map: &mut Map,
    kind: ElementKind,
    removal: &Removal,
) -> Result<RemovalReport, CyberError> {
    if let Some((referencing, uses)) = refused_uses(kind, map, removal)
        && !uses.is_empty()
    {
        return Err(dependent_refusal(kind, referencing, &uses));
    }
    apply_removal(map, removal)
}

/// The removal step, run as `Session::edit_map`'s closure: check the
/// indices are present, seed the single-kind [`Removal`] and either grow it
/// with `cascade`'s [`expand_cascade`] under its [`CascadeOptions`] or take
/// the refuse-or-plain arm. A refused call leaves `map` untouched.
pub(crate) fn remove_in_place(
    map: &mut Map,
    kind: ElementKind,
    indices: &[usize],
    cascade: Option<CascadeOptions>,
) -> Result<RemovalReport, CyberError> {
    check_present(kind, indices, map)?;
    let removal = seed_removal(kind, indices);
    match cascade {
        Some(opts) => {
            let grown = expand_cascade(map, &removal, opts);
            apply_removal(map, &grown)
        }
        None => plain_or_refuse(map, kind, &removal),
    }
}

/// The `remove_*` work shared by the five tools: resolve the map, run
/// [`remove_in_place`] as one [`Session::edit_map`] step labelled
/// `remove {n} {kind plural}`, and report the [`RemovalReport`] beside the
/// standard edit fragment.
///
/// [`Session::edit_map`]: crate::session::Session::edit_map
pub(crate) fn remove_run(
    session: &mut Session,
    plan: &RemovePlan<'_>,
) -> Result<RemoveToolReport, CyberError> {
    let name = common::resolve_map_name(session, plan.map)?;
    let label = format!("remove {} {}", plan.indices.len(), plan.kind.plural());
    let (removed, report) = session.edit_map(name, &label, |map| {
        remove_in_place(map, plan.kind, plan.indices, plan.cascade)
    })?;
    Ok(RemoveToolReport {
        removed,
        edit: edit_result_fragment(&report),
    })
}

#[cfg(test)]
mod tests {
    use alloc::collections::{BTreeMap, BTreeSet};
    use serde_json::from_str;

    use super::{
        ElementKind, RemovePlan, cascade_options, check_present, dependent_refusal,
        remove_in_place, seed_removal,
    };
    use crate::error::CyberError;
    use crate::tools::maps::params::IndexSelector;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::remove::CascadeOptions;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::name::LumpName;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
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

    /// A two-sided line from `v1` to `v2` over sidedefs `front`/`back`.
    fn open_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
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

    /// A fully visible thing at `x`/`y`.
    fn spawn(x: i32, y: i32) -> Thing {
        Thing {
            x,
            y,
            kind: 2_001_i32,
            flags: 7,
            ..Default::default()
        }
    }

    /// The shared fixture: a four-corner shell (`0..=3`) with the two-sided
    /// line `1` (sidedefs `1`/`2`) splitting hall `0` from room `1`, the
    /// unreferenced vertex `4`, the unreferenced sidedef `5` and one thing.
    fn fixture() -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("a valid name"),
            format: MapFormat::Doom,
            vertices: vec![
                vertex(0, 0),
                vertex(128, 0),
                vertex(128, 128),
                vertex(0, 128),
                vertex(256, 256),
            ],
            linedefs: vec![
                solid_line(0, 1, 0),
                open_line(1, 2, 1, 2),
                solid_line(2, 3, 3),
                solid_line(3, 0, 4),
            ],
            sidedefs: vec![side(0), side(0), side(1), side(0), side(0), side(0)],
            sectors: vec![Sector::default(), Sector::default()],
            things: vec![spawn(64_i32, 64_i32)],
            ..Default::default()
        }
    }

    /// The no-cascade arm, spelled once for the ladder tests.
    const PLAIN: Option<CascadeOptions> = None;

    /// A uses map from `(doomed, users)` pairs, for the refusal texts.
    fn uses(pairs: [(usize, Vec<usize>); 2]) -> BTreeMap<usize, Vec<usize>> {
        pairs.into_iter().collect()
    }

    #[test]
    fn each_kind_seeds_exactly_its_own_deduplicated_set() {
        for kind in [
            ElementKind::Vertices,
            ElementKind::Linedefs,
            ElementKind::Sidedefs,
            ElementKind::Sectors,
            ElementKind::Things,
        ] {
            let removal = seed_removal(kind, &[2_usize, 5, 2]);
            let sets: [&BTreeSet<usize>; 5] = [
                &removal.vertices,
                &removal.linedefs,
                &removal.sidedefs,
                &removal.sectors,
                &removal.things,
            ];
            let seeded: Vec<&BTreeSet<usize>> =
                sets.into_iter().filter(|set| !set.is_empty()).collect();
            assert_eq!(seeded.len(), 1, "{kind:?} seeds exactly one set");
            assert_eq!(
                seeded.first().expect("the seeded set").len(),
                2,
                "{kind:?} deduplicated its indices",
            );
        }
    }

    #[test]
    fn check_present_names_the_first_missing_index_of_the_kind() {
        let map = fixture();
        assert!(
            check_present(ElementKind::Sidedefs, &[0_usize, 5], &map).is_ok(),
            "both sides exist",
        );
        let error = check_present(ElementKind::Vertices, &[0_usize, 40], &map)
            .expect_err("vertex 40 does not");
        assert_eq!(code(&error), "not_found", "out of range code");
        assert_eq!(
            error.message(),
            "vertex 40 does not exist (the map has 5 vertices, valid indices are 0..=4)",
            "the shared check_index wording stands",
        );
    }

    #[test]
    fn absent_pruning_flags_default_to_the_cascading_pair() {
        let defaults = cascade_options(None, None);
        assert!(
            defaults.prune_sidedefs && defaults.prune_vertices,
            "absent prunes, the cascading default",
        );
        let given = cascade_options(Some(false), Some(true));
        assert!(
            !given.prune_sidedefs && given.prune_vertices,
            "given flags pass through",
        );
    }

    #[test]
    fn each_kind_spells_its_own_dependent_refusal() {
        let vertex = dependent_refusal(
            ElementKind::Vertices,
            ElementKind::Linedefs,
            &uses([(4, vec![1_usize, 2, 4]), (5, vec![2])]),
        );
        assert_eq!(code(&vertex), "conflict", "vertex conflict code");
        assert_eq!(
            vertex.message(),
            "vertex 4 is used by linedef(s) 1, 2, 4; vertex 5 is used by linedef(s) \
             2; pass cascade: true to remove the referencing linedefs with them",
            "the vertex wording, every user joined, one advice at the end",
        );
        let sidedef = dependent_refusal(
            ElementKind::Sidedefs,
            ElementKind::Linedefs,
            &uses([(1, vec![1_usize, 4]), (2, vec![1])]),
        );
        assert_eq!(
            sidedef.message(),
            "sidedef 1 is used by linedef(s) 1, 4; sidedef 2 is used by linedef(s) \
             1; pass cascade: true to remove the referencing linedefs with them",
            "the sidedef wording",
        );
        let sector = dependent_refusal(
            ElementKind::Sectors,
            ElementKind::Sidedefs,
            &uses([(0, vec![0_usize, 1]), (1, vec![2, 3])]),
        );
        assert_eq!(
            sector.message(),
            "sector 0 is used by sidedef(s) 0, 1; sector 1 is used by sidedef(s) \
             2, 3; pass cascade: true to remove the referencing sidedefs with them",
            "the sector wording names sidedefs",
        );
    }

    #[test]
    fn a_used_vertex_refuses_the_plain_arm_and_cascades_the_grown_one() {
        let mut map = fixture();
        let before = map.clone();
        let error = remove_in_place(&mut map, ElementKind::Vertices, &[0], PLAIN)
            .expect_err("two lines end at vertex 0");
        assert_eq!(code(&error), "conflict", "dependents code");
        assert_eq!(
            error.message(),
            "vertex 0 is used by linedef(s) 0, 3; pass cascade: true to remove the \
             referencing linedefs with them",
            "the users come listed ascending",
        );
        assert_eq!(map, before, "the refusal changed nothing");
        let report = remove_in_place(
            &mut map,
            ElementKind::Vertices,
            &[0],
            Some(CascadeOptions::default()),
        )
        .expect("the cascade lifts the conflict");
        assert_eq!(report.vertices, vec![0_usize], "the seeded point");
        assert_eq!(
            report.linedefs,
            vec![0_usize, 3],
            "both lines that ended there",
        );
        assert_eq!(
            report.sidedefs,
            vec![0_usize, 4],
            "and the sides only they held",
        );
        assert!(report.things.is_empty(), "things never follow a cascade");
        assert_eq!(map.vertices.len(), 4, "one vertex dropped");
    }

    #[test]
    fn an_unreferenced_sidedef_goes_through_the_plain_arm() {
        let mut map = fixture();
        let report = remove_in_place(&mut map, ElementKind::Sidedefs, &[5], PLAIN)
            .expect("sidedef 5 is referenced by nobody");
        assert_eq!(report.sidedefs, vec![5_usize], "the removed index");
        assert!(
            report.linedefs.is_empty() && report.flipped_linedefs.is_empty(),
            "nothing else followed: {report:?}"
        );
        assert_eq!(map.sidedefs.len(), 5, "the list shrank");
    }

    #[test]
    fn a_used_sector_refuses_and_a_cascaded_removal_closes_the_line() {
        let mut map = fixture();
        let error = remove_in_place(&mut map, ElementKind::Sectors, &[1], PLAIN)
            .expect_err("sidedef 2 uses sector 1");
        assert_eq!(code(&error), "conflict", "referenced without cascade");
        assert_eq!(
            error.message(),
            "sector 1 is used by sidedef(s) 2; pass cascade: true to remove the \
             referencing sidedefs with them",
            "the sector conflict names the sides",
        );
        let report = remove_in_place(
            &mut map,
            ElementKind::Sectors,
            &[1],
            Some(CascadeOptions::default()),
        )
        .expect("the cascade takes the room");
        assert_eq!(report.sectors, vec![1_usize], "the seeded sector");
        assert_eq!(report.sidedefs, vec![2_usize], "its side goes with it");
        assert!(report.linedefs.is_empty(), "the shared line survives");
        assert!(report.flipped_linedefs.is_empty(), "its front stood");
        assert_eq!(
            report.one_sided_linedefs,
            vec![1_usize],
            "the shared line closed",
        );
        assert_eq!(map.sectors.len(), 1, "only the hall survives");
        assert_eq!(map.linedefs.get(1).expect("line 1").back, None, "no back");
    }

    #[test]
    fn a_linedef_removal_cascades_and_a_thing_removal_applies_plain() {
        let mut map = fixture();
        let report = remove_in_place(
            &mut map,
            ElementKind::Linedefs,
            &[1],
            Some(cascade_options(None, None)),
        )
        .expect("the linedef arm runs the grown removal");
        assert_eq!(report.linedefs, vec![1_usize], "the seeded line");
        assert_eq!(report.sidedefs, vec![1_usize, 2], "the sides only it held");
        assert_eq!(map.linedefs.len(), 3, "the other lines stand");
        let things = remove_in_place(&mut map, ElementKind::Things, &[0], PLAIN)
            .expect("nothing references a thing, so the plain arm applies it");
        assert_eq!(things.things, vec![0_usize], "the thing went");
        assert!(map.things.is_empty(), "and the block is empty");
        assert_eq!(map.linedefs.len(), 3, "no geometry followed the thing");
    }

    #[test]
    fn a_missing_index_refuses_before_the_ladder_touches_anything() {
        let mut map = fixture();
        let before = map.clone();
        let error = remove_in_place(&mut map, ElementKind::Linedefs, &[40], PLAIN)
            .expect_err("the map holds four linedefs");
        assert_eq!(code(&error), "not_found", "out of range code");
        assert!(
            error
                .message()
                .contains("linedef 40 does not exist (the map has 4 linedefs"),
            "the spelled plural: {error}"
        );
        assert_eq!(map, before, "nothing was removed");
    }

    #[test]
    fn the_plan_carries_the_wire_forms_into_the_ladder() {
        let selector: IndexSelector = from_str("[2, 2, 0]").expect("a list selects");
        let indices = selector.indices().expect("the sorted, deduped pair");
        let plan = RemovePlan {
            kind: ElementKind::Vertices,
            map: Some("MAP01"),
            indices: &indices,
            cascade: PLAIN,
        };
        assert_eq!(plan.indices, vec![0_usize, 2], "the selector read first");
        let mut map = fixture();
        let error = remove_in_place(&mut map, plan.kind, plan.indices, plan.cascade)
            .expect_err("both vertices are used");
        assert_eq!(
            code(&error),
            "conflict",
            "the plan's kind drives the refusal",
        );
    }
}
