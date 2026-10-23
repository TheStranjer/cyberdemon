# Graphics tools

Browsing the session's graphic lumps: the listings `list_flats`,
`list_patches` and `list_sprites`, and the image tools `get_flat`,
`get_patch`, `get_sprite`, `get_thing_sprite` and `decode_patch`. All of
them read the **merged view** — the same directory an engine sees. The
listings report measurements only: names, sizes, dimensions, drawing
offsets and grouping.
The image tools decode pixels and publish them as files on the asset
origin (see `reference/assets.md`), never inline. Use the lump tools
(`reference/lumps.md`) for raw content and `reference/textures.md` for the
`PNAMES`/`TEXTURE1` definitions.

## Flats, patches and sprites

`Doom` archives hold three distinct kinds of graphic lumps, told apart by
the marker region they sit in (all aliases included):

- **Flats** (between `F_START`/`F_END` or `FF_START`/`FF_END`) are the
  tiled wall, floor and ceiling textures: raw `PLAYPAL` index planes, no
  header and no transparency. The vanilla shapes are 4096 bytes = 64x64,
  8192 = 64x128, 16384 = 128x128 and 65536 = 256x256; modern ports also
  accept a `PNG`-encoded flat, which is recognised by its signature.
- **Patches** (between `P_START`/`P_END` or `PP_START`/`PP_END`) are
  column-post images with drawing offsets: the components `TEXTURE1` and
  `TEXTURE2` composite into textures, and `PNAMES` numbers them by name.
  Ports also carry patches (and HUD art) as `PNG` lumps, optionally with
  the offsets in a `grAb` chunk. A patch that `PNAMES` names may sit
  anywhere in the archive; the listings count those too.
- **Sprites** (between `S_START`/`S_END` or `SS_START`/`SS_END`) are the
  same patch format, but addressed by name rather than by `PNAMES` index,
  one lump per angle.

When a name occurs more than once in a region, the **last lump wins**
(the merged view resolves engine lookups that way), so every listing
shows one row per name.

## The sprite name grammar

