//! DeHackEd thing overrides: the `Thing <n>` block subset and the mobj map.
//!
//! A DeHackEd patch edits an internal mobj table indexed from 1 in Doom II
//! order. `apply` reads the patch text through the shared line reader
//! `crate::defs::dehacked_edit::parse` — the same one `get_dehacked` and
//! `edit_dehacked` list and edit blocks with — and moves the built-in
//! definitions of its `Thing <n>` blocks: `ID #` sets a new doomednum
//! (`-1` makes the mobj unplaceable), `Width` replaces the blocking radius
//! (written in 16.16 fixed point), and every other key is ignored. `#`
//! comment lines are skipped and every other block kind or BEX section
//! simply bounds the Thing blocks, so both readers always see the same
//! blocks. Unparseable parts become warnings, never errors; the only error
//! is a broken embedded mobj table, which is a build bug.

use std::sync::LazyLock;

use crate::defs::dehacked_edit::{self, Block};
use crate::defs::registry::{Definitions, game_label};
use crate::defs::table::{Category, ThingDef};
use crate::defs::tsv::{Table, embedded};
use crate::error::CyberError;
use crate::fixed::FRACUNIT;
use crate::wad::game::Game;

/// The DeHackEd mobj index to doomednum map, compiled into the binary from
/// `data/defs/deh_mobj.tsv` (`index<TAB>doomednum`, 1-based, `-1` for a
/// mobj the editors give no number).
const DEH_MOBJ_TSV: &str = include_str!("../../data/defs/deh_mobj.tsv");

/// The [`ThingDef::source`] label of rows moved or edited by DEHACKED.
const SOURCE_DEHACKED: &str = "dehacked";

/// The warning prefix naming this overlay as the reporting stream.
const WARNING_PREFIX: &str = "dehacked";

/// The category of synthesized rows whose mobj has no built-in definition.
const CATEGORY_DEHACKED: &str = "Custom (DeHackEd)";

/// The radius of a synthesized row whose patch names none.
const DEFAULT_RADIUS: u32 = 20;

/// The canonical block kind whose header numbers a mobj.
const KIND_THING: &str = "Thing";

/// The key that moves a thing's doomednum.
const KEY_ID: &str = "id #";

/// The key that replaces a thing's blocking radius (16.16 fixed point).
const KEY_WIDTH: &str = "width";

/// The tab-separated header columns [`DEH_MOBJ_TSV`] must carry.
const MOBJ_HEADER: [&str; MOBJ_FIELDS] = ["index", "doomednum"];

/// The number of tab-separated fields one mobj row must carry.
const MOBJ_FIELDS: usize = 2;

/// The parsed [`DEH_MOBJ_TSV`], produced on first use. A parse failure is
/// kept as an error string and reported as [`CyberError::Internal`], never
/// a panic: broken embedded data is a build bug.
static DEH_MOBJS: LazyLock<Result<Vec<i64>, String>> =
    LazyLock::new(|| parse_mobj_table(DEH_MOBJ_TSV));

/// One `Thing <n>` block reduced to the values the table edit needs.
struct Edit {
    /// The 1-based mobj index, for warning text.
    index: i64,
    /// The doomednum the mobj table names for the index.
    original: i64,
    /// The header name, when given.
    name: Option<String>,
    /// The raw `ID #` text, when the block sets it.
    id: Option<String>,
    /// The radius from `Width`, when it parsed.
    radius: Option<u32>,
}

/// Applies the DeHackEd thing blocks in `text` to `defs`, collecting
/// warnings into [`Definitions::warnings`]. `game` labels synthesized rows.
///
/// # Errors
///
/// [`CyberError::Internal`] when the embedded mobj table is malformed.
pub(crate) fn apply(text: &str, game: Game, defs: &mut Definitions) -> Result<(), CyberError> {
    let mobjs = embedded_mobjs()?;
    let patch = dehacked_edit::parse(text);
    defs.warnings.extend(
        patch
            .warnings
            .into_iter()
            .map(|note| format!("{WARNING_PREFIX}: {note}")),
    );
    for block in patch.blocks.iter().filter(|block| block.kind == KIND_THING) {
        if let Some(edit) = build_edit(block, mobjs, &mut defs.warnings) {
            apply_edit(&edit, game, defs);
        }
    }
    Ok(())
}

