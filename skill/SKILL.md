---
name: cyberdemon
description: Use the Cyberdemon MCP server to inspect and edit Doom WAD files. Use when asked to read, understand, or modify a WAD.
---

# Using Cyberdemon

## Start here

Cyberdemon reads and edits Doom WAD files over MCP: maps, graphics, audio and game definitions, with every edit undoable.
Start a session with open_iwad (the game data), then new_wad for a fresh project archive or open_wad for an existing file.
Prefer high-level tools over element edits: build_map_from_plan compiles a whole playable map from one plan; add_door, add_window, add_stairs, add_lift, add_teleporter, add_switch and add_exit place working features; create_texture_from_image, create_flat_from_image and create_sprite_from_image turn pictures into finished assets; bulk_edit changes many elements at once; place_things populates rooms.
The element tools (add_linedef, add_sector, add_vertex, edit_thing, ...) are fine control, not first choices.
For a hand-drawn sketch: trace_sketch measures it (no WAD needed), turn the reported regions and gaps into a plan, build_map_from_plan compiles it, then render_topdown with an underlay draws the picture behind the lines.
Verify after every meaningful change: validate_map for map health, check_playability for whether a player can finish, render_topdown and render_view to look.
Use run_batch to make a multi-step change atomic and reversible as one step; undo (and redo) take changes back.
Failed calls return an error object with a hint field: do what the hint says before retrying.
Image and sound results are URLs on the server's localhost asset origin - open them to look or listen.
The skill's guides/ directory holds the judgment guides: guides/design-rules.md, guides/verification.md, guides/recipes.md, guides/sketch-to-map.md, guides/editing-existing-maps.md and guides/images-to-assets.md.

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
  {"error": "<code>", "message": "<human-readable text>", "hint": "<what to do next>"}
  ```

  `error` is a stable machine-readable code; `message` is for humans and may
  change; `hint` is one sentence telling you the next call or parameter that
  fixes the situation. Read `hint` first. Codes, each with the default hint
  used when a tool site gives no more specific one:

  | Code               | Meaning and default hint                                             |
  |--------------------|----------------------------------------------------------------------|
  | `not_found`        | A lump, map or other resource does not exist: use a list_* / find_elements / describe_map tool to see what exists. |
  | `invalid_argument` | An argument is missing, malformed or out of range: check the parameter against the tool's input schema. |
  | `already_exists`   | A resource name is taken: pass replace: true or choose another name. |
  | `conflict`         | The operation conflicts with current state (e.g. unsaved edits): reload the state (get_session_info) and retry. |
  | `format_error`     | An input is malformed: the input is not a valid file of the expected kind. |
  | `io_error`         | Filesystem failure: use an absolute path or an http(s) URL; an image that only exists in the conversation must first be saved to a file by the user. |
  | `fetch_error`      | A remote fetch failed: same guidance as `io_error`.                   |
  | `unsupported`      | A feature or format is not supported: see skill/reference for supported formats. |
  | `limit_exceeded`   | A configured limit was exceeded: narrow the selection or raise the limit parameter. |
  | `internal`         | Server-side fault (e.g. a poisoned session lock): this is a bug; undo the last change if the map looks wrong. Do not retry blindly. |

  Where a tool site can say more precisely what to do (an unknown map, an
  out-of-range element index, an ambiguous thing name, a plan whose rooms do
  not touch), it attaches that specific hint instead of the code default.
  `run_batch` reports `hint: null` only for a step that failed at the
  protocol layer, before any tool's own error ran.

- **Unknown tools fail at the protocol layer** with the JSON-RPC error
  `INVALID_PARAMS`, not as a tool error. Do not retry; list the tools.
- **Every edit is one undoable step.** `undo`/`redo` walk the labelled
  history (newest 100 edits; `get_session_info` reports where it stands);
  results echo `copied_to_working` and `nodes_invalidated` so you know what
  the step touched (`reference/session.md`). `run_batch` executes several
  tool calls as ONE atomic step that rolls back whole on the first failing
  one, and chains results with `$ref` — use it for any multi-call change you
  want to be able to take back at once (`reference/batch.md`).
- **Edits only touch the working archive.** The `IWAD` and any resource
  layers (`merge_wad mode: layer`) are read-only; editing a layer-owned map
  copies its block into the working archive first (copy-on-write, reported
  as `copied_to_working: true`), and reads answer from the merged view —
  what an engine would see — unless you pass a `scope` (`reference/session.md`,
  `reference/maps.md`).

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

Every tool, grouped by its tool group (`assets`, `session`, `query`, ...).
Each group's full parameter and result contract lives in its `reference/`
page; read that page before using a tool whose result shape you do not
know yet.

| Tool                      | Group         | Reference                |
|---------------------------|---------------|--------------------------|
| `get_asset_server_info`     | assets        | `reference/assets.md`      |
| `get_asset_url`             | assets        | `reference/assets.md`      |
| `set_asset_capacity`        | assets        | `reference/assets.md`      |
| `import_sound`              | audio         | `reference/audio.md`       |
| `preview_sound`             | audio         | `reference/audio.md`       |
| `run_batch`                 | batch         | `reference/batch.md`       |
| `convert_map_format`        | convert       | `reference/convert.md`     |
| `get_linedef_specials`      | defs          | `reference/defs.md`        |
| `get_sector_specials`       | defs          | `reference/defs.md`        |
| `get_thing_types`           | defs          | `reference/defs.md`        |
| `delete_geometry`           | edit_geometry | `reference/geometry.md`    |
| `transform_selection`       | edit_geometry | `reference/geometry.md`    |
| `add_linedef`               | edit_linedefs | `reference/linedefs.md`    |
| `edit_linedef`              | edit_linedefs | `reference/linedefs.md`    |
| `join_linedefs`             | edit_linedefs | `reference/linedefs.md`    |
| `remove_linedef`            | edit_linedefs | `reference/linedefs.md`    |
| `split_linedef`             | edit_linedefs | `reference/linedefs.md`    |
| `add_sector`                | edit_sectors  | `reference/sectors.md`     |
| `carve_void`                | edit_sectors  | `reference/sectors.md`     |
| `create_sector_from_loop`   | edit_sectors  | `reference/sectors.md`     |
| `edit_sector`               | edit_sectors  | `reference/sectors.md`     |
| `remove_sector`             | edit_sectors  | `reference/sectors.md`     |
| `add_sidedef`               | edit_sidedefs | `reference/sidedefs.md`    |
| `edit_sidedef`              | edit_sidedefs | `reference/sidedefs.md`    |
| `remove_sidedef`            | edit_sidedefs | `reference/sidedefs.md`    |
| `add_thing`                 | edit_things   | `reference/things.md`      |
| `edit_thing`                | edit_things   | `reference/things.md`      |
| `remove_thing`              | edit_things   | `reference/things.md`      |
| `add_vertex`                | edit_vertices | `reference/vertices.md`    |
| `move_vertex`               | edit_vertices | `reference/vertices.md`    |
| `remove_vertex`             | edit_vertices | `reference/vertices.md`    |
| `add_door`                  | features      | `reference/features.md`    |
| `add_exit`                  | features      | `reference/features.md`    |
| `add_lift`                  | features      | `reference/features.md`    |
| `add_stairs`                | features      | `reference/features.md`    |
| `add_switch`                | features      | `reference/features.md`    |
| `add_teleporter`            | features      | `reference/features.md`    |
| `add_window`                | features      | `reference/features.md`    |
| `mark_secret`               | features      | `reference/features.md`    |
| `place_things`              | features      | `reference/features.md`    |
| `set_light_effect`          | features      | `reference/features.md`    |
| `decode_patch`              | graphics      | `reference/graphics.md`    |
| `get_flat`                  | graphics      | `reference/graphics.md`    |
| `get_patch`                 | graphics      | `reference/graphics.md`    |
| `get_sprite`                | graphics      | `reference/graphics.md`    |
| `get_thing_sprite`          | graphics      | `reference/graphics.md`    |
| `list_flats`                | graphics      | `reference/graphics.md`    |
| `list_patches`              | graphics      | `reference/graphics.md`    |
| `list_sprites`              | graphics      | `reference/graphics.md`    |
| `create_flat_from_image`    | imports       | `reference/imports.md`     |
| `create_sprite_from_image`  | imports       | `reference/imports.md`     |
| `create_texture_from_image` | imports       | `reference/imports.md`     |
| `import_image_as_flat`      | imports       | `reference/imports.md`     |
| `import_image_as_patch`     | imports       | `reference/imports.md`     |
| `import_image_as_raw`       | imports       | `reference/imports.md`     |
| `import_image_as_sprite`    | imports       | `reference/imports.md`     |
| `import_png_as_flat`        | imports       | `reference/imports.md`     |
| `import_png_as_patch`       | imports       | `reference/imports.md`     |
| `import_png_as_sprite`      | imports       | `reference/imports.md`     |
| `process_image`             | imports       | `reference/imports.md`     |
| `diff_wads`                 | inspect       | `reference/inspect.md`     |
| `optimize_wad`              | inspect       | `reference/inspect.md`     |
| `add_lump`                  | lumps         | `reference/lumps.md`       |
| `extract_lump`              | lumps         | `reference/lumps.md`       |
| `get_lump`                  | lumps         | `reference/lumps.md`       |
| `import_asset`              | lumps         | `reference/lumps.md`       |
| `import_file_as_lump`       | lumps         | `reference/lumps.md`       |
| `list_lumps`                | lumps         | `reference/lumps.md`       |
| `remove_lump`               | lumps         | `reference/lumps.md`       |
| `rename_lump`               | lumps         | `reference/lumps.md`       |
| `reorder_lumps`             | lumps         | `reference/lumps.md`       |
| `replace_lump`              | lumps         | `reference/lumps.md`       |
| `search_lumps`              | lumps         | `reference/lumps.md`       |
| `assemble_map`              | maps          | `reference/maps.md`        |
| `copy_map`                  | maps          | `reference/maps.md`        |
| `create_map`                | maps          | `reference/maps.md`        |
| `get_map_lumps`             | maps          | `reference/maps.md`        |
| `get_map_summary`           | maps          | `reference/maps.md`        |
| `list_maps`                 | maps          | `reference/maps.md`        |
| `remove_map`                | maps          | `reference/maps.md`        |
| `rename_map`                | maps          | `reference/maps.md`        |
| `set_active_map`            | maps          | `reference/maps.md`        |
| `version`                   | meta          | `reference/meta.md`        |
| `import_music`              | music         | `reference/music.md`       |
| `preview_music`             | music         | `reference/music.md`       |
| `build_all_nodes`           | nodes         | `reference/nodes.md`       |
| `build_blockmap`            | nodes         | `reference/nodes.md`       |
| `build_nodes`               | nodes         | `reference/nodes.md`       |
| `build_reject`              | nodes         | `reference/nodes.md`       |
| `get_blockmap`              | nodes         | `reference/nodes.md`       |
| `get_nodes`                 | nodes         | `reference/nodes.md`       |
| `get_reject`                | nodes         | `reference/nodes.md`       |
| `get_palette`               | palette       | `reference/palette.md`     |
| `set_palette`               | palette       | `reference/palette.md`     |
| `add_patch`                 | patches       | `reference/patches.md`     |
| `remove_patch`              | patches       | `reference/patches.md`     |
| `replace_patch`             | patches       | `reference/patches.md`     |
| `build_map_from_plan`       | plan          | `reference/plan.md`        |
| `check_playability`         | playability   | `reference/playability.md` |
| `describe_map`              | playability   | `reference/playability.md` |
| `bulk_edit`                 | query         | `reference/query.md`       |
| `find_elements`             | query         | `reference/query.md`       |
| `replace_flat`              | query         | `reference/query.md`       |
| `replace_texture`           | query         | `reference/query.md`       |
| `replace_thing_type`        | query         | `reference/query.md`       |
| `get_render_settings`       | render        | `reference/render.md`      |
| `render_minimap`            | render        | `reference/render.md`      |
| `render_topdown`            | render        | `reference/render.md`      |
| `render_view`               | render        | `reference/render.md`      |
| `set_render_options`        | render        | `reference/render.md`      |
| `set_render_resolution`     | render        | `reference/render.md`      |
| `edit_decorate`             | scripts       | `reference/scripts.md`     |
| `edit_dehacked`             | scripts       | `reference/scripts.md`     |
| `edit_zscript`              | scripts       | `reference/scripts.md`     |
| `get_dehacked`              | scripts       | `reference/scripts.md`     |
| `get_session_info`          | session       | `reference/session.md`     |
| `get_wad_info`              | session       | `reference/session.md`     |
| `merge_wad`                 | session       | `reference/session.md`     |
| `new_wad`                   | session       | `reference/session.md`     |
| `open_iwad`                 | session       | `reference/session.md`     |
| `open_wad`                  | session       | `reference/session.md`     |
| `redo`                      | session       | `reference/session.md`     |
| `save_wad`                  | session       | `reference/session.md`     |
| `undo`                      | session       | `reference/session.md`     |
| `trace_sketch`              | sketch        | `reference/sketch.md`      |
| `build_texture`             | textures      | `reference/textures.md`    |
| `edit_pnames`               | textures      | `reference/textures.md`    |
| `list_pnames`               | textures      | `reference/textures.md`    |
| `list_textures`             | textures      | `reference/textures.md`    |
| `remove_texture`            | textures      | `reference/textures.md`    |
| `align_textures`            | themes        | `reference/themes.md`      |
| `auto_texture`              | themes        | `reference/themes.md`      |
| `get_texture_theme`         | themes        | `reference/themes.md`      |
| `set_texture_theme`         | themes        | `reference/themes.md`      |
| `validate_map`              | validate      | `reference/validate.md`    |

## Workflows

The [guides index](guides/README.md) is the judgment half of the skill: short
pages on what to decide, not how the tools work. Each flagship chain has its
own page:

- [Sketch to map](guides/sketch-to-map.md) — reading a hand-drawn floor plan
  through `trace_sketch` into regions, gaps and marks, turning them into a
  `build_map_from_plan` plan and verifying the result against the drawing.
- [Images to assets](guides/images-to-assets.md) — choosing the right image
  importer, the `process_image` ops to run first, sizes and palette limits
  for textures, flats and sprites.
- [Editing existing maps](guides/editing-existing-maps.md) — orienting on a
  map you did not build, describing it by query rather than by index, and
  landing changes safely.
- [Recipes](guides/recipes.md) — worked end-to-end answers to common
  requests, each runnable as a `run_batch` payload.
- [Design rules](guides/design-rules.md) — the Doom dimensions and gameplay
  facts (player size, door widths, stair heights, light levels) every
  parameter choice measures against.
- [Verification](guides/verification.md) — the change → `validate_map` →
  `check_playability` → render → fix loop to run after every meaningful
  change.

## Typical workflow

`open_iwad` the game archive → `open_wad` your project WAD or `new_wad`
for a fresh one → edit the working archive → `build_all_nodes` after the
geometry edits (one undoable step rebuilding the BSP tree, the `REJECT`
matrix and the `BLOCKMAP` so vanilla `Doom` can play the map;
`reference/nodes.md`) → `validate_map` → `check_playability` → `save_wad`.
`validate_map` reads a map's health — dangling references, broken lines,
missing starts, specials outside the tables, unclosed or crossing
geometry, textures and flats the session does not define — as
severity-ranked issues you fix with the editing tools before saving
(`reference/validate.md`). `check_playability` then walks the map as a
player would and answers `ok` plus reasons: unreachable sectors each named
with the rule that blocks them and the tool that fixes it, stuck and
unreachable things, locks without their keys (`reference/playability.md`).
When a request is about a *set* ("every dark room", "all STARTAN walls"),
`find_elements` answers it as a query — kind, filters, optional bound — and
returns the matching indices instead of making you scan whole maps; when
the request is a *change of values* over such a set ("make every nukage
room darker", "block every line in sector 3"), `bulk_edit` selects with
the same query and applies the set/`add` amounts or named flag edits to
every matched element in one undoable step, `dry_run` first; and when the
request is a *swap* ("use BRICK7 instead of STARTAN everywhere", "turn
every imp into a demon"), `replace_texture` and `replace_flat` glob the old
name and `replace_thing_type` matches one doomednum or definition name,
and all three rewrite the whole set across the maps in one undoable
copy-on-write step, `dry_run` first (`reference/query.md`).
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
pixels and publishes a debug image labelled `R1`, `G1`, `M1` — only the rows
it reports are labelled, and the gap and mark lists are floored at the
measured stroke and capped so a noisy scan still answers small — you decide
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

The table above and the `reference/` pages cover every tool the server
lists; `list_tools` is the live answer if in doubt.
