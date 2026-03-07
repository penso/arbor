mod app_config;
mod log_layer;
mod notifications;
mod repository_store;
mod simple_http_client;
mod terminal_backend;
mod terminal_daemon_http;
mod terminal_keys;
mod theme;
mod ui_state_store;

use {
    arbor_core::{
        agent::AgentState,
        changes::{self, ChangeKind, ChangedFile},
        daemon::{
            self, CreateOrAttachRequest, DaemonSessionRecord, DetachRequest, KillRequest,
            ResizeRequest, SignalRequest, SnapshotRequest, TerminalSessionState, TerminalSignal,
            WriteRequest,
        },
        worktree,
    },
    gix_diff::blob::v2::{
        Algorithm as DiffAlgorithm, Diff as BlobDiff, InternedInput as BlobInternedInput,
    },
    gpui::{
        App, Application, Bounds, ClipboardItem, Context, Div, DragMoveEvent, ElementId,
        FocusHandle, FontFallbacks, FontFeatures, FontWeight, KeyBinding, KeyDownEvent, Keystroke,
        Menu, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
        PathPromptOptions, ScrollHandle, ScrollStrategy, Stateful, SystemMenuType, TextRun,
        TitlebarOptions, UniformListScrollHandle, Window, WindowBounds, WindowControlArea,
        WindowDecorations, WindowOptions, actions, canvas, div, fill, font, img, point, prelude::*,
        px, rgb, size, uniform_list,
    },
    ropey::Rope,
    std::{
        collections::{HashMap, HashSet},
        env, fs,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::{Arc, Mutex},
        time::{Duration, Instant, SystemTime},
    },
    terminal_backend::{
        EMBEDDED_TERMINAL_DEFAULT_BG, EMBEDDED_TERMINAL_DEFAULT_FG, EmbeddedTerminal,
        TerminalBackendKind, TerminalCursor, TerminalLaunch, TerminalStyledCell,
        TerminalStyledLine, TerminalStyledRun,
    },
    terminal_daemon_http::HttpTerminalDaemon,
    theme::{ThemeKind, ThemePalette},
};

const FONT_UI: &str = ".ZedSans";
const FONT_MONO: &str = "CaskaydiaMono Nerd Font Mono";
const TERMINAL_FONT_FAMILIES: [&str; 6] = [
    FONT_MONO,
    ".ZedMono",
    "SF Mono",
    "Menlo",
    "Monaco",
    "Courier New",
];
const TERMINAL_CELL_WIDTH_PX: f32 = 9.0;
const TERMINAL_CELL_HEIGHT_PX: f32 = 19.0;
const TERMINAL_FONT_SIZE_PX: f32 = 15.0;
const TERMINAL_SCROLLBAR_WIDTH_PX: f32 = 12.0;

const TITLEBAR_HEIGHT: f32 = 34.;
const QUIT_ARM_WINDOW: Duration = Duration::from_millis(1200);
const WORKTREE_AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
const GITHUB_PR_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const CONFIG_AUTO_REFRESH_INTERVAL: Duration = Duration::from_millis(600);
const TERMINAL_TAB_COMMAND_MAX_CHARS: usize = 28;
const DEFAULT_DAEMON_BASE_URL: &str = "http://127.0.0.1:8787";
const DEFAULT_LEFT_PANE_WIDTH: f32 = 290.;
const DEFAULT_RIGHT_PANE_WIDTH: f32 = 340.;
const LEFT_PANE_MIN_WIDTH: f32 = 220.;
const LEFT_PANE_MAX_WIDTH: f32 = 520.;
const RIGHT_PANE_MIN_WIDTH: f32 = 240.;
const RIGHT_PANE_MAX_WIDTH: f32 = 560.;
const PANE_RESIZE_HANDLE_WIDTH: f32 = 8.;
const PANE_CENTER_MIN_WIDTH: f32 = 360.;
const DIFF_ROW_HEIGHT_PX: f32 = 19.;
const DIFF_LINE_NUMBER_WIDTH_CHARS: usize = 5;
const DIFF_ZONEMAP_WIDTH_PX: f32 = 14.;
const DIFF_ZONEMAP_MARGIN_PX: f32 = 4.;
const DIFF_ZONEMAP_MARKER_HEIGHT_PX: f32 = 2.;
const DIFF_ZONEMAP_MIN_THUMB_HEIGHT_PX: f32 = 12.;
const DIFF_FONT_SIZE_PX: f32 = 12.0;
const DIFF_HUNK_CONTEXT_LINES: usize = 3;
const TAB_ICON_TERMINAL: &str = "\u{f489}";
const TAB_ICON_DIFF: &str = "\u{f440}";
const TAB_ICON_LOGS: &str = "\u{f4ed}";
const GIT_ACTION_ICON_COMMIT: &str = "\u{f417}";
const GIT_ACTION_ICON_PUSH: &str = "\u{f093}";
const GIT_ACTION_ICON_PR: &str = "\u{f126}";
const LOG_POLLER_INTERVAL: Duration = Duration::from_millis(200);

fn terminal_mono_font(cx: &App) -> gpui::Font {
    let fallbacks = FontFallbacks::from_fonts(
        TERMINAL_FONT_FAMILIES
            .iter()
            .map(|family| (*family).to_owned())
            .collect::<Vec<_>>(),
    );

    for family in TERMINAL_FONT_FAMILIES {
        let mut candidate = font(family);
        candidate.features = FontFeatures::disable_ligatures();
        candidate.fallbacks = Some(fallbacks.clone());
        let font_id = cx.text_system().resolve_font(&candidate);
        let resolved_family = cx
            .text_system()
            .get_font_for_id(font_id)
            .map(|font| font.family.to_string())
            .unwrap_or_default();
        if resolved_family == family {
            return candidate;
        }
    }

    let mut fallback = font("Menlo");
    fallback.features = FontFeatures::disable_ligatures();
    fallback.fallbacks = Some(fallbacks);
    fallback
}

actions!(arbor, [
    RequestQuit,
    ImmediateQuit,
    NewWindow,
    SpawnTerminal,
    CloseActiveTerminal,
    RefreshWorktrees,
    RefreshChanges,
    OpenAddRepository,
    OpenCreateWorktree,
    UseOneDarkTheme,
    UseAyuDarkTheme,
    UseGruvboxTheme,
    UseEmbeddedBackend,
    UseAlacrittyBackend,
    UseGhosttyBackend,
    ToggleLeftPane,
    NavigateWorktreeBack,
    NavigateWorktreeForward,
    CollapseAllRepositories,
    ViewLogs
]);

#[derive(Debug, Clone)]
struct WorktreeSummary {
    repo_root: PathBuf,
    path: PathBuf,
    label: String,
    branch: String,
    is_primary_checkout: bool,
    pr_number: Option<u64>,
    pr_url: Option<String>,
    diff_summary: Option<changes::DiffLineSummary>,
    agent_state: Option<AgentState>,
}

#[derive(Debug, Clone)]
struct RepositorySummary {
    root: PathBuf,
    label: String,
    avatar_url: Option<String>,
    github_repo_slug: Option<String>,
}

