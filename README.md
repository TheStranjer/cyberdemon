# Cyberdemon

An [MCP](https://modelcontextprotocol.io) server that lets an LLM read, understand and edit Doom WADs.

WADs are binary archives of lumps (maps, textures, sprites, sounds, palettes, demos). Cyberdemon exposes them to an LLM as structured tools, so a model can inspect a WAD, reason about its contents, and make precise edits without hand-parsing bytes.

The work splits in two: the server is the **reflex** — deterministic tools performing formulaic operations on explicit inputs — and the calling model is the **judgment**, guided by the [skill guides](skill/guides/README.md): interpreting requests and images, choosing shapes, scale and theme.

It speaks MCP over **stdio** (not HTTP), so any MCP client can launch it as a subprocess.

> Status: foundation. The server starts per-connection with shared session
> state, exposes a single `version` tool, and defines the structured JSON
> result/error contract every WAD tool will use. An internal graphics core
> (`src/gfx*`: images, PNG/JPEG/GIF/BMP codecs, `grAb`, palettes,
> quantization) and a float-free math core (`src/fixed*`: 16.16 fixed point,
> BAM angles, CORDIC trig, `isqrt`) back the upcoming WAD tools.

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
