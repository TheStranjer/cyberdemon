# Plan tools

Build a whole map from a declarative plan: rooms with footprints and
sector properties, connections joining them, thick walls grown in
between, things to stand in the result, and a theme to dress it all
with. One call compiles the plan into a complete map block and writes
it into the working archive as a single undoable edit.

Things land in the same call: `things` rows and a mandatory
`player_start` (or a thing of type `1`) populate the block. `finalize`
builds the node run into the block and validates the result, all inside
the one undoable edit. Door connectors are wired: the band sector takes
a fresh tag and the room-facing lines the style's door special
(coloured jambs for locked doors, a bare tagged band for `remote`).
`features` rows — `exit`, `switch`, `lift`
and `teleporter` declarations — parse, transform and validate now and
install nothing until the follow-up CD-95 wiring slices land.

## `build_map_from_plan`

`{map, format? = "doom", namespace?, replace? = false, wall_thickness?
= 16, rooms: [...], connections: [...], features?: [{kind: exit |
switch | lift | teleporter, …}], things?: [{type | name, x + y |
room, count? = 1, angle? = 0, skills?, multiplayer_only?}],
player_start?: {x, y, angle?} | {room, angle?}, theme?, units?
{scale_num? = 1, scale_den? = 1, origin_x? = 0, origin_y? = 0, flip_y?
= false}, grid? = 8, finalize? = false}` → `{map, format, rooms:
{id: [sectors]}, connections: [{id, kind, sectors}], features:
[{kind, room, sectors, lines, tag}], things_placed,
bounds: {x1, y1, x2, y2} | null, warnings, nodes: {...} | null,
validation: {errors, counts} | null, next_steps}`

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
ones), `outdoor` (default `false`; an outdoor room's ceiling is the
sky unless it names a flat), `secret` (default `false`) and
`light_effect` (unset by default; one of `none`, `flicker`,
`strobe_fast`, `strobe_slow`, `glow` or `random_off`,
case-insensitive). `secret` and `light_effect` describe the room for
the follow-up CD-95 wiring slices and install nothing yet.

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
| `door`     | `style?`     | A **closed** band: floor equals ceiling. `style` is `normal` (default), `fast`, `stay_open`, `locked_red`, `locked_blue`, `locked_yellow` or `remote` — it picks the door's action and dressing. |
| `window`   | `sill?` = 32, `lintel?` = 24 | A glass band: `sill` lifts its floor, `lintel` drops its ceiling; under 16 units of glass left is `invalid_argument`. |
| `stairs`   | `step_height?` = 16, `step_depth?` = 24 | A stepped run from the lower room's floor to the higher one's, one sector per step (`step_depth` must be positive). |

> **Doors are wired.** A `door` connector's band sector takes a fresh
> tag and its two room-facing lines the style's action in the map's
> flag family, turned so each room's side is the line's front: `normal`
> → `DR` special `1` (Hexen `Door_Raise`), `fast` → `117`
> (`Door_Raise`, speed `64`), `stay_open` → `D1` `31` (`Door_Open`),
> `locked_red`/`locked_blue`/`locked_yellow` → `28`/`26`/`27`
> (`Door_LockedRaise` with the key in `args[3]`), with the jambs
> re-dressed in the theme's coloured track. In the `Hexen` family the
> lines carry no tag — `args[0] = 0` acts on the sector behind each —
> and take the `playeruse` activation bits. `remote` stamps the sector
> tag and writes nothing on the lines: the tagged band is the handle a
> later `switch` row (or your own trigger line) fires on. Every door
> lands in the result's `features` list with its sector, lines and tag.

**The features**: `features` (default none) declares game structures
beside the geometry. Each row gives a `kind` (case-insensitive) plus
that kind's fields:

