mod actions;
mod app_config;
mod checkout;
mod connection_history;
mod constants;
mod github_auth_store;
mod github_service;
mod helpers;
mod log_layer;
#[cfg(feature = "mdns")]
mod mdns_browser;
mod notifications;
mod repository_store;
mod simple_http_client;
mod terminal_backend;
mod terminal_daemon_http;
mod terminal_keys;
mod terminal_runtime;
mod theme;
mod types;
mod ui_state_store;

pub(crate) use {actions::*, constants::*, helpers::*, terminal_runtime::*, types::*};
use {
    arbor_core::{
        SessionId,
        agent::AgentState,
        changes::{self, ChangeKind, ChangedFile},
        daemon::{self, CreateOrAttachRequest, DaemonSessionRecord},
        worktree,
    },
    checkout::CheckoutKind,
    gix_diff::blob::v2::{
        Algorithm as DiffAlgorithm, Diff as BlobDiff, InternedInput as BlobInternedInput,
    },
    gpui::{
        Animation, AnimationExt, AnyElement, App, Application, AssetSource, Bounds, ClipboardItem,
        Context, Div, DragMoveEvent, ElementId, ElementInputHandler, EntityInputHandler,
        FocusHandle, FontWeight, Image, ImageFormat, KeyBinding, KeyDownEvent, Keystroke, Menu,
        MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions,
        Pixels, ScrollHandle, ScrollStrategy, SharedString, Stateful, SystemMenuType, TextRun,
        TitlebarOptions, UTF16Selection, UniformListScrollHandle, Window, WindowBounds,
        WindowControlArea, WindowDecorations, WindowOptions, canvas, div, ease_in_out, fill, img,
        point, prelude::*, px, rgb, size, svg, uniform_list,
    },
    std::{
        borrow::Cow,
        collections::{HashMap, HashSet},
        env, fs,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::{Arc, OnceLock},
        time::{Duration, Instant, SystemTime},
    },
    terminal_backend::{
        EMBEDDED_TERMINAL_DEFAULT_BG, EMBEDDED_TERMINAL_DEFAULT_FG, TerminalBackendKind,
        TerminalLaunch, TerminalModes,
    },
    theme::{ThemeKind, ThemePalette},
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TopBarIconKind {
    RemoteControl,
    GitHub,
    WorktreeActions,
    ReportIssue,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TopBarIconTone {
    Muted,
    Disabled,
    Connected,
    Busy,
}

fn find_assets_root_dir() -> Option<PathBuf> {
    if let Ok(exe) = env::current_exe() {
        let exe_dir = exe.parent()?;

        let macos_bundle = exe_dir.parent().map(|p| p.join("Resources"));
        if let Some(dir) = macos_bundle
            && dir.is_dir()
        {
            return Some(dir);
        }

        let share_dir = exe_dir.parent().map(|p| p.join("share").join("arbor"));
        if let Some(dir) = share_dir
            && dir.is_dir()
        {
            return Some(dir);
        }
    }

    let dev_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    if dev_dir.is_dir() {
        return Some(dev_dir);
    }

    None
}

fn find_asset_dir(relative_subdir: &str) -> Option<PathBuf> {
    let dir = find_assets_root_dir()?.join(relative_subdir);
    dir.is_dir().then_some(dir)
}

fn find_top_bar_icons_dir() -> Option<PathBuf> {
    static TOP_BAR_ICONS_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

    TOP_BAR_ICONS_DIR
        .get_or_init(|| find_asset_dir("icons/top-bar"))
        .clone()
}

fn find_ui_icons_dir() -> Option<PathBuf> {
    static UI_ICONS_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

    UI_ICONS_DIR
        .get_or_init(|| find_asset_dir("icons/ui"))
        .clone()
}

struct ArborAssets {
    base: PathBuf,
}

impl AssetSource for ArborAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(Cow::Owned(data)))
            .map_err(Into::into)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(Into::into)
    }
}

struct ArborWindow {
    app_config_store: Box<dyn app_config::AppConfigStore>,
    repository_store: Box<dyn repository_store::RepositoryStore>,
    daemon_session_store: Box<dyn daemon::DaemonSessionStore>,
    terminal_daemon: Option<terminal_daemon_http::SharedTerminalDaemonClient>,
    daemon_base_url: String,
    ui_state_store: Box<dyn ui_state_store::UiStateStore>,
    github_auth_store: Box<dyn github_auth_store::GithubAuthStore>,
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
    outposts: Vec<OutpostSummary>,
    outpost_store: Box<dyn arbor_core::outpost_store::OutpostStore>,
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
    settings_modal: Option<SettingsModal>,
    daemon_auth_modal: Option<DaemonAuthModal>,
    /// When set, a successful auth submission should retry fetching for this remote daemon index.
    pending_remote_daemon_auth: Option<usize>,
    start_daemon_modal: bool,
    connect_to_host_modal: Option<ConnectToHostModal>,
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
    window_is_active: bool,
    notice: Option<String>,
    theme_toast: Option<String>,
    theme_toast_generation: u64,
    right_pane_tab: RightPaneTab,
    right_pane_search: String,
    right_pane_search_cursor: usize,
    right_pane_search_active: bool,
    file_tree_entries: Vec<FileTreeEntry>,
    expanded_dirs: HashSet<PathBuf>,
    selected_file_tree_entry: Option<PathBuf>,
    left_pane_visible: bool,
    collapsed_repositories: HashSet<usize>,
    sidebar_order: HashMap<String, Vec<SidebarItemId>>,
    repository_context_menu: Option<RepositoryContextMenu>,
    worktree_context_menu: Option<WorktreeContextMenu>,
    worktree_hover_popover: Option<WorktreeHoverPopover>,
    _hover_show_task: Option<gpui::Task<()>>,
    _hover_dismiss_task: Option<gpui::Task<()>>,
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
        let app_config_store = app_config::default_app_config_store();
        let repository_store = repository_store::default_repository_store();
        let ui_state_store = ui_state_store::default_ui_state_store();
        let github_auth_store = github_auth_store::default_github_auth_store();
        let github_service = github_service::default_github_service();
        let notification_service = notifications::default_notification_service();
        let loaded_github_auth_state = github_auth_store.load();
        let config_path = app_config_store.config_path();
        let cwd = match env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                let mut notice_parts = vec![format!("failed to read current directory: {error}")];
                let loaded_config = app_config_store.load_or_create_config();
                notice_parts.extend(loaded_config.notices);
                let config_last_modified = app_config_store.config_last_modified();
                let github_auth_state = match loaded_github_auth_state.clone() {
                    Ok(state) => state,
                    Err(error) => {
                        notice_parts.push(format!("failed to load GitHub auth state: {error}"));
                        github_auth_store::GithubAuthState::default()
                    },
                };

                let repositories = match repository_store.load_entries() {
                    Ok(entries) => repository_store::resolve_repositories_from_entries(entries),
                    Err(err) => {
                        notice_parts.push(format!("failed to load saved repositories: {err}"));
                        Vec::new()
                    },
                };
                let active_repository_index = if repositories.is_empty() {
                    None
                } else {
                    Some(0)
                };
                let active_repository = active_repository_index
                    .and_then(|i| repositories.get(i))
                    .cloned();
                let repo_root = active_repository
                    .as_ref()
                    .map(|r| r.root.clone())
                    .unwrap_or_else(|| PathBuf::from("."));
                let github_repo_slug = active_repository.and_then(|r| r.github_repo_slug);

                let active_backend_kind = match parse_terminal_backend_kind(
                    loaded_config.config.terminal_backend.as_deref(),
                ) {
                    Ok(kind) => kind,
                    Err(err) => {
                        notice_parts.push(err);
                        TerminalBackendKind::Embedded
                    },
                };
                let theme_kind = match parse_theme_kind(loaded_config.config.theme.as_deref()) {
                    Ok(kind) => kind,
                    Err(err) => {
                        notice_parts.push(err);
                        ThemeKind::One
                    },
                };
                let configured_embedded_shell = loaded_config.config.embedded_shell.clone();
                let notifications_enabled = loaded_config.config.notifications.unwrap_or(true);
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
                let agent_presets = normalize_agent_presets(&loaded_config.config.agent_presets);
                let outpost_store = Box::new(arbor_core::outpost_store::default_outpost_store());
                let outposts = load_outpost_summaries(outpost_store.as_ref(), &remote_hosts);
                let (terminal_poll_tx, terminal_poll_rx) = std::sync::mpsc::channel();
                let startup_sidebar_order = startup_ui_state.sidebar_order.clone();

                let app = Self {
                    app_config_store,
                    repository_store,
                    daemon_session_store,
                    terminal_daemon: None,
                    daemon_base_url: DEFAULT_DAEMON_BASE_URL.to_owned(),
                    ui_state_store,
                    github_auth_store,
                    github_service,
                    github_auth_state,
                    github_auth_in_progress: false,
                    github_auth_copy_feedback_active: false,
                    github_auth_copy_feedback_generation: 0,
                    config_last_modified,
                    repositories,
                    active_repository_index,
                    repo_root,
                    github_repo_slug,
                    worktrees: Vec::new(),
                    worktree_stats_loading: false,
                    worktree_prs_loading: false,
                    active_worktree_index: None,
                    worktree_selection_epoch: 0,
                    changed_files: Vec::new(),
                    selected_changed_file: None,
                    terminals: Vec::new(),
                    terminal_poll_tx,
                    terminal_poll_rx: Some(terminal_poll_rx),
                    diff_sessions: Vec::new(),
                    active_diff_session_id: None,
                    file_view_sessions: Vec::new(),
                    active_file_view_session_id: None,
                    next_file_view_session_id: 1,
                    file_view_scroll_handle: UniformListScrollHandle::new(),
                    file_view_editing: false,
                    active_terminal_by_worktree: HashMap::new(),
                    next_terminal_id: 1,
                    next_diff_session_id: 1,
                    active_backend_kind,
                    configured_embedded_shell,
                    theme_kind,
                    left_pane_width: startup_ui_state
                        .left_pane_width
                        .map_or(DEFAULT_LEFT_PANE_WIDTH, |width| width as f32),
                    right_pane_width: startup_ui_state
                        .right_pane_width
                        .map_or(DEFAULT_RIGHT_PANE_WIDTH, |width| width as f32),
                    terminal_focus: cx.focus_handle(),
                    welcome_clone_focus: cx.focus_handle(),
                    terminal_scroll_handle: ScrollHandle::new(),
                    last_terminal_grid_size: None,
                    center_tabs_scroll_handle: ScrollHandle::new(),
                    diff_scroll_handle: UniformListScrollHandle::new(),
                    terminal_selection: None,
                    terminal_selection_drag_anchor: None,
                    create_modal: None,
                    preferred_checkout_kind: startup_ui_state
                        .preferred_checkout_kind
                        .unwrap_or_default(),
                    github_auth_modal: None,
                    delete_modal: None,
                    outposts,
                    outpost_store,
                    active_outpost_index: None,
                    remote_hosts,
                    ssh_connection_pool: Arc::new(arbor_ssh::connection::SshConnectionPool::new()),
                    ssh_daemon_tunnel: None,
                    manage_hosts_modal: None,
                    manage_presets_modal: None,
                    agent_presets,
                    active_preset_tab: None,
                    repo_presets: Vec::new(),
                    manage_repo_presets_modal: None,
                    show_about: false,
                    show_theme_picker: false,
                    settings_modal: None,
                    daemon_auth_modal: None,
                    pending_remote_daemon_auth: None,
                    start_daemon_modal: false,
                    connect_to_host_modal: None,
                    connection_history: connection_history::load_history(),
                    daemon_auth_tokens: connection_history::load_tokens(),
                    connected_daemon_label: None,
                    pending_diff_scroll_to_file: None,
                    focus_terminal_on_next_render: true,
                    git_action_in_flight: None,
                    top_bar_quick_actions_open: false,
                    top_bar_quick_actions_submenu: None,
                    ide_launchers: Vec::new(),
                    terminal_launchers: Vec::new(),
                    last_persisted_ui_state: startup_ui_state,
                    last_ui_state_error: None,
                    notification_service,
                    notifications_enabled,
                    window_is_active: true,
                    notice: (!notice_parts.is_empty()).then_some(notice_parts.join(" | ")),
                    theme_toast: None,
                    theme_toast_generation: 0,
                    right_pane_tab: RightPaneTab::Changes,
                    right_pane_search: String::new(),
                    right_pane_search_cursor: 0,
                    right_pane_search_active: false,
                    file_tree_entries: Vec::new(),
                    expanded_dirs: HashSet::new(),
                    selected_file_tree_entry: None,
                    left_pane_visible: true,
                    collapsed_repositories: HashSet::new(),
                    sidebar_order: startup_sidebar_order,
                    repository_context_menu: None,
                    worktree_context_menu: None,
                    worktree_hover_popover: None,
                    _hover_show_task: None,
                    _hover_dismiss_task: None,
                    last_mouse_position: point(px(0.), px(0.)),
                    outpost_context_menu: None,
                    discovered_daemons: Vec::new(),
                    mdns_browser: None,
                    active_discovered_daemon: None,
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
                    ime_marked_text: None,
                    welcome_clone_url: String::new(),
                    welcome_clone_url_cursor: 0,
                    welcome_clone_url_active: false,
                    welcome_cloning: false,
                    welcome_clone_error: None,
                    remote_daemon_states: HashMap::new(),
                    active_remote_worktree: None,
                };

