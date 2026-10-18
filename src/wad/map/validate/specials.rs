//! Linedef-special checks over a decoded [`Map`]: specials outside the
//! flag family's table, sector specials that lost their tag, and tags no
//! sector answers.
//!
//! The `Doom` flag family consults [`Context::definitions`] (the detected
//! game's table, so generalized Boom specials above `0x2F80` are excused
//! as a family property); the `Hexen` family consults [`Context::hexen_specials`].
//! The tag checks are `Doom`-family only: `Hexen` lines trigger through
//! script arguments instead, and arguments are not validated here.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::vec::Vec;

use super::{Context, Issue, Severity};
use crate::defs::is_generalized;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::model::{Linedef, Map};

/// The [`super::specials`] table a map's flag family resolves its specials
/// against.
fn table<'ctx>(map: &Map, context: &'ctx Context<'ctx>) -> &'ctx crate::defs::Definitions {
    match map.flag_family() {
        FlagFamily::Doom => context.definitions,
        FlagFamily::Hexen => context.hexen_specials,
    }
}

/// Whether `line` carries a nonzero special number.
const fn has_special(line: &Linedef) -> bool {
    line.special != 0_i32
}

/// Linedefs whose special is neither in the family's table nor a Boom
/// generalized number.
pub(super) fn unknown_special(map: &Map, context: &Context<'_>) -> Vec<Issue> {
    let doom_family = map.flag_family() == FlagFamily::Doom;
    let table = table(map, context);
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| has_special(line))
        .filter(|&(_, line)| {
            !u32::try_from(line.special).is_ok_and(|number| {
                table.special(number).is_some() || (doom_family && is_generalized(number))
            })
        })
        .map(|(index, line)| {
            Issue::new(
                Severity::Warning,
                format!(
                    "linedef {index} uses special {}, which its map family does not define",
                    line.special,
                ),
            )
            .linedef(&[index])
        })
        .collect()
}

/// `Doom`-family linedefs whose special acts on a tagged sector but carry
/// tag 0. Unknown specials are left to [`unknown_special`].
pub(super) fn special_without_tag(map: &Map, context: &Context<'_>) -> Vec<Issue> {
    if map.flag_family() != FlagFamily::Doom {
        return Vec::new();
    }
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| has_special(line))
        .filter_map(|(index, line)| {
            let number = u32::try_from(line.special).ok()?;
            let def = context.definitions.special(number)?;
            (def.target == TARGET_SECTOR && line.tag == 0_i32).then(|| {
                Issue::new(
                    Severity::Warning,
                    format!(
                        "linedef {index} uses special {number} ({name}), which acts on a \
                         tagged sector, but carries tag 0",
                        name = def.name,
                    ),
                )
                .linedef(&[index])
            })
        })
        .collect()
}

/// `Doom`-family linedefs whose nonzero tag matches no sector's tag.
pub(super) fn tag_without_sector(map: &Map, _context: &Context<'_>) -> Vec<Issue> {
    if map.flag_family() != FlagFamily::Doom {
        return Vec::new();
    }
    let tags: BTreeSet<i32> = map
        .sectors
        .iter()
        .map(|sector| sector.tag)
        .filter(|tag| *tag != 0_i32)
        .collect();
    map.linedefs
        .iter()
        .enumerate()
        .filter(|&(_, line)| line.tag != 0_i32 && !tags.contains(&line.tag))
        .map(|(index, line)| {
            Issue::new(
                Severity::Warning,
                format!(
                    "linedef {index} carries tag {}, but no sector in the map uses that tag",
                    line.tag
                ),
            )
            .linedef(&[index])
        })
        .collect()
}

