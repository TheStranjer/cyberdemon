//! The MAPINFO / ZMAPINFO reader (subset): `DoomEdNums` blocks.
//!
//! `doom_ed_nums` lifts the `DoomEdNums { <number> = <ClassName> }`
//! entries out of one MAPINFO-shaped text. `//` and `/* */` comments are
//! dropped (across lines) and quoted strings keep their contents but never
//! their braces; inside a string a backslash escapes the character after
//! it, so `\"` cannot close the string. Other blocks (`GameInfo`, `map`,
//! `addmap`, …) are skipped by brace matching; only the entries of a
//! braced `DoomEdNums` block are returned. Anything unparseable inside the
//! block becomes a warning.

use alloc::collections::VecDeque;

/// The block header word this reader looks for, matched case-insensitively.
const BLOCK_DOOMEDNUMS: &str = "doomednums";

/// One `DoomEdNums` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DoomEdEntry {
    /// The doomednum the editor should offer.
    pub(crate) number: u32,
    /// The class name the entry maps it to, as written.
    pub(crate) class: String,
}

/// Where the line walk stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Outside any block.
    Outside,
    /// Inside a `DoomEdNums { }` block.
    Target,
    /// Inside another block, `n` braces deep.
    Skip(usize),
}

/// The `DoomEdNums` entries of one MAPINFO text, with warnings for lines
/// inside the block that do not parse.
pub(crate) fn doom_ed_nums(text: &str) -> (Vec<DoomEdEntry>, Vec<String>) {
    let mut entries: Vec<DoomEdEntry> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut lines: VecDeque<String> = text.lines().map(str::to_owned).collect();
    let mut mode = Mode::Outside;
    let mut block_comment = false;
    while let Some(raw) = lines.pop_front() {
        let (code, open) = strip_comments(&raw, block_comment);
        block_comment = open;
        if let Mode::Skip(depth) = mode {
            mode = skipped(&code, depth, &mut lines);
            continue;
        }
        if mode == Mode::Outside {
            mode = read_outside(&code, &mut lines);
            continue;
        }
        mode = read_target(&code, &mut lines, &mut entries, &mut warnings);
    }
    (entries, warnings)
}

/// One line outside any block: a `Name {` opens the `DoomEdNums` block or a
/// block to skip; everything else (plain statements, blank) is ignored. The
/// text after the brace, if any, goes back onto the queue.
fn read_outside(line: &str, lines: &mut VecDeque<String>) -> Mode {
    let Some((before, after)) = line.split_once('{') else {
        return Mode::Outside;
    };
    requeue(lines, after);
    let names_block = before
        .split_whitespace()
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case(BLOCK_DOOMEDNUMS));
    if names_block {
        return Mode::Target;
    }
    Mode::Skip(1)
}

/// One line inside a `DoomEdNums` block: entries until the `}` that closes
/// it; what follows the brace goes back onto the queue.
fn read_target(
    line: &str,
    lines: &mut VecDeque<String>,
    entries: &mut Vec<DoomEdEntry>,
    warnings: &mut Vec<String>,
) -> Mode {
    if let Some((head, tail)) = line.split_once('}') {
        parse_entries(head, entries, warnings);
        requeue(lines, tail);
        return Mode::Outside;
    }
    parse_entries(line, entries, warnings);
    Mode::Target
}

/// Advances the brace depth of a skipped block over one line; when a
/// depth-1 block closes mid-line, the text after the brace goes back onto
/// the queue.
fn skipped(line: &str, depth: usize, lines: &mut VecDeque<String>) -> Mode {
    let (opens, closes) = braces(line);
    let left = depth.saturating_add(opens).saturating_sub(closes);
    if left != 0 {
        return Mode::Skip(left);
    }
    if depth == 1 {
        if let Some((_head, tail)) = split_close(line) {
            requeue(lines, &tail);
        }
    }
    Mode::Outside
}

