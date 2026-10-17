# Patches

The patch lifecycle. `add_patch` puts one patch lump into the working
archive's **patches** region; overwriting an existing patch and deleting
one (`replace_patch`, `remove_patch`) complete the lifecycle as they
arrive. Where `import_image_as_patch` (`reference/imports.md`) is the
one-shot conversion of a picture file — its name derived from the file,
its `replace` flag overwriting in place — this group treats the region as
a set of lumps you manage: the name is always explicit, and `add_patch`
never overwrites (a working twin is `already_exists`, pointing at
`replace_patch`). Every edit is one undoable step (`undo`/`redo` step
through it, `save_wad` writes the working archive out), and `add_patch`
builds on exactly the same import parameters as `reference/imports.md`.

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
