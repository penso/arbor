use {
    arbor_core::{
        outpost::RemoteHost,
        remote::{RemoteCommandOutput, RemoteError, RemoteTransport},
    },
    libssh_rs::{AuthStatus, Session, SshOption},
    std::{
        collections::HashMap,
        io::Read,
        sync::{Arc, Mutex},
        time::Duration,
    },
    thiserror::Error,
};

const SSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Error)]
pub enum SshError {
    #[error("SSH session error: {0}")]
    Session(#[from] libssh_rs::Error),
    #[error("authentication failed for {user}@{hostname}: no accepted auth method")]
    AuthFailed { user: String, hostname: String },
    #[error("connection pool lock poisoned")]
    LockPoisoned,
}

pub struct SshConnection {
    session: Session,
}

impl SshConnection {
    pub fn connect(host: &RemoteHost) -> Result<Self, SshError> {
        let session = Session::new()?;

        session.set_option(SshOption::Hostname(host.hostname.clone()))?;
        session.set_option(SshOption::Port(host.port))?;
        session.set_option(SshOption::User(Some(host.user.clone())))?;
        session.set_option(SshOption::Timeout(SSH_CONNECT_TIMEOUT))?;
        session.set_option(SshOption::ProcessConfig(true))?;

        if let Some(identity_file) = &host.identity_file {
            let expanded = shellexpand_tilde(identity_file);
            session.set_option(SshOption::AddIdentity(expanded))?;
        }

        session.connect()?;

        // Try ssh-agent first, fall back to public key auto
        let auth_status = session.userauth_agent(None)?;
        if auth_status != AuthStatus::Success {
            let auto_status = session.userauth_public_key_auto(None, None)?;
            if auto_status != AuthStatus::Success {
                return Err(SshError::AuthFailed {
                    user: host.user.clone(),
                    hostname: host.hostname.clone(),
                });
            }
        }

        Ok(Self { session })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn is_connected(&self) -> bool {
        self.session.is_connected()
    }
}

impl RemoteTransport for SshConnection {
    fn run_command(&self, command: &str) -> Result<RemoteCommandOutput, RemoteError> {
        let channel = self
            .session
            .new_channel()
            .map_err(|e| RemoteError::Command(e.to_string()))?;
        channel
            .open_session()
            .map_err(|e| RemoteError::Command(e.to_string()))?;
        channel
            .request_exec(command)
            .map_err(|e| RemoteError::Command(e.to_string()))?;

        let mut stdout = String::new();
        channel
            .stdout()
            .read_to_string(&mut stdout)
            .map_err(|e| RemoteError::Io(e.to_string()))?;

        let mut stderr = String::new();
        channel
            .stderr()
            .read_to_string(&mut stderr)
            .map_err(|e| RemoteError::Io(e.to_string()))?;

        let _ = channel.send_eof();
        let _ = channel.close();

        Ok(RemoteCommandOutput {
            stdout,
            stderr,
            exit_code: channel.get_exit_status(),
        })
    }

    fn is_connected(&self) -> bool {
        self.session.is_connected()
    }
}

pub struct SshConnectionPool {
    connections: Mutex<HashMap<String, Arc<Mutex<Option<SshConnection>>>>>,
}

impl SshConnectionPool {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_or_connect(
        &self,
        host: &RemoteHost,
    ) -> Result<Arc<Mutex<Option<SshConnection>>>, SshError> {
        let mut pool = self
            .connections
            .lock()
            .map_err(|_| SshError::LockPoisoned)?;

        let entry = pool
            .entry(host.name.clone())
            .or_insert_with(|| Arc::new(Mutex::new(None)));

        let conn_slot = Arc::clone(entry);

        {
            let mut guard = conn_slot.lock().map_err(|_| SshError::LockPoisoned)?;
            if guard.as_ref().is_none_or(|conn| !conn.is_connected()) {
                *guard = Some(SshConnection::connect(host)?);
            }
        }

        Ok(conn_slot)
    }

    pub fn disconnect(&self, host_name: &str) -> Result<(), SshError> {
        let mut pool = self
            .connections
            .lock()
            .map_err(|_| SshError::LockPoisoned)?;
        pool.remove(host_name);
        Ok(())
    }

    pub fn disconnect_all(&self) -> Result<(), SshError> {
        let mut pool = self
            .connections
            .lock()
            .map_err(|_| SshError::LockPoisoned)?;
        pool.clear();
        Ok(())
    }
}

impl Default for SshConnectionPool {
    fn default() -> Self {
        Self::new()
    }
}

fn shellexpand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_owned()
}
