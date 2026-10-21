//! The bulk-editing machinery the `replace_*` tools are built on: the
//! `maps` selector, the one-undoable-step-over-many-map-blocks runner
//! and the merged view's replaceable-texture stock.
//!
//! [`MapsSelector`] reads the wire shape every bulk edit documents
//! (`"all"`, a list of map names, or absent for the session's default
//! map) and [`resolve`] turns it into the concrete [`LumpName`] list to
//! act on, exactly as the merged view resolves them. [`apply_to_maps`]
//! then runs one pure per-map pass (a [`crate::wad::map::bulk`]
//! replacement, or anything else shaped `Fn(&mut Map) -> T`) over every
//! named block inside a single [`Session::edit`], the one-step pattern
//! `build_all_nodes` uses in the nodes tool group: the copies into the
//! working archive and all rewrites undo together, and a refusal
//! mid-loop leaves the session exactly as it was. A
//! [`ApplyOptions::dry_run`] instead applies the pass to merged-view
//! clones, reporting what would change while writing nothing and
//! recording no history.
//!
//! [`Stock`] answers "does this texture exist?" for replacement
//! targets. It loads the merged view through the theme tools'
//! inventory, so it holds exactly what the theme roles resolve
//! against: the composite `TEXTURE1`/`TEXTURE2` names, the `TX_` lump
//! names, and the flats namespace plus `F_SKY1`.
//!
//! The module sits under the query tools because CD-108's bulk edits
//! act on exactly the element indices `find_elements` reports; nothing
//! here is a `#[tool]` yet.

use alloc::borrow::Cow;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use core::fmt::{self, Formatter};

use schemars::json_schema;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Deserialize;
use serde::de::{self, Deserializer, Unexpected, Visitor};
use serde_json::Value;

use crate::error::CyberError;
use crate::session::map_edit;
use crate::session::{MergedLump, Scope, Session};
use crate::tools::maps::common;
use crate::tools::themes::inventory;
use crate::wad::archive::Wad;
use crate::wad::map::Map;
use crate::wad::name::LumpName;

/// The marker payload of [`MapsSelector::All`]: deserializes exactly
/// the literal string `"all"` and nothing else.
///
/// Serde's untagged matching cannot bind a unit variant to a string at
/// all, so this one-string type is what makes [`MapsSelector::All`] the
/// `"all"` literal; every other bare string falls through to
/// [`MapsSelector::Other`] for [`resolve`]'s instructive refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AllMaps;

impl JsonSchema for AllMaps {
    /// The wire schema of the `"all"` literal: that one string.
    #[inline]
    fn schema_name() -> Cow<'static, str> {
        "AllMaps".into()
    }

    /// The string-constant schema of the `"all"` literal.
    #[inline]
    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "const": "all"
        })
    }
}

impl<'de> Deserialize<'de> for AllMaps {
    /// Reads through the `AllMapsVisitor`: the literal `"all"` or a
    /// deserialization error.
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(AllMapsVisitor)
    }
}

/// The visitor behind [`AllMaps`]: the string `"all"` or an
/// invalid-value error.
struct AllMapsVisitor;

impl Visitor<'_> for AllMapsVisitor {
    type Value = AllMaps;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("the literal string \"all\"")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if v == "all" {
            return Ok(AllMaps);
        }
        Err(E::invalid_value(Unexpected::Str(v), &self))
    }
}

/// The `maps` parameter of a bulk edit: the literal string `"all"`, a
/// list of map names, or anything else.
///
/// The absent form (`None`, which is a parameter's own default for this
/// type) means the session's default map: the active map, else the
/// merged view's only block. An unrecognized value is recorded, not
/// rejected at the serde boundary, so [`resolve`] can refuse it with
/// the documented instruction.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MapsSelector {
    /// Every distinct map block of the merged view: the literal
    /// string `"all"`, read through [`AllMaps`].
    All(AllMaps),
    /// Exactly these maps, named as `list_maps` shows them.
    Names(Vec<String>),
    /// Anything that is neither `"all"` nor a list of names (a bare
    /// string, a number, an object, a mixed list): kept verbatim so
    /// [`resolve`] can quote it in its refusal.
    Other(Value),
}

