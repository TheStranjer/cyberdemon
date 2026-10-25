//! Resource checks over a decoded [`Map`] and the [`super::ResourceIndex`] the
//! tool layer read from the merged view: wall textures and flats the
//! session does not define, and the classic hall-of-mirrors gaps -
//! faces the neighbouring sectors' heights demand but nobody dressed.
//!
//! An empty name is "unset" (the binary readers store a blank field as
//! `"-"`, hand-built `UDMF` rows may leave it empty). On a sidedef face
//! unset is legal, so the texture side speaks only about named-but-undefined
//! faces and about faces explicitly `"-"` where the geometry demands one.
//! On a sector plane it is not: every floor and ceiling wears a flat, so
//! [`missing_flat`] reports an undressed plane whatever the session carries.
//! The name lookups run only when the merged view defines that resource at
//! all - without any `TEXTURE1`/`TEXTURE2`/`TX_` source there is no standard
//! to check texture names against, and [`texture_check_skipped`] says so
//! instead; the flat side likewise judges no names when the session carries
//! no flats.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::{Context, Issue, Severity};
use crate::wad::map::model::{Map, Sector, Sidedef};

/// The model's explicit "no texture here" name.
const NO_TEXTURE: &str = "-";
/// The sky flat, always allowed even when no lump defines it.
const SKY_FLAT: &str = "F_SKY1";

/// One resolved side of a two-sided line: sidedef index, its row and
/// the sector it opens into.
type Side<'rows> = (usize, &'rows Sidedef, &'rows Sector);

/// Whether a face is left bare: the model's explicit no-texture marker.
fn bare(name: &str) -> bool {
    name == NO_TEXTURE
}

/// Whether a name needs a lookup: neither unset (the empty string, as
/// hand-built rows leave faces) nor the `"-"` marker.
fn checkable(name: &str) -> bool {
    !name.is_empty() && !bare(name)
}

/// Whether `name` is in `set` case-insensitively (names are stored
/// uppercase, a hand-built row might not be).
fn known(set: &BTreeSet<String>, name: &str) -> bool {
    set.contains(&name.to_ascii_uppercase())
}

/// The one info row of a session whose merged view defines no composite
/// textures at all: the [`missing_texture`] check has nothing to check
/// against.
pub(super) fn texture_check_skipped(context: &Context<'_>) -> Vec<Issue> {
    if context.resources.has_texture_defs {
        return Vec::new();
    }
    vec![Issue::new(
        Severity::Info,
        "wall textures were not checked: the merged view defines no composite textures (no \
         TEXTURE1/TEXTURE2 lump and no textures namespace); load the archive that carries \
         them, or define the textures, and validate again"
            .to_owned(),
    )]
}

/// One undefined-face row for sidedef `index`.
fn texture_issue(index: usize, role: &str, name: &str) -> Issue {
    Issue::new(
        Severity::Error,
        format!(
            "sidedef {index} names {role} texture \"{name}\", which the merged view does \
             not define (TEXTURE1/TEXTURE2 and the textures region): define it or \
             edit_sidedef the face to a known texture",
        ),
    )
    .sidedef(&[index])
}

/// The undefined faces named by one sidedef.
fn side_texture_gaps(index: usize, side: &Sidedef, textures: &BTreeSet<String>) -> Vec<Issue> {
    let mut gaps: Vec<Issue> = Vec::new();
    for (role, name) in [
        ("upper", side.upper.as_str()),
        ("lower", side.lower.as_str()),
        ("middle", side.middle.as_str()),
    ] {
        if checkable(name) && !known(textures, name) {
            gaps.push(texture_issue(index, role, name));
        }
    }
    gaps
}

/// Sidedef faces naming a texture the merged view does not define.
pub(super) fn missing_texture(map: &Map, context: &Context<'_>) -> Vec<Issue> {
    if !context.resources.has_texture_defs {
        return Vec::new();
    }
    map.sidedefs
        .iter()
        .enumerate()
        .flat_map(|(index, side)| side_texture_gaps(index, side, &context.resources.textures))
        .collect()
}

/// One undefined-flat row for sector `index`.
fn flat_issue(index: usize, role: &str, flat: &str) -> Issue {
    Issue::new(
        Severity::Error,
        format!(
            "sector {index} names {role} flat \"{flat}\", which the merged view does not \
             carry as a flat lump: load the archive that holds it or edit_sector the sector \
             to a real flat",
        ),
    )
    .sector(&[index])
}

