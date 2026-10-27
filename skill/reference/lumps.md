# Lumps tools

Directory browsing, single-lump reads, and the lump editing tools over the
session's archives: `add_lump`, the import tools, `replace_lump`,
`remove_lump`, `rename_lump` and `reorder_lumps`. Edits go to the working
archive only and are undoable; content a read-only layer owns is edited by
copy-on-write (see below).

## The row shape

The listing tools report lumps as rows of this shape (`get_lump`'s result
starts from the same fields):

```json
{
  "index": 12,
  "name": "FLOOR1",
  "size": 4096,
  "offset": 53212,
  "kind": "flat",
  "namespace": "flats",
  "map": null,
  "source": "working"
}
```

- `index` — position in the listed scope (the merged-view index when the
  scope is `merged`).
- `name` — the eight-byte lump name, uppercased.
- `size` — payload bytes.
- `offset` — byte offset of the lump's data **inside the archive it comes
  from, as `save_wad` would lay that archive out** (header, then payload
  bytes in directory order, then the directory). It is not necessarily the
  position inside the file on disk, which may carry padding or a different
  order.
- `kind` — a type hint from the classifier (values below).
- `namespace` — the marker region the lump sits in, or `null`.
- `map` — the map block owning the lump (`"MAP01"`, `"E1M1"`, …), or `null`.
- `source` — where the merged view took the lump from: `"working"` for the
  editable archive, or `{"layer": n}` (0 is the IWAD, then resource layers
  in load order). Outside a `merged` scope this just repeats the scope.

### `kind` values

`marker`, `map_marker`, `map_data`, `palette`, `colormap`, `pnames`,
`texture_defs`, `endoom`, `demo`, `dehacked`, `decorate`, `zscript`,
`mapinfo`, `png`, `jpeg`, `gif`, `mus`, `midi`, `wav`, `ogg`, `flac`,
`mp3`, `doom_sound`, `pc_speaker`, `flat`, `sprite`, `patch`, `text`,
`empty`, `unknown`.

Inside `flats`, `sprites` and `patches` the namespace hint wins over a
guessed payload: only an unmistakable container signature (`png`, `jpeg`,
`gif`, `wav`, `ogg`, `flac`, `midi`, `mus`, or an `mp3` starting with its
`ID3` tag) overrides it. The three tests raw indexed pixels satisfy by
accident — a bare `mp3` frame sync, a `doom_sound` header, a `pc_speaker`
zero word — are consulted only outside those three regions.

### `namespace` values

`sprites`, `flats`, `patches`, `colormaps`, `acs`, `voices`, `hires`,
`textures`.

## Scopes

`scope` selects the archive to list or search: `"merged"` (the default —
the directory an engine would see), `"working"` (the editable archive
alone) or `"layer:N"` (one read-only layer, 0 is the IWAD). An unknown
scope name is `invalid_argument`; `layer:N` for a layer that is not loaded
is `not_found`.

## `list_lumps`

- Purpose: walk a scope's directory in order, with optional filters and
  paging. This is the safe way to survey a large archive; `total` tells you
  how many match before you page.
- Arguments (all optional):
  - `scope` (string, default `"merged"`).
  - `namespace` (string, one of the names above) — keep region members,
    markers included.
  - `kind` (string, one of the names above) — keep that type hint.
  - `map` (string, e.g. `"MAP01"`) — keep only the header and data lumps of
    that map block.
  - `offset` (number, default 0) — matching rows to skip.
  - `limit` (number, default 200, 1 to 2000) — page size.
- Returns: `{"scope", "total", "offset", "count", "lumps": [rows]}` —
  `total` counts every match in the scope, `count` is this page
  (`lumps.len()`), and `index` always counts positions in the whole scope,
  so paging never renumbers rows.
- Errors: `invalid_argument` (unknown `scope`/`namespace`/`kind`, an
  unparsable `map` name, `limit` of 0 or above 2000), `not_found`
  (`layer:N` not loaded).

## `search_lumps`

- Purpose: find lumps by name, type, region or **content** — e.g. which
  lumps carry a `MUS\x1A` header or mention a thing number.
