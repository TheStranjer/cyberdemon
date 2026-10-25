//! Sector editing tools: `add_sector`, `edit_sector`, `remove_sector`,
//! `create_sector_from_loop` and `carve_void` (see the `edit_sectors`
//! router methods on [`crate::server::Cyberdemon`]).
//!
//! This file keeps what the five tools share: the constants, the reported
//! [`SectorDetail`], the checks every resulting sector faces, the outcome
//! wrapper and the router. `add_sector`, `edit_sector` and `remove_sector`
//! live in [`edit`], their wire parameters, report rows and one-step work
//! together; `create_sector_from_loop` and `carve_void` live in [`draw`],
//! their wire parameters beside the validated requests they build.
//!
//! Every call is exactly one [`crate::session::Session::edit_map`] step, so
//! one undo entry moves the whole call. The parameter shapes
//! ([`IndexSelector`]) and validators ([`check_index`],
//! [`validate_texture_name`]) come from [`crate::tools::maps::params`];
//! removal runs through [`crate::wad::map::remove`] and refuses the
//! `conflict` of a sector that sidedefs still use unless `cascade` is set.
//! `create_sector_from_loop` and `carve_void` wrap the pure overlay
//! engine [`crate::wad::map::overlay::apply_overlay`]: [`draw`] turns
//! the wire parameters into a validated
//! [`crate::wad::map::overlay::OverlayRequest`]
//! (sector fields checked like [`crate::session::Session::edit_map`] hands
//! in, textures and coordinates checked for the map's format, the outer
//! rule parsed) and reports the engine's
//! [`crate::wad::map::overlay::OverlayResult`]
//! with the standard edit fragment.
//!
//! The geometry rule: the sector count is part of the geometry signature,
//! so appending or removing sectors reports `nodes_invalidated: true` (the
//! generated lumps are emptied and the `GL` run dropped), while property
//! edits of existing sectors (heights, flats, light, special, tag) keep the
//! nodes. The binary formats store the heights and the light as `i16` and
//! `special`/`tag` as `u16`, so values outside those ranges are refused as
//! `invalid_argument` before anything is written; `UDMF` stores full
//! integers and has no such caps. The light level stays inside the vanilla
//! `0..=255` in every format. A flat may not be `"-"`: that names no
//! texture, not a floor or ceiling surface.
//!
//! [`IndexSelector`]: crate::tools::maps::params::IndexSelector
//! [`check_index`]: crate::tools::maps::params::check_index

mod draw;
mod edit;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use serde::Serialize;

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::tools::edit_sectors::draw::{
    CarveVoidParams, CreateSectorFromLoopParams, carve_run, create_run,
};
use crate::tools::edit_sectors::edit::{
    AddSectorParams, EditSectorParams, RemoveSectorParams, add_run, edit_run, remove_run,
};
use crate::tools::maps::params::validate_texture_name;
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::Sector;
use crate::wad::map::consts::NO_TEXTURE;

/// The `add_sector` default floor height, in map units.
const DEFAULT_FLOOR_HEIGHT: i32 = 0_i32;
/// The `add_sector` default ceiling height, in map units.
const DEFAULT_CEILING_HEIGHT: i32 = 128_i32;
/// The `add_sector` default floor flat name.
const DEFAULT_FLOOR_FLAT: &str = "FLOOR4_8";
/// The `add_sector` default ceiling flat name.
const DEFAULT_CEILING_FLAT: &str = "CEIL3_5";
/// The `add_sector` default light level.
const DEFAULT_LIGHT: i32 = 160_i32;
/// The lowest light level the vanilla range allows.
const LIGHT_MIN: i32 = 0_i32;
/// The highest light level the vanilla range allows (`u8::MAX`).
const LIGHT_MAX: i32 = 255_i32;
/// The lowest height an `i16` binary `SECTORS` column holds (`i16::MIN`).
const HEIGHT_MIN: i32 = -0x8000_i32;
/// The highest height an `i16` binary `SECTORS` column holds (`i16::MAX`).
const HEIGHT_MAX: i32 = 0x7FFF_i32;
/// The lowest value a `u16` binary `SECTORS` column holds.
const FIELD_MIN: i32 = 0_i32;
/// The highest value a `u16` binary `SECTORS` column holds (`u16::MAX`).
const FIELD_MAX: i32 = 0xFFFF_i32;
/// The `create_sector_from_loop` default middle texture of a new wall.
const DEFAULT_WALL_TEXTURE: &str = "STARTAN3";
/// The `outer_sector` mode that probes the sector outside each new edge.
const OUTER_AUTO: &str = "auto";
/// The `outer_sector` mode that keeps every new wall one-sided.
const OUTER_NONE: &str = "none";
/// The lowest coordinate an `i16` binary `VERTEXES` column holds (`i16::MIN`).
const COORD_MIN: i32 = -0x8000_i32;
/// The highest coordinate an `i16` binary `VERTEXES` column holds (`i16::MAX`).
const COORD_MAX: i32 = 0x7FFF_i32;

