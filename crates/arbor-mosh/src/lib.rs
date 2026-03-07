pub mod detect;
pub mod handshake;
pub mod shell;

use thiserror::Error;
pub use {handshake::MoshHandshakeResult, shell::MoshShell};

#[derive(Debug, Error)]
pub enum MoshError {
    #[error("mosh-client is not installed locally")]
    ClientNotInstalled,
    #[error("mosh-server is not installed on remote host: {0}")]
    ServerNotInstalled(String),
    #[error("mosh-server failed to start: {0}")]
    ServerStartFailed(String),
    #[error("failed to parse MOSH CONNECT handshake: {0}")]
    HandshakeParseFailed(String),
    #[error("SSH error: {0}")]
    Ssh(String),
    #[error("PTY error: {0}")]
    Pty(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("mosh session closed")]
    Closed,
}
