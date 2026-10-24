//! Shared builders for the query engine's tests: the definitions and family
//! contexts, the filter and query makers, the expected-hits reader, the three
//! fixture maps that carry a role, a diagonal and flags, the bounded-query
//! maker and the refusal runner.
//!
//! `filters` runs every op over the fixture's stored and derived fields and
//! both `within` bounds; `checks` combines filters, reads every refusal and
//! the family tables, and checks the wire shape and the plain context reads.

mod checks;
mod filters;

use serde_json::Value;

use super::{ElementKind, Filter, Op, Query, QueryContext, Within, run_query};
use crate::defs::{Definitions, definitions_for};
use crate::wad::game::Game;
use crate::wad::map::Map;
use crate::wad::map::common::fixtures::{solid_line, two_room_map};
use crate::wad::map::flags::FlagFamily;

/// The built-in `DOOM II` rows, naming the fixture's things.
pub(super) fn defs() -> Definitions {
    definitions_for(Game::Doom2).expect("the embedded Doom II table is valid")
}

/// A `Doom`-family context over `defs`.
pub(super) fn doom_ctx(defs: &Definitions) -> QueryContext<'_> {
    QueryContext::new(FlagFamily::Doom, defs)
}

/// A `Hexen`-family context over `defs`.
pub(super) fn hexen_ctx(defs: &Definitions) -> QueryContext<'_> {
    QueryContext::new(FlagFamily::Hexen, defs)
}

/// One filter row.
pub(super) fn filter(field: &str, op: Op, value: Value) -> Filter {
    Filter {
        field: field.to_owned(),
        op,
        value,
    }
}

/// A query over just these filters and bound.
pub(super) fn query(kind: ElementKind, filters: Vec<Filter>, within: Option<Within>) -> Query {
    Query {
        kind,
        r#where: filters,
        within,
    }
}

/// A one-filter unbounded query.
pub(super) fn field_query(kind: ElementKind, f: Filter) -> Query {
    query(kind, vec![f], None)
}

/// Assert every `needles` appears in `message`.
pub(super) fn assert_mentions(message: &str, needles: &[&str]) {
    for needle in needles {
        assert!(message.contains(needle), "{needle} missing from: {message}");
    }
}

/// Run `q` over `map`, expecting success.
pub(super) fn hits(map: &Map, q: &Query) -> Vec<usize> {
    let defs = defs();
    run_query(map, &doom_ctx(&defs), q).expect("test queries are valid")
}

/// The two rooms plus a `secret`-special and `F_SKY1` role marker on
/// sector 1, for the derived-field tests.
pub(super) fn roley_map() -> Map {
    let mut map = two_room_map();
    let sector = map.sectors.get_mut(1).expect("the fixture has 2 sectors");
    sector.special = 9_i32;
    sector.ceiling_flat = "F_SKY1".to_owned();
    map
}

/// The fixture plus a diagonal line 7 from `(0, 0)` to `(512, 256)`.
pub(super) fn diagonal_map() -> Map {
    let mut map = two_room_map();
    map.linedefs.push(solid_line(0, 3, 0));
    map
}

/// The fixture with an ambush-flagged start and a thing in the void.
pub(super) fn flaggy_map() -> Map {
    let mut map = two_room_map();
    map.things
        .get_mut(0)
        .expect("the fixture has a start")
        .flags |= 8;
    map.things
        .push(crate::wad::map::common::fixtures::spawn(99999, 600, 600, 0));
    map
}

/// A query for `kind` bounded by `bbox` alone.
pub(super) fn boxed(kind: ElementKind, bbox: [i64; 4]) -> Query {
    query(kind, vec![], Some(Within::Bbox { bbox }))
}

/// Refuse each query in `cases`, checking the code and message.
pub(super) fn refuse_all(cases: Vec<(Query, Vec<&str>)>) {
    let defs = defs();
    let map = two_room_map();
    for (q, needles) in cases {
        let error =
            run_query(&map, &doom_ctx(&defs), &q).expect_err("these filters must be refused");
        assert_eq!(error.code(), "invalid_argument", "{q:?} code");
        assert_mentions(&error.to_string(), &needles);
    }
}
