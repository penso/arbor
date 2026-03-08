import type {
  Repository,
  Worktree,
  TerminalSession,
  TerminalState,
  ChangedFile,
  ChangeKind,
  ProcessInfo,
  ProcessStatus,
  WsServerEvent,
  WsClientEvent,
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

async function fetchJson(url: string): Promise<unknown> {
  const response = await fetch(url, { headers: { Accept: "application/json" } });
  if (!response.ok) {
    throw new Error(`request failed (${response.status}) for ${url}`);
  }
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

export type CreateTerminalResult = {
  isNew: boolean;
  sessionId: string;
};

export async function createTerminal(
  cwd: string,
  cols: number,
  rows: number,
  title?: string,
): Promise<CreateTerminalResult> {
  const response = await fetch("/api/v1/terminals", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      cwd,
      workspace_id: cwd,
      cols,
      rows,
      title,
    }),
  });
  if (!response.ok) {
    throw new Error(`failed to create terminal (${response.status})`);
  }
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
    case "output": {
      const outputData = readString(parsed["data"]);
      if (outputData === null) return null;
      return { type: "output", data: outputData };
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

export function buildWsUrl(sessionId: string): string {
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}/api/v1/terminals/${encodeURIComponent(sessionId)}/ws`;
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
  const name = readString(item["name"]);
  const command = readString(item["command"]);
  const status = parseProcessStatus(item["status"]);
  const restartCount = readNumber(item["restart_count"]);
  if (name === null || command === null || status === null || restartCount === null) {
    return null;
  }
  return {
    name,
    command,
    status,
    exit_code: readNumber(item["exit_code"]),
    restart_count: restartCount,
    session_id: readString(item["session_id"]),
  };
}

export async function fetchProcesses(): Promise<ProcessInfo[]> {
  const raw = await fetchJson("/api/v1/processes");
  if (!Array.isArray(raw)) throw new Error("processes payload is not an array");
  const processes: ProcessInfo[] = [];
  for (const item of raw) {
    const info = parseProcessInfo(item);
    if (info !== null) processes.push(info);
  }
  return processes;
}

async function postProcessAction(url: string): Promise<void> {
  const response = await fetch(url, {
    method: "POST",
    headers: { Accept: "application/json" },
  });
  if (!response.ok) {
    throw new Error(`process action failed (${response.status})`);
  }
}

export async function startAllProcesses(): Promise<void> {
  await postProcessAction("/api/v1/processes/start-all");
}

export async function stopAllProcesses(): Promise<void> {
  await postProcessAction("/api/v1/processes/stop-all");
}

export async function startProcess(name: string): Promise<void> {
  await postProcessAction(`/api/v1/processes/${encodeURIComponent(name)}/start`);
}

export async function stopProcess(name: string): Promise<void> {
  await postProcessAction(`/api/v1/processes/${encodeURIComponent(name)}/stop`);
}

export async function restartProcess(name: string): Promise<void> {
  await postProcessAction(`/api/v1/processes/${encodeURIComponent(name)}/restart`);
}
