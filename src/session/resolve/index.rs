//! The merged view's picture lookups, built once per operation.
//!
//! A `PictureIndex` (re-exported at the module root) walks the session's
//! cached merged directory once and
//! keeps what the resolvers match on: the winning data row for every
//! (name, `Place`) pair - the last lump of that name each place rule
//! admits, exactly what the old per-call linear scan answered with - the
//! sprites-region rows in directory order with their names parsed once, the
//! merged `PNAMES`/`TEXTURE1`/`TEXTURE2` definitions decoded on the first
//! texture of the operation, the `PLAYPAL` entry `PNG` pictures quantize
//! against loaded on the first of those, and the patches decoded so far
//! keyed by name. One composite texture, one whole [`crate::render::Scene`]
//! or one sprite-dressed `render_view` therefore reads the directory, the
//! definition lumps and every shared patch component once, not once per
//! picture. The index is a read view, never session state: it borrows the
//! session it was built from and holds nothing an edit could invalidate
//! within one operation.

use alloc::collections::BTreeMap;
use core::cell::RefCell;

#[cfg(test)]
use core::cell::Cell;

use crate::error::CyberError;
use crate::gfx::palette::Palette;
use crate::gfx::patch::Patch;
use crate::gfx::sprite_name::SpriteName;
use crate::session::palette::load_palette;
use crate::session::textures::TextureSet;
use crate::session::{MergedLump, Scope, Session};
use crate::wad::archive::Lump;
use crate::wad::classify::{Analysis, LumpKind, Namespace};
use crate::wad::name::LumpName;
use crate::wad::textures::TextureDef;

use super::{PNG_PALETTE, Place};

/// One sprites-region row of the index: the lump, its name, and the sprite
/// grammar's reading of that name (`None` for a name the grammar rejects),
/// parsed once at build so a rotation sweep never re-parses a lump name.
#[derive(Clone, Debug)]
pub(super) struct SpriteRow {
    /// Normalised lump name.
    pub(super) name: LumpName,
    /// The lump itself, whose payload is decoded.
    pub(super) lump: Lump,
    /// What [`SpriteName::parse`] made of the name.
    pub(super) parsed: Option<SpriteName>,
}

/// The merged view's picture lookups for one operation: which lump a name
/// resolves to under each place rule, the sprite rows, the decoded texture
/// definitions and the patches read so far. Build one per operation (or per
/// [`crate::render::Scene`]) and pass it to every resolver call.
#[derive(Debug)]
pub(crate) struct PictureIndex<'sess> {
    /// The session whose merged view this index read; every resolver still
    /// reaches it for reads the index does not serve (the map, the
    /// `COLORMAP`, the requested output palette).
    session: &'sess Session,
    /// The last data row per (name, place): the answer `locate` used to
    /// scan the whole directory for.
    rows: BTreeMap<(LumpName, Place), Lump>,
    /// The sprites-region data rows, in directory order.
    sprites: Vec<SpriteRow>,
    /// The merged view's texture definitions, decoded on the first texture
    /// lookup of the operation; a refusal (a malformed lump) is not stored,
    /// so every lookup reports it as its own.
    textures: RefCell<Option<TextureSet>>,
    /// The `PLAYPAL` entry `PNG` pictures quantize against, loaded on the
    /// first of those.
    palette: RefCell<Option<Palette>>,
    /// The patches decoded through [`super::load_patch`] so far, by the
    /// name they were looked up under.
    patches: RefCell<BTreeMap<LumpName, Patch>>,
    /// How many patches this index has decoded; the resolver tests count
    /// these.
    #[cfg(test)]
    decodes: Cell<u64>,
}

impl<'sess> PictureIndex<'sess> {
    /// Reads the session's cached merged directory once and files every
    /// data lump under it.
    ///
    /// # Errors
    ///
    /// [`Session::analyzed`]' [`CyberError::NotFound`](crate::error::CyberError::NotFound)
    /// for an out-of-range scope (the merged scope is never out of range).
    pub(crate) fn new(session: &'sess Session) -> Result<Self, CyberError> {
        #[cfg(test)]
        session.cache.count_picture_index();
        let (entries, analysis) = session.analyzed(Scope::Merged)?;
        let mut built = Self {
            session,
            rows: BTreeMap::new(),
            sprites: Vec::new(),
            textures: RefCell::new(None),
            palette: RefCell::new(None),
            patches: RefCell::new(BTreeMap::new()),
            #[cfg(test)]
            decodes: Cell::new(0_u64),
        };
        built.collect(&entries, &analysis);
        Ok(built)
    }

