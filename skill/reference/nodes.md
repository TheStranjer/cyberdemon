# Nodes tools

The node-builder output of a map: `get_nodes`, `get_reject` and
`get_blockmap` read it, `build_nodes` rebuilds the BSP tree itself —
as the vanilla `SEGS`/`SSECTORS`/`NODES` run or `ZDoom`'s extended
lump (`NODES`/`ZNODES`) — `build_blockmap` and `build_reject`
rebuild the two lumps that need no tree, and `build_all_nodes`
rebuilds all of it in one undoable step.

## What these lumps are for

An engine never solves geometry at runtime: a node builder precomputes a
BSP tree over the map's linedef segments so rendering and sound can walk
the map from any point. The result lives in five generated lumps inside
the map block:

- **SEGS** — 12-byte records: the split pieces of every linedef side
  (`v1`, `v2`, an angle, the `linedef` and side `direction`, and the
  `offset` along the line where the seg starts).
- **SSECTORS** — 4-byte records: convex leaf regions, each a run of
  consecutive segs (`count`, `first`).
- **NODES** — 28-byte records: the interior tree. Each node holds a
  partition line (`x`,`y` along `dx`,`dy`), the bounding boxes of its two
  sides and two child words; a child with `bit 15` set is a subsector
  index, otherwise a node index. The last node is the root.
- **REJECT** — a `sectors × sectors` bit matrix, row-major, least
  significant bit first: bit `i*sectors+j` set means sector `j` is never
  visible from sector `i`. The expected size is `ceil(sectors²/8)` bytes;
  engines tolerate wrong sizes, and so do these tools — a mismatch is
  reported as `size_ok: false`, never as an error.
- **BLOCKMAP** — a grid of 128×128 map-unit cells; an 8-byte header (`x`
  origin, `y` origin, columns, rows), then one `u16` offset per cell
  (counted in 16-bit words from the lump start) pointing at a blocklist:
  `0x0000`, the cell's linedef indices, `0xFFFF`. Block `(col, row)`'s
  offset sits at table position `row*columns+col`.

`ZDoom` replaces the three-lump tree with one extended lump starting with
the magic `XNOD` (raw) or `ZNOD` (zlib-compressed after the magic), kept
in `NODES` for binary maps or in a `ZNODES` lump for `UDMF` maps. It adds
fraction-precise (`16.16`) vertices beyond `VERTEXES`, wider child words
(`bit 31` marks the leaves) and side bytes instead of directions. These
parse into the same shape. The glBSP (`GL_*`) lumps are a separate block
attached right after the map block (below); their records are counted for
a `gNd2` (glBSP v2) run and otherwise **reported, never parsed**, and the
in-`NODES` `XGLN`/`ZGLN`/`XGL2`/`ZGL2`/`XGL3`/`ZGL3` magics are reported
with their size only.

## The `map`, `scope` and `detail` parameters

All three readers take `map` and `scope` exactly as the maps tools do
(`reference/maps.md`: the active map or the merged view's only map by
default; `"merged"`, `"working"` or `"layer:N"`). `detail` is
`"summary"` (the default: numbers only) or `"full"`: `full` publishes the
parsed structure as a JSON asset and attaches
`"asset": {url, filename, mime, size}` to the result. A missing or empty
lump never publishes anything. The three single-lump builders and
`build_all_nodes` take `map` the same way and always edit the working
archive; they carry no `scope` or `detail`.

## `get_nodes`

`{map?, scope?, detail?}` → `{map, format, bytes, segs, subsectors,
nodes, extra_vertices, root, depth, gl {present, magic, segs, subsectors,
nodes, vertices, lumps}}`

- `format` is `"none"` (no node bytes anywhere; the counts read `0`),
  `"vanilla"`, `"xnod"`/`"znod"`, or a lowercase GL magic (`"xgln"`,
  `"zgln"`, `"xgl2"`, `"zgl2"`, `"xgl3"`, `"zgl3"`) — for the GL shapes
  the counts and `depth` are `null` and `bytes` carries the lump size.
- `bytes` is the size of the lump the format was read from (`NODES`, or
  `ZNODES` when `NODES` is empty).
- `extra_vertices` counts the `16.16` vertices an extended run adds to
  `VERTEXES`; vanilla runs report `0`.
