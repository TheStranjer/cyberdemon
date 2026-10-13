# Definitions tools

Reference tables that describe what the game formats mean: thing types
today, linedef specials and sector types in later tickets. The tables are
embedded in the server binary, so these tools need no archives loaded.

## What ships

The thing table comes from SLADE's game configurations; each row's `game`
names the first game that has that thing. DOOM and DOOM II ship today
(97 and 123 rows). Heretic and Hexen tables are planned: those games
currently answer with an empty list and a "not yet available" warning.
Strife and an unidentified session fall back to the DOOM II rows, with a
warning saying so.

## `get_thing_types`

- Purpose: list the thing types an editor can offer for a game — type
  numbers, display names, sprite and default frame, blocking radius,
  category path and the hanging flag.
- Arguments (all optional):
  - `game` — `"doom"`, `"doom2"`, `"heretic"` or `"hexen"`. Defaults to
    the game detected from the session (see `reference/session.md`),
    with DOOM II standing for an unidentified session. Any other value
    is refused with `invalid_argument`.
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
  game; it is empty for DOOM and DOOM II.
- Errors: `invalid_argument` for a `game` outside the four supported
  names (the message lists them). A broken embedded table is a server
  bug and reports `internal`.
- Typical use: looking up spawn numbers when writing `THINGS`, and
  translating a type number found in a map (`{ "type": 3001 }` gives the
  Imp; pair it with the map's game when the row matters).
