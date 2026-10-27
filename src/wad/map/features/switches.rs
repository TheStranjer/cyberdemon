//! The pure switch core: the action a wall-mounted switch installs and
//! the map edits that hang one on a wall.
//!
//! [`switch_special`] turns a flag family, a [`SwitchAction`] and the tag
//! of the sector the switch acts on into the same [`DoorLineSetup`]
//! [`super::doors`] writes onto a door's use lines: the line special, the
//! `Doom` trigger tag, the `Hexen` script arguments and the `Hexen`
//! activation bits. [`build_switch`] is the placement side:
//! [`project_onto_line`] and [`door_segment`] fit a
//! [`DEFAULT_SWITCH_WIDTH`]-wide panel chunk to a named wall,
//! [`isolate_chunk`] cuts it out as its own linedef with
//! [`crate::wad::map::ops::split_linedef`], [`switch_tag`] resolves the
//! tag the action fires on — the target sector's own, or a fresh
//! [`next_free_tag`] stamped onto it when it carries none — and
//! [`dress_switch`] writes the [`switch_special`] row onto the line and
//! the panel texture onto its front middle. [`build_walk_over_exit`] is
//! the floor-side twin of the level end: every two-sided boundary of a
//! sector takes the family's walk-over special from
//! [`walk_over_special`], existing tags kept — in the `Doom` family a
//! `W1` `52`/`124` reads the line's tag as the destination map number,
//! so a plain `0` ends the game. The tools parse their wire parameters;
//! like the rest of [`crate::wad`] this is pure model arithmetic and
//! table lookup: no sessions, no archives, no `MCP`.
//!
//! `Exit` and `SecretExit` need no tag and have no repeatable form: the
//! `Doom` family only ships `S1` exits (`11` and `51`), and a level change
//! cannot fire twice, so `repeatable` is ignored for both actions in both
//! families.

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::map::flags::{ACTIVATION_MASK, FlagFamily, PLAYER_USE_BITS};
use crate::wad::map::model::{Linedef, Map};
use crate::wad::map::ops::{self, Point};
use crate::wad::map::query::{ElementKind, check_index, missing_index};

use super::doors::{DoorLineSetup, USE_REPEAT, next_free_tag};
use super::place::{VertexPair, door_segment, endpoints, project_onto_line};

/// What a switch is meant to do when the player uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchAction {
    /// Opens the target door (and closes it again after its wait, the
    /// `Door Stay Open` specials).
    OpenDoor,
    /// Ends the level.
    Exit,
    /// Ends the level through the secret exit.
    SecretExit,
    /// Raises the target floor to the highest neighbouring floor.
    RaiseFloor,
    /// Lowers the target floor to the lowest neighbouring floor.
    LowerFloor,
}

impl SwitchAction {
    /// Every action, in table order.
    #[must_use]
    #[inline]
    pub const fn all() -> [Self; 5] {
        [
            Self::OpenDoor,
            Self::Exit,
            Self::SecretExit,
            Self::RaiseFloor,
            Self::LowerFloor,
        ]
    }

    /// Whether this action acts on a tagged sector and so needs one: the
    /// exits change the level itself and carry no tag.
    #[must_use]
    #[inline]
    pub const fn needs_target(self) -> bool {
        matches!(self, Self::OpenDoor | Self::RaiseFloor | Self::LowerFloor)
    }

    /// Whether `repeatable` can mean anything for this action: the exits
    /// fire once in every family (the `Doom` table simply has no `SR`
    /// exit row; a level change is over after its first use).
    #[must_use]
    #[inline]
    pub const fn takes_repeat(self) -> bool {
        self.needs_target()
    }
}

impl crate::named::Named for SwitchAction {
    const ALL: &'static [Self] = &Self::all();

    #[inline]
    fn name(self) -> &'static str {
        action_name(self)
    }
}

