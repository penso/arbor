import { openCreateWorktreeModal } from "./create-worktree-modal";
import {
  refresh,
  refreshIssues,
  selectWorktree,
  selectedIssueRepoRoot,
  setRightPaneTab,
  state,
  subscribe,
  updateState,
} from "../state";
import { createTerminal as apiCreateTerminal } from "../api";
import type { Issue, RightPaneTab } from "../types";
import { el, shortPath, titleFromPath } from "../utils";

type PaletteMode = "actions" | "issues";

type PaletteItem = {
  id: string;
  icon: string;
  title: string;
  subtitle: string;
  keywords: string;
  category: string;
  run: () => void;
};

let overlay: HTMLDivElement | null = null;
let inputEl: HTMLInputElement | null = null;
let resultsEl: HTMLDivElement | null = null;
let open = false;
let mode: PaletteMode = "actions";
let query = "";
let selectedIndex = 0;

// Track recently used action IDs to prioritize them in results
const MAX_RECENT = 5;
let recentActionIds: string[] = [];

function trackRecent(id: string): void {
  recentActionIds = [id, ...recentActionIds.filter((r) => r !== id)].slice(0, MAX_RECENT);
}

// --- Static actions ---

function staticActions(): PaletteItem[] {
  return [
    {
      id: "new-worktree",
      icon: "\u{f126}",
      title: "New Worktree",
      subtitle: "Create a worktree from an issue",
      keywords: "new worktree create branch issue",
      category: "Worktrees",
      run: () => openIssuesMode(),
    },
    {
      id: "issues",
      icon: "\u{f188}",
      title: "View Issues",
      subtitle: "Browse repository issues",
      keywords: "issues github gitlab linear tickets bugs browse",
      category: "Navigation",
      run: () => openIssuesMode(),
    },
    {
      id: "refresh",
      icon: "\u{f021}",
      title: "Refresh",
      subtitle: "Reload repositories, worktrees, and terminals",
      keywords: "refresh reload sync fetch update",
      category: "Actions",
      run: () => {
        closeCommandPalette();
        void refresh();
      },
    },
    {
      id: "new-terminal",
      icon: "\u{f120}",
      title: "New Terminal",
      subtitle: "Open a plain terminal in the selected worktree",
      keywords: "new terminal shell open",
      category: "Terminal",
      run: () => {
        closeCommandPalette();
        void openNewTerminal();
      },
    },
  ];
}

// --- Right pane tab actions ---

function rightPaneActions(): PaletteItem[] {
  const tabs: { id: RightPaneTab; title: string; icon: string; keywords: string }[] = [
    { id: "changes", title: "Changes", icon: "\u{f440}", keywords: "changes diff modified staged" },
    { id: "files", title: "Files", icon: "\u{f15c}", keywords: "files tree explorer browse" },
    {
      id: "processes",
      title: "Processes",
      icon: "\u{f085}",
      keywords: "processes running services daemon procfile",
    },
    { id: "notes", title: "Notes", icon: "\u{f249}", keywords: "notes markdown readme" },
  ];

  return tabs.map((tab) => ({
    id: `tab-${tab.id}`,
    icon: tab.icon,
    title: `Show ${tab.title}`,
    subtitle:
      state.rightPaneTab === tab.id ? "Right panel · Active" : "Switch right panel tab",
    keywords: `${tab.keywords} panel tab right show view`,
    category: "Panels",
    run: () => {
      closeCommandPalette();
      setRightPaneTab(tab.id);
    },
  }));
}

// --- Agent preset actions ---

const AGENT_PRESETS = [
  { id: "claude", label: "Claude", command: "claude", icon: "\u{f120}" },
  { id: "codex", label: "Codex", command: "codex", icon: "\u{f121}" },
  { id: "opencode", label: "OpenCode", command: "opencode", icon: "\u{f121}" },
  { id: "copilot", label: "Copilot", command: "copilot", icon: "\u{f121}" },
];

function agentPresetActions(): PaletteItem[] {
  return AGENT_PRESETS.map((preset) => ({
    id: `agent-${preset.id}`,
    icon: preset.icon,
    title: `Launch ${preset.label}`,
    subtitle: "Start an agent terminal",
    keywords: `launch agent ${preset.label.toLowerCase()} terminal ai coding`,
    category: "Agents",
    run: () => {
      closeCommandPalette();
      void launchAgent(preset.command, preset.label);
    },
  }));
}

