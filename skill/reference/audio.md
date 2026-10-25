# Audio tools

Bringing external sound files into the working archive and previewing the
sounds a session holds.

## `import_sound`

- Purpose: fetches a `WAV`, `OGG`, `MP3` or `FLAC` file, decodes it, and
  stores it in the working archive as one undoable edit labelled
  `import_sound NAME` (`save_wad` writes the result). Vanilla `Doom` needs
  `SNDINFO`-free `DS*` names that match its built-in sound table (`DSPISTOL`,
  `DSDOORSW`, ...; the engine plays lump `DS<x>` for the sound named `<x>`),
  while `ZDoom`-family ports can map any lump name through `SNDINFO` and play
  `WAV`/`OGG`/`FLAC`/`MP3` lumps directly — use `passthrough` for those.
- Arguments:
  - `source` (string, required): absolute path, `file://` URI, `http(s)://`
    URL or this server's asset URL of the sound file.
  - `name` (string, optional): 1 to 8 printable ASCII characters, uppercased.
    Defaults to `DS` plus the file stem uppercased and trimmed: `pistol.wav`
    becomes `DSPISTOL`, `very long sound name.wav` becomes `DSVERYLO`. A stem
    that already starts with `DS` is kept whole, so the prefix never doubles:
    `DSPISTOL.wav` becomes `DSPISTOL`, not `DSDSPIST`. When nothing usable
    can be derived the call fails asking for this name.
  - `format` (string, optional): `"doom"` (the default) stores a vanilla
    `DMX` `DS*` lump (unsigned 8-bit mono, the engine's 16 pad bytes at each
    end); `"passthrough"` validates that the bytes decode, then stores the
    original file unchanged.
  - `sample_rate` (integer, optional, `doom` only): target rate in
    `4000..=48000`, default 11025.
  - `keep_rate` (boolean, optional, `doom` only, default `false`): store the
    source sample rate instead of resampling, when it is at most 48000 Hz.
  - `max_seconds` (integer, optional): length cap in `1..=600`, default 60.
    Longer sounds fail with `limit_exceeded` before anything is stored;
    decoding itself never returns more than 10 minutes of audio.
  - `replace` (boolean, optional, default `false`): overwrite the existing
    working lump of this name in place instead of refusing.
- Returns (structured content):

  ```json
  {
    "name": "DSPISTOL",
    "format": "doom",
    "source_format": "wav",
    "sample_rate": 11025,
    "channels_in": 2,
    "duration_ms": 1000,
    "bytes": 11065
  }
  ```

  `source_format` is the sniffed container (`wav`, `ogg`, `mp3` or `flac`);
  `sample_rate` is the rate of the stored sound (11025 by default, or the
  source rate under `keep_rate` / `passthrough`); `channels_in` is the
  source's channel count (`doom` sounds are downmixed to mono);
  `duration_ms` and `bytes` describe the stored lump.
- Errors: `invalid_argument` for a bad `name`, `format`, `sample_rate` or
  `max_seconds`; `format_error` for undecodable or corrupt input;
  `unsupported` for a container whose codec cannot be decoded here;
  `limit_exceeded` for a sound over `max_seconds` (or over 10 minutes);
  `already_exists` when the working archive holds that name and `replace`
  was not passed; `not_found` / `fetch_error` / `io_error` from the source
  lookup. Read the lump back with `get_lump` (hex or url) and check names
  with `list_lumps scope=working`.
- Example: convert a recorded pistol report for vanilla `Doom`:

  ```json
  {"source": "/data/pistol.wav", "keep_rate": false}
  ```

  For a `ZDoom` port keeping high-quality sound, pass the original through
  under any name its `SNDINFO` can reference:

  ```json
  {"source": "/data/pistol.ogg", "name": "PISTOL22", "format": "passthrough"}
  ```

## `preview_sound`

- Purpose: publishes one sound lump of the session as a playable file on
  the localhost asset server and returns its `URL`. The archives are not
  modified and nothing is recorded in the undo history.
- Arguments:
  - The shared lump selector: `name` (required) plus `scope`
    (`"merged"` default, `"working"`, `"layer:N"`), `namespace` and
    `occurrence` (default the last, winning match) — see
    `reference/lumps.md`.
  - `bits` (integer, optional): `8` or `16`, default `16`. The sample
    width of the generated `WAV` files; container lumps pass through
    unchanged.
- Behaviour by the lump's detected kind:
  - `doom_sound` (a `DMX` `DS*` lump): decoded and re-encoded as
    `NAME.wav`.
  - `pc_speaker` (a `DP*` tone-list lump: `u16` format `0`, `u16` tick
    count, one byte per 1/140 s tick): synthesized into a mono 11025 Hz
    square-wave `WAV`. Each tick keeps its exact share of samples, so
    timing does not drift. **The pitch is approximate**: tone `n` plays at
    `110 + 13·n` Hz instead of the vanilla divisor table, so treat it as a
    rhythmic preview, not a reference for the real tune.
  - `wav`, `ogg`, `mp3`, `flac`: the bytes are published unchanged with the
    matching extension.
  - anything else: `unsupported`, message
    `lump NAME is not a recognised sound; use get_lump format=url for raw
    bytes`.
- Returns (structured content):

  ```json
  {
    "name": "DSTEST",
    "kind": "doom_sound",
    "format": "wav",
    "sample_rate": 22050,
    "duration_ms": 1000,
    "asset": {
      "url": "http://localhost:25369/assets/3f9a.../DSTEST.wav",
      "filename": "DSTEST.wav",
      "mime": "audio/wav",
      "size": 44144
    }
  }
  ```

  `sample_rate` and `duration_ms` are present when the format names them:
  `DMX`, `WAV` and `PC` speaker results; `ogg`/`mp3`/`flac` pass-throughs
  report only the asset.
- Errors: `invalid_argument` for a bad selector field or a `bits` value
  that is neither `8` nor `16`; `not_found` when no lump matches;
  `format_error` for a corrupt `DMX` or `DP` lump; `unsupported` for a
  non-sound kind.
- Example: listen to the working copy of a door sound:

  ```json
  {"name": "DSDOORSW", "scope": "working"}
  ```
