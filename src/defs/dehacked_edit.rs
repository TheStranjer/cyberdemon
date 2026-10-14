//! Line-oriented DeHackEd/BEX patch parsing and editing for the scripts
//! tools.
//!
//! The parser keeps the file's line structure: every block, section and key
//! line remembers its line, so `set_patch` and `remove_patch` rewrite
//! exactly the lines they must and leave the untouched ones as they were
//! (line endings normalise to `\n` and a non-empty text keeps a trailing
//! newline). Parsing is tolerant: unknown headers, `Include` lines and
//! malformed numbers become `Patch::warnings` entries, never errors; only
//! the edit functions fail, with `CyberError::InvalidArgument` for a
//! refused target, key or value and `CyberError::NotFound` for a missing
//! block or key.

use crate::error::CyberError;

/// The block kinds a `<Kind> [number]` header may name, canonical spelling.
const KINDS: [&str; 10] = [
    "Thing", "Frame", "Pointer", "Sound", "Ammo", "Weapon", "Misc", "Cheat", "Sprite", "Text",
];

/// The BEX sections a `[NAME]` header may open, canonical spelling.
const SECTIONS: [&str; 7] = [
    "CODEPTR", "STRINGS", "PARS", "SPRITES", "SOUNDS", "MUSIC", "HELPER",
];

/// The `Patch File for DeHackEd v3.0` title line, lowercased for matching.
const TITLE_LINE: &str = "patch file for dehacked";

/// The out-of-block key naming the `Doom` executable version.
const KEY_DOOM_VERSION: &str = "doom version";

/// The out-of-block key naming the DeHackEd patch format level.
const KEY_PATCH_FORMAT: &str = "patch format";

/// The header word of an include line: reported, never followed.
const WORD_INCLUDE: &str = "include";

/// The block kind whose header carries two lengths and an opaque body.
const KIND_TEXT: &str = "Text";

/// The text a created patch starts with: the canonical DeHackEd 3.0 header
/// plus a blank line, so appended blocks stand separated.
pub(crate) const NEW_PATCH_HEADER: &str =
    "Patch File for DeHackEd v3.0\nDoom version = 21\nPatch format = 6\n\n";

/// One `Key = Value` line of a block or section.
pub(crate) struct Field {
    /// The key as written (outer space trimmed).
    pub(crate) key: String,
    /// The value as written (outer space trimmed).
    pub(crate) value: String,
    /// The 0-based line this key stands on.
    pub(crate) line: usize,
}

/// One `<Kind> <number> [(name)]` block with its key lines.
pub(crate) struct Block {
    /// The canonical kind word (`Thing`, `Frame`, ...).
    pub(crate) kind: &'static str,
    /// The header number, `None` when the header carries none; a `Text`
    /// block stores its first length here.
    pub(crate) number: Option<i64>,
    /// The parenthesised name, when the header gives one; for a `Text`
    /// block, the second length as text.
    pub(crate) name: Option<String>,
    /// The 0-based line of the header.
    pub(crate) line: usize,
    /// The exclusive line the block ends before: the blank line, the next
    /// header or the file end.
    pub(crate) end: usize,
    /// The `Key = Value` lines, in file order.
    pub(crate) fields: Vec<Field>,
}

/// One `[NAME]` BEX section with its entry lines.
pub(crate) struct Section {
    /// The canonical section name (`STRINGS`, `CODEPTR`, ...).
    pub(crate) name: &'static str,
    /// The 0-based line of the header.
    pub(crate) line: usize,
    /// The exclusive line the section ends before: the next header or the
    /// file end (blank lines do not close a section).
    pub(crate) end: usize,
    /// The `KEY = VALUE` lines, in file order.
    pub(crate) entries: Vec<Field>,
}

/// A parsed DeHackEd/BEX patch text.
#[derive(Default)]
pub(crate) struct Patch {
    /// The `Doom version` header value, when present and numeric.
    pub(crate) doom_version: Option<i64>,
    /// The `Patch format` header value, when present and numeric.
    pub(crate) format: Option<i64>,
    /// The blocks, in file order.
    pub(crate) blocks: Vec<Block>,
    /// The BEX sections, in file order.
    pub(crate) sections: Vec<Section>,
    /// Notes about lines the tolerant parser refused to read (1-based).
    pub(crate) warnings: Vec<String>,
}

/// The block or section whose lines are being read.
enum Opened {
    /// A `<Kind> [number]` block.
    Block(Block),
    /// A `[NAME]` BEX section.
    Section(Section),
}