/// One row for sector `index`'s plane that wears no flat at all.
fn bare_plane_issue(index: usize, role: &str, flat: &str) -> Issue {
    Issue::new(
        Severity::Error,
        format!(
            "sector {index} has no {role} flat: \"{flat}\" is the unset-field placeholder, never \
             a flat name, so the engine draws that plane missing or broken - edit_sector the \
             sector's {role}_flat to a real flat (load the archive that carries the flats first \
             when the merged view holds none)",
        ),
    )
    .sector(&[index])
}

/// Whether a flat name leaves its plane undressed: the `"-"` marker (what
/// the binary readers store for a blank field) or the empty name a
/// hand-built row carries. Unlike a sidedef face, a sector plane always
/// needs a flat, so neither is ever valid.
fn bare_plane(flat: &str) -> bool {
    !checkable(flat)
}

/// The row one plane of sector `index` earns, or `None` when it is sound.
/// An undressed plane always reports; a named flat reports only when
/// `flats` does not carry it, `None` standing for a session that carries no
/// flats and so judges no names at all. `F_SKY1` never reports: every port
/// draws the sky itself.
fn plane_gap(
    index: usize,
    role: &str,
    flat: &str,
    flats: Option<&BTreeSet<String>>,
) -> Option<Issue> {
    if bare_plane(flat) {
        return Some(bare_plane_issue(index, role, flat));
    }
    let carried = flats?;
    let sound = flat.eq_ignore_ascii_case(SKY_FLAT) || known(carried, flat);
    (!sound).then(|| flat_issue(index, role, flat))
}

/// The rows both planes of sector `index` earn, in floor then ceiling
/// order.
fn sector_flat_gaps(index: usize, space: &Sector, flats: Option<&BTreeSet<String>>) -> Vec<Issue> {
    [
        ("floor", space.floor_flat.as_str()),
        ("ceiling", space.ceiling_flat.as_str()),
    ]
    .into_iter()
    .filter_map(|(role, flat)| plane_gap(index, role, flat, flats))
    .collect()
}

/// Sector planes left with no flat, and sector flats naming a flat the
/// merged view does not carry. `F_SKY1` is always allowed on either plane -
/// the engine substitutes the sky. A carried flat is the standard a name is
/// measured against, so the name half stays quiet when the session carries
/// none; an undressed plane needs no standard and reports whatever the
/// session holds.
pub(super) fn missing_flat(map: &Map, context: &Context<'_>) -> Vec<Issue> {
    let flats = (!context.resources.flats.is_empty()).then_some(&context.resources.flats);
    map.sectors
        .iter()
        .enumerate()
        .flat_map(|(index, space)| sector_flat_gaps(index, space, flats))
        .collect()
}

/// Whether both sectors' ceilings are the sky: the band above a line
/// between two sky-holding sectors draws the sky on both sides, so an
/// upper texture there is nobody's missing face.
fn both_sky(front: &Sector, back: &Sector) -> bool {
    front.ceiling_flat.eq_ignore_ascii_case(SKY_FLAT)
        && back.ceiling_flat.eq_ignore_ascii_case(SKY_FLAT)
}

/// One hall-of-mirrors row for a bare required face, `why` naming the
/// height gap that makes the face necessary.
fn hom_issue(line: usize, side: usize, role: &str, why: &str) -> Issue {
    Issue::new(
        Severity::Warning,
        format!(
            "linedef {line} sidedef {side}: {why} but the {role} texture is \"{NO_TEXTURE}\"; \
             that face is a hall of mirrors - dress it (edit_sidedef) or close the height gap",
        ),
    )
    .linedef(&[line])
    .sidedef(&[side])
}

/// The bare middle of a one-sided line leaves the void itself on screen.
fn one_sided_gap(line: usize, side: usize, row: &Sidedef) -> Option<Issue> {
    if !bare(&row.middle) {
        return None;
    }
    Some(hom_issue(
        line,
        side,
        "middle",
        "the line is one-sided, so its middle face is all that hides the void",
    ))
}

