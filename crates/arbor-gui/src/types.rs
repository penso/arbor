use {
    super::*,
    serde::{Deserialize, Serialize},
    std::sync::atomic::AtomicU64,
};

/// Identifies a sidebar item for persisted UI ordering.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum SidebarItemId {
    Worktree(PathBuf),
    Outpost(String),
}

/// Payload carried while dragging a sidebar worktree or outpost row.
#[derive(Debug, Clone)]
pub(crate) struct DraggedSidebarItem {
    pub(crate) item_id: SidebarItemId,
    pub(crate) group_key: String,
    pub(crate) label: String,
    pub(crate) icon: String,
    pub(crate) icon_color: u32,
    pub(crate) bg_color: u32,
    pub(crate) border_color: u32,
    pub(crate) text_color: u32,
}

impl Render for DraggedSidebarItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(220.))
            .font_family(FONT_MONO)
            .rounded_sm()
            .border_1()
            .border_color(rgb(self.border_color))
            .bg(rgb(self.bg_color))
            .px_2()
            .py_1()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .opacity(0.9)
            .child(
                div()
                    .flex_none()
                    .w(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(16.))
                    .text_color(rgb(self.icon_color))
                    .child(self.icon.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(self.text_color))
                    .child(self.label.clone()),
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum RepositorySidebarTab {
    #[default]
    Worktrees,
    Issues,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TerminalFontMetrics {
    pub(crate) cell_width: f32,
    pub(crate) line_height: f32,
    pub(crate) diff_cell_width: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct WorktreeSummary {
    pub(crate) group_key: String,
    pub(crate) checkout_kind: CheckoutKind,
    pub(crate) repo_root: PathBuf,
    pub(crate) path: PathBuf,
    pub(crate) label: String,
    pub(crate) branch: String,
    pub(crate) is_primary_checkout: bool,
    pub(crate) pr_loading: bool,
    pub(crate) pr_loaded: bool,
    pub(crate) pr_number: Option<u64>,
    pub(crate) pr_url: Option<String>,
    pub(crate) pr_details: Option<github_service::PrDetails>,
    pub(crate) branch_divergence: Option<BranchDivergenceSummary>,
    pub(crate) diff_summary: Option<changes::DiffLineSummary>,
    pub(crate) detected_ports: Vec<DetectedPort>,
    pub(crate) managed_processes: Vec<ManagedWorktreeProcess>,
    pub(crate) recent_turns: Vec<AgentTurnSnapshot>,
    pub(crate) stuck_turn_count: usize,
    pub(crate) recent_agent_sessions: Vec<arbor_core::session::AgentSessionSummary>,
    pub(crate) recent_agent_sessions_loaded: bool,
    pub(crate) agent_state: Option<AgentState>,
    pub(crate) agent_task: Option<String>,
    pub(crate) agent_task_loaded: bool,
    pub(crate) last_activity_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedWorktreeProcess {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) working_dir: PathBuf,
    pub(crate) source: ProcessSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BranchDivergenceSummary {
    pub(crate) ahead: usize,
    pub(crate) behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetectedPort {
    pub(crate) port: u16,
    pub(crate) pid: Option<u32>,
    pub(crate) address: String,
    pub(crate) process_name: String,
    pub(crate) label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentTurnSnapshot {
    pub(crate) timestamp_unix_ms: Option<u64>,
    pub(crate) diff_summary: Option<changes::DiffLineSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentActivitySessionRecord {
    pub(crate) cwd: String,
    pub(crate) state: AgentState,
    pub(crate) updated_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct RepositorySummary {
    pub(crate) group_key: String,
    pub(crate) root: PathBuf,
    pub(crate) checkout_roots: Vec<repository_store::RepositoryCheckoutRoot>,
    pub(crate) label: String,
    pub(crate) avatar_url: Option<String>,
    pub(crate) github_repo_slug: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum ManagedDaemonTarget {
    Primary,
    Remote(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct IssueTarget {
    pub(crate) daemon_target: ManagedDaemonTarget,
    pub(crate) repo_root: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct IssueListState {
    pub(crate) issues: Vec<terminal_daemon_http::IssueDto>,
    pub(crate) source: Option<terminal_daemon_http::IssueSourceDto>,
    pub(crate) notice: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) loading: bool,
    pub(crate) loaded: bool,
    pub(crate) refresh_generation: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct CreateModalIssueContext {
    pub(crate) source_label: String,
    pub(crate) display_id: String,
    pub(crate) title: String,
    pub(crate) url: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct IssueDetailsModal {
    pub(crate) target: IssueTarget,
    pub(crate) source_label: String,
    pub(crate) issue: terminal_daemon_http::IssueDto,
}

pub(crate) type SharedTerminalRuntime = Arc<dyn TerminalRuntimeHandle>;

#[derive(Clone)]
pub(crate) struct TerminalSession {
    pub(crate) id: u64,
    pub(crate) daemon_session_id: String,
    pub(crate) worktree_path: PathBuf,
    pub(crate) managed_process_id: Option<String>,
    pub(crate) title: String,
    pub(crate) last_command: Option<String>,
    pub(crate) pending_command: String,
    pub(crate) command: String,
    pub(crate) agent_preset: Option<AgentPresetKind>,
    pub(crate) execution_mode: Option<ExecutionMode>,
    pub(crate) state: TerminalState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) updated_at_unix_ms: Option<u64>,
    pub(crate) root_pid: Option<u32>,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
    pub(crate) generation: u64,
    pub(crate) output: String,
    pub(crate) styled_output: Vec<TerminalStyledLine>,
    pub(crate) cursor: Option<TerminalCursor>,
    pub(crate) modes: TerminalModes,
    pub(crate) last_runtime_sync_at: Option<Instant>,
    pub(crate) interactive_sync_until: Option<Instant>,
    pub(crate) last_port_hint_scan_at: Option<Instant>,
    pub(crate) queued_input: Vec<u8>,
    pub(crate) is_initializing: bool,
    pub(crate) runtime: Option<SharedTerminalRuntime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalState {
    Running,
    Completed,
    Failed,
}

/// SSH terminal shell wrapper that provides Clone (via Arc<Mutex>) and
/// a terminal emulator for rendering. Polling is done from the GUI timer
/// since libssh's Channel is not Send/Sync.
#[derive(Clone)]
pub(crate) struct SshTerminalShell {
    pub(crate) shell: Arc<Mutex<arbor_ssh::shell::SshShell>>,
    pub(crate) emulator: Arc<Mutex<arbor_terminal_emulator::TerminalEmulator>>,
    pub(crate) generation: Arc<AtomicU64>,
}

impl SshTerminalShell {
    pub(crate) fn open(
        connection: &arbor_ssh::connection::SshConnection,
        cols: u16,
        rows: u16,
        remote_path: &str,
    ) -> Result<Self, TerminalError> {
        let shell = arbor_ssh::shell::SshShell::open(
            connection.session(),
            u32::from(cols),
            u32::from(rows),
        )
        .map_err(|e| TerminalError::Pty(format!("failed to open SSH shell: {e}")))?;

        // Send cd command to navigate to the outpost directory
        shell
            .write_input(format!("cd {remote_path} && clear\n").as_bytes())
            .map_err(|e| TerminalError::Pty(format!("failed to send cd command: {e}")))?;

        Ok(Self {
            shell: Arc::new(Mutex::new(shell)),
            emulator: Arc::new(Mutex::new(
                arbor_terminal_emulator::TerminalEmulator::with_size(rows, cols),
            )),
            generation: Arc::new(AtomicU64::new(1)),
        })
    }

    pub(crate) fn write_input(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        let shell = self
            .shell
            .lock()
            .map_err(|_| TerminalError::LockPoisoned("SSH shell"))?;
        shell
            .write_input(bytes)
            .map_err(|e| TerminalError::Pty(format!("failed to write to SSH shell: {e}")))
    }

    /// Poll the shell for new output and feed it to the terminal emulator.
    /// Called from the GUI polling timer. Returns true if new data was processed.
    pub(crate) fn poll(&self) -> bool {
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
                self.generation.fetch_add(1, Ordering::Relaxed);
                true
            },
            Ok(_) | Err(_) if shell.is_closed() || shell.is_eof() => {
                drop(shell);
                self.generation.fetch_add(1, Ordering::Relaxed);
                true
            },
            _ => false,
        }
    }

    pub(crate) fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot {
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

    pub(crate) fn resize(&self, rows: u16, cols: u16) -> Result<(), TerminalError> {
        let shell = self
            .shell
            .lock()
            .map_err(|_| TerminalError::LockPoisoned("SSH shell"))?;
        shell
            .resize(u32::from(cols), u32::from(rows))
            .map_err(|e| TerminalError::Pty(format!("failed to resize SSH shell: {e}")))?;
        drop(shell);

        if let Ok(mut emulator) = self.emulator.lock() {
            emulator.resize(rows, cols);
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub(crate) fn close(&self) {
        if let Ok(shell) = self.shell.lock() {
            let _ = shell.close();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalRuntimeKind {
    Local,
    Outpost,
}

pub(crate) struct RuntimeNotification {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) play_sound: bool,
}

#[derive(Clone)]
pub(crate) struct TerminalRenderSnapshot {
    pub(crate) terminal: Arc<arbor_terminal_emulator::TerminalSnapshot>,
    pub(crate) state: TerminalState,
}

#[derive(Default)]
pub(crate) struct TerminalRuntimeSyncOutcome {
    pub(crate) changed: bool,
    pub(crate) repaint: bool,
    pub(crate) close_session: bool,
    pub(crate) clear_global_daemon: bool,
    pub(crate) notice: Option<String>,
    pub(crate) notification: Option<RuntimeNotification>,
}

pub(crate) trait EmulatorRuntimeBackend: Clone {
    fn poll(&self);
    fn write_input(&self, input: &[u8]) -> Result<(), TerminalError>;
    fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot;
    fn render_snapshot(&self) -> Option<Arc<arbor_terminal_emulator::TerminalSnapshot>> {
        None
    }
    fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), TerminalError>;
    fn generation(&self) -> u64;
    fn close(&self);
    fn background_sync_interval(
        &self,
        is_active: bool,
        session_state: TerminalState,
    ) -> Option<Duration>;
}

pub(crate) trait TerminalRuntimeHandle {
    fn kind(&self) -> TerminalRuntimeKind;
    fn sync_interval(&self, is_active: bool, session_state: TerminalState) -> Duration;
    fn background_sync_interval(
        &self,
        session: &TerminalSession,
        is_active: bool,
    ) -> Option<Duration> {
        let interval = self.sync_interval(is_active, session.state);
        (interval > Duration::ZERO).then_some(interval)
    }
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
    fn write_input(&self, session: &TerminalSession, input: &[u8]) -> Result<(), TerminalError>;
    fn session_became_active(&self, _session: &TerminalSession) {}
    fn render_snapshot(&self, _session: &TerminalSession) -> Option<TerminalRenderSnapshot> {
        None
    }
    fn sync(
        &self,
        session: &mut TerminalSession,
        is_active: bool,
        target_grid_size: Option<(u16, u16, u16, u16)>,
    ) -> TerminalRuntimeSyncOutcome;
    fn close(&self, session: &TerminalSession) -> Result<(), TerminalError>;
}

#[derive(Clone, Copy)]
pub(crate) struct RuntimeExitLabels {
    pub(crate) completed_title: &'static str,
    pub(crate) failed_title: &'static str,
    pub(crate) failed_notice_prefix: &'static str,
}

#[derive(Clone)]
pub(crate) struct EmulatorTerminalRuntime<B> {
    pub(crate) backend: B,
    pub(crate) kind: TerminalRuntimeKind,
    pub(crate) resize_error_label: &'static str,
    pub(crate) exit_labels: RuntimeExitLabels,
}

pub(crate) struct DaemonTerminalRuntime {
    pub(crate) daemon: terminal_daemon_http::SharedTerminalDaemonClient,
    pub(crate) ws_state: Arc<DaemonTerminalWsState>,
    pub(crate) last_synced_ws_generation: AtomicU64,
    pub(crate) snapshot_request_in_flight: Arc<AtomicBool>,
    pub(crate) snapshot_request_pending: Arc<AtomicBool>,
    pub(crate) kind: TerminalRuntimeKind,
    pub(crate) resize_error_label: &'static str,
    pub(crate) exit_labels: Option<RuntimeExitLabels>,
    pub(crate) clear_global_daemon_on_connection_refused: bool,
}

#[derive(Clone)]
pub(crate) struct DaemonTerminalCachedSnapshot {
    pub(crate) terminal: Arc<arbor_terminal_emulator::TerminalSnapshot>,
    pub(crate) state: TerminalState,
    pub(crate) updated_at_unix_ms: Option<u64>,
    pub(crate) ready: bool,
}

impl Default for DaemonTerminalCachedSnapshot {
    fn default() -> Self {
        Self {
            terminal: Arc::new(arbor_terminal_emulator::TerminalSnapshot {
                output: String::new(),
                styled_lines: Vec::new(),
                cursor: None,
                modes: TerminalModes::default(),
                exit_code: None,
            }),
            state: TerminalState::Running,
            updated_at_unix_ms: None,
            ready: false,
        }
    }
}

pub(crate) struct DaemonTerminalWsState {
    pub(crate) event_generation: AtomicU64,
    pub(crate) snapshot_generation: AtomicU64,
    pub(crate) snapshot_refresh_requested: AtomicBool,
    pub(crate) snapshot_build_in_flight: AtomicBool,
    pub(crate) snapshot_build_pending: AtomicBool,
    pub(crate) interactive_output_until_unix_ms: AtomicU64,
    pub(crate) closed: AtomicBool,
    pub(crate) connection_refused: AtomicBool,
    /// Channel to send keystroke bytes to the WS thread for low-latency binary transmission.
    pub(crate) ws_writer: Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
    /// Channel to wake the terminal poller when new data arrives.
    pub(crate) poll_notify: Option<std::sync::mpsc::Sender<()>>,
    pub(crate) size: Mutex<(u16, u16)>,
    pub(crate) emulator: Mutex<arbor_terminal_emulator::TerminalEmulator>,
    pub(crate) snapshot: Mutex<DaemonTerminalCachedSnapshot>,
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
    pub(crate) fn new(
        poll_notify: Option<std::sync::mpsc::Sender<()>>,
        rows: u16,
        cols: u16,
    ) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(2);
        Self {
            event_generation: AtomicU64::new(0),
            snapshot_generation: AtomicU64::new(0),
            snapshot_refresh_requested: AtomicBool::new(false),
            snapshot_build_in_flight: AtomicBool::new(false),
            snapshot_build_pending: AtomicBool::new(false),
            interactive_output_until_unix_ms: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            connection_refused: AtomicBool::new(false),
            ws_writer: Mutex::new(None),
            poll_notify,
            size: Mutex::new((rows, cols)),
            emulator: Mutex::new(arbor_terminal_emulator::TerminalEmulator::with_size(
                rows, cols,
            )),
            snapshot: Mutex::new(DaemonTerminalCachedSnapshot::default()),
        }
    }

    pub(crate) fn note_event(&self) {
        self.event_generation.fetch_add(1, Ordering::Relaxed);
        if let Some(ref tx) = self.poll_notify {
            let _ = tx.send(());
        }
    }

    pub(crate) fn note_event_with_cached_snapshot(&self) {
        let generation = self.event_generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.snapshot_generation
            .store(generation, Ordering::Relaxed);
        if let Some(ref tx) = self.poll_notify {
            let _ = tx.send(());
        }
    }

    pub(crate) fn event_generation(&self) -> u64 {
        self.event_generation.load(Ordering::Relaxed)
    }

    pub(crate) fn request_snapshot_refresh(&self) {
        self.snapshot_refresh_requested
            .store(true, Ordering::Relaxed);
        if let Some(ref tx) = self.poll_notify {
            let _ = tx.send(());
        }
    }

    pub(crate) fn enter_interactive_output_window(&self) {
        let Some(now) = current_unix_timestamp_millis() else {
            return;
        };
        let until =
            now.saturating_add(INTERACTIVE_DAEMON_INLINE_SNAPSHOT_WINDOW.as_millis() as u64);
        self.interactive_output_until_unix_ms
            .store(until, Ordering::Relaxed);
    }

    pub(crate) fn should_inline_output_snapshot(&self, byte_len: usize) -> bool {
        if byte_len == 0 || byte_len > INTERACTIVE_DAEMON_INLINE_SNAPSHOT_MAX_BYTES {
            return false;
        }

        let Some(now) = current_unix_timestamp_millis() else {
            return false;
        };
        self.interactive_output_until_unix_ms
            .load(Ordering::Relaxed)
            > now
    }

    pub(crate) fn snapshot_refresh_requested(&self) -> bool {
        self.snapshot_refresh_requested.load(Ordering::Relaxed)
    }

    pub(crate) fn take_snapshot_refresh_requested(&self) -> bool {
        self.snapshot_refresh_requested
            .swap(false, Ordering::Relaxed)
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub(crate) fn note_connection_refused(&self) {
        self.connection_refused.store(true, Ordering::Relaxed);
        self.note_event();
    }

    pub(crate) fn take_connection_refused(&self) -> bool {
        self.connection_refused.swap(false, Ordering::Relaxed)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Try to send keystroke bytes through the WebSocket channel.
    /// Returns true if sent, false if WS writer is not available.
    pub(crate) fn try_write(&self, bytes: Vec<u8>) -> bool {
        if let Ok(guard) = self.ws_writer.lock()
            && let Some(ref sender) = *guard
        {
            return sender.send(bytes).is_ok();
        }
        false
    }

    pub(crate) fn set_writer(&self, sender: Option<std::sync::mpsc::Sender<Vec<u8>>>) {
        if let Ok(mut guard) = self.ws_writer.lock() {
            *guard = sender;
        }
    }

    pub(crate) fn apply_snapshot_text(
        &self,
        ansi_output: &str,
        state: TerminalState,
        exit_code: Option<i32>,
        updated_at_unix_ms: Option<u64>,
    ) {
        let (rows, cols) = self.size.lock().map(|guard| *guard).unwrap_or((
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
            let mut snapshot = emulator.snapshot_tail(DAEMON_TERMINAL_WS_MAX_LINES);
            snapshot.exit_code = exit_code;
            snapshot
        };

        let mut cached = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *cached = DaemonTerminalCachedSnapshot {
            terminal: Arc::new(terminal_snapshot),
            state,
            updated_at_unix_ms: updated_at_unix_ms.or_else(current_unix_timestamp_millis),
            ready: true,
        };
        drop(cached);
        self.note_event_with_cached_snapshot();
    }

    pub(crate) fn apply_output_bytes(&self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }

        let inline_snapshot = {
            let mut emulator = match self.emulator.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            emulator.process(bytes);

            self.should_inline_output_snapshot(bytes.len())
                .then(|| emulator.snapshot_tail(DAEMON_TERMINAL_WS_MAX_LINES))
        };

        let Some(mut terminal_snapshot) = inline_snapshot else {
            return false;
        };

        let exit_code = self
            .snapshot
            .lock()
            .ok()
            .map(|cached| cached.terminal.exit_code)
            .unwrap_or(None);
        terminal_snapshot.exit_code = exit_code;

        let mut cached = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        cached.terminal = Arc::new(terminal_snapshot);
        cached.updated_at_unix_ms = current_unix_timestamp_millis();
        cached.ready = true;
        drop(cached);
        self.note_event_with_cached_snapshot();
        true
    }

    pub(crate) fn apply_exit(&self, state: TerminalState, exit_code: Option<i32>) {
        let mut cached = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        cached.state = state;
        let mut terminal = (*cached.terminal).clone();
        terminal.exit_code = exit_code;
        cached.terminal = Arc::new(terminal);
        cached.updated_at_unix_ms = current_unix_timestamp_millis();
        cached.ready = true;
        drop(cached);
        self.note_event_with_cached_snapshot();
    }

    pub(crate) fn resize_emulator(&self, rows: u16, cols: u16) {
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
            emulator.snapshot_tail(DAEMON_TERMINAL_WS_MAX_LINES)
        };

        let mut cached = match self.snapshot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if cached.ready {
            cached.terminal = Arc::new(terminal_snapshot);
            cached.updated_at_unix_ms = current_unix_timestamp_millis();
        }
        self.snapshot_generation
            .store(self.event_generation(), Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> Option<DaemonTerminalCachedSnapshot> {
        let current_generation = self.event_generation();
        if self.snapshot_generation.load(Ordering::Relaxed) != current_generation {
            return None;
        }

        self.snapshot
            .lock()
            .ok()
            .map(|guard| guard.clone())
            .filter(|snapshot| snapshot.ready)
    }

    pub(crate) fn has_ready_snapshot(&self) -> bool {
        self.snapshot
            .lock()
            .ok()
            .is_some_and(|snapshot| snapshot.ready)
    }
}

impl EmulatorRuntimeBackend for EmbeddedTerminal {
    fn poll(&self) {}

    fn write_input(&self, input: &[u8]) -> Result<(), TerminalError> {
        EmbeddedTerminal::write_input(self, input)
    }

    fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot {
        EmbeddedTerminal::snapshot(self)
    }

    fn render_snapshot(&self) -> Option<Arc<arbor_terminal_emulator::TerminalSnapshot>> {
        Some(EmbeddedTerminal::shared_snapshot(self))
    }

    fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), TerminalError> {
        EmbeddedTerminal::resize(self, rows, cols, pixel_width, pixel_height)
    }

    fn generation(&self) -> u64 {
        EmbeddedTerminal::generation(self)
    }

    fn close(&self) {
        EmbeddedTerminal::close(self);
    }

    fn background_sync_interval(
        &self,
        is_active: bool,
        session_state: TerminalState,
    ) -> Option<Duration> {
        event_driven_terminal_sync_interval(is_active, session_state)
    }
}

impl EmulatorRuntimeBackend for SshTerminalShell {
    fn poll(&self) {
        let _ = SshTerminalShell::poll(self);
    }

    fn write_input(&self, input: &[u8]) -> Result<(), TerminalError> {
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
    ) -> Result<(), TerminalError> {
        SshTerminalShell::resize(self, rows, cols)
    }

    fn generation(&self) -> u64 {
        SshTerminalShell::generation(self)
    }

    fn close(&self) {
        SshTerminalShell::close(self);
    }

    fn background_sync_interval(
        &self,
        is_active: bool,
        session_state: TerminalState,
    ) -> Option<Duration> {
        ssh_terminal_sync_interval(is_active, session_state)
    }
}

impl EmulatorRuntimeBackend for arbor_mosh::MoshShell {
    fn poll(&self) {}

    fn write_input(&self, input: &[u8]) -> Result<(), TerminalError> {
        arbor_mosh::MoshShell::write_input(self, input)
            .map_err(|e| TerminalError::Pty(e.to_string()))
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
    ) -> Result<(), TerminalError> {
        arbor_mosh::MoshShell::resize(self, rows, cols, pixel_width, pixel_height)
            .map_err(|e| TerminalError::Pty(e.to_string()))
    }

    fn generation(&self) -> u64 {
        arbor_mosh::MoshShell::generation(self)
    }

    fn close(&self) {
        arbor_mosh::MoshShell::close(self);
    }

    fn background_sync_interval(
        &self,
        is_active: bool,
        session_state: TerminalState,
    ) -> Option<Duration> {
        event_driven_terminal_sync_interval(is_active, session_state)
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

    fn background_sync_interval(
        &self,
        session: &TerminalSession,
        is_active: bool,
    ) -> Option<Duration> {
        self.backend
            .background_sync_interval(is_active, session.state)
    }

    fn should_sync(
        &self,
        session: &TerminalSession,
        is_active: bool,
        target_grid_size: Option<(u16, u16, u16, u16)>,
        now: Instant,
    ) -> bool {
        if self.kind == TerminalRuntimeKind::Local
            && is_active
            && let Some((rows, cols, ..)) = target_grid_size
            && (cols != session.cols || rows != session.rows)
        {
            return true;
        }

        if self.kind == TerminalRuntimeKind::Local
            && let Some(interval) = self
                .backend
                .background_sync_interval(is_active, session.state)
        {
            let generation = self.backend.generation();
            if generation == session.generation {
                return false;
            }

            return runtime_sync_interval_elapsed(
                session.last_runtime_sync_at,
                terminal_sync_interval_for_session(session, interval, now),
                now,
            );
        }

        runtime_sync_interval_elapsed(
            session.last_runtime_sync_at,
            terminal_sync_interval_for_session(
                session,
                self.sync_interval(is_active, session.state),
                now,
            ),
            now,
        )
    }

    fn write_input(&self, _session: &TerminalSession, input: &[u8]) -> Result<(), TerminalError> {
        self.backend.write_input(input)
    }

    fn render_snapshot(&self, session: &TerminalSession) -> Option<TerminalRenderSnapshot> {
        self.backend
            .render_snapshot()
            .map(|terminal| TerminalRenderSnapshot {
                terminal,
                state: session.state,
            })
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
        if apply_terminal_emulator_snapshot(session, &snapshot) {
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

    fn close(&self, _session: &TerminalSession) -> Result<(), TerminalError> {
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

    fn background_sync_interval(
        &self,
        session: &TerminalSession,
        is_active: bool,
    ) -> Option<Duration> {
        Some(self.sync_interval(is_active, session.state))
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

        if self.ws_state.snapshot_refresh_requested() {
            let interval = if is_active {
                ACTIVE_DAEMON_EVENT_COALESCE_INTERVAL
            } else {
                self.sync_interval(false, session.state)
            };
            return runtime_sync_interval_elapsed(
                session.last_runtime_sync_at,
                terminal_sync_interval_for_session(session, interval, now),
                now,
            );
        }

        let current_generation = self.ws_state.event_generation();
        let last_synced_generation = self.last_synced_ws_generation.load(Ordering::Relaxed);
        if current_generation > last_synced_generation {
            let interval = if is_active {
                ACTIVE_DAEMON_EVENT_COALESCE_INTERVAL
            } else {
                self.sync_interval(false, session.state)
            };
            return runtime_sync_interval_elapsed(
                session.last_runtime_sync_at,
                terminal_sync_interval_for_session(session, interval, now),
                now,
            );
        }

        runtime_sync_interval_elapsed(
            session.last_runtime_sync_at,
            terminal_sync_interval_for_session(
                session,
                self.sync_interval(is_active, session.state),
                now,
            ),
            now,
        )
    }

    fn write_input(&self, session: &TerminalSession, input: &[u8]) -> Result<(), TerminalError> {
        let (result, used_http_fallback) = if input == [0x03] {
            (
                self.daemon
                    .signal(SignalRequest {
                        session_id: session.daemon_session_id.clone().into(),
                        signal: TerminalSignal::Interrupt,
                    })
                    .map_err(|error| TerminalError::Pty(error.to_string())),
                false,
            )
        } else if self.ws_state.try_write(input.to_vec()) {
            tracing::trace!("write_input: sent via WS binary frame");
            (Ok(()), false)
        } else {
            tracing::trace!("write_input: WS unavailable, falling back to HTTP POST");
            (
                self.daemon
                    .write(WriteRequest {
                        session_id: session.daemon_session_id.clone().into(),
                        bytes: input.to_vec(),
                    })
                    .map_err(|error| TerminalError::Pty(error.to_string())),
                true,
            )
        };

        if result.is_ok() {
            self.ws_state.enter_interactive_output_window();
            if used_http_fallback {
                self.ws_state.request_snapshot_refresh();
                request_async_daemon_snapshot(
                    self.daemon.clone(),
                    session.daemon_session_id.clone(),
                    self.ws_state.clone(),
                    self.snapshot_request_in_flight.clone(),
                    self.snapshot_request_pending.clone(),
                );
            }
        }

        result
    }

    fn session_became_active(&self, session: &TerminalSession) {
        self.ws_state.enter_interactive_output_window();
        if self.ws_state.snapshot().is_none() {
            self.ws_state.request_snapshot_refresh();
            request_async_daemon_snapshot(
                self.daemon.clone(),
                session.daemon_session_id.clone(),
                self.ws_state.clone(),
                self.snapshot_request_in_flight.clone(),
                self.snapshot_request_pending.clone(),
            );
        }
    }

    fn render_snapshot(&self, _session: &TerminalSession) -> Option<TerminalRenderSnapshot> {
        self.ws_state
            .snapshot()
            .map(|snapshot| TerminalRenderSnapshot {
                terminal: snapshot.terminal,
                state: snapshot.state,
            })
    }

    fn sync(
        &self,
        session: &mut TerminalSession,
        is_active: bool,
        target_grid_size: Option<(u16, u16, u16, u16)>,
    ) -> TerminalRuntimeSyncOutcome {
        let mut outcome = TerminalRuntimeSyncOutcome::default();
        let observed_ws_generation = self.ws_state.event_generation();
        let last_synced_generation = self.last_synced_ws_generation.load(Ordering::Relaxed);
        let refresh_requested = self.ws_state.take_snapshot_refresh_requested();

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

        if refresh_requested {
            request_async_daemon_snapshot(
                self.daemon.clone(),
                session.daemon_session_id.clone(),
                self.ws_state.clone(),
                self.snapshot_request_in_flight.clone(),
                self.snapshot_request_pending.clone(),
            );
        }

        let Some(snapshot) = self.ws_state.snapshot() else {
            request_async_daemon_snapshot(
                self.daemon.clone(),
                session.daemon_session_id.clone(),
                self.ws_state.clone(),
                self.snapshot_request_in_flight.clone(),
                self.snapshot_request_pending.clone(),
            );
            return outcome;
        };

        self.last_synced_ws_generation
            .store(observed_ws_generation, Ordering::Relaxed);
        outcome.repaint = is_active && observed_ws_generation > last_synced_generation;

        let should_materialize_active_snapshot = !is_active
            || snapshot.state != TerminalState::Running
            || (session.output.is_empty()
                && session.styled_output.is_empty()
                && session.cursor.is_none());
        if should_materialize_active_snapshot
            && apply_terminal_emulator_snapshot(session, &snapshot.terminal)
        {
            outcome.changed = true;
        }

        if session.state != snapshot.state {
            session.state = snapshot.state;
            outcome.changed = true;
        }
        if session.exit_code != snapshot.terminal.exit_code {
            session.exit_code = snapshot.terminal.exit_code;
            outcome.changed = true;
        }
        if session.updated_at_unix_ms != snapshot.updated_at_unix_ms {
            // Keep terminal metadata fresh, but do not force a full UI redraw when
            // only the daemon timestamp changed. Hidden control traffic can update
            // the timestamp without changing any visible terminal content.
            session.updated_at_unix_ms = snapshot.updated_at_unix_ms;
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

    fn close(&self, session: &TerminalSession) -> Result<(), TerminalError> {
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

        result.map_err(|error| TerminalError::Pty(error.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CenterTab {
    Terminal(u64),
    Diff(u64),
    FileView(u64),
    AgentChat(u64),
    Logs,
}

/// A chat message in the native agent chat UI.
#[derive(Debug, Clone)]
pub(crate) struct AgentChatMessage {
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) tool_calls: Vec<String>,
    /// Per-turn input tokens (only meaningful for assistant messages).
    pub(crate) input_tokens: u64,
    /// Per-turn output tokens (only meaningful for assistant messages).
    pub(crate) output_tokens: u64,
    /// Output tokens per second for this turn.
    pub(crate) tokens_per_sec: Option<f64>,
    /// Model used for this turn.
    pub(crate) model_id: Option<String>,
    /// Transport label for debugging (e.g. "acp:claude", "openai:http://…").
    pub(crate) transport_label: Option<String>,
}

/// Local state for an agent chat session displayed in the native GUI.
#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "agent-chat"), allow(dead_code))]
pub(crate) struct NativeAgentChatSession {
    /// Local numeric ID for use in `CenterTab::AgentChat`.
    pub(crate) local_id: u64,
    /// Daemon-managed session ID (string).
    pub(crate) session_id: String,
    /// Agent kind / provider key (e.g. "claude", "codex").
    pub(crate) agent_kind: String,
    /// Selected model ID within the provider (e.g. "claude-opus").
    /// When `None`, the provider default is used.
    pub(crate) selected_model_id: Option<String>,
    /// Workspace path this chat is associated with.
    pub(crate) workspace_path: PathBuf,
    /// Current status from the daemon.
    pub(crate) status: String,
    /// Conversation messages.
    pub(crate) messages: Vec<AgentChatMessage>,
    /// User input text being composed.
    pub(crate) input_text: String,
    /// Cursor position in input text.
    pub(crate) input_cursor: usize,
    /// Cumulative input token usage.
    pub(crate) input_tokens: u64,
    /// Cumulative output token usage.
    pub(crate) output_tokens: u64,
    /// Cumulative input tokens at the start of the current turn (for delta).
    pub(crate) turn_start_input_tokens: u64,
    /// Cumulative output tokens at the start of the current turn (for delta).
    pub(crate) turn_start_output_tokens: u64,
    /// Wall-clock time when the current turn started (for speed calc).
    pub(crate) turn_start_time: Option<Instant>,
    /// Characters streamed during the current turn (for estimated token count).
    pub(crate) turn_streamed_chars: usize,
    /// Transport label from the daemon (e.g. "acp:claude", "openai:http://…").
    pub(crate) transport_label: Option<String>,
    /// Permission mode for this chat session.
    pub(crate) chat_mode: AgentChatMode,
}

/// Permission/autonomy mode for an agent chat session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AgentChatMode {
    /// Create a plan before making changes.
    Plan,
    /// Ask permission before making changes.
    #[default]
    AskPermission,
    /// Automatically accept edits.
    AutoAccept,
    /// Full permissions, no approval needed.
    Bypass,
}

impl AgentChatMode {
    pub(crate) const ORDER: [Self; 4] = [
        Self::AskPermission,
        Self::AutoAccept,
        Self::Plan,
        Self::Bypass,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan mode",
            Self::AskPermission => "Ask permissions",
            Self::AutoAccept => "Auto accept edits",
            Self::Bypass => "Bypass permissions",
        }
    }

    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            Self::Plan => "Create a plan before making changes",
            Self::AskPermission => "Always ask before making changes",
            Self::AutoAccept => "Automatically accept all file edits",
            Self::Bypass => "Accepts all permissions",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Plan => "\u{f0c9}",          // nf-fa-bars
            Self::AskPermission => "\u{f013}", // nf-fa-gear
            Self::AutoAccept => "\u{f00c}",    // nf-fa-check
            Self::Bypass => "\u{f0e7}",        // nf-fa-bolt
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RightPaneTab {
    Changes,
    FileTree,
    Procfile,
    Notes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AgentPresetKind {
    Claude,
    Codex,
    Copilot,
    Cursor,
    Droid,
    Gemini,
    Iflow,
    Kilocode,
    Kimi,
    Kiro,
    OpenClaw,
    OpenCode,
    Pi,
    Qwen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ExecutionMode {
    Plan,
    Build,
    Yolo,
}

impl ExecutionMode {
    pub(crate) const ORDER: [Self; 3] = [Self::Plan, Self::Build, Self::Yolo];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Build => "Build",
            Self::Yolo => "Yolo",
        }
    }

    pub(crate) fn subtitle(self) -> &'static str {
        match self {
            Self::Plan => "Minimal write access",
            Self::Build => "Normal autonomous work",
            Self::Yolo => "Full permissions",
        }
    }
}

impl AgentPresetKind {
    /// All known agent kinds, in display order.
    pub(crate) const ORDER: [Self; 14] = [
        Self::Claude,
        Self::Codex,
        Self::Copilot,
        Self::Cursor,
        Self::Droid,
        Self::Gemini,
        Self::Iflow,
        Self::Kilocode,
        Self::Kimi,
        Self::Kiro,
        Self::OpenClaw,
        Self::OpenCode,
        Self::Pi,
        Self::Qwen,
    ];

    /// The acpx subcommand name for this agent.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
            Self::Droid => "droid",
            Self::Gemini => "gemini",
            Self::Iflow => "iflow",
            Self::Kilocode => "kilocode",
            Self::Kimi => "kimi",
            Self::Kiro => "kiro",
            Self::OpenClaw => "openclaw",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::Qwen => "qwen",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Copilot => "Copilot",
            Self::Cursor => "Cursor",
            Self::Droid => "Droid",
            Self::Gemini => "Gemini",
            Self::Iflow => "iFlow",
            Self::Kilocode => "Kilocode",
            Self::Kimi => "Kimi",
            Self::Kiro => "Kiro",
            Self::OpenClaw => "OpenClaw",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
            Self::Qwen => "Qwen",
        }
    }

    pub(crate) fn fallback_icon(self) -> &'static str {
        match self {
            Self::Claude => "C",
            Self::Codex => "\u{f121}",   // nf-fa-code
            Self::Copilot => "\u{f09b}", // nf-fa-github
            Self::Cursor => "\u{f245}",  // nf-fa-mouse_pointer
            Self::Droid => "\u{f17b}",   // nf-fa-android
            Self::Gemini => "G",
            Self::Iflow => "\u{f126}", // nf-fa-code_fork
            Self::Kilocode => "K",
            Self::Kimi => "\u{f005}",     // nf-fa-star
            Self::Kiro => "\u{f0e7}",     // nf-fa-bolt
            Self::OpenClaw => "\u{f085}", // nf-fa-cogs
            Self::OpenCode => "\u{f085}", // nf-fa-cogs
            Self::Pi => "P",
            Self::Qwen => "Q",
        }
    }

    pub(crate) fn default_command(self) -> &'static str {
        match self {
            Self::Codex => {
                "codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox -c model_reasoning_summary=\"detailed\" -c model_supports_reasoning_summaries=true"
            },
            Self::Claude => "claude --dangerously-skip-permissions",
            Self::Pi => "pi",
            Self::OpenCode => "opencode",
            Self::Copilot => "copilot --allow-all",
            Self::Cursor => "cursor",
            Self::Droid => "droid",
            Self::Gemini => "gemini",
            Self::Iflow => "iflow",
            Self::Kilocode => "kilocode",
            Self::Kimi => "kimi",
            Self::Kiro => "kiro",
            Self::OpenClaw => "openclaw",
            Self::Qwen => "qwen",
        }
    }

    pub(crate) fn executable_name(self) -> &'static str {
        self.key()
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "copilot" => Some(Self::Copilot),
            "cursor" => Some(Self::Cursor),
            "droid" => Some(Self::Droid),
            "gemini" => Some(Self::Gemini),
            "iflow" => Some(Self::Iflow),
            "kilocode" => Some(Self::Kilocode),
            "kimi" => Some(Self::Kimi),
            "kiro" => Some(Self::Kiro),
            "openclaw" => Some(Self::OpenClaw),
            "opencode" => Some(Self::OpenCode),
            "pi" => Some(Self::Pi),
            "qwen" => Some(Self::Qwen),
            _ => None,
        }
    }

    pub(crate) fn cycle(self, reverse: bool) -> Self {
        let installed = installed_preset_kinds();
        let order: Vec<Self> = Self::ORDER
            .iter()
            .copied()
            .filter(|k| installed.contains(k))
            .collect();
        if order.is_empty() {
            return self;
        }
        let current = order
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        if reverse {
            order[(current + order.len() - 1) % order.len()]
        } else {
            order[(current + 1) % order.len()]
        }
    }

    /// Check if the default command for this preset is available in PATH.
    pub(crate) fn is_installed(self) -> bool {
        is_command_in_path(self.executable_name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentPreset {
    pub(crate) kind: AgentPresetKind,
    pub(crate) command: String,
}

/// A model offered by a provider (agent preset), used in the model selector popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentModel {
    pub(crate) provider: AgentPresetKind,
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
}

/// A model from an OpenAI-compatible provider configured in config.toml.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ConfiguredModel {
    /// Provider name (e.g. "Ollama").
    pub(crate) provider_name: String,
    /// Model identifier sent to the API.
    pub(crate) id: String,
    /// Human-readable display name.
    pub(crate) label: String,
}

/// A provider loaded from `[[providers]]` in config.toml.
#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "agent-chat"), allow(dead_code))]
pub(crate) struct ConfiguredProvider {
    /// Display name (e.g. "Ollama", "LM Studio").
    pub(crate) name: String,
    /// API base URL.
    pub(crate) base_url: String,
    /// Resolved API key (from config or env var).
    pub(crate) api_key: Option<String>,
    /// Whether to probe `/v1/models` for model discovery.
    pub(crate) fetch_models: bool,
    /// Discovered or configured models.
    pub(crate) models: Vec<ConfiguredModel>,
}

/// An entry in the model selector popup — either a section header or a model.
#[derive(Debug, Clone)]
pub(crate) enum ModelSelectorEntry {
    /// Section header (provider name).
    Separator(String),
    /// An ACP agent model (via acpx).
    AcpModel(AgentModel),
    /// An OpenAI-compatible model (from config.toml).
    ApiModel(ConfiguredModel),
}

impl AgentPresetKind {
    /// Models available for each provider.
    pub(crate) fn models(self) -> &'static [AgentModel] {
        match self {
            Self::Claude => &[
                AgentModel {
                    provider: Self::Claude,
                    id: "claude-sonnet-4-20250514",
                    label: "Claude Sonnet 4",
                },
                AgentModel {
                    provider: Self::Claude,
                    id: "claude-opus-4-20250514",
                    label: "Claude Opus 4",
                },
                AgentModel {
                    provider: Self::Claude,
                    id: "claude-3-5-haiku-20241022",
                    label: "Claude Haiku 3.5",
                },
            ],
            Self::Codex => &[
                AgentModel {
                    provider: Self::Codex,
                    id: "o4-mini",
                    label: "o4-mini",
                },
                AgentModel {
                    provider: Self::Codex,
                    id: "o3",
                    label: "o3",
                },
            ],
            Self::Copilot => &[AgentModel {
                provider: Self::Copilot,
                id: "copilot",
                label: "Copilot",
            }],
            Self::Cursor => &[AgentModel {
                provider: Self::Cursor,
                id: "cursor",
                label: "Cursor",
            }],
            Self::Droid => &[AgentModel {
                provider: Self::Droid,
                id: "droid",
                label: "Droid",
            }],
            Self::Gemini => &[
                AgentModel {
                    provider: Self::Gemini,
                    id: "gemini-2.5-pro",
                    label: "Gemini 2.5 Pro",
                },
                AgentModel {
                    provider: Self::Gemini,
                    id: "gemini-2.5-flash",
                    label: "Gemini 2.5 Flash",
                },
            ],
            Self::Iflow => &[AgentModel {
                provider: Self::Iflow,
                id: "iflow",
                label: "iFlow",
            }],
            Self::Kilocode => &[AgentModel {
                provider: Self::Kilocode,
                id: "kilocode",
                label: "Kilocode",
            }],
            Self::Kimi => &[AgentModel {
                provider: Self::Kimi,
                id: "kimi",
                label: "Kimi",
            }],
            Self::Kiro => &[AgentModel {
                provider: Self::Kiro,
                id: "kiro",
                label: "Kiro",
            }],
            Self::OpenClaw => &[AgentModel {
                provider: Self::OpenClaw,
                id: "openclaw",
                label: "OpenClaw",
            }],
            Self::OpenCode => &[AgentModel {
                provider: Self::OpenCode,
                id: "opencode",
                label: "OpenCode",
            }],
            Self::Pi => &[AgentModel {
                provider: Self::Pi,
                id: "pi",
                label: "Pi",
            }],
            Self::Qwen => &[AgentModel {
                provider: Self::Qwen,
                id: "qwen",
                label: "Qwen",
            }],
        }
    }

    /// Provider display name for section headers.
    pub(crate) fn provider_name(self) -> &'static str {
        match self {
            Self::Claude => "Anthropic",
            Self::Codex => "OpenAI",
            Self::Copilot => "GitHub",
            Self::Cursor => "Cursor",
            Self::Droid => "Samsung",
            Self::Gemini => "Google",
            Self::Iflow => "iFlow",
            Self::Kilocode => "Kilocode",
            Self::Kimi => "Moonshot",
            Self::Kiro => "Amazon",
            Self::OpenClaw => "OpenClaw",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
            Self::Qwen => "Alibaba",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SettingsModal {
    pub(crate) active_control: SettingsControl,
    pub(crate) daemon_bind_mode: DaemonBindMode,
    pub(crate) initial_daemon_bind_mode: DaemonBindMode,
    pub(crate) notifications: bool,
    pub(crate) daemon_auth_token: String,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsControl {
    DaemonBindMode,
    Notifications,
}

impl SettingsControl {
    pub(crate) fn cycle(self, reverse: bool) -> Self {
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
pub(crate) enum DaemonBindMode {
    Localhost,
    AllInterfaces,
}

impl DaemonBindMode {
    pub(crate) fn from_config(raw: Option<&str>) -> Self {
        match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("localhost" | "local" | "loopback" | "127.0.0.1") => Self::Localhost,
            Some("all" | "all-interfaces" | "public" | "0.0.0.0") => Self::AllInterfaces,
            _ => Self::AllInterfaces,
        }
    }

    pub(crate) fn as_config_value(self) -> &'static str {
        match self {
            Self::Localhost => "localhost",
            Self::AllInterfaces => "all-interfaces",
        }
    }
}

pub(crate) enum SettingsModalInputEvent {
    CycleControl(bool),
    SelectDaemonBindMode(DaemonBindMode),
    ToggleActiveControl,
    ToggleNotifications,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagePresetsModal {
    pub(crate) active_preset: AgentPresetKind,
    pub(crate) command: String,
    pub(crate) command_cursor: usize,
    pub(crate) error: Option<String>,
}

pub(crate) enum PresetsModalInputEvent {
    SetActivePreset(AgentPresetKind),
    CycleActivePreset(bool),
    Edit(TextEditAction),
    RestoreDefault,
    ClearError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoPreset {
    pub(crate) name: String,
    pub(crate) icon: String,
    pub(crate) command: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepoPresetModalField {
    Icon,
    Name,
    Command,
}

impl RepoPresetModalField {
    pub(crate) const ORDER: [Self; 3] = [Self::Icon, Self::Name, Self::Command];

    pub(crate) fn next(self) -> Self {
        let index = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(index + 1) % Self::ORDER.len()]
    }

    pub(crate) fn prev(self) -> Self {
        let index = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0);
        Self::ORDER[(index + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ManageRepoPresetsModal {
    pub(crate) editing_index: Option<usize>,
    pub(crate) icon: String,
    pub(crate) icon_cursor: usize,
    pub(crate) name: String,
    pub(crate) name_cursor: usize,
    pub(crate) command: String,
    pub(crate) command_cursor: usize,
    pub(crate) active_tab: RepoPresetModalTab,
    pub(crate) active_field: RepoPresetModalField,
    pub(crate) error: Option<String>,
    pub(crate) saving: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepoPresetModalTab {
    Edit,
    LocalPreset,
}

pub(crate) enum RepoPresetsModalInputEvent {
    SetActiveTab(RepoPresetModalTab),
    SetActiveField(RepoPresetModalField),
    MoveActiveField(bool),
    Edit(TextEditAction),
    ClearError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitActionKind {
    Commit,
    CommitPushCreatePullRequest,
    Push,
    CreatePullRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeQuickAction {
    OpenFinder,
    CopyPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickActionSubmenu {
    Ide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalLauncherKind {
    Command(&'static str),
    MacApp(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExternalLauncher {
    pub(crate) label: &'static str,
    pub(crate) icon: &'static str,
    pub(crate) icon_color: u32,
    pub(crate) kind: ExternalLauncherKind,
}

#[derive(Debug, Clone)]
pub(crate) struct FileTreeEntry {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) is_dir: bool,
    pub(crate) depth: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffLineKind {
    FileHeader,
    Context,
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone)]
pub(crate) struct DiffLine {
    pub(crate) left_line_number: Option<usize>,
    pub(crate) right_line_number: Option<usize>,
    pub(crate) left_text: String,
    pub(crate) right_text: String,
    pub(crate) kind: DiffLineKind,
}

#[derive(Debug, Clone)]
pub(crate) struct DiffSession {
    pub(crate) id: u64,
    pub(crate) worktree_path: PathBuf,
    pub(crate) title: String,
    pub(crate) raw_lines: Arc<[DiffLine]>,
    pub(crate) raw_file_row_indices: HashMap<PathBuf, usize>,
    pub(crate) lines: Arc<[DiffLine]>,
    pub(crate) file_row_indices: HashMap<PathBuf, usize>,
    pub(crate) wrapped_columns: usize,
    pub(crate) is_loading: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct FileViewSpan {
    pub(crate) text: String,
    pub(crate) color: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum FileViewContent {
    Text {
        highlighted: Arc<[Vec<FileViewSpan>]>,
        raw_lines: Vec<String>,
        dirty: bool,
    },
    Image(PathBuf),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FileViewCursor {
    pub(crate) line: usize,
    pub(crate) col: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct FileViewSession {
    pub(crate) id: u64,
    pub(crate) worktree_path: PathBuf,
    pub(crate) file_path: PathBuf,
    pub(crate) title: String,
    pub(crate) content: FileViewContent,
    pub(crate) is_loading: bool,
    pub(crate) cursor: FileViewCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DraggedPaneDivider {
    Left,
    Right,
}

impl Render for DraggedPaneDivider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalGridPosition {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalSelection {
    pub(crate) session_id: u64,
    pub(crate) anchor: TerminalGridPosition,
    pub(crate) head: TerminalGridPosition,
}

#[derive(Debug, Clone)]
pub(crate) struct OutpostSummary {
    pub(crate) outpost_id: String,
    pub(crate) repo_root: PathBuf,
    pub(crate) remote_path: String,
    pub(crate) label: String,
    pub(crate) branch: String,
    pub(crate) host_name: String,
    pub(crate) hostname: String,
    pub(crate) status: arbor_core::outpost::OutpostStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateModalTab {
    LocalWorktree,
    ReviewPullRequest,
    RemoteOutpost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateOutpostField {
    HostSelector,
    CloneUrl,
    OutpostName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateWorktreeField {
    RepositoryPath,
    WorktreeName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateReviewPrField {
    RepositoryPath,
    PullRequestReference,
    WorktreeName,
}

#[derive(Debug, Clone)]
pub(crate) struct CreateModal {
    pub(crate) instance_id: u64,
    pub(crate) tab: CreateModalTab,
    // Worktree fields
    pub(crate) repository_path: String,
    pub(crate) repository_path_cursor: usize,
    pub(crate) worktree_name: String,
    pub(crate) worktree_name_cursor: usize,
    pub(crate) checkout_kind: CheckoutKind,
    pub(crate) worktree_active_field: CreateWorktreeField,
    // Review PR fields
    pub(crate) pr_reference: String,
    pub(crate) pr_reference_cursor: usize,
    pub(crate) review_active_field: CreateReviewPrField,
    // Outpost fields
    pub(crate) host_index: usize,
    pub(crate) host_dropdown_open: bool,
    pub(crate) clone_url: String,
    pub(crate) clone_url_cursor: usize,
    pub(crate) outpost_name: String,
    pub(crate) outpost_name_cursor: usize,
    pub(crate) outpost_active_field: CreateOutpostField,
    pub(crate) daemon_managed_target: Option<ManagedDaemonTarget>,
    pub(crate) managed_preview: Option<terminal_daemon_http::ManagedWorktreePreviewDto>,
    pub(crate) managed_preview_loading: bool,
    pub(crate) managed_preview_error: Option<String>,
    pub(crate) managed_preview_generation: u64,
    pub(crate) branch_preview_generation: u64,
    pub(crate) local_branch_preview: String,
    pub(crate) review_branch_preview: String,
    pub(crate) outpost_branch_preview: String,
    pub(crate) issue_context: Option<CreateModalIssueContext>,
    // Shared
    pub(crate) is_creating: bool,
    pub(crate) creating_status: Option<String>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct GitHubAuthModal {
    pub(crate) user_code: String,
    pub(crate) verification_url: String,
}

pub(crate) enum ModalInputEvent {
    SetActiveField(CreateWorktreeField),
    MoveActiveField,
    Edit(TextEditAction),
    ClearError,
}

pub(crate) enum ReviewPrModalInputEvent {
    SetActiveField(CreateReviewPrField),
    MoveActiveField,
    Edit(TextEditAction),
    ClearError,
}

pub(crate) enum OutpostModalInputEvent {
    SetActiveField(CreateOutpostField),
    MoveActiveField(bool),
    CycleHost(bool),
    SelectHost(usize),
    ToggleHostDropdown,
    Edit(TextEditAction),
    ClearError,
}

#[derive(Clone)]
pub(crate) struct ManageHostsModal {
    pub(crate) adding: bool,
    pub(crate) name: String,
    pub(crate) name_cursor: usize,
    pub(crate) hostname: String,
    pub(crate) hostname_cursor: usize,
    pub(crate) user: String,
    pub(crate) user_cursor: usize,
    pub(crate) active_field: ManageHostsField,
    pub(crate) error: Option<String>,
    pub(crate) saving: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManageHostsField {
    Name,
    Hostname,
    User,
}

pub(crate) enum HostsModalInputEvent {
    SetActiveField(ManageHostsField),
    MoveActiveField(bool),
    Edit(TextEditAction),
    ClearError,
}

#[derive(Debug, Clone)]
pub(crate) enum DeleteTarget {
    Worktree(usize),
    Outpost(usize),
    Repository(usize),
}

#[derive(Debug, Clone)]
pub(crate) struct DeleteModal {
    pub(crate) target: DeleteTarget,
    pub(crate) label: String,
    pub(crate) branch: String,
    pub(crate) has_unpushed: Option<bool>,
    pub(crate) delete_branch: bool,
    pub(crate) is_deleting: bool,
    pub(crate) error: Option<String>,
}

pub(crate) struct DaemonAuthModal {
    pub(crate) daemon_url: String,
    pub(crate) token: String,
    pub(crate) token_cursor: usize,
    pub(crate) error: Option<String>,
}

pub(crate) struct ConnectToHostModal {
    pub(crate) address: String,
    pub(crate) address_cursor: usize,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CommitModal {
    pub(crate) message: String,
    pub(crate) message_cursor: usize,
    pub(crate) generating: bool,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CommandPaletteModal {
    pub(crate) scope: CommandPaletteScope,
    pub(crate) query: String,
    pub(crate) query_cursor: usize,
    pub(crate) selected_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandPaletteScope {
    Actions,
    Issues,
}

#[derive(Debug, Clone)]
pub(crate) struct CommandPaletteItem {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) search_text: String,
    pub(crate) action: CommandPaletteAction,
}

#[derive(Debug, Clone)]
pub(crate) enum CommandPaletteAction {
    OpenCreateWorktree,
    BrowseIssues,
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
    OpenIssueCreateModal(Box<terminal_daemon_http::IssueDto>),
    LaunchTaskTemplate(TaskTemplate),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskTemplate {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) prompt: String,
    pub(crate) agent: Option<AgentPresetKind>,
    pub(crate) path: PathBuf,
    pub(crate) repo_root: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) enum TextEditAction {
    Insert(String),
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    MoveHome,
    MoveEnd,
}

pub(crate) enum ConnectHostTarget {
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
pub(crate) struct SshDaemonTarget {
    pub(crate) user: Option<String>,
    pub(crate) host: String,
    pub(crate) ssh_port: u16,
    pub(crate) daemon_port: u16,
}

impl SshDaemonTarget {
    pub(crate) fn ssh_destination(&self) -> String {
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

pub(crate) struct SshDaemonTunnel {
    pub(crate) child: Child,
    pub(crate) local_port: u16,
}

impl SshDaemonTunnel {
    pub(crate) fn start(target: &SshDaemonTarget) -> Result<Self, ConnectionError> {
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
            ConnectionError::Io(format!(
                "failed to launch ssh tunnel to {}: {error}",
                target.ssh_destination()
            ))
        })?;

        Ok(Self { child, local_port })
    }

    pub(crate) fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }

    pub(crate) fn stop(&mut self) {
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

pub(crate) struct RepositoryContextMenu {
    pub(crate) repository_index: usize,
    pub(crate) position: gpui::Point<Pixels>,
}

pub(crate) struct WorktreeContextMenu {
    pub(crate) worktree_index: usize,
    pub(crate) position: gpui::Point<Pixels>,
}

pub(crate) struct OutpostContextMenu {
    pub(crate) outpost_index: usize,
    pub(crate) position: gpui::Point<Pixels>,
}

pub(crate) struct WorktreeHoverPopover {
    pub(crate) worktree_index: usize,
    /// Vertical position of the mouse when hover started (window coords).
    pub(crate) mouse_y: Pixels,
    pub(crate) checks_expanded: bool,
}

pub(crate) struct CreatedWorktree {
    pub(crate) worktree_name: String,
    pub(crate) branch_name: String,
    pub(crate) worktree_path: PathBuf,
    pub(crate) checkout_kind: CheckoutKind,
    pub(crate) source_repo_root: PathBuf,
    pub(crate) review_pull_request_number: Option<u64>,
}

#[derive(Debug, Default)]
pub(crate) struct PendingSave<T> {
    pub(crate) pending: Option<T>,
    pub(crate) in_flight: bool,
}

impl<T> PendingSave<T> {
    pub(crate) fn queue(&mut self, value: T) {
        self.pending = Some(value);
    }

    pub(crate) fn begin_next(&mut self) -> Option<T> {
        if self.in_flight {
            return None;
        }

        let value = self.pending.take()?;
        self.in_flight = true;
        Some(value)
    }

    pub(crate) fn finish(&mut self) {
        self.in_flight = false;
    }

    pub(crate) fn has_work(&self) -> bool {
        self.in_flight || self.pending.is_some()
    }
}

pub(crate) struct ArborWindow {
    pub(crate) app_config_store: Arc<dyn app_config::AppConfigStore>,
    pub(crate) repository_store: Arc<dyn repository_store::RepositoryStore>,
    pub(crate) daemon_session_store: Arc<dyn daemon::DaemonSessionStore>,
    pub(crate) terminal_daemon: Option<terminal_daemon_http::SharedTerminalDaemonClient>,
    pub(crate) daemon_base_url: String,
    pub(crate) ui_state_store: Arc<dyn ui_state_store::UiStateStore>,
    pub(crate) issue_cache_store: Arc<dyn issue_cache_store::IssueCacheStore>,
    pub(crate) github_auth_store: Arc<dyn github_auth_store::GithubAuthStore>,
    pub(crate) github_service: Arc<dyn github_service::GitHubService>,
    pub(crate) github_auth_state: github_auth_store::GithubAuthState,
    pub(crate) github_auth_in_progress: bool,
    pub(crate) github_auth_copy_feedback_active: bool,
    pub(crate) github_auth_copy_feedback_generation: u64,
    pub(crate) next_create_modal_instance_id: u64,
    pub(crate) config_last_modified: Option<SystemTime>,
    pub(crate) repositories: Vec<RepositorySummary>,
    pub(crate) active_repository_index: Option<usize>,
    pub(crate) repo_root: PathBuf,
    pub(crate) github_repo_slug: Option<String>,
    pub(crate) worktrees: Vec<WorktreeSummary>,
    pub(crate) worktree_stats_loading: bool,
    pub(crate) worktree_prs_loading: bool,
    pub(crate) pending_startup_worktree_restore: bool,
    pub(crate) loading_animation_active: bool,
    pub(crate) loading_animation_frame: usize,
    pub(crate) github_rate_limited_until: Option<SystemTime>,
    pub(crate) expanded_pr_checks_worktree: Option<PathBuf>,
    pub(crate) active_worktree_index: Option<usize>,
    pub(crate) pending_local_worktree_selection: Option<PathBuf>,
    pub(crate) worktree_selection_epoch: usize,
    pub(crate) changed_files: Vec<ChangedFile>,
    pub(crate) selected_changed_file: Option<PathBuf>,
    pub(crate) terminals: Vec<TerminalSession>,
    pub(crate) terminal_poll_tx: std::sync::mpsc::Sender<()>,
    pub(crate) terminal_poll_rx: Option<std::sync::mpsc::Receiver<()>>,
    pub(crate) diff_sessions: Vec<DiffSession>,
    pub(crate) active_diff_session_id: Option<u64>,
    pub(crate) file_view_sessions: Vec<FileViewSession>,
    pub(crate) active_file_view_session_id: Option<u64>,
    pub(crate) next_file_view_session_id: u64,
    pub(crate) file_view_scroll_handle: UniformListScrollHandle,
    pub(crate) file_view_editing: bool,
    pub(crate) active_terminal_by_worktree: HashMap<PathBuf, u64>,
    pub(crate) next_terminal_id: u64,
    pub(crate) next_diff_session_id: u64,
    pub(crate) active_backend_kind: TerminalBackendKind,
    pub(crate) configured_embedded_shell: Option<String>,
    pub(crate) theme_kind: ThemeKind,
    pub(crate) left_pane_width: f32,
    pub(crate) right_pane_width: f32,
    pub(crate) terminal_focus: FocusHandle,
    pub(crate) issue_details_focus: FocusHandle,
    pub(crate) welcome_clone_focus: FocusHandle,
    pub(crate) terminal_scroll_handle: ScrollHandle,
    pub(crate) issue_details_scroll_handle: ScrollHandle,
    pub(crate) issue_details_scrollbar_drag_offset: Option<Pixels>,
    pub(crate) last_terminal_grid_size: Option<(u16, u16)>,
    pub(crate) terminal_font_metrics: Option<TerminalFontMetrics>,
    pub(crate) center_tabs_scroll_handle: ScrollHandle,
    pub(crate) center_tabs_last_scrolled_index: Option<usize>,
    pub(crate) diff_scroll_handle: UniformListScrollHandle,
    pub(crate) terminal_selection: Option<TerminalSelection>,
    pub(crate) terminal_selection_drag_anchor: Option<TerminalGridPosition>,
    pub(crate) create_modal: Option<CreateModal>,
    pub(crate) issue_details_modal: Option<IssueDetailsModal>,
    pub(crate) preferred_checkout_kind: CheckoutKind,
    pub(crate) github_auth_modal: Option<GitHubAuthModal>,
    pub(crate) delete_modal: Option<DeleteModal>,
    pub(crate) commit_modal: Option<CommitModal>,
    pub(crate) outposts: Vec<OutpostSummary>,
    pub(crate) outpost_store: Arc<dyn arbor_core::outpost_store::OutpostStore>,
    pub(crate) active_outpost_index: Option<usize>,
    pub(crate) remote_hosts: Vec<arbor_core::outpost::RemoteHost>,
    pub(crate) ssh_connection_pool: Arc<arbor_ssh::connection::SshConnectionPool>,
    pub(crate) ssh_daemon_tunnel: Option<SshDaemonTunnel>,
    pub(crate) manage_hosts_modal: Option<ManageHostsModal>,
    pub(crate) manage_presets_modal: Option<ManagePresetsModal>,
    pub(crate) agent_presets: Vec<AgentPreset>,
    /// OpenAI-compatible providers loaded from `[[providers]]` in config.toml.
    pub(crate) configured_providers: Vec<ConfiguredProvider>,
    pub(crate) active_preset_tab: Option<AgentPresetKind>,
    pub(crate) repo_presets: Vec<RepoPreset>,
    pub(crate) manage_repo_presets_modal: Option<ManageRepoPresetsModal>,
    pub(crate) show_about: bool,
    pub(crate) show_theme_picker: bool,
    pub(crate) theme_picker_selected_index: usize,
    pub(crate) theme_picker_scroll_handle: ScrollHandle,
    pub(crate) settings_modal: Option<SettingsModal>,
    pub(crate) daemon_auth_modal: Option<DaemonAuthModal>,
    /// When set, a successful auth submission should retry fetching for this remote daemon index.
    pub(crate) pending_remote_daemon_auth: Option<usize>,
    pub(crate) pending_remote_create_repo_root: Option<String>,
    pub(crate) start_daemon_modal: bool,
    pub(crate) connect_to_host_modal: Option<ConnectToHostModal>,
    pub(crate) command_palette_modal: Option<CommandPaletteModal>,
    pub(crate) command_palette_scroll_handle: ScrollHandle,
    pub(crate) command_palette_recent_actions: Vec<String>,
    pub(crate) command_palette_task_templates: Vec<TaskTemplate>,
    pub(crate) compact_sidebar: bool,
    pub(crate) execution_mode: ExecutionMode,
    pub(crate) connection_history: Vec<connection_history::ConnectionHistoryEntry>,
    pub(crate) connection_history_save:
        PendingSave<Vec<connection_history::ConnectionHistoryEntry>>,
    pub(crate) repository_entries_save: PendingSave<Vec<repository_store::StoredRepositoryEntry>>,
    pub(crate) daemon_auth_tokens: HashMap<String, String>,
    pub(crate) daemon_auth_tokens_save: PendingSave<HashMap<String, String>>,
    pub(crate) github_auth_state_save: PendingSave<github_auth_store::GithubAuthState>,
    pub(crate) pending_app_config_save_count: usize,
    pub(crate) connected_daemon_label: Option<String>,
    pub(crate) daemon_connect_epoch: u64,
    pub(crate) pending_diff_scroll_to_file: Option<PathBuf>,
    pub(crate) focus_terminal_on_next_render: bool,
    pub(crate) git_action_in_flight: Option<GitActionKind>,
    pub(crate) top_bar_quick_actions_open: bool,
    pub(crate) top_bar_quick_actions_submenu: Option<QuickActionSubmenu>,
    pub(crate) ide_launchers: Vec<ExternalLauncher>,
    pub(crate) last_persisted_ui_state: ui_state_store::UiState,
    pub(crate) pending_ui_state_save: Option<ui_state_store::UiState>,
    pub(crate) ui_state_save_in_flight: Option<ui_state_store::UiState>,
    pub(crate) last_persisted_issue_cache: issue_cache_store::IssueCache,
    pub(crate) pending_issue_cache_save: Option<issue_cache_store::IssueCache>,
    pub(crate) issue_cache_save_in_flight: Option<issue_cache_store::IssueCache>,
    pub(crate) daemon_session_store_save: PendingSave<Vec<DaemonSessionRecord>>,
    pub(crate) daemon_session_store_dirty: bool,
    pub(crate) last_ui_state_error: Option<String>,
    pub(crate) last_issue_cache_error: Option<String>,
    pub(crate) notification_service: Box<dyn notifications::NotificationService>,
    pub(crate) notifications_enabled: bool,
    pub(crate) agent_activity_sessions: HashMap<String, AgentActivitySessionRecord>,
    pub(crate) last_agent_finished_notifications: HashMap<PathBuf, u64>,
    pub(crate) auto_checkpoint_in_flight: Arc<Mutex<HashSet<PathBuf>>>,
    pub(crate) agent_activity_epochs: Arc<Mutex<HashMap<PathBuf, u64>>>,
    pub(crate) window_is_active: bool,
    pub(crate) last_window_geometry: Option<ui_state_store::WindowGeometry>,
    pub(crate) notice: Option<String>,
    pub(crate) theme_toast: Option<String>,
    pub(crate) theme_toast_generation: u64,
    pub(crate) right_pane_tab: RightPaneTab,
    pub(crate) right_pane_search: String,
    pub(crate) right_pane_search_cursor: usize,
    pub(crate) right_pane_search_active: bool,
    pub(crate) sidebar_order: HashMap<String, Vec<SidebarItemId>>,
    pub(crate) repository_sidebar_tabs: HashMap<String, RepositorySidebarTab>,
    pub(crate) issue_lists: HashMap<IssueTarget, IssueListState>,
    pub(crate) worktree_notes_lines: Vec<String>,
    pub(crate) worktree_notes_cursor: FileViewCursor,
    pub(crate) worktree_notes_path: Option<PathBuf>,
    pub(crate) worktree_notes_active: bool,
    pub(crate) worktree_notes_error: Option<String>,
    pub(crate) worktree_notes_save_pending: bool,
    pub(crate) worktree_notes_edit_generation: u64,
    pub(crate) _worktree_notes_save_task: Option<gpui::Task<()>>,
    pub(crate) file_tree_entries: Vec<FileTreeEntry>,
    pub(crate) file_tree_loading: bool,
    pub(crate) expanded_dirs: HashSet<PathBuf>,
    pub(crate) selected_file_tree_entry: Option<PathBuf>,
    pub(crate) left_pane_visible: bool,
    pub(crate) collapsed_repositories: HashSet<usize>,
    pub(crate) agent_chat_sessions: Vec<NativeAgentChatSession>,
    pub(crate) active_agent_chat_by_worktree: HashMap<PathBuf, u64>,
    #[cfg_attr(not(feature = "agent-chat"), allow(dead_code))]
    pub(crate) next_agent_chat_id: u64,
    pub(crate) agent_chat_scroll_handle: ScrollHandle,
    /// When `Some(local_id)`, the agent selector popup is open for this chat session.
    pub(crate) agent_selector_open_for: Option<u64>,
    /// Search text for filtering models in the agent selector popup.
    pub(crate) agent_selector_search: String,
    /// Cursor position in the agent selector search field.
    pub(crate) agent_selector_search_cursor: usize,
    /// When `Some(local_id)`, the chat mode selector popup is open for this chat session.
    pub(crate) chat_mode_selector_open_for: Option<u64>,
    /// Tracks creation order of center tabs for stable tab bar ordering.
    pub(crate) center_tab_order: Vec<CenterTab>,
    pub(crate) new_tab_menu_position: Option<gpui::Point<Pixels>>,
    pub(crate) repository_context_menu: Option<RepositoryContextMenu>,
    pub(crate) worktree_context_menu: Option<WorktreeContextMenu>,
    pub(crate) worktree_hover_popover: Option<WorktreeHoverPopover>,
    pub(crate) _hover_show_task: Option<gpui::Task<()>>,
    pub(crate) _hover_dismiss_task: Option<gpui::Task<()>>,
    pub(crate) _worktree_refresh_task: Option<gpui::Task<()>>,
    pub(crate) _changed_files_refresh_task: Option<gpui::Task<()>>,
    pub(crate) _config_refresh_task: Option<gpui::Task<()>>,
    pub(crate) _repo_metadata_refresh_task: Option<gpui::Task<()>>,
    pub(crate) _launcher_refresh_task: Option<gpui::Task<()>>,
    pub(crate) _connection_history_save_task: Option<gpui::Task<()>>,
    pub(crate) _repository_entries_save_task: Option<gpui::Task<()>>,
    pub(crate) _daemon_auth_tokens_save_task: Option<gpui::Task<()>>,
    pub(crate) _github_auth_state_save_task: Option<gpui::Task<()>>,
    pub(crate) _ui_state_save_task: Option<gpui::Task<()>>,
    pub(crate) _issue_cache_save_task: Option<gpui::Task<()>>,
    pub(crate) _daemon_session_store_save_task: Option<gpui::Task<()>>,
    pub(crate) _daemon_session_store_debounce_task: Option<gpui::Task<()>>,
    pub(crate) _create_modal_preview_task: Option<gpui::Task<()>>,
    pub(crate) _file_tree_refresh_task: Option<gpui::Task<()>>,
    pub(crate) worktree_refresh_epoch: u64,
    pub(crate) changed_files_refresh_epoch: u64,
    pub(crate) config_refresh_epoch: u64,
    pub(crate) repo_metadata_refresh_epoch: u64,
    pub(crate) launcher_refresh_epoch: u64,
    pub(crate) last_mouse_position: gpui::Point<Pixels>,
    pub(crate) outpost_context_menu: Option<OutpostContextMenu>,
    pub(crate) discovered_daemons: Vec<mdns_browser::DiscoveredDaemon>,
    pub(crate) mdns_browser: Option<Box<dyn mdns_browser::MdnsDiscovery>>,
    pub(crate) active_discovered_daemon: Option<usize>,
    pub(crate) worktree_nav_back: Vec<usize>,
    pub(crate) worktree_nav_forward: Vec<usize>,
    pub(crate) log_buffer: log_layer::LogBuffer,
    pub(crate) log_entries: Vec<log_layer::LogEntry>,
    pub(crate) log_generation: u64,
    pub(crate) log_scroll_handle: ScrollHandle,
    pub(crate) log_auto_scroll: bool,
    pub(crate) logs_tab_open: bool,
    pub(crate) logs_tab_active: bool,
    pub(crate) quit_overlay_until: Option<Instant>,
    pub(crate) quit_after_persistence_flush: bool,
    pub(crate) ime_marked_text: Option<String>,
    pub(crate) welcome_clone_url: String,
    pub(crate) welcome_clone_url_cursor: usize,
    pub(crate) welcome_clone_url_active: bool,
    pub(crate) welcome_cloning: bool,
    pub(crate) welcome_clone_error: Option<String>,
    /// Remote daemons that have been expanded in the sidebar.
    pub(crate) remote_daemon_states: HashMap<usize, RemoteDaemonState>,
    /// Currently selected remote worktree (if any). The window stays connected
    /// to the local daemon; only terminal sessions use the remote client.
    pub(crate) active_remote_worktree: Option<ActiveRemoteWorktree>,
    /// When `Some`, a newer version of Arbor is available on GitHub.
    pub(crate) update_available: Option<String>,
    /// Current process CPU usage percentage, updated periodically.
    pub(crate) self_cpu_percent: Option<u16>,
    /// Current process RSS memory usage in bytes, updated periodically.
    pub(crate) self_memory_bytes: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct RemoteDaemonState {
    pub(crate) client: terminal_daemon_http::SharedTerminalDaemonClient,
    pub(crate) hostname: String,
    pub(crate) repositories: Vec<terminal_daemon_http::RemoteRepositoryDto>,
    pub(crate) worktrees: Vec<terminal_daemon_http::RemoteWorktreeDto>,
    pub(crate) loading: bool,
    pub(crate) expanded: bool,
    pub(crate) error: Option<String>,
}

/// Tracks which remote worktree is currently selected in the sidebar,
/// without switching the window's primary daemon connection.
#[derive(Debug, Clone)]
pub(crate) struct ActiveRemoteWorktree {
    pub(crate) daemon_index: usize,
    pub(crate) worktree_path: PathBuf,
    pub(crate) repo_root: String,
}
