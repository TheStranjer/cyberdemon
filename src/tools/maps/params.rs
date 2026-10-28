//! Shared wire types and validators for the map editing tools.
//!
//! The `flags` and index parameter shapes ([`FlagsInput`] with the shared
//! [`EditBits`] adjustment the flag-editing tools resolve their three flag
//! parameters into, [`IndexSelector`]), the one `{x, y}` map-unit point
//! ([`PointParam`]),
//! element bounds checking ([`check_index`]), texture-name validation
//! ([`validate_texture_name`]) and the standard result fragment
//! ([`edit_result_fragment`]) every edit tool merges into its structured
//! content.
//!
//! Beside these, the small shared readings the element-editing groups'
//! refusal and reuse paths repeat: the comma-joined index list ([`join`]),
//! the changed-nothing result fragment ([`untouched_fragment`]) and the
//! exact-point vertex lookup ([`find_vertex`]).
//!
//! Beside these, the one binary field-range checker ([`fits_field`], with
//! the [`fits_coord`] and [`fits_offset`] conveniences for the `i16`
//! columns) every `edit_*` group's per-format check calls, so the shared
//! refusal message is spelled in exactly one place.
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
use crate::wad::map::consts::{COORD_MAX, COORD_MIN};
use crate::wad::map::{FlagFamily, FlagKind, Map, flags};
use crate::wad::name::{LumpName, MAX_NAME_LEN};

/// Longest texture name a `UDMF` map may carry.
const UDMF_TEXTURE_MAX: usize = 64;

/// The lowest offset an `i16` binary `SIDEDEFS` column holds (`i16::MIN`).
pub const OFFSET_MIN: i32 = -0x8000_i32;
/// The highest offset an `i16` binary `SIDEDEFS` column holds (`i16::MAX`).
pub const OFFSET_MAX: i32 = 0x7FFF_i32;

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

/// The `flags`, `set_flags` and `clear_flags` parameters of one `edit_*`
/// call, exactly as parsed from the wire and waiting to be resolved into
/// [`EditBits`].
///
/// The three travel as one bundle so [`EditBits::resolve`] keeps its kind
/// and family selectors inside the crate's four-parameter budget.
pub struct FlagInputs {
    /// The whole-replacement `flags` parameter, `None` when absent.
    pub flags: Option<FlagsInput>,
    /// The `set_flags` parameter, `None` when absent.
    pub set_flags: Option<FlagsInput>,
    /// The `clear_flags` parameter, `None` when absent.
    pub clear_flags: Option<FlagsInput>,
}

/// The resolved flag adjustments of one `edit_*` call: the three
/// `flags`/`set_flags`/`clear_flags` parameters as bits.
///
/// The `edit_thing` and `edit_linedef` tools share this one shape; each
/// keeps its own policy (like the linedef's flag-given check) beside it.
#[derive(Debug)]
pub struct EditBits {
    /// Whole-replacement bits from `flags`, `None` when it was absent.
    pub absolute: Option<u32>,
    /// Bits from `set_flags`.
    pub set: u32,
    /// Bits from `clear_flags`.
    pub clear: u32,
}

impl EditBits {
    /// Resolves the three flag parameters of one edit call through
    /// [`FlagsInput::resolve`] for `kind` and `family`.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when a name is not in the table.
    #[inline]
    pub fn resolve(
        given: FlagInputs,
        kind: FlagKind,
        family: FlagFamily,
    ) -> Result<Self, CyberError> {
        Ok(Self {
            absolute: given
                .flags
                .map(|input| input.resolve(kind, family))
                .transpose()?,
            set: given
                .set_flags
                .map_or_else(|| Ok(0_u32), |input| input.resolve(kind, family))?,
            clear: given
                .clear_flags
                .map_or_else(|| Ok(0_u32), |input| input.resolve(kind, family))?,
        })
    }