/// The [`DoorLineSetup`] a switch of `action` carries in flag family
/// `family`, triggered `repeatable` times or once, acting on tag `tag`.
///
/// The table:
///
/// | Action | `Doom` once | `Doom` repeated | `Hexen` |
/// |---|---|---|---|
/// | [`OpenDoor`] | `103` (`S1` door stays open) | `61` (`SR`) | `11` `Door_Open`, args `[tag, 16]` |
/// | [`RaiseFloor`] | `18` (`S1` to highest adjacent) | `69` (`SR`) | `25` `Floor_RaiseToNearest`, args `[tag, 8]` |
/// | [`LowerFloor`] | `23` (`S1` to lowest adjacent) | `60` (`SR`) | `21` `Floor_LowerToLowest`, args `[tag, 8]` |
/// | [`Exit`] | `11` | `11` (no `SR` exit exists) | `243` `Portal` |
/// | [`SecretExit`] | `51` | `51` (no `SR` exit exists) | `244` `Portal`, secret |
///
/// `Doom` tagged setups carry `tag` in [`DoorLineSetup::tag`] (the sector
/// the switch moves answers with the same one); their args and activation
/// stay zero. `Hexen` setups carry no line tag at all: the tag rides in
/// `args[0]` (the floor/door speed goes to `args[1]`), and
/// [`DoorLineSetup::flags_activation`] holds `playeruse`, plus
/// `repeatspecial` when `repeatable` applies. The exit actions ignore
/// `tag` (they level-end, targeting nothing) and `repeatable` (a level
/// change fires once, and the `Doom` family has no repeatable exit at
/// all): `11`/`51` with tag 0 there, `243`/`244` with all-zero arguments
/// and plain `playeruse` here.
///
/// [`OpenDoor`]: SwitchAction::OpenDoor
/// [`RaiseFloor`]: SwitchAction::RaiseFloor
/// [`LowerFloor`]: SwitchAction::LowerFloor
/// [`Exit`]: SwitchAction::Exit
/// [`SecretExit`]: SwitchAction::SecretExit
#[must_use]
#[inline]
pub const fn switch_special(
    family: FlagFamily,
    action: SwitchAction,
    repeatable: bool,
    tag: i32,
) -> DoorLineSetup {
    match family {
        FlagFamily::Doom => doom_switch(action, repeatable, tag),
        FlagFamily::Hexen => hexen_switch(action, repeatable, tag),
    }
}

/// The `Doom`-family row of [`switch_special`]: a vanilla special (the
/// `SR` form when the action repeats), the targeted tag for the sector
/// actions, no arguments and no activation bits.
const fn doom_switch(action: SwitchAction, repeatable: bool, tag: i32) -> DoorLineSetup {
    let (once, again) = match action {
        SwitchAction::OpenDoor => (103_i32, 61_i32),
        SwitchAction::Exit => (11_i32, 11_i32),
        SwitchAction::SecretExit => (51_i32, 51_i32),
        SwitchAction::RaiseFloor => (18_i32, 69_i32),
        SwitchAction::LowerFloor => (23_i32, 60_i32),
    };
    DoorLineSetup {
        special: if repeatable { again } else { once },
        tag: if action.needs_target() { tag } else { 0_i32 },
        args: [0_i32; 5],
        flags_activation: 0_u32,
    }
}

/// The `Hexen`-family row of [`switch_special`]: a tagged script special
/// for the sector actions (`args[0]` carries the tag, `args[1]` the
/// speed) or a bare portal for the exits, plain `playeruse` activation
/// plus `repeatspecial` where the action repeats, and no line tag.
const fn hexen_switch(action: SwitchAction, repeatable: bool, tag: i32) -> DoorLineSetup {
    let (special, speed) = match action {
        SwitchAction::OpenDoor => (11_i32, 16_i32),
        SwitchAction::Exit => (243_i32, 0_i32),
        SwitchAction::SecretExit => (244_i32, 0_i32),
        SwitchAction::RaiseFloor => (25_i32, 8_i32),
        SwitchAction::LowerFloor => (21_i32, 8_i32),
    };
    let args = if action.needs_target() {
        [tag, speed, 0_i32, 0_i32, 0_i32]
    } else {
        [0_i32; 5]
    };
    DoorLineSetup {
        special,
        tag: 0_i32,
        args,
        flags_activation: if repeatable && action.takes_repeat() {
            USE_REPEAT
        } else {
            PLAYER_USE_BITS
        },
    }
}

/// The default switch panel width in map units: one switch face.
pub const DEFAULT_SWITCH_WIDTH: i32 = 32_i32;

