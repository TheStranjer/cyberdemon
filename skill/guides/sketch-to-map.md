# Sketch to map

Tools: open_iwad, new_wad, get_asset_server_info, trace_sketch, build_map_from_plan, validate_map, check_playability, render_topdown, render_view, describe_map, undo, add_stairs, edit_sector, add_thing, add_door, edit_decorate, save_wad

Turn a hand-drawn floor plan into a playable map. The server measures the
drawing and builds exactly what the plan says; every reading of the drawing
— which blob is the armour room, which break is the red door, what the "X"
means — is yours. `guides/design-rules.md` holds the dimensions to design
against; `guides/verification.md` is the loop you finish with.

## 1. Who decides what

| You decide | The tools do |
|------------|--------------|
| Which regions are rooms and which is junk (hatching, coffee rings) | Measure ink: regions, gaps, marks, adjacency (`trace_sketch`) |
| What each mark means: monster, item, key, player, pillar | Grow walls, floors and sector data from your room shapes (`build_map_from_plan`) |
| Scale: how many map units one pixel is | Wire doors, stairs, lifts, switches, teleporters and exits |
| Floor and ceiling heights, light levels, texture theme | Check structure (`validate_map`) and play (`check_playability`) |
| What the user meant ("cosy" → low ceiling, 128 light) | Draw the result to look at (`render_topdown`, `render_view`) |
| Whether the built map answers the drawing | Nothing — comparing pictures to intent is yours |

The tools never guess: `trace_sketch` names its findings `R1`, `G1`, `M1` —
measurements, never meanings.

## 2. Get the image as a file

Every image tool takes a `source`: an absolute path, a `file://` URI, an
`http(s)://` URL, or an upload URL on the server's own asset origin. Pixels
you can see in the conversation are not a source — you cannot pass an image
you only see.

- Ask the user for the file's absolute path, or to save the picture and give
  you the path.
- If they can only hand you bytes, call `get_asset_server_info` for the
  upload URL and `PUT` the file there; the receipt's URL is a valid `source`.
- A photograph at an angle will not trace well: ask for a flat scan. The
  tools do not correct perspective.

Start the editing session while you are at it: `open_iwad` the game archive
(themes, definitions and checks read it) — a `new_wad` gives the work a
home. `trace_sketch` itself needs no open WAD.

## 3. Read the sketch yourself first

Before any tool call, write down your reading of the drawing:

- **Rooms**, with your own names: `entry`, `great_hall`, `loft`. Sizes in
  pixels, as you measure them on the image.
- **Door marks**: gaps in a wall, a quarter-arc swinging across an opening,
  a tick across the break, the letter "D". A wide plain break with no symbol
  is an opening, not a door.
- **Stairs**: an arrow with steps drawn on it, parallel tread lines, "up",
  "DN", a number.
- **Heights and labels** written on the sheet: "16" beside a line,
  "CEIL 256", "+64", a note in a corner.
- **A legend for the marks**: X, dots, letters, colours. If there is no
  legend, use position and count: a lone X in a small room near the map
  edge is the player start; a scatter of dots in a big room reads as
  monsters; a ring is a pillar. Say what you assumed.
- **The player start and the exit.** If the drawing marks neither, ask —
  these two are genuinely ambiguous and the map fails without them.

Write your interpretation down before calling tools. Ask the user only
about the key points you cannot default — where the exit is, what a symbol
means, which side is north. Everything else has a conservative default
below; mention the assumption and move on.

## 4. Choose the scale

The `units` object maps plan pixels to map units: `x` becomes
`origin_x + x * scale_num / scale_den`, and `y` is mirrored when
`flip_y: true`.

- **Dimensions given?** Compute exactly. A sketch on grid squares of
  40 pixels that must be 64 units each: `64 / 40 = 8 / 5`, so
  `scale_num: 8, scale_den: 5` — 40 px × 8 ÷ 5 = 64 units. A written room
  width ("hall is 512 wide" next to a 160-pixel room) gives the same
  fraction: `512 / 160 = 16 / 5`.
- **Nothing given?** Measure the typical room's shorter side in pixels and
  pick the fraction so it lands at **256–384 units**; keep corridors at
  least 96 units wide and doors at least 64. A room drawn 128 px short-side
  wants about 2 units per pixel: `scale_num: 2, scale_den: 1`.
- **Always `flip_y: true` for images** — rows grow down, map `y` grows up.
- **`grid: 16`** snaps everything to the design grid (10 px at
  `8/5`, 8 px at `2/1`).

Use the **same** `units` and `grid` object in every call — `trace_sketch`,
`build_map_from_plan` and the `render_topdown` underlay. That is what makes
the built map land exactly on the drawing.

## 5. Trace

Call `trace_sketch` with `source`, your `units` and `grid: 16`. Then open
the `debug` image URL and compare it with the sketch: every region a flat
colour with an `R` id, every gap a magenta circle, every mark a cyan box.

Check the picture against your list:

- every room is one region (not two, not merged);
- every door gap has a `G` circle on it;
- your marks have `M` boxes (small marks may be dropped — see the table).

Fix what does not match, then trace again:

| The debug image shows | Change |
|-----------------------|--------|
| Two rooms merged into one region | Raise `close_gaps` — the wall between them is broken and needs a wider bridge |
| A room missing | Lower `min_region_area` — it was dropped as too small |
| Many tiny regions from writing or shading | Raise `min_region_area`, or raise `mark_max_size` so scribbles count as marks, not regions |
| Wall strokes missed (pencil too light) | Set `ink.level` (a luminance `0..=255`; raise it until light graphite counts as ink; `threshold` echoes what was used) |
| Colour-coded drawing (red walls only, say) | `ink: {"mode": "color", "color": "#FF0000"}` — trace each colour separately |
| A `G` circle hugging the sheet's frame, `between` `null` | Lower `close_gaps` — the radius is bridging walls to the image edge; keep every wall at least `close_gaps` px clear of the frame |
| Rooms split by a stray light pencil line | Lower `ink.level` until only the real walls count as ink; if the split stays, merge the two regions by hand in the plan |

Zero regions is not an error: read `warnings`, adjust, trace again. The
regions' `points` come back in original-image pixels and, with `units`, in
`map_points` too; `plan_rooms` is ready-made for the next step.

**A noisy scan is bounded, not broken.** A photo of ruled paper measures
hundreds of bridges and speckles, so `gaps` and `marks` arrive largest first,
at most 50 rows each: `gaps_total` and `marks_total` say what the analysis
found, `truncated` says a cap left rows out, and `warnings` names every
omitted count with the parameter that lifts it. Plan from the rows you get.
When you need the long tail raise `max_gaps`/`max_marks`, and when the tail
is only dust raise `noise_floor_px` (default the measured `stroke_px`) past
it; `verbose: true` returns every row and can outgrow your client's result
limit. The `debug` image labels only the reported rows, so the picture and
the lists never disagree.

## 6. Write the plan

Start from `plan_rooms`: keep each room's `points` as they are, rename the
`id` to your room name. Then:

- **Heights.** Defaults are `floor: 0`, `ceiling: 128`. A room marked "up"
  gets a floor of +32 to +128 — reach it with `stairs` (a `connections`
  row) or a `lift` (a `features` row) if the drop is 64 or more. Big halls:
  `ceiling: 192` to `256`. A courtyard: `outdoor: true` (its ceiling
  becomes the sky).
- **Light.** Default 160; 128 for a dungeon, 96 for caves, 255 outdoors.
- **Connections.** Every gap in the walls becomes one row, at the gap's
  `center` (plan pixels), with `width` = the gap's `width_px` through
  `units`, rounded to a multiple of 16 and **at least 64**:
  - a door mark, an arc, a symbol, or the user said "door" →
    `{"kind": "door", "style": ...}` (`normal`, `fast`, `stay_open`,
    `locked_red` / `locked_blue` / `locked_yellow`, `remote`);
  - a plain wide break → `{"kind": "opening"}`.
  Add `stairs` where two rooms' floors differ by more than 24 — even where
  the sketch shows no steps; 24 is the player's maximum step-up. Add
  `window` where glass is drawn.
- **Things.** Convert marks through the legend into `things` rows:
  `{"name": "imp", "room": "great_hall", "count": 2}`,
  `{"name": "shotgun", "x": 40, "y": 88}`, `{"name": "medikit", ...}`.
  Names resolve against the loaded game's definitions; an ambiguous name
  errors with the candidate list — pick one.
- **Player start.** `player_start: {"room": "entry", "angle": 0}` (angles:
  0 east, 90 north, 180 west, 270 south).
- **Exit.** A `features` row:
  `{"kind": "exit", "room": "loft", "style": "walk_over"}` for an E boxed
  on a room, or the default `switch` panel — which takes a `wall` side and
  the plan `at` point where your mark stands, so the panel lands on the
  wall you drew it on: `{"kind": "exit", "room": "loft", "wall": "west",
  "at": [104, 12]}`. `at` also centres the panel; without it the longest
  matching wall wins, and an exact tie falls to the lowest line index,
  which is where several equal walls on one side become a coin flip you
  cannot see. Read the row's `line_ends` back: they are the panel's own
  endpoints, and they should bracket your mark.
- **Keys before locks.** Every `locked_*` door needs a thing of that
  colour standing in a room the player reaches first:
  `{"name": "Red Keycard", "x": 32, "y": 48}`. The build warns about a
  lock with no key; the walk stops at it.
- **Theme.** Pick from the user's words with `theme`: `tech`, `hell`,
  `brick` or `rock` for the DOOM games (textures come from the theme — a
  plan never names textures unless asked to).

## 7. Build

Send the whole object to `build_map_from_plan` with your `map` name, the
same `units` and `grid`, and `finalize: true` — one call compiles rooms,
walls, connections, features, things and node data as one undoable edit.

Read the result, not your memory of the input:

- `rooms` maps your ids to sector indices; `features` lists every door,
  switch, lift, teleporter and exit with its sectors, lines and tag, and
  each line's `line_ends` coordinates beside its index — the check that a
  wall-mounted panel landed where the drawing put it.
