//! The domain error type shared by every tool.

use std::io;

/// A domain failure reported back to the client as structured tool content.
///
/// The payload of each variant is a human-readable message. [`CyberError::code`]
/// maps the variant to the stable machine-readable code clients can branch on.
/// [`CyberError::with_hint`] wraps any error with a next-step instruction;
/// [`crate::tools::fail`] always reports one, falling back to
/// [`default_hint`] for the code when nothing specific was attached.
#[derive(Clone, Debug, thiserror::Error)]
pub enum CyberError {
    /// An argument was missing, malformed or out of range.
    #[error("{0}")]
    InvalidArgument(String),
    /// The requested lump, map or other resource does not exist.
    #[error("{0}")]
    NotFound(String),
    /// Creating a resource whose name is already taken.
    #[error("{0}")]
    AlreadyExists(String),
    /// The operation conflicts with the current state, e.g. a dirty buffer.
    #[error("{0}")]
    Conflict(String),
    /// The WAD (or another input file) is malformed or unreadable as data.
    #[error("{0}")]
    Format(String),
    /// Reading or writing the filesystem failed.
    #[error("{0}")]
    Io(String),
    /// Fetching a remote resource failed.
    #[error("{0}")]
    Fetch(String),
    /// The requested feature or format is not supported.
    #[error("{0}")]
    Unsupported(String),
    /// A configured limit (size, count, depth) was exceeded.
    #[error("{0}")]
    LimitExceeded(String),
    /// An unexpected server-side condition, e.g. a poisoned lock.
    #[error("{0}")]
    Internal(String),
    /// Any other error carrying a specific next-step hint for the client.
    ///
    /// [`Self::code`] and [`Self::message`] delegate to `inner`; only
    /// [`Self::hint`] sees the wrapper. Wrapping twice keeps the outermost
    /// hint.
    #[error("{inner}")]
    WithHint {
        /// The wrapped domain error whose code and message the client sees.
        inner: Box<Self>,
        /// The specific instruction what to do next.
        hint: String,
    },
}

impl CyberError {
    /// The stable machine-readable code for this error variant.
    ///
    /// Clients see this as the `error` field of a failed structured tool result.
    /// A [`Self::WithHint`] wrapper reports its `inner`'s code.
    #[must_use]
    #[inline]
    pub fn code(&self) -> &'static str {
        // The clone matches the repo's pattern lints (bindings need an owned
        // scrutinee) and costs one small allocation on an error path only.
        let mut current = self.clone();
        loop {
            current = match current {
                Self::InvalidArgument(_) => return "invalid_argument",
                Self::NotFound(_) => return "not_found",
                Self::AlreadyExists(_) => return "already_exists",
                Self::Conflict(_) => return "conflict",
                Self::Format(_) => return "format_error",
                Self::Io(_) => return "io_error",
                Self::Fetch(_) => return "fetch_error",
                Self::Unsupported(_) => return "unsupported",
                Self::LimitExceeded(_) => return "limit_exceeded",
                Self::Internal(_) => return "internal",
                Self::WithHint { inner, .. } => *inner,
            };
        }
    }

    /// The human-readable message, delegated through any [`Self::WithHint`]
    /// wrapper to the wrapped error's text (the `Display` implementation
    /// already prints exactly that).
    #[must_use]
    #[inline]
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// Wrap this error with a specific next-step instruction for the client.
    ///
    /// The wrapper changes only the reported hint; the code and message stay
    /// the wrapped error's own.
    #[must_use]
    #[inline]
    pub fn with_hint<H>(self, hint: H) -> Self
    where
        H: Into<String>,
    {
        Self::WithHint {
            inner: Box::new(self),
            hint: hint.into(),
        }
    }

    /// The hint to report to the client: the nearest attached [`Self::WithHint`]
    /// hint, else [`default_hint`] for this error's code.
    #[must_use]
    #[inline]
    pub fn hint(&self) -> String {
        match self.clone() {
            Self::WithHint { hint, .. } => hint,
            bare @ (Self::InvalidArgument(_)
            | Self::NotFound(_)
            | Self::AlreadyExists(_)
            | Self::Conflict(_)
            | Self::Format(_)
            | Self::Io(_)
            | Self::Fetch(_)
            | Self::Unsupported(_)
            | Self::LimitExceeded(_)
            | Self::Internal(_)) => default_hint(bare.code()).to_owned(),
        }
    }

    /// Rebuild this error with its message text run through `change`, keeping
    /// the [`Self::code`] and any attached [`Self::with_hint`] hint. A
    /// [`Self::WithHint`] wrapper maps its `inner` and keeps its hint, so the
    /// message prefix reaches through the wrapper too.
    #[must_use]
    pub(crate) fn map_message<F>(self, change: F) -> Self
    where
        F: FnOnce(String) -> String,
    {
        match self {
            Self::InvalidArgument(message) => Self::InvalidArgument(change(message)),
            Self::NotFound(message) => Self::NotFound(change(message)),
            Self::AlreadyExists(message) => Self::AlreadyExists(change(message)),
            Self::Conflict(message) => Self::Conflict(change(message)),
            Self::Format(message) => Self::Format(change(message)),
            Self::Io(message) => Self::Io(change(message)),
            Self::Fetch(message) => Self::Fetch(change(message)),
            Self::Unsupported(message) => Self::Unsupported(change(message)),
            Self::LimitExceeded(message) => Self::LimitExceeded(change(message)),
            Self::Internal(message) => Self::Internal(change(message)),
            Self::WithHint { inner, hint } => Self::WithHint {
                inner: Box::new(inner.map_message(change)),
                hint,
            },
        }
    }
}

