//! Pure comparison of two lump directories: what changed between them.
//!
//! [`diff`] takes the two directories as plain [`Lump`] slices, classifies
//! both with [`analyze`] and reports every unit of comparison:
//!
//! - map blocks are units keyed by map name: `added`, `removed`, `changed`
//!   (any member lump's bytes differ) or `unchanged`; a changed map carries
//!   per-member lump statuses and, when both blocks decode through
//!   [`read_map`], the element counts of each side;
//! - every other lump is keyed by `(namespace, name, occurrence within that
//!   key)` so a flat and a patch of one name are different units; markers
//!   take part in no comparison;
//! - [`Mode::Override`] is the `base`-to-`working` vocabulary: lumps only in
//!   the second side are `new`, lumps in both are `override` or
//!   `identical_override`, and first-side-only units are omitted because a
//!   working archive never removes what the layers provide.
//!
//! Content comparison runs length first, then a 64-bit `FNV`-1a hash
//! ([`fnv1a64`]), then the bytes themselves. Nothing here knows about
//! sessions or MCP; it is pure data in, pure data out.

use std::collections::HashMap;

use serde::Serialize;

use crate::wad::archive::Lump;
use crate::wad::classify::{Analysis, LumpKind, MapBlock, Namespace, analyze};
use crate::wad::map::{Map, read_map};
use crate::wad::name::LumpName;

/// `FNV`-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// `FNV`-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The comparison vocabulary [`diff`] reports statuses in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Neutral sides: `added`, `removed`, `changed`, `unchanged`.
    General,
    /// `base` against `working`: `new`, `override`, `identical_override`;
    /// units only in the first side are omitted.
    Override,
}

/// How one unit of a diff ended up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Present only in the second side ([`Mode::General`]).
    Added,
    /// Present only in the first side ([`Mode::General`]).
    Removed,
    /// Present in both sides with different bytes ([`Mode::General`]).
    Changed,
    /// Present in both sides with identical bytes.
    Unchanged,
    /// Present only in the second side ([`Mode::Override`]).
    New,
    /// Present in both sides with different bytes ([`Mode::Override`]).
    Override,
    /// Present in both sides with identical bytes ([`Mode::Override`]): an
    /// override that changes nothing, usually a mistake.
    IdenticalOverride,
}

/// The status of a unit both sides hold, from whether its bytes match.
const fn status_both(identical: bool, mode: Mode) -> Status {
    match mode {
        Mode::General => {
            if identical {
                Status::Unchanged
            } else {
                Status::Changed
            }
        }
        Mode::Override => {
            if identical {
                Status::IdenticalOverride
            } else {
                Status::Override
            }
        }
    }
}

/// The status of a unit only the first side holds; `None` when `mode`
/// omits those.
const fn status_only_a(mode: Mode) -> Option<Status> {
    match mode {
        Mode::General => Some(Status::Removed),
        Mode::Override => None,
    }
}

/// The status of a unit only the second side holds.
const fn status_only_b(mode: Mode) -> Status {
    match mode {
        Mode::General => Status::Added,
        Mode::Override => Status::New,
    }
}

