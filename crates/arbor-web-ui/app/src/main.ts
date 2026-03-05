import "./styles.css";

type Repository = {
  root: string;
  label: string;
};

type Worktree = {
  repo_root: string;
  path: string;
  branch: string;
  is_primary_checkout: boolean;
};

type TerminalState = "running" | "completed" | "failed";

type TerminalSession = {
  session_id: string;
  workspace_id: string;
  cwd: string;
  shell: string;
  cols: number;
  rows: number;
  title: string | null;
  last_command: string | null;
  output_tail: string | null;
  exit_code: number | null;
  state: TerminalState | null;
  updated_at_unix_ms: number | null;
};

type DashboardState = {
  repositories: Repository[];
  worktrees: Worktree[];
  sessions: TerminalSession[];
  selectedRepositoryRoot: string | null;
  selectedWorktreePath: string | null;
  activeSessionId: string | null;
  liveOutput: string;
  liveStatus: string;
  loading: boolean;
  error: string | null;
  refreshedAt: string | null;
};

type WsSnapshotEvent = {
  output_tail: string;
  state: TerminalState;
  exit_code: number | null;
  updated_at_unix_ms: number | null;
};

type WsOutputEvent = {
  data: string;
};

type WsExitEvent = {
  state: TerminalState;
  exit_code: number | null;
};

const appNode = document.getElementById("app");
if (!(appNode instanceof HTMLDivElement)) {
  throw new Error("missing #app root");
}

const state: DashboardState = {
  repositories: [],
  worktrees: [],
  sessions: [],
  selectedRepositoryRoot: null,
  selectedWorktreePath: null,
  activeSessionId: null,
  liveOutput: "",
  liveStatus: "No live session",
  loading: true,
  error: null,
  refreshedAt: null
};

