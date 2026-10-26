# Plan tools

Build a whole map from a declarative plan: rooms with footprints and
sector properties, connections joining them, thick walls grown in
between, things to stand in the result, and a theme to dress it all
with. One call compiles the plan into a complete map block and writes
it into the working archive as a single undoable edit.

One plan describes the whole playable map: geometry, things and
behaviour. Things land in the same call: `things` rows and a mandatory
`player_start` (or a thing of type `1`) populate the block. `finalize`
builds the node run into the block and validates the result, all inside
the one undoable edit. Door connectors are wired: the band sector takes
a fresh tag and the room-facing lines the style's door special
(coloured jambs for locked doors, a bare tagged band for `remote`).
`features` rows — `exit`, `switch`, `lift` and `teleporter`
declarations — install too, in a fixed pipeline order (doors, switches,
lifts, teleporters, exits, lights, secrets): a `switch` row mounts a
panel on its room's wall that fires a remote door's tag, a `lift` row
dresses its room's sector as a tagged platform, a `teleporter` row
draws the pad and destination squares and stands its type-14 things,
and an `exit` row ends the level by panel or by walk-over. Rooms'
`secret` and `light_effect` fields paint the family's sector specials
— lights before secrets. When the wiring leaves a judgment gap — a
locked door whose key stands nowhere in the plan, a remote door no
switch opens, a map with no exit — the result says so in `warnings`;
the build never fails on them.

## `build_map_from_plan`

