use {
    arbor_core::daemon::{
        CreateOrAttachRequest, CreateOrAttachResponse, DaemonSessionRecord, DaemonSessionStore,
        DaemonSessionStoreError, DetachRequest, JsonDaemonSessionStore, KillRequest, ResizeRequest,
        SignalRequest, SnapshotRequest, TerminalDaemon, TerminalSessionState, TerminalSignal,
        TerminalSnapshot, WriteRequest,
    },
    portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system},
    std::{
        collections::{HashMap, HashSet},
        io::{Read, Write},
        path::PathBuf,
        sync::{Arc, Mutex, MutexGuard},
        thread,
    },
    thiserror::Error,
    tokio::sync::broadcast,
};

const OUTPUT_TAIL_MAX_CHARS: usize = 24_000;
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 35;
const DAEMON_SESSION_PREFIX: &str = "daemon-";

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Output(String),
    Exit {
        exit_code: Option<i32>,
        state: TerminalSessionState,
    },
    Error(String),
}

#[derive(Debug, Error)]
pub enum LocalTerminalDaemonError {
    #[error("session `{session_id}` not found")]
    SessionNotFound { session_id: String },
    #[error("{message}")]
    Message { message: String },
    #[error("daemon session store error: {0}")]
    SessionStore(#[from] DaemonSessionStoreError),
}

impl LocalTerminalDaemonError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message {
            message: message.into(),
        }
    }
}

struct LiveSession {
    session_id: String,
    workspace_id: String,
    cwd: PathBuf,
    shell: String,
    cols: Arc<Mutex<u16>>,
    rows: Arc<Mutex<u16>>,
    title: Arc<Mutex<Option<String>>>,
    last_command: Arc<Mutex<Option<String>>>,
    pending_command: Arc<Mutex<String>>,
    output_tail: Arc<Mutex<String>>,
    exit_code: Arc<Mutex<Option<i32>>>,
    state: Arc<Mutex<TerminalSessionState>>,
    updated_at_unix_ms: Arc<Mutex<Option<u64>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Arc<Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>>,
    sender: broadcast::Sender<SessionEvent>,
}

impl LiveSession {
    fn from_request(request: CreateOrAttachRequest) -> Result<Arc<Self>, LocalTerminalDaemonError> {
        let shell = if request.shell.trim().is_empty() {
            default_shell()
        } else {
            request.shell
        };

        let cols = request.cols.max(2);
        let rows = request.rows.max(1);

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| {
                LocalTerminalDaemonError::message(format!(
                    "failed to create PTY for `{}`: {error}",
                    request.session_id
                ))
            })?;

        let mut command = CommandBuilder::new(shell.clone());
        if let Some(ref cmd) = request.command {
            command.arg("-c");
            command.arg(cmd);
        } else {
            command.arg("-l");
        }
        command.cwd(request.cwd.as_os_str());
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(command).map_err(|error| {
            LocalTerminalDaemonError::message(format!(
                "failed to spawn shell for `{}`: {error}",
                request.session_id
            ))
        })?;

        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader().map_err(|error| {
            LocalTerminalDaemonError::message(format!(
                "failed to clone PTY reader for `{}`: {error}",
                request.session_id
            ))
        })?;
        let writer = pair.master.take_writer().map_err(|error| {
            LocalTerminalDaemonError::message(format!(
                "failed to open PTY writer for `{}`: {error}",
                request.session_id
            ))
        })?;
        let master = pair.master;

        let (sender, _) = broadcast::channel(512);

