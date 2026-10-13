//! Directory browsing tools: [`Cyberdemon::list_lumps`] and
//! [`Cyberdemon::search_lumps`], built on the shared rows in
//! [`crate::tools::lumps::rows`].

mod rows;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::{fail, ok};
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::LumpName;

/// Default page size for both tools.
const DEFAULT_LIMIT: usize = 200;
/// Largest page either tool will return.
const MAX_LIMIT: usize = 2000;

/// Arguments for [`Cyberdemon::list_lumps`].
#[derive(Debug, Deserialize, JsonSchema)]
struct ListLumpsParams {
    /// Which archive to list: `"merged"` (the default: the directory an
    /// engine sees), `"working"` (the editable archive alone) or
    /// `"layer:N"` (read-only layer `N`, 0 is the IWAD).
    #[serde(default)]
    scope: Option<String>,
    /// Keep only lumps inside this namespace region: `sprites`, `flats`,
    /// `patches`, `colormaps`, `acs`, `voices`, `hires` or `textures`.
    #[serde(default)]
    namespace: Option<String>,
    /// Keep only lumps with this type hint, e.g. `map_marker`, `map_data`,
    /// `flat`, `sprite`, `patch`, `palette`, `doom_sound` (every value is
    /// listed in `skill/reference/lumps.md`).
    #[serde(default)]
    kind: Option<String>,
    /// Keep only the header and data lumps of this map block, e.g.
    /// `"MAP01"`.
    #[serde(default)]
    map: Option<String>,
    /// Matching lumps to skip before the page (default 0).
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum rows in the result (default 200, at most 2000).
    #[serde(default)]
    limit: Option<usize>,
}

/// Arguments for [`Cyberdemon::search_lumps`]. At least one of `pattern`,
/// `kind`, `namespace`, `contains_text` or `contains_hex` is required.
#[derive(Debug, Deserialize, JsonSchema)]
struct SearchLumpsParams {
    /// Case-insensitive lump-name glob: `*` matches any run of characters,
    /// `?` exactly one, e.g. `"FLOOR*"` or `"?LOOR1"`.
    #[serde(default)]
    pattern: Option<String>,
    /// Keep only lumps with this type hint, e.g. `palette`, `flat`,
    /// `doom_sound` (every value is listed in
    /// `skill/reference/lumps.md`).
    #[serde(default)]
    kind: Option<String>,
    /// Keep only lumps inside this namespace region: `sprites`, `flats`,
    /// `patches`, `colormaps`, `acs`, `voices`, `hires` or `textures`.
    #[serde(default)]
    namespace: Option<String>,
    /// Keep only lumps whose payload contains this text, matched
    /// case-insensitively on `ASCII` bytes.
    #[serde(default)]
    contains_text: Option<String>,
    /// Keep only lumps whose payload contains these bytes: a hex string
    /// with optional spaces, e.g. `"4D 55 53 1A"`.
    #[serde(default)]
    contains_hex: Option<String>,
    /// Which archive to search: `"merged"` (the default), `"working"` or
    /// `"layer:N"`.
    #[serde(default)]
    scope: Option<String>,
    /// Maximum rows in the result (default 200, at most 2000); `truncated`
    /// says whether more matched.
    #[serde(default)]
    limit: Option<usize>,
}

/// The parsed filters and paging of one `list_lumps` call.
struct Listing {
    /// Archive to list.
    scope: Scope,
    /// Namespace to keep, if any.
    namespace: Option<Namespace>,
    /// Type hint to keep, if any.
    kind: Option<LumpKind>,
    /// Map block to keep, if any.
    map: Option<LumpName>,
    /// Matching rows to skip.
    offset: usize,
    /// Largest page to return.
    limit: usize,
}

impl Listing {
    /// Whether a row passes every filter that was set.
    fn accepts(&self, row: &LumpRow) -> bool {
        self.namespace
            .is_none_or(|wanted| row.namespace == Some(wanted))
            && self.kind.is_none_or(|wanted| row.kind == wanted)
            && self.map.is_none_or(|wanted| row.map == Some(wanted))
    }
}

/// One content needle of a [`Search`], with its case rule.
struct Needle<'crit> {
    /// The bytes to look for.
    bytes: &'crit [u8],
    /// Whether the comparison folds `ASCII` case on both sides.
    ignore_case: bool,
}

