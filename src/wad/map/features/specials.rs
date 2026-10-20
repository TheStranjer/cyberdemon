//! The pure sector-special builders: the secret flag and the light effect.
//!
//! Both are bit arithmetic on one sector's `special` number, the same shape
//! as the per-family tables in [`super::doors`] and [`super::switches`]:
//! [`secret_edit`] answers what marking (or clearing) a sector's secret
//! flag turns its special into, and [`light_special`] answers what a
//! [`LightEffect`] turns it into. The `features` tools `mark_secret` and
//! `set_light_effect` write the answers, and so does the plan feature
//! engine's light and secret steps; [`boom_note`] is the one vanilla-Doom
//! warning both secret paths emit. Nothing here reads a session, an
//! archive or the `MCP` layer.
//!
//! The two flag families disagree about where the meaning lives. The `Doom`
//! family keeps a vanilla table number in its low five bits and, from
//! [`GENERALIZED_SECTOR_MIN`] up, a `Boom` generalized bit field whose bit
//! 7, [`BOOM_SECRET_BIT`], is the secret flag; vanilla `Doom` compares the
//! whole number instead of reading bits, so a `Boom` flag riding on a
//! vanilla type is read as neither the effect nor the secret, which is what
//! [`SecretEdit::boom_warning`] reports. The `Hexen` family keeps its basic
//! type in the low eight bits, numbers the `Doom`-equivalent effects `64`
//! higher, and flags secrecy with bit 10, [`HEXEN_SECRET_BIT`].

use serde::{Deserialize, Serialize};

use crate::wad::map::flags::FlagFamily;

/// The `Doom` family's vanilla secret sector type.
pub const DOOM_SECRET_SPECIAL: i32 = 9;
/// The `Boom` generalized secret flag: bit 7 of a `Doom`-family special.
pub const BOOM_SECRET_BIT: i32 = 128;
/// The lowest `Boom` generalized `Doom` sector special: at and above it a
/// sector special is a bit field, below it a vanilla table number.
pub const GENERALIZED_SECTOR_MIN: i32 = 32;
/// The `Hexen` family's secret sector flag: bit 10 of a sector special.
pub const HEXEN_SECRET_BIT: i32 = 1024;
/// The `Doom` family's basic sector type field: bits 0 to 4.
const DOOM_TYPE_MASK: i32 = 31;
/// The `Hexen` family's basic sector type field: bits 0 to 7.
const HEXEN_TYPE_MASK: i32 = 255;

/// What [`secret_edit`] turns one sector's `special` into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretEdit {
    /// The `special` value to store.
    pub special: i32,
    /// Whether a `Boom` secret bit was set on a vanilla `Doom` type: to a
    /// vanilla engine the number now names neither the effect nor the
    /// secret, because it compares the special whole instead of reading
    /// bits.
    pub boom_warning: bool,
}

/// The [`SecretEdit`] of flag family `family` for a sector carrying
/// `special`, marking it secret when `secret` is set and clearing the flag
/// when it is not.
///
/// The `Doom` family: a plain sector (`0`) becomes the vanilla
/// [`DOOM_SECRET_SPECIAL`], one already secret that way stays `9`, and any
/// other value gains or loses [`BOOM_SECRET_BIT`] with every other bit
/// kept; a `Boom` generalized special (from [`GENERALIZED_SECTOR_MIN`] up,
/// which includes one already carrying the flag) changes silently, while a
/// vanilla type in `1..=31` comes back with
/// [`SecretEdit::boom_warning`]. Clearing a vanilla type that never carried
/// the flag leaves it untouched.
///
/// The `Hexen` family sets or clears [`HEXEN_SECRET_BIT`] on top of
/// whatever special the sector holds and never warns: its basic type lives
/// in bits below the flag, so both survive.
#[must_use]
#[inline]
pub const fn secret_edit(family: FlagFamily, special: i32, secret: bool) -> SecretEdit {
    match family {
        FlagFamily::Doom => doom_secret(special, secret),
        FlagFamily::Hexen => hexen_secret(special, secret),
    }
}

/// The `Doom`-family row of [`secret_edit`]: the vanilla `9` for a plain
/// sector, the `Boom` bit 7 for everything that already does something.
const fn doom_secret(special: i32, secret: bool) -> SecretEdit {
    if secret {
        return doom_mark(special);
    }
    doom_clear(special)
}

