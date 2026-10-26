//! Batch tool: `run_batch` executes several tool calls as one atomic,
//! single-undo step and chains their results with `$ref` references.
//!
//! The server is a reflex, and one plain-English request is often five to
//! twenty calls. A batch makes that sequence all-or-nothing: every step runs
//! through this server's own [`ToolRouter`] with the incoming
//! [`RequestContext`], so any tool — including ones added later — is
//! dispatchable without batch-specific code. The whole run happens inside
//! one [`Session::begin_batch`] scope: every inner edit folds into the
//! batch's single history entry as it lands, and [`Session::end_batch`]
//! commits that entry or rolls the batch back to its exact pre-batch
//! archive and revision. The session refuses, as a typed conflict, every
//! operation the fold cannot carry (whole-archive and saved-state swaps);
//! only the steps of the non-nestable tool groups (the batch and history
//! routers, composed by `Cyberdemon::non_nestable_router`) are refused
//! before anything runs.
//!
//! [`RequestContext`]: rmcp::service::RequestContext
//! [`Session::begin_batch`]: crate::session::Session::begin_batch
//! [`Session::end_batch`]: crate::session::Session::end_batch
//! [`ToolRouter`]: rmcp::handler::server::router::tool::ToolRouter

use core::mem;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value, json};

use crate::error::CyberError;
use crate::server::Cyberdemon;
use crate::session::history::{BatchOutcome, BatchToken, HISTORY_LIMIT};
use crate::tools::reply;

/// The largest number of steps one batch may hold.
const MAX_STEPS: usize = 50;
/// The longest undo label a batch keeps, in characters.
const LABEL_LIMIT: usize = 80;
/// The member every domain failure's structured content carries, and whose
/// presence marks a step's result as a failure.
const ERROR_KEY: &str = "error";
/// The error code reported for a step whose result carried no structured
/// domain error (a protocol-level or shape failure, not a tool's `fail`).
const TOOL_ERROR: &str = "tool_error";

/// One step of a [`Cyberdemon::run_batch`] call.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
struct BatchStep {
    /// The tool to call.
    tool: String,
    /// The arguments object for the call, as the tool itself takes it.
    #[serde(default)]
    arguments: Option<Value>,
}

/// Arguments for [`Cyberdemon::run_batch`].
#[derive(Debug, Deserialize, JsonSchema)]
struct RunBatchParams {
    /// The steps to run, in order; 1 to 50 of them.
    steps: Vec<BatchStep>,
    /// Stop at the first failing step and undo everything the batch
    /// recorded (default `true`); `false` runs every step and keeps the
    /// successful edits.
    #[serde(default = "always_atomic")]
    atomic: bool,
    /// The undo label for the whole batch; defaults to the step tool names
    /// and is kept to 80 characters.
    #[serde(default)]
    label: Option<String>,
}

/// The `atomic` default: a batch is all-or-nothing unless said otherwise.
const fn always_atomic() -> bool {
    true
}

/// One step's outcome as the batch reports it.
#[derive(Serialize)]
struct StepOutcome {
    /// The tool the step called.
    tool: String,
    /// Whether the call succeeded.
    ok: bool,
    /// The call's structured content on success; `null` on failure.
    result: Option<Value>,
    /// The call's `{"error", "message", "hint"}` object on failure; `null`
    /// on success.
    error: Option<Value>,
}

impl StepOutcome {
    /// Judges one finished call: a step fails when the result carries
    /// `is_error` or a structured `{"error": ...}` object.
    fn of(tool: &str, outcome: Result<CallToolResponse, ErrorData>) -> Self {
        let result = match outcome {
            Ok(CallToolResponse::Complete(result)) => result,
            Ok(_incomplete) => {
                return Self::synthetic(tool, "the tool asked the client for input");
            }
            Err(protocol) => return Self::synthetic(tool, &protocol.message),
        };
        if is_failure(&result) {
            Self {
                tool: tool.to_owned(),
                ok: false,
                result: None,
                error: Some(error_payload(&result)),
            }
        } else {
            Self {
                tool: tool.to_owned(),
                ok: true,
                result: Some(result.structured_content.unwrap_or(Value::Null)),
                error: None,
            }
        }
    }

