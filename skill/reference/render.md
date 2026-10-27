# Render tools

Session-level rendering defaults plus the renderers. Three settings
tools set and read the defaults every render starts from;
`render_topdown` draws a whole-map overview as a `PNG` asset,
`render_minimap` draws a compact automap-style square, and
`render_view` casts a first-person frame.

## The settings

| Field           | Default    | Range / values                                                                 |
|-----------------|------------|--------------------------------------------------------------------------------|
| `width`         | `640`      | `16..=4096` pixels                                                             |
| `height`        | `400`      | `16..=4096` pixels; `width * height` at most `16777216` (exactly `4096x4096`)  |
| `lighting`      | `"doom"`   | `"doom"` (sector light plus the `COLORMAP` distance fade), `"flat"` (sector light only), `"fullbright"` |
| `show_things`   | `true`     | thing markers in top-down renders                                              |
| `show_sprites`  | `true`     | sprites in the 3D view                                                         |
| `sky`           | `true`     | draw the sky texture; off means the `background` colour                        |
| `fov`           | `90`       | horizontal field of view, `10..=170` degrees                                   |
| `gamma`         | `0`        | `0..=4` (Doom gamma levels); applied once to every output channel at encode    |
| `topdown_style` | `"editor"` | `"editor"` or `"automap"`                                                      |
| `background`    | `"#000000"` | `#RRGGBB` hex colour (either case, `#` optional); reported lower-cased       |

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
a stored colour reports back as lowercase `#rrggbb`. `gamma` is a float-free
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
brighter at the camera, one table darker every 128 map units. The published
colour is `PALETTE[COLORMAP[table][index]]` with the `gamma` curve applied
to each channel exactly once, over the finished frame when it is encoded;
no texel is corrected a second time on its way in. Tables 32/33
(invulnerability, all-black) are not selected by this formula.

How the view gets its pictures: the scene loads every wall texture the
map names (composited from `TEXTURE1`/`TEXTURE2` definitions, `TEXTURE2`
winning, or a `TX_`-region patch of that name), every flat the sectors
name, and — only where some sector's ceiling or floor is the session's
sky flat (`F_SKY1`, or `F_SKY` in `Hexen`) — a sky
chosen by map name: `E#M#` episodes take `SKY1..4` (`Heretic`:
1,2,3,1,3), `MAP01`–`MAP11` take `SKY1`, `MAP12`–`MAP20` `SKY2`, `MAP21`
up `SKY3`, anything else `SKY1`. That sky picture is the
`TEXTURE1`/`TEXTURE2` record of the chosen name, which is how the games
store skies (`SKY1` is a texture, not a lump), or the flat of that name
for a session that carries one instead. A merged `COLORMAP` of at least 34
full tables is used as-is; otherwise the tables are generated from
`PLAYPAL` palette 0. What the session cannot serve — a texture, a flat,
the sky a map actually shows, one of a sky's components — becomes a 64x64
checkerboard of the palette's brightest and darkest colours and a line in
`render_view`'s `problems` list, and a generated `COLORMAP` is reported
the same way. Nothing else is: a picture no part of the map draws is
never looked for, so an indoor map under an archive with no sky at all
renders with `problems: []`.

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
    (when unset) is the map's vertex bounds, unioned with the underlay's
    map rectangle when there is one, padded by 32 units. Needs
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
  - `underlay`: `{source, units, opacity}` draws a reference picture (the
    sketch you built from) under the map. `source` is a file path,
    `file://`/`http(s)://` URI or asset `URL` like every import tool's.
    `units` — `{scale_num (1), scale_den (1), origin_x (0), origin_y (0),
    flip_y (false)}` — is the same object you pass to
    `build_map_from_plan.units`: image pixel `(px, py)` sits at map
    `x = origin_x + px*scale_num/scale_den`,
    `y = origin_y - py*scale_num/scale_den` (`+` when `flip_y` is false),
    so the sketch lines up with the built map exactly. `opacity` is a
    percent, `0..=100` (default `50`); every texel blends over the
    background scaled by its own alpha. Sampling stops at the picture's own
    edges: it is drawn once, never repeated past one, and output pixels
    clear of its map rectangle keep the background. With `bounds` unset the
    frame is the union of the map bounds and the underlay's map rectangle,
    padded by 32 units — an empty map with an underlay needs no explicit
    bounds.
  - `grid`: spacing in map units, `8..=1024`, drawn under the map: minor
    lines `#2A2A2A`, every 8th line `#3C3C3C`. A spacing that measures
    under 4 pixels at the current zoom is skipped and reported in `notes`.
  - `labels`: `{sectors, linedefs, things, vertices}` (default all
    `false`) draws those elements' indices in `#FFFF80` with a black
    outline, on top of everything: sectors at a point guaranteed inside
    them, linedefs at their midpoint, things and vertices beside their
    marks.
  - `annotations`: `[{x, y, text, color}]` free text (e.g. room names
    from your plan) centred on world coordinates, colour
    `"#RRGGBB"` (default `"#FFFFFF"`). Text upper-cases; characters
    outside the built-in 5x7 font draw as `?`. At most 64 rows of at
    most 32 characters each — beyond either limit is `invalid_argument`.
  - `label_scale`: `1..=4` magnification of every label and annotation
    (default `2` when the output is at least 1024 pixels wide, else `1`).
  All five are overlays of the top-down projection: with
  `projection: "isometric"` any of them fails `invalid_argument`
  ("overlays need the topdown projection"). Drawing order: background,
  underlay, grid, sector fills, lines, vertices, things, highlights,
  labels, annotations.
