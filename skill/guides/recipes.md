# Recipes

Tools: run_batch, bulk_edit, set_light_effect, edit_sector, auto_texture, align_textures, mark_secret, create_sector_from_loop, add_door, add_thing, add_switch, add_exit, add_stairs, add_lift, add_window, add_teleporter, place_things, replace_thing_type, replace_texture, replace_flat, create_texture_from_image, create_flat_from_image, create_sprite_from_image, import_music, import_sound, build_map_from_plan, check_playability, build_all_nodes, describe_map, set_texture_theme

Worked answers to the requests people actually make. Each recipe is the
request, the judgment behind it, and the exact calls. Read
`editing-existing-maps.md` for the loop these slots into;
`design-rules.md` for the numbers.

## The recipe format

Each fenced block below is machine-checkable. One JSON object:
`{"fixture": bool, "steps": [...]}` where `steps` has **exactly** the
shape `run_batch` takes (see `../reference/batch.md`; `{"$ref": "0/..."}`
chaining allowed), and `fixture: true` means the steps run unchanged on
the test fixture session: the `iwad-features` IWAD (a two-room `MAP01`:
sector 0 the hall, floor 0 light 160; sector 1 the raised back room,
floor 32 light 192; imp at (384, 128), start at (64, 128); flats
`FLOOR1`/`FLAT10`; textures `WALL1`/`STEP1`/`DOOR1`/`DOORTRAK`/
`DOORRED`/`DOORBLU`/`DOORYEL`/`SW1STRTN`/`SW1EXIT`) plus an empty
working WAD.
File sources are the placeholders `"<IMAGE>"` (a PNG), `"<MIDI>"` (a
type-0 MIDI) and `"<WAV>"` (a PCM WAV). Recipes written against the
fixture name sector 0 and sector 1 — in your own map, resolve "the big
room" with `describe_map` first and substitute.

```json recipe
{"fixture": true, "steps": [{"tool": "bulk_edit", "arguments": {"kind": "sectors", "within": {"sectors": [1]}, "add": {"light": -48}}}]}
```

## Lighting and heights

### Darken a room

**"Make the big room darker."** Sector 0 is the fixture's hall. Take
48 off the light, not to a fixed number, so relative brightness survives;
below 64 stops reading as "dim" and starts reading as "pitch black".

```json recipe
{"fixture": true, "steps": [{"tool": "bulk_edit", "arguments": {"kind": "sectors", "within": {"sectors": [0]}, "add": {"light": -48}}}]}
```

### Brighten a room

**"Light that room up."** Sector 1 is 192; +32 lands on 224. `bulk_edit`
refuses the whole call if any row would leave `0..=255`, so an unknown
spread goes `dry_run` first.

```json recipe
{"fixture": true, "steps": [{"tool": "bulk_edit", "arguments": {"kind": "sectors", "within": {"sectors": [1]}, "add": {"light": 32}}}]}
```

### Flickering lights

**"Make the lights flicker in there."** `set_light_effect` is the vanilla
sector special: `flicker` for candles and faulty tubes, `glow` for slow
breathing, `strobe_fast` for alarms. The `light` you pass is the level it
flashes around.

```json recipe
{"fixture": true, "steps": [{"tool": "set_light_effect", "arguments": {"sectors": [0], "effect": "flicker", "light": 176}}]}
```

### Raise a ceiling

**"The hall feels claustrophobic."** Sector 0 is 128 tall; 192 opens it.
The two-sided line against sector 1 now shows an upper band — the
`auto_texture` step dresses the empty gap so it is not a hall of mirrors.

```json recipe
{"fixture": true, "steps": [{"tool": "edit_sector", "arguments": {"index": 0, "ceiling_height": 192}}, {"tool": "auto_texture", "arguments": {"mode": "fill_missing", "sectors": [1]}}]}
```

### Make a room outdoors

