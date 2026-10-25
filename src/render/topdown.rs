//! Shared top-down drawing: the `Doom` automap line classification, the
//! palette-derived automap colours and the square window mapping the
//! minimap frames its view with.
//!
//! This is the part of the overview renderer (`src/tools/render.rs`) that
//! is pure drawing maths: [`automap_color`] turns a linedef into the colour
//! the in-game automap gives it, [`AutomapColors`] holds the RGB defaults
//! and the `PLAYPAL` extraction, [`Window`] maps world coordinates into a
//! square pixel window, and the `paint_*` functions stroke whole maps,
//! thing dots and the player arrow onto a [`Canvas`]. The top-down tool
//! borrows the classification and the thing visuals; `render_minimap`
//! builds its whole picture from these pieces.

use crate::defs::Definitions;
use crate::fixed::{Angle, rotate_point};
use crate::gfx::palette::Palette;
use crate::render::{Canvas, Pt, Rgba};
use crate::wad::map::flags::{DONT_DRAW, SECRET};
use crate::wad::map::{Linedef, Map};

/// `PLAYPAL` palette-0 index of the automap one-sided-wall colour.
const PALETTE_WALL: usize = 176;
/// `PLAYPAL` palette-0 index of the automap floor-change colour.
const PALETTE_FLOOR_CHANGE: usize = 64;
/// `PLAYPAL` palette-0 index of the automap ceiling-change colour.
const PALETTE_CEILING_CHANGE: usize = 231;
/// `PLAYPAL` palette-0 index of the automap flat-line colour.
const PALETTE_OTHER: usize = 96;
/// `PLAYPAL` palette-0 index of the automap player-marker colour.
const PALETTE_PLAYER: usize = 209;

/// The five colours an automap-style drawing strokes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutomapColors {
    /// One-sided walls and secret lines.
    pub wall: Rgba,
    /// A floor-height change across a two-sided line.
    pub floor_change: Rgba,
    /// A ceiling-height change across a two-sided line.
    pub ceiling_change: Rgba,
    /// A flat two-sided line.
    pub other: Rgba,
    /// The player marker.
    pub player: Rgba,
}

impl AutomapColors {
    /// The RGB defaults: the automap style of the whole-map overview.
    pub const DEFAULT: Self = Self {
        wall: [252, 0, 0, 255],
        floor_change: [188, 120, 72, 255],
        ceiling_change: [252, 252, 0, 255],
        other: [128, 128, 128, 255],
        player: [255, 255, 255, 255],
    };

    /// The vanilla automap colours of `palette` (a `PLAYPAL` palette): the
    /// entries at the indices `Doom`'s automap uses, each kept opaque.
    #[must_use]
    #[inline]
    pub fn from_palette(palette: &Palette) -> Self {
        Self {
            wall: palette_pixel(palette, PALETTE_WALL, Self::DEFAULT.wall),
            floor_change: palette_pixel(palette, PALETTE_FLOOR_CHANGE, Self::DEFAULT.floor_change),
            ceiling_change: palette_pixel(
                palette,
                PALETTE_CEILING_CHANGE,
                Self::DEFAULT.ceiling_change,
            ),
            other: palette_pixel(palette, PALETTE_OTHER, Self::DEFAULT.other),
            player: palette_pixel(palette, PALETTE_PLAYER, Self::DEFAULT.player),
        }
    }
}

/// Palette colour `index` as an opaque pixel, `fallback` when the palette
/// does not hold that index.
fn palette_pixel(palette: &Palette, index: usize, fallback: Rgba) -> Rgba {
    match palette.color(index) {
        Some([red, green, blue]) => [red, green, blue, 255_u8],
        None => fallback,
    }
}

/// The `Doom` automap colour of `line`.
///
/// `None` for a hidden (`dontdraw`) line, the wall colour for one-sided
/// and secret lines, the floor-change and ceiling-change colours for
/// height changes across two-sided lines and the other colour for
/// everything else.
#[must_use]
#[inline]
pub fn automap_color(map: &Map, line: &Linedef, colors: &AutomapColors) -> Option<Rgba> {
    if line.flags & DONT_DRAW != 0 {
        return None;
    }
    let across = if line.flags & SECRET == 0 {
        line.back
    } else {
        None
    };
    let (Some(front), Some(back)) = (map.side_sector(line.front), map.side_sector(across)) else {
        return Some(colors.wall);
    };
    let (Some(front_sector), Some(back_sector)) = (map.sectors.get(front), map.sectors.get(back))
    else {
        return Some(colors.wall);
    };
    if front_sector.floor_height != back_sector.floor_height {
        return Some(colors.floor_change);
    }
    if front_sector.ceiling_height != back_sector.ceiling_height {
        return Some(colors.ceiling_change);
    }
    Some(colors.other)
}