    /// The outcome of a step that failed before (or without) a tool result.
    fn synthetic(tool: &str, message: &str) -> Self {
        Self {
            tool: tool.to_owned(),
            ok: false,
            result: None,
            error: Some(json!({
                "error": TOOL_ERROR,
                "message": format!("the step could not run: {message}"),
                "hint": Value::Null,
            })),
        }
    }

    /// The outcome of a step its `$ref` substitution refused.
    fn refused(tool: &str, error: &CyberError) -> Self {
        Self {
            tool: tool.to_owned(),
            ok: false,
            result: None,
            error: Some(crate::tools::error_json(error)),
        }
    }
}

/// Whether a finished call is a failure: `is_error` or a structured
/// `{"error": ...}` object.
fn is_failure(result: &CallToolResult) -> bool {
    if result.is_error == Some(true) {
        return true;
    }
    result
        .structured_content
        .as_ref()
        .is_some_and(|value| value.get(ERROR_KEY).is_some())
}

/// The error payload of a failed call: its structured `{"error", "message",
/// "hint"}` object when it carries one, else a [`TOOL_ERROR`] built from the
/// result's first text block.
fn error_payload(result: &CallToolResult) -> Value {
    let structured = result
        .structured_content
        .as_ref()
        .filter(|value| value.get(ERROR_KEY).is_some());
    structured.cloned().unwrap_or_else(|| {
        let text = result
            .content
            .first()
            .and_then(ContentBlock::as_text)
            .map_or_else(String::new, |item| item.text.clone());
        json!({ "error": TOOL_ERROR, "message": text, "hint": Value::Null })
    })
}

/// The structured result of [`Cyberdemon::run_batch`].
#[derive(Serialize)]
struct BatchReport {
    /// `true` when every planned step ran and succeeded.
    ok: bool,
    /// `true` when a failing atomic step rolled the recorded edits back.
    rolled_back: bool,
    /// The executed steps in order; an atomic batch stops at the first
    /// failure, so later steps are not listed.
    steps: Vec<StepOutcome>,
    /// The undo label of the batch's single history entry; `null` for a
    /// rolled-back batch or one whose steps recorded no edits at all.
    undo_label: Option<String>,
}

/// Refuses a batch before anything runs: the step count, then every step's
/// non-nestable group, unknown name and non-object arguments, each naming
/// the offending step. The whole-archive and saved-state tools are not
/// refused here: the session itself refuses them while the batch is open.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for each refusal above.
fn precheck(steps: &[BatchStep], router: &ToolRouter<Cyberdemon>) -> Result<(), CyberError> {
    if steps.is_empty() || steps.len() > MAX_STEPS {
        return Err(CyberError::InvalidArgument(format!(
            "a batch needs 1..={MAX_STEPS} steps, got {}; split it into smaller batches",
            steps.len(),
        )));
    }
    let blocked = Cyberdemon::non_nestable_router();
    for (index, step) in steps.iter().enumerate() {
        check_step(index, step, router, &blocked)?;
    }
    Ok(())
}

/// Refuses one bad step: a tool of a non-nestable group, an unknown name,
/// or arguments that are neither absent, `null`, nor an object.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming `index` for each refusal.
fn check_step(
    index: usize,
    step: &BatchStep,
    router: &ToolRouter<Cyberdemon>,
    blocked: &ToolRouter<Cyberdemon>,
) -> Result<(), CyberError> {
    if blocked.has_route(&step.tool) {
        return Err(CyberError::InvalidArgument(format!(
            "step {index} calls {}, whose tool group cannot run inside a batch; run it between \
             batches",
            step.tool,
        )));
    }
    if !router.has_route(&step.tool) {
        return Err(CyberError::InvalidArgument(format!(
            "step {index} calls unknown tool {}; list_tools shows the callable names",
            step.tool,
        )));
    }
    if step
        .arguments
        .as_ref()
        .is_some_and(|args| !args.is_object() && !args.is_null())
    {
        return Err(CyberError::InvalidArgument(format!(
            "step {index} passes arguments that are not an object; pass an object or leave them \
             out"
        )));
    }
    Ok(())
}

