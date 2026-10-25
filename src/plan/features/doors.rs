//! The door step of the pipeline: wire every placed door connector in
//! connection order. A fresh [`next_free_tag`] is stamped on the band
//! sector ([`crate::plan::model::DoorStyle::Remote`] included: the
//! remote's sector tag is its handle), non-remote styles write their
//! [`door_specials`] action onto the band's room-facing lines with
//! [`face_room_line`], turned so each room's sidedef faces the geometric
//! front, and a locked band's jamb faces are re-dressed in the coloured
//! track [`FeatureDress`] names for the style's
//! [`track_role_name`](WadDoorStyle::track_role_name); plain styles keep
//! what
//! [`apply_textures`](crate::plan::textures::apply_textures) dressed
//! there.
//!
//! [`door_specials`]: crate::wad::map::features::door_specials
//! [`face_room_line`]: crate::wad::map::features::face_room_line
//! [`next_free_tag`]: crate::wad::map::features::next_free_tag

use crate::error::CyberError;
use crate::plan::emit::{CompiledPlan, ConnectorInfo};
use crate::plan::model::ConnectionKind;
use crate::wad::map::Map;
use crate::wad::map::features::DoorStyle as WadDoorStyle;
use crate::wad::map::features::{door_specials, face_room_line, next_free_tag};

use super::{FeatureDress, FeatureRow, FeatureRun, wad_style};

/// The door step: wire every placed door connector, in connection
/// order.
pub(super) fn doors(
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for info in &compiled.connector_sectors {
        wire_door(&mut compiled.map, info, dress, run)?;
    }
    Ok(())
}

/// Install one door connector: stamp a fresh tag on its band sector,
/// give the band's room-facing lines the style's action, dress a locked
/// band's jamb faces in their coloured track, and record the row and
/// the tag under the connector id. A [`crate::plan::model::DoorStyle::Remote`] band keeps
/// its lines bare — the tagged sector is the handle a remote trigger
/// (the switch step's panel, or the caller's own line) fires on.
///
/// A band a later connector painted over owns no sector: there is
/// nothing to tag and no line to face, so the door reports no row.
///
/// # Errors
///
/// [`wire_door`]'s and [`apply_features`](super::apply_features)'s `Internal` refusal for a
/// band sector the map does not hold.
fn wire_door(
    map: &mut Map,
    info: &ConnectorInfo,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    let ConnectionKind::Door { style } = info.kind else {
        return Ok(());
    };
    let Some(band) = info.sectors.first().copied() else {
        return Ok(());
    };
    let core = wad_style(style);
    let tag = next_free_tag(map);
    stamp_band_tag(map, band, tag, info)?;
    if core != WadDoorStyle::Remote {
        let setup = door_specials(map.flag_family(), core, tag);
        for &line in &info.door_lines {
            face_room_line(map, line, band, &setup);
        }
    }
    dress_locked_jambs(map, band, core, dress);
    run.tags.insert(info.id.clone(), tag);
    run.bands.insert(info.id.clone(), band);
    run.rows.push(FeatureRow {
        kind: "door",
        room: None,
        sectors: vec![band],
        lines: info.door_lines.clone(),
        tag: Some(tag),
        line_ends: Vec::new(),
    });
    Ok(())
}

/// Stamp `tag` on the band sector `band`, refusing bookkeeping the map
/// does not back: [`emit`](crate::plan::emit::emit) allocated every
/// recorded sector, so a missing row means `compiled` did not come from
/// this plan.
fn stamp_band_tag(
    map: &mut Map,
    band: usize,
    tag: i32,
    info: &ConnectorInfo,
) -> Result<(), CyberError> {
    match map.sectors.get_mut(band) {
        Some(space) => {
            space.tag = tag;
            Ok(())
        }
        None => Err(CyberError::Internal(format!(
            "door connector '{}' names band sector {band}, which the compiled map does \
             not hold; apply features to the plan's own compiled result",
            info.id
        ))),
    }
}

/// Re-dress a locked door band's jamb faces — the one-sided lines whose
/// front faces the band sector — in the coloured track the style's role
/// names. Plain and remote styles keep what the compiler dressed, and
/// so does a locked style whose colour the theme left unnamed.
fn dress_locked_jambs(map: &mut Map, band: usize, style: WadDoorStyle, dress: &FeatureDress) {
    let locked = matches!(
        style,
        WadDoorStyle::LockedRed | WadDoorStyle::LockedBlue | WadDoorStyle::LockedYellow
    );
    if !locked {
        return;
    }
    let Some(track) = dress.texture(style.track_role_name()) else {
        return;
    };
    paint_band_jambs(map, band, track);
}

/// Give every one-sided line whose front faces sector `band` the `track`
/// texture on its `middle` face.
fn paint_band_jambs(map: &mut Map, band: usize, track: &str) {
    let faces: Vec<usize> = map
        .linedefs
        .iter()
        .filter(|line| line.back.is_none())
        .filter_map(|line| line.front)
        .filter(|&side| map.sidedefs.get(side).is_some_and(|row| row.sector == band))
        .collect();
    for side in faces {
        if let Some(row) = map.sidedefs.get_mut(side) {
            track.clone_into(&mut row.middle);
        }
    }
}
