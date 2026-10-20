# Playability tools

Reading a map the way a player does: `check_playability` walks it from the
player start and answers one question with numbers - *can a player start
here, get around, and finish?* - plus why not, sector by sector, with the
tool that fixes each problem named in the row. It never changes anything;
the fixes go through `edit_sector`, `edit_linedef`, `add_stairs`, `add_lift`,
`add_door`, `add_thing`, `add_exit`, `create_sector_from_loop` and friends.
Run it after `validate_map` (see `../guides/verification.md`).

`describe_map` is the other reading eye: measured per-sector rows - area,
heights, flats, contents, roles, neighbours - beside a whole-map summary
and a labelled overview image. Read it first to turn plain words ("the
dark room with the shotguns") into sector numbers, then act through the
sector-indexed tools.

## `check_playability`

`{map?, skill? = "medium", render? = false}` →
`{ok, summary, start, sectors, blockers, exits, keys, locks,
doors_never_opened, narrow_lines, stuck_things, unreachable_things,
warnings, render, next_steps}`

- `map` and the merged scope work as in the other map tools
  (`reference/maps.md`): the active map, else the merged view's only map.
  An unknown name is `not_found`.
- `skill` is `"easy"`, `"medium"` (default) or `"hard"`, any case: thing
  presence bits decide which starts, keys and monsters the walk sees. A
  bad name is `invalid_argument`.
- `render: true` also publishes a top-down overview PNG
  (`reference/render.md`'s machinery) with the unreachable sectors tinted
  red, the blocker lines drawn thick red, the stuck things ringed in white
  and the sector labels on; the `render` field carries the `AssetRef`,
  `null` otherwise.
- `ok` is true when a player start stands in a sector, at least one
  non-secret exit can be worked from a reached side, **and** the start
  itself is not stuck. A map without a player start is a normal
  `ok: false` result, never an error.
- `summary` is one or two plain sentences: the exit answer,
  `"N of M sectors reachable."`, and the stuck-monster and
  unreachable-key counts when they are nonzero.
- `start` is `{thing, sector}` (`sector` null when the start stands in the
  void) or `null` when no type-1 thing spawns at this skill.
- `sectors` is `{total, reachable, unreachable}` with the unreached sector
  indices listed.
- `next_steps` carries `["render_view from the player start to check the
  look"]` when `ok`, else the hints of the first three problems.

## What the walk believes

Movement is modelled with the vanilla player's shape, in integers:

| Limit | Value |
|-------|-------|
| Step up | at most 24 units (falling is free at any depth) |
| Headroom, across a line and inside a sector | at least 56 units |
| Opening | a two-sided line at least 32 units long |

Doors are evaluated in their **open** state once the walk reaches a
trigger for them (and holds the key a locked one asks for); a lift
platform is available both at its stored floor and at the floor it drops
to; an opened teleporter jumps from either reached side of its line.
`narrow_lines` lists the two-sided lines too short to walk through whose
**both** sides the walk otherwise reached - ways through you can see and
not take. `doors_never_opened` lists the door sectors no opened feature
ever raised.

## `blockers` rows

For every unreachable sector next to a reachable one across a two-sided
line, one row `{sector, from, line, reason, hint}`; a sector no reachable
sector borders reads `{sector, reason: "isolated", hint}`. The `reason`
is the first failing rule at that line:

| reason | meaning | hint says |
|--------|---------|-----------|
| `impassable` | the line carries the blocking flag (or a `Hexen` player-keeping bit) | clear the flag on line N (`edit_linedef`) |
| `narrow {length?}` | the line is shorter than 32 units | widen the opening to at least 32 units (`move_vertex`) |
| `locked {color}` | a locked door whose key the walk never reached | place a `<color>` key in a reachable area (`add_thing`) |
| `door_never_opened` | a door no reachable trigger works | make the switch reachable or `add_door` style normal |
| `step_too_high {delta}` | the far floor stands more than 24 above the near one | lower it (`edit_sector`) or `add_stairs` / `add_lift` |
| `too_low_sector {height}` | the sector itself is shallower than 56 | raise the ceiling (`edit_sector`) to at least floor + 56 |
| `no_headroom {gap}` | the gap over the line is under 56 | raise the ceiling (`edit_sector`) to at least floor + 56 |
| `isolated` | nothing connects it | connect it (`create_sector_from_loop` / `add_door` / a plan connection) |

A sector with several reachable borders gets one row per border, sorted
by sector and line.

## `stuck_things`, `unreachable_things`

`stuck_things` rows are `{thing, name, reason, hint}` for things that
spawn at this skill. Every thing is checked for `outside_map` (no sector
holds its point). Monsters, player starts and unknown types (treated as
monsters, radius 20, height 56 - `Doom` things body as axis-aligned
squares of side 2 r; the definition tables carry no heights, so 56 stands
in for all) additionally get: `in_wall {line}` when a one-sided or
blocking line runs strictly through the square (touching is fine),
`low_ceiling {height}` when the sector leaves it less than 56, and
`overlaps {other}` when two bodies' squares strictly overlap - reported
once, lower index first.

`unreachable_things` lists every spawning thing standing in a sector the
walk never reached: `{thing, name, category, sector}`.

## What is not modelled yet

Boom generalized specials, ACS/`ZDoom` scripts, `UDMF`-only line effects
and skill-vanilla monster behaviour are outside the walk: unhandled line
specials are counted into `warnings`, never guessed. The walk is
single-player: `notsingle` things do not spawn. `render_topdown`
(`reference/render.md`) remains the way to look at everything the numbers
report.

## `describe_map`

`{map?, sectors?, offset? = 0, limit? = 150, render? = true}` →
`{map, summary, sectors, next_offset, image}`

One measured picture of the whole map: what every sector **is**, what it
**holds** and what it **borders**, plus the map-wide `summary` and the
labelled overview image. It never changes anything, and it never guesses:
every field is a number, a name or one of the fixed role and `via`
vocabulary words.

- `map` and the merged scope work as in the other map tools
  (`reference/maps.md`): the active map, else the merged view's only map.
  An unknown name is `not_found`.
- `sectors` lists the indices to describe; unset describes every sector.
  The list is deduplicated and read ascending; an index at or above the
  sector count is `not_found`, and the message names the valid `0..N`
  range.
- `offset` and `limit` page the (possibly filtered) row list: `limit`
  defaults to 150, accepts `1..=500` (anything else is `invalid_argument`
  naming the range), and `next_offset` names the `offset` where the rows
  left behind continue - `null` when the page took everything. An offset
  past the end is not an error: the rows come back empty.
- `render` defaults to **`true`** and publishes the labelled top-down
  PNG - sector fills, sector numbers and things drawn, the same picture
  `check_playability`'s `render` draws without highlights - in `image`; a
  render failure fails the call. `render: false` answers `image: null`
  and publishes nothing.

### The `sectors` rows

Rows come in ascending index order, each
`{sector, area, bbox, center, floor, ceiling, height, light, floor_flat,
ceiling_flat, sky, special, tag, roles, wall_textures, neighbours,
things}`: `area` is the even-odd floor in square map units, `bbox` the
`[min_x, min_y, max_x, max_y]` box and `center` a label point the even-odd
rule places inside; `special` is `{value, name}` through the session's
definitions or `null`; `wall_textures` tallies each texture name by sides
worn; `things` counts the sector's things by definition name, most first;
and `neighbours` is one row per sector across the two-sided boundary -
`{sector, via, lines, floor_delta}` with `floor_delta` the neighbour's
floor over this sector's own.

The `roles` vocabulary - what a sector *is*:

| role | reads as |
|------|----------|
| `door` | a door action raises its ceiling, or it is a shut band at most 8 units tall with two distinct neighbours |
| `lift` | a lift action drops it |
| `teleport_destination` | a teleport action lands a player here |
| `secret` / `damaging` / `light_effect` | the family's sector-special read (vanilla numbers, `Boom` bits, the `Hexen` family's own) |
| `exit_room` | a level-end line touches it on either side, or a walk-over end crosses into it |
| `stair_step` | a footprint of at most 64x256 square units with a neighbour 1 to 24 units up and another the same span down |
| `sky` | an `F_SKY1` ceiling |

The neighbour `via` vocabulary - how a player gets through: `door` or
`lift` when the neighbour sector wears that role; `window` when every
shared line blocks yet leaves at least 16 units of light; `impassable`
for any other boundary with a blocking line; otherwise `opening`,
`step_up` or `step_down` by `floor_delta`.

### The `summary` object

`{format, bounds, counts, player_starts, exits, keys, locked_doors,
teleporters, secret_sectors, monsters, items, top_wall_textures,
top_flats, largest_sectors, playable}` - computed over the whole map
whatever the `sectors` filter and the paging say. `counts` is
`{things, vertices, linedefs, sidedefs, sectors}`; `exits`, `keys`,
`locked_doors` and `teleporters` are the `Skill::Medium` walk's rows -
the same reading `check_playability` answers with, `keys` also naming
the sector each key stands in; `monsters` and `items` tally things by
definition name;
`top_wall_textures`/`top_flats` are the ten busiest names as
`[name, count]` pairs; `largest_sectors` ranks the five biggest by
`area`; `playable` is `{ok, summary}` in `check_playability`'s own words.

### Answering a plain question

"Which room has the shotgun?" - read the rows: match the `things` counts
against the item's definition name (or `wall_textures` and `roles` for a
look), take that row's `sector`, and confirm on the `image` - its labels
are the same numbers. Then act: `edit_sector` with that index, `add_door`
between it and the neighbour the row names, `place_things` into its
`center`. A large map arrives in pages; follow `next_offset` with
`offset` until it reads `null`, or filter with `sectors` once you know
which rows you want.

