# Verification

Never trust a change you have not looked at. After every meaningful edit,
run the loop:

1. **Change** — make one edit: build a room, place things, apply a texture.
   One intent per iteration so a failure names itself.
2. **`validate_map`** — structural check of the result: broken geometry, bad
   references, out-of-range values. Fix every error before continuing; read
   the numbers in the result instead of recalling what you sent.
3. **`check_playability`** (arriving with the map tools) — starts, exits,
   reachable items and keys, monster paths.
4. **`render_topdown` / `render_view`** — actually look: top-down for layout
   and scale, first-person for what the player sees.
5. **Compare with intent** — does the render answer the original request?
   Dimensions, lighting and proportions are checked against the picture, not
   against your memory of the parameters.
6. **Fix** — loop back to step 1 with the smallest change that closes the
   gap.

When a change went wrong and repairing it looks fiddlier than repeating it,
call `undo` and redo the step deliberately rather than patching blind.

Until each tool above ships (see the tool index in `../SKILL.md`), run its
step by hand: re-read the affected map data and compare the numbers.