/// The canonical wire name of `action`.
#[must_use]
#[inline]
pub const fn action_name(action: SwitchAction) -> &'static str {
    match action {
        SwitchAction::OpenDoor => "open_door",
        SwitchAction::Exit => "exit",
        SwitchAction::SecretExit => "secret_exit",
        SwitchAction::RaiseFloor => "raise_floor",
        SwitchAction::LowerFloor => "lower_floor",
    }
}

/// Everything [`build_switch`] needs once the wire parameters resolved.
#[derive(Debug)]
pub struct SwitchSpec {
    /// The wall to mount the panel on.
    pub linedef: usize,
    /// The requested panel centre, if any.
    pub at: Option<Point>,
    /// The panel width in map units.
    pub width: i32,
    /// The action to install.
    pub action: SwitchAction,
    /// The sector the switch acts on (sector-moving actions).
    pub target_sector: Option<usize>,
    /// Whether the line keeps working after use.
    pub repeatable: bool,
    /// The theme-resolved panel texture.
    pub texture: String,
}

/// The [`Map`] endpoints of the wall switch `linedef`, refused with
/// switch-role instructions when the index dangles or a vertex is gone
/// ([`door_segment`] speaks of doors, so the wall is checked first).
///
/// # Errors
///
/// [`CyberError::NotFound`] when `index` is past the map's linedefs,
/// [`CyberError::InvalidArgument`] when the line names a vertex the map
/// does not hold.
#[inline]
pub fn switch_span(map: &Map, index: usize) -> Result<(Point, Point), CyberError> {
    check_index(ElementKind::Linedefs, index, map.linedefs.len())?;
    endpoints(map, index).ok_or_else(|| {
        CyberError::invalid(&format!(
            "switch linedef {index} references a vertex the map does not hold: resolve \
             its endpoints first"
        ))
    })
}

/// The `width`-wide panel chunk of the wall, centred on `at` (or the
/// midpoint).
///
/// [`project_onto_line`] and [`door_segment`] on a wall already checked
/// for [`switch_span`], so only the width, zero-length and fit refusals
/// can reach the caller.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `spec.width` is not positive,
/// the wall does not resolve, or the panel does not fit it centred
/// here.
#[inline]
pub fn switch_cut(map: &Map, spec: &SwitchSpec) -> Result<(VertexPair, i32), CyberError> {
    switch_span(map, spec.linedef)?;
    if spec.width <= 0_i32 {
        return Err(CyberError::invalid(
            "the switch width must be a positive number of map units (the default 32 \
             covers one switch panel)",
        ));
    }
    let centre = project_onto_line(map, spec.linedef, spec.at);
    door_segment(map, spec.linedef, centre, spec.width)
}

/// The isolated panel chunk: which linedef carries it and whether the
/// wall had to be cut.
#[derive(Debug)]
pub struct Chunk {
    /// Index in [`Map::linedefs`] of the switch line.
    pub line: usize,
    /// Whether [`build_switch`] split the wall to isolate the chunk.
    pub split: bool,
}

/// Make the `start`-`finish` chunk of linedef `linedef` its own linedef.
///
/// Cuts with [`ops::split_linedef`] only where the chunk does not
/// already meet an endpoint; the chunk keeps the wall's sides (split
/// copies them) and the pieces beyond its ends stand aside.
///
/// # Errors
///
/// [`switch_span`]'s refusals for the wall, and
/// [`crate::wad::map::ops::split_linedef`]'s for a cut the map cannot
/// store.
#[inline]
pub fn isolate_chunk(
    map: &mut Map,
    linedef: usize,
    start: Point,
    finish: Point,
) -> Result<Chunk, CyberError> {
    let (from, to) = switch_span(map, linedef)?;
    let mut chunk = Chunk {
        line: linedef,
        split: false,
    };
    if start != from {
        chunk.line = ops::split_linedef(map, chunk.line, Some(start))?.new_linedef;
        chunk.split = true;
    }
    if finish != to {
        ops::split_linedef(map, chunk.line, Some(finish))?;
        chunk.split = true;
    }
    Ok(chunk)
}

