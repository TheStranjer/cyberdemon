# Editing existing maps

Tools: open_iwad, open_wad, new_wad, merge_wad, list_maps, set_active_map, get_session_info, describe_map, find_elements, get_map_summary, edit_sector, add_linedef, edit_sidedef, bulk_edit, replace_texture, replace_flat, replace_thing_type, place_things, add_door, add_window, add_stairs, add_lift, add_teleporter, add_switch, add_exit, mark_secret, set_light_effect, create_sector_from_loop, carve_void, build_map_from_plan, auto_texture, align_textures, create_texture_from_image, create_flat_from_image, create_sprite_from_image, import_music, import_sound, run_batch, build_all_nodes, validate_map, check_playability, render_topdown, render_view, undo, diff_wads, save_wad

Most requests are not "build me a level" but "change this one thing in
that map": darken a room, add a secret, swap the imps for demons, put the
user's photo on a wall, replace the music. Six steps, in this order, every
time. `guides/recipes.md` holds worked one-request examples for step 3.

## 1. Orient

1. `open_iwad` the game archive the project runs against. The IWAD is a
   read-only base layer: definitions, themes, stock textures and every
   check read from it.
2. Load the user's own file. `open_wad` adopts an existing WAD as the
   **working archive** — the one editable layer; every edit lands there,
   and only `save_wad` ever writes it back to disk. `new_wad` starts a
   fresh project WAD. `merge_wad` with `mode: "layer"` (the default)
   stacks extra WADs **read-only** when you only need their textures or
   sounds as resources.
3. `list_maps` to see which map blocks exist and which archive holds each;
   `get_map_summary` gives one map's cheap counts, bounds and node state
   when that is all you need. With more than one map, `set_active_map` so
   the `map` parameter can stay omitted afterwards. `get_session_info`
   answers the same questions plus where the working archive points and
   how far `undo` reaches.
4. `describe_map` on the map you are about to edit and **open the labelled
   image it publishes** (`render: true` is the default). The per-sector
   rows carry area, bbox, heights, `light`, flats, `roles`,
   `wall_textures`, `neighbours` and `things` counts; the `summary` carries
   the whole-map facts. From here on, talk about the map in sector numbers
   that are also visible in the picture.

## 2. Find what the user means

Turn the words into indices before touching anything:

- "the big room" → `describe_map`'s `summary.largest_sectors` (the five
  biggest by area).
- "the room with the shotgun" → the row whose `things` count a Shotgun.
- "the start" → `summary.player_starts`, or the `start` sector that
  `check_playability` reports.
- "the dark corridor" → rows with a low `light` and a long thin `bbox`
  (a small `area` in a wide box).
- "all the lava" → `find_elements` with `kind: "sectors"` and
  `where: [{field: "floor_flat", op: "glob", value: "LAVA*"}]`.
- "every wall wearing STARTAN" → `find_elements` with `kind: "sidedefs"`
  and a `middle`/`upper` filter.

When two candidates fit equally well, do not guess: point at the labelled
image and ask — "the dark room — sector 12 or 15?" A wrong index applied
map-wide is worse than one extra question.

Say the numbers back before acting: "the big room is sector 12 (148×96,
light 96) — darken it by how much?" The user can correct a sector index;
they cannot correct a silent guess.

## 3. Pick the tool

One intent, one high-level call. Only fall to the element tools
(`edit_sector`, `edit_sidedef`, `add_linedef`, ...) for a single-element
fix a bulk tool cannot express.

| The user says... | Call |
|------------------|------|
| "make this room darker / bigger / tag everything 3" | `bulk_edit` on `sectors` (`add: {light: -48}` shifts the current value; `set` replaces it) |
| "every lava room darker" | `bulk_edit` with the `find_elements` query as its selection |
| "use BRICK7 instead of STARTAN everywhere" | `replace_texture` (glob `from`, `parts`, `maps: "all"`) |
| "all the nukage floors back to concrete" | `replace_flat` (`surfaces: ["floor"]` keeps sky ceilings) |
| "turn every imp into a demon" | `replace_thing_type` (names resolve: `from: "imp", to: "demon"`) |
| "put some monsters in the hall" | `place_things` (thing by name, `sectors`/`bbox`/`near`, `count` or `per_sector`) |
| "add a door / window / stairs / lift / teleporter / switch / exit here" | `add_door`, `add_window`, `add_stairs`, `add_lift`, `add_teleporter`, `add_switch`, `add_exit` |
| "make it a secret" | `mark_secret` |
| "flickering lights" | `set_light_effect` (`flicker`, `glow`, `strobe_fast`, `strobe_slow`, `random_off`) |
| "a new room off this one" | `create_sector_from_loop` (it splits touched walls at T-junctions and rebuilds sides for you), then `add_door` between the two sectors |
| "a pillar here / cut that room off" | `carve_void` |
| "dress the whole map in the brick theme" / "the walls look chopped mid-corner" | `auto_texture` (`retexture`), `align_textures` |
| "a whole new map, no drawing" | `build_map_from_plan` |
| "use my photo as this wall / this floor" | `create_texture_from_image` / `create_flat_from_image`, then apply — `guides/images-to-assets.md` owns the image side |
| "a new decoration from my drawing" | `create_sprite_from_image` |
| "replace the level music / the pistol sound" | `import_music` / `import_sound` under the vanilla lump name (`D_RUNNIN`, `D_STALKS`, ..., `DSPISTOL`, `DSDOORSW`, ...) — the engine plays lump `D<x>` for map music slot `<x>` and `DS<x>` for sound `<x>` |
| "several of these at once" | `run_batch` |

