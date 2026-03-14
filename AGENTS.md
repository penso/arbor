# AGENTS.md

This file defines how coding agents should behave in this repository.

## Priorities

1. Keep code simple, explicit, and maintainable.
2. Fix root causes, avoid temporary band-aids.
3. Preserve user changes, never revert unrelated edits.

## Workflow

1. Read this file at task start.
2. Prefer `just` recipes for common tasks.
3. Before committing code, always run `just format` and `just lint` and fix any failures.
4. Before handoff, run relevant checks for touched code.

## UI Parity

- Arbor has two user-facing UI surfaces: `arbor-gui` and `arbor-web-ui`.
- When adding, changing, or removing a user-visible feature in one surface, check whether the other surface needs the same capability.
- Default to keeping both surfaces in parity. If parity is intentionally deferred, call that out clearly in the handoff and create follow-up work instead of silently shipping only one side.
- UI verification should cover both surfaces when the feature is meant to exist in both.

## GPUI Threading Rules

- Treat the GPUI app/window/entity context as the UI thread unless you have explicitly moved work off it.
- Be extremely careful not to block the UI thread with disk I/O, network I/O, daemon RPCs, SSH, git/process spawning, sleeps, waits, or CPU-heavy work. If it can stall a frame, assume it is forbidden on the UI thread.
- GPUI/Zed guidance matters here: `App::spawn`, `Context::spawn_in`, and `AsyncWindowContext::spawn` run futures that are polled on the main thread. Do not put blocking or CPU-intensive work directly inside those futures.
- Use `cx.background_spawn(...)` or a `BackgroundExecutor` for blocking or CPU-heavy work. Use the foreground task only to kick work off, await the result, and then hop back into `update(...)` to apply state changes.
- If you must adapt a synchronous blocking function, run it on a background thread/executor. Prefer GPUI background tasks, and use `smol::unblock` when you need to wrap a blocking call into a future. Do not introduce Tokio.
- Render paths must be pure state reads. Never do filesystem scans, config loads, git queries, daemon calls, SSH work, process spawning, or expensive recomputation from `render_*` methods.
- Event handlers and hot paths must stay thin. `on_action`, `listener`, websocket/message handlers, timers, auto-refresh loops, and key/mouse handlers should schedule background work and return quickly instead of doing the slow part inline.
- Cache derived data that is expensive to compute or load. If the UI needs it often, compute it in the background, store it in state, and render from the cached state.
- When reviewing GPUI code, ask two questions every time: `could this block?` and `could this run during render or a hot UI path?` If yes, move it off-thread.

## Commands

- Format: `just format`
- Format check: `just format-check`
- Lint: `just lint`
- Test: `just test`
- Run app: `just run`
- Run HTTP daemon: `just run-httpd`

## Rust Rules

- Do not use `unwrap()` or `expect()` in non-test code. In test modules, allow them with `#[allow(clippy::unwrap_used, clippy::expect_used)]`.
- Use `arbor_core::ResultExt` / `arbor_core::OptionExt` for `.context()` instead of ad-hoc `.map_err()`.
- Use `SessionId` and `WorkspaceId` newtypes (from `arbor_core::id`) instead of raw `String`.
- Use clear error handling with typed errors (`thiserror`/`anyhow` where appropriate).
- Keep modules focused and delete dead code instead of leaving it around.
- **Never shell out to external CLIs** (`gh`, `git` via `Command::new`, etc.) for GitHub API calls. Use Rust crates (`octocrab`, `reqwest`, etc.) instead. `std::process::Command` is if no rust crate exists.

## Code Organization

- Split large files by domain. Target ~800 lines per file max.
- Use `pub(crate)` for items shared within a crate. Apply to fields, methods, and free functions.
- Use `pub(crate) use module::*` re-exports to keep call sites clean after splitting.
- When extracting code to a new file, ensure all struct fields are `pub(crate)` (not private) so the parent module can access them.

## Feature Flags

- `arbor-gui` features: `ssh`, `mosh`, `mdns` (default: all enabled).
- `arbor-httpd` features: `mdns` (default: enabled).
- `arbor-core` features: `ssh`, `mosh` (propagated from GUI).
- Gate optional modules/functions with `#[cfg(feature = "...")]`.
- Use `dep:crate_name` syntax for optional dependency features.
- `mosh` implies `ssh` — features are hierarchical.

## Workspace Dependencies

All dependency versions live in the root `Cargo.toml` `[workspace.dependencies]`. Subcrate `Cargo.toml` files use `{ workspace = true }`. Never hardcode a version in a subcrate.

## Common Mistakes to Avoid

