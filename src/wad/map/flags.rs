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
//! The `Hexen` linedef table carries the trigger activation as a 3-bit
//! field (`flags & 0x1C00`, the value shifted to bit 10):
//! the six activation names hold the *field value* `0..=5` in their table
//! entry, not a bit. [`to_names`] reports the name whose field value the
//! bits carry (so `playercross` names the all-clear field), and
//! [`from_names`] writes the field: a later activation name replaces an
//! earlier one. Every other entry, in every table, is a single bit.
//!
//! [`Linedef`]: crate::wad::map::Linedef
//! [`Thing`]: crate::wad::map::Thing
//! [`Map::flag_family`]: crate::wad::map::Map::flag_family

use crate::error::CyberError;

/// Mask covering the `Hexen` linedef activation field.
pub(super) const ACTIVATION_MASK: u32 = 0x1C00;
/// Shift from an activation field value to its position in the flag bits.
pub(super) const ACTIVATION_SHIFT: u32 = 10_u32;
/// `Hexen` linedef bit letting monsters trigger special lines.
pub(super) const MONSTER_ACTIVATE: u32 = 0x2000;

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
    ("blocking", 1_u32),
    ("blockmonsters", 2_u32),
    ("twosided", 4_u32),
    ("dontpegtop", 8_u32),
    ("dontpegbottom", 16_u32),
    ("secret", 32_u32),
    ("blocksound", 64_u32),
    ("dontdraw", 128_u32),
    ("mapped", 256_u32),
    ("passuse", 512_u32),
];

/// `Linedef` flags in the `Hexen` family: the shared vanilla bits, the
/// `Hexen`-specific bits and the activation names carrying their 3-bit
/// field value (see the module docs).
const LINEDEF_HEXEN: &[(&str, u32)] = &[
    ("blocking", 1_u32),
    ("blockmonsters", 2_u32),
    ("twosided", 4_u32),
    ("dontpegtop", 8_u32),
    ("dontpegbottom", 16_u32),
    ("secret", 32_u32),
    ("blocksound", 64_u32),
    ("dontdraw", 128_u32),
    ("mapped", 256_u32),
    ("repeatspecial", 0x200),
    ("playercross", 0_u32),
    ("playeruse", 1_u32),
    ("monstercross", 2_u32),
    ("impact", 3_u32),
    ("playerpush", 4_u32),
    ("missilecross", 5_u32),
    ("monsteractivate", MONSTER_ACTIVATE),
    ("blockplayers", 0x4000),
    ("blockeverything", 0x8000),
];

/// `Thing` flags in the `Doom` family: the game and skill presence bits.
const THING_DOOM: &[(&str, u32)] = &[
    ("easy", 1_u32),
    ("medium", 2_u32),
    ("hard", 4_u32),
    ("ambush", 8_u32),
    ("notsingle", 16_u32),
    ("notdm", 32_u32),
    ("notcoop", 64_u32),
    ("friend", 128_u32),
];

/// `Thing` flags in the `Hexen` family: skill, game mode, class,
/// rendering and movement bits.
const THING_HEXEN: &[(&str, u32)] = &[
    ("easy", 1_u32),
    ("medium", 2_u32),
    ("hard", 4_u32),
    ("ambush", 8_u32),
    ("dormant", 16_u32),
    ("class1", 32_u32),
    ("class2", 64_u32),
    ("class3", 128_u32),
    ("single", 256_u32),
    ("coop", 512_u32),
    ("dm", 1024_u32),
    ("translucent", 2048_u32),
    ("invisible", 4096_u32),
    ("friend", 8192_u32),
    ("standing", 0x4000),
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
        ACTIVATION_MASK, FlagFamily, FlagKind, MONSTER_ACTIVATE, from_names, names, to_names,
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
}
