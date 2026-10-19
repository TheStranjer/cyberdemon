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
//! Test pictures are written with the library's own `PNG` encoder. The
//! preview pipeline `process_image` (ops, quantized and `quantize: false`,
//! the op-index errors) and the one-shot `create_texture_from_image`
//! (the size rules, the automatic `fit`/`make_tileable` steps, the
//! `PNAMES`/patch/definition landing together, the layer-patch and
//! texture-name collisions with and without `replace`, `assign_role`
//! against the theme, and the single `undo`) close the picture-in side of
//! the group. The other two one-shots: `create_flat_from_image` (always
//! `64x64`, the default `tileable` seam blend, `contain` padding, the
//! layer-flat collision with the everywhere-used hint and its `replace`
//! override, `assign_role` for flat roles with the wall-role refusal, and
//! the single `undo`) and `create_sprite_from_image` (background removal
//! and trim with the feet-on-origin default, `remove_background`/`trim`
//! switches, the `height` fit and the too-tall note, `frames` writing one
//! lump per letter in one undoable step, the mirror-pair naming, the
//! rotation-coverage warnings, the all-background refusal hint, the
//! duplicate and `replace` rule and the naming-conflict catalogue).

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

    /// The `RGBA` bytes of the published asset at `url`.
    async fn asset_png_dimensions(url: &str) -> (u64, u64) {
        let bytes = fetch_asset(url).await;
        let image = cyberdemon::gfx::codec::decode_image(&bytes).expect("the asset is a PNG");
        let width = u64::from(image.width);
        let height = u64::from(image.height);
        (width, height)
    }

    /// `process_image` over a 100x60 palette picture with `crop` then
    /// `fit 64x64`: both assets decode to 64x64, the report has two
    /// steps, and the quantized pixels are all fixture-palette colours.
    #[tokio::test]
    async fn process_image_publishes_processed_and_quantized_previews() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "WALLPHOTO.png",
            &flat_png(100_u32, 60_u32, |_| false),
        );
        let report = call(
            &client,
            "process_image",
            source_args(
                &file,
                json!({
                    "ops": [
                        { "op": "crop", "x": 0_u64, "y": 0_u64, "width": 90_u64, "height": 60_u64 },
                        { "op": "fit", "width": 64_u64, "height": 64_u64, "mode": "cover" },
                    ],
                }),
            ),
        )
        .await;
        assert_eq!(report["width"], json!(64_u64), "the processed shape");
        assert_eq!(report["height"], json!(64_u64), "the processed shape");
        let steps = report["steps"].as_array().expect("a step list");
        assert_eq!(steps.len(), 2_usize, "crop then fit reported");
        assert_eq!(steps[0]["op"], json!("crop"), "in order");
        assert_eq!(steps[1]["op"], json!("fit"), "in order");
        let processed_url = report["processed"]["url"]
            .as_str()
            .expect("a processed URL");
        assert_eq!(asset_png_dimensions(processed_url).await, (64_u64, 64_u64));
        let quantized_url = report["quantized"]["url"]
            .as_str()
            .expect("a quantized URL");
        assert_eq!(asset_png_dimensions(quantized_url).await, (64_u64, 64_u64));
        let bytes = fetch_asset(quantized_url).await;
        let image = cyberdemon::gfx::codec::decode_image(&bytes).expect("a PNG");
        assert!(
            image.pixels.chunks_exact(4).all(is_ramp_colour),
            "every quantized pixel is a palette colour (i, 255-i, i)"
        );
        assert_eq!(
            report["colors_used"].as_u64().map(|count| count > 0_u64),
            Some(true),
            "the count answers with a number"
        );
        assert!(report["max_color_error"].is_number(), "the error answers");
        assert_eq!(
            report["transparent_pixels"],
            json!(0_u64),
            "an opaque source"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "the preview edits nothing");
    }

    /// Whether one pixel is a fully opaque fixture-ramp colour `(i, 255-i, i)`.
    fn is_ramp_colour(pixel: &[u8]) -> bool {
        let &[red, green, blue, alpha] = pixel else {
            return false;
        };
        alpha == 255_u8 && red.saturating_add(green) == 255_u8 && blue == red
    }

    /// `quantize: false` publishes only the processed picture and answers
    /// the quantization measures with `null`.
    #[tokio::test]
    async fn process_image_without_quantize_publishes_one_picture() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "PLAIN.png",
            &flat_png(100_u32, 60_u32, |_| false),
        );
        let report = call(
            &client,
            "process_image",
            source_args(&file, json!({ "quantize": false })),
        )
        .await;
        assert_eq!(report["quantized"], Value::Null, "no quantized asset");
        assert_eq!(report["colors_used"], Value::Null, "no measure");
        assert_eq!(report["max_color_error"], Value::Null, "no error");
        assert_eq!(report["width"], json!(100_u64), "the untouched size");
        assert!(
            report["steps"].as_array().expect("steps").is_empty(),
            "no ops"
        );
    }

    /// A bad op parameter fails with `invalid_argument` naming the op
    /// index, and the processed asset `URL` stays a valid source for the
    /// import tools.
    #[tokio::test]
    async fn process_image_names_the_failing_op_and_its_output_reimports() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "PLAIN.png",
            &flat_png(100_u32, 60_u32, |_| false),
        );
        let (code, message) = call_err(
            &client,
            "process_image",
            source_args(
                &file,
                json!({
                    "ops": [
                        { "op": "fit", "width": 32_u64, "height": 32_u64, "mode": "cover" },
                        { "op": "crop", "x": 0_u64, "y": 0_u64, "width": 0_u64, "height": 4_u64 },
                    ],
                }),
            ),
        )
        .await;
        assert_eq!(code, "invalid_argument", "the op refuses");
        assert!(
            message.contains("ops[1]"),
            "the message names the op index: {message}"
        );
        let preview = call(&client, "process_image", source_args(&file, Value::Null)).await;
        let url = preview["processed"]["url"].as_str().expect("a URL");
        let imported = call(
            &client,
            "import_image_as_patch",
            json!({ "source": url, "name": "FROMPREV" }),
        )
        .await;
        assert_eq!(imported["width"], json!(100_u64), "the asset URL reimports");
    }

    /// One `create_texture_from_image` of `file` with `extra`.
    async fn create_texture(client: &Client, file: &Path, extra: Value) -> Value {
        call(
            client,
            "create_texture_from_image",
            source_args(file, extra),
        )
        .await
    }

    /// The default size rule: a 64x128 picture is kept, a 100x60 one
    /// falls back to 128x128 with the documented note, and an explicit
    /// width keeps the aspect.
    #[tokio::test]
    async fn create_texture_settles_a_tiling_friendly_size() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let tall = write_file(
            dir.path(),
            "TALL.png",
            &flat_png(64_u32, 128_u32, |_| false),
        );
        let kept = create_texture(&client, &tall, json!({ "name": "KEPT" })).await;
        assert_eq!(
            (kept["width"].as_u64(), kept["height"].as_u64()),
            (Some(64_u64), Some(128_u64)),
            "the kept size"
        );
        assert_eq!(kept["notes"], json!([]), "a kept size adds no note");
        assert_eq!(kept["steps"], json!([]), "no automatic op at this size");
        let odd = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 60_u32, |_| false));
        let fallback = create_texture(&client, &odd, json!({ "name": "FALLBACK" })).await;
        assert_eq!(
            (fallback["width"].as_u64(), fallback["height"].as_u64()),
            (Some(128_u64), Some(128_u64)),
            "the square fallback"
        );
        assert_eq!(
            fallback["notes"],
            json!(["resized to 128x128; pass width/height to choose"]),
            "the documented note"
        );
        assert_eq!(
            fallback["steps"][0]["op"],
            json!("fit"),
            "the automatic fit"
        );
        let aspect = create_texture(
            &client,
            &tall,
            json!({ "name": "ASPECT", "width": 128_u64 }),
        )
        .await;
        assert_eq!(aspect["height"].as_u64(), Some(256_u64), "the aspect kept");
        assert_eq!(
            aspect["width"].as_u64(),
            Some(128_u64),
            "the explicit width"
        );
    }

    /// A non-power-of-two width refuses, naming the nearest valid widths.
    #[tokio::test]
    async fn create_texture_refuses_widths_that_cannot_tile() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "ODD.png", &flat_png(100_u32, 60_u32, |_| false));
        let (code, message) = call_err(
            &client,
            "create_texture_from_image",
            source_args(&file, json!({ "name": "NOPE", "width": 100_u64 })),
        )
        .await;
        assert_eq!(code, "invalid_argument", "100 is no tile width");
        assert!(
            message.contains("64 or 128"),
            "the message names the neighbours: {message}"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "nothing was written");
    }

    /// `tileable` appends the `make_tileable` step to the report.
    #[tokio::test]
    async fn create_texture_appends_the_tileable_op() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "TILE.png",
            &flat_png(64_u32, 128_u32, |_| false),
        );
        let report =
            create_texture(&client, &file, json!({ "name": "TILE", "tileable": true })).await;
        let steps = report["steps"].as_array().expect("a step list");
        assert_eq!(steps.len(), 1_usize, "only the tiling op ran");
        assert_eq!(steps[0]["op"], json!("make_tileable"), "the tileable step");
    }

    /// The whole write lands: the texture is listed, the patch is in
    /// `PNAMES` and decodes, and the definition preview has its size.
    #[tokio::test]
    async fn create_texture_stores_patch_pnames_and_definition_together() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "MYTEX.png",
            &flat_png(64_u32, 128_u32, |_| false),
        );
        let report = create_texture(&client, &file, json!({ "name": "MYTEX" })).await;
        assert_eq!(report["texture"], json!("MYTEX"), "the texture name");
        assert_eq!(report["patch"], json!("MYTEX"), "the patch follows");
        assert_eq!(report["lump"], json!("TEXTURE1"), "the default target");
        assert!(report["preview"]["url"].is_string(), "a preview asset");
        let listed = call(&client, "list_textures", json!({ "pattern": "MYTEX" })).await;
        assert_eq!(listed["total"], json!(1_u64), "the texture is defined");
        assert_eq!(
            listed["textures"][0]["width"],
            json!(64_u64),
            "the record size"
        );
        assert_eq!(
            listed["textures"][0]["patches"],
            json!(1_u64),
            "the single patch"
        );
        let pnames = call(&client, "list_pnames", json!({ "pattern": "MYTEX" })).await;
        assert_eq!(pnames["total"], json!(1_u64), "PNAMES lists the patch");
        assert_eq!(
            pnames["pnames"][0]["lump_found"],
            json!(true),
            "and the lump stands"
        );
        let patch = call(&client, "get_patch", json!({ "name": "MYTEX" })).await;
        assert_eq!(patch["width"], json!(64_u64), "the patch decodes");
        assert_eq!(patch["height"], json!(128_u64), "at the texture size");
        assert_eq!(patch["left_offset"], json!(0_i64), "zero offsets");
        assert_eq!(patch["top_offset"], json!(0_i64), "both ways");
        let preview = call(
            &client,
            "get_asset_url",
            json!({ "name": "MYTEX", "prefer": "texture" }),
        )
        .await;
        assert_eq!(
            preview["resolved_as"],
            json!("texture"),
            "the definition serves"
        );
        assert_eq!(
            (preview["width"].as_u64(), preview["height"].as_u64()),
            (Some(64_u64), Some(128_u64)),
            "the preview matches the record"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["create_texture MYTEX"]),
            "one undoable step"
        );
    }

    /// A patch name a lower layer holds refuses until `replace`, and a
    /// defined texture name refuses until `replace` too.
    #[tokio::test]
    async fn create_texture_collisions_refuse_until_replace() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "CLASH.png",
            &flat_png(64_u32, 128_u32, |_| false),
        );
        let (patch_code, patch_message) = call_err(
            &client,
            "create_texture_from_image",
            source_args(&file, json!({ "name": "CLASH", "patch_name": "WALLP1" })),
        )
        .await;
        assert_eq!(patch_code, "already_exists", "the IWAD patch stands");
        assert!(
            patch_message.contains("a patch named WALLP1 already exists in the IWAD"),
            "the message names the layer: {patch_message}"
        );
        assert!(
            patch_message.contains("pass patch_name"),
            "and the way out: {patch_message}"
        );
        let (texture_code, texture_message) = call_err(
            &client,
            "create_texture_from_image",
            source_args(&file, json!({ "name": "WALL1" })),
        )
        .await;
        assert_eq!(texture_code, "already_exists", "the fixture texture stands");
        assert!(
            texture_message.contains("replace=true"),
            "the message names the way out: {texture_message}"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "neither refusal wrote");
        let replaced = create_texture(
            &client,
            &file,
            json!({ "name": "WALL1", "patch_name": "WALLP1", "replace": true }),
        )
        .await;
        assert_eq!(replaced["lump"], json!("TEXTURE1"), "redefined in place");
        let patch = call(&client, "get_patch", json!({ "name": "WALLP1" })).await;
        assert_eq!(patch["width"], json!(64_u64), "the override serves");
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames["total"],
            json!(FIXTURE_PNAMES),
            "WALLP1 kept its entry"
        );
    }

    /// `assign_role` points the theme at the new texture, and the theme
    /// note says undo will not roll it back.
    #[tokio::test]
    async fn assign_role_points_the_theme_at_the_new_texture() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "ROLE.png",
            &flat_png(64_u32, 128_u32, |_| false),
        );
        let report = create_texture(
            &client,
            &file,
            json!({ "name": "ROLE", "assign_role": "wall" }),
        )
        .await;
        assert_eq!(report["role"], json!("wall"), "the assigned role");
        assert!(
            report["notes"]
                .as_array()
                .expect("notes")
                .iter()
                .any(|note| note
                    .as_str()
                    .is_some_and(|text| text.contains("theme override wall -> ROLE")
                        && text.contains("undo does not roll it back"))),
            "the note explains the theme state: {}",
            report["notes"]
        );
        let theme = call(&client, "get_texture_theme", json!({})).await;
        assert_eq!(
            theme["roles"]["wall"]["texture"],
            json!("ROLE"),
            "the role resolves"
        );
        assert_eq!(
            theme["roles"]["wall"]["matched"],
            json!("exact"),
            "as an override"
        );
    }

    /// One `create_texture_from_image` call asking for role `role`.
    async fn refuse_role(client: &Client, file: &Path, role: &str) {
        let (code, message) = call_err(
            client,
            "create_texture_from_image",
            source_args(file, json!({ "name": "NOPE", "assign_role": role })),
        )
        .await;
        assert_eq!(code, "invalid_argument", "{role} refuses");
        assert!(
            message.contains("flat") || message.contains("role"),
            "the message explains {role}: {message}"
        );
    }

    /// Both the flat role and an unknown one refuse.
    async fn refuse_roles(client: &Client, file: &Path) {
        for role in ["floor", "roof"] {
            refuse_role(client, file, role).await;
        }
    }

    /// A flat role (and an unknown one) refuse before anything is
    /// written; an empty undo stack proves it.
    #[tokio::test]
    async fn flat_roles_refuse_before_anything_is_written() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "FLATROLE.png",
            &flat_png(64_u32, 128_u32, |_| false),
        );
        refuse_roles(&client, &file).await;
        let listed = call(&client, "list_textures", json!({ "pattern": "NOPE" })).await;
        assert_eq!(listed["total"], json!(0_u64), "nothing was defined");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "no edit landed");
        let theme = call(&client, "get_texture_theme", json!({})).await;
        assert_ne!(
            theme["roles"]["floor"]["texture"],
            json!("NOPE"),
            "the theme is untouched"
        );
    }

    /// One `undo` removes the patch, the `PNAMES` entry and the texture
    /// definition together.
    #[tokio::test]
    async fn one_undo_rolls_back_the_whole_created_texture() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "UNDONE.png",
            &flat_png(64_u32, 128_u32, |_| false),
        );
        create_texture(&client, &file, json!({ "name": "UNDONE" })).await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["create_texture UNDONE"]),
            "the single step rolls back"
        );
        let textures = call(&client, "list_textures", json!({ "pattern": "UNDONE" })).await;
        assert_eq!(textures["total"], json!(0_u64), "the definition left");
        let pnames = call(&client, "list_pnames", json!({})).await;
        assert_eq!(
            pnames["total"],
            json!(FIXTURE_PNAMES),
            "the PNAMES entry left"
        );
        let patch = call_err(&client, "get_patch", json!({ "name": "UNDONE" })).await;
        assert_eq!(patch.0, "not_found", "the patch lump left with them");
    }

    /// A 64x128 palette PNG with every other pixel fully transparent.
    fn holes_png() -> Vec<u8> {
        let even = |p: usize| p.checked_rem(2_usize).is_some_and(|rest| rest == 0_usize);
        flat_png(64_u32, 128_u32, even)
    }

    /// A transparent patch side is filled for the wall and warned about,
    /// and `ops` run before the size rule.
    #[tokio::test]
    async fn create_texture_fills_transparent_pixels_and_runs_user_ops() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "HOLES.png", &holes_png());
        let report = create_texture(
            &client,
            &file,
            json!({
                "name": "HOLES",
                "ops": [{ "op": "flip", "horizontal": true, "vertical": false }],
            }),
        )
        .await;
        let steps = report["steps"].as_array().expect("a step list");
        assert_eq!(steps[0]["op"], json!("flip"), "the user op ran first");
        let warnings = report["warnings"].as_array().expect("a warning list");
        assert_eq!(warnings.len(), 1_usize, "one fill warning");
        assert!(
            warnings[0]
                .as_str()
                .is_some_and(|text| text.contains("palette colour nearest to black")),
            "the warning explains the fill: {warnings:?}"
        );
        let patch = call(
            &client,
            "get_patch",
            json!({ "name": "HOLES", "format": "rgba" }),
        )
        .await;
        let url = patch["asset"]["url"].as_str().expect("an asset");
        let rgba = fetch_asset(url).await;
        assert!(
            rgba.chunks_exact(4).all(|pixel| pixel[3] == 255_u8),
            "the wall patch is fully opaque"
        );
    }

    /// One successful `create_flat_from_image` call of `file` with `extra`.
    async fn create_flat(client: &Client, file: &Path, extra: Value) -> Value {
        call(client, "create_flat_from_image", source_args(file, extra)).await
    }

    /// The `steps` of a report, as op names in order.
    fn step_ops(report: &Value) -> Vec<&str> {
        report["steps"]
            .as_array()
            .expect("a step list")
            .iter()
            .map(|step| step["op"].as_str().expect("a named step"))
            .collect()
    }

    /// Whether a `warnings` entry is text mentioning every one of `parts`.
    fn mentions(warning: &Value, parts: &[&str]) -> bool {
        let text = warning.as_str().unwrap_or_default();
        parts.iter().all(|part| text.contains(part))
    }

    /// One failing `create_flat_from_image` call of `file` with `extra`.
    async fn create_flat_err(client: &Client, file: &Path, extra: Value) -> (String, String) {
        call_err(client, "create_flat_from_image", source_args(file, extra)).await
    }

    /// The `RGBA` pixel at row-major `index` of a fetched picture.
    fn rgba_pixel(rgba: &[u8], index: usize) -> [u8; 4] {
        let start = index.checked_mul(4).expect("byte offset");
        let byte = |shift: usize| {
            *rgba
                .get(start.checked_add(shift).expect("in range"))
                .expect("pixel byte")
        };
        [byte(0), byte(1), byte(2), byte(3)]
    }

    /// The `RGB` triple at row-major `index` of a fetched picture.
    fn rgb_at(rgba: &[u8], index: usize) -> [u8; 3] {
        let pixel = rgba_pixel(rgba, index);
        [pixel[0], pixel[1], pixel[2]]
    }

    /// The `create_flat_from_image` rule: any picture becomes a `64x64`
    /// flat with the automatic `fit` (+ default `make_tileable`) steps, a
    /// preview, a stored lump and one undoable edit.
    #[tokio::test]
    async fn create_flat_fits_any_picture_to_the_vanilla_square() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "PHOTO.png",
            &flat_png(100_u32, 80_u32, |_| false),
        );
        let report = create_flat(&client, &file, json!({ "name": "myfloor" })).await;
        assert_eq!(report["flat"], json!("MYFLOOR"), "the name uppercases");
        assert_eq!(
            step_ops(&report),
            vec!["fit", "make_tileable"],
            "the fit always runs and tiling is on by default"
        );
        assert_eq!(report["overrides_existing"], json!(false), "a free name");
        assert!(report["preview"]["url"].is_string(), "a preview asset");
        assert_eq!(
            report["next_steps"][0],
            json!("edit_sector floor_flat/ceiling_flat"),
            "the documented way forward"
        );
        assert_eq!(report["warnings"], json!([]), "the picture is opaque");
        assert!(
            report["max_color_error"].as_u64().is_some(),
            "the quantization is measured"
        );
        let flat = call(&client, "get_flat", json!({ "name": "MYFLOOR" })).await;
        assert_eq!(flat["width"], json!(64_u64), "the vanilla square");
        assert_eq!(flat["height"], json!(64_u64), "both ways");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["create_flat MYFLOOR"]),
            "one undoable step"
        );
    }

    /// `tileable` (on by default) blends the edges; off keeps them.
    #[tokio::test]
    async fn the_tileable_switch_blends_the_flat_edges() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "RAMP.png",
            &flat_png(100_u32, 80_u32, |_| false),
        );
        let tiled = create_flat(&client, &file, json!({ "name": "TILED" })).await;
        let asdrawn = create_flat(
            &client,
            &file,
            json!({ "name": "ASDRAWN", "tileable": false }),
        )
        .await;
        assert_eq!(step_ops(&asdrawn), vec!["fit"], "no tiling step off");
        assert_eq!(
            step_ops(&tiled),
            vec!["fit", "make_tileable"],
            "on by default"
        );
        let blended = flat_rgba(&client, "TILED").await;
        let kept = flat_rgba(&client, "ASDRAWN").await;
        assert_eq!(blended.len(), kept.len(), "both are 64x64 RGBA");
        assert_ne!(blended, kept, "the 16-pixel band changed the seams");
    }

    /// `contain` letterboxes the whole picture: the padding quantizes to
    /// the palette colour nearest to black and counts as kept-opaque.
    #[tokio::test]
    async fn contain_fits_the_whole_picture_with_padding() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "WIDE.png", &flat_png(64_u32, 40_u32, |_| false));
        let report = create_flat(
            &client,
            &file,
            json!({ "name": "PADDED", "fit": "contain", "tileable": false, "dither": "none" }),
        )
        .await;
        let warnings = report["warnings"].as_array().expect("a warning list");
        assert_eq!(warnings.len(), 1_usize, "the padding warns");
        assert!(
            warnings[0]
                .as_str()
                .is_some_and(|text| text.contains("kept opaque") && text.contains("1536 pixels")),
            "the warning counts the padding: {warnings:?}"
        );
        let rgba = flat_rgba(&client, "PADDED").await;
        assert_eq!(
            rgb_at(&rgba, 0_usize),
            [85_u8, 170, 85],
            "the padding took the palette colour nearest to black"
        );
        let row_12 = 12_usize.checked_mul(64).expect("row 12");
        assert_eq!(
            rgb_at(&rgba, row_12),
            [0_u8, 255, 0],
            "row 12 is source row 0"
        );
    }

    /// A layer-held flat name refuses with the everywhere-used hint;
    /// `replace` accepts the override; a working twin then needs
    /// `replace` too.
    #[tokio::test]
    async fn create_flat_collisions_refuse_until_replace() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "CLASH.png",
            &flat_png(32_u32, 32_u32, |_| false),
        );
        let (code, message) = call_err(
            &client,
            "create_flat_from_image",
            source_args(&file, json!({ "name": "FLOOR1" })),
        )
        .await;
        assert_eq!(code, "already_exists", "the IWAD flat stands");
        assert!(
            message.contains("this would override FLOOR1 everywhere it is used"),
            "the hint names the consequence: {message}"
        );
        assert!(
            message.contains("the IWAD"),
            "and names the layer: {message}"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "the refusal wrote nothing");
        let replaced =
            create_flat(&client, &file, json!({ "name": "FLOOR1", "replace": true })).await;
        assert_eq!(
            replaced["overrides_existing"],
            json!(true),
            "the override reports"
        );
        assert_eq!(
            step_ops(&replaced),
            vec!["fit", "make_tileable"],
            "the normal chain"
        );
        let listed = call(&client, "list_flats", json!({ "pattern": "FLOOR1" })).await;
        assert_eq!(
            listed["flats"][0]["source"],
            json!("working"),
            "the working copy now serves"
        );
        let (twin_code, twin_message) = call_err(
            &client,
            "create_flat_from_image",
            source_args(&file, json!({ "name": "FLOOR1" })),
        )
        .await;
        assert_eq!(twin_code, "already_exists", "the working twin stands");
        assert!(
            twin_message.contains("working archive"),
            "the message names the archive: {twin_message}"
        );
    }

    /// `assign_role` points a theme flat role at the new flat and warns
    /// that undo will not roll the override back.
    #[tokio::test]
    async fn create_flat_assign_role_points_the_theme_at_the_new_flat() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "ROLED.png",
            &flat_png(64_u32, 64_u32, |_| false),
        );
        let report = create_flat(
            &client,
            &file,
            json!({ "name": "ROLED", "assign_role": "floor" }),
        )
        .await;
        assert_eq!(report["role"], json!("floor"), "the assigned role");
        assert!(
            report["warnings"]
                .as_array()
                .expect("warnings")
                .iter()
                .any(|warning| mentions(
                    warning,
                    &[
                        "theme override floor -> ROLED",
                        "undo does not roll it back"
                    ]
                )),
            "the warning explains the theme state: {}",
            report["warnings"]
        );
        let theme = call(&client, "get_texture_theme", json!({})).await;
        assert_eq!(
            theme["roles"]["floor"]["texture"],
            json!("ROLED"),
            "the role resolves"
        );
        assert_eq!(
            theme["roles"]["floor"]["matched"],
            json!("exact"),
            "as an override"
        );
    }

    /// One `create_flat_from_image` call asking for role `role`, refused.
    async fn refuse_flat_role(client: &Client, file: &Path, role: &str) {
        let (code, message) =
            create_flat_err(client, file, json!({ "name": "NOPE", "assign_role": role })).await;
        assert_eq!(code, "invalid_argument", "{role} refuses");
        assert!(
            message.contains("role"),
            "the message explains {role}: {message}"
        );
    }

    /// Both a wall role and an unknown one refuse.
    async fn refuse_flat_roles(client: &Client, file: &Path) {
        for role in ["wall", "roof"] {
            refuse_flat_role(client, file, role).await;
        }
    }

    /// A wall role (and an unknown one) refuse before anything is written.
    #[tokio::test]
    async fn create_flat_wall_roles_refuse_before_anything_is_written() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "WALLROLE.png",
            &flat_png(64_u32, 64_u32, |_| false),
        );
        refuse_flat_roles(&client, &file).await;
        let listed = call(&client, "list_flats", json!({ "pattern": "NOPE" })).await;
        assert_eq!(listed["total"], json!(0_u64), "nothing was stored");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "no edit landed");
        let theme = call(&client, "get_texture_theme", json!({})).await;
        assert_ne!(
            theme["roles"]["floor"]["texture"],
            json!("NOPE"),
            "the theme is untouched"
        );
    }

    /// One `undo` removes the created flat.
    #[tokio::test]
    async fn one_undo_rolls_back_the_created_flat() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "UNDONE.png",
            &flat_png(100_u32, 80_u32, |_| false),
        );
        create_flat(&client, &file, json!({ "name": "UNDONE" })).await;
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["create_flat UNDONE"]),
            "the single step rolls back"
        );
        let (code, _) = call_err(&client, "get_flat", json!({ "name": "UNDONE" })).await;
        assert_eq!(code, "not_found", "the flat left with its edit");
    }

    /// Bad sources and parameters each report their code; nothing is
    /// stored along the way.
    #[tokio::test]
    async fn bad_flat_creation_arguments_are_refused_without_edits() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let junk = write_file(dir.path(), "junk.txt", b"this is a text file, not an image");
        let good = write_file(dir.path(), "GOOD.png", &flat_png(64_u32, 64_u32, |_| false));
        let cases: [Refusal; 4] = [
            (
                "junk",
                source_args(&junk, json!({ "name": "X" })),
                "format_error",
                "image",
            ),
            (
                "long name",
                source_args(&good, json!({ "name": "TOO LONG NAME" })),
                "invalid_argument",
                "name",
            ),
            (
                "wall role",
                source_args(&good, json!({ "name": "X", "assign_role": "door" })),
                "invalid_argument",
                "wall role",
            ),
            (
                "missing",
                json!({ "source": dir.path().join("absent.png").display().to_string(), "name": "X" }),
                "not_found",
                "absent.png",
            ),
        ];
        refuse_all(&client, "create_flat_from_image", &cases).await;
        let flats = call(&client, "list_flats", json!({})).await;
        assert_eq!(flats["total"], json!(FIXTURE_FLATS), "nothing was stored");
    }

    /// The `40x60` figure PNG's pixel at `(x, y)`: red inside the `10x20`
    /// block at `(10, 20)`, white outside it.
    fn figure_pixel(x: u32, y: u32) -> [u8; 4] {
        if (10_u32..20_u32).contains(&x) && (20_u32..40_u32).contains(&y) {
            [255_u8, 0, 0, 255]
        } else {
            [255_u8, 255, 255, 255]
        }
    }

    /// A `40x60` palette PNG: white all over, a solid red `10x20` block.
    fn figure_png() -> Vec<u8> {
        let pixels: Vec<u8> = (0_u32..60_u32)
            .flat_map(|y| (0_u32..40_u32).map(move |x| figure_pixel(x, y)))
            .flatten()
            .collect();
        let img = RgbaImage::new(40_u32, 60_u32, pixels).expect("40x60 is valid");
        encode_png(&img, None).expect("test PNG encodes")
    }

    /// A `12x12` fully white PNG.
    fn white_png() -> Vec<u8> {
        let side = 12_usize;
        let count = side
            .checked_mul(side)
            .expect("test pixels")
            .checked_mul(4_usize)
            .expect("test bytes");
        let img = RgbaImage::new(12_u32, 12_u32, vec![255_u8; count]).expect("12x12 is valid");
        encode_png(&img, None).expect("test PNG encodes")
    }

    /// One successful `create_sprite_from_image` call of `file`.
    async fn create_sprite(client: &Client, file: &Path, extra: Value) -> Value {
        call(client, "create_sprite_from_image", source_args(file, extra)).await
    }

    /// The pixel index just inside the figure block at row 20, column 12.
    fn figure_index() -> usize {
        usize::try_from(
            20_u32
                .checked_mul(40_u32)
                .expect("row 20")
                .checked_add(12_u32)
                .expect("pixel 12"),
        )
        .expect("in range")
    }

    /// Every one of `names` answers `not_found` from `get_sprite`.
    async fn sprites_are_gone(client: &Client, names: &[&str]) {
        for name in names {
            let (code, _) = call_err(client, "get_sprite", json!({ "name": name })).await;
            assert_eq!(code, "not_found", "{name} left with the edit");
        }
    }

    /// The default pipeline: background removal, trim, feet-on-origin
    /// offsets, as one labelled edit with the definition hint.
    #[tokio::test]
    async fn create_sprite_removes_the_background_and_stands_on_its_feet() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "FIGURE.png", &figure_png());
        let report = create_sprite(&client, &file, json!({ "sprite": "BOBA" })).await;
        assert_eq!(
            report["lumps"],
            json!(["BOBAA0"]),
            "the parts form names one lump"
        );
        assert_eq!(
            (report["width"].as_u64(), report["height"].as_u64()),
            (Some(10_u64), Some(20_u64)),
            "trimmed to the figure"
        );
        assert_eq!(report["left_offset"], json!(5_i64), "half the final width");
        assert_eq!(report["top_offset"], json!(20_i64), "feet on the origin");
        assert_eq!(
            step_ops(&report),
            vec!["remove_background", "trim"],
            "the automatic chain"
        );
        assert_eq!(report["notes"], json!([]), "nothing scaled");
        assert_eq!(report["warnings"], json!([]), "a free sprite");
        assert!(report["preview"]["url"].is_string(), "a preview asset");
        assert_eq!(
            report["next_steps"][0],
            json!(
                "a new thing type needs a definition: edit_decorate (ZDoom ports) or edit_dehacked to repoint an existing type's frames"
            ),
            "the definition hint"
        );
        let sprite = call(&client, "get_sprite", json!({ "name": "BOBAA0" })).await;
        assert_eq!(sprite["width"], json!(10_u64), "the stored lump");
        assert_eq!(sprite["top_offset"], json!(20_i64), "keeps the offsets");
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["create_sprite BOBAA0"]),
            "one undoable step"
        );
    }

    /// `remove_background: false` keeps the white canvas opaque;
    /// `trim: false` keeps the cleared border as holes, so the area
    /// outside the figure is transparent.
    #[tokio::test]
    async fn the_background_switches_keep_the_canvas_or_its_holes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "FIGURE.png", &figure_png());
        let kept = create_sprite(
            &client,
            &file,
            json!({ "name": "BOBAA1", "remove_background": false }),
        )
        .await;
        assert_eq!(
            (kept["width"].as_u64(), kept["height"].as_u64()),
            (Some(40_u64), Some(60_u64)),
            "the white is content, so the trim keeps the full canvas"
        );
        assert_eq!(kept["top_offset"], json!(60_i64), "feet on the full height");
        let kept_rgba = sprite_rgba(&client, "BOBAA1").await;
        assert_eq!(
            rgba_pixel(&kept_rgba, 0_usize)[3],
            255_u8,
            "the corner stays opaque"
        );
        let holed = create_sprite(&client, &file, json!({ "name": "BOBAA2", "trim": false })).await;
        assert_eq!(
            holed["transparent_pixels"],
            json!(2200_u64),
            "the white was dropped as holes"
        );
        let holed_rgba = sprite_rgba(&client, "BOBAA2").await;
        assert_eq!(
            rgba_pixel(&holed_rgba, 0_usize)[3],
            0_u8,
            "outside the figure is transparent"
        );
        assert_eq!(
            rgba_pixel(&holed_rgba, figure_index())[3],
            255_u8,
            "the figure is opaque"
        );
    }

    /// `height` scales the trimmed figure with the aspect kept, and the
    /// offsets follow the final picture.
    #[tokio::test]
    async fn create_sprite_scales_to_the_requested_height() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "FIGURE.png", &figure_png());
        let report = create_sprite(
            &client,
            &file,
            json!({ "sprite": "BOBA", "rotation": 1_u64, "height": 40_u64 }),
        )
        .await;
        assert_eq!(report["lumps"], json!(["BOBAA1"]), "the rotation carries");
        assert_eq!(
            (report["width"].as_u64(), report["height"].as_u64()),
            (Some(20_u64), Some(40_u64)),
            "10x20 scaled to 40 tall keeps 20 wide"
        );
        assert_eq!(report["left_offset"], json!(10_i64), "half the final width");
        assert_eq!(report["top_offset"], json!(40_i64), "the final height");
        assert_eq!(
            step_ops(&report),
            vec!["remove_background", "trim", "fit"],
            "the fit follows the trim"
        );
        assert_eq!(report["notes"], json!([]), "an explicit height is quiet");
    }

    /// `frames` writes the same final image once per letter, in one edit
    /// that one undo rolls back entirely.
    #[tokio::test]
    async fn create_sprite_frames_writes_one_lump_per_letter_at_once() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "FIGURE.png", &figure_png());
        let report = create_sprite(
            &client,
            &file,
            json!({ "sprite": "BOBA", "frames": ["A", "B"] }),
        )
        .await;
        assert_eq!(
            report["lumps"],
            json!(["BOBAA0", "BOBAB0"]),
            "both frames named"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["create_sprite BOBAA0"]),
            "the first lump labels the single step"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["create_sprite BOBAA0"]),
            "one undo rolls the pair back"
        );
        sprites_are_gone(&client, &["BOBAA0", "BOBAB0"]).await;
    }

    /// The parts form with `mirror_rotation` names the pair lump.
    #[tokio::test]
    async fn create_sprite_names_the_mirror_pair() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "FIGURE.png", &figure_png());
        let report = create_sprite(
            &client,
            &file,
            json!({ "sprite": "BOBA", "frame": "B", "rotation": 2_u64, "mirror_rotation": 7_u64 }),
        )
        .await;
        assert_eq!(
            report["lumps"],
            json!(["BOBAB2B7"]),
            "the assembled pair name"
        );
        let sprite = call(&client, "get_sprite", json!({ "name": "BOBAB2B7" })).await;
        assert_eq!(sprite["width"], json!(10_u64), "the lump serves");
    }

    /// Rotations another lump already serves are warnings, not refusals.
    #[tokio::test]
    async fn create_sprite_warns_about_rotations_the_session_already_serves() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "FIGURE.png", &figure_png());
        let report = create_sprite(
            &client,
            &file,
            json!({ "sprite": "TROO", "rotation": 2_u64 }),
        )
        .await;
        assert_eq!(report["lumps"], json!(["TROOA2"]), "the import lands");
        let warnings = report["warnings"].as_array().expect("a warning list");
        assert!(
            warnings.iter().any(|warning| mentions(
                warning,
                &["frame A rotation 2 is also served by TROOA2A8"]
            )),
            "the pair's coverage warns: {warnings:?}"
        );
    }

    /// A picture that is all background refuses with the documented hint.
    #[tokio::test]
    async fn a_picture_that_is_all_background_refuses_with_the_hint() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "WHITE.png", &white_png());
        let (code, message) = call_err(
            &client,
            "create_sprite_from_image",
            source_args(&file, json!({ "sprite": "BOBA" })),
        )
        .await;
        assert_eq!(code, "invalid_argument", "nothing is left to store");
        assert!(
            message.contains("the whole image was removed as background"),
            "the message names the cause: {message}"
        );
        assert!(
            message.contains("lower background_tolerance")
                && message.contains("set remove_background false"),
            "both ways out are named: {message}"
        );
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "nothing was written");
    }

    /// A taller-than-`256` picture without `height` is scaled to `128`
    /// tall and says so in the notes.
    #[tokio::test]
    async fn a_tall_picture_is_scaled_with_the_documented_note() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(
            dir.path(),
            "TALL.png",
            &flat_png(60_u32, 300_u32, |_| false),
        );
        let report = create_sprite(
            &client,
            &file,
            json!({ "name": "BOBAA3", "remove_background": false, "trim": false }),
        )
        .await;
        assert_eq!(
            (report["width"].as_u64(), report["height"].as_u64()),
            (Some(26_u64), Some(128_u64)),
            "60x300 to 128 tall keeps the aspect"
        );
        assert_eq!(
            report["notes"],
            json!(["scaled to 128 px tall; pass height to choose"]),
            "the documented note"
        );
        assert_eq!(step_ops(&report), vec!["fit"], "the automatic fit");
    }

    /// A working twin refuses until `replace`, which overwrites in place;
    /// the refusal adds no undo step.
    #[tokio::test]
    async fn create_sprite_duplicates_need_replace() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "FIGURE.png", &figure_png());
        create_sprite(&client, &file, json!({ "sprite": "BOBA" })).await;
        let (code, message) = call_err(
            &client,
            "create_sprite_from_image",
            source_args(&file, json!({ "sprite": "BOBA" })),
        )
        .await;
        assert_eq!(code, "already_exists", "the working twin stands");
        assert!(
            message.contains("replace=true"),
            "the message names the way out: {message}"
        );
        let replaced =
            create_sprite(&client, &file, json!({ "sprite": "BOBA", "replace": true })).await;
        assert_eq!(replaced["lumps"], json!(["BOBAA0"]), "the overwrite lands");
        let session = call(&client, "get_session_info", json!({})).await;
        let undo = session["undo"].as_array().expect("the undo stack");
        assert_eq!(undo.len(), 2_usize, "the refusal added no step: {undo:?}");
        assert_eq!(
            undo[0],
            json!("create_sprite BOBAA0"),
            "both steps are the create"
        );
    }

    /// Naming conflicts, a bad height and a bad colour each report
    /// `invalid_argument`; nothing is stored along the way.
    #[tokio::test]
    async fn bad_sprite_creation_arguments_are_refused_without_edits() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let file = write_file(dir.path(), "FIGURE.png", &figure_png());
        let cases: [Refusal; 7] = [
            (
                "both forms",
                source_args(&file, json!({ "name": "TROOA1", "sprite": "TROO" })),
                "invalid_argument",
                "not both",
            ),
            (
                "frame and frames",
                source_args(
                    &file,
                    json!({ "sprite": "TROO", "frame": "A", "frames": ["B"] }),
                ),
                "invalid_argument",
                "frame and frames",
            ),
            (
                "name and frames",
                source_args(&file, json!({ "name": "TROOA1", "frames": ["B"] })),
                "invalid_argument",
                "invalid together with name",
            ),
            (
                "no selector",
                source_args(&file, Value::Null),
                "invalid_argument",
                "needs a sprite",
            ),
            (
                "empty frames",
                source_args(&file, json!({ "sprite": "TROO", "frames": [] })),
                "invalid_argument",
                "frames is empty",
            ),
            (
                "height",
                source_args(&file, json!({ "sprite": "BOBA", "height": 0_u64 })),
                "invalid_argument",
                "1..=512",
            ),
            (
                "background colour",
                source_args(
                    &file,
                    json!({ "sprite": "BOBA", "background_color": "green" }),
                ),
                "invalid_argument",
                "background_color",
            ),
        ];
        refuse_all(&client, "create_sprite_from_image", &cases).await;
        let session = call(&client, "get_session_info", json!({})).await;
        assert_eq!(session["undo"], json!([]), "nothing was stored");
    }
}
