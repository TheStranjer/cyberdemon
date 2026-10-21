//! Script tools: reading and editing the DeHackEd/BEX patch lumps and the
//! `DECORATE`/`ZSCRIPT` script lumps a session holds.
//!
//! [`Cyberdemon::get_dehacked`] parses one patch lump's text into its header
//! values, blocks and BEX sections (the tolerant line parser in
//! [`crate::defs::dehacked_edit`]); [`Cyberdemon::edit_dehacked`] writes one
//! patch lump back, either wholesale (`text`/`source`) or line by line
//! (`set`/`remove`), as a single undoable [`crate::session::Session::edit`]
//! labelled `edit_dehacked`. [`Cyberdemon::edit_decorate`] and
//! [`Cyberdemon::edit_zscript`] run one [`text_edit::TextOp`] over a script
//! lump's whole text with the same copy-on-write discipline, labelled
//! `edit_decorate LUMP` / `edit_zscript LUMP`. A lump held only by a
//! read-only layer is copied-on-write: the edited text is appended to the
//! working archive as the winning override. `get_thing_types` reads the same
//! merged texts, so `Thing`-block and script edits show up there.

mod text_edit;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::defs::dehacked_edit::{self, Block, Field, Section};
use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{LayerRole, MergedLump, Scope, Session, Source};
use crate::state::AppState;
use crate::tools::fail;
use crate::tools::lumps::rows::LumpRow;
use crate::tools::lumps::rows::build_rows;
use crate::tools::ok;
use crate::wad::archive::{Lump, Wad};
use crate::wad::name::LumpName;

use text_edit::TextOp;

/// The lump both tools read and edit when `name` is left out.
const DEFAULT_LUMP: &str = "DEHACKED";

/// The undo label of every `edit_dehacked` call.
const UNDO_LABEL: &str = "edit_dehacked";

