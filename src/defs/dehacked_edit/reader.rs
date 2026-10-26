//! The tolerant reader: `parse` folds a patch text into the `Patch` the
//! parent declares, one line at a time.
//!
//! `Reading` carries the region a header line opened, the `Block` or
//! `Section` being filled and the body bytes an unfinished `Text` block
//! still swallows: DeHackEd lengths count the original text's BYTES, the
//! exact newline bytes included. The helpers here split a header line into
//! its kind word, number and parenthesised name, and a `Text` header into
//! its two lengths. Every line the shapes cannot hold becomes a
//! `Patch::warnings` entry, so `parse` never fails.

use super::{
    Block, Field, KEY_DOOM_VERSION, KEY_PATCH_FORMAT, KIND_TEXT, KINDS, Opened, Patch, SECTIONS,
    Section, TITLE_LINE, WORD_INCLUDE,
};

/// The parser's running state over one patch text.
#[derive(Default)]
struct Reading {
    /// The patch under construction.
    patch: Patch,
    /// The region collecting the current lines.
    open: Option<Opened>,
    /// Body bytes still to swallow for the open `Text` block, newline
    /// bytes included.
    pending: usize,
}

/// Parses `text` into its header values, blocks, BEX sections and warnings.
/// This never fails; unreadable lines land in [`Patch::warnings`].
pub(crate) fn parse(text: &str) -> Patch {
    let mut reading = Reading::default();
    let mut total = 0_usize;
    for (index, chunk) in text.split_inclusive('\n').enumerate() {
        total = index.saturating_add(1);
        if reading.pending > 0 {
            reading.swallow(index, chunk);
            continue;
        }
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
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

    /// Swallows one opaque `Text` body line: its bytes with the exact
    /// newline bytes that end it, closing the block when the declared
    /// lengths are consumed.
    fn swallow(&mut self, index: usize, chunk: &str) {
        self.pending = self.pending.saturating_sub(chunk.len());
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
pub(super) fn split_word(line: &str) -> (&str, &str) {
    line.split_once(char::is_whitespace)
        .map_or((line, ""), |(head, tail)| (head, tail))
}

/// The canonical kind a header word names, case-insensitively.
pub(super) fn kind_of(word: &str) -> Option<&'static str> {
    KINDS
        .iter()
        .find(|kind| kind.eq_ignore_ascii_case(word))
        .copied()
}

/// The canonical section name a `[NAME]` line names, `None` for any other
/// line or an unknown name.
pub(super) fn section_name(trimmed: &str) -> Option<&'static str> {
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    SECTIONS
        .iter()
        .find(|name| name.eq_ignore_ascii_case(inner.trim()))
        .copied()
}

/// The header parts of a block: number, name and the body bytes a
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
