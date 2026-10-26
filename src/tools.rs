//! MCP tool implementations, grouped by router, plus the shared result
//! helpers every tool returns through.
//!
//! `ok` and `fail` are public so later tool groups (and callers embedding
//! this library) can build results with them; the tool group modules
//! themselves stay crate-private, except `textures`, whose session-level
//! [`crate::session::textures::TextureSet`] loader this group re-exports is
//! public API texture editing will grow around, `maps`, whose [`maps::params`]
//! wire types and validators the element-editing tools are built from,
//! `graphics` and `palette`, which re-export the shared session resolvers
//! (`crate::session::resolve`, `crate::session::palette`) the image tools and
//! renderers use, `themes`, whose [`themes::theme_texture`]
//! is how every role-asking tool (drawing, doors, stairs, the plan builder)
//! resolves its textures, and `query`, whose [`query::replace`] bulk-edit
//! machinery is public for the same reason as the texture loader:
//! not-yet-wired crate-private items would not survive the `dead_code`
//! check until the `replace_*` tool slices build on them. Those two
//! re-exporting groups (`graphics`, `palette`) and the loader re-export in
//! `textures` stay `pub` so their `pub(crate) use` re-exports are not flagged
//! as redundant.

pub(crate) mod assets;
pub(crate) mod audio;
pub(crate) mod audio_common;
pub(crate) mod batch;
pub(crate) mod convert;
pub(crate) mod defs;
pub(crate) mod edit_geometry;
pub(crate) mod edit_linedefs;
pub(crate) mod edit_sectors;
pub(crate) mod edit_sidedefs;
pub(crate) mod edit_things;
pub(crate) mod edit_vertices;
pub(crate) mod features;
pub mod graphics;
pub(crate) mod imports;
pub(crate) mod inspect;
pub(crate) mod lumps;
pub mod maps;
pub(crate) mod meta;
pub(crate) mod music;
pub(crate) mod nodes;
pub mod palette;
pub(crate) mod patches;
pub(crate) mod plan;
pub(crate) mod playability;
pub mod query;
pub(crate) mod render;
pub(crate) mod scripts;
pub(crate) mod session;
pub(crate) mod sketch;
pub mod textures;
pub mod themes;
pub(crate) mod validate;

use core::fmt::Display;

use rmcp::model::{CallToolResult, ErrorData};
use serde::Serialize;

use crate::error::CyberError;

/// The wire shape of a domain failure inside structured content.
#[derive(Serialize)]
struct ErrorPayload<'err> {
    /// The stable machine-readable error code.
    error: &'err str,
    /// The human-readable message.
    message: String,
    /// What to do next: the hint attached with
    /// [`CyberError::with_hint`], or [`crate::error::default_hint`] for the
    /// code.
    hint: String,
}

/// The `{"error", "message", "hint"}` object [`fail`] reports, shared with
/// the batch tool's step outcomes so one failure looks the same everywhere.
#[must_use]
#[inline]
pub(crate) fn error_json(error: &CyberError) -> serde_json::Value {
    serde_json::json!({
        "error": error.code(),
        "message": error.message(),
        "hint": error.hint(),
    })
}

/// Turn a serialization failure into the protocol's internal error.
fn serialize_failed(context: &str, error: &serde_json::Error) -> ErrorData {
    ErrorData::internal_error(format!("{context}: {error}"), None)
}

/// Wrap a serializable value in a successful structured tool result.
///
/// # Errors
///
/// [`ErrorData`] (JSON-RPC internal error) when `serde_json` cannot represent
/// `value` as JSON - a server-side bug such as a map with non-string keys,
/// never a domain failure. Domain failures go through [`fail`].
#[inline]
pub fn ok<T>(value: &T) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    serde_json::to_value(value)
        .map(CallToolResult::structured)
        .map_err(|error| serialize_failed("failed to serialize tool result", &error))
}

