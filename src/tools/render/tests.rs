//! Unit tests of the render tools: the settings reports in [`settings`],
//! the fit, resolution and painted fixture in [`topdown`], the overlay
//! parameters in [`overlays`], the painted overlays in [`painting`], the
//! minimap in [`minimap`] and the sprite pass in [`view`].
//! [`two_rooms`] builds the fixture map, [`request_for`] and [`pixel_of`]
//! the default calls, [`rgb`] the colours, [`fixture_palette`] the
//! `PLAYPAL` stand-in and [`options_with`] the settings arguments.

mod minimap;
mod overlays;
mod painting;
mod settings;
mod topdown;
mod view;

use crate::defs::definitions_for;
use crate::gfx::palette::Palette;
use crate::render::{Rgba, Switch, TopdownStyle};
use crate::wad::game::Game;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
use crate::wad::{LumpName, MapFormat};

use crate::tools::render::SetRenderOptionsParams;
use crate::tools::render::params::{Highlights, Labels, Projection, Request};
use crate::tools::render::resolve::resolve_bounds;
use crate::tools::render::topdown::paint_map;

/// Options with the listed fields set and every other left out.
pub(super) fn options_with(
    lighting: Option<&str>,
    fov: Option<u32>,
    background: Option<&str>,
) -> SetRenderOptionsParams {
    SetRenderOptionsParams {
        lighting: lighting.map(str::to_owned),
        fov,
        background: background.map(str::to_owned),
        ..SetRenderOptionsParams::default()
    }
}

/// An opaque test colour.
pub(super) const fn rgb(red: u8, green: u8, blue: u8) -> Rgba {
    [red, green, blue, 255_u8]
}

/// A vertex at `x`/`y`.
pub(super) fn vertex(x: i32, y: i32) -> Vertex {
    Vertex {
        x,
        y,
        ..Vertex::default()
    }
}

/// A blocking line from `v1` to `v2` with the given sides.
pub(super) fn edge(v1: usize, v2: usize, front: usize, back: Option<usize>) -> Linedef {
    Linedef {
        v1,
        v2,
        flags: 1_u32,
        front: Some(front),
        back,
        ..Linedef::default()
    }
}

/// A sidedef opening into `sector`.
pub(super) fn side(sector: usize) -> Sidedef {
    Sidedef {
        sector,
        ..Sidedef::default()
    }
}

/// A sector with the given heights and light.
pub(super) fn room(floor_height: i32, ceiling_height: i32, light: i32) -> Sector {
    Sector {
        floor_height,
        ceiling_height,
        light,
        ..Sector::default()
    }
}

/// A thing of `kind` at `x`/`y` facing `angle`.
pub(super) fn spawn(kind: i32, x: i32, y: i32, angle: i32) -> Thing {
    Thing {
        kind,
        x,
        y,
        angle,
        flags: 7_u32,
        ..Thing::default()
    }
}

/// The two-room fixture as the renderer sees it: a hall (sector 0,
/// light 160), a raised back room (sector 1, light 192), the two-sided
/// step line at `x = 256` (index 2), a player start and an imp.
pub(super) fn two_rooms() -> Map {
    Map {
        name: LumpName::parse("MAP01").expect("test names are valid"),
        format: MapFormat::Doom,
        udmf_namespace: None,
        things: vec![spawn(1, 64, 128, 0), spawn(3001, 384, 128, 180)],
        vertices: vec![
            vertex(0, 0),
            vertex(256, 0),
            vertex(512, 0),
            vertex(512, 256),
            vertex(256, 256),
            vertex(0, 256),
        ],
        linedefs: vec![
            edge(0, 5, 0, None),
            edge(5, 4, 1, None),
            edge(4, 1, 2, Some(3)),
            edge(1, 0, 4, None),
            edge(4, 3, 5, None),
            edge(3, 2, 6, None),
            edge(2, 1, 7, None),
        ],
        sidedefs: vec![
            side(0),
            side(0),
            side(0),
            side(1),
            side(0),
            side(1),
            side(1),
            side(1),
        ],
        sectors: vec![room(0, 128, 160), room(32, 128, 192)],
        other: Vec::new(),
    }
}

/// The default test request over `map`: top-down, 200x120, editor.
pub(super) fn request_for(map: &Map) -> Request {
    Request {
        projection: Projection::Topdown,
        width: 200,
        height: 120,
        style: TopdownStyle::Editor,
        bounds: resolve_bounds(None, map, None).expect("the fixture map bounds itself"),
        background: [0_u8, 0, 0, 255],
        show_things: Switch::new(true),
        show_vertices: Switch::new(false),
        fill_sectors: Switch::new(false),
        highlight: Highlights::default(),
        underlay: None,
        grid: None,
        labels: Labels::default(),
        annotations: Vec::new(),
        label_scale: 1_u32,
    }
}

/// Paint the `map` and read one pixel of the result.
pub(super) fn pixel_of(map: &Map, request: &Request, x: u32, y: u32) -> Option<Rgba> {
    let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
    paint_map(map, &defs, request).canvas.pixel(x, y)
}

/// The fixture palette: colour `i` is `(i, 255 - i, i)`.
pub(super) fn fixture_palette() -> Palette {
    let mut colors = [[0_u8; 3]; 256];
    for (index, entry) in colors.iter_mut().enumerate() {
        let value = u8::try_from(index).expect("indices stay in the u8 range");
        *entry = [value, 255_u8.wrapping_sub(value), value];
    }
    Palette::from(colors)
}