| kind         | Fields | Declares |
|--------------|--------|----------|
| `exit`       | `room`, `style?` = `switch` or `walk_over` (default `switch`), `secret?` = false, `wall?` | The level end standing in `room`; `wall` is `north`, `south`, `east` or `west` (unset lets the wiring decide). |
| `switch`     | `room`, `opens`, `wall?`, `repeatable?` = false | A wall switch in `room` operating the connection whose `id` is `opens`. |
| `lift`       | `room`, `low_floor?`, `style?` = `use_wall` or `walk_trigger` (default `use_wall`), `repeatable?` = true | A platform in `room` lowering to `low_floor` (default: the lowest neighbouring floor). |
| `teleporter` | `from_room`, `to_room`, `pad?`, `destination?`, `angle?` = 0, `two_way?` = false | A pad in `from_room` and a destination in `to_room`; the squares are plan points `[x, y]` carried through the same transform as the rooms (the room centres by default), `angle` is the arrival facing. |

The rows reference-check like connections: a feature naming a room the
plan does not hold, a `switch` whose `opens` matches no connection `id`
(an unnamed connection is refused by its `from->to` label), a row
missing its `room` (its `from_room`/`to_room`, or the switch's
`opens`), and a bad `kind`, `style` or `wall` name are all
`invalid_argument` naming `features[i]` — the vocabulary refusals list
the accepted names.

> **Features install nothing yet.** They parse, transform and validate
> now; dressing them onto the compiled map arrives with the follow-up
> CD-95 wiring slices on this branch. A plan with features compiles
> exactly like the plan without them.

**Units and the grid**: room footprints, connection `at` positions and
teleporter `pad`/`destination` squares are plan coordinates, drawn in
whatever units you think in (sketch
pixels, half-tiles, centimetres). `units` turns them into map units
before compiling: `x` maps to `origin_x + x * scale_num / scale_den`,
`y` to `origin_y + y * scale_num / scale_den` — or `origin_y` minus the
scaled `y` when `flip_y: true` mirrors an image-space drawing (image
rows grow down, map `y` grows up). Everything is integer and rounds
half away from zero. Omitted, the transform is the identity (scale
`1/1`, origins `0`, no mirror). Then `grid` (default `8`) snaps every
transformed coordinate to its nearest multiple; `1` switches snapping
off. A `grid` below `1`, a `scale_den` of `0` and a `scale_num` of `0`
are each `invalid_argument` naming the parameter. **Sizes are not
transformed**: `wall_thickness`, connection `width`, a lift's
`low_floor` and the `sill`/`lintel`/`step_height`/`step_depth` values
are map units already. Rectangles have both corners snapped and their
`w`/`h`
re-derived, so they stay grid-aligned; an edge the grid collapses to 0
is refused naming the plan path.

> **Image-space recipe**: traced a sketch at pixel scale? Draw the plan
> in pixels and pass `units: {scale_num: 4, flip_y: true}` — one pixel
> becomes 4 map units with `y` mirrored (the default `grid: 8` then
> aligns everything to 2-pixel lines).

**The things**: `things` is a list of rows, each naming *what* and
*where*:

- **What** — exactly one of `type` (a doomednum written straight into
  `THINGS`) and `name`, resolved against the session's game definitions
  (the same DEHACKED/DECORATE/MAPINFO-overlaid tables `validate_map`
  reads): a case-insensitive exact thing name wins, else a
  case-insensitive substring must match exactly one distinct type. No
  match and an ambiguous match are both `invalid_argument` naming
  `things[i]`; the ambiguity lists the candidate names so you can pick.
- **Where** — exactly one of an `x`/`y` plan point (transformed and
  snapped through `units` and `grid` exactly like the room footprints)
  and a `room` id to fill. Half a coordinate pair, or both a point and
  a room, is `invalid_argument` naming `things[i]`; an unknown room id
  names `things[i].room`.
- `count` (default `1`) is how many copies a room fill places; a pinned
  point holds exactly one thing. `angle` (default `0`) must lie in
  `0..=359`.

A room fill is deterministic: starting at the room sector's bounding
box minimum corner raised by the thing's *clearance* — its definition's
blocking radius (16 when the definitions do not list the type) plus 8
map units — candidate points run row-major on a grid spaced by twice
the clearance, and a point is kept only when the sector contains it and
it stands at least the clearance from every wall. When fewer points fit
than `count` asks for, the call still succeeds and `warnings` gains
`"room 'x' fits only N of M things"`.

