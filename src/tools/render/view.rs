//! The `render_view` tool: the camera and setting overrides resolved onto
//! a copy of the session's [`RenderSettings`], the [`crate::render::Scene`]
//! built under the session lock with the map's things dressed as
//! [`SpriteItem`]s through [`SpriteWiring`], and the integer caster's
//! frame gamma-corrected and published as a `PNG`.

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::server::AssetRef;
use crate::defs::{Definitions, ThingDef};
use crate::error::CyberError;
use crate::fixed::{Angle, FRACBITS};
use crate::gfx::codec::encode_png;
use crate::gfx::patch::Patch;
use crate::gfx::sprite_rotation::rotation_for;
use crate::render::sprites::SpriteItem;
use crate::render::view::ONE_16;
use crate::render::{
    RenderSettings, ViewCamera, ViewFrame, build_scene_with, camera_sector, check_pitch,
    default_eye_z, gamma_table, render_frame,
};
use crate::session::Session;
use crate::state::AppState;
use crate::tools::graphics::resolve::{PictureIndex, load_sprite_cached};
use crate::tools::maps::common;
use crate::wad::LumpName;
use crate::wad::map::geometry;
use crate::wad::map::{Map, Thing};

use super::PNG_MIME;

/// Name suffix of the published 3D view files.
const VIEW_SUFFIX: &str = "_view.png";

/// Arguments for [`Cyberdemon::render_view`]. `x` and `y` are required;
/// every other field is optional, defaulting to the camera floor, level
/// east, or the session's render settings.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct RenderViewParams {
    /// The map to render, e.g. `"MAP01"`. Defaults to the session's active
    /// map; with none active, the merged view's only map.
    #[serde(default)]
    map: Option<String>,
    /// Camera `x` in map units.
    x: i32,
    /// Camera `y` in map units.
    y: i32,
    /// Eye height in map units (default: the camera sector's floor plus
    /// `Doom`'s 41-unit view height).
    #[serde(default)]
    z: Option<i32>,
    /// Facing in degrees, counter-clockwise from east, 0 = east, 90 =
    /// north (default `0`).
    #[serde(default)]
    angle: Option<i32>,
    /// Pitch in degrees, positive looking up, `-60..=60` (default `0`).
    #[serde(default)]
    pitch: Option<i32>,
    /// Horizontal field of view in degrees, `10..=170` (default: the
    /// render setting).
    #[serde(default)]
    fov: Option<u32>,
    /// Output width in pixels, `16..=4096` (default: the render setting).
    #[serde(default)]
    width: Option<u32>,
    /// Output height in pixels, `16..=4096` (default: the render setting).
    #[serde(default)]
    height: Option<u32>,
    /// Lighting model for this call only: `"doom"`, `"flat"` or
    /// `"fullbright"` (default: the render setting).
    #[serde(default)]
    lighting: Option<String>,
}

/// The render settings one `render_view` call draws with: the session's,
/// with the `lighting`, `fov` and resolution overrides validated on top.
///
/// # Errors
///
/// [`check_pitch`] and [`RenderSettings`]' setter errors for a refused
/// override; the session settings are never touched.
fn view_settings(session: &Session, args: &RenderViewParams) -> Result<RenderSettings, CyberError> {
    check_pitch(args.pitch.unwrap_or(0_i32))?;
    let mut settings = session.render_settings().clone();
    if let Some(text) = args.lighting.as_deref() {
        settings.set_lighting(text)?;
    }
    if let Some(value) = args.fov {
        settings.set_fov(value)?;
    }
    settings.set_resolution(
        args.width.unwrap_or(settings.width),
        args.height.unwrap_or(settings.height),
    )?;
    Ok(settings)
}

/// The raw 16.16 depth a sprite may not stand nearer to the camera than:
/// nearer than four map units, and its projection would tower over (or
/// miss) the frame absurdly, so the thing is skipped.
pub(super) const SPRITE_MIN_TZ16: i128 = 4_i128.saturating_mul(ONE_16);

/// A thing's placement, in the units the sprite pass reads.
#[derive(Clone, Copy, Debug)]
pub(super) struct SpriteSpot {
    /// Thing `x`, in map units.
    pub(super) x: i64,
    /// Thing `y`, in map units.
    pub(super) y: i64,
    /// Spawn height offset, in map units.
    pub(super) z: i32,
    /// Facing in degrees.
    pub(super) angle: i32,
}

