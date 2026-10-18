# Patches

The patch lifecycle. `add_patch` puts one patch lump into the working
archive's **patches** region, `replace_patch` swaps the bytes of one the
session already has, and `remove_patch` deletes a working patch — its
`PNAMES` entry and every texture reference to it included, on request.
Where `import_image_as_patch` (`reference/imports.md`) is the one-shot
conversion of a picture file — its name derived from the file, its
`replace` flag overwriting in place — this group treats the region as a
set of lumps you manage: the name is always explicit, and `add_patch`
never overwrites (a working twin is `already_exists`, pointing at
`replace_patch`). Every edit is one undoable step (`undo`/`redo` step
through it, `save_wad` writes the working archive out), and `add_patch`
and `replace_patch` build on exactly the same import parameters as
`reference/imports.md`.

## `add_patch`

- Purpose: stores one patch lump in the working archive's **patches**
  region (`P_START`/`P_END` markers created there when the working
  archive has none) as one undoable edit labelled `add_patch NAME`, and
  by default lists the name in `PNAMES` copy-on-write.
- The passthrough rule: a source that already is a `Doom` patch (valid
  header and column table, and it decodes) is stored **unchanged**;
  `converted` tells you which half ran. `false` is the passthrough — the
  column bodies stay byte-identical — and `true` is the
  `import_image_as_patch` pipeline: decode, quantize to the session's
  `PLAYPAL`, encode the classic column/post format.
- Arguments:
  - `name` (string, required): 1 to 8 printable ASCII characters,
    uppercased. Unlike the importers, `add_patch` never derives a name
    from the file; a bad name is `invalid_argument`.
  - `source` (string, required): absolute path, `file://` URI,
    `http(s)://` URL or this server's asset/upload URL (see
    `reference/assets.md`).
  - `offsets` (string or object, optional, default `"zero"`): `"zero"`,
    `"grab"`, `"center"`, `"center_bottom"` or `{"x": n, "y": n}`, as in
    `reference/imports.md`. On the converted half the rules are the
    importer's: a `PNG` `grAb` chunk wins over the `"zero"` default. On
    the passthrough half only an explicit pair rewrites the two header
    fields — presets and an omitted `offsets` leave the stored header as
    it is. Offsets outside the `i16` range a patch header stores are
    `invalid_argument`.
  - `palette` (integer, optional, default 0) and `transparency`
    (object, optional; `{mode: "alpha", threshold: 128}`,
    `{mode: "color", color: "#00FFFF"}` or `{mode: "none"}`): the
    importer's parameters, acting on the converted half only — a
    passthrough never reads the palette.
  - `add_to_pnames` (boolean, optional, default `true`): also list the
    name in `PNAMES`, copy-on-write into the working archive. A name
    `PNAMES` already lists keeps its index and rewrites nothing.
- Returns:

  ```json
  {
    "name": "MYART",
    "converted": true,
    "width": 64,
    "height": 128,
    "left_offset": 0,
    "top_offset": 0,
    "pnames_index": 2
  }
  ```

  `width`, `height` and both offsets are the **stored patch header's** —
  on the passthrough half that includes offsets the source already
  carried, whichever `offsets` preset was asked for. `pnames_index` is
  `null` when the name is not listed (`add_to_pnames: false` for a new
  name).
- Errors: `invalid_argument` (a bad name, an unknown `offsets` preset or
  `transparency` mode, an offset outside the `i16` range), `not_found`
  (missing `source`; a missing `PLAYPAL` or a palette index it lacks —
  converted half only), `format_error` (bytes that are neither a `Doom`
  patch nor a readable image), `already_exists` (the working archive's
  patches region already holds this name: use `replace_patch`). A name
  only the `IWAD` or a resource layer holds never clashes: the add
  becomes an override.
- Check the result with `list_patches` (the row shows `namespace:
  "patches"`, `source: "working"` and its `in_pnames` state), publish it
  back with `get_patch`, and read the raw bytes with `get_lump` or
  `extract_lump`. `list_pnames` shows the name list. For a picture whose
  name comes from the file, or an overwrite in place, use
  `import_image_as_patch`.

## `replace_patch`

- Purpose: swaps the bytes of one patch the session already has —
  whether the working archive owns it or it comes from the `IWAD`/a
  resource layer — as one undoable edit labelled `replace_patch NAME`.
  `PNAMES` is never touched: `TEXTURE1`/`TEXTURE2` keep working and
  `list_pnames` answers unchanged.
