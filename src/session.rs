//! The per-connection editing session: layers, working archive, merged view
//! and edit history.
//!
//! A session holds the archives one client is working with, in roughly the
//! shape an engine sees them:
//!
//! - an optional **`IWAD` layer**: the read-only base game archive;
//! - any number of read-only **resource layers**: `PWAD`s stacked on top of
//!   the base in load order;
//! - one **working archive**: the editable [`Wad`], initially an empty
//!   `PWAD`. Every editing tool changes this and nothing else; it remembers
//!   the path it was opened from or last saved to.
//!
//! [`Session::merged`] is the **merged view**: the `IWAD` layer, then each
//! resource layer, then the working archive, folded together by
//! [`merge::merge`] so that later archives override earlier ones. Layers are
//! numbered from 0 over [`Session::layers`] with the `IWAD` first, and every
//! lump of the merged view remembers where it came from through
//! [`MergedLump::source`] and [`MergedLump::source_index`].
//!
//! Reads choose an archive with [`Scope`]: the merged view, the working
//! archive alone, or one layer by index. Edits go through [`Session::edit`],
//! which snapshots the working archive and pushes the snapshot onto
//! [`history`] so [`Session::undo`] and [`Session::redo`] can walk back and
//! forth; [`Session::is_dirty`] compares the working archive's revision with
//! the one it was last saved at.
//!
//! [`Session::begin_batch`] opens the one batch scope [`history`] allows:
//! while it is open every [`Session::edit`] folds into the single entry
//! [`Session::end_batch`] commits or rolls back, and the operations a fold
//! cannot carry — the whole-archive and saved-state swaps
//! ([`Session::replace_working`], [`Session::set_iwad`],
//! [`Session::add_resource`], [`Session::mark_saved`]) and the history
//! walkers — refuse with a typed conflict. That is structural enforcement
//! of the "one batch, one history entry" invariant, not a name list.
//!
//! Alongside the archives a session carries the client's [`RenderSettings`]
//! and [`TextureTheme`], the defaults the render and texture tools start
//! from. They are session state rather than archive state: no edit, undo or
//! replaced archive changes them.
//!
//! Read-only content reaches the working archive through [`cow`]:
//! [`Session::copy_map_to_working`] copies one whole map block out of the
//! merged view so the map tools can edit a map the `IWAD` owns, and
//! [`map_edit`] wraps that copy, the typed edit and the node invalidation
//! into the single undoable [`Session::edit_map`] step every map editing
//! tool builds on.
//!
//! The scope and origin types are `scope`; the read-view bodies are `views`;
//! the [`Session::custom_sources`] collection behind [`Session::definitions`]
//! is `custom`.

pub mod cow;
mod custom;
pub mod history;
pub mod map_edit;
pub mod mapdir;
pub mod merge;
pub mod palette;
pub mod resolve;
mod scope;
pub mod textures;
mod views;

pub use scope::{MergedLump, Scope, Source};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::defs::{CustomSources, Definitions, session_definitions};
use crate::error::CyberError;
use crate::render::RenderSettings;
use crate::session::{
    history::{BatchOutcome, BatchToken, History, batch_open_conflict},
    map_edit::MapEditReport,
};
use crate::themes::TextureTheme;
use crate::wad::{archive::Wad, classify::Namespace, game::Game, map::Map, name::LumpName};

/// What a read-only archive in the layer stack is to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerRole {
    /// The base game archive, layer 0 while it is loaded.
    Iwad,
    /// A content archive stacked above the base, in load order.
    Resource,
}

impl LayerRole {
    /// The role's lowercase wire word, as the layer tools report it.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Iwad => "iwad",
            Self::Resource => "resource",
        }
    }

    /// The role's label in messages: the base archive names its format in
    /// caps, a resource keeps the plain word.
    #[must_use]
    #[inline]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Iwad => "IWAD",
            Self::Resource => "resource",
        }
    }
}

