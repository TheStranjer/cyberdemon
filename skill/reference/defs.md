# Definitions tools

Reference tables that describe what the game formats mean: thing types,
linedef specials and sector types, plus Boom's bit-packed generalized
decodings. The tables are embedded in the server binary, so nothing here
needs archives loaded — except `get_thing_types`, which by default also
reads the custom definitions of the session's merged view (below).

## What ships

DOOM, DOOM II, Heretic and Hexen ship today. The Doom side has 123 thing
types and 100 + 39 linedef specials (SLADE-style categories) and the 16
named Doom sector types; each row's `game` names the first game that has
it. Heretic has its own three tables: 84 thing types, 105 linedef specials
and 47 sector types, all plain values — Heretic is a Doom-engine game but
not Boom-compatible, so its specials and sector types are never generalized
or bit-decoded. Hexen also has its own three tables: 233 thing types, 86
linedef specials and 46 sector types; Hexen actions are plain numbers too
(never generalized), and each special lists the names of its up to five
script arguments in `args`, `arg1` first. Strife and an unidentified
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
  category path, the hanging flag and where the row came from (`source`).
- Arguments (all optional):
  - `game` — see above.
  - `category` — keep rows whose category starts with this text,
    matched case-insensitively (`"Monsters"`, or `"Gore/"` for every
    gore category).
  - `search` — keep rows whose name contains this text, matched
    case-insensitively.
  - `type` — keep exactly this type number. An unknown number returns an
    empty list, not an error.
  - `include_custom` — default `true`: overlay the custom definitions of
    the loaded archives (see "Custom definitions" below). `false` answers
    from the built-in tables alone.
- Returns:

  ```json
  {
    "game": "doom2",
    "count": 123,
    "things": [
      {"type": 5, "name": "Blue Keycard", "sprite": "BKEY",
       "frame": "A", "radius": 12, "category": "Keys",
       "hanging": false, "source": "builtin"}
    ],
    "warnings": []
  }
  ```

  `count` is the number of things after filtering and `things` is sorted
  by `type` ascending. `source` is `"builtin"`, `"dehacked"`,
  `"decorate"`, `"zscript"` or `"mapinfo"`. `warnings` explains a fallback
  game and
  quotes every custom-definition line the parsers refused to read; it is
  empty for a DOOM, DOOM II, Heretic or Hexen session without custom
  lumps.
- Typical use: looking up spawn numbers when writing `THINGS`, and
  translating a type number found in a map (`{ "type": 3001 }` gives the
  Imp; pair it with the map's game when the row matters).

## Custom definitions

With `include_custom` left on, `get_thing_types` reads these lumps from
the merged view (resource layers included) and overlays them on the
built-in table, in this precedence: **built-in < DEHACKED < DECORATE /
ZScript (+ MAPINFO)**. Load the archives first (`open_iwad`,
`merge_wad`) for them to count; nothing is read from disk.

- Every `DEHACKED` lump: `Thing <n>` blocks (1-based Doom II mobj index),
  read through the same parser as `get_dehacked` (`reference/scripts.md`),
  use `ID #` (new doomednum, `-1` removes the thing from the editor) and
  `Width` (radius in 16.16 fixed units). `#` comment lines are skipped and
  other block kinds (including `Text` bodies) bound the Thing blocks, so
  the two readers always see the same blocks. The moved row keeps the
  built-in name, sprite, frame and category unless the header names one,
  and reports `source: "dehacked"`, category `Custom (DeHackEd)` when
  there was no built-in row to copy.
- Every `DECORATE` lump: `actor Name [: Parent] [replaces Other]
  [doomednum] { ... }` — actors with a doomednum become things (category
  `Custom (DECORATE)`, `source: "decorate"`); the sprite and frame come
  from the first state line after `Spawn:` in the `States` block, the
  radius from `Radius <n>`; missing values inherit from the parent class
  wherever it stands (any DECORATE/`ZScript` text, before or after the
  child, or a built-in thing named by class name), else sprite stays empty
  and radius is 20.
- Every `ZSCRIPT` lump: `class Name : Parent { Default { Radius <n>; }
  States { Spawn: SPRT A ...; } }` — classes carry no doomednum of their
  own and become things only through MAPINFO.
- `#include "NAME"` lines inside DECORATE/ZSCRIPT pull the named lump
  (merged view, last match) into the same stream; the chain is followed
  at most 8 levels deep and each lump is read once.
- Every `MAPINFO`/`ZMAPINFO` lump: `DoomEdNums { <number> = <ClassName>
  ... }` maps numbers to the parsed DECORATE/ZScript classes (category
  `Custom (MAPINFO)`, `source` naming the class's language). An entry
  whose class was never parsed yields a name-only row with
  `source: "mapinfo"` and a warning.

The parsers are deliberately subsets and always tolerant: unparseable
text becomes a warning in `warnings`, never an error. Limits: DEHACKED
reads only `ID #` and `Width` of `Thing` blocks (no `Bits`, `Health`,
frames, weapons, players or text replacements); DECORATE/ZScript read
only the class header, `Radius` and the first `Spawn:` state (states
beyond that first frame, flags, inventory, sounds and every other
property are ignored); MAPINFO reading is limited to `DoomEdNums`
blocks.

## `get_linedef_specials`

- Purpose: list the special-table linedef actions a game knows, and
  decode any Boom generalized number.
- Trigger prefix legend (both table names and the decoder use it):
  `W` = walk over, `S` = switch/use, `G` = gun shot, `D` = use on the
  line itself, `M` = monster-activated; `1` = trigger once, `R` =
  repeatable. So `SR Floor...` fires when used from either side and
  re-arms.
- Row fields: `target` is a free-form hint naming what the action acts on:
  `"sector"` (acts on sectors tagged with the line's tag), `"sector_back"`
  (acts on the sector behind the line, no tag needed), `"sector_or_back"`,
  `"thing"`, `"line"`, `"ex_5thing"`, `"no"` or `"-"` (nothing tagged,
  e.g. exits); the text is passed through from the table unchanged. `args`
  lists the argument names in order (index 0 is `arg1`); it is empty for
  the Doom-family tables and fills in for Hexen specials, which take up to
  five script arguments.
- Arguments (all optional): `game` (see above), `family` (`"doom"` or
  `"hexen"`, below), `category` (case-insensitive prefix, e.g. `"Doors"`,
  `"Floors/"`), `search` (case-insensitive name substring), `special`
  (exact number; unknown numbers return an empty list, not an error —
  unless the number is generalized, below).
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

- The `family` parameter picks the specials table explicitly, for maps that
  use the Hexen action format (binary Hexen maps, UDMF `zdoom`-namespace
  maps) while the session itself is DOOM-family: `family: "hexen"` answers
  with the Hexen table and a warning naming the override; `family: "doom"`
  insists on the DOOM-family table (it is the default there, and is refused
  with `invalid_argument` for games that are not DOOM-family). Without
  `family` the table follows the game. Under `family: "hexen"` numbers are
  never generalized-decoded.

  ```json
  {
    "special": 13, "name": "Door_LockedRaise", "category": "Doors",
    "target": "sector_or_back",
    "args": ["Sector Tag", "Speed", "Delay", "Lock"]
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
  `Wind North (5)`. Hexen lists 46 plain types the same way (`decoded`
  always null): `200` is `Sky 2 (MAPINFO)`, `198` the lightning special,
  and `201..=224` the scrolling and wind directions.
- Typical use: reading a sector's `special` from a map and asking what it
  does (`{ "special": 201 }` → secret sector that drains 10% per touch).
