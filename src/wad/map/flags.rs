//! Named bit tables for the `Doom` and `Hexen` flag families.
//!
//! The `flags` columns of [`Linedef`] and [`Thing`] hold raw bitsets whose
//! meaning depends on the map's format family, which [`Map::flag_family`]
//! reports. This module is the name↔bit side of that storage: [`names`]
//! lists the family's table as `(name, value)` pairs in bit order,
//! [`to_names`] splits a bitset into its names and [`from_names`] folds
//! names back into a bitset, refusing unknown ones with an
//! [`CyberError::InvalidArgument`] that lists the valid names.
//!
//! The difficulty skills a map can be read or written for live here too as
//! [`Skill`]: both thing tables name `easy`, `medium` and `hard` with the
//! same bits, so the skill whose presence bit a caller asks for is a
//! flag-family idea and one type serves the writers and the readers.
//!
//! The `Hexen` linedef table carries the trigger activation as a 3-bit
//! field (`flags & 0x1C00`, the value shifted to bit 10):
//! the six activation names hold the *field value* `0..=5` in their table
//! entry, not a bit. [`to_names`] reports the name whose field value the
//! bits carry (so `playercross` names the all-clear field), and
//! [`from_names`] writes the field: a later activation name replaces an
//! earlier one. Every other entry, in every table, is a single bit.
//!
//! This module is also the one home of the flag-bit names: every linedef
//! and thing bit the tables carry is a named `pub(crate)` constant here
//! (`BLOCKING`, `TWO_SIDED`, `THING_HEXEN_SINGLE`, the activation
//! field values, the masks), the tables are built from those constants,
//! and the rest of the crate spells its flag maths with them rather than
//! re-literalising a value. `blocks_players` is the one answer to the
//! one question every placement, walk and stuck test asks of a line.
//!
//! [`Linedef`]: crate::wad::map::Linedef
//! [`Thing`]: crate::wad::map::Thing
//! [`Map::flag_family`]: crate::wad::map::Map::flag_family

use crate::error::CyberError;

/// Mask covering the `Hexen` linedef activation field.
pub(crate) const ACTIVATION_MASK: u32 = 0x1C00;
/// Shift from an activation field value to its position in the flag bits.
pub(super) const ACTIVATION_SHIFT: u32 = 10_u32;
/// The `Hexen` activation field value `playercross`: a line worked by
/// walking across it, the field's all-clear zero.
pub(crate) const PLAYER_CROSS: u32 = 0_u32;
/// The `Hexen` activation field value `playeruse`: a line worked by
/// pressing use against its front side.
pub(crate) const PLAYER_USE: u32 = 1_u32;
/// The `Hexen` activation field value `monstercross`.
pub(crate) const MONSTER_CROSS: u32 = 2_u32;
/// The `Hexen` activation field value `impact`: worked by a hit.
pub(crate) const IMPACT: u32 = 3_u32;
/// The `Hexen` activation field value `playerpush`: worked by a crush.
pub(crate) const PLAYER_PUSH: u32 = 4_u32;
/// The `Hexen` activation field value `missilecross`.
pub(crate) const MISSILE_CROSS: u32 = 5_u32;
/// The `Hexen` `playercross` field value at its position in the flag bits.
pub(crate) const PLAYER_CROSS_BITS: u32 = 0_u32;
/// The `Hexen` `playeruse` field value at its position in the flag bits.
pub(crate) const PLAYER_USE_BITS: u32 = match PLAYER_USE.checked_shl(ACTIVATION_SHIFT) {
    Some(bits) => bits,
    None => 0_u32,
};
/// The `Hexen` linedef bit letting monsters trigger special lines.
pub(crate) const MONSTER_ACTIVATE: u32 = 0x2000;

