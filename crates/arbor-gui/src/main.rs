mod actions;
mod agent_activity;
mod agent_presets;
mod app_bootstrap;
mod app_config;
mod assets;
mod background_pollers;
mod center_panel;
mod changes_pane;
mod checkout;
mod command_palette;
mod config_refresh;
mod connection_history;
mod constants;
mod daemon_connection_ui;
mod daemon_runtime;
mod diff_engine;
mod diff_view;
mod error;
mod external_launchers;
mod file_view;
mod git_actions;
mod github_auth_modal;
mod github_auth_store;
mod github_helpers;
mod github_oauth;
mod github_pr_refresh;
mod github_service;
mod graphql;
mod helpers;
mod issue_cache_store;
mod issue_details_modal;
mod key_handling;
mod log_layer;
mod log_view;
mod manage_hosts;
mod mdns_browser;
mod notifications;
mod port_detection;
mod pr_summary_ui;
mod prompt_runner;
mod repo_presets;
mod repository_store;
mod settings_ui;
mod sidebar;
mod simple_http_client;
mod terminal_backend;
mod terminal_daemon_http;
mod terminal_interaction;
mod terminal_keys;
mod terminal_rendering;
mod terminal_session;
mod theme;
mod theme_picker;
mod top_bar;
mod types;
mod ui_state_store;
mod ui_widgets;
mod welcome_ui;
mod workspace_layout;
mod workspace_navigation;
mod worktree_lifecycle;
mod worktree_refresh;

pub(crate) use {
    actions::*, agent_activity::*, app_bootstrap::*, assets::*, config_refresh::*, constants::*,
    daemon_runtime::*, diff_engine::*, diff_view::*, error::*, external_launchers::*, file_view::*,
    git_actions::*, github_helpers::*, github_oauth::*, github_pr_refresh::*, helpers::*,
    issue_details_modal::*, port_detection::*, pr_summary_ui::*, prompt_runner::*, repo_presets::*,
    settings_ui::*, terminal_rendering::*, theme_picker::*, types::*, ui_widgets::*,
    workspace_layout::*, worktree_refresh::*,
};
use {
    arbor_core::{
        agent::AgentState,
        changes::{self, ChangeKind, ChangedFile},
        daemon::{
            self, CreateOrAttachRequest, DaemonSessionRecord, DetachRequest, KillRequest,
            ResizeRequest, SignalRequest, TerminalSessionState, TerminalSignal, WriteRequest,
        },
        process::{
            ProcessSource, managed_process_session_title,
            managed_process_source_and_name_from_title,
        },
        procfile, repo_config, worktree,
        worktree_scripts::{WorktreeScriptContext, WorktreeScriptPhase, run_worktree_scripts},
    },
    checkout::CheckoutKind,
    gix_diff::blob::v2::{
        Algorithm as DiffAlgorithm, Diff as BlobDiff, InternedInput as BlobInternedInput,
    },
    gpui::{
        Animation, AnimationExt, AnyElement, App, Application, Bounds, ClipboardItem, Context, Div,
        DragMoveEvent, ElementId, ElementInputHandler, EntityInputHandler, FocusHandle, FontWeight,
        KeyBinding, KeyDownEvent, Keystroke, Menu, MenuItem, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, PathPromptOptions, Pixels, ScrollHandle, ScrollStrategy,
        Stateful, SystemMenuType, TextRun, TitlebarOptions, UTF16Selection,
        UniformListScrollHandle, Window, WindowBounds, WindowControlArea, WindowOptions, canvas,
        div, ease_in_out, fill, img, point, prelude::*, px, rgb, size, uniform_list,
    },
    ropey::Rope,
    std::{
        collections::{HashMap, HashSet},
        env, fs,
        net::TcpListener,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant, SystemTime},
    },
    syntect::{easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet},
    terminal_backend::{
        EMBEDDED_TERMINAL_DEFAULT_BG, EMBEDDED_TERMINAL_DEFAULT_FG, EmbeddedTerminal,
        TerminalBackendKind, TerminalCursor, TerminalLaunch, TerminalModes, TerminalStyledCell,
        TerminalStyledLine, TerminalStyledRun,
    },
    theme::{ThemeKind, ThemePalette},
};