                return app;
            },
        };

        let repo_root = worktree::repo_root(&cwd).ok();

        tracing::info!(config = %config_path.display(), "loading configuration");
        let loaded_config = app_config_store.load_or_create_config();
        let mut notice_parts = loaded_config.notices;
        let config_last_modified = app_config_store.config_last_modified();
        let github_auth_state = match loaded_github_auth_state {
            Ok(state) => state,
            Err(error) => {
                notice_parts.push(format!("failed to load GitHub auth state: {error}"));
                github_auth_store::GithubAuthState::default()
            },
        };

        if let Err(error) = daemon_session_store.load() {
            tracing::warn!(%error, "failed to load daemon session metadata");
            notice_parts.push(format!("failed to load daemon session metadata: {error}"));
        }
        let daemon_base_url =
            daemon_base_url_from_config(loaded_config.config.daemon_url.as_deref());
        tracing::info!(url = %daemon_base_url, "connecting to terminal daemon");
        let mut terminal_daemon =
            match terminal_daemon_http::default_terminal_daemon_client(&daemon_base_url) {
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
                    Ok(records) => {
                        // Check for version mismatch on local daemons and restart if needed.
                        if daemon_url_is_local(&daemon_base_url) {
                            if let Some((records, restarted)) =
                                check_daemon_version_and_restart(daemon, &daemon_base_url)
                            {
                                if let Some(new_daemon) = restarted {
                                    terminal_daemon = Some(new_daemon);
                                }
                                (records, true)
                            } else {
                                (records, true)
                            }
                        } else {
                            (records, true)
                        }
                    },
                    Err(error) => {
                        let error_text = error.to_string();
                        if daemon_error_is_connection_refused(&error_text) {
                            tracing::debug!("daemon not running, attempting auto-start");
                            if let Some(started) = try_auto_start_daemon(&daemon_base_url) {
                                let records = started.list_sessions().unwrap_or_default();
                                terminal_daemon = Some(started);
                                (records, true)
                            } else {
                                tracing::debug!("auto-start failed, falling back to cold restore");
                                terminal_daemon = None;
                                let cold_records = daemon_session_store.load().unwrap_or_default();
                                (cold_records, false)
                            }
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

        let repository_store_file_exists = repository_store.has_store_file();
        let mut loaded_entries_were_empty = false;
        let mut repositories = match repository_store.load_entries() {
            Ok(entries) => {
                loaded_entries_were_empty = entries.is_empty();
                repository_store::resolve_repositories_from_entries(entries)
            },
            Err(error) => {
                notice_parts.push(format!("failed to load saved repositories: {error}"));
                Vec::new()
            },
        };
        let mut persist_repositories = false;

        if let Some(ref root) = repo_root
            && !repositories
                .iter()
                .any(|repository| repository.contains_checkout_root(root))
            && should_seed_repo_root_from_cwd(
                repository_store_file_exists,
                loaded_entries_were_empty,
            )
        {
            repositories.push(RepositorySummary::from_checkout_roots(
                root.clone(),
                repository_store::default_group_key_for_root(root),
                vec![repository_store::RepositoryCheckoutRoot {
                    path: root.clone(),
                    kind: CheckoutKind::LinkedWorktree,
                }],
            ));
            persist_repositories = true;
        }

        let active_repository_index = if let Some(ref root) = repo_root {
            repositories
                .iter()
                .position(|repository| repository.contains_checkout_root(root))
                .or(Some(0))
        } else if !repositories.is_empty() {
            Some(0)
        } else {
            None
        };
        let active_repository = active_repository_index
            .and_then(|index| repositories.get(index))
            .cloned();

        if persist_repositories {
            let entries_to_save =
                repository_store::repository_entries_from_summaries(&repositories);
            if let Err(error) = repository_store.save_entries(&entries_to_save) {
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
        let agent_presets = normalize_agent_presets(&loaded_config.config.agent_presets);

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
        let configured_embedded_shell = loaded_config.config.embedded_shell.clone();
        let notifications_enabled = loaded_config.config.notifications.unwrap_or(true);
        let (terminal_poll_tx, terminal_poll_rx) = std::sync::mpsc::channel();

        let mut app = Self {
            app_config_store,
            repository_store,
            daemon_session_store,
            terminal_daemon,
            daemon_base_url,
            ui_state_store,
            github_auth_store,
            github_service,
            github_auth_state,
            github_auth_in_progress: false,
            github_auth_copy_feedback_active: false,
            github_auth_copy_feedback_generation: 0,
            config_last_modified,
            repositories,
            active_repository_index,
            repo_root: active_repository
                .as_ref()
                .map(|repository| repository.root.clone())
                .or(repo_root)
                .unwrap_or(cwd),
            github_repo_slug: active_repository.and_then(|repository| repository.github_repo_slug),
            worktrees: Vec::new(),
            worktree_stats_loading: false,
            worktree_prs_loading: false,
            active_worktree_index: None,
            worktree_selection_epoch: 0,
            changed_files: Vec::new(),
            selected_changed_file: None,
            terminals: Vec::new(),
            terminal_poll_tx,
            terminal_poll_rx: Some(terminal_poll_rx),
            diff_sessions: Vec::new(),
            active_diff_session_id: None,
            file_view_sessions: Vec::new(),
            active_file_view_session_id: None,
            next_file_view_session_id: 1,
            file_view_scroll_handle: UniformListScrollHandle::new(),
            file_view_editing: false,
            active_terminal_by_worktree: HashMap::new(),
            next_terminal_id: 1,
            next_diff_session_id: 1,
            active_backend_kind,
            configured_embedded_shell,
            theme_kind,
            left_pane_width: startup_ui_state
                .left_pane_width
                .map_or(DEFAULT_LEFT_PANE_WIDTH, |width| width as f32),
            right_pane_width: startup_ui_state
                .right_pane_width
                .map_or(DEFAULT_RIGHT_PANE_WIDTH, |width| width as f32),
            terminal_focus: cx.focus_handle(),
            welcome_clone_focus: cx.focus_handle(),
            terminal_scroll_handle: ScrollHandle::new(),
            last_terminal_grid_size: None,
            center_tabs_scroll_handle: ScrollHandle::new(),
            diff_scroll_handle: UniformListScrollHandle::new(),
            terminal_selection: None,
            terminal_selection_drag_anchor: None,
            create_modal: None,
            preferred_checkout_kind: startup_ui_state.preferred_checkout_kind.unwrap_or_default(),
            github_auth_modal: None,
            delete_modal: None,
            outposts,
            outpost_store,
            active_outpost_index: None,
            remote_hosts,
            ssh_connection_pool: Arc::new(arbor_ssh::connection::SshConnectionPool::new()),
            ssh_daemon_tunnel: None,
            manage_hosts_modal: None,
            manage_presets_modal: None,
            agent_presets,
            active_preset_tab: None,
            repo_presets: Vec::new(),
            manage_repo_presets_modal: None,
            show_about: false,
            show_theme_picker: false,
            settings_modal: None,
            daemon_auth_modal: None,
            pending_remote_daemon_auth: None,
            start_daemon_modal: false,
            connect_to_host_modal: None,
            connection_history: connection_history::load_history(),
            daemon_auth_tokens: connection_history::load_tokens(),
            connected_daemon_label: None,
            pending_diff_scroll_to_file: None,
            focus_terminal_on_next_render: true,
            git_action_in_flight: None,
            top_bar_quick_actions_open: false,
            top_bar_quick_actions_submenu: None,
            ide_launchers: Vec::new(),
            terminal_launchers: Vec::new(),
            left_pane_visible: startup_ui_state.left_pane_visible.unwrap_or(true),
            collapsed_repositories: HashSet::new(),
            sidebar_order: startup_ui_state.sidebar_order.clone(),
            repository_context_menu: None,
            worktree_context_menu: None,
            worktree_hover_popover: None,
            _hover_show_task: None,
            _hover_dismiss_task: None,
            last_mouse_position: point(px(0.), px(0.)),
            outpost_context_menu: None,
            discovered_daemons: Vec::new(),
            mdns_browser: None,
            active_discovered_daemon: None,
            worktree_nav_back: Vec::new(),
            worktree_nav_forward: Vec::new(),
            last_persisted_ui_state: startup_ui_state,
            last_ui_state_error: None,
            notification_service,
            notifications_enabled,
            window_is_active: true,
            notice: (!notice_parts.is_empty()).then_some(notice_parts.join(" | ")),
            theme_toast: None,
            theme_toast_generation: 0,
            right_pane_tab: RightPaneTab::Changes,
            right_pane_search: String::new(),
            right_pane_search_cursor: 0,
            right_pane_search_active: false,
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
            ime_marked_text: None,
            welcome_clone_url: String::new(),
            welcome_clone_url_cursor: 0,
            welcome_clone_url_active: false,
            welcome_cloning: false,
            welcome_clone_error: None,
            remote_daemon_states: HashMap::new(),
            active_remote_worktree: None,
        };

        app.refresh_worktrees(cx);
        app.refresh_repo_config_if_changed(cx);
        app.restore_terminal_sessions_from_records(initial_daemon_records, attach_daemon_runtime);
        let _ = app.ensure_selected_worktree_terminal();
        app.sync_daemon_session_store(cx);
        app.start_terminal_poller(cx);
        app.start_log_poller(cx);
        app.start_worktree_auto_refresh(cx);
        app.start_github_pr_auto_refresh(cx);
        app.start_config_auto_refresh(cx);
        app.start_agent_activity_ws(cx);
        app.start_daemon_log_ws(cx);
        #[cfg(feature = "mdns")]
        app.start_mdns_browser(cx);
        app.ensure_claude_code_hooks(cx);
        app.ensure_pi_agent_extension(cx);

        app
    }

    fn start_terminal_poller(&mut self, cx: &mut Context<Self>) {
        let Some(poll_rx) = self.terminal_poll_rx.take() else {
            return;
        };

        cx.spawn(async move |this, cx| {
            let (bridge_tx, bridge_rx) = smol::channel::bounded::<()>(1);

            cx.background_spawn(async move {
                loop {
                    // Wait for a notification or fall back to 45ms timeout (for SSH/daemon
                    // terminals that use pull-based polling without a reader thread).
                    let _ = poll_rx.recv_timeout(Duration::from_millis(45));
                    // Drain queued notifications to coalesce burst output.
                    while poll_rx.try_recv().is_ok() {}
                    // Small deadline window to batch rapid output (e.g. `cat large_file`).
                    std::thread::sleep(Duration::from_millis(4));
                    while poll_rx.try_recv().is_ok() {}
                    if bridge_tx.send(()).await.is_err() {
                        break;
                    }
                }
            })
            .detach();

            loop {
                if bridge_rx.recv().await.is_err() {
                    break;
                }
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

                let updated = this.update(cx, |this, cx| {
                    this.refresh_config_if_changed(cx);
                    this.refresh_repo_config_if_changed(cx);
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    #[cfg(feature = "mdns")]
    fn start_mdns_browser(&mut self, cx: &mut Context<Self>) {
        match mdns_browser::start_browsing() {
            Ok(browser) => {
                self.mdns_browser = Some(browser);
                tracing::info!("mDNS: browsing for _arbor._tcp services on the LAN");
            },
            Err(e) => {
                tracing::warn!("mDNS browsing unavailable: {e}");
                return;
            },
        }

        let local_hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_default();

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_secs(2));
                })
                .await;

                let updated = this.update(cx, |this, cx| {
                    if let Some(browser) = &this.mdns_browser {
                        let events = browser.poll_updates();
                        let mut changed = false;
                        for event in events {
                            match event {
                                mdns_browser::MdnsEvent::Added(daemon) => {
                                    // Skip our own instance
                                    if daemon.instance_name == local_hostname {
                                        tracing::debug!(
                                            name = %daemon.instance_name,
                                            "mDNS: ignoring own instance"
                                        );
                                        continue;
                                    }
                                    tracing::info!(
                                        name = %daemon.instance_name,
                                        host = %daemon.host,
                                        addresses = ?daemon.addresses,
                                        port = daemon.port,
                                        has_auth = daemon.has_auth,
                                        "mDNS: discovered LAN daemon"
                                    );
                                    // Update existing or insert new
                                    if let Some(existing) = this
                                        .discovered_daemons
                                        .iter_mut()
                                        .find(|d| d.instance_name == daemon.instance_name)
                                    {
                                        if existing != &daemon {
                                            *existing = daemon;
                                            changed = true;
                                        }
                                    } else {
                                        this.discovered_daemons.push(daemon);
                                        changed = true;
                                    }
                                },
                                mdns_browser::MdnsEvent::Removed(name) => {
                                    tracing::info!(name = %name, "mDNS: LAN daemon removed");
                                    let before = this.discovered_daemons.len();
                                    this.discovered_daemons.retain(|d| d.instance_name != name);
                                    if this.discovered_daemons.len() != before {
                                        changed = true;
                                        // Rebuild remote_daemon_states with new indices
                                        let new_states: HashMap<usize, RemoteDaemonState> = this
                                            .remote_daemon_states
                                            .drain()
                                            .filter(|(idx, _)| *idx < this.discovered_daemons.len())
                                            .collect();
                                        this.remote_daemon_states = new_states;
                                        if let Some(idx) = this.active_discovered_daemon
                                            && idx >= this.discovered_daemons.len()
                                        {
                                            this.active_discovered_daemon = None;
                                        }
                                    }
                                },
                            }
                        }
                        if changed {
                            cx.set_menus(build_app_menus(&this.discovered_daemons));
                            cx.notify();
                        }
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn refresh_config_if_changed(&mut self, cx: &mut Context<Self>) {
        let next_modified = self.app_config_store.config_last_modified();
        if self.config_last_modified == next_modified {
            return;
        }
        self.config_last_modified = next_modified;

        let loaded = self.app_config_store.load_or_create_config();
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

        if self.configured_embedded_shell != loaded.config.embedded_shell {
            self.configured_embedded_shell = loaded.config.embedded_shell.clone();
            changed = true;
        }

        let next_daemon_base_url = daemon_base_url_from_config(loaded.config.daemon_url.as_deref());
        if self.daemon_base_url != next_daemon_base_url {
            // Remove hooks pointing at the old daemon before switching
            remove_claude_code_hooks();
            remove_pi_agent_extension();
            self.daemon_base_url = next_daemon_base_url.clone();
            self.terminal_daemon =
                match terminal_daemon_http::default_terminal_daemon_client(&next_daemon_base_url) {
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
                        remove_claude_code_hooks();
                        remove_pi_agent_extension();
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

        let next_agent_presets = normalize_agent_presets(&loaded.config.agent_presets);
        if self.agent_presets != next_agent_presets {
            self.agent_presets = next_agent_presets;
            if let Some(modal) = self.manage_presets_modal.as_mut()
                && let Some(preset) = self
                    .agent_presets
                    .iter()
                    .find(|preset| preset.kind == modal.active_preset)
            {
                modal.command = preset.command.clone();
            }
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

    fn refresh_repo_config_if_changed(&mut self, cx: &mut Context<Self>) {
        let next_presets = self.load_all_repo_presets();
        if self.repo_presets != next_presets {
            self.repo_presets = next_presets;
            cx.notify();
        }
    }

    fn load_all_repo_presets(&self) -> Vec<RepoPreset> {
        let mut presets = load_repo_presets(self.app_config_store.as_ref(), &self.repo_root);
        if let Some(wt_path) = self.selected_worktree_path()
            && wt_path != self.repo_root.as_path()
        {
            for wt_preset in load_repo_presets(self.app_config_store.as_ref(), wt_path) {
                if !presets.iter().any(|p| p.name == wt_preset.name) {
                    presets.push(wt_preset);
                }
            }
        }
        presets
    }

    /// Returns the directory where repo preset edits should be saved.
    /// Prefers the selected worktree path, falls back to repo_root.
    fn active_arbor_toml_dir(&self) -> PathBuf {
        self.selected_worktree_path()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.repo_root.clone())
    }

    fn sync_daemon_session_store(&mut self, cx: &mut Context<Self>) {
        let shell = self.embedded_shell();
        let updated_at_unix_ms = current_unix_timestamp_millis();

        let records: Vec<DaemonSessionRecord> = self
            .terminals
            .iter()
            .map(|session| DaemonSessionRecord {
                session_id: session.daemon_session_id.clone(),
                workspace_id: session.worktree_path.display().to_string().into(),
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

        // Don't restore terminals without a live runtime — they become
        // non-functional "ghost" sessions that show old output but cannot
        // accept input.  A fresh terminal will be created on demand.
        if !attach_runtime {
            tracing::debug!(
                count = records.len(),
                "skipping cold terminal restore (no daemon runtime available)"
            );
            return false;
        }

        records.sort_by(|left, right| {
            right
                .updated_at_unix_ms
                .unwrap_or(0)
                .cmp(&left.updated_at_unix_ms.unwrap_or(0))
                .then_with(|| left.session_id.as_str().cmp(right.session_id.as_str()))
        });

        let mut changed = false;

        for record in records {
            if record.session_id.as_str().trim().is_empty() {
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
                    session.modes = TerminalModes::default();
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
                if attach_runtime && let Some(daemon) = self.terminal_daemon.as_ref() {
                    session.runtime = Some(local_daemon_runtime(
                        daemon.clone(),
                        session.daemon_session_id.to_string(),
                        Some(self.terminal_poll_tx.clone()),
                    ));
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
                    modes: TerminalModes::default(),
                    last_runtime_sync_at: None,
                    runtime: attach_runtime
                        .then(|| {
                            self.terminal_daemon.as_ref().map(|daemon| {
                                local_daemon_runtime(
                                    daemon.clone(),
                                    record.session_id.to_string(),
                                    Some(self.terminal_poll_tx.clone()),
                                )
                            })
                        })
                        .flatten(),
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

        let workspace_path = PathBuf::from(record.workspace_id.as_str());
        self.match_worktree_path(workspace_path.as_path())
    }

    fn match_worktree_path(&self, candidate: &Path) -> Option<PathBuf> {
        self.worktrees
            .iter()
            .find(|worktree| paths_equivalent(worktree.path.as_path(), candidate))
            .map(|worktree| worktree.path.clone())
    }

    fn maybe_notify(&self, title: &str, body: &str, play_sound: bool) {
        if self.notifications_enabled && !self.window_is_active {
            self.notification_service.send(title, body, play_sound);
        }
    }

    fn sync_running_terminals(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let follow_output = terminal_scroll_is_near_bottom(&self.terminal_scroll_handle);
        let active_terminal_id = self.active_terminal_id_for_selected_worktree();
        let target_grid_size =
            terminal_grid_size_from_scroll_handle(&self.terminal_scroll_handle, cx);
        let now = Instant::now();
        if let Some((rows, cols, ..)) = target_grid_size {
            self.last_terminal_grid_size = Some((rows, cols));
        }
        let mut sessions_to_close = Vec::new();
        let mut pending_notifications = Vec::new();
        let sync_indices = ordered_terminal_sync_indices(&self.terminals, active_terminal_id);

        for index in sync_indices {
            let Some(runtime) = self
                .terminals
                .get(index)
                .and_then(|session| session.runtime.clone())
            else {
                continue;
            };

            let session_id = self.terminals[index].id;
            let is_active = active_terminal_id == Some(session_id);
            if !runtime.should_sync(&self.terminals[index], is_active, target_grid_size, now) {
                continue;
            }
            let outcome = {
                let session = &mut self.terminals[index];
                runtime.sync(session, is_active, target_grid_size)
            };
            self.terminals[index].last_runtime_sync_at = Some(now);

            changed |= outcome.changed;

            if outcome.clear_global_daemon {
                self.terminal_daemon = None;
            }
            if let Some(notice) = outcome.notice {
                self.notice = Some(notice);
            }
            if let Some(notification) = outcome.notification {
                pending_notifications.push(notification);
            }
            if outcome.close_session {
                sessions_to_close.push(session_id);
            }
        }

        for notification in pending_notifications {
            self.maybe_notify(
                &notification.title,
                &notification.body,
                notification.play_sound,
            );
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
        let previous_pr_details: HashMap<PathBuf, github_service::PrDetails> = self
            .worktrees
            .iter()
            .filter_map(|worktree| {
                worktree
                    .pr_details
                    .as_ref()
                    .map(|details| (worktree.path.clone(), details.clone()))
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
        let previous_agent_tasks: HashMap<PathBuf, String> = self
            .worktrees
            .iter()
            .filter_map(|worktree| {
                worktree
                    .agent_task
                    .as_ref()
                    .map(|task| (worktree.path.clone(), task.clone()))
            })
            .collect();
        let previous_activity: HashMap<PathBuf, u64> = self
            .worktrees
            .iter()
            .filter_map(|worktree| {
                worktree
                    .last_activity_unix_ms
                    .map(|ts| (worktree.path.clone(), ts))
            })
            .collect();

        let mut refresh_errors = Vec::new();
        let mut next_worktrees = Vec::new();
        let mut seen_worktree_paths = HashSet::new();

        for repository in &self.repositories {
            for checkout_root in &repository.checkout_roots {
                match worktree::list(&checkout_root.path) {
                    Ok(entries) => {
                        for entry in entries {
                            if !seen_worktree_paths.insert(entry.path.clone()) {
                                continue;
                            }
                            next_worktrees.push(WorktreeSummary::from_worktree(
                                &entry,
                                &checkout_root.path,
                                &repository.group_key,
                                if checkout_root.kind == CheckoutKind::DiscreteClone
                                    && entry.path == checkout_root.path
                                {
                                    CheckoutKind::DiscreteClone
                                } else {
                                    CheckoutKind::LinkedWorktree
                                },
                            ));
                        }
                    },
                    Err(error) => {
                        refresh_errors.push(format!(
                            "{} ({}): {error}",
                            repository.label,
                            checkout_root.path.display()
                        ));
                    },
                }
            }
        }

        for worktree in &mut next_worktrees {
            worktree.diff_summary = previous_summaries.get(&worktree.path).copied();
            worktree.pr_number = previous_pr_numbers.get(&worktree.path).copied();
            worktree.pr_url = previous_pr_urls.get(&worktree.path).cloned();
            worktree.pr_details = previous_pr_details.get(&worktree.path).cloned();
            worktree.agent_state = previous_agent_states.get(&worktree.path).copied();
            worktree.agent_task = previous_agent_tasks.get(&worktree.path).cloned();
            // Take the max of fresh git-based timestamp and previous value
            // (which may include agent activity).
            let prev = previous_activity.get(&worktree.path).copied();
            worktree.last_activity_unix_ms = match (worktree.last_activity_unix_ms, prev) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
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
                                .position(|worktree| worktree.group_key == repository.group_key)
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
        self.refresh_agent_tasks(cx);
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

    fn refresh_agent_tasks(&mut self, cx: &mut Context<Self>) {
        let worktree_paths: Vec<PathBuf> = self
            .worktrees
            .iter()
            .filter(|wt| wt.agent_task.is_none())
            .map(|wt| wt.path.clone())
            .collect();
        if worktree_paths.is_empty() {
            return;
        }

        cx.spawn(async move |this, cx| {
            let results = cx
                .background_spawn(async move {
                    worktree_paths
                        .into_iter()
                        .map(|path| {
                            let task = arbor_core::session::extract_agent_task(&path);
                            (path, task)
                        })
                        .collect::<Vec<_>>()
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let mut changed = false;
                for (path, task) in results {
                    if let Some(task) = task
                        && let Some(wt) = this.worktrees.iter_mut().find(|wt| wt.path == path)
                    {
                        wt.agent_task = Some(task);
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
        let daemon = self.terminal_daemon.clone();
        cx.spawn(async move |this, cx| {
            let mut backoff_secs = 3u64;

            loop {
                let connect_config = daemon
                    .as_ref()
                    .and_then(|daemon| {
                        daemon
                            .websocket_connect_config("/api/v1/agent/activity/ws")
                            .ok()
                    })
                    .or_else(|| {
                        daemon_url_is_local(&daemon_base_url).then(|| {
                            terminal_daemon_http::WebsocketConnectConfig {
                                url: daemon_base_url
                                    .replace("http://", "ws://")
                                    .replace("https://", "wss://")
                                    + "/api/v1/agent/activity/ws",
                                auth_token: None,
                            }
                        })
                    });
                let (tx, rx) = smol::channel::unbounded::<Option<String>>();

                cx.background_spawn(async move {
                    let Some(connect_config) = connect_config else {
                        let _ = tx.send(None).await;
                        return;
                    };
                    let request = match daemon_websocket_request(&connect_config) {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::debug!(%error, "failed to build agent activity websocket request");
                            let _ = tx.send(None).await;
                            return;
                        },
                    };

                    let Ok((mut ws, _)) = tungstenite::connect(request) else {
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
                if let Ok(Some(text)) = first {
                    tracing::info!("agent activity WS connected");
                    backoff_secs = 3;
                    // Process the first message
                    process_agent_ws_message(&this, cx, &text);

                    // Process subsequent messages
                    while let Ok(Some(text)) = rx.recv().await {
                        process_agent_ws_message(&this, cx, &text);
                    }
                }

                tracing::debug!("agent activity WS disconnected, will retry");

                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_secs(backoff_secs));
                })
                .await;
                backoff_secs = (backoff_secs * 2).min(30);
            }
        })
        .detach();
    }

    fn start_daemon_log_ws(&mut self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        let daemon = self.terminal_daemon.clone();
        let log_buffer = self.log_buffer.clone();
        cx.spawn(async move |_this, cx| {
            let mut backoff_secs = 3u64;

            loop {
                let connect_config = daemon
                    .as_ref()
                    .and_then(|daemon| daemon.websocket_connect_config("/api/v1/logs/ws").ok())
                    .or_else(|| {
                        daemon_url_is_local(&daemon_base_url).then(|| {
                            terminal_daemon_http::WebsocketConnectConfig {
                                url: daemon_base_url
                                    .replace("http://", "ws://")
                                    .replace("https://", "wss://")
                                    + "/api/v1/logs/ws",
                                auth_token: None,
                            }
                        })
                    });
                let (tx, rx) = smol::channel::unbounded::<Option<String>>();

                cx.background_spawn(async move {
                    let Some(connect_config) = connect_config else {
                        let _ = tx.send(None).await;
                        return;
                    };
                    let request = match daemon_websocket_request(&connect_config) {
                        Ok(request) => request,
                        Err(_) => {
                            let _ = tx.send(None).await;
                            return;
                        },
                    };

                    let Ok((mut ws, _)) = tungstenite::connect(request) else {
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
                if let Ok(Some(text)) = first {
                    tracing::info!("daemon log WS connected");
                    backoff_secs = 3;
                    inject_daemon_log_entry(&log_buffer, &text);

                    while let Ok(Some(text)) = rx.recv().await {
                        inject_daemon_log_entry(&log_buffer, &text);
                    }
                }

                tracing::debug!("daemon log WS disconnected, will retry");

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

    fn ensure_pi_agent_extension(&self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        cx.spawn(async move |_this, cx| {
            cx.background_spawn(async move {
                if let Err(error) = install_pi_agent_extension(&daemon_base_url) {
                    tracing::warn!(%error, "failed to install Pi activity extension");
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

        let repository_slug_by_group_key: HashMap<String, String> = self
            .repositories
            .iter()
            .filter_map(|repository| {
                repository
                    .github_repo_slug
                    .as_ref()
                    .map(|slug| (repository.group_key.clone(), slug.clone()))
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
                    repository_slug_by_group_key
                        .get(&worktree.group_key)
                        .cloned(),
                )
            })
            .collect();
        let github_token = self.github_access_token();
        let github_service = self.github_service.clone();

        if tracked_branches.is_empty() {
            let mut changed = false;
            for worktree in &mut self.worktrees {
                if worktree.pr_number.take().is_some()
                    || worktree.pr_url.take().is_some()
                    || worktree.pr_details.take().is_some()
                {
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
            let results = cx
                .background_spawn(async move {
                    tracked_branches
                        .into_iter()
                        .map(|(path, branch, repo_slug)| {
                            // Try gh CLI first for rich details
                            let details = repo_slug.as_ref().and_then(|slug| {
                                github_service::pull_request_details(slug, &branch)
                            });

                            let (pr_number, pr_url) = if let Some(ref d) = details {
                                (Some(d.number), Some(d.url.clone()))
                            } else {
                                // Fall back to octocrab for just the number
                                let num = repo_slug.as_ref().and_then(|_| {
                                    github_pr_number_for_worktree(
                                        github_service.as_ref(),
                                        &path,
                                        &branch,
                                        github_token.as_deref(),
                                    )
                                });
                                let url = num.and_then(|n| {
                                    repo_slug.as_ref().map(|slug| github_pr_url(slug, n))
                                });
                                (num, url)
                            };

                            (path, pr_number, pr_url, details)
                        })
                        .collect::<Vec<_>>()
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.worktree_prs_loading = false;

                type PrInfo = (
                    Option<u64>,
                    Option<String>,
                    Option<github_service::PrDetails>,
                );
                let pr_by_path: HashMap<PathBuf, PrInfo> = results
                    .into_iter()
                    .map(|(path, num, url, details)| (path, (num, url, details)))
                    .collect();

                let mut changed = false;

                for worktree in &mut this.worktrees {
                    let (next_num, next_url, next_details) = pr_by_path
                        .get(&worktree.path)
                        .cloned()
                        .unwrap_or((None, None, None));
                    if worktree.pr_number != next_num || worktree.pr_url != next_url {
                        worktree.pr_number = next_num;
                        worktree.pr_url = next_url;
                        changed = true;
                    }
                    // Always update pr_details (no PartialEq on PrDetails)
                    let had_details = worktree.pr_details.is_some();
                    let has_details = next_details.is_some();
                    worktree.pr_details = next_details;
                    if had_details != has_details {
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
        if let Some(ref arw) = self.active_remote_worktree {
            return Some(arw.worktree_path.as_path());
        }
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

    fn selected_local_worktree_path(&self) -> Option<&Path> {
        self.active_worktree()
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
                                .is_some_and(|rt| rt.kind() == TerminalRuntimeKind::Outpost)
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
                                    .is_some_and(|rt| rt.kind() == TerminalRuntimeKind::Outpost)
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
                            .is_some_and(|rt| rt.kind() == TerminalRuntimeKind::Outpost)
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

        if let Some(fv_id) = self.active_file_view_session_id {
            let worktree_path = self.selected_worktree_path()?;
            if self
                .file_view_sessions
                .iter()
                .any(|s| s.id == fv_id && s.worktree_path.as_path() == worktree_path)
            {
                return Some(CenterTab::FileView(fv_id));
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

        if let Some(session) = self.terminals.get(index)
            && let Some(runtime) = session.runtime.as_ref()
            && let Err(error) = runtime.close(session)
        {
            self.notice = Some(format!("failed to close terminal session: {error}"));
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

    fn selected_worktree_file_view_sessions(&self) -> Vec<&FileViewSession> {
        let Some(worktree_path) = self.selected_worktree_path() else {
            return Vec::new();
        };

        self.file_view_sessions
            .iter()
            .filter(|session| session.worktree_path.as_path() == worktree_path)
            .collect()
    }

    fn open_file_view_tab(&mut self, file_path: PathBuf, cx: &mut Context<Self>) {
        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            return;
        };

        // If a session already exists for this file+worktree, just activate it.
        if let Some(existing) = self
            .file_view_sessions
            .iter()
            .find(|s| s.worktree_path == worktree_path && s.file_path == file_path)
        {
            self.active_file_view_session_id = Some(existing.id);
            self.active_diff_session_id = None;
            self.logs_tab_active = false;
            cx.notify();
            return;
        }

        let session_id = self.next_file_view_session_id;
        self.next_file_view_session_id += 1;

        let title = file_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_path.to_string_lossy().into_owned());

        let full_path = worktree_path.join(&file_path);
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let is_image = matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "svg" | "tiff" | "tif"
        );

        if is_image {
            self.file_view_sessions.push(FileViewSession {
                id: session_id,
                worktree_path: worktree_path.clone(),
                file_path: file_path.clone(),
                title,
                content: FileViewContent::Image(full_path),
                is_loading: false,
                cursor: FileViewCursor { line: 0, col: 0 },
            });
            self.active_file_view_session_id = Some(session_id);
            self.active_diff_session_id = None;
            self.logs_tab_active = false;
            cx.notify();
            return;
        }

        self.file_view_sessions.push(FileViewSession {
            id: session_id,
            worktree_path: worktree_path.clone(),
            file_path: file_path.clone(),
            title,
            content: FileViewContent::Text {
                highlighted: Arc::from([]),
                raw_lines: Vec::new(),
                dirty: false,
            },
            is_loading: true,
            cursor: FileViewCursor { line: 0, col: 0 },
        });
        self.active_file_view_session_id = Some(session_id);
        self.active_diff_session_id = None;
        self.logs_tab_active = false;

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let default_color: u32 = 0xc8ccd4;
                    match fs::read_to_string(&full_path) {
                        Ok(content) => {
                            let raw: Vec<String> = content.lines().map(String::from).collect();
                            let highlighted =
                                highlight_lines_with_syntect(&raw, &ext, default_color);
                            (raw, highlighted)
                        },
                        Err(error) => {
                            let msg = format!("Error reading file: {error}");
                            (vec![msg.clone()], vec![vec![FileViewSpan {
                                text: msg,
                                color: default_color,
                            }]])
                        },
                    }
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if let Some(session) = this
                    .file_view_sessions
                    .iter_mut()
                    .find(|s| s.id == session_id)
                {
                    session.content = FileViewContent::Text {
                        highlighted: Arc::from(result.1),
                        raw_lines: result.0,
                        dirty: false,
                    };
                    session.is_loading = false;
                    cx.notify();
                }
            });
        })
        .detach();

        cx.notify();
    }

    fn select_file_view_tab(&mut self, session_id: u64, cx: &mut Context<Self>) {
        if self.active_file_view_session_id == Some(session_id) && !self.logs_tab_active {
            return;
        }
        self.active_file_view_session_id = Some(session_id);
        self.active_diff_session_id = None;
        self.logs_tab_active = false;
        cx.notify();
    }

    fn close_file_view_session_by_id(&mut self, session_id: u64) -> bool {
        let Some(index) = self
            .file_view_sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return false;
        };

        self.file_view_sessions.remove(index);
        if self.active_file_view_session_id == Some(session_id) {
            self.active_file_view_session_id = None;
            self.file_view_editing = false;
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
            Some(CenterTab::FileView(session_id)) => {
                if self.close_file_view_session_by_id(session_id) {
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

    fn embedded_shell(&self) -> String {
        if let Some(shell) = &self.configured_embedded_shell {
            return shell.clone();
        }
        match env::var("SHELL") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => "/bin/zsh".to_owned(),
        }
    }

    fn selected_repository(&self) -> Option<&RepositorySummary> {
        self.active_repository_index
            .and_then(|index| self.repositories.get(index))
    }

    fn set_repositories_preserving_state(&mut self, repositories: Vec<RepositorySummary>) {
        let active_group_key = self
            .active_repository_index
            .and_then(|index| self.repositories.get(index))
            .map(|repository| repository.group_key.clone());
        let collapsed_group_keys: HashSet<String> = self
            .collapsed_repositories
            .iter()
            .filter_map(|index| self.repositories.get(*index))
            .map(|repository| repository.group_key.clone())
            .collect();

        self.repositories = repositories;
        self.collapsed_repositories = self
            .repositories
            .iter()
            .enumerate()
            .filter_map(|(index, repository)| {
                collapsed_group_keys
                    .contains(&repository.group_key)
                    .then_some(index)
            })
            .collect();
        self.active_repository_index = active_group_key
            .as_ref()
            .and_then(|group_key| {
                self.repositories
                    .iter()
                    .position(|repository| &repository.group_key == group_key)
            })
            .or_else(|| (!self.repositories.is_empty()).then_some(0));

        if let Some(repository) = self.selected_repository().cloned() {
            self.repo_root = repository.root.clone();
            self.github_repo_slug = repository.github_repo_slug.clone();
        } else {
            self.github_repo_slug = None;
        }
    }

    fn upsert_repository_checkout_root(
        &mut self,
        root: PathBuf,
        kind: CheckoutKind,
        group_key: String,
    ) {
        let mut entries = repository_store::repository_entries_from_summaries(&self.repositories);
        entries.push(repository_store::StoredRepositoryEntry {
            root: root.clone(),
            group_key: Some(group_key),
            kind,
        });
        let repositories = repository_store::resolve_repositories_from_entries(entries);
        self.set_repositories_preserving_state(repositories);
        if let Some(index) = self
            .repositories
            .iter()
            .position(|repository| repository.contains_checkout_root(&root))
        {
            self.active_repository_index = Some(index);
        }
    }

    fn remove_repository_checkout_root(&mut self, root: &Path) {
        let entries = repository_store::repository_entries_from_summaries(&self.repositories)
            .into_iter()
            .filter(|entry| entry.root != root)
            .collect();
        let repositories = repository_store::resolve_repositories_from_entries(entries);
        self.set_repositories_preserving_state(repositories);
    }

    fn sync_active_repository_from_selected_worktree(&mut self) {
        let Some(worktree_group_key) = self
            .active_worktree()
            .map(|worktree| worktree.group_key.clone())
        else {
            return;
        };

        let Some(index) = self
            .repositories
            .iter()
            .position(|repository| repository.group_key == worktree_group_key)
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
                .find(|repository| repository.group_key == worktree.group_key)
                .map(|repository| repository.label.clone())
                .unwrap_or_else(|| repository_display_name(&worktree.repo_root));
        }

        self.selected_repository()
            .map(|repository| repository.label.clone())
            .unwrap_or_else(|| repository_display_name(&self.repo_root))
    }

    fn select_repository(&mut self, index: usize, cx: &mut Context<Self>) {
        self.repository_context_menu = None;
        self.worktree_context_menu = None;
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
        self.active_file_view_session_id = None;
        self.active_worktree_index = self
            .worktrees
            .iter()
            .position(|worktree| worktree.group_key == repository.group_key);
        self.refresh_worktrees(cx);
        self.refresh_repo_config_if_changed(cx);
        self.focus_terminal_on_next_render = true;
        cx.notify();
    }

    fn persist_repositories(&mut self, cx: &mut Context<Self>) {
        let entries_to_save =
            repository_store::repository_entries_from_summaries(&self.repositories);
        if let Err(error) = self.repository_store.save_entries(&entries_to_save) {
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
            .position(|repository| repository.contains_checkout_root(&repository_root))
        {
            self.select_repository(index, cx);
            self.notice = Some(format!(
                "repository `{}` is already added",
                repository_display_name(&repository_root)
            ));
            cx.notify();
            return;
        }

        let repository = RepositorySummary::from_checkout_roots(
            repository_root.clone(),
            repository_store::default_group_key_for_root(&repository_root),
            vec![repository_store::RepositoryCheckoutRoot {
                path: repository_root.clone(),
                kind: CheckoutKind::LinkedWorktree,
            }],
        );
        let repository_label = repository.label.clone();
        let mut next_repositories = self.repositories.clone();
        next_repositories.push(repository);
        self.set_repositories_preserving_state(next_repositories);
        self.persist_repositories(cx);
        let index = self
            .repositories
            .iter()
            .position(|entry| entry.contains_checkout_root(&repository_root))
            .unwrap_or_else(|| self.repositories.len().saturating_sub(1));
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

    fn submit_welcome_clone(&mut self, cx: &mut Context<Self>) {
        let url = self.welcome_clone_url.trim().to_owned();
        if url.is_empty() {
            self.welcome_clone_error = Some("Please enter a repository URL".to_owned());
            cx.notify();
            return;
        }
        if self.welcome_cloning {
            return;
        }

        let repo_name = extract_repo_name_from_url(&url);
        if repo_name.is_empty() {
            self.welcome_clone_error =
                Some("Could not determine repository name from URL".to_owned());
            cx.notify();
            return;
        }

        let clone_dir = match user_home_dir() {
            Ok(home) => home.join(".arbor").join("repos").join(&repo_name),
            Err(error) => {
                self.welcome_clone_error = Some(error);
                cx.notify();
                return;
            },
        };

        if clone_dir.exists() {
            self.add_repository_from_path(clone_dir, cx);
            self.welcome_clone_url.clear();
            self.welcome_clone_url_active = false;
            self.welcome_clone_error = None;
            return;
        }

        self.welcome_cloning = true;
        self.welcome_clone_error = None;
        cx.notify();

        let clone_url = url.clone();
        let target = clone_dir.clone();
        cx.spawn(async move |this, cx| {
            let result = std::thread::spawn(move || {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        format!("failed to create directory `{}`: {e}", parent.display())
                    })?;
                }
                let output = Command::new("git")
                    .arg("clone")
                    .arg(&clone_url)
                    .arg(&target)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
                    .map_err(|e| format!("failed to run git clone: {e}"))?;

                if output.status.success() {
                    Ok(target)
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!("git clone failed: {}", stderr.trim()))
                }
            })
            .join()
            .unwrap_or_else(|_| Err("git clone thread panicked".to_owned()));

            let _ = this.update(cx, |this, cx| match result {
                Ok(cloned_path) => {
                    this.welcome_cloning = false;
                    this.welcome_clone_url.clear();
                    this.welcome_clone_url_active = false;
                    this.welcome_clone_error = None;
                    this.add_repository_from_path(cloned_path, cx);
                },
                Err(error) => {
                    this.welcome_cloning = false;
                    this.welcome_clone_error = Some(error);
                    cx.notify();
                },
            });
        })
        .detach();
    }

    fn render_welcome_pane(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let clone_url_active = self.welcome_clone_url_active;
        let cloning = self.welcome_cloning;
        let clone_error = self.welcome_clone_error.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(rgb(theme.app_bg))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(theme.text_primary))
                    .child("Welcome to Arbor"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(theme.text_muted))
                    .text_center()
                    .max_w(px(460.))
                    .child("Get started by adding a repository. You can open a local git repository or clone one from a URL."),
            )
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(420.))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_muted))
                            .child("CLONE FROM URL"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                single_line_input_field(
                                    theme,
                                    "welcome-clone-url",
                                    &self.welcome_clone_url,
                                    self.welcome_clone_url_cursor,
                                    "https://github.com/user/repo or git@github.com:user/repo.git",
                                    clone_url_active,
                                )
                                .track_focus(&self.welcome_clone_focus)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                        window.focus(&this.welcome_clone_focus);
                                        this.welcome_clone_url_active = true;
                                        this.welcome_clone_url_cursor =
                                            char_count(&this.welcome_clone_url);
                                        cx.notify();
                                    }),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.welcome_clone_url_active = true;
                                    this.welcome_clone_url_cursor =
                                        char_count(&this.welcome_clone_url);
                                    cx.notify();
                                })),
                            )
                            .when_some(clone_error, |this, error| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.notice_text))
                                        .child(error),
                                )
                            })
                            .child(
                                action_button(
                                    theme,
                                    "welcome-clone-button",
                                    if cloning { "Cloning..." } else { "Clone Repository" },
                                    ActionButtonStyle::Primary,
                                    !cloning,
                                )
                                .when(!cloning, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_welcome_clone(cx);
                                    }))
                                }),
                            ),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(1.))
                                    .bg(rgb(theme.border)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child("or"),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(1.))
                                    .bg(rgb(theme.border)),
                            ),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_muted))
                            .child("LOCAL REPOSITORY"),
                    )
                    .child(
                        action_button(
                            theme,
                            "welcome-add-local",
                            "Open Local Repository",
                            ActionButtonStyle::Secondary,
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_add_repository_picker(cx);
                        })),
                    ),
            )
    }

    fn open_external_url(&mut self, url: &str, cx: &mut Context<Self>) {
        cx.open_url(url);
    }

    fn close_github_auth_modal(&mut self, cx: &mut Context<Self>) {
        self.github_auth_copy_feedback_active = false;
        if self.github_auth_modal.take().is_some() {
            cx.notify();
        }
    }

    fn copy_github_auth_code_to_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.github_auth_modal.as_ref() else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(modal.user_code.clone()));
        self.github_auth_copy_feedback_active = true;
        self.github_auth_copy_feedback_generation =
            self.github_auth_copy_feedback_generation.saturating_add(1);
        let generation = self.github_auth_copy_feedback_generation;
        self.notice = Some("GitHub device code copied to clipboard".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                std::thread::sleep(GITHUB_AUTH_COPY_FEEDBACK_DURATION);
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                if this.github_auth_copy_feedback_generation == generation
                    && this.github_auth_copy_feedback_active
                {
                    this.github_auth_copy_feedback_active = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn copy_settings_daemon_auth_token_to_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.settings_modal.as_ref() else {
            return;
        };
        if modal.daemon_auth_token.trim().is_empty() {
            return;
        }

        cx.write_to_clipboard(ClipboardItem::new_string(modal.daemon_auth_token.clone()));
        self.notice = Some("Daemon auth token copied to clipboard".to_owned());
        cx.notify();
    }

    fn open_github_auth_verification_page(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.github_auth_modal.as_ref() else {
            return;
        };

        let url = modal.verification_url.clone();
        self.open_external_url(&url, cx);
    }

    fn has_persisted_github_token(&self) -> bool {
        self.github_auth_state
            .access_token
            .as_deref()
            .and_then(non_empty_trimmed_str)
            .is_some()
    }

    fn github_access_token(&self) -> Option<String> {
        resolve_github_access_token(self.github_auth_state.access_token.as_deref())
    }

    fn persist_github_auth_state(&self) -> Result<(), String> {
        self.github_auth_store.save(&self.github_auth_state)
    }

    fn clear_saved_github_token(&mut self, cx: &mut Context<Self>) {
        if !self.has_persisted_github_token() {
            self.notice = Some("no saved GitHub session to disconnect".to_owned());
            cx.notify();
            return;
        }

        self.github_auth_state = github_auth_store::GithubAuthState::default();
        self.notice = match self.persist_github_auth_state() {
            Ok(()) => Some("disconnected from GitHub".to_owned()),
            Err(error) => Some(format!(
                "disconnected, but failed to persist auth state: {error}"
            )),
        };
        self.refresh_worktree_pull_requests(cx);
        cx.notify();
    }

    fn run_github_auth_button_action(&mut self, cx: &mut Context<Self>) {
        if self.github_auth_in_progress {
            return;
        }

        if self.has_persisted_github_token() {
            self.clear_saved_github_token(cx);
            return;
        }

        self.start_github_oauth_sign_in(cx);
    }

    fn start_github_oauth_sign_in(&mut self, cx: &mut Context<Self>) {
        if self.github_auth_in_progress {
            return;
        }

        let Some(client_id) = github_oauth_client_id() else {
            self.notice = Some(
                "GitHub OAuth client ID is not configured. Set ARBOR_GITHUB_OAUTH_CLIENT_ID."
                    .to_owned(),
            );
            cx.notify();
            return;
        };

        self.github_auth_modal = None;
        self.github_auth_copy_feedback_active = false;
        self.github_auth_in_progress = true;
        self.notice = Some("starting GitHub device authorization".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let client_id_for_start = client_id.clone();
            let device_code_result = cx
                .background_spawn(async move { github_request_device_code(&client_id_for_start) })
                .await;

            let device_code = match device_code_result {
                Ok(device_code) => device_code,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.github_auth_in_progress = false;
                        this.github_auth_modal = None;
                        this.github_auth_copy_feedback_active = false;
                        this.notice = Some(error);
                        cx.notify();
                    });
                    return;
                },
            };

            let verification_url = device_code
                .verification_uri_complete
                .clone()
                .unwrap_or_else(|| device_code.verification_uri.clone());
            let user_code = device_code.user_code.clone();

            if this
                .update(cx, |this, cx| {
                    this.github_auth_modal = Some(GitHubAuthModal {
                        user_code: user_code.clone(),
                        verification_url: verification_url.clone(),
                    });
                    this.github_auth_copy_feedback_active = false;
                    this.open_external_url(&verification_url, cx);
                    this.notice = Some("complete GitHub auth in browser".to_owned());
                    cx.notify();
                })
                .is_err()
            {
                return;
            }

            let poll_result = cx
                .background_spawn(async move {
                    github_poll_device_access_token(&client_id, &device_code)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.github_auth_in_progress = false;
                this.github_auth_modal = None;
                this.github_auth_copy_feedback_active = false;
                match poll_result {
                    Ok(token) => {
                        this.github_auth_state = github_auth_store::GithubAuthState {
                            access_token: Some(token.access_token),
                            token_type: token.token_type,
                            scope: token.scope,
                            user_login: None,
                            user_avatar_url: None,
                        };

                        this.notice = match this.persist_github_auth_state() {
                            Ok(()) => Some(
                                "GitHub connected, pull request numbers will refresh automatically"
                                    .to_owned(),
                            ),
                            Err(error) => Some(format!(
                                "GitHub connected, but failed to persist auth state: {error}"
                            )),
                        };
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

    fn close_top_bar_worktree_quick_actions(&mut self) {
        self.top_bar_quick_actions_open = false;
        self.top_bar_quick_actions_submenu = None;
    }

    fn refresh_top_bar_external_launchers(&mut self) {
        self.ide_launchers = detect_ide_launchers();
        self.terminal_launchers = detect_terminal_launchers();
    }

    fn toggle_top_bar_worktree_quick_actions_menu(&mut self, cx: &mut Context<Self>) {
        if self.selected_local_worktree_path().is_none() {
            self.notice = Some("select a local worktree first".to_owned());
            self.close_top_bar_worktree_quick_actions();
            cx.notify();
            return;
        }

        if self.top_bar_quick_actions_open {
            self.close_top_bar_worktree_quick_actions();
        } else {
            self.top_bar_quick_actions_open = true;
            self.top_bar_quick_actions_submenu = None;
            self.refresh_top_bar_external_launchers();
        }
        cx.notify();
    }

    fn toggle_top_bar_worktree_quick_actions_submenu(
        &mut self,
        submenu: QuickActionSubmenu,
        cx: &mut Context<Self>,
    ) {
        if !self.top_bar_quick_actions_open {
            return;
        }

        self.top_bar_quick_actions_submenu = if self.top_bar_quick_actions_submenu == Some(submenu)
        {
            None
        } else {
            Some(submenu)
        };
        cx.notify();
    }

    fn run_worktree_quick_action(&mut self, action: WorktreeQuickAction, cx: &mut Context<Self>) {
        let Some(worktree_path) = self.selected_local_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a local worktree first".to_owned());
            self.close_top_bar_worktree_quick_actions();
            cx.notify();
            return;
        };

        let result = match action {
            WorktreeQuickAction::OpenFinder => open_worktree_in_file_manager(&worktree_path),
            WorktreeQuickAction::CopyPath => {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    worktree_path.display().to_string(),
                ));
                Ok("copied worktree path to clipboard".to_owned())
            },
        };

        self.close_top_bar_worktree_quick_actions();
        self.notice = Some(match result {
            Ok(message) => message,
            Err(error) => error,
        });
        cx.notify();
    }

    fn run_worktree_external_launcher(
        &mut self,
        submenu: QuickActionSubmenu,
        launcher_index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree_path) = self.selected_local_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a local worktree first".to_owned());
            self.close_top_bar_worktree_quick_actions();
            cx.notify();
            return;
        };

        let launcher = match submenu {
            QuickActionSubmenu::Ide => self.ide_launchers.get(launcher_index).copied(),
            QuickActionSubmenu::Terminal => self.terminal_launchers.get(launcher_index).copied(),
        };
        let Some(launcher) = launcher else {
            self.notice = Some("launcher no longer available".to_owned());
            self.close_top_bar_worktree_quick_actions();
            cx.notify();
            return;
        };

        let result = open_worktree_with_external_launcher(&worktree_path, launcher);
        self.close_top_bar_worktree_quick_actions();
        self.notice = Some(match result {
            Ok(message) => message,
            Err(error) => error,
        });
        cx.notify();
    }

    fn select_worktree(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.repository_context_menu = None;
        self.worktree_context_menu = None;
        self._hover_show_task = None;
        self.worktree_hover_popover = None;
        self.active_remote_worktree = None;
        if let Some(worktree) = self.worktrees.get(index) {
            tracing::info!(worktree = %worktree.path.display(), branch = %worktree.branch, "switching worktree");
        }
        if let Some(old) = self.active_worktree_index
            && old != index
        {
            self.worktree_nav_back.push(old);
            self.worktree_nav_forward.clear();
        }
        self.close_top_bar_worktree_quick_actions();
        if self.active_worktree_index != Some(index) {
            self.worktree_selection_epoch = self.worktree_selection_epoch.wrapping_add(1);
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

    fn show_worktree_hover_popover(
        &mut self,
        index: usize,
        mouse_y: Pixels,
        cx: &mut Context<Self>,
    ) {
        self._hover_show_task = None;
        self._hover_dismiss_task = None;
        let checks_expanded = self
            .worktree_hover_popover
            .as_ref()
            .filter(|popover| popover.worktree_index == index)
            .is_some_and(|popover| popover.checks_expanded);
        self.worktree_hover_popover = Some(WorktreeHoverPopover {
            worktree_index: index,
            mouse_y,
            checks_expanded,
        });
        cx.notify();
    }

    fn cancel_worktree_hover_popover_show(&mut self) {
        self._hover_show_task = None;
    }

    fn cancel_worktree_hover_popover_dismiss(&mut self) {
        self._hover_dismiss_task = None;
    }

    fn update_worktree_hover_mouse_position(&mut self, position: gpui::Point<Pixels>) {
        self.last_mouse_position = position;
        if self.worktree_hover_safe_zone_contains_mouse() {
            self.cancel_worktree_hover_popover_dismiss();
        }
    }

    fn worktree_hover_safe_zone_contains_mouse(&self) -> bool {
        let Some(popover) = self.worktree_hover_popover.as_ref() else {
            return false;
        };
        let Some(worktree) = self.worktrees.get(popover.worktree_index) else {
            return false;
        };
        worktree_hover_safe_zone_contains(
            self.left_pane_width,
            popover,
            worktree,
            self.last_mouse_position,
        )
    }

    fn schedule_worktree_hover_popover_dismiss(
        &mut self,
        worktree_index: usize,
        cx: &mut Context<Self>,
    ) {
        self.cancel_worktree_hover_popover_show();
        self._hover_dismiss_task = Some(cx.spawn(async move |this, cx| {
            cx.background_spawn(async {
                smol::Timer::after(WORKTREE_HOVER_POPOVER_HIDE_DELAY).await;
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .worktree_hover_popover
                    .as_ref()
                    .is_some_and(|popover| popover.worktree_index == worktree_index)
                    && !this.worktree_hover_safe_zone_contains_mouse()
                {
                    this.worktree_hover_popover = None;
                    cx.notify();
                }
            });
        }));
    }

    fn schedule_worktree_hover_popover_show(
        &mut self,
        worktree_index: usize,
        mouse_y: Pixels,
        cx: &mut Context<Self>,
    ) {
        // Never show hover popover while a context menu is open.
        if self.worktree_context_menu.is_some() {
            return;
        }

        self.cancel_worktree_hover_popover_dismiss();

        if self
            .worktree_hover_popover
            .as_ref()
            .is_some_and(|popover| popover.worktree_index == worktree_index)
        {
            return;
        }

        // Show immediately — no delay. This avoids timing issues where the
        // dismiss timer of the previous cell races with the show timer of the
        // new cell, causing the tooltip to not appear.
        self.show_worktree_hover_popover(worktree_index, mouse_y, cx);
    }

    fn select_outpost(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        self.repository_context_menu = None;
        self.worktree_context_menu = None;
        self._hover_show_task = None;
        self.worktree_hover_popover = None;
        self.close_top_bar_worktree_quick_actions();
        self.active_outpost_index = Some(index);
        self.active_worktree_index = None;
        self.changed_files.clear();
        self.selected_changed_file = None;
        self.refresh_remote_changed_files(cx);
        cx.notify();
    }

    fn open_remote_create_modal(
        &mut self,
        daemon_url: String,
        hostname: String,
        repo_root: String,
        cx: &mut Context<Self>,
    ) {
        tracing::info!(
            url = %daemon_url,
            host = %hostname,
            repo = %repo_root,
            "opening create modal on remote daemon"
        );
        connection_history::record_connection(&daemon_url, Some(&hostname));
        self.connection_history = connection_history::load_history();
        let connected = self.connect_to_daemon_endpoint(&daemon_url, Some(hostname), None, cx);
        if connected {
            // Find the repo in the now-refreshed list by root path
            if let Some(repo_index) = self
                .repositories
                .iter()
                .position(|r| r.root.to_string_lossy().ends_with(&repo_root))
            {
                self.select_repository(repo_index, cx);
                self.open_create_modal(repo_index, CreateModalTab::LocalWorktree, cx);
            } else if let Some(repo_index) = self.repositories.first().map(|_| 0) {
                self.select_repository(repo_index, cx);
                self.open_create_modal(repo_index, CreateModalTab::LocalWorktree, cx);
            }
        }
    }

    fn select_remote_worktree(
        &mut self,
        daemon_index: usize,
        worktree_path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.remote_daemon_states.get(&daemon_index) else {
            return;
        };
        let client = Arc::clone(&state.client);
        let hostname = state.hostname.clone();

        tracing::info!(
            host = %hostname,
            path = %worktree_path,
            "selecting remote worktree (keeping local daemon)"
        );

        // Deselect local worktree, activate remote
        let cwd = PathBuf::from(&worktree_path);
        self.active_worktree_index = None;
        self.active_outpost_index = None;
        self.active_remote_worktree = Some(ActiveRemoteWorktree {
            daemon_index,
            worktree_path: cwd.clone(),
        });
        let has_terminal = self
            .terminals
            .iter()
            .any(|session| session.worktree_path == cwd);
        if has_terminal {
            // Already have a terminal for this remote worktree, just activate it
            if let Some(session_id) = self.active_terminal_id_for_worktree(&cwd) {
                self.active_terminal_by_worktree.insert(cwd, session_id);
            }
        } else {
            // Spawn a new terminal session on the remote daemon
            let session_id = self.next_terminal_id;
            self.next_terminal_id += 1;
            self.active_terminal_by_worktree
                .insert(cwd.clone(), session_id);

            let shell = self.embedded_shell();

            let mut session = TerminalSession {
                id: session_id,
                daemon_session_id: SessionId::new(session_id.to_string()),
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
                modes: TerminalModes::default(),
                last_runtime_sync_at: None,
                runtime: None,
            };

            match client.create_or_attach(CreateOrAttachRequest {
                session_id: SessionId::default(),
                workspace_id: cwd.display().to_string().into(),
                cwd: cwd.clone(),
                shell,
                cols: 120,
                rows: 35,
                title: Some(session.title.clone()),
                command: None,
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
                    session.runtime = Some(local_daemon_runtime(
                        client,
                        daemon_session.session_id.to_string(),
                        Some(self.terminal_poll_tx.clone()),
                    ));
                },
                Err(error) => {
                    tracing::warn!(%error, "failed to create remote terminal session");
                    self.notice = Some(format!("failed to create terminal on {hostname}: {error}"));
                },
            }

            self.terminals.push(session);
        }

        self.terminal_scroll_handle.scroll_to_bottom();
        window.focus(&self.terminal_focus);
        cx.notify();
    }

    fn toggle_discovered_daemon(&mut self, index: usize, cx: &mut Context<Self>) {
        // If already expanded, collapse it
        if let Some(state) = self.remote_daemon_states.get(&index) {
            if state.expanded {
                if let Some(s) = self.remote_daemon_states.get_mut(&index) {
                    s.expanded = false;
                }
                cx.notify();
                return;
            }
            // If collapsed but already fetched, just re-expand
            if !state.repositories.is_empty() || !state.worktrees.is_empty() {
                if let Some(s) = self.remote_daemon_states.get_mut(&index) {
                    s.expanded = true;
                }
                cx.notify();
                return;
            }
        }

        let Some(daemon) = self.discovered_daemons.get(index) else {
            return;
        };
        let url = daemon.base_url();
        let hostname = daemon.display_name().to_owned();

        // Create HTTP client for the remote daemon
        let client = match terminal_daemon_http::HttpTerminalDaemon::new(&url) {
            Ok(c) => Arc::new(c),
            Err(err) => {
                tracing::error!(%err, %url, "failed to create HTTP client for LAN daemon");
                return;
            },
        };

        // Apply stored auth token if we have one
        if let Some(token) = self.daemon_auth_tokens.get(&url) {
            client.set_auth_token(Some(token.clone()));
        }

        tracing::info!(%url, name = %hostname, "fetching repos/worktrees from LAN daemon");

        self.remote_daemon_states.insert(index, RemoteDaemonState {
            client: Arc::clone(&client),
            hostname: hostname.clone(),
            repositories: Vec::new(),
            worktrees: Vec::new(),
            loading: true,
            expanded: true,
            error: None,
        });
        cx.notify();

        // Fetch repos and worktrees in background
        let client_clone = Arc::clone(&client);
        let url_clone = url.clone();
        cx.spawn(async move |this, cx| {
            let (repos, worktrees, error, needs_auth) = {
                let repos = client_clone.list_repositories();
                let worktrees = client_clone.list_worktrees();
                match (repos, worktrees) {
                    (Ok(r), Ok(w)) => (r, w, None, false),
                    (Err(e), _) | (_, Err(e)) => {
                        let needs_auth = e.is_unauthorized();
                        tracing::warn!(%e, needs_auth, "failed to fetch from LAN daemon");
                        (Vec::new(), Vec::new(), Some(format!("{e}")), needs_auth)
                    },
                }
            };

            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    if let Some(state) = this.remote_daemon_states.get_mut(&index) {
                        state.repositories = repos;
                        state.worktrees = worktrees;
                        state.loading = false;
                        state.error = error;
                    }
                    if needs_auth {
                        this.daemon_auth_modal = Some(DaemonAuthModal {
                            daemon_url: url_clone,
                            token: String::new(),
                            token_cursor: 0,
                            error: None,
                        });
                        // Track which daemon index needs auth so we can retry
                        this.pending_remote_daemon_auth = Some(index);
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn connect_to_ssh_daemon(
        &mut self,
        target: SshDaemonTarget,
        label: Option<String>,
        auth_key: String,
        cx: &mut Context<Self>,
    ) {
        self.stop_active_ssh_daemon_tunnel();

        let tunnel = match SshDaemonTunnel::start(&target) {
            Ok(tunnel) => tunnel,
            Err(error) => {
                self.notice = Some(error);
                self.terminal_daemon = None;
                self.connected_daemon_label = None;
                cx.notify();
                return;
            },
        };

        let local_url = tunnel.local_url();
        let local_port = tunnel.local_port;
        tracing::info!(
            remote = %target.ssh_destination(),
            ssh_port = target.ssh_port,
            daemon_port = target.daemon_port,
            local_url = %local_url,
            "connecting to daemon through ssh tunnel"
        );

        self.ssh_daemon_tunnel = Some(tunnel);
        self.notice = Some(format!(
            "connecting to {} via SSH tunnel\u{2026}",
            target.ssh_destination()
        ));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let ready = cx
                .background_spawn(async move {
                    for _ in 0..40 {
                        if std::net::TcpStream::connect(("127.0.0.1", local_port)).is_ok() {
                            return true;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    false
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.notice = None;
                if ready {
                    let connected =
                        this.connect_to_daemon_endpoint(&local_url, label, Some(auth_key), cx);
                    if !connected {
                        this.stop_active_ssh_daemon_tunnel();
                    }
                } else {
                    tracing::warn!(
                        local_port = local_port,
                        "SSH tunnel did not become ready in time"
                    );
                    this.notice =
                        Some("SSH tunnel timed out — is the remote daemon running?".to_owned());
                    this.stop_active_ssh_daemon_tunnel();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn connect_to_daemon_endpoint(
        &mut self,
        url: &str,
        label: Option<String>,
        auth_key: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        tracing::info!(url = %url, "connecting to daemon");
        self.daemon_base_url = url.to_owned();
        let token_key = auth_key.unwrap_or_else(|| url.to_owned());
        let client = match terminal_daemon_http::default_terminal_daemon_client(url) {
            Ok(c) => c,
            Err(error) => {
                tracing::warn!(url = %url, %error, "failed to create daemon client");
                self.notice = Some(format!("failed to connect to {url}: {error}"));
                self.terminal_daemon = None;
                self.connected_daemon_label = None;
                cx.notify();
                return false;
            },
        };

        if let Some(token) = self.daemon_auth_tokens.get(&token_key) {
            client.set_auth_token(Some(token.clone()));
        }

        match client.list_sessions() {
            Ok(records) => {
                self.terminal_daemon = Some(client);
                self.connected_daemon_label = label;
                self.restore_terminal_sessions_from_records(records, true);
                self.refresh_worktrees(cx);
                cx.notify();
                true
            },
            Err(error) => {
                if error.is_forbidden() {
                    tracing::warn!(url = %url, "daemon rejected connection: forbidden (no auth token configured on remote)");
                    self.notice = Some(
                        "Remote host has no auth token configured. Set [daemon] auth_token in ~/.config/arbor/config.toml on the remote host.".to_owned(),
                    );
                    self.terminal_daemon = None;
                    self.connected_daemon_label = None;
                    cx.notify();
                    false
                } else if error.is_unauthorized() {
                    tracing::info!(url = %url, "daemon requires authentication, showing auth modal");
                    self.daemon_auth_modal = Some(DaemonAuthModal {
                        daemon_url: token_key,
                        token: String::new(),
                        token_cursor: 0,
                        error: None,
                    });
                    self.terminal_daemon = Some(client);
                    self.connected_daemon_label = label;
                    cx.notify();
                    true
                } else {
                    tracing::warn!(url = %url, %error, "failed to connect to daemon");
                    self.notice = Some(format!("failed to connect to {url}: {error}"));
                    self.terminal_daemon = None;
                    self.connected_daemon_label = None;
                    cx.notify();
                    false
                }
            },
        }
    }

    fn try_start_and_connect_daemon(&mut self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { try_auto_start_daemon(&daemon_base_url) })
                .await;

            let _ = this.update(cx, |this, cx| {
                if let Some(client) = result {
                    let records = client.list_sessions().unwrap_or_default();
                    this.terminal_daemon = Some(client);
                    this.restore_terminal_sessions_from_records(records, true);
                    this.refresh_worktrees(cx);
                } else {
                    this.notice =
                        Some("Failed to start daemon. Is arbor-httpd available?".to_owned());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_active_ssh_daemon_tunnel(&mut self) {
        let _ = self.ssh_daemon_tunnel.take();
    }

    fn submit_daemon_auth(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.daemon_auth_modal.take() else {
            return;
        };
        let token = modal.token.trim().to_owned();
        if token.is_empty() {
            self.daemon_auth_modal = Some(DaemonAuthModal {
                token_cursor: 0,
                error: Some("Token cannot be empty".to_owned()),
                ..modal
            });
            cx.notify();
            return;
        }
        let url = modal.daemon_url.clone();

        // Handle remote daemon auth (inline expand)
        if let Some(daemon_index) = self.pending_remote_daemon_auth.take() {
            self.daemon_auth_tokens.insert(url.clone(), token.clone());
            connection_history::save_tokens(&self.daemon_auth_tokens);
            // Set token on existing client and retry, or re-toggle to fetch
            if let Some(state) = self.remote_daemon_states.get(&daemon_index) {
                state.client.set_auth_token(Some(token));
            }
            // Clear the state so toggle_discovered_daemon will re-fetch
            self.remote_daemon_states.remove(&daemon_index);
            self.toggle_discovered_daemon(daemon_index, cx);
            return;
        }

        // Handle local daemon auth
        if let Some(client) = self.terminal_daemon.as_ref() {
            client.set_auth_token(Some(token.clone()));
        }
        // Verify the token works
        if let Some(client) = self.terminal_daemon.as_ref() {
            match client.list_sessions() {
                Ok(records) => {
                    self.daemon_auth_tokens.insert(url, token);
                    connection_history::save_tokens(&self.daemon_auth_tokens);
                    self.restore_terminal_sessions_from_records(records, true);
                    self.refresh_worktrees(cx);
                },
                Err(error) => {
                    if error.is_unauthorized() || error.is_forbidden() {
                        self.daemon_auth_modal = Some(DaemonAuthModal {
                            daemon_url: modal.daemon_url,
                            token_cursor: char_count(&modal.token),
                            token: modal.token,
                            error: Some("Invalid token".to_owned()),
                        });
                    } else {
                        self.notice = Some(format!("connection failed: {error}"));
                    }
                },
            }
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
        let github_token = self.github_access_token();
        let github_service = self.github_service.clone();

        self.git_action_in_flight = Some(GitActionKind::CreatePullRequest);
        self.notice = Some("creating pull request".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                run_create_pr_for_worktree(
                    github_service.as_ref(),
                    worktree_path.as_path(),
                    repo_slug.as_deref(),
                    github_token.as_deref(),
                )
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
        self.selected_file_tree_entry = Some(path.clone());

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_image = matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "svg" | "tiff" | "tif"
        );

        if !is_image
            && let Ok(editor) = env::var("EDITOR")
            && let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf)
        {
            let full_path = worktree_path.join(&path);
            if is_gui_editor(&editor) {
                if let Err(error) = create_command(&editor)
                    .arg(&full_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    self.notice = Some(format!("Failed to open $EDITOR ({editor}): {error}"));
                }
            } else {
                self.open_editor_in_terminal(&editor, &full_path, cx);
            }
            cx.notify();
            return;
        }

        self.open_file_view_tab(path, cx);
        cx.notify();
    }

    fn set_right_pane_tab(&mut self, tab: RightPaneTab, cx: &mut Context<Self>) {
        if self.right_pane_tab == tab {
            return;
        }
        self.right_pane_tab = tab;
        self.right_pane_search.clear();
        self.right_pane_search_cursor = 0;
        self.right_pane_search_active = false;
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
        if let Err(error) = self
            .app_config_store
            .save_scalar_settings(&[("theme", Some(theme_kind.slug()))])
        {
            self.notice = Some(format!("failed to save theme setting: {error}"));
        } else {
            self.config_last_modified = None;
        }
        if !self.show_theme_picker {
            self.theme_toast = Some(format!("Theme switched to {}", theme_kind.label()));
        }
        self.theme_toast_generation = self.theme_toast_generation.saturating_add(1);
        let generation = self.theme_toast_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                std::thread::sleep(THEME_TOAST_DURATION);
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if this.theme_toast_generation == generation {
                    this.theme_toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
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
            repository_path_cursor: char_count(&repository_path),
            repository_path,
            worktree_name: String::new(),
            worktree_name_cursor: 0,
            checkout_kind: self.preferred_checkout_kind,
            worktree_active_field: CreateWorktreeField::WorktreeName,
            host_index: 0,
            host_dropdown_open: false,
            clone_url_cursor: char_count(&clone_url),
            clone_url,
            outpost_name: String::new(),
            outpost_name_cursor: 0,
            outpost_active_field: CreateOutpostField::CloneUrl,
            is_creating: false,
            creating_status: None,
            error: None,
        });
        cx.notify();
    }

    fn set_create_modal_checkout_kind(
        &mut self,
        checkout_kind: CheckoutKind,
        cx: &mut Context<Self>,
    ) {
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };
        if modal.is_creating || modal.checkout_kind == checkout_kind {
            return;
        }

        modal.checkout_kind = checkout_kind;
        modal.error = None;
        self.preferred_checkout_kind = checkout_kind;
        cx.notify();
    }

    fn close_create_modal(&mut self, cx: &mut Context<Self>) {
        self.create_modal = None;
        cx.notify();
    }

    fn open_delete_modal(
        &mut self,
        target: DeleteTarget,
        label: String,
        branch: String,
        cx: &mut Context<Self>,
    ) {
        let worktree_index = match &target {
            DeleteTarget::Worktree(i) => Some(*i),
            _ => None,
        };
        self.delete_modal = Some(DeleteModal {
            target,
            label,
            branch: worktree::short_branch(&branch),
            has_unpushed: if worktree_index.is_some() {
                None
            } else {
                Some(false)
            },
            delete_branch: false,
            is_deleting: false,
            error: None,
        });
        cx.notify();

        // For worktrees, spawn async check for unpushed commits.
        if let Some(worktree_index) = worktree_index
            && let Some(wt) = self.worktrees.get(worktree_index)
        {
            let wt_path = wt.path.clone();
            cx.spawn(async move |this, cx| {
                let has_unpushed = cx
                    .background_spawn(async move { worktree::has_unpushed_commits(&wt_path) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(modal) = this.delete_modal.as_mut() {
                        modal.has_unpushed = Some(has_unpushed);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    fn close_delete_modal(&mut self, cx: &mut Context<Self>) {
        self.delete_modal = None;
        cx.notify();
    }

    fn execute_delete(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.delete_modal.as_ref() else {
            return;
        };
        if modal.is_deleting {
            return;
        }

        match modal.target.clone() {
            DeleteTarget::Worktree(index) => {
                let Some(wt) = self.worktrees.get(index) else {
                    self.close_delete_modal(cx);
                    return;
                };
                let is_discrete_clone = wt.checkout_kind == CheckoutKind::DiscreteClone;
                let repo_root = wt.repo_root.clone();
                let wt_path = wt.path.clone();
                let branch = modal.branch.clone();
                let delete_branch = modal.delete_branch && !is_discrete_clone;

                if let Some(modal) = self.delete_modal.as_mut() {
                    modal.is_deleting = true;
                    modal.error = None;
                    cx.notify();
                }

                cx.spawn(async move |this, cx| {
                    let result = if is_discrete_clone {
                        cx.background_spawn({
                            let wt_path = wt_path.clone();
                            async move {
                                fs::remove_dir_all(&wt_path).map_err(|error| {
                                    format!(
                                        "failed to remove discrete clone `{}`: {error}",
                                        wt_path.display()
                                    )
                                })
                            }
                        })
                        .await
                    } else {
                        cx.background_spawn({
                            let repo_root = repo_root.clone();
                            let wt_path = wt_path.clone();
                            async move {
                                worktree::remove(&repo_root, &wt_path, true)
                                    .map_err(|error| error.to_string())
                            }
                        })
                        .await
                    };

                    if let Err(e) = &result {
                        let err_msg = e.to_string();
                        let _ = this.update(cx, |this, cx| {
                            if let Some(modal) = this.delete_modal.as_mut() {
                                modal.is_deleting = false;
                                modal.error = Some(err_msg);
                                cx.notify();
                            }
                        });
                        return;
                    }

                    if delete_branch && !branch.is_empty() {
                        let _ = cx
                            .background_spawn(async move {
                                worktree::delete_branch(&repo_root, &branch)
                            })
                            .await;
                    }

                    let _ = this.update(cx, |this, cx| {
                        if is_discrete_clone {
                            this.remove_repository_checkout_root(&wt_path);
                            this.persist_repositories(cx);
                        }
                        this.delete_modal = None;
                        this.refresh_worktrees(cx);
                        cx.notify();
                    });
                })
                .detach();
            },
            DeleteTarget::Outpost(index) => {
                let Some(outpost) = self.outposts.get(index) else {
                    self.close_delete_modal(cx);
                    return;
                };
                let outpost_id = outpost.outpost_id.clone();

                if let Err(e) = self.outpost_store.remove(&outpost_id) {
                    if let Some(modal) = self.delete_modal.as_mut() {
                        modal.error = Some(e.to_string());
                        cx.notify();
                    }
                    return;
                }

                self.outposts.remove(index);
                if self.active_outpost_index == Some(index) {
                    self.active_outpost_index = None;
                } else if let Some(active) = self.active_outpost_index
                    && active > index
                {
                    self.active_outpost_index = Some(active - 1);
                }
                self.delete_modal = None;
                cx.notify();
            },
            DeleteTarget::Repository(index) => {
                if index >= self.repositories.len() {
                    self.close_delete_modal(cx);
                    return;
                }

                let mut repositories = self.repositories.clone();
                repositories.remove(index);
                self.set_repositories_preserving_state(repositories);

                self.delete_modal = None;
                self.persist_repositories(cx);
                self.refresh_worktrees(cx);
                cx.notify();
            },
        }
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
                match field {
                    CreateWorktreeField::RepositoryPath => {
                        modal.repository_path_cursor = char_count(&modal.repository_path);
                    },
                    CreateWorktreeField::WorktreeName => {
                        modal.worktree_name_cursor = char_count(&modal.worktree_name);
                    },
                }
            },
            ModalInputEvent::MoveActiveField => {
                modal.worktree_active_field = match modal.worktree_active_field {
                    CreateWorktreeField::RepositoryPath => CreateWorktreeField::WorktreeName,
                    CreateWorktreeField::WorktreeName => CreateWorktreeField::RepositoryPath,
                };
            },
            ModalInputEvent::Edit(action) => match modal.worktree_active_field {
                CreateWorktreeField::RepositoryPath => {
                    apply_text_edit_action(
                        &mut modal.repository_path,
                        &mut modal.repository_path_cursor,
                        &action,
                    );
                },
                CreateWorktreeField::WorktreeName => {
                    apply_text_edit_action(
                        &mut modal.worktree_name,
                        &mut modal.worktree_name_cursor,
                        &action,
                    );
                },
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
        let checkout_kind = modal.checkout_kind;

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
                    create_managed_worktree(repository_input, worktree_input, checkout_kind)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match creation {
                    Ok(created) => {
                        if created.checkout_kind == CheckoutKind::DiscreteClone {
                            let group_key = this
                                .repositories
                                .iter()
                                .find(|repository| {
                                    repository.contains_checkout_root(&created.source_repo_root)
                                })
                                .map(|repository| repository.group_key.clone())
                                .unwrap_or_else(|| {
                                    repository_store::default_group_key_for_root(
                                        &created.source_repo_root,
                                    )
                                });
                            this.upsert_repository_checkout_root(
                                created.worktree_path.clone(),
                                CheckoutKind::DiscreteClone,
                                group_key,
                            );
                            this.persist_repositories(cx);
                        }

                        this.notice = Some(format!(
                            "created {} `{}` on branch `{}`",
                            created.checkout_kind.label().to_ascii_lowercase(),
                            created.worktree_name,
                            created.branch_name
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
                        tracing::error!("worktree creation failed: {error}");
                        if let Some(modal) = this.create_modal.as_mut() {
                            modal.is_creating = false;
                            modal.creating_status = None;
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
            name_cursor: 0,
            hostname: String::new(),
            hostname_cursor: 0,
            user: String::new(),
            user_cursor: 0,
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
                match field {
                    ManageHostsField::Name => modal.name_cursor = char_count(&modal.name),
                    ManageHostsField::Hostname => {
                        modal.hostname_cursor = char_count(&modal.hostname);
                    },
                    ManageHostsField::User => modal.user_cursor = char_count(&modal.user),
                }
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
            HostsModalInputEvent::Edit(action) => match modal.active_field {
                ManageHostsField::Name => {
                    apply_text_edit_action(&mut modal.name, &mut modal.name_cursor, &action);
                },
                ManageHostsField::Hostname => {
                    apply_text_edit_action(
                        &mut modal.hostname,
                        &mut modal.hostname_cursor,
                        &action,
                    );
                },
                ManageHostsField::User => {
                    apply_text_edit_action(&mut modal.user, &mut modal.user_cursor, &action);
                },
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

        if let Err(error) = self.app_config_store.append_remote_host(&host_config) {
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
            modal.name_cursor = 0;
            modal.hostname.clear();
            modal.hostname_cursor = 0;
            modal.user.clear();
            modal.user_cursor = 0;
            modal.error = None;
        }
        cx.notify();
    }

    fn remove_host_at(&mut self, host_name: String, cx: &mut Context<Self>) {
        if let Err(error) = self.app_config_store.remove_remote_host(&host_name) {
            self.notice = Some(error);
            cx.notify();
            return;
        }
        self.config_last_modified = None;
        self.refresh_config_if_changed(cx);
        self.notice = Some(format!("Host \"{host_name}\" removed."));
        cx.notify();
    }

    fn preset_command_for_kind(&self, kind: AgentPresetKind) -> String {
        self.agent_presets
            .iter()
            .find(|preset| preset.kind == kind)
            .map(|preset| preset.command.clone())
            .unwrap_or_else(|| kind.default_command().to_owned())
    }

    fn set_preset_command_for_kind(&mut self, kind: AgentPresetKind, command: String) {
        if let Some(preset) = self
            .agent_presets
            .iter_mut()
            .find(|preset| preset.kind == kind)
        {
            preset.command = command;
            return;
        }

        self.agent_presets.push(AgentPreset { kind, command });
        self.agent_presets.sort_by_key(|preset| {
            AgentPresetKind::ORDER
                .iter()
                .position(|kind| *kind == preset.kind)
                .unwrap_or(usize::MAX)
        });
    }

    fn save_agent_presets(&self) -> Result<(), String> {
        let presets = self
            .agent_presets
            .iter()
            .map(|preset| app_config::AgentPresetConfig {
                key: preset.kind.key().to_owned(),
                command: preset.command.clone(),
            })
            .collect::<Vec<_>>();
        self.app_config_store.save_agent_presets(&presets)
    }

    fn open_manage_presets_modal(&mut self, cx: &mut Context<Self>) {
        let active_preset = self.active_preset_tab.unwrap_or(AgentPresetKind::Codex);
        let command = self.preset_command_for_kind(active_preset);
        self.manage_presets_modal = Some(ManagePresetsModal {
            active_preset,
            command_cursor: char_count(&command),
            command,
            error: None,
        });
        cx.notify();
    }

    fn close_manage_presets_modal(&mut self, cx: &mut Context<Self>) {
        self.manage_presets_modal = None;
        cx.notify();
    }

    fn update_manage_presets_modal_input(
        &mut self,
        input: PresetsModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(mut modal) = self.manage_presets_modal.clone() else {
            return;
        };

        match input {
            PresetsModalInputEvent::SetActivePreset(kind) => {
                modal.active_preset = kind;
                modal.command = self.preset_command_for_kind(kind);
                modal.command_cursor = char_count(&modal.command);
            },
            PresetsModalInputEvent::CycleActivePreset(reverse) => {
                modal.active_preset = modal.active_preset.cycle(reverse);
                modal.command = self.preset_command_for_kind(modal.active_preset);
                modal.command_cursor = char_count(&modal.command);
            },
            PresetsModalInputEvent::Edit(action) => {
                apply_text_edit_action(&mut modal.command, &mut modal.command_cursor, &action);
            },
            PresetsModalInputEvent::RestoreDefault => {
                modal.command = modal.active_preset.default_command().to_owned();
                modal.command_cursor = char_count(&modal.command);
            },
            PresetsModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        self.manage_presets_modal = Some(modal);
        cx.notify();
    }

    fn submit_manage_presets_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.manage_presets_modal.clone() else {
            return;
        };

        let command = modal.command.trim().to_owned();
        if command.is_empty() {
            if let Some(modal_state) = self.manage_presets_modal.as_mut() {
                modal_state.error = Some("Command is required.".to_owned());
            }
            cx.notify();
            return;
        }

        self.set_preset_command_for_kind(modal.active_preset, command);
        if let Err(error) = self.save_agent_presets() {
            if let Some(modal_state) = self.manage_presets_modal.as_mut() {
                modal_state.error = Some(error);
            }
            cx.notify();
            return;
        }

        self.config_last_modified = None;
        self.refresh_config_if_changed(cx);
        self.manage_presets_modal = None;
        self.notice = Some(format!("{} preset updated", modal.active_preset.label(),));
        cx.notify();
    }

    fn launch_agent_preset(
        &mut self,
        preset: AgentPresetKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = self.preset_command_for_kind(preset).trim().to_owned();
        self.active_preset_tab = Some(preset);
        if command.is_empty() {
            self.notice = Some(format!("{} preset command is empty", preset.label()));
            cx.notify();
            return;
        }

        let terminal_count_before = self.terminals.len();
        self.spawn_terminal_session(window, cx);
        if self.terminals.len() <= terminal_count_before {
            return;
        }

        let Some(session_id) = self.terminals.last().map(|session| session.id) else {
            return;
        };

        let input = format!("{command}\n");
        if let Err(error) = self.write_input_to_terminal(session_id, input.as_bytes()) {
            self.notice = Some(format!("failed to run {} preset: {error}", preset.label()));
            cx.notify();
            return;
        }

        if let Some(session) = self
            .terminals
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.last_command = Some(command);
            session.pending_command.clear();
            session.updated_at_unix_ms = current_unix_timestamp_millis();
        }

        self.sync_daemon_session_store(cx);
        cx.notify();
    }

    fn launch_repo_preset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = self.repo_presets.get(index) else {
            return;
        };
        let command = preset.command.trim().to_owned();
        let name = preset.name.clone();
        if command.is_empty() {
            self.notice = Some(format!("{name} preset command is empty"));
            cx.notify();
            return;
        }

        let terminal_count_before = self.terminals.len();
        self.spawn_terminal_session(window, cx);
        if self.terminals.len() <= terminal_count_before {
            return;
        }

        let Some(session_id) = self.terminals.last().map(|session| session.id) else {
            return;
        };

        let input = format!("{command}\n");
        if let Err(error) = self.write_input_to_terminal(session_id, input.as_bytes()) {
            self.notice = Some(format!("failed to run {name} preset: {error}"));
            cx.notify();
            return;
        }

        if let Some(session) = self
            .terminals
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.last_command = Some(command);
            session.pending_command.clear();
            session.updated_at_unix_ms = current_unix_timestamp_millis();
        }

        self.sync_daemon_session_store(cx);
        cx.notify();
    }

    fn open_manage_repo_presets_modal(
        &mut self,
        editing_index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let (icon, name, command) = if let Some(index) = editing_index {
            if let Some(preset) = self.repo_presets.get(index) {
                (
                    preset.icon.clone(),
                    preset.name.clone(),
                    preset.command.clone(),
                )
            } else {
                return;
            }
        } else {
            (String::new(), String::new(), String::new())
        };

        self.manage_repo_presets_modal = Some(ManageRepoPresetsModal {
            editing_index,
            icon_cursor: char_count(&icon),
            icon,
            name_cursor: char_count(&name),
            name,
            command_cursor: char_count(&command),
            command,
            active_tab: RepoPresetModalTab::Edit,
            active_field: RepoPresetModalField::Icon,
            error: None,
        });
        cx.notify();
    }

    fn close_manage_repo_presets_modal(&mut self, cx: &mut Context<Self>) {
        self.manage_repo_presets_modal = None;
        cx.notify();
    }

    fn update_manage_repo_presets_modal_input(
        &mut self,
        input: RepoPresetsModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(mut modal) = self.manage_repo_presets_modal.clone() else {
            return;
        };

        match input {
            RepoPresetsModalInputEvent::SetActiveTab(tab) => {
                modal.active_tab = tab;
            },
            RepoPresetsModalInputEvent::SetActiveField(field) => {
                if modal.active_tab != RepoPresetModalTab::Edit {
                    self.manage_repo_presets_modal = Some(modal);
                    cx.notify();
                    return;
                }
                modal.active_field = field;
                match field {
                    RepoPresetModalField::Icon => modal.icon_cursor = char_count(&modal.icon),
                    RepoPresetModalField::Name => modal.name_cursor = char_count(&modal.name),
                    RepoPresetModalField::Command => {
                        modal.command_cursor = char_count(&modal.command);
                    },
                }
            },
            RepoPresetsModalInputEvent::MoveActiveField(reverse) => {
                if modal.active_tab != RepoPresetModalTab::Edit {
                    self.manage_repo_presets_modal = Some(modal);
                    cx.notify();
                    return;
                }
                modal.active_field = if reverse {
                    modal.active_field.prev()
                } else {
                    modal.active_field.next()
                };
            },
            RepoPresetsModalInputEvent::Edit(action) => {
                if modal.active_tab != RepoPresetModalTab::Edit {
                    self.manage_repo_presets_modal = Some(modal);
                    cx.notify();
                    return;
                }
                match modal.active_field {
                    RepoPresetModalField::Icon => {
                        apply_text_edit_action(&mut modal.icon, &mut modal.icon_cursor, &action);
                    },
                    RepoPresetModalField::Name => {
                        apply_text_edit_action(&mut modal.name, &mut modal.name_cursor, &action);
                    },
                    RepoPresetModalField::Command => {
                        apply_text_edit_action(
                            &mut modal.command,
                            &mut modal.command_cursor,
                            &action,
                        );
                    },
                }
            },
            RepoPresetsModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        self.manage_repo_presets_modal = Some(modal);
        cx.notify();
    }

    fn submit_manage_repo_presets_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.manage_repo_presets_modal.clone() else {
            return;
        };

        let name = modal.name.trim().to_owned();
        let command = modal.command.trim().to_owned();
        let icon = modal.icon.trim().to_owned();

        if name.is_empty() {
            if let Some(m) = self.manage_repo_presets_modal.as_mut() {
                m.error = Some("Name is required.".to_owned());
            }
            cx.notify();
            return;
        }
        if command.is_empty() {
            if let Some(m) = self.manage_repo_presets_modal.as_mut() {
                m.error = Some("Command is required.".to_owned());
            }
            cx.notify();
            return;
        }

        let new_preset = RepoPreset {
            name: name.clone(),
            icon,
            command,
        };

        if let Some(index) = modal.editing_index {
            if let Some(preset) = self.repo_presets.get_mut(index) {
                *preset = new_preset;
            }
        } else {
            self.repo_presets.push(new_preset);
        }

        if let Err(error) = self.save_repo_presets() {
            if let Some(m) = self.manage_repo_presets_modal.as_mut() {
                m.error = Some(error);
            }
            cx.notify();
            return;
        }

        self.manage_repo_presets_modal = None;
        let action = if modal.editing_index.is_some() {
            "updated"
        } else {
            "added"
        };
        self.notice = Some(format!("Preset \"{name}\" {action}."));
        cx.notify();
    }

    fn delete_repo_preset(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.manage_repo_presets_modal.as_ref() else {
            return;
        };
        let Some(index) = modal.editing_index else {
            return;
        };
        let Some(preset) = self.repo_presets.get(index) else {
            return;
        };
        let name = preset.name.clone();
        let save_dir = self.active_arbor_toml_dir();

        if let Err(error) = self.app_config_store.remove_repo_preset(&save_dir, &name) {
            if let Some(m) = self.manage_repo_presets_modal.as_mut() {
                m.error = Some(error);
            }
            cx.notify();
            return;
        }

        self.repo_presets.remove(index);
        self.manage_repo_presets_modal = None;
        self.notice = Some(format!("Preset \"{name}\" removed."));
        cx.notify();
    }

    fn save_repo_presets(&self) -> Result<(), String> {
        let save_dir = self.active_arbor_toml_dir();
        let presets: Vec<app_config::RepoPresetConfig> = self
            .repo_presets
            .iter()
            .map(|p| app_config::RepoPresetConfig {
                name: p.name.clone(),
                icon: p.icon.clone(),
                command: p.command.clone(),
            })
            .collect();
        self.app_config_store.save_repo_presets(&save_dir, &presets)
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
                modal.host_dropdown_open = false;
                modal.outpost_active_field = field;
                match field {
                    CreateOutpostField::HostSelector => {},
                    CreateOutpostField::CloneUrl => {
                        modal.clone_url_cursor = char_count(&modal.clone_url);
                    },
                    CreateOutpostField::OutpostName => {
                        modal.outpost_name_cursor = char_count(&modal.outpost_name);
                    },
                }
            },
            OutpostModalInputEvent::MoveActiveField(reverse) => {
                modal.host_dropdown_open = false;
                modal.outpost_active_field = match (modal.outpost_active_field, reverse) {
                    (CreateOutpostField::HostSelector, false) => CreateOutpostField::CloneUrl,
                    (CreateOutpostField::CloneUrl, false) => CreateOutpostField::OutpostName,
                    (CreateOutpostField::OutpostName, false) => CreateOutpostField::HostSelector,
                    (CreateOutpostField::HostSelector, true) => CreateOutpostField::OutpostName,
                    (CreateOutpostField::CloneUrl, true) => CreateOutpostField::HostSelector,
                    (CreateOutpostField::OutpostName, true) => CreateOutpostField::CloneUrl,
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
            OutpostModalInputEvent::SelectHost(index) => {
                if index < self.remote_hosts.len() {
                    modal.host_index = index;
                }
                modal.host_dropdown_open = false;
            },
            OutpostModalInputEvent::ToggleHostDropdown => {
                modal.host_dropdown_open = !modal.host_dropdown_open;
                modal.outpost_active_field = CreateOutpostField::HostSelector;
            },
            OutpostModalInputEvent::Edit(action) => {
                if modal.outpost_active_field == CreateOutpostField::HostSelector {
                    return;
                }
                match modal.outpost_active_field {
                    CreateOutpostField::HostSelector => return,
                    CreateOutpostField::CloneUrl => {
                        apply_text_edit_action(
                            &mut modal.clone_url,
                            &mut modal.clone_url_cursor,
                            &action,
                        );
                    },
                    CreateOutpostField::OutpostName => {
                        apply_text_edit_action(
                            &mut modal.outpost_name,
                            &mut modal.outpost_name_cursor,
                            &action,
                        );
                    },
                }
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
        let outpost_name = modal.outpost_name.trim().to_owned();
        let host_index = modal.host_index;

        if clone_url.is_empty() {
            modal.error = Some("Clone URL is required.".to_owned());
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

        let branch = derive_branch_name(&outpost_name);

        modal.is_creating = true;
        modal.creating_status = Some("Connecting over SSH…".to_owned());
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

        // Channel carries either a progress status (Left) or the final result (Right).
        enum ProvisionMsg {
            Progress(String),
            Done(Result<arbor_core::remote::ProvisionResult, String>),
        }

        let (msg_tx, msg_rx) = smol::channel::unbounded::<ProvisionMsg>();

        cx.spawn(async move |this, cx| {
            // Spawn the provisioning work on a background thread.
            cx.background_spawn(async move {
                let result = (|| {
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
                    provisioner
                        .provision_with_progress(
                            &bg_clone_url,
                            &bg_outpost_name,
                            &bg_branch,
                            |status| {
                                let _ =
                                    msg_tx.send_blocking(ProvisionMsg::Progress(status.to_owned()));
                            },
                        )
                        .map_err(|e| format!("{e}"))
                })();
                let _ = msg_tx.send_blocking(ProvisionMsg::Done(result));
            })
            .detach();

            // Read messages until we get the final result.
            let mut result = Err("provisioning task was cancelled".to_owned());
            while let Ok(msg) = msg_rx.recv().await {
                match msg {
                    ProvisionMsg::Progress(status) => {
                        let _ = this.update(cx, |this, cx| {
                            if let Some(modal) = this.create_modal.as_mut() {
                                modal.creating_status = Some(status);
                            }
                            cx.notify();
                        });
                    },
                    ProvisionMsg::Done(r) => {
                        result = r;
                        break;
                    },
                }
            }

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
                        // Select the newly created outpost in the sidebar.
                        let new_index = this
                            .outposts
                            .iter()
                            .position(|o| o.label == outpost_name && o.host_name == host_name);
                        if let Some(idx) = new_index {
                            this.active_outpost_index = Some(idx);
                        }
                        this.create_modal = None;
                    },
                    Err(error) => {
                        tracing::error!("outpost creation failed: {error}");
                        if let Some(modal) = this.create_modal.as_mut() {
                            modal.is_creating = false;
                            modal.creating_status = None;
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.is_held {
            return;
        }

        if self.welcome_clone_url_active {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.welcome_clone_url_active = false;
                    cx.notify();
                    cx.stop_propagation();
                    return;
                },
                "enter" | "return" => {
                    self.submit_welcome_clone(cx);
                    cx.stop_propagation();
                    return;
                },
                _ => {},
            }
            if let Some(action) = text_edit_action_for_event(event, cx) {
                apply_text_edit_action(
                    &mut self.welcome_clone_url,
                    &mut self.welcome_clone_url_cursor,
                    &action,
                );
                self.welcome_clone_error = None;
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }

        if self.right_pane_search_active {
            if event.keystroke.key.as_str() == "escape" {
                self.right_pane_search.clear();
                self.right_pane_search_cursor = 0;
                self.right_pane_search_active = false;
                cx.notify();
                cx.stop_propagation();
                return;
            }
            if let Some(action) = text_edit_action_for_event(event, cx) {
                apply_text_edit_action(
                    &mut self.right_pane_search,
                    &mut self.right_pane_search_cursor,
                    &action,
                );
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }

        if self.quit_overlay_until.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.quit_overlay_until = None;
                    cx.notify();
                    cx.stop_propagation();
                },
                "enter" | "return" => {
                    self.action_confirm_quit(window, cx);
                    cx.stop_propagation();
                },
                _ => {},
            }
            return;
        }

        if self.show_theme_picker {
            if event.keystroke.key.as_str() == "escape" {
                self.show_theme_picker = false;
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }

        if self.settings_modal.is_some() {
            let active_control = self
                .settings_modal
                .as_ref()
                .map(|modal| modal.active_control)
                .unwrap_or(SettingsControl::DaemonBindMode);
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.close_settings_modal(cx);
                    cx.stop_propagation();
                    return;
                },
                "tab" => {
                    self.update_settings_modal_input(
                        SettingsModalInputEvent::CycleControl(event.keystroke.modifiers.shift),
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                },
                "left" if active_control == SettingsControl::DaemonBindMode => {
                    self.update_settings_modal_input(
                        SettingsModalInputEvent::SelectDaemonBindMode(DaemonBindMode::Localhost),
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                },
                "right" if active_control == SettingsControl::DaemonBindMode => {
                    self.update_settings_modal_input(
                        SettingsModalInputEvent::SelectDaemonBindMode(
                            DaemonBindMode::AllInterfaces,
                        ),
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                },
                "space" => {
                    self.update_settings_modal_input(
                        SettingsModalInputEvent::ToggleActiveControl,
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                },
                "enter" | "return" => {
                    self.submit_settings_modal(cx);
                    cx.stop_propagation();
                    return;
                },
                _ => {},
            }
            return;
        }

        if self.github_auth_modal.is_some() {
            if event.keystroke.modifiers.platform {
                return;
            }

            if event.keystroke.key.as_str() == "escape" {
                self.close_github_auth_modal(cx);
                cx.stop_propagation();
            }
            return;
        }

        if self.delete_modal.is_some() {
            if event.keystroke.modifiers.platform {
                return;
            }
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.close_delete_modal(cx);
                    cx.stop_propagation();
                },
                "enter" | "return" => {
                    self.execute_delete(cx);
                    cx.stop_propagation();
                },
                "space" | " " => {
                    if let Some(modal) = self.delete_modal.as_mut()
                        && matches!(modal.target, DeleteTarget::Worktree(_))
                    {
                        modal.delete_branch = !modal.delete_branch;
                        cx.notify();
                    }
                    cx.stop_propagation();
                },
                _ => {},
            }
            return;
        }

        if self.start_daemon_modal {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.start_daemon_modal = false;
                    cx.notify();
                    cx.stop_propagation();
                },
                "enter" | "return" => {
                    self.start_daemon_modal = false;
                    self.try_start_and_connect_daemon(cx);
                    cx.stop_propagation();
                },
                _ => {},
            }
            return;
        }

        if self.daemon_auth_modal.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.daemon_auth_modal = None;
                    cx.notify();
                    cx.stop_propagation();
                },
                "enter" | "return" => {
                    self.submit_daemon_auth(cx);
                    cx.stop_propagation();
                },
                _ => {
                    if let Some(modal) = self.daemon_auth_modal.as_mut()
                        && let Some(action) = text_edit_action_for_event(event, cx)
                    {
                        apply_text_edit_action(&mut modal.token, &mut modal.token_cursor, &action);
                        modal.error = None;
                        cx.notify();
                        cx.stop_propagation();
                    }
                },
            }
            return;
        }

        if self.connect_to_host_modal.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.connect_to_host_modal = None;
                    cx.notify();
                    cx.stop_propagation();
                },
                "enter" | "return" => {
                    self.submit_connect_to_host(cx);
                    cx.stop_propagation();
                },
                _ => {
                    if let Some(modal) = self.connect_to_host_modal.as_mut()
                        && let Some(action) = text_edit_action_for_event(event, cx)
                    {
                        apply_text_edit_action(
                            &mut modal.address,
                            &mut modal.address_cursor,
                            &action,
                        );
                        modal.error = None;
                        cx.notify();
                        cx.stop_propagation();
                    }
                },
            }
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
                    _ => {},
                }

                if let Some(action) = text_edit_action_for_event(event, cx) {
                    self.update_manage_hosts_modal_input(HostsModalInputEvent::ClearError, cx);
                    self.update_manage_hosts_modal_input(HostsModalInputEvent::Edit(action), cx);
                    cx.stop_propagation();
                }
            } else if event.keystroke.key.as_str() == "escape" {
                self.close_manage_hosts_modal(cx);
                cx.stop_propagation();
            }
            return;
        }

        if self.manage_presets_modal.is_some() {
            if event.keystroke.modifiers.platform {
                return;
            }

            match event.keystroke.key.as_str() {
                "escape" => {
                    self.close_manage_presets_modal(cx);
                    cx.stop_propagation();
                    return;
                },
                "tab" => {
                    self.update_manage_presets_modal_input(
                        PresetsModalInputEvent::CycleActivePreset(event.keystroke.modifiers.shift),
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                },
                "enter" | "return" => {
                    self.submit_manage_presets_modal(cx);
                    cx.stop_propagation();
                    return;
                },
                _ => {},
            }
            if let Some(action) = text_edit_action_for_event(event, cx) {
                self.update_manage_presets_modal_input(PresetsModalInputEvent::ClearError, cx);
                self.update_manage_presets_modal_input(PresetsModalInputEvent::Edit(action), cx);
                cx.stop_propagation();
            }
            return;
        }

        if self.manage_repo_presets_modal.is_some() {
            let active_tab = self
                .manage_repo_presets_modal
                .as_ref()
                .map(|modal| modal.active_tab)
                .unwrap_or(RepoPresetModalTab::Edit);

            match event.keystroke.key.as_str() {
                "escape" => {
                    self.close_manage_repo_presets_modal(cx);
                    cx.stop_propagation();
                    return;
                },
                "tab" => {
                    if active_tab == RepoPresetModalTab::Edit {
                        self.update_manage_repo_presets_modal_input(
                            RepoPresetsModalInputEvent::MoveActiveField(
                                event.keystroke.modifiers.shift,
                            ),
                            cx,
                        );
                    }
                    cx.stop_propagation();
                    return;
                },
                "enter" | "return" => {
                    if active_tab == RepoPresetModalTab::Edit {
                        self.submit_manage_repo_presets_modal(cx);
                    }
                    cx.stop_propagation();
                    return;
                },
                _ => {},
            }
            if active_tab == RepoPresetModalTab::Edit
                && let Some(action) = text_edit_action_for_event(event, cx)
            {
                self.update_manage_repo_presets_modal_input(
                    RepoPresetsModalInputEvent::ClearError,
                    cx,
                );
                self.update_manage_repo_presets_modal_input(
                    RepoPresetsModalInputEvent::Edit(action),
                    cx,
                );
                cx.stop_propagation();
            }
            return;
        }

        let Some(modal) = self.create_modal.as_ref() else {
            return;
        };

        let active_tab = modal.tab;

        match event.keystroke.key.as_str() {
            "escape" => {
                if self
                    .create_modal
                    .as_ref()
                    .is_some_and(|m| m.host_dropdown_open)
                {
                    if let Some(modal) = self.create_modal.as_mut() {
                        modal.host_dropdown_open = false;
                    }
                    cx.notify();
                } else {
                    self.close_create_modal(cx);
                }
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
                if active_tab == CreateModalTab::RemoteOutpost
                    && self
                        .create_modal
                        .as_ref()
                        .is_some_and(|m| m.outpost_active_field == CreateOutpostField::HostSelector)
                {
                    self.update_create_outpost_modal_input(
                        OutpostModalInputEvent::ToggleHostDropdown,
                        cx,
                    );
                } else {
                    match active_tab {
                        CreateModalTab::LocalWorktree => self.submit_create_worktree_modal(cx),
                        CreateModalTab::RemoteOutpost => self.submit_create_outpost_modal(cx),
                    }
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
        if let Some(action) = text_edit_action_for_event(event, cx) {
            match active_tab {
                CreateModalTab::LocalWorktree => {
                    self.update_create_worktree_modal_input(ModalInputEvent::ClearError, cx);
                    self.update_create_worktree_modal_input(ModalInputEvent::Edit(action), cx);
                },
                CreateModalTab::RemoteOutpost => {
                    self.update_create_outpost_modal_input(OutpostModalInputEvent::ClearError, cx);
                    self.update_create_outpost_modal_input(
                        OutpostModalInputEvent::Edit(action),
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

    fn action_open_manage_presets(
        &mut self,
        _: &OpenManagePresets,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_manage_presets_modal(cx);
    }

    fn action_open_manage_repo_presets(
        &mut self,
        _: &OpenManageRepoPresets,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_manage_repo_presets_modal(None, cx);
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
        self.quit_overlay_until = if self.quit_overlay_until.is_some() {
            None
        } else {
            Some(Instant::now())
        };
        cx.notify();
    }

    fn action_confirm_quit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.sync_daemon_session_store(cx);
        self.stop_active_ssh_daemon_tunnel();
        cx.quit();
    }

    fn action_dismiss_quit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.quit_overlay_until = None;
        cx.notify();
    }

    fn action_immediate_quit(&mut self, _: &ImmediateQuit, _: &mut Window, cx: &mut Context<Self>) {
        self.sync_daemon_session_store(cx);
        self.stop_active_ssh_daemon_tunnel();
        cx.quit();
    }

    fn action_view_logs(&mut self, _: &ViewLogs, _: &mut Window, cx: &mut Context<Self>) {
        self.logs_tab_open = true;
        self.logs_tab_active = true;
        self.active_diff_session_id = None;
        cx.notify();
    }

    fn action_show_about(&mut self, _: &ShowAbout, _: &mut Window, cx: &mut Context<Self>) {
        self.show_about = true;
        cx.notify();
    }

    fn action_open_theme_picker(
        &mut self,
        _: &OpenThemePicker,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_theme_picker = true;
        cx.notify();
    }

    fn action_open_settings(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_modal(cx);
    }

    fn action_open_manage_hosts(
        &mut self,
        _: &OpenManageHosts,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_manage_hosts_modal(cx);
    }

    fn action_connect_to_lan_daemon(
        &mut self,
        action: &ConnectToLanDaemon,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_discovered_daemon(action.index, cx);
    }

    fn action_connect_to_host(
        &mut self,
        _: &ConnectToHost,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connect_to_host_modal = Some(ConnectToHostModal {
            address: String::new(),
            address_cursor: 0,
            error: None,
        });
        cx.notify();
    }

    fn submit_connect_to_host(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.connect_to_host_modal.take() else {
            return;
        };
        let addr = modal.address.trim().to_owned();
        if addr.is_empty() {
            self.connect_to_host_modal = Some(ConnectToHostModal {
                address_cursor: char_count(&modal.address),
                error: Some("Address cannot be empty".to_owned()),
                ..modal
            });
            cx.notify();
            return;
        }

        let target = match parse_connect_host_target(&addr) {
            Ok(target) => target,
            Err(error) => {
                let address = modal.address;
                self.connect_to_host_modal = Some(ConnectToHostModal {
                    address_cursor: char_count(&address),
                    address,
                    error: Some(error),
                });
                cx.notify();
                return;
            },
        };
        let label = addr.clone();
        connection_history::record_connection(&addr, None);
        self.connection_history = connection_history::load_history();
        match target {
            ConnectHostTarget::Http { url, auth_key } => {
                self.stop_active_ssh_daemon_tunnel();
                let _ = self.connect_to_daemon_endpoint(&url, Some(label), Some(auth_key), cx);
            },
            ConnectHostTarget::Ssh { target, auth_key } => {
                self.connect_to_ssh_daemon(target, Some(label), auth_key, cx);
            },
        }
    }

    fn open_settings_modal(&mut self, cx: &mut Context<Self>) {
        let loaded = self.app_config_store.load_or_create_config();
        let daemon_auth_token = loaded
            .config
            .daemon
            .as_ref()
            .and_then(|daemon| daemon.auth_token.clone())
            .unwrap_or_default();
        let daemon_bind_mode = DaemonBindMode::from_config(
            loaded
                .config
                .daemon
                .as_ref()
                .and_then(|daemon| daemon.bind.as_deref()),
        );
        self.settings_modal = Some(SettingsModal {
            active_control: SettingsControl::DaemonBindMode,
            daemon_bind_mode,
            initial_daemon_bind_mode: daemon_bind_mode,
            notifications: self.notifications_enabled,
            daemon_auth_token,
            error: None,
        });
        cx.notify();
    }

    fn close_settings_modal(&mut self, cx: &mut Context<Self>) {
        self.settings_modal = None;
        cx.notify();
    }

    fn update_settings_modal_input(
        &mut self,
        input: SettingsModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(mut modal) = self.settings_modal.clone() else {
            return;
        };

        modal.error = None;
        match input {
            SettingsModalInputEvent::CycleControl(reverse) => {
                modal.active_control = modal.active_control.cycle(reverse);
            },
            SettingsModalInputEvent::SelectDaemonBindMode(bind_mode) => {
                modal.active_control = SettingsControl::DaemonBindMode;
                modal.daemon_bind_mode = bind_mode;
            },
            SettingsModalInputEvent::ToggleActiveControl => match modal.active_control {
                SettingsControl::DaemonBindMode => {
                    modal.daemon_bind_mode = match modal.daemon_bind_mode {
                        DaemonBindMode::Localhost => DaemonBindMode::AllInterfaces,
                        DaemonBindMode::AllInterfaces => DaemonBindMode::Localhost,
                    };
                },
                SettingsControl::Notifications => {
                    modal.notifications = !modal.notifications;
                },
            },
            SettingsModalInputEvent::ToggleNotifications => {
                modal.active_control = SettingsControl::Notifications;
                modal.notifications = !modal.notifications;
            },
        }

        self.settings_modal = Some(modal);
        cx.notify();
    }

    fn submit_settings_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.settings_modal.clone() else {
            return;
        };

        let notifications_str = if modal.notifications {
            "true"
        } else {
            "false"
        };
        let theme_slug = self.theme_kind.slug();
        let daemon_bind_changed = modal.daemon_bind_mode != modal.initial_daemon_bind_mode;

        if let Err(error) = self.app_config_store.save_scalar_settings(&[
            ("notifications", Some(notifications_str)),
            ("theme", Some(theme_slug)),
        ]) {
            if let Some(modal_state) = self.settings_modal.as_mut() {
                modal_state.error = Some(error);
            }
            cx.notify();
            return;
        }

        if let Err(error) = self
            .app_config_store
            .save_daemon_bind_mode(Some(modal.daemon_bind_mode.as_config_value()))
        {
            if let Some(modal_state) = self.settings_modal.as_mut() {
                modal_state.error = Some(error);
            }
            cx.notify();
            return;
        }

        self.config_last_modified = None;
        self.refresh_config_if_changed(cx);
        self.settings_modal = None;
        if daemon_bind_changed && daemon_url_is_local(&self.daemon_base_url) {
            let allow_remote = modal.daemon_bind_mode == DaemonBindMode::AllInterfaces;
            if let Some(daemon) = &self.terminal_daemon {
                match daemon.set_bind_mode(allow_remote) {
                    Ok(()) => {
                        let mode = if allow_remote {
                            "all interfaces"
                        } else {
                            "localhost only"
                        };
                        self.notice =
                            Some(format!("Settings saved. Daemon now listening on {mode}."));
                    },
                    Err(error) => {
                        tracing::warn!(%error, "failed to update daemon bind mode, restarting");
                        self.restart_local_daemon_after_settings_save(cx);
                        return;
                    },
                }
            } else {
                self.notice = Some("Settings saved".to_owned());
            }
        } else {
            self.notice = Some("Settings saved".to_owned());
        }
        cx.notify();
    }

    fn restart_local_daemon_after_settings_save(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.terminal_daemon.clone() else {
            self.notice = Some("Settings saved".to_owned());
            cx.notify();
            return;
        };
        let daemon_base_url = self.daemon_base_url.clone();
        self.notice = Some("Settings saved. Restarting daemon…".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let _ = daemon.shutdown();
                    std::thread::sleep(Duration::from_millis(500));
                    try_auto_start_daemon(&daemon_base_url)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if let Some(client) = result {
                    let records = client.list_sessions().unwrap_or_default();
                    this.terminal_daemon = Some(client);
                    this.restore_terminal_sessions_from_records(records, true);
                    this.refresh_worktrees(cx);
                    this.notice = Some("Settings saved".to_owned());
                } else {
                    this.notice = Some(
                        "Settings saved, but Arbor could not restart the daemon automatically."
                            .to_owned(),
                    );
                }
                cx.notify();
            });
        })
        .detach();
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
            daemon_session_id: SessionId::new(session_id.to_string()),
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
            modes: TerminalModes::default(),
            last_runtime_sync_at: None,
            runtime: None,
        };

        let mut launched_with_daemon = false;
        if backend_kind == TerminalBackendKind::Embedded
            && let Some(daemon) = self.terminal_daemon.as_ref()
        {
            let shell = self.embedded_shell();
            match daemon.create_or_attach(CreateOrAttachRequest {
                session_id: SessionId::default(),
                workspace_id: cwd.display().to_string().into(),
                cwd: cwd.clone(),
                shell,
                cols: 120,
                rows: 35,
                title: Some(session.title.clone()),
                command: None,
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
                    session.runtime = Some(local_daemon_runtime(
                        daemon.clone(),
                        daemon_session.session_id.to_string(),
                        Some(self.terminal_poll_tx.clone()),
                    ));
                    launched_with_daemon = true;
                },
                Err(error) => {
                    let error_text = error.to_string();
                    tracing::warn!(%error, "failed to create daemon terminal session, falling back to local");
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
            let (initial_rows, initial_cols) = self.last_terminal_grid_size.unwrap_or((0, 0));
            match terminal_backend::launch_backend(backend_kind, &cwd, initial_rows, initial_cols) {
                Ok(TerminalLaunch::Embedded(runtime)) => {
                    runtime.set_notify(self.terminal_poll_tx.clone());
                    session.command = "embedded shell".to_owned();
                    session.generation = runtime.generation();
                    session.runtime = Some(local_embedded_runtime(runtime));
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

    fn open_editor_in_terminal(&mut self, editor: &str, file_path: &Path, cx: &mut Context<Self>) {
        if !self.spawn_terminal_session_inner(true) {
            cx.notify();
            return;
        }

        // Find the session we just spawned, set its title, and send the editor command
        let session_id = self.next_terminal_id - 1;
        let editor_basename = Path::new(editor)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(editor);
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        if let Some(session) = self.terminals.iter_mut().find(|s| s.id == session_id) {
            session.title = format!("{editor_basename}: {file_name}");
        }
        let cmd = format!(
            "{} {}; exit\n",
            shell_escape(editor),
            shell_escape(&file_path.to_string_lossy()),
        );
        if let Err(error) = self.write_input_to_terminal(session_id, cmd.as_bytes()) {
            self.notice = Some(format!("Failed to send command to terminal: {error}"));
        }

        self.sync_daemon_session_store(cx);
        self.active_diff_session_id = None;
        self.active_file_view_session_id = None;
        self.file_view_editing = false;
        self.logs_tab_active = false;
        self.terminal_scroll_handle.scroll_to_bottom();
        self.focus_terminal_on_next_render = true;
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
        self.active_file_view_session_id = None;
        self.file_view_editing = false;
        self.logs_tab_active = false;
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
            daemon_session_id: SessionId::new(session_id.to_string()),
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
            modes: TerminalModes::default(),
            last_runtime_sync_at: None,
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
                            mosh.set_notify(self.terminal_poll_tx.clone());
                            session.command = "mosh".to_owned();
                            session.generation = mosh.generation();
                            session.runtime = Some(outpost_mosh_runtime(mosh));
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
                            session.runtime = Some(outpost_ssh_runtime(ssh_shell));
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
        self.active_file_view_session_id = None;
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
        self.active_file_view_session_id = None;
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

        let Some(runtime) = self.terminals[index].runtime.clone() else {
            return Ok(());
        };

        {
            let session = &self.terminals[index];
            runtime.write_input(session, input)?;
        }

        self.terminals[index].updated_at_unix_ms = current_unix_timestamp_millis();
        Ok(())
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

        track_terminal_command_keystroke(session, keystroke);
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
        if self.quit_overlay_until.is_some() {
            // The quit overlay is modal — suppress terminal input entirely so
            // the key event propagates up to handle_global_key_down which
            // handles Enter/Escape for the overlay.
            return;
        }

        if self.right_pane_search_active
            || self.create_modal.is_some()
            || self.settings_modal.is_some()
            || self.github_auth_modal.is_some()
            || self.delete_modal.is_some()
            || self.manage_hosts_modal.is_some()
            || self.manage_presets_modal.is_some()
            || self.manage_repo_presets_modal.is_some()
            || self.daemon_auth_modal.is_some()
            || self.start_daemon_modal
            || self.connect_to_host_modal.is_some()
            || self.show_theme_picker
        {
            return;
        }

        let active_tab = self.active_center_tab_for_selected_worktree();

        // Handle file view editing before terminal input
        if matches!(active_tab, Some(CenterTab::FileView(_))) {
            if self.handle_file_view_key_down(event, cx) {
                cx.stop_propagation();
            }
            return;
        }

        let Some(CenterTab::Terminal(active_terminal_id)) = active_tab else {
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

        let terminal_modes = self
            .terminals
            .iter()
            .find(|session| session.id == active_terminal_id)
            .map(|session| session.modes)
            .unwrap_or_default();

        let Some(input) =
            terminal_keys::terminal_bytes_from_keystroke(&event.keystroke, terminal_modes)
        else {
            // No bytes for this key — let the event propagate to the IME /
            // InputHandler so composed characters arrive via
            // `replace_text_in_range`.
            return;
        };

        self.track_terminal_command_input(active_terminal_id, &event.keystroke);
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
        self.right_pane_search_active = false;
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
    }

    fn handle_file_view_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        // Always handle Cmd+S for save, even when not in editing mode
        if event.keystroke.modifiers.platform && event.keystroke.key.as_str() == "s" {
            self.save_active_file_view(cx);
            return true;
        }
        if !self.file_view_editing {
            return false;
        }
        let Some(session_id) = self.active_file_view_session_id else {
            return false;
        };
        let Some(session) = self
            .file_view_sessions
            .iter_mut()
            .find(|s| s.id == session_id)
        else {
            return false;
        };
        let FileViewContent::Text {
            raw_lines, dirty, ..
        } = &mut session.content
        else {
            return false;
        };
        if raw_lines.is_empty() {
            return false;
        }

        let cursor = &mut session.cursor;

        // Skip platform combos (Cmd+S handled above)
        if event.keystroke.modifiers.platform {
            return false;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.file_view_editing = false;
                cx.notify();
                return true;
            },
            "backspace" => {
                if cursor.col > 0 {
                    let line = &mut raw_lines[cursor.line];
                    let byte_pos = char_to_byte_offset(line, cursor.col);
                    let prev_byte = char_to_byte_offset(line, cursor.col - 1);
                    line.replace_range(prev_byte..byte_pos, "");
                    cursor.col -= 1;
                } else if cursor.line > 0 {
                    let removed = raw_lines.remove(cursor.line);
                    cursor.line -= 1;
                    cursor.col = raw_lines[cursor.line].chars().count();
                    raw_lines[cursor.line].push_str(&removed);
                }
                *dirty = true;
                cx.notify();
                return true;
            },
            "delete" => {
                let line_char_count = raw_lines[cursor.line].chars().count();
                if cursor.col < line_char_count {
                    let line = &mut raw_lines[cursor.line];
                    let byte_pos = char_to_byte_offset(line, cursor.col);
                    let next_byte = char_to_byte_offset(line, cursor.col + 1);
                    line.replace_range(byte_pos..next_byte, "");
                } else if cursor.line + 1 < raw_lines.len() {
                    let next = raw_lines.remove(cursor.line + 1);
                    raw_lines[cursor.line].push_str(&next);
                }
                *dirty = true;
                cx.notify();
                return true;
            },
            "enter" | "return" => {
                let line = &raw_lines[cursor.line];
                let byte_pos = char_to_byte_offset(line, cursor.col);
                let rest = line[byte_pos..].to_owned();
                raw_lines[cursor.line].truncate(byte_pos);
                cursor.line += 1;
                cursor.col = 0;
                raw_lines.insert(cursor.line, rest);
                *dirty = true;
                cx.notify();
                return true;
            },
            "left" => {
                if cursor.col > 0 {
                    cursor.col -= 1;
                } else if cursor.line > 0 {
                    cursor.line -= 1;
                    cursor.col = raw_lines[cursor.line].chars().count();
                }
                cx.notify();
                return true;
            },
            "right" => {
                let line_len = raw_lines[cursor.line].chars().count();
                if cursor.col < line_len {
                    cursor.col += 1;
                } else if cursor.line + 1 < raw_lines.len() {
                    cursor.line += 1;
                    cursor.col = 0;
                }
                cx.notify();
                return true;
            },
            "up" => {
                if cursor.line > 0 {
                    cursor.line -= 1;
                    let line_len = raw_lines[cursor.line].chars().count();
                    cursor.col = cursor.col.min(line_len);
                }
                cx.notify();
                return true;
            },
            "down" => {
                if cursor.line + 1 < raw_lines.len() {
                    cursor.line += 1;
                    let line_len = raw_lines[cursor.line].chars().count();
                    cursor.col = cursor.col.min(line_len);
                }
                cx.notify();
                return true;
            },
            "tab" => {
                let line = &mut raw_lines[cursor.line];
                let byte_pos = char_to_byte_offset(line, cursor.col);
                line.insert_str(byte_pos, "    ");
                cursor.col += 4;
                *dirty = true;
                cx.notify();
                return true;
            },
            "home" => {
                cursor.col = 0;
                cx.notify();
                return true;
            },
            "end" => {
                cursor.col = raw_lines[cursor.line].chars().count();
                cx.notify();
                return true;
            },
            _ => {},
        }

        if event.keystroke.modifiers.control || event.keystroke.modifiers.alt {
            return false;
        }

        // Character input
        if let Some(key_char) = event.keystroke.key_char.as_ref() {
            let line = &mut raw_lines[cursor.line];
            let byte_pos = char_to_byte_offset(line, cursor.col);
            line.insert_str(byte_pos, key_char);
            cursor.col += key_char.chars().count();
            *dirty = true;
            cx.notify();
            return true;
        }

        false
    }

    fn save_active_file_view(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_file_view_session_id else {
            return;
        };
        let Some(session) = self.file_view_sessions.iter().find(|s| s.id == session_id) else {
            return;
        };
        let FileViewContent::Text {
            raw_lines, dirty, ..
        } = &session.content
        else {
            return;
        };
        if !dirty {
            return;
        }
        let content = raw_lines.join("\n");
        let full_path = session.worktree_path.join(&session.file_path);
        let ext = session
            .file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let raw_clone = raw_lines.clone();
        match fs::write(&full_path, &content) {
            Ok(()) => {
                let highlighted = highlight_lines_with_syntect(&raw_clone, &ext, 0xc8ccd4);
                if let Some(s) = self
                    .file_view_sessions
                    .iter_mut()
                    .find(|s| s.id == session_id)
                    && let FileViewContent::Text {
                        highlighted: h,
                        dirty: d,
                        ..
                    } = &mut s.content
                {
                    *h = Arc::from(highlighted);
                    *d = false;
                }
            },
            Err(error) => {
                self.notice = Some(format!("Failed to save: {error}"));
            },
        }
        cx.notify();
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
            preferred_checkout_kind: Some(self.preferred_checkout_kind),
            sidebar_order: self.sidebar_order.clone(),
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

    fn build_sidebar_order_for_group(
        &self,
        group_key: &str,
        repo_root: &Path,
    ) -> Vec<SidebarItemId> {
        let worktree_ids: Vec<SidebarItemId> = self
            .worktrees
            .iter()
            .filter(|w| w.group_key == group_key)
            .map(|w| SidebarItemId::Worktree(w.path.clone()))
            .collect();
        let outpost_ids: Vec<SidebarItemId> = self
            .outposts
            .iter()
            .filter(|o| o.repo_root == repo_root)
            .map(|o| SidebarItemId::Outpost(o.outpost_id.clone()))
            .collect();

        let all_current: HashSet<_> = worktree_ids.iter().chain(&outpost_ids).cloned().collect();

        if let Some(saved) = self.sidebar_order.get(group_key) {
            let mut ordered: Vec<SidebarItemId> = saved
                .iter()
                .filter(|id| all_current.contains(id))
                .cloned()
                .collect();
            let ordered_set: HashSet<_> = ordered.iter().cloned().collect();
            for id in worktree_ids.into_iter().chain(outpost_ids) {
                if !ordered_set.contains(&id) {
                    ordered.push(id);
                }
            }
            ordered
        } else {
            worktree_ids.into_iter().chain(outpost_ids).collect()
        }
    }

    fn handle_sidebar_item_drop(
        &mut self,
        source_id: &SidebarItemId,
        insert_before: usize,
        group_key: &str,
        repo_root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut items = self.build_sidebar_order_for_group(group_key, repo_root);

        let Some(source_pos) = items.iter().position(|id| id == source_id) else {
            return;
        };

        // Compute effective insertion index after removal
        let target_pos = if insert_before > source_pos {
            insert_before - 1
        } else {
            insert_before
        };

        if source_pos == target_pos {
            return;
        }

        let item = items.remove(source_pos);
        items.insert(target_pos, item);

        self.sidebar_order.insert(group_key.to_owned(), items);
        self.sync_ui_state_store(window);
        cx.notify();
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
        let worktree_quick_actions_enabled = self.selected_local_worktree_path().is_some();
        let worktree_quick_actions_open =
            worktree_quick_actions_enabled && self.top_bar_quick_actions_open;
        let github_saved_token = self.has_persisted_github_token();
        let github_env_token = github_access_token_from_env().is_some();
        let github_auth_busy = self.github_auth_in_progress;
        let github_auth_label = if github_auth_busy {
            "Authorizing"
        } else if github_saved_token {
            "Disconnect"
        } else if github_env_token {
            "Connected (env)"
        } else {
            "Sign in"
        };
        let github_auth_icon_color = if github_auth_busy {
            theme.accent
        } else if github_saved_token || github_env_token {
            0x68c38d
        } else {
            theme.text_muted
        };
        let github_auth_text_color = if github_auth_busy || github_saved_token || github_env_token {
            theme.text_primary
        } else {
            theme.text_muted
        };
        let github_avatar_url = self.github_auth_state.user_avatar_url.clone();

        div()
            .h(px(TITLEBAR_HEIGHT))
            .bg(rgb(theme.chrome_bg))
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .flex()
            .items_center()
            // Left group: sidebar toggle + back/forward navigation
            .child(
                div()
                    .absolute()
                    .left(px(TOP_BAR_LEFT_OFFSET))
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
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
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
                            .hover(|this| this.text_color(rgb(theme.accent)))
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
                            .hover(|this| this.text_color(rgb(theme.accent)))
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
            // Right group: GitHub auth, worktree quick actions, and report issue button
            .child(
                div()
                    .absolute()
                    .right(px(16.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child({
                        let daemon_connected = self.terminal_daemon.is_some();
                        let web_ui_url = self.daemon_base_url.clone();
                        top_bar_button(
                            theme,
                            "web-ui-link",
                            true,
                            if daemon_connected {
                                theme.text_muted
                            } else {
                                theme.text_disabled
                            },
                            theme.text_primary,
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(11.))
                                .child(top_bar_icon_element(
                                    TopBarIconKind::RemoteControl,
                                    if daemon_connected {
                                        TopBarIconTone::Connected
                                    } else {
                                        TopBarIconTone::Disabled
                                    },
                                    if daemon_connected {
                                        0x68c38d
                                    } else {
                                        theme.text_disabled
                                    },
                                    "\u{f0ac}",
                                ))
                                .child("Remote Control"),
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            if this.terminal_daemon.is_some() {
                                this.open_external_url(&web_ui_url, cx);
                            } else {
                                this.start_daemon_modal = true;
                                cx.notify();
                            }
                        }))
                    })
                    .child(
                        top_bar_button(
                            theme,
                            "github-auth",
                            !github_auth_busy,
                            github_auth_text_color,
                            theme.text_primary,
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(11.))
                                .child(match github_avatar_url {
                                    Some(url) => div()
                                        .size(px(12.))
                                        .rounded_full()
                                        .overflow_hidden()
                                        .child(img(url).size_full().rounded_full().with_fallback(
                                            move || {
                                                top_bar_icon_element(
                                                    TopBarIconKind::GitHub,
                                                    if github_auth_busy {
                                                        TopBarIconTone::Busy
                                                    } else if github_saved_token || github_env_token {
                                                        TopBarIconTone::Connected
                                                    } else {
                                                        TopBarIconTone::Muted
                                                    },
                                                    github_auth_icon_color,
                                                    "\u{f09b}",
                                                )
                                                .into_any_element()
                                            },
                                        ))
                                        .into_any_element(),
                                    None => top_bar_icon_element(
                                        TopBarIconKind::GitHub,
                                        if github_auth_busy {
                                            TopBarIconTone::Busy
                                        } else if github_saved_token || github_env_token {
                                            TopBarIconTone::Connected
                                        } else {
                                            TopBarIconTone::Muted
                                        },
                                        github_auth_icon_color,
                                        "\u{f09b}",
                                    )
                                    .into_any_element(),
                                })
                                .child(github_auth_label),
                        )
                        .when(!github_auth_busy, |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.run_github_auth_button_action(cx);
                            }))
                        }),
                    )
                    .child(
                        top_bar_button(
                            theme,
                            "worktree-quick-actions",
                            worktree_quick_actions_enabled,
                            if worktree_quick_actions_enabled {
                                theme.text_muted
                            } else {
                                theme.text_disabled
                            },
                            theme.text_primary,
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(top_bar_icon_element(
                                    TopBarIconKind::WorktreeActions,
                                    if worktree_quick_actions_enabled {
                                        TopBarIconTone::Muted
                                    } else {
                                        TopBarIconTone::Disabled
                                    },
                                    if worktree_quick_actions_enabled {
                                        theme.text_muted
                                    } else {
                                        theme.text_disabled
                                    },
                                    "\u{f0e7}",
                                ))
                                .child(div().text_size(px(11.)).child("Action"))
                                .child(
                                    div()
                                        .font_family(FONT_MONO)
                                        .text_size(px(9.))
                                        .child(if worktree_quick_actions_open {
                                            "\u{f077}"
                                        } else {
                                            "\u{f078}"
                                        }),
                                ),
                        )
                        .when(worktree_quick_actions_enabled, |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_top_bar_worktree_quick_actions_menu(cx);
                            }))
                        }),
                    )
                    .child(
                        top_bar_button(
                            theme,
                            "report-issue",
                            true,
                            theme.text_muted,
                            theme.text_primary,
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(11.))
                                .child(top_bar_icon_element(
                                    TopBarIconKind::ReportIssue,
                                    TopBarIconTone::Muted,
                                    theme.text_muted,
                                    "\u{f188}",
                                ))
                                .child("Report issue"),
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.close_top_bar_worktree_quick_actions();
                            cx.open_url("https://github.com/penso/arbor/issues/new");
                        })),
                    ),
            )
    }

    fn render_top_bar_worktree_quick_actions_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let menu_open =
            self.top_bar_quick_actions_open && self.selected_local_worktree_path().is_some();

        if !menu_open {
            return div();
        }

        let ide_has_launchers = !self.ide_launchers.is_empty();
        let terminal_has_launchers = !self.terminal_launchers.is_empty();
        let submenu = self.top_bar_quick_actions_submenu;
        let ide_row_active = submenu == Some(QuickActionSubmenu::Ide);
        let terminal_row_active = submenu == Some(QuickActionSubmenu::Terminal);

        let mut overlay = div()
            .absolute()
            .right(px(16.))
            .top(px(TITLEBAR_HEIGHT))
            .mt(px(4.))
            .child(
                div()
                    .w(px(192.))
                    .py(px(4.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.chrome_bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .id("quick-action-open-finder")
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_worktree_quick_action(WorktreeQuickAction::OpenFinder, cx);
                            }))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_color(rgb(0xe5c07b))
                                    .child("\u{f07b}"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Open in Finder"),
                            ),
                    )
                    .child(
                        div()
                            .id("quick-action-open-ide-submenu")
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .text_color(rgb(if ide_has_launchers {
                                theme.text_primary
                            } else {
                                theme.text_disabled
                            }))
                            .when(ide_has_launchers, |this| {
                                this.cursor_pointer()
                                    .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_top_bar_worktree_quick_actions_submenu(
                                            QuickActionSubmenu::Ide,
                                            cx,
                                        );
                                    }))
                            })
                            .when(ide_row_active, |this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .font_family(FONT_MONO)
                                            .text_size(px(12.))
                                            .text_color(rgb(0x39a0ed))
                                            .child("\u{f121}"),
                                    )
                                    .child(div().text_size(px(11.)).child("IDE")),
                            )
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.))
                                    .text_color(rgb(if ide_has_launchers {
                                        theme.text_muted
                                    } else {
                                        theme.text_disabled
                                    }))
                                    .child("\u{f054}"),
                            ),
                    )
                    .child(
                        div()
                            .id("quick-action-open-terminal-submenu")
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .text_color(rgb(if terminal_has_launchers {
                                theme.text_primary
                            } else {
                                theme.text_disabled
                            }))
                            .when(terminal_has_launchers, |this| {
                                this.cursor_pointer()
                                    .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_top_bar_worktree_quick_actions_submenu(
                                            QuickActionSubmenu::Terminal,
                                            cx,
                                        );
                                    }))
                            })
                            .when(terminal_row_active, |this| {
                                this.bg(rgb(theme.panel_active_bg))
                            })
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(terminal_quick_action_icon_element(0x68c38d, 12.0))
                                    .child(div().text_size(px(11.)).child("Terminal")),
                            )
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.))
                                    .text_color(rgb(if terminal_has_launchers {
                                        theme.text_muted
                                    } else {
                                        theme.text_disabled
                                    }))
                                    .child("\u{f054}"),
                            ),
                    )
                    .child(div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(rgb(theme.border)))
                    .child(
                        div()
                            .id("quick-action-copy-path")
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_worktree_quick_action(WorktreeQuickAction::CopyPath, cx);
                            }))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_color(rgb(theme.text_muted))
                                    .child("\u{f0c5}"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Copy path"),
                            ),
                    ),
            );

        if let Some(submenu) = submenu {
            let launchers: &[ExternalLauncher] = match submenu {
                QuickActionSubmenu::Ide => &self.ide_launchers,
                QuickActionSubmenu::Terminal => &self.terminal_launchers,
            };
            if launchers.is_empty() {
                return overlay;
            }
            let submenu_top = match submenu {
                QuickActionSubmenu::Ide => px(28.),
                QuickActionSubmenu::Terminal => px(52.),
            };

            overlay = overlay.child(
                div()
                    .id("quick-action-launcher-submenu")
                    .absolute()
                    .right(px(200.))
                    .top(submenu_top)
                    .w(px(220.))
                    .py(px(4.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.chrome_bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .children(launchers.iter().enumerate().map(|(index, launcher)| {
                        let launcher = *launcher;
                        div()
                            .id(ElementId::NamedInteger(
                                "quick-action-launcher-item".into(),
                                index as u64,
                            ))
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_worktree_external_launcher(submenu, index, cx);
                            }))
                            .child(
                                div()
                                    .w(px(20.))
                                    .flex_none()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_center()
                                    .text_color(rgb(launcher.icon_color))
                                    .child(launcher.icon),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme.text_primary))
                                    .child(launcher.label),
                            )
                    })),
            );
        }

        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.close_top_bar_worktree_quick_actions();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(overlay)
    }

    fn render_notice_toast(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(notice) = self.notice.clone() else {
            return div();
        };

        let theme = self.theme();
        let is_error = notice_looks_like_error(&notice);
        let background = if is_error {
            theme.notice_bg
        } else {
            theme.chrome_bg
        };
        let text_color = if is_error {
            theme.notice_text
        } else {
            theme.text_primary
        };
        let border_color = if is_error {
            0xb95d5d
        } else {
            theme.accent
        };
        let icon = if is_error {
            "\u{f06a}"
        } else {
            "\u{f05a}"
        };
        let icon_color = if is_error {
            theme.notice_text
        } else {
            theme.accent
        };

        div()
            .absolute()
            .right(px(16.))
            .bottom(px(36.))
            .w(px(420.))
            .max_w(px(420.))
            .rounded_sm()
            .border_1()
            .border_color(rgb(border_color))
            .bg(rgb(background))
            .px_2()
            .py(px(8.))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(rgb(icon_color))
                            .child(icon),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(rgb(text_color))
                            .child(notice),
                    ),
            )
            .child(
                div()
                    .id("notice-toast-dismiss")
                    .cursor_pointer()
                    .font_family(FONT_MONO)
                    .text_size(px(11.))
                    .text_color(rgb(theme.text_muted))
                    .hover(|this| this.text_color(rgb(theme.text_primary)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    }))
                    .child("\u{f00d}"),
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
                let repository_github_url = repository
                    .github_repo_slug
                    .as_ref()
                    .map(|repo_slug| github_repo_url(repo_slug));
                let repo_worktrees: Vec<(usize, &WorktreeSummary)> = worktrees
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| w.group_key == repository.group_key)
                    .collect();

                // Add spacing between repo groups (not before the first)
                if repo_index > 0 {
                    pane = pane.child(div().h(px(4.)));
                }

                // Repo icon row: circular avatar or GitHub icon
                let repo_icon = match (repository.avatar_url.clone(), repository_github_url.clone())
                {
                    (Some(url), Some(github_url)) => div()
                        .id(("collapsed-repository-github-link", repo_index))
                        .size(px(32.))
                        .rounded_md()
                        .overflow_hidden()
                        .cursor_pointer()
                        .hover(|this| this.opacity(0.9))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_external_url(&github_url, cx);
                            cx.stop_propagation();
                        }))
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
                        .into_any_element(),
                    (Some(url), None) => div()
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
                        .into_any_element(),
                    (None, Some(github_url)) => div()
                        .id(("collapsed-repository-github-link", repo_index))
                        .size(px(24.))
                        .font_family(FONT_MONO)
                        .text_size(px(14.))
                        .text_color(rgb(theme.text_muted))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|this| this.opacity(0.9))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_external_url(&github_url, cx);
                            cx.stop_propagation();
                        }))
                        .child("\u{f09b}")
                        .into_any_element(),
                    (None, None) => div()
                        .size(px(24.))
                        .font_family(FONT_MONO)
                        .text_size(px(14.))
                        .text_color(rgb(theme.text_muted))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("\u{f09b}")
                        .into_any_element(),
                };
                pane = pane.child(repo_icon);

                let selection_epoch = self.worktree_selection_epoch;
                for (wt_index, worktree) in repo_worktrees {
                    let is_active = self.active_worktree_index == Some(wt_index);
                    let first_char: String = worktree
                        .branch
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .collect();

                    let cell = div()
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
                        }));
                    if is_active {
                        pane = pane.child(cell.with_animation(
                            ("collapsed-wt-select", selection_epoch),
                            Animation::new(Duration::from_millis(150)).with_easing(ease_in_out),
                            |el, delta| el.opacity(0.8 + 0.2 * delta),
                        ));
                    } else {
                        pane = pane.child(cell.opacity(0.8));
                    }
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
                            let is_collapsed =
                                self.collapsed_repositories.contains(&repository_index);
                            let repository_avatar_url = repository.avatar_url.clone();
                            let repository_github_url = repository
                                .github_repo_slug
                                .as_ref()
                                .map(|repo_slug| github_repo_url(repo_slug));
                            let repo_worktrees: Vec<(usize, WorktreeSummary)> = worktrees
                                .iter()
                                .cloned()
                                .enumerate()
                                .filter(|(_, worktree)| {
                                    worktree.group_key == repository.group_key
                                })
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
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .h(px(32.))
                                        .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_repository(repository_index, cx);
                                        }))
                                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.repository_context_menu = Some(RepositoryContextMenu {
                                                repository_index,
                                                position: event.position,
                                            });
                                            cx.notify();
                                        }))
                                        // GitHub icon or avatar outside the cell
                                        .child(
                                            match (
                                                repository_avatar_url.clone(),
                                                repository_github_url.clone(),
                                            ) {
                                                (Some(url), Some(github_url)) => div()
                                                    .id((
                                                        "repository-github-link",
                                                        repository_index,
                                                    ))
                                                    .flex_none()
                                                    .size(px(20.))
                                                    .rounded_sm()
                                                    .overflow_hidden()
                                                    .cursor_pointer()
                                                    .hover(|this| this.opacity(0.9))
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.open_external_url(
                                                                &github_url,
                                                                cx,
                                                            );
                                                            cx.stop_propagation();
                                                        },
                                                    ))
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
                                                    .into_any_element(),
                                                (Some(url), None) => div()
                                                    .flex_none()
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
                                                    .into_any_element(),
                                                (None, Some(github_url)) => div()
                                                    .id((
                                                        "repository-github-link",
                                                        repository_index,
                                                    ))
                                                    .flex_none()
                                                    .font_family(FONT_MONO)
                                                    .text_size(px(12.))
                                                    .text_color(rgb(theme.text_muted))
                                                    .cursor_pointer()
                                                    .hover(|this| this.opacity(0.9))
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.open_external_url(
                                                                &github_url,
                                                                cx,
                                                            );
                                                            cx.stop_propagation();
                                                        },
                                                    ))
                                                    .child("\u{f09b}")
                                                    .into_any_element(),
                                                (None, None) => div()
                                                    .flex_none()
                                                    .font_family(FONT_MONO)
                                                    .text_size(px(12.))
                                                    .text_color(rgb(theme.text_muted))
                                                    .child("\u{f09b}")
                                                    .into_any_element(),
                                            },
                                        )
                                        // Cell with chevron, name, count, etc.
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .justify_between()
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
                                                                .hover(|this| this.text_color(rgb(theme.text_primary)))
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
                                                // Sidebar item count badge
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(theme.text_disabled))
                                                        .child(format!(
                                                            "{}",
                                                            repo_worktrees.len() + repo_outposts.len()
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
                                                .size(px(20.))
                                                .rounded_sm()
                                                .cursor_pointer()
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(theme.text_muted))
                                                .hover(|this| this.text_color(rgb(theme.text_primary)))
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
                                )
                                .when(!is_collapsed, |this| {
                                    let selection_epoch = self.worktree_selection_epoch;
                                    let group_key = repository.group_key.clone();
                                    let repo_root = repository.root.clone();

                                    // Build worktree/outpost index maps for lookup
                                    let worktree_map: HashMap<PathBuf, (usize, WorktreeSummary)> =
                                        repo_worktrees.into_iter()
                                            .map(|(i, w)| (w.path.clone(), (i, w)))
                                            .collect();
                                    let outpost_map: HashMap<String, (usize, OutpostSummary)> =
                                        repo_outposts.into_iter()
                                            .map(|(i, o)| (o.outpost_id.clone(), (i, o)))
                                            .collect();

                                    // Build the unified ordered sidebar item list
                                    let sidebar_order = self.build_sidebar_order_for_group(&group_key, &repo_root);
                                    let item_count = sidebar_order.len();

                                    let mut elements: Vec<AnyElement> = Vec::with_capacity(item_count * 2 + 1);
                                    for (slot, item_id) in sidebar_order.into_iter().enumerate() {
                                        // Drop zone before this item
                                        {
                                            let dz_group = group_key.clone();
                                            let dz_root = repo_root.clone();
                                            let accent = theme.accent;
                                            elements.push(
                                                div()
                                                    .id(SharedString::from(format!("sidebar-drop-zone-{repository_index}-{slot}")))
                                                    .h(px(6.))
                                                    .mx(px(4.))
                                                    .rounded_sm()
                                                    .drag_over::<DraggedSidebarItem>({
                                                        move |style, _, _, _| style.bg(rgb(accent)).h(px(3.)).my(px(1.5))
                                                    })
                                                    .on_drop(cx.listener(move |this, dragged: &DraggedSidebarItem, window, cx| {
                                                        if dragged.group_key == dz_group {
                                                            this.handle_sidebar_item_drop(&dragged.item_id, slot, &dz_group, &dz_root, window, cx);
                                                        }
                                                    }))
                                                    .into_any_element(),
                                            );
                                        }

                                        match &item_id {
                                            SidebarItemId::Worktree(path) => {
                                                let Some((index, worktree)) = worktree_map.get(path).cloned() else { continue };
                                                let is_active =
                                                    self.active_worktree_index == Some(index);
                                                let diff_summary = worktree.diff_summary;
                                                let pr_number = worktree.pr_number;
                                                let pr_url = worktree.pr_url.clone();
                                                let is_merged_pr = worktree
                                                    .pr_details
                                                    .as_ref()
                                                    .is_some_and(|pr| {
                                                        pr.state == github_service::PrState::Merged
                                                    });
                                                let pr_badge_color = if is_merged_pr {
                                                    0xbb9af7_u32
                                                } else {
                                                    theme.accent
                                                };
                                                let is_primary = worktree.is_primary_checkout;
                                                let agent_dot_color = match worktree.agent_state {
                                                    Some(AgentState::Working) => Some(0xe5c07b_u32),
                                                    Some(AgentState::Waiting) => Some(0x61afef_u32),
                                                    None => None,
                                                };
                                                let drag_item_id = item_id.clone();
                                                let drag_group_key = group_key.clone();
                                                let drop_group_key = group_key.clone();
                                                let drop_repo_root = repo_root.clone();
                                                let drag_label = worktree.branch.clone();
                                                let drag_icon = worktree.checkout_kind.icon().to_owned();
                                                let drag_icon_color = theme.text_muted;
                                                let row = div()
                                                    .id(("worktree-row", index))
                                                    .font_family(FONT_MONO)
                                                    .cursor_pointer()
                                                    .rounded_sm()
                                                    .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                                    .flex()
                                                    .items_center()
                                                    .on_drag(DraggedSidebarItem { item_id: drag_item_id, group_key: drag_group_key, label: drag_label, icon: drag_icon, icon_color: drag_icon_color, bg_color: theme.panel_active_bg, border_color: theme.accent, text_color: theme.text_primary }, |dragged, _, _, cx| {
                                                        cx.stop_propagation();
                                                        cx.new(|_| dragged.clone())
                                                    })
                                                    .drag_over::<DraggedSidebarItem>({
                                                        let accent = theme.accent;
                                                        move |style, _, _, _| style.border_color(rgb(accent)).border_t_2()
                                                    })
                                                    .on_drop(cx.listener(move |this, dragged: &DraggedSidebarItem, window, cx| {
                                                        if dragged.group_key == drop_group_key {
                                                            this.handle_sidebar_item_drop(&dragged.item_id, slot, &drop_group_key, &drop_repo_root, window, cx);
                                                        }
                                                    }))
                                                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                                                        this.update_worktree_hover_mouse_position(event.position);
                                                    }))
                                                    .on_click(
                                                        cx.listener(move |this, _, window, cx| {
                                                            this.select_worktree(index, window, cx)
                                                        }),
                                                    )
                                                    .when(
                                                        !is_primary
                                                            || worktree.checkout_kind
                                                                == CheckoutKind::DiscreteClone,
                                                        |this| {
                                                        this.on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            this.worktree_context_menu = Some(WorktreeContextMenu {
                                                                worktree_index: index,
                                                                position: event.position,
                                                            });
                                                            this.worktree_hover_popover = None;
                                                            this._hover_show_task = None;
                                                            cx.notify();
                                                        }))
                                                    },
                                                    )
                                                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                                        this.update_worktree_hover_mouse_position(window.mouse_position());
                                                        if *hovered {
                                                            let mouse_position = window.mouse_position();
                                                            this.schedule_worktree_hover_popover_show(index, mouse_position.y, cx);
                                                        } else if this.worktree_hover_popover.as_ref().is_some_and(|p| p.worktree_index == index) {
                                                            this.schedule_worktree_hover_popover_dismiss(index, cx);
                                                        } else {
                                                            this.cancel_worktree_hover_popover_show();
                                                        }
                                                    }))
                                                    // Bordered cell
                                                    .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
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
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap(px(4.))
                                                        .hover(|this| {
                                                            this.bg(rgb(theme.panel_active_bg))
                                                        })
                                                        .when(is_active, |this| {
                                                            this.bg(rgb(theme.panel_active_bg))
                                                                .border_color(rgb(theme.accent))
                                                        })
                                                        .when(is_merged_pr && !is_active, |this| {
                                                            this.opacity(0.72)
                                                        })
                                                    // Git branch icon — vertically centered
                                                    .child(
                                                        div()
                                                            .flex_none()
                                                            .w(px(18.))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .text_size(px(16.))
                                                            .text_color(rgb(theme.text_muted))
                                                            .child(worktree.checkout_kind.icon()),
                                                    )
                                                    // Two-line text column
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .flex()
                                                            .flex_col()
                                                            .gap(px(1.))
                                                    // Line 1: [spinner] [name] ... [+- lines] [time ago]
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(2.))
                                                            // Activity spinner dot
                                                            .when_some(agent_dot_color, |this, color| {
                                                                this.child(
                                                                    div()
                                                                        .flex_none()
                                                                        .size(px(6.))
                                                                        .rounded_full()
                                                                        .bg(rgb(color)),
                                                                )
                                                            })
                                                            // Name/label
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
                                                                    .child(worktree.branch.clone()),
                                                            )
                                                            // Right side: [+- lines] [time ago]
                                                            .child({
                                                                let summary =
                                                                    diff_summary.unwrap_or_default();
                                                                let show_diff_summary =
                                                                    summary.additions > 0
                                                                        || summary.deletions > 0;
                                                                let mut right = div()
                                                                    .flex_none()
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap_1();

                                                                if self.worktree_stats_loading
                                                                    && diff_summary.is_none()
                                                                {
                                                                    right = right.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(rgb(
                                                                                theme.text_muted,
                                                                            ))
                                                                            .child("..."),
                                                                    );
                                                                } else if show_diff_summary {
                                                                    if summary.additions > 0 {
                                                                        right = right.child(
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
                                                                        );
                                                                    }
                                                                    if summary.deletions > 0 {
                                                                        right = right.child(
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
                                                                }

                                                                if let Some(activity_ms) = worktree.last_activity_unix_ms {
                                                                    right = right.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(rgb(
                                                                                theme.text_disabled,
                                                                            ))
                                                                            .child(format_relative_time(activity_ms)),
                                                                    );
                                                                }

                                                                right
                                                            }),
                                                    )
                                                    // Line 2: [agent task or dir name] ... [PR number]
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .child(
                                                                div()
                                                                    .min_w_0()
                                                                    .flex_1()
                                                                    .overflow_hidden()
                                                                    .whitespace_nowrap()
                                                                    .text_ellipsis()
                                                                    .text_xs()
                                                                    .text_color(rgb(theme.text_disabled))
                                                                    .child(
                                                                        worktree
                                                                            .agent_task
                                                                            .clone()
                                                                            .unwrap_or_else(|| worktree.label.clone()),
                                                                    ),
                                                            )
                                                            .when_some(pr_number, |this, pr_num| {
                                                                let pr_text = format!("#{pr_num}");
                                                                if let Some(pr_url) = pr_url.clone() {
                                                                    this.child(
                                                                        div()
                                                                            .id(("worktree-pr-link", index))
                                                                            .cursor_pointer()
                                                                            .flex_none()
                                                                            .text_xs()
                                                                            .text_color(rgb(pr_badge_color))
                                                                            .hover(|this| this.text_color(rgb(theme.text_primary)))
                                                                            .child(pr_text)
                                                                            .on_click(cx.listener(
                                                                                move |this, _, _, cx| {
                                                                                    this.open_external_url(
                                                                                        &pr_url,
                                                                                        cx,
                                                                                    );
                                                                                    cx.stop_propagation();
                                                                                },
                                                                            )),
                                                                    )
                                                                } else {
                                                                    this.child(
                                                                        div()
                                                                            .flex_none()
                                                                            .text_xs()
                                                                            .text_color(rgb(pr_badge_color))
                                                                            .child(pr_text),
                                                                    )
                                                                }
                                                            }),
                                                    )
                                                    ) // text column
                                                    ); // bordered cell
                                                if is_active {
                                                    elements.push(row.with_animation(
                                                        ("worktree-select", selection_epoch),
                                                        Animation::new(Duration::from_millis(150))
                                                            .with_easing(ease_in_out),
                                                        |el, delta| {
                                                            el.opacity(0.8 + 0.2 * delta)
                                                        },
                                                    )
                                                    .into_any_element());
                                                } else {
                                                    elements.push(row.opacity(0.8).into_any_element());
                                                }
                                            },
                                            SidebarItemId::Outpost(outpost_id) => {
                                                let Some((outpost_index, outpost)) = outpost_map.get(outpost_id).cloned() else { continue };
                                                    let is_active = self.active_outpost_index == Some(outpost_index);
                                                    let status_color = match outpost.status {
                                                        arbor_core::outpost::OutpostStatus::Available => theme.accent,
                                                        arbor_core::outpost::OutpostStatus::Unreachable => 0xeb6f92,
                                                        arbor_core::outpost::OutpostStatus::NotCloned | arbor_core::outpost::OutpostStatus::Provisioning => theme.text_muted,
                                                    };
                                                    let drag_item_id = item_id.clone();
                                                    let drag_group_key = group_key.clone();
                                                    let drop_group_key = group_key.clone();
                                                    let drop_repo_root = repo_root.clone();
                                                    let drag_label = format!("{}@{}", outpost.branch, outpost.hostname);
                                                    let drag_icon_color = status_color;
                                                    elements.push(div()
                                                        .id(("outpost-row", outpost_index))
                                                        .font_family(FONT_MONO)
                                                        .cursor_pointer()
                                                        .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                                        .flex()
                                                        .items_center()
                                                        .on_drag(DraggedSidebarItem { item_id: drag_item_id, group_key: drag_group_key, label: drag_label, icon: "\u{f0ac}".to_owned(), icon_color: drag_icon_color, bg_color: theme.panel_active_bg, border_color: theme.accent, text_color: theme.text_primary }, |dragged, _, _, cx| {
                                                            cx.stop_propagation();
                                                            cx.new(|_| dragged.clone())
                                                        })
                                                        .drag_over::<DraggedSidebarItem>({
                                                            let accent = theme.accent;
                                                            move |style, _, _, _| style.border_color(rgb(accent)).border_t_2()
                                                        })
                                                        .on_drop(cx.listener(move |this, dragged: &DraggedSidebarItem, window, cx| {
                                                            if dragged.group_key == drop_group_key {
                                                                this.handle_sidebar_item_drop(&dragged.item_id, slot, &drop_group_key, &drop_repo_root, window, cx);
                                                            }
                                                        }))
                                                        .on_click(cx.listener(move |this, _, window, cx| {
                                                            this.select_outpost(outpost_index, window, cx);
                                                        }))
                                                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            this.outpost_context_menu = Some(OutpostContextMenu {
                                                                outpost_index,
                                                                position: event.position,
                                                            });
                                                            cx.notify();
                                                        }))
                                                        // Bordered cell
                                                        .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .rounded_sm()
                                                            .border_1()
                                                            .border_color(rgb(if is_active { theme.accent } else { theme.border }))
                                                            .bg(rgb(theme.panel_bg))
                                                            .px_2()
                                                            .py_1()
                                                            .flex()
                                                            .flex_row()
                                                            .items_center()
                                                            .gap(px(4.))
                                                            .when(is_active, |this| this.bg(rgb(theme.panel_active_bg)))
                                                        // Globe icon — vertically centered
                                                        .child(
                                                            div()
                                                                .flex_none()
                                                                .w(px(18.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .text_size(px(18.))
                                                                .text_color(rgb(status_color))
                                                                .child("\u{f0ac}"),
                                                        )
                                                        // Two-line text column
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .flex()
                                                                .flex_col()
                                                                .gap(px(1.))
                                                        // Line 1: branch@host
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
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
                                                                        .child(format!("{}@{}", outpost.branch, outpost.hostname)),
                                                                ),
                                                        )
                                                        // Line 2: outpost label
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
                                                                .gap_2()
                                                                .child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .flex_1()
                                                                        .overflow_hidden()
                                                                        .whitespace_nowrap()
                                                                        .text_ellipsis()
                                                                        .text_xs()
                                                                        .text_color(rgb(theme.text_disabled))
                                                                        .child(outpost.label.clone()),
                                                                ),
                                                        )
                                                        )
                                                        )
                                                        .opacity(0.8)
                                                        .into_any_element());
                                            },
                                        }
                                    }

                                    // Final drop zone after last item
                                    {
                                        let dz_group = group_key;
                                        let dz_root = repo_root;
                                        let accent = theme.accent;
                                        elements.push(
                                            div()
                                                .id(SharedString::from(format!("sidebar-drop-zone-{repository_index}-{item_count}")))
                                                .h(px(6.))
                                                .mx(px(4.))
                                                .rounded_sm()
                                                .drag_over::<DraggedSidebarItem>({
                                                    move |style, _, _, _| style.bg(rgb(accent)).h(px(3.)).my(px(1.5))
                                                })
                                                .on_drop(cx.listener(move |this, dragged: &DraggedSidebarItem, window, cx| {
                                                    if dragged.group_key == dz_group {
                                                        this.handle_sidebar_item_drop(&dragged.item_id, item_count, &dz_group, &dz_root, window, cx);
                                                    }
                                                }))
                                                .into_any_element(),
                                        );
                                    }

                                    this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .children(elements),
                                )
                                })
                        },
                    ))
                    // ── Remote repos from expanded LAN daemons ───────────
                    .children({
                        // Build remote repo group elements imperatively so we can use cx.listener()
                        let mut remote_elements: Vec<AnyElement> = Vec::new();
                        let mut remote_wt_id = 0_usize;
                        for (&daemon_index, state) in &self.remote_daemon_states {
                            if !state.expanded {
                                continue;
                            }
                            let daemon_url = state.client.base_url();
                            // Show loading placeholder in the repo list
                            if state.loading {
                                remote_elements.push(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .h(px(32.))
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .font_family(FONT_MONO)
                                                        .text_size(px(12.))
                                                        .text_color(rgb(theme.text_muted))
                                                        .child("\u{f233}"),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(theme.text_disabled))
                                                        .child(format!("{}@{} — loading…", "", state.hostname)),
                                                ),
                                        )
                                        .into_any_element(),
                                );
                                continue;
                            }
                            if let Some(ref err) = state.error {
                                remote_elements.push(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0xe06c75_u32))
                                                .child(format!("{}@{}: {err}", "", state.hostname)),
                                        )
                                        .into_any_element(),
                                );
                                continue;
                            }
                            for repo in &state.repositories {
                                let repo_label = format!("{}@{}", repo.label, state.hostname);
                                let repo_wts: Vec<_> = state
                                    .worktrees
                                    .iter()
                                    .filter(|w| w.repo_root == repo.root)
                                    .collect();
                                let wt_count = repo_wts.len();

                                // Build worktree row elements
                                let mut wt_rows: Vec<AnyElement> = Vec::new();
                                for wt in &repo_wts {
                                    let branch = wt.branch.clone();
                                    let dir_label = wt.path.rsplit('/').next()
                                        .unwrap_or(&wt.path).to_owned();
                                    let additions = wt.diff_additions.unwrap_or(0);
                                    let deletions = wt.diff_deletions.unwrap_or(0);
                                    let has_diff = additions > 0 || deletions > 0;
                                    let pr_number = wt.pr_number;
                                    let last_activity = wt.last_activity_unix_ms;
                                    let click_path = wt.path.clone();
                                    let row_id = remote_wt_id;
                                    remote_wt_id += 1;
                                    let is_active = self.active_remote_worktree.as_ref().is_some_and(
                                        |arw| arw.daemon_index == daemon_index && arw.worktree_path == Path::new(&wt.path),
                                    );

                                    wt_rows.push(
                                        div()
                                            .id(("remote-wt-row", row_id))
                                            .font_family(FONT_MONO)
                                            .cursor_pointer()
                                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                            .flex()
                                            .items_center()
                                            .on_click(cx.listener(
                                                move |this, _, window, cx| {
                                                    this.select_remote_worktree(
                                                        daemon_index,
                                                        click_path.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ))
                                            .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(if is_active { theme.accent } else { theme.border }))
                                                .bg(rgb(theme.panel_bg))
                                                .px_2()
                                                .py_1()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap(px(4.))
                                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                                .when(is_active, |this| {
                                                    this.bg(rgb(theme.panel_active_bg))
                                                        .border_color(rgb(theme.accent))
                                                })
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .w(px(18.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(16.))
                                                    .text_color(rgb(theme.text_muted))
                                                    .child("\u{e725}"),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(1.))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(2.))
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
                                                            .child(branch),
                                                    )
                                                    .child({
                                                        let mut right = div()
                                                            .flex_none()
                                                            .flex()
                                                            .items_center()
                                                            .gap_1();
                                                        if has_diff {
                                                            if additions > 0 {
                                                                right = right.child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(rgb(0x72d69c))
                                                                        .child(format!("+{additions}")),
                                                                );
                                                            }
                                                            if deletions > 0 {
                                                                right = right.child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(rgb(0xeb6f92))
                                                                        .child(format!("-{deletions}")),
                                                                );
                                                            }
                                                        }
                                                        if let Some(activity_ms) = last_activity {
                                                            right = right.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(rgb(theme.text_disabled))
                                                                    .child(format_relative_time(activity_ms)),
                                                            );
                                                        }
                                                        right
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
                                                            .overflow_hidden()
                                                            .whitespace_nowrap()
                                                            .text_ellipsis()
                                                            .text_xs()
                                                            .text_color(rgb(theme.text_disabled))
                                                            .child(dir_label),
                                                    )
                                                    .when_some(pr_number, |el, pr_num| {
                                                        el.child(
                                                            div()
                                                                .flex_none()
                                                                .text_xs()
                                                                .text_color(rgb(theme.accent))
                                                                .child(format!("#{pr_num}")),
                                                        )
                                                    }),
                                            )
                                            ) // text column
                                            ) // bordered cell
                                            .when(!is_active, |el| el.opacity(0.8))
                                            .into_any_element(),
                                    );
                                }

                                // Repo header + worktree rows
                                let avatar_url = repo.avatar_url.clone();
                                let icon: AnyElement = if let Some(url) = avatar_url {
                                    div()
                                        .flex_none()
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
                                                        .text_color(rgb(theme.text_muted))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .child("\u{f233}")
                                                        .into_any_element()
                                                }),
                                        )
                                        .into_any_element()
                                } else {
                                    div()
                                        .flex_none()
                                        .font_family(FONT_MONO)
                                        .text_size(px(12.))
                                        .text_color(rgb(theme.text_muted))
                                        .child("\u{f233}")
                                        .into_any_element()
                                };

                                // "+" button to create worktree on remote
                                let plus_url = daemon_url.clone();
                                let plus_hostname = state.hostname.clone();
                                let plus_repo_root = repo.root.clone();
                                let plus_id = remote_wt_id;
                                remote_wt_id += 1;

                                remote_elements.push(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .h(px(32.))
                                                .child(icon)
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .child(
                                                            div()
                                                                .min_w_0()
                                                                .flex_1()
                                                                .flex()
                                                                .items_center()
                                                                .gap_1()
                                                                .child(
                                                                    div()
                                                                        .text_size(px(16.))
                                                                        .text_color(rgb(theme.text_muted))
                                                                        .w(px(14.))
                                                                        .flex()
                                                                        .items_center()
                                                                        .justify_center()
                                                                        .child("\u{25BE}"),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .overflow_hidden()
                                                                        .whitespace_nowrap()
                                                                        .text_ellipsis()
                                                                        .text_sm()
                                                                        .font_weight(FontWeight::MEDIUM)
                                                                        .text_color(rgb(theme.text_primary))
                                                                        .child(repo_label),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(rgb(theme.text_disabled))
                                                                        .child(format!("{wt_count}")),
                                                                ),
                                                        )
                                                        // "+" button
                                                        .child(
                                                            div()
                                                                .id(("remote-repo-add-wt", plus_id))
                                                                .size(px(20.))
                                                                .rounded_sm()
                                                                .cursor_pointer()
                                                                .flex_none()
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .text_sm()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(theme.text_muted))
                                                                .hover(|this| this.text_color(rgb(theme.text_primary)))
                                                                .child("+")
                                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                                    this.open_remote_create_modal(
                                                                        plus_url.clone(),
                                                                        plus_hostname.clone(),
                                                                        plus_repo_root.clone(),
                                                                        cx,
                                                                    );
                                                                    cx.stop_propagation();
                                                                })),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(6.))
                                                .children(wt_rows),
                                        )
                                        .into_any_element(),
                                );
                            }
                        }
                        remote_elements
                    }),
            )
            // ── LAN Daemons section ──────────────────────────────────────
            .when(!self.discovered_daemons.is_empty(), |pane| {
                let daemons = self.discovered_daemons.clone();
                pane.child(div().h(px(1.)).bg(rgb(theme.border)))
                    .child(
                        div()
                            .px_2()
                            .pt_2()
                            .pb_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_family(FONT_MONO)
                                            .text_size(px(14.))
                                            .text_color(rgb(theme.text_muted))
                                            .child("\u{f0ac}"), // globe/network icon
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(theme.text_muted))
                                            .child("LAN Daemons"),
                                    ),
                            )
                            .children(daemons.into_iter().enumerate().map(
                                |(daemon_index, daemon)| {
                                    let remote_state = self.remote_daemon_states.get(&daemon_index);
                                    let is_expanded = remote_state.is_some_and(|s| s.expanded);
                                    let is_loading = remote_state.is_some_and(|s| s.loading);
                                    let display_name = daemon.display_name().to_owned();
                                    let chevron = if is_expanded { "\u{f078}" } else { "\u{f054}" };
                                    let mut col = div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .id(("lan-daemon-row", daemon_index))
                                                .cursor_pointer()
                                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                                .flex()
                                                .items_center()
                                                .on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        this.toggle_discovered_daemon(
                                                            daemon_index,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .rounded_sm()
                                                        .border_1()
                                                        .border_color(rgb(if is_expanded {
                                                            theme.accent
                                                        } else {
                                                            theme.border
                                                        }))
                                                        .bg(rgb(if is_expanded {
                                                            theme.panel_active_bg
                                                        } else {
                                                            theme.panel_bg
                                                        }))
                                                        .px_2()
                                                        .py_1()
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap(px(4.))
                                                        .child(
                                                            div()
                                                                .flex_none()
                                                                .w(px(12.))
                                                                .font_family(FONT_MONO)
                                                                .text_size(px(10.))
                                                                .text_color(rgb(theme.text_muted))
                                                                .child(chevron),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_none()
                                                                .w(px(18.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .font_family(FONT_MONO)
                                                                .text_size(px(18.))
                                                                .text_color(rgb(theme.accent))
                                                                .child("\u{f233}"),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .text_xs()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(theme.text_primary))
                                                                .child(display_name),
                                                        ),
                                                ),
                                        );

                                    // Loading/error status below the toggle
                                    if let Some(state) = remote_state
                                        && state.expanded
                                    {
                                        if is_loading {
                                            col = col.child(
                                                div()
                                                    .pl(px(30.))
                                                    .py_1()
                                                    .text_xs()
                                                    .text_color(rgb(theme.text_disabled))
                                                    .child("Loading…"),
                                            );
                                        } else if let Some(ref err) = state.error {
                                            col = col.child(
                                                div()
                                                    .pl(px(30.))
                                                    .py_1()
                                                    .text_xs()
                                                    .text_color(rgb(0xe06c75_u32))
                                                    .child(err.clone()),
                                            );
                                        }
                                    }

                                    col
                                },
                            )),
                    )
            })
            // ── Bottom bar ───────────────────────────────────────────────
            .child(div().h(px(1.)).bg(rgb(theme.border)))
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("open-add-repository")
                            .cursor_pointer()
                            .h(px(24.))
                            .w_full()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.panel_bg))
                            .hover(|s| {
                                s.bg(rgb(theme.panel_active_bg))
                                    .border_color(rgb(theme.accent))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_add_repository_picker(cx);
                            }))
                            .child(
                                div()
                                    .h_full()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_family(FONT_MONO)
                                            .text_size(px(11.))
                                            .text_color(rgb(theme.accent))
                                            .child("\u{f067}"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(theme.text_primary))
                                            .child("Add Repository"),
                                    ),
                            ),
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
        let file_view_sessions = self.selected_worktree_file_view_sessions();
        let mut tabs: Vec<CenterTab> = terminals
            .iter()
            .map(|session| CenterTab::Terminal(session.id))
            .collect();
        tabs.extend(
            diff_sessions
                .iter()
                .map(|session| CenterTab::Diff(session.id)),
        );
        tabs.extend(
            file_view_sessions
                .iter()
                .map(|session| CenterTab::FileView(session.id)),
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
            self.active_file_view_session_id = match active_tab {
                Some(CenterTab::FileView(fv_id)) => Some(fv_id),
                _ => None,
            };
        }

        let active_tab_index =
            active_tab.and_then(|tab| tabs.iter().position(|entry| *entry == tab));
        if let Some(index) = active_tab_index {
            self.center_tabs_scroll_handle.scroll_to_item(index);
        }
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
        let active_file_view_session = match active_tab {
            Some(CenterTab::FileView(fv_id)) => self
                .file_view_sessions
                .iter()
                .find(|session| session.id == fv_id)
                .cloned(),
            _ => None,
        };
        let active_preset_tab = self.active_preset_tab;
        let preset_button = |kind: AgentPresetKind| {
            let is_active = active_preset_tab == Some(kind);
            let text_color = if is_active {
                theme.text_primary
            } else {
                theme.text_muted
            };
            div()
                .id(ElementId::Name(
                    format!("terminal-preset-tab-{}", kind.key()).into(),
                ))
                .cursor_pointer()
                .h(px(22.))
                .px_2()
                .flex()
                .items_center()
                .rounded_sm()
                .border_b_1()
                .border_color(rgb(if is_active {
                    theme.accent
                } else {
                    theme.tab_bg
                }))
                .text_color(rgb(text_color))
                .hover(|s| {
                    s.bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(theme.text_primary))
                })
                .child(agent_preset_button_content(kind, text_color))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.active_preset_tab = Some(kind);
                        this.launch_agent_preset(kind, window, cx);
                    }),
                )
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
            .child({
                let entity = cx.entity().clone();
                let focus = self.terminal_focus.clone();
                canvas(
                    move |_, _, _| {},
                    move |_, _, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(
                                Bounds {
                                    origin: point(px(0.), px(0.)),
                                    size: size(px(0.), px(0.)),
                                },
                                entity.clone(),
                            ),
                            cx,
                        );
                    },
                )
                .size(px(0.))
            })
            .child(
                div()
                    .h(px(32.))
                    .bg(rgb(theme.tab_bg))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .id("center-tabs-scroll")
                            .track_scroll(&self.center_tabs_scroll_handle)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
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
                                                terminal_tab_icon_element(
                                                    if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    },
                                                    16.0,
                                                )
                                                .into_any_element(),
                                                self.terminals
                                                    .iter()
                                                    .find(|session| session.id == session_id)
                                                    .map(terminal_tab_title)
                                                    .unwrap_or_else(|| "terminal".to_owned()),
                                            ),
                                            CenterTab::Diff(diff_id) => (
                                                div()
                                                    .font_family(FONT_MONO)
                                                    .text_xs()
                                                    .text_color(rgb(if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    }))
                                                    .child(TAB_ICON_DIFF)
                                                    .into_any_element(),
                                                self.diff_sessions
                                                    .iter()
                                                    .find(|session| session.id == diff_id)
                                                    .map(diff_tab_title)
                                                    .unwrap_or_else(|| "diff".to_owned()),
                                            ),
                                            CenterTab::FileView(fv_id) => (
                                                div()
                                                    .font_family(FONT_MONO)
                                                    .text_xs()
                                                    .text_color(rgb(if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    }))
                                                    .child(TAB_ICON_FILE)
                                                    .into_any_element(),
                                                self.file_view_sessions
                                                    .iter()
                                                    .find(|session| session.id == fv_id)
                                                    .map(|s| s.title.clone())
                                                    .unwrap_or_else(|| "file".to_owned()),
                                            ),
                                            CenterTab::Logs => (
                                                logs_tab_icon_element(
                                                    if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    },
                                                    16.0,
                                                )
                                                .into_any_element(),
                                                "Logs".to_owned(),
                                            ),
                                        };
                                        let tab_id = match tab {
                                            CenterTab::Terminal(id) => ("center-tab-terminal", id),
                                            CenterTab::Diff(id) => ("center-tab-diff", id),
                                            CenterTab::FileView(id) => ("center-tab-fileview", id),
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
                                            .when(!is_active, |this| {
                                                this.hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                            })
                                            .child(tab_icon)
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
                                                        CenterTab::FileView(id) => ("tab-close-fileview", id),
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
                                                                CenterTab::FileView(fv_id) => {
                                                                    if this.close_file_view_session_by_id(fv_id) {
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
                                                CenterTab::FileView(fv_id) => {
                                                    this.logs_tab_active = false;
                                                    this.select_file_view_tab(fv_id, cx);
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
                                            .hover(|this| this.text_color(rgb(theme.text_primary)))
                                            .child("+")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(
                                                    |this, _: &MouseDownEvent, window, cx| {
                                                        this.spawn_terminal_session(window, cx)
                                                    },
                                                ),
                                            ),
                                    ),
                            ),
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
                            .children(
                                AgentPresetKind::ORDER
                                    .iter()
                                    .copied()
                                    .filter(|kind| installed_preset_kinds().contains(kind))
                                    .map(&preset_button),
                            )
                            .children(self.repo_presets.iter().enumerate().map(|(index, preset)| {
                                let icon_text = preset.icon.clone();
                                let name_text = preset.name.clone();
                                div()
                                    .id(ElementId::Name(
                                        format!("terminal-repo-preset-tab-{index}").into(),
                                    ))
                                    .cursor_pointer()
                                    .h(px(22.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded_sm()
                                    .border_b_1()
                                    .border_color(rgb(theme.tab_bg))
                                    .text_color(rgb(theme.text_muted))
                                    .hover(|s| {
                                        s.bg(rgb(theme.panel_active_bg))
                                            .text_color(rgb(theme.text_primary))
                                    })
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.))
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .line_height(px(14.))
                                                    .child(if icon_text.is_empty() {
                                                        "\u{f013}".to_owned()
                                                    } else {
                                                        icon_text
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .line_height(px(14.))
                                                    .child(name_text),
                                            ),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                            this.launch_repo_preset(index, window, cx);
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                            this.open_manage_repo_presets_modal(Some(index), cx);
                                        }),
                                    )
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(rgb(theme.terminal_bg))
                    .when(
                        active_terminal.is_none() && active_diff_session.is_none() && active_file_view_session.is_none() && active_tab != Some(CenterTab::Logs),
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
                                            ActionButtonStyle::Secondary,
                                            true,
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
                        let ime_text = self.ime_marked_text.as_deref();
                        let styled_lines =
                            styled_lines_for_session(&session, theme, true, selection, ime_text);
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
                    .when_some(active_file_view_session, |this, session| {
                        let mono_font = terminal_mono_font(cx);
                        let editing = self.file_view_editing;
                        this.child(render_file_view_session(
                            session,
                            theme,
                            &self.file_view_scroll_handle,
                            mono_font,
                            editing,
                            cx,
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
                                    .hover(|this| this.text_color(rgb(theme.text_primary)))
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
                                    .hover(|this| this.text_color(rgb(theme.text_primary)))
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
        let search_active = self.right_pane_search_active;
        let search_text = self.right_pane_search.clone();

        div()
            .w(px(self.right_pane_width))
            .h_full()
            .min_h_0()
            .bg(rgb(theme.sidebar_bg))
            .flex()
            .flex_col()
            .child(self.render_right_pane_tabs(cx))
            .child(
                div()
                    .id("right-pane-search")
                    .h(px(28.))
                    .mx_1()
                    .my(px(4.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(if search_active {
                        theme.accent
                    } else {
                        theme.border
                    }))
                    .bg(rgb(theme.panel_bg))
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.right_pane_search_active = true;
                            this.right_pane_search_cursor = char_count(&this.right_pane_search);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_xs()
                            .min_w_0()
                            .flex_1()
                            .child(if search_active {
                                if search_text.is_empty() {
                                    active_input_display(
                                        theme,
                                        "",
                                        "Filter files…",
                                        theme.text_disabled,
                                        self.right_pane_search_cursor,
                                        28,
                                    )
                                } else {
                                    active_input_display(
                                        theme,
                                        &search_text,
                                        "Filter files…",
                                        theme.text_primary,
                                        self.right_pane_search_cursor,
                                        28,
                                    )
                                }
                            } else if search_text.is_empty() {
                                div()
                                    .text_color(rgb(theme.text_disabled))
                                    .child("Filter files…")
                                    .into_any_element()
                            } else {
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(rgb(theme.text_primary))
                                    .child(search_text)
                                    .into_any_element()
                            }),
                    ),
            )
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
                .when(!is_active, |this| {
                    this.hover(|this| {
                        this.bg(rgb(theme.panel_active_bg))
                            .text_color(rgb(theme.text_primary))
                    })
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
        let search_lower = self.right_pane_search.to_lowercase();
        let filtered_changes: Vec<_> = self
            .changed_files
            .iter()
            .filter(|change| {
                search_lower.is_empty()
                    || change
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&search_lower)
            })
            .cloned()
            .collect();

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
                    .children(filtered_changes.iter().map(|change| {
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
                        let display_path = truncate_middle_path_for_width(
                            change.path.as_path(),
                            self.right_pane_width,
                        );
                        let file_path = change.path.clone();

                        div()
                            .id(ElementId::Name(
                                format!("changed-file-{}", display_path).into(),
                            ))
                            .h(px(24.))
                            .pl(px(4.))
                            .pr_1()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(is_selected, |this| this.bg(rgb(theme.panel_active_bg)))
                            .when(!is_selected, |this| {
                                this.hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            })
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
                                    .when(change.additions > 0, |this| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x72d69c))
                                                .child(format!("+{}", change.additions)),
                                        )
                                    })
                                    .when(change.deletions > 0, |this| {
                                        this.child(
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
        let search_lower = self.right_pane_search.to_lowercase();
        let is_filtering = !search_lower.is_empty();
        let filtered_entries: Vec<_> = self
            .file_tree_entries
            .iter()
            .filter(|entry| {
                if !is_filtering {
                    return true;
                }
                if entry.is_dir {
                    return false;
                }
                entry
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&search_lower)
            })
            .collect();

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
                .children(filtered_entries.iter().map(|entry| {
                    let is_selected = selected_entry
                        .as_ref()
                        .is_some_and(|selected| selected == &entry.path);
                    let indent = if is_filtering {
                        4.
                    } else {
                        entry.depth as f32 * 16. + 4.
                    };
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
                        .when(!is_selected, |this| {
                            this.hover(|this| this.bg(rgb(theme.panel_active_bg)))
                        })
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
                                .child(if is_filtering {
                                    entry.path.to_string_lossy().into_owned()
                                } else {
                                    entry.name.clone()
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
                    .child(status_text(theme, "•"))
                    .child(status_text(
                        theme,
                        format!("theme {}", self.theme_kind.label()),
                    ))
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
        let checkout_kind = modal.checkout_kind;
        let is_discrete_clone = checkout_kind == CheckoutKind::DiscreteClone;
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
            .map(|h| {
                let dir_name =
                    arbor_ssh::provisioner::sanitize_outpost_dir_name(&modal.outpost_name);
                format!("{}/{dir_name}", h.remote_base_path)
            })
            .unwrap_or_else(|| "-".to_owned());
        let host_active = modal.outpost_active_field == CreateOutpostField::HostSelector;
        let host_dropdown_open = modal.host_dropdown_open;
        let host_names: Vec<(usize, String)> = self
            .remote_hosts
            .iter()
            .enumerate()
            .map(|(i, h)| (i, h.name.clone()))
            .collect();
        let selected_host_index = modal.host_index;
        let clone_url_active = modal.outpost_active_field == CreateOutpostField::CloneUrl;
        let outpost_name_active = modal.outpost_active_field == CreateOutpostField::OutpostName;
        let outpost_branch_preview = derive_branch_name(&modal.outpost_name);
        let outpost_create_disabled = modal.is_creating
            || modal.clone_url.trim().is_empty()
            || modal.outpost_name.trim().is_empty()
            || self.remote_hosts.is_empty();

        let create_disabled = if is_worktree_tab {
            worktree_create_disabled
        } else {
            outpost_create_disabled
        };
        let creating_status = modal.creating_status.clone();
        let submit_label: String = if modal.is_creating {
            creating_status.as_deref().unwrap_or("Creating…").to_owned()
        } else if is_worktree_tab {
            checkout_kind.action_label().to_owned()
        } else {
            "Create Outpost".to_owned()
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_create_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_create_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(620.))
                    .max_w(px(620.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Header
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Add"),
                    )
                    // Tab bar
                    .child(
                        div()
                            .flex_none()
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
                                    .when(!is_worktree_tab, |this| {
                                        this.hover(|this| this.text_color(rgb(theme.text_primary)))
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
                                        .when(!is_outpost_tab, |this| {
                                            this.hover(|this| this.text_color(rgb(theme.text_primary)))
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
                                .flex_none()
                                .text_xs()
                                .text_color(rgb(theme.text_muted))
                                .child("Target base: ~/.arbor/worktrees/<repo>/<worktree>/"),
                        )
                        .child(
                            div()
                                .flex_none()
                                .id("create-discrete-clone-checkbox")
                                .cursor_pointer()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(if is_discrete_clone {
                                    theme.accent
                                } else {
                                    theme.border
                                }))
                                .bg(rgb(theme.panel_bg))
                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                .px_2()
                                .py_2()
                                .flex()
                                .items_start()
                                .gap_2()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let next_kind = if is_discrete_clone {
                                        CheckoutKind::LinkedWorktree
                                    } else {
                                        CheckoutKind::DiscreteClone
                                    };
                                    this.set_create_modal_checkout_kind(next_kind, cx);
                                }))
                                .child(
                                    div()
                                        .mt(px(1.))
                                        .w(px(14.))
                                        .h(px(14.))
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(if is_discrete_clone {
                                            theme.accent
                                        } else {
                                            theme.border
                                        }))
                                        .bg(rgb(if is_discrete_clone {
                                            theme.accent
                                        } else {
                                            theme.panel_bg
                                        }))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .font_family(FONT_MONO)
                                                .text_size(px(9.))
                                                .text_color(rgb(if is_discrete_clone {
                                                    theme.sidebar_bg
                                                } else {
                                                    theme.panel_bg
                                                }))
                                                .child(if is_discrete_clone {
                                                    "\u{f00c}"
                                                } else {
                                                    ""
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.))
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(theme.text_primary))
                                                .child("Discrete clone"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(theme.text_muted))
                                                .child(checkout_kind.description()),
                                        ),
                                ),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "create-worktree-repo-input",
                                "Repository",
                                &modal.repository_path,
                                modal.repository_path_cursor,
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
                                modal.worktree_name_cursor,
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
                                .flex_none()
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
                                .flex_none()
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
                                .flex_none()
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
                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
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
                                                .text_xs()
                                                .text_color(rgb(theme.text_muted))
                                                .child(if host_dropdown_open {
                                                    "\u{25b2}"
                                                } else {
                                                    "\u{25bc}"
                                                }),
                                        ),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.update_create_outpost_modal_input(
                                        OutpostModalInputEvent::ToggleHostDropdown,
                                        cx,
                                    );
                                })),
                        )
                        .when(host_dropdown_open, |this| {
                            this.child(
                                div()
                                    .id("outpost-host-dropdown")
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(theme.accent))
                                    .bg(rgb(theme.panel_bg))
                                    .py_1()
                                    .max_h(px(200.))
                                    .overflow_y_scroll()
                                    .children(host_names.into_iter().map(
                                        |(index, name)| {
                                            let is_selected = index == selected_host_index;
                                            div()
                                                .id(("host-option", index))
                                                .cursor_pointer()
                                                .px_2()
                                                .py_1()
                                                .text_sm()
                                                .font_family(FONT_MONO)
                                                .rounded_sm()
                                                .mx_1()
                                                .text_color(rgb(theme.text_primary))
                                                .when(is_selected, |this| {
                                                    this.bg(rgb(theme.panel_active_bg))
                                                })
                                                .hover(|this| {
                                                    this.bg(rgb(theme.panel_active_bg))
                                                })
                                                .child(name)
                                                .on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        this.update_create_outpost_modal_input(
                                                            OutpostModalInputEvent::SelectHost(
                                                                index,
                                                            ),
                                                            cx,
                                                        );
                                                    },
                                                ))
                                        },
                                    )),
                            )
                        })
                        .child(
                            modal_input_field(
                                theme,
                                "outpost-clone-url-input",
                                "Clone URL",
                                &modal.clone_url,
                                modal.clone_url_cursor,
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
                                "outpost-name-input",
                                "Outpost Name",
                                &modal.outpost_name,
                                modal.outpost_name_cursor,
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
                                        .child("Branch"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family(FONT_MONO)
                                        .text_color(rgb(theme.text_primary))
                                        .child(outpost_branch_preview),
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
                    .when_some(modal.error.clone(), |this, error| {
                        this.child(
                            div()
                                .flex_none()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(0xa44949))
                                .bg(rgb(0x4d2a2a))
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(rgb(0xffd7d7))
                                .child(error),
                        )
                    })
                    // Buttons
                    .child(
                        div()
                            .flex_none()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "cancel-create-modal",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
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
                                    ActionButtonStyle::Primary,
                                    !create_disabled,
                                )
                                .when(!create_disabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
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
                                    }))
                                }),
                            ),
                    ),
            )
    }

    fn render_github_auth_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.github_auth_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let copy_feedback_active = self.github_auth_copy_feedback_active;
        let copy_label = if copy_feedback_active {
            "Copied"
        } else {
            "Copy code"
        };
        let status_line = if self.github_auth_in_progress {
            "Waiting for GitHub authorization..."
        } else {
            "Authorization complete."
        };
        let detail_line = if self.github_auth_in_progress {
            "Arbor will continue automatically after you approve access."
        } else {
            "You can close this dialog."
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_github_auth_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_github_auth_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(560.))
                    .max_w(px(560.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
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
                                    .child("GitHub Sign In"),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "close-github-auth-modal",
                                    "Close",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_github_auth_modal(cx);
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child("1. Open GitHub and enter this device code."),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child("2. Return here after approving Arbor."),
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
                                    .child("Device code"),
                            )
                            .child(
                                div()
                                    .pt_1()
                                    .text_lg()
                                    .font_family(FONT_MONO)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_primary))
                                    .child(modal.user_code),
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
                                    .child("Verification URL"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(FONT_MONO)
                                    .text_color(rgb(theme.text_primary))
                                    .child(modal.verification_url),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if copy_feedback_active {
                                0x68c38d
                            } else {
                                theme.accent
                            }))
                            .child(if copy_feedback_active {
                                "Code copied to clipboard"
                            } else {
                                status_line
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(detail_line),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "github-auth-copy-code",
                                    copy_label,
                                    if copy_feedback_active {
                                        ActionButtonStyle::Primary
                                    } else {
                                        ActionButtonStyle::Secondary
                                    },
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.copy_github_auth_code_to_clipboard(cx);
                                    },
                                )),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "github-auth-open",
                                    "Open GitHub",
                                    ActionButtonStyle::Primary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.open_github_auth_verification_page(cx);
                                    },
                                )),
                            ),
                    ),
            )
    }

    fn render_repository_context_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(menu) = self.repository_context_menu.as_ref() else {
            return div();
        };

        let theme = self.theme();
        let index = menu.repository_index;
        let position = menu.position;

        // Full-screen invisible overlay to dismiss on click outside,
        // with the menu as a child — same pattern as render_top_bar_worktree_quick_actions_overlay
        div()
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.repository_context_menu = None;
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                this.repository_context_menu = None;
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                this.repository_context_menu = None;
                cx.notify();
            }))
            // Absolutely-positioned menu at cursor position
            .child(
                div()
                    .absolute()
                    .left(position.x)
                    .top(position.y)
                    .w(px(180.))
                    .py(px(4.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.chrome_bg))
                    .on_mouse_move(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .id("repository-context-remove")
                            .h(px(30.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(0x3a2030)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let label = this
                                    .repositories
                                    .get(index)
                                    .map(|r| r.label.clone())
                                    .unwrap_or_default();
                                this.repository_context_menu = None;
                                this.delete_modal = Some(DeleteModal {
                                    target: DeleteTarget::Repository(index),
                                    label,
                                    branch: String::new(),
                                    has_unpushed: None,
                                    delete_branch: false,
                                    is_deleting: false,
                                    error: None,
                                });
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(16.))
                                    .text_color(rgb(0xeb6f92))
                                    .child("\u{f1f8}"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(0xeb6f92))
                                    .child("Remove"),
                            ),
                    ),
            )
    }

    fn render_worktree_context_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(menu) = self.worktree_context_menu.as_ref() else {
            return div();
        };

        let theme = self.theme();
        let index = menu.worktree_index;
        let position = menu.position;

        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.worktree_context_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.worktree_context_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                this.worktree_context_menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .absolute()
                    .left(position.x)
                    .top(position.y)
                    .w(px(180.))
                    .py(px(4.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.chrome_bg))
                    .on_mouse_move(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .id("worktree-context-delete")
                            .h(px(30.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(0x3a2030)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.worktree_context_menu = None;
                                let wt_label = this
                                    .worktrees
                                    .get(index)
                                    .map(|wt| wt.label.clone())
                                    .unwrap_or_default();
                                let wt_branch = this
                                    .worktrees
                                    .get(index)
                                    .map(|wt| wt.branch.clone())
                                    .unwrap_or_default();
                                this.open_delete_modal(
                                    DeleteTarget::Worktree(index),
                                    wt_label,
                                    wt_branch,
                                    cx,
                                );
                            }))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(16.))
                                    .text_color(rgb(0xeb6f92))
                                    .child("\u{f1f8}"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(0xeb6f92))
                                    .child("Delete"),
                            ),
                    ),
            )
    }

    fn render_worktree_hover_popover(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(popover) = self.worktree_hover_popover.as_ref() else {
            return div();
        };
        let Some(worktree) = self.worktrees.get(popover.worktree_index) else {
            return div();
        };

        let theme = self.theme();
        let checks_expanded = popover.checks_expanded;
        let popover_zone_bounds =
            worktree_hover_popover_zone_bounds(self.left_pane_width, popover, worktree);

        // Build popover card content
        let popover_wt_index = popover.worktree_index;
        let mut card = div()
            .id("worktree-hover-popover-card")
            .font_family(FONT_MONO)
            .w(px(WORKTREE_HOVER_POPOVER_CARD_WIDTH_PX))
            .bg(rgb(theme.panel_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .rounded_md()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.cancel_worktree_hover_popover_dismiss();
                } else {
                    this.schedule_worktree_hover_popover_dismiss(popover_wt_index, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            });

        // Header: branch name + relative time (top-right), then directory label
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(theme.text_primary))
                                .child(worktree.branch.clone()),
                        )
                        .when_some(worktree.last_activity_unix_ms, |el, ms| {
                            el.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child(format_relative_time(ms)),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_muted))
                        .child(worktree.label.clone()),
                ),
        );

        // Diff summary
        if let Some(summary) = worktree.diff_summary
            && (summary.additions > 0 || summary.deletions > 0)
        {
            let mut diff_row = div().flex().items_center().gap_1();
            if summary.additions > 0 {
                diff_row = diff_row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x72d69c))
                        .child(format!("+{}", summary.additions)),
                );
            }
            if summary.deletions > 0 {
                diff_row = diff_row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xeb6f92))
                        .child(format!("-{}", summary.deletions)),
                );
            }
            card = card.child(diff_row);
        }

        // Agent section
        if let Some(state) = worktree.agent_state {
            let (dot_color, state_label) = match state {
                AgentState::Working => (0xe5c07b_u32, "Working"),
                AgentState::Waiting => (0x61afef_u32, "Waiting"),
            };

            let mut agent_row = div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .flex_none()
                        .size(px(6.))
                        .rounded_full()
                        .bg(rgb(dot_color)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_primary))
                        .child(state_label),
                );

            if let Some(ref task) = worktree.agent_task {
                agent_row = agent_row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_muted))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(task.clone()),
                );
            }

            card = card.child(agent_row);
        }

        // PR section
        if let Some(ref pr) = worktree.pr_details {
            card = card.child(div().h(px(1.)).bg(rgb(theme.border)).my_1());

            let (state_label, state_color) = match pr.state {
                github_service::PrState::Open => ("Open", 0x72d69c_u32),
                github_service::PrState::Draft => ("Draft", theme.text_disabled),
                github_service::PrState::Merged => ("Merged", 0xbb9af7_u32),
                github_service::PrState::Closed => ("Closed", 0xeb6f92_u32),
            };

            let pr_url = pr.url.clone();
            let mut pr_header = div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .id("popover-pr-link")
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme.accent))
                        .hover(|this| this.text_color(rgb(theme.text_primary)))
                        .child(format!("#{}", pr.number))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_external_url(&pr_url, cx);
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .px_1()
                        .rounded_sm()
                        .text_color(rgb(state_color))
                        .child(state_label),
                );

            if pr.additions > 0 || pr.deletions > 0 {
                if pr.additions > 0 {
                    pr_header = pr_header.child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x72d69c))
                            .child(format!("+{}", pr.additions)),
                    );
                }
                if pr.deletions > 0 {
                    pr_header = pr_header.child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xeb6f92))
                            .child(format!("-{}", pr.deletions)),
                    );
                }
            }
            card = card.child(pr_header);

            // PR title
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child(pr.title.clone()),
            );

            // Checks + review (only for open/draft PRs)
            if pr.state == github_service::PrState::Open
                || pr.state == github_service::PrState::Draft
            {
                let mut status_row = div().flex().items_center().gap_1();

                if !pr.checks.is_empty() {
                    let passed = pr
                        .checks
                        .iter()
                        .filter(|(_, s)| *s == github_service::CheckStatus::Success)
                        .count();
                    let total = pr.checks.len();
                    let (check_icon, check_color) = match pr.checks_status {
                        github_service::CheckStatus::Success => ("\u{f00c}", 0x72d69c_u32),
                        github_service::CheckStatus::Failure => ("\u{f00d}", 0xeb6f92_u32),
                        github_service::CheckStatus::Pending => ("\u{f192}", 0xe5c07b_u32),
                    };
                    let chevron = if checks_expanded {
                        "\u{f078}"
                    } else {
                        "\u{f054}"
                    };
                    status_row = status_row.child(
                        div()
                            .id("popover-checks-toggle")
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(check_color))
                                    .child(check_icon),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child(format!("{passed}/{total} checks")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child(chevron),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(ref mut p) = this.worktree_hover_popover {
                                    p.checks_expanded = !p.checks_expanded;
                                }
                                cx.notify();
                            })),
                    );
                }

                let (review_label, review_color) = match pr.review_decision {
                    github_service::ReviewDecision::Approved => ("Approved", 0x72d69c_u32),
                    github_service::ReviewDecision::ChangesRequested => {
                        ("Changes requested", 0xeb6f92_u32)
                    },
                    github_service::ReviewDecision::Pending => {
                        ("Review pending", theme.text_disabled)
                    },
                };
                status_row = status_row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(review_color))
                        .child(review_label),
                );

                card = card.child(status_row);

                // Expanded checks list
                if checks_expanded {
                    let mut checks_list = div().flex().flex_col().gap(px(2.)).pl_2();
                    let mut sorted_checks: Vec<_> = pr.checks.iter().collect();
                    sorted_checks.sort_by_key(|(_, status)| match status {
                        github_service::CheckStatus::Failure => 0,
                        github_service::CheckStatus::Pending => 1,
                        github_service::CheckStatus::Success => 2,
                    });
                    for (name, status) in sorted_checks {
                        let (icon, color) = match status {
                            github_service::CheckStatus::Success => ("\u{f00c}", 0x72d69c_u32),
                            github_service::CheckStatus::Failure => ("\u{f00d}", 0xeb6f92_u32),
                            github_service::CheckStatus::Pending => ("\u{f192}", 0xe5c07b_u32),
                        };
                        checks_list = checks_list.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(div().text_xs().text_color(rgb(color)).child(icon))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(name.clone()),
                                ),
                        );
                    }
                    card = card.child(checks_list);
                }
            }
        }

        div().absolute().inset_0().child(
            div()
                .id("worktree-hover-popover-zone")
                .absolute()
                .left(popover_zone_bounds.origin.x)
                .top(popover_zone_bounds.origin.y)
                .p(px(WORKTREE_HOVER_POPOVER_ZONE_PADDING_PX))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                    this.update_worktree_hover_mouse_position(event.position);
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    this.update_worktree_hover_mouse_position(window.mouse_position());
                    if *hovered {
                        this.cancel_worktree_hover_popover_dismiss();
                    } else {
                        this.schedule_worktree_hover_popover_dismiss(popover_wt_index, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(card),
        )
    }

    fn render_outpost_context_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(menu) = self.outpost_context_menu.as_ref() else {
            return div();
        };

        let theme = self.theme();
        let index = menu.outpost_index;
        let position = menu.position;

        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.outpost_context_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.outpost_context_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                this.outpost_context_menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .absolute()
                    .left(position.x)
                    .top(position.y)
                    .w(px(180.))
                    .py(px(4.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.chrome_bg))
                    .on_mouse_move(|_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .id("outpost-context-delete")
                            .h(px(30.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(0x3a2030)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.outpost_context_menu = None;
                                let op_label = this
                                    .outposts
                                    .get(index)
                                    .map(|op| op.label.clone())
                                    .unwrap_or_default();
                                let op_branch = this
                                    .outposts
                                    .get(index)
                                    .map(|op| op.branch.clone())
                                    .unwrap_or_default();
                                this.open_delete_modal(
                                    DeleteTarget::Outpost(index),
                                    op_label,
                                    op_branch,
                                    cx,
                                );
                            }))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(16.))
                                    .text_color(rgb(0xeb6f92))
                                    .child("\u{f1f8}"),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(0xeb6f92))
                                    .child("Delete"),
                            ),
                    ),
            )
    }

    fn render_delete_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.delete_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let delete_worktree = match &modal.target {
            DeleteTarget::Worktree(index) => self.worktrees.get(*index),
            _ => None,
        };
        let is_worktree = delete_worktree.is_some();
        let is_discrete_clone = delete_worktree
            .is_some_and(|worktree| worktree.checkout_kind == CheckoutKind::DiscreteClone);
        let title = match &modal.target {
            DeleteTarget::Worktree(_) if is_discrete_clone => "Delete Discrete Clone",
            DeleteTarget::Worktree(_) => "Delete Worktree",
            DeleteTarget::Outpost(_) => "Remove Outpost",
            DeleteTarget::Repository(_) => "Remove Repository",
        };
        let label_prefix = match &modal.target {
            DeleteTarget::Worktree(_) if is_discrete_clone => "Discrete Clone",
            DeleteTarget::Worktree(_) => "Worktree",
            DeleteTarget::Outpost(_) => "Outpost",
            DeleteTarget::Repository(_) => "Repository",
        };
        let delete_disabled = modal.is_deleting;
        let delete_label = if modal.is_deleting {
            if is_worktree {
                "Deleting..."
            } else {
                "Removing..."
            }
        } else if is_worktree {
            "Delete"
        } else {
            "Remove"
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_delete_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_delete_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(440.))
                    .max_w(px(440.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
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
                                    .child(title),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "close-delete-modal",
                                    "Close",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_delete_modal(cx);
                                    })),
                            ),
                    )
                    // Label
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(format!("{}: {}", label_prefix, modal.label)),
                    )
                    // Unpushed commits warning (worktrees only)
                    .when(is_worktree, |this| {
                        match modal.has_unpushed {
                            None => this.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child("Checking for unpushed commits..."),
                            ),
                            Some(true) => this.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xe5c07b))
                                    .child("\u{f071} This worktree has unpushed commits that will be lost."),
                            ),
                            Some(false) => this,
                        }
                    })
                    // Branch deletion checkbox (worktrees only)
                    .when(is_worktree && !is_discrete_clone && !modal.branch.is_empty(), |this| {
                        this.child(
                            div()
                                .id("delete-branch-checkbox")
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                .flex()
                                .items_center()
                                .gap_2()
                                .py_1()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(modal) = this.delete_modal.as_mut() {
                                        modal.delete_branch = !modal.delete_branch;
                                        cx.notify();
                                    }
                                }))
                                .child(
                                    div()
                                        .w(px(14.))
                                        .h(px(14.))
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .when(modal.delete_branch, |this| {
                                            this.bg(rgb(theme.accent))
                                                .child(
                                                    div()
                                                        .font_family(FONT_MONO)
                                                        .text_size(px(10.))
                                                        .text_color(rgb(theme.sidebar_bg))
                                                        .child("\u{f00c}"),
                                                )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.text_primary))
                                        .child(format!("Also delete branch `{}`", modal.branch)),
                                ),
                        )
                    })
                    // Error display
                    .when_some(modal.error.clone(), |this, err| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0xeb6f92))
                                .child(err),
                        )
                    })
                    // Buttons
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "delete-cancel",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.close_delete_modal(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("delete-confirm")
                                    .cursor_pointer()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(0xeb6f92))
                                    .bg(rgb(theme.panel_bg))
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(rgb(0xeb6f92))
                                    .when(delete_disabled, |this| {
                                        this.opacity(0.5).cursor_default()
                                    })
                                    .when(!delete_disabled, |this| {
                                        this.hover(|s| s.bg(rgb(0xeb6f92)).text_color(rgb(theme.app_bg)))
                                    })
                                    .child(delete_label)
                                    .when(!delete_disabled, |this| {
                                        this.on_click(cx.listener(|this, _, _, cx| {
                                            this.execute_delete(cx);
                                        }))
                                    }),
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
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_manage_hosts_modal(cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.close_manage_hosts_modal(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(modal_backdrop())
                .child(
                    div()
                        .w(px(620.))
                        .max_w(px(620.))
                        .flex_none()
                        .overflow_hidden()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.sidebar_bg))
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .on_mouse_down(MouseButton::Right, |_, _, cx| {
                            cx.stop_propagation();
                        })
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
                                        ActionButtonStyle::Secondary,
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
                                modal.name_cursor,
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
                                modal.hostname_cursor,
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
                                modal.user_cursor,
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
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(
                                    action_button(
                                        theme,
                                        "cancel-add-host",
                                        "Cancel",
                                        ActionButtonStyle::Secondary,
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
                                        ActionButtonStyle::Primary,
                                        !add_disabled,
                                    )
                                    .when(!add_disabled, |this| {
                                        this.on_click(cx.listener(|this, _, _, cx| {
                                            this.submit_add_host(cx);
                                        }))
                                    }),
                                ),
                        ),
                );
        }

        // List view
        let hosts = self.remote_hosts.clone();
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_hosts_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_hosts_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(620.))
                    .max_w(px(620.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
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
                                    ActionButtonStyle::Secondary,
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
                                            ActionButtonStyle::Secondary,
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
                                    ActionButtonStyle::Primary,
                                    true,
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

    fn render_manage_presets_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.manage_presets_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let save_disabled = modal.command.trim().is_empty();
        let tab_button = |kind: AgentPresetKind| {
            let is_active = modal.active_preset == kind;
            let text_color = if is_active {
                theme.text_primary
            } else {
                theme.text_muted
            };
            div()
                .id(ElementId::Name(
                    format!("preset-modal-tab-{}", kind.key()).into(),
                ))
                .cursor_pointer()
                .px_3()
                .py_1()
                .flex()
                .items_center()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(text_color))
                .when(is_active, |this| {
                    this.border_b_2().border_color(rgb(theme.accent))
                })
                .hover(|s| {
                    s.bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(theme.text_primary))
                })
                .child(agent_preset_button_content(kind, text_color))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_manage_presets_modal_input(
                        PresetsModalInputEvent::SetActivePreset(kind),
                        cx,
                    );
                }))
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_presets_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_presets_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(620.))
                    .max_w(px(620.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div().flex().items_center().child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(theme.text_primary))
                                .child("Edit Agent Preset"),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_0()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .children(AgentPresetKind::ORDER.iter().copied().map(&tab_button)),
                    )
                    .child(
                        modal_input_field(
                            theme,
                            "preset-command-input",
                            "Command",
                            &modal.command,
                            modal.command_cursor,
                            modal.active_preset.default_command(),
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_manage_presets_modal_input(
                                PresetsModalInputEvent::ClearError,
                                cx,
                            );
                        })),
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
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "preset-restore-default",
                                    "Restore Default",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.update_manage_presets_modal_input(
                                            PresetsModalInputEvent::RestoreDefault,
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "preset-cancel",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_manage_presets_modal(cx);
                                    },
                                )),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "preset-save",
                                    "Save",
                                    ActionButtonStyle::Primary,
                                    !save_disabled,
                                )
                                .when(!save_disabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_manage_presets_modal(cx);
                                    }))
                                }),
                            ),
                    ),
            )
    }

    fn render_about_modal(&mut self, cx: &mut Context<Self>) -> Div {
        if !self.show_about {
            return div();
        }

        let theme = self.theme();
        let version = APP_VERSION;

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_about = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.show_about = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(340.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
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
                                    .child("About Arbor"),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "close-about",
                                    "Close",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.show_about = false;
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(theme.text_primary))
                                    .child(format!("Arbor {version}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child("Git worktree manager"),
                            ),
                    ),
            )
    }

    fn render_theme_picker_modal(&mut self, cx: &mut Context<Self>) -> Div {
        if !self.show_theme_picker {
            return div();
        }

        let theme = self.theme();
        let current_theme = self.theme_kind;

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_theme_picker = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.show_theme_picker = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(820.))
                    .max_h(px(600.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                    })
                    // Header
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Choose Theme"),
                    )
                    // Theme grid
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(ThemeKind::ALL.iter().enumerate().map(
                                |(idx, &kind)| {
                                let palette = kind.palette();
                                let is_active = kind == current_theme;
                                let border_color = if is_active {
                                    theme.accent
                                } else {
                                    theme.border
                                };
                                div()
                                    .id(("theme-card", idx))
                                    .w(px(148.))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(border_color))
                                    .when(is_active, |d| d.border_2())
                                    .bg(rgb(theme.panel_bg))
                                    .overflow_hidden()
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.85))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.switch_theme(kind, cx);
                                    }))
                                    // Color swatch strip
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .h(px(36.))
                                            .child(
                                                div().flex_1().bg(rgb(palette.app_bg)),
                                            )
                                            .child(
                                                div().flex_1().bg(rgb(palette.sidebar_bg)),
                                            )
                                            .child(
                                                div().flex_1().bg(rgb(palette.accent)),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .bg(rgb(palette.text_primary)),
                                            )
                                            .child(
                                                div().flex_1().bg(rgb(palette.border)),
                                            ),
                                    )
                                    // Theme name
                                    .child(
                                        div()
                                            .px_2()
                                            .py(px(6.))
                                            .text_xs()
                                            .text_color(rgb(theme.text_primary))
                                            .when(is_active, |d| {
                                                d.font_weight(FontWeight::SEMIBOLD)
                                            })
                                            .child(kind.label()),
                                    )
                            },
                            )),
                    ),
            )
    }

    fn render_daemon_auth_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(ref modal) = self.daemon_auth_modal else {
            return div();
        };
        let theme = self.theme();
        let token_value = modal.token.clone();
        let error = modal.error.clone();
        let daemon_url = modal.daemon_url.clone();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.daemon_auth_modal = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(420.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Authentication Required"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(format!(
                                "Enter the auth token for {daemon_url}. Find it in Settings (\u{2318},) on the remote host, or in ~/.config/arbor/config.toml under [daemon] auth_token."
                            )),
                    )
                    .when_some(error, |this, err| {
                        this.child(div().text_xs().text_color(rgb(0xf38ba8_u32)).child(err))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_muted))
                                    .child("Auth Token"),
                            )
                            .child(
                                div()
                                    .id("daemon-auth-token-field")
                                    .h(px(30.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(theme.accent))
                                    .bg(rgb(theme.panel_bg))
                                    .text_sm()
                                    .font_family(FONT_MONO)
                                    .text_color(rgb(theme.text_primary))
                                    .child(if token_value.is_empty() {
                                        active_input_display(
                                            theme,
                                            "",
                                            "paste token here",
                                            theme.text_disabled,
                                            modal.token_cursor,
                                            40,
                                        )
                                    } else {
                                        active_input_display(
                                            theme,
                                            &"\u{2022}".repeat(token_value.len()),
                                            "paste token here",
                                            theme.text_primary,
                                            modal.token_cursor,
                                            40,
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "cancel-auth",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.daemon_auth_modal = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "submit-auth",
                                    "Connect",
                                    ActionButtonStyle::Primary,
                                    true,
                                )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_daemon_auth(cx);
                                    })),
                            ),
                    ),
            )
    }

    fn render_start_daemon_modal(&mut self, cx: &mut Context<Self>) -> Div {
        if !self.start_daemon_modal {
            return div();
        }
        let theme = self.theme();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.start_daemon_modal = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(420.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Start Daemon"),
                    )
                    .child(div().text_xs().text_color(rgb(theme.text_muted)).child(
                        "The terminal daemon (arbor-httpd) is not running. \
                                 Start it to enable remote control and terminal persistence.",
                    ))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "cancel-start-daemon",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.start_daemon_modal = false;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "confirm-start-daemon",
                                    "Start",
                                    ActionButtonStyle::Primary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.start_daemon_modal = false;
                                        this.try_start_and_connect_daemon(cx);
                                    },
                                )),
                            ),
                    ),
            )
    }

    fn render_connect_to_host_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(ref modal) = self.connect_to_host_modal else {
            return div();
        };
        let theme = self.theme();
        let address = modal.address.clone();
        let address_empty = address.is_empty();
        let error = modal.error.clone();
        let history = self.connection_history.clone();
        let has_history = !history.is_empty();
        let has_daemons = !self.discovered_daemons.is_empty();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.connect_to_host_modal = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(420.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Title
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Connect to Host"),
                    )
                    // Recent section
                    .when(has_history, |modal_div| {
                        modal_div.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .font_family(FONT_MONO)
                                                .text_size(px(15.))
                                                .text_color(rgb(theme.text_muted))
                                                .child("\u{f1da}"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(theme.text_muted))
                                                .child("Recent"),
                                        ),
                                )
                                .children(history.into_iter().enumerate().map(|(idx, entry)| {
                                    let display = entry
                                        .label
                                        .clone()
                                        .unwrap_or_else(|| entry.address.clone());
                                    let subtitle = entry.address.clone();
                                    let has_label = entry.label.is_some();
                                    let connect_addr = entry.address.clone();
                                    let remove_addr = entry.address.clone();
                                    div()
                                        .id(("connect-modal-history", idx))
                                        .cursor_pointer()
                                        .px_2()
                                        .py_1()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .bg(rgb(theme.panel_bg))
                                        .hover(|s| s.bg(rgb(theme.panel_active_bg)))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(modal) =
                                                this.connect_to_host_modal.as_mut()
                                            {
                                                modal.address = connect_addr.clone();
                                            }
                                            this.submit_connect_to_host(cx);
                                        }))
                                        .child(
                                            div()
                                                .flex_none()
                                                .font_family(FONT_MONO)
                                                .text_size(px(14.))
                                                .text_color(rgb(theme.text_muted))
                                                .child("\u{f1da}"),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(theme.text_primary))
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .text_ellipsis()
                                                        .child(display),
                                                )
                                                .when(has_label, |d| {
                                                    d.child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(rgb(theme.text_muted))
                                                            .overflow_hidden()
                                                            .whitespace_nowrap()
                                                            .text_ellipsis()
                                                            .child(subtitle),
                                                    )
                                                }),
                                        )
                                        .child(
                                            div()
                                                .id(("connect-modal-history-remove", idx))
                                                .flex_none()
                                                .w(px(20.))
                                                .h(px(20.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded_sm()
                                                .cursor_pointer()
                                                .text_xs()
                                                .font_family(FONT_MONO)
                                                .text_color(rgb(theme.text_muted))
                                                .hover(|s| {
                                                    s.bg(rgb(theme.panel_active_bg))
                                                        .text_color(rgb(theme.text_primary))
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        connection_history::remove_entry(
                                                            &remove_addr,
                                                        );
                                                        this.daemon_auth_tokens
                                                            .retain(|k, _| !k.contains(&*remove_addr));
                                                        connection_history::save_tokens(
                                                            &this.daemon_auth_tokens,
                                                        );
                                                        this.connection_history =
                                                            connection_history::load_history();
                                                        cx.stop_propagation();
                                                        cx.notify();
                                                    },
                                                ))
                                                .child("\u{f00d}"),
                                        )
                                })),
                        )
                    })
                    // Discovered on LAN section
                    .when(has_daemons, |modal_div| {
                        let daemons = self.discovered_daemons.clone();
                        modal_div.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .font_family(FONT_MONO)
                                                .text_size(px(15.))
                                                .text_color(rgb(theme.text_muted))
                                                .child("\u{f0ac}"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(theme.text_muted))
                                                .child("Discovered on LAN"),
                                        ),
                                )
                                .children(daemons.into_iter().enumerate().map(|(idx, daemon)| {
                                    let display_name = daemon.display_name().to_owned();
                                    let addr = daemon
                                        .addresses
                                        .first()
                                        .cloned()
                                        .unwrap_or_else(|| daemon.host.clone());
                                    let subtitle = format!("{}:{}", addr, daemon.port);
                                    div()
                                        .id(("connect-modal-daemon", idx))
                                        .cursor_pointer()
                                        .px_2()
                                        .py_1()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .bg(rgb(theme.panel_bg))
                                        .hover(|s| s.bg(rgb(theme.panel_active_bg)))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.connect_to_host_modal = None;
                                            this.toggle_discovered_daemon(idx, cx);
                                        }))
                                        .child(
                                            div()
                                                .flex_none()
                                                .font_family(FONT_MONO)
                                                .text_size(px(14.))
                                                .text_color(rgb(theme.accent))
                                                .child("\u{f233}"),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(theme.text_primary))
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .text_ellipsis()
                                                        .child(display_name),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(theme.text_muted))
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .text_ellipsis()
                                                        .child(subtitle),
                                                ),
                                        )
                                })),
                        )
                    })
                    // Manual address label
                    .child(div().text_xs().text_color(rgb(theme.text_muted)).child(
                        if has_history || has_daemons {
                            "Or enter an address manually:"
                        } else {
                            "Use http://HOST:PORT or ssh://[user@]HOST[:ssh_port]/"
                        },
                    ))
                    // Error display
                    .when_some(error, |this, err| {
                        this.child(div().text_xs().text_color(rgb(0xf38ba8_u32)).child(err))
                    })
                    // Address input
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_muted))
                                    .child("Address"),
                            )
                            .child(
                                div()
                                    .id("connect-host-address-field")
                                    .h(px(30.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(theme.accent))
                                    .bg(rgb(theme.panel_bg))
                                    .text_sm()
                                    .font_family(FONT_MONO)
                                    .text_color(rgb(theme.text_primary))
                                    .child(if address_empty {
                                        active_input_display(
                                            theme,
                                            "",
                                            "ssh://dev@192.168.1.42/",
                                            theme.text_disabled,
                                            modal.address_cursor,
                                            42,
                                        )
                                    } else {
                                        active_input_display(
                                            theme,
                                            &address,
                                            "ssh://dev@192.168.1.42/",
                                            theme.text_primary,
                                            modal.address_cursor,
                                            42,
                                        )
                                    }),
                            ),
                    )
                    // Buttons
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "cancel-connect",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.connect_to_host_modal = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "submit-connect",
                                    "Connect",
                                    ActionButtonStyle::Primary,
                                    !address_empty,
                                )
                                .when(!address_empty, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_connect_to_host(cx);
                                    }))
                                }),
                            ),
                    ),
            )
    }

    fn render_settings_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.settings_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let daemon_auth_token_empty = modal.daemon_auth_token.trim().is_empty();
        let section_card = |this: Div| {
            this.rounded_sm()
                .border_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.panel_bg))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
        };
        let section_heading = |title: &str| {
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme.text_primary))
                .child(title.to_owned())
        };
        let bind_mode_button = |mode: DaemonBindMode, title: &str, detail: &str| {
            let selected = modal.daemon_bind_mode == mode;
            let active = modal.active_control == SettingsControl::DaemonBindMode;
            div()
                .flex_1()
                .min_w_0()
                .cursor_pointer()
                .rounded_sm()
                .border_1()
                .border_color(rgb(if selected || active {
                    theme.accent
                } else {
                    theme.border
                }))
                .bg(rgb(if selected {
                    theme.panel_active_bg
                } else {
                    theme.sidebar_bg
                }))
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap(px(3.))
                .hover(|style| style.opacity(0.92))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if selected {
                            theme.text_primary
                        } else {
                            theme.text_muted
                        }))
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_muted))
                        .child(detail.to_owned()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.update_settings_modal_input(
                            SettingsModalInputEvent::SelectDaemonBindMode(mode),
                            cx,
                        );
                    }),
                )
        };
        let daemon_helper_text = match modal.daemon_bind_mode {
            DaemonBindMode::Localhost => "Only this machine can connect to the daemon.",
            DaemonBindMode::AllInterfaces => {
                "Other Arbor instances can connect with your host IP and the token below."
            },
        };
        let notifications_enabled = modal.notifications;
        let notifications_active = modal.active_control == SettingsControl::Notifications;
        let notifications_toggle = div()
            .id("settings-notifications-toggle")
            .cursor_pointer()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(rgb(if notifications_active {
                theme.accent
            } else {
                theme.border
            }))
            .bg(rgb(if notifications_enabled {
                theme.accent
            } else {
                theme.sidebar_bg
            }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if notifications_enabled {
                theme.app_bg
            } else {
                theme.text_muted
            }))
            .hover(|s| s.opacity(0.85))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_settings_modal_input(SettingsModalInputEvent::ToggleNotifications, cx);
            }))
            .child(if notifications_enabled {
                "Enabled"
            } else {
                "Disabled"
            });

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_settings_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_settings_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(500.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Settings"),
                    )
                    .child(
                        section_card(div())
                            .child(section_heading("Daemon settings"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child("Choose who can reach this Arbor daemon."),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(bind_mode_button(
                                        DaemonBindMode::Localhost,
                                        "Localhost only",
                                        "Keep the daemon private to this machine.",
                                    ))
                                    .child(bind_mode_button(
                                        DaemonBindMode::AllInterfaces,
                                        "All interfaces",
                                        "Allow other machines to connect with a token.",
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child(daemon_helper_text),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(theme.text_muted))
                                            .child("Auth token"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .h(px(30.))
                                                    .px_2()
                                                    .flex()
                                                    .items_center()
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(rgb(theme.border))
                                                    .bg(rgb(theme.sidebar_bg))
                                                    .text_sm()
                                                    .font_family(FONT_MONO)
                                                    .text_color(rgb(theme.text_disabled))
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(if modal.daemon_auth_token.is_empty() {
                                                        "(not configured)".to_owned()
                                                    } else {
                                                        modal.daemon_auth_token.clone()
                                                    }),
                                            )
                                            .child(
                                                action_button(
                                                    theme,
                                                    "settings-copy-daemon-auth-token",
                                                    "Copy",
                                                    ActionButtonStyle::Secondary,
                                                    !daemon_auth_token_empty,
                                                )
                                                .when(!daemon_auth_token_empty, |this| {
                                                    this.on_click(cx.listener(|this, _, _, cx| {
                                                        this.copy_settings_daemon_auth_token_to_clipboard(cx);
                                                    }))
                                                }),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        section_card(div())
                            .child(section_heading("Notifications"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(theme.text_muted))
                                                    .child("Desktop notifications"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme.text_muted))
                                                    .child(
                                                        "Show notices for daemon status and background activity.",
                                                    ),
                                            ),
                                    )
                                    .child(notifications_toggle),
                            ),
                    )
                    .when_some(modal.error.clone(), |this, error| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.notice_text))
                                .bg(rgb(theme.notice_bg))
                                .rounded_sm()
                                .px_2()
                                .py_1()
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "settings-cancel",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_settings_modal(cx);
                                })),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "settings-save",
                                    "Save",
                                    ActionButtonStyle::Primary,
                                    true,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.submit_settings_modal(cx);
                                })),
                            ),
                    ),
            )
    }

    fn render_manage_repo_presets_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.manage_repo_presets_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let is_editing = modal.editing_index.is_some();
        let is_edit_tab = modal.active_tab == RepoPresetModalTab::Edit;
        let title = if is_editing {
            "Edit Custom Preset"
        } else {
            "Add Custom Preset"
        };
        let save_disabled = modal.name.trim().is_empty() || modal.command.trim().is_empty();
        let local_preset_path = self.active_arbor_toml_dir().join("arbor.toml");
        let local_preset_example = format!(
            "[[presets]]\nname = \"{}\"\nicon = \"{}\"\ncommand = \"{}\"",
            if modal.name.trim().is_empty() {
                "dev"
            } else {
                modal.name.trim()
            },
            if modal.icon.trim().is_empty() {
                "\u{f013}"
            } else {
                modal.icon.trim()
            },
            if modal.command.trim().is_empty() {
                "just run"
            } else {
                modal.command.trim()
            }
        );
        let tab_button = |tab: RepoPresetModalTab, label: &'static str| {
            let is_active = modal.active_tab == tab;
            div()
                .cursor_pointer()
                .px_3()
                .py_1()
                .flex()
                .items_center()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if is_active {
                    theme.text_primary
                } else {
                    theme.text_muted
                }))
                .when(is_active, |this| {
                    this.border_b_2().border_color(rgb(theme.accent))
                })
                .hover(|s| {
                    s.bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(theme.text_primary))
                })
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.update_manage_repo_presets_modal_input(
                            RepoPresetsModalInputEvent::SetActiveTab(tab),
                            cx,
                        );
                    }),
                )
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_repo_presets_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_repo_presets_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(620.))
                    .max_w(px(620.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div().flex().items_center().child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(theme.text_primary))
                                .child(title),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_0()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .child(tab_button(RepoPresetModalTab::Edit, "Edit"))
                            .child(tab_button(RepoPresetModalTab::LocalPreset, "Local Preset")),
                    )
                    .when(is_edit_tab, |this| {
                        this.child(
                            modal_input_field(
                                theme,
                                "repo-preset-icon-input",
                                "Icon (emoji)",
                                &modal.icon,
                                modal.icon_cursor,
                                "\u{f013}",
                                modal.active_field == RepoPresetModalField::Icon,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_repo_presets_modal_input(
                                    RepoPresetsModalInputEvent::SetActiveField(
                                        RepoPresetModalField::Icon,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "repo-preset-name-input",
                                "Name",
                                &modal.name,
                                modal.name_cursor,
                                "my preset",
                                modal.active_field == RepoPresetModalField::Name,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_repo_presets_modal_input(
                                    RepoPresetsModalInputEvent::SetActiveField(
                                        RepoPresetModalField::Name,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "repo-preset-command-input",
                                "Command",
                                &modal.command,
                                modal.command_cursor,
                                "just run",
                                modal.active_field == RepoPresetModalField::Command,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_repo_presets_modal_input(
                                    RepoPresetsModalInputEvent::SetActiveField(
                                        RepoPresetModalField::Command,
                                    ),
                                    cx,
                                );
                            })),
                        )
                    })
                    .when(!is_edit_tab, |this| {
                        this.child(
                            div()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(theme.border))
                                .bg(rgb(theme.panel_bg))
                                .p_3()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme.text_primary))
                                        .child("Add repo-local presets directly in `arbor.toml`."),
                                )
                                .child(div().text_xs().text_color(rgb(theme.text_muted)).child(
                                    format!(
                                        "Arbor reads local presets from {}",
                                        local_preset_path.display()
                                    ),
                                ))
                                .child(
                                    div()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .bg(rgb(theme.terminal_bg))
                                        .p_2()
                                        .font_family(FONT_MONO)
                                        .text_xs()
                                        .text_color(rgb(theme.text_primary))
                                        .children(
                                            local_preset_example
                                                .lines()
                                                .map(|line| div().child(line.to_owned())),
                                        ),
                                ),
                        )
                    })
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
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .when(is_edit_tab && is_editing, |this| {
                                this.child(
                                    action_button(
                                        theme,
                                        "repo-preset-new",
                                        "New Preset",
                                        ActionButtonStyle::Secondary,
                                        true,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.open_manage_repo_presets_modal(None, cx);
                                        },
                                    )),
                                )
                                .child(
                                    action_button(
                                        theme,
                                        "repo-preset-delete",
                                        "Delete",
                                        ActionButtonStyle::Secondary,
                                        true,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.delete_repo_preset(cx);
                                        },
                                    )),
                                )
                            })
                            .child(
                                action_button(
                                    theme,
                                    "repo-preset-cancel",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_manage_repo_presets_modal(cx);
                                    },
                                )),
                            )
                            .when(is_edit_tab, |this| {
                                this.child(
                                    action_button(
                                        theme,
                                        "repo-preset-save",
                                        "Save",
                                        ActionButtonStyle::Primary,
                                        !save_disabled,
                                    )
                                    .when(
                                        !save_disabled,
                                        |this| {
                                            this.on_click(cx.listener(|this, _, _, cx| {
                                                this.submit_manage_repo_presets_modal(cx);
                                            }))
                                        },
                                    ),
                                )
                            }),
                    ),
            )
    }
}

