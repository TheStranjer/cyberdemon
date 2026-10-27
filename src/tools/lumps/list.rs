//! `list_lumps` and `search_lumps`: their arguments, the filters and
//! criteria those arguments parse into, and the reports the runs assemble.
//!
//! [`Listing`] is the parsed [`ListLumpsParams`], [`Search`] the parsed
//! [`SearchLumpsParams`]; both come from [`parse_listing`] and
//! [`parse_search`] over the shared names in [`crate::tools::lumps::parse`].
//! [`list_run`] filters and pages the scope's rows, [`search_run`] judges
//! every row with [`Search::judge`] and keeps up to `limit` [`Match`]es.
//! [`Needle`] is one content criterion, [`find_needle`] its first offset in
//! a payload, and [`parse_pair`] over [`hex_digit`] spells one byte of the
//! hex string [`crate::tools::lumps::parse::hex_bytes`] turns into a needle.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::ops::hex_digit;
use crate::session::{Scope, Session};
use crate::tools::lumps::parse::{parse_hex, parse_kind, parse_map, parse_namespace};
use crate::tools::parse_limit;

use super::{DEFAULT_LIMIT, MAX_LIMIT};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::list_lumps`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListLumpsParams {
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
pub(super) struct SearchLumpsParams {
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
pub(super) struct Listing {
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
pub(super) struct Needle<'crit> {
    /// The bytes to look for.
    pub(super) bytes: &'crit [u8],
    /// Whether the comparison folds `ASCII` case on both sides.
    pub(super) ignore_case: bool,
}

/// The parsed criteria of one `search_lumps` call.
pub(super) struct Search {
    /// Case-insensitive name glob, if given.
    pub(super) pattern: Option<String>,
    /// Type hint to keep, if given.
    pub(super) kind: Option<LumpKind>,
    /// Namespace to keep, if given.
    pub(super) namespace: Option<Namespace>,
    /// Text needle as given, matched case-insensitively, if given.
    pub(super) text: Option<Vec<u8>>,
    /// Needle bytes decoded from the hex string, if given.
    pub(super) hex: Option<Vec<u8>>,
}

/// How [`Search::judge`] decided about one row.
pub(super) enum Match {
    /// The row matches; the payload offset of the first content match,
    /// `None` when no content criterion was given.
    Hit(Option<usize>),
    /// The row fails some criterion and is dropped.
    Miss,
}

impl Search {
    /// Whether the caller gave no criterion at all.
    pub(super) const fn is_empty(&self) -> bool {
        self.pattern.is_none()
            && self.kind.is_none()
            && self.namespace.is_none()
            && self.text.is_none()
            && self.hex.is_none()
    }

    /// Judge one row against every criterion of this search.
    pub(super) fn judge(&self, row: &LumpRow) -> Match {
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
/// not. An empty needle matches at offset 0. The case-insensitive rule
/// folds `ASCII` on the fly, without copying the payload.
pub(super) fn find_needle(data: &[u8], needle: &Needle<'_>) -> Option<usize> {
    if needle.bytes.is_empty() {
        return Some(0);
    }
    if needle.ignore_case {
        return data
            .windows(needle.bytes.len())
            .position(|window| window.eq_ignore_ascii_case(needle.bytes));
    }
    data.windows(needle.bytes.len())
        .position(|window| window == needle.bytes)
}

/// The byte two hex characters spell, `None` unless the slice is exactly
/// two characters and both are hex digits.
pub(super) fn parse_pair(pair: &[char]) -> Option<u8> {
    let &[high, low] = pair else {
        return None;
    };
    let (Some(h), Some(l)) = (ascii_hex_digit(high), ascii_hex_digit(low)) else {
        return None;
    };
    h.checked_mul(16)?.checked_add(l)
}

/// The value of one `ASCII` hex character through the shared [`hex_digit`],
/// `None` for anything else.
fn ascii_hex_digit(ch: char) -> Option<u8> {
    u8::try_from(ch).ok().and_then(hex_digit)
}

/// Structured result of [`Cyberdemon::list_lumps`].
#[derive(Serialize)]
pub(super) struct ListReport {
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
pub(super) struct SearchRow {
    /// The directory row, flattened into the object.
    #[serde(flatten)]
    row: LumpRow,
    /// Byte offset of the first `contains_text`/`contains_hex` match
    /// (lowest across both needles), `null` when neither was given.
    match_offset: Option<usize>,
}

/// Structured result of [`Cyberdemon::search_lumps`].
#[derive(Serialize)]
pub(super) struct SearchReport {
    /// The scope that was searched.
    scope: Scope,
    /// Rows in the result.
    count: usize,
    /// Whether more lumps matched than `limit` allowed.
    truncated: bool,
    /// The hits, in directory order, capped at `limit`.
    lumps: Vec<SearchRow>,
}

/// The `list_lumps` filters and paging, parsed from its arguments.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad `scope`, `namespace`, `kind`,
/// `map` or `limit`.
pub(super) fn parse_listing(args: &ListLumpsParams) -> Result<Listing, CyberError> {
    Ok(Listing {
        scope: Scope::parse_or_merged(args.scope.as_deref())?,
        namespace: parse_namespace(args.namespace.as_deref())?,
        kind: parse_kind(args.kind.as_deref())?,
        map: parse_map(args.map.as_deref())?,
        offset: args.offset.unwrap_or(0_usize),
        limit: parse_limit(args.limit, DEFAULT_LIMIT, MAX_LIMIT, "limit")?,
    })
}

/// List, filter and page the scope's rows.
pub(super) fn list_run(session: &Session, listing: &Listing) -> Result<ListReport, CyberError> {
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
pub(super) fn parse_search(args: &SearchLumpsParams) -> Result<(Scope, usize, Search), CyberError> {
    let search = Search {
        pattern: args.pattern.clone(),
        kind: parse_kind(args.kind.as_deref())?,
        namespace: parse_namespace(args.namespace.as_deref())?,
        text: args
            .contains_text
            .as_deref()
            .map(str::as_bytes)
            .map(<[u8]>::to_vec),
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
        Scope::parse_or_merged(args.scope.as_deref())?,
        parse_limit(args.limit, DEFAULT_LIMIT, MAX_LIMIT, "limit")?,
        search,
    ))
}

/// Judge every row of the scope and keep up to `limit` hits.
pub(super) fn search_run(
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