fn main() {
    use app_bootstrap::*;

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

    run_gui(log_buffer);
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
        Self::load(Arc::new(S::default()), startup_ui_state, log_buffer, cx)
    }

    fn load(
        daemon_session_store: Arc<dyn daemon::DaemonSessionStore>,
        startup_ui_state: ui_state_store::UiState,
        log_buffer: log_layer::LogBuffer,
        cx: &mut Context<Self>,
    ) -> Self {
        let app_config_store = app_config::default_app_config_store();
        let repository_store = repository_store::default_repository_store();
        let ui_state_store = ui_state_store::default_ui_state_store();
        let issue_cache_store = issue_cache_store::default_issue_cache_store();
        let github_auth_store = github_auth_store::default_github_auth_store();
        let github_service = github_service::default_github_service();
        let notification_service = notifications::default_notification_service();
        let loaded_github_auth_state = github_auth_store.load().map_err(|e| e.to_string());
        let loaded_issue_cache = issue_cache_store.load().map_err(|e| e.to_string());
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
                let startup_issue_cache = match loaded_issue_cache.clone() {
                    Ok(cache) => cache,
                    Err(error) => {
                        notice_parts.push(format!("failed to load issue cache: {error}"));
                        issue_cache_store::IssueCache::default()
                    },
                };

                let repositories = match repository_store.load_entries() {
                    Ok(entries) => repository_store::resolve_repositories_from_entries(entries),
                    Err(err) => {
                        notice_parts.push(format!("failed to load saved repositories: {err}"));
                        Vec::new()
                    },
                };
                let startup_repository_root = persisted_sidebar_selection_repository_root(
                    startup_ui_state.selected_sidebar_selection.as_ref(),
                );
                let active_repository_index = if let Some(root) = startup_repository_root.as_deref()
                {
                    repositories
                        .iter()
                        .position(|repository| repository.contains_checkout_root(root))
                        .or(Some(0))
                } else if repositories.is_empty() {
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
                let github_repo_slug = active_repository
                    .as_ref()
                    .and_then(|repository| repository.github_repo_slug.clone());

                let active_backend_kind = match parse_terminal_backend_kind(
                    loaded_config.config.terminal_backend.as_deref(),
                ) {
                    Ok(kind) => kind,
                    Err(err) => {
                        notice_parts.push(err.to_string());
                        TerminalBackendKind::Embedded
                    },
                };
                let embedded_terminal_engine = resolve_embedded_terminal_engine(
                    loaded_config.config.embedded_terminal_engine.as_deref(),
                    &mut notice_parts,
                );
                tracing::info!(
                    terminal_engine = embedded_terminal_engine.as_str(),
                    "configured embedded terminal engine",
                );
                let theme_kind = match parse_theme_kind(loaded_config.config.theme.as_deref()) {
                    Ok(kind) => kind,
                    Err(err) => {
                        notice_parts.push(err.to_string());
                        ThemeKind::One
                    },
                };
                let startup_sidebar_order = startup_ui_state.sidebar_order.clone();
                let repository_sidebar_tabs = startup_ui_state.repository_sidebar_tabs.clone();
                let startup_collapsed_repository_groups =
                    startup_ui_state.collapsed_repository_group_keys.clone();
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
                let outpost_store = Arc::new(arbor_core::outpost_store::default_outpost_store());
                let outposts = load_outpost_summaries(outpost_store.as_ref(), &remote_hosts);
                let active_outpost_index = persisted_sidebar_selection_outpost_index(
                    startup_ui_state.selected_sidebar_selection.as_ref(),
                    &outposts,
                );
                let startup_right_pane_tab =
                    right_pane_tab_from_persisted(startup_ui_state.right_pane_tab);
                let startup_logs_tab_open = persisted_logs_tab_open(&startup_ui_state);
                let startup_logs_tab_active = persisted_logs_tab_active(&startup_ui_state);
                let pending_startup_worktree_restore = matches!(
                    startup_ui_state.selected_sidebar_selection.as_ref(),
                    Some(ui_state_store::PersistedSidebarSelection::Worktree { .. })
                );
                let collapsed_repositories = collapsed_repository_indices_from_group_keys(
                    &repositories,
                    &startup_collapsed_repository_groups,
                );
                let startup_issue_lists =
                    issue_cache_store::issue_lists_from_cache(&repositories, &startup_issue_cache);
                let (terminal_poll_tx, terminal_poll_rx) = std::sync::mpsc::channel();

                let app = Self {
                    app_config_store,
                    repository_store,
                    daemon_session_store,
                    terminal_daemon: None,
                    daemon_base_url: DEFAULT_DAEMON_BASE_URL.to_owned(),
                    ui_state_store,
                    issue_cache_store,
                    github_auth_store,
                    github_service,
                    github_auth_state,
                    github_auth_in_progress: false,
                    github_auth_copy_feedback_active: false,
                    github_auth_copy_feedback_generation: 0,
                    next_create_modal_instance_id: 1,
                    config_last_modified,
                    repositories,
                    active_repository_index,
                    repo_root: active_repository
                        .as_ref()
                        .map(|repository| repository.root.clone())
                        .or(startup_repository_root)
                        .unwrap_or(repo_root),
                    github_repo_slug,
                    worktrees: Vec::new(),
                    worktree_stats_loading: false,
                    worktree_prs_loading: false,
                    pending_startup_worktree_restore,
                    loading_animation_active: false,
                    loading_animation_frame: 0,
                    github_rate_limited_until: None,
                    expanded_pr_checks_worktree: None,
                    active_worktree_index: None,
                    pending_local_worktree_selection: None,
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
                    issue_details_focus: cx.focus_handle(),
                    welcome_clone_focus: cx.focus_handle(),
                    terminal_scroll_handle: ScrollHandle::new(),
                    issue_details_scroll_handle: ScrollHandle::new(),
                    issue_details_scrollbar_drag_offset: None,
                    last_terminal_grid_size: None,
                    center_tabs_scroll_handle: ScrollHandle::new(),
                    diff_scroll_handle: UniformListScrollHandle::new(),
                    terminal_selection: None,
                    terminal_selection_drag_anchor: None,
                    create_modal: None,
                    issue_details_modal: None,
                    preferred_checkout_kind: startup_ui_state
                        .preferred_checkout_kind
                        .unwrap_or_default(),
                    github_auth_modal: None,
                    delete_modal: None,
                    commit_modal: None,
                    outposts,
                    outpost_store,
                    active_outpost_index,
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
                    theme_picker_selected_index: theme_picker_index_for_kind(theme_kind),
                    theme_picker_scroll_handle: ScrollHandle::new(),
                    settings_modal: None,
                    daemon_auth_modal: None,
                    pending_remote_daemon_auth: None,
                    pending_remote_create_repo_root: None,
                    start_daemon_modal: false,
                    connect_to_host_modal: None,
                    command_palette_modal: None,
                    command_palette_scroll_handle: ScrollHandle::new(),
                    command_palette_recent_actions: Vec::new(),
                    command_palette_task_templates: Vec::new(),
                    compact_sidebar: startup_ui_state.compact_sidebar.unwrap_or(false),
                    execution_mode: startup_ui_state
                        .execution_mode
                        .unwrap_or(ExecutionMode::Build),
                    connection_history: connection_history::load_history(),
                    connection_history_save: PendingSave::default(),
                    repository_entries_save: PendingSave::default(),
                    daemon_auth_tokens: connection_history::load_tokens(),
                    daemon_auth_tokens_save: PendingSave::default(),
                    github_auth_state_save: PendingSave::default(),
                    pending_app_config_save_count: 0,
                    connected_daemon_label: None,
                    daemon_connect_epoch: 0,
                    pending_diff_scroll_to_file: None,
                    focus_terminal_on_next_render: true,
                    git_action_in_flight: None,
                    top_bar_quick_actions_open: false,
                    top_bar_quick_actions_submenu: None,
                    ide_launchers: Vec::new(),
                    last_persisted_ui_state: startup_ui_state,
                    pending_ui_state_save: None,
                    ui_state_save_in_flight: None,
                    last_persisted_issue_cache: startup_issue_cache,
                    pending_issue_cache_save: None,
                    issue_cache_save_in_flight: None,
                    daemon_session_store_save: PendingSave::default(),
                    last_ui_state_error: None,
                    last_issue_cache_error: None,
                    notification_service,
                    notifications_enabled,
                    agent_activity_sessions: HashMap::new(),
                    last_agent_finished_notifications: HashMap::new(),
                    auto_checkpoint_in_flight: Arc::new(Mutex::new(HashSet::new())),
                    agent_activity_epochs: Arc::new(Mutex::new(HashMap::new())),
                    window_is_active: true,
                    notice: (!notice_parts.is_empty()).then_some(notice_parts.join(" | ")),
                    theme_toast: None,
                    theme_toast_generation: 0,
                    right_pane_tab: startup_right_pane_tab,
                    right_pane_search: String::new(),
                    right_pane_search_cursor: 0,
                    right_pane_search_active: false,
                    sidebar_order: startup_sidebar_order,
                    repository_sidebar_tabs,
                    issue_lists: startup_issue_lists,
                    worktree_notes_lines: vec![String::new()],
                    worktree_notes_cursor: FileViewCursor { line: 0, col: 0 },
                    worktree_notes_path: None,
                    worktree_notes_active: false,
                    worktree_notes_error: None,
                    worktree_notes_save_pending: false,
                    worktree_notes_edit_generation: 0,
                    _worktree_notes_save_task: None,
                    file_tree_entries: Vec::new(),
                    file_tree_loading: false,
                    expanded_dirs: HashSet::new(),
                    selected_file_tree_entry: None,
                    left_pane_visible: true,
                    collapsed_repositories,
                    repository_context_menu: None,
                    worktree_context_menu: None,
                    worktree_hover_popover: None,
                    _hover_show_task: None,
                    _hover_dismiss_task: None,
                    _worktree_refresh_task: None,
                    _changed_files_refresh_task: None,
                    _config_refresh_task: None,
                    _repo_metadata_refresh_task: None,
                    _launcher_refresh_task: None,
                    _connection_history_save_task: None,
                    _repository_entries_save_task: None,
                    _daemon_auth_tokens_save_task: None,
                    _github_auth_state_save_task: None,
                    _ui_state_save_task: None,
                    _issue_cache_save_task: None,
                    _daemon_session_store_save_task: None,
                    _create_modal_preview_task: None,
                    _file_tree_refresh_task: None,
                    worktree_refresh_epoch: 0,
                    config_refresh_epoch: 0,
                    repo_metadata_refresh_epoch: 0,
                    launcher_refresh_epoch: 0,
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
                    logs_tab_open: startup_logs_tab_open,
                    logs_tab_active: startup_logs_tab_active,
                    quit_overlay_until: None,
                    quit_after_persistence_flush: false,
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
        let startup_issue_cache = match loaded_issue_cache {
            Ok(cache) => cache,
            Err(error) => {
                notice_parts.push(format!("failed to load issue cache: {error}"));
                issue_cache_store::IssueCache::default()
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

        let startup_repository_root = persisted_sidebar_selection_repository_root(
            startup_ui_state.selected_sidebar_selection.as_ref(),
        );
        let preferred_repo_root = repo_root
            .clone()
            .or_else(|| startup_repository_root.clone());
        let active_repository_index = if let Some(ref root) = preferred_repo_root {
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

        let outpost_store = Arc::new(arbor_core::outpost_store::default_outpost_store());
        let outposts = load_outpost_summaries(outpost_store.as_ref(), &remote_hosts);
        let active_outpost_index = if repo_root.is_none() {
            persisted_sidebar_selection_outpost_index(
                startup_ui_state.selected_sidebar_selection.as_ref(),
                &outposts,
            )
        } else {
            None
        };

        let active_backend_kind =
            match parse_terminal_backend_kind(loaded_config.config.terminal_backend.as_deref()) {
                Ok(kind) => kind,
                Err(error) => {
                    notice_parts.push(error.to_string());
                    TerminalBackendKind::Embedded
                },
            };
        let embedded_terminal_engine = resolve_embedded_terminal_engine(
            loaded_config.config.embedded_terminal_engine.as_deref(),
            &mut notice_parts,
        );
        tracing::info!(
            terminal_engine = embedded_terminal_engine.as_str(),
            "configured embedded terminal engine",
        );
        let theme_kind = match parse_theme_kind(loaded_config.config.theme.as_deref()) {
            Ok(kind) => kind,
            Err(error) => {
                notice_parts.push(error.to_string());
                ThemeKind::One
            },
        };
        let startup_sidebar_order = startup_ui_state.sidebar_order.clone();
        let repository_sidebar_tabs = startup_ui_state.repository_sidebar_tabs.clone();
        let startup_collapsed_repository_groups =
            startup_ui_state.collapsed_repository_group_keys.clone();
        let configured_embedded_shell = loaded_config.config.embedded_shell.clone();
        let notifications_enabled = loaded_config.config.notifications.unwrap_or(true);
        let startup_right_pane_tab = right_pane_tab_from_persisted(startup_ui_state.right_pane_tab);
        let startup_logs_tab_open = persisted_logs_tab_open(&startup_ui_state);
        let startup_logs_tab_active = persisted_logs_tab_active(&startup_ui_state);
        let pending_startup_worktree_restore = matches!(
            startup_ui_state.selected_sidebar_selection.as_ref(),
            Some(ui_state_store::PersistedSidebarSelection::Worktree { .. })
        );
        let collapsed_repositories = collapsed_repository_indices_from_group_keys(
            &repositories,
            &startup_collapsed_repository_groups,
        );
        let startup_issue_lists =
            issue_cache_store::issue_lists_from_cache(&repositories, &startup_issue_cache);
        let (terminal_poll_tx, terminal_poll_rx) = std::sync::mpsc::channel();

        let mut app = Self {
            app_config_store,
            repository_store,
            daemon_session_store,
            terminal_daemon,
            daemon_base_url,
            ui_state_store,
            issue_cache_store,
            github_auth_store,
            github_service,
            github_auth_state,
            github_auth_in_progress: false,
            github_auth_copy_feedback_active: false,
            github_auth_copy_feedback_generation: 0,
            next_create_modal_instance_id: 1,
            config_last_modified,
            repositories,
            active_repository_index,
            repo_root: active_repository
                .as_ref()
                .map(|repository| repository.root.clone())
                .or(preferred_repo_root)
                .unwrap_or(cwd),
            github_repo_slug: active_repository.and_then(|repository| repository.github_repo_slug),
            worktrees: Vec::new(),
            worktree_stats_loading: false,
            worktree_prs_loading: false,
            pending_startup_worktree_restore,
            loading_animation_active: false,
            loading_animation_frame: 0,
            github_rate_limited_until: None,
            expanded_pr_checks_worktree: None,
            active_worktree_index: None,
            pending_local_worktree_selection: None,
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
            issue_details_focus: cx.focus_handle(),
            welcome_clone_focus: cx.focus_handle(),
            terminal_scroll_handle: ScrollHandle::new(),
            issue_details_scroll_handle: ScrollHandle::new(),
            issue_details_scrollbar_drag_offset: None,
            last_terminal_grid_size: None,
            center_tabs_scroll_handle: ScrollHandle::new(),
            diff_scroll_handle: UniformListScrollHandle::new(),
            terminal_selection: None,
            terminal_selection_drag_anchor: None,
            create_modal: None,
            issue_details_modal: None,
            preferred_checkout_kind: startup_ui_state.preferred_checkout_kind.unwrap_or_default(),
            github_auth_modal: None,
            delete_modal: None,
            commit_modal: None,
            outposts,
            outpost_store,
            active_outpost_index,
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
            theme_picker_selected_index: theme_picker_index_for_kind(theme_kind),
            theme_picker_scroll_handle: ScrollHandle::new(),
            settings_modal: None,
            daemon_auth_modal: None,
            pending_remote_daemon_auth: None,
            pending_remote_create_repo_root: None,
            start_daemon_modal: false,
            connect_to_host_modal: None,
            command_palette_modal: None,
            command_palette_scroll_handle: ScrollHandle::new(),
            command_palette_recent_actions: Vec::new(),
            command_palette_task_templates: Vec::new(),
            compact_sidebar: startup_ui_state.compact_sidebar.unwrap_or(false),
            execution_mode: startup_ui_state
                .execution_mode
                .unwrap_or(ExecutionMode::Build),
            connection_history: connection_history::load_history(),
            connection_history_save: PendingSave::default(),
            repository_entries_save: PendingSave::default(),
            daemon_auth_tokens: connection_history::load_tokens(),
            daemon_auth_tokens_save: PendingSave::default(),
            github_auth_state_save: PendingSave::default(),
            pending_app_config_save_count: 0,
            connected_daemon_label: None,
            daemon_connect_epoch: 0,
            pending_diff_scroll_to_file: None,
            focus_terminal_on_next_render: true,
            git_action_in_flight: None,
            top_bar_quick_actions_open: false,
            top_bar_quick_actions_submenu: None,
            ide_launchers: Vec::new(),
            left_pane_visible: startup_ui_state.left_pane_visible.unwrap_or(true),
            collapsed_repositories,
            repository_context_menu: None,
            worktree_context_menu: None,
            worktree_hover_popover: None,
            _hover_show_task: None,
            _hover_dismiss_task: None,
            _worktree_refresh_task: None,
            _changed_files_refresh_task: None,
            _config_refresh_task: None,
            _repo_metadata_refresh_task: None,
            _launcher_refresh_task: None,
            _connection_history_save_task: None,
            _repository_entries_save_task: None,
            _daemon_auth_tokens_save_task: None,
            _github_auth_state_save_task: None,
            _ui_state_save_task: None,
            _issue_cache_save_task: None,
            _daemon_session_store_save_task: None,
            _create_modal_preview_task: None,
            _file_tree_refresh_task: None,
            worktree_refresh_epoch: 0,
            config_refresh_epoch: 0,
            repo_metadata_refresh_epoch: 0,
            launcher_refresh_epoch: 0,
            last_mouse_position: point(px(0.), px(0.)),
            outpost_context_menu: None,
            discovered_daemons: Vec::new(),
            mdns_browser: None,
            active_discovered_daemon: None,
            worktree_nav_back: Vec::new(),
            worktree_nav_forward: Vec::new(),
            last_persisted_ui_state: startup_ui_state,
            pending_ui_state_save: None,
            ui_state_save_in_flight: None,
            last_persisted_issue_cache: startup_issue_cache,
            pending_issue_cache_save: None,
            issue_cache_save_in_flight: None,
            daemon_session_store_save: PendingSave::default(),
            last_ui_state_error: None,
            last_issue_cache_error: None,
            notification_service,
            notifications_enabled,
            agent_activity_sessions: HashMap::new(),
            last_agent_finished_notifications: HashMap::new(),
            auto_checkpoint_in_flight: Arc::new(Mutex::new(HashSet::new())),
            agent_activity_epochs: Arc::new(Mutex::new(HashMap::new())),
            window_is_active: true,
            notice: (!notice_parts.is_empty()).then_some(notice_parts.join(" | ")),
            theme_toast: None,
            theme_toast_generation: 0,
            right_pane_tab: startup_right_pane_tab,
            right_pane_search: String::new(),
            right_pane_search_cursor: 0,
            right_pane_search_active: false,
            sidebar_order: startup_sidebar_order,
            repository_sidebar_tabs,
            issue_lists: startup_issue_lists,
            worktree_notes_lines: vec![String::new()],
            worktree_notes_cursor: FileViewCursor { line: 0, col: 0 },
            worktree_notes_path: None,
            worktree_notes_active: false,
            worktree_notes_error: None,
            worktree_notes_save_pending: false,
            worktree_notes_edit_generation: 0,
            _worktree_notes_save_task: None,
            file_tree_entries: Vec::new(),
            file_tree_loading: false,
            expanded_dirs: HashSet::new(),
            selected_file_tree_entry: None,
            log_buffer,
            log_entries: Vec::new(),
            log_generation: 0,
            log_scroll_handle: ScrollHandle::new(),
            log_auto_scroll: true,
            logs_tab_open: startup_logs_tab_open,
            logs_tab_active: startup_logs_tab_active,
            quit_overlay_until: None,
            quit_after_persistence_flush: false,
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
        app.refresh_cached_issue_lists_on_startup(cx);
        app.refresh_repo_config_if_changed(cx);
        app.refresh_github_auth_identity(cx);
        app.restore_terminal_sessions_from_records(initial_daemon_records, attach_daemon_runtime);
        if app.active_outpost_index.is_some() {
            app.refresh_remote_changed_files(cx);
        } else {
            let _ = app.ensure_selected_worktree_terminal(cx);
        }
        app.sync_daemon_session_store(cx);
        app.start_terminal_poller(cx);
        app.start_log_poller(cx);
        app.start_worktree_auto_refresh(cx);
        app.start_github_pr_auto_refresh(cx);
        app.start_github_rate_limit_poller(cx);
        app.start_config_auto_refresh(cx);
        app.start_agent_activity_ws(cx);
        app.start_daemon_log_ws(cx);
        app.start_mdns_browser(cx);
        app.ensure_claude_code_hooks(cx);
        app.ensure_pi_agent_extension(cx);

        app
    }

    /// Returns the directory where repo preset edits should be saved.
    /// Prefers the selected worktree path, falls back to repo_root.
    fn active_arbor_toml_dir(&self) -> PathBuf {
        self.selected_worktree_path()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.repo_root.clone())
    }

    fn selected_agent_preset_or_default(&self) -> AgentPresetKind {
        self.active_preset_tab.unwrap_or(AgentPresetKind::Codex)
    }

    fn branch_prefix_github_login(&self) -> Option<String> {
        self.github_auth_state
            .user_login
            .clone()
            .or_else(|| env::var("ARBOR_GITHUB_USER").ok())
            .or_else(|| env::var("GITHUB_USER").ok())
            .and_then(|value| non_empty_trimmed_str(&value).map(str::to_owned))
    }

    fn maybe_finish_quit_after_persistence_flush(&mut self, cx: &mut Context<Self>) {
        if !self.quit_after_persistence_flush {
            return;
        }

        if self.daemon_session_store_save.has_work()
            || self.connection_history_save.has_work()
            || self.repository_entries_save.has_work()
            || self.daemon_auth_tokens_save.has_work()
            || self.github_auth_state_save.has_work()
            || background_config_save_has_work(self.pending_app_config_save_count)
            || ui_state_save_has_work(
                self.pending_ui_state_save.as_ref(),
                self.ui_state_save_in_flight.as_ref(),
            )
            || issue_cache_save_has_work(
                self.pending_issue_cache_save.as_ref(),
                self.issue_cache_save_in_flight.as_ref(),
            )
            || self.worktree_notes_save_pending
            || self._worktree_notes_save_task.is_some()
        {
            return;
        }

        self.quit_after_persistence_flush = false;
        self.stop_active_ssh_daemon_tunnel();
        cx.quit();
    }

    fn request_quit_after_persistence_flush(&mut self, cx: &mut Context<Self>) {
        self.quit_after_persistence_flush = true;
        self.sync_daemon_session_store(cx);
        self.maybe_finish_quit_after_persistence_flush(cx);
    }

    fn maybe_notify(&self, title: &str, body: &str, play_sound: bool) {
        if self.notifications_enabled && !self.window_is_active {
            self.notification_service.send(title, body, play_sound);
        }
    }

    fn maybe_notify_agent_finished(&mut self, worktree: &WorktreeSummary, updated_at: Option<u64>) {
        if !should_emit_agent_finished_notification(
            &mut self.last_agent_finished_notifications,
            &worktree.path,
            updated_at.or(worktree.last_activity_unix_ms),
        ) {
            return;
        }

        let repo_name = repository_display_name(&worktree.repo_root);
        let branch = worktree::short_branch(&worktree.branch);
        let body = if let Some(task) = worktree.agent_task.as_deref() {
            format!(
                "{} · {} · {} is waiting: {task}",
                repo_name, worktree.label, branch
            )
        } else {
            format!("{} · {} · {} is waiting", repo_name, worktree.label, branch)
        };
        self.maybe_notify("Agent finished", &body, true);
    }

    fn switch_theme(&mut self, theme_kind: ThemeKind, cx: &mut Context<Self>) {
        if self.theme_kind == theme_kind {
            return;
        }

        self.theme_kind = theme_kind;
        self.theme_picker_selected_index = theme_picker_index_for_kind(theme_kind);
        self.config_last_modified = None;
        let store = self.app_config_store.clone();
        let theme_slug = theme_kind.slug();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    store.save_scalar_settings(&[("theme", Some(theme_slug))])
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.notice = Some(format!("failed to save theme setting: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
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
}

fn is_command_in_path(command: &str) -> bool {
    use std::env;
    let path_var = env::var_os("PATH").unwrap_or_default();
    env::split_paths(&path_var).any(|dir| dir.join(command).is_file())
}

/// Return the set of `AgentPresetKind` variants whose CLI is found in PATH.
/// Cached for the lifetime of the process (the set of installed tools is
/// unlikely to change while the app is running).
fn installed_preset_kinds() -> &'static HashSet<AgentPresetKind> {
    static INSTALLED: OnceLock<HashSet<AgentPresetKind>> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        AgentPresetKind::ORDER
            .iter()
            .copied()
            .filter(|kind| kind.is_installed())
            .collect()
    })
}

fn default_agent_presets() -> Vec<AgentPreset> {
    AgentPresetKind::ORDER
        .iter()
        .copied()
        .map(|kind| AgentPreset {
            kind,
            command: kind.default_command().to_owned(),
        })
        .collect()
}

fn normalize_agent_presets(configured: &[app_config::AgentPresetConfig]) -> Vec<AgentPreset> {
    let mut presets = default_agent_presets();

    for configured_preset in configured {
        let Some(kind) = AgentPresetKind::from_key(&configured_preset.key) else {
            continue;
        };
        let command = configured_preset.command.trim();
        if command.is_empty() {
            continue;
        }
        if let Some(preset) = presets.iter_mut().find(|preset| preset.kind == kind) {
            preset.command = command.to_owned();
        }
    }

    presets
}

impl Drop for ArborWindow {
    fn drop(&mut self) {
        self.stop_active_ssh_daemon_tunnel();
        remove_claude_code_hooks();
        remove_pi_agent_extension();
    }
}

impl WorktreeSummary {
    fn from_worktree(
        entry: &worktree::Worktree,
        repo_root: &Path,
        group_key: &str,
        checkout_kind: CheckoutKind,
    ) -> Self {
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

        let last_activity_unix_ms = worktree::last_git_activity_ms(&entry.path);
        let managed_processes = managed_processes_for_worktree(repo_root, &entry.path);

        Self {
            group_key: group_key.to_owned(),
            checkout_kind,
            repo_root: repo_root.to_path_buf(),
            path: entry.path.clone(),
            label,
            branch,
            is_primary_checkout,
            pr_loading: false,
            pr_loaded: false,
            pr_number: None,
            pr_url: None,
            pr_details: None,
            branch_divergence: branch_divergence_summary(&entry.path),
            diff_summary: None,
            detected_ports: Vec::new(),
            managed_processes,
            recent_turns: Vec::new(),
            stuck_turn_count: 0,
            recent_agent_sessions: Vec::new(),
            agent_state: None,
            agent_task: None,
            last_activity_unix_ms,
        }
    }

    fn apply_cached_pull_request_state(&mut self, cached: &ui_state_store::CachedPullRequestState) {
        self.pr_loaded = true;
        self.pr_number = cached.number;
        self.pr_url = cached.url.clone();
        self.pr_details = cached.details.clone();
    }

    fn cached_pull_request_state(&self) -> Option<ui_state_store::CachedPullRequestState> {
        self.pr_loaded
            .then(|| ui_state_store::CachedPullRequestState {
                branch: self.branch.clone(),
                number: self.pr_number,
                url: self.pr_url.clone(),
                details: self.pr_details.clone(),
            })
    }
}

impl RepositorySummary {
    fn from_checkout_roots(
        root: PathBuf,
        group_key: String,
        checkout_roots: Vec<repository_store::RepositoryCheckoutRoot>,
    ) -> Self {
        let label = repository_display_name(&root);
        let github_repo_slug = github_repo_slug_for_repo(&root);
        let avatar_url = github_repo_slug
            .as_ref()
            .and_then(|repo_slug| github_avatar_url_for_repo_slug(repo_slug));

        Self {
            group_key,
            root,
            checkout_roots,
            label,
            avatar_url,
            github_repo_slug,
        }
    }

    fn contains_checkout_root(&self, root: &Path) -> bool {
        self.checkout_roots
            .iter()
            .any(|checkout_root| checkout_root.path == root)
    }
}

impl EntityInputHandler for ArborWindow {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.ime_marked_text.as_ref().map(|text| {
            let len: usize = text.encode_utf16().count();
            0..len
        })
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime_marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = None;
        if text.is_empty() {
            cx.notify();
            return;
        }
        // Suppress all text input while the quit overlay is showing.
        if self.quit_overlay_until.is_some() {
            return;
        }
        // When a modal with a text field is open, route IME text there instead
        if let Some(ref mut modal) = self.daemon_auth_modal {
            modal.token.push_str(text);
            modal.error = None;
            cx.notify();
            return;
        }
        if let Some(ref mut modal) = self.connect_to_host_modal {
            modal.address.push_str(text);
            modal.error = None;
            cx.notify();
            return;
        }
        if let Some(ref mut modal) = self.command_palette_modal {
            modal.query.push_str(text);
            modal.selected_index = 0;
            self.command_palette_scroll_handle.scroll_to_item(0);
            cx.notify();
            return;
        }
        if let Some(ref mut modal) = self.commit_modal {
            modal.message.push_str(text);
            modal.error = None;
            cx.notify();
            return;
        }
        if self.welcome_clone_url_active {
            self.welcome_clone_url.push_str(text);
            self.welcome_clone_error = None;
            cx.notify();
            return;
        }
        if self.worktree_notes_active && self.right_pane_tab == RightPaneTab::Notes {
            self.insert_text_into_selected_worktree_notes(text, cx);
            cx.notify();
            return;
        }
        let Some(session_id) = self.active_terminal_id_for_selected_worktree() else {
            return;
        };
        self.append_pasted_text_to_pending_command(session_id, text);
        if let Err(error) = self.write_input_to_terminal(session_id, text.as_bytes()) {
            self.notice = Some(format!("failed to write to terminal: {error}"));
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = if new_text.is_empty() {
            None
        } else {
            Some(new_text.to_owned())
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for ArborWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update window title to reflect connected daemon
        let title = app_window_title(self.connected_daemon_label.as_deref());
        window.set_window_title(&title);

        self.window_is_active = window.is_window_active();
        if self.focus_terminal_on_next_render && self.active_terminal().is_some() {
            window.focus(&self.terminal_focus);
            self.focus_terminal_on_next_render = false;
        }
        let workspace_width = f32::from(window.window_bounds().get_bounds().size.width);
        self.clamp_pane_widths_for_workspace(workspace_width);
        self.sync_ui_state_store(window, cx);

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
            .on_action(cx.listener(Self::action_open_manage_presets))
            .on_action(cx.listener(Self::action_open_manage_repo_presets))
            .on_action(cx.listener(Self::action_open_command_palette))
            .on_action(cx.listener(Self::action_refresh_worktrees))
            .on_action(cx.listener(Self::action_refresh_changes))
            .on_action(cx.listener(Self::action_open_add_repository))
            .on_action(cx.listener(Self::action_open_create_worktree))
            .on_action(cx.listener(Self::action_toggle_left_pane))
            .on_action(cx.listener(Self::action_navigate_worktree_back))
            .on_action(cx.listener(Self::action_navigate_worktree_forward))
            .on_action(cx.listener(Self::action_collapse_all_repositories))
            .on_action(cx.listener(Self::action_view_logs))
            .on_action(cx.listener(Self::action_show_about))
            .on_action(cx.listener(Self::action_open_theme_picker))
            .on_action(cx.listener(Self::action_open_settings))
            .on_action(cx.listener(Self::action_open_manage_hosts))
            .on_action(cx.listener(Self::action_connect_to_lan_daemon))
            .on_action(cx.listener(Self::action_connect_to_host))
            .on_action(cx.listener(Self::action_request_quit))
            .on_action(cx.listener(Self::action_immediate_quit))
            .child(self.render_top_bar(cx))
            .child(div().h(px(1.)).bg(rgb(theme.chrome_border)))
            .when(self.repositories.is_empty(), |this| {
                this.child(self.render_welcome_pane(cx))
            })
            .when(!self.repositories.is_empty(), |this| {
                this.child(
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
            })
            .child(self.render_status_bar())
            .child(self.render_top_bar_worktree_quick_actions_menu(cx))
            .child(self.render_notice_toast(cx))
            .child(self.render_issue_details_modal(cx))
            .child(self.render_create_modal(cx))
            .child(self.render_github_auth_modal(cx))
            .child(self.render_repository_context_menu(cx))
            .child(self.render_worktree_context_menu(cx))
            .child(self.render_worktree_hover_popover(cx))
            .child(self.render_outpost_context_menu(cx))
            .child(self.render_delete_modal(cx))
            .child(self.render_manage_hosts_modal(cx))
            .child(self.render_manage_presets_modal(cx))
            .child(self.render_manage_repo_presets_modal(cx))
            .child(self.render_commit_modal(cx))
            .child(self.render_command_palette_modal(cx))
            .child(self.render_about_modal(cx))
            .child(self.render_theme_picker_modal(cx))
            .child(self.render_settings_modal(cx))
            .child(self.render_daemon_auth_modal(cx))
            .child(self.render_start_daemon_modal(cx))
            .child(self.render_connect_to_host_modal(cx))
            .child(div().when_some(self.theme_toast.clone(), |this, toast| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_end()
                        .justify_end()
                        .px_3()
                        .pb(px(34.))
                        .child(
                            div()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(theme.accent))
                                .bg(rgb(theme.panel_active_bg))
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(rgb(theme.text_primary))
                                .child(toast),
                        ),
                )
            }))
            .when(self.quit_overlay_until.is_some(), |this| {
                this.child(
                    div()
                        .id("quit-backdrop")
                        .absolute()
                        .inset_0()
                        .bg(rgb(0x000000))
                        .opacity(0.5)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.action_dismiss_quit(window, cx);
                        })),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(
                            div()
                                .px_6()
                                .py_4()
                                .rounded_lg()
                                .bg(rgb(theme.chrome_bg))
                                .border_1()
                                .border_color(rgb(theme.border))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme.text_primary))
                                        .child("Are you sure you want to quit Arbor?"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            action_button(
                                                theme,
                                                "quit-cancel",
                                                "Cancel",
                                                ActionButtonStyle::Secondary,
                                                true,
                                            )
                                            .min_w(px(64.))
                                            .flex()
                                            .justify_center()
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.action_dismiss_quit(window, cx);
                                                }),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .id("quit-confirm")
                                                .cursor_pointer()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(0xc94040))
                                                .bg(rgb(0xc94040))
                                                .min_w(px(64.))
                                                .flex()
                                                .justify_center()
                                                .px_2()
                                                .py_1()
                                                .text_xs()
                                                .text_color(rgb(0xffffff))
                                                .child("Quit")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.action_confirm_quit(window, cx);
                                                })),
                                        ),
                                ),
                        ),
                )
            })
    }
}