/// The text before and after the first `}` that sits outside quoted
/// spans, when the line holds one.
fn split_close(line: &str) -> Option<(String, String)> {
    let mut state = SplitState::default();
    for ch in line.chars() {
        if let Some(rest) = state.tail.as_mut() {
            rest.push(ch);
            continue;
        }
        take_head_char(ch, &mut state);
    }
    state.tail.map(|rest| (state.head, rest))
}

/// The string state of a character scan over MAPINFO text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Quote {
    /// Outside a quoted string.
    #[default]
    Out,
    /// Inside a quoted string.
    In,
    /// Inside a quoted string, with the next character escaped by the
    /// backslash just seen.
    Escaped,
}

/// Whether the scan sits outside every quoted string.
const fn outside(quote: Quote) -> bool {
    matches!(quote, Quote::Out)
}

/// The string state after one character: a backslash inside a string
/// escapes the character after it and only an unescaped `"` closes it.
const fn next_quote(quote: Quote, ch: char) -> Quote {
    match quote {
        Quote::Escaped => Quote::In,
        Quote::In => match ch {
            '\\' => Quote::Escaped,
            '"' => Quote::Out,
            _ => Quote::In,
        },
        Quote::Out => {
            if ch == '"' {
                Quote::In
            } else {
                Quote::Out
            }
        }
    }
}

/// The split point state while scanning for the closing brace.
#[derive(Default)]
struct SplitState {
    /// Text before the split.
    head: String,
    /// Text after the split brace, once found.
    tail: Option<String>,
    /// The string state of the scan.
    quote: Quote,
}

/// One character before the split point: the first outside-string `}`
/// starts the tail, every other character joins the head.
fn take_head_char(ch: char, state: &mut SplitState) {
    state.quote = next_quote(state.quote, ch);
    if ch == '}' && outside(state.quote) {
        state.tail = Some(String::new());
        return;
    }
    state.head.push(ch);
}

/// Puts non-blank leftovers back at the head of the line queue.
fn requeue(lines: &mut VecDeque<String>, rest: &str) {
    if !rest.trim().is_empty() {
        lines.push_front(rest.to_owned());
    }
}

/// Parses the entry lines of one `DoomEdNums` chunk: `number = Class` with
/// optional quoted names and trailing comma arguments.
fn parse_entries(chunk: &str, entries: &mut Vec<DoomEdEntry>, warnings: &mut Vec<String>) {
    for line in chunk.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        parse_entry(trimmed, entries, warnings);
    }
}

/// Parses one `number = class` entry line, warning about bad halves.
fn parse_entry(line: &str, entries: &mut Vec<DoomEdEntry>, warnings: &mut Vec<String>) {
    let Some((left, right)) = line.split_once('=') else {
        warnings.push(format!("mapinfo: DoomEdNums line {line:?} has no = value"));
        return;
    };
    match (left.trim().parse::<u32>(), class_name(right)) {
        (Ok(number), Some(class)) => entries.push(DoomEdEntry { number, class }),
        (Err(_), _) => {
            warnings.push(format!(
                "mapinfo: DoomEdNums entry {line:?} does not start with a number"
            ));
        }
        (Ok(_), None) => {
            warnings.push(format!("mapinfo: DoomEdNums entry {line:?} names no class"));
        }
    }
}

/// The class name on the right of the `=`: quoted or bare, cut at the
/// first comma or space; `None` when nothing is left.
fn class_name(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix('"') {
        let (name, _closed) = rest.split_once('"')?;
        if name.is_empty() {
            return None;
        }
        return Some(name.to_owned());
    }
    let head = trimmed.split([' ', '\t', ',', ';']).next()?;
    if head.is_empty() {
        return None;
    }
    Some(head.to_owned())
}

/// The `{`/`}` counter of one line, tracking quoted spans.
#[derive(Default)]
struct Counter {
    /// Braces opened outside strings.
    opens: usize,
    /// Braces closed outside strings.
    closes: usize,
    /// The string state of the scan.
    quote: Quote,
}