/// One read-only archive of the session's layer stack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    /// Where the archive was read from, kept for reporting.
    pub path: PathBuf,
    /// Whether this layer is the base archive or a stacked resource.
    pub role: LayerRole,
    /// The parsed archive.
    pub wad: Wad,
}

impl Layer {
    /// A layer holding `wad`, read from `path`, playing `role`.
    #[must_use]
    #[inline]
    pub const fn new(path: PathBuf, role: LayerRole, wad: Wad) -> Self {
        Self { path, role, wad }
    }
}

/// State of a single client connection's editing session.
///
/// The session is the reflex every WAD tool reads from and writes to: it
/// never decides what an edit should be, it only keeps the archives, the
/// merged view they imply and the history of changes to the working archive.
/// [`Session::edit`] is the only way to change that archive, so an editing
/// tool cannot leave a half-applied edit or an unrecorded one behind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    /// The optional read-only base archive: layer 0 while it is loaded.
    iwad: Option<Layer>,
    /// The read-only archives above the base, in load order.
    resources: Vec<Layer>,
    /// The one editable archive.
    working: Wad,
    /// Where [`Session::working`] was opened from or last saved to.
    working_path: Option<PathBuf>,
    /// Undo and redo entries plus the revision bookkeeping of `working`.
    history: History,
    /// The map tools should default to, when the client has picked one.
    active_map: Option<LumpName>,
    /// Session-level render defaults, never part of the undo history.
    render: RenderSettings,
    /// Session-level texture theme, never part of the undo history.
    theme: TextureTheme,
}

impl Session {
    /// Makes `layer` the base game archive, replacing any previous one.
    ///
    /// Its role is normalised to [`LayerRole::Iwad`], so layer 0 is the base
    /// whatever the caller put in [`Layer::role`].
    ///
    /// # Errors
    ///
    /// [`CyberError::Conflict`] while a batch is open (see
    /// [`Session::begin_batch`]): the swap moves the merged view out from
    /// under the batch's steps and no fold can undo it.
    #[inline]
    pub fn set_iwad(&mut self, mut layer: Layer) -> Result<(), CyberError> {
        self.refuse_in_batch("replacing the base game layer")?;
        layer.role = LayerRole::Iwad;
        self.iwad = Some(layer);
        Ok(())
    }

    /// Appends a read-only archive above the existing layers, in load order.
    ///
    /// Its role is normalised to [`LayerRole::Resource`].
    ///
    /// # Errors
    ///
    /// [`Session::set_iwad`]'s batch refusal.
    #[inline]
    pub fn add_resource(&mut self, mut layer: Layer) -> Result<(), CyberError> {
        self.refuse_in_batch("stacking a resource layer")?;
        layer.role = LayerRole::Resource;
        self.resources.push(layer);
        Ok(())
    }

    /// The read-only layers, numbered from 0: the `IWAD` while it is loaded,
    /// then each resource in load order. These are the indices [`Source`] and
    /// [`Scope`] refer to.
    #[must_use]
    #[inline]
    pub fn layers(&self) -> Vec<&Layer> {
        self.iwad.iter().chain(self.resources.iter()).collect()
    }

    /// The base game layer, if one is loaded.
    #[must_use]
    #[inline]
    pub const fn iwad(&self) -> Option<&Layer> {
        self.iwad.as_ref()
    }

    /// The editable working archive.
    #[must_use]
    #[inline]
    pub const fn working(&self) -> &Wad {
        &self.working
    }

    /// Where the working archive was opened from or last saved to.
    #[must_use]
    #[inline]
    pub fn working_path(&self) -> Option<&Path> {
        self.working_path.as_deref()
    }

    /// The map tools should default to, if the client selected one.
    #[must_use]
    #[inline]
    pub const fn active_map(&self) -> Option<LumpName> {
        self.active_map
    }

    /// Selects the active map, or clears the selection.
    #[inline]
    pub const fn set_active_map(&mut self, map: Option<LumpName>) {
        self.active_map = map;
    }