fn managed_processes_for_worktree(
    repo_root: &Path,
    worktree_path: &Path,
) -> Vec<ManagedWorktreeProcess> {
    let mut processes = Vec::new();

    if paths_equivalent(repo_root, worktree_path) {
        processes.extend(arbor_toml_processes_for_worktree(repo_root, worktree_path));
    }
    processes.extend(procfile_processes_for_worktree(worktree_path));

    processes
}

fn arbor_toml_processes_for_worktree(
    repo_root: &Path,
    worktree_path: &Path,
) -> Vec<ManagedWorktreeProcess> {
    let Some(config) = repo_config::load_repo_config(repo_root) else {
        return Vec::new();
    };

    config
        .processes
        .into_iter()
        .filter(|process| !process.name.trim().is_empty() && !process.command.trim().is_empty())
        .map(|process| ManagedWorktreeProcess {
            id: managed_process_id(ProcessSource::ArborToml, worktree_path, &process.name),
            name: process.name,
            command: process.command,
            working_dir: process
                .working_dir
                .as_deref()
                .map(|dir| repo_root.join(dir))
                .unwrap_or_else(|| repo_root.to_path_buf()),
            source: ProcessSource::ArborToml,
        })
        .collect()
}

fn procfile_processes_for_worktree(worktree_path: &Path) -> Vec<ManagedWorktreeProcess> {
    match procfile::read_procfile(worktree_path) {
        Ok(Some(entries)) => entries
            .into_iter()
            .map(|entry| ManagedWorktreeProcess {
                id: managed_process_id(ProcessSource::Procfile, worktree_path, &entry.name),
                name: entry.name,
                command: entry.command,
                working_dir: worktree_path.to_path_buf(),
                source: ProcessSource::Procfile,
            })
            .collect(),
        Ok(None) => Vec::new(),
        Err(error) => {
            tracing::warn!(path = %worktree_path.display(), %error, "failed to read Procfile");
            Vec::new()
        },
    }
}

