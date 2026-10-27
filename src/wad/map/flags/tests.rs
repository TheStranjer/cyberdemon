//! The flag tables' unit tests: every name round-trips through its
//! family's table, an unknown name is refused with the valid list, the
//! named constants are the bits the tables carry and
//! `blocks_players` answers for both families.

use super::{
    ACTIVATION_MASK, BLOCK_MONSTERS, BLOCKING, DOOM_BLOCK_LAND_MONSTERS, DOOM_BLOCK_PLAYERS,
    FlagFamily, FlagKind, HEXEN_BLOCK_BITS, HEXEN_BLOCK_EVERYTHING, HEXEN_BLOCK_PLAYERS,
    MONSTER_ACTIVATE, PASS_USE, PLAYER_CROSS_BITS, PLAYER_USE_BITS, REPEAT_SPECIAL, SECRET,
    SHARED_LINEDEF_BITS, Skill, THING_AMBUSH, THING_DOOM_NOT_SINGLE, THING_EASY,
    THING_HEXEN_SINGLE, THING_HEXEN_STANDING, THING_SKILL_BITS, TWO_SIDED, blocks_players,
    check_flag, from_names, names, to_names,
};
use crate::error::CyberError;

/// The table names, for building full-table requests.
fn all_names(kind: FlagKind, family: FlagFamily) -> Vec<String> {
    names(kind, family)
        .iter()
        .map(|entry| entry.0.to_owned())
        .collect()
}

/// The `InvalidArgument` message `from_names` gives for `wanted`.
fn error_for(wanted: &str, kind: FlagKind, family: FlagFamily) -> String {
    let request = [wanted.to_owned()];
    match from_names(&request, kind, family) {
        Err(CyberError::InvalidArgument(message)) => message,
        Err(other) => panic!("expected InvalidArgument, got {other:?}"),
        Ok(bits) => panic!("expected a refusal, got bits {bits:#x}"),
    }
}