/// The `Doom` and `Hexen` linedef `blocking` bit: engines stop motion at
/// the line, in both flag families.
pub(crate) const BLOCKING: u32 = 1_u32;
/// The shared `blockmonsters` linedef bit: monsters cannot cross, in both
/// families.
pub(crate) const BLOCK_MONSTERS: u32 = 2_u32;
/// The shared `twosided` linedef bit: the line has a sector behind it.
pub(crate) const TWO_SIDED: u32 = 4_u32;
/// The shared `dontpegtop` linedef bit: the upper texture is pinned to
/// the ceiling instead of scrolling with it.
pub(crate) const DONT_PEG_TOP: u32 = 8_u32;
/// The shared `dontpegbottom` linedef bit: the lower texture is pinned to
/// the floor instead of scrolling with it.
pub(crate) const DONT_PEG_BOTTOM: u32 = 16_u32;
/// The shared `secret` linedef bit: the automap keeps what the line
/// guards hidden.
pub(crate) const SECRET: u32 = 32_u32;
/// The shared `blocksound` linedef bit: sound does not pass the line.
pub(crate) const BLOCK_SOUND: u32 = 64_u32;
/// The shared `dontdraw` linedef bit: the automap must not draw the line.
pub(crate) const DONT_DRAW: u32 = 128_u32;
/// The shared `mapped` linedef bit: the automap reveals the line from
/// the start.
pub(crate) const MAPPED: u32 = 256_u32;
/// The `Doom`-family linedef bit 512: `passuse`, a one-shot special whose
/// line stays usable after it fires.
pub(crate) const PASS_USE: u32 = 0x200;
/// The `Hexen`-family linedef bit 512: `repeatspecial`, the line's action
/// keeps working after every firing.
pub(crate) const REPEAT_SPECIAL: u32 = 0x200;
/// The `Hexen`-family linedef bit 0x4000: `blockplayers`, which only the
/// player cannot cross.
pub(crate) const HEXEN_BLOCK_PLAYERS: u32 = 0x4000;
/// The `Hexen`-family linedef bit 0x8000: `blockeverything`, which
/// nothing crosses.
pub(crate) const HEXEN_BLOCK_EVERYTHING: u32 = 0x8000;
/// The linedef flag bits the two families share: `blocking` through
/// `mapped`, bits 0 to 8.
pub(crate) const SHARED_LINEDEF_BITS: u32 = 0x1FF;
/// The two `Hexen`-family block bits no `Doom`-family line holds:
/// `blockplayers` and `blockeverything`.
pub(crate) const HEXEN_BLOCK_BITS: u32 = HEXEN_BLOCK_PLAYERS | HEXEN_BLOCK_EVERYTHING;

/// The `easy` skill bit of both thing flag tables.
pub(crate) const THING_EASY: u32 = 1_u32;
/// The `medium` skill bit of both thing flag tables.
pub(crate) const THING_MEDIUM: u32 = 2_u32;
/// The `hard` skill bit of both thing flag tables.
pub(crate) const THING_HARD: u32 = 4_u32;
/// The three skill bits together: a thing that stands at every skill.
pub(crate) const THING_SKILL_BITS: u32 = THING_EASY | THING_MEDIUM | THING_HARD;
/// The `ambush` bit of both thing flag tables.
pub(crate) const THING_AMBUSH: u32 = 8_u32;
/// The thing bits the two families share: the three skills and `ambush`.
pub(crate) const THING_SHARED_BITS: u32 = THING_SKILL_BITS | THING_AMBUSH;
/// The `Doom` thing bit 16: `notsingle`, never spawned in single player.
pub(crate) const THING_DOOM_NOT_SINGLE: u32 = 16_u32;
/// The `Doom` thing bit 32: `notdm`.
pub(crate) const THING_DOOM_NOT_DM: u32 = 32_u32;
/// The `Doom` thing bit 64: `notcoop`.
pub(crate) const THING_DOOM_NOT_COOP: u32 = 64_u32;
/// The `Doom` thing bit 128: `friend`.
pub(crate) const THING_DOOM_FRIEND: u32 = 128_u32;
/// The `Hexen` thing bit 16: `dormant`.
pub(crate) const THING_HEXEN_DORMANT: u32 = 16_u32;
/// The `Hexen` thing bit 32: `class1`.
pub(crate) const THING_HEXEN_CLASS1: u32 = 32_u32;
/// The `Hexen` thing bit 64: `class2`.
pub(crate) const THING_HEXEN_CLASS2: u32 = 64_u32;
/// The `Hexen` thing bit 128: `class3`.
pub(crate) const THING_HEXEN_CLASS3: u32 = 128_u32;
/// The three `Hexen` class bits together: a thing every class may meet.
pub(crate) const THING_HEXEN_CLASSES: u32 =
    THING_HEXEN_CLASS1 | THING_HEXEN_CLASS2 | THING_HEXEN_CLASS3;
