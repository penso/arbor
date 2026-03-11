# MCP

Arbor ships `arbor-mcp`, a stdio MCP server backed by `arbor-httpd`.

## Environment

- `ARBOR_DAEMON_URL`
- `ARBOR_DAEMON_AUTH_TOKEN`

## Typical Local Run

```bash
just run-mcp
```

## Typical Direct Run

```bash
ARBOR_DAEMON_URL=http://127.0.0.1:8787 cargo run -p arbor-mcp
```

## What Arbor Exposes Over MCP

- tools for repositories, worktrees, changed files, terminals, processes, and agent activity
- resources for daemon state
- prompts for common Arbor workflows

## Remote Auth

`arbor-mcp` forwards auth to `arbor-httpd`; it does not add a second auth layer.

For the canonical standalone reference, see [../mcp.md](../mcp.md).