    /// The session-level render defaults the rendering tools start from.
    #[must_use]
    #[inline]
    pub const fn render_settings(&self) -> &RenderSettings {
        &self.render
    }

    /// The session-level render defaults, mutably: the render tools validate
    /// every change through [`RenderSettings`]'s setters before assigning it.
    #[inline]
    pub const fn render_settings_mut(&mut self) -> &mut RenderSettings {
        &mut self.render
    }

    /// Replaces the render defaults (see [`RenderSettings`]).
    ///
    /// Settings are session state, not archive state: neither this nor
    /// [`Session::replace_working`] nor [`Session::undo`] touches the history
    /// of edits.
    #[inline]
    pub const fn set_render_settings(&mut self, settings: RenderSettings) {
        self.render = settings;
    }

    /// The session-level texture theme the texture tools start from (see
    /// [`crate::themes`]).
    #[must_use]
    #[inline]
    pub const fn texture_theme(&self) -> &TextureTheme {
        &self.theme
    }

    /// The session-level texture theme, mutably: the themes tools validate
    /// every change before assigning it.
    ///
    /// Like the render settings, the theme is session state: it is never
    /// part of the undo history.
    #[inline]
    pub const fn texture_theme_mut(&mut self) -> &mut TextureTheme {
        &mut self.theme
    }

    /// Adopts a new working archive, forgetting the edit history.
    ///
    /// This is what opening or reloading a file does: undo cannot reach back
    /// past it, and the fresh archive starts out clean.
    ///
    /// # Errors
    ///
    /// [`CyberError::Conflict`] while a batch is open (see
    /// [`Session::begin_batch`]): a fold cannot carry an archive swap, so
    /// the batch could not roll back over it.
    #[inline]
    pub fn replace_working(&mut self, wad: Wad, path: Option<PathBuf>) -> Result<(), CyberError> {
        self.refuse_in_batch("replacing the working archive")?;
        self.working = wad;
        self.working_path = path;
        self.history.reset();
        Ok(())
    }

    /// Records that the working archive is saved at `path` and is now clean.
    ///
    /// # Errors
    ///
    /// [`Session::replace_working`]'s batch refusal: a batch rolled back
    /// after a save would leave the file holding edits the session dropped,
    /// so a `save_wad` waits for the batch to end.
    #[inline]
    pub fn mark_saved(&mut self, path: PathBuf) -> Result<(), CyberError> {
        self.refuse_in_batch("marking the working archive saved")?;
        self.working_path = Some(path);
        self.history.mark_saved();
        Ok(())
    }

    /// Refuses an operation that replaces session state the open batch
    /// cannot fold: `what` names it in the message.
    ///
    /// # Errors
    ///
    /// [`CyberError::Conflict`] naming `what` while [`Session::batch_open`].
    fn refuse_in_batch(&self, what: &str) -> Result<(), CyberError> {
        if self.batch_open() {
            return Err(batch_open_conflict(what));
        }
        Ok(())
    }

    /// Whether the working archive changed since it was last saved.
    #[must_use]
    #[inline]
    pub const fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    /// Runs one editing operation over the working archive, with undo.
    ///
    /// `f` gets a clone of the working archive, which is cheap because lump
    /// payloads are shared `Arc` slices, and returns whatever the caller needs
    /// back. An `Err` from `f` discards that clone: the session keeps the
    /// archive it had and no history entry appears. An `Ok` commits the clone
    /// and records `label` as the step to undo.
    ///
    /// While a [`Session::begin_batch`] scope is open the commit folds into
    /// the batch's single entry instead: nothing stands on the stack per
    /// inner edit, and the batch's own label is what one [`Session::undo`]
    /// will report for the whole run.
    ///
    /// # Errors
    ///
    /// Whatever `f` reports, unchanged.
    #[inline]
    pub fn edit<T, F>(&mut self, label: &str, f: F) -> Result<T, CyberError>
    where
        F: FnOnce(&mut Wad) -> Result<T, CyberError>,
    {
        let mut candidate = self.working.clone();
        let value = f(&mut candidate)?;
        self.history.record(label, &mut self.working, candidate);
        Ok(value)
    }