- Returns:

  ```json
  {"map": "MAP01", "projection": "topdown", "width": 200, "height": 120,
   "transform": {"scale_num": 88, "scale_den": 320,
                 "offset_x": 30, "offset_y": 95},
   "underlay": {"width": 50, "height": 40,
                "map_bounds": {"min_x": 0, "min_y": -320,
                               "max_x": 400, "max_y": 0}},
   "notes": [],
   "asset": {"url": "http://localhost:PORT/assets/ID/MAP01_topdown.png", ...}}
  ```

  `asset` is the overview `PNG` (the `gamma` setting is applied to the
  output); `transform` is `null` for the isometric projection; `underlay`
  is `null` without one and otherwise reports the decoded picture's size
  and the map-space rectangle its four corners land in; `notes` lists
  what the render skipped (currently only the too-fine grid).

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
do not apply, `show_things` does, and the overlay parameters (`underlay`,
`grid`, `labels`, `annotations`, `label_scale`) fail `invalid_argument`
("overlays need the topdown projection").

- Errors: `not_found` for an unknown map or an out-of-range highlight
  index; `invalid_argument` when an empty map is requested without
  `bounds` and without an `underlay`, when `bounds` has `min >= max`, for
  a size outside the settings' ranges, for an unknown `projection`/`style`
  name, for a `grid` outside `8..=1024`, a `label_scale` outside `1..=4`
  or an `underlay.opacity` outside `0..=100`, for more than 64
  `annotations` or one longer than 32 characters, for a malformed
  annotation colour, and for any overlay parameter on the isometric
  projection; `format_error` when an `underlay.source` is no readable
  picture; `fetch_error`/`not_found` when an `underlay.source` cannot be
  fetched.
- Example: a quick editor-style overview at the session's size, then a
  zoom into one corner:

  ```json
  {}
  {"projection": "isometric"}
  {"bounds": {"min_x": 0, "min_y": 0, "max_x": 512, "max_y": 512},
   "style": "automap", "fill_sectors": true}
  {"highlight": {"linedefs": [12, 13], "sectors": [4]}}
  {"grid": 64, "labels": {"sectors": true, "things": true}}
  {"underlay": {"source": "/path/to/sketch.png",
                "units": {"scale_num": 4, "flip_y": true}},
   "labels": {"sectors": true},
   "annotations": [{"x": 256, "y": 128, "text": "hall"}]}
  ```

  The second line of the pair is the verification view: pass the same
  `units` you gave `build_map_from_plan` and the sketch sits exactly
  under the map you built from it.

