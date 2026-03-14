fn worktree_notes_load_is_current(
    current_selection_epoch: usize,
    load_selection_epoch: usize,
    current_path: Option<&Path>,
    load_path: &Path,
    current_edit_generation: u64,
    load_edit_generation: u64,
) -> bool {
    current_selection_epoch == load_selection_epoch
        && current_path == Some(load_path)
        && current_edit_generation == load_edit_generation
}

impl ArborWindow {
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

    fn managed_process_session(
        &self,
        worktree_path: &Path,
        process_id: &str,
    ) -> Option<&TerminalSession> {
        self.terminals.iter().find(|session| {
            session.worktree_path.as_path() == worktree_path
                && session.managed_process_id.as_deref() == Some(process_id)
        })
    }

    fn start_managed_process_for_worktree(
        &mut self,
        worktree_index: usize,
        process_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((worktree_path, process)) = self
            .worktrees
            .get(worktree_index)
            .and_then(|worktree| {
                worktree
                    .managed_processes
                    .iter()
                    .find(|process| process.id == process_id)
                    .cloned()
                    .map(|process| (worktree.path.clone(), process))
            })
        else {
            self.notice = Some(format!("managed process `{process_id}` not found"));
            cx.notify();
            return;
        };

        let existing_session = self
            .managed_process_session(&worktree_path, &process.id)
            .map(|session| (session.id, managed_process_session_is_active(session)));
        let session_id = match existing_session {
            Some((session_id, true)) => session_id,
            Some((session_id, false)) => {
                let _ = self.close_terminal_session_by_id(session_id);
                self.spawn_managed_process_session(worktree_path, process, cx)
            },
            None => self.spawn_managed_process_session(worktree_path, process, cx),
        };

        self.select_terminal(session_id, window, cx);
        self.sync_daemon_session_store(cx);
        cx.notify();
    }

    fn restart_managed_process_for_worktree(
        &mut self,
        worktree_index: usize,
        process_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((worktree_path, process)) = self
            .worktrees
            .get(worktree_index)
            .and_then(|worktree| {
                worktree
                    .managed_processes
                    .iter()
                    .find(|process| process.id == process_id)
                    .cloned()
                    .map(|process| (worktree.path.clone(), process))
            })
        else {
            self.notice = Some(format!("managed process `{process_id}` not found"));
            cx.notify();
            return;
        };

        if let Some(session_id) = self
            .managed_process_session(&worktree_path, &process.id)
            .map(|session| session.id)
        {
            self.close_terminal_session_by_id(session_id);
        }

        let session_id = self.spawn_managed_process_session(worktree_path, process, cx);
        self.select_terminal(session_id, window, cx);
        self.sync_daemon_session_store(cx);
        cx.notify();
    }

    fn stop_managed_process_for_worktree(
        &mut self,
        worktree_index: usize,
        process_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree_path) = self
            .worktrees
            .get(worktree_index)
            .map(|worktree| worktree.path.clone())
        else {
            return;
        };

        let Some(session_id) = self
            .managed_process_session(&worktree_path, process_id)
            .map(|session| session.id)
        else {
            return;
        };

