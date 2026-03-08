import { el, formatAge, shortPath } from "../utils";
import type { Repository, Worktree, ProcessInfo } from "../types";
import {
  state,
  subscribe,
  notify,
  selectWorktree,
} from "../state";
import { startAllProcesses, stopAllProcesses, startProcess, stopProcess, restartProcess } from "../api";

/** Track which repo groups are collapsed (by repo root). */
const collapsedRepos = new Set<string>();

export function createSidebar(): HTMLElement {
  const sidebar = el("aside", "sidebar");
  sidebar.setAttribute("data-testid", "sidebar");

  function render(): void {
    sidebar.replaceChildren();

    const header = el("div", "sidebar-header");
    const titleRow = el("div", "sidebar-title");
    const titleIcon = el("span", "sidebar-title-icon");
    titleIcon.innerHTML = SIGNAL_TOWER_SVG;
    titleRow.append(titleIcon, document.createTextNode("Remote Control"));
    header.append(titleRow);
    sidebar.append(header);

    const scroll = el("div", "sidebar-scroll");

    if (state.repositories.length === 0) {
      scroll.append(el("div", "sidebar-empty", "No repositories"));
      sidebar.append(scroll);
      return;
    }

    for (const repo of state.repositories) {
      const repoWorktrees = state.worktrees.filter(
        (w) => w.repo_root === repo.root,
      );
      scroll.append(renderRepoGroup(repo, repoWorktrees));
    }

    // Stack section (managed processes)
    if (state.processes.length > 0) {
      scroll.append(renderStackSection(state.processes));
    }

    sidebar.append(scroll);
  }

  subscribe(render);
  render();
  return sidebar;
}

function renderRepoGroup(repo: Repository, worktrees: Worktree[]): HTMLElement {
  const isCollapsed = collapsedRepos.has(repo.root);
  const group = el("div", "repo-group");

  // Repository header row
  const header = el("div", "repo-header");

  const chevron = el("span", "repo-chevron", isCollapsed ? "\u25B8" : "\u25BE");
  chevron.addEventListener("click", (e) => {
    e.stopPropagation();
    if (collapsedRepos.has(repo.root)) {
      collapsedRepos.delete(repo.root);
    } else {
      collapsedRepos.add(repo.root);
    }
    notify();
  });

  // GitHub avatar or letter icon
  const icon = renderRepoIcon(repo);

  const name = el("span", "repo-name", repo.label);

  const count = el("span", "repo-wt-count", String(worktrees.length));

  header.append(chevron, icon, name, count);
  group.append(header);

  // Worktree cards (when not collapsed)
  if (!isCollapsed) {
    const wtList = el("div", "wt-list");
    for (const wt of worktrees) {
      wtList.append(renderWorktreeCard(wt, repo));
    }
    group.append(wtList);
  }

  return group;
}

function renderRepoIcon(repo: Repository): HTMLElement {
  if (repo.avatar_url !== null) {
    const img = document.createElement("img");
    img.className = "repo-avatar";
    img.src = repo.avatar_url;
    img.alt = repo.label;
    img.width = 32;
    img.height = 32;
    // Fallback to letter on load error
    img.addEventListener("error", () => {
      const fallback = el("span", "repo-icon", repo.label.charAt(0).toUpperCase());
      img.replaceWith(fallback);
    });
    return img;
  }

  if (repo.github_repo_slug !== null) {
    // GitHub SVG icon
    const icon = el("span", "repo-icon repo-icon-github");
    icon.innerHTML = GITHUB_SVG;
    return icon;
  }

  return el("span", "repo-icon", repo.label.charAt(0).toUpperCase());
}

function renderWorktreeCard(wt: Worktree, repo: Repository): HTMLElement {
  const isActive = state.selectedWorktreePath === wt.path;
  const card = el("div", "wt-card");
  if (isActive) card.classList.add("active");

  card.addEventListener("click", () => selectWorktree(wt.path));

  // Git branch SVG icon
  const branchIcon = el("span", "wt-branch-icon");
  branchIcon.innerHTML = GIT_BRANCH_SVG;

  // Text column
  const info = el("div", "wt-info");

  // Line 1: branch name + diff stats + age
  const line1 = el("div", "wt-line1");
  const branchName = el("span", "wt-branch", wt.branch);
  line1.append(branchName);

  // Diff stats (+N -N)
  const hasAdditions = wt.diff_additions !== null && wt.diff_additions > 0;
  const hasDeletions = wt.diff_deletions !== null && wt.diff_deletions > 0;
  if (hasAdditions || hasDeletions) {
    const stats = el("span", "wt-diff-stats");
    if (hasAdditions) {
      stats.append(el("span", "wt-diff-add", `+${wt.diff_additions}`));
    }
    if (hasDeletions) {
      stats.append(el("span", "wt-diff-del", `-${wt.diff_deletions}`));
    }
    line1.append(stats);
  }

  // PR number
  if (wt.pr_number !== null) {
    const prBadge = el("span", "wt-pr");
    if (wt.pr_url !== null) {
      const link = document.createElement("a");
      link.href = wt.pr_url;
      link.target = "_blank";
      link.rel = "noopener";
      link.textContent = `#${wt.pr_number}`;
      link.addEventListener("click", (e) => e.stopPropagation());
      prBadge.append(link);
    } else {
      prBadge.textContent = `#${wt.pr_number}`;
    }
    line1.append(prBadge);
  }

  if (wt.last_activity_unix_ms !== null) {
    line1.append(el("span", "wt-age", formatAge(wt.last_activity_unix_ms)));
  }

  // Line 2: path + primary badge
  const line2 = el("div", "wt-line2");
  line2.append(el("span", "wt-path", shortPath(wt.path)));
  if (wt.is_primary_checkout) {
    line2.append(el("span", "wt-badge", "primary"));
  }

  info.append(line1, line2);
  card.append(branchIcon, info);

  return card;
}

