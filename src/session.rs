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
//! Alongside the archives a session carries the client's [`RenderSettings`],
//! the defaults the render tools start from. They are session state rather
//! than archive state: no edit, undo or replaced archive changes them.
//!
//! Read-only content reaches the working archive through [`cow`]:
//! [`Session::copy_map_to_working`] copies one whole map block out of the
//! merged view so the map tools can edit a map the `IWAD` owns.

pub mod cow;
pub mod history;
pub mod merge;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::render::RenderSettings;
use crate::session::history::History;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{LumpInfo, Namespace};
use crate::wad::game::{Game, detect_game};
use crate::wad::name::LumpName;

/// What a read-only archive in the layer stack is to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerRole {
    /// The base game archive, layer 0 while it is loaded.
    Iwad,
    /// A content archive stacked above the base, in load order.
    Resource,
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

/// Which archive a lump of the merged view was taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Layer `n` of [`Session::layers`]: 0 is the `IWAD`, then the resources.
    Layer(usize),
    /// The editable working archive.
    Working,
}

/// A lump as the merged view sees it: the bytes plus their origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedLump {
    /// The lump itself.
    pub lump: Lump,
    /// The archive it was taken from.
    pub source: Source,
    /// Its index inside that archive's directory.
    pub source_index: usize,
}

/// The archive a read tool looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The merged view: `IWAD`, resource layers, then the working archive.
    Merged,
    /// The editable working archive alone.
    Working,
    /// Read-only layer `n` of [`Session::layers`] alone.
    Layer(usize),
}

impl Scope {
    /// Parses a scope name: `merged`, `working` or `layer:N`, where `N` is a
    /// zero-based index into [`Session::layers`]. Every name is matched
    /// case-insensitively.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the accepted forms when `text`
    /// is neither keyword and `layer:` is not followed by a decimal index.
    #[inline]
    pub fn parse(text: &str) -> Result<Self, CyberError> {
        let lowered = text.to_ascii_lowercase();
        if lowered == "merged" {
            return Ok(Self::Merged);
        }
        if lowered == "working" {
            return Ok(Self::Working);
        }
        let Some(digits) = lowered.strip_prefix("layer:") else {
            return Err(unknown_scope(text));
        };
        digits.parse::<usize>().map(Self::Layer).map_err(|error| {
            CyberError::InvalidArgument(format!(
                "scope {text:?} names layer {digits:?}, which is not a decimal index: {error}"
            ))
        })
    }
}

/// The refusal for text that is not one of the accepted scope names.
fn unknown_scope(text: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "unknown scope {text:?}: expected \"merged\", \"working\" or \"layer:N\", where N is a \
         zero-based index into the loaded layers"
    ))
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
}

impl Session {
    /// Makes `layer` the base game archive, replacing any previous one.
    ///
    /// Its role is normalised to [`LayerRole::Iwad`], so layer 0 is the base
    /// whatever the caller put in [`Layer::role`].
    #[inline]
    pub fn set_iwad(&mut self, mut layer: Layer) {
        layer.role = LayerRole::Iwad;
        self.iwad = Some(layer);
    }

