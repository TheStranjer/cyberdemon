//! The pure per-map replacements the bulk `replace_*` tools run: rewrite
//! every sidedef texture face, sector flat or thing type of one decoded
//! [`Map`] that matches a pattern, and count the fields that changed.
//!
//! Name matching is the case-insensitive `*`/`?` glob every listing
//! already uses (`crate::wad::name::glob_match`): a literal pattern
//! matches its own name, `WALL*` sweeps up everything with that prefix,
//! and a face storing `"-"` or the empty string is only rewritten when
//! the pattern truly globs it (`"*"` does, `WALL*` does not). A field
//! that already stores the target name is left alone and stays out of
//! the count, so the answer is what changed, not what matched.
//!
//! [`replace_textures`] and [`replace_flats`] take the face/surface set
//! to touch; an empty set means all of them. [`replace_thing_types`]
//! rewrites [`Thing`](crate::wad::map::model::Thing) numbers only: the
//! position, height, facing, flags, tid, special and args of every
//! thing it touches come out byte-identical.

use alloc::format;
use alloc::string::String;

use super::super::model::{Map, Sector, Sidedef};
use crate::error::CyberError;
use crate::wad::name::glob_match;

/// One [`Sidedef`] texture face a replacement can target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// [`Sidedef::upper`].
    Upper,
    /// [`Sidedef::middle`].
    Middle,
    /// [`Sidedef::lower`].
    Lower,
}

impl Face {
    /// Every face, in the order the `SIDEDEFS` record stores them.
    pub const ALL: [Self; 3] = [Self::Upper, Self::Middle, Self::Lower];

    /// Parses a face name: `"upper"`, `"middle"` or `"lower"`,
    /// case-insensitively.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the three accepted values.
    #[inline]
    pub fn parse(text: &str) -> Result<Self, CyberError> {
        match text.to_ascii_lowercase().as_str() {
            "upper" => Ok(Self::Upper),
            "middle" => Ok(Self::Middle),
            "lower" => Ok(Self::Lower),
            other => Err(CyberError::InvalidArgument(format!(
                "texture face {other:?} is neither \"upper\", \"middle\" nor \"lower\"; \
                 pass one of those three"
            ))),
        }
    }

    /// The name this face of `side` stores.
    fn get(self, side: &Sidedef) -> &str {
        match self {
            Self::Upper => &side.upper,
            Self::Middle => &side.middle,
            Self::Lower => &side.lower,
        }
    }

    /// Stores `name` in this face of `side`.
    fn set(self, side: &mut Sidedef, name: String) {
        match self {
            Self::Upper => side.upper = name,
            Self::Middle => side.middle = name,
            Self::Lower => side.lower = name,
        }
    }
}

/// One [`Sector`] flat surface a replacement can target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// [`Sector::floor_flat`].
    Floor,
    /// [`Sector::ceiling_flat`].
    Ceiling,
}

impl Surface {
    /// Both surfaces.
    pub const ALL: [Self; 2] = [Self::Floor, Self::Ceiling];

    /// Parses a surface name: `"floor"` or `"ceiling"`,
    /// case-insensitively.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the two accepted values.
    #[inline]
    pub fn parse(text: &str) -> Result<Self, CyberError> {
        match text.to_ascii_lowercase().as_str() {
            "floor" => Ok(Self::Floor),
            "ceiling" => Ok(Self::Ceiling),
            other => Err(CyberError::InvalidArgument(format!(
                "flat surface {other:?} is neither \"floor\" nor \"ceiling\"; \
                 pass one of those two"
            ))),
        }
    }

    /// The name this surface of `sector` stores.
    fn get(self, sector: &Sector) -> &str {
        match self {
            Self::Floor => &sector.floor_flat,
            Self::Ceiling => &sector.ceiling_flat,
        }
    }

    /// Stores `name` on this surface of `sector`.
    fn set(self, sector: &mut Sector, name: String) {
        match self {
            Self::Floor => sector.floor_flat = name,
            Self::Ceiling => sector.ceiling_flat = name,
        }
    }
}

/// Whether the field storing `current` changes: it is globbed by `from`
/// and does not yet hold `to`.
fn matches(current: &str, from: &str, to: &str) -> bool {
    glob_match(current.as_bytes(), from) && current != to
}

/// Rewrites the listed faces of one `side`; its count of changed fields.
fn side_changes(side: &mut Sidedef, wanted: &[Face], from: &str, to: &str) -> usize {
    let mut changed = 0_usize;
    for face in wanted {
        if matches(face.get(side), from, to) {
            face.set(side, to.to_owned());
            changed = changed.saturating_add(1);
        }
    }
    changed
}

/// Rewrites the listed surfaces of one `sector`; its count of changed
/// fields.
fn sector_changes(sector: &mut Sector, wanted: &[Surface], from: &str, to: &str) -> usize {
    let mut changed = 0_usize;
    for surface in wanted {
        if matches(surface.get(sector), from, to) {
            surface.set(sector, to.to_owned());
            changed = changed.saturating_add(1);
        }
    }
    changed
}

