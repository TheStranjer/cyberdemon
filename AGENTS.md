# AGENTS.md

Cyberdemon is a Rust MCP server (stdio transport, built on the `rmcp` crate) for reading and editing Doom WADs.

## Directory structure

```
Cargo.toml        Package manifest. Lint levels live here, under [lints.*].
clippy.toml       Clippy thresholds and knobs.
src/
  main.rs         Stdio entry point only. stdout is the MCP protocol channel; never print to it.
  lib.rs          Library root; module declarations, re-exports, crate aliases.
  server.rs       MCP server: identity, per-connection state and the composed tool router.
  error.rs        CyberError: the domain error enum every tool reports through.
  fixed.rs        Float-free math core: 16.16 `Fixed`, `FRACBITS`/`FRACUNIT`,
                  `isqrt`, `mul_div_round`. Use this instead of f32/f64.
  fixed/          Math internals (no MCP concerns): `fixed/angle.rs` 32-bit
                  binary angles (BAM) with `atan2`, `hypot`, `rotate_point`,
                  `fixed/trig.rs` the integer CORDIC engine.
  gfx.rs          Graphics core module root: dimensions limits and shared helpers.
  gfx/            Internal image building blocks (no MCP concerns):
                  `gfx/image.rs` RGBA/indexed images and resize/flip,
                  `gfx/codec.rs` PNG/JPEG/GIF/BMP decode and PNG encode,
                  `gfx/grab.rs` the `grAb` PNG chunk, `gfx/palette.rs`
                  PLAYPAL/hex palettes, `gfx/quantize.rs` quantization,
                  `gfx/ops.rs` the image-op framework (`ImageOp`, `apply_ops`,
                  reports, hex/luminance helpers) with `gfx/ops/geometry.rs`
                  crop/fit/rotate/flip, `gfx/ops/resample.rs` nearest/
                  bilinear/area sampling, `gfx/ops/tone.rs` auto-levels/
                  adjust, `gfx/ops/alpha.rs` colour key and trim.
  audio.rs        Audio core module root: sample-rate limits and the shared
                  unsigned-8-bit conversions.
  audio/          Audio building blocks (no MCP concerns): `audio/pcm.rs`
                  interleaved i16 `Pcm` with mono downmix, integer linear
                  resampling and 8-bit conversion, `audio/dmx.rs` Doom
                  "DS*" DMX sound lump decode/encode, `audio/wav.rs`
                  RIFF/WAV decode (8/16/24/32-bit and extensible PCM) and
                  canonical 8/16-bit encode.
  music.rs        Music core module root: re-exports the converter and writer.
  music/          Music building blocks (no MCP concerns): `music/midi_write.rs`
                  the type-0 SMF writer (`MidiWriter`, big-endian chunks,
                  variable-length quantities in both directions),
                  `music/mus2mid.rs` Doom MUS music lump ("D_*") to MIDI bytes.
  assets.rs       LRU asset store for generated files (`assets/store.rs`),
                  the lazy localhost asset HTTP server (`assets/server.rs`)
                  and the one client->bytes input reader for import tools
                  (`assets/fetch.rs`: `Fetched`, `derive_lump_name`).
  session.rs      The per-connection editing session (placeholder for now).
  state.rs        AppState: session and asset store behind mutexes; with_session
                  and with_assets are the only access; publish, fetch_source,
                  asset_base_url and asset_id_from_url bridge tools and asset URLs.
  tools.rs        Shared result helpers (ok/fail) plus the tool group modules.
  tools/          One module per tool group, each with its own #[tool_router];
                  Cyberdemon::router() in server.rs merges them.
  wad.rs          WAD format core module root: lump names, byte access and
                  the in-memory archive.
  wad/            Format building blocks (no MCP concerns): `wad/name.rs` the
                  eight-byte uppercase `LumpName` with glob matching,
                  `wad/bytes.rs` bounds-checked little-endian `Reader` and
                  `Writer` plus the checked usize/u32 casts,
                  `wad/archive.rs` the `Wad` archive with parsing,
                  serialization, lookup and atomic I/O, `wad/classify.rs`
                  the pure lump-directory analysis pass: `Namespace`s,
                  `MapBlock`s and `LumpKind` type hints.
tests/
  mcp.rs          Single integration test crate root.
  mcp/            support.rs: rmcp client harness plus a probe server speaking
                    the structured-result contract; server.rs: end-to-end tests;
                    docs.rs: skill/ and README.md layout checks.
scripts/ci.sh     Runs fmt, clippy, tests and docs. Exit 0 only if everything passes.
skill/            LLM-facing documentation for using the MCP server (see below).
skill/reference/  Per-tool-group documentation, indexed by skill/SKILL.md.
skill/guides/     Judgment guides for the calling model, indexed by
                  skill/guides/README.md.
```