impl Counter {
    /// Feeds one character of the line; braces inside strings never count.
    const fn take(&mut self, ch: char) {
        let was_outside = outside(self.quote);
        self.quote = next_quote(self.quote, ch);
        if !was_outside {
            return;
        }
        match ch {
            '{' => self.opens = self.opens.saturating_add(1_usize),
            '}' => self.closes = self.closes.saturating_add(1_usize),
            _ => {}
        }
    }
}

/// The count of `{` and `}` outside quoted spans of one line; inside a
/// string a backslash escapes the character after it.
fn braces(line: &str) -> (usize, usize) {
    let mut counter = Counter::default();
    for ch in line.chars() {
        counter.take(ch);
    }
    (counter.opens, counter.closes)
}

/// The line-comment state machine behind [`strip_comments`].
struct Stripper {
    /// The comment-free text so far.
    out: String,
    /// Whether a `/* */` comment is open.
    block: bool,
    /// The string state of the scan.
    quote: Quote,
}

impl Stripper {
    /// Feeds one character; `false` ends the line (a `//` comment).
    fn take<T>(&mut self, ch: char, chars: &mut core::iter::Peekable<T>) -> bool
    where
        T: Iterator<Item = char>,
    {
        if self.block {
            self.block = !(ch == '*' && eat_char(chars, '/'));
            return true;
        }
        match self.quote {
            Quote::Escaped => {
                self.quote = Quote::In;
                self.out.push(ch);
                true
            }
            Quote::In => {
                self.take_in(ch);
                true
            }
            Quote::Out => self.take_out(ch, chars),
        }
    }

    /// One character inside a string: kept as written, with the backslash
    /// and closing-quote transitions read off it.
    fn take_in(&mut self, ch: char) {
        self.out.push(ch);
        self.quote = next_quote(Quote::In, ch);
    }

    /// One character outside comments and strings: quotes open a string,
    /// a `/` may start a comment, everything else is kept.
    fn take_out<T>(&mut self, ch: char, chars: &mut core::iter::Peekable<T>) -> bool
    where
        T: Iterator<Item = char>,
    {
        if ch == '"' {
            self.quote = Quote::In;
        }
        if ch == '/' {
            return self.take_slash(chars);
        }
        self.out.push(ch);
        true
    }

    /// One `/` outside comments and strings: `//` ends the line, `/*`
    /// opens a block comment, anything else is kept.
    fn take_slash<T>(&mut self, chars: &mut core::iter::Peekable<T>) -> bool
    where
        T: Iterator<Item = char>,
    {
        match chars.peek().copied() {
            Some('/') => false,
            Some('*') => {
                chars.next();
                self.block = true;
                self.out.push(' ');
                true
            }
            _ => {
                self.out.push('/');
                true
            }
        }
    }
}

/// Consumes the next character when it equals `want`.
fn eat_char<T>(chars: &mut core::iter::Peekable<T>, want: char) -> bool
where
    T: Iterator<Item = char>,
{
    if chars.peek().copied() != Some(want) {
        return false;
    }
    chars.next();
    true
}

/// The line with `//` and `/* */` comment text removed (strings kept),
/// plus whether a block comment is still open at the line's end.
fn strip_comments(line: &str, block_open: bool) -> (String, bool) {
    let mut stripper = Stripper {
        out: String::new(),
        block: block_open,
        quote: Quote::default(),
    };
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if !stripper.take(ch, &mut chars) {
            break;
        }
    }
    (stripper.out, stripper.block)
}

#[cfg(test)]
mod tests {
    use super::{DoomEdEntry, braces, class_name, doom_ed_nums, strip_comments};

    /// The entries of one text, panicking on warnings.
    fn entries(text: &str) -> Vec<DoomEdEntry> {
        let (found, warnings) = doom_ed_nums(text);
        assert!(warnings.is_empty(), "no warnings: {warnings:?}");
        found
    }