/// The embedded mobj table's doomednums: `doomednums.get(index - 1)` is
/// the doomednum DeHackEd thing `index` patches, parsed once on first use.
///
/// # Errors
///
/// [`CyberError::Internal`] when the embedded table is malformed, which is
/// a build bug rather than a client problem.
pub(crate) fn embedded_mobjs() -> Result<&'static [i64], CyberError> {
    embedded(&DEH_MOBJS, "dehacked mobj").map(Vec::as_slice)
}

/// The tab-separated mobj rows in order: `doomednums.get(index - 1)` is the
/// doomednum DeHackEd thing `index` patches.
///
/// # Errors
///
/// A `String` naming the header problem or the first malformed or
/// out-of-order row.
fn parse_mobj_table(text: &str) -> Result<Vec<i64>, String> {
    let table = Table::parse("mobj table", &MOBJ_HEADER, text)?;
    let mut doomednums: Vec<i64> = Vec::new();
    for row in table.rows() {
        let [index_text, doomednum_text] = row.columns::<MOBJ_FIELDS>()?;
        let expected = i64::try_from(doomednums.len())
            .ok()
            .and_then(|seen| seen.checked_add(1))
            .ok_or_else(|| row.fail("too many rows"))?;
        let index = row.i64("index", index_text)?;
        if index != expected {
            return Err(row.fail(&format!("index {index} is not the expected {expected}")));
        }
        doomednums.push(row.i64("doomednum", doomednum_text)?);
    }
    Ok(doomednums)
}

/// The trimmed value of the block's `Key = Value` line whose key matches
/// the lowercased `key` case-insensitively, when the block sets it.
fn block_value<'block>(block: &'block Block, key: &str) -> Option<&'block str> {
    block
        .fields
        .iter()
        .find(|field| field.key.eq_ignore_ascii_case(key))
        .map(|field| field.value.as_str())
}

/// Resolves the block's mobj index against `mobjs` and reads the keys, or
/// warns and returns `None` for a header that names no index or one outside
/// the table.
fn build_edit(block: &Block, mobjs: &[i64], warnings: &mut Vec<String>) -> Option<Edit> {
    let Some(number) = block.number else {
        warnings.push(format!(
            "{WARNING_PREFIX}: the Thing block on line {} names no mobj index; its keys are \
             ignored",
            block.line.saturating_add(1)
        ));
        return None;
    };
    let position = usize::try_from(number)
        .ok()
        .filter(|index| *index > 0_usize)
        .and_then(|index| index.checked_sub(1));
    let mapped = position.and_then(|offset| mobjs.get(offset).copied());
    let Some(original) = mapped else {
        warnings.push(format!(
            "{WARNING_PREFIX}: thing index {number} is outside the mobj table of {} entries",
            mobjs.len()
        ));
        return None;
    };
    Some(Edit {
        index: number,
        original,
        name: block.name.clone(),
        id: block_value(block, KEY_ID).map(str::to_owned),
        radius: block_value(block, KEY_WIDTH).and_then(|text| parse_width(text, warnings)),
    })
}

/// The radius in map units from a `Width` written in 16.16 fixed point.
fn parse_width(text: &str, warnings: &mut Vec<String>) -> Option<u32> {
    let Ok(fixed) = text.parse::<i64>() else {
        warnings.push(format!(
            "{WARNING_PREFIX}: Width {text:?} is not a 16.16 fixed-point number"
        ));
        return None;
    };
    let radius = fixed
        .checked_div(i64::from(FRACUNIT))
        .and_then(|units| u32::try_from(units).ok());
    if radius.is_none() {
        warnings.push(format!(
            "{WARNING_PREFIX}: Width {text:?} is not a positive 16.16 fixed-point radius"
        ));
    }
    radius
}

