import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { el, titleFromPath } from "../utils";
import {
  state,
  subscribe,
  setActiveSession,
  filteredSessions,
  filteredAgentChatSessions,
  refresh,
} from "../state";
import {
  createTerminal as apiCreateTerminal,
  killTerminal as apiKillTerminal,
  createAgentChat,
  killAgentChat,
  buildWsUrl,
  parseWsServerEvent,
  serializeWsClientEvent,
} from "../api";
import type { TerminalSession, AgentChatSession, ThemeResponse } from "../types";
import { createAgentPanel, activateAgentSession, deactivateAgentSession } from "./agent-panel";

const INPUT_FLUSH_MS = 16;
const TERMINAL_TAB_COMMAND_MAX_CHARS = 14;
const TEXT_ENCODER = new TextEncoder();

// ── Tab types ────────────────────────────────────────────────────────

// "terminal:session_id" or "agent:session_id"
type TabId = string;

function terminalTabId(sessionId: string): TabId { return `terminal:${sessionId}`; }
function agentTabId(sessionId: string): TabId { return `agent:${sessionId}`; }
function parseTabId(id: TabId): { kind: "terminal" | "agent"; sessionId: string } | null {
  if (id.startsWith("terminal:")) return { kind: "terminal", sessionId: id.slice(9) };
  if (id.startsWith("agent:")) return { kind: "agent", sessionId: id.slice(6) };
  return null;
}

// ── Terminal instance ────────────────────────────────────────────────

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
let activeTabId: TabId | null = null;
let panel: HTMLElement | null = null;
let tabsContainer: HTMLElement | null = null;
let terminalContainer: HTMLElement | null = null;
let agentContainer: HTMLElement | null = null;
let statusEl: HTMLElement | null = null;

// Agents that should use integrated chat UI instead of terminal
const AGENT_CHAT_KINDS = new Set(["claude", "codex"]);

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

  // Terminal container (for xterm)
  terminalContainer = el("div", "terminal-container");

  // Agent container (for chat UI)
  agentContainer = createAgentPanel();
  agentContainer.style.display = "none";

  // Status bar
  statusEl = el("div", "terminal-status");

  panel.append(toolbar, terminalContainer, agentContainer, statusEl);

  subscribe(renderTabs);
  renderTabs();

  return panel;
}

function renderTabs(): void {
  if (tabsContainer === null) return;
  tabsContainer.replaceChildren();

  const sessions = filteredSessions();
  const agentChats = filteredAgentChatSessions();
  const totalTabs = sessions.length + agentChats.length;

  if (totalTabs === 0) {
    teardownActiveTab();
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

  // Determine the current active tab
  const currentActiveTabId = resolveActiveTab(sessions, agentChats);

  // Auto-activate if needed
  if (currentActiveTabId !== null && currentActiveTabId !== activeTabId) {
    setTimeout(() => activateTab(currentActiveTabId), 0);
  }

  // Render terminal tabs
  for (const session of sessions) {
    const tabId = terminalTabId(session.session_id);
    const tab = el("button", "terminal-tab");
    if (tabId === currentActiveTabId) {
      tab.classList.add("active");
    }

    const stateIndicator = el("span", "terminal-tab-indicator");
    if (session.state === "running") stateIndicator.classList.add("running");
    else if (session.state === "completed") stateIndicator.classList.add("completed");
    else if (session.state === "failed") stateIndicator.classList.add("failed");

    const icon = el("span", "terminal-tab-icon");
    icon.setAttribute("aria-hidden", "true");

    const label = el("span", "terminal-tab-label", terminalTabTitle(session));

    const closeBtn = el("span", "terminal-tab-close", "\u00d7");
    closeBtn.title = "Close terminal";
    closeBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      closeTerminal(session.session_id);
    });

    tab.append(stateIndicator, icon, label, closeBtn);
    tab.addEventListener("click", () => activateTab(tabId));
    tabsContainer.append(tab);
  }

  // Render agent chat tabs
  for (const chat of agentChats) {
    const tabId = agentTabId(chat.id);
    const tab = el("button", "terminal-tab");
    if (tabId === currentActiveTabId) {
      tab.classList.add("active");
    }

    const stateIndicator = el("span", "terminal-tab-indicator");
    if (chat.status === "working") stateIndicator.classList.add("running");
    else if (chat.status === "idle") stateIndicator.classList.add("completed");
    else if (chat.status === "exited") stateIndicator.classList.add("failed");

    const icon = el("span", "terminal-tab-icon");
    icon.textContent = "\u2728"; // sparkle for agent tabs
    icon.setAttribute("aria-hidden", "true");

    const agentLabel = chat.agent_kind.charAt(0).toUpperCase() + chat.agent_kind.slice(1);
    const label = el("span", "terminal-tab-label", agentLabel);

    const closeBtn = el("span", "terminal-tab-close", "\u00d7");
    closeBtn.title = "Close agent";
    closeBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      closeAgentChat(chat.id);
    });

    tab.append(stateIndicator, icon, label, closeBtn);
    tab.addEventListener("click", () => activateTab(tabId));
    tabsContainer.append(tab);
  }
}

