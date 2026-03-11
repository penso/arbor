# Workspace Model

Arbor is organized around repository groups and worktrees.

## Repository Groups

A repository group is the primary container shown in the left pane. Arbor can manage multiple repositories at once, each with:

- a root path
- zero or more linked worktrees
- optional GitHub metadata
- repo-local presets and automation config

## Worktree-Centered UI

Most of the UI updates around the currently selected worktree:

- terminal tabs belong to the selected worktree
- changed files and diffs are scoped to the selected worktree
- PR status, agent state, and notifications are derived from that worktree

## Navigation Patterns

Arbor supports:

- direct selection from the sidebar
- back / forward history between worktrees
- keyboard-driven action switching through the command palette

## Persistence

Arbor persists window and UI state such as pane sizes and visibility. The daemon separately persists terminal session metadata for reconnect and restore flows.
