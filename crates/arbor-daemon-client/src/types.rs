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
pub struct IssueSourceDto {
    pub provider: String,
    pub label: String,
    pub repository: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IssueReviewKind {
    PullRequest,
    MergeRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct IssueReviewDto {
    pub kind: IssueReviewKind,
    pub label: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct IssueLabelDto {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct IssueTypeDto {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct IssueDto {
    pub id: String,
    pub display_id: String,
    pub title: String,
    pub state: String,
    pub url: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    pub suggested_worktree_name: String,
    pub updated_at: Option<String>,
    #[serde(default)]
    pub labels: Vec<IssueLabelDto>,
    #[serde(default)]
    pub issue_type: Option<IssueTypeDto>,
    pub linked_branch: Option<String>,
    pub linked_review: Option<IssueReviewDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct IssueListResponse {
    pub source: Option<IssueSourceDto>,
    pub issues: Vec<IssueDto>,
    pub notice: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct AgentSessionDtoWire {
    session_id: Option<String>,
    cwd: String,
    state: String,
    updated_at_unix_ms: u64,
    metadata: Option<serde_json::Value>,
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
            metadata: wire.metadata,
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
pub struct ManagedWorktreePreviewRequest {
    pub repo_root: String,
    pub worktree_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ManagedWorktreePreviewResponse {
    pub sanitized_worktree_name: String,
    pub branch: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CreateManagedWorktreeRequest {
    pub repo_root: String,
    pub worktree_name: String,
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

/// DTO for agent chat sessions returned by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct AgentChatSessionDto {
    pub id: String,
    pub agent_kind: String,
    pub workspace_path: String,
    pub status: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Human-readable transport label (e.g. "acp:claude", "openai:http://…").
    #[serde(default)]
    pub transport_label: String,
}

/// Transport used by an agent chat session.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentChatTransport {
    /// ACP agent via acpx CLI subprocess.
    Acp,
    /// OpenAI-compatible HTTP API (Ollama, LM Studio, OpenRouter, Requesty, etc.).
    OpenAiChat {
        base_url: String,
        api_key: Option<String>,
    },
}

/// Request to create a new agent chat session.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CreateAgentChatRequest {
    pub workspace_path: String,
    pub agent_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    /// Model identifier to pass via `--model` to acpx or as `model` in OpenAI requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Transport to use. Defaults to ACP if omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<AgentChatTransport>,
}

/// Response from creating an agent chat session.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CreateAgentChatResponse {
    pub session_id: String,
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
        assert!(dto.metadata.is_none());
    }

    #[test]
    fn agent_session_dto_deserializes_with_metadata() {
        let dto: AgentSessionDto = serde_json::from_value(serde_json::json!({
            "session_id": "sess-1",
            "cwd": "/tmp/test",
            "state": "working",
            "updated_at_unix_ms": 42_u64,
            "metadata": {
                "terminal": { "type": "tmux", "server": "my-project", "pane_id": "%42" },
                "git": { "branch": "feat/cool" }
            }
        }))
        .expect("payload with metadata should deserialize");

        let meta = dto.metadata.expect("metadata should be Some");
        assert_eq!(meta["terminal"]["type"], "tmux");
        assert_eq!(meta["terminal"]["server"], "my-project");
        assert_eq!(meta["terminal"]["pane_id"], "%42");
        assert_eq!(meta["git"]["branch"], "feat/cool");
    }

    #[test]
    fn agent_session_dto_without_metadata_omits_field_in_json() {
        let dto = AgentSessionDto {
            session_id: "s1".to_owned(),
            cwd: "/tmp".to_owned(),
            state: "idle".to_owned(),
            updated_at_unix_ms: 0,
            metadata: None,
        };
        let json = serde_json::to_value(&dto).expect("should serialize");
        assert!(
            json.get("metadata").is_none(),
            "metadata should be omitted when None"
        );
    }

    #[test]
    fn agent_session_dto_with_metadata_includes_field_in_json() {
        let dto = AgentSessionDto {
            session_id: "s2".to_owned(),
            cwd: "/tmp".to_owned(),
            state: "working".to_owned(),
            updated_at_unix_ms: 1,
            metadata: Some(serde_json::json!({"foo": "bar"})),
        };
        let json = serde_json::to_value(&dto).expect("should serialize");
        assert_eq!(json["metadata"]["foo"], "bar");
    }
}