        let session = Arc::new(Self {
            session_id: request.session_id,
            workspace_id: request.workspace_id,
            cwd: request.cwd,
            shell,
            cols: Arc::new(Mutex::new(cols)),
            rows: Arc::new(Mutex::new(rows)),
            title: Arc::new(Mutex::new(request.title)),
            last_command: Arc::new(Mutex::new(None)),
            pending_command: Arc::new(Mutex::new(String::new())),
            output_tail: Arc::new(Mutex::new(String::new())),
            exit_code: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(TerminalSessionState::Running)),
            updated_at_unix_ms: Arc::new(Mutex::new(current_unix_timestamp_millis())),
            writer: Arc::new(Mutex::new(writer)),
            master: Arc::new(Mutex::new(master)),
            killer: Arc::new(Mutex::new(Some(killer))),
            sender,
        });

        spawn_reader_thread(reader, session.clone());
        spawn_wait_thread(child, session.clone());

        Ok(session)
    }

    fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.sender.subscribe()
    }

    fn touch(&self) {
        *lock_or_recover(&self.updated_at_unix_ms) = current_unix_timestamp_millis();
    }

    fn write_input(&self, bytes: &[u8]) -> Result<(), LocalTerminalDaemonError> {
        if bytes.is_empty() {
            return Ok(());
        }

        {
            let mut writer = lock_or_recover(&self.writer);
            writer.write_all(bytes).map_err(|error| {
                LocalTerminalDaemonError::message(format!(
                    "failed to write to session `{}`: {error}",
                    self.session_id
                ))
            })?;
            writer.flush().map_err(|error| {
                LocalTerminalDaemonError::message(format!(
                    "failed to flush session `{}`: {error}",
                    self.session_id
                ))
            })?;
        }

        self.track_command_input(bytes);
        self.touch();
        Ok(())
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), LocalTerminalDaemonError> {
        let cols = cols.max(2);
        let rows = rows.max(1);

        let master = lock_or_recover(&self.master);
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| {
                LocalTerminalDaemonError::message(format!(
                    "failed to resize session `{}`: {error}",
                    self.session_id
                ))
            })?;

        *lock_or_recover(&self.cols) = cols;
        *lock_or_recover(&self.rows) = rows;
        self.touch();
        Ok(())
    }

    fn signal(&self, signal: TerminalSignal) -> Result<(), LocalTerminalDaemonError> {
        match signal {
            TerminalSignal::Interrupt => self.write_input(&[0x03]),
            TerminalSignal::Terminate | TerminalSignal::Kill => self.kill(),
        }
    }

    fn kill(&self) -> Result<(), LocalTerminalDaemonError> {
        let mut killer_guard = lock_or_recover(&self.killer);
        let Some(killer) = killer_guard.as_mut() else {
            return Ok(());
        };

        killer.kill().map_err(|error| {
            LocalTerminalDaemonError::message(format!(
                "failed to terminate session `{}`: {error}",
                self.session_id
            ))
        })
    }

    fn append_output_chunk(&self, chunk: &str) {
        let mut output_tail = lock_or_recover(&self.output_tail);
        output_tail.push_str(chunk);
        *output_tail =
            trim_output_tail_preserving_ansi(output_tail.as_str(), OUTPUT_TAIL_MAX_CHARS);

        self.touch();
    }

    fn set_exit_state(&self, exit_code: Option<i32>, state: TerminalSessionState) {
        *lock_or_recover(&self.exit_code) = exit_code;
        *lock_or_recover(&self.state) = state;
        *lock_or_recover(&self.killer) = None;
        self.touch();
    }

    fn track_command_input(&self, bytes: &[u8]) {
        let mut pending = lock_or_recover(&self.pending_command);
        let mut next_last_command: Option<String> = None;

        for ch in String::from_utf8_lossy(bytes).chars() {
            match ch {
                '\r' | '\n' => {
                    let trimmed = pending.trim();
                    if !trimmed.is_empty() {
                        next_last_command = Some(trimmed.to_owned());
                    }
                    pending.clear();
                },
                '\u{08}' | '\u{7f}' => {
                    let _ = pending.pop();
                },
                _ => {
                    if !ch.is_control() {
                        pending.push(ch);
                    }
                },
            }
        }

        drop(pending);

        if let Some(command) = next_last_command {
            *lock_or_recover(&self.last_command) = Some(command);
        }
    }

    fn record(&self) -> DaemonSessionRecord {
        DaemonSessionRecord {
            session_id: self.session_id.clone(),
            workspace_id: self.workspace_id.clone(),
            cwd: self.cwd.clone(),
            shell: self.shell.clone(),
            cols: *lock_or_recover(&self.cols),
            rows: *lock_or_recover(&self.rows),
            title: lock_or_recover(&self.title).clone(),
            last_command: lock_or_recover(&self.last_command).clone(),
            output_tail: {
                let output = lock_or_recover(&self.output_tail);
                if output.is_empty() {
                    None
                } else {
                    Some(output.clone())
                }
            },
            exit_code: *lock_or_recover(&self.exit_code),
            state: Some(*lock_or_recover(&self.state)),
            updated_at_unix_ms: *lock_or_recover(&self.updated_at_unix_ms),
        }
    }

    fn snapshot(&self, max_lines: usize) -> TerminalSnapshot {
        let output_tail = {
            let output = lock_or_recover(&self.output_tail).clone();
            if max_lines == 0 {
                output
            } else {
                trim_to_last_lines(&output, max_lines)
            }
        };

        TerminalSnapshot {
            session_id: self.session_id.clone(),
            output_tail,
            exit_code: *lock_or_recover(&self.exit_code),
            state: *lock_or_recover(&self.state),
            updated_at_unix_ms: *lock_or_recover(&self.updated_at_unix_ms),
        }
    }
}

