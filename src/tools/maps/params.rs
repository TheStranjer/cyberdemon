//! Shared wire types and validators for the map editing tools.
//!
//! The `flags` and index parameter shapes ([`FlagsInput`],
//! [`IndexSelector`]), element bounds checking ([`check_index`]),
//! texture-name validation ([`validate_texture_name`]) and the standard
//! result fragment ([`edit_result_fragment`]) every edit tool merges into
//! its structured content.
//!
//! These are the pieces the per-element editing tools (vertices, linedefs,
//! sidedefs, sectors, things, ...) reuse so they stay thin wrappers around
//! [`crate::session::Session::edit_map`]: parse the parameters with these
//! helpers, run the typed edit, report the fragment.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::session::map_edit::MapEditReport;
use crate::wad::classify::MapFormat;
use crate::wad::map::{FlagFamily, FlagKind, flags};
use crate::wad::name::MAX_NAME_LEN;

/// Longest texture name a `UDMF` map may carry.
const UDMF_TEXTURE_MAX: usize = 64;

/// A `flags` parameter: the raw bitset or a list of flag names.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FlagsInput {
    /// The bitset exactly as the map's format stores it.
    Bits(
        /// The raw flag bits.
        u32,
    ),
    /// Names from the map's own flag family and the edited element kind;
    /// they combine, and an unknown name fails listing the valid ones.
    Names(
        /// The flag names to set.
        Vec<String>,
    ),
}

impl FlagsInput {
    /// The flag bits this input stands for: [`Bits`] pass through,
    /// [`Names`] fold through [`flags::from_names`] for `kind` and
    /// `family`.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] listing the valid names when a
    /// name is not in the table.
    ///
    /// [`Bits`]: Self::Bits
    /// [`Names`]: Self::Names
    #[inline]
    pub fn resolve(self, kind: FlagKind, family: FlagFamily) -> Result<u32, CyberError> {
        match self {
            Self::Bits(bits) => Ok(bits),
            Self::Names(names) => flags::from_names(&names, kind, family),
        }
    }
}

/// An element index parameter: one index or a list of them.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum IndexSelector {
    /// A single element.
    One(
        /// The index.
        usize,
    ),
    /// Several elements at once; duplicates collapse.
    Many(
        /// The listed indices.
        Vec<usize>,
    ),
}

impl IndexSelector {
    /// The selected indices, sorted ascending with duplicates removed.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when the list is empty: an edit
    /// over no elements is never what the caller meant.
    #[inline]
    pub fn indices(self) -> Result<Vec<usize>, CyberError> {
        let mut indices = match self {
            Self::One(index) => vec![index],
            Self::Many(list) => list,
        };
        if indices.is_empty() {
            return Err(CyberError::InvalidArgument(
                "the index list is empty: pass one index or a non-empty list".to_owned(),
            ));
        }
        indices.sort_unstable();
        indices.dedup();
        Ok(indices)
    }
}

/// Refuses an element `index` outside `0..len`.
///
/// # Errors
///
/// [`CyberError::NotFound`] phrased for `kind` (e.g. `"linedef 12 does not
/// exist (map has 7 linedefs)"`).
#[inline]
pub fn check_index(kind: &str, index: usize, len: usize) -> Result<(), CyberError> {
    if index < len {
        return Ok(());
    }
    Err(CyberError::NotFound(format!(
        "{kind} {index} does not exist (map has {len} {kind}s)"
    )))
}

/// Validates one texture or flat name and returns it uppercased.
///
/// `-` and any printable `ASCII` character are allowed, the name may not
/// be empty, and its length is capped at eight characters for the binary
/// formats and 64 for `UDMF`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty name, a name over the
/// format's cap, or a character outside printable `ASCII`.
#[inline]
pub fn validate_texture_name(name: &str, format: MapFormat) -> Result<String, CyberError> {
    if name.is_empty() {
        return Err(CyberError::InvalidArgument(
            "the texture name is empty: pass \"-\" for no texture".to_owned(),
        ));
    }
    let max = match format {
        MapFormat::Doom | MapFormat::Hexen => MAX_NAME_LEN,
        MapFormat::Udmf => UDMF_TEXTURE_MAX,
    };
    if name.len() > max {
        return Err(CyberError::InvalidArgument(format!(
            "texture name {name:?} is {} characters, the {format:?} limit is {max}",
            name.len()
        )));
    }
    if let Some(bad) = name
        .bytes()
        .find(|&byte| !(0x21_u8..=0x7E_u8).contains(&byte))
    {
        return Err(CyberError::InvalidArgument(format!(
            "texture name {name:?} contains byte {bad:#04X}, which is not printable ASCII"
        )));
    }
    Ok(name.to_ascii_uppercase())
}

