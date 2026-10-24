//! The reader and the editor over one representative patch: the header
//! values, blocks, sections and warnings [`parse`] folds out of it, the
//! opaque `Text` body, [`set_patch`]'s and [`remove_patch`]'s line rewrites,
//! insertions, creations and removals, the [`parse_spec`] and key/value
//! refusals, the canonical created-patch header and the line join rules.
use super::edit::{Target, join_lines, split_lines};
use super::{
    KIND_TEXT, NEW_PATCH_HEADER, check_key, check_value, parse, parse_spec, remove_patch, set_patch,
};
use crate::error::CyberError;

/// A representative patch: title, headers, a comment, an include,
/// Thing/Frame/Misc/Text blocks and two BEX sections.
const SAMPLE: &str = "Patch File for DeHackEd v3.0\n\
                      Doom version = 21\n\
                      Patch format = 6\n\
                      # written by hand\n\
                      Include other.deh\n\
                      \n\
                      Thing 12 (Evil Imp)\n\
                      ID # = 20000\n\
                      gib = 200\n\
                      \n\
                      Frame 0\n\
                      sprite number = 1\n\
                      \n\
                      Misc\n\
                      total ammo = 200\n\
                      \n\
                      Text 6 13\n\
                      Troop\n\
                      Baron of Hell\n\
                      \n\
                      [CODEPTR]\n\
                      FRAMEZ = A_Light0\n\
                      \n\
                      [STRINGS]\n\
                      HUDSTR_PAUSE = PAUSED\n";

/// The error code of a [`CyberError`], for compact asserts.
fn code(error: &CyberError) -> &str {
    error.code()
}

#[test]
fn the_sample_patch_parses_into_blocks_sections_and_headers() {
    let patch = parse(SAMPLE);
    assert_eq!(patch.doom_version, Some(21), "the doom version header");
    assert_eq!(patch.format, Some(6), "the format header");
    let kinds: Vec<&str> = patch.blocks.iter().map(|block| block.kind).collect();
    assert_eq!(
        kinds,
        vec!["Thing", "Frame", "Misc", "Text"],
        "the block order"
    );
    let sections: Vec<&str> = patch.sections.iter().map(|section| section.name).collect();
    assert_eq!(sections, vec!["CODEPTR", "STRINGS"], "both BEX sections");
    assert_eq!(
        patch.warnings.len(),
        1,
        "only the include notes: {:?}",
        patch.warnings
    );
    assert!(
        patch
            .warnings
            .first()
            .expect("one warning")
            .contains("line 5"),
        "the include warns with its 1-based line: {:?}",
        patch.warnings
    );
}

#[test]
fn a_thing_block_keeps_its_name_fields_and_lines() {
    let patch = parse(SAMPLE);
    let thing = patch.blocks.first().expect("the Thing block");
    assert_eq!(thing.number, Some(12), "the header index");
    assert_eq!(
        thing.name.as_deref(),
        Some("Evil Imp"),
        "the parenthesised name"
    );
    assert_eq!(thing.line, 6, "the header line, 0-based");
    assert_eq!(thing.end, 9, "ends at the blank line");
    let keys: Vec<&str> = thing
        .fields
        .iter()
        .map(|field| field.key.as_str())
        .collect();
    assert_eq!(keys, vec!["ID #", "gib"], "keys as written");
    assert_eq!(
        thing.fields.first().expect("ID #").value,
        "20000",
        "value trimmed"
    );
    assert_eq!(
        thing.fields.first().expect("ID #").line,
        7,
        "the key's line"
    );
}

#[test]
fn a_text_block_body_is_opaque_until_consumed() {
    let patch = parse(SAMPLE);
    let text = patch
        .blocks
        .iter()
        .find(|block| block.kind == KIND_TEXT)
        .expect("the Text block");
    assert_eq!(text.number, Some(6), "len1 as the number");
    assert_eq!(text.name.as_deref(), Some("13"), "len2 as the name");
    assert!(text.fields.is_empty(), "the body is not parsed as keys");
    assert_eq!(text.end, 19, "ends after the two body lines");
    assert!(
        patch
            .blocks
            .iter()
            .all(|block| block.kind != "Frame" || block.line == 10),
        "the body lines opened no blocks"
    );
}

#[test]
fn misc_and_bex_entries_read_their_fields() {
    let patch = parse(SAMPLE);
    let misc = patch
        .blocks
        .iter()
        .find(|b| b.kind == "Misc")
        .expect("the Misc block");
    assert_eq!(misc.number, None, "Misc carries no number");
    assert_eq!(
        misc.fields.first().expect("one key").key,
        "total ammo",
        "spaced keys survive"
    );
    let strings = patch
        .sections
        .iter()
        .find(|section| section.name == "STRINGS")
        .expect("the strings section");
    let entry = strings.entries.first().expect("one entry");
    assert_eq!(entry.key, "HUDSTR_PAUSE", "entry key");
    assert_eq!(entry.value, "PAUSED", "entry value");
    assert_eq!(strings.line, 23, "the section header line");
    assert_eq!(strings.end, 25, "runs to the file end");
}