**"I want to see the sky from the back room."** The session's sky-flat
ceiling is the sky in every engine — `F_SKY1` in DOOM, DOOM II, Heretic
and Strife, `F_SKY` in Hexen — and `bulk_edit` allows that name even
though no WAD carries it. Give the room real height, and drop a wall's
ceiling for a lip the sky sits behind.

```json recipe
{"fixture": true, "steps": [{"tool": "bulk_edit", "arguments": {"kind": "sectors", "within": {"sectors": [1]}, "set": {"ceiling_flat": "F_SKY1"}}}]}
```

## Secrets

### Add a secret room off a room

**"Add a secret room off the hall."** Draw it north of the hall (the
fixture map spans `0..512 x 0..256`, so `y 256..512` is empty field),
cut a normal door into it, mark it secret, rebuild the nodes. The room
stays off the automap; players find it through the door.

```json recipe
{"fixture": true, "steps": [{"tool": "create_sector_from_loop", "arguments": {"points": [{"x": 0, "y": 256}, {"x": 256, "y": 256}, {"x": 256, "y": 512}, {"x": 0, "y": 512}], "light": 96, "floor_flat": "FLOOR1", "ceiling_flat": "FLOOR1", "wall_texture": "WALL1"}}, {"tool": "add_door", "arguments": {"between": [0, 2]}}, {"tool": "mark_secret", "arguments": {"sectors": [2]}}, {"tool": "build_all_nodes", "arguments": {}}]}
```

### Mark a sector secret

**"That little closet should be secret."** `mark_secret` edits only the
sector special — geometry, textures and neighbours are untouched, so no
node rebuild. `secret: false` clears it again.

```json recipe
{"fixture": true, "steps": [{"tool": "mark_secret", "arguments": {"sectors": [1]}}]}
```

## Doors, switches and exits

### Add a normal door between two rooms

**"Put a door between the two rooms."** `add_door` carves the recess,
wires the use action and dresses the jambs in one step. On the fixture
the back room sits 32 above the hall, so pair the door with the stairs
recipe for full walkability.

```json recipe
{"fixture": true, "steps": [{"tool": "add_door", "arguments": {"between": [0, 1]}}, {"tool": "build_all_nodes", "arguments": {}}]}
```

### Locked blue door with a key

**"Blue-key door, key in the hall."** The locked style wears the blue
track; the key is thing 5 (Blue Keycard). Vanilla order matters: a door
with no reachable key is a map that cannot be finished — put the key on
this side of the door.

```json recipe
{"fixture": true, "steps": [{"tool": "add_door", "arguments": {"between": [0, 1], "style": "locked_blue"}}, {"tool": "add_thing", "arguments": {"type": 5, "x": 128, "y": 64, "angle": 0}}, {"tool": "build_all_nodes", "arguments": {}}]}
```

### Switch-operated door

**"A button on the wall opens that door."** Build the door `remote`: the
recess closes, the band sector takes a fresh tag, nothing is wired yet.
`add_switch` then mounts a panel that fires `open_door` on that sector —
`$ref` carries the door's own result numbers into the switch call.

```json recipe
{"fixture": true, "steps": [{"tool": "add_door", "arguments": {"between": [0, 1], "style": "remote"}}, {"tool": "add_switch", "arguments": {"linedef": {"$ref": "0/lines/0"}, "action": "open_door", "target_sector": {"$ref": "0/door_sector"}}}, {"tool": "build_all_nodes", "arguments": {}}]}
```

### Exit switch

**"Add an exit switch on the east wall."** Linedef 5 of the fixture map
is that wall (256 long, front side in sector 1). The panel is split out
at 32 wide and wears the theme's `exit_switch` texture.

```json recipe
{"fixture": true, "steps": [{"tool": "add_exit", "arguments": {"kind": "switch", "linedef": 5}}, {"tool": "build_all_nodes", "arguments": {}}]}
```

### Walk-over exit

