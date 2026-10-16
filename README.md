# Cyberdemon

An [MCP](https://modelcontextprotocol.io) server that lets an LLM read, understand and edit Doom WADs.

WADs are binary archives of lumps (maps, textures, sprites, sounds, palettes, demos). Cyberdemon exposes them to an LLM as structured tools, so a model can inspect a WAD, reason about its contents, and make precise edits without hand-parsing bytes.

The work splits in two: the server is the **reflex** — deterministic tools performing formulaic operations on explicit inputs — and the calling model is the **judgment**, guided by the [skill guides](skill/guides/README.md): interpreting requests and images, choosing shapes, scale and theme.

It speaks MCP over **stdio** (not HTTP), so any MCP client can launch it as a subprocess.

> Status: foundation plus the first WAD and audio tools. The server starts per-connection with shared session
> state, exposes a `version` tool, an `assets` group
> (`get_asset_server_info`, `set_asset_capacity`), a `defs` group
> (`get_thing_types`, `get_linedef_specials`, `get_sector_specials`), a
> `graphics` group (`list_flats`, `list_patches`, `list_sprites`,
> `get_flat`, `get_patch`, `get_sprite`, `get_thing_sprite`,
> `decode_patch`), an `imports` group (`import_image_as_patch`,
> `import_png_as_patch`, `import_image_as_raw`), a
> `lumps` group (`list_lumps`, `search_lumps`, `get_lump`, `extract_lump`,
> `add_lump`, `import_asset`, `import_file_as_lump`, `replace_lump`,
> `remove_lump`, `rename_lump`, `reorder_lumps`), a `maps` group
> (`list_maps`, `get_map_lumps`, `set_active_map`), an `audio` group
> (`import_sound`, `preview_sound`), a `music` group (`import_music`,
> `preview_music`), a `scripts` group (`get_dehacked`, `edit_dehacked`,
> `edit_decorate`, `edit_zscript`),
> a `session` group
> (`open_iwad`, `open_wad`, `new_wad`, `merge_wad`, `save_wad`, `undo`,
> `redo`, `get_session_info`, `get_wad_info`), a `render` group
> (`set_render_resolution`, `set_render_options`, `get_render_settings`),
> a `textures` group (`list_textures`, `list_pnames`, `edit_pnames`)
> and a `palette` group (`get_palette`, `set_palette`),
> and defines the structured JSON result/error contract every WAD tool uses. Generated files are
> served to clients as `http://localhost:<port>` URLs by a lazily started
> asset server (LRU store + uploads via `/upload/<name>`), so big binaries
> never cross the stdio channel. An internal graphics core (`src/gfx*`:
> images, PNG/JPEG/GIF/BMP codecs, `grAb`, palettes, quantization, pure
> integer image ops, Doom patch (incl. tall) and flat codecs), an internal
> audio core (`src/audio*`: interleaved
> `i16` `Pcm`, `DMX` `DS*` sound lumps, `WAV` codecs, `DP*` PC speaker
> tone-list synthesis, `symphonia`-backed
> `WAV`/`OGG`/`MP3`/`FLAC` import decoding, integer mono downmix
> and resampling), an internal music core (`src/music*`: Doom `MUS` to
> type-0 `MIDI` conversion and back, an `SMF` writer, a track-merging
> `SMF` parser and magic-based container sniffing), a float-free
> math core (`src/fixed*`: 16.16 fixed point, BAM angles, CORDIC trig,
> `isqrt`) and a pure WAD format core (`src/wad*`: eight-byte lump names,
> bounds-checked little-endian reader/writer, an in-memory `Wad` archive
> with parsing, serialization, name lookup and atomic file I/O, a lump
> classifier reporting namespaces, map blocks and type hints, a
> `PNAMES`/`TEXTURE1`/`TEXTURE2` codec pair with `Doom`/`Strife` layout
> detection and patch-reference renumbering, and a `UDMF` `TEXTMAP`
> tokenizer, parser and writer over documents of blocks and key/value
> pairs) and a session
> model (`src/session*`: an IWAD plus resource layers under one editable
> working archive, a merged view with vanilla last-archive-wins override
> rules, and a labelled undo/redo history with dirty-state tracking) and a
> definitions registry (`src/defs*`: TSV game tables embedded in the binary,
> parsed once, selected per game, and overlaid with the session's custom
> DEHACKED/DECORATE/ZScript/MAPINFO thing definitions) and render defaults (`src/render*`: the
> validated session-level resolution, lighting, field of view, gamma and
> style every render tool starts from, plus the integer gamma curve) and a
> pure planar geometry engine (`src/geom*`: layered polygons and external
> segments arranged into non-crossing, side-labelled integer pieces; all
> drawing goes through `geom::arrange`) and a pure float-free BSP node
> builder (`src/bsp*`: segs from linedef sides, 16.16/i128 side tests,
> splitting and cost-chosen partition lines, a tree of convex subsectors
> with integer bounding boxes) and a pure raster sketch measurer
> (`src/sketch*`: ink mask, stroke width, morphological closing,
> deterministic components, enclosed regions, marks and wall gaps, and
> regions grown to wall centrelines then traced, simplified, straightened
> and welded integer polygons; it measures and never interprets) back the
> first WAD tools.
> `AppState::fetch_source` turns client `source` strings (absolute paths,
> `file://`, `http(s)://` and own-asset URLs, capped at 256 MiB) into bytes
> for import tools.

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

- `CYBERDEMON_ASSET_CAPACITY` — entries kept in the in-memory LRU asset store before the oldest are discarded (default 50, max 10000); missing or invalid values use the default.

## Development

```sh
scripts/ci.sh   # rustfmt, clippy (strict), tests, docs; exit 0 only if all pass
```

See [AGENTS.md](AGENTS.md) for contributor rules and [skill/](skill/) for how to use the server as an LLM.