fn managed_process_id(source: ProcessSource, worktree_path: &Path, process_name: &str) -> String {
    format!(
        "{}:{}:{process_name}",
        managed_process_source_label(source),
        worktree_path.display()
    )
}

fn managed_process_source_label(source: ProcessSource) -> &'static str {
    match source {
        ProcessSource::ArborToml => "arbor-toml",
        ProcessSource::Procfile => "procfile",
    }
}

fn managed_process_source_display_name(source: ProcessSource) -> &'static str {
    match source {
        ProcessSource::ArborToml => "arbor.toml",
        ProcessSource::Procfile => "Procfile",
    }
}

fn managed_process_title(source: ProcessSource, process_name: &str) -> String {
    managed_process_session_title(source, process_name)
}

pub(crate) fn managed_process_id_from_title(worktree_path: &Path, title: &str) -> Option<String> {
    managed_process_source_and_name_from_title(title)
        .map(|(source, name)| managed_process_id(source, worktree_path, name))
}

fn managed_process_session_is_active(session: &TerminalSession) -> bool {
    session.is_initializing || session.state == TerminalState::Running
}

fn estimated_worktree_hover_popover_card_height(
    worktree: &WorktreeSummary,
    checks_expanded: bool,
) -> Pixels {
    let mut height = 72.;

    if worktree
        .diff_summary
        .is_some_and(|summary| summary.additions > 0 || summary.deletions > 0)
    {
        height += 18.;
    }

    height += 18.;

    if !worktree.recent_turns.is_empty() {
        height += 24. + worktree.recent_turns.iter().take(3).count() as f32 * 18.;
    }

    if !worktree.detected_ports.is_empty() {
        height += 22.;
    }

    if !worktree.recent_agent_sessions.is_empty() {
        let visible_sessions = worktree.recent_agent_sessions.iter().take(4);
        let provider_headers = visible_sessions
            .clone()
            .fold((None, 0usize), |(previous, count), session| {
                if previous == Some(session.provider) {
                    (previous, count)
                } else {
                    (Some(session.provider), count + 1)
                }
            })
            .1;
        height += 24.
            + worktree.recent_agent_sessions.iter().take(4).count() as f32 * 18.
            + provider_headers as f32 * 16.;
    }

    if let Some(pr) = worktree.pr_details.as_ref() {
        height += 110.;
        if checks_expanded
            && !pr.checks.is_empty()
            && matches!(
                pr.state,
                github_service::PrState::Open | github_service::PrState::Draft
            )
        {
            height += pr.checks.len() as f32 * 18.;
        }
    }

    px(height)
}

