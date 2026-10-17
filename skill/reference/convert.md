# Convert tools

Rewriting one map block in another map format: `convert_map_format`.

Formats come in two **flag families**, and a conversion never leaves its
own — that keeps every `flags` bit, and no linedef or thing special needs
translating:

| Family | Layouts |
|--------|---------|
| `doom` | binary `Doom`, and `UDMF` namespaces `doom`, `heretic`, `strife` |
| `hexen` | binary `Hexen`, and `UDMF` namespaces `hexen`, `zdoom`, `gzdoom`, `eternity`, … |

Converting **between** families is not implemented yet: it needs the
special translation tables, so it fails `unsupported` and says so. Pick a
target of the map's own family.

## `convert_map_format`

`{map?, to, namespace?, dry_run? = false}` → `{map, from, to, namespace,
dropped_fields {things, linedefs, sidedefs, sectors, vertices},
fractional_coordinates {same keys}, warnings, nodes_removed, dry_run}`

- `map` defaults to the active map (`set_active_map`, else the merged
  view's only map).
- `to` is `"doom"`, `"hexen"` or `"udmf"`, any case.
- `namespace` (1 to 32 `ASCII` letters, digits or underscores, lowercased)
  is the namespace the `UDMF` `TEXTMAP` declares. `UDMF` only — with a
  binary `to` it is `invalid_argument`. Its default follows the **source**:
  `"doom"` for a `Doom` family map, `"zdoom"` for a `Hexen` family one.
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
    "dropped_fields": {"things": 0, "linedefs": 0, "sidedefs": 0,
     "sectors": 0, "vertices": 0}, "fractional_coordinates": {"things": 0,
     "linedefs": 0, "sidedefs": 0, "sectors": 0, "vertices": 0},
    "warnings": ["the node data went with the old format (…): run
     build_nodes before playing the map in vanilla Doom"],
    "nodes_removed": true, "dry_run": false}

call convert_map_format {"map": "E1M1", "to": "hexen"}
-> error unsupported "cross-family conversion needs special translation;
   see part 2: this map stores doom family flags and the target stores
   hexen family flags, so convert to a layout of the doom family for now"

call convert_map_format {"map": "MAP03", "to": "udmf", "namespace": "zdoom"}
-> error invalid_argument "MAP03 already is a udmf (zdoom) map: nothing to
   convert; pass a different to (and, for UDMF, namespace) pair"
```

Verify a conversion with `get_map_summary` (its `counts` must be unchanged)
and `assemble_map` (`reference/maps.md`), then rebuild the nodes.
