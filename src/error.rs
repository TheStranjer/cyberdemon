//! The domain error type shared by every tool.

use std::io;

/// A domain failure reported back to the client as structured tool content.
///
/// The payload of each leaf variant is a human-readable message.
/// [`CyberError::code`] maps the variant to the stable machine-readable code
/// clients can branch on. [`CyberError::with_hint`] wraps any error with a
/// next-step instruction; [`crate::tools::fail`] always reports one, falling
/// back to [`default_hint`] for the code when nothing specific was attached.
///
/// Two other wrappers carry structure rather than words: [`Self::Emptied`]
/// marks the refusal an image op raises when it leaves the picture with no
/// content, and [`Self::AtPlan`] attributes a refusal to one element of a
/// [`crate::plan::model::Plan`]. Both keep the wrapped error's code, message
/// and hint, so nothing the client sees changes; they exist so the code that
/// raised or is about to report one branches on the variant — through
/// [`Self::is_emptied`] and [`Self::plan_ref`] — instead of on message text.
/// The [`Self::is_not_found`], [`Self::is_invalid_argument`] and
/// [`Self::is_limit_exceeded`] predicates answer the same way for the leaf
/// variants, through every wrapper.
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
    /// A refusal an image op took because it left the picture with no
    /// content to work on, which the caller rewords into its own
    /// instruction.
    ///
    /// [`Self::code`], [`Self::message`] and [`Self::hint`] delegate to
    /// `inner`; only [`Self::is_emptied`] sees the wrapper.
    #[error("{0}")]
    Emptied(Box<Self>),
    /// A refusal attributed to one element of a
    /// [`crate::plan::model::Plan`], so the tool edge can name the wire row
    /// that broke without reading the message text.
    ///
    /// [`Self::code`], [`Self::message`] and [`Self::hint`] delegate to
    /// `inner`; only [`Self::plan_ref`] sees the wrapper.
    #[error("{inner}")]
    AtPlan {
        /// The wrapped domain error whose code and message the client sees.
        inner: Box<Self>,
        /// The plan element the refusal points at.
        at: PlanRef,
    },
}

/// Which kind of [`crate::plan::model::Plan`] row a [`PlanRef`] cites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanKind {
    /// A row of [`Plan::rooms`](crate::plan::model::Plan::rooms).
    Room,
    /// A row of [`Plan::connections`](crate::plan::model::Plan::connections).
    Connection,
    /// A row of [`Plan::features`](crate::plan::model::Plan::features).
    Feature,
}

/// The element of a [`crate::plan::model::Plan`] a compiler or feature
/// refusal points at.
///
/// The reference names the element by the id the plan carries for it —
/// a room's `id`, a connection's `id` or its `from->to` label, or the room
/// or connection id a feature row names — so the tool edge can map it onto
/// its own wire row (`rooms[i]`, `connections[i]` or `features[i]`) instead
/// of reading an id off the refusal's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRef {
    /// The kind of row the refusal's wire path names.
    kind: PlanKind,
    /// The id that row carries (or, for a feature row, names).
    id: String,
}

impl PlanRef {
    /// The reference to the room with this `id`.
    #[must_use]
    #[inline]
    pub fn room(id: &str) -> Self {
        Self::new(PlanKind::Room, id)
    }

    /// The reference to the connection with this `id`, or with this
    /// `from->to` label when it carries none.
    #[must_use]
    #[inline]
    pub fn connection(id: &str) -> Self {
        Self::new(PlanKind::Connection, id)
    }

    /// The reference to a feature row by the room or connection `id` the
    /// row names.
    #[must_use]
    #[inline]
    pub fn feature(id: &str) -> Self {
        Self::new(PlanKind::Feature, id)
    }

    /// The row of `kind` named `id`.
    fn new(kind: PlanKind, id: &str) -> Self {
        Self {
            kind,
            id: id.to_owned(),
        }
    }

    /// The kind of row this cites.
    #[must_use]
    #[inline]
    pub const fn kind(&self) -> PlanKind {
        self.kind
    }

    /// The id the cited row carries (or, for a feature row, names).
    #[must_use]
    #[inline]
    pub fn id(&self) -> &str {
        self.id.as_str()
    }
}