    /// Opens the session's one batch scope: every [`Session::edit`] from
    /// here until [`Session::end_batch`] folds into a single history entry
    /// labelled `label`, whose snapshot is the working archive as it stands
    /// now. This is the structural form of the batch's "one entry per run"
    /// rule: inner edits cannot stand on the stack beside it, and the
    /// operations a fold cannot carry refuse with [`CyberError::Conflict`]
    /// while the scope is open.
    ///
    /// # Errors
    ///
    /// [`CyberError::Conflict`] when a batch is already open: a session runs
    /// one batch at a time.
    #[inline]
    pub fn begin_batch(&mut self, label: &str) -> Result<BatchToken, CyberError> {
        self.history.begin_batch(label, &self.working)
    }

    /// Closes the batch scope `token` opened, applying `outcome`: a
    /// [`BatchOutcome::Commit`] leaves the batch's fold as one undo entry
    /// and a [`BatchOutcome::Rollback`] restores the exact pre-batch
    /// working archive and revision and leaves nothing. Returns whether the
    /// batch left an undo entry, so a scope whose edits all failed stays
    /// invisible in the history.
    ///
    /// # Errors
    ///
    /// [`CyberError::Conflict`] when `token` does not name the open batch;
    /// the scope stays open for its holder to close.
    #[inline]
    pub fn end_batch(
        &mut self,
        token: BatchToken,
        outcome: BatchOutcome,
    ) -> Result<bool, CyberError> {
        self.history.end_batch(&mut self.working, token, outcome)
    }

    /// Whether a [`Session::begin_batch`] scope currently owns the history.
    #[must_use]
    #[inline]
    pub const fn batch_open(&self) -> bool {
        self.history.batch_open()
    }

    /// Steps back over the most recent edit and returns its label.
    ///
    /// Only the working archive changes: layers and the active map are not
    /// part of the history.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] with `"nothing to undo"` when the undo stack
    /// is empty, [`CyberError::Conflict`] while a batch scope is open.
    #[inline]
    pub fn undo(&mut self) -> Result<String, CyberError> {
        self.history.undo(&mut self.working)
    }

    /// How many undo entries the history currently holds. The batch tool
    /// reads this before [`Session::begin_batch`] to refuse a plan whose
    /// steps could not fit the undo stack at all; the open batch itself
    /// only ever adds one entry when it commits.
    #[must_use]
    #[inline]
    pub fn undo_depth(&self) -> usize {
        self.history.undo_depth()
    }

    /// Re-applies the most recently undone edit and returns its label.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] with `"nothing to redo"` when the redo stack
    /// is empty, [`CyberError::Conflict`] while a batch scope is open.
    #[inline]
    pub fn redo(&mut self) -> Result<String, CyberError> {
        self.history.redo(&mut self.working)
    }

    /// The labels of the edits that [`Session::undo`] would take back, newest
    /// first.
    #[must_use]
    #[inline]
    pub fn undo_labels(&self) -> Vec<&str> {
        self.history.undo_labels()
    }

    /// The labels of the edits that [`Session::redo`] would replay, newest
    /// first.
    #[must_use]
    #[inline]
    pub fn redo_labels(&self) -> Vec<&str> {
        self.history.redo_labels()
    }

    /// The directory an engine would see: every layer in load order with the
    /// working archive on top, later archives overriding earlier ones.
    #[must_use]
    #[inline]
    pub fn merged(&self) -> Vec<MergedLump> {
        views::merged(self)
    }

