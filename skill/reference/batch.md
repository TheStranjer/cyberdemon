# Batch tools

Running several tool calls as one atomic, single-undo step: `run_batch`.

## When to use it

A plain-English request often needs five to twenty calls ("make a secret
room east of the hall with a medikit, and darken it"). Send such a sequence
as one `run_batch`: either every step lands or nothing does, and one `undo`
takes the whole request back instead of one step per call. Use plain calls
when you need to look at a result before deciding the next step — batching
does not make the server think.

## `run_batch`

`{steps: [{tool, arguments?}], atomic? = true, label?}` →
`{ok, rolled_back, steps: [{tool, ok, result, error}], undo_label}`

- `steps`: 1 to 50 calls, run in order. `arguments` is the object that tool
  would take from a client directly (omit it for a tool that takes none).
- `atomic` (default `true`): a failing step stops the batch and **undoes
  every edit the batch recorded** — the map is exactly as it was before the
  batch and the failed step is not redoable. With `atomic: false` every
  step runs and the successful edits are kept.
- `label`: the undo label for the whole batch. The default is
  `"batch: tool1, tool2, ..."` from the step tools; either is kept to 80
  characters. A batch that ends without a rollback leaves **one** undo
  entry under that label (one `undo` reverts the batch, one `redo`
  re-applies it); a batch whose steps recorded no edits leaves none and
  reports `undo_label: null`.
- The result lists the steps that **ran**: an atomic batch that stopped at
  step 3 reports three entries, the last carrying the step's error. A step
  failed when the tool returned a domain error, so its `error` is the usual
  `{"error", "message"}` object and `result` is `null`.
- `run_batch` itself answers with an error result (nothing has run then)
  only for batch-level refusals: a step count outside `1..=50`, a
  **forbidden tool** in any step (`run_batch`, `undo`, `redo`, `open_iwad`,
  `open_wad`, `new_wad`, `merge_wad`, `save_wad`), an **unknown tool name**,
  arguments that are not an object, or a batch whose steps would push the
  undo stack past its 100-entry limit. Each refusal is `invalid_argument`
  and names the step.

## Chaining results: `$ref`

Before a step runs, any object in its arguments of the **exact form**

```json
{"$ref": "<step index>/<json pointer>"}
```

is replaced by the value that JSON pointer selects in that earlier step's
structured `result` (`/`-separated members and array indices; `"<n>"` alone
names the whole result). Substitution reaches into nested objects and
arrays, so only the one member moves. An object with any extra key beside
`$ref` is sent as a literal argument instead.

```json
{"steps": [
  {"tool": "add_sector", "arguments": {"ceiling_flat": "SKY1"}},
  {"tool": "add_thing", "arguments": {"type": 2011, "x": 100, "y": 100}},
  {"tool": "edit_sector",
   "arguments": {"index": {"$ref": "0/index"}, "light": 32}},
  {"tool": "edit_thing",
   "arguments": {"index": {"$ref": "1/index"}, "special": 10}}
]}
```

Step 2 darkens the sector step 0 created; step 3 scripts the thing step 1
added — no round trips to read the indices back.

Referencing a step that has not run (a later index), one that failed, or a
pointer its result does not hold is an `invalid_argument` **of that step**:
under `atomic: true` it stops and rolls back like any other step failure.

## What is not rolled back

A rollback restores the **working archive** only. Session state outside its
undo history stays as the batch left it: the texture palette (`set_palette`),
the render settings (`set_render_resolution`, `set_render_options`), the
asset capacity (`set_asset_capacity`) and the active map (`set_active_map`).
Steps that only read (`get_*`, `list_*`, `render_*`, `assemble_map`,
`preview_*`, `diff_wads`) are allowed and leave nothing behind.

## Verifying

A failed atomic batch must leave the map untouched: re-read it with
`get_map_summary` or `render_topdown` if the result's `rolled_back: true`
is all you have — the per-step `result` objects show what was applied
before the rollback threw it away.
