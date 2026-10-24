//! Pure lift core: the platform's activation edges and the down-wait-up
//! action each family runs on them.
//!
//! A lift is a sector standing at a high floor whose edges look down on
//! lower ground. [`build_lift`] takes a [`LiftSpec`] naming the platform
//! — an existing sector ([`LiftPlatform::Existing`]) or a polygon folded
//! over the map with [`apply_overlay`](crate::wad::map::overlay::apply_overlay)
//! ([`LiftPlatform::Draw`], whose floor, ceiling, flats and light come
//! from the sector under its
//! centre) — resolves the floor it lowers to (the explicit
//! [`LiftSpec::low_floor`], else the lowest neighbouring floor found
//! through [`neighbors`](crate::wad::map::geometry::neighbors)), stamps the
//! platform with a fresh [`next_free_tag`](super::doors::next_free_tag), selects
//! every boundary edge that faces a lower
//! sector and turns each one so its front looks down at the player who
//! triggers the ride, writes the [`lift_specials`] action onto it and
//! dresses the low-facing side's `lower` band with the caller's
//! [`LiftSpec::side`] texture. A drop under [`MIN_LIFT_DROP`] succeeds
//! with the note "a lift is not needed; stairs or a step will do": that
//! taste stays in the report, not in a refusal. Like the rest of
//! [`crate::wad`] this is pure model arithmetic and table lookup — no
//! sessions, no archives, no `MCP` — and every refusal on an existing
//! platform lands before a single row is written.

mod build;

pub use build::build_lift;

use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::geom::Pt;
use crate::wad::map::flags::{ACTIVATION_MASK, FlagFamily};

use super::doors::{DoorLineSetup, USE, USE_REPEAT};

/// The `blocking` linedef flag bit, cleared on the activation lines so
/// the player can reach (and walk across) the lowered platform.
const BLOCKING_BIT: u32 = 1_u32;
/// The model's name for "no texture here" on a [`Sidedef`] texture face.
///
/// [`Sidedef`]: crate::wad::map::Sidedef
const NO_TEXTURE: &str = "-";
/// The `Hexen` `repeatspecial` bit, added on top of either the
/// `playeruse` or `playercross` activation for a repeating lift line.
const REPEAT_SPECIAL_BIT: u32 = 0x200;
/// The `Hexen` trigger bits [`dress_edge`](build::dress_edge) replaces
/// wholesale: the 3-bit activation field plus the `repeatspecial` bit,
/// so a one-shot clears both and a repeating line gets exactly what
/// [`lift_specials`] names.
const TRIGGER_MASK: u32 = ACTIVATION_MASK | REPEAT_SPECIAL_BIT;
/// The `Hexen` `playercross` activation: the activation field's zero
/// value, crossed rather than used.
const CROSS: u32 = 0_u32;
/// The `Hexen` lift line special: `Plat_DownWaitUpStayLip`.
const HEXEN_PLAT_SPECIAL: i32 = 206;
/// The `Hexen` lift's speed argument, in map units per game tic.
const PLAT_SPEED: i32 = 32;
/// The `Hexen` lift's wait-at-bottom argument, in game tics.
const PLAT_DELAY: i32 = 105;
/// The smallest drop worth a lift: below this the build succeeds but
/// notes that stairs or a step will do.
pub const MIN_LIFT_DROP: i32 = 64;
/// The fewest corners a drawn platform polygon may carry.
const MIN_PLATFORM_CORNERS: usize = 3;

/// How the player starts the lift ride from below.
///
/// The serde names are the `snake_case` variant names (`use_wall`,
/// `walk_trigger`), so a plan spells a style the way the tools do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiftStyle {
    /// The player presses the platform's side wall from the lower area.
    UseWall,
    /// Crossing a boundary edge of the platform starts the ride.
    WalkTrigger,
}

impl LiftStyle {
    /// Every style, in table order.
    #[must_use]
    #[inline]
    pub const fn all() -> [Self; 2] {
        [Self::UseWall, Self::WalkTrigger]
    }
}