impl CyberError {
    /// The `InvalidArgument` with an actionable `message`.
    #[must_use]
    #[inline]
    pub fn invalid(message: &str) -> Self {
        Self::InvalidArgument(message.to_owned())
    }

    /// The stable machine-readable code for this error variant.
    ///
    /// Clients see this as the `error` field of a failed structured tool
    /// result. A [`Self::WithHint`], [`Self::Emptied`] or [`Self::AtPlan`]
    /// wrapper reports its `inner`'s code.
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
                Self::WithHint { inner, .. }
                | Self::Emptied(inner)
                | Self::AtPlan { inner, .. } => *inner,
            };
        }
    }

    /// The wrapper facts of this refusal: its leaf variant under every
    /// wrapper, plus the [`Self::Emptied`] marker, the outermost
    /// [`Self::WithHint`] hint and the outermost [`Self::AtPlan`]
    /// attribution. As in [`Self::code`], walking the chain owns each step;
    /// this runs on an error path only.
    #[must_use]
    fn markers(&self) -> Markers {
        let mut walked = Markers::default();
        let mut current = self.clone();
        loop {
            match walked.record(current) {
                Some(inner) => current = inner,
                None => return walked,
            }
        }
    }

    /// Whether this is the [`Self::NotFound`] refusal, through every
    /// wrapper: the resource the caller asked for is simply not there.
    #[must_use]
    #[inline]
    pub fn is_not_found(&self) -> bool {
        matches!(self.markers().leaf, Some(Self::NotFound(_)))
    }

    /// Whether this is the [`Self::InvalidArgument`] refusal, through every
    /// wrapper: the caller's own parameters are what needs fixing.
    #[must_use]
    #[inline]
    pub fn is_invalid_argument(&self) -> bool {
        matches!(self.markers().leaf, Some(Self::InvalidArgument(_)))
    }

    /// Whether this is the [`Self::LimitExceeded`] refusal, through every
    /// wrapper: the caller has to narrow the request or raise its limit.
    #[must_use]
    #[inline]
    pub fn is_limit_exceeded(&self) -> bool {
        matches!(self.markers().leaf, Some(Self::LimitExceeded(_)))
    }

    /// Whether an image op raised this refusal because the picture was left
    /// with no content, which the caller answers with its own instruction.
    #[must_use]
    #[inline]
    pub fn is_emptied(&self) -> bool {
        self.markers().emptied
    }

    /// The human-readable message: the wrapped refusal's own text, which the
    /// `Display` implementation already prints through every wrapper.
    /// The [`Self::Emptied`] and [`Self::AtPlan`] markers change nothing the
    /// client reads.
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

    /// The hint to report to the client: the outermost attached
    /// [`Self::WithHint`] hint, else [`default_hint`] for this error's code.
    /// Structure wrappers are looked through, so an [`Self::AtPlan`] or
    /// [`Self::Emptied`] error around a hinted one still reports that hint.
    #[must_use]
    #[inline]
    pub fn hint(&self) -> String {
        match self.markers().hint {
            Some(hint) => hint,
            None => default_hint(self.code()).to_owned(),
        }
    }

    /// Attribute this refusal to the plan element `at`, keeping its code,
    /// message and hint: [`Self::plan_ref`] reads `at` back at the tool edge,
    /// which turns it into the wire path the caller is pointed at.
    #[must_use]
    #[inline]
    pub fn at_plan(self, at: PlanRef) -> Self {
        Self::AtPlan {
            inner: Box::new(self),
            at,
        }
    }

    /// The plan element this refusal was attributed to, through every
    /// wrapper.
    #[must_use]
    #[inline]
    pub fn plan_ref(&self) -> Option<PlanRef> {
        self.markers().at
    }

    /// Rebuild this error with its message text run through `change`, keeping
    /// the [`Self::code`] and any attached [`Self::with_hint`] hint. A
    /// [`Self::WithHint`] wrapper maps its `inner` and keeps its hint, so the
    /// message prefix reaches through the wrapper too; the [`Self::Emptied`]
    /// and [`Self::AtPlan`] wrappers keep their structure.
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
            Self::Emptied(inner) => Self::Emptied(Box::new(inner.map_message(change))),
            Self::AtPlan { inner, at } => Self::AtPlan {
                inner: Box::new(inner.map_message(change)),
                at,
            },
        }
    }
}