#[derive(Clone)]
struct TerminalSession {
    id: u64,
    daemon_session_id: String,
    worktree_path: PathBuf,
    title: String,
    last_command: Option<String>,
    pending_command: String,
    command: String,
    state: TerminalState,
    exit_code: Option<i32>,
    updated_at_unix_ms: Option<u64>,
    cols: u16,
    rows: u16,
    generation: u64,
    output: String,
    styled_output: Vec<TerminalStyledLine>,
    cursor: Option<TerminalCursor>,
    runtime: Option<TerminalRuntime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalState {
    Running,
    Completed,
    Failed,
}

#[derive(Clone)]
enum OutpostTerminalRuntime {
    #[allow(dead_code)]
    RemoteDaemon {
        daemon: HttpTerminalDaemon,
    },
    SshShell(SshTerminalShell),
    MoshShell(arbor_mosh::MoshShell),
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
        let (output, styled_lines, cursor) = match self.emulator.lock() {
            Ok(emulator) => (
                emulator.snapshot_output(),
                emulator.collect_styled_lines(),
                emulator.snapshot_cursor(),
            ),
            Err(poisoned) => {
                let emulator = poisoned.into_inner();
                (
                    emulator.snapshot_output(),
                    emulator.collect_styled_lines(),
                    emulator.snapshot_cursor(),
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

#[derive(Clone)]
enum TerminalRuntime {
    Embedded(EmbeddedTerminal),
    Daemon,
    Outpost(OutpostTerminalRuntime),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CenterTab {
    Terminal(u64),
    Diff(u64),
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightPaneTab {
    Changes,
    FileTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitActionKind {
    Commit,
    Push,
    CreatePullRequest,
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
    RemoteOutpost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateOutpostField {
    HostSelector,
    CloneUrl,
    Branch,
    OutpostName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateWorktreeField {
    RepositoryPath,
    WorktreeName,
}

#[derive(Debug, Clone)]
struct CreateModal {
    tab: CreateModalTab,
    // Worktree fields
    repository_path: String,
    worktree_name: String,
    worktree_active_field: CreateWorktreeField,
    // Outpost fields
    host_index: usize,
    clone_url: String,
    branch: String,
    outpost_name: String,
    outpost_active_field: CreateOutpostField,
    // Shared
    is_creating: bool,
    error: Option<String>,
}

enum ModalInputEvent {
    SetActiveField(CreateWorktreeField),
    MoveActiveField,
    Backspace,
    Append(String),
    ClearError,
}

enum OutpostModalInputEvent {
    SetActiveField(CreateOutpostField),
    MoveActiveField(bool),
    CycleHost(bool),
    Backspace,
    Append(String),
    ClearError,
}

#[derive(Clone)]
struct ManageHostsModal {
    adding: bool,
    name: String,
    hostname: String,
    user: String,
    active_field: ManageHostsField,
    error: Option<String>,
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
    Backspace,
    Append(String),
    ClearError,
}

struct CreatedWorktree {
    worktree_name: String,
    branch_name: String,
    worktree_path: PathBuf,
}

struct ArborWindow {
    repository_store: Box<dyn repository_store::RepositoryStore>,
    daemon_session_store: Box<dyn daemon::DaemonSessionStore>,
    terminal_daemon: Option<HttpTerminalDaemon>,
    daemon_base_url: String,
    ui_state_store: Box<dyn ui_state_store::UiStateStore>,
    config_path: PathBuf,
    config_last_modified: Option<SystemTime>,
    repositories: Vec<RepositorySummary>,
    active_repository_index: Option<usize>,
    repo_root: PathBuf,
    github_repo_slug: Option<String>,
    worktrees: Vec<WorktreeSummary>,
    worktree_stats_loading: bool,
    worktree_prs_loading: bool,
    active_worktree_index: Option<usize>,
    changed_files: Vec<ChangedFile>,
    selected_changed_file: Option<PathBuf>,
    terminals: Vec<TerminalSession>,
    diff_sessions: Vec<DiffSession>,
    active_diff_session_id: Option<u64>,
    active_terminal_by_worktree: HashMap<PathBuf, u64>,
    next_terminal_id: u64,
    next_diff_session_id: u64,
    active_backend_kind: TerminalBackendKind,
    theme_kind: ThemeKind,
    left_pane_width: f32,
    right_pane_width: f32,
    terminal_focus: FocusHandle,
    terminal_scroll_handle: ScrollHandle,
    diff_scroll_handle: UniformListScrollHandle,
    terminal_selection: Option<TerminalSelection>,
    terminal_selection_drag_anchor: Option<TerminalGridPosition>,
    create_modal: Option<CreateModal>,
    outposts: Vec<OutpostSummary>,
    outpost_store: Box<dyn arbor_core::outpost_store::OutpostStore>,
    active_outpost_index: Option<usize>,
    remote_hosts: Vec<arbor_core::outpost::RemoteHost>,
    ssh_connection_pool: Arc<arbor_ssh::connection::SshConnectionPool>,
    manage_hosts_modal: Option<ManageHostsModal>,
    pending_diff_scroll_to_file: Option<PathBuf>,
    focus_terminal_on_next_render: bool,
    git_action_in_flight: Option<GitActionKind>,
    last_persisted_ui_state: ui_state_store::UiState,
    last_ui_state_error: Option<String>,
    notifications_enabled: bool,
    window_is_active: bool,
    notice: Option<String>,
    right_pane_tab: RightPaneTab,
    file_tree_entries: Vec<FileTreeEntry>,
    expanded_dirs: HashSet<PathBuf>,
    selected_file_tree_entry: Option<PathBuf>,
    left_pane_visible: bool,
    collapsed_repositories: HashSet<usize>,
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
    agent_ws_connected: bool,
}

impl ArborWindow {
    fn load_with_daemon_store<S>(
        startup_ui_state: ui_state_store::UiState,
        log_buffer: log_layer::LogBuffer,
        cx: &mut Context<Self>,
    ) -> Self
    where
        S: daemon::DaemonSessionStore + Default + 'static,
    {
        Self::load(Box::new(S::default()), startup_ui_state, log_buffer, cx)
    }

    fn load(
        daemon_session_store: Box<dyn daemon::DaemonSessionStore>,
        startup_ui_state: ui_state_store::UiState,
        log_buffer: log_layer::LogBuffer,
        cx: &mut Context<Self>,
    ) -> Self {
        let repository_store = repository_store::default_repository_store();
        let ui_state_store = ui_state_store::default_ui_state_store();
        let config_path = app_config::config_path();
        let cwd = match env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                return Self {
                    repository_store,
                    daemon_session_store,
                    terminal_daemon: None,
                    daemon_base_url: DEFAULT_DAEMON_BASE_URL.to_owned(),
                    ui_state_store,
                    config_path: config_path.clone(),
                    config_last_modified: app_config::config_last_modified(&config_path),
                    repositories: Vec::new(),
                    active_repository_index: None,
                    repo_root: PathBuf::from("."),
                    github_repo_slug: None,
                    worktrees: Vec::new(),
                    worktree_stats_loading: false,
                    worktree_prs_loading: false,
                    active_worktree_index: None,
                    changed_files: Vec::new(),
                    selected_changed_file: None,
                    terminals: Vec::new(),
                    diff_sessions: Vec::new(),
                    active_diff_session_id: None,
                    active_terminal_by_worktree: HashMap::new(),
                    next_terminal_id: 1,
                    next_diff_session_id: 1,
                    active_backend_kind: TerminalBackendKind::Embedded,
                    theme_kind: ThemeKind::One,
                    left_pane_width: startup_ui_state
                        .left_pane_width
                        .map_or(DEFAULT_LEFT_PANE_WIDTH, |width| width as f32),
                    right_pane_width: startup_ui_state
                        .right_pane_width
                        .map_or(DEFAULT_RIGHT_PANE_WIDTH, |width| width as f32),
                    terminal_focus: cx.focus_handle(),
                    terminal_scroll_handle: ScrollHandle::new(),
                    diff_scroll_handle: UniformListScrollHandle::new(),
                    terminal_selection: None,
                    terminal_selection_drag_anchor: None,
                    create_modal: None,
                    outposts: Vec::new(),
                    outpost_store: Box::new(arbor_core::outpost_store::default_outpost_store()),
                    active_outpost_index: None,
                    remote_hosts: Vec::new(),
                    ssh_connection_pool: Arc::new(arbor_ssh::connection::SshConnectionPool::new()),
                    manage_hosts_modal: None,
                    pending_diff_scroll_to_file: None,
                    focus_terminal_on_next_render: true,
                    git_action_in_flight: None,
                    last_persisted_ui_state: startup_ui_state,
                    last_ui_state_error: None,
                    notifications_enabled: true,
                    window_is_active: true,
                    notice: Some(format!("failed to read current directory: {error}")),
                    right_pane_tab: RightPaneTab::Changes,
                    file_tree_entries: Vec::new(),
                    expanded_dirs: HashSet::new(),
                    selected_file_tree_entry: None,
                    left_pane_visible: true,
                    collapsed_repositories: HashSet::new(),
                    worktree_nav_back: Vec::new(),
                    worktree_nav_forward: Vec::new(),
                    log_buffer: log_buffer.clone(),
                    log_entries: Vec::new(),
                    log_generation: 0,
                    log_scroll_handle: ScrollHandle::new(),
                    log_auto_scroll: true,
                    logs_tab_open: false,
                    logs_tab_active: false,
                    quit_overlay_until: None,
                    agent_ws_connected: false,
                };
            },
        };

        let repo_root = match worktree::repo_root(&cwd) {
            Ok(path) => path,
            Err(error) => {
                let github_repo_slug = github_repo_slug_for_repo(&cwd);
                return Self {
                    repository_store,
                    daemon_session_store,
                    terminal_daemon: None,
                    daemon_base_url: DEFAULT_DAEMON_BASE_URL.to_owned(),
                    ui_state_store,
                    config_path: config_path.clone(),
                    config_last_modified: app_config::config_last_modified(&config_path),
                    repositories: Vec::new(),
                    active_repository_index: None,
                    repo_root: cwd,
                    github_repo_slug,
                    worktrees: Vec::new(),
                    worktree_stats_loading: false,
                    worktree_prs_loading: false,
                    active_worktree_index: None,
                    changed_files: Vec::new(),
                    selected_changed_file: None,
                    terminals: Vec::new(),
                    diff_sessions: Vec::new(),
                    active_diff_session_id: None,
                    active_terminal_by_worktree: HashMap::new(),
                    next_terminal_id: 1,
                    next_diff_session_id: 1,
                    active_backend_kind: TerminalBackendKind::Embedded,
                    theme_kind: ThemeKind::One,
                    left_pane_width: startup_ui_state
                        .left_pane_width
                        .map_or(DEFAULT_LEFT_PANE_WIDTH, |width| width as f32),
                    right_pane_width: startup_ui_state
                        .right_pane_width
                        .map_or(DEFAULT_RIGHT_PANE_WIDTH, |width| width as f32),
                    terminal_focus: cx.focus_handle(),
                    terminal_scroll_handle: ScrollHandle::new(),
                    diff_scroll_handle: UniformListScrollHandle::new(),
                    terminal_selection: None,
                    terminal_selection_drag_anchor: None,
                    create_modal: None,
                    outposts: Vec::new(),
                    outpost_store: Box::new(arbor_core::outpost_store::default_outpost_store()),
                    active_outpost_index: None,
                    remote_hosts: Vec::new(),
                    ssh_connection_pool: Arc::new(arbor_ssh::connection::SshConnectionPool::new()),
                    manage_hosts_modal: None,
                    pending_diff_scroll_to_file: None,
                    focus_terminal_on_next_render: true,
                    git_action_in_flight: None,
                    last_persisted_ui_state: startup_ui_state,
                    last_ui_state_error: None,
                    notifications_enabled: true,
                    window_is_active: true,
                    notice: Some(format!("failed to resolve git repository root: {error}")),
                    right_pane_tab: RightPaneTab::Changes,
                    file_tree_entries: Vec::new(),
                    expanded_dirs: HashSet::new(),
                    selected_file_tree_entry: None,
                    left_pane_visible: true,
                    collapsed_repositories: HashSet::new(),
                    worktree_nav_back: Vec::new(),
                    worktree_nav_forward: Vec::new(),
                    log_buffer: log_buffer.clone(),
                    log_entries: Vec::new(),
                    log_generation: 0,
                    log_scroll_handle: ScrollHandle::new(),
                    log_auto_scroll: true,
                    logs_tab_open: false,
                    logs_tab_active: false,
                    quit_overlay_until: None,
                    agent_ws_connected: false,
                };
            },
        };

        tracing::info!(config = %config_path.display(), "loading configuration");
        let loaded_config = app_config::load_or_create_config();
        let mut notice_parts = loaded_config.notices;
        let config_last_modified = app_config::config_last_modified(&config_path);

        if let Err(error) = daemon_session_store.load() {
            tracing::warn!(%error, "failed to load daemon session metadata");
            notice_parts.push(format!("failed to load daemon session metadata: {error}"));
        }
        let daemon_base_url =
            daemon_base_url_from_config(loaded_config.config.daemon_url.as_deref());
        tracing::info!(url = %daemon_base_url, "connecting to terminal daemon");
        let mut terminal_daemon = match HttpTerminalDaemon::new(&daemon_base_url) {
            Ok(client) => Some(client),
            Err(error) => {
                tracing::error!(%error, url = %daemon_base_url, "invalid daemon URL");
                notice_parts.push(format!("invalid daemon_url `{daemon_base_url}`: {error}"));
                None
            },
        };
        let (initial_daemon_records, attach_daemon_runtime) =
            if let Some(daemon) = terminal_daemon.as_ref() {
                match daemon.list_sessions() {
                    Ok(records) => (records, true),
                    Err(error) => {
                        let error_text = error.to_string();
                        if daemon_error_is_connection_refused(&error_text) {
                            tracing::debug!("daemon not running (connection refused)");
                            terminal_daemon = None;
                            (Vec::new(), false)
                        } else {
                            notice_parts.push(format!(
                                "failed to list terminal sessions from daemon at {}: {error}",
                                daemon.base_url()
                            ));
                            (Vec::new(), false)
                        }
                    },
                }
            } else {
                (Vec::new(), false)
            };

        let mut repositories = match repository_store.load_roots() {
            Ok(roots) => repository_store::resolve_repositories_from_roots(roots),
            Err(error) => {
                notice_parts.push(format!("failed to load saved repositories: {error}"));
                Vec::new()
            },
        };
        let mut persist_repositories = false;

        if repositories.is_empty()
            || !repositories
                .iter()
                .any(|repository| repository.root == repo_root)
        {
            repositories.push(RepositorySummary::from_root(repo_root.clone()));
            persist_repositories = true;
        }

        let active_repository_index = repositories
            .iter()
            .position(|repository| repository.root == repo_root)
            .or(Some(0));
        let active_repository = active_repository_index
            .and_then(|index| repositories.get(index))
            .cloned();

        if persist_repositories {
            let roots_to_save = repository_store::repository_roots_from_summaries(&repositories);
            if let Err(error) = repository_store.save_roots(&roots_to_save) {
                notice_parts.push(format!("failed to save repositories: {error}"));
            }
        }

        let remote_hosts: Vec<arbor_core::outpost::RemoteHost> = loaded_config
            .config
            .remote_hosts
            .iter()
            .map(|host_config| arbor_core::outpost::RemoteHost {
                name: host_config.name.clone(),
                hostname: host_config.hostname.clone(),
                port: host_config.port,
                user: host_config.user.clone(),
                identity_file: host_config.identity_file.clone(),
                remote_base_path: host_config.remote_base_path.clone(),
                daemon_port: host_config.daemon_port,
                mosh: host_config.mosh,
                mosh_server_path: host_config.mosh_server_path.clone(),
            })
            .collect();

        let outpost_store = Box::new(arbor_core::outpost_store::default_outpost_store());
        let outposts = load_outpost_summaries(outpost_store.as_ref(), &remote_hosts);

        let active_backend_kind =
            match parse_terminal_backend_kind(loaded_config.config.terminal_backend.as_deref()) {
                Ok(kind) => kind,
                Err(error) => {
                    notice_parts.push(error);
                    TerminalBackendKind::Embedded
                },
            };
        let theme_kind = match parse_theme_kind(loaded_config.config.theme.as_deref()) {
            Ok(kind) => kind,
            Err(error) => {
                notice_parts.push(error);
                ThemeKind::One
            },
        };
        let notifications_enabled = loaded_config.config.notifications.unwrap_or(true);

        let mut app = Self {
            repository_store,
            daemon_session_store,
            terminal_daemon,
            daemon_base_url,
            ui_state_store,
            config_path,
            config_last_modified,
            repositories,
            active_repository_index,
            repo_root: active_repository
                .as_ref()
                .map(|repository| repository.root.clone())
                .unwrap_or(repo_root),
            github_repo_slug: active_repository.and_then(|repository| repository.github_repo_slug),
            worktrees: Vec::new(),
            worktree_stats_loading: false,
            worktree_prs_loading: false,
            active_worktree_index: None,
            changed_files: Vec::new(),
            selected_changed_file: None,
            terminals: Vec::new(),
            diff_sessions: Vec::new(),
            active_diff_session_id: None,
            active_terminal_by_worktree: HashMap::new(),
            next_terminal_id: 1,
            next_diff_session_id: 1,
            active_backend_kind,
            theme_kind,
            left_pane_width: startup_ui_state
                .left_pane_width
                .map_or(DEFAULT_LEFT_PANE_WIDTH, |width| width as f32),
            right_pane_width: startup_ui_state
                .right_pane_width
                .map_or(DEFAULT_RIGHT_PANE_WIDTH, |width| width as f32),
            terminal_focus: cx.focus_handle(),
            terminal_scroll_handle: ScrollHandle::new(),
            diff_scroll_handle: UniformListScrollHandle::new(),
            terminal_selection: None,
            terminal_selection_drag_anchor: None,
            create_modal: None,
            outposts,
            outpost_store,
            active_outpost_index: None,
            remote_hosts,
            ssh_connection_pool: Arc::new(arbor_ssh::connection::SshConnectionPool::new()),
            manage_hosts_modal: None,
            pending_diff_scroll_to_file: None,
            focus_terminal_on_next_render: true,
            git_action_in_flight: None,
            left_pane_visible: startup_ui_state.left_pane_visible.unwrap_or(true),
            collapsed_repositories: HashSet::new(),
            worktree_nav_back: Vec::new(),
            worktree_nav_forward: Vec::new(),
            last_persisted_ui_state: startup_ui_state,
            last_ui_state_error: None,
            notifications_enabled,
            window_is_active: true,
            notice: (!notice_parts.is_empty()).then_some(notice_parts.join(" | ")),
            right_pane_tab: RightPaneTab::Changes,
            file_tree_entries: Vec::new(),
            expanded_dirs: HashSet::new(),
            selected_file_tree_entry: None,
            log_buffer,
            log_entries: Vec::new(),
            log_generation: 0,
            log_scroll_handle: ScrollHandle::new(),
            log_auto_scroll: true,
            logs_tab_open: false,
            logs_tab_active: false,
            quit_overlay_until: None,
            agent_ws_connected: false,
        };

        app.refresh_worktrees(cx);
        app.restore_terminal_sessions_from_records(initial_daemon_records, attach_daemon_runtime);
        let _ = app.ensure_selected_worktree_terminal();
        app.sync_daemon_session_store(cx);
        app.start_terminal_poller(cx);
        app.start_log_poller(cx);
        app.start_worktree_auto_refresh(cx);
        app.start_github_pr_auto_refresh(cx);
        app.start_config_auto_refresh(cx);
        app.start_agent_activity_ws(cx);
        app.ensure_claude_code_hooks(cx);
        app
    }

    fn start_terminal_poller(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_millis(45));
                })
                .await;

                let updated = this.update(cx, |this, cx| this.sync_running_terminals(cx));
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_log_poller(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(LOG_POLLER_INTERVAL);
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    let current_generation = this.log_buffer.generation();
                    if current_generation == this.log_generation {
                        return;
                    }
                    this.log_generation = current_generation;
                    this.log_entries = this.log_buffer.snapshot();
                    if this.log_auto_scroll && this.logs_tab_active {
                        this.log_scroll_handle.scroll_to_bottom();
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_worktree_auto_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(WORKTREE_AUTO_REFRESH_INTERVAL);
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    if this.worktree_stats_loading {
                        return;
                    }

                    this.refresh_worktree_diff_summaries(cx);
                    this.refresh_agent_activity(cx);
                    if this.active_outpost_index.is_some() {
                        this.refresh_remote_changed_files(cx);
                    } else if this.reload_changed_files() {
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_github_pr_auto_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(GITHUB_PR_REFRESH_INTERVAL);
                })
                .await;

                let updated = this.update(cx, |this, cx| this.refresh_worktree_pull_requests(cx));
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_config_auto_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(CONFIG_AUTO_REFRESH_INTERVAL);
                })
                .await;

                let updated = this.update(cx, |this, cx| this.refresh_config_if_changed(cx));
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn refresh_config_if_changed(&mut self, cx: &mut Context<Self>) {
        let next_modified = app_config::config_last_modified(&self.config_path);
        if self.config_last_modified == next_modified {
            return;
        }
        self.config_last_modified = next_modified;

        let loaded = app_config::load_or_create_config();
        let mut notices = loaded.notices;
        let mut changed = false;

        match parse_theme_kind(loaded.config.theme.as_deref()) {
            Ok(theme_kind) => {
                if self.theme_kind != theme_kind {
                    self.theme_kind = theme_kind;
                    changed = true;
                }
            },
            Err(error) => notices.push(error),
        }

        match parse_terminal_backend_kind(loaded.config.terminal_backend.as_deref()) {
            Ok(backend_kind) => {
                if self.active_backend_kind != backend_kind {
                    self.active_backend_kind = backend_kind;
                    changed = true;
                }
            },
            Err(error) => notices.push(error),
        }

        let next_daemon_base_url = daemon_base_url_from_config(loaded.config.daemon_url.as_deref());
        if self.daemon_base_url != next_daemon_base_url {
            self.daemon_base_url = next_daemon_base_url.clone();
            self.terminal_daemon = match HttpTerminalDaemon::new(&next_daemon_base_url) {
                Ok(client) => Some(client),
                Err(error) => {
                    notices.push(format!(
                        "invalid daemon_url `{next_daemon_base_url}`: {error}"
                    ));
                    None
                },
            };
            changed = true;
        }

        if let Some(daemon) = self.terminal_daemon.as_ref() {
            match daemon.list_sessions() {
                Ok(records) => {
                    changed |= self.restore_terminal_sessions_from_records(records, true);
                },
                Err(error) => {
                    let error_text = error.to_string();
                    if daemon_error_is_connection_refused(&error_text) {
                        self.terminal_daemon = None;
                        changed = true;
                    } else {
                        notices.push(format!(
                            "failed to list terminal sessions from daemon at {}: {error}",
                            daemon.base_url()
                        ));
                    }
                },
            }
        }

        let next_remote_hosts: Vec<arbor_core::outpost::RemoteHost> = loaded
            .config
            .remote_hosts
            .iter()
            .map(|host_config| arbor_core::outpost::RemoteHost {
                name: host_config.name.clone(),
                hostname: host_config.hostname.clone(),
                port: host_config.port,
                user: host_config.user.clone(),
                identity_file: host_config.identity_file.clone(),
                remote_base_path: host_config.remote_base_path.clone(),
                daemon_port: host_config.daemon_port,
                mosh: host_config.mosh,
                mosh_server_path: host_config.mosh_server_path.clone(),
            })
            .collect();
        if self.remote_hosts != next_remote_hosts {
            self.remote_hosts = next_remote_hosts;
            self.outposts = load_outpost_summaries(self.outpost_store.as_ref(), &self.remote_hosts);
            changed = true;
        }

        self.notifications_enabled = loaded.config.notifications.unwrap_or(true);

        if !notices.is_empty() {
            self.notice = Some(notices.join(" | "));
            changed = true;
        }

        if changed {
            cx.notify();
        }
    }

    fn sync_daemon_session_store(&mut self, cx: &mut Context<Self>) {
        let shell = match env::var("SHELL") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => "/bin/zsh".to_owned(),
        };
        let updated_at_unix_ms = current_unix_timestamp_millis();

        let records: Vec<DaemonSessionRecord> = self
            .terminals
            .iter()
            .map(|session| DaemonSessionRecord {
                session_id: session.daemon_session_id.clone(),
                workspace_id: session.worktree_path.display().to_string(),
                cwd: session.worktree_path.clone(),
                shell: if session.command.trim().is_empty() {
                    shell.clone()
                } else {
                    session.command.clone()
                },
                cols: session.cols.max(2),
                rows: session.rows.max(1),
                title: Some(session.title.clone()),
                last_command: session.last_command.clone(),
                output_tail: Some(terminal_output_tail_for_metadata(session, 64, 24_000)),
                exit_code: session.exit_code,
                state: Some(daemon_state_from_terminal_state(session.state)),
                updated_at_unix_ms: session.updated_at_unix_ms.or(updated_at_unix_ms),
            })
            .collect();

        if let Err(error) = self.daemon_session_store.save(&records) {
            self.notice = Some(format!("failed to persist daemon sessions: {error}"));
            cx.notify();
        }
    }

    fn restore_terminal_sessions_from_records(
        &mut self,
        mut records: Vec<DaemonSessionRecord>,
        attach_runtime: bool,
    ) -> bool {
        if records.is_empty() {
            return false;
        }

        records.sort_by(|left, right| {
            right
                .updated_at_unix_ms
                .unwrap_or(0)
                .cmp(&left.updated_at_unix_ms.unwrap_or(0))
                .then_with(|| left.session_id.cmp(&right.session_id))
        });

        let mut changed = false;

        for record in records {
            if record.session_id.trim().is_empty() {
                continue;
            }

            let Some(worktree_path) = self.worktree_path_for_session_record(&record) else {
                continue;
            };
            let session_state = terminal_state_from_daemon_record(&record);
            let title = record
                .title
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("term-{}", self.next_terminal_id));
            let command = record.shell.clone();
            let output = record.output_tail.clone().unwrap_or_default();
            let cols = record.cols.max(2);
            let rows = record.rows.max(1);

            if let Some(session) = self
                .terminals
                .iter_mut()
                .find(|session| session.daemon_session_id == record.session_id)
            {
                if session.worktree_path != worktree_path {
                    session.worktree_path = worktree_path.clone();
                    changed = true;
                }
                if session.title != title {
                    session.title = title.clone();
                    changed = true;
                }
                if session.command != command {
                    session.command = command.clone();
                    changed = true;
                }
                if session.output != output {
                    session.output = output.clone();
                    session.styled_output.clear();
                    session.cursor = None;
                    changed = true;
                }
                if session.state != session_state {
                    session.state = session_state;
                    changed = true;
                }
                if session.exit_code != record.exit_code {
                    session.exit_code = record.exit_code;
                    changed = true;
                }
                if session.updated_at_unix_ms != record.updated_at_unix_ms {
                    session.updated_at_unix_ms = record.updated_at_unix_ms;
                    changed = true;
                }
                if session.cols != cols || session.rows != rows {
                    session.cols = cols;
                    session.rows = rows;
                    changed = true;
                }
                if attach_runtime && session.runtime.is_none() {
                    session.runtime = Some(TerminalRuntime::Daemon);
                    changed = true;
                }
            } else {
                let session_id = self.next_terminal_id;
                self.next_terminal_id += 1;
                self.terminals.push(TerminalSession {
                    id: session_id,
                    daemon_session_id: record.session_id.clone(),
                    worktree_path: worktree_path.clone(),
                    title,
                    last_command: record.last_command.clone(),
                    pending_command: String::new(),
                    command,
                    state: session_state,
                    exit_code: record.exit_code,
                    updated_at_unix_ms: record.updated_at_unix_ms,
                    cols,
                    rows,
                    generation: 0,
                    output,
                    styled_output: Vec::new(),
                    cursor: None,
                    runtime: attach_runtime.then_some(TerminalRuntime::Daemon),
                });
                changed = true;
            }

            let mapped_terminal_id = self
                .terminals
                .iter()
                .find(|session| session.daemon_session_id == record.session_id)
                .map(|session| session.id);
            if let Some(mapped_terminal_id) = mapped_terminal_id {
                self.active_terminal_by_worktree
                    .entry(worktree_path)
                    .or_insert(mapped_terminal_id);
            }
        }

        changed
    }

    fn worktree_path_for_session_record(&self, record: &DaemonSessionRecord) -> Option<PathBuf> {
        if let Some(path) = self.match_worktree_path(record.cwd.as_path()) {
            return Some(path);
        }

        let workspace_path = PathBuf::from(record.workspace_id.clone());
        self.match_worktree_path(workspace_path.as_path())
    }

    fn match_worktree_path(&self, candidate: &Path) -> Option<PathBuf> {
        self.worktrees
            .iter()
            .find(|worktree| paths_equivalent(worktree.path.as_path(), candidate))
            .map(|worktree| worktree.path.clone())
    }

    fn maybe_notify(&self, title: &str, body: &str) {
        if self.notifications_enabled && !self.window_is_active {
            notifications::send(title, body);
        }
    }

    fn sync_running_terminals(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let follow_output = terminal_scroll_is_near_bottom(&self.terminal_scroll_handle);
        let active_terminal_id = self.active_terminal_id_for_selected_worktree();
        let target_grid_size =
            terminal_grid_size_from_scroll_handle(&self.terminal_scroll_handle, cx);
        let daemon = self.terminal_daemon.clone();
        let mut sessions_to_close = Vec::new();
        let mut pending_notifications: Vec<(String, String)> = Vec::new();

        for index in 0..self.terminals.len() {
            let Some(runtime) = self
                .terminals
                .get(index)
                .and_then(|session| session.runtime.clone())
            else {
                continue;
            };

            match runtime {
                TerminalRuntime::Embedded(runtime) => {
                    let session_id = self.terminals[index].id;
                    if active_terminal_id == Some(session_id)
                        && let Some((rows, cols, pixel_width, pixel_height)) = target_grid_size
                        && let Err(error) = runtime.resize(rows, cols, pixel_width, pixel_height)
                    {
                        self.notice = Some(format!("failed to resize terminal: {error}"));
                    }

                    let generation = runtime.generation();
                    if generation == self.terminals[index].generation {
                        continue;
                    }

                    let snapshot = runtime.snapshot();
                    let session = &mut self.terminals[index];
                    let output = snapshot.output;
                    let styled_output = snapshot.styled_lines;
                    let cursor = snapshot.cursor;
                    if output != session.output
                        || styled_output != session.styled_output
                        || cursor != session.cursor
                    {
                        session.output = output;
                        session.styled_output = styled_output;
                        session.cursor = cursor;
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                        changed = true;
                    }
                    session.generation = generation;

                    if let Some(exit_code) = snapshot.exit_code
                        && session.state == TerminalState::Running
                    {
                        session.exit_code = Some(exit_code);
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                        if exit_code == 0 {
                            pending_notifications.push((
                                "Terminal completed".to_owned(),
                                format!("`{}` completed successfully", session.title),
                            ));
                            sessions_to_close.push(session.id);
                        } else {
                            session.state = TerminalState::Failed;
                            session.runtime = None;
                            changed = true;
                            pending_notifications.push((
                                "Terminal failed".to_owned(),
                                format!("`{}` failed with code {exit_code}", session.title),
                            ));
                            self.notice = Some(format!(
                                "terminal tab `{}` exited with code {exit_code}",
                                session.title,
                            ));
                        }
                    }
                },
                TerminalRuntime::Daemon => {
                    let Some(daemon) = daemon.as_ref() else {
                        continue;
                    };

                    let (session_id, daemon_session_id, previous_cols, previous_rows, title) = {
                        let session = &self.terminals[index];
                        (
                            session.id,
                            session.daemon_session_id.clone(),
                            session.cols,
                            session.rows,
                            session.title.clone(),
                        )
                    };

                    if active_terminal_id == Some(session_id)
                        && let Some((rows, cols, ..)) = target_grid_size
                        && (cols != previous_cols || rows != previous_rows)
                    {
                        match daemon.resize(ResizeRequest {
                            session_id: daemon_session_id.clone(),
                            cols,
                            rows,
                        }) {
                            Ok(()) => {
                                let session = &mut self.terminals[index];
                                session.cols = cols;
                                session.rows = rows;
                                changed = true;
                            },
                            Err(error) => {
                                self.notice = Some(format!("failed to resize terminal: {error}"));
                            },
                        }
                    }

                    match daemon.snapshot(SnapshotRequest {
                        session_id: daemon_session_id,
                        max_lines: 220,
                    }) {
                        Ok(Some(snapshot)) => {
                            let session = &mut self.terminals[index];
                            let snapshot_state = terminal_state_from_daemon_state(snapshot.state);
                            if session.output != snapshot.output_tail {
                                session.output = snapshot.output_tail;
                                session.styled_output.clear();
                                session.cursor = None;
                                changed = true;
                            }
                            if session.state != snapshot_state {
                                session.state = snapshot_state;
                                changed = true;
                            }
                            if session.exit_code != snapshot.exit_code {
                                session.exit_code = snapshot.exit_code;
                                changed = true;
                            }
                            if session.updated_at_unix_ms != snapshot.updated_at_unix_ms {
                                session.updated_at_unix_ms = snapshot.updated_at_unix_ms;
                                changed = true;
                            }

                            if let Some(exit_code) = snapshot.exit_code {
                                if exit_code == 0 {
                                    pending_notifications.push((
                                        "Terminal completed".to_owned(),
                                        format!("`{title}` completed successfully"),
                                    ));
                                    sessions_to_close.push(session.id);
                                } else if session.state == TerminalState::Failed {
                                    session.runtime = None;
                                    changed = true;
                                    pending_notifications.push((
                                        "Terminal failed".to_owned(),
                                        format!("`{title}` failed with code {exit_code}"),
                                    ));
                                    self.notice = Some(format!(
                                        "terminal tab `{title}` exited with code {exit_code}",
                                    ));
                                }
                            }
                        },
                        Ok(None) => {
                            sessions_to_close.push(session_id);
                        },
                        Err(error) => {
                            let error_text = error.to_string();
                            if daemon_error_is_connection_refused(&error_text) {
                                self.terminal_daemon = None;
                                let session = &mut self.terminals[index];
                                session.runtime = None;
                                session.state = TerminalState::Failed;
                                changed = true;
                            } else {
                                self.notice = Some(format!(
                                    "failed to load daemon snapshot for terminal `{title}`: {error}"
                                ));
                            }
                        },
                    }
                },
                TerminalRuntime::Outpost(OutpostTerminalRuntime::RemoteDaemon { ref daemon }) => {
                    let (session_id, daemon_session_id, previous_cols, previous_rows, title) = {
                        let session = &self.terminals[index];
                        (
                            session.id,
                            session.daemon_session_id.clone(),
                            session.cols,
                            session.rows,
                            session.title.clone(),
                        )
                    };

                    if active_terminal_id == Some(session_id)
                        && let Some((rows, cols, ..)) = target_grid_size
                        && (cols != previous_cols || rows != previous_rows)
                    {
                        match daemon.resize(ResizeRequest {
                            session_id: daemon_session_id.clone(),
                            cols,
                            rows,
                        }) {
                            Ok(()) => {
                                let session = &mut self.terminals[index];
                                session.cols = cols;
                                session.rows = rows;
                                changed = true;
                            },
                            Err(error) => {
                                self.notice =
                                    Some(format!("failed to resize outpost terminal: {error}"));
                            },
                        }
                    }

                    match daemon.snapshot(SnapshotRequest {
                        session_id: daemon_session_id,
                        max_lines: 220,
                    }) {
                        Ok(Some(snapshot)) => {
                            let session = &mut self.terminals[index];
                            let snapshot_state = terminal_state_from_daemon_state(snapshot.state);
                            if session.output != snapshot.output_tail {
                                session.output = snapshot.output_tail;
                                session.styled_output.clear();
                                session.cursor = None;
                                changed = true;
                            }
                            if session.state != snapshot_state {
                                session.state = snapshot_state;
                                changed = true;
                            }
                            if session.exit_code != snapshot.exit_code {
                                session.exit_code = snapshot.exit_code;
                                changed = true;
                            }
                            if session.updated_at_unix_ms != snapshot.updated_at_unix_ms {
                                session.updated_at_unix_ms = snapshot.updated_at_unix_ms;
                                changed = true;
                            }
                        },
                        Ok(None) => {
                            sessions_to_close.push(session_id);
                        },
                        Err(error) => {
                            self.notice = Some(format!(
                                "failed to load outpost daemon snapshot for terminal `{title}`: {error}"
                            ));
                        },
                    }
                },
                TerminalRuntime::Outpost(OutpostTerminalRuntime::SshShell(ref ssh)) => {
                    let session_id = self.terminals[index].id;
                    if active_terminal_id == Some(session_id)
                        && let Some((rows, cols, _pixel_width, _pixel_height)) = target_grid_size
                        && let Err(error) = ssh.resize(rows, cols)
                    {
                        self.notice = Some(format!("failed to resize SSH terminal: {error}"));
                    }

                    // Poll for new data from the SSH channel
                    ssh.poll();

                    let generation = ssh.generation();
                    if generation == self.terminals[index].generation {
                        continue;
                    }

                    let snapshot = ssh.snapshot();
                    let session = &mut self.terminals[index];
                    let output = snapshot.output;
                    let styled_output = snapshot.styled_lines;
                    let cursor = snapshot.cursor;
                    if output != session.output
                        || styled_output != session.styled_output
                        || cursor != session.cursor
                    {
                        session.output = output;
                        session.styled_output = styled_output;
                        session.cursor = cursor;
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                        changed = true;
                    }
                    session.generation = generation;

                    if let Some(exit_code) = snapshot.exit_code
                        && session.state == TerminalState::Running
                    {
                        session.exit_code = Some(exit_code);
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                        if exit_code == 0 {
                            pending_notifications.push((
                                "SSH terminal completed".to_owned(),
                                format!("`{}` completed successfully", session.title),
                            ));
                            sessions_to_close.push(session.id);
                        } else {
                            session.state = TerminalState::Failed;
                            session.runtime = None;
                            changed = true;
                            pending_notifications.push((
                                "SSH terminal failed".to_owned(),
                                format!("`{}` failed with code {exit_code}", session.title),
                            ));
                            self.notice = Some(format!(
                                "SSH terminal tab `{}` exited with code {exit_code}",
                                session.title,
                            ));
                        }
                    }
                },
                TerminalRuntime::Outpost(OutpostTerminalRuntime::MoshShell(ref mosh)) => {
                    let session_id = self.terminals[index].id;
                    if active_terminal_id == Some(session_id)
                        && let Some((rows, cols, pixel_width, pixel_height)) = target_grid_size
                        && let Err(error) = mosh.resize(rows, cols, pixel_width, pixel_height)
                    {
                        self.notice = Some(format!("failed to resize mosh terminal: {error}"));
                    }

                    let generation = mosh.generation();
                    if generation == self.terminals[index].generation {
                        continue;
                    }

                    let snapshot = mosh.snapshot();
                    let session = &mut self.terminals[index];
                    let output = snapshot.output;
                    let styled_output = snapshot.styled_lines;
                    let cursor = snapshot.cursor;
                    if output != session.output
                        || styled_output != session.styled_output
                        || cursor != session.cursor
                    {
                        session.output = output;
                        session.styled_output = styled_output;
                        session.cursor = cursor;
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                        changed = true;
                    }
                    session.generation = generation;

                    if let Some(exit_code) = snapshot.exit_code
                        && session.state == TerminalState::Running
                    {
                        session.exit_code = Some(exit_code);
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                        if exit_code == 0 {
                            pending_notifications.push((
                                "Mosh terminal completed".to_owned(),
                                format!("`{}` completed successfully", session.title),
                            ));
                            sessions_to_close.push(session.id);
                        } else {
                            session.state = TerminalState::Failed;
                            session.runtime = None;
                            changed = true;
                            pending_notifications.push((
                                "Mosh terminal failed".to_owned(),
                                format!("`{}` failed with code {exit_code}", session.title),
                            ));
                            self.notice = Some(format!(
                                "mosh terminal tab `{}` exited with code {exit_code}",
                                session.title,
                            ));
                        }
                    }
                },
            };
        }

        for (title, body) in pending_notifications {
            self.maybe_notify(&title, &body);
        }

        for session_id in sessions_to_close {
            changed |= self.close_terminal_session_by_id(session_id);
        }

        if changed {
            self.sync_daemon_session_store(cx);
            if should_auto_follow_terminal_output(changed, follow_output) {
                self.terminal_scroll_handle.scroll_to_bottom();
            }
            cx.notify();
        }
    }

    fn refresh_worktrees(&mut self, cx: &mut Context<Self>) {
        tracing::debug!("refreshing worktrees");
        let previously_selected = self.selected_worktree_path().map(Path::to_path_buf);
        let previous_summaries: HashMap<PathBuf, changes::DiffLineSummary> = self
            .worktrees
            .iter()
            .filter_map(|worktree| {
                worktree
                    .diff_summary
                    .map(|summary| (worktree.path.clone(), summary))
            })
            .collect();
        let previous_pr_numbers: HashMap<PathBuf, u64> = self
            .worktrees
            .iter()
            .filter_map(|worktree| {
                worktree
                    .pr_number
                    .map(|pr_number| (worktree.path.clone(), pr_number))
            })
            .collect();
        let previous_pr_urls: HashMap<PathBuf, String> = self
            .worktrees
            .iter()
            .filter_map(|worktree| {
                worktree
                    .pr_url
                    .as_ref()
                    .map(|pr_url| (worktree.path.clone(), pr_url.clone()))
            })
            .collect();
        let previous_agent_states: HashMap<PathBuf, AgentState> = self
            .worktrees
            .iter()
            .filter_map(|worktree| {
                worktree
                    .agent_state
                    .map(|state| (worktree.path.clone(), state))
            })
            .collect();

        let mut refresh_errors = Vec::new();
        let mut next_worktrees = Vec::new();

        for repository in &self.repositories {
            match worktree::list(&repository.root) {
                Ok(entries) => {
                    next_worktrees.extend(
                        entries
                            .iter()
                            .map(|entry| WorktreeSummary::from_worktree(entry, &repository.root)),
                    );
                },
                Err(error) => {
                    refresh_errors.push(format!("{}: {error}", repository.label));
                },
            }
        }

        for worktree in &mut next_worktrees {
            worktree.diff_summary = previous_summaries.get(&worktree.path).copied();
            worktree.pr_number = previous_pr_numbers.get(&worktree.path).copied();
            worktree.pr_url = previous_pr_urls.get(&worktree.path).cloned();
            worktree.agent_state = previous_agent_states.get(&worktree.path).copied();
        }

        let rows_changed = worktree_rows_changed(&self.worktrees, &next_worktrees);
        self.worktrees = next_worktrees;
        self.worktree_stats_loading = self
            .worktrees
            .iter()
            .any(|worktree| worktree.diff_summary.is_none());

        self.active_worktree_index = previously_selected
            .and_then(|path| {
                self.worktrees
                    .iter()
                    .position(|worktree| worktree.path == path)
            })
            .or_else(|| {
                self.active_repository_index.and_then(|repository_index| {
                    self.repositories
                        .get(repository_index)
                        .and_then(|repository| {
                            self.worktrees
                                .iter()
                                .position(|worktree| worktree.repo_root == repository.root)
                        })
                })
            })
            .or_else(|| (!self.worktrees.is_empty()).then_some(0));

        self.active_terminal_by_worktree.retain(|path, _| {
            self.worktrees
                .iter()
                .any(|worktree| worktree.path.as_path() == path.as_path())
        });
        self.diff_sessions.retain(|session| {
            self.worktrees
                .iter()
                .any(|worktree| worktree.path == session.worktree_path)
        });
        if self.active_diff_session_id.is_some_and(|diff_id| {
            !self
                .diff_sessions
                .iter()
                .any(|session| session.id == diff_id)
        }) {
            self.active_diff_session_id = None;
        }

        self.sync_active_repository_from_selected_worktree();

        if refresh_errors.is_empty() {
            if self
                .notice
                .as_deref()
                .is_some_and(|notice| notice.starts_with("failed to refresh worktrees:"))
            {
                self.notice = None;
            }
        } else {
            self.worktree_stats_loading = false;
            self.notice = Some(format!(
                "failed to refresh worktrees: {}",
                refresh_errors.join(", ")
            ));
        }

        self.refresh_worktree_diff_summaries(cx);
        self.refresh_worktree_pull_requests(cx);
        let changed_files_changed = self.reload_changed_files();
        let created_terminal = self.ensure_selected_worktree_terminal();
        if created_terminal {
            self.sync_daemon_session_store(cx);
        }
        if rows_changed || changed_files_changed || created_terminal {
            cx.notify();
        }
    }

    fn refresh_worktree_diff_summaries(&mut self, cx: &mut Context<Self>) {
        let worktree_paths: Vec<PathBuf> = self
            .worktrees
            .iter()
            .map(|worktree| worktree.path.clone())
            .collect();
        if worktree_paths.is_empty() {
            self.worktree_stats_loading = false;
            return;
        }

        cx.spawn(async move |this, cx| {
            let summaries = cx
                .background_spawn(async move {
                    let mut results = Vec::with_capacity(worktree_paths.len());
                    for path in worktree_paths {
                        results.push((path.clone(), changes::diff_line_summary(&path)));
                    }
                    results
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let mut changed = false;
                for (path, summary_result) in summaries {
                    if let Some(worktree) = this
                        .worktrees
                        .iter_mut()
                        .find(|worktree| worktree.path == path)
                    {
                        let next_summary = summary_result.ok();
                        if worktree.diff_summary != next_summary {
                            worktree.diff_summary = next_summary;
                            changed = true;
                        }
                    }
                }
                if this.worktree_stats_loading {
                    this.worktree_stats_loading = false;
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn refresh_agent_activity(&mut self, cx: &mut Context<Self>) {
        if self.agent_ws_connected {
            return;
        }

        let worktree_paths: Vec<PathBuf> = self
            .worktrees
            .iter()
            .map(|worktree| worktree.path.clone())
            .collect();
        if worktree_paths.is_empty() {
            return;
        }

        cx.spawn(async move |this, cx| {
            let active_worktrees = cx
                .background_spawn(async move {
                    let cwds = arbor_core::agent::detect_agent_cwds();
                    arbor_core::agent::worktrees_with_agents(&cwds, &worktree_paths)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let mut changed = false;
                for worktree in &mut this.worktrees {
                    let new_state = if active_worktrees.contains(&worktree.path) {
                        Some(AgentState::Working)
                    } else {
                        None
                    };
                    if worktree.agent_state != new_state {
                        worktree.agent_state = new_state;
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn start_agent_activity_ws(&mut self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        cx.spawn(async move |this, cx| {
            let ws_url = daemon_base_url
                .replace("http://", "ws://")
                .replace("https://", "wss://")
                + "/api/v1/agent/activity/ws";

            let mut backoff_secs = 3u64;

            loop {
                let url = ws_url.clone();
                let (tx, rx) = smol::channel::unbounded::<Option<String>>();

                cx.background_spawn(async move {
                    let Ok((mut ws, _)) = tungstenite::connect(&url) else {
                        let _ = tx.send(None).await;
                        return;
                    };
                    loop {
                        match ws.read() {
                            Ok(tungstenite::Message::Text(text)) => {
                                if tx.send(Some(text.to_string())).await.is_err() {
                                    break;
                                }
                            },
                            Ok(tungstenite::Message::Ping(_))
                            | Ok(tungstenite::Message::Pong(_)) => {},
                            Ok(tungstenite::Message::Close(_)) | Err(_) => {
                                let _ = tx.send(None).await;
                                break;
                            },
                            _ => {},
                        }
                    }
                })
                .detach();

                let first = rx.recv().await;
                match first {
                    Ok(Some(text)) => {
                        tracing::info!("agent activity WS connected");
                        backoff_secs = 3;
                        let _ = this.update(cx, |this, _| {
                            this.agent_ws_connected = true;
                        });
                        // Process the first message
                        process_agent_ws_message(&this, cx, &text);

                        // Process subsequent messages
                        loop {
                            match rx.recv().await {
                                Ok(Some(text)) => {
                                    process_agent_ws_message(&this, cx, &text);
                                },
                                Ok(None) | Err(_) => break,
                            }
                        }
                    },
                    _ => {},
                }

                tracing::debug!("agent activity WS disconnected, will retry");
                let _ = this.update(cx, |this, _| {
                    this.agent_ws_connected = false;
                });

                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_secs(backoff_secs));
                })
                .await;
                backoff_secs = (backoff_secs * 2).min(30);
            }
        })
        .detach();
    }

    fn ensure_claude_code_hooks(&self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        cx.spawn(async move |_this, cx| {
            cx.background_spawn(async move {
                if let Err(error) = install_claude_code_hooks(&daemon_base_url) {
                    tracing::warn!(%error, "failed to install Claude Code hooks");
                }
            })
            .await;
        })
        .detach();
    }

    fn refresh_worktree_pull_requests(&mut self, cx: &mut Context<Self>) {
        if self.worktree_prs_loading {
            return;
        }

        let repository_slug_by_root: HashMap<PathBuf, String> = self
            .repositories
            .iter()
            .filter_map(|repository| {
                repository
                    .github_repo_slug
                    .as_ref()
                    .map(|slug| (repository.root.clone(), slug.clone()))
            })
            .collect();

        let tracked_branches: Vec<(PathBuf, String, Option<String>)> = self
            .worktrees
            .iter()
            .filter(|worktree| should_lookup_pull_request_for_worktree(worktree))
            .map(|worktree| {
                (
                    worktree.path.clone(),
                    worktree.branch.clone(),
                    repository_slug_by_root.get(&worktree.repo_root).cloned(),
                )
            })
            .collect();

        if tracked_branches.is_empty() {
            let mut changed = false;
            for worktree in &mut self.worktrees {
                if worktree.pr_number.take().is_some() || worktree.pr_url.take().is_some() {
                    changed = true;
                }
            }
            if changed {
                cx.notify();
            }
            return;
        }

        self.worktree_prs_loading = true;
        cx.spawn(async move |this, cx| {
            let pr_details = cx
                .background_spawn(async move {
                    tracked_branches
                        .into_iter()
                        .map(|(path, branch, repo_slug)| {
                            let pr_number = repo_slug
                                .as_ref()
                                .and_then(|_| github_pr_number_for_worktree(&path, &branch));
                            let pr_url = pr_number.and_then(|pr_number| {
                                repo_slug
                                    .as_ref()
                                    .map(|repo_slug| github_pr_url(repo_slug, pr_number))
                            });
                            (path, pr_number, pr_url)
                        })
                        .collect::<Vec<_>>()
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.worktree_prs_loading = false;
                let pr_detail_by_path: HashMap<PathBuf, (Option<u64>, Option<String>)> = pr_details
                    .into_iter()
                    .map(|(path, pr_number, pr_url)| (path, (pr_number, pr_url)))
                    .collect();
                let mut changed = false;

                for worktree in &mut this.worktrees {
                    let (next_pr_number, next_pr_url) = pr_detail_by_path
                        .get(&worktree.path)
                        .cloned()
                        .unwrap_or((None, None));
                    if worktree.pr_number != next_pr_number || worktree.pr_url != next_pr_url {
                        worktree.pr_number = next_pr_number;
                        worktree.pr_url = next_pr_url;
                        changed = true;
                    }
                }

                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn selected_worktree_path(&self) -> Option<&Path> {
        if let Some(outpost_index) = self.active_outpost_index {
            return self
                .outposts
                .get(outpost_index)
                .map(|outpost| outpost.repo_root.as_path());
        }
        self.active_worktree_index
            .and_then(|index| self.worktrees.get(index))
            .map(|worktree| worktree.path.as_path())
    }

    fn can_run_local_git_actions(&self) -> bool {
        self.active_outpost_index.is_none() && self.selected_worktree_path().is_some()
    }

    fn active_worktree(&self) -> Option<&WorktreeSummary> {
        self.active_worktree_index
            .and_then(|index| self.worktrees.get(index))
    }

    fn active_terminal_id_for_worktree(&self, worktree_path: &Path) -> Option<u64> {
        self.active_terminal_by_worktree
            .get(worktree_path)
            .copied()
            .filter(|session_id| {
                self.terminals.iter().any(|session| {
                    session.id == *session_id && session.worktree_path.as_path() == worktree_path
                })
            })
            .or_else(|| {
                self.terminals
                    .iter()
                    .find(|session| session.worktree_path.as_path() == worktree_path)
                    .map(|session| session.id)
            })
    }

    fn active_terminal_id_for_selected_worktree(&self) -> Option<u64> {
        let worktree_path = self.selected_worktree_path()?;
        let is_outpost = self.active_outpost_index.is_some();

        self.active_terminal_by_worktree
            .get(worktree_path)
            .copied()
            .filter(|session_id| {
                self.terminals.iter().any(|session| {
                    session.id == *session_id
                        && session.worktree_path.as_path() == worktree_path
                        && is_outpost
                            == session
                                .runtime
                                .as_ref()
                                .is_some_and(|rt| matches!(rt, TerminalRuntime::Outpost(_)))
                })
            })
            .or_else(|| {
                self.terminals
                    .iter()
                    .find(|session| {
                        session.worktree_path.as_path() == worktree_path
                            && is_outpost
                                == session
                                    .runtime
                                    .as_ref()
                                    .is_some_and(|rt| matches!(rt, TerminalRuntime::Outpost(_)))
                    })
                    .map(|session| session.id)
            })
    }

    fn selected_worktree_terminals(&self) -> Vec<&TerminalSession> {
        let Some(worktree_path) = self.selected_worktree_path() else {
            return Vec::new();
        };

        let is_outpost = self.active_outpost_index.is_some();

        self.terminals
            .iter()
            .filter(|session| {
                session.worktree_path.as_path() == worktree_path
                    && is_outpost
                        == session
                            .runtime
                            .as_ref()
                            .is_some_and(|rt| matches!(rt, TerminalRuntime::Outpost(_)))
            })
            .collect()
    }

    fn selected_worktree_diff_sessions(&self) -> Vec<&DiffSession> {
        let Some(worktree_path) = self.selected_worktree_path() else {
            return Vec::new();
        };

        self.diff_sessions
            .iter()
            .filter(|session| session.worktree_path.as_path() == worktree_path)
            .collect()
    }

    fn active_center_tab_for_selected_worktree(&self) -> Option<CenterTab> {
        if self.logs_tab_active {
            return Some(CenterTab::Logs);
        }

        if let Some(diff_id) = self.active_diff_session_id {
            let worktree_path = self.selected_worktree_path()?;
            if self.diff_sessions.iter().any(|session| {
                session.id == diff_id && session.worktree_path.as_path() == worktree_path
            }) {
                return Some(CenterTab::Diff(diff_id));
            }
        }

        self.active_terminal_id_for_selected_worktree()
            .map(CenterTab::Terminal)
    }

    fn ensure_selected_worktree_terminal(&mut self) -> bool {
        // Don't auto-spawn local terminals when an outpost is selected;
        // outpost terminals are created explicitly via spawn_outpost_terminal.
        if self.active_outpost_index.is_some() {
            return false;
        }

        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            return false;
        };

        let has_terminal = self
            .terminals
            .iter()
            .any(|session| session.worktree_path == worktree_path);
        if !has_terminal {
            return self.spawn_terminal_session_inner(false);
        }

        if let Some(session_id) = self.active_terminal_id_for_worktree(&worktree_path) {
            self.active_terminal_by_worktree
                .insert(worktree_path, session_id);
        }

        true
    }

    fn close_terminal_session_by_id(&mut self, session_id: u64) -> bool {
        tracing::info!(session_id, "closing terminal session");
        let Some(index) = self
            .terminals
            .iter()
            .position(|session| session.id == session_id)
        else {
            return false;
        };

        if let Some(session) = self.terminals.get(index) {
            match &session.runtime {
                Some(TerminalRuntime::Outpost(OutpostTerminalRuntime::MoshShell(mosh))) => {
                    mosh.close();
                },
                Some(TerminalRuntime::Outpost(OutpostTerminalRuntime::SshShell(ssh))) => {
                    ssh.close();
                },
                runtime => {
                    let daemon_to_use = match runtime {
                        Some(TerminalRuntime::Daemon) => self.terminal_daemon.as_ref(),
                        Some(TerminalRuntime::Outpost(OutpostTerminalRuntime::RemoteDaemon {
                            daemon,
                        })) => Some(daemon),
                        _ => None,
                    };

                    if let Some(daemon) = daemon_to_use {
                        let result = if session.state == TerminalState::Running {
                            daemon.kill(KillRequest {
                                session_id: session.daemon_session_id.clone(),
                            })
                        } else {
                            daemon.detach(DetachRequest {
                                session_id: session.daemon_session_id.clone(),
                            })
                        };

                        if let Err(error) = result {
                            self.notice =
                                Some(format!("failed to close terminal session: {error}"));
                        }
                    }
                },
            }
        }

        let closed = self.terminals.remove(index);
        if self
            .active_terminal_by_worktree
            .get(&closed.worktree_path)
            .copied()
            == Some(closed.id)
        {
            let replacement = self
                .terminals
                .iter()
                .rev()
                .find(|session| session.worktree_path == closed.worktree_path)
                .map(|session| session.id);
            if let Some(replacement_id) = replacement {
                self.active_terminal_by_worktree
                    .insert(closed.worktree_path, replacement_id);
            } else {
                self.active_terminal_by_worktree
                    .remove(&closed.worktree_path);
            }
        }

        if self
            .terminal_selection
            .as_ref()
            .is_some_and(|selection| selection.session_id == session_id)
        {
            self.terminal_selection = None;
            self.terminal_selection_drag_anchor = None;
        }

        true
    }

    fn close_diff_session_by_id(&mut self, session_id: u64) -> bool {
        let Some(index) = self
            .diff_sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return false;
        };

        self.diff_sessions.remove(index);
        if self.active_diff_session_id == Some(session_id) {
            self.active_diff_session_id = None;
        }
        true
    }

    fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_center_tab_for_selected_worktree() {
            Some(CenterTab::Terminal(session_id)) => {
                if self.close_terminal_session_by_id(session_id) {
                    self.sync_daemon_session_store(cx);
                    self.terminal_scroll_handle.scroll_to_bottom();
                    window.focus(&self.terminal_focus);
                    self.focus_terminal_on_next_render = false;
                    cx.notify();
                }
            },
            Some(CenterTab::Diff(diff_session_id)) => {
                if self.close_diff_session_by_id(diff_session_id) {
                    cx.notify();
                }
            },
            Some(CenterTab::Logs) => {
                self.logs_tab_open = false;
                self.logs_tab_active = false;
                cx.notify();
            },
            None => {},
        }
    }

    fn theme(&self) -> ThemePalette {
        self.theme_kind.palette()
    }

    fn selected_repository(&self) -> Option<&RepositorySummary> {
        self.active_repository_index
            .and_then(|index| self.repositories.get(index))
    }

    fn sync_active_repository_from_selected_worktree(&mut self) {
        let Some(worktree_repo_root) = self
            .active_worktree()
            .map(|worktree| worktree.repo_root.clone())
        else {
            return;
        };

        let Some(index) = self
            .repositories
            .iter()
            .position(|repository| repository.root == worktree_repo_root)
        else {
            return;
        };

        self.active_repository_index = Some(index);
        if let Some(repository) = self.repositories.get(index) {
            self.repo_root = repository.root.clone();
            self.github_repo_slug = repository.github_repo_slug.clone();
        }
    }

    fn selected_repository_label(&self) -> String {
        if let Some(worktree) = self.active_worktree() {
            return self
                .repositories
                .iter()
                .find(|repository| repository.root == worktree.repo_root)
                .map(|repository| repository.label.clone())
                .unwrap_or_else(|| repository_display_name(&worktree.repo_root));
        }

        self.selected_repository()
            .map(|repository| repository.label.clone())
            .unwrap_or_else(|| repository_display_name(&self.repo_root))
    }

    fn select_repository(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(repository) = self.repositories.get(index).cloned() else {
            return;
        };
        if self.active_repository_index == Some(index) {
            return;
        }

        self.active_repository_index = Some(index);
        self.repo_root = repository.root.clone();
        self.github_repo_slug = repository.github_repo_slug.clone();
        self.worktree_stats_loading = false;
        self.worktree_prs_loading = false;
        self.active_diff_session_id = None;
        self.active_worktree_index = self
            .worktrees
            .iter()
            .position(|worktree| worktree.repo_root == repository.root);
        self.refresh_worktrees(cx);
        self.focus_terminal_on_next_render = true;
        cx.notify();
    }

    fn persist_repositories(&mut self, cx: &mut Context<Self>) {
        let roots_to_save = repository_store::repository_roots_from_summaries(&self.repositories);
        if let Err(error) = self.repository_store.save_roots(&roots_to_save) {
            self.notice = Some(format!("failed to save repositories: {error}"));
            cx.notify();
        }
    }

    fn add_repository_from_path(&mut self, selected_path: PathBuf, cx: &mut Context<Self>) {
        let repository_root = match worktree::repo_root(&selected_path) {
            Ok(path) => path,
            Err(error) => {
                self.notice = Some(format!(
                    "failed to resolve git repository root from `{}`: {error}",
                    selected_path.display()
                ));
                cx.notify();
                return;
            },
        };
        let repository_root = match repository_root.canonicalize() {
            Ok(path) => path,
            Err(_) => repository_root,
        };

        if let Some(index) = self
            .repositories
            .iter()
            .position(|repository| repository.root == repository_root)
        {
            self.select_repository(index, cx);
            self.notice = Some(format!(
                "repository `{}` is already added",
                repository_display_name(&repository_root)
            ));
            cx.notify();
            return;
        }

        let repository = RepositorySummary::from_root(repository_root.clone());
        let repository_label = repository.label.clone();
        self.repositories.push(repository);
        self.persist_repositories(cx);
        let index = self.repositories.len().saturating_sub(1);
        self.select_repository(index, cx);
        self.notice = Some(format!("added repository `{repository_label}`"));
        cx.notify();
    }

    fn open_add_repository_picker(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select Git Repository".into()),
        });

        cx.spawn(async move |this, cx| {
            let Ok(selection) = picker.await else {
                return;
            };

            let _ = this.update(cx, |this, cx| match selection {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.into_iter().next() {
                        this.add_repository_from_path(path, cx);
                    }
                },
                Ok(None) => {},
                Err(error) => {
                    this.notice = Some(format!("failed to open repository picker: {error}"));
                    cx.notify();
                },
            });
        })
        .detach();
    }

    fn open_external_url(&mut self, url: &str, cx: &mut Context<Self>) {
        cx.open_url(url);
    }

    fn select_worktree(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(worktree) = self.worktrees.get(index) {
            tracing::info!(worktree = %worktree.path.display(), branch = %worktree.branch, "switching worktree");
        }
        if let Some(old) = self.active_worktree_index
            && old != index
        {
            self.worktree_nav_back.push(old);
            self.worktree_nav_forward.clear();
        }
        self.active_worktree_index = Some(index);
        self.active_outpost_index = None;
        self.active_diff_session_id = None;
        self.sync_active_repository_from_selected_worktree();
        let _ = self.reload_changed_files();
        self.expanded_dirs.clear();
        self.selected_file_tree_entry = None;
        self.file_tree_entries.clear();
        if self.right_pane_tab == RightPaneTab::FileTree {
            self.rebuild_file_tree();
        }
        if self.ensure_selected_worktree_terminal() {
            self.sync_daemon_session_store(cx);
        }
        self.terminal_scroll_handle.scroll_to_bottom();
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        cx.notify();
    }

    fn select_outpost(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        self.active_outpost_index = Some(index);
        self.active_worktree_index = None;
        self.changed_files.clear();
        self.selected_changed_file = None;
        self.refresh_remote_changed_files(cx);
        cx.notify();
    }

    fn remove_outpost(&mut self, outpost_index: usize, cx: &mut Context<Self>) {
        let Some(outpost) = self.outposts.get(outpost_index) else {
            return;
        };

        let outpost_id = outpost.outpost_id.clone();
        if let Err(error) = self.outpost_store.remove(&outpost_id) {
            self.notice = Some(format!("failed to remove outpost: {error}"));
            cx.notify();
            return;
        }

        self.outposts.remove(outpost_index);

        if self.active_outpost_index == Some(outpost_index) {
            self.active_outpost_index = None;
        } else if let Some(active) = self.active_outpost_index
            && active > outpost_index
        {
            self.active_outpost_index = Some(active - 1);
        }

        cx.notify();
    }

    fn reload_changed_files(&mut self) -> bool {
        let previous_files = self.changed_files.clone();
        let previous_notice = self.notice.clone();
        // Remote outposts don't have a local working tree to diff against.
        if self.active_outpost_index.is_some() {
            self.changed_files.clear();
            self.selected_changed_file = None;
            return self.changed_files != previous_files;
        }
        let Some(path) = self.selected_worktree_path() else {
            self.changed_files.clear();
            self.selected_changed_file = None;
            return self.changed_files != previous_files || self.notice != previous_notice;
        };

        match changes::changed_files(path) {
            Ok(files) => {
                self.changed_files = files;
                self.notice = None;
            },
            Err(error) => {
                self.changed_files.clear();
                self.notice = Some(format!("failed to load changed files with gix: {error}"));
            },
        }

        self.sync_selected_changed_file();
        self.changed_files != previous_files || self.notice != previous_notice
    }

    fn refresh_remote_changed_files(&mut self, cx: &mut Context<Self>) {
        let Some(outpost_index) = self.active_outpost_index else {
            return;
        };
        let Some(outpost) = self.outposts.get(outpost_index) else {
            return;
        };
        let Some(host) = self
            .remote_hosts
            .iter()
            .find(|h| h.name == outpost.host_name)
            .cloned()
        else {
            return;
        };

        let remote_path = outpost.remote_path.clone();
        let pool = self.ssh_connection_pool.clone();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let conn_slot = pool
                        .get_or_connect(&host)
                        .map_err(|e| format!("SSH connection failed: {e}"))?;
                    let guard = conn_slot
                        .lock()
                        .map_err(|_| "SSH connection lock poisoned".to_owned())?;
                    let connection = guard
                        .as_ref()
                        .ok_or_else(|| "SSH connection not available".to_owned())?;

                    use arbor_core::remote::RemoteTransport;

                    let status_output = connection
                        .run_command(&format!("cd {remote_path} && git status --porcelain"))
                        .map_err(|e| format!("{e}"))?;
                    if status_output.exit_code != Some(0) {
                        return Err(format!("git status failed: {}", status_output.stderr));
                    }

                    let numstat_output = connection
                        .run_command(&format!(
                            "cd {remote_path} && git diff --numstat HEAD 2>/dev/null"
                        ))
                        .map_err(|e| format!("{e}"))?;
                    let numstat_map = parse_remote_numstat_output(&numstat_output.stdout);

                    let mut files = Vec::new();
                    for line in status_output.stdout.lines() {
                        if line.len() < 3 {
                            continue;
                        }
                        let xy = &line[..2];
                        let path_str = line[3..].trim();
                        if path_str.is_empty() {
                            continue;
                        }
                        let path = PathBuf::from(path_str);
                        let kind = porcelain_status_to_change_kind(xy);
                        let (additions, deletions) =
                            numstat_map.get(&path).copied().unwrap_or((0, 0));
                        files.push(ChangedFile {
                            path,
                            kind,
                            additions,
                            deletions,
                        });
                    }
                    files.sort_by(|a, b| a.path.cmp(&b.path));
                    Ok::<Vec<ChangedFile>, String>(files)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.active_outpost_index.is_some() {
                    if let Ok(files) = result {
                        this.changed_files = files;
                        this.sync_selected_changed_file();
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn run_commit_action(&mut self, cx: &mut Context<Self>) {
        if self.git_action_in_flight.is_some() {
            return;
        }

        if self.active_outpost_index.is_some() {
            self.notice = Some("git actions are only available for local worktrees".to_owned());
            cx.notify();
            return;
        }

        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a worktree before committing".to_owned());
            cx.notify();
            return;
        };

        if self.changed_files.is_empty() {
            self.notice = Some("nothing to commit".to_owned());
            cx.notify();
            return;
        }

        let changed_files = self.changed_files.clone();
        self.git_action_in_flight = Some(GitActionKind::Commit);
        self.notice = Some("running git commit".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                run_git_commit_for_worktree(worktree_path.as_path(), &changed_files)
            });
            let result = result.await;

            let _ = this.update(cx, |this, cx| {
                this.git_action_in_flight = None;
                match result {
                    Ok(message) => {
                        this.notice = Some(message);
                        let _ = this.reload_changed_files();
                        this.refresh_worktree_diff_summaries(cx);
                        this.refresh_worktree_pull_requests(cx);
                    },
                    Err(error) => {
                        this.notice = Some(error);
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn run_push_action(&mut self, cx: &mut Context<Self>) {
        if self.git_action_in_flight.is_some() {
            return;
        }

        if self.active_outpost_index.is_some() {
            self.notice = Some("git actions are only available for local worktrees".to_owned());
            cx.notify();
            return;
        }

        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a worktree before pushing".to_owned());
            cx.notify();
            return;
        };

        self.git_action_in_flight = Some(GitActionKind::Push);
        self.notice = Some("running git push".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { run_git_push_for_worktree(worktree_path.as_path()) })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.git_action_in_flight = None;
                match result {
                    Ok(message) => {
                        this.notice = Some(message);
                        this.refresh_worktree_pull_requests(cx);
                    },
                    Err(error) => {
                        this.notice = Some(error);
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn run_create_pr_action(&mut self, cx: &mut Context<Self>) {
        if self.git_action_in_flight.is_some() {
            return;
        }

        if self.active_outpost_index.is_some() {
            self.notice = Some("git actions are only available for local worktrees".to_owned());
            cx.notify();
            return;
        }

        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a worktree before creating a PR".to_owned());
            cx.notify();
            return;
        };

        let repo_slug = self
            .github_repo_slug
            .clone()
            .or_else(|| github_repo_slug_for_repo(worktree_path.as_path()));

        self.git_action_in_flight = Some(GitActionKind::CreatePullRequest);
        self.notice = Some("running gh pr create".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                run_create_pr_for_worktree(worktree_path.as_path(), repo_slug.as_deref())
            });
            let result = result.await;

            let _ = this.update(cx, |this, cx| {
                this.git_action_in_flight = None;
                match result {
                    Ok(message) => {
                        if let Some(url) = extract_first_url(&message) {
                            this.open_external_url(&url, cx);
                        }
                        this.notice = Some(message);
                        this.refresh_worktree_pull_requests(cx);
                    },
                    Err(error) => {
                        this.notice = Some(error);
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_selected_changed_file(&mut self) {
        let Some(selected) = self.selected_changed_file.as_ref() else {
            self.selected_changed_file =
                self.changed_files.first().map(|change| change.path.clone());
            return;
        };

        if !self
            .changed_files
            .iter()
            .any(|change| change.path.as_path() == selected.as_path())
        {
            self.selected_changed_file =
                self.changed_files.first().map(|change| change.path.clone());
        }
    }

    fn select_changed_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self
            .selected_changed_file
            .as_ref()
            .is_some_and(|selected| selected == &path)
        {
            return;
        }
        self.selected_changed_file = Some(path);
        if let Some(selected_path) = self.selected_changed_file.as_ref()
            && !self.scroll_diff_to_file(selected_path.as_path())
            && self
                .active_center_tab_for_selected_worktree()
                .is_some_and(|tab| matches!(tab, CenterTab::Diff(_)))
        {
            self.pending_diff_scroll_to_file = Some(selected_path.clone());
        }
        cx.notify();
    }

    fn selected_changed_file(&self) -> Option<&ChangedFile> {
        let selected_path = self.selected_changed_file.as_ref()?;
        self.changed_files
            .iter()
            .find(|change| change.path == *selected_path)
    }

    fn rebuild_file_tree(&mut self) {
        let Some(worktree_path) = self.selected_worktree_path().map(|p| p.to_path_buf()) else {
            self.file_tree_entries.clear();
            return;
        };
        let mut entries = Vec::new();
        self.walk_directory(&worktree_path, &worktree_path, 0, &mut entries);
        self.file_tree_entries = entries;
    }

    fn walk_directory(
        &self,
        base: &Path,
        dir: &Path,
        depth: usize,
        entries: &mut Vec<FileTreeEntry>,
    ) {
        let Ok(read_dir) = fs::read_dir(dir) else {
            return;
        };

        let mut children: Vec<(String, PathBuf, bool)> = Vec::new();
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().is_ok_and(|ft| ft.is_dir());
            if is_dir
                && matches!(
                    name.as_str(),
                    "node_modules" | "target" | "__pycache__" | ".git"
                )
            {
                continue;
            }
            children.push((name, entry.path(), is_dir));
        }

        children.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });

        for (name, full_path, is_dir) in children {
            let relative = full_path
                .strip_prefix(base)
                .unwrap_or(&full_path)
                .to_path_buf();
            entries.push(FileTreeEntry {
                path: relative.clone(),
                name,
                is_dir,
                depth,
            });
            if is_dir && self.expanded_dirs.contains(&relative) {
                self.walk_directory(base, &full_path, depth + 1, entries);
            }
        }
    }

    fn toggle_file_tree_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.expanded_dirs.contains(&path) {
            self.expanded_dirs.remove(&path);
        } else {
            self.expanded_dirs.insert(path.clone());
        }
        self.selected_file_tree_entry = Some(path);
        self.rebuild_file_tree();
        cx.notify();
    }

    fn select_file_tree_entry(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected_file_tree_entry = Some(path);
        cx.notify();
    }

    fn set_right_pane_tab(&mut self, tab: RightPaneTab, cx: &mut Context<Self>) {
        if self.right_pane_tab == tab {
            return;
        }
        self.right_pane_tab = tab;
        if tab == RightPaneTab::FileTree && self.file_tree_entries.is_empty() {
            self.rebuild_file_tree();
        }
        cx.notify();
    }

    fn switch_terminal_backend(
        &mut self,
        backend_kind: TerminalBackendKind,
        cx: &mut Context<Self>,
    ) {
        if self.active_backend_kind == backend_kind {
            return;
        }

        self.active_backend_kind = backend_kind;
        self.notice = None;
        cx.notify();
    }

    fn switch_theme(&mut self, theme_kind: ThemeKind, cx: &mut Context<Self>) {
        if self.theme_kind == theme_kind {
            return;
        }

        self.theme_kind = theme_kind;
        self.notice = Some(format!("theme switched to {}", theme_kind.label()));
        cx.notify();
    }

    fn open_create_modal(
        &mut self,
        repo_index: usize,
        tab: CreateModalTab,
        cx: &mut Context<Self>,
    ) {
        let repository_path = self
            .repositories
            .get(repo_index)
            .map(|r| r.root.display().to_string())
            .unwrap_or_else(|| self.repo_root.display().to_string());
        let clone_url = self
            .repositories
            .get(repo_index)
            .and_then(|r| r.github_repo_slug.as_ref())
            .map(|slug| format!("git@github.com:{slug}.git"))
            .unwrap_or_default();
        self.create_modal = Some(CreateModal {
            tab,
            repository_path,
            worktree_name: String::new(),
            worktree_active_field: CreateWorktreeField::WorktreeName,
            host_index: 0,
            clone_url,
            branch: "main".to_owned(),
            outpost_name: String::new(),
            outpost_active_field: CreateOutpostField::CloneUrl,
            is_creating: false,
            error: None,
        });
        cx.notify();
    }

    fn close_create_modal(&mut self, cx: &mut Context<Self>) {
        self.create_modal = None;
        cx.notify();
    }

    fn update_create_worktree_modal_input(
        &mut self,
        input: ModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };

        if modal.is_creating {
            return;
        }

        match input {
            ModalInputEvent::SetActiveField(field) => {
                modal.worktree_active_field = field;
            },
            ModalInputEvent::MoveActiveField => {
                modal.worktree_active_field = match modal.worktree_active_field {
                    CreateWorktreeField::RepositoryPath => CreateWorktreeField::WorktreeName,
                    CreateWorktreeField::WorktreeName => CreateWorktreeField::RepositoryPath,
                };
            },
            ModalInputEvent::Backspace => {
                let field_value = match modal.worktree_active_field {
                    CreateWorktreeField::RepositoryPath => &mut modal.repository_path,
                    CreateWorktreeField::WorktreeName => &mut modal.worktree_name,
                };
                let _ = field_value.pop();
            },
            ModalInputEvent::Append(text) => {
                let field_value = match modal.worktree_active_field {
                    CreateWorktreeField::RepositoryPath => &mut modal.repository_path,
                    CreateWorktreeField::WorktreeName => &mut modal.worktree_name,
                };
                field_value.push_str(&text);
            },
            ModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        cx.notify();
    }

    fn submit_create_worktree_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };
        if modal.is_creating {
            return;
        }

        modal.error = None;
        let repository_input = modal.repository_path.trim().to_owned();
        let worktree_input = modal.worktree_name.trim().to_owned();

        if repository_input.is_empty() {
            modal.error = Some("Repository path is required.".to_owned());
            cx.notify();
            return;
        }

        if worktree_input.is_empty() {
            modal.error = Some("Worktree name is required.".to_owned());
            cx.notify();
            return;
        }

        modal.is_creating = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let creation = cx
                .background_spawn(async move {
                    create_managed_worktree(repository_input, worktree_input)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match creation {
                    Ok(created) => {
                        this.notice = Some(format!(
                            "created worktree `{}` on branch `{}`",
                            created.worktree_name, created.branch_name
                        ));
                        this.create_modal = None;
                        this.refresh_worktrees(cx);
                        if let Some(index) = this
                            .worktrees
                            .iter()
                            .position(|worktree| worktree.path == created.worktree_path)
                        {
                            this.active_worktree_index = Some(index);
                            let _ = this.reload_changed_files();
                            if this.ensure_selected_worktree_terminal() {
                                this.sync_daemon_session_store(cx);
                            }
                            this.terminal_scroll_handle.scroll_to_bottom();
                            this.focus_terminal_on_next_render = true;
                        }
                    },
                    Err(error) => {
                        if let Some(modal) = this.create_modal.as_mut() {
                            modal.is_creating = false;
                            modal.error = Some(error);
                        } else {
                            this.notice = Some(error);
                        }
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_manage_hosts_modal(&mut self, cx: &mut Context<Self>) {
        self.manage_hosts_modal = Some(ManageHostsModal {
            adding: false,
            name: String::new(),
            hostname: String::new(),
            user: String::new(),
            active_field: ManageHostsField::Name,
            error: None,
        });
        cx.notify();
    }

    fn close_manage_hosts_modal(&mut self, cx: &mut Context<Self>) {
        self.manage_hosts_modal = None;
        cx.notify();
    }

    fn update_manage_hosts_modal_input(
        &mut self,
        input: HostsModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(modal) = self.manage_hosts_modal.as_mut() else {
            return;
        };

        match input {
            HostsModalInputEvent::SetActiveField(field) => {
                modal.active_field = field;
            },
            HostsModalInputEvent::MoveActiveField(reverse) => {
                modal.active_field = match (modal.active_field, reverse) {
                    (ManageHostsField::Name, false) => ManageHostsField::Hostname,
                    (ManageHostsField::Hostname, false) => ManageHostsField::User,
                    (ManageHostsField::User, false) => ManageHostsField::Name,
                    (ManageHostsField::Name, true) => ManageHostsField::User,
                    (ManageHostsField::Hostname, true) => ManageHostsField::Name,
                    (ManageHostsField::User, true) => ManageHostsField::Hostname,
                };
            },
            HostsModalInputEvent::Backspace => {
                let field_value = match modal.active_field {
                    ManageHostsField::Name => &mut modal.name,
                    ManageHostsField::Hostname => &mut modal.hostname,
                    ManageHostsField::User => &mut modal.user,
                };
                let _ = field_value.pop();
            },
            HostsModalInputEvent::Append(text) => {
                let field_value = match modal.active_field {
                    ManageHostsField::Name => &mut modal.name,
                    ManageHostsField::Hostname => &mut modal.hostname,
                    ManageHostsField::User => &mut modal.user,
                };
                field_value.push_str(&text);
            },
            HostsModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        cx.notify();
    }

    fn submit_add_host(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.manage_hosts_modal.as_mut() else {
            return;
        };
        let name = modal.name.trim().to_owned();
        let hostname = modal.hostname.trim().to_owned();
        let user = modal.user.trim().to_owned();

        if name.is_empty() || hostname.is_empty() || user.is_empty() {
            modal.error = Some("All fields are required.".to_owned());
            cx.notify();
            return;
        }

        if self.remote_hosts.iter().any(|h| h.name == name) {
            modal.error = Some(format!("Host \"{name}\" already exists."));
            cx.notify();
            return;
        }

        let host_config = app_config::RemoteHostConfig {
            name: name.clone(),
            hostname,
            user,
            port: 22,
            identity_file: None,
            remote_base_path: "~/arbor-outposts".to_owned(),
            daemon_port: None,
            mosh: None,
            mosh_server_path: None,
        };

        if let Err(error) = app_config::append_remote_host(&host_config) {
            modal.error = Some(error);
            cx.notify();
            return;
        }

        self.config_last_modified = None;
        self.refresh_config_if_changed(cx);
        self.notice = Some(format!("Host \"{name}\" added."));
        if let Some(modal) = self.manage_hosts_modal.as_mut() {
            modal.adding = false;
            modal.name.clear();
            modal.hostname.clear();
            modal.user.clear();
            modal.error = None;
        }
        cx.notify();
    }

    fn remove_host_at(&mut self, host_name: String, cx: &mut Context<Self>) {
        if let Err(error) = app_config::remove_remote_host(&host_name) {
            self.notice = Some(error);
            cx.notify();
            return;
        }
        self.config_last_modified = None;
        self.refresh_config_if_changed(cx);
        self.notice = Some(format!("Host \"{host_name}\" removed."));
        cx.notify();
    }

    fn update_create_outpost_modal_input(
        &mut self,
        input: OutpostModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };
        if modal.is_creating {
            return;
        }

        match input {
            OutpostModalInputEvent::SetActiveField(field) => {
                modal.outpost_active_field = field;
            },
            OutpostModalInputEvent::MoveActiveField(reverse) => {
                modal.outpost_active_field = match (modal.outpost_active_field, reverse) {
                    (CreateOutpostField::HostSelector, false) => CreateOutpostField::CloneUrl,
                    (CreateOutpostField::CloneUrl, false) => CreateOutpostField::Branch,
                    (CreateOutpostField::Branch, false) => CreateOutpostField::OutpostName,
                    (CreateOutpostField::OutpostName, false) => CreateOutpostField::HostSelector,
                    (CreateOutpostField::HostSelector, true) => CreateOutpostField::OutpostName,
                    (CreateOutpostField::CloneUrl, true) => CreateOutpostField::HostSelector,
                    (CreateOutpostField::Branch, true) => CreateOutpostField::CloneUrl,
                    (CreateOutpostField::OutpostName, true) => CreateOutpostField::Branch,
                };
            },
            OutpostModalInputEvent::CycleHost(reverse) => {
                let count = self.remote_hosts.len();
                if count > 0 {
                    if reverse {
                        modal.host_index = (modal.host_index + count - 1) % count;
                    } else {
                        modal.host_index = (modal.host_index + 1) % count;
                    }
                }
            },
            OutpostModalInputEvent::Backspace => {
                if modal.outpost_active_field == CreateOutpostField::HostSelector {
                    return;
                }
                let field_value = match modal.outpost_active_field {
                    CreateOutpostField::HostSelector => return,
                    CreateOutpostField::CloneUrl => &mut modal.clone_url,
                    CreateOutpostField::Branch => &mut modal.branch,
                    CreateOutpostField::OutpostName => &mut modal.outpost_name,
                };
                let _ = field_value.pop();
            },
            OutpostModalInputEvent::Append(text) => {
                if modal.outpost_active_field == CreateOutpostField::HostSelector {
                    return;
                }
                let field_value = match modal.outpost_active_field {
                    CreateOutpostField::HostSelector => return,
                    CreateOutpostField::CloneUrl => &mut modal.clone_url,
                    CreateOutpostField::Branch => &mut modal.branch,
                    CreateOutpostField::OutpostName => &mut modal.outpost_name,
                };
                field_value.push_str(&text);
            },
            OutpostModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        cx.notify();
    }

    fn submit_create_outpost_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };
        if modal.is_creating {
            return;
        }

        modal.error = None;
        let clone_url = modal.clone_url.trim().to_owned();
        let branch = modal.branch.trim().to_owned();
        let outpost_name = modal.outpost_name.trim().to_owned();
        let host_index = modal.host_index;

        if clone_url.is_empty() {
            modal.error = Some("Clone URL is required.".to_owned());
            cx.notify();
            return;
        }
        if branch.is_empty() {
            modal.error = Some("Branch is required.".to_owned());
            cx.notify();
            return;
        }
        if outpost_name.is_empty() {
            modal.error = Some("Outpost name is required.".to_owned());
            cx.notify();
            return;
        }
        let Some(host) = self.remote_hosts.get(host_index).cloned() else {
            modal.error = Some("No remote host selected.".to_owned());
            cx.notify();
            return;
        };

        modal.is_creating = true;
        cx.notify();

        let local_repo_root = self
            .selected_repository()
            .map(|r| r.root.display().to_string())
            .unwrap_or_default();
        let pool = self.ssh_connection_pool.clone();
        let host_name = host.name.clone();
        let bg_clone_url = clone_url.clone();
        let bg_outpost_name = outpost_name.clone();
        let bg_branch = branch.clone();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let conn_slot = pool
                        .get_or_connect(&host)
                        .map_err(|e| format!("SSH connection failed: {e}"))?;
                    let guard = conn_slot
                        .lock()
                        .map_err(|_| "SSH connection lock poisoned".to_owned())?;
                    let connection = guard
                        .as_ref()
                        .ok_or_else(|| "SSH connection not available".to_owned())?;
                    let provisioner =
                        arbor_ssh::provisioner::SshProvisioner::new(connection, &host);
                    use arbor_core::remote::RemoteProvisioner;
                    provisioner
                        .provision(&bg_clone_url, &bg_outpost_name, &bg_branch)
                        .map_err(|e| format!("{e}"))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(provision_result) => {
                        let timestamp = current_unix_timestamp_millis().unwrap_or(0);
                        let record = arbor_core::outpost::OutpostRecord {
                            id: format!("outpost-{timestamp}"),
                            host_name: host_name.clone(),
                            local_repo_root,
                            remote_path: provision_result.remote_path,
                            clone_url,
                            branch,
                            label: outpost_name.clone(),
                            has_remote_daemon: provision_result.has_remote_daemon,
                        };
                        if let Err(e) = this.outpost_store.upsert(record) {
                            this.notice = Some(format!("outpost created but failed to save: {e}"));
                        } else {
                            this.notice =
                                Some(format!("outpost `{outpost_name}` created on {host_name}"));
                        }
                        this.outposts =
                            load_outpost_summaries(this.outpost_store.as_ref(), &this.remote_hosts);
                        this.create_modal = None;
                    },
                    Err(error) => {
                        if let Some(modal) = this.create_modal.as_mut() {
                            modal.is_creating = false;
                            modal.error = Some(error);
                        } else {
                            this.notice = Some(error);
                        }
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn handle_global_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.is_held {
            return;
        }

        if self.manage_hosts_modal.is_some() {
            if event.keystroke.modifiers.platform {
                return;
            }

            let adding = self
                .manage_hosts_modal
                .as_ref()
                .map(|m| m.adding)
                .unwrap_or(false);

            if adding {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        if let Some(modal) = self.manage_hosts_modal.as_mut() {
                            modal.adding = false;
                            modal.error = None;
                            cx.notify();
                        }
                        cx.stop_propagation();
                        return;
                    },
                    "tab" => {
                        self.update_manage_hosts_modal_input(
                            HostsModalInputEvent::MoveActiveField(event.keystroke.modifiers.shift),
                            cx,
                        );
                        cx.stop_propagation();
                        return;
                    },
                    "enter" | "return" => {
                        self.submit_add_host(cx);
                        cx.stop_propagation();
                        return;
                    },
                    "backspace" => {
                        self.update_manage_hosts_modal_input(HostsModalInputEvent::Backspace, cx);
                        cx.stop_propagation();
                        return;
                    },
                    _ => {},
                }

                if event.keystroke.modifiers.control || event.keystroke.modifiers.alt {
                    return;
                }

                if let Some(key_char) = event.keystroke.key_char.as_ref() {
                    self.update_manage_hosts_modal_input(HostsModalInputEvent::ClearError, cx);
                    self.update_manage_hosts_modal_input(
                        HostsModalInputEvent::Append(key_char.to_owned()),
                        cx,
                    );
                    cx.stop_propagation();
                }
            } else if event.keystroke.key.as_str() == "escape" {
                self.close_manage_hosts_modal(cx);
                cx.stop_propagation();
            }
            return;
        }

        let Some(modal) = self.create_modal.as_ref() else {
            return;
        };

        if event.keystroke.modifiers.platform {
            return;
        }

        let active_tab = modal.tab;

        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_create_modal(cx);
                cx.stop_propagation();
                return;
            },
            "tab" => {
                match active_tab {
                    CreateModalTab::LocalWorktree => {
                        self.update_create_worktree_modal_input(
                            ModalInputEvent::MoveActiveField,
                            cx,
                        );
                    },
                    CreateModalTab::RemoteOutpost => {
                        self.update_create_outpost_modal_input(
                            OutpostModalInputEvent::MoveActiveField(
                                event.keystroke.modifiers.shift,
                            ),
                            cx,
                        );
                    },
                }
                cx.stop_propagation();
                return;
            },
            "enter" | "return" => {
                match active_tab {
                    CreateModalTab::LocalWorktree => self.submit_create_worktree_modal(cx),
                    CreateModalTab::RemoteOutpost => self.submit_create_outpost_modal(cx),
                }
                cx.stop_propagation();
                return;
            },
            "backspace" => {
                match active_tab {
                    CreateModalTab::LocalWorktree => {
                        self.update_create_worktree_modal_input(ModalInputEvent::Backspace, cx);
                    },
                    CreateModalTab::RemoteOutpost => {
                        self.update_create_outpost_modal_input(
                            OutpostModalInputEvent::Backspace,
                            cx,
                        );
                    },
                }
                cx.stop_propagation();
                return;
            },
            "left" | "right" => {
                if active_tab == CreateModalTab::RemoteOutpost
                    && self
                        .create_modal
                        .as_ref()
                        .map(|m| m.outpost_active_field == CreateOutpostField::HostSelector)
                        .unwrap_or(false)
                {
                    let reverse = event.keystroke.key.as_str() == "left";
                    self.update_create_outpost_modal_input(
                        OutpostModalInputEvent::CycleHost(reverse),
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                }
            },
            _ => {},
        }

        if event.keystroke.modifiers.control || event.keystroke.modifiers.alt {
            return;
        }

        if let Some(key_char) = event.keystroke.key_char.as_ref() {
            match active_tab {
                CreateModalTab::LocalWorktree => {
                    self.update_create_worktree_modal_input(ModalInputEvent::ClearError, cx);
                    self.update_create_worktree_modal_input(
                        ModalInputEvent::Append(key_char.to_owned()),
                        cx,
                    );
                },
                CreateModalTab::RemoteOutpost => {
                    self.update_create_outpost_modal_input(OutpostModalInputEvent::ClearError, cx);
                    self.update_create_outpost_modal_input(
                        OutpostModalInputEvent::Append(key_char.to_owned()),
                        cx,
                    );
                },
            }
            cx.stop_propagation();
        }
    }

    fn action_open_create_worktree(
        &mut self,
        _: &OpenCreateWorktree,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let repo_index = self.active_repository_index.unwrap_or(0);
        self.open_create_modal(repo_index, CreateModalTab::LocalWorktree, cx);
    }

    fn action_open_add_repository(
        &mut self,
        _: &OpenAddRepository,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_add_repository_picker(cx);
    }

    fn action_spawn_terminal(
        &mut self,
        _: &SpawnTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.spawn_terminal_session(window, cx);
    }

    fn action_close_active_terminal(
        &mut self,
        _: &CloseActiveTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_active_tab(window, cx);
    }

    fn action_refresh_worktrees(
        &mut self,
        _: &RefreshWorktrees,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_worktrees(cx);
        cx.notify();
    }

    fn action_refresh_changes(
        &mut self,
        _: &RefreshChanges,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.reload_changed_files();
        cx.notify();
    }

    fn action_use_one_dark_theme(
        &mut self,
        _: &UseOneDarkTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_theme(ThemeKind::One, cx);
    }

    fn action_use_ayu_dark_theme(
        &mut self,
        _: &UseAyuDarkTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_theme(ThemeKind::Ayu, cx);
    }

    fn action_use_gruvbox_theme(
        &mut self,
        _: &UseGruvboxTheme,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_theme(ThemeKind::Gruvbox, cx);
    }

    fn action_use_embedded_backend(
        &mut self,
        _: &UseEmbeddedBackend,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_terminal_backend(TerminalBackendKind::Embedded, cx);
    }

    fn action_use_alacritty_backend(
        &mut self,
        _: &UseAlacrittyBackend,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_terminal_backend(TerminalBackendKind::Alacritty, cx);
    }

    fn action_use_ghostty_backend(
        &mut self,
        _: &UseGhosttyBackend,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_terminal_backend(TerminalBackendKind::Ghostty, cx);
    }

    fn action_toggle_left_pane(
        &mut self,
        _: &ToggleLeftPane,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.left_pane_visible = !self.left_pane_visible;
        cx.notify();
    }

    fn action_navigate_worktree_back(
        &mut self,
        _: &NavigateWorktreeBack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.worktree_nav_back.pop() {
            if let Some(current) = self.active_worktree_index {
                self.worktree_nav_forward.push(current);
            }
            self.active_worktree_index = Some(target);
            self.active_diff_session_id = None;
            self.sync_active_repository_from_selected_worktree();
            let _ = self.reload_changed_files();
            if self.ensure_selected_worktree_terminal() {
                self.sync_daemon_session_store(cx);
            }
            self.terminal_scroll_handle.scroll_to_bottom();
            window.focus(&self.terminal_focus);
            self.focus_terminal_on_next_render = false;
            cx.notify();
        }
    }

    fn action_navigate_worktree_forward(
        &mut self,
        _: &NavigateWorktreeForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.worktree_nav_forward.pop() {
            if let Some(current) = self.active_worktree_index {
                self.worktree_nav_back.push(current);
            }
            self.active_worktree_index = Some(target);
            self.active_diff_session_id = None;
            self.sync_active_repository_from_selected_worktree();
            let _ = self.reload_changed_files();
            if self.ensure_selected_worktree_terminal() {
                self.sync_daemon_session_store(cx);
            }
            self.terminal_scroll_handle.scroll_to_bottom();
            window.focus(&self.terminal_focus);
            self.focus_terminal_on_next_render = false;
            cx.notify();
        }
    }

    fn action_collapse_all_repositories(
        &mut self,
        _: &CollapseAllRepositories,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let all_collapsed =
            (0..self.repositories.len()).all(|i| self.collapsed_repositories.contains(&i));
        if all_collapsed {
            self.collapsed_repositories.clear();
        } else {
            self.collapsed_repositories = (0..self.repositories.len()).collect();
        }
        cx.notify();
    }

    fn action_request_quit(&mut self, _: &RequestQuit, _: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        if self.quit_overlay_until.is_some_and(|until| now < until) {
            cx.quit();
            return;
        }

        let deadline = now + QUIT_ARM_WINDOW;
        self.quit_overlay_until = Some(deadline);
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                std::thread::sleep(QUIT_ARM_WINDOW);
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.quit_overlay_until = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn action_immediate_quit(&mut self, _: &ImmediateQuit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn action_view_logs(&mut self, _: &ViewLogs, _: &mut Window, cx: &mut Context<Self>) {
        self.logs_tab_open = true;
        self.logs_tab_active = true;
        self.active_diff_session_id = None;
        cx.notify();
    }

    fn spawn_terminal_session_inner(&mut self, show_notice_on_missing_worktree: bool) -> bool {
        let Some(cwd) = self.selected_worktree_path().map(Path::to_path_buf) else {
            if show_notice_on_missing_worktree {
                self.notice = Some("select a worktree before opening a terminal tab".to_owned());
            }
            return false;
        };

        tracing::info!(cwd = %cwd.display(), "spawning terminal session");
        let backend_kind = self.active_backend_kind;
        let session_id = self.next_terminal_id;
        self.next_terminal_id += 1;
        self.active_terminal_by_worktree
            .insert(cwd.clone(), session_id);

        let mut session = TerminalSession {
            id: session_id,
            daemon_session_id: session_id.to_string(),
            worktree_path: cwd.clone(),
            title: format!("term-{session_id}"),
            last_command: None,
            pending_command: String::new(),
            command: String::new(),
            state: TerminalState::Running,
            exit_code: None,
            updated_at_unix_ms: current_unix_timestamp_millis(),
            cols: 120,
            rows: 35,
            generation: 0,
            output: String::new(),
            styled_output: Vec::new(),
            cursor: None,
            runtime: None,
        };

        let mut launched_with_daemon = false;
        if backend_kind == TerminalBackendKind::Embedded
            && let Some(daemon) = self.terminal_daemon.as_ref()
        {
            let shell = match env::var("SHELL") {
                Ok(value) if !value.trim().is_empty() => value,
                _ => "/bin/zsh".to_owned(),
            };
            match daemon.create_or_attach(CreateOrAttachRequest {
                session_id: String::new(),
                workspace_id: cwd.display().to_string(),
                cwd: cwd.clone(),
                shell,
                cols: 120,
                rows: 35,
                title: Some(session.title.clone()),
            }) {
                Ok(response) => {
                    let daemon_session = response.session;
                    session.daemon_session_id = daemon_session.session_id.clone();
                    session.title = daemon_session
                        .title
                        .clone()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or(session.title);
                    session.last_command = daemon_session.last_command.clone();
                    session.command = daemon_session.shell.clone();
                    session.output = daemon_session.output_tail.clone().unwrap_or_default();
                    session.state = terminal_state_from_daemon_record(&daemon_session);
                    session.exit_code = daemon_session.exit_code;
                    session.updated_at_unix_ms = daemon_session.updated_at_unix_ms;
                    session.cols = daemon_session.cols.max(2);
                    session.rows = daemon_session.rows.max(1);
                    session.runtime = Some(TerminalRuntime::Daemon);
                    launched_with_daemon = true;
                },
                Err(error) => {
                    let error_text = error.to_string();
                    if daemon_error_is_connection_refused(&error_text) {
                        self.terminal_daemon = None;
                    } else {
                        self.notice = Some(format!(
                            "failed to create daemon terminal session (falling back to local embedded terminal): {error}"
                        ));
                    }
                },
            }
        }

        if !launched_with_daemon {
            match terminal_backend::launch_backend(backend_kind, &cwd) {
                Ok(TerminalLaunch::Embedded(runtime)) => {
                    session.command = "embedded shell".to_owned();
                    session.generation = runtime.generation();
                    session.runtime = Some(TerminalRuntime::Embedded(runtime));
                    session.output = String::new();
                    session.styled_output = Vec::new();
                    session.cursor = None;
                    session.exit_code = None;
                    session.updated_at_unix_ms = current_unix_timestamp_millis();
                },
                Ok(TerminalLaunch::External(result)) => {
                    session.command = result.command;
                    session.output = trim_to_last_lines(result.output, 120);
                    session.styled_output = Vec::new();
                    session.cursor = None;
                    session.state = if result.success {
                        TerminalState::Completed
                    } else {
                        TerminalState::Failed
                    };
                    session.exit_code = result.code;
                    session.updated_at_unix_ms = current_unix_timestamp_millis();
                    if !result.success {
                        self.notice = Some(format!(
                            "terminal backend launch failed with code {:?}",
                            result.code,
                        ));
                    }
                },
                Err(error) => {
                    session.command = "launch backend".to_owned();
                    session.output = error.clone();
                    session.styled_output = Vec::new();
                    session.cursor = None;
                    session.state = TerminalState::Failed;
                    session.updated_at_unix_ms = current_unix_timestamp_millis();
                    self.notice = Some(format!("terminal session failed: {error}"));
                },
            }
        }

        self.terminals.push(session);
        true
    }

    fn spawn_terminal_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(outpost_index) = self.active_outpost_index {
            self.spawn_outpost_terminal(outpost_index, window, cx);
            return;
        }

        if !self.spawn_terminal_session_inner(true) {
            cx.notify();
            return;
        }

        self.sync_daemon_session_store(cx);
        self.active_diff_session_id = None;
        self.terminal_scroll_handle.scroll_to_bottom();
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        cx.notify();
    }

    fn spawn_outpost_terminal(
        &mut self,
        outpost_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(outpost) = self.outposts.get(outpost_index) else {
            return;
        };

        let host = self
            .remote_hosts
            .iter()
            .find(|host| host.name == outpost.host_name)
            .cloned();
        let Some(host) = host else {
            self.notice = Some(format!(
                "no remote host config found for `{}`",
                outpost.host_name,
            ));
            cx.notify();
            return;
        };

        let worktree_path = outpost.repo_root.clone();
        let session_id = self.next_terminal_id;
        self.next_terminal_id += 1;
        self.active_terminal_by_worktree
            .insert(worktree_path.clone(), session_id);

        let title = format!("ssh-{}", outpost.label);
        let mut session = TerminalSession {
            id: session_id,
            daemon_session_id: session_id.to_string(),
            worktree_path,
            title,
            last_command: None,
            pending_command: String::new(),
            command: String::new(),
            state: TerminalState::Running,
            exit_code: None,
            updated_at_unix_ms: current_unix_timestamp_millis(),
            cols: 120,
            rows: 35,
            generation: 0,
            output: String::new(),
            styled_output: Vec::new(),
            cursor: None,
            runtime: None,
        };

        let mut launched = false;

        if host.mosh == Some(true) && arbor_mosh::detect::local_mosh_client_available() {
            let pool = self.ssh_connection_pool.clone();
            match pool.get_or_connect(&host) {
                Ok(conn_slot) => {
                    let mosh_result: Result<arbor_mosh::MoshShell, String> = (|| {
                        let guard = conn_slot
                            .lock()
                            .map_err(|_| "SSH connection lock poisoned".to_owned())?;
                        let connection = guard
                            .as_ref()
                            .ok_or_else(|| "SSH connection not available".to_owned())?;
                        let handshake = arbor_mosh::handshake::start_mosh_server(connection, &host)
                            .map_err(|error| {
                                format!("mosh handshake failed, falling back to SSH: {error}")
                            })?;
                        arbor_mosh::MoshShell::spawn(handshake, 120, 35).map_err(|error| {
                            format!("mosh-client failed, falling back to SSH: {error}")
                        })
                    })(
                    );

                    match mosh_result {
                        Ok(mosh) => {
                            session.command = "mosh".to_owned();
                            session.generation = mosh.generation();
                            session.runtime = Some(TerminalRuntime::Outpost(
                                OutpostTerminalRuntime::MoshShell(mosh),
                            ));
                            launched = true;
                        },
                        Err(error) => {
                            self.notice = Some(error);
                        },
                    }
                },
                Err(error) => {
                    self.notice =
                        Some(format!("SSH connection failed for mosh handshake: {error}",));
                },
            }
        } else if host.mosh == Some(true) {
            self.notice =
                Some("mosh-client not found locally, falling back to SSH shell".to_owned());
        }

        if !launched {
            let pool = self.ssh_connection_pool.clone();
            match pool.get_or_connect(&host) {
                Ok(conn_slot) => {
                    let ssh_result: Result<SshTerminalShell, String> = (|| {
                        let guard = conn_slot
                            .lock()
                            .map_err(|_| "SSH connection lock poisoned".to_owned())?;
                        let connection = guard
                            .as_ref()
                            .ok_or_else(|| "SSH connection not available".to_owned())?;
                        SshTerminalShell::open(connection, 120, 35, &outpost.remote_path)
                    })();

                    match ssh_result {
                        Ok(ssh_shell) => {
                            session.command = "ssh".to_owned();
                            session.generation = ssh_shell.generation();
                            session.runtime = Some(TerminalRuntime::Outpost(
                                OutpostTerminalRuntime::SshShell(ssh_shell),
                            ));
                            launched = true;
                        },
                        Err(error) => {
                            self.notice = Some(format!("SSH shell failed: {error}"));
                        },
                    }
                },
                Err(error) => {
                    self.notice = Some(format!("SSH connection failed: {error}"));
                },
            }
        }

        if !launched {
            // Don't push a terminal session with no runtime
            self.notice
                .get_or_insert_with(|| "failed to open SSH shell".to_owned());
            cx.notify();
            return;
        }

        self.terminals.push(session);
        self.sync_daemon_session_store(cx);
        self.active_diff_session_id = None;
        self.terminal_scroll_handle.scroll_to_bottom();
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        cx.notify();
    }

    fn select_terminal(&mut self, session_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            return;
        };

        if self.active_center_tab_for_selected_worktree() == Some(CenterTab::Terminal(session_id)) {
            window.focus(&self.terminal_focus);
            self.focus_terminal_on_next_render = false;
            return;
        }

        self.active_terminal_by_worktree
            .insert(worktree_path, session_id);
        self.active_diff_session_id = None;
        self.logs_tab_active = false;
        self.terminal_scroll_handle.scroll_to_bottom();
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        cx.notify();
    }

    fn scroll_diff_to_file(&self, file_path: &Path) -> bool {
        let Some(worktree_path) = self.selected_worktree_path() else {
            return false;
        };
        let Some(active_diff_id) = self.active_diff_session_id else {
            return false;
        };
        let Some(session) = self.diff_sessions.iter().find(|session| {
            session.id == active_diff_id && session.worktree_path.as_path() == worktree_path
        }) else {
            return false;
        };
        let Some(row_index) = session.file_row_indices.get(file_path) else {
            return false;
        };

        self.diff_scroll_handle
            .scroll_to_item_strict(*row_index, ScrollStrategy::Top);
        true
    }

    fn open_diff_tab_for_selected_file(&mut self, cx: &mut Context<Self>) {
        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a worktree before opening a diff".to_owned());
            return;
        };
        tracing::info!(worktree = %worktree_path.display(), "opening diff tab");
        let Some(selected_file_path) = self
            .selected_changed_file()
            .map(|change| change.path.clone())
            .or_else(|| self.changed_files.first().map(|change| change.path.clone()))
        else {
            self.notice = Some("select a changed file before opening a diff".to_owned());
            return;
        };

        let changed_files = self.changed_files.clone();
        let (session_id, should_rebuild) = match self
            .diff_sessions
            .iter_mut()
            .find(|session| session.worktree_path == worktree_path)
        {
            Some(existing) => {
                self.active_diff_session_id = Some(existing.id);
                (
                    existing.id,
                    !existing.is_loading
                        && (existing.lines.is_empty()
                            || !existing.file_row_indices.contains_key(&selected_file_path)),
                )
            },
            None => {
                let session_id = self.next_diff_session_id;
                self.next_diff_session_id = self.next_diff_session_id.saturating_add(1);
                self.diff_sessions.push(DiffSession {
                    id: session_id,
                    worktree_path: worktree_path.clone(),
                    title: "Diff".to_owned(),
                    raw_lines: Arc::<[DiffLine]>::from(Vec::<DiffLine>::new()),
                    raw_file_row_indices: HashMap::new(),
                    lines: Arc::<[DiffLine]>::from(Vec::<DiffLine>::new()),
                    file_row_indices: HashMap::new(),
                    wrapped_columns: 0,
                    is_loading: true,
                });
                self.active_diff_session_id = Some(session_id);
                (session_id, true)
            },
        };

        self.pending_diff_scroll_to_file = Some(selected_file_path.clone());
        if !should_rebuild {
            let _ = self.scroll_diff_to_file(selected_file_path.as_path());
            cx.notify();
            return;
        }

        if let Some(session) = self
            .diff_sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.is_loading = true;
            session.raw_lines = Arc::<[DiffLine]>::from(Vec::<DiffLine>::new());
            session.raw_file_row_indices.clear();
            session.lines = Arc::<[DiffLine]>::from(Vec::<DiffLine>::new());
            session.file_row_indices.clear();
            session.wrapped_columns = 0;
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            let diff_document = cx
                .background_spawn(async move {
                    build_worktree_diff_document(&worktree_path, &changed_files)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let cell_width = diff_cell_width_px(cx);
                let wrap_columns = this
                    .live_diff_list_width_px()
                    .map(|list_width| {
                        this.estimated_diff_wrap_columns_for_list_width(list_width, cell_width)
                    })
                    .unwrap_or_else(|| this.estimated_diff_wrap_columns(cell_width));
                let Some(session) = this
                    .diff_sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                else {
                    return;
                };

                match diff_document {
                    Ok((lines, file_row_indices)) => {
                        let raw_lines = Arc::<[DiffLine]>::from(lines);
                        let raw_file_row_indices = file_row_indices;
                        let (wrapped_lines, wrapped_indices) = wrap_diff_document_lines(
                            raw_lines.as_ref(),
                            &raw_file_row_indices,
                            wrap_columns,
                        );
                        session.raw_lines = raw_lines;
                        session.raw_file_row_indices = raw_file_row_indices;
                        session.lines = Arc::<[DiffLine]>::from(wrapped_lines);
                        session.file_row_indices = wrapped_indices;
                        session.wrapped_columns = wrap_columns;
                        session.is_loading = false;

                        if let Some(target_path) = this.pending_diff_scroll_to_file.clone()
                            && this.scroll_diff_to_file(target_path.as_path())
                        {
                            this.pending_diff_scroll_to_file = None;
                        }
                    },
                    Err(error) => {
                        let fallback_lines = Arc::<[DiffLine]>::from(vec![DiffLine {
                            left_line_number: None,
                            right_line_number: None,
                            left_text: format!("failed to build diff: {error}"),
                            right_text: String::new(),
                            kind: DiffLineKind::FileHeader,
                        }]);
                        let fallback_indices = HashMap::new();
                        let (wrapped_lines, wrapped_indices) = wrap_diff_document_lines(
                            fallback_lines.as_ref(),
                            &fallback_indices,
                            wrap_columns,
                        );
                        session.raw_lines = fallback_lines;
                        session.raw_file_row_indices = fallback_indices;
                        session.lines = Arc::<[DiffLine]>::from(wrapped_lines);
                        session.file_row_indices = wrapped_indices;
                        session.wrapped_columns = wrap_columns;
                        session.is_loading = false;
                        this.notice = Some(format!("failed to build diff: {error}"));
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_diff_tab(&mut self, session_id: u64, cx: &mut Context<Self>) {
        if self.active_diff_session_id == Some(session_id) && !self.logs_tab_active {
            return;
        }
        self.active_diff_session_id = Some(session_id);
        self.logs_tab_active = false;
        if let Some(selected_path) = self.selected_changed_file.clone()
            && !self.scroll_diff_to_file(selected_path.as_path())
        {
            self.pending_diff_scroll_to_file = Some(selected_path);
        }
        cx.notify();
    }

    fn active_terminal(&self) -> Option<&TerminalSession> {
        let worktree_path = self.selected_worktree_path()?;
        let session_id = self.active_terminal_id_for_worktree(worktree_path)?;

        self.terminals.iter().find(|session| {
            session.id == session_id && session.worktree_path.as_path() == worktree_path
        })
    }

    fn write_input_to_terminal(&mut self, session_id: u64, input: &[u8]) -> Result<(), String> {
        if input.is_empty() {
            return Ok(());
        }

        let Some(index) = self
            .terminals
            .iter()
            .position(|session| session.id == session_id)
        else {
            return Ok(());
        };

        let runtime = self.terminals[index].runtime.clone();
        match runtime {
            Some(TerminalRuntime::Embedded(runtime)) => runtime.write_input(input),
            Some(TerminalRuntime::Daemon) => {
                let Some(daemon) = self.terminal_daemon.as_ref() else {
                    return Err("terminal daemon is not configured".to_owned());
                };
                let daemon_session_id = self.terminals[index].daemon_session_id.clone();
                if input == [0x03] {
                    daemon
                        .signal(SignalRequest {
                            session_id: daemon_session_id,
                            signal: TerminalSignal::Interrupt,
                        })
                        .map_err(|error| error.to_string())?;
                } else {
                    daemon
                        .write(WriteRequest {
                            session_id: daemon_session_id,
                            bytes: input.to_vec(),
                        })
                        .map_err(|error| error.to_string())?;
                }
                self.terminals[index].updated_at_unix_ms = current_unix_timestamp_millis();
                Ok(())
            },
            Some(TerminalRuntime::Outpost(OutpostTerminalRuntime::RemoteDaemon { ref daemon })) => {
                let daemon_session_id = self.terminals[index].daemon_session_id.clone();
                if input == [0x03] {
                    daemon
                        .signal(SignalRequest {
                            session_id: daemon_session_id,
                            signal: TerminalSignal::Interrupt,
                        })
                        .map_err(|error| error.to_string())?;
                } else {
                    daemon
                        .write(WriteRequest {
                            session_id: daemon_session_id,
                            bytes: input.to_vec(),
                        })
                        .map_err(|error| error.to_string())?;
                }
                self.terminals[index].updated_at_unix_ms = current_unix_timestamp_millis();
                Ok(())
            },
            Some(TerminalRuntime::Outpost(OutpostTerminalRuntime::SshShell(ref ssh))) => {
                ssh.write_input(input)?;
                self.terminals[index].updated_at_unix_ms = current_unix_timestamp_millis();
                Ok(())
            },
            Some(TerminalRuntime::Outpost(OutpostTerminalRuntime::MoshShell(ref mosh))) => {
                mosh.write_input(input)?;
                self.terminals[index].updated_at_unix_ms = current_unix_timestamp_millis();
                Ok(())
            },
            None => Ok(()),
        }
    }

    fn clear_terminal_selection(&mut self) {
        self.terminal_selection = None;
        self.terminal_selection_drag_anchor = None;
    }

    fn clear_terminal_selection_for_session(&mut self, session_id: u64) {
        if self
            .terminal_selection
            .as_ref()
            .is_some_and(|selection| selection.session_id == session_id)
        {
            self.clear_terminal_selection();
        }
    }

    fn terminal_display_lines_for_session(&self, session_id: u64) -> Vec<String> {
        let Some(session) = self
            .terminals
            .iter()
            .find(|session| session.id == session_id)
        else {
            return vec![String::new()];
        };

        terminal_display_lines(session)
    }

    fn terminal_selection_for_session(&self, session_id: u64) -> Option<&TerminalSelection> {
        self.terminal_selection
            .as_ref()
            .filter(|selection| selection.session_id == session_id)
    }

    fn handle_terminal_output_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.button != MouseButton::Left {
            return;
        }

        let Some(session_id) = self.active_terminal_id_for_selected_worktree() else {
            return;
        };

        let lines = self.terminal_display_lines_for_session(session_id);
        let line_height = terminal_line_height_px(cx);
        let cell_width = terminal_cell_width_px(cx);
        let point = terminal_grid_position_from_pointer(
            event.position,
            self.terminal_scroll_handle.bounds(),
            self.terminal_scroll_handle.offset(),
            line_height,
            cell_width,
            lines.len(),
        );

        let Some(point) = point else {
            return;
        };

        if event.click_count >= 3 {
            if let Some((start, end)) = terminal_line_bounds(&lines, point) {
                self.terminal_selection = Some(TerminalSelection {
                    session_id,
                    anchor: start,
                    head: end,
                });
            } else {
                self.clear_terminal_selection_for_session(session_id);
            }
            self.terminal_selection_drag_anchor = None;
        } else if event.click_count == 2 {
            if let Some((start, end)) = terminal_token_bounds(&lines, point) {
                self.terminal_selection = Some(TerminalSelection {
                    session_id,
                    anchor: start,
                    head: end,
                });
            } else {
                self.clear_terminal_selection_for_session(session_id);
            }
            self.terminal_selection_drag_anchor = None;
        } else {
            self.terminal_selection = Some(TerminalSelection {
                session_id,
                anchor: point,
                head: point,
            });
            self.terminal_selection_drag_anchor = Some(point);
        }

        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        cx.notify();
    }

    fn handle_terminal_output_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }

        let Some(session_id) = self.active_terminal_id_for_selected_worktree() else {
            return;
        };
        let Some(anchor) = self.terminal_selection_drag_anchor else {
            return;
        };

        let lines = self.terminal_display_lines_for_session(session_id);
        let line_height = terminal_line_height_px(cx);
        let cell_width = terminal_cell_width_px(cx);
        let Some(head) = terminal_grid_position_from_pointer(
            event.position,
            self.terminal_scroll_handle.bounds(),
            self.terminal_scroll_handle.offset(),
            line_height,
            cell_width,
            lines.len(),
        ) else {
            return;
        };

        self.terminal_selection = Some(TerminalSelection {
            session_id,
            anchor,
            head,
        });
        cx.notify();
    }

    fn handle_terminal_output_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if event.button == MouseButton::Left {
            self.terminal_selection_drag_anchor = None;
        }
    }

    fn track_terminal_command_input(&mut self, session_id: u64, keystroke: &Keystroke) {
        let Some(session) = self
            .terminals
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            return;
        };

        if keystroke.modifiers.platform {
            return;
        }

        if keystroke.modifiers.control {
            if keystroke.key.eq_ignore_ascii_case("u") {
                session.pending_command.clear();
            }
            return;
        }

        if keystroke.modifiers.alt {
            return;
        }

        match keystroke.key.as_str() {
            "enter" | "return" => {
                let command = session.pending_command.trim();
                if !command.is_empty() {
                    session.last_command = Some(command.to_owned());
                }
                session.pending_command.clear();
            },
            "backspace" => {
                session.pending_command.pop();
            },
            "tab" => session.pending_command.push('\t'),
            "space" => session.pending_command.push(' '),
            _ => {
                if let Some(key_char) = keystroke.key_char.as_ref() {
                    session.pending_command.push_str(key_char);
                } else if keystroke.key.len() == 1 {
                    session.pending_command.push_str(&keystroke.key);
                }
            },
        }
    }

    fn copy_terminal_content_to_clipboard(&mut self, session_id: u64, cx: &mut Context<Self>) {
        let Some(session) = self
            .terminals
            .iter()
            .find(|session| session.id == session_id)
        else {
            return;
        };

        let clipboard_text =
            if let Some(selection) = self.terminal_selection_for_session(session_id) {
                let selected = terminal_selection_text(
                    &self.terminal_display_lines_for_session(session_id),
                    selection,
                );
                if !selected.is_empty() {
                    selected
                } else if !session.pending_command.trim().is_empty() {
                    session.pending_command.clone()
                } else {
                    session.output.clone()
                }
            } else if !session.pending_command.trim().is_empty() {
                session.pending_command.clone()
            } else {
                session.output.clone()
            };
        if clipboard_text.is_empty() {
            return;
        }

        cx.write_to_clipboard(ClipboardItem::new_string(clipboard_text));
    }

    fn append_pasted_text_to_pending_command(&mut self, session_id: u64, text: &str) {
        let Some(session) = self
            .terminals
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            return;
        };

        session.pending_command.push_str(text);
    }

    fn paste_clipboard_into_terminal(&mut self, session_id: u64, cx: &mut Context<Self>) {
        let Some(clipboard_item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = clipboard_item.text() else {
            return;
        };
        if text.is_empty() {
            return;
        }

        self.append_pasted_text_to_pending_command(session_id, &text);
        if let Err(error) = self.write_input_to_terminal(session_id, text.as_bytes()) {
            self.notice = Some(format!("failed to paste into terminal: {error}"));
        }
    }

    fn handle_terminal_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.create_modal.is_some() || self.manage_hosts_modal.is_some() {
            return;
        }

        let Some(CenterTab::Terminal(active_terminal_id)) =
            self.active_center_tab_for_selected_worktree()
        else {
            return;
        };

        if let Some(command) = terminal_keys::platform_command_for_keystroke(&event.keystroke) {
            match command {
                terminal_keys::TerminalPlatformCommand::Copy => {
                    self.copy_terminal_content_to_clipboard(active_terminal_id, cx);
                },
                terminal_keys::TerminalPlatformCommand::Paste => {
                    self.paste_clipboard_into_terminal(active_terminal_id, cx);
                },
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }

        self.clear_terminal_selection_for_session(active_terminal_id);
        self.track_terminal_command_input(active_terminal_id, &event.keystroke);

        let Some(input) = terminal_keys::terminal_bytes_from_keystroke(&event.keystroke) else {
            return;
        };

        if let Err(error) = self.write_input_to_terminal(active_terminal_id, &input) {
            self.notice = Some(format!("failed to write to terminal: {error}"));
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn focus_terminal_panel(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
    }

    fn clamp_pane_widths_for_workspace(&mut self, workspace_width: f32) {
        let available_side_width =
            (workspace_width - (2. * PANE_RESIZE_HANDLE_WIDTH) - PANE_CENTER_MIN_WIDTH).max(0.);

        self.left_pane_width = self
            .left_pane_width
            .clamp(LEFT_PANE_MIN_WIDTH, LEFT_PANE_MAX_WIDTH);
        self.right_pane_width = self
            .right_pane_width
            .clamp(RIGHT_PANE_MIN_WIDTH, RIGHT_PANE_MAX_WIDTH);

        let side_total = self.left_pane_width + self.right_pane_width;
        if side_total <= available_side_width {
            return;
        }

        let mut overflow = side_total - available_side_width;

        let right_reducible = (self.right_pane_width - RIGHT_PANE_MIN_WIDTH).max(0.);
        let right_reduction = overflow.min(right_reducible);
        self.right_pane_width -= right_reduction;
        overflow -= right_reduction;

        if overflow <= 0. {
            return;
        }

        let left_reducible = (self.left_pane_width - LEFT_PANE_MIN_WIDTH).max(0.);
        let left_reduction = overflow.min(left_reducible);
        self.left_pane_width -= left_reduction;
    }

    fn estimated_diff_wrap_columns(&self, cell_width_px: f32) -> usize {
        let fallback_window_width = self.left_pane_width
            + self.right_pane_width
            + PANE_CENTER_MIN_WIDTH
            + (2. * PANE_RESIZE_HANDLE_WIDTH);
        let window_width = self
            .last_persisted_ui_state
            .window
            .map(|window| window.width as f32)
            .unwrap_or(fallback_window_width)
            .max(600.);
        self.estimated_diff_wrap_columns_for_window_width(window_width, cell_width_px)
    }

    fn estimated_diff_wrap_columns_for_window_width(
        &self,
        window_width: f32,
        cell_width_px: f32,
    ) -> usize {
        let center_width = (window_width
            - self.left_pane_width
            - self.right_pane_width
            - (2. * PANE_RESIZE_HANDLE_WIDTH))
            .max(PANE_CENTER_MIN_WIDTH);
        let list_width =
            (center_width - DIFF_ZONEMAP_WIDTH_PX - (DIFF_ZONEMAP_MARGIN_PX * 2.)).max(80.);
        self.estimated_diff_wrap_columns_for_list_width(list_width, cell_width_px)
    }

    fn estimated_diff_wrap_columns_for_list_width(
        &self,
        list_width: f32,
        cell_width_px: f32,
    ) -> usize {
        let column_width = (list_width / 2.).max(40.);
        let safe_cell_width = cell_width_px.max(1.);
        let line_number_width = (DIFF_LINE_NUMBER_WIDTH_CHARS as f32 * safe_cell_width) + 12.;
        let marker_width = 10.;
        let horizontal_padding = 16.;
        let horizontal_gaps = 16.;
        let text_width = (column_width
            - line_number_width
            - marker_width
            - horizontal_padding
            - horizontal_gaps)
            .max(safe_cell_width);
        let estimated_columns = (text_width / safe_cell_width).floor() as usize;
        estimated_columns.saturating_add(2).clamp(12, 320)
    }

    fn live_diff_list_width_px(&self) -> Option<f32> {
        let width = self
            .diff_scroll_handle
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .width
            .to_f64() as f32;
        (width.is_finite() && width >= 80.).then_some(width)
    }

    fn rewrap_diff_sessions_if_needed(&mut self, wrap_columns: usize) {
        for session in &mut self.diff_sessions {
            if session.is_loading
                || session.raw_lines.is_empty()
                || session.wrapped_columns == wrap_columns
            {
                continue;
            }

            let (wrapped_lines, wrapped_indices) = wrap_diff_document_lines(
                session.raw_lines.as_ref(),
                &session.raw_file_row_indices,
                wrap_columns,
            );
            session.lines = Arc::<[DiffLine]>::from(wrapped_lines);
            session.file_row_indices = wrapped_indices;
            session.wrapped_columns = wrap_columns;
        }
    }

    fn ui_state_snapshot(&self, window: &Window) -> ui_state_store::UiState {
        let bounds = window.window_bounds().get_bounds();
        let x = f32::from(bounds.origin.x).round() as i32;
        let y = f32::from(bounds.origin.y).round() as i32;
        let width = f32::from(bounds.size.width).round().max(1.) as u32;
        let height = f32::from(bounds.size.height).round().max(1.) as u32;

        ui_state_store::UiState {
            left_pane_width: Some(self.left_pane_width.round() as i32),
            right_pane_width: Some(self.right_pane_width.round() as i32),
            window: Some(ui_state_store::WindowGeometry {
                x,
                y,
                width,
                height,
            }),
            left_pane_visible: Some(self.left_pane_visible),
        }
    }

    fn sync_ui_state_store(&mut self, window: &Window) {
        let next_state = self.ui_state_snapshot(window);
        if self.last_persisted_ui_state == next_state {
            return;
        }

        match self.ui_state_store.save(&next_state) {
            Ok(()) => {
                self.last_persisted_ui_state = next_state;
                self.last_ui_state_error = None;
            },
            Err(error) => {
                if self.last_ui_state_error.as_deref() != Some(error.as_str()) {
                    self.notice = Some(format!("failed to persist UI state: {error}"));
                    self.last_ui_state_error = Some(error);
                }
            },
        }
    }

    fn handle_pane_divider_drag_move(
        &mut self,
        event: &DragMoveEvent<DraggedPaneDivider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace_width = f32::from(event.bounds.size.width);
        let available_side_width =
            (workspace_width - (2. * PANE_RESIZE_HANDLE_WIDTH) - PANE_CENTER_MIN_WIDTH).max(0.);

        match event.drag(cx) {
            DraggedPaneDivider::Left => {
                let proposed = f32::from(event.event.position.x - event.bounds.left());
                let max_width =
                    (available_side_width - self.right_pane_width).min(LEFT_PANE_MAX_WIDTH);
                let min_width = LEFT_PANE_MIN_WIDTH.min(max_width);
                self.left_pane_width = proposed.clamp(min_width, max_width);
            },
            DraggedPaneDivider::Right => {
                let proposed = f32::from(event.bounds.right() - event.event.position.x);
                let max_width =
                    (available_side_width - self.left_pane_width).min(RIGHT_PANE_MAX_WIDTH);
                let min_width = RIGHT_PANE_MIN_WIDTH.min(max_width);
                self.right_pane_width = proposed.clamp(min_width, max_width);
            },
        }

        self.clamp_pane_widths_for_workspace(workspace_width);
        self.sync_ui_state_store(window);
        cx.stop_propagation();
        cx.notify();
    }

    fn render_pane_resize_handle(
        &self,
        id: &'static str,
        divider: DraggedPaneDivider,
        theme: ThemePalette,
    ) -> impl IntoElement {
        div()
            .id(id)
            .w(px(PANE_RESIZE_HANDLE_WIDTH))
            .h_full()
            .flex_none()
            .cursor_col_resize()
            .on_drag(divider, |dragged_divider, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| *dragged_divider)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(div().w(px(1.)).h_full().mx_auto().bg(rgb(theme.border)))
            .occlude()
    }

    fn render_top_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let repository = self.selected_repository_label();
        let branch = self
            .active_worktree()
            .map(|worktree| worktree.branch.clone())
            .unwrap_or_else(|| "no-worktree".to_owned());
        let centered_title = format!("{repository} · {branch}");
        let back_enabled = !self.worktree_nav_back.is_empty();
        let forward_enabled = !self.worktree_nav_forward.is_empty();
        let sidebar_hidden = !self.left_pane_visible;

        div()
            .h(px(TITLEBAR_HEIGHT))
            .bg(rgb(theme.chrome_bg))
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .flex()
            .items_center()
            // Left group: sidebar toggle + back/forward navigation (offset to clear macOS traffic lights)
            .child(
                div()
                    .absolute()
                    .left(px(76.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .child(
                        div()
                            .id("toggle-sidebar")
                            .cursor_pointer()
                            .font_family(FONT_MONO)
                            .text_size(px(20.))
                            .text_color(rgb(if sidebar_hidden {
                                theme.accent
                            } else {
                                theme.text_muted
                            }))
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.action_toggle_left_pane(&ToggleLeftPane, window, cx);
                            }))
                            .child("\u{f0c9}"),
                    )
                    .child(
                        div()
                            .id("nav-back")
                            .cursor_pointer()
                            .font_family(FONT_MONO)
                            .text_size(px(20.))
                            .text_color(rgb(if back_enabled {
                                theme.text_primary
                            } else {
                                theme.text_disabled
                            }))
                            .when(back_enabled, |this| {
                                this.on_click(cx.listener(|this, _, window, cx| {
                                    this.action_navigate_worktree_back(
                                        &NavigateWorktreeBack,
                                        window,
                                        cx,
                                    );
                                }))
                            })
                            .child("\u{f053}"),
                    )
                    .child(
                        div()
                            .id("nav-forward")
                            .cursor_pointer()
                            .font_family(FONT_MONO)
                            .text_size(px(20.))
                            .text_color(rgb(if forward_enabled {
                                theme.text_primary
                            } else {
                                theme.text_disabled
                            }))
                            .when(forward_enabled, |this| {
                                this.on_click(cx.listener(|this, _, window, cx| {
                                    this.action_navigate_worktree_forward(
                                        &NavigateWorktreeForward,
                                        window,
                                        cx,
                                    );
                                }))
                            })
                            .child("\u{f054}"),
                    ),
            )
            // Center: title
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child(centered_title),
                    ),
            )
    }

    fn render_left_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.left_pane_visible {
            let theme = self.theme();
            let repositories = self.repositories.clone();
            let worktrees = self.worktrees.clone();
            let mut pane = div()
                .id("collapsed-left-pane")
                .w(px(40.))
                .h_full()
                .flex_none()
                .bg(rgb(theme.sidebar_bg))
                .flex()
                .flex_col()
                .items_center()
                .pt_2()
                .gap_1()
                .overflow_y_scroll();

            for (repo_index, repository) in repositories.iter().enumerate() {
                let repo_worktrees: Vec<(usize, &WorktreeSummary)> = worktrees
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| w.repo_root == repository.root)
                    .collect();

                // Add spacing between repo groups (not before the first)
                if repo_index > 0 {
                    pane = pane.child(div().h(px(4.)));
                }

                // Repo icon row: circular avatar or GitHub icon
                let repo_icon = if let Some(url) = repository.avatar_url.clone() {
                    div()
                        .size(px(32.))
                        .rounded_md()
                        .overflow_hidden()
                        .child(img(url).size_full().rounded_md().with_fallback(move || {
                            div()
                                .size_full()
                                .font_family(FONT_MONO)
                                .text_size(px(14.))
                                .text_color(rgb(theme.text_muted))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child("\u{f09b}")
                                .into_any_element()
                        }))
                        .into_any_element()
                } else {
                    div()
                        .size(px(24.))
                        .font_family(FONT_MONO)
                        .text_size(px(14.))
                        .text_color(rgb(theme.text_muted))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("\u{f09b}")
                        .into_any_element()
                };
                pane = pane.child(repo_icon);

                for (wt_index, worktree) in repo_worktrees {
                    let is_active = self.active_worktree_index == Some(wt_index);
                    let first_char: String = worktree
                        .label
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .collect();

                    pane = pane.child(
                        div()
                            .id(("collapsed-worktree", wt_index))
                            .cursor_pointer()
                            .size(px(30.))
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(if is_active {
                                theme.accent
                            } else {
                                theme.border
                            }))
                            .bg(rgb(if is_active {
                                theme.panel_active_bg
                            } else {
                                theme.panel_bg
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(if is_active {
                                theme.text_primary
                            } else {
                                theme.text_muted
                            }))
                            .child(first_char)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_worktree(wt_index, window, cx);
                            })),
                    );
                }
            }

            return pane;
        }
        let theme = self.theme();
        let repositories = self.repositories.clone();
        let worktrees = self.worktrees.clone();
        div()
            .id("left-pane")
            .w(px(self.left_pane_width))
            .h_full()
            .bg(rgb(theme.sidebar_bg))
            .flex()
            .flex_col()
            .child(
                div()
                    .id("worktrees-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pt_2()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(repositories.into_iter().enumerate().map(
                        |(repository_index, repository)| {
                            let is_active_repository =
                                self.active_repository_index == Some(repository_index);
                            let is_collapsed =
                                self.collapsed_repositories.contains(&repository_index);
                            let repository_avatar_url = repository.avatar_url.clone();
                            let repo_worktrees: Vec<(usize, WorktreeSummary)> = worktrees
                                .iter()
                                .cloned()
                                .enumerate()
                                .filter(|(_, worktree)| worktree.repo_root == repository.root)
                                .collect();
                            let repo_agent_dot_color = if is_collapsed {
                                if repo_worktrees
                                    .iter()
                                    .any(|(_, wt)| wt.agent_state == Some(AgentState::Working))
                                {
                                    Some(0xe5c07b_u32)
                                } else if repo_worktrees
                                    .iter()
                                    .any(|(_, wt)| wt.agent_state == Some(AgentState::Waiting))
                                {
                                    Some(0x61afef_u32)
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            let repo_outposts: Vec<(usize, OutpostSummary)> = self
                                .outposts
                                .iter()
                                .cloned()
                                .enumerate()
                                .filter(|(_, outpost)| outpost.repo_root == repository.root)
                                .collect();

                            div()
                                .id(("repository-group", repository_index))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("repository-row", repository_index))
                                        .cursor_pointer()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(if is_active_repository {
                                            theme.accent
                                        } else {
                                            theme.border
                                        }))
                                        .bg(rgb(if is_active_repository {
                                            theme.panel_active_bg
                                        } else {
                                            theme.panel_bg
                                        }))
                                        .px_2()
                                        .py_1()
                                        .h(px(32.))
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_repository(repository_index, cx);
                                        }))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                // Chevron toggle
                                                .child(
                                                    div()
                                                        .id(("repo-chevron", repository_index))
                                                        .cursor_pointer()
                                                        .text_size(px(16.))
                                                        .text_color(rgb(theme.text_muted))
                                                        .w(px(14.))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .child(if is_collapsed {
                                                            "\u{25B8}"
                                                        } else {
                                                            "\u{25BE}"
                                                        })
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                if this
                                                                    .collapsed_repositories
                                                                    .contains(&repository_index)
                                                                {
                                                                    this.collapsed_repositories
                                                                        .remove(&repository_index);
                                                                } else {
                                                                    this.collapsed_repositories
                                                                        .insert(repository_index);
                                                                }
                                                                cx.stop_propagation();
                                                                cx.notify();
                                                            },
                                                        )),
                                                )
                                                // GitHub icon or avatar
                                                .child(
                                                    if let Some(url) =
                                                        repository_avatar_url.clone()
                                                    {
                                                        div()
                                                            .size(px(20.))
                                                            .rounded_sm()
                                                            .overflow_hidden()
                                                            .child(
                                                                img(url)
                                                                    .size_full()
                                                                    .rounded_sm()
                                                                    .with_fallback(move || {
                                                                        div()
                                                                            .size_full()
                                                                            .font_family(FONT_MONO)
                                                                            .text_size(px(12.))
                                                                            .text_color(rgb(
                                                                                theme.text_muted,
                                                                            ))
                                                                            .flex()
                                                                            .items_center()
                                                                            .justify_center()
                                                                            .child("\u{f09b}")
                                                                            .into_any_element()
                                                                    }),
                                                            )
                                                            .into_any_element()
                                                    } else {
                                                        div()
                                                            .font_family(FONT_MONO)
                                                            .text_size(px(12.))
                                                            .text_color(rgb(theme.text_muted))
                                                            .child("\u{f09b}")
                                                            .into_any_element()
                                                    },
                                                )
                                                // Repository name
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .text_ellipsis()
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(theme.text_primary))
                                                        .child(repository.label.clone()),
                                                )
                                                // Worktree count badge
                                                .child(
                                                    div()
                                                        .text_size(px(9.))
                                                        .text_color(rgb(theme.text_disabled))
                                                        .child(format!(
                                                            "{}",
                                                            repo_worktrees.len()
                                                        )),
                                                ),
                                        )
                                        .when_some(repo_agent_dot_color, |this, color| {
                                            this.child(
                                                div()
                                                    .flex_none()
                                                    .size(px(6.))
                                                    .rounded_full()
                                                    .bg(rgb(color)),
                                            )
                                        })
                                        .child(
                                            div()
                                                .id(("repository-add-worktree", repository_index))
                                                .size(px(18.))
                                                .rounded_sm()
                                                .cursor_pointer()
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(theme.text_muted))
                                                .child("+")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if this.active_repository_index
                                                        != Some(repository_index)
                                                    {
                                                        this.select_repository(repository_index, cx);
                                                    }
                                                    this.open_create_modal(
                                                        repository_index,
                                                        CreateModalTab::LocalWorktree,
                                                        cx,
                                                    );
                                                    cx.stop_propagation();
                                                })),
                                        ),
                                )
                                .when(!is_collapsed, |this| {
                                    this.child(
                                    div()
                                        .pl(px(8.))
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .children(
                                            repo_worktrees.into_iter().map(|(index, worktree)| {
                                                let is_active =
                                                    self.active_worktree_index == Some(index);
                                                let diff_summary = worktree.diff_summary;
                                                let pr_number = worktree.pr_number;
                                                let pr_url = worktree.pr_url.clone();
                                                let show_name = worktree.label != worktree.branch;
                                                let agent_dot_color = match worktree.agent_state {
                                                    Some(AgentState::Working) => Some(0xe5c07b_u32),
                                                    Some(AgentState::Waiting) => Some(0x61afef_u32),
                                                    None => None,
                                                };
                                                let checkout_icon = if worktree.is_primary_checkout
                                                {
                                                    "◦"
                                                } else {
                                                    "⎇"
                                                };
                                                div()
                                                    .id(("worktree-row", index))
                                                    .font_family(FONT_MONO)
                                                    .cursor_pointer()
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(rgb(if is_active {
                                                        theme.accent
                                                    } else {
                                                        theme.border
                                                    }))
                                                    .bg(rgb(theme.panel_bg))
                                                    .px_2()
                                                    .py_1()
                                                    .h(px(40.))
                                                    .flex()
                                                    .flex_col()
                                                    .justify_center()
                                                    .when(is_active, |this| {
                                                        this.bg(rgb(theme.panel_active_bg))
                                                    })
                                                    .on_click(
                                                        cx.listener(move |this, _, window, cx| {
                                                            this.select_worktree(index, window, cx)
                                                        }),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .child(
                                                                div()
                                                                    .min_w_0()
                                                                    .flex_1()
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap_1()
                                                                    .child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(rgb(
                                                                                theme.text_muted,
                                                                            ))
                                                                            .child(checkout_icon),
                                                                    )
                                                                    .when_some(agent_dot_color, |this, color| {
                                                                        this.child(
                                                                            div()
                                                                                .flex_none()
                                                                                .size(px(6.))
                                                                                .rounded_full()
                                                                                .bg(rgb(color)),
                                                                        )
                                                                    })
                                                                    .child(
                                                                        div().min_w_0().flex_1().when(
                                                                            show_name,
                                                                            |this| {
                                                                                this.child(
                                                                                    div()
                                                                                        .min_w_0()
                                                                                        .overflow_hidden()
                                                                                        .whitespace_nowrap()
                                                                                        .text_ellipsis()
                                                                                        .text_xs()
                                                                                        .font_weight(
                                                                                            FontWeight::SEMIBOLD,
                                                                                        )
                                                                                        .text_color(rgb(
                                                                                            theme.text_primary,
                                                                                        ))
                                                                                        .child(
                                                                                            worktree
                                                                                                .label
                                                                                                .clone(),
                                                                                        ),
                                                                                )
                                                                            },
                                                                        ),
                                                                    ),
                                                    )
                                                    .child(
                                                        div()
                                                            .pl(px(14.))
                                                            .min_w_0()
                                                            .flex()
                                                            .items_center()
                                                            .justify_between()
                                                            .gap_2()
                                                            .child(
                                                                div()
                                                                    .min_w_0()
                                                                    .overflow_hidden()
                                                                    .whitespace_nowrap()
                                                                    .text_ellipsis()
                                                                    .text_xs()
                                                                    .text_color(rgb(theme.text_disabled))
                                                                    .child(worktree.branch.clone()),
                                                            )
                                                            .child({
                                                                let summary =
                                                                    diff_summary.unwrap_or_default();
                                                                let show_diff_summary =
                                                                    summary.additions > 0
                                                                        || summary.deletions > 0;
                                                                let mut details = div()
                                                                    .flex_none()
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_end()
                                                                    .gap_1();

                                                                if let Some(pr_number) = pr_number {
                                                                    let pr_text =
                                                                        format!("#{pr_number}");
                                                                    if let Some(pr_url) =
                                                                        pr_url.clone()
                                                                    {
                                                                        details = details.child(
                                                                            div()
                                                                                .id((
                                                                                    "worktree-pr-link",
                                                                                    index,
                                                                                ))
                                                                                .cursor_pointer()
                                                                                .text_xs()
                                                                                .text_color(rgb(
                                                                                    theme.accent,
                                                                                ))
                                                                                .child(pr_text)
                                                                                .on_click(
                                                                                    cx.listener(
                                                                                        move |this, _, _, cx| {
                                                                                            this.open_external_url(
                                                                                                &pr_url,
                                                                                                cx,
                                                                                            );
                                                                                            cx.stop_propagation();
                                                                                        },
                                                                                    ),
                                                                                ),
                                                                        );
                                                                    } else {
                                                                        details = details.child(
                                                                            div()
                                                                                .text_xs()
                                                                                .text_color(rgb(
                                                                                    theme.accent,
                                                                                ))
                                                                                .child(pr_text),
                                                                        );
                                                                    }
                                                                }

                                                                if self.worktree_stats_loading
                                                                    && diff_summary.is_none()
                                                                {
                                                                    details = details.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(rgb(
                                                                                theme.text_muted,
                                                                            ))
                                                                            .child("..."),
                                                                    );
                                                                } else if show_diff_summary {
                                                                    details = details
                                                                        .child(
                                                                            div()
                                                                                .text_xs()
                                                                                .text_color(rgb(
                                                                                    0x72d69c,
                                                                                ))
                                                                                .child(format!(
                                                                                    "+{}",
                                                                                    summary
                                                                                        .additions
                                                                                )),
                                                                        )
                                                                        .child(
                                                                            div()
                                                                                .text_xs()
                                                                                .text_color(rgb(
                                                                                    0xeb6f92,
                                                                                ))
                                                                                .child(format!(
                                                                                    "-{}",
                                                                                    summary
                                                                                        .deletions
                                                                                )),
                                                                        );
                                                                }

                                                                details
                                                            }),
                                                    )
                                                    )
                                            }),
                                        ),
                                )
                                })
                                .when(!repo_outposts.is_empty(), |group| {
                                    group.child(
                                        div()
                                            .pl(px(8.))
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .children(
                                                repo_outposts.into_iter().map(|(outpost_index, outpost)| {
                                                    let is_active = self.active_outpost_index == Some(outpost_index);
                                                    let status_color = match outpost.status {
                                                        arbor_core::outpost::OutpostStatus::Available => theme.accent,
                                                        arbor_core::outpost::OutpostStatus::Unreachable => 0xeb6f92,
                                                        arbor_core::outpost::OutpostStatus::NotCloned | arbor_core::outpost::OutpostStatus::Provisioning => theme.text_muted,
                                                    };
                                                    div()
                                                        .id(("outpost-row", outpost_index))
                                                        .font_family(FONT_MONO)
                                                        .cursor_pointer()
                                                        .rounded_sm()
                                                        .border_1()
                                                        .border_color(rgb(if is_active { theme.accent } else { theme.border }))
                                                        .bg(rgb(theme.panel_bg))
                                                        .px_2()
                                                        .py_1()
                                                        .h(px(40.))
                                                        .flex()
                                                        .flex_col()
                                                        .justify_center()
                                                        .when(is_active, |this| this.bg(rgb(theme.panel_active_bg)))
                                                        .on_click(cx.listener(move |this, _, window, cx| {
                                                            this.select_outpost(outpost_index, window, cx);
                                                        }))
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_1()
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(rgb(status_color))
                                                                        .child("\u{f0ac}"),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .flex_1()
                                                                        .overflow_hidden()
                                                                        .whitespace_nowrap()
                                                                        .text_ellipsis()
                                                                        .text_xs()
                                                                        .font_weight(FontWeight::SEMIBOLD)
                                                                        .text_color(rgb(theme.text_primary))
                                                                        .child(outpost.label.clone()),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .pl(px(14.))
                                                                .min_w_0()
                                                                .flex()
                                                                .items_center()
                                                                .justify_between()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .overflow_hidden()
                                                                        .whitespace_nowrap()
                                                                        .text_ellipsis()
                                                                        .text_xs()
                                                                        .text_color(rgb(theme.text_disabled))
                                                                        .child(outpost.branch.clone()),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .flex_none()
                                                                        .text_xs()
                                                                        .text_color(rgb(theme.text_muted))
                                                                        .child(format!("@{}", outpost.hostname)),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .id(("outpost-remove", outpost_index))
                                                                        .flex_none()
                                                                        .cursor_pointer()
                                                                        .text_xs()
                                                                        .text_color(rgb(theme.text_muted))
                                                                        .hover(|style| style.text_color(rgb(theme.text_primary)))
                                                                        .ml_1()
                                                                        .child("\u{f00d}")
                                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                                            this.remove_outpost(outpost_index, cx);
                                                                        })),
                                                                ),
                                                        )
                                                }),
                                            ),
                                    )
                                })
                        },
                    )),
            )
            .child(div().h(px(1.)).bg(rgb(theme.border)))
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        action_button(theme, "open-add-repository", "+ Add Repo", false, false)
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_add_repository_picker(cx);
                            })),
                    )
                    .child(
                        action_button(theme, "open-manage-hosts", "Manage Hosts", false, false)
                            .flex_1()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_manage_hosts_modal(cx);
                            })),
                    ),
            )
    }

    fn render_terminal_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cell_width = diff_cell_width_px(cx);
        let wrap_columns = if let Some(list_width) = self.live_diff_list_width_px() {
            self.estimated_diff_wrap_columns_for_list_width(list_width, cell_width)
        } else {
            let window_width = f32::from(window.window_bounds().get_bounds().size.width).max(600.);
            self.estimated_diff_wrap_columns_for_window_width(window_width, cell_width)
        };
        self.rewrap_diff_sessions_if_needed(wrap_columns);

        let theme = self.theme();
        let terminals = self.selected_worktree_terminals();
        let diff_sessions = self.selected_worktree_diff_sessions();
        let mut tabs: Vec<CenterTab> = terminals
            .iter()
            .map(|session| CenterTab::Terminal(session.id))
            .collect();
        tabs.extend(
            diff_sessions
                .iter()
                .map(|session| CenterTab::Diff(session.id)),
        );
        if self.logs_tab_open {
            tabs.push(CenterTab::Logs);
        }

        let mut active_tab = self.active_center_tab_for_selected_worktree();
        if active_tab.is_some_and(|tab| !tabs.contains(&tab)) {
            active_tab = None;
        }
        if active_tab.is_none() {
            active_tab = tabs.first().copied();
            self.active_diff_session_id = match active_tab {
                Some(CenterTab::Diff(diff_id)) => Some(diff_id),
                _ => None,
            };
        }

        let active_tab_index =
            active_tab.and_then(|tab| tabs.iter().position(|entry| *entry == tab));
        let active_terminal = match active_tab {
            Some(CenterTab::Terminal(session_id)) => self
                .terminals
                .iter()
                .find(|session| session.id == session_id)
                .cloned(),
            _ => None,
        };
        let active_diff_session = match active_tab {
            Some(CenterTab::Diff(diff_id)) => self
                .diff_sessions
                .iter()
                .find(|session| session.id == diff_id)
                .cloned(),
            _ => None,
        };

        div()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .bg(rgb(theme.terminal_bg))
            .border_l_1()
            .border_r_1()
            .border_color(rgb(theme.border))
            .flex()
            .flex_col()
            .track_focus(&self.terminal_focus)
            .on_any_mouse_down(cx.listener(Self::focus_terminal_panel))
            .on_key_down(cx.listener(Self::handle_terminal_key_down))
            .child(
                div()
                    .h(px(32.))
                    .bg(rgb(theme.tab_bg))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .h_full()
                            .flex_1()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .when(tabs.is_empty(), |this| {
                                this.child(
                                    div()
                                        .px_3()
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .child("No tabs"),
                                )
                            })
                            .children(tabs.iter().copied().enumerate().map(|(index, tab)| {
                                let is_active = active_tab == Some(tab);
                                let tab_count = tabs.len();
                                let relation =
                                    active_tab_index.map(|active_index| index.cmp(&active_index));
                                let (tab_icon, tab_label, terminal_icon) = match tab {
                                    CenterTab::Terminal(session_id) => (
                                        TAB_ICON_TERMINAL,
                                        self.terminals
                                            .iter()
                                            .find(|session| session.id == session_id)
                                            .map(terminal_tab_title)
                                            .unwrap_or_else(|| "terminal".to_owned()),
                                        true,
                                    ),
                                    CenterTab::Diff(diff_id) => (
                                        TAB_ICON_DIFF,
                                        self.diff_sessions
                                            .iter()
                                            .find(|session| session.id == diff_id)
                                            .map(diff_tab_title)
                                            .unwrap_or_else(|| "diff".to_owned()),
                                        false,
                                    ),
                                    CenterTab::Logs => (
                                        TAB_ICON_LOGS,
                                        "Logs".to_owned(),
                                        true,
                                    ),
                                };
                                let tab_id = match tab {
                                    CenterTab::Terminal(id) => ("center-tab-terminal", id),
                                    CenterTab::Diff(id) => ("center-tab-diff", id),
                                    CenterTab::Logs => ("center-tab-logs", 0),
                                };

                                div()
                                    .id(tab_id)
                                    .group("tab")
                                    .relative()
                                    .h_full()
                                    .cursor_pointer()
                                    .w(px(160.))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .border_color(rgb(theme.border))
                                    .bg(rgb(if is_active {
                                        theme.tab_active_bg
                                    } else {
                                        theme.tab_bg
                                    }))
                                    .child(
                                        div()
                                            .font_family(FONT_MONO)
                                            .when(terminal_icon, |this| this.text_size(px(24.)))
                                            .when(!terminal_icon, |this| this.text_xs())
                                            .text_color(rgb(if is_active {
                                                theme.text_primary
                                            } else {
                                                theme.text_muted
                                            }))
                                            .child(tab_icon),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(if is_active {
                                                theme.text_primary
                                            } else {
                                                theme.text_muted
                                            }))
                                            .child(tab_label),
                                    )
                                    .child(
                                        div()
                                            .id(match tab {
                                                CenterTab::Terminal(id) => ("tab-close-terminal", id),
                                                CenterTab::Diff(id) => ("tab-close-diff", id),
                                                CenterTab::Logs => ("tab-close-logs", 0),
                                            })
                                            .absolute()
                                            .right(px(4.))
                                            .top_0()
                                            .bottom_0()
                                            .w(px(24.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .font_family(FONT_MONO)
                                            .text_size(px(24.))
                                            .text_color(rgb(theme.text_muted))
                                            .invisible()
                                            .group_hover("tab", |s| s.visible())
                                            .child("×")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                                    cx.stop_propagation();
                                                    match tab {
                                                        CenterTab::Terminal(session_id) => {
                                                            if this.close_terminal_session_by_id(session_id) {
                                                                this.sync_daemon_session_store(cx);
                                                                this.terminal_scroll_handle.scroll_to_bottom();
                                                                window.focus(&this.terminal_focus);
                                                                this.focus_terminal_on_next_render = false;
                                                                cx.notify();
                                                            }
                                                        },
                                                        CenterTab::Diff(diff_id) => {
                                                            if this.close_diff_session_by_id(diff_id) {
                                                                cx.notify();
                                                            }
                                                        },
                                                        CenterTab::Logs => {
                                                            this.logs_tab_open = false;
                                                            this.logs_tab_active = false;
                                                            cx.notify();
                                                        },
                                                    }
                                                }),
                                            ),
                                    )
                                    .when(index + 1 == tab_count, |this| this.border_r_1())
                                    .map(|this| match relation {
                                        Some(std::cmp::Ordering::Equal) => {
                                            let el = this.border_r_1();
                                            if index == 0 { el } else { el.border_l_1() }
                                        },
                                        Some(std::cmp::Ordering::Less) => {
                                            let el = this.border_b_1();
                                            if index == 0 { el } else { el.border_l_1() }
                                        },
                                        Some(std::cmp::Ordering::Greater) => {
                                            this.border_r_1().border_b_1()
                                        },
                                        None => this.border_b_1(),
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| match tab {
                                        CenterTab::Terminal(session_id) => {
                                            this.logs_tab_active = false;
                                            this.select_terminal(session_id, window, cx);
                                        },
                                        CenterTab::Diff(diff_id) => {
                                            this.logs_tab_active = false;
                                            this.select_diff_tab(diff_id, cx);
                                        },
                                        CenterTab::Logs => {
                                            this.logs_tab_active = true;
                                            this.active_diff_session_id = None;
                                            cx.notify();
                                        },
                                    }))
                            })),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .border_l_1()
                            .border_color(rgb(theme.border))
                            .border_b_1()
                            .child(
                                div()
                                    .id("terminal-tab-new")
                                    .size(px(20.))
                                    .cursor_pointer()
                                    .rounded_sm()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(rgb(theme.text_muted))
                                    .child("+")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                            this.spawn_terminal_session(window, cx)
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(rgb(theme.terminal_bg))
                    .when(
                        active_terminal.is_none() && active_diff_session.is_none() && active_tab != Some(CenterTab::Logs),
                        |this| {
                            this.child(
                                div()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap_2()
                                    .text_center()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(theme.text_primary))
                                            .child("Workspace"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(theme.text_muted))
                                            .child("Press Cmd-T to open a terminal tab."),
                                    )
                                    .child(
                                        action_button(
                                            theme,
                                            "spawn-terminal-empty-state",
                                            "Open Terminal Tab",
                                            false,
                                            false,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, window, cx| {
                                                this.spawn_terminal_session(window, cx)
                                            }),
                                        ),
                                    ),
                            )
                        },
                    )
                    .when_some(active_terminal, |this, session| {
                        let selection = self.terminal_selection_for_session(session.id);
                        let styled_lines =
                            styled_lines_for_session(&session, theme, true, selection);
                        let mono_font = terminal_mono_font(cx);
                        let cell_width = terminal_cell_width_px(cx);
                        let line_height = terminal_line_height_px(cx);

                        this.child(
                            div()
                                .h_full()
                                .w_full()
                                .min_w_0()
                                .min_h_0()
                                .overflow_hidden()
                                .font(mono_font.clone())
                                .text_size(px(TERMINAL_FONT_SIZE_PX))
                                .line_height(px(line_height))
                                .px_2()
                                .pt_1()
                                .flex()
                                .flex_col()
                                .gap_0()
                                .child(
                                    div()
                                        .id("terminal-output-scroll")
                                        .flex_1()
                                        .w_full()
                                        .min_w_0()
                                        .min_h_0()
                                        .overflow_x_hidden()
                                        .overflow_y_scroll()
                                        .scrollbar_width(px(12.))
                                        .track_scroll(&self.terminal_scroll_handle)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(Self::handle_terminal_output_mouse_down),
                                        )
                                        .on_mouse_move(
                                            cx.listener(Self::handle_terminal_output_mouse_move),
                                        )
                                        .on_mouse_up(
                                            MouseButton::Left,
                                            cx.listener(Self::handle_terminal_output_mouse_up),
                                        )
                                        .child(
                                            div()
                                                .w_full()
                                                .min_w_0()
                                                .flex_none()
                                                .flex()
                                                .flex_col()
                                                .gap_0()
                                                .children(styled_lines.into_iter().map(|line| {
                                                    render_terminal_line(
                                                        line,
                                                        theme,
                                                        cell_width,
                                                        line_height,
                                                        mono_font.clone(),
                                                    )
                                                })),
                                        ),
                                ),
                        )
                    })
                    .when_some(active_diff_session, |this, session| {
                        let mono_font = terminal_mono_font(cx);
                        let diff_cell_width = diff_cell_width_px(cx);
                        this.child(render_diff_session(
                            session,
                            theme,
                            &self.diff_scroll_handle,
                            mono_font,
                            diff_cell_width,
                        ))
                    })
                    .when(active_tab == Some(CenterTab::Logs), |this| {
                        this.child(self.render_logs_content(cx))
                    }),
            )
    }

    fn render_logs_content(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let entry_count = self.log_entries.len();
        let auto_scroll = self.log_auto_scroll;

        div()
            .h_full()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.tab_bg))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(format!("{entry_count} entries")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .id("log-copy-all")
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child("Copy All")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                            let text = this
                                                .log_entries
                                                .iter()
                                                .map(format_log_entry)
                                                .collect::<Vec<_>>()
                                                .join("\n");
                                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("log-auto-scroll-toggle")
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(if auto_scroll {
                                        theme.accent
                                    } else {
                                        theme.text_muted
                                    }))
                                    .child(if auto_scroll {
                                        "Auto-scroll: ON"
                                    } else {
                                        "Auto-scroll: OFF"
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                            this.log_auto_scroll = !this.log_auto_scroll;
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().child(if entry_count > 0 {
                let entries = self.log_entries.clone();
                div()
                    .id("log-entries")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.log_scroll_handle)
                    .children(
                        entries
                            .iter()
                            .enumerate()
                            .map(|(ix, entry)| render_log_row(entry, ix, theme)),
                    )
                    .into_any_element()
            } else {
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(theme.text_muted))
                    .child("No log entries yet")
                    .into_any_element()
            }))
    }

    fn render_center_pane(&mut self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        div()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .bg(rgb(theme.app_bg))
            .flex()
            .flex_col()
            .child(self.render_terminal_panel(window, cx))
    }

    fn render_right_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let content: Div = match self.right_pane_tab {
            RightPaneTab::Changes => self.render_changes_content(cx),
            RightPaneTab::FileTree => self.render_file_tree(cx),
        };

        div()
            .w(px(self.right_pane_width))
            .h_full()
            .min_h_0()
            .bg(rgb(theme.sidebar_bg))
            .flex()
            .flex_col()
            .child(self.render_right_pane_tabs(cx))
            .child(content)
    }

    fn render_right_pane_tabs(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let active_tab = self.right_pane_tab;

        let tab_button = |label: &'static str, tab: RightPaneTab| {
            let is_active = active_tab == tab;
            div()
                .id(ElementId::Name(
                    format!("right-tab-{label}").to_lowercase().into(),
                ))
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_xs()
                .font_family(FONT_UI)
                .bg(rgb(if is_active {
                    theme.tab_active_bg
                } else {
                    theme.tab_bg
                }))
                .text_color(rgb(if is_active {
                    theme.text_primary
                } else {
                    theme.text_muted
                }))
                .when(is_active, |this| {
                    this.border_b_2().border_color(rgb(theme.accent))
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.set_right_pane_tab(tab, cx);
                    }),
                )
                .child(label)
        };

        div()
            .h(px(28.))
            .flex()
            .flex_row()
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(tab_button("Changes", RightPaneTab::Changes))
            .child(tab_button("Files", RightPaneTab::FileTree))
    }

    fn render_changes_content(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let selected_path = self.selected_changed_file.clone();
        let can_run_actions = self.can_run_local_git_actions();
        let is_busy = self.git_action_in_flight.is_some();
        let commit_enabled = can_run_actions && !is_busy && !self.changed_files.is_empty();
        let push_enabled = can_run_actions && !is_busy;
        let pr_enabled = can_run_actions && !is_busy;

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(32.))
                    .px_1()
                    .gap_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                git_action_button(
                                    theme,
                                    "changes-action-commit",
                                    GIT_ACTION_ICON_COMMIT,
                                    "Commit",
                                    commit_enabled,
                                    self.git_action_in_flight == Some(GitActionKind::Commit),
                                )
                                .when(commit_enabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.run_commit_action(cx);
                                    }))
                                }),
                            )
                            .child(
                                git_action_button(
                                    theme,
                                    "changes-action-push",
                                    GIT_ACTION_ICON_PUSH,
                                    "Push",
                                    push_enabled,
                                    self.git_action_in_flight == Some(GitActionKind::Push),
                                )
                                .when(push_enabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.run_push_action(cx);
                                    }))
                                }),
                            )
                            .child(
                                git_action_button(
                                    theme,
                                    "changes-action-pr",
                                    GIT_ACTION_ICON_PR,
                                    "Create PR",
                                    pr_enabled,
                                    self.git_action_in_flight
                                        == Some(GitActionKind::CreatePullRequest),
                                )
                                .when(pr_enabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.run_create_pr_action(cx);
                                    }))
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("changes-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .scrollbar_width(px(10.))
                    .flex()
                    .flex_col()
                    .font_family(FONT_MONO)
                    .p_1()
                    .children(self.changed_files.iter().map(|change| {
                        let is_selected = selected_path
                            .as_ref()
                            .is_some_and(|selected| selected.as_path() == change.path.as_path());
                        let status_color = match change.kind {
                            ChangeKind::Added => 0xa6e3a1,
                            ChangeKind::Modified => 0xf9e2af,
                            ChangeKind::Removed => 0xf38ba8,
                            ChangeKind::Renamed => 0x89dceb,
                            ChangeKind::Copied => 0x74c7ec,
                            ChangeKind::TypeChange => 0xcba6f7,
                            ChangeKind::Conflict => 0xf38ba8,
                            ChangeKind::IntentToAdd => 0x94e2d5,
                        };
                        let path_color = match change.kind {
                            ChangeKind::Added => 0x8fd7ad,
                            ChangeKind::Removed => 0xf2a4b7,
                            ChangeKind::Modified => 0xd9d7cf,
                            ChangeKind::Renamed => 0x8ecae6,
                            ChangeKind::Copied => 0x91d7e3,
                            ChangeKind::TypeChange => 0xc4b1ee,
                            ChangeKind::Conflict => 0xf38ba8,
                            ChangeKind::IntentToAdd => 0x94e2d5,
                        };
                        let show_line_stats = change.additions > 0 || change.deletions > 0;
                        let display_path = truncate_middle_path_for_width(
                            change.path.as_path(),
                            self.right_pane_width,
                        );
                        let file_path = change.path.clone();

                        div()
                            .h(px(24.))
                            .pl(px(4.))
                            .pr_1()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(is_selected, |this| this.bg(rgb(theme.panel_active_bg)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.select_changed_file(file_path.clone(), cx);
                                    this.open_diff_tab_for_selected_file(cx);
                                }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.))
                                    .text_color(rgb(status_color))
                                    .child(change_code(change.kind)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(rgb(path_color))
                                    .child(display_path),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap_1()
                                    .when(show_line_stats, |this| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x72d69c))
                                                .child(format!("+{}", change.additions)),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0xeb6f92))
                                                .child(format!("-{}", change.deletions)),
                                        )
                                    }),
                            )
                    })),
            )
    }

    fn render_file_tree(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let selected_entry = self.selected_file_tree_entry.clone();
        let expanded_dirs = &self.expanded_dirs;

        div().flex_1().min_h_0().flex().flex_col().child(
            div()
                .id("file-tree-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .scrollbar_width(px(10.))
                .flex()
                .flex_col()
                .font_family(FONT_MONO)
                .p_1()
                .children(self.file_tree_entries.iter().map(|entry| {
                    let is_selected = selected_entry
                        .as_ref()
                        .is_some_and(|selected| selected == &entry.path);
                    let indent = entry.depth as f32 * 16. + 4.;
                    let entry_path = entry.path.clone();
                    let is_dir = entry.is_dir;

                    let chevron = if is_dir {
                        if expanded_dirs.contains(&entry.path) {
                            "\u{f078}" // chevron down
                        } else {
                            "\u{f054}" // chevron right
                        }
                    } else {
                        " "
                    };

                    let (file_icon, icon_color) = file_icon_and_color(&entry.name, is_dir);

                    div()
                        .id(ElementId::Name(
                            format!("ft-{}", entry.path.display()).into(),
                        ))
                        .h(px(24.))
                        .pl(px(indent))
                        .pr_1()
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap_1()
                        .bg(rgb(if is_selected {
                            theme.panel_active_bg
                        } else {
                            theme.sidebar_bg
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                if is_dir {
                                    this.toggle_file_tree_dir(entry_path.clone(), cx);
                                } else {
                                    this.select_file_tree_entry(entry_path.clone(), cx);
                                }
                            }),
                        )
                        .child(
                            div()
                                .w(px(12.))
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(rgb(theme.text_muted))
                                .child(chevron),
                        )
                        .child(
                            div()
                                .w(px(20.))
                                .flex_none()
                                .text_size(px(18.))
                                .text_color(rgb(icon_color))
                                .child(file_icon),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_xs()
                                .text_color(rgb(icon_color))
                                .when(is_dir, |this| this.font_weight(FontWeight::SEMIBOLD))
                                .child(entry.name.clone()),
                        )
                })),
        )
    }

    fn render_status_bar(&self) -> impl IntoElement {
        let theme = self.theme();
        let repo_name = self
            .repo_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.repo_root.display().to_string());
        let worktree = self
            .active_worktree()
            .map(|entry| entry.label.clone())
            .unwrap_or_else(|| "none".to_owned());
        let terminal_count = self.selected_worktree_path().map_or(0, |worktree_path| {
            self.terminals
                .iter()
                .filter(|session| session.worktree_path.as_path() == worktree_path)
                .count()
        });

        div()
            .h(px(26.))
            .bg(rgb(theme.chrome_bg))
            .border_t_1()
            .border_color(rgb(theme.chrome_border))
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_text(theme, "●"))
                    .child(status_text(theme, format!("repo {repo_name}")))
                    .child(status_text(theme, "•"))
                    .child(status_text(theme, format!("worktree {worktree}"))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_text(
                        theme,
                        format!("changes {}", self.changed_files.len()),
                    ))
                    .child(status_text(theme, "•"))
                    .child(status_text(theme, format!("terminals {terminal_count}")))
                    .child(
                        if self.worktree_stats_loading || self.worktree_prs_loading {
                            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                            let frame_index = (SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis()
                                / 100) as usize
                                % frames.len();
                            status_text(theme, format!("{} loading", frames[frame_index]))
                        } else {
                            status_text(theme, "ready")
                        },
                    ),
            )
    }

    fn render_create_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.create_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let has_remote_hosts = !self.remote_hosts.is_empty();
        let is_worktree_tab = modal.tab == CreateModalTab::LocalWorktree;
        let is_outpost_tab = modal.tab == CreateModalTab::RemoteOutpost;

        // Worktree tab data
        let branch_name = derive_branch_name(&modal.worktree_name);
        let target_path_preview =
            preview_managed_worktree_path(modal.repository_path.trim(), modal.worktree_name.trim())
                .unwrap_or_else(|_| "-".to_owned());
        let repository_active = modal.worktree_active_field == CreateWorktreeField::RepositoryPath;
        let worktree_active = modal.worktree_active_field == CreateWorktreeField::WorktreeName;
        let worktree_create_disabled = modal.is_creating
            || modal.repository_path.trim().is_empty()
            || modal.worktree_name.trim().is_empty();

        // Outpost tab data
        let host_name = self
            .remote_hosts
            .get(modal.host_index)
            .map(|h| h.name.clone())
            .unwrap_or_else(|| "-".to_owned());
        let remote_preview = self
            .remote_hosts
            .get(modal.host_index)
            .map(|h| format!("{}/{}", h.remote_base_path, modal.outpost_name.trim()))
            .unwrap_or_else(|| "-".to_owned());
        let host_active = modal.outpost_active_field == CreateOutpostField::HostSelector;
        let clone_url_active = modal.outpost_active_field == CreateOutpostField::CloneUrl;
        let branch_active = modal.outpost_active_field == CreateOutpostField::Branch;
        let outpost_name_active = modal.outpost_active_field == CreateOutpostField::OutpostName;
        let outpost_create_disabled = modal.is_creating
            || modal.clone_url.trim().is_empty()
            || modal.outpost_name.trim().is_empty()
            || modal.branch.trim().is_empty()
            || self.remote_hosts.is_empty();

        let create_disabled = if is_worktree_tab {
            worktree_create_disabled
        } else {
            outpost_create_disabled
        };
        let submit_label = if modal.is_creating {
            "Creating..."
        } else if is_worktree_tab {
            "Create Worktree"
        } else {
            "Create Outpost"
        };

        div()
            .absolute()
            .inset_0()
            .bg(rgb(0x10131a))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(620.))
                    .max_w(px(620.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // Header
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_primary))
                                    .child("Add"),
                            )
                            .child(
                                action_button(theme, "close-create-modal", "Close", false, true)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_create_modal(cx);
                                    })),
                            ),
                    )
                    // Tab bar
                    .child(
                        div()
                            .flex()
                            .gap_0()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .child(
                                div()
                                    .id("tab-local-worktree")
                                    .cursor_pointer()
                                    .px_3()
                                    .py_1()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(if is_worktree_tab {
                                        theme.text_primary
                                    } else {
                                        theme.text_muted
                                    }))
                                    .when(is_worktree_tab, |this| {
                                        this.border_b_2()
                                            .border_color(rgb(theme.accent))
                                    })
                                    .child("Local Worktree")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(modal) = this.create_modal.as_mut()
                                            && !modal.is_creating
                                        {
                                            modal.tab = CreateModalTab::LocalWorktree;
                                            modal.error = None;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .when(has_remote_hosts, |this| {
                                this.child(
                                    div()
                                        .id("tab-remote-outpost")
                                        .cursor_pointer()
                                        .px_3()
                                        .py_1()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(if is_outpost_tab {
                                            theme.text_primary
                                        } else {
                                            theme.text_muted
                                        }))
                                        .when(is_outpost_tab, |this| {
                                            this.border_b_2()
                                                .border_color(rgb(theme.accent))
                                        })
                                        .child("Remote Outpost")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(modal) = this.create_modal.as_mut()
                                                && !modal.is_creating
                                            {
                                                modal.tab = CreateModalTab::RemoteOutpost;
                                                modal.error = None;
                                                cx.notify();
                                            }
                                        })),
                                )
                            }),
                    )
                    // Local Worktree tab content
                    .when(is_worktree_tab, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_muted))
                                .child("Target base: ~/.arbor/worktrees/<repo>/<worktree>/"),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "create-worktree-repo-input",
                                "Repository",
                                &modal.repository_path,
                                "Path to git repository",
                                repository_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_create_worktree_modal_input(
                                    ModalInputEvent::SetActiveField(
                                        CreateWorktreeField::RepositoryPath,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "create-worktree-name-input",
                                "Worktree Name",
                                &modal.worktree_name,
                                "e.g. remote-ssh",
                                worktree_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_create_worktree_modal_input(
                                    ModalInputEvent::SetActiveField(
                                        CreateWorktreeField::WorktreeName,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            div()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(theme.border))
                                .bg(rgb(theme.panel_bg))
                                .p_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .child("Branch"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family(FONT_MONO)
                                        .text_color(rgb(theme.text_primary))
                                        .child(branch_name),
                                ),
                        )
                        .child(
                            div()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(theme.border))
                                .bg(rgb(theme.panel_bg))
                                .p_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .child("Path"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family(FONT_MONO)
                                        .text_color(rgb(theme.text_primary))
                                        .child(target_path_preview),
                                ),
                        )
                    })
                    // Remote Outpost tab content
                    .when(is_outpost_tab, |this| {
                        this.child(
                            div()
                                .id("outpost-host-selector")
                                .cursor_pointer()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(if host_active {
                                    theme.accent
                                } else {
                                    theme.border
                                }))
                                .bg(rgb(theme.panel_bg))
                                .p_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .child("Host"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_family(FONT_MONO)
                                                .text_color(rgb(theme.text_primary))
                                                .child(host_name),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .id("outpost-host-prev")
                                                        .cursor_pointer()
                                                        .text_xs()
                                                        .text_color(rgb(theme.text_muted))
                                                        .child("\u{25c0}")
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.update_create_outpost_modal_input(
                                                                    OutpostModalInputEvent::CycleHost(
                                                                        true,
                                                                    ),
                                                                    cx,
                                                                );
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .id("outpost-host-next")
                                                        .cursor_pointer()
                                                        .text_xs()
                                                        .text_color(rgb(theme.text_muted))
                                                        .child("\u{25b6}")
                                                        .on_click(cx.listener(
                                                            |this, _, _, cx| {
                                                                this.update_create_outpost_modal_input(
                                                                    OutpostModalInputEvent::CycleHost(
                                                                        false,
                                                                    ),
                                                                    cx,
                                                                );
                                                            },
                                                        )),
                                                ),
                                        ),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_create_outpost_modal_input(
                                        OutpostModalInputEvent::SetActiveField(
                                            CreateOutpostField::HostSelector,
                                        ),
                                        cx,
                                    );
                                })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "outpost-clone-url-input",
                                "Clone URL",
                                &modal.clone_url,
                                "git@github.com:user/repo.git",
                                clone_url_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_create_outpost_modal_input(
                                    OutpostModalInputEvent::SetActiveField(
                                        CreateOutpostField::CloneUrl,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "outpost-branch-input",
                                "Branch",
                                &modal.branch,
                                "main",
                                branch_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_create_outpost_modal_input(
                                    OutpostModalInputEvent::SetActiveField(
                                        CreateOutpostField::Branch,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "outpost-name-input",
                                "Outpost Name",
                                &modal.outpost_name,
                                "e.g. my-feature",
                                outpost_name_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_create_outpost_modal_input(
                                    OutpostModalInputEvent::SetActiveField(
                                        CreateOutpostField::OutpostName,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            div()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(theme.border))
                                .bg(rgb(theme.panel_bg))
                                .p_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .child("Remote Path"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family(FONT_MONO)
                                        .text_color(rgb(theme.text_primary))
                                        .child(remote_preview),
                                ),
                        )
                    })
                    // Error
                    .child(div().when_some(modal.error.clone(), |this, error| {
                        this.rounded_sm()
                            .border_1()
                            .border_color(rgb(0xa44949))
                            .bg(rgb(0x4d2a2a))
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(rgb(0xffd7d7))
                            .child(error)
                    }))
                    // Buttons
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "cancel-create-modal",
                                    "Cancel",
                                    false,
                                    true,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_create_modal(cx);
                                })),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "submit-create-modal",
                                    submit_label,
                                    !create_disabled,
                                    create_disabled,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(modal) = this.create_modal.as_ref() {
                                        match modal.tab {
                                            CreateModalTab::LocalWorktree => {
                                                this.submit_create_worktree_modal(cx);
                                            },
                                            CreateModalTab::RemoteOutpost => {
                                                this.submit_create_outpost_modal(cx);
                                            },
                                        }
                                    }
                                })),
                            ),
                    ),
            )
    }

    fn render_manage_hosts_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.manage_hosts_modal.clone() else {
            return div();
        };

        let theme = self.theme();

        if modal.adding {
            let name_active = modal.active_field == ManageHostsField::Name;
            let hostname_active = modal.active_field == ManageHostsField::Hostname;
            let user_active = modal.active_field == ManageHostsField::User;
            let add_disabled = modal.name.trim().is_empty()
                || modal.hostname.trim().is_empty()
                || modal.user.trim().is_empty();

            return div()
                .absolute()
                .inset_0()
                .bg(rgb(0x10131a))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(620.))
                        .max_w(px(620.))
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.sidebar_bg))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        // Header
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(theme.text_primary))
                                        .child("Add Host"),
                                )
                                .child(
                                    action_button(
                                        theme,
                                        "back-manage-hosts",
                                        "Back",
                                        false,
                                        true,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(modal) = this.manage_hosts_modal.as_mut() {
                                            modal.adding = false;
                                            modal.error = None;
                                            cx.notify();
                                        }
                                    })),
                                ),
                        )
                        // Name
                        .child(
                            modal_input_field(
                                theme,
                                "hosts-name-input",
                                "Name",
                                &modal.name,
                                "e.g. build-server",
                                name_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_hosts_modal_input(
                                    HostsModalInputEvent::SetActiveField(ManageHostsField::Name),
                                    cx,
                                );
                            })),
                        )
                        // Hostname
                        .child(
                            modal_input_field(
                                theme,
                                "hosts-hostname-input",
                                "Hostname",
                                &modal.hostname,
                                "e.g. build.example.com",
                                hostname_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_hosts_modal_input(
                                    HostsModalInputEvent::SetActiveField(
                                        ManageHostsField::Hostname,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        // User
                        .child(
                            modal_input_field(
                                theme,
                                "hosts-user-input",
                                "User",
                                &modal.user,
                                "e.g. dev",
                                user_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_hosts_modal_input(
                                    HostsModalInputEvent::SetActiveField(ManageHostsField::User),
                                    cx,
                                );
                            })),
                        )
                        // Error
                        .child(div().when_some(modal.error.clone(), |this, error| {
                            this.rounded_sm()
                                .border_1()
                                .border_color(rgb(0xa44949))
                                .bg(rgb(0x4d2a2a))
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(rgb(0xffd7d7))
                                .child(error)
                        }))
                        // Buttons
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(
                                    action_button(
                                        theme,
                                        "cancel-add-host",
                                        "Cancel",
                                        false,
                                        true,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(modal) = this.manage_hosts_modal.as_mut() {
                                            modal.adding = false;
                                            modal.error = None;
                                            cx.notify();
                                        }
                                    })),
                                )
                                .child(
                                    action_button(
                                        theme,
                                        "submit-add-host",
                                        "Add Host",
                                        !add_disabled,
                                        add_disabled,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_add_host(cx);
                                    })),
                                ),
                        ),
                );
        }

        // List view
        let hosts = self.remote_hosts.clone();
        div()
            .absolute()
            .inset_0()
            .bg(rgb(0x10131a))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(620.))
                    .max_w(px(620.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // Header
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_primary))
                                    .child("Manage Hosts"),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "close-manage-hosts",
                                    "Close",
                                    false,
                                    true,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_manage_hosts_modal(cx);
                                })),
                            ),
                    )
                    // Host list
                    .child(if hosts.is_empty() {
                        div()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.panel_bg))
                            .p_3()
                            .text_sm()
                            .text_color(rgb(theme.text_muted))
                            .child("No remote hosts configured.")
                            .into_any_element()
                    } else {
                        let mut list = div()
                            .id("manage-hosts-list")
                            .flex()
                            .flex_col()
                            .gap_1()
                            .max_h(px(300.))
                            .overflow_y_scroll();
                        for (i, host) in hosts.iter().enumerate() {
                            let host_name = host.name.clone();
                            let display = format!("{}@{}", host.user, host.hostname);
                            list = list.child(
                                div()
                                    .id(ElementId::NamedInteger("host-row".into(), i as u64))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(theme.border))
                                    .bg(rgb(theme.panel_bg))
                                    .px_2()
                                    .py_1()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(theme.text_primary))
                                                    .child(host.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_family(FONT_MONO)
                                                    .text_color(rgb(theme.text_muted))
                                                    .child(display),
                                            ),
                                    )
                                    .child(
                                        action_button(
                                            theme,
                                            ElementId::NamedInteger(
                                                "remove-host".into(),
                                                i as u64,
                                            ),
                                            "Remove",
                                            false,
                                            true,
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_host_at(host_name.clone(), cx);
                                        })),
                                    ),
                            );
                        }
                        list.into_any_element()
                    })
                    // Add Host button
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .child(
                                action_button(
                                    theme,
                                    "open-add-host-form",
                                    "+ Add Host",
                                    true,
                                    false,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(modal) = this.manage_hosts_modal.as_mut() {
                                        modal.adding = true;
                                        modal.name.clear();
                                        modal.hostname.clear();
                                        modal.user.clear();
                                        modal.active_field = ManageHostsField::Name;
                                        modal.error = None;
                                        cx.notify();
                                    }
                                })),
                            ),
                    ),
            )
    }
}

