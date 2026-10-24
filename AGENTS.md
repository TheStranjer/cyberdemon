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
                  `geom/arrange.rs` the pipeline root: the entry point,
                  the shared constants and polygon validation, over
                  `geom/arrange/split.rs` (chain splitting and the dedupe
                  table) and `geom/arrange/merge.rs` (side labelling and
                  the collinear merge), with the engine's tests in
                  `geom/arrange/tests.rs` plus
                  `geom/arrange/tests/cases.rs` and
                  `geom/arrange/tests/properties.rs`.
  gfx.rs          Graphics core module root: dimensions limits and shared helpers.
  gfx/            Internal image building blocks (no MCP concerns):
                  `gfx/image.rs` RGBA/indexed images and resize/flip,
                  `gfx/codec.rs` PNG/JPEG/GIF/BMP decode and PNG encode,
                  `gfx/grab.rs` the `grAb` PNG chunk, `gfx/palette.rs`
                  PLAYPAL/hex palettes, `gfx/quantize.rs` quantization,
                  `gfx/colormap.rs` the COLORMAP reader and palette
                  generator (34 tables), `gfx/composite.rs` texture
                  patch-stack folding (`compose`, `Composite`),
                  `gfx/patch.rs` the Doom patch codec's header and decoder
                  (incl. tall patches) with `gfx/patch/encode.rs` the
                  column/post encoder and `gfx/patch/tests.rs` plus
                  `gfx/patch/tests/codec.rs` and `gfx/patch/tests/header.rs`
                  its tests,
                  `gfx/flat.rs` the flat codec,
                  `gfx/sprite_name.rs` the sprite lump name grammar,
                  `gfx/sprite_rotation.rs` the engine's viewer-angle to
                  rotation-number bucketing (`rotation_for`),
                  `gfx/ops.rs` the image-op framework (`ImageOp`, `apply_ops`,
                  reports, hex/luminance helpers) with `gfx/ops/geometry.rs`
                  crop/fit/rotate/flip, `gfx/ops/resample.rs` nearest/
                  bilinear/area sampling, `gfx/ops/tone.rs` auto-levels/
                  adjust, `gfx/ops/alpha.rs` colour key and trim,
                  `gfx/ops/background.rs` the border-driven background
                  removal, `gfx/ops/threshold.rs` binarization plus the
                  `otsu_level`/`luminance_histogram`/`binary_mask` helpers
                  the sketch tools reuse, `gfx/ops/dither.rs` the
                  `Dither`-knopped quantization (Floyd–Steinberg serpentine),
                  and `gfx/ops/tile.rs` the seam-blend behind
                  `make_tileable`.
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
                  `music/midi_read.rs` the type-0/type-1 SMF reader
                  (`parse_midi`: running status, track merge to absolute
                  ticks), `music/mus2mid.rs` Doom MUS music lump ("D_*") to
                  MIDI bytes, `music/mid2mus.rs` the reverse
                  (`midi_to_mus`), and `music/sniff.rs` the magic-based
                  `sniff_music` container identification.
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
                  the `class` reader on the same scanner,
                  `defs/mapinfo.rs` the `DoomEdNums` block reader and
                  `defs/dehacked_edit.rs` the line-oriented DeHackEd/BEX
                  patch model (its constants and the `Field`, `Block`,
                  `Section`, `Patch` and `Opened` shapes) on
                  `defs/dehacked_edit/reader.rs` (the `Reading` pass,
                  `parse` and the header-word helpers) and
                  `defs/dehacked_edit/edit.rs` (`Target`, `parse_spec`, the
                  key and value checks, the `Region` lookup and the
                  `set_patch`/`remove_patch` line rewrites), with that
                  module's tests in `defs/dehacked_edit/tests.rs`.
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
                  against the caster's buffer, and `render/font.rs` the
                  5x7 bitmap font the `draw_text` overlay labels and
                  annotations of the top-down renders draw through.
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
  plan.rs         Plan compiler core root: re-exports the model, the
                  compiler and its stages (pure; no archive I/O, no tools).
  plan/           Compiler building blocks (no MCP concerns):
                  `plan/model.rs` the `Plan`/`Room`/`Connection` data model
                  and structural validation, `plan/units.rs` the integer
                  `Units` scaling, mirroring and grid snapping from plan
                  coordinates to map units with `plan/units/tests.rs` plus
                  `plan/units/tests/mapping.rs` and
                  `plan/units/tests/refusals.rs` its tests, `plan/walls.rs`
                  the thick-wall build (`room_layers`, `build_walls`,
                  `shared_boundaries`, every rectangle keyed by the
                  `WallKey` of the piece it fills) on `geom::arrange`,
                  `plan/connectors.rs` the
                  connection cut placement on those walls (a plain opening
                  spanning a whole shared run keys that wall `joined` and
                  paints no band), `plan/emit.rs` the
                  arrangement-to-`Map` emission (regions to sectors, sides
                  to sidedefs), `plan/textures.rs` the eight `TextureRole`
                  dressings of the emitted map (non-door band flats
                  inherited from the adjoining rooms), `plan/features.rs` the
                  fixed-order feature pipeline (doors, switches, lifts,
                  teleporters, exits, then room lights and secrets, ending
                  in a door/lift line re-face and the rows' line-endpoint
                  report; a mounted panel picks its wall by side, length and
                  an optional plan `at` point), `plan/things.rs` the name
                  resolution, in-room grid placement and presence flags,
                  and `plan/compile.rs` the `compile` pipeline folding it
                  all into a `CompiledPlan`.
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
                  Cyberdemon::router() in server.rs merges them. The group
                  modules are `tools/meta.rs`, `tools/assets.rs`,
                  `tools/batch.rs`, `tools/session.rs`, `tools/lumps.rs`,
                  `tools/maps.rs`, the five element groups
                  `tools/edit_things.rs`, `tools/edit_linedefs.rs`,
                  `tools/edit_sidedefs.rs`, `tools/edit_sectors.rs`,
                  `tools/edit_vertices.rs`, `tools/edit_geometry.rs`,
                  `tools/convert.rs`, `tools/features.rs`, `tools/plan.rs`,
                  `tools/sketch.rs`, `tools/graphics.rs`, `tools/imports.rs`,
                  `tools/patches.rs`, `tools/textures.rs`, `tools/themes.rs`,
                  `tools/palette.rs`, `tools/nodes.rs`, `tools/audio.rs`,
                  `tools/music.rs`, `tools/render.rs`, `tools/scripts.rs`,
                  `tools/defs.rs`, `tools/query.rs`, `tools/inspect.rs`,
                  `tools/validate.rs` and `tools/playability.rs`; the ones
                  with shared machinery split it into submodules described
                  below, e.g. `tools/lumps.rs` with `lumps/rows.rs` (shared
                  directory rows), `lumps/select.rs` (the lump selector),
                  `lumps/place.rs` (placement), `lumps/edit.rs`
                  (replace/remove/rename) and `lumps/reorder.rs` (the run
                  move), `tools/imports.rs` (the `PNAMES` reader and plan,
                  the name and region parsers, the tool descriptions and the
                  router) with `imports/patch.rs` (the patch and raw
                  imports), `imports/flat.rs` (the flat import with its size
                  and shape parsing), `imports/image_pipeline.rs` (the shared
                  patch-import run), `imports/sprite.rs`,
                  `imports/texture.rs`, `imports/create_flat.rs`,
                  `imports/create_sprite.rs`, `imports/processed.rs` (the
                  preview pipeline) and `imports/tests.rs` (that group's
                  unit tests), `tools/scripts.rs` with
                  `scripts/text_edit.rs` (the shared text-lump editor behind
                  `edit_decorate`/`edit_zscript`), `tools/defs.rs` with
                  `defs/collect.rs` (gathering the session's custom
                  definition texts), `tools/edit_things.rs` with
                  `edit_things/edit.rs` (the `edit_thing` request, report,
                  flag plan and edit steps, whose unit tests are
                  `edit_things/tests.rs`), `tools/palette.rs` with
                  `palette/set.rs` (`set_palette`'s parameter, payload and
                  request types beside its source resolution and commit
                  steps) and `palette/tests.rs` (that group's unit tests),
                  `tools/edit_vertices.rs` (the parameter and report shapes,
                  the append and removal paths and the router) with
                  `edit_vertices/moves.rs` (the move's request-to-points
                  math, the collapsed-line refusal and `move_vertex`'s run);
                  that group's tests are `edit_vertices/tests.rs` (the
                  shared fixture builders) plus
                  `edit_vertices/tests/add_move.rs` (the append and move
                  checks) and `edit_vertices/tests/remove.rs` (the removal
                  checks),
                  and `tools/edit_geometry.rs` with
                  `edit_geometry/delete.rs` (the `delete_geometry` request,
                  report and removal steps) and `edit_geometry/transform.rs`
                  (the `transform_selection` request types and its run); that
                  group's tests are `edit_geometry/tests.rs` (the shared
                  fixture builders and the deletion checks) plus
                  `edit_geometry/tests/transform.rs` (the transform request
                  checks).
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
                  special-bit steps over `features::specials`, and the
                  `place_things` tool runs the pure `wad::map::placement`
                  engine (name-resolved type, sector/bbox/circle areas,
                  proportional or per-sector counts, centred clearance
                  grids spread centre-outward, snapped facings with an
                  optional seeded xorshift32) as one step, sharing that
                  engine with the plan builder's room fills);
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
                  `tools/playability.rs` is the thin `check_playability`
                  and `describe_map` group: its root keeps the shared
                  `UNKNOWN_THING_RADIUS`, the `Prepared` inputs and `load`
                  and the two router methods, with `CheckPlayabilityParams`,
                  `parse_skill`, `Analysis`, the report's row types,
                  `PlayabilityReport`, its count, summary and mark builders
                  and `check_run` in `playability/check.rs`, the reason-keyed
                  `blocker_hint`, `step_hint`, `stuck_hint`, `stuck_rows` and
                  `unreachable_rows` in `playability/hints.rs`, the
                  `DESCRIBE_*` caps, `DescribeMapParams`, `parse_limit`,
                  `sector_selection`, `page_rows`, `DescribeReport` and
                  `describe_run` in `playability/describe.rs` and the group's
                  unit tests in `playability/tests.rs`; the first parses
                  `skill`, reads the map and the session's definitions
                  under one lock, runs the pure
                  `wad::map::playability` walk with
                  its `blockers` and `stuck_things` readings, folds the
                  reason-keyed hints and the plain summary, and sends
                  `render: true` over the `tools/render.rs` overview
                  machinery (`playability_overview`, the shared
                  `render_topdown` internals with the report's problems
                  highlighted); the second parses `sectors`/`offset`/
                  `limit` (validate first, dedupe and sort, `not_found`
                  naming the valid range), reads through the same one-lock
                  path, pages the pure `wad::map::describe` rows beside
                  its always-whole map summary, and publishes the same
                  overview, unhighlighted, by default;
                  `tools/query.rs` is the `query` group: `find_elements`
                  flattens the `Query` fields beside `map`/`offset`/
                  `limit` (check the limit first), reads the map and the
                  session's definitions under the same one-lock path the
                  playability tools use, runs the pure
                  `wad::map::query::run_query`, and pages its hits into
                  `{kind, count, indices, rows, next_offset}` with one
                  compact summary row per kind; the `bulk_edit` tool is
                  one `Session::edit_map` step of the pure
                  `wad::map::bulk::edits` engine over a `find_elements`
                  selection, with thing-type name resolution, a `max`
                  guard and a `dry_run` preview; the `replace_texture`/
                  `replace_flat`/`replace_thing_type` tools are one
                  `Session::edit` step over the `wad::map::bulk`
                  replacements, and `tools/query/
                  replace.rs` holds their shared machinery (the
                  `MapsSelector` resolution, the multi-map runner, the
                  `Stock` existence checks, the `ThingRef` doomednum-or-
                  name resolution behind `replace_thing_type` and the
                  folded `{maps, total, copied_maps, dry_run, next_steps}`
                  report);
                  `tools/sketch.rs` is the `sketch` group: `trace_sketch`
                  needs no session at all — it parses `crop`/`max_size`/
                  `ink`/the analysis and vectorize knobs, the plan `units`
                  and the `max_gaps`/`max_marks`/`noise_floor_px`/`verbose`
                  size knobs outside any lock, fetches, decodes, crops,
                  area-downscales to `max_size`, runs the `sketch::analyze`
                  and `sketch::vectorize` engines, scales every coordinate
                  back onto original-image pixels (and through `Units` into
                  grid-snapped `map_points`), thins the two long lists
                  (`thin_all`: the stroke-wide floor drops the sub-stroke
                  rows, the rest rank largest first and cut to their caps,
                  each omission counted in `gaps_total`/`marks_total`,
                  `truncated` and the warnings), reports the `R`/`G`/`M`
                  ids, adjacency and the `plan_rooms` fragments with each
                  region's id lists narrowed to the rows it carries, and
                  publishes the overlay of `tools/sketch/debug.rs` (washed
                  sketch, filled region polygons, the reported gap circles
                  and mark boxes, bitmap-font ids) as a `PNG` asset;
                  `tools/palette.rs` is the `palette` group: `get_palette`
                  beside the shared `load_palette` reader, the swatch
                  builder, the constants both halves share and the two
                  router methods, with `set_palette`'s parameter, payload
                  and request types, source resolution and commit steps in
                  `palette/set.rs` and the group's unit tests in
                  `palette/tests.rs`;
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
                  lines a teleporter rebuild turned and a pass pairing every
                  reported line with its map-unit endpoints — runs between the
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
                  `map/hexen.rs` the binary `Hexen` map reader/writer
                  (the vanilla block plus its `BEHAVIOR`),
                  `map/common.rs` the record codecs the two binary formats
                  share (`SIDEDEFS`/`VERTEXES`/`SECTORS` are byte-identical),
                  `map/udmf_map.rs` the `UDMF` `TEXTMAP`/`ENDMAP` reader and
                  writer over the model,
                  `map/nodes.rs` typed SEGS/SSECTORS/NODES parsing in the
                  vanilla and ZDoom `XNOD`/`ZNOD` layouts (`NodeSet`,
                  `Seg`, `Subsector`, `Node`, `Child`, magic detection, the
                  record sizes and child-word masks both layouts share, and
                  its two `pub use` entry points) on `nodes/vanilla.rs` (the
                  vanilla record walker, whose `read_box` the node records of
                  both layouts reuse) and `nodes/extended.rs` (the extended
                  lump's magic, `zlib` cap and four sections), with the two
                  layouts' tests in `nodes/tests.rs`,
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
                  `map/remove.rs` + `remove/cascade.rs` +
                  `remove/renumber.rs` the pure element-removal engine
                  shared by the edit tools (the `Removal`, `Dependents` and
                  `RemovalReport` model with `dependents`, `expand_cascade`
                  and `apply_removal` in the root, the orphan-growth rules
                  in `remove/cascade.rs`, the index shifts,
                  dangling-reference repair and one-sided normalisation in
                  `remove/renumber.rs`, and the engine's tests in
                  `remove/tests.rs` with `remove/tests/cascade.rs` and
                  `remove/tests/apply.rs`),
                  `map/transform.rs` + `transform/select.rs` +
                  `transform/apply.rs` the pure selection transform: the
                  integer scale/rotate/translate math (the `Point`, `Scale`,
                  `Rotation` and `Operations` values, the `SelectionRequest`
                  and `Selection`, `TransformReport`, `Move` and `ThingMove`
                  shapes and the whole-map `apply_transform` step behind
                  `transform_selection`) in the root, the request-to-`Selection`
                  expansion in `transform/select.rs`, the planned moves, the
                  default origin, fit, collapse and coincidence checks and the
                  commit in `transform/apply.rs`, with that module's tests in
                  `transform/tests.rs` plus `transform/tests/maths.rs` and
                  `transform/tests/engine.rs`,
                  `map/stats.rs` the pure fold of a decoded map into the
                  `MapSummary` the `get_map_summary` tool reports (counts,
                  bounds, line and height stats, thing tallies, used names
                  and node-lump facts),
                  `map/bulk.rs` + `map/bulk/replace.rs` the pure single-map
                  replacement passes (`replace_textures`, `replace_flats`,
                  `replace_thing_types`, the `Face` and `Surface` knobs)
                  the CD-108 bulk `replace_*` tools fold over maps, and
                  `map/bulk/edits.rs` the pure field-edit engine
                  (`FieldEdits`, `EditScope`, `apply`, `Applied`) behind
                  the CD-108 `bulk_edit` tool: per-kind `set`/`add`
                  tables over all five element kinds, the family-gated
                  linedef fields and the named flag edits,
                  `map/placement.rs` the pure thing-placement core: the
                  clearance primitives (`fits`, `keeps_distance`,
                  `boundary_clear`, `axis_points`, `thing_clearance`) the
                  plan builder's room fills share with the `distribute`
                  grid engine (centred clearance grids spread by distance
                  from the area centre), the `split_proportional` area
                  split and the seeded/face modes behind `place_things`,
                  `map/convert.rs` the pure within- and cross-flag-family
                  format conversion (`Target`, `convert_map`, `Conversion`)
                  behind `convert_map_format`,
                  `map/describe.rs` the pure description core behind
                  `describe_map`: one `Describe` read per map folding
                  per-sector fact rows (`SectorFacts`, `sector_facts`:
                  shoelace area, bbox, label centre, flats, sky, named
                  special, tag, wall-texture tally, counted things, role
                  names and neighbour `via` rows) and the whole-map
                  `MapOverview` summary, `map/features.rs` +
                  `map/features/doors.rs` the pure door core (the `DoorStyle`
                  action table, `apply_door`'s doorway dressing and `next_free_tag`),
                  `map/features/place.rs` the door placement geometry,
                  `map/features/window.rs` the pure window core
                  (`window_record`'s sill/lintel heights and
                  `apply_window`'s two-sided blocking boundary dressing),
                  `map/features/stairs.rs` the pure stair core
                  (`plan_stairs`' even step division and headroom test
                  and `build_stairs`' bottom-up overlay paints),
                  `map/features/lift.rs` + `map/features/lift/build.rs` the
                  pure lift core (`lift_specials`' per-family down-wait-up
                  action in the root; `build_lift`' lower-facing edge
                  selection, front-down orientation and platform tagging,
                  with an optional `apply_overlay` platform draw, in
                  `lift/build.rs`) with `map/features/lift/tests.rs` its
                  tests,
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
                  model and the `CHECKS` pipeline (`validate/references.rs`,
                  `validate/structure.rs`, `validate/things.rs`,
                  `validate/specials.rs`, `validate/geometry.rs`,
                  `validate/resources.rs` and `validate/node_lumps.rs`, one
                  per check group, each returning `Vec<Issue>`), and
                  `map/playability.rs` + `map/playability/` the pure
                  can-the-player-finish-this-map analysis:
                  `map/playability/specials.rs` +
                  `playability/specials/` the hand-written door/lift/
                  teleport/exit/lock/key tables read into a map's feature
                  inventory: `specials.rs` the constants, the `Effect`,
                  `Feature`, `ExitLine`, `LockLine`, `KeyThing` and
                  `Inventory` results, the shared `Reading` and `Plan`,
                  the `Reader` struct and the `spawns`, `start_thing`,
                  `start_sector` and `read_inventory` entry points, on
                  `specials/tables.rs` (the row types and the `DOOM_ROWS`,
                  `DOOM_EXITS`, `HEXEN_ROWS`, `HEXEN_IGNORED`,
                  `HEXEN_EXITS` and `KEY_THINGS` tables) and
                  `specials/reader.rs` (the whole `impl Reader` pass),
                  with that group's tests in `specials/tests.rs`,
                  `playability/graph.rs` the sector movement
                  graph and the player-shaped step/gap/width passability
                  tests, `playability/reach.rs` the fixpoint that
                  walks the sectors, collects the keys it reaches and
                  opens the features both allow, answering with
                  `Playability` (reachable sectors, exits, keys, locks,
                  unopened doors, narrow lines, warnings),
                  `playability/explain.rs` the per-unreachable-sector
                  blocker reasons read off a finished walk, and
                  `playability/stuck.rs` the body-square tests for the
                  things that cannot stand.
                  `map/query.rs` is the pure filter engine behind
                  `find_elements`: a `Query` (element kind, ANDed field
                  `Filter`s over stored and derived values, an optional
                  `Within` bbox/sector bound) runs as one
                  `run_query(map, ctx, query)` over the model, the
                  `QueryContext` carrying the flag family and the game
                  definitions the derived fields read, and answering the
                  ascending matching indices.
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
  mcp/            support.rs: rmcp client harness (connect, call, call_err,
                    raw-HTTP PUT/GET to the asset origin) plus a probe server
                    speaking the structured-result contract and the picture
                    helpers (PNG/MIDI/WAV builders, paper sheets);
                    fixtures.rs: WAD builders writing archives with the
                    library; one test module per tool group (server, assets,
                    session, save, batch, defs, lumps, map_edit,
                    maps, edit_*, features, graphics, imports, inspect,
                    nodes, palette, patches, plan, playability, query,
                    render, scripts, sketch, themes, validate) with the
                    `convert` group's shared helpers in `convert.rs` and
                    its tests in `convert/udmf.rs` (same-family rewrites),
                    `convert/forward.rs` (`Doom` into `Hexen` family records)
                    and `convert/reverse.rs` (the trips back and the
                    refusals); and the `textures` group's shared helpers in
                    `textures.rs` with its tests in `textures/list.rs` (the
                    `PNAMES` and `TEXTURE1`/`TEXTURE2` listings),
                    `textures/pnames.rs` (`edit_pnames`) and
                    `textures/build.rs` (`build_texture`, `remove_texture`),
                    and the `patches` group's shared helpers in `patches.rs`
                    with its tests in `patches/add.rs` (`add_patch`),
                    `patches/replace.rs` (`replace_patch`, carrying its
                    `refuse_replacements` half of the refusal table) and
                    `patches/remove.rs` (`remove_patch`),
                    and the `edit_linedefs` group's shared helpers in
                    `edit_linedefs.rs` with its tests in
                    `edit_linedefs/add.rs` (`add_linedef`),
                    `edit_linedefs/edit.rs` (`edit_linedef`) and
                    `edit_linedefs/remove_split.rs` (`remove_linedef`,
                    `split_linedef` and `join_linedefs`), and the `defs`
                    group's shared helpers in `defs.rs` with its tests in
                    `defs/things.rs` (the thing tables, the detected default
                    game and their filters), `defs/custom.rs` (the DEHACKED,
                    DECORATE, `ZScript` and MAPINFO rows joining that table),
                    `defs/specials.rs` (the linedef special tables with their
                    generalized decoding and `family` override) and
                    `defs/sectors.rs` (the sector type tables with their Boom
                    decoding);
                    the flagship chains in e2e_sketch.rs, e2e_assets.rs,
                    e2e_edit.rs and the capstone low-level workflow plus the
                    spawned-binary asset-server proof in e2e.rs; a
                    usability pass over hints and descriptions in
                    usability.rs; docs.rs: skill/ and README.md layout and
                    tool/doc consistency checks.
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
