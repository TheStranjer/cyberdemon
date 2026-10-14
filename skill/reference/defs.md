# Definitions tools

Reference tables that describe what the game formats mean: thing types,
linedef specials and sector types, plus Boom's bit-packed generalized
decodings. The tables are embedded in the server binary, so these tools
need no archives loaded.

## What ships

DOOM, DOOM II and Heretic ship today. The Doom side has 123 thing types
and 100 + 39 linedef specials (SLADE-style categories) and the 16 named
Doom sector types; each row's `game` names the first game that has it.
Heretic has its own three tables: 84 thing types, 105 linedef specials and
47 sector types, all plain values — Heretic is a Doom-engine game but not
Boom-compatible, so its specials and sector types are never generalized or
bit-decoded. Hexen tables are planned: that game currently answers with
empty lists and a "not yet available" warning. Strife and an unidentified
session fall back to the DOOM II rows, with a warning saying so.

All three tools share the `game` argument: `"doom"`, `"doom2"`,
`"heretic"` or `"hexen"`, defaulting to the game detected from the
session (see `reference/session.md`), with DOOM II standing for an
unidentified session. Any other value is refused with `invalid_argument`
listing the accepted names. A broken embedded table is a server bug and
reports `internal`.

## `get_thing_types`

- Purpose: list the thing types an editor can offer for a game — type
  numbers, display names, sprite and default frame, blocking radius,
  category path and the hanging flag.
- Arguments (all optional):
  - `game` — see above.
  - `category` — keep rows whose category starts with this text,
    matched case-insensitively (`"Monsters"`, or `"Gore/"` for every
    gore category).
  - `search` — keep rows whose name contains this text, matched
    case-insensitively.
  - `type` — keep exactly this type number. An unknown number returns an
    empty list, not an error.
- Returns:

  ```json
  {
    "game": "doom2",
    "count": 123,
    "things": [
      {"type": 5, "name": "Blue Keycard", "sprite": "BKEY",
       "frame": "A", "radius": 12, "category": "Keys",
       "hanging": false}
    ],
    "warnings": []
  }
  ```

  `count` is the number of things after filtering and `things` is sorted
  by `type` ascending. `warnings` explains an empty table or a fallback
  game; it is empty for DOOM, DOOM II and Heretic.
