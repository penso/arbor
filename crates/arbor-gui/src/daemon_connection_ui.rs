impl ArborWindow {
    fn persist_connection_history(&mut self, cx: &mut Context<Self>) {
        self.connection_history_save
            .queue(self.connection_history.clone());
        self.start_pending_connection_history_save(cx);
    }

    fn record_connection_history_entry(
        &mut self,
        address: &str,
        label: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.connection_history =
            connection_history::updated_history_entries(&self.connection_history, address, label);
        self.persist_connection_history(cx);
    }

    fn persist_daemon_auth_tokens(&mut self, cx: &mut Context<Self>) {
        self.daemon_auth_tokens_save
            .queue(self.daemon_auth_tokens.clone());
        self.start_pending_daemon_auth_tokens_save(cx);
    }

    fn start_pending_connection_history_save(&mut self, cx: &mut Context<Self>) {
        let Some(history) = self.connection_history_save.begin_next() else {
            self.maybe_finish_quit_after_persistence_flush(cx);
            return;
        };

        self._connection_history_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { connection_history::save_history(&history) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.connection_history_save.finish();
                if let Err(error) = result {
                    this.notice = Some(format!("failed to persist connection history: {error}"));
                    cx.notify();
                }

                this.start_pending_connection_history_save(cx);
                this.maybe_finish_quit_after_persistence_flush(cx);
            });
        }));
    }

    fn start_pending_daemon_auth_tokens_save(&mut self, cx: &mut Context<Self>) {
        let Some(tokens) = self.daemon_auth_tokens_save.begin_next() else {
            self.maybe_finish_quit_after_persistence_flush(cx);
            return;
        };

        self._daemon_auth_tokens_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { connection_history::save_tokens(&tokens) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.daemon_auth_tokens_save.finish();
                if let Err(error) = result {
                    this.notice = Some(format!("failed to persist daemon auth tokens: {error}"));
                    cx.notify();
                }

                this.start_pending_daemon_auth_tokens_save(cx);
                this.maybe_finish_quit_after_persistence_flush(cx);
            });
        }));
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
        self.record_connection_history_entry(&daemon_url, Some(&hostname), cx);
        self.pending_remote_create_repo_root = Some(repo_root.clone());
        self.connect_to_daemon_endpoint(
            &daemon_url,
            Some(hostname),
            None,
            Some(repo_root),
            false,
            cx,
        );
    }

    fn open_create_modal_for_connected_repo(&mut self, repo_root: &str, cx: &mut Context<Self>) {
        if let Some(repo_index) = self
            .repositories
            .iter()
            .position(|repository| repository.root.to_string_lossy().ends_with(repo_root))
        {
            self.select_repository(repo_index, cx);
            self.open_create_modal(repo_index, CreateModalTab::LocalWorktree, cx);
        } else if let Some(repo_index) = self.repositories.first().map(|_| 0) {
            self.select_repository(repo_index, cx);
            self.open_create_modal(repo_index, CreateModalTab::LocalWorktree, cx);
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
            if let Some(session_id) = self.active_terminal_id_for_worktree(&cwd) {
                self.active_terminal_by_worktree.insert(cwd, session_id);
            }
        } else {
            let session_id = self.next_terminal_id;
            self.next_terminal_id += 1;
            self.active_terminal_by_worktree
                .insert(cwd.clone(), session_id);

            let shell = self.embedded_shell();

            let session = TerminalSession {
                id: session_id,
                daemon_session_id: session_id.to_string(),
                worktree_path: cwd.clone(),
                title: format!("term-{session_id}"),
                last_command: None,
                pending_command: String::new(),
                command: String::new(),
                agent_preset: None,
                execution_mode: None,
                state: TerminalState::Running,
                exit_code: None,
                updated_at_unix_ms: current_unix_timestamp_millis(),
                root_pid: None,
                cols: 120,
                rows: 35,
                generation: 0,
                output: String::new(),
                styled_output: Vec::new(),
                cursor: None,
                modes: TerminalModes::default(),
                last_runtime_sync_at: None,
                queued_input: Vec::new(),
                is_initializing: true,
                runtime: None,
            };
            self.terminals.push(session);
            let poll_tx = self.terminal_poll_tx.clone();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_spawn(async move {
                        client
                            .create_or_attach(CreateOrAttachRequest {
                                session_id: String::new().into(),
                                workspace_id: cwd.display().to_string().into(),
                                cwd: cwd.clone(),
                                shell,
                                cols: 120,
                                rows: 35,
                                title: Some(format!("term-{session_id}")),
                                command: None,
                            })
                            .map(|response| (client, response.session))
                            .map_err(|error| error.to_string())
                    })
                    .await;

                let _ = this.update(cx, |this, cx| {
                    let Some(session) = this
                        .terminals
                        .iter_mut()
                        .find(|session| session.id == session_id)
                    else {
                        return;
                    };

                    match result {
                        Ok((client, daemon_session)) => {
                            session.daemon_session_id = daemon_session.session_id.to_string();
                            session.title = daemon_session
                                .title
                                .clone()
                                .filter(|value| !value.trim().is_empty())
                                .unwrap_or_else(|| session.title.clone());
                            session.last_command = daemon_session.last_command.clone();
                            session.command = daemon_session.shell.clone();
                            session.output =
                                daemon_session.output_tail.clone().unwrap_or_default();
                            session.state = terminal_state_from_daemon_record(&daemon_session);
                            session.exit_code = daemon_session.exit_code;
                            session.updated_at_unix_ms = daemon_session.updated_at_unix_ms;
                            session.root_pid = daemon_session.root_pid;
                            session.cols = daemon_session.cols.max(2);
                            session.rows = daemon_session.rows.max(1);
                            session.runtime = Some(local_daemon_runtime(
                                client,
                                daemon_session.session_id.to_string(),
                                session.rows,
                                session.cols,
                                Some(poll_tx.clone()),
                            ));
                            session.is_initializing = false;
                            if let Err(error) =
                                this.flush_queued_input_for_terminal(session_id)
                            {
                                this.notice = Some(format!(
                                    "failed to write queued terminal input: {error}"
                                ));
                            }
                        },
                        Err(error) => {
                            tracing::warn!(%error, "failed to create remote terminal session");
                            session.is_initializing = false;
                            session.state = TerminalState::Failed;
                            session.output = error.clone();
                            this.notice =
                                Some(format!("failed to create terminal on {hostname}: {error}"));
                        },
                    }
                    cx.notify();
                });
            })
            .detach();
        }

        self.terminal_scroll_handle.scroll_to_bottom();
        window.focus(&self.terminal_focus);
        cx.notify();
    }

    fn toggle_discovered_daemon(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(state) = self.remote_daemon_states.get(&index) {
            if state.expanded {
                if let Some(state) = self.remote_daemon_states.get_mut(&index) {
                    state.expanded = false;
                }
                cx.notify();
                return;
            }
            if !state.repositories.is_empty() || !state.worktrees.is_empty() {
                if let Some(state) = self.remote_daemon_states.get_mut(&index) {
                    state.expanded = true;
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

        let client = match terminal_daemon_http::HttpTerminalDaemon::new(&url) {
            Ok(client) => Arc::new(client),
            Err(error) => {
                tracing::error!(%error, %url, "failed to create HTTP client for LAN daemon");
                return;
            },
        };

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

        let client_clone = Arc::clone(&client);
        let url_clone = url.clone();
        cx.spawn(async move |this, cx| {
            let (repositories, worktrees, error, needs_auth) = cx
                .background_spawn(async move {
                    let repositories = client_clone.list_repositories();
                    let worktrees = client_clone.list_worktrees();
                    match (repositories, worktrees) {
                        (Ok(repositories), Ok(worktrees)) => {
                            (repositories, worktrees, None, false)
                        },
                        (Err(error), _) | (_, Err(error)) => {
                            let needs_auth = error.is_unauthorized();
                            tracing::warn!(%error, needs_auth, "failed to fetch from LAN daemon");
                            (Vec::new(), Vec::new(), Some(format!("{error}")), needs_auth)
                        },
                    }
                })
                .await;

            let _ = cx.update(|cx| {
                this.update(cx, |this, cx| {
                    if let Some(state) = this.remote_daemon_states.get_mut(&index) {
                        state.repositories = repositories;
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
        let ssh_destination = target.ssh_destination();
        let ssh_port = target.ssh_port;
        let daemon_port = target.daemon_port;
        self.notice = Some(format!(
            "connecting to {} via SSH tunnel\u{2026}",
            ssh_destination
        ));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let tunnel_result = cx
                .background_spawn(async move { SshDaemonTunnel::start(&target) })
                .await;

            let Some((local_url, local_port)) = this
                .update(cx, |this, cx| match tunnel_result {
                    Ok(tunnel) => {
                        let local_url = tunnel.local_url();
                        let local_port = tunnel.local_port;
                        tracing::info!(
                            remote = %ssh_destination,
                            ssh_port = ssh_port,
                            daemon_port = daemon_port,
                            local_url = %local_url,
                            "connecting to daemon through ssh tunnel"
                        );
                        this.ssh_daemon_tunnel = Some(tunnel);
                        Some((local_url, local_port))
                    },
                    Err(error) => {
                        this.notice = Some(error);
                        this.terminal_daemon = None;
                        this.connected_daemon_label = None;
                        cx.notify();
                        None
                    },
                })
                .ok()
                .flatten()
            else {
                return;
            };

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
                    this.connect_to_daemon_endpoint(
                        &local_url,
                        label,
                        Some(auth_key),
                        None,
                        true,
                        cx,
                    );
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
        open_create_repo_root: Option<String>,
        stop_tunnel_on_failure: bool,
        cx: &mut Context<Self>,
    ) {
        tracing::info!(url = %url, "connecting to daemon");
        let connect_epoch = self.daemon_connect_epoch.wrapping_add(1);
        self.daemon_connect_epoch = connect_epoch;
        self.daemon_base_url = url.to_owned();
        let token_key = auth_key.unwrap_or_else(|| url.to_owned());
        let client = match terminal_daemon_http::default_terminal_daemon_client(url) {
            Ok(client) => client,
            Err(error) => {
                tracing::warn!(url = %url, %error, "failed to create daemon client");
                self.notice = Some(format!("failed to connect to {url}: {error}"));
                self.terminal_daemon = None;
                self.connected_daemon_label = None;
                cx.notify();
                return;
            },
        };

        if let Some(token) = self.daemon_auth_tokens.get(&token_key) {
            client.set_auth_token(Some(token.clone()));
        }

        let url = url.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let client_for_error = client.clone();
                    client
                        .list_sessions()
                        .map(|records| (client, records))
                        .map_err(|error| (client_for_error, error.to_string()))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.daemon_connect_epoch != connect_epoch {
                    return;
                }

                match result {
                    Ok((client, records)) => {
                        this.terminal_daemon = Some(client);
                        this.connected_daemon_label = label;
                        this.restore_terminal_sessions_from_records(records, true);
                        this.refresh_worktrees(cx);
                        if let Some(repo_root) = open_create_repo_root.as_deref() {
                            this.open_create_modal_for_connected_repo(repo_root, cx);
                            this.pending_remote_create_repo_root = None;
                        }
                    },
                    Err((client, error)) => {
                        if error.contains("status 403") {
                            tracing::warn!(url = %url, "daemon rejected connection: forbidden (no auth token configured on remote)");
                            this.notice = Some(
                                "Remote host has no auth token configured. Set [daemon] auth_token in ~/.config/arbor/config.toml on the remote host.".to_owned(),
                            );
                            this.terminal_daemon = None;
                            this.connected_daemon_label = None;
                            if stop_tunnel_on_failure {
                                this.stop_active_ssh_daemon_tunnel();
                            }
                        } else if error.contains("status 401") {
                            tracing::info!(url = %url, "daemon requires authentication, showing auth modal");
                            this.daemon_auth_modal = Some(DaemonAuthModal {
                                daemon_url: token_key,
                                token: String::new(),
                                token_cursor: 0,
                                error: None,
                            });
                            this.terminal_daemon = Some(client);
                            this.connected_daemon_label = label;
                            this.pending_remote_create_repo_root = open_create_repo_root;
                        } else {
                            tracing::warn!(url = %url, %error, "failed to connect to daemon");
                            this.notice = Some(format!("failed to connect to {url}: {error}"));
                            this.terminal_daemon = None;
                            this.connected_daemon_label = None;
                            if stop_tunnel_on_failure {
                                this.stop_active_ssh_daemon_tunnel();
                            }
                        }
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn try_start_and_connect_daemon(&mut self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { try_auto_start_daemon(&daemon_base_url) })
                .await;

            match result {
                Some(client) => {
                    let list_result = cx
                        .background_spawn(async move {
                            client
                                .list_sessions()
                                .map(|records| (client, records))
                                .map_err(|error| error.to_string())
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        match list_result {
                            Ok((client, records)) => {
                                this.terminal_daemon = Some(client);
                                this.restore_terminal_sessions_from_records(records, true);
                                this.refresh_worktrees(cx);
                            },
                            Err(error) => {
                                this.notice =
                                    Some(format!("Failed to start daemon cleanly: {error}"));
                            },
                        }
                        cx.notify();
                    });
                },
                None => {
                    let _ = this.update(cx, |this, cx| {
                        this.notice =
                            Some("Failed to start daemon. Is arbor-httpd available?".to_owned());
                        cx.notify();
                    });
                },
            }
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

        if let Some(daemon_index) = self.pending_remote_daemon_auth.take() {
            self.daemon_auth_tokens.insert(url.clone(), token.clone());
            self.persist_daemon_auth_tokens(cx);
            if let Some(state) = self.remote_daemon_states.get(&daemon_index) {
                state.client.set_auth_token(Some(token));
            }
            self.remote_daemon_states.remove(&daemon_index);
            self.toggle_discovered_daemon(daemon_index, cx);
            return;
        }

        if let Some(client) = self.terminal_daemon.as_ref() {
            client.set_auth_token(Some(token.clone()));
        }
        if let Some(client) = self.terminal_daemon.clone() {
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_spawn(async move {
                        client
                            .list_sessions()
                            .map(|records| (client, records))
                            .map_err(|error| error.to_string())
                    })
                    .await;

                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok((_client, records)) => {
                            this.daemon_auth_tokens.insert(url, token);
                            this.persist_daemon_auth_tokens(cx);
                            this.restore_terminal_sessions_from_records(records, true);
                            this.refresh_worktrees(cx);
                            if let Some(repo_root) = this.pending_remote_create_repo_root.take() {
                                this.open_create_modal_for_connected_repo(&repo_root, cx);
                            }
                        },
                        Err(error) => {
                            if error.contains("status 401") || error.contains("status 403") {
                                this.daemon_auth_modal = Some(DaemonAuthModal {
                                    daemon_url: modal.daemon_url,
                                    token_cursor: char_count(&modal.token),
                                    token: modal.token,
                                    error: Some("Invalid token".to_owned()),
                                });
                            } else {
                                this.notice = Some(format!("connection failed: {error}"));
                            }
                        },
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        cx.notify();
    }

    fn submit_connect_to_host(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.connect_to_host_modal.take() else {
            return;
        };
        let address = modal.address.trim().to_owned();
        if address.is_empty() {
            self.connect_to_host_modal = Some(ConnectToHostModal {
                address_cursor: char_count(&modal.address),
                error: Some("Address cannot be empty".to_owned()),
                ..modal
            });
            cx.notify();
            return;
        }

        let target = match parse_connect_host_target(&address) {
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
        let label = address.clone();
        self.record_connection_history_entry(&address, None, cx);
        match target {
            ConnectHostTarget::Http { url, auth_key } => {
                self.stop_active_ssh_daemon_tunnel();
                self.connect_to_daemon_endpoint(&url, Some(label), Some(auth_key), None, false, cx);
            },
            ConnectHostTarget::Ssh { target, auth_key } => {
                self.connect_to_ssh_daemon(target, Some(label), auth_key, cx);
            },
        }
    }

    fn render_daemon_auth_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.daemon_auth_modal.as_ref() else {
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
                    .when_some(error, |this, error| {
                        this.child(div().text_xs().text_color(rgb(0xf38ba8_u32)).child(error))
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
                        "The terminal daemon (arbor-httpd) is not running. Start it to enable remote control and terminal persistence.",
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
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_daemon_modal = false;
                                    cx.notify();
                                })),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "confirm-start-daemon",
                                    "Start",
                                    ActionButtonStyle::Primary,
                                    true,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.start_daemon_modal = false;
                                    this.try_start_and_connect_daemon(cx);
                                })),
                            ),
                    ),
            )
    }

    fn render_connect_to_host_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.connect_to_host_modal.as_ref() else {
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
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Connect to Host"),
                    )
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
                                        .hover(|surface| surface.bg(rgb(theme.panel_active_bg)))
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(modal) = this.connect_to_host_modal.as_mut()
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
                                                .when(has_label, |container| {
                                                    container.child(
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
                                                .hover(|surface| {
                                                    surface
                                                        .bg(rgb(theme.panel_active_bg))
                                                        .text_color(rgb(theme.text_primary))
                                                })
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.connection_history =
                                                        connection_history::history_without_address(
                                                            &this.connection_history,
                                                            &remove_addr,
                                                        );
                                                    this.daemon_auth_tokens
                                                        .retain(|key, _| !key.contains(&*remove_addr));
                                                    this.persist_connection_history(cx);
                                                    this.persist_daemon_auth_tokens(cx);
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                }))
                                                .child("\u{f00d}"),
                                        )
                                })),
                        )
                    })
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
                                        .hover(|surface| surface.bg(rgb(theme.panel_active_bg)))
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
                    .child(div().text_xs().text_color(rgb(theme.text_muted)).child(
                        if has_history || has_daemons {
                            "Or enter an address manually:"
                        } else {
                            "Use http://HOST:PORT or ssh://[user@]HOST[:ssh_port]/"
                        },
                    ))
                    .when_some(error, |this, error| {
                        this.child(div().text_xs().text_color(rgb(0xf38ba8_u32)).child(error))
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
}
