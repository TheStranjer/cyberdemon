//! The switch and exit steps of the pipeline: wall-mounted panels and
//! the level's end.
//!
//! The switch step mounts each `switch` row's panel on its room's wall
//! ([`choose_switch_wall`]: among the one-sided room walls facing the
//! row's `wall`, the row's plan `at` point picks the nearest one and
//! centres the panel there, and without it the longest wall wins — the
//! longest outright when `wall` is unset), mounted with the
//! [`build_switch`] core at the default 32 units, and fires the `opens`
//! door's own tag — refused unless the row names a wired
//! [`DoorStyle::Remote`](crate::plan::model::DoorStyle::Remote) door.
//! The exit step ends the level: its `switch` style mounts the panel the
//! same way, reading the row's `wall` and `at` and carrying the exit
//! action, and its `walk_over` style dresses every two-sided boundary of
//! the room's sectors with [`build_walk_over_exit`].

use core::cmp::Ordering;

use crate::error::CyberError;
use crate::geom::Pt;
use crate::plan::emit::CompiledPlan;
use crate::plan::model::{ExitStyle, Feature, Plan, Side};
use crate::wad::map::Map;
use crate::wad::map::features::place::distance_fraction;
use crate::wad::map::features::{
    DEFAULT_SWITCH_WIDTH, PlacedSwitch, SwitchAction, SwitchSpec, WalkOverExit, build_switch,
    build_walk_over_exit,
};
use crate::wad::map::model::Linedef;
use crate::wad::map::ops::Point;

use super::checks::remote_door;
use super::teleporters::to_map_point;
use super::{FeatureDress, FeatureRow, FeatureRun, NO_TEXTURE, invalid, room_sectors_of};

/// The switch step: mount every `switch` row's panel on its room's wall
/// — the one its `wall` side and `at` point pick — firing the remote
/// door's tag, in plan order.
pub(super) fn switches(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for feature in plan.features.clone() {
        let Feature::Switch {
            room,
            opens,
            wall,
            repeatable,
            at,
        } = feature
        else {
            continue;
        };
        let door = remote_door(plan, run, &opens)?;
        let sectors = room_sectors_of(compiled, &room)?;
        let mount = SwitchMount {
            action: SwitchAction::OpenDoor,
            role: "switch",
            target: Some(door.band),
            repeatable,
            dress,
            choice: WallChoice {
                side: wall,
                at: wall_at(at, "switch")?,
                room: &room,
            },
        };
        let placed = mount_panel(&mut compiled.map, &sectors, &mount)?;
        run.rows.push(FeatureRow {
            kind: "switch",
            room: Some(room),
            sectors: vec![door.band],
            lines: vec![placed.linedef],
            tag: Some(placed.tag),
            line_ends: Vec::new(),
        });
    }
    Ok(())
}

/// Which wall of a room carries a mounted panel and where on it.
#[derive(Clone, Copy)]
pub(super) struct WallChoice<'room> {
    /// The compass side the wall must face, `None` for any one-sided wall.
    pub(super) side: Option<Side>,
    /// The map-unit point that picks among several matching walls,
    /// `None` for the longest one; the panel centres on it too.
    pub(super) at: Option<Point>,
    /// The room the panel is mounted in, for the wall-refusal wording.
    pub(super) room: &'room str,
}

/// One wall-mounted panel to place: what it does, which dressed role it
/// wears, where it stands and the room it stands in.
struct SwitchMount<'mount> {
    /// The action the [`build_switch`] core installs.
    action: SwitchAction,
    /// The [`FeatureDress`] role naming the panel texture.
    role: &'mount str,
    /// The sector the action fires on (tagged actions only).
    target: Option<usize>,
    /// Whether the line keeps working after use.
    repeatable: bool,
    /// The theme-resolved names the run dresses with.
    dress: &'mount FeatureDress,
    /// The wall the panel goes on and the point on it.
    choice: WallChoice<'mount>,
}

/// Choose the room's switch wall and mount `mount`'s panel centred on
/// the chosen point.
fn mount_panel(
    map: &mut Map,
    sectors: &[usize],
    mount: &SwitchMount<'_>,
) -> Result<PlacedSwitch, CyberError> {
    let linedef = choose_switch_wall(map, sectors, mount.choice)?;
    place_switch(map, linedef, mount)
}