/// Arguments for [`Cyberdemon::get_dehacked`].
#[derive(Debug, Deserialize, JsonSchema)]
struct GetDehackedParams {
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
type FieldMap = BTreeMap<String, String>;

/// Where a read patch lump was taken from.
#[derive(Serialize)]
struct SourceEntry {
    /// `"iwad"`, `"resource"` or `"working"`.
    kind: &'static str,
    /// The layer index (0 is the `IWAD`), `null` for the working archive.
    index: Option<usize>,
    /// The layer's archive path, `null` for the working archive.
    path: Option<String>,
}

/// The file-level header values of a patch.
#[derive(Serialize)]
struct HeaderEntry {
    /// The `Doom version` value, `null` when absent or not a number.
    doom_version: Option<i64>,
    /// The `Patch format` value, `null` when absent or not a number.
    patch_format: Option<i64>,
}

/// One parsed block as the tool reports it.
#[derive(Serialize)]
struct BlockEntry {
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
struct SectionEntry {
    /// The canonical section name (`STRINGS`, `CODEPTR`, ...).
    section: &'static str,
    /// The 1-based line of the section header.
    line: usize,
    /// The `KEY = VALUE` lines, keys as written.
    entries: FieldMap,
}

/// Structured result of [`Cyberdemon::get_dehacked`].
#[derive(Serialize)]
struct GetReport {
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

/// Arguments for [`Cyberdemon::edit_dehacked`]: exactly one of the four
/// modes, named by the field that is present.
#[derive(Debug, Deserialize, JsonSchema)]
struct EditDehackedParams {
    /// Patch lump to edit: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively. Defaults to `"DEHACKED"`.
    #[serde(default)]
    name: Option<String>,
    /// Replace the whole lump text with this. Exactly one mode.
    #[serde(default)]
    text: Option<String>,
    /// A `.deh`/`.bex` file (absolute path, `file://` URI, `http(s)://`
    /// URL or uploaded asset URL) whose bytes replace the lump text.
    /// Exactly one mode.
    #[serde(default)]
    source: Option<String>,
    /// Line updates to apply in order. Exactly one mode.
    #[serde(default)]
    set: Option<Vec<SetEntry>>,
    /// Line removals to apply in order. Exactly one mode.
    #[serde(default)]
    remove: Option<Vec<RemoveEntry>>,
}

/// One `set` entry: a key of a named block or BEX section.
#[derive(Debug, Deserialize, JsonSchema)]
struct SetEntry {
    /// The block or section, e.g. `"Thing 12"`, `"Misc"` or `"[STRINGS]"`
    /// (case-insensitive; the first matching block wins).
    block: String,
    /// The key to update or insert (case-insensitive match; the written
    /// line uses this spelling).
    key: String,
    /// The new value; must not contain newlines.
    value: String,
}

/// One `remove` entry: a key of a named block, or the whole block.
#[derive(Debug, Deserialize, JsonSchema)]
struct RemoveEntry {
    /// The block or section to take lines from, same spelling as in `set`.
    block: String,
    /// The key line to remove; omit it to remove the whole block, header
    /// through its last line.
    #[serde(default)]
    key: Option<String>,
}

/// Structured result of [`Cyberdemon::edit_dehacked`].
#[derive(Serialize)]
struct EditReport {
    /// The lump that was written.
    name: LumpName,
    /// Total lines of the resulting text.
    lines: usize,
    /// Lines rewritten, inserted or removed by this call.
    changed_lines: usize,
    /// Whether the lump was created in the working archive (it was absent,
    /// or held only by a read-only layer that was copied-on-write).
    created: bool,
}

/// Which text-lump tool is running: its name and default lump.
struct TextTool {
    /// The tool name, also the prefix of the undo label.
    name: &'static str,
    /// The lump edited when the `lump` argument is left out.
    default_lump: &'static str,
}

/// The [`Cyberdemon::edit_decorate`] descriptor.
const EDIT_DECORATE: TextTool = TextTool {
    name: "edit_decorate",
    default_lump: "DECORATE",
};

/// The [`Cyberdemon::edit_zscript`] descriptor.
const EDIT_ZSCRIPT: TextTool = TextTool {
    name: "edit_zscript",
    default_lump: "ZSCRIPT",
};

/// Arguments for [`Cyberdemon::edit_decorate`]: the optional lump name plus
/// exactly one [`TextOp`], flattened through its `op` tag.
#[derive(Debug, Deserialize, JsonSchema)]
struct EditDecorateParams {
    /// Text lump to edit: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively. Defaults to `"DECORATE"`.
    #[serde(default)]
    lump: Option<String>,
    /// The single operation to apply, selected by its `op` field.
    #[serde(flatten)]
    op: TextOp,
}

/// Arguments for [`Cyberdemon::edit_zscript`]: the optional lump name plus
/// exactly one [`TextOp`], flattened through its `op` tag.
#[derive(Debug, Deserialize, JsonSchema)]
struct EditZscriptParams {
    /// Text lump to edit: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively. Defaults to `"ZSCRIPT"`; pass the name of an
    /// `#include`d lump (e.g. `"ZSCRIPT2"`) to edit that file instead.
    #[serde(default)]
    lump: Option<String>,
    /// The single operation to apply, selected by its `op` field.
    #[serde(flatten)]
    op: TextOp,
}

/// Structured result of [`Cyberdemon::edit_decorate`] and
/// [`Cyberdemon::edit_zscript`].
#[derive(Debug, Serialize)]
struct TextReport {
    /// The lump that was written.
    lump: LumpName,
    /// Total lines of the resulting text.
    lines: usize,
    /// The 1-based inclusive range of changed lines, `[0, 0]` when nothing
    /// changed.
    changed_range: (usize, usize),
    /// The changed lines with two lines of context, each numbered.
    excerpt: String,
    /// Non-fatal notes about the result (e.g. unbalanced braces).
    warnings: Vec<String>,
    /// Whether the lump was created in the working archive (it was absent,
    /// or held only by a read-only layer that was copied-on-write).
    created: bool,
}

/// The parsed mode of one `edit_dehacked` call.
#[derive(Debug)]
enum Draft {
    /// The new full text is already in hand.
    Replace(String),
    /// Fetch this source and use its bytes as the new full text.
    Fetch(String),
    /// Apply these line operations to the current text.
    Ops(Vec<Op>),
}

/// One validated line operation.
#[derive(Clone, Debug)]
enum Op {
    /// Update or insert one key, creating the block when missing.
    Set {
        /// The target block or section spec.
        block: String,
        /// The key to set.
        key: String,
        /// The value to write.
        value: String,
    },
    /// Remove one key, or the whole block when `key` is `None`.
    Remove {
        /// The target block or section spec.
        block: String,
        /// The key to remove, `None` for the whole block.
        key: Option<String>,
    },
}

impl Op {
    /// Whether this operation removes lines (it cannot create a patch).
    const fn is_remove(&self) -> bool {
        matches!(self, Self::Remove { .. })
    }

