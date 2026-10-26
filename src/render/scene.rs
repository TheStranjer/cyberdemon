//! Scene preparation for the 3D view: everything a render needs, read
//! once from the session.
//!
//! [`build_scene`] folds the merged view into a [`Scene`]: the map model,
//! the `PLAYPAL` palette, the 34 `COLORMAP` lighting tables, the composed
//! image of every wall texture and flat the map actually uses, the sky
//! image its name and game select when the map shows a sky plane, and the
//! settings to draw with. What cannot be found is not an error: a texture,
//! flat or sky the map names and the session lacks gets a 64x64
//! checkerboard fallback built from the palette's brightest and darkest
//! colours and a line in [`Scene::problems`], so the view still renders.
//! A picture no part of the map draws is never looked for and never
//! reported. Only a missing map block or `PLAYPAL` fails the call. The
//! camera helpers locate the viewer's sector and its default eye height.

use alloc::collections::{BTreeMap, BTreeSet};

use crate::error::CyberError;
use crate::gfx::colormap::{TABLE_BYTES, colormaps, colormaps_from_bytes};
use crate::gfx::image::IndexedImage;
use crate::gfx::ops::luminance;
use crate::gfx::palette::Palette;
use crate::render::RenderSettings;
use crate::session::mapdir::load_map;
use crate::session::palette::load_palette;
use crate::session::resolve::{load_flat, load_texture};
use crate::session::{Scope, Session};
use crate::wad::game::Game;
use crate::wad::lumps;
use crate::wad::map::Map;
use crate::wad::map::consts::{SKY_FLAT, is_sky_flat, normalized_picture};
use crate::wad::map::geometry;
use crate::wad::name::LumpName;

/// The `PLAYPAL` entry the scene shades with.
const PALETTE_NORMAL: usize = 0_usize;
/// `Doom`'s view height above the sector floor, in map units.
pub const EYE_HEIGHT: i32 = 41;
/// Pixels on a fallback image's edge.
const FALLBACK_SIDE: u32 = 64;
/// Pixels on one checkerboard cell's edge.
const FALLBACK_CELL: usize = 8;
/// Cells on one fallback edge.
const FALLBACK_CELLS: usize = 8;
/// The sky map a `Doom` episode selects, indexed by `episode - 1`.
const DOOM_SKIES: [u32; 4] = [1_u32, 2, 3, 4];
/// The sky map a `Heretic` episode selects, indexed by `episode - 1`.
const HERETIC_SKIES: [u32; 5] = [1_u32, 2, 3, 1, 3];
/// `MAP`-format numbers that still take the first sky.
const SKY_ONE_LAST_MAP: u32 = 11;
/// `MAP`-format numbers that still take the second sky.
const SKY_TWO_LAST_MAP: u32 = 20;

/// The `doom` episode table or the `Heretic` one, for `game`.
fn sky_table(game: Game) -> &'static [u32] {
    if game == Game::Heretic {
        &HERETIC_SKIES
    } else {
        &DOOM_SKIES
    }
}

/// Everything one 3D render reads from the session, prepared once.
///
/// Build one with [`build_scene`]; the images are palette-indexed, so a
/// renderer shades them through [`crate::render::light::shade`] with the
/// tables in [`Scene::colormaps`] and the palette. Every name key is the
/// lump-style uppercase name with surrounding spaces trimmed.
#[derive(Clone, Debug)]
pub struct Scene {
    /// The map's decoded lumps, as the merged view holds them.
    pub map: Map,
    /// `PLAYPAL` entry 0, the colours the indices refer to.
    pub palette: Palette,
    /// The 34 lighting tables: maps of [`TABLE_BYTES`] palette indices,
    /// from the merged `COLORMAP` or generated from the palette.
    pub colormaps: Vec<[u8; TABLE_BYTES]>,
    /// The composed image of every wall texture the map's sidedefs name,
    /// keyed by name. `"-"` means no texture and never appears.
    pub textures: BTreeMap<String, IndexedImage>,
    /// The image of every flat the map's sectors name, keyed by name.
    pub flats: BTreeMap<String, IndexedImage>,
    /// The sky image the map's name and the session's game select, drawn
    /// where a ceiling or floor carries [`Scene::sky_flat`]; the silent
    /// fallback for a map no plane of which asks for a sky.
    pub sky: IndexedImage,
    /// The flat name that means "sky": [`SKY_FLAT`].
    pub sky_flat: String,
    /// The settings this scene was built with.
    pub settings: RenderSettings,
    /// What could not be loaded, one line per item: missing textures,
    /// flats and sky (only where the map actually shows one), missing
    /// texture components, and a generated `COLORMAP`.
    pub problems: Vec<String>,
}

