//! Composite wall textures: the `TEXTURE1`/`TEXTURE2` definitions behind
//! `load_texture`.
//!
//! A name resolves to the merged view's `TEXTURE2` definition, else its
//! `TEXTURE1` one, else a `TX_` region patch. A definition draws its
//! `PNAMES`-resolved patch components at their offsets in record order.

use crate::error::CyberError;
use crate::gfx::composite::{Component, Composite, compose};
use crate::gfx::image::IndexedImage;
use crate::session::Session;
use crate::session::textures::TextureSet;
use crate::wad::classify::Namespace;
use crate::wad::lumps;
use crate::wad::name::LumpName;
use crate::wad::textures::{TextureDef, parse_pnames};

use super::{Place, decode_patch_bytes, load_patch, locate, lump_bytes, merged_rows, not_found};

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
/// no `TX_` patch carries it; [`TextureSet::load`]' and
/// [`decode_patch_bytes`]' format errors for lumps that do not decode; a
/// `TX_` fallback that is not a decodable patch fails the same way.
pub(crate) fn load_texture(session: &Session, name: LumpName) -> Result<Composite, CyberError> {
    let set = TextureSet::load(session)?;
    let text = name.to_string();
    if let Some((def, _lump)) = set.find(&text) {
        return compose_texture(session, &text, def);
    }
    let entry = locate(
        &merged_rows(session)?,
        name,
        &[Place::In(Namespace::Textures)],
    )
    .ok_or_else(|| not_found("texture", "list_textures", name))?;
    let patch = decode_patch_bytes(session, &entry.lump.data)?;
    Ok(Composite {
        image: patch.image,
        missing: Vec::new(),
        problems: Vec::new(),
    })
}

/// The merged view's `PNAMES` list: the winning `PNAMES` lump parsed, or
/// empty when none exists or it does not decode (a texture component
/// index then has no name to resolve).
fn patch_name_list(session: &Session) -> Vec<String> {
    lump_bytes(session, lumps::PNAMES)
        .ok()
        .and_then(|bytes| parse_pnames(&bytes).ok())
        .unwrap_or_default()
}

/// The pixels of the patch lump `name`, `None` when the name is no lump
/// name or no patches-region or loose lump of it decodes.
fn patch_pixels(session: &Session, name: &str) -> Option<IndexedImage> {
    let lump = LumpName::parse(name).ok()?;
    load_patch(session, lump).ok().map(|patch| patch.image)
}

/// Composite one texture definition: its `PNAMES`-resolved components
/// (or a placeholder naming the raw index when `PNAMES` cannot answer),
/// drawn at their offsets in record order.
fn compose_texture(
    session: &Session,
    name: &str,
    def: &TextureDef,
) -> Result<Composite, CyberError> {
    let pnames = patch_name_list(session);
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
        .map(|label| patch_pixels(session, label))
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
