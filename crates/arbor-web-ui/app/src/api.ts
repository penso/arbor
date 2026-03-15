import type {
  Repository,
  Worktree,
  TerminalSession,
  TerminalState,
  ChangedFile,
  ChangeKind,
  ProcessInfo,
  ProcessStatus,
  ProcessSource,
  Issue,
  IssueReview,
  IssueReviewKind,
  IssueListResponse,
  IssueSource,
  ManagedWorktreePreview,
  WorktreeMutationResponse,
  ThemeResponse,
  ThemePalette,
  WsServerEvent,
  WsClientEvent,
  AgentChatSession,
  ChatMessage,
} from "./types";

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

function parseTerminalState(value: unknown): TerminalState | null {
  if (value === "running" || value === "completed" || value === "failed") {
    return value;
  }
  return null;
}

function parseProcessSource(value: unknown): ProcessSource | null {
  if (value === "arbor-toml" || value === "procfile") {
    return value;
  }
  return null;
}

const VALID_CHANGE_KINDS = new Set<string>([
  "added", "modified", "removed", "renamed", "copied",
  "type-change", "conflict", "intent-to-add",
]);

function parseChangeKind(value: unknown): ChangeKind | null {
  if (typeof value === "string" && VALID_CHANGE_KINDS.has(value)) {
    return value as ChangeKind;
  }
  return null;
}

type RequestOptions = {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
};

async function request(url: string, options: RequestOptions = {}): Promise<Response> {
  const response = await fetch(url, options);
  if (response.status === 401) {
    window.location.href = "/login";
    throw new Error("authentication required");
  }
  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new Error(text || `request failed (${response.status}) for ${url}`);
  }
  return response;
}

async function fetchJson(url: string): Promise<unknown> {
  const response = await request(url, { headers: { Accept: "application/json" } });
  return response.json();
}

export async function fetchRepositories(): Promise<Repository[]> {
  const raw = await fetchJson("/api/v1/repositories");
  if (!Array.isArray(raw)) throw new Error("repositories payload is not an array");

  const repos: Repository[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const root = readString(item["root"]);
    const label = readString(item["label"]);
    if (root !== null && label !== null) {
      repos.push({
        root,
        label,
        github_repo_slug: readString(item["github_repo_slug"]),
        avatar_url: readString(item["avatar_url"]),
      });
    }
  }
  return repos;
}

export async function fetchWorktrees(repoRoot?: string): Promise<Worktree[]> {
  const url = repoRoot
    ? `/api/v1/worktrees?repo_root=${encodeURIComponent(repoRoot)}`
    : "/api/v1/worktrees";
  const raw = await fetchJson(url);
  if (!Array.isArray(raw)) throw new Error("worktrees payload is not an array");

  const worktrees: Worktree[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const repoRoot = readString(item["repo_root"]);
    const path = readString(item["path"]);
    const branch = readString(item["branch"]);
    const isPrimary = readBoolean(item["is_primary_checkout"]);
    if (repoRoot !== null && path !== null && branch !== null && isPrimary !== null) {
      worktrees.push({
        repo_root: repoRoot,
        path,
        branch,
        is_primary_checkout: isPrimary,
        last_activity_unix_ms: readNumber(item["last_activity_unix_ms"]),
        diff_additions: readNumber(item["diff_additions"]),
        diff_deletions: readNumber(item["diff_deletions"]),
        pr_number: readNumber(item["pr_number"]),
        pr_url: readString(item["pr_url"]),
        processes: Array.isArray(item["processes"])
          ? item["processes"]
            .map((process) => parseProcessInfo(process))
            .filter((process): process is ProcessInfo => process !== null)
          : [],
      });
    }
  }
  return worktrees;
}

export async function fetchTerminals(): Promise<TerminalSession[]> {
  const raw = await fetchJson("/api/v1/terminals");
  if (!Array.isArray(raw)) throw new Error("terminals payload is not an array");

  const sessions: TerminalSession[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const sessionId = readString(item["session_id"]);
    const workspaceId = readString(item["workspace_id"]);
    const cwd = readString(item["cwd"]);
    const shell = readString(item["shell"]);
    const cols = readNumber(item["cols"]);
    const rows = readNumber(item["rows"]);
    if (
      sessionId !== null && workspaceId !== null && cwd !== null &&
      shell !== null && cols !== null && rows !== null
    ) {
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
        updated_at_unix_ms: readNumber(item["updated_at_unix_ms"]),
      });
    }
  }
  return sessions;
}

