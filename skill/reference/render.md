# Render tools

Session-level rendering defaults plus the first renderer. Three settings
tools set and read the defaults every render starts from;
`render_topdown` draws a whole-map overview as a `PNG` asset, and
`render_view` and `render_minimap` will join it later.

## The settings

| Field           | Default    | Range / values                                                                 |
|-----------------|------------|--------------------------------------------------------------------------------|
| `width`         | `640`      | `16..=4096` pixels                                                             |
| `height`        | `400`      | `16..=4096` pixels; `width * height` at most `16777216` (exactly `4096x4096`)  |
| `lighting`      | `"doom"`   | `"doom"` (sector light plus the `COLORMAP` distance fade), `"flat"` (sector light only), `"fullbright"` |
| `show_things`   | `true`     | thing markers in top-down renders and minimaps                                 |
| `show_sprites`  | `true`     | sprites in the 3D view                                                         |
| `sky`           | `true`     | draw the sky texture; off means the `background` colour                        |
| `fov`           | `90`       | horizontal field of view, `10..=170` degrees                                   |
| `gamma`         | `0`        | `0..=4` (Doom gamma levels); applied to every output channel                   |
| `topdown_style` | `"editor"` | `"editor"` or `"automap"`                                                      |
| `background`    | `"#000000"` | `#RRGGBB` hex colour (either case)                                            |

`get_render_settings`, `set_render_resolution` and `set_render_options` all
return the whole object above, so the result of any change is the complete
current state:

```json
{
  "width": 640,
  "height": 400,
  "lighting": "doom",
  "show_things": true,
  "show_sprites": true,
  "sky": true,
  "fov": 90,
  "gamma": 0,
  "topdown_style": "editor",
  "background": "#000000"
}
```

Settings belong to the connection, not to the WAD: `open_iwad`, `open_wad`,
`new_wad`, `merge_wad` and an undo never change them, and nothing writes
them into an archive. Enum and colour names are matched case-insensitively;
a stored colour keeps the spelling you sent. `gamma` is a float-free
approximation of Doom's gamma curves: level `0` changes nothing, higher
levels lift mid-tones while black and white stay pinned
(`out = in + (255 - in) * in * level / (255 * 8)`).

## Lighting

The 3D view shades every pixel through one of the 32 lighting tables of
the `COLORMAP`. Which table depends on the `lighting` setting, the
surface's sector light, how far the surface is from the camera (in map
units) and — for walls — the engine's fake contrast:

```
fullbright : table 0
flat       : clamp((255 - light) >> 3,        0, 31)
doom       : clamp(((255 - light) >> 3) + (depth >> 7) - 4, 0, 31)

then, for walls:  east-west lines (dy = 0) -1, north-south lines (dx = 0) +1,
                  anything else unchanged; clamp to 0..=31 again
```

The `doom` rule approximates the game's diminishing light: four tables
brighter at the camera, one table darker every 128 map units. The final
colour is `PALETTE[COLORMAP[table][index]]` with the `gamma` curve
applied to each channel. Tables 32/33 (invulnerability, all-black) are
not selected by this formula.

How the view gets its pictures: the scene loads every wall texture the
map names (composited from `TEXTURE1`/`TEXTURE2` definitions, `TEXTURE2`
winning, or a `TX_`-region patch of that name), every flat the sectors
name, and a sky chosen by map name — `E#M#` episodes take `SKY1..4`
(`Heretic`: 1,2,3,1,3), `MAP01`–`MAP11` take `SKY1`, `MAP12`–`MAP20`
`SKY2`, `MAP21` up `SKY3`, anything else `SKY1`. A merged `COLORMAP` of
at least 34 full tables is used as-is; otherwise the tables are generated
from `PLAYPAL` palette 0. Missing textures, flats and skies, and a
generated `COLORMAP`, are reported as `problems` while the view still
renders: missing pictures become a 64x64 checkerboard of the palette's
brightest and darkest colours. `render_view` surfaces all of this once it
ships.

## `get_render_settings`

- Purpose: read the settings this connection currently holds. Call it before
  a render to know what you are looking at, and after a refused change to
  confirm nothing moved.
- Arguments: none.
- Returns: the settings object above.
- Errors: `internal` on a poisoned session lock.

## `set_render_resolution`

- Purpose: set the pixel size every render defaults to.
- Arguments: `width` (integer, `16..=4096`), `height` (integer,
  `16..=4096`). The pair is also capped at `16777216` pixels in total.
- Returns: the full settings object with the new size.
- Errors: `invalid_argument` when an edge is outside `16..=4096` or the total
  is over the cap — the message names the range or the cap and the settings
  keep the size they had.

## `set_render_options`

- Purpose: change the options one at a time or several at once. Pass only the
  fields you want changed; anything you leave out keeps its current value.
- Arguments: `lighting`, `show_things`, `show_sprites`, `sky`, `fov`,
  `gamma`, `topdown_style`, `background` — all optional, ranges and values as
  in the table above. At least one is required.
- Returns: the full settings object after the change.
- Errors: `invalid_argument` when no field at all is given (the message lists
  the option names) or when a value is out of range or not one of the accepted
  names (the message lists the valid values). The call is all-or-nothing: one
  bad value means none of the others land either, and `get_render_settings`
  still reports what it did before.
