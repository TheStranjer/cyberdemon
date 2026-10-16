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

## Tools

| Tool                    | Reference               |
|-------------------------|-------------------------|
| `version`               | `reference/meta.md`     |
| `get_asset_server_info` | `reference/assets.md`   |
| `set_asset_capacity`    | `reference/assets.md`   |
| `get_thing_types`       | `reference/defs.md`     |
| `get_linedef_specials`  | `reference/defs.md`     |
| `get_sector_specials`   | `reference/defs.md`     |
| `list_lumps`            | `reference/lumps.md`    |
| `search_lumps`          | `reference/lumps.md`    |
| `get_lump`              | `reference/lumps.md`    |
| `extract_lump`          | `reference/lumps.md`    |
| `add_lump`              | `reference/lumps.md`    |
| `import_asset`          | `reference/lumps.md`    |
| `import_file_as_lump`   | `reference/lumps.md`    |
| `replace_lump`          | `reference/lumps.md`    |
| `remove_lump`           | `reference/lumps.md`    |
| `rename_lump`           | `reference/lumps.md`    |
| `reorder_lumps`         | `reference/lumps.md`    |
| `list_maps`             | `reference/maps.md`     |
| `get_map_lumps`         | `reference/maps.md`     |
| `set_active_map`        | `reference/maps.md`     |
| `list_flats`            | `reference/graphics.md` |
| `list_patches`          | `reference/graphics.md` |
| `list_sprites`          | `reference/graphics.md` |
| `get_flat`              | `reference/graphics.md` |
| `get_patch`             | `reference/graphics.md` |
| `get_sprite`            | `reference/graphics.md` |
| `get_thing_sprite`      | `reference/graphics.md` |
| `decode_patch`          | `reference/graphics.md` |
| `import_image_as_patch` | `reference/imports.md`  |
| `import_png_as_patch`   | `reference/imports.md`  |
| `import_image_as_flat`  | `reference/imports.md`  |
| `import_png_as_flat`    | `reference/imports.md`  |
| `import_image_as_raw`   | `reference/imports.md`  |
| `import_sound`          | `reference/audio.md`    |
| `preview_sound`         | `reference/audio.md`    |
| `import_music`          | `reference/music.md`    |
| `preview_music`         | `reference/music.md`    |
| `get_palette`           | `reference/palette.md`  |
| `set_palette`           | `reference/palette.md`  |
| `open_iwad`             | `reference/session.md`  |
| `open_wad`              | `reference/session.md`  |
| `new_wad`               | `reference/session.md`  |
| `merge_wad`             | `reference/session.md`  |
| `save_wad`              | `reference/session.md`  |
| `undo`                  | `reference/session.md`  |
| `redo`                  | `reference/session.md`  |
| `get_session_info`      | `reference/session.md`  |
| `get_wad_info`          | `reference/session.md`  |
| `set_render_resolution` | `reference/render.md`   |
| `set_render_options`    | `reference/render.md`   |
| `get_render_settings`   | `reference/render.md`   |
| `get_dehacked`          | `reference/scripts.md`  |
| `edit_dehacked`         | `reference/scripts.md`  |
| `edit_decorate`         | `reference/scripts.md`  |
| `edit_zscript`          | `reference/scripts.md`  |
| `list_textures`         | `reference/textures.md` |
| `list_pnames`           | `reference/textures.md` |
| `edit_pnames`           | `reference/textures.md` |

## Typical workflow

`open_iwad` the game archive → `open_wad` your project WAD or `new_wad`
for a fresh one → edit the working archive → `save_wad`. `get_session_info`
shows what is loaded and where undo stands; `get_wad_info` summarizes the
merged view an engine would see. Editing a lump the `IWAD` or a resource
layer owns creates an override in the working archive instead of failing
(`replace_lump` reports it); such a lump cannot be deleted, renamed or moved
(`reference/lumps.md`). `set_render_resolution` and
`set_render_options` tune the picture for the whole connection (they are not
part of a WAD and survive every archive change); `get_render_settings` reads
them back.

More WAD reading and editing tools are not implemented yet. When they are,
add one reference file per tool group under `reference/` and index each
tool here.
