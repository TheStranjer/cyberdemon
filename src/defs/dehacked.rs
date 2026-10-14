//! DeHackEd thing overrides: the `Thing <n>` block subset and the mobj map.
//!
//! A DeHackEd patch edits an internal mobj table indexed from 1 in Doom II
//! order. `apply` reads the `Thing <n> (<name>)` blocks of one DEHACKED
//! lump text and moves the built-in definitions: `ID #` sets a new
//! doomednum (`-1` makes the mobj unplaceable), `Width` replaces the
//! blocking radius (written in 16.16 fixed point), and every other key is
//! ignored. Unparseable parts become warnings, never errors; the only
//! error is a broken embedded mobj table, which is a build bug.

use std::sync::LazyLock;

use crate::defs::registry::{Definitions, game_label};
use crate::defs::table::ThingDef;
use crate::error::CyberError;
use crate::fixed::FRACUNIT;
use crate::wad::game::Game;

/// The DeHackEd mobj index to doomednum map, compiled into the binary from
/// `data/defs/deh_mobj.tsv` (`index<TAB>doomednum`, 1-based, `-1` for a
/// mobj the editors give no number).
const DEH_MOBJ_TSV: &str = include_str!("../../data/defs/deh_mobj.tsv");

/// The [`ThingDef::source`] label of rows moved or edited by DEHACKED.
const SOURCE_DEHACKED: &str = "dehacked";

/// The category of synthesized rows whose mobj has no built-in definition.
const CATEGORY_DEHACKED: &str = "Custom (DeHackEd)";

/// The radius of a synthesized row whose patch names none.
const DEFAULT_RADIUS: u32 = 20;

/// The block header word.
const HEADER_THING: &str = "thing";

/// The key that moves a thing's doomednum.
const KEY_ID: &str = "id #";

/// The key that replaces a thing's blocking radius (16.16 fixed point).
const KEY_WIDTH: &str = "width";

/// The tab-separated header [`DEH_MOBJ_TSV`] must carry.
const MOBJ_HEADER: &str = "index\tdoomednum";

/// The number of fields one mobj row must carry.
const MOBJ_FIELDS: usize = 2;

/// The parsed [`DEH_MOBJ_TSV`], produced on first use. A parse failure is
/// kept as an error string and reported as [`CyberError::Internal`], never
/// a panic: broken embedded data is a build bug.
static DEH_MOBJS: LazyLock<Result<Vec<i64>, String>> =
    LazyLock::new(|| parse_mobj_table(DEH_MOBJ_TSV));

/// One parsed `Thing <n>` block.
struct ThingBlock {
    /// The 1-based mobj index from the header.
    number: i64,
    /// The parenthesised display name, when the header gives one.
    name: Option<String>,
    /// The `key = value` lines, keys lowercased and both sides trimmed.
    keys: Vec<(String, String)>,
}

impl ThingBlock {
    /// The trimmed value of the lowercased `key`, when the block sets it.
    fn value(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|pair| pair.0 == key)
            .map(|pair| pair.1.as_str())
    }
}

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
    let mobjs = DEH_MOBJS.as_deref().map_err(|reason| {
        CyberError::Internal(format!(
            "embedded dehacked mobj table is malformed (this is a server bug): {reason}"
        ))
    })?;
    let (blocks, warnings) = thing_blocks(text);
    defs.warnings.extend(warnings);
    for block in &blocks {
        if let Some(edit) = build_edit(block, mobjs, &mut defs.warnings) {
            apply_edit(&edit, game, defs);
        }
    }
    Ok(())
}

/// The tab-separated mobj rows in order: `doomednums.get(index - 1)` is the
/// doomednum DeHackEd thing `index` patches.
///
/// # Errors
///
/// A `String` naming the header problem or the first malformed or
/// out-of-order row.
fn parse_mobj_table(text: &str) -> Result<Vec<i64>, String> {
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Err("mobj table is empty: no header line".to_owned());
    };
    if header != MOBJ_HEADER {
        return Err(format!(
            "bad mobj table header {header:?}: expected {MOBJ_HEADER:?}"
        ));
    }
    let mut doomednums: Vec<i64> = Vec::new();
    for (offset, line) in lines.filter(|line| !line.is_empty()).enumerate() {
        let number = offset.saturating_add(2);
        let fields: Vec<&str> = line.split('\t').collect();
        let Ok([index_text, doomednum_text]) = <[&str; MOBJ_FIELDS]>::try_from(fields) else {
            return Err(format!(
                "mobj table line {number}: expected {MOBJ_FIELDS} tab-separated fields, found \
                 {line:?}"
            ));
        };
        let expected = i64::try_from(doomednums.len())
            .ok()
            .and_then(|seen| seen.checked_add(1))
            .ok_or_else(|| format!("mobj table line {number}: too many rows"))?;
        let Ok(index) = index_text.parse::<i64>() else {
            return Err(format!(
                "mobj table line {number}: index {index_text:?} is not a number"
            ));
        };
        if index != expected {
            return Err(format!(
                "mobj table line {number}: index {index} is not the expected {expected}"
            ));
        }
        let Ok(doomednum) = doomednum_text.parse::<i64>() else {
            return Err(format!(
                "mobj table line {number}: doomednum {doomednum_text:?} is not a number"
            ));
        };
        doomednums.push(doomednum);
    }
    Ok(doomednums)
}

