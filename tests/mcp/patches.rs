//! The `patches` tool group as seen by a real MCP client: `add_patch`
//! converting a `PNG` through the shared import pipeline into the created
//! `P_START` region with its `PNAMES` entry, storing raw `Doom` patch bytes
//! byte-identically (or with only header bytes `4..8` rewritten by an
//! explicit offsets pair), the `add_to_pnames` opt-out, the duplicate
//! refusal that names `replace_patch`, the parameter refusal catalogue,
//! and the undo/redo pair. `replace_patch` then swaps a patch the session
//! already has: an `IWAD`-owned patch gains a working override the merged
//! view serves while `PNAMES` stays put, and a working-owned patch is
//! overwritten in place at its own index. `remove_patch` then deletes a
//! working patch - the lump alone, or the lump and its `PNAMES` entry with
//! every texture reference renumbered through the shared removal plan, in
//! one undoable step the conflict refusal never reaches. Test pictures are
//! written with the library's own `PNG` encoder; the passthrough bytes are
//! one `encode_patch` produces.

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
    /// The file name [`iwad`] writes the fixture `IWAD` under.
    const IWAD_FILE: &str = "iwad-doom2.wad";
    /// Pixels on a side of the fixture `IWAD`'s `WALLP1`.
    const IWAD_SIDE: u64 = 16_u64;

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

    /// One successful `replace_patch` of `name` with `file`.
    async fn replace(client: &Client, file: &Path, name: &str) -> Value {
        call(
            client,
            "replace_patch",
            patch_args(file, json!({ "name": name })),
        )
        .await
    }

    /// One successful `remove_patch` of `name` with extra arguments merged
    /// in (`remove_from_pnames`, `force`).
    async fn remove(client: &Client, name: &str, extra: Value) -> Value {
        let mut args = json!({ "name": name });
        if let Value::Object(additions) = extra {
            args.as_object_mut()
                .expect("a JSON object")
                .extend(additions);
        }
        call(client, "remove_patch", args).await
    }

    /// The `list_pnames` names, in entry order.
    fn pnames_list(report: &Value) -> Vec<String> {
        report["pnames"]
            .as_array()
            .expect("a listing")
            .iter()
            .map(|row| row["name"].as_str().expect("a named row").to_owned())
            .collect()
    }

    /// The `list_textures` patch count of the texture `name`.
    async fn texture_patches(client: &Client, name: &str) -> u64 {
        let listed = call(client, "list_textures", json!({ "pattern": name })).await;
        assert_eq!(listed["total"], json!(1_u64), "{name} is defined");
        listed["textures"][0]["patches"]
            .as_u64()
            .unwrap_or_else(|| panic!("{name} reports its patch count"))
    }

    /// The fixture `IWAD` file's own bytes on disk, for the untouched-base
    /// assertions.
    fn iwad_bytes(dir: &Path) -> Vec<u8> {
        std::fs::read(dir.join(IWAD_FILE)).expect("the fixture IWAD reads back")
    }

    /// The working archive's directory index of the lump `name`.
    async fn working_index(client: &Client, name: &str) -> u64 {
        let listing = call(client, "list_lumps", json!({ "scope": "working" })).await;
        let wanted = json!(name);
        listing["lumps"]
            .as_array()
            .expect("a listing")
            .iter()
            .find(|row| row["name"] == wanted)
            .and_then(|row| row["index"].as_u64())
            .unwrap_or_else(|| panic!("{name} is not in the working directory"))
    }

    /// One `list_patches` row matching `name` exactly.
    async fn patch_row(client: &Client, name: &str) -> Value {
        let listed = call(client, "list_patches", json!({ "pattern": name })).await;
        assert_eq!(listed["total"], json!(1_u64), "{name} is listed once");
        listed["patches"][0].clone()
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
        let (code, message, _) = call_err(
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
            let (code, message, _) = call_err(client, "add_patch", args.clone()).await;
            assert_eq!(code, *expect_code, "{label} code: {message}");
            assert!(
                message.contains(expect_text),
                "{label} message should name {expect_text:?}: {message}"
            );
        }
    }

    /// Assert every `replace_patch` call in `cases` refuses as documented.
    async fn refuse_replacements(client: &Client, cases: &[Refusal]) {
        for case in cases {
            let (label, args, expect_code, expect_text) = (case.0, &case.1, case.2, case.3);
            let (code, message, _) = call_err(client, "replace_patch", args.clone()).await;
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

    /// Replacing the `IWAD`'s own `WALLP1` with a converted `PNG` creates
    /// a working override in a newly-created `patches` region; the `IWAD`
    /// file on disk is never touched and `PNAMES` is never rewritten.
    #[tokio::test]
    async fn replace_patch_creates_a_working_override_of_an_iwad_patch() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let before = iwad_bytes(dir.path());
        let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
        let report = replace(&client, &file, "WALLP1").await;
        assert_eq!(report["name"], json!("WALLP1"), "the name echoes");
        assert_eq!(report["converted"], json!(true), "the picture half ran");
        assert_eq!(report["width"], json!(4_u64), "the new 4x4 width");
        assert_eq!(report["height"], json!(4_u64), "the new 4x4 height");
        assert_eq!(report["pnames_index"], json!(0_u64), "its old PNAMES index");
        assert_eq!(
            report["created_override"],
            json!(true),
            "only a lower layer held it"
        );
        let listed = patch_row(&client, "WALLP1").await;
        assert_eq!(listed["namespace"], json!("patches"), "in the region");
        assert_eq!(listed["source"], json!("working"), "from the working copy");
        assert_eq!(
            working_names(&client).await,
            ["P_START", "WALLP1", "P_END"],
            "the working archive gained the region and the override"
        );
        assert_eq!(iwad_bytes(dir.path()), before, "the IWAD file is unchanged");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["replace_patch WALLP1"]),
            "one undoable edit labelled as documented"
        );
    }

    /// The override is what the merged view now serves: `get_patch` reads
    /// the new pixels and `list_patches` shows `source: "working"`, while
    /// `list_pnames` keeps `WALLP1` at its old index.
    #[tokio::test]
    async fn the_override_wins_the_merged_view_and_pnames_stays_put() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
        replace(&client, &file, "WALLP1").await;
        let picture = call(&client, "get_patch", json!({ "name": "WALLP1" })).await;
        assert_eq!(
            picture["width"],
            json!(4_u64),
            "get_patch serves the override"
        );
        assert_eq!(picture["height"], json!(4_u64), "both new dimensions");
        let listed = patch_row(&client, "WALLP1").await;
        assert_eq!(listed["source"], json!("working"), "the working copy wins");
        let pnames = call(&client, "list_pnames", json!({ "pattern": "WALLP1" })).await;
        assert_eq!(pnames["total"], json!(1_u64), "the one entry");
        assert_eq!(
            pnames["pnames"][0]["index"],
            json!(0_u64),
            "at its old index"
        );
        assert_eq!(
            pnames["pnames"][0]["lump_found"],
            json!(true),
            "and a lump still answers to it"
        );
        let all = call(&client, "list_pnames", json!({})).await;
        assert_eq!(all["total"], json!(FIXTURE_PNAMES), "PNAMES still has two");
    }

    /// Replacing a patch the working archive already owns overwrites it in
    /// place: `created_override` is `false`, its directory index is
    /// unchanged and the passthrough payload now stands there.
    #[tokio::test]
    async fn replace_patch_overwrites_a_working_owned_patch_in_place() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let png = write_file(dir.path(), "MYART.png", &palette_png());
        let added = add(&client, &png, "MYART").await;
        let added_index = working_index(&client, "MYART").await;
        let bytes = doom_patch();
        let patch_file = write_file(dir.path(), "MYART.patch", &bytes);
        let report = replace(&client, &patch_file, "MYART").await;
        assert_eq!(
            report["converted"],
            json!(false),
            "the passthrough half ran"
        );
        assert_eq!(
            report["created_override"],
            json!(false),
            "the working archive already owned it"
        );
        assert_eq!(
            report["pnames_index"], added["pnames_index"],
            "the same index"
        );
        assert_eq!(
            report["lump_index"],
            json!(added_index),
            "the same directory slot"
        );
        assert_eq!(
            working_index(&client, "MYART").await,
            added_index,
            "still in the same place"
        );
        assert_eq!(
            working_names(&client).await,
            ["P_START", "MYART", "P_END", "PNAMES"],
            "the directory order is unchanged"
        );
        assert_eq!(
            stored_bytes(&client, dir.path(), "MYART").await,
            bytes,
            "the payload now stands replaced"
        );
    }

    /// A name no layer holds comes back `not_found` naming `add_patch`,
    /// with nothing written.
    #[tokio::test]
    async fn replacing_an_unknown_name_refuses_naming_add_patch() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "MYART.png", &palette_png());
        let (code, message, _) = call_err(
            &client,
            "replace_patch",
            patch_args(&file, json!({ "name": "NOPE" })),
        )
        .await;
        assert_eq!(code, "not_found", "the missing target");
        assert!(
            message.contains("add_patch"),
            "the message names the way in: {message}"
        );
        let listed = call(&client, "list_patches", json!({ "pattern": "NOPE" })).await;
        assert_eq!(listed["total"], json!(0_u64), "nothing was written");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "the refusal added no step");
    }

    /// Undo steps the created override back: `list_patches` reports the
    /// `IWAD` source again and the working copy is gone; redo re-creates
    /// the override.
    #[tokio::test]
    async fn replace_patch_override_undoes_to_the_layer_and_redoes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
        replace(&client, &file, "WALLP1").await;
        let undone = call(&client, "undo", json!({ "steps": 1_u64 })).await;
        assert_eq!(
            undone["undone"],
            json!(["replace_patch WALLP1"]),
            "the labelled step went back"
        );
        let back = patch_row(&client, "WALLP1").await;
        assert_eq!(
            back["source"],
            json!({ "layer": 0_u64 }),
            "the IWAD copy serves again"
        );
        assert_eq!(back["width"], json!(IWAD_SIDE), "the IWAD's own pixels");
        let working = working_names(&client).await;
        assert!(
            !working.contains(&"WALLP1".to_owned()),
            "the working override is gone: {working:?}"
        );
        let redone = call(&client, "redo", json!({ "steps": 1_u64 })).await;
        assert_eq!(
            redone["redone"],
            json!(["replace_patch WALLP1"]),
            "the step replayed"
        );
        let forward = patch_row(&client, "WALLP1").await;
        assert_eq!(forward["source"], json!("working"), "the override is back");
        assert_eq!(forward["width"], json!(4_u64), "with the new pixels");
    }

    /// `replace_patch` refuses the shared parameter catalogue the same way
    /// `add_patch` does, and nothing lands: the `IWAD`'s `WALLP1` still
    /// serves and the undo stack stays empty.
    #[tokio::test]
    async fn bad_replace_arguments_are_refused_without_edits() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let patch_file = write_file(dir.path(), "WALLP1.patch", &doom_patch());
        let cases: [Refusal; 3] = [
            (
                "nine characters",
                patch_args(&patch_file, json!({ "name": "TOOLONGNM" })),
                "invalid_argument",
                "name",
            ),
            (
                "preset",
                patch_args(
                    &patch_file,
                    json!({ "name": "WALLP1", "offsets": "middle" }),
                ),
                "invalid_argument",
                "center_bottom",
            ),
            (
                "offset past i16",
                patch_args(
                    &patch_file,
                    json!({ "name": "WALLP1", "offsets": { "x": 40000_i64, "y": 0_i64 } }),
                ),
                "invalid_argument",
                "does not fit",
            ),
        ];
        refuse_replacements(&client, &cases).await;
        let listed = patch_row(&client, "WALLP1").await;
        assert_eq!(
            listed["source"],
            json!({ "layer": 0_u64 }),
            "the IWAD copy still stands"
        );
        assert_eq!(listed["width"], json!(IWAD_SIDE), "with its original bytes");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "nothing was written");
    }

    /// `add_patch` then `remove_patch`: the lump leaves the working
    /// archive alone - `removed_from_pnames: false`, the name gone from
    /// `list_patches`, the markers (and so the closed region) and the
    /// fixture's unchanged `PNAMES` behind.
    #[tokio::test]
    async fn remove_patch_deletes_the_working_lump_and_answers_on_the_list() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "OFFLIST.png", &palette_png());
        call(
            &client,
            "add_patch",
            patch_args(&file, json!({ "name": "OFFLIST", "add_to_pnames": false })),
        )
        .await;
        let report = remove(&client, "OFFLIST", json!({})).await;
        assert_eq!(report["name"], json!("OFFLIST"), "the name echoes");
        assert_eq!(report["removed_lump"], json!(true), "the working lump went");
        assert_eq!(
            report["removed_from_pnames"],
            json!(false),
            "the flag was not asked for"
        );
        assert_eq!(
            report["textures_changed"],
            json!(0_u64),
            "no list write, no renumbering"
        );
        let listed = call(&client, "list_patches", json!({ "pattern": "OFFLIST" })).await;
        assert_eq!(listed["total"], json!(0_u64), "gone from list_patches");
        assert_eq!(
            working_names(&client).await,
            ["P_START", "P_END"],
            "the markers stayed, so the region stayed closed"
        );
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(pnames["total"], json!(FIXTURE_PNAMES), "PNAMES never moved");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["add_patch OFFLIST", "remove_patch OFFLIST"]),
            "each step is its own labelled edit, oldest first"
        );
    }

    /// The `PNAMES` half of the pair: `add_patch` lists the name by
    /// default, and a `remove_patch` with `remove_from_pnames: true` drops
    /// the entry again with no texture shifted.
    #[tokio::test]
    async fn remove_patch_can_drop_the_pnames_entry_too() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let on = write_file(dir.path(), "ONLIST.png", &palette_png());
        call(
            &client,
            "add_patch",
            patch_args(&on, json!({ "name": "ONLIST" })),
        )
        .await;
        let grown = call(&client, "list_pnames", json!({})).await;
        assert_eq!(grown["total"], json!(3_u64), "add_patch listed it");
        let cut = remove(&client, "ONLIST", json!({ "remove_from_pnames": true })).await;
        assert_eq!(cut["removed_lump"], json!(true), "both halves went");
        assert_eq!(
            cut["removed_from_pnames"],
            json!(true),
            "the entry dropped out"
        );
        assert_eq!(
            cut["textures_changed"],
            json!(0_u64),
            "no texture referred to it, nothing shifted"
        );
        let back = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames_list(&back),
            vec!["WALLP1", "STEPP1"],
            "count 2 again"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["add_patch ONLIST", "remove_patch ONLIST"]),
            "the add and its removal stand side by side, oldest first"
        );
    }

    /// `remove_patch` reads the working archive only: the `IWAD`'s own
    /// `WALLP1` answers `not_found` pointing at `replace_patch`, and a bad
    /// name is `invalid_argument` - nothing is written either way.
    #[tokio::test]
    async fn removing_an_iwad_owned_patch_refuses_naming_replace_patch() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let (code, message, _) =
            call_err(&client, "remove_patch", json!({ "name": "WALLP1" })).await;
        assert_eq!(code, "not_found", "no working copy exists");
        assert!(
            message.contains("replace_patch") && message.contains("working"),
            "the hint names the way to a working copy: {message}"
        );
        let (bad_code, bad_message, _) =
            call_err(&client, "remove_patch", json!({ "name": "TOOLONGNM" })).await;
        assert_eq!(bad_code, "invalid_argument", "nine characters");
        assert!(
            bad_message.contains("name"),
            "the message names the parameter: {bad_message}"
        );
        let listed = patch_row(&client, "WALLP1").await;
        assert_eq!(
            listed["source"],
            json!({ "layer": 0_u64 }),
            "the IWAD copy still serves"
        );
        assert!(
            working_names(&client).await.is_empty(),
            "nothing was written"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "and nothing was recorded");
    }

    /// `replace_patch` gives `WALLP1` a working copy; removing it with
    /// `remove_from_pnames` while `WALL1` still refers to it is a
    /// `conflict` and NOTHING leaves - not the lump, not the list.
    #[tokio::test]
    async fn a_removal_texture_still_refuses_conflicts_without_force() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
        replace(&client, &file, "WALLP1").await;
        let (code, message, _) = call_err(
            &client,
            "remove_patch",
            json!({ "name": "WALLP1", "remove_from_pnames": true }),
        )
        .await;
        assert_eq!(code, "conflict", "WALL1 refers to the entry");
        assert!(
            message.contains("WALL1") && message.contains("TEXTURE1"),
            "the message names the texture and its lump: {message}"
        );
        assert!(
            message.contains("force=true"),
            "the message states the escape: {message}"
        );
        assert_eq!(
            working_names(&client).await,
            ["P_START", "WALLP1", "P_END"],
            "the override lump is still there"
        );
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames["total"],
            json!(FIXTURE_PNAMES),
            "PNAMES is still there"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["replace_patch WALLP1"]),
            "the refusal added no step"
        );
    }

    /// The same removal forced through: the working lump, the `PNAMES`
    /// entry and the renumbered `TEXTURE1` leave in one edit - `WALL1`
    /// loses its 32 references, `STEP1`'s survive renumbered from index 1
    /// to 0, and `STEPP1` still resolves.
    #[tokio::test]
    async fn forcing_the_removal_drops_the_lump_the_entry_and_renumbers() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
        replace(&client, &file, "WALLP1").await;
        let report = remove(
            &client,
            "WALLP1",
            json!({ "remove_from_pnames": true, "force": true }),
        )
        .await;
        assert_eq!(report["removed_lump"], json!(true), "the override went");
        assert_eq!(
            report["removed_from_pnames"],
            json!(true),
            "the entry went with it"
        );
        assert_eq!(
            report["textures_changed"],
            json!(2_u64),
            "WALL1 pruned, STEP1 renumbered"
        );
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames_list(&pnames),
            vec!["STEPP1"],
            "WALLP1 is off the list"
        );
        assert_eq!(
            pnames["pnames"][0]["index"],
            json!(0_u64),
            "the survivor shifted down"
        );
        assert_eq!(
            texture_patches(&client, "WALL1").await,
            0_u64,
            "its 32 references fell"
        );
        assert_eq!(
            texture_patches(&client, "STEP1").await,
            16_u64,
            "STEP1 kept its references"
        );
        let step = call(&client, "get_patch", json!({ "name": "STEPP1" })).await;
        assert_eq!(
            step["width"],
            json!(IWAD_SIDE),
            "the renumbered patch resolves"
        );
        assert_eq!(
            working_names(&client).await,
            ["P_START", "P_END", "PNAMES", "TEXTURE1"],
            "lump gone, list and texture rewritten as overrides"
        );
    }

    /// Undo steps the forced removal back in ONE entry: lump, old list and
    /// the old texture bytes return together; redo repeats the whole
    /// change.
    #[tokio::test]
    async fn undo_and_redo_walk_the_forced_removal_in_one_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "NEWWALL.png", &palette_png());
        replace(&client, &file, "WALLP1").await;
        remove(
            &client,
            "WALLP1",
            json!({ "remove_from_pnames": true, "force": true }),
        )
        .await;
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["replace_patch WALLP1", "remove_patch WALLP1"]),
            "the removal is one labelled entry"
        );
        let undone = call(&client, "undo", json!({ "steps": 1_u64 })).await;
        assert_eq!(
            undone["undone"],
            json!(["remove_patch WALLP1"]),
            "one step back"
        );
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames_list(&pnames),
            vec!["WALLP1", "STEPP1"],
            "the old list is back"
        );
        assert_eq!(
            texture_patches(&client, "WALL1").await,
            32_u64,
            "the old texture bytes are back"
        );
        let back = patch_row(&client, "WALLP1").await;
        assert_eq!(
            back["source"],
            json!("working"),
            "the override stands again"
        );
        let redone = call(&client, "redo", json!({ "steps": 1_u64 })).await;
        assert_eq!(
            redone["redone"],
            json!(["remove_patch WALLP1"]),
            "the change replayed"
        );
        let after = call(&client, "list_pnames", json!({})).await;
        assert_eq!(pnames_list(&after), vec!["STEPP1"], "and undid again");
    }

    /// A name the working `PNAMES` lists but no working lump carries is
    /// still removable with `remove_from_pnames`: `removed_lump` reports
    /// `false`, the entry leaves, and no `not_found` is raised.
    #[tokio::test]
    async fn a_pnames_entry_without_a_lump_is_removable_without_a_lump_write() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        call(&client, "edit_pnames", json!({ "add": ["GHOSTP"] })).await;
        let report = remove(&client, "GHOSTP", json!({ "remove_from_pnames": true })).await;
        assert_eq!(
            report["removed_lump"],
            json!(false),
            "the name never had a working lump"
        );
        assert_eq!(report["removed_from_pnames"], json!(true), "its entry left");
        assert_eq!(
            report["textures_changed"],
            json!(0_u64),
            "appended last, nothing shifted"
        );
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames_list(&pnames),
            vec!["WALLP1", "STEPP1"],
            "the ghost is off the list"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["edit_pnames add", "remove_patch GHOSTP"]),
            "one labelled removal on top of the list edit"
        );
    }
}
