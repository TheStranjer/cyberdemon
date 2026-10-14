# Palette tools

Reading and writing the session's colours: the `PLAYPAL` palette bank and
the `COLORMAP` light tables derived from it.

## How palettes work in Doom

- **`PLAYPAL`** is a bank of 256-colour RGB palettes, 768 bytes each.
  `Doom` has 14: entry 0 is the normal palette, 1..=8 the red damage
  flash, 9..=12 the gold item-pickup tint and 13 the green
  radiation-suit view. Every sprite, flat and HUD graphic indexes into
  the palette the engine has selected.
- **`COLORMAP`** is 34 tables of 256 bytes. Table `k` in 0..=31 maps each
  palette index to the index the engine draws it with at light level `k`
  (0 = full brightness, 31 = darkest); table 32 is the invulnerability
  view (inverse greyscale) and table 33 is all black. Sector lights,
  fog and the distance fade all pick a table.
- Both are global lumps: the merged view's last copy wins, and editing
  them creates an override in the working archive (copy-on-write — the
  `IWAD` layer is never modified).

## `get_palette`

- Purpose: reads one `PLAYPAL` entry and publishes a preview.
- Arguments:
  - `index` (number, optional, default 0): which palette entry to read.
- Returns:

  ```json
  {
    "palettes": 14,
    "index": 0,
    "colors": ["#000000", "... 256 total ...", "#ffffff"],
    "swatch": {"url": "http://localhost:PORT/assets/ID/PLAYPAL_0.png",
               "filename": "PLAYPAL_0.png", "mime": "image/png", "size": 890},
    "has_colormap": true
  }
  ```

  `colors` are the 256 entries as `#RRGGBB` (uppercase hex); `swatch` is
  the published 256x256 `PNG` preview, drawn as a 16x16 grid of 16-pixel
  cells in palette order; `has_colormap` says whether the session also
  holds a `COLORMAP` lump.
- Errors: `not_found` when no loaded archive has a `PLAYPAL` (the message
  says to `open_iwad`) or when `index` is past the last entry (the
  message states how many palettes there are).

## `set_palette`

- Purpose: writes palettes into the working archive's `PLAYPAL` — and
  optionally rebuilds the vanilla tints and the `COLORMAP` — as ONE
  undoable edit labelled `set_palette` (`save_wad` writes the result).
- Arguments — exactly one source is required:
  - `colors` (list of strings): exactly 256 `"#RRGGBB"` (or `RRGGBB`)
    colours: one new palette.
  - `source` (string): absolute path, `file://` URI, `http(s)://` URL or
    uploaded asset URL of either an image with exactly 256 pixels (read
    row-major, alpha dropped — a 16x16 `PNG` is the natural shape), or
    raw palette bytes whose length is a non-empty multiple of 768, which
    replace the WHOLE `PLAYPAL` bank.
  - `index` (number, optional, default 0): which entry a 256-colour
    payload replaces; passing the current palette count appends a new
    entry; a larger index is `invalid_argument`. A raw multi-palette
    `source` ignores it.
  - `regenerate_tints` (boolean, optional, default `false`): rebuild
    palettes 1..=13 from palette 0, appending what is missing to reach 14
    — an integer approximation of the vanilla tables: per channel
    `c' = c + (t - c)·num/den` rounded, with the red tints 1..=8 targeting
    (255,0,0) at weight `i/9`, the gold tints 9..=12 targeting (215,186,69)
    at weight `(i-8)/8` and the green suit tint 13 targeting (0,255,0) at
    weight 1/8.
  - `regenerate_colormap` (boolean, optional, default `false`): rebuild
    `COLORMAP` (34 tables of 256) from palette 0 by nearest-colour
    matching: map `k` scales colours by `(32-k)/32`, map 32 is the
    inverted greyscale `255 - (299r+587g+114b)/1000` (invulnerability)
    and map 33 maps every index to the nearest colour to black. Also an
    approximation of id's tables, exact enough for a consistent look.
- Returns:

  ```json
  {"palettes": 14, "replaced": [0, 1, 2], "colormap_regenerated": false}
  ```

  `replaced` lists, ascending, the entries the call wrote: the one
  `index` for a 256-colour source, every carried entry for a raw one,
  plus 1..=13 when the tints were rebuilt. `colormap_regenerated` says
  whether `COLORMAP` was rewritten into the working archive too.
- Errors: `invalid_argument` when neither or both sources are given, a
  hex colour is malformed, the colour list is not exactly 256 long, the
  source image has a pixel count other than 256, the raw bytes are not a
  non-empty multiple of 768, or `index` is past the count. Fetching the
  `source` can report `not_found` / `fetch_error` / `io_error` /
  `limit_exceeded` as usual.
- Undo: one `set_palette` entry covers the `PLAYPAL` and `COLORMAP`
  writes together; the layers and the file on disk never change.
- Verify: `get_palette` reads any entry back; `get_lump` with
  `format: "hex"` or `"url"` inspects the raw `PLAYPAL`/`COLORMAP`
  overrides; `list_lumps scope=working` shows the created overrides.