/// The per-code fallback instruction [`crate::tools::fail`] reports when no
/// specific hint was attached with [`CyberError::with_hint`].
#[must_use]
#[inline]
pub fn default_hint(code: &str) -> &'static str {
    match code {
        "not_found" => "use a list_* / find_elements / describe_map tool to see what exists",
        "invalid_argument" => "check the parameter against the tool's input schema",
        "already_exists" => "pass replace: true or choose another name",
        "conflict" => "reload the state (get_session_info) and retry",
        "format_error" => "the input is not a valid file of the expected kind",
        "io_error" | "fetch_error" => {
            "use an absolute path or an http(s) URL; an image that only exists in the \
             conversation must first be saved to a file by the user"
        }
        "unsupported" => "see skill/reference for supported formats",
        "limit_exceeded" => "narrow the selection or raise the limit parameter",
        _ => "this is a bug; undo the last change if the map looks wrong",
    }
}

impl From<io::Error> for CyberError {
    #[inline]
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::{CyberError, default_hint};

    #[test]
    fn code_maps_every_variant_to_its_stable_string() {
        let cases = [
            (
                CyberError::InvalidArgument("m".to_owned()),
                "invalid_argument",
            ),
            (CyberError::NotFound("m".to_owned()), "not_found"),
            (CyberError::AlreadyExists("m".to_owned()), "already_exists"),
            (CyberError::Conflict("m".to_owned()), "conflict"),
            (CyberError::Format("m".to_owned()), "format_error"),
            (CyberError::Io("m".to_owned()), "io_error"),
            (CyberError::Fetch("m".to_owned()), "fetch_error"),
            (CyberError::Unsupported("m".to_owned()), "unsupported"),
            (CyberError::LimitExceeded("m".to_owned()), "limit_exceeded"),
            (CyberError::Internal("m".to_owned()), "internal"),
        ];
        for (error, expected) in cases {
            assert_eq!(error.code(), expected, "code for {error:?}");
        }
    }

    #[test]
    fn codes_are_all_distinct() {
        let errors = [
            CyberError::InvalidArgument("m".to_owned()),
            CyberError::NotFound("m".to_owned()),
            CyberError::AlreadyExists("m".to_owned()),
            CyberError::Conflict("m".to_owned()),
            CyberError::Format("m".to_owned()),
            CyberError::Io("m".to_owned()),
            CyberError::Fetch("m".to_owned()),
            CyberError::Unsupported("m".to_owned()),
            CyberError::LimitExceeded("m".to_owned()),
            CyberError::Internal("m".to_owned()),
        ];
        let mut seen: Vec<&str> = Vec::new();
        for error in &errors {
            let code = error.code();
            assert!(
                !seen.contains(&code),
                "{code} reported by more than one variant"
            );
            seen.push(code);
        }
    }

