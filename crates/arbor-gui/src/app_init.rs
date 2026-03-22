use super::*;

impl ArborWindow {
    pub(crate) fn load_with_daemon_store<S>(
        startup_ui_state: ui_state_store::UiState,
        log_buffer: log_layer::LogBuffer,
        cx: &mut Context<Self>,
    ) -> Self
    where
        S: daemon::DaemonSessionStore + Default + 'static,
    {
        Self::load(Arc::new(S::default()), startup_ui_state, log_buffer, cx)
    }

    pub(crate) fn load(
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
                arbor_terminal_emulator::set_default_terminal_scrollback_lines(
                    arbor_terminal_emulator::sanitize_terminal_scrollback_lines(
                        loaded_config.config.terminal_scrollback_lines,
                    ),
                );
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
                let configured_providers =
                    load_configured_providers(&loaded_config.config.providers);
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
                    terminal_font_metrics: None,
                    center_tabs_scroll_handle: ScrollHandle::new(),
                    center_tabs_last_scrolled_index: None,
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
                    configured_providers,
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
                    daemon_session_store_dirty: false,
                    last_ui_state_error: None,
                    last_issue_cache_error: None,
                    notification_service,
                    notifications_enabled,
                    agent_activity_sessions: HashMap::new(),
                    last_agent_finished_notifications: HashMap::new(),
                    auto_checkpoint_in_flight: Arc::new(Mutex::new(HashSet::new())),
                    agent_activity_epochs: Arc::new(Mutex::new(HashMap::new())),
                    window_is_active: true,
                    last_window_geometry: None,
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
                    agent_chat_sessions: Vec::new(),
                    active_agent_chat_by_worktree: HashMap::new(),
                    next_agent_chat_id: 1,
                    agent_chat_scroll_handle: ScrollHandle::new(),
                    agent_selector_open_for: None,
                    agent_selector_search: String::new(),
                    agent_selector_search_cursor: 0,
                    chat_mode_selector_open_for: None,
                    center_tab_order: Vec::new(),
                    new_tab_menu_position: None,
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
                    _daemon_session_store_debounce_task: None,
                    _create_modal_preview_task: None,
                    _file_tree_refresh_task: None,
                    worktree_refresh_epoch: 0,
                    changed_files_refresh_epoch: 0,
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
                    pending_terminal_text_input_fallback: None,
                    welcome_clone_url: String::new(),
                    welcome_clone_url_cursor: 0,
                    welcome_clone_url_active: false,
                    welcome_cloning: false,
                    welcome_clone_error: None,
                    remote_daemon_states: HashMap::new(),
                    active_remote_worktree: None,
                    update_available: None,
                    self_cpu_percent: None,
                    self_memory_bytes: None,
                };

