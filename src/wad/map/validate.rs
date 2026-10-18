//! Pure map validation: the reference, structure, thing, special,
//! geometry, resource and generated-lump checks behind the `validate_map`
//! tool.
//!
//! Every check is a pure function of the decoded [`Map`], plus the
//! [`Context`] of game definitions for the thing and special checks, and
//! returns its [`Issue`] rows: no sessions, no MCP, no editing. [`CHECKS`]
//! lists the checks in report order, [`run`] executes the requested subset
//! and stamps every issue with its check's [`Check::code`], so a later
//! validation ticket adds rows to [`CHECKS`] and check functions to the
//! submodules without touching the tool layer.
//!
//! Codes are stable snake-case strings, one per check; the severity lives
//! with the issue the check builds. The filtering, counting and truncation
//! a client asks about (`min_severity`, `codes`, `limit`) are the tool's
//! decisions, not this module's.

mod geometry;
mod node_lumps;
mod references;
mod resources;
mod specials;
mod structure;
mod things;

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use serde::Serialize;

use super::model::Map;
use crate::defs::Definitions;

/// How much an issue matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The map is broken: references leave a block, a line collapses, a
    /// player cannot spawn.
    Error,
    /// The map is probably wrong: a mismatched flag, an orphaned sector, a
    /// special outside the definitions.
    Warning,
    /// The map merely deserves a look: unused or shared elements, missing
    /// optional starts.
    Info,
}

impl Severity {
    /// The filter rank of this severity: bigger matters more.
    const fn rank(self) -> u8 {
        match self {
            Self::Info => 1_u8,
            Self::Warning => 2_u8,
            Self::Error => 3_u8,
        }
    }

    /// Whether an issue of this severity passes a minimum of `threshold`.
    #[must_use]
    #[inline]
    pub const fn passes(self, threshold: Self) -> bool {
        self.rank() >= threshold.rank()
    }
}

/// One finding of one check: what was found, where, and how bad it is.
///
/// Empty index lists and absent coordinates do not serialize, so an issue
/// carries only the references it actually has. [`run`] fills [`Issue::code`]
/// from the [`Check`] that produced the row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Issue {
    /// How much this finding matters.
    pub severity: Severity,
    /// The stable snake-case code of the check that reported it.
    #[serde(default)]
    pub code: &'static str,
    /// The human-readable description.
    pub message: String,
    /// The indices in [`Map::vertices`] this issue concerns.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vertices: Vec<usize>,
    /// The indices in [`Map::linedefs`] this issue concerns.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub linedefs: Vec<usize>,
    /// The indices in [`Map::sidedefs`] this issue concerns.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sidedefs: Vec<usize>,
    /// The indices in [`Map::sectors`] this issue concerns.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sectors: Vec<usize>,
    /// The indices in [`Map::things`] this issue concerns.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub things: Vec<usize>,
    /// The `X` map coordinate of the affected geometry, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    /// The `Y` map coordinate of the affected geometry, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
}

impl Issue {
    /// A row of `severity` explaining itself in `message`; [`run`] stamps
    /// the code and the builders below attach the references.
    pub(super) const fn new(severity: Severity, message: String) -> Self {
        Self {
            severity,
            code: "",
            message,
            vertices: Vec::new(),
            linedefs: Vec::new(),
            sidedefs: Vec::new(),
            sectors: Vec::new(),
            things: Vec::new(),
            x: None,
            y: None,
        }
    }

    /// Attaches the code of the check that produced this row.
    #[must_use]
    pub(super) const fn with_code(mut self, code: &'static str) -> Self {
        self.code = code;
        self
    }

    /// Attaches the map coordinates of the affected geometry.
    #[must_use]
    pub(super) const fn at(mut self, x: i32, y: i32) -> Self {
        self.x = Some(x);
        self.y = Some(y);
        self
    }

    /// Appends `indices` to the vertices this issue concerns.
    #[must_use]
    pub(super) fn vertex(mut self, indices: &[usize]) -> Self {
        self.vertices.extend_from_slice(indices);
        self
    }

    /// Appends `indices` to the linedefs this issue concerns.
    #[must_use]
    pub(super) fn linedef(mut self, indices: &[usize]) -> Self {
        self.linedefs.extend_from_slice(indices);
        self
    }

