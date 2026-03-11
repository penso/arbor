import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { el, titleFromPath } from "../utils";
import {
  state,
  subscribe,
  setActiveSession,
  filteredSessions,
  refresh,
} from "../state";
import {
  createTerminal as apiCreateTerminal,
  buildWsUrl,
  parseWsServerEvent,
  serializeWsClientEvent,
} from "../api";
import type { TerminalSession } from "../types";

const INPUT_FLUSH_MS = 16;
const TERMINAL_TAB_COMMAND_MAX_CHARS = 14;
const TEXT_ENCODER = new TextEncoder();

type TerminalInstance = {
  sessionId: string;
  xterm: Terminal;
  fitAddon: FitAddon;
  socket: WebSocket | null;
  inputQueue: Uint8Array[];
  inputTimer: ReturnType<typeof setTimeout> | null;
  resizeObserver: ResizeObserver | null;
};

let activeInstance: TerminalInstance | null = null;
let panel: HTMLElement | null = null;
let tabsContainer: HTMLElement | null = null;
let terminalContainer: HTMLElement | null = null;
let statusEl: HTMLElement | null = null;

export function createTerminalPanel(): HTMLElement {
  panel = el("div", "terminal-panel");
  panel.setAttribute("data-testid", "terminal-panel");

  // Tab bar
  const toolbar = el("div", "terminal-toolbar");
  tabsContainer = el("div", "terminal-tabs");

  const presetGroup = el("div", "preset-group");
  for (const preset of AGENT_PRESETS) {
    const btn = el("button", "preset-btn");
    const icon = el("span", `preset-icon ${preset.cssClass}`);
    const label = el("span", "", preset.label);
    btn.append(icon, label);
    btn.title = `Launch ${preset.label}`;
    btn.addEventListener("click", () => launchPreset(preset));
    presetGroup.append(btn);
  }

  const addBtn = el("button", "terminal-add-btn", "+");
  addBtn.title = "New terminal";
  addBtn.addEventListener("click", openNewTerminal);
  toolbar.append(tabsContainer, presetGroup, addBtn);

  // Terminal container
  terminalContainer = el("div", "terminal-container");

  // Status bar
  statusEl = el("div", "terminal-status");

  panel.append(toolbar, terminalContainer, statusEl);

  subscribe(renderTabs);
  renderTabs();

  return panel;
}

function renderTabs(): void {
  if (tabsContainer === null) return;
  tabsContainer.replaceChildren();

  const sessions = filteredSessions();
  if (sessions.length === 0) {
    teardownActiveInstance();
    tabsContainer.append(
      el("span", "terminal-tabs-empty", state.loading ? "Loading\u2026" : "No terminals"),
    );
    renderEmptyState(
      state.loading
        ? "Loading terminals\u2026"
        : "Click + to add a terminal",
    );
    return;
  }

  // Auto-connect if a session is selected but no xterm instance exists
  if (
    state.activeSessionId !== null &&
    (activeInstance === null || activeInstance.sessionId !== state.activeSessionId)
  ) {
    // Defer to avoid re-entrancy during render
    setTimeout(() => activateSession(state.activeSessionId!), 0);
  }

  for (const session of sessions) {
    const tab = el("button", "terminal-tab");
    if (state.activeSessionId === session.session_id) {
      tab.classList.add("active");
    }

    const stateIndicator = el("span", "terminal-tab-indicator");
    if (session.state === "running") {
      stateIndicator.classList.add("running");
    } else if (session.state === "completed") {
      stateIndicator.classList.add("completed");
    } else if (session.state === "failed") {
      stateIndicator.classList.add("failed");
    }

    const icon = el("span", "terminal-tab-icon");
    icon.setAttribute("aria-hidden", "true");

    const label = el(
      "span",
      "terminal-tab-label",
      terminalTabTitle(session),
    );

    tab.append(stateIndicator, icon, label);
    tab.addEventListener("click", () => activateSession(session.session_id));
    tabsContainer.append(tab);
  }
}

function renderEmptyState(text: string): void {
  if (terminalContainer === null) return;
  terminalContainer.replaceChildren(
    el("div", "terminal-empty", text),
  );
}

function activateSession(sessionId: string): void {
  if (activeInstance !== null && activeInstance.sessionId === sessionId) return;

  teardownActiveInstance();
  setActiveSession(sessionId);
  createXtermInstance(sessionId);
}

