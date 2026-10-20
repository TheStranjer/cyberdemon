# Map feature tools

Complete game structures built on a map you have already drawn.
`add_door` cuts working doors; `add_window` glazes the walls between
rooms; `add_stairs` climbs a height change with evenly rising steps;
`add_lift` drops the high floor so a player standing below can ride up;
`add_teleporter` wires a pad to a destination so players step across the
map; `add_switch` mounts the triggers that fire
tagged sectors — including a `remote` door's; `add_exit` ends the level,
from a switch or by walking over a line. `mark_secret` and
`set_light_effect` are the two smallest: whole-sector edits of
`Sector::special` (and of `Sector::light`), for rooms and lighting you
have already drawn.

A **feature is ordinary map elements plus the action that makes them do
something**: `add_door` draws the doorway geometry (through the same
overlay engine as `create_sector_from_loop`), installs the line special
and tags, and dresses it with textures — all in one undoable step. Draw
your rooms first (`reference/sectors.md`), then add the doors, their
switches and the exit.

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
the sector behind their line and need no tag). Point an `add_switch` at
it (pass this call's `door_sector` as the switch's `target_sector` and
the tags match), or wire a walkover or an `ACS` script to that tag;
until a trigger exists the door never moves.

**Textures** default through the session's texture theme
(`reference/themes.md`): the `door` role dresses the moving face (the
upper of every room-facing side), `door_track` the jambs
(`door_track_red`/`_blue`/`_yellow` for the locked styles), and
`door_floor` the recess floor (room A's flat when the theme names none).
`texture` and `track_texture` override, validated for the map's format.
The jambs become one-sided, blocking, floor-pinned track walls; every
room-facing line is reversed — its sides and its `v1`/`v2` swap together,
so its front (the line's right, where `Doom` use actions fire and where
the renderer draws) faces its own room — cleared of its blocking bit and
top-pegged so the face rides the rising ceiling.

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

## `add_window`

`{map?, between: [sectorA, sectorB] | linedef: N, at?: {x, y}, width? =
64, sill? = 32, lintel? = 24}` → `{window_sector, lines, width, sill,
lintel, notes, map, nodes_invalidated, copied_to_working}`

Glazes a working window into a wall: a raised view strip between two
rooms that nobody can walk through, as one `undo` step. **Placement
mirrors `add_door` exactly** — same `between`/`linedef` pair (exactly one,
both or neither is `invalid_argument`), same optional `at` centre, same
`width` (default 64; a window wider than its wall names the wall's
available length), and between touching rooms the same 16-unit **void
strip** is carved along the *whole* shared boundary first (the `notes`
say so) before the window strip spans it. A one-sided wall reaches to the
first sector behind it (the same 256-unit outward ray and "no room behind
this wall" refusals as `add_door`). Sector A lends the strip its flats
and light.

**The heights are the taste parameters**: the window sector's floor is
`max(floorA, floorB) + sill` and its ceiling is `min(ceilA, ceilB) -
lintel` (defaults `sill: 32`, `lintel: 24`). The opening (ceiling minus
floor) must be **at least 16 map units** or the call fails
`invalid_argument` stating the minimum — and nothing at all is drawn, not
even the void strip. Its `special` and `tag` are 0: a window is inert.

**Nobody walks through**: the window's room-facing lines come out
two-sided (both rooms see through) with the `blocking` flag set, so
engines that read blocking on two-sided lines stop players and monsters
there; vanilla keeps them out on the sill height alone (a 32-unit step is
impassable). Their room sides wear the theme's `window_sill` role as the
`lower` (the band under the glass) and its `wall` role as the `upper`
(the band over it), validated for the map's format; the window-facing
sides stay clear, and the strip's end plugs are one-sided walls in the
wall texture.

**After the call**: geometry changed, so `nodes_invalidated` is true —
run `build_nodes` before playing in vanilla `Doom`. `window_sector` is
the strip, `lines` the two-sided blocking view lines; check the result
with `assemble_map` and `render_topdown`, and it keeps `validate_map`
clean.

```json
call add_window {"between": [0, 1]}
-> {"window_sector": 2, "lines": [4, 10], "width": 64, "sill": 32,
    "lintel": 24,
    "notes": ["carved a 16-unit void strip along the shared boundary of
              sectors 0 and 1: the window strip spans it"],
    "map": "MAP01", "nodes_invalidated": true, "copied_to_working": true}
```

## `add_stairs`

`{map?, from: {x, y}, to: {x, y}, width? = 64, step_height? = 16,
step_depth?, floor_from?, floor_to?}` → `{steps: [{sector,
floor_height, ceiling_height}], count, floor_from, floor_to,
step_height, width, notes, map, nodes_invalidated, copied_to_working}`

Builds a working staircase over a height change: the straight path from
`from` (in the lower area) to `to` (in the higher) is divided into
evenly rising step rectangles, painted on bottom-up, each becoming its
own sector — all in one `undo` step (label `add stairs`).

**The ends**: `from` and `to` must each lie inside a sector; a point in
no sector is `invalid_argument` naming it. `floor_from` and `floor_to`
default to the floors of the sectors at those points (explicit values
override). The count is `ceil(|floor_to - floor_from| / step_height)`
and the step floors rise evenly from `floor_from` to `floor_to`, so the
top step lands exactly on `floor_to`. Both ends at one floor is
`invalid_argument` (nothing to climb), and so is a zero-length path.

**The steps**: `step_height` (default 16) must be `1..=24` — the
player's maximum single step — or `invalid_argument`; `width` (default
64) must be positive and is each step's measure across the path. Each
step's ceiling is the ceiling of the sector under it, and that ceiling
must sit at least **72 map units** over the step's new floor: a step
that leaves less says *"not enough headroom"* (`invalid_argument`) and
nothing at all is painted. A path that crosses solid rock (a step
centre in no sector) or is shorter than its step count refuses the same
way, before any drawing.

**`step_depth`** (optional) fixes every tread: the path length must
then equal `count * step_depth`, ± 8 map units, else `invalid_argument`
states the required length *and* the depth that would divide this path
evenly. Without it the steps fill the path.

**Textures**: the risers — the `lower`/`upper` faces every height
difference creates — wear the session theme's `step` role
(`reference/themes.md`), validated for the map's format. Each step
sector copies the base sector under it: flats and light included, but
`special` and `tag` are 0 (stairs are inert; the geometry is the
walkable part).

**After the call**: geometry changed, so `nodes_invalidated` is true —
run `build_nodes` before playing in vanilla `Doom`. `steps` names the
created sectors with their heights, in path order (`from` first). Look
at the result with `render_topdown` and `render_view`, and check it with
`validate_map` (a clean staircase keeps it clean).

```json
call add_stairs {"from": {"x": 64, "y": 128}, "to": {"x": 448, "y": 128}}
-> {"steps": [{"sector": 2, "floor_height": 16, "ceiling_height": 160},
              {"sector": 3, "floor_height": 32, "ceiling_height": 160}],
    "count": 2, "floor_from": 0, "floor_to": 32, "step_height": 16,
    "width": 64, "notes": [],
    "map": "MAP01", "nodes_invalidated": true, "copied_to_working": true}
```

## `add_lift`

`{map?, platform: {points: [{x, y}, ...]} | sector: N, low_floor?,
style? = "use_wall", repeatable? = true}` → `{platform_sector, tag,
lines, floor_height, low_floor, drop, style, repeatable, notes, map,
nodes_invalidated, copied_to_working}`

Installs a working lift — a platform floor that drops to a lower level,
waits, and rises back — in one `undo` step (label `add use_wall lift`
or `add walk_trigger lift`). The player starts the ride from **below**.

**The platform** (name it exactly one way; both or neither is
`invalid_argument`):

- `sector: N` — an existing sector that already stands at the high
  floor. Out of range is `invalid_argument` naming it.
- `platform: {points: [{x, y}, ...]}` — draw a **new** platform: at
  least three corners, implicitly closed, painted with the same overlay
  engine as `create_sector_from_loop`. It copies the floor, ceiling,
  flats and light of the sector under its polygon centre, so draw it
  across a height change with the centre on the high side (a centre in
  the void is `invalid_argument`). The new sector's index comes back as
  `platform_sector`.

The platform is the **high floor**: it lowers to `low_floor`, which
defaults to its lowest neighbouring sector's floor (`get_map_summary`
lists neighbours) and must sit below the platform or the call is
`invalid_argument`. A platform with no neighbour must pass `low_floor`
explicitly.

