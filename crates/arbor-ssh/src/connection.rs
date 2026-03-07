use {
    arbor_core::{
        outpost::RemoteHost,
        remote::{RemoteCommandOutput, RemoteError, RemoteTransport},
    },
    libssh_rs::{AuthStatus, Channel, Session, SshOption},
    std::{
        collections::HashMap,
        io::{Read, Write},
        os::unix::net::UnixStream,
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

    /// Run a command with SSH agent forwarding.
    ///
    /// Proxies agent channel data between the remote and the local
    /// `SSH_AUTH_SOCK` socket so that commands like `git clone` can
    /// authenticate via the local SSH agent.
    pub fn run_command_with_agent_forwarding(
        &self,
        command: &str,
    ) -> Result<RemoteCommandOutput, RemoteError> {
        let auth_sock = std::env::var("SSH_AUTH_SOCK").map_err(|_| {
            RemoteError::Command("SSH_AUTH_SOCK is not set; cannot forward SSH agent".to_owned())
        })?;

        self.session.enable_accept_agent_forward(true);

        let channel = self
            .session
            .new_channel()
            .map_err(|e| RemoteError::Command(e.to_string()))?;
        channel
            .open_session()
            .map_err(|e| RemoteError::Command(e.to_string()))?;

        // Request agent forwarding — best-effort, some servers may reject.
        let _ = channel.request_auth_agent();

        channel
            .request_exec(command)
            .map_err(|e| RemoteError::Command(e.to_string()))?;

        let mut stdout_data = Vec::new();
        let mut stderr_data = Vec::new();
        let mut agent_proxies: Vec<(Channel, UnixStream)> = Vec::new();
        let mut buf = [0u8; 32768];

        loop {
            // Read command stdout (non-blocking).
            match channel.read_nonblocking(&mut buf, false) {
                Ok(n) if n > 0 => stdout_data.extend_from_slice(&buf[..n]),
                _ => {},
            }

            // Read command stderr (non-blocking).
            match channel.read_nonblocking(&mut buf, true) {
                Ok(n) if n > 0 => stderr_data.extend_from_slice(&buf[..n]),
                _ => {},
            }

            // Accept new agent forwarding channels from the server.
            while let Some(agent_chan) = self.session.accept_agent_forward() {
                if let Ok(sock) = UnixStream::connect(&auth_sock) {
                    let _ = sock.set_nonblocking(true);
                    agent_proxies.push((agent_chan, sock));
                }
            }

            // Proxy data between each agent channel and the local agent socket.
            let mut i = 0;
            while i < agent_proxies.len() {
                let (agent_chan, sock) = &mut agent_proxies[i];

                // Remote -> local agent socket.
                match agent_chan.read_nonblocking(&mut buf, false) {
                    Ok(n) if n > 0 => {
                        let _ = sock.write_all(&buf[..n]);
                    },
                    _ => {},
                }

                // Local agent socket -> remote.
                match sock.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        let _ = agent_chan.stdin().write_all(&buf[..n]);
                    },
                    Ok(0) | Err(_) => {},
                    _ => {},
                }

                if agent_chan.is_eof() || agent_chan.is_closed() {
                    agent_proxies.swap_remove(i);
                } else {
                    i += 1;
                }
            }

            if channel.is_eof() {
                break;
            }

            std::thread::sleep(Duration::from_millis(10));
        }

        let _ = channel.send_eof();
        let _ = channel.close();
        self.session.enable_accept_agent_forward(false);

        Ok(RemoteCommandOutput {
            stdout: String::from_utf8_lossy(&stdout_data).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_data).into_owned(),
            exit_code: channel.get_exit_status(),
        })
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
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return format!("{home}/{rest}");
    }
    path.to_owned()
}