fn spawn_reader_thread(mut reader: Box<dyn Read + Send>, session: Arc<LiveSession>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    let text = String::from_utf8_lossy(&buffer[..bytes_read]).into_owned();
                    if text.is_empty() {
                        continue;
                    }

                    session.append_output_chunk(&text);
                    let _ = session.sender.send(SessionEvent::Output(text));
                },
                Err(error) => {
                    let _ = session.sender.send(SessionEvent::Error(format!(
                        "failed to read terminal output: {error}"
                    )));
                    break;
                },
            }
        }
    });
}

fn spawn_wait_thread(mut child: Box<dyn Child + Send + Sync>, session: Arc<LiveSession>) {
    thread::spawn(move || match child.wait() {
        Ok(status) => {
            let exit_code = i32::try_from(status.exit_code()).ok();
            let state = if status.success() {
                TerminalSessionState::Completed
            } else {
                TerminalSessionState::Failed
            };
            session.set_exit_state(exit_code, state);
            let _ = session.sender.send(SessionEvent::Exit { exit_code, state });
        },
        Err(error) => {
            session.set_exit_state(None, TerminalSessionState::Failed);
            let _ = session.sender.send(SessionEvent::Error(format!(
                "failed waiting for session exit: {error}"
            )));
        },
    });
}

pub struct LocalTerminalDaemon {
    sessions: HashMap<String, Arc<LiveSession>>,
    session_store: JsonDaemonSessionStore,
    next_session_id: u64,
}

impl LocalTerminalDaemon {
    pub fn new(session_store: JsonDaemonSessionStore) -> Self {
        Self {
            sessions: HashMap::new(),
            session_store,
            next_session_id: 1,
        }
    }

    pub fn subscribe(
        &self,
        session_id: &str,
    ) -> Result<broadcast::Receiver<SessionEvent>, LocalTerminalDaemonError> {
        let session = self.sessions.get(session_id).ok_or_else(|| {
            LocalTerminalDaemonError::SessionNotFound {
                session_id: session_id.to_owned(),
            }
        })?;

        Ok(session.subscribe())
    }

    fn collect_live_records(&self) -> Vec<DaemonSessionRecord> {
        self.sessions
            .values()
            .map(|session| session.record())
            .collect()
    }

    fn merge_live_and_stored(
        &self,
        live_records: Vec<DaemonSessionRecord>,
    ) -> Result<Vec<DaemonSessionRecord>, LocalTerminalDaemonError> {
        let live_ids: HashSet<String> = live_records
            .iter()
            .map(|record| record.session_id.clone())
            .collect();

        let mut stored = self.session_store.load()?;
        stored.retain(|record| {
            !live_ids.contains(&record.session_id)
                && !is_generated_daemon_session_id(&record.session_id)
        });
        stored.extend(live_records);
        Ok(stored)
    }

