//! Placement of a new lump in the working archive's directory, shared by
//! the lump-editing tools.
//!
//! [`PlacementParams`] is the wire form (the four optional fields flattened
//! into each tool's arguments); [`parse_placement`] validates it into a
//! [`Placement`]; [`insert`] performs the directory mutation on a
//! `&mut Wad` and reports the final index. [`inspect`] answers what the
//! whole directory says about the lump once it sits there: its region and
//! whether it duplicates a same-name twin in the same region.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;
use crate::wad::archive::{Lump, Wad};
use crate::wad::classify::{Namespace, analyze, namespace_end, namespace_start};
use crate::wad::name::LumpName;

/// The placement fields the lump-editing tools share, flattened into each
/// tool's arguments. At most one of them may be set.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
pub(super) struct PlacementParams {
    /// Insert at this directory position of the working archive
    /// (`0..=current length`).
    #[serde(default)]
    pub(super) index: Option<usize>,
    /// Insert just before the last lump with this name in the working
    /// archive.
    #[serde(default)]
    pub(super) before: Option<String>,
    /// Insert just after the last lump with this name in the working
    /// archive.
    #[serde(default)]
    pub(super) after: Option<String>,
    /// Insert just before the end marker of this namespace region
    /// (`sprites`, `flats`, `patches`, `colormaps`, `acs`, `voices`,
    /// `hires` or `textures`); when the working archive has no such region
    /// its canonical markers are created around the new lump.
    #[serde(default)]
    pub(super) namespace: Option<String>,
}

/// The validated placement of one new lump: at most one field set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Placement {
    /// Insert at this directory position.
    pub(super) index: Option<usize>,
    /// Insert before the last lump with this name.
    pub(super) before: Option<LumpName>,
    /// Insert after the last lump with this name.
    pub(super) after: Option<LumpName>,
    /// Insert at the end of this namespace region.
    pub(super) namespace: Option<Namespace>,
}

/// Validate the placement arguments: at most one field, parsed names.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when more than one field is set, a
/// `before`/`after` name does not parse or `namespace` names no region.
pub(super) fn parse_placement(args: &PlacementParams) -> Result<Placement, CyberError> {
    let placement = Placement {
        index: args.index,
        before: args.before.as_deref().map(LumpName::parse).transpose()?,
        after: args.after.as_deref().map(LumpName::parse).transpose()?,
        namespace: super::parse_namespace(args.namespace.as_deref())?,
    };
    if given(&placement) > 1 {
        return Err(CyberError::InvalidArgument(
            "placement takes at most one of index, before, after or namespace: pick one".to_owned(),
        ));
    }
    Ok(placement)
}

/// How many placement fields are set.
fn given(placement: &Placement) -> usize {
    [
        placement.index.is_some(),
        placement.before.is_some(),
        placement.after.is_some(),
        placement.namespace.is_some(),
    ]
    .into_iter()
    .filter(|set| *set)
    .count()
}

/// Insert `lump` into `wad` where `placement` says and report its final
/// directory index.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an `index` past the directory's end,
/// [`CyberError::NotFound`] when `before`/`after` name a lump `wad` does not
/// hold, [`CyberError::LimitExceeded`] if the address space cannot take one
/// more entry.
pub(super) fn insert(
    wad: &mut Wad,
    placement: &Placement,
    lump: Lump,
) -> Result<usize, CyberError> {
    if let Some(index) = placement.index {
        return insert_at(wad, index, lump);
    }
    if let Some(anchor) = placement.before {
        return insert_at(wad, last_named(wad, anchor)?, lump);
    }
    if let Some(anchor) = placement.after {
        let at = last_named(wad, anchor)?
            .checked_add(1)
            .ok_or_else(address_space_lost)?;
        return insert_at(wad, at, lump);
    }
    if let Some(namespace) = placement.namespace {
        return insert_in_namespace(wad, namespace, lump);
    }
    let at = wad.len();
    wad.lumps.push(lump);
    Ok(at)
}

/// Insert at `at`, refusing a position past the directory's end.
fn insert_at(wad: &mut Wad, at: usize, lump: Lump) -> Result<usize, CyberError> {
    let len = wad.len();
    if at > len {
        return Err(CyberError::InvalidArgument(format!(
            "index {at} is past the end of the {len}-lump working archive: pass an index \
             between 0 and {len}, or omit placement to append"
        )));
    }
    wad.lumps.insert(at, lump);
    Ok(at)
}

