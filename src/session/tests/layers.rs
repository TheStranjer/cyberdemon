//! The read side of a session: the scope names and their wire forms, the
//! layer stack and its roles, what the merged view folds to, what an
//! engine's lookup finds there and what each [`Scope`] answers with.

use serde_json::json;

use super::{layer, loaded, lump, name, pwad, tagged};
use crate::session::{LayerRole, Scope, Session, Source};
use crate::wad::archive::WadKind;
use crate::wad::classify::Namespace;

#[test]
fn scope_parses_every_documented_form() {
    assert_eq!(
        Scope::parse("merged").expect("merged"),
        Scope::Merged,
        "the merged view"
    );
    assert_eq!(
        Scope::parse("Working").expect("working"),
        Scope::Working,
        "the working archive, any case"
    );
    assert_eq!(
        Scope::parse("LAYER:2").expect("layer 2"),
        Scope::Layer(2),
        "a layer index, any case"
    );
    assert_eq!(
        Scope::parse("layer:0").expect("layer 0"),
        Scope::Layer(0),
        "the base archive"
    );
}

#[test]
fn scope_rejects_a_non_numeric_layer() {
    let error = Scope::parse("layer:x").expect_err("x is no index");
    assert_eq!(error.code(), "invalid_argument", "bad layer index code");
    assert!(
        error.to_string().contains("\"x\"") || error.to_string().contains("'x'"),
        "the message shows what was not a number: {error}"
    );
    let empty = Scope::parse("layer:").expect_err("a bare prefix is no index");
    assert_eq!(empty.code(), "invalid_argument", "empty layer index code");
    let negative = Scope::parse("layer:-1").expect_err("no negative indices");
    assert_eq!(
        negative.code(),
        "invalid_argument",
        "negative layer index code"
    );
}

#[test]
fn scope_rejects_an_unknown_name_with_the_accepted_forms() {
    let error = Scope::parse("nope").expect_err("nope is no scope");
    assert_eq!(error.code(), "invalid_argument", "unknown scope code");
    let message = error.to_string();
    assert!(
        message.contains("merged") && message.contains("working") && message.contains("layer:"),
        "the error lists the forms to use: {message}"
    );
}

#[test]
fn scope_and_source_use_the_documented_wire_names() {
    assert_eq!(
        serde_json::to_value(Scope::Merged).expect("serializes"),
        json!("merged")
    );
    assert_eq!(
        serde_json::to_value(Scope::Working).expect("serializes"),
        json!("working")
    );
    assert_eq!(
        serde_json::to_value(Scope::Layer(3)).expect("serializes"),
        json!({ "layer": 3_u64 })
    );
    assert_eq!(
        serde_json::from_value::<Scope>(json!({"layer": 1_u64})).expect("deserializes"),
        Scope::Layer(1)
    );
    assert_eq!(
        serde_json::from_value::<Scope>(json!("working")).expect("deserializes"),
        Scope::Working
    );
    assert_eq!(
        serde_json::to_value(Source::Working).expect("serializes"),
        json!("working")
    );
    assert_eq!(
        serde_json::to_value(Source::Layer(2)).expect("serializes"),
        json!({ "layer": 2_u64 })
    );
    assert_eq!(
        serde_json::from_value::<Source>(json!("working")).expect("deserializes"),
        Source::Working
    );
    assert_eq!(
        serde_json::to_value(LayerRole::Iwad).expect("serializes"),
        json!("iwad")
    );
    assert_eq!(
        serde_json::to_value(LayerRole::Resource).expect("serializes"),
        json!("resource")
    );
}

#[test]
fn a_fresh_session_has_no_layers_and_an_empty_working_archive() {
    let session = Session::default();
    assert!(session.iwad().is_none(), "no base archive yet");
    assert!(session.layers().is_empty(), "no layers");
    assert!(session.working().is_empty(), "an empty working PWAD");
    assert_eq!(
        session.working().kind,
        WadKind::Pwad,
        "the default kind is PWAD"
    );
    assert!(session.working_path().is_none(), "never opened from a file");
    assert!(!session.is_dirty(), "nothing has changed");
    assert_eq!(session.active_map(), None, "no map selected");
    assert!(session.merged().is_empty(), "the merged view is empty too");
}

#[test]
fn layers_are_the_iwad_then_the_resources_in_load_order() {
    let session = loaded();
    let paths: Vec<String> = session
        .layers()
        .iter()
        .map(|layer| layer.path.display().to_string())
        .collect();
    assert_eq!(paths, vec!["doom.wad", "extras.wad"], "layer order");
    assert_eq!(
        session
            .layers()
            .iter()
            .map(|item| item.role)
            .collect::<Vec<_>>(),
        vec![LayerRole::Iwad, LayerRole::Resource],
        "roles follow position"
    );
    assert_eq!(
        session.iwad().expect("the base").path.display().to_string(),
        "doom.wad",
        "the iwad accessor names the base"
    );
}

#[test]
fn layer_roles_are_normalised_to_their_position() {
    let mut session = Session::default();
    session.set_iwad(layer(LayerRole::Resource, "doom", vec![lump("PLAYPAL")]));
    session.add_resource(layer(LayerRole::Iwad, "one", vec![]));
    session.add_resource(layer(LayerRole::Resource, "two", vec![]));
    assert_eq!(
        session.iwad().expect("base").role,
        LayerRole::Iwad,
        "set_iwad wins"
    );
    assert_eq!(
        session
            .layers()
            .iter()
            .map(|item| item.role)
            .collect::<Vec<_>>(),
        vec![LayerRole::Iwad, LayerRole::Resource, LayerRole::Resource],
        "a resource never claims to be the base"
    );
    assert_eq!(session.layers().len(), 3, "one iwad plus two resources");
}