/// Report a domain failure as an errored structured tool result.
///
/// The client sees `{"error": "<code>", "message": "<text>", "hint": "<text>"}`
/// with `is_error` set; the JSON-RPC layer itself stays healthy. `hint` is the
/// error's attached [`CyberError::with_hint`] text when a tool site provided
/// one, else [`crate::error::default_hint`] for the code, so every failure
/// says what to do next. This is the rule for every tool: missing lumps, bad
/// data or I/O problems return through `fail`, and `Err(ErrorData)` is
/// reserved for a broken server.
///
/// # Errors
///
/// [`ErrorData`] (JSON-RPC internal error) if the payload cannot be
/// serialized - a server-side bug, not a domain failure.
#[inline]
pub fn fail(error: &CyberError) -> Result<CallToolResult, ErrorData> {
    let payload = ErrorPayload {
        error: error.code(),
        message: error.message(),
        hint: error.hint(),
    };
    serde_json::to_value(payload)
        .map(CallToolResult::structured_error)
        .map_err(|failed| serialize_failed("failed to serialize error result", &failed))
}

/// Hand one handler's outcome to the client: [`ok`] for a success, [`fail`]
/// for a domain error. Every tool method returns through this, so the
/// `Ok`/`Err` fold of a [`CyberError`]-bearing run is written once.
#[inline]
pub(crate) fn reply<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

/// Serializes an asset body to pretty `JSON`.
///
/// # Errors
///
/// [`CyberError::Internal`] naming `what` when serialization fails (a
/// server bug).
pub(crate) fn json_bytes<T: Serialize>(value: &T, what: &str) -> Result<Vec<u8>, CyberError> {
    serde_json::to_vec_pretty(value)
        .map_err(|error| CyberError::Internal(format!("the {what} does not serialize: {error}")))
}

/// The value of an optional capped integer parameter: `default` when
/// unset, else `value`, which must sit in `1..=max` or the call refuses
/// naming `name` and the range.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for `0` or anything above `max`.
pub(crate) fn parse_limit<T>(
    value: Option<T>,
    default: T,
    max: T,
    name: &str,
) -> Result<T, CyberError>
where
    T: Copy + Display + PartialOrd + From<u8>,
{
    let Some(given) = value else {
        return Ok(default);
    };
    if given == T::from(0_u8) || given > max {
        return Err(CyberError::InvalidArgument(format!(
            "{name} {given} is out of range: pass a value between 1 and {max}"
        )));
    }
    Ok(given)
}

/// The validated `offset`/`limit` window a listing or paged-report tool
/// was given: rows skipped before the page, rows kept on it.
#[derive(Debug)]
pub(crate) struct Page {
    /// Rows to skip before the page.
    pub(crate) offset: usize,
    /// Maximum rows on the page.
    pub(crate) limit: usize,
}

impl Page {
    /// The page starting at `offset` and holding at most `limit` rows.
    #[inline]
    pub(crate) const fn new(offset: usize, limit: usize) -> Self {
        Self { offset, limit }
    }

    /// The `{offset, limit}` pair a listing tool was given: `offset`
    /// defaults to `0`, `limit` runs through [`parse_limit`] with the
    /// listing's own `default` and `max`.
    ///
    /// # Errors
    ///
    /// [`parse_limit`]'s [`CyberError::InvalidArgument`] for a bad `limit`.
    pub(crate) fn parse(
        offset: Option<usize>,
        limit: Option<usize>,
        default: usize,
        max: usize,
    ) -> Result<Self, CyberError> {
        let value = parse_limit(limit, default, max, "limit")?;
        Ok(Self::new(offset.unwrap_or(0_usize), value))
    }

    /// Keeps the rows of `matched` this page covers and counts all matches.
    pub(crate) fn take<T>(&self, matched: Vec<T>) -> (usize, Vec<T>) {
        let total = matched.len();
        (
            total,
            matched
                .into_iter()
                .skip(self.offset)
                .take(self.limit)
                .collect(),
        )
    }

