//! The shared text-lump editor behind `edit_decorate` and `edit_zscript`.
//!
//! [`apply`] runs exactly one [`TextOp`] over a lump text and reports a
//! [`TextEditOutcome`]: the new text, its line count, the 1-based range of
//! changed lines, a numbered excerpt around that range and warnings about
//! unbalanced braces. The text is treated as UTF-8 lines: `\r\n` files keep
//! their endings, everything else round-trips as `\n`. No MCP, no session,
//! no filesystem here.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::CyberError;

/// Lines of context the excerpt shows above and below the changed range.
const CONTEXT_LINES: usize = 2;

/// Most excerpt body lines before the rest is counted in a tail note.
const EXCERPT_MAX_LINES: usize = 40;

/// The one text operation a call applies, tagged by its `op` field.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(super) enum TextOp {
    /// Replace the whole text with `text` as given.
    ReplaceAll {
        /// The new full text.
        text: String,
    },
    /// Add `text` at the end, inserting a newline separator first when
    /// the current text does not end with one.
    Append {
        /// The text to add.
        text: String,
    },
    /// Replace literal, case-sensitive occurrences of `find`.
    FindReplace {
        /// The text to look for; empty is `invalid_argument`.
        find: String,
        /// The replacement text.
        replace: String,
        /// Replace every occurrence, not just the first; default `false`.
        #[serde(default)]
        all: bool,
    },
    /// Insert the payload's lines before the 1-based line `line`;
    /// `line = line count + 1` appends at the end.
    InsertLines {
        /// The 1-based line to insert before.
        line: usize,
        /// The text to insert.
        text: String,
    },
    /// Swap the inclusive 1-based line range `start..=end` for the
    /// payload's lines.
    ReplaceLines {
        /// The first line to replace (1-based).
        start: usize,
        /// The last line to replace (1-based, inclusive).
        end: usize,
        /// The replacement text.
        text: String,
    },
    /// Remove the inclusive 1-based line range `start..=end`.
    DeleteLines {
        /// The first line to delete (1-based).
        start: usize,
        /// The last line to delete (1-based, inclusive).
        end: usize,
    },
}

impl TextOp {
    /// Whether this operation can create a missing lump: it brings its own
    /// full text instead of editing lines of an existing one.
    pub(super) const fn is_wholesale(&self) -> bool {
        matches!(self, Self::ReplaceAll { .. } | Self::Append { .. })
    }
}

/// What one [`apply`] call produced.
#[derive(Debug)]
pub(super) struct TextEditOutcome {
    /// The resulting full text.
    pub(super) text: String,
    /// Total lines of the resulting text.
    pub(super) lines: usize,
    /// The 1-based inclusive range of changed lines, `(0, 0)` when the
    /// text did not change.
    pub(super) changed_range: (usize, usize),
    /// The changed lines with [`CONTEXT_LINES`] lines of context, each
    /// prefixed by its 1-based number.
    pub(super) excerpt: String,
    /// Non-fatal notes about the result (unbalanced braces).
    pub(super) warnings: Vec<String>,
}

/// Apply one text operation to `text`, reporting the new text and what it
/// changed. The input is never mutated and nothing is written anywhere.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty `find`, an out-of-range
/// line number or `start > end`; [`CyberError::NotFound`] when
/// `find_replace` matches nowhere.
pub(super) fn apply(text: &str, op: &TextOp) -> Result<TextEditOutcome, CyberError> {
    let old = split_lines(text);
    let file = next_text(text, &old, op)?;
    let new = split_lines(&file);
    let reterminated = text.ends_with('\n') != file.ends_with('\n');
    let changed_range = diff_range(&old, &new, reterminated);
    let excerpt = make_excerpt(&new, changed_range);
    let lines = new.len();
    let warnings = brace_warnings(&file);
    Ok(TextEditOutcome {
        text: file,
        lines,
        changed_range,
        excerpt,
        warnings,
    })
}

/// The full text after `op`, over the base `text` and its line model.
fn next_text(text: &str, old: &[String], op: &TextOp) -> Result<String, CyberError> {
    match op.clone() {
        TextOp::ReplaceAll { text: body } => Ok(body),
        TextOp::Append { text: body } => Ok(append_text(text, &body)),
        TextOp::FindReplace { find, replace, all } => replace_found(text, &find, &replace, all),
        TextOp::InsertLines { line, text: body } => Ok(render(&insert_at(old, line, &body)?, text)),
        TextOp::ReplaceLines {
            start,
            end,
            text: body,
        } => Ok(render(&replace_range(old, start, end, &body)?, text)),
        TextOp::DeleteLines { start, end } => Ok(render(&delete_range(old, start, end)?, text)),
    }
}

