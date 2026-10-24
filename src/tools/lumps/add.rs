//! `add_lump` and the two import tools: their arguments, the payload they
//! carry and the one undoable insert that commits it.
//!
//! [`AddLumpParams`] and [`ImportParams`] are the wire forms;
//! [`plan_add`] and [`plan_import`] validate them into a [`PendingEdit`]
//! whose bytes [`prepare`] brings in (a `Payload::Source` is fetched *before*
//! any session lock, an import's [`chosen_name`] derived from the fetched
//! file name). [`apply`] then runs exactly one
//! [`crate::session::Session::edit`] through
//! [`crate::tools::lumps::place`], refusing a same-name twin with
//! [`clash_message`] unless `allow_duplicate` says otherwise; [`commit`] and
//! [`imported`] are the two shapes the router calls.

use rmcp::ErrorData;
use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assets::fetch::derive_lump_name;
use crate::error::CyberError;
use crate::session::Session;
use crate::state::AppState;
use crate::tools::lumps::parse::hex_bytes;
use crate::tools::lumps::place::{self, Placement, PlacementParams, parse_placement};
use crate::tools::{fail, ok};
use crate::wad::archive::Lump;
use crate::wad::classify::Namespace;
use crate::wad::name::LumpName;

/// Arguments for [`Cyberdemon::add_lump`]: the name, at most one payload
/// field, the shared placement and the duplicate rule.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct AddLumpParams {
    /// Name for the new lump: 1 to 8 printable ASCII characters, matched
    /// case-insensitively (it is uppercased).
    pub(super) name: String,
    /// Payload as UTF-8 text, written as-is. At most one of `text`, `hex`
    /// or `source`.
    #[serde(default)]
    pub(super) text: Option<String>,
    /// Payload as hex bytes, spaces allowed (e.g. `"4D 55 53 1A"`). At
    /// most one of `text`, `hex` or `source`.
    #[serde(default)]
    pub(super) hex: Option<String>,
    /// Absolute path, `file://` URI, `http(s)://` URL or uploaded asset
    /// URL whose bytes become the payload. At most one of `text`, `hex`
    /// or `source`.
    #[serde(default)]
    pub(super) source: Option<String>,
    /// Where the lump goes: at most one of `index`, `before`, `after`,
    /// `namespace` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    pub(super) placement: PlacementParams,
    /// Add the lump even when the working archive already holds one with
    /// this name in the same namespace (default `false`).
    #[serde(default)]
    pub(super) allow_duplicate: bool,
}

/// Arguments for [`Cyberdemon::import_asset`] and
/// [`Cyberdemon::import_file_as_lump`]: the source to fetch, an optional
/// name, the shared placement and the duplicate rule.
#[derive(Debug, Deserialize, JsonSchema)]
pub(super) struct ImportParams {
    /// Source to fetch: absolute path, `file://` URI, `http(s)://` URL or
    /// an uploaded asset URL. Its bytes are inserted without conversion.
    pub(super) source: String,
    /// Lump name to store the bytes under; defaults to the fetched file
    /// name's stem uppercased and trimmed to 8 characters. When nothing
    /// usable remains the call fails asking for this name.
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Where the lump goes: at most one of `index`, `before`, `after`,
    /// `namespace` (see `skill/reference/lumps.md`).
    #[serde(flatten)]
    pub(super) placement: PlacementParams,
    /// Import even when the working archive already holds one with this
    /// name in the same namespace (default `false`).
    #[serde(default)]
    pub(super) allow_duplicate: bool,
}

/// How the caller supplied a new lump's bytes.
#[derive(Debug)]
pub(super) enum Payload {
    /// No data field: the lump will be an empty marker.
    Empty,
    /// The `text` argument, kept as UTF-8 bytes.
    Text(String),
    /// The `hex` argument, waiting to be decoded.
    Hex(String),
    /// A `source` to fetch.
    Source(String),
}

