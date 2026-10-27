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
//! The two families' block bits are the tables' only overlap in *meaning*
//! rather than in position: `blocking` both families share, `Hexen` adds
//! its `blockplayers` and `blockeverything` at bits 14 and 15, and `Doom`
//! adds `Boom`'s `passuse` beside `MBF21`'s `blocklandmonsters` and
//! `blockplayers` at bits 12 and 13, the positions its Level Editing Spec
//! gives them. `blocks_players` is the one family-aware answer to which of
//! them keep a player out.
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
/// The `Doom`-family linedef bit 4096: `blocklandmonsters`, which only
/// walking monsters cannot cross.
///
/// `MBF21`'s Level Editing Spec v1.1 lists the two bits it adds to the
/// `Boom` flags word under **Linedef Flags**: `4096`, bit 12, "Block land
/// monsters" and `8192`, bit 13, "Block players"
/// (<https://github.com/kraflab/mbf21/blob/master/docs/level_editor_spec.md>).
/// The `Strife` namespace's own line translucency bits share these two
/// positions and are not modelled here.
pub(crate) const DOOM_BLOCK_LAND_MONSTERS: u32 = 0x1000;
/// The `Doom`-family linedef bit 8192: `blockplayers`, which only the
/// player cannot cross: `MBF21`'s second bit, at the position its Level
/// Editing Spec v1.1's **Linedef Flags** table gives it (see
/// [`DOOM_BLOCK_LAND_MONSTERS`]). `ZDoom`'s own `blockplayers` sits at
/// bit 14 ([`HEXEN_BLOCK_PLAYERS`]) instead.
pub(crate) const DOOM_BLOCK_PLAYERS: u32 = 0x2000;
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
/// The two `Hexen`-family block bits no `Doom`-family table names:
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
/// bit every family shares, plus the family's own player-only block: the
/// `Hexen` family's `blockplayers` and `blockeverything` bits, the `Doom`
/// family's `MBF21` `blockplayers`. The monster blocks are deliberately
/// not among them: a player walks past a line that only holds monsters,
/// and a bit one family does not define reads as nothing on the other's
/// lines.
#[must_use]
pub(crate) const fn blocks_players(flags: u32, family: FlagFamily) -> bool {
    if flags & BLOCKING == BLOCKING {
        return true;
    }
    match family {
        FlagFamily::Doom => flags & DOOM_BLOCK_PLAYERS == DOOM_BLOCK_PLAYERS,
        FlagFamily::Hexen => {
            flags & HEXEN_BLOCK_PLAYERS == HEXEN_BLOCK_PLAYERS
                || flags & HEXEN_BLOCK_EVERYTHING == HEXEN_BLOCK_EVERYTHING
        }
    }
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

/// `Linedef` flags in the `Doom` family: the nine vanilla bits, `Boom`'s
/// `passuse` and the two `MBF21` block bits.
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
    ("blocklandmonsters", DOOM_BLOCK_LAND_MONSTERS),
    ("blockplayers", DOOM_BLOCK_PLAYERS),
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

/// Whether `wanted` names a flag in `kind`'s table for `family`,
/// refusing it with the shared unknown-flag wording listing the valid
/// names when the table does not carry it.
///
/// The single check the `find_elements` flag filters run on a named
/// flag; [`from_names`] refuses the same name the same way through the
/// same builder.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when the table holds no entry named
/// `wanted`.
#[inline]
pub fn check_flag(kind: FlagKind, family: FlagFamily, wanted: &str) -> Result<(), CyberError> {
    if names(kind, family).iter().any(|entry| entry.0 == wanted) {
        return Ok(());
    }
    Err(unknown(wanted, kind, family))
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
mod tests;
