//! The sector-special tests: the secret bit and light effect writes
//! per flag family, the untouched-bits promises and the refusals that
//! name the sector the map does not hold.

use crate::named::Named as _;
use serde_json::from_str;

use crate::tools::features::specials::{
    LIGHT_MAX, LIGHT_MIN, LightSpec, MarkSecretParams, SecretSpec, SetLightParams,
    check_light_level, effect_of, mark_secrets, sector_selection, set_lights,
};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::features::LightEffect;

use super::{missing, refuse, two_rooms};

/// A `mark_secret` request parsed from `json`.
fn secret_args(json: &str) -> MarkSecretParams {
    from_str(json).expect("test secret requests parse")
}

/// A `set_light_effect` request parsed from `json`.
fn light_args(json: &str) -> SetLightParams {
    from_str(json).expect("test light requests parse")
}

/// A secret spec over `indices`.
fn secret_spec(indices: &[usize], secret: bool) -> SecretSpec {
    SecretSpec {
        indices: indices.to_vec(),
        secret,
    }
}

/// A light spec over `indices` with an optional level.
fn light_spec(indices: &[usize], effect: LightEffect, light: Option<i32>) -> LightSpec {
    LightSpec {
        indices: indices.to_vec(),
        effect,
        light,
    }
}

/// `two_rooms` with the given sector specials.
fn rooms_special(specials: [i32; 2]) -> Map {
    let mut map = two_rooms();
    for (row, special) in map.sectors.iter_mut().zip(specials) {
        row.special = special;
    }
    map
}

/// The special of sector `index`.
fn special_of(map: &Map, index: usize) -> i32 {
    map.sectors.get(index).expect("the sector exists").special
}

/// The light level of sector `index`.
fn light_of(map: &Map, index: usize) -> i32 {
    map.sectors.get(index).expect("the sector exists").light
}

#[test]
fn the_two_special_tools_read_their_wire_shapes() {
    let marked = secret_args(r#"{"sectors": [1, 0, 1]}"#);
    assert!(marked.secret, "marking is the default");
    assert_eq!(marked.map, None, "the map defaults like the other tools");
    assert_eq!(marked.sectors, vec![1_usize, 0_usize, 1_usize], "as listed");
    let cleared = secret_args(r#"{"sectors": [0], "secret": false, "map": "MAP01"}"#);
    assert!(!cleared.secret, "the flag arrives");
    assert_eq!(cleared.map.as_deref(), Some("MAP01"));
    let lit = light_args(r#"{"sectors": [0], "effect": "flicker", "light": 200}"#);
    assert_eq!(lit.effect, "flicker");
    assert_eq!(lit.light, Some(200_i32), "the level arrives");
    assert_eq!(
        light_args(r#"{"sectors": [0], "effect": "glow"}"#).light,
        None,
        "and stands aside when unasked"
    );
    assert!(
        from_str::<MarkSecretParams>(r#"{"secret": true}"#).is_err(),
        "the sectors are required"
    );
    assert!(
        from_str::<SetLightParams>(r#"{"sectors": [0]}"#).is_err(),
        "the effect is required"
    );
}

#[test]
fn a_selection_sorts_itself_and_refuses_to_be_empty() {
    assert_eq!(
        sector_selection(&[2_usize, 0_usize, 2_usize]).expect("a sorted copy"),
        vec![0_usize, 2_usize],
        "ascending and deduplicated"
    );
    let message = refuse(&sector_selection(&[]).unwrap_err());
    assert!(message.contains("sectors is empty"), "{message}");
}

#[test]
fn light_effects_round_trip_and_refuse_unknown_names() {
    for effect in LightEffect::all() {
        assert_eq!(effect_of(effect.name()).expect("canonical"), effect);
        assert_eq!(
            effect_of(&effect.name().to_ascii_uppercase()).expect("case-insensitive"),
            effect
        );
    }
    let message = refuse(&effect_of("pulsate").unwrap_err());
    assert!(
        message.contains(
            "\"none\", \"flicker\", \"strobe_fast\", \"strobe_slow\", \"glow\", \"random_off\""
        ),
        "the refusal lists every name: {message}"
    );
    assert_eq!(
        LightEffect::names().len(),
        LightEffect::all().len(),
        "one name each"
    );
}

#[test]
fn a_light_level_stays_in_the_vanilla_range() {
    assert!(
        check_light_level(LIGHT_MIN).is_ok(),
        "the floor of the range"
    );
    assert!(check_light_level(LIGHT_MAX).is_ok(), "and the top");
    let low = refuse(&check_light_level(-1_i32).unwrap_err());
    assert!(low.contains("light -1 is out of range"), "{low}");
    let high = refuse(&check_light_level(256_i32).unwrap_err());
    assert!(high.contains("light 256 is out of range"), "{high}");
}

#[test]
fn a_plain_doom_sector_takes_the_vanilla_secret_type() {
    let mut map = rooms_special([0_i32, 1_i32]);
    let step = mark_secrets(&mut map, &secret_spec(&[0_usize], true)).expect("sector 0");
    assert_eq!(step.family, "doom", "the family echoes");
    assert_eq!(step.changed, vec![0_usize], "one row moved");
    assert!(step.warnings.is_empty(), "the vanilla type needs no note");
    assert_eq!(special_of(&map, 0), 9_i32, "the vanilla secret");
    assert_eq!(special_of(&map, 1), 1_i32, "the unlisted sector stands");
    let again = mark_secrets(&mut map, &secret_spec(&[0_usize], true)).expect("marked twice");
    assert!(
        again.changed.is_empty(),
        "the second call is honestly a no-op"
    );
    let cleared = mark_secrets(&mut map, &secret_spec(&[0_usize], false)).expect("cleared again");
    assert_eq!(cleared.changed, vec![0_usize]);
    assert_eq!(
        special_of(&map, 0),
        0_i32,
        "and the vanilla type clears back to plain"
    );
}

#[test]
fn a_boom_doom_sector_gains_and_loses_only_bit_seven() {
    let mut map = rooms_special([32_i32, 4_097_i32]);
    let step =
        mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], true)).expect("both sectors");
    assert_eq!(step.changed, vec![0_usize, 1_usize], "both moved");
    assert!(
        step.warnings.is_empty(),
        "Boom bits are Boom's own business"
    );
    assert_eq!(special_of(&map, 0), 160_i32, "32 | 128");
    assert_eq!(
        special_of(&map, 1),
        4_225_i32,
        "4097 | 128 keeps the generalized field"
    );
    let cleared =
        mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], false)).expect("both cleared");
    assert_eq!(cleared.changed, vec![0_usize, 1_usize]);
    assert_eq!(special_of(&map, 0), 32_i32, "the flag came back off");
    assert_eq!(
        special_of(&map, 1),
        4_097_i32,
        "with every other bit where it was"
    );
}