fn worktree_hover_popover_zone_bounds(
    left_pane_width: f32,
    popover: &WorktreeHoverPopover,
    worktree: &WorktreeSummary,
) -> Bounds<Pixels> {
    let padding = px(WORKTREE_HOVER_POPOVER_ZONE_PADDING_PX);
    Bounds::new(
        point(
            px(left_pane_width) + px(4.) - padding,
            popover.mouse_y - px(8.) - padding,
        ),
        size(
            px(WORKTREE_HOVER_POPOVER_CARD_WIDTH_PX) + padding * 2.,
            estimated_worktree_hover_popover_card_height(worktree, popover.checks_expanded)
                + padding * 2.,
        ),
    )
}

fn worktree_hover_trigger_zone_bounds(left_pane_width: f32, mouse_y: Pixels) -> Bounds<Pixels> {
    let height = px(WORKTREE_HOVER_TRIGGER_ZONE_HEIGHT_PX);
    Bounds::new(
        point(px(0.), mouse_y - height / 2.),
        size(px(left_pane_width), height),
    )
}

fn worktree_hover_safe_zone_contains(
    left_pane_width: f32,
    popover: &WorktreeHoverPopover,
    worktree: &WorktreeSummary,
    position: gpui::Point<Pixels>,
) -> bool {
    worktree_hover_popover_zone_bounds(left_pane_width, popover, worktree).contains(&position)
        || worktree_hover_trigger_zone_bounds(left_pane_width, popover.mouse_y).contains(&position)
}

