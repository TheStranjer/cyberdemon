//! The `defs` tool group as seen by a real MCP client: the embedded game
//! definition tables, the session-detected default game, the filters, the
//! Boom generalized and sector decodings, and the session custom definitions
//! (DEHACKED, DECORATE, `ZScript` and MAPINFO).
//!
//! The helpers the tests share live here: the generalized floor value, the
//! type and special numbers of a report, the single-row and warning lookups
//! and the client with the custom-definitions PWAD stacked on the fixture
//! `IWAD`. The tests themselves are split by topic into the `things`,
//! `custom`, `specials` and `sectors` modules, covering the thing tables with
//! their filters and the detected default game, the custom definitions joining
//! those tables, the linedef special tables with their generalized decoding
//! and family override, and the sector type tables with their Boom decoding.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::{iwad, pwad_custom_defs};
use crate::support::{call, connect};

/// A connected MCP client.
type Client = RunningService<RoleClient, ()>;

/// A generalized floor value: `0x6000 | WR | fast | up | lowest
/// neighbour ceiling` (bits `1 | (2 << 3) | (1 << 6) | (3 << 7)`),
/// combined without bare operators.
fn generalized_floor() -> u64 {
    0x6000_u64
        .checked_add(1)
        .and_then(|v| v.checked_add(2_u64.checked_shl(3).expect("shift fits")))
        .and_then(|v| v.checked_add(1_u64.checked_shl(6).expect("shift fits")))
        .and_then(|v| v.checked_add(3_u64.checked_shl(7).expect("shift fits")))
        .expect("the fixture value fits")
}

/// The type numbers of a report's `things` array.
fn types(report: &Value) -> Vec<u64> {
    report["things"]
        .as_array()
        .expect("things is an array")
        .iter()
        .map(|entry| {
            entry["type"]
                .as_u64()
                .expect("every entry has a type number")
        })
        .collect()
}

/// A session with the DOOM II fixture as base and the custom-definitions
/// PWAD stacked as a resource layer; returns the client.
async fn custom_defs_client(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    let extra = pwad_custom_defs(dir);
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    call(
        &client,
        "merge_wad",
        json!({ "path": extra.display().to_string() }),
    )
    .await;
    client
}

/// The single row of a `type` lookup.
fn single_row(report: &Value) -> Value {
    assert_eq!(report["count"].as_u64(), Some(1), "exactly one row");
    report["things"].get(0).expect("one row").clone()
}

/// Whether `warnings` holds a text containing `needle`.
fn warns(warnings: &Value, needle: &str) -> bool {
    warnings
        .as_array()
        .expect("warnings is an array")
        .iter()
        .filter_map(Value::as_str)
        .any(|text| text.contains(needle))
}

/// The special numbers of a report's `specials` array.
fn specials_of(report: &Value) -> Vec<u64> {
    report["specials"]
        .as_array()
        .expect("specials is an array")
        .iter()
        .map(|entry| {
            entry["special"]
                .as_u64()
                .expect("every entry has a special number")
        })
        .collect()
}

/// The row with this `special` number in a report, or panic.
fn special_row(report: &Value, number: i64) -> Value {
    report["specials"]
        .as_array()
        .expect("specials is an array")
        .iter()
        .find(|entry| entry["special"] == json!(number))
        .cloned()
        .unwrap_or_else(|| panic!("special {number} is listed"))
}

#[cfg(test)]
#[path = "defs/custom.rs"]
mod custom;
#[cfg(test)]
#[path = "defs/sectors.rs"]
mod sectors;
#[cfg(test)]
#[path = "defs/specials.rs"]
mod specials;
#[cfg(test)]
#[path = "defs/things.rs"]
mod things;
