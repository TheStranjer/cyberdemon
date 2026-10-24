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
//!
//! The work is split by tool: this root keeps what the three share - the
//! default lump and the undo label, the lump-name and scope helpers, the
//! directory lookup and the router - while [`mod@get_dehacked`] carries the
//! read (its parameters, the report rows and [`get_dehacked::get_run`]),
//! [`mod@edit_dehacked`] the patch write (its parameters, the draft, the
//! validated [`edit_dehacked::Op`] list and [`edit_dehacked::edit_run`]) and
//! [`mod@text_tools`] the two script tools (their [`text_tools::TextTool`]
//! descriptors, parameters, report and [`text_tools::run_text_tool`]) over
//! the shared [`text_edit`] engine. The group's unit tests live in
//! `scripts/tests.rs`.

mod edit_dehacked;
mod get_dehacked;
mod text_edit;
mod text_tools;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData, tool, tool_router};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::{Scope, Session};
use crate::tools::lumps::rows::{LumpRow, build_rows};
use crate::tools::{fail, ok};
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
mod tests;
