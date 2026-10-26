//! The `textures` tool group as seen by a real MCP client.
//!
//! The helpers every test drives the server through live here: a connected
//! client with the fixture `IWAD` as its base, the undo-label list and the
//! `name` fields of a report list. The tests themselves are split by
//! topic into the `list`, `pnames` and `build` modules, which read
//! copy-on-write into the working archive and one-edit undo bookkeeping
//! through them.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::Value;

use crate::fixtures::iwad;
use crate::support::{call, connect, hex_of, path_arg};

/// A connected MCP client.
type Client = RunningService<RoleClient, ()>;

/// Load the fixture `IWAD` (patches, `PNAMES`, `TEXTURE1`) as the base.
async fn iwad_client(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// `get_session_info`'s undo-label list.
async fn undo_labels(client: &Client) -> Vec<String> {
    let info = call(client, "get_session_info", Value::Null).await;
    info["undo"]
        .as_array()
        .expect("undo is an array")
        .iter()
        .map(|entry| entry.as_str().expect("labels are strings").to_owned())
        .collect()
}

/// The `name` fields of a report list, in order.
fn listed(report: &Value, key: &str) -> Vec<String> {
    report[key]
        .as_array()
        .expect("the list is an array")
        .iter()
        .map(|row| row["name"].as_str().expect("names are strings").to_owned())
        .collect()
}

#[cfg(test)]
#[path = "textures/build.rs"]
mod build;
#[cfg(test)]
#[path = "textures/list.rs"]
mod list;
#[cfg(test)]
#[path = "textures/pnames.rs"]
mod pnames;
