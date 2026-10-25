//! The room-property steps' tests: a lit secret keeping both, the
//! silent vanilla secret, the explicit `none` clear, the plain room
//! earning no row and the painted-over room installing nothing.

use super::{rect_room, wired};
use crate::plan::model::Plan;
use crate::wad::map::features::LightEffect;

/// A one-room plan with the given light effect and secret flag.
fn lit_room_plan(effect: Option<LightEffect>, secret: bool) -> Plan {
    let mut room = rect_room("a", (0, 0), (128, 128));
    room.light_effect = effect;
    room.secret = secret;
    Plan {
        rooms: vec![room],
        connections: Vec::new(),
        features: Vec::new(),
        wall_thickness: 16_i64,
    }
}

#[test]
fn a_lit_secret_room_keeps_the_light_under_its_secret_bit() {
    let (compiled, rows) = wired(&lit_room_plan(Some(LightEffect::StrobeSlow), true));
    let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
    assert_eq!(kinds, vec!["light", "secret"], "lights run before secrets");
    let space = compiled.map.sectors.first().expect("one sector");
    assert_eq!(space.special, 131_i32, "strobe 3 keeps the Boom bit 128");
    assert_eq!(space.light, 160_i32, "the level is kept");
    assert!(
        compiled
            .warnings
            .iter()
            .any(|note| note.contains("sector 0 now carries special 131")),
        "the vanilla warning passes through: {:?}",
        compiled.warnings
    );
    for row in &rows {
        assert_eq!(row.room.as_deref(), Some("a"), "the lit room");
        assert_eq!(row.sectors, vec![0_usize], "the room's one sector");
        assert!(row.lines.is_empty(), "a painted special owns no line");
        assert_eq!(row.tag, None, "and no tag");
    }
}

#[test]
fn an_unlit_secret_room_takes_the_vanilla_type_in_silence() {
    let (compiled, rows) = wired(&lit_room_plan(None, true));
    let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
    assert_eq!(kinds, vec!["secret"], "no effect, no light row");
    assert_eq!(
        compiled.map.sectors.first().expect("one sector").special,
        9_i32,
        "the plain sector takes the vanilla secret type"
    );
    assert!(
        !compiled
            .warnings
            .iter()
            .any(|note| note.contains("Boom bits")),
        "the vanilla 9 needs no warning: {:?}",
        compiled.warnings
    );
}

#[test]
fn an_explicit_none_effect_clears_the_type_like_the_tool_does() {
    let (compiled, rows) = wired(&lit_room_plan(Some(LightEffect::None), false));
    let kinds: Vec<&str> = rows.iter().map(|row| row.kind).collect();
    assert_eq!(kinds, vec!["light"], "Some(none) is still an effect");
    assert_eq!(
        compiled.map.sectors.first().expect("one sector").special,
        0_i32,
        "the basic type clears"
    );
}

#[test]
fn a_plain_room_earns_neither_light_nor_secret_row() {
    let (compiled, rows) = wired(&lit_room_plan(None, false));
    assert!(rows.is_empty(), "nothing to paint: {rows:?}");
    assert_eq!(
        compiled.map.sectors.first().expect("one sector").special,
        0_i32
    );
}

#[test]
fn a_painted_over_lit_room_installs_nothing_and_reports_no_row() {
    let mut lit = rect_room("a", (0, 0), (128, 128));
    lit.light_effect = Some(LightEffect::Glow);
    lit.secret = true;
    let plan = Plan {
        rooms: vec![lit, rect_room("b", (0, 0), (128, 128))],
        connections: Vec::new(),
        features: Vec::new(),
        wall_thickness: 16_i64,
    };
    let (compiled, rows) = wired(&plan);
    assert!(
        rows.is_empty(),
        "the vanished room paints nothing: {rows:?}"
    );
    assert!(
        compiled
            .warnings
            .iter()
            .any(|note| note.contains("painted over")),
        "the compiler already said so: {:?}",
        compiled.warnings
    );
}