/// The 64-bit `FNV`-1a hash of `data`.
#[must_use]
#[inline]
pub fn fnv1a64(data: &[u8]) -> u64 {
    data.iter().fold(FNV_OFFSET_BASIS, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Whether two payloads are equal: length first, then the 64-bit `FNV`-1a
/// hash, then the bytes themselves when the hashes match.
#[must_use]
#[inline]
pub fn same_content(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && fnv1a64(a) == fnv1a64(b) && a == b
}

/// One compared lump: its identity, verdict and the sizes on both sides.
#[derive(Clone, Debug, Serialize)]
pub struct LumpDiff {
    /// The lump's directory name.
    pub name: LumpName,
    /// The region it sits in, `null` for a loose lump.
    pub namespace: Option<Namespace>,
    /// The comparison verdict.
    pub status: Status,
    /// Its payload size in the first side, `null` when absent there.
    pub size_a: Option<usize>,
    /// Its payload size in the second side, `null` when absent there.
    pub size_b: Option<usize>,
}

/// The editable element counts of a decoded map block.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct MapCounts {
    /// Things in [`Map::things`].
    pub things: usize,
    /// Linedefs in [`Map::linedefs`].
    pub linedefs: usize,
    /// Sidedefs in [`Map::sidedefs`].
    pub sidedefs: usize,
    /// Vertices in [`Map::vertices`].
    pub vertices: usize,
    /// Sectors in [`Map::sectors`].
    pub sectors: usize,
}

impl MapCounts {
    /// The five element counts of `map`.
    fn of(map: &Map) -> Self {
        Self {
            things: map.things.len(),
            linedefs: map.linedefs.len(),
            sidedefs: map.sidedefs.len(),
            vertices: map.vertices.len(),
            sectors: map.sectors.len(),
        }
    }
}

/// One compared map block: its verdict, the members that differ and the
/// element counts of the sides that decode.
#[derive(Clone, Debug, Serialize)]
pub struct MapDiff {
    /// The map's name, from its header lump.
    pub name: LumpName,
    /// The block-level verdict.
    pub status: Status,
    /// The member lumps whose bytes differ, empty unless the block changed.
    pub lumps: Vec<LumpDiff>,
    /// Element counts decoded from the first side, `null` when the block is
    /// absent or does not parse.
    pub counts_a: Option<MapCounts>,
    /// Element counts decoded from the second side, `null` when the block
    /// is absent or does not parse.
    pub counts_b: Option<MapCounts>,
}

/// The tally of every compared unit, lump and map alike.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Units only in the second side ([`Mode::General`]).
    pub added: usize,
    /// Units only in the first side ([`Mode::General`]).
    pub removed: usize,
    /// Units in both sides whose bytes differ ([`Mode::General`]).
    pub changed: usize,
    /// Units in both sides with identical bytes.
    pub unchanged: usize,
    /// Units the working side overrides with different bytes
    /// ([`Mode::Override`]); serialized as `override`.
    #[serde(rename = "override")]
    pub overrides: usize,
    /// Units the working side overrides with identical bytes
    /// ([`Mode::Override`]).
    pub identical_override: usize,
    /// Units the working side brings ([`Mode::Override`]).
    pub new: usize,
}

impl Summary {
    /// Counts one compared unit under its status.
    const fn bump(&mut self, status: Status) {
        match status {
            Status::Added => self.added = self.added.saturating_add(1),
            Status::Removed => self.removed = self.removed.saturating_add(1),
            Status::Changed => self.changed = self.changed.saturating_add(1),
            Status::Unchanged => self.unchanged = self.unchanged.saturating_add(1),
            Status::Override => self.overrides = self.overrides.saturating_add(1),
            Status::IdenticalOverride => {
                self.identical_override = self.identical_override.saturating_add(1);
            }
            Status::New => self.new = self.new.saturating_add(1),
        }
    }
}

/// The whole comparison of two directories.
#[derive(Clone, Debug)]
pub struct Diff {
    /// Every compared non-map lump: first side order, then second-side
    /// arrivals.
    pub lumps: Vec<LumpDiff>,
    /// Every compared map block in the same order.
    pub maps: Vec<MapDiff>,
}

impl Diff {
    /// Tally every compared unit under its status.
    #[must_use]
    #[inline]
    pub fn summary(&self) -> Summary {
        let mut summary = Summary::default();
        for status in self
            .lumps
            .iter()
            .map(|row| row.status)
            .chain(self.maps.iter().map(|unit| unit.status))
        {
            summary.bump(status);
        }
        summary
    }
}

/// A comparable lump paired with its match key.
struct Entry<'lumps> {
    /// The region the lump sits in, `None` for a loose lump.
    namespace: Option<Namespace>,
    /// The directory name.
    name: LumpName,
    /// Zero-based index among this side's entries with the same region and
    /// name.
    occurrence: usize,
    /// The lump itself.
    lump: &'lumps Lump,
}

/// The match key of a non-map lump.
type Key = (Option<Namespace>, LumpName, usize);

/// A map block of one side with its compared members.
struct MapUnit<'lumps> {
    /// The block's name.
    name: LumpName,
    /// Zero-based index among this side's blocks with the same name.
    occurrence: usize,
    /// The classified block, for decoding.
    block: MapBlock,
    /// The whole directory the block was found in, for decoding.
    lumps: &'lumps [Lump],
    /// The block's member lumps (header and `GL` runs included).
    members: Vec<Entry<'lumps>>,
}