    /// Runs the operation over a patch text, reporting lines changed.
    ///
    /// # Errors
    ///
    /// [`dehacked_edit::set_patch`] and [`dehacked_edit::remove_patch`]
    /// errors pass through.
    fn apply(&self, text: &str) -> Result<(String, usize), CyberError> {
        match self.clone() {
            Self::Set { block, key, value } => dehacked_edit::set_patch(text, &block, &key, &value),
            Self::Remove { block, key } => {
                dehacked_edit::remove_patch(text, &block, key.as_deref())
            }
        }
    }
}

/// The lump name to act on, defaulting to [`DEFAULT_LUMP`].
///
/// # Errors
///
/// [`LumpName::parse`]'s [`CyberError::InvalidArgument`] for a name that
/// does not parse.
fn lump_name(text: Option<&str>) -> Result<LumpName, CyberError> {
    LumpName::parse(text.unwrap_or(DEFAULT_LUMP))
}

/// The read scope, the merged view being the default.
///
/// # Errors
///
/// [`Scope::parse`]'s [`CyberError::InvalidArgument`] for any other name.
fn parse_scope(text: Option<&str>) -> Result<Scope, CyberError> {
    text.map_or(Ok(Scope::Merged), Scope::parse)
}

/// The `name` parameter as text, for scope labels in messages.
fn scope_label(scope: Scope) -> String {
    match scope {
        Scope::Merged => "merged".to_owned(),
        Scope::Working => "working".to_owned(),
        Scope::Layer(index) => format!("layer:{index}"),
    }
}

/// The row of the last working- or scope-directory copy of `name`.
///
/// # Errors
///
/// [`CyberError::NotFound`] naming the scope and the creating tool.
fn find_lump(session: &Session, name: LumpName, scope: Scope) -> Result<LumpRow, CyberError> {
    build_rows(session, scope)?
        .into_iter()
        .rev()
        .find(|row| row.name == name)
        .ok_or_else(|| {
            CyberError::NotFound(format!(
                "lump {name} not found in {scope}: merge an archive that holds it, or run \
                 edit_dehacked (set creates the lump in the working archive)",
                scope = scope_label(scope)
            ))
        })
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
/// [`lump_name`] and [`parse_scope`] refusals, [`build_rows`]' and
/// [`find_lump`]'s errors for the scope.
fn get_run(session: &Session, args: &GetDehackedParams) -> Result<GetReport, CyberError> {
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

/// Validate [`Cyberdemon::edit_dehacked`]'s arguments before any fetching.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] unless exactly one mode is present, and
/// for an empty `set`/`remove` list, a refused block spec, key or value.
fn plan(args: &EditDehackedParams) -> Result<Draft, CyberError> {
    let modes = usize::from(args.text.is_some())
        .saturating_add(usize::from(args.source.is_some()))
        .saturating_add(usize::from(args.set.is_some()))
        .saturating_add(usize::from(args.remove.is_some()));
    if modes != 1 {
        return Err(CyberError::InvalidArgument(
            "edit_dehacked takes exactly one mode: pass one of text, source, set and remove"
                .to_owned(),
        ));
    }
    if let Some(body) = args.text.as_ref() {
        return Ok(Draft::Replace(body.clone()));
    }
    if let Some(raw) = args.source.as_ref() {
        return Ok(Draft::Fetch(raw.clone()));
    }
    if let Some(entries) = args.set.as_ref() {
        return Ok(Draft::Ops(plan_set(entries)?));
    }
    args.remove.as_ref().map_or_else(
        || {
            Err(CyberError::Internal(
                "edit_dehacked planned no mode".to_owned(),
            ))
        },
        |entries| Ok(Draft::Ops(plan_remove(entries)?)),
    )
}

/// The validated `set` operations.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty list or a refused block,
/// key or value.
fn plan_set(entries: &[SetEntry]) -> Result<Vec<Op>, CyberError> {
    if entries.is_empty() {
        return Err(CyberError::InvalidArgument(
            "set needs at least one entry: pass block, key and value objects".to_owned(),
        ));
    }
    let mut ops: Vec<Op> = Vec::new();
    for entry in entries {
        dehacked_edit::parse_spec(&entry.block)?;
        dehacked_edit::check_key(&entry.key)?;
        dehacked_edit::check_value(&entry.value)?;
        ops.push(Op::Set {
            block: entry.block.clone(),
            key: entry.key.clone(),
            value: entry.value.clone(),
        });
    }
    Ok(ops)
}

/// The validated `remove` operations.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an empty list, a refused block spec
/// or an empty key.
fn plan_remove(entries: &[RemoveEntry]) -> Result<Vec<Op>, CyberError> {
    if entries.is_empty() {
        return Err(CyberError::InvalidArgument(
            "remove needs at least one entry: pass block (and optionally key) objects".to_owned(),
        ));
    }
    let mut ops: Vec<Op> = Vec::new();
    for entry in entries {
        dehacked_edit::parse_spec(&entry.block)?;
        if entry
            .key
            .as_deref()
            .is_some_and(|key| key.trim().is_empty())
        {
            return Err(CyberError::InvalidArgument(
                "remove key is empty: pass a key or omit it to drop the whole block".to_owned(),
            ));
        }
        ops.push(Op::Remove {
            block: entry.block.clone(),
            key: entry.key.clone(),
        });
    }
    Ok(ops)
}

/// Finish a draft outside the session lock: fetch the `source` mode's file.
///
/// # Errors
///
/// [`AppState::fetch_source`]'s errors pass through.
async fn resolve(state: &AppState, draft: Draft) -> Result<Body, CyberError> {
    match draft {
        Draft::Replace(body) => Ok(Body::Replace(body)),
        Draft::Fetch(raw) => {
            let fetched = state.fetch_source(&raw).await?;
            Ok(Body::Replace(
                String::from_utf8_lossy(&fetched.bytes).into_owned(),
            ))
        }
        Draft::Ops(ops) => Ok(Body::Ops(ops)),
    }
}

/// The resolved edit: a whole new text, or line operations on the current.
#[derive(Debug)]
enum Body {
    /// Replace the lump with this text.
    Replace(String),
    /// Apply these operations to the current text.
    Ops(Vec<Op>),
}

/// The text the winning copy of `name` holds, the working one first.
fn current_text(session: &Session, name: LumpName, at: Option<usize>) -> Option<String> {
    if let Some(index) = at {
        let lump = session.working().lumps.get(index)?;
        return Some(decode(lump));
    }
    let entry = session
        .merged()
        .into_iter()
        .rev()
        .find(|found| found.lump.name == name)?;
    Some(decode(&entry.lump))
}

/// A lump payload as text, invalid sequences replaced.
fn decode(lump: &Lump) -> String {
    String::from_utf8_lossy(&lump.data).into_owned()
}

/// The lines of `next` that differ from `base` (absent base counts every
/// line as new).
fn changed_against(base: Option<&str>, next: &str) -> usize {
    let old: Vec<&str> = base.unwrap_or("").lines().collect();
    let new: Vec<&str> = next.lines().collect();
    let common = old.len().min(new.len());
    let same = old.iter().zip(&new).filter(|pair| pair.0 == pair.1).count();
    let replaced = common.saturating_sub(same);
    let dropped = old.len().saturating_sub(common);
    let added = new.len().saturating_sub(common);
    replaced.saturating_add(dropped).saturating_add(added)
}

/// The result text of the operations mode, over the current text or the
/// canonical header when the lump is absent everywhere.
///
/// # Errors
///
/// [`CyberError::NotFound`] when `remove` runs against no lump at all, and
/// [`dehacked_edit::set_patch`]/[`dehacked_edit::remove_patch`]'s errors
/// for missing blocks and keys; nothing changes when an operation fails.
fn apply_ops(
    base: Option<&str>,
    ops: &[Op],
    name: LumpName,
) -> Result<(String, usize), CyberError> {
    let mut text = match base {
        Some(found) => found.to_owned(),
        None if ops.iter().any(Op::is_remove) => {
            return Err(CyberError::NotFound(format!(
                "lump {name} is in none of the loaded archives: remove has nothing to take \
                 from; get_dehacked lists what exists and edit_dehacked set creates a patch"
            )));
        }
        None => dehacked_edit::NEW_PATCH_HEADER.to_owned(),
    };
    let mut changed = 0_usize;
    for op in ops {
        let (next, step) = op.apply(&text)?;
        text = next;
        changed = changed.saturating_add(step);
    }
    if base.is_none() {
        changed = text.lines().count();
    }
    Ok((text, changed))
}

/// Overwrite the working copy at `at`, or append the override.
fn write_lump(wad: &mut Wad, name: LumpName, at: Option<usize>, text: &str) {
    let data = text.as_bytes();
    if let Some(index) = at
        && let Some(lump) = wad.lumps.get_mut(index)
    {
        lump.data = Arc::from(data);
        return;
    }
    wad.lumps.push(Lump::new(name, data.to_vec()));
}

/// `edit_dehacked`'s synchronous half: compute the new text, then commit
/// exactly one undoable write.
///
/// # Errors
///
/// [`apply_ops`]' refusals (the session does not change), and
/// [`crate::session::Session::edit`]'s error pass-through.
fn edit_run(session: &mut Session, name: LumpName, body: Body) -> Result<EditReport, CyberError> {
    let at = session.working().find_last(name);
    let base = current_text(session, name, at);
    let (text, changed) = match body {
        Body::Replace(next) => {
            let altered = changed_against(base.as_deref(), &next);
            (next, altered)
        }
        Body::Ops(ops) => apply_ops(base.as_deref(), &ops, name)?,
    };
    let created = at.is_none();
    let lines = text.lines().count();
    session.edit(UNDO_LABEL, |wad| {
        write_lump(wad, name, at, &text);
        Ok(())
    })?;
    Ok(EditReport {
        name,
        lines,
        changed_lines: changed,
        created,
    })
}

/// The lump name a text tool acts on, defaulting per [`TextTool`].
///
/// # Errors
///
/// [`LumpName::parse`]'s [`CyberError::InvalidArgument`] for a name that
/// does not parse.
fn text_lump_name(tool: &TextTool, text: Option<&str>) -> Result<LumpName, CyberError> {
    LumpName::parse(text.unwrap_or(tool.default_lump))
}

/// The winning copy of `name` in the merged view: the last match, the form
/// global script lumps take.
fn merged_copy(session: &Session, name: LumpName) -> Option<MergedLump> {
    session
        .merged()
        .into_iter()
        .rev()
        .find(|found| found.lump.name == name)
}

/// A lump payload that must be valid `UTF-8` text.
///
/// # Errors
///
/// [`CyberError::Format`] when the bytes are not `UTF-8`.
fn decode_text(name: LumpName, lump: &Lump) -> Result<String, CyberError> {
    String::from_utf8(lump.data.to_vec()).map_err(|error| {
        CyberError::Format(format!(
            "lump {name} is not valid UTF-8 text ({error}): this editor stores UTF-8 only; \
             read the bytes with get_lump format=hex"
        ))
    })
}

/// The base text the operation runs on: the merged copy decoded strictly,
/// or `None` (meaning "start empty") when a creating operation runs against
/// an absent lump.
///
/// # Errors
///
/// [`CyberError::Format`] for a non-`UTF-8` lump, [`CyberError::NotFound`]
/// for a line operation against no lump at all.
fn text_base(
    session: &Session,
    tool: &TextTool,
    name: LumpName,
    op: &TextOp,
) -> Result<Option<String>, CyberError> {
    match merged_copy(session, name) {
        Some(found) => decode_text(name, &found.lump).map(Some),
        None if op.is_wholesale() => Ok(None),
        None => Err(CyberError::NotFound(format!(
            "lump {name} is in none of the loaded archives: {tool} with op replace_all or \
             append creates it in the working archive; get_lump lists what exists",
            tool = tool.name
        ))),
    }
}

/// `edit_decorate`/`edit_zscript`'s synchronous work: compute the new text,
/// then commit exactly one undoable write.
///
/// # Errors
///
/// [`text_base`]' refusals and [`text_edit::apply`]' refusals (the session
/// does not change), and [`crate::session::Session::edit`]'s error
/// pass-through.
fn text_edit_run(
    session: &mut Session,
    tool: &TextTool,
    name: LumpName,
    op: &TextOp,
) -> Result<TextReport, CyberError> {
    let at = session.working().find_last(name);
    let base = text_base(session, tool, name, op)?;
    let outcome = text_edit::apply(base.as_deref().unwrap_or(""), op)?;
    let created = at.is_none();
    let label = format!("{} {name}", tool.name);
    let body = outcome.text.clone();
    session.edit(&label, |wad| {
        write_lump(wad, name, at, &body);
        Ok(())
    })?;
    Ok(TextReport {
        lump: name,
        lines: outcome.lines,
        changed_range: outcome.changed_range,
        excerpt: outcome.excerpt,
        warnings: outcome.warnings,
        created,
    })
}

/// One text tool call: resolve the name, run the edit, shape the result.
fn run_text_tool(
    state: &AppState,
    tool: &TextTool,
    lump: Option<&str>,
    op: &TextOp,
) -> Result<CallToolResult, ErrorData> {
    let outcome = text_lump_name(tool, lump)
        .and_then(|name| state.with_session(|session| text_edit_run(session, tool, name, op)));
    match outcome {
        Ok(found) => ok(&found),
        Err(error) => fail(&error),
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = scripts_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reads one DeHackEd/BEX patch lump as structured data.
    #[tool(
        description = "Reads one DeHackEd/BEX patch lump as structured data: the patch header, every Thing/Weapon/Ammo/Momentary block with its parsed fields, the BEX sections, parse warnings and optionally the raw text. name defaults to DEHACKED and scope picks the archive. Read it before edit_dehacked so your patch applies to what is really there."
    )]
    fn get_dehacked(
        &self,
        Parameters(args): Parameters<GetDehackedParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let outcome = self.state().with_session(|session| get_run(session, &args));
        match outcome {
            Ok(found) => ok(&found),
            Err(error) => fail(&error),
        }
    }

    /// Edits one DeHackEd/BEX patch lump as one undoable write.
    #[tool(
        description = "Edits one DeHackEd/BEX patch lump in the working archive as exactly one undoable edit: name defaults to DEHACKED and a copy a lower layer holds comes over as a working override. Pass exactly one mode: text or source replace the whole lump, set applies line-oriented {block, key, value} updates and remove drops blocks or keys. get_thing_types then answers the changed table."
    )]
    async fn edit_dehacked(
        &self,
        Parameters(args): Parameters<EditDehackedParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = match lump_name(args.name.as_deref()) {
            Ok(name) => name,
            Err(error) => return fail(&error),
        };
        let draft = match plan(&args) {
            Ok(draft) => draft,
            Err(error) => return fail(&error),
        };
        let body = match resolve(self.state(), draft).await {
            Ok(body) => body,
            Err(error) => return fail(&error),
        };
        let outcome = self
            .state()
            .with_session(|session| edit_run(session, name, body));
        match outcome {
            Ok(found) => ok(&found),
            Err(error) => fail(&error),
        }
    }

