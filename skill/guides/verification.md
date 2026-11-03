# Verification

Tools: add_exit, add_stairs, add_thing, build_map_from_plan, check_playability, create_map, create_sector_from_loop, describe_map, edit_linedef, edit_sector, edit_thing, place_things, redo, render_topdown, render_view, run_batch, undo, validate_map

Never trust a change you have not looked at. After every meaningful edit,
run the loop:

1. **Change** — make one edit: build a room, place things, apply a texture.
   One intent per iteration so a failure names itself.
2. **`validate_map`** — structural check of the result: broken geometry, bad
   references, out-of-range values. Fix every error before continuing; read
   the numbers in the result instead of recalling what you sent. The one
   people forget is `no_player1_start` — see "Check the player 1 start
   first" below.
3. **`check_playability`** — the player's question: can a start reach an
   exit, which sectors are cut off and why, which things cannot stand
   where they are. Every unreachable sector arrives explained: `blockers`
   rows carry a `reason` — `step_too_high` when the far floor stands more
   than 24 units above the one the player is in (its `delta` says how far
   past the climb), `isolated` when no adjacent sector lets a player in at
   all (a plain gap), `locked` when a door wants a key the walk never
   reached — plus the `from` sector and the boundary `line` they were tried
   across, so a counter or planter floor and a road with no curb are numbers
   to fix, not engine rules to remember. Run it after `validate_map` and fix
   every blocker with the tool its `hint` names (`add_stairs`, `edit_sector`,
   `edit_linedef`, `add_thing`, `add_exit`), then run it again; `render: true`
   draws the unreachable sectors, blocker lines and stuck things highlighted.
4. **`render_topdown` / `render_view`** — actually look: top-down for layout
   and scale, first-person for what the player sees. After building from a
   sketch, render it with the sketch as `underlay` (same `units` as
   `build_map_from_plan`) and `labels: {sectors: true}` so the built map and
   the drawing line up and you can say "sector 7 is too small" by index.
5. **Compare with intent** — does the render answer the original request?
   Dimensions, lighting and proportions are checked against the picture, not
   against your memory of the parameters.
6. **Fix** — loop back to step 1 with the smallest change that closes the
   gap.

## Check the player 1 start first

Nothing above matters until somebody can spawn, and a start is easiest to
lose at the beginning: `create_map` appends an empty block,
`create_sector_from_loop` and `place_things` fill it with rooms and
monsters, and a map whose thing 0 is an imp has never spawned a player.
Read the start before the layout:

- `validate_map` answers `no_player1_start` (**error**: "no player 1 start
  … add thing type 1 standing inside a sector") and `missing_coop_starts`
  (**info**: which of types 2-4 are absent — add one of each, spaced apart,
  when the map should host coop). Every message names its own fix; there is
  no separate `hint` field to look for.
- `check_playability` answers `ok: false` with `start: null`, the summary
  "No player 1 start: nothing can be reached." and that ask as
  `next_steps[0]`. Every sector reads `isolated` behind it, so fix the start
  *before* chasing `blockers` — that whole list is an artefact of a walk
  that never began. A start that exists but stands outside every sector is
  a different problem and is worded as one: "Player 1 start thing N stands
  outside every sector", hinted at with `edit_thing` into a room or
  `create_sector_from_loop` to draw a floor under it.
- `describe_map`'s `summary.player_starts` lists the type 1-4 things it
  found, so an empty array says "nobody spawns" without walking anything.
- `build_map_from_plan` cannot produce the problem: it refuses with "a map
  needs a player 1 start; pass player_start or a thing of type 1".
  `create_map` can, so it says so — its report warns "the new map holds no
  player 1 start" and its `next_steps` lead with `add_thing type 1`; pass
  `player_start: {x, y, angle}` to stand the start in with the block. It
  deliberately does **not** default one to `(0, 0)`: a spawn with no floor
  under it trades a loud error for a quieter one, so the choice stays with
  whoever knows where the level begins.

When a change went wrong and repairing it looks fiddlier than repeating it,
call `undo`, then lay the step back down with `redo` or with a better
input — a deliberate repeat beats a blind patch.

A failed `run_batch` (default `atomic: true`) leaves the map untouched: the
whole batch rolls back with it, so there is nothing half-applied to repair —
fix the failing step's input and send the batch again. A batch that went
through is one `undo` wide, whatever its length
(see `../reference/batch.md`).

Until each tool above ships (see the tool index in `../SKILL.md`), run its
step by hand: re-read the affected map data and compare the numbers.
