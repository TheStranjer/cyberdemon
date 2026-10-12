# Design rules

Facts about Doom's engine and play conventions you need when choosing values
for tool parameters: dimensions, scale, lighting, gameplay. These are the
defaults of good play — break them on purpose, not by accident.

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

## Things and gameplay

- Every thing needs its own radius clear of walls, plus slack; a monster
  spawned inside geometry stays inside geometry.
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
