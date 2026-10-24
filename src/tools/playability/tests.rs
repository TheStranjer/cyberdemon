//! Unit tests of the playability group: the skill and limit parsers, the fixed
//! blocker, step and stuck hint wording, the sector selection and the paging of
//! a description, and the two runs' refusals before the session is touched.

use super::check::{Analysis, CheckPlayabilityParams, check_run, parse_skill};
use super::describe::{
    DESCRIBE_DEFAULT_LIMIT, DESCRIBE_MAX_LIMIT, DescribeMapParams, describe_run, page_rows,
    parse_limit, sector_selection,
};
use super::hints::{blocker_hint, step_hint, stuck_hint};
use crate::defs::definitions_for;
use crate::state::AppState;
use crate::wad::game::Game;
use crate::wad::map::flags::{FlagFamily, Skill};
use crate::wad::map::playability::{self, Blocker, BlockerReason, KeyColor, Stuck, StuckReason};
use crate::wad::map::{Map, Thing};
use crate::wad::name::LumpName;

/// A `check_playability` call over just these parameter texts.
fn args(skill: Option<&str>) -> CheckPlayabilityParams {
    CheckPlayabilityParams {
        map: None,
        skill: skill.map(str::to_owned),
        render: None,
    }
}

/// A `describe_map` call over just these parameter values, unpinned
/// fields at their defaults (`render` off so no call ever publishes).
fn describe_args(sectors: Option<Vec<usize>>, limit: Option<usize>) -> DescribeMapParams {
    DescribeMapParams {
        map: None,
        sectors,
        offset: None,
        limit,
        render: Some(false),
    }
}

/// A parsed test map name.
fn test_name() -> LumpName {
    LumpName::parse("MAP01").expect("test names are valid")
}

/// A border blocker on line 17 from sector 2 into sector 5.
fn border(reason: BlockerReason) -> Blocker {
    Blocker {
        sector: 5,
        from: Some(2),
        line: Some(17),
        reason,
    }
}

#[test]
fn parse_skill_defaults_to_medium_and_names_the_three() {
    assert_eq!(parse_skill(None).expect("absent"), Skill::Medium);
    assert_eq!(
        parse_skill(Some("HARD")).expect("case-insensitive"),
        Skill::Hard
    );
    assert_eq!(parse_skill(Some("easy")).expect("easy"), Skill::Easy);
    let error = parse_skill(Some("nightmare")).expect_err("not a skill");
    assert_eq!(error.code(), "invalid_argument", "bad skill code");
    let message = error.to_string();
    assert!(
        message.contains("\"easy\"")
            && message.contains("\"medium\"")
            && message.contains("\"hard\""),
        "the message names all three values: {message}"
    );
}

#[test]
fn blocker_hints_name_their_fix_tools_per_reason() {
    assert_eq!(
        blocker_hint(&border(BlockerReason::Impassable)),
        "clear the impassable flag on line 17 (edit_linedef)",
    );
    assert_eq!(
        blocker_hint(&border(BlockerReason::Narrow {
            length: Some(24_u64)
        })),
        "widen the opening to at least 32 units (move_vertex)",
    );
    assert_eq!(
        blocker_hint(&border(BlockerReason::Locked {
            color: KeyColor::Blue
        })),
        "place a blue key in a reachable area (add_thing)",
    );
    assert_eq!(
        blocker_hint(&border(BlockerReason::DoorNeverOpened)),
        "make the switch reachable or use add_door style normal",
    );
    assert_eq!(
        blocker_hint(&border(BlockerReason::TooLowSector { height: 40_i32 })),
        "raise the ceiling (edit_sector) to at least floor + 56",
    );
    assert_eq!(
        blocker_hint(&border(BlockerReason::Isolated)),
        "connect it (create_sector_from_loop / add_door / build_map_from_plan connection)",
    );
}

#[test]
fn the_step_hint_names_the_sector_pair_when_there_is_one() {
    assert_eq!(
        step_hint(&border(BlockerReason::StepTooHigh { delta: 40_i64 })),
        "lower sector 5's floor to no more than 24 above sector 2, or add_stairs / add_lift",
    );
    let lonely = Blocker {
        sector: 5,
        from: None,
        line: None,
        reason: BlockerReason::StepTooHigh { delta: 40_i64 },
    };
    assert_eq!(
        step_hint(&lonely),
        "add_stairs / add_lift or change floor heights (edit_sector)",
    );
}