fn daemon_state_from_terminal_state(state: TerminalState) -> TerminalSessionState {
    match state {
        TerminalState::Running => TerminalSessionState::Running,
        TerminalState::Completed => TerminalSessionState::Completed,
        TerminalState::Failed => TerminalSessionState::Failed,
    }
}

fn terminal_state_from_daemon_state(state: TerminalSessionState) -> TerminalState {
    match state {
        TerminalSessionState::Running => TerminalState::Running,
        TerminalSessionState::Completed => TerminalState::Completed,
        TerminalSessionState::Failed => TerminalState::Failed,
    }
}

fn terminal_state_from_daemon_record(record: &DaemonSessionRecord) -> TerminalState {
    if let Some(state) = record.state {
        return terminal_state_from_daemon_state(state);
    }

    match record.exit_code {
        Some(0) => TerminalState::Completed,
        Some(_) => TerminalState::Failed,
        None => TerminalState::Running,
    }
}

fn terminal_output_tail_for_metadata(
    session: &TerminalSession,
    max_lines: usize,
    max_chars: usize,
) -> String {
    let lines = terminal_display_lines(session);
    if lines.is_empty() {
        return String::new();
    }

    let start = lines.len().saturating_sub(max_lines);
    let mut tail = lines
        .into_iter()
        .skip(start)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();

    let char_count = tail.chars().count();
    if char_count > max_chars {
        let skip = char_count.saturating_sub(max_chars);
        tail = tail.chars().skip(skip).collect::<String>();
    }

    tail
}

