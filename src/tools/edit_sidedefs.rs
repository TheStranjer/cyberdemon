//! Sidedef editing tools: `add_sidedef`, `edit_sidedef` and `remove_sidedef`
//! (see the `edit_sidedefs` router methods on [`crate::server::Cyberdemon`]).
//!
//! Every call is exactly one [`crate::session::Session::edit_map`] step, so
//! one undo entry moves the whole call. The parameter shapes
//! ([`IndexSelector`]) and validators ([`check_index`],
//! [`validate_texture_name`]) come from [`crate::tools::maps::params`];
//! removal runs through [`crate::wad::map::remove`] and refuses the
//! `conflict` of a referenced sidedef unless `cascade` is set.
//!
//! The geometry rule: the session treats the sidedef block as geometry, so
//! appending a sidedef, changing its sector or removing one reports
//! `nodes_invalidated: true` (the generated lumps are emptied and the `GL`
//! run dropped), while texture- and offset-only edits keep the nodes.
//! The binary formats store the offsets as `i16`, so values outside
//! `-32768..=32767` are refused as `invalid_argument` before anything is
//! written; `UDMF` stores full integers and has no such cap.

use alloc::collections::{BTreeMap, BTreeSet};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::Session;
use crate::tools::maps::common;
use crate::tools::maps::params::{
    IndexSelector, check_index, edit_result_fragment, validate_texture_name,
};
use crate::tools::{fail, ok};
use crate::wad::classify::MapFormat;
use crate::wad::map::Map;
use crate::wad::map::Sidedef;
use crate::wad::map::remove::{
    CascadeOptions, Removal, RemovalReport, apply_removal, dependents, expand_cascade,
};

/// The name of an empty texture field, `"-"` for no texture.
const NO_TEXTURE: &str = "-";
/// The lowest offset an `i16` binary `SIDEDEFS` column holds (`i16::MIN`).
const OFFSET_MIN: i32 = -0x8000_i32;
/// The highest offset an `i16` binary `SIDEDEFS` column holds (`i16::MAX`).
const OFFSET_MAX: i32 = 0x7FFF_i32;

/// Arguments for [`Cyberdemon::add_sidedef`].
#[derive(Debug, Deserialize, JsonSchema)]
struct AddSidedefParams {
    /// The map to edit. Defaults to the session's active map; with none
    /// active, the merged view's only map when it holds exactly one.
    #[serde(default)]
    map: Option<String>,
    /// Index of the sector this side opens into; it must exist.
    sector: usize,
    /// Upper texture name (default `"-"`, no texture).
    #[serde(default)]
    upper: Option<String>,
    /// Middle texture name (default `"-"`, no texture).
    #[serde(default)]
    middle: Option<String>,
    /// Lower texture name (default `"-"`, no texture).
    #[serde(default)]
    lower: Option<String>,
    /// Texture shift along the line in map units (default `0`).
    #[serde(default)]
    x_offset: Option<i32>,
    /// Texture shift across the line in map units (default `0`).
    #[serde(default)]
    y_offset: Option<i32>,
}

/// Arguments for [`Cyberdemon::edit_sidedef`].
#[derive(Debug, Deserialize, JsonSchema)]
struct EditSidedefParams {
    /// The map to edit; defaults like [`AddSidedefParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The sidedef to change: one index or a list of them (duplicates
    /// collapse); the shared changes below apply to every selected sidedef.
    index: IndexSelector,
    /// Replace the upper texture name.
    #[serde(default)]
    upper: Option<String>,
    /// Replace the middle texture name.
    #[serde(default)]
    middle: Option<String>,
    /// Replace the lower texture name.
    #[serde(default)]
    lower: Option<String>,
    /// Absolute texture shift along the line.
    #[serde(default)]
    x_offset: Option<i32>,
    /// Absolute texture shift across the line.
    #[serde(default)]
    y_offset: Option<i32>,
    /// Shift every selected sidedef's along-line offset by this much.
    #[serde(default)]
    x_offset_delta: Option<i32>,
    /// Shift every selected sidedef's across-line offset by this much.
    #[serde(default)]
    y_offset_delta: Option<i32>,
    /// Point this side at another sector; it must exist.
    #[serde(default)]
    sector: Option<usize>,
}