#[test]
fn a_vanilla_type_marked_secret_carries_a_boom_warning() {
    let mut map = rooms_special([17_i32, 9_i32]);
    let step =
        mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], true)).expect("both sectors");
    assert_eq!(
        step.changed,
        vec![0_usize],
        "the sector already secret by the vanilla type did not move"
    );
    assert_eq!(special_of(&map, 0), 145_i32, "17 | 128");
    assert_eq!(special_of(&map, 1), 9_i32, "and 9 stays 9");
    let note = step.warnings.first().expect("one warning");
    assert!(note.contains("sector 0"), "{note}");
    assert!(
        note.contains("vanilla Doom ignores Boom bits"),
        "the warning says what vanilla Doom does not read: {note}"
    );
    let back = mark_secrets(&mut map, &secret_spec(&[0_usize], false)).expect("cleared");
    assert_eq!(
        back.warnings.len(),
        0_usize,
        "clearing has nothing to warn about"
    );
    assert_eq!(
        special_of(&map, 0),
        17_i32,
        "the flicker is the number again"
    );
}

#[test]
fn a_hexen_map_flags_secret_over_its_own_effect() {
    let mut map = rooms_special([81_i32, 1_i32]);
    map.format = MapFormat::Hexen;
    let step = mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], true))
        .expect("both sectors of a Hexen map");
    assert_eq!(step.family, "hexen", "binary Hexen is its own family");
    assert!(step.warnings.is_empty(), "the Hexen bit never needs a note");
    assert_eq!(special_of(&map, 0), 1_105_i32, "81 | 1024 keeps the effect");
    assert_eq!(special_of(&map, 1), 1_025_i32, "1 | 1024 too");
    let cleared = mark_secrets(&mut map, &secret_spec(&[1_usize], false)).expect("one sector back");
    assert_eq!(cleared.changed, vec![1_usize]);
    assert_eq!(special_of(&map, 1), 1_i32, "bit 10 came off alone");
    assert_eq!(
        special_of(&map, 0),
        1_105_i32,
        "the unlisted sector stands untouched"
    );
}

#[test]
fn a_zdoom_udmf_map_shares_the_hexen_secret_bit() {
    let mut map = rooms_special([0_i32, 1_i32]);
    map.format = MapFormat::Udmf;
    map.udmf_namespace = Some("zdoom".to_owned());
    let step = mark_secrets(&mut map, &secret_spec(&[0_usize], true)).expect("sector 0");
    assert_eq!(
        step.family, "hexen",
        "a zdoom namespace reads as Hexen family"
    );
    assert_eq!(special_of(&map, 0), 1_024_i32, "the bit binary Hexen uses");
}