/// Rewrites every [`Sidedef`] texture face of `map` the glob `from`
/// matches to `to`, over the faces in `parts` (all three when empty).
///
/// Fields that already hold `to` are left alone and stay out of the
/// returned count of changed face fields. Offsets, sector assignments
/// and every other block stay untouched.
#[must_use]
#[inline]
pub fn replace_textures(map: &mut Map, from: &str, to: &str, parts: &[Face]) -> usize {
    let wanted: &[Face] = if parts.is_empty() { &Face::ALL } else { parts };
    let count_side = |side: &mut Sidedef| side_changes(side, wanted, from, to);
    map.sidedefs.iter_mut().map(count_side).sum()
}

/// Rewrites every [`Sector`] flat of `map` the glob `from` matches to
/// `to`, over the surfaces in `surfaces` (both when empty).
///
/// Fields that already hold `to` are left alone and stay out of the
/// returned count of changed flat fields. Heights, light levels,
/// specials, tags and every other block stay untouched.
#[must_use]
#[inline]
pub fn replace_flats(map: &mut Map, from: &str, to: &str, surfaces: &[Surface]) -> usize {
    let wanted: &[Surface] = if surfaces.is_empty() {
        &Surface::ALL
    } else {
        surfaces
    };
    let count_sector = |sector: &mut Sector| sector_changes(sector, wanted, from, to);
    map.sectors.iter_mut().map(count_sector).sum()
}