    /// The flags after the edit: an absolute bitset replaces everything;
    /// otherwise the `clear` bits go and the `set` bits go on (a bit named
    /// for both ends up set).
    #[must_use]
    #[inline]
    pub const fn apply(&self, current: u32) -> u32 {
        match self.absolute {
            Some(bits) => bits,
            None => (current & !self.clear) | self.set,
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
    /// Takes the selector by reference, so every edit tool reads its index
    /// parameter without cloning it first. The one copy happens here, where
    /// the answer needs the list as its own: binding a field out of a
    /// `&Self` would need a pattern of a different kind than the expression
    /// it matches, which this crate's denied `pattern_type_mismatch` and
    /// `ref_patterns` forbid.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when the list is empty: an edit
    /// over no elements is never what the caller meant.
    #[inline]
    pub fn indices(&self) -> Result<Vec<usize>, CyberError> {
        let mut indices = match self.clone() {
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

/// One `{x, y}` point parameter in map units: the endpoint form of an
/// added or split linedef, a drawn loop's corner, a feature's `at` point,
/// a transform `origin` and a minimap `center` are all this same shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
pub struct PointParam {
    /// `X` coordinate in map units.
    pub x: i32,
    /// `Y` coordinate in map units.
    pub y: i32,
}

impl PointParam {
    /// The point as the shared wide [`geom::Pt`](crate::geom::Pt) grid,
    /// for the math that runs in `i64`.
    #[must_use]
    #[inline]
    pub fn to_wide(self) -> crate::geom::Pt {
        (i64::from(self.x), i64::from(self.y))
    }
}

/// Refuses an element `index` outside `0..len` of one `kind` of map
/// element.
///
/// This is [`crate::wad::map::query::check_index`], the one index guard
/// every tool and pure engine shares, re-exported for the editing tools'
/// import path.
pub use crate::wad::map::query::{ElementKind, check_index};

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

/// The edit fragment of a call that changed nothing: the map's name with
/// both flags `false`.
#[must_use]
#[inline]
pub fn untouched_fragment(name: LumpName) -> JsonMap<String, Value> {
    edit_result_fragment(&MapEditReport {
        map: name,
        copied_to_working: false,
        nodes_invalidated: false,
    })
}

/// The index of the first vertex sitting exactly on `x`/`y`.
#[must_use]
#[inline]
pub fn find_vertex(map: &Map, x: i32, y: i32) -> Option<usize> {
    map.vertices
        .iter()
        .position(|vertex| vertex.x == x && vertex.y == y)
}

/// The indices as one comma-separated list.
#[must_use]
#[inline]
pub fn join(indices: &[usize]) -> String {
    indices
        .iter()
        .map(usize::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

/// Refuses unless `value` sits inside the binary record field's
/// `min..=max` range.
///
/// This is the one range checker every `edit_*` group's format check calls
/// (each keeps its own `check_binary_*` wrapper because only the wrapper
/// knows which `MapFormat` needs the check at all).
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the field, the value and the
/// range.
#[inline]
pub fn fits_field(name: &str, value: i32, min: i32, max: i32) -> Result<(), CyberError> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{name} {value} is out of range for the binary map format: the field holds {min}..={max}"
    )))
}

/// Refuses unless `value` sits inside the `i16` range of a binary
/// `VERTEXES` column.
///
/// # Errors
///
/// As [`fits_field`].
#[inline]
pub fn fits_coord(name: &str, value: i32) -> Result<(), CyberError> {
    fits_field(name, value, COORD_MIN, COORD_MAX)
}

/// Refuses unless `value` sits inside the `i16` range of a binary
/// `SIDEDEFS` offset column.
///
/// # Errors
///
/// As [`fits_field`].
#[inline]
pub fn fits_offset(name: &str, value: i32) -> Result<(), CyberError> {
    fits_field(name, value, OFFSET_MIN, OFFSET_MAX)
}

#[cfg(test)]
mod tests {
    use serde_json::{from_str, json};

    use super::{
        COORD_MAX, COORD_MIN, EditBits, ElementKind, FlagInputs, FlagsInput, IndexSelector,
        OFFSET_MAX, OFFSET_MIN, PointParam, check_index, edit_result_fragment, find_vertex,
        fits_coord, fits_field, fits_offset, join, untouched_fragment, validate_texture_name,
    };
    use crate::session::map_edit::MapEditReport;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{FlagFamily, FlagKind, Map, Vertex};
    use crate::wad::name::LumpName;

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

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

    /// The shared edit `flags`/`set_flags`/`clear_flags` parameter bundle.
    fn given(
        flags: Option<FlagsInput>,
        set_flags: Option<FlagsInput>,
        clear_flags: Option<FlagsInput>,
    ) -> FlagInputs {
        FlagInputs {
            flags,
            set_flags,
            clear_flags,
        }
    }

    /// One name as a [`FlagsInput::Names`] parameter value.
    fn name(one: &str) -> FlagsInput {
        FlagsInput::Names(vec![one.to_owned()])
    }

    #[test]
    fn edit_bits_absolute_replaces_all_and_adjusts_clear_then_set() {
        let replacing = EditBits {
            absolute: Some(3_u32),
            set: 8_u32,
            clear: 1_u32,
        };
        assert_eq!(replacing.apply(7_u32), 3_u32, "the absolute bitset wins");
        let adjusting = EditBits {
            absolute: None,
            set: 8_u32,
            clear: 1_u32,
        };
        assert_eq!(
            adjusting.apply(7_u32),
            14_u32,
            "the clear bit goes, the set bit comes"
        );
        assert_eq!(
            adjusting.apply(0_u32),
            8_u32,
            "clearing an absent bit changes nothing"
        );
        let both = EditBits {
            absolute: None,
            set: 2_u32,
            clear: 2_u32,
        };
        assert_eq!(both.apply(2_u32), 2_u32, "a bit in both ends stays set");
        assert_eq!(both.apply(0_u32), 2_u32, "and comes on when it was off");
        let untouched = EditBits {
            absolute: None,
            set: 0_u32,
            clear: 0_u32,
        };
        assert_eq!(
            untouched.apply(42_u32),
            42_u32,
            "no adjustments keep the flags"
        );
    }

    #[test]
    fn edit_bits_resolve_reads_the_three_parameters_in_order() {
        let bits = EditBits::resolve(
            given(
                Some(FlagsInput::Bits(9)),
                Some(FlagsInput::Bits(8)),
                Some(FlagsInput::Bits(1)),
            ),
            FlagKind::Linedef,
            FlagFamily::Doom,
        )
        .expect("raw bitsets always resolve");
        assert_eq!(bits.absolute, Some(9_u32), "the first is the replacement");
        assert_eq!(bits.set, 8_u32, "the second sets");
        assert_eq!(bits.clear, 1_u32, "the third clears");
        let empty = EditBits::resolve(given(None, None, None), FlagKind::Thing, FlagFamily::Hexen)
            .expect("three absent parameters resolve");
        assert_eq!(
            (empty.absolute, empty.set, empty.clear),
            (None, 0_u32, 0_u32),
            "absent means no replacement and no bits"
        );
    }

    #[test]
    fn edit_bits_refuses_unknown_names_with_the_kind_and_family_list() {
        let error = EditBits::resolve(
            given(None, Some(name("nope")), None),
            FlagKind::Linedef,
            FlagFamily::Doom,
        )
        .expect_err("nope is in no table");
        assert_eq!(error.code(), "invalid_argument", "unknown set name code");
        assert!(
            error.to_string().contains("valid names are"),
            "the refusal lists the linedef names: {error}"
        );
        let clear_bad = EditBits::resolve(
            given(None, None, Some(name("twosided"))),
            FlagKind::Thing,
            FlagFamily::Doom,
        )
        .expect_err("twosided is no thing flag");
        assert!(
            clear_bad.to_string().contains("easy, medium, hard"),
            "the thing table is listed: {clear_bad}"
        );
        let absolute_bad = EditBits::resolve(
            given(Some(name("standing")), None, None),
            FlagKind::Thing,
            FlagFamily::Doom,
        )
        .expect_err("standing is a Hexen thing bit");
        assert!(
            absolute_bad.to_string().contains("'standing'"),
            "the refusal names the bad flag: {absolute_bad}"
        );
    }

    #[test]
    fn edit_bits_names_resolve_per_kind_and_family() {
        let line_doom = EditBits::resolve(
            given(None, Some(name("mapped")), Some(name("twosided"))),
            FlagKind::Linedef,
            FlagFamily::Doom,
        )
        .expect("Doom linedef names");
        assert_eq!(line_doom.set, 256, "mapped is bit 8 of 256");
        assert_eq!(line_doom.clear, 4, "twosided is bit 4");
        let line_hexen = EditBits::resolve(
            given(None, Some(name("playeruse")), None),
            FlagKind::Linedef,
            FlagFamily::Hexen,
        )
        .expect("Hexen linedef activation name");
        assert_eq!(
            line_hexen.set, 1024,
            "the activation field value 1 at bit 10"
        );
        let thing_doom = EditBits::resolve(
            given(None, Some(name("friend")), Some(name("ambush"))),
            FlagKind::Thing,
            FlagFamily::Doom,
        )
        .expect("Doom thing names");
        assert_eq!(thing_doom.set, 128, "friend is the top Doom bit");
        assert_eq!(thing_doom.clear, 8, "ambush is bit 8");
        let thing_hexen = EditBits::resolve(
            given(Some(name("standing")), None, None),
            FlagKind::Thing,
            FlagFamily::Hexen,
        )
        .expect("Hexen thing name");
        assert_eq!(thing_hexen.absolute, Some(0x4000), "the top Hexen bit");
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
    fn point_params_read_xy_and_widen_for_i64_math() {
        let point = from_str::<PointParam>(r#"{"x": -5, "y": 64}"#).expect("an object is a point");
        assert_eq!((point.x, point.y), (-5_i32, 64_i32), "both coordinates");
        assert_eq!(point.to_wide(), (-5_i64, 64_i64), "the wide form");
        let round = PointParam { x: 8, y: -8 };
        let copied = round;
        assert_eq!(round, copied, "Copy and PartialEq round-trip");
        assert!(
            from_str::<PointParam>(r#"{"x": "0", "y": 1}"#).is_err(),
            "a string is no coordinate"
        );
    }

    #[test]
    fn check_index_refuses_with_the_spelled_plural_and_the_index_hint() {
        assert!(
            check_index(ElementKind::Linedefs, 6, 7).is_ok(),
            "the last index is in"
        );
        let error = check_index(ElementKind::Vertices, 40, 6).expect_err("40 is out");
        assert_eq!(error.code(), "not_found", "out of range code");
        assert_eq!(
            error.message(),
            "vertex 40 does not exist (the map has 6 vertices, valid indices are 0..=5)",
            "vertices, never vertexs"
        );
        assert_eq!(
            error.hint(),
            "describe_map or find_elements returns indices",
            "the hint names the index-listing tools"
        );
        let zero = check_index(ElementKind::Vertices, 0, 0).expect_err("an empty map has none");
        assert_eq!(
            zero.message(),
            "vertex 0 does not exist (the map has no vertices)",
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

    #[test]
    fn join_lists_no_one_or_many_indices() {
        assert_eq!(join(&[]), "", "the empty list joins to nothing");
        assert_eq!(join(&[4_usize]), "4", "one index stands alone");
        assert_eq!(
            join(&[4_usize, 7, 12]),
            "4, 7, 12",
            "many are comma-separated in order"
        );
    }

    #[test]
    fn the_untouched_fragment_names_the_map_with_both_flags_false() {
        let name = LumpName::parse("MAP01").expect("a valid name");
        assert_eq!(
            json!(untouched_fragment(name)),
            json!({
                "map": "MAP01",
                "nodes_invalidated": false,
                "copied_to_working": false,
            }),
            "a call that changed nothing reports the name alone"
        );
    }

    #[test]
    fn find_vertex_matches_the_exact_point_and_the_first_one() {
        let map = Map {
            vertices: vec![vertex(0, 0), vertex(256, 0), vertex(0, 0)],
            ..Default::default()
        };
        assert_eq!(
            find_vertex(&map, 256_i32, 0_i32),
            Some(1_usize),
            "an exact hit answers its index"
        );
        assert_eq!(
            find_vertex(&map, 0_i32, 0_i32),
            Some(0_usize),
            "of two matches the first wins"
        );
        assert_eq!(
            find_vertex(&map, 0_i32, 256_i32),
            None,
            "one axis off is another point"
        );
        assert_eq!(
            find_vertex(&Map::default(), 0_i32, 0_i32),
            None,
            "an empty list is a miss"
        );
    }

    #[test]
    fn fits_field_accepts_both_endpoints_and_refuses_outside_naming_the_field() {
        assert!(
            fits_field("type", 0_i32, 0_i32, 0xFFFF_i32).is_ok(),
            "the low end"
        );
        assert!(
            fits_field("type", 0xFFFF_i32, 0_i32, 0xFFFF_i32).is_ok(),
            "the high end"
        );
        let over = fits_field("type", 70_000_i32, 0_i32, 0xFFFF_i32).expect_err("above refuses");
        assert_eq!(over.code(), "invalid_argument", "above code");
        assert_eq!(
            over.message(),
            "type 70000 is out of range for the binary map format: the field holds 0..=65535",
            "the refusal names the field, value and range"
        );
        let under = fits_field("arg", -1_i32, 0_i32, 255_i32).expect_err("below refuses");
        assert_eq!(under.code(), "invalid_argument", "below code");
        assert_eq!(
            under.message(),
            "arg -1 is out of range for the binary map format: the field holds 0..=255",
            "the refusal spells a negative value"
        );
    }

    #[test]
    fn fits_coord_spells_the_vertexes_i16_range_at_both_edges() {
        assert!(fits_coord("x", COORD_MIN).is_ok(), "i16::MIN fits");
        assert!(fits_coord("y", COORD_MAX).is_ok(), "i16::MAX fits");
        let over = fits_coord("x", COORD_MAX.saturating_add(1)).expect_err("above i16 refuses");
        assert_eq!(
            over.message(),
            "x 32768 is out of range for the binary map format: the field holds -32768..=32767",
            "the refusal names the field and the i16 range"
        );
        let under = fits_coord("y", COORD_MIN.saturating_sub(1)).expect_err("below i16 refuses");
        assert_eq!(
            under.message(),
            "y -32769 is out of range for the binary map format: the field holds -32768..=32767",
            "the lower side refuses too"
        );
    }

    #[test]
    fn fits_offset_spells_the_sidedefs_i16_range_at_both_edges() {
        assert!(fits_offset("x_offset", OFFSET_MIN).is_ok(), "i16::MIN fits");
        assert!(fits_offset("y_offset", OFFSET_MAX).is_ok(), "i16::MAX fits");
        let over =
            fits_offset("y_offset", OFFSET_MAX.saturating_add(1)).expect_err("above i16 refuses");
        assert_eq!(
            over.message(),
            "y_offset 32768 is out of range for the binary map format: the field holds -32768..=32767",
            "the refusal names the field and the i16 range"
        );
        let under =
            fits_offset("x_offset", OFFSET_MIN.saturating_sub(1)).expect_err("below i16 refuses");
        assert_eq!(
            under.message(),
            "x_offset -32769 is out of range for the binary map format: the field holds -32768..=32767",
            "the lower side refuses too"
        );
    }
}
