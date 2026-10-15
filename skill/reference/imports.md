# Image imports

Turning external picture files into lumps: the patch importer
`import_image_as_patch` (alias `import_png_as_patch`) and the byte-for-byte
`import_image_as_raw`. Where the graphics tools (`reference/graphics.md`)
*publish* session pictures outward, these tools bring pictures *in*, as one
undoable edit each (`undo` steps them back; `save_wad` writes them out).

## Shared parameters

`import_image_as_patch` and `import_png_as_patch` take the same set:

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
- `transparency` (object, optional): which source pixels become patch
  holes. `{ "mode": "alpha", "threshold": 128 }` (the default) drops pixels
  whose alpha is below `threshold`; `{ "mode": "color", "color":
  "#RRGGBB" }` drops pixels equal to `color` exactly (default `"#00FFFF"`,
  the cyan Doom sprite sources are keyed against); `{ "mode": "none" }`
  keeps every pixel opaque. Fields irrelevant to the chosen mode are
  ignored.
- `offsets` (string or object, optional): the drawing offsets stored in the
  patch header. `"zero"`, `"grab"`, `"center"`, `"center_bottom"` or an
  explicit `{ "x": n, "y": n }` pair. Omitted means `"grab"`: a `PNG`'s
  `grAb` chunk when it carries one, else the tool's own default — for the
  patch tools that is `"zero"` (wall patches). `center` is `(width/2,
  height/2)` and `center_bottom` is `(width/2, height)` in the Doom offset
  convention: a positive left offset shifts the image left, and the top
  offset is the distance from the image's top edge down to the origin.
  Offsets outside the `-32768..=32767` a patch header stores are
  `invalid_argument`.
- `replace` (boolean, optional, default `false`): a working-archive lump of
  this name in the target region is `already_exists` unless `replace`
  overwrites it in place. A same-name lump only the `IWAD` or a resource
  layer holds never clashes: the import becomes an override.

The result always measures what was stored: `width`, `height`,
`left_offset`, `top_offset`, `transparent_pixels` (holes the quantization
dropped), `colors_used` (distinct opaque source colours) and
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