/// Run the [`build_switch`] core for one mount: the default-width panel
/// centred on the row's point projected onto `linedef` (its midpoint
/// when the row named none), carrying the door's own tag.
fn place_switch(
    map: &mut Map,
    linedef: usize,
    mount: &SwitchMount<'_>,
) -> Result<PlacedSwitch, CyberError> {
    let texture = panel_texture(map, linedef, mount);
    let spec = SwitchSpec {
        linedef,
        at: mount.choice.at,
        width: DEFAULT_SWITCH_WIDTH,
        action: mount.action,
        target_sector: mount.target,
        repeatable: mount.repeatable,
        texture,
    };
    build_switch(map, &spec).map_err(|error| panel_hint(error, mount.choice.room))
}

/// The plan-side reading of a panel the core refused to fit: the message
/// names `room` (the core speaks only of the linedef and its length) and
/// the hint names the knob — the row cannot retune the fixed 32-unit
/// width, so its own `at` is what moves the panel onto a stretch of wall
/// that holds it. A refusal that is not an argument problem (a contract
/// break) keeps its own code, message and hint.
pub(super) fn panel_hint(error: CyberError, room: &str) -> CyberError {
    if error.code() != "invalid_argument" {
        return error;
    }
    error
        .map_message(|old| format!("room '{room}': {old}"))
        .with_hint(
            "move the row's at nearer the middle of the wall, or leave at out to centre \
             the panel on the wall",
        )
}

/// The panel texture: the theme's answer for the mount's role, or the
/// wall's own current middle when the theme left the role unnamed —
/// those faces keep what the compiler dressed, door-slice style.
fn panel_texture(map: &Map, linedef: usize, mount: &SwitchMount<'_>) -> String {
    mount
        .dress
        .texture(mount.role)
        .map_or_else(|| wall_middle(map, linedef), str::to_owned)
}

/// The chosen wall's front-side `middle` texture, `"-"` when the line
/// wears nothing.
fn wall_middle(map: &Map, linedef: usize) -> String {
    map.linedefs
        .get(linedef)
        .and_then(|line| line.front)
        .and_then(|side| map.sidedefs.get(side))
        .map_or_else(|| NO_TEXTURE.to_owned(), |side| side.middle.clone())
}

/// The exit step: end the level where every `exit` row stands, in plan
/// order — its `switch` style through the same wall-mounted panel as
/// the switch step, reading the row's `wall` and `at`, its `walk_over`
/// style on every plain two-sided boundary of the room's sectors (that
/// style mounts no panel, so it reads neither, and it leaves a boundary
/// line an earlier step dressed to its own action, warning about it).
pub(super) fn exits(
    plan: &Plan,
    compiled: &mut CompiledPlan,
    dress: &FeatureDress,
    run: &mut FeatureRun,
) -> Result<(), CyberError> {
    for feature in plan.features.clone() {
        let Feature::Exit {
            room,
            style,
            secret,
            wall,
            at,
        } = feature
        else {
            continue;
        };
        let sectors = room_sectors_of(compiled, &room)?;
        let lines = match style {
            ExitStyle::Switch => {
                let choice = WallChoice {
                    side: wall,
                    at: wall_at(at, "exit")?,
                    room: &room,
                };
                let mount = exit_mount(dress, choice, secret);
                vec![mount_panel(&mut compiled.map, &sectors, &mount)?.linedef]
            }
            ExitStyle::WalkOver => walk_over_row(compiled, &sectors, secret)?,
        };
        run.rows.push(FeatureRow {
            kind: "exit",
            room: Some(room),
            sectors,
            lines,
            tag: None,
            line_ends: Vec::new(),
        });
    }
    Ok(())
}

/// The `switch`-style exit mount: the (secret) exit action on the
/// room's chosen wall, wearing the theme's `exit_switch` panel.
const fn exit_mount<'mount>(
    dress: &'mount FeatureDress,
    choice: WallChoice<'mount>,
    secret: bool,
) -> SwitchMount<'mount> {
    let action = if secret {
        SwitchAction::SecretExit
    } else {
        SwitchAction::Exit
    };
    SwitchMount {
        action,
        role: "exit_switch",
        target: None,
        repeatable: false,
        dress,
        choice,
    }
}

