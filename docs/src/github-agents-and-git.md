# GitHub, Agents, and Git Actions

## GitHub Integration

Arbor surfaces GitHub information in the UI, including:

- PR number and link per worktree
- review/check state in hover detail
- GitHub auth state in the top bar

## Agent Visibility

Arbor tracks coding-agent activity and shows:

- working / waiting state
- per-worktree state indicators
- real-time updates through daemon-backed activity streams

## Tier 1 Notification Routing

This branch adds notification behavior for agent and process lifecycle events:

- native desktop notifications in the GUI for relevant agent state transitions
- daemon-side webhook POST delivery for `agent_finished`
- daemon-side webhook POST delivery for `agent_error`

Repo-level notification config:

```toml
[notifications]
desktop = true
events = ["agent_finished", "agent_error"]
webhook_urls = ["https://example.com/hook"]
```

## Git Actions

Arbor includes in-UI git actions for:

- commit
- push
- PR visibility

Tier 1 added a richer commit flow:

- editable commit message in a modal
- fallback "Use Default" message path
- AI-generated commit message path for supported presets