- Arguments (all optional, but at least one of the first five is required):
  - `pattern` (string) — case-insensitive name glob; `*` matches any run of
    characters, `?` exactly one (`"FLOOR*"`, `"?LOOR1"`).
  - `kind` (string) — as `list_lumps`.
  - `namespace` (string) — as `list_lumps`.
  - `contains_text` (string) — keep lumps whose payload contains this text,
    case-insensitively on ASCII bytes.
  - `contains_hex` (string) — keep lumps whose payload contains these
    bytes; hex digits in pairs, spaces allowed (`"4D 55 53 1A"`), matched
    exactly.
  - `scope` (string, default `"merged"`).
  - `offset` (number, default 0) — matching lumps to skip before the page.
  - `limit` (number, default 200, 1 to 2000).
- Criteria combine with AND.
- Returns: `{"scope", "total", "offset", "count", "next_offset",
  "lumps": [rows + "match_offset"]}` — `total` counts every match in the
  scope before paging and `next_offset` names the `offset` that continues
  the hits (`null` when this page ends them), so paging a long search works
  exactly like `list_lumps`. `match_offset` is the byte offset of the first
  content match (lowest when both `contains_text` and `contains_hex` were
  given), `null` when no content criterion was used.
- Errors: `invalid_argument` (no criterion at all, bad `scope`/`kind`/
  `namespace`/`limit`, or hex text that is not whole hex pairs), `not_found`
  (`layer:N` not loaded).

## The lump selector

`get_lump` and `extract_lump` pick one lump with the same four fields,
which the editing tools reuse in a reduced form:

- `name` (string, required) — the lump name, 1 to 8 printable ASCII
  characters, case-insensitive (uppercased). Anything else is
  `invalid_argument`.
- `scope` (string, default `"merged"`) — as `list_lumps`. The editing tools
  (`replace_lump`, `remove_lump`, `rename_lump`, `reorder_lumps`) take no
  `scope`: they choose the archive themselves, as Copy-on-write below
  describes.
- `namespace` (string, optional) — keep only matches inside that region
  (`flats`, `sprites`, ...). Without it, matches in any region and loose
  lumps all count.
- `occurrence` (number, optional) — the 0-based index **among the matching
  lumps**, in the scope's directory order. The default is the **last**
  match, which is the copy that wins an engine's lookup of that name. Set
  it to read an earlier (losing) copy.

No match is `not_found` (`"lump FOO not found in merged"`); an occurrence
past the last match is `not_found` naming how many copies exist.
`reorder_lumps` names its source `from_namespace` instead of `namespace`,
because there `namespace` is the destination region.

## Copy-on-write

The `IWAD` and the resource layers are **read-only**: no edit tool changes
them, and `save_wad` writes the working archive (or the merged view) out of
them, never over them. The editing tools therefore behave like this:

- The lump is in the **working archive** → it is changed there, in place.
- The lump is only in a layer (the `IWAD` or a resource `PWAD`) → the tool
  **creates an override in the working archive** instead of failing:
  - map data (`THINGS`, `LINEDEFS`, …): the whole map block is copied in
    first — header, every member and the block's `GL` nodes, as the merged
    view shows them, so overrides from higher layers come along as the
    merged bytes — then the copy is written. This is one undoable edit.
  - a namespace member (`FLOOR1` in `flats`, a sprite, …): the override is
    inserted into that region of the working archive, whose markers are
    created when it has none.
  - anything else (a loose global like `PLAYPAL` or `DEHACKED`): appended.
  `replace_lump` reports `"created_override": true` in these cases.
- `remove_lump`, `rename_lump` and `reorder_lumps` have no override to
  create: a name the working archive does not hold is `not_found`. An
  override can only shadow what a layer holds, never take it out of the
  engine's view: the lump stays in the `IWAD` and an empty `replace_lump`
  (`hex: ""`) just empties the copy. Removing a map member that way breaks
  the map, so prefer replacing its contents.

Verify any of it with `list_lumps scope=working` (what changed) and
`list_lumps scope=merged` (what an engine would see).

## `get_lump`

- Purpose: read one lump's contents, in whichever delivery form suits the
  task.