/// The parsed criteria of one `search_lumps` call.
struct Search {
    /// Case-insensitive name glob, if given.
    pattern: Option<String>,
    /// Type hint to keep, if given.
    kind: Option<LumpKind>,
    /// Namespace to keep, if given.
    namespace: Option<Namespace>,
    /// Upper-cased text needle, if given.
    text: Option<Vec<u8>>,
    /// Needle bytes decoded from the hex string, if given.
    hex: Option<Vec<u8>>,
}

/// How [`Search::judge`] decided about one row.
enum Match {
    /// The row matches; the payload offset of the first content match,
    /// `None` when no content criterion was given.
    Hit(Option<usize>),
    /// The row fails some criterion and is dropped.
    Miss,
}

impl Search {
    /// Whether the caller gave no criterion at all.
    const fn is_empty(&self) -> bool {
        self.pattern.is_none()
            && self.kind.is_none()
            && self.namespace.is_none()
            && self.text.is_none()
            && self.hex.is_none()
    }

    /// Judge one row against every criterion of this search.
    fn judge(&self, row: &LumpRow) -> Match {
        if self
            .pattern
            .as_deref()
            .is_some_and(|glob| !row.name.matches_glob(glob))
        {
            return Match::Miss;
        }
        if self.kind.is_some_and(|kind| row.kind != kind)
            || self
                .namespace
                .is_some_and(|namespace| row.namespace != Some(namespace))
        {
            return Match::Miss;
        }
        self.content_match(row)
    }

    /// The verdict of the content criteria for one row's payload: every
    /// needle must occur, and the lowest offset among them wins.
    fn content_match(&self, row: &LumpRow) -> Match {
        let offsets: Option<Vec<usize>> = self
            .needles()
            .map(|needle| find_needle(&row.lump.data, &needle))
            .collect();
        offsets.map_or(Match::Miss, |positions| {
            Match::Hit(positions.into_iter().min())
        })
    }

    /// The content needles to look for, text first, each a borrow of this.
    fn needles(&self) -> impl Iterator<Item = Needle<'_>> {
        let text = self.text.as_deref().map(|bytes| Needle {
            bytes,
            ignore_case: true,
        });
        let hex = self.hex.as_deref().map(|bytes| Needle {
            bytes,
            ignore_case: false,
        });
        text.into_iter().chain(hex)
    }
}

/// The first offset in `data` where `needle` occurs, `None` when it does
/// not. An empty needle matches at offset 0.
fn find_needle(data: &[u8], needle: &Needle<'_>) -> Option<usize> {
    if needle.bytes.is_empty() {
        return Some(0);
    }
    if needle.ignore_case {
        let folded = data.to_ascii_uppercase();
        return folded
            .windows(needle.bytes.len())
            .position(|window| window == needle.bytes);
    }
    data.windows(needle.bytes.len())
        .position(|window| window == needle.bytes)
}

/// The value of one hex digit, `None` for any other character.
fn hex_digit(ch: char) -> Option<u8> {
    ch.to_digit(16).and_then(|value| u8::try_from(value).ok())
}

/// The byte two hex characters spell, `None` unless the slice is exactly
/// two characters and both are hex digits.
fn parse_pair(pair: &[char]) -> Option<u8> {
    let &[high, low] = pair else {
        return None;
    };
    let (Some(h), Some(l)) = (hex_digit(high), hex_digit(low)) else {
        return None;
    };
    h.checked_mul(16)?.checked_add(l)
}

/// Structured result of [`Cyberdemon::list_lumps`].
#[derive(Serialize)]
struct ListReport {
    /// The scope that was listed.
    scope: Scope,
    /// Lumps matching the filters, before paging.
    total: usize,
    /// Rows skipped before this page.
    offset: usize,
    /// Rows in this page.
    count: usize,
    /// The page, in directory order.
    lumps: Vec<LumpRow>,
}

/// One `search_lumps` hit: the shared row plus the content match offset.
#[derive(Serialize)]
struct SearchRow {
    /// The directory row, flattened into the object.
    #[serde(flatten)]
    row: LumpRow,
    /// Byte offset of the first `contains_text`/`contains_hex` match
    /// (lowest across both needles), `null` when neither was given.
    match_offset: Option<usize>,
}

