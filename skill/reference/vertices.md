# Vertex tools

Creating, moving and removing the vertices of a map: `add_vertex`,
`move_vertex` and `remove_vertex`. A vertex is one point of the map's
geometry — a linedef's two endpoints are vertices, and every wall, floor
and ceiling shape is built from the lines between them. Vertices carry
nothing but coordinates.

## Vertices and their linedefs

A vertex is **only in play when a linedef ends at it**: `add_vertex` on
its own appends a point that no line uses (an `IWAD`-style orphan, which
engines ignore and `build_nodes` skips). Attach it by adding a linedef
that names it, or bring an existing line's endpoint onto it with
`move_vertex`. `assemble_map include=["vertices"]` (`reference/maps.md`)
lists a map's points in index order with the `linedefs` that use each one,
so orphans show up as `"linedefs": []`.

An edit changes geometry — reports `nodes_invalidated: true` and empties
the generated `SEGS`/`SSECTORS`/`NODES`/`REJECT`/`BLOCKMAP` (and drops the
map's `GL` run) — when it touches a vertex a linedef ends at, a linedef, a
sidedef's sector or the sector count (so an `add_sector` is one); run
`build_nodes` before playing such a map in vanilla `Doom`. Such an
invalidating edit also sheds the split points an earlier `build_nodes`
appended (`reference/nodes.md`) — and **only** those: the shed reaches
exactly the trailing vertices that run's own `SEGS` records named, so a
point you appended yourself and no line ends at survives every edit,
whatever the nodes do. Hand-building a map — append the corners, add the
sector, then run lines between the points — never loses a vertex. The two
cases that change no buildable geometry: `add_vertex` **reusing** a point
that already exists (nothing is written at all: `nodes_invalidated: false`,
`copied_to_working: false`, and **no undo entry**), and editing vertices
that sit unreferenced past the last one a line ends at (e.g. a fresh append
or a leftover split point), which keeps the nodes trusted.

Every call is one undoable step (a list of vertices moves as one step),
and editing a layer-owned map copies the block into the working archive
first (`copied_to_working: true`). The shared rules — the `map` default,
index parameters, the `{"map", "nodes_invalidated", "copied_to_working"}`
fragment — are `reference/maps.md`'s **Editing maps** section.

## Format rules

- The binary formats store each coordinate as `i16`: `x`/`y` outside
  `-32768..=32767` fail `invalid_argument` on `Doom` and `Hexen` maps,
  both when appending and as the result of a move. `UDMF` maps store full
  integers and have no such cap.
- A move that would put **both endpoints of one linedef on the same
  coordinates** is refused with `conflict` naming that linedef: a
  zero-length line is not buildable geometry. Two vertices may still share
  a point as long as no single line spans them.
- Removing a vertex a linedef uses is refused with `conflict` unless
  `cascade` is set (below).

## `add_vertex`

`{map?, x, y, reuse_existing? = true}` → `{index, x, y, reused, map,
nodes_invalidated, copied_to_working}`

Appends one point and reports its index. With `reuse_existing` (the
default) a vertex already sitting at exactly `x`/`y` is returned instead:
`reused: true`, the map untouched, no undo entry. Pass
`reuse_existing: false` to store a duplicate point anyway. Out-of-range
coordinates on a binary map are `invalid_argument`; an unknown map is
`not_found`.

```json
call add_vertex {"map": "MAP01", "x": 128, "y": 128}
-> {"index": 6, "x": 128, "y": 128, "reused": false, "map": "MAP01",
    "nodes_invalidated": false, "copied_to_working": true}
call add_vertex {"x": 128, "y": 128}
-> {"index": 6, "x": 128, "y": 128, "reused": true, "map": "MAP01",
    "nodes_invalidated": false, "copied_to_working": false}
```

## `move_vertex`

`{map?, index, x?, y?, dx?, dy?}` → `{moved: [{index, x, y}], map,
nodes_invalidated, copied_to_working}`

`index` is one integer or a list (sorted, deduplicated). Say where in
exactly one of two ways: **absolute** `x` and/or `y`, which place a
*single* vertex, or **relative** `dx` and/or `dy`, which shift every
selected vertex by the same amount and are the way to move a list (e.g. a
whole room's corners at once). An axis you leave out stays where it is.
Mixing an absolute parameter with a delta, passing none of the four, or
passing absolute `x`/`y` with a list are all `invalid_argument`; so is a
result outside the binary `i16` range. A move that would collapse a
linedef to a point is `conflict` and nothing changes; an index the map
does not hold is `not_found`.

```json
call move_vertex {"index": 4, "x": 300, "y": 256}
call move_vertex {"index": [0, 1, 2], "dx": -8, "dy": 16}
-> {"moved": [{"index": 0, "x": -8, "y": 16},
              {"index": 1, "x": 248, "y": 16},
              {"index": 2, "x": 504, "y": 16}],
    "map": "MAP01", "nodes_invalidated": true,
    "copied_to_working": false}
```

## `remove_vertex`

`{map?, index, cascade? = false, prune_sidedefs? = true,
prune_vertices? = true}` → `{removed: {vertices, linedefs, sidedefs,
sectors, things, flipped_linedefs, one_sided_linedefs}, map,
nodes_invalidated, copied_to_working}`

Drops the selected points (one index or a list); the survivors renumber
down and every surviving linedef's `v1`/`v2` follow. `removed` lists the
**original** indices per kind (sorted) and the **new** indices of linedefs
the removal flipped (`flipped_linedefs`) or closed into one side
(`one_sided_linedefs`).

- Without `cascade`, a vertex any linedef still ends at is refused with
  `conflict` naming those linedefs; nothing changes. An unreferenced
  vertex (one just added, or an orphan an earlier removal left) removes
  fine.
- With `cascade: true` the removal takes the geometry with it: every
  linedef using a removed vertex goes; a linedef that loses its front side
  is **flipped** (`v1`/`v2` swap, the back becomes the front), one that
  loses its back becomes **one-sided** (two-sided bit cleared, blocking
  bit set, an otherwise invisible `lower`/`upper` texture of the surviving
  side moved to `middle`), and one that loses both sides goes with them.
  `prune_sidedefs` (default `true`) also drops sidedefs no surviving
  linedef references and `prune_vertices` (default `true`) also drops
  vertices no surviving linedef references, so a cascade leaves no orphans
  unless you switch a pruning off (and then they stay: the geometry change
  invalidates the nodes, which sheds only the split points that node run
  named).

```json
call remove_vertex {"index": 4}
-> error conflict "vertex 4 is used by linedef(s) 1, 2, 4; pass
   cascade: true to remove the referencing linedefs with them"
call remove_vertex {"index": 4, "cascade": true}
-> {"removed": {"vertices": [4], "linedefs": [1, 2, 4],
    "sidedefs": [1, 2, 3, 5], "sectors": [], "things": [],
    "flipped_linedefs": [], "one_sided_linedefs": []}, "map": "MAP01",
    "nodes_invalidated": true, "copied_to_working": true}
```

## After editing

Check the work with `assemble_map include=["vertices", "linedefs"]` on the
`"working"` scope (points with the lines that use them, lines with their
resolved `v1`/`v2` coordinates and lengths), or `render_topdown` with
`show_vertices` and `highlight` (`reference/render.md`) to look at it.
After a geometry change (`nodes_invalidated: true`) rebuild the nodes
before a vanilla play test.