/// Applies one resolved block: `ID #` places (or unplaces) the mobj's
/// definition, otherwise a parsed `Width` edits the original in place.
fn apply_edit(edit: &Edit, game: Game, defs: &mut Definitions) {
    let Some(id_text) = edit.id.as_deref() else {
        apply_radius(edit, defs);
        return;
    };
    let Ok(number) = id_text.parse::<i64>() else {
        defs.warnings.push(format!(
            "{WARNING_PREFIX}: thing {} has ID # {id_text:?}, which is not a number",
            edit.index
        ));
        return;
    };
    if number < 0_i64 {
        if let Ok(original) = u32::try_from(edit.original) {
            defs.remove_thing(original);
        }
        return;
    }
    match u32::try_from(number) {
        Ok(kind) => place(edit, kind, game, defs),
        Err(_) => defs.warnings.push(format!(
            "{WARNING_PREFIX}: thing {} has ID # {id_text:?}, which is above the thing number \
             range",
            edit.index
        )),
    }
}

/// Copies the mobj's current definition to type `kind` (or synthesizes one
/// from the header name), and removes the original number when it differs.
fn place(edit: &Edit, kind: u32, game: Game, defs: &mut Definitions) {
    let source_kind = u32::try_from(edit.original).ok();
    let base = source_kind
        .and_then(|original| defs.thing(original))
        .cloned();
    let Some(mut def) = base else {
        synthesize(edit, kind, game, defs);
        return;
    };
    def.kind = kind;
    if let Some(name) = edit.name.as_deref() {
        name.clone_into(&mut def.name);
    }
    if let Some(radius) = edit.radius {
        def.radius = radius;
    }
    SOURCE_DEHACKED.clone_into(&mut def.source);
    if let Some(original) = source_kind.filter(|found| *found != kind) {
        defs.remove_thing(original);
    }
    defs.put_thing(def);
}

/// A row for a mobj whose doomednum has no built-in definition: possible
/// only when the block header names one; otherwise this warns.
fn synthesize(edit: &Edit, kind: u32, game: Game, defs: &mut Definitions) {
    let Some(name) = edit.name.as_deref() else {
        defs.warnings.push(format!(
            "{WARNING_PREFIX}: thing {} has no built-in row to move to {kind} and the header \
             names none",
            edit.index
        ));
        return;
    };
    defs.put_thing(ThingDef {
        kind,
        name: name.to_owned(),
        sprite: String::new(),
        frame: 'A',
        radius: edit.radius.unwrap_or(DEFAULT_RADIUS),
        category: CATEGORY_DEHACKED.to_owned(),
        group: Category::parse(CATEGORY_DEHACKED),
        hanging: false,
        game: game_label(game).to_owned(),
        source: SOURCE_DEHACKED.to_owned(),
    });
}

/// A parsed `Width` without `ID #` edits the definition the mobj currently
/// has under its doomednum.
fn apply_radius(edit: &Edit, defs: &mut Definitions) {
    let Some(radius) = edit.radius else {
        return;
    };
    let Some(kind) = u32::try_from(edit.original).ok() else {
        return;
    };
    let Some(def) = defs.thing_mut(kind) else {
        return;
    };
    def.radius = radius;
    SOURCE_DEHACKED.clone_into(&mut def.source);
}

#[cfg(test)]
mod tests {
    use crate::defs::dehacked::{DEH_MOBJ_TSV, apply, parse_mobj_table};
    use crate::defs::registry::definitions_for;
    use crate::wad::game::Game;

    /// A fresh DOOM II table to patch.
    fn defs() -> crate::defs::Definitions {
        definitions_for(Game::Doom2).expect("the embedded tables are valid")
    }

    /// Whether any warning contains `needle`.
    fn warns(defs: &crate::defs::Definitions, needle: &str) -> bool {
        defs.warnings.iter().any(|w| w.contains(needle))
    }