**"Crossing the threshold should end the level."** `walk_over` dresses
every plain two-sided boundary of the sector with a W1 exit action. A
boundary line that already carries another feature's action (a door's
use line, a lift trigger, a teleporter pad border) keeps it and comes
back in `skipped_lines` with a warning; a sector whose every boundary
line is busy is refused — mount the end on a wall with `kind: "switch"`
or open a plain line instead. Tag 0 ends the game; set a destination map
with `edit_linedef` where the port supports one.

```json recipe
{"fixture": true, "steps": [{"tool": "add_exit", "arguments": {"kind": "walk_over", "sector": 1}}]}
```

## Movement

### Stairs between two floors

**"Stairs up to the back room."** `from` must stand in the low area,
`to` in the high one (fixture hall at (64, 128), raised room at
(448, 128)). Steps rise by `step_height` 16 each — 24 is the player's
maximum and 16 is what reads as stairs.

```json recipe
{"fixture": true, "steps": [{"tool": "add_stairs", "arguments": {"from": {"x": 64, "y": 128}, "to": {"x": 448, "y": 128}}}, {"tool": "build_all_nodes", "arguments": {}}]}
```

### Lift to a ledge

**"A lift up to the ledge."** `add_lift` on the platform sector turns
its low-facing edges into trigger lines and wires a fresh tag; the ride
starts by pressing the wall from below. Drops under 64 get the "stairs
will do" note — this fixture ledge is 32 up, which is exactly that case;
real lifts want more.

```json recipe
{"fixture": true, "steps": [{"tool": "add_lift", "arguments": {"sector": 1, "style": "use_wall"}}]}
```

### Window between rooms

**"Glaze the wall between the rooms."** A window is a raised, lowered
band you can see through but not walk: sill 32 over the higher floor,
lintel 24 under the lower ceiling. Under 16 units of glass the call
refuses and nothing is drawn.

```json recipe
{"fixture": true, "steps": [{"tool": "add_window", "arguments": {"between": [0, 1], "sill": 32, "lintel": 24}}, {"tool": "build_all_nodes", "arguments": {}}]}
```

### Teleporter pair

**"Teleport from the hall into the back room."** Two squares through the
overlay engine: the pad you step on and the destination you land in,
wired with a fresh tag (a `Doom`-family type-14 destination thing, or a
`Hexen`-family TID). `two_way: true` gives it a return leg.

```json recipe
{"fixture": true, "steps": [{"tool": "add_teleporter", "arguments": {"pad": {"x": 64, "y": 64}, "destination": {"x": 384, "y": 64, "angle": 90}}}, {"tool": "build_all_nodes", "arguments": {}}]}
```

## Things

### Add N monsters to a room

**"Put three demons in the back room."** `place_things` computes the
positions: a clearance grid over the sector, wall and body spacing
handled, centre-outward spread. The result is `placed`/`requested` plus
the indices — if the room cannot hold its share you get a warning, not
an error, and fewer things.

```json recipe
{"fixture": true, "steps": [{"tool": "place_things", "arguments": {"thing": "demon", "sectors": [1], "count": 3}}]}
```

### Harder on hard skill only

**"Save the baron for UV."** `skills: ["hard"]` sets only the hard
presence bit; `ambush` wakes him when the player is close. Check it by
walking the map twice: `check_playability` with `skill: "medium"` should
not see this thing at all.

```json recipe
{"fixture": true, "steps": [{"tool": "place_things", "arguments": {"thing": "baron", "sectors": [1], "count": 1, "skills": ["hard"], "ambush": true}}]}
```

### Swap monster types

**"Swap all the imps for demons."** `replace_thing_type` resolves names
through the session's definitions ("imp" is 3001, "demon" is 3002) and
rewrites every match in one undoable step, positions and flags byte-
identical. `dry_run` it first to count the swap.

```json recipe
{"fixture": true, "steps": [{"tool": "replace_thing_type", "arguments": {"from": "imp", "to": "demon"}}]}
```