enum LaunchMode {
    Gui,
    Daemon { bind_addr: Option<String> },
    Help,
}

fn parse_launch_mode(args: impl IntoIterator<Item = String>) -> Result<LaunchMode, String> {
    let mut daemon_mode = false;
    let mut bind_addr: Option<String> = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--daemon" | "--daemon-only" | "daemon" => {
                daemon_mode = true;
            },
            "--bind" | "--daemon-bind" => {
                let Some(value) = args.next() else {
                    return Err(format!("missing value for `{arg}`"));
                };
                if value.trim().is_empty() {
                    return Err(format!("`{arg}` requires a non-empty address"));
                }
                bind_addr = Some(value);
            },
            "-h" | "--help" => return Ok(LaunchMode::Help),
            unknown => return Err(format!("unknown argument `{unknown}`")),
        }
    }

    if daemon_mode {
        Ok(LaunchMode::Daemon { bind_addr })
    } else {
        Ok(LaunchMode::Gui)
    }
}

fn daemon_cli_usage(program_name: &str) -> String {
    format!(
        "Usage:\n  {program_name}\n  {program_name} --daemon [--bind ADDR]\n\nExamples:\n  {program_name} --daemon\n  {program_name} --daemon --bind 0.0.0.0:8787"
    )
}

fn top_bar_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    enabled: bool,
    base_text_color: u32,
    hover_text_color: u32,
    content: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(22.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.chrome_bg))
        .text_color(rgb(base_text_color))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| {
                    this.bg(rgb(theme.panel_bg))
                        .text_color(rgb(hover_text_color))
                        .border_color(rgb(theme.panel_active_bg))
                })
                .active(|this| {
                    this.bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(hover_text_color))
                })
        })
        .child(content)
}