/// Prepare the [`Scene`] of map `map` from the session's merged view.
///
/// Textures come through the shared texture compositor
/// (`session/resolve/texture.rs`), flats through the flat resolver, and the
/// sky of a map that shows one through the texture definitions first and
/// the flat resolver after them; the palette from `PLAYPAL` entry 0 and
/// the lighting tables from the merged `COLORMAP` when it holds at least
/// 34 full tables, generated from the palette otherwise.
///
/// # Errors
///
/// [`CyberError::NotFound`] when no loaded archive holds a map block
/// named `map` or when the session has no `PLAYPAL` (rendering needs
/// one); the map loader's format errors for an undecodable block pass
/// through.
#[inline]
pub fn build_scene(
    session: &Session,
    map: LumpName,
    settings: &RenderSettings,
) -> Result<Scene, CyberError> {
    let model = load_map(session, map, Scope::Merged)?;
    let palette = load_palette(session, PALETTE_NORMAL)?;
    let mut problems = Vec::new();
    let lighting = scene_colormaps(session, &palette, &mut problems);
    let fallback = fallback_image(&palette)?;
    let textures = wall_textures(session, &model, &fallback, &mut problems);
    let flats = sector_flats(session, &model, &fallback, &mut problems);
    let sky = sky_image(session, &model, &fallback, &mut problems);
    Ok(Scene {
        map: model,
        palette,
        colormaps: lighting,
        textures,
        flats,
        sky,
        sky_flat: SKY_FLAT.to_owned(),
        settings: settings.clone(),
        problems,
    })
}

/// The sector containing the camera position `x`/`y`, `None` outside the
/// map.
#[must_use]
#[inline]
pub fn camera_sector(scene: &Scene, x: i64, y: i64) -> Option<usize> {
    geometry::point_in_sector(&scene.map, x, y)
}

/// The default eye height in sector `sector`: its floor plus `Doom`'s
/// [`EYE_HEIGHT`] view height. A sector index the map does not have
/// reads as floor `0`.
#[must_use]
#[inline]
pub fn default_eye_z(scene: &Scene, sector: usize) -> i32 {
    scene
        .map
        .sectors
        .get(sector)
        .map_or(0, |entry| entry.floor_height)
        .checked_add(EYE_HEIGHT)
        .unwrap_or(i32::MAX)
}

/// The scene's 34 lighting tables: the merged `COLORMAP` when it is at
/// least [`crate::gfx::colormap::MIN_BYTES`] long, generated from the
/// palette (and reported as a problem) otherwise.
fn scene_colormaps(
    session: &Session,
    palette: &Palette,
    problems: &mut Vec<String>,
) -> Vec<[u8; TABLE_BYTES]> {
    session
        .find_global(lumps::COLORMAP)
        .and_then(|entry| colormaps_from_bytes(&entry.lump.data))
        .unwrap_or_else(|| {
            problems.push("generated COLORMAP".to_owned());
            colormaps(palette)
        })
}

/// The composed image of every texture the map's sidedefs name, with a
/// fallback and a problem line for each one the session cannot load.
fn wall_textures(
    session: &Session,
    map: &Map,
    fallback: &IndexedImage,
    problems: &mut Vec<String>,
) -> BTreeMap<String, IndexedImage> {
    let mut textures = BTreeMap::new();
    for name in texture_names(map) {
        let loaded = LumpName::parse(&name)
            .ok()
            .and_then(|lump| load_texture(session, lump).ok());
        if let Some(composite) = loaded {
            problems.extend(composite.problems);
            textures.insert(name, composite.image);
        } else {
            problems.push(format!("missing texture {name}"));
            textures.insert(name, fallback.clone());
        }
    }
    textures
}

/// The image of every flat the map's sectors name, with a fallback and a
/// problem line for each one the session cannot load.
fn sector_flats(
    session: &Session,
    map: &Map,
    fallback: &IndexedImage,
    problems: &mut Vec<String>,
) -> BTreeMap<String, IndexedImage> {
    let mut flats = BTreeMap::new();
    for name in flat_names(map) {
        let image = load_flat_image(session, &name).unwrap_or_else(|| {
            problems.push(format!("missing flat {name}"));
            fallback.clone()
        });
        flats.insert(name, image);
    }
    flats
}