    /// Appends `indices` to the sidedefs this issue concerns.
    #[must_use]
    pub(super) fn sidedef(mut self, indices: &[usize]) -> Self {
        self.sidedefs.extend_from_slice(indices);
        self
    }

    /// Appends `indices` to the sectors this issue concerns.
    #[must_use]
    pub(super) fn sector(mut self, indices: &[usize]) -> Self {
        self.sectors.extend_from_slice(indices);
        self
    }

    /// Appends `indices` to the things this issue concerns.
    #[must_use]
    pub(super) fn thing(mut self, indices: &[usize]) -> Self {
        self.things.extend_from_slice(indices);
        self
    }
}

/// What the merged view provides to draw with, collected by the tool
/// layer and consulted by the resource checks.
///
/// `Default` is a session that defines nothing: the texture side reports
/// its `texture_check_skipped` info row instead of judging names, and
/// the flat side stays quiet when [`ResourceIndex::flats`] is empty.
#[derive(Debug, Default)]
pub struct ResourceIndex {
    /// Defined composite texture names, uppercase: the `TEXTURE1` and
    /// `TEXTURE2` entries plus the `TX_` names of the textures namespace.
    pub textures: BTreeSet<String>,
    /// The names of the merged flats-namespace lumps, uppercase.
    pub flats: BTreeSet<String>,
    /// Whether the merged view defines composite textures at all: a
    /// `TEXTURE1`/`TEXTURE2` lump or a textures namespace to fall back on.
    pub has_texture_defs: bool,
}

/// The game definitions the thing and special checks consult.
#[derive(Debug)]
pub struct Context<'defs> {
    /// The session's definitions: thing types overlaid with the loaded
    /// archives' customs, and the detected game's specials table (the
    /// `Doom`-family checks' table).
    pub definitions: &'defs Definitions,
    /// The `Hexen` specials table: the `unknown_special` check of a
    /// `Hexen`-family map consults it whatever game the session detected.
    pub hexen_specials: &'defs Definitions,
    /// What the merged view provides as textures and flats, for the
    /// resource checks.
    pub resources: &'defs ResourceIndex,
}

/// One named validation check in the pipeline.
pub struct Check {
    /// The stable snake-case code this check's issues carry.
    pub code: &'static str,
    /// Runs the check over `map` with the definitions of `context`.
    pub run: fn(&Map, &Context<'_>) -> Vec<Issue>,
}

/// Every check the server runs, in report order.
pub const CHECKS: &[Check] = &[
    Check {
        code: "vertex_out_of_range",
        run: |map, _context| references::vertex_out_of_range(map),
    },
    Check {
        code: "sidedef_out_of_range",
        run: |map, _context| references::sidedef_out_of_range(map),
    },
    Check {
        code: "sector_out_of_range",
        run: |map, _context| references::sector_out_of_range(map),
    },
    Check {
        code: "missing_front_side",
        run: |map, _context| references::missing_front_side(map),
    },
    Check {
        code: "zero_length_line",
        run: |map, _context| structure::zero_length_line(map),
    },
    Check {
        code: "duplicate_line",
        run: |map, _context| structure::duplicate_line(map),
    },
    Check {
        code: "two_sided_flag_mismatch",
        run: |map, _context| structure::two_sided_flag_mismatch(map),
    },
    Check {
        code: "unused_vertex",
        run: |map, _context| structure::unused_vertex(map),
    },
    Check {
        code: "unused_sidedef",
        run: |map, _context| structure::unused_sidedef(map),
    },
    Check {
        code: "unused_sector",
        run: |map, _context| structure::unused_sector(map),
    },
    Check {
        code: "shared_sidedef",
        run: |map, _context| structure::shared_sidedef(map),
    },
    Check {
        code: "no_player1_start",
        run: |map, _context| things::no_player1_start(map),
    },
    Check {
        code: "missing_coop_starts",
        run: |map, _context| things::missing_coop_starts(map),
    },
    Check {
        code: "unknown_thing_type",
        run: |map, context| things::unknown_thing_type(map, context),
    },
    Check {
        code: "unknown_special",
        run: |map, context| specials::unknown_special(map, context),
    },
    Check {
        code: "special_without_tag",
        run: |map, context| specials::special_without_tag(map, context),
    },
    Check {
        code: "tag_without_sector",
        run: |map, context| specials::tag_without_sector(map, context),
    },
    Check {
        code: "unclosed_sector",
        run: |map, _context| geometry::unclosed_sector(map),
    },
    Check {
        code: "crossing_lines",
        run: |map, _context| geometry::crossing_lines(map),
    },
    Check {
        code: "overlapping_lines",
        run: |map, _context| geometry::overlapping_lines(map),
    },
    Check {
        code: "check_skipped",
        run: |map, _context| geometry::check_skipped(map),
    },
    Check {
        code: "ceiling_below_floor",
        run: |map, _context| geometry::ceiling_below_floor(map),
    },
    Check {
        code: "thing_outside_map",
        run: |map, _context| things::thing_outside_map(map),
    },
    Check {
        code: "missing_texture",
        run: |map, context| resources::missing_texture(map, context),
    },
    Check {
        code: "missing_flat",
        run: |map, context| resources::missing_flat(map, context),
    },
    Check {
        code: "missing_required_texture",
        run: |map, _context| resources::missing_required_texture(map),
    },
    Check {
        code: "texture_check_skipped",
        run: |_map, context| resources::texture_check_skipped(context),
    },
    Check {
        code: "nodes_missing",
        run: |map, _context| node_lumps::nodes_missing(map),
    },
    Check {
        code: "blockmap_missing",
        run: |map, _context| node_lumps::blockmap_missing(map),
    },
    Check {
        code: "reject_size_mismatch",
        run: |map, _context| node_lumps::reject_size_mismatch(map),
    },
];

/// The check codes, in [`CHECKS`] order: what `validate_map` accepts as
/// `codes` and reports as `checks_run`.
#[must_use]
#[inline]
pub fn codes() -> Vec<&'static str> {
    CHECKS.iter().map(|check| check.code).collect()
}