fn top_bar_icon_asset_path(kind: TopBarIconKind, tone: TopBarIconTone) -> Option<PathBuf> {
    let file_name = match (kind, tone) {
        (TopBarIconKind::RemoteControl, TopBarIconTone::Connected) => {
            "remote-control-connected.svg"
        },
        (TopBarIconKind::RemoteControl, TopBarIconTone::Disabled) => "remote-control-disabled.svg",
        (TopBarIconKind::GitHub, TopBarIconTone::Muted) => "github-muted.svg",
        (TopBarIconKind::GitHub, TopBarIconTone::Connected) => "github-connected.svg",
        (TopBarIconKind::GitHub, TopBarIconTone::Busy) => "github-busy.svg",
        (TopBarIconKind::WorktreeActions, TopBarIconTone::Muted) => "worktree-actions-enabled.svg",
        (TopBarIconKind::WorktreeActions, TopBarIconTone::Disabled) => {
            "worktree-actions-disabled.svg"
        },
        (TopBarIconKind::ReportIssue, TopBarIconTone::Muted) => "report-issue.svg",
        _ => return None,
    };

    find_top_bar_icons_dir().map(|dir| dir.join(file_name))
}

fn top_bar_icon_size_px(kind: TopBarIconKind) -> f32 {
    match kind {
        TopBarIconKind::GitHub => 10.5,
        TopBarIconKind::RemoteControl
        | TopBarIconKind::WorktreeActions
        | TopBarIconKind::ReportIssue => 12.0,
    }
}