A sprite lump name is four characters of sprite (`TROO`), a frame letter
(`A`-`Z`, then `[`, `\`, `]` — 29 frames) and a rotation digit `0`-`8`.
Rotation `0` serves every angle with the one image. An eight-character
name appends a second frame+rotation pair served by the **same image
horizontally mirrored**: `POSSA2A8` serves rotation 2 of frame `A`
directly and rotation 8 of frame `A` flipped. Names outside this grammar
still show up — in the `invalid` list — because broken ones are common
in real WADs.

## `list_flats`

- Purpose: lists the flat lumps of the merged view's flats regions.
- Arguments:
  - `pattern` (string, optional): case-insensitive name glob (`*` any
    run, `?` exactly one), e.g. `"LAVA*"`.
  - `offset` (number, optional, default 0): matches skipped before the page.
  - `limit` (number, optional, default 200, max 2000): rows per page.
- Returns:

  ```json
  {
    "total": 2,
    "flats": [
      {"name": "FLOOR1", "size": 4096, "width": 64, "height": 64,
       "format": "raw", "source": {"layer": 0}},
      {"name": "PNGFLAT", "size": 118, "width": 64, "height": 64,
       "format": "png", "source": "working"}
    ]
  }
  ```

  `format` is `"png"` when the bytes carry a `PNG` signature — then the
  dimensions come from the `IHDR` — and `"raw"` otherwise, where the four
  vanilla sizes answer; an unsupported size reports `width`/`height`
  `null`. `source` is `list_lumps`' archive origin. A session without
  flats answers `{"total": 0, "flats": []}` — not an error. `total`
  counts pattern matches before paging.
- Errors: `invalid_argument` for a `limit` outside 1..2000.

## `list_patches`

- Purpose: lists the patch lumps: every lump in a patches region, plus
  every lump `PNAMES` names from outside the flats and sprites regions.
- Arguments:
  - `pattern` (string, optional): case-insensitive name glob.
  - `offset` / `limit`: as in `list_flats`.
  - `in_pnames_only` (boolean, optional, default `false`): restrict the
    listing to names `PNAMES` holds.
- Returns:

  ```json
  {
    "total": 2,
    "patches": [
      {"name": "WALLP1", "size": 352, "width": 16, "height": 16,
       "left_offset": -2, "top_offset": -14, "format": "doom",
       "in_pnames": true, "namespace": "patches", "source": {"layer": 0}}
    ]
  }
  ```

  `format` is `"doom"` when the first eight bytes read as a `Doom` patch
  header (dimensions and offsets come from the header; the column bodies
  are never touched), `"png"` for a `PNG` lump (dimensions from `IHDR`,
  offsets from a `grAb` chunk when present) and `"unknown"` otherwise,
  where all measurements are `null`. `in_pnames` says whether `PNAMES`
  lists the name; `namespace` is `"patches"` inside a patches region and
  `null` elsewhere. A session without patches answers
  `{"total": 0, "patches": []}`; a malformed `PNAMES` fails with
  `format_error`.
- Errors: `invalid_argument` for a `limit` outside 1..2000;
  `format_error` for a malformed `PNAMES`.

## `list_sprites`

- Purpose: groups the merged view's sprite-region lumps by sprite.
- Arguments:
  - `prefix` (string, optional): keep one sprite — exactly four `ASCII`
    letters or digits, case-insensitive (e.g. `"POSS"`). Anything else
    fails with `invalid_argument`; leave it out to list every sprite.
  - `offset` / `limit`: paging over **sprite groups** (not lumps).
- Returns:

  ```json
  {
    "total": 1,
    "sprites": [
      {"sprite": "TROO", "frames": [
        {"frame": "A", "rotations": [
          {"rotation": 1, "lump": "TROOA1", "mirrored": false},
          {"rotation": 2, "lump": "TROOA2A8", "mirrored": false},
          {"rotation": 8, "lump": "TROOA2A8", "mirrored": true}
        ]}
      ]}
    ],
    "invalid": ["BADNAME"]
  }
  ```

  Groups are sorted by sprite, frames by letter, rotations by number
  then lump name. `mirrored: true` means the engine must flip the image
  horizontally for that rotation. `invalid` lists the region lumps whose
  names match no grammar, in directory order (a `prefix` keeps only
  invalid names starting with it). `total` counts the matching groups
  before paging. A session without sprites answers
  `{"total": 0, "sprites": [], "invalid": []}` — not an error.
- Errors: `invalid_argument` for a malformed `prefix` or a `limit`
  outside 1..2000.

## Pictures as assets

`get_flat`, `get_patch`, `get_sprite`, `get_thing_sprite` and
`decode_patch` decode pixels.
Nothing is inlined and nothing is edited: each publishes a file on the
asset origin and returns its `AssetRef`, so you fetch the `url` with any
HTTP client. Three arguments shape every picture result:

- `palette` (number, default 0): which `PLAYPAL` entry colours the
  picture. An index the `PLAYPAL` does not hold is `not_found`, as is a
  session with no `PLAYPAL` at all (`open_iwad` first).
- `format` (string, default `"png"`): `"png"` publishes `NAME.png`
  (`image/png`), with the drawing offsets kept in a `grAb` chunk when the
  picture has any; `"rgba"` publishes `NAME.rgba`
  (`application/octet-stream`) as raw row-major `RGBA8` bytes, exactly
  `width*height*4` of them.
- `scale` (number, default 1, `1..=8`): whole-pixel nearest-neighbour
  upscale applied before publishing. It multiplies the reported
  dimensions *and* the reported drawing offsets, so the numbers always
  describe the published file.

Both stored encodings read: the vanilla `Doom` formats, and the `PNG` form
ports carry (high-resolution flats, HUD art, ZDoom sprite lumps). A `PNG`
is quantized to `PLAYPAL` entry **0** — pixels with alpha below 128 become
transparent holes — and is then coloured with the `palette` you asked for,
so `scale` and `palette` are how you inspect a picture without touching
the archive. Transparent pixels are published as holes, not as index 0.
Name resolution follows the engine: within the region the **last** lump of
a name wins.

## `get_flat`

- Purpose: publishes one flat (wall/floor/ceiling) texture.
- Arguments:
  - `name` (string, required): the flat's lump name, case-insensitive.
    The last flats-region lump of that name wins; failing that, any other
    lump of that name.
  - `palette` / `format` / `scale`: as described above.
- Returns:

  ```json
  {
    "name": "FLOOR1", "width": 64, "height": 64,
    "asset": {"url": "http://localhost:PORT/assets/ID/FLOOR1.png",
              "filename": "FLOOR1.png", "mime": "image/png", "size": 194}
  }
  ```

  Flats store no drawing offsets, so the result carries none.
- Errors: `not_found` for a name no archive holds (`list_flats` shows the
  names) or an out-of-range `palette`; `format_error` for bytes that are
  neither a supported raw size nor a readable `PNG`; `invalid_argument`
  for a bad `format`, `scale` or name.

## `get_patch`

- Purpose: publishes one patch lump — a texture component, HUD graphic or
  port graphic.
- Arguments:
  - `name` (string, required): the patch's lump name, case-insensitive.
    The last patches-region lump wins; failing that, the last loose lump
    of that name from outside the flats and sprites regions (`PNAMES` can
    name a patch anywhere).
  - `palette` / `format` / `scale`: as described above.
- Returns:

  ```json
  {
    "name": "WALLP1", "width": 16, "height": 16,
    "left_offset": -2, "top_offset": -14,
    "asset": {"url": "...", "filename": "WALLP1.png", "mime": "image/png",
              "size": 371}
  }
  ```

  `left_offset` and `top_offset` say where the image's top-left corner
  sits relative to the drawing point, in published pixels; a `PNG` asset
  carries the same pair in a `grAb` chunk.
- Errors: as `get_flat`, with `not_found` naming `list_patches`. Read the
  raw bytes with `get_lump` or `extract_lump`.

## `get_sprite`

- Purpose: publishes one sprite rotation.
- Arguments — exactly one of `name` and `sprite`:
  - `name` (string, optional): a whole sprite lump name, served **as
    stored**: `"TROOA1"`, or `"TROOA2A8"` for that lump's own unflipped
    image.
  - `sprite` (string, optional): the four-character prefix, e.g. `"TROO"`.
  - `frame` (string, optional, default `"A"`): one frame letter.
  - `rotation` (number, optional, default 1): `0`-`8`.
  - `palette` / `format` / `scale`: as described above.
- Returns:

  ```json
  {
    "name": "TROOA8", "lump": "TROOA2A8", "mirrored": true,
    "width": 8, "height": 8, "left_offset": 8, "top_offset": 0,
    "asset": {"url": "...", "filename": "TROOA8.png", "mime": "image/png",
              "size": 402}
  }
  ```

  A `sprite`/`frame`/`rotation` request resolves the way an engine does:
  the lump named for that rotation wins, then a lump whose eight-character
  name carries this rotation as **its own pair** — `TROOA2A8` is where an
  `IWAD` keeps frame `A` rotation `2`, published as stored — then one that
  carries it as its **mirror pair**, published flipped horizontally with
  `left_offset` mirrored to `width - left_offset` — then the frame's
  rotation-`0` all-angles image, so `PLAYA0` answers every rotation of
  frame `A`. `name` is the rotation you asked for, `lump` the name whose
  pixels served it, and `mirrored` says whether they were flipped.
  Rotations are looked for in the sprite regions; a whole
  `name` also falls back to the patches region and to loose lumps.
- Errors: `invalid_argument` for both selectors or neither, a malformed
  `sprite`, `frame` or `rotation`; `not_found` for a sprite nothing serves
  (`list_sprites` groups what exists); `format_error` for a lump that is no
  picture.

## `get_thing_sprite`

- Purpose: publishes what one thing type looks like in game — the sprite
  rotation an engine shows — as an image asset.
- Arguments:
  - `type` (number, required): the thing type number, as editors write it
    into `THINGS` (e.g. `3001`, the Imp). Looked up in the **session
    definitions**: the built-in table of the session's game (DOOM II when
    unidentified) overlaid with the merged view's DEHACKED, DECORATE,
    ZScript and MAPINFO custom types — load the archives first.
  - `frame` (string, optional): one frame letter; **default: the
    definition's own frame**.
  - `rotation` (number, optional, default 1): `1..=8` — the eight views,
    `1` the front a head-on viewer sees, `5` the back. (No rotation `0`
    here; `get_sprite` serves that directly.)
  - `palette` / `format` / `scale`: as described above.
- Returns:

  ```json
  {
    "type": 3001, "name": "Imp", "sprite": "TROO", "frame": "A",
    "rotation": 8, "lump": "TROOA2A8", "mirrored": true,
    "width": 8, "height": 8, "left_offset": 8, "top_offset": 0,
    "hanging": false,
    "asset": {"url": "...", "filename": "TROOA8.png", "mime": "image/png",
              "size": 402}
  }
  ```

  The rotation resolves exactly as `get_sprite` does: exact lump, then
  the pair name that carries it as its own rotation, then that name
  flipped for its mirror rotation, then the frame's rotation-`0`
  all-angles image.
  `name` is the definition's display name, `lump` the sprite lump whose
  pixels served, `mirrored` whether they were flipped, `hanging` whether
  the thing hangs from the ceiling.
- Errors: `not_found` for an unknown type (`get_thing_types` lists what
  exists), for a definition without a sprite ("thing type N has no
  sprite") and for a frame/rotation no lump serves — naming the expected
  pattern, e.g. `TROOA1 / TROOA0`; `invalid_argument` for a malformed
  `frame` or a `rotation` outside 1..8.

  To see a thing *from a specific viewpoint*, compute the rotation with
  the engine rule the server documents (`(angle_viewer_to_thing −
  thing_angle + 202.5°) bucketed into eighths`, `1` = front): viewer due
  east of an east-facing thing shows rotation 1, due west shows 5, due
  north shows 3.

## `decode_patch`

- Purpose: decodes one **`Doom`-format** patch — from a session lump or
  from an external file — and publishes it *both* ways, so you pick the
  encoding afterwards. Use it to read a patch file that is not in a WAD
  yet, or to check the bytes you wrote into a `P_START` lump.
- Arguments — exactly one of `name` and `source`:
  - `name` (string, optional): a lump, resolved anywhere in the merged
    view.
  - `source` (string, optional): absolute path, `file://` URI,
    `http(s)://` URL or this server's asset URL (see
    `reference/assets.md`).
  - `palette` (number, optional, default 0): colours both published
    pictures.
- Returns:

  ```json
  {
    "name": "STEPP1", "width": 16, "height": 8,
    "left_offset": 0, "top_offset": 0, "palette": 0,
    "png": {"url": "...", "filename": "STEPP1.png", "mime": "image/png",
            "size": 118},
    "rgba": {"url": "...", "filename": "STEPP1.rgba",
             "mime": "application/octet-stream", "size": 512}
  }
  ```

  `name` is the lump name, or the fetched file's stem uppercased to a lump
  name when it allows one. There is no `format` or `scale`: the tool
  always answers with both encodings at the stored size.
- Errors: `invalid_argument` for both `name` and `source` or neither;
  `not_found` for a missing lump or file; `format_error` for bytes that
  are no `Doom` patch — `PNG` lumps and files are not read here, use
  `get_patch` for those.