export async function fetchChangedFiles(worktreePath: string): Promise<ChangedFile[]> {
  const raw = await fetchJson(
    `/api/v1/worktrees/changes?path=${encodeURIComponent(worktreePath)}`
  );
  if (!Array.isArray(raw)) throw new Error("changes payload is not an array");

  const files: ChangedFile[] = [];
  for (const item of raw) {
    if (!isRecord(item)) continue;
    const path = readString(item["path"]);
    const kind = parseChangeKind(item["kind"]);
    const additions = readNumber(item["additions"]);
    const deletions = readNumber(item["deletions"]);
    if (path !== null && kind !== null && additions !== null && deletions !== null) {
      files.push({ path, kind, additions, deletions });
    }
  }
  return files;
}

function parseIssueSource(item: unknown): IssueSource | null {
  if (!isRecord(item)) return null;
  const provider = readString(item["provider"]);
  const label = readString(item["label"]);
  const repository = readString(item["repository"]);
  if (provider === null || label === null || repository === null) {
    return null;
  }
  return {
    provider,
    label,
    repository,
    url: readString(item["url"]),
  };
}

function parseIssueReviewKind(value: unknown): IssueReviewKind | null {
  if (value === "pull_request" || value === "merge_request") {
    return value;
  }
  return null;
}

function parseIssueReview(item: unknown): IssueReview | null {
  if (!isRecord(item)) return null;
  const kind = parseIssueReviewKind(item["kind"]);
  const label = readString(item["label"]);
  if (kind === null || label === null) {
    return null;
  }
  return {
    kind,
    label,
    url: readString(item["url"]),
  };
}

function parseIssue(item: unknown): Issue | null {
  if (!isRecord(item)) return null;
  const id = readString(item["id"]);
  const displayId = readString(item["display_id"]);
  const title = readString(item["title"]);
  const state = readString(item["state"]);
  const suggestedWorktreeName = readString(item["suggested_worktree_name"]);
  if (
    id === null ||
    displayId === null ||
    title === null ||
    state === null ||
    suggestedWorktreeName === null
  ) {
    return null;
  }
  return {
    id,
    display_id: displayId,
    title,
    state,
    url: readString(item["url"]),
    suggested_worktree_name: suggestedWorktreeName,
    updated_at: readString(item["updated_at"]),
    linked_branch: readString(item["linked_branch"]),
    linked_review: parseIssueReview(item["linked_review"]),
  };
}

export async function fetchIssues(repoRoot: string): Promise<IssueListResponse> {
  const raw = await fetchJson(`/api/v1/issues?repo_root=${encodeURIComponent(repoRoot)}`);
  if (!isRecord(raw)) throw new Error("issues payload is not an object");

  const issues: Issue[] = [];
  const rawIssues = raw["issues"];
  if (Array.isArray(rawIssues)) {
    for (const item of rawIssues) {
      const issue = parseIssue(item);
      if (issue !== null) issues.push(issue);
    }
  }

  return {
    source: parseIssueSource(raw["source"]),
    issues,
    notice: readString(raw["notice"]),
  };
}

type ManagedWorktreePreviewRequest = {
  repo_root: string;
  worktree_name: string;
};

type CreateManagedWorktreeRequest = {
  repo_root: string;
  worktree_name: string;
};

async function postJson(url: string, body: unknown): Promise<unknown> {
  const response = await request(url, {
    method: "POST",
    headers: {
      Accept: "application/json",
      "Content-Type": "application/json",
    },
    body: JSON.stringify(body),
  });
  return response.json();
}