/// The map blocks one bulk call acts on.
///
/// The `None` selector takes the session's default map, resolved
/// exactly like every other map tool's `map` parameter.
///
/// [`MapsSelector::All`] answers with every distinct block name of the
/// merged view, in directory order. A name carried by two blocks
/// appears once; later steps resolve it to the last block, exactly as
/// the shared `Directory` resolves a double-named map.
///
/// [`MapsSelector::Names`] answers with the parsed names, deduplicated
/// in the order given. [`MapsSelector::Other`] is refused with a
/// [`CyberError::InvalidArgument`] naming `"all"` and `list_maps`.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a selector that is neither
/// `"all"` nor a list, for an empty list, for a name that does not
/// parse, and for the no-default case of `None` (no active map and no
/// single map in the merged view); [`CyberError::NotFound`] for a named
/// map the merged view does not hold.
#[inline]
pub fn resolve(
    session: &Session,
    maps: Option<&MapsSelector>,
) -> Result<Vec<LumpName>, CyberError> {
    let Some(selector) = maps else {
        return common::resolve_map_name(session, None).map(|name| vec![name]);
    };
    // Matching the owned clone keeps every pattern's shape explicit;
    // `IndexSelector::indices` in the shared params follows the same
    // consume-a-clone style for the same reason.
    match selector.clone() {
        MapsSelector::All(..) => every_map(session),
        MapsSelector::Names(names) => listed_maps(session, &names),
        MapsSelector::Other(value) => Err(stray_selector(&value)),
    }
}

/// The refusal for a `maps` value that is neither `"all"` nor a list.
fn stray_selector(value: &Value) -> CyberError {
    CyberError::InvalidArgument(format!(
        "maps {value} is not a map selection: pass \"all\" for every map of the \
         merged view, a list of map names, or leave the parameter out for the \
         active map (list_maps shows the names)"
    ))
}

/// Every distinct block name of the merged view, in directory order.
fn every_map(session: &Session) -> Result<Vec<LumpName>, CyberError> {
    let dir = common::directory(session, Scope::Merged)?;
    let mut names = Vec::new();
    for block in &dir.analysis.maps {
        push_distinct(&mut names, block.name);
    }
    Ok(names)
}

/// The listed names, parsed, deduplicated and checked against the
/// merged view.
fn listed_maps(session: &Session, texts: &[String]) -> Result<Vec<LumpName>, CyberError> {
    if texts.is_empty() {
        return Err(CyberError::InvalidArgument(
            "the maps list is empty: pass at least one map name or \"all\"".to_owned(),
        ));
    }
    let dir = common::directory(session, Scope::Merged)?;
    let mut names = Vec::new();
    for text in texts {
        let name = LumpName::parse(text)?;
        if dir.find_block(name).is_none() {
            return Err(common::missing_map(name, Scope::Merged));
        }
        push_distinct(&mut names, name);
    }
    Ok(names)
}

/// Appends `name` unless the list already carries it.
fn push_distinct(names: &mut Vec<LumpName>, name: LumpName) {
    if !names.contains(&name) {
        names.push(name);
    }
}

/// How one [`apply_to_maps`] run goes: the label of the single undo
/// step and whether the run touches the session at all.
#[derive(Clone, Copy, Debug)]
pub struct ApplyOptions<'lbl> {
    /// The label recorded as the one undo entry (a [`Self::dry_run`]
    /// records no entry at all).
    pub label: &'lbl str,
    /// Apply the pass to merged-view clones: write nothing, record
    /// nothing, copy nothing.
    pub dry_run: bool,
}

/// What one [`apply_to_maps`] run answered, map by map.
#[derive(Debug)]
pub struct MapReplaceRun<T> {
    /// The pass's result for every named map, in the order [`resolve`]
    /// produced it.
    pub per_map: Vec<(LumpName, T)>,
    /// The maps that had to be copied from the read-only layers into
    /// the working archive before they could be edited, one name per
    /// [`map_edit::MapEditReport::copied_to_working`]; always empty for
    /// a [`ApplyOptions::dry_run`].
    pub copied_maps: Vec<String>,
}

