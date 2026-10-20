# Query tools

Sets, not scans. Plain-English requests are usually about *collections* of
elements — "every room with a nukage floor", "all STARTAN walls", "the
monsters in the hall" — and `find_elements` turns such a request into a
query: a kind, a list of filters and an optional bound, answered with the
matching indices and compact rows. It never changes anything; act on the
indices with the edit tools (and, from CD-108 on, with bulk-edit tools that
take the same query).

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

The `rows` are deliberately compact summaries (sector look, line ends and
sides, side textures, thing names and positions); for full detail on a hit,
`get_map_summary`/`describe_map`/`render_topdown` read the same indices.
