# HTTP API v1

Base URL defaults to:

- `http://127.0.0.1:8787` when remote auth is disabled
- `http://0.0.0.0:8787` when `[daemon] auth_token` is configured

## Auth

- loopback callers are allowed without authentication
- non-loopback callers require `[daemon] auth_token`
- remote requests use `Authorization: Bearer <token>`

## Main Endpoints

### Health and Repositories

- `GET /api/v1/health`
- `GET /api/v1/repositories`

### Worktrees

- `GET /api/v1/worktrees`
- `POST /api/v1/worktrees`
- `POST /api/v1/worktrees/delete`
- `GET /api/v1/worktrees/changes`
- `POST /api/v1/worktrees/commit`
- `POST /api/v1/worktrees/push`

### Terminals

- `GET /api/v1/terminals`
- `POST /api/v1/terminals`
- `GET /api/v1/terminals/:session_id/snapshot`
- `POST /api/v1/terminals/:session_id/write`
- `POST /api/v1/terminals/:session_id/resize`
- `POST /api/v1/terminals/:session_id/signal`
- `POST /api/v1/terminals/:session_id/detach`
- `DELETE /api/v1/terminals/:session_id`
- `GET /api/v1/terminals/:session_id/ws`

### Agent Activity

- `GET /api/v1/agent/activity`
- `POST /api/v1/agent/notify`
- `GET /api/v1/agent/activity/ws`

### Processes

- `GET /api/v1/processes`
- `POST /api/v1/processes/start-all`
- `POST /api/v1/processes/stop-all`
- `POST /api/v1/processes/:name/start`
- `POST /api/v1/processes/:name/stop`
- `POST /api/v1/processes/:name/restart`
- `GET /api/v1/processes/ws`

### Daemon Control

- `POST /api/v1/shutdown`
- `POST /api/v1/config/bind`
- `GET /api/v1/config/bind`

For the canonical standalone reference, see [../http-api-v1.md](../http-api-v1.md).