- Arguments: the selector fields plus:
  - `format` (string, default `"auto"`): `"info"`, `"text"`, `"hex"` or
    `"url"`.
  - `start` (number, default 0) and `length` (number, default 256, max
    4096) — the `hex` window, clipped to the payload; a `start` past the
    end is `invalid_argument`.
  - `max_chars` (number, default 100000) — the `text` character cap.
- Returns: the `list_lumps` row fields (`index`, `name`, `size`, `offset`,
  `kind`, `namespace`, `map`, `source`) plus `"format"` (what actually ran)
  and the format's fields:
  - `info` — nothing more.
  - `text` — `"text"` (UTF-8, invalid sequences replaced), `"lossy"` (a
    sequence was replaced), `"truncated"` (`max_chars` cut the text).
  - `hex` — `"hex"` (space-separated uppercase bytes, e.g.
    `"4D 55 53 1A"`), `"start"`, `"length"` (bytes actually returned).
  - `url` — `"asset": {"url", "filename", "mime", "size"}` holding the raw
    bytes on the asset server; the file name is the lump name plus an
    extension from the kind (`png`→`.png`, `jpeg`→`.jpg`, `gif`→`.gif`,
    `wav`→`.wav`, `ogg`→`.ogg`, `mp3`→`.mp3`, `flac`→`.flac`,
    `midi`→`.mid`, `mus`→`.mus`, text kinds→`.txt`, otherwise `.lmp`).
    No base64 crosses the stdio channel.
  - `auto` — `text` for text-like kinds (`text`, `dehacked`, `decorate`,
    `zscript`, `mapinfo`), `info` for empty lumps, and `url` plus
    `"preview_hex"` (the first 64 bytes as hex) for everything else.
- Images and sounds have dedicated decoding tools (to be added later);
  until then, or for any whole-binary read, use `format: "url"`.
- Errors: `invalid_argument` (bad selector fields, unknown `format`, `hex`
  `length` above 4096, `start` past the payload end), `not_found` (no
  match, `occurrence` out of range, `layer:N` not loaded).

## `extract_lump`

- Purpose: write one lump's raw bytes to a file.
- Arguments: the selector fields plus `path` (string, absolute filesystem
  path) and `overwrite` (bool, default `false`).
- Returns: `{"path", "bytes"}` — the file written and its size.
- Errors: `invalid_argument` (relative `path`, bad selector),
  `not_found` (`path`'s parent directory missing, no matching lump),
  `already_exists` (the target exists, or appears before the write lands,
  and `overwrite` was not passed), `io_error` (the write failed). The write
  is atomic through a uniquely named staging file beside the target:
  readers see the old file or the complete new one, never a mix, and the
  no-overwrite refusal is decided by the landing write itself, not by an
  earlier check a concurrent writer could slip past.

## Placement

The inserting tools (`add_lump` and the two import tools) take the same four
optional placement fields, and **at most one of them** (two is
`invalid_argument`):

- `index` (number) — insert at this directory position of the working
  archive; valid range `0..=current length`, anything larger is
  `invalid_argument`.
- `before` / `after` (string) — insert just before/after the **last** lump
  with that name in the working archive; a name it does not hold is
  `not_found`.
- `namespace` (string) — insert just before that region's end marker (any
  alias counts: `F_END`, `FF_END`, ...). When the working archive has no
  such region, its canonical markers are created at the end and the lump
  sits between them (`S_START`/`S_END`, `F_START`/`F_END`,
  `P_START`/`P_END`, `C_START`/`C_END`, `A_START`/`A_END`,
  `V_START`/`V_END`, `HI_START`/`HI_END`, `TX_START`/`TX_END`).
- none — append at the end.

`reorder_lumps` takes these same fields for its destination, with two
differences: it **requires** one of them (there is no append), and it
resolves them **after** the moved run has been taken out of the directory —
see `reorder_lumps` below.

Verify placement results with `list_lumps scope=working`.

## `add_lump`

- Purpose: add a new lump to the **working archive** as one undoable edit.
- Arguments: `name` (string, 1 to 8 printable ASCII characters) plus at
  most one payload field — `text` (UTF-8 written as-is), `hex` (bytes as
  hex digits, spaces allowed) or `source` (an absolute path or URI,
  fetched) — and no payload field at all makes an **empty lump**, which is
  how you write a directory marker. Plus the placement fields above and
  `allow_duplicate` (bool, default `false`).
