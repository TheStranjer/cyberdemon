# Sketch tools

Measure a hand-drawn floor plan into exact numbers: the bridge from "here's
my drawing" to `build_map_from_plan`. The server is the reflex — it measures
ink, regions, gaps and marks in pixels — and you are the judgment: which
region is a room, which gap is a door, which mark is a monster. No open WAD
or session is needed to trace.

## `trace_sketch`

`{source, crop? {x, y, width, height}, max_size? = 1024, ink? {mode: "dark"
| "color" = "dark", level?, color?, tolerance? = 48}, close_gaps?,
min_region_area?, mark_max_size?, simplify?, orthogonal? = true, weld?,
units?, grid? = 8}` → `{image {width, height, analysed_scale {num, den}},
stroke_px, threshold, close_gaps_px, regions [{id, points, map_points,
bbox, area_px, centroid, is_rectangle, holes, marks, gaps}], adjacency
[{a, b, shared_px}], gaps [{id, center, map_center, width_px, between}],
marks [{id, center, map_center, bbox, area_px, color, region}], plan_rooms
[{id, points}], debug (asset), warnings, next_steps}`

**The source.** `source`: absolute path, `file://` URI, `http(s)://` URL or
this server's asset/upload URL (PNG, JPEG, GIF or BMP). Bytes that are no
readable image fail `format_error` (or `unsupported`).

**Scale.** The image is downscaled (area filter) so its longer side is at
most `max_size` pixels (`256..=4096`, default `1024`; out of range is
`invalid_argument`) before analysis, and **every coordinate in the result is
converted back to original-image pixels**, so the numbers match the picture
you see. `analysed_scale {num, den}` is the exact analysis-source fraction
(`1/2` = measured at half size). `crop {x, y, width, height}` (original
pixels) analyses a rectangle; coordinates still carry the crop offset and
`image` still reports the full decoded size. A crop outside the image is
`invalid_argument`. Keep walls at least `close_gaps` pixels inside the crop
edge, or the morphological closing smears them to the border.

**Ink.** `ink.mode: "dark"` (default) finds strokes by luminance: `level`
fixes the cut-off (`0..=255`), unset detects it (Otsu) and the applied value
is reported as `threshold`. `ink.mode: "color"` traces only pixels within
`tolerance` (default `48`) per channel of `color` (`#RRGGBB`) — a color mode
without `color` is `invalid_argument`, and `threshold` is then `null`.

**The measurement knobs** (all in original pixels; defaults follow the
measured stroke): `close_gaps` is the closing radius — breaks in the walls
up to twice it wide are bridged and reported as gaps (default
`min(2 × stroke, 12)` at analysis scale); `min_region_area` drops smaller
enclosures; `mark_max_size` bounds what counts as a mark (a dot, cross or
letter, not wall). `simplify` is the polygon tolerance (default
`max(2, stroke)`), `orthogonal` (default `true`) straightens near-axis
edges, and `weld` (default `max(2, stroke)`, `0` disables) gives a shared
wall one identical coordinate across the regions that meet there.

**Units.** `units {scale_num, scale_den, origin_x, origin_y, flip_y}` is the
same object `build_map_from_plan` and `render_topdown.underlay.units` take;
with it, every reported point also gets map coordinates (`map_points` on
regions, `map_center` on gaps and marks) computed as the original-pixel
point through `Units::to_map` and snapped to `grid` (default `8`, `1`
switches snapping off). Pass `flip_y: true` for image-space drawings, whose
rows grow down while map `y` grows up. Use the same `units` and `grid` again
in `build_map_from_plan` and the map lands exactly on the sketch.

**The ids are measurements, never meanings.** Regions are `R1`, `R2`, ...
(scan order), gaps `G1`, ..., marks `M1`, .... The tool never names a region
a room, a gap a door or a mark a monster: read the `debug` image, match the
ids against your own reading of the sketch, then write the plan.
`regions[].points` are pixel-corner rings, clockwise in image space (the
plan compiler accepts either orientation); `is_rectangle` and `holes`
describe the shape; `marks` and `gaps` list the ids inside/bordering the
region; `adjacency[].shared_px` measures grown-region contact, and the
rescaled pixel counts (`area_px`, `shared_px`, `stroke_px`) are estimates
when `analysed_scale` is not `1/1`. `plan_rooms` are the region polygons in
the exact wire shape of `build_map_from_plan` rooms (`{id, points}`) —
rename the ids, set heights, add `connections` at gap centers, and build.

**Zero regions is not an error.** An over-crowded or empty sheet answers
empty lists, a `warnings` line saying why, and the `debug` image; adjust
`ink`/`close_gaps`/`min_region_area` and trace again.

**The debug image** (`debug`, a published PNG asset): the analysed picture
washed out to 40%, each region filled with a distinct colour (a fixed
12-colour table, 45% alpha) with its outline darkened, gaps as magenta
circles (radius = half the gap width), marks as cyan boxes, and every `R`,
`G`, `M` id at its centroid. Fetch and look at it before writing the plan.

Errors: `invalid_argument` (bad `crop`, `max_size`, `ink`, `units`/`grid`),
`unsupported`/`format_error` (no decodable image).
