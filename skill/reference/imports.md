# Image imports

Turning external picture files into lumps: the patch importer
`import_image_as_patch` (alias `import_png_as_patch`), the flat importer
`import_image_as_flat` (alias `import_png_as_flat`), the sprite importer
`import_image_as_sprite` (alias `import_png_as_sprite`) and the
byte-for-byte `import_image_as_raw`. The preview pipeline `process_image`
runs image operations over a picture and publishes the result so you can
look at it before committing, and three one-shot creators commit a whole
finished resource in a single undoable step:
`create_texture_from_image` (wall texture: patch + `PNAMES` +
definition), `create_flat_from_image` (a square floor/ceiling flat —
`64x64` by default, `128` or `256` for ZDoom-family ports — optionally
tileable and assigned to a theme flat role) and
`create_sprite_from_image` (background-stripped, trimmed, scaled sprite
lumps, one call per frame set). Where the graphics tools
(`reference/graphics.md`) *publish* session pictures outward, these tools
bring pictures *in*, as one undoable edit each (`undo` steps them back;
`save_wad` writes them out).

## Shared parameters

`import_image_as_patch`, `import_png_as_patch`, `import_image_as_sprite`,
`import_png_as_sprite`, `import_image_as_flat` and `import_png_as_flat`
share these:

- `source` (string, required): the picture to import — absolute path,
  `file://` URI, `http(s)://` URL or this server's asset/upload URL (see
  `reference/assets.md`). `PNG`, `JPEG`, `GIF` (first frame) and `BMP`
  bytes are accepted; the format is sniffed from the bytes, the file name
  never matters.
- `name` (string, optional): 1 to 8 printable ASCII characters, uppercased.
  Defaults to the fetched file stem uppercased and trimmed: `my art.png`
  becomes `MYART`. When nothing usable can be derived the call fails with
  `invalid_argument` asking for this name.
- `palette` (integer, optional, default 0): which `PLAYPAL` entry the
  picture is quantized against. A session with no `PLAYPAL` (open one with
  `open_iwad`) or an index the lump does not hold is `not_found`.
- `transparency` (object, optional; patch importers only): which source
  pixels become patch holes. `{ "mode": "alpha", "threshold": 128 }` (the
  default) drops pixels whose alpha is below `threshold`; `{ "mode":
  "color", "color": "#RRGGBB" }` drops pixels equal to `color` exactly
  (default `"#00FFFF"`, the cyan Doom sprite sources are keyed against);
  `{ "mode": "none" }` keeps every pixel opaque. Fields irrelevant to the
  chosen mode are ignored. Flats store no transparency at all, so
  `import_image_as_flat` keeps every pixel opaque and only counts the
  ignored alpha.
- `offsets` (string or object, optional; patch and sprite importers): the
  drawing offsets stored in the patch header. `"zero"`, `"grab"`,
  `"center"`, `"center_bottom"` or an explicit `{ "x": n, "y": n }` pair.
  Omitted means `"grab"`: a `PNG`'s `grAb` chunk when it carries one, else
  the tool's own default — `"zero"` for the patch tools (wall patches) and
  `"center_bottom"` for the sprite tools (a sprite stands on its origin).
  `center`
  is `(width/2, height/2)` and `center_bottom` is `(width/2, height)` in
  the Doom offset convention: a positive left offset shifts the image left,
  and the top offset is the distance from the image's top edge down to the
  origin. Offsets outside the `-32768..=32767` a patch header stores are
  `invalid_argument`.
- `replace` (boolean, optional, default `false`): a working-archive lump of
  this name in the target region is `already_exists` unless `replace`
  overwrites it in place. A same-name lump only the `IWAD` or a resource
  layer holds never clashes: the import becomes an override.

The result of a patch import always measures what was stored: `width`,
`height`, `left_offset`, `top_offset`, `transparent_pixels` (holes the
quantization dropped), `colors_used` (distinct opaque source colours) and
`max_color_error` (worst squared RGB distance to the palette — `0` means
the source was already palette-exact).

## `import_image_as_patch`

- Purpose: quantizes the picture to the session's `PLAYPAL` and stores it
  in the classic Doom column/post patch format in the working archive's
  **patches** region (`P_START`/`P_END` markers created there when
  missing), as one undoable edit labelled `import_patch NAME`.
- Arguments: the shared parameters above, plus
  - `add_to_pnames` (boolean, optional, default `true`): also list the
    name in `PNAMES`, copy-on-write into the working archive. A name
    `PNAMES` already lists keeps its index and rewrites nothing.
