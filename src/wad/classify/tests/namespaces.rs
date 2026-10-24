//! The namespace topic: the marker helpers and [`Namespace`]'s wire names,
//! the [`LumpKind`] names mirroring their serialization and parsing back,
//! every marker pair delimiting its region, the `SS_START`-style aliases and
//! inner markers, the unclosed start and stray end warnings, a later start
//! closing the earlier one and markers winning over content rules.

use serde_json::json;

use super::{junk, kind_at, lump, marker, namespace_at};
use crate::wad::classify::{LumpKind, Namespace, analyze};

#[test]
fn namespace_helpers_and_serialization() {
    assert_eq!(Namespace::Sprites.as_str(), "sprites", "snake-case name");
    assert_eq!(
        Namespace::Sprites.start_marker(),
        "S_START",
        "canonical start"
    );
    assert_eq!(Namespace::Sprites.end_marker(), "S_END", "canonical end");
    assert_eq!(Namespace::Flats.start_marker(), "F_START", "flats start");
    assert_eq!(Namespace::Patches.end_marker(), "P_END", "patches end");
    assert_eq!(
        Namespace::Colormaps.start_marker(),
        "C_START",
        "colormaps start"
    );
    assert_eq!(Namespace::Acs.end_marker(), "A_END", "acs end");
    assert_eq!(Namespace::Voices.start_marker(), "V_START", "voices start");
    assert_eq!(Namespace::Hires.end_marker(), "HI_END", "hires end");
    assert_eq!(
        Namespace::Textures.start_marker(),
        "TX_START",
        "textures start"
    );
    assert_eq!(
        Namespace::parse("flats"),
        Some(Namespace::Flats),
        "exact parse"
    );
    assert_eq!(
        Namespace::parse("Hires"),
        Some(Namespace::Hires),
        "case-insensitive"
    );
    assert_eq!(Namespace::parse("nope"), None, "unknown name");
    assert_eq!(
        serde_json::to_value(Namespace::Colormaps).expect("serializes"),
        json!("colormaps"),
        "snake-case wire name"
    );
}

#[test]
fn lump_kind_names_mirror_serialization_and_parse() {
    assert_eq!(LumpKind::ALL.len(), 30, "the documented kind count");
    for kind in LumpKind::ALL {
        let serialized = serde_json::to_value(kind).expect("kinds serialize");
        let text = serialized
            .as_str()
            .expect("kinds serialize as snake-case strings");
        assert_eq!(kind.as_str(), text, "as_str mirrors Serialize for {kind:?}");
        assert_eq!(LumpKind::parse(text), Some(kind), "exact parse of {text}");
        assert_eq!(
            LumpKind::parse(&text.to_ascii_uppercase()),
            Some(kind),
            "case-insensitive parse of {text}"
        );
    }
    assert_eq!(LumpKind::parse("nope"), None, "unknown kind name");
}

#[test]
fn every_namespace_pair_delimits_its_region() {
    for namespace in Namespace::ALL {
        let lumps = vec![
            marker(namespace.start_marker()),
            lump("XMEMBER", &junk()),
            marker(namespace.end_marker()),
            marker("AFTER"),
        ];
        let analysis = analyze(&lumps);
        assert_eq!(
            namespace_at(&analysis, 0),
            Some(namespace),
            "{} start marker knows its namespace",
            namespace.as_str()
        );
        assert_eq!(
            namespace_at(&analysis, 1),
            Some(namespace),
            "member sits inside {}",
            namespace.as_str()
        );
        assert_eq!(
            namespace_at(&analysis, 2),
            Some(namespace),
            "{} end marker knows its namespace",
            namespace.as_str()
        );
        assert_eq!(
            namespace_at(&analysis, 3),
            None,
            "after the pair is outside"
        );
        assert_eq!(kind_at(&analysis, 0), LumpKind::Marker, "start is a marker");
        assert_eq!(kind_at(&analysis, 2), LumpKind::Marker, "end is a marker");
        assert!(analysis.warnings.is_empty(), "a clean pair must not warn");
    }
}

