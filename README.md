# Cyberdemon

An [MCP](https://modelcontextprotocol.io) server that lets an LLM read, understand and edit Doom WADs over stdio. A session loads an `IWAD` plus read-only `PWAD` layers under one editable working archive, and ~135 tools inspect and change maps, graphics, audio and game definitions — every edit undoable, every result structured JSON, every large file (renders, sounds, uploads) delivered as a localhost `HTTP` URL so nothing big crosses the stdio channel. Two flagship workflows: a hand-drawn floor plan goes through `trace_sketch` (regions, gaps, marks and adjacency measured to map units) into `build_map_from_plan` and comes out a playable, node-built map; and any picture — a file path or a URL — lands as a finished asset (`import_image_as_patch`/`flat`/`sprite`, or the one-shot `create_texture_from_image`/`create_flat_from_image`/`create_sprite_from_image`, which quantize to the `PLAYPAL` and write `PNAMES`/`TEXTURE1` for you).

## Build and run

```sh
cargo build --release
./target/release/cyberdemon   # speaks MCP on stdin/stdout
```

Register it with an MCP client, e.g. Claude Code:

```sh
claude mcp add cyberdemon -- /path/to/cyberdemon/target/release/cyberdemon
```

## Configuration

- `CYBERDEMON_ASSET_CAPACITY` — entries kept in the in-memory LRU asset store before the oldest are discarded (default 50, max 10000); missing or invalid values use the default. Runtime equivalent: `set_asset_capacity`.

## Who decides what

The server is the reflex — deterministic tools carrying out formulaic operations on explicit inputs — and the calling model is the judgment, deciding shapes, scale, theme and balance as explained in the [skill guides](skill/guides/README.md) (entry point: [skill/SKILL.md](skill/SKILL.md)).

## Tool groups

- `meta` — server identity (`version`).
- `assets` — asset origin info, capacity, and one URL for any session resource (`get_asset_url`).
- `session` — load archives (`open_iwad`, `open_wad`, `new_wad`, `merge_wad`), `save_wad`, `undo`, `redo`, session state.
- `batch` — `run_batch`: several tool calls as one atomic, single-undo step with `$ref`-chained results.
- `lumps` — the lump directory: list, search, read, extract, add, import, replace, remove, rename, reorder.
- `maps` — map blocks: list, summary, assemble, create, copy, rename, remove, active map.
- `convert` — `convert_map_format` between binary `Doom`/`Hexen` and `UDMF`, `dry_run`-previewable.
- `edit_things` — add, edit and remove things (typed or raw fields, angles normalized).
- `edit_vertices` — add, move and remove vertices with reference-aware cleanup.
- `edit_linedefs` — add, edit, remove, split and join linedefs.
- `edit_sidedefs` — add, edit and remove sidedefs: faces, textures, offsets, sector re-pointing.
- `edit_sectors` — add, edit and remove sectors, draw whole rooms with `create_sector_from_loop`, carve solid rock with `carve_void`.
- `edit_geometry` — `delete_geometry` and `transform_selection` over index selections.
- `features` — working structures in one call: `add_door`, `add_window`, `add_stairs`, `add_lift`, `add_teleporter`, `add_switch`, `add_exit`, `place_things`, `mark_secret`, `set_light_effect`.
- `plan` — `build_map_from_plan`: a whole playable map (rooms, walls, dressing, features, things) from one JSON plan.
- `sketch` — `trace_sketch`: measure a hand-drawn floor plan into region polygons, gaps, marks and map-unit points.
- `query` — `find_elements`, `bulk_edit` and the `replace_*` swaps over query-selected sets.
- `validate` — `validate_map`: map health as severity-ranked, code-tagged issues.
- `playability` — `check_playability` (can a player finish, and what blocks them) and `describe_map` fact rows.
- `nodes` — inspect and build `SEGS`/`SSECTORS`/`NODES` (`classic`, `extended`, `gl`), `REJECT` and `BLOCKMAP` (`build_all_nodes` before vanilla play).
- `graphics` — read pictures the engine sees: flats, patches, sprites and thing sprites as `PNG` URLs.
- `patches` — patch lump lifecycle: add, replace, remove whole patch lumps.
- `textures` — `PNAMES`/`TEXTURE1`/`TEXTURE2` definitions: list, build composite textures, remove.
- `themes` — per-game texture roles: get/set the theme, `auto_texture` repaint, `align_textures` chains.
- `palette` — `PLAYPAL` get/set with `COLORMAP`-derived tables.
- `imports` — any picture (PNG/JPEG/GIF/BMP) in: `import_image_as_*`, `process_image` ops, and the one-shot `create_*` asset tools.
- `audio` — `DS*`/`DP*` sound import and `WAV` preview.
- `music` — `D_*` music import (`MUS`/MIDI) and `MIDI` preview.
- `render` — `render_topdown`, `render_minimap`, the `render_view` column-caster view, and the session's render settings.
- `defs` — the game thing/special/sector tables, overlaid with DEHACKED/DECORATE/ZScript/MAPINFO definitions.
- `scripts` — read and patch `DEHACKED`/`DECORATE`/`ZSCRIPT` lumps.
- `inspect` — `diff_wads` between session views or files; `optimize_wad` strips redundant lumps.

## Development

```sh
scripts/ci.sh   # rustfmt, clippy (strict), tests, docs; exit 0 only if all pass
```

[AGENTS.md](AGENTS.md) holds the contributor rules and the directory map.

## Limitations

- No floats anywhere: the renderer and geometry are integer (16.16 fixed point), so perspective and scaling are approximations of Doom's, and lighting runs through generated `COLORMAP` tables when a WAD has none.
- `UDMF` is a practical subset: the modelled fields, `TEXTMAP` grammar and common namespaces; unmodelled text properties ride along only where the model stores them.
- `DEHACKED`/`DECORATE`/`ZScript` support is a deliberate subset parser (thing/mobj tables, `actor`/`class` blocks, `DoomEdNums`), not a full language implementation.
- Sketch tracing measures ink: it needs reasonably clean, straight scans (walls near axis lines, closed rings) and never interprets what it finds.
- Image and sound tools read files or URLs only — a picture that exists only inside the chat must first be saved to a file by the user or uploaded to the asset origin.
