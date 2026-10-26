//! Theme-aware defaults over the Heretic fixture: `add_sector` without
//! flat names stores the `castle` theme's Heretic flats, and
//! `create_sector_from_loop` without `wall_texture` dresses its walls with
//! the theme's Heretic wall, while an untextured session keeps falling
//! back to the documented vanilla names.

use std::path::Path;

use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::edit_sectors::{square, working_linedef, working_sector_row};
use crate::fixtures::iwad_heretic;
use crate::support::{call, connect};

/// A connected client on the Heretic fixture (`E1M1`, `E2M1`).
async fn heretic_loaded(dir: &Path) -> RunningService<RoleClient, ()> {
    let client = connect().await;
    let base = iwad_heretic(dir);
    call(
        &client,
        "open_iwad",
        json!({"path": base.display().to_string()}),
    )
    .await;
    client
}

/// Add one named lump to the session, bytes and all: the inventory only
/// asks that the name lives in the right namespace.
async fn add(client: &RunningService<RoleClient, ()>, name: &str, namespace: &str) {
    call(
        client,
        "add_lump",
        json!({"name": name, "text": "theme probe bytes", "namespace": namespace}),
    )
    .await;
}

/// The theme rows the Heretic `castle` table names first for its roles.
async fn heretic_theme(client: &RunningService<RoleClient, ()>) -> Value {
    call(client, "get_texture_theme", json!({})).await
}

#[tokio::test]
async fn add_sector_defaults_to_the_heretic_theme_flats() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = heretic_loaded(dir.path()).await;
    add(&client, "FLOOR03", "flats").await;
    add(&client, "FLAT506", "flats").await;
    let theme = heretic_theme(&client).await;
    assert_eq!(theme["theme"], json!("castle"), "the Heretic default theme");
    let result = call(&client, "add_sector", json!({"map": "E1M1"})).await;
    assert_eq!(
        result["sector"]["floor_flat"],
        json!("FLOOR03"),
        "the castle theme's floor, not FLOOR4_8"
    );
    assert_eq!(
        result["sector"]["ceiling_flat"],
        json!("FLAT506"),
        "and ceiling"
    );
    let position = usize::try_from(result["index"].as_i64().expect("an index")).expect("small");
    let stored = working_sector_row(&client, "E1M1", position).await;
    assert_eq!(stored["floor_flat"], json!("FLOOR03"), "read back");
    assert_eq!(stored["ceiling_flat"], json!("FLAT506"));
}

#[tokio::test]
async fn create_sector_from_loop_defaults_to_the_heretic_theme_wall() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = heretic_loaded(dir.path()).await;
    call(
        &client,
        "build_texture",
        json!({"name": "GRSTNPB", "width": 64_u16, "height": 64_u16,
               "patches": [{"patch": "GRSTNPA", "x": 0_i32, "y": 0_i32}],
               "allow_missing_patches": true}),
    )
    .await;
    add(&client, "FLOOR03", "flats").await;
    add(&client, "FLAT506", "flats").await;
    let result = call(
        &client,
        "create_sector_from_loop",
        square(r#", "map": "E1M1", "outer_sector": "none""#),
    )
    .await;
    let created = result["linedefs_created"]
        .as_array()
        .expect("a list")
        .first()
        .cloned()
        .expect("a first wall");
    let position = usize::try_from(created.as_i64().expect("a numeric index")).expect("small");
    let line = working_linedef(&client, "E1M1", position).await;
    assert_eq!(
        line["front"]["middle"],
        json!("GRSTNPB"),
        "the castle theme's wall, not STARTAN3"
    );
    let sector =
        usize::try_from(result["sector"].as_i64().expect("a sector index")).expect("a small index");
    let room = working_sector_row(&client, "E1M1", sector).await;
    assert_eq!(room["floor_flat"], json!("FLOOR03"), "the theme flats too");
    assert_eq!(room["ceiling_flat"], json!("FLAT506"));
}

#[tokio::test]
async fn an_untextured_heretic_session_keeps_the_vanilla_wall_fallback() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = heretic_loaded(dir.path()).await;
    let result = call(
        &client,
        "create_sector_from_loop",
        square(r#", "map": "E1M1", "outer_sector": "none""#),
    )
    .await;
    let created = result["linedefs_created"]
        .as_array()
        .expect("a list")
        .first()
        .cloned()
        .expect("a first wall");
    let position = usize::try_from(created.as_i64().expect("a numeric index")).expect("small");
    let line = working_linedef(&client, "E1M1", position).await;
    assert_eq!(
        line["front"]["middle"],
        json!("STARTAN3"),
        "with no wall texture anywhere the documented fallback stands"
    );
}
