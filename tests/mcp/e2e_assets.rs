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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::gfx::image::RgbaImage;
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad;
    use crate::support::{asset_png, call, connect, fill_rect, paper_sheet, write_png};

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
}
