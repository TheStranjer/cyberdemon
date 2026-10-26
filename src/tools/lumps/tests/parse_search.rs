//! The shared argument parsers and the search criteria: every filter's
//! accepted text and its refusals, the hex needles and their case rules, the
//! `AND` of all the criteria together and the hint text that names them.

use crate::session::Scope;
use crate::tools::lumps::list::{Match, Needle, Search, find_needle, parse_pair};
use crate::tools::lumps::parse::{
    kind_names, namespace_names, parse_hex, parse_kind, parse_map, parse_namespace,
};
use crate::tools::lumps::{DEFAULT_LIMIT, MAX_LIMIT};
use crate::tools::parse_limit;
use crate::wad::classify::{LumpKind, Namespace};

use super::{code, glob_search, name, row};

#[test]
fn scope_defaults_to_merged_and_parses_every_form() {
    assert_eq!(
        Scope::parse_or_merged(None).expect("default"),
        Scope::Merged
    );
    assert_eq!(
        Scope::parse_or_merged(Some("working")).expect("working"),
        Scope::Working
    );
    assert_eq!(
        Scope::parse_or_merged(Some("layer:1")).expect("layer"),
        Scope::Layer(1)
    );
    let error = Scope::parse_or_merged(Some("bogus")).expect_err("not a scope");
    assert_eq!(code(&error), "invalid_argument", "bad scope code");
    assert_eq!(
        Scope::Layer(2).to_string(),
        "layer:2",
        "the Display label round-trips"
    );
}

#[test]
fn namespace_and_kind_filters_parse_names_or_refuse() {
    assert_eq!(parse_namespace(None).expect("absent"), None);
    assert_eq!(
        parse_namespace(Some("Flats")).expect("flats"),
        Some(Namespace::Flats)
    );
    let error = parse_namespace(Some("nope")).expect_err("not a namespace");
    assert_eq!(code(&error), "invalid_argument", "bad namespace code");
    assert!(
        error.to_string().contains("sprites"),
        "the hint lists the names: {error}"
    );
    assert_eq!(
        parse_kind(Some("DOOM_SOUND")).expect("kind"),
        Some(LumpKind::DoomSound)
    );
    let kind = parse_kind(Some("nope")).expect_err("not a kind");
    assert_eq!(code(&kind), "invalid_argument", "bad kind code");
    assert!(
        kind.to_string().contains("map_marker"),
        "the hint lists the kinds: {kind}"
    );
}

#[test]
fn map_filter_parses_lump_names_strictly() {
    assert_eq!(parse_map(None).expect("absent"), None);
    assert_eq!(
        parse_map(Some("map01")).expect("parsed"),
        Some(name("MAP01")),
        "the name parser uppercases"
    );
    let error = parse_map(Some("TOO LONG FOR A NAME")).expect_err("over eight bytes");
    assert_eq!(code(&error), "invalid_argument", "bad map name code");
}

#[test]
fn limit_defaults_and_stays_in_range() {
    assert_eq!(
        parse_limit(None, DEFAULT_LIMIT, MAX_LIMIT, "limit").expect("default"),
        200,
        "documented default"
    );
    assert_eq!(
        parse_limit(Some(2000), DEFAULT_LIMIT, MAX_LIMIT, "limit").expect("max"),
        2000,
        "the cap"
    );
    assert_eq!(
        parse_limit(Some(1), DEFAULT_LIMIT, MAX_LIMIT, "limit").expect("min"),
        1,
        "the floor"
    );
    for bad in [0_usize, 2001] {
        let error =
            parse_limit(Some(bad), DEFAULT_LIMIT, MAX_LIMIT, "limit").expect_err("out of range");
        assert_eq!(code(&error), "invalid_argument", "limit {bad} code");
    }
}

#[test]
fn hex_needles_accept_digits_with_spaces_and_refuse_the_rest() {
    assert_eq!(parse_hex(None).expect("absent"), None);
    assert_eq!(
        parse_hex(Some("4D 55 53 1A")).expect("spaced"),
        Some(vec![0x4D_u8, 0x55, 0x53, 0x1A])
    );
    assert_eq!(
        parse_hex(Some("4d55")).expect("lowercase"),
        Some(vec![0x4D_u8, 0x55])
    );
    let odd = parse_hex(Some("4D5")).expect_err("odd digits");
    assert_eq!(code(&odd), "invalid_argument", "odd hex code");
    assert!(
        odd.to_string().contains("odd"),
        "the message says odd: {odd}"
    );
    let bad = parse_hex(Some("0G")).expect_err("not hex");
    assert_eq!(code(&bad), "invalid_argument", "non-hex code");
    assert!(
        bad.to_string().contains("'G'"),
        "the message names the digit: {bad}"
    );
}