    #[test]
    fn the_embedded_mobj_table_is_the_doom_ii_order() {
        let doomednums = parse_mobj_table(DEH_MOBJ_TSV).expect("the shipped table must parse");
        assert_eq!(doomednums.len(), 137, "137 Doom II mobjs");
        let first = doomednums.first().expect("a first row");
        assert_eq!(*first, -1, "mobj 1 (Player) is not placeable");
        let imp = doomednums.get(11).expect("mobj 12 exists");
        assert_eq!(*imp, 3001, "mobj 12 is the Imp");
        let last = doomednums.last().expect("a last row");
        assert_eq!(*last, 81, "mobj 137 is the space barrel");
    }

    #[test]
    fn a_mobj_table_row_with_the_wrong_field_count_is_rejected() {
        let error = parse_mobj_table("index\tdoomednum\n1\t-1\textra\n")
            .expect_err("a three-field row is malformed");
        assert!(error.contains("line 2"), "the line is named: {error}");
    }

    #[test]
    fn a_mobj_table_index_gap_is_rejected() {
        let error =
            parse_mobj_table("index\tdoomednum\n2\t3004\n").expect_err("indices start at 1");
        assert!(error.contains("expected 1"), "the gap is named: {error}");
    }

    #[test]
    fn a_comment_inside_a_thing_block_keeps_its_keys() {
        let mut defs = defs();
        let text = "Thing 12 (Evil Imp)\n# a note\nID # = 20000\nWidth = 655360\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        assert!(defs.thing(3001).is_none(), "3001 no longer spawns the Imp");
        let moved = defs
            .thing(20_000)
            .expect("the comment did not close the block");
        assert_eq!(moved.name, "Evil Imp", "the header name");
        assert_eq!(
            moved.radius, 10,
            "the Width after the comment still applies"
        );
        assert!(defs.warnings.is_empty(), "no warnings: {:?}", defs.warnings);
    }

