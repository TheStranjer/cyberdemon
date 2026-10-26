//! `list_sprites`: the sprite region grouped by sprite, frame and rotation.
//!
//! [`list_sprites_run`] walks the merged view's sprites region (again with
//! [`last_wins`] and [`Page::take`]) and folds every grammar-matching name
//! into [`Groups`] with [`collect_sprite`], which records a pair name's
//! second rotation as a mirrored entry through [`push_rotation`]; a name
//! the grammar rejects is kept by [`shows_invalid`] when the caller's
//! prefix covers it. [`sprite_rows`] orders the accumulated map into the
//! report's rows. [`parse_prefix`] validates the caller's four characters
//! through the grammar's own [`crate::gfx::sprite_name::parse_sprite_prefix`],
//! which `get_sprite` shares through [`parse_sprite`].

use alloc::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::gfx::sprite_name::{SpriteName, parse_sprite_prefix};
use crate::session::Session;
use crate::tools::Page;
use crate::tools::graphics::{in_namespace, last_wins, merged_rows};
use crate::wad::classify::Namespace;

/// Arguments for [`Cyberdemon::list_sprites`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ListSpritesParams {
    /// Four-character sprite prefix to list, e.g. `"POSS"` (case-
    /// insensitive); without it every sprite of the session is grouped.
    #[serde(default)]
    pub(super) prefix: Option<String>,
    /// Matching sprites to skip before the page (default 0).
    #[serde(default)]
    pub(super) offset: Option<usize>,
    /// Maximum sprite groups in the result (default 200, at most 2000).
    #[serde(default)]
    pub(super) limit: Option<usize>,
}

/// One `list_sprites` rotation entry: which lump serves this frame and
/// rotation, and whether the image is flipped to do it.
#[derive(Debug, Serialize)]
pub(super) struct RotationRow {
    /// Rotation `0`-`8` this entry serves.
    pub(super) rotation: u8,
    /// The lump providing the image.
    pub(super) lump: String,
    /// Whether the engine must flip the image horizontally for this
    /// rotation.
    pub(super) mirrored: bool,
}

/// One frame of a sprite: a frame letter and the rotations it serves.
#[derive(Debug, Serialize)]
pub(super) struct FrameRow {
    /// The frame letter.
    pub(super) frame: char,
    /// The rotations this frame serves, by number then lump name.
    pub(super) rotations: Vec<RotationRow>,
}

/// One sprite group: a four-character prefix and its frames.
#[derive(Debug, Serialize)]
pub(super) struct SpriteRow {
    /// The four-character sprite prefix.
    pub(super) sprite: String,
    /// The frames, in letter order.
    pub(super) frames: Vec<FrameRow>,
}

/// Structured result of [`Cyberdemon::list_sprites`].
#[derive(Debug, Serialize)]
pub(super) struct SpritesReport {
    /// Sprite groups matching `prefix` (before paging).
    pub(super) total: usize,
    /// The matching sprite groups, by name, on this page.
    pub(super) sprites: Vec<SpriteRow>,
    /// Sprites-region lumps whose names match no sprite grammar, in
    /// directory order.
    pub(super) invalid: Vec<String>,
}

/// Sprite name to frames to rotations, built over the directory walk.
pub(super) type Groups = BTreeMap<String, BTreeMap<char, Vec<RotationRow>>>;

/// The four-character sprite prefix to filter by, uppercased; anything
/// that is not four `ASCII` letters or digits is refused.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected text.
pub(super) fn parse_prefix(text: Option<&str>) -> Result<Option<String>, CyberError> {
    text.map(parse_sprite).transpose()
}

/// One validated sprite prefix: four `ASCII` letters or digits,
/// uppercased, the grammar's own [`parse_sprite_prefix`] check with the
/// listing's refusal text.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming the rejected text.
pub(super) fn parse_sprite(text: &str) -> Result<String, CyberError> {
    parse_sprite_prefix(text).map_err(|_error| {
        CyberError::InvalidArgument(format!(
            "prefix {text:?} is not four ASCII letters or digits: pass the sprite prefix, e.g. \
             \"POSS\"; leave it out to list every sprite"
        ))
    })
}

/// Records the one or two rotations a parsed sprite name contributes.
fn push_rotation(groups: &mut Groups, parsed: &SpriteName, mirror: Option<(char, u8)>, lump: &str) {
    let (frame, rotation, mirrored) = match mirror {
        None => (parsed.frame, parsed.rotation, false),
        Some((frame, rotation)) => (frame, rotation, true),
    };
    groups
        .entry(parsed.sprite.clone())
        .or_default()
        .entry(frame)
        .or_default()
        .push(RotationRow {
            rotation,
            lump: lump.to_owned(),
            mirrored,
        });
}

/// Whether an unparseable region name belongs in the `invalid` list:
/// without a prefix filter every one does, with one only names starting
/// with it.
fn shows_invalid(text: &str, prefix: Option<&str>) -> bool {
    prefix.is_none_or(|wanted| text.starts_with(wanted))
}

/// Collects one sprite lump's rotations into `groups`, honouring the
/// prefix filter.
pub(super) fn collect_sprite(
    groups: &mut Groups,
    parsed: &SpriteName,
    lump: &str,
    prefix: Option<&str>,
) {
    if prefix.is_some_and(|wanted| wanted != parsed.sprite) {
        return;
    }
    push_rotation(groups, parsed, None, lump);
    if let Some(mirror) = parsed.mirror {
        push_rotation(groups, parsed, Some(mirror), lump);
    }
}

/// Orders the collected groups into rows: frames by letter (the
/// [`BTreeMap`] order), rotations by number then lump name.
pub(super) fn sprite_rows(groups: Groups) -> Vec<SpriteRow> {
    groups
        .into_iter()
        .map(|(sprite, frames)| SpriteRow {
            sprite,
            frames: frames
                .into_iter()
                .map(|(frame, mut rotations)| {
                    rotations.sort_by(|first, second| {
                        first
                            .rotation
                            .cmp(&second.rotation)
                            .then_with(|| first.lump.cmp(&second.lump))
                    });
                    FrameRow { frame, rotations }
                })
                .collect(),
        })
        .collect()
}

/// `list_sprites`' work over the session. `prefix` filters by four
/// characters and must already be uppercased (`parse_prefix` normalises
/// the client's text).
///
/// # Errors
///
/// [`merged_rows`]' errors for the merged scope.
pub(super) fn list_sprites_run(
    session: &Session,
    page: &Page,
    prefix: Option<&str>,
) -> Result<SpritesReport, CyberError> {
    let rows = merged_rows(session)?;
    let mut groups: Groups = BTreeMap::new();
    let mut invalid: Vec<String> = Vec::new();
    for row in last_wins(
        rows.iter()
            .filter(|row| in_namespace(row, Namespace::Sprites))
            .collect(),
    ) {
        let text = row.name.to_string();
        match SpriteName::parse(&text) {
            Some(parsed) => collect_sprite(&mut groups, &parsed, &text, prefix),
            None if shows_invalid(&text, prefix) => invalid.push(text),
            None => {}
        }
    }
    let (total, sprites) = page.take(sprite_rows(groups));
    Ok(SpritesReport {
        total,
        sprites,
        invalid,
    })
}