#[test]
fn setting_the_iwad_twice_keeps_one_base_layer() {
    let mut session = Session::default();
    session.set_iwad(layer(LayerRole::Iwad, "doom", vec![lump("PLAYPAL")]));
    session.set_iwad(layer(LayerRole::Iwad, "plutonia", vec![lump("PLAYPAL")]));
    assert_eq!(
        session.layers().len(),
        1,
        "the base was replaced, not stacked"
    );
    assert_eq!(
        session.iwad().expect("base").path.display().to_string(),
        "plutonia.wad",
        "the later call won"
    );
}

#[test]
fn merged_view_overrides_by_last_archive_wins() {
    let session = loaded();
    assert_eq!(
        tagged(&session.merged()),
        vec![
            "PLAYPAL@L0",
            "GAMEINFO@L1",
            "F_START@L0",
            "FLOOR_1@L0",
            "FLOOR_2@L0",
            "F_SKY1@L1",
            "F_END@L0",
            "MAP01@L0",
            "THINGS@L0",
        ],
        "the resource's GAMEINFO won in place, its flat joined the region"
    );
}

#[test]
fn find_returns_the_last_match_in_the_merged_view() {
    let session = loaded();
    let gameinfo = session.find(name("GAMEINFO"), None).expect("found");
    assert_eq!(gameinfo.source, Source::Layer(1), "the resource won");
    let floor = session.find(name("FLOOR_1"), None).expect("found");
    assert_eq!(floor.source, Source::Layer(0), "only the base has it");
    assert!(
        session
            .find(name("F_START"), Some(Namespace::Flats))
            .is_some(),
        "markers sit in their region too"
    );
    assert!(
        session
            .find(name("FLOOR_1"), Some(Namespace::Sprites))
            .is_none(),
        "a flat is not a sprite"
    );
    assert!(
        session.find(name("NOPE"), None).is_none(),
        "an unknown name"
    );
    assert!(
        session
            .find(name("THINGS"), Some(Namespace::Flats))
            .is_none(),
        "map data sits in no namespace"
    );
    assert_eq!(
        session
            .find(name("THINGS"), None)
            .expect("map data is findable")
            .source,
        Source::Layer(0),
        "any namespace finds it"
    );
}

#[test]
fn find_global_answers_the_winning_lump_whatever_region_it_sits_in() {
    let session = loaded();
    assert_eq!(
        session
            .find_global(name("GAMEINFO"))
            .expect("a global lump")
            .source,
        Source::Layer(1),
        "the resource's copy wins, as with an engine lookup"
    );
    assert_eq!(
        session
            .find_global(name("FLOOR_1"))
            .expect("a region lump is findable by name alone")
            .source,
        Source::Layer(0),
        "the namespace is not consulted"
    );
    assert!(
        session.find_global(name("NOPE")).is_none(),
        "an unknown name"
    );
}

#[test]
fn find_reports_the_working_copy_once_it_overrides() {
    let mut session = loaded();
    session.replace_working(pwad(vec![lump("GAMEINFO")]), None);
    session
        .edit("bring a flat", |wad| {
            wad.lumps.push(lump("FLOOR_1"));
            Ok(())
        })
        .expect("a plain append");
    let anywhere = session.find(name("FLOOR_1"), None).expect("found");
    assert_eq!(
        anywhere.source,
        Source::Working,
        "the working archive's loose copy is the last one"
    );
    let in_region = session
        .find(name("FLOOR_1"), Some(Namespace::Flats))
        .expect("found in flats");
    assert_eq!(
        in_region.source,
        Source::Layer(0),
        "a namespace lookup still finds the region's copy: the loose lump did not join it"
    );
    assert_eq!(
        session.find(name("GAMEINFO"), None).expect("found").source,
        Source::Working,
        "the working copy overrode the resource's"
    );
}

#[test]
fn lumps_in_returns_each_scope() {
    let session = loaded();
    assert_eq!(
        tagged(
            &session
                .lumps_in(Scope::Working)
                .expect("the working archive")
        ),
        Vec::<String>::new(),
        "nothing has been edited yet"
    );
    assert_eq!(
        tagged(&session.lumps_in(Scope::Layer(0)).expect("the base")).len(),
        8,
        "the base archive reports itself unchanged"
    );
    assert_eq!(
        tagged(&session.lumps_in(Scope::Layer(1)).expect("the resource")),
        vec!["GAMEINFO@L1", "F_START@L1", "F_SKY1@L1", "F_END@L1"],
        "a layer scope never applies the merge"
    );
    assert_eq!(
        tagged(&session.lumps_in(Scope::Merged).expect("the merged view")),
        tagged(&session.merged()),
        "the merged scope is the merged view"
    );
}

#[test]
fn lumps_in_reports_an_out_of_range_layer() {
    let session = loaded();
    let error = session
        .lumps_in(Scope::Layer(2))
        .expect_err("only two layers are loaded");
    assert_eq!(error.code(), "not_found", "out of range code");
    let message = error.to_string();
    assert!(
        message.contains('2') && message.contains("working"),
        "the message says what is loaded and what to use instead: {message}"
    );
    let bare = Session::default();
    bare.lumps_in(Scope::Layer(0))
        .expect_err("no layers at all");
}