    /// The session this index was built from: what a caller still reads
    /// directly beside the resolvers (the map block, the `COLORMAP`, the
    /// game's sky marker).
    #[must_use]
    pub(crate) const fn session(&self) -> &'sess Session {
        self.session
    }

    /// The winning lump of `name` under the first of `places` that has a
    /// match: the last data row that place admits, which is what an
    /// engine's lookup of a name reaches for.
    pub(super) fn located(&self, name: LumpName, places: &[Place]) -> Option<&Lump> {
        places
            .iter()
            .find_map(|place| self.rows.get(&(name, *place)))
    }

    /// The sprites-region rows, in directory order.
    pub(super) fn sprites(&self) -> &[SpriteRow] {
        &self.sprites
    }

    /// The patch decoded for `name` earlier in this operation.
    pub(super) fn cached_patch(&self, name: LumpName) -> Option<Patch> {
        self.patches.borrow().get(&name).cloned()
    }

    /// Records `patch` as the decode for `name` for the rest of this
    /// operation.
    pub(super) fn store_patch(&self, name: LumpName, patch: &Patch) {
        #[cfg(test)]
        self.decodes.set(self.decodes.get().saturating_add(1));
        self.patches.borrow_mut().insert(name, patch.clone());
    }

    /// The palette `PNG` graphics are quantized against: `PLAYPAL` entry 0,
    /// loaded into the index on the first `PNG` picture of the operation.
    ///
    /// # Errors
    ///
    /// [`load_palette`]' [`CyberError::NotFound`](crate::error::CyberError::NotFound)
    /// for a session without a `PLAYPAL`; every call reporting it reads
    /// again.
    pub(super) fn reference_palette(&self) -> Result<Palette, CyberError> {
        if let Some(palette) = self.palette.borrow().as_ref() {
            return Ok(palette.clone());
        }
        let loaded = load_palette(self.session, PNG_PALETTE)?;
        self.palette.replace(Some(loaded.clone()));
        Ok(loaded)
    }

    /// The texture definition named `text`: the definitions decoded into
    /// the index on this first call, read from it after that.
    ///
    /// # Errors
    ///
    /// [`TextureSet::load`]' errors for a malformed definition lump,
    /// reported unchanged on every call until one decodes.
    pub(crate) fn find_texture(&self, text: &str) -> Result<Option<TextureDef>, CyberError> {
        self.definitions()?;
        let found = self
            .textures
            .borrow()
            .as_ref()
            .and_then(|set| set.find(text))
            .map(|(def, _which)| def.clone());
        Ok(found)
    }

    /// The merged `PNAMES` names, in index order (empty without a `PNAMES`
    /// lump), warming the definitions slot first so it answers whatever
    /// [`Self::find_texture`] was asked about.
    ///
    /// # Errors
    ///
    /// [`Self::find_texture`]' errors.
    pub(super) fn pnames(&self) -> Result<Vec<String>, CyberError> {
        self.definitions()?;
        let names = self
            .textures
            .borrow()
            .as_ref()
            .map_or_else(Vec::new, |set| set.pnames().to_vec());
        Ok(names)
    }

    /// How many patches this index has decoded (resolver tests only).
    #[cfg(test)]
    pub(super) fn decode_count(&self) -> u64 {
        self.decodes.get()
    }

    /// Decodes the merged definition lumps into the slot, once.
    ///
    /// # Errors
    ///
    /// [`TextureSet::load`]' errors.
    fn definitions(&self) -> Result<(), CyberError> {
        if self.textures.borrow().is_none() {
            let loaded = TextureSet::load(self.session)?;
            self.textures.replace(Some(loaded));
        }
        Ok(())
    }

    /// Files every row of one cached merged directory.
    fn collect(&mut self, entries: &[MergedLump], analysis: &Analysis) {
        for (entry, info) in entries
            .iter()
            .zip(analysis.lumps.iter())
            .filter(|pair| pair.1.kind != LumpKind::Marker)
        {
            self.index_lump(info.namespace, &entry.lump);
        }
    }

    /// Files one data lump under its name: every place rule that admits it
    /// (later lumps of the same name overwrite earlier ones, so the last
    /// wins exactly as the engine's lookup does), and the sprites-region
    /// list with its grammar reading.
    fn index_lump(&mut self, namespace: Option<Namespace>, lump: &Lump) {
        self.rows.insert((lump.name, Place::Anywhere), lump.clone());
        if !matches!(namespace, Some(Namespace::Flats | Namespace::Sprites)) {
            self.rows.insert((lump.name, Place::Loose), lump.clone());
        }
        if let Some(region) = namespace {
            self.rows
                .insert((lump.name, Place::In(region)), lump.clone());
        }
        if namespace == Some(Namespace::Sprites) {
            self.sprites.push(SpriteRow {
                name: lump.name,
                lump: lump.clone(),
                parsed: SpriteName::parse(&lump.name.to_string()),
            });
        }
    }
}
