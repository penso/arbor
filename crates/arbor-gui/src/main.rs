mod actions;
mod agent_activity;
mod agent_presets;
mod app_config;
mod assets;
mod background_pollers;
mod center_panel;
mod changes_pane;
mod checkout;
mod command_palette;
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
mod welcome_ui;
mod workspace_layout;
mod workspace_navigation;
mod worktree_lifecycle;
mod worktree_refresh;

pub(crate) use {
    actions::*, agent_activity::*, assets::*, constants::*, daemon_runtime::*, diff_engine::*,
    diff_view::*, error::*, external_launchers::*, file_view::*, git_actions::*, github_helpers::*,
    github_oauth::*, github_pr_refresh::*, helpers::*, issue_details_modal::*, port_detection::*,
    pr_summary_ui::*, prompt_runner::*, repo_presets::*, settings_ui::*, terminal_rendering::*,
    theme_picker::*, types::*, workspace_layout::*, worktree_refresh::*,
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
        Image, ImageFormat, KeyBinding, KeyDownEvent, Keystroke, Menu, MenuItem, MouseButton,
        MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions, Pixels, ScrollHandle,
        ScrollStrategy, Stateful, SystemMenuType, TextRun, TitlebarOptions, UTF16Selection,
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

include!("app_bootstrap.rs");

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

    fn refresh_config_if_changed(&mut self, cx: &mut Context<Self>) {
        struct ConfigRefreshOutcome {
            next_modified: Option<SystemTime>,
            next_theme_kind: Option<ThemeKind>,
            next_backend_kind: Option<TerminalBackendKind>,
            next_embedded_shell: Option<String>,
            next_daemon_base_url: String,
            next_terminal_daemon: Option<terminal_daemon_http::SharedTerminalDaemonClient>,
            daemon_records: Option<Vec<DaemonSessionRecord>>,
            daemon_connection_refused: bool,
            remote_hosts: Vec<arbor_core::outpost::RemoteHost>,
            agent_presets: Vec<AgentPreset>,
            notifications_enabled: bool,
            notices: Vec<String>,
        }

        let store = self.app_config_store.clone();
        let current_modified = self.config_last_modified;
        let current_daemon = self.terminal_daemon.clone();
        let current_daemon_base_url = self.daemon_base_url.clone();
        let next_epoch = self.config_refresh_epoch.wrapping_add(1);
        self.config_refresh_epoch = next_epoch;
        self._config_refresh_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let next_modified = store.config_last_modified();
                    if next_modified == current_modified {
                        return None;
                    }

                    let loaded = store.load_or_create_config();
                    let mut notices = loaded.notices;

                    let next_theme_kind = match parse_theme_kind(loaded.config.theme.as_deref()) {
                        Ok(theme_kind) => Some(theme_kind),
                        Err(error) => {
                            notices.push(error.to_string());
                            None
                        },
                    };

                    let next_backend_kind =
                        match parse_terminal_backend_kind(loaded.config.terminal_backend.as_deref())
                        {
                            Ok(backend_kind) => Some(backend_kind),
                            Err(error) => {
                                notices.push(error.to_string());
                                None
                            },
                        };

                    let _ = resolve_embedded_terminal_engine(
                        loaded.config.embedded_terminal_engine.as_deref(),
                        &mut notices,
                    );

                    let next_daemon_base_url =
                        daemon_base_url_from_config(loaded.config.daemon_url.as_deref());
                    let daemon_url_changed = next_daemon_base_url != current_daemon_base_url;
                    if daemon_url_changed {
                        remove_claude_code_hooks();
                        remove_pi_agent_extension();
                    }

                    let next_terminal_daemon = if daemon_url_changed {
                        match terminal_daemon_http::default_terminal_daemon_client(
                            &next_daemon_base_url,
                        ) {
                            Ok(client) => Some(client),
                            Err(error) => {
                                notices.push(format!(
                                    "invalid daemon_url `{next_daemon_base_url}`: {error}"
                                ));
                                None
                            },
                        }
                    } else {
                        current_daemon.clone()
                    };

                    let mut daemon_records = None;
                    let mut daemon_connection_refused = false;
                    if let Some(daemon) = next_terminal_daemon.as_ref() {
                        match daemon.list_sessions() {
                            Ok(records) => daemon_records = Some(records),
                            Err(error) => {
                                let error_text = error.to_string();
                                daemon_connection_refused =
                                    daemon_error_is_connection_refused(&error_text);
                                if daemon_connection_refused {
                                    remove_claude_code_hooks();
                                    remove_pi_agent_extension();
                                }
                                if !daemon_connection_refused {
                                    notices.push(format!(
                                        "failed to list terminal sessions from daemon at {}: {error}",
                                        daemon.base_url()
                                    ));
                                }
                            },
                        }
                    }

                    let remote_hosts: Vec<arbor_core::outpost::RemoteHost> = loaded
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

                    Some(ConfigRefreshOutcome {
                        next_modified,
                        next_theme_kind,
                        next_backend_kind,
                        next_embedded_shell: loaded.config.embedded_shell.clone(),
                        next_daemon_base_url,
                        next_terminal_daemon,
                        daemon_records,
                        daemon_connection_refused,
                        remote_hosts,
                        agent_presets: normalize_agent_presets(&loaded.config.agent_presets),
                        notifications_enabled: loaded.config.notifications.unwrap_or(true),
                        notices,
                    })
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.config_refresh_epoch != next_epoch {
                    return;
                }
                let Some(outcome) = outcome else {
                    return;
                };

                this.config_last_modified = outcome.next_modified;
                let mut changed = false;

                if let Some(theme_kind) = outcome.next_theme_kind
                    && this.theme_kind != theme_kind
                {
                    this.theme_kind = theme_kind;
                    changed = true;
                }
                if let Some(backend_kind) = outcome.next_backend_kind
                    && this.active_backend_kind != backend_kind
                {
                    this.active_backend_kind = backend_kind;
                    changed = true;
                }
                if this.configured_embedded_shell != outcome.next_embedded_shell {
                    this.configured_embedded_shell = outcome.next_embedded_shell.clone();
                    changed = true;
                }
                if this.daemon_base_url != outcome.next_daemon_base_url {
                    this.daemon_base_url = outcome.next_daemon_base_url.clone();
                    changed = true;
                }

                if outcome.daemon_connection_refused {
                    this.terminal_daemon = None;
                    changed = true;
                } else if this.terminal_daemon.as_ref().map(|daemon| daemon.base_url())
                    != outcome
                        .next_terminal_daemon
                        .as_ref()
                        .map(|daemon| daemon.base_url())
                {
                    this.terminal_daemon = outcome.next_terminal_daemon.clone();
                    changed = true;
                } else {
                    this.terminal_daemon = outcome.next_terminal_daemon.clone();
                }

                if let Some(records) = outcome.daemon_records {
                    changed |= this.restore_terminal_sessions_from_records(records, true);
                }

                if this.remote_hosts != outcome.remote_hosts {
                    this.remote_hosts = outcome.remote_hosts;
                    this.outposts =
                        load_outpost_summaries(this.outpost_store.as_ref(), &this.remote_hosts);
                    changed = true;
                }

                if this.agent_presets != outcome.agent_presets {
                    this.agent_presets = outcome.agent_presets;
                    if let Some(modal) = this.manage_presets_modal.as_mut()
                        && let Some(preset) = this
                            .agent_presets
                            .iter()
                            .find(|preset| preset.kind == modal.active_preset)
                    {
                        modal.command = preset.command.clone();
                    }
                    changed = true;
                }

                if this.notifications_enabled != outcome.notifications_enabled {
                    this.notifications_enabled = outcome.notifications_enabled;
                    changed = true;
                }

                if !outcome.notices.is_empty() {
                    this.notice = Some(outcome.notices.join(" | "));
                    changed = true;
                }

                if changed {
                    cx.notify();
                }
            });
        }));
    }

    fn refresh_repo_config_if_changed(&mut self, cx: &mut Context<Self>) {
        let repo_root = self.repo_root.clone();
        let result_repo_root = repo_root.clone();
        let selected_worktree_path = self.selected_worktree_path().map(Path::to_path_buf);
        let repositories = self.repositories.clone();
        let store = self.app_config_store.clone();
        let next_epoch = self.repo_metadata_refresh_epoch.wrapping_add(1);
        self.repo_metadata_refresh_epoch = next_epoch;
        self._repo_metadata_refresh_task = Some(cx.spawn(async move |this, cx| {
            let (next_presets, next_default_preset, task_templates) = cx
                .background_spawn(async move {
                    let mut presets = load_repo_presets(store.as_ref(), &repo_root);
                    if let Some(worktree_path) = selected_worktree_path
                        .as_ref()
                        .filter(|worktree_path| *worktree_path != &repo_root)
                    {
                        for preset in load_repo_presets(store.as_ref(), worktree_path) {
                            if !presets
                                .iter()
                                .any(|candidate| candidate.name == preset.name)
                            {
                                presets.push(preset);
                            }
                        }
                    }
                    let default_preset = store
                        .load_repo_config(&repo_root)
                        .and_then(|config| config.agent.default_preset)
                        .and_then(|value| AgentPresetKind::from_key(&value));
                    let mut task_templates = Vec::new();
                    for repository in repositories {
                        task_templates.extend(load_task_templates_for_repo(&repository.root));
                    }
                    (presets, default_preset, task_templates)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.repo_metadata_refresh_epoch != next_epoch
                    || this.repo_root != result_repo_root
                {
                    return;
                }

                let mut changed = false;
                if this.repo_presets != next_presets {
                    this.repo_presets = next_presets;
                    changed = true;
                }
                if this.command_palette_task_templates != task_templates {
                    this.command_palette_task_templates = task_templates;
                    changed = true;
                }
                if this.active_preset_tab.is_none()
                    && let Some(preset) = next_default_preset
                {
                    this.active_preset_tab = Some(preset);
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            });
        }));
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

        if self.worktree_notes_active && self.right_pane_tab == RightPaneTab::Notes {
            if self.handle_worktree_notes_key_down(event, cx) {
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

        if self.command_palette_modal.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.close_command_palette(cx);
                    cx.stop_propagation();
                    return;
                },
                "enter" | "return" => {
                    self.execute_command_palette_selection(window, cx);
                    cx.stop_propagation();
                    return;
                },
                "up" => {
                    self.move_command_palette_selection(-1, cx);
                    cx.stop_propagation();
                    return;
                },
                "down" => {
                    self.move_command_palette_selection(1, cx);
                    cx.stop_propagation();
                    return;
                },
                _ => {},
            }

            if let Some(action) = text_edit_action_for_event(event, cx) {
                if let Some(modal) = self.command_palette_modal.as_mut() {
                    apply_text_edit_action(&mut modal.query, &mut modal.query_cursor, &action);
                    modal.selected_index = 0;
                }
                self.command_palette_scroll_handle.scroll_to_item(0);
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }

        if self.show_theme_picker {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.show_theme_picker = false;
                    cx.stop_propagation();
                    cx.notify();
                },
                "left" => {
                    self.move_theme_picker_selection(-1, cx);
                    cx.stop_propagation();
                },
                "right" => {
                    self.move_theme_picker_selection(1, cx);
                    cx.stop_propagation();
                },
                "up" => {
                    self.move_theme_picker_selection(
                        -(theme_picker_columns(ThemeKind::ALL.len()) as isize),
                        cx,
                    );
                    cx.stop_propagation();
                },
                "down" => {
                    self.move_theme_picker_selection(
                        theme_picker_columns(ThemeKind::ALL.len()) as isize,
                        cx,
                    );
                    cx.stop_propagation();
                },
                "enter" | "return" => {
                    self.apply_selected_theme_picker_theme(cx);
                    cx.stop_propagation();
                },
                _ => {},
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

        if self.commit_modal.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.close_commit_modal(cx);
                    cx.stop_propagation();
                    return;
                },
                "enter" | "return" => {
                    self.submit_commit_modal(cx);
                    cx.stop_propagation();
                    return;
                },
                _ => {},
            }

            if let Some(action) = text_edit_action_for_event(event, cx) {
                if let Some(modal) = self.commit_modal.as_mut() {
                    apply_text_edit_action(&mut modal.message, &mut modal.message_cursor, &action);
                    modal.error = None;
                }
                cx.notify();
                cx.stop_propagation();
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

        if self.issue_details_modal.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => {
                    self.close_issue_details_modal(Some(window), cx);
                    cx.stop_propagation();
                },
                "enter" | "return" => {
                    self.open_create_modal_from_issue_details(cx);
                    cx.stop_propagation();
                },
                _ => {},
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
                    CreateModalTab::ReviewPullRequest => {
                        self.update_create_review_pr_modal_input(
                            ReviewPrModalInputEvent::MoveActiveField,
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
                        CreateModalTab::ReviewPullRequest => self.submit_create_review_pr_modal(cx),
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
                CreateModalTab::ReviewPullRequest => {
                    self.update_create_review_pr_modal_input(
                        ReviewPrModalInputEvent::ClearError,
                        cx,
                    );
                    self.update_create_review_pr_modal_input(
                        ReviewPrModalInputEvent::Edit(action),
                        cx,
                    );
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

    fn action_open_command_palette(
        &mut self,
        _: &OpenCommandPalette,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_palette(cx);
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
        self.refresh_changed_files(cx);
        cx.notify();
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
            self.refresh_changed_files(cx);
            if self.ensure_selected_worktree_terminal(cx) {
                self.sync_daemon_session_store(cx);
            }
            self.sync_navigation_ui_state_store(cx);
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
            self.refresh_changed_files(cx);
            if self.ensure_selected_worktree_terminal(cx) {
                self.sync_daemon_session_store(cx);
            }
            self.sync_navigation_ui_state_store(cx);
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
        self.sync_collapsed_repositories_store(cx);
        cx.notify();
    }

    fn action_request_quit(&mut self, _: &RequestQuit, _: &mut Window, cx: &mut Context<Self>) {
        self.quit_overlay_until = if self.quit_overlay_until.is_some() {
            self.quit_after_persistence_flush = false;
            None
        } else {
            Some(Instant::now())
        };
        cx.notify();
    }

    fn action_confirm_quit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.request_quit_after_persistence_flush(cx);
    }

    fn action_dismiss_quit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.quit_overlay_until = None;
        self.quit_after_persistence_flush = false;
        cx.notify();
    }

    fn action_immediate_quit(&mut self, _: &ImmediateQuit, _: &mut Window, cx: &mut Context<Self>) {
        self.request_quit_after_persistence_flush(cx);
    }

    fn action_view_logs(&mut self, _: &ViewLogs, _: &mut Window, cx: &mut Context<Self>) {
        self.logs_tab_open = true;
        self.logs_tab_active = true;
        self.active_diff_session_id = None;
        self.sync_navigation_ui_state_store(cx);
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
        self.open_theme_picker_modal(cx);
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

fn loading_status_text(theme: ThemePalette, text: impl Into<String>) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme.accent))
        .child(text.into())
}

