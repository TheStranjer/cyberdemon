# Textures tools

Reading and maintaining the composite-texture definitions: patch lumps,
the `PNAMES` name index and the `TEXTURE1`/`TEXTURE2` texture records.

## How textures fit together

A `Doom` TEXTUREx entry is a named rectangle of pixels the engine tiles
along a linedef or uses as a middle/upper/lower texture. It is not an
image: it is a *recipe* of patch images.

- **Patch lumps** (`WALLP1`, `STEPP1`, ...) are `Doom` column-post images,
  conventionally stored between `P_START`/`P_END` markers.
- **`PNAMES`** is the ordered list of patch *names*. Position in the list
  is the patch's index; texture records refer to patches by that number,
  never by name.
- **`TEXTURE1`/`TEXTURE2`** hold the texture records: name, width, height
  and a list of mappatch placements (patch index + x/y origin). A texture
  is built by drawing each patch at its origin, clipping to the
  rectangle. Both lumps define textures the same way; `TEXTURE2` exists
  only to extend the list (and its entries win name lookups here).
- Two layouts are read: the vanilla `Doom` record (with the obsolete
  column-directory word and `stepdir`/`colormap` fields) and the shorter
  `Strife` record. The layout is detected per lump and preserved on
  write; unknown-but-valid ZDoom flag words are kept byte-for-byte.

So the tools cannot "add a texture" from an image: you put the patch lump
in the archive (lump tools), make sure `PNAMES` names it
(`list_pnames` / `edit_pnames`), and reference it from a texture record.

## `list_textures`

- Purpose: lists the composite textures defined by the merged view's
  `TEXTURE1` and `TEXTURE2` lumps, in lump order.
- Arguments:
  - `pattern` (string, optional): case-insensitive texture-name glob
    (`*` any run, `?` exactly one), e.g. `"WALL*"`.
  - `offset` (number, optional, default 0): matches skipped before the page.
  - `limit` (number, optional, default 200, max 2000): rows per page.
- Returns:

  ```json
  {
    "total": 2,
    "textures": [
      {"name": "WALL1", "width": 64, "height": 128, "patches": 32,
       "lump": "TEXTURE1", "source": {"layer": 0}}
    ]
  }
  ```

  `lump` is `"TEXTURE1"` or `"TEXTURE2"`, `source` is `list_lumps`'
  archive origin (`"working"` or `{"layer": N}`). `total` counts pattern
  matches before paging. A session without either lump answers
  `{"total": 0, "textures": []}` — not an error.
- Errors: `invalid_argument` for a `limit` outside 1..2000;
  `format_error` when a texture lump is neither layout.

## `list_pnames`

- Purpose: lists the merged view's `PNAMES` entries in index order.
- Arguments: `pattern`, `offset`, `limit` as in `list_textures`.
- Returns:

  ```json
  {
    "total": 2,
    "pnames": [
      {"index": 0, "name": "WALLP1", "lump_found": true},
      {"index": 1, "name": "STEPP1", "lump_found": true}
    ]
  }
  ```

  `index` is the number texture records refer to; `lump_found` says
  whether a lump with that name exists in the merged view *outside the
  flats and sprites namespaces* — false means the index points at a name
  with no image behind it. No `PNAMES` lump answers `{"total": 0,
  "pnames": []}`, not an error.
- Errors: as `list_textures`, plus `format_error` for a malformed
  `PNAMES`.

## `edit_pnames`

- Purpose: changes the `PNAMES` list and keeps every texture reference
  truthful, as one undoable edit (`save_wad` writes the result).
- Arguments — pass **exactly one** operation:
  - `add` (list of names): appends; entries already present are skipped
    and reported in `skipped`.
  - `remove` (list of names) + `force` (bool, optional, default false):
    deletes entries.
  - `rename` (list of `{from, to}`): changes names in place; indices stay
    put, so textures are untouched.
  - `replace_all` (list of names): the new complete list (an empty list
    clears `PNAMES`; duplicates are `invalid_argument`).
- Index bookkeeping: removing, adding before, or replacing shifts
  indices, so surviving texture patch references are renumbered by name
  and changed `TEXTUREx` lumps are rewritten in the layout they were
  read in. A removed/replaced-away name that a texture still refers to
  fails with `conflict` listing the affected textures — pass
  `force: true` to drop those patch references instead.
- Copy-on-write: only the working archive changes. `PNAMES` and each
  changed `TEXTUREx` lump is brought in as an override (over an existing
  working copy in place, else appended); the `IWAD` and resource layers
  never change. All written lumps are ONE undoable edit labelled
  `edit_pnames <operation>`. A no-op (everything skipped) records
  nothing.
- Names: 1 to 8 printable `ASCII` characters, stored uppercase; invalid
  names fail with `invalid_argument`.
- Returns:

  ```json
  {"pnames": 3, "added": ["NEWP1"], "skipped": [], "removed": [],
   "renamed": [], "textures_changed": 0}
  ```

  `pnames` is the entry count after the edit; `textures_changed` counts
  texture definitions renumbered or pruned (0 for `add`/`rename`).
- Errors: `invalid_argument` (zero or several operations, empty
  `add`/`remove`/`rename`, bad or duplicated names, limit-like problems);
  `not_found` (a `rename` source that is not in `PNAMES`); `conflict`
  (dropped references without `force`, or a `rename` onto a taken name);
  `format_error` (a texture lump that cannot be decoded);
  `limit_exceeded` (an oversized list).
- Examples:

  ```json
  {"add": ["DOOR1"]}
  {"remove": ["OLDPAT"], "force": true}
  {"rename": [{"from": "WALLP1", "to": "WALLPB"}]}
  {"replace_all": ["WALLP1", "STEPP1", "DOOR1"]}
  ```

  Check the result with `list_pnames` / `list_textures`; read a raw lump
  back with `get_lump`.