impl MapUnit<'_> {
    /// The block's match key.
    const fn key(&self) -> (LumpName, usize) {
        (self.name, self.occurrence)
    }
}

/// The region-and-name key occurrence counting runs on.
type RegionKey = (Option<Namespace>, LumpName);

/// Record `key` once more in `counts` and report its zero-based occurrence.
fn occurrence(counts: &mut HashMap<RegionKey, usize>, key: RegionKey) -> usize {
    let slot = counts.entry(key).or_insert(0_usize);
    let at = *slot;
    *slot = at.saturating_add(1);
    at
}

/// The comparable entries of one directory: neither markers nor map data.
fn entries<'lumps>(lumps: &'lumps [Lump], analysis: &Analysis) -> Vec<Entry<'lumps>> {
    let mut counts = HashMap::new();
    let mut out = Vec::new();
    for (info, lump) in analysis.lumps.iter().zip(lumps) {
        if info.kind == LumpKind::Marker || info.map.is_some() {
            continue;
        }
        let occurrence = occurrence(&mut counts, (info.namespace, info.name));
        out.push(Entry {
            namespace: info.namespace,
            name: info.name,
            occurrence,
            lump,
        });
    }
    out
}

/// The ranges a block's members cover: after the header to the end, plus
/// any `GL` run.
fn member_ranges(block: &MapBlock) -> Vec<(usize, usize)> {
    let mut ranges = vec![(block.header.saturating_add(1), block.end)];
    if let Some(gl) = block.gl {
        ranges.push(gl);
    }
    ranges
}

/// The member lumps of `block` in the directory `lumps`, keyed by name.
fn members<'lumps>(block: &MapBlock, lumps: &'lumps [Lump]) -> Vec<Entry<'lumps>> {
    let mut counts = HashMap::new();
    let mut out = Vec::new();
    for (start, end) in member_ranges(block) {
        for lump in lumps.get(start..end).unwrap_or(&[]) {
            let occurrence = occurrence(&mut counts, (None, lump.name));
            out.push(Entry {
                namespace: None,
                name: lump.name,
                occurrence,
                lump,
            });
        }
    }
    out
}

/// The map units of one directory, in block order.
fn map_units<'lumps>(lumps: &'lumps [Lump], analysis: &Analysis) -> Vec<MapUnit<'lumps>> {
    let mut counts = HashMap::new();
    let mut out = Vec::new();
    for block in &analysis.maps {
        let occurrence = occurrence(&mut counts, (None, block.name));
        out.push(MapUnit {
            name: block.name,
            occurrence,
            block: block.clone(),
            lumps,
            members: members(block, lumps),
        });
    }
    out
}

