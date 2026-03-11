# Daemon Contract

This page mirrors the current daemon contract overview used by Arbor.

## Goals

- keep terminal sessions alive across GUI restarts
- allow warm reattach to live sessions
- keep the backend replaceable through traits

## Trait Surface

Defined in `arbor-core::daemon`:

- `TerminalDaemon`
  - `create_or_attach`
  - `write`
  - `resize`
  - `signal`
  - `detach`
  - `kill`
  - `snapshot`
  - `list_sessions`
- `DaemonSessionStore`
  - `load`
  - `save`
  - `upsert`
  - `remove`
- `JsonDaemonSessionStore`
  - default persistence path: `~/.arbor/daemon/sessions.json`

## Session Model

`DaemonSessionRecord` stores:

- `session_id`
- `workspace_id`
- `cwd`
- `shell`
- `cols`
- `rows`
- `title`
- `last_command`
- `output_tail`
- `exit_code`
- `state`
- `updated_at_unix_ms`

`TerminalSnapshot` stores:

- `session_id`
- `output_tail`
- `exit_code`
- `state`
- `updated_at_unix_ms`
