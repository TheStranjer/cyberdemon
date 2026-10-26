//! The merged view's composite-texture definitions: the session-level
//! loader the texture tools, the picture resolvers, the theme tools and the
//! map validator all read through.
//!
//! [`TextureSet`] loads the winning `PNAMES`, `TEXTURE1` and `TEXTURE2` of
//! the merged view (decoding with the codecs in [`crate::wad::textures`])
//! and answers [`TextureSet::find`] the way an engine does: a `TEXTURE2`
//! record wins over a `TEXTURE1` one of the same name. It lives here,
//! beside the session it reads, so [`crate::session::resolve`] does not
//! reach into the tool layer for it; the `textures` tools re-export it and
//! add the listing rows and the copy-on-write `PNAMES` editing on top.

use alloc::collections::BTreeMap;

use crate::error::CyberError;
use crate::session::{MergedLump, Session, Source};
use crate::wad::lumps;
use crate::wad::name::LumpName;
use crate::wad::textures::{TextureDef, TextureLump, parse_pnames, parse_texture_lump};

/// The lump a listed texture came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhichLump {
    /// The merged view's `TEXTURE1` lump.
    Texture1,
    /// The merged view's `TEXTURE2` lump.
    Texture2,
}

impl WhichLump {
    /// The lump name clients see in the `lump` field.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Texture1 => "TEXTURE1",
            Self::Texture2 => "TEXTURE2",
        }
    }

    /// The name this lump carries.
    pub(crate) const fn name(self) -> LumpName {
        match self {
            Self::Texture1 => lumps::TEXTURE1,
            Self::Texture2 => lumps::TEXTURE2,
        }
    }

    /// The set's decoded entry for this lump, `None` when no archive
    /// holds it.
    pub(crate) const fn decoded(self, set: &TextureSet) -> Option<&(TextureLump, Source)> {
        match self {
            Self::Texture1 => set.texture1.as_ref(),
            Self::Texture2 => set.texture2.as_ref(),
        }
    }
}

/// The merged view's texture definitions, loaded once per call.
///
/// `TEXTURE2` entries override `TEXTURE1` ones of the same name in
/// [`TextureSet::find`]; the listings show both lumps, each row tagged with
/// its origin. This is the session-level loader the texture tools read and
/// write through; it is public so embedding callers can reuse the lookup (an
/// unused crate-private item would not survive the `dead_code` check).
#[derive(Debug)]
pub struct TextureSet {
    /// The merged `PNAMES` list; empty when no `PNAMES` lump exists.
    pub(crate) pnames: Vec<String>,
    /// The decoded `TEXTURE1` lump and where it came from.
    pub(crate) texture1: Option<(TextureLump, Source)>,
    /// The decoded `TEXTURE2` lump and where it came from.
    pub(crate) texture2: Option<(TextureLump, Source)>,
}

impl TextureSet {
    /// Reads the last (winning) `PNAMES`, `TEXTURE1` and `TEXTURE2` of the
    /// merged view. Without a `PNAMES` lump the patch indices inside the
    /// texture lumps stay unbounded.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] when `PNAMES` is malformed or a texture lump
    /// decodes neither as the `Doom` nor as the `Strife` layout.
    #[inline]
    pub fn load(session: &Session) -> Result<Self, CyberError> {
        let pnames_lump = session.find_global(lumps::PNAMES);
        let pnames = match pnames_lump.as_ref() {
            Some(entry) => parse_pnames(&entry.lump.data)?,
            None => Vec::new(),
        };
        let bound = if pnames_lump.is_some() {
            pnames.len()
        } else {
            usize::MAX
        };
        Ok(Self {
            texture1: decode_texture_lump(session.find_global(lumps::TEXTURE1), bound)?,
            texture2: decode_texture_lump(session.find_global(lumps::TEXTURE2), bound)?,
            pnames,
        })
    }