- Arguments: `name` (string, required) plus the same `source`, `offsets`,
  `palette` and `transparency` as `add_patch` above, behaving exactly as
  documented there (the passthrough rule, the pipeline, the `"zero"`
  offset default and the `i16` range). There is no `add_to_pnames` (the
  list is never written) and no `replace` (overwriting is what this tool
  always does).
- How the target resolves: `name` is looked up in the merged view the way
  `list_patches` reads one — the last lump of that name inside a
  `P_START` region, or, when `PNAMES` lists the name, an occurrence
  living outside the flats and sprites regions as well. Nothing matching
  is `not_found`, naming `add_patch` as the way in for a patch that does
  not exist yet.
- Two outcomes, decided before the edit: the working archive's patches
  region already holds the name → its lump is **overwritten in place** at
  its own directory index (`created_override: false`, directory order
  unchanged); only a lower layer holds it → the bytes land as a working
  **override** in the patches region, whose `P_START`/`P_END` markers are
  created there when the working archive has none (`created_override:
  true`). The lower layer's file on disk is never touched.
- Returns:

  ```json
  {
    "name": "WALLP1",
    "converted": true,
    "width": 64,
    "height": 128,
    "left_offset": 0,
    "top_offset": 0,
    "pnames_index": 0,
    "created_override": true,
    "lump_index": 1
  }
  ```

  The first seven fields are `add_patch`'s (their `already_exists` case
  cannot occur here); `pnames_index` is the name's unchanged entry, `null`
  when `PNAMES` does not list it, and `lump_index` is the replaced lump's
  position in the working archive.
- Errors: `not_found` (no such patch in the merged view — see the
  resolution rule; or a missing `source`/`PLAYPAL` on the converted half),
  `invalid_argument` and `format_error` exactly as in `add_patch`. A
  refusal writes nothing.
- Check the swap with `get_patch` and `list_patches` (the row shows
  `source: "working"` after the edit); `undo` hands the merged view back
  to the lower layer and `redo` re-creates the override.

## `remove_patch`

- Purpose: deletes one patch from the **working archive** as one undoable
  edit labelled `remove_patch NAME` — the lump alone, or the lump plus its
  `PNAMES` entry, with every surviving entry shifted down and every
  `TEXTURE1`/`TEXTURE2` patch reference renumbered in the same edit.
  `save_wad` is what makes the removal durable on disk; `undo` steps the
  whole change back and `redo` replays it.
- How the target resolves: `name` is looked up in the working archive
  only — the last lump of that name inside a `P_START` region, or, when
  the merged `PNAMES` lists the name, a working occurrence living outside
  the flats and sprites regions (the `list_patches` candidate rule).
  Nothing answers is `not_found`: only working lumps can be removed, so an
  `IWAD` or resource-layer patch must first be overridden by
  `replace_patch` to gain the working copy (or be left alone).
- Arguments:
  - `name` (string, required): 1 to 8 printable ASCII characters,
    uppercased; anything else is `invalid_argument`.
  - `remove_from_pnames` (boolean, optional, default `false`): also delete
    the `PNAMES` entry, copy-on-write like `edit_pnames remove`, and
    renumber (or drop, together with the entry) every texture patch
    reference in the **same** edit — the lump never leaves without its
    list, and the list never leaves without the lump. A name `PNAMES` does
    not list is no obstacle: `removed_from_pnames` reports `false` and no
    `PNAMES` write happens.
  - `force` (boolean, optional, default `false`): a `PNAMES` removal whose
    entry textures still refer to is `conflict` and removes nothing (the
    message names the offending textures, e.g. `WALL1 (TEXTURE1)`);
    `force=true` accepts dropping those patch references. Without
    `remove_from_pnames` the flag does nothing.
- Returns:

  ```json
  {
    "name": "WALLP1",
    "removed_lump": true,
    "removed_from_pnames": true,
    "textures_changed": 2
  }
  ```

  (That forced example is the classic case: with a `WALL1` referring to
  the removed entry and a `STEP1` at a later index, `WALL1` has its
  references dropped and `STEP1`'s renumber down, so both count.)
  `removed_lump` is `false` only when the name had a `PNAMES` entry but no
  working lump of its own — still a legitimate edit when
  `remove_from_pnames` is set. `textures_changed` counts the textures the
  renumbering pruned or shifted (`0` without a `PNAMES` write).
- Check the result with `list_patches` (the name is gone from the working
  archive), `list_pnames` (the entry list and the shifted indices) and
  `list_textures` (pruned or renumbered patch counts). To put a replaced
  patch back, `add_patch` it again; `undo` restores lump, list and texture
  bytes in one step.