fn loading_spinner_frame(frame: usize) -> &'static str {
    LOADING_SPINNER_FRAMES[frame % LOADING_SPINNER_FRAMES.len()]
}

fn action_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    style: ActionButtonStyle,
    enabled: bool,
) -> Stateful<Div> {
    let background = if enabled && style == ActionButtonStyle::Primary {
        theme.panel_active_bg
    } else {
        theme.panel_bg
    };
    let text_color = if enabled {
        theme.text_primary
    } else {
        theme.text_disabled
    };

    div()
        .id(id)
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
        })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionButtonStyle {
    Primary,
    Secondary,
}

fn preset_icon_image(kind: AgentPresetKind) -> Arc<Image> {
    static CLAUDE_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static CODEX_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static PI_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static OPENCODE_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static COPILOT_ICON: OnceLock<Arc<Image>> = OnceLock::new();

    let lock = match kind {
        AgentPresetKind::Codex => &CODEX_ICON,
        AgentPresetKind::Claude => &CLAUDE_ICON,
        AgentPresetKind::Pi => &PI_ICON,
        AgentPresetKind::OpenCode => &OPENCODE_ICON,
        AgentPresetKind::Copilot => &COPILOT_ICON,
    };

    lock.get_or_init(|| {
        tracing::info!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            bytes = preset_icon_bytes(kind).len(),
            "loading preset icon asset"
        );
        Arc::new(Image::from_bytes(
            preset_icon_format(kind),
            preset_icon_bytes(kind).to_vec(),
        ))
    })
    .clone()
}