fn current_unix_timestamp_millis() -> Option<u64> {
    daemon::current_unix_timestamp_millis()
}

fn daemon_base_url_from_config(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DAEMON_BASE_URL)
        .to_owned()
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    worktree::paths_equivalent(left, right)
}

fn porcelain_status_to_change_kind(xy: &str) -> ChangeKind {
    let bytes = xy.as_bytes();
    let x = bytes.first().copied().unwrap_or(b' ');
    let y = bytes.get(1).copied().unwrap_or(b' ');

    match (x, y) {
        (b'?', b'?') => ChangeKind::Added,
        (b'A', _) | (_, b'A') => ChangeKind::Added,
        (b'D', _) | (_, b'D') => ChangeKind::Removed,
        (b'R', _) | (_, b'R') => ChangeKind::Renamed,
        (b'C', _) | (_, b'C') => ChangeKind::Copied,
        (b'T', _) | (_, b'T') => ChangeKind::TypeChange,
        (b'U', _) | (_, b'U') => ChangeKind::Conflict,
        (b'M', _) | (_, b'M') => ChangeKind::Modified,
        _ => ChangeKind::Modified,
    }
}

fn parse_remote_numstat_output(output: &str) -> HashMap<PathBuf, (usize, usize)> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let mut columns = line.split('\t');
        let Some(added) = columns.next() else {
            continue;
        };
        let Some(removed) = columns.next() else {
            continue;
        };
        let Some(path_str) = columns.next() else {
            continue;
        };
        let additions = added.parse::<usize>().unwrap_or(0);
        let deletions = removed.parse::<usize>().unwrap_or(0);
        if additions > 0 || deletions > 0 {
            map.insert(PathBuf::from(path_str), (additions, deletions));
        }
    }
    map
}

