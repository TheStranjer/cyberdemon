# Sidedef tools

Creating, changing and removing the sidedefs of a map: `add_sidedef`,
`edit_sidedef` and `remove_sidedef`. A sidedef is one side of a linedef —
its upper/middle/lower textures, its texture offsets and the sector it
opens into. A linedef has at most two: the **front** (the `Doom` right
side) and the optional **back** (the left side; a line with both is
two-sided).

## Sidedefs and their linedefs

`assemble_map include=["sidedefs"]` (`reference/maps.md`) lists a map's
sidedefs with the first linedef using each one (`linedef`/`side`, `null`
for orphans); the `linedefs` rows carry their `front`/`back` sides fully
resolved. A sidedef is **not visible in the game until a linedef uses
it**: `add_sidedef` on its own only appends a record (attach it with
`add_linedef` or point a side at it with `edit_linedef`,
`reference/linedefs.md`).

The sidedef block counts as map geometry for the node lumps: adding or
removing a sidedef, or changing a sidedef's `sector`, reports
`nodes_invalidated: true` and empties the generated `SEGS`/`SSECTORS`/
`NODES`/`REJECT`/`BLOCKMAP` (and drops the `GL` run) — run
`build_nodes` before playing the map in vanilla `Doom`.
Texture and offset edits are property edits: the nodes stay trusted
(`nodes_invalidated: false`). Every call is one undoable step, and
editing a layer-owned map copies the block in first
(`copied_to_working: true`) — see `reference/maps.md` and
`reference/session.md`.

## Format rules

- Texture names (`upper`, `middle`, `lower`) are uppercased and
  validated: `-` means no texture, an empty name, a name over the
  format's cap (8 characters binary, 64 `UDMF`) or characters outside
  printable `ASCII` fail `invalid_argument`.
- `x_offset`/`y_offset` are texture shifts in map units. The binary
  formats store them as `i16`: values outside `-32768..=32767` fail
  `invalid_argument`; `UDMF` maps store full integers.
- `sector` must be an existing sector index (`not_found` otherwise).

## `add_sidedef`

`{map?, sector, upper? = "-", middle? = "-", lower? = "-", x_offset? = 0,
y_offset? = 0}` → `{index, sidedef: {sector, upper, middle, lower,
x_offset, y_offset}, map, nodes_invalidated: true, copied_to_working}`

Appends one sidedef and reports its index with the stored fields. The
sector must exist. Because the sidedef list changed, the result reports
`nodes_invalidated: true`.

```json
call add_sidedef {"map": "MAP01", "sector": 0, "middle": "brick01",
                  "x_offset": 8}
-> {"index": 8, "sidedef": {"sector": 0, "upper": "-", "middle":
    "BRICK01", "lower": "-", "x_offset": 8, "y_offset": 0},
    "map": "MAP01", "nodes_invalidated": true, "copied_to_working": true}
```

## `edit_sidedef`

`{map?, index, upper?, middle?, lower?, x_offset?, x_offset_delta?,
y_offset?, y_offset_delta?, sector?}` → `{sidedefs: [{index, sector,
upper, middle, lower, x_offset, y_offset}], map, nodes_invalidated,
copied_to_working}`

`index` is one integer or a list; every change parameter applies to all
selected sidedefs and only the given fields change. `x_offset`/`y_offset`
set absolutely, `x_offset_delta`/`y_offset_delta` shift each selected
sidedef from its own value; an absolute and its delta together (or no
change parameter at all) are `invalid_argument`. `sector` re-points the
side at another existing sector. Texture and offset changes keep the
nodes (`false`); changing `sector` invalidates them (`true`). A missing
index or sector is `not_found` and nothing changes.

```json
call edit_sidedef {"index": [0, 1], "x_offset_delta": 8}
call edit_sidedef {"index": 2, "middle": "DOOR1", "sector": 1}
```

## `remove_sidedef`

`{map?, index, cascade? = false}` → `{removed: {sidedefs, linedefs,
vertices, sectors, things, flipped_linedefs, one_sided_linedefs}, map,
nodes_invalidated: true, copied_to_working}`

Drops the selected sidedefs (one index or a list); survivors renumber
down. `removed` lists the original indices per kind (sorted) and the
**new** indices of linedefs the removal flipped (`flipped_linedefs`) or
closed into one side (`one_sided_linedefs`).

- Without `cascade`, a sidedef a linedef still uses is refused with
  `conflict` naming the referencing linedefs; nothing changes. An
  unreferenced sidedef (e.g. one just added) removes fine.
- With `cascade: true` the removal takes its users' geometry with it: a
  linedef that loses its front side is **flipped** (`v1`/`v2` swap, the
  back becomes the front), one that loses its back becomes **one-sided**
  (two-sided bit cleared, blocking bit set, and an otherwise invisible
  `lower`/`upper` texture of the surviving side moved to `middle`), and
  one that loses both sides is removed with them; vertices and sidedefs
  orphaned by those linedef removals are pruned too.

```json
call remove_sidedef {"index": 2}
-> error conflict "sidedef 2 is used by linedef(s) 2; pass cascade: true
   to remove the referencing linedefs with them"
call remove_sidedef {"index": 2, "cascade": true}
-> {"removed": {"vertices": [], "linedefs": [], "sidedefs": [2],
    "sectors": [], "things": [], "flipped_linedefs": [2],
    "one_sided_linedefs": [2]}, "map": "MAP01",
    "nodes_invalidated": true, "copied_to_working": true}
```

## After editing

Check the work with `assemble_map include=["sidedefs", "linedefs"]` on
the `"working"` scope (sides with their sectors and back-references,
linedefs with their resolved `front`/`back`), or `render_topdown`
(`reference/render.md`) to see the textures. After a geometry change
(`nodes_invalidated: true`) rebuild the nodes before a vanilla play test.