                return app;
            },
        };

        let repo_root = worktree::repo_root(&cwd).ok();

        tracing::info!(config = %config_path.display(), "loading configuration");
        let loaded_config = app_config_store.load_or_create_config();
        arbor_terminal_emulator::set_default_terminal_scrollback_lines(
            arbor_terminal_emulator::sanitize_terminal_scrollback_lines(
                loaded_config.config.terminal_scrollback_lines,
            ),
        );
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
        let daemon_auth_tokens = connection_history::load_tokens();
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
        if let Some(token) = daemon_auth_tokens.get(&daemon_base_url)
            && let Some(daemon) = terminal_daemon.as_ref()
        {
            daemon.set_auth_token(Some(token.clone()));
        }
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
        let preferred_repo_root =
            preferred_startup_repository_root(startup_repository_root.clone(), repo_root.clone());
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
        let configured_providers = load_configured_providers(&loaded_config.config.providers);

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
            terminal_font_metrics: None,
            center_tabs_scroll_handle: ScrollHandle::new(),
            center_tabs_last_scrolled_index: None,
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
            configured_providers,
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
            daemon_auth_tokens,
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
            agent_chat_sessions: Vec::new(),
            active_agent_chat_by_worktree: HashMap::new(),
            next_agent_chat_id: 1,
            agent_chat_scroll_handle: ScrollHandle::new(),
            agent_selector_open_for: None,
            agent_selector_search: String::new(),
            agent_selector_search_cursor: 0,
            chat_mode_selector_open_for: None,
            center_tab_order: Vec::new(),
            new_tab_menu_position: None,
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
            _daemon_session_store_debounce_task: None,
            _create_modal_preview_task: None,
            _file_tree_refresh_task: None,
            worktree_refresh_epoch: 0,
            changed_files_refresh_epoch: 0,
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
            daemon_session_store_dirty: false,
            last_ui_state_error: None,
            last_issue_cache_error: None,
            notification_service,
            notifications_enabled,
            agent_activity_sessions: HashMap::new(),
            last_agent_finished_notifications: HashMap::new(),
            auto_checkpoint_in_flight: Arc::new(Mutex::new(HashSet::new())),
            agent_activity_epochs: Arc::new(Mutex::new(HashMap::new())),
            window_is_active: true,
            last_window_geometry: None,
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
            pending_terminal_text_input_fallback: None,
            welcome_clone_url: String::new(),
            welcome_clone_url_cursor: 0,
            welcome_clone_url_active: false,
            welcome_cloning: false,
            welcome_clone_error: None,
            remote_daemon_states: HashMap::new(),
            active_remote_worktree: None,
            update_available: None,
            self_cpu_percent: None,
            self_memory_bytes: None,
        };

        app.refresh_worktrees(cx);
        app.refresh_cached_issue_lists_on_startup(cx);
        app.refresh_repo_config_if_changed(cx);
        app.refresh_github_auth_identity(cx);
        app.restore_terminal_sessions_from_records(initial_daemon_records, attach_daemon_runtime);
        app.restore_agent_chat_sessions(cx);
        app.probe_provider_models(cx);
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
        app.start_version_check_poller(cx);
        app.start_memory_poller(cx);
        app.ensure_claude_code_hooks(cx);
        app.ensure_pi_agent_extension(cx);

        app
    }

    /// Returns the directory where repo preset edits should be saved.
    /// Prefers the selected worktree path, falls back to repo_root.
    pub(crate) fn active_arbor_toml_dir(&self) -> PathBuf {
        self.selected_worktree_path()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.repo_root.clone())
    }

    pub(crate) fn selected_agent_preset_or_default(&self) -> AgentPresetKind {
        self.active_preset_tab.unwrap_or(AgentPresetKind::Codex)
    }

    pub(crate) fn branch_prefix_github_login(&self) -> Option<String> {
        self.github_auth_state
            .user_login
            .clone()
            .or_else(|| env::var("ARBOR_GITHUB_USER").ok())
            .or_else(|| env::var("GITHUB_USER").ok())
            .and_then(|value| non_empty_trimmed_str(&value).map(str::to_owned))
    }

    pub(crate) fn maybe_finish_quit_after_persistence_flush(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn request_quit_after_persistence_flush(&mut self, cx: &mut Context<Self>) {
        self.quit_after_persistence_flush = true;
        self.sync_daemon_session_store(cx);
        self.maybe_finish_quit_after_persistence_flush(cx);
    }

    pub(crate) fn maybe_notify(&self, title: &str, body: &str, play_sound: bool) {
        if self.notifications_enabled && !self.window_is_active {
            self.notification_service.send(title, body, play_sound);
        }
    }

    pub(crate) fn maybe_notify_agent_finished(
        &mut self,
        worktree: &WorktreeSummary,
        updated_at: Option<u64>,
    ) {
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

    pub(crate) fn switch_theme(&mut self, theme_kind: ThemeKind, cx: &mut Context<Self>) {
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

    pub(crate) fn launch_repo_preset(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
