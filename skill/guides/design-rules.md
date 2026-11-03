# Design rules

Tools: check_playability, place_things, render_topdown, create_sector_from_loop, carve_void, split_linedef, validate_map, assemble_map

Facts about Doom's engine and play conventions you need when choosing values
for tool parameters: dimensions, scale, lighting, gameplay. These are the
defaults of good play — break them on purpose, not by accident. The section
called "The integer grid" is about the server's own geometry, which is
stricter than the game's.

## The player

- The player is 32 map units wide (a 16-unit radius) and 56 tall; the eye
  line sits 41 units above the floor. Anything the player must walk through
  needs at least the full 32 units of clear width plus slack for turning.
- Maximum step-up is 24 units: the player climbs ledges of 24 or less
  automatically. 16 is the comfortable stair step; 25 or more is a wall
  unless you build stairs.

## Geometry sizes

- Corridors: 64 units is the minimum comfortable width; 48 is the absolute
  minimum (the player fits, but monsters cannot pass each other, or you).
  96 to 128 lets a fight happen in a hallway.
- Rooms: keep ceilings at 96 or above for comfortable space, 128 and up for
  grand halls. Ceilings at 56 to 80 read as caves, ducts and bunkers.
- Doors: usually 64 to 128 wide and 8 to 16 deep (the thickness of the door
  frame in linedefs). Wider than 128 stops reading as a door and starts
  reading as a gateway.
- Walls: 16 units of thickness is the safe default.

## Verticality

- Height changes up to 24: stairs (each step at most 24; 16 is gentler).
- Lifts move a floor between levels that differ by 64 or more; below that,
  stairs are cheaper to build and read better. Give a lift shaft clearance
  around the platform and keep the switch out of sight of the whole ride.
- Drops are survivable at any height but hurt beyond about 50 units — use
  them as punishment, not as transit.

## Lighting

- Light levels run 0 to 255 and move in steps of 16.
- 128 is dim, 160 is normal indoor light, 192 is bright, 255 is full blast —
  save 255 for outdoors and drama.
- 64 to 96 is shadowy but navigable; below 32 is effectively black. Flicker
  and pulse are effects to spend, not a default state.

## Grid and textures

- Build on the map grid: snap vertices and sizes to multiples of 8, and
  prefer 16, 32 and 64 for structure. Off-grid points are for detail.
- Wall textures are usually 64 or 128 wide and tile vertically by 128. Match
  the texture's vertical span to the wall height or expect visible seams.
- Flats are always 64×64 world units; floors and ceilings tile them.

## The integer grid (hand-built geometry)

Every geometry tool here is integer end to end: a vertex is a whole map unit
(`i64` inside, `i16` once written to a binary map) and every meeting test is
an exact product. Two points are one point only when they are the same two
integers; the main slack in the arrangement is the half-unit test in the
first bullet. When you draw with `create_sector_from_loop` or `carve_void`
there is no snap parameter, so the snapping is yours:

- **Work on 8s; put structure on 16s.** Two corners are the same corner only
  when both integers match exactly — that is how a new room reuses a
  neighbour's vertex. A point is pulled onto a wall it should meet only within
  **half a unit** perpendicular of it, so a one-unit slip leaves that wall
  unsplit and the two sectors sharing no edge; the rebuild ends with free ends
  that `validate_map` reports as `unclosed_sector` and the overlay already
  warned about in its `notes`. Multiples of 8 also keep a 16-unit offset —
  the default wall thickness, and the usual wall depth — on the lattice, and
  stay far inside `i16`.
- **Make every diagonal exactly 45°** — `|dx| == |dy|`, both ends on the grid.
  A wall line is keyed by its **gcd-reduced** direction and integer cross
  constant, so a run of `(100, 98)` reduces to `(50, 49)` and is a different
  line from `(1, 1)` forever: a near-miss diagonal never stitches with the
  wall it was meant to continue, and the T-junction test is an exact sign
  test that it fails. Crossings are rounded to the nearest integer point too.
  Two 8-grid runs that are axis-aligned or exact 45°s meet at whole
  coordinates, so that rounding does nothing; with any other slope the meeting
  point is a fraction most of the time and lands up to about 0.7 units from
  where the lines truly cross, outside the half-unit window above. Keep to
  stays, jogs and exact 45°s.
- **Share vertices by copying them, never by re-deriving them.** Read the
  neighbour's corner integers with `assemble_map` and put those exact numbers
  in your loop. To join a wall mid-span, land a corner on it exactly, or
  `split_linedef` it at that point first and draw to the new vertex. Draw a
  room *against* an existing wall rather than across it: a touched wall turns
  two-sided and keeps its texture, special and tag, while a loop that grazes
  one — a diagonal especially — can lose that piece, because a wall's sides
  are labelled an eighth of a unit off its midpoint and a piece whose two
  sides read the same is dropped as interior. After each draw: `validate_map`
  (`unclosed_sector`, `crossing_lines`, `overlapping_lines`,
  `duplicate_line`) and `render_topdown` with `grid: 8` under the lines.
- **Pixels to map units: scale, then snap.** The plan path computes
  `map = origin + px × scale_num ÷ scale_den`, rounding halves away from zero,
  then pulls the result to the nearest multiple of `grid` — that is what
  `units` and `grid` do in `build_map_from_plan` and `trace_sketch`, with
  `flip_y: true` mirroring image rows. Hand-built loops carry neither, so run
  those two roundings on your pixel corners yourself before sending them, and
  pick the fraction so your corners land on 8s and the snap moves nothing:
  `8/1` gives 8 units per pixel (any whole pixel), `2/1` needs pixels in
  multiples of 4, `16/5` needs them in multiples of 5. Use one scale, origin
  and grid in every call, the `render_topdown` underlay included, or the
  picture and the map disagree.

## Things and gameplay

- Every thing needs its own radius clear of walls, plus slack; a monster
  spawned inside geometry stays inside geometry.
- How many monsters or items a room should hold is your call; the
  `place_things` tool only places the number you ask for, so decide the
  count and the area, then check the spread with `check_playability` and
  `render_topdown`.
- Every monster needs a path to the player, or it never fires. Check
  connectivity after walling anything in.
- Every map needs a player 1 start (thing type 1) and an exit the player can
  reach and trigger.
- Keys must be reachable before the doors they open, unless the design is a
  deliberate return loop. Trace it from the start, not from the door.

## Vanilla limits

Classic Doom runs out of breath near about 32 767 segs/sidedefs and, more
quickly, in visplane space; stay inside that comfort zone and favour a few
complex sectors over many tiny ones.