/// The sky image the map's name and the session's game select; a sky a map
/// shows and the session lacks falls back, with a problem line. A map no
/// sector of which names [`SKY_FLAT`] shows no sky at all, so its picture
/// is neither looked for nor missed: the silent fallback stands in for an
/// image the renderer never samples.
fn sky_image(
    session: &Session,
    map: &Map,
    fallback: &IndexedImage,
    problems: &mut Vec<String>,
) -> IndexedImage {
    if !shows_sky(map) {
        return fallback.clone();
    }
    let name = sky_name(session.game(), &map.name.to_string());
    load_sky_image(session, &name, problems).unwrap_or_else(|| {
        problems.push(format!("missing sky {name}"));
        fallback.clone()
    })
}

/// Whether any plane of `map` shows the sky: `Doom` draws the sky texture
/// on every ceiling *and* every floor named [`SKY_FLAT`].
fn shows_sky(map: &Map) -> bool {
    map.sectors
        .iter()
        .any(|sector| is_sky_flat(&sector.ceiling_flat) || is_sky_flat(&sector.floor_flat))
}

/// The picture a sky plane draws with: the texture definition named `name`
/// first, which is where a `Doom` `IWAD` keeps its skies (`SKY1` is a
/// `TEXTURE1` record over the `RSKY1` patch and no lump of the sky's own
/// name exists), then the flat of that name, the shape the import tools
/// and some ports write. A definition whose components the session lacks
/// reports them; `None` when it serves neither picture.
fn load_sky_image(
    session: &Session,
    name: &str,
    problems: &mut Vec<String>,
) -> Option<IndexedImage> {
    let lump = LumpName::parse(name).ok()?;
    match load_texture(session, lump) {
        Ok(composite) => {
            problems.extend(composite.problems);
            Some(composite.image)
        }
        Err(_) => load_flat(session, lump).ok(),
    }
}

/// The flat named `name`, `None` when no lump of that name decodes.
fn load_flat_image(session: &Session, name: &str) -> Option<IndexedImage> {
    let lump = LumpName::parse(name).ok()?;
    load_flat(session, lump).ok()
}

/// The texture names the sidedefs use, normalized and `"-"` excluded.
fn texture_names(map: &Map) -> BTreeSet<String> {
    map.sidedefs
        .iter()
        .flat_map(|side| [side.upper.clone(), side.lower.clone(), side.middle.clone()])
        .filter_map(|name| normalized_picture(&name))
        .collect()
}

/// The flat names the sectors use, normalized and `"-"` excluded.
fn flat_names(map: &Map) -> BTreeSet<String> {
    map.sectors
        .iter()
        .flat_map(|sector| [sector.floor_flat.clone(), sector.ceiling_flat.clone()])
        .filter_map(|name| normalized_picture(&name))
        .collect()
}

/// The sky texture name for map `map` of game `game`: `E#M#` episodes
/// index the `Doom` (or `Heretic`) table, `MAP##` numbers switch at 11
/// and 20, and anything else takes `SKY1`.
fn sky_name(game: Game, map: &str) -> String {
    format!("SKY{}", sky_index(game, map))
}

/// The sky number `map` selects in `game`.
fn sky_index(game: Game, map: &str) -> u32 {
    let upper = map.to_ascii_uppercase();
    if let Some(episode) = episode_of(&upper) {
        let table = sky_table(game);
        return episode
            .checked_sub(1)
            .and_then(|number| usize::try_from(number).ok())
            .and_then(|slot| table.get(slot).copied())
            .unwrap_or(1);
    }
    match map_number(&upper) {
        Some(number) if number <= SKY_ONE_LAST_MAP => 1,
        Some(number) if number <= SKY_TWO_LAST_MAP => 2,
        Some(_) => 3,
        None => 1,
    }
}

/// The episode number of an `E#M#` name, `None` for anything else.
fn episode_of(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    let (Some('E'), Some(episode), Some('M'), Some(map), None) = (
        chars.next(),
        chars.next(),
        chars.next(),
        chars.next(),
        chars.next(),
    ) else {
        return None;
    };
    if episode.is_ascii_digit() && map.is_ascii_digit() {
        episode.to_digit(10)
    } else {
        None
    }
}

/// The number of a `MAP##` name, `None` for anything else.
fn map_number(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("MAP")?;
    if digits.len() != 2 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok()
}