**The trigger** — `style` (default `use_wall`, case-insensitive) is
`use_wall` (press the platform's side wall from below) or
`walk_trigger` (cross its edge); an unknown style lists both.
`repeatable` (default true) keeps the lines working. Every two-sided
edge of the platform that faces a sector **lower** than the platform
becomes an activation line, oriented with its **front** facing the
lower area, unblocked, wearing the theme's `lift_side` role on its
low-facing `lower` band; the platform's floor flat becomes the theme's
`lift_floor` role (both validated for the map's format). A platform with
no lower edge is `invalid_argument`. Everything is wired with a fresh
unused tag (the result's `tag`) stamped on the platform sector.

| Style          | `Doom` (once / repeat) | `Hexen` |
|----------------|------------------------|---------|
| `use_wall`     | `S1` 21 / `SR` 62      | `206` `Plat_DownWaitUpStayLip`, speed 32, delay 105, `playeruse` |
| `walk_trigger` | `W1` 10 / `WR` 88      | same special, `playercross` |

The `Doom` family moves the tagged sector's floor and back and carries
the tag on the line; the `Hexen` family puts the tag in `args[0]` (its
lines carry none) and replaces the whole trigger field, adding
`repeatspecial` when `repeatable`.

**A short drop is allowed**: below 64 map units of drop the lift still
builds but adds the note *"a lift is not needed; stairs or a step will
do"* — take `add_stairs` instead there (see `guides/design-rules.md`).
It is not an error.

**After the call**: `nodes_invalidated` is `true` when the platform was
drawn or an edge had to turn around (a property-only lift keeps its
nodes). `lines` are the activation linedefs, `floor_height` the high
floor, `drop` its distance above `low_floor`. Look at it with
`render_topdown` and `render_view`, check it with `validate_map`, and
start the ride from elsewhere by wiring an `add_switch` to the reported
tag.

```json
call add_lift {"sector": 1}
-> {"platform_sector": 1, "tag": 2, "lines": [2], "floor_height": 32,
    "low_floor": 0, "drop": 32, "style": "use_wall", "repeatable": true,
    "notes": ["this lift would drop only 32 map units: a lift is not
              needed; stairs or a step will do (below 64 the guide says
              prefer add_stairs)"],
    "map": "MAP01", "nodes_invalidated": false, "copied_to_working": true}
```

## `add_teleporter`

`{map?, pad: {x, y}, destination: {x, y, angle? = 0}, pad_size? = 64,
two_way? = false}` → `{pad_sector, destination_sector, things, pad_lines,
destination_lines, tag, return_tag, pad_size, two_way, notes, map,
nodes_invalidated, copied_to_working}`

Places a working teleporter in one undoable edit (label `add teleporter`,
or `add two-way teleporter`): a floor pad the player walks onto and a
destination square they arrive in.

**The two squares**: `pad: {x, y}` is the centre of the pad and
`destination: {x, y, angle?}` the centre of the destination, with the
angle (default 0, wrapped into `0..=359`) the arriving player faces.
Both points are required — a missing one is `invalid_argument` naming
it — and each must lie inside a sector (the refusal names the point).
`pad_size` (default 64, positive) is the side length of **both**
squares, centred on their points. The squares go through the same
overlay engine as `create_sector_from_loop`: each copies the floor,
ceiling, flats and light of the sector under its centre; the pad's
floor flat becomes the theme's `teleport_floor` role and the theme's
`wall` role dresses any new edge across nothing (both validated for the
map's format). A square whose borders face only solid rock is
`invalid_argument`.

**The destination** is a `TeleportDestination` Thing (type 14) standing
at the destination point. Which number makes the trip depends on the
flag family:

| Family | Pad lines | Destination |
|--------|-----------|-------------|
| `Doom` | special `97` (`WR` teleport, repeatable) on every two-sided pad border, answering with the fresh tag | the destination sector carries the tag (`97` picks the type-14 thing inside it) |
| `Hexen` | special `70` `Teleport` with the fresh TID in `args[0]`, the line's activation field replaced with `playercross` plus `repeatspecial` | the Thing itself carries the fresh TID (its sectors stay untagged) |

Every dressed border is oriented with its **front facing out of the
pad** and unblocked — players walk onto the pad from the surrounding
rooms, crossing the line front-first. The fresh tag/TID is one above
every tag the map carries (the result's `tag`).

**`two_way: true`** makes the destination a pad leading back as well: a
second type-14 Thing stands at the pad (facing the way back,
`angle + 180`), with its own fresh tag/TID (`return_tag`; `Doom`
stamps it on the pad sector, `Hexen` on the Thing), and every
two-sided border of the destination sector carries the same action
pointing at it. A one-way call leaves `return_tag` null and
`destination_lines` empty.

**After the call**: this is geometry, so `nodes_invalidated` is true —
run `build_nodes` before vanilla play. `pad_sector` and
`destination_sector` name the drawn squares, `things` the placed
indices (destination first, the return thing after), `pad_lines` and
`destination_lines` the dressed borders. Look at the result with
`render_topdown` (`reference/render.md`) and check it with
`validate_map`; one `undo` rolls the whole teleporter back.

```json
call add_teleporter {"pad": {"x": 64, "y": 64},
                     "destination": {"x": 384, "y": 64, "angle": 90}}
-> {"pad_sector": 2, "destination_sector": 3, "things": [2],
    "pad_lines": [11, 12, 13, 14], "destination_lines": [], "tag": 2,
    "return_tag": null, "pad_size": 64, "two_way": false, "notes": [],
    "map": "MAP01", "nodes_invalidated": true, "copied_to_working": true}
```

## `add_switch`

`{map?, linedef: N, at?: {x, y}, width? = 32, action, target_sector?,
repeatable? = false}` → `{linedef, split, action, special, tag, texture,
map, nodes_invalidated, copied_to_working}`

Mounts a usable switch panel on a wall: it isolates a `width`-wide chunk
of `linedef` as its own linedef (splitting the wall if needed), paints
the theme's switch texture on its front middle, and installs a
use-activated special that acts on a tagged sector — all in one `undo`
step. This is the trigger a `remote` `add_door` waits for.

**The wall**: `linedef: N` is required and must exist (`invalid_argument`
naming it otherwise). `at: {x, y}` optionally centres the panel there (a
point off the wall is projected onto it); without it the panel sits at
the wall's midpoint. `width` (default 32, positive) is the panel; a wall
too short for it fails `invalid_argument` naming the available length.
When the wall is longer than the panel it is split so the chunk becomes
its own linedef — the result's `linedef` names that new line and `split`
is `true`; when the line already is exactly that chunk nothing is cut
(`split` false, `linedef` is the one you passed). The front side must
wear the panel: a wall with no front sidedef fails `invalid_argument`
(give it one with `add_sidedef`).

**Actions** — `action` (case-insensitive) is required; an unknown name
fails `invalid_argument` listing all five:

| Action          | What it does                                             | Sector? | `Doom` family | `Hexen` family              |
|-----------------|----------------------------------------------------------|---------|---------------|-----------------------------|
| `open_door`     | Opens the tagged door, closing it after its wait          | yes     | `S1` 103 / `SR` 61 | `Door_Open` 11 (speed 16) |
| `raise_floor`   | Raises the tagged floor to the highest neighbouring floor | yes     | `S1` 18 / `SR` 69  | `Floor_RaiseToNearest` 25 (speed 8) |
| `lower_floor`   | Lowers the tagged floor to the lowest neighbouring floor  | yes     | `S1` 23 / `SR` 60  | `Floor_LowerToLowest` 21 (speed 8)  |
| `exit`          | Ends the level (normal exit)                              | no      | `S1` 11            | `Portal` 243                |
| `secret_exit`   | Ends the level (secret exit)                              | no      | `S1` 51            | `Portal` 244                |

**Targets**: the three sector-moving actions (`open_door`, `raise_floor`,
`lower_floor`) require `target_sector` — the index of the sector they act
on (missing or out of range is `invalid_argument`). The line answers with
that sector's existing tag; an **untagged** sector (`tag 0`) is stamped
with a fresh unused tag and the result's `tag` reports it. So `add_door`'s
`remote` door is wired by passing its reported `door_sector` here — the
tags match. The two exit actions act on no sector: `target_sector` and
`repeatable` are ignored for them (a level change fires once, and the
`Doom` family ships no repeatable exit).

**Repeatable**: `repeatable` (default false) picks the `SR` specials in
the `Doom` family and adds `repeatspecial` activation in the `Hexen`
family; the `Hexen` line's activation field is replaced with `playeruse`
(a `Doom`-family line keeps its flags).

**Textures**: the panel comes from the session's texture theme
(`reference/themes.md`) — the `switch` role, or `exit_switch` for the two
exit actions. It is written to the isolated line's front middle.

**After the call**: `nodes_invalidated` is true only when the wall was
split (that changed geometry — run `build_nodes` before playing in
vanilla `Doom`); a line already the right size is a property edit that
keeps the nodes. `special`/`tag`/`texture` echo what was installed.

```json
call add_switch {"linedef": 7, "action": "open_door", "target_sector": 2}
-> {"linedef": 42, "split": true, "action": "open_door", "special": 103,
    "tag": 2, "texture": "SW1COMP",
    "map": "MAP01", "nodes_invalidated": true, "copied_to_working": true}
```

## `add_exit`

`{map?, kind, secret? = false, linedef?, sector?}` → `{kind, lines,
special, secret, map, nodes_invalidated, copied_to_working}`

Places the level end in one undoable edit, either as an exit switch on a
wall or as lines the player crosses. `kind` (required, case-insensitive)
picks the trigger; `secret` (default false) picks the map's secret exit.

**`kind: "switch"`** is exactly `add_switch` with the `exit` (or
`secret_exit`) action: `linedef` (required — missing or out of range is
`invalid_argument` naming it) is the wall the panel mounts on, at the
default 32-unit width, wearing the theme's `exit_switch` texture. The
Doom family gets `S1` 11 (51); the Hexen family gets `Portal` 243 (244)
with `playeruse` activation. `sector` is ignored.

**`kind: "walk_over"`** dresses a whole sector: `sector` (required —
missing or out of range is `invalid_argument` naming it) — **every
two-sided linedef with the sector on either side** becomes a level-end
line. A sector with no two-sided boundary is refused with a hint. The
Doom family gets `W1` 52 (124); such a line's tag is the destination map
number in vanilla, so each line's existing tag is **kept** untouched (0
ends the game, or the secret level on 124 — set a real destination with
`edit_linedef`). The Hexen family gets special 243 (244) with its
activation field set to `playercross` — cross, not use; the lines' other
flag bits, tags and args stay. `linedef` is ignored, and nothing is
re-textured.

The result reports `kind`, the installed `special`, the `secret` flag and
`lines`: the one linedef for the switch kind (new when the wall was
split), every dressed boundary for `walk_over`. Switch-kind edits set
`nodes_invalidated` only when they split; walk-over edits are property
changes and never do.

```json
call add_exit {"kind": "walk_over", "sector": 1, "secret": true}
-> {"kind": "walk_over", "lines": [2], "special": 124, "secret": true,
    "map": "E1M1", "nodes_invalidated": false, "copied_to_working": true}
```

## `mark_secret`

`{map?, sectors: [index, ...], secret? = true}` → `{secret, family,
sectors, warnings, map, nodes_invalidated, copied_to_working}`

Marks whole sectors secret — or clears the flag — in one undoable step. It
is a pure edit of `Sector::special` on sectors that already exist: no
geometry, no textures, no tags, so `nodes_invalidated` is `false`.

**Where**: `sectors` is the list of sector indices to change (duplicates
collapse; an empty list is `invalid_argument`). An index the map does not
hold is `invalid_argument` naming it, and then nothing at all is written.
`secret` (default `true`) marks; `secret: false` clears. `map` defaults to
the active map.

**What the flag is** depends on the map's flag family:

| Family | Mark | Clear |
|--------|------|-------|
| `Doom` (binary `Doom`, `UDMF` `doom`/`heretic`/`strife`) | special `0` → the vanilla secret type `9`; a `Boom` generalized special (`32` and up) gains bit `128`; any other vanilla type (`1..=31`) also gains bit `128` — and a warning | the vanilla `9` → `0`; anything from `32` up loses bit `128`, every other bit kept |
| `Hexen` (binary `Hexen`, every other `UDMF` namespace) | bit `1024` set over the sector's own special | bit `1024` cleared, the special kept |

Vanilla `Doom` compares the whole special instead of reading bits, so a
`Boom` flag on a vanilla type is read as *neither* the old effect nor the
secret: that call still succeeds but returns the warning *"vanilla Doom
ignores Boom bits"* naming the sector. Give such a room its own sector
with special `9` when the map has to hide it in every engine. The `Hexen`
bit lives above the effect number, so it never needs a note.

**The result** echoes `secret` and `family`, lists in `sectors` the
indices whose special actually changed (an already-secret sector is
honestly reported as an empty list) and in `warnings` the Boom notes. One
`undo` rolls the whole call back — its history entry is labelled
`mark secret`, or `clear secret` when clearing. Check the numbers with
`assemble_map`, or ask `get_sector_specials` to decode a value (`flags`
then names `"secret"`).

```json
call mark_secret {"sectors": [4, 5]}
-> {"secret": true, "family": "doom", "sectors": [4, 5], "warnings": [],
    "map": "E1M1", "nodes_invalidated": false, "copied_to_working": true}
```

## `set_light_effect`

`{map?, sectors: [index, ...], effect, light?}` → `{effect, special,
family, light, sectors, map, nodes_invalidated, copied_to_working}`

Installs a vanilla light effect on whole sectors in one undoable step: the
same pure `Sector::special` edit as `mark_secret`, plus `Sector::light`
when you ask for it. No geometry, no textures, no tags —
`nodes_invalidated` is `false`.

**The effect** — `effect` (case-insensitive) is one of six names; an
unknown name is `invalid_argument` listing all six. Which number the
family stores differs:

| Effect | `Doom` family | `Hexen` family |
|--------|---------------|----------------|
| `none` | `0` | `0` |
| `random_off` | `1` | `65` |
| `strobe_fast` | `2` | `66` |
| `strobe_slow` | `3` | `67` |
| `glow` | `8` | `72` |
| `flicker` | `17` | `81` |

The `Hexen` numbers are `ZDoom`'s `Doom`-equivalents: the `Doom` type
plus `64`. Only the family's basic-type field is rewritten — a
`Doom`-family sector keeps every bit from `32` up (so a `Boom` generalized
sector stays generalized, secret bit included) and a `Hexen`-family sector
keeps every bit from `256` up (its damage flags and the secret bit at
`1024` survive). Marking a sector secret and lighting it are therefore
independent calls, in either order.

**The level** — `light` (optional, `0..=255`) also sets each selected
sector's `Sector::light`, the base the effect flashes around. Outside the
range is `invalid_argument`; left out, the level stands. Nothing else on
the sector moves: `UDMF` fields the model does not carry (a
`lightsequence`, a `colormap`) are preserved untouched.

**The result** echoes `effect`, `family` and the `special` this family
stores, reports the `light` you applied (`null` when you passed none), and
lists in `sectors` the indices whose special or light actually changed.
One `undo` rolls the whole call back, under the history label
`set light effect`. Read the rows back with `assemble_map`, or look at the
lighting with `render_topdown` (`fill_sectors` greys each sector by half
its light) and `render_view`.

```json
call set_light_effect {"sectors": [7], "effect": "flicker", "light": 176}
-> {"effect": "flicker", "special": 17, "family": "doom", "light": 176,
    "sectors": [7],
    "map": "E1M1", "nodes_invalidated": false, "copied_to_working": true}
```

## Growth points

- All nine feature tools (`add_door`, `add_window`, `add_stairs`,
  `add_lift`, `add_teleporter`, `add_switch`, `add_exit`, `mark_secret`,
  `set_light_effect`) are in place. Lower-level control stays with the
  `edit_*` groups and `run_batch`.