- Example: a bright, wide view for checking layout, and the editor-style
  top-down map with thing markers:

  ```json
  {"lighting": "fullbright", "fov": 110, "gamma": 1}
  {"topdown_style": "editor", "show_things": true, "sky": false}
  ```

## `render_topdown`

- Purpose: render a whole map overview as a `PNG` asset — the 2D line map
  or an isometric wireframe — to look at after (or while) building.
- Arguments: all optional.
  - `map`: as everywhere (`list_maps`/`set_active_map` decide the default).
  - `width`, `height`: `16..=4096`, default from the settings.
  - `projection`: `"topdown"` (default) or `"isometric"`.
  - `bounds`: `{min_x, min_y, max_x, max_y}` in world units; the default
    is the map's vertex bounds padded by 32 units. Needs
    `min_x < max_x` and `min_y < max_y`.
  - `style`: `"editor"` (default) or `"automap"`, the line colours below.
  - `fill_sectors` (default `false`): paint each sector's polygon with a
    grey equal to half its light level before the lines.
  - `show_vertices` (default `false`): a `3x3` `#FFD000` mark per vertex.
  - `show_things` (default from the settings): thing markers — a circle
    sized from the definition radius and a facing tick.
  - `highlight`: `{linedefs: [...], sectors: [...], things: [...],
    vertices: [...]}` index lists; highlighted linedefs draw red and 3 px
    wide, sectors get a red tint, things a white outline, vertices turn
    red. An index past the end of its table fails `not_found`.
- Returns:

  ```json
  {"map": "MAP01", "projection": "topdown", "width": 200, "height": 120,
   "transform": {"scale_num": 88, "scale_den": 320,
                 "offset_x": 30, "offset_y": 95},
   "asset": {"url": "http://localhost:PORT/assets/ID/MAP01_topdown.png", ...}}
  ```

  `asset` is the overview `PNG` (the `gamma` setting is applied to the
  output); `transform` is `null` for the isometric projection.

### Reading the picture: the colour legend

Editor style (top-down): one-sided lines `#C0C0C0`, two-sided `#707070`,
lines with a `special` `#6A8CFF`. Automap style reproduces the in-game
map: one-sided (and `secret`) lines red `#FC0000`, floor-height changes
brown `#BC7848`, ceiling-height changes yellow `#FCFC00`, flat two-sided
lines grey `#808080`; lines flagged `dontdraw` are skipped. Filled
sectors take a grey of half their light (`#505050` for light 160).
Vertices are `#FFD000` squares. Thing circles are coloured by their
editor category: Monsters `#E04040`, Player Starts `#40E040`,
Weapons/Ammunition `#E0A040`, Health And Armour/Powerups `#40A0E0`,
Keys `#E0E040`, everything else (and unknown types) `#B0B0B0`; the tick
shows the facing angle. Highlights: `#FF4040` lines and vertices, the
same colour blended over highlighted sectors, white outlines on things.

### Converting pixels to map coordinates and back

For `projection: "topdown"` the `transform` is an integer, uniform scale
with north up. With `n = scale_num`, `d = scale_den`:

```
px = offset_x + floor(x * n / d)     py = offset_y - floor(y * n / d)
x  = (px - offset_x) * d / n         y  = (offset_y - py) * d / n
```

(the forward mapping floors; the inverse is integer division). Use the
inverse to ask "what is at this pixel?" — e.g. after spotting a gap in
the picture, convert it back and `get_map_summary`/`inspect` the linedefs
near those coordinates. A world point's own radius scales by `n / d` too.

### The isometric projection

`projection: "isometric"` draws a `2:1` wireframe: a point at
`x`/`y`/`z` lands on the plane `u = x - y`, `v = (x + y) / 2 - z` before
the same fit-and-centre step, then the map is drawn as edges: every
linedef's floor edge at the front sector's floor, the back sector's
floor edge where it differs, and for one-sided lines the ceiling edge
plus a vertical post at each vertex. Things are small category-coloured
dots at their sector's floor. There is no hidden-line removal: it is a
structure check, not a pretty picture. `transform` is `null` (two
projections were fitted through one mapping, so pixel math back to world
coordinates is not offered); `style`, `fill_sectors` and `show_vertices`
do not apply, `show_things` does.

- Errors: `not_found` for an unknown map or an out-of-range highlight
  index; `invalid_argument` when an empty map is requested without
  `bounds`, when `bounds` has `min >= max`, for a size outside the
  settings' ranges, and for an unknown `projection`/`style` name.
- Example: a quick editor-style overview at the session's size, then a
  zoom into one corner:

  ```json
  {}
  {"projection": "isometric"}
  {"bounds": {"min_x": 0, "min_y": 0, "max_x": 512, "max_y": 512},
   "style": "automap", "fill_sectors": true}
  {"highlight": {"linedefs": [12, 13], "sectors": [4]}}
  ```

## Where this fits

These settings are defaults only: a render tool's own arguments win over
them. How to pick resolution, lighting and gamma for the check you are
making is your call, not the server's; the render-then-compare loop they
feed is in [guides/verification.md](guides/verification.md).