`{map, format? = "doom", namespace?, replace? = false, wall_thickness?
= 16, rooms: [...], connections: [...], features?: [{kind: exit |
switch | lift | teleporter, …}], things?: [{type | name, x + y |
room, count? = 1, angle? = 0, skills?, multiplayer_only?}],
player_start?: {x, y, angle?} | {room, angle?}, theme?, units?
{scale_num? = 1, scale_den? = 1, origin_x? = 0, origin_y? = 0, flip_y?
= false}, grid? = 8, finalize? = false}` → `{map, format, rooms:
{id: [sectors]}, connections: [{id, kind, sectors}], features:
[{kind, room, sectors, lines, tag, line_ends}], things_placed,
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
case-insensitive). The feature run installs both: `light_effect`
paints the family's basic light type onto every sector of the room
(keeping the room's `light` and the bits above the type field), and
`secret` sets the family's secret flag over whatever the earlier steps
left — the lights step always runs before the secrets step, so a lit
secret room keeps both. A `Doom`-family room that already carries a
vanilla type (a light effect, say) gets a `Boom` secret *bit* rather
than a second type, and the vanilla-engine warning for that rides in
`warnings`.

**The walls**: `wall_thickness` (default `16`) is the map-unit wall
grown between and around rooms; `0` lets rooms share their walls
directly. Negative is refused. A wall rectangle spans the shared run it
fills and no further, so no wall stands where the rooms really have no
shared edge; where two shared runs meet at a corner the rectangles reach
half a thickness past it and overlap, leaving no unwalled gap.

**The connections**: each joins two distinct existing rooms and an `id`,
when given, must be unique (a duplicate is `invalid_argument` naming
both `connections[i]` indices). `kind`
(case-insensitive) picks the intervention; `at: [x, y]` places it on
the shared boundary nearest that point (default: centred on it) and
`width` (default `64`) is its clear width, which must fit the
boundary. Rooms that share no boundary fail `invalid_argument` naming
the connection and saying they *do not touch*.

| kind       | Extra fields | What lands |
|------------|--------------|------------|
| `opening`  | —            | A plain gap: the band sector spans `max(floor)` to `min(ceiling)`. A `width` that spans the *whole* shared edge takes the wall down along that run instead: the rooms join on their own boundary line, own no band sector (`sectors: []`) and no jamb walls stand at the ends. |
| `door`     | `style?`     | A **closed** band: floor equals ceiling. `style` is `normal` (default), `fast`, `stay_open`, `locked_red`, `locked_blue`, `locked_yellow` or `remote` — it picks the door's action and dressing. |
| `window`   | `sill?` = 32, `lintel?` = 24 | A glass band: `sill` lifts its floor, `lintel` drops its ceiling. Both trims must be `0` or greater and together must leave the rooms' opening partly clear — a negative trim or one that fills the opening is `invalid_argument`, and so is under 16 units of glass left. A window blocks the way: a room joined only by windows warns it cannot be reached. |
| `stairs`   | `step_height?` = 16, `step_depth?` = 24 | A stepped run from the lower room's floor to the higher one's, one sector per step (`step_depth` must be positive, and `step_height` must be `1..=24` — outside that the plan is refused, not clamped). |

> **Bands wear the rooms' flats.** A `door` band's floor and ceiling take
> the theme's `door_floor`; every other band's two surfaces take the flat
> of the adjoining room standing at that height (the room the connection
> leads `from` where neither does, so a stair step or a glass box
> continues its own room) and its light is the dimmer of the two rooms.
> So a passage never stands in flats its own rooms do not wear.

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
> lands in the result's `features` list with its sector, its lines and
> their endpoints, and its tag.

**The features**, one plain-English line each, across the whole set a
plan can carry:

- **Door wiring** (a `door` connection) — the closed band opens on use,
  on touch, on its key card, or from a remote switch; wired the moment
  the map compiles.
- **Switch** (`kind: switch`) — a wall panel that fires the remote door
  whose id it names.
- **Lift** (`kind: lift`) — the room's own sector becomes a platform
  that rides down to its low floor and back up.
- **Teleporter** (`kind: teleporter`) — walk onto the pad, arrive at
  the destination square facing the angle you set.
- **Exit** (`kind: exit`) — the level ends at its panel or across its
  walked-over boundaries.
- **Light effect** (a room's `light_effect`) — flicker, strobe, glow or
  random-off painted onto every sector of the room.
- **Secret** (a room's `secret`) — the family's secret flag over
  whatever special the room carries; a lit secret keeps both.

`features` (default none) declares the middle four beside the geometry;
`light_effect` and `secret` ride on the rooms and the door wiring comes
with the connections. Each `features` row gives a `kind`
(case-insensitive) plus that kind's fields:

| kind         | Fields | Installs |
|--------------|--------|----------|
| `exit`       | `room`, `style?` = `switch` or `walk_over` (default `switch`), `secret?` = false, `wall?`, `at?` | The level end standing in `room`. `switch` mounts the 32-unit exit panel on a wall of `room` (`Doom` specials `11`/`51`, Hexen portal `243`/`244`); `walk_over` dresses every two-sided boundary of the room's sectors with the family's walk-over exit (`W1` `52`/`124`, tag kept as the destination map number — a plan-built room's bare `0` ends the game). `secret: true` picks the secret form. `wall` is `north`, `south`, `east` or `west` and `at` the plan point that picks among several matching walls; both apply to the `switch` style only — a `walk_over` exit dresses every boundary. See the callout below. |
| `switch`     | `room`, `opens`, `wall?`, `at?`, `repeatable?` = false | A 32-unit switch panel on the chosen wall of `room`, centred on its own `at` point (or the wall midpoint without one), opening the remote door whose connection `id` is `opens` (`Doom` `S1` `103`, or `SR` `61` with `repeatable: true`; `Hexen` `Door_Open`). The switch fires exactly the tag the door's band sector carries. `opens` must name a `remote` door: any other connection fails `invalid_argument`. |
| `lift`       | `room`, `low_floor?`, `style?` = `use_wall` or `walk_trigger` (default `use_wall`), `repeatable?` = true | The room's own sector becomes the platform: it takes a fresh tag, its edges facing lower sectors turn front-down onto the player below and carry the family's down-wait-up action (`Doom` `SR` `62` / `S1` `21` for `use_wall`, `WR` `88` / `W1` `10` for `walk_trigger`, `repeatable` picking the pair; `Hexen` `Plat_DownWaitUpStayLip` `206` with tag, speed and wait args), and those faces wear the theme's `step` texture. `low_floor` is the ride's bottom (default: the lowest neighbouring floor). Every connection from the lift room to a room with a lower floor must be an `opening` — anything else fails `invalid_argument` (*a lift needs openings to the lower rooms*, naming the rooms and the connection) — and a drop under 64 units notes that stairs will do. |
| `teleporter` | `from_room`, `to_room`, `pad?`, `destination?`, `angle?` = 0, `two_way?` = false | A 64-unit pad square in `from_room` and a destination square in `to_room`, centred on `pad`/`destination` (plan points carried through the same transform as the rooms; the room sectors' bounding-box centres by default). Every square corner inset 8 units must resolve into its room, else `invalid_argument` names that room. The pad's borders carry the family's walk-on teleport (`Doom` `WR` `97` answering the destination's fresh sector tag, `Hexen` `Teleport` `70` with the destination thing's TID); a type-14 destination thing stands at the destination facing `angle`, and the pad keeps the room's own floor flat. `two_way` runs the return leg — its own tag, and a thing at the pad facing back. |

> **The wall a panel mounts on.** The candidates are the room's
> one-sided walls whose outward normal points at `wall` (`north` is
> `+y`, `south` `-y`, `east` `+x`, `west` `-x`); with `wall` unset every
> one-sided wall of the room competes. A diagonal wall answers two sides
> at once, and a wall a notch or a connector band cuts counts as one
> wall per piece, so a room often has several walls matching one side.
>
> - **With `at`** the panel goes to the wall standing *nearest* that
>   point — the exact point-to-segment distance, so a point inside the
>   room measures to the wall face that faces it. The panel then centres
>   on `at` projected onto that wall (clamped to its ends).
> - **Without `at`** the *longest* matching wall wins.
> - Either way an exact tie falls to the longer wall and then to the
>   **lowest linedef index**, the order the compiled map holds its lines.
>
> A room with no wall on the asked side (or none at all) fails
> `invalid_argument` naming the room and the side; an `at` the map
> cannot hold fails naming the point; and an `at` whose projected spot
> leaves the 32-unit panel no room on that stretch of wall fails naming
> the line and its length, with a hint to move `at` toward the middle of
> the wall (or to leave it out). Read the result's `line_ends` (see
> **The result** below) to check the wall you asked for is the wall you
> got, instead of guessing from a bare line index.

The rows reference-check like connections: a feature naming a room the
plan does not hold, a `switch` whose `opens` matches no connection `id`
(an unnamed connection is refused by its `from->to` label), a row
missing its `room` (its `from_room`/`to_room`, or the switch's
`opens`), and a bad `kind`, `style` or `wall` name are all
`invalid_argument` naming `features[i]` — the vocabulary refusals list
the accepted names.

> **Playability warnings.** After the last feature installs, three
> checks read the finished map and their notes ride in `warnings` —
> never errors, always plan order: every `locked` door whose colour no
> placed thing answers as a key (a thing definition in the `Keys`
> category naming that colour — skull keys count, their names carry it)
> warns `"{colour} door at {connection id} but no {colour} key in the
> plan"`, one note per door in connection order; a plan with no `exit`
> row warns `"the map has no exit"`; and every `remote` door no
> `switch` row names warns `"remote door at {connection id} is opened
> by no switch: add a switch feature whose opens is {connection id}"`,
> again in connection order — an *unnamed* remote door, which no `opens`
> can reference, warns `"remote door at {from}->{to} is opened by no
> switch: give the connection an id and reference it in a switch
> feature's opens"` instead. The keys are what the map actually holds,
> so a card pinned by `type` counts like one named. Stand a key for
> every lock, set an exit somewhere, and switch every remote, and these
> three notes stay silent.