function resolveActiveTab(
  sessions: TerminalSession[],
  agentChats: AgentChatSession[],
): TabId | null {
  // If we have an active tab that still exists, keep it
  if (activeTabId !== null) {
    const parsed = parseTabId(activeTabId);
    if (parsed !== null) {
      if (parsed.kind === "terminal" && sessions.some((s) => s.session_id === parsed.sessionId)) {
        return activeTabId;
      }
      if (parsed.kind === "agent" && agentChats.some((c) => c.id === parsed.sessionId)) {
        return activeTabId;
      }
    }
  }

  // Fall back to state.activeSessionId (terminal)
  if (state.activeSessionId !== null && sessions.some((s) => s.session_id === state.activeSessionId)) {
    return terminalTabId(state.activeSessionId);
  }

  // Auto-select first running terminal, then first agent chat
  const running = sessions.find((s) => s.state === "running");
  if (running !== undefined) return terminalTabId(running.session_id);
  const firstSession = sessions[0];
  if (firstSession !== undefined) return terminalTabId(firstSession.session_id);
  const firstAgent = agentChats[0];
  if (firstAgent !== undefined) return agentTabId(firstAgent.id);
  return null;
}

function activateTab(tabId: TabId): void {
  if (tabId === activeTabId) return;

  const parsed = parseTabId(tabId);
  if (parsed === null) return;

  teardownActiveTab();
  activeTabId = tabId;

  if (parsed.kind === "terminal") {
    // Show terminal, hide agent
    showTerminalView();
    setActiveSession(parsed.sessionId);
    createXtermInstance(parsed.sessionId);
  } else {
    // Show agent, hide terminal
    showAgentView();
    activateAgentSession(parsed.sessionId);
  }
}

function teardownActiveTab(): void {
  if (activeTabId === null) return;
  const parsed = parseTabId(activeTabId);
  if (parsed !== null && parsed.kind === "terminal") {
    teardownActiveInstance();
  } else {
    deactivateAgentSession();
  }
  activeTabId = null;
}

function showTerminalView(): void {
  if (terminalContainer !== null) terminalContainer.style.display = "";
  if (agentContainer !== null) agentContainer.style.display = "none";
  if (statusEl !== null) statusEl.style.display = "";
}

function showAgentView(): void {
  if (terminalContainer !== null) terminalContainer.style.display = "none";
  if (agentContainer !== null) agentContainer.style.display = "";
  if (statusEl !== null) statusEl.style.display = "none";
}