// --- Dynamic worktree/repository items ---

function worktreeActions(): PaletteItem[] {
  return state.worktrees.map((worktree) => {
    const label = titleFromPath(worktree.path);
    const isActive = state.selectedWorktreePath === worktree.path;
    return {
      id: `worktree-${worktree.path}`,
      icon: "\u{f126}",
      title: label,
      subtitle: `Worktree · ${worktree.branch}${isActive ? " · Active" : ""}`,
      keywords: `worktree switch ${label.toLowerCase()} ${worktree.branch.toLowerCase()} ${shortPath(worktree.path).toLowerCase()}`,
      category: "Worktrees",
      run: () => {
        closeCommandPalette();
        selectWorktree(worktree.path);
      },
    };
  });
}

function repositoryActions(): PaletteItem[] {
  if (state.repositories.length <= 1) return [];

  return state.repositories.map((repo) => {
    const isActive = state.selectedRepoRoot === repo.root;
    return {
      id: `repo-${repo.root}`,
      icon: "\u{f1d3}",
      title: repo.label,
      subtitle: `Repository${isActive ? " · Active" : ""}`,
      keywords: `repository repo switch ${repo.label.toLowerCase()} ${shortPath(repo.root).toLowerCase()}`,
      category: "Repositories",
      run: () => {
        closeCommandPalette();
        updateState({ selectedRepoRoot: repo.root });
        // Auto-select first worktree in this repo
        const repoWorktree = state.worktrees.find((w) => w.repo_root === repo.root);
        if (repoWorktree !== undefined) {
          selectWorktree(repoWorktree.path);
        }
      },
    };
  });
}

// --- Build all action items ---

function allActionItems(): PaletteItem[] {
  return [
    ...staticActions(),
    ...rightPaneActions(),
    ...agentPresetActions(),
    ...worktreeActions(),
    ...repositoryActions(),
  ];
}

// --- Search/filter with multi-token matching and ranking ---

function scoreItem(item: PaletteItem, tokens: string[]): number {
  const haystack = `${item.title} ${item.subtitle} ${item.keywords}`.toLowerCase();

  // All tokens must match
  for (const token of tokens) {
    if (!haystack.includes(token)) return -1;
  }

  let score = 0;
  const titleLower = item.title.toLowerCase();

  // Boost for recent usage
  const recentIndex = recentActionIds.indexOf(item.id);
  if (recentIndex >= 0) {
    score += (MAX_RECENT - recentIndex) * 100;
  }

  // Boost for title matches (more relevant than keyword matches)
  for (const token of tokens) {
    if (titleLower.includes(token)) score += 20;
    if (titleLower.startsWith(token)) score += 30;
    if (titleLower === token) score += 50;
  }

  // Boost active items slightly (they're relevant to current context)
  if (item.subtitle.includes("Active")) score += 5;

  return score;
}

function filteredItems(): PaletteItem[] {
  if (mode === "issues") {
    return filteredIssueItems();
  }

  const items = allActionItems();
  const trimmed = query.trim().toLowerCase();

  if (trimmed.length === 0) {
    // No query: show static actions first, then dynamic, boosted by recency
    return items.sort((a, b) => {
      const aRecent = recentActionIds.indexOf(a.id);
      const bRecent = recentActionIds.indexOf(b.id);
      if (aRecent >= 0 && bRecent >= 0) return aRecent - bRecent;
      if (aRecent >= 0) return -1;
      if (bRecent >= 0) return 1;
      return 0;
    });
  }

  const tokens = trimmed.split(/\s+/).filter((t) => t.length > 0);
  const scored: { item: PaletteItem; score: number }[] = [];

  for (const item of items) {
    const score = scoreItem(item, tokens);
    if (score >= 0) {
      scored.push({ item, score });
    }
  }

  scored.sort((a, b) => b.score - a.score);
  return scored.map((s) => s.item);
}

