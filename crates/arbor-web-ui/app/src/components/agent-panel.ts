import { el } from "../utils";
import { state, subscribe } from "../state";
import {
  sendAgentMessage,
  cancelAgentChat,
  buildAgentChatWsUrl,
} from "../api";
import type { AgentChatEvent, AgentChatStatus, ChatMessage } from "../types";

// ── State ────────────────────────────────────────────────────────────

let panelEl: HTMLElement | null = null;
let messagesEl: HTMLElement | null = null;
let inputEl: HTMLTextAreaElement | null = null;
let sendBtn: HTMLButtonElement | null = null;
let cancelBtn: HTMLButtonElement | null = null;
let statusDot: HTMLElement | null = null;
let statusLabel: HTMLElement | null = null;
let tokenLabel: HTMLElement | null = null;

let activeSessionId: string | null = null;
let activeSocket: WebSocket | null = null;
let currentStatus: AgentChatStatus = "idle";
let stickyScroll = true;

// Accumulated assistant text for the current streaming message
let streamingAssistantText = "";
let streamingEl: HTMLElement | null = null;
let cursorEl: HTMLElement | null = null;

// Accumulated thinking text
let thinkingText = "";
let thinkingEl: HTMLElement | null = null;

// ── Public API ───────────────────────────────────────────────────────

export function createAgentPanel(): HTMLElement {
  panelEl = el("div", "agent-chat");

  messagesEl = el("div", "agent-chat-messages");
  messagesEl.addEventListener("scroll", () => {
    if (messagesEl === null) return;
    const threshold = 40;
    stickyScroll =
      messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight < threshold;
  });

  // Status bar
  const statusBar = el("div", "agent-status-bar");
  statusDot = el("span", "agent-status-dot idle");
  statusLabel = el("span", "", "Ready");
  tokenLabel = el("span", "agent-token-count", "0 / 0 tokens");
  statusBar.append(statusDot, statusLabel, tokenLabel);

  // Input area
  const inputArea = el("div", "agent-input-area");
  inputEl = document.createElement("textarea");
  inputEl.className = "agent-input-textarea";
  inputEl.placeholder = "Send a message\u2026";
  inputEl.rows = 1;
  inputEl.addEventListener("keydown", onInputKeydown);
  inputEl.addEventListener("input", autoResizeTextarea);

  sendBtn = document.createElement("button");
  sendBtn.className = "agent-send-btn";
  sendBtn.textContent = "Send";
  sendBtn.addEventListener("click", handleSend);

  cancelBtn = document.createElement("button");
  cancelBtn.className = "agent-cancel-btn";
  cancelBtn.textContent = "Cancel";
  cancelBtn.style.display = "none";
  cancelBtn.addEventListener("click", handleCancel);

  inputArea.append(inputEl, sendBtn, cancelBtn);

  panelEl.append(messagesEl, statusBar, inputArea);

  subscribe(onStateChange);

  return panelEl;
}

export function activateAgentSession(sessionId: string): void {
  if (activeSessionId === sessionId) return;
  deactivateAgentSession();

  activeSessionId = sessionId;
  streamingAssistantText = "";
  streamingEl = null;
  cursorEl = null;
  thinkingText = "";
  thinkingEl = null;
  currentStatus = "idle";

  if (messagesEl !== null) {
    messagesEl.replaceChildren();
  }

  connectAgentWs(sessionId);
  updateInputState();
}

export function deactivateAgentSession(): void {
  if (activeSocket !== null) {
    activeSocket.close();
    activeSocket = null;
  }
  activeSessionId = null;
  streamingEl = null;
  cursorEl = null;
  thinkingEl = null;
}

// ── WebSocket ────────────────────────────────────────────────────────

