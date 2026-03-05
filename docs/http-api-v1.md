# Arbor HTTP API (v1)

Base URL defaults to `http://0.0.0.0:8787` (set via `ARBOR_HTTPD_BIND`).

## Endpoints

### `GET /api/v1/health`

Returns daemon health.

### `GET /api/v1/repositories`

Returns known repository roots from `~/.arbor/repositories.json`.

### `GET /api/v1/worktrees`

Returns worktrees across known repositories.

Query params:

- `repo_root` (optional): filter to one repository root path.

### `GET /api/v1/terminals`

Returns merged terminal session records from the daemon runtime and `~/.arbor/daemon/sessions.json`.

### `POST /api/v1/terminals`

Creates or attaches a terminal session.

Request body:

```json
{
  "session_id": "daemon-1",
  "workspace_id": "/Users/penso/code/arbor",
  "cwd": "/Users/penso/code/arbor",
  "shell": "/bin/zsh",
  "cols": 120,
  "rows": 35,
  "title": "term-arbor"
}
```

`session_id` is optional, the daemon will generate one when omitted.

### `GET /api/v1/terminals/:session_id/snapshot`

Returns output tail and state for one session.

Query params:

- `max_lines` (optional, default `180`, max `2000`)

### `POST /api/v1/terminals/:session_id/write`

Writes UTF-8 input to a terminal.

Request body:

```json
{
  "data": "ls -la\n"
}
```

### `POST /api/v1/terminals/:session_id/resize`

Resizes a terminal grid.

Request body:

```json
{
  "cols": 120,
  "rows": 35
}
```

### `POST /api/v1/terminals/:session_id/signal`

Sends a signal to a terminal session.

Request body:

```json
{
  "signal": "interrupt"
}
```

Allowed values: `interrupt`, `terminate`, `kill`.

### `POST /api/v1/terminals/:session_id/detach`

Detaches the current client from a daemon-managed terminal session without killing it.

### `DELETE /api/v1/terminals/:session_id`

Kills and removes a daemon-managed terminal session.

### `GET /api/v1/terminals/:session_id/ws`

WebSocket stream for interactive terminal I/O.

Client messages:

- `{"type":"input","data":"echo hi\n"}`
- `{"type":"resize","cols":140,"rows":40}`
- `{"type":"signal","signal":"interrupt"}`
- `{"type":"detach"}`

Server messages:

- `{"type":"snapshot", ...}`
- `{"type":"output", ...}`
- `{"type":"exit", ...}`
- `{"type":"error", ...}`