/// Arguments for [`Cyberdemon::remove_sidedef`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveSidedefParams {
    /// The map to edit; defaults like [`AddSidedefParams::map`].
    #[serde(default)]
    map: Option<String>,
    /// The sidedef to remove: one index or a list of them.
    index: IndexSelector,
    /// Remove the linedefs that use these sidedefs with them (default
    /// `false`: a referenced sidedef is refused with `conflict`).
    #[serde(default)]
    cascade: bool,
}

/// One sidedef's fields as the tools report them.
#[derive(Debug, Serialize)]
struct SidedefDetail {
    /// Index of the sector this side opens into.
    sector: usize,
    /// Upper texture name, `"-"` for none.
    upper: String,
    /// Middle texture name, `"-"` for none.
    middle: String,
    /// Lower texture name, `"-"` for none.
    lower: String,
    /// Texture shift along the line, in map units.
    x_offset: i32,
    /// Texture shift across the line, in map units.
    y_offset: i32,
}

/// One edited sidedef: its position plus its fields.
#[derive(Debug, Serialize)]
struct EditRow {
    /// Position in [`Map::sidedefs`].
    index: usize,
    /// The sidedef's fields, inlined next to the index.
    #[serde(flatten)]
    detail: SidedefDetail,
}

/// Structured result of [`Cyberdemon::add_sidedef`].
#[derive(Serialize)]
struct AddReport {
    /// Index of the created sidedef.
    index: usize,
    /// The created sidedef's fields.
    sidedef: SidedefDetail,
    /// The standard `map`/`nodes_invalidated`/`copied_to_working` fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::edit_sidedef`].
#[derive(Serialize)]
struct EditReport {
    /// The edited sidedefs, ascending by index.
    sidedefs: Vec<EditRow>,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// Structured result of [`Cyberdemon::remove_sidedef`].
#[derive(Serialize)]
struct RemoveToolReport {
    /// What the removal engine dropped, flipped and normalised.
    removed: RemovalReport,
    /// The standard edit fragment.
    #[serde(flatten)]
    edit: JsonMap<String, Value>,
}

/// The reported fields of `side`.
fn detail_of(side: &Sidedef) -> SidedefDetail {
    SidedefDetail {
        sector: side.sector,
        upper: side.upper.clone(),
        middle: side.middle.clone(),
        lower: side.lower.clone(),
        x_offset: side.x_offset,
        y_offset: side.y_offset,
    }
}

/// One texture parameter validated and uppercased, `NO_TEXTURE` when absent.
fn new_texture(given: Option<&String>, format: MapFormat) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(NO_TEXTURE.to_owned()),
        |name| validate_texture_name(name, format),
    )
}

/// The texture after an edit: the validated new name, or `current`.
fn edited_texture(
    current: &str,
    given: Option<&String>,
    format: MapFormat,
) -> Result<String, CyberError> {
    given.map_or_else(
        || Ok(current.to_owned()),
        |name| validate_texture_name(name, format),
    )
}

/// Refuses unless `value` sits inside the `i16` range of a binary
/// `SIDEDEFS` offset column.
fn fits_offset(name: &str, value: i32) -> Result<(), CyberError> {
    if (OFFSET_MIN..=OFFSET_MAX).contains(&value) {
        return Ok(());
    }
    Err(CyberError::InvalidArgument(format!(
        "{name} {value} is out of range for the binary map format: the field holds \
         {OFFSET_MIN}..={OFFSET_MAX}"
    )))
}

/// Range-checks `side`'s offsets against the binary `SIDEDEFS` record; a
/// `UDMF` map stores full integers and needs no check.
fn check_binary_offsets(format: MapFormat, side: &Sidedef) -> Result<(), CyberError> {
    match format {
        MapFormat::Doom | MapFormat::Hexen => {
            fits_offset("x_offset", side.x_offset)?;
            fits_offset("y_offset", side.y_offset)?;
        }
        MapFormat::Udmf => {}
    }
    Ok(())
}

