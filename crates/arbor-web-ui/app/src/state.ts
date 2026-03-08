import type { Repository, Worktree, TerminalSession, ChangedFile, ProcessInfo } from "./types";
import { fetchRepositories, fetchWorktrees, fetchTerminals, fetchChangedFiles, fetchProcesses } from "./api";

export type AppState = {
  repositories: Repository[];
  worktrees: Worktree[];
  sessions: TerminalSession[];
  changedFiles: ChangedFile[];
  processes: ProcessInfo[];

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

    // Validate selections still exist
    const selectedRepoRoot =
      state.selectedRepoRoot !== null &&
      repositories.some((r) => r.root === state.selectedRepoRoot)
        ? state.selectedRepoRoot
        : null;

    const selectedWorktreePath =
      state.selectedWorktreePath !== null &&
      worktrees.some((w) => w.path === state.selectedWorktreePath)
        ? state.selectedWorktreePath
        : null;

    let activeSessionId =
      state.activeSessionId !== null &&
      sessions.some((s) => s.session_id === state.activeSessionId)
        ? state.activeSessionId
        : null;

    // Auto-select first running terminal if none is active
    if (activeSessionId === null && sessions.length > 0) {
      const running = sessions.find((s) => s.state === "running");
      const first = running ?? sessions[0];
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
