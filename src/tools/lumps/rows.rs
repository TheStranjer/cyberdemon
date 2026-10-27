//! The shared directory-row builder behind the `lumps` tools.
//!
//! [`build_rows`] lists one scope's lumps in directory order and gives every
//! entry the shared row shape: its position in the scope, name, size, the
//! byte offset `save_wad` would give its data in the source archive, the
//! [`analyze`] classification (`kind`, `namespace`, `map`) and the [`Source`]
//! it came from. Later tools (`diff_wads`, graphics listings) reuse the same
//! rows.

use serde::Serialize;

use crate::error::CyberError;
use crate::session::{MergedLump, Scope, Session, Source};
use crate::wad::archive::Lump;
use crate::wad::classify::{LumpInfo, LumpKind, Namespace};
use crate::wad::name::LumpName;

/// One lump of a listed scope, in the shared row shape.
#[derive(Clone, Debug, Serialize)]
pub(in crate::tools) struct LumpRow {
    /// Position in the listed scope (the merged-view index for `merged`).
    pub(in crate::tools) index: usize,
    /// Normalised lump name.
    pub(in crate::tools) name: LumpName,
    /// Payload size in bytes.
    pub(in crate::tools) size: usize,
    /// Byte offset of the data inside the source archive, as `save_wad`
    /// would lay that archive out.
    pub(in crate::tools) offset: u32,
    /// Type hint from [`analyze`].
    pub(in crate::tools) kind: LumpKind,
    /// Namespace region the lump sits in, if any.
    pub(in crate::tools) namespace: Option<Namespace>,
    /// Map block the lump belongs to, if any.
    pub(in crate::tools) map: Option<LumpName>,
    /// Archive the lump was taken from.
    pub(in crate::tools) source: Source,
    /// The lump itself, for content searches; never serialized.
    #[serde(skip)]
    pub(in crate::tools) lump: Lump,
}

/// The save-layout data offsets of the archives a scope lists, one table
/// per source.
type OffsetTables = Vec<(Source, Vec<u32>)>;

/// The rows for every lump of `scope`, in directory order, classified over
/// the full scope listing so indices and namespace regions match what an
/// engine (and [`crate::session::Session::merged`]) sees. The listing and
/// its classification come from [`Session::analyzed`]: the merged scope is
/// served from the session's read cache instead of a fresh merge plus a
/// re-analysis of every lump.
///
/// # Errors
///
/// [`CyberError::NotFound`] for an out-of-range layer scope (through
/// [`Session::analyzed`]) and [`CyberError::LimitExceeded`] when a source
/// archive no longer fits the `WAD` layout.
pub(in crate::tools) fn build_rows(
    session: &Session,
    scope: Scope,
) -> Result<Vec<LumpRow>, CyberError> {
    let (entries, analysis) = session.analyzed(scope)?;
    let tables = source_offsets(session, &entries)?;
    entries
        .iter()
        .zip(analysis.lumps.iter())
        .map(|(entry, info)| row(entry, info, &tables))
        .collect()
}

/// The save-layout data offsets of every archive `entries` come from, one
/// table per distinct source, in entry order.
///
/// # Errors
///
/// Whatever [`archive_offsets`] reports for a source's archive.
fn source_offsets(session: &Session, entries: &[MergedLump]) -> Result<OffsetTables, CyberError> {
    let mut tables: OffsetTables = Vec::new();
    for entry in entries {
        if tables.iter().any(|table| table.0 == entry.source) {
            continue;
        }
        tables.push((entry.source, archive_offsets(session, entry.source)?));
    }
    Ok(tables)
}

/// The save-layout data offsets of the archive `source` names.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the archive does not fit the `WAD`
/// layout, [`CyberError::Internal`] when a [`Source::Layer`] names a layer
/// that is not loaded.
fn archive_offsets(session: &Session, source: Source) -> Result<Vec<u32>, CyberError> {
    match source {
        Source::Working => session.working().data_offsets(),
        Source::Layer(index) => session.layers().get(index).map_or_else(
            || {
                Err(CyberError::Internal(format!(
                    "source layer {index} is not loaded; list \"working\" or a layer index \
                     below the loaded count"
                )))
            },
            |layer| layer.wad.data_offsets(),
        ),
    }
}

/// The data offset of one entry: the position its source archive's
/// save-layout table gives for [`MergedLump::source_index`].
///
/// # Errors
///
/// [`CyberError::Internal`] when no table holds the entry's source or the
/// source index runs past the table.
fn entry_offset(tables: &OffsetTables, entry: &MergedLump) -> Result<u32, CyberError> {
    let found = tables
        .iter()
        .find(|table| table.0 == entry.source)
        .and_then(|table| table.1.get(entry.source_index))
        .copied();
    found.ok_or_else(|| {
        CyberError::Internal(format!(
            "lump {} has no data offset in its source archive",
            entry.lump.name
        ))
    })
}