/// The offset after the edit: `absolute` wins, `delta` shifts `current`,
/// neither keeps it.
fn new_offset(
    name: &str,
    current: i32,
    absolute: Option<i32>,
    delta: Option<i32>,
) -> Result<i32, CyberError> {
    if let Some(value) = absolute {
        return Ok(value);
    }
    delta.map_or(Ok(current), |shift| {
        current.checked_add(shift).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "{name} {current} plus {shift} leaves the range of texture offsets"
            ))
        })
    })
}

/// The sidedef the `add_sidedef` parameters describe for a map of `format`.
fn new_sidedef(args: &AddSidedefParams, format: MapFormat) -> Result<Sidedef, CyberError> {
    Ok(Sidedef {
        x_offset: args.x_offset.unwrap_or_default(),
        y_offset: args.y_offset.unwrap_or_default(),
        upper: new_texture(args.upper.as_ref(), format)?,
        lower: new_texture(args.lower.as_ref(), format)?,
        middle: new_texture(args.middle.as_ref(), format)?,
        sector: args.sector,
        extra: BTreeMap::new(),
    })
}

/// The sidedef `current` becomes under the fields `args` gives; unasked
/// fields and the `UDMF` extras stand.
fn edited_sidedef(
    current: &Sidedef,
    args: &EditSidedefParams,
    format: MapFormat,
) -> Result<Sidedef, CyberError> {
    Ok(Sidedef {
        x_offset: new_offset(
            "x_offset",
            current.x_offset,
            args.x_offset,
            args.x_offset_delta,
        )?,
        y_offset: new_offset(
            "y_offset",
            current.y_offset,
            args.y_offset,
            args.y_offset_delta,
        )?,
        upper: edited_texture(&current.upper, args.upper.as_ref(), format)?,
        lower: edited_texture(&current.lower, args.lower.as_ref(), format)?,
        middle: edited_texture(&current.middle, args.middle.as_ref(), format)?,
        sector: args.sector.unwrap_or(current.sector),
        extra: current.extra.clone(),
    })
}

/// `add_sidedef`'s step: check the sector, append the described sidedef and
/// report its row.
fn add_in_place(map: &mut Map, args: &AddSidedefParams) -> Result<EditRow, CyberError> {
    check_index("sector", args.sector, map.sectors.len())?;
    let side = new_sidedef(args, map.format)?;
    check_binary_offsets(map.format, &side)?;
    let index = map.sidedefs.len();
    map.sidedefs.push(side.clone());
    Ok(EditRow {
        index,
        detail: detail_of(&side),
    })
}

