//! The character cursor behind [`crate::wad::udmf`]: what the reader holds,
//! where on the line it sits and how trivia and quoted strings are eaten.
//!
//! [`Cursor`] walks the source as `(byte offset, char)` pairs, keeping the
//! 1-based line and column of the character it holds so every grammar failure
//! can name it. It consumes whitespace and both comment styles, reads a
//! quoted string with its escapes resolved and scans a run of characters
//! accepted by a predicate; deciding what a scanned word means is
//! `crate::wad::udmf`'s `parser` module.

use core::iter::Peekable;
use core::str::CharIndices;

use crate::error::CyberError;
use crate::wad::udmf::{Pos, err_at};

/// A character cursor that knows the line and column of what it reads.
pub(super) struct Cursor<'input> {
    /// The characters not yet read, with their byte offsets.
    chars: Peekable<CharIndices<'input>>,
    /// The 1-based line of the next character.
    line: usize,
    /// The 1-based column of the next character.
    col: usize,
}

impl<'input> Cursor<'input> {
    /// A cursor at the start of `text`.
    pub(super) fn new(text: &'input str) -> Self {
        Self {
            chars: text.char_indices().peekable(),
            line: 1_usize,
            col: 1_usize,
        }
    }

    /// The position of the character the cursor holds.
    #[must_use]
    pub(super) const fn here(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.col,
        }
    }

    /// The next character, without consuming it.
    pub(super) fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied().map(|(_, ch)| ch)
    }

    /// Consumes and returns the next character.
    fn eat(&mut self) -> Option<char> {
        let (_, ch) = self.chars.next()?;
        self.walk_over(ch);
        Some(ch)
    }

    /// Consumes the next character, whatever it is.
    pub(super) fn skip(&mut self) {
        if let Some((_, ch)) = self.chars.next() {
            self.walk_over(ch);
        }
    }

    /// Moves the line and column past `ch`.
    const fn walk_over(&mut self, ch: char) {
        if ch == '\n' {
            self.line = self.line.saturating_add(1);
            self.col = 1_usize;
        } else {
            self.col = self.col.saturating_add(1);
        }
    }

    /// Consumes characters into `sink` while `accept` holds of them.
    pub(super) fn scan_while<Accept>(&mut self, accept: &Accept, sink: &mut String)
    where
        Accept: Fn(char) -> bool,
    {
        while let Some(ch) = self.peek().filter(|ch| accept(*ch)) {
            sink.push(ch);
            self.skip();
        }
    }

    /// What the cursor holds, phrased for a message.
    fn found(&mut self) -> String {
        self.peek()
            .map_or_else(|| "end of input".to_owned(), |ch| format!("'{ch}'"))
    }

    /// The failure saying the cursor holds no `expected` thing.
    pub(super) fn unexpected(&mut self, pos: Pos, expected: &str) -> CyberError {
        let found = self.found();
        err_at(pos, &format!("expected {expected}, found {found}"))
    }

    /// Consumes whitespace and comments.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] for an unterminated block comment or a `'/'`
    /// that starts none.
    pub(super) fn skip_trivia(&mut self) -> Result<(), CyberError> {
        while self.peek().is_some_and(is_trivia_char) {
            self.skip_trivia_step()?;
        }
        Ok(())
    }

    /// Consumes one whitespace character or comment; the cursor holds it.
    fn skip_trivia_step(&mut self) -> Result<(), CyberError> {
        if self.peek() == Some('/') {
            return self.skip_comment();
        }
        self.skip();
        Ok(())
    }

    /// Consumes one comment; the cursor sits on its `'/'`.
    fn skip_comment(&mut self) -> Result<(), CyberError> {
        let open = self.here();
        self.skip();
        match self.peek() {
            Some('/') => {
                self.finish_line_comment();
                Ok(())
            }
            Some('*') => self.finish_block_comment(open),
            Some(other) => Err(err_at(
                open,
                &format!("a '/' must start a comment, found '/{other}'"),
            )),
            None => Err(err_at(open, "a '/' must start a comment")),
        }
    }

    /// Consumes the rest of a `//` comment; the cursor sits on its second
    /// `'/'`.
    fn finish_line_comment(&mut self) {
        self.skip();
        while self.peek().is_some_and(|ch| ch != '\n') {
            self.skip();
        }
    }

    /// Consumes the rest of a `/* */` comment; the cursor sits on its `'*'`.
    fn finish_block_comment(&mut self, open: Pos) -> Result<(), CyberError> {
        self.skip();
        let mut closed = false;
        while !closed && self.peek().is_some() {
            self.eat_comment_char(&mut closed);
        }
        if closed {
            return Ok(());
        }
        Err(err_at(open, "unterminated block comment: expected '*/'"))
    }

    /// Consumes one block-comment character, setting `closed` when it eats the
    /// `'*/'` pair that ends the comment.
    fn eat_comment_char(&mut self, closed: &mut bool) {
        if self.eat().is_some_and(|ch| ch == '*') && self.take_if_slash() {
            *closed = true;
        }
    }

    /// Consumes a `'/'` when the cursor holds one, reporting whether it did.
    fn take_if_slash(&mut self) -> bool {
        if self.peek() != Some('/') {
            return false;
        }
        self.skip();
        true
    }

    /// Consumes a quoted string, resolving the escapes inside it.
    pub(super) fn read_string(&mut self) -> Result<String, CyberError> {
        let open = self.here();
        self.skip();
        let mut text = String::new();
        let mut closed = false;
        while !closed {
            closed = self.read_string_chunk(&mut text, open)?;
        }
        Ok(text)
    }

    /// Consumes one run of string characters and the escape or quote that ends
    /// it, appending what they stand for to `text`; reports whether the
    /// closing quote was read.
    fn read_string_chunk(&mut self, text: &mut String, open: Pos) -> Result<bool, CyberError> {
        self.scan_while(&is_string_char, text);
        match self.eat() {
            Some('"') => Ok(true),
            None => Err(err_at(
                open,
                "unterminated string: expected a closing quote",
            )),
            Some('\n') => Err(err_at(open, "unterminated string: a raw newline ends it")),
            Some(_) => {
                text.push(self.read_escape(open)?);
                Ok(false)
            }
        }
    }

    /// Consumes the character after a backslash: `'\"'` and `'\\'` keep
    /// themselves, `'\n'` is a newline and any other escape stands for its own
    /// character.
    fn read_escape(&mut self, open: Pos) -> Result<char, CyberError> {
        match self.eat() {
            Some('n') => Ok('\n'),
            Some('\n') => Err(err_at(open, "unterminated string: an escape ends the line")),
            Some(other) => Ok(other),
            None => Err(err_at(
                open,
                "unterminated string: an escape ends the input",
            )),
        }
    }
}

/// Whether `ch` may appear in a bare value word: anything but the punctuation
/// of the grammar, a quote or whitespace.
pub(super) fn is_word_char(ch: char) -> bool {
    ch != '"' && !matches!(ch, '{' | '}' | '(' | ')' | ';' | '\'') && !ch.is_whitespace()
}

/// Whether `ch` is whitespace or a comment's start.
fn is_trivia_char(ch: char) -> bool {
    ch == '/' || ch.is_whitespace()
}

/// Whether `ch` is a plain character of a quoted string.
const fn is_string_char(ch: char) -> bool {
    !matches!(ch, '"' | '\\' | '\n')
}
