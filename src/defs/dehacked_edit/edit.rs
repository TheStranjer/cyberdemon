//! The line-addressed editor: `parse_spec` reads the block or section an
//! edit names and `set_patch`/`remove_patch` rewrite exactly the lines of
//! it they must touch.
//!
//! `region_of` matches a `Target` against the `Patch` `parse` produced and
//! answers the `Region` of lines it covers, so a set replaces one
//! `Key = Value` line, inserts an absent key after the region's last one,
//! or appends a fresh region after a blank line, and a removal takes a key
//! line or a whole region with the blank line behind it. A `Text` block's
//! body is raw text, so key edits inside it are refused.

use super::reader::{kind_of, parse, section_name, split_word};
use super::{Field, KIND_TEXT, KINDS, Patch};
use crate::error::CyberError;

/// The block or section an edit names.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Target {
    /// A `<Kind> [number]` block; the trimmed spec itself is the header
    /// line written when the block has to be created.
    Block {
        /// The canonical kind word.
        kind: &'static str,
        /// The number to match, `None` for the first block of the kind.
        number: Option<i64>,
    },
    /// A `[NAME]` BEX section.
    Section {
        /// The canonical section name.
        name: &'static str,
    },
}

/// Parses an edit target: `"Thing 12"`, `"thing 12 (evil)"`, `"Misc"`,
/// `"Text 6 0"` (matched on the first length) or a BEX section such as
/// `"[STRINGS]"`; every kind and section name is case-insensitive.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `spec` names no known block or
/// section, or when the number does not parse.
pub(crate) fn parse_spec(spec: &str) -> Result<Target, CyberError> {
    let trimmed = spec.trim();
    if let Some(name) = section_name(trimmed) {
        return Ok(Target::Section { name });
    }
    let (word, rest) = split_word(trimmed);
    let Some(kind) = kind_of(word) else {
        return Err(CyberError::InvalidArgument(format!(
            "block {spec:?} is not a DeHackEd target: use \"Kind [number]\" with one of {kinds} \
             or a BEX section like \"[STRINGS]\"",
            kinds = KINDS.join(", ")
        )));
    };
    let head = rest
        .split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("")
        .trim();
    let number = match head {
        "" => None,
        text => Some(text.parse::<i64>().map_err(|error| {
            CyberError::InvalidArgument(format!(
                "block {spec:?} carries {text:?}, which is not a number: {error}"
            ))
        })?),
    };
    Ok(Target::Block { kind, number })
}

/// Refuses a key that could not stand on a `Key = Value` line.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty key or one containing a
/// newline or `=`.
pub(crate) fn check_key(key: &str) -> Result<(), CyberError> {
    if key.trim().is_empty() {
        return Err(CyberError::InvalidArgument(
            "set and remove entries need a non-empty key: pass key (or, for remove, omit it to \
             drop the whole block)"
                .to_owned(),
        ));
    }
    if key.contains(['\n', '\r']) || key.contains('=') {
        return Err(CyberError::InvalidArgument(format!(
            "key {key:?} cannot stand on a single Key = Value line: keys hold no newlines or ="
        )));
    }
    Ok(())
}

/// Refuses a value that could not stand on one line.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `value` contains a newline.
pub(crate) fn check_value(value: &str) -> Result<(), CyberError> {
    if value.contains(['\n', '\r']) {
        return Err(CyberError::InvalidArgument(
            "values must not contain newlines: a DeHackEd key line is one line; split the text \
             across keys or replace the whole lump with the text mode"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The editable shape of one block or section, in the same line numbers as
/// the text it was parsed from.
struct Region<'patch> {
    /// The header line's index.
    header: usize,
    /// The exclusive line the region ends before.
    end: usize,
    /// The key lines, in file order.
    keys: Vec<&'patch Field>,
    /// Whether the region is a `Text` block: its body is raw text, so key
    /// lines cannot be addressed inside it.
    opaque: bool,
}

/// The region `target` selects in `patch` (the first match), `None` when
/// no block or section answers to it.
fn region_of(patch: &Patch, target: Target) -> Option<Region<'_>> {
    match target {
        Target::Block { kind, number, .. } => block_region(patch, kind, number),
        Target::Section { name } => section_region(patch, name),
    }
}

/// The first block of `kind`, with `number` when one is given.
fn block_region<'patch>(
    patch: &'patch Patch,
    kind: &str,
    number: Option<i64>,
) -> Option<Region<'patch>> {
    let found = patch.blocks.iter().find(|entry| {
        entry.kind == kind && number.is_none_or(|wanted| entry.number == Some(wanted))
    })?;
    Some(Region {
        header: found.line,
        end: found.end,
        keys: found.fields.iter().collect(),
        opaque: kind == KIND_TEXT,
    })
}

/// The first section named `name`.
fn section_region<'patch>(patch: &'patch Patch, name: &str) -> Option<Region<'patch>> {
    let found = patch.sections.iter().find(|entry| entry.name == name)?;
    Some(Region {
        header: found.line,
        end: found.end,
        keys: found.entries.iter().collect(),
        opaque: false,
    })
}

