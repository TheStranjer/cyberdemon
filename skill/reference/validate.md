# Validate tools

Reading a map's health: `validate_map` runs the structural, geometric and
resource checks and reports what it finds. It never changes anything —
fixing is the editing tools' job (`reference/things.md`,
`reference/sidedefs.md`, `reference/sectors.md`, `reference/maps.md`,
`reference/lumps.md`, `reference/nodes.md`).

## `validate_map`

`{map?, scope?, min_severity? = "info", codes?, limit? = 500}` →
`{map, counts: {error, warning, info}, issues, truncated, checks_run}`

- `map` and `scope` work as in the other map tools
  (`reference/maps.md`): the active map or the merged view's only map by
  default; `"merged"`, `"working"` or `"layer:N"`.
- `min_severity` is `"error"`, `"warning"` or `"info"` (default, keep
  everything), any case: milder rows drop out of `issues`.
- `codes` runs only the named checks (snake-case codes from the table
  below). An unknown code is `invalid_argument` listing the valid ones.
- `limit` (1 to 5000, default 500) pages `issues`; `truncated` says
  whether kept rows stayed behind. `counts` always totals **everything**
  the enabled checks found, before the severity filter and the limit —
  so `counts.info: 4` with an `"error"` floor tells you four milder rows
  exist without listing them.
- `checks_run` lists the codes that actually ran, in report order.

Each issue is `{severity, code, message, vertices, linedefs, sidedefs,
sectors, things, x, y}`: the index lists name the offending elements
(0-based, within the map) and only appear when the issue has them;
likewise `x`/`y` mark the affected point where there is one. Use them
with `render_topdown` to look at the spot.

## The checks

| Code | Severity | Meaning |
|------|----------|---------|
| `vertex_out_of_range` | error | A linedef's `v1`/`v2` names a vertex the map does not have. |
| `sidedef_out_of_range` | error | A linedef side names a sidedef past the block. |
| `sector_out_of_range` | error | A sidedef names a sector past the block. |
| `missing_front_side` | error | A linedef has no front side: nothing renders or blocks on it. |
| `zero_length_line` | error | A linedef's ends sit at the same point. |
| `duplicate_line` | error | Two linedefs connect the same vertex pair (either direction). |
| `two_sided_flag_mismatch` | warning | A back side without the two-sided flag (4), or the flag without a back side. |
| `unused_vertex` | info | No linedef end references the vertex. |
| `unused_sidedef` | info | No linedef side references the sidedef. |
| `unused_sector` | warning | No sidedef references the sector: it borders no geometry. |
| `shared_sidedef` | info | Several linedefs use one sidedef — legal in vanilla, but some editors refuse it. |
| `no_player1_start` | error | No thing of type 1: nobody can spawn. |
| `missing_coop_starts` | info | One or more of thing types 2-4 are absent. |
| `unknown_thing_type` | warning | The type is in no definition the session knows (see below). |
| `unknown_special` | warning | The linedef special is outside the map's flag-family table (see below). |
| `special_without_tag` | warning | A Doom-family sector special carries tag 0, so nothing triggers its sector. |
| `tag_without_sector` | warning | A Doom-family linedef carries a non-zero tag no sector in the map has. |
| `unclosed_sector` | error | A sector's directed boundary edges (front sides `v1`→`v2`, back sides `v2`→`v1`) leave vertices with more edges one way than the other: the ring has free ends. The issue lists every unbalanced vertex and gives the first one's `x`/`y`. |
| `crossing_lines` | error | Two linedefs cross at a point that is not one of their endpoints. Maps above 20 000 linedefs skip this check (see `check_skipped`). |
| `overlapping_lines` | error | Two collinear linedefs share a stretch of positive length — the same wall drawn twice. Touching at a single endpoint is fine. |
| `check_skipped` | info | The crossing check declined a map with more than 20 000 linedefs; nothing was checked there. |
| `ceiling_below_floor` | error | A sector's ceiling height is below its floor height. |
| `thing_outside_map` | warning | No sector contains the thing's point: it spawns into the void. |
| `missing_texture` | error | A sidedef face names a texture the merged view does not define (`TEXTURE1`/`TEXTURE2` entries plus the textures region). Empty and `"-"` faces are skipped. Runs only when the merged view defines textures (see `texture_check_skipped`). |
| `missing_flat` | error | A sector plane is not dressed. Its floor or ceiling flat is `"-"` (what a blank binary field reads back as) or empty — always an error, because every plane needs a flat; fix it with `edit_sector` `floor_flat`/`ceiling_flat` — or it names a flat the merged flats region does not carry, judged only when the session carries flats at all. The session's sky flat is always allowed, on either plane: the engine draws the sky itself (`F_SKY1` in every game but `Hexen`, where it is `F_SKY`). |
| `missing_required_texture` | warning | Hall of mirrors: a one-sided line with `"-"` middle, or a two-sided line whose neighbour's lower ceiling / higher floor demands an upper / lower face that is `"-"` (checked on both sides; the session's sky flat on both sectors' ceiling flats exempts the upper faces). |
| `texture_check_skipped` | info | The merged view has no `TEXTURE1`/`TEXTURE2` lump and no textures region, so wall-texture names were not checked. |
| `nodes_missing` | warning | A binary (Doom/Hexen-format) map whose `SEGS`, `SSECTORS` and `NODES` lumps are all empty or absent. Run `build_all_nodes`; `ZDoom`-family ports build the tree themselves, every other port does not. |
| `blockmap_missing` | info | A binary map without `BLOCKMAP` data. `build_all_nodes` or `build_blockmap` writes it. |
| `reject_size_mismatch` | warning | A present non-empty `REJECT` whose size is not `ceil(sectors² / 8)` bytes. `build_reject` fixes it. |

Errors mean the map is broken; warnings mean it is probably wrong; info
rows deserve a look. Run the check list again after every round of fixes
— `guides/verification.md` folds it into the verification loop.

## What "known" means

- **Thing types** check against the session's definitions: the detected
  game's table (`get_thing_types`, `reference/defs.md`) overlaid with the
  loaded archives' `DEHACKED`, `DECORATE`, `ZScript` and MAPINFO customs.
  A total conversion's own doomednums stay quiet; a monster the loaded
  `DEHACKED` moved away warns.
- **Specials** check against the map's own **flag family** table: binary
  `Doom` and the `doom`/`heretic`/`strife` `UDMF` namespaces use the
  detected game's table and also accept the Boom generalized range
  `0x2F80..=0x7FFF`; binary `Hexen` and `zdoom`-family namespaces use the
  `Hexen` table.
- The tag checks run **Doom-family only**: `Hexen`-family lines trigger
  through script arguments instead.
- **Textures and flats** check against the **merged view** whatever scope
  the map came from: defined textures are the winning `TEXTURE1`/
  `TEXTURE2` records plus the `TX_` lumps of a textures region, and flats
  are the lumps of a flats region. A sector plane wearing `"-"` or nothing
  needs no view to judge: no WAD carries such a flat, so `missing_flat`
  reports it whatever the session holds (a name is judged only against a
  view that carries textures or flats). An empty geometry edit's
  invalidated node lumps show up as `nodes_missing` / `blockmap_missing`
  until `build_all_nodes` runs.

## Not checked yet

Hexen/`UDMF` line and thing **arguments**, the internal consistency of
the node records (seg endpoints, subsector convexity) and sprite/sound
presence join this list in later tickets. `validate_map` reports
`checks_run` so you can tell which world you are in.
