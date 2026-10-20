# Plan tools

Build a whole map from a declarative plan: rooms with footprints and
sector properties, connections joining them, thick walls grown in
between, and a theme to dress it all with. One call compiles the plan
into a complete map block and writes it into the working archive as a
single undoable edit.

Geometry only for now: the compiled map has no things (no player start
yet), door connectors come out closed but un-wired, and `nodes` /
`validation` in the result are reserved fields later slices fill.

## `build_map_from_plan`

`{map, format? = "doom", namespace?, replace? = false, wall_thickness?
= 16, rooms: [...], connections: [...], theme?}` → `{map, format,
rooms: {id: [sectors]}, connections: [{id, kind, sectors}], bounds:
{x1, y1, x2, y2} | null, warnings, nodes: null, validation: null,
next_steps}`

**The map**: `map` is the new block's header name (1 to 8 printable
ASCII, case-insensitive). When the *working archive* already holds a
block of that name the call fails `already_exists` — pass
`replace: true` to discard the old block (and its GL run) first. A map
only the `IWAD` or a resource layer holds is always overridable and
needs no `replace`. The built map becomes the session's active map;
one `undo` takes the whole block back.

**The layout**: `format` is `"doom"` (default), `"hexen"` or `"udmf"`
(case-insensitive). `namespace` is the `UDMF` `TEXTMAP`'s namespace
declaration (1 to 32 ASCII letters, digits or underscores, lowercased,
default `"zdoom"`) and is ignored for the binary formats.

**The rooms**: `rooms` is painter's order — a later room paints over an
earlier one where they overlap. Each room has an `id` (unique in the
plan; connections refer to it) and exactly one footprint:

- `rect: {x, y, w, h}` — the axis-aligned rectangle from `(x, y)` to
  `(x + w, y + h)`; both edges must be positive.
- `points: [[x, y], ...]` — a closed ring of at least three distinct
  points, implicitly closing to the first.

Both or neither fails `invalid_argument` naming the plan path
(`rooms[2]`). Sector properties, all optional: `floor` (default `0`),
`ceiling` (default `128`), `light` (default `160`), `floor_flat`,
`ceiling_flat` and `wall_texture` (the theme's roles answer unset
ones) and `outdoor` (default `false`; an outdoor room's ceiling is the
sky unless it names a flat).

**The walls**: `wall_thickness` (default `16`) is the map-unit wall
grown between and around rooms; `0` lets rooms share their walls
directly. Negative is refused.

**The connections**: each joins two distinct existing rooms. `kind`
(case-insensitive) picks the intervention; `at: [x, y]` places it on
the shared boundary nearest that point (default: centred on it) and
`width` (default `64`) is its clear width, which must fit the
boundary. Rooms that share no boundary fail `invalid_argument` naming
the connection and saying they *do not touch*.

| kind       | Extra fields | What lands |
|------------|--------------|------------|
| `opening`  | —            | A plain gap: the band sector spans `max(floor)` to `min(ceiling)`. |
| `door`     | `style?`     | A **closed** band: floor equals ceiling. `style` is `normal` (default), `fast`, `stay_open`, `locked_red`, `locked_blue`, `locked_yellow` or `remote` — it picks the door's dressing only. |
| `window`   | `sill?` = 32, `lintel?` = 24 | A glass band: `sill` lifts its floor, `lintel` drops its ceiling; under 16 units of glass left is `invalid_argument`. |
| `stairs`   | `step_height?` = 16, `step_depth?` = 24 | A stepped run from the lower room's floor to the higher one's, one sector per step (`step_depth` must be positive). |

> **Doors have no action yet.** A `door` connector emits the doorway
> geometry dressed with the theme's `door`/`door_track`/`door_floor`
> roles, but the door sector carries **special `0`** and its lines no
> trigger — until a later slice (or `add_door` on the finished map)
> wires it, the door is a solid closed threshold. Read the reported
> `sectors` and check with `assemble_map` / `render_topdown`.

**The theme**: textures resolve through the session's texture theme
(`reference/themes.md`) once per call — the plan roles `floor`,
`ceiling`, `wall`, `door`, `door_floor`, `door_track`, `window_sill`
and `step`. `theme` names a different theme **for this call only**
(case-insensitive; unknown names fail `invalid_argument` listing the
game's themes); the session's theme and per-role overrides never
change, and they still apply under the override. A room's own
`floor_flat` / `ceiling_flat` / `wall_texture` beat the theme. A role
the theme answers `"-"` (an empty session) leaves its faces undressed
and adds a `warnings` note.

**Errors name the plan path.** Structural refusals from the compiler
(bad rectangle edges, polygons with too few points, duplicate room
ids, unknown or self-joined rooms, non-positive widths, too-short
boundaries…) already name the offending id; the tool prefixes them
with the wire index — `rooms[0]: room 'a' has a 0x64 rectangle; both
edges must be positive`.

**The result**: `rooms` maps each room id to the sector indices it
owns (a fully painted-over room holds no entry and warns about it);
`connections` reports `{id, kind, sectors}` per connection in plan
order (`id` is the connection's `id`, or its derived `from->to`
label). `bounds` is the vertex box `{x1, y1, x2, y2}`. `warnings`
carries the compiler's advisories — rooms unreachable from the first,
rooms painted over, floor gaps taller than 24 units through an
opening/door/window ("use stairs") — plus the theme notes. `nodes` and
`validation` are `null` placeholders the later slices fill; there is
no `things_placed` count yet because no things are placed.

```json
call build_map_from_plan {
  "map": "MAP05",
  "rooms": [
    {"id": "hall",   "rect": {"x": 0,   "y": 0, "w": 128, "h": 128}},
    {"id": "vault",  "rect": {"x": 128, "y": 0, "w": 128, "h": 128}, "light": 200}
  ],
  "connections": [
    {"id": "vdoor", "from": "hall", "to": "vault", "kind": "door", "at": [128, 32]},
    {"from": "hall", "to": "vault", "kind": "opening", "at": [128, 96]}
  ]
}
-> {"map": "MAP05", "format": "doom",
    "rooms": {"hall": [0], "vault": [1]},
    "connections": [
      {"id": "vdoor", "kind": "door", "sectors": [2]},
      {"id": "hall->vault", "kind": "opening", "sectors": [3]}],
    "bounds": {"x1": 0, "y1": 0, "x2": 256, "y2": 128},
    "warnings": [],
    "nodes": null, "validation": null,
    "next_steps": ["render_topdown to compare with the sketch",
                   "render_view from the player start"]}
```

Two 128×128 rooms sharing the line `x = 128`: a door low on their
shared wall, an opening high on it. The wall between them is the
default 16 units thick; the door sector (2) is closed and carries no
special until something wires it.

**After the call**: `render_topdown` compares the layout with the
sketch you worked from, `render_view` walks it first-person (place
start things first — no things exist yet), and `validate_map` judges
the geometry (`nodes_missing` is expected on a fresh block: run
`build_nodes` / `build_all_nodes` before playing the map in vanilla
`Doom`, `reference/nodes.md`). Failed calls change nothing: no block
is written and no undo entry is recorded.