/// The parser's running state over one patch text.
#[derive(Default)]
struct Reading {
    /// The patch under construction.
    patch: Patch,
    /// The region collecting the current lines.
    open: Option<Opened>,
    /// Body characters still to swallow for the open `Text` block.
    pending: usize,
}

/// Parses `text` into its header values, blocks, BEX sections and warnings.
/// This never fails; unreadable lines land in [`Patch::warnings`].
pub(crate) fn parse(text: &str) -> Patch {
    let mut reading = Reading::default();
    let mut total = 0_usize;
    for (index, line) in text.lines().enumerate() {
        total = index.saturating_add(1);
        if reading.pending > 0 {
            reading.swallow(index, line);
            continue;
        }
        reading.line(index, line);
    }
    reading.finish(total);
    reading.patch
}

impl Reading {
    /// One ordinary line: blank closes a block, `#` is a comment, `=` makes
    /// a key and anything else is a header candidate.
    fn line(&mut self, index: usize, raw: &str) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            self.blank(index);
            return;
        }
        if trimmed.starts_with('#') {
            return;
        }
        if trimmed.contains('=') {
            self.key_line(index, trimmed);
            return;
        }
        self.header_line(index, trimmed);
    }

    /// Swallows one opaque `Text` body line, closing the block when the
    /// declared lengths are consumed.
    fn swallow(&mut self, index: usize, line: &str) {
        let eaten = line.chars().count().saturating_add(1_usize);
        self.pending = self.pending.saturating_sub(eaten);
        if self.pending == 0_usize {
            self.close_current(index.saturating_add(1));
        }
    }

    /// A blank line ends the open block (sections run past blanks).
    fn blank(&mut self, index: usize) {
        if matches!(self.open, Some(Opened::Block(..))) {
            self.close_current(index);
        }
    }

    /// One `Key = Value` line: into the open region, or a header value.
    fn key_line(&mut self, index: usize, trimmed: &str) {
        let (raw_key, raw_value) = trimmed
            .split_once('=')
            .map_or((trimmed, ""), |(head, tail)| (head, tail));
        let field = Field {
            key: raw_key.trim().to_owned(),
            value: raw_value.trim().to_owned(),
            line: index,
        };
        let back = match self.open.take() {
            Some(Opened::Block(mut block)) => {
                block.fields.push(field);
                Opened::Block(block)
            }
            Some(Opened::Section(mut section)) => {
                section.entries.push(field);
                Opened::Section(section)
            }
            None => {
                self.stray_key(&field);
                return;
            }
        };
        self.open = Some(back);
    }

    /// A key line above the first header: the two known header values, or
    /// a warning.
    fn stray_key(&mut self, field: &Field) {
        let lowered = field.key.to_ascii_lowercase();
        if lowered == KEY_DOOM_VERSION || lowered == KEY_PATCH_FORMAT {
            self.patch_header(&lowered, field.line, &field.value);
            return;
        }
        self.warn(
            field.line,
            &format!(
                "key {} outside any block is kept but not reported",
                field.key
            ),
        );
    }

    /// Records the numeric `Doom version` or `Patch format` header.
    fn patch_header(&mut self, lowered: &str, line: usize, value: &str) {
        let Ok(number) = value.parse::<i64>() else {
            self.warn(line, &format!("{lowered} value {value:?} is not a number"));
            return;
        };
        if lowered == KEY_DOOM_VERSION {
            self.patch.doom_version = Some(number);
            return;
        }
        self.patch.format = Some(number);
    }

    /// A line without `=`: a section header, the patch title, an include,
    /// a block header or an unrecognised boundary.
    fn header_line(&mut self, index: usize, trimmed: &str) {
        self.close_current(index);
        if let Some(name) = section_name(trimmed) {
            self.open = Some(Opened::Section(Section {
                name,
                line: index,
                end: index,
                entries: Vec::new(),
            }));
            return;
        }
        if trimmed.to_ascii_lowercase().starts_with(TITLE_LINE) {
            return;
        }
        let (word, rest) = split_word(trimmed);
        if word.eq_ignore_ascii_case(WORD_INCLUDE) {
            self.warn(
                index,
                &format!("Include {rest:?} is reported, not followed"),
            );
            return;
        }
        self.open_block(index, word, rest);
    }

    /// Opens the block a recognised header word names, warning about a
    /// malformed header number.
    fn open_block(&mut self, index: usize, word: &str, rest: &str) {
        let Some(kind) = kind_of(word) else {
            self.warn(
                index,
                &format!("unrecognised header word {word:?} ends the current region"),
            );
            return;
        };
        let (number, name, body) = parts_of(kind, rest, index, &mut self.patch);
        self.pending = body;
        self.open = Some(Opened::Block(Block {
            kind,
            number,
            name,
            line: index,
            end: index,
            fields: Vec::new(),
        }));
    }

    /// Moves the open region into its list, recording where it ended.
    fn close_current(&mut self, end: usize) {
        let Some(opened) = self.open.take() else {
            return;
        };
        match opened {
            Opened::Block(mut block) => {
                block.end = end;
                self.patch.blocks.push(block);
            }
            Opened::Section(mut section) => {
                section.end = end;
                self.patch.sections.push(section);
            }
        }
    }

    /// Ends the pass: an unfinished region stops at `total`.
    fn finish(&mut self, total: usize) {
        self.pending = 0;
        self.close_current(total);
    }

    /// Pushes a warning naming the 1-based line.
    fn warn(&mut self, line: usize, message: &str) {
        self.patch
            .warnings
            .push(format!("line {}: {message}", line.saturating_add(1)));
    }
}

