# Repositories and Worktrees

## Repository Management

Arbor can track multiple repositories and list all linked worktrees under each one.

Repository-level capabilities include:

- add and remove repositories
- collapse or expand repository groups
- identify the primary checkout
- resolve GitHub repo slug and avatar when available

## Worktree Management

Worktree capabilities include:

- create local worktrees from the create modal
- delete non-primary worktrees
- optionally delete the branch during worktree removal
- show last git activity and PR metadata
- maintain navigation history across worktrees

## Tier 1 Additions

This branch adds repo-level lifecycle automation during worktree create and delete:

- setup scripts run after a worktree is created
- teardown scripts run before a worktree is deleted
- if setup fails, Arbor rolls back the created worktree

The repo config lives in `<repo>/arbor.toml`.

Example:

```toml
[scripts]
setup = ["cp .env.example .env"]
teardown = ["rm -f .env"]
```