/// The index of the last lump named `name`.
///
/// # Errors
///
/// [`CyberError::NotFound`] with a `list_lumps` hint when `wad` holds no
/// such lump.
fn last_named(wad: &Wad, name: LumpName) -> Result<usize, CyberError> {
    wad.find_last(name).ok_or_else(|| {
        CyberError::NotFound(format!(
            "no lump named {name} in the working archive: list_lumps scope=working shows what \
             is there, or omit before/after to append"
        ))
    })
}

/// Put `lump` just before `namespace`'s end marker; an unclosed region is
/// extended and closed, and a missing region is created with its canonical
/// markers around the new lump.
fn insert_in_namespace(
    wad: &mut Wad,
    namespace: Namespace,
    lump: Lump,
) -> Result<usize, CyberError> {
    if let Some(at) = last_end_marker(wad, namespace) {
        return insert_at(wad, at, lump);
    }
    if region_is_open(wad, namespace) {
        let at = wad.len();
        wad.lumps.push(lump);
        wad.lumps.push(marker(namespace.end_marker())?);
        return Ok(at);
    }
    wad.lumps.push(marker(namespace.start_marker())?);
    let at = wad.len();
    wad.lumps.push(lump);
    wad.lumps.push(marker(namespace.end_marker())?);
    Ok(at)
}

/// The empty marker lump spelled `text`.
fn marker(text: &str) -> Result<Lump, CyberError> {
    Ok(Lump::marker(LumpName::parse(text)?))
}

/// The index of the last end marker (any alias) of `namespace`.
fn last_end_marker(wad: &Wad, namespace: Namespace) -> Option<usize> {
    wad.lumps
        .iter()
        .rposition(|lump| namespace_end(lump.name.as_bytes()) == Some(namespace))
}

/// Whether `wad` holds a start marker (any alias) of `namespace` somewhere.
fn region_is_open(wad: &Wad, namespace: Namespace) -> bool {
    wad.lumps
        .iter()
        .any(|lump| namespace_start(lump.name.as_bytes()) == Some(namespace))
}

/// What the whole directory says about the lump at one index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct At {
    /// The region the lump sits in, `None` for a loose lump.
    pub(super) namespace: Option<Namespace>,
    /// Whether another lump shares this lump's name and region.
    pub(super) clashes: bool,
}

/// Classify the lump at `index` of `wad` against the rest of the directory.
/// An index past the end reports no region and no clash.
pub(super) fn inspect(wad: &Wad, index: usize) -> At {
    let analysis = analyze(&wad.lumps);
    let Some(this) = analysis.lumps.get(index) else {
        return At {
            namespace: None,
            clashes: false,
        };
    };
    let clashes = analysis.lumps.iter().enumerate().any(|(other, info)| {
        other != index && info.name == this.name && info.namespace == this.namespace
    });
    At {
        namespace: this.namespace,
        clashes,
    }
}

