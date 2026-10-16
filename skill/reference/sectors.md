# Sector tools

Creating, changing and removing the sectors of a map: `add_sector`,
`edit_sector` and `remove_sector`. A sector is one contiguous 3-D region:
its floor and ceiling heights, the two flat textures, the light level, the
effect number (`special`) and the `tag` linedef actions match.

## Sectors and their sidedefs

A sector **does nothing until sidedefs reference it**: sides point at
sectors, and the region a sector describes is the collection of the sides
that name it. `add_sector` on its own only appends a record — attach it
with `add_sidedef`/`edit_sidedef` (and those to linedefs), or use
`create_sector_from_loop` (not yet implemented) to grow a whole room in
one call. `assemble_map include=["sectors"]` (`reference/maps.md`) lists
each sector's boundary `linedefs`, `sidedefs` and neighbouring `sectors`.

The sector count is part of the geometry the node lumps were built from:
adding or removing sectors reports `nodes_invalidated: true` and empties
the generated `SEGS`/`SSECTORS`/`NODES`/`REJECT`/`BLOCKMAP` (and drops the
`GL` run) — run `build_all_nodes` before playing the map in vanilla
`Doom`. Heights, flats, light, special and tag are sector *properties*:
editing them keeps the nodes trusted (`nodes_invalidated: false`). Every
call is one undoable step, and editing a layer-owned map copies the block
in first (`copied_to_working: true`) — see `reference/maps.md` and
`reference/session.md`.

## Format rules

- `light` must sit in the vanilla `0..=255` in every format; outside it
  fails `invalid_argument`.
- Flats (`floor_flat`, `ceiling_flat`) are uppercased and validated like
  texture names (non-empty, printable `ASCII`, 8 characters on binary
  maps, 64 on `UDMF`), but `"-"` — which names *no texture* — is refused
  for a flat.
- The resulting sector needs its ceiling at or above its floor; equal
  heights are allowed (the closed shell of a raised door), a ceiling
  below the floor fails `invalid_argument`.
- Binary maps store `floor_height`/`ceiling_height` as `i16`
  (`-32768..=32767`) and `special`/`tag` as `u16` (`0..=65535`);
  out-of-range values fail `invalid_argument`. `UDMF` stores full
  integers and has no such caps.

## `add_sector`

`{map?, floor_height? = 0, ceiling_height? = 128, floor_flat? =
"FLOOR4_8", ceiling_flat? = "CEIL3_5", light? = 160, special? = 0,
tag? = 0}` → `{index, sector: {floor_height, ceiling_height, floor_flat,
ceiling_flat, light, special, tag}, map, nodes_invalidated: true,
copied_to_working}`

Appends one sector with the given fields (each optional, the defaults
above) and reports its index and stored values. Because the sector list
grew, the result reports `nodes_invalidated: true`.

```json
call add_sector {"map": "MAP01"}
-> {"index": 2, "sector": {"floor_height": 0, "ceiling_height": 128,
    "floor_flat": "FLOOR4_8", "ceiling_flat": "CEIL3_5", "light": 160,
    "special": 0, "tag": 0}, "map": "MAP01", "nodes_invalidated": true,
    "copied_to_working": true}
```

## `edit_sector`

`{map?, index, floor_height?, ceiling_height?, floor_delta?,
ceiling_delta?, floor_flat?, ceiling_flat?, light?, special?, tag?}` →
`{sectors: [{index, floor_height, ceiling_height, floor_flat,
ceiling_flat, light, special, tag}], map, nodes_invalidated: false,
copied_to_working}`

`index` is one integer or a list; every change parameter applies to all
selected sectors and only the given fields change. `floor_height`/
`ceiling_height` set absolutely, `floor_delta`/`ceiling_delta` shift each
selected sector from its own value; an absolute and its delta together
(or no change parameter at all) are `invalid_argument`. The format rules
above apply to each resulting sector. A missing index is `not_found` and
nothing changes. Property edits keep the nodes (`false`).

```json
call edit_sector {"index": [0, 1], "floor_delta": 16}
call edit_sector {"index": 2, "ceiling_height": 0, "floor_flat": "RROCK16"}
```

## `remove_sector`

`{map?, index, cascade? = false, prune_sidedefs? = true, prune_vertices?
= true}` → `{removed: {sectors, sidedefs, linedefs, vertices, things,
flipped_linedefs, one_sided_linedefs}, map, nodes_invalidated: true,
copied_to_working}`

Drops the selected sectors (one index or a list); survivors renumber
down and every surviving sidedef's sector reference follows. `removed`
lists the original indices per kind (sorted) and the **new** indices of
linedefs the removal flipped (`flipped_linedefs`) or closed into one side
(`one_sided_linedefs`).

- Without `cascade`, a sector a sidedef still uses is refused with
  `conflict` naming the referencing sidedefs; nothing changes. An
  unreferenced sector (e.g. one just added) removes fine.
- With `cascade: true` the removal takes the room with it: every sidedef
  of a removed sector goes; a linedef that loses its front side is
  **flipped** (the back becomes the front), one that loses its back
  becomes **one-sided** (two-sided bit cleared, blocking bit set, an
  otherwise invisible `lower`/`upper` texture of the surviving side moved
  to `middle`), and one that loses both sides is removed with them.
  `prune_sidedefs` (default `true`) also drops sidedefs no surviving
  linedef references; `prune_vertices` (default `true`) also drops
  orphaned vertices. Turning a pruning off leaves those orphans in place.

```json
call remove_sector {"index": 0}
-> error conflict "sector 0 is used by sidedef(s) 0, 1, 2, 4; pass
   cascade: true to remove the referencing sidedefs with them"
call remove_sector {"index": 1, "cascade": true}
-> {"removed": {"sectors": [1], "sidedefs": [3, 5, 6, 7], "linedefs":
    [4, 5, 6], "vertices": [2, 3], "things": [], "flipped_linedefs": [],
    "one_sided_linedefs": [2]}, "map": "MAP01", "nodes_invalidated":
    true, "copied_to_working": true}
```

## After editing

Check the work with `assemble_map include=["sectors", "sidedefs"]` on the
`"working"` scope (sectors with their boundary lines and sides, sides
with the sector they face), or `render_topdown` (`reference/render.md`)
to see the heights. After a geometry change (`nodes_invalidated: true`)
rebuild the nodes before a vanilla play test.
