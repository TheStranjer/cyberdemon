---
name: cyberdemon
description: Use the Cyberdemon MCP server to inspect and edit Doom WAD files. Use when asked to read, understand, or modify a WAD.
---

# Using Cyberdemon

Cyberdemon is an MCP server over stdio for working with Doom WADs. Tools appear in your client prefixed with the server name (e.g. `mcp__cyberdemon__version`).

## Who decides what

The server is the reflex; you are the judgment. Every tool is a deterministic
function of the parameters you pass: it does formulaic work exactly — builds
the shape at the size you said, compiles the plan you wrote, measures the
image you attached — and never guesses what you meant. Interpreting requests
and drawings, choosing shapes, scale, themes and balance, deciding what "make
it scarier" means, and checking renders against intent are your job.

The [guides index](guides/README.md) collects what you need for that side.
Start with `guides/design-rules.md` for the dimensions and gameplay facts to
design against, and run the loop in `guides/verification.md` after every
meaningful change.

## Conventions

- **Successful results are structured JSON.** Read the result's structured
  content; the text content mirrors it. Exception: `version` returns plain
  text (see `reference/meta.md`).
- **Domain failures come back as errored results**, with structured content
  exactly:

  ```json
  {"error": "<code>", "message": "<human-readable text>"}
  ```

  `error` is a stable machine-readable code; `message` is for humans and may
  change. Codes:

  | Code               | Meaning                                                        |
  |--------------------|----------------------------------------------------------------|
  | `invalid_argument` | An argument was missing, malformed or out of range.            |
  | `not_found`        | The requested lump, map or other resource does not exist.      |
  | `already_exists`   | A resource with that name is already taken.                    |
  | `conflict`         | The operation conflicts with current state (e.g. unsaved edits). |
  | `format_error`     | A WAD or other input is malformed or unreadable as data.       |
  | `io_error`         | Reading or writing the filesystem failed.                      |
  | `fetch_error`      | Fetching a remote resource failed.                             |
  | `unsupported`      | The requested feature or format is not supported.              |
  | `limit_exceeded`   | A configured limit (size, count, depth) was exceeded.          |
  | `internal`         | Server-side fault (e.g. a poisoned session lock). Report it; do not retry blindly. |

- **Unknown tools fail at the protocol layer** with the JSON-RPC error
  `INVALID_PARAMS`, not as a tool error. Do not retry; list the tools.

## Files and URLs

Large binary results (images, sounds, raw lumps) do not come through the
stdio channel. Tools return a `url` instead — a file served from the
server's own localhost HTTP origin, e.g.
`http://localhost:25369/assets/3f9a0c1b2d4e5f60/MAP01.png`. The port is
chosen when the first file is published (per session, random in
`3000..=60000`) and is visible in every returned `url`; fetch them with
any HTTP client. Responses carry `Cache-Control: no-store`.

You can upload a local file the same way and pass the returned URL
anywhere a tool accepts a `source`:

```sh
curl -T my-drawing.png http://localhost:25369/upload/my-drawing.png
# -> 201 {"url": "...", "id": "...", "filename": "...", "size": n}
```

`PUT` and `POST` both work; the media type is the request's
`Content-Type`, or the uploaded filename's extension when absent.
Before any file exists, `get_asset_server_info` reports the origin and
the upload URL template.

Tools that take input files accept a `source` string in exactly these
forms, capped at 256 MiB (the same limit as the asset store):

- an absolute local path: `/path/to/image.png`;
- a `file:///path/to/image.png` URI (percent-decoded);
- an `http://` or `https://` URL — fetched with a 60-second timeout and
  at most 5 redirects, its `Content-Type` becomes the result's media
  type;
- a URL from this server's own asset origin (as returned by `publish`
  or `PUT /upload`) — read straight from the store, no HTTP round trip.

Relative paths are rejected with `invalid_argument`; any other URI
scheme with `unsupported`; missing files, evicted assets, non-2xx
responses and oversize sources each report their own error code.

Only the most recent N assets are kept (default 50, set with the
`CYBERDEMON_ASSET_CAPACITY` environment variable or at runtime with
`set_asset_capacity`). Fetching an asset refreshes it, but an evicted one
comes back `404`; regenerate it rather than retrying the old URL.

`get_asset_url` is the one call that turns any session resource — a
texture, flat, patch, sprite, embedded picture, sound, song, `PLAYPAL` or
`COLORMAP` — into such a file without you first deciding which kind it is
(see `reference/assets.md`).

## Tools

