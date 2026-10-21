//! The `save_wad` tool as seen by a real MCP client: saving the working
//! archive, exporting the merged view, and every refusal path.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::wad::{LumpName, Wad, WadKind};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad};
    use crate::support::{call, call_err, connect};

    /// The `{"path": ...}` argument object for `file`.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    /// The `{"path": ..., "mode": ...}` argument object for `file` and
    /// `mode` (`full` exports, anything else saves).
    fn mode_arg(file: &Path, mode: &str) -> Value {
        json!({ "path": file.display().to_string(), "mode": mode })
    }

    /// The directory names of an archive file, in order.
    fn names(path: &Path) -> Vec<LumpName> {
        let wad = Wad::read_file(path).expect("test archives are valid WADs");
        wad.lumps.iter().map(|lump| lump.name).collect()
    }

    /// The working archive as it currently sits on disk, by its session
    /// path: `{"path": ..., "mode": ...}` for a later `merge_wad` call.
    fn fold_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string(), "mode": "working" })
    }

    /// Stack the fixtures: `IWAD` base, `PWAD` resource layer, and a
    /// `PWAD` folded into the working archive so the session is dirty.
    async fn load_every_layer(client: &RunningService<RoleClient, ()>, dir: &Path) {
        let base = iwad(dir);
        let extra = pwad(dir);
        call(client, "open_iwad", path_arg(&base)).await;
        call(client, "merge_wad", path_arg(&extra)).await;
        call(client, "merge_wad", fold_arg(&extra)).await;
    }

    #[tokio::test]
    async fn save_wad_writes_the_working_archive_and_marks_it_saved() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let extra = pwad(dir.path());
        let out = dir.path().join("out.wad");
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        call(&client, "merge_wad", fold_arg(&extra)).await;
        let report = call(&client, "save_wad", path_arg(&out)).await;
        assert_eq!(report["mode"], json!("pwad"), "pwad mode by default");
        assert_eq!(report["kind"], json!("PWAD"), "the working magic");
        assert_eq!(report["lumps"].as_u64(), Some(15), "the folded fixture");
        assert_eq!(report["path"], path_arg(&out)["path"], "echoed target");
        assert_eq!(report["dirty"], json!(false), "the save cleared dirty");
        let bytes = report["bytes"].as_u64().expect("a byte count");
        let written = usize::try_from(bytes).expect("test files are small");
        assert_eq!(
            written,
            std::fs::read(&out).expect("the file").len(),
            "bytes match"
        );
        assert_eq!(
            names(&out),
            names(&extra),
            "the saved archive holds the same lumps as the fixture folded in"
        );
    }

    #[tokio::test]
    async fn a_saved_archive_reopens_with_the_same_lumps_and_clears_the_session() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let extra = pwad(dir.path());
        let out = dir.path().join("out.wad");
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        call(&client, "merge_wad", fold_arg(&extra)).await;
        call(&client, "save_wad", path_arg(&out)).await;
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["working"]["path"],
            json!(out.display().to_string()),
            "the session remembers where it saved"
        );
        assert_eq!(
            session["working"]["dirty"],
            json!(false),
            "and considers itself saved"
        );
        let reopened = call(&client, "open_wad", path_arg(&out)).await;
        assert_eq!(reopened["lumps"].as_u64(), Some(15), "it re-opens");
        assert_eq!(names(&out), names(&extra), "with the same lump names");
    }

    #[tokio::test]
    async fn save_wad_without_a_path_reuses_the_opened_file() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let extra = pwad(dir.path());
        let client = connect().await;
        call(&client, "open_wad", path_arg(&extra)).await;
        call(&client, "merge_wad", fold_arg(&extra)).await;
        let report = call(&client, "save_wad", json!({})).await;
        assert_eq!(
            report["path"],
            json!(extra.display().to_string()),
            "the opened path is the default target"
        );
        assert_eq!(report["dirty"], json!(false), "the save cleared the merge");
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["working"]["path"],
            json!(extra.display().to_string()),
            "and stays pointed there"
        );
    }

    #[tokio::test]
    async fn save_wad_without_any_known_path_is_invalid_argument() {
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        let (code, message, _) = call_err(&client, "save_wad", json!({})).await;
        assert_eq!(code, "invalid_argument", "no path anywhere code");
        assert!(
            message.contains("pass path"),
            "the message says what to do: {message}"
        );
    }

    #[tokio::test]
    async fn save_wad_full_exports_the_merged_view_as_one_iwad() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let out = dir.path().join("full.wad");
        let client = connect().await;
        load_every_layer(&client, dir.path()).await;
        let report = call(&client, "save_wad", mode_arg(&out, "full")).await;
        assert_eq!(report["mode"], json!("full"), "full mode");
        assert_eq!(
            report["kind"],
            json!("IWAD"),
            "an IWAD layer is loaded, so the export is a full game archive"
        );
        assert_eq!(
            report["lumps"].as_u64(),
            Some(45),
            "IWAD plus the PWAD's contributions, FLOOR1 replaced in place"
        );
        assert_eq!(report["dirty"], json!(true), "an export is not a save");
        let wad = Wad::read_file(&out).expect("the export is a valid WAD");
        assert_eq!(wad.kind, WadKind::Iwad, "the written magic");
        let flat = wad
            .find_last(parsed_name("FLOOR1"))
            .and_then(|index| wad.lumps.get(index))
            .expect("FLOOR1 is there");
        assert_eq!(flat.size(), 4096, "the override flat's size");
        assert!(
            flat.data.iter().all(|byte| *byte == 2_u8),
            "the merged view took the PWAD's FLOOR1"
        );
        let saved = names(&out);
        let missing: Vec<&str> = ["PLAYPAL", "ENDOOM", "MAP01", "MAP02", "DEHACKED"]
            .into_iter()
            .filter(|wanted| !saved.contains(&parsed_name(wanted)))
            .collect();
        assert!(
            missing.is_empty(),
            "the export is missing {}",
            missing.join(", ")
        );
    }

    #[tokio::test]
    async fn save_wad_full_leaves_the_working_path_and_dirty_state_untouched() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let out = dir.path().join("full.wad");
        let client = connect().await;
        load_every_layer(&client, dir.path()).await;
        call(&client, "save_wad", mode_arg(&out, "full")).await;
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["working"]["dirty"],
            json!(true),
            "the working archive stays dirty"
        );
        assert_eq!(
            session["working"]["path"],
            Value::Null,
            "and never gains a path from an export"
        );
    }

    /// A valid parsed name for export checks.
    fn parsed_name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    #[tokio::test]
    async fn save_wad_kind_override_writes_the_requested_magic() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let out = dir.path().join("as-iwad.wad");
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        let mut args = path_arg(&out);
        args["kind"] = json!("IWAD");
        let report = call(&client, "save_wad", args).await;
        assert_eq!(report["kind"], json!("IWAD"), "the override is reported");
        assert_eq!(report["mode"], json!("pwad"), "still a plain save");
        let wad = Wad::read_file(&out).expect("the save is a valid WAD");
        assert_eq!(wad.kind, WadKind::Iwad, "the written magic");
        assert_eq!(wad.len(), 0, "an empty working archive saves bare");
        let session = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            session["working"]["kind"],
            json!("PWAD"),
            "the session's own archive keeps its kind"
        );
    }

    #[tokio::test]
    async fn save_wad_refuses_to_overwrite_a_loaded_iwad_layer() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let client = connect().await;
        call(&client, "open_iwad", path_arg(&base)).await;
        let (code, message, _) = call_err(&client, "save_wad", path_arg(&base)).await;
        assert_eq!(code, "conflict", "the IWAD layer is protected");
        assert!(
            message.contains("IWAD") && message.contains("pass a different path"),
            "the message names the role and the way out: {message}"
        );
    }

    #[tokio::test]
    async fn save_wad_refuses_to_overwrite_a_resource_layer() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let extra = pwad(dir.path());
        let client = connect().await;
        call(&client, "merge_wad", path_arg(&extra)).await;
        let (code, message, _) = call_err(&client, "save_wad", path_arg(&extra)).await;
        assert_eq!(code, "conflict", "resource layers are protected too");
        assert!(
            message.contains("resource"),
            "the message names the role: {message}"
        );
    }

    #[tokio::test]
    async fn save_wad_reports_a_missing_parent_directory_as_not_found() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        call(&client, "new_wad", json!({})).await;
        let (code, message, _) = call_err(
            &client,
            "save_wad",
            path_arg(&dir.path().join("gone").join("out.wad")),
        )
        .await;
        assert_eq!(code, "not_found", "missing parent code");
        assert!(
            message.contains("parent directory"),
            "the message says what is missing: {message}"
        );
    }

    #[tokio::test]
    async fn save_wad_rejects_a_relative_path() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let extra = pwad(dir.path());
        let client = connect().await;
        call(&client, "open_wad", path_arg(&extra)).await;
        let (code, message, _) = call_err(&client, "save_wad", json!({ "path": "out.wad" })).await;
        assert_eq!(code, "invalid_argument", "relative path code");
        assert!(
            message.contains("absolute"),
            "the message says what to pass: {message}"
        );
    }

    #[tokio::test]
    async fn save_wad_rejects_an_unknown_mode() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let out = dir.path().join("out.wad");
        let client = connect().await;
        let (code, message, _) = call_err(&client, "save_wad", mode_arg(&out, "sideways")).await;
        assert_eq!(code, "invalid_argument", "unknown mode code");
        assert!(
            message.contains("\"pwad\"") && message.contains("\"full\""),
            "the error names both accepted modes: {message}"
        );
        assert!(!out.exists(), "the refusal wrote nothing");
    }

    #[tokio::test]
    async fn save_wad_rejects_an_unknown_kind() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let out = dir.path().join("out.wad");
        let client = connect().await;
        let mut args = path_arg(&out);
        args["kind"] = json!("EXE");
        let (code, message, _) = call_err(&client, "save_wad", args).await;
        assert_eq!(code, "invalid_argument", "unknown kind code");
        assert!(
            message.contains("\"PWAD\"") && message.contains("\"IWAD\""),
            "the error names both accepted kinds: {message}"
        );
        assert!(!out.exists(), "the refusal wrote nothing");
    }
}