fn daemon_error_is_connection_refused(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("connection refused")
        || lower.contains("failed to connect")
        || lower.contains("actively refused")
}

fn load_outpost_summaries(
    store: &dyn arbor_core::outpost_store::OutpostStore,
    remote_hosts: &[arbor_core::outpost::RemoteHost],
) -> Vec<OutpostSummary> {
    let records = match store.load() {
        Ok(records) => records,
        Err(_) => return Vec::new(),
    };

    records
        .into_iter()
        .map(|record| {
            let hostname = remote_hosts
                .iter()
                .find(|host| host.name == record.host_name)
                .map(|host| host.hostname.clone())
                .unwrap_or_else(|| record.host_name.clone());

            OutpostSummary {
                outpost_id: record.id,
                repo_root: PathBuf::from(&record.local_repo_root),
                remote_path: record.remote_path,
                label: record.label,
                branch: record.branch,
                host_name: record.host_name,
                hostname,
                status: arbor_core::outpost::OutpostStatus::default(),
            }
        })
        .collect()
}

impl WorktreeSummary {
    fn from_worktree(entry: &worktree::Worktree, repo_root: &Path) -> Self {
        let label = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.path.display().to_string());

        let branch = entry
            .branch
            .as_deref()
            .map(short_branch)
            .unwrap_or_else(|| "-".to_owned());
        let is_primary_checkout = entry.path.as_path() == repo_root;

