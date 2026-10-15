//! `UDMF` text maps: the `TEXTMAP` grammar as a document of blocks and
//! key/value pairs.
//!
//! [`parse_udmf`] reads `UDMF` 1.1 source into a [`UdmfDoc`]: the global
//! assignments and the typed blocks that make up a `TEXTMAP` lump, every
//! field kept in the order it was written. [`write_udmf`] renders a document
//! back to source: the `namespace` global first, then the other globals, then
//! the blocks in order, each block keyword carrying a `// index` comment that
//! counts the blocks of its kind from zero. Re-parsing written output yields
//! an equal document, comments aside.
//!
//! Floats never touch a binary format: [`UdmfValue::Float`] holds the text as
//! written and [`decimal_to_rounded_int`] turns such text into an integer by
//! digit arithmetic, rounding half away from zero; [`has_fraction`] says
//! whether a decimal text names a whole number at all.
//!
//! Identifiers are stored lowercased while value text keeps its case: `true`
//! and `false` are the only boolean spellings, and a bare word that starts
//! like a number has to be a valid integer or float. Every grammar failure
//! is a [`CyberError::Format`] whose message starts with the
//! `TEXTMAP line:column: ` of the offending token.

use core::iter::Peekable;
use core::str::CharIndices;

use crate::error::CyberError;

/// The largest `TEXTMAP` text [`parse_udmf`] accepts: 64 MiB.
const MAX_UDMF_TEXT: usize = 0x0400_0000;

/// The global whose value [`write_udmf`] puts on the first line.
const NAMESPACE: &str = "namespace";

/// The most digits an `i64` magnitude can carry (`9223372036854775808`).
const MAX_I64_MAGNITUDE_DIGITS: i128 = 19_i128;

/// One value of the `UDMF` grammar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UdmfValue {
    /// An integer literal: decimal, leading-`0` octal or `0x` hexadecimal.
    Int(i64),
    /// A float literal, kept as the text it was written in; `UDMF` floats are
    /// never converted to a binary format.
    Float(String),
    /// A quoted string, with its escapes resolved.
    Str(String),
    /// The `true` or `false` keyword.
    Bool(bool),
    /// Any other bare word, spelled as written.
    Keyword(String),
}

impl UdmfValue {
    /// The value as an integer: an [`UdmfValue::Int`] as it stands and an
    /// [`UdmfValue::Float`] rounded half away from zero; nothing otherwise.
    ///
    /// The rounded value is an `i64`, so a [`UdmfValue::Float`] too large for
    /// one reads as no value at all.
    #[must_use]
    #[inline]
    pub fn as_i64_rounded(&self) -> Option<i64> {
        rounded_of(self.clone())
    }

    /// The value as a boolean: `true` and `false` only, nothing otherwise.
    #[must_use]
    #[inline]
    pub const fn as_bool(&self) -> Option<bool> {
        match *self {
            Self::Bool(yes) => Some(yes),
            Self::Float(_) | Self::Int(_) | Self::Keyword(_) | Self::Str(_) => None,
        }
    }

    /// The value's string contents as their own text, or `None` for any other
    /// kind.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> Option<String> {
        string_of(self.clone())
    }

    /// The value rendered as `UDMF` source text: integers decimal, floats
    /// their own text, strings quoted with escapes, booleans `true` or
    /// `false` and keywords bare. Re-parsing the result gives this value
    /// back.
    #[must_use]
    #[inline]
    pub fn to_source(&self) -> String {
        let mut out = String::new();
        write_value(&mut out, self.clone());
        out
    }
}

/// The integer a value stands for, its float rounded half away from zero.
fn rounded_of(value: UdmfValue) -> Option<i64> {
    match value {
        UdmfValue::Int(number) => Some(number),
        UdmfValue::Float(text) => decimal_to_rounded_int(text.as_str()).ok(),
        UdmfValue::Bool(_) | UdmfValue::Keyword(_) | UdmfValue::Str(_) => None,
    }
}

/// The text a [`UdmfValue::Str`] holds.
fn string_of(value: UdmfValue) -> Option<String> {
    match value {
        UdmfValue::Str(text) => Some(text),
        UdmfValue::Bool(_) | UdmfValue::Float(_) | UdmfValue::Int(_) | UdmfValue::Keyword(_) => {
            None
        }
    }
}

/// One `identifier { fields }` block of a `UDMF` document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UdmfBlock {
    /// The block's kind, an identifier in lowercase: `vertex`, `linedef`.
    pub kind: String,
    /// The block's fields, in the order they were written.
    pub fields: Vec<(String, UdmfValue)>,
}

/// A whole `UDMF` document: the globals outside any block and the blocks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UdmfDoc {
    /// Assignments outside every block, in the order they were written.
    pub globals: Vec<(String, UdmfValue)>,
    /// The document's blocks, in the order they were written.
    pub blocks: Vec<UdmfBlock>,
}

/// Reads a `UDMF` document from source text.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when `text` is larger than 64 MiB;
/// [`CyberError::Format`] naming the `TEXTMAP line:column` of the first token
/// that breaks the grammar: a missing `'='`, `';'` or `'}'`, an unterminated
/// string, comment or block, a malformed number, an integer outside the
/// `i64` range, or an invalid octal or hexadecimal digit.
#[inline]
pub fn parse_udmf(text: &str) -> Result<UdmfDoc, CyberError> {
    if text.len() > MAX_UDMF_TEXT {
        return Err(CyberError::LimitExceeded(format!(
            "UDMF text of {} bytes exceeds the {MAX_UDMF_TEXT} byte limit",
            text.len()
        )));
    }
    Parser::new(text).parse()
}

