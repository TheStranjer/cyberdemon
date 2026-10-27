//! The `place_things` tests: the wire parsing of numbers, names,
//! boxes, circles and angles, the area and count refusals, the sector
//! split, the centre-outward spread and the dressed thing rows.

use serde_json::from_str;

use crate::defs::definitions_for;
use crate::error::CyberError;
use crate::tools::features::place::{
    check_sectors, dry_edit, place_on, presence_bits, target_areas,
};
use crate::tools::features::place_params::{
    AngleParam, NeedSpec, PlaceRequest, PlaceThingsParams, ThingKindParam, facing_of, need_spec,
    place_kind, place_request, thing_bit,
};
use crate::wad::game::Game;
use crate::wad::map::Thing;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::placement::{Facing, player_start, snap_angle};
use crate::wad::name::LumpName;

use super::two_rooms;

/// Wire JSON parsed into `place_things` arguments.
fn place_args(text: &str) -> PlaceThingsParams {
    from_str(text).expect("test parameters parse")
}

/// The DOOM definitions every `place_things` test resolves against.
fn place_definitions() -> crate::defs::Definitions {
    definitions_for(Game::Doom).expect("the embedded tables parse")
}

/// The minimum legal request these tests vary.
const MINIMAL: &str = r#"{"thing": 3001, "sectors": [0], "count": 1}"#;

/// A resolved request, panicking on refusals the test did not expect.
fn resolved(text: &str) -> PlaceRequest {
    place_request(&place_args(text), &place_definitions()).expect("test parameters resolve")
}

/// The refusal message of a failed resolution.
fn refused(text: &str) -> CyberError {
    place_request(&place_args(text), &place_definitions()).expect_err("the test asks for a refusal")
}

#[test]
fn place_params_read_numbers_names_boxes_circles_and_angles() {
    let typed = place_args(MINIMAL);
    assert!(
        matches!(typed.thing, ThingKindParam::Type(3001)),
        "a number is a doomednum"
    );
    assert_eq!(typed.seed, 1_u32, "the seed defaults to one");
    assert!(
        !typed.multiplayer_only && !typed.ambush && !typed.dry_run.get(),
        "bits default off"
    );
    let named = place_args(
        r#"{"thing": "imp", "bbox": [0, 0, 64, 64], "per_sector": 2, "angle": "face_center"}"#,
    );
    assert!(
        matches!(named.thing, ThingKindParam::Name(text) if text == "imp"),
        "a string is a name"
    );
    assert!(
        matches!(named.angle, Some(AngleParam::Mode(_))),
        "a mode string"
    );
    let degrees = place_args(
        r#"{"thing": 2035, "near": {"x": 0, "y": 0, "radius": 8}, "angle": 70, "count": 1}"#,
    );
    assert!(
        matches!(degrees.angle, Some(AngleParam::Degrees(70_i32))),
        "a number is degrees"
    );
    let dressed = place_args(
        r#"{"thing": 2035, "sectors": [0], "count": 1, "tid": 7, "special": {"number": 10, "args": [1, 2, 3, 4, 5]}}"#,
    );
    assert_eq!(dressed.tid, Some(7_i32), "the script fields ride along");
    assert_eq!(
        dressed.special.map(|row| row.number),
        Some(10_i32),
        "the special number too"
    );
}