function connectAgentWs(sessionId: string): void {
  const url = buildAgentChatWsUrl(sessionId);
  const ws = new WebSocket(url);
  activeSocket = ws;

  ws.addEventListener("message", (event) => {
    if (typeof event.data !== "string") return;
    const parsed = parseAgentChatEvent(event.data);
    if (parsed !== null) {
      handleEvent(parsed);
    }
  });

  ws.addEventListener("close", () => {
    if (activeSocket === ws) {
      activeSocket = null;
      setStatus("exited", "Disconnected");
    }
  });

  ws.addEventListener("error", () => {
    ws.close();
  });
}

function parseAgentChatEvent(data: string): AgentChatEvent | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
  const rec = parsed as Record<string, unknown>;
  const eventType = typeof rec["type"] === "string" ? rec["type"] : null;
  if (eventType === null) return null;

  switch (eventType) {
    case "message_chunk":
      return { type: "message_chunk", content: String(rec["content"] ?? "") };
    case "thought_chunk":
      return { type: "thought_chunk", content: String(rec["content"] ?? "") };
    case "tool_call":
      return { type: "tool_call", name: String(rec["name"] ?? ""), status: String(rec["status"] ?? "") };
    case "turn_started":
      return { type: "turn_started" };
    case "turn_completed":
      return { type: "turn_completed" };
    case "usage_update":
      return { type: "usage_update", input_tokens: Number(rec["input_tokens"] ?? 0), output_tokens: Number(rec["output_tokens"] ?? 0) };
    case "error":
      return { type: "error", message: String(rec["message"] ?? "") };
    case "session_exited":
      return { type: "session_exited", exit_code: typeof rec["exit_code"] === "number" ? rec["exit_code"] : null };
    case "snapshot":
      return {
        type: "snapshot",
        messages: Array.isArray(rec["messages"]) ? parseChatMessages(rec["messages"]) : [],
        status: parseStatus(rec["status"]),
        input_tokens: Number(rec["input_tokens"] ?? 0),
        output_tokens: Number(rec["output_tokens"] ?? 0),
      };
    case "user_message":
      return { type: "user_message", content: String(rec["content"] ?? "") };
    case "status_update":
      return { type: "status_update", message: String(rec["message"] ?? "") };
    default:
      return null;
  }
}

function parseChatMessages(arr: unknown[]): ChatMessage[] {
  const result: ChatMessage[] = [];
  for (const item of arr) {
    if (typeof item === "object" && item !== null && !Array.isArray(item)) {
      const rec = item as Record<string, unknown>;
      result.push({
        role: String(rec["role"] ?? ""),
        content: String(rec["content"] ?? ""),
        tool_calls: Array.isArray(rec["tool_calls"]) ? rec["tool_calls"].map(String) : [],
      });
    }
  }
  return result;
}

function parseStatus(value: unknown): AgentChatStatus {
  if (value === "idle" || value === "working" || value === "exited") return value;
  return "idle";
}

// ── Event handling ───────────────────────────────────────────────────

function handleEvent(event: AgentChatEvent): void {
  switch (event.type) {
    case "snapshot":
      renderSnapshot(event.messages);
      setStatus(event.status, statusText(event.status));
      updateTokens(event.input_tokens, event.output_tokens);
      break;

    case "turn_started":
      setStatus("working", "Thinking\u2026");
      streamingAssistantText = "";
      streamingEl = null;
      cursorEl = null;
      thinkingText = "";
      thinkingEl = null;
      break;

    case "message_chunk":
      appendAssistantChunk(event.content);
      break;

    case "thought_chunk":
      appendThoughtChunk(event.content);
      break;

    case "tool_call":
      appendToolCall(event.name, event.status);
      break;

    case "turn_completed":
      finishStreaming();
      setStatus("idle", "Ready");
      break;

    case "usage_update":
      updateTokens(event.input_tokens, event.output_tokens);
      break;

    case "error":
      appendStatusMessage("Error: " + event.message);
      setStatus("idle", "Error");
      break;

    case "session_exited":
      finishStreaming();
      setStatus("exited", "Exited" + (event.exit_code !== null ? ` (${event.exit_code})` : ""));
      break;

    case "user_message":
      appendUserMessage(event.content);
      break;

    case "status_update":
      appendStatusMessage(event.message);
      break;
  }
}

