# Assets tools

Finding the asset server, tuning the store it keeps, and handing one
session resource to you as a file you can look at or listen to.

## `get_asset_server_info`

- Purpose: reports the localhost HTTP origin your client can upload files
  to and read assets from, plus how full the store currently is. Call it
  before an upload, or when no tool result has handed you a URL yet. Starts
  the asset server if it is not running.
- Arguments: none.
- Returns (structured content):

  ```json
  {
    "base_url": "http://localhost:41234",
    "upload_url_template": "http://localhost:41234/upload/{filename}",
    "capacity": 50,
    "stored": 3,
    "stored_bytes": 12345,
    "max_asset_bytes": 268435456
  }
  ```

  `upload_url_template` is where you send a file body: replace `{filename}`
  with the name to store it under (names are sanitized to
  `[A-Za-z0-9._-]`, 64 characters). `capacity` is how many of the most
  recent assets are kept, `stored` and `stored_bytes` the current fill, and
  `max_asset_bytes` the largest single file the server accepts.
- Errors: `limit_exceeded` if no free port can be bound (retry later);
  `io_error` if the server socket or thread cannot start; `internal` on a
  poisoned lock.
- Example: upload with curl, then pass the receipt's URL to tools that
  accept a `source`:

  ```sh
  curl -T ./pic.png http://localhost:<port>/upload/pic.png
  # -> 201 {"url": "...", "id": "...", "filename": "...", "size": n}
  ```

  Take `<port>` from `base_url`; `PUT` and `POST` both work.

## `set_asset_capacity`

- Purpose: changes how many of the most recent assets the store keeps, at
  runtime, instead of via the `CYBERDEMON_ASSET_CAPACITY` environment
  variable. Use it when the default of 50 keeps too few files (things you
  still need 404) or too many in memory.
- Arguments: `capacity` (integer, `1..=10000`) — the new maximum number of
  stored assets. Shrinking below the current fill evicts the oldest assets
  immediately; growing evicts nothing.
- Returns: `{"capacity": <n>, "evicted": <k>}` — the new capacity and how
  many assets the change dropped (`0` when growing or when it still fits).
- Errors: `invalid_argument` if `capacity` is outside `1..=10000`; the store
  is then left unchanged.

## `get_asset_url`

- Purpose: one entry point for "give me something to look at or listen to
  for NAME". It resolves the name, decides what kind of resource the bytes
  are, converts them with the same code the dedicated tools use and
  publishes one file on the asset origin. The archives are never modified:
  a call is a read, not an edit, so nothing lands in the undo history.
- Arguments:
  - `name` (string, required): the resource's name, matched as a lump name
    (1-8 printable ASCII characters, case-insensitive).
  - `scope` / `namespace` / `occurrence` (optional): the shared lump
    selector, exactly as `get_lump` takes it (see `reference/lumps.md`).
    The default `occurrence` is the last, winning match.
  - `palette` (number, optional, default 0): the `PLAYPAL` entry that
    colours an indexed picture and the palette swatch.
  - `prefer` (string, optional, default `"lump"`): what a name that both a
    lump and a `TEXTURE1`/`TEXTURE2` definition owns means. `"texture"`
    composites the definition. A texture name no lump carries composites
    either way. Any other word is `invalid_argument`.
- What it publishes, in the order the decision is made:

  | The name resolves to | `resolved_as` | File |
  |---|---|---|
  | a `TEXTURE1`/`TEXTURE2` definition (`prefer: "texture"`, or a texture with no lump of its name) | `texture` | `PNG` of its `PNAMES` patches drawn on its own canvas, later mappatches over earlier |
  | a flat, or any lump in the flats region | `flat` | `PNG` of the `PLAYPAL` index plane; a `PNG`-encoded flat is quantized then recoloured |
  | a patch, a sprite picture, or any bytes with a `Doom` patch header | `patch` or `sprite` | `PNG` with the drawing offsets carried in a `grAb` chunk |
  | a `PNG`, `JPEG` or `GIF` lump | `image` | the same picture re-encoded as `PNG`, keeping a `PNG`'s `grAb` offsets |
  | a `DMX`, `PC` speaker, `WAV`, `OGG`, `MP3` or `FLAC` sound lump | `sound` | what `preview_sound` publishes: a `WAV`, or the original container |
  | a `MUS`, `MIDI` or tracker-module lump | `music` | what `preview_music` publishes: `MIDI` for a `MUS` lump |
  | `PLAYPAL` | `palette` | the swatch `get_palette` publishes, of the requested entry |
  | `COLORMAP`, or any lump in the `colormaps` region | `colormap` | a `PNG` one row per light table, one column per palette index, drawn through `PLAYPAL` entry 0 |
  | anything else | - | `unsupported`, naming `get_lump format=url` as the way out |

  A `COLORMAP` grid always uses entry `0`: the tables map that palette onto
  its darkened copies, so another entry would show colours no light level
  ever produced.
- Returns (structured content):

  ```json
  {
    "name": "WALL1", "resolved_as": "texture", "mime": "image/png",
    "width": 64, "height": 128,
    "asset": {"url": "http://localhost:PORT/assets/ID/WALL1.png",
              "filename": "WALL1.png", "mime": "image/png", "size": 386}
  }
  ```

  `width` and `height` are present for a picture only; a sound or music
  file reports no pixel size. `resolved_as` tells you which row of the
  table matched, so a name you were unsure about answers for itself.
- Errors: `not_found` when no lump matches and no texture definition carries
  the name either (the message says so and points at `list_textures`), for
  a `palette` past the loaded `PLAYPAL`'s last entry, or for a component a
  texture record needs that is not there; `unsupported` for bytes nothing
  converts; `format_error` for a payload the chosen converter cannot read;
  `invalid_argument` for a bad name, `scope`, `namespace` or `prefer`.
- Example: the four things worth looking at in one map's texture set:

  ```json
  {"name": "WALL1", "prefer": "texture"}
  {"name": "FLOOR4", "palette": 2}
  {"name": "DSPISTOL"}
  {"name": "D_RUNNIN"}
  ```

  Fetch each returned `url` to see (or hear) the file.