#[test]
fn a_doom_namespace_udmf_map_keeps_the_boom_bit() {
    let mut map = rooms_special([0_i32, 1_i32]);
    map.format = MapFormat::Udmf;
    map.udmf_namespace = Some("doom".to_owned());
    let step =
        mark_secrets(&mut map, &secret_spec(&[0_usize, 1_usize], true)).expect("both sectors");
    assert_eq!(
        step.family, "doom",
        "the doom namespace is Doom family even in UDMF"
    );
    assert_eq!(
        special_of(&map, 0),
        9_i32,
        "the plain sector took the vanilla type"
    );
    assert_eq!(special_of(&map, 1), 129_i32, "the basic 1 took bit 7");
    assert_eq!(
        step.warnings.len(),
        1_usize,
        "one note: {:?}",
        step.warnings
    );
}

#[test]
fn the_secret_step_names_the_sector_the_map_does_not_hold() {
    let mut map = rooms_special([0_i32, 0_i32]);
    let message =
        missing(&mark_secrets(&mut map, &secret_spec(&[1_usize, 99_usize], true)).unwrap_err());
    assert!(message.contains("sector 99 does not exist"), "{message}");
    assert_eq!(special_of(&map, 0), 0_i32, "a refusal wrote nothing");
    assert_eq!(
        special_of(&map, 1),
        0_i32,
        "not even the sectors it could have reached"
    );
}

#[test]
fn a_doom_light_effect_writes_the_type_and_keeps_the_generalized_bits() {
    let wanted = [
        (LightEffect::Flicker, 17_i32),
        (LightEffect::StrobeFast, 2_i32),
        (LightEffect::StrobeSlow, 3_i32),
        (LightEffect::Glow, 8_i32),
        (LightEffect::RandomOff, 1_i32),
        (LightEffect::None, 0_i32),
    ];
    for (effect, basic) in wanted {
        let mut map = rooms_special([4_105_i32, 9_i32]);
        let step = set_lights(&mut map, &light_spec(&[0_usize], effect, None))
            .expect("sector 0 takes the effect");
        assert_eq!(step.effect, effect, "{effect:?} echoes");
        assert_eq!(step.family, "doom", "{effect:?} reads its family");
        assert_eq!(step.special, basic, "{effect:?} is the vanilla type");
        assert_eq!(step.changed, vec![0_usize], "{effect:?} moved one row");
        assert_eq!(
            special_of(&map, 0),
            4_096_i32 | basic,
            "{effect:?} landed below 32, the generalized bit above it survived"
        );
        assert_eq!(light_of(&map, 0), 176_i32, "an unasked level stands");
        assert_eq!(special_of(&map, 1), 9_i32, "the unlisted sector stands");
    }
}

#[test]
fn a_hexen_light_effect_is_the_doom_number_sixty_four_higher() {
    let wanted = [
        (LightEffect::Flicker, 81_i32),
        (LightEffect::StrobeFast, 66_i32),
        (LightEffect::StrobeSlow, 67_i32),
        (LightEffect::Glow, 72_i32),
        (LightEffect::RandomOff, 65_i32),
        (LightEffect::None, 0_i32),
    ];
    for (effect, basic) in wanted {
        let mut map = rooms_special([1_024_i32 | 256_i32, 0_i32]);
        map.format = MapFormat::Hexen;
        let step = set_lights(&mut map, &light_spec(&[0_usize], effect, Some(96_i32)))
            .expect("sector 0 takes the effect");
        assert_eq!(step.effect, effect, "{effect:?} echoes");
        assert_eq!(step.special, basic, "{effect:?} is the Doom number plus 64");
        assert_eq!(
            special_of(&map, 0),
            1_024_i32 | 256_i32 | basic,
            "{effect:?} landed below 256, the secret bit and the flag above it survived"
        );
        assert_eq!(
            light_of(&map, 0),
            96_i32,
            "and the asked-for level rode along"
        );
    }
}

#[test]
fn a_light_repaint_that_changes_nothing_reports_nothing() {
    let mut map = rooms_special([17_i32, 17_i32]);
    let first = set_lights(
        &mut map,
        &light_spec(&[0_usize, 1_usize], LightEffect::Flicker, None),
    )
    .expect("both already flicker");
    assert!(first.changed.is_empty(), "the effect was already there");
    assert_eq!(
        set_lights(
            &mut map,
            &light_spec(&[0_usize], LightEffect::Flicker, Some(64_i32))
        )
        .expect("a level alone still counts")
        .changed,
        vec![0_usize],
        "the light moved even though the special did not"
    );
    assert_eq!(light_of(&map, 0), 64_i32);
    assert_eq!(light_of(&map, 1), 176_i32, "the unlisted level stands");
}

#[test]
fn the_light_step_names_the_sector_the_map_does_not_hold() {
    let mut map = rooms_special([0_i32, 0_i32]);
    let message = missing(
        &set_lights(
            &mut map,
            &light_spec(&[0_usize, 42_usize], LightEffect::Glow, None),
        )
        .unwrap_err(),
    );
    assert!(message.contains("sector 42 does not exist"), "{message}");
    assert_eq!(special_of(&map, 0), 0_i32, "a refusal wrote nothing");
}