    /// The merged view of the read-only layers alone: `IWAD` plus resources
    /// folded in load order, with an empty archive where [`Session::merged`]
    /// puts the working one. This is what the layers provide before any of
    /// this session's edits.
    #[must_use]
    #[inline]
    pub fn base_lumps(&self) -> Vec<MergedLump> {
        views::base_lumps(self)
    }

    /// The game the session's archives identify: from the `IWAD` layer's
    /// lump names when one is loaded, else from the merged view's names
    /// (see [`crate::wad::game::detect_game`]).
    #[must_use]
    #[inline]
    pub fn game(&self) -> Game {
        views::game(self)
    }

    /// The game the tools treat the session as: [`Session::game`]'s
    /// detection, with an unidentified session standing for DOOM II (the
    /// same default the `defs` tools document).
    #[must_use]
    #[inline]
    pub fn effective_game(&self) -> Game {
        let found = self.game();
        if found == Game::Unknown {
            return Game::Doom2;
        }
        found
    }

    /// The custom definition texts of the merged view: every `DEHACKED`,
    /// `DECORATE`, `ZSCRIPT` and `MAPINFO`/`ZMAPINFO` lump in load order
    /// with `#include` lumps appended, plus notes about includes that
    /// could not be followed (see [`crate::defs::CustomSources`]).
    ///
    /// # Errors
    ///
    /// [`Session::lumps_in`]'s error for the merged scope.
    #[inline]
    pub fn custom_sources(&self) -> Result<(CustomSources, Vec<String>), CyberError> {
        custom::custom_sources(self)
    }

    /// The definitions the session's tools read: the [`Session::effective_game`]
    /// tables overlaid with the merged view's `DEHACKED`, `DECORATE`,
    /// `ZScript` and `MAPINFO` customs — one read under one session lock,
    /// shared by every tool that resolves thing names or describes specials.
    ///
    /// # Errors
    ///
    /// [`Session::custom_sources`]' read errors and
    /// [`session_definitions`]' refusal for malformed embedded tables.
    #[inline]
    pub fn definitions(&self) -> Result<Definitions, CyberError> {
        let (sources, _notes) = self.custom_sources()?;
        session_definitions(self.effective_game(), &sources)
    }