/// The standard result fragment of a map edit.
///
/// Every map editing tool merges this into its structured content:
/// `map`, `nodes_invalidated` and `copied_to_working`, exactly as the
/// [`MapEditReport`] recorded them.
#[must_use]
#[inline]
pub fn edit_result_fragment(report: &MapEditReport) -> JsonMap<String, Value> {
    [
        ("map".to_owned(), Value::String(report.map.to_string())),
        (
            "nodes_invalidated".to_owned(),
            Value::Bool(report.nodes_invalidated),
        ),
        (
            "copied_to_working".to_owned(),
            Value::Bool(report.copied_to_working),
        ),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::{from_str, json};

    use super::{
        FlagsInput, IndexSelector, check_index, edit_result_fragment, validate_texture_name,
    };
    use crate::session::map_edit::MapEditReport;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{FlagFamily, FlagKind};
    use crate::wad::name::LumpName;

    #[test]
    fn flags_input_reads_bits_or_names_from_json() {
        let bits = from_str::<FlagsInput>("260").expect("a number is the bitset");
        assert!(matches!(bits, FlagsInput::Bits(260)), "raw bits");
        let names = from_str::<FlagsInput>(r#"["twosided", "mapped"]"#).expect("a list is names");
        assert!(
            matches!(names, FlagsInput::Names(list) if list.len() == 2),
            "names"
        );
        assert!(
            from_str::<FlagsInput>("2.5").is_err(),
            "a fraction is neither bits nor names"
        );
        assert!(
            from_str::<FlagsInput>("\"twosided\"").is_err(),
            "a bare string is not a name list"
        );
    }

    #[test]
    fn flag_names_resolve_per_kind_and_family() {
        let combined = FlagsInput::Names(vec!["twosided".to_owned(), "mapped".to_owned()])
            .resolve(FlagKind::Linedef, FlagFamily::Doom)
            .expect("two Doom linedef names");
        assert_eq!(combined, 260, "4 | 256");
        let thing_bits = FlagsInput::Names(vec!["ambush".to_owned(), "friend".to_owned()])
            .resolve(FlagKind::Thing, FlagFamily::Doom)
            .expect("two Doom thing names");
        assert_eq!(thing_bits, 0x88, "8 | 128");
        let hexen = FlagsInput::Names(vec!["playeruse".to_owned()])
            .resolve(FlagKind::Linedef, FlagFamily::Hexen)
            .expect("a Hexen activation name");
        assert_eq!(hexen, 1024, "the activation field value 1 at bit 10");
        let standing = FlagsInput::Names(vec!["standing".to_owned()])
            .resolve(FlagKind::Thing, FlagFamily::Hexen)
            .expect("a Hexen thing bit");
        assert_eq!(standing, 0x4000, "the top Hexen thing bit");
        assert_eq!(
            FlagsInput::Bits(7)
                .resolve(FlagKind::Linedef, FlagFamily::Doom)
                .expect("bits pass through"),
            7,
            "bits ignore the table"
        );
    }

    #[test]
    fn unknown_flag_names_are_refused_with_the_valid_list() {
        for (kind, family) in [
            (FlagKind::Linedef, FlagFamily::Doom),
            (FlagKind::Thing, FlagFamily::Hexen),
        ] {
            let error = FlagsInput::Names(vec!["nope".to_owned()])
                .resolve(kind, family)
                .expect_err("nope is in no table");
            assert_eq!(error.code(), "invalid_argument", "unknown name code");
            let message = error.to_string();
            assert!(
                message.contains("'nope'") && message.contains("valid names are"),
                "the message names the bad flag and the table: {message}"
            );
        }
        let doom_thing = FlagsInput::Names(vec!["twosided".to_owned()])
            .resolve(FlagKind::Thing, FlagFamily::Doom)
            .expect_err("twosided is a linedef flag");
        assert!(
            doom_thing.to_string().contains("easy, medium, hard"),
            "the thing table is listed: {doom_thing}"
        );
    }

    #[test]
    fn index_selector_reads_one_or_many() {
        let single = from_str::<IndexSelector>("7").expect("a number is one index");
        assert_eq!(single.indices().expect("one"), vec![7_usize], "single");
        let many = from_str::<IndexSelector>("[3, 1, 3]").expect("a list selects many");
        assert_eq!(
            many.indices().expect("deduped"),
            vec![1_usize, 3],
            "sorted and deduplicated"
        );
        let empty = from_str::<IndexSelector>("[]").expect("an empty list parses");
        let error = empty.indices().expect_err("no indices is a refusal");
        assert_eq!(error.code(), "invalid_argument", "empty selector code");
        assert!(
            error.to_string().contains("empty"),
            "the message says what to pass: {error}"
        );
        assert!(
            from_str::<IndexSelector>("\"3\"").is_err(),
            "a string is no index"
        );
    }

    #[test]
    fn check_index_accepts_the_range_and_names_the_bounds() {
        assert!(check_index("linedef", 6, 7).is_ok(), "the last index is in");
        let error = check_index("linedef", 12, 7).expect_err("12 is out");
        assert_eq!(error.code(), "not_found", "out of range code");
        assert_eq!(
            error.to_string(),
            "linedef 12 does not exist (map has 7 linedefs)",
            "the documented phrasing"
        );
        let zero = check_index("sector", 0, 0).expect_err("an empty map has none");
        assert_eq!(
            zero.to_string(),
            "sector 0 does not exist (map has 0 sectors)",
            "the empty-map phrasing"
        );
    }

    #[test]
    fn texture_names_uppercase_and_fit_the_format() {
        assert_eq!(
            validate_texture_name("wall1", MapFormat::Doom).expect("a vanilla name"),
            "WALL1"
        );
        assert_eq!(
            validate_texture_name("a-b", MapFormat::Hexen).expect("dashes are allowed"),
            "A-B"
        );
        assert_eq!(
            validate_texture_name("brick01_metal", MapFormat::Udmf).expect("a long UDMF name"),
            "BRICK01_METAL"
        );
        let long = validate_texture_name("123456789", MapFormat::Doom).expect_err("nine is over");
        assert_eq!(long.code(), "invalid_argument", "over-long code");
        assert!(long.to_string().contains('8'), "the limit is named: {long}");
        let huge =
            validate_texture_name(&"x".repeat(100), MapFormat::Udmf).expect_err("100 is over");
        assert_eq!(huge.code(), "invalid_argument", "UDMF cap code");
        assert!(
            validate_texture_name(&"x".repeat(64), MapFormat::Udmf).is_ok(),
            "exactly 64 fits UDMF"
        );
        let spaced = validate_texture_name("bad name", MapFormat::Doom).expect_err("no spaces");
        assert_eq!(spaced.code(), "invalid_argument", "bad character code");
        let empty = validate_texture_name("", MapFormat::Doom).expect_err("empty is refused");
        assert!(
            empty.to_string().contains('"'),
            "the message suggests \"-\": {empty}"
        );
        assert_eq!(
            validate_texture_name("-", MapFormat::Doom).expect("the no-texture name"),
            "-"
        );
        assert_eq!(
            validate_texture_name("STAC_CAT", MapFormat::Udmf).expect("dashes anywhere"),
            "STAC_CAT"
        );
    }

    #[test]
    fn the_edit_fragment_carries_the_documented_keys() {
        let report = MapEditReport {
            map: LumpName::parse("MAP01").expect("a valid name"),
            copied_to_working: true,
            nodes_invalidated: false,
        };
        assert_eq!(
            json!(edit_result_fragment(&report)),
            json!({
                "map": "MAP01",
                "nodes_invalidated": false,
                "copied_to_working": true,
            }),
            "the standard fragment"
        );
        let cleared = MapEditReport {
            map: report.map,
            copied_to_working: false,
            nodes_invalidated: true,
        };
        assert_eq!(
            json!(edit_result_fragment(&cleared)),
            json!({
                "map": "MAP01",
                "nodes_invalidated": true,
                "copied_to_working": false,
            }),
            "the flags mirror the report"
        );
    }
}