fn terminal_tab_title(session: &TerminalSession) -> String {
    if let Some(last_command) = session
        .last_command
        .as_ref()
        .filter(|command| !command.trim().is_empty())
    {
        return truncate_with_ellipsis(last_command.trim(), TERMINAL_TAB_COMMAND_MAX_CHARS);
    }

    if !session.title.is_empty() && !session.title.starts_with("term-") {
        return truncate_with_ellipsis(&session.title, TERMINAL_TAB_COMMAND_MAX_CHARS);
    }

    String::new()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) mod tests {
    use {
        crate::{
            WorktreeHoverPopover, WorktreeSummary, checkout::CheckoutKind,
            daemon_runtime::session_with_styled_line, estimated_worktree_hover_popover_card_height,
            worktree_hover_popover_zone_bounds, worktree_hover_safe_zone_contains,
        },
        arbor_core::{agent::AgentState, changes::DiffLineSummary, process::ProcessSource},
        gpui::{point, px},
        std::{env, fs, path::Path, time::SystemTime},
    };

    pub(crate) fn sample_worktree_summary() -> WorktreeSummary {
        WorktreeSummary {
            group_key: "/tmp/repo".to_owned(),
            checkout_kind: CheckoutKind::LinkedWorktree,
            repo_root: "/tmp/repo".into(),
            path: "/tmp/repo/wt".into(),
            label: "wt".to_owned(),
            branch: "feature/hover".to_owned(),
            is_primary_checkout: false,
            pr_loading: false,
            pr_loaded: false,
            pr_number: None,
            pr_url: None,
            pr_details: None,
            branch_divergence: None,
            diff_summary: Some(DiffLineSummary {
                additions: 3,
                deletions: 1,
            }),
            detected_ports: vec![],
            managed_processes: vec![],
            recent_turns: vec![],
            stuck_turn_count: 0,
            recent_agent_sessions: vec![],
            agent_state: Some(AgentState::Working),
            agent_task: Some("Investigating hover".to_owned()),
            last_activity_unix_ms: None,
        }
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
            mergeable: crate::github_service::MergeableState::Mergeable,
            merge_state_status: crate::github_service::MergeStateStatus::Clean,
            passed_checks: 1,
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
    fn managed_process_title_round_trips_to_process_id() {
        let worktree_path = Path::new("/tmp/repo");
        assert_eq!(
            crate::managed_process_id_from_title(
                worktree_path,
                &crate::managed_process_title(ProcessSource::Procfile, "web"),
            ),
            Some(crate::managed_process_id(
                ProcessSource::Procfile,
                worktree_path,
                "web",
            ))
        );
        assert_eq!(
            crate::managed_process_id_from_title(
                worktree_path,
                &crate::managed_process_title(ProcessSource::ArborToml, "worker"),
            ),
            Some(crate::managed_process_id(
                ProcessSource::ArborToml,
                worktree_path,
                "worker",
            ))
        );
    }

    #[test]
    fn managed_processes_for_primary_worktree_include_arbor_toml_processes() {
        let unique_suffix = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos(),
            Err(error) => panic!("current time should be after the unix epoch: {error}"),
        };
        let repo_root = env::temp_dir().join(format!("arbor-managed-processes-{unique_suffix}"));
        let linked_worktree = repo_root.join("worktrees").join("feature");

        if let Err(error) = fs::create_dir_all(&linked_worktree) {
            panic!("linked worktree dir should be created: {error}");
        }
        if let Err(error) = fs::write(
            repo_root.join("arbor.toml"),
            "[[processes]]\nname = \"worker\"\ncommand = \"cargo run -- worker\"\nworking_dir = \"backend\"\n",
        ) {
            panic!("arbor.toml should be written: {error}");
        }

        let primary_processes = crate::managed_processes_for_worktree(&repo_root, &repo_root);
        assert!(primary_processes.iter().any(|process| {
            process.source == ProcessSource::ArborToml
                && process.name == "worker"
                && process.working_dir == repo_root.join("backend")
        }));

        let linked_processes = crate::managed_processes_for_worktree(&repo_root, &linked_worktree);
        assert!(
            !linked_processes
                .iter()
                .any(|process| process.source == ProcessSource::ArborToml)
        );

        if let Err(error) = fs::remove_dir_all(&repo_root) {
            panic!("temp repo root should be removed: {error}");
        }
    }

    #[test]
    fn completed_managed_process_sessions_are_not_active() {
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.managed_process_id = Some("procfile:/tmp/worktree:web".to_owned());
        session.is_initializing = false;
        session.state = crate::TerminalState::Completed;
        assert!(!crate::managed_process_session_is_active(&session));

        session.state = crate::TerminalState::Running;
        assert!(crate::managed_process_session_is_active(&session));

        session.is_initializing = true;
        session.state = crate::TerminalState::Completed;
        assert!(crate::managed_process_session_is_active(&session));
    }
}