/// The `i64` nearest an `i128` value (saturating at the bounds).
fn narrow(value: i128) -> i64 {
    i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX })
}

/// `value * num / den` rounded down, exact in `i128` and saturating back
/// to `i64`; a non-positive `den` behaves like `1`.
#[must_use]
#[inline]
pub fn scale_down(value: i64, num: i64, den: i64) -> i64 {
    let product = i128::from(value).saturating_mul(i128::from(num));
    narrow(product.div_euclid(i128::from(den).max(1)))
}

/// The square world window a minimap frames.
///
/// Maps world coordinates onto a square canvas: `radius` map units reach
/// from the centre to a window edge, and the canvas centre sits at half
/// the edge.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    /// Scale numerator: the canvas edge in pixels.
    num: i64,
    /// Scale denominator: the window width in world units.
    den: i64,
    /// Pixel column and row of the window centre.
    mid: i64,
    /// World `x` of the window centre.
    centre_x: i64,
    /// World `y` of the window centre.
    centre_y: i64,
    /// Map units from the centre to a window edge.
    radius: i64,
}

impl Window {
    /// The `size`-pixel square window centred on `(centre_x, centre_y)`
    /// with `radius` world units to every edge.
    #[must_use]
    #[inline]
    pub fn new(size: u32, centre_x: i64, centre_y: i64, radius: i64) -> Self {
        let edge = i64::from(size);
        Self {
            num: edge,
            den: radius.saturating_mul(2).max(1),
            mid: edge.checked_div(2).unwrap_or(0),
            centre_x,
            centre_y,
            radius,
        }
    }

    /// The world centre of the window.
    #[must_use]
    #[inline]
    pub const fn centre(&self) -> (i64, i64) {
        (self.centre_x, self.centre_y)
    }

    /// The map units from the centre to a window edge.
    #[must_use]
    #[inline]
    pub const fn radius(&self) -> i64 {
        self.radius
    }

    /// The pixel of world point `x`/`y` (floor-rounded, `y` flipped so
    /// north is up).
    #[must_use]
    #[inline]
    pub fn pixel(&self, x: i64, y: i64) -> Pt {
        let column = scale_down(x.saturating_sub(self.centre_x), self.num, self.den);
        let row = scale_down(y.saturating_sub(self.centre_y), self.num, self.den);
        (
            self.mid.saturating_add(column),
            self.mid.saturating_sub(row),
        )
    }
}

/// Length of the automap player arrow, in pixels.
pub const PLAYER_ARROW_PX: i64 = 16;
/// Length of the two barbs at the arrow's tip, in pixels.
const ARROW_BARB_PX: i64 = 5;
/// Degrees a barb turns back from the shaft at the arrow's tip.
const ARROW_BARB_DEG: i64 = 150;

/// A player position and facing for the automap marker; `angle` is in
/// degrees, counter-clockwise from east.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerMark {
    /// World `x`.
    pub x: i64,
    /// World `y`.
    pub y: i64,
    /// Facing in degrees, counter-clockwise from east.
    pub angle: i64,
}

/// Draws the player arrow: a shaft from `mark`'s position to its tip plus
/// two barbs back from the tip, all in the colours' player entry.
#[inline]
pub fn paint_player_arrow(
    mark: &PlayerMark,
    window: &Window,
    colors: &AutomapColors,
    canvas: &mut Canvas,
) {
    let at = window.pixel(mark.x, mark.y);
    let tip = offset_pixel(at, Angle::from_degrees(mark.angle), PLAYER_ARROW_PX);
    canvas.line(at, tip, colors.player);
    for turn in [ARROW_BARB_DEG.saturating_neg(), ARROW_BARB_DEG] {
        let heading = Angle::from_degrees(mark.angle.saturating_add(turn));
        let barb = offset_pixel(tip, heading, ARROW_BARB_PX);
        canvas.line(tip, barb, colors.player);
    }
}

