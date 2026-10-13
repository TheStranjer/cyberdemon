# Lumps tools

Directory browsing and single-lump reads over the session's archives.

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
  - `limit` (number, default 200, 1 to 2000).
- Criteria combine with AND.
- Returns: `{"scope", "count", "truncated", "lumps": [rows +
  "match_offset"]}` — `match_offset` is the byte offset of the first
  content match (lowest when both `contains_text` and `contains_hex` were
  given), `null` when no content criterion was used; `truncated` says more
  lumps matched than `limit` allowed.
- Errors: `invalid_argument` (no criterion at all, bad `scope`/`kind`/
  `namespace`/`limit`, or hex text that is not whole hex pairs), `not_found`
  (`layer:N` not loaded).

## The lump selector

`get_lump` and `extract_lump` pick one lump with the same four fields,
which later lump tools reuse:

- `name` (string, required) — the lump name, 1 to 8 printable ASCII
  characters, case-insensitive (uppercased). Anything else is
  `invalid_argument`.
- `scope` (string, default `"merged"`) — as `list_lumps`.
- `namespace` (string, optional) — keep only matches inside that region
  (`flats`, `sprites`, ...). Without it, matches in any region and loose
  lumps all count.
- `occurrence` (number, optional) — the 0-based index **among the matching
  lumps**, in the scope's directory order. The default is the **last**
  match, which is the copy that wins an engine's lookup of that name. Set
  it to read an earlier (losing) copy.

No match is `not_found` (`"lump FOO not found in merged"`); an occurrence
past the last match is `not_found` naming how many copies exist.

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
  `already_exists` (the target exists and `overwrite` was not passed),
  `io_error` (the write failed). The write is atomic: readers see the old
  file or the complete new one.

## Notes

- Namespace regions come from marker pairs (`S_START`/`S_END` and friends),
  so region **markers themselves** carry the region's `namespace` and the
  kind `marker`; filters on `namespace` therefore include them.
- `offset` is derived, not read from a file: it answers "where would this
  lump's bytes sit if I saved its source archive right now". Compare it
  with `save_wad`'s output, never with a raw file offset of an archive
  that was edited after being read.
