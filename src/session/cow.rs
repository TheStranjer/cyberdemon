//! Copy-on-write access to read-only archive content: bringing a whole map
//! block from the merged view into the working archive so it can be edited.
//!
//! The session's layers are read-only, so editing a map that only the `IWAD`
//! (or a resource `PWAD`) holds means copying it first. [`copy_map_block`]
//! appends one block — header lump, member lumps and any `GL` node block, in
//! the order the merged view shows them, so overrides from higher layers come
//! along as the merged bytes — to the end of the working archive. It is a
//! plain function over `&mut Wad` plus the merged directory, which keeps it
//! testable without a session; [`crate::session::Session::copy_map_to_working`]
//! is the undoable wrapper the tools reach for, and every map-editing tool
//! that meets a read-only map starts with the same copy.

use crate::error::CyberError;
use crate::session::{MergedLump, merge::analyze_directory};
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{MapBlock, analyze};
use crate::wad::name::LumpName;

/// Whether `wad`'s own directory already holds a map block named `map`.
///
/// A block means a header lump followed by map data (see
/// [`crate::wad::classify::analyze`]), so a loose `THINGS` does not count.
#[must_use]
#[inline]
pub fn has_map_block(wad: &Wad, map: LumpName) -> bool {
    analyze(&wad.lumps)
        .maps
        .iter()
        .any(|block| block.name == map)
}

/// Copies the map block named `map` from the merged view `merged` onto the
/// end of the working archive `working`.
///
/// The copy takes the block exactly as the merged view shows it, so member
/// lumps a higher layer overrode arrive with the override's bytes, and the
/// block's `GL` node run comes along with it. Nothing already in `working`
/// moves, and `merged` is never modified.
///
/// Returns `false` without changing anything when `working` already holds a
/// block of this name (it is the one the merged view shows, so it is already
/// editable), and `true` when the block was copied.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the map when neither the working archive
/// nor any layer holds such a block.
#[inline]
pub fn copy_map_block(
    working: &mut Wad,
    merged: &[MergedLump],
    map: LumpName,
) -> Result<bool, CyberError> {
    if has_map_block(working, map) {
        return Ok(false);
    }
    let block = merged_block(merged, map)?;
    let copies: Vec<Lump> = block_positions(&block)
        .into_iter()
        .filter_map(|position| merged.get(position))
        .map(|entry| entry.lump.clone())
        .collect();
    working.lumps.extend(copies);
    Ok(true)
}

/// The merged view's map block named `map`.
///
/// # Errors
///
/// [`CyberError::NotFound`] with a `list_lumps` hint when the view holds no
/// such block.
fn merged_block(merged: &[MergedLump], map: LumpName) -> Result<MapBlock, CyberError> {
    analyze_directory(merged)
        .maps
        .into_iter()
        .find(|block| block.name == map)
        .ok_or_else(|| {
            CyberError::NotFound(format!(
                "no map block named {map} in the loaded archives: list_lumps kind=map_marker \
                 shows the maps of the merged view"
            ))
        })
}