/// One row: the classification of `info`, the origin and data offset of
/// `entry`.
///
/// # Errors
///
/// Whatever [`entry_offset`] reports for the entry's position.
fn row(entry: &MergedLump, info: &LumpInfo, tables: &OffsetTables) -> Result<LumpRow, CyberError> {
    Ok(LumpRow {
        index: info.index,
        name: info.name,
        size: info.size,
        offset: entry_offset(tables, entry)?,
        kind: info.kind,
        namespace: info.namespace,
        map: info.map,
        source: entry.source,
        lump: entry.lump.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::session::{Layer, LayerRole, MergedLump, Scope, Session, Source};
    use crate::tools::lumps::rows::{archive_offsets, build_rows, entry_offset};
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::{LumpInfo, LumpKind, Namespace};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` holding `size` bytes of `fill`.
    fn filled(text: &str, fill: u8, size: usize) -> Lump {
        Lump::new(name(text), vec![fill; size])
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A base layer: a 4-byte global, a flats region with a 2-byte flat,
    /// and one empty map block.
    fn base() -> Layer {
        Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![
                    filled("GAMEINFO", b'a', 4),
                    marker("F_START"),
                    filled("FLATA", b'b', 2),
                    marker("F_END"),
                    marker("MAP01"),
                    marker("THINGS"),
                ],
            },
        )
    }

    /// A session with the base layer and a working archive adding a sprites
    /// region.
    fn session() -> Session {
        let mut session = Session::default();
        session.set_iwad(base()).expect("no batch is open");
        session
            .edit("add sprites", |wad| {
                wad.lumps.push(marker("S_START"));
                wad.lumps.push(filled("SPR1A1", b'c', 3));
                wad.lumps.push(marker("S_END"));
                Ok(())
            })
            .expect("plain appends");
        session
    }

    /// A classification entry matching a bare tagged lump.
    fn info_at(index: usize, lump: &Lump) -> LumpInfo {
        LumpInfo {
            index,
            name: lump.name,
            size: lump.size(),
            kind: LumpKind::Unknown,
            namespace: None,
            map: None,
        }
    }

    /// An entry tagged from `source` at `source_index`, payload `lump`.
    fn tagged(lump: Lump, source: Source, source_index: usize) -> MergedLump {
        MergedLump {
            lump,
            source,
            source_index,
        }
    }

    #[test]
    fn merged_rows_classify_the_view_and_track_sources() {
        let rows = build_rows(&session(), Scope::Merged).expect("merged rows");
        let flat = rows
            .iter()
            .find(|row| row.name == name("FLATA"))
            .expect("the base flat");
        assert_eq!(flat.index, 2, "position in the merged view");
        assert_eq!(flat.kind, LumpKind::Flat, "classified in its region");
        assert_eq!(flat.namespace, Some(Namespace::Flats), "flats region");
        assert_eq!(flat.map, None, "not map data");
        assert_eq!(flat.source, Source::Layer(0), "still the base's copy");
        assert_eq!(flat.offset, 16, "after the header and GAMEINFO's 4 bytes");
        let things = rows
            .iter()
            .find(|row| row.name == name("THINGS"))
            .expect("map member");
        assert_eq!(things.kind, LumpKind::MapData, "classified by block");
        assert_eq!(things.map, Some(name("MAP01")), "owned by the map");
        assert_eq!(things.offset, 18, "markers are empty");
        let sprite = rows
            .iter()
            .find(|row| row.name == name("SPR1A1"))
            .expect("the working sprite");
        assert_eq!(
            sprite.source,
            Source::Working,
            "came from the working archive"
        );
        assert_eq!(sprite.offset, 12, "first data in the working layout");
        assert_eq!(sprite.kind, LumpKind::Sprite, "inside the sprites region");
    }

    #[test]
    fn working_and_layer_rows_read_the_single_archive() {
        let working = build_rows(&session(), Scope::Working).expect("working rows");
        assert_eq!(working.len(), 3, "the three appended lumps");
        assert!(
            working.iter().all(|row| row.source == Source::Working),
            "every working row is tagged working"
        );
        assert_eq!(
            working.first().expect("a row").index,
            0,
            "scope-local indices restart"
        );
        let layer = build_rows(&session(), Scope::Layer(0)).expect("layer rows");
        assert_eq!(layer.len(), 6, "the base archive unchanged");
        assert!(
            layer.iter().all(|row| row.source == Source::Layer(0)),
            "every layer row is tagged the layer"
        );
    }

    #[test]
    fn a_layer_out_of_range_is_not_found() {
        let error = build_rows(&session(), Scope::Layer(5)).expect_err("only one layer");
        assert_eq!(error.code(), "not_found", "out of range layer code");
    }

    #[test]
    fn archive_offsets_reports_a_vanished_layer_as_internal() {
        let error = archive_offsets(&session(), Source::Layer(9)).expect_err("no layer 9");
        assert_eq!(error.code(), "internal", "vanished source layer code");
        assert!(
            error.to_string().contains("not loaded"),
            "the message names the problem: {error}"
        );
    }

    #[test]
    fn entry_offset_finds_tables_and_reports_gaps() {
        let lump = marker("X");
        let entry = tagged(lump, Source::Working, 0);
        let no_table = vec![(Source::Layer(0), vec![12_u32])];
        let unknown = entry_offset(&no_table, &entry).expect_err("no working table");
        assert_eq!(unknown.code(), "internal", "missing source table");
        let short = vec![(Source::Working, Vec::new())];
        let past_end = entry_offset(&short, &entry).expect_err("empty table");
        assert_eq!(past_end.code(), "internal", "position past the table");
        let ok = vec![(Source::Working, vec![12_u32])];
        assert_eq!(
            entry_offset(&ok, &entry).expect("a known position"),
            12,
            "the table position wins"
        );
    }

    #[test]
    fn a_source_index_past_the_archive_is_internal() {
        let lump = marker("GHOST");
        let entry = tagged(lump.clone(), Source::Working, 99);
        let info = info_at(0, &lump);
        let tables = vec![(Source::Working, vec![12_u32])];
        let error = super::row(&entry, &info, &tables).expect_err("past the archive");
        assert_eq!(error.code(), "internal", "row keeps the offset error");
        assert!(
            error.to_string().contains("GHOST"),
            "the message names the lump: {error}"
        );
    }
}