- Returns:

  ```json
  {
    "name": "MYART",
    "lump_index": 1,
    "pnames_index": 2,
    "width": 64,
    "height": 128,
    "left_offset": 0,
    "top_offset": 0,
    "transparent_pixels": 250,
    "colors_used": 180,
    "max_color_error": 30
  }
  ```

  `lump_index` is the patch's position in the working archive;
  `pnames_index` its `PNAMES` entry (`null` when `add_to_pnames` is false
  and the name is not listed).
- Errors: `invalid_argument` (bad name or offsets, underivable name),
  `format_error` / `unsupported` (bytes that are no readable image),
  `not_found` (missing file, missing `PLAYPAL`, bad palette index),
  `already_exists` (working twin without `replace`).
- Check the result with `list_patches` (it appears with `in_pnames: true`)
  and publish it back with `get_patch`; `list_pnames` / `edit_pnames`
  manage the name list afterwards.

## `import_png_as_patch`

An alias of `import_image_as_patch` for PNG-fluent clients. Despite the
name it accepts any supported image, not just `PNG`; everything —
parameters, placement, `PNAMES` listing, undo label and result — is
identical.

## `import_image_as_flat`

- Purpose: quantizes the picture to the session's `PLAYPAL` in your
  `dither` mode and stores it as the raw row-major palette-index plane a
  flat is, in the working archive's **flats** region (`F_START`/`F_END`
  markers created there when missing), as one undoable edit labelled
  `import_flat NAME`.
- Arguments: `source`, `name`, `palette` and `replace` as in the shared
  parameters above — plus, instead of `transparency` and `offsets`:
  - `resize` (string, optional, default `"none"`): what to do with a
    picture that is not already a flat shape. `"none"` refuses it;
    `"nearest"` scales it to `size x size`. A picture that already is
    `64x64`, `64x128`, `128x128` or `256x256` is kept as it is.
  - `size` (integer, optional, default 64): the square side for
    `resize="nearest"`: `64`, `128` or `256`; another value is
    `invalid_argument`. Only `64x64` loads in vanilla Doom — `128` and
    `256` need a ZDoom-family port. The result echoes it as `size`.
  - `dither` (string, optional, default `"floyd_steinberg"`, the same
    default `process_image` takes): how the quantization handles colours
    the `PLAYPAL` lacks. `"none"` maps every pixel to its nearest palette
    entry, `"floyd_steinberg"` diffuses the rounding error over the
    picture (serpentine), `"ordered"` applies a fixed `4x4` `Bayer`
    offset. Any other word is refused. The result echoes the mode as
    `dither`.
- Returns:

  ```json
  {
    "name": "MYFLAT",
    "lump_index": 1,
    "width": 64,
    "height": 64,
    "size": 64,
    "dither": "floyd_steinberg",
    "resized": false,
    "colors_used": 256,
    "max_color_error": 0,
    "transparent_pixels_ignored": 0
  }
  ```

  `lump_index` is the flat's position in the working archive; `width` and
  `height` are the stored flat's dimensions and `resized` says whether the
  picture was scaled. `size` echoes the `size` argument in force (the
  square side `resize: "nearest"` scales to) and `dither` the mode the
  quantization used; `colors_used` and `max_color_error` measure the
  source against the palette whichever mode ran. Flats store no
  transparency: every pixel stays opaque and
  `transparent_pixels_ignored` counts the source pixels that had alpha
  below 128.
- Errors: `invalid_argument` (bad or underivable name, a non-flat shape
  without `resize`, a bad `resize` word, a `size` outside 64/128/256),
  `format_error` / `unsupported` (bytes that are no readable image),
  `not_found` (missing file, missing `PLAYPAL`, bad palette index),
  `already_exists` (working twin without `replace`). A flat only the
  `IWAD` or a resource layer holds becomes an override.
- Check the result with `list_flats` (it appears with `source: "working"`)
  and publish it back with `get_flat`. For the column/post texture format
  used by wall patches use `import_image_as_patch`; a `PNG` lump for
  ZDoom-family ports goes through `import_image_as_raw` with
  `namespace: "flats"`.

## `import_png_as_flat`

An alias of `import_image_as_flat` for PNG-fluent clients. Despite the
name it accepts any supported image, not just `PNG`; everything —
parameters, flats-region placement, undo label and result — is identical.

## `import_image_as_sprite`