- **Missing `pub(crate)` on extracted items**: When moving structs, enums, or functions from `main.rs` to submodules, every field and method that was previously accessible needs `pub(crate)`.
- **Unused imports after extraction**: After moving code out of a file, clean up `use` statements in the source file — the compiler will flag these.
- **Duplicate definitions**: When splitting code, ensure a function/type exists in exactly one place. Check both the source and destination files.
- **Test module lint attributes**: Test modules need both `#[allow(clippy::unwrap_used)]` and `#[allow(clippy::expect_used)]` since the workspace denies both.
- **Forgetting `#[cfg(feature)]` on imports**: When gating a module with a feature flag, also gate its `use` import and any code that references its types.

## Git Rules

- Treat `git status` / `git diff` as read-only context.
- Do not run destructive git commands.
- Do not amend commits unless explicitly asked.
- Only create commits when the user asks.

## PR Review Comments

When working in a worktree linked to a pull request, check `.arbor/pr-comments.md` for
review comments left by reviewers. This file is auto-generated by the Arbor GUI and
contains all PR review threads grouped by file path. Address unresolved comments when
they relate to your current task. Delete `.arbor/pr-comments.md` before merging to main.

## UI Verification

When adding or modifying UI, use `screencapture -x /tmp/screenshot.png` to take a screenshot and verify the result visually. Run the app with `just run`, capture, then inspect the image.

## Changelog

- Use `git-cliff` for changelog generation.
- Config file: `cliff.toml`
- Commands:
  - `just changelog`
  - `just changelog-unreleased`
  - `just changelog-release <version>`

## Project Structure

| Crate | Description |
|-------|-------------|
| `arbor-core` | Worktree primitives, change detection, shared types (`SessionId`, `WorkspaceId`, `ResultExt`) |
| `arbor-daemon-client` | HTTP client for talking to arbor-httpd |
| `arbor-gui` | GPUI desktop app (`Arbor` binary) |
| `arbor-httpd` | Remote HTTP daemon (`arbor-httpd` binary) |
| `arbor-mcp` | MCP server for AI agent integration |
| `arbor-mosh` | Mosh shell backend (optional) |
| `arbor-ssh` | SSH shell backend (optional) |
| `arbor-terminal-emulator` | Terminal emulation layer |
| `arbor-web-ui` | TypeScript dashboard assets + helper crate |

<!-- BEGIN BEADS INTEGRATION -->
## Issue Tracking with bd (beads)

**IMPORTANT**: This project uses **bd (beads)** for ALL issue tracking. Do NOT use markdown TODOs, task lists, or other tracking methods.

### Why bd?

- Dependency-aware: Track blockers and relationships between issues
- Git-friendly: Dolt-powered version control with native sync
- Agent-optimized: JSON output, ready work detection, discovered-from links
- Prevents duplicate tracking systems and confusion

### Quick Start

**Check for ready work:**

```bash
bd ready --json
```

**Create new issues:**

```bash
bd create "Issue title" --description="Detailed context" -t bug|feature|task -p 0-4 --json
bd create "Issue title" --description="What this issue is about" -p 1 --deps discovered-from:bd-123 --json
```

**Claim and update:**

```bash
bd update <id> --claim --json
bd update bd-42 --priority 1 --json
```

**Complete work:**

```bash
bd close bd-42 --reason "Completed" --json
```

### Issue Types

- `bug` - Something broken
- `feature` - New functionality
- `task` - Work item (tests, docs, refactoring)
- `epic` - Large feature with subtasks
- `chore` - Maintenance (dependencies, tooling)

### Priorities

- `0` - Critical (security, data loss, broken builds)
- `1` - High (major features, important bugs)
- `2` - Medium (default, nice-to-have)
- `3` - Low (polish, optimization)
- `4` - Backlog (future ideas)

### Workflow for AI Agents

1. **Check ready work**: `bd ready` shows unblocked issues
2. **Claim your task atomically**: `bd update <id> --claim`
3. **Work on it**: Implement, test, document
4. **Discover new work?** Create linked issue:
   - `bd create "Found bug" --description="Details about what was found" -p 1 --deps discovered-from:<parent-id>`
5. **Complete**: `bd close <id> --reason "Done"`

### Auto-Sync

bd automatically syncs via Dolt:

- Each write auto-commits to Dolt history
- Use `bd dolt push`/`bd dolt pull` for remote sync
- No manual export/import needed!

### Important Rules

- ✅ Use bd for ALL task tracking
- ✅ Always use `--json` flag for programmatic use
- ✅ Link discovered work with `discovered-from` dependencies
- ✅ Check `bd ready` before asking "what should I work on?"
- ❌ Do NOT create markdown TODO lists
- ❌ Do NOT use external issue trackers
- ❌ Do NOT duplicate tracking systems

For more details, see README.md and docs/QUICKSTART.md.

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt pull
   git push
   bd dolt push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds

<!-- END BEADS INTEGRATION -->