function createXtermInstance(sessionId: string): void {
  if (terminalContainer === null) return;
  terminalContainer.replaceChildren();

  const xterm = new Terminal({
    convertEol: false,
    disableStdin: false,
    cursorBlink: true,
    scrollback: 4000,
    fontFamily:
      "JetBrains Mono, CaskaydiaMono Nerd Font Mono, ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
    fontSize: 13,
    lineHeight: 1.35,
    theme: buildXtermTheme(),
  });

  registerOscGuards(xterm);

  const fitAddon = new FitAddon();
  xterm.loadAddon(fitAddon);

  const webLinksAddon = new WebLinksAddon();
  xterm.loadAddon(webLinksAddon);

  const wrapper = el("div", "xterm-wrapper");
  terminalContainer.append(wrapper);
  xterm.open(wrapper);

  const instance: TerminalInstance = {
    sessionId,
    xterm,
    fitAddon,
    socket: null,
    inputQueue: [],
    inputTimer: null,
    resizeObserver: null,
  };

  // Fit after open
  requestAnimationFrame(() => {
    fitAddon.fit();
    connectWebSocket(instance);
  });

  // Resize observer
  const resizeObserver = new ResizeObserver(() => {
    scheduleFit(instance);
  });
  resizeObserver.observe(wrapper);
  instance.resizeObserver = resizeObserver;

  // Handle input
  xterm.onData((data) => {
    queueInput(instance, data);
  });

  // Handle resize
  xterm.onResize((size) => {
    sendResize(instance, size.cols, size.rows);
  });

  // Focus terminal
  xterm.focus();

  activeInstance = instance;
  setStatus(`Connected: ${sessionId}`);
}

function connectWebSocket(instance: TerminalInstance): void {
  const wsUrl = buildWsUrl(instance.sessionId, instance.xterm.cols, instance.xterm.rows);
  const socket = new WebSocket(wsUrl);
  socket.binaryType = "arraybuffer";
  instance.socket = socket;

  socket.addEventListener("open", () => {
    setStatus(`Live: ${instance.sessionId}`);
    // Send current dimensions so the PTY learns the correct size.
    // The initial fitAddon.fit() fires before the socket is open,
    // so the resize event from that fit is lost.
    sendResize(instance, instance.xterm.cols, instance.xterm.rows);
  });

  socket.addEventListener("message", (event) => {
    if (typeof event.data === "string") {
      const parsed = parseWsServerEvent(event.data);
      if (parsed === null) return;

      switch (parsed.type) {
        case "snapshot":
          instance.xterm.write(parsed.output_tail);
          setStatus(`Live: ${instance.sessionId} (${parsed.state})`);
          // Re-fit after snapshot so programs like tmux get the correct size
          scheduleFit(instance);
          break;
        case "exit":
          instance.xterm.write(
            `\r\n\x1b[90m[session exited: ${parsed.state}, code=${String(parsed.exit_code)}]\x1b[0m\r\n`,
          );
          setStatus(`Closed: ${instance.sessionId}`);
          break;
        case "error":
          instance.xterm.write(`\r\n\x1b[31m[error] ${parsed.message}\x1b[0m\r\n`);
          break;
      }
      return;
    }

    if (event.data instanceof ArrayBuffer) {
      instance.xterm.write(new Uint8Array(event.data));
    }
  });

  socket.addEventListener("close", () => {
    if (activeInstance === instance) {
      setStatus(`Disconnected: ${instance.sessionId}`);
    }
  });

  socket.addEventListener("error", () => {
    setStatus(`Socket error: ${instance.sessionId}`);
  });
}

function queueInput(instance: TerminalInstance, data: string): void {
  instance.inputQueue.push(TEXT_ENCODER.encode(data));
  if (instance.inputTimer === null) {
    instance.inputTimer = setTimeout(() => flushInput(instance), INPUT_FLUSH_MS);
  }
}

function flushInput(instance: TerminalInstance): void {
  instance.inputTimer = null;
  if (instance.socket === null || instance.socket.readyState !== WebSocket.OPEN) return;
  const batch = concatBytes(instance.inputQueue);
  instance.inputQueue.length = 0;
  if (batch.byteLength > 0) {
    instance.socket.send(batch);
  }
}

function sendResize(instance: TerminalInstance, cols: number, rows: number): void {
  if (instance.socket === null || instance.socket.readyState !== WebSocket.OPEN) return;
  instance.socket.send(serializeWsClientEvent({ type: "resize", cols, rows }));
}

let fitTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleFit(instance: TerminalInstance): void {
  if (fitTimer !== null) clearTimeout(fitTimer);
  fitTimer = setTimeout(() => {
    fitTimer = null;
    try {
      instance.fitAddon.fit();
    } catch {
      // ignore fit errors during teardown
    }
  }, 50);
}