- Duplicate rule: when the working archive already holds a lump with this
  name **in the same namespace** (global, i.e. no region, counts as its
  own namespace) the call is `already_exists` unless `allow_duplicate` is
  true. The same name in a different region is not a duplicate.
- Returns: `{"name", "index", "size", "namespace"}` — `index` is the
  position in the working archive, `namespace` the region it landed in or
  `null`. The undo label is `add_lump NAME`.
- Errors: `invalid_argument` (bad name, two payload fields, bad hex, two
  placement fields, `index` past the end, unknown `namespace`),
  `not_found` (missing `before`/`after` anchor; `source` fetch failures
  pass through as their own codes), `already_exists` (a duplicate without
  `allow_duplicate` — the message suggests `allow_duplicate` or
  `replace_lump`).

## `import_asset` / `import_file_as_lump`

- Purpose: fetch a file and insert its bytes into the working archive
  **without any conversion**, as one undoable edit. The two names are
  identical; clients may call either.
- Arguments: `source` (required — absolute path, `file://` URI,
  `http(s)://` URL or an uploaded asset URL), optional `name`, the
  placement fields and `allow_duplicate` as in `add_lump`. Without `name`
  the lump name is derived from the fetched file name: its extension
  stripped, uppercased, keeping letters/digits/underscore, trimmed to 8
  characters — when nothing usable remains (e.g. `.png`) the call is
  `invalid_argument` asking for `name`.
- Returns: the `add_lump` shape plus `"source"` (the fetched source
  echoed). The undo label is `import NAME`.
- Errors: as `add_lump`, plus the `source` fetch errors (`not_found` for a
  missing file or evicted asset, `fetch_error`, `limit_exceeded`, ...).

## `replace_lump`

- Purpose: overwrite one lump's payload — the way to change anything a
  read-only archive owns. Copy-on-write (above) decides *where* the bytes
  land; you never have to copy a map or a region first.
- Arguments: the selector fields `name`, `namespace`, `occurrence` (no
  `scope`), plus **exactly one** payload field: `text` (UTF-8 written
  as-is), `hex` (bytes as hex digits, spaces allowed) or `source` (absolute
  path, `file://` URI, `http(s)://` URL or uploaded asset URL, fetched for
  its bytes). None or several is `invalid_argument`; there is no empty form
  — that is what `add_lump` is for.
  `occurrence` indexes the **working archive's** matches (default the last);
  when the name only a layer holds, the winning copy of the merged view is
  the one overridden.
- Returns: `{"name", "index", "size", "created_override"}` — `index` counts
  positions in the working archive, `size` is the new payload length, and
  `created_override` is `true` when the lump had to be brought in first
  (whole map block copied, region insert, or append). The undo label is
  `replace_lump NAME`, and a block copy plus its write is that one step.
- Errors: `invalid_argument` (bad name, unknown `namespace`, bad hex, zero
  or two payload fields), `not_found` (no loaded archive holds the name;
  `occurrence` past the working matches; `source` fetch failures pass
  through with their own codes).

## `remove_lump`

- Purpose: take lumps out of the **working archive** in one undoable step.
- Arguments: `names` (array of strings, at least one), `namespace` (applies
  to every name), `occurrence` (allowed only when `names` holds exactly one
  name — with several it is `invalid_argument`), `all_occurrences` (bool,
  default `false`: remove only the selected, by default last, copy of each
  name).
- All or nothing: if any name is missing from the working archive the call
  is `not_found` and **nothing** is removed. Layer lumps cannot be removed
  at all — the error says so and points at `replace_lump`.
- Returns: `{"removed": [{"name", "index"}]}` in directory order, with the
  indices the lumps held **before** the removal. The undo label is
  `remove_lump A, B, ...`, listing the names in the order you gave them.
- Errors: `invalid_argument` (empty `names`, an unparsable name, an unknown
  `namespace`, `occurrence` beside several names), `not_found` (a name the
  working archive lacks, or an `occurrence` past its matches).