/// Refuses a batch whose steps could push the undo stack past
/// [`HISTORY_LIMIT`]. The open batch folds its edits into the one entry it
/// commits, so its own bookkeeping cannot overflow; this plan-time guard
/// still refuses a batch whose every step could not have stood as its own
/// undo entry, keeping the documented depth budget the caller plans
/// against.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] naming what to do when the marked depth
/// plus the step count exceeds the history limit.
fn check_history_room(depth: usize, steps: usize) -> Result<(), CyberError> {
    if depth.saturating_add(steps) > HISTORY_LIMIT {
        return Err(CyberError::InvalidArgument(format!(
            "the undo stack holds {depth} of {HISTORY_LIMIT} entries and the batch would record \
             up to {steps} more: undo or save first, or split the batch"
        )));
    }
    Ok(())
}

/// The batch's undo label: the caller's when given, else `batch: tool1,
/// tool2, …` from the step tool names; either is kept to [`LABEL_LIMIT`]
/// characters.
fn batch_label(given: Option<&str>, steps: &[BatchStep]) -> String {
    let names: Vec<&str> = steps.iter().map(|step| step.tool.as_str()).collect();
    let text = given.map_or_else(|| format!("batch: {}", names.join(", ")), str::to_owned);
    text.chars().take(LABEL_LIMIT).collect()
}

/// The `$ref` marker of an argument object: the named reference when the
/// object is exactly `{"$ref": "..."}`, `None` otherwise.
fn ref_marker(map: &JsonMap<String, Value>) -> Option<&str> {
    if map.len() != 1 {
        return None;
    }
    let (key, value) = map.iter().next()?;
    if key != "$ref" {
        return None;
    }
    value.as_str()
}

/// Replaces every exact `{"$ref": "<step index>/<json pointer>"}` object in
/// `value` with the value it names in an earlier step's result, at any
/// nesting depth.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] for a malformed reference, a forward or
/// failed step, and a pointer that does not resolve.
fn resolve_refs(value: Value, results: &[Option<Value>]) -> Result<Value, CyberError> {
    match value {
        Value::Object(map) => resolve_object(map, results),
        Value::Array(items) => resolve_items(items, results),
        scalar @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Null) => Ok(scalar),
    }
}

/// Resolves every element of an arguments array.
fn resolve_items(items: Vec<Value>, results: &[Option<Value>]) -> Result<Value, CyberError> {
    let mut resolved = Vec::with_capacity(items.len());
    for item in items {
        resolved.push(resolve_refs(item, results)?);
    }
    Ok(Value::Array(resolved))
}

/// Resolves one object: the reference it is, or a copy with every member
/// resolved.
fn resolve_object(
    map: JsonMap<String, Value>,
    results: &[Option<Value>],
) -> Result<Value, CyberError> {
    if let Some(text) = ref_marker(&map) {
        return lookup_ref(text, results).cloned();
    }
    let mut resolved = JsonMap::with_capacity(map.len());
    for (key, item) in map {
        resolved.insert(key, resolve_refs(item, results)?);
    }
    Ok(Value::Object(resolved))
}

/// Reads one `"<step index>/<json pointer>"` reference against the results
/// the batch recorded so far.
///
/// # Errors
///
/// [`CyberError::InvalidArgument`] when `text` is malformed, names a step
/// that has not run or failed, or points into nothing.
fn lookup_ref<'record>(
    text: &str,
    results: &'record [Option<Value>],
) -> Result<&'record Value, CyberError> {
    let unusable = |reason: &str| {
        CyberError::InvalidArgument(format!(
            "reference {text:?} is not usable: {reason} (the form is \
             \"<step index>/<json pointer>\", e.g. \"0/door_sector\")"
        ))
    };
    let (index_text, pointer) = text.split_once('/').unwrap_or((text, ""));
    let Ok(index) = index_text.parse::<usize>() else {
        return Err(unusable("the step index is not a number"));
    };
    let Some(record) = results.get(index) else {
        return Err(unusable(
            format!(
                "it names step {index}, which has not run yet; a step may only reference \
                     earlier steps"
            )
            .as_str(),
        ));
    };
    let Some(value) = record.as_ref() else {
        return Err(unusable(
            format!("it names step {index}, which failed and produced no result").as_str(),
        ));
    };
    follow_pointer(value, pointer).ok_or_else(|| {
        unusable(format!("no value at that pointer in step {index}'s result").as_str())
    })
}