/// Runs `f` over every named map block as one undoable step: the same
/// [`Session::edit`] wrapper the whole-set node builds use.
///
/// The shared single-map edit step runs once per block, so every
/// block's copy-in and rewrite share one history entry, and a refusal
/// anywhere leaves the session exactly as it was: nothing is committed
/// and no entry is recorded. With [`ApplyOptions::dry_run`] nothing is
/// written: `f` runs on the merged-view clone of each map instead, and
/// a map the merged view lacks refuses just like the editing path. An
/// empty `names` list writes nothing and records no entry either.
///
/// # Errors
///
/// The single-map edit step's refusals ([`CyberError::NotFound`] for a
/// map no scope holds, and the map writer's encoder errors) and the
/// merged-view read's, unchanged.
#[inline]
pub fn apply_to_maps<T, F>(
    session: &mut Session,
    names: &[LumpName],
    options: &ApplyOptions<'_>,
    f: F,
) -> Result<MapReplaceRun<T>, CyberError>
where
    F: Fn(&mut Map) -> T + Copy,
{
    if options.dry_run {
        return dry_run(session, names, f);
    }
    if names.is_empty() {
        return Ok(MapReplaceRun {
            per_map: Vec::new(),
            copied_maps: Vec::new(),
        });
    }
    let merged = session.merged();
    let answered = session.edit(options.label, |wad| edit_each(wad, &merged, names, f))?;
    Ok(assembled(answered))
}

/// One named map's answer: the pass's value plus whether the block
/// had to be copied into the working archive.
type Answered<T> = Vec<(LumpName, T, bool)>;

/// The [`Session::edit`] body: one `edit_in_place` per named block,
/// each answer recorded beside whether its block had to be copied in.
fn edit_each<T, F>(
    wad: &mut Wad,
    merged: &[MergedLump],
    names: &[LumpName],
    f: F,
) -> Result<Answered<T>, CyberError>
where
    F: Fn(&mut Map) -> T + Copy,
{
    let mut answered = Vec::new();
    for name in names {
        let (value, report) = map_edit::edit_in_place(wad, merged, *name, |map| Ok(f(map)))?;
        answered.push((*name, value, report.copied_to_working));
    }
    Ok(answered)
}

/// Folds one run's per-map answers into the report.
fn assembled<T>(answered: Answered<T>) -> MapReplaceRun<T> {
    let mut run = MapReplaceRun {
        per_map: Vec::new(),
        copied_maps: Vec::new(),
    };
    for (name, value, copied) in answered {
        if copied {
            run.copied_maps.push(name.to_string());
        }
        run.per_map.push((name, value));
    }
    run
}

/// The dry pass: `f` over the merged-view clone of each named map.
fn dry_run<T, F>(
    session: &Session,
    names: &[LumpName],
    f: F,
) -> Result<MapReplaceRun<T>, CyberError>
where
    F: Fn(&mut Map) -> T,
{
    let mut per_map = Vec::new();
    for name in names {
        let mut map = common::load_map(session, *name, Scope::Merged)?;
        per_map.push((*name, f(&mut map)));
    }
    Ok(MapReplaceRun {
        per_map,
        copied_maps: Vec::new(),
    })
}

/// The merged view's replaceable texture stock: the wall and flat names
/// a replacement target may use.
#[derive(Clone, Debug, Default)]
pub struct Stock {
    /// Existing wall texture names.
    pub walls: BTreeSet<String>,
    /// Existing flat names.
    pub flats: BTreeSet<String>,
}

impl Stock {
    /// The merged view's current stock, loaded through the theme tools'
    /// inventory: composite `TEXTURE1`/`TEXTURE2` names, `TX_` lump
    /// names, the flats namespace and `F_SKY1`.
    ///
    /// # Errors
    ///
    /// The inventory load's refusals: the merged directory read's
    /// [`CyberError::NotFound`] and the texture-definition read's
    /// [`CyberError::Format`] for a malformed texture lump.
    #[inline]
    pub fn load(session: &Session) -> Result<Self, CyberError> {
        let available = inventory(session)?.available;
        Ok(Self {
            walls: available.walls,
            flats: available.flats,
        })
    }