/// The pixel `length` away from `from` along `angle` (north up).
fn offset_pixel(from: Pt, angle: Angle, length: i64) -> Pt {
    let (dx, dy) = rotate_point(length, 0_i64, angle);
    (from.0.saturating_add(dx), from.1.saturating_sub(dy))
}

/// Side of a minimap thing-marker dot, in pixels.
pub const THING_DOT_PX: i64 = 2;

/// Thing markers: Monsters.
const THING_MONSTER: Rgba = [224, 64, 64, 255];
/// Thing markers: Player Starts.
const THING_PLAYER: Rgba = [64, 224, 64, 255];
/// Thing markers: Weapons and Ammunition.
const THING_WEAPON: Rgba = [224, 160, 64, 255];
/// Thing markers: Health And Armour and Powerups.
const THING_SUPPORT: Rgba = [64, 160, 224, 255];
/// Thing markers: Keys.
const THING_KEY: Rgba = [224, 224, 64, 255];
/// Thing markers: every other category, and unknown types.
const THING_OTHER: Rgba = [176, 176, 176, 255];

/// The marker colour an editor category prefix selects.
#[must_use]
#[inline]
pub fn category_color(category: &str) -> Rgba {
    if category.starts_with("Monsters") {
        return THING_MONSTER;
    }
    if category.starts_with("Player Starts") {
        return THING_PLAYER;
    }
    if category.starts_with("Weapons") || category.starts_with("Ammunition") {
        return THING_WEAPON;
    }
    if category.starts_with("Health") || category.starts_with("Powerups") {
        return THING_SUPPORT;
    }
    if category.starts_with("Keys") {
        return THING_KEY;
    }
    THING_OTHER
}

/// World radius assumed for thing types the definitions do not know.
pub const UNKNOWN_THING_RADIUS: u32 = 16;

/// The marker colour and world radius of thing type `kind` according to
/// `defs`; an unknown type reads as `#B0B0B0` with radius 16.
#[must_use]
#[inline]
pub fn thing_visual(defs: &Definitions, kind: i32) -> (Rgba, u32) {
    let found = u32::try_from(kind)
        .ok()
        .and_then(|number| defs.thing(number));
    let Some(row) = found else {
        return (THING_OTHER, UNKNOWN_THING_RADIUS);
    };
    (category_color(&row.category), row.radius)
}

/// Draws every linedef of `map` through `window` in its automap colour,
/// skipping the hidden ones; a line with a dangling vertex draws nothing.
#[inline]
pub fn paint_automap_lines(
    map: &Map,
    window: &Window,
    colors: &AutomapColors,
    canvas: &mut Canvas,
) {
    for line in &map.linedefs {
        let Some(rgba) = automap_color(map, line, colors) else {
            continue;
        };
        let (Some(start), Some(end)) = (map.vertices.get(line.v1), map.vertices.get(line.v2))
        else {
            continue;
        };
        canvas.line(
            window.pixel(i64::from(start.x), i64::from(start.y)),
            window.pixel(i64::from(end.x), i64::from(end.y)),
            rgba,
        );
    }
}

