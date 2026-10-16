# Image imports

Turning external picture files into lumps: the patch importer
`import_image_as_patch` (alias `import_png_as_patch`), the flat importer
`import_image_as_flat` (alias `import_png_as_flat`), and the byte-for-byte
`import_image_as_raw`. Where the graphics tools (`reference/graphics.md`)
*publish* session pictures outward, these tools bring pictures *in*, as one
undoable edit each (`undo` steps them back; `save_wad` writes them out).

## Shared parameters

`import_image_as_patch`, `import_png_as_patch`, `import_image_as_flat` and
`import_png_as_flat` share these:

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
- `offsets` (string or object, optional; patch importers only): the drawing
  offsets stored in the patch header. `"zero"`, `"grab"`, `"center"`,
  `"center_bottom"` or an explicit `{ "x": n, "y": n }` pair. Omitted means
  `"grab"`: a `PNG`'s `grAb` chunk when it carries one, else the tool's own
  default — for the patch tools that is `"zero"` (wall patches). `center`
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

- Purpose: quantizes the picture to the session's `PLAYPAL` and stores it
  as the raw row-major palette-index plane a flat is, in the working
  archive's **flats** region (`F_START`/`F_END` markers created there when
  missing), as one undoable edit labelled `import_flat NAME`.
- Arguments: `source`, `name`, `palette` and `replace` as in the shared
  parameters above — plus, instead of `transparency` and `offsets`:
  - `resize` (string, optional, default `"none"`): what to do with a
    picture that is not already a flat shape. `"none"` refuses it;
    `"nearest"` scales it to `size x size`. A picture that already is
    `64x64`, `64x128`, `128x128` or `256x256` is kept as it is.
  - `size` (integer, optional, default 64): the square side for
    `resize="nearest"`: `64`, `128` or `256`; another value is
    `invalid_argument`.
- Returns:

  ```json
  {
    "name": "MYFLAT",
    "lump_index": 1,
    "width": 64,
    "height": 64,
    "resized": false,
    "colors_used": 256,
    "max_color_error": 0,
    "transparent_pixels_ignored": 0
  }
  ```

  `lump_index` is the flat's position in the working archive; `width` and
  `height` are the stored flat's dimensions and `resized` says whether the
  picture was scaled. Flats store no transparency: every pixel stays
  opaque and `transparent_pixels_ignored` counts the source pixels that
  had alpha below 128.
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
