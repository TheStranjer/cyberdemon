# Scripts tools

Reading and editing the text lumps a session holds: DeHackEd/BEX patches
(`get_dehacked`, `edit_dehacked`) and `DECORATE`/`ZScript` scripts
(`edit_decorate`, `edit_zscript`). `get_dehacked` parses one patch into
header values, blocks and BEX sections; `edit_dehacked` writes one back —
wholesale or line by line — as a single undoable edit. The two script
editors run one tagged text operation over a whole lump. All of them feed
`get_thing_types` immediately: it reads the same merged texts (see
`reference/defs.md`).

## What the parser recognises

- Optional title lines: `Patch File for DeHackEd v3.0`, and the out-of-block
  header keys `Doom version = 21` / `Patch format = 6` (reported as
  `header`).
- Blocks `<Kind> <number> [(name)]` with Kind one of `Thing`, `Frame`,
  `Pointer`, `Sound`, `Ammo`, `Weapon`, `Misc` (number optional), `Cheat`,
  `Sprite`, and `Text <len1> <len2>`; keys run until a blank line or the
  next header. A `Text` block stores its first length as `number` and its
  second as `name`; its raw body lines are opaque (not parsed, not listed —
  read them with `include_text`).
- BEX sections `[CODEPTR]`, `[STRINGS]`, `[PARS]`, `[SPRITES]`, `[SOUNDS]`,
  `[MUSIC]`, `[HELPER]`; entries run until the next section or block
  header, so blank lines never cut a section.
- `#` comment lines are skipped; `Include <file>` lines are reported in
  `warnings`, never followed.
- Keys are matched case-insensitively; every line the edit does not touch
  keeps its exact original spelling and spacing. Line endings normalise to
  LF and the stored text keeps a trailing newline.

Parsing is tolerant: an unrecognised header line, a non-numeric block
number or a stray key ends the current region and lands in `warnings` with
its 1-based line number, never as an error.

## `get_dehacked`

- Purpose: read one patch lump as structured data instead of raw text.
- Arguments: `name` (default `"DEHACKED"`), `scope` (`"merged"` default,
  `"working"`, `"layer:N"`), `include_text` (default `false`).
- Returns:

  ```json
  {
    "name": "DEHACKED",
    "source": {"kind": "resource", "index": 1, "path": "/mods/patch.wad"},
    "header": {"doom_version": 21, "patch_format": 6},
    "blocks": [
      {"kind": "Thing", "number": 12, "name": "Evil Imp", "line": 6,
       "fields": {"ID #": "20000", "Width": "655360"}}
    ],
    "bex": [
      {"section": "STRINGS", "line": 10, "entries": {"HUDSTR_PAUSE": "PAUSED"}}
    ],
    "text": "…only when include_text…",
    "warnings": ["line 3: Include other.deh is reported, not followed"]
  }
  ```

  `line` is the 1-based line of the block/section header; `null` numeric
  header fields mean absent or unparseable. `source.kind` is `"iwad"`,
  `"resource"` or `"working"`. A missing lump is `not_found`.
- Typical use: inspect what a patch does before editing it, or diff two
  layers by calling with `scope: "layer:0"` / `"layer:1"`.

## `edit_dehacked`

- Purpose: write a patch lump with exactly one undoable edit (label
  `edit_dehacked`; take it back with `undo`).
- Arguments: `name` (default `"DEHACKED"`) plus **exactly one mode**:
  - `text` — replaces the whole lump with the given UTF-8 text.
  - `source` — a `.deh`/`.bex` file (absolute path, `file://` URI,
    `http(s)://` URL or uploaded asset URL, see "Files and URLs" in
    `SKILL.md`) whose bytes replace the lump text.
  - `set` — `[{block, key, value}]`, applied in order: rewrites the first
    `Key = Value` line matching `key` (case-insensitive) inside the first
    block matching `block`, inserts one after the block's last key when the
    key is absent, or appends the block — preceded by a blank line — when
    the block is absent.
  - `remove` — `[{block, key}]` deletes the key line, or the whole block
    (header through its last line, plus one following blank line) when
    `key` is omitted.
- `block` is `"Thing 12"`, `"thing 12 (Evil Imp)"` (the name is only used
  when the block gets created), `"Misc"`, `"Text 6 0"` (matched on the
  first length; `set`/key-`remove` inside a `Text` body are refused), or a
  BEX section like `"[STRINGS]"`. An unrecognised block string is
  `invalid_argument`, as are values containing newlines and keys
  containing `=` or newlines; a missing `remove` block or key is
  `not_found` and changes nothing.
