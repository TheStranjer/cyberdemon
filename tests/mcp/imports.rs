//! The `imports` tool group as seen by a real MCP client: the shared image
//! pipeline through `import_image_as_patch` and its alias
//! `import_png_as_patch` (exact-palette quantization with an alpha hole,
//! colour-key and opaque transparency, `grAb`/preset/explicit offsets, the
//! `PNAMES` listing and its opt-out, the duplicate and `replace` rule,
//! undo, and the refusal catalogue), the flat importer `import_image_as_flat`
//! with its alias `import_png_as_flat` (exact 64x64 round trips, nearest
//! resizing to 64 and 128, the `IWAD` override, the duplicate and `replace`
//! rule, ignored-and-counted alpha, and its refusal catalogue), and
//! `import_image_as_raw` storing validated bytes byte-for-byte and the sprite
//! importer `import_image_as_sprite` with its alias
//! `import_png_as_sprite` (both naming forms, the mirror pair, the
//! feet-on-origin default offsets, the sprites region, the rotation
//! warnings, the duplicate and `replace` rule and its refusal catalogue).
//! Test pictures are written with the library's own `PNG` encoder.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use cyberdemon::gfx::codec::encode_png;
    use cyberdemon::gfx::grab::insert_grab;
    use cyberdemon::gfx::image::{RgbaImage, resize_nearest};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad;
    use crate::support::{call, call_err, connect, http_get, http_put};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// Pixels on a side of the 8x8 test rasters.
    const SIDE: u32 = 8_u32;
    /// Pixels in the 8x8 test rasters.
    const PIXELS: usize = 64;
    /// Pixels on a side of the 64x64 test flats.
    const FLAT_SIDE: u32 = 64_u32;
    /// Pixels in the 64x64 test flats.
    const FLAT_PIXELS: usize = 4_096;
    /// Palette entries the flat test rasters cycle through.
    const CYCLE: usize = 256_usize;
    /// The pixel index the alpha hole is placed at.
    const HOLE: usize = 9;
    /// Entries in the fixture `IWAD`'s `PNAMES`.
    const FIXTURE_PNAMES: u64 = 2_u64;
    /// Flats in the fixture `IWAD` (`FLOOR1` and `PNGFLAT`).
    const FIXTURE_FLATS: u64 = 2_u64;
    /// The cyan the colour key defaults to.
    const CYAN: [u8; 4] = [0_u8, 255, 255, 255];

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

    /// Fixture palette colour `i`: `(i, 255 - i, i)`.
    fn palette_color(index: u8) -> [u8; 3] {
        [index, 255_u8.wrapping_sub(index), index]
    }

    /// An opaque pixel of palette colour `p`.
    fn color_pixel(p: usize) -> [u8; 4] {
        let slot = u8::try_from(p).expect("test indices fit u8");
        let [red, green, blue] = palette_color(slot);
        [red, green, blue, 255_u8]
    }

    /// The palette-colour pixel of raster position `p`, fully transparent
    /// when `hole` names it.
    fn raster_pixel(p: usize, hole: Option<usize>) -> [u8; 4] {
        let mut pixel = color_pixel(p);
        if hole.is_some_and(|dropped| dropped == p) {
            pixel[3] = 0_u8;
        }
        pixel
    }

    /// A palette-colour PNG of `width` by `height`, pixel `p` coloured
    /// index `p`, optionally alpha-transparent at `alpha_of` and carrying
    /// `grAb` offsets.
    fn palette_png(
        width: u32,
        height: u32,
        alpha_of: Option<usize>,
        grab: Option<(i32, i32)>,
    ) -> Vec<u8> {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let pixels: Vec<u8> = (0..count).flat_map(|p| raster_pixel(p, alpha_of)).collect();
        let img = RgbaImage::new(width, height, pixels).expect("test shapes are valid");
        let png = encode_png(&img, None).expect("test PNGs encode");
        match grab {
            Some((x, y)) => insert_grab(&png, x, y).expect("test grAb inserts"),
            None => png,
        }
    }

    /// The 8x8 alpha-hole PNG: palette colours `0..=63`, pixel [`HOLE`]
    /// fully transparent.
    fn hole_png() -> Vec<u8> {
        palette_png(SIDE, SIDE, Some(HOLE), None)
    }

    /// The 8x8 fully-opaque cyan PNG.
    fn cyan_png() -> Vec<u8> {
        let pixels: Vec<u8> = vec![CYAN; PIXELS].concat();
        let img = RgbaImage::new(SIDE, SIDE, pixels).expect("test shape is valid");
        encode_png(&img, None).expect("test PNGs encode")
    }

    /// A 4x4 `JPEG`, encoded with the `image` crate the library ships.
    fn jpeg_of() -> Vec<u8> {
        use image::codecs::jpeg::JpegEncoder;
        use image::{ExtendedColorType, ImageEncoder as _};
        let side = 4_usize;
        let bytes = side
            .checked_mul(side)
            .expect("test pixels fit")
            .checked_mul(3)
            .expect("test rgb fits");
        let rgb = vec![3_u8; bytes];
        let mut out: Vec<u8> = Vec::new();
        JpegEncoder::new(std::io::Cursor::new(&mut out))
            .write_image(&rgb, 4_u32, 4_u32, ExtendedColorType::Rgb8)
            .expect("a 4x4 image encodes as JPEG");
        out
    }

    /// Write `bytes` to `file` in `dir` and return the path.
    fn write_file(dir: &Path, file: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(file);
        std::fs::write(&path, bytes).expect("test files write");
        path
    }

    /// Arguments carrying a `source` path merged with an object of extras.
    fn source_args(file: &Path, extra: Value) -> Value {
        let mut args = json!({ "source": file.display().to_string() });
        if let Value::Object(additions) = extra {
            args.as_object_mut()
                .expect("a JSON object")
                .extend(additions);
        }
        args
    }

    /// Fetch a published asset's bytes.
    async fn fetch_asset(url: &str) -> Vec<u8> {
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
        assert_eq!(status, 200, "the asset serves: {url}");
        body
    }

    /// The `RGBA` bytes of the stored patch `name`, via `get_patch`.
    async fn patch_rgba(client: &Client, name: &str) -> Vec<u8> {
        let report = call(
            client,
            "get_patch",
            json!({ "name": name, "format": "rgba" }),
        )
        .await;
        let url = report["asset"]["url"]
            .as_str()
            .expect("get_patch publishes an asset");
        fetch_asset(url).await
    }

    /// The expected `RGBA` of [`hole_png`] after a patch round-trip: every
    /// opaque palette colour, and [`HOLE`] as an alpha-`0` pixel at index
    /// `0` (a patch stores no index under its holes).
    fn expected_rgba() -> Vec<u8> {
        (0..PIXELS).flat_map(expected_pixel).collect()
    }

    /// The round-tripped pixel `p`: its colour when opaque, index `0` and
    /// alpha `0` at the hole.
    fn expected_pixel(p: usize) -> [u8; 4] {
        if p == HOLE {
            [0_u8, 255, 0, 0]
        } else {
            color_pixel(p)
        }
    }

    /// Assert two patch-import reports agree on every shared pipeline
    /// field.
    fn assert_same_pipeline(main: &Value, alias: &Value) {
        for field in [
            "width",
            "height",
            "left_offset",
            "top_offset",
            "transparent_pixels",
            "colors_used",
            "max_color_error",
        ] {
            assert_eq!(
                alias[field], main[field],
                "the alias reports {field} identically"
            );
        }
    }

    /// The alpha-hole round-trip: exact palette colours import as a patch
    /// whose decoded pixels are the source's, hole included.
    #[tokio::test]
    async fn an_exact_png_imports_as_a_patch_that_decodes_back_identically() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "MYART.png", &hole_png());
        let report = call(
            &client,
            "import_image_as_patch",
            source_args(&file, json!({ "name": "MYART" })),
        )
        .await;
        assert_eq!(report["name"], json!("MYART"), "the name echoes");
        assert_eq!(
            report["lump_index"],
            json!(1_u64),
            "after the created P_START"
        );
        assert_eq!(
            report["left_offset"],
            json!(0_i64),
            "the wall-patch default without grAb"
        );
        assert_eq!(report["top_offset"], json!(0_i64), "zero both ways");
        assert_eq!(
            report["transparent_pixels"],
            json!(1_u64),
            "the alpha hole dropped one pixel"
        );
        assert_eq!(report["colors_used"], json!(63_u64), "63 palette colours");
        assert_eq!(
            report["max_color_error"],
            json!(0_i64),
            "the palette is exact"
        );
        assert_eq!(
            report["pnames_index"],
            json!(FIXTURE_PNAMES),
            "the new PNAMES entry follows the fixture pair"
        );
        assert_eq!(patch_rgba(&client, "MYART").await, expected_rgba());
        let listed = call(&client, "list_patches", json!({ "pattern": "MYART" })).await;
        assert_eq!(listed["total"], json!(1_u64), "list_patches shows it");
        assert_eq!(
            listed["patches"][0]["namespace"],
            json!("patches"),
            "in the region"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["import_patch MYART"]),
            "one undoable edit labelled as documented"
        );
    }

    /// The alias stores the same picture with the same measurements.
    #[tokio::test]
    async fn the_alias_imports_the_same_picture() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "MYART.png", &hole_png());
        let main = call(
            &client,
            "import_image_as_patch",
            source_args(&file, json!({ "name": "MAIN" })),
        )
        .await;
        let alias = call(
            &client,
            "import_png_as_patch",
            source_args(&file, json!({ "name": "ALIAS" })),
        )
        .await;
        assert_same_pipeline(&main, &alias);
        assert_eq!(
            alias["pnames_index"],
            json!(3_u64),
            "and lists in PNAMES too"
        );
        assert_eq!(
            patch_rgba(&client, "ALIAS").await,
            patch_rgba(&client, "MAIN").await,
            "the stored bytes match"
        );
    }

    /// Colour-key mode drops exactly the key colour, default and explicit.
    #[tokio::test]
    async fn the_colour_key_mode_drops_the_key_colour() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "CYAN.png", &cyan_png());
        let keyed = call(
            &client,
            "import_image_as_patch",
            source_args(
                &file,
                json!({ "name": "KEYED", "transparency": { "mode": "color" } }),
            ),
        )
        .await;
        assert_eq!(
            keyed["transparent_pixels"],
            json!(64_u64),
            "the default cyan key empties the picture"
        );
        let rgba = patch_rgba(&client, "KEYED").await;
        assert_eq!(
            rgba.len(),
            PIXELS.checked_mul(4).expect("rgba size"),
            "the shape"
        );
        assert!(
            rgba.chunks_exact(4).all(|pixel| pixel[3] == 0_u8),
            "every pixel is a hole"
        );
        let explicit = call(
            &client,
            "import_image_as_patch",
            source_args(
                &file,
                json!({
                    "name": "EXPLICIT",
                    "transparency": { "mode": "color", "color": "#00FFFF" },
                }),
            ),
        )
        .await;
        assert_eq!(
            explicit["transparent_pixels"],
            json!(64_u64),
            "the explicit key matches the default"
        );
    }

    /// `none` keeps every pixel opaque, thresholds included.
    #[tokio::test]
    async fn opaque_mode_keeps_everything() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "CYAN.png", &cyan_png());
        let opaque = call(
            &client,
            "import_image_as_patch",
            source_args(
                &file,
                json!({ "name": "OPAQUE", "transparency": { "mode": "none" } }),
            ),
        )
        .await;
        assert_eq!(
            opaque["transparent_pixels"],
            json!(0_u64),
            "none keeps everything"
        );
        assert_eq!(opaque["colors_used"], json!(1_u64), "one distinct colour");
        let solid = patch_rgba(&client, "OPAQUE").await;
        assert!(
            solid.chunks_exact(4).all(|pixel| pixel[3] == 255_u8),
            "all opaque"
        );
        let hole = write_file(
            dir.path(),
            "SOFT.png",
            &palette_png(SIDE, SIDE, Some(HOLE), None),
        );
        let strict = call(
            &client,
            "import_image_as_patch",
            source_args(
                &hole,
                json!({ "name": "STRICT", "transparency": { "mode": "alpha", "threshold": 0_u64 } }),
            ),
        )
        .await;
        assert_eq!(
            strict["transparent_pixels"],
            json!(0_u64),
            "threshold 0 drops nothing"
        );
    }

    /// Offsets default to the `grAb` chunk and `"zero"` overrides it.
    #[tokio::test]
    async fn offsets_default_to_the_grab_chunk() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let grabbed = write_file(
            dir.path(),
            "GRABBED.png",
            &palette_png(SIDE, SIDE, None, Some((7_i32, -9_i32))),
        );
        let taken = call(
            &client,
            "import_image_as_patch",
            source_args(&grabbed, json!({ "name": "TAKEN" })),
        )
        .await;
        assert_eq!(taken["left_offset"], json!(7_i64), "grAb by default");
        assert_eq!(taken["top_offset"], json!(-9_i64), "grAb by default");
        let off = call(
            &client,
            "import_image_as_patch",
            source_args(&grabbed, json!({ "name": "IGNORED", "offsets": "zero" })),
        )
        .await;
        assert_eq!(
            off["left_offset"],
            json!(0_i64),
            "\"zero\" outranks the chunk"
        );
        assert_eq!(off["top_offset"], json!(0_i64), "\"zero\" both ways");
        let plain = write_file(
            dir.path(),
            "PLAIN.png",
            &palette_png(SIDE, SIDE, None, None),
        );
        let fallback = call(
            &client,
            "import_image_as_patch",
            source_args(&plain, json!({ "name": "FALLBACK", "offsets": "grab" })),
        )
        .await;
        assert_eq!(
            fallback["left_offset"],
            json!(0_i64),
            "grab without a chunk uses the tool default"
        );
    }

    /// The presets and the explicit pair place the origin as documented.
    #[tokio::test]
    async fn offset_presets_and_literals_place_the_origin() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let plain = write_file(
            dir.path(),
            "PLAIN.png",
            &palette_png(SIDE, SIDE, None, None),
        );
        let bottom = call(
            &client,
            "import_image_as_patch",
            source_args(
                &plain,
                json!({ "name": "BOTTOM", "offsets": "center_bottom" }),
            ),
        )
        .await;
        assert_eq!(bottom["left_offset"], json!(4_i64), "width/2");
        assert_eq!(bottom["top_offset"], json!(8_i64), "full height");
        let middle = call(
            &client,
            "import_image_as_patch",
            source_args(&plain, json!({ "name": "MIDDLE", "offsets": "center" })),
        )
        .await;
        assert_eq!(middle["left_offset"], json!(4_i64), "width/2");
        assert_eq!(middle["top_offset"], json!(4_i64), "height/2");
        let literal = call(
            &client,
            "import_image_as_patch",
            source_args(
                &plain,
                json!({ "name": "LITERAL", "offsets": { "x": -2_i64, "y": 3_i64 } }),
            ),
        )
        .await;
        assert_eq!(literal["left_offset"], json!(-2_i64), "the literal left");
        assert_eq!(literal["top_offset"], json!(3_i64), "the literal top");
    }

    /// `PNAMES` grows with the import unless the tool opts out.
    #[tokio::test]
    async fn pnames_lists_the_import_and_opt_out_leaves_it_alone() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "LISTED.png",
            &palette_png(SIDE, SIDE, None, None),
        );
        let listed = call(
            &client,
            "import_image_as_patch",
            source_args(&file, json!({ "name": "LISTED" })),
        )
        .await;
        assert_eq!(
            listed["pnames_index"],
            json!(2_u64),
            "appended after the fixture pair"
        );
        let after = call(&client, "list_pnames", json!({})).await;
        assert_eq!(after["total"], json!(3_u64), "PNAMES grew by one");
        assert_eq!(after["pnames"][2]["name"], json!("LISTED"), "the new entry");
        let skipped = call(
            &client,
            "import_image_as_patch",
            source_args(&file, json!({ "name": "UNLISTED", "add_to_pnames": false })),
        )
        .await;
        assert_eq!(skipped["pnames_index"], Value::Null, "opted out");
        let still = call(&client, "list_pnames", json!({})).await;
        assert_eq!(still["total"], json!(3_u64), "PNAMES did not grow");
        assert_eq!(
            still["pnames"][2]["name"],
            json!("LISTED"),
            "no UNLISTED entry"
        );
    }

    /// A name `PNAMES` already lists keeps its index and changes nothing.
    #[tokio::test]
    async fn an_already_listed_name_keeps_its_pnames_entry() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "WALLP1.png",
            &palette_png(SIDE, SIDE, None, None),
        );
        let reused = call(
            &client,
            "import_image_as_patch",
            source_args(&file, json!({ "name": "WALLP1" })),
        )
        .await;
        assert_eq!(
            reused["pnames_index"],
            json!(0_u64),
            "a name PNAMES already lists keeps its index"
        );
        let unchanged = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            unchanged["total"],
            json!(FIXTURE_PNAMES),
            "the reused name adds nothing"
        );
    }

    /// A same-name working lump refuses until `replace`, and each import
    /// undoes separately.
    #[tokio::test]
    async fn a_duplicate_needs_replace_and_overwrites_in_place() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "TWIN.png", &palette_png(SIDE, SIDE, None, None));
        let first = import_twin(&client, &file, json!({ "name": "TWIN" })).await;
        let (code, message) = import_twin_err(&client, &file, json!({ "name": "TWIN" })).await;
        assert_eq!(code, "already_exists", "the twin refuses");
        assert!(
            message.contains("replace=true"),
            "the message names the way out: {message}"
        );
        let replaced =
            import_twin(&client, &file, json!({ "name": "TWIN", "replace": true })).await;
        assert_eq!(
            replaced["lump_index"], first["lump_index"],
            "replace overwrites in place"
        );
        assert_eq!(
            replaced["pnames_index"],
            json!(2_u64),
            "the entry stays put"
        );
        let working = call(
            &client,
            "list_lumps",
            json!({ "scope": "working", "namespace": "patches" }),
        )
        .await;
        let twins = working["lumps"]
            .as_array()
            .expect("a listing")
            .iter()
            .filter(|row| row["name"] == json!("TWIN"))
            .count();
        assert_eq!(twins, 1, "one copy stands");
    }

    /// One successful `import_image_as_patch` of `file` with `extra`.
    async fn import_twin(client: &Client, file: &Path, extra: Value) -> Value {
        call(client, "import_image_as_patch", source_args(file, extra)).await
    }

    /// One failing `import_image_as_patch` of `file` with `extra`.
    async fn import_twin_err(client: &Client, file: &Path, extra: Value) -> (String, String) {
        call_err(client, "import_image_as_patch", source_args(file, extra)).await
    }

    /// Undo steps each import back separately, the `PNAMES` override
    /// included.
    #[tokio::test]
    async fn undo_reverts_the_import_and_its_pnames_write() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "TWIN.png", &palette_png(SIDE, SIDE, None, None));
        call(
            &client,
            "import_image_as_patch",
            source_args(&file, json!({ "name": "TWIN" })),
        )
        .await;
        let replaced = call(
            &client,
            "import_image_as_patch",
            source_args(&file, json!({ "name": "TWIN", "replace": true })),
        )
        .await;
        assert_eq!(
            replaced["pnames_index"],
            json!(2_u64),
            "the second edit landed"
        );
        let undone = call(&client, "undo", json!({ "steps": 2_u64 })).await;
        assert_eq!(
            undone["undone"],
            json!(["import_patch TWIN", "import_patch TWIN"]),
            "both imports undo separately"
        );
        let gone = call_err(&client, "get_patch", json!({ "name": "TWIN" })).await;
        assert_eq!(gone.0, "not_found", "the lump left with its edit");
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames["total"],
            json!(FIXTURE_PNAMES),
            "the PNAMES override undid too"
        );
    }

    /// An unnamed import derives the lump name from the file stem.
    #[tokio::test]
    async fn the_name_comes_from_the_filename_when_unnamed() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "my_art.png",
            &palette_png(SIDE, SIDE, None, None),
        );
        let report = call(
            &client,
            "import_image_as_patch",
            source_args(&file, Value::Null),
        )
        .await;
        assert_eq!(report["name"], json!("MY_ART"), "the stem uppercases");
        assert_eq!(report["pnames_index"], json!(2_u64), "and PNAMES lists it");
    }

    /// A patch-import refusal: arguments plus the expected `(code,
    /// message-substring)` pair and a debug label.
    type Refusal = (&'static str, Value, &'static str, &'static str);

    /// Assert every `tool` call in `cases` refuses as documented.
    async fn refuse_all(client: &Client, tool: &str, cases: &[Refusal]) {
        for case in cases {
            let (label, expect_code, expect_text) = (case.0, case.2, case.3);
            let (code, message) = call_err(client, tool, case.1.clone()).await;
            assert_eq!(code, expect_code, "{label} code: {message}");
            assert!(
                message.contains(expect_text),
                "{label} message should name {expect_text:?}: {message}"
            );
        }
    }

    /// Bad sources and parameters each report their code, and nothing is
    /// stored along the way.
    #[tokio::test]
    async fn bad_images_and_parameters_are_refused_without_edits() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let junk = write_file(dir.path(), "junk.txt", b"this is a text file, not an image");
        let qoi = write_file(dir.path(), "picture.qoi", b"qoif\0\0\0\x03\0\0\0\x02");
        let unnamed = write_file(dir.path(), "###.png", &palette_png(SIDE, SIDE, None, None));
        let cases: [Refusal; 7] = [
            (
                "junk",
                source_args(&junk, Value::Null),
                "format_error",
                "image",
            ),
            ("qoi", source_args(&qoi, Value::Null), "unsupported", "PNG"),
            (
                "unnamed",
                source_args(&unnamed, Value::Null),
                "invalid_argument",
                "derive",
            ),
            (
                "long name",
                source_args(&junk, json!({ "name": "TOO LONG NAME" })),
                "invalid_argument",
                "name",
            ),
            (
                "mode",
                source_args(
                    &junk,
                    json!({ "name": "X", "transparency": { "mode": "blur" } }),
                ),
                "invalid_argument",
                "\"none\"",
            ),
            (
                "preset",
                source_args(&junk, json!({ "name": "X", "offsets": "middle" })),
                "invalid_argument",
                "center_bottom",
            ),
            (
                "missing",
                json!({ "source": dir.path().join("absent.png").display().to_string() }),
                "not_found",
                "absent.png",
            ),
        ];
        refuse_all(&client, "import_image_as_patch", &cases).await;
        let empty = call(&client, "list_patches", json!({})).await;
        assert_eq!(empty["total"], json!(2_u64), "nothing was stored");
    }

    /// A decodable picture still checks the palette index and the `i16`
    /// offset range.
    #[tokio::test]
    async fn a_valid_picture_still_checks_palette_and_offset_range() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let good = write_file(dir.path(), "GOOD.png", &palette_png(SIDE, SIDE, None, None));
        let cases: [Refusal; 2] = [
            (
                "palette",
                source_args(&good, json!({ "name": "X", "palette": 99_u64 })),
                "not_found",
                "PLAYPAL",
            ),
            (
                "huge offset",
                source_args(
                    &good,
                    json!({ "name": "X", "offsets": { "x": 40000_i64, "y": 0_i64 } }),
                ),
                "invalid_argument",
                "does not fit",
            ),
        ];
        refuse_all(&client, "import_image_as_patch", &cases).await;
    }

    /// Without a `PLAYPAL` the quantization step has nothing to answer.
    #[tokio::test]
    async fn a_session_without_a_playpal_cannot_quantize() {
        let client = connect().await;
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = write_file(dir.path(), "ART.png", &palette_png(SIDE, SIDE, None, None));
        let (code, message) = call_err(
            &client,
            "import_image_as_patch",
            source_args(&file, json!({ "name": "ART" })),
        )
        .await;
        assert_eq!(code, "not_found", "no palette to quantize against");
        assert!(
            message.contains("PLAYPAL"),
            "the message names it: {message}"
        );
    }

    /// The `source` also accepts this server's own upload `URL`s.
    #[tokio::test]
    async fn a_source_can_be_an_uploaded_asset_url() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let info = call(&client, "get_asset_server_info", json!({})).await;
        let base = info["base_url"]
            .as_str()
            .expect("an asset origin")
            .to_owned();
        let (status, receipt) = http_put(&base, "/upload/UPLOADED.png", &hole_png()).await;
        assert_eq!(status, 201, "the upload is accepted");
        let url: String = serde_json::from_slice::<Value>(&receipt).expect("a JSON receipt")["url"]
            .as_str()
            .expect("the receipt's url")
            .to_owned();
        let report = call(&client, "import_image_as_patch", json!({ "source": url })).await;
        assert_eq!(report["name"], json!("UPLOADED"), "the url names the lump");
        assert_eq!(
            report["transparent_pixels"],
            json!(1_u64),
            "the hole survived"
        );
        assert_eq!(patch_rgba(&client, "UPLOADED").await, expected_rgba());
    }

    /// The palette colour index for flat raster position `p`.
    fn cycled(p: usize) -> usize {
        p.checked_rem(CYCLE).expect("non-zero cycle")
    }

    /// The `width`x`height` fully opaque raster cycling palette colours.
    fn flat_raster(width: u32, height: u32) -> RgbaImage {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let pixels: Vec<u8> = (0..count).flat_map(|p| color_pixel(cycled(p))).collect();
        RgbaImage::new(width, height, pixels).expect("test shapes are valid")
    }

    /// The `RGBA` of flat raster position `p`: its cycled palette colour,
    /// fully transparent when `transparent` picks it.
    fn flat_pixel(p: usize, transparent: &impl Fn(usize) -> bool) -> [u8; 4] {
        let mut pixel = color_pixel(cycled(p));
        if transparent(p) {
            pixel[3] = 0_u8;
        }
        pixel
    }

    /// A palette PNG of `width` by `height` cycling palette colours, with
    /// alpha `0` where `transparent` picks the pixel.
    fn flat_png(width: u32, height: u32, transparent: impl Fn(usize) -> bool) -> Vec<u8> {
        let count = usize::try_from(width)
            .expect("test width")
            .checked_mul(usize::try_from(height).expect("test height"))
            .expect("test pixels fit");
        let pixels: Vec<u8> = (0..count)
            .flat_map(|p| flat_pixel(p, &transparent))
            .collect();
        let img = RgbaImage::new(width, height, pixels).expect("test shapes are valid");
        encode_png(&img, None).expect("test PNGs encode")
    }

    /// The `RGBA` bytes a stored [`flat_png`] answers back through
    /// `get_flat`: every cycled palette colour, fully opaque.
    fn flat_expected_rgba(count: usize) -> Vec<u8> {
        (0..count).flat_map(|p| color_pixel(cycled(p))).collect()
    }

    /// The `RGBA` bytes of the stored flat `name`, via `get_flat`.
    async fn flat_rgba(client: &Client, name: &str) -> Vec<u8> {
        let report = call(
            client,
            "get_flat",
            json!({ "name": name, "format": "rgba" }),
        )
        .await;
        let url = report["asset"]["url"]
            .as_str()
            .expect("get_flat publishes an asset");
        fetch_asset(url).await
    }

    /// One successful `import_image_as_flat` of `file` with `extra`.
    async fn flat_import(client: &Client, file: &Path, extra: Value) -> Value {
        call(client, "import_image_as_flat", source_args(file, extra)).await
    }

    /// One failing `import_image_as_flat` of `file` with `extra`.
    async fn flat_import_err(client: &Client, file: &Path, extra: Value) -> (String, String) {
        call_err(client, "import_image_as_flat", source_args(file, extra)).await
    }

    /// Assert a flat report stores the given shape and resize verdict.
    fn assert_flat_shape(report: &Value, width: u64, height: u64, resized: bool) {
        assert_eq!(report["width"], json!(width), "the stored width");
        assert_eq!(report["height"], json!(height), "the stored height");
        assert_eq!(report["resized"], json!(resized), "the resize verdict");
    }

    /// Assert two flat-import reports agree on every measured field.
    fn assert_same_flat(main: &Value, alias: &Value) {
        for field in [
            "width",
            "height",
            "resized",
            "colors_used",
            "max_color_error",
            "transparent_pixels_ignored",
        ] {
            assert_eq!(
                alias[field], main[field],
                "the alias reports {field} identically"
            );
        }
    }

    /// The 64x64 exact-palette round trip: the flat stores the picture and
    /// `get_flat` answers the pixels back unchanged.
    #[tokio::test]
    async fn an_exact_flat_png_imports_and_reads_back_pixel_perfect() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "MYFLAT.png",
            &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
        );
        let report = flat_import(&client, &file, json!({ "name": "MYFLAT" })).await;
        assert_eq!(report["name"], json!("MYFLAT"), "the name echoes");
        assert_eq!(
            report["lump_index"],
            json!(1_u64),
            "after the created F_START"
        );
        assert_flat_shape(&report, 64_u64, 64_u64, false);
        assert_eq!(
            report["colors_used"],
            json!(256_u64),
            "every palette colour"
        );
        assert_eq!(
            report["max_color_error"],
            json!(0_i64),
            "the palette is exact"
        );
        assert_eq!(
            report["transparent_pixels_ignored"],
            json!(0_u64),
            "an opaque source"
        );
        assert_eq!(
            flat_rgba(&client, "MYFLAT").await,
            flat_expected_rgba(FLAT_PIXELS),
            "the same pixels back"
        );
        let listed = call(&client, "list_flats", json!({ "pattern": "MYFLAT" })).await;
        assert_eq!(listed["total"], json!(1_u64), "list_flats shows it");
        assert_eq!(
            listed["flats"][0]["source"],
            json!("working"),
            "in the working archive"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["import_flat MYFLAT"]),
            "one undoable edit labelled as documented"
        );
    }

    /// A 100x50 picture without `resize` refuses, naming the shapes and the
    /// option, and stores nothing.
    #[tokio::test]
    async fn an_odd_sized_picture_without_resize_is_refused() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 50_u32, |_| false));
        let (code, message) = flat_import_err(&client, &file, json!({ "name": "ODD" })).await;
        assert_eq!(code, "invalid_argument", "no resize, no flat");
        assert!(
            message.contains("64x128"),
            "the message names the shapes: {message}"
        );
        assert!(
            message.contains("resize"),
            "the message names the option: {message}"
        );
        let listed = call(&client, "list_flats", json!({ "pattern": "ODD" })).await;
        assert_eq!(listed["total"], json!(0_u64), "nothing was stored");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "no edit landed");
    }

    /// `resize: "nearest"` scales to 64x64 by default and to 128x128 when
    /// asked, storing the nearest samples; a 64x128 picture is kept.
    #[tokio::test]
    async fn resizing_nearest_scales_the_picture_to_the_square_size() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 50_u32, |_| false));
        let small = flat_import(
            &client,
            &file,
            json!({ "name": "SMALL", "resize": "nearest" }),
        )
        .await;
        assert_flat_shape(&small, 64_u64, 64_u64, true);
        let scaled = resize_nearest(&flat_raster(100_u32, 50_u32), FLAT_SIDE, FLAT_SIDE);
        assert_eq!(flat_rgba(&client, "SMALL").await, scaled.pixels);
        let big = flat_import(
            &client,
            &file,
            json!({ "name": "BIG", "resize": "nearest", "size": 128_u64 }),
        )
        .await;
        assert_flat_shape(&big, 128_u64, 128_u64, true);
        let flats = call(&client, "list_flats", json!({ "pattern": "BIG" })).await;
        assert_eq!(
            flats["flats"][0]["size"],
            json!(0x4000_u64),
            "the raw 128x128 byte count"
        );
        let tall = write_file(
            dir.path(),
            "TALL.png",
            &flat_png(FLAT_SIDE, 128_u32, |_| false),
        );
        let kept = flat_import(&client, &tall, json!({ "name": "TALL" })).await;
        assert_flat_shape(&kept, 64_u64, 128_u64, false);
    }

    /// A flat only the `IWAD` holds does not clash: the import lands as a
    /// working override the merged view resolves to.
    #[tokio::test]
    async fn a_flat_import_overrides_the_iwad_copy() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "FLOOR1.png",
            &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
        );
        let report = flat_import(&client, &file, json!({ "name": "FLOOR1" })).await;
        assert_eq!(report["name"], json!("FLOOR1"), "the override imports");
        assert_eq!(report["lump_index"], json!(1_u64), "in the created region");
        let listed = call(&client, "list_flats", json!({ "pattern": "FLOOR1" })).await;
        assert_eq!(
            listed["total"],
            json!(1_u64),
            "last wins in the merged view"
        );
        assert_eq!(
            listed["flats"][0]["source"],
            json!("working"),
            "the working copy serves"
        );
        assert_eq!(
            flat_rgba(&client, "FLOOR1").await,
            flat_expected_rgba(FLAT_PIXELS),
            "the new pixels serve"
        );
    }

    /// A same-name working flat refuses until `replace`, which overwrites
    /// in place.
    #[tokio::test]
    async fn a_flat_duplicate_needs_replace_and_overwrites_in_place() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "TWIN.png",
            &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
        );
        let first = flat_import(&client, &file, json!({ "name": "TWIN" })).await;
        let (code, message) = flat_import_err(&client, &file, json!({ "name": "TWIN" })).await;
        assert_eq!(code, "already_exists", "the twin refuses");
        assert!(message.contains("flats"), "names the region: {message}");
        assert!(
            message.contains("replace=true"),
            "the message names the way out: {message}"
        );
        let replaced =
            flat_import(&client, &file, json!({ "name": "TWIN", "replace": true })).await;
        assert_eq!(
            replaced["lump_index"], first["lump_index"],
            "replace overwrites in place"
        );
        let listed = call(&client, "list_flats", json!({ "pattern": "TWIN" })).await;
        assert_eq!(listed["total"], json!(1_u64), "one copy stands");
    }

    /// The alias stores the same flat with the same measurements.
    #[tokio::test]
    async fn the_flat_alias_imports_the_same_picture() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "MYFLAT.png",
            &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
        );
        let main = flat_import(&client, &file, json!({ "name": "MAIN" })).await;
        let alias = call(
            &client,
            "import_png_as_flat",
            source_args(&file, json!({ "name": "ALIAS" })),
        )
        .await;
        assert_same_flat(&main, &alias);
        assert_eq!(
            flat_rgba(&client, "ALIAS").await,
            flat_rgba(&client, "MAIN").await,
            "the stored pixels match"
        );
    }

    /// Alpha below 128 is counted, kept opaque, and stored at its colour.
    #[tokio::test]
    async fn flat_imports_keep_alpha_pixels_and_count_them() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let picked = |p: usize| p.checked_rem(7_usize).is_some_and(|rest| rest == 0_usize);
        let expected = (0..FLAT_PIXELS).filter(|p| picked(*p)).count();
        let file = write_file(
            dir.path(),
            "SEMI.png",
            &flat_png(FLAT_SIDE, FLAT_SIDE, picked),
        );
        let report = flat_import(&client, &file, json!({ "name": "SEMI" })).await;
        assert_eq!(
            report["transparent_pixels_ignored"],
            json!(u64::try_from(expected).expect("test count fits")),
            "every seventh pixel had alpha below 128"
        );
        assert_eq!(
            report["colors_used"],
            json!(256_u64),
            "the alpha carried no colour information away"
        );
        assert_eq!(
            flat_rgba(&client, "SEMI").await,
            flat_expected_rgba(FLAT_PIXELS),
            "all pixels stored opaque at their colours"
        );
    }

    /// Bad flat sources and parameters each report their code, and nothing
    /// is stored along the way.
    #[tokio::test]
    async fn bad_flat_sources_and_parameters_are_refused() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let good = write_file(
            dir.path(),
            "GOOD.png",
            &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
        );
        let junk = write_file(dir.path(), "junk.txt", b"this is a text file, not an image");
        let unnamed = write_file(
            dir.path(),
            "###.png",
            &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
        );
        let cases: [Refusal; 6] = [
            (
                "missing",
                json!({ "source": dir.path().join("absent.png").display().to_string() }),
                "not_found",
                "absent.png",
            ),
            (
                "junk",
                source_args(&junk, Value::Null),
                "format_error",
                "image",
            ),
            (
                "unnamed",
                source_args(&unnamed, Value::Null),
                "invalid_argument",
                "derive",
            ),
            (
                "resize",
                source_args(&junk, json!({ "name": "X", "resize": "linear" })),
                "invalid_argument",
                "\"nearest\"",
            ),
            (
                "size",
                source_args(
                    &junk,
                    json!({ "name": "X", "resize": "nearest", "size": 32_u64 }),
                ),
                "invalid_argument",
                "64, 128, 256",
            ),
            (
                "palette",
                source_args(&good, json!({ "name": "X", "palette": 99_u64 })),
                "not_found",
                "PLAYPAL",
            ),
        ];
        refuse_all(&client, "import_image_as_flat", &cases).await;
        let flats = call(&client, "list_flats", json!({})).await;
        assert_eq!(flats["total"], json!(FIXTURE_FLATS), "nothing was stored");
    }

    /// Without a `PLAYPAL` the quantization step has nothing to answer.
    #[tokio::test]
    async fn a_flat_import_without_a_playpal_cannot_quantize() {
        let client = connect().await;
        let dir = tempfile::tempdir().expect("a scratch directory");
        let file = write_file(
            dir.path(),
            "ART.png",
            &flat_png(FLAT_SIDE, FLAT_SIDE, |_| false),
        );
        let (code, message) = flat_import_err(&client, &file, json!({ "name": "ART" })).await;
        assert_eq!(code, "not_found", "no palette to quantize against");
        assert!(
            message.contains("PLAYPAL"),
            "the message names it: {message}"
        );
    }

    /// A raw import stores the file's own bytes, loose by default, and
    /// never touches `PNAMES`.
    #[tokio::test]
    async fn raw_imports_keep_the_original_bytes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let png = palette_png(4_u32, 4_u32, None, Some((2_i32, 6_i32)));
        let file = write_file(dir.path(), "RAWTEX.png", &png);
        let report = call(
            &client,
            "import_image_as_raw",
            source_args(&file, Value::Null),
        )
        .await;
        assert_eq!(report["name"], json!("RAWTEX"), "derived from the file");
        assert_eq!(report["format"], json!("png"), "the sniffed format");
        assert_eq!(report["width"], json!(4_u64), "the decoded width");
        assert_eq!(report["height"], json!(4_u64), "the decoded height");
        let out = dir.path().join("roundtrip.png");
        call(
            &client,
            "extract_lump",
            json!({ "name": "RAWTEX", "path": out.display().to_string() }),
        )
        .await;
        assert_eq!(
            std::fs::read(&out).expect("read back"),
            png,
            "byte for byte"
        );
        let listed = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        let rows = listed["lumps"].as_array().expect("a listing");
        assert_eq!(rows.len(), 1, "one loose lump");
        assert_eq!(rows[0]["name"], json!("RAWTEX"), "the stored lump");
        assert_eq!(rows[0]["namespace"], Value::Null, "stored loose");
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(pnames["total"], json!(FIXTURE_PNAMES), "PNAMES untouched");
    }

    /// A raw import joins a named region and obeys the duplicate rule.
    #[tokio::test]
    async fn raw_imports_place_by_namespace_and_obey_replace() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "INREG.png",
            &palette_png(4_u32, 4_u32, None, None),
        );
        let placed = call(
            &client,
            "import_image_as_raw",
            source_args(&file, json!({ "namespace": "patches" })),
        )
        .await;
        assert_eq!(
            placed["lump_index"],
            json!(1_u64),
            "inside the created region"
        );
        let listed = call(
            &client,
            "list_lumps",
            json!({ "scope": "working", "namespace": "patches" }),
        )
        .await;
        assert_eq!(
            listed["lumps"][1]["namespace"],
            json!("patches"),
            "the region classifies it"
        );
        let (code, message) = call_err(
            &client,
            "import_image_as_raw",
            source_args(&file, json!({ "namespace": "patches" })),
        )
        .await;
        assert_eq!(code, "already_exists", "the twin refuses");
        assert!(
            message.contains("replace=true"),
            "the message names the way out: {message}"
        );
        let replaced = call(
            &client,
            "import_image_as_raw",
            source_args(&file, json!({ "namespace": "patches", "replace": true })),
        )
        .await;
        assert_eq!(
            replaced["lump_index"], placed["lump_index"],
            "replace overwrites in place"
        );
    }

    /// A raw import names the format, refuses non-images and bad regions.
    #[tokio::test]
    async fn raw_imports_name_the_format_and_refuse_non_images() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let jpeg = write_file(dir.path(), "PHOTO.jpg", &jpeg_of());
        let report = call(
            &client,
            "import_image_as_raw",
            source_args(&jpeg, Value::Null),
        )
        .await;
        assert_eq!(report["format"], json!("jpeg"), "the sniffed format");
        assert_eq!(report["name"], json!("PHOTO"), "the stem names it");
        let junk = write_file(
            dir.path(),
            "notes.txt",
            b"this is a text file, not an image",
        );
        let (code, message) = call_err(
            &client,
            "import_image_as_raw",
            source_args(&junk, Value::Null),
        )
        .await;
        assert_eq!(code, "format_error", "a non-image refuses");
        assert!(message.contains("image"), "the message explains: {message}");
        let empty = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(empty["total"], json!(1_u64), "only the JPEG landed");
        let (bad_region, _) = call_err(
            &client,
            "import_image_as_raw",
            source_args(&jpeg, json!({ "namespace": "nope", "name": "SECOND" })),
        )
        .await;
        assert_eq!(bad_region, "invalid_argument", "an unknown region refuses");
    }

    /// One successful `import_image_as_sprite` of `file` with `extra`.
    async fn sprite_import(client: &Client, file: &Path, extra: Value) -> Value {
        call(client, "import_image_as_sprite", source_args(file, extra)).await
    }

    /// The `RGBA` bytes of the sprite lump `name` as stored, via
    /// `get_sprite`.
    async fn sprite_rgba(client: &Client, name: &str) -> Vec<u8> {
        let report = call(
            client,
            "get_sprite",
            json!({ "name": name, "format": "rgba" }),
        )
        .await;
        let url = report["asset"]["url"]
            .as_str()
            .expect("get_sprite publishes an asset");
        fetch_asset(url).await
    }

    /// The rotation row `list_sprites` answers with for `sprite` frame `A`.
    async fn listed_rotation(client: &Client, sprite: &str, index: usize) -> Value {
        let listed = call(client, "list_sprites", json!({ "prefix": sprite })).await;
        listed["sprites"][0]["frames"][0]["rotations"][index].clone()
    }
    /// What a sprite report should name: the lump name, its four-character
    /// prefix, its frame letter and the rotation the image serves.
    type SpriteNaming = (&'static str, &'static str, &'static str, u64);

    /// Assert a sprite report's four naming fields.
    fn assert_naming(report: &Value, naming: SpriteNaming) {
        let (name, sprite, frame, rotation) = naming;
        assert_eq!(report["name"], json!(name), "the name echoes");
        assert_eq!(report["sprite"], json!(sprite), "the four-character prefix");
        assert_eq!(report["frame"], json!(frame), "the frame letter");
        assert_eq!(
            report["rotation"],
            json!(rotation),
            "the rotation the image itself serves"
        );
    }

    /// Assert a report carries the `center_bottom` default for the 8x8 test
    /// picture: feet on the origin, half the width to the left.
    fn assert_feet_on_origin(report: &Value) {
        assert_eq!(
            report["left_offset"],
            json!(4_i64),
            "center_bottom is width/2"
        );
        assert_eq!(
            report["top_offset"],
            json!(8_i64),
            "center_bottom is the full height"
        );
    }

    /// The sprite round trip: a whole lump name, the shared pipeline
    /// measurements, the default feet-on-origin offsets and the undo label.
    #[tokio::test]
    async fn a_png_imports_as_a_sprite_under_its_full_name() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "BOBA.png", &hole_png());
        let report = sprite_import(&client, &file, json!({ "name": "BOBAA1" })).await;
        assert_naming(&report, ("BOBAA1", "BOBA", "A", 1));
        assert_eq!(
            report["mirror"],
            Value::Null,
            "a six-character name mirrors nothing"
        );
        assert_eq!(
            report["warnings"],
            json!([]),
            "a fresh sprite clashes with nothing"
        );
        assert_eq!(
            report["lump_index"],
            json!(1_u64),
            "after the created S_START"
        );
        assert_feet_on_origin(&report);
        assert_eq!(
            report["transparent_pixels"],
            json!(1_u64),
            "the alpha hole dropped one pixel"
        );
        assert_eq!(report["colors_used"], json!(63_u64), "63 palette colours");
        assert_eq!(
            report["max_color_error"],
            json!(0_i64),
            "the palette is exact"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["import_sprite BOBAA1"]),
            "one undoable edit labelled as documented"
        );
    }

    /// The stored sprite sits in the working sprites region and reads back
    /// through `get_sprite` and `list_sprites`.
    #[tokio::test]
    async fn an_imported_sprite_reads_back_through_the_graphics_tools() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "BOBA.png", &hole_png());
        sprite_import(&client, &file, json!({ "name": "BOBAA1" })).await;
        let shown = call(
            &client,
            "get_sprite",
            json!({ "sprite": "BOBA", "frame": "A", "rotation": 1_u64 }),
        )
        .await;
        assert_eq!(shown["lump"], json!("BOBAA1"), "the new lump serves it");
        assert_eq!(shown["mirrored"], json!(false), "used as stored");
        assert_feet_on_origin(&shown);
        assert_eq!(
            listed_rotation(&client, "BOBA", 0).await,
            json!({ "rotation": 1_u64, "lump": "BOBAA1", "mirrored": false }),
            "list_sprites shows the new frame"
        );
        let working = call(
            &client,
            "list_lumps",
            json!({ "scope": "working", "namespace": "sprites" }),
        )
        .await;
        assert_eq!(
            working["lumps"][1]["name"],
            json!("BOBAA1"),
            "inside the created region"
        );
    }

    /// The parts form assembles the eight-character mirror-pair name, and
    /// `get_sprite` serves the mirrored rotation flipped.
    #[tokio::test]
    async fn the_parts_form_builds_the_mirror_pair_name() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "BOBA.png", &hole_png());
        let report = sprite_import(
            &client,
            &file,
            json!({
                "sprite": "boba",
                "frame": "b",
                "rotation": 2_u64,
                "mirror_rotation": 8_u64,
            }),
        )
        .await;
        assert_naming(&report, ("BOBAB2B8", "BOBA", "B", 2));
        assert_eq!(report["mirror"], json!(8_u64), "the mirrored rotation");
        assert_eq!(
            report["warnings"],
            json!([]),
            "a fresh sprite clashes with nothing"
        );
        let flipped = call(
            &client,
            "get_sprite",
            json!({ "sprite": "BOBA", "frame": "B", "rotation": 8_u64 }),
        )
        .await;
        assert_eq!(
            flipped["lump"],
            json!("BOBAB2B8"),
            "the pair lump serves it"
        );
        assert_eq!(flipped["mirrored"], json!(true), "flipped for rotation 8");
        assert_eq!(
            flipped["left_offset"],
            json!(4_i64),
            "left' = width - left keeps a centred sprite centred"
        );
        assert_eq!(
            listed_rotation(&client, "BOBA", 1).await,
            json!({ "rotation": 8_u64, "lump": "BOBAB2B8", "mirrored": true }),
            "the listing carries both rotations"
        );
    }

    /// A `grAb` chunk beats the `center_bottom` default; a preset or an
    /// explicit word outranks the chunk.
    #[tokio::test]
    async fn sprite_offsets_take_grab_then_the_chosen_preset() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let grabbed = write_file(
            dir.path(),
            "GRAB.png",
            &palette_png(SIDE, SIDE, None, Some((7_i32, -9_i32))),
        );
        let taken = sprite_import(&client, &grabbed, json!({ "name": "BOBAA2" })).await;
        assert_eq!(taken["left_offset"], json!(7_i64), "grAb beats the default");
        assert_eq!(taken["top_offset"], json!(-9_i64), "grAb beats the default");
        let zeroed = sprite_import(
            &client,
            &grabbed,
            json!({ "name": "BOBAA3", "offsets": "zero" }),
        )
        .await;
        assert_eq!(
            zeroed["left_offset"],
            json!(0_i64),
            "\"zero\" outranks the chunk"
        );
        assert_eq!(zeroed["top_offset"], json!(0_i64), "\"zero\" both ways");
        let plain = write_file(
            dir.path(),
            "PLAIN.png",
            &palette_png(SIDE, SIDE, None, None),
        );
        let middle = sprite_import(
            &client,
            &plain,
            json!({ "name": "BOBAA4", "offsets": "center" }),
        )
        .await;
        assert_eq!(middle["left_offset"], json!(4_i64), "width/2");
        assert_eq!(middle["top_offset"], json!(4_i64), "height/2");
        let literal = sprite_import(
            &client,
            &plain,
            json!({ "name": "BOBAA5", "offsets": { "x": -2_i64, "y": 20_i64 } }),
        )
        .await;
        assert_eq!(literal["left_offset"], json!(-2_i64), "the literal left");
        assert_eq!(literal["top_offset"], json!(20_i64), "the literal top");
        assert_eq!(
            literal["warnings"],
            json!([]),
            "five views of one frame do not clash"
        );
    }

    /// Every sprite lump of one frame warns about nothing until the
    /// all-angles image or a covered rotation joins in.
    #[tokio::test]
    async fn a_sprite_import_warns_about_rotations_the_session_already_serves() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "TROO.png", &hole_png());
        let twin = sprite_import(&client, &file, json!({ "name": "TROOA2" })).await;
        let warnings = twin["warnings"].as_array().expect("a report");
        assert_eq!(warnings.len(), 1, "one overlap: {warnings:?}");
        let text = warnings[0].as_str().expect("a message");
        assert!(text.contains("rotation 2"), "names the rotation: {text}");
        assert!(text.contains("TROOA2A8"), "names the lump: {text}");
        let angles = sprite_import(&client, &file, json!({ "name": "PLAYA1" })).await;
        let mixed = angles["warnings"].as_array().expect("a report");
        assert_eq!(mixed.len(), 1, "one mixed frame: {mixed:?}");
        let message = mixed[0].as_str().expect("a message");
        assert!(message.contains("mixes"), "names the mixing: {message}");
        assert!(message.contains("PLAYA0"), "names the lump: {message}");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["import_sprite TROOA2", "import_sprite PLAYA1"]),
            "warnings never stop an import"
        );
    }

    /// A same-name working sprite refuses until `replace`, which overwrites
    /// in place and never warns about the lump it replaced.
    #[tokio::test]
    async fn a_sprite_duplicate_needs_replace_and_overwrites_in_place() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "BOBA.png", &hole_png());
        let first = sprite_import(&client, &file, json!({ "name": "BOBAA1" })).await;
        let (code, message) = call_err(
            &client,
            "import_image_as_sprite",
            source_args(&file, json!({ "name": "BOBAA1" })),
        )
        .await;
        assert_eq!(code, "already_exists", "the twin refuses");
        assert!(message.contains("sprites"), "names the region: {message}");
        assert!(
            message.contains("replace=true"),
            "the message names the way out: {message}"
        );
        let replaced =
            sprite_import(&client, &file, json!({ "name": "BOBAA1", "replace": true })).await;
        assert_eq!(
            replaced["lump_index"], first["lump_index"],
            "replace overwrites in place"
        );
        assert_eq!(
            replaced["warnings"],
            json!([]),
            "the copy is not an overlap"
        );
        let listed = call(&client, "list_sprites", json!({ "prefix": "BOBA" })).await;
        let rows = listed["sprites"][0]["frames"][0]["rotations"]
            .as_array()
            .expect("a listing");
        assert_eq!(rows.len(), 1, "one rotation stands, not two");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["import_sprite BOBAA1", "import_sprite BOBAA1"]),
            "each import is its own undoable edit"
        );
    }

    /// The sprite naming refusals over one readable `file`.
    fn sprite_refusals(file: &Path) -> [Refusal; 8] {
        [
            (
                "too long",
                source_args(file, json!({ "name": "TOOLONG1" })),
                "invalid_argument",
                "sprite lump name",
            ),
            (
                "rotation 9",
                source_args(file, json!({ "name": "TROOA9" })),
                "invalid_argument",
                "rotation digit",
            ),
            (
                "mirror equals rotation",
                source_args(
                    file,
                    json!({ "sprite": "BOBA", "rotation": 2_u64, "mirror_rotation": 2_u64 }),
                ),
                "invalid_argument",
                "equals rotation",
            ),
            (
                "mirror with rotation 0",
                source_args(
                    file,
                    json!({ "sprite": "BOBA", "rotation": 0_u64, "mirror_rotation": 8_u64 }),
                ),
                "invalid_argument",
                "no mirror partner",
            ),
            (
                "both forms",
                source_args(file, json!({ "name": "BOBAA1", "sprite": "BOBA" })),
                "invalid_argument",
                "not both",
            ),
            (
                "neither form",
                source_args(file, Value::Null),
                "invalid_argument",
                "needs a sprite",
            ),
            (
                "short prefix",
                source_args(file, json!({ "sprite": "BOB" })),
                "invalid_argument",
                "four ASCII",
            ),
            (
                "bad frame",
                source_args(file, json!({ "sprite": "BOBA", "frame": "AA" })),
                "invalid_argument",
                "frame letter",
            ),
        ]
    }

    /// Bad sprite names and naming combinations each report
    /// `invalid_argument`, and nothing is stored along the way.
    #[tokio::test]
    async fn bad_sprite_naming_is_refused_without_edits() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "BOBA.png", &hole_png());
        let cases = sprite_refusals(&file);
        refuse_all(&client, "import_image_as_sprite", &cases).await;
        let listed = call(&client, "list_sprites", json!({ "prefix": "BOBA" })).await;
        assert_eq!(listed["total"], json!(0_u64), "no sprite group landed");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "no edit landed");
    }

    /// The alias stores the same picture with the same measurements under
    /// the parts form too.
    #[tokio::test]
    async fn the_sprite_alias_imports_the_same_picture() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "BOBA.png", &hole_png());
        let main = sprite_import(&client, &file, json!({ "name": "BOBAA1" })).await;
        let alias = call(
            &client,
            "import_png_as_sprite",
            source_args(
                &file,
                json!({ "sprite": "POSS", "rotation": 3_u64, "mirror_rotation": 7_u64 }),
            ),
        )
        .await;
        assert_same_pipeline(&main, &alias);
        assert_naming(&alias, ("POSSA3A7", "POSS", "A", 3));
        assert_eq!(alias["mirror"], json!(7_u64), "and carries the pair");
        assert_eq!(
            sprite_rgba(&client, "BOBAA1").await,
            sprite_rgba(&client, "POSSA3A7").await,
            "the stored pixels match"
        );
    }
}