/// The [`crate::defs::SpecialDef::target`] text of specials that move
/// tagged sectors.
const TARGET_SECTOR: &str = "sector";

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{special_without_tag, tag_without_sector, unknown_special};
    use crate::defs::definitions_for;
    use crate::wad::classify::MapFormat;
    use crate::wad::game::Game;
    use crate::wad::map::model::{Linedef, Map, Sector, Sidedef, Thing, Vertex};
    use crate::wad::map::validate::Context;
    use crate::wad::name::LumpName;

    /// The built-in tables plus an empty resource index, owned together
    /// so a [`Context`] can borrow all three.
    struct Tables {
        /// The DOOM II rows.
        doom: crate::defs::Definitions,
        /// The `Hexen` specials rows.
        hexen: crate::defs::Definitions,
        /// The empty resource index: the specials checks never read it.
        resources: crate::wad::map::validate::ResourceIndex,
    }

    /// The built-in tables.
    fn tables() -> Tables {
        Tables {
            doom: definitions_for(Game::Doom2).expect("the embedded Doom II table is valid"),
            hexen: definitions_for(Game::Hexen).expect("the embedded Hexen table is valid"),
            resources: crate::wad::map::validate::ResourceIndex::default(),
        }
    }

    /// A validation context over the built-in tables.
    fn context(tables: &Tables) -> Context<'_> {
        Context {
            definitions: &tables.doom,
            hexen_specials: &tables.hexen,
            resources: &tables.resources,
        }
    }

    /// A vertex at `x`/`y`.
    const fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A line from `v1` to `v2` with `special` and `tag`.
    const fn special_line(v1: usize, v2: usize, special: i32, tag: i32) -> Linedef {
        Linedef {
            v1,
            v2,
            flags: 1,
            special,
            tag,
            args: [0; 5],
            front: Some(0),
            back: None,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    /// A `Doom` map of one wall line, one side, one sector, plus a player
    /// start so the thing checks stay out of these tests' way.
    fn map_with(lines: Vec<Linedef>) -> Map {
        Map {
            name: LumpName::parse("MAP01").expect("test names are valid"),
            format: MapFormat::Doom,
            things: vec![Thing {
                kind: 1,
                ..Default::default()
            }],
            vertices: vec![vertex(0, 0), vertex(64, 0), vertex(64, 64)],
            linedefs: lines,
            sidedefs: vec![Sidedef::default()],
            sectors: vec![Sector::default()],
            ..Default::default()
        }
    }

    /// The same map declared with the `zdoom` `UDMF` namespace: the
    /// `Hexen` flag family.
    fn hexen_map(lines: Vec<Linedef>) -> Map {
        let mut map = map_with(lines);
        map.format = MapFormat::Udmf;
        map.udmf_namespace = Some("zdoom".to_owned());
        map
    }

    #[test]
    fn unknown_special_accepts_table_rows_and_generalized_numbers() {
        let tables = tables();
        let ctx = context(&tables);
        let door = special_line(0, 1, 1, 0);
        let generalized = special_line(1, 2, 0x4000, 0);
        let plain = map_with(vec![door, generalized]);
        assert!(
            unknown_special(&plain, &ctx).is_empty(),
            "special 1 is a DOOM II row and 0x4000 is a generalized number"
        );
        let wild = map_with(vec![special_line(0, 1, 4321, 0)]);
        let issues = unknown_special(&wild, &ctx);
        let issue = issues.first().expect("4321 is in no table");
        assert_eq!(issue.linedefs, vec![0], "the offending linedef is attached");
        assert!(
            issue.message.contains("4321"),
            "the message names the number: {}",
            issue.message
        );
    }

    #[test]
    fn unknown_special_uses_the_hexen_table_for_hexen_family_maps() {
        let tables = tables();
        let ctx = context(&tables);
        // Hexen special 80 (PolyObj_Teleport) is not a DOOM II row, and a
        // generalized number is not a Hexen row either.
        let map = hexen_map(vec![special_line(0, 1, 80, 0)]);
        assert!(
            unknown_special(&map, &ctx).is_empty(),
            "special 80 is a Hexen row"
        );
        let generalized = hexen_map(vec![special_line(0, 1, 0x4000, 0)]);
        assert_eq!(
            unknown_special(&generalized, &ctx).len(),
            1,
            "generalized numbers are a Doom-family property only"
        );
        let doom_line = map_with(vec![special_line(0, 1, 104, 0)]);
        assert!(
            unknown_special(&doom_line, &ctx).is_empty(),
            "the Doom family reads its own table: {}",
            definitions_for(Game::Doom2)
                .expect("valid table")
                .special(104)
                .map_or("absent", |_| "present"),
        );
        let hexen_gap = hexen_map(vec![special_line(0, 1, 104, 0)]);
        assert_eq!(
            unknown_special(&hexen_gap, &ctx).len(),
            1,
            "104 is a DOOM II row the Hexen table lacks"
        );
    }

    #[test]
    fn special_without_tag_flags_sector_specials_on_untagged_lines() {
        let tables = tables();
        let ctx = context(&tables);
        let flagged = map_with(vec![special_line(0, 1, 29, 3)]);
        assert!(
            special_without_tag(&flagged, &ctx).is_empty(),
            "special 29 with a tag is complete"
        );
        let untagged = map_with(vec![special_line(0, 1, 29, 0)]);
        let issues = special_without_tag(&untagged, &ctx);
        let issue = issues.first().expect("special 29 (S1 Door) needs a tag");
        assert_eq!(issue.linedefs, vec![0], "the linedef is attached");
        assert!(
            issue.message.contains("Door") && issue.message.contains("carries tag 0"),
            "the message names the special and the problem: {}",
            issue.message
        );
        let unknown = map_with(vec![special_line(0, 1, 4321, 0)]);
        assert!(
            special_without_tag(&unknown, &ctx).is_empty(),
            "unknown specials are not second-guessed here"
        );
    }

    #[test]
    fn special_without_tag_skips_the_hexen_family() {
        let tables = tables();
        let ctx = context(&tables);
        let map = hexen_map(vec![special_line(0, 1, 80, 0)]);
        assert!(
            special_without_tag(&map, &ctx).is_empty(),
            "Hexen-family lines trigger through arguments, not tags"
        );
    }

    #[test]
    fn tag_without_sector_finds_tags_no_sector_answers() {
        let tables = tables();
        let ctx = context(&tables);
        let quiet = map_with(vec![special_line(0, 1, 0, 0)]);
        assert!(
            tag_without_sector(&quiet, &ctx).is_empty(),
            "tag 0 is 'no tag', not a search term"
        );
        let mut answered = map_with(vec![special_line(0, 1, 1, 3)]);
        answered.sectors.first_mut().expect("the base sector").tag = 3_i32;
        assert!(
            tag_without_sector(&answered, &ctx).is_empty(),
            "sector 0 carries the tag"
        );
        answered.sectors.first_mut().expect("the base sector").tag = 9_i32;
        let issues = tag_without_sector(&answered, &ctx);
        let issue = issues.first().expect("nothing answers tag 3 any more");
        assert_eq!(issue.linedefs, vec![0], "the orphaned line is attached");
        assert!(
            issue.message.contains("tag 3"),
            "the message names the tag: {}",
            issue.message
        );
    }

    #[test]
    fn tag_without_sector_skips_the_hexen_family() {
        let tables = tables();
        let ctx = context(&tables);
        let map = hexen_map(vec![special_line(0, 1, 0, 5)]);
        assert!(
            tag_without_sector(&map, &ctx).is_empty(),
            "Hexen-family tags belong to the arguments, not the sector set"
        );
    }
}
