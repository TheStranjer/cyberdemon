# Maps tools

Map-level reading and the session-wide map selection: `list_maps`,
`get_map_lumps`, `get_map_summary`, `assemble_map` and `set_active_map`.
Later map tools (geometry, building, node building) join this group and
share its conventions.

## How maps are stored

A map is a block in the lump directory: a zero-size **header lump** carrying
the map's name (`MAP01`, `E2M7`, ...) followed by its **member lumps**.
The members present decide the format:

- `doom` — the vanilla binary set: `THINGS`, `LINEDEFS`, `SIDEDEFS`,
  `VERTEXES`, then usually `SEGS`, `SSECTORS`, `NODES`, `SECTORS`, `REJECT`,
  `BLOCKMAP`.
- `hexen` — the binary set with wider records plus a `BEHAVIOR` lump; the
  `BEHAVIOR` is what identifies the block as Hexen.
- `udmf` — a text block running from `TEXTMAP` to `ENDMAP`; the `TEXTMAP`
  begins with a namespace declaration (`zdoom`, `glbdoom`, `doom`, ...) and
  the block may carry `BEHAVIOR` and `ZNODES` members.

Optionally a **GL node block** sits right after the map: a header named
`GL_<mapname>` (or `GL_LEVEL`) with `GL_VERT`, `GL_SEGS`, `GL_SSECT`,
`GL_NODES` and `GL_PVS`. A block "has nodes" when its `NODES` lump carries
bytes or a `ZNODES` lump is present; tools that rebuild nodes overwrite
them. The merged view applies last-archive-wins to whole map blocks: a
`PWAD` holding `MAP01` replaces the `IWAD`'s block entirely.

## The `map` parameter

Every tool with an optional `map` parameter resolves it the same way:

1. an explicit `map` (1 to 8 printable `ASCII` characters, any case) is
   used as given;
2. else the session's **active map**, chosen with `set_active_map`;
3. else the merged view's only map, when it holds exactly one;
4. else the call fails `invalid_argument` — pass `map` or call
   `set_active_map`.

## `list_maps`

- Purpose: survey which maps a scope holds, in what format and from which
  archive, before touching any of them.
- Arguments (all optional): `scope` — `"merged"` (the default),
  `"working"` or `"layer:N"` (0 is the `IWAD`).
- Returns: `{"maps": [{name, format, udmf_namespace, source, lumps,
  has_nodes, has_gl_nodes}]}` in directory order. `udmf_namespace` is the
  namespace a `UDMF` `TEXTMAP` declares (`null` for binary blocks and for
  blocks that do not decode); `source` is `"working"` or `{"layer": n}`
  like `list_lumps` rows; `lumps` lists the member lump names in directory
  order, an attached GL run included, without the map header itself.
- Errors: `invalid_argument` (unknown `scope` name), `not_found`
  (`layer:N` not loaded).

## `get_map_lumps`

- Purpose: the raw lump set of one map block — what `build_nodes` or a
  node copy would replace — with sizes and `list_lumps` type hints.
- Arguments (all optional): `map` (see the rule above), `scope` (as in
  `list_maps`; the `map` default always resolves against the merged view,
  so pass `map` explicitly with a `working`/`layer:N` scope), `urls`
  (boolean, default `false`).
- Returns: `{"map", "format", "lumps": [{name, size, kind}]}` in directory
  order, the GL run included. With `urls` true every non-empty lump's raw
  bytes are published as an asset named `<MAP>_<LUMP>.lmp` and the row also
  carries `"asset": {url, filename, mime, size}`; zero-size marker lumps
  get none. Fetch the URLs with any HTTP client.
- Errors: `invalid_argument` (unparsable `map`, unknown `scope`, or no map
  resolvable), `not_found` (no such map block in the scope, or the layer
  is not loaded).

## `get_map_summary`

- Purpose: a cheap, compact overview of one map — what it holds, how big
  it is and whether it is built — before deciding which tools to call on
  it. The map is decoded, never measured in detail.
- Arguments (all optional): `map` (see the rule above), `scope` (as in
  `list_maps`; the `map` default always resolves against the merged view).