        Self {
            repo_root: repo_root.to_path_buf(),
            path: entry.path.clone(),
            label,
            branch,
            is_primary_checkout,
            pr_number: None,
            pr_url: None,
            diff_summary: None,
            agent_state: None,
        }
    }
}

impl RepositorySummary {
    fn from_root(root: PathBuf) -> Self {
        let label = repository_display_name(&root);
        let github_repo_slug = github_repo_slug_for_repo(&root);
        let avatar_url = github_repo_slug
            .as_ref()
            .and_then(|repo_slug| github_avatar_url_for_repo_slug(repo_slug));

        Self {
            root,
            label,
            avatar_url,
            github_repo_slug,
        }
    }
}

impl Render for ArborWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.window_is_active = window.is_window_active();
        if self.focus_terminal_on_next_render && self.active_terminal().is_some() {
            window.focus(&self.terminal_focus);
            self.focus_terminal_on_next_render = false;
        }
        let workspace_width = f32::from(window.window_bounds().get_bounds().size.width);
        self.clamp_pane_widths_for_workspace(workspace_width);
        self.sync_ui_state_store(window);

        let theme = self.theme();
        div()
            .size_full()
            .bg(rgb(theme.app_bg))
            .text_color(rgb(theme.text_primary))
            .font_family(FONT_UI)
            .relative()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(Self::handle_global_key_down))
            .on_action(cx.listener(Self::action_spawn_terminal))
            .on_action(cx.listener(Self::action_close_active_terminal))
            .on_action(cx.listener(Self::action_refresh_worktrees))
            .on_action(cx.listener(Self::action_refresh_changes))
            .on_action(cx.listener(Self::action_open_add_repository))
            .on_action(cx.listener(Self::action_open_create_worktree))
            .on_action(cx.listener(Self::action_use_one_dark_theme))
            .on_action(cx.listener(Self::action_use_ayu_dark_theme))
            .on_action(cx.listener(Self::action_use_gruvbox_theme))
            .on_action(cx.listener(Self::action_use_embedded_backend))
            .on_action(cx.listener(Self::action_use_alacritty_backend))
            .on_action(cx.listener(Self::action_use_ghostty_backend))
            .on_action(cx.listener(Self::action_toggle_left_pane))
            .on_action(cx.listener(Self::action_navigate_worktree_back))
            .on_action(cx.listener(Self::action_navigate_worktree_forward))
            .on_action(cx.listener(Self::action_collapse_all_repositories))
            .on_action(cx.listener(Self::action_view_logs))
            .on_action(cx.listener(Self::action_request_quit))
            .on_action(cx.listener(Self::action_immediate_quit))
            .child(self.render_top_bar(cx))
            .child(div().h(px(1.)).bg(rgb(theme.chrome_border)))
            .child(div().when_some(self.notice.clone(), |this, notice| {
                this.px_3()
                    .py_2()
                    .bg(rgb(theme.notice_bg))
                    .text_color(rgb(theme.notice_text))
                    .text_xs()
                    .child(notice)
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .flex()
                    .flex_row()
                    .on_drag_move(cx.listener(Self::handle_pane_divider_drag_move))
                    .child(self.render_left_pane(cx))
                    .when(self.left_pane_visible, |this| {
                        this.child(self.render_pane_resize_handle(
                            "left-pane-resize",
                            DraggedPaneDivider::Left,
                            theme,
                        ))
                    })
                    .child(self.render_center_pane(window, cx))
                    .child(self.render_pane_resize_handle(
                        "right-pane-resize",
                        DraggedPaneDivider::Right,
                        theme,
                    ))
                    .child(self.render_right_pane(cx)),
            )
            .child(self.render_status_bar())
            .child(self.render_create_modal(cx))
            .child(self.render_manage_hosts_modal(cx))
            .when(
                self.quit_overlay_until
                    .is_some_and(|until| Instant::now() < until),
                |this| {
                    this.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .px_4()
                                    .py_2()
                                    .rounded_md()
                                    .bg(rgb(theme.chrome_bg))
                                    .border_1()
                                    .border_color(rgb(theme.border))
                                    .text_sm()
                                    .text_color(rgb(theme.text_primary))
                                    .child("Hold ⌘Q to quit"),
                            ),
                    )
                },
            )
    }
}

fn process_agent_ws_message(
    this: &gpui::WeakEntity<ArborWindow>,
    cx: &mut gpui::AsyncApp,
    text: &str,
) {
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(text);
    let Ok(value) = parsed else {
        return;
    };

    let msg_type = value.get("type").and_then(|v| v.as_str());
    match msg_type {
        Some("snapshot") => {
            let sessions = value
                .get("sessions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let entries: Vec<(String, AgentState)> = sessions
                .iter()
                .filter_map(|s| {
                    let cwd = s.get("cwd")?.as_str()?;
                    let state_str = s.get("state")?.as_str()?;
                    let state = match state_str {
                        "working" => AgentState::Working,
                        "waiting" => AgentState::Waiting,
                        _ => return None,
                    };
                    Some((cwd.to_owned(), state))
                })
                .collect();
            let _ = this.update(cx, |this, cx| {
                apply_agent_ws_snapshot(this, &entries);
                cx.notify();
            });
        },
        Some("update") => {
            if let Some(session) = value.get("session") {
                let cwd = session.get("cwd").and_then(|v| v.as_str());
                let state_str = session.get("state").and_then(|v| v.as_str());
                if let (Some(cwd), Some(state_str)) = (cwd, state_str) {
                    let state = match state_str {
                        "working" => AgentState::Working,
                        "waiting" => AgentState::Waiting,
                        _ => return,
                    };
                    let entries = vec![(cwd.to_owned(), state)];
                    let _ = this.update(cx, |this, cx| {
                        apply_agent_ws_update(this, &entries);
                        cx.notify();
                    });
                }
            }
        },
        _ => {},
    }
}

fn apply_agent_ws_snapshot(app: &mut ArborWindow, entries: &[(String, AgentState)]) {
    tracing::debug!(count = entries.len(), "agent WS snapshot received");
    for worktree in &mut app.worktrees {
        worktree.agent_state = None;
    }
    apply_agent_ws_update(app, entries);
}

fn apply_agent_ws_update(app: &mut ArborWindow, entries: &[(String, AgentState)]) {
    let worktree_paths: Vec<PathBuf> = app.worktrees.iter().map(|w| w.path.clone()).collect();

    for (cwd, state) in entries {
        let cwd_path = Path::new(cwd);
        // Find the most specific (longest) worktree path that is a prefix of this cwd,
        // same logic as worktrees_with_agents().
        let best_match = worktree_paths
            .iter()
            .filter(|wt_path| cwd_path.starts_with(wt_path))
            .max_by_key(|wt_path| wt_path.as_os_str().len());

        if let Some(matched_path) = best_match {
            if let Some(worktree) = app
                .worktrees
                .iter_mut()
                .find(|w| &w.path == matched_path)
            {
                tracing::debug!(
                    cwd = %cwd,
                    worktree = %worktree.path.display(),
                    ?state,
                    "agent activity matched"
                );
                worktree.agent_state = Some(*state);
            }
        }
    }
}

fn install_claude_code_hooks(daemon_base_url: &str) -> Result<(), String> {
    let home = env::var("HOME").map_err(|_| "HOME not set".to_owned())?;
    let claude_dir = PathBuf::from(&home).join(".claude");
    let settings_path = claude_dir.join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .map_err(|e| format!("failed to read settings.json: {e}"))?;
        serde_json::from_str(&content).map_err(|e| format!("failed to parse settings.json: {e}"))?
    } else {
        if !claude_dir.exists() {
            fs::create_dir_all(&claude_dir)
                .map_err(|e| format!("failed to create .claude dir: {e}"))?;
        }
        serde_json::json!({})
    };

    let notify_url = format!("{daemon_base_url}/api/v1/agent/notify");

    // Check if our hooks are already present
    if let Some(hooks) = settings.get("hooks") {
        let hooks_str = hooks.to_string();
        if hooks_str.contains("/api/v1/agent/notify") {
            tracing::debug!("Claude Code hooks already installed");
            return Ok(());
        }
    }

    let hook_entry = serde_json::json!([
        {
            "matcher": "",
            "hooks": [
                {
                    "type": "http",
                    "url": notify_url,
                    "timeout": 2
                }
            ]
        }
    ]);

    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_obj = hooks.as_object_mut().ok_or("hooks is not an object")?;

    if !hooks_obj.contains_key("UserPromptSubmit") {
        hooks_obj.insert("UserPromptSubmit".to_owned(), hook_entry.clone());
    }
    if !hooks_obj.contains_key("Stop") {
        hooks_obj.insert("Stop".to_owned(), hook_entry);
    }

    let serialized = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("failed to serialize settings: {e}"))?;
    fs::write(&settings_path, serialized)
        .map_err(|e| format!("failed to write settings.json: {e}"))?;

    tracing::info!(path = %settings_path.display(), "installed Claude Code hooks");
    Ok(())
}

fn worktree_rows_changed(previous: &[WorktreeSummary], next: &[WorktreeSummary]) -> bool {
    if previous.len() != next.len() {
        return true;
    }

    previous.iter().zip(next.iter()).any(|(left, right)| {
        left.repo_root != right.repo_root
            || left.path != right.path
            || left.label != right.label
            || left.branch != right.branch
            || left.is_primary_checkout != right.is_primary_checkout
    })
}

fn terminal_tab_title(session: &TerminalSession) -> String {
    let Some(last_command) = session
        .last_command
        .as_ref()
        .filter(|command| !command.trim().is_empty())
    else {
        return String::new();
    };

    truncate_with_ellipsis(last_command.trim(), TERMINAL_TAB_COMMAND_MAX_CHARS)
}

fn diff_tab_title(session: &DiffSession) -> String {
    truncate_with_ellipsis(&session.title, TERMINAL_TAB_COMMAND_MAX_CHARS)
}

fn build_worktree_diff_document(
    worktree_path: &Path,
    changed_files: &[ChangedFile],
) -> Result<(Vec<DiffLine>, HashMap<PathBuf, usize>), String> {
    let mut lines = Vec::new();
    let mut file_row_indices = HashMap::new();

    for changed_file in changed_files {
        file_row_indices.insert(changed_file.path.clone(), lines.len());
        lines.push(DiffLine {
            left_line_number: None,
            right_line_number: None,
            left_text: format!(
                "{} {}",
                change_code(changed_file.kind),
                changed_file.path.display()
            ),
            right_text: String::new(),
            kind: DiffLineKind::FileHeader,
        });

        let file_lines = build_file_diff_lines(
            worktree_path,
            changed_file.path.as_path(),
            changed_file.kind,
        )?;
        if file_lines.is_empty() {
            lines.push(DiffLine {
                left_line_number: None,
                right_line_number: None,
                left_text: "  no textual changes".to_owned(),
                right_text: String::new(),
                kind: DiffLineKind::Context,
            });
        } else {
            lines.extend(file_lines);
        }
    }

    Ok((lines, file_row_indices))
}

fn build_file_diff_lines(
    worktree_path: &Path,
    file_path: &Path,
    change_kind: ChangeKind,
) -> Result<Vec<DiffLine>, String> {
    let head_bytes = match change_kind {
        ChangeKind::Added | ChangeKind::IntentToAdd => Vec::new(),
        _ => read_head_file_bytes(worktree_path, file_path)?,
    };
    let worktree_bytes = match change_kind {
        ChangeKind::Removed => Vec::new(),
        _ => read_worktree_file_bytes(worktree_path, file_path)?,
    };
    let head_text = String::from_utf8_lossy(&head_bytes).into_owned();
    let worktree_text = String::from_utf8_lossy(&worktree_bytes).into_owned();
    Ok(build_side_by_side_diff_lines(&head_text, &worktree_text))
}

fn read_head_file_bytes(worktree_path: &Path, file_path: &Path) -> Result<Vec<u8>, String> {
    let relative = git_relative_path(file_path)?;
    let object_spec = format!("HEAD:{relative}");

    let exists_status = Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(["cat-file", "-e", object_spec.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            format!(
                "failed to check git object `{object_spec}` in `{}`: {error}",
                worktree_path.display()
            )
        })?;

    if !exists_status.success() {
        return Ok(Vec::new());
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args(["show", object_spec.as_str()])
        .output()
        .map_err(|error| {
            format!(
                "failed to read `{relative}` at HEAD in `{}`: {error}",
                worktree_path.display()
            )
        })?;

    if !output.status.success() {
        return Err(format!(
            "failed to read `{relative}` at HEAD in `{}`: {}",
            worktree_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(output.stdout)
}

fn read_worktree_file_bytes(worktree_path: &Path, file_path: &Path) -> Result<Vec<u8>, String> {
    let absolute = worktree_path.join(file_path);
    match fs::read(&absolute) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!(
            "failed to read worktree file `{}`: {error}",
            absolute.display()
        )),
    }
}

fn git_relative_path(file_path: &Path) -> Result<String, String> {
    let path_text = file_path.to_string_lossy();
    if path_text.trim().is_empty() {
        return Err("cannot diff an empty path".to_owned());
    }

    Ok(path_text.replace('\\', "/"))
}

fn build_side_by_side_diff_lines(before_text: &str, after_text: &str) -> Vec<DiffLine> {
    let before_rope = Rope::from_str(before_text);
    let after_rope = Rope::from_str(after_text);
    let input = BlobInternedInput::new(before_text.as_bytes(), after_text.as_bytes());
    let mut diff = BlobDiff::compute(DiffAlgorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let hunks = diff.hunks().collect::<Vec<_>>();

    if hunks.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut before_cursor = 0_usize;
    let mut after_cursor = 0_usize;
    let hunk_count = hunks.len();

    for (hunk_index, hunk) in hunks.iter().enumerate() {
        let before_start = hunk.before.start as usize;
        let before_end = hunk.before.end as usize;
        let after_start = hunk.after.start as usize;
        let after_end = hunk.after.end as usize;

        let (leading_context, trailing_context) = if hunk_index == 0 {
            (0, DIFF_HUNK_CONTEXT_LINES)
        } else {
            (DIFF_HUNK_CONTEXT_LINES, DIFF_HUNK_CONTEXT_LINES)
        };
        push_hunk_context_lines(
            &mut lines,
            &before_rope,
            &after_rope,
            before_cursor,
            before_start,
            after_cursor,
            after_start,
            leading_context,
            trailing_context,
        );

        let removed_count = before_end.saturating_sub(before_start);
        let added_count = after_end.saturating_sub(after_start);
        let changed_count = removed_count.max(added_count);

        for offset in 0..changed_count {
            let left_index = (offset < removed_count).then_some(before_start + offset);
            let right_index = (offset < added_count).then_some(after_start + offset);
            let kind = match (left_index.is_some(), right_index.is_some()) {
                (true, true) => DiffLineKind::Modified,
                (true, false) => DiffLineKind::Removed,
                (false, true) => DiffLineKind::Added,
                (false, false) => DiffLineKind::Context,
            };
            push_diff_line(
                &mut lines,
                &before_rope,
                &after_rope,
                left_index,
                right_index,
                kind,
            );
        }

        before_cursor = before_end;
        after_cursor = after_end;

        if hunk_index + 1 == hunk_count {
            push_hunk_context_lines(
                &mut lines,
                &before_rope,
                &after_rope,
                before_cursor,
                input.before.len(),
                after_cursor,
                input.after.len(),
                DIFF_HUNK_CONTEXT_LINES,
                0,
            );
        }
    }

    lines
}

fn push_hunk_context_lines(
    output: &mut Vec<DiffLine>,
    before_rope: &Rope,
    after_rope: &Rope,
    before_start: usize,
    before_end: usize,
    after_start: usize,
    after_end: usize,
    leading_context: usize,
    trailing_context: usize,
) {
    let before_count = before_end.saturating_sub(before_start);
    let after_count = after_end.saturating_sub(after_start);
    if before_count == 0 && after_count == 0 {
        return;
    }

    let leading_before_count = leading_context.min(before_count);
    let leading_after_count = leading_context.min(after_count);
    let leading_before_end = before_start.saturating_add(leading_before_count);
    let leading_after_end = after_start.saturating_add(leading_after_count);

    let trailing_before_available = before_end.saturating_sub(leading_before_end);
    let trailing_after_available = after_end.saturating_sub(leading_after_end);
    let trailing_before_count = trailing_context.min(trailing_before_available);
    let trailing_after_count = trailing_context.min(trailing_after_available);
    let trailing_before_start = before_end.saturating_sub(trailing_before_count);
    let trailing_after_start = after_end.saturating_sub(trailing_after_count);

    if leading_before_end > before_start || leading_after_end > after_start {
        push_context_diff_lines(
            output,
            before_rope,
            after_rope,
            before_start,
            leading_before_end,
            after_start,
            leading_after_end,
        );
    }

    let hidden_before_count = trailing_before_start.saturating_sub(leading_before_end);
    let hidden_after_count = trailing_after_start.saturating_sub(leading_after_end);
    if hidden_before_count > 0 || hidden_after_count > 0 {
        push_collapsed_gap_line(output, hidden_before_count, hidden_after_count);
    }

    if trailing_before_start < before_end || trailing_after_start < after_end {
        push_context_diff_lines(
            output,
            before_rope,
            after_rope,
            trailing_before_start,
            before_end,
            trailing_after_start,
            after_end,
        );
    }
}

fn push_collapsed_gap_line(
    output: &mut Vec<DiffLine>,
    hidden_before_count: usize,
    hidden_after_count: usize,
) {
    output.push(DiffLine {
        left_line_number: None,
        right_line_number: None,
        left_text: format!("… {hidden_before_count} unchanged lines hidden"),
        right_text: format!("… {hidden_after_count} unchanged lines hidden"),
        kind: DiffLineKind::Context,
    });
}

fn push_context_diff_lines(
    output: &mut Vec<DiffLine>,
    before_rope: &Rope,
    after_rope: &Rope,
    before_start: usize,
    before_end: usize,
    after_start: usize,
    after_end: usize,
) {
    let before_count = before_end.saturating_sub(before_start);
    let after_count = after_end.saturating_sub(after_start);
    let paired_count = before_count.min(after_count);

    for offset in 0..paired_count {
        push_diff_line(
            output,
            before_rope,
            after_rope,
            Some(before_start + offset),
            Some(after_start + offset),
            DiffLineKind::Context,
        );
    }

    for offset in paired_count..before_count {
        push_diff_line(
            output,
            before_rope,
            after_rope,
            Some(before_start + offset),
            None,
            DiffLineKind::Removed,
        );
    }

    for offset in paired_count..after_count {
        push_diff_line(
            output,
            before_rope,
            after_rope,
            None,
            Some(after_start + offset),
            DiffLineKind::Added,
        );
    }
}

fn push_diff_line(
    output: &mut Vec<DiffLine>,
    before_rope: &Rope,
    after_rope: &Rope,
    left_index: Option<usize>,
    right_index: Option<usize>,
    kind: DiffLineKind,
) {
    output.push(DiffLine {
        left_line_number: left_index.map(|index| index + 1),
        right_line_number: right_index.map(|index| index + 1),
        left_text: left_index
            .map(|index| rope_display_line(before_rope, index))
            .unwrap_or_default(),
        right_text: right_index
            .map(|index| rope_display_line(after_rope, index))
            .unwrap_or_default(),
        kind,
    });
}

fn rope_display_line(rope: &Rope, line_index: usize) -> String {
    if line_index >= rope.len_lines() {
        return String::new();
    }

    let mut text = rope.line(line_index).to_string();
    while text.ends_with('\n') || text.ends_with('\r') {
        let _ = text.pop();
    }
    text.replace('\t', "    ")
}

fn format_log_entry(entry: &log_layer::LogEntry) -> String {
    let timestamp = entry
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = timestamp.as_secs();
    let millis = timestamp.subsec_millis();
    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    let level_str = match entry.level {
        tracing::Level::ERROR => "ERROR",
        tracing::Level::WARN => "WARN ",
        tracing::Level::INFO => "INFO ",
        tracing::Level::DEBUG => "DEBUG",
        tracing::Level::TRACE => "TRACE",
    };
    let message = if entry.fields.is_empty() {
        entry.message.clone()
    } else {
        let fields_str: Vec<String> = entry
            .fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        format!("{} {}", entry.message, fields_str.join(" "))
    };
    format!(
        "{hours:02}:{minutes:02}:{seconds:02}.{millis:03} {level_str} {} {message}",
        entry.target
    )
}

fn render_log_row(entry: &log_layer::LogEntry, index: usize, theme: ThemePalette) -> Div {
    let timestamp = entry
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = timestamp.as_secs();
    let millis = timestamp.subsec_millis();
    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    let time_str = format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}");

    let (level_str, level_color) = match entry.level {
        tracing::Level::ERROR => ("ERROR", 0xf38ba8_u32),
        tracing::Level::WARN => ("WARN ", 0xf9e2af),
        tracing::Level::INFO => ("INFO ", 0xa6e3a1),
        tracing::Level::DEBUG => ("DEBUG", 0x89b4fa),
        tracing::Level::TRACE => ("TRACE", 0x9399b2),
    };

    let target = truncate_with_ellipsis(&entry.target, 30);
    let bg = if index.is_multiple_of(2) {
        theme.terminal_bg
    } else {
        theme.sidebar_bg
    };

    div()
        .py(px(2.))
        .w_full()
        .flex()
        .items_start()
        .gap_2()
        .px_2()
        .font_family(FONT_MONO)
        .text_size(px(DIFF_FONT_SIZE_PX))
        .bg(rgb(bg))
        .child(
            div()
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(time_str),
        )
        .child(
            div()
                .flex_none()
                .w(px(40.))
                .text_color(rgb(level_color))
                .child(level_str),
        )
        .child(
            div()
                .flex_none()
                .w(px(200.))
                .text_color(rgb(theme.text_muted))
                .overflow_hidden()
                .child(target),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(rgb(theme.text_primary))
                .child(if entry.fields.is_empty() {
                    entry.message.clone()
                } else {
                    let fields_str: Vec<String> = entry
                        .fields
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect();
                    format!("{} {}", entry.message, fields_str.join(" "))
                }),
        )
}

fn render_diff_session(
    session: DiffSession,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> Div {
    let path_label = truncate_middle_text(&session.title, 84);
    let line_count = session.lines.len();
    let is_loading = session.is_loading;
    let session_id = session.id;

    div()
        .h_full()
        .w_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(28.))
                .px_3()
                .bg(rgb(theme.tab_active_bg))
                .border_b_1()
                .border_color(rgb(theme.border))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .font(mono_font.clone())
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(theme.text_muted))
                        .child(path_label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_disabled))
                        .child(if is_loading {
                            "loading...".to_owned()
                        } else {
                            format!("{line_count} rows")
                        }),
                ),
        )
        .child(
            div()
                .id(("diff-scroll", session_id))
                .flex_1()
                .min_h_0()
                .bg(rgb(theme.terminal_bg))
                .when(is_loading, |this| {
                    this.child(
                        div()
                            .h_full()
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .text_sm()
                            .text_color(rgb(theme.text_muted))
                            .child("Computing diff..."),
                    )
                })
                .when(!is_loading, |this| {
                    let lines = session.lines.clone();
                    let zonemap_lines = lines.clone();
                    let scroll_handle = scroll_handle.clone();
                    let mono_font = mono_font.clone();
                    this.child(
                        div()
                            .size_full()
                            .min_w_0()
                            .flex()
                            .child(
                                uniform_list(
                                    ("diff-list", session_id),
                                    lines.len(),
                                    move |range, _, _| {
                                        range
                                            .map(|index| {
                                                render_diff_row(
                                                    session_id,
                                                    index,
                                                    lines[index].clone(),
                                                    theme,
                                                    mono_font.clone(),
                                                    diff_cell_width,
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                )
                                .h_full()
                                .flex_1()
                                .min_w_0()
                                .track_scroll(scroll_handle.clone()),
                            )
                            .child(render_diff_zonemap(zonemap_lines, theme, &scroll_handle)),
                    )
                }),
        )
}

fn render_diff_row(
    session_id: u64,
    row_index: usize,
    line: DiffLine,
    theme: ThemePalette,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> impl IntoElement {
    if line.kind == DiffLineKind::FileHeader {
        return div()
            .id(diff_row_element_id(
                "diff-row-header",
                session_id,
                row_index,
            ))
            .w_full()
            .h(px(DIFF_ROW_HEIGHT_PX))
            .min_h(px(DIFF_ROW_HEIGHT_PX))
            .bg(rgb(theme.tab_active_bg))
            .px_2()
            .flex()
            .items_center()
            .child(
                div()
                    .min_w_0()
                    .font(mono_font)
                    .text_size(px(DIFF_FONT_SIZE_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .text_color(rgb(theme.text_primary))
                    .child(line.left_text),
            );
    }

    let (left_bg, right_bg) = diff_line_backgrounds(line.kind, theme);
    let (left_marker, right_marker) = diff_line_markers(line.kind);
    let (left_text_color, right_text_color) = diff_line_text_colors(line.kind, theme);
    div()
        .id(diff_row_element_id("diff-row", session_id, row_index))
        .w_full()
        .min_w_0()
        .h(px(DIFF_ROW_HEIGHT_PX))
        .min_h(px(DIFF_ROW_HEIGHT_PX))
        .flex()
        .child(render_diff_column(
            session_id,
            row_index,
            0,
            line.left_line_number,
            line.left_text,
            left_marker,
            left_bg,
            left_text_color,
            theme,
            mono_font.clone(),
            diff_cell_width,
        ))
        .child(render_diff_column(
            session_id,
            row_index,
            1,
            line.right_line_number,
            line.right_text,
            right_marker,
            right_bg,
            right_text_color,
            theme,
            mono_font,
            diff_cell_width,
        ))
}

fn render_diff_column(
    session_id: u64,
    row_index: usize,
    side: usize,
    line_number: Option<usize>,
    text: String,
    marker: char,
    background: u32,
    text_color: u32,
    theme: ThemePalette,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> impl IntoElement {
    let number_width = px((DIFF_LINE_NUMBER_WIDTH_CHARS as f32 * diff_cell_width) + 12.);

    let column_id = diff_row_side_element_id("diff-column", session_id, row_index, side);
    let marker_id = diff_row_side_element_id("diff-marker", session_id, row_index, side);
    let text_id = diff_row_side_element_id("diff-text", session_id, row_index, side);

    div()
        .id(column_id)
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(rgb(background))
        .child(
            div()
                .h_full()
                .min_w_0()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(number_width)
                        .flex_none()
                        .text_right()
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(theme.text_disabled))
                        .child(line_number.map_or(String::new(), |line| line.to_string())),
                )
                .child(
                    div()
                        .id(marker_id)
                        .w(px(10.))
                        .flex_none()
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(diff_marker_color(marker)))
                        .child(marker.to_string()),
                )
                .child(
                    div()
                        .id(text_id)
                        .min_w_0()
                        .flex_1()
                        .font(mono_font)
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .whitespace_nowrap()
                        .text_color(rgb(text_color))
                        .child(if text.is_empty() {
                            " ".to_owned()
                        } else {
                            text
                        }),
                ),
        )
}

fn render_diff_zonemap(
    lines: Arc<[DiffLine]>,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
) -> Div {
    let scroll_handle_for_draw = scroll_handle.clone();
    let scroll_handle_for_click = scroll_handle.clone();
    let scroll_handle_for_drag = scroll_handle.clone();
    let total_rows = lines.len();
    let marker_spans = build_zonemap_marker_spans(lines.as_ref());

    div()
        .h_full()
        .w(px(DIFF_ZONEMAP_WIDTH_PX + (DIFF_ZONEMAP_MARGIN_PX * 2.)))
        .pt(px(DIFF_ZONEMAP_MARGIN_PX))
        .pb(px(DIFF_ZONEMAP_MARGIN_PX))
        .pl(px(DIFF_ZONEMAP_MARGIN_PX))
        .pr(px(DIFF_ZONEMAP_MARGIN_PX))
        .flex_none()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, _| {
            if total_rows == 0 {
                return;
            }

            let bounds = scroll_handle_for_click.0.borrow().base_handle.bounds();
            let height = bounds.size.height.to_f64() as f32;
            if !height.is_finite() || height <= 0. {
                return;
            }

            let relative_y = (f32::from(event.position.y - bounds.top()) / height).clamp(0., 1.);
            let mut target_row = (relative_y * total_rows as f32).floor() as usize;
            if target_row >= total_rows {
                target_row = total_rows.saturating_sub(1);
            }
            scroll_handle_for_click.scroll_to_item(target_row, ScrollStrategy::Center);
        })
        .on_mouse_move(move |event: &MouseMoveEvent, _, _| {
            if event.pressed_button != Some(MouseButton::Left) || total_rows == 0 {
                return;
            }

            let bounds = scroll_handle_for_drag.0.borrow().base_handle.bounds();
            let height = bounds.size.height.to_f64() as f32;
            if !height.is_finite() || height <= 0. {
                return;
            }

            let relative_y = (f32::from(event.position.y - bounds.top()) / height).clamp(0., 1.);
            let mut target_row = (relative_y * total_rows as f32).floor() as usize;
            if target_row >= total_rows {
                target_row = total_rows.saturating_sub(1);
            }
            scroll_handle_for_drag.scroll_to_item(target_row, ScrollStrategy::Center);
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    window.paint_quad(fill(bounds, rgb(theme.app_bg)));

                    let track_origin = point(bounds.origin.x + px(1.), bounds.origin.y + px(1.));
                    let track_size = size(
                        (bounds.size.width - px(2.)).max(px(1.)),
                        (bounds.size.height - px(2.)).max(px(1.)),
                    );
                    let track_bounds = Bounds::new(track_origin, track_size);
                    window.paint_quad(fill(track_bounds, rgb(theme.panel_bg)));

                    if total_rows == 0 {
                        return;
                    }

                    let height = track_bounds.size.height.to_f64() as f32;
                    if !height.is_finite() || height <= 0. {
                        return;
                    }

                    let marker_origin_x = track_bounds.origin.x + px(1.);
                    let marker_width = (track_bounds.size.width - px(2.)).max(px(1.));

                    for span in &marker_spans {
                        let start_ratio = span.start_row as f32 / total_rows as f32;
                        let end_ratio = span.end_row.saturating_add(1) as f32 / total_rows as f32;
                        let y = track_bounds.origin.y + px(start_ratio * height);
                        let marker_height =
                            px(((end_ratio - start_ratio) * height)
                                .max(DIFF_ZONEMAP_MARKER_HEIGHT_PX));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(marker_origin_x, y),
                                size(marker_width, marker_height),
                            ),
                            rgb(span.color),
                        ));
                    }

                    let (visible_top, visible_bottom) =
                        diff_visible_row_range(&scroll_handle_for_draw, total_rows);
                    let visible_count =
                        visible_bottom.saturating_sub(visible_top).saturating_add(1);
                    let thumb_top_ratio = visible_top as f32 / total_rows as f32;
                    let thumb_height_ratio = visible_count as f32 / total_rows as f32;
                    let thumb_height = px((thumb_height_ratio * height)
                        .max(DIFF_ZONEMAP_MIN_THUMB_HEIGHT_PX)
                        .min(height));
                    let max_thumb_top =
                        track_bounds.origin.y + track_bounds.size.height - thumb_height;
                    let thumb_top = (track_bounds.origin.y + px(thumb_top_ratio * height))
                        .min(max_thumb_top)
                        .max(track_bounds.origin.y);

                    window.paint_quad(fill(
                        Bounds::new(
                            point(track_bounds.origin.x, thumb_top),
                            size(track_bounds.size.width, thumb_height),
                        ),
                        rgb(theme.accent),
                    ));
                },
            )
            .size_full(),
        )
}