- Purpose: quantizes the picture to the session's `PLAYPAL`, stores it in
  the `Doom` column/post patch format and places it in the working
  archive's **sprites** region (`S_START`/`S_END` markers created there
  when missing), as one undoable edit labelled `import_sprite NAME`.
- Arguments: `source`, `palette`, `transparency`, `offsets` and `replace`
  as in the shared parameters above (the `offsets` default is
  `"center_bottom"`: `left = width/2`, `top = height`, which puts the
  sprite's feet on its origin — a `PNG` `grAb` chunk wins over it), plus
  the naming below. There is no `PNAMES` listing: sprites are named by
  their lump name, not by a texture list.

### Sprite naming

Give exactly one of the two forms (both, or neither, is
`invalid_argument`); unlike the other importers this tool never derives a
name from the file stem:

- `name` (string): the complete lump name, `PPPPFN` or `PPPPFNFN` — four
  sprite characters (`ASCII` letters or digits, e.g. `TROO`, `B4R1`), one
  frame letter (`A`-`Z`, then `[`, `\`, `]`: 29 frames) and one rotation
  digit `0`-`8`. `0` is the all-angles image: one picture serves every
  view. A second frame+rotation pair on the end is served by the same
  image horizontally mirrored, so `TROOA2A8` draws rotation 2 as stored
  and rotation 8 flipped. Lowercase input is uppercased like any lump
  name. Anything outside the grammar (`TOOLONG1`, `TROOA9`, `TROOA0A8`)
  is `invalid_argument`, and the error explains the grammar.
  Examples: `TROOA1` (view 1 of frame `A`), `PLAYA0` (every view of frame
  `A`), `POSSA2A8` (views 2 and 8, the second mirrored).
- `sprite` (string, four characters) with `frame` (default `"A"`),
  `rotation` (default `0`) and `mirror_rotation` (optional). The tool
  builds the name: `sprite: "TROO"`, `frame: "A"`, `rotation: 2`,
  `mirror_rotation: 8` gives `TROOA2A8`. `mirror_rotation` must be one of
  `1`-`8`, must differ from `rotation`, and needs `rotation` to be a view
  `1`-`8` too (rotation 0 has no mirror partner); each violation is
  `invalid_argument`.

- Returns:

  ```json
  {
    "name": "TROOA2A8",
    "lump_index": 3,
    "sprite": "TROO",
    "frame": "A",
    "rotation": 2,
    "mirror": 8,
    "warnings": [],
    "width": 48,
    "height": 64,
    "left_offset": 24,
    "top_offset": 64,
    "transparent_pixels": 1200,
    "colors_used": 32,
    "max_color_error": 18
  }
  ```

  `lump_index` is the position in the working archive and `mirror` is the
  rotation the mirrored image serves (`null` for a six-character name);
  the rest are the shared picture measurements.
- `warnings` never stop the import; they report what an engine would trip
  over in the merged view:
  - another sprite lump already serves one of the rotations this lump
    covers, e.g. importing `TROOA2` while `TROOA2A8` exists leaves two
    images for rotation 2 — remove one lump or import a rotation none
    covers;
  - the frame ends up mixing the all-angles rotation `0` with per-view
    rotations `1`-`8`, which engines resolve differently.
  A lump named exactly like the import is not an overlap (it is the
  duplicate rule below, or the copy being replaced).
- Errors: `invalid_argument` (bad or ungrammatical naming, both forms or
  neither, bad offsets), `format_error` / `unsupported` (bytes that are no
  readable image), `not_found` (missing file, missing `PLAYPAL`, bad
  palette index), `already_exists` (a working sprites lump of this name
  without `replace`). A name only the `IWAD` or a resource layer holds
  becomes an override.
- Check the result with `list_sprites` (the frame appears under its
  prefix, malformed region names are listed separately in `invalid`) and
  publish it back with `get_sprite`, which resolves a rotation through the
  engine's rules: the exact lump, then the pair lump that carries the
  rotation as its own or as its mirror (flipped), then the frame's
  rotation-`0` image.

## `import_png_as_sprite`

An alias of `import_image_as_sprite` for PNG-fluent clients. Despite the
name it accepts any supported image, not just `PNG`; everything —
parameters, naming, sprites-region placement, undo label, `warnings` and
result — is identical.

## `import_image_as_raw`

- Purpose: validates that the bytes are a readable image, then inserts
  them **unchanged** — no quantization, no re-encoding. This is how
  ZDoom-family ports carry graphics: they read `PNG` and `JPEG` lumps
  directly, so the original file reaches the game untouched. One undoable
  edit labelled `import_raw NAME`.
- Arguments:
  - `source` (string, required): as above.
  - `name` (string, optional): as above.
  - `namespace` (string, optional): place the lump inside a region
    (`patches`, `sprites`, `flats`, ...; its markers are created in the
    working archive when it has none). Without it the lump is stored loose,
    in the global area.
  - `replace` (boolean, optional, default `false`): as above, scoped to
    the chosen region.
- Returns:

  ```json
  { "name": "MYART", "lump_index": 0, "format": "png", "width": 64, "height": 128 }
  ```

  `format` is the sniffed image format (`png`, `jpeg`, `gif` or `bmp`);
  `width` and `height` come from the decode check.
- Errors: as for `import_image_as_patch`; bytes failing the decode check
  store nothing.
- Note: a raw `PNG` lump is not a vanilla patch. `get_patch` and
  `list_patches` still read it (ports carry such lumps), but vanilla
  engines need `import_image_as_patch` instead. Check the stored bytes with
  `get_lump format=hex` or `extract_lump`.
- For a vanilla sprite use `import_image_as_sprite` (its name carries the
  frames and rotations an engine reads); a raw `PNG` in the sprites region
  only works for ports that read them.

## Image operations (`ops`)

`process_image` and `create_texture_from_image` take an `ops` array: a
list of image operations applied **in order** to the decoded picture. Each
object carries an `op` tag naming the operation plus its parameters; every
edge an op produces must stay within `1..=4096` and a bad parameter fails
with `invalid_argument` naming the op index and field (e.g.
`ops[2].width`), the first failing op stopping the chain. This is the
canonical operation reference.

- `crop` — cut out a rectangle; it must lie fully inside the picture.
  `{ "op": "crop", "x": 0, "y": 0, "width": 64, "height": 64 }` (`x`/`y`
  the top-left corner, `width`/`height` `1..=4096`).
- `fit` — resize to exactly `width` x `height` with one `mode`:
  `"stretch"` ignores the aspect ratio, `"cover"` scales until both edges
  cover the target and centre-crops, `"contain"` scales until both edges
  fit and centres on a canvas. `filter` is `"nearest"`, `"bilinear"` or
  `"area"` (default `area`: footprint-averaging shrink, bilinear grow);
  `background` (`#RRGGBBAA`/`#RRGGBB`, default transparent) fills the
  `contain` canvas.
  `{ "op": "fit", "width": 128, "height": 128, "mode": "cover" }`.
- `rotate` — turn clockwise a quarter, half or three-quarter turn.
  `{ "op": "rotate", "quarter_turns": 1 }` (`1..=3`).
- `flip` — mirror horizontally and/or vertically.
  `{ "op": "flip", "horizontal": true, "vertical": false }`.
- `perspective_correct` — warp a four-corner source quad onto a straight
  rectangle: `corners` are the quad's `{"x": .., "y": ..}` source pixel
  positions ordered top-left, top-right, bottom-right, bottom-left (they
  may be negative); each destination corner pixel samples exactly its
  corner position and the projective map between is solved in fixed
  point. `width`/`height` are `1..=4096`; repeated or collinear corners
  are `invalid_argument`, and destination pixels whose source position
  falls outside the picture come back transparent black.
  `{ "op": "perspective_correct", "width": 256, "height": 256, "corners": [{"x": 20, "y": 10}, {"x": 230, "y": 4}, {"x": 250, "y": 240}, {"x": 12, "y": 232}] }`.
- `auto_levels` — stretch the luminance range of opaque pixels to full
  scale. `clip_percent` (`0..=10`, default `1`) is the share of pixels
  allowed below the low and above the high cut.
  `{ "op": "auto_levels", "clip_percent": 2 }`.
- `adjust` — per-channel tone correction. `brightness` (`-255..=255`,
  default `0`, added after contrast), `contrast` (`0..=400` percent about
  mid grey, default `100`) and `saturation` (`0..=400`, default `100`).
  `{ "op": "adjust", "contrast": 120, "saturation": 80 }`.
- `color_key` — make every pixel whose maximum channel difference to
  `color` fits within `tolerance` fully transparent. `color` is
  `#RRGGBBAA`/`#RRGGBB` (the `#` optional); `tolerance` is `0..=255`
  (default `0`). `{ "op": "color_key", "color": "#00FFFF", "tolerance": 16 }`.
- `trim` — crop to the bounding box of pixels reaching `alpha_threshold`
  (default `128`), then add `padding` transparent pixels on every side
  (default `0`). `{ "op": "trim", "padding": 4 }`.
- `remove_background` — flood fill from the border and make every reached
  pixel matching the background colour transparent (unlike `color_key`,
  interior regions of that colour survive). `tolerance` is `0..=255`
  (default `32`); `color` defaults to the average colour of the dominant
  border bucket. `{ "op": "remove_background", "color": "#00FF00" }`.
- `make_tileable` — blend each edge into its opposite half-period shift so
  the result tiles seamlessly. `band` is the blend width in pixels
  (default `width / 4` horizontally and `height / 4` vertically,
  `1..=length / 2` on every enabled axis); `horizontal` and `vertical`
  switch the two axes (both default `true`).
  `{ "op": "make_tileable", "band": 16 }`.
- `threshold` — binarize every opaque pixel to black or white by its
  luminance. `level` is `0..=255` or omitted for the Otsu-picked cut;
  `invert` swaps the two classes (default `false`).
  `{ "op": "threshold", "level": 128 }`.
- `adaptive_threshold` — the uneven-light variant of `threshold`: binarize
  each pixel against its own flattened background instead of one global
  cut. The background of a pixel is the mean luminance of the
  `(2r + 1) x (2r + 1)` window around it (borders clipped); it is ink when
  it sits `offset` or more below that local mean, and — with `invert` —
  `offset` or more above it. `radius` is the background window radius,
  `1..=128` (small for line art, a large value, around `80`, for photos);
  `offset` is `0..=255` (default `15`); `invert` defaults `false`.
  Transparent pixels count as paper. Every output pixel ends opaque black
  or white. Reports `radius`, `offset`, `background_min`/`background_max`
  (the local-mean extremes: the measurable proof the gradient was
  flattened) and `ink_pixels`.
  `{ "op": "adaptive_threshold", "radius": 80, "offset": 15 }`.
- `grayscale` — replace the colour channels of every opaque pixel (alpha
  `128` or above) with its luminance; every alpha stays untouched. Reports
  `distinct_colors_before` and `distinct_colors_after` counted over unique
  opaque `RGBA` triples, so you can see how much colour the step collapsed.
  `{ "op": "grayscale" }`.
- `blur` — box-blur the picture: every pixel becomes the average of the
  `(2r + 1) x (2r + 1)` window around it, clipped at the borders; colours
  are weighted by alpha, so transparent pixels never darken an edge.
  `radius` is `1..=256`. `{ "op": "blur", "radius": 2 }`.
- `thin_stroke_filter` — drop hairline ruling. Binarize the picture the same
  way as `threshold` (`level` omitted means Otsu; `invert` swaps the two
  classes), then keep only the ink wide enough to contain the square
  `(2r + 1) x (2r + 1)` element: strokes thinner than that element
  (1 px rules, stray specks) are erased while walls `2r + 1` pixels or thicker
  survive intact, and `erode`'s border convention keeps frame-touching ink.
  `radius` is `1..=16`. Reports `level`, `otsu`, `removed_ink_pixels` and
  `kept_ink_pixels`. `{ "op": "thin_stroke_filter", "radius": 1 }`.

## `process_image`

- Purpose: run the `ops` over a picture and publish the result so you can
  **look at it before committing** — it changes no archive and records no
  undo step. Decode, `ops`, quantize (optional), publish two `PNG` assets:
  `processed` (the full `RGBA` picture after `ops`) and `quantized` (the
  indexed result rendered back through the palette, transparency as alpha).
  The `processed` `URL` is itself a valid `source` for any import tool, so
  you can preview, re-edit and only then commit.
- Arguments:
  - `source` (string, required): the picture to process — absolute path,
    `file://` URI, `http(s)://` URL or this server's asset/upload `URL`
    (a previously published asset `URL` included).
  - `ops` (array, optional, default none): the operations above, applied in
    order.
  - `dither` (string, optional, default `"floyd_steinberg"`): the palette
    quantization of the `quantized` preview — `"none"`, `"floyd_steinberg"`
    or `"ordered"`.
  - `palette` (integer, optional, default 0): the `PLAYPAL` entry to
    quantize against; a session with no `PLAYPAL` or a past-the-end index
    is `not_found`.
  - `transparency` (object, optional): the same object the import pipeline
    takes (default `{mode: "alpha", threshold: 128}`); which source pixels
    are transparent in the `quantized` preview.
  - `quantize` (boolean, optional, default `true`): when `false` only the
    `processed` `PNG` is published and the three measures answer `null`.
- Returns:

  ```json
  {
    "width": 64, "height": 64,
    "steps": [ { "op": "crop", "x": 0, "y": 0, "width": 64, "height": 64 } ],
    "processed": { "url": "...", "filename": "MYART-processed.png", "mime": "image/png", "size": 210 },
    "quantized": { "url": "...", "filename": "MYART-quantized.png", "mime": "image/png", "size": 180 },
    "colors_used": 42, "max_color_error": 30, "transparent_pixels": 250
  }
  ```

  `steps` is one measurement object per applied op in order;
  `colors_used`, `max_color_error` and `transparent_pixels` describe the
  quantization (`0` error means the source was already palette-exact) and
  answer `null` when `quantize` is `false`.
- Errors: `invalid_argument` (a bad op parameter, naming `ops[i].field`,
  or a bad `transparency`), `format_error` / `unsupported` (bytes that are
  no readable image), `not_found` (missing `source`, missing `PLAYPAL`, bad
  palette index).
- `create_texture_from_image`, `create_flat_from_image` and
  `create_sprite_from_image` below commit a whole resource in one step;
  for a lone patch, flat or sprite use the import tools with the
  `processed` `URL` as `source`.

## `create_texture_from_image`

- Purpose: **one photo to a working wall texture** — it processes the
  picture, settles a tiling-friendly size, quantizes it opaquely, stores a
  patch lump, lists it in `PNAMES` and defines a single-patch
  `TEXTURE1`/`TEXTURE2` record, all as **one** undoable edit labelled
  `create_texture NAME` (`save_wad` writes it; a single `undo` removes
  patch, `PNAMES` entry and definition together).
- Arguments:
  - `source` (string, required): the picture, as in `process_image`.
  - `name` (string, required): the texture name, 1 to 8 printable ASCII
    characters, uppercased.
  - `patch_name` (string, optional, default `name`): the patch lump to
    store the picture under.
  - `width` / `height` (integers, optional): the texture size. A `width`
    must be a power of two in `16..=1024` (a bad one is `invalid_argument`
    naming the nearest valid widths) and a `height` `1..=1024`. A missing
    edge follows the picture's aspect ratio rounded to a multiple of `8`
    (a derived `width` to the nearest valid power of two). With **neither**
    given, a processed picture whose `width` is a power of two in `16..=1024`
    and `height` at most `1024` keeps its size; any other is fit to
    `128x128` and `notes` says so.
  - `fit` (string, optional, default `"cover"`): `cover`, `contain` or
    `stretch` for that automatic resize step (an `area`-filtered `fit` op).
  - `tileable` (boolean, optional, default `false`): append a
    `make_tileable` op blending opposite edges.
  - `ops`, `dither`, `palette`: as in `process_image` (your `ops` run
    before the automatic resize/tiling).
  - `target` (string, optional): `"TEXTURE1"` or `"TEXTURE2"`
    (case-insensitive); unset means the lump that already defines `name`,
    else `TEXTURE1`.
  - `assign_role` (string, optional): a theme **wall** role (e.g. `"wall"`)
    to point at the new texture afterwards. A flat or unknown role is
    `invalid_argument` and nothing is written. Theme state is session
    state, so `undo` does not roll the override back (the result notes say);
    clear it with `set_texture_theme overrides`.
  - `replace` (boolean, optional, default `false`): redefine an existing
    texture of this name in place and allow overriding a layer-held patch
    name.
- Size: walls are opaque — pixels whose alpha is below `128` become the
  palette colour nearest to black and a `warning` counts them (for a masked
  patch use `import_image_as_patch`).
- Collisions refuse **before anything is written**: a patch lump
  `patch_name` a lower (`IWAD` or resource) layer holds and the working
  archive does not is `already_exists` (`pass patch_name` for another, or
  `replace=true` to write a working override), and a `name` the merged view
  already defines is `already_exists` unless `replace=true`.
- Returns:

  ```json
  {
    "texture": "MYTEX", "patch": "MYTEX", "width": 128, "height": 128,
    "lump": "TEXTURE1",
    "steps": [ { "op": "fit", ... } ],
    "colors_used": 60, "max_color_error": 24,
    "notes": ["resized to 128x128; pass width/height to choose"],
    "warnings": [],
    "preview": { "url": "...", "filename": "MYTEX_texture.png", ... },
    "role": null,
    "next_steps": [
      "apply with edit_sidedef (texture fields) or auto_texture mode retexture after assign_role",
      "render_view to see it in place"
    ]
  }
  ```

  `preview` is the composited texture's `PNG`; `steps` is one measurement
  object per applied op (your `ops` first, then the automatic ones); `role`
  echoes an `assign_role` that landed.
- Errors: `invalid_argument` (a bad name, a width that is no power of two,
  a bad height, an unknown `target` or `assign_role`, a failing op),
  `format_error` / `unsupported` (bytes that are no readable image),
  `not_found` (missing `source`, missing `PLAYPAL`, bad palette index),
  `already_exists` (a layer-held `patch_name` or a defined `name` without
  `replace`).
- Look at the result with `list_textures` (the definition), `get_patch` (the
  stored lump) and `get_asset_url prefer=texture` (the same composited
  preview), then apply it with `edit_sidedef` or dress a map with it via
  `auto_texture mode=retexture` after `assign_role`.

## `create_flat_from_image`

- Purpose: **one photo to a working floor/ceiling flat** — it processes
  the picture (the same `ops`/`dither`/`palette` parameters
  `process_image` takes), fits it to the square `size` you pass (`64x64`
  by default), optionally blends opposite edges, quantizes it opaquely and
  stores the raw index plane in the working archive's **flats** region
  (`F_START`/`F_END` markers created there when missing), all as **one**
  undoable edit labelled `create_flat NAME` (`save_wad` writes it; one
  `undo` removes the flat).
- Size: `size` is `64` (default), `128` or `256`; any other value is
  `invalid_argument` listing the three. Vanilla engines tile floors and
  ceilings at `64x64` and load nothing bigger, so the picture is fit to
  that square unless you ask for one of the ZDoom-family shapes — and then
  `warnings` says so ("vanilla Doom loads only 64x64 flats"). Whichever
  side you get, an oversized source is cropped by `cover`, letterboxed by
  `contain`, distorted by `stretch`; `import_image_as_flat` keeps the
  other flat shapes (including `64x128`) when you need them.
- Arguments:
  - `source` (string, required): the picture; a `process_image`
    `processed` URL is a valid source.
  - `name` (string, required): the flat lump name, 1 to 8 printable ASCII
    characters, uppercased. Nothing is derived from the file.
  - `fit` (string, optional, default `"cover"`): how the picture maps
    onto the square. `contain` pads with transparency, which quantizes to
    the palette colour nearest to black (flats store no transparency).
  - `size` (integer, optional, default `64`): the square side of the
    stored flat — `64`, `128` or `256`. The result echoes it.
  - `tileable` (boolean, optional, default `true`): append a
    `make_tileable` op with a 16-pixel band so neighbouring tiles meet
    without seams. Pass `false` to keep the edges exactly as drawn.
  - `ops`, `dither`, `palette`: as in `process_image` (`dither` defaults
    to `"floyd_steinberg"`); your `ops` run before the automatic fit. The
    result echoes the `dither` mode it quantized with.
  - `assign_role` (string, optional): a theme **flat** role (`floor`,
    `ceiling`, `light_ceiling`, `outdoor_ceiling`, `door_floor`,
    `lift_floor`, `teleport_floor`, `liquid_floor`) to point at the new
    flat afterwards. A wall role or unknown name is `invalid_argument`
    and nothing is written. Theme state is session state, so `undo` does
    not roll the override back (the result warns); clear it with
    `set_texture_theme overrides`.
  - `replace` (boolean, optional, default `false`): see the collision
    rule below.
- Collisions refuse **before anything is written**, stricter than the
  plain flat importer because flats are used *by name* everywhere: a
  working `flats` lump of this name is `already_exists` unless
  `replace=true` overwrites it in place, and a flat this name only the
  `IWAD` or a resource layer holds is `already_exists` too — "this would
  override <name> everywhere it is used; pass replace to do that or
  choose another name". `replace=true` accepts the override and the
  result answers `overrides_existing: true`.
- Returns:

  ```json
  {
    "flat": "MYFLOOR",
    "size": 64,
    "dither": "floyd_steinberg",
    "steps": [ { "op": "fit", ... }, { "op": "make_tileable", ... } ],
    "colors_used": 210, "max_color_error": 24,
    "overrides_existing": false,
    "preview": { "url": "...", "filename": "MYFLOOR_flat.png", ... },
    "role": null,
    "warnings": [],
    "next_steps": [ "edit_sector floor_flat/ceiling_flat", "auto_texture mode retexture after assign_role" ]
  }
  ```

  `steps` is one measurement object per applied op (yours first, then the
  automatic `fit` and `make_tileable`); `preview` is the stored flat's
  picture as the flat decoder reads it back; pixels whose alpha is below
  `128` stay opaque and a `warning` counts them.
- Look at it with `list_flats`/`get_flat` (both read the bigger squares
  back unchanged), assign it with `edit_sector` (`floor_flat`/
  `ceiling_flat`) or dress a whole map with `auto_texture mode=retexture`
  after `assign_role`.

## `create_sprite_from_image`

- Purpose: **one drawing to working sprite lumps** — it processes the
  picture, removes the border background, trims the empty margin, scales
  it to a sprite-sized height, quantizes it with alpha holes and stores
  the `Doom` column/post patch in the working archive's **sprites**
  region (`S_START`/`S_END` markers created there when missing), all as
  **one** undoable edit labelled `create_sprite NAME` (one `undo` removes
  every lump the call wrote).
- Naming follows `import_image_as_sprite` exactly (see *Sprite naming*
  above — the reminder: `PPPPFN`/`PPPPFNFN`, nothing derived from the
  file), with one addition: `frames` (an array of frame letters, e.g.
  `["A","B","C","D"]`) writes the **same final image once per letter** at
  the given `rotation` — the static-replacement form. `frames` goes with
  `sprite` and is `invalid_argument` together with `name` or `frame`.
- Arguments:
  - `source` (string, required): the picture; a `process_image`
    `processed` URL is a valid source.
  - `name`, or `sprite` with `frame`/`frames`/`rotation`/
    `mirror_rotation`: the three naming forms above.
  - `remove_background` (boolean, optional, default `true`): flood-clear
    pixels matching the border background colour from the edges inwards,
    so a drawing on white loses the white but keeps enclosed white
    details (unlike `color_key`).
  - `background_tolerance` (integer, optional, default `32`): per-channel
    match distance; `background_color` (`#RRGGBB`/`#RRGGBBAA`, optional)
    forces the colour instead of detecting the dominant border bucket.
  - `trim` (boolean, optional, default `true`): crop to the bounding box
    of pixels at alpha `128` or above, padding `0`.
  - `height` (integer, optional, `1..=512`): the final pixel height,
    aspect kept (a `contain` fit with the `area` filter). Without it, a
    picture taller than 256 px is scaled to 128 tall and `notes` says
    "scaled to 128 px tall; pass height to choose" — a sane default for
    a standing monster, not a law: pass `height` to choose.
  - `offsets`: the forms the shared parameters document, but resolved
    against the **final** trimmed picture; the default `center_bottom`
    puts the sprite's feet on its origin (why: floor-standing sprites are
    drawn with the origin at the feet). `grAb` chunks are not consulted —
    the pipeline may have moved the picture.
  - `ops`, `dither`, `palette`: as in `process_image`; your `ops` run
    first, then background removal, trim and the height fit.
    Transparency is fixed to alpha below `128`.
  - `replace` (boolean, optional, default `false`): overwrite working
    sprites of these names; a same-name lump only a read-only layer holds
    becomes an override, and rotations other lumps already serve are
    reported as `warnings`, never refusals.