/// One sector's fields as the tools report them.
#[derive(Debug, Serialize)]
struct SectorDetail {
    /// Floor height in map units.
    floor_height: i32,
    /// Ceiling height in map units.
    ceiling_height: i32,
    /// Floor flat name.
    floor_flat: String,
    /// Ceiling flat name.
    ceiling_flat: String,
    /// Light level, `0..=255`.
    light: i32,
    /// Sector effect number.
    special: i32,
    /// Tag matched by linedef actions.
    tag: i32,
}

/// The reported fields of `sector`.
fn detail_of(sector: &Sector) -> SectorDetail {
    SectorDetail {
        floor_height: sector.floor_height,
        ceiling_height: sector.ceiling_height,
        floor_flat: sector.floor_flat.clone(),
        ceiling_flat: sector.ceiling_flat.clone(),
        light: sector.light,
        special: sector.special,
        tag: sector.tag,
    }
}

/// A flat name validated for `format`; `"-"` names no texture and is
/// never a flat.
fn validate_flat(name: &str, format: MapFormat) -> Result<String, CyberError> {
    let checked = validate_texture_name(name, format)?;
    if checked == NO_TEXTURE {
        return Err(CyberError::InvalidArgument(
            "a flat cannot be \"-\": pass a flat lump name such as FLOOR4_8".to_owned(),
        ));
    }
    Ok(checked)
}

/// The flat the `add_sector` parameters ask for, `fallback` when absent.
fn flat_or_default(
    given: Option<&String>,
    fallback: &str,
    format: MapFormat,
) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(fallback.to_owned()),
        |name| validate_flat(name, format),
    )
}

/// The flat after an edit: the validated new name, or `current`.
fn edited_flat(
    current: &str,
    given: Option<&String>,
    format: MapFormat,
) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(current.to_owned()),
        |name| validate_flat(name, format),
    )
}

/// Refuses unless `value` sits inside the binary record field's
/// `min..=max` range.
fn fits_field(name: &str, value: i32, min: i32, max: i32) -> Result<(), CyberError> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{name} {value} is out of range for the binary map format: the field holds \
         {min}..={max}"
    )))
}

/// Range-checks `sector` against the binary `SECTORS` record: heights are
/// `i16` columns and `special`/`tag` are `u16` ones; a `UDMF` map stores
/// full integers and needs no check.
fn check_binary_ranges(format: MapFormat, sector: &Sector) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom | MapFormat::Hexen => {
            fits_field("floor_height", sector.floor_height, HEIGHT_MIN, HEIGHT_MAX)?;
            fits_field(
                "ceiling_height",
                sector.ceiling_height,
                HEIGHT_MIN,
                HEIGHT_MAX,
            )?;
            fits_field("special", sector.special, FIELD_MIN, FIELD_MAX)?;
            fits_field("tag", sector.tag, FIELD_MIN, FIELD_MAX)?;
        }
        MapFormat::Udmf => {}
    }
    Ok(())
}