function filteredIssueItems(): PaletteItem[] {
  const items = state.issues.map(buildIssuePaletteItem);
  const trimmed = query.trim().toLowerCase();

  if (trimmed.length === 0) return items;

  const tokens = trimmed.split(/\s+/).filter((t) => t.length > 0);
  return items.filter((item) => {
    const haystack = `${item.title} ${item.subtitle} ${item.keywords}`.toLowerCase();
    return tokens.every((token) => haystack.includes(token));
  });
}

// --- Terminal/agent helpers ---

async function openNewTerminal(): Promise<void> {
  const worktreePath = state.selectedWorktreePath;
  if (worktreePath === null) return;

  try {
    const result = await apiCreateTerminal(worktreePath, 120, 35, titleFromPath(worktreePath));
    await refresh();
    // Import is at top level — state update triggers terminal activation
    void result;
  } catch {
    // Silently ignore — terminal panel will show status
  }
}

async function launchAgent(command: string, label: string): Promise<void> {
  const worktreePath = state.selectedWorktreePath;
  if (worktreePath === null) return;

  try {
    await apiCreateTerminal(worktreePath, 120, 35, label.toLowerCase(), command);
    await refresh();
  } catch {
    // Silently ignore — terminal panel will show status
  }
}

// --- Issue items ---

function buildIssuePaletteItem(issue: Issue): PaletteItem {
  const subtitleParts = [issue.state];
  if (issue.linked_review !== null) {
    subtitleParts.push(issue.linked_review.label);
  }
  if (issue.linked_branch !== null) {
    subtitleParts.push(issue.linked_branch);
  }
  if (subtitleParts.length === 1) {
    subtitleParts.push("Create worktree");
  }

  return {
    id: `issue-${issue.id}`,
    icon: "\u{f188}",
    title: `${issue.display_id} ${issue.title}`,
    subtitle: subtitleParts.join(" · "),
    keywords: [
      "issue",
      "issues",
      issue.id,
      issue.display_id,
      issue.title,
      issue.state,
      issue.suggested_worktree_name,
      issue.linked_branch ?? "",
      issue.linked_review?.label ?? "",
    ]
      .join(" ")
      .toLowerCase(),
    category: "Issues",
    run: () => {
      closeCommandPalette();
      openCreateWorktreeModal(issue);
    },
  };
}

// --- DOM / lifecycle ---