#[derive(Debug, Clone, Copy)]
struct ZonemapMarkerSpan {
    start_row: usize,
    end_row: usize,
    color: u32,
}

fn build_zonemap_marker_spans(lines: &[DiffLine]) -> Vec<ZonemapMarkerSpan> {
    let mut spans: Vec<ZonemapMarkerSpan> = Vec::new();

    for (row, line) in lines.iter().enumerate() {
        let Some(color) = zonemap_marker_color(line.kind) else {
            continue;
        };

        if let Some(last) = spans.last_mut()
            && last.color == color
            && row == last.end_row.saturating_add(1)
        {
            last.end_row = row;
            continue;
        }

        spans.push(ZonemapMarkerSpan {
            start_row: row,
            end_row: row,
            color,
        });
    }

    spans
}

fn diff_visible_row_range(
    scroll_handle: &UniformListScrollHandle,
    total_rows: usize,
) -> (usize, usize) {
    if total_rows == 0 {
        return (0, 0);
    }

    let state = scroll_handle.0.borrow();
    let max_row = total_rows.saturating_sub(1);
    let viewport_height = f32::from(state.base_handle.bounds().size.height).max(0.);
    let scroll_offset_y = (-f32::from(state.base_handle.offset().y)).max(0.);

    let top = (scroll_offset_y / DIFF_ROW_HEIGHT_PX).floor() as usize;
    let visible_rows = ((viewport_height / DIFF_ROW_HEIGHT_PX).ceil() as usize).max(1);
    let bottom = top.saturating_add(visible_rows.saturating_sub(1));

    let clamped_top = top.min(max_row);
    let clamped_bottom = bottom.min(max_row);
    (clamped_top, clamped_bottom.max(clamped_top))
}

fn zonemap_marker_color(kind: DiffLineKind) -> Option<u32> {
    match kind {
        DiffLineKind::FileHeader => Some(0x6d88a6),
        DiffLineKind::Added => Some(0x72d69c),
        DiffLineKind::Removed => Some(0xeb6f92),
        DiffLineKind::Modified => Some(0xf9e2af),
        DiffLineKind::Context => None,
    }
}

fn diff_line_backgrounds(kind: DiffLineKind, theme: ThemePalette) -> (u32, u32) {
    match kind {
        DiffLineKind::FileHeader => (theme.tab_active_bg, theme.tab_active_bg),
        DiffLineKind::Context
        | DiffLineKind::Added
        | DiffLineKind::Removed
        | DiffLineKind::Modified => (theme.terminal_bg, theme.terminal_bg),
    }
}

fn diff_line_text_colors(kind: DiffLineKind, theme: ThemePalette) -> (u32, u32) {
    match kind {
        DiffLineKind::FileHeader => (theme.text_primary, theme.text_primary),
        DiffLineKind::Context => (theme.text_primary, theme.text_primary),
        DiffLineKind::Added => (theme.text_disabled, 0x8fd7ad),
        DiffLineKind::Removed => (0xf2a4b7, theme.text_disabled),
        DiffLineKind::Modified => (0xf2a4b7, 0x8fd7ad),
    }
}

fn diff_line_markers(kind: DiffLineKind) -> (char, char) {
    match kind {
        DiffLineKind::FileHeader => (' ', ' '),
        DiffLineKind::Context => (' ', ' '),
        DiffLineKind::Added => (' ', '+'),
        DiffLineKind::Removed => ('-', ' '),
        DiffLineKind::Modified => ('-', '+'),
    }
}

fn diff_marker_color(marker: char) -> u32 {
    match marker {
        '+' => 0x72d69c,
        '-' => 0xeb6f92,
        '~' => 0xf9e2af,
        _ => 0x7c8599,
    }
}

fn wrap_diff_document_lines(
    raw_lines: &[DiffLine],
    raw_file_row_indices: &HashMap<PathBuf, usize>,
    wrap_columns: usize,
) -> (Vec<DiffLine>, HashMap<PathBuf, usize>) {
    let mut wrapped_lines = Vec::new();
    let mut raw_to_wrapped_index = Vec::with_capacity(raw_lines.len());

    for raw_line in raw_lines {
        raw_to_wrapped_index.push(wrapped_lines.len());
        wrapped_lines.extend(wrap_diff_line(raw_line.clone(), wrap_columns));
    }

    let wrapped_file_row_indices = raw_file_row_indices
        .iter()
        .map(|(path, raw_index)| {
            let wrapped_index = raw_to_wrapped_index.get(*raw_index).copied().unwrap_or(0);
            (path.clone(), wrapped_index)
        })
        .collect::<HashMap<_, _>>();

    (wrapped_lines, wrapped_file_row_indices)
}

fn wrap_diff_line(line: DiffLine, wrap_columns: usize) -> Vec<DiffLine> {
    let wrap_columns = wrap_columns.max(1);
    if line.kind == DiffLineKind::FileHeader {
        return split_diff_text_chunks(line.left_text, wrap_columns.saturating_mul(2))
            .into_iter()
            .map(|chunk| DiffLine {
                left_line_number: None,
                right_line_number: None,
                left_text: chunk,
                right_text: String::new(),
                kind: DiffLineKind::FileHeader,
            })
            .collect();
    }

    let left_chunks = split_diff_text_chunks(line.left_text, wrap_columns);
    let right_chunks = split_diff_text_chunks(line.right_text, wrap_columns);
    let chunk_count = left_chunks.len().max(right_chunks.len()).max(1);
    let mut wrapped = Vec::with_capacity(chunk_count);

    for index in 0..chunk_count {
        wrapped.push(DiffLine {
            left_line_number: (index == 0).then_some(line.left_line_number).flatten(),
            right_line_number: (index == 0).then_some(line.right_line_number).flatten(),
            left_text: left_chunks.get(index).cloned().unwrap_or_default(),
            right_text: right_chunks.get(index).cloned().unwrap_or_default(),
            kind: line.kind,
        });
    }

    wrapped
}

fn split_diff_text_chunks(text: String, wrap_columns: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let wrap_columns = wrap_columns.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0_usize;

    for ch in text.chars() {
        current.push(ch);
        current_len += 1;

        if current_len >= wrap_columns {
            chunks.push(current);
            current = String::new();
            current_len = 0;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    if chunks.is_empty() {
        chunks.push(String::new());
    }

    chunks
}

fn diff_row_element_id(prefix: &'static str, session_id: u64, row_index: usize) -> ElementId {
    let session_scope = ElementId::from((prefix, session_id));
    ElementId::from((session_scope, row_index.to_string()))
}

fn diff_row_side_element_id(
    prefix: &'static str,
    session_id: u64,
    row_index: usize,
    side: usize,
) -> ElementId {
    let row_scope = diff_row_element_id(prefix, session_id, row_index);
    ElementId::from((row_scope, side.to_string()))
}

fn truncate_with_ellipsis(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let mut chars = value.chars();
    let mut output = String::new();

    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return value.to_owned();
        };
        output.push(ch);
    }

    if chars.next().is_some() {
        output.push_str("...");
    }

    output
}

fn action_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    active: bool,
    muted: bool,
) -> Stateful<Div> {
    let background = if active {
        theme.panel_active_bg
    } else {
        theme.panel_bg
    };
    let text_color = if muted {
        theme.text_disabled
    } else {
        theme.text_primary
    };

    div()
        .id(id)
        .cursor_pointer()
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(background))
        .px_2()
        .py_1()
        .text_xs()
        .text_color(rgb(text_color))
        .child(label.into())
}

fn git_action_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    icon: &'static str,
    label: &'static str,
    enabled: bool,
    active: bool,
) -> Stateful<Div> {
    let background = if active {
        theme.panel_active_bg
    } else {
        theme.panel_bg
    };
    let icon_color = if active {
        theme.accent
    } else if enabled {
        theme.text_muted
    } else {
        theme.text_disabled
    };
    let text_color = if enabled || active {
        theme.text_primary
    } else {
        theme.text_disabled
    };

    div()
        .id(id)
        .h(px(24.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(background))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .when(enabled, |this| this.cursor_pointer())
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(px(13.))
                .text_color(rgb(icon_color))
                .child(icon),
        )
        .child(div().text_xs().text_color(rgb(text_color)).child(label))
}

fn modal_input_field(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    value: &str,
    placeholder: impl Into<String>,
    active: bool,
) -> Stateful<Div> {
    let label = label.into();
    let placeholder = placeholder.into();

    div()
        .id(id)
        .cursor_pointer()
        .rounded_sm()
        .border_1()
        .border_color(rgb(if active {
            theme.accent
        } else {
            theme.border
        }))
        .bg(rgb(theme.panel_bg))
        .p_2()
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(
            div()
                .text_sm()
                .font_family(FONT_MONO)
                .text_color(rgb(if value.is_empty() {
                    theme.text_disabled
                } else {
                    theme.text_primary
                }))
                .child(if value.is_empty() {
                    placeholder
                } else {
                    value.to_owned()
                }),
        )
}

fn status_text(theme: ThemePalette, text: impl Into<String>) -> Div {
    div()
        .text_xs()
        .text_color(rgb(theme.text_muted))
        .child(text.into())
}

fn file_icon_and_color(name: &str, is_dir: bool) -> (&'static str, u32) {
    if is_dir {
        return ("\u{f07b}", 0xe5c07b);
    }

    // Check full filename first
    match name {
        "Dockerfile" | ".dockerignore" => return ("\u{e7b0}", 0x61afef),
        "Makefile" | "Justfile" => return ("\u{e615}", 0x98c379),
        ".gitignore" | ".env" => return ("\u{e615}", 0x838994),
        _ => {},
    }

    // Check extension
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => ("\u{e7a8}", 0xe06c75),
        "toml" => ("\u{e615}", 0x838994),
        "py" => ("\u{e73c}", 0x61afef),
        "js" => ("\u{e74e}", 0xe5c07b),
        "ts" => ("\u{e628}", 0x61afef),
        "jsx" | "tsx" => ("\u{e7ba}", 0x56b6c2),
        "json" => ("\u{e60b}", 0xe5c07b),
        "html" => ("\u{e736}", 0xe06c75),
        "css" | "scss" | "sass" => ("\u{e749}", 0x56b6c2),
        "md" | "mdx" => ("\u{e73e}", 0x61afef),
        "yaml" | "yml" => ("\u{e615}", 0xc678dd),
        "sh" | "bash" | "zsh" => ("\u{e795}", 0x98c379),
        "go" => ("\u{e627}", 0x56b6c2),
        "c" | "h" => ("\u{e61e}", 0x61afef),
        "cpp" | "hpp" | "cc" => ("\u{e61d}", 0xe06c75),
        "java" => ("\u{e738}", 0xe06c75),
        "rb" => ("\u{e739}", 0xe06c75),
        "swift" => ("\u{e755}", 0xe06c75),
        "lock" => ("\u{f023}", 0x838994),
        "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" => ("\u{f1c5}", 0xc678dd),
        "txt" | "log" => ("\u{f15c}", 0x838994),
        "xml" => ("\u{e619}", 0xe5c07b),
        "sql" => ("\u{f1c0}", 0xe5c07b),
        _ => ("\u{f15c}", 0x838994),
    }
}

fn change_code(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Removed => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::TypeChange => "T",
        ChangeKind::Conflict => "U",
        ChangeKind::IntentToAdd => "I",
    }
}

fn truncate_middle_path_for_width(path: &Path, right_pane_width: f32) -> String {
    let path_text = path.display().to_string();
    let available_width = (right_pane_width - 110.).max(120.);
    let max_chars = ((available_width / 7.3).floor() as usize).clamp(18, 96);
    truncate_middle_text(&path_text, max_chars)
}

fn truncate_middle_text(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_owned();
    }

    if max_chars <= 1 {
        return "…".to_owned();
    }

    let keep = max_chars - 1;
    let tail_keep = (keep * 3) / 5;
    let head_keep = keep.saturating_sub(tail_keep);
    let tail_start = chars.len().saturating_sub(tail_keep);

    let mut output = String::with_capacity(max_chars);
    output.extend(chars.iter().take(head_keep));
    output.push('…');
    output.extend(chars.iter().skip(tail_start));
    output
}

fn run_command_output(
    command: &mut Command,
    operation: &str,
) -> Result<std::process::Output, String> {
    command
        .output()
        .map_err(|error| format!("failed to run {operation}: {error}"))
}

fn command_failure_message(operation: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        return format!("{operation} failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stdout.is_empty() {
        return format!("{operation} failed: {stdout}");
    }

    match output.status.code() {
        Some(code) => format!("{operation} failed with exit code {code}"),
        None => format!("{operation} failed"),
    }
}

fn first_non_empty_output_line(buffer: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(buffer);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn auto_commit_subject(changed_files: &[ChangedFile]) -> String {
    if changed_files.len() == 1 {
        let file_label = changed_files[0]
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| changed_files[0].path.display().to_string());
        return format!("chore: update {file_label}");
    }

    let has_added = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Added | ChangeKind::IntentToAdd));
    let has_removed = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Removed));
    let has_renamed = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Renamed));
    let verb = if has_added && !has_removed && !has_renamed {
        "add"
    } else if has_removed && !has_added && !has_renamed {
        "remove"
    } else if has_renamed && !has_added && !has_removed {
        "rename"
    } else {
        "update"
    };

    format!("chore: {verb} {} files", changed_files.len())
}

fn auto_commit_body(changed_files: &[ChangedFile]) -> String {
    let mut lines = vec!["Auto-generated by Arbor.".to_owned(), String::new()];

    for change in changed_files.iter().take(12) {
        let mut line = format!("- {} {}", change_code(change.kind), change.path.display());
        if change.additions > 0 || change.deletions > 0 {
            line.push_str(&format!(" (+{} -{})", change.additions, change.deletions));
        }
        lines.push(line);
    }

    if changed_files.len() > 12 {
        lines.push(format!("- ... and {} more", changed_files.len() - 12));
    }

    lines.join("\n")
}

fn run_git_commit_for_worktree(
    worktree_path: &Path,
    changed_files: &[ChangedFile],
) -> Result<String, String> {
    if changed_files.is_empty() {
        return Err("nothing to commit".to_owned());
    }

    let mut add_command = Command::new("git");
    add_command.arg("-C").arg(worktree_path).args(["add", "-A"]);
    let add_output = run_command_output(&mut add_command, "git add")?;
    if !add_output.status.success() {
        return Err(command_failure_message("git add", &add_output));
    }

    let subject = auto_commit_subject(changed_files);
    let body = auto_commit_body(changed_files);
    let mut commit_command = Command::new("git");
    commit_command
        .arg("-C")
        .arg(worktree_path)
        .arg("commit")
        .arg("-m")
        .arg(subject.as_str())
        .arg("-m")
        .arg(body.as_str());

    let commit_output = run_command_output(&mut commit_command, "git commit")?;
    if !commit_output.status.success() {
        let failure = command_failure_message("git commit", &commit_output);
        if failure.to_ascii_lowercase().contains("nothing to commit") {
            return Err("nothing to commit".to_owned());
        }
        return Err(failure);
    }

    let summary = first_non_empty_output_line(&commit_output.stdout)
        .or_else(|| first_non_empty_output_line(&commit_output.stderr))
        .unwrap_or_else(|| subject.clone());
    Ok(format!("commit complete: {summary}"))
}

fn run_git_push_for_worktree(worktree_path: &Path) -> Result<String, String> {
    let mut push_command = Command::new("git");
    push_command
        .arg("-C")
        .arg(worktree_path)
        .args(["push", "--set-upstream", "origin", "HEAD"]);
    let output = run_command_output(&mut push_command, "git push")?;
    if !output.status.success() {
        return Err(command_failure_message("git push", &output));
    }

    let summary = first_non_empty_output_line(&output.stderr)
        .or_else(|| first_non_empty_output_line(&output.stdout))
        .unwrap_or_else(|| "pushed current branch".to_owned());
    Ok(format!("push complete: {summary}"))
}

fn git_branch_name_for_worktree(worktree_path: &Path) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(worktree_path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"]);
    let output = run_command_output(&mut command, "git rev-parse")?;
    if !output.status.success() {
        return Err(command_failure_message("git rev-parse", &output));
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if branch.is_empty() || branch == "HEAD" {
        return Err("cannot create a PR from detached HEAD".to_owned());
    }

    Ok(branch)
}

fn git_has_tracking_branch(worktree_path: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])
        .output();

    matches!(output, Ok(output) if output.status.success())
}

fn git_default_base_branch(worktree_path: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree_path)
        .args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let base = trimmed.strip_prefix("origin/").unwrap_or(trimmed);
    if base.is_empty() {
        return None;
    }

    Some(base.to_owned())
}

fn run_create_pr_for_worktree(
    worktree_path: &Path,
    repo_slug: Option<&str>,
) -> Result<String, String> {
    if !git_has_tracking_branch(worktree_path) {
        return Err("push the branch before creating a PR".to_owned());
    }

    let branch = git_branch_name_for_worktree(worktree_path)?;
    let mut command = Command::new("gh");
    command
        .current_dir(worktree_path)
        .arg("pr")
        .arg("create")
        .arg("--fill")
        .arg("--head")
        .arg(branch.as_str());

    if let Some(base_branch) = git_default_base_branch(worktree_path) {
        command.arg("--base").arg(base_branch);
    }
    if let Some(repo_slug) = repo_slug {
        command.arg("--repo").arg(repo_slug);
    }

    let output = run_command_output(&mut command, "gh pr create")?;
    if !output.status.success() {
        return Err(command_failure_message("gh pr create", &output));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    if let Some(url) = extract_first_url(&combined) {
        return Ok(format!("created PR: {url}"));
    }

    Ok("pull request created".to_owned())
}

fn extract_first_url(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|token| {
        let trimmed =
            token.trim_matches(|character: char| matches!(character, '"' | '\'' | ',' | '.'));
        if trimmed.starts_with("https://") {
            Some(trimmed.to_owned())
        } else {
            None
        }
    })
}

fn github_repo_slug_for_repo(repo_root: &Path) -> Option<String> {
    let remote_url = git_origin_remote_url(repo_root)?;
    github_repo_slug_from_remote_url(remote_url.trim())
}

fn github_avatar_url_for_repo_slug(repo_slug: &str) -> Option<String> {
    let (owner, _) = repo_slug.split_once('/')?;
    Some(format!(
        "https://avatars.githubusercontent.com/{owner}?size=96"
    ))
}

fn git_origin_remote_url(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let remote = String::from_utf8(output.stdout).ok()?;
    let trimmed = remote.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.to_owned())
}

fn github_repo_slug_from_remote_url(remote_url: &str) -> Option<String> {
    if let Some(path) = remote_url.strip_prefix("git@github.com:") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("https://github.com/") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("http://github.com/") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("ssh://git@github.com/") {
        return github_repo_slug_from_path(path);
    }

    None
}

fn github_repo_slug_from_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let repository_path = normalized.strip_suffix(".git").unwrap_or(normalized);
    let (owner, repository) = repository_path.split_once('/')?;
    if owner.is_empty() || repository.is_empty() {
        return None;
    }

    Some(format!("{owner}/{repository}"))
}

fn github_pr_number_for_worktree(worktree_path: &Path, branch: &str) -> Option<u64> {
    if branch.trim().is_empty() || branch == "-" {
        return None;
    }

    github_pr_number_by_tracking_branch(worktree_path)
        .or_else(|| github_pr_number_by_head_branch(worktree_path, branch))
}

fn should_lookup_pull_request_for_worktree(worktree: &WorktreeSummary) -> bool {
    if worktree.is_primary_checkout {
        return false;
    }

    let branch = worktree.branch.as_str();
    if branch == "-" || branch.is_empty() {
        return false;
    }

    !(branch.eq_ignore_ascii_case("main")
        || branch.eq_ignore_ascii_case("master")
        || branch.eq_ignore_ascii_case("develop")
        || branch.eq_ignore_ascii_case("dev")
        || branch.eq_ignore_ascii_case("trunk"))
}

fn github_pr_number_by_tracking_branch(worktree_path: &Path) -> Option<u64> {
    let output = Command::new("gh")
        .current_dir(worktree_path)
        .args(["pr", "view", "--json", "number", "--jq", ".number // empty"])
        .output()
        .ok()?;

    parse_github_pr_number_output(output)
}

fn github_pr_number_by_head_branch(worktree_path: &Path, branch: &str) -> Option<u64> {
    let output = Command::new("gh")
        .current_dir(worktree_path)
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "all",
            "--json",
            "number",
            "--jq",
            ".[0].number // empty",
        ])
        .output()
        .ok()?;

    parse_github_pr_number_output(output)
}

fn parse_github_pr_number_output(output: std::process::Output) -> Option<u64> {
    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    trimmed.parse::<u64>().ok()
}

fn github_pr_url(repo_slug: &str, pr_number: u64) -> String {
    format!("https://github.com/{repo_slug}/pull/{pr_number}")
}

fn repository_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn short_branch(value: &str) -> String {
    worktree::short_branch(value)
}

fn expand_home_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("repository path cannot be empty".to_owned());
    }

    if trimmed == "~" {
        return user_home_dir();
    }

    if let Some(suffix) = trimmed.strip_prefix("~/") {
        return user_home_dir().map(|home| home.join(suffix));
    }

    Ok(PathBuf::from(trimmed))
}

fn user_home_dir() -> Result<PathBuf, String> {
    env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "HOME environment variable is not set".to_owned())
}

fn sanitize_worktree_name(value: &str) -> String {
    let mut sanitized = String::new();
    let mut previous_dash = false;

    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            sanitized.push(character.to_ascii_lowercase());
            previous_dash = false;
            continue;
        }

        if character == '-' || character == '_' || character == '.' {
            sanitized.push(character);
            previous_dash = false;
            continue;
        }

        if !previous_dash && !sanitized.is_empty() {
            sanitized.push('-');
            previous_dash = true;
        }
    }

    while sanitized.ends_with('-') {
        let _ = sanitized.pop();
    }

    sanitized
}

fn derive_branch_name(worktree_name: &str) -> String {
    let sanitized = sanitize_worktree_name(worktree_name);
    if sanitized.is_empty() {
        "worktree".to_owned()
    } else {
        sanitized
    }
}

fn build_managed_worktree_path(repo_name: &str, worktree_name: &str) -> Result<PathBuf, String> {
    let home_dir = user_home_dir()?;
    Ok(home_dir
        .join(".arbor")
        .join("worktrees")
        .join(repo_name)
        .join(worktree_name))
}

fn preview_managed_worktree_path(
    repository_path: &str,
    worktree_name: &str,
) -> Result<String, String> {
    let repository_path = expand_home_path(repository_path)?;
    let repository_name = repository_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "repository name cannot be determined".to_owned())?;
    let sanitized_worktree = sanitize_worktree_name(worktree_name);
    if sanitized_worktree.is_empty() {
        return Err("invalid worktree name".to_owned());
    }

    let path = build_managed_worktree_path(repository_name, &sanitized_worktree)?;
    Ok(path.display().to_string())
}

fn create_managed_worktree(
    repository_path_input: String,
    worktree_name_input: String,
) -> Result<CreatedWorktree, String> {
    let repository_path = expand_home_path(&repository_path_input)?;
    if !repository_path.exists() {
        return Err(format!(
            "repository path does not exist: {}",
            repository_path.display()
        ));
    }

    let repository_root = worktree::repo_root(&repository_path)
        .map_err(|error| format!("failed to resolve repository root: {error}"))?;
    let repository_name = repository_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "repository root has no terminal directory name".to_owned())?;

    let sanitized_worktree_name = sanitize_worktree_name(&worktree_name_input);
    if sanitized_worktree_name.is_empty() {
        return Err("worktree name contains no usable characters".to_owned());
    }

    let branch_name = derive_branch_name(&worktree_name_input);
    let worktree_path = build_managed_worktree_path(repository_name, &sanitized_worktree_name)?;
    if worktree_path.exists() {
        return Err(format!(
            "worktree path already exists: {}",
            worktree_path.display()
        ));
    }

    let Some(parent_directory) = worktree_path.parent() else {
        return Err("invalid worktree path".to_owned());
    };
    fs::create_dir_all(parent_directory).map_err(|error| {
        format!(
            "failed to create worktree parent directory `{}`: {error}",
            parent_directory.display()
        )
    })?;

    worktree::add(
        &repository_root,
        &worktree_path,
        worktree::AddWorktreeOptions {
            branch: Some(&branch_name),
            detach: false,
            force: false,
        },
    )
    .map_err(|error| format!("failed to create worktree: {error}"))?;

    Ok(CreatedWorktree {
        worktree_name: sanitized_worktree_name,
        branch_name,
        worktree_path,
    })
}

fn styled_lines_for_session(
    session: &TerminalSession,
    theme: ThemePalette,
    show_cursor: bool,
    selection: Option<&TerminalSelection>,
) -> Vec<TerminalStyledLine> {
    let mut lines = if !session.styled_output.is_empty() {
        session.styled_output.clone()
    } else {
        plain_lines_to_styled(lines_for_display(&session.output), theme)
    };

    for line in &mut lines {
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        } else if line.runs.is_empty() && !line.cells.is_empty() {
            line.runs = runs_from_cells(&line.cells);
        }

        let mut changed = false;
        for cell in &mut line.cells {
            if cell.bg == EMBEDDED_TERMINAL_DEFAULT_BG {
                cell.bg = theme.terminal_bg;
                changed = true;
            }
            if cell.fg == EMBEDDED_TERMINAL_DEFAULT_FG {
                cell.fg = theme.text_primary;
                changed = true;
            }
        }

        if changed {
            line.runs = runs_from_cells(&line.cells);
        }
    }

    if show_cursor
        && session.state == TerminalState::Running
        && let Some(cursor) = session.cursor
    {
        apply_cursor_to_lines(&mut lines, cursor, theme);
    }

    if let Some(selection) = selection.filter(|selection| selection.session_id == session.id) {
        apply_selection_to_lines(&mut lines, selection, theme);
    }

    lines
}

fn apply_cursor_to_lines(
    lines: &mut Vec<TerminalStyledLine>,
    cursor: TerminalCursor,
    theme: ThemePalette,
) {
    while lines.len() <= cursor.line {
        lines.push(TerminalStyledLine {
            cells: Vec::new(),
            runs: Vec::new(),
        });
    }

    if let Some(line) = lines.get_mut(cursor.line) {
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        }

        let insert_index = line
            .cells
            .iter()
            .position(|cell| cell.column >= cursor.column)
            .unwrap_or(line.cells.len());

        if line
            .cells
            .get(insert_index)
            .is_none_or(|cell| cell.column != cursor.column)
        {
            line.cells.insert(insert_index, TerminalStyledCell {
                column: cursor.column,
                text: " ".to_owned(),
                fg: theme.text_primary,
                bg: theme.terminal_bg,
            });
        }

        if let Some(cell) = line.cells.get_mut(insert_index) {
            if cell.text.is_empty() {
                cell.text = " ".to_owned();
            }

            if cell.text.chars().all(|character| character == ' ') {
                cell.fg = theme.text_primary;
            }
            cell.bg = theme.terminal_cursor;
        }

        line.runs = runs_from_cells(&line.cells);
    }
}

fn apply_selection_to_lines(
    lines: &mut Vec<TerminalStyledLine>,
    selection: &TerminalSelection,
    theme: ThemePalette,
) {
    let Some((start, end)) = normalized_terminal_selection(selection) else {
        return;
    };

    while lines.len() <= end.line {
        lines.push(TerminalStyledLine {
            cells: Vec::new(),
            runs: Vec::new(),
        });
    }

    for line_index in start.line..=end.line {
        let Some(line) = lines.get_mut(line_index) else {
            continue;
        };
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        }

        let line_start = if line_index == start.line {
            start.column
        } else {
            0
        };
        let line_end_exclusive = if line_index == end.line {
            end.column
        } else {
            usize::MAX
        };
        if line_end_exclusive <= line_start {
            continue;
        }

        let mut changed = false;
        for cell in &mut line.cells {
            if cell.column >= line_start && cell.column < line_end_exclusive {
                cell.fg = theme.terminal_selection_fg;
                cell.bg = theme.terminal_selection_bg;
                changed = true;
            }
        }

        if changed {
            line.runs = runs_from_cells(&line.cells);
        }
    }
}

fn normalized_terminal_selection(
    selection: &TerminalSelection,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let (start, end) = if selection.anchor.line < selection.head.line
        || (selection.anchor.line == selection.head.line
            && selection.anchor.column <= selection.head.column)
    {
        (selection.anchor, selection.head)
    } else {
        (selection.head, selection.anchor)
    };

    if start == end {
        return None;
    }

    Some((start, end))
}

