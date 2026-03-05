<p align="center">
  <a href="assets/screenshot.png">
    <img src="assets/screenshot.png" alt="Arbor UI screenshot" width="1100" />
  </a>
</p>

# Arbor

[![CI](https://github.com/penso/arbor/actions/workflows/ci.yml/badge.svg)](https://github.com/penso/arbor/actions/workflows/ci.yml)

Arbor is a Rust workspace for a desktop Git worktree manager inspired by tools like Superset and Conductor.

## What Arbor Includes

- `arbor-core`: worktree primitives (list/add/remove, porcelain parsing)
- `arbor-gui`: GPUI desktop app (`arbor` binary)
- `arbor-httpd`: remote HTTP daemon (`arbor-httpd` binary)
- `arbor-web-ui`: TypeScript dashboard assets + helper crate

## Workspace Layout

- `crates/arbor-core`
- `crates/arbor-gui`
- `crates/arbor-httpd`
- `crates/arbor-web-ui`

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
