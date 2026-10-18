# Geometry tools

Editing a **mixed selection** of map elements at once. Today that is
`delete_geometry`, one call that removes any combination of vertices,
linedefs, sidedefs, sectors and things with full reference cleanup, and
`transform_selection`, one call that moves a mixed selection of vertices
and things through one scale/rotate/translate. For single-kind edits
with opt-in cascades see `remove_vertex`
(`reference/vertices.md`), `remove_linedef` (`reference/linedefs.md`),
`remove_sidedef` (`reference/sidedefs.md`) and `remove_sector`
(`reference/sectors.md`).

Every call is one undoable step; editing a layer-owned map copies the
block into the working archive first (`copied_to_working: true`). The
shared rules — the `map` default, the element counts to pick indices
from (`get_map_summary`, `assemble_map`) and the
`{"map", "nodes_invalidated", "copied_to_working"}` result fragment — are
`reference/maps.md`'s **Editing maps** section. Deleting anything but
things alone is a geometry change: the node lumps are invalidated, so
run `build_nodes` before playing the map in vanilla `Doom`.

## `delete_geometry`

`{map?, vertices?, linedefs?, sidedefs?, sectors?, things?,
prune_sidedefs? = true, prune_vertices? = true, dry_run? = false}` →
`{removed: {vertices, linedefs, sidedefs, sectors, things,
flipped_linedefs, one_sided_linedefs}, dry_run, map, nodes_invalidated,
copied_to_working}`

`vertices`, `linedefs`, `sidedefs`, `sectors` and `things` are arrays of
0-based indices into that map's blocks (duplicates collapse). At least
one array must hold an index; an all-empty call is `invalid_argument`
and an index the map does not hold is `not_found` naming the kind and
index — in both cases nothing changes. `removed` lists the **original**
indices per kind (sorted) and the **new** indices of linedefs the
removal flipped (`flipped_linedefs`) or closed into one side
(`one_sided_linedefs`).

Unlike the per-kind tools, `delete_geometry` **always cascades**; there
is no `cascade` switch and a referenced element is never refused. The
cleanup rules, applied repeatedly until the selection is self-consistent:

1. Every **sidedef of a removed sector** goes with it.
2. Every **linedef touching a removed vertex** goes with it.
3. A linedef that loses its **front** side is **flipped**: `v1`/`v2`
   swap and the back becomes the front.
4. A linedef that loses its **back** side becomes **one-sided**: the
   two-sided bit is cleared, the blocking bit set, and an otherwise
   invisible `lower` (preferred) or `upper` texture of the surviving
   side is moved to `middle` so the wall shows.
5. A linedef that loses **both** its sides is removed with them.
6. `prune_sidedefs` (default `true`): **sidedefs orphaned** by the
   linedef removals — referenced by no surviving linedef — go too.
7. `prune_vertices` (default `true`): **vertices orphaned** by the
   linedef removals — used by no surviving linedef — go too.

The cascade never adds sectors or things: those go only when you name
them. Switching a pruning off keeps those orphans in the block (an
engine ignores unreferenced sides and trailing unreferenced vertices;
note that any node-invalidating edit — a later `build_nodes` included —
strips vertices no linedef references past the last one that does).

`dry_run: true` computes the entire cascade on a copy and returns the
same `removed` report without editing anything: no undo entry,
`copied_to_working: false`, `nodes_invalidated: false`. Use it to see
what a deletion would take down before committing it.

```json
call delete_geometry {"sectors": [1], "dry_run": true}
-> {"removed": {"vertices": [2, 3], "linedefs": [4, 5, 6],
     "sidedefs": [3, 5, 6, 7], "sectors": [1], "things": [],
     "flipped_linedefs": [], "one_sided_linedefs": [2]},
    "dry_run": true, "map": "MAP01", "nodes_invalidated": false,
    "copied_to_working": false}
call delete_geometry {"things": [1], "vertices": [5]}
-> {"removed": {"vertices": [5], "linedefs": [0, 1], "sidedefs": [0, 1],
     "sectors": [], "things": [1], "flipped_linedefs": [],
     "one_sided_linedefs": []}, "dry_run": false, "map": "MAP01",
    "nodes_invalidated": true, "copied_to_working": true}
```

## After deleting