/// The 64x64 checkerboard that stands in for a missing picture: 8x8-pixel
/// cells alternating the palette indices of its brightest and darkest
/// colours, brightest at the top-left.
fn fallback_image(palette: &Palette) -> Result<IndexedImage, CyberError> {
    let light = extreme_index(palette, true);
    let dark = extreme_index(palette, false);
    let side = usize::try_from(FALLBACK_SIDE).unwrap_or(0);
    let mut indices = Vec::with_capacity(side.checked_mul(side).unwrap_or(0));
    for cell_row in 0..FALLBACK_CELLS {
        for _row in 0..FALLBACK_CELL {
            push_checker_row(cell_row, light, dark, &mut indices);
        }
    }
    let opaque = vec![true; indices.len()];
    IndexedImage::new(FALLBACK_SIDE, FALLBACK_SIDE, indices, opaque)
}

/// Appends one checkerboard row: alternating `light`/`dark` cells, the
/// parity of `cell_row` deciding which comes first.
fn push_checker_row(cell_row: usize, light: u8, dark: u8, row: &mut Vec<u8>) {
    for cell in 0..FALLBACK_CELLS {
        let parity = cell_row.checked_add(cell).unwrap_or(0);
        let index = if parity.checked_rem(2) == Some(0) {
            light
        } else {
            dark
        };
        row.extend(core::iter::repeat_n(index, FALLBACK_CELL));
    }
}