fn cells_from_runs(runs: &[TerminalStyledRun]) -> Vec<TerminalStyledCell> {
    let mut cells = Vec::new();
    let mut column = 0_usize;
    for run in runs {
        for character in run.text.chars() {
            cells.push(TerminalStyledCell {
                column,
                text: character.to_string(),
                fg: run.fg,
                bg: run.bg,
            });
            column = column.saturating_add(1);
        }
    }
    cells
}

fn runs_from_cells(cells: &[TerminalStyledCell]) -> Vec<TerminalStyledRun> {
    let mut runs = Vec::new();
    let mut current_fg = None;
    let mut current_bg = None;
    let mut current_text = String::new();
    let mut next_expected_column: Option<usize> = None;
    let mut current_contains_complex_cell = false;
    let mut current_contains_decorative_cell = false;

    for cell in cells {
        let cell_is_complex = cell.text.chars().count() != 1;
        let cell_is_powerline = cell
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
            && cell.text.chars().count() == 1;
        let style_changed = current_fg != Some(cell.fg) || current_bg != Some(cell.bg);
        let gap_breaks_run = next_expected_column != Some(cell.column);
        let complex_breaks_run = current_contains_complex_cell || cell_is_complex;
        let decorative_breaks_run = current_contains_decorative_cell || cell_is_powerline;
        if style_changed || gap_breaks_run || complex_breaks_run || decorative_breaks_run {
            if let (Some(fg), Some(bg)) = (current_fg.take(), current_bg.take())
                && !current_text.is_empty()
            {
                runs.push(TerminalStyledRun {
                    text: std::mem::take(&mut current_text),
                    fg,
                    bg,
                });
            }

            current_fg = Some(cell.fg);
            current_bg = Some(cell.bg);
            current_contains_complex_cell = cell_is_complex;
            current_contains_decorative_cell = cell_is_powerline;
        }

        current_text.push_str(&cell.text);
        next_expected_column = Some(cell.column.saturating_add(1));
        current_contains_decorative_cell |= cell_is_powerline;
    }

    if let (Some(fg), Some(bg)) = (current_fg, current_bg)
        && !current_text.is_empty()
    {
        runs.push(TerminalStyledRun {
            text: current_text,
            fg,
            bg,
        });
    }

    runs
}

#[derive(Clone)]
struct PositionedTerminalRun {
    text: String,
    fg: u32,
    bg: u32,
    start_column: usize,
    cell_count: usize,
    force_cell_width: bool,
}

fn positioned_runs_from_cells(cells: &[TerminalStyledCell]) -> Vec<PositionedTerminalRun> {
    let mut runs = Vec::new();
    let mut current_fg: Option<u32> = None;
    let mut current_bg: Option<u32> = None;
    let mut current_start_column = 0_usize;
    let mut current_text = String::new();
    let mut next_expected_column: Option<usize> = None;
    let mut current_contains_complex_cell = false;
    let mut current_contains_decorative_cell = false;
    let mut current_cell_count = 0_usize;

    for cell in cells {
        let cell_is_complex = cell.text.chars().count() != 1;
        let cell_is_powerline = cell
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
            && cell.text.chars().count() == 1;
        let style_changed = current_fg != Some(cell.fg) || current_bg != Some(cell.bg);
        let gap_breaks_run = next_expected_column != Some(cell.column);
        let complex_breaks_run = current_contains_complex_cell || cell_is_complex;
        let decorative_breaks_run = current_contains_decorative_cell || cell_is_powerline;
        if style_changed || gap_breaks_run || complex_breaks_run || decorative_breaks_run {
            if let (Some(fg), Some(bg)) = (current_fg.take(), current_bg.take())
                && !current_text.is_empty()
            {
                runs.push(PositionedTerminalRun {
                    text: std::mem::take(&mut current_text),
                    fg,
                    bg,
                    start_column: current_start_column,
                    cell_count: current_cell_count,
                    force_cell_width: !current_contains_complex_cell
                        && !current_contains_decorative_cell,
                });
            }

            current_fg = Some(cell.fg);
            current_bg = Some(cell.bg);
            current_start_column = cell.column;
            current_contains_complex_cell = cell_is_complex;
            current_contains_decorative_cell = cell_is_powerline;
            current_cell_count = 0;
        }

        current_text.push_str(&cell.text);
        current_cell_count = current_cell_count.saturating_add(1);
        current_contains_complex_cell |= cell_is_complex;
        current_contains_decorative_cell |= cell_is_powerline;
        next_expected_column = Some(cell.column.saturating_add(1));
    }

    if let (Some(fg), Some(bg)) = (current_fg, current_bg)
        && !current_text.is_empty()
    {
        runs.push(PositionedTerminalRun {
            text: current_text,
            fg,
            bg,
            start_column: current_start_column,
            cell_count: current_cell_count,
            force_cell_width: !current_contains_complex_cell && !current_contains_decorative_cell,
        });
    }

    runs
}

fn is_terminal_powerline_character(ch: char) -> bool {
    matches!(ch as u32, 0xE0B0..=0xE0D7)
}

fn plain_lines_to_styled(lines: Vec<String>, theme: ThemePalette) -> Vec<TerminalStyledLine> {
    lines
        .into_iter()
        .map(|line| {
            let cells: Vec<TerminalStyledCell> = line
                .chars()
                .enumerate()
                .map(|(column, character)| TerminalStyledCell {
                    column,
                    text: character.to_string(),
                    fg: theme.text_primary,
                    bg: theme.terminal_bg,
                })
                .collect();

            let runs = if line.is_empty() {
                Vec::new()
            } else {
                vec![TerminalStyledRun {
                    text: line,
                    fg: theme.text_primary,
                    bg: theme.terminal_bg,
                }]
            };

            TerminalStyledLine { cells, runs }
        })
        .collect()
}

fn render_terminal_line(
    line: TerminalStyledLine,
    theme: ThemePalette,
    cell_width: f32,
    line_height: f32,
    mono_font: gpui::Font,
) -> Div {
    let cells = if line.cells.is_empty() {
        cells_from_runs(&line.runs)
    } else {
        line.cells
    };

    if cells.is_empty() {
        return div()
            .flex_none()
            .w_full()
            .min_w_0()
            .h(px(line_height))
            .overflow_x_hidden()
            .whitespace_nowrap()
            .font(mono_font)
            .text_size(px(TERMINAL_FONT_SIZE_PX))
            .line_height(px(line_height))
            .bg(rgb(theme.terminal_bg))
            .text_color(rgb(theme.text_primary))
            .child(" ");
    }

    let line_height = px(line_height);
    let font_size = px(TERMINAL_FONT_SIZE_PX);
    let positioned_runs = positioned_runs_from_cells(&cells);

    div()
        .flex_none()
        .w_full()
        .min_w_0()
        .h(line_height)
        .overflow_hidden()
        .bg(rgb(theme.terminal_bg))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let scale_factor = window.scale_factor();
                    for run in &positioned_runs {
                        if run.text.is_empty() {
                            continue;
                        }

                        if run.cell_count > 0 {
                            let start_x = snap_pixels_floor(
                                bounds.origin.x + px(run.start_column as f32 * cell_width),
                                scale_factor,
                            );
                            let end_x = snap_pixels_ceil(
                                bounds.origin.x
                                    + px((run.start_column + run.cell_count) as f32 * cell_width),
                                scale_factor,
                            );
                            let background_origin = point(start_x, bounds.origin.y);
                            let background_size = size((end_x - start_x).max(px(0.)), line_height);
                            window.paint_quad(fill(
                                Bounds::new(background_origin, background_size),
                                rgb(run.bg),
                            ));
                        }

                        let is_powerline = should_force_powerline(run);
                        let force_cell_width = run.force_cell_width || is_powerline;
                        let force_width = if force_cell_width {
                            Some(px(cell_width))
                        } else {
                            None
                        };

                        let shaped_line = window.text_system().shape_line(
                            run.text.clone().into(),
                            font_size,
                            &[TextRun {
                                len: run.text.len(),
                                font: mono_font.clone(),
                                color: rgb(run.fg).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            force_width,
                        );

                        let run_origin = bounds.origin.x + px(run.start_column as f32 * cell_width);
                        let run_x = if is_powerline || force_cell_width {
                            run_origin
                        } else {
                            run_origin.floor()
                        };

                        let _ = shaped_line.paint(
                            point(run_x, bounds.origin.y),
                            line_height,
                            window,
                            cx,
                        );
                    }
                },
            )
            .size_full(),
        )
}

fn should_force_powerline(run: &PositionedTerminalRun) -> bool {
    run.text.chars().count() == 1
        && run
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
}

fn snap_pixels_floor(value: gpui::Pixels, scale_factor: f32) -> gpui::Pixels {
    if !(scale_factor.is_finite() && scale_factor > 0.) {
        return value.floor();
    }

    let scaled = value.to_f64() as f32 * scale_factor;
    px(scaled.floor() / scale_factor)
}

fn snap_pixels_ceil(value: gpui::Pixels, scale_factor: f32) -> gpui::Pixels {
    if !(scale_factor.is_finite() && scale_factor > 0.) {
        return value.ceil();
    }

    let scaled = value.to_f64() as f32 * scale_factor;
    px(scaled.ceil() / scale_factor)
}

fn lines_for_display(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec!["<no output yet>".to_owned()];
    }

    text.lines().map(ToOwned::to_owned).collect()
}

fn terminal_display_lines(session: &TerminalSession) -> Vec<String> {
    if !session.styled_output.is_empty() {
        return session
            .styled_output
            .iter()
            .map(styled_line_to_string)
            .collect();
    }

    if session.output.is_empty() {
        return vec![String::new()];
    }

    session.output.lines().map(ToOwned::to_owned).collect()
}

fn styled_line_to_string(line: &TerminalStyledLine) -> String {
    let mut cells = if line.cells.is_empty() {
        cells_from_runs(&line.runs)
    } else {
        line.cells.clone()
    };
    if cells.is_empty() {
        return String::new();
    }

    cells.sort_by_key(|cell| cell.column);
    let mut output = String::new();
    let mut current_column = 0_usize;

    for cell in cells {
        while current_column < cell.column {
            output.push(' ');
            current_column = current_column.saturating_add(1);
        }
        output.push_str(&cell.text);
        current_column = current_column.saturating_add(1);
    }

    output
}

fn terminal_grid_position_from_pointer(
    position: gpui::Point<gpui::Pixels>,
    bounds: Bounds<gpui::Pixels>,
    scroll_offset: gpui::Point<gpui::Pixels>,
    line_height: f32,
    cell_width: f32,
    line_count: usize,
) -> Option<TerminalGridPosition> {
    if line_height <= 0. || cell_width <= 0. || line_count == 0 {
        return None;
    }

    let local_x = f32::from(position.x - bounds.left()).max(0.);
    let local_y = f32::from(position.y - bounds.top()).max(0.);
    let content_y = (local_y - f32::from(scroll_offset.y)).max(0.);

    let max_line = line_count.saturating_sub(1);
    let line = ((content_y / line_height).floor() as usize).min(max_line);
    let column = (local_x / cell_width).floor().max(0.) as usize;

    Some(TerminalGridPosition { line, column })
}

fn terminal_token_bounds(
    lines: &[String],
    point: TerminalGridPosition,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let line = lines.get(point.line)?;
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let index = point.column.min(chars.len().saturating_sub(1));
    if chars
        .get(index)
        .is_none_or(|character| character.is_whitespace())
    {
        return None;
    }

    let mut start = index;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }

    let mut end = index.saturating_add(1);
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }

    Some((
        TerminalGridPosition {
            line: point.line,
            column: start,
        },
        TerminalGridPosition {
            line: point.line,
            column: end,
        },
    ))
}

fn terminal_line_bounds(
    lines: &[String],
    point: TerminalGridPosition,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let line = lines.get(point.line)?;
    let width = line.chars().count();
    if width == 0 {
        return None;
    }

    Some((
        TerminalGridPosition {
            line: point.line,
            column: 0,
        },
        TerminalGridPosition {
            line: point.line,
            column: width,
        },
    ))
}

fn terminal_selection_text(lines: &[String], selection: &TerminalSelection) -> String {
    let Some((start, end)) = normalized_terminal_selection(selection) else {
        return String::new();
    };

    let mut output = String::new();
    for line_index in start.line..=end.line {
        let line = lines.get(line_index).map_or("", String::as_str);
        let chars: Vec<char> = line.chars().collect();

        let from = if line_index == start.line {
            start.column.min(chars.len())
        } else {
            0
        };
        let to = if line_index == end.line {
            end.column.min(chars.len())
        } else {
            chars.len()
        };

        if from < to {
            output.extend(chars[from..to].iter());
        }

        if line_index != end.line {
            output.push('\n');
        }
    }

    output
}

fn trim_to_last_lines(text: String, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text;
    }

    let mut trimmed = String::new();
    let start = lines.len().saturating_sub(max_lines);
    for line in lines.iter().skip(start) {
        trimmed.push_str(line);
        trimmed.push('\n');
    }
    trimmed
}

fn terminal_scroll_is_near_bottom(scroll_handle: &ScrollHandle) -> bool {
    let max_offset = scroll_handle.max_offset();
    if max_offset.height <= px(0.) {
        return true;
    }

    let offset = scroll_handle.offset();
    let distance_from_bottom = (offset.y + max_offset.height).abs();
    distance_from_bottom <= px(6.)
}

fn terminal_grid_size_from_scroll_handle(
    scroll_handle: &ScrollHandle,
    cx: &App,
) -> Option<(u16, u16, u16, u16)> {
    let bounds = scroll_handle.bounds();
    let width = (bounds.size.width.to_f64() as f32 - TERMINAL_SCROLLBAR_WIDTH_PX).max(1.);
    let height = bounds.size.height.to_f64() as f32;
    let cell_width = terminal_cell_width_px(cx);
    let line_height = terminal_line_height_px(cx);
    let (rows, cols) = terminal_grid_size_for_viewport(width, height, cell_width, line_height)?;
    let pixel_width = width.floor().clamp(1., f32::from(u16::MAX)) as u16;
    let pixel_height = height.floor().clamp(1., f32::from(u16::MAX)) as u16;
    Some((rows, cols, pixel_width, pixel_height))
}

fn terminal_cell_width_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);

    text_system
        .advance(font_id, px(TERMINAL_FONT_SIZE_PX), 'm')
        .map(|size| size.width.to_f64() as f32)
        .ok()
        .filter(|width| width.is_finite() && *width > 0.)
        .unwrap_or(TERMINAL_CELL_WIDTH_PX)
}

fn diff_cell_width_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);
    let fallback = (TERMINAL_CELL_WIDTH_PX * (DIFF_FONT_SIZE_PX / TERMINAL_FONT_SIZE_PX)).max(1.);

    text_system
        .advance(font_id, px(DIFF_FONT_SIZE_PX), 'm')
        .map(|size| size.width.to_f64() as f32)
        .ok()
        .filter(|width| width.is_finite() && *width > 0.)
        .unwrap_or(fallback)
}

fn terminal_line_height_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);
    let font_size = px(TERMINAL_FONT_SIZE_PX);

    let ascent = text_system.ascent(font_id, font_size).to_f64() as f32;
    let descent = text_system.descent(font_id, font_size).to_f64() as f32;
    let measured_height = if descent.is_sign_negative() {
        ascent - descent
    } else {
        ascent + descent
    };

    if measured_height.is_finite() && measured_height > 0. {
        return measured_height.ceil().max(TERMINAL_FONT_SIZE_PX).max(1.);
    }

    TERMINAL_CELL_HEIGHT_PX
}

fn terminal_grid_size_for_viewport(
    width: f32,
    height: f32,
    cell_width: f32,
    cell_height: f32,
) -> Option<(u16, u16)> {
    if width <= 0. || height <= 0. || cell_width <= 0. || cell_height <= 0. {
        return None;
    }

    let cols = (width / cell_width).floor() as i32;
    let rows = (height / cell_height).floor() as i32;
    if cols <= 0 || rows <= 0 {
        return None;
    }

    let cols = cols.clamp(2, i32::from(u16::MAX)) as u16;
    let rows = rows.clamp(1, i32::from(u16::MAX)) as u16;
    Some((rows, cols))
}

fn should_auto_follow_terminal_output(changed: bool, was_near_bottom: bool) -> bool {
    changed && was_near_bottom
}

fn parse_terminal_backend_kind(
    terminal_backend: Option<&str>,
) -> Result<TerminalBackendKind, String> {
    let Some(value) = terminal_backend
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(TerminalBackendKind::Embedded);
    };

    match value.to_ascii_lowercase().as_str() {
        "embedded" => Ok(TerminalBackendKind::Embedded),
        "alacritty" => Ok(TerminalBackendKind::Alacritty),
        "ghostty" => Ok(TerminalBackendKind::Ghostty),
        _ => Err(format!(
            "invalid terminal_backend `{value}` in config, expected embedded/alacritty/ghostty"
        )),
    }
}

fn parse_theme_kind(theme: Option<&str>) -> Result<ThemeKind, String> {
    let Some(value) = theme.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(ThemeKind::One);
    };

    match value.to_ascii_lowercase().as_str() {
        "one-dark" | "onedark" => Ok(ThemeKind::One),
        "ayu-dark" | "ayu" => Ok(ThemeKind::Ayu),
        "gruvbox-dark" | "gruvbox" => Ok(ThemeKind::Gruvbox),
        _ => Err(format!(
            "invalid theme `{value}` in config, expected one-dark/ayu-dark/gruvbox-dark"
        )),
    }
}

fn open_arbor_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1460.), px(900.)), cx);
    if let Err(error) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(1180.), px(760.))),
            app_id: Some("so.pen.arbor".to_owned()),
            titlebar: Some(TitlebarOptions {
                title: Some("Arbor".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(9.), px(9.))),
            }),
            window_decorations: Some(WindowDecorations::Client),
            ..Default::default()
        },
        |_, cx| {
            cx.new(|cx| {
                ArborWindow::load_with_daemon_store::<daemon::JsonDaemonSessionStore>(
                    ui_state_store::UiState::default(),
                    log_layer::LogBuffer::new(),
                    cx,
                )
            })
        },
    ) {
        eprintln!("failed to open Arbor window: {error:#}");
    }
}

fn new_window(_: &NewWindow, cx: &mut App) {
    open_arbor_window(cx);
}

fn install_app_menu_and_keys(cx: &mut App) {
    cx.on_action(new_window);
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-q", RequestQuit, None),
        KeyBinding::new("cmd-t", SpawnTerminal, None),
        KeyBinding::new("cmd-w", CloseActiveTerminal, None),
        KeyBinding::new("cmd-shift-o", OpenAddRepository, None),
        KeyBinding::new("cmd-shift-n", OpenCreateWorktree, None),
        KeyBinding::new("cmd-shift-r", RefreshWorktrees, None),
        KeyBinding::new("cmd-alt-r", RefreshChanges, None),
        KeyBinding::new("cmd-shift-1", UseOneDarkTheme, None),
        KeyBinding::new("cmd-shift-2", UseAyuDarkTheme, None),
        KeyBinding::new("cmd-shift-3", UseGruvboxTheme, None),
        KeyBinding::new("cmd-1", UseEmbeddedBackend, None),
        KeyBinding::new("cmd-2", UseAlacrittyBackend, None),
        KeyBinding::new("cmd-3", UseGhosttyBackend, None),
        KeyBinding::new("cmd-\\", ToggleLeftPane, None),
        KeyBinding::new("cmd-[", NavigateWorktreeBack, None),
        KeyBinding::new("cmd-]", NavigateWorktreeForward, None),
        KeyBinding::new("cmd-shift-l", ViewLogs, None),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "Arbor".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Arbor", ImmediateQuit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Window", NewWindow),
                MenuItem::separator(),
                MenuItem::action("Add Repository...", OpenAddRepository),
                MenuItem::separator(),
                MenuItem::action("New Terminal Tab", SpawnTerminal),
                MenuItem::action("Close Terminal Tab", CloseActiveTerminal),
                MenuItem::action("New Worktree", OpenCreateWorktree),
                MenuItem::separator(),
                MenuItem::action("Quit Arbor", ImmediateQuit),
            ],
        },
        Menu {
            name: "Terminal".into(),
            items: vec![
                MenuItem::action("New Terminal Tab", SpawnTerminal),
                MenuItem::action("Close Terminal Tab", CloseActiveTerminal),
                MenuItem::separator(),
                MenuItem::action("Use Embedded Backend", UseEmbeddedBackend),
                MenuItem::action("Use Alacritty Backend", UseAlacrittyBackend),
                MenuItem::action("Use Ghostty Backend", UseGhosttyBackend),
            ],
        },
        Menu {
            name: "Theme".into(),
            items: vec![
                MenuItem::action("Use One Dark", UseOneDarkTheme),
                MenuItem::action("Use Ayu Dark", UseAyuDarkTheme),
                MenuItem::action("Use Gruvbox Dark", UseGruvboxTheme),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Toggle Sidebar", ToggleLeftPane),
                MenuItem::action("Collapse All Repositories", CollapseAllRepositories),
                MenuItem::separator(),
                MenuItem::action("View Logs", ViewLogs),
            ],
        },
        Menu {
            name: "Worktree".into(),
            items: vec![
                MenuItem::action("Add Repository...", OpenAddRepository),
                MenuItem::separator(),
                MenuItem::action("New Worktree", OpenCreateWorktree),
                MenuItem::separator(),
                MenuItem::action("Navigate Back", NavigateWorktreeBack),
                MenuItem::action("Navigate Forward", NavigateWorktreeForward),
                MenuItem::separator(),
                MenuItem::action("Refresh Worktrees", RefreshWorktrees),
                MenuItem::action("Refresh Changes", RefreshChanges),
            ],
        },
    ]);
}

fn bounds_from_window_geometry(
    geometry: ui_state_store::WindowGeometry,
) -> Option<Bounds<gpui::Pixels>> {
    if geometry.width == 0 || geometry.height == 0 {
        return None;
    }

    let width = geometry.width as f32;
    let height = geometry.height as f32;
    if !width.is_finite() || !height.is_finite() {
        return None;
    }

    Some(Bounds::new(
        point(px(geometry.x as f32), px(geometry.y as f32)),
        size(px(width), px(height)),
    ))
}

fn main() {
    let log_buffer = log_layer::LogBuffer::new();

    {
        use tracing_subscriber::{
            EnvFilter, Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt,
        };

        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let in_memory_layer =
            log_layer::InMemoryLayer::new(log_buffer.clone()).with_filter(env_filter);

        Registry::default().with(in_memory_layer).init();
    }

    tracing::info!("Arbor starting");

    Application::new().run(move |cx: &mut App| {
        cx.set_http_client(simple_http_client::create_http_client());
        install_app_menu_and_keys(cx);
        let startup_ui_state = ui_state_store::load_startup_state();
        let default_bounds = Bounds::centered(None, size(px(1460.), px(900.)), cx);
        let bounds = startup_ui_state
            .window
            .and_then(bounds_from_window_geometry)
            .unwrap_or(default_bounds);
        let startup_ui_state_for_window = startup_ui_state.clone();
        let log_buffer_for_window = log_buffer.clone();

        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(1180.), px(760.))),
                app_id: Some("so.pen.arbor".to_owned()),
                titlebar: Some(TitlebarOptions {
                    title: Some("Arbor".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(9.), px(9.))),
                }),
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            move |_, cx| {
                let startup_ui_state = startup_ui_state_for_window.clone();
                let log_buffer = log_buffer_for_window.clone();
                cx.new(move |cx| {
                    ArborWindow::load_with_daemon_store::<daemon::JsonDaemonSessionStore>(
                        startup_ui_state,
                        log_buffer,
                        cx,
                    )
                })
            },
        ) {
            eprintln!("failed to open Arbor window: {error:#}");
            cx.quit();
            return;
        }

        cx.activate(true);
    });
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use {
        crate::{
            DiffLineKind, TerminalSession, TerminalState, auto_commit_body, auto_commit_subject,
            build_side_by_side_diff_lines, extract_first_url, styled_lines_for_session,
            terminal_backend::{
                TerminalCursor, TerminalStyledCell, TerminalStyledLine, TerminalStyledRun,
            },
            theme::ThemeKind,
        },
        arbor_core::changes::{ChangeKind, ChangedFile},
    };

    fn session_with_styled_line(
        text: &str,
        fg: u32,
        bg: u32,
        cursor: Option<TerminalCursor>,
    ) -> TerminalSession {
        TerminalSession {
            id: 1,
            daemon_session_id: "daemon-test-1".to_owned(),
            worktree_path: std::path::PathBuf::from("/tmp/worktree"),
            title: "term-1".to_owned(),
            last_command: None,
            pending_command: String::new(),
            command: "zsh".to_owned(),
            state: TerminalState::Running,
            exit_code: None,
            updated_at_unix_ms: None,
            cols: 120,
            rows: 35,
            generation: 0,
            output: text.to_owned(),
            styled_output: vec![TerminalStyledLine {
                cells: text
                    .chars()
                    .enumerate()
                    .map(|(column, character)| TerminalStyledCell {
                        column,
                        text: character.to_string(),
                        fg,
                        bg,
                    })
                    .collect(),
                runs: vec![TerminalStyledRun {
                    text: text.to_owned(),
                    fg,
                    bg,
                }],
            }],
            cursor,
            runtime: None,
        }
    }

    #[test]
    fn sanitizes_worktree_name_for_branch_and_path() {
        let sanitized = crate::sanitize_worktree_name("  Remote SSH / Demo  ");
        assert_eq!(sanitized, "remote-ssh-demo");
    }

    #[test]
    fn derives_default_branch_name_when_empty() {
        let branch = crate::derive_branch_name(" !!! ");
        assert_eq!(branch, "worktree");
    }

    #[test]
    fn auto_follow_requires_new_output_and_bottom_position() {
        assert!(crate::should_auto_follow_terminal_output(true, true));
        assert!(!crate::should_auto_follow_terminal_output(true, false));
        assert!(!crate::should_auto_follow_terminal_output(false, true));
    }

    #[test]
    fn auto_follow_is_disabled_without_new_output() {
        assert!(!crate::should_auto_follow_terminal_output(false, false));
    }

    #[test]
    fn computes_terminal_grid_size_from_viewport() {
        let result = crate::terminal_grid_size_for_viewport(
            900.,
            380.,
            crate::TERMINAL_CELL_WIDTH_PX,
            crate::TERMINAL_CELL_HEIGHT_PX,
        );
        assert_eq!(result, Some((20, 100)));
    }

    #[test]
    fn cursor_is_painted_at_terminal_column_instead_of_line_end() {
        let theme = ThemeKind::One.palette();
        let session = session_with_styled_line(
            "abcdef",
            0x112233,
            0x445566,
            Some(TerminalCursor { line: 0, column: 2 }),
        );

        let lines = styled_lines_for_session(&session, theme, true, None);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].runs.len(), 3);
        assert_eq!(lines[0].runs[0].text, "ab");
        assert_eq!(lines[0].runs[1].text, "c");
        assert_eq!(lines[0].runs[1].fg, 0x112233);
        assert_eq!(lines[0].runs[1].bg, theme.terminal_cursor);
        assert_eq!(lines[0].runs[2].text, "def");
    }

    #[test]
    fn cursor_pads_to_column_when_it_is_after_line_content() {
        let theme = ThemeKind::One.palette();
        let session = session_with_styled_line(
            "abc",
            0x112233,
            0x445566,
            Some(TerminalCursor { line: 0, column: 5 }),
        );

        let lines = styled_lines_for_session(&session, theme, true, None);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].runs.len(), 2);
        assert_eq!(lines[0].runs[0].text, "abc");
        assert_eq!(lines[0].runs[1].text, " ");
        assert_eq!(lines[0].runs[1].fg, theme.text_primary);
        assert_eq!(lines[0].runs[1].bg, theme.terminal_cursor);
        assert!(lines[0].cells.iter().any(|cell| {
            cell.column == 5 && cell.text == " " && cell.bg == theme.terminal_cursor
        }));
    }

    #[test]
    fn positioned_runs_split_cells_with_zero_width_sequences() {
        let cells = vec![
            TerminalStyledCell {
                column: 0,
                text: "A".to_owned(),
                fg: 0x112233,
                bg: 0x445566,
            },
            TerminalStyledCell {
                column: 1,
                text: "☀️".to_owned(),
                fg: 0x112233,
                bg: 0x445566,
            },
            TerminalStyledCell {
                column: 2,
                text: "B".to_owned(),
                fg: 0x112233,
                bg: 0x445566,
            },
        ];

        let runs = crate::positioned_runs_from_cells(&cells);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "A");
        assert_eq!(runs[0].start_column, 0);
        assert_eq!(runs[0].cell_count, 1);
        assert!(runs[0].force_cell_width);
        assert_eq!(runs[1].text, "☀️");
        assert_eq!(runs[1].start_column, 1);
        assert_eq!(runs[1].cell_count, 1);
        assert!(!runs[1].force_cell_width);
        assert_eq!(runs[2].text, "B");
        assert_eq!(runs[2].start_column, 2);
        assert_eq!(runs[2].cell_count, 1);
        assert!(runs[2].force_cell_width);
    }

    #[test]
    fn positioned_runs_do_not_force_cell_width_for_powerline_symbols() {
        let cells = vec![
            TerminalStyledCell {
                column: 0,
                text: "\u{e0b0}".to_owned(),
                fg: 0xaabbcc,
                bg: 0x112233,
            },
            TerminalStyledCell {
                column: 1,
                text: "X".to_owned(),
                fg: 0xaabbcc,
                bg: 0x112233,
            },
        ];

        let runs = crate::positioned_runs_from_cells(&cells);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "\u{e0b0}");
        assert!(!runs[0].force_cell_width);
        assert_eq!(runs[1].text, "X");
        assert!(runs[1].force_cell_width);
    }

    #[test]
    fn positioned_runs_keep_cell_width_for_box_drawing_symbols() {
        let cells = vec![
            TerminalStyledCell {
                column: 0,
                text: "│".to_owned(),
                fg: 0xaabbcc,
                bg: 0x112233,
            },
            TerminalStyledCell {
                column: 1,
                text: "X".to_owned(),
                fg: 0xaabbcc,
                bg: 0x112233,
            },
        ];

        let runs = crate::positioned_runs_from_cells(&cells);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "│X");
        assert!(runs[0].force_cell_width);
    }

    #[test]
    fn powerline_glyph_is_forced_to_cell_width() {
        let run = crate::PositionedTerminalRun {
            text: "\u{e0b6}".to_owned(),
            fg: 0,
            bg: 0,
            start_column: 7,
            cell_count: 1,
            force_cell_width: false,
        };

        assert!(crate::should_force_powerline(&run));
    }

    #[test]
    fn token_bounds_capture_full_url() {
        let lines = vec!["visit https://example.com/path?q=1 please".to_owned()];
        let point = crate::TerminalGridPosition {
            line: 0,
            column: 12,
        };

        let bounds = crate::terminal_token_bounds(&lines, point);
        assert!(bounds.is_some());
        let (start, end) = bounds.expect("token bounds");
        let selection = crate::TerminalSelection {
            session_id: 1,
            anchor: start,
            head: end,
        };
        let selected = crate::terminal_selection_text(&lines, &selection);
        assert_eq!(selected, "https://example.com/path?q=1");
    }

    #[test]
    fn selection_text_spans_multiple_lines() {
        let lines = vec!["abc".to_owned(), "def".to_owned(), "ghi".to_owned()];
        let selection = crate::TerminalSelection {
            session_id: 1,
            anchor: crate::TerminalGridPosition { line: 0, column: 1 },
            head: crate::TerminalGridPosition { line: 2, column: 2 },
        };

        let selected = crate::terminal_selection_text(&lines, &selection);
        assert_eq!(selected, "bc\ndef\ngh");
    }

    #[test]
    fn line_bounds_capture_entire_line_on_triple_click() {
        let lines = vec!["hello world".to_owned()];
        let point = crate::TerminalGridPosition { line: 0, column: 3 };

        let bounds = crate::terminal_line_bounds(&lines, point);
        assert!(bounds.is_some());
        let (start, end) = bounds.expect("line bounds");
        assert_eq!(start.line, 0);
        assert_eq!(start.column, 0);
        assert_eq!(end.line, 0);
        assert_eq!(end.column, 11);
    }

    #[test]
    fn styled_lines_remap_embedded_default_palette_to_active_theme() {
        let theme = ThemeKind::Gruvbox.palette();
        let session = session_with_styled_line(
            "abc",
            crate::terminal_backend::EMBEDDED_TERMINAL_DEFAULT_FG,
            crate::terminal_backend::EMBEDDED_TERMINAL_DEFAULT_BG,
            None,
        );

        let lines = styled_lines_for_session(&session, theme, false, None);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0]
                .cells
                .iter()
                .all(|cell| cell.bg == theme.terminal_bg)
        );
        assert!(
            lines[0]
                .cells
                .iter()
                .all(|cell| cell.fg == theme.text_primary)
        );
    }

    #[test]
    fn truncate_middle_text_keeps_tail_visible() {
        let truncated = crate::truncate_middle_text("src/some/really/long/path/main.rs", 16);
        assert!(truncated.contains('…'));
        assert!(truncated.ends_with("main.rs"));
    }

    #[test]
    fn truncate_middle_text_returns_original_when_short() {
        let input = "src/main.rs";
        let truncated = crate::truncate_middle_text(input, 32);
        assert_eq!(truncated, input);
    }

    #[test]
    fn auto_commit_subject_uses_filename_for_single_change() {
        let changed_files = vec![ChangedFile {
            path: std::path::PathBuf::from("src/main.rs"),
            kind: ChangeKind::Modified,
            additions: 4,
            deletions: 1,
        }];

        let subject = auto_commit_subject(&changed_files);
        assert_eq!(subject, "chore: update main.rs");
    }

    #[test]
    fn auto_commit_body_includes_stats_and_overflow_line() {
        let changed_files = (0..13)
            .map(|index| ChangedFile {
                path: std::path::PathBuf::from(format!("src/file-{index}.rs")),
                kind: ChangeKind::Modified,
                additions: index + 1,
                deletions: index,
            })
            .collect::<Vec<_>>();

        let body = auto_commit_body(&changed_files);
        assert!(body.contains("Auto-generated by Arbor."));
        assert!(body.contains("- M src/file-0.rs (+1 -0)"));
        assert!(body.contains("- ... and 1 more"));
    }

    #[test]
    fn extract_first_url_ignores_punctuation() {
        let url = extract_first_url("created PR: https://github.com/acme/repo/pull/42.");
        assert_eq!(url.as_deref(), Some("https://github.com/acme/repo/pull/42"));
    }

    #[test]
    fn side_by_side_diff_marks_modified_lines() {
        let lines = build_side_by_side_diff_lines("alpha\nbeta\n", "alpha\ngamma\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].kind, DiffLineKind::Context);
        assert_eq!(lines[0].left_text, "alpha");
        assert_eq!(lines[0].right_text, "alpha");
        assert_eq!(lines[1].kind, DiffLineKind::Modified);
        assert_eq!(lines[1].left_text, "beta");
        assert_eq!(lines[1].right_text, "gamma");
    }

    #[test]
    fn side_by_side_diff_marks_inserted_and_removed_lines() {
        let insertion = build_side_by_side_diff_lines("one\n", "one\ntwo\n");
        assert_eq!(insertion.len(), 2);
        assert_eq!(insertion[1].kind, DiffLineKind::Added);
        assert_eq!(insertion[1].left_text, "");
        assert_eq!(insertion[1].right_text, "two");

        let removal = build_side_by_side_diff_lines("one\ntwo\n", "one\n");
        assert_eq!(removal.len(), 2);
        assert_eq!(removal[1].kind, DiffLineKind::Removed);
        assert_eq!(removal[1].left_text, "two");
        assert_eq!(removal[1].right_text, "");
    }

    #[test]
    fn side_by_side_diff_hides_large_unchanged_gaps() {
        let before = "a1\na2\na3\na4\na5\na6\na7\na8\na9\na10\n";
        let after = "a1\na2\na3\na4\na5\nchanged\na7\na8\na9\na10\n";
        let lines = build_side_by_side_diff_lines(before, after);

        assert!(!lines.is_empty());
        assert!(
            lines
                .iter()
                .any(|line| line.left_text.contains("unchanged lines hidden"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.left_text == "a3" && line.right_text == "a3")
        );
    }
}
