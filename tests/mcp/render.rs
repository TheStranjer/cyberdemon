//! The `render` tool group as seen by a real MCP client: the session-level
//! render defaults, `render_topdown`' pictures, `render_minimap`' compact
//! automap square, `render_view`' first-person frame and the `build_scene`
//! preparation the 3D view runs on.
//!
//! The helpers every topic drives the server through live here: a connected
//! client on the fixture `IWAD` (the two-room `MAP01`), the fitted
//! `transform` of a top-down report with its documented pixel mapping, the
//! asset fetch and `PNG` decode behind every pixel assertion and the
//! non-background pixel count. The tests
//! themselves are split by topic into the `settings`, `topdown`, `overlays`,
//! `decorations`, `minimap`, `view`, `view_sprites` and `scene` modules: the
//! settings and their validation, the plain top-down pictures, the `underlay`
//! beneath them, the grid, labels and annotations over them, the minimap
//! square, the first-person frame, its sprite pass and the scene build.

use std::path::Path;

use cyberdemon::gfx::codec::decode_image;
use cyberdemon::gfx::image::RgbaImage;
use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::Value;

use crate::fixtures::iwad;
use crate::support::{call, connect, count_pixels, fetch_asset, path_arg};

/// A connected MCP client.
type Client = RunningService<RoleClient, ()>;

/// Load the fixture `IWAD` (the two-room `MAP01`) as the base.
async fn iwad_client(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad(dir);
    call(&client, "open_iwad", path_arg(&base)).await;
    client
}

/// The fitted `transform` of a top-down report.
struct Transform {
    /// Scale numerator.
    num: i64,
    /// Scale denominator.
    den: i64,
    /// Pixel column of world `x = 0`.
    offset_x: i64,
    /// Pixel row of world `y = 0`.
    offset_y: i64,
}

impl Transform {
    /// Read the transform out of a report.
    fn of(report: &Value) -> Self {
        let object = &report["transform"];
        Self {
            num: object["scale_num"].as_i64().expect("scale_num"),
            den: object["scale_den"].as_i64().expect("scale_den"),
            offset_x: object["offset_x"].as_i64().expect("offset_x"),
            offset_y: object["offset_y"].as_i64().expect("offset_y"),
        }
    }

    /// The documented pixel mapping: `px = offset_x + x*num/den`,
    /// `py = offset_y - y*num/den` (non-negative test points).
    fn project(&self, x: i64, y: i64) -> (u32, u32) {
        let column = self.offset_x.saturating_add(
            x.checked_mul(self.num)
                .expect("test product")
                .checked_div(self.den)
                .expect("non-zero scale"),
        );
        let row = self.offset_y.saturating_sub(
            y.checked_mul(self.num)
                .expect("test product")
                .checked_div(self.den)
                .expect("non-zero scale"),
        );
        (
            u32::try_from(column).expect("a fitted column"),
            u32::try_from(row).expect("a fitted row"),
        )
    }
}

/// Fetch and decode the overview asset of a report.
async fn rendered_image(report: &Value) -> RgbaImage {
    let url = report["asset"]["url"].as_str().expect("an asset url");
    decode_image(&fetch_asset(url).await).expect("the overview is a readable PNG")
}

/// The number of pixels of `image` that are not the plain black
/// background the renders start from.
fn non_background(image: &RgbaImage) -> usize {
    (0..image.height)
        .flat_map(|row| (0..image.width).map(move |col| (col, row)))
        .filter(|&(col, row)| image.get(col, row) != Some([0, 0, 0, 255]))
        .count()
}

#[cfg(test)]
#[path = "render/decorations.rs"]
mod decorations;
#[cfg(test)]
#[path = "render/minimap.rs"]
mod minimap;
#[cfg(test)]
#[path = "render/overlays.rs"]
mod overlays;
#[cfg(test)]
#[path = "render/scene.rs"]
mod scene;
#[cfg(test)]
#[path = "render/settings.rs"]
mod settings;
#[cfg(test)]
#[path = "render/topdown.rs"]
mod topdown;
#[cfg(test)]
#[path = "render/view.rs"]
mod view;
#[cfg(test)]
#[path = "render/view_sprites.rs"]
mod view_sprites;