/// Renders a `UDMF` document as source text.
///
/// The `namespace` global is written first when present, then the remaining
/// globals in their order, then the blocks in theirs, one field per line and
/// each block keyword followed by a `// index` comment counting the blocks of
/// that kind from zero. Integers are decimal, floats keep the text they were
/// read from, strings are quoted with `'\"'`, `'\\'` and `'\n'` escapes,
/// booleans are `true` or `false` and keywords stand bare. Re-parsing the
/// result gives back an equal document, except that a `namespace` global that
/// was not already first moves to the front.
#[must_use]
#[inline]
pub fn write_udmf(doc: &UdmfDoc) -> String {
    let mut out = String::new();
    let mut rest: Vec<&(String, UdmfValue)> = Vec::with_capacity(doc.globals.len());
    for field in &doc.globals {
        if field.0 == NAMESPACE {
            write_field(&mut out, &field.0, &field.1);
        } else {
            rest.push(field);
        }
    }
    for field in &rest {
        write_field(&mut out, &field.0, &field.1);
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    for block in &doc.blocks {
        write_block(&mut out, block, &mut counts);
    }
    out
}

/// The integer a decimal text names, rounded half away from zero.
///
/// `text` is `[+-]digits[.digits][e[+-]digits]`, which is wider than the
/// `UDMF` float grammar: the digits after the point are optional here, so
/// `"1e-3"` is accepted. The value is worked out from the digit text alone;
/// no binary floating point is involved.
///
/// # Errors
///
/// [`CyberError::Format`] when `text` is not shaped like a decimal number or
/// when the rounded value does not fit an `i64`.
#[inline]
pub fn decimal_to_rounded_int(text: &str) -> Result<i64, CyberError> {
    let parts = split_decimal(text)?;
    let magnitude = rounded_magnitude(&parts).ok_or_else(|| out_of_range(text))?;
    let signed = if parts.negative {
        magnitude.saturating_neg()
    } else {
        magnitude
    };
    i64::try_from(signed).ok().ok_or_else(|| out_of_range(text))
}

/// Whether the decimal `text` names a value with a fractional part.
///
/// Text that is no decimal literal counts as fractional: its value cannot be
/// shown as an integer.
#[must_use]
#[inline]
pub fn has_fraction(text: &str) -> bool {
    let Ok(parts) = split_decimal(text) else {
        return true;
    };
    let (digits, shift) = normalize(&parts);
    if digits.is_empty() || shift >= 0 {
        return false;
    }
    let places = shift
        .checked_neg()
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(usize::MAX);
    let skip = digits.len().saturating_sub(places);
    digits.chars().skip(skip).any(|digit| digit != '0')
}

/// The failure saying `text` names a value no `i64` can hold.
fn out_of_range(text: &str) -> CyberError {
    CyberError::Format(format!("the value '{text}' does not fit into an i64"))
}

/// The position of a token, for messages.
#[derive(Clone, Copy, Debug)]
struct Pos {
    /// The 1-based line.
    line: usize,
    /// The 1-based column, counted in characters.
    col: usize,
}

/// A `UDMF` parse failure at `pos`, as a client sees it.
fn err_at(pos: Pos, message: &str) -> CyberError {
    CyberError::Format(format!("TEXTMAP {}:{}: {message}", pos.line, pos.col))
}

/// A character cursor that knows the line and column of what it reads.
struct Cursor<'input> {
    /// The characters not yet read, with their byte offsets.
    chars: Peekable<CharIndices<'input>>,
    /// The 1-based line of the next character.
    line: usize,
    /// The 1-based column of the next character.
    col: usize,
}

impl<'input> Cursor<'input> {
    /// A cursor at the start of `text`.
    fn new(text: &'input str) -> Self {
        Self {
            chars: text.char_indices().peekable(),
            line: 1_usize,
            col: 1_usize,
        }
    }

    /// The position of the character the cursor holds.
    #[must_use]
    const fn here(&self) -> Pos {
        Pos {
            line: self.line,
            col: self.col,
        }
    }