/// The merged-view positions one map block covers: header through last
/// member, plus its `GL` node block when it has one.
fn block_positions(block: &MapBlock) -> Vec<usize> {
    let mut positions: Vec<usize> = (block.header..block.end).collect();
    if let Some((start, end)) = block.gl {
        positions.extend(start..end);
    }
    positions
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{block_positions, copy_map_block, has_map_block};
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, MergedLump, Session, merge};
    use crate::wad::archive::{Lump, Wad, WadKind};
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

    /// A lump named `text` carrying `payload` as its bytes.
    fn carrying(text: &str, payload: &str) -> Lump {
        Lump::new(name(text), payload.as_bytes().to_vec())
    }

    /// A `PWAD` holding `lumps`.
    fn pwad(lumps: Vec<Lump>) -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps,
        }
    }

    /// The directory names of an archive, in order.
    fn names(wad: &Wad) -> Vec<String> {
        wad.lumps.iter().map(|lump| lump.name.to_string()).collect()
    }

    /// Each entry as `NAME(payload)`, so order, names and bytes show at once.
    fn with_payloads(wad: &Wad) -> Vec<String> {
        wad.lumps
            .iter()
            .map(|lump| {
                let text = String::from_utf8(lump.data.to_vec()).expect("test payloads are text");
                format!("{}({text})", lump.name)
            })
            .collect()
    }

    /// The payload of the first lump named `text`, as text.
    fn payload_of(wad: &Wad, text: &str) -> String {
        let lump = wad
            .lumps
            .iter()
            .find(|lump| lump.name == name(text))
            .unwrap_or_else(|| panic!("no lump named {text} in {:?}", names(wad)));
        String::from_utf8(lump.data.to_vec()).expect("test payloads are text")
    }

    /// A base layer: globals, one `MAP01` block with its `GL` run and one
    /// `MAP02` block without.
    fn base() -> Layer {
        Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    lump("PLAYPAL"),
                    marker("MAP01"),
                    lump("THINGS"),
                    lump("LINEDEFS"),
                    lump("SECTORS"),
                    marker("GL_MAP01"),
                    lump("GL_VERT"),
                    lump("GL_NODES"),
                    marker("MAP02"),
                    lump("THINGS"),
                    lump("PLAYPAL"),
                ],
            },
        )
    }

    /// The merged view of the base layer over `working`.
    fn merged_of(working: &Wad) -> Vec<MergedLump> {
        merge::merge_all(&[&base().wad], working)
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    #[test]
    fn has_map_block_needs_a_header_and_members() {
        let with_block = pwad(vec![marker("MAP01"), lump("THINGS")]);
        assert!(has_map_block(&with_block, name("MAP01")), "a real block");
        assert!(
            !has_map_block(&with_block, name("MAP02")),
            "another map is not this one"
        );
        let loose = pwad(vec![lump("THINGS"), lump("LINEDEFS")]);
        assert!(
            !has_map_block(&loose, name("THINGS")),
            "map data without a header is no block"
        );
        let header_only = pwad(vec![marker("MAP09")]);
        assert!(
            !has_map_block(&header_only, name("MAP09")),
            "a lone header names no map block"
        );
    }

    #[test]
    fn copy_map_block_appends_the_block_with_its_gl_nodes() {
        let mut working = Wad::default();
        let merged = merged_of(&working);
        let copied = copy_map_block(&mut working, &merged, name("MAP01")).expect("the base has it");
        assert!(copied, "the block was copied");
        assert_eq!(
            names(&working),
            vec![
                "MAP01", "THINGS", "LINEDEFS", "SECTORS", "GL_MAP01", "GL_VERT", "GL_NODES"
            ],
            "header through the last GL member, in view order"
        );
        assert_eq!(payload_of(&working, "THINGS"), "THINGS", "bytes came over");
    }

    #[test]
    fn copy_map_block_reports_false_when_the_working_archive_has_the_map() {
        let mut working = pwad(vec![marker("MAP01"), lump("THINGS")]);
        let before = names(&working);
        let merged = merged_of(&working);
        let copied = copy_map_block(&mut working, &merged, name("MAP01")).expect("already there");
        assert!(
            !copied,
            "the working block is the merged one, so nothing to copy"
        );
        assert_eq!(names(&working), before, "and the archive did not change");
    }

    #[test]
    fn copy_map_block_refuses_a_map_no_archive_holds() {
        let mut working = Wad::default();
        let merged = merged_of(&working);
        let error =
            copy_map_block(&mut working, &merged, name("MAP07")).expect_err("there is no MAP07");
        assert_eq!(code(&error), "not_found", "missing map code");
        assert!(
            error.to_string().contains("MAP07"),
            "the message names the map: {error}"
        );
        assert!(working.is_empty(), "nothing was appended");
    }

    #[test]
    fn copy_map_block_copies_the_winners_of_the_merged_view() {
        let mut session = Session::default();
        session.set_iwad(base());
        session.add_resource(Layer::new(
            PathBuf::from("fix.wad"),
            LayerRole::Resource,
            pwad(vec![
                marker("MAP02"),
                carrying("THINGS", "from-resource"),
                carrying("SECTORS", "sectors"),
            ]),
        ));
        session
            .edit("start a map", |wad| {
                wad.lumps.push(marker("MAP01"));
                wad.lumps.push(carrying("THINGS", "mine"));
                Ok(())
            })
            .expect("a plain append");
        let merged = session.merged();
        let mut working = session.working().clone();
        let copied = copy_map_block(&mut working, &merged, name("MAP02")).expect("layer 1 has it");
        assert!(copied, "MAP02 was not in the working archive yet");
        assert_eq!(
            with_payloads(&working),
            vec![
                "MAP01()",
                "THINGS(mine)",
                "MAP02()",
                "THINGS(from-resource)",
                "SECTORS(sectors)"
            ],
            "the copy carries the resource's bytes, the working archive keeps its own lumps"
        );
        assert!(
            !copy_map_block(&mut working, &merged, name("MAP01")).expect("already editable"),
            "the working archive's own MAP01 wins the view, so it needs no copy"
        );
    }

    #[test]
    fn block_positions_covers_the_gl_run_and_tolerates_an_absent_one() {
        let merged = merged_of(&Wad::default());
        let analysis = merge::analyze_directory(&merged);
        let with_gl = analysis
            .maps
            .iter()
            .find(|block| block.name == name("MAP01"))
            .expect("the base map");
        assert_eq!(
            block_positions(with_gl),
            vec![1_usize, 2, 3, 4, 5, 6, 7],
            "header, members, then the GL block"
        );
        let plain = analysis
            .maps
            .iter()
            .find(|block| block.name == name("MAP02"))
            .expect("the second map");
        assert_eq!(block_positions(plain), vec![8_usize, 9], "no GL run");
    }
}
