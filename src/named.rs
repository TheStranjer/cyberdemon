//! The shared name-to-enum option parser.
//!
//! Every tool that takes a named option (a lighting model, a merge mode,
//! a door style) refuses an unknown word the same way: the [`Named`]
//! trait says which values an enum has and what each is called, and
//! [`parse_named`] matches a word against them case-insensitively and
//! produces the uniform [`CyberError::InvalidArgument`] naming the field,
//! quoting the rejected text and listing every accepted name. Call sites
//! keep their own defaults (`None` or `""` meaning some variant) and only
//! hand settled, non-empty text to the helper.

use crate::error::CyberError;

/// An enum whose values all carry a fixed lowercase wire name.
///
/// Implementors list every named value in [`Self::ALL`] (the order the
/// refusal lists them in) and spell each one's canonical name in
/// [`Self::name`]; [`Self::by_name`] and [`Self::names`] come for free.
pub trait Named: Copy + Sized + 'static {
    /// Every named value, in report order.
    const ALL: &'static [Self];

    /// The canonical lowercase wire name of this value.
    fn name(self) -> &'static str;

    /// The value [`Self::name`] spells as `text`, matched
    /// case-insensitively; `None` for any other word.
    #[must_use]
    #[inline]
    fn by_name(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|value| value.name().eq_ignore_ascii_case(text))
    }

    /// Every name, in [`Self::ALL`] order.
    #[must_use]
    #[inline]
    fn names() -> Vec<&'static str> {
        Self::ALL.iter().copied().map(Self::name).collect()
    }
}

/// The enum value `text` names, matched case-insensitively against
/// `T::ALL`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming `field`, quoting `text` and
/// listing every accepted name in [`Named::ALL`] order.
#[inline]
pub fn parse_named<T: Named>(field: &str, text: &str) -> Result<T, CyberError> {
    T::by_name(text).ok_or_else(|| CyberError::InvalidArgument(unknown_named::<T>(field, text)))
}

/// The uniform refusal [`parse_named`] reports.
///
/// `unknown {field} {text:?}: pass one of "a", "b", "c"`. Callers
/// wrapping the error in a path prefix (`connections[0]: ...`) build
/// their message from this too, so the wording of a named-option
/// refusal is written once.
#[must_use]
#[inline]
pub fn unknown_named<T: Named>(field: &str, text: &str) -> String {
    let listed = T::names()
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<String>>()
        .join(", ");
    format!("unknown {field} {text:?}: pass one of {listed}")
}

#[cfg(test)]
mod tests {
    use super::{Named, parse_named, unknown_named};
    use crate::error::CyberError;

    /// A three-valued option with names of different shapes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Choice {
        /// Named `plain`.
        Plain,
        /// Named `two_words`.
        TwoWords,
        /// Named `x`.
        Short,
    }

    impl Named for Choice {
        const ALL: &'static [Self] = &[Self::Plain, Self::TwoWords, Self::Short];

        #[inline]
        fn name(self) -> &'static str {
            match self {
                Self::Plain => "plain",
                Self::TwoWords => "two_words",
                Self::Short => "x",
            }
        }
    }

    /// The message of an `InvalidArgument` refusal, checking its code.
    fn refusal(error: &CyberError) -> String {
        assert_eq!(error.code(), "invalid_argument", "refusal code");
        error.to_string()
    }

    #[test]
    fn by_name_and_names_walk_all_in_order() {
        assert_eq!(
            Choice::names(),
            vec!["plain", "two_words", "x"],
            "ALL order"
        );
        assert_eq!(Choice::by_name("two_words"), Some(Choice::TwoWords));
        assert_eq!(Choice::by_name("nothing"), None, "no value answers");
    }

    #[test]
    fn parse_named_accepts_every_name_any_case() {
        for (text, wanted) in [
            ("plain", Choice::Plain),
            ("PLAIN", Choice::Plain),
            ("Two_Words", Choice::TwoWords),
            ("X", Choice::Short),
        ] {
            assert_eq!(
                parse_named::<Choice>("choice", text).expect(text),
                wanted,
                "{text} maps to its value"
            );
        }
    }

    #[test]
    fn parse_named_refuses_an_unknown_word_naming_the_choices() {
        let error = parse_named::<Choice>("choice", "sliding").expect_err("no such name");
        let message = refusal(&error);
        assert_eq!(
            message, "unknown choice \"sliding\": pass one of \"plain\", \"two_words\", \"x\"",
            "the uniform wording: field, quoted text, quoted name list"
        );
    }

    #[test]
    fn unknown_named_phrases_the_refusal_for_a_prefixed_caller() {
        assert_eq!(
            unknown_named::<Choice>("style", "roof"),
            "unknown style \"roof\": pass one of \"plain\", \"two_words\", \"x\"",
            "the same message the helper itself reports"
        );
    }
}