A whole playable map in one plan — six rooms, every feature kind, and
the card that opens it all:

```json
call build_map_from_plan {
  "map": "MAP12",
  "wall_thickness": 0,
  "rooms": [
    {"id": "start",    "rect": {"x": 0,    "y": 0,   "w": 128, "h": 128}},
    {"id": "corridor", "rect": {"x": 128,  "y": 0,   "w": 128, "h": 128}},
    {"id": "secret",   "rect": {"x": 256,  "y": 0,   "w": 128, "h": 128},
     "secret": true},
    {"id": "ledge",    "rect": {"x": 128,  "y": 128, "w": 128, "h": 128},
     "floor": 24},
    {"id": "exit",     "rect": {"x": 128,  "y": 256, "w": 128, "h": 128},
     "floor": 24, "light_effect": "strobe_slow"},
    {"id": "foyer",    "rect": {"x": -128, "y": 0,   "w": 128, "h": 128}}
  ],
  "connections": [
    {"id": "blue_door", "from": "start", "to": "corridor",
     "kind": "door", "style": "locked_blue"},
    {"id": "cell_door", "from": "corridor", "to": "secret", "kind": "door"},
    {"id": "gap",   "from": "corridor", "to": "ledge", "kind": "opening"},
    {"id": "gate",  "from": "ledge", "to": "exit", "kind": "door",
     "style": "remote"},
    {"id": "home",  "from": "start", "to": "foyer", "kind": "opening"}
  ],
  "features": [
    {"kind": "lift", "room": "ledge", "style": "use_wall", "low_floor": -40},
    {"kind": "teleporter", "from_room": "corridor", "to_room": "ledge"},
    {"kind": "switch", "room": "ledge", "opens": "gate", "wall": "east"},
    {"kind": "exit", "room": "exit", "style": "walk_over"}
  ],
  "things": [{"name": "Blue Keycard", "room": "start"}],
  "player_start": {"room": "start"}
}
-> {"map": "MAP12", "format": "doom",
    "rooms": {"start": [0], "corridor": [1], "secret": [2], "ledge": [3],
              "exit": [4], "foyer": [5]},
    "connections": [
      {"id": "blue_door", "kind": "door", "sectors": [6]},
      {"id": "cell_door", "kind": "door", "sectors": [7]},
      {"id": "gap", "kind": "opening", "sectors": [8]},
      {"id": "gate", "kind": "door", "sectors": [9]},
      {"id": "home", "kind": "opening", "sectors": [10]}],
    "features": [
      {"kind": "door", "room": null, "sectors": [6], "lines": [13, 14, 15, 18, 20, 26],
       "tag": 1},
      {"kind": "door", "room": null, "sectors": [7], "lines": [49, 50, 51, 54, 56, 62],
       "tag": 2},
      {"kind": "door", "room": null, "sectors": [9], "lines": [37, 38, 39, 40, 46, 47],
       "tag": 3},
      {"kind": "switch", "room": "ledge", "sectors": [9], "lines": [59], "tag": 3},
      {"kind": "lift", "room": "ledge", "sectors": [3], "lines": [22, 44], "tag": 4},
      {"kind": "teleporter", "room": "corridor", "sectors": [11, 12],
       "lines": [27, 28, 29, 41], "tag": 5},
      {"kind": "exit", "room": "exit", "sectors": [4], "lines": [24, 39, 40, 47, 48],
       "tag": null},
      {"kind": "light", "room": "exit", "sectors": [4], "lines": [], "tag": null},
      {"kind": "secret", "room": "secret", "sectors": [2], "lines": [], "tag": null}],
    "things_placed": 2,
    "bounds": {"x1": -128, "y1": 0, "x2": 384, "y2": 384},
    "warnings": [],
    "nodes": null, "validation": null, "…": "…"}
```