export function createCommandPalette(): HTMLElement {
  overlay = el("div", "overlay-shell overlay-hidden");
  overlay.setAttribute("data-testid", "command-palette");
  overlay.addEventListener("click", (event) => {
    if (event.target === overlay) {
      closeCommandPalette();
    }
  });

  const dialog = el("div", "overlay-dialog palette-dialog");
  dialog.addEventListener("click", (event) => event.stopPropagation());

  inputEl = document.createElement("input");
  inputEl.className = "palette-input";
  inputEl.type = "text";
  inputEl.autocomplete = "off";
  inputEl.spellcheck = false;
  inputEl.addEventListener("input", () => {
    query = inputEl?.value ?? "";
    selectedIndex = 0;
    render();
  });
  inputEl.addEventListener("keydown", handlePaletteKeydown);

  resultsEl = el("div", "palette-results");
  dialog.append(inputEl, resultsEl);
  overlay.append(dialog);

  document.addEventListener("keydown", (event) => {
    const isPaletteShortcut =
      (event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === "k";

    if (isPaletteShortcut) {
      event.preventDefault();
      if (open) {
        closeCommandPalette();
      } else {
        openCommandPalette();
      }
      return;
    }

    if (!open || event.target === inputEl) {
      return;
    }

    handlePaletteKeydown(event);
  });

  subscribe(render);
  render();
  return overlay;
}

function openCommandPalette(): void {
  open = true;
  mode = "actions";
  query = "";
  selectedIndex = 0;
  if (inputEl !== null) {
    inputEl.value = "";
  }
  render();
  requestAnimationFrame(() => inputEl?.focus());
}

function closeCommandPalette(): void {
  open = false;
  mode = "actions";
  query = "";
  selectedIndex = 0;
  if (inputEl !== null) {
    inputEl.value = "";
  }
  render();
}

function openIssuesMode(): void {
  mode = "issues";
  query = "";
  selectedIndex = 0;
  if (inputEl !== null) {
    inputEl.value = "";
  }

  const repoRoot = selectedIssueRepoRoot();
  if (repoRoot !== null) {
    refreshIssues(
      repoRoot,
      state.issuesRepoRoot !== repoRoot || state.issuesLoadedRepoRoot !== repoRoot,
    );
  }

  render();
  requestAnimationFrame(() => inputEl?.focus());
}

function emptyMessage(): string {
  if (mode === "actions") {
    return "No matching actions";
  }

  const repoRoot = selectedIssueRepoRoot();
  if (repoRoot === null) {
    return "Select a repository to browse issues";
  }
  if (state.issuesLoading) {
    return "Loading issues…";
  }
  if (state.issuesError !== null) {
    return state.issuesError;
  }
  if (state.issuesNotice !== null) {
    return state.issuesNotice;
  }
  if (query.trim().length > 0) {
    return "No matching issues";
  }
  return "No open issues";
}

function render(): void {
  if (overlay === null || resultsEl === null || inputEl === null) {
    return;
  }

  overlay.classList.toggle("overlay-hidden", !open);
  overlay.classList.toggle("overlay-visible", open);
  resultsEl.replaceChildren();

  if (!open) {
    return;
  }

  // Local alias so TypeScript knows it's non-null inside closures
  const results = resultsEl;

  inputEl.placeholder = mode === "actions" ? "Search actions…" : "Search issues…";

  // Show a hint below the input when in actions mode with no query
  if (mode === "actions" && query.trim().length === 0) {
    const hint = el("div", "palette-hint", "⌘K to toggle · Esc to close · ↑↓ to navigate");
    results.append(hint);
  }

  const items = filteredItems();
  if (items.length === 0) {
    results.append(el("div", "palette-empty", emptyMessage()));
    return;
  }

  // Group items by category for visual separation when no search query
  let lastCategory = "";

  items.forEach((itemData, index) => {
    // Add category headers when not searching
    if (mode === "actions" && query.trim().length === 0 && itemData.category !== lastCategory) {
      lastCategory = itemData.category;
      const header = el("div", "palette-category", itemData.category);
      results.append(header);
    }

    const item = el("button", "palette-item");
    item.type = "button";
    item.setAttribute("data-palette-item-id", itemData.id);
    if (index === selectedIndex) {
      item.classList.add("active");
    }

    const iconEl = el("span", "palette-item-icon", itemData.icon);
    const textGroup = el("div", "palette-item-text");
    textGroup.append(
      el("div", "palette-item-title", itemData.title),
      el("div", "palette-item-subtitle", itemData.subtitle),
    );

    item.append(iconEl, textGroup);
    item.addEventListener("mousemove", () => {
      if (selectedIndex !== index) {
        selectedIndex = index;
        render();
      }
    });
    item.addEventListener("click", () => {
      trackRecent(itemData.id);
      itemData.run();
    });
    results.append(item);
  });

  // Scroll active item into view
  requestAnimationFrame(() => {
    const active = results.querySelector(".palette-item.active");
    if (active !== null) {
      active.scrollIntoView({ block: "nearest" });
    }
  });
}

function handlePaletteKeydown(event: KeyboardEvent): void {
  if (!open) {
    return;
  }

  if (event.key === "Escape") {
    event.preventDefault();
    if (mode === "issues") {
      mode = "actions";
      query = "";
      selectedIndex = 0;
      if (inputEl !== null) {
        inputEl.value = "";
      }
      render();
    } else {
      closeCommandPalette();
    }
    return;
  }

  const items = filteredItems();
  if (event.key === "ArrowDown") {
    event.preventDefault();
    if (items.length > 0) {
      selectedIndex = (selectedIndex + 1) % items.length;
      render();
    }
    return;
  }

  if (event.key === "ArrowUp") {
    event.preventDefault();
    if (items.length > 0) {
      selectedIndex = (selectedIndex + items.length - 1) % items.length;
      render();
    }
    return;
  }

  if (event.key === "Enter") {
    event.preventDefault();
    const selected = items[selectedIndex];
    if (selected !== undefined) {
      trackRecent(selected.id);
      selected.run();
    }
  }
}
