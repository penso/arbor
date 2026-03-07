# CLAUDE.md

This file provides instructions for Claude Code when working in this repository.

## Priorities

1. Keep code simple, explicit, and maintainable.
2. Fix root causes, avoid temporary band-aids.
3. Preserve user changes, never revert unrelated edits.

## Toolchain

This project uses a pinned Rust nightly toolchain: `nightly-2025-11-30`. All commands go through `just` recipes which apply the correct toolchain automatically.

## Commands

- Format: `just format`
- Format check: `just format-check`
- Lint (clippy): `just lint`
- Test: `just test`
- Full CI suite: `just ci`
- Run app: `just run`
- Run HTTP daemon: `just run-httpd`

## Before Committing / Pushing

Always run these checks before committing and fix any issues:

1. `just format` — auto-fix formatting
2. `just lint` — must pass with zero warnings (`-D warnings`)
3. `just test` — all tests must pass

If any check fails, fix the issue, then commit the fix.

## Rust Rules

- Do not use `unwrap()` or `expect()` in non-test code.
- Use clear error handling with typed errors (`thiserror`/`anyhow` where appropriate).
- Keep modules focused and delete dead code instead of leaving it around.
- Collapse nested `if` / `if let` statements when possible (clippy `collapsible_if`).

## Git Rules

- Treat `git status` / `git diff` as read-only context.
- Do not run destructive git commands.
- Do not amend commits unless explicitly asked.
- Only create commits when the user asks.

## Changelog

- Use `git-cliff` for changelog generation (config: `cliff.toml`).
- `just changelog` / `just changelog-unreleased` / `just changelog-release <version>`

## Project Structure

| Crate | Description |
|-------|-------------|
| `arbor-core` | Worktree primitives, change detection, agent hooks |
| `arbor-gui` | GPUI desktop app (`arbor` binary) |
| `arbor-httpd` | Remote HTTP daemon (`arbor-httpd` binary) |
| `arbor-web-ui` | TypeScript dashboard assets + helper crate |
