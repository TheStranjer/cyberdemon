//! The lump tools' shared argument parsers: the `scope`, `namespace`,
//! `kind` and `map` filters, the `limit` page size and the hex needles a
//! `contains_hex` argument carries.
//!
//! Each one is a pure function of the wire text, so every tool can refuse a
//! bad argument before it takes a session lock. [`crate::tools::lumps::list`]
//! and [`crate::tools::lumps::get`] import what their own tools parse; the
//! one name [`crate::tools::lumps::select`],
//! [`crate::tools::lumps::place`] and [`crate::tools::lumps::edit`] parse
//! with stays re-exported from the group's root, so their
//! `crate::tools::lumps::parse_namespace` imports keep working. Scope names
//! come from [`Scope::parse_or_merged`] and page sizes from
//! [`crate::tools::parse_limit`].

use crate::error::CyberError;
use crate::named::Named as _;
use crate::tools::lumps::list::parse_pair;
use crate::wad::classify::{LumpKind, Namespace};
use crate::wad::name::LumpName;

/// Every `Namespace` name, for error hints.
pub(super) fn namespace_names() -> String {
    Namespace::ALL
        .iter()
        .map(|namespace| namespace.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every `LumpKind` name, for error hints.
pub(super) fn kind_names() -> String {
    LumpKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The namespace a filter keeps, [`Namespace::parse`] being
/// case-insensitive.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
pub(super) fn parse_namespace(text: Option<&str>) -> Result<Option<Namespace>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    Namespace::parse(raw).map(Some).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "unknown namespace {raw:?}: expected one of {names}",
            names = namespace_names()
        ))
    })
}

/// The type hint a filter keeps, matched case-insensitively against
/// [`LumpKind::as_str`].
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] listing the accepted names.
pub(super) fn parse_kind(text: Option<&str>) -> Result<Option<LumpKind>, CyberError> {
    let Some(raw) = text else {
        return Ok(None);
    };
    LumpKind::parse(raw).map(Some).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "unknown kind {raw:?}: expected one of {names}",
            names = kind_names()
        ))
    })
}

/// The map block a filter keeps, parsed strictly as a lump name.
///
/// # Errors
///
/// [`LumpName::parse`]'s [`CyberError::InvalidArgument`] for a name that is
/// not 1 to 8 printable `ASCII` characters.
pub(super) fn parse_map(text: Option<&str>) -> Result<Option<LumpName>, CyberError> {
    text.map(LumpName::parse).transpose()
}

/// The needle bytes of a `contains_hex` string: hex digits in pairs, with
/// any whitespace between them ignored.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a non-hex character or an odd
/// number of digits.
pub(super) fn parse_hex(text: Option<&str>) -> Result<Option<Vec<u8>>, CyberError> {
    text.map(|raw| hex_bytes("contains_hex", raw)).transpose()
}

/// The bytes a hex argument spells: hex digits in pairs, whitespace
/// between them ignored; `field` names the argument in the errors.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a non-hex character or an odd
/// number of digits.
pub(super) fn hex_bytes(field: &str, raw: &str) -> Result<Vec<u8>, CyberError> {
    let digits: Vec<char> = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
    let mut bytes = Vec::new();
    let mut pairs = digits.chunks_exact(2);
    for pair in pairs.by_ref() {
        let Some(byte) = parse_pair(pair) else {
            return Err(CyberError::InvalidArgument(format!(
                "{field} {raw:?} holds {pair:?}, which is not a hex pair (use 0-9 and a-f)"
            )));
        };
        bytes.push(byte);
    }
    if let Some(left) = pairs.remainder().first() {
        return Err(CyberError::InvalidArgument(format!(
            "{field} {raw:?} holds an odd number of hex digits ({left:?} is left over): write \
             whole bytes such as \"4D 55 53 1A\""
        )));
    }
    Ok(bytes)
}