#[test]
fn pair_parsing_is_strict() {
    assert_eq!(parse_pair(&['4', 'd']), Some(0x4D), "mixed case digits");
    assert_eq!(parse_pair(&['F', 'F']), Some(0xFF), "top of the range");
    assert_eq!(parse_pair(&['Z', 'Q']), None, "non-hex characters");
    assert_eq!(parse_pair(&['+', 'f']), None, "a sign is not a hex digit");
    assert_eq!(
        parse_pair(&['\u{663}', '4']),
        None,
        "non-ASCII digits are refused"
    );
    assert_eq!(parse_pair(&[]), None, "an empty pair is not a byte");
    assert_eq!(parse_pair(&['7']), None, "a lone digit is not a byte");
}

#[test]
fn needles_match_with_the_right_case_rule() {
    let data = b"Thing 1 (Troop)".to_vec();
    let text = Needle {
        bytes: b"TROOP",
        ignore_case: true,
    };
    let hex = Needle {
        bytes: b"(T",
        ignore_case: false,
    };
    let lone = Needle {
        bytes: b"",
        ignore_case: false,
    };
    assert_eq!(find_needle(&data, &text), Some(9), "case folds both ways");
    assert_eq!(find_needle(&data, &hex), Some(8), "exact bytes");
    assert_eq!(
        find_needle(&data, &lone),
        Some(0),
        "an empty needle matches at 0"
    );
    let missing = Needle {
        bytes: b"(t",
        ignore_case: false,
    };
    assert_eq!(find_needle(&data, &missing), None, "hex stays case-exact");
    let absent = Needle {
        bytes: b"NOPE",
        ignore_case: true,
    };
    assert_eq!(find_needle(&data, &absent), None, "absent text");
}

#[test]
fn judge_combines_every_criterion_with_and() {
    let empty = Search {
        pattern: None,
        kind: None,
        namespace: None,
        text: None,
        hex: None,
    };
    assert!(empty.is_empty(), "no criteria at all");
    let floor = row("FLOOR1", LumpKind::Flat, Some(Namespace::Flats), b"aa");
    let dehacked = row("DEHACKED", LumpKind::Dehacked, None, b"Thing 1 (Troop)");
    assert!(!glob_search("F*").is_empty(), "a pattern is a criterion");
    assert!(
        matches!(glob_search("F*").judge(&floor), Match::Hit(None)),
        "a name-only hit carries no offset"
    );
    assert!(
        matches!(glob_search("?LOOR1").judge(&floor), Match::Hit(None)),
        "question marks"
    );
    assert!(
        matches!(glob_search("F*").judge(&dehacked), Match::Miss),
        "a name miss drops the row"
    );
    let conflicting = Search {
        pattern: None,
        kind: Some(LumpKind::Dehacked),
        namespace: Some(Namespace::Flats),
        text: None,
        hex: None,
    };
    assert!(
        matches!(conflicting.judge(&dehacked), Match::Miss),
        "kind AND namespace must both hold"
    );
    let content = Search {
        pattern: None,
        kind: None,
        namespace: None,
        text: Some(b"TROOP".to_vec()),
        hex: Some(b"(T".to_vec()),
    };
    assert!(
        matches!(content.judge(&dehacked), Match::Hit(Some(8))),
        "the lowest content offset wins"
    );
    let missing_hex = Search {
        pattern: None,
        kind: None,
        namespace: None,
        text: Some(b"TROOP".to_vec()),
        hex: Some(b"ZZ".to_vec()),
    };
    assert!(
        matches!(missing_hex.judge(&dehacked), Match::Miss),
        "one absent needle drops the row"
    );
}

#[test]
fn error_hints_list_every_documented_name() {
    assert_eq!(
        namespace_names(),
        "sprites, flats, patches, colormaps, acs, voices, hires, textures",
        "the namespace hint text"
    );
    assert!(
        kind_names().starts_with("marker, map_marker, map_data, palette"),
        "the kind hint leads with the directory kinds"
    );
    assert!(
        kind_names().contains("pc_speaker"),
        "the kind hint is complete"
    );
}