/// Structured result of [`Cyberdemon::search_lumps`].
#[derive(Serialize)]
struct SearchReport {
    /// The scope that was searched.
    scope: Scope,
    /// Rows in the result.
    count: usize,
    /// Whether more lumps matched than `limit` allowed.
    truncated: bool,
    /// The hits, in directory order, capped at `limit`.
    lumps: Vec<SearchRow>,
}

/// The accepted `scope` text, the merged view being the default.
///
/// # Errors
///
/// [`Scope::parse`]'s [`CyberError::InvalidArgument`] for any other name.
fn parse_scope(text: Option<&str>) -> Result<Scope, CyberError> {
    text.map_or(Ok(Scope::Merged), Scope::parse)
}

/// Every `Namespace` name, for error hints.
fn namespace_names() -> String {
    Namespace::ALL
        .iter()
        .map(|namespace| namespace.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every `LumpKind` name, for error hints.
fn kind_names() -> String {
    LumpKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The namespace a filter keeps, [`Namespace::parse`] being
/// case-insensitive.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
fn parse_namespace(text: Option<&str>) -> Result<Option<Namespace>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    Namespace::parse(raw).map(Some).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "unknown namespace {raw:?}: expected one of {names}",
            names = namespace_names()
        ))
    })
}

/// The type hint a filter keeps, matched case-insensitively against
/// [`LumpKind::as_str`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
fn parse_kind(text: Option<&str>) -> Result<Option<LumpKind>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    LumpKind::parse(raw).map(Some).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "unknown kind {raw:?}: expected one of {names}",
            names = kind_names()
        ))
    })
}

/// The map block a filter keeps, parsed strictly as a lump name.
///
/// # Errors
///
/// [`LumpName::parse`]'s [`CyberError::InvalidArgument`] for a name that is
/// not 1 to 8 printable `ASCII` characters.
fn parse_map(text: Option<&str>) -> Result<Option<LumpName>, CyberError> {
    text.map(LumpName::parse).transpose()
}

/// The page size to apply, the default being [`DEFAULT_LIMIT`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `0` or anything above
/// [`MAX_LIMIT`].
fn parse_limit(value: Option<usize>) -> Result<usize, CyberError> {
    let Some(limit) = value else {
        return Ok(DEFAULT_LIMIT);
    };
    if limit == 0 || limit > MAX_LIMIT {
        return Err(CyberError::InvalidArgument(format!(
            "limit {limit} is out of range: pass a value between 1 and {MAX_LIMIT}"
        )));
    }
    Ok(limit)
}

/// The needle bytes of a `contains_hex` string: hex digits in pairs, with
/// any whitespace between them ignored.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a non-hex character or an odd
/// number of digits.
fn parse_hex(text: Option<&str>) -> Result<Option<Vec<u8>>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    let digits: Vec<char> = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
    let mut bytes = Vec::new();
    let mut pairs = digits.chunks_exact(2);
    for pair in pairs.by_ref() {
        let Some(byte) = parse_pair(pair) else {
            return Err(CyberError::InvalidArgument(format!(
                "contains_hex {raw:?} holds {pair:?}, which is not a hex pair (use 0-9 and a-f)"
            )));
        };
        bytes.push(byte);
    }
    if let Some(left) = pairs.remainder().first() {
        return Err(CyberError::InvalidArgument(format!(
            "contains_hex {raw:?} holds an odd number of hex digits ({left:?} is left over): \
             write whole bytes such as \"4D 55 53 1A\""
        )));
    }
    Ok(Some(bytes))
}

/// The `list_lumps` filters and paging, parsed from its arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad `scope`, `namespace`, `kind`,
/// `map` or `limit`.
fn parse_listing(args: &ListLumpsParams) -> Result<Listing, CyberError> {
    Ok(Listing {
        scope: parse_scope(args.scope.as_deref())?,
        namespace: parse_namespace(args.namespace.as_deref())?,
        kind: parse_kind(args.kind.as_deref())?,
        map: parse_map(args.map.as_deref())?,
        offset: args.offset.unwrap_or(0_usize),
        limit: parse_limit(args.limit)?,
    })
}

