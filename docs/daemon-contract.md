# Arbor Terminal Daemon Contract (v0)

## Goals
- Keep terminal sessions alive across GUI restarts.
- Allow warm reattach to live sessions, and cold restore when daemon is gone.
- Keep backend replaceable through traits (local daemon today, remote daemon later).

## Trait Surface
Defined in `arbor-core::daemon`:

- `TerminalDaemon`
  - `create_or_attach`
  - `write`
  - `resize`
  - `signal`
  - `detach`
  - `kill`
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

## Restart Persistence Model
1. On daemon create/attach, `upsert` the session record.
2. On detach, keep record (session may still be running).
3. On kill/exit, `remove` record.
4. On app start, daemon/runtime reads `load()` and reattaches to known sessions.
5. If daemon is unavailable, UI can still show restorable session metadata from the store.

## Next Steps
1. Add daemon host crate implementing `TerminalDaemon`.
2. Add event stream contract (`data`, `exit`, `disconnect`, `error`).
3. Persist scrollback snapshots for cold restore.
4. Promote JSON store behind same trait to SQLite implementation.