    /// The window over the ascending row list `rows`: this page's rows and
    /// the `next_offset` that continues them (`None` when nothing stayed
    /// behind, which an offset past the end always does).
    pub(crate) fn window(&self, rows: &[usize]) -> (Vec<usize>, Option<usize>) {
        let start = self.offset.min(rows.len());
        let stop = start.saturating_add(self.limit).min(rows.len());
        let page = rows.get(start..stop).unwrap_or_default().to_vec();
        let next = (stop < rows.len()).then(|| self.offset.saturating_add(page.len()));
        (page, next)
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde_json::json;

    use super::{Page, fail, json_bytes, ok, parse_limit, reply};
    use crate::error::CyberError;

    /// A tiny payload whose fields serialize as JSON object keys.
    #[derive(Serialize)]
    struct Payload {
        /// A message for the client.
        message: String,
        /// A count for the client.
        count: u32,
    }

    #[test]
    fn ok_produces_a_successful_structured_result() {
        let payload = Payload {
            message: "hello".to_owned(),
            count: 3,
        };
        let result = ok(&payload).expect("serializable payload");
        assert_ne!(result.is_error, Some(true), "ok must not error");
        assert_eq!(
            result.structured_content,
            Some(json!({ "message": "hello", "count": 3_i64 })),
            "structured content shape"
        );
        let text = result
            .content
            .first()
            .and_then(rmcp::model::ContentBlock::as_text)
            .map(|item| item.text.as_str());
        assert_eq!(
            text,
            Some(r#"{"count":3,"message":"hello"}"#),
            "text content mirrors the JSON"
        );
    }

    /// A payload whose `Serialize` implementation always fails.
    struct Broken;

    impl Serialize for Broken {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("broken payload"))
        }
    }

    #[test]
    fn ok_reports_serialization_failure_as_internal_error_data() {
        let protocol_error = ok(&Broken).expect_err("failing payload must error");
        assert_eq!(
            protocol_error.message, "failed to serialize tool result: broken payload",
            "internal error message"
        );
    }

    #[test]
    fn fail_produces_the_documented_error_shape() {
        let error = CyberError::NotFound("lump FLOOR_4 does not exist".to_owned());
        let result = fail(&error).expect("fail itself does not fail");
        assert_eq!(result.is_error, Some(true), "fail must flag the result");
        assert_eq!(
            result.structured_content,
            Some(json!({
                "error": "not_found",
                "message": "lump FLOOR_4 does not exist",
                "hint": "use a list_* / find_elements / describe_map tool to see what exists",
            })),
            "error object shape"
        );
    }

    #[test]
    fn fail_reports_an_attached_hint_instead_of_the_default() {
        let error = CyberError::NotFound("lump FLOOR_4 does not exist".to_owned())
            .with_hint("search_lumps finds lumps by pattern");
        let result = fail(&error).expect("fail itself does not fail");
        let content = result.structured_content.expect("structured content");
        assert_eq!(content["error"], "not_found", "code delegates to inner");
        assert_eq!(
            content["message"], "lump FLOOR_4 does not exist",
            "message delegates to inner: {content}"
        );
        assert_eq!(
            content["hint"], "search_lumps finds lumps by pattern",
            "hint"
        );
    }

    #[test]
    fn reply_folds_success_and_failure_like_ok_and_fail() {
        let payload = Payload {
            message: "done".to_owned(),
            count: 1,
        };
        let success = reply(Ok(payload)).expect("serializable payload");
        let direct = ok(&Payload {
            message: "done".to_owned(),
            count: 1,
        })
        .expect("same payload");
        assert_eq!(
            success.structured_content, direct.structured_content,
            "a success replies as ok"
        );
        let error = CyberError::NotFound("nothing here".to_owned());
        let failure = reply(Err::<Payload, _>(error.clone())).expect("fail itself does not fail");
        let expected = fail(&error).expect("fail itself does not fail");
        assert_eq!(
            failure.structured_content, expected.structured_content,
            "a domain error replies as fail"
        );
        assert_eq!(failure.is_error, Some(true), "and still flags the result");
    }