/// The refusal for a sector-moving action that names no target sector.
fn target_required(action: SwitchAction) -> CyberError {
    CyberError::invalid(&format!(
        "the {} action needs target_sector: pass the index of the sector the switch \
         moves (add_door reports the door sector of its remote door)",
        action_name(action)
    ))
}

/// The tag the switch line answers with: the target sector's existing
/// one, or a fresh [`next_free_tag`] stamped onto the sector when it
/// carries none. Exit actions level-end and act on no tag.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when a sector-moving action names no
/// `target_sector`, [`CyberError::NotFound`] for a `target_sector` past
/// the map's sectors.
#[inline]
pub fn switch_tag(map: &mut Map, spec: &SwitchSpec) -> Result<i32, CyberError> {
    if !spec.action.needs_target() {
        return Ok(0_i32);
    }
    let index = spec
        .target_sector
        .ok_or_else(|| target_required(spec.action))?;
    let current = map
        .sectors
        .get(index)
        .ok_or_else(|| missing_index(ElementKind::Sectors, index, map.sectors.len()))?
        .tag;
    if current != 0_i32 {
        return Ok(current);
    }
    let fresh = next_free_tag(map);
    if let Some(sector) = map.sectors.get_mut(index) {
        sector.tag = fresh;
    }
    Ok(fresh)
}

/// The front sidedef index of the switch `line`, when the line has one
/// and the sidedef resolves.
fn switch_side(map: &Map, line: usize) -> Option<usize> {
    map.linedefs
        .get(line)?
        .front
        .filter(|index| map.sidedefs.get(*index).is_some())
}

/// Write the action onto the isolated line and the panel onto its front
/// side; the `Hexen` family replaces the activation field with the
/// setup's `playeruse` bits, the `Doom` family keeps the line's flags.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `line` has no front sidedef to
/// wear the panel.
#[inline]
pub fn dress_switch(
    map: &mut Map,
    line: usize,
    setup: &DoorLineSetup,
    texture: &str,
) -> Result<(), CyberError> {
    let hexen = map.flag_family() == FlagFamily::Hexen;
    let Some(side) = switch_side(map, line) else {
        return Err(CyberError::invalid(&format!(
            "linedef {line} has no front sidedef to wear the switch: give the wall a \
             front side facing the triggering room first (see add_sidedef)"
        )));
    };
    if let Some(def) = map.linedefs.get_mut(line) {
        def.special = setup.special;
        def.tag = setup.tag;
        def.args = setup.args;
        if hexen {
            def.flags = (def.flags & !ACTIVATION_MASK) | setup.flags_activation;
        }
    }
    if let Some(worn) = map.sidedefs.get_mut(side) {
        texture.clone_into(&mut worn.middle);
    }
    Ok(())
}

/// What one [`build_switch`] step did, as the tool reports it.
#[derive(Debug, Serialize)]
pub struct PlacedSwitch {
    /// Index of the linedef carrying the switch (new when split).
    pub linedef: usize,
    /// Whether the wall was split to isolate the panel.
    pub split: bool,
    /// The canonical action name the switch got.
    pub action: &'static str,
    /// The line special installed.
    pub special: i32,
    /// The tag the line answers with (`0` for the exit actions).
    pub tag: i32,
    /// The panel texture installed on the front middle.
    pub texture: String,
}

/// The whole switch as one map edit: fit the chunk, isolate it, resolve
/// the tag it answers with and dress the line with the table's action.
///
/// # Errors
///
/// [`switch_cut`]'s, [`crate::wad::map::ops::split_linedef`]'s,
/// [`switch_tag`]'s and [`dress_switch`]'s refusals, all naming the
/// wall, the width or the sector to fix.
#[inline]
pub fn build_switch(map: &mut Map, spec: &SwitchSpec) -> Result<PlacedSwitch, CyberError> {
    let ((start, finish), _available) = switch_cut(map, spec)?;
    let chunk = isolate_chunk(map, spec.linedef, start, finish)?;
    let tag = switch_tag(map, spec)?;
    let setup = switch_special(map.flag_family(), spec.action, spec.repeatable, tag);
    dress_switch(map, chunk.line, &setup, &spec.texture)?;
    Ok(PlacedSwitch {
        linedef: chunk.line,
        split: chunk.split,
        action: action_name(spec.action),
        special: setup.special,
        tag,
        texture: spec.texture.clone(),
    })
}

