//! Reading one scope as a directory: the map-block lookup and decode every
//! map tool and the 3D scene builder start from.
//!
//! `Directory` wraps a [`Scope`]'s classified listing;
//! `resolve_map_name` picks the map a call works on and `load_map`
//! decodes one block out of a scope. They live here, beside the session they
//! read, so [`crate::render::scene`] does not reach into the tool layer to
//! load a map; the `maps` tools re-export them and add their own single
//! locked read of a map with the session's definitions on top.

use crate::error::CyberError;
use crate::session::{MergedLump, Scope, Session};
use crate::wad::archive::Lump;
use crate::wad::classify::{Analysis, MapBlock};
use crate::wad::lumps;
use crate::wad::map::Map;
use crate::wad::map::read_map;
use crate::wad::name::LumpName;

/// One scope read as a directory: the tagged entries, the plain lumps
/// (index-aligned with them) and the [`analyze`] result over the listing.
pub(crate) struct Directory {
    /// The scope's lumps with their origins, in directory order.
    pub(crate) entries: Vec<MergedLump>,
    /// The same lumps without origin tags, index-aligned with `entries`.
    pub(crate) lumps: Vec<Lump>,
    /// The classification of `lumps`.
    pub(crate) analysis: Analysis,
}

impl Directory {
    /// The map block named `name`; when the scope holds two blocks with one
    /// name, the last in directory order wins, the way an engine reads it.
    pub(crate) fn find_block(&self, name: LumpName) -> Option<&MapBlock> {
        self.analysis
            .maps
            .iter()
            .rev()
            .find(|block| block.name == name)
    }

    /// The directory positions one block covers past its header: the member
    /// lumps first, then the attached `GL` node run when there is one.
    pub(crate) fn block_positions(block: &MapBlock) -> Vec<usize> {
        let members = block.header.saturating_add(1)..block.end;
        let Some((gl_start, gl_end)) = block.gl else {
            return members.collect();
        };
        members.chain(gl_start..gl_end).collect()
    }

    /// The member lump names of `block`, [`Self::block_positions`] spelled
    /// out, for `list_maps` rows.
    pub(crate) fn block_lump_names(&self, block: &MapBlock) -> Vec<String> {
        self.block_lumps(block)
            .map(|lump| lump.name.to_string())
            .collect()
    }

    /// The lumps of [`Self::block_positions`], in directory order.
    fn block_lumps<'dir>(&'dir self, block: &MapBlock) -> impl Iterator<Item = &'dir Lump> {
        Self::block_positions(block)
            .into_iter()
            .filter_map(|position| self.lumps.get(position))
    }

    /// Whether `block` carries node data: a non-empty `NODES` lump, or a
    /// `ZNODES` lump (the `UDMF` node run, present is enough).
    pub(crate) fn block_has_nodes(&self, block: &MapBlock) -> bool {
        self.block_lumps(block).any(is_node_lump)
    }
}

/// Whether a block member is node data: a name from
/// [`lumps::NODE_DATA_LUMPS`], where `ZNODES` counts by mere presence
/// and `NODES` only when it carries bytes (an empty one is an
/// invalidated placeholder).
fn is_node_lump(lump: &Lump) -> bool {
    lumps::NODE_DATA_LUMPS.contains(&lump.name) && (lump.name == lumps::ZNODES || lump.size() > 0)
}

/// Read `scope` into a [`Directory`].
///
/// The entries and their classification come from [`Session::analyzed`]:
/// the merged scope is served from the session's read cache, and the
/// working and layer scopes are classified here once instead of every
/// caller re-analyzing the listing it was handed.
///
/// # Errors
///
/// [`Session::analyzed`]'s [`CyberError::NotFound`] for a [`Scope::Layer`]
/// that is not loaded.
pub(crate) fn directory(session: &Session, scope: Scope) -> Result<Directory, CyberError> {
    let (entries, analysis) = session.analyzed(scope)?;
    let lumps: Vec<Lump> = entries.iter().map(|entry| entry.lump.clone()).collect();
    Ok(Directory {
        entries,
        lumps,
        analysis,
    })
}

/// The map one tool should act on: the explicit `requested` name (parsed
/// strictly as a [`LumpName`]), else the session's active map, else the
/// merged view's only map when it holds exactly one.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a name that does not parse, and for
/// the no-default case (no name, no active map, merged view empty or with
/// several maps).
pub(crate) fn resolve_map_name(
    session: &Session,
    requested: Option<&str>,
) -> Result<LumpName, CyberError> {
    if let Some(text) = requested {
        return LumpName::parse(text);
    }
    if let Some(active) = session.active_map() {
        return Ok(active);
    }
    let dir = directory(session, Scope::Merged)?;
    let mut blocks = dir.analysis.maps.iter();
    let only = blocks.next().filter(|_| blocks.next().is_none());
    only.map_or_else(|| Err(no_map_error()), |block| Ok(block.name))
}

/// The refusal when no map was given, none is active and the merged view
/// does not hold exactly one map.
fn no_map_error() -> CyberError {
    CyberError::InvalidArgument(
        "no map given and no active map; pass map or call set_active_map".to_owned(),
    )
    .with_hint(MAP_LIST_HINT)
}

/// The hint attached to every refusal about a map the session does not
/// name: list what exists, or make a new one.
const MAP_LIST_HINT: &str = "list_maps shows the maps; create_map makes a new one";

/// Read the map named `name` out of `scope`, decoding its block.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the scope holds no such block,
/// [`CyberError::Format`] when the block's decoder rejects its members, and
/// [`Session::lumps_in`]'s errors for an unloaded layer scope.
pub(crate) fn load_map(session: &Session, name: LumpName, scope: Scope) -> Result<Map, CyberError> {
    let dir = directory(session, scope)?;
    let Some(block) = dir.find_block(name) else {
        return Err(missing_map(name, scope));
    };
    read_map(&dir.lumps, block)
}

/// The `not_found` refusal for a map a scope does not hold.
pub(crate) fn missing_map(name: LumpName, scope: Scope) -> CyberError {
    CyberError::NotFound(format!("no map block named {name} in the {scope} scope"))
        .with_hint(MAP_LIST_HINT)
}