/// Refuses a light level outside the vanilla `0..=255`, every format alike.
fn check_light(light: i32) -> Result<(), CyberError> {
    if (LIGHT_MIN..=LIGHT_MAX).contains(&light) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "light {light} is out of range: the light level holds {LIGHT_MIN}..={LIGHT_MAX}"
    )))
}

/// Refuses a sector whose ceiling ends below its floor; equal heights are
/// allowed, the closed shell of a raised door.
fn check_height_order(sector: &Sector) -> Result<(), CyberError> {
    if sector.ceiling_height >= sector.floor_height {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "ceiling_height {} is below floor_height {}: keep the ceiling at or above \
         the floor (equal is a closed door)",
        sector.ceiling_height, sector.floor_height
    )))
}

/// Everything one resulting sector must satisfy before anything is written.
fn check_sector(format: MapFormat, sector: &Sector) -> Result<(), CyberError> {
    check_light(sector.light)?;
    check_binary_ranges(format, sector)?;
    check_height_order(sector)
}

/// Hand an outcome to the client as the documented structured result.
fn reported<T>(outcome: Result<T, CyberError>) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    match outcome {
        Ok(value) => ok(&value),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = edit_sectors_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Appends one sector to a map in a single undoable edit.
    #[tool(
        description = "Adds one sector (a region sharing floor, ceiling, flats, light and effect) to a map in a single undoable edit with vanilla-flavoured defaults; the appended record stays dead space until sidedefs reference it. Prefer create_sector_from_loop to grow a whole room with its walls in one call, or build_map_from_plan for a finished map; pair this with add_sidedef only for exact low-level control."
    )]
    fn add_sector(
        &self,
        Parameters(args): Parameters<AddSectorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Changes one or several sectors in a single undoable edit.
    #[tool(
        description = "Changes one or several sectors of a map in a single undoable edit: floor_height/ceiling_height or deltas, floor_flat/ceiling_flat, light, special and tag, every change applied to each selected index; a ceiling below its floor is refused. Prefer mark_secret and set_light_effect for those specials, and bulk_edit plus replace_texture for sweeping values or flats across many sectors by query; undo steps it all back."
    )]
    fn edit_sector(
        &self,
        Parameters(args): Parameters<EditSectorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| edit_run(session, &args)),
        )
    }

    /// Removes one or several sectors in a single undoable edit.
    #[tool(
        description = "Removes one sector or a list of them from a map in a single undoable edit; the remaining sectors renumber and surviving sidedef references follow. Without cascade a sector a sidedef still uses is refused with conflict naming them; cascade: true takes the room with it - its sides go, lines flip or close, orphans prune. Sector count changes invalidate nodes: rebuild before vanilla play."
    )]
    fn remove_sector(
        &self,
        Parameters(args): Parameters<RemoveSectorParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }

    /// Draws a closed corner loop as one new sector with its walls.
    #[tool(
        description = "Draws a closed loop of corners as one new sector with its boundary walls, in a single undoable edit: the draw-a-room tool. points: [{x, y}, ...] (at least 3, implicitly closed) or vertices: [i, ...]; sector properties as in add_sector. The geometry engine splits existing walls at T-junctions and crossings and merges painted-over sides, so the result never crosses itself. Prefer build_map_from_plan for a whole fresh level; use this to add rooms to a map."
    )]
    fn create_sector_from_loop(
        &self,
        Parameters(args): Parameters<CreateSectorFromLoopParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| create_run(session, &args)),
        )
    }

    /// Carves a polygon of solid rock into the map.
    #[tool(
        description = "Carves a closed polygon of solid rock out of a map in a single undoable edit: the area holds no sector and the surrounding sectors get one-sided walls along the cut. The ring comes as points: [{x, y}, ...] or existing vertices: [i, ...]; zero-area or self-intersecting rings are refused by the geometry engine. Use it to pull a boundary in or delete leftover space; create_sector_from_loop does the opposite."
    )]
    fn carve_void(
        &self,
        Parameters(args): Parameters<CarveVoidParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| carve_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests;