fn top_bar_icon_element(
    kind: TopBarIconKind,
    tone: TopBarIconTone,
    fallback_color: u32,
    fallback_glyph: &'static str,
) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(match top_bar_icon_asset_path(kind, tone) {
            Some(path) => img(path)
                .size(px(top_bar_icon_size_px(kind)))
                .with_fallback(move || {
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .line_height(px(12.))
                        .text_color(rgb(fallback_color))
                        .child(fallback_glyph)
                        .into_any_element()
                })
                .into_any_element(),
            None => div()
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .line_height(px(12.))
                .text_color(rgb(fallback_color))
                .child(fallback_glyph)
                .into_any_element(),
        })
}

fn terminal_quick_action_icon_element(fallback_color: u32, size_px: f32) -> Div {
    div()
        .size(px(size_px))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            match find_ui_icons_dir().map(|dir| dir.join("terminal-accent.svg")) {
                Some(path) => img(path)
                    .size(px(size_px))
                    .with_fallback(move || {
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(size_px))
                            .line_height(px(size_px))
                            .text_color(rgb(fallback_color))
                            .child("\u{f120}")
                            .into_any_element()
                    })
                    .into_any_element(),
                None => div()
                    .font_family(FONT_MONO)
                    .text_size(px(size_px))
                    .line_height(px(size_px))
                    .text_color(rgb(fallback_color))
                    .child("\u{f120}")
                    .into_any_element(),
            },
        )
}

