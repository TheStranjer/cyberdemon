//! The session's read views as free functions over a `&Session`: the merged
//! directory, its classification, the layers' own directory, the game the
//! archives identify, the lumps of one [`Scope`] and the engine's lookup of
//! a lump name.
//!
//! Each [`Session`] view method is a one-line delegate to the matching
//! function here, the way [`crate::session::cow`] and
//! [`crate::session::map_edit`] hold the bodies of the copy and edit steps:
//! the single `impl Session` block stays in `session.rs` and the machinery
//! that walks the layers lives here. Nothing in this module changes state;
//! the merged directory, its classification and the layers-only view are
//! served from the session's read cache (see [`crate::session::cache`]),
//! so the fold and the classification run once per working revision and
//! layer generation, not once per call.

use crate::error::CyberError;
use crate::session::merge::{self, analyze_directory};
use crate::session::{Layer, MergedLump, Scope, Session, Source};
use crate::wad::archive::Wad;
use crate::wad::classify::{Analysis, LumpInfo, Namespace};
use crate::wad::game::{Game, detect_game};
use crate::wad::name::LumpName;

/// The directory an engine would see: every layer in load order with the
/// working archive on top, later archives overriding earlier ones.
///
/// Served from the session's cache at its current key; the merge only runs
/// when the slot is cold or holds an earlier key.
///
/// The body of [`Session::merged`].
pub(super) fn merged(session: &Session) -> Vec<MergedLump> {
    let key = session.cache_key();
    session.cache.merged.get_or_init(key, || {
        build_merged(session);
        layer_fold(session, &session.working)
    })
}

/// The merged view of the read-only layers alone: `IWAD` plus resources
/// folded in load order, with an empty archive where [`merged`] puts the
/// working one. This is what the layers provide before any of this session's
/// edits.
///
/// Served from its own cache slot at the session's current key.
///
/// The body of [`Session::base_lumps`].
pub(super) fn base_lumps(session: &Session) -> Vec<MergedLump> {
    let key = session.cache_key();
    session.cache.base.get_or_init(key, || {
        build_merged(session);
        layer_fold(session, &Wad::default())
    })
}

/// The classification of [`merged`]'s directory, served from its own cache
/// slot at the session's current key.
pub(super) fn analysis(session: &Session) -> Analysis {
    let key = session.cache_key();
    session.cache.analysis.get_or_init(key, || {
        let view = merged(session);
        analyze_directory(&view)
    })
}

/// The lumps of one scope, in directory order, with the scope's
/// [`Analysis`], index-aligned.
///
/// [`Scope::Merged`] answers from the cached [`merged`] directory and the
/// cached [`analysis`]; [`Scope::Working`] and [`Scope::Layer`] classify
/// their own single-archive listing, which every caller otherwise analyzed
/// again right after fetching it.
///
/// The body of [`Session::analyzed`].
///
/// # Errors
///
/// [`lumps_in`]' [`CyberError::NotFound`] for a [`Scope::Layer`] that is
/// not loaded.
pub(super) fn analyzed(
    session: &Session,
    scope: Scope,
) -> Result<(Vec<MergedLump>, Analysis), CyberError> {
    match scope {
        Scope::Merged => Ok((merged(session), analysis(session))),
        Scope::Working => Ok(classify(merge::tag(&session.working, Source::Working))),
        Scope::Layer(index) => Ok(classify(layer_lumps(&session.layers(), index)?)),
    }
}

/// The game the session's archives identify: from the `IWAD` layer's lump
/// names when one is loaded, else from the merged view's names (see
/// [`crate::wad::game::detect_game`]).
///
/// The body of [`Session::game`].
pub(super) fn game(session: &Session) -> Game {
    session.iwad.as_ref().map_or_else(
        || {
            let view = merged(session);
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
/// [`Scope::Working`] tags every lump [`Source::Working`], [`Scope::Layer`]
/// tags its own layer and [`Scope::Merged`] returns the cached [`merged`].
///
/// The body of [`Session::lumps_in`].
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the loaded layer count when
/// [`Scope::Layer`] asks for a layer that is not there.
pub(super) fn lumps_in(session: &Session, scope: Scope) -> Result<Vec<MergedLump>, CyberError> {
    match scope {
        Scope::Merged => Ok(merged(session)),
        Scope::Working => Ok(merge::tag(&session.working, Source::Working)),
        Scope::Layer(index) => layer_lumps(&session.layers(), index),
    }
}

/// The lump an engine's lookup of `name` would find: the last match in
/// the merged view. With `namespace` set, only lumps inside that
/// namespace match; `None` means any region counts.
///
/// The merged directory and its classification come from the session's
/// cache, so a lookup no longer re-folds and re-classifies the whole view.
///
/// The body of [`Session::find`].
pub(super) fn find(
    session: &Session,
    name: LumpName,
    namespace: Option<Namespace>,
) -> Option<MergedLump> {
    let view = merged(session);
    let classified = analysis(session);
    view.iter()
        .zip(classified.lumps.iter())
        .rev()
        .find(|pair| is_wanted(pair.0, pair.1, name, namespace))
        .map(|pair| pair.0.clone())
}

/// A `merged`/`base` cache miss: counts as one merge for the cache tests.
#[cfg(test)]
fn build_merged(session: &Session) {
    session.cache.count_rebuild();
}

/// A `merged`/`base` cache miss: nothing to count outside the tests.
#[cfg(not(test))]
#[inline]
const fn build_merged(_session: &Session) {}

/// Fold the read-only layers in load order with `top` as the winning
/// archive: [`merged`] passes the working archive, [`base_lumps`] an empty
/// one.
fn layer_fold(session: &Session, top: &Wad) -> Vec<MergedLump> {
    let layers: Vec<&Wad> = session
        .layers()
        .into_iter()
        .map(|layer| &layer.wad)
        .collect();
    merge::merge_all(&layers, top)
}

/// Pair a freshly tagged single-archive listing with its classification.
fn classify(entries: Vec<MergedLump>) -> (Vec<MergedLump>, Analysis) {
    let analysis = analyze_directory(&entries);
    (entries, analysis)
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