/// The kind word and the rest of a header line.
fn split_word(line: &str) -> (&str, &str) {
    line.split_once(char::is_whitespace)
        .map_or((line, ""), |(head, tail)| (head, tail))
}

/// The canonical kind a header word names, case-insensitively.
fn kind_of(word: &str) -> Option<&'static str> {
    KINDS
        .iter()
        .find(|kind| kind.eq_ignore_ascii_case(word))
        .copied()
}

/// The canonical section name a `[NAME]` line names, `None` for any other
/// line or an unknown name.
fn section_name(trimmed: &str) -> Option<&'static str> {
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    SECTIONS
        .iter()
        .find(|name| name.eq_ignore_ascii_case(inner.trim()))
        .copied()
}

/// The header parts of a block: number, name and the body characters a
/// `Text` header declares (0 for every other kind).
fn parts_of(
    kind: &str,
    rest: &str,
    index: usize,
    patch: &mut Patch,
) -> (Option<i64>, Option<String>, usize) {
    if kind == KIND_TEXT {
        return text_parts(rest, index, &mut patch.warnings);
    }
    let (number, name) = header_parts(rest, index, &mut patch.warnings);
    (number, name, 0_usize)
}

/// The `<number> [(name)]` tail of an ordinary block header.
fn header_parts(
    rest: &str,
    index: usize,
    warnings: &mut Vec<String>,
) -> (Option<i64>, Option<String>) {
    let (number_part, tail) = rest
        .split_once('(')
        .map_or((rest, None), |(before, after)| {
            (before, after.strip_suffix(')').map(str::trim))
        });
    let named = tail.map(str::to_owned);
    let head = number_part.trim();
    if head.is_empty() {
        return (None, named);
    }
    if let Ok(number) = head.parse::<i64>() {
        return (Some(number), named);
    }
    warnings.push(format!(
        "line {}: header number {head:?} is not numeric",
        index.saturating_add(1)
    ));
    (None, named)
}

/// The `Text <len1> <len2>` header: len1 as the number, len2 as the name,
/// and the body length to swallow.
fn text_parts(
    rest: &str,
    index: usize,
    warnings: &mut Vec<String>,
) -> (Option<i64>, Option<String>, usize) {
    let mut words = rest.split_whitespace();
    let (first, second) = (words.next(), words.next());
    let lengths = match (first, second) {
        (Some(one), Some(two)) => (one.parse::<i64>().ok(), two.parse::<i64>().ok()),
        _ => (None, None),
    };
    if let (Some(one), Some(two)) = lengths {
        let body = usize::try_from(one.saturating_add(two)).unwrap_or(0_usize);
        return (Some(one), Some(two.to_string()), body);
    }
    warnings.push(format!(
        "line {}: Text header {rest:?} carries two numbers",
        index.saturating_add(1)
    ));
    (None, None, 0_usize)
}

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
fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// The patch text the lines spell: `\n` between and after every line; no
/// lines spell no bytes.
fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::{
        KIND_TEXT, NEW_PATCH_HEADER, Target, check_key, check_value, join_lines, parse, parse_spec,
        remove_patch, set_patch, split_lines,
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
            NEW_PATCH_HEADER,
            "Patch File for DeHackEd v3.0\nDoom version = 21\nPatch format = 6\n\n",
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
}
