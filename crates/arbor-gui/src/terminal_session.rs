use super::*;

impl ArborWindow {
    pub(crate) fn terminal_background_sync_interval(&self) -> Option<Duration> {
        let active_terminal_id = self.active_terminal_id_for_selected_worktree();

        self.terminals
            .iter()
            .filter_map(|session| {
                let runtime = session.runtime.as_ref()?;
                let is_active = active_terminal_id == Some(session.id);
                runtime.background_sync_interval(session, is_active)
            })
            .min()
    }

    pub(crate) fn wake_terminal_poller(&self) {
        let _ = self.terminal_poll_tx.send(());
    }

    pub(crate) fn sync_daemon_session_store(&mut self, cx: &mut Context<Self>) {
        let records = self.daemon_session_records_snapshot();
        self.daemon_session_store_save.queue(records);
        self.start_pending_daemon_session_store_save(cx);
    }

    pub(crate) fn daemon_session_records_snapshot(&self) -> Vec<DaemonSessionRecord> {
        let shell = self.embedded_shell();
        let updated_at_unix_ms = current_unix_timestamp_millis();
        self.terminals
            .iter()
            .map(|session| DaemonSessionRecord {
                session_id: session.daemon_session_id.clone().into(),
                workspace_id: session.worktree_path.display().to_string().into(),
                cwd: session.worktree_path.clone(),
                shell: if session.command.trim().is_empty() {
                    shell.clone()
                } else {
                    session.command.clone()
                },
                root_pid: session.root_pid,
                cols: session.cols.max(2),
                rows: session.rows.max(1),
                title: Some(session.title.clone()),
                last_command: session.last_command.clone(),
                output_tail: Some(terminal_output_tail_for_metadata(session, 64, 24_000)),
                exit_code: session.exit_code,
                state: Some(daemon_state_from_terminal_state(session.state)),
                updated_at_unix_ms: session.updated_at_unix_ms.or(updated_at_unix_ms),
            })
            .collect()
    }