impl SpriteSpot {
    /// The spot `mark` stands on.
    fn of(mark: &Thing) -> Self {
        Self {
            x: i64::from(mark.x),
            y: i64::from(mark.y),
            z: mark.z,
            angle: mark.angle,
        }
    }
}

/// The session half of one `render_view` sprite pass: the definitions
/// that dress the things, the camera basis they project through, the
/// per-render patch cache and everything the walk has produced. One of
/// these lives exactly one render, which is what keeps the decode-each-
/// lump-once cache local to it.
pub(super) struct SpriteWiring<'defs> {
    /// The session's thing definitions.
    pub(super) defs: &'defs Definitions,
    /// Camera `x`, in map units.
    pub(super) cam_x: i64,
    /// Camera `y`, in map units.
    pub(super) cam_y: i64,
    /// The camera forward `x` (its angle's cosine), raw 16.16.
    pub(super) forward_x: i128,
    /// The camera forward `y` (its angle's sine), raw 16.16.
    pub(super) forward_y: i128,
    /// Sprite lump patches decoded this render, by lump name.
    pub(super) cache: HashMap<LumpName, Patch>,
    /// Sprite names a `problems` line has already reported.
    pub(super) reported: HashSet<String>,
    /// Problem lines for sprite names that would not load.
    pub(super) problems: Vec<String>,
    /// The prepared items, in thing order.
    pub(super) items: Vec<SpriteItem>,
}

impl<'defs> SpriteWiring<'defs> {
    /// The wiring for `camera`: `defs` dress the map's things, and the
    /// camera's position and facing project and rotate them.
    pub(super) fn new(defs: &'defs Definitions, camera: &ViewCamera) -> Self {
        let facing = Angle::from_degrees(i64::from(camera.angle));
        Self {
            defs,
            cam_x: i64::from(camera.x),
            cam_y: i64::from(camera.y),
            forward_x: i128::from(facing.cos().0),
            forward_y: i128::from(facing.sin().0),
            cache: HashMap::new(),
            reported: HashSet::new(),
            problems: Vec::new(),
            items: Vec::new(),
        }
    }