    /// The next character, without consuming it.
    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied().map(|(_, ch)| ch)
    }

    /// Consumes and returns the next character.
    fn eat(&mut self) -> Option<char> {
        let (_, ch) = self.chars.next()?;
        self.walk_over(ch);
        Some(ch)
    }

    /// Consumes the next character, whatever it is.
    fn skip(&mut self) {
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
    fn scan_while<Accept>(&mut self, accept: &Accept, sink: &mut String)
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
    fn unexpected(&mut self, pos: Pos, expected: &str) -> CyberError {
        let found = self.found();
        err_at(pos, &format!("expected {expected}, found {found}"))
    }

    /// Consumes whitespace and comments.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] for an unterminated block comment or a `'/'`
    /// that starts none.
    fn skip_trivia(&mut self) -> Result<(), CyberError> {
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
    fn read_string(&mut self) -> Result<String, CyberError> {
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
fn is_word_char(ch: char) -> bool {
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

/// The `UDMF` grammar, read from a [`Cursor`].
struct Parser<'input> {
    /// The character source.
    cursor: Cursor<'input>,
}

impl<'input> Parser<'input> {
    /// A parser over `text`.
    fn new(text: &'input str) -> Self {
        Self {
            cursor: Cursor::new(text),
        }
    }

    /// Reads the whole document: global assignments and blocks, in any order.
    fn parse(&mut self) -> Result<UdmfDoc, CyberError> {
        let mut doc = UdmfDoc::default();
        while self.start_item()? {
            self.parse_item(&mut doc)?;
        }
        Ok(doc)
    }

    /// Consumes the trivia between items, reporting whether the input holds
    /// another one.
    fn start_item(&mut self) -> Result<bool, CyberError> {
        self.cursor.skip_trivia()?;
        Ok(self.cursor.peek().is_some())
    }

    /// Reads one block or global assignment into `doc`.
    fn parse_item(&mut self, doc: &mut UdmfDoc) -> Result<(), CyberError> {
        let name = self.expect_ident()?;
        self.cursor.skip_trivia()?;
        if self.cursor.peek() == Some('{') {
            let block = self.parse_block(name)?;
            doc.blocks.push(block);
        } else {
            let field = self.parse_assignment(name)?;
            doc.globals.push(field);
        }
        Ok(())
    }

    /// Reads one identifier, lowercasing it.
    fn expect_ident(&mut self) -> Result<String, CyberError> {
        let pos = self.cursor.here();
        if !self.cursor.peek().is_some_and(is_ident_start) {
            return Err(self.cursor.unexpected(pos, "an identifier"));
        }
        let mut name = String::new();
        self.cursor.scan_while(&is_ident_char, &mut name);
        Ok(name.to_lowercase())
    }

    /// Reads `'{' fields '}'`; the cursor sits on the `'{'`.
    fn parse_block(&mut self, kind: String) -> Result<UdmfBlock, CyberError> {
        self.cursor.skip();
        let mut fields: Vec<(String, UdmfValue)> = Vec::new();
        while self.start_field()? {
            fields.push(self.parse_block_field()?);
        }
        self.cursor.skip();
        Ok(UdmfBlock { kind, fields })
    }

    /// Consumes the trivia inside a block, reporting whether a field stands
    /// before the `'}'`; running out of input leaves the block unclosed.
    fn start_field(&mut self) -> Result<bool, CyberError> {
        self.cursor.skip_trivia()?;
        if self.cursor.peek().is_none() {
            return Err(err_at(
                self.cursor.here(),
                "unterminated block: expected '}'",
            ));
        }
        Ok(self.cursor.peek() != Some('}'))
    }

    /// Reads one `name = value;` field of the block being parsed.
    fn parse_block_field(&mut self) -> Result<(String, UdmfValue), CyberError> {
        let name = self.expect_ident()?;
        self.cursor.skip_trivia()?;
        self.parse_assignment(name)
    }

    /// Reads `'=' value ';'` for the field named `name`; the cursor sits on
    /// the `'='`.
    fn parse_assignment(&mut self, name: String) -> Result<(String, UdmfValue), CyberError> {
        let equals = self.cursor.here();
        if self.cursor.peek() != Some('=') {
            return Err(err_at(
                equals,
                &format!("expected '=' after identifier '{name}'"),
            ));
        }
        self.cursor.skip();
        self.cursor.skip_trivia()?;
        let value = self.parse_value()?;
        self.cursor.skip_trivia()?;
        let semicolon = self.cursor.here();
        if self.cursor.peek() != Some(';') {
            return Err(self
                .cursor
                .unexpected(semicolon, &format!("';' after the value of '{name}'")));
        }
        self.cursor.skip();
        Ok((name, value))
    }

    /// Reads one value: a quoted string or a bare word.
    fn parse_value(&mut self) -> Result<UdmfValue, CyberError> {
        let pos = self.cursor.here();
        if self.cursor.peek() == Some('"') {
            return self.cursor.read_string().map(UdmfValue::Str);
        }
        let mut word = String::new();
        self.cursor.scan_while(&is_word_char, &mut word);
        if word.is_empty() {
            return Err(self.cursor.unexpected(pos, "a value"));
        }
        classify_word(&word, pos)
    }
}

/// Whether `ch` may start an identifier.
const fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

/// Whether `ch` may continue an identifier.
const fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// Decides which value a bare word is: a number, a boolean keyword or a
/// keyword of its own.
///
/// # Errors
///
/// [`CyberError::Format`] when the word starts like a number and is no valid
/// integer or float.
fn classify_word(word: &str, pos: Pos) -> Result<UdmfValue, CyberError> {
    if word.contains('.') {
        if is_float_literal(word) {
            return Ok(UdmfValue::Float(word.to_owned()));
        }
        return Err(err_at(pos, &format!("'{word}' is not a valid float")));
    }
    if starts_like_number(word) {
        return parse_integer(word, pos);
    }
    Ok(match word {
        "true" => UdmfValue::Bool(true),
        "false" => UdmfValue::Bool(false),
        other => UdmfValue::Keyword(other.to_owned()),
    })
}

/// Whether a bare word begins like a number and so has to be one.
fn starts_like_number(word: &str) -> bool {
    drop_sign(word)
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
}

/// Reads a signed integer literal in decimal, leading-`0` octal or `0x`
/// hexadecimal.
///
/// # Errors
///
/// [`CyberError::Format`] for a digit outside its radix or a value outside
/// the `i64` range.
fn parse_integer(word: &str, pos: Pos) -> Result<UdmfValue, CyberError> {
    let (negative, digits) = take_sign(word);
    let (radix, body) = literal_radix(digits);
    if body.is_empty() {
        return Err(err_at(pos, &format!("'{word}' is not an integer")));
    }
    if let Some(bad) = body.chars().find(|digit| digit.to_digit(radix).is_none()) {
        return Err(invalid_digit(word, bad, radix, pos));
    }
    let magnitude = accumulate(body, radix).ok_or_else(|| overflow(word, pos))?;
    let signed = if negative {
        magnitude.saturating_neg()
    } else {
        magnitude
    };
    i64::try_from(signed)
        .ok()
        .ok_or_else(|| overflow(word, pos))
        .map(UdmfValue::Int)
}

/// The failure saying the integer `word` at `pos` leaves the `i64` range.
fn overflow(word: &str, pos: Pos) -> CyberError {
    err_at(pos, &format!("the value '{word}' does not fit into an i64"))
}

/// The radix an unsigned integer literal is written in and its digit text.
fn literal_radix(digits: &str) -> (u32, &str) {
    if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        return (16_u32, hex);
    }
    if let Some(octal) = digits.strip_prefix('0') {
        if !octal.is_empty() {
            return (8_u32, octal);
        }
    }
    (10_u32, digits)
}

/// Accumulates a digit run in `radix`, or `None` past the `i128` range.
fn accumulate(digits: &str, radix: u32) -> Option<i128> {
    let base = i128::from(radix);
    digits.chars().try_fold(0_i128, |value, digit| {
        let unit = i128::from(digit.to_digit(radix)?);
        value.checked_mul(base)?.checked_add(unit)
    })
}

/// The failure naming a `digit` that `radix` does not hold.
fn invalid_digit(word: &str, digit: char, radix: u32, pos: Pos) -> CyberError {
    let kind = if radix == 8 {
        "octal"
    } else if radix == 16 {
        "hexadecimal"
    } else {
        "decimal"
    };
    err_at(
        pos,
        &format!("invalid {kind} digit '{digit}' in integer '{word}'"),
    )
}

/// Whether `word` is the float grammar: `[+-]?digits '.' digits?` with an
/// optional exponent.
fn is_float_literal(word: &str) -> bool {
    let Some((head, tail)) = drop_sign(word).split_once('.') else {
        return false;
    };
    let fraction = exponent_digits(tail).unwrap_or(tail);
    !head.is_empty() && all_digits(head) && all_digits(fraction) && exponent_is_valid(tail)
}

/// The digits before the exponent marker of a float's tail.
fn exponent_digits(tail: &str) -> Option<&str> {
    tail.split_once(['e', 'E']).map(|(head, _)| head)
}

/// Whether whatever follows an exponent marker is a signed digit run.
fn exponent_is_valid(tail: &str) -> bool {
    match tail.split_once(['e', 'E']) {
        Some((_head, exponent)) => is_digit_run(drop_sign(exponent)),
        None => true,
    }
}

/// Whether `text` is a non-empty run of decimal digits.
fn is_digit_run(text: &str) -> bool {
    !text.is_empty() && all_digits(text)
}

/// Whether every character of `text` is a decimal digit.
fn all_digits(text: &str) -> bool {
    text.chars().all(|ch| ch.is_ascii_digit())
}

/// Splits a leading sign from `text`, reporting a `'-'` first.
fn take_sign(text: &str) -> (bool, &str) {
    if let Some(body) = text.strip_prefix('-') {
        return (true, body);
    }
    (false, text.strip_prefix('+').unwrap_or(text))
}

/// `text` without a leading sign.
fn drop_sign(text: &str) -> &str {
    take_sign(text).1
}

/// A decimal literal taken apart.
struct Decimal<'text> {
    /// Whether a `'-'` introduced the value.
    negative: bool,
    /// The digits before the point.
    whole: &'text str,
    /// The digits after the point, empty when there is no point.
    fraction: &'text str,
    /// The power of ten the digits stand for, saturating away from zero.
    exponent: i128,
}

/// Splits `[+-]digits[.digits][e[+-]digits]` into its parts.
///
/// # Errors
///
/// [`CyberError::Format`] when `text` is not shaped like that.
fn split_decimal(text: &str) -> Result<Decimal<'_>, CyberError> {
    let (negative, body) = take_sign(text);
    let (mantissa, exponent) = split_exponent(body);
    let (whole, fraction) = split_point(mantissa);
    if !is_digit_run(whole) || !all_digits(fraction) {
        return Err(CyberError::Format(format!(
            "'{text}' is not a decimal number"
        )));
    }
    Ok(Decimal {
        negative,
        whole,
        fraction,
        exponent: parse_exponent(exponent, text)?,
    })
}