/// A validated editing call whose payload may still need fetching.
#[derive(Debug)]
pub(super) struct PendingEdit {
    /// The undo-label verb: `add_lump` or `import`.
    pub(super) verb: &'static str,
    /// The lump name when the arguments carried one; imports otherwise
    /// derive it from the fetched file name.
    pub(super) name: Option<LumpName>,
    /// Where the payload bytes come from.
    pub(super) payload: Payload,
    /// Where the lump goes in the working archive.
    pub(super) placement: Placement,
    /// Whether a same-name twin in the same namespace is allowed.
    pub(super) allow_duplicate: bool,
    /// The `source` to echo back (the import tools only).
    pub(super) source: Option<String>,
}

/// An editing call with its payload bytes in hand, ready to commit.
pub(super) struct Prepared {
    /// The undo-label verb (`add_lump` or `import`).
    pub(super) verb: &'static str,
    /// The name to insert under.
    pub(super) name: LumpName,
    /// The payload bytes.
    pub(super) data: Vec<u8>,
    /// Where the lump goes in the working archive.
    pub(super) placement: Placement,
    /// Whether a same-name twin in the same namespace is allowed.
    pub(super) allow_duplicate: bool,
    /// The `source` to echo back (the import tools only).
    pub(super) source: Option<String>,
}

/// Structured result of the three editing tools.
#[derive(Debug, Serialize)]
pub(super) struct EditReport {
    /// The name the lump was added under.
    pub(super) name: LumpName,
    /// Its directory position in the working archive.
    pub(super) index: usize,
    /// Payload bytes.
    pub(super) size: usize,
    /// The region it sits in, `null` for a loose lump.
    pub(super) namespace: Option<Namespace>,
    /// The fetched source; only the import tools carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) source: Option<String>,
}

/// The at-most-one-of rule for the three `add_lump` payload fields.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when more than one is given.
pub(super) fn pick_payload(
    text: Option<&str>,
    hex: Option<&str>,
    source: Option<&str>,
) -> Result<Payload, CyberError> {
    match (text, hex, source) {
        (None, None, None) => Ok(Payload::Empty),
        (Some(body), None, None) => Ok(Payload::Text(body.to_owned())),
        (None, Some(digits), None) => Ok(Payload::Hex(digits.to_owned())),
        (None, None, Some(raw)) => Ok(Payload::Source(raw.to_owned())),
        _ => Err(CyberError::InvalidArgument(
            "add_lump takes at most one of text, hex and source: pick one payload form".to_owned(),
        )),
    }
}

/// Validate [`Cyberdemon::add_lump`]'s arguments before any fetching.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a bad name, more than one payload
/// field or more than one placement field.
pub(super) fn plan_add(args: &AddLumpParams) -> Result<PendingEdit, CyberError> {
    Ok(PendingEdit {
        verb: "add_lump",
        name: Some(LumpName::parse(&args.name)?),
        payload: pick_payload(
            args.text.as_deref(),
            args.hex.as_deref(),
            args.source.as_deref(),
        )?,
        placement: parse_placement(&args.placement)?,
        allow_duplicate: args.allow_duplicate,
        source: None,
    })
}

/// Validate the import tools' arguments before any fetching.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for an explicit name that does not
/// parse or more than one placement field.
pub(super) fn plan_import(args: &ImportParams) -> Result<PendingEdit, CyberError> {
    Ok(PendingEdit {
        verb: "import",
        name: args.name.as_deref().map(LumpName::parse).transpose()?,
        payload: Payload::Source(args.source.clone()),
        placement: parse_placement(&args.placement)?,
        allow_duplicate: args.allow_duplicate,
        source: Some(args.source.clone()),
    })
}