    /// The definition that sprites `mark`; a thing the definitions do
    /// not know, or whose def carries no sprite, reads as `None`.
    pub(super) fn definition(&self, mark: &Thing) -> Option<&'defs ThingDef> {
        let defs = self.defs;
        u32::try_from(mark.kind)
            .ok()
            .and_then(|kind| defs.thing(kind))
            .filter(|def| !def.sprite.is_empty())
    }

    /// `spot`'s camera-relative offsets: `dot(rel, right)` and
    /// `dot(rel, forward)` in raw 16.16 map units, forward `(cos, sin)`
    /// and right `(sin, -cos)`, the forward one positive in front of the
    /// camera.
    pub(super) fn project(&self, spot: &SpriteSpot) -> (i128, i128) {
        let rel_x = i128::from(spot.x.saturating_sub(self.cam_x)).saturating_mul(ONE_16);
        let rel_y = i128::from(spot.y.saturating_sub(self.cam_y)).saturating_mul(ONE_16);
        let right16 = rel_x
            .saturating_mul(self.forward_y)
            .saturating_sub(rel_y.saturating_mul(self.forward_x))
            .checked_shr(FRACBITS)
            .unwrap_or(0_i128);
        let depth16 = rel_x
            .saturating_mul(self.forward_x)
            .saturating_add(rel_y.saturating_mul(self.forward_y))
            .checked_shr(FRACBITS)
            .unwrap_or(0_i128);
        (right16, depth16)
    }

    /// The sprite patch at the spot's bucketed rotation, decoded once per
    /// lump for the whole render; a sprite name that will not load
    /// reports itself once and dresses nothing.
    fn patch(
        &mut self,
        index: &PictureIndex<'_>,
        def: &ThingDef,
        spot: &SpriteSpot,
    ) -> Option<Patch> {
        let rotation = rotation_for((self.cam_x, self.cam_y), (spot.x, spot.y), spot.angle);
        match load_sprite_cached(index, def, rotation, &mut self.cache) {
            Ok(sprite) => Some(sprite.patch),
            Err(error) => {
                let missing = error.is_not_found();
                self.report(&def.sprite, missing);
                None
            }
        }
    }

    /// One problem line per sprite name: `missing` words a lump the
    /// session serves no picture for; the alternative words a picture
    /// that is no readable image.
    pub(super) fn report(&mut self, name: &str, missing: bool) {
        if !self.reported.insert(name.to_owned()) {
            return;
        }
        let word = if missing { "missing" } else { "unreadable" };
        self.problems.push(format!("{word} sprite {name}"));
    }

    /// Adds `mark` to the render when the definitions dress it, it
    /// stands at least [`SPRITE_MIN_TZ16`] ahead of the camera inside a
    /// sector, and its sprite loads; the sector's light shades the whole
    /// sprite, standing things anchor on the sector's floor plus the
    /// spawn `z` and hanging ones under the ceiling by the patch height.
    fn add(&mut self, index: &PictureIndex<'_>, map: &Map, mark: &Thing) {
        let Some(def) = self.definition(mark) else {
            return;
        };
        let spot = SpriteSpot::of(mark);
        let (right16, depth16) = self.project(&spot);
        if depth16 < SPRITE_MIN_TZ16 {
            return;
        }
        let Some(sector) = geometry::point_in_sector(map, spot.x, spot.y) else {
            return;
        };
        let Some(row) = map.sectors.get(sector) else {
            return;
        };
        let Some(patch) = self.patch(index, def, &spot) else {
            return;
        };
        let base = if def.hanging {
            i64::from(row.ceiling_height).saturating_sub(i64::from(patch.image.height))
        } else {
            i64::from(row.floor_height).saturating_add(i64::from(spot.z))
        };
        let left_offset16 = i128::from(patch.left_offset).saturating_mul(ONE_16);
        let top_offset16 = i128::from(patch.top_offset).saturating_mul(ONE_16);
        self.items.push(SpriteItem {
            image: patch.image,
            left_offset16,
            top_offset16,
            x: spot.x,
            y: spot.y,
            base,
            light: row.light,
            tx16: right16,
            tz16: depth16,
        });
    }
}

/// The session half of one `render_view` sprite pass: the map's things
/// dressed as sprite items for the transparent pass, and the problem
/// lines naming the sprite names the session could not dress with. The
/// whole pass reads through the render's one [`PictureIndex`], the same
/// the scene was built with.
fn view_sprites(
    index: &PictureIndex<'_>,
    scene: &crate::render::Scene,
    defs: &Definitions,
    camera: &ViewCamera,
) -> (Vec<SpriteItem>, Vec<String>) {
    let mut wiring = SpriteWiring::new(defs, camera);
    for mark in &scene.map.things {
        wiring.add(index, &scene.map, mark);
    }
    (wiring.items, wiring.problems)
}

/// The session half of one `render_view` call: the scene, the placed
/// camera, the sprite items and their problem lines, all read under the
/// session lock.
struct PreparedView {
    /// The prepared scene (its settings carry the resolved overrides).
    scene: crate::render::Scene,
    /// The sector index holding the camera.
    sector: usize,
    /// The effective camera: resolved `z`, defaulted `angle` and
    /// `pitch`, the settings' `fov`.
    camera: ViewCamera,
    /// The sprite items for the transparent pass.
    sprites: Vec<SpriteItem>,
    /// Problem lines for sprite names the session could not load.
    sprite_problems: Vec<String>,
}