/// The file's lines for the editor model: one entry per line, the trailing
/// newline being a terminator rather than a line of its own.
fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// The file text for the model `lines`: joined with the base's dominant
/// line ending and closed by its trailing newline when any line remains.
fn render(lines: &[String], base: &str) -> String {
    let newline = if base.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = lines.join(newline);
    if base.ends_with('\n') && !out.is_empty() {
        out.push_str(newline);
    }
    out
}

/// `base` with `body` added at the end, a newline separator inserted when
/// the base is non-empty and does not end on one.
fn append_text(base: &str, body: &str) -> String {
    let mut out = base.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(if base.contains("\r\n") { "\r\n" } else { "\n" });
    }
    out.push_str(body);
    out
}

/// `text` with literal, case-sensitive `find` occurrences replaced.
fn replace_found(text: &str, find: &str, replace: &str, all: bool) -> Result<String, CyberError> {
    if find.is_empty() {
        return Err(CyberError::InvalidArgument(
            "find_replace needs a non-empty find string: pass the exact text to look for"
                .to_owned(),
        ));
    }
    if !text.contains(find) {
        return Err(CyberError::NotFound(format!(
            "find_replace matched {find:?} nowhere in the text (the search is literal and \
             case-sensitive): read the current text with get_lump format=text"
        )));
    }
    let limit = if all { usize::MAX } else { 1 };
    Ok(text.replacen(find, replace, limit))
}

/// The payload text as model lines: a trailing newline terminates the last
/// line, it does not add an empty one.
fn body_lines(body: &str) -> Vec<String> {
    body.lines().map(str::to_owned).collect()
}

/// `lines` with the payload's lines inserted before the 1-based line
/// `line` (`lines.len() + 1` appends at the end).
fn insert_at(lines: &[String], line: usize, body: &str) -> Result<Vec<String>, CyberError> {
    let last = lines.len().saturating_add(1);
    if line == 0 || line > last {
        return Err(bad_position("insert_lines", line, lines.len()));
    }
    let head = line.saturating_sub(1);
    let mut out: Vec<String> = Vec::new();
    out.extend(lines.iter().take(head).cloned());
    out.extend(body_lines(body));
    out.extend(lines.iter().skip(head).cloned());
    Ok(out)
}

/// `lines` with `start..=end` (1-based, inclusive) swapped for the
/// payload's lines.
fn replace_range(
    lines: &[String],
    start: usize,
    end: usize,
    body: &str,
) -> Result<Vec<String>, CyberError> {
    let (head, tail) = checked_bounds("replace_lines", start, end, lines.len())?;
    let mut out: Vec<String> = Vec::new();
    out.extend(lines.iter().take(head).cloned());
    out.extend(body_lines(body));
    out.extend(lines.iter().skip(tail).cloned());
    Ok(out)
}

/// `lines` without `start..=end` (1-based, inclusive).
fn delete_range(lines: &[String], start: usize, end: usize) -> Result<Vec<String>, CyberError> {
    let (head, tail) = checked_bounds("delete_lines", start, end, lines.len())?;
    Ok(lines
        .iter()
        .take(head)
        .chain(lines.iter().skip(tail))
        .cloned()
        .collect())
}

/// The 0-based half-open bounds of the 1-based inclusive `start..=end`.
fn checked_bounds(
    op: &str,
    start: usize,
    end: usize,
    count: usize,
) -> Result<(usize, usize), CyberError> {
    if start == 0 || end < start || end > count {
        return Err(bad_range(op, start, end, count));
    }
    Ok((start.saturating_sub(1), end))
}

/// The refusal for an insert position outside `1..=count + 1`.
fn bad_position(op: &str, line: usize, count: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{op} line {line} is out of range: the text has {count} line(s), line numbers are \
         1-based and {} appends at the end (read the current text with get_lump format=text)",
        count.saturating_add(1)
    ))
}