- `warnings` are judgment gaps: "red door at X but no red key in the
  plan", "the map has no exit", "room 'y' fits only N of M things",
  "step taller than 24 between a and b; use stairs". Each names its fix;
  a remote door wants a `switch` row.
- `validation` (with `finalize`) carries structural errors only — fix
  those with the element tools.

Rebuilding is cheap: edit the JSON, send again with `replace: true`, and
one `undo` takes the whole block back if you need it.

## 8. Verify and iterate

1. **`render_topdown`** with the sketch under the map:
   `{"map": ..., "underlay": {"source": "<the sketch>", "units": <the same
   units>}, "labels": {"sectors": true}}`. Open the asset: the built walls
   should sit on the drawn lines, room for room. A shifted map means the
   `units` or `flip_y` drifted between calls — fix the parameters, not the
   pixels by hand.
2. **`check_playability`** with `map` — `ok: true` is the bar. When false,
   work `blockers` top to bottom: every row names the reason and the tool
   that fixes it (`add_stairs`, `edit_sector`, `add_thing`, `add_door`).
   `render: true` draws the unreachable sectors red. Run it again after
   each fix.
3. **`render_view`** from the player start: `{"x": ..., "y": ..., "angle":
   ...}` at the start's coordinates and facing. Walk the map in pictures —
   the door, the stairs, the key, the exit should each be visible or
   findable the way you planned. `problems` names missing textures.
4. **Small fixes** go through the element and feature tools on the sector
   indices from step 1 (`edit_sector` on "sector 7 is too dark").
   `describe_map` answers "which room has the shotguns?" when you need
   indices again. **Layout fixes** go into the plan JSON and rebuild with
   `replace: true`.

Iterate until `ok: true` and the pictures match the drawing; then re-read
the original request — dimensions, mood, contents — against the renders,
not against your parameters.

## 9. Save

`save_wad` with an absolute path (default: the working archive's path — a
`new_wad` has none, so pass one). Tell the user where the file is and what
it needs to play:

- `format: "doom"` (the default) with `finalize: true`: vanilla `Doom`
  and `DOOM II` read it.
- `format: "hexen"` / `"udmf"`, a `DECORATE` actor from `edit_decorate`,
  PNG lumps or a new `PLAYPAL`: name the feature and recommend a
  ZDoom-family port.

## 10. Worked example

The drawing: a pen sketch on 32-pixel grid squares, 3 rooms. An **entry**
(3×3 squares) with an "X" and a note "key here"; a gap in its east wall
with a quarter-arc and "red"; a **great hall** (4×3 squares) with one dot
(the legend says "dots are imps"); a step-marked gap in the wall the hall
shares with a **loft** (2×2 squares, "UP 64" written beside it, an "E"
boxed in its far corner). The user says each square is 64 units, and wants
it "hell-flavoured".

The reading: entry + hall + loft; the arc is a `locked_red` door; the
step-marked gap is `stairs` to floor 64; X = player start, the note = a
red key in the entry, one dot = one imp in the hall, the boxed E = a
`walk_over` exit in the loft. Scale: 64 units per 32 px → `scale_num: 2`;
`flip_y: true`, `grid: 16`.

The plan (the trace's `plan_rooms`, renamed, with heights, connections,
features and things added):

```json plan-example
{
  "map": "MAP02",
  "format": "doom",
  "theme": "hell",
  "units": {"scale_num": 2, "scale_den": 1, "flip_y": true},
  "grid": 16,
  "wall_thickness": 16,
  "rooms": [
    {"id": "entry", "points": [[0, 0], [96, 0], [96, 96], [0, 96]],
     "light": 128},
    {"id": "hall", "points": [[96, 0], [224, 0], [224, 96], [96, 96]],
     "ceiling": 192, "light": 192},
    {"id": "loft", "points": [[96, 96], [160, 96], [160, 160], [96, 160]],
     "floor": 64, "ceiling": 192, "light": 96}
  ],
  "connections": [
    {"id": "red_gate", "from": "entry", "to": "hall", "kind": "door",
     "style": "locked_red", "at": [96, 48], "width": 64},
    {"id": "steps", "from": "hall", "to": "loft", "kind": "stairs",
     "at": [128, 96], "width": 96, "step_height": 16}
  ],
  "features": [
    {"kind": "exit", "room": "loft", "style": "walk_over"}
  ],
  "things": [
    {"name": "Red Keycard", "x": 16, "y": 48},
    {"name": "imp", "room": "hall"}
  ],
  "player_start": {"room": "entry", "angle": 0},
  "finalize": true
}
```

All plan coordinates are pixels from the sketch image; `units` turns them
into a 448 × 320 map (`x 0..448`, `y -320..0`), `grid: 16` snaps every
corner. The `red_gate` sits on the entry/hall shared wall centred at
pixel `(96, 48)` = map `(192, -96)`; `steps` climbs 0→64 in four 16-unit
treads. The key stands west of the player, the imp fills the hall, and the
walk-over boundaries of the loft end the level. `finalize: true` built the
node run into the block, so this map plays in vanilla Doom as written —
`check_playability` answers `ok: true` (the key is reachable, the door
opens on it, the exit is past the stairs), and `save_wad` ships it.