    /// The lumps of one scope, in directory order.
    ///
    /// [`Scope::Working`] tags every lump [`Source::Working`],
    /// [`Scope::Layer`] tags its own layer and [`Scope::Merged`] returns
    /// [`Session::merged`].
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] naming the loaded layer count when
    /// [`Scope::Layer`] asks for a layer that is not there.
    #[inline]
    pub fn lumps_in(&self, scope: Scope) -> Result<Vec<MergedLump>, CyberError> {
        views::lumps_in(self, scope)
    }

    /// The lump an engine's lookup of `name` would find: the last match in
    /// the merged view. With `namespace` set, only lumps inside that
    /// namespace match; `None` means any region counts.
    #[must_use]
    #[inline]
    pub fn find(&self, name: LumpName, namespace: Option<Namespace>) -> Option<MergedLump> {
        views::find(self, name, namespace)
    }

    /// The merged view's last lump named `name`, wherever it sits: the
    /// read of a global engine lump such as `PLAYPAL`, `COLORMAP`,
    /// `PNAMES` or `TEXTURE1`, which lives outside any namespace region
    /// and is answered by the winning copy alone.
    #[must_use]
    #[inline]
    pub fn find_global(&self, name: LumpName) -> Option<MergedLump> {
        views::find(self, name, None)
    }

    /// Copies the map block `map` from a read-only layer into the working
    /// archive so it can be edited: the session's copy-on-write entry point.
    ///
    /// The whole block (header, member lumps and any `GL` node run, as the
    /// merged view shows them) is appended to the working archive as one
    /// undoable edit labelled `copy_map NAME`. Returns `false`, recording
    /// nothing, when the working archive already holds that map: its own copy
    /// is what the merged view shows anyway.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] when no loaded archive holds a block named
    /// `map`; neither the working archive nor the history changes.
    #[inline]
    pub fn copy_map_to_working(&mut self, map: LumpName) -> Result<bool, CyberError> {
        if cow::has_map_block(self.working(), map) {
            return Ok(false);
        }
        let merged = self.merged();
        self.edit(&format!("copy_map {map}"), |wad| {
            cow::copy_map_block(wad, &merged, map)
        })
    }

    /// Runs one undoable edit of the map block named `map`, the step every
    /// map editing tool is a thin wrapper around.
    ///
    /// This is a single [`Session::edit`]: one history entry under `label`,
    /// and any error — including the copy below — rolls everything back.
    /// The step proper lives in [`map_edit`]:
    ///
    /// 1. copies the block from the merged view into the working archive
    ///    when the working archive does not hold it yet
    ///    (see [`cow::copy_map_block`]), reporting `copied_to_working`;
    /// 2. decodes the block with [`crate::wad::map::read_map`] and hands
    ///    the typed [`Map`] to `f`;
    /// 3. when `f` changed geometry (a referenced vertex, a linedef's
    ///    `(v1, v2, front, back)`, a sidedef's sector or the sector count
    ///    — trailing vertices no linedef references, the split points
    ///    `build_nodes` appends, are invisible to the node builder and
    ///    stay out of this comparison), empties `SEGS`, `SSECTORS`,
    ///    `NODES`, `REJECT` and `BLOCKMAP` in place, strips those trailing
    ///    vertices, drops `ZNODES` and removes the block's `GL` node run
    ///    from the working archive, reporting `nodes_invalidated`;
    /// 4. writes the map back with [`crate::wad::map::write_map`] and
    ///    splices the new lumps over the old block range, at the same
    ///    directory position.
    ///
    /// The emptied node lumps mean vanilla `Doom` cannot play the map
    /// until `build_nodes` rebuilds them; `ZDoom`-family ports rebuild
    /// missing node data themselves and play fine. Property-only edits keep
    /// the node lumps.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] when no loaded archive holds a block named
    /// `map`, [`CyberError::Format`] when the block does not decode, the
    /// encoder's refusals for unrepresentable values, and whatever `f`
    /// reports; the session is unchanged in every one of those cases.
    #[inline]
    pub fn edit_map<T, F>(
        &mut self,
        map: LumpName,
        label: &str,
        f: F,
    ) -> Result<(T, MapEditReport), CyberError>
    where
        F: FnOnce(&mut Map) -> Result<T, CyberError>,
    {
        let merged = self.merged();
        self.edit(label, move |wad| {
            map_edit::edit_in_place(wad, &merged, map, f)
        })
    }

    /// Runs one undoable edit that rewrites a map block's *format*, the step
    /// `convert_map_format` is a thin wrapper around.
    ///
    /// Exactly [`Session::edit_map`]'s contract — the copy-on-write step, the
    /// typed [`Map`] handed to `f`, the re-encode over the block's own
    /// directory range, one history entry under `label`, nothing changing on
    /// any error — plus one difference: node data cannot survive a format
    /// change, so the block's `GL` node run is removed and the report says
    /// `nodes_invalidated` even when the geometry signature is untouched
    /// (which a conversion always leaves it). `f` owns the node lumps inside
    /// [`Map::other`]; see [`crate::wad::map::convert`].
    ///
    /// # Errors
    ///
    /// [`Session::edit_map`]'s errors, including the target encoder's refusal
    /// for a value the new format cannot store.
    #[inline]
    pub fn convert_map<T, F>(
        &mut self,
        map: LumpName,
        label: &str,
        f: F,
    ) -> Result<(T, MapEditReport), CyberError>
    where
        F: FnOnce(&mut Map) -> Result<T, CyberError>,
    {
        let merged = self.merged();
        self.edit(label, move |wad| {
            map_edit::rewrite_in_place(wad, &merged, map, f)
        })
    }
}

#[cfg(test)]
mod tests;
