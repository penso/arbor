import { expect, test } from "@playwright/test";

type ManagedWorktreeRequest = {
  repo_root: string;
  worktree_name: string;
};

function parseManagedWorktreeRequest(raw: unknown): ManagedWorktreeRequest {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    throw new Error("managed worktree request body must be an object");
  }

  const repoRoot = raw["repo_root"];
  const worktreeName = raw["worktree_name"];
  if (typeof repoRoot !== "string" || typeof worktreeName !== "string") {
    throw new Error("managed worktree request body is missing required fields");
  }

  return {
    repo_root: repoRoot,
    worktree_name: worktreeName,
  };
}

test.describe("Arbor Web UI", () => {
  test.beforeEach(async ({ page }) => {
    let managedWorktreeCreateCount = 0;
    let managedWorktreeName: string | null = null;
    let managedWorktreeRepoRoot: string | null = null;

    await page.route("**/api/v1/repositories", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            root: "/home/user/projects/arbor",
            label: "arbor",
            github_repo_slug: "penso/arbor",
            avatar_url: null,
          },
          {
            root: "/home/user/projects/moltis",
            label: "moltis",
            github_repo_slug: "penso/moltis",
            avatar_url: "https://avatars.githubusercontent.com/penso?size=96",
          },
        ]),
      }),
    );

    await page.route("**/api/v1/worktrees", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          {
            repo_root: "/home/user/projects/arbor",
            path: "/home/user/projects/arbor",
            branch: "main",
            is_primary_checkout: true,
            last_activity_unix_ms: Date.now() - 30_000,
            diff_additions: 84,
            diff_deletions: 2,
            pr_number: null,
            pr_url: null,
          },
          {
            repo_root: "/home/user/projects/arbor",
            path: "/home/user/projects/arbor-worktrees/feature-auth",
            branch: "feature/auth",
            is_primary_checkout: false,
            last_activity_unix_ms: Date.now() - 120_000,
            diff_additions: 15,
            diff_deletions: 3,
            pr_number: 365,
            pr_url: "https://github.com/penso/arbor/pull/365",
          },
          ...(managedWorktreeCreateCount > 0 &&
            managedWorktreeName !== null &&
            managedWorktreeRepoRoot !== null
            ? [{
                repo_root: managedWorktreeRepoRoot,
                path: `/Users/penso/.arbor/worktrees/arbor/${managedWorktreeName}`,
                branch: `codex/${managedWorktreeName}`,
                is_primary_checkout: false,
                last_activity_unix_ms: Date.now(),
                diff_additions: 0,
                diff_deletions: 0,
                pr_number: null,
                pr_url: null,
              }]
            : []),
          {
            repo_root: "/home/user/projects/moltis",
            path: "/home/user/projects/moltis",
            branch: "main",
            is_primary_checkout: true,
            last_activity_unix_ms: null,
            diff_additions: null,
            diff_deletions: null,
            pr_number: null,
            pr_url: null,
          },
        ]),
      }),
    );

    await page.route("**/api/v1/terminals", (route) => {
      if (route.request().method() === "GET") {
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify([
            {
              session_id: "daemon-1",
              workspace_id: "/home/user/projects/arbor",
              cwd: "/home/user/projects/arbor",
              shell: "/bin/zsh",
              cols: 120,
              rows: 35,
              title: "claude",
              last_command: "just test",
              output_tail: "All tests passed!",
              exit_code: null,
              state: "running",
              updated_at_unix_ms: Date.now() - 5_000,
            },
            {
              session_id: "daemon-2",
              workspace_id: "/home/user/projects/arbor-worktrees/feature-auth",
              cwd: "/home/user/projects/arbor-worktrees/feature-auth",
              shell: "/bin/zsh",
              cols: 120,
              rows: 35,
              title: "feature-auth",
              last_command: "cargo build",
              output_tail: null,
              exit_code: 0,
              state: "completed",
              updated_at_unix_ms: Date.now() - 60_000,
            },
          ]),
        });
      }
      return route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
    });

    await page.route("**/api/v1/worktrees/changes**", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          { path: "src/main.rs", kind: "modified", additions: 15, deletions: 3 },
          { path: "src/api.rs", kind: "added", additions: 42, deletions: 0 },
          { path: "tests/old_test.rs", kind: "removed", additions: 0, deletions: 28 },
          { path: "README.md", kind: "modified", additions: 2, deletions: 1 },
        ]),
      }),
    );

    await page.route("**/api/v1/processes", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([]),
      }),
    );

    await page.route("**/api/v1/issues**", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          source: {
            provider: "github",
            label: "GitHub",
            repository: "penso/arbor",
            url: "https://github.com/penso/arbor/issues",
          },
          issues: [
            {
              id: "512",
              display_id: "#512",
              title: "Ship daemon-backed issue worktrees",
              state: "open",
              url: "https://github.com/penso/arbor/issues/512",
              suggested_worktree_name: "github-512-ship-httpd-issues",
              updated_at: "2026-03-13T10:00:00Z",
              linked_branch: "codex/github-512-ship-httpd-issues",
              linked_review: {
                kind: "pull_request",
                label: "PR #365",
                url: "https://github.com/penso/arbor/pull/365",
              },
            },
            {
              id: "513",
              display_id: "#513",
              title: "Teach the command palette about issues",
              state: "open",
              url: "https://github.com/penso/arbor/issues/513",
              suggested_worktree_name: "github-513-command-palette-issues",
              updated_at: "2026-03-13T08:30:00Z",
              linked_branch: null,
              linked_review: null,
            },
          ],
          notice: null,
        }),
      }),
    );

    await page.route("**/api/v1/worktrees/managed/preview", (route) => {
      const body = parseManagedWorktreeRequest(route.request().postDataJSON());
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          sanitized_worktree_name: body.worktree_name,
          branch: `codex/${body.worktree_name}`,
          path: `/Users/penso/.arbor/worktrees/arbor/${body.worktree_name}`,
        }),
      });
    });

    await page.route("**/api/v1/worktrees/managed", (route) => {
      managedWorktreeCreateCount += 1;
      const body = parseManagedWorktreeRequest(route.request().postDataJSON());
      managedWorktreeName = body.worktree_name;
      managedWorktreeRepoRoot = body.repo_root;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          repo_root: body.repo_root,
          path: `/Users/penso/.arbor/worktrees/arbor/${body.worktree_name}`,
          branch: `codex/${body.worktree_name}`,
          deleted_branch: null,
          message: "created",
        }),
      });
    });

    await page.goto("/");
  });

  test("renders three-pane layout", async ({ page }) => {
    const sidebar = page.getByTestId("sidebar");
    const terminalPanel = page.getByTestId("terminal-panel");
    const changesPanel = page.getByTestId("changes-panel");
    const statusBar = page.getByTestId("status-bar");

    await expect(sidebar).toBeVisible();
    await expect(terminalPanel).toBeVisible();
    await expect(changesPanel).toBeVisible();
    await expect(statusBar).toBeVisible();

    await page.screenshot({ path: "e2e/screenshots/layout.png", fullPage: true });
  });

  test("sidebar shows repos with worktrees grouped underneath", async ({ page }) => {
    const sidebar = page.getByTestId("sidebar");

    await expect(sidebar.locator(".repo-name").getByText("arbor", { exact: true })).toBeVisible();
    await expect(sidebar.locator(".repo-name").getByText("moltis", { exact: true })).toBeVisible();
    await expect(sidebar.locator(".repo-icon-github").first()).toBeVisible();
    await expect(sidebar.locator(".repo-avatar")).toBeVisible();
    await expect(sidebar.locator(".wt-branch").getByText("main").first()).toBeVisible();
    await expect(sidebar.locator(".wt-branch").getByText("feature/auth")).toBeVisible();
    await expect(sidebar.locator(".wt-branch-icon").first()).toBeVisible();
    await expect(sidebar.locator(".wt-diff-add").getByText("+84")).toBeVisible();
    await expect(sidebar.locator(".wt-diff-del").getByText("-2")).toBeVisible();
    await expect(sidebar.locator(".wt-pr").getByText("#365")).toBeVisible();
    await expect(sidebar.locator(".repo-wt-count").getByText("2")).toBeVisible();
    await expect(sidebar.locator(".repo-wt-count").getByText("1")).toBeVisible();

    await page.screenshot({
      path: "e2e/screenshots/sidebar-details.png",
      fullPage: true,
    });
  });

  test("collapsing repo hides its worktrees", async ({ page }) => {
    const sidebar = page.getByTestId("sidebar");
    const arborGroup = sidebar.locator(".repo-group").first();
    await arborGroup.locator(".repo-chevron").click();

    await expect(sidebar.locator(".wt-branch").getByText("feature/auth")).not.toBeVisible();
    await expect(sidebar.locator(".wt-branch").getByText("main")).toBeVisible();

    await page.screenshot({
      path: "e2e/screenshots/repo-collapsed.png",
      fullPage: true,
    });
  });

  test("terminal panel shows session tabs for selected worktree", async ({ page }) => {
    const terminalPanel = page.getByTestId("terminal-panel");
    await expect(terminalPanel.locator(".terminal-tab-label").getByText("just test")).toBeVisible();

    const sidebar = page.getByTestId("sidebar");
    await sidebar.locator(".wt-card").nth(1).click();
    await expect(terminalPanel.locator(".terminal-tab-label").getByText("cargo build")).toBeVisible();
  });

  test("terminal panel shows empty state for worktree without terminals", async ({ page }) => {
    const sidebar = page.getByTestId("sidebar");
    const terminalPanel = page.getByTestId("terminal-panel");

    await sidebar.locator(".wt-card").nth(2).click();

    await expect(
      terminalPanel.locator(".terminal-empty").getByText("Click + to add a terminal"),
    ).toBeVisible();
  });

  test("changes panel shows files when worktree selected", async ({ page }) => {
    const changesPanel = page.getByTestId("changes-panel");
    await expect(changesPanel.getByText("src/main.rs")).toBeVisible();
    await expect(changesPanel.getByText("src/api.rs")).toBeVisible();
    await expect(changesPanel.getByText("+15")).toBeVisible();
    await expect(changesPanel.getByText("-3")).toBeVisible();

    await page.screenshot({
      path: "e2e/screenshots/changes.png",
      fullPage: true,
    });
  });

  test("status bar shows summary", async ({ page }) => {
    const statusBar = page.getByTestId("status-bar");
    await expect(statusBar.getByText(/2 repos/)).toBeVisible();
    await expect(statusBar.getByText(/3 worktrees/)).toBeVisible();
    await expect(statusBar.getByText(/2 terminals/)).toBeVisible();
  });

  test("command palette browses issues and opens the create modal", async ({ page }) => {
    const changesPanel = page.getByTestId("changes-panel");
    await expect(changesPanel.getByText("src/main.rs")).toBeVisible();

    await page.evaluate(() => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "k",
          ctrlKey: true,
          bubbles: true,
        }),
      );
    });
    await expect(page.getByTestId("command-palette")).toBeVisible();
    const paletteInput = page.locator(".palette-input");
    await paletteInput.fill("issues");
    await paletteInput.press("Enter");

    const palette = page.getByTestId("command-palette");
    await expect(palette).toBeVisible();
    await expect(palette.locator('[data-palette-item-id="issue-512"]')).toContainText(
      "Ship daemon-backed issue worktrees",
    );
    await expect(palette.locator('[data-palette-item-id="issue-512"]')).toContainText("PR #365");
    await expect(palette.locator('[data-palette-item-id="issue-512"]')).toContainText(
      "codex/github-512-ship-httpd-issues",
    );
    await expect(palette.locator('[data-palette-item-id="issue-513"]')).toContainText(
      "Teach the command palette about issues",
    );

    await paletteInput.press("ArrowDown");
    await paletteInput.press("Enter");

    const modal = page.getByTestId("create-worktree-modal");
    await expect(modal).toBeVisible();
    await expect(modal.locator(".worktree-input")).toHaveValue("github-513-command-palette-issues");
    await expect(palette).toBeHidden();
  });

  test("issue selection opens a prefilled managed worktree modal", async ({ page }) => {
    const changesPanel = page.getByTestId("changes-panel");
    await expect(changesPanel.getByText("src/main.rs")).toBeVisible();

    await changesPanel.getByRole("button", { name: /Issues/ }).click();
    await expect(changesPanel.getByText("Ship daemon-backed issue worktrees")).toBeVisible();

    await page.locator(".issue-item").nth(1).click();

    const modal = page.getByTestId("create-worktree-modal");
    await expect(modal).toBeVisible();
    await expect(modal.locator(".overlay-title")).toHaveText("Create Worktree");
    await expect(modal.locator(".worktree-input")).toHaveValue("github-513-command-palette-issues");
    await expect(modal.getByText("codex/github-513-command-palette-issues")).toBeVisible();
    await expect(
      modal.getByText("/Users/penso/.arbor/worktrees/arbor/github-513-command-palette-issues"),
    ).toBeVisible();

    await modal.getByRole("button", { name: "Create worktree" }).click();

    await expect(modal).toBeHidden();
    await expect(
      page.getByTestId("sidebar").getByText("codex/github-513-command-palette-issues"),
    ).toBeVisible();
  });

  test("creating a worktree forces a fresh refresh after an in-flight refresh", async ({ page }) => {
    let worktreeRequestCount = 0;
    let releaseStaleRefresh: (() => void) | null = null;
    let resolveStaleRefreshSeen: (() => void) | null = null;
    const staleRefreshSeen = new Promise<void>((resolve) => {
      resolveStaleRefreshSeen = resolve;
    });

    await page.unroute("**/api/v1/worktrees");
    await page.route("**/api/v1/worktrees", async (route) => {
      worktreeRequestCount += 1;

      const baseWorktrees = [
        {
          repo_root: "/home/user/projects/arbor",
          path: "/home/user/projects/arbor",
          branch: "main",
          is_primary_checkout: true,
          last_activity_unix_ms: Date.now() - 30_000,
          diff_additions: 84,
          diff_deletions: 2,
          pr_number: null,
          pr_url: null,
        },
        {
          repo_root: "/home/user/projects/arbor",
          path: "/home/user/projects/arbor-worktrees/feature-auth",
          branch: "feature/auth",
          is_primary_checkout: false,
          last_activity_unix_ms: Date.now() - 120_000,
          diff_additions: 15,
          diff_deletions: 3,
          pr_number: 365,
          pr_url: "https://github.com/penso/arbor/pull/365",
        },
        {
          repo_root: "/home/user/projects/moltis",
          path: "/home/user/projects/moltis",
          branch: "main",
          is_primary_checkout: true,
          last_activity_unix_ms: null,
          diff_additions: null,
          diff_deletions: null,
          pr_number: null,
          pr_url: null,
        },
      ];

      if (worktreeRequestCount === 1) {
        resolveStaleRefreshSeen?.();
        await new Promise<void>((resolve) => {
          releaseStaleRefresh = resolve;
        });
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(baseWorktrees),
        });
      }

      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify([
          ...baseWorktrees,
          {
            repo_root: "/home/user/projects/arbor",
            path: "/Users/penso/.arbor/worktrees/arbor/github-513-command-palette-issues",
            branch: "codex/github-513-command-palette-issues",
            is_primary_checkout: false,
            last_activity_unix_ms: Date.now(),
            diff_additions: 0,
            diff_deletions: 0,
            pr_number: null,
            pr_url: null,
          },
        ]),
      });
    });

    const changesPanel = page.getByTestId("changes-panel");
    await changesPanel.getByRole("button", { name: /Issues/ }).click();
    await expect(changesPanel.getByText("Teach the command palette about issues")).toBeVisible();
    await page.locator(".issue-item").nth(1).click();

    const modal = page.getByTestId("create-worktree-modal");
    await expect(modal).toBeVisible();

    await page.waitForTimeout(5_100);
    await staleRefreshSeen;

    const submitPromise = modal.getByRole("button", { name: "Create worktree" }).click();
    releaseStaleRefresh?.();
    await submitPromise;

    await expect(modal).toBeHidden();
    await expect(
      page.getByTestId("sidebar").getByText("codex/github-513-command-palette-issues"),
    ).toBeVisible();
    expect(worktreeRequestCount).toBeGreaterThanOrEqual(2);
  });

  test("issues tab does not refetch unsupported providers on background refresh", async ({ page }) => {
    let issuesRequestCount = 0;

    await page.unroute("**/api/v1/issues**");
    await page.route("**/api/v1/issues**", (route) => {
      issuesRequestCount += 1;
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          source: null,
          issues: [],
          notice: "No supported issue provider resolved from the origin remote.",
        }),
      });
    });

    const changesPanel = page.getByTestId("changes-panel");
    await changesPanel.getByRole("button", { name: /Issues/ }).click();

    await expect(
      changesPanel.getByText("No supported issue provider resolved from the origin remote."),
    ).toBeVisible();

    await page.waitForTimeout(11_000);

    expect(issuesRequestCount).toBe(1);
  });

  test("full layout screenshot", async ({ page }) => {
    const changesPanel = page.getByTestId("changes-panel");
    await expect(changesPanel.getByText("src/main.rs")).toBeVisible();

    await page.screenshot({
      path: "e2e/screenshots/full-layout.png",
      fullPage: true,
    });
  });

  test("resize handles exist", async ({ page }) => {
    const handles = page.locator(".resize-handle");
    await expect(handles).toHaveCount(2);
  });

  test("mobile layout shows burger menu", async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 667 });

    const burgerBtn = page.locator(".burger-btn");
    await expect(burgerBtn).toBeVisible();

    const sidebar = page.getByTestId("sidebar");
    await expect(sidebar).not.toBeInViewport();

    await burgerBtn.click();
    await expect(sidebar).toHaveClass(/open/);
    await expect(page.locator(".sidebar-overlay.visible")).toBeAttached();
    await expect(page.getByTestId("terminal-panel")).toBeVisible();

    await page.screenshot({
      path: "e2e/screenshots/mobile-sidebar-open.png",
      fullPage: true,
    });

    await page.locator(".sidebar-overlay").click();
    await expect(sidebar).not.toHaveClass(/open/);
  });
});