/// The fault for an exhausted address space.
fn address_space_lost() -> CyberError {
    CyberError::LimitExceeded(
        "the working archive directory cannot grow by one more lump".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use crate::error::CyberError;
    use crate::tools::lumps::place::{
        At, Placement, PlacementParams, insert, inspect, parse_placement,
    };
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::classify::Namespace;
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A one-byte lump named `text` so copies are tellable.
    fn lump(text: &str) -> Lump {
        Lump::new(name(text), vec![1_u8])
    }

    /// An empty marker lump named `text`.
    fn marker(text: &str) -> Lump {
        Lump::marker(name(text))
    }

    /// A `PWAD` holding `lumps`.
    fn wad_with(lumps: Vec<Lump>) -> Wad {
        Wad {
            kind: WadKind::Pwad,
            lumps,
        }
    }

    /// The names of a directory, in order.
    fn names(wad: &Wad) -> Vec<String> {
        wad.lumps.iter().map(|lump| lump.name.to_string()).collect()
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// A [`PlacementParams`] with only `index` set.
    fn at(index: usize) -> PlacementParams {
        PlacementParams {
            index: Some(index),
            ..PlacementParams::default()
        }
    }

    /// A [`PlacementParams`] from optional name texts.
    fn named(before: Option<&str>, after: Option<&str>) -> PlacementParams {
        PlacementParams {
            before: before.map(str::to_owned),
            after: after.map(str::to_owned),
            ..PlacementParams::default()
        }
    }

    #[test]
    fn an_empty_placement_appends_at_the_end() {
        let mut wad = wad_with(vec![lump("ONE")]);
        let index = insert(&mut wad, &Placement::default(), lump("TWO")).expect("append");
        assert_eq!(index, 1, "the new directory position");
        assert_eq!(names(&wad), vec!["ONE", "TWO"], "appended");
    }

    #[test]
    fn index_places_from_zero_to_the_end() {
        let mut wad = wad_with(vec![lump("ONE"), lump("TWO")]);
        let index = insert(
            &mut wad,
            &Placement {
                index: Some(1),
                ..Placement::default()
            },
            lump("NEW"),
        )
        .expect("middle insert");
        assert_eq!(index, 1, "reported position");
        assert_eq!(names(&wad), vec!["ONE", "NEW", "TWO"], "spliced in");
        let tail = insert(
            &mut wad,
            &Placement {
                index: Some(3),
                ..Placement::default()
            },
            lump("LAST"),
        )
        .expect("end insert");
        assert_eq!(tail, 3, "index == length appends");
        assert_eq!(names(&wad), vec!["ONE", "NEW", "TWO", "LAST"], "at the end");
    }

    #[test]
    fn an_index_past_the_end_is_invalid_argument() {
        let mut wad = wad_with(vec![lump("ONE")]);
        let error = insert(
            &mut wad,
            &Placement {
                index: Some(2),
                ..Placement::default()
            },
            lump("NEW"),
        )
        .expect_err("only one lump exists");
        assert_eq!(code(&error), "invalid_argument", "out of range code");
        assert!(
            error.to_string().contains("0 and 1"),
            "the message names the valid range: {error}"
        );
        assert_eq!(names(&wad), vec!["ONE"], "nothing changed");
    }

    #[test]
    fn before_and_after_anchor_on_the_last_same_name_copy() {
        let mut wad = wad_with(vec![lump("EDGE"), lump("MID"), lump("EDGE")]);
        let placement = parse_placement(&named(Some("EDGE"), None)).expect("parses");
        let before = insert(&mut wad, &placement, lump("NEW")).expect("before");
        assert_eq!(before, 2, "just before the last EDGE");
        assert_eq!(
            names(&wad),
            vec!["EDGE", "MID", "NEW", "EDGE"],
            "anchored on the last copy"
        );
        let tail_placement = parse_placement(&named(None, Some("EDGE"))).expect("parses");
        let after = insert(&mut wad, &tail_placement, lump("TAIL")).expect("after");
        assert_eq!(after, 4, "just after the last EDGE");
        assert_eq!(
            names(&wad),
            vec!["EDGE", "MID", "NEW", "EDGE", "TAIL"],
            "anchored on the last copy"
        );
    }

    #[test]
    fn a_missing_before_or_after_anchor_is_not_found() {
        let mut wad = wad_with(vec![lump("ONE")]);
        let placement = parse_placement(&named(Some("NOPE"), None)).expect("parses");
        let error = insert(&mut wad, &placement, lump("NEW")).expect_err("no anchor");
        assert_eq!(code(&error), "not_found", "missing anchor code");
        assert!(
            error.to_string().contains("scope=working"),
            "the message names the browsing tool: {error}"
        );
        let after_placement = parse_placement(&named(None, Some("NOPE"))).expect("parses");
        let after_error = insert(&mut wad, &after_placement, lump("NEW")).expect_err("no anchor");
        assert_eq!(code(&after_error), "not_found", "missing anchor code");
        assert_eq!(names(&wad), vec!["ONE"], "nothing changed");
    }

    #[test]
    fn namespace_inserts_before_the_region_end_marker() {
        let mut wad = wad_with(vec![
            marker("F_START"),
            lump("FLATA"),
            marker("F_END"),
            lump("LOOSE"),
        ]);
        let placement = parse_placement(&PlacementParams {
            namespace: Some("flats".to_owned()),
            ..PlacementParams::default()
        })
        .expect("parses");
        let index = insert(&mut wad, &placement, lump("FLATB")).expect("placed");
        assert_eq!(index, 2, "just before F_END");
        assert_eq!(
            names(&wad),
            vec!["F_START", "FLATA", "FLATB", "F_END", "LOOSE"],
            "joined the region"
        );
        let here = inspect(&wad, index);
        assert_eq!(here.namespace, Some(Namespace::Flats), "classified inside");
        assert!(!here.clashes, "FLATB is unique");
    }

    #[test]
    fn namespace_alias_markers_are_found_too() {
        let mut wad = wad_with(vec![marker("FF_START"), lump("FLATA"), marker("FF_END")]);
        let placement = parse_placement(&PlacementParams {
            namespace: Some("Flats".to_owned()),
            ..PlacementParams::default()
        })
        .expect("parses case-insensitively");
        let index = insert(&mut wad, &placement, lump("FLATB")).expect("placed");
        assert_eq!(index, 2, "just before FF_END");
        assert_eq!(
            names(&wad),
            vec!["FF_START", "FLATA", "FLATB", "FF_END"],
            "the alias region was used"
        );
    }

    #[test]
    fn a_missing_namespace_region_is_created_with_its_canonical_markers() {
        let mut wad = wad_with(vec![lump("PLAYPAL")]);
        let placement = parse_placement(&PlacementParams {
            namespace: Some("sprites".to_owned()),
            ..PlacementParams::default()
        })
        .expect("parses");
        let index = insert(&mut wad, &placement, lump("SPRA1")).expect("placed");
        assert_eq!(index, 2, "the lump sits between the new markers");
        assert_eq!(
            names(&wad),
            vec!["PLAYPAL", "S_START", "SPRA1", "S_END"],
            "canonical markers created at the end"
        );
        assert_eq!(
            inspect(&wad, index).namespace,
            Some(Namespace::Sprites),
            "the new lump is in the new region"
        );
    }

    #[test]
    fn an_unclosed_region_is_extended_and_closed() {
        let mut wad = wad_with(vec![marker("A_START"), lump("SCRIPT")]);
        let placement = parse_placement(&PlacementParams {
            namespace: Some("acs".to_owned()),
            ..PlacementParams::default()
        })
        .expect("parses");
        let index = insert(&mut wad, &placement, lump("MORE")).expect("placed");
        assert_eq!(index, 2, "the lump joined the open run");
        assert_eq!(
            names(&wad),
            vec!["A_START", "SCRIPT", "MORE", "A_END"],
            "no second start marker, the region is closed"
        );
    }

    #[test]
    fn parse_placement_accepts_one_field_and_refuses_more() {
        let empty = parse_placement(&PlacementParams::default()).expect("defaults");
        assert_eq!(empty, Placement::default(), "no field means append");
        let only = parse_placement(&at(3)).expect("one field");
        assert_eq!(only.index, Some(3), "the index carried through");
        let both = parse_placement(&PlacementParams {
            index: Some(1),
            before: Some("X".to_owned()),
            ..PlacementParams::default()
        })
        .expect_err("two fields");
        assert_eq!(code(&both), "invalid_argument", "conflict code");
        assert!(
            both.to_string().contains("at most one"),
            "the message states the rule: {both}"
        );
        let anchor_and_region = parse_placement(&PlacementParams {
            after: Some("X".to_owned()),
            namespace: Some("flats".to_owned()),
            ..PlacementParams::default()
        })
        .expect_err("two fields");
        assert_eq!(
            code(&anchor_and_region),
            "invalid_argument",
            "second conflict code"
        );
    }

    #[test]
    fn parse_placement_refuses_bad_names_and_regions() {
        let long = parse_placement(&named(Some("TOO LONG"), None)).expect_err("bad name");
        assert_eq!(code(&long), "invalid_argument", "bad anchor name code");
        let region = parse_placement(&PlacementParams {
            namespace: Some("nope".to_owned()),
            ..PlacementParams::default()
        })
        .expect_err("not a region");
        assert_eq!(code(&region), "invalid_argument", "bad namespace code");
        assert!(
            region.to_string().contains("sprites"),
            "the hint lists the regions: {region}"
        );
    }

    #[test]
    fn inspect_sees_twins_only_inside_their_own_region() {
        let wad = wad_with(vec![
            lump("TWIN"),
            marker("F_START"),
            lump("TWIN"),
            marker("F_END"),
        ]);
        assert!(!inspect(&wad, 0).clashes, "the loose twin is in no region");
        assert!(
            !inspect(&wad, 2).clashes,
            "the flats twin is in its own region"
        );
        assert_eq!(
            inspect(&wad, 2).namespace,
            Some(Namespace::Flats),
            "the region is reported"
        );
        let twice = wad_with(vec![
            marker("F_START"),
            lump("TWIN"),
            lump("TWIN"),
            marker("F_END"),
        ]);
        assert!(inspect(&twice, 1).clashes, "same name, same region");
        assert!(inspect(&twice, 2).clashes, "either copy reports it");
        let loose = wad_with(vec![lump("TWIN"), lump("TWIN")]);
        assert!(inspect(&loose, 0).clashes, "global counts as a namespace");
        assert!(
            !inspect(&loose, 9).clashes,
            "an index past the end clashes with nothing"
        );
        assert_eq!(
            inspect(&loose, 9),
            At {
                namespace: None,
                clashes: false
            },
            "and reports no region"
        );
    }
}