    /// The texture named `name`, case-insensitively; a `TEXTURE2` entry
    /// wins over a `TEXTURE1` one of the same name.
    #[must_use]
    #[inline]
    pub fn find(&self, name: &str) -> Option<(&TextureDef, WhichLump)> {
        first_match(self.texture2.as_ref(), name)
            .map(|def| (def, WhichLump::Texture2))
            .or_else(|| {
                first_match(self.texture1.as_ref(), name).map(|def| (def, WhichLump::Texture1))
            })
    }

    /// The merged `PNAMES` names, in index order; empty when the session
    /// has no `PNAMES` lump, which leaves every texture's patch references
    /// unresolvable.
    #[must_use]
    #[inline]
    pub fn pnames(&self) -> &[String] {
        &self.pnames
    }

    /// The texture lumps a renumbering would touch, as
    /// `(lump, lump name)` pairs for conflict reporting.
    pub(crate) fn textures(&self) -> Vec<(&TextureLump, &'static str)> {
        let mut list = Vec::new();
        if let Some(pair) = self.texture1.as_ref() {
            list.push((&pair.0, WhichLump::Texture1.as_str()));
        }
        if let Some(pair) = self.texture2.as_ref() {
            list.push((&pair.0, WhichLump::Texture2.as_str()));
        }
        list
    }

    /// Every defined texture of both lumps, in lump order (`TEXTURE1` first,
    /// `TEXTURE2` after), each tagged with the lump it came from and the
    /// archive it was read from. The `list_textures` tool pages these into
    /// its rows; a `TEXTURE2` record does not hide a `TEXTURE1` one of the
    /// same name here, only in [`TextureSet::find`].
    #[must_use]
    #[inline]
    pub fn texture_entries(&self) -> Vec<(&TextureDef, WhichLump, Source)> {
        let mut out = Vec::new();
        collect_entries(self.texture1.as_ref(), WhichLump::Texture1, &mut out);
        collect_entries(self.texture2.as_ref(), WhichLump::Texture2, &mut out);
        out
    }

    /// Every defined texture name (uppercased) with its width: the
    /// `TEXTURE1` records first, `TEXTURE2` wins on a duplicate name
    /// exactly as [`TextureSet::find`] does. The theme tools read the
    /// merged view's texture inventory through this.
    #[must_use]
    #[inline]
    pub fn widths(&self) -> BTreeMap<String, u16> {
        [&self.texture1, &self.texture2]
            .into_iter()
            .flatten()
            .flat_map(|pair| pair.0.textures.iter())
            .map(|def| (def.name.to_ascii_uppercase(), def.width))
            .collect()
    }
}

/// The first definition named `name` (case-insensitive) in one lump entry.
fn first_match<'lump>(
    entry: Option<&'lump (TextureLump, Source)>,
    name: &str,
) -> Option<&'lump TextureDef> {
    let pair = entry?;
    pair.0
        .textures
        .iter()
        .find(|def| def.name.eq_ignore_ascii_case(name))
}

/// Appends one lump entry's definitions to `out`, tagged with `which` and
/// the archive it came from.
fn collect_entries<'lump>(
    entry: Option<&'lump (TextureLump, Source)>,
    which: WhichLump,
    out: &mut Vec<(&'lump TextureDef, WhichLump, Source)>,
) {
    let Some(pair) = entry else {
        return;
    };
    out.extend(pair.0.textures.iter().map(|def| (def, which, pair.1)));
}

/// One texture lump decoded together with the archive it came from.
type DecodedLump = Option<(TextureLump, Source)>;

/// Decodes one found texture lump; `None` stays `None`.
///
/// # Errors
///
/// [`crate::wad::textures::parse_texture_lump`]'s [`CyberError::Format`].
fn decode_texture_lump(
    found: Option<MergedLump>,
    pnames_len: usize,
) -> Result<DecodedLump, CyberError> {
    let Some(entry) = found else {
        return Ok(None);
    };
    let lump = parse_texture_lump(&entry.lump.data, pnames_len)?;
    Ok(Some((lump, entry.source)))
}