/// List, filter and page the scope's rows.
fn list_run(session: &Session, listing: &Listing) -> Result<ListReport, CyberError> {
    let rows = build_rows(session, listing.scope)?;
    let matched: Vec<LumpRow> = rows
        .into_iter()
        .filter(|row| listing.accepts(row))
        .collect();
    let total = matched.len();
    let lumps: Vec<LumpRow> = matched
        .into_iter()
        .skip(listing.offset)
        .take(listing.limit)
        .collect();
    Ok(ListReport {
        scope: listing.scope,
        total,
        offset: listing.offset,
        count: lumps.len(),
        lumps,
    })
}

/// The `search_lumps` scope, paging and criteria.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad `scope`, `kind`, `namespace`,
/// `contains_hex` or `limit`, and when no criterion was given.
fn parse_search(args: &SearchLumpsParams) -> Result<(Scope, usize, Search), CyberError> {
    let search = Search {
        pattern: args.pattern.clone(),
        kind: parse_kind(args.kind.as_deref())?,
        namespace: parse_namespace(args.namespace.as_deref())?,
        text: args
            .contains_text
            .as_deref()
            .map(|needle| needle.as_bytes().to_ascii_uppercase()),
        hex: parse_hex(args.contains_hex.as_deref())?,
    };
    if search.is_empty() {
        return Err(CyberError::InvalidArgument(
            "search_lumps needs at least one criterion: pass pattern, kind, namespace, \
             contains_text or contains_hex"
                .to_owned(),
        ));
    }
    Ok((
        parse_scope(args.scope.as_deref())?,
        parse_limit(args.limit)?,
        search,
    ))
}

/// Judge every row of the scope and keep up to `limit` hits.
fn search_run(
    session: &Session,
    scope: Scope,
    limit: usize,
    search: &Search,
) -> Result<SearchReport, CyberError> {
    let rows = build_rows(session, scope)?;
    let hits: Vec<SearchRow> = rows
        .into_iter()
        .filter_map(|row| match search.judge(&row) {
            Match::Hit(match_offset) => Some(SearchRow { row, match_offset }),
            Match::Miss => None,
        })
        .collect();
    let truncated = hits.len() > limit;
    let lumps: Vec<SearchRow> = hits.into_iter().take(limit).collect();
    Ok(SearchReport {
        scope,
        count: lumps.len(),
        truncated,
        lumps,
    })
}

/// Hand an outcome to the client as the documented structured result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = lumps_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Lists one scope's lump directory with filters and paging.
    #[tool(
        description = "Lists a scope's lumps in directory order. scope is \"merged\" (the default: what an engine sees), \"working\" or \"layer:N\" (0 is the IWAD). Optional filters: namespace (sprites, flats, patches, colormaps, acs, voices, hires, textures), kind (a LumpKind type hint such as map_marker, flat, sprite, patch, palette, doom_sound), map (only that map block's lumps, e.g. \"MAP01\"). Paging: offset (default 0), limit (default 200, max 2000). Returns {scope, total, offset, count, lumps:[{index, name, size, offset, kind, namespace, map, source}]} where index counts positions in the listed scope and offset is the byte position of the lump's data inside its source archive as save_wad would lay it out."
    )]
    fn list_lumps(
        &self,
        Parameters(args): Parameters<ListLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let listing = match parse_listing(&args) {
            Ok(listing) => listing,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| list_run(session, &listing));
        reported(report)
    }

    /// Searches a scope's directory by name, type, namespace or content.
    #[tool(
        description = "Searches a scope's lumps (scope: \"merged\" default, \"working\" or \"layer:N\"); give at least one criterion. pattern is a case-insensitive name glob (* and ?), kind and namespace filter like list_lumps, contains_text is a case-insensitive ASCII substring of the payload and contains_hex is a byte substring written as hex, spaces allowed (e.g. \"4D 55 53 1A\"); all criteria combine with AND. Returns {scope, count, truncated, lumps:[row + match_offset]} where row is list_lumps' shape and match_offset is the first content-match byte offset (null when only name/kind/namespace was searched). limit defaults to 200, max 2000; truncated says more matched."
    )]
    fn search_lumps(
        &self,
        Parameters(args): Parameters<SearchLumpsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (scope, limit, search) = match parse_search(&args) {
            Ok(parsed) => parsed,
            Err(error) => return fail(&error),
        };
        let report = self
            .state()
            .with_session(|session| search_run(session, scope, limit, &search));
        reported(report)
    }
}

