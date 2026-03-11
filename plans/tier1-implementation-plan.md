# Arbor Tier 1 Implementation Plan

Date: 2026-03-11
Source docs reviewed: `plans/arbor-feature-roadmap.md` plus the comparison notes in `plans/`

## Goal

Implement the Tier 1 roadmap items without duplicating config parsing, forking behavior between GUI and daemon, or hard-coding provider-specific logic into UI event handlers.

## Current State Audit

### 1. Setup/Teardown Scripts per Worktree
Status: not implemented.

Relevant code:
- Worktree creation/removal lives in `crates/arbor-core/src/worktree.rs`.
- Native GUI creation/removal flows live in `crates/arbor-gui/src/main.rs`.
- HTTP creation/removal flows live in `crates/arbor-httpd/src/main.rs`.
- Repo-local config is already read from `arbor.toml`, not `.arbor.toml`, in `crates/arbor-gui/src/app_config.rs`.

Implication:
- We should extend existing `arbor.toml` support, not introduce a second repo config filename.

### 2. Command Palette
Status: not implemented.

Relevant code:
- Actions are already centralized via `actions!(arbor, [...])` in `crates/arbor-gui/src/main.rs`.
- There is existing modal/keybinding infrastructure in the same file.

Implication:
- This can be added as a thin UI layer if actions and targets are normalized first.

### 3. AI Commit Messages
Status: partially adjacent, but not implemented.

Relevant code:
- Commit currently auto-generates a heuristic subject/body in `run_git_commit_for_worktree()` in `crates/arbor-gui/src/main.rs`.
- The changes pane only offers a direct `Commit` action; there is no commit editor or prompt step.

Implication:
- This is not just “add an AI button”; it requires a commit flow with editable message state.

### 4. Notification Routing
Status: partial.

Already present:
- Desktop notification service exists in `crates/arbor-gui/src/notifications.rs`.
- GUI can already send notifications when background terminal sessions complete/fail.
- Agent activity state already flows through `/api/v1/agent/notify` and WebSocket updates in `crates/arbor-httpd/src/main.rs`.

Missing:
- Notifications on agent `working -> waiting` transitions.
- Outbound webhook delivery and per-event routing.
- Shared config for notification channels and filters.

### 5. CI Status per Worktree
Status: largely implemented in the native GUI.

Already present:
- PR details and check rollups are fetched via `gh pr view` in `crates/arbor-gui/src/github_service.rs`.
- Worktree cards already render aggregated check status, per-check details, review decision, and additions/deletions in `crates/arbor-gui/src/main.rs`.

Remaining gap:
- The roadmap should be treated as “complete for native GUI, optional parity work for daemon/web UI/docs”.

### 6. Task Templates / Magic Commands
Status: not implemented, but there is a strong precursor.

Relevant code:
- Repo-local command presets already exist in `arbor.toml` via `[[presets]]`.
- Presets can already launch terminal commands in the GUI.
- Daemon terminal creation already supports an initial `command`.

Implication:
- Tier 1 should build on repo presets and daemon command launching instead of inventing a second unrelated launch mechanism.

## Architectural Decisions

### A. Move repo config parsing out of `arbor-gui`

Reason:
- Tier 1 features need repo config in both GUI and daemon.
- `arbor-httpd` already parses `arbor.toml` separately for `[[processes]]`.
- Keeping separate parsers will drift quickly once scripts, tasks, and notifications land.

Plan:
- Add a shared repo-config module in `crates/arbor-core`, for example `repo_config.rs`.
- Make it the single parser for:
  - `[[presets]]`
  - `[[processes]]`
  - `[scripts]`
  - `[notifications]`
  - task metadata if we choose to mirror any in TOML
- Keep the file name `arbor.toml`.

### B. Add one shared “prompt runner” abstraction

Reason:
- AI commit messages and task templates both need “take text input, run it through a provider command, capture text output”.
- Reimplementing that twice will create provider-specific edge cases in unrelated features.

Plan:
- Add a small service in `arbor-gui` first, behind a trait or focused module.
- Input: selected agent preset, prompt text, working directory, optional extra context.
- Output: captured stdout string or typed error.
- Start with best-effort provider support for configured presets, not every CLI permutation.

### C. Native-first UI, daemon-backed behavior where it matters

Reason:
- Tier 1 items are mostly native-GUI features today.
- The daemon must still own behavior that needs to fire when the GUI is closed or when web clients are attached.

Plan:
- Scripts: invoke from both GUI and HTTP worktree mutation flows.
- Webhooks: emit from `arbor-httpd`.
- Desktop notifications and command palette: native GUI first.
- CI status: leave as-is unless parity is explicitly required next.

## Recommended Delivery Order

## Phase 1: Shared config + worktree lifecycle hooks

Deliverables:
- Shared `arbor.toml` parser in `arbor-core`.
- New config schema:
  - `[scripts]`
  - `setup = ["..."]`
  - `teardown = ["..."]`
- Hook execution helper with explicit environment:
  - `ARBOR_WORKTREE_PATH`
  - `ARBOR_REPO_PATH`
  - `ARBOR_BRANCH`

Execution points:
- Native GUI worktree creation path.
- Native GUI worktree deletion path.
- HTTP create/delete endpoints.

Rules:
- Setup runs only after successful worktree creation.
- Teardown runs before removal if the path still exists and after branch resolution.
- Script failure should fail the overall operation by default.
- Capture stdout/stderr in error text for diagnosis.

