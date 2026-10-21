# Query tools

Sets, not scans. Plain-English requests are usually about *collections* of
elements — "every room with a nukage floor", "all STARTAN walls", "the
monsters in the hall" — and `find_elements` turns such a request into a
query: a kind, a list of filters and an optional bound, answered with the
matching indices and compact rows. It never changes anything; act on the
indices with the edit tools — or, when the request is a whole swap ("use
BRICK7 instead of STARTAN everywhere", "turn every imp into a demon"),
with this group's bulk replacements `replace_texture`, `replace_flat` and
`replace_thing_type`, which fold the same question across maps into one
undoable step.

## `find_elements`

`{kind, where? = [], within?, map?, offset? = 0, limit? = 200}` →
`{kind, count, indices, rows, next_offset}`

- `kind` is `"sectors"`, `"linedefs"`, `"sidedefs"`, `"things"` or
  `"vertices"` — the block the answer indexes into.
- `where` is a list of `{"field": ..., "op": ..., "value": ...}` filters,
  combined with **AND**: an element matches when it passes every one of
  them. An empty (or absent) list keeps everything the bound allows.
- `within` bounds the hits (see Bounds).
- `map` and the merged scope work as in the other map tools
  (`reference/maps.md`): the active map, else the merged view's only map.
  An unknown name is `not_found`.
- `offset` and `limit` page the ascending hits: `limit` defaults to 200,
  accepts `1..=1000` (anything else is `invalid_argument` naming the
  range), and `next_offset` names the `offset` where the hits left behind
  continue — `null` when the page took everything. An offset past the end
  is not an error: the page comes back empty.
- `count` totals **every** hit, whatever the paging says.
- A bad field, op or value is `invalid_argument`, and the message lists
  the valid options for that kind — read it and re-issue.

## Ops

| op | works on | value |
|----|----------|-------|
| `eq` / `ne` | numbers, text, `two_sided` | one value of the field's shape |
| `lt` / `le` / `gt` / `ge` | numbers | one integer |
| `in` / `not_in` | numbers, text, `two_sided` | an array of such values |
| `glob` | text | a case-insensitive pattern: `*` any run, `?` one character |
| `has` / `has_not` | `flags`, `roles` | one flag or role name |

Numbers compare as integers (no floats on the wire); text compares exactly
— names and flats are stored uppercase, so use `glob` when case may differ.
A numeric field compared with a string, a `flags` field compared with
anything but `has`/`has_not`, an unknown flag or role name: all are
`invalid_argument` listing what is valid.

## Fields per kind

Derived fields are marked `*`; when one does not resolve (the back side of
a one-sided line, a thing in the void) it matches **nothing**, `ne` and
`has_not` included.

### `sectors`

| field | reads |
|-------|-------|
| `index` | position in `SECTORS` |
| `floor` / `ceiling` | heights in map units |
| `height*` | `ceiling - floor` |
| `light` | light level |
| `floor_flat` / `ceiling_flat` | flat names |
| `special` / `tag` | stored numbers |
| `area*` | even-odd floor in square map units (as `describe_map`) |
| `roles*` | what the sector *is* (see `reference/playability.md`) |

### `linedefs`

| field | reads |
|-------|-------|
| `index`, `v1`, `v2` | position and endpoint vertex indices |
| `flags` | with `has`, a name from the map's family table: `blocking`, `blockmonsters`, `twosided`, `secret`, `mapped`, ... and in the `Hexen` family also `playercross`, `playeruse`, `repeatspecial`, ... |
| `special` | line action number |
| `tag` (`Doom` family) / `arg0..arg4` (`Hexen` family) | trigger tag / script arguments |
| `front_sector*` / `back_sector*` | sectors the resolvable sides open into |
| `two_sided*` | whether the line carries both sides |
| `length*` | integer hypot of the endpoints, map units |
| `front_upper*`, `front_middle*`, `front_lower*`, `back_upper*`, `back_middle*`, `back_lower*` | the six faces' texture names, read through the sidedefs; a missing side has no faces |

### `sidedefs`

`index`, `sector`, `upper`, `middle`, `lower`, `x_offset`, `y_offset`,
and `linedef*` — the first line wearing this side.

### `things`

| field | reads |
|-------|-------|
| `index`, `type`, `x`, `y`, `angle` | stored numbers |
| `flags` | `has` a name from the family's thing table: `easy`, `medium`, `hard`, `ambush`, `notsingle`, ... |
| `name*` / `category*` | the definition's display name and category through the session's definitions (custom `DEHACKED`/`DECORATE` included); an undefined type reads `Unknown (n)` / `Unknown` |
| `sector*` | the sector the thing stands in |
| `tid`, `special` | the `Hexen`-family script fields |

### `vertices`

`index`, `x`, `y`.

## Bounds

`within` is either positional or membership-shaped, per kind:

- `{"bbox": [min_x, min_y, max_x, max_y]}` (inclusive) keeps things and
  vertices at points inside the box, sectors whose label **centre** lies
  inside it, linedefs with **both** vertices inside it, and sidedefs whose
  owning line meets that rule.
- `{"sectors": [i, ...]}` keeps sectors by index, things and vertices
  standing in one of them, linedefs with a resolvable side in one of them
  and sidedefs opening into one of them. An index past the sector block
  matches nothing.

## `replace_texture`

`{from, to, parts?, maps?, dry_run? = false}` → `{maps: {<block>: n},
total, copied_maps, dry_run, next_steps}`

Rewrites every sidedef face whose texture the case-insensitive `*`/`?`
glob `from` matches to `to`, across maps, in **one** undoable step
labelled `replace_texture <FROM> -> <TO>`. A face that already holds
`to` stays out of the counts, so the numbers report what changed.

- `parts` names the faces to touch — any of `"upper"`, `"middle"`,
  `"lower"` (case-insensitive); absent means all three. A stray name
  is `invalid_argument` listing the three.
- `to` is uppercased, validated as a texture name, and must exist in
  the merged view: `list_textures` shows the composites and the import
  tools add more. An unknown `to` is `not_found`. `"-"` is accepted
  without an existence check and *clears* the matched faces.
- `maps` selects the blocks: absent takes the session's default map
  (the active map, else the merged view's only map), `"all"` every
  distinct block of the merged view, and a list of names exactly those
  (an unknown name is `not_found`).
- **Copy-on-write**: a map a read-only layer (the `IWAD` or a merged
  resource) holds is copied into the working archive before it is
  rewritten; `copied_maps` names those copies. The copies, every
  rewrite and the label sit in one undo entry — a single `undo` takes
  the whole swap back across every map — and any refusal leaves the
  session exactly as it was.
- `dry_run: true` reports the same `{maps, total}` numbers while
  writing nothing and recording nothing. Run it first for big or
  wildcard swaps (`from: "*"`) to measure the blast radius.
- Every selected map holds a `maps` entry even at `0`, and `total`
  sums the values.

"use BRICK7 instead of STARTAN everywhere" → `list_textures` to confirm
BRICK7 exists, a `dry_run` pass of
`replace_texture {from: "STARTAN", to: "BRICK7", maps: "all"}` to see
how many faces it moves, then the same call with `dry_run` dropped.
`find_elements {kind: "sidedefs", where: [{field: "middle", op: "eq",
value: "STARTAN"}]}` shows the faces one by one, before and after.

## `replace_flat`

`{from, to, surfaces?, maps?, dry_run? = false}` → the same report
shape, one undoable step labelled `replace_flat <FROM> -> <TO>`.

- Rewrites the sector *surfaces* the glob `from` matches; `surfaces`
  names any of `"floor"` and `"ceiling"` (case-insensitive; absent
  means both). A stray name is `invalid_argument` listing the two.
- `to` must exist as a flat in the merged view — `list_flats` shows
  them and `F_SKY1` is always available. There is no clearing `"-"`
  here: a flat field always names a flat, so `"-"` takes the ordinary
  unknown-name `not_found`.
- Heights, light levels, specials and tags come out byte-identical.
  The `maps` selector, copy-on-write `copied_maps`, `dry_run` and the
  one-undo guarantee are exactly as in `replace_texture`.

Find the wearers first with `find_elements {kind: "sectors", where:
[{field: "floor_flat", op: "glob", value: "FLOOR*"}]}` and read the
result back through `describe_map`'s per-sector flat fields.

## `replace_thing_type`

`{from, to, maps?, dry_run? = false, allow_unknown? = false}` →
`{maps: {<block>: n}, total, copied_maps, from, to, dry_run,
next_steps}`

Rewrites every thing whose type number equals the resolved `from` to
the resolved `to`, across maps, in **one** undoable step labelled
`replace_thing_type <FROM> -> <TO>` (the resolved numbers). Only the
type moves: every touched thing's position, facing, presence flags and
the `Hexen`-family `tid`, `special` and args come out byte-identical,
and things that already hold `to` stay out of the counts.

- **Both sides take a number or a name.** A JSON number is the doomednum
  as stored; a string resolves through the session's definitions
  (`get_thing_types` shows them, custom `DEHACKED`/`DECORATE` included):
  an exact case-insensitive name wins, else a unique case-insensitive
  substring — `from: "imp"` means `3001`, and `to: "demon"` means `3002`
  *even though* `Demon` is also a substring of `CyberDemon` (16) and
  `Cacodemon` (3005). A name nothing holds, or one several distinct
  types share, is `invalid_argument` listing the candidates and naming
  which parameter (`from` or `to`) failed.
- **`to` must exist** in the session's definitions; an undefined number
  is `invalid_argument` naming `get_thing_types` and the escape hatch:
  `allow_unknown: true` skips the check and the report carries the
  written number as-is. An undefined `from` needs no escape — it simply
  matches nothing. An `allow_unknown` number must still fit each
  targeted map's format (binary `Doom` stores thing types as `u16`; a
  larger one is `limit_exceeded` and nothing is written).
- The `maps` selector, copy-on-write `copied_maps`, `dry_run` and the
  one-undo guarantee are exactly as in `replace_texture`; every selected
  map holds a `maps` entry even at `0`, counting the *things* moved.

"Turn all imps into demons" → `replace_thing_type {from: "imp",
to: "demon"}`; run it with `dry_run: true` first to count the imps, and
read the result back with `find_elements {kind: "things", where:
[{field: "type", op: "eq", value: 3002}]}`.

## Typical requests

- "Make every dark room dimmer" — `find_elements {kind: "sectors", where:
  [{field: "light", op: "lt", value: 96}]}`, then `edit_sector` per index.
- "Every wall wearing STARTAN" — `{kind: "sidedefs", where: [{field:
  "middle", op: "eq", value: "STARTAN"}]}` (or `op: "glob"` with
  `"startan*"` when case may differ).
- "The monsters in sector 4" — `{kind: "things", where: [{field:
  "category", op: "eq", value: "Monsters"}], within: {sectors: [4]}}`.
- "Rooms with a nukage floor" — `{kind: "sectors", where: [{field:
  "floor_flat", op: "glob", value: "NUKAGE*"}]}`, or the exact flat name
  once you know it; `describe_map`'s `top_flats` lists what a map uses.
- "Every line that runs a teleport action" — `{kind: "linedefs", where:
  [{field: "special", op: "in", value: [1, 2]}]}` in the `Doom` family.
- "The things of a kind, then their rooms" — read the `sector` off the
  `rows` of a `{kind: "things", where: [{field: "name", op: "glob",
  value: "imp*"}]}` query; skip the second round trip entirely with
  `within: {sectors: [...]}` when the rooms are already known.
- "Use BRICK7 instead of STARTAN everywhere" — skip the per-element
  walk: `replace_texture {from: "STARTAN", to: "BRICK7", maps: "all"}`
  (with a `dry_run: true` pass first for a swap this wide).
- "Every nukage floor in the episode goes back to concrete" —
  `replace_flat {from: "NUKAGE*", to: "FLOOR4_6", maps: "all"}`;
  `surfaces: ["floor"]` keeps any sector's sky or liquid ceiling as it
  is.
- "Turn all imps into demons" — skip the per-thing walk:
  `replace_thing_type {from: "imp", to: "demon"}` (names or doomednums
  both sides; `maps: "all"`, or a `dry_run: true` pass first when the
  swap runs across the episode).

The `rows` are deliberately compact summaries (sector look, line ends and
sides, side textures, thing names and positions); for full detail on a hit,
`get_map_summary`/`describe_map`/`render_topdown` read the same indices.
