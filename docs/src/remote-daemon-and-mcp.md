# Remote Access, Daemon, and MCP

## `arbor-httpd`

The daemon provides:

- terminal session persistence
- remote GUI access
- process and agent activity endpoints
- webhook notification delivery
- the API surface used by the MCP server

## Remote Access

Remote daemon access can be authenticated with a bearer token from:

```toml
[daemon]
auth_token = "replace-me"
```

The GUI can connect to remote daemons and use them for terminal and worktree operations.

## Remote Outposts

Arbor also supports remote outposts over SSH and daemon-backed access, with:

- host management
- remote worktree creation
- remote terminal sessions
- availability tracking

## MCP

`arbor-mcp` exposes Arbor over stdio for MCP clients. It depends on a reachable daemon and supports:

- tools for repositories, worktrees, terminals, processes, and agent activity
- daemon-backed resources
- prompts for Arbor workflows

Use:

```bash
just run-mcp
```

or:

```bash
ARBOR_DAEMON_URL=http://127.0.0.1:8787 cargo run -p arbor-mcp
```