export async function previewManagedWorktree(
  repoRoot: string,
  worktreeName: string,
): Promise<ManagedWorktreePreview> {
  const payload: ManagedWorktreePreviewRequest = {
    repo_root: repoRoot,
    worktree_name: worktreeName,
  };
  const raw = await postJson("/api/v1/worktrees/managed/preview", payload);
  if (!isRecord(raw)) throw new Error("managed worktree preview payload is not an object");
  const sanitizedWorktreeName = readString(raw["sanitized_worktree_name"]);
  const branch = readString(raw["branch"]);
  const path = readString(raw["path"]);
  if (sanitizedWorktreeName === null || branch === null || path === null) {
    throw new Error("managed worktree preview is missing required fields");
  }
  return {
    sanitized_worktree_name: sanitizedWorktreeName,
    branch,
    path,
  };
}

export async function createManagedWorktree(
  repoRoot: string,
  worktreeName: string,
): Promise<WorktreeMutationResponse> {
  const payload: CreateManagedWorktreeRequest = {
    repo_root: repoRoot,
    worktree_name: worktreeName,
  };
  const raw = await postJson("/api/v1/worktrees/managed", payload);
  if (!isRecord(raw)) throw new Error("managed worktree response is not an object");

  const repoRootValue = readString(raw["repo_root"]);
  const path = readString(raw["path"]);
  const message = readString(raw["message"]);
  if (repoRootValue === null || path === null || message === null) {
    throw new Error("managed worktree response is missing required fields");
  }

  return {
    repo_root: repoRootValue,
    path,
    branch: readString(raw["branch"]),
    deleted_branch: readString(raw["deleted_branch"]),
    message,
  };
}

export type CreateTerminalResult = {
  isNew: boolean;
  sessionId: string;
};

export async function createTerminal(
  cwd: string,
  cols: number,
  rows: number,
  title?: string,
  command?: string,
): Promise<CreateTerminalResult> {
  const body: Record<string, unknown> = {
    cwd,
    workspace_id: cwd,
    cols,
    rows,
    title,
  };
  if (command !== undefined) {
    body["command"] = command;
  }
  const response = await request("/api/v1/terminals", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const payload: unknown = await response.json();
  if (!isRecord(payload) || !isRecord(payload["session"])) {
    throw new Error("unexpected create terminal response");
  }
  const sessionId = readString(payload["session"]["session_id"]);
  if (sessionId === null) throw new Error("missing session_id in response");
  const isNew = payload["is_new_session"] === true;
  return { isNew, sessionId };
}

export function parseWsServerEvent(data: string): WsServerEvent | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    return null;
  }
  if (!isRecord(parsed)) return null;

  const eventType = readString(parsed["type"]);
  if (eventType === null) return null;

  switch (eventType) {
    case "snapshot": {
      const outputTail = readString(parsed["output_tail"]);
      const state = parseTerminalState(parsed["state"]);
      if (outputTail === null || state === null) return null;
      return {
        type: "snapshot",
        output_tail: outputTail,
        state,
        exit_code: readNumber(parsed["exit_code"]),
        updated_at_unix_ms: readNumber(parsed["updated_at_unix_ms"]),
      };
    }
    case "exit": {
      const state = parseTerminalState(parsed["state"]);
      if (state === null) return null;
      return { type: "exit", state, exit_code: readNumber(parsed["exit_code"]) };
    }
    case "error": {
      const message = readString(parsed["message"]);
      if (message === null) return null;
      return { type: "error", message };
    }
    default:
      return null;
  }
}

export function serializeWsClientEvent(event: WsClientEvent): string {
  return JSON.stringify(event);
}

export function buildWsUrl(sessionId: string, cols?: number, rows?: number): string {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  let url = `${protocol}//${window.location.host}/api/v1/terminals/${encodeURIComponent(sessionId)}/ws`;
  if (cols !== undefined && rows !== undefined) {
    url += `?cols=${cols}&rows=${rows}`;
  }
  return url;
}

// ── Process management ───────────────────────────────────────────────

const VALID_PROCESS_STATUSES = new Set<string>([
  "running", "restarting", "crashed", "stopped",
]);

function parseProcessStatus(value: unknown): ProcessStatus | null {
  if (typeof value === "string" && VALID_PROCESS_STATUSES.has(value)) {
    return value as ProcessStatus;
  }
  return null;
}