function teardownActiveInstance(): void {
  if (activeInstance === null) return;

  if (activeInstance.inputTimer !== null) {
    clearTimeout(activeInstance.inputTimer);
    flushInput(activeInstance);
  }
  if (activeInstance.socket !== null) {
    activeInstance.socket.close();
  }
  if (activeInstance.resizeObserver !== null) {
    activeInstance.resizeObserver.disconnect();
  }
  activeInstance.xterm.dispose();
  activeInstance = null;

  if (terminalContainer !== null) {
    terminalContainer.replaceChildren();
  }
}

type AgentPreset = { label: string; command: string; cssClass: string };

const AGENT_PRESETS: AgentPreset[] = [
  { label: "Claude", command: "claude", cssClass: "preset-icon-claude" },
  { label: "Codex", command: "codex", cssClass: "preset-icon-codex" },
  { label: "OpenCode", command: "opencode", cssClass: "preset-icon-opencode" },
  { label: "Copilot", command: "copilot", cssClass: "preset-icon-copilot" },
];

async function launchPreset(preset: AgentPreset): Promise<void> {
  const worktreePath = state.selectedWorktreePath;
  if (worktreePath === null) {
    setStatus("Select a worktree first");
    return;
  }

  try {
    const result = await apiCreateTerminal(
      worktreePath,
      120,
      35,
      preset.label.toLowerCase(),
      preset.command,
    );
    setActiveSession(result.sessionId);
    await refresh();
    activateSession(result.sessionId);
  } catch (error) {
    setStatus(
      `Failed: ${error instanceof Error ? error.message : "unknown error"}`,
    );
  }
}

async function openNewTerminal(): Promise<void> {
  const worktreePath = state.selectedWorktreePath;
  if (worktreePath === null) return;

  try {
    const result = await apiCreateTerminal(
      worktreePath,
      120,
      35,
      titleFromPath(worktreePath),
    );
    setActiveSession(result.sessionId);
    // Re-fetch sessions to get the new one in the list
    await refresh();
    activateSession(result.sessionId);
  } catch (error) {
    setStatus(
      `Failed: ${error instanceof Error ? error.message : "unknown error"}`,
    );
  }
}

function setStatus(text: string): void {
  if (statusEl !== null) {
    statusEl.textContent = text;
  }
}

function buildXtermTheme(): Record<string, string> {
  return {
    background: "#0f1115",
    foreground: "#e4e4e7",
    cursor: "#4ade80",
    cursorAccent: "#0f1115",
    selectionBackground: "rgba(74, 222, 128, 0.12)",
    black: "#27272a",
    red: "#f38ba8",
    green: "#a6e3a1",
    yellow: "#f9e2af",
    blue: "#89b4fa",
    magenta: "#cba6f7",
    cyan: "#89dceb",
    white: "#e4e4e7",
    brightBlack: "#71717a",
    brightRed: "#f38ba8",
    brightGreen: "#a6e3a1",
    brightYellow: "#f9e2af",
    brightBlue: "#89b4fa",
    brightMagenta: "#cba6f7",
    brightCyan: "#89dceb",
    brightWhite: "#ffffff",
  };
}

function terminalTabTitle(session: TerminalSession): string {
  const lastCommand = session.last_command?.trim() ?? "";
  if (lastCommand.length > 0) {
    return truncateWithEllipsis(lastCommand, TERMINAL_TAB_COMMAND_MAX_CHARS);
  }

  const title = session.title?.trim() ?? "";
  if (title.length > 0 && !title.startsWith("term-")) {
    return truncateWithEllipsis(title, TERMINAL_TAB_COMMAND_MAX_CHARS);
  }

  return "";
}

function truncateWithEllipsis(value: string, maxChars: number): string {
  if (maxChars <= 0) return "";
  const chars = Array.from(value);
  if (chars.length <= maxChars) return value;
  return `${chars.slice(0, maxChars - 1).join("")}\u2026`;
}

function concatBytes(chunks: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const chunk of chunks) {
    total += chunk.byteLength;
  }

  const output = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return output;
}

function registerOscGuards(xterm: Terminal): void {
  const guardedCodes = [4, 10, 11, 12, 104, 110, 111, 112];
  for (const code of guardedCodes) {
    xterm.parser.registerOscHandler(code, () => true);
  }
}

export function getActiveInstance(): TerminalInstance | null {
  return activeInstance;
}
