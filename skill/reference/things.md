# Thing tools

Creating, changing and removing the things of a map: `add_thing`,
`edit_thing` and `remove_thing`. A thing is one spawned object — a player
start, monster, item, decoration, teleport destination or script trigger —
placed at a point with a facing angle. To spread many copies across
sectors, a box or a circle with clearance and computed positions, use
`place_things` (`reference/features.md`); these three tools move exactly
one thing at coordinates you name.

## Things and their numbers

The `type` field is the thing number (`doomednum`); `get_thing_types`
(`reference/defs.md`) lists the game's table (numbers, names and what each
thing is), and `assemble_map include=["things"]` (`reference/maps.md`)
shows a map's current set with each thing's sector. For `Hexen`-family and
`UDMF` maps `tid` is the script id, `special` plus `args` the line-special
invocation — see `reference/defs.md` for the specials tables.

Thing edits never change geometry, so the generated node lumps
(`SEGS`/`SSECTORS`/`NODES`, `REJECT`, `BLOCKMAP`) stay trusted: every
result reports `nodes_invalidated: false`. Each call is one undoable step
(`undo` rolls the whole call back, `reference/session.md`), and editing a
map owned by a layer copies the block into the working archive first
(`copied_to_working: true`, `reference/maps.md`).

## Format rules

- `Doom`-family maps — binary `Doom`, and `UDMF` in the `doom`, `heretic`
  or `strife` namespace — store no `z`, `tid`, `special` or `args` on
  things. Passing any of them nonzero fails `invalid_argument`.
- Binary formats bound the stored fields: `type` must fit `u16`
  (`0..=65535`), `x`/`y`/`z`/`tid` must fit `i16` (`-32768..=32767`), and
  on `Hexen` `special` and every `arg` must fit `u8` (`0..=255`).
  Violations fail `invalid_argument`; `UDMF` maps store full integers and
  take any value.
- `angle` is always normalised into `0..=359` (`450` stores `90`);
  degrees are compass-style: `0` east, `90` north.

## Thing flags

`flags` is a raw bitset; every tool also takes names, from the table the
map's own family uses (a `Doom`-format map is `Doom`, a `Hexen`-format map
and any other `UDMF` namespace is `Hexen`). Absent `flags` on `add_thing`
defaults to all skills and all modes: `7` in the `Doom` family, `2023`
(`0x7E7`) in the `Hexen` one. Results echo both the bits (`flags`) and the
names (`flag_names`).

`Doom` family (the vanilla presence set):

| Name         | Bit | Meaning                                            |
|--------------|-----|----------------------------------------------------|
| `easy`       | 1   | Spawns on the easy skill.                          |
| `medium`     | 2   | Spawns on the normal skill.                        |
| `hard`       | 4   | Spawns on the hard skill.                          |
| `ambush`     | 8   | Deaf until it sees a player (BTU "ambush" bit).    |
| `notsingle`  | 16  | Multiplayer only: hidden in single player.         |
| `notdm`      | 32  | Never spawns in deathmatch.                        |
| `notcoop`    | 64  | Never spawns in cooperative play.                  |
| `friend`     | 128 | Friendly monster (Heretic/`Strife` games).         |

`Hexen` family (skill, mode, class and spawn-shape bits):

| Name           | Bit  | Meaning                                          |
|----------------|------|--------------------------------------------------|
| `easy`         | 1    | Spawns on the easy skill.                        |
| `medium`       | 2    | Spawns on the normal skill.                      |
| `hard`         | 4    | Spawns on the hard skill.                        |
| `ambush`       | 8    | Dormant until a player is close.                 |
| `dormant`      | 16   | Only activated by special means (not by sight).  |
| `class1`       | 32   | Spawns for the Fighter.                          |
| `class2`       | 64   | Spawns for the Cleric.                           |
| `class3`       | 128  | Spawns for the Mage.                             |
| `single`       | 256  | Spawns in single player.                         |
| `coop`         | 512  | Spawns in cooperative play.                      |
| `dm`           | 1024 | Spawns in deathmatch.                            |
| `translucent`  | 2048 | Drawn translucent.                               |
| `invisible`    | 4096 | Hidden in `Hexen` (fly item etc.).               |
| `friend`       | 8192 | Friendly monster.                                |
| `standing`     | 16384 | Spawns standing (rather than in its other pose). |

## `add_thing`

`{map?, type, x, y, angle?, z?, flags?, tid?, special?, args?}` →
`{index, thing, map, nodes_invalidated: false, copied_to_working}`

Appends one thing and reports its index with the stored fields. `map`
defaults like every map tool (active map, else the merged view's only
map). `flags` is the bitset or a list of names; an unknown name fails
`invalid_argument` listing the valid ones. Out-of-range binary values and
`Hexen`-only fields on a `Doom`-family map fail `invalid_argument`; an
unknown map fails `not_found`.

```json
call add_thing {"map": "MAP01", "type": 3004, "x": 384, "y": 128,
                "angle": 450, "flags": ["medium", "hard"]}
-> {"index": 2, "thing": {"type": 3004, "x": 384, "y": 128, "z": 0,
    "angle": 90, "flags": 6, "flag_names": ["medium", "hard"],
    "tid": 0, "special": 0, "args": [0, 0, 0, 0, 0]},
    "map": "MAP01", "nodes_invalidated": false, "copied_to_working": true}
```

## `edit_thing`

`{map?, index, type?, x?, y?, dx?, dy?, angle?, angle_delta?, z?, flags?,
set_flags?, clear_flags?, tid?, special?, args?}` →
`{things: [{index, type, x, y, z, angle, flags, flag_names, tid, special,
args}], map, nodes_invalidated: false, copied_to_working}`

`index` is one integer or a list; the changes apply to every selected
thing, and the reported rows come back sorted by index. Positions: `x`/`y`
set absolutely (single index only), `dx`/`dy` shift — a list moves with
the deltas. Angles: `angle` or `angle_delta`, both normalised into
`0..=359`. Flags: `flags` replaces the whole bitset (bits or names);
`set_flags`/`clear_flags` adjust the current bits by name (or bits) — a
bit named in both ends up set. Conflicts all fail `invalid_argument`:
`x` with `dx`, `y` with `dy`, `angle` with `angle_delta`, `flags` with
`set_flags`/`clear_flags`, absolute `x`/`y` with more than one index, and
a request that changes nothing. The `Doom`-family and binary range rules
apply to the resulting thing; an index the map does not hold is
`not_found` and nothing changes.

```json
call edit_thing {"index": [0, 1], "dx": 16, "dy": 16}
call edit_thing {"index": 0, "set_flags": ["ambush"],
                 "clear_flags": ["easy"], "angle_delta": -90}
```

## `remove_thing`

`{map?, index}` → `{removed: [indices], map, nodes_invalidated: false,
copied_to_working}`

Drops the selected things (one index or a list); the survivors renumber
down and nothing else — vertices, lines, sectors — is touched. `removed`
reports the original indices, sorted. An index past the end is
`not_found`; an empty list is `invalid_argument`.

## After editing

Check the work with `assemble_map include=["things"]` on the `"working"`
scope (types, positions and `flag_names` in index order), or
`render_topdown` with the thing markers on (`reference/render.md`) to see
the placement.