/// What one walk of a [`CyberError`]'s wrapper chain learns about it: the
/// leaf variant under every wrapper and the payload of each wrapper the
/// chain holds.
#[derive(Debug, Default)]
struct Markers {
    /// The variant the code, message and predicates answer from.
    leaf: Option<CyberError>,
    /// Whether a [`CyberError::Emptied`] marker sits in the chain.
    emptied: bool,
    /// The outermost [`CyberError::WithHint`]'s instruction.
    hint: Option<String>,
    /// The outermost [`CyberError::AtPlan`]'s element.
    at: Option<PlanRef>,
}

impl Markers {
    /// Records one step of the walk: a wrapper's payload is kept and the
    /// error it wraps is answered, while a leaf ends the walk with `None`.
    fn record(&mut self, step: CyberError) -> Option<CyberError> {
        match step {
            CyberError::Emptied(inner) => {
                self.emptied = true;
                Some(*inner)
            }
            CyberError::WithHint { inner, hint } => {
                self.keep_hint(hint);
                Some(*inner)
            }
            CyberError::AtPlan { inner, at } => {
                self.keep_at(at);
                Some(*inner)
            }
            leaf @ (CyberError::InvalidArgument(_)
            | CyberError::NotFound(_)
            | CyberError::AlreadyExists(_)
            | CyberError::Conflict(_)
            | CyberError::Format(_)
            | CyberError::Io(_)
            | CyberError::Fetch(_)
            | CyberError::Unsupported(_)
            | CyberError::LimitExceeded(_)
            | CyberError::Internal(_)) => {
                self.leaf = Some(leaf);
                None
            }
        }
    }

    /// Keeps `hint` when the walk has not met one yet: the outermost
    /// wrapper's instruction is the one the client gets.
    fn keep_hint(&mut self, hint: String) {
        if self.hint.is_none() {
            self.hint = Some(hint);
        }
    }