| Tool                     | Reference               |
|--------------------------|-------------------------|
| `version`                | `reference/meta.md`     |
| `get_asset_server_info`  | `reference/assets.md`   |
| `get_asset_url`          | `reference/assets.md`   |
| `set_asset_capacity`     | `reference/assets.md`   |
| `get_thing_types`        | `reference/defs.md`     |
| `get_linedef_specials`   | `reference/defs.md`     |
| `get_sector_specials`    | `reference/defs.md`     |
| `list_lumps`             | `reference/lumps.md`    |
| `search_lumps`           | `reference/lumps.md`    |
| `get_lump`               | `reference/lumps.md`    |
| `extract_lump`           | `reference/lumps.md`    |
| `add_lump`               | `reference/lumps.md`    |
| `import_asset`           | `reference/lumps.md`    |
| `import_file_as_lump`    | `reference/lumps.md`    |
| `replace_lump`           | `reference/lumps.md`    |
| `remove_lump`            | `reference/lumps.md`    |
| `rename_lump`            | `reference/lumps.md`    |
| `reorder_lumps`          | `reference/lumps.md`    |
| `list_maps`              | `reference/maps.md`     |
| `get_map_lumps`          | `reference/maps.md`     |
| `get_map_summary`        | `reference/maps.md`     |
| `assemble_map`           | `reference/maps.md`     |
| `set_active_map`         | `reference/maps.md`     |
| `create_map`             | `reference/maps.md`     |
| `copy_map`               | `reference/maps.md`     |
| `rename_map`             | `reference/maps.md`     |
| `remove_map`             | `reference/maps.md`     |
| `convert_map_format`     | `reference/convert.md`  |
| `add_thing`              | `reference/things.md`   |
| `edit_thing`             | `reference/things.md`   |
| `remove_thing`           | `reference/things.md`   |
| `add_linedef`            | `reference/linedefs.md` |
| `edit_linedef`           | `reference/linedefs.md` |
| `remove_linedef`         | `reference/linedefs.md` |
| `split_linedef`          | `reference/linedefs.md` |
| `join_linedefs`          | `reference/linedefs.md` |
| `add_sidedef`            | `reference/sidedefs.md` |
| `edit_sidedef`           | `reference/sidedefs.md` |
| `remove_sidedef`         | `reference/sidedefs.md` |
| `add_sector`             | `reference/sectors.md`  |
| `edit_sector`            | `reference/sectors.md`  |
| `remove_sector`          | `reference/sectors.md`  |
| `create_sector_from_loop` | `reference/sectors.md` |
| `carve_void`             | `reference/sectors.md`  |
| `add_door`               | `reference/features.md` |
| `add_window`             | `reference/features.md` |
| `add_stairs`             | `reference/features.md` |
| `add_lift`               | `reference/features.md` |
| `add_teleporter`         | `reference/features.md` |
| `add_switch`             | `reference/features.md` |
| `add_exit`               | `reference/features.md` |
| `mark_secret`            | `reference/features.md` |
| `set_light_effect`       | `reference/features.md` |
| `build_map_from_plan`    | `reference/plan.md`     |
| `trace_sketch`           | `reference/sketch.md`   |
| `add_vertex`             | `reference/vertices.md` |
| `move_vertex`            | `reference/vertices.md` |
| `remove_vertex`          | `reference/vertices.md` |
| `delete_geometry`        | `reference/geometry.md` |
| `transform_selection`    | `reference/geometry.md` |
| `validate_map`           | `reference/validate.md` |
| `list_flats`             | `reference/graphics.md` |
| `list_patches`           | `reference/graphics.md` |
| `list_sprites`           | `reference/graphics.md` |
| `get_flat`               | `reference/graphics.md` |
| `get_patch`              | `reference/graphics.md` |
| `get_sprite`             | `reference/graphics.md` |
| `get_thing_sprite`       | `reference/graphics.md` |
| `decode_patch`           | `reference/graphics.md` |
| `import_image_as_patch`  | `reference/imports.md`  |
| `import_png_as_patch`    | `reference/imports.md`  |
| `import_image_as_flat`   | `reference/imports.md`  |
| `import_png_as_flat`     | `reference/imports.md`  |
| `import_image_as_sprite` | `reference/imports.md`  |
| `import_png_as_sprite`   | `reference/imports.md`  |
| `import_image_as_raw`    | `reference/imports.md`  |
| `process_image`          | `reference/imports.md`  |
| `create_texture_from_image` | `reference/imports.md` |
| `create_flat_from_image` | `reference/imports.md`  |
| `create_sprite_from_image` | `reference/imports.md` |
| `add_patch`              | `reference/patches.md`  |
| `replace_patch`          | `reference/patches.md`  |
| `remove_patch`           | `reference/patches.md`  |
| `diff_wads`              | `reference/inspect.md`  |
| `optimize_wad`           | `reference/inspect.md`  |
| `import_sound`           | `reference/audio.md`    |
| `preview_sound`          | `reference/audio.md`    |
| `import_music`           | `reference/music.md`    |
| `preview_music`          | `reference/music.md`    |
| `get_nodes`              | `reference/nodes.md`    |
| `get_reject`             | `reference/nodes.md`    |
| `get_blockmap`           | `reference/nodes.md`    |
| `build_nodes`            | `reference/nodes.md`    |
| `build_all_nodes`        | `reference/nodes.md`    |
| `build_blockmap`         | `reference/nodes.md`    |
| `build_reject`           | `reference/nodes.md`    |
| `get_palette`            | `reference/palette.md`  |
| `set_palette`            | `reference/palette.md`  |
| `open_iwad`              | `reference/session.md`  |
| `open_wad`               | `reference/session.md`  |
| `new_wad`                | `reference/session.md`  |
| `merge_wad`              | `reference/session.md`  |
| `save_wad`               | `reference/session.md`  |
| `undo`                   | `reference/session.md`  |
| `redo`                   | `reference/session.md`  |
| `get_session_info`       | `reference/session.md`  |
| `get_wad_info`           | `reference/session.md`  |
| `run_batch`              | `reference/batch.md`    |
| `render_topdown`         | `reference/render.md`   |
| `render_minimap`         | `reference/render.md`   |
| `render_view`            | `reference/render.md`   |
| `set_render_resolution`  | `reference/render.md`   |
| `set_render_options`     | `reference/render.md`   |
| `get_render_settings`    | `reference/render.md`   |
| `get_dehacked`           | `reference/scripts.md`  |
| `edit_dehacked`          | `reference/scripts.md`  |
| `edit_decorate`          | `reference/scripts.md`  |
| `edit_zscript`           | `reference/scripts.md`  |
| `list_textures`          | `reference/textures.md` |
| `list_pnames`            | `reference/textures.md` |
| `edit_pnames`            | `reference/textures.md` |
| `build_texture`          | `reference/textures.md` |
| `remove_texture`         | `reference/textures.md` |
| `get_texture_theme`      | `reference/themes.md`   |
| `set_texture_theme`      | `reference/themes.md`   |
| `auto_texture`           | `reference/themes.md`   |
| `align_textures`         | `reference/themes.md`   |