/// Rewrites every [`Thing`](crate::wad::map::model::Thing) of `map` whose type
/// number is `from` to `to`, and returns how many things changed.
///
/// Only the type number moves: `x`, `y`, `z`, `angle`, `flags`, `tid`,
/// `special` and `args` of every touched thing stay exactly as they
/// were, and `from == to` changes and counts nothing.
#[must_use]
#[inline]
pub fn replace_thing_types(map: &mut Map, from: i32, to: i32) -> usize {
    if from == to {
        return 0_usize;
    }
    let mut changed = 0_usize;
    for spawn in &mut map.things {
        if spawn.kind == from {
            spawn.kind = to;
            changed = changed.saturating_add(1);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::{Face, Surface, replace_flats, replace_textures, replace_thing_types};
    use crate::wad::map::{Map, Sector, Sidedef, Thing};

    /// A map with the given sidedef faces (upper, middle, lower per row)
    /// and sectors (floor, ceiling per row).
    fn map_with(faces: &[(&str, &str, &str)], flats: &[(&str, &str)]) -> Map {
        Map {
            sidedefs: faces
                .iter()
                .map(|&(upper, middle, lower)| Sidedef {
                    upper: upper.to_owned(),
                    middle: middle.to_owned(),
                    lower: lower.to_owned(),
                    ..Default::default()
                })
                .collect(),
            sectors: flats
                .iter()
                .map(|&(floor, ceiling)| Sector {
                    floor_flat: floor.to_owned(),
                    ceiling_flat: ceiling.to_owned(),
                    ..Default::default()
                })
                .collect(),
            ..Map::default()
        }
    }

    /// A thing of type `kind` with distinctive values in every other
    /// field, for the untouched-field guarantees.
    fn loaded_thing(kind: i32) -> Thing {
        Thing {
            tid: 7_i32,
            x: -128_i32,
            y: 256_i32,
            z: 48_i32,
            angle: 315_i32,
            kind,
            flags: 0x1F_u32,
            special: 10_i32,
            args: [1_i32, 2, 3, 4, 5],
            ..Default::default()
        }
    }

    #[test]
    fn face_and_surface_parses_are_case_insensitive() {
        for (text, want) in [
            ("upper", Face::Upper),
            ("MIDDLE", Face::Middle),
            ("Lower", Face::Lower),
        ] {
            assert_eq!(Face::parse(text).expect("a face name"), want, "{text}");
        }
        for text in ["", "wall", "up"] {
            let error = Face::parse(text).expect_err("a non-face word is refused");
            assert_eq!(error.code(), "invalid_argument", "bad face code");
            assert!(
                error.to_string().contains("upper"),
                "the message names the choices: {error}"
            );
        }
        assert_eq!(
            Surface::parse("Ceiling").expect("a surface name"),
            Surface::Ceiling,
            "case folds"
        );
        assert_eq!(
            Surface::parse("floor").expect("a surface name"),
            Surface::Floor,
            "the other one"
        );
        let error = Surface::parse("roof").expect_err("not a surface");
        assert_eq!(error.code(), "invalid_argument", "bad surface code");
        assert!(
            error.to_string().contains("ceiling"),
            "the message names the choices: {error}"
        );
    }

    #[test]
    fn replace_textures_globs_case_insensitively_across_faces() {
        let mut map = map_with(
            &[("WALL64", "brick", "-"), ("wall2", "-", "WALL1")],
            &[("FLOOR4", "FLOOR4")],
        );
        let changed = replace_textures(&mut map, "WALL*", "STARTAN", &[]);
        assert_eq!(changed, 3_usize, "the glob hits three faces in two rows");
        assert_eq!(map.sidedefs[0].upper, "STARTAN", "the matching upper");
        assert_eq!(map.sidedefs[0].middle, "brick", "no match keeps its name");
        assert_eq!(map.sidedefs[0].lower, "-", "an unglobbed \"-\" stays");
        assert_eq!(
            map.sidedefs[1].upper, "STARTAN",
            "lowercase input globs too"
        );
        assert_eq!(
            map.sidedefs[1].lower, "STARTAN",
            "and so does the second wall"
        );
    }

    #[test]
    fn replace_textures_narrows_to_the_listed_faces() {
        let mut map = map_with(&[("WALL64", "WALL2", "WALL1")], &[]);
        let changed = replace_textures(&mut map, "WALL*", "BRICK1", &[Face::Upper]);
        assert_eq!(changed, 1_usize, "only the named face counts");
        assert_eq!(map.sidedefs[0].upper, "BRICK1", "the upper moved");
        assert_eq!(map.sidedefs[0].middle, "WALL2", "the middle stands");
        assert_eq!(map.sidedefs[0].lower, "WALL1", "and the lower stands");
    }

    #[test]
    fn replace_textures_counts_a_star_over_the_empty_faces() {
        let mut map = map_with(&[("-", "", "WALL1")], &[]);
        let changed = replace_textures(&mut map, "*", "AZTEC1", &[]);
        assert_eq!(changed, 3_usize, "\"*\" globs \"-\" and the empty face too");
        assert_eq!(map.sidedefs[0].upper, "AZTEC1", "the dash became a texture");
        assert_eq!(
            map.sidedefs[0].middle, "AZTEC1",
            "and so did the empty slot"
        );
    }

    #[test]
    fn replace_textures_leaves_and_ignores_fields_already_named_to() {
        let mut map = map_with(&[("TWALL1", "WALL2", "-")], &[]);
        let changed = replace_textures(&mut map, "WALL*", "WALL2", &[Face::Upper]);
        assert_eq!(changed, 0_usize, "a miss on the sole targeted face");
        assert_eq!(map.sidedefs[0].middle, "WALL2", "the unglobbed face stands");
        let mut already = map_with(&[("TWALL1", "-", "-")], &[]);
        assert_eq!(
            replace_textures(&mut already, "T*", "TWALL1", &[Face::Upper]),
            0_usize,
            "already the target name: matched, changed nothing"
        );
        assert_eq!(already.sidedefs[0].upper, "TWALL1", "the field stands");
    }

    #[test]
    fn replace_flats_targets_the_listed_surfaces_and_counts() {
        let mut map = map_with(&[], &[("FLOOR4", "F_SKY1"), ("FLOOR4", "CEIL5")]);
        let floors = replace_flats(&mut map, "FLOOR*", "FLAT1", &[Surface::Floor]);
        assert_eq!(floors, 2_usize, "both floors");
        assert_eq!(map.sectors[0].floor_flat, "FLAT1", "the first floor moved");
        assert_eq!(
            map.sectors[0].ceiling_flat, "F_SKY1",
            "ceilings were not listed"
        );
        assert_eq!(
            map.sectors[1].ceiling_flat, "CEIL5",
            "and stay exactly as they were"
        );
        let all = replace_flats(&mut map, "*", "NUKAGE1", &[]);
        assert_eq!(
            all, 4_usize,
            "an empty surface set means both, and \"*\" globs all four"
        );
        assert_eq!(
            map.sectors[1].floor_flat, "NUKAGE1",
            "the other floor moved too"
        );
        assert_eq!(
            map.sectors[0].ceiling_flat, "NUKAGE1",
            "and so did every ceiling"
        );
    }

    #[test]
    fn replacements_that_match_nothing_change_nothing() {
        let mut map = map_with(&[("WALL64", "-", "-")], &[("FLOOR4", "FLOOR4")]);
        let before = map.clone();
        assert_eq!(
            replace_textures(&mut map, "NOPE*", "X", &[]),
            0_usize,
            "no face"
        );
        assert_eq!(
            replace_flats(&mut map, "NOPE*", "X", &[]),
            0_usize,
            "no flat"
        );
        assert_eq!(replace_thing_types(&mut map, 3004, 9), 0_usize, "no thing");
        assert_eq!(map, before, "the map stands exactly as it was");
    }

    #[test]
    fn replace_thing_types_moves_only_the_kind() {
        let mut map = Map {
            things: vec![
                loaded_thing(3004_i32),
                loaded_thing(1_i32),
                loaded_thing(3004_i32),
            ],
            ..Map::default()
        };
        // The whole-map equality below is the untouched-field guarantee:
        // the expected map differs from the fixture only in the two
        // rewritten type numbers.
        let mut want = map.clone();
        want.things[0].kind = 9_i32;
        want.things[2].kind = 9_i32;
        let changed = replace_thing_types(&mut map, 3004, 9);
        assert_eq!(changed, 2_usize, "both barons");
        assert_eq!(
            map, want,
            "only the two type numbers moved: every x, y, z, angle, flags, tid, special and args stands"
        );
        let same = replace_thing_types(&mut map, 9, 9);
        assert_eq!(same, 0_usize, "a no-op rewrite counts nothing");
    }
}
