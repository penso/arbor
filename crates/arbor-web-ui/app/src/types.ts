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
  processes: ProcessInfo[];
};

export type RightPaneTab = "changes" | "files" | "processes" | "notes";

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

export type IssueSource = {
  provider: string;
  label: string;
  repository: string;
  url: string | null;
};

export type IssueReviewKind = "pull_request" | "merge_request";

export type IssueReview = {
  kind: IssueReviewKind;
  label: string;
  url: string | null;
};

export type Issue = {
  id: string;
  display_id: string;
  title: string;
  state: string;
  url: string | null;
  suggested_worktree_name: string;
  updated_at: string | null;
  linked_branch: string | null;
  linked_review: IssueReview | null;
};

export type IssueListResponse = {
  source: IssueSource | null;
  issues: Issue[];
  notice: string | null;
};

export type ManagedWorktreePreview = {
  sanitized_worktree_name: string;
  branch: string;
  path: string;
};

export type WorktreeMutationResponse = {
  repo_root: string;
  path: string;
  branch: string | null;
  deleted_branch: string | null;
  message: string;
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
export type ProcessSource = "arbor-toml" | "procfile";

export type ProcessInfo = {
  id: string;
  name: string;
  command: string;
  repo_root: string;
  workspace_id: string;
  source: ProcessSource;
  status: ProcessStatus;
  exit_code: number | null;
  restart_count: number;
  memory_bytes: number | null;
  session_id: string | null;
};

export type AgentSession = {
  session_id: string;
  cwd: string;
  state: "working" | "waiting";
  updated_at_unix_ms: number;
};

export type AgentActivityWsEvent =
  | { type: "snapshot"; sessions: AgentSession[] }
  | { type: "update"; session: AgentSession }
  | { type: "clear"; session_id: string };

export type ThemePalette = {
  chrome_bg: string;
  chrome_border: string;
  app_bg: string;
  sidebar_bg: string;
  terminal_bg: string;
  panel_bg: string;
  panel_active_bg: string;
  tab_bg: string;
  tab_active_bg: string;
  border: string;
  text_primary: string;
  text_muted: string;
  text_disabled: string;
  notice_bg: string;
  notice_text: string;
  accent: string;
  terminal_cursor: string;
  terminal_selection_bg: string;
  terminal_selection_fg: string;
};

export type ThemeResponse = {
  slug: string;
  label: string;
  is_light: boolean;
  palette: ThemePalette;
};

// ── Agent Chat types ─────────────────────────────────────────────────

export type AgentChatStatus = "idle" | "working" | "exited";

export type AgentChatSession = {
  id: string;
  agent_kind: string;
  workspace_path: string;
  status: AgentChatStatus;
  input_tokens: number;
  output_tokens: number;
};

export type ChatMessage = {
  role: string;
  content: string;
  tool_calls: string[];
};

export type AgentChatEvent =
  | { type: "message_chunk"; content: string }
  | { type: "thought_chunk"; content: string }
  | { type: "tool_call"; name: string; status: string }
  | { type: "turn_started" }
  | { type: "turn_completed" }
  | { type: "usage_update"; input_tokens: number; output_tokens: number }
  | { type: "error"; message: string }
  | { type: "session_exited"; exit_code: number | null }
  | { type: "snapshot"; messages: ChatMessage[]; status: AgentChatStatus; input_tokens: number; output_tokens: number }
  | { type: "user_message"; content: string }
  | { type: "status_update"; message: string };

export type WsClientEvent =
  | { type: "resize"; cols: number; rows: number }
  | { type: "signal"; signal: "interrupt" | "terminate" | "kill" }
  | { type: "detach" };

export type WsServerEvent =
  | { type: "snapshot"; output_tail: string; state: TerminalState; exit_code: number | null; updated_at_unix_ms: number | null }
  | { type: "exit"; state: TerminalState; exit_code: number | null }
  | { type: "error"; message: string };
