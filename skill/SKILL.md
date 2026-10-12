---
name: cyberdemon
description: Use the Cyberdemon MCP server to inspect and edit Doom WAD files. Use when asked to read, understand, or modify a WAD.
---

# Using Cyberdemon

Cyberdemon is an MCP server over stdio for working with Doom WADs. Tools appear in your client prefixed with the server name (e.g. `mcp__cyberdemon__version`).

## Who decides what

The server is the reflex; you are the judgment. Every tool is a deterministic
function of the parameters you pass: it does formulaic work exactly — builds
the shape at the size you said, compiles the plan you wrote, measures the
image you attached — and never guesses what you meant. Interpreting requests
and drawings, choosing shapes, scale, themes and balance, deciding what "make
it scarier" means, and checking renders against intent are your job.

The [guides index](guides/README.md) collects what you need for that side.
Start with `guides/design-rules.md` for the dimensions and gameplay facts to
design against, and run the loop in `guides/verification.md` after every
meaningful change.

## Conventions

- **Successful results are structured JSON.** Read the result's structured
  content; the text content mirrors it. Exception: `version` returns plain
  text (see `reference/meta.md`).
- **Domain failures come back as errored results**, with structured content
  exactly:

  ```json
  {"error": "<code>", "message": "<human-readable text>"}
  ```

  `error` is a stable machine-readable code; `message` is for humans and may
  change. Codes:

  | Code               | Meaning                                                        |
  |--------------------|----------------------------------------------------------------|
  | `invalid_argument` | An argument was missing, malformed or out of range.            |
  | `not_found`        | The requested lump, map or other resource does not exist.      |
  | `already_exists`   | A resource with that name is already taken.                    |
  | `conflict`         | The operation conflicts with current state (e.g. unsaved edits). |
  | `format_error`     | A WAD or other input is malformed or unreadable as data.       |
  | `io_error`         | Reading or writing the filesystem failed.                      |
  | `fetch_error`      | Fetching a remote resource failed.                             |
  | `unsupported`      | The requested feature or format is not supported.              |
  | `limit_exceeded`   | A configured limit (size, count, depth) was exceeded.          |
  | `internal`         | Server-side fault (e.g. a poisoned session lock). Report it; do not retry blindly. |

- **Unknown tools fail at the protocol layer** with the JSON-RPC error
  `INVALID_PARAMS`, not as a tool error. Do not retry; list the tools.

## Tools

| Tool      | Reference                    |
|-----------|------------------------------|
| `version` | `reference/meta.md`          |

WAD reading and editing tools are not implemented yet. When they are, add one
reference file per tool group under `reference/` and index each tool here.
