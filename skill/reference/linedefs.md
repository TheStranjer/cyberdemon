# Linedef tools

Creating, changing and removing the linedefs of a map: `add_linedef`,
`edit_linedef` and `remove_linedef`. A linedef is the line between two
vertices with at most one [`Sidedef`](sidedefs.md) per side — the wall,
step or trigger the engine draws and walks along it.

## Orientation: the front faces right

**Walk from `v1` to `v2`: the front sidedef is on your right, the back
sidedef on your left.** A line with a back side is two-sided (both faces
exist); a one-sided line has only the front. `edit_linedef`'s `flip`
upholds the rule: it swaps `v1`/`v2` *and* `front`/`back`, so the line
turns around while both physical faces keep their sidedefs. Side
textures and offsets are edited from the linedef's point of view
(`front_side`/`back_side`), so after a flip the `front_side` of a call
lands on the sidedef that became the front.

Linedefs are pure references: `v1`/`v2` point into the vertices and
`front`/`back` into the sidedefs (see `reference/vertices.md` and
`reference/sidedefs.md`). Every reference must already exist —
`add_linedef` is the one call that creates them for you, and reports the
new indices in `created_vertices`/`created_sidedefs`. `assemble_map
include=["linedefs"]` (`reference/maps.md`) lists every line with its
ends and sides fully resolved.

Every call is one undoable step, editing a layer-owned map copies the
block in first (`copied_to_working: true`), and changing any linedef —
its endpoints, its sides, even its position in the list — is geometry:
`nodes_invalidated: true`, the generated lumps emptied and the `GL` run
dropped, so run `build_nodes` before playing the map in vanilla `Doom`.
Flags, specials, tags, args and side texture/offset edits are property
edits and keep the nodes. The shared rules — the `map` default, index
parameters, the result fragment — are `reference/maps.md`'s **Editing
maps** section.

## Format and family rules

- **Points.** `from`/`to` coordinates must fit `i16` (`-32768..=32767`)
  on binary maps; `UDMF` is unbounded.
- **Tag.** The `tag` field exists only in the `Doom` family (binary
  `Doom`, `UDMF` in the `doom`/`heretic`/`strife` namespace). A non-zero
  `tag` on a `Hexen`-family map is `invalid_argument` — `Hexen` specials
  take their parameters from `args` instead. Binary `Doom` stores
  `special` and `tag` as `u16`.
- **Args.** The five `args` exist only in the `Hexen` family; non-zero
  args on a `Doom`-family map are `invalid_argument`, and the binary
  `Hexen` record stores each one as `u8` (`0..=255`).
- **Flags.** `flags` parameters take the raw bitset or names from the
  map's own table (below). Binary maps store the bitset as `u16`.
- **Duplicates.** One linedef per vertex pair: `add_linedef` between two
  vertices that already share a line — in either direction — refuses
  `already_exists` naming it. `v1 == v2` (or `from` and `to` naming one
  point) is `invalid_argument`.

### Flag names

`Doom` family (`Doom` and `Hexen` share the first nine bits):

| Name | Bit | Name | Bit |
|------|-----|------|-----|
| `blocking` | 1 | `secret` | 32 |
| `blockmonsters` | 2 | `blocksound` | 64 |
| `twosided` | 4 | `dontdraw` | 128 |
| `dontpegtop` | 8 | `mapped` | 256 |
| `dontpegbottom` | 16 | `passuse` | 512 (`Doom` family only) |

`Hexen` family adds `repeatspecial` (512), `monsteractivate` (8192),
`blockplayers` (16384), `blockeverything` (32768) and the **activation
field** — the 3-bit trigger type at bits 10-12 named by `playercross`
(0), `playeruse` (1), `monstercross` (2), `impact` (3), `playerpush` (4)
and `missilecross` (5). An activation name writes the whole field (a
later one replaces an earlier one, never combines); `playercross` is the
all-clear value, so a `Hexen`-family line with `flags: 1` reads back as
`["blocking", "playercross"]`.

## `add_linedef`

`{map?, v1?, v2?, from?, to?, front?, front_side?, back?, back_side?,
flags?, special? = 0, tag? = 0, args?}` → `{index, linedef,
created_vertices, created_sidedefs, map, nodes_invalidated,
copied_to_working}`

Appends one line in a single undoable edit. Endpoints come in exactly
one form — `v1`/`v2` (existing vertices) or `from`/`to` points `{x, y}`,
each of which reuses the vertex already at exactly those coordinates and
creates one where none exists. The front side is required (`front` or
`front_side: {sector, upper?, middle?, lower?, x_offset?, y_offset?}` —
the create form behaves like `add_sidedef`); `back`/`back_side` is at
most one form and its presence makes the line two-sided. Absent `flags`
then default to `blocking` (1) one-sided, `twosided` (4) two-sided. A
wrong endpoint form, both forms of one side, no front, a degenerate pair
or out-of-range numbers are `invalid_argument`; a missing vertex,
sidedef or sector index is `not_found`; the duplicate is
`already_exists`. The refusal changes nothing at all.