- Returns: `{"name", "format", "udmf_namespace", "source", "counts",
  "bounds", "lines", "heights", "things_by_type", "player_starts",
  "textures_used", "flats_used", "nodes}`:
  - `source`: `"working"` or `{"layer": n}` — where the scope took the
    block's header from, as in `list_maps`.
  - `counts`: `{things, vertices, linedefs, sidedefs, sectors}` — how many
    of each element the map holds.
  - `bounds`: `{min_x, min_y, max_x, max_y, width, height}` from the
    vertices, or `null` when the map holds none.
  - `lines`: `{one_sided, two_sided, with_special}` — `two_sided` counts
    the linedefs with both sides set, `one_sided` the rest, and
    `with_special` those with a nonzero special.
  - `heights`: `{floor_min, floor_max, ceiling_min, ceiling_max}` across
    the sectors; all `null` when the map has none.
  - `things_by_type`: at most 50 `{type, count}` rows, most numerous
    first, ties by ascending type number.
  - `player_starts`: which of the thing types 1, 2, 3 and 4 are present.
  - `textures_used` / `flats_used`: the sidedef textures (upper, lower,
    middle) and sector flats, sorted, deduplicated, `"-"` left out.
  - `nodes`: `{"present", "segs", "subsectors", "nodes", "reject_bytes",
    "blockmap_bytes", "znodes", "gl"}` from the block's lump sizes.
    `present` needs bytes in `NODES` or `ZNODES`; `segs`, `subsectors`
    and `nodes` are vanilla record counts (`SEGS`/12, `SSECTORS`/4,
    `NODES`/28). When `NODES` starts with an extended-format magic
    (`XNOD`, `ZNOD`, `XGLN`, `ZGLN`, ...) those three counts are replaced
    by `"nodes_format"` carrying the four-letter magic. `znodes` is true
    when a `ZNODES` lump exists, `gl` when a `GL_` lump does.
- Errors: `invalid_argument` (unparsable `map`, unknown `scope`, or no map
  resolvable), `not_found` (no such map block in the scope, or the layer
  is not loaded), `format_error` (the block does not decode).

## `assemble_map`

- Purpose: the whole map as one typed graph with every reference already
  resolved — vertices looked up, linedef sides mapped to sectors, sidedefs
  back-referenced, things placed into sectors, sectors given boundary,
  neighbour and occupant lists — so the map can be reasoned about without
  chasing indices.
- Arguments (all optional): `map` (see the rule above), `scope` (as in
  `list_maps`), `include` — a subset of `"vertices"`, `"linedefs"`,
  `"sidedefs"`, `"sectors"`, `"things"` (default all five; an unknown name
  is `invalid_argument`), `inline_limit` — the element count up to which
  the graph is returned in full (default 2000, maximum 20000; `0` or
  above the maximum is `invalid_argument`).
- Returns, while the included arrays hold at most `inline_limit` elements,
  `{map, problems}` plus one array per included kind:
  - `vertices`: `{index, x, y, linedefs: [indices]}`.
  - `linedefs`: `{index, v1: {index, x, y}|null, v2: ..., length (map
    units, floor-rounded)|null, flags, flag_names (named with the map's
    own flag family), special, tag, args, front|back: {sidedef, sector,
    upper, middle, lower, x_offset, y_offset}|null}` — `front` is the
    `Doom` right side, `back` the left; absent sides are `null`.
  - `sidedefs`: `{index, sector, linedef|null, side:
    "front"|"back"|null, upper, middle, lower, x_offset, y_offset}` —
    `linedef`/`side` name the first line using the sidedef (`null` for
    orphans).
  - `sectors`: `{index, floor_height, ceiling_height, floor_flat,
    ceiling_flat, light, special, tag, linedefs (boundary lines),
    sidedefs, neighbors (sectors across two-sided lines), things
    (occupants by point-in-sector), bbox [min_x, min_y, max_x, max_y]
    |null}`.
  - `things`: `{index, type, x, y, z, angle, flags, flag_names, tid,
    special, args, sector|null}` — `sector` is `null` when the point sits
    outside every sector; a point exactly on a line counts as inside the
    sector on that line's front side, nested sectors resolve to the
    smallest bounding box.
  - `problems`: strings naming every dangling vertex/sidedef/sector
    reference found while resolving. Broken references never fail the
    call; the unresolvable part reads `null`.
- Above `inline_limit` the arrays are not inlined: the full JSON is
  published as `<MAP>_graph.json` (`application/json`) and the result is
  `{map, counts: {vertices, linedefs, sidedefs, sectors, things,
  problems}, asset: {url, filename, mime, size}}`. Fetch the URL with any
  HTTP client for the same document.
- Errors: like `get_map_summary` (`not_found` unknown map,
  `invalid_argument` unparsable name/scope, no resolvable default, unknown
  `include` name, or `inline_limit` `0`/above 20000, `format_error`
  undecodable block).

## `set_active_map`

- Purpose: choose the map the `map` parameter defaults to for the whole
  connection, so later calls can omit it.
- Arguments: `map` (string or `null`) — the map must be a block in the
  merged view; `null` or an omitted argument clears the selection.
- Returns: `{"active_map": name|null}` — what is selected now (also shown
  by `get_session_info`).
- Errors: `invalid_argument` (unparsable name), `not_found` (no such map
  block; the previous selection stands).
- This is a session setting, not an undoable edit: `undo`/`redo` never
  record it, and `undo` only clears it as a side effect when the stepped-
  back working archive removes the map from the merged view.