- Returns:

  ```json
  {
    "lumps": ["TROOA1", "TROOB1"],
    "width": 20, "height": 40,
    "left_offset": 10, "top_offset": 40,
    "steps": [ { "op": "remove_background", ... }, { "op": "trim", ... }, { "op": "fit", ... } ],
    "colors_used": 12, "transparent_pixels": 380,
    "preview": { "url": "...", "filename": "TROOA1_sprite.png", ... },
    "warnings": [], "notes": [],
    "next_steps": [ "a new thing type needs a definition: edit_decorate (ZDoom ports) or edit_dehacked to repoint an existing type's frames" ]
  }
  ```

  `lumps` lists every name written, `preview` is the first lump decoded
  back through the sprite decoder.
- Errors: `invalid_argument` (naming conflicts — both selectors, `frame`
  with `frames`, `frames` with `name` or without `sprite`, an empty
  list — a `height` outside `1..=512`, a bad `background_color`, and a
  picture background removal and trim empty entirely: "the whole image
  was removed as background; lower background_tolerance or set
  remove_background false"), `format_error`/`unsupported` (bytes that are
  no readable image), `not_found` (missing `source`, missing `PLAYPAL`,
  bad palette index), `already_exists` (a working twin without `replace`).
- After it lands, `get_sprite`/`list_sprites` read the lumps back; a new
  monster or decoration still needs a definition — `edit_decorate`
  (ZDoom-family) or `edit_dehacked` to repoint an existing type's frames.