/// The name to insert under: the explicit one, else one derived from the
/// fetched file name.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] asking for `name` when nothing usable
/// can be derived.
pub(super) fn chosen_name(
    name: Option<LumpName>,
    filename: Option<&str>,
) -> Result<LumpName, CyberError> {
    if let Some(explicit) = name {
        return Ok(explicit);
    }
    let file = filename.unwrap_or_default();
    derive_lump_name(file).ok_or_else(|| {
        CyberError::InvalidArgument(format!(
            "cannot derive a lump name from {file:?}: pass name (1 to 8 printable ASCII \
             characters)"
        ))
    })
}

/// Bring the payload bytes in, fetching a `source` before the session lock
/// is ever taken.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for bad hex or an underivable import
/// name; [`AppState::fetch_source`]'s errors pass through.
async fn prepare(state: &AppState, pending: PendingEdit) -> Result<Prepared, CyberError> {
    let PendingEdit {
        verb,
        name,
        payload,
        placement,
        allow_duplicate,
        source,
    } = pending;
    let (data, filename) = match payload {
        Payload::Empty => (Vec::new(), None),
        Payload::Text(body) => (body.into_bytes(), None),
        Payload::Hex(digits) => (hex_bytes("hex", &digits)?, None),
        Payload::Source(raw) => {
            let fetched = state.fetch_source(&raw).await?;
            (fetched.bytes, Some(fetched.filename))
        }
    };
    Ok(Prepared {
        verb,
        name: chosen_name(name, filename.as_deref())?,
        data,
        placement,
        allow_duplicate,
        source,
    })
}

/// The `already_exists` text for a same-name twin, naming its region.
pub(super) fn clash_message(name: LumpName, namespace: Option<Namespace>) -> String {
    let region = namespace.map_or_else(
        || "the global area".to_owned(),
        |wanted| format!("the {} region", wanted.as_str()),
    );
    format!(
        "lump {name} already exists in {region} of the working archive: pass allow_duplicate \
         true to add a second copy beside it or use replace_lump to overwrite the existing one"
    )
}

/// The synchronous half of an edit: one undoable insert into the working
/// archive plus the duplicate refusal (a refusal rolls the insert back).
///
/// # Errors
///
/// [`place::insert`]'s errors and [`CyberError::AlreadyExists`] for a
/// same-name twin in the same namespace when `allow_duplicate` is `false`.
pub(super) fn apply(session: &mut Session, edit: &Prepared) -> Result<EditReport, CyberError> {
    let label = format!("{} {}", edit.verb, edit.name);
    let size = edit.data.len();
    let (index, namespace) = session.edit(&label, |wad| {
        let lump = Lump::new(edit.name, edit.data.clone());
        let at = place::insert(wad, &edit.placement, lump)?;
        let here = place::inspect(wad, at);
        if !edit.allow_duplicate && here.clashes {
            return Err(CyberError::AlreadyExists(clash_message(
                edit.name,
                here.namespace,
            )));
        }
        Ok((at, here.namespace))
    })?;
    Ok(EditReport {
        name: edit.name,
        index,
        size,
        namespace,
        source: edit.source.clone(),
    })
}

/// Run one editing tool end to end: fetch the payload (never holding the
/// session lock), then commit the undoable edit.
///
/// # Errors
///
/// Whatever [`prepare`] or [`apply`] reports.
pub(super) async fn commit(
    state: &AppState,
    pending: PendingEdit,
) -> Result<EditReport, CyberError> {
    let prepared = prepare(state, pending).await?;
    state.with_session(|session| apply(session, &prepared))
}

/// The shared body of [`Cyberdemon::import_asset`] and
/// [`Cyberdemon::import_file_as_lump`].
pub(super) async fn imported(
    state: &AppState,
    args: &ImportParams,
) -> Result<CallToolResult, ErrorData> {
    let pending = match plan_import(args) {
        Ok(pending) => pending,
        Err(error) => return fail(&error),
    };
    match commit(state, pending).await {
        Ok(report) => ok(&report),
        Err(error) => fail(&error),
    }
}