/// Dress every plain two-sided boundary of `sectors` with the family's
/// walk-over level end, keeping the tags those lines carry — a plan
/// room's lines hold tag `0`, which in the `Doom` family ends the game.
/// The lines the earlier steps already dressed (a door's use lines, a
/// lift trigger, a teleporter pad border) come back in
/// [`WalkOverExit::skipped_lines`]; the run warns with them.
fn walk_over_exit(
    map: &mut Map,
    sectors: &[usize],
    secret: bool,
) -> Result<WalkOverExit, CyberError> {
    let mut built = WalkOverExit {
        special: 0_i32,
        lines: Vec::new(),
        skipped_lines: Vec::new(),
    };
    for &sector in sectors {
        let one = build_walk_over_exit(map, sector, secret)?;
        built.special = one.special;
        built.lines.extend(one.lines);
        built.skipped_lines.extend(one.skipped_lines);
    }
    built.lines.sort_unstable();
    built.lines.dedup();
    built.skipped_lines.sort_by_key(|skip| skip.line);
    built.skipped_lines.dedup();
    Ok(built)
}

/// The `walk_over` style's row: dress the room's plain boundaries and
/// turn every boundary line another feature owns into an advisory note
/// on the compiled plan.
fn walk_over_row(
    compiled: &mut CompiledPlan,
    sectors: &[usize],
    secret: bool,
) -> Result<Vec<usize>, CyberError> {
    let built = walk_over_exit(&mut compiled.map, sectors, secret)?;
    if let Some(note) = built.warning() {
        compiled.warnings.push(note);
    }
    Ok(built.lines)
}

/// The map-unit point of a wall-mounted row's plan `at`, refused when an
/// axis leaves the map's `i32` grid (`kind` words the refusal: `exit` or
/// `switch`).
fn wall_at(at: Option<Pt>, kind: &str) -> Result<Option<Point>, CyberError> {
    at.map(|point| {
        to_map_point(point).ok_or_else(|| {
            invalid(&format!(
                "the {kind} at point {}, {} is outside the map coordinate range: keep \
                 plan points and the units transform inside the map grid",
                point.0, point.1
            ))
        })
    })
    .transpose()
}

/// The linedef a wall-mounted feature takes in the room owning
/// `room_sectors`, read off `choice`: the candidates are the room's
/// one-sided walls — no back, front side in one of those sectors — whose
/// outward normal points at `choice.side` (with `None`, every one-sided
/// wall competes), and lines whose vertices do not resolve never win.
/// With `choice.at` the nearest candidate wins, measured by the exact
/// point-to-segment [`distance_fraction`] from the wall's endpoints to
/// the point; without it the longest wall wins. Either way a tie falls to the longer
/// wall and then to the lowest linedef index, the order
/// [`matching_walls`] returns. All integer: the facing test is the sign
/// of the outward normal's (`-dy`, `dx`) dot product with the side's
/// axis vector, the length comparison that of the squared run and the
/// distance comparison a cross-multiplication of two `i128` fractions.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming `choice.room` and, when given,
/// the side, when the room holds no such wall.
pub(super) fn choose_switch_wall(
    map: &Map,
    room_sectors: &[usize],
    choice: WallChoice<'_>,
) -> Result<usize, CyberError> {
    let walls = matching_walls(map, room_sectors, choice.side);
    let ranked = choice
        .at
        .map_or_else(|| longest_wall(&walls), |at| nearest_wall(&walls, at));
    ranked.ok_or_else(|| no_switch_wall(choice.room, choice.side))
}

/// The room's candidate walls facing `side` (`None`: all of them), in
/// linedef order.
fn matching_walls(map: &Map, room_sectors: &[usize], side: Option<Side>) -> Vec<Wall> {
    map.linedefs
        .iter()
        .enumerate()
        .filter_map(|(index, line)| wall_of(map, index, line, room_sectors))
        .filter(|wall| side.is_none_or(|asked| wall.faces_toward(asked)))
        .collect()
}

/// One wall a panel can mount on: its linedef `index`, its endpoints,
/// its `v1` to `v2` vector and the squared run length.
#[derive(Clone, Copy, Debug)]
struct Wall {
    /// The index into [`Map::linedefs`] the wall was read at.
    index: usize,
    /// The `v1` vertex.
    from: Point,
    /// The `v2` vertex.
    to: Point,
    /// `v2`'s `x` minus `v1`'s.
    dx: i128,
    /// `v2`'s `y` minus `v1`'s.
    dy: i128,
    /// The squared run length: the integer `longest` measure.
    squared: i128,
}

