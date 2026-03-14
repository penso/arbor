import type { Repository, Worktree, TerminalSession, ChangedFile, ProcessInfo, AgentSession, AgentActivityWsEvent } from "./types";
import { fetchRepositories, fetchWorktrees, fetchTerminals, fetchChangedFiles, fetchProcesses } from "./api";

export type AppState = {
  repositories: Repository[];
  worktrees: Worktree[];
  sessions: TerminalSession[];
  changedFiles: ChangedFile[];
  processes: ProcessInfo[];
  agentSessions: AgentSession[];

  selectedRepoRoot: string | null;
  selectedWorktreePath: string | null;
  activeSessionId: string | null;

  loading: boolean;
  error: string | null;
};

export function createInitialState(): AppState {
  return {
    repositories: [],
    worktrees: [],
    sessions: [],
    changedFiles: [],
    processes: [],
    agentSessions: [],
    selectedRepoRoot: null,
    selectedWorktreePath: null,
    activeSessionId: null,
    loading: true,
    error: null,
  };
}

type Listener = () => void;

const listeners = new Set<Listener>();

export function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function notify(): void {
  for (const listener of listeners) {
    listener();
  }
}

export let state = createInitialState();

export function updateState(partial: Partial<AppState>): void {
  Object.assign(state, partial);
  notify();
}

let refreshInFlight = false;

export async function refresh(): Promise<void> {
  if (refreshInFlight) return;
  refreshInFlight = true;
  updateState({ loading: true, error: null });

  try {
    const [repositories, worktrees, sessions, processes] = await Promise.all([
      fetchRepositories(),
      fetchWorktrees(),
      fetchTerminals(),
      fetchProcesses().catch(() => [] as ProcessInfo[]),
    ]);

    // Validate selections still exist, auto-select on first load
    let selectedRepoRoot =
      state.selectedRepoRoot !== null &&
      repositories.some((r) => r.root === state.selectedRepoRoot)
        ? state.selectedRepoRoot
        : null;

    // Auto-select first repo on initial load
    if (selectedRepoRoot === null && repositories.length > 0) {
      selectedRepoRoot = repositories[0].root;
    }

    let selectedWorktreePath =
      state.selectedWorktreePath !== null &&
      worktrees.some((w) => w.path === state.selectedWorktreePath)
        ? state.selectedWorktreePath
        : null;

    // Auto-select primary worktree (or first) for the selected repo on initial load
    if (selectedWorktreePath === null && selectedRepoRoot !== null) {
      const repoWorktrees = worktrees.filter((w) => w.repo_root === selectedRepoRoot);
      const primary = repoWorktrees.find((w) => w.is_primary_checkout);
      const first = primary ?? repoWorktrees[0];
      if (first !== undefined) {
        selectedWorktreePath = first.path;
      }
    }

    let activeSessionId =
      state.activeSessionId !== null &&
      sessions.some((s) => s.session_id === state.activeSessionId)
        ? state.activeSessionId
        : null;

    // Auto-select first running terminal for the selected worktree
    const visibleSessions = selectedWorktreePath !== null
      ? sessions.filter((s) => s.workspace_id === selectedWorktreePath || s.cwd === selectedWorktreePath)
      : sessions;

    // Clear active session if it doesn't belong to the selected worktree
    if (activeSessionId !== null && selectedWorktreePath !== null) {
      const belongs = visibleSessions.some((s) => s.session_id === activeSessionId);
      if (!belongs) {
        activeSessionId = null;
      }
    }

    if (activeSessionId === null && visibleSessions.length > 0) {
      const running = visibleSessions.find((s) => s.state === "running");
      const first = running ?? visibleSessions[0];
      if (first !== undefined) {
        activeSessionId = first.session_id;
      }
    }

    updateState({
      repositories,
      worktrees,
      sessions,
      processes,
      selectedRepoRoot,
      selectedWorktreePath,
      activeSessionId,
      loading: false,
    });

    // Fetch changed files for selected worktree
    if (selectedWorktreePath !== null) {
      refreshChangedFiles(selectedWorktreePath);
    } else {
      updateState({ changedFiles: [] });
    }
  } catch (error) {
    updateState({
      loading: false,
      error: error instanceof Error ? error.message : "unknown request failure",
    });
  } finally {
    refreshInFlight = false;
  }
}

export function refreshChangedFiles(worktreePath: string): void {
  fetchChangedFiles(worktreePath)
    .then((changedFiles) => {
      if (state.selectedWorktreePath === worktreePath) {
        updateState({ changedFiles });
      }
    })
    .catch(() => {
      // Silently ignore change detection failures
    });
}

