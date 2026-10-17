//! The `patches` tool group as seen by a real MCP client: `add_patch`
//! converting a `PNG` through the shared import pipeline into the created
//! `P_START` region with its `PNAMES` entry, storing raw `Doom` patch bytes
//! byte-identically (or with only header bytes `4..8` rewritten by an
//! explicit offsets pair), the `add_to_pnames` opt-out, the duplicate
//! refusal that names `replace_patch`, the parameter refusal catalogue,
//! and the undo/redo pair. Test pictures are written with the library's
//! own `PNG` encoder; the passthrough bytes are one `encode_patch`
//! produces.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use cyberdemon::gfx::codec::encode_png;
    use cyberdemon::gfx::image::{IndexedImage, RgbaImage};
    use cyberdemon::gfx::patch::{Patch, encode_patch};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad;
    use crate::support::{call, call_err, connect};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// Pixels on a side of the 4x4 test pictures.
    const SIDE: u32 = 4_u32;
    /// Pixels in the 4x4 test pictures.
    const PIXELS: usize = 16;
    /// Entries in the fixture `IWAD`'s `PNAMES`.
    const FIXTURE_PNAMES: u64 = 2_u64;
    /// Header offsets the passthrough fixture patch carries.
    const PATCH_OFFSETS: (i16, i16) = (-2_i16, 11_i16);

    /// Load the fixture `IWAD` (palettes, patches, `PNAMES`) as the base.
    async fn iwad_client(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        client
    }

    /// The palette-colour pixel of raster position `p`, fully opaque.
    fn color_pixel(p: usize) -> [u8; 4] {
        let slot = u8::try_from(p).expect("test indices fit u8");
        [slot, 255_u8.wrapping_sub(slot), slot, 255_u8]
    }

    /// An opaque 4x4 `PNG` of palette colours.
    fn palette_png() -> Vec<u8> {
        let pixels: Vec<u8> = (0..PIXELS).flat_map(color_pixel).collect();
        let img = RgbaImage::new(SIDE, SIDE, pixels).expect("test shape is valid");
        encode_png(&img, None).expect("test PNGs encode")
    }

    /// The 4x4 fully-opaque `Doom` patch bytes the passthrough tests store,
    /// header offsets [`PATCH_OFFSETS`], every pixel index `7`.
    fn doom_patch() -> Vec<u8> {
        let indices = vec![7_u8; PIXELS];
        let opaque = vec![true; PIXELS];
        let image = IndexedImage::new(SIDE, SIDE, indices, opaque).expect("test shape is valid");
        let patch = Patch {
            left_offset: PATCH_OFFSETS.0,
            top_offset: PATCH_OFFSETS.1,
            image,
        };
        encode_patch(&patch).expect("a 4x4 patch encodes")
    }

    /// Write `bytes` to `file` in `dir` and return the path.
    fn write_file(dir: &Path, file: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(file);
        std::fs::write(&path, bytes).expect("test files write");
        path
    }

    /// `add_patch` arguments over a `source` path merged with extras.
    fn patch_args(file: &Path, extra: Value) -> Value {
        let mut args = json!({ "source": file.display().to_string() });
        if let Value::Object(additions) = extra {
            args.as_object_mut()
                .expect("a JSON object")
                .extend(additions);
        }
        args
    }

    /// One successful `add_patch` under `name` of `file`.
    async fn add(client: &Client, file: &Path, name: &str) -> Value {
        call(
            client,
            "add_patch",
            patch_args(file, json!({ "name": name })),
        )
        .await
    }

    /// The stored bytes of lump `name`, extracted to disk.
    async fn stored_bytes(client: &Client, dir: &Path, name: &str) -> Vec<u8> {
        let out = dir.join(format!("{name}.stored"));
        call(
            client,
            "extract_lump",
            json!({ "name": name, "path": out.display().to_string() }),
        )
        .await;
        std::fs::read(&out).expect("the extracted lump reads back")
    }

    /// The working archive's directory, as lump names in order.
    async fn working_names(client: &Client) -> Vec<String> {
        let listing = call(client, "list_lumps", json!({ "scope": "working" })).await;
        listing["lumps"]
            .as_array()
            .expect("a listing")
            .iter()
            .map(|row| row["name"].as_str().expect("a named row").to_owned())
            .collect()
    }

    /// The `PNG` half: the picture is quantized and encoded, lands in the
    /// created `patches` region of the working archive, and `PNAMES` lists
    /// it at the reported index.
    #[tokio::test]
    async fn add_patch_converts_a_picture_into_the_patches_region() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "MYART.png", &palette_png());
        let report = add(&client, &file, "MYART").await;
        assert_eq!(report["name"], json!("MYART"), "the name echoes");
        assert_eq!(report["converted"], json!(true), "the pipeline half ran");
        assert_eq!(report["width"], json!(4_u64), "the stored width");
        assert_eq!(report["height"], json!(4_u64), "the stored height");
        assert_eq!(report["left_offset"], json!(0_i64), "the zero default");
        assert_eq!(report["top_offset"], json!(0_i64), "zero both ways");
        assert_eq!(
            report["pnames_index"],
            json!(FIXTURE_PNAMES),
            "appended after the fixture pair"
        );
        let listed = call(&client, "list_patches", json!({ "pattern": "MYART" })).await;
        assert_eq!(listed["total"], json!(1_u64), "list_patches shows it");
        assert_eq!(
            listed["patches"][0]["namespace"],
            json!("patches"),
            "in the region"
        );
        assert_eq!(
            listed["patches"][0]["source"],
            json!("working"),
            "from the working archive"
        );
        assert_eq!(
            working_names(&client).await,
            ["P_START", "MYART", "P_END", "PNAMES"],
            "the region markers were created and the grown PNAMES appended"
        );
        let pnames = call(&client, "list_pnames", json!({ "pattern": "MYART" })).await;
        assert_eq!(pnames["total"], json!(1_u64), "the listing has it");
        assert_eq!(
            pnames["pnames"][0]["index"], report["pnames_index"],
            "the listing agrees with the report"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["add_patch MYART"]),
            "one undoable edit labelled as documented"
        );
    }

    /// The passthrough half: `Doom` patch bytes stand byte-identical, and
    /// an explicit offsets pair rewrites only header bytes `4..8`.
    #[tokio::test]
    async fn add_patch_stores_existing_doom_patch_bytes_unchanged() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let bytes = doom_patch();
        let file = write_file(dir.path(), "PASSTHRU.patch", &bytes);
        let kept = add(&client, &file, "KEPT").await;
        assert_eq!(kept["converted"], json!(false), "the passthrough half ran");
        assert_eq!(kept["width"], json!(4_u64), "the header's width");
        assert_eq!(kept["height"], json!(4_u64), "the header's height");
        assert_eq!(
            kept["left_offset"],
            json!(i64::from(PATCH_OFFSETS.0)),
            "the stored left offset"
        );
        assert_eq!(
            kept["top_offset"],
            json!(i64::from(PATCH_OFFSETS.1)),
            "the stored top offset"
        );
        assert_eq!(
            stored_bytes(&client, dir.path(), "KEPT").await,
            bytes,
            "byte for byte"
        );
        let shifted = call(
            &client,
            "add_patch",
            patch_args(
                &file,
                json!({ "name": "SHIFTED", "offsets": { "x": 3_i64, "y": -5_i64 } }),
            ),
        )
        .await;
        assert_eq!(shifted["converted"], json!(false), "still a passthrough");
        assert_eq!(shifted["left_offset"], json!(3_i64), "the rewritten pair");
        assert_eq!(shifted["top_offset"], json!(-5_i64), "both fields");
        let moved = stored_bytes(&client, dir.path(), "SHIFTED").await;
        assert_eq!(moved.len(), bytes.len(), "the same length");
        assert_eq!(&moved[..4], &bytes[..4], "width and height stay");
        assert_ne!(&moved[4..8], &bytes[4..8], "the offsets changed");
        assert_eq!(&moved[8..], &bytes[8..], "the payload stays byte-identical");
    }

    /// `add_to_pnames: false` answers `pnames_index: null` and leaves the
    /// list as it stands.
    #[tokio::test]
    async fn add_patch_can_stay_out_of_pnames() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "OFFLIST.png", &palette_png());
        let report = call(
            &client,
            "add_patch",
            patch_args(&file, json!({ "name": "OFFLIST", "add_to_pnames": false })),
        )
        .await;
        assert_eq!(report["pnames_index"], Value::Null, "opted out");
        assert_eq!(
            report["converted"],
            json!(true),
            "the picture still converted"
        );
        let listed = call(&client, "list_patches", json!({ "pattern": "OFFLIST" })).await;
        assert_eq!(listed["total"], json!(1_u64), "the lump stands");
        assert_eq!(
            listed["patches"][0]["in_pnames"],
            json!(false),
            "PNAMES does not know it"
        );
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames["total"],
            json!(FIXTURE_PNAMES),
            "PNAMES is unchanged by the add"
        );
    }

    /// A working twin refuses, pointing at `replace_patch`, and stores
    /// nothing: the undo stack keeps the single first edit.
    #[tokio::test]
    async fn a_duplicate_add_refuses_and_names_replace_patch() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "TWIN.png", &palette_png());
        add(&client, &file, "TWIN").await;
        let (code, message) = call_err(
            &client,
            "add_patch",
            patch_args(&file, json!({ "name": "TWIN" })),
        )
        .await;
        assert_eq!(code, "already_exists", "the twin refuses");
        assert!(
            message.contains("replace_patch"),
            "the message names the way out: {message}"
        );
        let listed = call(&client, "list_patches", json!({ "pattern": "TWIN" })).await;
        assert_eq!(listed["total"], json!(1_u64), "one copy stands");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["add_patch TWIN"]),
            "the refusal added no step"
        );
    }

    /// One refusal case: arguments, the expected `(code, message-substring)`
    /// pair and a debug label.
    type Refusal = (&'static str, Value, &'static str, &'static str);

    /// Assert every `add_patch` call in `cases` refuses as documented.
    async fn refuse_all(client: &Client, cases: &[Refusal]) {
        for case in cases {
            let (label, args, expect_code, expect_text) = (case.0, &case.1, case.2, case.3);
            let (code, message) = call_err(client, "add_patch", args.clone()).await;
            assert_eq!(code, *expect_code, "{label} code: {message}");
            assert!(
                message.contains(expect_text),
                "{label} message should name {expect_text:?}: {message}"
            );
        }
    }

    /// Missing sources, bad parameters and undecodable non-patch bytes each
    /// report their code, and nothing lands.
    #[tokio::test]
    async fn bad_add_arguments_are_refused_without_edits() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let patch_file = write_file(dir.path(), "SRC.patch", &doom_patch());
        let junk = write_file(dir.path(), "junk.txt", b"this is a text file, not an image");
        let cases: [Refusal; 6] = [
            (
                "missing",
                json!({
                    "name": "X",
                    "source": dir.path().join("absent.png").display().to_string(),
                }),
                "not_found",
                "absent.png",
            ),
            (
                "nine characters",
                patch_args(&patch_file, json!({ "name": "TOOLONGNM" })),
                "invalid_argument",
                "name",
            ),
            (
                "preset",
                patch_args(&patch_file, json!({ "name": "X", "offsets": "middle" })),
                "invalid_argument",
                "center_bottom",
            ),
            (
                "mode",
                patch_args(
                    &patch_file,
                    json!({ "name": "X", "transparency": { "mode": "blur" } }),
                ),
                "invalid_argument",
                "\"none\"",
            ),
            (
                "offset past i16",
                patch_args(
                    &patch_file,
                    json!({ "name": "X", "offsets": { "x": 40000_i64, "y": 0_i64 } }),
                ),
                "invalid_argument",
                "does not fit",
            ),
            (
                "junk",
                patch_args(&junk, json!({ "name": "X" })),
                "format_error",
                "image",
            ),
        ];
        refuse_all(&client, &cases).await;
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "nothing was stored");
    }

    /// Undo removes the lump and the `PNAMES` write together; redo brings
    /// both back.
    #[tokio::test]
    async fn undo_and_redo_move_the_patch_and_its_pnames_entry() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "MYART.png", &palette_png());
        add(&client, &file, "MYART").await;
        let undone = call(&client, "undo", json!({ "steps": 1_u64 })).await;
        assert_eq!(
            undone["undone"],
            json!(["add_patch MYART"]),
            "the labelled step went back"
        );
        let listed = call(&client, "list_patches", json!({ "pattern": "MYART" })).await;
        assert_eq!(listed["total"], json!(0_u64), "the lump left");
        assert!(
            working_names(&client).await.is_empty(),
            "the created region left with it"
        );
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames["total"],
            json!(FIXTURE_PNAMES),
            "the PNAMES write undid too"
        );
        let redone = call(&client, "redo", json!({ "steps": 1_u64 })).await;
        assert_eq!(
            redone["redone"],
            json!(["add_patch MYART"]),
            "the step replayed"
        );
        let back = call(&client, "list_patches", json!({ "pattern": "MYART" })).await;
        assert_eq!(back["total"], json!(1_u64), "the lump is back");
        assert_eq!(
            back["patches"][0]["in_pnames"],
            json!(true),
            "and PNAMES lists it again"
        );
        let after = call(&client, "list_pnames", json!({})).await;
        assert_eq!(after["total"], json!(3_u64), "the grown list stands");
    }

    /// A lump the `IWAD` holds does not clash: the add becomes a working
    /// override the merged view resolves to.
    #[tokio::test]
    async fn add_patch_overrides_an_iwad_patch_of_the_same_name() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "WALLP1.png", &palette_png());
        let report = add(&client, &file, "WALLP1").await;
        assert_eq!(report["name"], json!("WALLP1"), "the override lands");
        assert_eq!(report["converted"], json!(true), "from the picture half");
        assert_eq!(
            report["pnames_index"],
            json!(0_u64),
            "the name PNAMES already lists keeps its index"
        );
        let listed = call(&client, "list_patches", json!({ "pattern": "WALLP1" })).await;
        assert_eq!(
            listed["total"],
            json!(1_u64),
            "last wins in the merged view"
        );
        assert_eq!(
            listed["patches"][0]["source"],
            json!("working"),
            "the working copy serves"
        );
    }
}