#[test]
fn alias_markers_open_and_close_their_namespace() {
    let lumps = vec![
        marker("SS_START"),
        marker("S_END"),
        marker("FF_START"),
        marker("F_END"),
        marker("PP_START"),
        marker("P_END"),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(
        analysis
            .lumps
            .iter()
            .map(|entry| entry.namespace)
            .collect::<Vec<_>>(),
        vec![
            Some(Namespace::Sprites),
            Some(Namespace::Sprites),
            Some(Namespace::Flats),
            Some(Namespace::Flats),
            Some(Namespace::Patches),
            Some(Namespace::Patches),
        ],
        "aliases carry their namespace and close on the canonical end"
    );
    assert!(analysis.warnings.is_empty(), "mixed pairs are legal");
}

#[test]
fn inner_markers_stay_inside_the_namespace() {
    let lumps = vec![
        marker("F_START"),
        marker("F1_START"),
        lump("FLATA", &junk()),
        marker("F2_END"),
        lump("FLATB", &junk()),
        marker("F_END"),
    ];
    let analysis = analyze(&lumps);
    for (index, expected) in [
        LumpKind::Marker,
        LumpKind::Marker,
        LumpKind::Flat,
        LumpKind::Marker,
        LumpKind::Flat,
        LumpKind::Marker,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(kind_at(&analysis, index), expected, "kind at {index}");
    }
    assert!(
        (0..6).all(|at| namespace_at(&analysis, at) == Some(Namespace::Flats)),
        "inner markers do not close the enclosing namespace"
    );
    assert!(
        analysis.warnings.is_empty(),
        "inner markers are not warnings"
    );
}

#[test]
fn unclosed_namespace_runs_to_the_end_and_warns() {
    let lumps = vec![
        marker("S_START"),
        lump("SPRITE01", &junk()),
        marker("AFTER"),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(
        analysis.warnings.len(),
        1,
        "exactly one warning: {analysis:?}"
    );
    assert!(
        analysis.warnings[0].contains("never closed") && analysis.warnings[0].contains("sprites"),
        "the warning explains the fallback: {}",
        analysis.warnings[0]
    );
    assert_eq!(
        namespace_at(&analysis, 1),
        Some(Namespace::Sprites),
        "runs on"
    );
    assert_eq!(
        namespace_at(&analysis, 2),
        Some(Namespace::Sprites),
        "to the end"
    );
}

#[test]
fn stray_end_marker_warns_without_closing_another_namespace() {
    let lumps = vec![
        marker("F_START"),
        marker("P_END"),
        lump("FLATA", &junk()),
        marker("F_END"),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(
        analysis.warnings.len(),
        1,
        "only the stray warns: {analysis:?}"
    );
    assert!(
        analysis.warnings[0].contains("stray end marker")
            && analysis.warnings[0].contains("patches"),
        "the warning names the stray: {}",
        analysis.warnings[0]
    );
    assert_eq!(
        namespace_at(&analysis, 1),
        Some(Namespace::Patches),
        "the marker still reports the namespace it closes"
    );
    assert_eq!(
        kind_at(&analysis, 1),
        LumpKind::Marker,
        "a stray is a marker"
    );
    assert_eq!(
        namespace_at(&analysis, 2),
        Some(Namespace::Flats),
        "the open namespace survives a stray end"
    );
}

#[test]
fn a_new_start_closes_the_previous_namespace_with_a_warning() {
    let lumps = vec![
        marker("F_START"),
        marker("S_START"),
        lump("SPRITE01", &junk()),
        marker("S_END"),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(
        analysis.warnings.len(),
        1,
        "one unclosed start: {analysis:?}"
    );
    assert!(
        analysis.warnings[0].contains("F_START") && analysis.warnings[0].contains("flats"),
        "the warning names the opener: {}",
        analysis.warnings[0]
    );
    assert_eq!(
        namespace_at(&analysis, 2),
        Some(Namespace::Sprites),
        "the later start wins"
    );
}

#[test]
fn markers_win_over_content_rules() {
    let lumps = vec![
        lump("S_START", b"not really empty"),
        lump("TX_END", b"tail"),
    ];
    let analysis = analyze(&lumps);
    assert_eq!(
        kind_at(&analysis, 0),
        LumpKind::Marker,
        "a filled start is a marker"
    );
    assert_eq!(
        namespace_at(&analysis, 0),
        Some(Namespace::Sprites),
        "and opens sprites"
    );
    assert_eq!(
        kind_at(&analysis, 1),
        LumpKind::Marker,
        "a filled end is a marker"
    );
    assert_eq!(
        namespace_at(&analysis, 1),
        Some(Namespace::Textures),
        "and reports textures"
    );
}
