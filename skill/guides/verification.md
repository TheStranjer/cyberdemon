# Verification

Tools: add_exit, add_stairs, add_thing, build_map_from_plan, check_playability, edit_linedef, edit_sector, redo, render_topdown, render_view, run_batch, undo, validate_map

Never trust a change you have not looked at. After every meaningful edit,
run the loop:

1. **Change** — make one edit: build a room, place things, apply a texture.
   One intent per iteration so a failure names itself.
2. **`validate_map`** — structural check of the result: broken geometry, bad
   references, out-of-range values. Fix every error before continuing; read
   the numbers in the result instead of recalling what you sent.
3. **`check_playability`** — the player's question: can a start reach an
   exit, which sectors are cut off and why, which things cannot stand
   where they are. Run it after `validate_map` and fix every blocker with
   the tool its `hint` names (`add_stairs`, `edit_sector`, `edit_linedef`,
   `add_thing`, `add_exit`), then run it again; `render: true` draws the
   unreachable sectors, blocker lines and stuck things highlighted.
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