/// Refuses an absolute/delta pair carried together on one axis.
fn check_conflicts(args: &EditSidedefParams) -> Result<(), CyberError> {
    let moves = [
        (args.x_offset, args.x_offset_delta),
        (args.y_offset, args.y_offset_delta),
    ];
    if moves
        .into_iter()
        .any(|(absolute, delta)| absolute.is_some() && delta.is_some())
    {
        return Err(CyberError::InvalidArgument(
            "an absolute offset conflicts with its delta: pass one of x_offset/\
             x_offset_delta or y_offset/y_offset_delta"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Whether the request changes at least one field.
#[must_use]
const fn has_change(args: &EditSidedefParams) -> bool {
    args.upper.is_some()
        || args.middle.is_some()
        || args.lower.is_some()
        || args.sector.is_some()
        || args.x_offset.is_some()
        || args.y_offset.is_some()
        || args.x_offset_delta.is_some()
        || args.y_offset_delta.is_some()
}

/// Validates an `edit_sidedef` request up front and returns its sorted,
/// deduplicated target indices.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty index list, an
/// absolute/delta conflict or a request that changes nothing.
fn validate_edit(args: &EditSidedefParams) -> Result<Vec<usize>, CyberError> {
    let indices = args.index.clone().indices()?;
    check_conflicts(args)?;
    if !has_change(args) {
        return Err(CyberError::InvalidArgument(
            "nothing to change: pass at least one of upper, middle, lower, x_offset, \
             x_offset_delta, y_offset, y_offset_delta, sector"
                .to_owned(),
        ));
    }
    Ok(indices)
}

/// `edit_sidedef`'s step: rewrite every selected sidedef and report the rows.
fn edit_in_place(
    map: &mut Map,
    args: &EditSidedefParams,
    indices: &[usize],
) -> Result<Vec<EditRow>, CyberError> {
    for index in indices {
        check_index("sidedef", *index, map.sidedefs.len())?;
    }
    if let Some(sector) = args.sector {
        check_index("sector", sector, map.sectors.len())?;
    }
    let format = map.format;
    let mut rows = Vec::new();
    for (index, slot) in map.sidedefs.iter_mut().enumerate() {
        if !indices.contains(&index) {
            continue;
        }
        let updated = edited_sidedef(slot, args, format)?;
        check_binary_offsets(format, &updated)?;
        rows.push(EditRow {
            index,
            detail: detail_of(&updated),
        });
        *slot = updated;
    }
    Ok(rows)
}

/// The `conflict` naming every linedef that still uses a selected sidedef.
fn dependent_refusal(uses: &BTreeMap<usize, Vec<usize>>) -> CyberError {
    let parts: Vec<String> = uses
        .iter()
        .map(|(side, lines)| format!("sidedef {side} is used by linedef(s) {}", join(lines)))
        .collect();
    CyberError::Conflict(format!(
        "{}; pass cascade: true to remove the referencing linedefs with them",
        parts.join("; ")
    ))
}

/// The indices as one comma-separated list.
fn join(indices: &[usize]) -> String {
    indices
        .iter()
        .map(usize::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}

/// The seed removal of exactly the selected sidedefs.
fn sidedef_removal(indices: &[usize]) -> Removal {
    Removal {
        sidedefs: indices.iter().copied().collect::<BTreeSet<usize>>(),
        ..Default::default()
    }
}

/// `remove_sidedef`'s step: check the indices, refuse a referenced sidedef
/// without `cascade`, then run the removal (grown by the cascade when set).
fn remove_in_place(
    map: &mut Map,
    args: &RemoveSidedefParams,
    indices: &[usize],
) -> Result<RemovalReport, CyberError> {
    for index in indices {
        check_index("sidedef", *index, map.sidedefs.len())?;
    }
    let removal = sidedef_removal(indices);
    if !args.cascade {
        let uses = dependents(map, &removal).linedefs_using_sidedefs;
        if !uses.is_empty() {
            return Err(dependent_refusal(&uses));
        }
        return apply_removal(map, &removal);
    }
    let grown = expand_cascade(map, &removal, CascadeOptions::default());
    apply_removal(map, &grown)
}

/// `add_sidedef`'s work: resolve the map, run the one-step append.
fn add_run(session: &mut Session, args: &AddSidedefParams) -> Result<AddReport, CyberError> {
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("add sidedef for sector {}", args.sector);
    let (row, report) = session.edit_map(name, &label, |map| add_in_place(map, args))?;
    Ok(AddReport {
        index: row.index,
        sidedef: row.detail,
        edit: edit_result_fragment(&report),
    })
}

/// `edit_sidedef`'s work: validate, resolve the map, run the one-step edit.
fn edit_run(session: &mut Session, args: &EditSidedefParams) -> Result<EditReport, CyberError> {
    let indices = validate_edit(args)?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("edit {} sidedefs", indices.len());
    let (rows, report) =
        session.edit_map(name, &label, |map| edit_in_place(map, args, &indices))?;
    Ok(EditReport {
        sidedefs: rows,
        edit: edit_result_fragment(&report),
    })
}

/// `remove_sidedef`'s work: resolve the map and run the removal.
fn remove_run(
    session: &mut Session,
    args: &RemoveSidedefParams,
) -> Result<RemoveToolReport, CyberError> {
    let indices = args.index.clone().indices()?;
    let name = common::resolve_map_name(session, args.map.as_deref())?;
    let label = format!("remove {} sidedefs", indices.len());
    let (removed, report) =
        session.edit_map(name, &label, |map| remove_in_place(map, args, &indices))?;
    Ok(RemoveToolReport {
        removed,
        edit: edit_result_fragment(&report),
    })
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
#[tool_router(router = edit_sidedefs_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Appends one sidedef to a map in a single undoable edit.
    #[tool(
        description = "Adds one sidedef (one side of a linedef: its face textures, offsets and the sector it opens into) to a map in a single undoable edit; sector must name an existing sector and the new side starts unreferenced until a linedef points at it. Prefer create_sector_from_loop to draw walls together with their sides, or build_map_from_plan for a whole map."
    )]
    fn add_sidedef(
        &self,
        Parameters(args): Parameters<AddSidedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(self.state().with_session(|session| add_run(session, &args)))
    }

    /// Changes one or several sidedefs in a single undoable edit.
    #[tool(
        description = "Changes one or several sidedefs of a map in a single undoable edit: upper/middle/lower texture names, x_offset and y_offset (absolute or delta) and the sector the side faces. Prefer bulk_edit to apply texture or offset changes across a whole find_elements selection, and replace_texture to swap one texture name for another everywhere; use this for one side's exact fields."
    )]
    fn edit_sidedef(
        &self,
        Parameters(args): Parameters<EditSidedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| edit_run(session, &args)),
        )
    }

    /// Removes one or several sidedefs in a single undoable edit.
    #[tool(
        description = "Removes one sidedef or a list of them from a map in a single undoable edit; the remaining sidedefs renumber down. A side a linedef still uses is refused with conflict unless cascade: true, which flips lines that lose their front, makes lines that lose their back one-sided and removes lines that lose both sides. Geometry change - rebuild nodes; for mixed selections prefer delete_geometry."
    )]
    fn remove_sidedef(
        &self,
        Parameters(args): Parameters<RemoveSidedefParams>,
    ) -> Result<CallToolResult, ErrorData> {
        reported(
            self.state()
                .with_session(|session| remove_run(session, &args)),
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::{
        AddSidedefParams, EditSidedefParams, OFFSET_MAX, OFFSET_MIN, add_in_place,
        check_binary_offsets, dependent_refusal, detail_of, edit_in_place, edited_sidedef,
        edited_texture, fits_offset, has_change, new_offset, new_sidedef, new_texture,
        remove_in_place, validate_edit,
    };
    use crate::error::CyberError;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{Linedef, Map, Sector, Sidedef, Vertex};
    use crate::wad::name::LumpName;
    use alloc::collections::BTreeMap;

    /// The error code of `error`, for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// An add request parsed from `json`.
    fn add_args(json: &str) -> AddSidedefParams {
        from_str(json).expect("test add requests parse")
    }

    /// An edit request parsed from `json`.
    fn edit_args(json: &str) -> EditSidedefParams {
        from_str(json).expect("test edit requests parse")
    }

    /// A remove request parsed from `json`.
    fn remove_args(json: &str) -> super::RemoveSidedefParams {
        from_str(json).expect("test remove requests parse")
    }

    /// A sidedef with the given texture names over `sector`.
    fn side(upper: &str, middle: &str, lower: &str, sector: usize) -> Sidedef {
        Sidedef {
            upper: upper.to_owned(),
            middle: middle.to_owned(),
            lower: lower.to_owned(),
            sector,
            ..Default::default()
        }
    }

    /// A vertex at `x`/`y`.
    fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            ..Default::default()
        }
    }

    /// A one-sided line from `v1` to `v2` over front sidedef `front`.
    fn solid_line(v1: usize, v2: usize, front: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            front: Some(front),
            ..Default::default()
        }
    }

    /// A two-sided line from `v1` to `v2` over sidedefs `front`/`back`.
    fn open_line(v1: usize, v2: usize, front: usize, back: usize) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 4,
            front: Some(front),
            back: Some(back),
            ..Default::default()
        }
    }

    /// The shared test map: two rooms split by the two-sided line `1`
    /// (sidedefs 1/2), an outer wall on line `0` (sidedef 0) and the
    /// unreferenced sidedef `3`.
    fn test_map() -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("a valid name"),
            format: MapFormat::Doom,
            vertices: vec![vertex(0, 0), vertex(128, 0), vertex(128, 128)],
            linedefs: vec![solid_line(0, 1, 0), open_line(1, 2, 1, 2)],
            sidedefs: vec![
                side("-", "WALL1", "-", 0),
                side("-", "STEP1", "-", 0),
                side("-", "-", "-", 1),
                side("-", "FREE1", "-", 1),
            ],
            sectors: vec![Sector::default(), Sector::default()],
            ..Default::default()
        }
    }

    #[test]
    fn new_sidedefs_take_the_dash_and_zero_defaults() {
        let args = add_args(r#"{"sector": 2}"#);
        let side = new_sidedef(&args, MapFormat::Doom).expect("defaults build");
        assert_eq!(side.sector, 2, "the required sector lands");
        assert_eq!(side.upper, "-", "the three texture fields default to -");
        assert_eq!(side.middle, "-");
        assert_eq!(side.lower, "-");
        assert_eq!(
            (side.x_offset, side.y_offset),
            (0_i32, 0_i32),
            "and the offsets to 0"
        );
        let named = add_args(
            r#"{"sector": 0, "upper": "brick", "middle": "-", "lower": "STARTAN3",
                "x_offset": 8, "y_offset": -16}"#,
        );
        let dressed = new_sidedef(&named, MapFormat::Doom).expect("a full request builds");
        assert_eq!(dressed.upper, "BRICK", "names are uppercased");
        assert_eq!(dressed.lower, "STARTAN3");
        assert_eq!(
            (dressed.x_offset, dressed.y_offset),
            (8_i32, -16_i32),
            "shifts land"
        );
    }

    #[test]
    fn texture_parameters_validate_or_default() {
        assert_eq!(
            new_texture(None, MapFormat::Doom).expect("absent is the dash"),
            "-"
        );
        assert_eq!(
            new_texture(Some(&"wall1".to_owned()), MapFormat::Doom).expect("a vanilla name"),
            "WALL1"
        );
        let long = new_texture(Some(&"123456789".to_owned()), MapFormat::Doom)
            .expect_err("nine characters refuse on binary");
        assert_eq!(code(&long), "invalid_argument", "over-long code");
        assert!(
            new_texture(Some(&"123456789".to_owned()), MapFormat::Udmf).is_ok(),
            "UDMF takes 64"
        );
        assert_eq!(
            edited_texture("OLD", None, MapFormat::Doom).expect("absent keeps the current"),
            "OLD"
        );
        assert_eq!(
            edited_texture("OLD", Some(&"new".to_owned()), MapFormat::Doom).expect("a new name"),
            "NEW",
            "replacement wins"
        );
    }

    #[test]
    fn binary_offsets_must_fit_i16() {
        let edge = side("-", "-", "-", 0);
        assert!(fits_offset("x_offset", OFFSET_MAX).is_ok(), "i16::MAX fits");
        assert!(fits_offset("y_offset", OFFSET_MIN).is_ok(), "i16::MIN fits");
        let over = Sidedef {
            x_offset: OFFSET_MAX.saturating_add(1),
            ..edge
        };
        let error = check_binary_offsets(MapFormat::Doom, &over).expect_err("above i16 refuses");
        assert_eq!(code(&error), "invalid_argument", "range code");
        assert!(
            error.to_string().contains("-32768..=32767"),
            "the message shows the range: {error}"
        );
        assert!(
            check_binary_offsets(MapFormat::Udmf, &over).is_ok(),
            "UDMF stores full integers"
        );
        assert!(
            check_binary_offsets(MapFormat::Hexen, &over).is_err(),
            "binary Hexen refuses too"
        );
    }

    #[test]
    fn offsets_shift_by_delta_and_absolute_wins() {
        assert_eq!(
            new_offset("x_offset", 8_i32, Some(-5_i32), None).expect("absolute"),
            -5_i32
        );
        assert_eq!(
            new_offset("y_offset", 8_i32, None, Some(16_i32)).expect("delta shifts"),
            24_i32
        );
        assert_eq!(
            new_offset("x_offset", 8_i32, None, None).expect("neither keeps"),
            8_i32
        );
        let overflow =
            new_offset("x_offset", i32::MAX, None, Some(1_i32)).expect_err("above i32 refuses");
        assert_eq!(code(&overflow), "invalid_argument", "overflow code");
        assert!(
            overflow.to_string().contains("x_offset"),
            "the field is named: {overflow}"
        );
    }

    #[test]
    fn edited_sidedefs_combine_unchanged_fields_with_the_request() {
        let current = side("UP1", "MID1", "LOW1", 0);
        let args =
            edit_args(r#"{"index": 1, "middle": "NEW1", "y_offset_delta": -4, "sector": 1}"#);
        let edited = edited_sidedef(
            &Sidedef {
                y_offset: 8_i32,
                ..current.clone()
            },
            &args,
            MapFormat::Doom,
        )
        .expect("the edit computes");
        assert_eq!(edited.middle, "NEW1", "the asked field changed");
        assert_eq!(edited.upper, "UP1", "the unasked ones stand");
        assert_eq!(edited.lower, "LOW1");
        assert_eq!(edited.y_offset, 4_i32, "the delta shifted it");
        assert_eq!(edited.x_offset, 0_i32, "and the untouched offset holds");
        assert_eq!(edited.sector, 1, "the sector moved");
        assert_eq!(
            detail_of(&current).upper,
            "UP1",
            "the detail mirrors the row"
        );
    }

    #[test]
    fn edit_validation_names_conflicts_and_nothings() {
        let sorted = validate_edit(&edit_args(r#"{"index": [3, 1], "x_offset_delta": 8}"#))
            .expect("a list with deltas is fine");
        assert_eq!(sorted, vec![1_usize, 3], "sorted and deduplicated");
        let refusals = [
            r#"{"index": 0, "x_offset": 1, "x_offset_delta": 2}"#,
            r#"{"index": 0, "y_offset": 1, "y_offset_delta": 2}"#,
            r#"{"index": 0}"#,
            r#"{"index": []}"#,
        ];
        for json in refusals {
            let error = validate_edit(&edit_args(json)).expect_err("the request must refuse");
            assert_eq!(code(&error), "invalid_argument", "refusal code for {json}");
        }
        assert!(
            has_change(&edit_args(r#"{"index": 0, "sector": 0}"#)),
            "an explicit sector counts as a change"
        );
    }

    #[test]
    fn adding_checks_the_sector_and_appends() {
        let mut map = test_map();
        let missing = add_args(r#"{"sector": 9}"#);
        let error = add_in_place(&mut map, &missing).expect_err("sector 9 is not there");
        assert_eq!(code(&error), "not_found", "missing sector code");
        assert!(
            error.to_string().contains("map has 2 sectors"),
            "the message names the count: {error}"
        );
        assert_eq!(map.sidedefs.len(), 4, "and nothing appended");
        let args = add_args(r#"{"sector": 1, "middle": "brick01", "x_offset": 40000}"#);
        let binary = add_in_place(&mut map, &args).expect_err("the i16 offset refuses");
        assert_eq!(code(&binary), "invalid_argument", "range code");
        assert_eq!(map.sidedefs.len(), 4, "the refusal appended nothing");
        map.format = MapFormat::Udmf;
        let row = add_in_place(&mut map, &args).expect("UDMF stores the wide offset");
        assert_eq!(row.index, 4, "after the existing four");
        assert_eq!(row.detail.x_offset, 40_000_i32, "and reports it");
        assert_eq!(map.sidedefs.len(), 5, "the list grew by one");
    }

    #[test]
    fn editing_touches_only_the_selection() {
        let mut map = test_map();
        let args = edit_args(r#"{"index": [0, 2], "x_offset": 32768}"#);
        let error = edit_in_place(&mut map, &args, &[0_usize, 2])
            .expect_err("the absolute offset refuses on binary");
        assert_eq!(code(&error), "invalid_argument", "range code");
        let ok = edit_in_place(
            &mut map,
            &edit_args(r#"{"index": [2], "upper": "TOP"}"#),
            &[2],
        )
        .expect("sidedef 2 gets an upper texture");
        assert_eq!(ok.len(), 1, "one row reported");
        assert_eq!(ok[0].index, 2, "for the selection");
        assert_eq!(
            map.sidedefs.get(2).expect("sidedef 2").upper,
            "TOP",
            "stored"
        );
        assert_eq!(
            map.sidedefs.get(1).expect("sidedef 1").upper,
            "-",
            "neighbours stand"
        );
        let past = edit_in_place(&mut map, &edit_args(r#"{"index": 9, "upper": "X"}"#), &[9]);
        assert_eq!(
            code(&past.expect_err("sidedef 9 is not there")),
            "not_found",
            "a missing index refuses"
        );
        let sectorless = edit_in_place(&mut map, &edit_args(r#"{"index": 2, "sector": 9}"#), &[2]);
        assert_eq!(
            code(&sectorless.expect_err("sector 9 is not there")),
            "not_found",
            "a missing sector refuses"
        );
    }

    #[test]
    fn removal_refuses_referenced_sides_and_cuts_orphans() {
        let mut map = test_map();
        let used = remove_in_place(&mut map, &remove_args(r#"{"index": 1}"#), &[1]);
        let error = used.expect_err("sidedef 1 is a linedef's front");
        assert_eq!(code(&error), "conflict", "referenced without cascade");
        assert!(
            error
                .to_string()
                .contains("sidedef 1 is used by linedef(s) 1")
                && error.to_string().contains("cascade"),
            "the message names the user and the fix: {error}"
        );
        assert_eq!(map.sidedefs.len(), 4, "the conflict changed nothing");
        let report = remove_in_place(&mut map, &remove_args(r#"{"index": 3}"#), &[3])
            .expect("the orphan goes without a cascade");
        assert_eq!(report.sidedefs, vec![3_usize], "the removed original index");
        assert!(report.flipped_linedefs.is_empty(), "no line was touched");
        assert_eq!(map.sidedefs.len(), 3, "and the list shrank");
    }

    #[test]
    fn a_cascaded_removal_flips_the_line_that_lost_its_front() {
        let mut map = test_map();
        let report = remove_in_place(
            &mut map,
            &remove_args(r#"{"index": 1, "cascade": true}"#),
            &[1],
        )
        .expect("the cascade lifts the conflict");
        assert_eq!(report.sidedefs, vec![1_usize], "the seeded side goes");
        assert_eq!(report.flipped_linedefs, vec![1_usize], "line 1 flipped");
        assert_eq!(report.one_sided_linedefs, vec![1_usize], "and closed");
        let flipped = map.linedefs.get(1).expect("line 1 survives");
        assert_eq!((flipped.v1, flipped.v2), (2, 1), "the ends swapped");
        assert_eq!(flipped.front, Some(1), "old sidedef 2 renumbered to 1");
        assert_eq!(flipped.back, None, "there is no back any more");
    }

    #[test]
    fn the_conflict_message_joins_every_user() {
        let uses: BTreeMap<usize, Vec<usize>> = [(1_usize, vec![1_usize, 4]), (2_usize, vec![1])]
            .into_iter()
            .collect();
        let message = dependent_refusal(&uses).to_string();
        assert!(
            message.contains("sidedef 1 is used by linedef(s) 1, 4")
                && message.contains("sidedef 2 is used by linedef(s) 1"),
            "both sides are listed with their lines: {message}"
        );
    }
}