## `rename_lump`

- Purpose: rename one lump of the **working archive**, keeping its position
  and payload.
- Arguments: `name`, `new_name` (both 1 to 8 printable ASCII characters,
  uppercased), `namespace`, `occurrence` as in `replace_lump`, and
  `allow_duplicate` (bool, default `false`).
- Duplicate rule: another working lump already called `new_name` **in the
  same region** (no region counts as its own) is `already_exists` unless
  `allow_duplicate` is true. Nothing changes in that case.
- Returns: `{"old_name", "new_name", "index"}`. The undo label is
  `rename_lump OLD -> NEW`.
- Errors: `invalid_argument` (either name unparsable, unknown `namespace`),
  `not_found` (the working archive holds no match — a layer lump cannot be
  renamed; override it with `replace_lump`, or `add_lump` the new name and
  `remove_lump` the copy).

## `reorder_lumps`

- Purpose: move a contiguous run of lumps within the **working archive**, in
  one undoable step. Order is load-bearing in a WAD — namespaces, map blocks
  and markers all depend on position — so this is how you re-place a lump,
  slide a whole map block, or push a region's members back together.
- What to move, **exactly one of**:
  - `name` (string) — the first lump of the run, resolved in the working
    archive only, with the usual `occurrence` refinement (default the last
    match) and `from_namespace` (string) to keep only matches inside that
    region. `from_namespace` is the source-side name of the selector's
    `namespace`, because `namespace` here is the *destination* region.
  - `from_index` (number) — the working archive's directory position the run
    starts at.
  - plus `count` (number, default 1): how many contiguous lumps travel
    together. A whole map block is `name: "MAP01", count: 11` (header, its
    members and any `GL` run); read the block's size off
    `list_lumps map=MAP01` first.
- Where to put it, **exactly one** of the placement fields, resolved
  **after the run has been taken out of the directory**:
  - `index` (number) — a position in the directory *without* the moved
    lumps, so `0..=(length - count)`. Moving the lump at index 2 to `index:
    4` of a five-lump archive sends it to the end, not one short of it.
  - `before` / `after` (string) — beside the last lump with that name
    **among the remaining lumps**. Naming a lump that is inside the run is
    `invalid_argument`: a run cannot be placed beside itself.
  - `namespace` (string) — just before that region's end marker as the
    directory stands without the run; when the working archive has no such
    region its canonical markers are created around the run.
  - There is **no append default**: no destination field is
    `invalid_argument`.
- Returns: `{"moved": [names in directory order], "from_index": n,
  "to_index": m}` — `to_index` is where the run's first lump now stands, and
  `to_index + moved.len() - 1` where the last one does. The undo label is
  `reorder_lumps NAME`, `NAME` the run's first lump: one step takes the whole
  run back.
- Errors: `invalid_argument` (both or neither of `name` and `from_index`,
  `occurrence`/`from_namespace` beside `from_index`, `count` 0, a run
  reaching past the end of the working archive, two destination fields, no
  destination, `index` past the shortened directory, an anchor inside the
  run, an unparsable name or region), `not_found` (`name` or the region
  filter matches nothing in the working archive, `occurrence` past the
  matches, a `before`/`after` anchor the remaining directory lacks). A
  refusal changes nothing and leaves no history entry.
- A lump the `IWAD` or a resource layer owns cannot be moved: the working
  archive holds nothing to move, so the call is `not_found` — `add_lump` a
  copy (or `replace_lump` to override) and move that.

## Notes

- The editing tools touch **only the working archive**; read-only layers
  never change, and every edit is undoable (`get_session_info` lists the
  undo labels). Nothing is on disk until `save_wad`. Changing a lump a layer
  holds means an override in the working archive — see Copy-on-write; you
  cannot delete, rename or move a layer's lump.
- Namespace regions come from marker pairs (`S_START`/`S_END` and friends),
  so region **markers themselves** carry the region's `namespace` and the
  kind `marker`; filters on `namespace` therefore include them.
- `offset` is derived, not read from a file: it answers "where would this
  lump's bytes sit if I saved its source archive right now". Compare it
  with `save_wad`'s output, never with a raw file offset of an archive
  that was edited after being read.
