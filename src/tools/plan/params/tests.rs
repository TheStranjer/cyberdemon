//! Shared builders and readers for the wire-parameter tests: the
//! one-room and one-connection requests, the `features`, `units` and
//! whole-plan splicers and the refusal reader every topic asserts with.
//! `rooms` runs the room and connection conversions, `features` the
//! feature rows and a room's own dressing fields, `units_things` the
//! `units` mapping, the snap grid and the thing rows.

mod features;
mod rooms;
mod units_things;

use serde_json::from_str;

use super::BuildPlanParams;

/// A one-room request with `body` spliced in as its `rooms`.
pub(super) fn rooms_request(body: &str) -> BuildPlanParams {
    let text = format!(r#"{{"map": "MAP01", "rooms": {body}, "connections": []}}"#);
    from_str(&text).expect("the request parses")
}

/// A one-connection request with `body` spliced in as its
/// `connections`.
pub(super) fn connections_request(body: &str) -> BuildPlanParams {
    let text = format!(
        r#"{{"map": "MAP01",
                "rooms": [{{"id": "a", "rect": {{"x": 0, "y": 0, "w": 64, "h": 64}}}}],
                "connections": {body}}}"#
    );
    from_str(&text).expect("the request parses")
}

/// The error code of `error`, for compact asserts.
fn code(error: &crate::error::CyberError) -> &str {
    error.code()
}

/// The message of a refusal that must be an `InvalidArgument`.
#[track_caller]
pub(super) fn refuse(error: &crate::error::CyberError) -> String {
    assert_eq!(code(error), "invalid_argument", "refusal code");
    error.to_string()
}
/// A request with `body` spliced in as its `features`.
pub(super) fn features_request(body: &str) -> BuildPlanParams {
    let text = format!(
        r#"{{"map": "MAP01",
                "rooms": [{{"id": "a", "rect": {{"x": 0, "y": 0, "w": 64, "h": 64}}}}],
                "connections": [], "features": {body}}}"#
    );
    from_str(&text).expect("the request parses")
}
/// A request with `extra` spliced in beside the required fields.
pub(super) fn request_with(extra: &str) -> BuildPlanParams {
    let text = format!(
        r#"{{"map": "MAP01",
                "rooms": [{{"id": "a", "rect": {{"x": 0, "y": 0, "w": 64, "h": 64}}}}],
                "connections": []{extra}}}"#
    );
    from_str(&text).expect("the request parses")
}
/// A one-room request with `body` spliced in beside the required
/// fields.
pub(super) fn plan_request(body: &str) -> BuildPlanParams {
    let text = format!(
        r#"{{"map": "MAP01",
                "rooms": [{{"id": "a", "rect": {{"x": 0, "y": 0, "w": 64, "h": 64}}}}],
                "connections": []{body}}}"#
    );
    from_str(&text).expect("the request parses")
}
