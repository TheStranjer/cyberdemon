# Graphics tools

Browsing the session's graphic lumps: `list_flats`, `list_patches` and
`list_sprites`. All three read the **merged view** — the same directory an
engine sees — and report measurements only: names, sizes, dimensions,
drawing offsets and grouping. They never decode pixels and never edit;
use the lump tools (`reference/lumps.md`) for content and
`reference/textures.md` for the `PNAMES`/`TEXTURE1` definitions.

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
