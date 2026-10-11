---
name: cyberdemon
description: Use the Cyberdemon MCP server to inspect and edit Doom WAD files. Use when asked to read, understand, or modify a WAD.
---

# Using Cyberdemon

Cyberdemon is an MCP server over stdio for working with Doom WADs. Tools appear in your client prefixed with the server name (e.g. `mcp__cyberdemon__version`).

## Tools

| Tool      | Arguments | Returns                                            |
|-----------|-----------|----------------------------------------------------|
| `version` | none      | Text: `cyberdemon <semver>`. Use it to confirm the server is alive and which build you have. |

WAD reading and editing tools are not implemented yet. When they are, document each one here: purpose, arguments, result shape, and error cases.

## Errors

Calling an unknown tool fails with a protocol error (`INVALID_PARAMS`). Do not retry; check the tool list.