/// The bands `this` side must dress against the sector across: an upper
/// where the other ceiling is lower (unless both ceilings are sky) and a
/// lower where the other floor is higher.
fn this_side_gaps(issues: &mut Vec<Issue>, line: usize, this: Side<'_>, across: Side<'_>) {
    let (side_index, row, sector) = this;
    let (across_index, _, other) = across;
    if other.ceiling_height < sector.ceiling_height && !both_sky(sector, other) && bare(&row.upper)
    {
        issues.push(hom_issue(
            line,
            side_index,
            "upper",
            &format!(
                "the sector across (sidedef {across_index}) has ceiling {} below this side's {}",
                other.ceiling_height, sector.ceiling_height,
            ),
        ));
    }
    if other.floor_height > sector.floor_height && bare(&row.lower) {
        issues.push(hom_issue(
            line,
            side_index,
            "lower",
            &format!(
                "the sector across (sidedef {across_index}) has floor {} above this side's {}",
                other.floor_height, sector.floor_height,
            ),
        ));
    }
}

/// The resolved rows and sectors of both sides of a two-sided line,
/// `None` when either sector reference dangles.
fn dressed_sides(map: &Map, front: usize, back: usize) -> Option<(Side<'_>, Side<'_>)> {
    let front_row = map.sidedefs.get(front)?;
    let back_row = map.sidedefs.get(back)?;
    let front_sector = map.sectors.get(front_row.sector)?;
    let back_sector = map.sectors.get(back_row.sector)?;
    Some((
        (front, front_row, front_sector),
        (back, back_row, back_sector),
    ))
}

/// The gaps of one linedef: its bare middle when it is one-sided, the
/// required upper/lower faces of both sides when it is two-sided.
fn line_gaps(map: &Map, line: usize) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();
    let Some(edge) = map.linedefs.get(line) else {
        return issues;
    };
    let Some(front_index) = edge.front else {
        return issues;
    };
    let Some(front_row) = map.sidedefs.get(front_index) else {
        return issues;
    };
    let Some(back_index) = edge.back else {
        issues.extend(one_sided_gap(line, front_index, front_row));
        return issues;
    };
    if let Some((front, back)) = dressed_sides(map, front_index, back_index) {
        this_side_gaps(&mut issues, line, front, back);
        this_side_gaps(&mut issues, line, back, front);
    }
    issues
}