#[test]
fn place_kind_resolves_names_and_refuses_ambiguity() {
    let definitions = place_definitions();
    let request = place_args(r#"{"thing": "imp", "sectors": [0], "count": 1}"#);
    assert_eq!(
        place_kind(&request.thing, &definitions).expect("Imp is exact"),
        3001_u32
    );
    let skul = refused(r#"{"thing": "skul", "sectors": [0], "count": 1}"#);
    assert_eq!(skul.code(), "invalid_argument", "an ambiguous name");
    assert!(skul.to_string().contains("ambiguous"), "{skul}");
    assert!(skul.to_string().contains("Skull on Pole"), "{skul}");
    let ghost = refused(r#"{"thing": "platypus", "sectors": [0], "count": 1}"#);
    assert!(
        ghost.to_string().contains("no thing named 'platypus'"),
        "{ghost}"
    );
}

#[test]
fn place_wires_refuse_the_area_and_count_violations() {
    let neither = refused(r#"{"thing": 3001, "count": 1}"#);
    assert!(
        neither.to_string().contains("exactly one of sectors"),
        "{neither}"
    );
    let both_areas =
        refused(r#"{"thing": 3001, "sectors": [0], "bbox": [0, 0, 8, 8], "count": 1}"#);
    assert_eq!(both_areas.code(), "invalid_argument", "two areas");
    let both_counts = refused(r#"{"thing": 3001, "sectors": [0], "count": 2, "per_sector": 2}"#);
    assert!(
        both_counts.to_string().contains("not both"),
        "{both_counts}"
    );
    let no_counts = refused(r#"{"thing": 3001, "sectors": [0]}"#);
    assert!(no_counts.to_string().contains("pass count"), "{no_counts}");
    let zero = refused(r#"{"thing": 3001, "sectors": [0], "count": 0}"#);
    assert!(
        zero.to_string().contains("count 0 is out of range"),
        "{zero}"
    );
    let huge = refused(r#"{"thing": 3001, "sectors": [0], "count": 257}"#);
    assert!(
        huge.to_string().contains("count 257 is out of range"),
        "{huge}"
    );
    let empty = refused(r#"{"thing": 3001, "sectors": [], "count": 1}"#);
    assert!(empty.to_string().contains("sectors is empty"), "{empty}");
    let backwards = refused(r#"{"thing": 3001, "bbox": [8, 8, 0, 0], "count": 1}"#);
    assert!(backwards.to_string().contains("backwards"), "{backwards}");
    let flat_circle =
        refused(r#"{"thing": 3001, "near": {"x": 0, "y": 0, "radius": 0}, "count": 1}"#);
    assert!(
        flat_circle.to_string().contains("not a circle"),
        "{flat_circle}"
    );
    let cold = refused(r#"{"thing": 3001, "sectors": [0], "count": 1, "clearance": 0}"#);
    assert!(
        cold.to_string().contains("clearance 0 is out of range"),
        "{cold}"
    );
    let nightmare =
        refused(r#"{"thing": 3001, "sectors": [0], "count": 1, "skills": ["nightmare"]}"#);
    assert!(
        nightmare.to_string().contains("unknown skill"),
        "{nightmare}"
    );
    let blank = refused(r#"{"thing": 3001, "sectors": [0], "count": 1, "skills": []}"#);
    assert!(blank.to_string().contains("skills is empty"), "{blank}");
}

#[test]
fn the_demand_and_facing_shapes_resolve_exactly_one_way() {
    let total = need_spec(&place_args(MINIMAL)).expect("a total");
    assert!(matches!(total, NeedSpec::Total(1)), "count is the total");
    let facing = facing_of(Some(&AngleParam::Degrees(70)), 3).expect("degrees");
    assert_eq!(facing, Facing::Fixed(90_i32), "70 snaps to 90");
    assert_eq!(
        facing_of(Some(&AngleParam::Mode("Face_Player_Start".to_owned())), 1).expect("a mode"),
        Facing::PlayerStart,
        "modes are case-insensitive"
    );
    assert_eq!(
        facing_of(Some(&AngleParam::Mode("random_seeded".to_owned())), 7).expect("seeded"),
        Facing::Seeded(7_u32),
        "the seed rides through"
    );
    let spin = facing_of(Some(&AngleParam::Mode("spin".to_owned())), 1).expect_err("no such mode");
    assert!(
        spin.to_string()
            .contains("face_player_start, face_center, random_seeded"),
        "{spin}"
    );
    assert_eq!(
        facing_of(None, 1).expect("the default"),
        Facing::Fixed(0_i32)
    );
}

#[test]
fn target_areas_checks_sectors_and_splits_the_total() {
    let map = two_rooms();
    let request = resolved(r#"{"thing": 3001, "sectors": [0, 1], "count": 3}"#);
    let (areas, needs) = target_areas(&map, &request).expect("both sectors exist");
    assert_eq!(areas.len(), 2, "two areas");
    assert_eq!(
        needs,
        vec![2_usize, 1_usize],
        "equal areas: the extra copy is the lower index's"
    );
    let missing = resolved(r#"{"thing": 3001, "sectors": [9], "count": 1}"#);
    let lost = target_areas(&map, &missing).expect_err("sector 9 does not exist");
    assert_eq!(lost.code(), "not_found", "an unknown sector is not_found");
    assert!(lost.to_string().contains("sector 9"), "{lost}");
    let each = resolved(r#"{"thing": 3001, "sectors": [0, 1, 0], "per_sector": 2}"#);
    let (twice, twice_needs) = target_areas(&map, &each).expect("duplicates collapse");
    assert_eq!(twice.len(), 2, "the doubled sector 0 fills once");
    assert_eq!(
        twice_needs,
        vec![2_usize, 2_usize],
        "per_sector skips the split"
    );
    check_sectors(&map, &[0, 1]).expect("in range");
}

#[test]
fn place_on_spreads_the_hall_and_notes_the_shortfall() {
    let mut map = two_rooms();
    let definitions = place_definitions();
    let request = resolved(r#"{"thing": "imp", "sectors": [0], "count": 4}"#);
    let step = place_on(&mut map, &request, &definitions).expect("the hall holds four imps");
    assert_eq!(step.indices.len(), 4, "four landed");
    assert_eq!(step.requested, 4, "four were asked");
    assert_eq!(
        step.indices,
        vec![0_usize, 1, 2, 3],
        "the map held no things"
    );
    assert!(step.warnings.is_empty(), "no shortfall");
    for position in &step.positions {
        assert!(
            position[0] >= 28_i32 && position[0] <= 256_i32,
            "{position:?} west-clear and inside"
        );
        assert!(
            position[1] >= 28_i32 && position[1] <= 228_i32,
            "{position:?} off the flat walls"
        );
    }
    let cramped = resolved(r#"{"thing": "imp", "sectors": [0], "count": 200}"#);
    let mut tight = two_rooms();
    let short = place_on(&mut tight, &cramped, &definitions).expect("shortfalls are warnings");
    assert_eq!(short.requested, 200, "the ask is reported honestly");
    assert_eq!(
        short.indices.len(),
        12,
        "twelve grid points clear the walls"
    );
    assert_eq!(
        short.warnings,
        vec!["sector 0 fits only 12 of 200 things".to_owned()],
        "the documented shortfall note"
    );
}

#[test]
fn placed_things_carry_the_family_flags_and_the_facing() {
    let mut map = two_rooms();
    let definitions = place_definitions();
    let request = resolved(
        r#"{"thing": 3001, "sectors": [0], "count": 2, "skills": ["easy", "hard"], "multiplayer_only": true, "ambush": true, "angle": 44}"#,
    );
    let step = place_on(&mut map, &request, &definitions).expect("two imps land");
    assert_eq!(step.indices, vec![0_usize, 1], "both landed as new rows");
    assert_eq!(
        map.things.first().expect("two things").flags,
        1_u32
            .checked_add(4)
            .expect("easy+hard")
            .checked_add(16)
            .expect("+notsingle")
            .checked_add(8)
            .expect("+ambush"),
        "presence, multiplayer and ambush bits"
    );
    assert_eq!(
        map.things.first().expect("two things").kind,
        3001_i32,
        "the resolved type"
    );
    assert!(
        map.things
            .iter()
            .all(|row| snap_angle(row.angle) == row.angle),
        "every angle is a compass point"
    );
    assert_eq!(thing_bit(FlagFamily::Doom, "ambush"), 8_u32, "the Doom bit");
    assert_eq!(
        thing_bit(FlagFamily::Hexen, "ambush"),
        8_u32,
        "the Hexen bit"
    );
    assert_eq!(
        presence_bits(&request, FlagFamily::Doom) & 8_u32,
        8_u32,
        "ambush rides"
    );
}

#[test]
fn the_hexen_only_fields_refuse_a_doom_map_and_missing_starts_refuse() {
    let mut map = two_rooms();
    let definitions = place_definitions();
    let tidied = resolved(r#"{"thing": 3001, "sectors": [0], "count": 1, "tid": 7}"#);
    let refused = place_on(&mut map, &tidied, &definitions).expect_err("Doom has no tid");
    assert_eq!(
        refused.code(),
        "invalid_argument",
        "a Doom-family field refusal"
    );
    assert!(refused.to_string().contains("Doom-format"), "{refused}");
    let facing =
        resolved(r#"{"thing": 3001, "sectors": [0], "count": 1, "angle": "face_player_start"}"#);
    let alone = place_on(&mut map, &facing, &definitions).expect_err("no start on the map");
    assert!(alone.to_string().contains("player 1 start"), "{alone}");
    map.things.push(Thing {
        x: 64_i32,
        y: 128_i32,
        kind: 1_i32,
        flags: 7_u32,
        ..Default::default()
    });
    assert_eq!(
        player_start(&map),
        Some((64_i64, 128_i64)),
        "the start resolves"
    );
    place_on(&mut map, &facing, &definitions).expect("with a start the call runs");
}

#[test]
fn the_dry_fragment_and_the_report_shape_name_the_inspected_map() {
    let fragment = dry_edit(LumpName::parse("MAP01").expect("a valid name"));
    assert_eq!(
        fragment.get("map").expect("map key").as_str(),
        Some("MAP01")
    );
    assert_eq!(
        fragment.get("nodes_invalidated").expect("key").as_bool(),
        Some(false)
    );
    assert_eq!(
        fragment.get("copied_to_working").expect("key").as_bool(),
        Some(false)
    );
}
