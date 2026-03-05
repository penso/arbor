mod app_config;
mod repository_store;
mod terminal_backend;
mod terminal_daemon_http;
mod terminal_keys;
mod theme;
mod ui_state_store;

use {
    arbor_core::{
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
        collections::HashMap,
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
const TAB_ICON_TERMINAL: &str = "\u{f489}";
const TAB_ICON_DIFF: &str = "\u{f440}";

static QUIT_ARMED_AT: Mutex<Option<Instant>> = Mutex::new(None);

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
    UseGhosttyBackend
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
enum TerminalRuntime {
    Embedded(EmbeddedTerminal),
    Daemon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CenterTab {
    Terminal(u64),
    Diff(u64),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreateWorktreeField {
    RepositoryPath,
    WorktreeName,
}

#[derive(Debug, Clone)]
struct CreateWorktreeModal {
    repository_path: String,
    worktree_name: String,
    active_field: CreateWorktreeField,
    is_creating: bool,
    error: Option<String>,
}

enum ModalInputEvent {
    SetActiveField(CreateWorktreeField),
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
    create_worktree_modal: Option<CreateWorktreeModal>,
    pending_diff_scroll_to_file: Option<PathBuf>,
    focus_terminal_on_next_render: bool,
    last_persisted_ui_state: ui_state_store::UiState,
    last_ui_state_error: Option<String>,
    notice: Option<String>,
}

impl ArborWindow {
    fn load_with_daemon_store<S>(
        startup_ui_state: ui_state_store::UiState,
        cx: &mut Context<Self>,
    ) -> Self
    where
        S: daemon::DaemonSessionStore + Default + 'static,
    {
        Self::load(Box::new(S::default()), startup_ui_state, cx)
    }

    fn load(
        daemon_session_store: Box<dyn daemon::DaemonSessionStore>,
        startup_ui_state: ui_state_store::UiState,
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
                    create_worktree_modal: None,
                    pending_diff_scroll_to_file: None,
                    focus_terminal_on_next_render: true,
                    last_persisted_ui_state: startup_ui_state,
                    last_ui_state_error: None,
                    notice: Some(format!("failed to read current directory: {error}")),
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
                    create_worktree_modal: None,
                    pending_diff_scroll_to_file: None,
                    focus_terminal_on_next_render: true,
                    last_persisted_ui_state: startup_ui_state,
                    last_ui_state_error: None,
                    notice: Some(format!("failed to resolve git repository root: {error}")),
                };
            },
        };

        let loaded_config = app_config::load_or_create_config();
        let mut notice_parts = loaded_config.notices;
        let config_last_modified = app_config::config_last_modified(&config_path);

        if let Err(error) = daemon_session_store.load() {
            notice_parts.push(format!("failed to load daemon session metadata: {error}"));
        }
        let daemon_base_url =
            daemon_base_url_from_config(loaded_config.config.daemon_url.as_deref());
        let mut terminal_daemon = match HttpTerminalDaemon::new(&daemon_base_url) {
            Ok(client) => Some(client),
            Err(error) => {
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
            create_worktree_modal: None,
            pending_diff_scroll_to_file: None,
            focus_terminal_on_next_render: true,
            last_persisted_ui_state: startup_ui_state,
            last_ui_state_error: None,
            notice: (!notice_parts.is_empty()).then_some(notice_parts.join(" | ")),
        };

        app.refresh_worktrees(cx);
        app.restore_terminal_sessions_from_records(initial_daemon_records, attach_daemon_runtime);
        let _ = app.ensure_selected_worktree_terminal();
        app.sync_daemon_session_store(cx);
        app.start_terminal_poller(cx);
        app.start_worktree_auto_refresh(cx);
        app.start_github_pr_auto_refresh(cx);
        app.start_config_auto_refresh(cx);
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
                    if this.reload_changed_files() {
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

    fn sync_running_terminals(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let follow_output = terminal_scroll_is_near_bottom(&self.terminal_scroll_handle);
        let active_terminal_id = self.active_terminal_id_for_selected_worktree();
        let target_grid_size =
            terminal_grid_size_from_scroll_handle(&self.terminal_scroll_handle, cx);
        let daemon = self.terminal_daemon.clone();
        let mut sessions_to_close = Vec::new();

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
                            sessions_to_close.push(session.id);
                        } else {
                            session.state = TerminalState::Failed;
                            session.runtime = None;
                            changed = true;
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
                                    sessions_to_close.push(session.id);
                                } else if session.state == TerminalState::Failed {
                                    session.runtime = None;
                                    changed = true;
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
            };
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
        self.active_worktree_index
            .and_then(|index| self.worktrees.get(index))
            .map(|worktree| worktree.path.as_path())
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
        self.active_terminal_id_for_worktree(worktree_path)
    }

    fn selected_worktree_terminals(&self) -> Vec<&TerminalSession> {
        let Some(worktree_path) = self.selected_worktree_path() else {
            return Vec::new();
        };

        self.terminals
            .iter()
            .filter(|session| session.worktree_path.as_path() == worktree_path)
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
        let Some(index) = self
            .terminals
            .iter()
            .position(|session| session.id == session_id)
        else {
            return false;
        };

        if let Some(session) = self.terminals.get(index)
            && matches!(session.runtime, Some(TerminalRuntime::Daemon))
            && let Some(daemon) = self.terminal_daemon.as_ref()
        {
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
                self.notice = Some(format!("failed to close terminal session: {error}"));
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
            None => {},
        }
    }

    fn close_active_terminal_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active_tab(window, cx);
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
        self.active_worktree_index = Some(index);
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

    fn reload_changed_files(&mut self) -> bool {
        let previous_files = self.changed_files.clone();
        let previous_notice = self.notice.clone();
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

    fn open_create_worktree_modal(&mut self, cx: &mut Context<Self>) {
        let repository_root = self
            .selected_repository()
            .map(|repository| repository.root.clone())
            .unwrap_or_else(|| self.repo_root.clone());
        self.open_create_worktree_modal_for_repository_root(repository_root, cx);
    }

    fn open_create_worktree_modal_for_repository_root(
        &mut self,
        repository_root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.create_worktree_modal = Some(CreateWorktreeModal {
            repository_path: repository_root.display().to_string(),
            worktree_name: String::new(),
            active_field: CreateWorktreeField::WorktreeName,
            is_creating: false,
            error: None,
        });
        cx.notify();
    }

    fn close_create_worktree_modal(&mut self, cx: &mut Context<Self>) {
        self.create_worktree_modal = None;
        cx.notify();
    }

    fn update_create_worktree_modal_input(
        &mut self,
        input: ModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(modal) = self.create_worktree_modal.as_mut() else {
            return;
        };

        if modal.is_creating {
            return;
        }

        match input {
            ModalInputEvent::SetActiveField(field) => {
                modal.active_field = field;
            },
            ModalInputEvent::MoveActiveField(reverse) => {
                modal.active_field = match (modal.active_field, reverse) {
                    (CreateWorktreeField::RepositoryPath, false) => {
                        CreateWorktreeField::WorktreeName
                    },
                    (CreateWorktreeField::WorktreeName, false) => {
                        CreateWorktreeField::RepositoryPath
                    },
                    (CreateWorktreeField::RepositoryPath, true) => {
                        CreateWorktreeField::WorktreeName
                    },
                    (CreateWorktreeField::WorktreeName, true) => {
                        CreateWorktreeField::RepositoryPath
                    },
                };
            },
            ModalInputEvent::Backspace => {
                let field_value = match modal.active_field {
                    CreateWorktreeField::RepositoryPath => &mut modal.repository_path,
                    CreateWorktreeField::WorktreeName => &mut modal.worktree_name,
                };
                let _ = field_value.pop();
            },
            ModalInputEvent::Append(text) => {
                let field_value = match modal.active_field {
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
        let Some(modal) = self.create_worktree_modal.as_mut() else {
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
                        this.create_worktree_modal = None;
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
                        if let Some(modal) = this.create_worktree_modal.as_mut() {
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
        if self.create_worktree_modal.is_none() || event.is_held {
            return;
        }

        if event.keystroke.modifiers.platform {
            return;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_create_worktree_modal(cx);
                cx.stop_propagation();
                return;
            },
            "tab" => {
                self.update_create_worktree_modal_input(
                    ModalInputEvent::MoveActiveField(event.keystroke.modifiers.shift),
                    cx,
                );
                cx.stop_propagation();
                return;
            },
            "enter" | "return" => {
                self.submit_create_worktree_modal(cx);
                cx.stop_propagation();
                return;
            },
            "backspace" => {
                self.update_create_worktree_modal_input(ModalInputEvent::Backspace, cx);
                cx.stop_propagation();
                return;
            },
            _ => {},
        }

        if event.keystroke.modifiers.control || event.keystroke.modifiers.alt {
            return;
        }

        if let Some(key_char) = event.keystroke.key_char.as_ref() {
            self.update_create_worktree_modal_input(ModalInputEvent::ClearError, cx);
            self.update_create_worktree_modal_input(
                ModalInputEvent::Append(key_char.to_owned()),
                cx,
            );
            cx.stop_propagation();
        }
    }

    fn action_open_create_worktree(
        &mut self,
        _: &OpenCreateWorktree,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_create_worktree_modal(cx);
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
        self.close_active_terminal_session(window, cx);
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

    fn spawn_terminal_session_inner(&mut self, show_notice_on_missing_worktree: bool) -> bool {
        let Some(cwd) = self.selected_worktree_path().map(Path::to_path_buf) else {
            if show_notice_on_missing_worktree {
                self.notice = Some("select a worktree before opening a terminal tab".to_owned());
            }
            return false;
        };

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
            .scroll_to_item(*row_index, ScrollStrategy::Top);
        true
    }

    fn open_diff_tab_for_selected_file(&mut self, cx: &mut Context<Self>) {
        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a worktree before opening a diff".to_owned());
            return;
        };
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
                let wrap_columns = this.estimated_diff_wrap_columns(terminal_cell_width_px(cx));
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
        if self.active_diff_session_id == Some(session_id) {
            return;
        }
        self.active_diff_session_id = Some(session_id);
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
        if self.create_worktree_modal.is_some() {
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
        let center_width = (window_width
            - self.left_pane_width
            - self.right_pane_width
            - (2. * PANE_RESIZE_HANDLE_WIDTH))
            .max(PANE_CENTER_MIN_WIDTH);
        let list_width =
            (center_width - DIFF_ZONEMAP_WIDTH_PX - (DIFF_ZONEMAP_MARGIN_PX * 2.)).max(80.);
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
        estimated_columns.clamp(12, 240)
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

    fn render_top_bar(&self) -> impl IntoElement {
        let theme = self.theme();
        let repository = self.selected_repository_label();
        let branch = self
            .active_worktree()
            .map(|worktree| worktree.branch.clone())
            .unwrap_or_else(|| "no-worktree".to_owned());
        let centered_title = format!("{repository} · {branch}");

        div()
            .h(px(TITLEBAR_HEIGHT))
            .bg(rgb(theme.chrome_bg))
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .flex()
            .items_center()
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
        let theme = self.theme();
        let repositories = self.repositories.clone();
        let worktrees = self.worktrees.clone();
        div()
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
                            let repository_icon = repository
                                .label
                                .chars()
                                .next()
                                .map(|ch| ch.to_ascii_uppercase().to_string())
                                .unwrap_or_else(|| "R".to_owned());
                            let repository_avatar_url = repository.avatar_url.clone();
                            let repository_root = repository.root.clone();
                            let repo_worktrees: Vec<(usize, WorktreeSummary)> = worktrees
                                .iter()
                                .cloned()
                                .enumerate()
                                .filter(|(_, worktree)| worktree.repo_root == repository.root)
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
                                        .h(px(28.))
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
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .size(px(14.))
                                                        .rounded_full()
                                                        .overflow_hidden()
                                                        .child(
                                                            if let Some(url) =
                                                                repository_avatar_url.clone()
                                                            {
                                                                img(url)
                                                                    .size_full()
                                                                    .with_fallback({
                                                                        let repository_icon =
                                                                            repository_icon.clone();
                                                                        move || {
                                                                            div()
                                                                                .size_full()
                                                                                .bg(rgb(
                                                                                    theme
                                                                                        .panel_active_bg,
                                                                                ))
                                                                                .flex()
                                                                                .items_center()
                                                                                .justify_center()
                                                                                .text_size(px(9.))
                                                                                .font_weight(
                                                                                    FontWeight::SEMIBOLD,
                                                                                )
                                                                                .text_color(rgb(
                                                                                    theme
                                                                                        .text_primary,
                                                                                ))
                                                                                .child(
                                                                                    repository_icon
                                                                                        .clone(),
                                                                                )
                                                                                .into_any_element()
                                                                        }
                                                                    })
                                                                    .into_any_element()
                                                            } else {
                                                                div()
                                                                    .size_full()
                                                                    .bg(rgb(theme.panel_active_bg))
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .text_size(px(9.))
                                                                    .font_weight(
                                                                        FontWeight::SEMIBOLD,
                                                                    )
                                                                    .text_color(rgb(
                                                                        theme.text_primary,
                                                                    ))
                                                                    .child(repository_icon)
                                                                    .into_any_element()
                                                            },
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .text_ellipsis()
                                                        .text_xs()
                                                        .text_color(rgb(theme.text_primary))
                                                        .child(repository.label.clone()),
                                                ),
                                        )
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
                                                    this.open_create_worktree_modal_for_repository_root(
                                                        repository_root.clone(),
                                                        cx,
                                                    );
                                                    cx.stop_propagation();
                                                })),
                                        ),
                                )
                                .child(
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
                    .child(
                        action_button(theme, "open-add-repository", "+ Add Repo", false, false)
                            .w_full()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_add_repository_picker(cx);
                            })),
                    ),
            )
    }

    fn render_terminal_panel(&mut self, _: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let wrap_columns = self.estimated_diff_wrap_columns(terminal_cell_width_px(cx));
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
                                let (tab_icon, tab_label) = match tab {
                                    CenterTab::Terminal(session_id) => (
                                        TAB_ICON_TERMINAL,
                                        self.terminals
                                            .iter()
                                            .find(|session| session.id == session_id)
                                            .map(terminal_tab_title)
                                            .unwrap_or_else(|| "terminal".to_owned()),
                                    ),
                                    CenterTab::Diff(diff_id) => (
                                        TAB_ICON_DIFF,
                                        self.diff_sessions
                                            .iter()
                                            .find(|session| session.id == diff_id)
                                            .map(diff_tab_title)
                                            .unwrap_or_else(|| "diff".to_owned()),
                                    ),
                                };
                                let tab_id = match tab {
                                    CenterTab::Terminal(id) => ("center-tab-terminal", id),
                                    CenterTab::Diff(id) => ("center-tab-diff", id),
                                };

                                div()
                                    .id(tab_id)
                                    .h_full()
                                    .cursor_pointer()
                                    .min_w(px(122.))
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
                                            .text_xs()
                                            .text_color(rgb(if is_active {
                                                theme.text_primary
                                            } else {
                                                theme.text_muted
                                            }))
                                            .child(tab_icon),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(if is_active {
                                                theme.text_primary
                                            } else {
                                                theme.text_muted
                                            }))
                                            .child(tab_label),
                                    )
                                    .when(index == 0, |this| this.border_l_1())
                                    .when(index + 1 == tab_count, |this| this.border_r_1())
                                    .map(|this| match relation {
                                        Some(std::cmp::Ordering::Equal) => {
                                            this.border_l_1().border_r_1()
                                        },
                                        Some(std::cmp::Ordering::Less) => {
                                            this.border_l_1().border_b_1()
                                        },
                                        Some(std::cmp::Ordering::Greater) => {
                                            this.border_r_1().border_b_1()
                                        },
                                        None => this.border_b_1(),
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| match tab {
                                        CenterTab::Terminal(session_id) => {
                                            this.select_terminal(session_id, window, cx);
                                        },
                                        CenterTab::Diff(diff_id) => {
                                            this.select_diff_tab(diff_id, cx);
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
                        active_terminal.is_none() && active_diff_session.is_none(),
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
                        this.child(render_diff_session(
                            session,
                            theme,
                            &self.diff_scroll_handle,
                            mono_font,
                        ))
                    }),
            )
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
        let selected_path = self.selected_changed_file.clone();
        let has_changed_file_selected = selected_path.is_some();

        div()
            .w(px(self.right_pane_width))
            .h_full()
            .min_h_0()
            .bg(rgb(theme.sidebar_bg))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div().h(px(24.)).flex().items_center().justify_end().child(
                    action_button(
                        theme,
                        "open-diff-tab",
                        "Diff",
                        has_changed_file_selected,
                        !has_changed_file_selected,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_diff_tab_for_selected_file(cx);
                    })),
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
                    .gap_0()
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
                            .px_1()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(if is_selected {
                                theme.panel_active_bg
                            } else {
                                theme.sidebar_bg
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.select_changed_file(file_path.clone(), cx);
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
                    .child(status_text(theme, "ready")),
            )
    }

    fn render_create_worktree_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.create_worktree_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let branch_name = derive_branch_name(&modal.worktree_name);
        let target_path_preview =
            preview_managed_worktree_path(modal.repository_path.trim(), modal.worktree_name.trim())
                .unwrap_or_else(|_| "-".to_owned());

        let repository_active = modal.active_field == CreateWorktreeField::RepositoryPath;
        let worktree_active = modal.active_field == CreateWorktreeField::WorktreeName;
        let create_disabled = modal.is_creating
            || modal.repository_path.trim().is_empty()
            || modal.worktree_name.trim().is_empty();

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
                                    .child("Create Worktree"),
                            )
                            .child(
                                action_button(theme, "close-create-worktree", "Close", false, true)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_create_worktree_modal(cx);
                                    })),
                            ),
                    )
                    .child(
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
                                ModalInputEvent::SetActiveField(CreateWorktreeField::WorktreeName),
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "cancel-create-worktree",
                                    "Cancel",
                                    false,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_create_worktree_modal(cx);
                                    },
                                )),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "submit-create-worktree",
                                    if modal.is_creating {
                                        "Creating..."
                                    } else {
                                        "Create Worktree"
                                    },
                                    !create_disabled,
                                    create_disabled,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.submit_create_worktree_modal(cx);
                                    },
                                )),
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
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()?;
    u64::try_from(duration.as_millis()).ok()
}

