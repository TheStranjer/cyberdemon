//! MCP tool implementations, grouped by router, plus the shared result
//! helpers every tool returns through.
//!
//! `ok` and `fail` are public so later tool groups (and callers embedding
//! this library) can build results with them; the tool group modules
//! themselves stay crate-private, except `textures`, whose session-level
//! [`textures::TextureSet`] loader is public API texture editing will grow
//! around, `maps`, whose [`maps::params`] wire types and validators the
//! element-editing tools are built from, `graphics` and `palette`, whose
//! resolvers the renderers share, and `themes`, whose [`themes::theme_texture`]
//! is how every role-asking tool (drawing, doors, stairs, the plan builder)
//! resolves its textures.

pub(crate) mod assets;
pub(crate) mod audio;
pub(crate) mod batch;
pub(crate) mod convert;
pub(crate) mod defs;
pub(crate) mod edit_geometry;
pub(crate) mod edit_linedefs;
pub(crate) mod edit_sectors;
pub(crate) mod edit_sidedefs;
pub(crate) mod edit_things;
pub(crate) mod edit_vertices;
pub mod graphics;
pub(crate) mod imports;
pub(crate) mod inspect;
pub(crate) mod lumps;
pub mod maps;
pub(crate) mod meta;
pub(crate) mod music;
pub(crate) mod nodes;
pub mod palette;
pub(crate) mod render;
pub(crate) mod scripts;
pub(crate) mod session;
pub mod textures;
pub mod themes;
pub(crate) mod validate;

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
/// The client sees `{"error": "<code>", "message": "<text>"}` with `is_error`
/// set; the JSON-RPC layer itself stays healthy. This is the rule for every
/// tool: missing lumps, bad data or I/O problems return through `fail`, and
/// `Err(ErrorData)` is reserved for a broken server.
///
/// # Errors
///
/// [`ErrorData`] (JSON-RPC internal error) if the payload cannot be
/// serialized - a server-side bug, not a domain failure.
#[inline]
pub fn fail(error: &CyberError) -> Result<CallToolResult, ErrorData> {
    let payload = ErrorPayload {
        error: error.code(),
        message: error.to_string(),
    };
    serde_json::to_value(payload)
        .map(CallToolResult::structured_error)
        .map_err(|failed| serialize_failed("failed to serialize error result", &failed))
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde_json::json;

    use super::{fail, ok};
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
            })),
            "error object shape"
        );
    }
}
