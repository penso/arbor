# QA Checklist

This checklist is focused on the branch-added Tier 1 workflow features plus the related keyboard/UI follow-ups.

## QA-01: `arbor.toml` Repo Config Loads

Purpose: validate repo-level presets, processes, scripts, and notifications parse correctly.

Setup:

```toml
[[presets]]
name = "Review"
icon = "R"
command = "echo review"

[[processes]]
name = "web"
command = "sleep 30"
auto_start = false

[scripts]
setup = ["echo setup-ran > .arbor-setup.txt"]
teardown = ["echo teardown-ran > .arbor-teardown.txt"]

[notifications]
desktop = true
events = ["agent_finished", "agent_error"]
webhook_urls = ["http://127.0.0.1:9999/hook"]
```

Steps:

1. Add the file to a tracked repository root.
2. Launch Arbor and select that repository.
3. Open repo presets and process views.

Expected:

- the repo preset appears
- the configured process appears
- no config parse errors are shown

## QA-02: Worktree Setup Script Runs

Purpose: validate setup hooks execute after worktree creation.

Steps:

1. Configure a `[scripts].setup` command that writes a marker file into the new worktree.
2. Create a local worktree from Arbor.
3. Open the created worktree in Finder or a shell.

Expected:

- the worktree is created successfully
- the setup marker file exists
- no rollback occurs

## QA-03: Worktree Setup Failure Rolls Back

Purpose: validate failed setup does not leave a broken worktree behind.

Setup:

```toml
[scripts]
setup = ["exit 1"]
```

Steps:

1. Create a new worktree.
2. Observe the result in Arbor and on disk.

Expected:

- Arbor reports failure
- the new worktree directory is removed
- if a branch was created for that worktree, it is cleaned up

## QA-04: Teardown Script Runs Before Delete

Purpose: validate teardown executes before worktree removal.

Setup:

```toml
[scripts]
teardown = ["touch ../teardown-sentinel.txt"]
```

Steps:

1. Create a worktree.
2. Delete that worktree from Arbor.

Expected:

- the sentinel file is created outside the worktree
- the worktree is deleted after the script runs

## QA-05: Desktop Notifications for Agent Completion

Purpose: validate GUI-side native notifications.

Steps:

1. Enable desktop notifications in settings.
2. Trigger a worktree’s agent state from working to waiting.

Expected:

- a desktop notification is shown once for the relevant transition

## QA-06: Webhook Notification for `agent_finished`

Purpose: validate daemon-side webhook delivery on agent completion.

Steps:

1. Configure `[notifications].webhook_urls` to point at a local test server.
2. Trigger an agent stop event for a worktree.
3. Capture the webhook request body.

Expected:

- a POST request is sent
- payload includes `event = "agent_finished"`
- payload includes repo/worktree/cwd context

## QA-07: Webhook Notification for `agent_error`

Purpose: validate daemon-side webhook delivery for process crash/error.

Steps:

1. Configure a managed process that exits non-zero.
2. Start it through Arbor or the daemon.
3. Capture webhook traffic.

Expected:

- a POST request is sent
- payload includes `event = "agent_error"`
- payload includes process name / command / exit code

## QA-08: Command Palette Search Coverage

Purpose: validate all intended sources are searchable.

Steps:

1. Open `Cmd+K`.
2. Search for:
   - a built-in action
   - a repository label
   - a worktree label or branch
   - an agent preset
   - a repo preset
   - a task template name

Expected:

- each item type appears in search results
- selecting a result runs the correct action

## QA-09: Command Palette Keyboard Navigation

Purpose: validate keyboard-first behavior.

Steps:

1. Open `Cmd+K`.
2. Use Up/Down repeatedly through a long result list.
3. Press `Escape`.
4. Reopen and press `Enter` on a selected item.

Expected:

- selection moves correctly
- selected item stays visible
- palette dismisses on `Escape`
- selected item executes on `Enter`

## QA-10: Command Palette Mouse Selection Only On Movement

Purpose: validate opening the palette under a stationary cursor does not immediately change selection.

Steps:

1. Rest the mouse over an area where a result row will appear.
2. Open `Cmd+K`.
3. Do not move the mouse.
4. Then move the mouse onto a different row.

Expected:

- initial selection remains keyboard/default selection on open
- selection changes only after actual mouse movement

## QA-11: Command Palette Overflow Indicator and Icons

Purpose: validate the visible affordances added for large result sets.

Steps:

1. Open `Cmd+K` with enough items to overflow.
2. Inspect the right edge and left side of the list.

Expected:

- a visible scrollbar/overflow indicator is shown
- a result count is visible
- each row has an icon appropriate to its action type

## QA-12: Theme Picker Keyboard Support

Purpose: validate full keyboard control of the theme modal.

Steps:

1. Open the theme picker from `Cmd+K`.
2. Move with Left/Right/Up/Down.
3. Press `Enter` to apply a theme.
4. Reopen and press `Escape`.

Expected:

- selection moves in grid order
- selected card is visibly highlighted
- `Enter` applies the highlighted theme
- `Escape` dismisses the modal

## QA-13: Commit Modal Default and AI Message Flow

Purpose: validate the enhanced commit experience.

Steps:

1. Make a small change in a worktree.
2. Open commit action.
3. Use `Use Default`.
4. Reopen commit and use AI generation with a supported preset.
5. Edit the generated text before submitting.

Expected:

- default commit text is populated correctly
- AI generation returns a message or clean fallback behavior
- edited message is respected for the final commit

## QA-14: Repo-Local Task Templates

Purpose: validate `.arbor/tasks/*.md` loading and execution.

Setup:

```text
.arbor/tasks/review.md
```

Example content:

```md
# Review PR

Agent: Codex

Review the current branch and summarize the highest-risk changes.
```

Steps:

1. Add one or more task template files.
2. Open `Cmd+K`.
3. Search by task name or prompt text.
4. Launch the task.

Expected:

- tasks appear in palette results
- the configured/default agent is selected
- the launched task passes the template prompt into the agent flow

## QA-15: UI State Persistence

Purpose: validate supporting config/state changes.

Steps:

1. Resize panes and toggle sidebar visibility.
2. Close Arbor.
3. Reopen Arbor.

Expected:

- pane widths are restored
- sidebar visibility is restored

## QA-16: Docs Build

Purpose: validate the new HTML docs pipeline.

Steps:

1. Run `just docs-build`.
2. Open `docs/book/index.html`.

Expected:

- the docs build succeeds
- the book includes product guide, QA checklist, and reference sections