/// Setting the `Doom` secret flag: the plain sector takes the vanilla
/// secret type, everything else rides on the `Boom` bit.
const fn doom_mark(special: i32) -> SecretEdit {
    if special == 0_i32 || special == DOOM_SECRET_SPECIAL {
        return SecretEdit {
            special: DOOM_SECRET_SPECIAL,
            boom_warning: false,
        };
    }
    SecretEdit {
        special: special | BOOM_SECRET_BIT,
        boom_warning: special > 0_i32 && special < GENERALIZED_SECTOR_MIN,
    }
}

/// Clearing the `Doom` secret flag: the vanilla type goes back to plain and
/// the `Boom` bit comes off anything from the generalized range up, which
/// is where a flagged vanilla type landed.
const fn doom_clear(special: i32) -> SecretEdit {
    if special == DOOM_SECRET_SPECIAL {
        return SecretEdit {
            special: 0_i32,
            boom_warning: false,
        };
    }
    if special >= GENERALIZED_SECTOR_MIN {
        return SecretEdit {
            special: special & !BOOM_SECRET_BIT,
            boom_warning: false,
        };
    }
    SecretEdit {
        special,
        boom_warning: false,
    }
}

/// The `Hexen`-family row of [`secret_edit`]: bit 10 rides over the
/// sector's own effect number, so only that one bit moves.
const fn hexen_secret(special: i32, secret: bool) -> SecretEdit {
    let updated = if secret {
        special | HEXEN_SECRET_BIT
    } else {
        special & !HEXEN_SECRET_BIT
    };
    SecretEdit {
        special: updated,
        boom_warning: false,
    }
}

/// The light effects [`light_special`] installs, as the tool names them.
///
/// The serde names are the `snake_case` [`LightEffect::name`]s, so a plan
/// or a tool payload spells an effect exactly as the tools do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightEffect {
    /// No effect: the basic type clears to `0` and the sector is lit
    /// statically.
    None,
    /// The torch-like flicker: vanilla `17`, `Hexen` `81`.
    Flicker,
    /// The half-second strobe: vanilla `2`, `Hexen` `66`.
    StrobeFast,
    /// The one-second strobe: vanilla `3`, `Hexen` `67`.
    StrobeSlow,
    /// The slow sinusoidal glow: vanilla `8`, `Hexen` `72`.
    Glow,
    /// The light that drops out at random: vanilla `1`, `Hexen` `65`.
    RandomOff,
}

impl LightEffect {
    /// Every effect, in table order.
    #[must_use]
    #[inline]
    pub const fn all() -> [Self; 6] {
        [
            Self::None,
            Self::Flicker,
            Self::StrobeFast,
            Self::StrobeSlow,
            Self::Glow,
            Self::RandomOff,
        ]
    }

    /// The canonical wire name of this effect.
    #[must_use]
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Flicker => "flicker",
            Self::StrobeFast => "strobe_fast",
            Self::StrobeSlow => "strobe_slow",
            Self::Glow => "glow",
            Self::RandomOff => "random_off",
        }
    }

    /// This effect's basic sector type number in flag family `family`: the
    /// vanilla `Doom` table number, and that number `64` higher in the
    /// `Hexen` family, whose `Doom`-equivalent specials start at `64`
    /// ([`LightEffect::None`] is `0` in both families).
    #[must_use]
    #[inline]
    pub const fn basic_special(self, family: FlagFamily) -> i32 {
        let (doom, hexen) = match self {
            Self::None => (0_i32, 0_i32),
            Self::Flicker => (17_i32, 81_i32),
            Self::StrobeFast => (2_i32, 66_i32),
            Self::StrobeSlow => (3_i32, 67_i32),
            Self::Glow => (8_i32, 72_i32),
            Self::RandomOff => (1_i32, 65_i32),
        };
        match family {
            FlagFamily::Doom => doom,
            FlagFamily::Hexen => hexen,
        }
    }
}

/// The sector `special` that carries `effect` for a sector already holding
/// `special`, in flag family `family`: the effect's basic type in the
/// family's basic-type field, every bit above that field kept.
///
/// So a `Doom`-family `Boom` generalized sector keeps its generalized
/// behaviour while its vanilla type (bits 0 to 4) is replaced, and a
/// `Hexen`-family sector keeps everything from bit 8 up (its damage flags
/// and the secret bit at [`HEXEN_SECRET_BIT`]) while the `Doom`-equivalent
/// number goes into bits 0 to 7.
#[must_use]
#[inline]
pub const fn light_special(family: FlagFamily, special: i32, effect: LightEffect) -> i32 {
    match family {
        FlagFamily::Doom => (special & !DOOM_TYPE_MASK) | effect.basic_special(FlagFamily::Doom),
        FlagFamily::Hexen => (special & !HEXEN_TYPE_MASK) | effect.basic_special(FlagFamily::Hexen),
    }
}