## Textures, flats and sprites

### Replace a texture everywhere

**"Every STARTAN-style wall should be this photo."** A whole-wall sweep
is `replace_texture`, not a `bulk_edit` walk: it folds every map you
select into one undoable step. `maps: "all"` takes the merged view's
blocks; run a `dry_run: true` pass first when the glob is wide.

```json recipe
{"fixture": true, "steps": [{"tool": "create_texture_from_image", "arguments": {"source": "<IMAGE>", "name": "PHOTO1", "width": 128, "height": 128, "tileable": true}}, {"tool": "replace_texture", "arguments": {"from": "WALL1", "to": "PHOTO1", "maps": "all"}}]}
```

### Texture from a photo onto one room's walls

**"Use my photo on the walls of the start room."** One call commits the
finished texture (patch, `PNAMES`, definition), `bulk_edit` applies it
to exactly the sides of sector 0 — the fixture's start room — still
wearing `WALL1`, so the step line's clear glass face is left alone.

```json recipe
{"fixture": true, "steps": [{"tool": "create_texture_from_image", "arguments": {"source": "<IMAGE>", "name": "PHOTO2", "width": 128, "height": 128, "tileable": true}}, {"tool": "bulk_edit", "arguments": {"kind": "sidedefs", "where": [{"field": "middle", "op": "eq", "value": "WALL1"}], "within": {"sectors": [0]}, "set": {"middle": "PHOTO2"}}}]}
```

### Floor from a photo

**"Floor the hall with my photo."** `create_flat_from_image` lands on the
64×64 vanilla tile (pass `size` for a 128 or 256 square a ZDoom-family
port reads), seams blended. Apply it to sector 0's floor; `ceiling_flat`
would dress the ceiling instead.

```json recipe
{"fixture": true, "steps": [{"tool": "create_flat_from_image", "arguments": {"source": "<IMAGE>", "name": "PHOTOFL1", "tileable": true}}, {"tool": "bulk_edit", "arguments": {"kind": "sectors", "within": {"sectors": [0]}, "set": {"floor_flat": "PHOTOFL1"}}}]}
```

### New decoration sprite from a drawing

**"Here's a torch I drew — put it in."** `create_sprite_from_image`
strips the border background, trims, scales and files the lump; naming
it over an existing thing's sprite (`SMRT`, the short red torch, type
57) makes the decoration spawn with the vanilla definition and your
art. A brand-new thing type needs a definition: `edit_dehacked` or
`edit_decorate` (`images-to-assets.md` owns that choice).

```json recipe
{"fixture": true, "steps": [{"tool": "create_sprite_from_image", "arguments": {"source": "<IMAGE>", "sprite": "SMRT", "frame": "A", "rotation": 0, "height": 32}}, {"tool": "add_thing", "arguments": {"type": 57, "x": 96, "y": 64, "angle": 0}}]}
```

### Theme repaint and alignment

**"Re-dress the map in the brick theme."** Set the theme with
`set_texture_theme` (`themes.md`; session state, not an undoable edit),
then `auto_texture` `retexture` repaints one-sided middles, bands and
flats from the theme's roles while door actions keep their dressing.
`align_textures` runs the picture continuously across corners.

```json recipe
{"fixture": true, "steps": [{"tool": "auto_texture", "arguments": {"mode": "retexture"}}, {"tool": "align_textures", "arguments": {}}]}
```

### Fix chopped wall textures

**"The brick pattern jumps at every corner."** `align_textures` on the
room rewrites `x_offset` along drawing-order chains modulo the texture
width; `y: true` also pegs the tops to the chain's highest ceiling.
Offset edits keep the nodes.

```json recipe
{"fixture": true, "steps": [{"tool": "align_textures", "arguments": {"sector": 0, "y": true}}]}
```

### Swap all one flat for another