The card stands in `start` beside the player, so the blue `start →
corridor` lock opens; the `ledge` room's own sector becomes the
platform riding down to `-40` on a use of its south wall; the teleporter
flips corridor and ledge; the ledge's wall switch fires the remote
`gate`'s own tag to unlock the strobing `exit` room, whose every
two-sided boundary ends the level; and a plain door hides the `secret`
room. The `features` rows come back in pipeline order — the three doors
tagged `1`, `2` and `3`, the switch reusing the gate's `3`, the lift `4`,
the teleporter's destination `5` — then the exit and the two painted
room rows. The zero-thickness door bands meet their rooms along several
cuts and a later teleporter rebuild re-sorts the lines, so the doors'
and lift's lines are more than the two use faces and every row reports
the finished indices, facing the player as wired. The rows' `line_ends`
are left out of the abbreviated output above: each is the row's `lines`
again as `{index, x1, y1, x2, y2}` endpoint pairs in map units.

**Units and the grid**: room footprints, connection `at` positions,
teleporter `pad`/`destination` squares and the `at` point of an `exit`
or `switch` row are plan coordinates, drawn in whatever units you think
in (sketch
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
and `step`. The feature steps additionally consult `door_track_red`,
`door_track_blue` and `door_track_yellow` (the theme table falls them
back to `door_track`), plus `switch` and `exit_switch` for the mounted
panels (falling back to `wall` and `switch`). A lift's platform side
wears `step` (falling back to `wall`, the name the compiler's fill pass
already dressed the height gaps in); a teleporter square's bare new
edges fall back to `wall`. A feature role the theme
answers `"-"` leaves its faces as the compiler dressed them — a locked
door's jambs in the plain track, a switch panel on the wall's own
middle — and adds its own note. `theme`
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
ids, duplicate connection ids, unknown or self-joined rooms,
non-positive widths, out-of-range stair `step_height`, window trims
that fill the opening, too-short
boundaries…) already name the offending id; the tool prefixes them
with the wire index — `rooms[0]: room 'a' has a 0x64 rectangle; both
edges must be positive`. Feature rows name `features[i]`
(`features[0]: exit feature names unknown room 'ghost'`), and thing
rows fail the same way, naming `things[i]`, `things[i].room`,
`things[i].skills` or `player_start`.

