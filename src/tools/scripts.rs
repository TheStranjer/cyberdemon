//! Script tools: reading and editing the DeHackEd/BEX patch lumps and the
//! `DECORATE`/`ZSCRIPT` script lumps a session holds.
//!
//! The two encodings differ on purpose: `DeHackEd` is a byte-oriented
//! format, so the patch pair reads lump bytes with [`decode_patch`] (one
//! `Latin-1` byte per character, every byte value valid) and writes them
//! back with [`encode_patch`], byte-for-byte where the text allows; the
//! script editors keep their strict `UTF-8` rule and refuse other bytes.
//! Neither ever writes lossy text.
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
//!
//! The work is split by tool: this root keeps what the three share - the
//! default lump and the undo label, the lump-name and scope helpers, the
//! `Latin-1` patch codecs and the directory lookup beside the router -
//! while [`mod@get_dehacked`] carries the read (its parameters, the report
//! rows and [`get_dehacked::get_run`]), [`mod@edit_dehacked`] the patch
//! write (its parameters, the draft, the validated [`edit_dehacked::Op`]
//! list, its [`mod@line_diff`]-counted report and [`edit_dehacked::edit_run`])
//! and [`mod@text_tools`] the two script tools (their [`text_tools::TextTool`]
//! descriptors, parameters, report and [`text_tools::run_text_tool`]) over
//! the shared [`text_edit`] engine. The group's unit tests live in
//! `scripts/tests.rs`.

mod edit_dehacked;
mod get_dehacked;
mod line_diff;
mod text_edit;
mod text_tools;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::{fail, reply};
use crate::wad::name::LumpName;

use edit_dehacked::{EditDehackedParams, edit_run, plan, resolve};
use get_dehacked::{GetDehackedParams, get_run};
use text_tools::{
    EDIT_DECORATE, EDIT_ZSCRIPT, EditDecorateParams, EditZscriptParams, run_text_tool,
};

/// The lump both tools read and edit when `name` is left out.
const DEFAULT_LUMP: &str = "DEHACKED";

/// The undo label of every `edit_dehacked` call.
const UNDO_LABEL: &str = "edit_dehacked";

/// A `DeHackEd`/BEX patch lump's bytes as text: one `Latin-1` byte per
/// character, so every byte value of a legacy `.deh` file reads as itself
/// and [`encode_patch`] gives it back unchanged.
pub(super) fn decode_patch(data: &[u8]) -> String {
    data.iter()
        .filter_map(|byte| char::from_u32(u32::from(*byte)))
        .collect()
}

/// [`decode_patch`]'s inverse: a character up to `U+00FF` becomes its one
/// `Latin-1` byte and a wider character keeps its `UTF-8` bytes, so no
/// byte and no character is ever replaced.
pub(super) fn encode_patch(text: &str) -> Vec<u8> {
    let mut data = Vec::with_capacity(text.len());
    for ch in text.chars() {
        match u8::try_from(ch) {
            Ok(byte) => data.push(byte),
            Err(_) => data.extend_from_slice(ch.encode_utf8(&mut [0_u8; 4]).as_bytes()),
        }
    }
    data
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
                 edit_dehacked (set creates the lump in the working archive)"
            ))
        })
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = scripts_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Reads one DeHackEd/BEX patch lump as structured data.
    #[tool(
        description = "Reads one DeHackEd/BEX patch lump as structured data: the patch header, every Thing/Weapon/Ammo/Momentary block with its parsed fields, the BEX sections, parse warnings and optionally the raw text (lump bytes are Latin-1 decoded, one character per byte, never replaced). name defaults to DEHACKED and scope picks the archive. Read it before edit_dehacked so your patch applies to what is really there."
    )]
    fn get_dehacked(
        &self,
        Parameters(args): Parameters<GetDehackedParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let outcome = self.state().with_session(|session| get_run(session, &args));
        reply(outcome)
    }

    /// Edits one DeHackEd/BEX patch lump as one undoable write.
    #[tool(
        description = "Edits one DeHackEd/BEX patch lump in the working archive as exactly one undoable edit: name defaults to DEHACKED and a copy a lower layer holds comes over as a working override. Pass exactly one mode: text or source replace the whole lump, set applies line-oriented {block, key, value} updates and remove drops blocks or keys. Patch bytes are byte-oriented Latin-1: every byte the edit does not change is written back unchanged, so legacy .deh files survive. Reports the line diff as added, removed and changed. get_thing_types then answers the changed table."
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
        reply(outcome)
    }

    /// Edits one `DECORATE` script lump as text, as one undoable write.
    #[tool(
        description = "Edits one DECORATE script lump in the working archive as exactly one undoable edit: lump defaults to DECORATE and a lower layer's copy is followed by a working override. Pass exactly one op (append, insert_lines, delete_lines or replace_all) over the script text; script lumps are strict UTF-8 and other bytes are refused as format_error. Read it with get_lump first; get_thing_types re-parses the merged texts, so new actors answer as types."
    )]
    fn edit_decorate(
        &self,
        Parameters(args): Parameters<EditDecorateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        run_text_tool(self.state(), &EDIT_DECORATE, args.lump.as_deref(), &args.op)
    }

    /// Edits one `ZScript` lump as text, as one undoable write.
    #[tool(
        description = "Edits one ZScript lump in the working archive as exactly one undoable edit: lump defaults to ZSCRIPT (pass any name for an #include-d file) and a lower layer's copy is followed by a working override. Pass exactly one op (append, insert_lines, delete_lines or replace_all); script lumps are strict UTF-8 and other bytes are refused as format_error. get_thing_types follows includes by name, so classes you write appear in the type list."
    )]
    fn edit_zscript(
        &self,
        Parameters(args): Parameters<EditZscriptParams>,
    ) -> Result<CallToolResult, ErrorData> {
        run_text_tool(self.state(), &EDIT_ZSCRIPT, args.lump.as_deref(), &args.op)
    }
}

#[cfg(test)]
mod tests;