fn preset_icon_bytes(kind: AgentPresetKind) -> &'static [u8] {
    match kind {
        AgentPresetKind::Codex => PRESET_ICON_CODEX_SVG,
        AgentPresetKind::Claude => PRESET_ICON_CLAUDE_PNG,
        AgentPresetKind::Pi => PRESET_ICON_PI_SVG,
        AgentPresetKind::OpenCode => PRESET_ICON_OPENCODE_SVG,
        AgentPresetKind::Copilot => PRESET_ICON_COPILOT_SVG,
    }
}

fn preset_icon_format(kind: AgentPresetKind) -> ImageFormat {
    match kind {
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => ImageFormat::Svg,
        AgentPresetKind::Claude => ImageFormat::Png,
    }
}

fn preset_icon_asset_path(kind: AgentPresetKind) -> &'static str {
    match kind {
        AgentPresetKind::Codex => "assets/preset-icons/codex-white.svg",
        AgentPresetKind::Claude => "assets/preset-icons/claude.png",
        AgentPresetKind::Pi => "assets/preset-icons/pi-white.svg",
        AgentPresetKind::OpenCode => "assets/preset-icons/opencode-white.svg",
        AgentPresetKind::Copilot => "assets/preset-icons/copilot-white.svg",
    }
}

fn log_preset_icon_fallback_once(kind: AgentPresetKind, fallback_glyph: &'static str) {
    static CLAUDE_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static CODEX_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static PI_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static OPENCODE_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static COPILOT_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();

    let once = match kind {
        AgentPresetKind::Codex => &CODEX_FALLBACK_LOGGED,
        AgentPresetKind::Claude => &CLAUDE_FALLBACK_LOGGED,
        AgentPresetKind::Pi => &PI_FALLBACK_LOGGED,
        AgentPresetKind::OpenCode => &OPENCODE_FALLBACK_LOGGED,
        AgentPresetKind::Copilot => &COPILOT_FALLBACK_LOGGED,
    };

    once.get_or_init(|| {
        tracing::warn!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            bytes = preset_icon_bytes(kind).len(),
            fallback = fallback_glyph,
            "preset icon asset could not be rendered, using fallback glyph"
        );
        eprintln!(
            "WARN preset icon fallback preset={} asset={} bytes={} fallback={}",
            kind.key(),
            preset_icon_asset_path(kind),
            preset_icon_bytes(kind).len(),
            fallback_glyph
        );
    });
}