// ── Rendering helpers ────────────────────────────────────────────────

function renderSnapshot(messages: ChatMessage[]): void {
  if (messagesEl === null) return;
  messagesEl.replaceChildren();
  streamingEl = null;
  cursorEl = null;
  thinkingEl = null;

  for (const msg of messages) {
    if (msg.role === "user") {
      appendUserMessage(msg.content);
    } else if (msg.role === "assistant") {
      const msgEl = el("div", "agent-msg agent-msg-assistant");
      renderMarkdownContent(msgEl, msg.content);
      messagesEl.append(msgEl);

      for (const toolName of msg.tool_calls) {
        appendToolCall(toolName, "done");
      }
    }
  }
  scrollToBottom();
}

function appendUserMessage(text: string): void {
  if (messagesEl === null) return;
  const msgEl = el("div", "agent-msg agent-msg-user");
  msgEl.textContent = text;
  messagesEl.append(msgEl);
  scrollToBottom();
}

function appendAssistantChunk(content: string): void {
  if (messagesEl === null) return;

  // Close any open thinking block
  if (thinkingEl !== null) {
    thinkingEl = null;
  }

  streamingAssistantText += content;

  if (streamingEl === null) {
    streamingEl = el("div", "agent-msg agent-msg-assistant");
    messagesEl.append(streamingEl);
  }

  // Re-render the full accumulated text
  streamingEl.replaceChildren();
  renderMarkdownContent(streamingEl, streamingAssistantText);

  // Add blinking cursor
  if (cursorEl === null) {
    cursorEl = el("span", "agent-cursor");
  }
  streamingEl.append(cursorEl);

  scrollToBottom();
}

function appendThoughtChunk(content: string): void {
  if (messagesEl === null) return;

  thinkingText += content;

  if (thinkingEl === null) {
    thinkingEl = el("div", "agent-thought");

    const toggle = el("div", "agent-thought-toggle");
    toggle.textContent = "\u25b6 Thinking\u2026";

    const body = el("div", "agent-thought-content");
    body.textContent = thinkingText;

    toggle.addEventListener("click", () => {
      body.classList.toggle("open");
      toggle.textContent = body.classList.contains("open")
        ? "\u25bc Thinking"
        : "\u25b6 Thinking\u2026";
    });

    thinkingEl.append(toggle, body);
    messagesEl.append(thinkingEl);
  } else {
    // Update the body content
    const body = thinkingEl.querySelector(".agent-thought-content");
    if (body !== null) {
      body.textContent = thinkingText;
    }
  }
  scrollToBottom();
}

function appendToolCall(name: string, status: string): void {
  if (messagesEl === null) return;

  const wrapper = el("div", "agent-tool-call");

  const header = el("div", "agent-tool-header");
  const chevron = el("span", "agent-tool-chevron", "\u25b6");
  const nameEl = el("span", "agent-tool-name", name);
  const statusEl = el("span", "agent-tool-status");
  statusEl.textContent = status;
  if (status === "running") statusEl.classList.add("running");
  else if (status === "done" || status === "completed") statusEl.classList.add("done");
  else if (status === "error") statusEl.classList.add("error");

  header.append(chevron, nameEl, statusEl);

  const body = el("div", "agent-tool-body");
  body.textContent = name;

  header.addEventListener("click", () => {
    chevron.classList.toggle("open");
    body.classList.toggle("open");
  });

  wrapper.append(header, body);
  messagesEl.append(wrapper);
  scrollToBottom();
}

function appendStatusMessage(text: string): void {
  if (messagesEl === null) return;
  const msgEl = el("div", "agent-status-message");
  msgEl.textContent = text;
  messagesEl.append(msgEl);
  scrollToBottom();
}