```json
call add_linedef {"v1": 2, "v2": 4, "front": 6}
-> {"index": 7, "linedef": {"v1": 2, "v2": 4, "flags": 1,
    "flag_names": ["blocking"], "special": 0, "tag": 0,
    "args": [0, 0, 0, 0, 0], "front": 6, "back": null},
    "created_vertices": [], "created_sidedefs": [], "map": "MAP01",
    "nodes_invalidated": true, "copied_to_working": true}
call add_linedef {"from": {"x": 0, "y": 128}, "to": {"x": 256, "y": 128},
                  "front_side": {"sector": 1, "middle": "wall1"},
                  "back": 3}
-> {"index": 7, "linedef": {..., "flags": 4, "front": 8, "back": 3},
    "created_vertices": [6, 7], "created_sidedefs": [8], ...}
```

## `edit_linedef`

`{map?, index, flags?, set_flags?, clear_flags?, special?, tag?, args?,
v1?, v2?, front?, back?, flip?, front_side?, back_side?}` → `{linedefs:
[{index, v1, v2, flags, flag_names, special, tag, args, front, back}],
map, nodes_invalidated, copied_to_working}`

Changes one linedef or a list of them (`index` is one integer or an
array, sorted and deduplicated); every parameter applies to each
selected line.

- `flags` replaces the whole bitset; `set_flags`/`clear_flags` adjust the
  current bits (a bit named in both ends up set). `flags` together with
  either is `invalid_argument`.
- `special`, `tag`, `args`, `v1`/`v2` (existing vertices), `front`/`back`
  (existing sidedefs) replace their fields. `front: null` is refused —
  turn the line with `flip: true`, close it with `back: null`, or remove
  it. `back: null` drops the back reference.
- `flip: true` swaps the endpoints and the sides (the front stays on the
  right).
- Setting `back` also sets `twosided` and clears `blocking`; `back: null`
  also clears `twosided` and sets `blocking` — unless you pass
  `flags`/`set_flags`/`clear_flags`, which leave the bits to you.
- `front_side`/`back_side` `{upper?, middle?, lower?, x_offset?,
  y_offset?}` edit the referenced sidedefs' textures and offsets in
  place, from this linedef's point of view. Naming a side the line does
  not have is `invalid_argument`, as is `back: null` together with
  `back_side` fields.
- A call that changes nothing is `invalid_argument`; an endpoint change
  that leaves `v1 == v2` is `invalid_argument`; indices the map does not
  hold are `not_found`. The family and binary rules above apply to the
  results.

```json
call edit_linedef {"index": 0, "set_flags": ["mapped"]}
-> {"linedefs": [{"index": 0, "v1": 0, "v2": 5, "flags": 257,
    "flag_names": ["blocking", "mapped"], ...}], "map": "MAP01",
    "nodes_invalidated": false, "copied_to_working": true}
call edit_linedef {"index": 2, "flip": true}
call edit_linedef {"index": 2, "back": null}
call edit_linedef {"index": [4, 5, 6], "front_side": {"x_offset": 16}}
```

## `remove_linedef`

`{map?, index, prune_sidedefs? = true, prune_vertices? = true}` →
`{removed: {linedefs, sidedefs, vertices, sectors, things,
flipped_linedefs, one_sided_linedefs}, map, nodes_invalidated,
copied_to_working}`

Drops the selected lines (one index or a list); the survivors renumber
down and references follow. The removal always runs the cascade and the
pruning switches say how far: sidedefs no surviving linedef references
go too (`prune_sidedefs`, default `true`) and so do vertices no
surviving linedef ends at (`prune_vertices`, default `true`) — switch a
pruning off to leave the orphans in the block (trailing orphan vertices
still go with the invalidated nodes). Sectors are never touched.
`removed` lists the **original** indices per kind (sorted);
`flipped_linedefs`/`one_sided_linedefs` stay empty for linedef removals.

```json
call remove_linedef {"index": [5, 6]}
-> {"removed": {"linedefs": [5, 6], "sidedefs": [6, 7],
    "vertices": [2], "sectors": [], "things": [], "flipped_linedefs": [],
    "one_sided_linedefs": []}, "map": "MAP01",
    "nodes_invalidated": true, "copied_to_working": true}
```

## After editing

Check the work with `assemble_map include=["linedefs"]` on the
`"working"` scope (ends and sides resolved, coordinates and lengths
shown), or `render_topdown` / `render_view` (`reference/render.md`) to
look at it. A `validate_map` pass (`reference/validate.md`) catches the
dangling sides and zero-length lines a hand-built map can grow; after a
geometry change (`nodes_invalidated: true`) rebuild the nodes before a
vanilla play test.