Tests:
- Config parsing tests in `arbor-core`.
- Worktree integration test covering setup success/failure.
- HTTP handler test for create/delete with scripts enabled.

Why first:
- It establishes the shared config foundation required by later Tier 1 work.

## Phase 2: Notification routing

Deliverables:
- `[notifications]` config in `arbor.toml`.
- GUI desktop notifications for agent state transitions, specifically `working -> waiting`.
- Daemon webhook router for selected events.

Initial event set:
- `agent_finished`
- `agent_error`
- `agent_stuck` reserved but not implemented in Tier 1

Recommended schema:
- `[notifications]`
- `desktop = true`
- `events = ["agent_finished", "agent_error"]`
- `webhook_urls = ["https://..."]`

Behavior split:
- GUI:
  - detect transition using previous vs current `AgentState` in `apply_agent_ws_update`
  - notify only when window is unfocused
- Daemon:
  - emit webhook on agent notify events and process failure events
  - send asynchronously with bounded timeout

Tests:
- Transition detection unit tests.
- Config parsing tests.
- Webhook sender tests with mocked HTTP endpoint.

Why second:
- Most plumbing already exists, so this is the fastest Tier 1 win after shared config.

## Phase 3: Command palette

Deliverables:
- `Cmd+K` modal in the native GUI.
- Fuzzy search across:
  - global actions
  - repositories
  - worktrees
  - repo presets
  - task templates

Implementation notes:
- Add a lightweight `CommandPaletteItem` model instead of branching directly in the renderer.
- Native-only in v1.
- Keep matching simple and deterministic first: lowercase contains + token scoring is enough.

Minimum actions for v1:
- New worktree
- Refresh worktrees
- Open settings
- Open theme picker
- Launch agent preset
- Launch repo preset
- Select repository
- Select worktree
- Open task template

Tests:
- Filtering/ranking unit tests.
- Keyboard interaction smoke tests if the existing GUI test setup can support them.

Why third:
- It becomes the main entry point for task templates and reduces UI sprawl.

## Phase 4: Task templates / magic commands

Deliverables:
- Repo-local template discovery from `.arbor/tasks/*.md`.
- New task metadata parser with frontmatter kept intentionally small.
- Launch flow from command palette.

Recommended task file shape:
```md
---
name: Investigate failing test
agent: codex
---
Investigate the failing test and propose the smallest safe fix.
```

Implementation notes:
- Reuse the shared prompt runner where possible.
- If a provider supports non-interactive prompt execution cleanly, launch directly from the template.
- If not, fall back to spawning a terminal with the provider command and preloading the prompt in the session input path.
- Keep task templates repo-local and versioned; do not create a second GUI-only storage system.

Scope guard:
- Do not implement full CAR-style task queues in Tier 1.
- Do not add provider-specific rich prompt builders yet.

Tests:
- Task discovery and frontmatter parsing.
- Launch command construction.

Why fourth:
- It depends on command palette for good UX and on the prompt runner for launch behavior.

## Phase 5: AI commit messages

Deliverables:
- Replace direct auto-commit with a small commit flow:
  - editable message buffer
  - `Generate` action
  - `Commit` action
- AI generation from staged diff or Arbor’s changed-file view.

Implementation notes:
- Preserve the current heuristic subject/body generator as a fallback when AI generation fails.
- Do not commit immediately after generation; always let the user edit/confirm.
- Limit context size to staged diff or bounded file summaries to avoid shelling giant prompts into CLIs.

Suggested UX:
- Clicking `Commit` opens a compact modal.
- Modal contains message textarea, `Generate`, `Use Default`, `Commit`, `Cancel`.

Tests:
- Commit message state reducer tests.
- Prompt runner tests for generation failure/success.
- Existing commit path regression test.

Why fifth:
- This requires the most UI change and benefits directly from the prompt runner introduced for tasks.

## Tier 1 Completion Criteria

Treat Tier 1 as complete when:
- Worktree setup/teardown scripts run from both native and HTTP flows via `arbor.toml`.
- Agent-finished desktop notifications fire on real state transitions, and optional webhooks can be routed from the daemon.
- Native GUI has a working `Cmd+K` command palette.
- Repo-local task templates can be discovered and launched.
- Commit flow supports AI generation with manual confirmation.
- CI status is either left as complete-for-native or explicitly documented as such.

## Explicit Non-Goals For This Pass

- No new `.arbor.toml` file.
- No full chat/composer UI.
- No ticket queue runner.
- No rich webhook provider integrations beyond generic webhook POSTs.
- No web UI parity for command palette or AI commit generation in the first pass.

## Suggested Work Breakdown

1. Shared repo config in `arbor-core`.
2. Lifecycle script executor plus tests.
3. Notification config + transition notifications + webhook sender.
4. Command palette model, modal, keybinding.
5. Task template discovery and launch.
6. Commit modal and AI generation path.
7. Cleanup pass: docs, examples, and roadmap status updates.

## Risks To Manage

- Provider variance: task launching and AI commit generation should degrade gracefully when a preset command cannot be used non-interactively.
- Duplication: avoid parallel config structs in GUI and daemon.
- Overloading `main.rs`: extract new feature modules early rather than appending another large block into the window file.
- False-positive notifications: fire only on actual transitions, not on every state snapshot.
