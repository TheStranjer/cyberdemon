//! The one payload picker and byte fetch behind the lump-editing tools.
//!
//! [`pick_payload`] turns the three optional wire fields (`text`, `hex`
//! and `source`) into a single [`Payload`] under a [`PayloadRule`]:
//! `add_lump` takes at most one and reads none as an empty marker,
//! `replace_lump` requires exactly one. Both refusal wordings come from
//! this one function, so the tools' rules can only ever differ in the
//! rule word. [`payload_bytes`] brings the bytes in, fetching a `source`
//! before any session lock is ever taken and reporting the fetched file
//! name so the import tools can derive a lump name from it.

use crate::error::CyberError;
use crate::state::AppState;
use crate::tools::lumps::parse::hex_bytes;

/// How the caller supplied a lump's bytes.
#[derive(Debug)]
pub(in crate::tools::lumps) enum Payload {
    /// No data field: the lump will be an empty marker.
    Empty,
    /// The `text` argument, kept as UTF-8 bytes.
    Text(String),
    /// The `hex` argument, waiting to be decoded.
    Hex(String),
    /// A `source` to fetch.
    Source(String),
}

/// The three optional payload fields of an editing call, as borrowed
/// text.
#[derive(Clone, Copy, Debug)]
pub(in crate::tools::lumps) struct PayloadFields<'fields> {
    /// The `text` argument.
    pub(in crate::tools::lumps) text: Option<&'fields str>,
    /// The `hex` argument.
    pub(in crate::tools::lumps) hex: Option<&'fields str>,
    /// The `source` argument.
    pub(in crate::tools::lumps) source: Option<&'fields str>,
}

impl<'fields> PayloadFields<'fields> {
    /// The fields `text`, `hex` and `source` of one call's arguments.
    pub(in crate::tools::lumps) const fn new(
        text: Option<&'fields str>,
        hex: Option<&'fields str>,
        source: Option<&'fields str>,
    ) -> Self {
        Self { text, hex, source }
    }
}

/// How strictly a tool enforces the payload fields.
#[derive(Clone, Copy, Debug)]
pub(in crate::tools::lumps) enum PayloadRule {
    /// `add_lump`: at most one field; none is an empty marker.
    AtMostOne,
    /// `replace_lump`: exactly one field; none is refused.
    ExactlyOne,
}

impl PayloadRule {
    /// The rule's wording in the more-than-one refusal: "takes {this}
    /// one of text, hex and source".
    const fn word(self) -> &'static str {
        match self {
            Self::AtMostOne => "at most",
            Self::ExactlyOne => "exactly",
        }
    }
}

/// The one payload field `tool` was given, enforced under `rule`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when more than one field is present,
/// or when `rule` requires a payload and none was given.
pub(in crate::tools::lumps) fn pick_payload(
    tool: &str,
    rule: PayloadRule,
    fields: PayloadFields<'_>,
) -> Result<Payload, CyberError> {
    match (fields.text, fields.hex, fields.source) {
        (None, None, None) => no_payload(tool, rule),
        (Some(body), None, None) => Ok(Payload::Text(body.to_owned())),
        (None, Some(digits), None) => Ok(Payload::Hex(digits.to_owned())),
        (None, None, Some(raw)) => Ok(Payload::Source(raw.to_owned())),
        _ => Err(CyberError::InvalidArgument(format!(
            "{tool} takes {} one of text, hex and source: pick one payload form",
            rule.word()
        ))),
    }
}

/// What the absence of every payload field means under the rule:
/// `add_lump`'s empty marker, or `replace_lump`'s missing payload.
fn no_payload(tool: &str, rule: PayloadRule) -> Result<Payload, CyberError> {
    match rule {
        PayloadRule::AtMostOne => Ok(Payload::Empty),
        PayloadRule::ExactlyOne => Err(CyberError::InvalidArgument(format!(
            "{tool} needs a new payload: pass exactly one of text, hex and source"
        ))),
    }
}

/// The payload's bytes, fetching a `source` before the session lock is
/// ever taken. The second element is the fetched file's name, which only
/// a `source` has; the import tools derive a lump name from it when the
/// call carried none.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for malformed hex;
/// [`AppState::fetch_source`]' errors pass through for every fetch
/// failure.
pub(in crate::tools::lumps) async fn payload_bytes(
    state: &AppState,
    payload: Payload,
) -> Result<(Vec<u8>, Option<String>), CyberError> {
    match payload {
        Payload::Empty => Ok((Vec::new(), None)),
        Payload::Text(body) => Ok((body.into_bytes(), None)),
        Payload::Hex(digits) => Ok((hex_bytes("hex", &digits)?, None)),
        Payload::Source(raw) => {
            let fetched = state.fetch_source(&raw).await?;
            Ok((fetched.bytes, Some(fetched.filename)))
        }
    }
}