    /// Whether `name` exists as a wall texture.
    #[must_use]
    #[inline]
    pub fn has_wall(&self, name: &str) -> bool {
        self.walls.contains(name)
    }

    /// Whether `name` exists as a flat.
    #[must_use]
    #[inline]
    pub fn has_flat(&self, name: &str) -> bool {
        self.flats.contains(name)
    }

    /// Refuse a wall name the merged view lacks.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] naming `list_textures` as the way to
    /// see what exists.
    #[inline]
    pub fn require_wall(&self, name: &str) -> Result<(), CyberError> {
        if self.has_wall(name) {
            return Ok(());
        }
        Err(CyberError::NotFound(format!(
            "wall texture {name} does not exist in the merged view: list_textures shows \
             the composites, build_texture and the import tools add more"
        )))
    }

    /// Refuse a flat name the merged view lacks.
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] naming `list_flats` and
    /// `import_image_as_flat` as the fixes.
    #[inline]
    pub fn require_flat(&self, name: &str) -> Result<(), CyberError> {
        if self.has_flat(name) {
            return Ok(());
        }
        Err(CyberError::NotFound(format!(
            "flat {name} does not exist in the merged view: list_flats shows the flats, \
             import_image_as_flat adds one"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{from_value, json};

    use super::{AllMaps, ApplyOptions, MapsSelector, Stock, apply_to_maps, resolve};
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::tools::maps::common;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::map::bulk::{replace_flats, replace_textures, replace_thing_types};
    use crate::wad::map::{Map, Sector, Sidedef, Thing, Vertex, write_map};
    use crate::wad::name::LumpName;

    /// A parsed test name.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// An eight-byte dummy lump named `text`.
    fn dummy(text: &str) -> Lump {
        Lump::new(name(text), vec![0xA5_u8; 8])
    }

    /// A tiny `Doom` block named `text`: one sector floored and ceiled
    /// `FLOOR4`, one `WALL64` middle and one type-`3004` thing.
    fn demo_map(text: &str) -> Map {
        Map {
            name: name(text),
            things: vec![Thing {
                kind: 3004_i32,
                x: 64_i32,
                y: -32_i32,
                angle: 90_i32,
                flags: 7_u32,
                ..Default::default()
            }],
            vertices: vec![
                Vertex {
                    x: 0_i32,
                    ..Default::default()
                },
                Vertex {
                    x: 128_i32,
                    ..Default::default()
                },
            ],
            sidedefs: vec![Sidedef {
                middle: "WALL64".to_owned(),
                ..Default::default()
            }],
            sectors: vec![Sector {
                floor_flat: "FLOOR4".to_owned(),
                ceiling_flat: "FLOOR4".to_owned(),
                ..Default::default()
            }],
            ..Map::default()
        }
    }

    /// The encoded blocks named `texts`, concatenated.
    fn blocks(texts: &[&str]) -> Vec<Lump> {
        let mut lumps = Vec::new();
        for text in texts {
            let written = write_map(&demo_map(text)).expect("the fixture encodes");
            lumps.extend(written);
        }
        lumps
    }

    /// A session whose only layer is an IWAD of `lumps`.
    fn base_session(lumps: Vec<Lump>) -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        ));
        session
    }

    /// The named map of the merged view, decoded for assertions.
    fn merged_map(session: &Session, text: &str) -> Map {
        common::load_map(session, name(text), Scope::Merged).expect("the fixture map reads")
    }

    /// A `Names` selector for the listed texts.
    fn names_of(texts: &[&str]) -> MapsSelector {
        MapsSelector::Names(texts.iter().map(|text| (*text).to_owned()).collect())
    }

    /// The `"all"` selector: every distinct block of the merged view.
    fn all_maps() -> MapsSelector {
        MapsSelector::All(AllMaps)
    }

    #[test]
    fn maps_selector_reads_its_documented_wire_shapes() {
        let all: MapsSelector = from_value(json!("all")).expect("\"all\" is the whole view");
        assert!(
            matches!(all, MapsSelector::All(AllMaps)),
            "the literal string binds All"
        );
        let listed: MapsSelector =
            from_value(json!(["MAP01", "MAP02"])).expect("a list is a name set");
        let MapsSelector::Names(texts) = listed else {
            panic!("a list must read as Names: {listed:?}");
        };
        assert_eq!(texts, vec!["MAP01".to_owned(), "MAP02".to_owned()]);
        let stray: MapsSelector = from_value(json!("MAP01")).expect("a bare string deserializes");
        assert!(matches!(stray, MapsSelector::Other(_)), "not a selection");
        let number: MapsSelector = from_value(json!(7_i64)).expect("a number deserializes");
        assert!(
            matches!(number, MapsSelector::Other(_)),
            "also not a selection"
        );
    }

    #[test]
    fn resolve_none_takes_the_active_or_only_map() {
        let only = base_session(blocks(&["MAP01"]));
        assert_eq!(
            resolve(&only, None).expect("the single map is the default"),
            vec![name("MAP01")],
            "one block needs no selection"
        );
        let mut busy = base_session(blocks(&["MAP01", "MAP02"]));
        let error = resolve(&busy, None).expect_err("two blocks, no default");
        assert_eq!(error.code(), "invalid_argument", "no-default code");
        busy.set_active_map(Some(name("MAP02")));
        assert_eq!(
            resolve(&busy, None).expect("the active map is the default"),
            vec![name("MAP02")],
            "and an active map resolves it"
        );
    }

    #[test]
    fn resolve_all_lists_the_merged_blocks_once_each_in_order() {
        let session = base_session(blocks(&["MAP01", "MAP01", "MAP02"]));
        let all = resolve(&session, Some(&all_maps())).expect("the merged view reads");
        assert_eq!(
            all,
            vec![name("MAP01"), name("MAP02")],
            "a double-named block appears once, in directory order"
        );
        let empty = Session::default();
        assert!(
            resolve(&empty, Some(&all_maps()))
                .expect("no blocks is no names")
                .is_empty(),
            "an empty merged view selects nothing"
        );
    }

    #[test]
    fn resolve_names_folds_case_dedupes_and_checks_the_merged_view() {
        let session = base_session(blocks(&["MAP01", "MAP02"]));
        let listed = resolve(&session, Some(&names_of(&["map02", "MAP01", "MAP02"])))
            .expect("listed names resolve");
        assert_eq!(
            listed,
            vec![name("MAP02"), name("MAP01")],
            "given order, uppercased, deduplicated"
        );
        let CyberError::NotFound(message) =
            resolve(&session, Some(&names_of(&["MAP09"]))).expect_err("no MAP09 here")
        else {
            panic!("a missing map is not_found");
        };
        assert!(message.contains("MAP09"), "the message names it: {message}");
        let long = resolve(&session, Some(&names_of(&["MAP012345"])))
            .expect_err("nine characters is over");
        assert_eq!(long.code(), "invalid_argument", "unparsable name code");
        let empty_list = resolve(&session, Some(&names_of(&[]))).expect_err("no names given");
        assert_eq!(empty_list.code(), "invalid_argument", "empty list code");
        assert!(
            empty_list.to_string().contains("\"all\""),
            "the message names the alternative: {empty_list}"
        );
    }

    #[test]
    fn resolve_refuses_a_stray_maps_value_naming_the_two_shapes() {
        let session = base_session(blocks(&["MAP01"]));
        let stray = from_value::<MapsSelector>(json!("ALL")).expect("a wrong-case string reads");
        let error = resolve(&session, Some(&stray)).expect_err("not a selection");
        assert_eq!(error.code(), "invalid_argument", "stray selector code");
        let message = error.to_string();
        assert!(
            message.contains("\"all\"") && message.contains("list_maps"),
            "the message names \"all\" and list_maps: {message}"
        );
    }

    #[test]
    fn a_dry_run_reports_the_maps_without_touching_the_session() {
        let mut session = base_session(blocks(&["MAP01", "MAP02"]));
        let targets = resolve(&session, Some(&all_maps())).expect("both maps");
        let options = ApplyOptions {
            label: "replace flats",
            dry_run: true,
        };
        let run = apply_to_maps(&mut session, &targets, &options, |map| {
            replace_flats(map, "FLOOR4", "FLOOR9", &[])
        })
        .expect("the dry pass runs");
        assert_eq!(
            run.per_map,
            vec![(name("MAP01"), 2_usize), (name("MAP02"), 2_usize)],
            "two flat fields per map"
        );
        assert!(run.copied_maps.is_empty(), "a dry run copies nothing");
        assert!(session.undo_labels().is_empty(), "and records no history");
        assert!(!session.is_dirty(), "the session stayed clean");
        assert!(
            session.working().is_empty(),
            "the working archive stayed empty"
        );
        let kept = merged_map(&session, "MAP01");
        assert_eq!(
            kept.sectors.first().expect("sector 0").floor_flat.as_str(),
            "FLOOR4",
            "the merged view still reads the layer's block"
        );
    }

    #[test]
    fn one_apply_step_covers_every_named_map_and_undoes_as_one() {
        let mut session = base_session(blocks(&["MAP01", "MAP02"]));
        let targets = resolve(&session, Some(&all_maps())).expect("both maps");
        let options = ApplyOptions {
            label: "bulk flats",
            dry_run: false,
        };
        let run = apply_to_maps(&mut session, &targets, &options, |map| {
            replace_flats(map, "*", "FLOOR9", &[])
        })
        .expect("the step runs");
        assert_eq!(
            run.per_map,
            vec![(name("MAP01"), 2_usize), (name("MAP02"), 2_usize)],
            "both maps answered"
        );
        assert_eq!(
            run.copied_maps,
            vec!["MAP01".to_owned(), "MAP02".to_owned()],
            "both maps came from the layer alone"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["bulk flats"],
            "two maps, one undo entry"
        );
        assert!(session.is_dirty(), "and unsaved work");
        for text in ["MAP01", "MAP02"] {
            let written = merged_map(&session, text);
            assert_eq!(
                written
                    .sectors
                    .first()
                    .expect("sector 0")
                    .floor_flat
                    .as_str(),
                "FLOOR9",
                "{text} was rewritten"
            );
        }
        assert_eq!(session.undo().expect("undo the step"), "bulk flats");
        assert!(
            session.working().is_empty(),
            "both copies rolled back with the edits"
        );
    }

    #[test]
    fn a_reapplied_map_is_already_editable_and_already_replaced() {
        let mut session = base_session(blocks(&["MAP01"]));
        let targets = resolve(&session, Some(&all_maps())).expect("the one map");
        let options = ApplyOptions {
            label: "bulk things",
            dry_run: false,
        };
        let first = apply_to_maps(&mut session, &targets, &options, |map| {
            replace_thing_types(map, 3004, 9)
        })
        .expect("the first run");
        assert_eq!(
            first.per_map,
            vec![(name("MAP01"), 1_usize)],
            "one thing moved"
        );
        assert_eq!(first.copied_maps, vec!["MAP01".to_owned()], "copied in");
        let second = apply_to_maps(&mut session, &targets, &options, |map| {
            replace_thing_types(map, 3004, 9)
        })
        .expect("the second run");
        assert_eq!(
            second.per_map,
            vec![(name("MAP01"), 0_usize)],
            "nothing left to move"
        );
        assert!(second.copied_maps.is_empty(), "already editable");
        assert_eq!(session.undo_labels().len(), 2, "two steps, two labels");
    }

    #[test]
    fn apply_refuses_a_map_no_archive_holds_and_changes_nothing() {
        let mut session = base_session(blocks(&["MAP01"]));
        let write = ApplyOptions {
            label: "bulk walls",
            dry_run: false,
        };
        let dry = ApplyOptions {
            label: "bulk walls",
            dry_run: true,
        };
        let missing = name("MAP09");
        let error = apply_to_maps(&mut session, &[name("MAP01"), missing], &write, |map| {
            replace_textures(map, "WALL*", "BRICK1", &[])
        })
        .expect_err("MAP09 is nowhere");
        assert_eq!(error.code(), "not_found", "missing map code");
        assert!(session.working().is_empty(), "the whole step was discarded");
        assert!(session.undo_labels().is_empty(), "and recorded nothing");
        let dry_error = apply_to_maps(&mut session, &[missing], &dry, |map| {
            replace_textures(map, "WALL*", "BRICK1", &[])
        })
        .expect_err("the dry read refuses too");
        assert_eq!(dry_error.code(), "not_found", "dry missing map code");
    }

    /// A pass that repaints every flat, for the no-op shape checks.
    fn repaint(map: &mut Map) -> usize {
        replace_flats(map, "*", "FLOOR9", &[])
    }

    #[test]
    fn empty_selections_apply_and_record_nothing() {
        let mut session = base_session(blocks(&["MAP01"]));
        let dry = ApplyOptions {
            label: "bulk nothing",
            dry_run: true,
        };
        let write = ApplyOptions {
            label: "bulk nothing",
            dry_run: false,
        };
        let preview = apply_to_maps(&mut session, &[], &dry, repaint).expect("dry no-op");
        let done = apply_to_maps(&mut session, &[], &write, repaint).expect("write no-op");
        assert!(
            preview.per_map.is_empty() && preview.copied_maps.is_empty(),
            "the dry no-op answered nothing"
        );
        assert!(
            done.per_map.is_empty() && done.copied_maps.is_empty(),
            "the write no-op answered nothing"
        );
        assert!(session.working().is_empty(), "nothing written");
        assert!(
            session.undo_labels().is_empty(),
            "no history entry for nothing"
        );
        assert!(!session.is_dirty(), "and the session stayed clean");
    }

    #[test]
    fn stock_answers_from_the_merged_namespaces_and_refuses_the_rest() {
        let mut lumps = blocks(&["MAP01"]);
        lumps.extend(vec![
            Lump::marker(name("F_START")),
            dummy("FLOOR4"),
            Lump::marker(name("F_END")),
            Lump::marker(name("TX_START")),
            dummy("TX_WALL1"),
            Lump::marker(name("TX_END")),
        ]);
        let session = base_session(lumps);
        let stock = Stock::load(&session).expect("the merged view reads");
        assert!(stock.has_flat("FLOOR4"), "the flats namespace");
        assert!(stock.has_flat("F_SKY1"), "F_SKY1 is always available");
        assert!(stock.has_wall("WALL1"), "the TX_ lump names a wall");
        assert!(!stock.has_wall("TX_WALL1"), "not under its lump name");
        assert!(!stock.has_flat("NUKAGE1"), "unknown flats stay unknown");
        stock.require_wall("WALL1").expect("WALL1 exists");
        stock.require_flat("FLOOR4").expect("FLOOR4 exists");
        let missing_wall = stock.require_wall("BRICK1").expect_err("no such composite");
        assert_eq!(missing_wall.code(), "not_found", "missing wall code");
        assert!(
            missing_wall.to_string().contains("list_textures"),
            "the fix is named: {missing_wall}"
        );
        let missing_flat = stock.require_flat("GRASS1").expect_err("no such flat");
        assert_eq!(missing_flat.code(), "not_found", "missing flat code");
        let message = missing_flat.to_string();
        assert!(
            message.contains("list_flats") && message.contains("import_image_as_flat"),
            "both fixes are named: {message}"
        );
    }

    #[test]
    fn an_empty_stock_still_offers_the_sky() {
        let session = Session::default();
        let stock = Stock::load(&session).expect("an empty view reads");
        assert!(stock.walls.is_empty(), "no textures exist yet");
        assert!(stock.has_flat("F_SKY1"), "F_SKY1 is always available");
    }
}