fn themed_ui_svg_icon(
    path: &'static str,
    color: u32,
    size_px: f32,
    fallback_glyph: &'static str,
) -> Div {
    div()
        .size(px(size_px))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(path)
                .size(px(size_px))
                .text_color(rgb(color))
                .into_any_element(),
        )
        .when(find_assets_root_dir().is_none(), |this| {
            this.child(
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(size_px))
                    .line_height(px(size_px))
                    .text_color(rgb(color))
                    .child(fallback_glyph),
            )
        })
}

fn terminal_tab_icon_element(color: u32, size_px: f32) -> Div {
    themed_ui_svg_icon("icons/ui/terminal-active.svg", color, size_px, "\u{f120}")
}

fn logs_tab_icon_element(color: u32, size_px: f32) -> Div {
    themed_ui_svg_icon("icons/ui/logs-active.svg", color, size_px, "\u{f4ed}")
}

fn run_daemon_mode(bind_addr: Option<String>) -> Result<(), String> {
    let binary = find_arbor_httpd_binary().ok_or_else(|| {
        "could not find `arbor-httpd` in PATH or next to the current executable".to_owned()
    })?;

    let mut command = Command::new(&binary);
    if let Some(path) = AUGMENTED_PATH.get() {
        command.env("PATH", path);
    }
    if let Some(bind_addr) = bind_addr {
        command.env("ARBOR_HTTPD_BIND", bind_addr);
    }

    let status = command.status().map_err(|error| {
        format!(
            "failed to start `{}`: {error}",
            binary
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("arbor-httpd")
        )
    })?;

    if status.success() {
        return Ok(());
    }

    Err(format!("arbor-httpd exited with status {status}"))
}

