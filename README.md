# Cyberdemon

An [MCP](https://modelcontextprotocol.io) server that lets an LLM read, understand and edit Doom WADs.

WADs are binary archives of lumps (maps, textures, sprites, sounds, palettes, demos). Cyberdemon exposes them to an LLM as structured tools, so a model can inspect a WAD, reason about its contents, and make precise edits without hand-parsing bytes.

The work splits in two: the server is the **reflex** — deterministic tools performing formulaic operations on explicit inputs — and the calling model is the **judgment**, guided by the [skill guides](skill/guides/README.md): interpreting requests and images, choosing shapes, scale and theme.

It speaks MCP over **stdio** (not HTTP), so any MCP client can launch it as a subprocess.

> Status: foundation. The server starts per-connection with shared session
> state, exposes a `version` tool plus an `assets` group
> (`get_asset_server_info`, `set_asset_capacity`), and defines the structured
> JSON result/error contract every WAD tool will use. Generated files are
> served to clients as `http://localhost:<port>` URLs by a lazily started
> asset server (LRU store + uploads via `/upload/<name>`), so big binaries
> never cross the stdio channel. An internal graphics core (`src/gfx*`:
> images, PNG/JPEG/GIF/BMP codecs, `grAb`, palettes, quantization, pure
> integer image ops, Doom patch (incl. tall) and flat codecs), an internal
> audio core (`src/audio*`: interleaved
> `i16` `Pcm`, `DMX` `DS*` sound lumps, `WAV` codecs, integer mono downmix
> and resampling), an internal music core (`src/music*`: Doom `MUS` to
> type-0 `MIDI` conversion and back, an `SMF` writer and a track-merging
> `SMF` parser), a float-free
> math core (`src/fixed*`: 16.16 fixed point, BAM angles, CORDIC trig,
> `isqrt`) and a pure WAD format core (`src/wad*`: eight-byte lump names,
> bounds-checked little-endian reader/writer, an in-memory `Wad` archive
> with parsing, serialization, name lookup and atomic file I/O, and a lump
> classifier reporting namespaces, map blocks and type hints) and a session
> model (`src/session*`: an IWAD plus resource layers under one editable
> working archive, a merged view with vanilla last-archive-wins override
> rules, and a labelled undo/redo history with dirty-state tracking) back
> the upcoming WAD tools.
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
