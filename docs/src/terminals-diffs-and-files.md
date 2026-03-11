# Terminals, Diffs, and Files

## Terminal Sessions

Arbor supports:

- embedded terminal sessions
- daemon-backed local sessions that survive GUI restarts
- alternative terminal launch backends such as Alacritty and Ghostty
- multiple tabs per worktree
- signal handling for interrupt / terminate / kill

## Diff and Change Inspection

For each worktree, Arbor can show:

- changed file list
- additions and deletions per file
- side-by-side diff view
- multiple diff tabs

## File Tree

The right pane can switch between:

- changed files
- repository file tree

The file tree supports:

- directory expand / collapse
- keyboard-friendly browsing of selected entries
- opening file-view tabs

## Command Palette Interaction

The command palette now supports long lists better:

- selection stays visible while moving with the keyboard
- `Escape` dismisses reliably
- the list shows overflow indication and result count
- commands have left-side icons for scanning
