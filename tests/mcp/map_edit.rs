//! The map-editing infrastructure as seen from outside the crate:
//! [`cyberdemon::session::Session::edit_map`] run over archives read from
//! fixture files — copy-on-write into the working archive, node lump
//! invalidation, undo and a serialize round trip — plus the shared editing
//! parameter helpers in [`cyberdemon::tools::maps::params`].

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use cyberdemon::error::CyberError;
    use cyberdemon::session::{Layer, LayerRole, Session};
    use cyberdemon::tools::maps::params::{
        ElementKind, FlagsInput, IndexSelector, check_index, edit_result_fragment,
        validate_texture_name,
    };
    use cyberdemon::wad::classify::MapFormat;
    use cyberdemon::wad::map::{FlagFamily, FlagKind, Map, read_map};
    use cyberdemon::wad::{LumpName, Wad, analyze};
    use serde_json::{json, to_value};

    /// A valid parsed name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("fixture names are valid")
    }

    /// A session whose base layer is the archive stored at `path`.
    fn session_over(path: &Path) -> Session {
        let wad = Wad::read_file(path).expect("the fixture archive parses");
        let mut session = Session::default();
        session
            .set_iwad(Layer::new(path.to_path_buf(), LayerRole::Iwad, wad))
            .expect("no batch is open");
        session
    }

    /// The working archive's decoded block named `text`.
    fn working_map(session: &Session, text: &str) -> Map {
        let lumps = &session.working().lumps;
        let analysis = analyze(lumps);
        let block = analysis
            .maps
            .iter()
            .rev()
            .find(|block| block.name == name(text))
            .unwrap_or_else(|| panic!("no {text} block in the working archive"));
        read_map(lumps, block).expect("the edited block decodes")
    }

    /// An edit closure raising sector 0's floor to 48.
    fn raise_floor(map: &mut Map) -> Result<(), CyberError> {
        let sector = map
            .sectors
            .first_mut()
            .ok_or_else(|| CyberError::InvalidArgument("the map has no sectors".to_owned()))?;
        sector.floor_height = 48_i32;
        Ok(())
    }

    /// An edit closure moving vertex 0 by 100 units in `x`.
    fn move_vertex(map: &mut Map) -> Result<(), CyberError> {
        let vertex = map
            .vertices
            .first_mut()
            .ok_or_else(|| CyberError::InvalidArgument("the map has no vertices".to_owned()))?;
        vertex.x = vertex.x.checked_add(100_i32).expect("the move fits");
        Ok(())
    }

    /// The directory names of the working archive.
    fn working_names(session: &Session) -> Vec<String> {
        session
            .working()
            .lumps
            .iter()
            .map(|lump| lump.name.to_string())
            .collect()
    }

    #[test]
    fn a_property_edit_copies_in_undoes_wholly_and_leaves_the_iwad_file() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = crate::fixtures::iwad(dir.path());
        let before = fs::read(&base).expect("the fixture file reads");
        let mut session = session_over(&base);
        let ((), report) = session
            .edit_map(name("MAP01"), "raise hall floor", raise_floor)
            .expect("the edit lands");
        assert!(report.copied_to_working, "the IWAD map was copied in");
        assert!(
            !report.nodes_invalidated,
            "a height change keeps the node lumps"
        );
        assert_eq!(
            to_value(edit_result_fragment(&report)).expect("the fragment serializes"),
            json!({
                "map": "MAP01",
                "nodes_invalidated": false,
                "copied_to_working": true,
            }),
            "the standard fragment"
        );
        let edited = working_map(&session, "MAP01");
        assert_eq!(
            edited.sectors.first().expect("sector 0").floor_height,
            48_i32
        );
        let bytes = session
            .working()
            .to_bytes()
            .expect("the archive serializes");
        let reread = Wad::parse(&bytes).expect("and parses back");
        let analysis = analyze(&reread.lumps);
        let block = analysis.maps.first().expect("the copied block");
        let map = read_map(&reread.lumps, block).expect("it decodes");
        assert_eq!(map, edited, "the on-disk shape matches the session");
        assert_eq!(session.undo_labels(), vec!["raise hall floor"], "one step");
        session.undo().expect("undo it");
        assert!(session.working().is_empty(), "copy and edit rolled back");
        assert_eq!(
            fs::read(&base).expect("the fixture file reads"),
            before,
            "the IWAD file on disk never changed"
        );
    }

    #[test]
    fn a_geometry_edit_drops_znodes_and_the_gl_run_of_the_udmf_block() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let formats = crate::fixtures::pwad_map_formats(dir.path());
        let mut session = session_over(&formats);
        let ((), report) = session
            .edit_map(name("MAP03"), "move corner", move_vertex)
            .expect("the edit lands");
        assert!(report.nodes_invalidated, "moving a vertex is geometry");
        let names = working_names(&session);
        assert!(
            !names
                .iter()
                .any(|lump| lump == "ZNODES" || lump.starts_with("GL_")),
            "ZNODES and the GL run are gone: {names:?}"
        );
        assert!(
            names.contains(&"TEXTMAP".to_owned()) && names.contains(&"ENDMAP".to_owned()),
            "the UDMF block still frames its TEXTMAP: {names:?}"
        );
        let map = working_map(&session, "MAP03");
        assert_eq!(
            map.format,
            MapFormat::Udmf,
            "the block still decodes as UDMF"
        );
        assert!(map.other.is_empty(), "no node carriers survived");
        let moved = map.vertices.first().expect("vertex 0").x;
        let original = crate::fixtures::two_room_map();
        assert_eq!(
            moved,
            original
                .vertices
                .first()
                .expect("the fixture's vertex 0")
                .x
                .checked_add(100)
                .expect("the move fits"),
            "the edit itself landed"
        );
        assert_eq!(
            map.linedefs.len(),
            original.linedefs.len(),
            "the rest came along"
        );
    }

    #[test]
    fn the_params_helpers_validate_inputs_through_the_public_api() {
        let bits = FlagsInput::Names(vec!["twosided".to_owned(), "mapped".to_owned()])
            .resolve(FlagKind::Linedef, FlagFamily::Doom)
            .expect("two known names");
        assert_eq!(bits, 260, "4 | 256");
        assert_eq!(
            IndexSelector::Many(vec![3, 1, 3])
                .indices()
                .expect("a deduplicated list"),
            vec![1_usize, 3],
            "sorted and deduplicated"
        );
        let error = check_index(ElementKind::Linedefs, 12, 7).expect_err("12 is out of range");
        assert_eq!(error.code(), "not_found", "out of range code");
        assert_eq!(
            error.message(),
            "linedef 12 does not exist (the map has 7 linedefs, valid indices are 0..=6)",
            "the documented phrasing"
        );
        assert_eq!(
            error.hint(),
            "describe_map or find_elements returns indices",
            "the documented hint"
        );
        assert_eq!(
            validate_texture_name("step1", MapFormat::Doom).expect("a vanilla name"),
            "STEP1"
        );
    }
}