        if self.close_terminal_session_by_id(session_id) {
            self.sync_daemon_session_store(cx);
            cx.notify();
        }
    }

    fn spawn_managed_process_session(
        &mut self,
        worktree_path: PathBuf,
        process: ManagedWorktreeProcess,
        cx: &mut Context<Self>,
    ) -> u64 {
        let session_id = self.next_terminal_id;
        self.next_terminal_id += 1;
        self.active_terminal_by_worktree
            .insert(worktree_path.clone(), session_id);

        let title = managed_process_title(process.source, &process.name);
        let process_id = process.id.clone();
        let process_name_for_spawn = process.name.clone();
        let process_name_for_update = process.name.clone();
        let process_command_for_spawn = process.command.clone();
        let process_command_for_update = process.command.clone();
        let process_working_dir = process.working_dir.clone();
        self.terminals.push(TerminalSession {
            id: session_id,
            daemon_session_id: session_id.to_string(),
            worktree_path: worktree_path.clone(),
            managed_process_id: Some(process_id),
            title: title.clone(),
            last_command: None,
            pending_command: String::new(),
            command: process_command_for_update.clone(),
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
        let poll_tx = self.terminal_poll_tx.clone();
        cx.spawn(async move |this, cx| {
            enum SpawnManagedProcessOutcome {
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
                            workspace_id: worktree_path.display().to_string().into(),
                            cwd: process_working_dir.clone(),
                            shell,
                            cols: 120,
                            rows: 35,
                            title: Some(title.clone()),
                            command: Some(process_command_for_spawn.clone()),
                        }) {
                            Ok(response) => {
                                return SpawnManagedProcessOutcome::Daemon {
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
                                    process = %process_name_for_spawn,
                                    worktree = %worktree_path.display(),
                                    "failed to create daemon-managed process session, falling back to embedded",
                                );
                                clear_global_daemon =
                                    daemon_error_is_connection_refused(&error_text);
                                if !clear_global_daemon {
                                    fallback_notice = Some(format!(
                                        "failed to create managed process in daemon (falling back to embedded): {error}",
                                    ));
                                }
                            },
                        }
                    }

                    match EmbeddedTerminal::spawn_command(
                        &process_working_dir,
                        &process_command_for_spawn,
                        35,
                        120,
                    )
                    {
                        Ok(runtime) => SpawnManagedProcessOutcome::Embedded {
                            runtime,
                            notice: fallback_notice,
                            clear_global_daemon,
                        },
                        Err(error) => SpawnManagedProcessOutcome::Failed {
                            error,
                            notice: fallback_notice,
                            clear_global_daemon,
                        },
                    }
                })
                .await;

            let orphaned_daemon_session = match &outcome {
                SpawnManagedProcessOutcome::Daemon { daemon, record, .. } => {
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
                    SpawnManagedProcessOutcome::Daemon {
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
                        session.command = process_command_for_update.clone();
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
                    SpawnManagedProcessOutcome::Embedded {
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
                        session.command = process_command_for_update.clone();
                        session.generation = runtime.generation();
                        session.runtime = Some(local_embedded_runtime(runtime));
                        session.output.clear();
                        session.styled_output.clear();
                        session.cursor = None;
                        session.exit_code = None;
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                    },
                    SpawnManagedProcessOutcome::Failed {
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
                        session.command = process_command_for_update.clone();
                        session.output = error.clone();
                        session.styled_output.clear();
                        session.cursor = None;
                        session.state = TerminalState::Failed;
                        session.updated_at_unix_ms = current_unix_timestamp_millis();
                        this.notice = Some(format!(
                            "managed process `{}` failed to start: {error}",
                            process_name_for_update
                        ));
                    },
                }

                session.is_initializing = false;
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

        session_id
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

    fn ensure_selected_worktree_terminal(&mut self, cx: &mut Context<Self>) -> bool {
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
        let created_terminal = selected_worktree_terminal_was_created(has_terminal, || {
            self.spawn_terminal_session_inner(false, cx)
        });

        if !created_terminal
            && let Some(session_id) = self.active_terminal_id_for_worktree(&worktree_path)
        {
            self.active_terminal_by_worktree
                .insert(worktree_path, session_id);
        }

        created_terminal
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
        self.sync_selected_worktree_notes(cx);
        self.focus_terminal_on_next_render = true;
        cx.notify();
    }

    fn persist_repositories(&mut self, cx: &mut Context<Self>) {
        self.repository_entries_save.queue(
            repository_store::repository_entries_from_summaries(&self.repositories),
        );
        self.start_pending_repository_entries_save(cx);
    }

    fn persist_github_auth_state(&mut self, cx: &mut Context<Self>) {
        self.github_auth_state_save
            .queue(self.github_auth_state.clone());
        self.start_pending_github_auth_state_save(cx);
    }

    fn start_pending_repository_entries_save(&mut self, cx: &mut Context<Self>) {
        let Some(entries_to_save) = self.repository_entries_save.begin_next() else {
            self.maybe_finish_quit_after_persistence_flush(cx);
            return;
        };

        let store = self.repository_store.clone();
        self._repository_entries_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.save_entries(&entries_to_save) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.repository_entries_save.finish();
                if let Err(error) = result {
                    this.notice = Some(format!("failed to save repositories: {error}"));
                    cx.notify();
                }

                this.start_pending_repository_entries_save(cx);
                this.maybe_finish_quit_after_persistence_flush(cx);
            });
        }));
    }

    fn start_pending_github_auth_state_save(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.github_auth_state_save.begin_next() else {
            self.maybe_finish_quit_after_persistence_flush(cx);
            return;
        };

        let store = self.github_auth_store.clone();
        self._github_auth_state_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.save(&state) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.github_auth_state_save.finish();
                if let Err(error) = result {
                    this.notice =
                        Some(format!("failed to persist GitHub auth state: {error}"));
                    cx.notify();
                }

                this.start_pending_github_auth_state_save(cx);
                this.maybe_finish_quit_after_persistence_flush(cx);
            });
        }));
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

    fn open_external_url(&mut self, url: &str, cx: &mut Context<Self>) {
        cx.open_url(url);
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

    fn has_persisted_github_token(&self) -> bool {
        self.github_auth_state
            .access_token
            .as_deref()
            .and_then(non_empty_trimmed_str)
            .is_some()
    }

    fn refresh_github_auth_identity(&mut self, cx: &mut Context<Self>) {
        let Some(token) = self.github_access_token() else {
            return;
        };
        if self.github_auth_in_progress
            || (self.github_auth_state.user_login.is_some()
                && self.github_auth_state.user_avatar_url.is_some())
        {
            return;
        }

        cx.spawn(async move |this, cx| {
            let identity = cx
                .background_spawn(async move { github_authenticated_user(Some(&token)) })
                .await;

            let _ = this.update(cx, |this, cx| {
                let Some((login, avatar_url)) = identity else {
                    return;
                };
                if this.github_auth_state.user_login.as_deref() == Some(login.as_str())
                    && this.github_auth_state.user_avatar_url == avatar_url
                {
                    return;
                }

                this.github_auth_state.user_login = Some(login);
                this.github_auth_state.user_avatar_url = avatar_url;
                this.persist_github_auth_state(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn github_access_token(&self) -> Option<String> {
        resolve_github_access_token(self.github_auth_state.access_token.as_deref())
    }

    fn clear_saved_github_token(&mut self, cx: &mut Context<Self>) {
        if !self.has_persisted_github_token() {
            self.notice = Some("no saved GitHub session to disconnect".to_owned());
            cx.notify();
            return;
        }

        self.github_auth_state = github_auth_store::GithubAuthState::default();
        self.notice = Some("disconnected from GitHub".to_owned());
        self.refresh_worktree_pull_requests(cx);
        self.persist_github_auth_state(cx);
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
                    github_poll_device_access_token(&client_id, &device_code).map(|token| {
                        let identity =
                            github_authenticated_user(Some(token.access_token.as_str()));
                        (token, identity)
                    })
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.github_auth_in_progress = false;
                this.github_auth_modal = None;
                this.github_auth_copy_feedback_active = false;
                match poll_result {
                    Ok((token, identity)) => {
                        this.github_auth_state = github_auth_store::GithubAuthState {
                            access_token: Some(token.access_token),
                            token_type: token.token_type,
                            scope: token.scope,
                            user_login: identity.as_ref().map(|(login, _)| login.clone()),
                            user_avatar_url: identity.and_then(|(_, avatar_url)| avatar_url),
                        };

                        this.refresh_worktree_pull_requests(cx);
                        this.notice = Some(
                            "GitHub connected, pull request numbers will refresh automatically"
                                .to_owned(),
                        );
                        this.persist_github_auth_state(cx);
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

    fn refresh_top_bar_external_launchers(&mut self, cx: &mut Context<Self>) {
        let next_epoch = self.launcher_refresh_epoch.wrapping_add(1);
        self.launcher_refresh_epoch = next_epoch;
        self._launcher_refresh_task = Some(cx.spawn(async move |this, cx| {
            let (ide_launchers, terminal_launchers) = cx
                .background_spawn(async move {
                    (detect_ide_launchers(), detect_terminal_launchers())
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.launcher_refresh_epoch != next_epoch {
                    return;
                }
                this.ide_launchers = ide_launchers;
                this.terminal_launchers = terminal_launchers;
                if this.top_bar_quick_actions_open {
                    cx.notify();
                }
            });
        }));
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
            self.refresh_top_bar_external_launchers(cx);
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
        self.refresh_repo_config_if_changed(cx);
        self.refresh_changed_files(cx);
        self.sync_selected_worktree_notes(cx);
        self.expanded_dirs.clear();
        self.selected_file_tree_entry = None;
        self.file_tree_entries.clear();
        if self.right_pane_tab == RightPaneTab::FileTree {
            self.rebuild_file_tree(cx);
        }
        if self.ensure_selected_worktree_terminal(cx) {
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
        self.sync_selected_worktree_notes(cx);
        self.refresh_remote_changed_files(cx);
        cx.notify();
    }

    fn refresh_changed_files(&mut self, cx: &mut Context<Self>) {
        if self.active_outpost_index.is_some() {
            self.changed_files.clear();
            self.selected_changed_file = None;
            cx.notify();
            return;
        }

        let Some(path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.changed_files.clear();
            self.selected_changed_file = None;
            cx.notify();
            return;
        };
        let result_path = path.clone();
        let selection_epoch = self.worktree_selection_epoch;
        self._changed_files_refresh_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    changes::changed_files(&path).map_err(|error| error.to_string())
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.active_outpost_index.is_some()
                    || this.worktree_selection_epoch != selection_epoch
                    || this.selected_worktree_path() != Some(result_path.as_path())
                {
                    return;
                }

                match result {
                    Ok(files) => {
                        this.changed_files = files;
                        if this
                            .notice
                            .as_deref()
                            .is_some_and(|notice| notice.starts_with("failed to load changed files"))
                        {
                            this.notice = None;
                        }
                    },
                    Err(error) => {
                        this.changed_files.clear();
                        this.notice =
                            Some(format!("failed to load changed files with gix: {error}"));
                    },
                }

                this.sync_selected_changed_file();
                cx.notify();
            });
        }));
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

    fn rebuild_file_tree(&mut self, cx: &mut Context<Self>) {
        let Some(worktree_path) = self.selected_worktree_path().map(|p| p.to_path_buf()) else {
            self.file_tree_entries.clear();
            self.file_tree_loading = false;
            return;
        };
        let expanded_dirs = self.expanded_dirs.clone();
        let result_worktree_path = worktree_path.clone();
        let selection_epoch = self.worktree_selection_epoch;
        self.file_tree_loading = true;
        self._file_tree_refresh_task = Some(cx.spawn(async move |this, cx| {
            let entries = cx
                .background_spawn(async move {
                    let mut entries = Vec::new();
                    collect_file_tree_entries(
                        &worktree_path,
                        &worktree_path,
                        0,
                        &expanded_dirs,
                        &mut entries,
                    );
                    entries
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.worktree_selection_epoch != selection_epoch
                    || this.selected_worktree_path() != Some(result_worktree_path.as_path())
                {
                    return;
                }
                this.file_tree_entries = entries;
                this.file_tree_loading = false;
                cx.notify();
            });
        }));
    }

    fn toggle_file_tree_dir(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.expanded_dirs.contains(&path) {
            self.expanded_dirs.remove(&path);
        } else {
            self.expanded_dirs.insert(path.clone());
        }
        self.selected_file_tree_entry = Some(path);
        self.rebuild_file_tree(cx);
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
            self.rebuild_file_tree(cx);
        }
        if tab != RightPaneTab::Notes {
            self.worktree_notes_active = false;
        }
        cx.notify();
    }

    fn sync_selected_worktree_notes(&mut self, cx: &mut Context<Self>) {
        let Some(worktree_path) = self.selected_local_worktree_path().map(Path::to_path_buf) else {
            if self.worktree_notes_path.is_none() {
                return;
            }
            self.worktree_notes_active = false;
            self.worktree_notes_error = None;
            self.worktree_notes_cursor = FileViewCursor { line: 0, col: 0 };
            self.worktree_notes_lines = vec![String::new()];
            self.worktree_notes_path = None;
            return;
        };

        let notes_path = worktree_notes_storage_path(&worktree_path);
        if self.worktree_notes_path.as_ref() == Some(&notes_path) {
            return;
        }

        self.worktree_notes_active = false;
        self.worktree_notes_error = None;
        self.worktree_notes_cursor = FileViewCursor { line: 0, col: 0 };
        self.worktree_notes_path = Some(notes_path.clone());
        self.worktree_notes_lines = vec![String::new()];
        let result_notes_path = notes_path.clone();
        let selection_epoch = self.worktree_selection_epoch;
        let notes_edit_generation = self.worktree_notes_edit_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    fs::read_to_string(&notes_path)
                        .map(Some)
                        .or_else(|error| match error.kind() {
                            std::io::ErrorKind::NotFound => Ok(None),
                            _ => Err(error.to_string()),
                        })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if !worktree_notes_load_is_current(
                    this.worktree_selection_epoch,
                    selection_epoch,
                    this.worktree_notes_path.as_deref(),
                    result_notes_path.as_path(),
                    this.worktree_notes_edit_generation,
                    notes_edit_generation,
                )
                {
                    return;
                }

                match result {
                    Ok(Some(content)) => {
                        let mut lines: Vec<String> =
                            content.lines().map(ToOwned::to_owned).collect();
                        if lines.is_empty() {
                            lines.push(String::new());
                        }
                        let last_line = lines.len().saturating_sub(1);
                        let last_col = lines[last_line].chars().count();
                        this.worktree_notes_lines = lines;
                        this.worktree_notes_cursor = FileViewCursor {
                            line: last_line,
                            col: last_col,
                        };
                        this.worktree_notes_error = None;
                    },
                    Ok(None) => {
                        this.worktree_notes_lines = vec![String::new()];
                        this.worktree_notes_error = None;
                    },
                    Err(error) => {
                        this.worktree_notes_lines = vec![String::new()];
                        this.worktree_notes_error = Some(format!("failed to load notes: {error}"));
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_selected_worktree_notes(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.worktree_notes_path.clone() else {
            return;
        };
        let content = self.worktree_notes_lines.join("\n");
        self.worktree_notes_save_pending = true;
        self._worktree_notes_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    std::thread::sleep(Duration::from_millis(150));
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).map_err(|error| {
                            format!("failed to create notes directory: {error}")
                        })?;
                    }
                    fs::write(&path, content)
                        .map_err(|error| format!("failed to save notes: {error}"))?;
                    Ok::<PathBuf, String>(path)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.worktree_notes_save_pending = false;
                match result {
                    Ok(saved_path) => {
                        if this.worktree_notes_path.as_ref() == Some(&saved_path) {
                            this.worktree_notes_error = None;
                        }
                    },
                    Err(error) => {
                        this.worktree_notes_error = Some(error);
                    },
                }
                this.maybe_finish_quit_after_persistence_flush(cx);
                cx.notify();
            });
        }));
    }

    fn insert_text_into_selected_worktree_notes(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.worktree_notes_lines.is_empty() {
            self.worktree_notes_lines.push(String::new());
        }

        for character in text.chars() {
            if character == '\n' {
                let line = &self.worktree_notes_lines[self.worktree_notes_cursor.line];
                let byte_pos = char_to_byte_offset(line, self.worktree_notes_cursor.col);
                let trailing = line[byte_pos..].to_owned();
                self.worktree_notes_lines[self.worktree_notes_cursor.line].truncate(byte_pos);
                self.worktree_notes_cursor.line += 1;
                self.worktree_notes_cursor.col = 0;
                self.worktree_notes_lines
                    .insert(self.worktree_notes_cursor.line, trailing);
                continue;
            }

            let line = &mut self.worktree_notes_lines[self.worktree_notes_cursor.line];
            let byte_pos = char_to_byte_offset(line, self.worktree_notes_cursor.col);
            line.insert(byte_pos, character);
            self.worktree_notes_cursor.col += 1;
        }

        self.worktree_notes_edit_generation = self.worktree_notes_edit_generation.wrapping_add(1);
        self.save_selected_worktree_notes(cx);
    }

    fn handle_worktree_notes_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.worktree_notes_active || self.right_pane_tab != RightPaneTab::Notes {
            return false;
        }
        if self.worktree_notes_lines.is_empty() {
            self.worktree_notes_lines.push(String::new());
        }
        if event.keystroke.modifiers.platform {
            return false;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.worktree_notes_active = false;
                cx.notify();
                return true;
            },
            "backspace" => {
                let mut changed = false;
                if self.worktree_notes_cursor.col > 0 {
                    let line = &mut self.worktree_notes_lines[self.worktree_notes_cursor.line];
                    let byte_pos = char_to_byte_offset(line, self.worktree_notes_cursor.col);
                    let prev_byte =
                        char_to_byte_offset(line, self.worktree_notes_cursor.col.saturating_sub(1));
                    line.replace_range(prev_byte..byte_pos, "");
                    self.worktree_notes_cursor.col -= 1;
                    changed = true;
                } else if self.worktree_notes_cursor.line > 0 {
                    let removed = self.worktree_notes_lines.remove(self.worktree_notes_cursor.line);
                    self.worktree_notes_cursor.line -= 1;
                    let previous = &mut self.worktree_notes_lines[self.worktree_notes_cursor.line];
                    self.worktree_notes_cursor.col = previous.chars().count();
                    previous.push_str(&removed);
                    changed = true;
                }
                if changed {
                    self.worktree_notes_edit_generation =
                        self.worktree_notes_edit_generation.wrapping_add(1);
                    self.save_selected_worktree_notes(cx);
                }
                cx.notify();
                return true;
            },
            "delete" => {
                let mut changed = false;
                let line_len = self.worktree_notes_lines[self.worktree_notes_cursor.line]
                    .chars()
                    .count();
                if self.worktree_notes_cursor.col < line_len {
                    let line = &mut self.worktree_notes_lines[self.worktree_notes_cursor.line];
                    let byte_pos = char_to_byte_offset(line, self.worktree_notes_cursor.col);
                    let next_byte = char_to_byte_offset(line, self.worktree_notes_cursor.col + 1);
                    line.replace_range(byte_pos..next_byte, "");
                    changed = true;
                } else if self.worktree_notes_cursor.line + 1 < self.worktree_notes_lines.len() {
                    let next = self
                        .worktree_notes_lines
                        .remove(self.worktree_notes_cursor.line + 1);
                    self.worktree_notes_lines[self.worktree_notes_cursor.line].push_str(&next);
                    changed = true;
                }
                if changed {
                    self.worktree_notes_edit_generation =
                        self.worktree_notes_edit_generation.wrapping_add(1);
                    self.save_selected_worktree_notes(cx);
                }
                cx.notify();
                return true;
            },
            "enter" | "return" => {
                self.insert_text_into_selected_worktree_notes("\n", cx);
                cx.notify();
                return true;
            },
            "left" => {
                if self.worktree_notes_cursor.col > 0 {
                    self.worktree_notes_cursor.col -= 1;
                } else if self.worktree_notes_cursor.line > 0 {
                    self.worktree_notes_cursor.line -= 1;
                    self.worktree_notes_cursor.col = self.worktree_notes_lines
                        [self.worktree_notes_cursor.line]
                        .chars()
                        .count();
                }
                cx.notify();
                return true;
            },
            "right" => {
                let line_len = self.worktree_notes_lines[self.worktree_notes_cursor.line]
                    .chars()
                    .count();
                if self.worktree_notes_cursor.col < line_len {
                    self.worktree_notes_cursor.col += 1;
                } else if self.worktree_notes_cursor.line + 1 < self.worktree_notes_lines.len() {
                    self.worktree_notes_cursor.line += 1;
                    self.worktree_notes_cursor.col = 0;
                }
                cx.notify();
                return true;
            },
            "up" => {
                if self.worktree_notes_cursor.line > 0 {
                    self.worktree_notes_cursor.line -= 1;
                    self.worktree_notes_cursor.col = self.worktree_notes_cursor.col.min(
                        self.worktree_notes_lines[self.worktree_notes_cursor.line]
                            .chars()
                            .count(),
                    );
                }
                cx.notify();
                return true;
            },
            "down" => {
                if self.worktree_notes_cursor.line + 1 < self.worktree_notes_lines.len() {
                    self.worktree_notes_cursor.line += 1;
                    self.worktree_notes_cursor.col = self.worktree_notes_cursor.col.min(
                        self.worktree_notes_lines[self.worktree_notes_cursor.line]
                            .chars()
                            .count(),
                    );
                }
                cx.notify();
                return true;
            },
            "home" => {
                self.worktree_notes_cursor.col = 0;
                cx.notify();
                return true;
            },
            "end" => {
                self.worktree_notes_cursor.col = self.worktree_notes_lines
                    [self.worktree_notes_cursor.line]
                    .chars()
                    .count();
                cx.notify();
                return true;
            },
            "tab" => {
                self.insert_text_into_selected_worktree_notes("    ", cx);
                cx.notify();
                return true;
            },
            _ => {},
        }

        if event.keystroke.modifiers.control || event.keystroke.modifiers.alt {
            return false;
        }
        if let Some(text) = event.keystroke.key_char.as_deref() {
            self.insert_text_into_selected_worktree_notes(text, cx);
            cx.notify();
            return true;
        }

        false
    }
}

fn collect_file_tree_entries(
    base: &Path,
    dir: &Path,
    depth: usize,
    expanded_dirs: &HashSet<PathBuf>,
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
        let is_dir = entry.file_type().is_ok_and(|file_type| file_type.is_dir());
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

    children.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| left.0.to_lowercase().cmp(&right.0.to_lowercase()))
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
        if is_dir && expanded_dirs.contains(&relative) {
            collect_file_tree_entries(base, &full_path, depth + 1, expanded_dirs, entries);
        }
    }
}

fn selected_worktree_terminal_was_created<F>(has_terminal: bool, spawn_terminal: F) -> bool
where
    F: FnOnce() -> bool,
{
    if has_terminal {
        false
    } else {
        spawn_terminal()
    }
}
