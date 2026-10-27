//! The shared map helpers behind every tool that acts on a map.
//!
//! The block-level machinery - the `map`-parameter resolvers
//! (`resolve_map_name`, `load_map`) and the `Directory` wrapper around a
//! scope's classified listing - lives beside the session it reads, in
//! [`crate::session::mapdir`]; this module re-exports it so every map tool
//! keeps reaching it at [`crate::tools::maps::common`]. On top of those,
//! `load` adds [`crate::session::Session::definitions`] in the same lock
//! and answers with the `Prepared` pair the analysing tools
//! (`find_elements`) read from; the `playability` group pairs the same
//! read with the render settings under its own `load`.

use crate::defs::Definitions;
use crate::error::CyberError;
use crate::session::Scope;
pub(crate) use crate::session::mapdir::load_map;
pub(in crate::tools) use crate::session::mapdir::{
    Directory, directory, missing_map, resolve_map_name,
};
use crate::state::AppState;
use crate::wad::map::Map;

/// The map and the session's definitions, read together out of one
/// session lock: what the analysing tool (`find_elements`) runs on.
#[derive(Debug)]
pub(crate) struct Prepared {
    /// The map to read, decoded from the merged view; its `name` carries
    /// what the call resolved.
    pub(crate) map: Map,
    /// The session's definitions: custom `DECORATE`/`DEHACKED` things
    /// included, exactly what the sprite renderers dress from.
    pub(crate) defs: Definitions,
}

/// Read the map named by an optional `map` parameter (or the session's
/// default) from the merged view together with the session's definitions,
/// all under the session lock.
///
/// # Errors
///
/// [`resolve_map_name`] and [`load_map`]'s errors and
/// [`crate::session::Session::definitions`]' collection errors.
pub(crate) fn load(state: &AppState, requested: Option<&str>) -> Result<Prepared, CyberError> {
    state.with_session(|session| {
        let name = resolve_map_name(session, requested)?;
        let map = load_map(session, name, Scope::Merged)?;
        Ok(Prepared {
            map,
            defs: session.definitions()?,
        })
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Directory, directory, load, load_map, resolve_map_name};
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::state::AppState;
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
        session.set_iwad(base(lumps)).expect("no batch is open");
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
    fn load_reads_the_map_with_the_session_definitions_under_one_lock() {
        let state = AppState::default();
        state
            .with_session(|session| {
                session
                    .set_iwad(base(doom_block("MAP01")))
                    .expect("no batch is open");
                Ok(())
            })
            .expect("seeding the base layer");
        let prepared = load(&state, None).expect("the one map resolves");
        assert_eq!(prepared.map.name, name("MAP01"), "the resolved map");
        assert_eq!(
            prepared.defs.game,
            crate::wad::game::Game::Doom2,
            "an unidentified session stands for DOOM II"
        );
        let missing = load(&state, Some("MAP09")).expect_err("no MAP09 here");
        assert_eq!(
            missing.code(),
            "not_found",
            "the missing-map refusal surfaces"
        );
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
