# Meta tools

Server identity and liveness.

## `version`

- Arguments: none.
- Returns: plain text `cyberdemon <semver>` (e.g. `cyberdemon 0.1.0`), as the
  only content block. No structured content.
- Purpose: confirm the server is alive and which build you are talking to.
- Errors: none. This tool cannot fail at the domain level; an unknown-tool
  protocol error means you misspelled it.