## Rules

1. **Clippy must pass, and its strictness must not drop.** Fix the code, not the lint. Never loosen `[lints.*]` in `Cargo.toml` or `clippy.toml`, never raise thresholds, and never add `#[allow]` (denied by `allow_attributes`). The one narrow exception is `#[expect(lint, reason = "...")]` on code generated by a third-party macro that cannot be changed; keep it on the smallest item and give a real reason.
2. **Test every code path you add or change.** Every new or changed branch, error case and tool needs a test that exercises it. Prefer integration tests through a real rmcp client (`tests/`); add unit tests for parsing and pure logic. No code lands untested.
3. **`skill/` documents outward behavior.** Any change to what an MCP client can observe (tools, parameters, results, errors, server instructions) must update `skill/` in the same change. `skill/` tells an LLM how to use the server, not how it works inside.
4. **Run `scripts/ci.sh` before finishing.** It must exit 0.
5. **Never write to stdout** from the server process (no `println!`, no `io::stdout`). Clippy enforces this. Diagnostics go to stderr.
6. **Keep README.md up to date and terse.** All the important details need to be in README.md, but it should exceed 100 lines under any circumstances.

## Tool design rules

The server is the reflex, the calling model is the judgment: tools carry out formulaic operations on explicit inputs ("make this shape with these dimensions", "compile this plan", "split this line") and never exercise taste.

1. **Deterministic.** A tool's behaviour is a function of its parameters and the session state. Never infer intent from names or free text, and never use randomness except where a `seed` parameter is exposed.
2. **Taste is a parameter.** Every choice that needs judgment gets an explicit parameter with a documented, conservative default (e.g. wall thickness 16, door width 64, step height 16). How to choose the value is explained in a `skill/guides/*.md` page, not in code.
3. **Results are measurable.** Report what was done in numbers the model can check (indices created, sizes, counts, warnings) and, where geometry changed, how to look at it (`render_topdown`, `render_view`).
4. **Errors are instructions.** Every error says what to do next: a short hint naming the tool or parameter that fixes it.
5. **Image tools only measure.** Tools that analyse images (e.g. sketch tracing) return regions, coordinates, colours; they never decide what something *is*.
6. **High-level first.** Prefer one tool that does a complete formulaic job (a working door, a whole room) over requiring many low-level calls, but keep the low-level tools for fine control.

## Clippy traps

The repo's deny list goes well beyond `pedantic`. Accepted fixes (tests are exempt from unwrap/expect/panic/indexing/print, but not from arithmetic rules):

- No bare `+ - * / % <<` on integers or custom types → `checked_*` (treat `None` as an error), `saturating_*`, `wrapping_*`. No float arithmetic at all.
- No `as` casts → `try_from`/`from`. Byte order is explicit: only `to_le_bytes`/`from_le_bytes`.
- No `v[i]`/`v[a..b]` in production code → `.get()`, `.split_at_checked()`, `.chunks_exact()`, iterators.
- No `unwrap`/`expect`/`panic!`/`todo!`/`unreachable!` in production; never print (stdout is the MCP channel).
- Doc comments on every item, including private fields and functions.
- Budgets: functions ≤ 60 lines, ≤ 4 parameters, cognitive complexity ≤ 10, nesting ≤ 3.
- Modules are `src/a.rs` + `src/a/b.rs`, never `mod.rs`. Prefer `core::`/`alloc::` paths where they exist; `.to_owned()` on `&str`; no shadowing; suffixed literals (`0_u32`); no `_ =>` on enums; `else if` chains end with `else`.
