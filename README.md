<p align="center">
  <a href="assets/screenshot.png">
    <img src="assets/screenshot.png" alt="Arbor UI screenshot" width="1100" />
  </a>
</p>

# Arbor

[![CI](https://github.com/penso/arbor/actions/workflows/ci.yml/badge.svg)](https://github.com/penso/arbor/actions/workflows/ci.yml)

Arbor is a desktop Git worktree manager built with Rust and [GPUI](https://gpui.rs).

## Features

### Git Worktree Management
- List, create, and delete worktrees across multiple repositories
- Delete confirmation modal with unpushed commits detection
- Optional branch cleanup on worktree deletion
- Worktree navigation history (back/forward)
- Last git activity timestamp per worktree

### Embedded Terminal
- Built-in PTY terminal with truecolor and xterm-256color support
- Multiple terminal tabs per worktree
- Alternative backends: Alacritty, Ghostty
- Persistent daemon-based sessions (survive app restarts)
- Session attach/detach, signals (interrupt/terminate/kill)

### Diff & Changes Viewer
- Side-by-side diff display with addition/deletion line counts
- Changed file listing per worktree
- File tree browsing with directory expand/collapse
- Multi-tab diff sessions

### GitHub Integration
- Automatic PR detection and linking per worktree
- Repository avatars from GitHub
- Git actions from the UI: commit, push

### Remote Outposts
- Create and manage remote worktrees over SSH
- Multi-host configuration with custom ports and identity files
- Mosh support for better connectivity
- Remote terminal sessions via `arbor-httpd` daemon
- Outpost status tracking (available, unreachable, provisioning)

### AI Agent Activity
- Detects running coding agents (Claude Code, Codex, OpenCode)
- Working/waiting state indicators with color-coded dots
- Real-time updates via WebSocket streaming

### UI
- Three-pane layout: repository sidebar, terminal center, changes/file tree right
- Resizable panes with drag handles and collapsible sidebar
- Three themes: One Dark, Ayu Dark, Gruvbox Dark
- Desktop notifications for terminal events
- Keyboard-driven modal dialogs

### Configuration
- TOML config at `~/.config/arbor/config.toml`
- Configurable: terminal backend, theme, daemon URL, notifications, remote hosts
- Hot reload on config file changes

## Crates

| Crate | Description |
|-------|-------------|
| `arbor-core` | Worktree primitives, change detection, agent hooks |
| `arbor-gui` | GPUI desktop app (`arbor` binary) |
| `arbor-httpd` | Remote HTTP daemon (`arbor-httpd` binary) |
| `arbor-web-ui` | TypeScript dashboard assets + helper crate |

## Development

Use `just` as the task runner.

- `just format`
- `just format-check`
- `just lint`
- `just test`
- `just run`
- `just run-httpd`

## Remote Access

Run the remote daemon:

- `just run-httpd`
- or `ARBOR_HTTPD_BIND=0.0.0.0:8787 cargo +nightly-2025-11-30 run -p arbor-httpd`

HTTP API:

- `GET /api/v1/health`
- `GET /api/v1/repositories`
- `GET /api/v1/worktrees`
- `GET /api/v1/terminals`
- `POST /api/v1/terminals`
- `GET /api/v1/terminals/:session_id/snapshot`
- `POST /api/v1/terminals/:session_id/write`
- `POST /api/v1/terminals/:session_id/resize`
- `POST /api/v1/terminals/:session_id/signal`
- `POST /api/v1/terminals/:session_id/detach`
- `DELETE /api/v1/terminals/:session_id`
- `GET /api/v1/terminals/:session_id/ws`

If `crates/arbor-web-ui/app/dist/index.html` is missing, the daemon attempts an on-demand build with `npm`.

Desktop daemon URL override:

- `~/.config/arbor/config.toml`
- `daemon_url = "http://127.0.0.1:8787"`

## CI

GitHub Actions runs format, lint, and test checks on pushes to `main` and pull requests:

- Workflow: [`CI`](https://github.com/penso/arbor/actions/workflows/ci.yml)

On pushes to `main`, CI also runs a cross-platform build matrix for:

- Linux (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`)
- macOS (`aarch64-apple-darwin`, `x86_64-apple-darwin`)
- Windows (`x86_64-pc-windows-msvc`)

## Releases

Push a tag in `YYYYMMDD.NN` format (example: `20260301.01`) to trigger an automated release:

- Workflow: [`Release`](https://github.com/penso/arbor/actions/workflows/release.yml)
- Artifacts:
  - macOS `.app` bundle (zipped, universal2, with `Info.plist` and app icon)
  - Linux `tar.gz` bundles (`x86_64` and `aarch64`)
  - Windows `.zip` bundle (`x86_64`)

## Changelog

This repo uses [`git-cliff`](https://git-cliff.org/) for changelog generation.

- `just changelog`: generate/update `CHANGELOG.md`
- `just changelog-unreleased`: preview unreleased entries in stdout
- `just changelog-release <version>`: preview a release section tagged as `v<version>`

Config lives in `cliff.toml`.
