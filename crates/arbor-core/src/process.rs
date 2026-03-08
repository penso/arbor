use serde::{Deserialize, Serialize};

/// Status of a managed process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessStatus {
    Running,
    Restarting,
    Crashed,
    Stopped,
}

/// Runtime information about a managed process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub name: String,
    pub command: String,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub restart_count: u32,
    /// Links to a terminal daemon session, if any.
    pub session_id: Option<String>,
}
