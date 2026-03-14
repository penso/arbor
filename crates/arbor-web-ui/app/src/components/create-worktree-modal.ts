import { createManagedWorktree, previewManagedWorktree } from "../api";
import { forceRefresh, selectWorktree, selectedIssueRepoRoot } from "../state";
import type { Issue, ManagedWorktreePreview } from "../types";
import { el } from "../utils";

let overlay: HTMLDivElement | null = null;
let issueBadgeEl: HTMLElement | null = null;
let issueTitleEl: HTMLElement | null = null;
let issueLinkEl: HTMLAnchorElement | null = null;
let repoEl: HTMLElement | null = null;
let inputEl: HTMLInputElement | null = null;
let branchEl: HTMLElement | null = null;
let pathEl: HTMLElement | null = null;
let errorEl: HTMLElement | null = null;
let createButtonEl: HTMLButtonElement | null = null;

let activeIssue: Issue | null = null;
let activeRepoRoot: string | null = null;
let preview: ManagedWorktreePreview | null = null;
let previewLoading = false;
let submitting = false;
let errorMessage: string | null = null;
let previewRequestId = 0;
let previewTimer: ReturnType<typeof setTimeout> | null = null;

export function createWorktreeModal(): HTMLElement {
  overlay = el("div", "overlay-shell overlay-hidden");
  overlay.setAttribute("data-testid", "create-worktree-modal");
  overlay.addEventListener("click", (event) => {
    if (event.target === overlay) {
      closeCreateWorktreeModal();
    }
  });

  const dialog = el("div", "overlay-dialog worktree-modal");
  dialog.addEventListener("click", (event) => event.stopPropagation());

  const header = el("div", "overlay-header");
  const titleGroup = el("div", "overlay-title-group");
  titleGroup.append(
    el("div", "overlay-title", "Create Worktree"),
    el("div", "overlay-subtitle", "Create a managed worktree from the selected issue."),
  );
  const closeButton = el("button", "overlay-close", "Close");
  closeButton.type = "button";
  closeButton.addEventListener("click", closeCreateWorktreeModal);
  header.append(titleGroup, closeButton);

  issueBadgeEl = el("div", "worktree-issue-badge");
  issueTitleEl = el("div", "worktree-issue-title");
  issueLinkEl = document.createElement("a");
  issueLinkEl.className = "worktree-issue-link";
  issueLinkEl.target = "_blank";
  issueLinkEl.rel = "noopener";
  issueLinkEl.textContent = "Open issue";

  repoEl = el("div", "worktree-repo");

  const issueCard = el("div", "worktree-issue-card");
  issueCard.append(issueBadgeEl, issueTitleEl, issueLinkEl, repoEl);

  const field = el("label", "worktree-field");
  field.append(el("span", "worktree-field-label", "Worktree name"));
  inputEl = document.createElement("input");
  inputEl.className = "worktree-input";
  inputEl.type = "text";
  inputEl.autocomplete = "off";
  inputEl.spellcheck = false;
  inputEl.addEventListener("input", () => {
    if (previewTimer !== null) {
      clearTimeout(previewTimer);
    }
    previewTimer = setTimeout(() => {
      previewTimer = null;
      void loadPreview();
    }, 120);
  });
  field.append(inputEl);

  branchEl = el("div", "worktree-preview-value");
  pathEl = el("div", "worktree-preview-value");
  const previewGrid = el("div", "worktree-preview-grid");
  previewGrid.append(
    buildPreviewBlock("Branch", branchEl),
    buildPreviewBlock("Path", pathEl),
  );

  errorEl = el("div", "overlay-error");

  const footer = el("div", "overlay-footer");
  const cancelButton = el("button", "overlay-button overlay-button-secondary", "Cancel");
  cancelButton.type = "button";
  cancelButton.addEventListener("click", closeCreateWorktreeModal);
  createButtonEl = el(
    "button",
    "overlay-button overlay-button-primary",
    "Create worktree",
  );
  createButtonEl.type = "button";
  createButtonEl.addEventListener("click", () => {
    void submitCreateWorktree();
  });
  footer.append(cancelButton, createButtonEl);

  dialog.append(header, issueCard, field, previewGrid, errorEl, footer);
  overlay.append(dialog);

  document.addEventListener("keydown", (event) => {
    if (!isOpen()) {
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      closeCreateWorktreeModal();
    } else if (event.key === "Enter" && !event.metaKey && !event.ctrlKey) {
      event.preventDefault();
      void submitCreateWorktree();
    }
  });

  render();
  return overlay;
}