/// Splits `text` into its `Thing <n>` blocks: keys run until the next
/// header line (without `=`) or a blank line. Other block headers end the
/// current block and contribute nothing.
fn thing_blocks(text: &str) -> (Vec<ThingBlock>, Vec<String>) {
    let mut blocks: Vec<ThingBlock> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut current: Option<ThingBlock> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            finish(&mut blocks, &mut current);
            continue;
        }
        if !trimmed.contains('=') {
            finish(&mut blocks, &mut current);
            current = header_of(trimmed, &mut warnings);
            continue;
        }
        let Some(block) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        block
            .keys
            .push((key.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    finish(&mut blocks, &mut current);
    (blocks, warnings)
}

/// Moves the block under construction into the finished list, if any.
fn finish(blocks: &mut Vec<ThingBlock>, current: &mut Option<ThingBlock>) {
    if let Some(block) = current.take() {
        blocks.push(block);
    }
}

/// Whether the first word of `line` names a `Thing` block header.
fn starts_with_thing(line: &str) -> bool {
    line.split_whitespace()
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case(HEADER_THING))
}

/// The block header when this non-key line opens a `Thing` block; other
/// headers (and their blocks) contribute nothing.
fn header_of(line: &str, warnings: &mut Vec<String>) -> Option<ThingBlock> {
    if !starts_with_thing(line) {
        return None;
    }
    parse_header(line, warnings)
}

/// Parses the `Thing <n> (<name>)` header line, warning about one that
/// names no numeric index.
fn parse_header(line: &str, warnings: &mut Vec<String>) -> Option<ThingBlock> {
    let (_, rest) = line
        .split_once(char::is_whitespace)
        .map_or((line, ""), |(head, tail)| (head, tail));
    let (index_part, name) = rest.split_once('(').map_or((rest, None), |(head, tail)| {
        (head, tail.strip_suffix(')').map(str::trim))
    });
    let Ok(number) = index_part.trim().parse::<i64>() else {
        warnings.push(format!(
            "dehacked: thing header {line:?} names no numeric mobj index"
        ));
        return None;
    };
    Some(ThingBlock {
        number,
        name: name.map(str::to_owned),
        keys: Vec::new(),
    })
}

/// Resolves the block's mobj index against `mobjs` and reads the keys, or
/// warns and returns `None` for an index outside the table.
fn build_edit(block: &ThingBlock, mobjs: &[i64], warnings: &mut Vec<String>) -> Option<Edit> {
    let position = usize::try_from(block.number)
        .ok()
        .filter(|index| *index > 0_usize)
        .and_then(|index| index.checked_sub(1));
    let mapped = position.and_then(|offset| mobjs.get(offset).copied());
    let Some(original) = mapped else {
        warnings.push(format!(
            "dehacked: thing index {} is outside the mobj table of {} entries",
            block.number,
            mobjs.len()
        ));
        return None;
    };
    Some(Edit {
        index: block.number,
        original,
        name: block.name.clone(),
        id: block.value(KEY_ID).map(str::to_owned),
        radius: block
            .value(KEY_WIDTH)
            .and_then(|text| parse_width(text, warnings)),
    })
}