/// The refusal for a line range outside `1..=count` or with `start > end`.
fn bad_range(op: &str, start: usize, end: usize, count: usize) -> CyberError {
    CyberError::InvalidArgument(format!(
        "{op} range {start}..={end} is out of range: the text has {count} line(s), line \
         numbers are 1-based inclusive and start must not exceed end (read the current text \
         with get_lump format=text)"
    ))
}

/// The 1-based inclusive range of lines `new` changed relative to `old`,
/// `(0, 0)` when the texts are line-identical and end the same way (a
/// trailing-newline change alone counts as a change to the last line).
fn diff_range(old: &[String], new: &[String], reterminated: bool) -> (usize, usize) {
    let prefix = old
        .iter()
        .zip(new.iter())
        .take_while(|pair| pair.0 == pair.1)
        .count();
    let tail_old = old.len().saturating_sub(prefix);
    let tail_new = new.len().saturating_sub(prefix);
    let suffix = old
        .iter()
        .skip(prefix)
        .rev()
        .zip(new.iter().skip(prefix).rev())
        .take_while(|pair| pair.0 == pair.1)
        .count();
    let changed = tail_old.max(tail_new).saturating_sub(suffix);
    if changed > 0 {
        return (prefix.saturating_add(1), prefix.saturating_add(changed));
    }
    if reterminated && !new.is_empty() {
        return (new.len(), new.len());
    }
    (0, 0)
}

/// The excerpt of the changed lines with context, each prefixed
/// `N: ` by its 1-based number, capped at [`EXCERPT_MAX_LINES`] body lines.
fn make_excerpt(new: &[String], changed_range: (usize, usize)) -> String {
    let (first, last) = changed_range;
    if first == 0 {
        return "(no changes)".to_owned();
    }
    if new.is_empty() {
        return "(the text is now empty)".to_owned();
    }
    let start = first.saturating_sub(CONTEXT_LINES).max(1).min(new.len());
    let end = last.saturating_add(CONTEXT_LINES).min(new.len());
    let shown = end.saturating_sub(start).saturating_add(1);
    let stop = shown.min(EXCERPT_MAX_LINES);
    let numbered: Vec<String> = new
        .iter()
        .enumerate()
        .skip(start.saturating_sub(1))
        .take(stop)
        .map(|(index, line)| format!("{}: {line}", index.saturating_add(1)))
        .collect();
    let mut out = numbered.join("\n");
    if stop < shown {
        let rest = shown.saturating_sub(stop);
        let note = format!("... ({rest} more lines)");
        out.push('\n');
        out.push_str(&note);
    }
    out
}

/// The warnings about `text`: braces that do not balance outside strings
/// and `//` / `/* */` comments.
fn brace_warnings(text: &str) -> Vec<String> {
    let (opens, closes) = count_braces(text);
    if opens == closes {
        return Vec::new();
    }
    vec![format!(
        "unbalanced braces: {opens} '{{' vs {closes} '}}' outside strings and comments"
    )]
}

/// The `{` and `}` counts of `text`, skipping quoted strings and comments.
fn count_braces(text: &str) -> (usize, usize) {
    let mut scan = BraceScan::default();
    for ch in text.chars() {
        scan.step(ch);
    }
    (scan.opens, scan.closes)
}

/// What lexical region of the text the [`BraceScan`] cursor is inside. The
/// `Slash` and `Star` states remember a possible comment start.
#[derive(Clone, Copy, Default)]
enum ScanState {
    /// Plain code: braces count here.
    #[default]
    Code,
    /// A `/` was the last code character.
    Slash,
    /// Inside a `"..."` string.
    Text,
    /// Inside a `//` comment.
    LineComment,
    /// Inside a `/* */` comment.
    BlockComment,
    /// A `*` was the last comment character.
    Star,
}

/// The running brace counter and its lexical state machine.
#[derive(Default)]
struct BraceScan {
    /// The region the cursor sits in.
    state: ScanState,
    /// `{` counted outside strings and comments.
    opens: usize,
    /// `}` counted outside strings and comments.
    closes: usize,
    /// A backslash was the previous character inside a string.
    escaped: bool,
}

impl BraceScan {
    /// Advances the cursor by one character.
    const fn step(&mut self, ch: char) {
        match self.state {
            ScanState::Code => self.code(ch),
            ScanState::Slash => self.slash(ch),
            ScanState::Text => self.quoted(ch),
            ScanState::LineComment => self.line_comment(ch),
            ScanState::BlockComment => self.block_comment(ch),
            ScanState::Star => self.star(ch),
        }
    }

