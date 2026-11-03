# Images to assets

Tools: open_iwad, get_asset_server_info, process_image, create_texture_from_image, create_flat_from_image, create_sprite_from_image, import_image_as_patch, import_image_as_flat, import_image_as_raw, import_image_as_sprite, list_sprites, get_sprite, get_thing_sprite, edit_decorate, edit_dehacked, undo, set_texture_theme, auto_texture, replace_texture, bulk_edit, edit_sidedef, edit_sector, edit_linedef, find_elements, describe_map, render_view, render_topdown, set_palette, save_wad

Turn a drawn or photographed image into a working Doom resource. The tools
resize, quantize and file the picture away; what to make, how big, how
much colour to spend and where it goes are your calls. Every importer needs
a session with a `PLAYPAL` to quantize against — `open_iwad` first — and
every `source` is an absolute path or URL (for bytes you can only see, ask
the user for the file or `PUT` it to the origin from
`get_asset_server_info`). One undoable edit per commit; `undo` takes a bad
one back.

## 1. Which tool

| What the user wants | Tool |
|---------------------|------|
| A wall texture (STARTAN-style surface for walls) | `create_texture_from_image` |
| A floor or ceiling surface | `create_flat_from_image` |
| A monster or decoration sprite frame | `create_sprite_from_image` |
| HUD, menu, title or status-bar art | `process_image`, then `import_image_as_patch` (set `offsets`) — or `import_image_as_raw` to keep a PNG for ZDoom-family ports |
| A sky | `create_texture_from_image`, named after the sky the map uses (`SKY1`; `MAP01`–`MAP11` take `SKY1`, `MAP12`–`MAP20` `SKY2`, `MAP21`+ `SKY3`), size `256x128`, `tileable: false`, `replace: true` to override the game's own |
| Just experimenting | `process_image` — publishes, changes nothing |

The `create_*` tools do the whole job in one call: ops, resize, quantize,
lump, `PNAMES`, texture definition. Reach for the plain importers only when
you need their extra knobs (`transparency`, `offsets`, `add_to_pnames`).

## 2. Look before you commit

Run `process_image` with exactly the `ops` you plan to use. It publishes
two PNGs: `processed` (full colour after your ops) and `quantized` (the
indexed result rendered back through the palette — what the game will
actually show). Open `quantized`, compare it with the original, and adjust
the ops until the loss looks acceptable. The numbers tell the same story:
`colors_used` against the 256-colour budget, `max_color_error` (`0` means
the source was already palette-exact; large means heavy banding or shift),
`transparent_pixels`. A `processed` URL is itself a valid `source` for the
import tools, so commit from the preview you approved — do not re-run your
ops from memory on the original file.

When the user only wants their picture *in* unchanged — a portrait as a
custom patch, a hi-res PNG flat for a modern port — skip the pipeline and
`import_image_as_raw`; it writes the bytes as they are, no quantization.

## 3. Choosing the ops

Apply them in this order: clean, correct, tone, tile.

- **`crop` to the material first.** A photo of a wall: cut away the floor,
  the ceiling line, the vignette corners and any shadow gradient — all of
  it would stretch into the texture.
- **`auto_levels`** for dull photos (`clip_percent` 1–3) — it stretches
  the luminance range to full scale.
- **`adjust`** for tone: `saturation` 110–130 revives washed-out prints;
  `contrast` 110–140 clears muddy scans; `brightness` is a last resort.
- **`remove_background`** for drawings on paper — it floods in from the
  border, so enclosed white details survive (a `color_key` would punch
  holes through them). Raise `tolerance` (default 32) for uneven paper.
- **`make_tileable`** (or `tileable: true` on the `create_*` calls) blends
  opposite edges so the surface repeats seamlessly: use it for wall and
  floor material, **never** for signs, pictures, switches or anything that
  reads as one object — a blended duplicate of the object ghosts at the
  seams.
- **`dither`** is a quantize knob, not an op: `process_image`, the three
  `create_*` calls and `import_image_as_flat` take `"floyd_steinberg"`
  (the default), `"none"` or `"ordered"`; the plain patch and sprite
  importers have no knob and map every pixel to the nearest palette
  colour — what `"none"` does. **Keep `floyd_steinberg`** for photos and
  smooth gradients: error diffusion fakes the in-between shades the
  `PLAYPAL` lacks. **Pass `none`** for anything with large flat areas —
  solid fills, logos, signage, pixel art, line drawings — where
  diffusion only adds noise dots inside the solids and no missing shade
  is worth faking. `ordered` (a fixed 4×4 `Bayer` pattern) is the middle
  choice when floyd looks patchy and an even fine texture is wanted.
  `import_image_as_flat` and `create_flat_from_image` echo the mode they
  used in their result.
