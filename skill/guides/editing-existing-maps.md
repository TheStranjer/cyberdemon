# Editing existing maps

To be written. This guide will answer:

- How to inspect a map before touching it: what exists, what references it,
  how much vanilla limit headroom is left?
- How to keep edits non-destructive (working session, save-as, `undo`) and
  leave the user's original WAD intact?
- How to add geometry to an existing map without breaking its grid alignment,
  texture scale or visual style?
- How to handle maps that depend on external lumps (textures, sounds,
  DEHACKED patches) you did not author?
- What to re-check after every edit that `validate_map` cannot see: secret
  routing, key loops, monster reachability?