    fn persist_current_sessions(&self) -> Result<(), LocalTerminalDaemonError> {
        let merged = self.merge_live_and_stored(self.collect_live_records())?;
        self.session_store.save(&merged)?;
        Ok(())
    }

    fn session_by_id(
        &self,
        session_id: &str,
    ) -> Result<Arc<LiveSession>, LocalTerminalDaemonError> {
        self.sessions.get(session_id).cloned().ok_or_else(|| {
            LocalTerminalDaemonError::SessionNotFound {
                session_id: session_id.to_owned(),
            }
        })
    }

    fn next_generated_session_id(&mut self) -> String {
        let next = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);
        format!("{DAEMON_SESSION_PREFIX}{next}")
    }
}

impl TerminalDaemon for LocalTerminalDaemon {
    type Error = LocalTerminalDaemonError;

    fn create_or_attach(
        &mut self,
        mut request: CreateOrAttachRequest,
    ) -> Result<CreateOrAttachResponse, Self::Error> {
        if request.session_id.trim().is_empty() {
            request.session_id = self.next_generated_session_id();
        }

        if let Some(existing) = self.sessions.get(&request.session_id) {
            return Ok(CreateOrAttachResponse {
                is_new_session: false,
                session: existing.record(),
            });
        }

        if request.cols == 0 {
            request.cols = DEFAULT_COLS;
        }
        if request.rows == 0 {
            request.rows = DEFAULT_ROWS;
        }

        let session = LiveSession::from_request(request)?;
        let record = session.record();
        self.sessions.insert(record.session_id.clone(), session);
        self.persist_current_sessions()?;

        Ok(CreateOrAttachResponse {
            is_new_session: true,
            session: record,
        })
    }

    fn write(&mut self, request: WriteRequest) -> Result<(), Self::Error> {
        let session = self.session_by_id(&request.session_id)?;
        session.write_input(&request.bytes)?;
        self.persist_current_sessions()?;
        Ok(())
    }

    fn resize(&mut self, request: ResizeRequest) -> Result<(), Self::Error> {
        let session = self.session_by_id(&request.session_id)?;
        session.resize(request.cols, request.rows)?;
        self.persist_current_sessions()?;
        Ok(())
    }

    fn signal(&mut self, request: SignalRequest) -> Result<(), Self::Error> {
        let session = self.session_by_id(&request.session_id)?;
        session.signal(request.signal)?;
        self.persist_current_sessions()?;
        Ok(())
    }

    fn detach(&mut self, _request: DetachRequest) -> Result<(), Self::Error> {
        self.persist_current_sessions()?;
        Ok(())
    }

    fn kill(&mut self, request: KillRequest) -> Result<(), Self::Error> {
        let session = self.session_by_id(&request.session_id)?;
        session.kill()?;
        self.sessions.remove(&request.session_id);
        self.persist_current_sessions()?;
        Ok(())
    }

    fn snapshot(&self, request: SnapshotRequest) -> Result<Option<TerminalSnapshot>, Self::Error> {
        let Some(session) = self.sessions.get(&request.session_id) else {
            return Ok(None);
        };

        Ok(Some(session.snapshot(request.max_lines)))
    }

    fn list_sessions(&self) -> Result<Vec<DaemonSessionRecord>, Self::Error> {
        self.merge_live_and_stored(self.collect_live_records())
    }
}

fn is_generated_daemon_session_id(session_id: &str) -> bool {
    session_id.starts_with(DAEMON_SESSION_PREFIX)
}

fn default_shell() -> String {
    arbor_core::daemon::default_shell()
}

fn current_unix_timestamp_millis() -> Option<u64> {
    arbor_core::daemon::current_unix_timestamp_millis()
}

fn trim_to_last_lines(text: &str, max_lines: usize) -> String {
    if max_lines == 0 {
        return String::new();
    }

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_owned();
    }

    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn trim_output_tail_preserving_ansi(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_owned();
    }

    let target = total_chars.saturating_sub(max_chars);
    let mut state = AnsiScanState::Ground;
    let mut cut_byte = None;
    let mut consumed = 0_usize;

    for (byte_index, ch) in text.char_indices() {
        if consumed >= target && state == AnsiScanState::Ground {
            cut_byte = Some(byte_index);
            break;
        }

        state = state.step(ch);
        consumed = consumed.saturating_add(1);
    }

    let start = cut_byte.unwrap_or(text.len());
    let trimmed = &text[start..];
    strip_incomplete_trailing_ansi(trimmed)
}