`skills` (some of `"easy"`, `"medium"`, `"hard"`, case-insensitive; an
unknown name is `invalid_argument` naming `things[i].skills`) sets the
skill presence bits, defaulting to all three. `multiplayer_only: true`
keeps the thing out of single-player: the `notsingle` bit in the Doom
family, the `coop` + `dm` mode bits in the Hexen one (where `single`
clears the same way). Unset, a thing is present in every skill and
game mode.

**The player start**: every built map needs one. `player_start` is
`{x, y, angle?}` (a plan point through the same transform) or
`{room, angle?}` (the room sector's bounding-box centre); it adds a
thing of type `1`. Without it — and without any `things` row whose
`type` is `1` or whose `name` resolves to type `1` — the call fails
`invalid_argument`: `a map needs a player 1 start; pass player_start or
a thing of type 1`.

**The theme**: textures resolve through the session's texture theme
(`reference/themes.md`) once per call — the plan roles `floor`,
`ceiling`, `wall`, `door`, `door_floor`, `door_track`, `window_sill`
and `step`. The door step additionally consults `door_track_red`,
`door_track_blue` and `door_track_yellow` (the theme table falls them
back to `door_track`); a coloured track the theme answers `"-"` leaves
a locked door's jambs in the plain track and adds its own note. `theme`
names a different theme **for this call only**
(case-insensitive; unknown names fail `invalid_argument` listing the
game's themes); the session's theme and per-role overrides never
change, and they still apply under the override. A room's own
`floor_flat` / `ceiling_flat` / `wall_texture` beat the theme. A role
the theme answers `"-"` (an empty session) leaves its faces undressed
and adds a `warnings` note.

**Finalize: node data and validation.** `finalize: true` runs the whole
`build_all_nodes` set (`reference/nodes.md`) on the compiled model
before the write, inside the same undoable edit — one `undo` still takes
the whole block back, node run included:

- A **binary** map (`doom`/`hexen`) gets the classic
  `SEGS`/`SSECTORS`/`NODES` run (split vertices appended to
  `VERTEXES`), a `zero` `REJECT` and a fresh `BLOCKMAP` — the same
  defaults `build_all_nodes` picks for it.
- A **`udmf`** map gets the compressed extended lump alone as
  `ZNODES`; no `REJECT`/`BLOCKMAP`, which `ZDoom` builds at load.

The result's `nodes` then carries the build summary
`{variant: "classic" | "extended", segs, subsectors, nodes, warnings}`
plus the layout's extra counts (`split_vertices` for `classic`;
`format`, `bytes` and `new_vertices` for `extended`). `validation`
carries an errors-only pass: every `validate_map` check run over the
finished model with the session's definitions and merged resource index,
reported as `{"errors": [...], "counts": {error, warning, info}}` —
error rows in the `validate_map` issue shape, at most 20 of them, while
`counts` is always exact over everything the checks found. **Validation
errors do not roll the build back**: the map was built to plan, and the
rows are for you to fix with the element tools. Door connectors are
already wired when `finalize` runs — the pass judges the finished
specials and tags. With `finalize` unset or
`false`, `nodes` and `validation` stay `null` and the block ships
without node data, as before.

**Errors name the plan path.** Structural refusals from the compiler
(bad rectangle edges, polygons with too few points, duplicate room
ids, unknown or self-joined rooms, non-positive widths, too-short
boundaries…) already name the offending id; the tool prefixes them
with the wire index — `rooms[0]: room 'a' has a 0x64 rectangle; both
edges must be positive`. Feature rows name `features[i]`
(`features[0]: exit feature names unknown room 'ghost'`), and thing
rows fail the same way, naming `things[i]`, `things[i].room`,
`things[i].skills` or `player_start`.

**The result**: `rooms` maps each room id to the sector indices it
owns (a fully painted-over room holds no entry and warns about it);
`connections` reports `{id, kind, sectors}` per connection in plan
order (`id` is the connection's `id`, or its derived `from->to`
label); `features` reports one `{kind, room, sectors, lines, tag}` row
per installed game feature in pipeline order — doors first, in
connection order, each with `kind` `door`, `room` `null`, its band
`sectors`, the room-facing `lines` carrying the action (a `remote`'s
stay bare) and the shared `tag`; the follow-up slices append the
switch, lift, teleporter and exit rows. `things_placed` counts the
`THINGS` rows written, room fills
and the player start included. `bounds` is the vertex box
`{x1, y1, x2, y2}`. `warnings` carries the compiler's advisories —
rooms unreachable from the first, rooms painted over, floor gaps taller
than 24 units through an opening/door/window ("use stairs"), rooms that
fit fewer things than asked — plus the theme notes. `nodes` and
`validation` are `null` unless `finalize: true` filled them (see the
**Finalize** section above).

```json
call build_map_from_plan {
  "map": "MAP07",
  "units": {"scale_num": 4, "flip_y": true},
  "rooms": [
    {"id": "court", "rect": {"x": 0,  "y": 0,  "w": 60, "h": 48}},
    {"id": "hall",  "rect": {"x": 60, "y": 0,  "w": 48, "h": 48}, "light": 192},
    {"id": "crypt", "rect": {"x": 60, "y": 48, "w": 48, "h": 36},
     "floor": 64, "ceiling": 224, "light": 96}
  ],
  "connections": [
    {"id": "gate", "from": "court", "to": "hall", "kind": "door",
     "at": [60, 24], "width": 48, "style": "locked_red"},
    {"from": "hall", "to": "crypt", "kind": "stairs",
     "at": [84, 48], "width": 64}
  ],
  "things": [
    {"name": "imp", "room": "hall", "count": 3},
    {"name": "barrel", "x": 16, "y": 40}
  ],
  "player_start": {"room": "court", "angle": 90},
  "finalize": true
}
-> {"map": "MAP07", "format": "doom",
    "rooms": {"court": [0], "crypt": [2], "hall": [1]},
    "connections": [
      {"id": "gate", "kind": "door", "sectors": [3]},
      {"id": "hall->crypt", "kind": "stairs", "sectors": [4, 5, 6, 7]}],
    "features": [
      {"kind": "door", "room": null, "sectors": [3], "lines": [5, 17], "tag": 1}],
    "things_placed": 5,
    "bounds": {"x1": 0, "y1": -336, "x2": 432, "y2": 0},
    "warnings": [],
    "nodes": {"variant": "classic", "segs": 61, "subsectors": 15, "nodes": 14,
              "split_vertices": 5, "warnings": []},
    "validation": {"errors": [], "counts": {"error": 0, "warning": 4, "info": 6}},
    "next_steps": ["render_topdown to compare with the sketch",
                   "render_view from the player start"]}
```

A sketch measured in pixels: `units: {scale_num: 4, flip_y: true}` turns
the 108x84 drawing into a 432x336 map with `y` mirrored, on the default
`grid: 8`. The courtyard and hall share the line `x = 60` (px); the
red-key door `gate` sits low on it and comes out closed, dressed and
wired: band sector 3 tagged `1`, its two room-facing lines (5 and 17)
carrying `DR` special `28` and tag `1` with the fronts facing their
rooms, and the jambs in the theme's red track. The crypt stands
behind the hall with its floor 64 units up, reached by the four stair
steps (sectors 4–7) the stairs connector grew. Three imps fill the
hall's clearance grid, the barrel pins at plan pixel `(16, 40)`, and
the player starts centre-courtyard facing east. `finalize` built the
classic node run plus `REJECT` and `BLOCKMAP` into the same written
block — one `undo` takes all of it back — and the errors-only
validation came back clean.

**After the call**: `render_topdown` compares the layout with the
sketch you worked from, `render_view` walks it first-person from the
placed player start, and `validate_map` judges the map. A block built
without `finalize` reports `nodes_missing`: play it in vanilla `Doom`
only after `build_nodes` / `build_all_nodes` ran (`reference/nodes.md`),
or build it with `finalize: true` from the start. Failed calls change
nothing: no block is written and no undo entry is recorded.