/// Splits `text` at its first exponent marker, if it has one.
fn split_exponent(text: &str) -> (&str, Option<&str>) {
    match text.split_once(['e', 'E']) {
        Some((head, tail)) => (head, Some(tail)),
        None => (text, None),
    }
}

/// Splits `text` at its first point, leaving the fraction empty without one.
fn split_point(text: &str) -> (&str, &str) {
    text.split_once('.').unwrap_or((text, ""))
}

/// Reads an exponent, saturating so that an absurd scale still decides
/// whole against fractional.
///
/// # Errors
///
/// [`CyberError::Format`] when `exponent` is not `[+-]?digits`; `source` is
/// the whole literal, named in the message.
fn parse_exponent(exponent: Option<&str>, source: &str) -> Result<i128, CyberError> {
    let Some(body) = exponent else {
        return Ok(0_i128);
    };
    let (negative, digits) = take_sign(body);
    if !is_digit_run(digits) {
        return Err(CyberError::Format(format!(
            "'{source}' is not a decimal number"
        )));
    }
    let magnitude = accumulate(digits, 10_u32).unwrap_or(i128::MAX);
    if negative {
        return Ok(magnitude.saturating_neg());
    }
    Ok(magnitude)
}

/// The literal's digits without leading zeroes and the power of ten scaling
/// them; the digits stand for zero when none are left.
fn normalize(parts: &Decimal<'_>) -> (String, i128) {
    let digits: String = parts.whole.chars().chain(parts.fraction.chars()).collect();
    let scale = digit_count(parts.fraction);
    let shift = parts.exponent.checked_sub(scale).unwrap_or(i128::MIN);
    (digits.trim_start_matches('0').to_owned(), shift)
}

/// The integer a decimal literal names, before its sign is applied.
fn rounded_magnitude(parts: &Decimal<'_>) -> Option<i128> {
    let (digits, shift) = normalize(parts);
    if digits.is_empty() {
        return Some(0_i128);
    }
    if shift >= 0 {
        scale_up(&digits, shift)
    } else {
        scale_down(&digits, shift)
    }
}

/// `digits` times ten to the `shift`, or `None` past an `i64` magnitude.
fn scale_up(digits: &str, shift: i128) -> Option<i128> {
    let width = digit_count(digits).checked_add(shift)?;
    if width > MAX_I64_MAGNITUDE_DIGITS {
        return None;
    }
    let magnitude = i128_of_digits(digits)?;
    (0..shift).try_fold(magnitude, |value, _| value.checked_mul(10_i128))
}

/// `digits` divided by ten to the `-shift`, rounded half away from zero.
fn scale_down(digits: &str, shift: i128) -> Option<i128> {
    let Some(places) = shift
        .checked_neg()
        .and_then(|value| usize::try_from(value).ok())
    else {
        return Some(0_i128);
    };
    if places > digits.len() {
        return Some(0_i128);
    }
    let keep = digits.len().saturating_sub(places);
    let head: String = digits.chars().take(keep).collect();
    let round_up = digits.chars().nth(keep).is_some_and(|digit| digit >= '5');
    let magnitude = i128_of_digits(&head)?;
    if round_up {
        magnitude.checked_add(1_i128)
    } else {
        Some(magnitude)
    }
}

/// The number of characters in `text`, counted in `i128`.
fn digit_count(text: &str) -> i128 {
    text.chars()
        .fold(0_i128, |count, _| count.saturating_add(1_i128))
}

