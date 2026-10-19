# Convert tools

Rewriting one map block in another map format: `convert_map_format`.

Formats come in two **flag families**:

| Family | Layouts |
|--------|---------|
| `doom` | binary `Doom`, and `UDMF` namespaces `doom`, `heretic`, `strife` |
| `hexen` | binary `Hexen`, and `UDMF` namespaces `hexen`, `zdoom`, `gzdoom`, `eternity`, … |

Inside one family every `flags` bit keeps its meaning and the step is
mechanical. Across the families the conversion **translates best-effort**
through the embedded `ZDoom` linedef table, in either direction: pass a
target of the other family (a binary format, or a `UDMF` namespace of the
other family) and the specials, thing flags and sector types are rewritten.
The losses below are documented per direction.

## `convert_map_format`

`{map?, to, namespace?, dry_run? = false}` → `{map, from, to, namespace,
translated_specials, untranslated [{linedef, special}],
thing_flags_changed, sector_types_changed, dropped_fields {things,
linedefs, sidedefs, sectors, vertices}, fractional_coordinates {same keys},
warnings, nodes_removed, dry_run}`

- `map` defaults to the active map (`set_active_map`, else the merged
  view's only map).
- `to` is `"doom"`, `"hexen"` or `"udmf"`, any case.
- `namespace` (1 to 32 `ASCII` letters, digits or underscores, lowercased)
  is the namespace the `UDMF` `TEXTMAP` declares. `UDMF` only — with a
  binary `to` it is `invalid_argument`. Its default follows the **source**:
  `"doom"` for a `Doom` family map, `"zdoom"` for a `Hexen` family one.
  Naming a namespace of the other family is the cross-family request; the
  spelling is best-effort (e.g. a `Hexen` map converted to `heretic`).
- `dry_run: true` converts a copy and reports, touching neither the working
  archive nor the undo stack. It still checks that the result writes, so a
  `limit_exceeded` shows up here too.
- A request naming the layout the map is already in (same format and
  namespace, compared case-insensitively) is `invalid_argument`.

One successful call is **one undo step**, and a map the `IWAD` or a resource
layer owns is copied into the working archive by that same step
(`reference/maps.md`, `reference/session.md`).

## What carries over

- **Binary `Doom` ↔ `UDMF`** (`doom`/`heretic`/`strife`): lossless. The
  line `tag` stays the line `tag`; `UDMF` stores it as both `id` and `arg0`
  and folds them back on read.
- **Binary `Hexen` → `UDMF`**: lossless except `Line_SetIdentification`
  (`special` 121), which `UDMF` has no room for: each such line's `args[0]`
  becomes the line id and the special clears. `BEHAVIOR` and `SCRIPTS` ride
  along in the block.
- **`UDMF` → binary `Hexen`**: the reverse. A line with a non-zero id and no
  special becomes `Line_SetIdentification` carrying the id in `args[0]`. An
  id that **cannot** be stored drops with a warning naming the line: one
  above `255` (a `Hexen` arg is one byte), or on a line that already has a
  special. The `Hexen` writer synthesizes the minimal empty `BEHAVIOR` the
  format needs when the block carries none.
- **`UDMF` → any binary format**: a binary record has no room for what the
  model does not name, so every unmodelled field goes, counted per element
  kind in `dropped_fields`. `fractional_coordinates` counts, per kind, how
  many elements had a fractional number that was **already** rounded when
  the `UDMF` was read — the conversion loses nothing more, but it is worth
  knowing. `UDMF` thing `height` (`z`), `id` (`tid`) and line args beyond the
  target's fields have nowhere to go in binary `Doom`, and are simply not
  written.
- **`UDMF` → `UDMF`** inside one family rewrites the `namespace` declaration
  only; every element and every field stays.
- Values outside the target's ranges (a `Doom` `i16`/`u16` field, a `Hexen`
  byte `special` or `arg`) fail `limit_exceeded` naming the element and
  field, and **nothing changes**.
- A `BEHAVIOR` lump cannot stay in a block converted to binary `Doom` — that
  lump is what makes a block read as `Hexen` — so it drops with a warning.
  Convert to `UDMF` to keep the script.

## Across the families

The table stages run first, then exactly the mechanical steps above for the
target layout. `translated_specials` counts the lines that moved through
the table; every other cross-family effect is in `untranslated`,
`thing_flags_changed`, `sector_types_changed` and `warnings`. `untranslated`
lists the affected `linedef` index and old `special` of the first 100 lost
lines; a warning counts any beyond them.

**`Doom` family → `Hexen` family** (to `hexen`, or to a `UDMF` namespace of
the `Hexen` family):

- Each special the table covers becomes its generalized action: the row's
  argument template is written into the line's `args` with the trigger
  `tag` in the row's `tag` slot (the tag field then empties), and the row's
  activation becomes the `Hexen` activation field plus the `repeatspecial`
  and `monsteractivate` bits. A row **without** a `tag` slot leaves the
  trigger tag in the tag field, where a `UDMF` target carries it as the
  line id; a binary `Hexen` target has no room for an id next to a special,
  so there the tag drops with a warning naming the line.
