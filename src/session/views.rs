//! The session's read views as free functions over a `&Session`: the merged
//! directory, the layers' own directory, the game the archives identify, the
//! lumps of one [`Scope`] and the engine's lookup of a lump name.
//!
//! Each [`Session`] view method is a one-line delegate to the matching
//! function here, the way [`crate::session::cow`] and
//! [`crate::session::map_edit`] hold the bodies of the copy and edit steps:
//! the single `impl Session` block stays in `session.rs` and the machinery
//! that walks the layers lives here. Nothing in this module changes state;
//! every function reads the layers and the working archive as they stand.

use crate::error::CyberError;
use crate::session::merge;
use crate::session::{Layer, MergedLump, Scope, Session, Source};
use crate::wad::archive::Wad;
use crate::wad::classify::{LumpInfo, Namespace};
use crate::wad::game::{Game, detect_game};
use crate::wad::name::LumpName;

/// The directory an engine would see: every layer in load order with the
/// working archive on top, later archives overriding earlier ones.
///
/// The body of [`Session::merged`].
pub(super) fn merged(session: &Session) -> Vec<MergedLump> {
    let layers: Vec<&Wad> = session
        .layers()
        .into_iter()
        .map(|layer| &layer.wad)
        .collect();
    merge::merge_all(&layers, &session.working)
}

/// The merged view of the read-only layers alone: `IWAD` plus resources
/// folded in load order, with an empty archive where [`merged`] puts the
/// working one. This is what the layers provide before any of this session's
/// edits.
///
/// The body of [`Session::base_lumps`].
pub(super) fn base_lumps(session: &Session) -> Vec<MergedLump> {
    let layers: Vec<&Wad> = session
        .layers()
        .into_iter()
        .map(|layer| &layer.wad)
        .collect();
    merge::merge_all(&layers, &Wad::default())
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
/// tags its own layer and [`Scope::Merged`] returns [`merged`].
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
/// The body of [`Session::find`].
pub(super) fn find(
    session: &Session,
    name: LumpName,
    namespace: Option<Namespace>,
) -> Option<MergedLump> {
    let view = merged(session);
    let analysis = merge::analyze_directory(&view);
    view.iter()
        .zip(analysis.lumps.iter())
        .rev()
        .find(|pair| is_wanted(pair.0, pair.1, name, namespace))
        .map(|pair| pair.0.clone())
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