/// The value of an ASCII digit run, or `None` past the `i128` range.
fn i128_of_digits(digits: &str) -> Option<i128> {
    digits.chars().try_fold(0_i128, |value, digit| {
        let unit = i128::from(digit.to_digit(10_u32)?);
        value.checked_mul(10_i128)?.checked_add(unit)
    })
}

/// Appends one `name = value;` line.
fn write_field(out: &mut String, name: &str, value: &UdmfValue) {
    out.push_str(name);
    out.push_str(" = ");
    write_value(out, value.clone());
    out.push_str(";\n");
}

/// Appends one block with its index comment and tab-indented fields.
fn write_block(out: &mut String, block: &UdmfBlock, counts: &mut Vec<(String, usize)>) {
    let index = next_index(counts, &block.kind);
    out.push_str(&block.kind);
    out.push_str(" { // ");
    out.push_str(&index.to_string());
    out.push('\n');
    for field in &block.fields {
        out.push('\t');
        write_field(out, &field.0, &field.1);
    }
    out.push_str("}\n");
}

/// The next 0-based index for blocks of `kind`, counting within `counts`.
fn next_index(counts: &mut Vec<(String, usize)>, kind: &str) -> usize {
    let known = counts.iter_mut().find(|entry| entry.0 == kind);
    if let Some(entry) = known {
        let seen = entry.1;
        entry.1 = seen.saturating_add(1);
        return seen;
    }
    counts.push((kind.to_owned(), 1_usize));
    0_usize
}

/// Appends one value as source text.
fn write_value(out: &mut String, value: UdmfValue) {
    match value {
        UdmfValue::Int(number) => out.push_str(&number.to_string()),
        UdmfValue::Bool(yes) => out.push_str(if yes { "true" } else { "false" }),
        UdmfValue::Float(text) | UdmfValue::Keyword(text) => out.push_str(text.as_str()),
        UdmfValue::Str(text) => write_quoted(out, text.as_str()),
    }
}