    #[test]
    fn reads_plain_and_quoted_entries_with_arguments() {
        let found =
            entries("DoomEdNums {\n  1234 = Foo\n  1235 = \"Blue Guy\", blue, \"more\"\n}\n");
        assert_eq!(found.len(), 2, "both entries");
        let plain = found.first().expect("first entry");
        assert_eq!(plain.number, 1234, "number");
        assert_eq!(plain.class, "Foo", "bare class name");
        let quoted = found.get(1).expect("second entry");
        assert_eq!(quoted.number, 1235, "number");
        assert_eq!(quoted.class, "Blue Guy", "the quoted name keeps its space");
    }

    #[test]
    fn a_one_line_block_works_and_other_blocks_are_skipped() {
        let text = "GameInfo { sky1 = SKY1 }\naddmap \"odd {\" { title = \"a } b\" }\n\
                    DoomEdNums { 5001 = Baz }\n";
        let found = entries(text);
        assert_eq!(found.len(), 1, "only the DoomEdNums entry");
        assert_eq!(
            found.first().expect("entry").class,
            "Baz",
            "the mapped class"
        );
    }

    #[test]
    fn nested_and_unrelated_blocks_with_comments_never_leak_entries() {
        let text = "map MAP01 {\n // DoomEdNums { 1 = Ghost }\n lookup \"A\" = \"B\"\n skybox = \
                    \"space\" {\n depth = 1\n }\n}\n/* block\ncomment { with braces */\n\
                    DoomEdNums {\n /* skip = Nothing */\n 77 = Real\n}\n";
        let found = entries(text);
        assert_eq!(found.len(), 1, "only the real entry");
        assert_eq!(found.first().expect("entry").number, 77, "its number");
    }

    #[test]
    fn bad_entry_lines_warn_without_stopping_the_block() {
        let (found, warnings) =
            doom_ed_nums("DoomEdNums {\n  x = Foo\n  42 =\n  nothing here\n}\n");
        assert!(found.is_empty(), "no entry parses");
        assert_eq!(warnings.len(), 3, "every bad line warns: {warnings:?}");
    }

    #[test]
    fn a_bare_doomednums_without_a_block_is_ignored() {
        let found = entries("DoomEdNums\n5001 = Foo\n");
        assert!(found.is_empty(), "the braced form is required");
    }

    #[test]
    fn an_escaped_quote_keeps_a_string_open_to_its_real_close() {
        let text = "addmap x {\n note = \"a \\\" b { c\"\n}\nDoomEdNums {\n 7 = Real\n}\n";
        let found = entries(text);
        assert_eq!(
            found.len(),
            1,
            "the skipped block's string brace dug no deeper hole"
        );
        assert_eq!(found.first().expect("entry").number, 7, "its number");
    }

    #[test]
    fn helpers_honour_backslash_escapes_in_strings() {
        assert_eq!(
            braces("q \"a \\\" b { c\""),
            (0, 0),
            "the open brace sits inside the string"
        );
        assert_eq!(
            strip_comments("\"a \\\"// keep\" // cut", false).0,
            "\"a \\\"// keep\" ",
            "the escaped quote keeps the slashes inside the string; the real comment cuts"
        );
    }

    #[test]
    fn helpers_strip_and_count_as_documented() {
        assert_eq!(
            braces("a { \"b } c\" }"),
            (1, 1),
            "the brace in the string misses"
        );
        assert_eq!(
            class_name(" \"Spaced Name\", arg"),
            Some("Spaced Name".to_owned()),
            "quoted"
        );
        assert_eq!(
            class_name(" Bare, 1"),
            Some("Bare".to_owned()),
            "bare cuts at the comma"
        );
        assert_eq!(class_name(" "), None, "nothing to name");
        assert_eq!(
            strip_comments("a \"b//c\" d // e", false),
            ("a \"b//c\" d ".to_owned(), false),
            "the comment after the string drops, the one inside stays"
        );
        assert!(strip_comments("a /* b", false).1, "the block opens");
        assert_eq!(
            strip_comments(" c */ d", true).0,
            " d",
            "it closes mid-line"
        );
    }
}