/// The walk-over exit special flag family `family` installs for a
/// normal or `secret` level end: the `Doom` family's `W1` 52/124, the
/// `Hexen` family's portal 243/244.
#[must_use]
#[inline]
pub const fn walk_over_special(family: FlagFamily, secret: bool) -> i32 {
    match family {
        FlagFamily::Doom => {
            if secret {
                124_i32
            } else {
                52_i32
            }
        }
        FlagFamily::Hexen => {
            if secret {
                244_i32
            } else {
                243_i32
            }
        }
    }
}

/// Whether `line` is two-sided (both sides resolving to sectors) with
/// `sector` opening on either of them.
fn sector_boundary(map: &Map, line: &Linedef, sector: usize) -> bool {
    let (front, back) = map.line_sectors(line);
    front.is_some() && back.is_some() && (front == Some(sector) || back == Some(sector))
}

/// The lines [`build_walk_over_exit`] dressed and the special they carry.
#[derive(Debug)]
pub struct WalkOverExit {
    /// The level-end special installed on every boundary line.
    pub special: i32,
    /// Indices of the two-sided lines that now end the level.
    pub lines: Vec<usize>,
}

/// Set one boundary line's level-end special; in the `Hexen` family
/// clear its activation field to the `playercross` value (the field's
/// zero), keeping the other flag bits, the tag and the args.
fn mark_walk_over(map: &mut Map, index: usize, special: i32, hexen: bool) {
    if let Some(line) = map.linedefs.get_mut(index) {
        line.special = special;
        if hexen {
            line.flags &= !ACTIVATION_MASK;
        }
    }
}

/// Dress every two-sided boundary of `sector` with the family's
/// walk-over level end.
///
/// The `Doom` family's `W1` 52/124 read the line's tag as the
/// destination map number, so existing tags stay as they are (a plain
/// 0 ends the game, or the secret level on 124).
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `sector` is past the map's
/// sectors or the sector has no two-sided boundary to walk over.
#[inline]
pub fn build_walk_over_exit(
    map: &mut Map,
    sector: usize,
    secret: bool,
) -> Result<WalkOverExit, CyberError> {
    check_index(ElementKind::Sectors, sector, map.sectors.len())?;
    let special = walk_over_special(map.flag_family(), secret);
    let hexen = map.flag_family() == FlagFamily::Hexen;
    let lines: Vec<usize> = map
        .linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| sector_boundary(map, line, sector))
        .map(|(index, _line)| index)
        .collect();
    if lines.is_empty() {
        return Err(CyberError::invalid(&format!(
            "sector {sector} has no two-sided boundary to walk over: draw a shared wall \
             to the next sector (add_door, or create_sector_from_loop overlapping it) or \
             trigger the exit with kind \"switch\" instead"
        )));
    }
    for index in &lines {
        mark_walk_over(map, *index, special, hexen);
    }
    Ok(WalkOverExit { special, lines })
}

#[cfg(test)]
mod tests {
    use super::{SwitchAction, switch_special};
    use crate::wad::map::features::DoorLineSetup;
    use crate::wad::map::flags::FlagFamily;

    /// The tag the table tests ask `switch_special` for.
    const SAMPLE_TAG: i32 = 42;

    /// A plain `Doom` use-row: tag on the line, no args, no activation.
    const fn doom(special: i32, tag: i32) -> DoorLineSetup {
        DoorLineSetup {
            special,
            tag,
            args: [0_i32; 5],
            flags_activation: 0_u32,
        }
    }

    /// A `Hexen` script row: no line tag, `playeruse` (`0x400`) or
    /// `playeruse` + `repeatspecial` (`0x600`).
    const fn hexen(special: i32, args: [i32; 5], repeat: bool) -> DoorLineSetup {
        DoorLineSetup {
            special,
            tag: 0_i32,
            args,
            flags_activation: if repeat { 0x600_u32 } else { 0x400_u32 },
        }
    }

    #[test]
    fn action_list_and_target_rule() {
        assert_eq!(SwitchAction::all().len(), 5_usize, "five actions");
        for action in SwitchAction::all() {
            let targeted = matches!(
                action,
                SwitchAction::OpenDoor | SwitchAction::RaiseFloor | SwitchAction::LowerFloor
            );
            assert_eq!(action.needs_target(), targeted, "{action:?} targeting");
            assert_eq!(action.takes_repeat(), targeted, "{action:?} repeating");
        }
    }

