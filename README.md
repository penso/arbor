# Arbor

Arbor is a Rust workspace for building a desktop worktree manager inspired by tools like Superset and Conductor.

## Workspace Layout

- `crates/arbor-core`: Git worktree primitives (list/add/remove, porcelain parsing)
- `crates/arbor-gui`: GPUI desktop application (`arbor` binary)

## Development

Use `just` as the task runner:

- `just format`
- `just format-check`
- `just lint`
- `just test`
- `just run`

## Changelog

This repo uses [`git-cliff`](https://git-cliff.org/) for changelog generation.

- `just changelog`: generate/update `CHANGELOG.md`
- `just changelog-unreleased`: preview unreleased entries in stdout
- `just changelog-release <version>`: preview a release section tagged as `v<version>`

Config lives in `cliff.toml`.

## Notes

The GUI currently focuses on repository discovery and worktree listing. Add/remove flows are implemented in `arbor-core` and can be wired into UI actions next.
