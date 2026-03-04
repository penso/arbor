# AGENTS.md

This file defines how coding agents should behave in this repository.

## Priorities

1. Keep code simple, explicit, and maintainable.
2. Fix root causes, avoid temporary band-aids.
3. Preserve user changes, never revert unrelated edits.

## Workflow

1. Read this file at task start.
2. Prefer `just` recipes for common tasks.
3. Before handoff, run relevant checks for touched code.

## Commands

- Format: `just format`
- Format check: `just format-check`
- Lint: `just lint`
- Test: `just test`
- Run app: `just run`

## Rust Rules

- Do not use `unwrap()` or `expect()` in non-test code.
- Use clear error handling with typed errors (`thiserror`/`anyhow` where appropriate).
- Keep modules focused and delete dead code instead of leaving it around.

## Git Rules

- Treat `git status` / `git diff` as read-only context.
- Do not run destructive git commands.
- Do not amend commits unless explicitly asked.
- Only create commits when the user asks.

## Changelog

- Use `git-cliff` for changelog generation.
- Config file: `cliff.toml`
- Commands:
  - `just changelog`
  - `just changelog-unreleased`
  - `just changelog-release <version>`