function finishStreaming(): void {
  if (cursorEl !== null) {
    cursorEl.remove();
    cursorEl = null;
  }
  streamingEl = null;
  streamingAssistantText = "";
  thinkingEl = null;
  thinkingText = "";
}

// ── Markdown rendering (safe, no innerHTML) ──────────────────────────

function renderMarkdownContent(container: HTMLElement, text: string): void {
  const lines = text.split("\n");
  let i = 0;
  let currentParagraph: HTMLElement | null = null;

  function flushParagraph(): void {
    if (currentParagraph !== null) {
      container.append(currentParagraph);
      currentParagraph = null;
    }
  }

  while (i < lines.length) {
    const line = lines[i] ?? "";

    // Fenced code block
    if (line.startsWith("```")) {
      flushParagraph();
      const lang = line.slice(3).trim();
      const codeLines: string[] = [];
      i++;
      while (i < lines.length) {
        const codeLine = lines[i] ?? "";
        if (codeLine.startsWith("```")) {
          i++;
          break;
        }
        codeLines.push(codeLine);
        i++;
      }
      container.append(createCodeBlock(codeLines.join("\n"), lang));
      continue;
    }

    // Empty line = paragraph break
    if (line.trim() === "") {
      flushParagraph();
      i++;
      continue;
    }

    // Heading
    const headingMatch = /^(#{1,6})\s+(.*)$/.exec(line);
    if (headingMatch !== null) {
      flushParagraph();
      const level = (headingMatch[1] ?? "").length;
      const tag = `h${Math.min(level, 6)}` as keyof HTMLElementTagNameMap;
      const heading = el(tag);
      appendInlineContent(heading, headingMatch[2] ?? "");
      container.append(heading);
      i++;
      continue;
    }

    // Unordered list item
    if (/^[-*]\s+/.test(line)) {
      flushParagraph();
      const ul = el("ul");
      while (i < lines.length && /^[-*]\s+/.test(lines[i] ?? "")) {
        const li = el("li");
        appendInlineContent(li, (lines[i] ?? "").replace(/^[-*]\s+/, ""));
        ul.append(li);
        i++;
      }
      container.append(ul);
      continue;
    }

    // Ordered list item
    if (/^\d+[.)]\s+/.test(line)) {
      flushParagraph();
      const ol = el("ol");
      while (i < lines.length && /^\d+[.)]\s+/.test(lines[i] ?? "")) {
        const li = el("li");
        appendInlineContent(li, (lines[i] ?? "").replace(/^\d+[.)]\s+/, ""));
        ol.append(li);
        i++;
      }
      container.append(ol);
      continue;
    }

    // Regular text — accumulate into paragraph
    if (currentParagraph === null) {
      currentParagraph = el("p");
    } else {
      currentParagraph.append(document.createTextNode(" "));
    }
    appendInlineContent(currentParagraph, line);
    i++;
  }

  flushParagraph();
}