#[test]
fn an_unknown_header_word_and_a_bad_block_number_warn() {
    let patch = parse("Thing xx\nID # = 1\nWat? lines\n\nFrame 5\nsprite number = 2\n");
    assert_eq!(
        patch.warnings.len(),
        2,
        "both problems warn: {:?}",
        patch.warnings
    );
    assert_eq!(patch.blocks.len(), 2, "Thing and Frame survive");
    assert_eq!(
        patch.blocks.first().expect("thing").number,
        None,
        "the number is dropped"
    );
    assert_eq!(
        patch.blocks.last().expect("frame").number,
        Some(5),
        "the next parses"
    );
}

#[test]
fn a_stray_key_outside_every_block_is_reported() {
    let patch = parse("Doom version = wide\nHeight = 9\nMisc\nX = 1\n");
    assert_eq!(
        patch.warnings.len(),
        2,
        "bad header and stray key: {:?}",
        patch.warnings
    );
    assert_eq!(
        patch.doom_version, None,
        "a non-numeric header value does not count"
    );
}

#[test]
fn set_replaces_only_the_matching_key_line() {
    let (text, changed) = set_patch(SAMPLE, "Thing 12", "id #", "3001").expect("a set");
    assert_eq!(changed, 1, "one line changed");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[7], "id # = 3001", "the one rewritten line");
    assert_eq!(lines[6], "Thing 12 (Evil Imp)", "the header keeps its text");
    assert_eq!(lines[8], "gib = 200", "the sibling keeps its spacing");
    assert_eq!(
        lines[0], "Patch File for DeHackEd v3.0",
        "the title is untouched"
    );
    assert!(
        text.ends_with("HUDSTR_PAUSE = PAUSED\n"),
        "the tail is unchanged"
    );
}

#[test]
fn set_inserts_an_absent_key_at_the_end_of_the_region() {
    let (text, changed) = set_patch(SAMPLE, "Misc", "foo", "bar").expect("a set");
    assert_eq!(changed, 1, "one line inserted");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[15], "foo = bar", "just after Misc's last key");
    assert_eq!(lines[16], "", "the blank line still separates");
    let (bex, _) = set_patch(SAMPLE, "[strings]", "HUDSTR_NEW", "hi").expect("a bex set");
    assert!(
        bex.ends_with("HUDSTR_NEW = hi\n"),
        "the entry joins the section tail"
    );
}

#[test]
fn set_creates_a_missing_block_after_a_blank_line() {
    let (text, changed) =
        set_patch("Misc\nfoo = 1\n", "Weapon 3", "Ammo #1", "40").expect("a create");
    assert_eq!(
        text, "Misc\nfoo = 1\n\nWeapon 3\nAmmo #1 = 40\n",
        "blank, header and key"
    );
    assert_eq!(changed, 3, "three lines added");
    let (created, _) = set_patch("", "Thing 5", "Bits", "8").expect("an empty text");
    assert_eq!(
        created, "Thing 5\nBits = 8\n",
        "no leading blank for an empty patch"
    );
}

#[test]
fn set_matches_blocks_case_insensitively_and_sections_by_name() {
    let (text, _) = set_patch(SAMPLE, "thing 12", "Health", "60").expect("a fold");
    let patched = parse(text.as_str());
    let thing = patched.blocks.first().expect("the Thing block");
    assert_eq!(thing.fields.len(), 3, "the lowercase spec found Thing 12");
    assert_eq!(
        thing.fields.last().expect("new key").value,
        "60",
        "it landed inside"
    );
    let (codeptr, _) = set_patch(SAMPLE, "[CODEPTR]", "FRAMEZ", "A_Light1").expect("a rewrite");
    assert!(
        codeptr.contains("FRAMEZ = A_Light1\n"),
        "an existing entry rewrites"
    );
    assert!(
        !codeptr.contains("FRAMEZ = A_Light0\n"),
        "the old value is gone"
    );
}

#[test]
fn remove_takes_one_key_line_or_the_whole_block() {
    let (text, changed) = remove_patch(SAMPLE, "Thing 12", Some("id #")).expect("a key");
    assert_eq!(changed, 1, "one line went");
    assert!(!text.contains("ID #"), "the line is gone");
    assert!(text.contains("gib = 200"), "the sibling stays");
    let (gone, blocks_removed) = remove_patch(SAMPLE, "Frame 0", None).expect("a block");
    assert_eq!(
        blocks_removed, 3,
        "header, key and the following blank line"
    );
    assert!(!gone.contains("sprite number"), "the whole block went");
    assert!(gone.contains("Thing 12 (Evil Imp)"), "the neighbours stay");
}

#[test]
fn remove_finds_no_missing_block_or_key_and_changes_nothing() {
    let error = remove_patch(SAMPLE, "Weapon 3", None).expect_err("no Weapon block");
    assert_eq!(code(&error), "not_found", "missing block code");
    assert!(
        error.to_string().contains("nothing changed"),
        "the message promises: {error}"
    );
    let key_error = remove_patch(SAMPLE, "Thing 12", Some("nope")).expect_err("no such key");
    assert_eq!(code(&key_error), "not_found", "missing key code");
}

