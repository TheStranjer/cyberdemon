# Inspect tools

Comparing WAD states: what a `PWAD` overrides, what two archives differ in,
what changed since the last look — and removing what the working archive
does not need (`optimize_wad`).

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

## `optimize_wad`

- Purpose: find and remove redundant lumps in the **working archive**.
  Planning is pure (identical bytes by length, then a 64-bit FNV-1a hash,
  then bytes; lump keys are the same namespace/name/occurrence units
  `diff_wads` uses); applying the plan is one undoable step.
- Arguments:
  - `operations` (array of strings, optional): which sweeps to run. The
    default is every **safe** sweep — the ones whose removal never changes
    what an engine sees:
    - `identical_overrides` — working lumps, and whole map blocks, that are
      byte-identical to what the base view (IWAD + resource layers) already
      provides for the same namespace and name. A working lump only pairs
      with the base lump in the same region and the same occurrence.
    - `empty_namespaces` — adjacent start/end marker pairs with nothing
      between them; inner markers (`F1_START`/`F1_END`) that end up empty
      once their inner pair goes are removed in the same call.
    - `shadowed_duplicates` — a working global whose name appears more than
      once keeps only the last copy (vanilla last-match-wins). Names that
      ZDoom-family ports concatenate (`DECORATE`, `ZSCRIPT`, `SNDINFO`,
      `MAPINFO`, `ZMAPINFO`, `UMAPINFO`, `LANGUAGE`, `GLDEFS`, `ANIMDEFS`,
      `KEYCONF`, `TEXTURES`, `DECALDEF`, `LOCKDEFS`, `SBARINFO`, `TERRAIN`,
      `SNDSEQ`, `LOADACS`, `MENUDEF`, `FONTDEFS`, `CVARINFO`, `MODELDEF`,
      `VOXELDEF`, `TEAMINFO`, `X11R6RGB`, `DEHACKED`) are **never** pruned;
      their duplicated names come back in `kept_concatenated`.
    - `unused_pnames` — trim the working `PNAMES` to the entries the merged
      `TEXTURE1`/`TEXTURE2` reference, renumbering those lumps' patch
      references (written back as working overrides). Does nothing when the
      working archive has no `PNAMES` or its `PNAMES` is itself a redundant
      copy the plan removes.
    Opt-in sweeps, named explicitly:
    - `unused_patches` — working `patches`-namespace lumps no texture
      references by PNAMES name (needs a `PNAMES` in the merged view;
      without one nothing is touched).
    - `unused_flats` — working `flats`-namespace lumps no sector of any
      merged map uses. **Warning:** other PWADs loaded after this one in a
      game can still use them; this sweep can only see this session.
  - `dry_run` (bool, default `true`): report the plan without changing
    anything. `false` applies the whole plan as ONE undoable edit labelled
    `optimize_wad`; an empty plan records no history entry.
- Returns:

  ```json
  {
    "dry_run": true,
    "actions": [
      {"operation": "identical_overrides", "lump": "PLAYPAL", "index": 0, "bytes": 10752},
      {"operation": "identical_overrides", "map": "MAP01", "index": 12, "bytes": 4096},
      {"operation": "unused_pnames", "lump": "TALK", "index": 3, "bytes": 8}
    ],
    "bytes_saved": 14856,
    "lumps_removed": 3,
    "kept_concatenated": ["DECORATE"]
  }
  ```

  `index` is the working directory position of the removed entry (the map
  block's header index for a `"map"` row; the `PNAMES`-list position for a
  trimmed name, which removes no directory entry). `bytes` is the payload
  freed by that row; `bytes_saved` sums the rows and `lumps_removed` counts
  the directory entries. Actions come in sweep order (the order listed
  above), directory order within a sweep. Repeated runs converge: a sweep
  only judges the current snapshot, so e.g. markers left empty by removed
  region members need a second call.
- Errors: `invalid_argument` for an unknown operation name (the message
  lists all six); `format_error` when a requested sweep cannot read its
  input — a malformed `PNAMES`/`TEXTURE1`/`TEXTURE2` (`unused_pnames`,
  `unused_patches`) or an undecodable merged map (`unused_flats`).
- Examples:

  ```json
  {}                                                        // plan every safe sweep
  {"dry_run": false}                                        // apply the same plan
  {"operations": ["unused_patches", "unused_flats"]}        // deep, opt-in cleanup
  {"operations": ["identical_overrides"], "dry_run": false} // just the copy-paste leftovers
  ```

  Review what goes with `diff_wads` (`identical_override` rows) and
  `get_wad_info`; `undo` steps the whole run back in one move.