function appendInlineContent(container: HTMLElement, text: string): void {
  // Process inline formatting: **bold**, *italic*, `code`, [link](url)
  const pattern = /(\*\*(.+?)\*\*|\*(.+?)\*|`([^`]+)`|\[([^\]]+)\]\(([^)]+)\))/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    // Text before match
    if (match.index > lastIndex) {
      container.append(document.createTextNode(text.slice(lastIndex, match.index)));
    }

    if (match[2] !== undefined) {
      // **bold**
      const bold = el("span", "agent-bold");
      bold.textContent = match[2];
      container.append(bold);
    } else if (match[3] !== undefined) {
      // *italic*
      const italic = el("span", "agent-italic");
      italic.textContent = match[3];
      container.append(italic);
    } else if (match[4] !== undefined) {
      // `code`
      const code = document.createElement("code");
      code.textContent = match[4];
      container.append(code);
    } else if (match[5] !== undefined && match[6] !== undefined) {
      // [text](url)
      const link = document.createElement("a");
      link.textContent = match[5];
      link.href = match[6];
      link.target = "_blank";
      link.rel = "noopener noreferrer";
      container.append(link);
    }

    lastIndex = match.index + (match[0] ?? "").length;
  }

  // Remaining text
  if (lastIndex < text.length) {
    container.append(document.createTextNode(text.slice(lastIndex)));
  }
}

function createCodeBlock(code: string, language: string): HTMLElement {
  const wrapper = el("div", "agent-code-block");

  const header = el("div", "agent-code-header");
  const langLabel = el("span", "", language || "code");
  const copyBtn = document.createElement("button");
  copyBtn.textContent = "Copy";
  copyBtn.addEventListener("click", () => {
    void navigator.clipboard.writeText(code).then(() => {
      copyBtn.textContent = "Copied!";
      setTimeout(() => { copyBtn.textContent = "Copy"; }, 1500);
    });
  });
  header.append(langLabel, copyBtn);

  const pre = document.createElement("pre");
  const codeEl = document.createElement("code");
  codeEl.textContent = code;
  pre.append(codeEl);

  wrapper.append(header, pre);
  return wrapper;
}

// ── Input handling ───────────────────────────────────────────────────

function onInputKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    handleSend();
  }
}

function autoResizeTextarea(): void {
  if (inputEl === null) return;
  inputEl.style.height = "auto";
  inputEl.style.height = Math.min(inputEl.scrollHeight, 160) + "px";
}

function handleSend(): void {
  if (inputEl === null || activeSessionId === null) return;
  const text = inputEl.value.trim();
  if (text.length === 0) return;

  inputEl.value = "";
  inputEl.style.height = "auto";
  appendUserMessage(text);

  void sendAgentMessage(activeSessionId, text).catch((error) => {
    appendStatusMessage(
      "Failed to send: " + (error instanceof Error ? error.message : "unknown error"),
    );
  });
}

function handleCancel(): void {
  if (activeSessionId === null) return;
  void cancelAgentChat(activeSessionId).catch(() => {
    // Ignore cancel failures
  });
}

// ── UI state ─────────────────────────────────────────────────────────

function setStatus(status: AgentChatStatus, label: string): void {
  currentStatus = status;
  if (statusDot !== null) {
    statusDot.className = "agent-status-dot " + status;
  }
  if (statusLabel !== null) {
    statusLabel.textContent = label;
  }
  updateInputState();
}

function updateTokens(input: number, output: number): void {
  if (tokenLabel !== null) {
    tokenLabel.textContent = formatTokenCount(input) + " / " + formatTokenCount(output) + " tokens";
  }
}

function formatTokenCount(count: number): string {
  if (count >= 1_000_000) return (count / 1_000_000).toFixed(1) + "M";
  if (count >= 1_000) return (count / 1_000).toFixed(1) + "k";
  return String(count);
}

function updateInputState(): void {
  const isWorking = currentStatus === "working";
  const isExited = currentStatus === "exited";

  if (inputEl !== null) {
    inputEl.disabled = isWorking || isExited;
  }
  if (sendBtn !== null) {
    sendBtn.disabled = isWorking || isExited;
    sendBtn.style.display = isWorking ? "none" : "";
  }
  if (cancelBtn !== null) {
    cancelBtn.style.display = isWorking ? "" : "none";
  }
}

function statusText(status: AgentChatStatus): string {
  switch (status) {
    case "idle": return "Ready";
    case "working": return "Thinking\u2026";
    case "exited": return "Exited";
  }
}

function scrollToBottom(): void {
  if (messagesEl !== null && stickyScroll) {
    requestAnimationFrame(() => {
      if (messagesEl !== null) {
        messagesEl.scrollTop = messagesEl.scrollHeight;
      }
    });
  }
}

function onStateChange(): void {
  // Re-render if the active agent chat session changes
  // (e.g. on refresh when the session list updates)
  void state;
}