#[cfg(test)]
mod tests {
    use crate::error::CyberError;
    use crate::session::{Scope, Source};
    use crate::tools::lumps::rows::LumpRow;
    use crate::tools::lumps::{
        Match, Needle, Search, find_needle, kind_names, namespace_names, parse_hex, parse_kind,
        parse_limit, parse_map, parse_namespace, parse_pair, parse_scope,
    };
    use crate::wad::archive::Lump;
    use crate::wad::classify::{LumpKind, Namespace};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A row carrying `data` for judging, everything else filled in.
    fn row(text: &str, kind: LumpKind, namespace: Option<Namespace>, data: &[u8]) -> LumpRow {
        LumpRow {
            index: 0,
            name: name(text),
            size: data.len(),
            offset: 12,
            kind,
            namespace,
            map: None,
            source: Source::Working,
            lump: Lump::new(name(text), data.to_vec()),
        }
    }

    /// A `Search` with only a name glob.
    fn glob_search(pattern: &str) -> Search {
        Search {
            pattern: Some(pattern.to_owned()),
            kind: None,
            namespace: None,
            text: None,
            hex: None,
        }
    }

    /// The error code of a failing [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    #[test]
    fn scope_defaults_to_merged_and_parses_every_form() {
        assert_eq!(parse_scope(None).expect("default"), Scope::Merged);
        assert_eq!(
            parse_scope(Some("working")).expect("working"),
            Scope::Working
        );
        assert_eq!(
            parse_scope(Some("layer:1")).expect("layer"),
            Scope::Layer(1)
        );
        let error = parse_scope(Some("bogus")).expect_err("not a scope");
        assert_eq!(code(&error), "invalid_argument", "bad scope code");
    }

    #[test]
    fn namespace_and_kind_filters_parse_names_or_refuse() {
        assert_eq!(parse_namespace(None).expect("absent"), None);
        assert_eq!(
            parse_namespace(Some("Flats")).expect("flats"),
            Some(Namespace::Flats)
        );
        let error = parse_namespace(Some("nope")).expect_err("not a namespace");
        assert_eq!(code(&error), "invalid_argument", "bad namespace code");
        assert!(
            error.to_string().contains("sprites"),
            "the hint lists the names: {error}"
        );
        assert_eq!(
            parse_kind(Some("DOOM_SOUND")).expect("kind"),
            Some(LumpKind::DoomSound)
        );
        let kind = parse_kind(Some("nope")).expect_err("not a kind");
        assert_eq!(code(&kind), "invalid_argument", "bad kind code");
        assert!(
            kind.to_string().contains("map_marker"),
            "the hint lists the kinds: {kind}"
        );
    }

    #[test]
    fn map_filter_parses_lump_names_strictly() {
        assert_eq!(parse_map(None).expect("absent"), None);
        assert_eq!(
            parse_map(Some("map01")).expect("parsed"),
            Some(name("MAP01")),
            "the name parser uppercases"
        );
        let error = parse_map(Some("TOO LONG FOR A NAME")).expect_err("over eight bytes");
        assert_eq!(code(&error), "invalid_argument", "bad map name code");
    }

    #[test]
    fn limit_defaults_and_stays_in_range() {
        assert_eq!(
            parse_limit(None).expect("default"),
            200,
            "documented default"
        );
        assert_eq!(parse_limit(Some(2000)).expect("max"), 2000, "the cap");
        assert_eq!(parse_limit(Some(1)).expect("min"), 1, "the floor");
        for bad in [0_usize, 2001] {
            let error = parse_limit(Some(bad)).expect_err("out of range");
            assert_eq!(code(&error), "invalid_argument", "limit {bad} code");
        }
    }

    #[test]
    fn hex_needles_accept_digits_with_spaces_and_refuse_the_rest() {
        assert_eq!(parse_hex(None).expect("absent"), None);
        assert_eq!(
            parse_hex(Some("4D 55 53 1A")).expect("spaced"),
            Some(vec![0x4D_u8, 0x55, 0x53, 0x1A])
        );
        assert_eq!(
            parse_hex(Some("4d55")).expect("lowercase"),
            Some(vec![0x4D_u8, 0x55])
        );
        let odd = parse_hex(Some("4D5")).expect_err("odd digits");
        assert_eq!(code(&odd), "invalid_argument", "odd hex code");
        assert!(
            odd.to_string().contains("odd"),
            "the message says odd: {odd}"
        );
        let bad = parse_hex(Some("0G")).expect_err("not hex");
        assert_eq!(code(&bad), "invalid_argument", "non-hex code");
        assert!(
            bad.to_string().contains("'G'"),
            "the message names the digit: {bad}"
        );
    }

