//! The domain error type shared by every tool.

use std::io;

/// A domain failure reported back to the client as structured tool content.
///
/// The payload of each variant is a human-readable message. [`CyberError::code`]
/// maps the variant to the stable machine-readable code clients can branch on.
#[derive(Debug, thiserror::Error)]
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
}

impl CyberError {
    /// The stable machine-readable code for this error variant.
    ///
    /// Clients see this as the `error` field of a failed structured tool result.
    #[must_use]
    #[inline]
    pub const fn code(&self) -> &'static str {
        match *self {
            Self::InvalidArgument(_) => "invalid_argument",
            Self::NotFound(_) => "not_found",
            Self::AlreadyExists(_) => "already_exists",
            Self::Conflict(_) => "conflict",
            Self::Format(_) => "format_error",
            Self::Io(_) => "io_error",
            Self::Fetch(_) => "fetch_error",
            Self::Unsupported(_) => "unsupported",
            Self::LimitExceeded(_) => "limit_exceeded",
            Self::Internal(_) => "internal",
        }
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

    use super::CyberError;

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
        let error = CyberError::NotFound("lump FOO does not exist".to_owned());
        assert_eq!(error.to_string(), "lump FOO does not exist", "message text");
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
}
