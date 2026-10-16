# Inspect tools

Comparing WAD states: what a `PWAD` overrides, what two archives differ in,
what changed since the last look.

## Sides

Every `diff_wads` side is a string naming one directory of lumps:

| Side         | Meaning                                                            |
|--------------|--------------------------------------------------------------------|
| `"working"`  | The editable working archive alone.                                 |
| `"merged"`   | The directory an engine sees: `IWAD`, resource layers, `working`.   |
| `"base"`     | The read-only layers only — what the session holds before any edit. |
| `"layer:N"`  | Read-only layer `N` alone (`0` is the `IWAD`).                      |
| `/path.wad`  | An absolute path to a `WAD` file, read fresh; never loaded in.      |

The defaults are `a = "base"`, `b = "working"`: "what does my `PWAD`
override?". Resource layers are numbered in load order; `get_session_info`
lists them.

## Statuses

In the default `base`-to-`working` mode every reported unit comes from the
working side:

- `new` — only `b` holds it;
- `override` — both hold it and the bytes differ;
- `identical_override` — both hold it with the same bytes: usually a
  copy-on-write artifact worth cleaning up;
- `unchanged` — identical and the layers never provided it either (with
  `include_unchanged` this is how a byte-exact copied map block reports).
  `removed` cannot happen in this mode; units only the layers hold are
  simply not yours to report.

Any other side pair uses the neutral vocabulary: `added` (only in `b`),
`removed` (only in `a`), `changed` (bytes differ), `unchanged`.

Units: map blocks compare as one unit by map name (changed when any member
lump's bytes differ); every other lump is keyed by its namespace, name and
occurrence index within that key — a flat and a patch of one name are
different units, and duplicate same-name lumps pair up copy by copy.
Namespace markers (`F_START` and friends) take part in no comparison.
Content is compared by length, then a 64-bit FNV-1a hash, then bytes.

## `diff_wads`

- Purpose: diff two sides and report every unit that differs.
- Arguments:
  - `a` (string, optional, default `"base"`): the left side (see Sides).
  - `b` (string, optional, default `"working"`): the right side.
  - `include_unchanged` (bool, optional, default `false`): also list units
    that match.
  - `limit` (number, optional, default 500, max 5000): rows per list;
    `truncated` says whether either list held more.
- Returns:

  ```json
  {
    "a": "base", "b": "working",
    "summary": {"added": 0, "removed": 0, "changed": 0, "unchanged": 0,
                "override": 1, "identical_override": 0, "new": 2},
    "lumps": [
      {"name": "FLOOR1", "namespace": "flats", "status": "override",
       "size_a": 4096, "size_b": 4096}
    ],
    "maps": [
      {"name": "MAP02", "status": "new", "lumps": [], "counts_a": null,
       "counts_b": {"things": 2, "linedefs": 7, "sidedefs": 8,
                    "vertices": 6, "sectors": 2}}
    ],
    "truncated": false
  }
  ```

  `a`/`b` echo the side strings used. `summary` counts every compared unit
  — lumps and map blocks alike — before filtering and paging, so it is
  correct even with `limit` in play. `lumps` and `maps` are in `a`'s
  directory order with `b`'s arrivals last. `size_a`/`size_b` are `null`
  on the side that lacks the unit. A changed map lists the member lumps
  whose bytes differ (statuses there are always `added`/`removed`/
  `changed`) and, when both blocks parse, each side's element counts;
  `counts_*` is `null` when a block is absent or does not decode.
- Errors: `invalid_argument` for an unknown side name or a relative path,
  for `limit` outside `1..=5000`; `not_found` for a missing file or an
  out-of-range `layer:N`; `format_error` for a file that is not a readable
  `WAD`.
- Examples:

  ```json
  {}                                        // what does my PWAD override?
  {"a": "base", "b": "merged"}              // layers vs what an engine sees
  {"a": "layer:0", "b": "/abs/path.wad"}    // two files on disk
  {"a": "working", "b": "working", "include_unchanged": true}
  ```

  To review an override, read the unit with `get_lump` on both scopes
  (`"base"` is not a `get_lump` scope; use its `"layer:N"` or `"merged"`).