    /// Edits one `DECORATE` script lump as text, as one undoable write.
    #[tool(
        description = "Edits one DECORATE script lump in the working archive as exactly one undoable edit: lump defaults to DECORATE and a lower layer's copy is followed by a working override. Pass exactly one op (append, insert_lines, delete_lines or replace_all) over the script text. Read it with get_lump first; get_thing_types re-parses the merged texts, so new actors answer as types."
    )]
    fn edit_decorate(
        &self,
        Parameters(args): Parameters<EditDecorateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        run_text_tool(self.state(), &EDIT_DECORATE, args.lump.as_deref(), &args.op)
    }

    /// Edits one `ZScript` lump as text, as one undoable write.
    #[tool(
        description = "Edits one ZScript lump in the working archive as exactly one undoable edit: lump defaults to ZSCRIPT (pass any name for an #include-d file) and a lower layer's copy is followed by a working override. Pass exactly one op (append, insert_lines, delete_lines or replace_all). get_thing_types follows includes by name, so classes you write appear in the type list."
    )]
    fn edit_zscript(
        &self,
        Parameters(args): Parameters<EditZscriptParams>,
    ) -> Result<CallToolResult, ErrorData> {
        run_text_tool(self.state(), &EDIT_ZSCRIPT, args.lump.as_deref(), &args.op)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        Body, Draft, EDIT_DECORATE, EDIT_ZSCRIPT, EditDehackedParams, Op, TextOp, apply_ops,
        changed_against, edit_run, find_lump, lump_name, parse_scope, plan, run_text_tool,
        text_edit_run, text_lump_name,
    };
    use crate::error::CyberError;
    use crate::session::{Layer, LayerRole, Scope, Session};
    use crate::state::AppState;
    use crate::tools::lumps::rows::build_rows;
    use crate::wad::archive::{Lump, Wad, WadKind};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A lump named `text` carrying `body`.
    fn carrying(text: &str, body: &str) -> Lump {
        Lump::new(name(text), body.as_bytes().to_vec())
    }

    /// The error code of a [`CyberError`], for compact asserts.
    fn code(error: &CyberError) -> &str {
        error.code()
    }

    /// An empty `edit_dehacked` argument set.
    fn args() -> EditDehackedParams {
        EditDehackedParams {
            name: None,
            text: None,
            source: None,
            set: None,
            remove: None,
        }
    }

    /// A session: a base layer holding a `DEHACKED` global.
    fn loaded() -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![carrying("DEHACKED", "Thing 1 (Troop)\n")],
            },
        ));
        session
    }

    #[test]
    fn the_default_name_is_dehacked_and_bad_names_are_refused() {
        assert_eq!(
            lump_name(None).expect("the default"),
            name("DEHACKED"),
            "the documented default"
        );
        assert_eq!(
            code(&lump_name(Some("TOO LONG")).expect_err("nine characters")),
            "invalid_argument",
            "bad name code"
        );
    }

    #[test]
    fn scope_defaults_to_merged_and_refuses_other_names() {
        assert_eq!(parse_scope(None).expect("default"), Scope::Merged, "merged");
        assert_eq!(
            parse_scope(Some("working")).expect("working"),
            Scope::Working,
            "working"
        );
        assert_eq!(
            code(&parse_scope(Some("nope")).expect_err("not a scope")),
            "invalid_argument",
            "bad scope code"
        );
    }

    #[test]
    fn find_lump_picks_the_winning_copy_and_names_a_miss() {
        let session = loaded();
        let row = find_lump(&session, name("DEHACKED"), Scope::Merged).expect("the layer copy");
        assert_eq!(
            row.source,
            crate::session::Source::Layer(0),
            "from the base"
        );
        let error = find_lump(&session, name("DEHACKED"), Scope::Working)
            .expect_err("the working archive is empty");
        assert_eq!(code(&error), "not_found", "missing lump code");
        assert!(
            error.to_string().contains("edit_dehacked"),
            "the message names the tool that creates it: {error}"
        );
    }

    #[test]
    fn rows_carry_the_classifications_the_report_builds_on() {
        let session = loaded();
        let rows = build_rows(&session, Scope::Merged).expect("merged rows");
        assert_eq!(rows.len(), 1, "one lump");
    }

    #[test]
    fn plan_requires_exactly_one_mode() {
        assert_eq!(
            code(&plan(&args()).expect_err("no mode")),
            "invalid_argument",
            "none"
        );
        let both = EditDehackedParams {
            text: Some("x".to_owned()),
            source: Some("/x/y.deh".to_owned()),
            ..args()
        };
        assert_eq!(
            code(&plan(&both).expect_err("two modes")),
            "invalid_argument",
            "two"
        );
        let body = EditDehackedParams {
            text: Some("Patch File for DeHackEd v3.0\n".to_owned()),
            ..args()
        };
        assert!(
            matches!(plan(&body).expect("text mode"), Draft::Replace(text) if text.starts_with("Patch")),
            "the text mode is taken as-is"
        );
        let fetched = EditDehackedParams {
            source: Some("/x/y.deh".to_owned()),
            ..args()
        };
        assert!(
            matches!(plan(&fetched).expect("source mode"), Draft::Fetch(raw) if raw == "/x/y.deh"),
            "the source mode defers the fetch"
        );
    }

    #[test]
    fn plan_validates_set_entries_up_front() {
        let empty = EditDehackedParams {
            set: Some(Vec::new()),
            ..args()
        };
        assert_eq!(
            code(&plan(&empty).expect_err("empty set")),
            "invalid_argument",
            "empty list"
        );
        let bad = EditDehackedParams {
            set: Some(vec![super::SetEntry {
                block: "Gong".to_owned(),
                key: "X".to_owned(),
                value: "1".to_owned(),
            }]),
            ..args()
        };
        assert_eq!(
            code(&plan(&bad).expect_err("bad block")),
            "invalid_argument",
            "bad block"
        );
        let multiline = EditDehackedParams {
            set: Some(vec![super::SetEntry {
                block: "Misc".to_owned(),
                key: "X".to_owned(),
                value: "1\n2".to_owned(),
            }]),
            ..args()
        };
        assert_eq!(
            code(&plan(&multiline).expect_err("newline value")),
            "invalid_argument",
            "newline value"
        );
    }

    #[test]
    fn plan_validates_remove_entries_and_keeps_absent_keys() {
        let empty_remove = EditDehackedParams {
            remove: Some(Vec::new()),
            ..args()
        };
        assert_eq!(
            code(&plan(&empty_remove).expect_err("empty remove")),
            "invalid_argument",
            "empty remove list"
        );
        let empty_key = EditDehackedParams {
            remove: Some(vec![super::RemoveEntry {
                block: "Misc".to_owned(),
                key: Some(" ".to_owned()),
            }]),
            ..args()
        };
        assert_eq!(
            code(&plan(&empty_key).expect_err("blank key")),
            "invalid_argument",
            "blank key"
        );
        let ok = EditDehackedParams {
            remove: Some(vec![super::RemoveEntry {
                block: "[STRINGS]".to_owned(),
                key: None,
            }]),
            ..args()
        };
        match plan(&ok).expect("a whole-section removal") {
            Draft::Ops(ops) => assert!(
                matches!(ops.first(), Some(Op::Remove { key: None, .. })),
                "the key stays absent"
            ),
            Draft::Replace(_) | Draft::Fetch(_) => panic!("the remove mode plans operations"),
        }
    }

    #[test]
    fn changed_lines_counts_positional_differences_and_new_text() {
        assert_eq!(
            changed_against(Some("a\nb\n"), "a\nx\n"),
            1,
            "one line differs"
        );
        assert_eq!(
            changed_against(Some("a\nb\n"), "a\nb\n"),
            0,
            "identical texts"
        );
        assert_eq!(
            changed_against(None, "a\nb\n"),
            2,
            "an absent base is all new"
        );
        assert_eq!(
            changed_against(Some("a\nb\nc\n"), "a\n"),
            2,
            "dropped lines count"
        );
    }

    #[test]
    fn apply_ops_creates_a_header_bearing_patch_for_a_set_on_nothing() {
        let ops = vec![Op::Set {
            block: "Thing 12".to_owned(),
            key: "ID #".to_owned(),
            value: "20000".to_owned(),
        }];
        let (text, changed) = apply_ops(None, &ops, name("DEHACKED")).expect("a create");
        assert!(
            text.starts_with("Patch File for DeHackEd v3.0\n"),
            "the canonical header: {text}"
        );
        assert!(
            text.contains("Thing 12\nID # = 20000\n"),
            "the block landed: {text}"
        );
        assert_eq!(changed, text.lines().count(), "everything is new");
    }

    #[test]
    fn apply_ops_refuses_removals_from_an_absent_lump() {
        let ops = vec![Op::Remove {
            block: "Thing 12".to_owned(),
            key: None,
        }];
        let error = apply_ops(None, &ops, name("DEHACKED")).expect_err("nothing to remove");
        assert_eq!(code(&error), "not_found", "absent lump code");
        assert!(
            error.to_string().contains("none of the loaded"),
            "the message: {error}"
        );
    }

    #[test]
    fn apply_ops_chains_and_aborts_at_the_first_missing_key() {
        let base = "Misc\nfoo = 1\n";
        let ops = vec![
            Op::Set {
                block: "Misc".to_owned(),
                key: "bar".to_owned(),
                value: "2".to_owned(),
            },
            Op::Remove {
                block: "Misc".to_owned(),
                key: Some("nope".to_owned()),
            },
        ];
        let error = apply_ops(Some(base), &ops, name("DEHACKED")).expect_err("key missing");
        assert_eq!(code(&error), "not_found", "missing key code");
    }

    #[test]
    fn edit_run_appends_the_override_and_undo_takes_it_back() {
        let mut session = loaded();
        let body = Body::Replace("Thing 12\nID # = 20000\n".to_owned());
        let report = edit_run(&mut session, name("DEHACKED"), body).expect("an override");
        assert!(report.created, "the layer copy was copied-on-write");
        assert_eq!(report.lines, 2, "two lines");
        assert_eq!(report.changed_lines, 2, "one line replaced, one added");
        assert_eq!(
            session.undo_labels(),
            vec!["edit_dehacked"],
            "the one labelled edit"
        );
        assert_eq!(
            session.working().len(),
            1,
            "the override sits in the working archive"
        );
        session.undo().expect("undo the edit");
        assert!(session.working().is_empty(), "the override is gone");
    }

    #[test]
    fn edit_run_overwrites_an_existing_working_copy_in_place() {
        let mut session = loaded();
        session
            .edit("seed", |wad| {
                wad.lumps.push(carrying("DEHACKED", "Misc\nfoo = 1\n"));
                Ok(())
            })
            .expect("a plain append");
        let body = Body::Ops(vec![Op::Set {
            block: "Misc".to_owned(),
            key: "foo".to_owned(),
            value: "2".to_owned(),
        }]);
        let report = edit_run(&mut session, name("DEHACKED"), body).expect("an overwrite");
        assert!(!report.created, "the working copy was edited in place");
        assert_eq!(report.changed_lines, 1, "one line changed");
        assert_eq!(report.lines, 2, "still two lines");
        let text = String::from_utf8(
            session
                .working()
                .lumps
                .last()
                .expect("the lump")
                .data
                .to_vec(),
        )
        .expect("text");
        assert!(text.contains("foo = 2\n"), "the new value: {text}");
    }

    #[test]
    fn an_absent_lump_still_takes_a_full_replace() {
        let mut session = Session::default();
        let report = edit_run(
            &mut session,
            name("MYPATCH"),
            Body::Replace("Misc\na = 1\n".to_owned()),
        )
        .expect("a create");
        assert!(report.created, "the lump is new");
        assert_eq!(report.changed_lines, 2, "all lines are new");
        assert_eq!(session.working().len(), 1, "one lump was appended");
    }

    /// An `IWAD`-layer session holding one `DECORATE` lump.
    fn with_decorate(body: &str) -> Session {
        let mut session = Session::default();
        session.set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps: vec![carrying("DECORATE", body)],
            },
        ));
        session
    }

    #[test]
    fn text_lump_names_default_per_tool_and_refuse_junk() {
        assert_eq!(
            text_lump_name(&EDIT_DECORATE, None).expect("the default"),
            name("DECORATE"),
            "the decorate default"
        );
        assert_eq!(
            text_lump_name(&EDIT_ZSCRIPT, None).expect("the default"),
            name("ZSCRIPT"),
            "the zscript default"
        );
        assert_eq!(
            text_lump_name(&EDIT_ZSCRIPT, Some("zscript2")).expect("an include file"),
            name("ZSCRIPT2"),
            "case folds"
        );
        assert_eq!(
            code(&text_lump_name(&EDIT_DECORATE, Some("TOO LONG")).expect_err("nine characters")),
            "invalid_argument",
            "bad name code"
        );
    }

    #[test]
    fn text_edit_run_creates_a_missing_lump_by_appending() {
        let mut session = Session::default();
        let op = TextOp::Append {
            text: "actor Test 20001 { }\n".to_owned(),
        };
        let report =
            text_edit_run(&mut session, &EDIT_DECORATE, name("DECORATE"), &op).expect("a create");
        assert!(report.created, "the lump is new");
        assert_eq!(report.lines, 1, "one line");
        assert_eq!(report.changed_range, (1, 1), "the added line");
        assert!(
            report.warnings.is_empty(),
            "balanced: {:?}",
            report.warnings
        );
        assert_eq!(
            session.undo_labels(),
            vec!["edit_decorate DECORATE"],
            "the label carries the lump"
        );
        session.undo().expect("undo the create");
        assert!(session.working().is_empty(), "the lump went back");
    }

    #[test]
    fn text_edit_run_refuses_line_ops_on_an_absent_lump() {
        let mut session = Session::default();
        let op = TextOp::DeleteLines { start: 1, end: 1 };
        let error = text_edit_run(&mut session, &EDIT_ZSCRIPT, name("ZSCRIPT"), &op)
            .expect_err("nothing to delete");
        assert_eq!(code(&error), "not_found", "absent lump code");
        assert!(
            error.to_string().contains("replace_all"),
            "the message names the creating ops: {error}"
        );
        assert!(session.undo_labels().is_empty(), "no history was made");
    }

    #[test]
    fn text_edit_run_copies_on_write_then_edits_in_place() {
        let mut session = with_decorate("actor A {\n}\n");
        let op = TextOp::InsertLines {
            line: 2,
            text: "  radius 30".to_owned(),
        };
        let first =
            text_edit_run(&mut session, &EDIT_DECORATE, name("DECORATE"), &op).expect("a write");
        assert!(first.created, "the layer copy was not editable");
        assert_eq!(first.lines, 3, "the insert added a line");
        assert_eq!(first.changed_range, (2, 2), "its line");
        assert!(first.warnings.is_empty(), "still balanced");
        let second = text_edit_run(
            &mut session,
            &EDIT_DECORATE,
            name("DECORATE"),
            &TextOp::ReplaceAll {
                text: "actor A {\n}\n".to_owned(),
            },
        )
        .expect("an in-place edit");
        assert!(!second.created, "the working copy won the merged view");
        assert_eq!(session.working().len(), 1, "one override only");
        assert_eq!(
            session.undo_labels(),
            vec!["edit_decorate DECORATE", "edit_decorate DECORATE"],
            "two labelled steps"
        );
    }

    #[test]
    fn text_edit_run_reports_unbalanced_braces_after_a_replace() {
        let mut session = with_decorate("actor A { }\n");
        let report = text_edit_run(
            &mut session,
            &EDIT_DECORATE,
            name("DECORATE"),
            &TextOp::ReplaceAll {
                text: "actor A {\n".to_owned(),
            },
        )
        .expect("a write");
        assert_eq!(report.warnings.len(), 1, "one note: {:?}", report.warnings);
        assert!(
            report
                .warnings
                .first()
                .is_some_and(|w| w.contains("unbalanced")),
            "the brace note: {:?}",
            report.warnings
        );
    }

    #[test]
    fn a_non_utf8_lump_is_a_format_error_and_changes_nothing() {
        let mut session = Session::default();
        session
            .edit("seed", |wad| {
                wad.lumps
                    .push(Lump::new(name("DECORATE"), vec![0x61_u8, 0xFF_u8]));
                Ok(())
            })
            .expect("a plain append");
        let error = text_edit_run(
            &mut session,
            &EDIT_DECORATE,
            name("DECORATE"),
            &TextOp::Append {
                text: "b".to_owned(),
            },
        )
        .expect_err("not UTF-8");
        assert_eq!(code(&error), "format_error", "non-UTF-8 code");
        assert!(
            error.to_string().contains("get_lump format=hex"),
            "the message points at the byte reader: {error}"
        );
        assert_eq!(
            session.undo_labels(),
            vec!["seed"],
            "the edit left no trace"
        );
    }

    #[test]
    fn run_text_tool_answers_through_the_result_helpers() {
        let state = AppState::default();
        let good = run_text_tool(
            &state,
            &EDIT_DECORATE,
            None,
            &TextOp::ReplaceAll {
                text: "actor A { }\n".to_owned(),
            },
        )
        .expect("a protocol-level success");
        assert_ne!(good.is_error, Some(true), "the edit ran");
        let content = good.structured_content.expect("structured content");
        assert_eq!(content["lump"], json!("DECORATE"), "the lump field");
        assert_eq!(content["created"], json!(true), "it created the lump");
        let bad = run_text_tool(
            &state,
            &EDIT_DECORATE,
            Some("TOO LONG"),
            &TextOp::Append {
                text: "x".to_owned(),
            },
        )
        .expect("a protocol-level answer");
        assert_eq!(bad.is_error, Some(true), "a bad name is a domain error");
        assert_eq!(
            bad.structured_content.expect("error content")["error"],
            json!("invalid_argument"),
            "the code field"
        );
    }
}
