use super::*;

impl ArborWindow {
    pub(crate) fn open_commit_modal(&mut self, cx: &mut Context<Self>) {
        if self.git_action_in_flight.is_some() {
            return;
        }

        if self.active_outpost_index.is_some() {
            self.notice = Some("git actions are only available for local worktrees".to_owned());
            cx.notify();
            return;
        }

        let Some(_) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a worktree before committing".to_owned());
            cx.notify();
            return;
        };

        if self.changed_files.is_empty() {
            self.notice = Some("nothing to commit".to_owned());
            cx.notify();
            return;
        }

        let initial_message = default_commit_message(&self.changed_files);
        self.commit_modal = Some(CommitModal {
            message_cursor: char_count(&initial_message),
            message: initial_message,
            generating: false,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_commit_modal(&mut self, cx: &mut Context<Self>) {
        self.commit_modal = None;
        cx.notify();
    }

    pub(crate) fn submit_commit_modal(&mut self, cx: &mut Context<Self>) {
        self.submit_commit_modal_with_follow_up(false, cx);
    }

    pub(crate) fn submit_commit_modal_and_create_pr(&mut self, cx: &mut Context<Self>) {
        self.submit_commit_modal_with_follow_up(true, cx);
    }

    pub(crate) fn submit_commit_modal_with_follow_up(
        &mut self,
        create_pull_request: bool,
        cx: &mut Context<Self>,
    ) {
        if self.git_action_in_flight.is_some() {
            return;
        }

        let Some(modal) = self.commit_modal.as_ref() else {
            return;
        };

        if create_pull_request && self.selected_local_worktree_has_pull_request() {
            if let Some(modal) = self.commit_modal.as_mut() {
                modal.error = Some("This worktree already has a pull request.".to_owned());
            }
            cx.notify();
            return;
        }

        let message = modal.message.trim().to_owned();
        if message.is_empty() {
            if let Some(modal) = self.commit_modal.as_mut() {
                modal.error = Some("Commit message is required.".to_owned());
            }
            cx.notify();
            return;
        }

        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a worktree before committing".to_owned());
            cx.notify();
            return;
        };

        let changed_files = self.changed_files.clone();
        if !create_pull_request {
            self.git_action_in_flight = Some(GitActionKind::Commit);
            self.notice = Some("running git commit".to_owned());
            cx.spawn(async move |this, cx| {
                let result = cx.background_spawn(async move {
                    run_git_commit_for_worktree(worktree_path.as_path(), &changed_files, &message)
                });
                let result = result.await;

                let _ = this.update(cx, |this, cx| {
                    this.git_action_in_flight = None;
                    match result {
                        Ok(message) => {
                            this.commit_modal = None;
                            this.notice = Some(message);
                            this.refresh_changed_files(cx);
                            this.refresh_worktree_diff_summaries(cx);
                            this.refresh_worktree_pull_requests(cx);
                        },
                        Err(error) => {
                            if let Some(modal) = this.commit_modal.as_mut() {
                                modal.error = Some(error.to_string());
                            } else {
                                this.notice = Some("failed to create commit".to_owned());
                            }
                        },
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }

        let repo_slug = self
            .github_repo_slug
            .clone()
            .or_else(|| github_repo_slug_for_repo(worktree_path.as_path()));
        let github_token = self.github_access_token();
        let github_service = self.github_service.clone();

        self.git_action_in_flight = Some(GitActionKind::CommitPushCreatePullRequest);
        self.notice = Some("committing changes…".to_owned());
        cx.notify();

        enum StackedGitActionProgress {
            Status(String),
            Done(Result<String, StackedGitActionFailure>),
        }

        cx.spawn(async move |this, cx| {
            let (progress_tx, progress_rx) = smol::channel::unbounded::<StackedGitActionProgress>();

            cx.background_spawn(async move {
                let _ = progress_tx.send_blocking(StackedGitActionProgress::Status(
                    "committing changes…".to_owned(),
                ));

                let commit_message = match run_git_commit_for_worktree(
                    worktree_path.as_path(),
                    &changed_files,
                    &message,
                ) {
                    Ok(message) => message,
                    Err(error) => {
                        let _ = progress_tx.send_blocking(StackedGitActionProgress::Done(Err(
                            StackedGitActionFailure::Commit(error.to_string()),
                        )));
                        return;
                    },
                };

                let _ = progress_tx.send_blocking(StackedGitActionProgress::Status(
                    "pushing branch…".to_owned(),
                ));
                let push_message = match run_git_push_for_worktree(worktree_path.as_path()) {
                    Ok(message) => message,
                    Err(error) => {
                        let _ = progress_tx.send_blocking(StackedGitActionProgress::Done(Err(
                            StackedGitActionFailure::Push {
                                commit_message,
                                error: error.to_string(),
                            },
                        )));
                        return;
                    },
                };

                let _ = progress_tx.send_blocking(StackedGitActionProgress::Status(
                    "creating pull request…".to_owned(),
                ));
                let pr_message = match run_create_pr_for_worktree(
                    github_service.as_ref(),
                    worktree_path.as_path(),
                    repo_slug.as_deref(),
                    github_token.as_deref(),
                ) {
                    Ok(message) => message,
                    Err(error) => {
                        let _ = progress_tx.send_blocking(StackedGitActionProgress::Done(Err(
                            StackedGitActionFailure::CreatePullRequest {
                                commit_message,
                                push_message,
                                error: error.to_string(),
                            },
                        )));
                        return;
                    },
                };

                let summary = format!(
                    "{}; {}; {}",
                    commit_message, push_message, pr_message
                );
                let _ = progress_tx
                    .send_blocking(StackedGitActionProgress::Done(Ok(summary)));
            })
            .detach();

            while let Ok(progress) = progress_rx.recv().await {
                let should_stop = matches!(progress, StackedGitActionProgress::Done(_));
                let _ = this.update(cx, |this, cx| {
                    match progress {
                        StackedGitActionProgress::Status(status) => {
                            this.notice = Some(status);
                        },
                        StackedGitActionProgress::Done(result) => {
                            this.git_action_in_flight = None;
                            match result {
                                Ok(message) => {
                                    this.commit_modal = None;
                                    if let Some(url) = extract_first_url(&message) {
                                        this.open_external_url(&url, cx);
                                    }
                                    this.notice = Some(message);
                                    this.refresh_changed_files(cx);
                                    this.refresh_worktree_diff_summaries(cx);
                                    this.refresh_worktree_pull_requests(cx);
                                },
                                Err(StackedGitActionFailure::Commit(error)) => {
                                    if let Some(modal) = this.commit_modal.as_mut() {
                                        modal.error = Some(error);
                                    } else {
                                        this.notice = Some("failed to create commit".to_owned());
                                    }
                                },
                                Err(StackedGitActionFailure::Push {
                                    commit_message,
                                    error,
                                }) => {
                                    this.commit_modal = None;
                                    this.notice = Some(format!(
                                        "{commit_message}; push failed: {error}"
                                    ));
                                    this.refresh_changed_files(cx);
                                    this.refresh_worktree_diff_summaries(cx);
                                    this.refresh_worktree_pull_requests(cx);
                                },
                                Err(StackedGitActionFailure::CreatePullRequest {
                                    commit_message,
                                    push_message,
                                    error,
                                }) => {
                                    this.commit_modal = None;
                                    this.notice = Some(format!(
                                        "{commit_message}; {push_message}; PR creation failed: {error}"
                                    ));
                                    this.refresh_changed_files(cx);
                                    this.refresh_worktree_diff_summaries(cx);
                                    this.refresh_worktree_pull_requests(cx);
                                },
                            }
                        },
                    }
                    cx.notify();
                });

                if should_stop {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn generate_commit_message_with_ai(&mut self, cx: &mut Context<Self>) {
        if self.git_action_in_flight.is_some() {
            return;
        }

        let Some(worktree_path) = self.selected_worktree_path().map(Path::to_path_buf) else {
            self.notice = Some("select a worktree before generating a commit message".to_owned());
            cx.notify();
            return;
        };
        let changed_files = self.changed_files.clone();
        let preset = self.selected_agent_preset_or_default();
        let command = self.preset_command_for_kind(preset);
        let execution_mode = self.execution_mode;

        if let Some(modal) = self.commit_modal.as_mut() {
            modal.generating = true;
            modal.error = None;
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                generate_commit_message_with_ai(
                    worktree_path.as_path(),
                    &changed_files,
                    preset,
                    &command,
                    execution_mode,
                )
            });
            let result = result.await;

            let _ = this.update(cx, |this, cx| {
                if let Some(modal) = this.commit_modal.as_mut() {
                    modal.generating = false;
                    match result {
                        Ok(message) => {
                            modal.message = message.clone();
                            modal.message_cursor = char_count(&message);
                            modal.error = None;
                        },
                        Err(error) => {
                            modal.error = Some(error.to_string());
                        },
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn run_push_action(&mut self, cx: &mut Context<Self>) {
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
                        this.notice = Some(error.to_string());
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn run_create_pr_action(&mut self, cx: &mut Context<Self>) {
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

        if self.selected_local_worktree_has_pull_request() {
            self.notice = Some("selected worktree already has a pull request".to_owned());
            cx.notify();
            return;
        }

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
                        this.notice = Some(error.to_string());
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_commit_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.commit_modal.clone() else {
            return div();
        };
        let theme = self.theme();
        let default_message = default_commit_message(&self.changed_files);
        let has_existing_pull_request = self.selected_local_worktree_has_pull_request();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_commit_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(640.))
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
                            .child("Commit Changes"),
                    )
                    .child(
                        div().text_xs().text_color(rgb(theme.text_muted)).child(
                            "Review or generate the commit message before creating the commit.",
                        ),
                    )
                    .child(
                        div()
                            .min_h(px(110.))
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.accent))
                            .bg(rgb(theme.panel_bg))
                            .px_3()
                            .py_2()
                            .font_family(FONT_MONO)
                            .text_sm()
                            .text_color(rgb(theme.text_primary))
                            .child(active_input_display(
                                theme,
                                &modal.message,
                                "commit message",
                                theme.text_primary,
                                modal.message_cursor,
                                240,
                            )),
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
                    .when(has_existing_pull_request, |this| {
                        this.child(
                            div()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(theme.border))
                                .bg(rgb(theme.panel_bg))
                                .px_2()
                                .py_1()
                                .text_xs()
                                .text_color(rgb(theme.text_muted))
                                .child("This worktree already has a pull request."),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .justify_between()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        action_button(
                                            theme,
                                            "commit-generate",
                                            if modal.generating {
                                                "Generating..."
                                            } else {
                                                "Generate"
                                            },
                                            ActionButtonStyle::Secondary,
                                            !modal.generating,
                                        )
                                        .when(
                                            !modal.generating,
                                            |this| {
                                                this.on_click(cx.listener(|this, _, _, cx| {
                                                    this.generate_commit_message_with_ai(cx);
                                                }))
                                            },
                                        ),
                                    )
                                    .child(
                                        action_button(
                                            theme,
                                            "commit-default",
                                            "Use Default",
                                            ActionButtonStyle::Secondary,
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                if let Some(modal) = this.commit_modal.as_mut() {
                                                    modal.message = default_message.clone();
                                                    modal.message_cursor =
                                                        char_count(&default_message);
                                                    modal.error = None;
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        action_button(
                                            theme,
                                            "commit-cancel",
                                            "Cancel",
                                            ActionButtonStyle::Secondary,
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.close_commit_modal(cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        action_button(
                                            theme,
                                            "commit-submit",
                                            "Commit",
                                            ActionButtonStyle::Primary,
                                            !modal.generating,
                                        )
                                        .when(
                                            !modal.generating,
                                            |this| {
                                                this.on_click(cx.listener(|this, _, _, cx| {
                                                    this.submit_commit_modal(cx);
                                                }))
                                            },
                                        ),
                                    )
                                    .child(
                                        action_button(
                                            theme,
                                            "commit-submit-pr",
                                            "Commit + Push + PR",
                                            ActionButtonStyle::Primary,
                                            !modal.generating && !has_existing_pull_request,
                                        )
                                        .when(
                                            !modal.generating && !has_existing_pull_request,
                                            |this| {
                                                this.on_click(cx.listener(|this, _, _, cx| {
                                                    this.submit_commit_modal_and_create_pr(cx);
                                                }))
                                            },
                                        ),
                                    ),
                            ),
                    ),
            )
    }
}

pub(crate) enum StackedGitActionFailure {
    Commit(String),
    Push {
        commit_message: String,
        error: String,
    },
    CreatePullRequest {
        commit_message: String,
        push_message: String,
        error: String,
    },
}

pub(crate) fn generate_commit_message_with_ai(
    worktree_path: &Path,
    changed_files: &[ChangedFile],
    preset: AgentPresetKind,
    command: &str,
    execution_mode: ExecutionMode,
) -> Result<String, PromptError> {
    let prompt = build_commit_message_prompt(changed_files);
    run_prompt_capture(
        worktree_path,
        preset,
        command,
        &prompt,
        execution_mode,
        "commit message generation",
    )
}

pub(crate) fn build_commit_message_prompt(changed_files: &[ChangedFile]) -> String {
    let mut prompt = String::from(
        "Write a concise git commit message for these changes. Return only the commit message text.\n",
    );
    prompt
        .push_str("Use a short subject line, then an optional blank line and body if useful.\n\n");
    prompt.push_str("Changed files:\n");

    for change in changed_files.iter().take(20) {
        let mut line = format!("- {} {}", change_code(change.kind), change.path.display());
        if change.additions > 0 || change.deletions > 0 {
            line.push_str(&format!(" (+{} -{})", change.additions, change.deletions));
        }
        prompt.push_str(&line);
        prompt.push('\n');
    }

    if changed_files.len() > 20 {
        prompt.push_str(&format!(
            "- ... and {} more files\n",
            changed_files.len() - 20
        ));
    }

    prompt
}

pub(crate) fn run_git_commit_for_worktree(
    worktree_path: &Path,
    changed_files: &[ChangedFile],
    message: &str,
) -> Result<String, GitError> {
    if changed_files.is_empty() {
        return Err(GitError::Operation("nothing to commit".to_owned()));
    }

    let repo = git2::Repository::open(worktree_path).map_err(|error| {
        GitError::Operation(format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        ))
    })?;

    let mut index = repo
        .index()
        .map_err(|error| GitError::Operation(format!("failed to read index: {error}")))?;
    index
        .add_all(["."], git2::IndexAddOption::DEFAULT, None)
        .map_err(|error| GitError::Operation(format!("failed to stage changes: {error}")))?;
    index
        .update_all(["."], None)
        .map_err(|error| GitError::Operation(format!("failed to update index: {error}")))?;
    index
        .write()
        .map_err(|error| GitError::Operation(format!("failed to write index: {error}")))?;

    let tree_oid = index
        .write_tree()
        .map_err(|error| GitError::Operation(format!("failed to write tree: {error}")))?;
    let tree = repo
        .find_tree(tree_oid)
        .map_err(|error| GitError::Operation(format!("failed to find tree: {error}")))?;

    if let Ok(head_commit) = repo.head().and_then(|h| h.peel_to_commit())
        && head_commit.tree_id() == tree_oid
    {
        return Err(GitError::Operation("nothing to commit".to_owned()));
    }

    let message = message.trim();
    if message.is_empty() {
        return Err(GitError::Operation(
            "commit message cannot be empty".to_owned(),
        ));
    }
    let subject = message.lines().next().unwrap_or("commit");

    let sig = repo
        .signature()
        .map_err(|error| GitError::Operation(format!("failed to create signature: {error}")))?;

    let parent_commits: Vec<git2::Commit<'_>> = match repo.head().and_then(|h| h.peel_to_commit()) {
        Ok(commit) => vec![commit],
        Err(_) => vec![],
    };
    let parents: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();

    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|error| GitError::Operation(format!("failed to create commit: {error}")))?;

    Ok(format!("commit complete: {subject}"))
}

pub(crate) fn run_git_push_for_worktree(worktree_path: &Path) -> Result<String, GitError> {
    let repo = git2::Repository::open(worktree_path).map_err(|error| {
        GitError::Operation(format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        ))
    })?;

    let head_ref = repo
        .head()
        .map_err(|error| GitError::Operation(format!("failed to read HEAD: {error}")))?;
    let branch_name = head_ref
        .shorthand()
        .ok_or_else(|| GitError::Operation("cannot push detached HEAD".to_owned()))?
        .to_owned();
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");

    let mut remote = repo
        .find_remote("origin")
        .map_err(|error| GitError::Operation(format!("failed to find remote 'origin': {error}")))?;

    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(|_url, username_from_url, allowed_types| {
        if allowed_types.contains(git2::CredentialType::SSH_KEY) {
            let username = username_from_url.unwrap_or("git");
            git2::Cred::ssh_key_from_agent(username)
        } else if allowed_types.contains(git2::CredentialType::DEFAULT) {
            git2::Cred::default()
        } else {
            Err(git2::Error::from_str(
                "no suitable credential type available",
            ))
        }
    });

    let mut push_options = git2::PushOptions::new();
    push_options.remote_callbacks(callbacks);

    remote
        .push(&[&refspec], Some(&mut push_options))
        .map_err(|error| GitError::Operation(format!("push failed: {error}")))?;

    let mut config = repo
        .config()
        .map_err(|error| GitError::Operation(format!("failed to read config: {error}")))?;
    let _ = config.set_str(&format!("branch.{branch_name}.remote"), "origin");
    let _ = config.set_str(
        &format!("branch.{branch_name}.merge"),
        &format!("refs/heads/{branch_name}"),
    );

    Ok(format!(
        "push complete: {branch_name} -> origin/{branch_name}"
    ))
}

pub(crate) fn git_branch_name_for_worktree(worktree_path: &Path) -> Result<String, GitError> {
    let repo = gix::open(worktree_path).map_err(|error| {
        GitError::Operation(format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        ))
    })?;

    let head_ref = repo
        .head_ref()
        .map_err(|error| GitError::Operation(format!("failed to read HEAD: {error}")))?;

    match head_ref {
        Some(reference) => {
            let name = reference.name().shorten().to_string();
            if name.is_empty() {
                return Err(GitError::Operation(
                    "cannot create a PR from detached HEAD".to_owned(),
                ));
            }
            Ok(name)
        },
        None => Err(GitError::Operation(
            "cannot create a PR from detached HEAD".to_owned(),
        )),
    }
}

pub(crate) fn git_has_tracking_branch(worktree_path: &Path) -> bool {
    let Ok(repo) = gix::open(worktree_path) else {
        return false;
    };
    let Ok(Some(head_ref)) = repo.head_ref() else {
        return false;
    };

    let branch_name = head_ref.name().shorten().to_string();
    let config = repo.config_snapshot();
    config
        .string(format!("branch.{branch_name}.remote"))
        .is_some()
        && config
            .string(format!("branch.{branch_name}.merge"))
            .is_some()
}

pub(crate) fn git_default_base_branch(worktree_path: &Path) -> Option<String> {
    let repo = gix::open(worktree_path).ok()?;
    let reference = repo.find_reference("refs/remotes/origin/HEAD").ok()?;
    let target = reference.target();
    let target_name = target.try_name()?.to_string();
    let short = target_name
        .strip_prefix("refs/remotes/origin/")
        .unwrap_or(&target_name);

    if short.is_empty() {
        return None;
    }

    Some(short.to_owned())
}

pub(crate) fn run_create_pr_for_worktree(
    github_service: &dyn github_service::GitHubService,
    worktree_path: &Path,
    repo_slug: Option<&str>,
    github_token: Option<&str>,
) -> Result<String, GitHubError> {
    if !git_has_tracking_branch(worktree_path) {
        return Err(GitHubError::Api(
            "push the branch before creating a PR".to_owned(),
        ));
    }

    let branch = git_branch_name_for_worktree(worktree_path)
        .map_err(|error| GitHubError::Api(error.to_string()))?;
    let base_branch = git_default_base_branch(worktree_path).unwrap_or_else(|| "main".to_owned());

    let slug = repo_slug
        .map(str::to_owned)
        .or_else(|| github_repo_slug_for_repo(worktree_path))
        .ok_or_else(|| GitHubError::Api("could not determine GitHub repository slug".to_owned()))?;

    let title = branch.replace(['-', '_'], " ");

    let token = resolve_github_access_token(github_token).ok_or_else(|| {
        GitHubError::Auth("GitHub authentication required, click GitHub Sign in first".to_owned())
    })?;

    if let Some(existing_pr_number) =
        github_service.open_pull_request_number(&slug, &branch, &token)
    {
        return Err(GitHubError::Api(format!(
            "pull request already exists: {}",
            github_pr_url(&slug, existing_pr_number)
        )));
    }

    github_service
        .create_pull_request(&slug, &title, &branch, &base_branch, &token)
        .map_err(|error| GitHubError::Api(error.to_string()))
}

pub(crate) fn extract_first_url(text: &str) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_first_url_ignores_punctuation() {
        let url = extract_first_url("created PR: https://github.com/acme/repo/pull/42.");
        assert_eq!(url.as_deref(), Some("https://github.com/acme/repo/pull/42"));
    }
}
