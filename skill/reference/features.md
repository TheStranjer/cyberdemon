# Map feature tools

Complete game structures built on a map you have already drawn.
`add_door` is the first member of this group; `add_switch` and `add_exit`
will join it later (until then, wire a door's trigger with the low-level
`edit_linedef` tools).

A **feature is ordinary map elements plus the action that makes them do
something**: `add_door` draws the doorway geometry (through the same
overlay engine as `create_sector_from_loop`), installs the line special
and tags, and dresses it with textures — all in one undoable step. Draw
your rooms first (`reference/sectors.md`), then add the doors.

## `add_door`

`{map?, between: [sectorA, sectorB] | linedef: N, at?: {x, y}, width? =
64, depth?, style? = "normal", texture?, track_texture?}` →
`{door_sector, lines, jambs, style, tag, notes, map, nodes_invalidated,
copied_to_working}`

Cuts a working door into a wall: the doorway, the recess the door slides
in, the use action and the dressing, as one `undo` step.

**Where**: pass exactly one of

- `between: [sectorA, sectorB]` — the two rooms the door joins. They
  must share a two-sided boundary (both sector indices must exist, else
  `invalid_argument`). The door cuts the shared line nearest `at`, or the
  longest one when no `at` is given. Sector A lends the recess its floor
  and ceiling heights.
- `linedef: N` — the wall to cut. A two-sided line behaves like
  `between` on its two sectors; a one-sided line opens a recess towards
  the space its front faces.

`at: {x, y}` optionally centres the cut there (a point off the wall is
projected onto it); without it the cut sits at the wall's midpoint.
`width` (default 64) is the opening; a door wider than its wall fails
`invalid_argument` naming the wall's available length.

**Touching rooms**: when the wall separates two sectors, `add_door` first
carves a 16-unit **void strip** along their *whole* shared boundary (the
`notes` say so) and spans the door recess across it, so each room gets a
real wall face to the doorway and the door has somewhere to slide. The
rooms stop touching — if that matters for a corridor, check the map
afterwards with `render_topdown`.

**One-sided walls**: `depth` defaults to an outward ray — the distance
(≤ 256 units) to the first sector reachable behind the wall's front. An
explicit positive `depth` overrides the ray, but the recess's far middle
must land on (or inside) a sector. Both failures say *"no room behind
this wall within 256 units; move the rooms closer or give a larger
depth"* (`invalid_argument`). The recess opens into every sector it
touches: through-room doors become passages.

**Styles** — `style` (case-insensitive) picks the action installed on
every room-facing line:

| Style            | What it does                                            | `Doom` family | `Hexen` family                |
|------------------|---------------------------------------------------------|---------------|-------------------------------|
| `normal`         | Opens on use, waits, closes again                        | `DR` 1        | `Door_Raise` 12 speed 16, delay 150 |
| `fast`           | The same, at double speed                                | `DR` 117      | `Door_Raise` 12 speed 64      |
| `stay_open`      | Opens once on use and stays open (one-shot in `Doom`)    | `D1` 31       | `Door_Open` 11                |
| `locked_red`     | `normal` with the red key card                           | `DR` 28       | `Door_LockedRaise` 13, red    |
| `locked_blue`    | `normal` with the blue key card                          | `DR` 26       | `Door_LockedRaise` 13, blue   |
| `locked_yellow`  | `normal` with the yellow key card                        | `DR` 27       | `Door_LockedRaise` 13, yellow |
| `remote`         | No action on the door's lines — see below                | 0             | 0                             |

Unknown style names fail `invalid_argument` listing all seven.

A **`remote` door is inert until a switch is added**: the recess is built
closed and its sector tagged with a fresh unused tag (the result's
`tag`, `null` for every other style — plain `Doom`-family doors act on
the sector behind their line and need no tag). Point a switch or
walkover at that tag (or an `ACS` script at the sector) when it exists;
until then the door never moves.

**Textures** default through the session's texture theme
(`reference/themes.md`): the `door` role dresses the moving face (the
upper of every room-facing side), `door_track` the jambs
(`door_track_red`/`_blue`/`_yellow` for the locked styles), and
`door_floor` the recess floor (room A's flat when the theme names none).
`texture` and `track_texture` override, validated for the map's format.
The jambs become one-sided, blocking, floor-pinned track walls; every
room-facing line is flipped to front its own room (front-only use in the
`Doom` family), cleared of its blocking bit and top-pegged so the face
rides the rising ceiling.

**After the call**: the geometry changed, so `nodes_invalidated` is true
— run `build_nodes` (or `build_all_nodes`) before playing the map in
vanilla `Doom`. `lines` are the room-facing action lines, `jambs` the
track walls, `door_sector` the recess; check the result with
`assemble_map` and `render_topdown` (`reference/maps.md`,
`reference/render.md`).

```json
call add_door {"between": [0, 1]}
-> {"door_sector": 2, "lines": [4, 10], "jambs": [5, 7],
    "style": "normal", "tag": null,
    "notes": ["carved a 16-unit void strip along the shared boundary of
              sectors 0 and 1: the door recess spans it"],
    "map": "MAP01", "nodes_invalidated": true, "copied_to_working": true}
```

## Growth points

- `add_switch` will tag a wall face to raise a `remote` door (or any
  tagged sector), matching the `tag` reported here.
- `add_exit` will place level-end switches and lines by the same role
  defaults (`exit_switch`).
