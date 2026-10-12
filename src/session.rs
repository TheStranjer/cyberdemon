//! The per-connection editing session.

/// State of a single client connection's editing session.
///
/// An editing session spans everything a client does over one stdio
/// connection: which WAD is open, pending edits and similar. This is a
/// placeholder; a later ticket fills in the fields. Default-constructing a
/// session is covered by the [`crate::state::AppState`] tests, which build it
/// through [`crate::state::AppState::with_session`]. It is only ever reached
/// through that method.
#[derive(Debug, Default)]
pub struct Session;
