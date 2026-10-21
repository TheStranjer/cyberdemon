//! The first steps of the `images-to-assets` guide as one real client
//! chain. `open_iwad` gives the session the `PLAYPAL` every quantizing
//! import needs; a harness-drawn 300x200 "brick photo" (rows of reddish
//! rectangles over grey mortar inside a 20-px pure-blue border) runs
//! through `process_image` with a `crop` to the inner area and
//! `auto_levels`, and neither published preview keeps a blue-dominant
//! pixel; the same file then goes through `create_texture_from_image`
//! with that crop, `tileable: true`, a `128x128` size and
//! `assign_role: "wall"`, landing the `MYBRICK` patch, its `PNAMES`
//! entry and its `TEXTURE1` definition, which `list_textures` answers at
//! `128x128` and whose published preview has seam-smooth left and right
//! columns (integer-summed mean per-channel difference within 24).
//!
//! The remaining steps chain on in one further session. A `set_palette`
//! warm ramp `(i, 255-i, 255-i)` first replaces the fixture `PLAYPAL`,
//! whose entries tie red to blue so no rendered pixel through it can be
//! red-dominant at all; `MYBRICK` is re-created as that session's wall
//! theme and a deterministic grey-noise `MYFLOOR` becomes the floor theme
//! through `create_flat_from_image`'s `assign_role`. `auto_texture` in
//! `retexture` mode then dresses `MAP01`'s light-160 hall: every side of
//! sector 0 wears `MYBRICK` and the sector's floor reads `MYFLOOR`, and a
//! `render_view` from the player start (coordinates read back through
//! `find_elements`, never hard-coded) shows red-dominant pixels on its
//! middle row. A green 20x40 figure lands as the all-angles `TREEA0`
//! sprite, trimmed with its feet on the origin, and three `undo` calls
//! take the sprite, the retexture and the flat back in order while
//! `MYBRICK` - created before that window - still serves `list_textures`.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::image::RgbaImage;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad;
    use crate::support::{asset_png, call, call_err, connect, fill_rect, paper_sheet, write_png};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// Pixels across the brick photo.
    const SHEET_WIDTH: u32 = 300_u32;
    /// Pixels down the brick photo.
    const SHEET_HEIGHT: u32 = 200_u32;
    /// The pure-blue strip on every sheet edge.
    const BORDER: u32 = 20_u32;
    /// Brick rows stacked inside the border.
    const ROWS: u32 = 6_u32;
    /// The first row's top edge and the step between row top edges.
    const ROW_TOP: u32 = 20_u32;
    /// Brick height in pixels.
    const BRICK_HEIGHT: u32 = 20_u32;
    /// The mortar gap between stacked brick rows.
    const ROW_GAP: u32 = 5_u32;
    /// The grey mortar under the bricks.
    const MORTAR: [u8; 4] = [110, 110, 110, 255];
    /// The reddish brick colour; equal green and blue keeps every fixture
    /// colour free of a dominant blue channel through any shared curve.
    const BRICK: [u8; 4] = [185, 77, 77, 255];
    /// The pure blue of the border.
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    /// The `make_tileable`-blended seam may drift at most this much per
    /// channel, averaged over the column (integer sums).
    const MAX_SEAM_DRIFT: u64 = 24_u64;

    /// The half-open brick spans of the even rows, in sheet pixels.
    const ROW_EVEN: [(u32, u32); 5] = [(20, 76), (84, 140), (148, 204), (212, 268), (276, 280)];

    /// The half-open brick spans of the odd rows, offset by half a pitch.
    const ROW_ODD: [(u32, u32); 5] = [(20, 44), (52, 108), (116, 172), (180, 236), (244, 280)];

    /// The `crop` removing the blue border: the inner area of the sheet.
    fn inner_crop() -> Value {
        let inset = BORDER.checked_mul(2).expect("test border fits");
        json!({
            "op": "crop",
            "x": BORDER,
            "y": BORDER,
            "width": SHEET_WIDTH.checked_sub(inset).expect("test sheet is wide"),
            "height": SHEET_HEIGHT.checked_sub(inset).expect("test sheet is tall"),
        })
    }

    /// Stacks the brick rows over the mortar inside the border.
    fn paint_brick_rows(sheet: &mut RgbaImage) {
        for row in 0..ROWS {
            paint_brick_row(sheet, row);
        }
    }

    /// Paints one brick row, even or odd layout, at its stepped `y` band.
    fn paint_brick_row(sheet: &mut RgbaImage, row: u32) {
        let step = row.checked_mul(ROW_GAP).expect("test rows fit");
        let y0 = ROW_TOP.checked_add(step).expect("test rows fit");
        let y1 = y0.checked_add(BRICK_HEIGHT).expect("test rows fit");
        let spans: &[(u32, u32)] = if row & 1 == 0 { &ROW_EVEN } else { &ROW_ODD };
        for &(x0, x1) in spans {
            fill_rect(sheet, (x0, y0, x1, y1), BRICK);
        }
    }

    /// Paints the pure-blue strip around the whole sheet.
    fn paint_blue_border(sheet: &mut RgbaImage) {
        let right = SHEET_WIDTH.checked_sub(BORDER).expect("test border fits");
        let bottom = SHEET_HEIGHT.checked_sub(BORDER).expect("test border fits");
        fill_rect(sheet, (0, 0, SHEET_WIDTH, BORDER), BLUE);
        fill_rect(sheet, (0, bottom, SHEET_WIDTH, SHEET_HEIGHT), BLUE);
        fill_rect(sheet, (0, 0, BORDER, SHEET_HEIGHT), BLUE);
        fill_rect(sheet, (right, 0, SHEET_WIDTH, SHEET_HEIGHT), BLUE);
    }

    /// The 300x200 "brick photo" the chain starts from.
    fn brick_photo() -> RgbaImage {
        let mut sheet = paper_sheet(SHEET_WIDTH, SHEET_HEIGHT);
        fill_rect(&mut sheet, (0, 0, SHEET_WIDTH, SHEET_HEIGHT), MORTAR);
        paint_brick_rows(&mut sheet);
        paint_blue_border(&mut sheet);
        sheet
    }

    /// Load the fixture `IWAD` as the base: the `PLAYPAL` imports
    /// quantize against.
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

    /// Whether one `RGBA8` chunk is a pixel whose blue channel strictly
    /// dominates both others: "bluish", compared without float math.
    fn blue_dominant(pixel: &[u8]) -> bool {
        let &[red, green, blue, _] = pixel else {
            return false;
        };
        blue > red && blue > green
    }

    /// Assert no pixel of `image` is blue-dominant.
    fn assert_no_blue(image: &RgbaImage, label: &str) {
        for chunk in image.pixels.chunks_exact(4) {
            assert!(
                !blue_dominant(chunk),
                "{label} keeps a blue-dominant pixel: {chunk:?}"
            );
        }
    }

    /// Adds the absolute per-channel difference of one pixel pair onto
    /// the running `sums`.
    fn add_drift(sums: &mut [u64; 3], one: [u8; 4], two: [u8; 4]) {
        sums.iter_mut()
            .zip(one.into_iter().zip(two))
            .for_each(|(sum, (first, second))| {
                let drift = first.max(second).saturating_sub(first.min(second));
                *sum = sum.saturating_add(u64::from(drift));
            });
    }

    /// The mean absolute per-channel difference between `image`'s left
    /// and right columns, built from integer sums only.
    fn seam_drift(image: &RgbaImage) -> [u64; 3] {
        let last = image.width.checked_sub(1).expect("a test-wide image");
        let mut sums = [0_u64; 3];
        for row in 0..image.height {
            let left = image.get(0, row).expect("the left column");
            let right = image.get(last, row).expect("the right column");
            add_drift(&mut sums, left, right);
        }
        let rows = u64::from(image.height);
        sums.map(|sum| sum.checked_div(rows).expect("rows were counted"))
    }

    /// The ops of the `process_image` step: crop to the inner area, then
    /// `auto_levels` (guide section 3's clean, then tone).
    fn preview_ops() -> Vec<Value> {
        vec![inner_crop(), json!({ "op": "auto_levels" })]
    }

    /// Step 2: `process_image` over the brick photo reports the cropped
    /// shape and the two ops, and neither published preview keeps a
    /// blue-dominant pixel - the border never reaches the quantized
    /// picture the game would show.
    #[tokio::test]
    async fn process_image_crops_the_blue_border_before_quantizing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let source = write_png(dir.path(), "BRICKS.png", &brick_photo());
        let report = call(
            &client,
            "process_image",
            json!({ "source": source, "ops": preview_ops() }),
        )
        .await;
        assert_eq!(report["width"], json!(260_u64), "the cropped width");
        assert_eq!(report["height"], json!(160_u64), "the cropped height");
        let steps = report["steps"].as_array().expect("a step list");
        assert_eq!(steps.len(), 2_usize, "crop then auto_levels");
        assert_eq!(steps[0]["op"], json!("crop"), "in order");
        assert_eq!(steps[1]["op"], json!("auto_levels"), "in order");
        let processed_url = report["processed"]["url"]
            .as_str()
            .expect("a processed asset");
        assert_no_blue(&asset_png(processed_url).await, "the processed preview");
        let quantized_url = report["quantized"]["url"]
            .as_str()
            .expect("a quantized asset");
        assert_no_blue(&asset_png(quantized_url).await, "the quantized preview");
        assert_eq!(
            report["transparent_pixels"],
            json!(0_u64),
            "an opaque photo loses nothing"
        );
    }

    /// Step 3: `create_texture_from_image` over the same file with the
    /// same crop, `tileable: true`, `128x128` and `assign_role: "wall"`
    /// lands patch, `PNAMES` entry and `TEXTURE1` definition as one
    /// report; `list_textures` answers the definition at `128x128`; and
    /// the published preview's left and right columns are seam-smooth,
    /// the guide's promise of `tileable` for wall material.
    #[tokio::test]
    async fn create_texture_tiles_the_brick_photo_into_a_wall() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_client(dir.path()).await;
        let source = write_png(dir.path(), "BRICKS.png", &brick_photo());
        let report = call(
            &client,
            "create_texture_from_image",
            json!({
                "source": source,
                "name": "MYBRICK",
                "tileable": true,
                "width": 128_u64,
                "height": 128_u64,
                "assign_role": "wall",
                "ops": [inner_crop()],
            }),
        )
        .await;
        assert_eq!(report["texture"], json!("MYBRICK"), "the texture name");
        assert_eq!(report["patch"], json!("MYBRICK"), "the patch follows");
        assert_eq!(report["lump"], json!("TEXTURE1"), "the definition lump");
        assert_eq!(report["role"], json!("wall"), "the assigned role");
        let steps = report["steps"].as_array().expect("a step list");
        let kinds: Vec<&str> = steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            kinds,
            vec!["crop", "fit", "make_tileable"],
            "the crop, the automatic resize, then the tiling blend"
        );
        let listed = call(&client, "list_textures", json!({ "pattern": "MYBRICK" })).await;
        assert_eq!(listed["total"], json!(1_u64), "one definition landed");
        assert_eq!(
            (
                listed["textures"][0]["width"].as_u64(),
                listed["textures"][0]["height"].as_u64()
            ),
            (Some(128_u64), Some(128_u64)),
            "the record answers 128x128"
        );
        let pnames = call(&client, "list_pnames", json!({ "pattern": "MYBRICK" })).await;
        assert_eq!(pnames["total"], json!(1_u64), "PNAMES lists the patch");
        assert_eq!(
            pnames["pnames"][0]["lump_found"],
            json!(true),
            "and the patch lump stands"
        );
        let preview_url = report["preview"]["url"].as_str().expect("a preview asset");
        let preview = asset_png(preview_url).await;
        assert_eq!(
            (preview.width, preview.height),
            (128_u32, 128_u32),
            "the preview carries the texture"
        );
        let drift = seam_drift(&preview);
        assert!(
            drift.iter().all(|channel| *channel <= MAX_SEAM_DRIFT),
            "the tileable edges must blend within {MAX_SEAM_DRIFT} per channel, got {drift:?}"
        );
    }

    /// The wall texture the second session dresses with (the same product
    /// as the third step's, re-created there because theme overrides are
    /// session state).
    const MYBRICK: &str = "MYBRICK";

    /// The floor flat the chain imports for the hall.
    const MYFLOOR: &str = "MYFLOOR";

    /// The all-angles sprite lump the drawing lands as.
    const TREEA0: &str = "TREEA0";

    /// The fixture map's hall sector: light 160, confirmed from the
    /// `describe_map` row at read time.
    const HALL_SECTOR: u64 = 0_u64;

    /// The `set_palette` colors of the second session: entry `i` is
    /// `(i, 255-i, 255-i)` — the fixture ramp with its cool channels
    /// paired, so warm greys and reds land red-dominant while the fixture
    /// lumps' stored indices (`WALL1`'s 100, `FLOOR1`'s 1) stay cool.
    /// The fixture `PLAYPAL` ties red to blue in every entry, so through
    /// it no rendered pixel can be red-dominant at all.
    fn warm_ramp_colors() -> Vec<String> {
        let mut colors = Vec::new();
        for index in 0_u8..=255_u8 {
            let warm = 255_u8.wrapping_sub(index);
            colors.push(format!("#{index:02X}{warm:02X}{warm:02X}"));
        }
        colors
    }

    /// Whether one `RGBA8` chunk is a pixel whose red channel strictly
    /// dominates both others: "reddish", compared without float math.
    ///
    /// The session's warm ramp `(i, 255-i, 255-i)` keeps the fixture's
    /// `WALL1` (stored index 100, so `100,155,155`) and `FLOOR1` (index 1)
    /// green/blue-dominant, so a red-dominant pixel at eye level can only
    /// be the retextured `MYBRICK` wall showing its mortar band (the brick
    /// rows sit above the middle row): the mirror of the first steps'
    /// blue-border check, and a render that proves the dress reached the
    /// engine, not just the model.
    fn red_dominant(pixel: &[u8]) -> bool {
        let &[red, green, blue, _] = pixel else {
            return false;
        };
        red > green && red > blue
    }

    /// The red-dominant pixel count of `frame`'s middle row, integer
    /// comparisons only.
    fn reddish_in_middle_row(frame: &RgbaImage) -> usize {
        let row = frame.height.checked_div(2_u32).expect("a test-tall frame");
        (0..frame.width)
            .filter(|&column| reddish_at(frame, column, row))
            .count()
    }

    /// Whether `frame`'s pixel at `(column, row)` exists and is
    /// red-dominant.
    fn reddish_at(frame: &RgbaImage, column: u32, row: u32) -> bool {
        frame
            .get(column, row)
            .is_some_and(|pixel| red_dominant(&pixel))
    }

    /// The flat's grey-noise source: a `70x70` sheet cycling five opaque
    /// greys over a `51x + 113y` mix, deterministic and integer-only.
    fn grey_noise_sheet() -> RgbaImage {
        let side = 70_u32;
        let pixels: Vec<u8> = (0..side)
            .flat_map(|y| (0..side).map(move |x| noise_grey(x, y)))
            .flatten()
            .collect();
        RgbaImage::new(side, side, pixels).expect("70x70 is a valid shape")
    }

    /// The noise grey of `(x, y)`: `30 + 45 * ((51x + 113y) mod 5)` in
    /// every channel, fully opaque.
    fn noise_grey(x: u32, y: u32) -> [u8; 4] {
        let mixed = x
            .checked_mul(51_u32)
            .expect("test x fits")
            .checked_add(y.checked_mul(113_u32).expect("test y fits"))
            .expect("the mix fits");
        let step =
            u8::try_from(mixed.checked_rem(5_u32).expect("a five-step cycle")).expect("steps fit");
        let grey = 30_u8
            .checked_add(step.checked_mul(45_u8).expect("greys fit a u8"))
            .expect("the grey fits");
        [grey, grey, grey, 255_u8]
    }

    /// The sprite drawing: a `60x60` white sheet carrying a solid green
    /// `20x40` figure.
    fn tree_drawing() -> RgbaImage {
        let mut sheet = paper_sheet(60_u32, 60_u32);
        fill_rect(&mut sheet, (20_u32, 10_u32, 40_u32, 50_u32), TREE_GREEN);
        sheet
    }

    /// The figure's solid green, far from the white the flood clears.
    const TREE_GREEN: [u8; 4] = [0_u8, 190, 0, 255];

    /// The session the remaining steps run on: the fixture `IWAD`, a warm
    /// `set_palette` ramp over its red-equals-blue `PLAYPAL`, and the
    /// first steps' `MYBRICK` brick photo created again as this session's
    /// wall theme — before the three-step undo window the last step
    /// walks back.
    async fn warm_brick_client(dir: &Path) -> Client {
        let client = iwad_client(dir).await;
        let palette = call(
            &client,
            "set_palette",
            json!({ "colors": warm_ramp_colors() }),
        )
        .await;
        assert_eq!(
            palette["palettes"],
            json!(14_u64),
            "all fourteen `PLAYPAL` entries stand"
        );
        assert_eq!(
            palette["replaced"],
            json!([0_u64]),
            "the normal palette is the one that moved"
        );
        let source = write_png(dir, "BRICKS.png", &brick_photo());
        let texture = call(
            &client,
            "create_texture_from_image",
            json!({
                "source": source,
                "name": MYBRICK,
                "tileable": true,
                "width": 128_u64,
                "height": 128_u64,
                "assign_role": "wall",
                "dither": "none",
                "ops": [inner_crop()],
            }),
        )
        .await;
        assert_eq!(texture["texture"], json!(MYBRICK), "the definition lands");
        assert_eq!(texture["role"], json!("wall"), "the wall role points at it");
        client
    }

    /// The `find_elements` read of every sidedef of the hall sector.
    async fn hall_sides(client: &Client) -> Value {
        call(
            client,
            "find_elements",
            json!({
                "map": "MAP01",
                "kind": "sidedefs",
                "within": { "sectors": [HALL_SECTOR] },
            }),
        )
        .await
    }

    /// The three texture slots a `find_elements` sidedef row reports.
    const SIDE_SLOTS: [&str; 3] = ["upper", "middle", "lower"];

    /// Whether one sidedef row wears `MYBRICK` alone: every slot carries
    /// `MYBRICK` or the empty `"-"`, and some slot carries `MYBRICK` —
    /// what `retexture` leaves, since it never fills empty slots.
    fn side_wears_mybrick(row: &Value) -> bool {
        let worn: Vec<&str> = SIDE_SLOTS
            .iter()
            .map(|slot| row[*slot].as_str().expect("a texture name"))
            .collect();
        worn.iter()
            .all(|texture| *texture == MYBRICK || *texture == "-")
            && worn.contains(&MYBRICK)
    }

    /// Assert the hall's four sides all wear `MYBRICK` alone.
    fn assert_sides_wear_mybrick(sides: &Value) {
        assert_eq!(sides["count"], json!(4_u64), "the hall has four sides");
        for row in sides["rows"].as_array().expect("a row list") {
            assert!(
                side_wears_mybrick(row),
                "side {} does not wear {MYBRICK:?} alone: {row}",
                row["index"]
            );
        }
    }

    /// The `describe_map` floor flat of the hall sector (row checked for
    /// the light-160 identity the ticket names).
    async fn hall_floor(client: &Client) -> String {
        let described = call(
            client,
            "describe_map",
            json!({ "map": "MAP01", "sectors": [HALL_SECTOR], "render": false }),
        )
        .await;
        let row = described["sectors"].get(0).expect("the asked row");
        assert_eq!(row["sector"], json!(HALL_SECTOR), "the row is the hall's");
        assert_eq!(
            row["light"],
            json!(160_i64),
            "sector 0 is the light-160 hall"
        );
        row["floor_flat"].as_str().expect("a floor flat").to_owned()
    }

    /// The chain's first step: the grey-noise sheet goes through
    /// `create_flat_from_image` with `assign_role: "floor"` and reads
    /// back through `list_flats` and `get_flat` as a `64x64` flat.
    async fn create_the_flat(client: &Client, dir: &Path) {
        let source = write_png(dir, "NOISE.png", &grey_noise_sheet());
        let report = call(
            client,
            "create_flat_from_image",
            json!({ "source": source, "name": MYFLOOR, "assign_role": "floor" }),
        )
        .await;
        assert_eq!(report["flat"], json!(MYFLOOR), "the name echoes");
        assert_eq!(
            report["role"],
            json!("floor"),
            "the floor role points at it"
        );
        let steps = report["steps"].as_array().expect("a step list");
        let kinds: Vec<&str> = steps
            .iter()
            .map(|step| step["op"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            kinds,
            vec!["fit", "make_tileable"],
            "the automatic square fit then the tiling blend"
        );
        let listed = call(client, "list_flats", json!({ "pattern": MYFLOOR })).await;
        assert_eq!(listed["total"], json!(1_u64), "the flats region holds it");
        assert_eq!(
            (
                listed["flats"][0]["width"].as_u64(),
                listed["flats"][0]["height"].as_u64()
            ),
            (Some(64_u64), Some(64_u64)),
            "a vanilla 64x64 raw flat"
        );
        let shown = call(client, "get_flat", json!({ "name": MYFLOOR })).await;
        assert_eq!(
            (shown["width"].as_u64(), shown["height"].as_u64()),
            (Some(64_u64), Some(64_u64)),
            "and `get_flat` reads the same shape back"
        );
    }

    /// The chain's second step: `auto_texture` in `retexture` mode over
    /// the hall sector, read back through `find_elements` and
    /// `describe_map`: every side of sector 0 wears `MYBRICK` and the
    /// sector's floor reads `MYFLOOR`.
    async fn retexture_the_hall(client: &Client) {
        // The same view before the dress keeps no red-dominant middle-row
        // pixel at all (every surface is cool through the warm ramp), so
        // the reddish pixels read after the dress can only be `MYBRICK`.
        assert_eq!(
            middle_row_reddish(client).await,
            0_usize,
            "the fixture dress shows no reddish wall at eye level"
        );
        let dress = call(
            client,
            "auto_texture",
            json!({ "map": "MAP01", "mode": "retexture", "sectors": [HALL_SECTOR] }),
        )
        .await;
        assert_eq!(dress["mode"], json!("retexture"), "the asked mode ran");
        assert_eq!(dress["map"], json!("MAP01"), "the fixture's only map");
        assert_eq!(
            dress["changed_sidedefs"],
            json!(4_u64),
            "three one-sided middles plus the step's lower band"
        );
        assert_eq!(
            dress["changed_sectors"],
            json!(1_u64),
            "the hall floor moved to the theme flat"
        );
        assert_eq!(
            dress["copied_to_working"],
            json!(true),
            "the block came in from the layer"
        );
        assert_eq!(
            dress["nodes_invalidated"],
            json!(false),
            "a dress edits no geometry"
        );
        assert_sides_wear_mybrick(&hall_sides(client).await);
        assert_eq!(
            hall_floor(client).await,
            MYFLOOR,
            "the hall now floors on the imported flat"
        );
    }

    /// The full-bright `render_view` from the player start, whose `x`,
    /// `y` and `angle` come from the `find_elements` thing row and are
    /// never hard-coded: the report and the fetched frame.
    async fn hall_view(client: &Client) -> (Value, RgbaImage) {
        let start = call(
            client,
            "find_elements",
            json!({
                "map": "MAP01",
                "kind": "things",
                "where": [{ "field": "type", "op": "eq", "value": 1_i64 }],
            }),
        )
        .await;
        assert_eq!(start["count"], json!(1_u64), "one player start");
        let row = start["rows"].get(0).expect("the start row");
        let report = call(
            client,
            "render_view",
            json!({
                "map": "MAP01",
                "x": row["x"],
                "y": row["y"],
                "angle": row["angle"],
                "width": 160_u64,
                "height": 100_u64,
                "lighting": "fullbright",
            }),
        )
        .await;
        assert_eq!(report["camera"]["x"], row["x"], "the echoed x");
        assert_eq!(report["camera"]["y"], row["y"], "and y");
        assert_eq!(
            report["camera"]["sector"],
            json!(HALL_SECTOR),
            "inside the hall"
        );
        let url = report["asset"]["url"]
            .as_str()
            .expect("a published frame")
            .to_owned();
        (report, asset_png(&url).await)
    }

    /// The red-dominant pixel count of the hall view's middle row.
    async fn middle_row_reddish(client: &Client) -> usize {
        let (_report, frame) = hall_view(client).await;
        reddish_in_middle_row(&frame)
    }

    /// The chain's look step: the dressed hall viewed from the player
    /// start keeps red-dominant middle-row pixels — the `MYBRICK` walls
    /// reach the camera, where the pre-dress view had none — and neither
    /// dressed picture reports as missing.
    async fn view_the_dressed_hall(client: &Client) {
        let (report, frame) = hall_view(client).await;
        let problems = report["problems"].as_array().expect("a problems list");
        assert!(
            !problems.contains(&json!("missing texture MYBRICK")),
            "the dressed wall loads: {problems:?}"
        );
        assert!(
            !problems.contains(&json!("missing flat MYFLOOR")),
            "the dressed floor loads: {problems:?}"
        );
        assert_eq!(
            (frame.width, frame.height),
            (160_u32, 100_u32),
            "the frame shape"
        );
        let reddish = reddish_in_middle_row(&frame);
        assert!(
            reddish > 0_usize,
            "the middle row shows no red-dominant pixel of the {MYBRICK} dress"
        );
    }

    /// The chain's sprite step: the green drawing lands as the all-angles
    /// `TREEA0` through the parts naming, trimmed to the figure with its
    /// feet on the origin, and `get_sprite` reads it back.
    async fn create_the_tree_sprite(client: &Client, dir: &Path) {
        let source = write_png(dir, "TREE.png", &tree_drawing());
        let report = call(
            client,
            "create_sprite_from_image",
            json!({ "source": source, "sprite": "TREE", "frame": "A", "rotation": 0_u64 }),
        )
        .await;
        assert_eq!(report["lumps"], json!([TREEA0]), "the parts name lands");
        assert_eq!(
            (report["width"].as_u64(), report["height"].as_u64()),
            (Some(20_u64), Some(40_u64)),
            "the flood and trim cut the figure out exactly"
        );
        let shown = call(client, "get_sprite", json!({ "name": TREEA0 })).await;
        assert_eq!(shown["lump"], json!(TREEA0), "the new lump serves it");
        assert_eq!(
            (shown["width"].as_u64(), shown["height"].as_u64()),
            (Some(20_u64), Some(40_u64)),
            "as stored"
        );
        assert_eq!(
            (shown["left_offset"].as_i64(), shown["top_offset"].as_i64()),
            (Some(10_i64), Some(40_i64)),
            "half the width left, the full height top: feet on the origin"
        );
    }

    /// One `undo` step, asserted to take exactly `label` back and leave
    /// `remaining` entries below it.
    async fn step_back(client: &Client, label: &str, remaining: u64) -> Value {
        let done = call(client, "undo", json!({})).await;
        assert_eq!(done["undone"], json!([label]), "the newest edit steps back");
        assert_eq!(
            done["remaining_undo"],
            json!(remaining),
            "the stack below it stands"
        );
        assert_eq!(
            done["active_map_cleared"],
            json!(false),
            "the fixture's map never left the merged view"
        );
        done
    }

    /// The chain's last step: three `undo` calls take the sprite lump,
    /// the `auto_texture` retexture and the flat back in that order, each
    /// verified with its read tool, while the `MYBRICK` created before
    /// that window — and the patch lump behind it — still stand.
    async fn undo_walks_back_the_window(client: &Client) {
        step_back(client, "create_sprite TREEA0", 4).await;
        let (sprite_code, sprite_message, _) =
            call_err(client, "get_sprite", json!({ "name": TREEA0 })).await;
        assert_eq!(
            sprite_code, "not_found",
            "the sprite is gone: {sprite_message}"
        );
        step_back(client, "auto_texture MAP01", 3).await;
        let plain = hall_sides(client).await;
        assert_eq!(
            plain["count"],
            json!(4_u64),
            "the four sides still belong to the hall"
        );
        let first = plain["rows"].get(0).expect("the first side");
        assert_eq!(
            first["middle"],
            json!("WALL1"),
            "the walls wear the fixture dress again"
        );
        let step = plain["rows"].get(2).expect("the step side");
        assert_eq!(step["lower"], json!("STEP1"), "and the step its own band");
        assert_eq!(
            hall_floor(client).await,
            "FLOOR1",
            "the retexture's flat paint rolled back with it"
        );
        step_back(client, "create_flat MYFLOOR", 2).await;
        let (flat_code, flat_message, _) =
            call_err(client, "get_flat", json!({ "name": MYFLOOR })).await;
        assert_eq!(flat_code, "not_found", "the flat is gone: {flat_message}");
        let listed = call(client, "list_flats", json!({ "pattern": MYFLOOR })).await;
        assert_eq!(
            listed["total"],
            json!(0_u64),
            "no flats-region trace is left"
        );
        let textures = call(client, "list_textures", json!({ "pattern": MYBRICK })).await;
        assert_eq!(
            textures["total"],
            json!(1_u64),
            "{MYBRICK}, created before the window, still defines"
        );
        let patch = call(client, "get_patch", json!({ "name": MYBRICK })).await;
        assert_eq!(
            (patch["width"].as_u64(), patch["height"].as_u64()),
            (Some(128_u64), Some(128_u64)),
            "and its patch still decodes"
        );
        let session = call(client, "get_session_info", json!({})).await;
        assert_eq!(
            session["undo"],
            json!(["set_palette", "create_texture MYBRICK"]),
            "exactly the two steps below the window remain"
        );
    }

    /// The guide's remaining chain as one real client run on one session:
    /// a warm palette, the `MYFLOOR` flat import (step 1), the `MYBRICK`
    /// retexture of the light-160 hall with its reads and a reddish
    /// first-person view from the player start (step 2), the `TREEA0`
    /// sprite import (step 3) and the three-step undo back over
    /// sprite, retexture and flat with `MYBRICK` standing (step 4).
    #[tokio::test]
    async fn flat_texture_dressing_sprite_and_undo_chain_end_to_end() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = warm_brick_client(dir.path()).await;
        create_the_flat(&client, dir.path()).await;
        retexture_the_hall(&client).await;
        view_the_dressed_hall(&client).await;
        create_the_tree_sprite(&client, dir.path()).await;
        undo_walks_back_the_window(&client).await;
    }
}
