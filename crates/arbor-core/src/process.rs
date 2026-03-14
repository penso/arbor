use {
    schemars::JsonSchema,
    serde::{Deserialize, Serialize},
};

/// Status of a managed process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessStatus {
    Running,
    Restarting,
    Crashed,
    Stopped,
}

/// Source of a managed process definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessSource {
    ArborToml,
    Procfile,
}

/// Runtime information about a managed process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessInfo {
    pub id: String,
    pub name: String,
    pub command: String,
    pub repo_root: String,
    pub workspace_id: String,
    pub source: ProcessSource,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub restart_count: u32,
    /// Resident memory for the process tree rooted at this managed process.
    pub memory_bytes: Option<u64>,
    /// Links to a terminal daemon session, if any.
    pub session_id: Option<String>,
}
