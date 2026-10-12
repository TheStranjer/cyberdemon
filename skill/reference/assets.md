# Assets tools

Finding the asset server and tuning the store it keeps.

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