/// Rewrites `text` so that `key` in the region named by `spec` holds
/// `value`: an existing `Key = Value` line is replaced (case-insensitive
/// match, only that line changes), an absent key is inserted after the
/// region's last key, and a missing region is appended after a blank line.
///
/// # Errors
///
/// [`parse_spec`], [`check_key`] and [`check_value`] refusals; nothing
/// changes when one reports an error.
pub(crate) fn set_patch(
    text: &str,
    spec: &str,
    key: &str,
    value: &str,
) -> Result<(String, usize), CyberError> {
    let target = parse_spec(spec)?;
    check_key(key)?;
    check_value(value)?;
    let wanted = key.trim();
    let assign = format!("{wanted} = {value}");
    let mut lines = split_lines(text);
    let parsed = parse(text);
    let region = region_of(&parsed, target);
    if region.as_ref().is_some_and(|found| found.opaque) {
        return Err(opaque_refusal(spec));
    }
    let changed = match region {
        Some(found) => set_in_region(&mut lines, &found, wanted, &assign),
        None => create_region(&mut lines, target, spec.trim(), &assign),
    };
    Ok((join_lines(&lines), changed))
}

/// The refusal for addressing key lines inside a `Text` block body.
fn opaque_refusal(spec: &str) -> CyberError {
    CyberError::InvalidArgument(format!(
        "block {spec:?} is a Text block: its body is raw text, not Key = Value lines; replace \
         the whole lump with the text or source mode instead"
    ))
}

/// Updates `assign` into `region`'s lines: replace the matching key line,
/// else insert after the region's last key, and report the one changed line.
fn set_in_region(lines: &mut Vec<String>, region: &Region<'_>, key: &str, assign: &str) -> usize {
    if let Some(field) = region
        .keys
        .iter()
        .find(|entry| entry.key.eq_ignore_ascii_case(key))
    {
        if let Some(line) = lines.get_mut(field.line) {
            assign.clone_into(line);
        }
        return 1_usize;
    }
    let at = region
        .keys
        .last()
        .map_or(region.end, |entry| entry.line.saturating_add(1));
    lines.insert(at, assign.to_owned());
    1_usize
}

/// Appends a fresh region (header plus the one key) after a blank line and
/// reports the number of lines added. A block is created with the caller's
/// trimmed spec as its header; a section with its canonical bracket name.
fn create_region(lines: &mut Vec<String>, target: Target, spec: &str, assign: &str) -> usize {
    let mut added = 0_usize;
    if lines.last().is_some_and(|last| !last.is_empty()) {
        lines.push(String::new());
        added = added.saturating_add(1);
    }
    let header = match target {
        Target::Block { .. } => spec.to_owned(),
        Target::Section { name } => format!("[{name}]"),
    };
    lines.push(header);
    lines.push(assign.to_owned());
    added.saturating_add(2_usize)
}

/// Removes from `text` either one key line (`Some(key)`, case-insensitive)
/// or the whole region named by `spec`, header through its last line plus
/// one following blank line.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming what is missing when no block or section
/// answers to `spec` or the key is absent, [`parse_spec`]'s refusal
/// otherwise; nothing changes on error.
pub(crate) fn remove_patch(
    text: &str,
    spec: &str,
    key: Option<&str>,
) -> Result<(String, usize), CyberError> {
    let target = parse_spec(spec)?;
    let mut lines = split_lines(text);
    let parsed = parse(text);
    let region = region_of(&parsed, target).ok_or_else(|| missing("block", spec))?;
    let changed = match key {
        Some(..) if region.opaque => return Err(opaque_refusal(spec)),
        Some(wanted) => remove_key(&mut lines, &region, wanted)?,
        None => remove_whole(&mut lines, &region),
    };
    Ok((join_lines(&lines), changed))
}

/// The `not_found` text pointing at the way forward.
fn missing(what: &str, name: &str) -> CyberError {
    CyberError::NotFound(format!(
        "{what} {name:?} not found: nothing changed (set creates a missing block; get_dehacked \
         lists what the patch holds)"
    ))
}

/// Deletes one key line, reporting the refusal for an absent key.
///
/// # Errors
///
/// [`CyberError::NotFound`] when the region holds no such key.
fn remove_key(
    lines: &mut Vec<String>,
    region: &Region<'_>,
    key: &str,
) -> Result<usize, CyberError> {
    let field = region
        .keys
        .iter()
        .find(|entry| entry.key.eq_ignore_ascii_case(key))
        .ok_or_else(|| missing("key", key))?;
    Ok(remove_line(lines, field.line))
}

/// Deletes the single line at `at`, reporting 1 when it was there.
fn remove_line(lines: &mut Vec<String>, at: usize) -> usize {
    if lines.get(at).is_some() {
        lines.remove(at);
        return 1_usize;
    }
    0_usize
}

/// Deletes the whole region and one blank line following it.
fn remove_whole(lines: &mut Vec<String>, region: &Region<'_>) -> usize {
    let stop = if lines.get(region.end).is_some_and(String::is_empty) {
        region.end.saturating_add(1)
    } else {
        region.end
    };
    lines.drain(region.header..stop).count()
}

/// The lines of a patch text, ending normalised.
pub(super) fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// The patch text the lines spell: `\n` between and after every line; no
/// lines spell no bytes.
pub(super) fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}