    pub(crate) fn start_pending_daemon_session_store_save(&mut self, cx: &mut Context<Self>) {
        let Some(records) = self.daemon_session_store_save.begin_next() else {
            self.maybe_finish_quit_after_persistence_flush(cx);
            return;
        };

        let store = self.daemon_session_store.clone();
        self._daemon_session_store_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.save(&records) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.daemon_session_store_save.finish();
                if let Err(error) = result {
                    this.notice = Some(format!("failed to persist daemon sessions: {error}"));
                    cx.notify();
                }

                this.start_pending_daemon_session_store_save(cx);
                this.maybe_finish_quit_after_persistence_flush(cx);
            });
        }));
    }

    pub(crate) fn restore_terminal_sessions_from_records(
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
        let mut restored_count = 0_usize;
        let mut skipped_count = 0_usize;

        for record in records {
            if record.session_id.as_str().trim().is_empty() {
                continue;
            }

            let Some(worktree_path) = self.worktree_path_for_session_record(&record) else {
                skipped_count += 1;
                tracing::debug!(
                    session_id = %record.session_id,
                    cwd = %record.cwd.display(),
                    workspace_id = %record.workspace_id,
                    "skipping daemon session restore: no matching worktree"
                );
                continue;
            };
            let session_state = terminal_state_from_daemon_record(&record);
            let title = record
                .title
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("term-{}", self.next_terminal_id));
            let managed_process_id = managed_process_id_from_title(&worktree_path, &title);
            let command = record.shell.clone();
            let output = record.output_tail.clone().unwrap_or_default();
            let cols = record.cols.max(2);
            let rows = record.rows.max(1);

            if let Some(session) = self
                .terminals
                .iter_mut()
                .find(|session| session.daemon_session_id == record.session_id.as_str())
            {
                if session.worktree_path != worktree_path {
                    session.worktree_path = worktree_path.clone();
                    changed = true;
                }
                if session.title != title {
                    session.title = title.clone();
                    changed = true;
                }
                if session.managed_process_id != managed_process_id {
                    session.managed_process_id = managed_process_id.clone();
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
                if session.root_pid != record.root_pid {
                    session.root_pid = record.root_pid;
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
                        session.daemon_session_id.clone(),
                        session.rows,
                        session.cols,
                        Some(self.terminal_poll_tx.clone()),
                    ));
                    changed = true;
                }
            } else {
                let session_id = self.next_terminal_id;
                self.next_terminal_id += 1;
                self.terminals.push(TerminalSession {
                    id: session_id,
                    daemon_session_id: record.session_id.to_string(),
                    worktree_path: worktree_path.clone(),
                    managed_process_id,
                    title,
                    last_command: record.last_command.clone(),
                    pending_command: String::new(),
                    command,
                    agent_preset: None,
                    execution_mode: None,
                    state: session_state,
                    exit_code: record.exit_code,
                    updated_at_unix_ms: record.updated_at_unix_ms,
                    root_pid: record.root_pid,
                    cols,
                    rows,
                    generation: 0,
                    output,
                    styled_output: Vec::new(),
                    cursor: None,
                    modes: TerminalModes::default(),
                    last_runtime_sync_at: None,
                    queued_input: Vec::new(),
                    is_initializing: false,
                    runtime: attach_runtime
                        .then(|| {
                            self.terminal_daemon.as_ref().map(|daemon| {
                                local_daemon_runtime(
                                    daemon.clone(),
                                    record.session_id.to_string(),
                                    rows,
                                    cols,
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
                .find(|session| session.daemon_session_id == record.session_id.as_str())
                .map(|session| session.id);
            restored_count += 1;
            if let Some(mapped_terminal_id) = mapped_terminal_id {
                self.active_terminal_by_worktree
                    .entry(worktree_path)
                    .or_insert(mapped_terminal_id);
            }
        }

        if restored_count > 0 || skipped_count > 0 {
            tracing::info!(
                restored_count,
                skipped_count,
                total_terminals = self.terminals.len(),
                worktree_count = self.worktrees.len(),
                "terminal session restore completed"
            );
        }

        changed
    }

    fn worktree_path_for_session_record(&self, record: &DaemonSessionRecord) -> Option<PathBuf> {
        if let Some(path) = self.match_worktree_path(record.cwd.as_path()) {
            return Some(path);
        }

        let workspace_path = PathBuf::from(record.workspace_id.to_string());
        if let Some(path) = self.match_worktree_path(workspace_path.as_path()) {
            return Some(path);
        }

        // When worktrees haven't been populated yet (e.g. during startup before
        // the async refresh completes), fall back to the record's own paths so
        // that daemon sessions are still restored.  Without this, every GUI
        // restart would skip all daemon sessions and create fresh terminals —
        // causing orphaned agent processes to accumulate.
        if record.cwd.is_dir() {
            return Some(record.cwd.clone());
        }
        if workspace_path.is_dir() {
            return Some(workspace_path);
        }

        None
    }

    fn match_worktree_path(&self, candidate: &Path) -> Option<PathBuf> {
        self.worktrees
            .iter()
            .find(|worktree| paths_equivalent(worktree.path.as_path(), candidate))
            .map(|worktree| worktree.path.clone())
    }

    pub(crate) fn sync_running_terminals(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let mut should_refresh_ports = false;
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
            if outcome.changed {
                let recent_output =
                    terminal_output_tail_for_metadata(&self.terminals[index], 24, 4_000);
                if output_contains_port_hint(&recent_output) {
                    should_refresh_ports = true;
                }
            }

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
            if should_refresh_ports {
                self.refresh_worktree_ports(cx);
            }
            if should_auto_follow_terminal_output(changed, follow_output) {
                self.terminal_scroll_handle.scroll_to_bottom();
            }
            cx.notify();
        }
    }

    pub(crate) fn spawn_terminal_session_inner(
        &mut self,
        show_notice_on_missing_worktree: bool,
        cx: &mut Context<Self>,
    ) -> bool {
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
        let title = format!("term-{session_id}");
        self.terminals.push(TerminalSession {
            id: session_id,
            daemon_session_id: session_id.to_string(),
            worktree_path: cwd.clone(),
            managed_process_id: None,
            title: title.clone(),
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
        });

        let daemon = self.terminal_daemon.clone();
        let shell = self.embedded_shell();
        let target_grid_size = self.last_terminal_grid_size.unwrap_or((0, 0));
        let poll_tx = self.terminal_poll_tx.clone();
        cx.spawn(async move |this, cx| {
            enum SpawnTerminalOutcome {
                Daemon {
                    daemon: terminal_daemon_http::SharedTerminalDaemonClient,
                    record: DaemonSessionRecord,
                    notice: Option<String>,
                    clear_global_daemon: bool,
                },
                Embedded {
                    runtime: EmbeddedTerminal,
                    notice: Option<String>,
                    clear_global_daemon: bool,
                },
                Failed {
                    error: String,
                    notice: Option<String>,
                    clear_global_daemon: bool,
                },
            }

            let outcome = cx
                .background_spawn(async move {
                    let mut fallback_notice = None;
                    let mut clear_global_daemon = false;

                    if let Some(daemon) = daemon {
                        match daemon.create_or_attach(CreateOrAttachRequest {
                            session_id: String::new().into(),
                            workspace_id: cwd.display().to_string().into(),
                            cwd: cwd.clone(),
                            shell,
                            cols: 120,
                            rows: 35,
                            title: Some(title),
                            command: None,
                        }) {
                            Ok(response) => {
                                return SpawnTerminalOutcome::Daemon {
                                    daemon,
                                    record: response.session,
                                    notice: None,
                                    clear_global_daemon: false,
                                };
                            },
                            Err(error) => {
                                let error_text = error.to_string();
                                tracing::warn!(
                                    %error,
                                    "failed to create daemon terminal session, falling back to local"
                                );
                                clear_global_daemon =
                                    daemon_error_is_connection_refused(&error_text);
                                if !clear_global_daemon {
                                    fallback_notice = Some(format!(
                                        "failed to create daemon terminal session (falling back to local embedded terminal): {error}"
                                    ));
                                }
                            },
                        }
                    }

                    match terminal_backend::launch_backend(
                        backend_kind,
                        &cwd,
                        target_grid_size.0,
                        target_grid_size.1,
                    ) {
                        Ok(TerminalLaunch::Embedded(runtime)) => SpawnTerminalOutcome::Embedded {
                            runtime,
                            notice: fallback_notice,
                            clear_global_daemon,
                        },
                        Err(error) => SpawnTerminalOutcome::Failed {
                            error: error.to_string(),
                            notice: fallback_notice,
                            clear_global_daemon,
                        },
                    }
                })
                .await;

            let orphaned_daemon_session = match &outcome {
                SpawnTerminalOutcome::Daemon { daemon, record, .. } => {
                    Some((daemon.clone(), record.clone()))
                },
                _ => None,
            };
            let orphaned_daemon_session_for_update = orphaned_daemon_session.clone();

            let updated = this.update(cx, |this, cx| {
                let Some(session) = this
                    .terminals
                    .iter_mut()
                    .find(|session| session.id == session_id)
                else {
                    if let Some((daemon, record)) = orphaned_daemon_session_for_update {
                        schedule_orphaned_daemon_session_cleanup(cx, daemon, record);
                    }
                    return;
                };

                match outcome {
                    SpawnTerminalOutcome::Daemon {
                        daemon,
                        record,
                        notice,
                        clear_global_daemon,
                    } => {
                        if clear_global_daemon {
                            this.terminal_daemon = None;
                        }
                        if let Some(notice) = notice {
                            this.notice = Some(notice);
                        }
                        session.daemon_session_id = record.session_id.to_string();
                        session.title = record
                            .title
                            .clone()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| session.title.clone());
                        session.last_command = record.last_command.clone();
                        session.command = record.shell.clone();
                        session.output = record.output_tail.clone().unwrap_or_default();
                        session.state = terminal_state_from_daemon_record(&record);
                        session.exit_code = record.exit_code;
                        session.updated_at_unix_ms = record.updated_at_unix_ms;
                        session.root_pid = record.root_pid;
                        session.cols = record.cols.max(2);
                        session.rows = record.rows.max(1);
                        session.runtime = Some(local_daemon_runtime(
                            daemon,
                            record.session_id.to_string(),
                            session.rows,
                            session.cols,
                            Some(poll_tx.clone()),
                        ));
                    },
                    SpawnTerminalOutcome::Embedded {
                        runtime,
                        notice,
                        clear_global_daemon,
                    } => {
                        if clear_global_daemon {
                            this.terminal_daemon = None;
                        }
                        if let Some(notice) = notice {
                            this.notice = Some(notice);
                        }
                        session.root_pid = runtime.root_pid();
                        runtime.set_notify(poll_tx.clone());
                        session.command = "embedded shell".to_owned();
                        session.generation = runtime.generation();
                        session.runtime = Some(local_embedded_runtime(runtime));
                        session.output.clear();
                        session.styled_output.clear();
                        session.cursor = None;
                        session.exit_code = None;
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                    },
                    SpawnTerminalOutcome::Failed {
                        error,
                        notice,
                        clear_global_daemon,
                    } => {
                        if clear_global_daemon {
                            this.terminal_daemon = None;
                        }
                        if let Some(notice) = notice {
                            this.notice = Some(notice);
                        }
                        session.command = "launch backend".to_owned();
                        session.output = error.clone();
                        session.styled_output.clear();
                        session.cursor = None;
                        session.state = TerminalState::Failed;
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                        this.notice = Some(format!("terminal session failed: {error}"));
                    },
                }

                session.is_initializing = false;
                this.wake_terminal_poller();
                if let Err(error) = this.flush_queued_input_for_terminal(session_id) {
                    this.notice = Some(format!("failed to write queued terminal input: {error}"));
                }
                this.sync_daemon_session_store(cx);
                cx.notify();
            });

            if updated.is_err()
                && let Some((daemon, record)) = orphaned_daemon_session
            {
                schedule_orphaned_daemon_session_cleanup(cx, daemon, record);
            }
        })
        .detach();
        true
    }

    pub(crate) fn open_editor_in_terminal(
        &mut self,
        editor: &str,
        file_path: &Path,
        cx: &mut Context<Self>,
    ) {
        if !self.spawn_terminal_session_inner(true, cx) {
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

    pub(crate) fn spawn_terminal_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(outpost_index) = self.active_outpost_index {
            self.spawn_outpost_terminal(outpost_index, window, cx);
            return;
        }

        if !self.spawn_terminal_session_inner(true, cx) {
            cx.notify();
            return;
        }

        self.sync_daemon_session_store(cx);
        self.active_diff_session_id = None;
        self.active_file_view_session_id = None;
        self.file_view_editing = false;
        self.logs_tab_active = false;
        // Clear agent chat selection so the new terminal tab becomes active
        if let Some(worktree_path) = self.selected_worktree_path() {
            self.active_agent_chat_by_worktree
                .remove(&worktree_path.to_path_buf());
        }
        self.terminal_scroll_handle.scroll_to_bottom();
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        self.sync_ui_state_store(window, cx);
        cx.notify();
    }

    pub(crate) fn spawn_outpost_terminal(
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
        self.active_agent_chat_by_worktree.remove(&worktree_path);

        let title = format!("ssh-{}", outpost.label);
        let session = TerminalSession {
            id: session_id,
            daemon_session_id: session_id.to_string(),
            worktree_path,
            managed_process_id: None,
            title,
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
        self.active_diff_session_id = None;
        self.active_file_view_session_id = None;
        self.file_view_editing = false;
        self.logs_tab_active = false;
        self.terminal_scroll_handle.scroll_to_bottom();
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        let pool = self.ssh_connection_pool.clone();
        let remote_path = outpost.remote_path.clone();
        let poll_tx = self.terminal_poll_tx.clone();
        cx.spawn(async move |this, cx| {
            enum OutpostLaunchOutcome {
                Mosh {
                    mosh: arbor_mosh::MoshShell,
                    notice: Option<String>,
                },
                Ssh {
                    ssh_shell: SshTerminalShell,
                    notice: Option<String>,
                },
            }

            let result = cx
                .background_spawn(async move {
                    let mut fallback_notice = None;
                    if host.mosh == Some(true) && arbor_mosh::detect::local_mosh_client_available()
                    {
                        let conn_slot = pool.get_or_connect(&host).map_err(|error| {
                            format!("SSH connection failed for mosh handshake: {error}")
                        })?;
                        let guard = conn_slot
                            .lock()
                            .map_err(|_| "SSH connection lock poisoned".to_owned())?;
                        let connection = guard
                            .as_ref()
                            .ok_or_else(|| "SSH connection not available".to_owned())?;
                        match arbor_mosh::handshake::start_mosh_server(connection, &host)
                            .map_err(|error| {
                                format!("mosh handshake failed, falling back to SSH: {error}")
                            })
                            .and_then(|handshake| {
                                arbor_mosh::MoshShell::spawn(handshake, 120, 35).map_err(|error| {
                                    format!("mosh-client failed, falling back to SSH: {error}")
                                })
                            }) {
                            Ok(mosh) => {
                                return Ok(OutpostLaunchOutcome::Mosh { mosh, notice: None });
                            },
                            Err(error) => fallback_notice = Some(error),
                        }
                    } else if host.mosh == Some(true) {
                        fallback_notice = Some(
                            "mosh-client not found locally, falling back to SSH shell".to_owned(),
                        );
                    }

                    let conn_slot = pool
                        .get_or_connect(&host)
                        .map_err(|error| format!("SSH connection failed: {error}"))?;
                    let guard = conn_slot
                        .lock()
                        .map_err(|_| "SSH connection lock poisoned".to_owned())?;
                    let connection = guard
                        .as_ref()
                        .ok_or_else(|| "SSH connection not available".to_owned())?;
                    let ssh_shell = SshTerminalShell::open(connection, 120, 35, &remote_path)
                        .map_err(|error| format!("SSH shell failed: {error}"))?;
                    Ok::<OutpostLaunchOutcome, String>(OutpostLaunchOutcome::Ssh {
                        ssh_shell,
                        notice: fallback_notice,
                    })
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
                    Ok(OutpostLaunchOutcome::Mosh { mosh, notice }) => {
                        if let Some(notice) = notice {
                            this.notice = Some(notice);
                        }
                        mosh.set_notify(poll_tx.clone());
                        session.command = "mosh".to_owned();
                        session.generation = mosh.generation();
                        session.runtime = Some(outpost_mosh_runtime(mosh));
                    },
                    Ok(OutpostLaunchOutcome::Ssh { ssh_shell, notice }) => {
                        if let Some(notice) = notice {
                            this.notice = Some(notice);
                        }
                        session.command = "ssh".to_owned();
                        session.generation = ssh_shell.generation();
                        session.runtime = Some(outpost_ssh_runtime(ssh_shell));
                    },
                    Err(error) => {
                        session.state = TerminalState::Failed;
                        session.output = error.clone();
                        this.notice = Some(error);
                    },
                }
                session.is_initializing = false;
                this.wake_terminal_poller();
                if let Err(error) = this.flush_queued_input_for_terminal(session_id) {
                    this.notice = Some(format!("failed to write queued terminal input: {error}"));
                }
                this.sync_daemon_session_store(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn select_terminal(
        &mut self,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        if let Some(session) = self
            .terminals
            .iter()
            .find(|session| session.id == session_id)
        {
            if let Some(preset) = session.agent_preset {
                self.active_preset_tab = Some(preset);
            }
            if let Some(mode) = session.execution_mode {
                self.execution_mode = mode;
            }
        }
        self.active_diff_session_id = None;
        self.active_file_view_session_id = None;
        // Clear agent chat selection so terminal tab takes priority
        if let Some(wt_path) = self.selected_worktree_path().map(Path::to_path_buf) {
            self.active_agent_chat_by_worktree.remove(&wt_path);
        }
        self.logs_tab_active = false;
        self.sync_navigation_ui_state_store(cx);
        self.terminal_scroll_handle.scroll_to_bottom();
        self.wake_terminal_poller();
        window.focus(&self.terminal_focus);
        self.focus_terminal_on_next_render = false;
        cx.notify();
    }
}