    #[test]
    fn json_bytes_pretties_and_names_the_thing_that_failed() {
        let body = json_bytes(&json!({ "a": 1_i64 }), "asset").expect("a value serializes");
        assert!(
            String::from_utf8_lossy(&body).contains('\n'),
            "the body is pretty JSON"
        );
        let error = json_bytes(&Broken, "graph").expect_err("broken payload");
        assert_eq!(error.code(), "internal", "a serialization bug is internal");
        assert!(
            error
                .to_string()
                .starts_with("the graph does not serialize:"),
            "the message names the thing: {error}"
        );
    }

    #[test]
    fn parse_limit_defaults_bounds_and_refuses_with_the_named_range() {
        assert_eq!(
            parse_limit(None, 200_usize, 2000_usize, "limit").expect("absent"),
            200
        );
        assert_eq!(
            parse_limit(Some(1_usize), 200_usize, 2000_usize, "limit").expect("the floor"),
            1,
            "one is the floor"
        );
        assert_eq!(
            parse_limit(Some(2000_usize), 200, 2000, "limit").expect("the ceiling"),
            2000,
            "the maximum is inclusive"
        );
        for bad in [0_usize, 2001] {
            let error = parse_limit(Some(bad), 200, 2000, "limit").expect_err("out of range");
            assert_eq!(error.code(), "invalid_argument", "bad limit code");
            assert_eq!(
                error.message(),
                format!("limit {bad} is out of range: pass a value between 1 and 2000"),
                "the message names the parameter and the range"
            );
        }
        assert_eq!(
            parse_limit(None, 60_u32, 600_u32, "max_seconds").expect("absent"),
            60,
            "the fold works for other integer types too"
        );
        let small =
            parse_limit(Some(601_u32), 60_u32, 600_u32, "max_seconds").expect_err("above the cap");
        assert!(
            small
                .message()
                .starts_with("max_seconds 601 is out of range:"),
            "the name leads the message: {small}"
        );
    }

    #[test]
    fn page_parses_the_wire_pair_with_the_shared_refusal() {
        let page = Page::parse(None, None, 200, 2000).expect("both absent");
        assert_eq!(
            (page.offset, page.limit),
            (0, 200),
            "0 offset, default limit"
        );
        let given = Page::parse(Some(7), Some(9), 200, 2000).expect("both given");
        assert_eq!((given.offset, given.limit), (7, 9), "both pass through");
        let error = Page::parse(None, Some(0), 200, 2000).expect_err("zero is no page");
        assert_eq!(
            error.message(),
            "limit 0 is out of range: pass a value between 1 and 2000",
            "the shared limit refusal"
        );
    }

    #[test]
    fn page_take_windows_a_vec_and_counts_all_matches() {
        let rows: Vec<usize> = (0..6).collect();
        let (total, page) = Page::new(2, 3).take(rows.clone());
        assert_eq!(total, 6, "the total counts every match");
        assert_eq!(page, vec![2_usize, 3, 4], "the windowed rows");
        let (count, past) = Page::new(9, 2).take(rows);
        assert_eq!(
            (count, past),
            (6, Vec::new()),
            "an offset past the end is empty"
        );
    }

    #[test]
    fn page_window_names_the_continuation_or_its_absence() {
        let rows: Vec<usize> = (0..6).collect();
        let (first, next) = Page::new(0, 2).window(&rows);
        assert_eq!(first, vec![0_usize, 1], "the leading page");
        assert_eq!(next, Some(2), "the rest continues at 2");
        let (last, rest) = Page::new(4, 2).window(&rows);
        assert_eq!(last, vec![4_usize, 5], "the final page");
        assert_eq!(rest, None, "nothing stayed behind");
        let (past, beyond) = Page::new(9, 2).window(&rows);
        assert!(
            past.is_empty(),
            "an offset past the end is empty, not an error"
        );
        assert_eq!(beyond, None, "and has no continuation");
        let (all, tail) = Page::new(1, 100).window(&rows);
        assert_eq!(
            all,
            vec![1_usize, 2, 3, 4, 5],
            "a generous page takes the rest"
        );
        assert_eq!(tail, None, "leaving nothing to continue at");
    }
}