    #[test]
    fn keys_run_until_the_next_header_or_a_blank_line() {
        let mut defs = defs();
        let text = "Thing 12\nID # = 20000\n\nID # = 30001\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        assert!(
            defs.thing(20_000).is_some(),
            "the block's own ID # moved it"
        );
        assert!(defs.thing(30_001).is_none(), "the stray key edited nothing");
        assert_eq!(defs.warnings.len(), 1, "the stray key warns once");
        assert!(
            warns(&defs, "outside any block"),
            "the warning names the stray key: {:?}",
            defs.warnings
        );
    }

    #[test]
    fn a_non_numeric_thing_header_warns_and_cannot_move_anything() {
        let mut defs = defs();
        let text = "Thing xx\nID # = 20000\nThing 12\nID # = 20001\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        assert_eq!(
            defs.warnings.len(),
            2,
            "both notes arrive: {:?}",
            defs.warnings
        );
        assert!(
            warns(&defs, "header number"),
            "the shared parser's own note is kept: {:?}",
            defs.warnings
        );
        assert!(
            defs.thing(3001).is_none(),
            "the later block still moves the Imp"
        );
        assert!(defs.thing(20_001).is_some(), "to its own number");
    }

    #[test]
    fn bex_sections_and_text_blocks_frame_the_thing_edits() {
        let mut defs = defs();
        let text = "Patch File for DeHackEd v3.0\nDoom version = 21\nPatch format = 6\n\n\
                    Thing 12\nID # = 20000\n\n[STRINGS]\nHUDSTR_X = HELLO\n\nText 6 13\n\
                    Troop\nBaron of Hell\n\nMisc\nID # = 9\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        assert!(defs.thing(20_000).is_some(), "the Thing block edited");
        assert!(defs.thing(3001).is_none(), "and the old number is freed");
        assert!(
            defs.warnings.is_empty(),
            "a well-formed BEX patch warns about nothing: {:?}",
            defs.warnings
        );
    }

    #[test]
    fn a_text_block_body_never_opens_thing_blocks() {
        let mut defs = defs();
        let text = "Text 9 9\nThing 13\nID # = 7\n\nMisc\nID # = 8\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        assert_eq!(
            defs.thing(7).expect("type 7").name,
            "Spider Mastermind",
            "the body's Thing line changed nothing"
        );
        assert!(
            defs.warnings.is_empty(),
            "the body stays opaque: {:?}",
            defs.warnings
        );
    }

    #[test]
    fn moving_the_imp_gives_the_new_number_and_frees_the_old() {
        let mut defs = defs();
        apply("Thing 12\nID # = 20000\n", Game::Doom2, &mut defs).expect("the table is valid");
        assert!(defs.thing(3001).is_none(), "3001 no longer spawns the Imp");
        let moved = defs.thing(20_000).expect("20000 is the Imp now");
        assert_eq!(moved.name, "Imp", "the built-in name carries over");
        assert_eq!(moved.sprite, "TROO", "the built-in sprite carries over");
        assert_eq!(moved.radius, 20, "the built-in radius carries over");
        assert_eq!(moved.source, "dehacked", "the row records its source");
        assert!(defs.warnings.is_empty(), "no warnings: {:?}", defs.warnings);
    }

    #[test]
    fn the_header_name_wins_and_width_replaces_the_radius() {
        let mut defs = defs();
        let text = "Thing 12 (Evil Imp)\nID # = 20000\nWidth = 655360\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        let moved = defs.thing(20_000).expect("20000 exists");
        assert_eq!(moved.name, "Evil Imp", "the header name wins");
        assert_eq!(moved.radius, 10, "655360 fixed units are 10 map units");
    }

    #[test]
    fn minus_one_makes_the_mobj_unplaceable() {
        let mut defs = defs();
        apply("Thing 12\nID # = -1\n", Game::Doom2, &mut defs).expect("the table is valid");
        assert!(defs.thing(3001).is_none(), "the Imp leaves the table");
    }

    #[test]
    fn width_without_id_edits_the_original_in_place() {
        let mut defs = defs();
        apply("Thing 12\nWidth = 131072\n", Game::Doom2, &mut defs).expect("the table is valid");
        let imp = defs.thing(3001).expect("the Imp stays");
        assert_eq!(imp.radius, 2, "131072 fixed units are 2 map units");
        assert_eq!(imp.source, "dehacked", "the edit records its source");
    }

    #[test]
    fn a_mobj_without_a_built_in_row_synthesizes_from_the_header_name() {
        let mut defs = defs();
        let text = "Thing 1 (Player Sprite)\nID # = 20001\nWidth = 1310720\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        let made = defs.thing(20_001).expect("the number is placed");
        assert_eq!(made.name, "Player Sprite", "the header name");
        assert_eq!(made.sprite, "", "nothing to copy the sprite from");
        assert_eq!(made.radius, 20, "the Width still applies");
        assert_eq!(
            made.category, "Custom (DeHackEd)",
            "the synthesized category"
        );
    }

    #[test]
    fn a_moved_number_without_any_definition_or_name_warns() {
        let mut defs = defs();
        apply("Thing 1\nID # = 20001\n", Game::Doom2, &mut defs).expect("the table is valid");
        assert!(defs.thing(20_001).is_none(), "nothing can be copied");
        assert_eq!(defs.warnings.len(), 1, "one warning: {:?}", defs.warnings);
    }

    #[test]
    fn an_index_outside_the_table_and_bad_values_warn() {
        let mut defs = defs();
        let text = "Thing 999\nID # = 5\nThing 0\nID # = 5\nThing 12\nID # = wide\nWidth = \
                    huge\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        assert_eq!(
            defs.warnings.len(),
            4,
            "every problem warns: {:?}",
            defs.warnings
        );
        assert!(defs.thing(3001).is_some(), "the Imp is untouched");
        assert_eq!(
            defs.thing(3001).expect("imp").source,
            "builtin",
            "still builtin"
        );
    }

    #[test]
    fn unknown_keys_and_other_blocks_are_ignored() {
        let mut defs = defs();
        let text = "Misc\nID # = 20000\nThing 12\nBits = anything\nHealth = 100\n";
        apply(text, Game::Doom2, &mut defs).expect("the table is valid");
        assert!(defs.thing(3001).is_some(), "the Imp keeps its number");
        assert_eq!(
            defs.thing(3001).expect("imp").source,
            "builtin",
            "ignored keys change nothing"
        );
        assert!(defs.warnings.is_empty(), "no warnings: {:?}", defs.warnings);
    }
}