- A special the table does not cover becomes `special` 0, is listed in
  `untranslated` and is counted in one grouped warning naming the lost
  lines. Its old tag rides on as a line id (binary `Hexen` stores that in
  `Line_SetIdentification`).
- For a binary `Hexen` target, a row whose action number, one of whose
  arguments or whose `tag` slot value is above `255` is refused the same
  way — `special` 0 and one `untranslated` entry — but the line gets its
  **own** warning naming the action it would have run and the field too
  wide for one byte. Convert those lines to a `Hexen`-family `UDMF`
  namespace instead, which stores them.
- `Doom`'s `passuse` bit has no `Hexen` bit (512 is `repeatspecial`
  there): it moves to the line's `extra` fields under the `passuse` key. A
  `UDMF` target keeps the key; a binary target drops it, counted in
  `dropped_fields`. Nothing restores the bit, so run `edit_linedef` on the
  result if the line needs it.
- Thing presence bits invert to their positive `Hexen` twins (all three
  class bits are set); the `tid`, `special` and `args` fields the `Doom`
  family never fills stay zero. Boom-style sector specials (bits 0-9)
  move behind the `Hexen` generalized base, except the secret base type
  `9`, which arrives as the `Hexen` secret bit; a value outside the
  Boom-style range clears to 0 with a grouped warning.

**`Hexen` family → `Doom` family** (to `doom`, or to a `UDMF` namespace of
the `Doom` family):

- Each line is matched against the table's rows: the activation field, the
  repeat and monster bits and every argument outside the row's `tag` slot
  must agree, and the slot value becomes the restored `Doom` trigger `tag`
  (a row without a `tag` slot gives tag 0). When several rows match, the
  **lowest `Doom` special wins**.
- A special no row matches — unknown, negative, or an activation field no
  row uses — becomes `special` 0, is listed in `untranslated` with a grouped
  warning, and keeps its tag as the `Doom` trigger tag. A `Line_SetIdentification`
  from binary `Hexen` folds to its id first, so `121 id 7` comes back as
  `special` 0 `tag` 7; a literal special 121 from a `UDMF` source has no
  table row at all and lands on the same unmatched path, id kept as the tag.
- A line **id** becomes a `Doom` **trigger tag**: a plain line's id stands,
  and a line whose special matched a row loses its id (the `Doom` tag field
  holds the trigger tag) with a warning naming the line. Restored tags are
  range-checked only by the target's writer: an id beyond a binary `Doom`
  `u16` tag surfaces as `limit_exceeded` — use `dry_run` to check.
- `Hexen`-only thing bits (`dormant`, `translucent`, `invisible`,
  `standing`) and line bits (`blockplayers`, `blockeverything`) have no
  `Doom` equivalent and drop, each named in one grouped warning. Thing
  `tid`, `special` and `args` zero. `Hexen`-native sector types (not the
  translated Boom range) clear to 0 with a grouped warning.

## Node data always goes

`SEGS`, `SSECTORS`, `NODES`, `REJECT`, `BLOCKMAP`, `ZNODES` and the block's
`GL` run are written in the map's own format, so **every** conversion removes
them: the result says `nodes_removed: true` and its `warnings` tell you to
run `build_nodes` before playing the map in vanilla `Doom`. `ZDoom`-family
ports rebuild missing node data themselves. Check the removal with
`get_map_lumps` (`reference/maps.md`) or `get_nodes` (`reference/nodes.md`).

## Examples

```json
call convert_map_format {"map": "MAP01", "to": "udmf"}
-> {"map": "MAP01", "from": "doom", "to": "udmf", "namespace": "doom",
    "translated_specials": 0, "untranslated": [], "thing_flags_changed": 0,
    "sector_types_changed": 0, "dropped_fields": {"things": 0,
    "linedefs": 0, "sidedefs": 0, "sectors": 0, "vertices": 0},
    "fractional_coordinates": {"things": 0, "linedefs": 0, "sidedefs": 0,
     "sectors": 0, "vertices": 0},
    "warnings": ["the node data went with the old format (…): run
     build_nodes before playing the map in vanilla Doom"],
    "nodes_removed": true, "dry_run": false}

call convert_map_format {"map": "MAP01", "to": "udmf", "namespace": "zdoom"}
-> {..., "translated_specials": 1, "untranslated": [],
    "thing_flags_changed": 2, ...}
   (with the step line set to special 1 by an earlier edit_linedef, the
    door line became special 12 Door_Raise with the row's args, and the
    two things moved to Hexen presence bits)

call convert_map_format {"map": "MAP01", "to": "doom"}
-> {..., "from": "udmf", "to": "doom", "translated_specials": 1, ...}
   (forward then back: special 1 is again the line's special, and its
    trigger tag is restored to the tag field)

call convert_map_format {"map": "MAP03", "to": "udmf", "namespace": "zdoom"}
-> error invalid_argument "MAP03 already is a udmf (zdoom) map: nothing to
   convert; pass a different to (and, for UDMF, namespace) pair"
```

Verify a conversion with `get_map_summary` (its `counts` must be unchanged),
`assemble_map` (the specials and tags, `reference/maps.md`) and
`render_topdown`, then rebuild the nodes. Use `get_linedef_specials` with
`family: "hexen"` (`reference/defs.md`) to read what a generalized action
number means.
