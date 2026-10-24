//! The inline side changes of `edit_linedef`: the `front_side`/`back_side`
//! parameter, the sidedef it rewrites and the side the call points at.
//!
//! [`SideEdit`] arrives inside [`crate::tools::edit_linedefs::edit::EditLinedefParams`],
//! its own [`SideEdit::has_fields`] deciding whether a request asks for a
//! change at all. [`side_edited`] computes the sidedef a side edit asks for,
//! keeping the sector and the `UDMF` extras of the side standing, and
//! [`apply_side_edit`] writes it back through the [`SideTarget`]
//! [`side_target`] builds, refusing the [`no_side`] of a linedef without the
//! side it names.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::wad::classify::MapFormat;
use crate::wad::map::Sidedef;

use super::{check_binary_offsets, validate_texture_name};

/// Texture and offset changes for the sidedef on one side of a linedef;
/// fields left out stay as they are.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct SideEdit {
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
}

impl SideEdit {
    /// Whether this edit asks for at least one field.
    pub(super) fn has_fields(&self) -> bool {
        let textures = [
            self.upper.as_ref(),
            self.middle.as_ref(),
            self.lower.as_ref(),
        ]
        .iter()
        .any(Option::is_some);
        let offsets = [self.x_offset, self.y_offset].iter().any(Option::is_some);
        textures || offsets
    }
}

/// One side of an [`Cyberdemon::edit_linedef`]: the linedef it belongs to,
/// the side reference to edit and the changes asked for.
pub(super) struct SideTarget<'req> {
    /// Index of the linedef whose side this is (for messages).
    line: usize,
    /// Which side, for messages (`"front"` or `"back"`).
    which: &'static str,
    /// The side's sidedef index after the edit, `None` when absent.
    side: Option<usize>,
    /// The requested field changes, `None` when this side was not named.
    edit: Option<&'req SideEdit>,
}

/// The texture after a side edit: the validated new name, or `current`.
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

/// The sidedef `current` becomes under the side edit: its textures and
/// offsets change, its sector and `UDMF` extras stand.
pub(super) fn side_edited(
    current: &Sidedef,
    edit: &SideEdit,
    format: MapFormat,
) -> Result<Sidedef, CyberError> {
    Ok(Sidedef {
        x_offset: edit.x_offset.unwrap_or(current.x_offset),
        y_offset: edit.y_offset.unwrap_or(current.y_offset),
        upper: edited_texture(&current.upper, edit.upper.as_ref(), format)?,
        lower: edited_texture(&current.lower, edit.lower.as_ref(), format)?,
        middle: edited_texture(&current.middle, edit.middle.as_ref(), format)?,
        sector: current.sector,
        extra: current.extra.clone(),
    })
}

/// The refusal when a side edit names a side the linedef does not have.
fn no_side(index: usize, which: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "linedef {index} has no {which} side to edit: create one (add_linedef with a back side, \
         or edit_linedef with back) or drop the {which}_side parameter"
    ))
}

/// Applies one side edit to the sidedef `target` names, refusing when the
/// side is absent. `sides` is [`Map::sidedefs`] on its own so the linedef
/// iteration can hold its own field borrow.
pub(super) fn apply_side_edit(
    sides: &mut [Sidedef],
    target: &SideTarget<'_>,
    format: MapFormat,
) -> Result<(), CyberError> {
    let Some(edit) = target.edit.filter(|side| side.has_fields()) else {
        return Ok(());
    };
    let Some(side) = target.side else {
        return Err(no_side(target.line, target.which));
    };
    let slot = sides
        .get_mut(side)
        .ok_or_else(|| CyberError::NotFound(format!("sidedef {side} does not exist")))?;
    let updated = side_edited(slot, edit, format)?;
    check_binary_offsets(format, &updated)?;
    *slot = updated;
    Ok(())
}

/// The one side-reference edit for linedef `index`, resolved against the
/// updated line.
pub(super) const fn side_target<'req>(
    index: usize,
    which: &'static str,
    side: Option<usize>,
    edit: Option<&'req SideEdit>,
) -> SideTarget<'req> {
    SideTarget {
        line: index,
        which,
        side,
        edit,
    }
}