## `render_minimap`

- Purpose: a compact automap-style view for quick orientation — the same
  line map the in-game automap draws, in a small square.
- Arguments: all optional.
  - `map`: as everywhere (`list_maps`/`set_active_map` decide the default).
  - `size`: square output edge in pixels, `32..=1024` (default `256`).
  - `center`: `{x, y}` — the world point to frame (default: the map
    centre, the midpoint of the vertex bounds).
  - `radius`: map units from the centre to a window edge, `1..=65536`
    (default: fit the whole map — half the larger bound span rounded up,
    padded by one unit).
  - `player`: `{x, y, angle}` — draws the white automap player arrow at
    that world position, 16 pixels long, facing `angle` degrees
    counter-clockwise from east.
  - `show_things` (default `false`): a `2x2` pixel dot per thing in its
    category colour (the `render_topdown` legend above; the
    `show_things` setting does not carry over here).
  - `palette_colors` (default `true`): with a `PLAYPAL` in the session,
    take the colours from palette 0 at the vanilla automap indices — 176
    red, 64 brown, 231 yellow, 96 grey and 209 white; otherwise (or with
    the flag off) use the RGB defaults below.
- Style: always automap. Black background; one-sided (and `secret`) lines
  red, floor-height changes brown, ceiling-height changes yellow, other
  two-sided lines grey; `dontdraw` lines hidden. Lines are clipped to the
  square window.
- Returns:

  ```json
  {"map": "MAP01", "size": 256, "center": {"x": 256, "y": 128},
   "radius": 257,
   "asset": {"url": "http://localhost:PORT/assets/ID/MAP01_minimap.png", ...}}
  ```

  The window maps pixels by `px = size/2 + floor((x - center.x) * size /
  (2 * radius))`, `py = size/2 - floor((y - center.y) * size /
  (2 * radius))` (north up). The `gamma` render setting is applied to the
  output.
- Errors: `invalid_argument` for a `size` outside `32..=1024`, a `radius`
  outside `1..=65536`, or an empty map without `center` and `radius`;
  `not_found` for an unknown map.
- Example: the whole map at a glance, then a zoom on one corner with the
  player marked:

  ```json
  {}
  {"center": {"x": 64, "y": 128}, "radius": 64, "player": {"x": 64, "y": 128, "angle": 90}}
  ```

## `render_view`

- Purpose: render a first-person 3D frame from a camera inside the map
  and publish it as a `PNG` asset — look at what you built the way the
  engine draws it.
- Arguments: `x` and `y` (map units) are required; the rest is optional.
  - `map`: as everywhere (`list_maps`/`set_active_map` decide the default).
  - `z`: eye height in map units (default: the camera sector's floor
    plus Doom's 41-unit view height).
  - `angle`: facing in degrees **counter-clockwise from east** (`0`
    east, `90` north, `180` west, `270` south). Default `0`.
  - `pitch`: look up or down in degrees, **positive looks up**,
    `-60..=60`. Default `0` (level).
  - `fov`: horizontal field of view in degrees, `10..=170` (default:
    the setting).
  - `width`, `height`: `16..=4096`, total capped like
    `set_render_resolution` (default: the settings).
  - `lighting`: `"doom"`, `"flat"` or `"fullbright"` for this call
    only (default: the setting).
- Returns:

  ```json
  {"map": "MAP01",
   "camera": {"x": 64, "y": 128, "z": 41, "angle": 0, "pitch": 0,
              "fov": 90, "sector": 0},
   "width": 160, "height": 100,
   "sprites_drawn": 1, "masked_columns": 0,
   "problems": [],
   "asset": {"url": "http://localhost:PORT/assets/ID/MAP01_view.png", ...}}
  ```

  The result's `camera` object echoes the effective camera — the resolved `z` and the index
  of the sector holding it — so you can re-find the exact viewpoint.
  `sprites_drawn` and `masked_columns` count the layers the transparent
  pass painted. The `gamma` render setting is applied to the output.
- What it draws: linedef faces with their `upper`/`middle`/`lower`
  textures, including texture offsets, Doom's pegging flags (one-sided
  middles hang from the ceiling; upper bands sit on the far ceiling,
  lower bands on the far floor) and `COLORMAP` shading with fake
  contrast, as above. Floors and ceilings draw their **flat textured**,
  aligned to the world grid like Doom: each pixel samples the flat at
  the world point its row's depth lands on, texel column `floor(x)`
  and texel row `floor(-y)`, wrapped into the picture (any supported
  flat size works: `64x64`, `64x128`, `128x128`, `256x256`), shaded
  row by row by that row's plane depth. A ceiling (or floor) whose
  flat is the session's sky flat (`F_SKY1`, or `F_SKY` in `Hexen`) draws
  the **sky texture** instead: the sky column
  comes from the column's view angle (Doom's 1024 slices per full
  turn, so a 256-wide sky repeats four times), the row from Doom's
  `320x200` sky placement, and sky is always full bright and infinitely
  far. With the `sky` setting off, sky surfaces show the `background`
  colour; an upper wall between two sky ceilings is not drawn, so the
  sky shows through. A `-` texture draws nothing and leaves the
  background.
