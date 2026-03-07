use thiserror::Error;

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("command execution failed: {0}")]
    Command(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("shell error: {0}")]
    Shell(String),
}

#[derive(Debug)]
pub struct RemoteCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// A protocol-agnostic remote connection (SSH, mosh, etc.).
pub trait RemoteTransport {
    fn run_command(&self, command: &str) -> Result<RemoteCommandOutput, RemoteError>;
    fn is_connected(&self) -> bool;
}

/// A protocol-agnostic interactive remote shell.
pub trait RemoteShell {
    fn write_input(&self, input: &[u8]) -> Result<(), RemoteError>;
    fn read_available(&self) -> Result<Vec<u8>, RemoteError>;
    fn resize(&self, cols: u32, rows: u32) -> Result<(), RemoteError>;
    fn is_closed(&self) -> bool;
    fn close(&self) -> Result<(), RemoteError>;
}

/// Provision a remote outpost (clone repo, detect daemon, etc.).
pub trait RemoteProvisioner {
    fn provision(
        &self,
        clone_url: &str,
        outpost_label: &str,
        branch: &str,
    ) -> Result<ProvisionResult, RemoteError>;
}

pub struct ProvisionResult {
    pub remote_path: String,
    pub has_remote_daemon: bool,
}