    #[test]
    fn switch_special_matches_the_doom_table() {
        let once = [
            (SwitchAction::OpenDoor, 103_i32, SAMPLE_TAG),
            (SwitchAction::Exit, 11_i32, 0_i32),
            (SwitchAction::SecretExit, 51_i32, 0_i32),
            (SwitchAction::RaiseFloor, 18_i32, SAMPLE_TAG),
            (SwitchAction::LowerFloor, 23_i32, SAMPLE_TAG),
        ];
        let repeated = [
            (SwitchAction::OpenDoor, 61_i32, SAMPLE_TAG),
            (SwitchAction::Exit, 11_i32, 0_i32),
            (SwitchAction::SecretExit, 51_i32, 0_i32),
            (SwitchAction::RaiseFloor, 69_i32, SAMPLE_TAG),
            (SwitchAction::LowerFloor, 60_i32, SAMPLE_TAG),
        ];
        assert_eq!(once.len(), SwitchAction::all().len(), "the table is whole");
        for (action, special, tag) in once {
            let setup = switch_special(FlagFamily::Doom, action, false, SAMPLE_TAG);
            assert_eq!(setup, doom(special, tag), "the once row of {action:?}");
        }
        for (action, special, tag) in repeated {
            let setup = switch_special(FlagFamily::Doom, action, true, SAMPLE_TAG);
            assert_eq!(
                setup,
                doom(special, tag),
                "the repeatable row of {action:?}"
            );
        }
    }

    #[test]
    fn switch_special_matches_the_hexen_table() {
        let expected = [
            (
                SwitchAction::OpenDoor,
                hexen(11_i32, [SAMPLE_TAG, 16_i32, 0_i32, 0_i32, 0_i32], false),
                hexen(11_i32, [SAMPLE_TAG, 16_i32, 0_i32, 0_i32, 0_i32], true),
            ),
            (
                SwitchAction::Exit,
                hexen(243_i32, [0_i32; 5], false),
                hexen(243_i32, [0_i32; 5], false),
            ),
            (
                SwitchAction::SecretExit,
                hexen(244_i32, [0_i32; 5], false),
                hexen(244_i32, [0_i32; 5], false),
            ),
            (
                SwitchAction::RaiseFloor,
                hexen(25_i32, [SAMPLE_TAG, 8_i32, 0_i32, 0_i32, 0_i32], false),
                hexen(25_i32, [SAMPLE_TAG, 8_i32, 0_i32, 0_i32, 0_i32], true),
            ),
            (
                SwitchAction::LowerFloor,
                hexen(21_i32, [SAMPLE_TAG, 8_i32, 0_i32, 0_i32, 0_i32], false),
                hexen(21_i32, [SAMPLE_TAG, 8_i32, 0_i32, 0_i32, 0_i32], true),
            ),
        ];
        assert_eq!(
            expected.len(),
            SwitchAction::all().len(),
            "the table is whole"
        );
        for (action, once, repeated) in expected {
            assert_eq!(
                switch_special(FlagFamily::Hexen, action, false, SAMPLE_TAG),
                once,
                "the once row of {action:?}: playeruse, no line tag"
            );
            assert_eq!(
                switch_special(FlagFamily::Hexen, action, true, SAMPLE_TAG),
                repeated,
                "the repeatable row of {action:?}"
            );
        }
    }

    #[test]
    fn exit_actions_ignore_their_parameters_everywhere() {
        let probes = [
            (FlagFamily::Doom, SwitchAction::Exit),
            (FlagFamily::Doom, SwitchAction::SecretExit),
            (FlagFamily::Hexen, SwitchAction::Exit),
            (FlagFamily::Hexen, SwitchAction::SecretExit),
        ];
        for (family, action) in probes {
            let setup = switch_special(family, action, true, SAMPLE_TAG);
            assert_eq!(setup.tag, 0_i32, "{family:?} {action:?} carries no tag");
            assert_eq!(setup.args, [0_i32; 5], "{family:?} {action:?} has no args");
        }
    }
}
