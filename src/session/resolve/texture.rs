//! Composite wall textures: the `TEXTURE1`/`TEXTURE2` definitions behind
//! `load_texture`.
//!
//! A name resolves to the merged view's `TEXTURE2` definition, else its
//! `TEXTURE1` one, else a `TX_` region patch. A definition draws its
//! `PNAMES`-resolved patch components at their offsets in record order.
//! The definitions ride in the caller's `PictureIndex`, decoded once per
//! operation, and every component comes through `super::load_patch`, so a
//! patch shared by several textures - or placed twice in one - is decoded
//! once.

use crate::error::CyberError;
use crate::gfx::composite::{Component, Composite, compose};
use crate::gfx::image::IndexedImage;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;
use crate::wad::textures::TextureDef;

use super::{PictureIndex, Place, decode_patch_bytes, load_patch, not_found};

/// The composed pixels of the wall texture `name`: the merged view's
/// `TEXTURE2` definition, else its `TEXTURE1` one, else a patch lump of
/// that name inside the `TX_` region (what ports store already-composed
/// textures as). A definition composites through
/// [`crate::gfx::composite`]: present patches draw in record order,
/// absent ones leave holes and land in [`Composite::missing`] and
/// [`Composite::problems`].
///
/// # Errors
///
/// [`CyberError::NotFound`] when neither texture lump defines `name` and
/// no `TX_` patch carries it; [`PictureIndex::find_texture`]' and
/// [`decode_patch_bytes`]' format errors for lumps that do not decode; a
/// `TX_` fallback that is not a decodable patch fails the same way.
pub(crate) fn load_texture(
    index: &PictureIndex<'_>,
    name: LumpName,
) -> Result<Composite, CyberError> {
    let text = name.to_string();
    if let Some(def) = index.find_texture(&text)? {
        return compose_texture(index, &text, &def);
    }
    let lump = index
        .located(name, &[Place::In(Namespace::Textures)])
        .ok_or_else(|| not_found("texture", "list_textures", name))?;
    let patch = decode_patch_bytes(index, &lump.data)?;
    Ok(Composite {
        image: patch.image,
        missing: Vec::new(),
        problems: Vec::new(),
    })
}

/// The pixels of the patch lump `name`, `None` when the name is no lump
/// name or no patches-region or loose lump of it decodes.
fn patch_pixels(index: &PictureIndex<'_>, name: &str) -> Option<IndexedImage> {
    let lump = LumpName::parse(name).ok()?;
    load_patch(index, lump).ok().map(|patch| patch.image)
}

/// Composite one texture definition: its `PNAMES`-resolved components
/// (or a placeholder naming the raw index when `PNAMES` cannot answer),
/// drawn at their offsets in record order.
fn compose_texture(
    index: &PictureIndex<'_>,
    name: &str,
    def: &TextureDef,
) -> Result<Composite, CyberError> {
    let pnames = index.pnames()?;
    let labels: Vec<String> = def
        .patches
        .iter()
        .map(|place| {
            pnames
                .get(usize::from(place.patch))
                .cloned()
                .unwrap_or_else(|| format!("PNAMES index {}", place.patch))
        })
        .collect();
    let images: Vec<Option<IndexedImage>> = labels
        .iter()
        .map(|label| patch_pixels(index, label))
        .collect();
    let components: Vec<Component<'_>> = def
        .patches
        .iter()
        .zip(labels.iter().zip(images.iter()))
        .map(|(place, (label, image))| Component {
            name: label,
            image: image.as_ref(),
            x: i32::from(place.x),
            y: i32::from(place.y),
        })
        .collect();
    compose(
        name,
        u32::from(def.width),
        u32::from(def.height),
        &components,
    )
}