Check the work with `get_map_summary` (the counts should drop by exactly
the `removed` lists) or `assemble_map` (`reference/maps.md`) on the
`"working"` scope, and `render_topdown` (`reference/render.md`) to look
at the hole; `undo` takes the whole call back as one step. After a
geometry change (`nodes_invalidated: true`) rebuild the nodes before a
vanilla play test.

## `transform_selection`

`{map?, selection: {vertices?, linedefs?, sectors?, things?, all?,
include_things_in_sectors? = true}, scale?: {num, den}, rotate?:
{degrees | millidegrees}, translate?: {dx, dy}, origin?: {x, y}}` →
`{origin, vertices_selected, things_selected, vertices_moved,
things_moved, coincident_vertices, map, nodes_invalidated,
copied_to_working}`

`selection` picks what moves, as 0-based index arrays into the map's
blocks plus two switches. **Expansion** turns it into the vertices and
things that actually move: a listed linedef drags in its two endpoints;
a listed sector drags in every vertex of its boundary linedefs and (when
`include_things_in_sectors`, the default) the things standing in it —
containment is judged *before* anything moves; `all: true` selects every
vertex and thing. The expanded selection must hold at least one vertex
or thing (`invalid_argument` otherwise) and every index must exist
(`not_found` naming kind and index); nothing changes in either case.

Give **at least one operation** (`invalid_argument` otherwise); all
given ones run in this one call, in the order **scale, rotate,
translate**, in integer math rounded to nearest:
`p' = origin + rotate(scale(p - origin))` then `+ translate`.

- `scale`: `{num, den}`, an exact positive fraction (both above zero):
  `3/2` grows to 150%, `1/2` shrinks to 50%. Positions only.
- `rotate`: `{degrees}` **or** `{millidegrees}` (exactly one of the
  two), counter-clockwise. Selected things' `angle` additionally gains
  the rotation in whole degrees, normalised into `0..=359`
  (`millidegrees` round to the nearest degree for facings).
- `translate`: `{dx, dy}` in map units.
- `origin`: `{x, y}`, the pivot of `scale` and `rotate`; the default is
  the centre of the selection's bounding box, rounded to nearest
  (reported back as `origin`). A translation ignores it.

Validation refuses the **whole call** (nothing moves):

- A coordinate the map's format cannot store is `invalid_argument`: the
  binary `Doom`/`Hexen` formats hold `i16`, so a big scale or shift must
  shrink or move `origin` — or `convert_map_format` the map to `UDMF`,
  which takes the larger range (`reference/convert.md`).
- A linedef that would become **zero-length** is `conflict` naming it
  (a `1/1000` scale stacks everything onto the origin — lower it or
  leave a vertex out of the selection).
- Vertices landing on an **unselected** vertex are allowed: they come
  back as `coincident_vertices` pairs `[moved, unmoved]` and are
  **not** merged; check with `validate_map` afterwards if you wanted a
  weld.

`vertices_selected`/`things_selected` list the expanded selection
(ascending); `*_moved` count them. Moving any vertex invalidates the
node lumps (`nodes_invalidated: true` — run `build_nodes` before a
vanilla play test); a things-only move keeps the nodes.

```json
call transform_selection {"selection": {"sectors": [1]},
     "translate": {"dx": 0, "dy": 64}}
-> {"origin": {"x": 384, "y": 128}, "vertices_selected": [1, 2, 3, 4],
    "things_selected": [1], "vertices_moved": 4, "things_moved": 1,
    "coincident_vertices": [], "map": "MAP01",
    "nodes_invalidated": true, "copied_to_working": true}
call transform_selection {"selection": {"all": true},
     "rotate": {"degrees": 180}, "origin": {"x": 256, "y": 128}}
-> every vertex and thing moves; run the same call again and the map
   is exactly what it was (integer math, no drift at 180 degrees)
call transform_selection {"selection": {"things": [0]},
     "rotate": {"millidegrees": 45500}}
-> thing 0 spins to facing 46 (0 + 45.5 rounds to 46); a lone thing is
   its own default origin, so it keeps its position;
   nodes_invalidated: false
```

## After transforming

Look at the result with `render_topdown` (`reference/render.md`) and
read coordinates back through `assemble_map` on the `"working"` scope
(`reference/maps.md`); `undo` takes the whole transform back as one
step.
