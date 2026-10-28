//! The `playability` tool group as seen by a real MCP client, split by
//! topic: the `check` module drives `check_playability` over the
//! plan-built maps and the two-room fixture - the played-out blueprint is
//! `ok`, the keyless one holds at its `locked` blocker, a `hexen` plan
//! door turned to `monstercross` activation keeps the walk out of its
//! wing, the ledge blocks until `add_stairs`, the wall-shoved imp reports
//! `in_wall`, the missing start and exit answer `ok: false` with the
//! fixing tool named, and `render: true` publishes the highlighted PNG;
//! the `describe` module reads `describe_map` rows, roles, filters,
//! paging and refusals over the same fixtures.
//!
//! The shared clients and the `build_map_from_plan` report readers live
//! here; the tests themselves are split by topic into the `check` and
//! `describe` modules.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::{iwad_features, iwad_hexen};
use crate::support::{call, connect};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// Open the features `IWAD` (textured two-room `MAP01`) as layer 0.
async fn features_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_features(dir);
    call(
        &client,
        "open_iwad",
        json!({"path": base.display().to_string()}),
    )
    .await;
    client
}

/// Open the Hexen `IWAD` fixture (plain two-room `MAP01`, detected
/// `Hexen`) as layer 0.
async fn hexen_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_hexen(dir);
    call(
        &client,
        "open_iwad",
        json!({"path": base.display().to_string()}),
    )
    .await;
    client
}

/// The one sector `build_map_from_plan` reported for room `room`.
fn built_room(built: &Value, room: &str) -> u64 {
    built["rooms"][room]
        .as_array()
        .expect("a room sector list")
        .first()
        .expect("a plan room owns one sector")
        .as_u64()
        .expect("an index")
}

/// The band sector `build_map_from_plan` reported for connection
/// `index` (zero-based plan order).
fn built_band(built: &Value, index: usize) -> u64 {
    built["connections"]
        .as_array()
        .expect("a connection list")
        .get(index)
        .expect("a connection")["sectors"]
        .as_array()
        .expect("a sector list")
        .first()
        .expect("one band")
        .as_u64()
        .expect("an index")
}

#[cfg(test)]
#[path = "playability/check.rs"]
mod check;
#[cfg(test)]
#[path = "playability/describe.rs"]
mod describe;