/// The [`DoorLineSetup`] a lift of `style` installs in flag family
/// `family`, triggered `repeatable` times or once, answering with the
/// platform's `tag`.
///
/// The table:
///
/// | Style | `Doom` once | `Doom` repeated | `Hexen` |
/// |---|---|---|---|
/// | [`UseWall`] | `21` (`S1` floor to lowest adjacent) | `62` (`SR`) | `206` `Plat_DownWaitUpStayLip`, args `[tag, 32, 105, 0]`, `playeruse` |
/// | [`WalkTrigger`] | `10` (`W1`) | `88` (`WR`) | the same special, `playercross` |
///
/// `Doom` setups carry `tag` in [`DoorLineSetup::tag`] (the platform
/// sector answers with the same one); their args and activation stay
/// zero. `Hexen` setups carry no line tag at all: the tag rides in
/// `args[0]` with the speed in `args[1]` and the bottom wait in
/// `args[2]`, and [`DoorLineSetup::flags_activation`] holds `playeruse`
/// for [`UseWall`] or the field's zero value (`playercross`) for
/// [`WalkTrigger`], plus `repeatspecial` when `repeatable`.
///
/// [`UseWall`]: LiftStyle::UseWall
/// [`WalkTrigger`]: LiftStyle::WalkTrigger
#[must_use]
#[inline]
pub const fn lift_specials(
    family: FlagFamily,
    style: LiftStyle,
    repeatable: bool,
    tag: i32,
) -> DoorLineSetup {
    match family {
        FlagFamily::Doom => doom_lift(style, repeatable, tag),
        FlagFamily::Hexen => hexen_lift(style, repeatable, tag),
    }
}

/// The `Doom`-family row of [`lift_specials`]: the vanilla down-wait-up
/// special (the `S1`/`W1` once form or its `SR`/`WR` repeat), the
/// platform's tag on the line, no arguments and no activation bits.
const fn doom_lift(style: LiftStyle, repeatable: bool, tag: i32) -> DoorLineSetup {
    let (once, again) = match style {
        LiftStyle::UseWall => (21_i32, 62_i32),
        LiftStyle::WalkTrigger => (10_i32, 88_i32),
    };
    DoorLineSetup {
        special: if repeatable { again } else { once },
        tag,
        args: [0_i32; 5],
        flags_activation: 0_u32,
    }
}

/// The `Hexen`-family row of [`lift_specials`]: `Plat_DownWaitUpStayLip`
/// targeting the tagged sector through `args[0]`, `use` or `cross`
/// activation and no line tag.
const fn hexen_lift(style: LiftStyle, repeatable: bool, tag: i32) -> DoorLineSetup {
    let activation = match (style, repeatable) {
        (LiftStyle::UseWall, true) => USE_REPEAT,
        (LiftStyle::UseWall, false) => USE,
        (LiftStyle::WalkTrigger, true) => CROSS | REPEAT_SPECIAL_BIT,
        (LiftStyle::WalkTrigger, false) => CROSS,
    };
    DoorLineSetup {
        special: HEXEN_PLAT_SPECIAL,
        tag: 0_i32,
        args: [tag, PLAT_SPEED, PLAT_DELAY, 0_i32, 0_i32],
        flags_activation: activation,
    }
}

/// The platform the parameters name: an existing sector or a polygon to
/// draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiftPlatform {
    /// The index in
    /// [`Map::sectors`](crate::wad::map::model::Map::sectors) of the existing
    /// platform sector.
    Existing(usize),
    /// The corners of a new platform's polygon, in order, implicitly
    /// closed; it takes its heights from the sector under its centre.
    Draw(Vec<Pt>),
}

/// Everything one lift needs, as the tool layer resolved it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftSpec {
    /// The platform: an existing sector or the polygon to draw.
    pub platform: LiftPlatform,
    /// The floor the platform lowers to; the lowest neighbouring floor
    /// by default.
    pub low_floor: Option<i32>,
    /// How the ride is triggered from below.
    pub style: LiftStyle,
    /// Whether the activation lines keep working after use.
    pub repeatable: bool,
    /// The lift's side texture, worn by every activation edge's
    /// low-facing `lower` band.
    pub side: String,
    /// The platform's floor flat (`"-"` keeps the sector's own).
    pub floor_flat: String,
}