- **Quiet a noisy source before blaming the dither.** Generated images
  and photographed screens carry per-pixel colour noise, and
  `floyd_steinberg` spreads that noise over the whole picture.
  Downscaling already helps (the automatic `fit` resamples through the
  area filter, averaging the noise away); a `blur` of radius 1–2 helps
  more. Then re-check `process_image`'s `quantized` preview (§2): solid
  areas must stay solid, and a `colors_used` near 256 for a logo drawn
  in three says the noise is still in the picture.

## 4. Sizes

- **Wall textures**: 64×128 or 128×128. The `width` must be a power of two
  (`16..=1024`); `height` reaches 1024, so a tall wall panel is 128×256.
  128 is the height of a standard wall band — a texture half that height
  tiles twice and shows a repeat seam at eye level. For art cut to one
  named edge — a facade, a sign — size it from that edge; §5 has the fit.
- **Flats**: **64×64** for anything vanilla must load — floors and
  ceilings tile one flat per 64×64 map units. `create_flat_from_image`
  also takes `size` 128 or 256 for ZDoom-family ports and warns that
  vanilla cannot read them; `import_image_as_flat` keeps `64×128` too.
  Both echo the `size` and `dither` they used.
- **Sprites**: measure in pixels, standing on their origin —
  player-height monsters 40–60 px tall, big monsters 80–110, items 16–32.
  Pass `height` on `create_sprite_from_image` to pin it (it keeps the
  aspect and defaults to trimming the empty margin first).
- **Skies**: 256×128.

Oversized or oddly shaped sources are resized, never silently kept: watch
`notes` and `steps` in the result for what the tool decided.

## 5. Fitting one wall: facades and signage

`create_texture_from_image` never sees the wall it is cut for — it files the
picture at the size you pass. For a facade or sign that must sit on **one
specific edge** (a storefront, a plaque, a nameplate over a door), size and
place from the edge itself. Read `L`, the edge's length, from
`find_elements` (`kind: "linedefs"`, the derived `length` is the integer
hypot of its endpoints in map units).

- **Width: the smallest power of two ≥ `L`.** The engine maps a face
  horizontally as `x_offset + distance along the edge`, wrapped modulo the
  texture width (which is why only powers of two `16..=1024` are accepted).
  With `W ≥ L` the wall can never show a second copy of the artwork: paint
  it into the middle `L` columns of the picture, and the spare margin sits
  off the wall. An edge over `1024` long must be split, or accept a repeat.
- **`x_offset = (W − L) / 2 + d`**, with `d` the inset from the edge's
  start vertex — from `v1` seen from the front, from `v2` seen from the
  back, so the two faces of a shared wall need their own offsets and the
  back face reads mirrored. This is a per-wall number: `edit_sidedef`, or
  `edit_linedef`'s `front_side`/`back_side` for the side the player sees.
- **Height: at least the band the face shows.** Texture rows wrap modulo
  the height, so anything shorter tiles vertically through the artwork.
  The height is not restricted to powers of two (`1..=1024`), and a sign
  is never `tileable` (§3). `144` serves a `112`-high walkway well: door
  art in the lower `112` rows, a `32`-row sign band above them, in one
  picture.
- **The sign's upper band is the ceiling gap.** Standing in the walkway,
  the `upper` face draws between the two sectors' ceilings — so a sign `S`
  rows tall needs the sector behind the wall (the header/bridge over the
  door) to reach at least `walkway ceiling + S`; a header ceiling level
  with the walkway shows no band at all.
- **Set `dontpegtop` on the sign's line — and never on a sliding door
  face.** A pegged upper hangs its window from the far ceiling and slides
  whenever that ceiling moves: right for the door face itself (that is why
  `add_door` clears the bit, so the face rides the rising door ceiling),
  wrong for a sign. `dontpegtop` measures from the viewer-side (sidedef's
  own) ceiling instead: with `y_offset = header − walkway` the picture's
  top rows sit flush under the header ceiling, upright, and no door motion
  shifts them. One line, one texture: closed, the band shows the door art;
  open, it shows the sign.

