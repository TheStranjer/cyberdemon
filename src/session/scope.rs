//! Where the archives stand: the origin of a lump of the merged view, that
//! lump as the view holds it, and the scope names a read tool asks for.
//!
//! [`Source`] tags every [`MergedLump`] with the archive it was folded in
//! from, and [`Scope`] is how a client picks the archive it wants to read:
//! the merged view, the working archive alone, or one read-only layer by
//! index. Both name the same stack [`crate::session::Session::layers`]
//! returns, so a layer index means the same thing on both sides;
//! [`crate::session::Session::lumps_in`] turns a [`Scope`] into a directory.

use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::wad::archive::Lump;

/// Which archive a lump of the merged view was taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Layer `n` of [`crate::session::Session::layers`]: 0 is the `IWAD`,
    /// then the resources.
    Layer(usize),
    /// The editable working archive.
    Working,
}

/// A lump as the merged view sees it: the bytes plus their origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedLump {
    /// The lump itself.
    pub lump: Lump,
    /// The archive it was taken from.
    pub source: Source,
    /// Its index inside that archive's directory.
    pub source_index: usize,
}

/// The archive a read tool looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The merged view: `IWAD`, resource layers, then the working archive.
    Merged,
    /// The editable working archive alone.
    Working,
    /// Read-only layer `n` of [`crate::session::Session::layers`] alone.
    Layer(usize),
}

impl Scope {
    /// Parses a scope name: `merged`, `working` or `layer:N`, where `N` is a
    /// zero-based index into [`crate::session::Session::layers`]. Every name
    /// is matched case-insensitively.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the accepted forms when `text`
    /// is neither keyword and `layer:` is not followed by a decimal index.
    #[inline]
    pub fn parse(text: &str) -> Result<Self, CyberError> {
        let lowered = text.to_ascii_lowercase();
        if lowered == "merged" {
            return Ok(Self::Merged);
        }
        if lowered == "working" {
            return Ok(Self::Working);
        }
        let Some(digits) = lowered.strip_prefix(LAYER_PREFIX) else {
            return Err(unknown_scope(text));
        };
        digits.parse::<usize>().map(Self::Layer).map_err(|error| {
            CyberError::InvalidArgument(format!(
                "scope {text:?} names layer {digits:?}, which is not a decimal index: {error}"
            ))
        })
    }

    /// Parses an optional `scope` parameter, [`Scope::Merged`] standing for
    /// an absent or `null` one: the default read every scoped tool runs on.
    ///
    /// # Errors
    ///
    /// [`Scope::parse`]'s [`CyberError::InvalidArgument`] for any other name.
    #[inline]
    pub fn parse_or_merged(text: Option<&str>) -> Result<Self, CyberError> {
        text.map_or(Ok(Self::Merged), Self::parse)
    }

    /// Whether `text` names a scope in [`Scope::parse`]' grammar, matched
    /// case-insensitively: one of the keywords or the `layer:` prefix, even
    /// when the index behind the prefix is malformed. Callers that layer a
    /// scope grammar over other names (a side string that is also `base` or
    /// a path) test this before delegating the full parse.
    #[must_use]
    #[inline]
    pub fn accepts(text: &str) -> bool {
        let lowered = text.to_ascii_lowercase();
        matches!(lowered.as_str(), "merged" | "working") || lowered.starts_with(LAYER_PREFIX)
    }
}

/// The prefix a [`Scope::Layer`] name carries.
const LAYER_PREFIX: &str = "layer:";

/// The scope's wire name: `merged`, `working` or `layer:N`, the exact
/// spellings [`Scope::parse`] accepts, so a label round-trips.
impl core::fmt::Display for Scope {
    #[inline]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Merged => f.write_str("merged"),
            Self::Working => f.write_str("working"),
            Self::Layer(index) => write!(f, "{LAYER_PREFIX}{index}"),
        }
    }
}

/// The refusal for text that is not one of the accepted scope names.
fn unknown_scope(text: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "unknown scope {text:?}: expected \"merged\", \"working\" or \"layer:N\", where N is a \
         zero-based index into the loaded layers"
    ))
}

#[cfg(test)]
mod tests {
    use super::{LAYER_PREFIX, Scope};

    #[test]
    fn parse_or_merged_defaults_an_absent_name_to_the_merged_view() {
        assert_eq!(Scope::parse_or_merged(None).expect("absent"), Scope::Merged);
        assert_eq!(
            Scope::parse_or_merged(Some("WORKING")).expect("working"),
            Scope::Working,
            "case-insensitive like Scope::parse"
        );
        assert_eq!(
            Scope::parse_or_merged(Some("layer:2")).expect("layer"),
            Scope::Layer(2),
            "a layer index"
        );
        let error = Scope::parse_or_merged(Some("nope")).expect_err("not a scope");
        assert_eq!(error.code(), "invalid_argument", "bad scope code");
        assert_eq!(
            error.to_string(),
            "unknown scope \"nope\": expected \"merged\", \"working\" or \"layer:N\", where N \
             is a zero-based index into the loaded layers",
            "the shared refusal text"
        );
    }

    #[test]
    fn display_renders_the_wire_names_that_parse_back() {
        for scope in [
            Scope::Merged,
            Scope::Working,
            Scope::Layer(0),
            Scope::Layer(12),
        ] {
            let text = scope.to_string();
            let reparsed = Scope::parse(text.as_str()).expect("the label is a name");
            assert_eq!(reparsed, scope, "round trip of {scope:?}");
        }
        assert_eq!(Scope::Merged.to_string(), "merged", "merged label");
        assert_eq!(Scope::Working.to_string(), "working", "working label");
        assert_eq!(
            Scope::Layer(3).to_string(),
            format!("{LAYER_PREFIX}3"),
            "layer label"
        );
    }

    #[test]
    fn accepts_tests_the_grammar_without_reading_the_index() {
        assert!(Scope::accepts("merged"), "the keyword");
        assert!(Scope::accepts("Working"), "a keyword, any case");
        assert!(Scope::accepts("LAYER:1"), "a layer name, any case");
        assert!(Scope::accepts("layer:x"), "a malformed index still matches");
        assert!(!Scope::accepts("base"), "not a scope keyword");
        assert!(!Scope::accepts("wads/extra.wad"), "nor a path");
        assert!(!Scope::accepts("layers:2"), "nor a near miss");
    }
}