    #[test]
    fn pair_parsing_is_strict() {
        assert_eq!(parse_pair(&['4', 'd']), Some(0x4D), "mixed case digits");
        assert_eq!(parse_pair(&['F', 'F']), Some(0xFF), "top of the range");
        assert_eq!(parse_pair(&['Z', 'Q']), None, "non-hex characters");
        assert_eq!(parse_pair(&[]), None, "an empty pair is not a byte");
        assert_eq!(parse_pair(&['7']), None, "a lone digit is not a byte");
    }

    #[test]
    fn needles_match_with_the_right_case_rule() {
        let data = b"Thing 1 (Troop)".to_vec();
        let text = Needle {
            bytes: b"TROOP",
            ignore_case: true,
        };
        let hex = Needle {
            bytes: b"(T",
            ignore_case: false,
        };
        let lone = Needle {
            bytes: b"",
            ignore_case: false,
        };
        assert_eq!(find_needle(&data, &text), Some(9), "case folds both ways");
        assert_eq!(find_needle(&data, &hex), Some(8), "exact bytes");
        assert_eq!(
            find_needle(&data, &lone),
            Some(0),
            "an empty needle matches at 0"
        );
        let missing = Needle {
            bytes: b"(t",
            ignore_case: false,
        };
        assert_eq!(find_needle(&data, &missing), None, "hex stays case-exact");
        let absent = Needle {
            bytes: b"NOPE",
            ignore_case: true,
        };
        assert_eq!(find_needle(&data, &absent), None, "absent text");
    }

    #[test]
    fn judge_combines_every_criterion_with_and() {
        let empty = Search {
            pattern: None,
            kind: None,
            namespace: None,
            text: None,
            hex: None,
        };
        assert!(empty.is_empty(), "no criteria at all");
        let floor = row("FLOOR1", LumpKind::Flat, Some(Namespace::Flats), b"aa");
        let dehacked = row("DEHACKED", LumpKind::Dehacked, None, b"Thing 1 (Troop)");
        assert!(!glob_search("F*").is_empty(), "a pattern is a criterion");
        assert!(
            matches!(glob_search("F*").judge(&floor), Match::Hit(None)),
            "a name-only hit carries no offset"
        );
        assert!(
            matches!(glob_search("?LOOR1").judge(&floor), Match::Hit(None)),
            "question marks"
        );
        assert!(
            matches!(glob_search("F*").judge(&dehacked), Match::Miss),
            "a name miss drops the row"
        );
        let conflicting = Search {
            pattern: None,
            kind: Some(LumpKind::Dehacked),
            namespace: Some(Namespace::Flats),
            text: None,
            hex: None,
        };
        assert!(
            matches!(conflicting.judge(&dehacked), Match::Miss),
            "kind AND namespace must both hold"
        );
        let content = Search {
            pattern: None,
            kind: None,
            namespace: None,
            text: Some(b"TROOP".to_vec()),
            hex: Some(b"(T".to_vec()),
        };
        assert!(
            matches!(content.judge(&dehacked), Match::Hit(Some(8))),
            "the lowest content offset wins"
        );
        let missing_hex = Search {
            pattern: None,
            kind: None,
            namespace: None,
            text: Some(b"TROOP".to_vec()),
            hex: Some(b"ZZ".to_vec()),
        };
        assert!(
            matches!(missing_hex.judge(&dehacked), Match::Miss),
            "one absent needle drops the row"
        );
    }

    #[test]
    fn error_hints_list_every_documented_name() {
        assert_eq!(
            namespace_names(),
            "sprites, flats, patches, colormaps, acs, voices, hires, textures",
            "the namespace hint text"
        );
        assert!(
            kind_names().starts_with("marker, map_marker, map_data, palette"),
            "the kind hint leads with the directory kinds"
        );
        assert!(
            kind_names().contains("pc_speaker"),
            "the kind hint is complete"
        );
    }
}
