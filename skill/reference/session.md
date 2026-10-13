# Session tools

Building up the editing session and describing it.

## The session model

Every tool call reads and writes one per-connection **session**, which
holds archives the way an engine loads them:

- an optional **IWAD layer**: the read-only base game archive (`open_iwad`);
- any number of read-only **resource layers**: PWADs stacked on top of the
  base in load order (`merge_wad` with `mode: "layer"`);
- one **working archive**: the editable WAD that every editing tool changes
  and nothing else (`open_wad`, `new_wad`, and `merge_wad` with
  `mode: "working"`); it remembers the path it was opened from or last
  saved to, and starts out an empty PWAD.

The **merged view** is what an engine would see: the IWAD layer, then each
resource layer, then the working archive, folded together with vanilla
override rules — a map block replaces the map of the same name in place or
appends, a flat/sprite/patch replaces the copy in its own region, any other
lump replaces the last copy of its name — so within each area **the last
match wins**. Reads that take a `scope` see one archive or the merged view;
edits only ever touch the working archive, and each edit lands on a
labelled undo stack. `get_session_info` reports where every archive
currently is.

## `open_iwad`

- Purpose: read a file and make it the session's base game layer,
  replacing any previous IWAD. Resource layers and the working archive are
  kept. Accepts `IWAD` or `PWAD` magic — the layer's job, not the header,
  makes it the base.
- Arguments: `path` (string, absolute filesystem path; relative paths are
  rejected with `invalid_argument`).
- Returns: `{"path", "kind": "IWAD"|"PWAD", "lumps": n, "maps": n, "game"}`
  — `game` is the detected title (`"doom"`, `"doom2"`, `"heretic"`,
  `"hexen"`, `"strife"` or `"unknown"`).
- Errors: `invalid_argument` (relative path), `not_found` (no file there),
  `io_error` (cannot read it), `format_error` (not a WAD).

## `open_wad`

- Purpose: read a file and adopt it as the editable working archive. The
  previous working archive and its undo history are dropped.
- Arguments: `path` (string, absolute); `discard_changes` (bool, default
  `false`) — replace the working archive even when it holds unsaved edits.
- Returns: like `open_iwad` plus `"dirty": false` (a fresh open is saved
  state).
- Errors: `conflict` while the working archive is dirty and
  `discard_changes` was not passed — `save_wad` first or pass the flag;
  plus the same path errors as `open_iwad`.

## `new_wad`

- Purpose: start a fresh empty PWAD as the working archive, with no file
  behind it yet (the next `save_wad` gives it one).
- Arguments: `discard_changes` (bool, default `false`), same guard as
  `open_wad`.
- Returns: `{"kind": "PWAD", "lumps": 0}`.
- Errors: `conflict` on unsaved changes without `discard_changes`.

## `merge_wad`

- Purpose: read a file and combine it with the session, one of two ways.
- Arguments: `path` (string, absolute); `mode` (string, `"layer"` (the
  default) or `"working"`).
- `mode: "layer"` stacks the file as a read-only resource layer above the
  existing layers, below the working archive — the working archive keeps
  winning every name it shares. Returns
  `{"mode": "layer", "layer_index": n, "lumps": n}` where `layer_index` is
  its position in the layer numbering (0 is the IWAD, so the first
  resource of a loaded-IWAD session is 1).
- `mode: "working"` folds the file's lumps into the working archive under
  the merged view's override rules — maps replace or append, namespace
  lumps replace or join their region, globals replace the last copy or
  append. The fold is one undoable edit labelled
  `merge_wad <filename>`. Returns `{"mode": "working", "replaced": n,
  "added": m}`, counted per override unit (a whole map block counts as
  one). This marks the working archive dirty.
- Errors: `invalid_argument` for an unknown `mode` (the message names both
  accepted forms) or a relative path; `not_found` / `io_error` /
  `format_error` as for `open_iwad`.

## `get_session_info`

- Purpose: a complete picture of the session — what is loaded, which
  archive edits would change, and how far undo reaches.
- Arguments: none.
- Returns:

  ```json
  {
    "iwad": {"path": "...", "kind": "IWAD", "lumps": 4200} | null,
    "layers": [{"index": 0, "role": "iwad", "path": "...", "kind": "IWAD", "lumps": 4200}],
    "working": {"path": "...", "kind": "PWAD", "lumps": 3, "dirty": true},
    "active_map": "MAP01" | null,
    "game": "doom2",
    "undo": ["build room", "merge_wad extras.wad"],
    "redo": []
  }
  ```

  `layers` numbers every read-only archive from 0 (the IWAD first, then
  each resource in load order) — the indices `scope` names use elsewhere.
  `undo` and `redo` are label lists oldest first, newest last; `undo`
  shows what `undo` would take back, newest entry next to go.

## `get_wad_info`

- Purpose: the merged view an engine would load from this session, in one
  summary: which game, which archives, which maps, what is where.
- Arguments: none.
- Returns:

  ```json
  {
    "game": "doom2",
    "iwad": "/path/to/doom2.wad" | null,
    "pwads": ["<resource layer paths, in load order>", "(unsaved working archive)"],
    "total_lumps": 4219,
    "maps": [{"name": "MAP01", "format": "doom", "source": {"layer": 0}}],
    "namespaces": {"flats": 13},
    "warnings": []
  }
  ```

  `maps[].source` names the archive the merged view took the block from:
  `{"layer": n}` or `"working"`. `format` is `"doom"`, `"hexen"` or
  `"udmf"`. `namespaces` counts member lumps per region actually present
  (markers excluded). `warnings` lists structural problems in the merged
  directory (unclosed region markers, `TEXTMAP` without `ENDMAP`, ...).
- Game detection reads the IWAD layer when one is loaded, else the merged
  view.

## Typical workflow

`open_iwad` the game archive → `open_wad` your project or `new_wad` for a
fresh one → edit → `save_wad`. Check where you are with `get_session_info`
and what an engine would see with `get_wad_info`.
