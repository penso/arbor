# Getting Started

## Install and Run

From source:

```bash
git clone https://github.com/penso/arbor
cd arbor
just run
```

Useful local commands:

- `just run` starts `arbor-httpd` and the native GUI together
- `just run-httpd` starts only the daemon
- `just run-mcp` starts the daemon and MCP server together
- `just docs-build` builds this documentation book into `docs/book`

## Main Concepts

- Repository: a tracked git root
- Worktree: one checkout belonging to a repository
- Terminal session: an attached shell for a worktree
- Outpost: a remote worktree target over SSH / daemon access
- Daemon: `arbor-httpd`, which backs terminal persistence, remote access, and the web / MCP surface

## Core User Flows

- add a repository, then create or select a worktree
- open a terminal tab in that worktree
- inspect changed files and diffs
- commit or push from the GUI
- launch agent presets or task templates
- use `Cmd+K` to jump to actions, repos, worktrees, presets, and tasks

## Configuration Locations

- app config: `~/.config/arbor/config.toml`
- repo config: `<repo>/arbor.toml`
- repo-local tasks: `<repo>/.arbor/tasks/*.md`
- daemon session store: `~/.arbor/daemon/sessions.json`