    #[test]
    fn display_shows_the_payload_message() {
        let error = CyberError::NotFound("lump FLOOR_4 does not exist".to_owned());
        assert_eq!(
            error.to_string(),
            "lump FLOOR_4 does not exist",
            "message text"
        );
    }

    #[test]
    fn io_error_converts_to_the_io_variant() {
        let source = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let error: CyberError = source.into();
        assert!(
            matches!(&error, CyberError::Io(message) if message.as_str() == "denied"),
            "io error text must be kept verbatim, got {error:?}"
        );
        assert_eq!(error.code(), "io_error", "code of converted error");
    }

    #[test]
    fn with_hint_delegates_code_and_message_to_inner() {
        let error = CyberError::NotFound("lump SKY1 does not exist".to_owned())
            .with_hint("search_lumps finds lumps by pattern");
        assert_eq!(error.code(), "not_found", "the code stays the inner one");
        assert_eq!(
            error.message(),
            "lump SKY1 does not exist",
            "the message stays the inner one"
        );
        assert_eq!(
            error.to_string(),
            "lump SKY1 does not exist",
            "Display shows the inner message too"
        );
        assert!(
            matches!(&error, CyberError::WithHint { inner, hint }
                if matches!(**inner, CyberError::NotFound(_))
                    && hint.as_str() == "search_lumps finds lumps by pattern"),
            "the wrapper shape: {error:?}"
        );
    }

    #[test]
    fn hint_returns_the_attached_text_or_the_code_default() {
        let attached =
            CyberError::Conflict("dirty".to_owned()).with_hint("reload the state and retry");
        assert_eq!(
            attached.hint(),
            "reload the state and retry",
            "attached wins"
        );
        let bare = CyberError::Conflict("dirty".to_owned());
        assert_eq!(
            bare.hint(),
            default_hint("conflict"),
            "without a wrapper the code default is reported"
        );
    }

    #[test]
    fn the_outermost_hint_wins_on_double_wrapping() {
        let error = CyberError::Format("bad".to_owned())
            .with_hint("first")
            .with_hint("second");
        assert_eq!(error.code(), "format_error", "code delegates through both");
        assert_eq!(error.message(), "bad", "message delegates through both");
        assert_eq!(error.hint(), "second", "the last attached hint answers");
    }

    #[test]
    fn map_message_prefixes_the_text_and_keeps_the_code_and_hint() {
        let error = CyberError::NotFound("lump SKY1 does not exist".to_owned())
            .with_hint("search_lumps finds lumps by pattern");
        let changed = error.map_message(|message| format!("from: {message}"));
        assert_eq!(changed.code(), "not_found", "the code stands");
        assert_eq!(
            changed.hint(),
            "search_lumps finds lumps by pattern",
            "the attached hint rides along"
        );
        assert_eq!(
            changed.message(),
            "from: lump SKY1 does not exist",
            "the message is mapped"
        );
    }

    #[test]
    fn every_code_has_its_documented_default_hint() {
        let cases = [
            ("not_found", "list_* / find_elements / describe_map"),
            ("invalid_argument", "input schema"),
            ("already_exists", "pass replace: true"),
            ("conflict", "get_session_info"),
            ("format_error", "not a valid file"),
            ("io_error", "absolute path"),
            ("fetch_error", "absolute path"),
            ("unsupported", "skill/reference"),
            ("limit_exceeded", "narrow the selection"),
            ("internal", "undo the last change"),
        ];
        for (code, needle) in cases {
            assert!(
                default_hint(code).contains(needle),
                "{code} default hint must name {needle:?}: {}",
                default_hint(code)
            );
        }
        assert_eq!(
            default_hint("anything_else"),
            default_hint("internal"),
            "an unknown code falls back to the bug hint"
        );
    }
}