- `root` is the root node's index (the last node), `depth` the node
  levels on the longest root-to-leaf path.
- `gl.lumps` maps each lump of the attached `GL_` block to its size;
  `gl.magic` is the four letters `GL_VERT` starts with (`gNd2`, `gNd5`,
  ...). For a `gNd2` run `gl.segs`, `gl.subsectors`, `gl.nodes` and
  `gl.vertices` count the records in `GL_SEGS`, `GL_SSECT`, `GL_NODES`
  and `GL_VERT`; they are `null` for any other magic or a lump whose
  size is not a whole number of records.

`detail: "full"` publishes `<MAP>_nodes.json` —
`{format, extra_vertices [{x, y}], segs [...], subsectors [...],
nodes [...]}`; segs and subsectors carry `u32` indices, node children are
tagged `{"node": i}` or `{"subsector": i}` and the added vertices are raw
`16.16` values. Nothing parsed (a GL magic, or no data) publishes
nothing.

## `get_reject`

`{map?, scope?, detail?}` → `{map, bytes, expected_bytes, sectors,
size_ok, all_zero, rejected_pairs}`. `rejected_pairs` counts set bits
inside the logical `sectors²` matrix (junk bytes beyond it never count).
`detail: "full"` publishes `<MAP>_reject.json` — `{sectors, rows}` with
one `'0'`/`'1'` string per sector, first character the row's first bit.

## `get_blockmap`

`{map?, scope?, detail?}` → `{map, bytes, origin {x, y}, columns, rows,
blocks, total_entries, max_lines_per_block, empty_blocks}`. `detail:
"full"` publishes `<MAP>_blockmap.json`, the whole parsed
`{origin_x, origin_y, columns, rows, blocks}` with `blocks` in row-major
order, each entry its block's linedef indices.

## `build_blockmap`

`{map?, compress? (true)}` → `{bytes, columns, rows, origin {x, y},
vanilla_safe, map, nodes_invalidated, copied_to_working}` — one undoable
edit that recomputes the `BLOCKMAP` from the map's current geometry and
writes it into the working archive (the lump is replaced, or inserted
after `REJECT` when the block has none). The grid is the vanilla one:
origins 8 map units below the lowest vertex, 128×128 map-unit cells,
`columns = (max_x - origin_x) / 128 + 1`, `rows` likewise. A linedef is
listed in every block whose closed square it touches (skipped only when
all four block corners lie strictly on the same side of its infinite
line), so a line exactly on a shared block edge is listed in both
neighbours and untouched blocks get empty lists. `compress: true` (the
default) shares one copy of identical blocklists; `false` gives every
block its own list and a bigger lump. `vanilla_safe` reports the lump is
at most 32767 words — what vanilla `Doom` reads with its signed offsets.
Linedefs whose vertex references do not resolve are listed in no block.
This rewrites only `BLOCKMAP` — no geometry changed, so
`nodes_invalidated` is `false`. A map with no vertices is
`invalid_argument`; a grid, linedef index or lump past the format's
shape is `limit_exceeded` ("blockmap too large").

## `build_reject`

`{map?, mode? ("zero" | "components")}` → `{bytes, sectors,
rejected_pairs, mode, map, nodes_invalidated, copied_to_working}` — one
undoable edit writing the `REJECT` lump (`ceil(sectors² / 8)` bytes, bit
`i * sectors + j`, row-major, least significant bit first). `mode`
`"zero"` (the default) clears the matrix: every sector may see every
other, always correct and what `ZDBSP` writes. `"components"` builds the
sector graph (an edge joins the two different sectors on the sides of a
two-sided linedef; sides or sectors that do not resolve join nothing) and
sets the bit for exactly the ordered pairs in *different* connected
components — sectors with no opening between them can never see each
other — while same-component pairs stay clear. `rejected_pairs` counts
the set bits written. Only `REJECT` changes (`nodes_invalidated` false);
an unknown `mode` is `invalid_argument`.

## `build_nodes`

`{map?, variant? ("classic" | "extended" | "gl"), split_cost? (8),
compress? (true)}` → `{variant, segs, subsectors, nodes, warnings, map,
nodes_invalidated, copied_to_working}` plus the per-variant keys below —
one undoable edit that rebuilds the map's BSP tree from its current
geometry with the in-house builder and writes the result. `variant`
selects the layout:

- `"classic"` (the default) writes the vanilla three-lump run —
  12-byte `SEGS`, 4-byte `SSECTORS`, 28-byte `NODES` with the root last
  — which vanilla `Doom` needs to play the map and every port can use.
  Binary maps only: on a `UDMF` map it fails `invalid_argument` (`UDMF
  maps use extended nodes`). Seg endpoints that fall between vertices
  append new vertices to `VERTEXES` (`split_vertices` counts them).
- `"extended"` writes `ZDoom`'s single extended lump: fraction-precise
  `16.16` split vertices, `u32` vertex/child indices and the `bit 31`
  leaf flag. Binary blocks get it in `NODES`, with `SEGS` and
  `SSECTORS` emptied (`ZDoom` ignores them when `NODES` is extended);
  `UDMF` blocks get a `ZNODES` lump between `TEXTMAP` and `ENDMAP`, and
  a stale `NODES`/`SEGS`/`SSECTORS` run there is dropped. The map's
  `VERTEXES` are never modified — the split points live inside the lump
  (`new_vertices` counts them). Use it for `ZDoom`-family ports, for
  `UDMF` maps (the only layout they read), and when a classic build
  refuses `limit_exceeded` on the vanilla record limits. `compress`
  (default `true`) `zlib`-compresses the payload behind the `ZNOD`
  magic; `false` stores it raw behind `XNOD`. The result adds `format`
  (`"ZNOD"`/`"XNOD"`) and `bytes` (the written lump's size).
- `"gl"` writes the classic run **and** the `glBSP` v2 block, for
  `OpenGL` ports (`GZDoom`, Crispy GL, Woof) and `glBSP`-compatible
  engines: every subsector becomes a closed convex polygon by stretching
  **minisegs** (segs with linedef `0xFFFF`) over the stretches of its
  region's edges that no real seg covers. Segs facing each other across
  a partition line get `partner` words pointing at one another
  (`0xFFFF` = none). The block is attached outside the map block, right
  after it: a zero-size `GL_<MAPNAME>` marker (or `GL_LEVEL` for map
  names longer than five characters) followed by `GL_VERT` (magic
  `gNd2` plus the fraction-precise vertices beyond `VERTEXES`),
  `GL_SEGS`, `GL_SSECT`, `GL_NODES` and an empty `GL_PVS`. A rebuild
  replaces the existing block; a geometry edit drops it like any other
  node data; `undo` takes it back with the classic run. Binary maps
  only: on a `UDMF` map it fails `invalid_argument` like `classic`. The
  result adds `gl_segs` (all `GL_SEGS` records, real and mini),
  `minisegs` (the `0xFFFF`-linedef filler) and `gl_vertices` (the
  `GL_VERT` rows), and keeps `split_vertices` from the classic run it
  writes alongside.

`split_cost` is the builder's judgment knob: what a candidate partition
is charged per seg it cuts (default `8`, added to the front/back
imbalance; the cheapest candidate wins, ties to the earliest). Raise it
to keep segs whole, lower it to prefer cutting.

How the result is measured: `segs`, `subsectors` and `nodes` count the
records written; `warnings` holds one note per subsector that spans
several sectors (not a failure — vanilla maps carry such leaves).
Classic split vertices are invisible to every other tool (no linedef
ends at them, so `reference/vertices.md`'s orphan rules apply) and are
shed automatically by the next geometry edit, which invalidates this
node run anyway. The shed reaches only the points that run's `SEGS`
records named, so a vertex appended after the build keeps itself (and the
split point behind it) instead of having its index moved under it.
Building nodes is a generated product, not geometry, so
`nodes_invalidated` is `false` here even though a classic
build grew the vertex list — run `get_nodes` afterwards to see the
tree. Undo takes the whole build (the lumps, the appended vertices)
back as one step.

A map with no linedef sides is `invalid_argument` (give the geometry
sides first); a region the builder cannot split is `internal` (adjust
`split_cost` or fix the geometry); values past a record limit — for
classic more than `65535` vertices, an `i16`/`u16` field, or more than
`32767` nodes or subsectors (`limit_exceeded` naming variant
`"extended"`, which takes over at `u32` indices), and for extended an
`i16`/`u16` node/seg field or a `16.16` coordinate past the `i32` range
(`limit_exceeded`: no layout holds wider), for `gl` a map-vertex or
`GL_VERT` index past `32767`, a `GL_SEGS` count past `65535`, a node
child past `32767` or a `16.16` vertex coordinate past `i32` — are
refused. A region that collapses below a triangle or a seg that lies
outside every edge of its region is `internal`. Unknown map is
`not_found`; an unparsable block is `format_error`.

## `build_all_nodes`

`{map?, variant? ("classic" | "extended" | "gl"), reject_mode? ("zero" |
"components"), compress? (true)}` → `{variant, nodes {...}, reject {...}
| null, blockmap {...} | null, map, nodes_invalidated,
copied_to_working}` — one undoable edit that rebuilds **all** of a map's
node data at once: this is the tool to run after geometry edits, before
`save_wad`. `variant` picks the node layout exactly as `build_nodes`
does; left out, a binary `Doom`/`Hexen` map takes `"classic"` and a
`UDMF` one `"extended"`. The split cost is the builder default `8`;
`build_nodes` is the tool for tuning it.

- Binary maps get all three: the tree (in the chosen layout), a fresh
  `REJECT` (the `reject_mode`, default `"zero"`, as in `build_reject`)
  and a fresh `BLOCKMAP` (blocklists shared while `compress`, the
  default, as in `build_blockmap`).
- `UDMF` maps get `ZNODES` alone: `ZDoom` builds the blockmap and the
  reject itself. Pass `reject_mode` explicitly and a `REJECT` lump is
  written into the block as well (which `ZDoom` reads); `BLOCKMAP` is
  never added to a `UDMF` block.
- `compress` also picks `ZNOD` vs raw `XNOD` for an extended build, as
  in `build_nodes`.

`nodes` carries exactly the `build_nodes` keys (`variant`, `segs`,
`subsectors`, `nodes`, `warnings` and the per-variant extras); `reject`
the `build_reject` keys (`bytes`, `sectors`, `rejected_pairs`, `mode`)
or `null`; `blockmap` the `build_blockmap` keys (`bytes`, `columns`,
`rows`, `origin`, `vanilla_safe`) or `null`. Everything happens in one
`Session::edit`: one undo entry covers the tree, the `GL_*` block, the
matrix and the grid together, and any refusal — a bad `variant` or
`reject_mode`, `classic`/`gl` on a `UDMF` map, a map with no vertices or
no linedef sides (`invalid_argument`), a record limit
(`limit_exceeded`) — rolls the whole step back with nothing partial
written. Like the single builders, a build is a product, not geometry:
`nodes_invalidated` reports `false`. Inspect with `get_nodes`,
`get_reject` and `get_blockmap`.

## Errors

An absent or empty lump is data, never an error. Malformed lumps fail
`format_error`: a `SEGS`/`SSECTORS`/`NODES` size that is not a whole
number of records, a child or subsector reference that leaves its table,
a truncated or trailing-magic extended lump, an inflated `ZNOD` over the
decompression cap, a `BLOCKMAP` offset outside the lump or a blocklist
without its `0xFFFF` terminator. The message names the lump; the fix is
`build_nodes` for the tree lumps, `build_blockmap` / `build_reject` for
those two, or `build_all_nodes` to rebuild everything in one step. An
unknown `map` is `not_found`; an unparsable `map`, `scope` or `detail`
is `invalid_argument`; `build_blockmap` on a map with no vertices is
`invalid_argument` and an oversized grid, linedef index or lump is
`limit_exceeded`; `build_nodes` and `build_all_nodes` on a map with no
linedef sides is `invalid_argument`, `classic` or `gl` on a `UDMF` map
is `invalid_argument` (build `"extended"`), a past-record limit (for
`gl` too: `GL_VERT` past `32767`, a `GL_SEGS` count past `65535`) is
`limit_exceeded` and a collapsed GL region or unplaceable seg is
`internal`; an unknown `variant` or `reject_mode` is
`invalid_argument`. A `build_all_nodes` refusal writes nothing partial.