#[test]
fn parse_spec_accepts_the_documented_forms() {
    match parse_spec("thing 12 (Evil Imp)").expect("a thing") {
        Target::Block { kind, number } => {
            assert_eq!(kind, "Thing", "canonical kind");
            assert_eq!(
                number,
                Some(12),
                "the number ignores the parenthesised name"
            );
        }
        Target::Section { .. } => {
            panic!("a Thing spec must not name a section");
        }
    }
    match parse_spec("[strings]").expect("a section") {
        Target::Section { name } => assert_eq!(name, "STRINGS", "canonical name"),
        Target::Block { .. } => panic!("a section spec must not name a block"),
    }
    assert!(
        matches!(
            parse_spec("Misc").expect("bare kind"),
            Target::Block { number: None, .. }
        ),
        "Misc without a number"
    );
    assert!(
        matches!(
            parse_spec("Text 6 0").expect("a text kind"),
            Target::Block {
                number: Some(6),
                ..
            }
        ),
        "Text matches on the first length"
    );
}

#[test]
fn parse_spec_refuses_unknown_kinds_and_bad_numbers() {
    let error = parse_spec("Gong 3").expect_err("no such kind");
    assert_eq!(code(&error), "invalid_argument", "unknown kind code");
    assert!(
        error.to_string().contains("[STRINGS]"),
        "the hint names sections: {error}"
    );
    let number_error = parse_spec("Thing xx").expect_err("not a number");
    assert_eq!(code(&number_error), "invalid_argument", "bad number code");
    assert!(parse_spec("").is_err(), "an empty spec");
    assert!(
        parse_spec("[TELEPORT]").is_err(),
        "an unknown section is refused"
    );
}

#[test]
fn keys_and_values_are_refused_before_any_edit() {
    assert!(check_key("fine key").is_ok(), "spaced keys are legal");
    assert_eq!(
        code(&check_key("").expect_err("empty key")),
        "invalid_argument",
        "empty"
    );
    assert_eq!(
        code(&check_key("a=b").expect_err("equals")),
        "invalid_argument",
        "equals"
    );
    assert_eq!(
        code(&check_key("a\nb").expect_err("newline")),
        "invalid_argument",
        "newline key"
    );
    assert_eq!(
        code(&check_value("line1\nline2").expect_err("newline")),
        "invalid_argument",
        "newline value"
    );
    assert!(check_value("").is_ok(), "an empty value is legal");
    let error = set_patch(SAMPLE, "Thing 12", "ID #", "1\n2").expect_err("newline value");
    assert_eq!(code(&error), "invalid_argument", "set forwards the check");
}

#[test]
fn text_blocks_refuse_key_edits_and_take_whole_block_removal() {
    let error = set_patch(SAMPLE, "Text 6 0", "X", "1").expect_err("opaque body");
    assert_eq!(code(&error), "invalid_argument", "opaque set code");
    assert!(
        error.to_string().contains("source mode"),
        "the message names the way out: {error}"
    );
    let key_error = remove_patch(SAMPLE, "Text 6 0", Some("X")).expect_err("opaque key");
    assert_eq!(code(&key_error), "invalid_argument", "opaque remove code");
    let (text, changed) = remove_patch(SAMPLE, "Text 6 0", None).expect("the whole block");
    assert_eq!(changed, 4, "header, both body lines and the blank line");
    assert!(!text.contains("Baron of Hell"), "the body went with it");
    assert!(text.contains("[CODEPTR]"), "the sections stay");
}

#[test]
fn the_created_patch_header_is_the_canonical_text() {
    assert_eq!(
        NEW_PATCH_HEADER, "Patch File for DeHackEd v3.0\nDoom version = 21\nPatch format = 6\n\n",
        "the documented header"
    );
    let patch = parse(NEW_PATCH_HEADER);
    assert_eq!(patch.doom_version, Some(21), "it parses back");
    assert_eq!(patch.format, Some(6), "both values");
    assert!(
        patch.warnings.is_empty(),
        "a bare header warns about nothing"
    );
}

#[test]
fn lines_join_back_with_one_trailing_newline_and_none_when_empty() {
    assert_eq!(
        join_lines(&split_lines("a\nb")),
        "a\nb\n",
        "the newline is completed"
    );
    assert_eq!(
        join_lines(&split_lines("a\r\nb\n")),
        "a\nb\n",
        "CRLF normalises"
    );
    assert_eq!(join_lines(&[]), "", "no lines, no bytes");
}

#[test]
fn a_section_removed_takes_its_trailing_blank_line_too() {
    let (text, changed) = remove_patch(SAMPLE, "[CODEPTR]", None).expect("a section");
    assert_eq!(changed, 3, "header, entry and the blank line");
    assert!(!text.contains("[CODEPTR]"), "the section is gone");
    assert!(text.contains("[STRINGS]"), "the other section stays");
}
