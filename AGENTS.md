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
  geom.rs         Planar arrangement engine root: `Pt`, layers, pieces and
                  `arrange`, turning layered polygons plus external
                  segments into non-crossing, side-labelled segments.
                  All drawing goes through `geom::arrange`.
  geom/           Arrangement internals (no MCP concerns):
                  `geom/label.rs` `Label`, the `Labeler` trait and the
                  topmost-layer `LayerLabeler` (even-odd in i128),
                  `geom/segment.rs` integer segment math: orientations,
                  rounded crossings, T-junction and overlap cut finding,
                  the bounding-box sweep and the point-merge union-find,
                  `geom/arrange.rs` the pipeline: polygon validation,
                  splitting, dedupe, side labelling and collinear merge.
  gfx.rs          Graphics core module root: dimensions limits and shared helpers.
  gfx/            Internal image building blocks (no MCP concerns):
                  `gfx/image.rs` RGBA/indexed images and resize/flip,
                  `gfx/codec.rs` PNG/JPEG/GIF/BMP decode and PNG encode,
                  `gfx/grab.rs` the `grAb` PNG chunk, `gfx/palette.rs`
                  PLAYPAL/hex palettes, `gfx/quantize.rs` quantization,
                  `gfx/patch.rs` the Doom patch codec (incl. tall patches),
                  `gfx/flat.rs` the flat codec,
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
                  "DS*" DMX sound lump decode/encode,
                  `audio/pcspeaker.rs` Doom "DP*" PC speaker tone-list
                  square-wave synthesis, `audio/wav.rs`
                  RIFF/WAV decode (8/16/24/32-bit and extensible PCM) and
                  canonical 8/16-bit encode, `audio/decode.rs` any-container
                  import decode (`decode_any` over WAV/OGG/MP3/FLAC via
                  symphonia with the in-house WAV reader first,
                  `sniff_format` magic detection, 10-minute cap).
  bsp.rs          BSP node builder core: 16.16 `Point`, integer
                  `Partition`, `BuildOptions` and the `build_tree` entry
                  point. No lump writing, no tools.
  bsp/            Builder internals (no MCP concerns): `bsp/seg.rs`
                  linedef sides to `BuildSeg`s with index validation,
                  `bsp/side.rs` `R_PointOnSide` tests and the tolerant
                  seg classification, `bsp/split.rs` crossing detection
                  and seg cutting in i128, `bsp/choose.rs` candidate
                  lines, sampling and split cost, `bsp/tree.rs` the
                  recursion, convexity, bounding boxes, guards and
                  `BspTree`.
  music.rs        Music core module root: re-exports the converter and writer.
  music/          Music building blocks (no MCP concerns): `music/midi_write.rs`
                  the type-0 SMF writer (`MidiWriter`, big-endian chunks,
                  variable-length quantities in both directions),
                  `music/mus2mid.rs` Doom MUS music lump ("D_*") to MIDI bytes.
  defs.rs         Definitions core module root: the game definitions registry
                  entry point, the Boom generalized/sector bit decoders, the
                  session-aware overlay entry (`session_definitions`) and
                  the embedded-table re-exports.
  defs/           Definition building blocks (no MCP concerns):
                  `defs/table.rs` the TSV parser producing `ThingDef`,
                  `SpecialDef` and `SectorTypeDef` rows,
                  `defs/registry.rs` the `include_str!`-embedded tables
                  parsed once behind `LazyLock` and the per-game selection
                  (`definitions_for`), `defs/boom.rs` the generalized
                  linedef and Boom sector-special bit unpackers,
                  `defs/custom.rs` the `CustomSources` text bundle and the
                  built-in < DEHACKED < DECORATE/ZScript (+ MAPINFO)
                  overlay, `defs/dehacked.rs` the `Thing <n>` block subset
                  over the embedded `deh_mobj.tsv` map, `defs/decorate.rs`
                  the tokenising DECORATE `actor` reader, `defs/zscript.rs`
                  the `class` reader on the same scanner and
                  `defs/mapinfo.rs` the `DoomEdNums` block reader.
  render.rs       Rendering core module root: the integer gamma curve
                  (`gamma_table`) and the settings re-exports.
  render/         Rendering building blocks (no MCP concerns):
                  `render/settings.rs` the session-level `RenderSettings`
                  (resolution, lighting, markers, sky, FOV, gamma, top-down
                  style, background), their ranges and validating setters.
  assets.rs       LRU asset store for generated files (`assets/store.rs`),
                  the lazy localhost asset HTTP server (`assets/server.rs`)
                  and the one client->bytes input reader for import tools
                  (`assets/fetch.rs`: `Fetched`, `derive_lump_name`).
  session.rs      The per-connection editing session: an optional IWAD layer and
                  read-only resource layers under one editable working archive,
                  the merged view, `Scope` reads and `Session::edit`.
  session/        Session building blocks (no MCP concerns):
                  `session/merge.rs` folds one archive over another with vanilla
                  override rules (map blocks, namespaces, globals, last wins;
                  `merge_into` and `merge_counts` are the whole-archive forms),
                  `session/history.rs` is the labelled undo/redo stack and the
                  revision bookkeeping behind dirty state, and
                  `session/cow.rs` the copy-on-write step that brings one map
                  block from the merged view into the working archive
                  (`copy_map_block`, `Session::copy_map_to_working`).
  state.rs        AppState: session and asset store behind mutexes; with_session
                  and with_assets are the only access; publish, fetch_source,
                  asset_base_url and asset_id_from_url bridge tools and asset URLs.
  tools.rs        Shared result helpers (ok/fail) plus the tool group modules.
  tools/          One module per tool group, each with its own #[tool_router];
                  Cyberdemon::router() in server.rs merges them.
  wad.rs          WAD format core module root: lump names, byte access, the
                  in-memory archive, the map module tree and the texture
                  definition codecs.
  wad/            Format building blocks (no MCP concerns): `wad/name.rs` the
                  eight-byte uppercase `LumpName` with glob matching,
                  `wad/bytes.rs` bounds-checked little-endian `Reader` and
                  `Writer` plus the checked usize/u32 casts,
                  `wad/archive.rs` the `Wad` archive with parsing,
                  serialization, lookup and atomic I/O, `wad/classify.rs`
                  the pure lump-directory analysis pass: `Namespace`s,
                  `MapBlock`s and `LumpKind` type hints, `wad/game.rs`
                  the pure game signature detection (`detect_game`) and
                  `wad/textures.rs` the `PNAMES` and `TEXTURE1`/`TEXTURE2`
                  codecs: patch-name index, texture records with
                  `Doom`/`Strife` layout detection and patch-reference
                  renumbering.
  wad/map.rs      Map layer root: the `read_map`/`write_map` format
                  dispatcher and the model re-exports.
  wad/map/        Map data (no MCP concerns): `map/model.rs` the
                  format-neutral `Map` model (`Thing`, `Vertex`,
                  `Linedef`, `Sidedef`, `Sector`, `extra` side-bags),
                  `map/doom.rs` the binary Doom map reader/writer
                  (`read_doom_map`/`write_doom_map`).
data/
  defs/           TSV definition tables embedded in the binary (each file
                  under the 64 KiB `large_include_file` budget), parsed by
                  `src/defs/table.rs`. `doom_things.tsv` is the DOOM/DOOM II
                  thing table, `doom_specials.tsv` and `doom_sectors.tsv` the
                  linedef special and sector type tables;
                  `heretic_things.tsv`, `heretic_specials.tsv` and
                  `heretic_sectors.tsv` are Heretic's own three.
                  `deh_mobj.tsv` maps the 1-based Doom II DeHackEd mobj
                  indices to their doomednums (`-1`: unplaceable), read by
                  `src/defs/dehacked.rs`; `defs/dehacked_edit.rs` is the
                  line-oriented DeHackEd/BEX patch parser and editor behind
                  the scripts tools.
tests/
  mcp.rs          Single integration test crate root.
  mcp/            support.rs: rmcp client harness plus a probe server speaking
                    the structured-result contract; fixtures.rs: WAD builders
                    writing archives with the library; server.rs: end-to-end
                    tests; assets.rs and session.rs: tool-group tests;
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