/// What one [`build_lift`] call did, as the tool reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LiftBuild {
    /// The platform sector's index in
    /// [`Map::sectors`](crate::wad::map::model::Map::sectors) (new when the
    /// platform was drawn).
    pub platform_sector: usize,
    /// The fresh tag stamped onto the sector and answered by the lines.
    pub tag: i32,
    /// The indices of the activation lines, ascending.
    pub lines: Vec<usize>,
    /// The platform's high floor.
    pub floor_height: i32,
    /// The floor the platform lowers to.
    pub low_floor: i32,
    /// `floor_height - low_floor`, the ride's length.
    pub drop: i32,
    /// What the drawing reported, plus the small-drop note when
    /// `drop < MIN_LIFT_DROP`.
    pub notes: Vec<String>,
}

/// One resolved side of a line: its sidedef, sector and floor height.
#[derive(Clone, Copy, Debug)]
struct Side {
    /// Index in
    /// [`Map::sidedefs`](crate::wad::map::model::Map::sidedefs).
    sidedef: usize,
    /// Index in
    /// [`Map::sectors`](crate::wad::map::model::Map::sectors) the side opens
    /// into.
    sector: usize,
    /// That sector's floor height.
    floor: i32,
}

/// An activation edge: the line and the side that must face the lower
/// area once the ride is installed.
#[derive(Clone, Copy, Debug)]
struct Edge {
    /// Index in
    /// [`Map::linedefs`](crate::wad::map::model::Map::linedefs) of the line.
    line: usize,
    /// Index in
    /// [`Map::sidedefs`](crate::wad::map::model::Map::sidedefs) of the side
    /// opening into the lower sector (the lift's face, wearing the side
    /// texture below).
    low_side: usize,
    /// Whether that side is currently the back, so the line turns
    /// around with it.
    flip: bool,
}

/// The platform `build_lift` matches lines against.
#[derive(Clone, Copy, Debug)]
struct EdgeProbe {
    /// The platform sector's index.
    platform: usize,
    /// The platform's floor height: only edges facing below it activate.
    floor: i32,
}

/// The `InvalidArgument` with an actionable `message`.
fn invalid(message: &str) -> CyberError {
    CyberError::InvalidArgument(message.to_owned())
}

/// `InvalidArgument` naming the reference `kind`, its out-of-range
/// `index` and how many the map holds.
fn range_error(kind: &str, index: usize, len: usize) -> CyberError {
    invalid(&format!(
        "{kind} {index} is out of range: the map holds {len} of them; re-read the map \
         and pass current indices"
    ))
}

/// The refusal for a platform with nothing beside it to lower towards.
fn isolated(sector: usize) -> CyberError {
    invalid(&format!(
        "sector {sector} has no neighbouring sector to take a low floor from: pass an \
         explicit low_floor or draw the platform beside a lower area"
    ))
}

/// The refusal for a platform that is not the high floor of its drop.
fn no_way_down(sector: usize, floor: i32, low_floor: i32) -> CyberError {
    invalid(&format!(
        "sector {sector} stands at floor {floor}, not above its lift floor {low_floor}: \
         the platform is the high floor that comes down; pick the higher sector or pass \
         a lower low_floor"
    ))
}

/// The refusal for a platform with no wall the lower area can trigger.
fn no_edges(sector: usize) -> CyberError {
    invalid(&format!(
        "no edge of sector {sector} faces a lower sector: the activation lines are the \
         platform's walls a player reaches from below; draw the polygon so its centre \
         stands on the high side of the height change, or pick the sector above its \
         neighbours"
    ))
}

/// The refusal for a platform polygon whose centre is in the void.
fn centre_lost(centre: Pt) -> CyberError {
    invalid(&format!(
        "the platform's centre {}, {} lies in no sector: draw the polygon across the \
         map so its centre sits on the floor the platform starts at",
        centre.0, centre.1
    ))
}

/// The small-drop note: legal, but a lift wasted on it.
fn shrink_note(drop: i32) -> String {
    format!(
        "this lift would drop only {drop} map units: a lift is not needed; stairs or a \
         step will do (below {MIN_LIFT_DROP} the guide says prefer add_stairs)"
    )
}

/// The server-bug refusal for a platform paint that reported no sector.
fn paint_lost() -> CyberError {
    CyberError::Internal("the lift platform paint reported no sector (server bug)".to_owned())
}

#[cfg(test)]
mod tests;
