//! The `find_elements` read: [`parse_limit`]'s row-cap range,
//! [`Page::window`]'s window over the ascending hits, the compact summary
//! [`rows`] builds per kind and the parameter and map errors
//! [`find_run`] reports.

use serde_json::json;

use crate::defs::definitions_for;
use crate::state::AppState;
use crate::tools::query::find::{Row, find_run, rows};
use crate::tools::query::{DEFAULT_LIMIT, MAX_LIMIT};
use crate::tools::{Page, parse_limit};
use crate::wad::game::Game;
use crate::wad::map::query::{ElementKind, QueryContext};

use super::{mini_map, params};

#[test]
fn parse_limit_pages_between_one_and_a_thousand() {
    let shared = |value| parse_limit(value, DEFAULT_LIMIT, MAX_LIMIT, "limit");
    assert_eq!(shared(None).expect("absent"), DEFAULT_LIMIT);
    assert_eq!(shared(Some(1)).expect("one"), 1);
    assert_eq!(shared(Some(MAX_LIMIT)).expect("max"), MAX_LIMIT);
    for bad in [0, MAX_LIMIT.checked_add(1).expect("small")] {
        let error = shared(Some(bad)).expect_err("out of range");
        assert_eq!(error.code(), "invalid_argument", "bad limit code");
        assert!(
            error.to_string().contains("between 1 and 1000"),
            "the message names the range: {error}"
        );
    }
}

#[test]
fn page_hits_windows_the_list_and_names_the_rest() {
    let hits: Vec<usize> = (0..6).collect();
    let (first, continues) = Page::new(0, 2).window(&hits);
    assert_eq!(first, vec![0_usize, 1], "the leading page");
    assert_eq!(continues, Some(2), "the rest continues at 2");
    let (middle, more) = Page::new(2, 2).window(&hits);
    assert_eq!(middle, vec![2_usize, 3], "the middle page");
    assert_eq!(more, Some(4), "and one page more");
    let (last, rest) = Page::new(4, 2).window(&hits);
    assert_eq!(last, vec![4_usize, 5], "the final page");
    assert_eq!(rest, None, "nothing stayed behind");
    let (past, beyond) = Page::new(9, 2).window(&hits);
    assert!(
        past.is_empty(),
        "an offset past the end is empty, not an error"
    );
    assert_eq!(beyond, None, "and has no continuation");
    let (all, tail) = Page::new(1, 100).window(&hits);
    assert_eq!(
        all,
        vec![1_usize, 2, 3, 4, 5],
        "a generous page takes the rest"
    );
    assert_eq!(tail, None, "leaving nothing to continue at");
}

#[test]
fn rows_summarize_each_kind_in_its_documented_shape() {
    let map = mini_map();
    let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
    let ctx = QueryContext::new(map.flag_family(), &defs);
    let sector = rows(ElementKind::Sectors, &[0_usize], &map, &ctx)
        .into_iter()
        .next()
        .expect("one row");
    assert_eq!(
        serde_json::to_value(&sector).expect("serializable"),
        json!({
            "index": 0_i64, "floor": 0_i64, "ceiling": 128_i64, "light": 170_i64,
            "floor_flat": "FLOOR1", "ceiling_flat": "F_SKY1",
            "special": 9_i64, "tag": 3_i64,
        }),
        "the sector shape"
    );
    let line = rows(ElementKind::Linedefs, &[0_usize], &map, &ctx)
        .into_iter()
        .next()
        .expect("one row");
    assert!(matches!(line, Row::Linedef(..)), "the linedef variant");
    assert_eq!(
        serde_json::to_value(&line).expect("serializable"),
        json!({
            "index": 0_i64, "v1": 0_i64, "v2": 1_i64, "special": 10_i64, "tag": 3_i64,
            "front_sector": 0_i64, "back_sector": null,
        }),
        "the linedef shape with a resolved front and no back"
    );
    let thing = rows(ElementKind::Things, &[0_usize], &map, &ctx)
        .into_iter()
        .next()
        .expect("one row");
    assert_eq!(
        serde_json::to_value(&thing).expect("serializable"),
        json!({
            "index": 0_i64, "type": 1_i64, "name": "Player 1 Start",
            "x": 64_i64, "y": 128_i64, "angle": 90_i64,
        }),
        "the thing shape, named through the definitions"
    );
    let side = rows(ElementKind::Sidedefs, &[0_usize], &map, &ctx)
        .into_iter()
        .next()
        .expect("one row");
    assert_eq!(
        serde_json::to_value(&side).expect("serializable"),
        json!({"index": 0_i64, "sector": 0_i64, "upper": "", "middle": "STARTAN", "lower": ""}),
        "the sidedef shape"
    );
    let vertex = rows(ElementKind::Vertices, &[0_usize, 1_usize], &map, &ctx);
    assert_eq!(vertex.len(), 2, "both vertices");
    assert_eq!(
        serde_json::to_value(&vertex[1_usize]).expect("serializable"),
        json!({"index": 1_i64, "x": 128_i64, "y": 0_i64}),
        "the vertex shape"
    );
}

#[test]
fn rows_drop_indices_that_left_their_block() {
    let map = mini_map();
    let defs = definitions_for(Game::Doom2).expect("the embedded table is valid");
    let ctx = QueryContext::new(map.flag_family(), &defs);
    assert!(
        rows(ElementKind::Sectors, &[0_usize, 42_usize], &map, &ctx).len() == 1,
        "the stale index contributes nothing"
    );
}

#[test]
fn find_run_reports_parameter_and_map_errors() {
    let state = AppState::default();
    let mut bad_limit = params(ElementKind::Sectors, vec![]);
    bad_limit.limit = Some(0);
    assert_eq!(
        find_run(&state, &bad_limit).expect_err("zero rows").code(),
        "invalid_argument",
        "the limit is checked before the session is touched"
    );
    let no_map = params(ElementKind::Things, vec![]);
    assert_eq!(
        find_run(&state, &no_map)
            .expect_err("nothing is open")
            .code(),
        "not_found",
        "an explicit unknown map over an empty session is not_found"
    );
    let mut anonymous = no_map;
    anonymous.map = None;
    assert_eq!(
        find_run(&state, &anonymous)
            .expect_err("no map default exists")
            .code(),
        "invalid_argument",
        "the shared resolver names the no-map problem"
    );
}