/// The `Hexen` thing bit 256: `single`, present in single player.
pub(crate) const THING_HEXEN_SINGLE: u32 = 256_u32;
/// The `Hexen` thing bit 512: `coop`.
pub(crate) const THING_HEXEN_COOP: u32 = 512_u32;
/// The `Hexen` thing bit 1024: `dm`.
pub(crate) const THING_HEXEN_DM: u32 = 1024_u32;
/// The `Hexen` thing bit 2048: `translucent`.
pub(crate) const THING_HEXEN_TRANSLUCENT: u32 = 2048_u32;
/// The `Hexen` thing bit 4096: `invisible`.
pub(crate) const THING_HEXEN_INVISIBLE: u32 = 4096_u32;
/// The `Hexen` thing bit 8192: `friend`.
pub(crate) const THING_HEXEN_FRIEND: u32 = 8192_u32;
/// The `Hexen` thing bit 0x4000: `standing`.
pub(crate) const THING_HEXEN_STANDING: u32 = 0x4000;

/// Whether `flags` stop a player from crossing the line: the `blocking`
/// bit every family shares, plus the `Hexen` family's `blockplayers` and
/// `blockeverything` bits. The `blockmonsters` bit is deliberately not
/// among them: a player walks past a line that only holds monsters, and
/// the two `Hexen` bits read as nothing on a `Doom`-family line.
#[must_use]
pub(crate) const fn blocks_players(flags: u32, family: FlagFamily) -> bool {
    let shared = flags & BLOCKING == BLOCKING;
    let hexen_only = matches!(family, FlagFamily::Hexen)
        && (flags & HEXEN_BLOCK_PLAYERS == HEXEN_BLOCK_PLAYERS
            || flags & HEXEN_BLOCK_EVERYTHING == HEXEN_BLOCK_EVERYTHING);
    shared || hexen_only
}

/// The flag vocabulary a map's format stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagFamily {
    /// Binary `Doom` maps and the `UDMF` namespaces `doom`, `heretic`
    /// and `strife`.
    Doom,
    /// Binary `Hexen` maps and every other `UDMF` namespace (`hexen`,
    /// `zdoom`, `gzdoom`, `eternity`, `vavoom`, ...).
    Hexen,
}

impl FlagFamily {
    /// The family the `UDMF` `namespace` declares, case-insensitively:
    /// `doom`, `heretic` and `strife` use the `Doom` tables, everything
    /// else the `Hexen` ones.
    #[must_use]
    #[inline]
    pub fn from_udmf_namespace(namespace: &str) -> Self {
        if is_doomish(namespace) {
            return Self::Doom;
        }
        Self::Hexen
    }

    /// The snake-case name of this family.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Doom => "doom",
            Self::Hexen => "hexen",
        }
    }
}

/// A difficulty skill a map is played, planned or read at.
///
/// Every [`Thing`](crate::wad::map::Thing) that spawns carries one bit per
/// skill; which of the three a caller asks about decides what the map holds
/// at that difficulty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skill {
    /// The easy skill bit.
    Easy,
    /// The medium skill bit.
    Medium,
    /// The hard skill bit.
    Hard,
}

impl Skill {
    /// Every skill, in flag-table order.
    pub const ALL: [Self; 3] = [Self::Easy, Self::Medium, Self::Hard];

    /// The name of this skill's bit in both families' thing flag tables.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
        }
    }

    /// The skill the wire name `text` spells, case-insensitively.
    #[must_use]
    #[inline]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|skill| skill.as_str().eq_ignore_ascii_case(text))
    }

    /// The bit this skill holds in `family`'s thing flag table, `0` when
    /// that table carries no such name.
    #[must_use]
    #[inline]
    pub fn bit(self, family: FlagFamily) -> u32 {
        names(FlagKind::Thing, family)
            .iter()
            .find(|entry| entry.0 == self.as_str())
            .map_or(0_u32, |entry| entry.1)
    }
}

/// The element whose `flags` column is being named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagKind {
    /// Flags of a [`crate::wad::map::Linedef`].
    Linedef,
    /// Flags of a [`crate::wad::map::Thing`].
    Thing,
}

impl FlagKind {
    /// The lowercase name of this element kind.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linedef => "linedef",
            Self::Thing => "thing",
        }
    }
}