export function selectWorktree(path: string | null): void {
  const newPath = state.selectedWorktreePath === path ? null : path;

  // Auto-select a terminal for this worktree
  let activeSessionId = state.activeSessionId;
  if (newPath !== null) {
    const wtSessions = state.sessions.filter(
      (s) => s.workspace_id === newPath || s.cwd === newPath,
    );
    const running = wtSessions.find((s) => s.state === "running");
    const first = running ?? wtSessions[0];
    activeSessionId = first?.session_id ?? null;
  }

  updateState({ selectedWorktreePath: newPath, changedFiles: [], activeSessionId });
  if (newPath !== null) {
    refreshChangedFiles(newPath);
  }
}

export function setActiveSession(sessionId: string | null): void {
  updateState({ activeSessionId: sessionId });
}

export function filteredSessions(): TerminalSession[] {
  if (state.selectedWorktreePath === null) {
    return state.sessions;
  }
  return state.sessions.filter(
    (s) => s.workspace_id === state.selectedWorktreePath || s.cwd === state.selectedWorktreePath,
  );
}

// ── Agent activity WebSocket ─────────────────────────────────────────

const AGENT_RECONNECT_BASE_MS = 3000;
const AGENT_RECONNECT_MAX_MS = 30000;

function parseAgentSession(item: unknown): AgentSession | null {
  if (typeof item !== "object" || item === null || Array.isArray(item)) return null;
  const rec = item as Record<string, unknown>;
  const sessionId = typeof rec["session_id"] === "string" ? rec["session_id"] : null;
  const cwd = typeof rec["cwd"] === "string" ? rec["cwd"] : null;
  const s = typeof rec["state"] === "string" ? rec["state"] : null;
  const ts = typeof rec["updated_at_unix_ms"] === "number" ? rec["updated_at_unix_ms"] : null;
  if (sessionId === null || cwd === null || (s !== "working" && s !== "waiting") || ts === null) {
    return null;
  }
  return { session_id: sessionId, cwd, state: s, updated_at_unix_ms: ts };
}

function parseAgentWsEvent(data: string): AgentActivityWsEvent | null {
  let parsed: unknown;
  try { parsed = JSON.parse(data); } catch { return null; }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
  const rec = parsed as Record<string, unknown>;
  const eventType = typeof rec["type"] === "string" ? rec["type"] : null;

  if (eventType === "snapshot" && Array.isArray(rec["sessions"])) {
    const sessions: AgentSession[] = [];
    for (const item of rec["sessions"]) {
      const s = parseAgentSession(item);
      if (s !== null) sessions.push(s);
    }
    return { type: "snapshot", sessions };
  }
  if (eventType === "update") {
    const session = parseAgentSession(rec["session"]);
    if (session !== null) return { type: "update", session };
  }
  if (eventType === "clear" && typeof rec["session_id"] === "string") {
    return { type: "clear", session_id: rec["session_id"] };
  }
  return null;
}

function applyAgentEvent(event: AgentActivityWsEvent): void {
  if (event.type === "snapshot") {
    updateState({ agentSessions: event.sessions });
  } else if (event.type === "update") {
    const existing = state.agentSessions.filter(
      (s) => s.session_id !== event.session.session_id,
    );
    updateState({ agentSessions: [...existing, event.session] });
  } else {
    updateState({
      agentSessions: state.agentSessions.filter(
        (s) => s.session_id !== event.session_id,
      ),
    });
  }
}

export function startAgentActivityWs(): void {
  let delay = AGENT_RECONNECT_BASE_MS;

  function connect(): void {
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${protocol}//${window.location.host}/api/v1/agent/activity/ws`;
    const ws = new WebSocket(url);

    ws.addEventListener("open", () => {
      delay = AGENT_RECONNECT_BASE_MS;
    });

    ws.addEventListener("message", (msg) => {
      if (typeof msg.data !== "string") return;
      const event = parseAgentWsEvent(msg.data);
      if (event !== null) applyAgentEvent(event);
    });

    ws.addEventListener("close", () => {
      setTimeout(connect, delay);
      delay = Math.min(delay * 2, AGENT_RECONNECT_MAX_MS);
    });

    ws.addEventListener("error", () => {
      ws.close();
    });
  }

  connect();
}

/**
 * Find the agent state for a worktree path using longest-prefix matching,
 * mirroring the desktop GUI logic.
 */
export function agentStateForWorktree(worktreePath: string): "working" | "waiting" | null {
  let bestState: "working" | "waiting" | null = null;
  let bestLen = 0;

  for (const session of state.agentSessions) {
    if (!session.cwd.startsWith(worktreePath)) continue;
    if (worktreePath.length > bestLen) {
      bestState = session.state;
      bestLen = worktreePath.length;
      continue;
    }
    if (worktreePath.length === bestLen && session.state === "waiting") {
      bestState = "waiting";
    }
  }
  return bestState;
}
