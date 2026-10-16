//! The shared map helpers behind every tool that acts on a map: name
//! resolution ([`resolve_map_name`]) and reading one map block out of a
//! scope ([`load_map`], through the [`Directory`] wrapper around a scope's
//! classified listing). Every later map tool takes an optional `map`
//! parameter and starts with these two.

use crate::error::CyberError;
use crate::session::{MergedLump, Scope, Session};
use crate::wad::archive::Lump;
use crate::wad::classify::{Analysis, MapBlock, analyze};
use crate::wad::map::Map;
use crate::wad::map::read_map;
use crate::wad::name::LumpName;

/// One scope read as a directory: the tagged entries, the plain lumps
/// (index-aligned with them) and the [`analyze`] result over the listing.
pub(in crate::tools) struct Directory {
    /// The scope's lumps with their origins, in directory order.
    pub(in crate::tools) entries: Vec<MergedLump>,
    /// The same lumps without origin tags, index-aligned with `entries`.
    pub(in crate::tools) lumps: Vec<Lump>,
    /// The classification of `lumps`.
    pub(in crate::tools) analysis: Analysis,
}

impl Directory {
    /// The map block named `name`; when the scope holds two blocks with one
    /// name, the last in directory order wins, the way an engine reads it.
    pub(in crate::tools) fn find_block(&self, name: LumpName) -> Option<&MapBlock> {
        self.analysis
            .maps
            .iter()
            .rev()
            .find(|block| block.name == name)
    }

    /// The directory positions one block covers past its header: the member
    /// lumps first, then the attached `GL` node run when there is one.
    pub(in crate::tools) fn block_positions(block: &MapBlock) -> Vec<usize> {
        let members = block.header.saturating_add(1)..block.end;
        let Some((gl_start, gl_end)) = block.gl else {
            return members.collect();
        };
        members.chain(gl_start..gl_end).collect()
    }