Two habits keep an edit invisible for the wrong reasons: snap new corners
to the existing grid (multiply of 8, and of 16 where the map's lines sit
on 16s), and dress new geometry in textures the map already wears — read
`describe_map`'s `top_wall_textures` and `top_flats` before choosing, not
after.

A map that depends on lumps you do not have (a custom texture pack, a
DEHACKED patch WAD) must load them the way the game does: ask for the
files and `merge_wad` them as layers. `validate_map` tells you whether
the merged view is complete — `missing_texture`, `missing_flat` and
`unknown_thing_type` rows name every gap before your edit compounds it.

## 4. Change safely

- **Preview big swings.** `bulk_edit`, `replace_texture`, `replace_flat`
  and `replace_thing_type` all take `dry_run: true`. Run the dry pass
  first whenever the call will touch more than about 20 elements: it
  reports the match count, the before/after `preview` rows and refuses
  without writing anything if a value would land out of range.
- **Cap the blast.** `bulk_edit`'s `max` (default 5000) refuses when the
  query matches more than you allowed. A `where: [{field: "light", ...}]`
  filter that unexpectedly matches 900 sectors fails instead of editing
  them.
- **Batch the multi-step changes.** A request that needs five to twenty
  calls ("secret room, medikit, darken it") goes through `run_batch` as
  **one** atomic, single-undo step: either every step lands or nothing
  does. Chain created indices between steps with `{"$ref": "0/index"}`
  instead of reading them back.
- **Rebuild nodes after geometry.** Anything that moves vertices or adds
  lines — `create_sector_from_loop`, `carve_void`, `add_door`,
  `add_stairs`, `add_teleporter`, a vertex-moving `bulk_edit` — reports
  `nodes_invalidated: true`. Finish the sequence with `build_all_nodes`
  (classic for vanilla play, `variant: "extended"` for ZDoom-family
  maps). `save_wad` never builds nodes for you: it writes the archive
  exactly as the session holds it.
- **`undo` beats repair.** An edit that went wrong comes back with one
  `undo` (the whole `run_batch` goes back as one step); redo with `redo`
  or lay it down again with better parameters.

## 5. Verify

1. `validate_map` — the structural floor: fix every `error` row before
   showing anything to the user. `nodes_missing` is the "you forgot step
   4" row.
2. `check_playability` — the player's question: can the start still reach
   the exit? Each blocker row names the tool that fixes it. Use
   `render: true` to get the picture with the broken parts highlighted.
3. `render_topdown` with `labels: {sectors: true}` — the layout, with the
   same numbers `bulk_edit` indexed. `fill_sectors: true` greys each
   sector by its light, which is how you *show* "darker".
4. `render_view` from the spot the user cares about — `x`, `y` and an
   `angle` — because a room that measures right can still look wrong.
5. The things a structural check cannot see are the gameplay ones: does
   the key stand behind the door it opens, is the secret reachable but
   hideable, can the monsters you just placed actually leave their room?
   `check_playability`'s `blockers`, `doors_never_opened`, `keys`, `locks`
   and `stuck_things` rows answer exactly these — read them, do not
   assume them.
6. Compare with the request, not with your memory of the parameters. If
   the answer is "not what they meant", `undo` and re-read; if it is,
   `diff_wads` with `a: "base", b: "working"` turns the session's deltas
   into the plain list you send back to the user ("changed MAP01, added
   3 lumps, new map MAP02").

## 6. Save

- `save_wad` with `path` to the user's file writes the **working archive
  only** — the normal, safe save: the IWAD and every merged layer are
  never modified. If the user wants their original kept, save to a new
  file name and tell them the load order.
- `save_wad` with `mode: "full"` exports the whole merged view as one
  standalone file — for handing a self-contained WAD to someone, not for
  a project save.
- Tell the user what the file needs to play: classic `build_all_nodes`
  output runs in vanilla and every port; `variant: "extended"` and
  `UDMF` maps need a ZDoom-family port; imported `PNG`/`OGG` lumps need
  one too.
- The session stays open after a save: `get_session_info` shows
  `dirty: false`, and more edits continue from there.