export function openCreateWorktreeModal(issue: Issue): void {
  activeIssue = issue;
  activeRepoRoot = selectedIssueRepoRoot();
  preview = null;
  previewLoading = false;
  submitting = false;
  errorMessage = activeRepoRoot === null
    ? "No repository is selected for this issue."
    : null;

  if (inputEl !== null) {
    inputEl.value = issue.suggested_worktree_name;
  }

  render();

  if (activeRepoRoot !== null) {
    void loadPreview();
  }

  requestAnimationFrame(() => {
    inputEl?.focus();
    inputEl?.select();
  });
}

export function closeCreateWorktreeModal(): void {
  activeIssue = null;
  activeRepoRoot = null;
  preview = null;
  previewLoading = false;
  submitting = false;
  errorMessage = null;
  if (previewTimer !== null) {
    clearTimeout(previewTimer);
    previewTimer = null;
  }
  render();
}

function render(): void {
  if (
    overlay === null ||
    issueBadgeEl === null ||
    issueTitleEl === null ||
    issueLinkEl === null ||
    repoEl === null ||
    branchEl === null ||
    pathEl === null ||
    errorEl === null ||
    createButtonEl === null
  ) {
    return;
  }

  overlay.classList.toggle("overlay-hidden", !isOpen());
  overlay.classList.toggle("overlay-visible", isOpen());

  if (activeIssue === null) {
    issueBadgeEl.textContent = "";
    issueTitleEl.textContent = "";
    issueLinkEl.removeAttribute("href");
    repoEl.textContent = "";
    branchEl.textContent = "";
    pathEl.textContent = "";
    errorEl.textContent = "";
    createButtonEl.disabled = true;
    createButtonEl.textContent = "Create worktree";
    return;
  }

  issueBadgeEl.textContent = activeIssue.display_id;
  issueTitleEl.textContent = activeIssue.title;
  if (activeIssue.url !== null) {
    issueLinkEl.href = activeIssue.url;
    issueLinkEl.style.display = "";
  } else {
    issueLinkEl.removeAttribute("href");
    issueLinkEl.style.display = "none";
  }
  repoEl.textContent = activeRepoRoot ?? "No repository selected";
  branchEl.textContent = previewLoading
    ? "Resolving preview…"
    : preview?.branch ?? "Preview unavailable";
  pathEl.textContent = previewLoading
    ? "Resolving preview…"
    : preview?.path ?? "Preview unavailable";
  errorEl.textContent = errorMessage ?? "";
  createButtonEl.disabled = activeRepoRoot === null || previewLoading || preview === null || submitting;
  createButtonEl.textContent = submitting ? "Creating…" : "Create worktree";
}

async function loadPreview(): Promise<void> {
  if (activeIssue === null || activeRepoRoot === null || inputEl === null) {
    return;
  }

  const worktreeName = inputEl.value.trim();
  if (worktreeName.length === 0) {
    preview = null;
    errorMessage = "Worktree name is required.";
    render();
    return;
  }

  const requestId = ++previewRequestId;
  previewLoading = true;
  preview = null;
  errorMessage = null;
  render();

  try {
    const nextPreview = await previewManagedWorktree(activeRepoRoot, worktreeName);
    if (requestId !== previewRequestId) {
      return;
    }
    preview = nextPreview;
    errorMessage = null;
  } catch (error) {
    if (requestId !== previewRequestId) {
      return;
    }
    preview = null;
    errorMessage = error instanceof Error ? error.message : "Failed to preview worktree";
  } finally {
    if (requestId === previewRequestId) {
      previewLoading = false;
      render();
    }
  }
}

async function submitCreateWorktree(): Promise<void> {
  if (activeRepoRoot === null || activeIssue === null || inputEl === null || previewLoading) {
    return;
  }

  const worktreeName = inputEl.value.trim();
  if (worktreeName.length === 0) {
    errorMessage = "Worktree name is required.";
    render();
    return;
  }

  submitting = true;
  errorMessage = null;
  render();

  try {
    const response = await createManagedWorktree(activeRepoRoot, worktreeName);
    await forceRefresh();
    selectWorktree(response.path);
    closeCreateWorktreeModal();
  } catch (error) {
    errorMessage = error instanceof Error ? error.message : "Failed to create worktree";
    submitting = false;
    render();
  }
}

function buildPreviewBlock(label: string, valueNode: HTMLElement): HTMLElement {
  const block = el("div", "worktree-preview-block");
  block.append(el("div", "worktree-preview-label", label), valueNode);
  return block;
}

function isOpen(): boolean {
  return activeIssue !== null;
}