- Copy-on-write: a lump held only by the `IWAD` or a resource layer is not
  touched there — the edited text is appended to the working archive as the
  winning override and `created` reports `true`. With `set` and the lump
  absent everywhere, the new patch starts from the canonical header
  (`Patch File for DeHackEd v3.0`, `Doom version = 21`, `Patch format = 6`,
  blank line).
- Returns:

  ```json
  {"name": "DEHACKED", "lines": 8, "changed_lines": 1, "created": true}
  ```

  `lines` is the resulting line count and `changed_lines` the lines
  rewritten, inserted or removed (for the whole-text modes: the lines that
  differ from the old text, all of them when the lump is created).
- Typical use:

  ```json
  { "set": [{ "block": "Thing 12", "key": "ID #", "value": "20000" }] }
  ```

  moves the Imp (mobj 12, Doom II order) to doomednum 20000; follow it
  with `get_thing_types { "type": 20000 }` to confirm and `render_topdown`
  style checks for anything placed at it.

## `edit_decorate` / `edit_zscript`

- Purpose: run exactly one text operation on one script lump as a single
  undoable edit (label `edit_decorate LUMP` / `edit_zscript LUMP`; take it
  back with `undo`).
- Arguments: `lump` (default `"DECORATE"` / `"ZSCRIPT"`; for `edit_zscript`
  you can pass the name of an `#include`d lump, e.g. `"ZSCRIPT2"`) plus one
  operation, selected by its `op` field:
  - `{"op": "replace_all", "text": T}` — replaces the whole text with `T`.
  - `{"op": "append", "text": T}` — adds `T` at the end, inserting a newline
    separator when the text does not already end with one.
  - `{"op": "find_replace", "find": F, "replace": R, "all": A}` — replaces
    the first literal, case-sensitive occurrence of `F` (`all: true`
    replaces every one). `F` empty is `invalid_argument`; zero matches is
    `not_found`.
  - `{"op": "insert_lines", "line": N, "text": T}` — inserts `T`'s lines
    before the 1-based line `N` (`N = line count + 1` appends at the end).
  - `{"op": "replace_lines", "start": S, "end": E, "text": T}` — swaps the
    inclusive 1-based range `S..=E` for `T`'s lines.
  - `{"op": "delete_lines", "start": S, "end": E}` — removes that range.
- Line numbers out of range or `start > end` are `invalid_argument` and
  change nothing; non-UTF-8 lump bytes are `format_error`. CRLF texts keep
  `\r\n` endings and LF texts keep LF; the trailing newline is preserved.
- Creation and copy-on-write: a missing lump is created in the working
  archive by `replace_all`/`append` (`created` reports `true`); every other
  op on a missing lump is `not_found`. A lump held only by the `IWAD` or a
  resource layer is copied-on-write (the edited text is appended to the
  working archive as the winning override, `created: true`).
- Read the current text first with `get_lump format=text`; the result's
  `excerpt` shows the changed lines with two lines of numbered context.
- Returns:

  ```json
  {
    "lump": "DECORATE",
    "lines": 3,
    "changed_range": [1, 3],
    "excerpt": "1: actor TestImp 20100 {\n2:   radius 24\n3: }",
    "warnings": ["unbalanced braces: 1 '{' vs 0 '}' outside strings and comments"],
    "created": true
  }
  ```

  `changed_range` is the 1-based inclusive `[first, last]` of changed lines
  (`[0, 0]` when nothing changed); `warnings` is normally `[]` and carries
  the brace-balance note when `{`/`}` counts differ outside strings and
  `//` / `/* */` comments.
- Typical use:

  ```json
  { "op": "append", "text": "actor TestImp 20100 {\n  radius 24\n}\n" }
  ```

  adds an actor; follow it with `get_thing_types { "type": 20100 }` to
  confirm it became a thing row.

## Limits

All four tools read the winning copy of the name (last match in the scope)
and carry text only. `get_dehacked`/`edit_dehacked` do not follow `Include`,
do not apply `[CODEPTR]` semantics beyond storing the lines, and never
interpret values; Width-style 16.16 fixed values and `ID #` semantics are
the definition overlay's concern (`reference/defs.md`).
`edit_decorate`/`edit_zscript` do not follow `#include` either (name the
included lump directly as `lump` to edit it), do not compile or validate
DECORATE/ZScript semantics, and only *warn* about unbalanced braces;
`get_thing_types` reads the merged DECORATE/ZSCRIPT texts
(`reference/defs.md`), so actors and classes with a doomednum (or a MAPINFO
`DoomEdNums` entry) show up there.
