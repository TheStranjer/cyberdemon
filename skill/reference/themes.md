# Themes tools

Role-based texture resolution: ask the session "the door texture", get a
texture that actually exists in the merged view. Every theme tool and every
later high-level tool (drawing, doors, stairs, the plan builder) resolves
*roles*, never hard-coded names like `DOORTRAK`.

## Roles

Twenty roles: `wall`, `wall_alt`, `floor`, `ceiling`, `light_ceiling`,
`outdoor_ceiling`, `door`, `door_track`, `door_track_red`,
`door_track_blue`, `door_track_yellow`, `door_floor`, `switch`,
`exit_switch`, `step`, `lift_side`, `lift_floor`, `window_sill`,
`teleport_floor`, `liquid_floor`.

The eight flat roles (`floor`, `ceiling`, `light_ceiling`,
`outdoor_ceiling`, `door_floor`, `lift_floor`, `teleport_floor`,
`liquid_floor`) resolve against the merged view's flats namespace
(`F_SKY1` is always available); every other role is a wall texture and
resolves against the `TEXTURE1`/`TEXTURE2` definitions plus `TX_` lumps.

## How a role resolves

Per role the theme holds a `|`-separated candidate list and an optional
`fallback_role`. Resolution is deterministic, in this order:

1. the exact candidate names, in listed order — `"matched": "exact"`;
2. the glob candidates (`*` and `?`, case-insensitive), each taking the
   alphabetically first existing name — `"glob"`;
3. the role's `fallback_role`, walking the chain — `"fallback"`;
4. the same role in the game's default theme (a theme that omits a role
   inherits it) — `"fallback"`;
5. the game's last-resort texture for the role's kind (`STARTAN` /
   `FLOOR0_1` / `CEIL1_1` for the DOOM games, `GRSTNPB` / `FLOOR03` /
   `FLAT506` for Heretic, `FOREST01` / `F_010` / `F_011` for Hexen,
   `BRKGRY17` / `F_NOLINE` / `F_DECK` for Strife), else the
   alphabetically first existing texture or flat of the right kind, else
   `"-"` when the session holds nothing — `"default"`.

The theme itself is session state (like the render settings): not part of
any WAD, not undoable, unaffected by opening or saving archives. The
detected game picks the default theme: `tech` (DOOM / DOOM II: also
`hell`, `brick`, `rock`), `castle` (Heretic), `forest` (Hexen), `urban`
(Strife); an undetected session uses the DOOM table.

## `get_texture_theme`

- Purpose: read the session's theme and every role's current answer.
- Arguments: none.
- Returns:

  ```json
  {
    "theme": "tech",
    "available_themes": ["brick", "hell", "rock", "tech"],
    "roles": {
      "wall": {"texture": "STARTAN3", "matched": "exact"},
      "door_track": {"texture": "DOORTRAK", "matched": "exact"}
    }
  }
  ```

  `roles` covers all twenty roles; see above for the `matched` kinds.
  An empty session answers every role `{"texture": "-", "matched":
  "default"}` — not an error.
- Errors: none from client input.

## `set_texture_theme`

- Purpose: switch themes and/or override single roles.
- Arguments:
  - `theme` (string, optional): case-insensitive name from
    `available_themes`; an unknown name fails with `invalid_argument`
    listing them.
  - `overrides` (object, optional): role → texture/flat name. Each name
    must exist in the merged view (`not_found` otherwise; check
    `list_textures` / `list_flats`, or add with `build_texture` /
    the import tools). `"-"` clears that role's override. An unknown role
    key fails with `invalid_argument` listing all roles.
  Passing neither fails with `invalid_argument`.
- Returns: the same document as `get_texture_theme`, after the change.
- Errors: `invalid_argument`, `not_found`.

## `auto_texture`

- Purpose: dress a map (or some sectors) from the theme, as one undoable
  edit. Two modes:
  - `fill_missing` (the default) changes only what would render wrong:
    a one-sided line with an empty middle gets `wall`; a visible upper gap
    (back ceiling below the front's, both sides not `F_SKY1`) with an
    empty upper gets `wall`, or `door` when the back sector is a closed
    door (ceiling equals floor); a visible lower gap with an empty lower
    gets `step` for a step of at most 24 units, else `wall`; flats that do
    not exist get replaced by the theme `floor`/`ceiling` flats.
  - `retexture` repaints the selection: every one-sided middle becomes
    `wall`, existing upper/lower bands follow the theme (low bands get
    `step`, others `wall`), lines carrying a door action keep their role
    and take the `door` texture, their plain two-sided middle-textured
    neighbours (the jambs) take `door_track`, other special-carrying lines
    keep their textures, and every floor/ceiling becomes the theme flat —
    `F_SKY1` is kept.
- Arguments:
  - `map` (string, optional): defaults to the active map (then the merged
    view's only map).
  - `sectors` (list of numbers, optional): the sectors to dress; default
    every sector. An index the map lacks is `not_found`; an empty list is
    `invalid_argument`.
  - `mode` (string, optional): `"fill_missing"` (default) or
    `"retexture"`.
  - `include_flats` (bool, optional, default `true`): set false to leave
    the sector flats alone in either mode.
- Returns:

  ```json
  {"mode": "fill_missing", "changed_sidedefs": 4, "changed_sectors": 1,
   "map": "MAP01", "nodes_invalidated": false, "copied_to_working": true}
  ```

  `changed_sidedefs` counts sides whose texture fields moved,
  `changed_sectors` counts sectors whose flats moved. A second run of the
  same call changes nothing (the rules are checks, not churn). Texture
  edits never invalidate node data. Verify the result with
  `render_topdown`.
- Errors: `invalid_argument` (mode, empty `sectors`, no map), `not_found`
  (map, sector), plus the ordinary `edit_map` refusals.

## `align_textures`

- Purpose: run wall textures continuously across corners, as one undoable
  edit. A chain is consecutive linedefs sharing a vertex in drawing order
  (previous `v2` == next `v1`) whose front side carries the same wall
  texture — the middle of a one-sided line, the upper of a two-sided one.
- Arguments:
  - `map` (string, optional): defaults as in `auto_texture`.
  - `linedefs` (number or list, optional) xor `sector` (number, optional):
    which lines to consider; neither means the whole map, both is
    `invalid_argument`.
  - `x` (bool, default `true`): rewrite each member's `x_offset` to the
    accumulated chain distance before the line modulo the texture width,
    so the picture continues across corners.
  - `y` (bool, default `false`): rewrite `y_offset` so every member's
    texture top pegs to the highest front ceiling in the chain (vanilla
    pegs an unset top to the line's own ceiling).
  Passing `x: false, y: false` fails with `invalid_argument`.
- Returns:

  ```json
  {"aligned": 6, "warnings": [], "map": "MAP01",
   "nodes_invalidated": false, "copied_to_working": false}
  ```

  `aligned` counts sidedefs whose offsets actually moved. `warnings`
  names chains left alone: a texture with no `TEXTURE1`/`TEXTURE2`
  definition has no width, and offsets outside the binary `i16` range
  (`-32768..=32767`) stay put (`UDMF` has no such cap).
- Errors: `invalid_argument` (both selectors, no axis, empty list),
  `not_found` (map, linedef, sector), plus the ordinary `edit_map`
  refusals.