fn log_preset_icon_render_once(kind: AgentPresetKind) {
    static CLAUDE_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static CODEX_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static PI_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static OPENCODE_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static COPILOT_RENDER_LOGGED: OnceLock<()> = OnceLock::new();

    let once = match kind {
        AgentPresetKind::Codex => &CODEX_RENDER_LOGGED,
        AgentPresetKind::Claude => &CLAUDE_RENDER_LOGGED,
        AgentPresetKind::Pi => &PI_RENDER_LOGGED,
        AgentPresetKind::OpenCode => &OPENCODE_RENDER_LOGGED,
        AgentPresetKind::Copilot => &COPILOT_RENDER_LOGGED,
    };

    once.get_or_init(|| {
        tracing::info!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            "preset icon render path active"
        );
    });
}

fn preset_icon_render_size_px(kind: AgentPresetKind) -> f32 {
    match kind {
        AgentPresetKind::Codex => 20.,
        AgentPresetKind::Claude
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => 14.,
    }
}

fn agent_preset_button_content(kind: AgentPresetKind, text_color: u32) -> Div {
    log_preset_icon_render_once(kind);
    let icon = preset_icon_image(kind);
    let icon_size = preset_icon_render_size_px(kind);
    // Use consistent slot size for all icons to ensure vertical alignment
    let icon_slot_size = 20_f32;
    let fallback_color = match kind {
        AgentPresetKind::Claude => 0xD97757,
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => text_color,
    };
    let fallback_glyph = match kind {
        AgentPresetKind::Claude => "C",
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => kind.fallback_icon(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .w(px(icon_slot_size))
                .h(px(icon_slot_size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(img(icon).size(px(icon_size)).with_fallback(move || {
                    log_preset_icon_fallback_once(kind, fallback_glyph);
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .line_height(px(12.))
                        .text_color(rgb(fallback_color))
                        .child(fallback_glyph)
                        .into_any_element()
                })),
        )
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(14.))
                .text_color(rgb(text_color))
                .child(kind.label()),
        )
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
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
        })
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(px(13.))
                .text_color(rgb(icon_color))
                .child(icon),
        )
        .child(div().text_xs().text_color(rgb(text_color)).child(label))
}

fn modal_backdrop() -> Div {
    div().absolute().inset_0().bg(rgb(0x000000)).opacity(0.28)
}

fn modal_input_field(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    value: &str,
    cursor: usize,
    placeholder: impl Into<String>,
    active: bool,
) -> Stateful<Div> {
    let label = label.into();
    let placeholder = placeholder.into();

    div()
        .id(id)
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(
            div()
                .overflow_hidden()
                .cursor_pointer()
                .rounded_sm()
                .border_1()
                .border_color(rgb(if active {
                    theme.accent
                } else {
                    theme.border
                }))
                .bg(rgb(theme.panel_bg))
                .px_2()
                .py_1()
                .text_sm()
                .font_family(FONT_MONO)
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(if active {
                    if value.is_empty() {
                        active_input_display(theme, "", &placeholder, theme.text_disabled, 0, 48)
                    } else {
                        active_input_display(
                            theme,
                            value,
                            &placeholder,
                            theme.text_primary,
                            cursor,
                            56,
                        )
                    }
                } else if value.is_empty() {
                    div()
                        .text_color(rgb(theme.text_disabled))
                        .child(placeholder)
                        .into_any_element()
                } else {
                    div()
                        .text_color(rgb(theme.text_primary))
                        .child(value.to_owned())
                        .into_any_element()
                }),
        )
}

fn single_line_input_field(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    value: &str,
    cursor: usize,
    placeholder: impl Into<String>,
    active: bool,
) -> Stateful<Div> {
    let placeholder = placeholder.into();

    div()
        .id(id)
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .h(px(30.))
        .cursor_text()
        .rounded_sm()
        .border_1()
        .border_color(rgb(if active {
            theme.accent
        } else {
            theme.border
        }))
        .bg(rgb(theme.panel_bg))
        .px_2()
        .text_sm()
        .font_family(FONT_MONO)
        .flex()
        .items_center()
        .child(if active {
            if value.is_empty() {
                active_input_display(theme, "", &placeholder, theme.text_disabled, 0, 48)
            } else {
                active_input_display(theme, value, &placeholder, theme.text_primary, cursor, 48)
            }
        } else {
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(rgb(if value.is_empty() {
                    theme.text_disabled
                } else {
                    theme.text_primary
                }))
                .child(if value.is_empty() {
                    placeholder
                } else {
                    value.to_owned()
                })
                .into_any_element()
        })
}