/// Sidedefs whose geometry demands a face that was left `"-"`: the
/// middle of a one-sided line, and the upper/lower faces of a two-sided
/// line where the neighbour's heights make the band visible (sky on
/// both ceilings exempts the upper faces).
pub(super) fn missing_required_texture(map: &Map) -> Vec<Issue> {
    (0..map.linedefs.len())
        .flat_map(|line| line_gaps(map, line))
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use alloc::string::{String, ToString};

    use super::{
        Issue, missing_flat, missing_required_texture, missing_texture, texture_check_skipped,
    };
    use crate::defs::definitions_for;
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::common::fixtures::{name, room, side, solid_line, vertex};
    use crate::wad::map::validate::{Context, ResourceIndex, Severity};
    use crate::wad::map::{Linedef, Map, Sidedef};

    /// A set of the given names.
    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(ToString::to_string).collect()
    }

    /// The tables and resource index one test runs against.
    struct Fixture {
        /// The DOOM II rows.
        doom: crate::defs::Definitions,
        /// The `Hexen` rows.
        hexen: crate::defs::Definitions,
        /// The resource index under test.
        resources: ResourceIndex,
    }

    /// A fixture over `resources` with the embedded definition tables.
    fn fixture(resources: ResourceIndex) -> Fixture {
        Fixture {
            doom: definitions_for(Game::Doom2).expect("the embedded Doom II table is valid"),
            hexen: definitions_for(Game::Hexen).expect("the embedded Hexen table is valid"),
            resources,
        }
    }

    /// The validation context borrowing a fixture's three tables.
    fn context_of(fixture: &Fixture) -> Context<'_> {
        Context {
            definitions: &fixture.doom,
            hexen_specials: &fixture.hexen,
            resources: &fixture.resources,
        }
    }

    /// A resource index with `WALL1` defined and `FLOOR1` carried.
    fn full_index() -> ResourceIndex {
        ResourceIndex {
            textures: set(&["WALL1"]),
            flats: set(&["FLOOR1"]),
            has_texture_defs: true,
        }
    }

    /// The one sector of [`ring_map`] re-flattened onto `floor` and `ceiling`.
    fn reflatted(mut map: Map, floor: &str, ceiling: &str) -> Map {
        let space = map.sectors.get_mut(0).expect("the only sector");
        space.floor_flat = floor.to_owned();
        space.ceiling_flat = ceiling.to_owned();
        map
    }

    /// The single issue of a one-row finding, panicking when the check said
    /// more or less than that.
    #[track_caller]
    fn only_issue(issues: Vec<Issue>) -> Issue {
        assert_eq!(issues.len(), 1, "exactly one issue expected");
        issues.into_iter().next().expect("the one issue")
    }

    /// The plane a bare-plane row names: its message says "sector N has no
    /// {role} flat".
    #[track_caller]
    fn plane_role(issue: &Issue) -> &str {
        let after = issue.message.split(" has no ").nth(1).expect("the role");
        after.split(' ').next().expect("the role word")
    }

    /// A `Doom` map: a closed square ring whose four walls all front
    /// sidedef 0 (`WALL1` middle) on sector 0.
    fn ring_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![
            vertex(0, 0),
            vertex(128, 0),
            vertex(128, 128),
            vertex(0, 128),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 0),
            solid_line(2, 3, 0),
            solid_line(3, 0, 0),
        ];
        map.sidedefs = vec![side("-", "-", "WALL1", 0)];
        map.sectors = vec![room(0, 128, 160, 0)];
        map
    }

    /// A single two-sided step line: front sidedef 0 on a floor-0
    /// sector, back sidedef 1 on a floor-64 sector, ceilings both 128,
    /// every face `"-"`.
    fn step_map() -> Map {
        let mut map = Map::new_empty(name("MAP01"), MapFormat::Doom);
        map.vertices = vec![vertex(0, 0), vertex(128, 0)];
        let mut line = solid_line(0, 1, 0);
        line.flags = 4;
        line.back = Some(1);
        map.linedefs = vec![line];
        map.sidedefs = vec![side("-", "-", "-", 0), side("-", "-", "-", 1)];
        map.sectors = vec![room(0, 128, 160, 0), room(64, 128, 160, 0)];
        map
    }

    #[test]
    fn texture_check_skipped_speaks_only_without_texture_definitions() {
        let given = fixture(full_index());
        assert!(
            texture_check_skipped(&context_of(&given)).is_empty(),
            "with definitions, nothing to say"
        );
        let nothing = fixture(ResourceIndex::default());
        let note = texture_check_skipped(&context_of(&nothing));
        let issue = note.first().expect("no texture defs, one note");
        assert_eq!(issue.severity, Severity::Info, "a skip is mild");
        assert!(
            issue.message.contains("defines no composite textures"),
            "the message explains: {}",
            issue.message
        );
    }

    #[test]
    fn missing_texture_names_undefined_sidedef_faces_case_insensitively() {
        let given = fixture(full_index());
        let context = context_of(&given);
        assert!(
            missing_texture(&ring_map(), &context).is_empty(),
            "WALL1 is known"
        );
        let mut odd = ring_map();
        odd.sidedefs.get_mut(0).expect("the shared sidedef").lower = "brick1".to_owned();
        let issues = missing_texture(&odd, &context);
        let issue = issues.first().expect("brick1 is not defined");
        assert_eq!(
            issue.severity,
            Severity::Error,
            "an undefined texture is broken"
        );
        assert_eq!(issue.sidedefs, vec![0_usize], "the sidedef named");
        assert!(
            issue.message.contains("lower texture \"brick1\""),
            "the message quotes the role and name: {}",
            issue.message
        );
    }

    #[test]
    fn missing_texture_skips_bare_and_unset_faces_and_stays_quiet_without_defs() {
        let given = fixture(full_index());
        let context = context_of(&given);
        let mut blanks = ring_map();
        blanks
            .sidedefs
            .get_mut(0)
            .expect("the shared sidedef")
            .middle = "-".to_owned();
        blanks.sidedefs.push(Sidedef {
            sector: 0,
            ..Default::default()
        });
        assert!(
            missing_texture(&blanks, &context).is_empty(),
            "\"-\" and empty names ask for no lookup"
        );
        let mut named = ring_map();
        named
            .sidedefs
            .get_mut(0)
            .expect("the shared sidedef")
            .middle = "DOOM3".to_owned();
        let without_defs = fixture(ResourceIndex::default());
        assert!(
            missing_texture(&named, &context_of(&without_defs)).is_empty(),
            "without texture definitions the check does not run"
        );
    }

    #[test]
    fn missing_flat_requires_a_known_flat_and_reports_an_unknown_one() {
        let given = fixture(full_index());
        let context = context_of(&given);
        assert!(
            missing_flat(&ring_map(), &context).is_empty(),
            "FLOOR1 is carried"
        );
        let issue = only_issue(missing_flat(
            &reflatted(ring_map(), "LAVA1", "FLOOR1"),
            &context,
        ));
        assert_eq!(
            issue.severity,
            Severity::Error,
            "an undefined flat is broken"
        );
        assert_eq!(issue.sectors, vec![0_usize], "the sector named");
        assert!(
            issue.message.contains("floor flat \"LAVA1\"") && issue.message.contains("edit_sector"),
            "the message quotes the role and name and names the fix: {}",
            issue.message
        );
    }

    #[test]
    fn the_sky_flat_is_always_allowed() {
        let given = fixture(full_index());
        let context = context_of(&given);
        assert!(
            missing_flat(&reflatted(ring_map(), "FLOOR1", "F_SKY1"), &context).is_empty(),
            "the engine draws the sky itself"
        );
        assert!(
            missing_flat(&reflatted(ring_map(), "F_SKY1", "f_sky1"), &context).is_empty(),
            "on either plane, and a hand-built row's lower case too"
        );
    }

    #[test]
    fn a_dash_flat_reports_the_undressed_plane() {
        let given = fixture(full_index());
        let context = context_of(&given);
        let floor = only_issue(missing_flat(
            &reflatted(ring_map(), "-", "FLOOR1"),
            &context,
        ));
        assert_eq!(floor.severity, Severity::Error, "a bare plane is broken");
        assert_eq!(floor.sectors, vec![0_usize], "the sector named");
        assert_eq!(plane_role(&floor), "floor", "its floor plane");
        assert!(
            floor.message.contains("floor_flat") && floor.message.contains("edit_sector"),
            "the message names the field and the fixing tool: {}",
            floor.message
        );
        let ceiling = only_issue(missing_flat(
            &reflatted(ring_map(), "FLOOR1", "-"),
            &context,
        ));
        assert_eq!(plane_role(&ceiling), "ceiling", "the other plane by itself");
        assert!(ceiling.message.contains("ceiling_flat"), "and its field");
        let both = missing_flat(&reflatted(ring_map(), "-", "-"), &context);
        assert_eq!(both.len(), 2, "both planes report: {both:?}");
    }

    #[test]
    fn an_empty_flat_reports_the_undressed_plane_too() {
        let given = fixture(full_index());
        let context = context_of(&given);
        let issues = missing_flat(&reflatted(ring_map(), "", ""), &context);
        assert_eq!(issues.len(), 2, "both unset planes report: {issues:?}");
        assert_eq!(
            issues.iter().map(plane_role).collect::<Vec<&str>>(),
            vec!["floor", "ceiling"],
            "each plane named, in order"
        );
        for issue in &issues {
            assert_eq!(issue.severity, Severity::Error, "broken either way");
            assert_eq!(issue.sectors, vec![0_usize], "the sector named");
        }
    }

    #[test]
    fn missing_flat_judges_names_only_when_the_session_carries_flats() {
        let given = fixture(ResourceIndex {
            textures: BTreeSet::new(),
            flats: BTreeSet::new(),
            has_texture_defs: true,
        });
        let context = context_of(&given);
        assert!(
            missing_flat(&reflatted(ring_map(), "EVERY", "F_SKY1"), &context).is_empty(),
            "no flats carried: no standard to check a name against"
        );
        let issue = only_issue(missing_flat(
            &reflatted(ring_map(), "-", "F_SKY1"),
            &context,
        ));
        assert_eq!(
            plane_role(&issue),
            "floor",
            "an undressed plane needs no standard to be broken"
        );
    }

    #[test]
    fn a_bare_middle_on_a_one_sided_line_is_a_hall_of_mirrors() {
        assert!(
            missing_required_texture(&ring_map()).is_empty(),
            "the ring's walls carry WALL1 middles"
        );
        let mut naked = ring_map();
        naked
            .sidedefs
            .get_mut(0)
            .expect("the shared sidedef")
            .middle = "-".to_owned();
        let issues = missing_required_texture(&naked);
        assert_eq!(issues.len(), 4, "every one of the four walls is bare");
        let issue = issues.first().expect("the first wall");
        assert_eq!(
            issue.severity,
            Severity::Warning,
            "HOM is doubtful, not broken"
        );
        assert_eq!(issue.linedefs, vec![0_usize], "the line named");
        assert_eq!(issue.sidedefs, vec![0_usize], "and the bare side");
        assert!(
            issue.message.contains("one-sided") && issue.message.contains("middle"),
            "the message explains the case: {}",
            issue.message
        );
    }

    #[test]
    fn the_step_reports_one_lower_and_dressing_it_closes_the_gap() {
        let issues = missing_required_texture(&step_map());
        assert_eq!(
            issues.len(),
            1,
            "only the low side needs a lower here: {issues:?}"
        );
        let issue = issues.first().expect("the missing lower");
        assert_eq!(issue.sidedefs, vec![0_usize], "sidedef 0 is the low side");
        assert_eq!(issue.linedefs, vec![0_usize], "the only linedef");
        assert!(
            issue.message.contains("floor 64 above this side's 0"),
            "the message quotes the step: {}",
            issue.message
        );
        let mut dressed = step_map();
        dressed.sidedefs.get_mut(0).expect("the front side").lower = "STEP1".to_owned();
        assert!(
            missing_required_texture(&dressed).is_empty(),
            "dressing the needed lower face closes the gap"
        );
    }

    #[test]
    fn a_missing_upper_on_the_taller_side_is_reported_too() {
        let mut stepped = step_map();
        stepped
            .sectors
            .get_mut(1)
            .expect("the back sector")
            .ceiling_height = 64_i32;
        stepped.sidedefs.get_mut(0).expect("the front side").lower = "STEP1".to_owned();
        let issues = missing_required_texture(&stepped);
        let issue = issues.first().expect("the bare upper band");
        assert_eq!(issues.len(), 1, "exactly the upper gap: {issues:?}");
        assert!(
            issue.message.contains("ceiling 64 below this side's 128"),
            "the message quotes the band: {}",
            issue.message
        );
    }

    #[test]
    fn sky_on_both_ceilings_exempts_the_upper_but_never_the_lower() {
        let mut sky_hack = step_map();
        sky_hack
            .sectors
            .get_mut(0)
            .expect("the front sector")
            .ceiling_height = 256_i32;
        for index in [0_usize, 1_usize] {
            sky_hack
                .sectors
                .get_mut(index)
                .expect("both sectors")
                .ceiling_flat = "F_SKY1".to_owned();
        }
        sky_hack.sidedefs.get_mut(0).expect("the front side").lower = "STEP1".to_owned();
        assert!(
            missing_required_texture(&sky_hack).is_empty(),
            "front ceiling 256 over back ceiling 128 needs an upper, but both sectors are sky"
        );
        let mut still_open = sky_hack;
        still_open
            .sidedefs
            .get_mut(0)
            .expect("the front side")
            .lower = "-".to_owned();
        let issues = missing_required_texture(&still_open);
        assert_eq!(
            issues.len(),
            1,
            "the lower gap reports despite the sky hack: {issues:?}"
        );
        assert!(
            issues
                .first()
                .expect("the lower row")
                .message
                .contains("lower"),
            "the exemption is upper-only"
        );
    }

    #[test]
    fn dangling_sides_and_sectors_never_report_hom() {
        let mut map = step_map();
        map.linedefs.get_mut(0).expect("the step line").back = Some(42);
        assert!(
            missing_required_texture(&map).is_empty(),
            "a dangling back side is the reference check's problem"
        );
        let mut sectors_gone = step_map();
        sectors_gone
            .sidedefs
            .get_mut(1)
            .expect("the back side")
            .sector = 42;
        assert!(
            missing_required_texture(&sectors_gone).is_empty(),
            "a dangling sector takes no measurement part"
        );
        let mut frontless = step_map();
        frontless.linedefs.get_mut(0).expect("the step line").front = None;
        assert!(
            missing_required_texture(&frontless).is_empty(),
            "a line without a front side is the reference check's problem"
        );
        let mut blank = step_map();
        blank.linedefs.push(Linedef {
            v1: 0,
            v2: 1,
            ..Default::default()
        });
        assert!(
            missing_required_texture(&blank)
                .iter()
                .all(|issue| issue.linedefs != vec![1]),
            "a line with no sides at all reports no face: {blank:?}"
        );
    }

    #[test]
    fn a_two_sided_line_with_equal_heights_needs_no_faces() {
        let mut flat_line = step_map();
        flat_line
            .sectors
            .get_mut(1)
            .expect("the back sector")
            .floor_height = 0_i32;
        assert!(
            missing_required_texture(&flat_line).is_empty(),
            "two sectors at equal heights share no visible band"
        );
    }
}
