import { test, expect } from "@playwright/test";

test.describe("Arbor Web UI", () => {
  test.beforeEach(async ({ page }) => {
    // Mock API responses so tests work without a running backend
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

    await page.route("**/api/v1/worktrees**", (route) =>
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

    // Repo headers
    await expect(sidebar.locator(".repo-name").getByText("arbor", { exact: true })).toBeVisible();
    await expect(sidebar.locator(".repo-name").getByText("moltis", { exact: true })).toBeVisible();

    // GitHub icon for repo without avatar (arbor has slug but no avatar_url)
    await expect(sidebar.locator(".repo-icon-github").first()).toBeVisible();

    // GitHub avatar for repo with avatar_url (moltis)
    await expect(sidebar.locator(".repo-avatar")).toBeVisible();

    // Worktree cards under their repo
    await expect(sidebar.locator(".wt-branch").getByText("main").first()).toBeVisible();
    await expect(sidebar.locator(".wt-branch").getByText("feature/auth")).toBeVisible();

    // Git branch icons on worktree cards
    await expect(sidebar.locator(".wt-branch-icon").first()).toBeVisible();

    // Diff stats on worktrees
    await expect(sidebar.locator(".wt-diff-add").getByText("+84")).toBeVisible();
    await expect(sidebar.locator(".wt-diff-del").getByText("-2")).toBeVisible();

    // PR number on feature/auth worktree
    await expect(sidebar.locator(".wt-pr").getByText("#365")).toBeVisible();

    // Worktree count badges
    await expect(sidebar.locator(".repo-wt-count").getByText("2")).toBeVisible();
    await expect(sidebar.locator(".repo-wt-count").getByText("1")).toBeVisible();

    await page.screenshot({
      path: "e2e/screenshots/sidebar-details.png",
      fullPage: true,
    });
  });

  test("collapsing repo hides its worktrees", async ({ page }) => {
    const sidebar = page.getByTestId("sidebar");

    // Click chevron on the arbor repo to collapse
    const arborGroup = sidebar.locator(".repo-group").first();
    await arborGroup.locator(".repo-chevron").click();

    // Worktrees for arbor should be hidden
    await expect(sidebar.locator(".wt-branch").getByText("feature/auth")).not.toBeVisible();
    // Moltis worktree should still show
    await expect(sidebar.locator(".wt-branch").getByText("main")).toBeVisible();

    await page.screenshot({
      path: "e2e/screenshots/repo-collapsed.png",
      fullPage: true,
    });
  });

  test("terminal panel shows session tabs", async ({ page }) => {
    const terminalPanel = page.getByTestId("terminal-panel");
    await expect(terminalPanel.locator(".terminal-tab-label").getByText("claude")).toBeVisible();
    await expect(terminalPanel.locator(".terminal-tab-label").getByText("feature-auth")).toBeVisible();
  });

  test("changes panel shows files when worktree selected", async ({ page }) => {
    const sidebar = page.getByTestId("sidebar");

    // Click the main worktree card under arbor
    await sidebar.locator(".wt-card").first().click();

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

  test("full layout screenshot", async ({ page }) => {
    // Select a worktree for full context
    const sidebar = page.getByTestId("sidebar");
    await sidebar.locator(".wt-card").first().click();

    // Wait for changes to load
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

    // Burger button should be visible
    const burgerBtn = page.locator(".burger-btn");
    await expect(burgerBtn).toBeVisible();

    // Sidebar should be hidden initially
    const sidebar = page.getByTestId("sidebar");
    await expect(sidebar).not.toBeInViewport();

    // Click burger to open sidebar
    await burgerBtn.click();
    await expect(sidebar).toHaveClass(/open/);

    // Overlay should be visible
    await expect(page.locator(".sidebar-overlay.visible")).toBeAttached();

    // Terminal panel should still be visible
    await expect(page.getByTestId("terminal-panel")).toBeVisible();

    await page.screenshot({
      path: "e2e/screenshots/mobile-sidebar-open.png",
      fullPage: true,
    });

    // Click overlay to close sidebar
    await page.locator(".sidebar-overlay").click();
    await expect(sidebar).not.toHaveClass(/open/);
  });
});
