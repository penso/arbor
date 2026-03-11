# Arbor Documentation

Arbor is a native Rust + GPUI app for agentic coding across local repositories, parallel worktrees, embedded terminals, remote outposts, and daemon-backed automation.

This book has two goals:

- explain the current Arbor feature surface in one place
- provide a practical QA checklist for the Tier 1 workflow features added on this branch

## What Arbor Covers

Arbor currently includes:

- multi-repository and multi-worktree navigation
- embedded and daemon-backed terminal sessions
- side-by-side diffs, changed files, and file-tree browsing
- GitHub PR visibility and agent activity state tracking
- repo-local automation through `arbor.toml`
- remote daemon access, remote outposts, and MCP integration
- command palette, theme picker, notifications, and UI settings

## Read This Book In Order If You Are New

1. [Getting Started](./getting-started.md)
2. [Workspace Model](./workspace-model.md)
3. [Repositories and Worktrees](./repositories-and-worktrees.md)
4. [Terminals, Diffs, and Files](./terminals-diffs-and-files.md)
5. [GitHub, Agents, and Git Actions](./github-agents-and-git.md)
6. [Automation and Repo Config](./automation-and-repo-config.md)
7. [Remote Access, Daemon, and MCP](./remote-daemon-and-mcp.md)
8. [Themes, Settings, and Notifications](./themes-settings-and-notifications.md)

If you are validating this branch, go straight to [QA Checklist](./qa-checklist.md).