/// Reads the view's inputs under the session lock: the map, the settings
/// with overrides, the scene, the camera placed in a sector, and the
/// things dressed as sprites while the settings ask for them.
///
/// # Errors
///
/// [`common::resolve_map_name`]'s errors, [`view_settings`]' refusals,
/// [`build_scene`]'s [`CyberError::NotFound`] for a missing `PLAYPAL`,
/// and [`CyberError::InvalidArgument`] when the camera lies outside every
/// sector of the map.
fn load_view(state: &AppState, args: &RenderViewParams) -> Result<PreparedView, CyberError> {
    state.with_session(|session| {
        let name = common::resolve_map_name(session, args.map.as_deref())?;
        let settings = view_settings(session, args)?;
        let index = PictureIndex::new(session)?;
        let scene = build_scene_with(&index, name, &settings)?;
        let sector = camera_sector(&scene, i64::from(args.x), i64::from(args.y)).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "camera is outside the map: ({}, {}) lies in no sector of {}; move x/y inside a \
                 sector (render_topdown shows the layout)",
                args.x, args.y, scene.map.name
            ))
        })?;
        let z = args.z.unwrap_or_else(|| default_eye_z(&scene, sector));
        let camera = ViewCamera {
            x: args.x,
            y: args.y,
            z,
            angle: args.angle.unwrap_or(0_i32),
            pitch: args.pitch.unwrap_or(0_i32),
            fov: settings.fov,
        };
        let (sprites, sprite_problems) = if settings.show_sprites.is_on() {
            let defs = session.definitions()?;
            view_sprites(&index, &scene, &defs, &camera)
        } else {
            (Vec::new(), Vec::new())
        };
        Ok(PreparedView {
            scene,
            sector,
            camera,
            sprites,
            sprite_problems,
        })
    })
}

/// [`Cyberdemon::render_view`]'s result: the effective camera, the frame
/// size, the transparent pass's counts, the scene's and sprite pass's
/// problems and the published `PNG` asset.
#[derive(Serialize)]
pub(super) struct ViewReport {
    /// The rendered map's name.
    map: String,
    /// The camera the frame was cast from.
    camera: CameraReport,
    /// Output width in pixels.
    width: u32,
    /// Output height in pixels.
    height: u32,
    /// Sprites the transparent pass painted at least one pixel of; `0`
    /// when it drew none, which is also what a turned-off `show_sprites`
    /// setting reports.
    sprites_drawn: u32,
    /// Screen columns the transparent pass painted at least one masked
    /// pixel in; a column carrying several masked items counts once.
    /// `0` when it drew nothing.
    masked_columns: usize,
    /// What the scene and the sprite pass could not load, one line per
    /// item.
    problems: Vec<String>,
    /// The published view.
    asset: AssetRef,
}

/// The `camera` object of a [`ViewReport`].
#[derive(Serialize)]
struct CameraReport {
    /// Camera `x` in map units.
    x: i32,
    /// Camera `y` in map units.
    y: i32,
    /// Eye height in map units.
    z: i32,
    /// Facing in degrees, counter-clockwise from east.
    angle: i32,
    /// Pitch in degrees, positive looking up.
    pitch: i32,
    /// Horizontal field of view in degrees.
    fov: u32,
    /// The index of the sector holding the camera.
    sector: usize,
}

/// `render_view`'s work: resolve, dress the things, cast, gamma, encode,
/// publish.
///
/// # Errors
///
/// [`load_view`]'s errors, [`encode_png`]' and [`AppState::publish`]'
/// failures.
pub(super) fn view_report(
    state: &AppState,
    args: &RenderViewParams,
) -> Result<ViewReport, CyberError> {
    let prepared = load_view(state, args)?;
    let width = prepared.scene.settings.width;
    let height = prepared.scene.settings.height;
    let gamma = prepared.scene.settings.gamma;
    let camera = prepared.camera;
    let mut rendered = render_frame(
        &prepared.scene,
        &camera,
        ViewFrame {
            width,
            height,
            sprites: prepared.sprites,
        },
    );
    let masked_columns = rendered.masked_columns;
    let sprites_drawn = u32::try_from(rendered.sprites_drawn).unwrap_or(u32::MAX);
    rendered.canvas.apply_gamma(&gamma_table(gamma));
    let png = encode_png(&rendered.canvas.to_rgba_image(), None)?;
    let asset = state.publish(
        &format!("{}{VIEW_SUFFIX}", prepared.scene.map.name),
        PNG_MIME,
        png,
    )?;
    let mut problems = prepared.scene.problems;
    problems.extend(prepared.sprite_problems);
    Ok(ViewReport {
        map: prepared.scene.map.name.to_string(),
        camera: CameraReport {
            x: camera.x,
            y: camera.y,
            z: camera.z,
            angle: camera.angle,
            pitch: camera.pitch,
            fov: camera.fov,
            sector: prepared.sector,
        },
        width,
        height,
        sprites_drawn,
        masked_columns,
        problems,
        asset,
    })
}