/// The `InvalidArgument` message `check_flag` gives for `wanted`.
fn refused_by_check(wanted: &str, kind: FlagKind, family: FlagFamily) -> String {
    match check_flag(kind, family, wanted) {
        Err(CyberError::InvalidArgument(message)) => message,
        Ok(()) => panic!("expected a refusal of '{wanted}'"),
        Err(other) => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn check_flag_accepts_every_table_name_and_refuses_the_rest() {
    for (kind, family) in [
        (FlagKind::Linedef, FlagFamily::Doom),
        (FlagKind::Linedef, FlagFamily::Hexen),
        (FlagKind::Thing, FlagFamily::Doom),
        (FlagKind::Thing, FlagFamily::Hexen),
    ] {
        for entry in names(kind, family) {
            assert!(
                check_flag(kind, family, entry.0).is_ok(),
                "'{}' is in the table",
                entry.0
            );
        }
    }
    let refused = refused_by_check("twosided", FlagKind::Thing, FlagFamily::Doom);
    assert!(
        refused.contains("unknown thing flag 'twosided'") && refused.contains("valid names are"),
        "the refusal names the bad flag and the valid list: {refused}"
    );
    assert_eq!(
        refused,
        error_for("twosided", FlagKind::Thing, FlagFamily::Doom),
        "the same wording the bit-setter uses"
    );
    assert!(
        refused_by_check("nope", FlagKind::Linedef, FlagFamily::Hexen)
            .contains("unknown linedef flag 'nope' for the hexen family"),
        "the other tables answer the same way"
    );
}

#[test]
fn the_doom_linedef_table_names_the_twelve_family_bits() {
    let bits = from_names(
        &all_names(FlagKind::Linedef, FlagFamily::Doom),
        FlagKind::Linedef,
        FlagFamily::Doom,
    )
    .expect("every name is known");
    assert_eq!(bits, 0x33FF, "twelve bits, 1 through 8192");
    assert_eq!(
        to_names(bits, FlagKind::Linedef, FlagFamily::Doom).len(),
        12,
        "and all twelve read back"
    );
    assert_eq!(
        to_names(3, FlagKind::Linedef, FlagFamily::Doom),
        vec!["blocking", "blockmonsters"],
        "in table order"
    );
    assert!(
        to_names(0, FlagKind::Linedef, FlagFamily::Doom).is_empty(),
        "no bits, no names"
    );
}

#[test]
fn the_mbf21_bits_round_trip_through_the_doom_table() {
    for (name, bit) in [
        ("blocklandmonsters", DOOM_BLOCK_LAND_MONSTERS),
        ("blockplayers", DOOM_BLOCK_PLAYERS),
    ] {
        let request = [name.to_owned()];
        let bits = from_names(&request, FlagKind::Linedef, FlagFamily::Doom).expect("a Doom name");
        assert_eq!(bits, bit, "'{name}' names its bit");
        assert_eq!(
            bit & SHARED_LINEDEF_BITS,
            0_u32,
            "'{name}' sits above the mask the two families share"
        );
        assert_eq!(
            to_names(bits, FlagKind::Linedef, FlagFamily::Doom),
            vec![name],
            "and reads back as itself"
        );
    }
    let both = from_names(
        &[
            "blocklandmonsters".to_owned(),
            "blockplayers".to_owned(),
            "blocking".to_owned(),
        ],
        FlagKind::Linedef,
        FlagFamily::Doom,
    )
    .expect("three known names");
    assert_eq!(both, 1_u32 | 0x1000 | 0x2000, "the bits combine");
    assert_eq!(
        to_names(both, FlagKind::Linedef, FlagFamily::Doom),
        vec!["blocking", "blocklandmonsters", "blockplayers"],
        "in table order"
    );
}

#[test]
fn the_mbf21_bits_are_the_doom_family_s_own() {
    let message = error_for("blocklandmonsters", FlagKind::Linedef, FlagFamily::Hexen);
    assert!(
        message.contains("unknown linedef flag 'blocklandmonsters'"),
        "the Hexen table does not name it: {message}"
    );
    assert_eq!(
        from_names(
            &["blockplayers".to_owned()],
            FlagKind::Linedef,
            FlagFamily::Hexen
        )
        .expect("a Hexen name"),
        HEXEN_BLOCK_PLAYERS,
        "and the same word names bit 14 there, not bit 13"
    );
    assert_eq!(
        to_names(DOOM_BLOCK_PLAYERS, FlagKind::Linedef, FlagFamily::Hexen),
        vec!["playercross", "monsteractivate"],
        "bit 13 is the Hexen monster bit, not a player block"
    );
    assert_eq!(
        to_names(
            DOOM_BLOCK_LAND_MONSTERS,
            FlagKind::Linedef,
            FlagFamily::Hexen
        ),
        vec!["playerpush"],
        "and bit 12 is part of its activation field"
    );
}

#[test]
fn the_hexen_linedef_table_adds_the_field_and_extra_bits() {
    let shared = [
        "blocking",
        "blockmonsters",
        "twosided",
        "dontpegtop",
        "dontpegbottom",
        "secret",
        "blocksound",
        "dontdraw",
        "mapped",
    ];
    for name in shared {
        let plain = from_names(&[name.to_owned()], FlagKind::Linedef, FlagFamily::Doom)
            .expect("a shared Doom name");
        let hexen = from_names(&[name.to_owned()], FlagKind::Linedef, FlagFamily::Hexen)
            .expect("the same name in the Hexen table");
        assert_eq!(plain, hexen, "'{name}' keeps its bit");
    }
    for (name, bits) in [
        ("repeatspecial", 0x200_u32),
        ("monsteractivate", MONSTER_ACTIVATE),
        ("blockplayers", 0x4000_u32),
        ("blockeverything", 0x8000_u32),
    ] {
        let request = [name.to_owned()];
        let found =
            from_names(&request, FlagKind::Linedef, FlagFamily::Hexen).expect("a Hexen name");
        assert_eq!(found, bits, "'{name}' names its bit");
    }
}

#[test]
fn activation_names_write_their_field_value() {
    for (name, value) in [
        ("playercross", 0_u32),
        ("playeruse", 1),
        ("monstercross", 2),
        ("impact", 3),
        ("playerpush", 4),
        ("missilecross", 5),
    ] {
        let request = [name.to_owned(), "blocking".to_owned()];
        let bits =
            from_names(&request, FlagKind::Linedef, FlagFamily::Hexen).expect("a known name");
        let expected = value
            .checked_shl(10)
            .unwrap_or(0_u32)
            .checked_add(1_u32)
            .expect("small");
        assert_eq!(bits, expected, "'{name}' writes field {value}");
        assert_eq!(
            to_names(bits, FlagKind::Linedef, FlagFamily::Hexen)[0],
            "blocking"
        );
        assert_eq!(
            to_names(bits, FlagKind::Linedef, FlagFamily::Hexen).last(),
            Some(&name),
            "and names the activation last of the set"
        );
    }
}

#[test]
fn a_later_activation_name_replaces_an_earlier_one() {
    let replaced = from_names(
        &[
            "playeruse".to_owned(),
            "monstercross".to_owned(),
            "repeatspecial".to_owned(),
        ],
        FlagKind::Linedef,
        FlagFamily::Hexen,
    )
    .expect("three known names");
    assert_eq!(
        replaced,
        0x200_u32 | 2_u32.checked_shl(10).unwrap_or(0),
        "the later activation name wins, bits combine"
    );
    assert_eq!(
        (replaced & ACTIVATION_MASK)
            .checked_shr(10)
            .unwrap_or(0_u32),
        2_u32,
        "the field holds monstercross"
    );
    assert_eq!(
        from_names(
            &["playercross".to_owned()],
            FlagKind::Linedef,
            FlagFamily::Hexen
        )
        .expect("the zero field is nameable"),
        0_u32,
        "playercross is the all-clear field"
    );
    assert_eq!(
        to_names(0, FlagKind::Linedef, FlagFamily::Hexen),
        vec!["playercross"],
        "and an all-clear bitset reads as playercross"
    );
}

#[test]
fn the_thing_tables_name_their_bits_exactly() {
    let doom_bits = from_names(
        &all_names(FlagKind::Thing, FlagFamily::Doom),
        FlagKind::Thing,
        FlagFamily::Doom,
    )
    .expect("eight known names");
    assert_eq!(doom_bits, 0xFF, "the eight Doom presence bits");
    assert_eq!(
        to_names(
            0x8_u32.checked_add(0x80).expect("ambush+friend"),
            FlagKind::Thing,
            FlagFamily::Doom
        ),
        vec!["ambush", "friend"],
        "in table order"
    );
    let hexen_bits = from_names(
        &all_names(FlagKind::Thing, FlagFamily::Hexen),
        FlagKind::Thing,
        FlagFamily::Hexen,
    )
    .expect("fifteen known names");
    assert_eq!(hexen_bits, 0x7FFF, "fifteen consecutive Hexen bits");
    assert_eq!(
        to_names(hexen_bits, FlagKind::Thing, FlagFamily::Hexen).len(),
        15,
        "and all fifteen read back"
    );
    assert_eq!(
        to_names(0x4000, FlagKind::Thing, FlagFamily::Hexen),
        vec!["standing"],
        "the top bit alone"
    );
}

#[test]
fn unknown_names_are_refused_with_the_valid_list() {
    let message = error_for("twosided", FlagKind::Thing, FlagFamily::Doom);
    assert!(
        message.contains("unknown thing flag 'twosided'")
            && message.contains("doom family")
            && message.contains("easy, medium, hard, ambush")
            && message.contains("friend"),
        "the message names the element, family and valid names: {message}"
    );
    let activation = error_for("playeruse", FlagKind::Linedef, FlagFamily::Doom);
    assert!(
        activation.contains("valid names are blocking") && activation.contains("passuse"),
        "the Doom table is listed: {activation}"
    );
    assert!(
        error_for("nope", FlagKind::Thing, FlagFamily::Hexen).contains("'nope'"),
        "the bad name is quoted"
    );
}

#[test]
fn each_skill_names_its_bit_in_both_thing_tables() {
    for (skill, bit) in [
        (Skill::Easy, 1_u32),
        (Skill::Medium, 2_u32),
        (Skill::Hard, 4_u32),
    ] {
        assert_eq!(skill.bit(FlagFamily::Doom), bit, "the Doom table");
        assert_eq!(skill.bit(FlagFamily::Hexen), bit, "and the Hexen one");
        assert_eq!(Skill::parse(skill.as_str()), Some(skill), "and its name");
    }
    assert_eq!(Skill::ALL.len(), 3, "all three, in table order");
    assert_eq!(Skill::parse("nightmare"), None, "and no fourth skill");
}

#[test]
fn the_namespace_decides_the_family() {
    for namespace in ["doom", "heretic", "strife", "Doom", "STRIfe"] {
        assert_eq!(
            FlagFamily::from_udmf_namespace(namespace),
            FlagFamily::Doom,
            "'{namespace}' is a Doom-flavoured namespace"
        );
    }
    for namespace in [
        "hexen", "zdoom", "gzdoom", "eternity", "vavoom", "glfezzot", "",
    ] {
        assert_eq!(
            FlagFamily::from_udmf_namespace(namespace),
            FlagFamily::Hexen,
            "'{namespace}' is not a Doom-flavoured namespace"
        );
    }
    assert_eq!(FlagFamily::Doom.as_str(), "doom");
    assert_eq!(FlagFamily::Hexen.as_str(), "hexen");
}

#[test]
fn the_named_linedef_consts_are_the_bits_the_tables_name() {
    let doom_only = ["passuse", "blocklandmonsters", "blockplayers"];
    let doom_shared: Vec<String> = names(FlagKind::Linedef, FlagFamily::Doom)
        .iter()
        .map(|entry| entry.0.to_owned())
        .filter(|name| !doom_only.contains(&name.as_str()))
        .collect();
    let shared = from_names(&doom_shared, FlagKind::Linedef, FlagFamily::Doom)
        .expect("the nine shared names");
    assert_eq!(
        shared, SHARED_LINEDEF_BITS,
        "the mask covers blocking..mapped"
    );
    assert_eq!(
        from_names(&["passuse".to_owned()], FlagKind::Linedef, FlagFamily::Doom).expect("passuse"),
        PASS_USE,
        "the Doom 512 bit"
    );
    assert_eq!(
        from_names(
            &["repeatspecial".to_owned()],
            FlagKind::Linedef,
            FlagFamily::Hexen
        )
        .expect("repeatspecial"),
        REPEAT_SPECIAL,
        "the Hexen 512 bit"
    );
    assert_eq!(
        from_names(
            &["playeruse".to_owned()],
            FlagKind::Linedef,
            FlagFamily::Hexen
        )
        .expect("the use activation"),
        PLAYER_USE_BITS,
        "the field value at its shifted position"
    );
    assert_eq!(
        from_names(
            &["playercross".to_owned()],
            FlagKind::Linedef,
            FlagFamily::Hexen
        )
        .expect("the cross activation"),
        PLAYER_CROSS_BITS,
        "the all-clear field is zero bits"
    );
    assert_eq!(
        HEXEN_BLOCK_BITS,
        HEXEN_BLOCK_PLAYERS | HEXEN_BLOCK_EVERYTHING,
        "the two block bits the Doom family cannot carry"
    );
}

#[test]
fn the_named_thing_consts_are_the_bits_the_tables_name() {
    assert_eq!(
        to_names(THING_DOOM_NOT_SINGLE, FlagKind::Thing, FlagFamily::Doom),
        vec!["notsingle"],
        "the presence consts name their table entries"
    );
    assert_eq!(
        to_names(THING_HEXEN_STANDING, FlagKind::Thing, FlagFamily::Hexen),
        vec!["standing"],
    );
    assert_eq!(
        to_names(THING_SKILL_BITS, FlagKind::Thing, FlagFamily::Doom),
        vec!["easy", "medium", "hard"],
        "the skill trio stands at every skill"
    );
    assert_eq!(
        to_names(THING_AMBUSH, FlagKind::Thing, FlagFamily::Hexen),
        vec!["ambush"],
    );
    assert_eq!(
        to_names(THING_EASY, FlagKind::Thing, FlagFamily::Doom),
        vec!["easy"],
    );
    assert_eq!(
        to_names(THING_HEXEN_SINGLE, FlagKind::Thing, FlagFamily::Hexen),
        vec!["single"],
    );
}

#[test]
fn blocks_players_answers_for_both_families() {
    assert!(
        blocks_players(1_u32, FlagFamily::Doom),
        "the raw blocking bit"
    );
    assert!(
        blocks_players(BLOCKING, FlagFamily::Hexen),
        "the same bit in the Hexen family"
    );
    assert!(
        !blocks_players(BLOCK_MONSTERS, FlagFamily::Doom),
        "a line that only holds monsters lets the player through"
    );
    assert!(
        !blocks_players(SECRET | TWO_SIDED, FlagFamily::Hexen),
        "flag bits with no blocking meaning pass"
    );
    assert!(
        blocks_players(HEXEN_BLOCK_PLAYERS, FlagFamily::Hexen),
        "blockplayers keeps the player out"
    );
    assert!(
        blocks_players(DOOM_BLOCK_PLAYERS, FlagFamily::Doom),
        "and MBF21's Doom-family blockplayers does the same"
    );
    assert!(
        !blocks_players(DOOM_BLOCK_LAND_MONSTERS, FlagFamily::Doom),
        "while blocklandmonsters only holds walkers"
    );
    assert!(
        !blocks_players(DOOM_BLOCK_PLAYERS, FlagFamily::Hexen),
        "and that Doom bit is monsteractivate to the Hexen family"
    );
    assert!(
        blocks_players(HEXEN_BLOCK_EVERYTHING, FlagFamily::Hexen),
        "and blockeverything more so"
    );
    assert!(
        !blocks_players(
            HEXEN_BLOCK_PLAYERS | HEXEN_BLOCK_EVERYTHING,
            FlagFamily::Doom
        ),
        "which the Doom family's tables do not define"
    );
    assert!(
        blocks_players(TWO_SIDED | HEXEN_BLOCK_PLAYERS, FlagFamily::Hexen),
        "alongside the two-sided bit of a glass pane"
    );
    assert!(
        !blocks_players(0_u32, FlagFamily::Doom) && !blocks_players(0_u32, FlagFamily::Hexen),
        "no bits, nothing blocked"
    );
}