**"All the lava floors back to concrete."** The name-swap twin of
`replace_texture` for sector surfaces: glob `from`, `surfaces` limits it
to floors so liquid skies stay put. Written for a stock DOOM WAD
(`NUKAGE1..3` → `FLOOR4_8`); not fixture-runnable, the test fixture has
no lava. Run the `dry_run` pass to count before the real one.

```json recipe
{"fixture": false, "steps": [{"tool": "replace_flat", "arguments": {"from": "NUKAGE*", "to": "FLOOR4_8", "surfaces": ["floor"], "maps": "all"}}]}
```

## Music and sound

### Replace the level music

**"Use my tune for MAP01."** Vanilla plays the `MUS` lump `D_<slot>` per
map (`D_RUNNIN` is DOOM II's MAP01); `import_music` converts a type-0
MIDI to MUS on the way in. Name the lump for the map you want it under,
or `MAPINFO` it. For OGG/MP3 keep the file as-is with
`format: "passthrough"` and play it from a ZDoom-family port.

```json recipe
{"fixture": true, "steps": [{"tool": "import_music", "arguments": {"source": "<MIDI>", "name": "D_RUNNIN"}}]}
```

### Replace a sound

**"Swap the pistol shot for this recording."** Vanilla plays lump
`DS<name>` for sound `<name>` — `DSPISTOL` is the pistol fire.
`import_sound` decodes any WAV/OGG/MP3/FLAC and stores the 8-bit DMX
form; check the result by ear with `preview_sound {"name": "DSPISTOL",
"scope": "working"}`.

```json recipe
{"fixture": true, "steps": [{"tool": "import_sound", "arguments": {"source": "<WAV>", "name": "DSPISTOL"}}]}
```

## Whole maps

### Start a new map from a description

**"Build me a small level: entry hall, big room with a door, exit in the
back."** No existing map to read — write the plan and `build_map_from_plan`
compiles it: rooms by rectangle, one `door` connection across the shared
wall, an `exit` feature on the hall's back wall (`switch` is the default
style; a `walk_over` exit would need a boundary line the door does not
own, since it leaves a dressed line to its own feature), things by name,
and `finalize` builds the nodes and runs the checks inside the same edit.
The coordinates here are the sketch-guide example's proportions without
the sketch.

```json recipe
{"fixture": true, "steps": [{"tool": "build_map_from_plan", "arguments": {"map": "MAP02", "rooms": [{"id": "entry", "rect": {"x": 0, "y": 0, "w": 128, "h": 128}, "light": 128}, {"id": "hall", "rect": {"x": 128, "y": 0, "w": 256, "h": 192}, "ceiling": 192, "light": 192}], "connections": [{"id": "gate", "from": "entry", "to": "hall", "kind": "door", "style": "normal", "at": [128, 64]}], "features": [{"kind": "exit", "room": "hall", "wall": "east"}], "things": [{"name": "shotgun", "room": "entry"}, {"name": "imp", "room": "hall"}], "player_start": {"room": "entry", "angle": 0}, "finalize": true}}]}
```

### Fix "can't reach the exit"

**"I can't finish this map."** Diagnose before you cut: the first
`check_playability` names the blocker and its reason (the fixture's back
room is 32 up — `step_too_high`, no exit at all). `add_stairs` answers
the height, `add_exit` the destination; the second check with
`render: true` is the picture you send back to the user.

```json recipe
{"fixture": true, "steps": [{"tool": "check_playability", "arguments": {"map": "MAP01", "skill": "medium", "render": false}}, {"tool": "add_stairs", "arguments": {"from": {"x": 64, "y": 128}, "to": {"x": 448, "y": 128}}}, {"tool": "add_exit", "arguments": {"kind": "walk_over", "sector": 1}}, {"tool": "build_all_nodes", "arguments": {}}, {"tool": "check_playability", "arguments": {"map": "MAP01", "render": true}}]}
```
