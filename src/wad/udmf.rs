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
//!
//! The value, block and document shapes, the [`parse_udmf`], [`write_udmf`],
//! [`leading_namespace`], [`decimal_to_rounded_int`] and [`has_fraction`]
//! entry points with the
//! failures they report, the token position and the writer helpers stay
//! here; reading the characters is the private `cursor` module, running the
//! grammar over them the private `parser` one and the decimal literal
//! arithmetic the private `decimal` one. All three are implementation
//! details of the entry points above.

mod cursor;
mod decimal;
mod parser;

pub use parser::leading_namespace;

use crate::error::CyberError;
use crate::wad::udmf::decimal::{normalize, rounded_magnitude, split_decimal};
use crate::wad::udmf::parser::Parser;

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
mod tests;