/// The stuck hint for a thing of `kind` at the head of a two-row map,
/// read against the built-in `DOOM II` definitions.
fn stuck_hint_for(kind: i32, reason: StuckReason) -> String {
    let map = Map {
        things: vec![
            Thing {
                kind,
                ..Default::default()
            },
            Thing {
                kind: 3001,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let defs = definitions_for(Game::Doom2).expect("the embedded Doom II table is valid");
    let a = Analysis {
        map: &map,
        defs: &defs,
        family: FlagFamily::Doom,
        skill: Skill::Medium,
        found: playability::Playability::default(),
        held_out: Vec::new(),
        stuck: Vec::new(),
        start: None,
    };
    stuck_hint(&a, &Stuck { thing: 0, reason })
}

#[test]
fn stuck_hints_quote_the_line_radius_and_the_partner() {
    assert_eq!(
        stuck_hint_for(3001, StuckReason::InWall { line: 12 }),
        "move it at least 20 units away from line 12 (edit_thing)",
        "the imp's own radius"
    );
    assert_eq!(
        stuck_hint_for(8, StuckReason::InWall { line: 12 }),
        "move it at least 16 units away from line 12 (edit_thing)",
        "the backpack's own radius rides along"
    );
    assert_eq!(
        stuck_hint_for(3001, StuckReason::Overlaps { other: 1 }),
        "move thing 0 or thing 1 apart (edit_thing)",
    );
    assert_eq!(
        stuck_hint_for(3001, StuckReason::OutsideMap),
        "move it inside a sector (edit_thing)",
    );
    assert!(
        stuck_hint_for(3001, StuckReason::LowCeiling { height: 30_i32 }).contains("edit_sector"),
        "the low-ceiling hint names both fixes"
    );
}

#[tokio::test]
async fn check_run_refuses_a_bad_skill_and_the_empty_session() {
    let state = AppState::default();
    let refused = check_run(&state, &args(Some("nightmare")))
        .await
        .expect_err("not a skill");
    assert_eq!(
        refused.code(),
        "invalid_argument",
        "a bad skill is refused even with no WAD open"
    );
    assert_eq!(
        check_run(&state, &args(None))
            .await
            .expect_err("no maps at all")
            .code(),
        "invalid_argument",
        "the empty-session no-map refusal comes from the shared resolver"
    );
}

#[test]
fn parse_limit_defaults_to_a_page_and_names_the_range_when_refusing() {
    assert_eq!(
        parse_limit(None).expect("absent"),
        DESCRIBE_DEFAULT_LIMIT,
        "the default pages a hundred and fifty rows"
    );
    assert_eq!(parse_limit(Some(1)).expect("one"), 1, "one row is legal");
    assert_eq!(
        parse_limit(Some(DESCRIBE_MAX_LIMIT)).expect("max"),
        DESCRIBE_MAX_LIMIT,
        "the cap itself is legal"
    );
    for bad in [0_usize, DESCRIBE_MAX_LIMIT.checked_add(1).expect("small")] {
        let error = parse_limit(Some(bad)).expect_err("this cap is out of range");
        assert_eq!(error.code(), "invalid_argument", "bad limit code");
        assert!(
            error.to_string().contains("between 1 and 500"),
            "the message names the range: {error}"
        );
    }
}

#[test]
fn sector_selection_sorts_dedupes_and_defaults_to_every_sector() {
    assert_eq!(
        sector_selection(None, 3, test_name()).expect("no filter"),
        vec![0_usize, 1, 2],
        "unset describes every stored sector"
    );
    assert!(
        sector_selection(None, 0, test_name())
            .expect("no sectors")
            .is_empty(),
        "an empty map selects nothing"
    );
    let wanted = vec![2_usize, 0, 2];
    assert_eq!(
        sector_selection(Some(&wanted), 5, test_name()).expect("two rows asked"),
        vec![0_usize, 2],
        "deduplicated and ascending"
    );
}

#[test]
fn sector_selection_refuses_past_the_end_with_the_valid_range() {
    let error = sector_selection(Some(&[5_usize]), 5, test_name()).expect_err("5 is not below 5");
    assert_eq!(error.code(), "not_found", "a past-the-end index refuses");
    let message = error.to_string();
    assert!(
        message.contains("0..5") && message.contains("`sectors`"),
        "the message names the valid range and the parameter: {message}"
    );
    let empty =
        sector_selection(Some(&[0_usize]), 0, test_name()).expect_err("nothing to index into");
    assert!(
        empty.to_string().contains("0..0"),
        "an empty map's range is empty too: {empty}"
    );
}

#[test]
fn page_rows_windows_the_selection_and_points_at_the_rest() {
    let all: Vec<usize> = (0..5).collect();
    let (first, next) = page_rows(&all, 0, 2);
    assert_eq!(first, vec![0_usize, 1], "the head of the list");
    assert_eq!(next, Some(2), "and where the rest continues");
    let (tail, done) = page_rows(&all, 3, 500);
    assert_eq!(
        tail,
        vec![3_usize, 4],
        "a page longer than the rest stops at it"
    );
    assert_eq!(done, None, "with nothing left behind");
    let (nothing, past) = page_rows(&all, 9, 2);
    assert!(nothing.is_empty(), "an offset past the end is no error");
    assert_eq!(past, None, "and it has no next page");
    let (empty, empty_next) = page_rows(&[], 0, DESCRIBE_DEFAULT_LIMIT);
    assert!(empty.is_empty(), "nothing selected, nothing paged");
    assert_eq!(empty_next, None, "and no next offset to chase");
}

#[tokio::test]
async fn describe_run_refuses_a_bad_limit_and_the_empty_session() {
    let state = AppState::default();
    let refused = describe_run(&state, &describe_args(None, Some(0)))
        .await
        .expect_err("zero rows");
    assert_eq!(
        refused.code(),
        "invalid_argument",
        "a bad limit is refused even with no WAD open"
    );
    assert_eq!(
        describe_run(&state, &describe_args(None, Some(501)))
            .await
            .expect_err("rows over the cap")
            .code(),
        "invalid_argument",
        "and so is one above the maximum page"
    );
    assert_eq!(
        describe_run(&state, &describe_args(None, None))
            .await
            .expect_err("no maps at all")
            .code(),
        "invalid_argument",
        "the empty-session no-map refusal comes from the shared resolver"
    );
}
