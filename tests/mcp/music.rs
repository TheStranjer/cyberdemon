//! The `music` tool group as seen by a real MCP client: `import_music`
//! sniffing containers, converting `MIDI` to vanilla `MUS` `D_*` lumps,
//! validating and storing `MUS` as-is, passing other formats through, and
//! every refusal path leaving the session untouched.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use cyberdemon::music::MidiWriter;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// Bytes of a `MUS` header ahead of its score fields.
    const MUS_HEADER: usize = 16;
    /// Score bytes of the one-note `MUS` fixture.
    const MUS_SCORE: [u8; 6] = [0x10_u8, 0x3C, 0x80, 0x3C, 0x0A, 0x60];
    /// Score bytes the one-note `MIDI` fixture converts to: a volume-carrying
    /// note on, the 70-tick delay closing its group, the release and the
    /// score end.
    const CONVERTED_SCORE: usize = 7;

    /// The `{"source": ...}` argument object for the absolute `file`.
    fn source_arg(file: &Path) -> Value {
        json!({ "source": file.display().to_string() })
    }

    /// `get_session_info`'s undo-label list.
    async fn undo_labels(client: &Client) -> Vec<String> {
        let info = call(client, "get_session_info", Value::Null).await;
        info["undo"]
            .as_array()
            .expect("undo is an array")
            .iter()
            .map(|label| label.as_str().expect("labels are strings").to_owned())
            .collect()
    }

    /// The working archive's lump names, in directory order.
    async fn working_names(client: &Client) -> Vec<String> {
        let listing = call(client, "list_lumps", json!({ "scope": "working" })).await;
        listing["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .map(|row| row["name"].as_str().expect("names are strings").to_owned())
            .collect()
    }

    /// The first `count` bytes of the working lump `name`, as `get_lump`
    /// formats them.
    async fn lump_hex(client: &Client, name: &str, count: u64) -> String {
        let read = call(
            client,
            "get_lump",
            json!({ "name": name, "scope": "working", "format": "hex", "length": count }),
        )
        .await;
        read["hex"].as_str().expect("hex is a string").to_owned()
    }

    /// Reads a lump's raw bytes through `extract_lump`.
    async fn lump_bytes(client: &Client, name: &str, dir: &Path, tag: &str) -> Vec<u8> {
        let target = dir.join(format!("read-{tag}.bin"));
        call(
            client,
            "extract_lump",
            json!({
                "name": name,
                "scope": "working",
                "path": target.display().to_string(),
                "overwrite": true,
            }),
        )
        .await;
        fs::read(&target).expect("read the extracted lump back")
    }

    /// A session a refused import left untouched: no lumps, no history.
    async fn assert_untouched(client: &Client) {
        let listing = call(client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(
            listing["total"],
            json!(0_u64),
            "nothing in the working archive"
        );
        assert!(undo_labels(client).await.is_empty(), "and nothing recorded");
    }

    /// One fixture `MIDI` event: delta, status byte, two data bytes.
    type FixtureEvent = (u32, u8, [u8; 2]);

    /// Writes a type-0 `MIDI` of `events` at 70 ticks per quarter, named
    /// `{stem}.mid`, and returns its path.
    fn write_midi(dir: &Path, stem: &str, events: &[FixtureEvent]) -> PathBuf {
        let mut midi = MidiWriter::new();
        midi.put_tempo(0_u32, 500_000_u32);
        for &(delta, status, data) in events {
            midi.put_event(delta, status, &data);
        }
        let bytes = midi.into_type0(70_u16, 0_u32).expect("the fixture encodes");
        let path = dir.join(format!("{stem}.mid"));
        fs::write(&path, bytes).expect("write the fixture");
        path
    }

    /// The one-note track body every `MIDI` fixture shares.
    const ONE_NOTE: [FixtureEvent; 2] = [
        (0_u32, 0x90_u8, [0x3C_u8, 0x64_u8]),
        (70_u32, 0x80_u8, [0x3C_u8, 0x00_u8]),
    ];

    /// Writes a minimal `MUS` lump (the header plus [`MUS_SCORE`]), named
    /// `{stem}.mu`, and returns its path.
    fn write_mus(dir: &Path, stem: &str) -> PathBuf {
        let score_len = u16::try_from(MUS_SCORE.len()).expect("the fixture score fits");
        let start = u16::try_from(MUS_HEADER).expect("the fixture header fits");
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"MUS\x1A");
        out.extend_from_slice(&score_len.to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        for _word in 0_usize..4 {
            out.extend_from_slice(&0_u16.to_le_bytes());
        }
        out.extend_from_slice(&MUS_SCORE);
        let path = dir.join(format!("{stem}.mu"));
        fs::write(&path, out).expect("write the fixture");
        path
    }

    /// Writes `head` plus zero padding as `{stem}.{ext}` and returns the
    /// path: containers only `passthrough` needs never get parsed.
    fn write_stuffed(dir: &Path, stem: &str, ext: &str, head: &[u8]) -> PathBuf {
        let mut out = head.to_vec();
        out.extend_from_slice(&[0_u8; 32]);
        let path = dir.join(format!("{stem}.{ext}"));
        fs::write(&path, out).expect("write the fixture");
        path
    }

    #[tokio::test]
    async fn a_midi_file_defaults_to_a_converted_mus_lump() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let midi = write_midi(dir.path(), "runnin", &ONE_NOTE);
        let report = call(&client, "import_music", source_arg(&midi)).await;
        assert_eq!(report["name"], json!("D_RUNNIN"), "D_ + the stem");
        assert_eq!(report["input_format"], json!("midi"), "MThd sniffed");
        assert_eq!(
            report["stored_format"],
            json!("mus"),
            "the default for MIDI"
        );
        let expected = MUS_HEADER.saturating_add(CONVERTED_SCORE);
        assert_eq!(report["bytes"], json!(expected), "header plus the score");
        assert_eq!(report["warnings"], json!([]), "the note maps as-is");
        assert_eq!(
            lump_hex(&client, "D_RUNNIN", 4).await,
            "4D 55 53 1A",
            "the lump starts with the MUS magic"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["import_music D_RUNNIN"],
            "one undoable edit"
        );
    }

    #[tokio::test]
    async fn a_conversion_reports_what_mus_cannot_carry() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let mut events = vec![(0_u32, 0xB0_u8, [0x02_u8, 0x09_u8])];
        events.extend_from_slice(&ONE_NOTE);
        let midi = write_midi(dir.path(), "bends", &events);
        let report = call(&client, "import_music", source_arg(&midi)).await;
        let warnings = report["warnings"].as_array().expect("an array").clone();
        assert_eq!(
            warnings,
            vec![json!("dropped 1 control changes with no MUS counterpart")],
            "CC 2 has no MUS number"
        );
    }

    #[tokio::test]
    async fn a_mus_file_is_validated_and_stored_unchanged() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let mus = write_mus(dir.path(), "e1m1");
        let report = call(&client, "import_music", source_arg(&mus)).await;
        assert_eq!(report["name"], json!("D_E1M1"), "D_ + the stem");
        assert_eq!(report["input_format"], json!("mus"), "the magic sniffed");
        assert_eq!(report["stored_format"], json!("mus"), "MUS stays MUS");
        let expected = MUS_HEADER.saturating_add(MUS_SCORE.len());
        assert_eq!(report["bytes"], json!(expected), "the file size");
        let stored = lump_bytes(&client, "D_E1M1", dir.path(), "e1m1").await;
        let original = fs::read(&mus).expect("read the fixture back");
        assert_eq!(stored, original, "byte for byte identical");
    }

    #[tokio::test]
    async fn a_midi_imported_as_passthrough_keeps_its_mthd_bytes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let midi = write_midi(dir.path(), "rawrun", &ONE_NOTE);
        let mut args = source_arg(&midi);
        args["format"] = json!("passthrough");
        let report = call(&client, "import_music", args).await;
        assert_eq!(report["input_format"], json!("midi"), "still MIDI");
        assert_eq!(report["stored_format"], json!("passthrough"), "the choice");
        assert_eq!(report["name"], json!("D_RAWRUN"), "names keep the prefix");
        let stored = lump_bytes(&client, "D_RAWRUN", dir.path(), "raw").await;
        let original = fs::read(&midi).expect("read the fixture back");
        assert_eq!(stored, original, "the SMF bytes survive");
        assert_eq!(report["bytes"], json!(original.len()), "the file size");
        assert_eq!(
            lump_hex(&client, "D_RAWRUN", 4).await,
            "4D 54 68 64",
            "MThd"
        );
    }

    #[tokio::test]
    async fn an_ogg_file_defaults_to_passthrough() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let ogg = write_stuffed(dir.path(), "song", "ogg", b"OggS\x01\x02");
        let report = call(&client, "import_music", source_arg(&ogg)).await;
        assert_eq!(report["name"], json!("D_SONG"), "D_ + the stem");
        assert_eq!(report["input_format"], json!("ogg"), "the magic sniffed");
        assert_eq!(
            report["stored_format"],
            json!("passthrough"),
            "OGG cannot become MUS"
        );
        let expected = ogg.metadata().expect("the fixture exists").len();
        assert_eq!(report["bytes"], json!(expected), "the file size");
    }

    #[tokio::test]
    async fn an_ogg_refuses_to_be_stored_as_mus() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let ogg = write_stuffed(dir.path(), "song", "ogg", b"OggS\x01\x02");
        let mut args = source_arg(&ogg);
        args["format"] = json!("mus");
        let (code, message) = call_err(&client, "import_music", args).await;
        assert_eq!(code, "invalid_argument", "OGG is not MUS-able");
        assert!(
            message.contains("passthrough"),
            "the message names the fix: {message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn unknown_bytes_and_a_corrupt_mus_are_format_errors() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let junk = dir.path().join("mystery.dat");
        fs::write(&junk, b"total nonsense, no music here").expect("write the junk");
        let (code, message) = call_err(&client, "import_music", source_arg(&junk)).await;
        assert_eq!(code, "format_error", "unrecognized container");
        assert!(
            message.contains("MUS, MIDI, OGG"),
            "the message lists the formats: {message}"
        );
        let mut broken = Vec::from(&b"MUS\x1A"[..]);
        broken.extend_from_slice(&968_u16.to_le_bytes());
        broken.extend_from_slice(&16_u16.to_le_bytes());
        broken.resize(20, 0_u8);
        let corrupt = dir.path().join("doomed.mid");
        fs::write(&corrupt, broken).expect("write the corrupt file");
        let (mus_code, mus_message) = call_err(&client, "import_music", source_arg(&corrupt)).await;
        assert_eq!(mus_code, "format_error", "a score past the lump end");
        assert!(
            mus_message.contains("MUS score"),
            "the validator's message survives: {mus_message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn duplicates_are_refused_until_replace_is_passed() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let midi = write_midi(dir.path(), "runnin", &ONE_NOTE);
        call(&client, "import_music", source_arg(&midi)).await;
        let (code, message) = call_err(&client, "import_music", source_arg(&midi)).await;
        assert_eq!(code, "already_exists", "the derived twin is refused");
        assert!(
            message.contains("replace true"),
            "the message points at the fix: {message}"
        );
        let mut args = source_arg(&midi);
        args["replace"] = json!(true);
        let again = call(&client, "import_music", args).await;
        assert_eq!(again["name"], json!("D_RUNNIN"), "the name stays");
        assert_eq!(
            working_names(&client).await,
            vec!["D_RUNNIN"],
            "still exactly one copy in the directory"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["import_music D_RUNNIN", "import_music D_RUNNIN"],
            "the replace is its own undoable edit"
        );
    }

    #[tokio::test]
    async fn bad_arguments_and_missing_sources_error_before_storing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let midi = write_midi(dir.path(), "runnin", &ONE_NOTE);
        let mut format = source_arg(&midi);
        format["format"] = json!("zdoom");
        let (format_code, _) = call_err(&client, "import_music", format).await;
        assert_eq!(format_code, "invalid_argument", "not a storage format");
        let mut long_name = source_arg(&midi);
        long_name["name"] = json!("NINECHARS");
        let (name_code, _) = call_err(&client, "import_music", long_name).await;
        assert_eq!(name_code, "invalid_argument", "over eight characters");
        let hidden = dir.path().join(".mid");
        fs::copy(&midi, &hidden).expect("copy the fixture under a bare extension");
        let (stem_code, stem_message) =
            call_err(&client, "import_music", source_arg(&hidden)).await;
        assert_eq!(
            stem_code, "invalid_argument",
            "nothing usable to derive from"
        );
        assert!(
            stem_message.contains("pass name"),
            "the message asks for the name: {stem_message}"
        );
        let missing = dir.path().join("nowhere.mid");
        let (missing_code, _) = call_err(&client, "import_music", source_arg(&missing)).await;
        assert_eq!(missing_code, "not_found", "fetch errors surface unchanged");
        assert_untouched(&client).await;
    }
}
