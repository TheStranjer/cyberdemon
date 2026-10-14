//! The `audio` tool group as seen by a real MCP client: `import_sound`
//! decoding `WAV` files (the in-house reader and the `symphonia` fallback),
//! building padded `DMX` `DS*` lumps, storing `passthrough` copies, and
//! every refusal path leaving the session untouched.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use cyberdemon::audio::{Pcm, encode_wav};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// Sample rate of the stereo fixtures.
    const SOURCE_RATE: u32 = 22050;
    /// Frames in the one-second fixtures.
    const ONE_SECOND: usize = 22050;
    /// Header bytes in a DMX lump.
    const DMX_HEADER: usize = 8;
    /// Pad bytes a padded DMX lump carries at each end.
    const DMX_PAD: usize = 16;

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

    /// A square wave flipping every 551 frames, at `SOURCE_RATE`.
    fn square(frames: usize) -> Vec<i16> {
        (0..frames).map(square_frame).collect()
    }

    /// The square-wave sample of one frame.
    fn square_frame(frame: usize) -> i16 {
        let step = frame
            .checked_div(551)
            .unwrap_or(0)
            .checked_rem(2)
            .unwrap_or(0);
        if step == 0 { 8000_i16 } else { -8000_i16 }
    }

    /// Writes a 16-bit stereo [`SOURCE_RATE`] Hz WAV of `frames` frames,
    /// named `{stem}.wav`, and returns its path.
    fn write_stereo_wav(dir: &Path, stem: &str, frames: usize) -> PathBuf {
        let tone = square(frames);
        let mut samples = Vec::with_capacity(frames.checked_mul(2).expect("test size fits"));
        for value in &tone {
            samples.extend_from_slice(&[*value, value.wrapping_neg()]);
        }
        let pcm = Pcm {
            sample_rate: SOURCE_RATE,
            channels: 2,
            samples,
        };
        let path = dir.join(format!("{stem}.wav"));
        let bytes = encode_wav(&pcm, 16).expect("the fixture encodes");
        fs::write(&path, bytes).expect("write the fixture");
        path
    }

    /// Writes a float-`WAV` the in-house reader refuses: format tag `3`,
    /// mono, 8000 Hz, 32-bit `IEEE` floats.
    fn write_float_wav(dir: &Path, stem: &str, values: &[f32]) -> PathBuf {
        let rate = 8000_u32;
        let mut fmt: Vec<u8> = Vec::new();
        fmt.extend_from_slice(&3_u16.to_le_bytes());
        fmt.extend_from_slice(&1_u16.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        let byte_rate = rate.checked_mul(4).expect("test rates fit");
        fmt.extend_from_slice(&byte_rate.to_le_bytes());
        fmt.extend_from_slice(&4_u16.to_le_bytes());
        fmt.extend_from_slice(&32_u16.to_le_bytes());
        let data: Vec<u8> = values.iter().copied().flat_map(f32::to_le_bytes).collect();
        let chunks = data.len().checked_add(fmt.len()).expect("test sizes fit");
        let riff_size =
            u32::try_from(chunks.checked_add(20).expect("test sizes fit")).expect("test sizes fit");
        let data_len = u32::try_from(data.len()).expect("test sizes fit");
        let fmt_len = u32::try_from(fmt.len()).expect("test sizes fit");
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&fmt_len.to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&data);
        let path = dir.join(format!("{stem}.wav"));
        fs::write(&path, out).expect("write the fixture");
        path
    }

    /// A mono unsigned-`8`-bit 8000 Hz WAV of `seconds` seconds: small on
    /// disk, long on the clock.
    fn write_long_wav(dir: &Path, stem: &str, seconds: u32) -> PathBuf {
        let frames = usize::try_from(seconds.checked_mul(8000).expect("test lengths fit"))
            .expect("test lengths fit");
        let pcm = Pcm {
            sample_rate: 8000,
            channels: 1,
            samples: vec![200_i16; frames],
        };
        let path = dir.join(format!("{stem}.wav"));
        let bytes = encode_wav(&pcm, 8).expect("the fixture encodes");
        fs::write(&path, bytes).expect("write the fixture");
        path
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

    #[tokio::test]
    async fn doom_import_stores_a_padded_dmx_lump_at_the_default_rate() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let wav = write_stereo_wav(dir.path(), "pistol", ONE_SECOND);
        let report = call(&client, "import_sound", source_arg(&wav)).await;
        assert_eq!(report["name"], json!("DSPISTOL"), "DS + the stem");
        assert_eq!(report["format"], json!("doom"), "the default format");
        assert_eq!(
            report["source_format"],
            json!("wav"),
            "the container sniffed"
        );
        assert_eq!(report["sample_rate"], json!(11025_u64), "the vanilla rate");
        assert_eq!(report["channels_in"], json!(2_u64), "stereo in, mono out");
        let ms = report["duration_ms"].as_u64().expect("a millisecond count");
        assert!((999..=1001).contains(&ms), "one second survives: {ms} ms");
        let frames = ONE_SECOND.checked_div(2).expect("halves exactly");
        let expected = frames
            .checked_add(DMX_PAD.saturating_mul(2))
            .and_then(|body| body.checked_add(DMX_HEADER))
            .expect("test sizes fit");
        assert_eq!(report["bytes"], json!(expected), "header, samples and pads");
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "DSPISTOL", "scope": "working", "format": "hex", "length": 8_u64 }),
        )
        .await;
        assert_eq!(
            read["hex"],
            json!("03 00 11 2B 31 2B 00 00"),
            "magic 03 00, rate 0x2B11 = 11025, count 11057 with the pads"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["import_sound DSPISTOL"],
            "one undoable edit"
        );
    }

    #[tokio::test]
    async fn keep_rate_stores_the_source_rate_instead_of_resampling() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let wav = write_stereo_wav(dir.path(), "stereo", ONE_SECOND);
        let mut args = source_arg(&wav);
        args["keep_rate"] = json!(true);
        args["name"] = json!("DSKEEPRT");
        let report = call(&client, "import_sound", args).await;
        assert_eq!(report["sample_rate"], json!(22050_u64), "22050 kept");
        let ms = report["duration_ms"].as_u64().expect("a millisecond count");
        assert!((999..=1001).contains(&ms), "still one second: {ms} ms");
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "DSKEEPRT", "scope": "working", "format": "hex", "length": 8_u64 }),
        )
        .await;
        assert_eq!(
            read["hex"],
            json!("03 00 22 56 42 56 00 00"),
            "rate 0x5622 = 22050, count 22082 with the pads"
        );
    }

    #[tokio::test]
    async fn passthrough_stores_the_original_bytes_unchanged() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let wav = write_stereo_wav(dir.path(), "keeper", ONE_SECOND);
        let mut args = source_arg(&wav);
        args["format"] = json!("passthrough");
        args["name"] = json!("KEEPER");
        let report = call(&client, "import_sound", args).await;
        assert_eq!(
            report["format"],
            json!("passthrough"),
            "the echo of the choice"
        );
        assert_eq!(report["source_format"], json!("wav"), "still a WAV");
        assert_eq!(report["sample_rate"], json!(22050_u64), "the decoded rate");
        assert_eq!(report["channels_in"], json!(2_u64), "the decoded channels");
        let stored = lump_bytes(&client, "KEEPER", dir.path(), "keeper").await;
        let original = fs::read(&wav).expect("read the fixture back");
        assert_eq!(stored, original, "byte-for-byte identical");
        assert_eq!(report["bytes"], json!(original.len()), "the file size");
    }

    #[tokio::test]
    async fn a_float_wav_imports_through_the_symphonia_fallback() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let halves = [0.5_f32, -0.5, 0.5, -0.5, 0.25, -0.25, 1.0, -1.0];
        let wav = write_float_wav(dir.path(), "fluty", &halves);
        let report = call(&client, "import_sound", source_arg(&wav)).await;
        assert_eq!(report["name"], json!("DSFLUTY"), "DS + the stem");
        assert_eq!(report["source_format"], json!("wav"), "a WAV all the same");
        assert_eq!(report["channels_in"], json!(1_u64), "mono float in");
        assert_eq!(
            report["sample_rate"],
            json!(11025_u64),
            "resampled like any doom sound"
        );
        // mul_div_round: 8 * 11025 / 8000 = 11.025 -> 11 frames.
        let frames = 11_usize;
        let expected = frames
            .checked_add(DMX_PAD.saturating_mul(2))
            .and_then(|body| body.checked_add(DMX_HEADER))
            .expect("test sizes fit");
        assert_eq!(
            report["bytes"],
            json!(expected),
            "11 samples plus header and pads"
        );
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "DSFLUTY", "scope": "working", "format": "hex", "length": 8_u64 }),
        )
        .await;
        assert_eq!(
            read["hex"],
            json!("03 00 11 2B 2B 00 00 00"),
            "magic, 11025 Hz, count 43 with the pads"
        );
    }

    #[tokio::test]
    async fn derived_names_truncate_and_explicit_names_win() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let long = write_stereo_wav(dir.path(), "very long sound name", 100);
        let report = call(&client, "import_sound", source_arg(&long)).await;
        assert_eq!(
            report["name"],
            json!("DSVERYLO"),
            "DS plus the stem's first six usable characters"
        );
        let mut named = source_arg(&long);
        named["name"] = json!("FIREBOLT");
        let explicit = call(&client, "import_sound", named).await;
        assert_eq!(explicit["name"], json!("FIREBOLT"), "an explicit name wins");
        let hidden = dir.path().join(".wav");
        let pcm = Pcm {
            sample_rate: 8000,
            channels: 1,
            samples: vec![7_i16; 8],
        };
        fs::write(&hidden, encode_wav(&pcm, 16).expect("encodes")).expect("write .wav");
        let (code, message) = call_err(&client, "import_sound", source_arg(&hidden)).await;
        assert_eq!(code, "invalid_argument", "nothing usable to derive from");
        assert!(
            message.contains("pass name"),
            "the message asks for the name: {message}"
        );
    }

    #[tokio::test]
    async fn duplicates_are_refused_until_replace_is_passed() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let wav = write_stereo_wav(dir.path(), "pistol", ONE_SECOND);
        call(&client, "import_sound", source_arg(&wav)).await;
        let (code, message) = call_err(&client, "import_sound", source_arg(&wav)).await;
        assert_eq!(code, "already_exists", "the derived twin is refused");
        assert!(
            message.contains("replace true"),
            "the message points at the fix: {message}"
        );
        let shorter = write_stereo_wav(dir.path(), "pistol2", 441);
        let mut replace = source_arg(&shorter);
        replace["name"] = json!("DSPISTOL");
        replace["replace"] = json!(true);
        let again = call(&client, "import_sound", replace).await;
        assert_eq!(again["name"], json!("DSPISTOL"), "the name stays");
        // 441 stereo frames at 22050 Hz -> mono -> 221 frames at 11025.
        assert_eq!(again["duration_ms"], json!(20_u64), "a tenth of a second");
        let body = 221_usize
            .checked_add(DMX_PAD.saturating_mul(2))
            .expect("fits");
        assert_eq!(
            again["bytes"],
            json!(DMX_HEADER.checked_add(body).expect("fits")),
            "the payload was replaced by the shorter sound"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["DSPISTOL"],
            "still exactly one copy in the directory"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["import_sound DSPISTOL", "import_sound DSPISTOL"],
            "the replace is its own undoable edit"
        );
    }

    #[tokio::test]
    async fn max_seconds_bounds_the_accepted_length() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let long = write_long_wav(dir.path(), "siren", 200);
        let (code, message) = call_err(&client, "import_sound", source_arg(&long)).await;
        assert_eq!(code, "limit_exceeded", "200 s over the 60 s default");
        assert!(
            message.contains("max_seconds"),
            "the message names the cap: {message}"
        );
        assert_untouched(&client).await;
        let mut args = source_arg(&long);
        args["max_seconds"] = json!(200_u64);
        args["keep_rate"] = json!(true);
        let report = call(&client, "import_sound", args).await;
        assert_eq!(report["duration_ms"], json!(200_000_u64), "200 s kept");
        let mut zero = source_arg(&long);
        zero["max_seconds"] = json!(0_u64);
        let (zero_code, _) = call_err(&client, "import_sound", zero).await;
        assert_eq!(
            zero_code, "invalid_argument",
            "a zero cap is refused at parse time"
        );
    }

    #[tokio::test]
    async fn garbage_and_bad_arguments_error_without_touching_the_session() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let junk = dir.path().join("junk.wav");
        fs::write(&junk, b"total nonsense, no container here").expect("write the junk");
        let (code, message) = call_err(&client, "import_sound", source_arg(&junk)).await;
        assert_eq!(code, "format_error", "unrecognized bytes");
        assert!(
            message.contains("cannot identify"),
            "the message names the supported containers: {message}"
        );
        let hinted = dir.path().join("junk.xyzzy");
        fs::write(&hinted, b"total nonsense, no container here").expect("write the junk");
        let (hint_code, _) = call_err(&client, "import_sound", source_arg(&hinted)).await;
        assert_eq!(
            hint_code, "format_error",
            "an unknown hint with junk still fails"
        );
        let good = write_stereo_wav(dir.path(), "clip", ONE_SECOND);
        let raw = fs::read(&good).expect("read the fixture");
        let cut = dir.path().join("cut.wav");
        let head = raw.get(..14).expect("the fixture is longer");
        fs::write(&cut, head).expect("write the truncated fixture");
        let (cut_code, _) = call_err(&client, "import_sound", source_arg(&cut)).await;
        assert_eq!(cut_code, "format_error", "a header that never completes");
        let mut rate = source_arg(&good);
        rate["sample_rate"] = json!(3999_u64);
        let (rate_code, _) = call_err(&client, "import_sound", rate).await;
        assert_eq!(rate_code, "invalid_argument", "below the rate window");
        let mut format = source_arg(&good);
        format["format"] = json!("zdoom");
        let (format_code, _) = call_err(&client, "import_sound", format).await;
        assert_eq!(format_code, "invalid_argument", "not a storage format");
        let missing = dir.path().join("nowhere.wav");
        let (missing_code, _) = call_err(&client, "import_sound", source_arg(&missing)).await;
        assert_eq!(missing_code, "not_found", "fetch errors surface unchanged");
        assert_untouched(&client).await;
    }
}