/// `Linedef` flags in the `Doom` family: the ten vanilla bits.
const LINEDEF_DOOM: &[(&str, u32)] = &[
    ("blocking", BLOCKING),
    ("blockmonsters", BLOCK_MONSTERS),
    ("twosided", TWO_SIDED),
    ("dontpegtop", DONT_PEG_TOP),
    ("dontpegbottom", DONT_PEG_BOTTOM),
    ("secret", SECRET),
    ("blocksound", BLOCK_SOUND),
    ("dontdraw", DONT_DRAW),
    ("mapped", MAPPED),
    ("passuse", PASS_USE),
];

/// `Linedef` flags in the `Hexen` family: the shared vanilla bits, the
/// `Hexen`-specific bits and the activation names carrying their 3-bit
/// field value (see the module docs).
const LINEDEF_HEXEN: &[(&str, u32)] = &[
    ("blocking", BLOCKING),
    ("blockmonsters", BLOCK_MONSTERS),
    ("twosided", TWO_SIDED),
    ("dontpegtop", DONT_PEG_TOP),
    ("dontpegbottom", DONT_PEG_BOTTOM),
    ("secret", SECRET),
    ("blocksound", BLOCK_SOUND),
    ("dontdraw", DONT_DRAW),
    ("mapped", MAPPED),
    ("repeatspecial", REPEAT_SPECIAL),
    ("playercross", PLAYER_CROSS),
    ("playeruse", PLAYER_USE),
    ("monstercross", MONSTER_CROSS),
    ("impact", IMPACT),
    ("playerpush", PLAYER_PUSH),
    ("missilecross", MISSILE_CROSS),
    ("monsteractivate", MONSTER_ACTIVATE),
    ("blockplayers", HEXEN_BLOCK_PLAYERS),
    ("blockeverything", HEXEN_BLOCK_EVERYTHING),
];

/// `Thing` flags in the `Doom` family: the game and skill presence bits.
const THING_DOOM: &[(&str, u32)] = &[
    ("easy", THING_EASY),
    ("medium", THING_MEDIUM),
    ("hard", THING_HARD),
    ("ambush", THING_AMBUSH),
    ("notsingle", THING_DOOM_NOT_SINGLE),
    ("notdm", THING_DOOM_NOT_DM),
    ("notcoop", THING_DOOM_NOT_COOP),
    ("friend", THING_DOOM_FRIEND),
];

/// `Thing` flags in the `Hexen` family: skill, game mode, class,
/// rendering and movement bits.
const THING_HEXEN: &[(&str, u32)] = &[
    ("easy", THING_EASY),
    ("medium", THING_MEDIUM),
    ("hard", THING_HARD),
    ("ambush", THING_AMBUSH),
    ("dormant", THING_HEXEN_DORMANT),
    ("class1", THING_HEXEN_CLASS1),
    ("class2", THING_HEXEN_CLASS2),
    ("class3", THING_HEXEN_CLASS3),
    ("single", THING_HEXEN_SINGLE),
    ("coop", THING_HEXEN_COOP),
    ("dm", THING_HEXEN_DM),
    ("translucent", THING_HEXEN_TRANSLUCENT),
    ("invisible", THING_HEXEN_INVISIBLE),
    ("friend", THING_HEXEN_FRIEND),
    ("standing", THING_HEXEN_STANDING),
];

/// Whether a `UDMF` namespace is one of the `Doom`-flavoured ones that
/// store `Doom` family flags.
fn is_doomish(namespace: &str) -> bool {
    ["doom", "heretic", "strife"]
        .iter()
        .any(|doomish| doomish.eq_ignore_ascii_case(namespace))
}

/// Whether `name` is one of the `Hexen` linedef activation names, whose
/// table value is a field value rather than a bit.
#[must_use]
pub(super) fn is_activation_name(name: &str) -> bool {
    matches!(
        name,
        "playercross" | "playeruse" | "monstercross" | "impact" | "playerpush" | "missilecross"
    )
}

/// The table of `(name, value)` pairs for one element kind and family,
/// in bit order with the activation field names in field order.
#[must_use]
#[inline]
pub const fn names(kind: FlagKind, family: FlagFamily) -> &'static [(&'static str, u32)] {
    match (kind, family) {
        (FlagKind::Linedef, FlagFamily::Doom) => LINEDEF_DOOM,
        (FlagKind::Linedef, FlagFamily::Hexen) => LINEDEF_HEXEN,
        (FlagKind::Thing, FlagFamily::Doom) => THING_DOOM,
        (FlagKind::Thing, FlagFamily::Hexen) => THING_HEXEN,
    }
}