/// The Boom-bit warning for sector `index`, whose special now carries
/// the secret flag on top of a vanilla `Doom` type.
///
/// The effect and the secret survive only in ports that read bits, so
/// a map that has to work in vanilla wants its own sector with the
/// vanilla type instead.
///
/// Shared by the `mark_secret` tool and the plan feature engine's
/// secret step so both paths emit the identical warning.
#[must_use]
#[inline]
pub fn boom_note(index: usize, flagged: i32) -> String {
    format!(
        "sector {index} now carries special {flagged}: vanilla Doom ignores Boom bits, so \
         that engine reads neither the old effect nor the secret (only a Boom-compatible \
         or ZDoom port does); mark it with the vanilla type {DOOM_SECRET_SPECIAL} instead \
         if it has to stay hidden from every engine"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        BOOM_SECRET_BIT, DOOM_SECRET_SPECIAL, DOOM_TYPE_MASK, GENERALIZED_SECTOR_MIN,
        HEXEN_SECRET_BIT, HEXEN_TYPE_MASK, LightEffect, SecretEdit, boom_note, light_special,
        secret_edit,
    };
    use crate::wad::map::flags::FlagFamily;

    /// The `Doom` secret answer with `special` and the given Boom warning.
    const fn doom(special: i32, warned: bool) -> SecretEdit {
        SecretEdit {
            special,
            boom_warning: warned,
        }
    }

    #[test]
    fn a_plain_doom_sector_takes_the_vanilla_secret_type() {
        assert_eq!(
            secret_edit(FlagFamily::Doom, 0_i32, true),
            doom(DOOM_SECRET_SPECIAL, false),
            "0 becomes the vanilla 9"
        );
        assert_eq!(
            secret_edit(FlagFamily::Doom, DOOM_SECRET_SPECIAL, true),
            doom(DOOM_SECRET_SPECIAL, false),
            "an already vanilla-secret sector stays 9 rather than gaining a Boom bit"
        );
        assert_eq!(
            secret_edit(FlagFamily::Doom, DOOM_SECRET_SPECIAL, false),
            doom(0_i32, false),
            "and clearing it goes back to plain"
        );
        assert_eq!(
            secret_edit(FlagFamily::Doom, 0_i32, false),
            doom(0_i32, false),
            "a plain sector has no flag to clear"
        );
    }

    #[test]
    fn a_boom_doom_sector_gains_and_loses_only_bit_seven() {
        let generalized = GENERALIZED_SECTOR_MIN | 1_i32;
        let marked = secret_edit(FlagFamily::Doom, generalized, true);
        assert_eq!(marked, doom(generalized | BOOM_SECRET_BIT, false));
        assert_eq!(
            marked.special & !BOOM_SECRET_BIT,
            generalized,
            "every other bit of the generalized field survives"
        );
        assert_eq!(
            secret_edit(FlagFamily::Doom, marked.special, false),
            doom(generalized, false),
            "clearing takes exactly the flag back off"
        );
        assert_eq!(
            secret_edit(FlagFamily::Doom, 0xFFFF_i32, true).special,
            0xFFFF_i32,
            "a sector whose flags are all set stays as it is"
        );
    }

    #[test]
    fn a_vanilla_type_marked_secret_warns_about_boom_bits() {
        for special in [1_i32, 2_i32, 8_i32, 17_i32, 31_i32] {
            assert_eq!(
                secret_edit(FlagFamily::Doom, special, true),
                doom(special | BOOM_SECRET_BIT, true),
                "the flag rides on the vanilla type and the caller hears about it"
            );
        }
        assert_eq!(
            secret_edit(FlagFamily::Doom, 0x11_i32 | BOOM_SECRET_BIT, false),
            doom(0x11_i32, false),
            "clearing a warned sector returns the plain vanilla type"
        );
        assert_eq!(
            secret_edit(FlagFamily::Doom, 0x11_i32, false),
            doom(0x11_i32, false),
            "clearing a never-flagged vanilla type changes nothing"
        );
    }

    #[test]
    fn the_hexen_family_flags_bit_ten_over_any_special() {
        for special in [0_i32, 9_i32, 81_i32, 0x123_i32] {
            let marked = secret_edit(FlagFamily::Hexen, special, true);
            assert_eq!(marked.special, special | HEXEN_SECRET_BIT, "{special}");
            assert!(!marked.boom_warning, "the Hexen family never warns");
            assert_eq!(
                secret_edit(FlagFamily::Hexen, marked.special, false).special,
                special,
                "and clearing returns exactly what it took, bit 10 only"
            );
        }
    }

    #[test]
    fn effects_name_themselves_in_one_canonical_order() {
        let names: Vec<&str> = LightEffect::all().map(LightEffect::name).to_vec();
        assert_eq!(
            names,
            vec![
                "none",
                "flicker",
                "strobe_fast",
                "strobe_slow",
                "glow",
                "random_off"
            ],
            "the wire vocabulary"
        );
    }

    #[test]
    fn the_serde_name_of_an_effect_is_its_canonical_name() {
        for effect in LightEffect::all() {
            let written = serde_json::to_value(effect).expect("serialising an effect");
            assert_eq!(
                written,
                serde_json::Value::String(effect.name().to_owned()),
                "{effect:?} serialises to its own name"
            );
            let again: LightEffect =
                serde_json::from_value(written).expect("re-reading the effect");
            assert_eq!(again, effect, "the name reads back");
        }
    }

    #[test]
    fn the_hexen_effect_number_is_the_doom_one_sixty_four_higher() {
        let table = [
            (LightEffect::None, 0_i32, 0_i32),
            (LightEffect::RandomOff, 1_i32, 65_i32),
            (LightEffect::StrobeFast, 2_i32, 66_i32),
            (LightEffect::StrobeSlow, 3_i32, 67_i32),
            (LightEffect::Glow, 8_i32, 72_i32),
            (LightEffect::Flicker, 17_i32, 81_i32),
        ];
        for (effect, doom, hexen) in table {
            assert_eq!(
                effect.basic_special(FlagFamily::Doom),
                doom,
                "the vanilla {effect:?} number"
            );
            assert_eq!(
                effect.basic_special(FlagFamily::Hexen),
                hexen,
                "the Hexen {effect:?} number is the Doom one 64 higher, none excepted"
            );
        }
    }

    #[test]
    fn a_doom_effect_replaces_only_the_low_five_bits() {
        let generalized = 0x2F81_i32;
        let lit = light_special(FlagFamily::Doom, generalized, LightEffect::Flicker);
        assert_eq!(lit & DOOM_TYPE_MASK, 17_i32, "the flicker type");
        assert_eq!(
            lit & !DOOM_TYPE_MASK,
            generalized & !DOOM_TYPE_MASK,
            "the generalized field survives the repaint"
        );
        assert_eq!(
            light_special(FlagFamily::Doom, generalized, LightEffect::None) & !DOOM_TYPE_MASK,
            generalized & !DOOM_TYPE_MASK,
            "none clears the type but nothing above it"
        );
    }

    #[test]
    fn a_hexen_effect_replaces_only_the_low_eight_bits() {
        let with_secret = 0x1_0090_i32;
        let lit = light_special(FlagFamily::Hexen, with_secret, LightEffect::Glow);
        assert_eq!(lit & HEXEN_TYPE_MASK, 72_i32, "the Doom-equivalent glow");
        assert_eq!(
            lit & !HEXEN_TYPE_MASK,
            with_secret & !HEXEN_TYPE_MASK,
            "the flags above the type field, the secret bit included, stay put"
        );
        assert_eq!(
            light_special(FlagFamily::Hexen, lit, LightEffect::None) & HEXEN_TYPE_MASK,
            0_i32,
            "none clears the type field only"
        );
    }

    #[test]
    fn every_effect_lands_in_its_own_family_field() {
        for effect in LightEffect::all() {
            assert_eq!(
                light_special(FlagFamily::Doom, 0_i32, effect),
                effect.basic_special(FlagFamily::Doom),
                "the Doom family's {effect:?} from a plain sector"
            );
            assert_eq!(
                light_special(FlagFamily::Hexen, 0_i32, effect),
                effect.basic_special(FlagFamily::Hexen),
                "the Hexen family's {effect:?} from a plain sector"
            );
        }
    }

    #[test]
    fn the_boom_note_names_the_sector_the_flagged_special_and_the_fix() {
        let flagged = BOOM_SECRET_BIT | 3_i32;
        let note = boom_note(3_usize, flagged);
        assert!(note.contains("sector 3"), "{note}");
        assert!(note.contains("special 131"), "{note}");
        assert!(
            note.contains("vanilla Doom ignores Boom bits")
                && note.contains(&format!("vanilla type {DOOM_SECRET_SPECIAL}")),
            "the note explains the engine gap and the fix: {note}"
        );
    }
}
