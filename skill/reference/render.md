# Render tools

Session-level rendering defaults. These three tools set and read the
settings that `render_view`, `render_topdown` and `render_minimap` use as
their defaults wherever their own arguments leave a value out.

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

## Where this fits

These settings are defaults only: a render tool's own arguments win over
them. How to pick resolution, lighting and gamma for the check you are
making is your call, not the server's; the render-then-compare loop they
feed is in [guides/verification.md](guides/verification.md).