    /// The member lump names of `block`, [`Self::block_positions`] spelled
    /// out, for `list_maps` rows.
    pub(in crate::tools) fn block_lump_names(&self, block: &MapBlock) -> Vec<String> {
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
    pub(in crate::tools) fn block_has_nodes(&self, block: &MapBlock) -> bool {
        self.block_lumps(block).any(is_node_lump)
    }
}

/// Whether a block member is node data.
fn is_node_lump(lump: &Lump) -> bool {
    let name = lump.name.as_bytes();
    name == b"ZNODES" || (name == b"NODES" && lump.size() > 0)
}

/// Read `scope` into a [`Directory`].
///
/// # Errors
///
/// [`Session::lumps_in`]'s [`CyberError::NotFound`] for a [`Scope::Layer`]
/// that is not loaded.
pub(in crate::tools) fn directory(
    session: &Session,
    scope: Scope,
) -> Result<Directory, CyberError> {
    let entries = session.lumps_in(scope)?;
    let lumps: Vec<Lump> = entries.iter().map(|entry| entry.lump.clone()).collect();
    let analysis = analyze(&lumps);
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
pub(in crate::tools) fn resolve_map_name(
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
        "no map given and no active map; pass map or call set_active_map (list_maps shows the \
         maps of the merged view)"
            .to_owned(),
    )
}

/// Read the map named `name` out of `scope`, decoding its block.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the scope holds no such block,
/// [`CyberError::Format`] when the block's decoder rejects its members, and
/// [`Session::lumps_in`]'s errors for an unloaded layer scope.
pub(in crate::tools) fn load_map(
    session: &Session,
    name: LumpName,
    scope: Scope,
) -> Result<Map, CyberError> {
    let dir = directory(session, scope)?;
    let Some(block) = dir.find_block(name) else {
        return Err(missing_map(name, scope));
    };
    read_map(&dir.lumps, block)
}

/// The `not_found` refusal for a map a scope does not hold.
pub(in crate::tools) fn missing_map(name: LumpName, scope: Scope) -> CyberError {
    CyberError::NotFound(format!(
        "no map block named {name} in the {} scope: list_maps shows the maps of each scope",
        scope_label(scope)
    ))
}

/// The scope's wire name, for messages.
fn scope_label(scope: Scope) -> String {
    match scope {
        Scope::Merged => "merged".to_owned(),
        Scope::Working => "working".to_owned(),
        Scope::Layer(index) => format!("layer:{index}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Directory, directory, load_map, resolve_map_name};
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::MapFormat;
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A lump named `text` holding `payload`.
    fn lump(text: &str, payload: &[u8]) -> Lump {
        Lump::new(name(text), payload.to_vec())
    }

    /// A minimal decodable binary `Doom` block named `text`.
    fn doom_block(text: &str) -> Vec<Lump> {
        vec![
            marker(text),
            marker("THINGS"),
            marker("LINEDEFS"),
            marker("SIDEDEFS"),
            marker("VERTEXES"),
            marker("SECTORS"),
            marker("NODES"),
        ]
    }

    /// A base layer holding `lumps`.
    fn base(lumps: Vec<Lump>) -> Layer {
        Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        )
    }

    /// A session over a base layer of `lumps`.
    fn session_over(lumps: Vec<Lump>) -> Session {
        let mut session = Session::default();
        session.set_iwad(base(lumps));
        session
    }

    #[test]
    fn resolve_map_name_prefers_explicit_then_active_then_the_only_map() {
        let session = session_over(doom_block("MAP01"));
        assert_eq!(
            resolve_map_name(&session, Some("map07")).expect("explicit wins"),
            name("MAP07"),
            "an explicit name is used as given, uppercased",
        );
        assert_eq!(
            resolve_map_name(&session, None).expect("the only map is the default"),
            name("MAP01"),
            "one map in the merged view needs no selection",
        );
        let mut busy = session_over([doom_block("MAP01"), doom_block("MAP02")].concat());
        let error = resolve_map_name(&busy, None).expect_err("two maps, no default");
        assert_eq!(error.code(), "invalid_argument", "no-default code");
        assert!(
            error
                .to_string()
                .contains("pass map or call set_active_map"),
            "the error names the fixes: {error}"
        );
        busy.set_active_map(Some(name("MAP02")));
        assert_eq!(
            resolve_map_name(&busy, None).expect("the active map is the default"),
            name("MAP02"),
            "the active map wins over an ambiguous view",
        );
        assert_eq!(
            resolve_map_name(&busy, Some("MAP01")).expect("explicit still wins"),
            name("MAP01"),
            "and an explicit name still beats it",
        );
    }

    #[test]
    fn resolve_map_name_refuses_unparsable_names_and_an_empty_session() {
        let session = Session::default();
        let empty = resolve_map_name(&session, None).expect_err("no maps at all");
        assert_eq!(empty.code(), "invalid_argument", "empty view code");
        let long =
            resolve_map_name(&session, Some("MAP012345")).expect_err("nine characters is over");
        assert_eq!(long.code(), "invalid_argument", "oversized name code");
        let spaced = resolve_map_name(&session, Some("MAP 1")).expect_err("no spaces in names");
        assert_eq!(spaced.code(), "invalid_argument", "bad character code");
    }

    #[test]
    fn load_map_decodes_the_named_block_of_either_scope() {
        let session = session_over(doom_block("MAP01"));
        let merged = load_map(&session, name("MAP01"), Scope::Merged).expect("merged reads");
        assert_eq!(merged.name, name("MAP01"), "the header names the map");
        assert_eq!(merged.format, MapFormat::Doom, "a plain binary block");
        let layer = load_map(&session, name("MAP01"), Scope::Layer(0)).expect("layer 0 reads");
        assert_eq!(layer, merged, "the same block through a layer scope");
        let error =
            load_map(&session, name("MAP01"), Scope::Working).expect_err("working is empty");
        assert_eq!(error.code(), "not_found", "missing in working");
        assert!(
            error.to_string().contains("working"),
            "the message names the scope: {error}"
        );
    }

    #[test]
    fn load_map_reports_a_missing_map_and_an_unloaded_layer() {
        let session = session_over(doom_block("MAP01"));
        let error = load_map(&session, name("MAP09"), Scope::Merged).expect_err("no MAP09 here");
        assert_eq!(error.code(), "not_found", "missing map code");
        assert!(
            error.to_string().contains("MAP09") && error.to_string().contains("merged"),
            "the message names the map and the scope: {error}"
        );
        let layer =
            load_map(&session, name("MAP01"), Scope::Layer(3)).expect_err("only layer 0 is loaded");
        assert_eq!(layer.code(), "not_found", "unloaded layer code");
    }

    /// The directory of a base archive carrying two blocks named `MAP01`
    /// (seven lumps each) plus a trailing `GL_MAP01` run.
    fn doubled() -> Directory {
        let session = session_over(
            [
                doom_block("MAP01"),
                doom_block("MAP01"),
                vec![marker("GL_MAP01"), lump("GL_VERT", b"gl")],
            ]
            .concat(),
        );
        directory(&session, Scope::Layer(0)).expect("layer 0 is the base")
    }

    #[test]
    fn find_block_takes_the_last_of_two_same_named_blocks() {
        let dir = doubled();
        let block = dir.find_block(name("MAP01")).expect("MAP01 is there");
        assert_eq!(block.header, 7, "the second block wins for the name");
        assert_eq!(block.gl, None, "the GL run attached to the first block");
        let first = dir
            .analysis
            .maps
            .first()
            .expect("the first block is still listed");
        assert_eq!(first.gl, Some((14_usize, 16)), "attached by name");
    }

    #[test]
    fn load_map_names_the_map_it_decoded() {
        let session = session_over([doom_block("MAP01"), doom_block("MAP02")].concat());
        let loaded =
            load_map(&session, name("MAP02"), Scope::Layer(0)).expect("the named block reads");
        assert_eq!(loaded.name, name("MAP02"), "the codec got the name");
        assert_eq!(loaded.format, MapFormat::Doom, "and the format");
    }

    #[test]
    fn block_positions_and_names_cover_the_members_and_the_gl_run() {
        let dir = doubled();
        let block = dir.analysis.maps.first().expect("the first block");
        assert_eq!(
            dir.block_lump_names(block),
            vec![
                "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SECTORS", "NODES", "GL_MAP01",
                "GL_VERT",
            ],
            "members then the GL block, without the map header",
        );
    }

    #[test]
    fn block_has_nodes_needs_data_or_a_znodes_lump() {
        let empty_nodes = session_over(doom_block("MAP01"));
        let dir = directory(&empty_nodes, Scope::Layer(0)).expect("layer 0");
        let block = dir.find_block(name("MAP01")).expect("the block");
        assert!(
            !dir.block_has_nodes(block),
            "an empty NODES is no node data",
        );
        let with_znodes = session_over(vec![
            marker("MAP02"),
            lump("TEXTMAP", b"namespace = \"zdoom\";"),
            marker("ZNODES"),
            marker("ENDMAP"),
        ]);
        let zdir = directory(&with_znodes, Scope::Layer(0)).expect("layer 0");
        let zblock = zdir.find_block(name("MAP02")).expect("the UDMF block");
        assert!(
            zdir.block_has_nodes(zblock),
            "a ZNODES lump counts even when empty",
        );
        let filled = session_over(vec![
            marker("MAP03"),
            lump("THINGS", b"t"),
            lump("NODES", b"ndata"),
        ]);
        let fdir = directory(&filled, Scope::Layer(0)).expect("layer 0");
        let fblock = fdir.find_block(name("MAP03")).expect("the block");
        assert!(fdir.block_has_nodes(fblock), "a non-empty NODES counts");
    }
}