/// Key every entry of `side` to its position.
fn index_entries(entries: &[Entry<'_>]) -> HashMap<Key, usize> {
    entries
        .iter()
        .enumerate()
        .map(|(at, entry)| ((entry.namespace, entry.name, entry.occurrence), at))
        .collect()
}

/// The row for a unit both sides hold.
fn both_row(a: &Entry<'_>, b: &Entry<'_>, mode: Mode) -> LumpDiff {
    let identical = same_content(&a.lump.data, &b.lump.data);
    LumpDiff {
        name: a.name,
        namespace: a.namespace,
        status: status_both(identical, mode),
        size_a: Some(a.lump.size()),
        size_b: Some(b.lump.size()),
    }
}

/// The row for a unit only one side holds.
fn single_row(entry: &Entry<'_>, status: Status, in_a: bool) -> LumpDiff {
    let size = entry.lump.size();
    LumpDiff {
        name: entry.name,
        namespace: entry.namespace,
        status,
        size_a: in_a.then_some(size),
        size_b: if in_a { None } else { Some(size) },
    }
}

/// The rows for `a`'s entries: paired with `b` or reported as one-sided.
fn rows_from_a(
    a: &[Entry<'_>],
    b: &[Entry<'_>],
    b_index: &HashMap<Key, usize>,
    mode: Mode,
) -> Vec<LumpDiff> {
    let mut rows = Vec::with_capacity(a.len());
    for entry in a {
        let key = (entry.namespace, entry.name, entry.occurrence);
        match b_index.get(&key).and_then(|at| b.get(*at)) {
            Some(other) => rows.push(both_row(entry, other, mode)),
            None => push_only_a(&mut rows, entry, mode),
        }
    }
    rows
}

/// Append `entry` to `rows` when the mode reports first-side-only units.
fn push_only_a(rows: &mut Vec<LumpDiff>, entry: &Entry<'_>, mode: Mode) {
    if let Some(status) = status_only_a(mode) {
        rows.push(single_row(entry, status, true));
    }
}

/// The rows for `b`'s entries that `a` has no twin for.
fn rows_only_in_b(a_index: &HashMap<Key, usize>, b: &[Entry<'_>], mode: Mode) -> Vec<LumpDiff> {
    let status = status_only_b(mode);
    b.iter()
        .filter(|entry| !a_index.contains_key(&(entry.namespace, entry.name, entry.occurrence)))
        .map(|entry| single_row(entry, status, false))
        .collect()
}

/// Compare two entry lists under `mode`: `a` order first, then arrivals.
fn compare_entries(a: &[Entry<'_>], b: &[Entry<'_>], mode: Mode) -> Vec<LumpDiff> {
    let a_index = index_entries(a);
    let b_index = index_entries(b);
    let mut rows = rows_from_a(a, b, &b_index, mode);
    rows.extend(rows_only_in_b(&a_index, b, mode));
    rows
}

/// Key every map unit of `side` to its position.
fn index_maps(units: &[MapUnit<'_>]) -> HashMap<(LumpName, usize), usize> {
    units
        .iter()
        .enumerate()
        .map(|(at, unit)| (unit.key(), at))
        .collect()
}

/// The element counts of one side's block, `None` when it does not decode.
fn map_counts(unit: &MapUnit<'_>) -> Option<MapCounts> {
    read_map(unit.lumps, &unit.block)
        .ok()
        .map(|map| MapCounts::of(&map))
}

/// The row for a map block both sides hold.
fn map_row(a: &MapUnit<'_>, b: &MapUnit<'_>, mode: Mode) -> MapDiff {
    let members = compare_entries(&a.members, &b.members, Mode::General);
    if members.iter().all(|row| row.status == Status::Unchanged) {
        return MapDiff {
            name: a.name,
            status: status_both(true, mode),
            lumps: Vec::new(),
            counts_a: None,
            counts_b: None,
        };
    }
    MapDiff {
        name: a.name,
        status: status_both(false, mode),
        lumps: members
            .into_iter()
            .filter(|row| row.status != Status::Unchanged)
            .collect(),
        counts_a: map_counts(a),
        counts_b: map_counts(b),
    }
}

/// The row for a map block only one side holds.
fn single_map(unit: &MapUnit<'_>, status: Status, in_a: bool) -> MapDiff {
    let counts = map_counts(unit);
    let (counts_a, counts_b) = if in_a { (counts, None) } else { (None, counts) };
    MapDiff {
        name: unit.name,
        status,
        lumps: Vec::new(),
        counts_a,
        counts_b,
    }
}

/// Compare two directories' map blocks under `mode`.
fn compare_maps(a: &[MapUnit<'_>], b: &[MapUnit<'_>], mode: Mode) -> Vec<MapDiff> {
    let a_index = index_maps(a);
    let b_index = index_maps(b);
    let mut rows = Vec::with_capacity(a.len());
    for unit in a {
        match b_index.get(&unit.key()).and_then(|at| b.get(*at)) {
            Some(other) => rows.push(map_row(unit, other, mode)),
            None => push_only_a_map(&mut rows, unit, mode),
        }
    }
    rows.extend(
        b.iter()
            .filter(|unit| !a_index.contains_key(&unit.key()))
            .map(|unit| single_map(unit, status_only_b(mode), false)),
    );
    rows
}

/// Append `unit` to `rows` when the mode reports first-side-only blocks.
fn push_only_a_map(rows: &mut Vec<MapDiff>, unit: &MapUnit<'_>, mode: Mode) {
    if let Some(status) = status_only_a(mode) {
        rows.push(single_map(unit, status, true));
    }
}

/// Compare two lump directories as the units of `mode` describe them.
#[must_use]
#[inline]
pub fn diff(a: &[Lump], b: &[Lump], mode: Mode) -> Diff {
    let analysis_a = analyze(a);
    let analysis_b = analyze(b);
    Diff {
        lumps: compare_entries(&entries(a, &analysis_a), &entries(b, &analysis_b), mode),
        maps: compare_maps(&map_units(a, &analysis_a), &map_units(b, &analysis_b), mode),
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, Status, diff, fnv1a64, same_content};
    use crate::wad::archive::Lump;
    use crate::wad::classify::MapFormat;
    use crate::wad::map::{Map, Sector, Sidedef, Thing, Vertex, write_map};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` carrying `payload`.
    fn lump(text: &str, payload: &[u8]) -> Lump {
        Lump::new(name(text), payload.to_vec())
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// The `(name, status)` pairs of a lump list, for one-glance assertions.
    fn pairs(rows: &[super::LumpDiff]) -> Vec<(String, Status)> {
        rows.iter()
            .map(|row| (row.name.to_string(), row.status))
            .collect()
    }

    /// The `things`/`linedefs`/`sidedefs`/`sectors` fixture map named
    /// `text`.
    fn doom_map(text: &str, things: usize) -> Vec<Lump> {
        let mut map = Map::new_empty(name(text), MapFormat::Doom);
        map.vertices.push(Vertex {
            x: 0,
            y: 0,
            ..Default::default()
        });
        map.linedefs.push(crate::wad::map::Linedef::default());
        map.sidedefs.push(Sidedef::default());
        map.sectors.push(Sector::default());
        for _thing in 0..things {
            map.things.push(Thing::default());
        }
        write_map(&map).expect("the fixture map serializes")
    }

    /// The directory of one flat region plus one global.
    fn region(flat: &[u8], global: &[u8]) -> Vec<Lump> {
        vec![
            marker("F_START"),
            lump("FLOOR_1", flat),
            marker("F_END"),
            lump("PLAYPAL", global),
        ]
    }

    #[test]
    fn fnv1a64_matches_the_known_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325_u64, "empty input");
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c_u64, "one byte");
    }

    #[test]
    fn same_content_agrees_with_byte_equality() {
        assert!(same_content(b"abc", b"abc"), "equal payloads");
        assert!(
            !same_content(b"abc", b"abd"),
            "same length, different bytes"
        );
        assert!(!same_content(b"ab", b"abc"), "different lengths");
        assert!(same_content(b"", b""), "two empties match");
    }

    #[test]
    fn general_mode_reports_the_four_neutral_statuses() {
        let a = vec![
            lump("SAME", b"x"),
            lump("GONE", b"g"),
            lump("PLAYPAL", b"old"),
        ];
        let b = vec![
            lump("SAME", b"x"),
            lump("PLAYPAL", b"new"),
            lump("DEHACKED", b"d"),
        ];
        let report = diff(&a, &b, Mode::General);
        assert_eq!(
            pairs(&report.lumps),
            vec![
                ("SAME".to_owned(), Status::Unchanged),
                ("GONE".to_owned(), Status::Removed),
                ("PLAYPAL".to_owned(), Status::Changed),
                ("DEHACKED".to_owned(), Status::Added),
            ],
            "a order first, then b arrivals"
        );
        let gone = report
            .lumps
            .iter()
            .find(|row| row.name == name("GONE"))
            .expect("GONE");
        assert_eq!(gone.size_a, Some(1), "the removed side has bytes");
        assert_eq!(gone.size_b, None, "the other side has nothing");
        let summary = report.summary();
        assert_eq!(
            (
                summary.unchanged,
                summary.removed,
                summary.changed,
                summary.added
            ),
            (1, 1, 1, 1),
            "one unit under each status"
        );
    }

    #[test]
    fn markers_and_their_regions_do_not_take_part() {
        let a = region(b"flat", b"global");
        let b = vec![
            marker("S_START"),
            lump("PLAYPAL", b"global"),
            marker("S_END"),
        ];
        let report = diff(&a, &b, Mode::General);
        assert_eq!(
            pairs(&report.lumps),
            vec![
                ("FLOOR_1".to_owned(), Status::Removed),
                ("PLAYPAL".to_owned(), Status::Removed),
                ("PLAYPAL".to_owned(), Status::Added),
            ],
            "markers are ignored and a sprite-area PLAYPAL never pairs with the global one"
        );
        assert_eq!(
            report.lumps.first().expect("the flat row").namespace,
            Some(crate::wad::classify::Namespace::Flats),
            "the flat reports its region"
        );
        assert_eq!(
            report.lumps.last().expect("the sprite row").namespace,
            Some(crate::wad::classify::Namespace::Sprites),
            "and so does the sprite-area global twin"
        );
    }

    #[test]
    fn duplicate_names_match_by_occurrence() {
        let a = vec![lump("NOTE", b"one"), lump("NOTE", b"two")];
        let b = vec![lump("NOTE", b"ONE"), lump("NOTE", b"two")];
        let report = diff(&a, &b, Mode::General);
        assert_eq!(
            pairs(&report.lumps),
            vec![
                ("NOTE".to_owned(), Status::Changed),
                ("NOTE".to_owned(), Status::Unchanged),
            ],
            "the first copy differs, the second matches"
        );
    }

    #[test]
    fn one_name_in_two_regions_is_two_units() {
        let a = vec![marker("F_START"), lump("ART", b"f"), marker("F_END")];
        let b = vec![marker("P_START"), lump("ART", b"f"), marker("P_END")];
        let report = diff(&a, &b, Mode::General);
        assert_eq!(
            pairs(&report.lumps),
            vec![
                ("ART".to_owned(), Status::Removed),
                ("ART".to_owned(), Status::Added),
            ],
            "a flat and a patch of one name never pair up"
        );
        assert_eq!(report.summary().unchanged, 0, "identical bytes do not help");
    }

    #[test]
    fn override_mode_names_new_overrides_and_omits_the_rest() {
        let a = vec![lump("PLAYPAL", b"base"), lump("COLORMAP", b"copy")];
        let b = vec![
            lump("PLAYPAL", b"patched"),
            lump("COLORMAP", b"copy"),
            lump("DEHACKED", b"new"),
        ];
        let report = diff(&a, &b, Mode::Override);
        assert_eq!(
            pairs(&report.lumps),
            vec![
                ("PLAYPAL".to_owned(), Status::Override),
                ("COLORMAP".to_owned(), Status::IdenticalOverride),
                ("DEHACKED".to_owned(), Status::New),
            ],
            "every b unit keeps override vocabulary"
        );
        let summary = report.summary();
        assert_eq!(
            (summary.overrides, summary.identical_override, summary.new),
            (1, 1, 1),
            "the summary tallies the override vocabulary"
        );
        assert_eq!(
            (summary.added, summary.removed, summary.changed),
            (0, 0, 0),
            "the neutral buckets stay empty"
        );
    }

    #[test]
    fn override_mode_drops_units_only_the_first_side_holds() {
        let a = vec![lump("PLAYPAL", b"base")];
        let b = vec![lump("DEHACKED", b"new")];
        let report = diff(&a, &b, Mode::Override);
        assert_eq!(
            pairs(&report.lumps),
            vec![("DEHACKED".to_owned(), Status::New)],
            "nothing was removed from a's point of view"
        );
        assert_eq!(report.summary().removed, 0, "removed cannot happen");
    }

    #[test]
    fn changed_maps_carry_member_statuses_and_count_deltas() {
        let mut a = vec![lump("PLAYPAL", b"pp")];
        a.extend(doom_map("MAP01", 1));
        let mut b = vec![lump("PLAYPAL", b"pp")];
        b.extend(doom_map("MAP01", 3));
        let report = diff(&a, &b, Mode::General);
        assert!(
            report
                .lumps
                .iter()
                .all(|row| row.status == Status::Unchanged),
            "no lump changed"
        );
        let map = report.maps.first().expect("MAP01 compared");
        assert_eq!(map.name, name("MAP01"), "the map's name");
        assert_eq!(map.status, Status::Changed, "one member differs");
        assert_eq!(
            map.lumps
                .iter()
                .map(|row| row.name.to_string())
                .collect::<Vec<_>>(),
            vec!["THINGS".to_owned()],
            "only the changed member is listed"
        );
        let counts_a = map.counts_a.expect("the base map decodes");
        let counts_b = map.counts_b.expect("the working map decodes");
        assert_eq!(counts_a.things, 1, "the first side's things");
        assert_eq!(counts_b.things, 3, "the second side's things");
        assert_eq!(counts_a.linedefs, 1, "the shared linedef count");
        assert_eq!(counts_b.sectors, 1, "the shared sector count");
        assert_eq!(report.summary().changed, 1, "the map counts as one unit");
    }

    #[test]
    fn maps_that_do_not_parse_report_no_counts() {
        let mut a = doom_map("MAP01", 1);
        let corrupt = Lump::new(name("THINGS"), vec![0_u8; 7]);
        if let Some(slot) = a.iter_mut().find(|lump| lump.name == name("THINGS")) {
            *slot = corrupt;
        }
        let report = diff(&a, &doom_map("MAP01", 2), Mode::General);
        let map = report.maps.first().expect("MAP01 compared");
        assert_eq!(map.status, Status::Changed, "the member bytes differ");
        assert!(
            map.counts_a.is_none(),
            "the corrupt side decodes to nothing"
        );
        assert!(map.counts_b.is_some(), "the clean side still decodes");
    }

    #[test]
    fn identical_map_blocks_are_unchanged_or_identical_overrides() {
        let a = doom_map("MAP01", 2);
        let b = doom_map("MAP01", 2);
        assert_eq!(
            diff(&a, &b, Mode::General)
                .maps
                .first()
                .expect("one map")
                .status,
            Status::Unchanged,
            "byte-identical blocks are unchanged"
        );
        assert_eq!(
            diff(&a, &b, Mode::Override)
                .maps
                .first()
                .expect("one map")
                .status,
            Status::IdenticalOverride,
            "the working copy of a map without a change is a mistake"
        );
    }

    #[test]
    fn added_and_removed_maps_are_units_of_their_side() {
        let a = doom_map("MAP01", 1);
        let b = doom_map("MAP02", 2);
        let general = diff(&a, &b, Mode::General);
        assert_eq!(
            general
                .maps
                .iter()
                .map(|unit| (unit.name.to_string(), unit.status))
                .collect::<Vec<_>>(),
            vec![
                ("MAP01".to_owned(), Status::Removed),
                ("MAP02".to_owned(), Status::Added),
            ],
            "each single-side block keeps its side's counts"
        );
        assert!(
            general.maps.first().expect("MAP01").counts_a.is_some(),
            "a's block decodes"
        );
        assert!(
            general.maps.first().expect("MAP01").counts_b.is_none(),
            "b has no block"
        );
        let override_mode = diff(&a, &b, Mode::Override);
        assert_eq!(
            pairs_of_maps(&override_mode),
            vec![("MAP02".to_owned(), Status::New)],
            "override mode reports only what the working side brings"
        );
    }

    /// The `(name, status)` pairs of a report's map list.
    fn pairs_of_maps(report: &super::Diff) -> Vec<(String, Status)> {
        report
            .maps
            .iter()
            .map(|unit| (unit.name.to_string(), unit.status))
            .collect()
    }

    #[test]
    fn gl_blocks_move_with_their_map() {
        let mut a = doom_map("MAP01", 1);
        a.push(marker("GL_MAP01"));
        a.push(lump("GL_VERT", b"old"));
        let mut b = doom_map("MAP01", 1);
        b.push(marker("GL_MAP01"));
        b.push(lump("GL_VERT", b"new"));
        let report = diff(&a, &b, Mode::General);
        let map = report.maps.first().expect("MAP01 compared");
        assert_eq!(map.status, Status::Changed, "the GL member differs");
        assert_eq!(
            map.lumps.first().expect("GL_VERT row").name,
            name("GL_VERT"),
            "the changed member is the GL lump"
        );
        assert!(map.counts_a.is_some(), "the binary members still decode");
    }

    #[test]
    fn two_blocks_of_one_name_pair_by_occurrence() {
        let a = doom_map("MAP01", 1);
        let mut b = doom_map("MAP01", 1);
        b.extend(doom_map("MAP01", 2));
        let report = diff(&a, &b, Mode::General);
        assert_eq!(report.maps.len(), 2, "both MAP01 blocks are compared");
        assert_eq!(
            report.maps.first().expect("first block").status,
            Status::Unchanged,
            "the first copies match"
        );
        assert_eq!(
            report.maps.get(1).expect("second block").status,
            Status::Added,
            "b's second block has no twin in a"
        );
    }
}