fn strip_incomplete_trailing_ansi(text: &str) -> String {
    let mut state = AnsiScanState::Ground;
    let mut sequence_start: Option<usize> = None;

    for (byte_index, ch) in text.char_indices() {
        let next = state.step(ch);
        if state == AnsiScanState::Ground && next != AnsiScanState::Ground {
            sequence_start = Some(byte_index);
        } else if next == AnsiScanState::Ground {
            sequence_start = None;
        }
        state = next;
    }

    if state == AnsiScanState::Ground {
        return text.to_owned();
    }

    match sequence_start {
        Some(index) => text[..index].to_owned(),
        None => String::new(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnsiScanState {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
    Dcs,
    DcsEscape,
    Apc,
    ApcEscape,
    Pm,
    PmEscape,
}

impl AnsiScanState {
    fn step(self, ch: char) -> Self {
        match self {
            Self::Ground => {
                if ch == '\u{1b}' {
                    Self::Escape
                } else {
                    Self::Ground
                }
            },
            Self::Escape => match ch {
                '[' => Self::Csi,
                ']' => Self::Osc,
                'P' => Self::Dcs,
                '_' => Self::Apc,
                '^' => Self::Pm,
                '\u{1b}' => Self::Escape,
                _ => Self::Ground,
            },
            Self::Csi => {
                if matches!(ch as u32, 0x40..=0x7E) {
                    Self::Ground
                } else {
                    Self::Csi
                }
            },
            Self::Osc => match ch {
                '\u{07}' => Self::Ground,
                '\u{1b}' => Self::OscEscape,
                _ => Self::Osc,
            },
            Self::OscEscape => {
                if ch == '\\' {
                    Self::Ground
                } else {
                    Self::Osc
                }
            },
            Self::Dcs => {
                if ch == '\u{1b}' {
                    Self::DcsEscape
                } else {
                    Self::Dcs
                }
            },
            Self::DcsEscape => {
                if ch == '\\' {
                    Self::Ground
                } else {
                    Self::Dcs
                }
            },
            Self::Apc => {
                if ch == '\u{1b}' {
                    Self::ApcEscape
                } else {
                    Self::Apc
                }
            },
            Self::ApcEscape => {
                if ch == '\\' {
                    Self::Ground
                } else {
                    Self::Apc
                }
            },
            Self::Pm => {
                if ch == '\u{1b}' {
                    Self::PmEscape
                } else {
                    Self::Pm
                }
            },
            Self::PmEscape => {
                if ch == '\\' {
                    Self::Ground
                } else {
                    Self::Pm
                }
            },
        }
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_trim_does_not_start_inside_osc_sequence() {
        let text = format!(
            "{}{}",
            "x".repeat(32),
            "\u{1b}]133;C;cmdline=echo hi\u{1b}\\\nplain\n"
        );

        let trimmed = trim_output_tail_preserving_ansi(&text, 18);
        assert!(
            !trimmed.starts_with("133;"),
            "trimmed tail started in OSC payload: {trimmed:?}"
        );
        assert!(
            !trimmed.starts_with(";C;"),
            "trimmed tail started in OSC payload: {trimmed:?}"
        );
    }

    #[test]
    fn tail_trim_drops_incomplete_trailing_escape() {
        let text = format!("{}hello\u{1b}]133;A", "x".repeat(32));
        let trimmed = trim_output_tail_preserving_ansi(&text, 12);
        assert_eq!(trimmed, "hello");
    }

    #[test]
    fn tail_trim_keeps_complete_st_terminated_escape() {
        let text = "a\u{1b}]133;A\u{1b}\\b";
        let trimmed = trim_output_tail_preserving_ansi(text, 64);
        assert_eq!(trimmed, text);
    }
}