/// The radius in map units from a `Width` written in 16.16 fixed point.
fn parse_width(text: &str, warnings: &mut Vec<String>) -> Option<u32> {
    let Ok(fixed) = text.parse::<i64>() else {
        warnings.push(format!(
            "dehacked: Width {text:?} is not a 16.16 fixed-point number"
        ));
        return None;
    };
    let radius = fixed
        .checked_div(i64::from(FRACUNIT))
        .and_then(|units| u32::try_from(units).ok());
    if radius.is_none() {
        warnings.push(format!(
            "dehacked: Width {text:?} is not a positive 16.16 fixed-point radius"
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
            "dehacked: thing {} has ID # {id_text:?}, which is not a number",
            edit.index
        ));
        return;
    };
    if number < 0_i64 {
        if let Ok(original) = u32::try_from(edit.original) {
            remove_kind(defs, original);
        }
        return;
    }
    match u32::try_from(number) {
        Ok(kind) => place(edit, kind, game, defs),
        Err(_) => defs.warnings.push(format!(
            "dehacked: thing {} has ID # {id_text:?}, which is above the thing number range",
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
        remove_kind(defs, original);
    }
    replace_kind(defs, def);
}

/// A row for a mobj whose doomednum has no built-in definition: possible
/// only when the block header names one; otherwise this warns.
fn synthesize(edit: &Edit, kind: u32, game: Game, defs: &mut Definitions) {
    let Some(name) = edit.name.as_deref() else {
        defs.warnings.push(format!(
            "dehacked: thing {} has no built-in row to move to {kind} and the header names none",
            edit.index
        ));
        return;
    };
    replace_kind(
        defs,
        ThingDef {
            kind,
            name: name.to_owned(),
            sprite: String::new(),
            frame: 'A',
            radius: edit.radius.unwrap_or(DEFAULT_RADIUS),
            category: CATEGORY_DEHACKED.to_owned(),
            hanging: false,
            game: game_label(game).to_owned(),
            source: SOURCE_DEHACKED.to_owned(),
        },
    );
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
    let Some(def) = defs.things.iter_mut().find(|row| row.kind == kind) else {
        return;
    };
    def.radius = radius;
    SOURCE_DEHACKED.clone_into(&mut def.source);
}

/// Drops the row of thing type `kind`, if this table has one.
fn remove_kind(defs: &mut Definitions, kind: u32) {
    defs.things.retain(|row| row.kind != kind);
}

/// Inserts `def`, dropping any row that already uses its type number.
fn replace_kind(defs: &mut Definitions, def: ThingDef) {
    defs.things.retain(|row| row.kind != def.kind);
    defs.things.push(def);
}

#[cfg(test)]
mod tests {
    use crate::defs::dehacked::{DEH_MOBJ_TSV, apply, parse_mobj_table, thing_blocks};
    use crate::defs::registry::definitions_for;
    use crate::wad::game::Game;

    /// A fresh DOOM II table to patch.
    fn defs() -> crate::defs::Definitions {
        definitions_for(Game::Doom2).expect("the embedded tables are valid")
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
    fn blocks_run_until_the_next_header_or_a_blank_line() {
        let text = "Thing 12 (Imp Thing)\nID # = 20000\nWidth = 655360\n\nMisc\nID # = 5\n";
        let (blocks, warnings) = thing_blocks(text);
        assert!(warnings.is_empty(), "the text parses: {warnings:?}");
        assert_eq!(blocks.len(), 1, "only the Thing block is kept");
        let block = blocks.first().expect("one block");
        assert_eq!(block.number, 12, "the header index");
        assert_eq!(
            block.name.as_deref(),
            Some("Imp Thing"),
            "the parenthesised name"
        );
        assert_eq!(block.value("id #"), Some("20000"), "the ID # value");
        assert_eq!(block.value("width"), Some("655360"), "the Width value");
        assert_eq!(block.value("bits"), None, "an absent key stays absent");
    }

    #[test]
    fn a_non_numeric_thing_header_warns_and_cannot_collect_keys() {
        let text = "Thing xx\nID # = 20000\nThing 12\nID # = 20001\n";
        let (blocks, warnings) = thing_blocks(text);
        assert_eq!(warnings.len(), 1, "one header warning: {warnings:?}");
        assert_eq!(blocks.len(), 1, "only the second block survives");
        assert_eq!(blocks.first().expect("block").number, 12, "its index");
    }

    #[test]
    fn moving_the_imp_gives_the_new_number_and_frees_the_old() {
        let mut defs = defs();
        apply("Thing 12\nID # = 20000\n", Game::Doom2, &mut defs).expect("the table is valid");
        assert!(defs.thing(3001).is_none(), "3001 no longer spawns the Imp");
        let moved = defs.thing(20000).expect("20000 is the Imp now");
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
        let moved = defs.thing(20000).expect("20000 exists");
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
        let made = defs.thing(20001).expect("the number is placed");
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
        assert!(defs.thing(20001).is_none(), "nothing can be copied");
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