/// Walks a `/`-separated JSON pointer (`""` names the whole value) from
/// `value`, segment by segment.
fn follow_pointer<'value>(mut value: &'value Value, pointer: &str) -> Option<&'value Value> {
    if pointer.is_empty() {
        return Some(value);
    }
    for segment in pointer.split('/') {
        value = follow_segment(value, segment)?;
    }
    Some(value)
}

/// One pointer segment against a value: an object member or an array index.
fn follow_segment<'value>(value: &'value Value, segment: &str) -> Option<&'value Value> {
    if let Some(map) = value.as_object() {
        return map.get(segment);
    }
    let items = value.as_array()?;
    let index = segment.parse::<usize>().ok()?;
    items.get(index)
}

/// One running batch: the steps, the results recorded for `$ref`s and the
/// session's open batch scope the steps edit inside.
struct Runner<'server> {
    /// The server whose router runs the steps.
    server: &'server Cyberdemon,
    /// The `run_batch` request's context, forwarded to every step.
    request: RequestContext<RoleServer>,
    /// The steps still to run.
    steps: Vec<BatchStep>,
    /// Whether a failing step rolls the batch back.
    atomic: bool,
    /// The session's open batch scope every step edits inside.
    token: BatchToken,
    /// Each executed step's structured result, `None` when it failed.
    results: Vec<Option<Value>>,
    /// Each executed step's report entry.
    outcomes: Vec<StepOutcome>,
}

impl Runner<'_> {
    /// Runs the steps under the batch's atomicity rule and closes the
    /// scope: an atomic failure rolls back, any other ending commits the
    /// batch's one folded entry under `label`.
    async fn run(&mut self, label: &str) -> Result<BatchReport, CyberError> {
        let planned = self.steps.len();
        let mut queue = mem::take(&mut self.steps).into_iter();
        while let Some(step) = queue.next().filter(|_| self.running()) {
            self.run_one(step).await;
        }
        self.finish(label, planned)
    }

    /// Whether the next step may still run: an atomic batch stops as soon
    /// as a step failed.
    fn running(&self) -> bool {
        !self.atomic || self.outcomes.iter().all(|step| step.ok)
    }

    /// Runs one step and records it for the report and for later `$ref`s.
    async fn run_one(&mut self, step: BatchStep) {
        let outcome = self.run_step(&step).await;
        let record = if outcome.ok {
            outcome.result.clone()
        } else {
            None
        };
        self.results.push(record);
        self.outcomes.push(outcome);
    }

    /// Runs one step: substitute its `$ref`s, then dispatch through the
    /// server's router with this request's context.
    async fn run_step(&self, step: &BatchStep) -> StepOutcome {
        let arguments = match self.substitute(step) {
            Ok(arguments) => arguments,
            Err(error) => return StepOutcome::refused(&step.tool, &error),
        };
        let outcome = self.dispatch(&step.tool, arguments).await;
        StepOutcome::of(&step.tool, outcome)
    }

    /// The step's arguments with every `$ref` resolved against the results
    /// so far, as the object to send.
    fn substitute(&self, step: &BatchStep) -> Result<JsonMap<String, Value>, CyberError> {
        let raw = step.arguments.clone().unwrap_or(Value::Null);
        match resolve_refs(raw, &self.results)? {
            Value::Object(map) => Ok(map),
            Value::Null => Ok(JsonMap::new()),
            _scalar @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_)) => {
                Err(CyberError::InvalidArgument(format!(
                    "the arguments of the {} step resolve to something that is not an object; a \
                     $ref replacing the whole arguments parameter must name an object",
                    step.tool,
                )))
            }
        }
    }

    /// Calls one tool through the server's own tool router, exactly as an
    /// external client would, and returns its protocol outcome.
    async fn dispatch(
        &self,
        tool: &str,
        arguments: JsonMap<String, Value>,
    ) -> Result<CallToolResponse, ErrorData> {
        let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
        let context = ToolCallContext::new(self.server, params, self.request.clone());
        self.server.tool_router().call(context).await
    }

    /// Closes the session's batch scope — rolling back or committing the
    /// single folded entry — and reports: the batch is `ok` only when every
    /// planned step ran and succeeded, and `undo_label` names the entry the
    /// commit left, `None` for a rollback or a scope whose steps recorded no
    /// edits at all.
    fn finish(&mut self, label: &str, planned: usize) -> Result<BatchReport, CyberError> {
        let all_ok = self.outcomes.len() == planned && self.outcomes.iter().all(|step| step.ok);
        let rolled_back = !all_ok && self.atomic;
        let outcome = if rolled_back {
            BatchOutcome::Rollback
        } else {
            BatchOutcome::Commit
        };
        let token = self.token;
        let recorded = self
            .server
            .state()
            .with_session(move |session| session.end_batch(token, outcome))?;
        Ok(BatchReport {
            ok: all_ok,
            rolled_back,
            steps: mem::take(&mut self.outcomes),
            undo_label: recorded.then(|| label.to_owned()),
        })
    }
}

