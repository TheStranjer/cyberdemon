//! The three settings tools: the empty-call refusal, the all-or-nothing
//! option application and the resolution round-trip.

use crate::render::{Lighting, RenderSettings, TopdownStyle};
use crate::state::AppState;

use crate::tools::render::{
    SetRenderOptionsParams, apply_options, options_report, resolution_report,
};

use super::options_with;

#[test]
fn empty_options_are_refused_naming_the_options() {
    let state = AppState::default();
    let error = options_report(&state, &SetRenderOptionsParams::default())
        .expect_err("a call with no option does nothing");
    assert_eq!(error.code(), "invalid_argument", "empty options code");
    let message = error.to_string();
    for name in ["lighting", "show_things", "topdown_style", "background"] {
        assert!(
            message.contains(name),
            "the message lists {name}: {message}"
        );
    }
}

#[test]
fn options_report_lands_on_the_session() {
    let state = AppState::default();
    let args = options_with(Some("fullbright"), Some(120), None);
    let report = options_report(&state, &args).expect("two valid options");
    assert_eq!(report.lighting, Lighting::Fullbright, "lighting changed");
    assert_eq!(report.fov, 120, "fov changed");
    let stored = state
        .with_session(|session| Ok(session.render_settings().clone()))
        .expect("a fresh lock is never poisoned");
    assert_eq!(stored, report, "the session holds what it reported");
}

#[test]
fn apply_options_keeps_every_field_it_was_not_given() {
    let mut settings = RenderSettings::default();
    let before = settings.clone();
    let args = options_with(Some("FLAT"), None, Some("#102030"));
    apply_options(&mut settings, &args).expect("a valid lighting and colour");
    assert_eq!(settings.lighting, Lighting::Flat, "lighting applied");
    assert_eq!(settings.background, "#102030", "background applied");
    assert_eq!(settings.width, before.width, "resolution kept");
    assert_eq!(settings.height, before.height, "resolution kept");
    assert_eq!(settings.show_things, before.show_things, "markers kept");
    assert_eq!(settings.show_sprites, before.show_sprites, "sprites kept");
    assert_eq!(settings.sky, before.sky, "sky kept");
    assert_eq!(settings.fov, before.fov, "fov kept");
    assert_eq!(settings.gamma, before.gamma, "gamma kept");
    assert_eq!(settings.topdown_style, before.topdown_style, "style kept");
}

#[test]
fn apply_options_is_all_or_nothing() {
    let mut settings = RenderSettings::default();
    let before = settings.clone();
    let args = options_with(Some("fullbright"), Some(200), Some("#ffffff"));
    let error =
        apply_options(&mut settings, &args).expect_err("200 degrees is outside the field of view");
    assert_eq!(error.code(), "invalid_argument", "bad fov code");
    assert!(
        error.to_string().contains("fov"),
        "the message names the option that failed: {error}"
    );
    assert_eq!(
        settings, before,
        "the valid lighting and background did not land with an invalid fov"
    );
}

#[test]
fn apply_options_turns_the_switches_off() {
    let mut settings = RenderSettings::default();
    let args = SetRenderOptionsParams {
        show_things: Some(false),
        show_sprites: Some(false),
        sky: Some(false),
        gamma: Some(4),
        topdown_style: Some("Automap".to_owned()),
        ..SetRenderOptionsParams::default()
    };
    apply_options(&mut settings, &args).expect("three switches and a level");
    assert!(!settings.show_things.is_on(), "thing markers off");
    assert!(!settings.show_sprites.is_on(), "sprites off");
    assert!(!settings.sky.is_on(), "sky off");
    assert_eq!(settings.gamma, 4, "the top gamma level");
    assert_eq!(
        settings.topdown_style,
        TopdownStyle::Automap,
        "the case-insensitive style"
    );
}

#[test]
fn resolution_report_stores_the_size_it_returns() {
    let state = AppState::default();
    let report = resolution_report(&state, 1_280, 800).expect("a supported size");
    assert_eq!((report.width, report.height), (1_280, 800), "the new size");
    let error = resolution_report(&state, 8, 8).expect_err("8 is below the floor");
    assert_eq!(error.code(), "invalid_argument", "tiny size code");
    let stored = state
        .with_session(|session| Ok(session.render_settings().clone()))
        .expect("a fresh lock is never poisoned");
    assert_eq!(
        (stored.width, stored.height),
        (1_280, 800),
        "the refused size did not land"
    );
}

#[test]
fn resolution_report_accepts_the_largest_square() {
    let state = AppState::default();
    let report = resolution_report(&state, 4_096, 4_096).expect("the cap square");
    assert_eq!(report.width, 4_096, "the widest edge");
    assert_eq!(report.height, 4_096, "the tallest edge");
}