fn active_input_display(
    theme: ThemePalette,
    value: &str,
    placeholder: &str,
    text_color: u32,
    cursor: usize,
    max_chars: usize,
) -> AnyElement {
    if value.is_empty() {
        return div()
            .relative()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(
                div()
                    .text_color(rgb(text_color))
                    .child(placeholder.to_owned()),
            )
            .child(
                input_caret(theme)
                    .flex_none()
                    .absolute()
                    .left(px(0.))
                    .top(px(2.)),
            )
            .into_any_element();
    }

    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .justify_start()
        .gap(px(0.))
        .child({
            let (before_cursor, after_cursor) = visible_input_segments(value, cursor, max_chars);
            div()
                .flex()
                .items_center()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(text_color))
                        .child(before_cursor),
                )
                .child(input_caret(theme).flex_none())
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(text_color))
                        .child(after_cursor),
                )
        })
        .into_any_element()
}

fn visible_input_segments(value: &str, cursor: usize, max_chars: usize) -> (String, String) {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();
    let cursor = cursor.min(len);
    if len <= max_chars {
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars[cursor..].iter().collect();
        return (before, after);
    }

    let window = max_chars.max(1);
    let preferred_left = window.saturating_sub(8);
    let mut start = cursor.saturating_sub(preferred_left);
    start = start.min(len.saturating_sub(window));
    let end = (start + window).min(len);

    let mut before: String = chars[start..cursor].iter().collect();
    let mut after: String = chars[cursor..end].iter().collect();
    if start > 0 {
        before.insert(0, '\u{2026}');
    }
    if end < len {
        after.push('\u{2026}');
    }
    (before, after)
}

fn input_caret(theme: ThemePalette) -> Div {
    div().w(px(1.)).h(px(14.)).bg(rgb(theme.accent)).mt(px(1.))
}

fn status_text(theme: ThemePalette, text: impl Into<String>) -> Div {
    div()
        .text_xs()
        .text_color(rgb(theme.text_muted))
        .child(text.into())
}

#[derive(Clone, Copy)]
struct WorktreeAttentionIndicator {
    label: &'static str,
    short_label: &'static str,
    color: u32,
}

fn worktree_attention_indicator(worktree: &WorktreeSummary) -> WorktreeAttentionIndicator {
    if worktree.stuck_turn_count >= 2 {
        return WorktreeAttentionIndicator {
            label: "Stuck",
            short_label: "Stuck",
            color: 0xeb6f92,
        };
    }
    if worktree.agent_state == Some(AgentState::Working) {
        return WorktreeAttentionIndicator {
            label: "Working",
            short_label: "Run",
            color: 0xe5c07b,
        };
    }
    if worktree.agent_state == Some(AgentState::Waiting)
        && worktree
            .recent_turns
            .first()
            .and_then(|snapshot| snapshot.diff_summary)
            .is_some_and(|summary| summary.additions > 0 || summary.deletions > 0)
    {
        return WorktreeAttentionIndicator {
            label: "Needs review",
            short_label: "Review",
            color: 0x61afef,
        };
    }
    if worktree.agent_state == Some(AgentState::Waiting) {
        return WorktreeAttentionIndicator {
            label: "Waiting",
            short_label: "Wait",
            color: 0x61afef,
        };
    }
    if !worktree.detected_ports.is_empty() {
        return WorktreeAttentionIndicator {
            label: "Serving",
            short_label: "Ports",
            color: 0x72d69c,
        };
    }
    if worktree.last_activity_unix_ms.is_some_and(|timestamp| {
        current_unix_timestamp_millis()
            .unwrap_or(0)
            .saturating_sub(timestamp)
            <= 15 * 60 * 1000
    }) {
        return WorktreeAttentionIndicator {
            label: "Recent",
            short_label: "Recent",
            color: 0xc0caf5,
        };
    }

    WorktreeAttentionIndicator {
        label: "Idle",
        short_label: "Idle",
        color: 0x7f8490,
    }
}

fn worktree_activity_sparkline(worktree: &WorktreeSummary) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if worktree.recent_turns.is_empty() {
        return String::new();
    }

    let values: Vec<usize> = worktree
        .recent_turns
        .iter()
        .take(5)
        .rev()
        .map(|snapshot| {
            snapshot
                .diff_summary
                .map(|summary| summary.additions + summary.deletions)
                .unwrap_or(0)
        })
        .collect();
    let max_value = values.iter().copied().max().unwrap_or(0);
    if max_value == 0 {
        return "▁▁▁".to_owned();
    }

    values
        .into_iter()
        .map(|value| {
            let index = value.saturating_mul(BARS.len() - 1) / max_value.max(1);
            BARS[index]
        })
        .collect()
}

fn parse_terminal_backend_kind(
    terminal_backend: Option<&str>,
) -> Result<TerminalBackendKind, ConfigParseError> {
    let Some(value) = terminal_backend
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(TerminalBackendKind::Embedded);
    };

    match value.to_ascii_lowercase().as_str() {
        "embedded" => Ok(TerminalBackendKind::Embedded),
        "alacritty" | "ghostty" => Err(ConfigParseError::InvalidValue(format!(
            "terminal_backend `{value}` is no longer supported; Arbor terminals are embedded-only. Using the embedded terminal instead. Configure `embedded_terminal_engine` to choose `alacritty` or `ghostty-vt-experimental`."
        ))),
        _ => Err(ConfigParseError::InvalidValue(format!(
            "invalid terminal_backend `{value}` in config, expected `embedded`"
        ))),
    }
}

