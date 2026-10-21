//! The capstone workflow: the low-level element tools composing a whole
//! level through one in-memory MCP client, plus the asset server proven
//! out of process against the spawned real binary.
//!
//! The chain is the ticket's: `open_iwad` the `DOOM II` fixture →
//! `new_wad` → `create_map MAP05` (binary `Doom`) → `create_sector_from_loop`
//! twice (room A `(0,0)-(512,512)` floor 0, room B `(512,128)-(768,384)`
//! floor 32 sharing the `x=512` edge so the step turns two-sided on its
//! own) → `edit_sidedef` the step's lower texture to `STEP1` → `add_thing`
//! a player 1 start and an imp → `build_all_nodes` → `validate_map` with
//! zero errors → `render_view` from the start facing east (fullbright: the
//! centre column must carry the flats, the step band and the far wall) and
//! the two `PNG` automap renders → `save_wad` to a temp
//! file, `new_wad`, `open_wad` it back and the summary matches, with
//! `diff_wads` naming `MAP05` as the working side's new map → a generated
//! `PNG` imported as a patch, dressed into the `NEWWALL` texture, hung on
//! the step and published at the texture's size by `get_asset_url` →
//! a `dry_run` `UDMF` conversion, three steps `undo`-ed and `redo`-ed back,
//! and an `optimize_wad` dry run that finds nothing to strip.
//!
//! The last test repeats the asset half through the real stdio binary
//! (`CARGO_BIN_EXE_cyberdemon`): upload `PNG`s with raw `HTTP` `PUT`s from
//! this process, import one by its uploaded `URL`, fetch its published
//! asset back over `HTTP`, and `trace_sketch` an uploaded drawing — the
//! asset server working across the process boundary, not just in memory.

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::path::Path;
    use std::process::Stdio;

    use cyberdemon::gfx::codec::encode_png;
    use cyberdemon::gfx::image::RgbaImage;
    use rmcp::RoleClient;
    use rmcp::ServiceExt as _;
    use rmcp::service::RunningService;
    use rmcp::transport::TokioChildProcess;
    use serde_json::{Value, json};
    use tokio::process::Command;

    use crate::fixtures::iwad;
    use crate::support::{asset_png, call, connect, fill_rect, http_put, paper_sheet, write_png};

    /// A connected MCP client.
    type Client = RunningService<RoleClient, ()>;

    /// The map every leg of the workflow builds, loads or renders.
    const MAP: &str = "MAP05";
    /// The doom-format name `create_map` gets.
    const FORMAT: &str = "doom";
    /// The fixture wall texture the rooms dress themselves with.
    const WALL: &str = "WALL1";
    /// The fixture step texture the shared wall's lower band gets.
    const STEP: &str = "STEP1";
    /// The fixture flat both rooms floor and ceiling.
    const FLAT: &str = "FLOOR1";
    /// The composite texture the image leg builds from its own patch.
    const NEW_TEXTURE: &str = "NEWWALL";
    /// The patch lump that image becomes.
    const NEW_PATCH: &str = "NEWWALLP";
    /// The doomednum of the player 1 start thing.
    const PLAYER1: i64 = 1;
    /// The doomednum of the imp thing.
    const IMP: i64 = 3001;
    /// The palette colour of the fixture's FLOOR1 flat (index 1).
    const FLAT_COLOUR: [u8; 4] = [1_u8, 254, 1, 255];
    /// Fullbright `STEP1` on a north-south line: `COLORMAP` table 1 of
    /// patch index 150 through the fixture palette.
    const STEP_COLOUR: [u8; 4] = [158_u8, 97, 158, 255];
    /// Fullbright `WALL1` on a north-south line: table 1 of index 100.
    const WALL_COLOUR: [u8; 4] = [108_u8, 147, 108, 255];
    /// The square edge of the imported wall picture, in pixels.
    const PATCH_SIDE: u32 = 32_u32;
    /// The green the imported wall picture is filled with (palette 64).
    const PATCH_COLOUR: [u8; 4] = [64_u8, 191, 64, 255];
    /// The wall thickness of the hand-drawn sketch, in pixels.
    const SKETCH_WALL: i64 = 4;
    /// How long the spawned binary gets to finish its handshake.
    const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);

    /// The `open_iwad` + `new_wad` + `create_map MAP05` opening of the
    /// workflow, on a fresh in-memory session.
    async fn fresh_map05(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        let started = call(&client, "new_wad", json!({})).await;
        assert_eq!(started["lumps"], json!(0_u64), "a fresh empty PWAD");
        let created = call(
            &client,
            "create_map",
            json!({"name": MAP, "format": FORMAT}),
        )
        .await;
        assert_eq!(created["map"], json!(MAP), "the block was created");
        assert_eq!(created["format"], json!(FORMAT), "in binary Doom format");
        assert_eq!(created["active_map"], json!(MAP), "and it is active");
        client
    }

    /// The `points` list of the rectangle `(x0,y0)..(x1,y1)`, CCW.
    fn rect_points((x0, y0, x1, y1): (i64, i64, i64, i64)) -> Vec<Value> {
        [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
            .into_iter()
            .map(|(x, y)| json!({"x": x, "y": y}))
            .collect()
    }

    /// One room's `create_sector_from_loop` arguments: the rectangle,
    /// its floor height, the shared `WALL1`/`FLOOR1` dressing.
    fn room_args(rect: (i64, i64, i64, i64), floor_height: i64) -> Value {
        json!({
            "map": MAP,
            "points": rect_points(rect),
            "floor_height": floor_height,
            "ceiling_height": 128_i64,
            "floor_flat": FLAT,
            "ceiling_flat": FLAT,
            "wall_texture": WALL,
        })
    }

    /// Draw both rooms, `B` sharing `A`'s east edge exactly: the reports
    /// prove the sector growth and that the shared wall turned two-sided.
    async fn draw_two_rooms(client: &Client) {
        let room_a = call(
            client,
            "create_sector_from_loop",
            room_args((0, 0, 512, 512), 0),
        )
        .await;
        assert_eq!(room_a["sector"], json!(0_u64), "room A is sector 0");
        assert_eq!(
            room_a["notes"].as_array().expect("notes is a list").len(),
            0_usize,
            "the void room needs no notes"
        );
        let room_b = call(
            client,
            "create_sector_from_loop",
            room_args((512, 128, 768, 384), 32),
        )
        .await;
        assert_eq!(room_b["sector"], json!(1_u64), "room B is sector 1");
        let summary = call(client, "get_map_summary", json!({"map": MAP})).await;
        assert_eq!(summary["counts"]["sectors"], json!(2_u64), "two rooms");
        assert_eq!(
            summary["lines"]["two_sided"],
            json!(1_u64),
            "the shared x=512 span became exactly one two-sided line"
        );
    }

    /// The sidedefs whose `lower` band carries `texture`: both faces of
    /// the shared step, found by query, never by a hard-coded index.
    async fn sides_with_lower(client: &Client, texture: &str) -> Vec<u64> {
        let found = call(
            client,
            "find_elements",
            json!({"kind": "sidedefs", "map": MAP,
                   "where": [{"field": "lower", "op": "eq", "value": texture}]}),
        )
        .await;
        assert_eq!(
            found["count"],
            json!(2_u64),
            "the step dresses its riser on both the A and the B side: {found}"
        );
        found["indices"]
            .as_array()
            .expect("an index list")
            .iter()
            .map(|index| index.as_u64().expect("a sidedef index"))
            .collect()
    }

    /// Dress both step faces' lower band and check the stored echo.
    async fn dress_lower(client: &Client, indices: &[u64], texture: &str) {
        let edited = call(
            client,
            "edit_sidedef",
            json!({"map": MAP, "index": indices, "lower": texture}),
        )
        .await;
        let rows = edited["sidedefs"].as_array().expect("edited rows");
        assert_eq!(rows.len(), indices.len(), "every selected face came back");
        for row in rows {
            assert_eq!(row["lower"], json!(texture), "a band now names {texture}");
        }
    }

    /// Stand the player 1 start in room A and the imp in room B.
    async fn stand_start_and_imp(client: &Client) {
        let start = call(
            client,
            "add_thing",
            json!({"map": MAP, "type": PLAYER1, "x": 64_i64, "y": 256_i64, "angle": 0_i64}),
        )
        .await;
        assert_eq!(start["index"], json!(0_u64), "the start is thing 0");
        let imp = call(
            client,
            "add_thing",
            json!({"map": MAP, "type": IMP, "x": 640_i64, "y": 256_i64, "angle": 180_i64}),
        )
        .await;
        assert_eq!(imp["index"], json!(1_u64), "the imp is thing 1");
    }

    /// The full workflow through the step dressing, the things, the node
    /// build and the clean validation: the shared prefix of every test.
    async fn built_and_validated(dir: &Path) -> Client {
        let client = fresh_map05(dir).await;
        draw_two_rooms(&client).await;
        let step_sides = sides_with_lower(&client, WALL).await;
        dress_lower(&client, &step_sides, STEP).await;
        stand_start_and_imp(&client).await;
        let built = call(&client, "build_all_nodes", json!({"map": MAP})).await;
        assert_eq!(built["variant"], json!("classic"), "a vanilla build");
        assert!(
            built["nodes"]["segs"].as_u64().is_some_and(|segs| segs > 0),
            "the tree has segs: {}",
            built["nodes"]
        );
        assert_eq!(
            built["nodes_invalidated"],
            json!(false),
            "a build invalidates nothing"
        );
        let report = call(&client, "validate_map", json!({"map": MAP})).await;
        assert_eq!(
            report["counts"]["error"],
            json!(0_u64),
            "no errors, warnings allowed: {}",
            report["issues"]
        );
        client
    }

    /// `image`'s pixels down column `x`, top to bottom.
    fn column(image: &RgbaImage, x: u32) -> Vec<[u8; 4]> {
        (0..image.height).filter_map(|y| image.get(x, y)).collect()
    }

    /// Render the whole map three ways: the east-facing fullbright view
    /// must paint flats, the step band and the far wall in its centre
    /// column, and both automap renders must publish decodable pictures.
    async fn renders_prove_the_room(client: &Client) {
        let view = call(
            client,
            "render_view",
            json!({"map": MAP, "x": 64_i64, "y": 256_i64, "angle": 0_i64,
                   "lighting": "fullbright"}),
        )
        .await;
        assert_eq!(
            view["camera"]["sector"],
            json!(0_u64),
            "the camera stands in room A: {}",
            view["camera"]
        );
        assert_eq!(view["masked_columns"], json!(0_u64), "nothing masks");
        let url = view["asset"]["url"].as_str().expect("a published view");
        let image = asset_png(url).await;
        assert_eq!((image.width, image.height), (640, 400), "the default frame");
        let middle = image
            .width
            .checked_div(2)
            .expect("a wide enough frame for a centre column");
        let pixels = column(&image, middle);
        assert!(
            !pixels.iter().all(|p| p[..3] == [0_u8, 0, 0]),
            "the centre column is not all background"
        );
        assert!(
            pixels.contains(&FLAT_COLOUR),
            "the FLOOR1 plane shows in the centre column"
        );
        assert!(
            pixels.contains(&STEP_COLOUR),
            "the STEP1 band of the x=512 step shows"
        );
        assert!(
            pixels.contains(&WALL_COLOUR),
            "the far WALL1 wall across room B shows"
        );
        let topdown = call(client, "render_topdown", json!({"map": MAP})).await;
        let top_url = topdown["asset"]["url"].as_str().expect("a published map");
        let sheet = asset_png(top_url).await;
        assert!(
            sheet.width > 0 && sheet.height > 0,
            "the top-down frame has a size"
        );
        let mini = call(client, "render_minimap", json!({"map": MAP})).await;
        let mini_url = mini["asset"]["url"].as_str().expect("a published square");
        let square = asset_png(mini_url).await;
        assert_eq!(square.width, square.height, "the minimap is square");
    }

    /// Write the workflow's map to a file, replace the working archive
    /// twice over with the file's copy, and return the pre-save and
    /// post-reload summaries.
    async fn save_reload_and_summarise(client: &Client, dir: &Path) -> (Value, Value) {
        let before = call(client, "get_map_summary", json!({"map": MAP})).await;
        let path = dir.join("capstone-map05.wad");
        let saved = call(
            client,
            "save_wad",
            json!({"path": path.display().to_string()}),
        )
        .await;
        assert_eq!(saved["dirty"], json!(false), "the save marks it saved");
        assert!(
            saved["bytes"].as_u64().is_some_and(|bytes| bytes > 0),
            "bytes went to disk: {saved}"
        );
        call(client, "new_wad", json!({})).await;
        call(
            client,
            "open_wad",
            json!({"path": path.display().to_string()}),
        )
        .await;
        let after = call(client, "get_map_summary", json!({"map": MAP})).await;
        (before, after)
    }

    /// The reload preserved what was saved, and the diff shows the reloaded
    /// file's `MAP05` as the working side's new map.
    async fn reload_round_trip(client: &Client, dir: &Path) {
        let (before, after) = save_reload_and_summarise(client, dir).await;
        for field in ["counts", "lines", "heights", "bounds"] {
            assert_eq!(
                after[field], before[field],
                "the {field} section survived the save and reload"
            );
        }
        assert_eq!(
            after["nodes"]["present"],
            json!(true),
            "the nodes came back"
        );
        let diff = call(client, "diff_wads", json!({})).await;
        assert_eq!(diff["a"], json!("base"), "base is the left side");
        assert_eq!(
            diff["maps"][0]["name"],
            json!(MAP),
            "the working side's only map row: {}",
            diff["maps"]
        );
        assert_eq!(
            diff["maps"][0]["status"],
            json!("new"),
            "the IWAD never held MAP05"
        );
        assert_eq!(diff["summary"]["new"], json!(1_u64), "one new unit");
    }

    /// The flat `32x32` green sheet the import legs use as their picture.
    fn wall_sheet() -> RgbaImage {
        let mut sheet = paper_sheet(PATCH_SIDE, PATCH_SIDE);
        fill_rect(
            &mut sheet,
            (0_u32, 0_u32, PATCH_SIDE, PATCH_SIDE),
            PATCH_COLOUR,
        );
        sheet
    }

    /// The imported wall picture's file source.
    fn wall_picture(dir: &Path) -> String {
        write_png(dir, "capstone-wall.png", &wall_sheet())
    }

    /// The asset chain on the reloaded map: import the picture as
    /// `NEWWALLP`, stack it into the `NEWWALL` texture, dress the step's
    /// lower band with it, and have `get_asset_url` publish the finished
    /// texture at its full size.
    async fn texture_from_the_picture(client: &Client, dir: &Path) {
        let source = wall_picture(dir);
        let imported = call(
            client,
            "import_image_as_patch",
            json!({"source": source, "name": NEW_PATCH}),
        )
        .await;
        assert_eq!(imported["width"], json!(PATCH_SIDE), "the patch's width");
        assert!(
            imported["pnames_index"].as_u64().is_some(),
            "the name joined PNAMES: {imported}"
        );
        let built = call(
            client,
            "build_texture",
            json!({"name": NEW_TEXTURE, "width": PATCH_SIDE, "height": PATCH_SIDE,
                   "patches": [{"patch": NEW_PATCH, "x": 0_i64, "y": 0_i64}]}),
        )
        .await;
        assert_eq!(built["lump"], json!("TEXTURE1"), "into TEXTURE1");
        assert_eq!(built["created"], json!(true), "copied in as an override");
        let step_sides = sides_with_lower(client, STEP).await;
        dress_lower(client, &step_sides, NEW_TEXTURE).await;
        let asset = call(client, "get_asset_url", json!({"name": NEW_TEXTURE})).await;
        assert_eq!(
            asset["resolved_as"],
            json!("texture"),
            "the name resolves as a composite texture"
        );
        assert_eq!(asset["width"], json!(PATCH_SIDE), "at the texture's width");
        assert_eq!(
            asset["height"],
            json!(PATCH_SIDE),
            "at the texture's height"
        );
        let url = asset["asset"]["url"].as_str().expect("a published PNG");
        let image = asset_png(url).await;
        assert_eq!(
            (image.width, image.height),
            (PATCH_SIDE, PATCH_SIDE),
            "the published picture is the texture size"
        );
        assert_eq!(
            image.get(0, 0).expect("a pixel"),
            PATCH_COLOUR,
            "texel (0,0) is the imported patch's colour"
        );
    }

    /// The dry `UDMF` conversion, the three-step `undo`/`redo` round trip
    /// and the clean `optimize_wad` dry run.
    async fn convert_undo_redo_optimize(client: &Client) {
        let converted = call(
            client,
            "convert_map_format",
            json!({"map": MAP, "to": "udmf", "namespace": "zdoom", "dry_run": true}),
        )
        .await;
        assert_eq!(converted["dry_run"], json!(true), "nothing was written");
        assert_eq!(converted["from"], json!("doom"), "from binary Doom");
        assert_eq!(converted["to"], json!("udmf"), "to UDMF");
        let assets = call(client, "get_map_summary", json!({"map": MAP})).await;
        expect_history_walk(client, &assets).await;
        let planned = call(client, "optimize_wad", json!({"dry_run": true})).await;
        assert_eq!(
            planned["actions"],
            json!([]),
            "the plan strips nothing: {}",
            planned["actions"]
        );
        assert_eq!(
            planned["lumps_removed"],
            json!(0_u64),
            "no lump is marked for removal"
        );
    }

    /// Undo the three asset edits (sidedef, texture, patch), watch the map
    /// and the archive shrink back, redo them all, watch them return.
    async fn expect_history_walk(client: &Client, assets: &Value) {
        let undone = call(client, "undo", json!({"steps": 3_u64})).await;
        assert_eq!(
            undone["undone"],
            json!([
                "edit 2 sidedefs",
                format!("build_texture {NEW_TEXTURE}"),
                format!("import_patch {NEW_PATCH}"),
            ]),
            "the three asset steps, newest first"
        );
        assert_eq!(
            undone["remaining_undo"],
            json!(0_u64),
            "the reload started the history: {undone}"
        );
        assert_eq!(undone["dirty"], json!(false), "back to the saved file");
        call(client, "redo", json!({"steps": 3_u64})).await;
        let back = call(client, "get_map_summary", json!({"map": MAP})).await;
        assert_eq!(back, *assets, "every asset edit replayed exactly");
    }

    /// `trace_sketch`'s picture: an `80x80` white sheet with a closed
    /// `4`-pixel black ring — one clean rectangular region.
    fn sketch_sheet() -> RgbaImage {
        let mut sheet = paper_sheet(80_u32, 80_u32);
        let wall = u32::try_from(SKETCH_WALL).expect("a test thickness");
        let (edge, end) = (8_u32, 72_u32);
        fill_rect(
            &mut sheet,
            (edge, edge, end, edge.checked_add(wall).expect("fits")),
            [0_u8, 0, 0, 255],
        );
        fill_rect(
            &mut sheet,
            (edge, end.checked_sub(wall).expect("fits"), end, end),
            [0_u8, 0, 0, 255],
        );
        fill_rect(
            &mut sheet,
            (edge, edge, edge.checked_add(wall).expect("fits"), end),
            [0_u8, 0, 0, 255],
        );
        fill_rect(
            &mut sheet,
            (end.checked_sub(wall).expect("fits"), edge, end, end),
            [0_u8, 0, 0, 255],
        );
        sheet
    }

    /// Encode `sheet` for an upload body.
    fn png_of(sheet: &RgbaImage) -> Vec<u8> {
        encode_png(sheet, None).expect("the test sheet encodes")
    }

    /// `PUT` one picture to the asset origin at `base` and return the
    /// receipt's URL that tools can take as a `source`.
    async fn upload_picture(base: &str, filename: &str, sheet: &RgbaImage) -> String {
        let (status, receipt) =
            http_put(base, &format!("/upload/{filename}"), &png_of(sheet)).await;
        assert_eq!(status, 201, "the upload was accepted: {receipt:?}");
        let body: Value = serde_json::from_slice(&receipt).expect("a JSON receipt");
        body["url"]
            .as_str()
            .expect("the receipt names a url")
            .to_owned()
    }

    #[tokio::test]
    async fn the_low_level_tools_compose_a_playable_textured_room() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = built_and_validated(dir.path()).await;
        renders_prove_the_room(&client).await;
    }

    #[tokio::test]
    async fn the_map_survives_the_save_reload_and_reports_as_new() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = built_and_validated(dir.path()).await;
        reload_round_trip(&client, dir.path()).await;
    }

    #[tokio::test]
    async fn the_whole_workflow_composes_through_assets_and_history() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = built_and_validated(dir.path()).await;
        renders_prove_the_room(&client).await;
        reload_round_trip(&client, dir.path()).await;
        texture_from_the_picture(&client, dir.path()).await;
        convert_undo_redo_optimize(&client).await;
    }

    /// Spawn the real `CARGO_BIN_EXE_cyberdemon` binary and connect an
    /// rmcp client to its stdio, the way `server.rs` does.
    async fn spawn_binary() -> Client {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyberdemon"));
        command.stderr(Stdio::null());
        let transport = TokioChildProcess::new(command).expect("spawn the binary");
        tokio::time::timeout(SPAWN_TIMEOUT, ().serve(transport))
            .await
            .expect("the spawned server initialized in time")
            .expect("the handshake completed")
    }

    #[tokio::test]
    async fn the_real_binary_serves_uploads_assets_and_sketches() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let mut client = spawn_binary().await;
        let info = call(&client, "get_asset_server_info", json!({})).await;
        let base = info["base_url"].as_str().expect("a base url").to_owned();
        assert_eq!(
            info["upload_url_template"],
            json!(format!("{base}/upload/{{filename}}")),
            "the upload template sits on the reported origin"
        );
        let picture = upload_picture(&base, "capstone-wall.png", &wall_sheet()).await;
        let sketch = upload_picture(&base, "capstone-sketch.png", &sketch_sheet()).await;
        let base_iwad = iwad(dir.path());
        call(
            &client,
            "open_iwad",
            json!({"path": base_iwad.display().to_string()}),
        )
        .await;
        call(&client, "new_wad", json!({})).await;
        let imported = call(
            &client,
            "import_image_as_patch",
            json!({"source": picture, "name": NEW_PATCH}),
        )
        .await;
        assert_eq!(imported["name"], json!(NEW_PATCH), "by its own name");
        assert_eq!(imported["width"], json!(PATCH_SIDE), "the uploaded width");
        let asset = call(&client, "get_asset_url", json!({"name": NEW_PATCH})).await;
        assert_eq!(asset["resolved_as"], json!("patch"), "a patch picture");
        let url = asset["asset"]["url"].as_str().expect("a published PNG");
        let fetched = asset_png(url).await;
        assert_eq!(
            (fetched.width, fetched.height),
            (PATCH_SIDE, PATCH_SIDE),
            "the published PNG came back across the socket"
        );
        let traced = call(&client, "trace_sketch", json!({"source": sketch})).await;
        let regions = traced["regions"].as_array().expect("a regions list");
        assert!(
            !regions.is_empty(),
            "the uploaded sketch measured at least one region: {}",
            traced["regions"]
        );
        client.close().await.expect("a clean shutdown");
    }
}