    /// Appends a read-only archive above the existing layers, in load order.
    ///
    /// Its role is normalised to [`LayerRole::Resource`].
    #[inline]
    pub fn add_resource(&mut self, mut layer: Layer) {
        layer.role = LayerRole::Resource;
        self.resources.push(layer);
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
    pub fn set_render_settings(&mut self, settings: RenderSettings) {
        self.render = settings;
    }

    /// Adopts a new working archive, forgetting the edit history.
    ///
    /// This is what opening or reloading a file does: undo cannot reach back
    /// past it, and the fresh archive starts out clean.
    #[inline]
    pub fn replace_working(&mut self, wad: Wad, path: Option<PathBuf>) {
        self.working = wad;
        self.working_path = path;
        self.history.reset();
    }

    /// Records that the working archive is saved at `path` and is now clean.
    #[inline]
    pub fn mark_saved(&mut self, path: PathBuf) {
        self.working_path = Some(path);
        self.history.mark_saved();
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

    /// Steps back over the most recent edit and returns its label.
    ///
    /// Only the working archive changes: layers and the active map are not
    /// part of the history.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] with `"nothing to undo"` when the undo stack
    /// is empty.
    #[inline]
    pub fn undo(&mut self) -> Result<String, CyberError> {
        self.history.undo(&mut self.working)
    }

    /// Re-applies the most recently undone edit and returns its label.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] with `"nothing to redo"` when the redo stack
    /// is empty.
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
        let layers: Vec<&Wad> = self.layers().into_iter().map(|layer| &layer.wad).collect();
        merge::merge_all(&layers, &self.working)
    }

    /// The game the session's archives identify: from the `IWAD` layer's
    /// lump names when one is loaded, else from the merged view's names
    /// (see [`crate::wad::game::detect_game`]).
    #[must_use]
    #[inline]
    pub fn game(&self) -> Game {
        self.iwad.as_ref().map_or_else(
            || {
                let view = self.merged();
                let names: Vec<LumpName> = view.iter().map(|entry| entry.lump.name).collect();
                detect_game(&names)
            },
            |layer| {
                let names: Vec<LumpName> = layer.wad.lumps.iter().map(|lump| lump.name).collect();
                detect_game(&names)
            },
        )
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
        match scope {
            Scope::Merged => Ok(self.merged()),
            Scope::Working => Ok(merge::tag(&self.working, Source::Working)),
            Scope::Layer(index) => layer_lumps(&self.layers(), index),
        }
    }

    /// The lump an engine's lookup of `name` would find: the last match in
    /// the merged view. With `namespace` set, only lumps inside that
    /// namespace match; `None` means any region counts.
    #[must_use]
    #[inline]
    pub fn find(&self, name: LumpName, namespace: Option<Namespace>) -> Option<MergedLump> {
        let view = self.merged();
        let analysis = merge::analyze_directory(&view);
        view.iter()
            .zip(analysis.lumps.iter())
            .rev()
            .find(|pair| is_wanted(pair.0, pair.1, name, namespace))
            .map(|pair| pair.0.clone())
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
}

/// The directory of layer `index`, tagged with that layer as its source.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming how many layers are stacked.
fn layer_lumps(layers: &[&Layer], index: usize) -> Result<Vec<MergedLump>, CyberError> {
    let Some(layer) = layers.get(index) else {
        return Err(CyberError::NotFound(format!(
            "layer {index} is not loaded: {} layer(s) are stacked, index 0 is the IWAD and each \
             resource follows in load order; use \"working\" for the editable archive",
            layers.len()
        )));
    };
    Ok(merge::tag(&layer.wad, Source::Layer(index)))
}

/// Whether this merged entry and its classification are the answer to a lookup
/// of `name` in `namespace`.
fn is_wanted(
    entry: &MergedLump,
    info: &LumpInfo,
    name: LumpName,
    namespace: Option<Namespace>,
) -> bool {
    entry.lump.name == name && namespace.is_none_or(|wanted| info.namespace == Some(wanted))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{Layer, LayerRole, MergedLump, Scope, Session, Source};
    use crate::error::CyberError;
    use crate::render::{Lighting, RenderSettings};
    use crate::session::history::HISTORY_LIMIT;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::game::Game;
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` whose payload is `text`, so copies are tellable.
    fn lump(text: &str) -> Lump {
        Lump::new(name(text), text.as_bytes().to_vec())
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A `PWAD` holding `lumps`.
    fn pwad(lumps: Vec<Lump>) -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps,
        }
    }

    /// A layer named after `file`, reading itself as its path.
    fn layer(role: LayerRole, file: &str, lumps: Vec<Lump>) -> Layer {
        Layer::new(PathBuf::from(format!("{file}.wad")), role, pwad(lumps))
    }

    /// The names and origins of a lump list, for one-glance assertions.
    fn tagged(list: &[MergedLump]) -> Vec<String> {
        list.iter()
            .map(|entry| {
                let source = match entry.source {
                    Source::Layer(index) => format!("L{index}"),
                    Source::Working => "W".to_owned(),
                };
                format!("{}@{source}", entry.lump.name)
            })
            .collect()
    }

    /// A session with a base archive, one resource and a working archive that
    /// overrides a flat and adds a map.
    fn loaded() -> Session {
        let mut session = Session::default();
        session.set_iwad(layer(
            LayerRole::Iwad,
            "doom",
            vec![
                lump("PLAYPAL"),
                lump("GAMEINFO"),
                marker("F_START"),
                lump("FLOOR_1"),
                lump("FLOOR_2"),
                marker("F_END"),
                marker("MAP01"),
                lump("THINGS"),
            ],
        ));
        session.add_resource(layer(
            LayerRole::Resource,
            "extras",
            vec![
                lump("GAMEINFO"),
                marker("F_START"),
                lump("F_SKY1"),
                marker("F_END"),
            ],
        ));
        session
    }

    /// Appends one marker lump to the working archive, always succeeding.
    fn add_marker(session: &mut Session, text: &str) -> Result<usize, CyberError> {
        let owned = lump(text);
        session.edit(&format!("add {text}"), move |wad| {
            let index = wad.lumps.len();
            wad.lumps.push(owned);
            Ok(index)
        })
    }

    /// Appends a fixed marker to a working archive. Parsing the name is the
    /// only step that can fail.
    fn append_marker(wad: &mut Wad) -> Result<(), CyberError> {
        let lump_name = LumpName::parse("MARKED")?;
        wad.lumps.push(Lump::marker(lump_name));
        Ok(())
    }

    #[test]
    fn scope_parses_every_documented_form() {
        assert_eq!(
            Scope::parse("merged").expect("merged"),
            Scope::Merged,
            "the merged view"
        );
        assert_eq!(
            Scope::parse("Working").expect("working"),
            Scope::Working,
            "the working archive, any case"
        );
        assert_eq!(
            Scope::parse("LAYER:2").expect("layer 2"),
            Scope::Layer(2),
            "a layer index, any case"
        );
        assert_eq!(
            Scope::parse("layer:0").expect("layer 0"),
            Scope::Layer(0),
            "the base archive"
        );
    }

    #[test]
    fn scope_rejects_a_non_numeric_layer() {
        let error = Scope::parse("layer:x").expect_err("x is no index");
        assert_eq!(error.code(), "invalid_argument", "bad layer index code");
        assert!(
            error.to_string().contains("\"x\"") || error.to_string().contains("'x'"),
            "the message shows what was not a number: {error}"
        );
        let empty = Scope::parse("layer:").expect_err("a bare prefix is no index");
        assert_eq!(empty.code(), "invalid_argument", "empty layer index code");
        let negative = Scope::parse("layer:-1").expect_err("no negative indices");
        assert_eq!(
            negative.code(),
            "invalid_argument",
            "negative layer index code"
        );
    }

    #[test]
    fn scope_rejects_an_unknown_name_with_the_accepted_forms() {
        let error = Scope::parse("nope").expect_err("nope is no scope");
        assert_eq!(error.code(), "invalid_argument", "unknown scope code");
        let message = error.to_string();
        assert!(
            message.contains("merged") && message.contains("working") && message.contains("layer:"),
            "the error lists the forms to use: {message}"
        );
    }

    #[test]
    fn scope_and_source_use_the_documented_wire_names() {
        assert_eq!(
            serde_json::to_value(Scope::Merged).expect("serializes"),
            json!("merged")
        );
        assert_eq!(
            serde_json::to_value(Scope::Working).expect("serializes"),
            json!("working")
        );
        assert_eq!(
            serde_json::to_value(Scope::Layer(3)).expect("serializes"),
            json!({ "layer": 3_u64 })
        );
        assert_eq!(
            serde_json::from_value::<Scope>(json!({"layer": 1_u64})).expect("deserializes"),
            Scope::Layer(1)
        );
        assert_eq!(
            serde_json::from_value::<Scope>(json!("working")).expect("deserializes"),
            Scope::Working
        );
        assert_eq!(
            serde_json::to_value(Source::Working).expect("serializes"),
            json!("working")
        );
        assert_eq!(
            serde_json::to_value(Source::Layer(2)).expect("serializes"),
            json!({ "layer": 2_u64 })
        );
        assert_eq!(
            serde_json::from_value::<Source>(json!("working")).expect("deserializes"),
            Source::Working
        );
        assert_eq!(
            serde_json::to_value(LayerRole::Iwad).expect("serializes"),
            json!("iwad")
        );
        assert_eq!(
            serde_json::to_value(LayerRole::Resource).expect("serializes"),
            json!("resource")
        );
    }

    #[test]
    fn a_fresh_session_has_no_layers_and_an_empty_working_archive() {
        let session = Session::default();
        assert!(session.iwad().is_none(), "no base archive yet");
        assert!(session.layers().is_empty(), "no layers");
        assert!(session.working().is_empty(), "an empty working PWAD");
        assert_eq!(
            session.working().kind,
            WadKind::Pwad,
            "the default kind is PWAD"
        );
        assert!(session.working_path().is_none(), "never opened from a file");
        assert!(!session.is_dirty(), "nothing has changed");
        assert_eq!(session.active_map(), None, "no map selected");
        assert!(session.merged().is_empty(), "the merged view is empty too");
    }

    #[test]
    fn layers_are_the_iwad_then_the_resources_in_load_order() {
        let session = loaded();
        let paths: Vec<String> = session
            .layers()
            .iter()
            .map(|layer| layer.path.display().to_string())
            .collect();
        assert_eq!(paths, vec!["doom.wad", "extras.wad"], "layer order");
        assert_eq!(
            session
                .layers()
                .iter()
                .map(|item| item.role)
                .collect::<Vec<_>>(),
            vec![LayerRole::Iwad, LayerRole::Resource],
            "roles follow position"
        );
        assert_eq!(
            session.iwad().expect("the base").path.display().to_string(),
            "doom.wad",
            "the iwad accessor names the base"
        );
    }

    #[test]
    fn layer_roles_are_normalised_to_their_position() {
        let mut session = Session::default();
        session.set_iwad(layer(LayerRole::Resource, "doom", vec![lump("PLAYPAL")]));
        session.add_resource(layer(LayerRole::Iwad, "one", vec![]));
        session.add_resource(layer(LayerRole::Resource, "two", vec![]));
        assert_eq!(
            session.iwad().expect("base").role,
            LayerRole::Iwad,
            "set_iwad wins"
        );
        assert_eq!(
            session
                .layers()
                .iter()
                .map(|item| item.role)
                .collect::<Vec<_>>(),
            vec![LayerRole::Iwad, LayerRole::Resource, LayerRole::Resource],
            "a resource never claims to be the base"
        );
        assert_eq!(session.layers().len(), 3, "one iwad plus two resources");
    }

    #[test]
    fn setting_the_iwad_twice_keeps_one_base_layer() {
        let mut session = Session::default();
        session.set_iwad(layer(LayerRole::Iwad, "doom", vec![lump("PLAYPAL")]));
        session.set_iwad(layer(LayerRole::Iwad, "plutonia", vec![lump("PLAYPAL")]));
        assert_eq!(
            session.layers().len(),
            1,
            "the base was replaced, not stacked"
        );
        assert_eq!(
            session.iwad().expect("base").path.display().to_string(),
            "plutonia.wad",
            "the later call won"
        );
    }

    #[test]
    fn merged_view_overrides_by_last_archive_wins() {
        let session = loaded();
        assert_eq!(
            tagged(&session.merged()),
            vec![
                "PLAYPAL@L0",
                "GAMEINFO@L1",
                "F_START@L0",
                "FLOOR_1@L0",
                "FLOOR_2@L0",
                "F_SKY1@L1",
                "F_END@L0",
                "MAP01@L0",
                "THINGS@L0",
            ],
            "the resource's GAMEINFO won in place, its flat joined the region"
        );
    }

    #[test]
    fn find_returns_the_last_match_in_the_merged_view() {
        let session = loaded();
        let gameinfo = session.find(name("GAMEINFO"), None).expect("found");
        assert_eq!(gameinfo.source, Source::Layer(1), "the resource won");
        let floor = session.find(name("FLOOR_1"), None).expect("found");
        assert_eq!(floor.source, Source::Layer(0), "only the base has it");
        assert!(
            session
                .find(name("F_START"), Some(Namespace::Flats))
                .is_some(),
            "markers sit in their region too"
        );
        assert!(
            session
                .find(name("FLOOR_1"), Some(Namespace::Sprites))
                .is_none(),
            "a flat is not a sprite"
        );
        assert!(
            session.find(name("NOPE"), None).is_none(),
            "an unknown name"
        );
        assert!(
            session
                .find(name("THINGS"), Some(Namespace::Flats))
                .is_none(),
            "map data sits in no namespace"
        );
        assert_eq!(
            session
                .find(name("THINGS"), None)
                .expect("map data is findable")
                .source,
            Source::Layer(0),
            "any namespace finds it"
        );
    }

    #[test]
    fn find_reports_the_working_copy_once_it_overrides() {
        let mut session = loaded();
        session.replace_working(pwad(vec![lump("GAMEINFO")]), None);
        session
            .edit("bring a flat", |wad| {
                wad.lumps.push(lump("FLOOR_1"));
                Ok(())
            })
            .expect("a plain append");
        let anywhere = session.find(name("FLOOR_1"), None).expect("found");
        assert_eq!(
            anywhere.source,
            Source::Working,
            "the working archive's loose copy is the last one"
        );
        let in_region = session
            .find(name("FLOOR_1"), Some(Namespace::Flats))
            .expect("found in flats");
        assert_eq!(
            in_region.source,
            Source::Layer(0),
            "a namespace lookup still finds the region's copy: the loose lump did not join it"
        );
        assert_eq!(
            session.find(name("GAMEINFO"), None).expect("found").source,
            Source::Working,
            "the working copy overrode the resource's"
        );
    }

    #[test]
    fn lumps_in_returns_each_scope() {
        let session = loaded();
        assert_eq!(
            tagged(
                &session
                    .lumps_in(Scope::Working)
                    .expect("the working archive")
            ),
            Vec::<String>::new(),
            "nothing has been edited yet"
        );
        assert_eq!(
            tagged(&session.lumps_in(Scope::Layer(0)).expect("the base")).len(),
            8,
            "the base archive reports itself unchanged"
        );
        assert_eq!(
            tagged(&session.lumps_in(Scope::Layer(1)).expect("the resource")),
            vec!["GAMEINFO@L1", "F_START@L1", "F_SKY1@L1", "F_END@L1"],
            "a layer scope never applies the merge"
        );
        assert_eq!(
            tagged(&session.lumps_in(Scope::Merged).expect("the merged view")),
            tagged(&session.merged()),
            "the merged scope is the merged view"
        );
    }

    #[test]
    fn lumps_in_reports_an_out_of_range_layer() {
        let session = loaded();
        let error = session
            .lumps_in(Scope::Layer(2))
            .expect_err("only two layers are loaded");
        assert_eq!(error.code(), "not_found", "out of range code");
        let message = error.to_string();
        assert!(
            message.contains('2') && message.contains("working"),
            "the message says what is loaded and what to use instead: {message}"
        );
        let bare = Session::default();
        bare.lumps_in(Scope::Layer(0))
            .expect_err("no layers at all");
    }

    #[test]
    fn edit_records_its_label_and_marks_the_session_dirty() {
        let mut session = loaded();
        let index = add_marker(&mut session, "F_START").expect("the edit succeeds");
        assert_eq!(index, 0, "the closure's value comes back");
        assert_eq!(session.undo_labels(), vec!["add F_START"], "label recorded");
        assert!(session.is_dirty(), "an edit is unsaved work");
        assert_eq!(session.working().len(), 1, "the working archive changed");
        assert_eq!(
            session
                .layers()
                .iter()
                .map(|item| item.wad.len())
                .collect::<Vec<_>>(),
            vec![8, 4],
            "layers are never edited"
        );
    }

    #[test]
    fn a_failing_edit_changes_nothing_and_is_not_recorded() {
        let mut session = loaded();
        add_marker(&mut session, "A_START").expect("the first edit lands");
        let error = session
            .edit("broken", |_| {
                Err::<(), CyberError>(CyberError::InvalidArgument("nope".to_owned()))
            })
            .expect_err("the second edit fails");
        assert_eq!(error.code(), "invalid_argument", "the error passes through");
        assert_eq!(session.working().len(), 1, "no partial edit landed");
        assert_eq!(
            session.undo_labels(),
            vec!["add A_START"],
            "the failed edit left no history"
        );
        assert!(session.is_dirty(), "only for the one edit that worked");
    }

    #[test]
    fn undo_and_redo_walk_the_working_archive() {
        let mut session = loaded();
        add_marker(&mut session, "A_START").expect("first edit");
        add_marker(&mut session, "A_END").expect("second edit");
        assert_eq!(session.working().len(), 2, "both landed");
        assert_eq!(
            session.undo().expect("undo"),
            "add A_END",
            "newest label first"
        );
        assert_eq!(session.working().len(), 1, "the last edit is gone");
        assert_eq!(session.undo_labels(), vec!["add A_START"], "one step left");
        assert_eq!(session.redo_labels(), vec!["add A_END"], "one step back");
        assert_eq!(session.redo().expect("redo"), "add A_END", "replayed label");
        assert_eq!(session.working().len(), 2, "the edit is back");
        assert!(session.redo().is_err(), "nothing further to replay");
    }

    #[test]
    fn undoing_back_to_the_saved_state_makes_the_session_clean() {
        let mut session = loaded();
        session.replace_working(pwad(vec![lump("PLAYPAL")]), Some(PathBuf::from("out.wad")));
        assert!(!session.is_dirty(), "a replaced archive starts clean");
        assert!(session.undo_labels().is_empty(), "and cannot be undone");
        add_marker(&mut session, "MARK").expect("an edit");
        assert!(session.is_dirty(), "now there is unsaved work");
        session.mark_saved(PathBuf::from("out.wad"));
        assert!(!session.is_dirty(), "saving cleared it");
        assert_eq!(
            session
                .working_path()
                .expect("a path")
                .display()
                .to_string(),
            "out.wad",
            "and recorded where"
        );
        add_marker(&mut session, "MORE").expect("another edit");
        assert!(session.is_dirty(), "unsaved again");
        session.undo().expect("undo it");
        assert!(!session.is_dirty(), "back to the saved revision");
    }

    #[test]
    fn editing_after_an_undo_drops_the_redo_branch() {
        let mut session = loaded();
        add_marker(&mut session, "ONE").expect("first");
        add_marker(&mut session, "TWO").expect("second");
        session.undo().expect("back to one");
        assert_eq!(session.redo_labels(), vec!["add TWO"], "a branch to replay");
        add_marker(&mut session, "THREE").expect("a new branch");
        assert!(session.redo_labels().is_empty(), "the old branch is gone");
        assert_eq!(
            session.undo_labels(),
            vec!["add THREE", "add ONE"],
            "the history continues from the undone state, with a fresh revision"
        );
        assert_eq!(session.working().len(), 2, "ONE and THREE, not TWO");
    }

    #[test]
    fn replace_working_forgets_history_and_remember_the_new_path() {
        let mut session = loaded();
        add_marker(&mut session, "ONE").expect("an edit");
        session.replace_working(pwad(vec![lump("S_START")]), Some(PathBuf::from("new.wad")));
        assert!(
            session.undo_labels().is_empty(),
            "no history across archives"
        );
        assert!(!session.is_dirty(), "the new archive is clean");
        assert_eq!(
            session
                .working_path()
                .expect("the new path")
                .display()
                .to_string(),
            "new.wad",
            "the path came with it"
        );
        session.replace_working(Wad::default(), None);
        assert!(
            session.working_path().is_none(),
            "an unsaved archive has no path"
        );
        assert!(session.undo().is_err(), "and nothing to undo");
    }

    #[test]
    fn undo_and_redo_on_an_empty_history_are_not_found() {
        let mut session = loaded();
        let undo = session.undo().expect_err("nothing was edited");
        assert_eq!(undo.code(), "not_found", "empty undo code");
        assert_eq!(undo.to_string(), "nothing to undo", "empty undo message");
        let redo = session.redo().expect_err("nothing was undone");
        assert_eq!(redo.code(), "not_found", "empty redo code");
        assert_eq!(redo.to_string(), "nothing to redo", "empty redo message");
    }

    #[test]
    fn history_keeps_only_the_newest_edits() {
        let mut session = loaded();
        let steps = HISTORY_LIMIT
            .checked_add(5)
            .expect("a count past the limit");
        for step in 0..steps {
            let label = format!("edit-{step}");
            session
                .edit(label.as_str(), append_marker)
                .expect("appends never fail");
        }
        let labels = session.undo_labels();
        assert_eq!(labels.len(), HISTORY_LIMIT, "the undo stack is capped");
        assert!(!labels.contains(&"edit-0"), "the oldest edit was dropped");
        let newest = steps.checked_sub(1).expect("a newest label");
        let newest_label = format!("edit-{newest}");
        assert_eq!(
            labels.first().copied().expect("a newest label"),
            newest_label.as_str(),
            "and the newest is on top"
        );
    }

    #[test]
    fn undo_only_touches_the_working_archive() {
        let mut session = loaded();
        session.set_active_map(Some(name("MAP01")));
        add_marker(&mut session, "ONE").expect("an edit");
        session.undo().expect("undo it");
        assert_eq!(
            session.active_map(),
            Some(name("MAP01")),
            "the selection is not part of the history"
        );
        assert_eq!(session.layers().len(), 2, "the layers never changed");
    }

    #[test]
    fn active_map_is_set_and_cleared() {
        let mut session = loaded();
        session.set_active_map(Some(name("MAP01")));
        assert_eq!(session.active_map(), Some(name("MAP01")));
        session.set_active_map(None);
        assert_eq!(session.active_map(), None, "cleared again");
    }

    #[test]
    fn a_fresh_session_starts_on_the_default_render_settings() {
        let session = Session::default();
        assert_eq!(
            session.render_settings(),
            &RenderSettings::default(),
            "the render tools' defaults are the documented ones"
        );
        assert_eq!(session.render_settings().width, 640, "the default width");
    }

    #[test]
    fn render_settings_survive_an_undo_and_a_replaced_archive() {
        let mut session = loaded();
        let mut settings = session.render_settings().clone();
        settings
            .set_resolution(1_280, 800)
            .expect("a supported resolution");
        settings.set_gamma(3).expect("a supported level");
        session.set_render_settings(settings.clone());
        session
            .edit("add MARK", append_marker)
            .expect("an edit to undo");
        session.undo().expect("undo it");
        session.replace_working(pwad(vec![lump("PLAYPAL")]), None);
        assert_eq!(
            session.render_settings(),
            &settings,
            "settings are session state: no edit, undo or new archive touches them"
        );
    }

    #[test]
    fn render_settings_mut_changes_the_stored_settings() {
        let mut session = Session::default();
        let mut expected = session.render_settings().clone();
        expected.lighting = Lighting::Fullbright;
        session
            .render_settings_mut()
            .set_lighting("fullbright")
            .expect("a supported model");
        assert_eq!(
            session.render_settings(),
            &expected,
            "the mutable accessor reaches the stored value"
        );
    }

    #[test]
    fn the_merged_view_reflects_working_archive_edits() {
        let mut session = loaded();
        session
            .edit("replace floor 1", |wad| {
                wad.lumps.push(marker("F_START"));
                wad.lumps.push(lump("FLOOR_1"));
                wad.lumps.push(marker("F_END"));
                Ok(())
            })
            .expect("a region override");
        assert_eq!(
            tagged(&session.merged()),
            vec![
                "PLAYPAL@L0",
                "GAMEINFO@L1",
                "F_START@L0",
                "FLOOR_1@W",
                "FLOOR_2@L0",
                "F_SKY1@L1",
                "F_END@L0",
                "MAP01@L0",
                "THINGS@L0",
            ],
            "the override landed where the base flat stood"
        );
    }

    #[test]
    fn game_of_a_fresh_session_is_unknown() {
        assert_eq!(
            Session::default().game(),
            Game::Unknown,
            "no archives, no game"
        );
    }

    #[test]
    fn game_reads_the_iwad_layer_first() {
        let mut session = loaded();
        session
            .edit("add MAP01", |wad| {
                wad.lumps.push(marker("MAP01"));
                Ok(())
            })
            .expect("an edit");
        assert_eq!(
            session.game(),
            Game::Doom2,
            "the base layer's MAP01 identifies DOOM II"
        );
        session.set_iwad(layer(
            LayerRole::Iwad,
            "doom1",
            vec![marker("E1M1"), lump("THINGS")],
        ));
        assert_eq!(
            session.game(),
            Game::Doom,
            "the episode base wins even with a MAP01 in the working archive"
        );
    }

    #[test]
    fn game_falls_back_to_the_merged_view_without_an_iwad() {
        let mut session = Session::default();
        session
            .edit("add MAP01", |wad| {
                wad.lumps.push(marker("MAP01"));
                Ok(())
            })
            .expect("an edit");
        assert_eq!(
            session.game(),
            Game::Doom2,
            "the working archive names the game"
        );
    }

    #[test]
    fn copy_map_to_working_brings_a_layer_map_into_the_working_archive() {
        let mut session = loaded();
        let copied = session
            .copy_map_to_working(name("MAP01"))
            .expect("the base holds MAP01");
        assert!(copied, "the working archive had no copy yet");
        assert_eq!(
            tagged(
                &session
                    .lumps_in(Scope::Working)
                    .expect("the working archive")
            ),
            vec!["MAP01@W", "THINGS@W"],
            "the whole block was appended, in view order"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["copy_map MAP01"],
            "the copy is one undoable edit"
        );
        assert!(session.is_dirty(), "and unsaved work");
        assert!(
            !session
                .copy_map_to_working(name("MAP01"))
                .expect("the working copy is already editable"),
            "the second call copies nothing"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["copy_map MAP01"],
            "and records no history for it"
        );
        assert_eq!(
            session.undo().expect("undo the copy"),
            "copy_map MAP01",
            "the label comes back"
        );
        assert!(session.working().is_empty(), "the block went back");
    }

    #[test]
    fn copy_map_to_working_reports_not_found_for_an_unknown_map() {
        let mut session = loaded();
        let error = session
            .copy_map_to_working(name("MAP07"))
            .expect_err("no archive holds MAP07");
        assert_eq!(error.code(), "not_found", "unknown map code");
        assert!(session.working().is_empty(), "nothing was copied");
        assert!(session.undo_labels().is_empty(), "and nothing recorded");
        assert!(!session.is_dirty(), "the session is still clean");
    }
}
