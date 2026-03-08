<p align="center">
  <a href="assets/screenshot.png">
    <img src="assets/screenshot.png" alt="Arbor UI screenshot" width="1100" />
  </a>
</p>

# Arbor

[![CI](https://github.com/penso/arbor/actions/workflows/ci.yml/badge.svg)](https://github.com/penso/arbor/actions/workflows/ci.yml)

Arbor is a **fully native app for agentic coding** built with Rust and [GPUI](https://gpui.rs).
It gives you one place to manage repositories, parallel worktrees, embedded terminals, diffs, and AI coding agent activity.

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

## Website (Static)

This repository includes a standalone static product site in `website/`.
Live site: [https://penso.github.io/arbor/](https://penso.github.io/arbor/)

Feature screenshots:

- [Worktrees](website/images/features/worktrees.png)
- [Terminal](website/images/features/terminal.png)
- [Diff](website/images/features/diff.png)
- [Agent Activity](website/images/features/agent-activity.png)
- [Remote Outposts](website/images/features/remote-outposts.png)
- [Themes](website/images/features/themes.png)

Local preview:

```bash
cd website
python3 -m http.server 4173
```

## Crates

| Crate | Description |
|-------|-------------|
| `arbor-core` | Worktree primitives, change detection, agent hooks |
| `arbor-gui` | GPUI desktop app (`arbor` binary) |
| `arbor-httpd` | Remote HTTP daemon (`arbor-httpd` binary) |
| `arbor-web-ui` | TypeScript dashboard assets + helper crate |

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

### Build & Run

Use `just` as the task runner.

- `just setup-macos` / `just setup-linux` — install dependencies (one-time)
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
- or `Arbor --daemon --bind 0.0.0.0:8787` (same as `arbor --daemon --bind ...` in packages that install a lowercase launcher)

From the GUI, **Connect to Host...** accepts:

- `http://IP:port/` for direct HTTP access
- `ssh://IP/` (or `ssh://user@IP:22/`) to create a local SSH tunnel and route daemon traffic securely over SSH

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

## Similar Tools

- [Superset](https://superset.sh) — terminal-based worktree manager
- [Jean](https://jean.build) — dev environment for AI agents with isolated worktrees and chat sessions
- [Conductor](https://www.conductor.build) — macOS app to orchestrate multiple AI coding agents in parallel worktrees

## Acknowledgements

Thanks to [Zed](https://zed.dev) for building and open-sourcing [GPUI](https://gpui.rs), the GPU-accelerated UI framework that powers Arbor.

## Changelog

This repo uses [`git-cliff`](https://git-cliff.org/) for changelog generation.

- `just changelog`: generate/update `CHANGELOG.md`
- `just changelog-unreleased`: preview unreleased entries in stdout
- `just changelog-release <version>`: preview a release section tagged as `v<version>`

Config lives in `cliff.toml`.

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=penso/arbor&type=date&legend=top-left)](https://www.star-history.com/#penso/arbor&type=date&legend=top-left)