    /// Keeps `at` when the walk has not met an attribution yet.
    fn keep_at(&mut self, at: PlanRef) {
        if self.at.is_none() {
            self.at = Some(at);
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

    use super::{CyberError, PlanRef, default_hint};

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
    fn hint_reaches_through_the_structure_wrappers() {
        let marked = CyberError::invalid("nothing left after trim")
            .with_hint("check the source picture")
            .at_plan(PlanRef::room("hall"));
        assert_eq!(
            marked.hint(),
            "check the source picture",
            "the hinted inner keeps its hint under a plan attribution"
        );
        let plain = CyberError::invalid("nothing left after trim").at_plan(PlanRef::room("hall"));
        assert_eq!(
            plain.hint(),
            default_hint("invalid_argument"),
            "without a hint the code default answers through the wrapper"
        );
        let emptied = CyberError::Emptied(Box::new(
            CyberError::invalid("no pixel reaches alpha 128").with_hint("pass trim=false"),
        ));
        assert_eq!(
            emptied.hint(),
            "pass trim=false",
            "the emptied wrapper reports its inner's hint"
        );
    }

    #[test]
    fn the_predicates_answer_from_the_leaf_variant_through_every_wrapper() {
        let cases = [
            (CyberError::NotFound("gone".to_owned()), "not_found"),
            (
                CyberError::InvalidArgument("bad".to_owned()),
                "invalid_argument",
            ),
            (
                CyberError::LimitExceeded("too many".to_owned()),
                "limit_exceeded",
            ),
            (CyberError::Conflict("dirty".to_owned()), "conflict"),
        ];
        for (error, code) in cases {
            every_wrapper_of(error, code);
        }
    }

    /// Checks `error` itself and its hinted and attributed wrappers: the
    /// same code, the same predicate answers, three ways in.
    fn every_wrapper_of(error: CyberError, code: &str) {
        let hinted = error.clone().with_hint("do this instead");
        let attributed = error.clone().at_plan(PlanRef::connection("a->b"));
        for wrapped in [error, hinted, attributed] {
            assert_shape_of(&wrapped, code);
        }
    }

    /// Every wrapper-free answer one refusal of `code` must give: its own
    /// code, the matching predicate and no emptied marker.
    fn assert_shape_of(error: &CyberError, code: &str) {
        assert_eq!(error.code(), code, "code of {error:?}");
        assert_eq!(
            error.is_not_found(),
            code == "not_found",
            "is_not_found of {error:?}"
        );
        assert_eq!(
            error.is_invalid_argument(),
            code == "invalid_argument",
            "is_invalid_argument of {error:?}"
        );
        assert_eq!(
            error.is_limit_exceeded(),
            code == "limit_exceeded",
            "is_limit_exceeded of {error:?}"
        );
        assert!(
            !error.is_emptied(),
            "a leaf error is never the emptied marker: {error:?}"
        );
    }

    #[test]
    fn the_emptied_marker_keeps_the_refusal_it_wraps() {
        let problem = CyberError::invalid("ops[0] nothing left after trim");
        let marked = CyberError::Emptied(Box::new(problem.clone()));
        assert!(marked.is_emptied(), "the marker reads back");
        assert!(
            !problem.is_emptied(),
            "the wrapped refusal alone carries no marker"
        );
        assert_eq!(marked.code(), "invalid_argument", "the code stands");
        assert_eq!(
            marked.message(),
            "ops[0] nothing left after trim",
            "the message stands"
        );
        assert!(
            marked.is_invalid_argument(),
            "the marker delegates to the refusal it wraps"
        );
    }

    #[test]
    fn at_plan_carries_the_element_and_delegates_everything_else() {
        let error = CyberError::LimitExceeded("connection 'd1' needs 9999 steps".to_owned())
            .at_plan(PlanRef::connection("d1"));
        assert_eq!(error.code(), "limit_exceeded", "the code stands");
        assert_eq!(
            error.message(),
            "connection 'd1' needs 9999 steps",
            "the message stands"
        );
        assert!(error.is_limit_exceeded(), "the predicates look through");
        assert_eq!(
            error.plan_ref().as_ref().map(PlanRef::id),
            Some("d1"),
            "the element reads back"
        );
        assert!(
            !matches!(&error, CyberError::InvalidArgument(_)),
            "the wrapper hides nothing from Debug"
        );
    }

    #[test]
    fn plan_ref_reads_through_the_wrappers_and_answers_none_without_one() {
        let hinted = CyberError::invalid("room 'a' has a 0x0 rectangle")
            .at_plan(PlanRef::room("a"))
            .with_hint("give the room positive edges");
        assert_eq!(
            hinted.plan_ref().as_ref().map(PlanRef::id),
            Some("a"),
            "an attribution under a hint still reads"
        );
        let marked = CyberError::Emptied(Box::new(
            CyberError::invalid("nothing left").at_plan(PlanRef::feature("ghost")),
        ));
        assert_eq!(
            marked.plan_ref().as_ref().map(PlanRef::id),
            Some("ghost"),
            "an attribution under an emptied marker still reads"
        );
        assert_eq!(
            CyberError::invalid("no id here").plan_ref(),
            None,
            "an unattributed refusal carries no element"
        );
    }

    #[test]
    fn map_message_keeps_the_plan_attribution_and_the_emptied_marker() {
        let attributed = CyberError::invalid("room 'a' is too small")
            .at_plan(PlanRef::room("a"))
            .map_message(|old| format!("rooms[0]: {old}"));
        assert_eq!(
            attributed.message(),
            "rooms[0]: room 'a' is too small",
            "the text is mapped"
        );
        assert_eq!(
            attributed.plan_ref().as_ref().map(PlanRef::id),
            Some("a"),
            "the attribution rides along"
        );
        let marked = CyberError::Emptied(Box::new(CyberError::invalid("nothing left")))
            .map_message(|old| format!("note: {old}"));
        assert!(marked.is_emptied(), "the marker rides along");
        assert_eq!(marked.message(), "note: nothing left", "the text is mapped");
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