/// The index of the palette's brightest (`brightest` set) or darkest
/// colour, by [`luminance`]; the lowest index wins a tie.
fn extreme_index(palette: &Palette, brightest: bool) -> u8 {
    let mut best: Option<(u8, u8)> = None;
    for (position, color) in palette.colors().enumerate() {
        let Ok(index) = u8::try_from(position) else {
            continue;
        };
        let [red, green, blue] = *color;
        let tone = luminance([red, green, blue, 255_u8]);
        let better = best.is_none_or(|(best_tone, _)| {
            (brightest && tone > best_tone) || (!brightest && tone < best_tone)
        });
        if better {
            best = Some((tone, index));
        }
    }
    best.map_or(0, |(_, index)| index)
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use super::{
        FALLBACK_CELL, FALLBACK_CELLS, FALLBACK_SIDE, Scene, camera_sector, default_eye_z,
        extreme_index, fallback_image, flat_names, shows_sky, sky_name, texture_names,
    };
    use crate::gfx::image::IndexedImage;
    use crate::gfx::palette::Palette;
    use crate::render::RenderSettings;
    use crate::wad::game::Game;
    use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Vertex};
    use crate::wad::name::LumpName;

    /// The fixture palette: colour `i` is `(i, 255 - i, i)`, brightest at
    /// index 0 and darkest at 255.
    fn fixture_palette() -> Palette {
        let mut colors = [[0_u8; 3]; 256];
        for (index, color) in colors.iter_mut().enumerate() {
            let byte = u8::try_from(index).expect("the palette covers exactly the u8 range");
            *color = [byte, 255_u8.wrapping_sub(byte), byte];
        }
        Palette::from(colors)
    }

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A sidedef with the given names over `sector`.
    fn side(upper: &str, lower: &str, middle: &str, sector: usize) -> Sidedef {
        Sidedef {
            upper: upper.to_owned(),
            lower: lower.to_owned(),
            middle: middle.to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// A sector with the given floor flat and height.
    fn sector(floor_flat: &str, floor_height: i32) -> Sector {
        Sector {
            floor_flat: floor_flat.to_owned(),
            ceiling_flat: "-".to_owned(),
            floor_height,
            ..Default::default()
        }
    }

    /// A blocking one-sided line from `v1` to `v2` with sidedef `front`.
    fn line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-room map like the integration fixtures': a hall (sector 0,
    /// floor 0) and a raised room (sector 1, floor 32) split by the step
    /// line at `x = 256` inside `0..512 * 0..256`.
    fn two_room_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        let corners: [(i32, i32); 6] =
            [(0, 0), (256, 0), (512, 0), (512, 256), (256, 256), (0, 256)];
        map.vertices = corners
            .into_iter()
            .map(|(x, y)| Vertex {
                x,
                y,
                ..Default::default()
            })
            .collect();
        map.linedefs = vec![
            line(0, 5, 0),
            line(5, 4, 1),
            Linedef {
                v1: 4,
                v2: 1,
                flags: 4,
                front: Some(2),
                back: Some(3),
                ..Default::default()
            },
            line(1, 0, 4),
            line(4, 3, 5),
            line(3, 2, 6),
            line(2, 1, 7),
        ];
        map.sidedefs = vec![
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 0),
            side("-", "STEP1", "-", 0),
            side("-", "-", "-", 1),
            side("-", "-", "WALL1", 0),
            side("-", "-", "WALL1", 1),
            side("-", "-", "WALL1", 1),
            side("-", "-", "WALL1", 1),
        ];
        map.sectors = vec![sector("FLOOR1", 0), sector("FLOOR1", 32)];
        map
    }

    /// A scene over `map` with everything else empty.
    fn scene_with(map: Map) -> Scene {
        Scene {
            map,
            palette: fixture_palette(),
            colormaps: Vec::new(),
            textures: BTreeMap::new(),
            flats: BTreeMap::new(),
            sky: IndexedImage::new(1, 1, vec![0_u8], vec![true]).expect("a 1x1 image builds"),
            sky_flat: "F_SKY1".to_owned(),
            settings: RenderSettings::default(),
            problems: Vec::new(),
        }
    }

    /// Reads one pixel of a built fallback.
    fn pixel(image: &IndexedImage, x: u32, y: u32) -> (u8, bool) {
        image
            .get(x, y)
            .expect("the fallback covers its whole 64x64")
    }

    #[test]
    fn sky_names_follow_the_doom_episode_table() {
        assert_eq!(sky_name(Game::Doom, "E1M1"), "SKY1", "episode 1");
        assert_eq!(sky_name(Game::Doom, "E2M7"), "SKY2", "episode 2");
        assert_eq!(sky_name(Game::Doom, "E3M1"), "SKY3", "episode 3");
        assert_eq!(sky_name(Game::Doom, "E4M5"), "SKY4", "episode 4");
        assert_eq!(sky_name(Game::Doom, "E5M1"), "SKY1", "episode 5 falls back");
        assert_eq!(
            sky_name(Game::Doom2, "E1M1"),
            "SKY1",
            "a MAP-format session reading an episode name"
        );
    }

    #[test]
    fn sky_names_follow_the_heretic_episode_table() {
        assert_eq!(sky_name(Game::Heretic, "E1M1"), "SKY1");
        assert_eq!(sky_name(Game::Heretic, "E2M1"), "SKY2");
        assert_eq!(sky_name(Game::Heretic, "E3M1"), "SKY3");
        assert_eq!(
            sky_name(Game::Heretic, "E4M1"),
            "SKY1",
            "heretic's fourth episode"
        );
        assert_eq!(
            sky_name(Game::Heretic, "E5M1"),
            "SKY3",
            "the secret episode"
        );
        assert_eq!(sky_name(Game::Heretic, "E6M1"), "SKY1", "past the table");
    }

    #[test]
    fn sky_names_bucket_map_numbers_and_other_names() {
        assert_eq!(sky_name(Game::Doom2, "MAP05"), "SKY1");
        assert_eq!(sky_name(Game::Doom2, "MAP11"), "SKY1", "the boundary");
        assert_eq!(sky_name(Game::Doom2, "MAP15"), "SKY2");
        assert_eq!(sky_name(Game::Doom2, "MAP20"), "SKY2", "the boundary");
        assert_eq!(sky_name(Game::Doom2, "MAP25"), "SKY3");
        assert_eq!(sky_name(Game::Doom2, "map30"), "SKY3", "lowercase input");
        assert_eq!(sky_name(Game::Doom2, "TITLEMAP"), "SKY1", "not a map name");
        assert_eq!(sky_name(Game::Unknown, "E10M1"), "SKY1", "not E#M# shaped");
        assert_eq!(
            sky_name(Game::Unknown, "MAPX1"),
            "SKY1",
            "MAP plus a non-digit"
        );
        assert_eq!(
            sky_name(Game::Unknown, "MAP00"),
            "SKY1",
            "MAP00 is a number"
        );
    }

    #[test]
    fn the_fallback_is_a_two_colour_checkerboard() {
        let palette = fixture_palette();
        let image = fallback_image(&palette).expect("64x64 builds");
        assert_eq!((image.width, image.height), (FALLBACK_SIDE, FALLBACK_SIDE));
        let cell = u32::try_from(FALLBACK_CELL).expect("the cell fits a u32");
        let edge = FALLBACK_SIDE.checked_sub(1).expect("64 is positive");
        assert_eq!(pixel(&image, 0, 0), (0, true), "brightest at the top-left");
        assert_eq!(
            pixel(&image, cell, 0),
            (252, true),
            "the darkest entry is 252: 252..=255 all truncate to luminance 105 and the \
             lowest index wins the tie"
        );
        assert_eq!(
            pixel(&image, 0, cell),
            (252, true),
            "and so is the one below"
        );
        assert_eq!(
            pixel(&image, edge, edge),
            (0, true),
            "the last cell (7, 7) is even"
        );
        assert!(image.opaque.iter().all(|visible| *visible), "fully opaque");
        let side = usize::try_from(FALLBACK_SIDE).expect("the side fits");
        assert_eq!(
            side.checked_div(FALLBACK_CELL),
            Some(FALLBACK_CELLS),
            "the cells tile the side"
        );
    }

    #[test]
    fn the_extreme_indices_pick_by_luminance_first_wins() {
        let mut colors = [[0_u8; 3]; 256];
        for (index, color) in colors.iter_mut().enumerate() {
            let byte = u8::try_from(index).expect("the palette covers exactly the u8 range");
            *color = [byte, byte, byte];
        }
        let grey = Palette::from(colors);
        assert_eq!(extreme_index(&grey, true), 255, "white is brightest");
        assert_eq!(extreme_index(&grey, false), 0, "black is darkest");
        let equal = Palette::from([[7_u8, 7_u8, 7_u8]; 256]);
        assert_eq!(
            extreme_index(&equal, true),
            0,
            "a tie keeps the lowest index"
        );
        assert_eq!(extreme_index(&equal, false), 0, "either way");
    }

    #[test]
    fn names_are_uppercased_trimmed_and_dashes_dropped() {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.sidedefs = vec![
            side("-", "-", "wall1", 0),
            side("-", "STEP1", "-", 0),
            side(" ", "-", "WALL2 ", 0),
        ];
        map.sectors = vec![sector("floor1", 0), sector("-", 0)];
        let textures: Vec<String> = texture_names(&map).into_iter().collect();
        assert_eq!(textures, ["STEP1", "WALL1", "WALL2"], "keys normalize");
        let flats: Vec<String> = flat_names(&map).into_iter().collect();
        assert_eq!(flats, ["FLOOR1"], "dashes and empties vanish");
    }

    #[test]
    fn only_a_map_that_names_the_sky_flat_shows_sky() {
        let indoor = two_room_map();
        assert!(
            !shows_sky(&indoor),
            "the fixture map's planes are all plain flats"
        );
        let mut outdoors = indoor.clone();
        if let Some(room) = outdoors.sectors.get_mut(1) {
            room.ceiling_flat = "f_sky1".to_owned();
        }
        assert!(shows_sky(&outdoors), "a lowercase ceiling marker counts");
        let mut pit = indoor.clone();
        if let Some(room) = pit.sectors.get_mut(0) {
            room.floor_flat = "F_SKY1 ".to_owned();
        }
        assert!(shows_sky(&pit), "and so does a padded floor one");
        let mut blank = indoor;
        if let Some(room) = blank.sectors.get_mut(0) {
            room.floor_flat = String::new();
            room.ceiling_flat = "-".to_owned();
        }
        if let Some(room) = blank.sectors.get_mut(1) {
            room.floor_flat = String::new();
            room.ceiling_flat = "-".to_owned();
        }
        assert!(
            !shows_sky(&blank),
            "an empty plane and the none-marker are not the sky"
        );
    }

    #[test]
    fn the_camera_finds_its_sector_and_eye_height() {
        let scene = scene_with(two_room_map());
        assert_eq!(camera_sector(&scene, 64, 128), Some(0), "in the hall");
        assert_eq!(camera_sector(&scene, 384, 128), Some(1), "in the room");
        assert_eq!(camera_sector(&scene, 600, 128), None, "outside the map");
        assert_eq!(
            default_eye_z(&scene, 0),
            41_i32,
            "hall floor plus the view height"
        );
        assert_eq!(
            default_eye_z(&scene, 1),
            73_i32,
            "raised floor plus the view height"
        );
        assert_eq!(
            default_eye_z(&scene, 9),
            41_i32,
            "a sector the map lacks reads floor 0"
        );
    }

    #[test]
    fn the_two_room_fixture_uses_one_texture_and_one_flat() {
        let map = two_room_map();
        let textures: Vec<String> = texture_names(&map).into_iter().collect();
        assert_eq!(
            textures,
            ["STEP1", "WALL1"],
            "the lower step texture shows up"
        );
        let flats: Vec<String> = flat_names(&map).into_iter().collect();
        assert_eq!(flats, ["FLOOR1"], "both rooms share the flat");
    }
}