/// Draws a [`THING_DOT_PX`] square dot in its category colour at every
/// thing of `map`, the thing's pixel as the dot's top-left corner.
#[inline]
pub fn paint_thing_dots(map: &Map, defs: &Definitions, window: &Window, canvas: &mut Canvas) {
    let corner = THING_DOT_PX.saturating_sub(1);
    for mark in &map.things {
        let at = window.pixel(i64::from(mark.x), i64::from(mark.y));
        let (rgba, _) = thing_visual(defs, mark.kind);
        canvas.fill_rect(
            at,
            (at.0.saturating_add(corner), at.1.saturating_add(corner)),
            rgba,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ARROW_BARB_PX, AutomapColors, PLAYER_ARROW_PX, PlayerMark, THING_DOT_PX, Window,
        automap_color, category_color, narrow, offset_pixel, paint_automap_lines,
        paint_player_arrow, paint_thing_dots, palette_pixel, scale_down, thing_visual,
    };
    use crate::defs::definitions_for;
    use crate::fixed::Angle;
    use crate::gfx::palette::Palette;
    use crate::render::{Canvas, Rgba};
    use crate::wad::game::Game;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::{LumpName, MapFormat};

    /// An opaque test colour.
    const fn rgb(red: u8, green: u8, blue: u8) -> Rgba {
        [red, green, blue, 255_u8]
    }

    /// The plain black canvas background the minimaps use.
    const BLACK: Rgba = [0, 0, 0, 255];

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Vertex::default()
        }
    }

    /// A flagged line from `v1` to `v2` with the given sides.
    fn edge(v1: usize, v2: usize, front: usize, back: Option<usize>) -> Linedef {
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
    fn side(sector: usize) -> Sidedef {
        Sidedef {
            sector,
            ..Sidedef::default()
        }
    }

    /// A sector with the given heights.
    fn room(floor_height: i32, ceiling_height: i32) -> Sector {
        Sector {
            floor_height,
            ceiling_height,
            ..Sector::default()
        }
    }

    /// A thing of `kind` at `x`/`y`.
    fn spawn(kind: i32, x: i32, y: i32) -> Thing {
        Thing {
            kind,
            x,
            y,
            flags: 7_u32,
            ..Thing::default()
        }
    }

    /// A four-corner box map: one-sided west wall (index 0), south
    /// floor-change line (1), east ceiling-change line (2) and a flat
    /// two-sided north line (3), plus one player start.
    fn box_map() -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("test names are valid"),
            format: MapFormat::Doom,
            udmf_namespace: None,
            things: vec![spawn(1, 64, 64)],
            vertices: vec![
                vertex(0, 0),
                vertex(128, 0),
                vertex(128, 128),
                vertex(0, 128),
            ],
            linedefs: vec![
                edge(0, 3, 0, None),
                edge(0, 1, 0, Some(1)),
                edge(1, 2, 0, Some(2)),
                edge(2, 3, 0, Some(3)),
            ],
            sidedefs: vec![side(0), side(1), side(2), side(3)],
            sectors: vec![room(0, 128), room(16, 128), room(0, 96), room(0, 128)],
            other: Vec::new(),
        }
    }

    /// The fixture palette: colour `i` is `(i, 255 - i, i)`.
    fn fixture_palette() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        for (index, entry) in colors.iter_mut().enumerate() {
            let value = u8::try_from(index).expect("indices stay in the u8 range");
            *entry = [value, 255_u8.wrapping_sub(value), value];
        }
        Palette::from(colors)
    }

    /// The automap colour of `map`'s linedef at `index`.
    fn colour_of(map: &Map, index: usize) -> Option<Rgba> {
        automap_color(map, map.linedefs.get(index)?, &AutomapColors::DEFAULT)
    }

    #[test]
    fn automap_colours_split_walls_changes_and_hidden_lines() {
        let map = box_map();
        assert_eq!(colour_of(&map, 0), Some(rgb(252, 0, 0)), "one-sided");
        assert_eq!(
            colour_of(&map, 1),
            Some(rgb(188, 120, 72)),
            "floor-height change"
        );
        assert_eq!(
            colour_of(&map, 2),
            Some(rgb(252, 252, 0)),
            "ceiling-height change"
        );
        assert_eq!(colour_of(&map, 3), Some(rgb(128, 128, 128)), "flat line");
        let mut hidden = map.linedefs.first().expect("line 0 exists").clone();
        hidden.flags = 128_u32;
        assert_eq!(
            automap_color(&map, &hidden, &AutomapColors::DEFAULT),
            None,
            "dontdraw lines are skipped"
        );
        let mut secret = map.linedefs.get(1).expect("line 1 exists").clone();
        secret.flags = 1_u32 | 32_u32;
        assert_eq!(
            automap_color(&map, &secret, &AutomapColors::DEFAULT),
            Some(rgb(252, 0, 0)),
            "a secret line reads as one-sided"
        );
    }

    #[test]
    fn dangling_sides_read_as_walls() {
        let map = box_map();
        let line = edge(0, 1, 7, Some(8));
        assert_eq!(
            automap_color(&map, &line, &AutomapColors::DEFAULT),
            Some(rgb(252, 0, 0)),
            "an unresolvable sidedef is a wall"
        );
        assert_eq!(
            map.side_sector(Some(0)),
            Some(0),
            "sidedef 0 opens sector 0"
        );
        assert_eq!(
            map.side_sector(Some(9)),
            None,
            "a dangling sidedef resolves to nothing"
        );
        assert_eq!(
            map.side_sector(None),
            None,
            "an absent side resolves to nothing"
        );
    }

    #[test]
    fn palette_colours_come_from_the_vanilla_indices() {
        let colors = AutomapColors::from_palette(&fixture_palette());
        assert_eq!(colors.wall, rgb(176, 79, 176), "index 176");
        assert_eq!(colors.floor_change, rgb(64, 191, 64), "index 64");
        assert_eq!(colors.ceiling_change, rgb(231, 24, 231), "index 231");
        assert_eq!(colors.other, rgb(96, 159, 96), "index 96");
        assert_eq!(colors.player, rgb(209, 46, 209), "index 209");
        assert_eq!(
            palette_pixel(&fixture_palette(), 999, AutomapColors::DEFAULT.wall),
            AutomapColors::DEFAULT.wall,
            "an index the palette lacks falls back"
        );
    }

    #[test]
    fn the_window_maps_centre_and_edges_to_pixels() {
        let window = Window::new(256, 100, -40, 50);
        assert_eq!(window.centre(), (100, -40), "the centre echoes");
        assert_eq!(window.radius(), 50, "the radius echoes");
        assert_eq!(
            window.pixel(100, -40),
            (128, 128),
            "the world centre centres"
        );
        assert_eq!(window.pixel(150, -40).0, 256, "the east edge clips away");
        assert_eq!(window.pixel(50, -40).0, 0, "the west edge lands");
        assert_eq!(window.pixel(100, 10).1, 0, "north rises to the top row");
    }

    #[test]
    fn scale_down_floors_towards_negative_infinity() {
        assert_eq!(scale_down(-32, 88, 320), -9, "-8.8 floors to -9");
        assert_eq!(scale_down(31, 88, 320), 8, "8.52 floors to 8");
        assert_eq!(scale_down(7, 1, 0), 7, "a zero denominator behaves like 1");
        assert_eq!(narrow(i128::from(i64::MAX)), i64::MAX, "in range stays");
        assert_eq!(
            narrow(i128::from(i64::MAX) + 1),
            i64::MAX,
            "above the range saturates"
        );
    }

    #[test]
    fn the_player_arrow_runs_from_its_position_to_its_tip() {
        let window = Window::new(64, 0, 0, 32);
        let mut canvas = Canvas::new(64, 64, BLACK);
        let mark = PlayerMark {
            x: 0,
            y: 0,
            angle: 0,
        };
        paint_player_arrow(&mark, &window, &AutomapColors::DEFAULT, &mut canvas);
        assert_eq!(
            canvas.pixel(32, 32),
            Some(rgb(255, 255, 255)),
            "the position"
        );
        assert_eq!(canvas.pixel(40, 32), Some(rgb(255, 255, 255)), "mid-shaft");
        assert_eq!(canvas.pixel(48, 32), Some(rgb(255, 255, 255)), "the tip");
        assert_eq!(
            canvas.pixel(16, 32),
            Some(BLACK),
            "behind the tail stays dark"
        );
        assert_eq!(
            offset_pixel((0, 0), Angle::NORTH, PLAYER_ARROW_PX),
            (0_i64, PLAYER_ARROW_PX.saturating_neg()),
            "north rises on the pixel row"
        );
        assert_eq!(
            offset_pixel((0, 0), Angle::EAST, ARROW_BARB_PX).0,
            ARROW_BARB_PX,
            "east runs along the column"
        );
    }

    #[test]
    fn automap_lines_and_thing_dots_land_through_the_window() {
        let map = box_map();
        let window = Window::new(128, 64, 64, 64);
        let mut canvas = Canvas::new(128, 128, BLACK);
        paint_automap_lines(&map, &window, &AutomapColors::DEFAULT, &mut canvas);
        assert_eq!(canvas.pixel(0, 64), Some(rgb(252, 0, 0)), "the west wall");
        assert_eq!(
            canvas.pixel(64, 0),
            Some(rgb(128, 128, 128)),
            "the north line"
        );
        let defs = definitions_for(Game::Doom2).expect("the embedded tables are valid");
        paint_thing_dots(&map, &defs, &window, &mut canvas);
        let dot = rgb(64, 224, 64);
        assert_eq!(canvas.pixel(64, 64), Some(dot), "the thing's own pixel");
        assert_eq!(canvas.pixel(65, 65), Some(dot), "the 2x2 corner");
        assert_eq!(canvas.pixel(66, 64), Some(BLACK), "just past the dot");
        assert_eq!(THING_DOT_PX, 2, "the documented dot size");
        assert_eq!(category_color("Monsters"), rgb(224, 64, 64), "monsters");
        assert_eq!(
            thing_visual(&defs, 4_000_000),
            (rgb(176, 176, 176), 16),
            "unknown types fall back"
        );
    }
}