// ── Stack section (managed processes) ────────────────────────────────

let stackCollapsed = false;

function renderStackSection(processes: ProcessInfo[]): HTMLElement {
  const section = el("div", "stack-section");

  const header = el("div", "stack-header");

  const chevron = el("span", "repo-chevron", stackCollapsed ? "\u25B8" : "\u25BE");
  chevron.addEventListener("click", (e) => {
    e.stopPropagation();
    stackCollapsed = !stackCollapsed;
    notify();
  });

  const title = el("span", "stack-title", "Stack");
  const count = el("span", "repo-wt-count", String(processes.length));

  header.append(chevron, title, count);

  // Start/Stop all buttons
  const actions = el("span", "stack-actions");
  const hasRunning = processes.some((p) => p.status === "running");

  if (hasRunning) {
    const stopBtn = el("button", "stack-btn stack-btn-stop", "Stop");
    stopBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      stopAllProcesses().then(() => { import("../state").then((m) => m.refresh()); });
    });
    actions.append(stopBtn);
  } else {
    const startBtn = el("button", "stack-btn stack-btn-start", "Start");
    startBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      startAllProcesses().then(() => { import("../state").then((m) => m.refresh()); });
    });
    actions.append(startBtn);
  }

  header.append(actions);
  section.append(header);

  if (!stackCollapsed) {
    const list = el("div", "stack-list");
    for (const proc of processes) {
      list.append(renderProcessCard(proc));
    }
    section.append(list);
  }

  return section;
}

function renderProcessCard(proc: ProcessInfo): HTMLElement {
  const card = el("div", "process-card");

  // Status dot
  const dotClass = statusDotClass(proc.status);
  const dot = el("span", `process-dot ${dotClass}`);
  card.append(dot);

  // Info column
  const info = el("div", "process-info");
  const line1 = el("div", "process-line1");
  line1.append(el("span", "process-name", proc.name));

  if (proc.restart_count > 0) {
    line1.append(el("span", "process-restarts", `x${proc.restart_count}`));
  }

  const line2 = el("div", "process-line2");
  line2.append(el("span", "process-command", proc.command));

  info.append(line1, line2);
  card.append(info);

  // Action button
  const actionBtn = el("button", "process-action-btn");
  if (proc.status === "running") {
    actionBtn.textContent = "Stop";
    actionBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      stopProcess(proc.name).then(() => { import("../state").then((m) => m.refresh()); });
    });
  } else if (proc.status === "restarting") {
    actionBtn.textContent = "Stop";
    actionBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      stopProcess(proc.name).then(() => { import("../state").then((m) => m.refresh()); });
    });
  } else {
    actionBtn.textContent = "Start";
    actionBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      startProcess(proc.name).then(() => { import("../state").then((m) => m.refresh()); });
    });
  }

  card.append(actionBtn);
  return card;
}

function statusDotClass(status: string): string {
  switch (status) {
    case "running": return "dot-green";
    case "restarting": return "dot-yellow";
    case "crashed": return "dot-red";
    default: return "dot-gray";
  }
}

const GITHUB_SVG = `<svg viewBox="0 0 16 16" width="28" height="28" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>`;

const SIGNAL_TOWER_SVG = `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4.9 16.1C1 12.2 1 5.8 4.9 1.9"/><path d="M7.8 4.7a6.14 6.14 0 0 0-.8 7.5"/><path d="M16.2 4.7a6.14 6.14 0 0 1 .8 7.5"/><path d="M19.1 1.9a10.56 10.56 0 0 1 0 14.2"/><circle cx="12" cy="9" r="2"/><path d="M12 11v11"/></svg>`;

const GIT_BRANCH_SVG = `<svg viewBox="0 0 16 16" width="14" height="14" fill="currentColor"><path d="M9.5 3.25a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.128a2.251 2.251 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.5 0v1.836A2.493 2.493 0 0 1 6 7h4a1 1 0 0 0 1-1v-.628A2.25 2.25 0 0 1 9.5 3.25zm-6 0a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0zm8.25-.75a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5zM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5z"/></svg>`;