/// What one [`run`] did and found.
#[derive(Debug)]
pub struct Outcome {
    /// The codes of the checks that ran, in [`CHECKS`] order.
    pub checks_run: Vec<&'static str>,
    /// Every issue the checks produced, in check order.
    pub issues: Vec<Issue>,
}

/// Runs the selected checks over `map` - all of them when `enabled` is
/// `None`, otherwise exactly those whose code the set holds - and stamps
/// each issue with the code of the check that produced it.
#[must_use]
#[inline]
pub fn run(map: &Map, context: &Context<'_>, enabled: Option<&BTreeSet<&'static str>>) -> Outcome {
    let mut checks_run: Vec<&'static str> = Vec::new();
    let mut issues: Vec<Issue> = Vec::new();
    for check in CHECKS {
        if enabled.is_some_and(|enabled_codes| !enabled_codes.contains(&check.code)) {
            continue;
        }
        checks_run.push(check.code);
        for issue in (check.run)(map, context) {
            issues.push(issue.with_code(check.code));
        }
    }
    Outcome { checks_run, issues }
}

/// `items` joined with `", "`, for issue messages.
pub(super) fn listed<T: core::fmt::Display>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;

    use serde_json::json;

    use super::{CHECKS, Context, Issue, ResourceIndex, Severity, codes, run};
    use crate::defs::definitions_for;
    use crate::wad::archive::Lump;
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// The built-in definitions and the empty resource index a default
    /// validation context needs.
    struct Tables {
        /// The DOOM II rows (things and the `Doom`-family specials).
        doom: crate::defs::Definitions,
        /// The `Hexen` specials rows.
        hexen: crate::defs::Definitions,
        /// A resource index defining nothing: the texture side reports
        /// its skip and the flat side stays quiet.
        resources: ResourceIndex,
    }

    /// The embedded tables, or a test failure when the embedded data broke.
    fn tables() -> Tables {
        Tables {
            doom: definitions_for(Game::Doom2).expect("the embedded Doom II table is valid"),
            hexen: definitions_for(Game::Hexen).expect("the embedded Hexen table is valid"),
            resources: ResourceIndex::default(),
        }
    }

    /// A validation context over the built-in tables.
    fn context(tables: &Tables) -> Context<'_> {
        Context {
            definitions: &tables.doom,
            hexen_specials: &tables.hexen,
            resources: &tables.resources,
        }
    }

    /// A vertex at `x`/`y`.
    const fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A line from `v1` to `v2` with `flags` and a front sidedef `front`.
    const fn side_line(v1: usize, v2: usize, flags: u32, front: Option<usize>) -> Linedef {
        Linedef {
            v1,
            v2,
            flags,
            special: 0,
            tag: 0,
            args: [0; 5],
            front,
            back: None,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A sidedef opening into `sector`.
    fn sector_side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Default::default()
        }
    }

    /// An untagged sector.
    fn plain_sector(tag: i32) -> Sector {
        Sector {
            tag,
            ..Default::default()
        }
    }

    /// A thing of `kind` at `x`/`y`.
    const fn thing_at(kind: i32, x: i32, y: i32) -> Thing {
        Thing {
            tid: 0,
            x,
            y,
            z: 0,
            angle: 0,
            kind,
            flags: 7,
            special: 0,
            args: [0; 5],
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// The two-room map of the integration fixtures: a hall and a raised
    /// back room split by a two-sided step line, with a player start and an
    /// imp. Every index is in range and used, the geometry is closed and
    /// uncrossed, and the block carries built node, reject and blockmap
    /// lumps, so nothing but info can show.
    fn two_room() -> Map {
        Map {
            name: name("MAP01"),
            format: MapFormat::Doom,
            udmf_namespace: None,
            things: vec![thing_at(1, 64, 128), thing_at(3001, 384, 128)],
            vertices: vec![
                vertex(0, 0),
                vertex(256, 0),
                vertex(512, 0),
                vertex(512, 256),
                vertex(256, 256),
                vertex(0, 256),
            ],
            linedefs: vec![
                side_line(0, 5, 1, Some(0)),
                side_line(5, 4, 1, Some(1)),
                Linedef {
                    back: Some(3),
                    ..side_line(4, 1, 4, Some(2))
                },
                side_line(1, 0, 1, Some(4)),
                side_line(4, 3, 1, Some(5)),
                side_line(3, 2, 1, Some(6)),
                side_line(2, 1, 1, Some(7)),
            ],
            sidedefs: vec![
                sector_side(0),
                sector_side(0),
                sector_side(0),
                sector_side(1),
                sector_side(0),
                sector_side(1),
                sector_side(1),
                sector_side(1),
            ],
            sectors: vec![plain_sector(0), plain_sector(1)],
            other: vec![
                Lump::new(name("NODES"), vec![0_u8; 28]),
                Lump::new(name("REJECT"), vec![0_u8; 1]),
                Lump::new(name("BLOCKMAP"), vec![0_u8; 12]),
            ],
        }
    }

    /// A map touching every registered code except the ones that need two
    /// well-formed elements: built to make all of them report.
    fn kitchen_sink() -> Map {
        let mut broken_end = side_line(5, 900, 1, None);
        broken_end.back = Some(900);
        let mut odd = side_line(0, 1, 4, Some(9));
        odd.special = 4_321_i32;
        odd.tag = 7_i32;
        Map {
            name: name("MAP01"),
            format: MapFormat::Doom,
            udmf_namespace: None,
            things: vec![thing_at(30_010, 10, 20)],
            vertices: vec![vertex(0, 0), vertex(0, 0)],
            linedefs: vec![odd, broken_end],
            sidedefs: vec![sector_side(42)],
            sectors: vec![plain_sector(0)],
            other: Vec::new(),
        }
    }

    #[test]
    fn severity_ordering_filters_the_mild_rows() {
        assert!(
            Severity::Error.passes(Severity::Info),
            "errors pass everything"
        );
        assert!(
            Severity::Warning.passes(Severity::Warning),
            "warnings pass their own level"
        );
        assert!(
            !Severity::Warning.passes(Severity::Error),
            "warnings do not pass an error minimum"
        );
        assert!(
            !Severity::Info.passes(Severity::Warning),
            "info is the mildest"
        );
        assert!(Severity::Info.passes(Severity::Info), "and passes itself");
    }

    #[test]
    fn issue_serialization_skips_empty_and_missing_parts() {
        let issue = Issue::new(Severity::Warning, "orphaned room".to_owned())
            .with_code("unused_sector")
            .sector(&[3]);
        assert_eq!(
            serde_json::to_value(&issue).expect("issues serialize"),
            json!({
                "severity": "warning",
                "code": "unused_sector",
                "message": "orphaned room",
                "sectors": [3_u64],
            }),
            "only the filled parts appear"
        );
        let placed = Issue::new(Severity::Error, "collapsed line".to_owned())
            .with_code("zero_length_line")
            .linedef(&[4])
            .vertex(&[2])
            .at(128, -64);
        assert_eq!(
            serde_json::to_value(&placed).expect("issues serialize"),
            json!({
                "severity": "error",
                "code": "zero_length_line",
                "message": "collapsed line",
                "vertices": [2_u64],
                "linedefs": [4_u64],
                "x": 128_i64,
                "y": -64_i64,
            }),
            "coordinates and both index lists appear"
        );
    }

    #[test]
    fn run_stamps_the_registered_code_on_every_issue() {
        let tables = tables();
        let outcome = run(&kitchen_sink(), &context(&tables), None);
        assert_eq!(
            outcome.checks_run,
            codes(),
            "with no filter every check in the table runs"
        );
        assert_eq!(
            outcome.checks_run.len(),
            CHECKS.len(),
            "the code list and the table agree"
        );
        let registered = codes();
        assert!(
            !outcome.issues.is_empty(),
            "the broken map must report something"
        );
        for issue in &outcome.issues {
            assert!(
                registered.contains(&issue.code),
                "issue carries an unregistered code {:?}: {issue:?}",
                issue.code
            );
            assert!(!issue.message.is_empty(), "issue without a message");
        }
        for wanted in [
            "vertex_out_of_range",
            "sidedef_out_of_range",
            "sector_out_of_range",
            "missing_front_side",
            "zero_length_line",
            "two_sided_flag_mismatch",
            "unused_sidedef",
            "unused_sector",
            "no_player1_start",
            "missing_coop_starts",
            "unknown_thing_type",
            "unknown_special",
            "tag_without_sector",
            "thing_outside_map",
            "texture_check_skipped",
            "nodes_missing",
            "blockmap_missing",
        ] {
            assert!(
                outcome.issues.iter().any(|issue| issue.code == wanted),
                "the kitchen-sink map should trip {wanted}"
            );
        }
    }

    #[test]
    fn every_geometry_resource_and_lump_code_is_registered() {
        let registered = codes();
        for wanted in [
            "unclosed_sector",
            "crossing_lines",
            "overlapping_lines",
            "check_skipped",
            "ceiling_below_floor",
            "thing_outside_map",
            "missing_texture",
            "missing_flat",
            "missing_required_texture",
            "texture_check_skipped",
            "nodes_missing",
            "blockmap_missing",
            "reject_size_mismatch",
        ] {
            assert!(
                registered.contains(&wanted),
                "{wanted} must be a filterable, reported code: {registered:?}"
            );
        }
    }

    #[test]
    fn run_filters_to_the_enabled_codes_in_table_order() {
        let tables = tables();
        let enabled: BTreeSet<&'static str> =
            ["unused_sector", "no_player1_start"].into_iter().collect();
        let outcome = run(&two_room(), &context(&tables), Some(&enabled));
        assert_eq!(
            outcome.checks_run,
            vec!["unused_sector", "no_player1_start"],
            "table order, requested subset"
        );
        assert!(
            outcome
                .issues
                .iter()
                .all(|issue| enabled.contains(&issue.code)),
            "only the enabled checks report: {:?}",
            outcome.issues
        );
        let nothing = run(&two_room(), &context(&tables), Some(&BTreeSet::new()));
        assert!(nothing.checks_run.is_empty(), "an empty set runs nothing");
        assert!(nothing.issues.is_empty(), "and finds nothing");
    }

    #[test]
    fn the_clean_two_room_map_reports_only_info() {
        let tables = tables();
        let outcome = run(&two_room(), &context(&tables), None);
        let heavy: Vec<&Issue> = outcome
            .issues
            .iter()
            .filter(|issue| issue.severity != Severity::Info)
            .collect();
        assert!(
            heavy.is_empty(),
            "a clean map must not error or warn: {heavy:?}"
        );
        assert!(
            outcome
                .issues
                .iter()
                .any(|issue| issue.code == "missing_coop_starts"),
            "the single-info coop set is the expected finding: {:?}",
            outcome.issues
        );
    }
}