    /// Consumes one character of plain code.
    const fn code(&mut self, ch: char) {
        match ch {
            '/' => self.state = ScanState::Slash,
            '"' => self.state = ScanState::Text,
            '{' => self.opens = self.opens.saturating_add(1),
            '}' => self.closes = self.closes.saturating_add(1),
            _ => {}
        }
    }

    /// Consumes the character after a `/` in code.
    const fn slash(&mut self, ch: char) {
        if ch == '/' {
            self.state = ScanState::LineComment;
            return;
        }
        if ch == '*' {
            self.state = ScanState::BlockComment;
            return;
        }
        self.state = ScanState::Code;
        self.code(ch);
    }

    /// Consumes one character inside a `"..."` string.
    const fn quoted(&mut self, ch: char) {
        if self.escaped {
            self.escaped = false;
            return;
        }
        if ch == '\\' {
            self.escaped = true;
            return;
        }
        if ch == '"' {
            self.state = ScanState::Code;
        }
    }

    /// Consumes one character inside a `//` comment.
    const fn line_comment(&mut self, ch: char) {
        if ch == '\n' {
            self.state = ScanState::Code;
        }
    }

    /// Consumes one character inside a `/* */` comment.
    const fn block_comment(&mut self, ch: char) {
        if ch == '*' {
            self.state = ScanState::Star;
        }
    }

