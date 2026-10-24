//! Tests for the `UDMF` grammar: the readers every topic measures with.
//!
//! [`parse`] and [`error_of`] run source text through
//! [`crate::wad::udmf::parse_udmf`], the first panicking with the message,
//! [`global`] reads one assignment out of it and [`round_trip`] writes a
//! parsed document again and checks that re-parsing gives it back.
//!
//! [`read`] reads documents and reports every grammar failure and [`write`]
//! renders them, rounds decimal texts and reads values as the types around
//! their own.

mod read;
mod write;

use crate::error::CyberError;
use crate::wad::udmf::{UdmfDoc, UdmfValue, parse_udmf, write_udmf};

/// Parses `text`, panicking with the message when it is no valid document.
pub(super) fn parse(text: &str) -> UdmfDoc {
    match parse_udmf(text) {
        Ok(doc) => doc,
        Err(error) => panic!("expected a document, got {error}"),
    }
}

/// The `Format` message `text` fails with, panicking when it parses.
pub(super) fn error_of(text: &str) -> String {
    match parse_udmf(text) {
        Ok(doc) => panic!("expected an error, parsed {} globals", doc.globals.len()),
        Err(CyberError::Format(message)) => message,
        Err(other) => panic!("expected a Format error, got {other:?}"),
    }
}

/// One global assignment read out of `text`.
pub(super) fn global(text: &str) -> (String, UdmfValue) {
    let doc = parse(text);
    let field = doc.globals.first().expect("the assignment is there");
    assert!(doc.blocks.is_empty(), "no block in {text}");
    field.clone()
}

/// Reads `text` and writes it again, returning the source.
pub(super) fn round_trip(text: &str) -> String {
    let doc = parse(text);
    let written = write_udmf(&doc);
    assert_eq!(parse(&written), doc, "writing must preserve the document");
    written
}