#[expect(
    clippy::multiple_inherent_impl,
    reason = "rmcp's documented multi-group layout needs one #[tool_router] impl block per group"
)]
#[tool_router(router = batch_router, vis = "pub(crate)")]
impl Cyberdemon {
    /// Runs several tool calls as one atomic, single-undo batch.
    #[tool(
        description = "Runs up to 50 tool calls in order as ONE undoable step: each step is {tool, arguments?}, an argument {\"$ref\": \"<step>/<json-pointer>\"} takes that earlier step's result value, and atomic (default true) stops at the first failure and rolls every recorded edit back. run_batch, undo and redo are refused as steps, and while a batch runs the session itself refuses the whole-archive and saved-state tools (open_iwad, open_wad, new_wad, merge_wad layer mode, save_wad pwad mode). Returns {ok, rolled_back, steps, undo_label}."
    )]
    async fn run_batch(
        &self,
        Parameters(args): Parameters<RunBatchParams>,
        request: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        reply(batch_run(self, args, request).await)
    }
}

/// The batch's work: precheck the whole plan, check the undo stack has
/// room for the steps, open the session's batch scope, run the steps
/// inside it and close the scope. Nothing runs, and no scope opens, before
/// every refusal has been checked, so a refused call leaves the session
/// untouched.
async fn batch_run(
    server: &Cyberdemon,
    args: RunBatchParams,
    request: RequestContext<RoleServer>,
) -> Result<BatchReport, CyberError> {
    precheck(&args.steps, server.tool_router())?;
    let depth = server
        .state()
        .with_session(|session| Ok(session.undo_depth()))?;
    check_history_room(depth, args.steps.len())?;
    let label = batch_label(args.label.as_deref(), &args.steps);
    let token = server
        .state()
        .with_session(|session| session.begin_batch(label.as_str()))?;
    let mut runner = Runner {
        server,
        request,
        steps: args.steps,
        atomic: args.atomic,
        token,
        results: Vec::new(),
        outcomes: Vec::new(),
    };
    runner.run(label.as_str()).await
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{
        BatchStep, LABEL_LIMIT, MAX_STEPS, batch_label, check_history_room, check_step,
        follow_pointer, lookup_ref, ref_marker, resolve_refs,
    };
    use crate::error::CyberError;
    use crate::server::Cyberdemon;

    /// The refusal a helper reported, as `(code, message)`.
    fn refusal(error: &CyberError) -> (String, String) {
        (error.code().to_owned(), error.to_string())
    }

    /// The recorded results of a two-step batch: step 0 succeeded with a
    /// nested payload, step 1 failed.
    fn results() -> Vec<Option<Value>> {
        vec![
            Some(json!({
                "index": 7_i64,
                "things": [{"x": 96_i64, "y": -128_i64}],
                "flag": true,
            })),
            None,
        ]
    }

    /// Resolves one reference string against [`results`].
    fn resolve(text: &str) -> Result<Value, CyberError> {
        lookup_ref(text, &results()).cloned()
    }

    /// A step for label tests.
    fn step(tool: &str) -> BatchStep {
        BatchStep {
            tool: tool.to_owned(),
            arguments: None,
        }
    }

    /// The object a `json!` literal produced, for the marker tests.
    fn object(value: &Value) -> serde_json::Map<String, Value> {
        value
            .as_object()
            .expect("test literals are objects")
            .clone()
    }

    #[test]
    fn ref_marker_requires_the_exact_single_key_object() {
        assert_eq!(
            ref_marker(&object(&json!({"$ref": "0/index"}))).expect("a reference"),
            "0/index",
            "the exact form is a reference"
        );
        assert!(
            ref_marker(&object(&json!({"$ref": "0/i", "other": 1_i64}))).is_none(),
            "a second key makes it a literal object"
        );
        assert!(
            ref_marker(&object(&json!({"$ref": 3_i64}))).is_none(),
            "a non-string $ref is a literal"
        );
        assert!(ref_marker(&object(&json!({}))).is_none(), "an empty object");
    }

    #[test]
    fn lookup_reads_indexes_members_and_array_positions() {
        assert_eq!(
            resolve("0/index").expect("member"),
            json!(7_i64),
            "object member"
        );
        assert_eq!(
            resolve("0/things/0").expect("element"),
            json!({"x": 96_i64, "y": -128_i64}),
            "array element by position"
        );
        assert_eq!(
            resolve("0/flag").expect("bool"),
            json!(true),
            "a boolean value"
        );
        assert_eq!(
            resolve("0").expect("whole result"),
            results()
                .first()
                .expect("step 0")
                .as_ref()
                .expect("step 0 succeeded")
                .clone(),
            "an index alone names the whole result"
        );
    }

    #[test]
    fn lookup_refuses_forward_failed_and_missing_references() {
        let forward = refusal(&resolve("2/index").expect_err("a later step"));
        assert_eq!(forward.0, "invalid_argument", "forward reference code");
        assert!(
            forward.1.contains("has not run yet"),
            "the message says why: {forward:?}"
        );
        let failed = refusal(&resolve("1/index").expect_err("a failed step"));
        assert_eq!(failed.0, "invalid_argument", "failed step code");
        assert!(
            failed.1.contains("failed"),
            "the message says why: {failed:?}"
        );
        let missing = refusal(&resolve("0/nothing").expect_err("a dead pointer"));
        assert_eq!(missing.0, "invalid_argument", "missing pointer code");
        assert!(
            missing.1.contains("no value at that pointer"),
            "the message names the problem: {missing:?}"
        );
        let negative = refusal(&resolve("-1/index").expect_err("no negative steps"));
        assert_eq!(negative.0, "invalid_argument", "bad index code");
    }

    #[test]
    fn follow_pointer_treats_the_empty_pointer_as_the_whole_value() {
        let value = json!({"a": {"b": [1_i64, 2_i64]}});
        assert_eq!(
            follow_pointer(&value, "").expect("the empty pointer is the value itself"),
            &value,
            "no segments, no movement"
        );
        assert_eq!(
            follow_pointer(&value, "a/b/1").expect("a nested path"),
            &json!(2_i64),
            "member, member, array index"
        );
        assert!(
            follow_pointer(&value, "a/b/9").is_none(),
            "an index past the array ends is missing"
        );
        assert!(
            follow_pointer(&value, "a/b/0/x").is_none(),
            "a scalar has no members"
        );
    }

    #[test]
    fn resolve_refs_substitutes_at_any_depth_and_keeps_literals() {
        let resolved = resolve_refs(
            json!({
                "index": {"$ref": "0/index"},
                "keep": {"$ref": "literal", "x": 1_i64},
                "nested": [[{"$ref": "0/things/0/x"}], 5_i64],
                "text": "not a reference at all",
            }),
            results().as_slice(),
        )
        .expect("all references resolve");
        assert_eq!(
            resolved,
            json!({
                "index": 7_i64,
                "keep": {"$ref": "literal", "x": 1_i64},
                "nested": [[96_i64], 5_i64],
                "text": "not a reference at all",
            }),
            "exact $ref objects are replaced everywhere, other objects stand"
        );
    }

    #[test]
    fn resolve_refs_propagates_a_bad_nested_reference() {
        let error = resolve_refs(
            json!({"list": [{"index": {"$ref": "1/index"}}]}),
            results().as_slice(),
        )
        .expect_err("a reference to the failed step must refuse");
        assert_eq!(
            error.code(),
            "invalid_argument",
            "nested bad reference code"
        );
    }

    #[test]
    fn batch_labels_name_the_tools_and_honour_the_limit() {
        let steps: Vec<BatchStep> = ["add_sector", "edit_sector", "add_thing"]
            .iter()
            .map(|tool| step(tool))
            .collect();
        assert_eq!(
            batch_label(None, &steps),
            "batch: add_sector, edit_sector, add_thing",
            "the default label lists the tools"
        );
        assert_eq!(
            batch_label(Some("darken it"), &steps),
            "darken it",
            "the given label wins"
        );
        let long = "x".repeat(LABEL_LIMIT.saturating_add(20));
        assert_eq!(
            batch_label(Some(long.as_str()), &steps).chars().count(),
            LABEL_LIMIT,
            "labels are kept to the limit"
        );
        let wide: Vec<BatchStep> = (0..MAX_STEPS).map(|_| step("render_topdown")).collect();
        assert_eq!(
            batch_label(None, &wide).chars().count(),
            LABEL_LIMIT,
            "a full batch's default label is truncated too"
        );
    }

    #[test]
    fn check_history_room_holds_at_the_limit() {
        assert!(
            check_history_room(0, MAX_STEPS).is_ok(),
            "a full batch from an empty history"
        );
        assert!(
            check_history_room(50, MAX_STEPS).is_ok(),
            "the tightest batch that still fits"
        );
        let over = MAX_STEPS
            .checked_add(1)
            .expect("a step count that does not fit at depth 51");
        let error = check_history_room(51, over).expect_err("51 + 51 > 100");
        assert_eq!(error.code(), "invalid_argument", "overflow refusal code");
        assert!(
            error.to_string().contains("undo"),
            "the message names the way out: {error}"
        );
    }

    #[test]
    fn the_non_nestable_groups_are_the_batch_and_the_history() {
        let blocked = Cyberdemon::non_nestable_router();
        for name in ["run_batch", "undo", "redo"] {
            assert!(
                blocked.has_route(name),
                "{name} belongs to a group a batch may not dispatch"
            );
        }
        for name in [
            "open_iwad",
            "open_wad",
            "new_wad",
            "merge_wad",
            "save_wad",
            "get_session_info",
        ] {
            assert!(
                !blocked.has_route(name),
                "{name} is refused at run time by the session, not by a group list"
            );
        }
    }

    #[test]
    fn check_step_refuses_the_group_tools_unknown_names_and_bad_arguments() {
        let server = Cyberdemon::new();
        let router = server.tool_router();
        let blocked = Cyberdemon::non_nestable_router();
        let refusal = |tool: &str, arguments: Option<Value>| {
            let step = BatchStep {
                tool: tool.to_owned(),
                arguments,
            };
            check_step(1, &step, router, &blocked).expect_err("the step is refused")
        };
        let nested = refusal("undo", None);
        assert_eq!(nested.code(), "invalid_argument", "group refusal code");
        assert!(
            nested.to_string().contains("step 1") && nested.to_string().contains("undo"),
            "the refusal names the step and tool: {nested}"
        );
        assert!(
            refusal("run_batch", None).to_string().contains("run_batch"),
            "a batch cannot nest inside itself"
        );
        let unknown = refusal("no_such_tool", None);
        assert_eq!(unknown.code(), "invalid_argument", "unknown refusal code");
        assert!(
            unknown.to_string().contains("no_such_tool"),
            "the refusal names the tool: {unknown}"
        );
        let bad_args = refusal("edit_sector", Some(json!(3_i64)));
        assert_eq!(
            bad_args.code(),
            "invalid_argument",
            "arguments refusal code"
        );
        assert!(
            bad_args.to_string().contains("not an object"),
            "the refusal says what to fix: {bad_args}"
        );
        let archive_swap = step("open_wad");
        check_step(0, &archive_swap, router, &blocked)
            .expect("archive swaps plan fine; the session refuses them while a batch is open");
        let plain_edit = BatchStep {
            tool: "edit_sector".to_owned(),
            arguments: Some(json!({"index": 0_i64})),
        };
        check_step(0, &plain_edit, router, &blocked).expect("a plain edit step plans");
        check_step(0, &step("get_session_info"), router, &blocked)
            .expect("a read step plans and sends no arguments");
    }
}
