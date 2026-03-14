import { changeKindInfo, el } from "../utils";
import type { ProcessInfo, Worktree } from "../types";
import {
  state,
  subscribe,
  refresh,
  setActiveSession,
  setRightPaneTab,
} from "../state";
import { restartProcess, startProcess, stopProcess } from "../api";

export function createChangesPanel(): HTMLElement {
  const panel = el("div", "changes-panel");
  panel.setAttribute("data-testid", "changes-panel");

  function render(): void {
    panel.replaceChildren();
    panel.append(renderRightPaneTabs());

    if (state.selectedWorktreePath === null) {
      panel.append(el("div", "changes-empty", "Select a worktree"));
      return;
    }

    const worktree = state.worktrees.find((entry) => entry.path === state.selectedWorktreePath);
    if (worktree === undefined) {
      panel.append(el("div", "changes-empty", "Select a worktree"));
      return;
    }

    if (state.rightPaneTab === "procfile") {
      panel.append(renderProcfileContent(worktree));
      return;
    }

    panel.append(renderChangesContent());
  }

  subscribe(render);
  render();
  return panel;
}

function renderRightPaneTabs(): HTMLElement {
  const tabs = el("div", "changes-tabs");
  tabs.append(
    renderTabButton("Changes", "changes"),
    renderTabButton("Procfile", "procfile"),
  );
  return tabs;
}

function renderTabButton(label: string, tab: "changes" | "procfile"): HTMLButtonElement {
  const button = document.createElement("button");
  button.className = "changes-tab-button";
  button.type = "button";
  if (state.rightPaneTab === tab) button.classList.add("active");
  button.textContent = label;
  button.addEventListener("click", () => setRightPaneTab(tab));
  return button;
}

function renderChangesContent(): HTMLElement {
  const wrapper = el("div", "changes-pane-body");
  const header = el("div", "changes-header");
  const title = el("h3", "changes-title", "Changes");
  const count = el("span", "changes-count", String(state.changedFiles.length));
  header.append(title, count);
  wrapper.append(header);

  if (state.changedFiles.length === 0) {
    wrapper.append(el("div", "changes-empty", "No changes"));
    return wrapper;
  }

  const list = el("ul", "changes-list");
  for (const file of state.changedFiles) {
    const info = changeKindInfo(file.kind);
    const item = el("li", "changes-item");

    const statusBadge = el("span", "changes-status");
    statusBadge.textContent = info.code;
    statusBadge.style.color = info.color;

    const pathEl = el("span", "changes-path", file.path);
    pathEl.title = file.path;

    const stats = el("span", "changes-stats");
    if (file.additions > 0) {
      stats.append(el("span", "changes-additions", `+${file.additions}`));
    }
    if (file.deletions > 0) {
      stats.append(el("span", "changes-deletions", `-${file.deletions}`));
    }

    item.append(statusBadge, pathEl, stats);
    list.append(item);
  }

  wrapper.append(list);
  return wrapper;
}

function renderProcfileContent(worktree: Worktree): HTMLElement {
  const wrapper = el("div", "changes-pane-body");
  const header = el("div", "changes-header");
  const titleText = worktree.processes.some((process) => process.source === "procfile")
    ? "Procfile"
    : "Processes";
  header.append(
    el("h3", "changes-title", titleText),
    el("span", "changes-count", String(worktree.processes.length)),
  );
  wrapper.append(header);

  if (worktree.processes.length === 0) {
    wrapper.append(el("div", "changes-empty", "No Procfile commands"));
    return wrapper;
  }

  const list = el("div", "procfile-list");
  for (const [index, process] of worktree.processes.entries()) {
    list.append(renderProcfileProcessCard(process, index));
  }
  wrapper.append(list);
  return wrapper;
}

function renderProcfileProcessCard(proc: ProcessInfo, processIndex: number): HTMLElement {
  const card = el("div", "procfile-card");
  const sessionId = proc.session_id;
  if (sessionId !== null) {
    card.classList.add("is-openable");
    card.addEventListener("click", () => setActiveSession(sessionId));
  }

  const header = el("div", "procfile-card-header");
  header.append(el("span", `process-dot ${statusDotClass(proc.status)}`));

  const info = el("div", "procfile-card-info");
  const line1 = el("div", "procfile-line1");
  line1.append(
    el("span", "procfile-name", proc.name),
    el("span", `procfile-status procfile-status-${proc.status}`, formatProcessStatus(proc.status)),
    el("span", "procfile-source", proc.source === "procfile" ? "Procfile" : "arbor.toml"),
  );
  if (proc.restart_count > 0) {
    line1.append(el("span", "process-restarts", `x${proc.restart_count}`));
  }
  if (proc.memory_bytes !== null) {
    line1.append(el("span", "process-memory", formatProcessMemory(proc.memory_bytes)));
  }
  if (proc.session_id !== null) {
    line1.append(el("span", "procfile-session", "Openable"));
  }

  const line2 = el("div", "procfile-line2");
  line2.append(el("span", "procfile-command", proc.command));

  info.append(line1, line2);
  header.append(info);
  card.append(header);

  const actions = el("div", "procfile-actions");
  if (sessionId !== null) {
    actions.append(renderProcessActionButton("Open", () => setActiveSession(sessionId), processIndex, "open"));
  }

  if (proc.status === "running" || proc.status === "restarting") {
    actions.append(renderProcessActionButton("Restart", () => restartProcessAndRefresh(proc.id), processIndex, "restart"));
    actions.append(renderProcessActionButton("Stop", () => stopProcessAndRefresh(proc.id), processIndex, "stop"));
  } else if (proc.status === "crashed") {
    actions.append(renderProcessActionButton("Restart", () => restartProcessAndRefresh(proc.id), processIndex, "restart"));
  } else {
    actions.append(renderProcessActionButton("Start", () => startProcessAndRefresh(proc.id), processIndex, "start"));
  }

  card.append(actions);
  return card;
}

function renderProcessActionButton(
  label: string,
  action: () => Promise<void> | void,
  processIndex: number,
  actionKind: string,
): HTMLButtonElement {
  const button = document.createElement("button");
  button.className = "process-action-btn";
  button.type = "button";
  button.textContent = label;
  button.dataset["processAction"] = `${actionKind}-${processIndex}`;
  button.addEventListener("click", (e) => {
    e.stopPropagation();
    Promise.resolve(action()).catch(() => {});
  });
  return button;
}

async function startProcessAndRefresh(id: string): Promise<void> {
  await startProcess(id);
  await refresh();
}

async function stopProcessAndRefresh(id: string): Promise<void> {
  await stopProcess(id);
  await refresh();
}

async function restartProcessAndRefresh(id: string): Promise<void> {
  await restartProcess(id);
  await refresh();
}

function formatProcessStatus(status: ProcessInfo["status"]): string {
  switch (status) {
    case "running":
      return "Running";
    case "restarting":
      return "Restarting";
    case "crashed":
      return "Crashed";
    default:
      return "Stopped";
  }
}

function formatProcessMemory(memoryBytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = memoryBytes;
  let unitIndex = 0;

  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  const digits = value >= 10 || unitIndex === 0 ? 0 : 1;
  return `RSS ${value.toFixed(digits)} ${units[unitIndex]}`;
}

function statusDotClass(status: ProcessInfo["status"]): string {
  switch (status) {
    case "running":
      return "dot-green";
    case "restarting":
      return "dot-yellow";
    case "crashed":
      return "dot-red";
    default:
      return "dot-gray";
  }
}