function parseProcessInfo(item: unknown): ProcessInfo | null {
  if (!isRecord(item)) return null;
  const id = readString(item["id"]);
  const name = readString(item["name"]);
  const command = readString(item["command"]);
  const repoRoot = readString(item["repo_root"]);
  const workspaceId = readString(item["workspace_id"]);
  const source = parseProcessSource(item["source"]);
  const status = parseProcessStatus(item["status"]);
  const restartCount = readNumber(item["restart_count"]);
  if (
    id === null || name === null || command === null || repoRoot === null ||
    workspaceId === null || source === null || status === null || restartCount === null
  ) {
    return null;
  }
  return {
    id,
    name,
    command,
    repo_root: repoRoot,
    workspace_id: workspaceId,
    source,
    status,
    exit_code: readNumber(item["exit_code"]),
    restart_count: restartCount,
    memory_bytes: readNumber(item["memory_bytes"]),
    session_id: readString(item["session_id"]),
  };
}

async function postAction(url: string): Promise<void> {
  await request(url, { method: "POST", headers: { Accept: "application/json" } });
}

export async function startProcess(id: string): Promise<void> {
  await postAction(`/api/v1/processes/${encodeURIComponent(id)}/start`);
}

export async function stopProcess(id: string): Promise<void> {
  await postAction(`/api/v1/processes/${encodeURIComponent(id)}/stop`);
}

export async function restartProcess(id: string): Promise<void> {
  await postAction(`/api/v1/processes/${encodeURIComponent(id)}/restart`);
}

export async function killTerminal(sessionId: string): Promise<void> {
  await request(`/api/v1/terminals/${encodeURIComponent(sessionId)}`, {
    method: "DELETE",
    headers: { Accept: "application/json" },
  });
}

// ── Theme ────────────────────────────────────────────────────────────

const THEME_PALETTE_KEYS: (keyof ThemePalette)[] = [
  "chrome_bg", "chrome_border", "app_bg", "sidebar_bg", "terminal_bg",
  "panel_bg", "panel_active_bg", "tab_bg", "tab_active_bg", "border",
  "text_primary", "text_muted", "text_disabled", "notice_bg", "notice_text",
  "accent", "terminal_cursor", "terminal_selection_bg", "terminal_selection_fg",
];

function parseThemePalette(raw: unknown): ThemePalette | null {
  if (!isRecord(raw)) return null;
  for (const key of THEME_PALETTE_KEYS) {
    if (typeof raw[key] !== "string") return null;
  }
  return raw as unknown as ThemePalette;
}

export async function fetchTheme(): Promise<ThemeResponse | null> {
  try {
    const raw = await fetchJson("/api/v1/config/theme");
    if (!isRecord(raw)) return null;
    const slug = readString(raw["slug"]);
    const label = readString(raw["label"]);
    const isLight = readBoolean(raw["is_light"]);
    const palette = parseThemePalette(raw["palette"]);
    if (slug === null || label === null || isLight === null || palette === null) {
      return null;
    }
    return { slug, label, is_light: isLight, palette };
  } catch {
    return null;
  }
}

/**
 * Apply a theme palette to the document by setting CSS custom properties.
 * Maps ThemePalette fields to the CSS variables used throughout the web UI.
 */