const REFRESH_INTERVAL_MS = 3500;
const LIVE_OUTPUT_MAX_CHARS = 40_000;
let liveSocket: WebSocket | null = null;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function readNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function readBoolean(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

function parseRepositories(value: unknown): Repository[] {
  if (!Array.isArray(value)) {
    throw new Error("repositories payload is not an array");
  }

  const repositories: Repository[] = [];
  for (const item of value) {
    if (!isRecord(item)) {
      continue;
    }

    const root = readString(item["root"]);
    const label = readString(item["label"]);
    if (root === null || label === null) {
      continue;
    }

    repositories.push({ root, label });
  }

  return repositories;
}

function parseWorktrees(value: unknown): Worktree[] {
  if (!Array.isArray(value)) {
    throw new Error("worktrees payload is not an array");
  }

  const worktrees: Worktree[] = [];
  for (const item of value) {
    if (!isRecord(item)) {
      continue;
    }

    const repoRoot = readString(item["repo_root"]);
    const path = readString(item["path"]);
    const branch = readString(item["branch"]);
    const isPrimaryCheckout = readBoolean(item["is_primary_checkout"]);

    if (
      repoRoot === null ||
      path === null ||
      branch === null ||
      isPrimaryCheckout === null
    ) {
      continue;
    }

    worktrees.push({
      repo_root: repoRoot,
      path,
      branch,
      is_primary_checkout: isPrimaryCheckout
    });
  }

  return worktrees;
}

function parseTerminalState(value: unknown): TerminalState | null {
  if (value === "running" || value === "completed" || value === "failed") {
    return value;
  }

  return null;
}

function parseSessions(value: unknown): TerminalSession[] {
  if (!Array.isArray(value)) {
    throw new Error("terminals payload is not an array");
  }

  const sessions: TerminalSession[] = [];
  for (const item of value) {
    if (!isRecord(item)) {
      continue;
    }

    const sessionId = readString(item["session_id"]);
    const workspaceId = readString(item["workspace_id"]);
    const cwd = readString(item["cwd"]);
    const shell = readString(item["shell"]);
    const cols = readNumber(item["cols"]);
    const rows = readNumber(item["rows"]);

    if (
      sessionId === null ||
      workspaceId === null ||
      cwd === null ||
      shell === null ||
      cols === null ||
      rows === null
    ) {
      continue;
    }

    sessions.push({
      session_id: sessionId,
      workspace_id: workspaceId,
      cwd,
      shell,
      cols,
      rows,
      title: readString(item["title"]),
      last_command: readString(item["last_command"]),
      output_tail: readString(item["output_tail"]),
      exit_code: readNumber(item["exit_code"]),
      state: parseTerminalState(item["state"]),
      updated_at_unix_ms: readNumber(item["updated_at_unix_ms"])
    });
  }

  return sessions;
}

async function fetchUnknown(url: string): Promise<unknown> {
  const response = await fetch(url, {
    headers: {
      Accept: "application/json"
    }
  });

  if (!response.ok) {
    throw new Error(`request failed (${response.status}) for ${url}`);
  }

  return response.json();
}

async function refresh(): Promise<void> {
  state.loading = true;
  state.error = null;
  render();

  try {
    const [repositoriesRaw, worktreesRaw, sessionsRaw] = await Promise.all([
      fetchUnknown("/api/v1/repositories"),
      fetchUnknown("/api/v1/worktrees"),
      fetchUnknown("/api/v1/terminals")
    ]);

    state.repositories = parseRepositories(repositoriesRaw);
    state.worktrees = parseWorktrees(worktreesRaw);
    state.sessions = parseSessions(sessionsRaw);

    if (
      state.selectedRepositoryRoot !== null &&
      !state.repositories.some(
        (repository) => repository.root === state.selectedRepositoryRoot
      )
    ) {
      state.selectedRepositoryRoot = null;
    }

    if (
      state.selectedWorktreePath !== null &&
      !state.worktrees.some((worktree) => worktree.path === state.selectedWorktreePath)
    ) {
      state.selectedWorktreePath = null;
    }

    if (
      state.activeSessionId !== null &&
      !state.sessions.some((session) => session.session_id === state.activeSessionId)
    ) {
      closeLiveSocket();
      state.activeSessionId = null;
      state.liveOutput = "";
      state.liveStatus = "No live session";
    }

    state.refreshedAt = new Date().toLocaleTimeString();
  } catch (error) {
    if (error instanceof Error) {
      state.error = error.message;
    } else {
      state.error = "unknown request failure";
    }
  } finally {
    state.loading = false;
    render();
  }
}

async function createTerminalForSelectedWorktree(): Promise<void> {
  const worktreePath = state.selectedWorktreePath;
  if (worktreePath === null) {
    state.error = "Select a worktree first";
    render();
    return;
  }

  try {
    const response = await fetch("/api/v1/terminals", {
      method: "POST",
      headers: {
        "Content-Type": "application/json"
      },
      body: JSON.stringify({
        cwd: worktreePath,
        workspace_id: worktreePath,
        title: terminalTitleFromPath(worktreePath)
      })
    });

    if (!response.ok) {
      throw new Error(`failed to create terminal (${response.status})`);
    }

    const payload = await response.json();
    if (isRecord(payload) && isRecord(payload["session"])) {
      const sessionId = readString(payload["session"]["session_id"]);
      if (sessionId !== null) {
        state.activeSessionId = sessionId;
        connectLiveSession(sessionId);
      }
    }

    await refresh();
  } catch (error) {
    state.error = error instanceof Error ? error.message : "failed to create terminal";
    render();
  }
}

function connectLiveSession(sessionId: string): void {
  if (state.activeSessionId === sessionId && liveSocket !== null) {
    return;
  }

  closeLiveSocket();
  state.activeSessionId = sessionId;
  state.liveOutput = "";
  state.liveStatus = "Connecting...";
  render();

  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  const wsUrl = `${protocol}//${window.location.host}/api/v1/terminals/${encodeURIComponent(
    sessionId
  )}/ws`;
  const socket = new WebSocket(wsUrl);
  liveSocket = socket;

  socket.addEventListener("open", () => {
    state.liveStatus = `Live: ${sessionId}`;
    render();
  });

  socket.addEventListener("message", (event) => {
    if (typeof event.data !== "string") {
      return;
    }

    let parsed: unknown;
    try {
      parsed = JSON.parse(event.data);
    } catch {
      return;
    }

    if (!isRecord(parsed)) {
      return;
    }

    const eventType = readString(parsed["type"]);
    if (eventType === null) {
      return;
    }

    if (eventType === "snapshot") {
      const snapshot = parseWsSnapshot(parsed);
      if (snapshot !== null) {
        state.liveOutput = snapshot.output_tail;
        state.liveStatus = `Live: ${sessionId} (${snapshot.state})`;
        render();
      }
      return;
    }

    if (eventType === "output") {
      const output = parseWsOutput(parsed);
      if (output !== null) {
        appendLiveOutput(output.data);
      }
      return;
    }

    if (eventType === "exit") {
      const exit = parseWsExit(parsed);
      if (exit !== null) {
        appendLiveOutput(
          `\n\n[session exited: state=${exit.state}, code=${String(exit.exit_code)}]\n`
        );
        state.liveStatus = `Closed: ${sessionId}`;
        render();
      }
      return;
    }

    if (eventType === "error") {
      const message = readString(parsed["message"]);
      if (message !== null) {
        appendLiveOutput(`\n[daemon error] ${message}\n`);
      }
    }
  });

  socket.addEventListener("close", () => {
    if (liveSocket === socket) {
      liveSocket = null;
      state.liveStatus = `Disconnected: ${sessionId}`;
      render();
    }
  });

  socket.addEventListener("error", () => {
    state.liveStatus = `Socket error: ${sessionId}`;
    render();
  });
}

function closeLiveSocket(): void {
  if (liveSocket !== null) {
    liveSocket.close();
    liveSocket = null;
  }
}

function parseWsSnapshot(value: Record<string, unknown>): WsSnapshotEvent | null {
  const outputTail = readString(value["output_tail"]);
  const stateValue = parseTerminalState(value["state"]);
  if (outputTail === null || stateValue === null) {
    return null;
  }

  return {
    output_tail: outputTail,
    state: stateValue,
    exit_code: readNumber(value["exit_code"]),
    updated_at_unix_ms: readNumber(value["updated_at_unix_ms"])
  };
}

function parseWsOutput(value: Record<string, unknown>): WsOutputEvent | null {
  const data = readString(value["data"]);
  if (data === null) {
    return null;
  }

  return { data };
}

function parseWsExit(value: Record<string, unknown>): WsExitEvent | null {
  const stateValue = parseTerminalState(value["state"]);
  if (stateValue === null) {
    return null;
  }

  return {
    state: stateValue,
    exit_code: readNumber(value["exit_code"])
  };
}

function appendLiveOutput(chunk: string): void {
  state.liveOutput += chunk;
  const charCount = state.liveOutput.length;
  if (charCount > LIVE_OUTPUT_MAX_CHARS) {
    state.liveOutput = state.liveOutput.slice(charCount - LIVE_OUTPUT_MAX_CHARS);
  }
  render();
}

function sendLiveInput(data: string): void {
  if (liveSocket === null || liveSocket.readyState !== WebSocket.OPEN) {
    state.error = "Live socket is not connected";
    render();
    return;
  }

  liveSocket.send(
    JSON.stringify({
      type: "input",
      data
    })
  );
}

function sendLiveSignal(signal: "interrupt" | "kill"): void {
  if (liveSocket === null || liveSocket.readyState !== WebSocket.OPEN) {
    return;
  }

  liveSocket.send(
    JSON.stringify({
      type: "signal",
      signal
    })
  );
}

function createElement<K extends keyof HTMLElementTagNameMap>(
  tagName: K,
  className?: string,
  text?: string
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tagName);
  if (className !== undefined) {
    element.className = className;
  }
  if (text !== undefined) {
    element.textContent = text;
  }
  return element;
}

