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
                  `gfx/colormap.rs` the COLORMAP reader and palette
                  generator (34 tables), `gfx/composite.rs` texture
                  patch-stack folding (`compose`, `Composite`),
                  `gfx/patch.rs` the Doom patch codec (incl. tall patches),
                  `gfx/flat.rs` the flat codec,
                  `gfx/sprite_name.rs` the sprite lump name grammar,
                  `gfx/sprite_rotation.rs` the engine's viewer-angle to
                  rotation-number bucketing (`rotation_for`),
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
                  point; `bsp::vanilla::to_vanilla` renders a finished
                  tree as the vanilla `SEGS`/`SSECTORS`/`NODES` record
                  bytes and `bsp::extended::to_extended` as one ZDoom
                  `XNOD`/`ZNOD` extended lump. No archive I/O, no tools.
  bsp/            Builder internals (no MCP concerns): `bsp/seg.rs`
                  linedef sides to `BuildSeg`s with index validation,
                  `bsp/side.rs` `R_PointOnSide` tests and the tolerant
                  seg classification, `bsp/split.rs` crossing detection
                  and seg cutting in i128, `bsp/choose.rs` candidate
                  lines, sampling and split cost, `bsp/tree.rs` the
                  recursion, convexity, bounding boxes, guards and
                  `BspTree`, `bsp/vanilla.rs` the vanilla serialization:
                  rounded split-vertex reuse/append, the three record
                  writers and the i16/u16 limit refusals, and
                  `bsp/extended.rs` the extended serialization: the
                  fraction-precise vertex table, the four record
                  sections, the zlib wrap and the limit refusals,
                  `bsp/polygon.rs` Sutherland-Hodgman convex clipping of
                  the map box by partition and seg lines, and
                  `bsp/gl.rs` the glBSP v2 writer: region polygons,
                  miniseg-closed clockwise seg loops, the partner pass
                  and the `GL_*` record lumps.
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
                  (`gamma_table`) and the settings, canvas, lighting, scene
                  and column-caster re-exports.
  render/         Rendering building blocks (no MCP concerns):
                  `render/settings.rs` the session-level `RenderSettings`
                  (resolution, lighting, markers, sky, FOV, gamma, top-down
                  style, background), their ranges and validating setters,
                  `render/canvas.rs` the integer `Canvas` raster: clipped
                  `i64` drawing (set/blend, Bresenham and thick lines,
                  rects, circles, even-odd polygon fills), gamma at encode
                  time and the `RGBA8` image out, `render/light.rs` the
                  COLORMAP table choice (`colormap_index`, `WallDir` fake
                  contrast) and `shade`, `render/scene.rs` the `Scene`
                  preparation: map, palette, COLORMAP, used textures and
                  flats, sky selection, checkerboard fallbacks, `problems`
                  and the camera helpers, `render/topdown.rs` the
                  shared automap line map: `Doom` classification and
                  colours (RGB defaults plus `PLAYPAL` entries), thing
                  visuals, and the square `Window` mapping the minimap
                  frames with, `render/view.rs` the 3D-view column
                  caster: 16.16 camera trig, exact `i128` ray/segment
                  fractions, the sorted portal column walk (textured
                  upper/middle/lower faces with pegging, world-aligned
                  textured planes, a view-angled sky, a per-pixel depth
                  buffer) and the masked-middle collection, and
                  `render/sprites.rs` the transparent pass: thing sprites
                  projected through the eight-rotation bucketing and the
                  masked middles painted far to near, depth-tested
                  against the caster's buffer.
  sketch.rs       Sketch analysis core module root: the `Mask`/`Labels`
                  rasters and the `analyze` and `vectorize` entry points.
                  It measures ink, stroke width, marks, enclosed regions
                  and wall gaps in pixels and grows the regions into
                  integer polygons; it never interprets them.
  sketch/         Sketch building blocks (no MCP concerns):
                  `sketch/mask.rs` `Mask`, `InkMode`, `ink_mask` and
                  `stroke_width`, `sketch/morph.rs` square-element
                  `dilate`/`erode`/`close` (separable running counts,
                  O(w·h)), `sketch/components.rs` `Labels`, `Component`
                  and deterministic scan-order `label_components`,
                  `sketch/analyze.rs` the `Analysis` pipeline: marks
                  split from walls, closing, regions, gaps and warnings,
                  `sketch/expand.rs` centreline BFS growth of regions
                  into the closed walls plus region `adjacency`,
                  `sketch/outline.rs` crack-following region outlines
                  (pixel-corner vertices, clockwise) and `is_simple`,
                  `sketch/simplify.rs` Douglas-Peucker for closed rings
                  and `snap_right_angles`, `sketch/weld.rs` clustering
                  wall coordinates across shapes so shared walls meet
                  exactly, `sketch/vectorize.rs` the `Analysis` to
                  `Vectorized` pipeline.
  assets.rs       LRU asset store for generated files (`assets/store.rs`),
                  the lazy localhost asset HTTP server (`assets/server.rs`)
                  and the one client->bytes input reader for import tools
                  (`assets/fetch.rs`: `Fetched`, `derive_lump_name`).
  session.rs      The per-connection editing session: an optional IWAD layer and
                  read-only resource layers under one editable working archive,
                  the merged view, the layer-only `base_lumps` view, `Scope`
                  reads and `Session::edit`.
  session/        Session building blocks (no MCP concerns):
                  `session/merge.rs` folds one archive over another with vanilla
                  override rules (map blocks, namespaces, globals, last wins;
                  `merge_into` and `merge_counts` are the whole-archive forms),
                  `session/history.rs` is the labelled undo/redo stack and the
                  revision bookkeeping behind dirty state, and
                  `session/cow.rs` the copy-on-write step that brings one map
                  block from the merged view into the working archive
                  (`copy_map_block`, `Session::copy_map_to_working`), and
                  `session/map_edit.rs` the single undoable step behind every
                  map edit tool (`Session::edit_map`: copy, typed edit, node
                  invalidation on geometry change, in-place block rewrite)
                  and its format-rewriting sibling `Session::convert_map`
                  (`rewrite_in_place`: the same step, always dropping the
                  block's node data and `GL` run).
  state.rs        AppState: session and asset store behind mutexes; with_session
                  and with_assets are the only access; publish, fetch_source,
                  asset_base_url and asset_id_from_url bridge tools and asset URLs.
  themes.rs       Texture themes: the twenty texture `Role`s, the deterministic
                  `resolve` (exact candidates, globs, `fallback_role` chain,
                  game default theme, game default names, first available) and
                  the session's `TextureTheme` (theme name plus per-role
                  overrides, session state outside the undo history).
  themes/         Theme building blocks (no MCP concerns):
                  `themes/table.rs` the `data/themes/*.tsv` readers and the
                  `include_str!`-embedded per-game theme tables,
                  `themes/auto.rs` the pure `auto_texture` engine (fill
                  missing middles, upper gaps and lower steps; theme-wide
                  repaint keeping door/switch specials, jambs getting
                  door_track), `themes/align.rs` the pure `align_textures`
                  engine (drawing-order wall chains, x offsets modulo the
                  texture width, y offsets pegged to the chain's highest
                  ceiling).
  tools.rs        Shared result helpers (ok/fail) plus the tool group modules.
  tools/          One module per tool group, each with its own #[tool_router];
                  Cyberdemon::router() in server.rs merges them.
                  `tools/graphics/resolve.rs` holds the shared picture
                  resolvers (flat, patch, composite texture, sprite
                  rotation) that every tool and the renderers decode
                  graphic lumps through;
                  `tools/maps/common.rs` holds the shared `map`-parameter
                  resolvers (`resolve_map_name`, `load_map`) every map
                  tool builds on, and `tools/maps/params.rs` the shared
                  editing parameter types (`FlagsInput`, `IndexSelector`,
                  `check_index`, `validate_texture_name`, `edit_result_fragment`)
                  every map edit tool builds on; `tools/maps/manage.rs`
                  the block-level machinery behind `create_map`,
                  `copy_map`, `rename_map` and `remove_map`;
                  `tools/maps/assemble.rs` builds
                  `assemble_map`'s typed graph (resolved references,
                  dangling ones reported as `problems`, published as a
                  JSON asset past the inline limit);
                  `tools/convert.rs` is the thin `convert_map_format` group:
                  it turns the parameters into a `wad::map::convert::Target`
                  and runs the pure conversion as one `Session::convert_map`
                  step (`dry_run` converts a copy and only reports);
                  `tools/features.rs` is the `features` group: `add_door`
                  composes the `wad::map::features` door core and the
                  overlay engine into one `Session::edit_map` step
                  (void strip, recess paint, `apply_door`; `add_window`
                  reuses the door placement engines and the
                  `features::window` core for its raised blocking strip;
                  `add_stairs` runs the `features::stairs` core (plan,
                  then bottom-up `Paint::Sector` paints) as one step;
                  `add_lift` runs the `features::lift` core (an existing
                  or overlay-drawn platform, its lower-facing edges
                  turned front-down and tagged) as one step;
                  `add_teleporter` runs the `features::teleport` core (the
                  pad and destination squares, the type-14 destination
                  things and their outward-facing border actions, one-way
                  or `two_way`) as one step;
                  the `add_switch`/`add_exit` tools are one step over the
                  `features::switches` mount core (`build_switch`'s panel
                  fit, `split_linedef` isolation, tag resolution and
                  dressing; `build_walk_over_exit`'s sector-boundary level
                  ends), and
                  the `mark_secret`/`set_light_effect` tools are the two
                  special-bit steps over `features::specials`);
                  `tools/inspect.rs`' `diff_wads` resolves side strings
                  ("working", "merged", "base", "layer:N" or an absolute
                  WAD path) onto `crate::wad::diff`, and its `optimize_wad`
                  runs the pure `crate::wad::optimize` plan over the
                  working/base/merged snapshots and applies it as one
                  `Session::edit`;
                  `tools/validate.rs` is the thin `validate_map` group: it
                  parses `min_severity`/`codes`/`limit`, reads the map and
                  the session's definitions under one lock and runs the
                  pure `wad::map::validate` checks, paging and counting the
                  issues outside them;
                  `tools/palette.rs`' `load_palette` is its palette side;
                  `tools/themes.rs` is the `themes` group plus the shared
                  `theme_texture(session, role)` every role-asking tool uses,
                  and `theme_roles(session, override, roles)` which
                  `build_map_from_plan` uses to resolve a whole dressing set
                  against an optional one-call theme override;
                  `tools/plan.rs` is the `plan` group: `build_map_from_plan`
                  maps its wire `rooms`/`connections` onto `crate::plan::model`
                  (`tools/plan/params.rs`, which also shape-validates the
                  `things`/`player_start` rows), carries the coordinates through
                  `crate::plan::units` (`units`/`grid` parameters), resolves
                  the eight `TextureRole` dressing slots through the theme, runs `crate::plan::compile`,
                  stands the things in with `crate::plan::things` (name
                  resolution over the session definitions, room grids,
                  presence flags; a type 1 row is mandatory) and
                  `wad::map::write_map`, and appends the block as one
                  `Session::edit` via `maps::write_block` (the `finalize`
                  parameter runs the `nodes` tools' whole-set build on the
                  model inside that step and fills the `nodes` summary and
                  the errors-only `validation` result fields; the fixed-order
                  feature pipeline in `crate::plan::features` — doors,
                  switches, lifts, teleporters, exits, then the rooms' lights
                  and secrets, ending in a re-face of the wired door and lift
                  lines a teleporter rebuild turned — runs between the
                  compile and the things, and `plan::playable_checks` appends
                  its never-error key/exit/remote warnings after the
                  things);
  wad.rs          WAD format core module root: lump names, byte access, the
                  in-memory archive, the map module tree and the texture
                  definition codecs.
  wad/            Format building blocks (no MCP concerns): `wad/name.rs` the
                  eight-byte uppercase `LumpName` with glob matching,
                  `wad/bytes.rs` bounds-checked little-endian `Reader` and
                  `Writer` plus the checked usize/u32 casts,
                  `wad/archive.rs` the `Wad` archive with parsing,
                  serialization (canonical and payload-deduplicating),
                  lookup and atomic I/O, `wad/classify.rs`
                  the pure lump-directory analysis pass: `Namespace`s,
                  `MapBlock`s and `LumpKind` type hints, `wad/diff.rs` the
                  pure two-directory comparison: map blocks as units, lumps
                  keyed by namespace, name and occurrence and the
                  length/`FNV`-1a/bytes content rule, `wad/optimize.rs` the
                  pure cleanup planner (`optimize_wad`): identical overrides,
                  empty marker pairs, shadowed duplicates with the
                  `ZDoom`-concatenated-name exception, unused
                  `PNAMES`/patches/flats and the renumbered `PNAMES`/texture
                  rewrite, `wad/game.rs`
                  the pure game signature detection (`detect_game`),
                  `wad/textures.rs` the `PNAMES` and `TEXTURE1`/`TEXTURE2`
                  codecs: patch-name index, texture records with
                  `Doom`/`Strife` layout detection and patch-reference
                  renumbering, and `wad/udmf.rs` the `UDMF` `TEXTMAP`
                  grammar: tokenizer, parser and writer over `UdmfDoc`
                  blocks and key/value pairs (no map model, no MCP).
  wad/map.rs      Map layer root: the `read_map`/`write_map` format
                  dispatcher and the model re-exports.
  wad/map/        Map data (no MCP concerns): `map/model.rs` the
                  format-neutral `Map` model (`Thing`, `Vertex`,
                  `Linedef`, `Sidedef`, `Sector`, `extra` side-bags),
                  `map/doom.rs` the binary Doom map reader/writer
                  (`read_doom_map`/`write_doom_map`),
                  `map/nodes.rs` typed SEGS/SSECTORS/NODES parsing in the
                  vanilla and ZDoom `XNOD`/`ZNOD` layouts (`NodeSet`,
                  `Seg`, `Subsector`, `Node`, `Child`, magic detection),
                  `map/reject.rs` the REJECT visibility bit-matrix,
                  `map/blockmap.rs` the BLOCKMAP grid (offset table,
                  blocklist validation and the `to_bytes` serializers),
                  `map/build_reject.rs` the pure REJECT builder (`zero` and
                  `components` modes) and
                  `map/build_blockmap.rs` the pure BLOCKMAP builder
                  (bounds-derived grid, block-touching linedef lists),
                  `map/geometry.rs` pure integer
                  map geometry (sector boundaries, point-in-sector,
                  bounding boxes, line lengths and sector adjacency),
                  `map/overlay.rs` the pure room-drawing and void-carving
                  engine behind `create_sector_from_loop` and `carve_void`
                  (a polygon folded over the map through `geom::arrange`
                  rebuilds every linedef, sidedef and vertex: T-junction
                  and crossing splits, painted-over drops, two-sided
                  opening, offset-preserving side copies, the
                  `keep_interior` probe and the original-map outer side),
                  `map/ops.rs` the pure linedef-geometry engine behind
                  `split_linedef` and `join_linedefs` (integer on-segment
                  and collinearity tests, offset-preserving side copies and
                  shifts, and the removal-engine join),
                  `map/convert.rs` the pure within- and cross-flag-family
                  format conversion (`Target`, `convert_map`, `Conversion`)
                  behind `convert_map_format`, `map/features.rs` +
                  `map/features/doors.rs` the pure door core (the `DoorStyle`
                  action table, `apply_door`'s doorway dressing and `next_free_tag`),
                  `map/features/place.rs` the door placement geometry,
                  `map/features/window.rs` the pure window core
                  (`window_record`'s sill/lintel heights and
                  `apply_window`'s two-sided blocking boundary dressing),
                  `map/features/stairs.rs` the pure stair core
                  (`plan_stairs`' even step division and headroom test
                  and `build_stairs`' bottom-up overlay paints),
                  `map/features/lift.rs` the pure lift core
                  (`lift_specials`' per-family down-wait-up action and
                  `build_lift`'s lower-facing edge selection,
                  front-down orientation and platform tagging, with an
                  optional `apply_overlay` platform draw),
                  `map/features/teleport.rs` the pure teleporter core
                  (`teleport_specials`' per-family pad-border action and
                  `build_teleporter`'s pad and destination squares, the
                  type-14 destination things it places and their
                  outward-facing borders, with the `two_way` return leg and
                  the `line_remap` index handoff over the squares' rebuilds),
                  `map/features/switches.rs` the pure switch core (the
                  `switch_special` action table, `build_switch`'s wall
                  mount: panel chunk fit, `split_linedef` isolation, tag
                  resolution and dressing, and `build_walk_over_exit`'s
                  two-sided-boundary level ends) and
                  `map/features/specials.rs` the pure sector-special builders
                  (`secret_edit`'s per-family secret bit, `light_special`'s
                  per-family light effect — each keeping every bit outside
                  its field — and the `boom_note` vanilla-Doom warning the
                  `mark_secret` tool and the plan secret step share), `map/xlat.rs` the pure
                  Doom↔ZDoom linedef-special translation tables
                  (`XlatFlags`, `XlatRow`, `parse_xlat_table` and the
                  embedded-table lookups `xlat_rows`, `rows_for_doom` and
                  `rows_for_zspecial`) and the cross-family stages the
                  conversion's `ToHexen`/`FromHexen` directions run:
                  `linedefs_to_hexen`/`linedefs_from_hexen` returning
                  `LinesReport`, and `things_to_hexen`,
                  `things_from_hexen`, `sectors_to_hexen`,
                  `sectors_from_hexen` returning `FieldReport`, and
                  `map/validate.rs` +
                  `map/validate/` the pure map-health checks behind
                  `validate_map`: `validate.rs` the `Issue`/`Severity`
                  model and the `CHECKS` pipeline (`references`,
                  `structure`, `things`, `specials` submodules, one per
                  check group, each returning `Vec<Issue>`), and
                  `map/playability.rs` + `map/playability/` the pure
                  can-the-player-finish-this-map analysis:
                  `playability/specials.rs` the hand-written door/lift/
                  teleport/exit/lock/key tables read into a map's feature
                  inventory, `playability/graph.rs` the sector movement
                  graph and the player-shaped step/gap/width passability
                  tests, and `playability/reach.rs` the fixpoint that
                  walks the sectors, collects the keys it reaches and
                  opens the features both allow, answering with
                  `Playability` (reachable sectors, exits, keys, locks,
                  unopened doors, narrow lines, warnings).
                  `map/flags.rs` names the two families' flag bits and
                  carries the `Skill` whose bit a thing's presence is
                  tested against.
data/
  themes/         Per-game texture-theme TSV tables embedded in the binary
                  (`doom.tsv` also serves DOOM II and undetected sessions):
                  `theme`, `role`, `|`-separated `candidates` and
                  `fallback_role` columns, read by `src/themes/table.rs`.
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
  xlat/           TSV translation tables embedded in the binary (each file
                  under the 64 KiB `large_include_file` budget), parsed by
                  `src/wad/map/xlat.rs`. `doom_to_zdoom.tsv` is the 141-row
                  Doom→ZDoom linedef-special table with `doom`,
                  `activation`, `zspecial`, `name` and `args` columns, the
                  args column naming the line-tag slot with `tag`.
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