fn parse_theme_kind(theme: Option<&str>) -> Result<ThemeKind, ConfigParseError> {
    let Some(value) = theme.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(ThemeKind::One);
    };

    match value.to_ascii_lowercase().as_str() {
        "one-dark" | "onedark" => Ok(ThemeKind::One),
        "ayu-dark" | "ayu" => Ok(ThemeKind::Ayu),
        "gruvbox-dark" | "gruvbox" => Ok(ThemeKind::Gruvbox),
        "dracula" => Ok(ThemeKind::Dracula),
        "solarized-light" | "solarized" => Ok(ThemeKind::SolarizedLight),
        "everforest-dark" | "everforest" => Ok(ThemeKind::Everforest),
        "catppuccin" => Ok(ThemeKind::Catppuccin),
        "catppuccin-latte" => Ok(ThemeKind::CatppuccinLatte),
        "ethereal" => Ok(ThemeKind::Ethereal),
        "flexoki-light" | "flexoki" => Ok(ThemeKind::FlexokiLight),
        "hackerman" => Ok(ThemeKind::Hackerman),
        "kanagawa" => Ok(ThemeKind::Kanagawa),
        "matte-black" | "matteblack" => Ok(ThemeKind::MatteBlack),
        "miasma" => Ok(ThemeKind::Miasma),
        "nord" => Ok(ThemeKind::Nord),
        "osaka-jade" | "osakajade" => Ok(ThemeKind::OsakaJade),
        "ristretto" => Ok(ThemeKind::Ristretto),
        "rose-pine" | "rosepine" => Ok(ThemeKind::RosePine),
        "tokyo-night" | "tokyonight" => Ok(ThemeKind::TokyoNight),
        "vantablack" => Ok(ThemeKind::Vantablack),
        "white" => Ok(ThemeKind::White),
        "atom-one-light" | "atomonelight" => Ok(ThemeKind::AtomOneLight),
        "github-light-default" | "githublightdefault" => Ok(ThemeKind::GitHubLightDefault),
        "github-light-high-contrast" | "githublighthighcontrast" => {
            Ok(ThemeKind::GitHubLightHighContrast)
        },
        "github-light-colorblind" | "githublightcolorblind" => Ok(ThemeKind::GitHubLightColorblind),
        "github-light" | "githublight" => Ok(ThemeKind::GitHubLight),
        "github-dark-default" | "githubdarkdefault" => Ok(ThemeKind::GitHubDarkDefault),
        "github-dark-high-contrast" | "githubdarkhighcontrast" => {
            Ok(ThemeKind::GitHubDarkHighContrast)
        },
        "github-dark-colorblind" | "githubdarkcolorblind" => Ok(ThemeKind::GitHubDarkColorblind),
        "github-dark-dimmed" | "githubdarkdimmed" => Ok(ThemeKind::GitHubDarkDimmed),
        "github-dark" | "githubdark" => Ok(ThemeKind::GitHubDark),
        "retrobox-classic" | "retrobox" => Ok(ThemeKind::RetroboxClassic),
        "tokyonight-day" | "tokionight-day" => Ok(ThemeKind::TokyoNightDay),
        "tokyonight-classic" | "tokionight-classic" => Ok(ThemeKind::TokyoNightClassic),
        "zellner" => Ok(ThemeKind::Zellner),
        _ => Err(ConfigParseError::InvalidValue(format!(
            "invalid theme `{value}` in config, expected one-dark/ayu-dark/gruvbox-dark/dracula/solarized-light/everforest-dark/catppuccin/catppuccin-latte/ethereal/flexoki-light/hackerman/kanagawa/matte-black/miasma/nord/osaka-jade/ristretto/rose-pine/tokyo-night/vantablack/white/atom-one-light/github-light-default/github-light-high-contrast/github-light-colorblind/github-light/github-dark-default/github-dark-high-contrast/github-dark-colorblind/github-dark-dimmed/github-dark/retrobox-classic/tokyonight-day/tokyonight-classic/zellner"
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use {
        crate::{
            DaemonTerminalRuntime, DaemonTerminalWsState, PendingSave, TerminalRuntimeHandle,
            TerminalRuntimeKind, TerminalSession, TerminalState, WorktreeHoverPopover,
            WorktreeSummary, apply_daemon_snapshot,
            checkout::CheckoutKind,
            estimated_worktree_hover_popover_card_height, extract_first_url,
            parse_terminal_backend_kind, prioritized_pr_checks_for_display,
            resolve_github_access_token_from_sources, styled_lines_for_session,
            terminal_backend::{
                TerminalBackendKind, TerminalCursor, TerminalModes, TerminalStyledCell,
                TerminalStyledLine, TerminalStyledRun,
            },
            terminal_daemon_http::{HttpTerminalDaemon, WebsocketConnectConfig},
            theme::ThemeKind,
            track_terminal_command_keystroke, ui_state_store, worktree_hover_popover_zone_bounds,
            worktree_hover_safe_zone_contains,
        },
        arbor_core::{agent::AgentState, changes::DiffLineSummary, daemon, process::ProcessSource},
        gpui::{Keystroke, point, px},
        std::{
            env, fs,
            path::{Path, PathBuf},
            sync::Arc,
            time::{Instant, SystemTime},
        },
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
            worktree_path: PathBuf::from("/tmp/worktree"),
            managed_process_id: None,
            title: "term-1".to_owned(),
            last_command: None,
            pending_command: String::new(),
            command: "zsh".to_owned(),
            agent_preset: None,
            execution_mode: None,
            state: TerminalState::Running,
            exit_code: None,
            updated_at_unix_ms: None,
            root_pid: None,
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
            queued_input: Vec::new(),
            is_initializing: false,
            runtime: None,
        }
    }

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
    fn parse_terminal_backend_defaults_to_embedded() {
        assert_eq!(
            parse_terminal_backend_kind(None),
            Ok(TerminalBackendKind::Embedded),
        );
        assert_eq!(
            parse_terminal_backend_kind(Some("")),
            Ok(TerminalBackendKind::Embedded),
        );
    }

    #[test]
    fn parse_terminal_backend_rejects_external_backends() {
        let alacritty = parse_terminal_backend_kind(Some("alacritty"));
        let ghostty = parse_terminal_backend_kind(Some("ghostty"));

        assert!(alacritty.is_err());
        assert!(ghostty.is_err());
    }

    fn daemon_runtime_for_test() -> DaemonTerminalRuntime {
        let daemon = match HttpTerminalDaemon::new("http://127.0.0.1:1") {
            Ok(daemon) => daemon,
            Err(error) => panic!("failed to create daemon client: {error}"),
        };

        DaemonTerminalRuntime {
            daemon: Arc::new(daemon),
            ws_state: Arc::new(DaemonTerminalWsState::default()),
            last_synced_ws_generation: std::sync::atomic::AtomicU64::new(0),
            snapshot_request_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            kind: TerminalRuntimeKind::Local,
            resize_error_label: "resize",
            exit_labels: None,
            clear_global_daemon_on_connection_refused: false,
        }
    }

    #[test]
    fn attention_indicator_prefers_stuck_state() {
        let mut worktree = sample_worktree_summary();
        worktree.agent_state = Some(AgentState::Waiting);
        worktree.stuck_turn_count = 2;

        let attention = crate::worktree_attention_indicator(&worktree);
        assert_eq!(attention.label, "Stuck");
    }

    #[test]
    fn normalized_sidebar_order_keeps_saved_items_and_appends_new_ones() {
        let saved = vec![
            crate::SidebarItemId::Outpost("outpost-1".to_owned()),
            crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-2")),
        ];
        let worktrees = vec![
            crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-1")),
            crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-2")),
        ];
        let outposts = vec![crate::SidebarItemId::Outpost("outpost-1".to_owned())];

        assert_eq!(
            crate::sidebar::normalized_sidebar_order(Some(saved.as_slice()), worktrees, outposts),
            vec![
                crate::SidebarItemId::Outpost("outpost-1".to_owned()),
                crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-2")),
                crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-1")),
            ]
        );
    }

    #[test]
    fn reordered_sidebar_items_moves_dragged_item_to_requested_slot() {
        let items = vec![
            crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-1")),
            crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-2")),
            crate::SidebarItemId::Outpost("outpost-1".to_owned()),
        ];

        assert_eq!(
            crate::sidebar::reordered_sidebar_items(
                &items,
                &crate::SidebarItemId::Outpost("outpost-1".to_owned()),
                0,
            ),
            Some(vec![
                crate::SidebarItemId::Outpost("outpost-1".to_owned()),
                crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-1")),
                crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-2")),
            ])
        );

        assert_eq!(
            crate::sidebar::reordered_sidebar_items(
                &items,
                &crate::SidebarItemId::Worktree(PathBuf::from("/tmp/repo/wt-1")),
                1,
            ),
            None
        );
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
    fn orphaned_daemon_session_cleanup_kills_only_running_sessions() {
        let mut record = daemon::DaemonSessionRecord {
            session_id: "daemon-test-1".into(),
            workspace_id: "/tmp/worktree".into(),
            cwd: PathBuf::from("/tmp/worktree"),
            shell: "zsh".to_owned(),
            ..Default::default()
        };

        assert!(crate::orphaned_daemon_session_should_kill(&record));

        record.state = Some(daemon::TerminalSessionState::Completed);
        assert!(!crate::orphaned_daemon_session_should_kill(&record));

        record.state = Some(daemon::TerminalSessionState::Failed);
        assert!(!crate::orphaned_daemon_session_should_kill(&record));
    }

    #[test]
    fn background_config_save_has_work_when_count_is_nonzero() {
        assert!(!crate::background_config_save_has_work(0));
        assert!(crate::background_config_save_has_work(1));
        assert!(crate::background_config_save_has_work(3));
    }

    #[test]
    fn terminal_input_buffers_only_while_session_is_initializing() {
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);

        session.is_initializing = true;
        assert!(crate::terminal_interaction::should_queue_terminal_input(
            &session
        ));

        session.is_initializing = false;
        assert!(!crate::terminal_interaction::should_queue_terminal_input(
            &session
        ));

        session.is_initializing = true;
        session.runtime = Some(Arc::new(daemon_runtime_for_test()));
        assert!(!crate::terminal_interaction::should_queue_terminal_input(
            &session
        ));
    }

    #[test]
    fn daemon_runtime_without_cached_snapshot_returns_without_sync_error() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.output.clear();
        session.styled_output.clear();
        session.cursor = None;
        session.last_runtime_sync_at = Some(Instant::now());

        let outcome = runtime.sync(&mut session, true, None);

        assert!(!outcome.changed);
        assert!(outcome.notice.is_none());
        assert_eq!(session.state, TerminalState::Running);
        assert!(session.output.is_empty());
    }

    #[test]
    fn daemon_ws_state_rehydrates_trimmed_snapshot_from_ansi_output() {
        let ws_state = DaemonTerminalWsState::default();
        ws_state.apply_snapshot_text("hello\r\nworld\r\n", TerminalState::Running, None, Some(42));

        let snapshot = ws_state
            .snapshot()
            .unwrap_or_else(|| panic!("expected websocket snapshot to be available"));

        assert_eq!(snapshot.state, TerminalState::Running);
        assert_eq!(snapshot.updated_at_unix_ms, Some(42));
        assert!(snapshot.terminal.output.contains("hello"));
        assert!(snapshot.terminal.output.contains("world"));
        assert_eq!(snapshot.terminal.styled_lines.len(), 2);
    }

    #[test]
    fn daemon_runtime_sync_applies_cached_ws_snapshot_without_http_roundtrip() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.output.clear();
        session.styled_output.clear();
        session.cursor = None;
        session.exit_code = None;
        runtime.ws_state.apply_snapshot_text(
            "codex> working\r\n",
            TerminalState::Running,
            None,
            Some(99),
        );

        let outcome = runtime.sync(&mut session, true, None);

        assert!(outcome.changed);
        assert_eq!(session.state, TerminalState::Running);
        assert_eq!(session.updated_at_unix_ms, Some(99));
        assert!(session.output.contains("codex> working"));
        assert_eq!(session.exit_code, None);
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
    fn prioritized_pr_checks_show_failures_before_pending_before_successes() {
        let pr = crate::github_service::PrDetails {
            number: 7,
            title: "Sort checks".to_owned(),
            url: "https://example.com/pr/7".to_owned(),
            state: crate::github_service::PrState::Open,
            additions: 1,
            deletions: 1,
            review_decision: crate::github_service::ReviewDecision::Pending,
            mergeable: crate::github_service::MergeableState::Mergeable,
            merge_state_status: crate::github_service::MergeStateStatus::Clean,
            passed_checks: 2,
            checks_status: crate::github_service::CheckStatus::Pending,
            checks: vec![
                (
                    "b-failure".to_owned(),
                    crate::github_service::CheckStatus::Failure,
                ),
                (
                    "a-pending".to_owned(),
                    crate::github_service::CheckStatus::Pending,
                ),
                (
                    "a-success".to_owned(),
                    crate::github_service::CheckStatus::Success,
                ),
                (
                    "z-success".to_owned(),
                    crate::github_service::CheckStatus::Success,
                ),
            ],
        };

        let checks = prioritized_pr_checks_for_display(&pr);

        assert_eq!(checks, &[
            (
                "b-failure".to_owned(),
                crate::github_service::CheckStatus::Failure
            ),
            (
                "a-pending".to_owned(),
                crate::github_service::CheckStatus::Pending
            ),
            (
                "a-success".to_owned(),
                crate::github_service::CheckStatus::Success
            ),
            (
                "z-success".to_owned(),
                crate::github_service::CheckStatus::Success
            ),
        ]);
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
            session_id: "daemon-test-1".to_owned().into(),
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
            crate::parse_theme_kind(Some("atom-one-light")).ok(),
            Some(ThemeKind::AtomOneLight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-light-default")).ok(),
            Some(ThemeKind::GitHubLightDefault)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-light-high-contrast")).ok(),
            Some(ThemeKind::GitHubLightHighContrast)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-light-colorblind")).ok(),
            Some(ThemeKind::GitHubLightColorblind)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-light")).ok(),
            Some(ThemeKind::GitHubLight)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark-default")).ok(),
            Some(ThemeKind::GitHubDarkDefault)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark-high-contrast")).ok(),
            Some(ThemeKind::GitHubDarkHighContrast)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark-colorblind")).ok(),
            Some(ThemeKind::GitHubDarkColorblind)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark-dimmed")).ok(),
            Some(ThemeKind::GitHubDarkDimmed)
        );
        assert_eq!(
            crate::parse_theme_kind(Some("github-dark")).ok(),
            Some(ThemeKind::GitHubDark)
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
    fn extract_first_url_ignores_punctuation() {
        let url = extract_first_url("created PR: https://github.com/acme/repo/pull/42.");
        assert_eq!(url.as_deref(), Some("https://github.com/acme/repo/pull/42"));
    }

    #[test]
    fn issue_markdown_to_text_produces_readable_plain_text() {
        let markdown = r#"# Summary

- [x] shipped **bold** change
- see [docs](https://example.com/docs)

> quoted _note_

```rs
let answer = 42;
```
"#;

        let plain_text = crate::issue_markdown_to_text(markdown);
        assert_eq!(
            plain_text,
            "Summary\n\nshipped bold change\nsee docs (https://example.com/docs)\n\nquoted note\n\nlet answer = 42;"
        );
    }

    #[test]
    fn issue_body_text_falls_back_when_body_is_missing_or_empty() {
        assert_eq!(
            crate::issue_body_text(None),
            crate::ISSUE_DESCRIPTION_FALLBACK
        );
        assert_eq!(
            crate::issue_body_text(Some("   \n\n")),
            crate::ISSUE_DESCRIPTION_FALLBACK
        );
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
    fn pending_save_coalesces_to_latest_value_after_inflight_write() {
        let mut pending = PendingSave::default();

        pending.queue("first");
        assert_eq!(pending.begin_next(), Some("first"));
        assert!(pending.has_work());

        pending.queue("second");
        pending.queue("third");
        assert!(pending.begin_next().is_none());

        pending.finish();

        assert_eq!(pending.begin_next(), Some("third"));
        pending.finish();
        assert!(!pending.has_work());
    }

    #[test]
    fn pending_save_reports_work_for_pending_and_inflight_states() {
        let mut pending = PendingSave::default();
        assert!(!pending.has_work());

        pending.queue(1_u8);
        assert!(pending.has_work());

        let _ = pending.begin_next();
        assert!(pending.has_work());

        pending.finish();
        assert!(!pending.has_work());
    }

    #[test]
    fn ui_state_save_has_work_for_pending_and_inflight_states() {
        let state = ui_state_store::UiState::default();

        assert!(!crate::ui_state_save_has_work(None, None));
        assert!(crate::ui_state_save_has_work(Some(&state), None));
        assert!(crate::ui_state_save_has_work(None, Some(&state)));
    }

    #[test]
    fn next_pending_ui_state_save_keeps_reverted_state_queued_while_other_save_is_in_flight() {
        let persisted = ui_state_store::UiState {
            left_pane_width: Some(240),
            ..ui_state_store::UiState::default()
        };
        let in_flight = ui_state_store::UiState {
            left_pane_width: Some(320),
            ..ui_state_store::UiState::default()
        };

        assert_eq!(
            crate::next_pending_ui_state_save(&persisted, None, Some(&in_flight), &persisted),
            Some(persisted),
        );
    }

    #[test]
    fn next_pending_ui_state_save_does_not_duplicate_inflight_state() {
        let state = ui_state_store::UiState {
            left_pane_width: Some(320),
            ..ui_state_store::UiState::default()
        };

        assert_eq!(
            crate::next_pending_ui_state_save(
                &ui_state_store::UiState::default(),
                None,
                Some(&state),
                &state,
            ),
            None,
        );
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
        session.state = TerminalState::Completed;
        assert!(!crate::managed_process_session_is_active(&session));

        session.state = TerminalState::Running;
        assert!(crate::managed_process_session_is_active(&session));

        session.is_initializing = true;
        session.state = TerminalState::Completed;
        assert!(crate::managed_process_session_is_active(&session));
    }
}
