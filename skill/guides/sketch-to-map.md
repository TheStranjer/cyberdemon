# Sketch to map

The pipeline is four calls:

`trace_sketch` (measure the drawing: region polygons, gaps, marks,
adjacency, a labelled debug image) → write a plan (rename `plan_rooms` into
rooms, set heights and dressing, add `connections` at the `gaps` centers) →
`build_map_from_plan` with the same `units` → `render_topdown` with the
sketch as `underlay` (same `units` again) to check the map against the
drawing.

`trace_sketch` only measures — it never names a region a room, a gap a door
or a mark a monster. That reading is yours; do it against the debug image
before writing the plan.

To be written. This guide will answer:

- What to assume or ask about a hand-drawn map before tracing it: scale,
  orientation, which lines are walls and which are furniture?
- How to measure the drawing with the image tools and turn measurements into
  vertices on the design grid from `design-rules.md`?
- When to trace automatically and when to place vertices by hand?
- How to choose room heights, light levels and a texture theme for a sketch
  that shows none of them?
- How to check the traced layout against the original drawing with
  `render_topdown` side by side?
- What to do when the geometry validates but does not play?
