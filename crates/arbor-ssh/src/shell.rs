use {
    arbor_core::remote::{RemoteError, RemoteShell},
    libssh_rs::{Channel, Session},
    std::{
        io::Write,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    },
    thiserror::Error,
};

#[derive(Debug, Error)]
pub enum ShellError {
    #[error("channel error: {0}")]
    Channel(#[from] libssh_rs::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("shell output lock poisoned")]
    LockPoisoned,
    #[error("shell is closed")]
    Closed,
}

pub struct SshShell {
    channel: Channel,
    closed: Arc<AtomicBool>,
}

impl SshShell {
    pub fn open(session: &Session, cols: u32, rows: u32) -> Result<Self, ShellError> {
        let channel = session.new_channel()?;
        channel.open_session()?;
        channel.request_pty("xterm-256color", cols, rows)?;
        channel.request_shell()?;

        let closed = Arc::new(AtomicBool::new(false));

        Ok(Self { channel, closed })
    }

    pub fn write_input(&self, input: &[u8]) -> Result<(), ShellError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(ShellError::Closed);
        }
        self.channel.stdin().write_all(input)?;
        Ok(())
    }

    pub fn read_available(&self) -> Result<Vec<u8>, ShellError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(ShellError::Closed);
        }

        let mut buf = vec![0u8; 65536];
        let mut all_output = Vec::new();

        loop {
            match self.channel.read_nonblocking(&mut buf, false) {
                Ok(0) => break,
                Ok(n) => all_output.extend_from_slice(&buf[..n]),
                Err(error) => {
                    if all_output.is_empty() {
                        return Err(ShellError::Channel(error));
                    }
                    break;
                },
            }
        }

        Ok(all_output)
    }

    pub fn resize(&self, cols: u32, rows: u32) -> Result<(), ShellError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(ShellError::Closed);
        }
        self.channel.change_pty_size(cols, rows)?;
        Ok(())
    }

    pub fn is_eof(&self) -> bool {
        self.channel.is_eof()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed) || self.channel.is_closed()
    }

    pub fn close(&self) -> Result<(), ShellError> {
        self.closed.store(true, Ordering::Relaxed);
        if !self.channel.is_closed() {
            self.channel.close()?;
        }
        Ok(())
    }
}

impl RemoteShell for SshShell {
    fn write_input(&self, input: &[u8]) -> Result<(), RemoteError> {
        self.write_input(input)
            .map_err(|e| RemoteError::Shell(e.to_string()))
    }

    fn read_available(&self) -> Result<Vec<u8>, RemoteError> {
        self.read_available()
            .map_err(|e| RemoteError::Shell(e.to_string()))
    }

    fn resize(&self, cols: u32, rows: u32) -> Result<(), RemoteError> {
        self.resize(cols, rows)
            .map_err(|e| RemoteError::Shell(e.to_string()))
    }

    fn is_closed(&self) -> bool {
        self.is_closed()
    }

    fn close(&self) -> Result<(), RemoteError> {
        self.close().map_err(|e| RemoteError::Shell(e.to_string()))
    }
}