- See-through layers paint last, **far to near** over the solid frame,
  each pixel drawn only where its layer is nearer than the wall behind
  it (the opaque depth buffer; it is read, never overwritten):
  - **Masked middle textures**: every two-sided line whose faced side
    carries a non-`-` middle texture (fences, grates, chain-link,
    door windows) draws that texture over the still-open window rows,
    from the view column the line shows. It hangs from the lower of the
    two ceilings, or — with the linedef's lower-unpegged bit (16) —
    stands on the higher floor. It is never tiled vertically: texture
    rows outside its height are left open, and transparent texels show
    the geometry behind.
  - **Sprites** (while the `show_sprites` setting is on): things whose
    game or custom definition carries a sprite draw their patch, scaled
    by distance and centred on `W/2 + tx·k`. The vanilla eight-rotation
    bucketing picks the frame from the viewing angle and the thing's
    facing, and the eight-character pair names carry both of their
    rotations: `TROOA2A8` serves rotation `2` as stored and rotation `8`
    flipped, so walking around a monster turns it. A sprite stands on the
    floor of the sector holding the thing plus its spawn `z`; hanging
    things (dead corpses, chandeliers) hang from the ceiling instead.
    Things outside every sector, or within 4 units of the camera, are
    skipped. A sprite lump the session lacks adds one `missing sprite NAME`
    line to `problems` (a lump that is no readable picture adds one
    `unreadable sprite NAME` line), and the rest of the frame still
    renders. Every `problems` line names a picture this render actually
    asked for: a sky is looked for in the texture definitions, where a
    stock archive keeps it, and a map with no plane wearing the session's
    sky flat asks for no
    sky and reports none missing.
- Current limitations: sprites and masked textures paint opaque (no
  translucency), fullbright sprite frames are not swapped in, and there
  are no 3D floors, slopes or portals.
- Errors: `invalid_argument` when `x`/`y` lies in no sector ("camera is
  outside the map"), when `pitch` or `fov` is outside its range, for an
  edge outside the resolution ranges or over the pixel cap, and for an
  unknown `lighting` name; `not_found` for an unknown map or a session
  without a `PLAYPAL` (rendering needs one).
- Example: standing on the player start looking east, then a bright
  wide-angle check of what is behind you:

  ```json
  {"x": 64, "y": 128, "angle": 0, "width": 320, "height": 200}
  {"x": 64, "y": 128, "angle": 180, "lighting": "fullbright", "fov": 110}
  ```

## Where this fits

These settings are defaults only: a render tool's own arguments win over
them. How to pick resolution, lighting and gamma for the check you are
making is your call, not the server's; the render-then-compare loop they
feed is in [guides/verification.md](guides/verification.md).
