# Music tools

Bringing external music files into the working archive and previewing the
music lumps a session holds.

## `import_music`

- Purpose: fetches a music file, identifies its container from the first
  bytes and stores it in the working archive as one undoable edit labelled
  `import_music NAME` (`save_wad` writes the result). Vanilla `Doom` plays
  only `MUS` from `D_*` lumps (`D_RUNNIN`, `D_INTER`, ...; Heretic uses
  `MUS_*` names; Hexen's map music names come from `MAPINFO`), while
  `ZDoom`-family ports also play `MIDI`, `OGG`, `MP3`, `FLAC` and tracker
  modules (`XM`, `IT`, `S3M`, `MOD`) straight from lumps — use `passthrough`
  for those.
- Recognised inputs (sniffed from the bytes): `MUS` (`MUS`+0x1A), `MIDI`
  (`MThd`), `OGG`, `MP3` (`ID3` tag or frame sync), `FLAC`, `WAV`, `XM`,
  `IT`, `S3M`, `MOD`. Anything else fails with `format_error`.
- Arguments:
  - `source` (string, required): absolute path, `file://` URI, `http(s)://`
    URL or this server's asset URL of the music file.
  - `name` (string, optional): 1 to 8 printable ASCII characters, uppercased.
    Defaults to `D_` plus the file stem uppercased and trimmed: `runnin.mid`
    becomes `D_RUNNIN`, `E1M2 theme.mid` becomes `D_E1M2TH`. A stem that
    already starts with `D_` is kept whole, so the prefix never doubles:
    `D_RUNNIN.mid` becomes `D_RUNNIN`, not `D_D_RUNN`. When nothing usable
    can be derived the call fails asking for this name.
  - `format` (string, optional): `"mus"` stores a vanilla `MUS` lump —
    `MIDI` is converted (this is where the `Doom` engine's own format comes
    from; `warnings` list the `MIDI` events `MUS` cannot carry), `MUS` is
    validated and stored unchanged, and any other input is refused with
    `invalid_argument`; `"passthrough"` stores the original bytes unchanged.
    Unset: `"mus"` when the input is `MIDI` or `MUS`, `"passthrough"`
    otherwise.
  - `replace` (boolean, optional, default `false`): overwrite the existing
    working lump of this name in place instead of refusing.
- Returns (structured content):

  ```json
  {
    "name": "D_RUNNIN",
    "input_format": "midi",
    "stored_format": "mus",
    "bytes": 23,
    "warnings": []
  }
  ```

  `input_format` is the sniffed container (`mus`, `midi`, `ogg`, `mp3`,
  `flac`, `wav`, `xm`, `it`, `s3m` or `mod`); `stored_format` is what was
  written (`"mus"` means a `MUS` lump, converted or not); `bytes` describes
  the stored lump; `warnings` carries one line per kind of `MIDI` event a
  conversion dropped (empty otherwise).
- Errors: `invalid_argument` for a bad `name` or `format`, or for asking
  `"mus"` of non-`MIDI`/`MUS` input; `format_error` for an unrecognized
  container or corrupt `MUS`/`MIDI` data; `unsupported` for `MIDI` this
  converter refuses (e.g. format 2); `limit_exceeded` when a converted
  score outgrows what `MUS` can hold; `already_exists` when the working
  archive holds that name and `replace` was not passed; `not_found` /
  `fetch_error` / `io_error` from the source lookup. Read the lump back
  with `get_lump` (hex or url) and check names with `list_lumps
  scope=working`.
- Example: turn a composed `.mid` into the `Doom` song lump:

  ```json
  {"source": "/data/runnin.mid"}
  ```

  For a `ZDoom` port keeping high-quality music, pass the original through
  under any name its `MAPINFO`/`D_DMGMUS` setup can reference:

  ```json
  {"source": "/data/doom3_intro.ogg", "name": "D_INTRO", "format": "passthrough"}
  ```

## `preview_music`

- Purpose: publishes one music lump of the session as a playable file on
  the localhost asset server and returns its `URL`. The archives are not
  modified and nothing is recorded in the undo history.
- Arguments: the shared lump selector: `name` (required) plus `scope`
  (`"merged"` default, `"working"`, `"layer:N"`), `namespace` and
  `occurrence` (default the last, winning match) — see
  `reference/lumps.md`.
- Behaviour by the lump's sniffed content (the same magics
  `import_music` recognises):
  - `mus` (a vanilla `D_*` score): converted to a type-0 `Standard MIDI`
    `File` and published as `NAME.mid` (`audio/midi`); a corrupt `MUS`
    is `format_error`.
  - `midi`: published unchanged as `NAME.mid` (`audio/midi`).
  - `ogg`, `mp3`, `flac`, `wav`: the bytes are published unchanged with
    the matching extension and media type.
  - `xm`, `it`, `s3m`, `mod` tracker modules: the bytes are published
    unchanged as `NAME.xm`/`.it`/`.s3m`/`.mod` (`application/octet-stream`;
    players that take module files directly accept these).
  - anything else: `unsupported`, message
    `lump NAME is not recognised music; use get_lump format=url for raw
    bytes`.
- Returns (structured content):

  ```json
  {
    "name": "D_RUNNIN",
    "input_format": "mus",
    "output_format": "mid",
    "asset": {
      "url": "http://localhost:25369/assets/3f9a.../D_RUNNIN.mid",
      "filename": "D_RUNNIN.mid",
      "mime": "audio/midi",
      "size": 128
    }
  }
  ```

  `input_format` is what the lump's bytes sniffed as; `output_format` is
  the published extension; `asset` is the playable file.
- Errors: `invalid_argument` for a bad selector field; `not_found` when no
  lump matches; `format_error` for a corrupt `MUS` score; `unsupported`
  for bytes no recognised music container owns.
- Example: listen to the working copy of a song converted on import:

  ```json
  {"name": "D_RUNNIN", "scope": "working"}
  ```