    /// Consumes the character after a `*` inside a block comment.
    const fn star(&mut self, ch: char) {
        if ch == '/' {
            self.state = ScanState::Code;
            return;
        }
        if ch != '*' {
            self.state = ScanState::BlockComment;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TextOp, apply};

    /// The outcome of one accepted operation.
    fn done(text: &str, op: &TextOp) -> super::TextEditOutcome {
        apply(text, op).expect("the operation applies")
    }

    /// The error code of one refused operation.
    fn refused(text: &str, op: &TextOp) -> String {
        apply(text, op)
            .expect_err("the operation is refused")
            .code()
            .to_owned()
    }

    #[test]
    fn replace_all_returns_the_new_text_and_its_whole_range() {
        let found = done(
            "old stuff\n",
            &TextOp::ReplaceAll {
                text: "a\nb\n".to_owned(),
            },
        );
        assert_eq!(found.text, "a\nb\n", "the payload as given");
        assert_eq!(found.lines, 2, "two lines");
        assert_eq!(found.changed_range, (1, 2), "everything changed");
        assert_eq!(found.excerpt, "1: a\n2: b", "numbered body");
        assert!(found.warnings.is_empty(), "no braces at all");
    }

    #[test]
    fn replace_all_onto_nothing_reports_every_line() {
        let found = done(
            "",
            &TextOp::ReplaceAll {
                text: "actor A { }\n".to_owned(),
            },
        );
        assert_eq!(found.lines, 1, "one line");
        assert_eq!(found.changed_range, (1, 1), "the new line");
        assert!(found.warnings.is_empty(), "balanced");
    }

    #[test]
    fn an_identical_replace_reports_no_changes() {
        let found = done(
            "a\nb\n",
            &TextOp::ReplaceAll {
                text: "a\nb\n".to_owned(),
            },
        );
        assert_eq!(found.changed_range, (0, 0), "nothing differs");
        assert_eq!(found.excerpt, "(no changes)", "the empty-edit note");
    }

    #[test]
    fn append_adds_a_separator_only_when_the_base_lacks_one() {
        let glued = done(
            "a\n",
            &TextOp::Append {
                text: "b\n".to_owned(),
            },
        );
        assert_eq!(glued.text, "a\nb\n", "the base ended on a newline");
        assert_eq!(glued.changed_range, (2, 2), "line 2 is new");
        let separated = done(
            "a",
            &TextOp::Append {
                text: "b".to_owned(),
            },
        );
        assert_eq!(separated.text, "a\nb", "the separator was inserted");
        let onto_empty = done(
            "",
            &TextOp::Append {
                text: "x".to_owned(),
            },
        );
        assert_eq!(onto_empty.text, "x", "nothing to separate from");
        assert_eq!(onto_empty.changed_range, (1, 1), "line 1 is new");
    }

    #[test]
    fn append_inserts_a_crlf_separator_for_a_crlf_base() {
        let found = done(
            "a\r\nb",
            &TextOp::Append {
                text: "c".to_owned(),
            },
        );
        assert_eq!(found.text, "a\r\nb\r\nc", "the file stays CRLF");
    }

    #[test]
    fn line_ops_keep_the_crlf_endings_and_the_trailing_newline() {
        let inserted = done(
            "actor A {\n}\r\n",
            &TextOp::InsertLines {
                line: 2,
                text: "radius 30".to_owned(),
            },
        );
        assert_eq!(
            inserted.text, "actor A {\r\nradius 30\r\n}\r\n",
            "the dominant CRLF won and the file keeps a final newline"
        );
        let deleted = done("a\r\nb\r\nc\r\n", &TextOp::DeleteLines { start: 2, end: 2 });
        assert_eq!(deleted.text, "a\r\nc\r\n", "the survivor lines keep CRLF");
        assert_eq!(deleted.lines, 2, "two lines left");
    }

    #[test]
    fn find_replace_edits_the_first_or_every_occurrence() {
        let once = done(
            "x x x\n",
            &TextOp::FindReplace {
                find: "x".to_owned(),
                replace: "y".to_owned(),
                all: false,
            },
        );
        assert_eq!(once.text, "y x x\n", "only the first");
        assert_eq!(once.changed_range, (1, 1), "one line");
        let every = done(
            "x x x\n",
            &TextOp::FindReplace {
                find: "x".to_owned(),
                replace: "y".to_owned(),
                all: true,
            },
        );
        assert_eq!(every.text, "y y y\n", "all three");
        let across = done(
            "a\nb\n",
            &TextOp::FindReplace {
                find: "a\nb".to_owned(),
                replace: "c\nd".to_owned(),
                all: false,
            },
        );
        assert_eq!(across.text, "c\nd\n", "multi-line find works");
    }

    #[test]
    fn find_replace_refuses_nothing_to_find_and_an_empty_needle() {
        let op = TextOp::FindReplace {
            find: "zzz".to_owned(),
            replace: "x".to_owned(),
            all: false,
        };
        assert_eq!(refused("aaa\n", &op), "not_found", "no match code");
        let empty = TextOp::FindReplace {
            find: String::new(),
            replace: "x".to_owned(),
            all: false,
        };
        assert_eq!(refused("aaa\n", &empty), "invalid_argument", "empty find");
    }

    #[test]
    fn insert_at_the_top_and_after_the_last_line() {
        let top = done(
            "a\nb\n",
            &TextOp::InsertLines {
                line: 1,
                text: "z".to_owned(),
            },
        );
        assert_eq!(top.text, "z\na\nb\n", "inserted as the new line 1");
        assert_eq!(top.changed_range, (1, 1), "one new line");
        let bottom = done(
            "a\nb\n",
            &TextOp::InsertLines {
                line: 3,
                text: "c\nd\n".to_owned(),
            },
        );
        assert_eq!(bottom.text, "a\nb\nc\nd\n", "appended at the end");
        assert_eq!(bottom.changed_range, (3, 4), "the two inserted lines");
        assert_eq!(
            bottom.excerpt, "1: a\n2: b\n3: c\n4: d",
            "context reaches the top"
        );
    }

    #[test]
    fn insert_into_an_empty_text_and_a_no_payload_insert() {
        let first = done(
            "",
            &TextOp::InsertLines {
                line: 1,
                text: "a".to_owned(),
            },
        );
        assert_eq!(first.text, "a", "the only line");
        assert_eq!(first.lines, 1, "one line");
        let noop = done(
            "a\n",
            &TextOp::InsertLines {
                line: 1,
                text: String::new(),
            },
        );
        assert_eq!(noop.text, "a\n", "nothing was inserted");
        assert_eq!(noop.changed_range, (0, 0), "and nothing changed");
    }

    #[test]
    fn replace_lines_spans_and_refuses() {
        let mid = done(
            "a\nb\nc\n",
            &TextOp::ReplaceLines {
                start: 2,
                end: 2,
                text: "x\ny".to_owned(),
            },
        );
        assert_eq!(mid.text, "a\nx\ny\nc\n", "one line became two");
        assert_eq!(mid.changed_range, (2, 3), "the two new lines");
        let last = done(
            "a\nb",
            &TextOp::ReplaceLines {
                start: 2,
                end: 2,
                text: "z".to_owned(),
            },
        );
        assert_eq!(last.text, "a\nz", "the final line without a newline");
    }

    #[test]
    fn delete_lines_takes_the_last_line_and_a_middle_span() {
        let tail = done("a\nb\nc\n", &TextOp::DeleteLines { start: 3, end: 3 });
        assert_eq!(tail.text, "a\nb\n", "the third line is gone");
        assert_eq!(tail.changed_range, (3, 3), "reported at its old position");
        let middle = done("a\nb\nc\nd\n", &TextOp::DeleteLines { start: 2, end: 3 });
        assert_eq!(middle.text, "a\nd\n", "both middle lines are gone");
        assert_eq!(middle.excerpt, "1: a\n2: d", "the neighbours frame it");
    }

    #[test]
    fn out_of_range_and_inverted_lines_are_invalid_argument() {
        let zero = TextOp::InsertLines {
            line: 0,
            text: "x".to_owned(),
        };
        assert_eq!(refused("a\n", &zero), "invalid_argument", "line 0");
        let beyond = TextOp::InsertLines {
            line: 3,
            text: "x".to_owned(),
        };
        assert_eq!(refused("a\n", &beyond), "invalid_argument", "past len + 1");
        let inverted = TextOp::ReplaceLines {
            start: 2,
            end: 1,
            text: "x".to_owned(),
        };
        assert_eq!(
            refused("a\nb\n", &inverted),
            "invalid_argument",
            "start > end"
        );
        let past_end = TextOp::DeleteLines { start: 1, end: 3 };
        assert_eq!(
            refused("a\nb\n", &past_end),
            "invalid_argument",
            "end > len"
        );
        let on_empty = TextOp::DeleteLines { start: 1, end: 1 };
        assert_eq!(
            refused("", &on_empty),
            "invalid_argument",
            "no lines at all"
        );
    }

    #[test]
    fn brace_warnings_ignore_strings_and_comments() {
        let clean = done(
            "actor A {\n  // }\n  /* { { */\n  msg = \"}{{\"\n}\n",
            &TextOp::Append {
                text: String::new(),
            },
        );
        assert!(clean.warnings.is_empty(), "balanced: {:?}", clean.warnings);
        let open = done(
            "actor A {\n",
            &TextOp::Append {
                text: "radius 30\n".to_owned(),
            },
        );
        assert_eq!(open.warnings.len(), 1, "one note: {:?}", open.warnings);
        assert!(
            open.warnings
                .first()
                .is_some_and(|warning| warning.contains("1 '{' vs 0 '}'")),
            "the counts: {:?}",
            open.warnings
        );
        let closed = done(
            "}\n",
            &TextOp::Append {
                text: String::new(),
            },
        );
        assert!(
            closed.warnings.first().is_some_and(|w| w.contains('0')),
            "a stray close warns: {:?}",
            closed.warnings
        );
    }

    #[test]
    fn the_excerpt_carries_two_lines_of_context_and_numbers() {
        let base = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        let found = done(
            base,
            &TextOp::ReplaceLines {
                start: 5,
                end: 5,
                text: "five".to_owned(),
            },
        );
        assert_eq!(found.changed_range, (5, 5), "the middle line");
        assert_eq!(
            found.excerpt, "3: 3\n4: 4\n5: five\n6: 6\n7: 7",
            "lines 3..=7 numbered"
        );
    }

    #[test]
    fn a_wide_excerpt_is_capped_with_a_tail_note() {
        let body = "x\n".repeat(60);
        let found = done("", &TextOp::ReplaceAll { text: body });
        let shown = found
            .excerpt
            .lines()
            .filter(|line| line.contains(": "))
            .count();
        assert_eq!(shown, 40, "the cap");
        assert!(
            found.excerpt.contains("... (20 more lines)"),
            "the tail note: {}",
            found.excerpt
        );
    }

    #[test]
    fn deleting_every_line_reports_the_empty_text_note() {
        let found = done("a\nb\n", &TextOp::DeleteLines { start: 1, end: 2 });
        assert_eq!(found.text, "", "all gone");
        assert_eq!(found.lines, 0, "no lines");
        assert_eq!(found.excerpt, "(the text is now empty)", "the note");
    }
}
