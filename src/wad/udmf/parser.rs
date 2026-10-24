//! The `UDMF` grammar behind [`crate::wad::udmf`]: the document, block,
//! field and value passes over the characters the cursor holds.
//!
//! [`Parser`] reads global assignments and `identifier { fields }` blocks in
//! any order, lowercases every identifier and classifies each bare word as an
//! integer, a float, a boolean or a keyword of its own. A word containing a
//! point is checked against the float grammar by
//! `crate::wad::udmf`'s `decimal` module and one starting with a digit is
//! read as a signed decimal, leading-`0` octal or `0x` hexadecimal integer;
//! anything else the grammar refuses is a [`CyberError::Format`] at the
//! position the cursor reported.

use crate::error::CyberError;
use crate::wad::udmf::cursor::{Cursor, is_word_char};
use crate::wad::udmf::decimal::{drop_sign, is_float_literal, take_sign};
use crate::wad::udmf::{Pos, UdmfBlock, UdmfDoc, UdmfValue, err_at};

/// The `UDMF` grammar, read from a [`Cursor`].
pub(super) struct Parser<'input> {
    /// The character source.
    cursor: Cursor<'input>,
}

impl<'input> Parser<'input> {
    /// A parser over `text`.
    pub(super) fn new(text: &'input str) -> Self {
        Self {
            cursor: Cursor::new(text),
        }
    }

    /// Reads the whole document: global assignments and blocks, in any order.
    pub(super) fn parse(&mut self) -> Result<UdmfDoc, CyberError> {
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
pub(super) fn accumulate(digits: &str, radix: u32) -> Option<i128> {
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