- Typical use: looking up spawn numbers when writing `THINGS`, and
  translating a type number found in a map (`{ "type": 3001 }` gives the
  Imp; pair it with the map's game when the row matters).

## `get_linedef_specials`

- Purpose: list the special-table linedef actions a game knows, and
  decode any Boom generalized number.
- Trigger prefix legend (both table names and the decoder use it):
  `W` = walk over, `S` = switch/use, `G` = gun shot, `D` = use on the
  line itself, `M` = monster-activated; `1` = trigger once, `R` =
  repeatable. So `SR Floor...` fires when used from either side and
  re-arms.
- Row fields: `target` is `"sector"` (acts on sectors tagged with the
  line's tag), `"sector_back"` (acts on the sector behind the line, no
  tag needed) or `"-"` (nothing tagged, e.g. exits); `args` is empty for
  the Doom-family tables (it exists for the Hexen-style arg columns that
  a later ticket adds).
- Arguments (all optional): `game` (see above), `category`
  (case-insensitive prefix, e.g. `"Doors"`, `"Floors/"`), `search`
  (case-insensitive name substring), `special` (exact number; unknown
  numbers return an empty list, not an error — unless the number is
  generalized, below).
- Returns:

  ```json
  {
    "game": "doom",
    "count": 1,
    "specials": [
      {"special": 1, "name": "DR Door", "category": "Doors/Local",
       "target": "sector_back", "args": []}
    ],
    "generalized": null,
    "warnings": []
  }
  ```

### Generalized decoding

Boom packed whole families of actions into `LINEDEFS.special` values
`0x2F80..=0x7FFF` instead of enumerating them. Pass such a `special`
(number ≥ `0x2F80`, at or below `0x7FFF`) on a DOOM-family game and the
answer comes back in `generalized` with no table rows (`count` 0):

```json
{
  "kind": "Floor",
  "trigger": "WR",
  "speed": "fast",
  "fields": {"trigger": "WR", "speed": "fast", "model": "trigger",
             "direction": "up", "target": "lowest neighbour ceiling",
             "change": "none", "crush": "no"},
  "summary": "WR Floor up to lowest neighbour ceiling, fast"
}
```

Bit layout: every generalized value carries trigger (bits 0-2) and speed
(bits 3-4, `slow|normal|fast|turbo`); the block decides the rest:

| Range             | Kind        | Extra fields                                                                 |
|-------------------|-------------|------------------------------------------------------------------------------|
| `0x6000..=0x7FFF` | Floor       | model bit 5 (`trigger`/`numeric`), direction bit 6 (`down`/`up`), target bits 7-9 (highest/lowest/next neighbour floor, lowest neighbour ceiling, ceiling, shortest lower texture, 24, 32), change bits 10-11 (`none`, `zero type`, `texture`, `texture and type`), crush bit 12 |
| `0x4000..=0x5FFF` | Ceiling     | as Floor; direction bit 6 is `up` when clear, `down` when set; targets highest/lowest/next neighbour ceiling, highest neighbour floor, floor, shortest upper texture, 24, 32 |
| `0x3C00..=0x3FFF` | Door        | kind bits 5-6 (`open, wait, close`; `open and stay`; `close, wait, open`; `close and stay`), monsters bit 7, delay bits 8-9 (1, 4, 9, 30 s) |
| `0x3800..=0x3BFF` | Locked Door | kind bit 5 (`open, wait, close`/`open and stay`), key bits 6-8 (any, red/blue/yellow card, red/blue/yellow skull, all six), skull-acts-as-card bit 9 |
| `0x3400..=0x37FF` | Lift        | monsters bit 5, delay bits 6-7 (1, 3, 5, 10 s), target bits 8-9 (lowest neighbour floor, next neighbour floor, lowest neighbour ceiling, perpetual) |
| `0x3000..=0x33FF` | Stairs      | monsters bit 5, step bits 6-7 (4, 8, 16, 24), direction bit 8 (`down`/`up`), ignore-textures bit 9 |
| `0x2F80..=0x2FFF` | Crusher     | monsters bit 5, silent bit 6 |

Outside the range, on heretic/hexen (Heretic has its own tables but is not
Boom-compatible), or absent, `generalized` is null.
- Typical use: translating a `special` read from `LINEDEFS` (a number
  above 141 is likely generalized — decode it), or picking an action by
  category when building a map.

## `get_sector_specials`

- Purpose: list the named sector types, and decode a sector `special`
  value with the Boom bit layout.
- Arguments (all optional): `game` (see above), `special` — a sector
  `special` value to decode.
- Returns:

  ```json
  {
    "game": "doom",
    "count": 16,
    "special": 201,
    "decoded": {
      "base": 9,
      "base_name": "Secret",
      "damage": "10%",
      "flags": ["secret"]
    },
    "sector_types": [{"type": 0, "name": "Normal"}],
    "warnings": []
  }
  ```

  `sector_types` is the game's full table (sorted by `type`) whether or
  not `special` was given; `special` echoes the parameter (null without
  it) and `decoded` is null unless a DOOM-family game was given a
  `special`. `damage` is `"none"`, `"5%"`, `"10%"` or `"20%"`; `flags`
  is some of `"secret"`, `"friction"` (friction/current active) and
  `"push"` (wind/pushers active).
- Bit layout: basic type = `value & 31` (its table name, or `"Unknown"`
  for the unassigned values 6, 15 and 18-31); damage bits 5-6; secret
  bit 7; friction bit 8; push bit 9. Bits above 9 are ignored.
- Heretic lists its own 47 plain types instead (`count` 47, `decoded`
  always null): damage, light and scroll/wind effects are separate values,
  e.g. `5` is `5% Lava Damage`, `20` is `Scroll East (5)` and `43` is
  `Wind North (5)`.
- Typical use: reading a sector's `special` from a map and asking what it
  does (`{ "special": 201 }` → secret sector that drains 10% per touch).
