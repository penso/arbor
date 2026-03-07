use {
    crate::connection::SshError,
    libssh_rs::Session,
    std::io::Read,
    thiserror::Error,
};

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("SSH error: {0}")]
    Ssh(#[from] SshError),
    #[error("channel error: {0}")]
    Channel(#[from] libssh_rs::Error),
    #[error("failed to read command output: {0}")]
    Io(#[from] std::io::Error),
}

pub fn run_command(session: &Session, command: &str) -> Result<CommandOutput, CommandError> {
    let channel = session.new_channel()?;
    channel.open_session()?;
    channel.request_exec(command)?;

    let mut stdout = String::new();
    channel.stdout().read_to_string(&mut stdout)?;

    let mut stderr = String::new();
    channel.stderr().read_to_string(&mut stderr)?;

    channel.send_eof()?;
    channel.close()?;

    let exit_code = channel.get_exit_status();

    Ok(CommandOutput {
        stdout,
        stderr,
        exit_code,
    })
}