/// Appends `text` as a quoted string, escaping the characters that would
/// otherwise end it or vanish.
fn write_quoted(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_UDMF_TEXT, UdmfBlock, UdmfDoc, UdmfValue, decimal_to_rounded_int, has_fraction,
        parse_udmf, write_udmf,
    };
    use crate::error::CyberError;

    /// Parses `text`, panicking with the message when it is no valid document.
    fn parse(text: &str) -> UdmfDoc {
        match parse_udmf(text) {
            Ok(doc) => doc,
            Err(error) => panic!("expected a document, got {error}"),
        }
    }

    /// The `Format` message `text` fails with, panicking when it parses.
    fn error_of(text: &str) -> String {
        match parse_udmf(text) {
            Ok(doc) => panic!("expected an error, parsed {} globals", doc.globals.len()),
            Err(CyberError::Format(message)) => message,
            Err(other) => panic!("expected a Format error, got {other:?}"),
        }
    }

    /// One global assignment read out of `text`.
    fn global(text: &str) -> (String, UdmfValue) {
        let doc = parse(text);
        let field = doc.globals.first().expect("the assignment is there");
        assert!(doc.blocks.is_empty(), "no block in {text}");
        field.clone()
    }

    /// Reads `text` and writes it again, returning the source.
    fn round_trip(text: &str) -> String {
        let doc = parse(text);
        let written = write_udmf(&doc);
        assert_eq!(parse(&written), doc, "writing must preserve the document");
        written
    }

    #[test]
    fn reads_every_value_kind() {
        assert_eq!(global("x = -12;"), ("x".to_owned(), UdmfValue::Int(-12)));
        assert_eq!(
            global("x = 64.000;"),
            ("x".to_owned(), UdmfValue::Float("64.000".to_owned()))
        );
        assert_eq!(
            global("x = \"hello\";"),
            ("x".to_owned(), UdmfValue::Str("hello".to_owned()))
        );
        assert_eq!(global("x = true;"), ("x".to_owned(), UdmfValue::Bool(true)));
        assert_eq!(
            global("x = false;"),
            ("x".to_owned(), UdmfValue::Bool(false))
        );
        assert_eq!(
            global("x = bottom;"),
            ("x".to_owned(), UdmfValue::Keyword("bottom".to_owned()))
        );
        assert_eq!(
            global("x = Sky1;"),
            ("x".to_owned(), UdmfValue::Keyword("Sky1".to_owned()))
        );
    }

    #[test]
    fn reads_hexadecimal_and_octal_integers() {
        assert_eq!(global("x = 0;").1, UdmfValue::Int(0));
        assert_eq!(global("x = 0x1f;").1, UdmfValue::Int(31));
        assert_eq!(global("x = 0X10;").1, UdmfValue::Int(16));
        assert_eq!(global("x = 0xFF;").1, UdmfValue::Int(255));
        assert_eq!(global("x = 017;").1, UdmfValue::Int(15));
        assert_eq!(global("x = -0x10;").1, UdmfValue::Int(-16));
        assert_eq!(global("x = +07;").1, UdmfValue::Int(7));
        assert_eq!(global("x = 000;").1, UdmfValue::Int(0));
    }

    #[test]
    fn reads_negative_and_exponent_numbers() {
        assert_eq!(global("x = -42;").1, UdmfValue::Int(-42));
        assert_eq!(global("x = -0.5;").1, UdmfValue::Float("-0.5".to_owned()));
        assert_eq!(
            global("x = 1.25e2;").1,
            UdmfValue::Float("1.25e2".to_owned())
        );
        assert_eq!(
            global("x = -3.5E-2;").1,
            UdmfValue::Float("-3.5E-2".to_owned())
        );
        assert_eq!(global("x = 64.;").1, UdmfValue::Float("64.".to_owned()));
    }

    #[test]
    fn keeps_punctuation_inside_a_bare_word() {
        assert_eq!(
            global("x = a/b;").1,
            UdmfValue::Keyword("a/b".to_owned()),
            "a slash only starts a comment between tokens"
        );
        assert_eq!(
            global("x = -weird;").1,
            UdmfValue::Keyword("-weird".to_owned()),
            "a sign only begins a number before a digit"
        );
    }

    #[test]
    fn reads_cr_lf_lines_and_raw_tabs_in_strings() {
        let doc = parse("x = \"a\tb\";\r\ny = 2;");
        assert_eq!(
            doc.globals[0].1,
            UdmfValue::Str("a\tb".to_owned()),
            "a raw tab is string content, an escape is its letter"
        );
        assert_eq!(doc.globals[1].0, "y", "CR LF separates lines");
    }

    #[test]
    fn resolves_string_escapes() {
        assert_eq!(
            global(r#"x = "a\"b";"#).1,
            UdmfValue::Str("a\"b".to_owned())
        );
        assert_eq!(
            global(r#"x = "a\\b";"#).1,
            UdmfValue::Str("a\\b".to_owned())
        );
        assert_eq!(
            global(r#"x = "a\nb";"#).1,
            UdmfValue::Str("a\nb".to_owned())
        );
        assert_eq!(global(r#"x = "a\zb";"#).1, UdmfValue::Str("azb".to_owned()));
        assert_eq!(global(r#"x = "a\tb";"#).1, UdmfValue::Str("atb".to_owned()));
    }

    #[test]
    fn skips_both_comment_styles() {
        let doc = parse(
            "// leading\nvertex /* a * b */ { /* two */ x = 1; // three\n } /* four */\n// five",
        );
        assert_eq!(doc.blocks.len(), 1, "one block survives the comments");
        assert_eq!(doc.blocks.first().expect("block").kind, "vertex");
        assert_eq!(doc.blocks.first().expect("block").fields.len(), 1);
    }

    #[test]
    fn lowercases_identifiers_only() {
        let doc = parse("LineDef { V1 = true; MyKey = Sky1; Other = True; }");
        let block = doc.blocks.first().expect("the block");
        assert_eq!(block.kind, "linedef", "the kind is lowercased");
        assert_eq!(block.fields[0].0, "v1", "so is the field name");
        assert_eq!(
            block.fields[0].1,
            UdmfValue::Bool(true),
            "lowercase true is a boolean"
        );
        assert_eq!(
            block.fields[1].1,
            UdmfValue::Keyword("Sky1".to_owned()),
            "a keyword keeps its case"
        );
        assert_eq!(
            block.fields[2].1,
            UdmfValue::Keyword("True".to_owned()),
            "only the exact spelling is a boolean"
        );
    }

    #[test]
    fn reads_globals_after_blocks() {
        let doc = parse("a = 1; v { x = 2; } namespace = \"zdoom\"; b = 3;");
        let names: Vec<&str> = doc.globals.iter().map(|field| field.0.as_str()).collect();
        assert_eq!(names, ["a", "namespace", "b"], "globals keep their order");
        assert_eq!(doc.blocks.len(), 1, "the block stays where it was");
    }

    #[test]
    fn reads_an_empty_block_and_an_empty_document() {
        let doc = parse("thing {}");
        assert_eq!(
            doc.blocks,
            vec![UdmfBlock {
                kind: "thing".to_owned(),
                fields: Vec::new(),
            }]
        );
        assert_eq!(parse(""), UdmfDoc::default(), "nothing parses to nothing");
        assert_eq!(parse("  // just a comment\n"), UdmfDoc::default());
    }

    #[test]
    fn reads_the_specification_example() {
        let doc = parse(
            "namespace = \"zdoom\";\n\
             vertex // 0\n\
             {\n\
             x = 64.000;\n\
             y = -128.000;\n\
             }\n\
             linedef { v1 = 0; v2 = 1; sidefront = 0; blocking = true; }\n",
        );
        assert_eq!(
            doc.globals,
            vec![("namespace".to_owned(), UdmfValue::Str("zdoom".to_owned()))]
        );
        assert_eq!(doc.blocks.len(), 2, "a vertex and a linedef");
        assert_eq!(doc.blocks[1].fields.len(), 4, "four linedef fields");
        assert_eq!(doc.blocks[1].fields[3].1, UdmfValue::Bool(true));
    }

    #[test]
    fn reports_a_missing_identifier() {
        assert_eq!(
            error_of("= 5;"),
            "TEXTMAP 1:1: expected an identifier, found '='"
        );
    }

    #[test]
    fn reports_a_missing_equals_sign() {
        assert_eq!(
            error_of("a = 1;\nb c = 2;"),
            "TEXTMAP 2:3: expected '=' after identifier 'b'"
        );
    }

    #[test]
    fn reports_a_missing_semicolon() {
        assert_eq!(
            error_of("x = 1 }"),
            "TEXTMAP 1:7: expected ';' after the value of 'x', found '}'"
        );
        assert_eq!(
            error_of("v { x = 1 }"),
            "TEXTMAP 1:11: expected ';' after the value of 'x', found '}'"
        );
    }

    #[test]
    fn reports_a_missing_value() {
        assert_eq!(
            error_of("x = ;"),
            "TEXTMAP 1:5: expected a value, found ';'"
        );
        assert_eq!(
            error_of("x = ),"),
            "TEXTMAP 1:5: expected a value, found ')'"
        );
        assert_eq!(
            error_of("x = 'a';"),
            "TEXTMAP 1:5: expected a value, found '''"
        );
    }

    #[test]
    fn reports_an_unterminated_string() {
        assert_eq!(
            error_of("x = \"abc"),
            "TEXTMAP 1:5: unterminated string: expected a closing quote"
        );
        assert_eq!(
            error_of("x = \"a\nb\";"),
            "TEXTMAP 1:5: unterminated string: a raw newline ends it"
        );
        assert_eq!(
            error_of("x = \"a\\"),
            "TEXTMAP 1:5: unterminated string: an escape ends the input"
        );
        assert_eq!(
            error_of("x = \"a\\\nb\";"),
            "TEXTMAP 1:5: unterminated string: an escape ends the line"
        );
    }

    #[test]
    fn reports_values_cut_off_by_the_end_of_input() {
        assert_eq!(
            error_of("x = "),
            "TEXTMAP 1:5: expected a value, found end of input"
        );
        assert_eq!(
            error_of("x"),
            "TEXTMAP 1:2: expected '=' after identifier 'x'"
        );
        assert_eq!(
            error_of("v { 1 = 2; }"),
            "TEXTMAP 1:5: expected an identifier, found '1'"
        );
    }

    #[test]
    fn reports_an_unterminated_comment() {
        assert_eq!(
            error_of("/* open\nx = 1;"),
            "TEXTMAP 1:1: unterminated block comment: expected '*/'"
        );
        assert_eq!(
            error_of("x = 1; / y"),
            "TEXTMAP 1:8: a '/' must start a comment, found '/ '"
        );
        assert_eq!(
            error_of("x = 1; /"),
            "TEXTMAP 1:8: a '/' must start a comment"
        );
    }

    #[test]
    fn reports_an_unterminated_block() {
        assert_eq!(
            error_of("linedef { x = 1;"),
            "TEXTMAP 1:17: unterminated block: expected '}'"
        );
    }

    #[test]
    fn reports_invalid_numbers() {
        assert_eq!(
            error_of("x = 09;"),
            "TEXTMAP 1:5: invalid octal digit '9' in integer '09'"
        );
        assert_eq!(
            error_of("x = -0o7;"),
            "TEXTMAP 1:5: invalid octal digit 'o' in integer '-0o7'"
        );
        assert_eq!(
            error_of("x = 0xg;"),
            "TEXTMAP 1:5: invalid hexadecimal digit 'g' in integer '0xg'"
        );
        assert_eq!(
            error_of("x = 1e5;"),
            "TEXTMAP 1:5: invalid decimal digit 'e' in integer '1e5'"
        );
        assert_eq!(error_of("x = 0x;"), "TEXTMAP 1:5: '0x' is not an integer");
        assert_eq!(
            error_of("x = 1.2.3;"),
            "TEXTMAP 1:5: '1.2.3' is not a valid float"
        );
        assert_eq!(
            error_of("x = .5;"),
            "TEXTMAP 1:5: '.5' is not a valid float"
        );
        assert_eq!(
            error_of("x = 1.5e;"),
            "TEXTMAP 1:5: '1.5e' is not a valid float"
        );
        assert_eq!(
            error_of("x = 1.5e+;"),
            "TEXTMAP 1:5: '1.5e+' is not a valid float"
        );
        assert_eq!(
            error_of("x = 1.2e3e4;"),
            "TEXTMAP 1:5: '1.2e3e4' is not a valid float"
        );
    }

    #[test]
    fn reports_integer_overflow() {
        assert_eq!(
            error_of("x = 99999999999999999999;"),
            "TEXTMAP 1:5: the value '99999999999999999999' does not fit into an i64"
        );
        assert_eq!(
            error_of("x = -9223372036854775809;"),
            "TEXTMAP 1:5: the value '-9223372036854775809' does not fit into an i64"
        );
        assert_eq!(
            global("x = -9223372036854775808;").1,
            UdmfValue::Int(i64::MIN),
            "the boundary itself parses"
        );
        assert_eq!(
            error_of(&format!("x = {};", "9".repeat(40))),
            format!(
                "TEXTMAP 1:5: the value '{}' does not fit into an i64",
                "9".repeat(40)
            ),
            "even digits past the i128 accumulator are refused"
        );
    }

    #[test]
    fn rejects_text_beyond_the_size_guard() {
        let too_big = "a".repeat(MAX_UDMF_TEXT.saturating_add(1));
        assert!(
            matches!(parse_udmf(&too_big), Err(CyberError::LimitExceeded(_))),
            "one byte over the limit is refused"
        );
    }

    #[test]
    fn writes_a_document_of_every_value_kind() {
        let written = round_trip(concat!(
            "namespace = \"zdoom\"; rate = 1.25e2; flag = false;",
            " name = \"a\\\"b\"; mode = Sky1; n = 0x10;",
            " vertex { x = 64.000; } vertex { x = 0; }",
            " linedef { v1 = 0; blocking = true; }",
        ));
        assert_eq!(
            written,
            concat!(
                "namespace = \"zdoom\";\n",
                "rate = 1.25e2;\n",
                "flag = false;\n",
                "name = \"a\\\"b\";\n",
                "mode = Sky1;\n",
                "n = 16;\n",
                "vertex { // 0\n",
                "\tx = 64.000;\n",
                "}\n",
                "vertex { // 1\n",
                "\tx = 0;\n",
                "}\n",
                "linedef { // 0\n",
                "\tv1 = 0;\n",
                "\tblocking = true;\n",
                "}\n",
            ),
            "globals keep their order after namespace, blocks count per kind"
        );
    }

    #[test]
    fn round_trips_a_typical_textmap() {
        let text = concat!(
            "namespace = \"zdoom\";\r\n",
            "things\r\n{\r\n",
            "\tx = -128.000;\r\n",
            "\ty = 64.000;\r\n",
            "\tangle = 90;\r\n",
            "\ttype = 1;\r\n",
            "\tflags = 7;\r\n",
            "}\r\n",
            "linedef\r\n{\r\n",
            "\tv1 = 0;\r\n",
            "\tv2 = 1;\r\n",
            "\tsidefront = 0;\r\n",
            "\tblocking = true;\r\n",
            "\tspecial = WS_Door_Close;\r\n",
            "}\r\n",
            "sector\r\n{\r\n",
            "\tfloorplane_sky = \"Sky1\";\r\n",
            "\tlight = 128;\r\n",
            "}\r\n",
        );
        let written = round_trip(text);
        assert!(
            written.contains("sector { // 0\n"),
            "the sector block is indexed: {written}"
        );
        assert_eq!(
            written.matches("// ").count(),
            3,
            "one index comment per block: {written}"
        );
    }

    #[test]
    fn writes_an_empty_document_as_nothing() {
        assert_eq!(write_udmf(&UdmfDoc::default()), "");
    }

    #[test]
    fn moves_the_namespace_to_the_front() {
        let doc = parse("a = 1; namespace = \"utf8\";");
        assert_eq!(
            write_udmf(&doc),
            "namespace = \"utf8\";\na = 1;\n",
            "namespace leads, the rest keeps its order"
        );
    }

    #[test]
    fn writes_a_string_newline_as_an_escape() {
        let written = round_trip("x = \"a\\nb\";");
        assert_eq!(written, "x = \"a\\nb\";\n");
    }

    #[test]
    fn rounds_the_text_of_a_decimal() {
        let cases = [
            ("64.000", 64_i64),
            ("-0.5", -1),
            ("1.25e2", 125),
            ("1e-3", 0),
            ("2.5", 3),
            ("-2.5", -3),
            ("0", 0),
            ("-0", 0),
            ("0.4999", 0),
            ("0.5", 1),
            ("-1.5", -2),
            ("1.4", 1),
            ("1.6", 2),
            ("9.999", 10),
            ("-12.34e1", -123),
            ("0.25e-1", 0),
            ("3.", 3),
            ("-3.", -3),
            ("9223372036854775807", i64::MAX),
            ("-9223372036854775808", i64::MIN),
            ("1e-99999999999999999999999", 0),
            ("-1e-99999999999999999999999", 0),
        ];
        for (text, expected) in cases {
            assert_eq!(
                decimal_to_rounded_int(text).expect("a rounded value"),
                expected,
                "'{text}' rounds to {expected}"
            );
        }
    }

    #[test]
    fn refuses_to_round_what_is_no_decimal() {
        let bad = ["", "x", "abc", "1e", ".5", "1.2.3", "+", "1e+", "e5"];
        for text in bad {
            assert!(
                matches!(decimal_to_rounded_int(text), Err(CyberError::Format(_))),
                "'{text}' is no decimal literal"
            );
        }
        let long = "1".repeat(40);
        for overflow in [
            "1.5e19",
            "9223372036854775808",
            "1e99999999999999999999999",
            "1e9999999999999999999999999999999999999999",
            "-1e9999999999999999999999999999999999999999",
            "9999999999999999999999999999999999999999.5",
            &long,
        ] {
            assert!(
                matches!(decimal_to_rounded_int(overflow), Err(CyberError::Format(_))),
                "'{overflow}' leaves the i64 range"
            );
        }
    }

    #[test]
    fn knows_whether_a_decimal_has_a_fraction() {
        let cases = [
            ("64.000", false),
            ("0", false),
            ("0.0", false),
            ("-3.", false),
            ("1.25e2", false),
            ("10e-1", false),
            ("1.5", true),
            ("-2.5", true),
            ("1e-3", true),
            ("1.0e-1", true),
            ("0.001", true),
            ("x", true),
            ("", true),
        ];
        for (text, expected) in cases {
            assert_eq!(
                has_fraction(text),
                expected,
                "'{text}' fraction is {expected}"
            );
        }
    }

    #[test]
    fn writes_each_value_in_its_own_shape() {
        let doc = parse("i = 0x10; f = 1.25e2; s = \"a\\\\b\"; b = false; k = Sky1; n = -3;");
        assert_eq!(
            write_udmf(&doc),
            concat!(
                "i = 16;\n",
                "f = 1.25e2;\n",
                "s = \"a\\\\b\";\n",
                "b = false;\n",
                "k = Sky1;\n",
                "n = -3;\n",
            ),
            "ints are decimal, floats keep their text, keywords stand bare"
        );
    }

    #[test]
    fn to_source_renders_every_value_kind() {
        assert_eq!(UdmfValue::Int(-3).to_source(), "-3");
        assert_eq!(UdmfValue::Float("64.5".to_owned()).to_source(), "64.5");
        assert_eq!(UdmfValue::Bool(true).to_source(), "true");
        assert_eq!(UdmfValue::Bool(false).to_source(), "false");
        assert_eq!(UdmfValue::Keyword("Sky1".to_owned()).to_source(), "Sky1");
        assert_eq!(
            UdmfValue::Str("a\"b\\c\nd".to_owned()).to_source(),
            r#""a\"b\\c\nd""#,
            "the rendering round trips back to the same value"
        );
        let values = [
            UdmfValue::Int(7),
            UdmfValue::Float("1.25e2".to_owned()),
            UdmfValue::Str("x".to_owned()),
            UdmfValue::Bool(false),
            UdmfValue::Keyword("bottom".to_owned()),
        ];
        for value in values {
            let text = value.to_source();
            let doc = parse(&format!("x = {text};"));
            let rendered = doc.globals.first().expect("the field").1.clone();
            assert_eq!(rendered, value, "'{text}' parses back");
        }
    }

    #[test]
    fn values_read_as_the_types_around_their_own() {
        assert_eq!(UdmfValue::Int(-7).as_i64_rounded(), Some(-7));
        assert_eq!(UdmfValue::Float("2.5".to_owned()).as_i64_rounded(), Some(3));
        assert_eq!(
            UdmfValue::Float("1e-3".to_owned()).as_i64_rounded(),
            Some(0),
            "a text too small to round up is still a whole number"
        );
        assert_eq!(
            UdmfValue::Float("1.5e19".to_owned()).as_i64_rounded(),
            None,
            "a float past i64 has no integer form"
        );
        assert_eq!(UdmfValue::Keyword("x".to_owned()).as_i64_rounded(), None);
        assert_eq!(UdmfValue::Str("x".to_owned()).as_i64_rounded(), None);
        assert_eq!(UdmfValue::Bool(false).as_i64_rounded(), None);

        assert_eq!(UdmfValue::Bool(true).as_bool(), Some(true));
        assert_eq!(UdmfValue::Int(1).as_bool(), None);
        assert_eq!(UdmfValue::Keyword("true".to_owned()).as_bool(), None);

        assert_eq!(
            UdmfValue::Str("a".to_owned()).as_str(),
            Some("a".to_owned())
        );
        assert_eq!(UdmfValue::Keyword("a".to_owned()).as_str(), None);
        assert_eq!(UdmfValue::Int(1).as_str(), None);
    }
}
