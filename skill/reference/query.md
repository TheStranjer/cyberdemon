# Query tools

Sets, not scans. Plain-English requests are usually about *collections* of
elements — "every room with a nukage floor", "all STARTAN walls", "the
monsters in the hall" — and `find_elements` turns such a request into a
query: a kind, a list of filters and an optional bound, answered with the
matching indices and compact rows. It never changes anything; act on the
indices with the edit tools — with `bulk_edit` when the request is a value
change over a whole set ("make every nukage room darker", "block every
line in sector 3") — and, when it is a whole name swap ("use BRICK7
instead of STARTAN everywhere", "turn every imp into a demon"), with this
group's bulk replacements `replace_texture`, `replace_flat` and
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
| `flags` | with `has`, a name from the map's family table: `blocking`, `blockmonsters`, `twosided`, `secret`, `mapped`, ... in the `Doom` family also `passuse`, `blocklandmonsters`, `blockplayers`, and in the `Hexen` family also `playercross`, `playeruse`, `repeatspecial`, ... |
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

## `bulk_edit`

`{map?, kind, where? = [], within?, set?, add?, flags_set?, flags_clear?,
allow_missing? = false, dry_run? = false, max? = 5000}` →
`{matched, changed, indices, truncated, dry_run, preview, map,
nodes_invalidated, copied_to_working, next_steps}`

The general field editor: selects elements with exactly the
`find_elements` query (`kind`, `where`, `within`) and applies one set of
field changes to **every** matched element as ONE undoable step labelled
`bulk_edit <kind> (<matched>)`. Use it for "all the X" requests that are
*changes of value* ("make every nukage room darker", "block every line in
sector 3"); pure name swaps have their own tools above.

| parameter | meaning |
|-----------|---------|
| `map` | the map block, as in `find_elements` |
| `kind` | `"sectors"`, `"linedefs"`, `"sidedefs"`, `"things"` or `"vertices"` |
| `where` / `within` | the selection, exactly as in `find_elements` |
| `set` | field → new value (integers; texture/flat names; thing `type` also takes a definition name) |
| `add` | field → integer amount shifted onto each element's **current** value |
| `flags_set` / `flags_clear` | flag names for linedefs and things (below) |
| `allow_missing` | write texture/flat names the merged view lacks (default `false`) |
| `dry_run` | check everything and report over a copy: writes nothing, records no undo step |
| `max` | refuse (and write nothing) when more elements match than this cap (default 5000) |

### Settable and addable fields

| kind | settable | `add`able |
|------|----------|-----------|
| sectors | `floor`, `ceiling`, `light`, `floor_flat`, `ceiling_flat`, `special`, `tag` | `floor`, `ceiling`, `light`, `tag` |
| sidedefs | `upper`, `middle`, `lower`, `x_offset`, `y_offset` | `x_offset`, `y_offset` |
| vertices | (none — a `set` on vertices is `invalid_argument`) | `x`, `y` |
| linedefs | `special` and `tag` (`Doom` family); `special` and `arg0..arg4` (`Hexen` family) | (none) |
| things | `type`, `angle`, `x`, `y` | `angle`, `x`, `y` |

- Vertices are **moved, never set**: put their values in `add`.
- Linedef fields are **family-gated**: naming `tag` on a `Hexen`-family
  map or `arg0..arg4` on a `Doom`-family one is `invalid_argument` naming
  the map's family and `convert_map_format`; linedef numbers never take
  `add` amounts.
- A thing `type` takes a doomednum integer **or** a definition name
  (resolved through the session's definitions like `replace_thing_type`'s
  sides; an unknown or ambiguous name is `invalid_argument` listing the
  candidates).

### Flags

`flags_set` and `flags_clear` carry names from the map's family tables
and apply to linedefs and things only — on sectors, sidedefs and vertices
they are `invalid_argument`. The result is the current bits with the
clear names off and the set names on: **a bit named on both ends ends up
set**. `Doom`-family linedefs: `blocking`, `blockmonsters`, `twosided`,
`dontpegtop`, `dontpegbottom`, `secret`, `blocksound`, `dontdraw`,
`mapped`, `passuse` and `MBF21`'s `blocklandmonsters` (4096) and
`blockplayers` (8192). `Hexen`-family linedefs keep the shared bits, add
`repeatspecial`, `monsteractivate`, `blockplayers` (16384), `blockeverything` and the six activation names
(`playercross`, `playeruse`, `monstercross`,
`impact`, `playerpush`, `missilecross`), and carry no `passuse`.
`Doom`-family things: `easy`, `medium`, `hard`, `ambush`, `notsingle`,
`notdm`, `notcoop`, `friend`; `Hexen`-family things: `easy`, `medium`,
`hard`, `ambush`, `dormant`, `class1`, `class2`, `class3`, `single`,
`coop`, `dm`, `translucent`, `invisible`, `friend`, `standing`. An
unknown name is `invalid_argument` listing the table. The `preview`
reports the raw bitset under `flags`.

### Never clamped, never half-applied

Validate all matched elements first — **values are never clamped**.
Every resulting value is range-checked before anything is written, and
one bad value refuses the WHOLE call with the map untouched: `light`
stays in `0..=255` and thing `angle` in `0..=359` (no normalising) in
every format; binary maps cap the rest at their record columns (`i16`
heights, offsets and thing coordinates, `u16` thing types and
`Doom`-family linedef `special`/`tag`, `u8` `Hexen`-family linedef
`special`/args); `UDMF` is unbounded beyond the model's `i32` columns; a
sector's ceiling must end at or above its floor. Texture and flat names
must exist in the merged view (`list_textures`/`list_flats`) — `not_found`
— unless `allow_missing: true` writes them anyway; `"-"` always clears a
sidedef face. `max` (default 5000) caps the matched set before anything
runs: a query matching more fails `limit_exceeded` and writes nothing
(`max: 0` with any match fails); narrow the `where`/`within` or raise
`max`.

### Preview, result, undo

`dry_run: true` runs every check — query, `max`, all the ranges — over a
copy of the map and returns the same numbers beside a `preview` of up to
20 `{index, before: {field: old}, after: {field: new}}` rows, writing
nothing and recording no undo step. **Run it first before large or broad
edits.** A written step copies a layer-only map into the working archive
inside the step (`copied_to_working`) and reports `nodes_invalidated`:
true only for vertex moves (geometry — run `build_nodes` before vanilla
play), never for thing, sector, sidedef or linedef field edits. One
`undo` takes every changed value on every matched element back as one
step. `indices` lists the changed elements (ascending, capped at 500 with
`truncated: true` when capped).

### English to call

- "Make all the nukage rooms darker" — `bulk_edit {kind: "sectors",
  where: [{field: "floor_flat", op: "glob", value: "NUKAGE*"}], add:
  {light: -32}}`; `dry_run` it first when the count is unknown.
- "Make the whole map darker" — `bulk_edit {kind: "sectors", add:
  {light: -32}}`; every row that would end below 0 or above 255 refuses
  the whole call, so read the lights with `find_elements` first.
- "Turn all imps into demons" — `bulk_edit {kind: "things", where:
  [{field: "name", op: "glob", value: "imp*"}], set: {type: "demon"}}`;
  a pure type swap can also go through `replace_thing_type`, which sweeps
  many maps in one step.
- "Block every line in sector 3" — `bulk_edit {kind: "linedefs", within:
  {sectors: [3]}, flags_set: ["blocking"]}`.
- "Slide the whole bridge 8 units east" — bound the bridge's vertices
  (`find_elements {kind: "vertices", within: {bbox: [...]}}`), then
  `bulk_edit {kind: "vertices", within: {bbox: [...]}, add: {x: 8}}` —
  geometry, so `build_nodes` after it.

## Typical requests

- "Make every dark room dimmer" — `bulk_edit {kind: "sectors", where:
  [{field: "light", op: "lt", value: 96}], add: {light: -32}}` (read the
  lights with `find_elements` first; the `add` refuses any row that would
  leave `0..=255`), or the per-index walk with `edit_sector` when only
  some rooms should change.
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
