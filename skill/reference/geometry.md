# Geometry tools

Editing a **mixed selection** of map elements at once. Today that is
`delete_geometry`, one call that removes any combination of vertices,
linedefs, sidedefs, sectors and things with full reference cleanup; the
whole-selection transform tools will join this file as they arrive. For
single-kind edits with opt-in cascades see `remove_vertex`
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
