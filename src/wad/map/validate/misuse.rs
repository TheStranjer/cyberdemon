//! The picture-kind rule: a name worn in the role the merged view does
//! not hold it in.
//!
//! A sidedef face may only wear a name the merged view defines as a
//! composite wall texture, and a sector plane may only wear a name it
//! carries as a flat lump. A name that belongs to the other set alone is
//! more than undefined: it is the wrong *kind* of picture, and the row
//! says so and how to fix it. [`misused_resource`] is `validate_map`'s
//! check; the editing tools' result warnings ask the same two predicates
//! and speak the same [`Misuse::explain`] sentence, so a warning at
//! assignment time and an error row later never disagree.
//!
//! The rule is membership, nothing else. A name the view carries as both
//! kinds (a flat lump and a texture record) is sound on a face and on a
//! plane, and a name it holds as neither is the ordinary undefined case
//! of the `missing_texture` and `missing_flat` checks, not a misuse -
//! and those two checks hold back on exactly the names reported here, so
//! one face or plane reports once. Unset faces (the empty name and the
//! `"-"` marker) are no names and never report here.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::{Context, Issue, ResourceIndex, Severity};
use crate::wad::map::consts::normalized_picture;
use crate::wad::map::model::{Map, Sector, Sidedef};

/// A picture name worn in the role the merged view does not hold it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Misuse {
    /// A sidedef face wears a name the merged view carries only as a flat.
    FlatOnWall,
    /// A sector plane wears a name the merged view defines only as a wall
    /// texture.
    TextureOnPlane,
}

impl Misuse {
    /// Whether `name`, worn on a sidedef face, is a flat and no wall
    /// texture: the sets are the merged view's, read through the shared
    /// picture-name normalisation, and an unset face is no name.
    #[must_use]
    #[inline]
    pub fn flat_only(name: &str, flats: &BTreeSet<String>, walls: &BTreeSet<String>) -> bool {
        normalized_picture(name).is_some_and(|key| flats.contains(&key) && !walls.contains(&key))
    }

    /// Whether `name`, worn on a sector plane, is a wall texture and no
    /// flat: the mirror arm of [`Self::flat_only`]. The sky markers are
    /// never composite-texture names, so the sky exemption of the flat
    /// check needs no restating here.
    #[must_use]
    #[inline]
    pub fn texture_only(name: &str, flats: &BTreeSet<String>, walls: &BTreeSet<String>) -> bool {
        normalized_picture(name).is_some_and(|key| walls.contains(&key) && !flats.contains(&key))
    }

    /// The one sentence the `misused_resource` rows and the editing
    /// tools' warnings share: `who` names the element or elements
    /// ("sidedef 3"), `role` the face ("middle", "floor") and `name` the
    /// picture worn there, quoted as the caller stores it. It ends with
    /// the tool and the alternative that fix it.
    #[must_use]
    #[inline]
    pub fn explain(self, who: &str, role: &str, name: &str) -> String {
        match self {
            Self::FlatOnWall => format!(
                "the {role} texture \"{name}\" on {who} is carried only as a flat: flats \
                 dress sector planes, not walls - edit_sidedef the face to a defined wall \
                 texture (list_textures shows them) or build one from the flat \
                 (create_texture_from_image)",
            ),
            Self::TextureOnPlane => format!(
                "the {role} flat \"{name}\" on {who} is defined only as a wall texture: \
                 composite textures dress walls, not planes - edit_sector the plane to a \
                 carried flat (list_flats shows them) or import one (create_flat_from_image)",
            ),
        }
    }
}

/// One error row for sidedef `index` wearing the flat `name` as its
/// `role` face texture.
fn face_issue(index: usize, role: &str, name: &str) -> Issue {
    Issue::new(
        Severity::Error,
        Misuse::FlatOnWall.explain(&format!("sidedef {index}"), role, name),
    )
    .sidedef(&[index])
}

/// One error row for sector `index` wearing the wall texture `name` as
/// its `role` plane.
fn plane_issue(index: usize, role: &str, name: &str) -> Issue {
    Issue::new(
        Severity::Error,
        Misuse::TextureOnPlane.explain(&format!("sector {index}"), role, name),
    )
    .sector(&[index])
}

/// The misuse rows of one sidedef's three faces, in the upper/lower/
/// middle order the texture check walks.
fn side_misuses(index: usize, side: &Sidedef, resources: &ResourceIndex) -> Vec<Issue> {
    let mut rows: Vec<Issue> = Vec::new();
    for (role, name) in [
        ("upper", side.upper.as_str()),
        ("lower", side.lower.as_str()),
        ("middle", side.middle.as_str()),
    ] {
        if Misuse::flat_only(name, &resources.flats, &resources.textures) {
            rows.push(face_issue(index, role, name));
        }
    }
    rows
}

