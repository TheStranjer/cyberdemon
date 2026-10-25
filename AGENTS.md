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
                  `bsp/gl.rs` the glBSP v2 writer's root: the constants,
                  `GlNodes`, `to_gl` and the refusal makers, on
                  `gl/regions.rs` (the subsector region polygons and the
                  miniseg-closed clockwise seg loops) and `gl/write.rs`
                  (the vertex table, the seg pool, the partner pass and
                  the `GL_*` record lumps), with that module's tests in
                  `gl/tests.rs` plus `gl/tests/loops.rs` (the closed
                  loops) and `gl/tests/records.rs` (the record layout
                  and the refusals).
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
                  caster's root: the pitch limits, `ViewCamera`,
                  `MaskedMiddle`, `Rendered` and `ViewFrame` beside the
                  `render_view` and `render_frame` entry points, on
                  `view/projection.rs` (the exact `Fraction`s and the
                  camera's `Projection`), `view/walk.rs` (the `Edge`,
                  `Side`, `Plane`, `Hit`, `Visit`, `Window`, `Walk`,
                  `Slot`, `FlatRun`, `FacePlan` and `Part` rows a column
                  moves through), `view/caster.rs` (the `Caster` and its
                  sorted portal column walk: textured upper/middle/lower
                  faces with pegging, world-aligned textured planes, a
                  view-angled sky, a per-pixel depth buffer and the
                  masked-middle collection) and `view/texels.rs`
                  (piercing, plane and wall texels, pegging, texture
                  keys and the edge and sidedef preparation), with that
                  module's tests in `view/tests.rs` plus
                  `view/tests/math.rs` (the exact maths),
                  `view/tests/texturing.rs` (the mapping helpers),
                  `view/tests/box_render.rs` (the box map pixel by
                  pixel) and `view/tests/masked.rs` (the divider and the
                  transparent pass), and
                  `render/sprites.rs` the transparent pass: thing sprites
                  projected through the eight-rotation bucketing and the
                  masked middles painted far to near, depth-tested
                  against the caster's buffer, on
                  `render/sprites/placement.rs` (the pixels-per-map-unit
                  `Scale`, the projected `Axis` and the `Placement` that
                  turn a patch into the columns and rows it touches)
                  whose tests are `render/sprites/tests.rs` plus
                  `render/sprites/tests/projection.rs` and
                  `render/sprites/tests/sampling.rs`, and
                  `render/font.rs` the 5x7 bitmap font the `draw_text`
                  overlay labels and annotations of the top-down renders
                  draw through.
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
                  on `plan/model/validate.rs` (the `impl Plan` validation
                  pass: the size and thickness budgets and the room,
                  connection and feature checks), with that module's tests
                  in `plan/model/tests.rs` plus `plan/model/tests/wire.rs`
                  (the serde shape) and `plan/model/tests/validate.rs`
                  (the refusals), `plan/units.rs` the integer
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
                  paints no band) on `plan/connectors/frame.rs` (the stair
                  run, the `Frame` a band is drawn in and the rounded integer
                  helpers) and `plan/connectors/pass.rs` (the boundary choice,
                  the refusals and the `Pass` placing one connection), with
                  that module's tests in `plan/connectors/tests.rs` plus
                  `plan/connectors/tests/bands.rs` (the placed bands and their
                  refusals) and `plan/connectors/tests/stairs.rs` (the step
                  stacks, the joined runs and the helper measures),
                  `plan/emit.rs` the
                  arrangement-to-`Map` emission (regions to sectors, sides
                  to sidedefs) on `plan/emit/sectors.rs` (the room and
                  connector `Sector` rows with their span and light reads)
                  and `plan/emit/lines.rs` (the pieces to `Linedef`s,
                  placeholder sides, shared vertices and the connector
                  door-line pairing), with that module's tests in
                  `plan/emit/tests.rs` plus
                  `plan/emit/tests/geometry.rs`, `plan/textures.rs` the eight `TextureRole`
                  dressings of the emitted map (non-door band flats
                  inherited from the adjoining rooms), `plan/features.rs` the
                  fixed-order feature pipeline's root (the dress, the rows,
                  the run and its re-face and line-endpoint report) on
                  `plan/features/doors.rs` (the door wiring),
                  `plan/features/switches.rs` (the switch and exit steps, a
                  mounted panel picking its wall by side, length and an
                  optional plan `at` point) with the chooser itself on
                  `plan/features/switches/wall.rs`,
                  `plan/features/lifts.rs`,
                  `plan/features/teleporters.rs`,
                  `plan/features/rooms.rs` (the lights and secrets) and
                  `plan/features/checks.rs` (the playability checks and the
                  switch step's remote-door lookup), with that module's
                  tests in `plan/features/tests.rs` (the shared plan and
                  map builders) plus `plan/features/tests/doors.rs`,
                  `plan/features/tests/switches.rs` with its
                  `tests/switches/wall.rs` and `tests/switches/points.rs`,
                  `plan/features/tests/lifts.rs`,
                  `plan/features/tests/teleporters.rs`,
                  `plan/features/tests/rooms.rs` and
                  `plan/features/tests/pipeline.rs`,
                  `plan/things.rs` the name
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
                  reads and `Session::edit`: `LayerRole`, `Layer` and the one
                  `impl Session` every tool reaches through, its read-view
                  methods delegating to `session/views.rs`.
  session/        Session building blocks (no MCP concerns):
                  `session/scope.rs` the origin and read-target types of those
                  archives (`Source`, `MergedLump` and `Scope` with the name
                  parser behind `Scope::parse`),
                  `session/merge.rs` folds one archive over another with vanilla
                  override rules (map blocks, namespaces, globals, last wins;
                  `merge_into` and `merge_counts` are the whole-archive forms)
                  on `session/merge/pass.rs` (the `Pass` copying one overlay's
                  map blocks, regions and globals, and its marker, lookup,
                  analysis and tagging helpers), with that module's tests in
                  `session/merge/tests.rs` plus
                  `session/merge/tests/rules.rs` (the override rules) and
                  `session/merge/tests/entry_points.rs` (the whole-archive
                  forms and helpers),
                  `session/history.rs` is the labelled undo/redo stack and the
                  revision bookkeeping behind dirty state, and
                  `session/cow.rs` the copy-on-write step that brings one map
                  block from the merged view into the working archive
                  (`copy_map_block`, `Session::copy_map_to_working`),
                  `session/views.rs` the bodies of the session's read views
                  (`merged`, `base_lumps`, `game`, `lumps_in` and `find`) as
                  free functions over a `&Session`, with the layer directory
                  and lookup-match helpers they share, and
                  `session/map_edit.rs` the single undoable step behind every
                  map edit tool (`Session::edit_map`: copy, typed edit, node
                  invalidation on geometry change, in-place block rewrite)
                  and its format-rewriting sibling `Session::convert_map`
                  (`rewrite_in_place`: the same step, always dropping the
                  block's node data and `GL` run), with the session's own
                  tests in `session/tests.rs` (the fixtures the topics share)
                  plus `session/tests/layers.rs` (the scope names and wire
                  forms, the layer stack, the merged view, `find` and the
                  `Scope` reads), `session/tests/history.rs` (labels, undo,
                  redo, dirty state, the cap and the batch marks) and
                  `session/tests/state.rs` (the active map, the render
                  settings and theme, the layer-only view, game detection
                  and the copy-on-write step).
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
                  `lumps/parse.rs` (the shared scope, namespace, kind, map,
                  limit and hex argument parsers), `lumps/list.rs` (the
                  `list_lumps` and `search_lumps` arguments, criteria and
                  runs), `lumps/get.rs` (`get_lump`'s formats, bodies and
                  delivery), `lumps/extract.rs` (`extract_lump`'s target,
                  overwrite rule and read), `lumps/add.rs` (`add_lump` and
                  the two import tools' payloads, plans and one undoable
                  insert), `lumps/place.rs` (placement), `lumps/edit.rs`
                  (the working-archive selector over `edit/replace.rs` (the
                  copy-on-write payload swap), `edit/remove.rs` (the
                  working-archive deletions) and `edit/rename.rs` (the
                  in-place rename), with those tools' tests in
                  `edit/tests.rs` plus `edit/tests/replace.rs`,
                  `edit/tests/remove.rs` and `edit/tests/rename.rs`) and
                  `lumps/reorder.rs` (the run move), that group's own tests
                  in `lumps/tests.rs` plus `lumps/tests/parse_search.rs`,
                  `lumps/tests/get.rs` and `lumps/tests/add.rs`,
                  `tools/imports.rs` (the
                  `PNAMES` reader and plan, the name and region parsers, the
                  tool descriptions and the router) with
                  `imports/patch.rs` (the patch and raw
                  imports), `imports/flat.rs` (the flat import with its size
                  and shape parsing), `imports/image_pipeline.rs` (the shared
                  patch-import run), `imports/sprite.rs`,
                  `imports/texture.rs` (the limits, the parameters, the
                  validated request and the run) with `texture/size.rs` (the
                  tiling size rule), `texture/build.rs` (the automatic shape
                  ops, the built patch and the composited preview) and
                  `texture/commit.rs` (the plan, the collision rules, the
                  one-edit commit, the theme role and the report), and that
                  tool's tests in `texture/tests.rs` plus
                  `texture/tests/size.rs` and `texture/tests/plan.rs`,
                  `imports/create_flat.rs`,
                  `imports/create_sprite.rs` (the constants, the parameters,
                  the validated request and the three naming forms) with
                  `create_sprite/build.rs` (the cleanup ops, the height
                  rule, the built patch, the warnings, the one-edit commit
                  and the report) and that tool's tests in
                  `create_sprite/tests.rs` plus
                  `create_sprite/tests/request.rs` and
                  `create_sprite/tests/build.rs`, `imports/processed.rs` (the
                  preview pipeline) and `imports/tests.rs` (that group's
                  unit tests), `tools/session.rs` (the shared path,
                  archive-read and dirty-work helpers, `UNSAVED_WORKING` and
                  the router) with `session/open.rs` (the three load tools'
                  parameters, reports and runs), `session/merge_save.rs`
                  (`merge_wad` and `save_wad` with their modes, the target
                  resolution and the layer-overwrite refusal),
                  `session/info.rs` (the session and merged-view reports'
                  shapes and builders), `session/undo.rs` (`undo` and `redo`;
                  named apart from `session/history.rs`, the stack itself)
                  and `session/tests.rs` (that group's unit tests),
                  `tools/scripts.rs` (the shared lump-name, scope and
                  directory helpers, `DEFAULT_LUMP`, `UNDO_LABEL` and the
                  router) with `scripts/get_dehacked.rs` (`get_dehacked`'s
                  parameters, the report rows, their builders and its run),
                  `scripts/edit_dehacked.rs` (the `edit_dehacked` parameters,
                  the draft, the validated line `Op`s, the source fetch, the
                  change counting, the shared `write_lump` and the one-edit
                  run), `scripts/text_edit.rs` (the shared text-lump editor
                  behind `edit_decorate`/`edit_zscript`) and
                  `scripts/text_tools.rs` (the two script tools' descriptors,
                  parameters, report and runs), with that group's unit tests
                  in `scripts/tests.rs` plus `scripts/tests/dehacked.rs` (the
                  patch pair) and `scripts/tests/text.rs` (the script tools),
                  `tools/defs.rs` with
                  `defs/collect.rs` (gathering the session's custom
                  definition texts), `tools/edit_things.rs` with
                  `edit_things/edit.rs` (the `edit_thing` request, report,
                  flag plan and edit steps, whose unit tests are
                  `edit_things/tests.rs`),
                  `tools/edit_linedefs.rs` (the constants, the reported
                  `LinedefDetail`, the flag adjustments and the checks every
                  resulting linedef faces, and the router) with
                  `edit_linedefs/endpoints.rs` (the two endpoint forms and
                  their resolution against the map), `edit_linedefs/add.rs`
                  (the `add_linedef` parameter, new side and report shapes
                  beside its validation and one-step work),
                  `edit_linedefs/edit.rs` (the `edit_linedef` parameter and
                  report shapes, the plan and the step rewriting the selected
                  lines), `edit_linedefs/side_edit.rs` (the `SideEdit`
                  parameter, the sidedef it computes and the side it points
                  at) and `edit_linedefs/remove.rs` (the
                  `remove_linedef`, `split_linedef` and `join_linedefs`
                  parameters, reports and runs); that group's tests are
                  `edit_linedefs/tests.rs` (the shared fixture builders) plus
                  `edit_linedefs/tests/add.rs` (the endpoint, side, family
                  and append checks) and
                  `edit_linedefs/tests/edit.rs` (the edit checks and the
                  cascading removals),
                  `tools/palette.rs` with
                  `palette/set.rs` (`set_palette`'s parameter, payload and
                  request types beside its source resolution and commit
                  steps) and `palette/tests.rs` (that group's unit tests),
                  `tools/patches.rs` (the parameters, the staged bytes and
                  their staging, the result reports, the `PNAMES` constant
                  and the router) with `patches/write.rs` (the `add_patch`
                  and `replace_patch` runs beside their merged-view scans
                  and one-edit commits), `patches/remove.rs` (the
                  `remove_patch` run with its working scan, the guarded
                  lump deletion and the `PNAMES` rewrite commit) and
                  `patches/tests.rs` (that group's unit tests),
                  `tools/edit_sectors.rs` (the constants, the reported
                  `SectorDetail`, the checks every resulting sector faces
                  and the router) with `edit_sectors/edit.rs` (the
                  `add_sector`, `edit_sector` and `remove_sector` parameter
                  and report shapes beside their planning and one-step
                  work) and `edit_sectors/draw.rs` (the
                  `create_sector_from_loop` and `carve_void` parameters and
                  the overlay requests they build); that group's tests are
                  `edit_sectors/tests.rs` (the shared fixture builders) plus
                  `edit_sectors/tests/edit.rs` (the add, edit and removal
                  checks) and `edit_sectors/tests/draw.rs` (the loop and
                  carve requests),
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
                  `tools/graphics.rs` (the limits, the page, glob, last-wins
                  and merged-row helpers and the router) splits into
                  `graphics/list.rs` (the `list_flats` and `list_patches`
                  arguments, rows, the `PNAMES` and candidate rules and the
                  runs), `graphics/list_sprites.rs` (the `list_sprites`
                  arguments, report, `Groups`, the prefix check and the
                  rotation rows), `graphics/picture.rs` (the shared image
                  options, `AssetFormat`, `Rendered`, `Prepared`, the render,
                  `grAb` and publish helpers and the report),
                  `graphics/get.rs` (the three `get_*` parameters, the sprite
                  request either-or, the frame and rotation parsers and the
                  runs), `graphics/thing_sprite.rs` (the `get_thing_sprite`
                  parameter and report, `session_game`, the session's thing
                  definitions and the preparation) and `graphics/decode.rs`
                  (`decode_patch`'s parameter, payload rule and
                  both-encoding report); that group's tests are
                  `graphics/tests.rs` (the shared fixture, lump-bytes and
                  argument builders) plus `graphics/tests/list.rs` (the three
                  listings with their paging, grouping and override checks)
                  and `graphics/tests/picture.rs` (the image options,
                  selectors, offsets, renders and thing-sprite checks).
                  `tools/graphics/resolve.rs` holds the shared picture
                  resolvers' name lookup (`Place`, `locate`, `lump_bytes`),
                  flat and patch loading and `PNG` quantizing,
                  `resolve/texture.rs` the composite texture definitions
                  and `resolve/sprite.rs` the sprite rotations (`Sprite`,
                  served by `Request`), that every tool and the renderers
                  decode graphic lumps through, with those resolvers'
                  tests in `resolve/tests.rs` plus
                  `resolve/tests/pictures.rs` and
                  `resolve/tests/textures.rs`;
                  `tools/maps/common.rs` holds the shared `map`-parameter
                  resolvers (`resolve_map_name`, `load_map`) every map
                  tool builds on, and `tools/maps/params.rs` the shared
                  editing parameter types (`FlagsInput`, `IndexSelector`,
                  `check_index`, `validate_texture_name`, `edit_result_fragment`)
                  every map edit tool builds on; `tools/maps/manage.rs`
                  the block-level machinery behind `create_map`,
                  `copy_map`, `rename_map` and `remove_map` (the plans,
                  the `GL` helpers and the block builders) on
                  `maps/manage/runs.rs` (the four runs and their reports,
                  re-exported through `manage::`), with that module's
                  tests in `maps/manage/tests.rs` (the shared fixtures)
                  plus `maps/manage/tests/plans.rs` (the pure validators
                  and helpers) and `maps/manage/tests/runs.rs` (the runs
                  end to end);
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
                  `tools/query.rs` is the `query` group: its root keeps the
                  row-cap and report-window constants beside the whole
                  `#[tool_router]` impl, `query/find.rs` the `find_elements`
                  parameters beside its read of the map and the session's
                  definitions under the same one-lock path the playability
                  tools use, the five compact summary rows and the paging of
                  the pure `wad::map::query::run_query` hits into
                  `{kind, count, indices, rows, next_offset}` with one
                  compact summary row per kind, `query/replace_tools.rs` the
                  `replace_texture`/`replace_flat`/`replace_thing_type`
                  parameters and runs, each one `Session::edit` step over
                  the `wad::map::bulk` replacements, and `query/bulk.rs` the
                  `bulk_edit` request, report and steps: one
                  `Session::edit_map` step of the pure
                  `wad::map::bulk::edits` engine over a `find_elements`
                  selection, with thing-type name resolution, a `max`
                  guard and a `dry_run` preview. `tools/query/replace.rs` +
                  `replace/` hold the machinery they share: the root keeps
                  the `MapsSelector` wire shape and its `resolve`d name
                  list, `replace/apply.rs` the multi-map runner and the
                  folded `{maps, total, copied_maps, dry_run, next_steps}`
                  report (`ApplyOptions`, `apply_to_maps`,
                  `run_replacement`), and `replace/stock.rs` the `Stock`
                  existence checks with `parse_terms`, `require_target`
                  and the `ThingRef` doomednum-or-name resolution behind
                  `replace_thing_type`; the group's tests are
                  `query/tests.rs` (the shared fixture map, session and
                  argument builders) plus `query/tests/find.rs` (the row
                  cap, the paging window, the summary rows and the read's
                  refusals), `query/tests/replace_tools.rs` (the three
                  swaps) and `query/tests/bulk.rs` (the bulk edit with its
                  guard, preview, stock checks and paged report); that
                  module's tests are `replace/tests.rs` (the shared
                  fixtures) plus `replace/tests/maps.rs` (the selections
                  and the multi-map steps) and `replace/tests/stock.rs`
                  (the stock and the parameter checks);
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
                  and mark boxes, bitmap-font ids) as a `PNG` asset. Its
                  root keeps the parameters, the constants, the run and the
                  router; the work is split over `sketch/request.rs` (the
                  validated `Request`, the `Frame` mapping, the crop, the
                  downscale and the option builders), `sketch/thin.rs` (the
                  noise floor, the ranking and the caps over the two long
                  lists) and `sketch/report.rs` (the row types and the
                  assembled `TraceReport`); that group's unit tests are
                  `sketch/tests.rs` (the fixtures the topics share) plus
                  `sketch/tests/params.rs` (the wire parsing and the crop
                  refusals), `sketch/tests/mapping.rs` (the scale, the frame
                  and one downscaled drawing measured), `sketch/tests/
                  report.rs` (the rows and the next steps) and
                  `sketch/tests/thin.rs` (the floor, the ranking, the caps
                  and the warnings);
                  `tools/palette.rs` is the `palette` group: `get_palette`
                  beside the shared `load_palette` reader, the swatch
                  builder, the constants both halves share and the two
                  router methods, with `set_palette`'s parameter, payload
                  and request types, source resolution and commit steps in
                  `palette/set.rs` and the group's unit tests in
                  `palette/tests.rs`;
                  `tools/textures.rs` is the `textures` group: its root keeps
                  the constants, the merged view's `TextureSet` loader with
                  its `WhichLump`, decode and paging helpers, `put_global`
                  (the global-lump write the `palette`, `patches` and
                  `imports` groups reuse), the re-exports those groups import
                  and the router methods, over `textures/list.rs` (the
                  `list_textures` and `list_pnames` arguments, rows and runs),
                  `textures/pnames.rs` (the `edit_pnames` arguments, the
                  `Operation` they carry and the `Plan` `plan_edit` computes),
                  `textures/pnames_rewrite.rs` (the committed change:
                  `rewrite_textures`, `PnamesRewrite`, `plan_pnames_removal`,
                  `write_pnames_rewrite`, the two `edit_pnames` runs and their
                  reports) and `textures/build.rs` (`build_texture`,
                  `remove_texture` and the definition steps
                  `create_texture_from_image` shares); that group's unit tests
                  are `textures/tests.rs` (the shared fixture lumps, session
                  and argument builders) plus `textures/tests/list.rs` (the
                  load, the two listings and their paging),
                  `textures/tests/pnames_edit.rs` (the adds and removes as one
                  undoable step and the reusable rewrite),
                  `textures/tests/pnames_plan.rs` (the rename and replace_all
                  plans, the picker, the mapping and `put_global`) and
                  `textures/tests/build.rs` (the builds, the removals and
                  their refusals);
                  `tools/themes.rs` is the `themes` group plus the shared
                  `theme_texture(session, role)` every role-asking tool uses,
                  and `theme_roles(session, override, roles)` which
                  `build_map_from_plan` uses to resolve a whole dressing set
                  against an optional one-call theme override;
                  `tools/plan.rs` is the `plan` group: `build_map_from_plan`
                  maps its wire `rooms`/`connections` onto `crate::plan::model`
                  (`tools/plan/params.rs`, whose root keeps the request, the
                  `DEFAULT_*` wire defaults, `UnitsParam` and the conversion
                  walking the three row kinds, over `params/rooms.rs`
                  (`RoomParam`, `RectParam`, `ConnectionParam` and
                  `parse_light_effect`), `params/features.rs` (`FeatureParam`
                  and its four variant builders) and `params/things.rs`
                  (`ThingParam`, `PlayerStartParam`, `KindSource`,
                  `ThingRequest`, `check_angle` and `parse_skills`), which also
                  shape-validates the `things`/`player_start` rows), carries
                  the coordinates through
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
                  things); its root keeps the constants, the theme
                  resolution (`plan_theme` with `feature_dress` and its
                  notes) and the naming helpers beside the router, while
                  `plan/build.rs` runs the rest — `plan_path` and
                  `with_plan_path`'s prefixed refusals, `plan_definitions`
                  and the resolved thing specs, the `finalize_nodes` and
                  `finalize_validation` steps, the `PlanReport` frame and
                  `build_run`'s one edit; and the group's tests are
                  `plan/tests.rs` (the fixtures the topics share) plus
                  `plan/tests/naming.rs` (the wire names, the dressing
                  roles and the plan paths) and `plan/tests/build.rs`
                  (the resolved specs, the report frame and the finalize
                  steps), with the parameter tests in
                  `plan/params/tests.rs` (the shared request builders and
                  the refusal reader) plus
                  `plan/params/tests/rooms.rs` (the room and connection
                  conversions), `plan/params/tests/features.rs` (the
                  feature rows and a room's dressing) and
                  `plan/params/tests/units_things.rs` (the `units`
                  mapping, the snap grid and the thing rows);
  wad.rs          WAD format core module root: lump names, byte access, the
                  in-memory archive, the map module tree and the texture
                  definition codecs.
  wad/            Format building blocks (no MCP concerns): `wad/name.rs` the
                  eight-byte uppercase `LumpName` with glob matching,
                  `wad/bytes.rs` bounds-checked little-endian `Reader` and
                  `Writer` plus the checked usize/u32 casts,
                  `wad/archive.rs` the `Wad` archive with parsing,
                  serialization (canonical and payload-deduplicating),
                  lookup and atomic I/O on `wad/archive/directory.rs` (the
                  raw `DirectoryEntry`, the parse walk and the two header
                  checks) and `wad/archive/io.rs` (the staging file and the
                  path errors `write_file_atomic` reports), with that
                  module's tests in `wad/archive/tests.rs` plus
                  `wad/archive/tests/bytes.rs` and
                  `wad/archive/tests/parse_io.rs`, `wad/classify.rs` the
                  pure lump-directory analysis pass's root: the `Namespace`,
                  `MapFormat` and `LumpKind` enums, the `LumpInfo`,
                  `MapBlock` and `Analysis` shapes, the size constants and
                  `analyze`, over `classify/namespaces.rs` (the marker
                  names and the open-namespace tracker), `classify/maps.rs`
                  (the binary, `UDMF` and `GL` block finding) and
                  `classify/kinds.rs` (the type hints and the `is_doom_patch`
                  header check), with that module's tests in
                  `classify/tests.rs` (the shared builders and readers) plus
                  `classify/tests/namespaces.rs` (the marker pairs, aliases
                  and warnings), `classify/tests/maps.rs` (the map blocks and
                  their formats) and `classify/tests/kinds.rs` (every name,
                  magic and fallback hint), `wad/diff.rs` the
                  pure two-directory comparison: map blocks as units, lumps
                  keyed by namespace, name and occurrence and the
                  length/`FNV`-1a/bytes content rule, `wad/optimize.rs` the
                  pure cleanup planner's (`optimize_wad`) root: the shared
                  shapes and the `plan`, `run` and `apply` entry points, over
                  `optimize/redundant.rs` (the identical-override,
                  empty-marker-pair and shadowed-duplicate sweeps, the
                  `ZDoom`-concatenated names excepted) and `optimize/unused.rs`
                  (the unused `PNAMES`/patches/flats sweeps and the renumbered
                  `PNAMES`/texture rewrite), with that module's tests in
                  `optimize/tests.rs` (the shared builders and readers) plus
                  `optimize/tests/redundant.rs` (the redundancy sweeps and the
                  plan bookkeeping) and `optimize/tests/unused.rs` (the
                  content-reading sweeps, their refusals, the commit and the
                  wire shape), `wad/game.rs`
                  the pure game signature detection (`detect_game`),
                  `wad/textures.rs` the `PNAMES` and `TEXTURE1`/`TEXTURE2`
                  codecs: patch-name index, texture records with
                  `Doom`/`Strife` layout detection and patch-reference
                  renumbering, and `wad/udmf.rs` the `UDMF` `TEXTMAP`
                  grammar's root: the constants, `UdmfValue`, `UdmfBlock`
                  and `UdmfDoc`, the `parse_udmf`, `write_udmf`,
                  `decimal_to_rounded_int` and `has_fraction` entry points,
                  `Pos`, `err_at` and the writer helpers, over
                  `udmf/cursor.rs` (the line-and-column character cursor:
                  trivia, comments and quoted strings), `udmf/parser.rs`
                  (the document, block, field and value passes, the integer
                  literals and their radixes) and `udmf/decimal.rs` (the
                  float shape and the digit-only rounding), with that
                  module's tests in `udmf/tests.rs` (the shared parse,
                  error, global and round-trip readers) plus
                  `udmf/tests/read.rs` (every value kind, the comments and
                  each grammar refusal) and `udmf/tests/write.rs` (the
                  renders, the decimal rounds and the typed value reads)
                  (no map model, no MCP).
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
                  writer over the model: the defaults and the flag-name
                  tables both halves read, the block walk dispatching a
                  document's blocks and the document builder ordering the
                  element blocks, on `udmf_map/read.rs` (the `Fields`
                  reader over one block, the five element readers, the
                  flag-boolean decoders and the shape errors) and
                  `udmf_map/write.rs` (the `Out` block builder, the five
                  element writers and the `Doom` thing booleans), with
                  that module's tests in `udmf_map/tests.rs`,
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
                  `map/overlay.rs` + `overlay/labeler.rs` +
                  `overlay/prepared.rs` + `overlay/rebuilt.rs` +
                  `overlay/geometry.rs` the pure room-drawing and
                  void-carving engine behind `create_sector_from_loop` and
                  `carve_void` (a polygon folded over the map through
                  `geom::arrange` rebuilds every linedef, sidedef and
                  vertex: T-junction and crossing splits, painted-over
                  drops, two-sided opening, offset-preserving side copies,
                  the `keep_interior` probe and the original-map outer
                  side): the request, paint and result shapes, the
                  `Source`, `Shape` and `OutsideRule` reads of the original
                  map and `apply_overlay` in the root, the `OverlayLabeler`
                  and the containment shapes it answers from in
                  `overlay/labeler.rs`, the `Prepared` snapshot, the
                  arrangement runs and the side dressing in
                  `overlay/prepared.rs`, the `Rebuilt`, `Plan`, `Built` and
                  `LineCtx` rows with their flag, orientation and texture
                  rules in `overlay/rebuilt.rs` and the integer geometry
                  from `grid_length` to `void_front` in
                  `overlay/geometry.rs`, with that module's tests in
                  `overlay/tests.rs` plus `overlay/tests/rooms.rs` (the
                  painted rooms) and `overlay/tests/carve.rs` (the
                  carvings),
                  `map/ops.rs` + `ops/split.rs` + `ops/join.rs` the pure
                  linedef-geometry engine behind `split_linedef` and
                  `join_linedefs` (the two entry points, the shapes they
                  report and the checks and geometry helpers both passes
                  share in the root, the split point, the on-segment test,
                  the side copies and the writes in `ops/split.rs`, the
                  head-to-tail, collinearity and sector tests, the
                  difference report and the removal-engine join in
                  `ops/join.rs`, with the engine's tests in `ops/tests.rs`
                  plus `ops/tests/split.rs` (the cut, its copies and its
                  refusals) and `ops/tests/join.rs` (the welds and their
                  own refusals)),
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
                  `map/bulk/edits.rs` + `edits/` the pure field-edit
                  engine (`FieldEdits`, `EditScope`, `apply`, `Applied`)
                  behind the CD-108 `bulk_edit` tool: per-kind `set`/`add`
                  tables over all five element kinds, the family-gated
                  linedef fields and the named flag edits — the request
                  and answer shapes, the range vocabulary and the `apply`
                  and `run` entry points in the root, the field tables
                  with the `KindPlan` of accessors each drives in
                  `edits/tables.rs`, the `Job` of one run with its request
                  checks and per-row `Pending` plans in `edits/job.rs`,
                  the check, plan, write and report steps in
                  `edits/commit.rs`, and the value bounds beside the typed
                  readers and writers of the five blocks in
                  `edits/access.rs`, with that module's tests in
                  `edits/tests.rs` (the stock, the request builders and
                  the sample maps) plus `edits/tests/geometry_kinds.rs`
                  (the sector, sidedef and vertex blocks),
                  `edits/tests/lines_things.rs` (the linedef and thing
                  blocks with their flags) and `edits/tests/report.rs`
                  (the answer and the entry gate),
                  `map/placement.rs` + `placement/` the pure
                  thing-placement core: the clearance primitives (`fits`,
                  `keeps_distance`, `boundary_clear`, `axis_points`,
                  `thing_clearance`) the plan builder's room fills share
                  with the `distribute` grid engine (centred clearance
                  grids spread by distance from the area centre), the
                  `split_proportional` area split and the seeded/face
                  modes behind `place_things`, on `placement/facing.rs`
                  (the `Facing` modes, `snap_angle`, `bearing`, `next_u32`
                  and the `FacingRun` threading them) and
                  `placement/distribute.rs` (`Placed`, `DistributeSpec`,
                  `Distribution`, the `Run` behind `distribute` and the
                  `Share` and `Order` of `split_proportional`), with that
                  module's tests in `placement/tests.rs` plus
                  `placement/tests/clearance.rs` and
                  `placement/tests/split_facing.rs`,
                  `map/convert.rs` + `convert/plan.rs` +
                  `convert/layout.rs` the pure within- and cross-flag-family
                  format conversion behind `convert_map_format`: the
                  `Target`, `Conversion` and `KindCounts` shapes, the
                  `convert_map` entry point with its `to_hexen`/`from_hexen`
                  table drivers and `fold_reports`, the shared constants and
                  the `format_name`/`is_node_lump` readers in the root, the
                  `Plan` and `Direction` resolution of a request against the
                  map it names on `convert/plan.rs`, and the layout rewrite —
                  `family_of`, the `into_udmf`/`into_binary` field sets, the
                  `Line_SetIdentification` folds, the node-lump, `BEHAVIOR`
                  and `extra`-bag drops and the `parse_format`/
                  `namespace_name` word checks — on `convert/layout.rs`,
                  with that module's tests in `convert/tests.rs` plus
                  `convert/tests/same_family.rs`,
                  `convert/tests/cross_family.rs` and
                  `convert/tests/requests.rs`,
                  `map/describe.rs` + `describe/` the pure description core
                  behind `describe_map`: one `Describe` read per map folding
                  per-sector fact rows (`SectorFacts`, `sector_facts`:
                  shoelace area, bbox, label centre, flats, sky, named
                  special, tag, wall-texture tally, counted things, role
                  names and neighbour `via` rows) and the whole-map
                  `MapOverview` summary — the answer shapes, the `ROLE_NAMES`
                  and `via` words, the `Describe` type and the one-sector
                  wrapper in the root, the one `impl Describe` with the
                  `door`/`lift` neighbour rules, the closed band, the window
                  gap and the walk-over sweep in `describe/reader.rs`, the
                  boundary and neighbour-group reads and the per-family
                  sector-special tables in `describe/roles.rs`, the shoelace,
                  label-point and coordinate helpers in `describe/shape.rs`
                  and the counts, tallies and playable sentences in
                  `describe/tally.rs`, with that module's tests in
                  `describe/tests.rs` (the definitions and the hand-drawn
                  fixtures) plus `describe/tests/plans.rs` (the plan-built
                  maps), `describe/tests/facts.rs` (the per-sector rows),
                  `describe/tests/neighbours.rs` (the boundary readings),
                  `describe/tests/roles.rs` (the role names) and
                  `describe/tests/overview.rs` (the whole-map summary),
                  `map/features.rs` +
                  `map/features/doors.rs` the pure door core (the `DoorStyle`
                  action table and `next_free_tag` in the root; the
                  `DoorSides`-to-map-dressing — `plan_door`'s validated
                  `RoomPlan`/`JambPlan` rows and the writes `apply_door`
                  and `face_room_line` run through them — in
                  `doors/apply.rs`), its tests in `doors/tests.rs` plus
                  `doors/tests/apply.rs` and `doors/tests/specials.rs`,
                  `map/features/place.rs` + `place/` the door placement
                  geometry: the boundary selection (`longest_boundary`,
                  `nearest_boundary`) and shared integer helpers in the
                  root, the projection and width fit (`project_onto_line`,
                  `door_segment`, `door_wall`, `door_rect`) in
                  `place/cut.rs`, the `outward_depth` raycast in
                  `place/depth.rs` and the `void_strip` boundary
                  thickening in `place/strip.rs`, with those tests in
                  `place/tests.rs` plus `place/tests/boundary.rs` and
                  `place/tests/depth_strip.rs`,
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
                  `mark_secret` tool and the plan secret step share),
                  `map/xlat.rs` + `map/xlat/` the pure
                  Doom↔ZDoom linedef-special translation tables and the
                  cross-family stages the conversion's
                  `ToHexen`/`FromHexen` directions run: the root the
                  `FieldReport`, `Untranslated` and `LinesReport` shapes
                  beside the helpers every stage shares (`move_field`,
                  `count_if`, `count_bit`, `show_indices` with
                  `GROUP_LIST_LIMIT`, `HexenDrops` and the two field masks),
                  on `xlat/table.rs` (`XlatFlags`, `XlatRow`,
                  `parse_xlat_table` and the embedded-table lookups
                  `xlat_rows`, `rows_for_doom` and `rows_for_zspecial`),
                  `xlat/things.rs` and `xlat/sectors.rs` the
                  `things_to_hexen`/`things_from_hexen` and
                  `sectors_to_hexen`/`sectors_from_hexen` stages returning
                  `FieldReport`, and `xlat/lines_to.rs` and
                  `xlat/lines_from.rs` the two directions of
                  `linedefs_to_hexen`/`linedefs_from_hexen` returning
                  `LinesReport`, with the stages' tests in `xlat/tests.rs`
                  (the shared row, map and trip builders) plus
                  `xlat/tests/table.rs` (the shipped rows and the parsing
                  refusals), `xlat/tests/things_sectors.rs` (the thing and
                  sector stages with the report wire shapes),
                  `xlat/tests/lines_restore.rs` (the round trips),
                  `xlat/tests/lines_warnings.rs` (the grouped loss warnings
                  with the byte vetoes) and `xlat/tests/lines_tags.rs` (the
                  tag and line-id rules with the whole-table trip), and
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
                  `map/query.rs` + `query/` is the pure filter engine behind
                  `find_elements`: a `Query` (element kind, ANDed field
                  `Filter`s over stored and derived values, an optional
                  `Within` bbox/sector bound) runs as one
                  `run_query(map, ctx, query)` over the model, the
                  `QueryContext` carrying the flag family and the game
                  definitions the derived fields read, and answering the
                  ascending matching indices; the root keeps those shapes,
                  the resolved `Bounds` of a `within` clause and
                  `side_sector`, `query/fields.rs` the `FieldType`
                  classification, every kind's field table and the `FValue`
                  a read widens to, `query/engine.rs` the `Prepared` filter
                  beside the `Engine` with its lazily built derived reads
                  and its per-element tests, and `query/check.rs`
                  `prepare_filter`'s field, op and value checks with the
                  `compare` family they promise, that module's tests in
                  `query/tests.rs` (the contexts, the query makers and the
                  fixture maps) plus `query/tests/filters.rs` (every op,
                  the derived fields and the two bounds) and
                  `query/tests/checks.rs` (the `AND`ing, every refusal,
                  the family tables and the wire shape).
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
                    library (the shared primitives, the two-room and L-room
                    maps and the archive writer, re-exporting every moved
                    builder) on fixtures/graphics.rs (palettes, patch,
                    sprite and texture lump bytes and the textured-view
                    `IWAD`), fixtures/iwads.rs (the `DOOM II`, stock,
                    duplicated, Heretic, Hexen, L-room and broken-map
                    `IWAD`s), fixtures/pwads.rs (the override, redundant,
                    map-format, custom-defs and empty-map `PWAD`s and the
                    not-a-`WAD` file), fixtures/nodes.rs (the hand-built
                    node-lump `PWAD` and the conversion-fixture `PWAD`) and
                    fixtures/features.rs (the door-wardrobe `IWAD`);
                    one test module per tool group (server, assets,
                    session, save, batch, defs, lumps, map_edit,
                    maps, edit_*, features, graphics, imports, inspect,
                    nodes, palette, patches, plan, playability, query,
                    render, scripts, sketch, themes, validate) with the
                    `lumps` group's shared helpers in `lumps.rs` (the
                    merged-layer and `IWAD`-only clients, the fixture lump
                    sizes, the row, working-name and undo-label readers, the
                    `path`, `source` and replace-hex argument builders, the
                    lump seeders and the refused-edit assertion) with its tests
                    in `lumps/list.rs` (the `list_lumps` scopes, filters and
                    paging), `lumps/search.rs` (the `search_lumps` globs,
                    criteria, content needles and truncation flag),
                    `lumps/get.rs` (`get_lump`'s formats, occurrence, scope and
                    asset reads beside `extract_lump`'s writes and refusals),
                    `lumps/add.rs` (`add_lump`'s payloads, placement, namespace
                    regions and twin rules), `lumps/import.rs` (the two import
                    tools' shared insert and the uploaded asset `URL`),
                    `lumps/replace.rs` (`replace_lump` in place, by occurrence
                    and as a copy-on-write override), `lumps/remove.rs` (the
                    `remove_lump` selections, rollback and namespace filter),
                    `lumps/rename.rs` (`rename_lump`'s moves and refusals) and
                    `lumps/reorder.rs` (the `reorder_lumps` run moves), the
                    `convert` group's shared helpers in `convert.rs` and
                    its tests in `convert/udmf.rs` (same-family rewrites),
                    `convert/forward.rs` (`Doom` into `Hexen` family records)
                    and `convert/reverse.rs` (the trips back and the
                    refusals), the `plan` group's shared helpers in `plan.rs`
                    (the features-`IWAD` client, the two-room plan, the refusal
                    caller and the assembled-graph row and feature-row
                    readers) with its tests in `plan/basics.rs` (the two-room
                    bookkeeping, the two opening widths, the blue-key door
                    wiring and the block's lifecycle), `plan/units.rs` (the
                    `units` scale-and-mirror and `grid` snap, the `UDMF` block
                    and the one-call `theme` override), `plan/things.rs` (the
                    `things` and `player_start` rows), `plan/finalize.rs` (the
                    finalized node run and the documented worked example),
                    `plan/features.rs` (the lift, teleporter, light, secret,
                    switch and exit rows), `plan/panels.rs` (the wall a panel
                    mounts on) and `plan/six_room.rs` (the whole playable
                    six-room map and the `UDMF` locked door); and the
                    `textures` group's shared helpers in
                    `textures.rs` with its tests in `textures/list.rs` (the
                    `PNAMES` and `TEXTURE1`/`TEXTURE2` listings),
                    `textures/pnames.rs` (`edit_pnames`) and
                    `textures/build.rs` (`build_texture`, `remove_texture`),
                    and the `patches` group's shared helpers in `patches.rs`
                    with its tests in `patches/add.rs` (`add_patch`),
                    `patches/replace.rs` (`replace_patch`, carrying its
                    `refuse_replacements` half of the refusal table) and
                    `patches/remove.rs` (`remove_patch`), and the `graphics`
                    group's shared helpers in `graphics.rs` (the fixture-`IWAD`
                    client, the `add_lump` picture builders, the asset
                    fetchers, the report checks and the refusal table) with its
                    tests in `graphics/list.rs` (the flats, patch and sprite
                    listings with their `PNAMES`, namespace, prefix and paging
                    rules), `graphics/flat_patch.rs` (`get_flat` and `get_patch`
                    beside the picture tools' missing-picture and argument
                    refusals) and `graphics/sprites.rs` (`get_sprite` with its
                    mirror rotations, `decode_patch` and `get_thing_sprite`),
                    the `imports` group's shared helpers in `imports.rs` (the
                    fixture-`IWAD` client, the palette-colour and
                    palette-cycling `PNG` builders, the `source` argument
                    builder, the asset fetcher, the stored `RGBA` readers, the
                    report checkers, the pixel readers and the refusal runner)
                    with its tests in `imports/patch.rs` (the
                    `import_image_as_patch` pipeline, its alias, colour key,
                    offsets, `PNAMES` and undo), `imports/patch_sources.rs`
                    (filename naming, the refusal catalogue, the missing
                    `PLAYPAL` and the uploaded asset `URL` source),
                    `imports/flat.rs` (`import_image_as_flat` with its
                    resizing, dither and override rules), `imports/raw.rs`
                    (`import_image_as_raw`), `imports/sprite.rs`
                    (`import_image_as_sprite` with its naming forms and mirror
                    pair), `imports/process.rs` (`process_image` previews),
                    `imports/create_texture.rs` (`create_texture_from_image`),
                    `imports/create_flat.rs` (`create_flat_from_image`) and
                    `imports/create_sprite.rs` (`create_sprite_from_image`),
                    the `render` group's shared helpers in `render.rs` (the
                    fixture-`IWAD` client, the `{"path": ...}` argument, the
                    fitted top-down `transform` and its pixel mapping, the
                    asset fetch, the PNG decode and the two pixel counters)
                    with its tests in `render/settings.rs` (the session
                    defaults and every setter's validation),
                    `render/topdown.rs` (the fitted picture, the styles,
                    sector fills, highlights and refusals),
                    `render/overlays.rs` (the `underlay` picture's rectangle,
                    the pixels it paints and the edges it must not cross),
                    `render/decorations.rs` (the underlay and isometric
                    refusals, the grid, sector labels and annotations and the
                    session defaults the frame follows), `render/minimap.rs`
                    (the automap square, its palette colours, zoom, arrow and
                    refusals), `render/view.rs` (the first-person frame,
                    pitch, lighting, floor texture, sky and masked grate),
                    `render/view_sprites.rs` (the sprite pass, the
                    stock-fixture ring and sky reads and the camera
                    refusals) and `render/scene.rs` (the `build_scene`
                    preparation the 3D view runs on),
                    and the `edit_linedefs` group's shared helpers in
                    `edit_linedefs.rs` with its tests in
                    `edit_linedefs/add.rs` (`add_linedef`),
                    `edit_linedefs/edit.rs` (`edit_linedef`) and
                    `edit_linedefs/remove_split.rs` (`remove_linedef`,
                    `split_linedef` and `join_linedefs`), the `edit_sectors`
                    group's shared helpers in `edit_sectors.rs` with its
                    tests in `edit_sectors/edit.rs` (`add_sector`,
                    `edit_sector`), `edit_sectors/remove.rs` (the format
                    caps, `remove_sector` with its cascade and the undo of
                    the add/edit/remove trio), `edit_sectors/create_loop.rs`
                    (`create_sector_from_loop` with the report and wall
                    checks the carvings reuse) and `edit_sectors/carve.rs`
                    (`carve_void`), and the `defs`
                    group's shared helpers in `defs.rs` with its tests in
                    `defs/things.rs` (the thing tables, the detected default
                    game and their filters), `defs/custom.rs` (the DEHACKED,
                    DECORATE, `ZScript` and MAPINFO rows joining that table),
                    `defs/specials.rs` (the linedef special tables with their
                    generalized decoding and `family` override) and
                    `defs/sectors.rs` (the sector type tables with their Boom
                    decoding), and the `query` group's shared helpers in
                    `query.rs` with its tests in `query/find.rs`
                    (`find_elements` with its `within` bounds and refusals),
                    `query/replace_surfaces.rs` (`replace_texture`,
                    `replace_flat`), `query/replace_things.rs`
                    (`replace_thing_type`) and `query/bulk.rs` (`bulk_edit`),
                    and the `maps` group's shared helpers in `maps.rs` (the
                    three fixture loaders, the asset-`URL` splitter and fetcher
                    and the `list_maps`/`get_map_lumps` row readers) with its
                    tests in `maps/list.rs` (`list_maps` over the scopes and
                    the `get_map_lumps` raw set), `maps/summary.rs`
                    (`get_map_summary` and `set_active_map`),
                    `maps/assemble.rs` (`assemble_map`), `maps/create.rs`
                    (`create_map`) and `maps/copy_rename.rs` (`copy_map`,
                    `rename_map` and `remove_map`), and the `nodes` group's
                    shared helpers in `nodes.rs` (the three fixture loaders,
                    the asset-`URL` splitter and fetcher, the `JSON` asset
                    reader and the directory name column) with its tests in
                    `nodes/read.rs` (`get_nodes`, `get_reject` and
                    `get_blockmap` over the hand-built, malformed and `XNOD`
                    fixtures), `nodes/build_tables.rs` (`build_blockmap` and
                    `build_reject` with their refusals), `nodes/build_nodes.rs`
                    (`build_nodes` as the classic run with its split vertices
                    and as one `ZDoom` extended lump), `nodes/build_gl.rs`
                    (the `glBSP` v2 `GL_*` block and its place in the
                    directory) and `nodes/build_all.rs` (`build_all_nodes` as
                    one undoable step); the `features` group's shared loaders
                    and readers in `features.rs` (the three `IWAD` loaders,
                    the `assemble_map` row readers, `validate_clean` and the
                    `add_door` `refuse_with`) with its tests in
                    `features/door.rs` (the touching-room door and its
                    track/key/`Hexen` dressings, the remote loop, the refusal
                    catalogue and the themed `render_view`),
                    `features/window.rs` (the blocking glass strip and its
                    refusals), `features/switch.rs` (the split panel, the
                    minted tag, the `Hexen` use lines and the remote-door
                    pairing), `features/exit.rs` (the wall-mounted and
                    walk-over level ends), `features/secret.rs` (the
                    per-family secret bits, holding the `pub(super)`
                    `set_special`/`refuse_secret`/`EffectCase` case machinery
                    the `light` module drives), `features/light.rs` (the
                    per-family basic types, the riding light level and the
                    two special tools' argument checks), `features/stairs.rs`
                    (the four rising steps, their risers and the geometry
                    refusals), `features/lift.rs` (the step platform, the
                    drawn ledge, the `Hexen` scripted plat and the undo),
                    `features/teleporter.rs` (the pad and destination
                    squares, the `two_way` return leg and the `Hexen` TID
                    threading, with the `place`-shared `reported_things`) and
                    `features/place.rs` (the `place_things` spreads, facings,
                    shortfall, dry run, undo and refusal catalogue);
                    the flagship chains in e2e_sketch.rs (the drawn sheet,
                    the session, the report readers and the plan builders
                    every slice shares) with its topic files
                    e2e_sketch/trace.rs (the `trace_sketch` measurements),
                    e2e_sketch/build.rs (the traced plan builds a playable
                    map), e2e_sketch/views.rs (the `render_topdown`
                    underlay and the `render_view` frame) and
                    e2e_sketch/save_and_lock.rs (`save_wad`/`open_wad` and
                    the locked blue door), e2e_assets.rs,
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