export function applyTheme(theme: ThemeResponse): void {
  const root = document.documentElement;
  const p = theme.palette;

  // Color scheme (affects scrollbars, form controls, etc.)
  root.style.setProperty("color-scheme", theme.is_light ? "light" : "dark");

  // Background surfaces
  root.style.setProperty("--bg", p.app_bg);
  root.style.setProperty("--bg-surface", p.sidebar_bg);
  root.style.setProperty("--bg-surface2", p.panel_bg);
  root.style.setProperty("--bg-hover", p.panel_active_bg);
  root.style.setProperty("--bg-active", p.chrome_bg);

  // Text
  root.style.setProperty("--text", p.text_primary);
  root.style.setProperty("--text-muted", p.text_muted);
  root.style.setProperty("--text-faint", p.text_disabled);

  // Accent
  root.style.setProperty("--accent", p.accent);
  root.style.setProperty("--accent-hover", p.accent);
  root.style.setProperty("--accent-subtle", `${p.accent}1a`);

  // Semantic colors — use saturated variants appropriate for the background
  if (theme.is_light) {
    root.style.setProperty("--green", "#2da44e");
    root.style.setProperty("--red", "#cf222e");
    root.style.setProperty("--yellow", "#9a6700");
    root.style.setProperty("--blue", "#0969da");
  } else {
    root.style.setProperty("--green", "#a6e3a1");
    root.style.setProperty("--red", "#f38ba8");
    root.style.setProperty("--yellow", "#f9e2af");
    root.style.setProperty("--blue", "#89b4fa");
  }

  // Borders
  root.style.setProperty("--border", p.border);
  root.style.setProperty("--border-strong", p.chrome_border);

  // Scrollbar
  root.style.setProperty("--scrollbar-track", p.app_bg);
  root.style.setProperty("--scrollbar-thumb", p.border);
  root.style.setProperty("--scrollbar-thumb-hover", p.chrome_border);

  // Tab colors
  root.style.setProperty("--tab-bg", p.tab_bg);
  root.style.setProperty("--tab-active-bg", p.tab_active_bg);

  // Terminal-specific
  root.style.setProperty("--terminal-bg", p.terminal_bg);
  root.style.setProperty("--terminal-cursor", p.terminal_cursor);
  root.style.setProperty("--terminal-selection-bg", p.terminal_selection_bg);
  root.style.setProperty("--terminal-selection-fg", p.terminal_selection_fg);

  // Overlay/dialog colors adapt to theme
  root.style.setProperty("--overlay-bg", theme.is_light
    ? "rgba(0, 0, 0, 0.25)"
    : "rgba(6, 8, 12, 0.72)");
  root.style.setProperty("--dialog-bg", theme.is_light
    ? p.app_bg
    : `color-mix(in srgb, ${p.panel_bg} 96%, transparent)`);
  root.style.setProperty("--input-bg", theme.is_light
    ? p.sidebar_bg
    : `color-mix(in srgb, ${p.app_bg} 94%, transparent)`);

}

// ── Agent Chat API ───────────────────────────────────────────────────

export async function createAgentChat(
  workspacePath: string,
  agentKind: string,
  initialPrompt?: string,
): Promise<{ sessionId: string }> {
  const body: Record<string, string> = { workspace_path: workspacePath, agent_kind: agentKind };
  if (initialPrompt !== undefined) {
    body.initial_prompt = initialPrompt;
  }
  const response = await request("/api/v1/agent/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const data: unknown = await response.json();
  if (!isRecord(data) || typeof data.session_id !== "string") {
    throw new Error("invalid create agent chat response");
  }
  return { sessionId: data.session_id };
}

export async function fetchAgentChats(): Promise<AgentChatSession[]> {
  const raw = await fetchJson("/api/v1/agent/chat");
  if (!Array.isArray(raw)) return [];
  return raw.filter(isRecord).map((item) => ({
    id: String(item.id ?? ""),
    agent_kind: String(item.agent_kind ?? ""),
    workspace_path: String(item.workspace_path ?? ""),
    status: String(item.status ?? "idle") as AgentChatSession["status"],
    input_tokens: Number(item.input_tokens ?? 0),
    output_tokens: Number(item.output_tokens ?? 0),
  }));
}

export async function sendAgentMessage(sessionId: string, message: string): Promise<void> {
  await request(`/api/v1/agent/chat/${encodeURIComponent(sessionId)}/send`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ message }),
  });
}

export async function cancelAgentChat(sessionId: string): Promise<void> {
  await request(`/api/v1/agent/chat/${encodeURIComponent(sessionId)}/cancel`, {
    method: "POST",
  });
}

export async function killAgentChat(sessionId: string): Promise<void> {
  await request(`/api/v1/agent/chat/${encodeURIComponent(sessionId)}`, {
    method: "DELETE",
  });
}

export async function fetchAgentChatHistory(sessionId: string): Promise<ChatMessage[]> {
  const raw = await fetchJson(`/api/v1/agent/chat/${encodeURIComponent(sessionId)}/history`);
  if (!Array.isArray(raw)) return [];
  return raw.filter(isRecord).map((item) => ({
    role: String(item.role ?? ""),
    content: String(item.content ?? ""),
    tool_calls: Array.isArray(item.tool_calls) ? item.tool_calls.map(String) : [],
  }));
}

export function buildAgentChatWsUrl(sessionId: string): string {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}/api/v1/agent/chat/${encodeURIComponent(sessionId)}/ws`;
}