/// The misuse rows of one sector's two planes, floor then ceiling.
fn plane_misuses(index: usize, space: &Sector, resources: &ResourceIndex) -> Vec<Issue> {
    let mut rows: Vec<Issue> = Vec::new();
    for (role, name) in [
        ("floor", space.floor_flat.as_str()),
        ("ceiling", space.ceiling_flat.as_str()),
    ] {
        if Misuse::texture_only(name, &resources.flats, &resources.textures) {
            rows.push(plane_issue(index, role, name));
        }
    }
    rows
}

/// Sidedef faces wearing a name the merged view carries only as a flat,
/// and sector planes wearing a name it defines only as a wall texture -
/// each an error row naming the element, the face, the picture and the
/// fix. The `missing_texture` and `missing_flat` checks stay quiet on
/// exactly these names, so a misused picture reports once; a name neither
/// set holds keeps its ordinary undefined row.
pub(super) fn misused_resource(map: &Map, context: &Context<'_>) -> Vec<Issue> {
    let resources = context.resources;
    let mut issues: Vec<Issue> = map
        .sidedefs
        .iter()
        .enumerate()
        .flat_map(|(index, side)| side_misuses(index, side, resources))
        .collect();
    issues.extend(
        map.sectors
            .iter()
            .enumerate()
            .flat_map(|(index, space)| plane_misuses(index, space, resources)),
    );
    issues
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeSet;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    use super::{Misuse, misused_resource};
    use crate::defs::definitions_for;
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::common::fixtures::{name, room, side, solid_line, vertex};
    use crate::wad::map::consts::SKY_FLAT;
    use crate::wad::map::validate::resources::{missing_flat, missing_texture};
    use crate::wad::map::validate::{Context, Issue, ResourceIndex, Severity};
    use crate::wad::map::{Map, Sector, Sidedef};

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

    /// The validation context borrowing a fixture's tables.
    fn context_of(fixture: &Fixture) -> Context<'_> {
        Context {
            definitions: &fixture.doom,
            hexen_specials: &fixture.hexen,
            resources: &fixture.resources,
            sky_flat: SKY_FLAT,
        }
    }

    /// A resource index with `WALL1` defined, `FLOOR1` carried and
    /// `BOTH` held as both kinds.
    fn full_index() -> ResourceIndex {
        ResourceIndex {
            textures: set(&["WALL1", "BOTH"]),
            flats: set(&["FLOOR1", "BOTH"]),
            has_texture_defs: true,
        }
    }

    /// A `Doom` map: a closed square ring whose four walls all front
    /// sidedef 0 with the given `middle` on one sector with the given
    /// floor and ceiling flats.
    fn ring(middle: &str, floor: &str, ceiling: &str) -> Map {
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
        map.sidedefs = vec![side("-", "-", middle, 0)];
        map.sectors = vec![Sector {
            floor_flat: floor.to_owned(),
            ceiling_flat: ceiling.to_owned(),
            ..room(0, 128, 160, 0)
        }];
        map
    }

    /// The one row of a one-row finding, panicking when the check said
    /// more or less than that.
    #[track_caller]
    fn only_issue(issues: Vec<Issue>) -> Issue {
        assert_eq!(issues.len(), 1, "exactly one issue expected");
        issues.into_iter().next().expect("the one issue")
    }

    #[test]
    fn the_predicates_read_membership_only() {
        let flats = set(&["FLOOR1", "BOTH"]);
        let walls = set(&["WALL1", "BOTH"]);
        assert!(
            Misuse::flat_only("FLOOR1", &flats, &walls),
            "a flat-only name is no wall texture"
        );
        assert!(
            Misuse::flat_only(" floor1 ", &flats, &walls),
            "the key is the shared normalisation"
        );
        assert!(
            !Misuse::flat_only("WALL1", &flats, &walls),
            "a defined texture is sound"
        );
        assert!(
            !Misuse::flat_only("BOTH", &flats, &walls),
            "a name of both kinds is sound"
        );
        assert!(
            !Misuse::flat_only("NOPE", &flats, &walls),
            "a name of neither kind is the undefined case"
        );
        assert!(
            !Misuse::flat_only("-", &flats, &walls),
            "the marker is no name"
        );
        assert!(
            !Misuse::flat_only("", &flats, &walls),
            "neither is the empty face"
        );
        assert!(
            Misuse::texture_only("WALL1", &flats, &walls),
            "and the plane arm reads the mirror"
        );
        assert!(
            !Misuse::texture_only("FLOOR1", &flats, &walls),
            "a carried flat dresses its plane"
        );
    }

    #[test]
    fn explain_names_the_role_the_picture_the_element_and_the_fix() {
        let wall = Misuse::FlatOnWall.explain("sidedef 3", "middle", "FLOOR1");
        assert!(
            wall.contains("middle texture \"FLOOR1\" on sidedef 3")
                && wall.contains("carried only as a flat")
                && wall.contains("edit_sidedef")
                && wall.contains("create_texture_from_image"),
            "the face sentence: {wall}"
        );
        let plane = Misuse::TextureOnPlane.explain("sectors 0, 2", "floor", "WALL1");
        assert!(
            plane.contains("floor flat \"WALL1\" on sectors 0, 2")
                && plane.contains("defined only as a wall texture")
                && plane.contains("edit_sector")
                && plane.contains("create_flat_from_image"),
            "the plane sentence: {plane}"
        );
    }

    #[test]
    fn a_flat_worn_on_a_face_and_a_texture_worn_on_a_plane_both_report() {
        let given = fixture(full_index());
        let context = context_of(&given);
        let face_row = only_issue(misused_resource(
            &ring("FLOOR1", "FLOOR1", "F_SKY1"),
            &context,
        ));
        assert_eq!(
            face_row.severity,
            Severity::Error,
            "a misused picture is broken"
        );
        assert_eq!(
            face_row.sidedefs,
            vec![0_usize],
            "the face names its sidedef"
        );
        assert!(
            face_row.message.contains("middle texture \"FLOOR1\""),
            "and quotes the role and name: {}",
            face_row.message
        );
        let plane_row = only_issue(misused_resource(
            &ring("WALL1", "WALL1", "FLOOR1"),
            &context,
        ));
        assert_eq!(
            plane_row.sectors,
            vec![0_usize],
            "the plane names its sector"
        );
        assert!(
            plane_row.message.contains("floor flat \"WALL1\""),
            "with its role and name: {}",
            plane_row.message
        );
    }

    #[test]
    fn sound_and_undefined_names_stay_out_of_the_misuse_rows() {
        let given = fixture(full_index());
        let context = context_of(&given);
        assert!(
            misused_resource(&ring("BOTH", "BOTH", "BOTH"), &context).is_empty(),
            "a name of both kinds dresses either role"
        );
        assert!(
            misused_resource(&ring("WALL1", "FLOOR1", "F_SKY1"), &context).is_empty(),
            "right kind on both sides says nothing"
        );
        assert!(
            misused_resource(&ring("NOPE", "LAVA1", "F_SKY1"), &context).is_empty(),
            "names of neither kind are the undefined checks' business"
        );
        let mut blanks = ring("-", "-", "-");
        blanks.sidedefs.push(Sidedef {
            sector: 0,
            ..Default::default()
        });
        assert!(
            misused_resource(&blanks, &context).is_empty(),
            "unset faces and the sky flat never report"
        );
    }

    #[test]
    fn the_undefined_checks_hold_back_on_exactly_the_misused_names() {
        let given = fixture(full_index());
        let context = context_of(&given);
        let face = ring("FLOOR1", "FLOOR1", "F_SKY1");
        assert!(
            missing_texture(&face, &context).is_empty(),
            "misused_resource owns the face row"
        );
        assert!(
            !misused_resource(&face, &context).is_empty(),
            "and it reports"
        );
        let plane = ring("WALL1", "WALL1", "FLOOR1");
        assert!(
            missing_flat(&plane, &context).is_empty(),
            "misused_resource owns the plane row"
        );
        assert!(
            !misused_resource(&plane, &context).is_empty(),
            "and it reports"
        );
        let both = fixture(ResourceIndex {
            textures: set(&["WALL1"]),
            flats: set(&["FLOOR1", "WALL1"]),
            has_texture_defs: true,
        });
        let both_context = context_of(&both);
        assert!(
            missing_texture(&ring("WALL1", "FLOOR1", "F_SKY1"), &both_context).is_empty()
                && missing_flat(&ring("WALL1", "WALL1", "F_SKY1"), &both_context).is_empty(),
            "a name carried as both kinds is judged sound by both checks"
        );
    }

    #[test]
    fn misuse_reports_even_where_the_undefined_checks_have_no_standard() {
        let no_defs = fixture(ResourceIndex {
            textures: set(&["WALL1"]),
            flats: set(&["FLOOR1"]),
            has_texture_defs: false,
        });
        let face_context = context_of(&no_defs);
        assert!(
            missing_texture(&ring("FLOOR1", "FLOOR1", "F_SKY1"), &face_context).is_empty(),
            "without definitions the texture check does not judge names"
        );
        let face_row = only_issue(misused_resource(
            &ring("FLOOR1", "FLOOR1", "F_SKY1"),
            &face_context,
        ));
        assert!(
            face_row.message.contains("middle texture \"FLOOR1\""),
            "the flat it carries still indicts the face: {}",
            face_row.message
        );
        let no_flats = fixture(ResourceIndex {
            textures: set(&["WALL1"]),
            flats: BTreeSet::new(),
            has_texture_defs: true,
        });
        let flat_context = context_of(&no_flats);
        assert!(
            missing_flat(&ring("WALL1", "WALL1", "F_SKY1"), &flat_context).is_empty(),
            "carrying no flats, the name check judges nothing"
        );
        let plane_row = only_issue(misused_resource(
            &ring("WALL1", "WALL1", "F_SKY1"),
            &flat_context,
        ));
        assert!(
            plane_row.message.contains("floor flat \"WALL1\""),
            "the texture it defines still indicts the plane: {}",
            plane_row.message
        );
    }
}