fn daemon_base_url_from_config(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DAEMON_BASE_URL)
        .to_owned()
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    let left_canonical = left.canonicalize().ok();
    let right_canonical = right.canonicalize().ok();

    left_canonical
        .zip(right_canonical)
        .is_some_and(|(left, right)| left == right)
}

fn daemon_error_is_connection_refused(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("connection refused")
        || lower.contains("failed to connect")
        || lower.contains("actively refused")
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
            .child(self.render_top_bar())
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
                    .child(self.render_pane_resize_handle(
                        "left-pane-resize",
                        DraggedPaneDivider::Left,
                        theme,
                    ))
                    .child(self.render_center_pane(window, cx))
                    .child(self.render_pane_resize_handle(
                        "right-pane-resize",
                        DraggedPaneDivider::Right,
                        theme,
                    ))
                    .child(self.render_right_pane(cx)),
            )
            .child(self.render_status_bar())
            .child(self.render_create_worktree_modal(cx))
    }
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

    let mut lines = Vec::new();
    let mut before_cursor = 0_usize;
    let mut after_cursor = 0_usize;

    for hunk in diff.hunks() {
        let before_start = hunk.before.start as usize;
        let before_end = hunk.before.end as usize;
        let after_start = hunk.after.start as usize;
        let after_end = hunk.after.end as usize;

        push_context_diff_lines(
            &mut lines,
            &before_rope,
            &after_rope,
            before_cursor,
            before_start,
            after_cursor,
            after_start,
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
    }

    push_context_diff_lines(
        &mut lines,
        &before_rope,
        &after_rope,
        before_cursor,
        input.before.len(),
        after_cursor,
        input.after.len(),
    );

    lines
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

fn render_diff_session(
    session: DiffSession,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
    mono_font: gpui::Font,
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
                        .text_xs()
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
                    .text_xs()
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
) -> impl IntoElement {
    let number_width = px((DIFF_LINE_NUMBER_WIDTH_CHARS as f32 * TERMINAL_CELL_WIDTH_PX) + 12.);

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
                        .text_xs()
                        .text_color(rgb(theme.text_disabled))
                        .child(line_number.map_or(String::new(), |line| line.to_string())),
                )
                .child(
                    div()
                        .id(marker_id)
                        .w(px(10.))
                        .flex_none()
                        .text_xs()
                        .text_color(rgb(diff_marker_color(marker)))
                        .child(marker.to_string()),
                )
                .child(
                    div()
                        .id(text_id)
                        .min_w_0()
                        .flex_1()
                        .font(mono_font)
                        .text_xs()
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

fn github_repo_slug_for_repo(repo_root: &Path) -> Option<String> {
    let remote_url = git_origin_remote_url(repo_root)?;
    github_repo_slug_from_remote_url(remote_url.trim())
}

fn github_avatar_url_for_repo_slug(repo_slug: &str) -> Option<String> {
    let (owner, _) = repo_slug.split_once('/')?;
    Some(format!("https://github.com/{owner}.png?size=40"))
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
    value
        .strip_prefix("refs/heads/")
        .unwrap_or(value)
        .to_owned()
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

fn request_quit(_: &RequestQuit, cx: &mut App) {
    let now = Instant::now();
    let mut guard = match QUIT_ARMED_AT.lock() {
        Ok(lock) => lock,
        Err(poisoned) => poisoned.into_inner(),
    };

    let should_quit = guard
        .as_ref()
        .is_some_and(|armed_at| now.duration_since(*armed_at) <= QUIT_ARM_WINDOW);

    if should_quit {
        *guard = None;
        cx.quit();
        return;
    }

    *guard = Some(now);
    eprintln!(
        "press Cmd-Q again within {}ms to quit Arbor",
        QUIT_ARM_WINDOW.as_millis(),
    );
}

fn install_app_menu_and_keys(cx: &mut App) {
    cx.on_action(request_quit);
    cx.bind_keys([
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
    ]);
    cx.set_menus(vec![
        Menu {
            name: "Arbor".into(),
            items: vec![
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Arbor", RequestQuit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("Add Repository...", OpenAddRepository),
                MenuItem::separator(),
                MenuItem::action("New Terminal Tab", SpawnTerminal),
                MenuItem::action("Close Terminal Tab", CloseActiveTerminal),
                MenuItem::action("New Worktree", OpenCreateWorktree),
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
            name: "Worktree".into(),
            items: vec![
                MenuItem::action("Add Repository...", OpenAddRepository),
                MenuItem::separator(),
                MenuItem::action("New Worktree", OpenCreateWorktree),
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
    Application::new().run(|cx: &mut App| {
        install_app_menu_and_keys(cx);
        let startup_ui_state = ui_state_store::load_startup_state();
        let default_bounds = Bounds::centered(None, size(px(1460.), px(900.)), cx);
        let bounds = startup_ui_state
            .window
            .and_then(bounds_from_window_geometry)
            .unwrap_or(default_bounds);
        let startup_ui_state_for_window = startup_ui_state.clone();

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
                cx.new(move |cx| {
                    ArborWindow::load_with_daemon_store::<daemon::JsonDaemonSessionStore>(
                        startup_ui_state,
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
    use crate::{
        DiffLineKind, TerminalSession, TerminalState, build_side_by_side_diff_lines,
        styled_lines_for_session,
        terminal_backend::{
            TerminalCursor, TerminalStyledCell, TerminalStyledLine, TerminalStyledRun,
        },
        theme::ThemeKind,
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
}