function formatSessionAge(timestamp: number | null): string {
  if (timestamp === null) {
    return "unknown";
  }

  const ageMs = Date.now() - timestamp;
  if (ageMs < 15_000) {
    return "just now";
  }
  const seconds = Math.floor(ageMs / 1000);
  if (seconds < 60) {
    return `${seconds}s ago`;
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m ago`;
  }
  const hours = Math.floor(minutes / 60);
  return `${hours}h ago`;
}

function formatTerminalState(stateValue: TerminalState | null): string {
  if (stateValue === null) {
    return "unknown";
  }

  return stateValue;
}

function terminalTitleFromPath(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const parts = normalized.split("/").filter((part) => part.length > 0);
  if (parts.length === 0) {
    return "term";
  }

  return `term-${parts[parts.length - 1]}`;
}

function selectedRepositoryRoot(): string | null {
  return state.selectedRepositoryRoot;
}

function filteredWorktrees(): Worktree[] {
  const root = selectedRepositoryRoot();
  if (root === null) {
    return state.worktrees;
  }

  return state.worktrees.filter((worktree) => worktree.repo_root === root);
}

function filteredSessions(): TerminalSession[] {
  const root = selectedRepositoryRoot();
  if (root === null) {
    return state.sessions;
  }

  const workspaceSet = new Set(
    state.worktrees
      .filter((worktree) => worktree.repo_root === root)
      .map((worktree) => worktree.path)
  );

  return state.sessions.filter((session) => workspaceSet.has(session.workspace_id));
}

function render(): void {
  appNode.replaceChildren();

  const shell = createElement("main", "app-shell");
  const header = createElement("header", "topbar");
  header.append(
    createElement("h1", "title", "Arbor Remote"),
    createElement(
      "p",
      "subtitle",
      state.refreshedAt === null
        ? "Waiting for first refresh"
        : `Refreshed ${state.refreshedAt}`
    )
  );

  const buttonBar = createElement("div", "actions");
  const refreshButton = createElement("button", "refresh-button", "Refresh now");
  refreshButton.addEventListener("click", () => {
    void refresh();
  });
  refreshButton.disabled = state.loading;
  buttonBar.append(refreshButton);
  header.append(buttonBar);

  const body = createElement("section", "columns");
  body.append(
    renderRepositoriesPanel(),
    renderWorktreesPanel(),
    renderTerminalPanel()
  );

  shell.append(header);

  if (state.error !== null) {
    shell.append(createElement("div", "error-banner", state.error));
  }

  if (state.loading) {
    shell.append(createElement("div", "loading-banner", "Loading..."));
  }

  shell.append(body);
  shell.append(renderLivePanel());
  appNode.append(shell);
}

function renderRepositoriesPanel(): HTMLElement {
  const panel = createElement("section", "panel");
  panel.append(createElement("h2", "panel-title", "Repositories"));

  if (state.repositories.length === 0) {
    panel.append(createElement("p", "empty", "No repositories found"));
    return panel;
  }

  const list = createElement("ul", "list");
  for (const repository of state.repositories) {
    const item = createElement("li", "list-item");
    if (state.selectedRepositoryRoot === repository.root) {
      item.classList.add("list-item-active");
    }

    const button = createElement("button", "list-button");
    button.addEventListener("click", () => {
      state.selectedRepositoryRoot =
        state.selectedRepositoryRoot === repository.root ? null : repository.root;
      state.selectedWorktreePath = null;
      render();
    });

    const title = createElement("span", "list-label", repository.label);
    const path = createElement("span", "list-meta", repository.root);
    button.append(title, path);
    item.append(button);
    list.append(item);
  }

  panel.append(list);
  return panel;
}

function renderWorktreesPanel(): HTMLElement {
  const panel = createElement("section", "panel");
  panel.append(createElement("h2", "panel-title", "Worktrees"));

  const worktrees = filteredWorktrees();
  if (worktrees.length === 0) {
    panel.append(createElement("p", "empty", "No worktrees found"));
    return panel;
  }

  const list = createElement("ul", "list");
  for (const worktree of worktrees) {
    const item = createElement("li", "list-item");
    if (state.selectedWorktreePath === worktree.path) {
      item.classList.add("list-item-active");
    }

    const button = createElement("button", "list-button");
    button.addEventListener("click", () => {
      state.selectedWorktreePath =
        state.selectedWorktreePath === worktree.path ? null : worktree.path;
      render();
    });

    const label = createElement("span", "list-label", worktree.path);
    const meta = createElement(
      "span",
      "list-meta",
      `${worktree.branch}${worktree.is_primary_checkout ? " (primary)" : ""}`
    );
    button.append(label, meta);
    item.append(button);
    list.append(item);
  }

  panel.append(list);
  return panel;
}

function renderTerminalPanel(): HTMLElement {
  const panel = createElement("section", "panel panel-wide");
  panel.append(createElement("h2", "panel-title", "Open Terminal Tabs"));

  const createButton = createElement("button", "refresh-button", "Open Terminal");
  createButton.disabled = state.selectedWorktreePath === null;
  createButton.addEventListener("click", () => {
    void createTerminalForSelectedWorktree();
  });
  panel.append(createButton);

  const sessions = filteredSessions();
  if (sessions.length === 0) {
    panel.append(createElement("p", "empty", "No open terminal tabs"));
    return panel;
  }

  const list = createElement("div", "terminal-list");
  for (const session of sessions) {
    const card = createElement("article", "terminal-card");
    if (state.activeSessionId === session.session_id) {
      card.classList.add("list-item-active");
    }

    const heading = createElement(
      "h3",
      "terminal-title",
      session.title ?? session.session_id
    );
    const meta = createElement(
      "p",
      "terminal-meta",
      `${formatTerminalState(session.state)} | ${formatSessionAge(
        session.updated_at_unix_ms
      )}`
    );
    const workspace = createElement("p", "terminal-path", session.cwd);
    const command = createElement(
      "p",
      "terminal-command",
      session.last_command === null || session.last_command.length === 0
        ? "No recent command"
        : `Last command: ${session.last_command}`
    );
    const output = createElement(
      "pre",
      "terminal-output",
      session.output_tail === null || session.output_tail.length === 0
        ? "No captured output yet"
        : session.output_tail
    );

    const openLiveButton = createElement("button", "refresh-button", "Open Live View");
    openLiveButton.addEventListener("click", () => {
      connectLiveSession(session.session_id);
    });

    card.append(heading, meta, workspace, command, output, openLiveButton);
    list.append(card);
  }

  panel.append(list);
  return panel;
}

function renderLivePanel(): HTMLElement {
  const panel = createElement("section", "panel");
  panel.append(createElement("h2", "panel-title", "Live Terminal"));
  panel.append(createElement("p", "list-meta", state.liveStatus));

  if (state.activeSessionId === null) {
    panel.append(createElement("p", "empty", "Select a terminal and open live view."));
    return panel;
  }

  const output = createElement(
    "pre",
    "terminal-output live-output",
    state.liveOutput.length === 0 ? "Waiting for output..." : state.liveOutput
  );

  const controls = createElement("div", "live-controls");
  const input = document.createElement("input");
  input.className = "live-input";
  input.type = "text";
  input.placeholder = "Type command and send";

  const sendButton = createElement("button", "refresh-button", "Send");
  sendButton.addEventListener("click", () => {
    const command = input.value;
    if (command.trim().length === 0) {
      return;
    }

    sendLiveInput(`${command}\n`);
    input.value = "";
  });

  const interruptButton = createElement("button", "refresh-button", "Ctrl-C");
  interruptButton.addEventListener("click", () => {
    sendLiveSignal("interrupt");
  });

  const killButton = createElement("button", "refresh-button", "Kill");
  killButton.addEventListener("click", () => {
    sendLiveSignal("kill");
  });

  controls.append(input, sendButton, interruptButton, killButton);
  panel.append(output, controls);
  return panel;
}

render();
void refresh();
setInterval(() => {
  void refresh();
}, REFRESH_INTERVAL_MS);