fn main() {
    let program_name = env::args().next().unwrap_or_else(|| "arbor".to_owned());
    let launch_mode = match parse_launch_mode(env::args().skip(1)) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("{error}\n\n{}", daemon_cli_usage(&program_name));
            std::process::exit(2);
        },
    };

    if matches!(launch_mode, LaunchMode::Help) {
        println!("{}", daemon_cli_usage(&program_name));
        return;
    }

    augment_path_from_login_shell();

    if let LaunchMode::Daemon { bind_addr } = launch_mode {
        if let Err(error) = run_daemon_mode(bind_addr) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }

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

    let assets_base = find_assets_root_dir()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"));

    Application::new()
        .with_assets(ArborAssets { base: assets_base })
        .run(move |cx: &mut App| {
            register_bundled_fonts(cx);
            set_dock_icon();
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
            DaemonTerminalRuntime, DaemonTerminalWsState, DiffLineKind, TerminalRuntimeHandle,
            TerminalRuntimeKind, TerminalSession, TerminalState, WorktreeHoverPopover,
            WorktreeSummary, apply_daemon_snapshot, auto_commit_body, auto_commit_subject,
            build_side_by_side_diff_lines,
            checkout::CheckoutKind,
            estimated_worktree_hover_popover_card_height, extract_first_url,
            resolve_github_access_token_from_sources, styled_lines_for_session,
            terminal_backend::{
                TerminalCursor, TerminalModes, TerminalStyledCell, TerminalStyledLine,
                TerminalStyledRun,
            },
            terminal_daemon_http::{HttpTerminalDaemon, WebsocketConnectConfig},
            theme::ThemeKind,
            track_terminal_command_keystroke, worktree_hover_popover_zone_bounds,
            worktree_hover_safe_zone_contains,
        },
        arbor_core::{
            agent::AgentState,
            changes::{ChangeKind, ChangedFile, DiffLineSummary},
            daemon,
        },
        gpui::{Keystroke, point, px},
        std::{sync::Arc, time::Instant},
    };

    fn session_with_styled_line(
        text: &str,
        fg: u32,
        bg: u32,
        cursor: Option<TerminalCursor>,
    ) -> TerminalSession {
        TerminalSession {
            id: 1,
            daemon_session_id: "daemon-test-1".into(),
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
            modes: TerminalModes::default(),
            last_runtime_sync_at: None,
            runtime: None,
        }
    }

    fn sample_worktree_summary() -> WorktreeSummary {
        WorktreeSummary {
            group_key: "/tmp/repo".to_owned(),
            checkout_kind: CheckoutKind::LinkedWorktree,
            repo_root: "/tmp/repo".into(),
            path: "/tmp/repo/wt".into(),
            label: "wt".to_owned(),
            branch: "feature/hover".to_owned(),
            is_primary_checkout: false,
            pr_number: None,
            pr_url: None,
            pr_details: None,
            diff_summary: Some(DiffLineSummary {
                additions: 3,
                deletions: 1,
            }),
            agent_state: Some(AgentState::Working),
            agent_task: Some("Investigating hover".to_owned()),
            last_activity_unix_ms: None,
        }
    }

    fn daemon_runtime_for_test() -> DaemonTerminalRuntime {
        let daemon = match HttpTerminalDaemon::new("http://127.0.0.1:8787") {
            Ok(daemon) => daemon,
            Err(error) => panic!("failed to create daemon client: {error}"),
        };

        DaemonTerminalRuntime {
            daemon: Arc::new(daemon),
            ws_state: Arc::new(DaemonTerminalWsState::default()),
            last_synced_ws_generation: std::sync::atomic::AtomicU64::new(0),
            kind: TerminalRuntimeKind::Local,
            resize_error_label: "resize",
            snapshot_error_label: "snapshot",
            exit_labels: None,
            clear_global_daemon_on_connection_refused: false,
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
    fn active_terminal_sync_is_prioritized() {
        let mut first = session_with_styled_line("one", 0xffffff, 0x000000, None);
        first.id = 10;
        let mut second = session_with_styled_line("two", 0xffffff, 0x000000, None);
        second.id = 20;
        let mut third = session_with_styled_line("three", 0xffffff, 0x000000, None);
        third.id = 30;

        let indices = crate::ordered_terminal_sync_indices(&[first, second, third], Some(30));

        assert_eq!(indices, vec![2, 0, 1]);
    }

    #[test]
    fn daemon_terminal_sync_interval_uses_active_fallback() {
        assert_eq!(
            crate::daemon_terminal_sync_interval(true, TerminalState::Running),
            crate::ACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL
        );
        assert_eq!(
            crate::daemon_terminal_sync_interval(false, TerminalState::Running),
            crate::INACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL
        );
        assert_eq!(
            crate::daemon_terminal_sync_interval(false, TerminalState::Completed),
            crate::IDLE_DAEMON_TERMINAL_SYNC_INTERVAL
        );
        assert_eq!(
            crate::daemon_terminal_sync_interval(false, TerminalState::Failed),
            crate::IDLE_DAEMON_TERMINAL_SYNC_INTERVAL
        );
    }

    #[test]
    fn daemon_runtime_syncs_active_session_immediately_on_ws_event() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        assert!(!runtime.should_sync(&session, true, None, now));

        runtime.ws_state.note_event();

        assert!(runtime.should_sync(&session, true, None, now));
    }

    #[test]
    fn daemon_runtime_throttles_inactive_sessions_even_when_ws_is_dirty() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("background", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        runtime.ws_state.note_event();

        assert!(!runtime.should_sync(&session, false, None, now));
        assert!(runtime.should_sync(
            &session,
            false,
            None,
            now + crate::INACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL
        ));
    }

    #[test]
    fn daemon_runtime_syncs_active_resize_without_waiting_for_ws() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        assert!(runtime.should_sync(
            &session,
            true,
            Some((session.rows + 1, session.cols, 0, 0)),
            now
        ));
    }

    #[test]
    fn daemon_websocket_request_adds_bearer_auth_header() {
        let request = match crate::daemon_websocket_request(&WebsocketConnectConfig {
            url: "ws://127.0.0.1:8787/api/v1/agent/activity/ws".to_owned(),
            auth_token: Some("secret-token".to_owned()),
        }) {
            Ok(request) => request,
            Err(error) => panic!("failed to build websocket request: {error}"),
        };

        assert_eq!(
            request
                .headers()
                .get(tungstenite::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret-token")
        );
    }

    #[test]
    fn worktree_hover_safe_zone_covers_trigger_row_and_popover() {
        let worktree = sample_worktree_summary();
        let popover = WorktreeHoverPopover {
            worktree_index: 0,
            mouse_y: px(100.),
            checks_expanded: false,
        };

        assert!(worktree_hover_safe_zone_contains(
            290.,
            &popover,
            &worktree,
            point(px(40.), px(100.)),
        ));
        assert!(worktree_hover_safe_zone_contains(
            290.,
            &popover,
            &worktree,
            point(px(320.), px(112.)),
        ));
        assert!(!worktree_hover_safe_zone_contains(
            290.,
            &popover,
            &worktree,
            point(px(700.), px(100.)),
        ));
    }

    #[test]
    fn expanded_checks_increase_worktree_hover_popover_height() {
        let mut worktree = sample_worktree_summary();
        worktree.pr_details = Some(crate::github_service::PrDetails {
            number: 42,
            title: "Improve hover stability".to_owned(),
            url: "https://example.com/pr/42".to_owned(),
            state: crate::github_service::PrState::Open,
            additions: 12,
            deletions: 4,
            review_decision: crate::github_service::ReviewDecision::Pending,
            checks_status: crate::github_service::CheckStatus::Pending,
            checks: vec![
                ("ci".to_owned(), crate::github_service::CheckStatus::Pending),
                (
                    "lint".to_owned(),
                    crate::github_service::CheckStatus::Success,
                ),
            ],
        });

        let collapsed = estimated_worktree_hover_popover_card_height(&worktree, false);
        let expanded = estimated_worktree_hover_popover_card_height(&worktree, true);
        let collapsed_bounds = worktree_hover_popover_zone_bounds(
            290.,
            &WorktreeHoverPopover {
                worktree_index: 0,
                mouse_y: px(120.),
                checks_expanded: false,
            },
            &worktree,
        );
        let expanded_bounds = worktree_hover_popover_zone_bounds(
            290.,
            &WorktreeHoverPopover {
                worktree_index: 0,
                mouse_y: px(120.),
                checks_expanded: true,
            },
            &worktree,
        );

        assert!(expanded > collapsed);
        assert!(expanded_bounds.size.height > collapsed_bounds.size.height);
    }

    #[test]
    fn shift_enter_does_not_submit_pending_terminal_command() {
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.pending_command = "hello".to_owned();

        track_terminal_command_keystroke(
            &mut session,
            &Keystroke::parse("shift-enter").expect("valid keystroke"),
        );

        assert_eq!(session.pending_command, "hello\n");
        assert_eq!(session.last_command, None);
    }

    #[test]
    fn daemon_snapshot_applies_structured_terminal_state() {
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.output.clear();
        session.styled_output.clear();
        session.cursor = None;
        session.modes = TerminalModes::default();

        let changed = apply_daemon_snapshot(&mut session, &daemon::TerminalSnapshot {
            session_id: "daemon-test-1".into(),
            output_tail: "READY".to_owned(),
            styled_lines: vec![daemon::DaemonTerminalStyledLine {
                cells: vec![daemon::DaemonTerminalStyledCell {
                    column: 0,
                    text: "READY".to_owned(),
                    fg: 0x123456,
                    bg: 0x654321,
                }],
                runs: vec![daemon::DaemonTerminalStyledRun {
                    text: "READY".to_owned(),
                    fg: 0x123456,
                    bg: 0x654321,
                }],
            }],
            cursor: Some(daemon::DaemonTerminalCursor { line: 0, column: 5 }),
            modes: daemon::DaemonTerminalModes {
                app_cursor: true,
                alt_screen: true,
            },
            exit_code: None,
            state: daemon::TerminalSessionState::Running,
            updated_at_unix_ms: Some(1),
        });

        assert!(changed);
        assert_eq!(session.output, "READY");
        assert_eq!(session.cursor, Some(TerminalCursor { line: 0, column: 5 }));
        assert_eq!(session.modes, TerminalModes {
            app_cursor: true,
            alt_screen: true,
        });
        assert_eq!(session.styled_output.len(), 1);
        assert_eq!(session.styled_output[0].runs[0].text, "READY");
        assert_eq!(session.styled_output[0].runs[0].fg, 0x123456);
        assert_eq!(session.styled_output[0].runs[0].bg, 0x654321);
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
    fn parse_theme_kind_supports_solarized_light_aliases() {
        assert_eq!(
            crate::parse_theme_kind(Some("solarized-light")).ok(),
            Some(ThemeKind::SolarizedLight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("solarized")).ok(),
            Some(ThemeKind::SolarizedLight)
        );
    }

    #[test]
    fn parse_theme_kind_supports_everforest_aliases() {
        assert_eq!(
            crate::parse_theme_kind(Some("everforest-dark")).ok(),
            Some(ThemeKind::Everforest)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("everforest")).ok(),
            Some(ThemeKind::Everforest)
        );
    }

    #[test]
    fn parse_theme_kind_supports_omarchy_and_custom_aliases() {
        assert_eq!(
            crate::parse_theme_kind(Some("catppuccin")).ok(),
            Some(ThemeKind::Catppuccin)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("catppuccin-latte")).ok(),
            Some(ThemeKind::CatppuccinLatte)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("ethereal")).ok(),
            Some(ThemeKind::Ethereal)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("flexoki-light")).ok(),
            Some(ThemeKind::FlexokiLight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("hackerman")).ok(),
            Some(ThemeKind::Hackerman)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("kanagawa")).ok(),
            Some(ThemeKind::Kanagawa)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("matte-black")).ok(),
            Some(ThemeKind::MatteBlack)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("miasma")).ok(),
            Some(ThemeKind::Miasma)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("nord")).ok(),
            Some(ThemeKind::Nord)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("osaka-jade")).ok(),
            Some(ThemeKind::OsakaJade)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("ristretto")).ok(),
            Some(ThemeKind::Ristretto)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("rose-pine")).ok(),
            Some(ThemeKind::RosePine)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokyo-night")).ok(),
            Some(ThemeKind::TokyoNight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("vantablack")).ok(),
            Some(ThemeKind::Vantablack)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("white")).ok(),
            Some(ThemeKind::White)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("retrobox-classic")).ok(),
            Some(ThemeKind::RetroboxClassic)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("retrobox")).ok(),
            Some(ThemeKind::RetroboxClassic)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokyonight-day")).ok(),
            Some(ThemeKind::TokyoNightDay)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokionight-day")).ok(),
            Some(ThemeKind::TokyoNightDay)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokyonight-classic")).ok(),
            Some(ThemeKind::TokyoNightClassic)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("tokionight-classic")).ok(),
            Some(ThemeKind::TokyoNightClassic)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("zellner")).ok(),
            Some(ThemeKind::Zellner)
        );
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

        let lines = styled_lines_for_session(&session, theme, true, None, None);
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

        let lines = styled_lines_for_session(&session, theme, true, None, None);
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

        let lines = styled_lines_for_session(&session, theme, false, None, None);
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
    fn github_token_resolution_prefers_saved_token() {
        let token =
            resolve_github_access_token_from_sources(Some(" saved-token "), Some("env-token"));
        assert_eq!(token.as_deref(), Some("saved-token"));
    }

    #[test]
    fn github_token_resolution_falls_back_to_environment_token() {
        let token = resolve_github_access_token_from_sources(Some(""), Some(" env-token "));
        assert_eq!(token.as_deref(), Some("env-token"));
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
    fn truncate_with_ellipsis_short_string_unchanged() {
        let result = crate::truncate_with_ellipsis("hello", 11);
        assert_eq!(result, "hello");
    }

    #[test]
    fn truncate_with_ellipsis_exact_limit_unchanged() {
        let result = crate::truncate_with_ellipsis("12345678901", 11);
        assert_eq!(result, "12345678901");
    }

    #[test]
    fn truncate_with_ellipsis_over_limit_adds_ellipsis() {
        let result = crate::truncate_with_ellipsis("123456789012", 11);
        assert_eq!(result, "1234567890\u{2026}");
        assert_eq!(result.chars().count(), 11);
    }

    #[test]
    fn truncate_with_ellipsis_tab_label_cases() {
        // These are the actual tab titles that need to show "…"
        let cases = [
            "nvim: CHANGELOG.md",
            "nvim: CLAUDE.md",
            "nvim: Cargo.lock",
            "nvim: Cargo.toml",
            "nvim: clippy.toml",
            "nvim: LICENSE",
            "nvim: AGENTS.md",
        ];
        for title in cases {
            let result = crate::truncate_with_ellipsis(title, 11);
            assert!(
                result.chars().count() <= 11,
                "'{result}' from '{title}' is {} chars, exceeds 11",
                result.chars().count()
            );
            if title.chars().count() > 11 {
                assert!(
                    result.ends_with('\u{2026}'),
                    "'{result}' from '{title}' should end with ellipsis"
                );
            }
        }
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

    #[test]
    fn parse_connect_host_target_normalizes_bare_http_host() {
        let target = crate::parse_connect_host_target("10.0.0.5")
            .expect("bare host should parse as http daemon target");

        match target {
            crate::ConnectHostTarget::Http { url, auth_key } => {
                assert_eq!(url, "http://10.0.0.5:8787");
                assert_eq!(auth_key, url);
            },
            crate::ConnectHostTarget::Ssh { .. } => panic!("expected http target"),
        }
    }

    #[test]
    fn parse_connect_host_target_supports_ssh_scheme() {
        let target = crate::parse_connect_host_target("ssh://dev@example.com:2222/9001")
            .expect("ssh address should parse");

        match target {
            crate::ConnectHostTarget::Ssh { target, auth_key } => {
                assert_eq!(target.user.as_deref(), Some("dev"));
                assert_eq!(target.host, "example.com");
                assert_eq!(target.ssh_port, 2222);
                assert_eq!(target.daemon_port, 9001);
                assert_eq!(auth_key, "ssh://dev@example.com:2222/9001");
            },
            crate::ConnectHostTarget::Http { .. } => panic!("expected ssh target"),
        }
    }

    #[test]
    fn parse_launch_mode_supports_daemon_bind() {
        let mode = crate::parse_launch_mode(vec![
            "--daemon".to_owned(),
            "--bind".to_owned(),
            "0.0.0.0:8787".to_owned(),
        ])
        .expect("daemon args should parse");

        match mode {
            crate::LaunchMode::Daemon { bind_addr } => {
                assert_eq!(bind_addr.as_deref(), Some("0.0.0.0:8787"));
            },
            crate::LaunchMode::Gui => panic!("expected daemon launch mode"),
            crate::LaunchMode::Help => panic!("expected daemon launch mode"),
        }
    }

    #[test]
    fn seed_repo_root_from_cwd_when_store_file_missing() {
        assert!(crate::should_seed_repo_root_from_cwd(false, false));
        assert!(crate::should_seed_repo_root_from_cwd(false, true));
    }

    #[test]
    fn does_not_seed_repo_root_from_cwd_when_store_is_explicitly_empty() {
        assert!(!crate::should_seed_repo_root_from_cwd(true, true));
    }

    #[test]
    fn seed_repo_root_from_cwd_when_store_has_saved_roots() {
        assert!(crate::should_seed_repo_root_from_cwd(true, false));
    }
}
