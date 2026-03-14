use {
    arbor_core::{SessionId, WorkspaceId, daemon::DaemonSessionRecord, process::ProcessInfo},
    schemars::JsonSchema,
    serde::{Deserialize, Deserializer, Serialize},
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

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct AgentSessionDto {
    pub session_id: String,
    pub cwd: String,
    pub state: String,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct AgentSessionDtoWire {
    session_id: Option<String>,
    cwd: String,
    state: String,
    updated_at_unix_ms: u64,
}

fn legacy_agent_session_id(cwd: &str) -> String {
    format!("legacy-cwd:{cwd}")
}

impl<'de> Deserialize<'de> for AgentSessionDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = AgentSessionDtoWire::deserialize(deserializer)?;
        Ok(Self {
            session_id: wire
                .session_id
                .unwrap_or_else(|| legacy_agent_session_id(&wire.cwd)),
            cwd: wire.cwd,
            state: wire.state,
            updated_at_unix_ms: wire.updated_at_unix_ms,
        })
    }
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use crate::AgentSessionDto;

    #[test]
    fn agent_session_dto_deserializes_legacy_payload_without_session_id() {
        let dto: AgentSessionDto = serde_json::from_value(serde_json::json!({
            "cwd": "/tmp/repo/worktree",
            "state": "working",
            "updated_at_unix_ms": 42_u64,
        }))
        .expect("legacy payload should deserialize");

        assert_eq!(dto.session_id, "legacy-cwd:/tmp/repo/worktree");
        assert_eq!(dto.cwd, "/tmp/repo/worktree");
        assert_eq!(dto.state, "working");
        assert_eq!(dto.updated_at_unix_ms, 42);
    }

    #[test]
    fn agent_session_dto_preserves_explicit_session_id() {
        let dto: AgentSessionDto = serde_json::from_value(serde_json::json!({
            "session_id": "terminal:daemon-1",
            "cwd": "/tmp/repo/worktree",
            "state": "waiting",
            "updated_at_unix_ms": 99_u64,
        }))
        .expect("payload with session_id should deserialize");

        assert_eq!(dto.session_id, "terminal:daemon-1");
        assert_eq!(dto.cwd, "/tmp/repo/worktree");
        assert_eq!(dto.state, "waiting");
        assert_eq!(dto.updated_at_unix_ms, 99);
    }
}
