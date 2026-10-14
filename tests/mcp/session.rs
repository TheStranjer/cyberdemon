//! The `session` tool group as seen by a real MCP client: loading archives,
//! merging them, and what the two info tools report about the result.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Value, json};

    use crate::fixtures::{iwad, not_a_wad, pwad};
    use crate::support::{call, call_err, connect};

    /// The `{"path": ...}` argument object for `file`.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    /// Load the `IWAD` fixture as the session's base layer.
    async fn load_iwad(client: &rmcp::service::RunningService<rmcp::RoleClient, ()>, dir: &Path) {
        let base = iwad(dir);
        call(client, "open_iwad", path_arg(&base)).await;
    }

    #[tokio::test]
    async fn open_iwad_loads_the_base_layer_and_detects_the_game() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let client = connect().await;
        let report = call(&client, "open_iwad", path_arg(&base)).await;
        assert_eq!(
            report["path"],
            json!(base.display().to_string()),
            "echoed path"
        );
        assert_eq!(report["kind"], json!("IWAD"), "header magic");
        assert_eq!(report["lumps"].as_u64(), Some(23), "fixture lump count");
        assert_eq!(report["maps"].as_u64(), Some(1), "MAP01 only");
        assert_eq!(report["game"], json!("doom2"), "MAP01 identifies DOOM II");
    }

    #[tokio::test]
    async fn open_iwad_on_a_missing_file_is_not_found() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let (code, message) = call_err(
            &client,
            "open_iwad",
            path_arg(&dir.path().join("absent.wad")),
        )
        .await;
        assert_eq!(code, "not_found", "missing file code");
        assert!(
            message.contains("absent.wad"),
            "the message names the path: {message}"
        );
    }

    #[tokio::test]
    async fn open_iwad_on_a_relative_path_is_invalid_argument() {
        let client = connect().await;
        let (code, message) =
            call_err(&client, "open_iwad", json!({ "path": "some/doom2.wad" })).await;
        assert_eq!(code, "invalid_argument", "relative path code");
        assert!(
            message.contains("absolute"),
            "the message says what to pass: {message}"
        );
    }

    #[tokio::test]
    async fn open_iwad_on_a_non_wad_file_is_a_format_error() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let junk = not_a_wad(dir.path());
        let client = connect().await;
        let (code, message) = call_err(&client, "open_iwad", path_arg(&junk)).await;
        assert_eq!(code, "format_error", "not a WAD code");
        assert!(
            message.contains("magic"),
            "the message explains why it is not a WAD: {message}"
        );
    }

    #[tokio::test]
    async fn open_wad_adopts_the_file_as_working_archive() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let extra = pwad(dir.path());
        let client = connect().await;
        let report = call(&client, "open_wad", path_arg(&extra)).await;
        assert_eq!(
            report["path"],
            json!(extra.display().to_string()),
            "echoed path"
        );
        assert_eq!(report["kind"], json!("PWAD"), "header magic");
        assert_eq!(report["lumps"].as_u64(), Some(15), "fixture lump count");
        assert_eq!(report["maps"].as_u64(), Some(1), "MAP02 only");
        assert_eq!(report["dirty"], json!(false), "a fresh open is saved state");
        assert_eq!(
            report["game"],
            json!("unknown"),
            "no IWAD, and the merged MAP02 names no game"
        );
    }

    #[tokio::test]
    async fn open_wad_refuses_unsaved_changes_unless_told_to_discard() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let extra = pwad(dir.path());
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        call(
            &client,
            "merge_wad",
            json!({ "path": base.display().to_string(), "mode": "working" }),
        )
        .await;
        let (code, message) = call_err(&client, "open_wad", path_arg(&extra)).await;
        assert_eq!(code, "conflict", "dirty working archive code");
        assert!(
            message.contains("discard_changes"),
            "the message names the escape hatch: {message}"
        );
        let forced = call(
            &client,
            "open_wad",
            json!({ "path": extra.display().to_string(), "discard_changes": true }),
        )
        .await;
        assert_eq!(forced["dirty"], json!(false), "the new archive is clean");
        assert_eq!(forced["lumps"].as_u64(), Some(15), "and it is the fixture");
    }

    #[tokio::test]
    async fn new_wad_starts_an_empty_pwad_and_respects_the_dirty_guard() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let client = connect().await;
        let fresh = call(&client, "new_wad", json!({})).await;
        assert_eq!(fresh["kind"], json!("PWAD"), "an empty PWAD");
        assert_eq!(fresh["lumps"].as_u64(), Some(0), "with no lumps");
        call(
            &client,
            "merge_wad",
            json!({ "path": base.display().to_string(), "mode": "working" }),
        )
        .await;
        let (code, message) = call_err(&client, "new_wad", json!({})).await;
        assert_eq!(code, "conflict", "unsaved work must not vanish silently");
        assert!(
            message.contains("save_wad"),
            "the message names the proper way out: {message}"
        );
        let forced = call(&client, "new_wad", json!({ "discard_changes": true })).await;
        assert_eq!(
            forced["lumps"].as_u64(),
            Some(0),
            "discarded and empty again"
        );
    }

    #[tokio::test]
    async fn merge_wad_layer_stacks_the_archive_and_overrides_in_place() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let extra = pwad(dir.path());
        let report = call(&client, "merge_wad", path_arg(&extra)).await;
        assert_eq!(report["mode"], json!("layer"), "layer mode by default");
        assert_eq!(
            report["layer_index"].as_u64(),
            Some(1),
            "the resource sits above the IWAD"
        );
        assert_eq!(report["lumps"].as_u64(), Some(15), "the layered archive");
        let info = call(&client, "get_wad_info", Value::Null).await;
        assert_eq!(
            info["total_lumps"].as_u64(),
            Some(35),
            "FLOOR1 replaced, not doubled"
        );
        assert_eq!(
            info["namespaces"],
            json!({ "flats": 1_u64, "patches": 2_u64 }),
            "the merged flats region holds one FLOOR1, the patches region both fixtures' patches"
        );
    }

    #[tokio::test]
    async fn merge_wad_working_folds_the_file_in_as_one_undoable_edit() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let extra = pwad(dir.path());
        let client = connect().await;
        call(&client, "open_iwad", path_arg(&base)).await;
        call(&client, "new_wad", json!({})).await;
        let first = call(
            &client,
            "merge_wad",
            json!({ "path": base.display().to_string(), "mode": "working" }),
        )
        .await;
        assert_eq!(first["mode"], json!("working"), "working mode");
        assert_eq!(
            first["replaced"].as_u64(),
            Some(0),
            "an empty base overrides nothing"
        );
        assert_eq!(
            first["added"].as_u64(),
            Some(9),
            "five globals, one flat, two patches, one map"
        );
        let second = call(
            &client,
            "merge_wad",
            json!({ "path": extra.display().to_string(), "mode": "working" }),
        )
        .await;
        assert_eq!(second["replaced"].as_u64(), Some(1), "the FLOOR1 flat");
        assert_eq!(
            second["added"].as_u64(),
            Some(2),
            "DEHACKED and the MAP02 block"
        );
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["working"]["dirty"],
            json!(true),
            "folding dirties the archive"
        );
        assert_eq!(
            session["undo"],
            json!(["merge_wad iwad-doom2.wad", "merge_wad pwad-extra.wad"]),
            "one labelled edit per merge, newest last"
        );
        assert_eq!(session["redo"], json!([]), "nothing has been undone");
        let info = call(&client, "get_wad_info", Value::Null).await;
        assert_eq!(info["maps"][0]["name"], json!("MAP01"), "the merged maps");
        assert_eq!(
            info["maps"][0]["source"],
            json!("working"),
            "the working copy overrode the base layer"
        );
    }

    #[tokio::test]
    async fn merge_wad_rejects_an_unknown_mode_and_a_missing_file() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let extra = pwad(dir.path());
        let (code, message) = call_err(
            &client,
            "merge_wad",
            json!({ "path": extra.display().to_string(), "mode": "sideways" }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "unknown mode code");
        assert!(
            message.contains("\"layer\"") && message.contains("\"working\""),
            "the error names both accepted modes: {message}"
        );
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["layers"].as_array().expect("layers").len(),
            1,
            "the refusal stacked nothing"
        );
        let (missing_code, _) = call_err(
            &client,
            "merge_wad",
            path_arg(&dir.path().join("absent.wad")),
        )
        .await;
        assert_eq!(
            missing_code, "not_found",
            "merge reads through the same loader"
        );
    }

    #[tokio::test]
    async fn get_session_info_on_a_fresh_server_is_all_defaults() {
        let client = connect().await;
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(session["iwad"], Value::Null, "no base archive");
        assert_eq!(session["layers"], json!([]), "nothing stacked");
        assert_eq!(session["working"]["path"], Value::Null, "never saved");
        assert_eq!(
            session["working"]["kind"],
            json!("PWAD"),
            "the default kind"
        );
        assert_eq!(session["working"]["lumps"].as_u64(), Some(0), "empty");
        assert_eq!(session["working"]["dirty"], json!(false), "nothing to save");
        assert_eq!(session["active_map"], Value::Null, "no map selected");
        assert_eq!(
            session["game"],
            json!("unknown"),
            "an empty session identifies no game"
        );
        assert_eq!(session["undo"], json!([]), "no history");
        assert_eq!(session["redo"], json!([]), "nothing undone");
    }

    #[tokio::test]
    async fn get_session_info_after_loads_shows_every_layer() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let extra = pwad(dir.path());
        let client = connect().await;
        call(&client, "open_iwad", path_arg(&base)).await;
        call(&client, "merge_wad", path_arg(&extra)).await;
        call(&client, "open_wad", path_arg(&base)).await;
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["iwad"]["path"],
            json!(base.display().to_string()),
            "the base layer"
        );
        assert_eq!(
            session["iwad"]["lumps"].as_u64(),
            Some(23),
            "the fixture's directory"
        );
        let layers = session["layers"].as_array().expect("a layer list");
        assert_eq!(layers.len(), 2, "IWAD plus one resource");
        assert_eq!(layers[0]["role"], json!("iwad"), "layer 0 is the base");
        assert_eq!(layers[1]["role"], json!("resource"), "layer 1 stacks on it");
        assert_eq!(
            layers[1]["path"],
            json!(extra.display().to_string()),
            "its file"
        );
        assert_eq!(layers[1]["lumps"].as_u64(), Some(15), "its directory size");
        assert_eq!(
            session["working"]["path"],
            json!(base.display().to_string()),
            "the working archive came from a file"
        );
        assert_eq!(
            session["working"]["dirty"],
            json!(false),
            "an open starts clean"
        );
        assert_eq!(session["game"], json!("doom2"), "the IWAD layer decides");
    }

    #[tokio::test]
    async fn get_wad_info_summarizes_the_merged_view() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        load_iwad(&client, dir.path()).await;
        let extra = pwad(dir.path());
        call(&client, "merge_wad", path_arg(&extra)).await;
        let info = call(&client, "get_wad_info", Value::Null).await;
        assert_eq!(info["game"], json!("doom2"), "detected from the IWAD layer");
        assert_eq!(
            info["iwad"].as_str().expect("an iwad path"),
            dir.path().join("iwad-doom2.wad").display().to_string(),
            "the base is named once"
        );
        assert_eq!(
            info["pwads"],
            json!([extra.display().to_string(), "(unsaved working archive)",]),
            "resource layers first, the working archive last"
        );
        assert_eq!(
            info["total_lumps"].as_u64(),
            Some(35),
            "the merged directory"
        );
        assert_eq!(
            info["maps"],
            json!([
                { "name": "MAP01", "format": "doom", "source": { "layer": 0_u64 } },
                { "name": "MAP02", "format": "doom", "source": { "layer": 1_u64 } },
            ]),
            "both layers contribute a map"
        );
        assert_eq!(
            info["namespaces"],
            json!({ "flats": 1_u64, "patches": 2_u64 }),
            "one flat wins, the patches region rides along"
        );
        assert_eq!(info["warnings"], json!([]), "the fixtures are well formed");
    }
}
