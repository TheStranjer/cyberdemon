# Editing existing maps

To be written. Start with the first step, which is settled:

- **Before touching anything, call `describe_map`** on the map (its
  labelled overview image is on by default): read the image, map the
  user's words onto sector numbers by matching the rows' `things`,
  `wall_textures`, `roles`, `light` and heights - "the dark room with the
  shotguns", "the big one with the sky" - and then act through the
  sector-indexed tools on exactly those rows (`edit_sector`, `add_door`,
  `place_things`, `mark_secret`, ...; `reference/playability.md` explains
  the rows, `reference/sectors.md` and friends the edits).

The rest of this guide will answer:

- How to keep edits non-destructive (working session, save-as, `undo`) and
  leave the user's original WAD intact?
- How to add geometry to an existing map without breaking its grid alignment,
  texture scale or visual style?
- How to handle maps that depend on external lumps (textures, sounds,
  DEHACKED patches) you did not author?
- What to re-check after every edit that `validate_map` cannot see: secret
  routing, key loops, monster reachability?
