# Automation and Repo Config

## `arbor.toml`

Repo-local automation is configured with `<repo>/arbor.toml`.

Supported areas include:

- `[[presets]]` for repo-specific commands
- `[[processes]]` for managed background processes
- `[scripts]` for worktree setup and teardown hooks
- `[notifications]` for desktop/webhook event routing

Example:

```toml
[[presets]]
name = "Review"
icon = "R"
command = "claude --dangerously-skip-permissions"

[[processes]]
name = "web"
command = "npm run dev"
auto_start = true

[scripts]
setup = ["cp .env.example .env"]
teardown = ["rm -f .env"]

[notifications]
desktop = true
events = ["agent_finished", "agent_error"]
webhook_urls = ["https://example.com/hook"]
```

## Repo Presets

Repo presets appear in the UI and command palette. They let a repository define named commands without editing the global Arbor config.

## Managed Processes

Processes configured in `arbor.toml` can be started, stopped, restarted, and observed through daemon APIs and process status streams.

## Task Templates

Tier 1 adds repo-local task templates loaded from:

```text
.arbor/tasks/*.md
```

These templates are searchable from `Cmd+K` and can launch a prompt with the selected agent preset.

## Command Palette

The command palette can search and execute:

- built-in actions
- repositories
- worktrees
- agent presets
- repo presets
- task templates