impl Wall {
    /// Whether this wall's outward normal (`-dy`, `dx`) points toward
    /// `side`: a positive dot product with the side's axis. The room
    /// sits on the front (right) side of a one-sided wall, so the void
    /// behind it is to the line's left.
    const fn faces_toward(&self, side: Side) -> bool {
        match side {
            Side::North => self.dx > 0_i128,
            Side::South => self.dx < 0_i128,
            Side::East => self.dy < 0_i128,
            Side::West => self.dy > 0_i128,
        }
    }
}

/// The [`Wall`] behind `line` at `index`: `None` when the line is
/// two-sided, its front side belongs to a sector outside
/// `room_sectors`, or either vertex does not resolve.
fn wall_of(map: &Map, index: usize, line: &Linedef, room_sectors: &[usize]) -> Option<Wall> {
    if line.back.is_some() {
        return None;
    }
    let sector = map.sidedefs.get(line.front?)?.sector;
    if !room_sectors.contains(&sector) {
        return None;
    }
    let first = map.vertices.get(line.v1)?;
    let second = map.vertices.get(line.v2)?;
    let from = Point {
        x: first.x,
        y: first.y,
    };
    let to = Point {
        x: second.x,
        y: second.y,
    };
    let dx = i128::from(second.x).saturating_sub(i128::from(first.x));
    let dy = i128::from(second.y).saturating_sub(i128::from(first.y));
    Some(Wall {
        index,
        from,
        to,
        dx,
        dy,
        squared: dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)),
    })
}

/// The longest of `walls`, which stands in linedef order: an exact tie
/// falls to the earlier wall.
fn longest_wall(walls: &[Wall]) -> Option<usize> {
    let mut best: Option<(i128, usize)> = None;
    for wall in walls {
        if best.is_none_or(|(known, _)| wall.squared > known) {
            best = Some((wall.squared, wall.index));
        }
    }
    best.map(|(_, index)| index)
}

/// The wall of `walls` standing nearest `at`: an exact tie falls to the
/// longer wall and then to the earlier one.
fn nearest_wall(walls: &[Wall], at: Point) -> Option<usize> {
    let mut best: Option<Ranked<'_>> = None;
    for wall in walls {
        let ranked = Ranked {
            wall,
            distance: distance_fraction(at, wall.from, wall.to),
        };
        if best.is_none_or(|known| ranked.beats(known)) {
            best = Some(ranked);
        }
    }
    best.map(|known| known.wall.index)
}

/// An exact `num / den` distance with a positive `den`, the shape
/// [`distance_fraction`] answers with.
type Fraction = (i128, i128);

/// One wall measured against the point a row named, ready to compare
/// with the incumbent best.
#[derive(Clone, Copy)]
struct Ranked<'wall> {
    /// The wall under measurement.
    wall: &'wall Wall,
    /// Its exact distance from the point.
    distance: Fraction,
}

impl Ranked<'_> {
    /// Whether this wall is the better mount: standing nearer than
    /// `known`, or equally near and longer.
    fn beats(self, known: Self) -> bool {
        match compare(self.distance, known.distance) {
            Ordering::Less => true,
            Ordering::Equal => self.wall.squared > known.wall.squared,
            Ordering::Greater => false,
        }
    }
}

/// The ordering of two [`Fraction`] distances, compared by exact `i128`
/// cross-multiplication (both axes of a distance are non-negative).
fn compare(first: Fraction, second: Fraction) -> Ordering {
    first
        .0
        .saturating_mul(second.1)
        .cmp(&second.0.saturating_mul(first.1))
}

/// The refusal for a room with no wall to mount on, naming the asked
/// side when there is one.
fn no_switch_wall(room: &str, wall: Option<Side>) -> CyberError {
    let facing = wall.map_or_else(
        || "no one-sided wall".to_owned(),
        |side| format!("no one-sided wall facing {}", side_name(side)),
    );
    invalid(&format!(
        "room '{room}' has {facing} to mount a switch on: every boundary line is shared \
         with another sector; pick another room, leave `wall` unset, or give the plan a \
         wall thickness"
    ))
}

/// The wire name of a compass `side`.
const fn side_name(side: Side) -> &'static str {
    match side {
        Side::North => "north",
        Side::South => "south",
        Side::East => "east",
        Side::West => "west",
    }
}
