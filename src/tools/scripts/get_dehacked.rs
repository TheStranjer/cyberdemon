//! `get_dehacked`'s read: the parsed shape of one DeHackEd/BEX patch lump.
//!
//! [`get_run`] takes the winning copy of the named lump in the asked-for
//! scope ([`super::find_lump`]), runs the tolerant parser of
//! [`crate::defs::dehacked_edit`] over its text and answers [`GetReport`]:
//! where the lump came from ([`source_entry`]), the file-level header
//! values, one [`BlockEntry`] per parsed block ([`block_entry`]), one
//! [`SectionEntry`] per BEX section ([`section_entry`]) and the parser's
//! warnings. The edit half lives in [`mod@super::edit_dehacked`], the two
//! script tools in [`mod@super::text_tools`], and what all three share -
//! the name and scope helpers and the router - in [`super`].

use alloc::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::defs::dehacked_edit::{self, Block, Field, Section};
use crate::error::CyberError;
use crate::session::{LayerRole, Session, Source};
use crate::tools::lumps::rows::LumpRow;
use crate::wad::name::LumpName;

use super::{find_lump, lump_name, parse_scope};

/// Arguments for [`Cyberdemon::get_dehacked`].
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct GetDehackedParams {
    /// Patch lump to read: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively. Defaults to `"DEHACKED"`.
    #[serde(default)]
    name: Option<String>,
    /// Which archive to read: `"merged"` (the default: what an engine
    /// sees), `"working"` or `"layer:N"` (0 is the `IWAD`).
    #[serde(default)]
    scope: Option<String>,
    /// Also return the whole lump text, decoded lossily (default `false`).
    #[serde(default)]
    include_text: Option<bool>,
}

/// One `Key = Value` map of the report, keys as written (later duplicates
/// win).
pub(super) type FieldMap = BTreeMap<String, String>;

/// Where a read patch lump was taken from.
#[derive(Serialize)]
pub(super) struct SourceEntry {
    /// `"iwad"`, `"resource"` or `"working"`.
    kind: &'static str,
    /// The layer index (0 is the `IWAD`), `null` for the working archive.
    index: Option<usize>,
    /// The layer's archive path, `null` for the working archive.
    path: Option<String>,
}

/// The file-level header values of a patch.
#[derive(Serialize)]
pub(super) struct HeaderEntry {
    /// The `Doom version` value, `null` when absent or not a number.
    doom_version: Option<i64>,
    /// The `Patch format` value, `null` when absent or not a number.
    patch_format: Option<i64>,
}

/// One parsed block as the tool reports it.
#[derive(Serialize)]
pub(super) struct BlockEntry {
    /// The canonical kind word (`Thing`, `Frame`, ..., `Text`).
    kind: &'static str,
    /// The header number; a `Text` block's first length.
    number: Option<i64>,
    /// The parenthesised header name; a `Text` block's second length.
    name: Option<String>,
    /// The 1-based line of the header.
    line: usize,
    /// The `Key = Value` lines (empty for a `Text` block: its body is
    /// opaque and only shows in `text`).
    fields: FieldMap,
}

/// One parsed BEX section as the tool reports it.
#[derive(Serialize)]
pub(super) struct SectionEntry {
    /// The canonical section name (`STRINGS`, `CODEPTR`, ...).
    section: &'static str,
    /// The 1-based line of the section header.
    line: usize,
    /// The `KEY = VALUE` lines, keys as written.
    entries: FieldMap,
}

/// Structured result of [`Cyberdemon::get_dehacked`].
#[derive(Serialize)]
pub(super) struct GetReport {
    /// The lump that was read.
    name: LumpName,
    /// The archive the lump came from.
    source: SourceEntry,
    /// The file-level header values.
    header: HeaderEntry,
    /// The blocks, in file order.
    blocks: Vec<BlockEntry>,
    /// The BEX sections, in file order.
    bex: Vec<SectionEntry>,
    /// The whole lump text; only present when `include_text` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// Lines the tolerant parser refused to read (1-based) and every
    /// `Include` it reported but did not follow.
    warnings: Vec<String>,
}

/// Where a row's lump came from, as the report spells it.
fn source_entry(session: &Session, row: &LumpRow) -> SourceEntry {
    match row.source {
        Source::Working => SourceEntry {
            kind: "working",
            index: None,
            path: None,
        },
        Source::Layer(index) => layer_entry(session, index),
    }
}

/// The layer source fields, tolerating a layer that vanished mid-call.
fn layer_entry(session: &Session, index: usize) -> SourceEntry {
    let layers = session.layers();
    let Some(layer) = layers.get(index) else {
        return SourceEntry {
            kind: "resource",
            index: Some(index),
            path: None,
        };
    };
    SourceEntry {
        kind: role_kind(layer.role),
        index: Some(index),
        path: Some(layer.path.display().to_string()),
    }
}

/// The report word for a layer's role.
const fn role_kind(role: LayerRole) -> &'static str {
    match role {
        LayerRole::Iwad => "iwad",
        LayerRole::Resource => "resource",
    }
}

/// The report map of key lines: the last value for a repeated key wins.
fn field_map(fields: &[Field]) -> FieldMap {
    fields
        .iter()
        .map(|entry| (entry.key.clone(), entry.value.clone()))
        .collect()
}

/// The report row for one parsed block.
fn block_entry(block: &Block) -> BlockEntry {
    BlockEntry {
        kind: block.kind,
        number: block.number,
        name: block.name.clone(),
        line: block.line.saturating_add(1),
        fields: field_map(&block.fields),
    }
}

/// The report row for one parsed BEX section.
fn section_entry(section: &Section) -> SectionEntry {
    SectionEntry {
        section: section.name,
        line: section.line.saturating_add(1),
        entries: field_map(&section.entries),
    }
}

/// `get_dehacked`'s work: read the winning copy of the lump in `scope` and
/// report its parsed shape.
///
/// # Errors
///
/// [`lump_name`] and [`parse_scope`] refusals,
/// [`crate::tools::lumps::rows::build_rows`]' and [`find_lump`]'s errors
/// for the scope.
pub(super) fn get_run(
    session: &Session,
    args: &GetDehackedParams,
) -> Result<GetReport, CyberError> {
    let name = lump_name(args.name.as_deref())?;
    let scope = parse_scope(args.scope.as_deref())?;
    let row = find_lump(session, name, scope)?;
    let text = String::from_utf8_lossy(&row.lump.data).into_owned();
    let patch = dehacked_edit::parse(&text);
    Ok(GetReport {
        name,
        source: source_entry(session, &row),
        header: HeaderEntry {
            doom_version: patch.doom_version,
            patch_format: patch.format,
        },
        blocks: patch.blocks.iter().map(block_entry).collect(),
        bex: patch.sections.iter().map(section_entry).collect(),
        text: args.include_text.unwrap_or(false).then_some(text),
        warnings: patch.warnings.clone(),
    })
}
