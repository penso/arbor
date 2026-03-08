export type Repository = {
  root: string;
  label: string;
  github_repo_slug: string | null;
  avatar_url: string | null;
};

export type Worktree = {
  repo_root: string;
  path: string;
  branch: string;
  is_primary_checkout: boolean;
  last_activity_unix_ms: number | null;
  diff_additions: number | null;
  diff_deletions: number | null;
  pr_number: number | null;
  pr_url: string | null;
};

export type TerminalState = "running" | "completed" | "failed";

export type TerminalSession = {
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

export type ChangedFile = {
  path: string;
  kind: ChangeKind;
  additions: number;
  deletions: number;
};

export type ChangeKind =
  | "added"
  | "modified"
  | "removed"
  | "renamed"
  | "copied"
  | "type-change"
  | "conflict"
  | "intent-to-add";

export type ProcessStatus = "running" | "restarting" | "crashed" | "stopped";

export type ProcessInfo = {
  name: string;
  command: string;
  status: ProcessStatus;
  exit_code: number | null;
  restart_count: number;
  session_id: string | null;
};

export type AgentSession = {
  cwd: string;
  state: "working" | "waiting";
  updated_at_unix_ms: number;
};

export type WsClientEvent =
  | { type: "input"; data: string }
  | { type: "resize"; cols: number; rows: number }
  | { type: "signal"; signal: "interrupt" | "terminate" | "kill" }
  | { type: "detach" };

export type WsServerEvent =
  | { type: "snapshot"; output_tail: string; state: TerminalState; exit_code: number | null; updated_at_unix_ms: number | null }
  | { type: "output"; data: string }
  | { type: "exit"; state: TerminalState; exit_code: number | null }
  | { type: "error"; message: string };