function renderEmptyState(text: string): void {
  showTerminalView();
  if (terminalContainer !== null) {
    terminalContainer.replaceChildren(
      el("div", "terminal-empty", text),
    );
  }
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

type AgentPreset = { label: string; command: string; cssClass: string; chatMode: boolean };

const AGENT_PRESETS: AgentPreset[] = [
  { label: "Claude", command: "claude", cssClass: "preset-icon-claude", chatMode: true },
  { label: "Codex", command: "codex", cssClass: "preset-icon-codex", chatMode: true },
  { label: "OpenCode", command: "opencode", cssClass: "preset-icon-opencode", chatMode: false },
  { label: "Copilot", command: "copilot", cssClass: "preset-icon-copilot", chatMode: false },
];

async function closeTerminal(sessionId: string): Promise<void> {
  try {
    if (activeTabId === terminalTabId(sessionId)) {
      teardownActiveTab();
    }
    await apiKillTerminal(sessionId);
    await refresh();

    // Activate another tab
    selectNextTab();
  } catch (error) {
    setStatus(
      `Failed to close: ${error instanceof Error ? error.message : "unknown error"}`,
    );
  }
}

async function closeAgentChat(chatId: string): Promise<void> {
  try {
    if (activeTabId === agentTabId(chatId)) {
      teardownActiveTab();
    }
    await killAgentChat(chatId);
    await refresh();

    selectNextTab();
  } catch (error) {
    setStatus(
      `Failed to close: ${error instanceof Error ? error.message : "unknown error"}`,
    );
  }
}

function selectNextTab(): void {
  const sessions = filteredSessions();
  const agentChats = filteredAgentChatSessions();

  if (sessions.length > 0) {
    const first = sessions[0];
    if (first !== undefined) {
      activateTab(terminalTabId(first.session_id));
      return;
    }
  }
  if (agentChats.length > 0) {
    const first = agentChats[0];
    if (first !== undefined) {
      activateTab(agentTabId(first.id));
      return;
    }
  }
  setActiveSession(null);
}

async function launchPreset(preset: AgentPreset): Promise<void> {
  const worktreePath = state.selectedWorktreePath;
  if (worktreePath === null) {
    setStatus("Select a worktree first");
    return;
  }

  try {
    if (preset.chatMode && AGENT_CHAT_KINDS.has(preset.command)) {
      // Launch integrated agent chat
      const result = await createAgentChat(worktreePath, preset.command);
      await refresh();
      activateTab(agentTabId(result.sessionId));
    } else {
      // Launch terminal session
      const result = await apiCreateTerminal(
        worktreePath,
        120,
        35,
        preset.label.toLowerCase(),
        preset.command,
      );
      setActiveSession(result.sessionId);
      await refresh();
      activateTab(terminalTabId(result.sessionId));
    }
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
    await refresh();
    activateTab(terminalTabId(result.sessionId));
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

let currentTheme: ThemeResponse | null = null;

function buildXtermTheme(): Record<string, string> {
  const t = currentTheme;
  if (t === null) {
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

  const p = t.palette;
  const isLight = t.is_light;

  const red = isLight ? "#cf222e" : "#f38ba8";
  const green = isLight ? "#2da44e" : "#a6e3a1";
  const yellow = isLight ? "#9a6700" : "#f9e2af";
  const blue = isLight ? "#0969da" : "#89b4fa";

  return {
    background: p.terminal_bg,
    foreground: p.text_primary,
    cursor: p.terminal_cursor,
    cursorAccent: p.terminal_bg,
    selectionBackground: p.terminal_selection_bg,
    selectionForeground: p.terminal_selection_fg,
    black: isLight ? "#383a42" : "#27272a",
    red,
    green,
    yellow,
    blue,
    magenta: isLight ? "#a626a4" : "#cba6f7",
    cyan: isLight ? "#0184bc" : "#89dceb",
    white: isLight ? "#696c77" : "#e4e4e7",
    brightBlack: isLight ? "#a0a1a7" : "#71717a",
    brightRed: red,
    brightGreen: green,
    brightYellow: yellow,
    brightBlue: blue,
    brightMagenta: isLight ? "#a626a4" : "#cba6f7",
    brightCyan: isLight ? "#0184bc" : "#89dceb",
    brightWhite: isLight ? "#383a42" : "#ffffff",
  };
}

/** Store the current theme and re-apply to active terminal. Call after applyTheme(). */
export function refreshTerminalTheme(theme: ThemeResponse): void {
  currentTheme = theme;
  if (activeInstance !== null) {
    activeInstance.xterm.options.theme = buildXtermTheme();
  }
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
