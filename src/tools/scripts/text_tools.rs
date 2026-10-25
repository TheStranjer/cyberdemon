//! The two script tools: `edit_decorate` and `edit_zscript` over one
//! [`TextOp`] and one undoable write.
//!
//! A [`TextTool`] carries what the two differ in - the tool name (also the
//! prefix of the undo label) and the default lump - and every other step is
//! shared: [`text_lump_name`] resolves the name, [`text_base`] reads the
//! winning merged copy strictly as `UTF-8` ([`merged_copy`],
//! [`decode_text`]), [`text_edit_run`] runs [`text_edit::apply`] over it and
//! commits the result with the patch write's [`write_lump`] in one
//! [`crate::session::Session::edit`], and [`run_text_tool`] shapes the
//! answer through the group's result helpers. The editor itself is
//! [`text_edit`]; the `DeHackEd` pair lives in [`mod@super::get_dehacked`]
//! and [`mod@super::edit_dehacked`] and the router in [`super`].

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CyberError;
use crate::session::{MergedLump, Session};
use crate::state::AppState;
use crate::tools::reply;
use crate::wad::archive::Lump;
use crate::wad::name::LumpName;

use super::edit_dehacked::write_lump;
use super::text_edit::{self, TextOp};

/// Which text-lump tool is running: its name and default lump.
pub(super) struct TextTool {
    /// The tool name, also the prefix of the undo label.
    name: &'static str,
    /// The lump edited when the `lump` argument is left out.
    default_lump: &'static str,
}

/// The [`Cyberdemon::edit_decorate`] descriptor.
pub(super) const EDIT_DECORATE: TextTool = TextTool {
    name: "edit_decorate",
    default_lump: "DECORATE",
};

/// The [`Cyberdemon::edit_zscript`] descriptor.
pub(super) const EDIT_ZSCRIPT: TextTool = TextTool {
    name: "edit_zscript",
    default_lump: "ZSCRIPT",
};

/// Arguments for [`Cyberdemon::edit_decorate`]: the optional lump name plus
/// exactly one [`TextOp`], flattened through its `op` tag.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct EditDecorateParams {
    /// Text lump to edit: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively. Defaults to `"DECORATE"`.
    #[serde(default)]
    pub(super) lump: Option<String>,
    /// The single operation to apply, selected by its `op` field.
    #[serde(flatten)]
    pub(super) op: TextOp,
}

/// Arguments for [`Cyberdemon::edit_zscript`]: the optional lump name plus
/// exactly one [`TextOp`], flattened through its `op` tag.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct EditZscriptParams {
    /// Text lump to edit: 1 to 8 printable `ASCII` characters, matched
    /// case-insensitively. Defaults to `"ZSCRIPT"`; pass the name of an
    /// `#include`d lump (e.g. `"ZSCRIPT2"`) to edit that file instead.
    #[serde(default)]
    pub(super) lump: Option<String>,
    /// The single operation to apply, selected by its `op` field.
    #[serde(flatten)]
    pub(super) op: TextOp,
}

/// Structured result of [`Cyberdemon::edit_decorate`] and
/// [`Cyberdemon::edit_zscript`].
#[derive(Debug, Serialize)]
pub(super) struct TextReport {
    /// The lump that was written.
    pub(super) lump: LumpName,
    /// Total lines of the resulting text.
    pub(super) lines: usize,
    /// The 1-based inclusive range of changed lines, `[0, 0]` when nothing
    /// changed.
    pub(super) changed_range: (usize, usize),
    /// The changed lines with two lines of context, each numbered.
    pub(super) excerpt: String,
    /// Non-fatal notes about the result (e.g. unbalanced braces).
    pub(super) warnings: Vec<String>,
    /// Whether the lump was created in the working archive (it was absent,
    /// or held only by a read-only layer that was copied-on-write).
    pub(super) created: bool,
}

/// The lump name a text tool acts on, defaulting per [`TextTool`].
///
/// # Errors
///
/// [`LumpName::parse`]'s [`CyberError::InvalidArgument`] for a name that
/// does not parse.
pub(super) fn text_lump_name(tool: &TextTool, text: Option<&str>) -> Result<LumpName, CyberError> {
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
pub(super) fn text_edit_run(
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
pub(super) fn run_text_tool(
    state: &AppState,
    tool: &TextTool,
    lump: Option<&str>,
    op: &TextOp,
) -> Result<CallToolResult, ErrorData> {
    let outcome = text_lump_name(tool, lump)
        .and_then(|name| state.with_session(|session| text_edit_run(session, tool, name, op)));
    reply(outcome)
}
