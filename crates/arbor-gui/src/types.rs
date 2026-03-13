use serde::{Deserialize, Serialize};

/// Identifies a sidebar item for persisted UI ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum SidebarItemId {
    Worktree(PathBuf),
    Outpost(String),
}

#[derive(Debug, Clone)]
struct WorktreeSummary {
    group_key: String,
    checkout_kind: CheckoutKind,
    repo_root: PathBuf,
    path: PathBuf,
    label: String,
    branch: String,
    is_primary_checkout: bool,
    pr_number: Option<u64>,
    pr_url: Option<String>,
    pr_details: Option<github_service::PrDetails>,
    branch_divergence: Option<BranchDivergenceSummary>,
    diff_summary: Option<changes::DiffLineSummary>,
    detected_ports: Vec<DetectedPort>,
    recent_turns: Vec<AgentTurnSnapshot>,
    stuck_turn_count: usize,
    recent_agent_sessions: Vec<arbor_core::session::AgentSessionSummary>,
    agent_state: Option<AgentState>,
    agent_task: Option<String>,
    last_activity_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BranchDivergenceSummary {
    ahead: usize,
    behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetectedPort {
    port: u16,
    pid: Option<u32>,
    address: String,
    process_name: String,
    label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AgentTurnSnapshot {
    timestamp_unix_ms: Option<u64>,
    diff_summary: Option<changes::DiffLineSummary>,
}

#[derive(Debug, Clone)]
struct RepositorySummary {
    group_key: String,
    root: PathBuf,
    checkout_roots: Vec<repository_store::RepositoryCheckoutRoot>,
    label: String,
    avatar_url: Option<String>,
    github_repo_slug: Option<String>,
}

type SharedTerminalRuntime = Arc<dyn TerminalRuntimeHandle>;

#[derive(Clone)]
struct TerminalSession {
    id: u64,
    daemon_session_id: String,
    worktree_path: PathBuf,
    title: String,
    last_command: Option<String>,
    pending_command: String,
    command: String,
    agent_preset: Option<AgentPresetKind>,
    execution_mode: Option<ExecutionMode>,
    state: TerminalState,
    exit_code: Option<i32>,
    updated_at_unix_ms: Option<u64>,
    root_pid: Option<u32>,
    cols: u16,
    rows: u16,
    generation: u64,
    output: String,
    styled_output: Vec<TerminalStyledLine>,
    cursor: Option<TerminalCursor>,
    modes: TerminalModes,
    last_runtime_sync_at: Option<Instant>,
    queued_input: Vec<u8>,
    is_initializing: bool,
    runtime: Option<SharedTerminalRuntime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalState {
    Running,
    Completed,
    Failed,
}

/// SSH terminal shell wrapper that provides Clone (via Arc<Mutex>) and
/// a terminal emulator for rendering. Polling is done from the GUI timer
/// since libssh's Channel is not Send/Sync.
#[derive(Clone)]
struct SshTerminalShell {
    shell: Arc<Mutex<arbor_ssh::shell::SshShell>>,
    emulator: Arc<Mutex<arbor_terminal_emulator::TerminalEmulator>>,
    generation: Arc<std::sync::atomic::AtomicU64>,
}

impl SshTerminalShell {
    fn open(
        connection: &arbor_ssh::connection::SshConnection,
        cols: u16,
        rows: u16,
        remote_path: &str,
    ) -> Result<Self, String> {
        let shell = arbor_ssh::shell::SshShell::open(
            connection.session(),
            u32::from(cols),
            u32::from(rows),
        )
        .map_err(|e| format!("failed to open SSH shell: {e}"))?;

        // Send cd command to navigate to the outpost directory
        shell
            .write_input(format!("cd {remote_path} && clear\n").as_bytes())
            .map_err(|e| format!("failed to send cd command: {e}"))?;

        Ok(Self {
            shell: Arc::new(Mutex::new(shell)),
            emulator: Arc::new(Mutex::new(
                arbor_terminal_emulator::TerminalEmulator::with_size(rows, cols),
            )),
            generation: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        })
    }

    fn write_input(&self, bytes: &[u8]) -> Result<(), String> {
        let shell = self
            .shell
            .lock()
            .map_err(|_| "SSH shell lock poisoned".to_owned())?;
        shell
            .write_input(bytes)
            .map_err(|e| format!("failed to write to SSH shell: {e}"))
    }

    /// Poll the shell for new output and feed it to the terminal emulator.
    /// Called from the GUI polling timer. Returns true if new data was processed.
    fn poll(&self) -> bool {
        let shell = match self.shell.lock() {
            Ok(guard) => guard,
            Err(_) => return false,
        };
        match shell.read_available() {
            Ok(data) if !data.is_empty() => {
                drop(shell);
                let mut emulator = match self.emulator.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                emulator.process(&data);
                self.generation
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                true
            },
            _ => false,
        }
    }

    fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot {
        let (output, styled_lines, cursor, modes) = match self.emulator.lock() {
            Ok(emulator) => (
                emulator.snapshot_output(),
                emulator.collect_styled_lines(),
                emulator.snapshot_cursor(),
                emulator.snapshot_modes(),
            ),
            Err(poisoned) => {
                let emulator = poisoned.into_inner();
                (
                    emulator.snapshot_output(),
                    emulator.collect_styled_lines(),
                    emulator.snapshot_cursor(),
                    emulator.snapshot_modes(),
                )
            },
        };

        let is_closed = self
            .shell
            .lock()
            .map(|s| s.is_closed() || s.is_eof())
            .unwrap_or(true);

        arbor_terminal_emulator::TerminalSnapshot {
            output,
            styled_lines,
            cursor,
            modes,
            exit_code: if is_closed {
                Some(0)
            } else {
                None
            },
        }
    }

    fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        let shell = self
            .shell
            .lock()
            .map_err(|_| "SSH shell lock poisoned".to_owned())?;
        shell
            .resize(u32::from(cols), u32::from(rows))
            .map_err(|e| format!("failed to resize SSH shell: {e}"))?;
        drop(shell);

        if let Ok(mut emulator) = self.emulator.lock() {
            emulator.resize(rows, cols);
        }
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn close(&self) {
        if let Ok(shell) = self.shell.lock() {
            let _ = shell.close();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalRuntimeKind {
    Local,
    Outpost,
}

struct RuntimeNotification {
    title: String,
    body: String,
    play_sound: bool,
}

#[derive(Default)]
struct TerminalRuntimeSyncOutcome {
    changed: bool,
    close_session: bool,
    clear_global_daemon: bool,
    notice: Option<String>,
    notification: Option<RuntimeNotification>,
}

trait EmulatorRuntimeBackend: Clone {
    fn poll(&self);
    fn write_input(&self, input: &[u8]) -> Result<(), String>;
    fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot;
    fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), String>;
    fn generation(&self) -> u64;
    fn close(&self);
}

trait TerminalRuntimeHandle {
    fn kind(&self) -> TerminalRuntimeKind;
    fn sync_interval(&self, is_active: bool, session_state: TerminalState) -> Duration;
    fn should_sync(
        &self,
        session: &TerminalSession,
        is_active: bool,
        _target_grid_size: Option<(u16, u16, u16, u16)>,
        now: Instant,
    ) -> bool {
        runtime_sync_interval_elapsed(
            session.last_runtime_sync_at,
            self.sync_interval(is_active, session.state),
            now,
        )
    }
    fn write_input(&self, session: &TerminalSession, input: &[u8]) -> Result<(), String>;
    fn sync(
        &self,
        session: &mut TerminalSession,
        is_active: bool,
        target_grid_size: Option<(u16, u16, u16, u16)>,
    ) -> TerminalRuntimeSyncOutcome;
    fn close(&self, session: &TerminalSession) -> Result<(), String>;
}

#[derive(Clone, Copy)]
struct RuntimeExitLabels {
    completed_title: &'static str,
    failed_title: &'static str,
    failed_notice_prefix: &'static str,
}

#[derive(Clone)]
struct EmulatorTerminalRuntime<B> {
    backend: B,
    kind: TerminalRuntimeKind,
    resize_error_label: &'static str,
    exit_labels: RuntimeExitLabels,
}

struct DaemonTerminalRuntime {
    daemon: terminal_daemon_http::SharedTerminalDaemonClient,
    ws_state: Arc<DaemonTerminalWsState>,
    last_synced_ws_generation: std::sync::atomic::AtomicU64,
    snapshot_request_in_flight: Arc<std::sync::atomic::AtomicBool>,
    kind: TerminalRuntimeKind,
    resize_error_label: &'static str,
    exit_labels: Option<RuntimeExitLabels>,
    clear_global_daemon_on_connection_refused: bool,
}

#[derive(Clone)]
struct DaemonTerminalCachedSnapshot {
    terminal: arbor_terminal_emulator::TerminalSnapshot,
    state: TerminalState,
    updated_at_unix_ms: Option<u64>,
    ready: bool,
}

impl Default for DaemonTerminalCachedSnapshot {
    fn default() -> Self {
        Self {
            terminal: arbor_terminal_emulator::TerminalSnapshot {
                output: String::new(),
                styled_lines: Vec::new(),
                cursor: None,
                modes: TerminalModes::default(),
                exit_code: None,
            },
            state: TerminalState::Running,
            updated_at_unix_ms: None,
            ready: false,
        }
    }
}

struct DaemonTerminalWsState {
    event_generation: std::sync::atomic::AtomicU64,
    closed: std::sync::atomic::AtomicBool,
    connection_refused: std::sync::atomic::AtomicBool,
    /// Channel to send keystroke bytes to the WS thread for low-latency binary transmission.
    ws_writer: Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
    /// Channel to wake the terminal poller when new data arrives.
    poll_notify: Option<std::sync::mpsc::Sender<()>>,
    size: Mutex<(u16, u16)>,
    emulator: Mutex<arbor_terminal_emulator::TerminalEmulator>,
    snapshot: Mutex<DaemonTerminalCachedSnapshot>,
}

impl Default for DaemonTerminalWsState {
    fn default() -> Self {
        Self::new(
            None,
            arbor_terminal_emulator::TERMINAL_ROWS,
            arbor_terminal_emulator::TERMINAL_COLS,
        )
    }
}

impl DaemonTerminalWsState {
    fn new(poll_notify: Option<std::sync::mpsc::Sender<()>>, rows: u16, cols: u16) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(2);
        Self {
            event_generation: std::sync::atomic::AtomicU64::new(0),
            closed: std::sync::atomic::AtomicBool::new(false),
            connection_refused: std::sync::atomic::AtomicBool::new(false),
            ws_writer: Mutex::new(None),
            poll_notify,
            size: Mutex::new((rows, cols)),
            emulator: Mutex::new(arbor_terminal_emulator::TerminalEmulator::with_size(rows, cols)),
            snapshot: Mutex::new(DaemonTerminalCachedSnapshot::default()),
        }
    }

    fn note_event(&self) {
        self.event_generation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Some(ref tx) = self.poll_notify {
            let _ = tx.send(());
        }
    }

    fn event_generation(&self) -> u64 {
        self.event_generation
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn note_connection_refused(&self) {
        self.connection_refused
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.note_event();
    }

    fn take_connection_refused(&self) -> bool {
        self.connection_refused
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Try to send keystroke bytes through the WebSocket channel.
    /// Returns true if sent, false if WS writer is not available.
    fn try_write(&self, bytes: Vec<u8>) -> bool {
        if let Ok(guard) = self.ws_writer.lock()
            && let Some(ref sender) = *guard
        {
            return sender.send(bytes).is_ok();
        }
        false
    }

    fn set_writer(&self, sender: Option<std::sync::mpsc::Sender<Vec<u8>>>) {
        if let Ok(mut guard) = self.ws_writer.lock() {
            *guard = sender;
        }
    }

    fn apply_snapshot_text(
        &self,
        ansi_output: &str,
        state: TerminalState,
        exit_code: Option<i32>,
        updated_at_unix_ms: Option<u64>,
    ) {
        let (rows, cols) = self
            .size
            .lock()
            .map(|guard| *guard)
            .unwrap_or((
                arbor_terminal_emulator::TERMINAL_ROWS,
                arbor_terminal_emulator::TERMINAL_COLS,
            ));

        let terminal_snapshot = {
            let mut emulator = match self.emulator.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *emulator = arbor_terminal_emulator::TerminalEmulator::with_size(rows, cols);
            emulator.process(ansi_output.as_bytes());
            let mut snapshot =
                trim_terminal_snapshot(emulator.snapshot(), DAEMON_TERMINAL_WS_MAX_LINES);
            snapshot.exit_code = exit_code;
            snapshot
        };

        let mut cached = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *cached = DaemonTerminalCachedSnapshot {
            terminal: terminal_snapshot,
            state,
            updated_at_unix_ms: updated_at_unix_ms.or_else(current_unix_timestamp_millis),
            ready: true,
        };
        drop(cached);
        self.note_event();
    }

    fn apply_output_bytes(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let terminal_snapshot = {
            let mut emulator = match self.emulator.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            emulator.process(bytes);
            trim_terminal_snapshot(emulator.snapshot(), DAEMON_TERMINAL_WS_MAX_LINES)
        };

        let mut cached = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        cached.terminal = terminal_snapshot;
        cached.updated_at_unix_ms = current_unix_timestamp_millis();
        cached.ready = true;
        drop(cached);
        self.note_event();
    }

    fn apply_exit(&self, state: TerminalState, exit_code: Option<i32>) {
        let mut cached = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        cached.state = state;
        cached.terminal.exit_code = exit_code;
        cached.updated_at_unix_ms = current_unix_timestamp_millis();
        cached.ready = true;
        drop(cached);
        self.note_event();
    }

    fn resize_emulator(&self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(2);
        if let Ok(mut size) = self.size.lock() {
            *size = (rows, cols);
        }

        let terminal_snapshot = {
            let mut emulator = match self.emulator.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            emulator.resize(rows, cols);
            trim_terminal_snapshot(emulator.snapshot(), DAEMON_TERMINAL_WS_MAX_LINES)
        };

        let mut cached = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if cached.ready {
            cached.terminal = terminal_snapshot;
            cached.updated_at_unix_ms = current_unix_timestamp_millis();
        }
    }

    fn snapshot(&self) -> Option<DaemonTerminalCachedSnapshot> {
        self.snapshot
            .lock()
            .ok()
            .map(|guard| guard.clone())
            .filter(|snapshot| snapshot.ready)
    }
}

impl EmulatorRuntimeBackend for EmbeddedTerminal {
    fn poll(&self) {}

    fn write_input(&self, input: &[u8]) -> Result<(), String> {
        EmbeddedTerminal::write_input(self, input)
    }

    fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot {
        EmbeddedTerminal::snapshot(self)
    }

    fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), String> {
        EmbeddedTerminal::resize(self, rows, cols, pixel_width, pixel_height)
    }

    fn generation(&self) -> u64 {
        EmbeddedTerminal::generation(self)
    }

    fn close(&self) {
        EmbeddedTerminal::close(self);
    }
}

impl EmulatorRuntimeBackend for SshTerminalShell {
    fn poll(&self) {
        let _ = SshTerminalShell::poll(self);
    }

    fn write_input(&self, input: &[u8]) -> Result<(), String> {
        SshTerminalShell::write_input(self, input)
    }

    fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot {
        SshTerminalShell::snapshot(self)
    }

    fn resize(
        &self,
        rows: u16,
        cols: u16,
        _pixel_width: u16,
        _pixel_height: u16,
    ) -> Result<(), String> {
        SshTerminalShell::resize(self, rows, cols)
    }

    fn generation(&self) -> u64 {
        SshTerminalShell::generation(self)
    }

    fn close(&self) {
        SshTerminalShell::close(self);
    }
}

impl EmulatorRuntimeBackend for arbor_mosh::MoshShell {
    fn poll(&self) {}

    fn write_input(&self, input: &[u8]) -> Result<(), String> {
        arbor_mosh::MoshShell::write_input(self, input)
    }

    fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot {
        arbor_mosh::MoshShell::snapshot(self)
    }

    fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), String> {
        arbor_mosh::MoshShell::resize(self, rows, cols, pixel_width, pixel_height)
    }

    fn generation(&self) -> u64 {
        arbor_mosh::MoshShell::generation(self)
    }

    fn close(&self) {
        arbor_mosh::MoshShell::close(self);
    }
}

impl<B> TerminalRuntimeHandle for EmulatorTerminalRuntime<B>
where
    B: EmulatorRuntimeBackend + 'static,
{
    fn kind(&self) -> TerminalRuntimeKind {
        self.kind
    }

    fn sync_interval(&self, _is_active: bool, _session_state: TerminalState) -> Duration {
        Duration::ZERO
    }

    fn write_input(&self, _session: &TerminalSession, input: &[u8]) -> Result<(), String> {
        self.backend.write_input(input)
    }

    fn sync(
        &self,
        session: &mut TerminalSession,
        is_active: bool,
        target_grid_size: Option<(u16, u16, u16, u16)>,
    ) -> TerminalRuntimeSyncOutcome {
        let mut outcome = TerminalRuntimeSyncOutcome::default();

        if is_active
            && let Some((rows, cols, pixel_width, pixel_height)) = target_grid_size
            && let Err(error) = self.backend.resize(rows, cols, pixel_width, pixel_height)
        {
            outcome.notice = Some(format!("{}: {error}", self.resize_error_label));
        }

        self.backend.poll();

        let generation = self.backend.generation();
        if generation == session.generation {
            return outcome;
        }

        let snapshot = self.backend.snapshot();
        if apply_terminal_emulator_snapshot(session, snapshot) {
            outcome.changed = true;
        }
        session.generation = generation;

        if let Some(exit_code) = session.exit_code
            && session.state == TerminalState::Running
        {
            session.updated_at_unix_ms = current_unix_timestamp_millis();
            if exit_code == 0 {
                outcome.notification = Some(RuntimeNotification {
                    title: self.exit_labels.completed_title.to_owned(),
                    body: format!("`{}` completed successfully", session.title),
                    play_sound: true,
                });
                outcome.close_session = true;
            } else {
                session.state = TerminalState::Failed;
                session.runtime = None;
                outcome.changed = true;
                outcome.notification = Some(RuntimeNotification {
                    title: self.exit_labels.failed_title.to_owned(),
                    body: format!("`{}` failed with code {exit_code}", session.title),
                    play_sound: false,
                });
                outcome.notice = Some(format!(
                    "{} `{}` exited with code {exit_code}",
                    self.exit_labels.failed_notice_prefix, session.title
                ));
            }
        }

        outcome
    }

    fn close(&self, _session: &TerminalSession) -> Result<(), String> {
        self.backend.close();
        Ok(())
    }
}

impl TerminalRuntimeHandle for DaemonTerminalRuntime {
    fn kind(&self) -> TerminalRuntimeKind {
        self.kind
    }

    fn sync_interval(&self, is_active: bool, session_state: TerminalState) -> Duration {
        daemon_terminal_sync_interval(is_active, session_state)
    }

    fn should_sync(
        &self,
        session: &TerminalSession,
        is_active: bool,
        target_grid_size: Option<(u16, u16, u16, u16)>,
        now: Instant,
    ) -> bool {
        if is_active
            && let Some((rows, cols, ..)) = target_grid_size
            && (cols != session.cols || rows != session.rows)
        {
            return true;
        }

        let current_generation = self.ws_state.event_generation();
        let last_synced_generation = self
            .last_synced_ws_generation
            .load(std::sync::atomic::Ordering::Relaxed);
        if current_generation > last_synced_generation {
            return is_active
                || runtime_sync_interval_elapsed(
                    session.last_runtime_sync_at,
                    self.sync_interval(false, session.state),
                    now,
                );
        }

        runtime_sync_interval_elapsed(
            session.last_runtime_sync_at,
            self.sync_interval(is_active, session.state),
            now,
        )
    }

    fn write_input(&self, session: &TerminalSession, input: &[u8]) -> Result<(), String> {
        if input == [0x03] {
            // Ctrl-C: send as signal for reliable delivery
            self.daemon
                .signal(SignalRequest {
                    session_id: session.daemon_session_id.clone().into(),
                    signal: TerminalSignal::Interrupt,
                })
                .map_err(|error| error.to_string())
        } else if self.ws_state.try_write(input.to_vec()) {
            // Fast path: send via WebSocket binary frame
            tracing::trace!("write_input: sent via WS binary frame");
            Ok(())
        } else {
            // Fallback: HTTP POST (WS not connected yet)
            tracing::trace!("write_input: WS unavailable, falling back to HTTP POST");
            self.daemon
                .write(WriteRequest {
                    session_id: session.daemon_session_id.clone().into(),
                    bytes: input.to_vec(),
                })
                .map_err(|error| error.to_string())
        }
    }

    fn sync(
        &self,
        session: &mut TerminalSession,
        is_active: bool,
        target_grid_size: Option<(u16, u16, u16, u16)>,
    ) -> TerminalRuntimeSyncOutcome {
        let mut outcome = TerminalRuntimeSyncOutcome::default();
        let observed_ws_generation = self.ws_state.event_generation();

        if self.clear_global_daemon_on_connection_refused && self.ws_state.take_connection_refused()
        {
            session.runtime = None;
            session.state = TerminalState::Failed;
            outcome.changed = true;
            outcome.clear_global_daemon = true;
            return outcome;
        }

        if is_active
            && let Some((rows, cols, ..)) = target_grid_size
            && (cols != session.cols || rows != session.rows)
        {
            match self.daemon.resize(ResizeRequest {
                session_id: session.daemon_session_id.clone().into(),
                cols,
                rows,
            }) {
                Ok(()) => {
                    session.cols = cols;
                    session.rows = rows;
                    self.ws_state.resize_emulator(rows, cols);
                    outcome.changed = true;
                },
                Err(error) => {
                    outcome.notice = Some(format!("{}: {error}", self.resize_error_label));
                },
            }
        }

        let Some(snapshot) = self.ws_state.snapshot() else {
            request_async_daemon_snapshot(
                self.daemon.clone(),
                session.daemon_session_id.clone(),
                self.ws_state.clone(),
                self.snapshot_request_in_flight.clone(),
            );
            return outcome;
        };

        self.last_synced_ws_generation
            .store(observed_ws_generation, std::sync::atomic::Ordering::Relaxed);

        if apply_terminal_emulator_snapshot(session, snapshot.terminal.clone()) {
            outcome.changed = true;
        }

        if session.state != snapshot.state {
            session.state = snapshot.state;
            outcome.changed = true;
        }
        if session.updated_at_unix_ms != snapshot.updated_at_unix_ms {
            session.updated_at_unix_ms = snapshot.updated_at_unix_ms;
            outcome.changed = true;
        }

        if let Some(exit_labels) = self.exit_labels
            && let Some(exit_code) = snapshot.terminal.exit_code
        {
            if exit_code == 0 {
                outcome.notification = Some(RuntimeNotification {
                    title: exit_labels.completed_title.to_owned(),
                    body: format!("`{}` completed successfully", session.title),
                    play_sound: true,
                });
                outcome.close_session = true;
            } else if session.state == TerminalState::Failed {
                session.runtime = None;
                outcome.changed = true;
                outcome.notification = Some(RuntimeNotification {
                    title: exit_labels.failed_title.to_owned(),
                    body: format!("`{}` failed with code {exit_code}", session.title),
                    play_sound: false,
                });
                outcome.notice = Some(format!(
                    "{} `{}` exited with code {exit_code}",
                    exit_labels.failed_notice_prefix, session.title
                ));
            }
        }

        outcome
    }

    fn close(&self, session: &TerminalSession) -> Result<(), String> {
        self.ws_state.close();

        let result = if session.state == TerminalState::Running {
            self.daemon.kill(KillRequest {
                session_id: session.daemon_session_id.clone().into(),
            })
        } else {
            self.daemon.detach(DetachRequest {
                session_id: session.daemon_session_id.clone().into(),
            })
        };

        result.map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CenterTab {
    Terminal(u64),
    Diff(u64),
    FileView(u64),
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightPaneTab {
    Changes,
    FileTree,
    Notes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AgentPresetKind {
    Codex,
    Claude,
    Pi,
    OpenCode,
    Copilot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ExecutionMode {
    Plan,
    Build,
    Yolo,
}

impl ExecutionMode {
    const ORDER: [Self; 3] = [Self::Plan, Self::Build, Self::Yolo];

    fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Build => "Build",
            Self::Yolo => "Yolo",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Plan => "Minimal write access",
            Self::Build => "Normal autonomous work",
            Self::Yolo => "Full permissions",
        }
    }
}

impl AgentPresetKind {
    const ORDER: [Self; 5] = [
        Self::Codex,
        Self::Claude,
        Self::Pi,
        Self::OpenCode,
        Self::Copilot,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
            Self::Pi => "Pi",
            Self::OpenCode => "OpenCode",
            Self::Copilot => "Copilot",
        }
    }

    fn fallback_icon(self) -> &'static str {
        match self {
            Self::Codex => "\u{f121}",
            Self::Claude => "C",
            Self::Pi => "P",
            Self::OpenCode => "\u{f085}",
            Self::Copilot => "\u{f09b}",
        }
    }

    fn default_command(self) -> &'static str {
        match self {
            Self::Codex => {
                "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox -c model_reasoning_summary=\"detailed\" -c model_supports_reasoning_summaries=true"
            },
            Self::Claude => "claude --dangerously-skip-permissions",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot --allow-all",
        }
    }

    fn executable_name(self) -> &'static str {
        self.key()
    }

    fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "codex" => Some(Self::Codex),
            "claude" => Some(Self::Claude),
            "pi" => Some(Self::Pi),
            "opencode" => Some(Self::OpenCode),
            "copilot" => Some(Self::Copilot),
            _ => None,
        }
    }

    fn cycle(self, reverse: bool) -> Self {
        let current = Self::ORDER
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        if reverse {
            Self::ORDER[(current + Self::ORDER.len() - 1) % Self::ORDER.len()]
        } else {
            Self::ORDER[(current + 1) % Self::ORDER.len()]
        }
    }

    /// Check if the default command for this preset is available in PATH.
    fn is_installed(self) -> bool {
        is_command_in_path(self.executable_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentPreset {
    kind: AgentPresetKind,
    command: String,
}

#[derive(Debug, Clone)]
struct SettingsModal {
    active_control: SettingsControl,
    daemon_bind_mode: DaemonBindMode,
    initial_daemon_bind_mode: DaemonBindMode,
    notifications: bool,
    daemon_auth_token: String,
    loading: bool,
    error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsControl {
    DaemonBindMode,
    Notifications,
}

impl SettingsControl {
    fn cycle(self, reverse: bool) -> Self {
        const ORDER: [SettingsControl; 2] = [
            SettingsControl::DaemonBindMode,
            SettingsControl::Notifications,
        ];
        let current = ORDER
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        if reverse {
            ORDER[(current + ORDER.len() - 1) % ORDER.len()]
        } else {
            ORDER[(current + 1) % ORDER.len()]
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonBindMode {
    Localhost,
    AllInterfaces,
}

impl DaemonBindMode {
    fn from_config(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("localhost" | "local" | "loopback" | "127.0.0.1") => Self::Localhost,
            Some("all" | "all-interfaces" | "public" | "0.0.0.0") => Self::AllInterfaces,
            _ => Self::AllInterfaces,
        }
    }

    fn as_config_value(self) -> &'static str {
        match self {
            Self::Localhost => "localhost",
            Self::AllInterfaces => "all-interfaces",
        }
    }
}

enum SettingsModalInputEvent {
    CycleControl(bool),
    SelectDaemonBindMode(DaemonBindMode),
    ToggleActiveControl,
    ToggleNotifications,
}

#[derive(Debug, Clone)]
struct ManagePresetsModal {
    active_preset: AgentPresetKind,
    command: String,
    command_cursor: usize,
    error: Option<String>,
}

enum PresetsModalInputEvent {
    SetActivePreset(AgentPresetKind),
    CycleActivePreset(bool),
    Edit(TextEditAction),
    RestoreDefault,
    ClearError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoPreset {
    name: String,
    icon: String,
    command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoPresetModalField {
    Icon,
    Name,
    Command,
}

impl RepoPresetModalField {
    const ORDER: [Self; 3] = [Self::Icon, Self::Name, Self::Command];

    fn next(self) -> Self {
        let index = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(index + 1) % Self::ORDER.len()]
    }

    fn prev(self) -> Self {
        let index = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(index + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

#[derive(Debug, Clone)]
struct ManageRepoPresetsModal {
    editing_index: Option<usize>,
    icon: String,
    icon_cursor: usize,
    name: String,
    name_cursor: usize,
    command: String,
    command_cursor: usize,
    active_tab: RepoPresetModalTab,
    active_field: RepoPresetModalField,
    error: Option<String>,
    saving: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoPresetModalTab {
    Edit,
    LocalPreset,
}

enum RepoPresetsModalInputEvent {
    SetActiveTab(RepoPresetModalTab),
    SetActiveField(RepoPresetModalField),
    MoveActiveField(bool),
    Edit(TextEditAction),
    ClearError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitActionKind {
    Commit,
    CommitPushCreatePullRequest,
    Push,
    CreatePullRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreeQuickAction {
    OpenFinder,
    CopyPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickActionSubmenu {
    Ide,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExternalLauncherKind {
    Command(&'static str),
    MacApp(&'static str),
}

#[derive(Debug, Clone, Copy)]
struct ExternalLauncher {
    label: &'static str,
    icon: &'static str,
    icon_color: u32,
    kind: ExternalLauncherKind,
}

#[derive(Debug, Clone)]
struct FileTreeEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLineKind {
    FileHeader,
    Context,
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone)]
struct DiffLine {
    left_line_number: Option<usize>,
    right_line_number: Option<usize>,
    left_text: String,
    right_text: String,
    kind: DiffLineKind,
}

#[derive(Debug, Clone)]
struct DiffSession {
    id: u64,
    worktree_path: PathBuf,
    title: String,
    raw_lines: Arc<[DiffLine]>,
    raw_file_row_indices: HashMap<PathBuf, usize>,
    lines: Arc<[DiffLine]>,
    file_row_indices: HashMap<PathBuf, usize>,
    wrapped_columns: usize,
    is_loading: bool,
}

#[derive(Debug, Clone)]
struct FileViewSpan {
    text: String,
    color: u32,
}

#[derive(Debug, Clone)]
enum FileViewContent {
    Text {
        highlighted: Arc<[Vec<FileViewSpan>]>,
        raw_lines: Vec<String>,
        dirty: bool,
    },
    Image(PathBuf),
}

#[derive(Debug, Clone, Copy)]
struct FileViewCursor {
    line: usize,
    col: usize,
}

#[derive(Debug, Clone)]
struct FileViewSession {
    id: u64,
    worktree_path: PathBuf,
    file_path: PathBuf,
    title: String,
    content: FileViewContent,
    is_loading: bool,
    cursor: FileViewCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraggedPaneDivider {
    Left,
    Right,
}

impl Render for DraggedPaneDivider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalGridPosition {
    line: usize,
    column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalSelection {
    session_id: u64,
    anchor: TerminalGridPosition,
    head: TerminalGridPosition,
}

#[derive(Debug, Clone)]
struct OutpostSummary {
    outpost_id: String,
    repo_root: PathBuf,
    remote_path: String,
    label: String,
    branch: String,
    host_name: String,
    hostname: String,
    status: arbor_core::outpost::OutpostStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateModalTab {
    LocalWorktree,
    ReviewPullRequest,
    RemoteOutpost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateOutpostField {
    HostSelector,
    CloneUrl,
    OutpostName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateWorktreeField {
    RepositoryPath,
    WorktreeName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateReviewPrField {
    RepositoryPath,
    PullRequestReference,
    WorktreeName,
}

#[derive(Debug, Clone)]
struct CreateModal {
    tab: CreateModalTab,
    // Worktree fields
    repository_path: String,
    repository_path_cursor: usize,
    worktree_name: String,
    worktree_name_cursor: usize,
    checkout_kind: CheckoutKind,
    worktree_active_field: CreateWorktreeField,
    // Review PR fields
    pr_reference: String,
    pr_reference_cursor: usize,
    review_active_field: CreateReviewPrField,
    // Outpost fields
    host_index: usize,
    host_dropdown_open: bool,
    clone_url: String,
    clone_url_cursor: usize,
    outpost_name: String,
    outpost_name_cursor: usize,
    outpost_active_field: CreateOutpostField,
    // Shared
    is_creating: bool,
    creating_status: Option<String>,
    worktree_branch_preview: String,
    review_branch_preview: String,
    outpost_branch_preview: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct GitHubAuthModal {
    user_code: String,
    verification_url: String,
}

enum ModalInputEvent {
    SetActiveField(CreateWorktreeField),
    MoveActiveField,
    Edit(TextEditAction),
    ClearError,
}

enum ReviewPrModalInputEvent {
    SetActiveField(CreateReviewPrField),
    MoveActiveField,
    Edit(TextEditAction),
    ClearError,
}

enum OutpostModalInputEvent {
    SetActiveField(CreateOutpostField),
    MoveActiveField(bool),
    CycleHost(bool),
    SelectHost(usize),
    ToggleHostDropdown,
    Edit(TextEditAction),
    ClearError,
}

#[derive(Clone)]
struct ManageHostsModal {
    adding: bool,
    name: String,
    name_cursor: usize,
    hostname: String,
    hostname_cursor: usize,
    user: String,
    user_cursor: usize,
    active_field: ManageHostsField,
    error: Option<String>,
    saving: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManageHostsField {
    Name,
    Hostname,
    User,
}

enum HostsModalInputEvent {
    SetActiveField(ManageHostsField),
    MoveActiveField(bool),
    Edit(TextEditAction),
    ClearError,
}

#[derive(Debug, Clone)]
enum DeleteTarget {
    Worktree(usize),
    Outpost(usize),
    Repository(usize),
}

#[derive(Debug, Clone)]
struct DeleteModal {
    target: DeleteTarget,
    label: String,
    branch: String,
    has_unpushed: Option<bool>,
    delete_branch: bool,
    is_deleting: bool,
    error: Option<String>,
}

struct DaemonAuthModal {
    daemon_url: String,
    token: String,
    token_cursor: usize,
    error: Option<String>,
}

struct ConnectToHostModal {
    address: String,
    address_cursor: usize,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct CommitModal {
    message: String,
    message_cursor: usize,
    generating: bool,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct CommandPaletteModal {
    query: String,
    query_cursor: usize,
    selected_index: usize,
}

#[derive(Debug, Clone)]
struct CommandPaletteItem {
    title: String,
    subtitle: String,
    search_text: String,
    action: CommandPaletteAction,
}

#[derive(Debug, Clone)]
enum CommandPaletteAction {
    OpenCreateWorktree,
    OpenReviewPullRequest,
    RefreshWorktrees,
    ToggleCompactSidebar,
    OpenSettings,
    OpenThemePicker,
    SetExecutionMode(ExecutionMode),
    LaunchAgentPreset(AgentPresetKind),
    LaunchRepoPreset(usize),
    SelectRepository(usize),
    SelectWorktree(usize),
    LaunchTaskTemplate(TaskTemplate),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskTemplate {
    name: String,
    description: String,
    prompt: String,
    agent: Option<AgentPresetKind>,
    path: PathBuf,
    repo_root: PathBuf,
}

#[derive(Debug, Clone)]
enum TextEditAction {
    Insert(String),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
}

enum ConnectHostTarget {
    Http {
        url: String,
        auth_key: String,
    },
    Ssh {
        target: SshDaemonTarget,
        auth_key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SshDaemonTarget {
    user: Option<String>,
    host: String,
    ssh_port: u16,
    daemon_port: u16,
}

impl SshDaemonTarget {
    fn ssh_destination(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };

        match self.user.as_deref() {
            Some(user) if !user.trim().is_empty() => format!("{user}@{host}"),
            _ => host,
        }
    }
}

struct SshDaemonTunnel {
    child: Child,
    local_port: u16,
}

impl SshDaemonTunnel {
    fn start(target: &SshDaemonTarget) -> Result<Self, String> {
        let local_port = reserve_local_loopback_port()?;
        let forward = format!("127.0.0.1:{local_port}:127.0.0.1:{}", target.daemon_port);

        let mut command = create_command("ssh");
        command
            .arg("-N")
            .arg("-T")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ExitOnForwardFailure=yes")
            .arg("-o")
            .arg("ServerAliveInterval=15")
            .arg("-o")
            .arg("ServerAliveCountMax=3")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg("-L")
            .arg(forward)
            .arg("-p")
            .arg(target.ssh_port.to_string())
            .arg(target.ssh_destination())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = command.spawn().map_err(|error| {
            format!(
                "failed to launch ssh tunnel to {}: {error}",
                target.ssh_destination()
            )
        })?;

        Ok(Self { child, local_port })
    }

    fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }

    fn stop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Drop for SshDaemonTunnel {
    fn drop(&mut self) {
        self.stop();
    }
}

struct RepositoryContextMenu {
    repository_index: usize,
    position: gpui::Point<Pixels>,
}

struct WorktreeContextMenu {
    worktree_index: usize,
    position: gpui::Point<Pixels>,
}

struct OutpostContextMenu {
    outpost_index: usize,
    position: gpui::Point<Pixels>,
}

struct WorktreeHoverPopover {
    worktree_index: usize,
    /// Vertical position of the mouse when hover started (window coords).
    mouse_y: Pixels,
    checks_expanded: bool,
}

struct CreatedWorktree {
    worktree_name: String,
    branch_name: String,
    worktree_path: PathBuf,
    checkout_kind: CheckoutKind,
    source_repo_root: PathBuf,
    review_pull_request_number: Option<u64>,
}

struct ArborWindow {
    app_config_store: Arc<dyn app_config::AppConfigStore>,
    repository_store: Arc<dyn repository_store::RepositoryStore>,
    daemon_session_store: Arc<dyn daemon::DaemonSessionStore>,
    terminal_daemon: Option<terminal_daemon_http::SharedTerminalDaemonClient>,
    daemon_base_url: String,
    ui_state_store: Arc<dyn ui_state_store::UiStateStore>,
    github_auth_store: Arc<dyn github_auth_store::GithubAuthStore>,
    github_service: Arc<dyn github_service::GitHubService>,
    github_auth_state: github_auth_store::GithubAuthState,
    github_auth_in_progress: bool,
    github_auth_copy_feedback_active: bool,
    github_auth_copy_feedback_generation: u64,
    config_last_modified: Option<SystemTime>,
    repositories: Vec<RepositorySummary>,
    active_repository_index: Option<usize>,
    repo_root: PathBuf,
    github_repo_slug: Option<String>,
    worktrees: Vec<WorktreeSummary>,
    worktree_stats_loading: bool,
    worktree_prs_loading: bool,
    active_worktree_index: Option<usize>,
    worktree_selection_epoch: usize,
    changed_files: Vec<ChangedFile>,
    selected_changed_file: Option<PathBuf>,
    terminals: Vec<TerminalSession>,
    terminal_poll_tx: std::sync::mpsc::Sender<()>,
    terminal_poll_rx: Option<std::sync::mpsc::Receiver<()>>,
    diff_sessions: Vec<DiffSession>,
    active_diff_session_id: Option<u64>,
    file_view_sessions: Vec<FileViewSession>,
    active_file_view_session_id: Option<u64>,
    next_file_view_session_id: u64,
    file_view_scroll_handle: UniformListScrollHandle,
    file_view_editing: bool,
    active_terminal_by_worktree: HashMap<PathBuf, u64>,
    next_terminal_id: u64,
    next_diff_session_id: u64,
    active_backend_kind: TerminalBackendKind,
    configured_embedded_shell: Option<String>,
    theme_kind: ThemeKind,
    left_pane_width: f32,
    right_pane_width: f32,
    terminal_focus: FocusHandle,
    welcome_clone_focus: FocusHandle,
    terminal_scroll_handle: ScrollHandle,
    last_terminal_grid_size: Option<(u16, u16)>,
    center_tabs_scroll_handle: ScrollHandle,
    diff_scroll_handle: UniformListScrollHandle,
    terminal_selection: Option<TerminalSelection>,
    terminal_selection_drag_anchor: Option<TerminalGridPosition>,
    create_modal: Option<CreateModal>,
    preferred_checkout_kind: CheckoutKind,
    github_auth_modal: Option<GitHubAuthModal>,
    delete_modal: Option<DeleteModal>,
    commit_modal: Option<CommitModal>,
    outposts: Vec<OutpostSummary>,
    outpost_store: Arc<dyn arbor_core::outpost_store::OutpostStore>,
    active_outpost_index: Option<usize>,
    remote_hosts: Vec<arbor_core::outpost::RemoteHost>,
    ssh_connection_pool: Arc<arbor_ssh::connection::SshConnectionPool>,
    ssh_daemon_tunnel: Option<SshDaemonTunnel>,
    manage_hosts_modal: Option<ManageHostsModal>,
    manage_presets_modal: Option<ManagePresetsModal>,
    agent_presets: Vec<AgentPreset>,
    active_preset_tab: Option<AgentPresetKind>,
    repo_presets: Vec<RepoPreset>,
    manage_repo_presets_modal: Option<ManageRepoPresetsModal>,
    show_about: bool,
    show_theme_picker: bool,
    theme_picker_selected_index: usize,
    settings_modal: Option<SettingsModal>,
    daemon_auth_modal: Option<DaemonAuthModal>,
    /// When set, a successful auth submission should retry fetching for this remote daemon index.
    pending_remote_daemon_auth: Option<usize>,
    pending_remote_create_repo_root: Option<String>,
    start_daemon_modal: bool,
    connect_to_host_modal: Option<ConnectToHostModal>,
    command_palette_modal: Option<CommandPaletteModal>,
    command_palette_scroll_handle: ScrollHandle,
    command_palette_recent_actions: Vec<String>,
    command_palette_task_templates: Vec<TaskTemplate>,
    compact_sidebar: bool,
    execution_mode: ExecutionMode,
    connection_history: Vec<connection_history::ConnectionHistoryEntry>,
    daemon_auth_tokens: HashMap<String, String>,
    connected_daemon_label: Option<String>,
    pending_diff_scroll_to_file: Option<PathBuf>,
    focus_terminal_on_next_render: bool,
    git_action_in_flight: Option<GitActionKind>,
    top_bar_quick_actions_open: bool,
    top_bar_quick_actions_submenu: Option<QuickActionSubmenu>,
    ide_launchers: Vec<ExternalLauncher>,
    terminal_launchers: Vec<ExternalLauncher>,
    last_persisted_ui_state: ui_state_store::UiState,
    last_ui_state_error: Option<String>,
    notification_service: Box<dyn notifications::NotificationService>,
    notifications_enabled: bool,
    last_agent_finished_notifications: HashMap<PathBuf, u64>,
    auto_checkpoint_in_flight: Arc<Mutex<HashSet<PathBuf>>>,
    agent_activity_epochs: Arc<Mutex<HashMap<PathBuf, u64>>>,
    window_is_active: bool,
    notice: Option<String>,
    theme_toast: Option<String>,
    theme_toast_generation: u64,
    right_pane_tab: RightPaneTab,
    right_pane_search: String,
    right_pane_search_cursor: usize,
    right_pane_search_active: bool,
    worktree_notes_lines: Vec<String>,
    worktree_notes_cursor: FileViewCursor,
    worktree_notes_path: Option<PathBuf>,
    worktree_notes_active: bool,
    worktree_notes_error: Option<String>,
    _worktree_notes_save_task: Option<gpui::Task<()>>,
    file_tree_entries: Vec<FileTreeEntry>,
    file_tree_loading: bool,
    expanded_dirs: HashSet<PathBuf>,
    selected_file_tree_entry: Option<PathBuf>,
    left_pane_visible: bool,
    collapsed_repositories: HashSet<usize>,
    repository_context_menu: Option<RepositoryContextMenu>,
    worktree_context_menu: Option<WorktreeContextMenu>,
    worktree_hover_popover: Option<WorktreeHoverPopover>,
    _hover_show_task: Option<gpui::Task<()>>,
    _hover_dismiss_task: Option<gpui::Task<()>>,
    _worktree_refresh_task: Option<gpui::Task<()>>,
    _changed_files_refresh_task: Option<gpui::Task<()>>,
    _config_refresh_task: Option<gpui::Task<()>>,
    _repo_metadata_refresh_task: Option<gpui::Task<()>>,
    _launcher_refresh_task: Option<gpui::Task<()>>,
    _ui_state_save_task: Option<gpui::Task<()>>,
    _daemon_session_store_save_task: Option<gpui::Task<()>>,
    _create_modal_preview_task: Option<gpui::Task<()>>,
    _file_tree_refresh_task: Option<gpui::Task<()>>,
    worktree_refresh_epoch: u64,
    config_refresh_epoch: u64,
    repo_metadata_refresh_epoch: u64,
    launcher_refresh_epoch: u64,
    last_mouse_position: gpui::Point<Pixels>,
    outpost_context_menu: Option<OutpostContextMenu>,
    discovered_daemons: Vec<mdns_browser::DiscoveredDaemon>,
    mdns_browser: Option<Box<dyn mdns_browser::MdnsDiscovery>>,
    active_discovered_daemon: Option<usize>,
    worktree_nav_back: Vec<usize>,
    worktree_nav_forward: Vec<usize>,
    log_buffer: log_layer::LogBuffer,
    log_entries: Vec<log_layer::LogEntry>,
    log_generation: u64,
    log_scroll_handle: ScrollHandle,
    log_auto_scroll: bool,
    logs_tab_open: bool,
    logs_tab_active: bool,
    quit_overlay_until: Option<Instant>,
    ime_marked_text: Option<String>,
    welcome_clone_url: String,
    welcome_clone_url_cursor: usize,
    welcome_clone_url_active: bool,
    welcome_cloning: bool,
    welcome_clone_error: Option<String>,
    /// Remote daemons that have been expanded in the sidebar.
    remote_daemon_states: HashMap<usize, RemoteDaemonState>,
    /// Currently selected remote worktree (if any). The window stays connected
    /// to the local daemon; only terminal sessions use the remote client.
    active_remote_worktree: Option<ActiveRemoteWorktree>,
}

#[derive(Debug, Clone)]
struct RemoteDaemonState {
    client: Arc<terminal_daemon_http::HttpTerminalDaemon>,
    hostname: String,
    repositories: Vec<terminal_daemon_http::RemoteRepositoryDto>,
    worktrees: Vec<terminal_daemon_http::RemoteWorktreeDto>,
    loading: bool,
    expanded: bool,
    error: Option<String>,
}

/// Tracks which remote worktree is currently selected in the sidebar,
/// without switching the window's primary daemon connection.
#[derive(Debug, Clone)]
struct ActiveRemoteWorktree {
    daemon_index: usize,
    worktree_path: PathBuf,
}
