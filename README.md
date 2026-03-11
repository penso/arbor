<p align="center">
  <a href="assets/screenshot.png">
    <img src="assets/screenshot.png" alt="Arbor UI screenshot" width="1100" />
  </a>
</p>

# Arbor

[![CI](https://github.com/penso/arbor/actions/workflows/ci.yml/badge.svg)](https://github.com/penso/arbor/actions/workflows/ci.yml)
[![Rust Nightly](https://img.shields.io/badge/rust-nightly--2025--11--30-orange?logo=rust)](https://rust-lang.org)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/penso/arbor?label=release)](https://github.com/penso/arbor/releases)
[![macOS](https://img.shields.io/badge/macOS-supported-brightgreen)](#install)
[![Linux](https://img.shields.io/badge/Linux-supported-brightgreen)](#install)
[![Windows](https://img.shields.io/badge/Windows-supported-brightgreen)](#install)

Arbor is a **fully native app for agentic coding** built with Rust and [GPUI](https://gpui.rs).
It gives you one place to manage repositories, parallel worktrees, embedded terminals, diffs, AI coding agent activity, and a daemon-backed MCP server.

## Why Arbor

- Fully native desktop app (UI + terminal stack, Rust + GPUI), optimized for long-running local workflows
- One workspace for worktrees, terminals, file changes, and git actions
- Built for parallel coding sessions across local repos and remote outposts

## Core Capabilities

### Worktree Management
- List, create, and delete worktrees across multiple repositories
- Delete confirmation with unpushed commit detection
- Optional branch cleanup on worktree deletion
- Worktree navigation history (back/forward)
- Last git activity timestamp per worktree

### Embedded Terminal
- Built-in PTY terminal with truecolor and `xterm-256color` support
- Multiple terminal tabs per worktree
- Alternative backends: Alacritty, Ghostty
- Persistent daemon-based sessions (survive app restarts)
- Session attach/detach and signals (interrupt/terminate/kill)

### Diff and Changes
- Side-by-side diff display with addition/deletion line counts
- Changed file listing per worktree
- File tree browsing with directory expand/collapse
- Multi-tab diff sessions

### AI Agent Visibility
- Detects running coding agents: Claude Code, Codex, OpenCode
- Working/waiting state indicators with color-coded dots
- Real-time updates over WebSocket streaming

### MCP Server
- Dedicated `arbor-mcp` binary backed by Arbor's daemon API
- Structured MCP tools for repositories, worktrees, terminals, processes, and agent activity
- MCP resources for daemon snapshots and prompts for common Arbor workflows
- Supports `ARBOR_DAEMON_URL` and `ARBOR_DAEMON_AUTH_TOKEN` for remote authenticated daemons

### Remote Outposts
- Create and manage remote worktrees over SSH
- Multi-host configuration with custom ports and identity files
- Mosh support for better connectivity
- Remote terminal sessions via `arbor-httpd`
- Outpost status tracking (available, unreachable, provisioning)

### GitHub + UI + Config
- Automatic PR detection and linking per worktree
- Git actions in the UI: commit, push
- Three-pane layout (repositories, terminal, changes/file tree)
- Resizable panes, collapsible sidebar, desktop notifications
- Twenty-five themes, including Omarchy defaults
- TOML config at `~/.config/arbor/config.toml` with hot reload

## Install

### Homebrew (macOS)

```bash
brew install penso/arbor/arbor
```

### Prebuilt Binaries

Download the latest build from [Releases](https://github.com/penso/arbor/releases).

### Quick Start from Source

```bash
git clone https://github.com/penso/arbor
cd arbor
just run
```

To run the MCP server against a local dev daemon:

```bash
just run-mcp
```

## Documentation

Full documentation is available at [penso.github.io/arbor/docs](https://penso.github.io/arbor/docs/).

## Crates

| Crate | Description |
|-------|-------------|
| `arbor-daemon-client` | Typed client and shared API DTOs for `arbor-httpd` |
| `arbor-core` | Worktree primitives, change detection, agent hooks |
| `arbor-gui` | GPUI desktop app (`arbor` binary) |
| `arbor-httpd` | Remote HTTP daemon (`arbor-httpd` binary) |
| `arbor-mcp` | MCP server exposing Arbor via stdio (`arbor-mcp` binary) |
| `arbor-web-ui` | TypeScript dashboard assets + helper crate |

## MCP

Arbor ships a dedicated `arbor-mcp` binary from the `arbor-mcp` crate. The stdio server is enabled by the crate's default `stdio-server` feature and talks to `arbor-httpd`, so the daemon must be reachable first.

Enable it in a normal build:

```bash
cargo build -p arbor-mcp
```

Environment variables:

- `ARBOR_DAEMON_URL` overrides the daemon base URL. Default: `http://127.0.0.1:8787`
- `ARBOR_DAEMON_AUTH_TOKEN` sends a bearer token for remote authenticated daemons

Remote access:

1. On the daemon host, set `[daemon] auth_token = "your-secret"` in `~/.config/arbor/config.toml`.
2. Start `arbor-httpd`. When an auth token is configured, Arbor binds remotely by default on `0.0.0.0:8787` unless `ARBOR_HTTPD_BIND` overrides it.
3. Point `arbor-mcp` at that daemon with `ARBOR_DAEMON_URL=http://HOST:8787`.
4. Pass the same secret with `ARBOR_DAEMON_AUTH_TOKEN=your-secret`.

Loopback requests are allowed without a token. Non-loopback requests require `Authorization: Bearer <token>`.

Example client config:

```json
{
  "mcpServers": {
    "arbor": {
      "command": "/path/to/arbor-mcp",
      "env": {
        "ARBOR_DAEMON_URL": "http://127.0.0.1:8787"
      }
    }
  }
}
```

The `arbor-mcp` binary is feature-gated. To disable the stdio server binary in a build, use:

```bash
cargo build -p arbor-mcp --no-default-features
```

See [docs/mcp.md](docs/mcp.md) for the full MCP setup guide.

## Building from Source

### Prerequisites

- **Rust nightly** — the project uses `nightly-2025-11-30` (install via [rustup](https://rustup.rs/))
- **[just](https://github.com/casey/just)** — task runner
- **[CaskaydiaMono Nerd Font](https://www.nerdfonts.com/)** — icons in the UI use Nerd Font glyphs

#### macOS

```
just setup-macos
```

Or manually:

```
xcode-select --install
xcodebuild -downloadComponent MetalToolchain
brew install --cask font-caskaydia-mono-nerd-font
```

#### Linux (Debian/Ubuntu)

```
just setup-linux
```

Or manually:

```
sudo apt-get install -y libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev
```

Then install the [CaskaydiaMono Nerd Font](https://www.nerdfonts.com/font-downloads) to `~/.local/share/fonts/`.

## Similar Tools

- [Superset](https://superset.sh) — terminal-based worktree manager
- [Jean](https://jean.build) — dev environment for AI agents with isolated worktrees and chat sessions
- [Conductor](https://www.conductor.build) — macOS app to orchestrate multiple AI coding agents in parallel worktrees

## Acknowledgements

Thanks to [Zed](https://zed.dev) for building and open-sourcing [GPUI](https://gpui.rs), the GPU-accelerated UI framework that powers Arbor.

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=penso/arbor&type=date&legend=top-left)](https://www.star-history.com/#penso/arbor&type=date&legend=top-left)
