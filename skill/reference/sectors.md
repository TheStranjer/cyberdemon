# Sector tools

Creating, changing and removing the sectors of a map:
`create_sector_from_loop`, `carve_void`, `add_sector`, `edit_sector` and
`remove_sector`.
A sector is one contiguous 3-D region: its floor and ceiling heights, the
two flat textures, the light level, the effect number (`special`) and the
`tag` linedef actions match.

## Sectors and their sidedefs

A sector **does nothing until sidedefs reference it**: sides point at
sectors, and the region a sector describes is the collection of the sides
that name it. `add_sector` on its own only appends a record — attach it
with `add_sidedef`/`edit_sidedef` (and those to linedefs), or use
`create_sector_from_loop` to grow a whole room in one call.
`assemble_map include=["sectors"]` (`reference/maps.md`) lists
each sector's boundary `linedefs`, `sidedefs` and neighbouring `sectors`.

The sector count is part of the geometry the node lumps were built from:
adding or removing sectors reports `nodes_invalidated: true` and empties
the generated `SEGS`/`SSECTORS`/`NODES`/`REJECT`/`BLOCKMAP` (and drops the
`GL` run) — run `build_nodes` before playing the map in vanilla
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
  for a flat. A map that arrives with a blank flat field reads it back as
  `"-"`; every plane needs a flat, so `validate_map` reports such a sector
  as `missing_flat` (see `reference/validate.md`) and `edit_sector` fixes
  it.
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

## `create_sector_from_loop`

`{map?, points? | vertices?, floor_height? = 0, ceiling_height? = 128,
floor_flat? = "FLOOR4_8", ceiling_flat? = "CEIL3_5", light? = 160,
special? = 0, tag? = 0, wall_texture? = "STARTAN3", upper_texture?,
lower_texture?, outer_sector? = "auto", keep_interior? = false}` →
`{sector, vertices_created, linedefs_created, sidedefs_created,
linedef_map, removed_linedefs, empty_sectors, notes, map,
nodes_invalidated: true, copied_to_working}`

The draw-a-room tool: one call turns a closed corner polygon into a new
sector with all its boundary walls, in one undoable step. Every drawing
goes through the shared geometry engine, so touched walls are split at
T-junctions and crossings, painted-over walls vanish, and no two linedefs
are ever left crossing or overlapping.

- **The polygon, exactly one form** (both or neither is
  `invalid_argument`): `points: [{x, y}, ...]` — at least 3 corners,
  implicitly closed; a vertex already at exactly a corner's coordinates is
  reused, the rest are created; or `vertices: [i, ...]` — at least 3
  existing vertex indices. A repeated corner or the closing repeat is
  dropped for you.
- **Degenerate polygons are `invalid_argument`**: zero area (collinear
  corners) or a self-intersecting ring. An unknown vertex or
  `outer_sector` index is `not_found`.
- **Sector and textures**: the sector fields are `add_sector`'s, with its
  defaults and format rules. `wall_texture` dresses the `middle` of new
  one-sided walls; `upper_texture`/`lower_texture` (each defaulting to
  `wall_texture`) dress a new two-sided face where the neighbour's
  ceiling (`upper`) or floor (`lower`) differs. On binary maps coordinates
  must fit `i16` and names 8 characters.
- **Existing walls are split, never duplicated**: a wall the polygon
  touches mid-span is cut; the piece along the boundary turns two-sided
  against the new sector (blocking cleared, and a one-sided wall opening
  moves its `middle` onto its free `upper`/`lower` faces), keeping its
  texture, `special`, `tag` and `args` on every piece with `x_offset`
  shifted to hold the texture in place. Every front side faces into the
  room (`Doom` puts the front on the right of `v1` to `v2`).
- **`outer_sector`** decides what brand-new edges face outside:
  `"auto"` (the default) opens each edge onto the sector found just
  outside it, so a room drawn inside another becomes a platform with that
  room across its walls while edges along the void stay one-sided; an
  existing sector index turns every new edge two-sided against it;
  `"none"` keeps every new wall a blocking single line.
- **`keep_interior`** (default `false`) leaves existing sectors that sit
  strictly inside the polygon (a pillar, an inner room) untouched: only
  the area of the sectors the boundary touches is repainted. With it off,
  everything inside is overwritten and any sector buried this way is
  reported in `empty_sectors`/`notes` (kept, not deleted).

**Indices move.** The whole map's linedefs, sidedefs and vertices are
rebuilt from the arrangement, so their indices can change; sector indices
are stable. `linedef_map` maps every old linedef index onto the new index
of its first surviving piece (`null` when it vanished), `removed_linedefs`
lists the old indices that died, `vertices_created`/`linedefs_created`/
`sidedefs_created` report the brand-new rows and `empty_sectors` the
sectors left without a boundary. Look things up by index with
`assemble_map` after the call, not before.

Worked example — an adjacent room sharing room 0's north wall (the
two-room fixture):

```json
call create_sector_from_loop {"points": [{"x": 0, "y": 256}, {"x": 256, "y": 256},
                                         {"x": 256, "y": 512}, {"x": 0, "y": 512}]}
-> {"sector": 2, "vertices_created": [6, 7], "linedefs_created": [2, 4, 7],
    "sidedefs_created": [2, 4, 5, 9], "linedef_map": [0, 3, 5, 1, 8, 9, 6],
    "removed_linedefs": [], "empty_sectors": [], "notes": [],
    "map": "MAP01", "nodes_invalidated": true, "copied_to_working": true}
```

Sector 2 sits above the hall. The old north wall (linedef 1) became new
linedef 3 (`linedef_map[1] == 3`), now two-sided with a fresh back side
facing the room; the hall keeps index 0 and `assemble_map` shows
`neighbors: [0]` on sector 2. The two free corners are vertices 6 and 7.

## `carve_void`

`{map?, points? | vertices?}` → `{sector: null, vertices_created,
linedefs_created, sidedefs_created, linedef_map, removed_linedefs,
empty_sectors, removed_sectors_empty, notes, map, nodes_invalidated:
true, copied_to_working}`

Carves a closed polygon of **solid rock** out of a map in one undoable
step: the area holds no sector, so surrounding sectors gain one-sided
walls along it and everything the polygon covers (linedefs, sector area,
sidedefs) vanishes from it. Walls it touches mid-span are split. Use it
to punch a pillar into a room, or to cut one room away from another.

- **The polygon** follows `create_sector_from_loop`: exactly one of
  `points` (at least 3) or `vertices`; degenerate polygons are
  `invalid_argument` and an unknown vertex is `not_found`.
- **Textures**: each new wall wears the facing sector's own `middle`
  texture from its nearest original wall; a sector that has none gets the
  session theme's `Wall` texture (`STARTAN3` when the theme names none).
- **`removed_sectors_empty`** (same list as `empty_sectors`) reports the
  sectors the carve painted out of existence; they stay in the map —
  clear them with `remove_sector cascade: true` if you meant it.
- The same index-mapping caveat applies: read the new indices from
  `linedef_map` and `assemble_map`.

```json
call carve_void {"points": [{"x": 96, "y": 96}, {"x": 160, "y": 96},
                            {"x": 160, "y": 160}, {"x": 96, "y": 160}]}
-> {"sector": null, "linedefs_created": [3, 4, 5, 6],
    "removed_linedefs": [], "removed_sectors_empty": [],
    "map": "MAP01", "nodes_invalidated": true, "copied_to_working": true}
```

A pillar of rock in the middle of the hall, its four walls each wearing
the hall's `WALL1`.

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
