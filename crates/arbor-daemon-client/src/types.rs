use {
    arbor_core::{SessionId, WorkspaceId, daemon::DaemonSessionRecord, process::ProcessInfo},
    schemars::JsonSchema,
    serde::{Deserialize, Serialize},
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct RepositoryDto {
    pub root: String,
    pub label: String,
    pub github_repo_slug: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorktreeDto {
    pub repo_root: String,
    pub path: String,
    pub branch: String,
    pub is_primary_checkout: bool,
    pub last_activity_unix_ms: Option<u64>,
    pub diff_additions: Option<usize>,
    pub diff_deletions: Option<usize>,
    pub pr_number: Option<u64>,
    pub pr_url: Option<String>,
    pub processes: Vec<ProcessInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ChangedFileDto {
    pub path: String,
    pub kind: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AgentSessionDto {
    pub cwd: String,
    pub state: String,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CreateTerminalRequest {
    pub session_id: Option<SessionId>,
    pub workspace_id: Option<WorkspaceId>,
    pub cwd: String,
    pub shell: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub title: Option<String>,
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CreateTerminalResponse {
    pub is_new_session: bool,
    pub session: DaemonSessionRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TerminalResizeRequest {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct TerminalSignalRequest {
    pub signal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CreateWorktreeRequest {
    pub repo_root: String,
    pub path: String,
    pub branch: Option<String>,
    pub detach: Option<bool>,
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DeleteWorktreeRequest {
    pub repo_root: String,
    pub path: String,
    pub delete_branch: Option<bool>,
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct WorktreeMutationResponse {
    pub repo_root: String,
    pub path: String,
    pub branch: Option<String>,
    pub deleted_branch: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CommitWorktreeRequest {
    pub path: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PushWorktreeRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct GitActionResponse {
    pub path: String,
    pub branch: Option<String>,
    pub message: String,
    pub commit_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiError {
    pub(crate) error: String,
}