## Typical workflow

`open_iwad` the game archive → `open_wad` your project WAD or `new_wad`
for a fresh one → edit the working archive → `build_all_nodes` after the
geometry edits (one undoable step rebuilding the BSP tree, the `REJECT`
matrix and the `BLOCKMAP` so vanilla `Doom` can play the map;
`reference/nodes.md`) → `validate_map` → `save_wad`.
`validate_map` reads a map's health — dangling references, broken lines,
missing starts, specials outside the tables, unclosed or crossing
geometry, textures and flats the session does not define — as
severity-ranked issues you fix with the editing tools before saving
(`reference/validate.md`).
`get_session_info`
shows what is loaded and where undo stands; `get_wad_info` summarizes the
merged view an engine would see. Building a level starts with `create_map`
(an empty map block in the working archive; `copy_map` clones an existing
one; `reference/maps.md`) — or one plan describes the whole playable
map: `build_map_from_plan` compiles geometry (rooms, connections, thick
walls, a theme), things and behaviour features (wired doors, switches,
lifts, teleporters, exits, light effects, secrets) into a finished,
thing-inhabited block in one undoable edit, warns about the judgment
gaps it cannot fix (a lock without its key, an unswitched remote, no
exit) and with `finalize: true` builds its node data and reports an
errors-only validation in the same call (`reference/plan.md`). A hand-drawn
floor plan comes in through `trace_sketch`, which needs no open WAD: it
measures the drawing's regions, gaps, marks and adjacency in original-image
pixels and publishes a debug image labelled `R1`, `G1`, `M1` — you decide
what those are, rename `plan_rooms` into rooms, add connections at the gap
centers and build with the same `units` (`reference/sketch.md`). Draw whole
rooms in one call with `create_sector_from_loop`, and carve solid rock out
of the map with `carve_void` (`reference/sectors.md`). Put working doors in
with `add_door`, glaze the rooms' shared walls with `add_window`, climb
height changes with `add_stairs`, ride the tall ones down
with `add_lift`, step between distant rooms with `add_teleporter`, wire
their triggers with `add_switch`, and end the level
with `add_exit` — a switch panel or walk-over lines; hide a room with
`mark_secret` and give it flickering or strobing light with
`set_light_effect` (`reference/features.md`).
Editing a lump
the `IWAD` or a resource layer owns creates an override in the working
archive instead of failing (`replace_lump` reports it); such a lump cannot
be deleted, renamed or moved (`reference/lumps.md`). `render_topdown` draws
the map so you can look at it, `render_minimap` gives you a small automap
square for quick orientation, and `render_view` casts a first-person frame
so you can see what you built (`reference/render.md`);
`set_render_resolution` and
`set_render_options` tune the picture for the whole connection (they are not
part of a WAD and survive every archive change); `get_render_settings` reads
them back.

More WAD reading and editing tools are not implemented yet. When they are,
add one reference file per tool group under `reference/` and index each
tool here.