/// The names `bits` carries in one table: every set bit's name plus, for
/// the `Hexen` linedef table, the activation name whose field value the
/// bits hold.
#[must_use]
#[inline]
pub fn to_names(bits: u32, kind: FlagKind, family: FlagFamily) -> Vec<&'static str> {
    let field = bits & ACTIVATION_MASK;
    names(kind, family)
        .iter()
        .copied()
        .filter(|entry| carries(entry, bits, field))
        .map(|entry| entry.0)
        .collect()
}

/// Whether `entry`'s name is carried by `bits`: a bit entry when the
/// bit is set, an activation entry when the field holds its value.
fn carries(entry: &(&str, u32), bits: u32, field: u32) -> bool {
    if is_activation_name(entry.0) {
        return activation_bits(entry.1).is_some_and(|shifted| shifted == field);
    }
    bits & entry.1 == entry.1
}

/// The `bits` the names in `requested` stand for in one table.
///
/// Bit names are or-ed together; an activation name writes the whole
/// 3-bit activation field, so a later activation name replaces an
/// earlier one instead of combining with it.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the valid names when
/// `requested` holds a name the table does not carry.
#[inline]
pub fn from_names(
    requested: &[String],
    kind: FlagKind,
    family: FlagFamily,
) -> Result<u32, CyberError> {
    let mut bits = 0_u32;
    for wanted in requested {
        let entry = names(kind, family)
            .iter()
            .find(|found| found.0 == wanted.as_str())
            .ok_or_else(|| unknown(wanted, kind, family))?;
        bits = set_name(bits, entry);
    }
    Ok(bits)
}

/// The bits after applying one table entry to them.
fn set_name(bits: u32, entry: &(&str, u32)) -> u32 {
    if is_activation_name(entry.0) {
        let cleared = bits & !ACTIVATION_MASK;
        return cleared | activation_bits(entry.1).unwrap_or(0_u32);
    }
    bits | entry.1
}

/// The activation name's field value at its position in the flag bits.
#[must_use]
const fn activation_bits(value: u32) -> Option<u32> {
    value.checked_shl(ACTIVATION_SHIFT)
}

/// The `InvalidArgument` naming `wanted` and listing the valid names.
fn unknown(wanted: &str, kind: FlagKind, family: FlagFamily) -> CyberError {
    let valid: Vec<&str> = names(kind, family).iter().map(|entry| entry.0).collect();
    CyberError::InvalidArgument(format!(
        "unknown {} flag '{wanted}' for the {} family: valid names are {}",
        kind.as_str(),
        family.as_str(),
        valid.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVATION_MASK, BLOCK_MONSTERS, BLOCKING, FlagFamily, FlagKind, HEXEN_BLOCK_BITS,
        HEXEN_BLOCK_EVERYTHING, HEXEN_BLOCK_PLAYERS, MONSTER_ACTIVATE, PASS_USE, PLAYER_CROSS_BITS,
        PLAYER_USE_BITS, REPEAT_SPECIAL, SECRET, SHARED_LINEDEF_BITS, Skill, THING_AMBUSH,
        THING_DOOM_NOT_SINGLE, THING_EASY, THING_HEXEN_SINGLE, THING_HEXEN_STANDING,
        THING_SKILL_BITS, TWO_SIDED, blocks_players, from_names, names, to_names,
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

    #[test]
    fn the_doom_linedef_table_names_the_ten_vanilla_bits() {
        let bits = from_names(
            &all_names(FlagKind::Linedef, FlagFamily::Doom),
            FlagKind::Linedef,
            FlagFamily::Doom,
        )
        .expect("every name is known");
        assert_eq!(bits, 0x3FF, "ten bits, 1 through 512");
        assert_eq!(
            to_names(bits, FlagKind::Linedef, FlagFamily::Doom).len(),
            10,
            "and all ten read back"
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
        let doom_shared: Vec<String> = names(FlagKind::Linedef, FlagFamily::Doom)
            .iter()
            .map(|entry| entry.0.to_owned())
            .filter(|name| name != "passuse")
            .collect();
        let shared = from_names(&doom_shared, FlagKind::Linedef, FlagFamily::Doom)
            .expect("the nine shared names");
        assert_eq!(
            shared, SHARED_LINEDEF_BITS,
            "the mask covers blocking..mapped"
        );
        assert_eq!(
            from_names(&["passuse".to_owned()], FlagKind::Linedef, FlagFamily::Doom)
                .expect("passuse"),
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
}