Worked example — a 100-unit storefront edge on a walkway of floor `0`,
ceiling `112`, over a door/header sector whose ceiling rises to `144` when
the door opens:

```json
call create_texture_from_image {"source": "/abs/shop01.png", "name": "SHOPFRNT",
                                "width": 128, "height": 144, "tileable": false}
call edit_linedef {"index": 42, "set_flags": ["dontpegtop"],
                   "front_side": {"upper": "SHOPFRNT", "x_offset": 14, "y_offset": 32}}
```

`W = 128` is the smallest power of two ≥ 100, so the sign occupies columns
14..114 and `x_offset = (128 − 100)/2 = 14` centres it; `y_offset = 144 −
112 = 32` drops the picture's top 32 rows into the `112..144` band. Check
with `render_view` from mid-walkway, door closed and open: the sign reads
upright and stays put while the door face slides away under it.

## 6. Using the result

- **New geometry**: pass `assign_role` on the create call (`"wall"` for a
  texture; `"floor"`, `"ceiling"`, `"liquid_floor"`... for a flat). The
  theme then dresses everything built afterwards — plan rooms, drawn
  sectors, automatic doors. For an existing map, `auto_texture` with
  `mode: "retexture"` repaints it from the theme in one step;
  `set_texture_theme` `overrides` pin a role by hand. Theme state is
  session-only and survives `undo` — say so when you change it.
- **Existing walls by name**: `replace_texture` with the old glob and the
  new name (`dry_run: true` first to count faces); `bulk_edit` for texture
  plus other fields at once; `edit_sidedef` for one side.
- **Existing floors/ceilings**: `edit_sector` on the sector indices from
  `describe_map`.

Then **look**: `render_view` at a spot where the surface shows, and
`render_topdown` for the whole dress. The source image is not the engine —
seams, vertical repeats and palette shift only appear in the render. A
missing name in `render_view`'s `problems` means the lump or definition
did not land where the engine reads it.

## 7. Sprites need definitions

A new sprite lump is pixels in a region: no monster, no decoration, nothing
to shoot. The name is the contract — four sprite characters, a frame
letter, a rotation digit: `TROOA1` … `TROOA8` are the imp's frame `A` from
the eight viewer angles; `TROOA2A8` stores one image and serves rotation 8
mirrored; rotation `0` (`PLAYA0`) serves every angle with one picture.
Frames `A`, `B`, `C`, … are the animation cells. `list_sprites` with a
four-character `prefix` enumerates the lumps a sprite really has — the
frame × rotation set you must match — and `get_sprite` /
`get_thing_sprite` publish individual rotations so you can compare your
drawing's proportions.

To make something appear, choose one:

- **Replace an existing thing's look** (vanilla-safe): overwrite **all**
  the lumps it reads — every frame × rotation, `replace: true`, same
  names, via `import_image_as_sprite` or `create_sprite_from_image`. Half
  a set leaves the monster flickering between your art and the original as
  it turns. `create_sprite_from_image` with `frames: ["A","B"]` writes one
  static pose across several frames.
- **Define a new actor**: `edit_decorate` a new `actor` with
  `obituary`, `spawnstate` and your sprite names — plays on ZDoom-family
  ports only; say so. `edit_dehacked` can repoint an existing type's
  frames for vanilla-family play, but the frames must still exist.
- **Static decorations first.** A single-frame, rotation-0 prop (a barrel
  that is now a fountain) needs one lump and reads well; an animated
  monster needs dozens of consistent drawings. Recommend the former when
  the user brings one picture.

## 8. Palette limits

Everything in the game shares **one** 256-colour `PLAYPAL`. Imports
quantize against it (`palette` picks the entry, default 0) — colours your
picture needs that the palette lacks are snapped to the nearest neighbours,
which is why §2 exists. Keep each asset's `colors_used` modest so the
budget lasts for the whole WAD.

Do **not** call `set_palette` unless the user explicitly wants a new
palette: it rewrites `PLAYPAL` (and with `regenerate_tints`, the vanilla
red/gold/green tint tables and `COLORMAP` too), which recolours every
texture, flat and sprite in the session — including the base game's own
art. Adding content to an existing WAD means fitting its palette, not
moving the palette.

When the work is committed and rendered well, `save_wad` writes it; a
`PNG`-encoded flat or a raw PNG lump (from `import_image_as_raw`) needs a
modern port — vanilla wants the importer's Doom formats.