**The result**: `rooms` maps each room id to the sector indices it
owns (a fully painted-over room holds no entry and warns about it);
`connections` reports `{id, kind, sectors}` per connection in plan
order (`id` is the connection's `id`, or its derived `from->to` label;
`sectors` holds the band sector, or the steps of a stair run, or nothing
at all for an `opening` wide enough to span its whole shared edge: that
one takes the wall down there and the rooms join directly, so it owns no
sector); `features` reports one `{kind, room, sectors, lines, tag,
line_ends}` row per installed game feature in pipeline order — doors first, in
connection order, each with `kind` `door`, `room` `null`, its band
`sectors`, the room-facing `lines` carrying the action (a `remote`'s
stay bare) and the shared `tag`; then switches (`kind` `switch`, the
room, its door's band sector, the mounted line and the door's tag);
then lifts (`kind` `lift`, the room, its platform sector, the
down-facing activation lines and the fresh tag); then teleporters
(`kind` `teleporter`, the pad room, the pad then destination sectors,
the pad's border lines and the destination's tag); then exits (`kind`
`exit`, the room and its sectors, the dressed lines and no tag); then
the light and secret rows (`kind` `light` / `secret`, the room, the
sectors painted, no lines and no tag) — a later teleporter rebuild
renumbers the map's lines, and every earlier row reports the finished
indices. `line_ends` repeats `lines` as
`{index, x1, y1, x2, y2}`: each reported linedef with the map-unit
coordinates of its two vertices **as the finished block holds them**,
read after every rebuild and re-facing the pipeline did, so a
wall-mounted panel's endpoints say which wall it took and where along it
it stands (a line whose vertices the map does not resolve is simply
left out). `things_placed` counts the
`THINGS` rows the plan asked for written, room fills
and the player start included (a teleporter's own type-14 destination
things ride along in the block beside them). `bounds` is the vertex box
`{x1, y1, x2, y2}`. `warnings` carries the compiler's advisories —
rooms unreachable from the first through the walkable connections (a
room joined only by windows counts as unreachable: glass is not a way
through), rooms painted over, floor gaps taller
than 24 units through an opening/door/window ("use stairs"), rooms that
fit fewer things than asked — plus the theme notes, the feature steps'
notes (a plan lift dropping under 64 units, the Boom secret-bit
warning for a lit `Doom` secret), the overlay notes of the drawn
teleporter squares and the playability notes (a locked door with no key
of its colour, no `exit` feature, a `remote` door no `switch` opens —
see the block above the six-room example). `nodes` and
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
      {"kind": "door", "room": null, "sectors": [3], "lines": [5, 17], "tag": 1,
       "line_ends": [{"index": 5, "x1": 232, "y1": -72, "x2": 232, "y2": -120},
                     {"index": 17, "x1": 248, "y1": -120, "x2": 248, "y2": -72}]}],
    "things_placed": 5,
    "bounds": {"x1": 0, "y1": -336, "x2": 432, "y2": 0},
    "warnings": ["red door at gate but no red key in the plan", "the map has no exit"],
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
validation came back clean. The two `warnings` are the playability
checks: this plan locks `gate` behind a red card it never places and
declares no exit — stand a red key in a `things` row and add an `exit`
row and they fall silent.

**After the call**: `render_topdown` compares the layout with the
sketch you worked from, `render_view` walks it first-person from the
placed player start, and `validate_map` judges the map. A block built
without `finalize` reports `nodes_missing`: play it in vanilla `Doom`
only after `build_nodes` / `build_all_nodes` ran (`reference/nodes.md`),
or build it with `finalize: true` from the start. Failed calls change
nothing: no block is written and no undo entry is recorded.
