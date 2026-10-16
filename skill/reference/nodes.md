# Nodes tools

The node-builder output of a map, for reading: `get_nodes`, `get_reject`
and `get_blockmap`. The tools that *build* these lumps join this group
later; today it measures what a map already carries.

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
parse into the same shape. The glBSP (`GL_*`) lumps and the `XGLN`/
`ZGLN`/`XGL2`/`ZGL2`/`XGL3`/`ZGL3` magics are **reported, never parsed**:
you get their presence, magic and sizes.

## The `map`, `scope` and `detail` parameters

All three tools take `map` and `scope` exactly as the maps tools do
(`reference/maps.md`: the active map or the merged view's only map by
default; `"merged"`, `"working"` or `"layer:N"`). `detail` is
`"summary"` (the default: numbers only) or `"full"`: `full` publishes the
parsed structure as a JSON asset and attaches
`"asset": {url, filename, mime, size}` to the result. A missing or empty
lump never publishes anything.

## `get_nodes`

`{map?, scope?, detail?}` → `{map, format, bytes, segs, subsectors,
nodes, extra_vertices, root, depth, gl {present, magic, lumps}}`

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
  ...).

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

## Errors

An absent or empty lump is data, never an error. Malformed lumps fail
`format_error`: a `SEGS`/`SSECTORS`/`NODES` size that is not a whole
number of records, a child or subsector reference that leaves its table,
a truncated or trailing-magic extended lump, an inflated `ZNOD` over the
decompression cap, a `BLOCKMAP` offset outside the lump or a blocklist
without its `0xFFFF` terminator. The message names the lump; the fix is
to rebuild the nodes (a later ticket's tools) or read a different scope.
An unknown `map` is `not_found`; an unparsable `map`, `scope` or `detail`
is `invalid_argument`.
