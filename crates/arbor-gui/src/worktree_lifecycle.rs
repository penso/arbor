use super::*;

impl ArborWindow {
    pub(crate) fn next_create_modal_instance_id(&mut self) -> u64 {
        let instance_id = self.next_create_modal_instance_id;
        self.next_create_modal_instance_id = self.next_create_modal_instance_id.wrapping_add(1);
        instance_id
    }

    pub(crate) fn queue_local_worktree_selection_after_refresh(
        &mut self,
        worktree_path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.pending_local_worktree_selection = Some(worktree_path);
        self.refresh_worktrees(cx);
        self.terminal_scroll_handle.scroll_to_bottom();
        self.focus_terminal_on_next_render = true;
    }

    pub(crate) fn refresh_create_modal_branch_previews(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };

        modal.branch_preview_generation = modal.branch_preview_generation.wrapping_add(1);
        let modal_instance_id = modal.instance_id;
        let generation = modal.branch_preview_generation;
        let repository_path = modal.repository_path.trim().to_owned();
        let worktree_name = modal.worktree_name.clone();
        let pr_reference = modal.pr_reference.clone();
        let review_name_preview =
            review_worktree_name_preview(pr_reference.trim(), modal.worktree_name.trim());
        let outpost_name = modal.outpost_name.clone();
        let repo_root = self.repo_root.clone();
        let github_login = self.branch_prefix_github_login();

        self._create_modal_preview_task = Some(cx.spawn(async move |this, cx| {
            let previews = cx
                .background_spawn(async move {
                    resolve_create_modal_branch_previews(
                        &repository_path,
                        &worktree_name,
                        review_name_preview.as_deref(),
                        &outpost_name,
                        &repo_root,
                        github_login.as_deref(),
                    )
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let Some(modal) = this.create_modal.as_mut() else {
                    return;
                };
                if !create_modal_branch_preview_matches(modal, modal_instance_id, generation) {
                    return;
                }

                modal.local_branch_preview = previews.local_branch_preview;
                modal.review_branch_preview = previews.review_branch_preview;
                modal.outpost_branch_preview = previews.outpost_branch_preview;
                cx.notify();
            });
        }));
    }

    pub(crate) fn repository_for_issue_target(
        &self,
        target: &IssueTarget,
    ) -> Option<&RepositorySummary> {
        match target.daemon_target {
            ManagedDaemonTarget::Primary => self
                .repositories
                .iter()
                .find(|repository| repository.root == Path::new(&target.repo_root)),
            ManagedDaemonTarget::Remote(_) => None,
        }
    }

    pub(crate) fn open_create_modal(
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
        self.issue_details_modal = None;
        self.create_modal = Some(CreateModal {
            instance_id: self.next_create_modal_instance_id(),
            tab,
            repository_path_cursor: char_count(&repository_path),
            repository_path,
            worktree_name: String::new(),
            worktree_name_cursor: 0,
            checkout_kind: self.preferred_checkout_kind,
            worktree_active_field: CreateWorktreeField::WorktreeName,
            pr_reference: String::new(),
            pr_reference_cursor: 0,
            review_active_field: CreateReviewPrField::PullRequestReference,
            host_index: 0,
            host_dropdown_open: false,
            clone_url_cursor: char_count(&clone_url),
            clone_url,
            outpost_name: String::new(),
            outpost_name_cursor: 0,
            outpost_active_field: CreateOutpostField::CloneUrl,
            daemon_managed_target: None,
            managed_preview: None,
            managed_preview_loading: false,
            managed_preview_error: None,
            managed_preview_generation: 0,
            branch_preview_generation: 0,
            local_branch_preview: derive_branch_name(""),
            review_branch_preview: "Will derive from pull request".to_owned(),
            outpost_branch_preview: derive_branch_name(""),
            issue_context: None,
            is_creating: false,
            creating_status: None,
            error: None,
        });
        self.refresh_create_modal_branch_previews(cx);
        cx.notify();
    }

    pub(crate) fn open_issue_create_modal(
        &mut self,
        issue: terminal_daemon_http::IssueDto,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.issue_target_for_current_selection() else {
            self.notice =
                Some("No daemon-backed repository is selected for this issue.".to_owned());
            cx.notify();
            return;
        };

        let source_label = self
            .issue_list_state(&target)
            .and_then(|state| state.source.as_ref())
            .map(issue_modal_source_label)
            .unwrap_or_else(|| "Issue".to_owned());

        self.open_issue_create_modal_for_target(target, source_label, issue, cx);
    }

    pub(crate) fn open_issue_create_modal_for_target(
        &mut self,
        target: IssueTarget,
        source_label: String,
        issue: terminal_daemon_http::IssueDto,
        cx: &mut Context<Self>,
    ) {
        let repository_path = target.repo_root.clone();
        let worktree_name = issue.suggested_worktree_name.clone();
        let clone_url = self
            .repository_for_issue_target(&target)
            .and_then(|repository| repository.github_repo_slug.as_ref())
            .map(|slug| format!("git@github.com:{slug}.git"))
            .unwrap_or_default();
        self.issue_details_modal = None;

        self.create_modal = Some(CreateModal {
            instance_id: self.next_create_modal_instance_id(),
            tab: CreateModalTab::LocalWorktree,
            repository_path_cursor: char_count(&repository_path),
            repository_path,
            worktree_name_cursor: char_count(&worktree_name),
            worktree_name,
            checkout_kind: CheckoutKind::LinkedWorktree,
            worktree_active_field: CreateWorktreeField::WorktreeName,
            pr_reference: String::new(),
            pr_reference_cursor: 0,
            review_active_field: CreateReviewPrField::PullRequestReference,
            host_index: 0,
            host_dropdown_open: false,
            clone_url_cursor: char_count(&clone_url),
            clone_url,
            outpost_name: String::new(),
            outpost_name_cursor: 0,
            outpost_active_field: CreateOutpostField::CloneUrl,
            daemon_managed_target: Some(target.daemon_target),
            managed_preview: None,
            managed_preview_loading: false,
            managed_preview_error: None,
            managed_preview_generation: 0,
            branch_preview_generation: 0,
            local_branch_preview: derive_branch_name(issue.suggested_worktree_name.as_str()),
            review_branch_preview: "Will derive from pull request".to_owned(),
            outpost_branch_preview: derive_branch_name(""),
            issue_context: Some(CreateModalIssueContext {
                source_label,
                display_id: issue.display_id,
                title: issue.title,
                url: issue.url,
            }),
            is_creating: false,
            creating_status: None,
            error: None,
        });
        self.refresh_create_modal_branch_previews(cx);
        self.refresh_create_modal_managed_preview(cx);
        cx.notify();
    }

    pub(crate) fn refresh_create_modal_managed_preview(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };
        let Some(target) = modal.daemon_managed_target else {
            return;
        };

        let repository_path = modal.repository_path.trim().to_owned();
        let worktree_name = modal.worktree_name.trim().to_owned();
        modal.managed_preview_generation = modal.managed_preview_generation.wrapping_add(1);
        let modal_instance_id = modal.instance_id;
        let generation = modal.managed_preview_generation;

        if repository_path.is_empty() || worktree_name.is_empty() {
            modal.managed_preview = None;
            modal.managed_preview_loading = false;
            modal.managed_preview_error = None;
            cx.notify();
            return;
        }

        let client = match target {
            ManagedDaemonTarget::Primary => self.terminal_daemon.clone(),
            ManagedDaemonTarget::Remote(index) => self
                .remote_daemon_states
                .get(&index)
                .map(|state| state.client.clone()),
        };
        let Some(client) = client else {
            modal.managed_preview = None;
            modal.managed_preview_loading = false;
            modal.managed_preview_error =
                Some("No daemon connection is available for this issue.".to_owned());
            cx.notify();
            return;
        };

        modal.managed_preview = None;
        modal.managed_preview_loading = true;
        modal.managed_preview_error = None;
        self.ensure_loading_animation(cx);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let preview = cx
                .background_spawn(async move {
                    client.preview_managed_worktree(&repository_path, &worktree_name)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let Some(modal) = this.create_modal.as_mut() else {
                    return;
                };
                if !managed_preview_request_matches(modal, modal_instance_id, generation) {
                    return;
                }

                modal.managed_preview_loading = false;
                match preview {
                    Ok(preview) => {
                        modal.managed_preview = Some(preview);
                        modal.managed_preview_error = None;
                    },
                    Err(error) => {
                        modal.managed_preview = None;
                        modal.managed_preview_error =
                            Some(format!("failed to resolve worktree preview: {error}"));
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn update_create_review_pr_modal_input(
        &mut self,
        input: ReviewPrModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let mut refresh_branch_previews = false;
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };

        if modal.is_creating {
            return;
        }

        match input {
            ReviewPrModalInputEvent::SetActiveField(field) => {
                modal.review_active_field = field;
                match field {
                    CreateReviewPrField::RepositoryPath => {
                        modal.repository_path_cursor = char_count(&modal.repository_path);
                    },
                    CreateReviewPrField::PullRequestReference => {
                        modal.pr_reference_cursor = char_count(&modal.pr_reference);
                    },
                    CreateReviewPrField::WorktreeName => {
                        modal.worktree_name_cursor = char_count(&modal.worktree_name);
                    },
                }
            },
            ReviewPrModalInputEvent::MoveActiveField => {
                modal.review_active_field = match modal.review_active_field {
                    CreateReviewPrField::RepositoryPath => {
                        CreateReviewPrField::PullRequestReference
                    },
                    CreateReviewPrField::PullRequestReference => CreateReviewPrField::WorktreeName,
                    CreateReviewPrField::WorktreeName => CreateReviewPrField::RepositoryPath,
                };
            },
            ReviewPrModalInputEvent::Edit(action) => match modal.review_active_field {
                CreateReviewPrField::RepositoryPath => {
                    apply_text_edit_action(
                        &mut modal.repository_path,
                        &mut modal.repository_path_cursor,
                        &action,
                    );
                    refresh_branch_previews = true;
                },
                CreateReviewPrField::PullRequestReference => {
                    apply_text_edit_action(
                        &mut modal.pr_reference,
                        &mut modal.pr_reference_cursor,
                        &action,
                    );
                    refresh_branch_previews = true;
                },
                CreateReviewPrField::WorktreeName => {
                    apply_text_edit_action(
                        &mut modal.worktree_name,
                        &mut modal.worktree_name_cursor,
                        &action,
                    );
                    refresh_branch_previews = true;
                },
            },
            ReviewPrModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        cx.notify();
        if refresh_branch_previews {
            self.refresh_create_modal_branch_previews(cx);
        }
    }

    pub(crate) fn set_create_modal_checkout_kind(
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

    pub(crate) fn close_create_modal(&mut self, cx: &mut Context<Self>) {
        self.create_modal = None;
        cx.notify();
    }

    pub(crate) fn open_delete_modal(
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

    pub(crate) fn close_delete_modal(&mut self, cx: &mut Context<Self>) {
        self.delete_modal = None;
        cx.notify();
    }

    pub(crate) fn execute_delete(&mut self, cx: &mut Context<Self>) {
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
                    let result = cx
                        .background_spawn({
                            let repo_root = repo_root.clone();
                            let wt_path = wt_path.clone();
                            let branch = branch.clone();
                            async move {
                                let script_context =
                                    WorktreeScriptContext::new(&repo_root, &wt_path, Some(&branch));
                                run_worktree_scripts(
                                    &repo_root,
                                    WorktreeScriptPhase::Teardown,
                                    &script_context,
                                )
                                .map_err(|error| error.to_string())?;

                                if is_discrete_clone {
                                    fs::remove_dir_all(&wt_path).map_err(|error| {
                                        format!(
                                            "failed to remove discrete clone `{}`: {error}",
                                            wt_path.display()
                                        )
                                    })
                                } else {
                                    worktree::remove(&repo_root, &wt_path, true)
                                        .map_err(|error| error.to_string())
                                }
                            }
                        })
                        .await;

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

                if let Err(error) = self.outpost_store.remove(&outpost_id) {
                    if let Some(modal) = self.delete_modal.as_mut() {
                        modal.error = Some(error.to_string());
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

    pub(crate) fn update_create_worktree_modal_input(
        &mut self,
        input: ModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let mut refresh_managed_preview = false;
        let mut refresh_branch_previews = false;
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
                    refresh_managed_preview = modal.daemon_managed_target.is_some();
                    refresh_branch_previews = true;
                },
                CreateWorktreeField::WorktreeName => {
                    apply_text_edit_action(
                        &mut modal.worktree_name,
                        &mut modal.worktree_name_cursor,
                        &action,
                    );
                    refresh_managed_preview = modal.daemon_managed_target.is_some();
                    refresh_branch_previews = true;
                },
            },
            ModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        cx.notify();
        if refresh_branch_previews {
            self.refresh_create_modal_branch_previews(cx);
        }
        if refresh_managed_preview {
            self.refresh_create_modal_managed_preview(cx);
        }
    }

    pub(crate) fn submit_create_worktree_modal(&mut self, cx: &mut Context<Self>) {
        let github_login = self.branch_prefix_github_login();
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
        let daemon_managed_target = modal.daemon_managed_target;
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
        modal.creating_status =
            daemon_managed_target.map(|_| "Creating managed worktree…".to_owned());
        cx.notify();

        if let Some(target) = daemon_managed_target {
            let client = match target {
                ManagedDaemonTarget::Primary => self.terminal_daemon.clone(),
                ManagedDaemonTarget::Remote(index) => self
                    .remote_daemon_states
                    .get(&index)
                    .map(|state| state.client.clone()),
            };
            let Some(client) = client else {
                if let Some(modal) = self.create_modal.as_mut() {
                    modal.is_creating = false;
                    modal.creating_status = None;
                    modal.error =
                        Some("No daemon connection is available for this issue.".to_owned());
                }
                cx.notify();
                return;
            };

            cx.spawn(async move |this, cx| {
                let creation = cx
                    .background_spawn(async move {
                        client.create_managed_worktree(&repository_input, &worktree_input)
                    })
                    .await;

                let _ = this.update(cx, |this, cx| {
                    match creation {
                        Ok(created) => {
                            this.create_modal = None;
                            this.notice = Some(created.message.clone());

                            match target {
                                ManagedDaemonTarget::Primary => {
                                    let worktree_path = PathBuf::from(&created.path);
                                    this.queue_local_worktree_selection_after_refresh(
                                        worktree_path,
                                        cx,
                                    );
                                },
                                ManagedDaemonTarget::Remote(index) => {
                                    if let Some(state) = this.remote_daemon_states.get_mut(&index) {
                                        let branch = created.branch.clone().unwrap_or_default();
                                        if !state
                                            .worktrees
                                            .iter()
                                            .any(|worktree| worktree.path == created.path)
                                        {
                                            state.worktrees.push(
                                                terminal_daemon_http::RemoteWorktreeDto {
                                                    repo_root: created.repo_root.clone(),
                                                    path: created.path.clone(),
                                                    branch,
                                                    is_primary_checkout: false,
                                                    last_activity_unix_ms: None,
                                                    diff_additions: None,
                                                    diff_deletions: None,
                                                    pr_number: None,
                                                    pr_url: None,
                                                },
                                            );
                                        }
                                    }

                                    this.activate_remote_worktree(index, created.path, cx);
                                },
                            }
                        },
                        Err(error) => {
                            tracing::error!("daemon worktree creation failed: {error}");
                            if let Some(modal) = this.create_modal.as_mut() {
                                modal.is_creating = false;
                                modal.creating_status = None;
                                modal.error = Some(format!("{error}"));
                            } else {
                                this.notice = Some(format!("{error}"));
                            }
                        },
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }

        cx.spawn(async move |this, cx| {
            let creation = cx
                .background_spawn(async move {
                    create_managed_worktree(
                        repository_input,
                        worktree_input,
                        checkout_kind,
                        github_login,
                    )
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
                        this.queue_local_worktree_selection_after_refresh(
                            created.worktree_path,
                            cx,
                        );
                    },
                    Err(error) => {
                        tracing::error!("worktree creation failed: {error}");
                        let message = error.to_string();
                        if let Some(modal) = this.create_modal.as_mut() {
                            modal.is_creating = false;
                            modal.creating_status = None;
                            modal.error = Some(message);
                        } else {
                            this.notice = Some(message);
                        }
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn submit_create_review_pr_modal(&mut self, cx: &mut Context<Self>) {
        let github_token = self.github_access_token();
        let github_login = self.branch_prefix_github_login();
        let Some(modal) = self.create_modal.as_mut() else {
            return;
        };
        if modal.is_creating {
            return;
        }

        modal.error = None;
        let repository_input = modal.repository_path.trim().to_owned();
        let pr_reference = modal.pr_reference.trim().to_owned();
        let worktree_input = modal.worktree_name.trim().to_owned();
        let checkout_kind = modal.checkout_kind;
        if repository_input.is_empty() {
            modal.error = Some("Repository path is required.".to_owned());
            cx.notify();
            return;
        }

        if pr_reference.is_empty() {
            modal.error = Some("Pull request reference is required.".to_owned());
            cx.notify();
            return;
        }

        modal.is_creating = true;
        modal.creating_status = Some("Resolving pull request…".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let creation = cx
                .background_spawn(async move {
                    create_review_worktree(
                        repository_input,
                        pr_reference,
                        worktree_input,
                        checkout_kind,
                        github_token,
                        github_login,
                    )
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

                        let notice = match created.review_pull_request_number {
                            Some(number) => format!(
                                "created PR review {} `{}` from pull request #{number} on branch `{}`",
                                created.checkout_kind.label().to_ascii_lowercase(),
                                created.worktree_name,
                                created.branch_name
                            ),
                            None => format!(
                                "created {} `{}` on branch `{}`",
                                created.checkout_kind.label().to_ascii_lowercase(),
                                created.worktree_name,
                                created.branch_name
                            ),
                        };
                        this.notice = Some(notice);
                        this.create_modal = None;
                        this.queue_local_worktree_selection_after_refresh(
                            created.worktree_path,
                            cx,
                        );
                    },
                    Err(error) => {
                        tracing::error!("pull request review creation failed: {error}");
                        let message = error.to_string();
                        if let Some(modal) = this.create_modal.as_mut() {
                            modal.is_creating = false;
                            modal.creating_status = None;
                            modal.error = Some(message);
                        } else {
                            this.notice = Some(message);
                        }
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn update_create_outpost_modal_input(
        &mut self,
        input: OutpostModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let mut refresh_branch_previews = false;
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
                        refresh_branch_previews = true;
                    },
                }
            },
            OutpostModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        cx.notify();
        if refresh_branch_previews {
            self.refresh_create_modal_branch_previews(cx);
        }
    }

    pub(crate) fn submit_create_outpost_modal(&mut self, cx: &mut Context<Self>) {
        let repo_root = self.repo_root.clone();
        let github_login = self.branch_prefix_github_login();
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

        let branch =
            derive_branch_name_with_repo_config(&repo_root, &outpost_name, github_login.as_deref());

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

        enum ProvisionMsg {
            Progress(String),
            Done(Result<arbor_core::remote::ProvisionResult, OutpostError>),
        }

        let (msg_tx, msg_rx) = smol::channel::unbounded::<ProvisionMsg>();

        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                let result = (|| {
                    let conn_slot = pool
                        .get_or_connect(&host)
                        .map_err(|error| OutpostError::Connection(format!("{error}")))?;
                    let guard = conn_slot.lock().map_err(|_| {
                        OutpostError::Connection("SSH connection lock poisoned".to_owned())
                    })?;
                    let connection = guard.as_ref().ok_or_else(|| {
                        OutpostError::Connection("SSH connection not available".to_owned())
                    })?;
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
                        .map_err(|error| OutpostError::Provisioning(format!("{error}")))
                })();
                let _ = msg_tx.send_blocking(ProvisionMsg::Done(result));
            })
            .detach();

            let mut result: Result<arbor_core::remote::ProvisionResult, OutpostError> = Err(
                OutpostError::Provisioning("provisioning task was cancelled".to_owned()),
            );
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
                    ProvisionMsg::Done(done) => {
                        result = done;
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
                        if let Err(error) = this.outpost_store.upsert(record) {
                            this.notice =
                                Some(format!("outpost created but failed to save: {error}"));
                        } else {
                            this.notice =
                                Some(format!("outpost `{outpost_name}` created on {host_name}"));
                        }
                        this.outposts =
                            load_outpost_summaries(this.outpost_store.as_ref(), &this.remote_hosts);
                        let new_index = this.outposts.iter().position(|outpost| {
                            outpost.label == outpost_name && outpost.host_name == host_name
                        });
                        if let Some(index) = new_index {
                            this.active_outpost_index = Some(index);
                        }
                        this.create_modal = None;
                    },
                    Err(error) => {
                        tracing::error!("outpost creation failed: {error}");
                        let message = error.to_string();
                        if let Some(modal) = this.create_modal.as_mut() {
                            modal.is_creating = false;
                            modal.creating_status = None;
                            modal.error = Some(message);
                        } else {
                            this.notice = Some(message);
                        }
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_outpost_context_menu(&mut self, cx: &mut Context<Self>) -> Div {
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
                                let outpost_label = this
                                    .outposts
                                    .get(index)
                                    .map(|outpost| outpost.label.clone())
                                    .unwrap_or_default();
                                let outpost_branch = this
                                    .outposts
                                    .get(index)
                                    .map(|outpost| outpost.branch.clone())
                                    .unwrap_or_default();
                                this.open_delete_modal(
                                    DeleteTarget::Outpost(index),
                                    outpost_label,
                                    outpost_branch,
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

    pub(crate) fn render_create_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.create_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let has_remote_hosts = !self.remote_hosts.is_empty();
        let is_worktree_tab = modal.tab == CreateModalTab::LocalWorktree;
        let is_review_pr_tab = modal.tab == CreateModalTab::ReviewPullRequest;
        let is_outpost_tab = modal.tab == CreateModalTab::RemoteOutpost;
        let daemon_managed_worktree = modal.daemon_managed_target.is_some();
        let issue_context = modal.issue_context.clone();

        // Worktree tab data
        let branch_name = if let Some(preview) = modal.managed_preview.as_ref() {
            preview.branch.clone()
        } else if daemon_managed_worktree && modal.managed_preview_loading {
            "Resolving preview…".to_owned()
        } else {
            modal.local_branch_preview.clone()
        };
        let target_path_preview = if let Some(preview) = modal.managed_preview.as_ref() {
            preview.path.clone()
        } else if daemon_managed_worktree && modal.managed_preview_loading {
            "Resolving preview…".to_owned()
        } else {
            preview_managed_worktree_path(modal.repository_path.trim(), modal.worktree_name.trim())
                .unwrap_or_else(|_| "-".to_owned())
        };
        let checkout_kind = modal.checkout_kind;
        let is_discrete_clone = checkout_kind == CheckoutKind::DiscreteClone;
        let repository_active = modal.worktree_active_field == CreateWorktreeField::RepositoryPath;
        let worktree_active = modal.worktree_active_field == CreateWorktreeField::WorktreeName;
        let worktree_create_disabled = modal.is_creating
            || modal.repository_path.trim().is_empty()
            || modal.worktree_name.trim().is_empty()
            || (daemon_managed_worktree
                && (modal.managed_preview_loading || modal.managed_preview.is_none()));

        // Review PR tab data
        let review_repository_active =
            modal.review_active_field == CreateReviewPrField::RepositoryPath;
        let review_pr_active =
            modal.review_active_field == CreateReviewPrField::PullRequestReference;
        let review_name_active = modal.review_active_field == CreateReviewPrField::WorktreeName;
        let review_name_preview =
            review_worktree_name_preview(modal.pr_reference.trim(), modal.worktree_name.trim());
        let review_branch_preview = modal.review_branch_preview.clone();
        let review_path_preview = review_name_preview
            .as_deref()
            .and_then(|name| preview_managed_worktree_path(modal.repository_path.trim(), name).ok())
            .unwrap_or_else(|| "Will resolve after pull request lookup".to_owned());
        let review_create_disabled = modal.is_creating
            || modal.repository_path.trim().is_empty()
            || modal.pr_reference.trim().is_empty();

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
        let outpost_branch_preview = modal.outpost_branch_preview.clone();
        let outpost_create_disabled = modal.is_creating
            || modal.clone_url.trim().is_empty()
            || modal.outpost_name.trim().is_empty()
            || self.remote_hosts.is_empty();

        let create_disabled = if is_worktree_tab {
            worktree_create_disabled
        } else if is_review_pr_tab {
            review_create_disabled
        } else {
            outpost_create_disabled
        };
        let modal_body_max_height = px(460.);
        let creating_status = modal.creating_status.clone();
        let submit_label: String = if modal.is_creating {
            creating_status.as_deref().unwrap_or("Creating…").to_owned()
        } else if daemon_managed_worktree && is_worktree_tab {
            "Create Worktree".to_owned()
        } else if is_worktree_tab {
            checkout_kind.action_label().to_owned()
        } else if is_review_pr_tab {
            "Review Pull Request".to_owned()
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
                    .h_auto()
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
                            .child(
                                div()
                                    .id("tab-review-pr")
                                    .cursor_pointer()
                                    .px_3()
                                    .py_1()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(if is_review_pr_tab {
                                        theme.text_primary
                                    } else {
                                        theme.text_muted
                                    }))
                                    .when(is_review_pr_tab, |this| {
                                        this.border_b_2().border_color(rgb(theme.accent))
                                    })
                                    .when(!is_review_pr_tab, |this| {
                                        this.hover(|this| this.text_color(rgb(theme.text_primary)))
                                    })
                                    .child("Review PR")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(modal) = this.create_modal.as_mut()
                                            && !modal.is_creating
                                        {
                                            modal.tab = CreateModalTab::ReviewPullRequest;
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
                    .child(
                        div()
                            .id("create-modal-body")
                            .flex_none()
                            .min_h_0()
                            .max_h(modal_body_max_height)
                            .overflow_y_scroll()
                            .pr_1()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    // Local Worktree tab content
                                    .when(is_worktree_tab, |this| {
                                        this.child(
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(rgb(theme.text_muted))
                                                .child(if daemon_managed_worktree {
                                                    "Managed worktrees are created by the daemon under ~/.arbor/worktrees/<repo>/<worktree>/."
                                                } else {
                                                    "Target base: ~/.arbor/worktrees/<repo>/<worktree>/"
                                                }),
                                        )
                                        .when_some(issue_context.clone(), |this, issue| {
                                            this.child(
                                                div()
                                                    .w_full()
                                                    .min_w_0()
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(rgb(theme.border))
                                                    .bg(rgb(theme.panel_bg))
                                                    .p_2()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(4.))
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .justify_between()
                                                            .gap_2()
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .font_family(FONT_MONO)
                                                                    .text_color(rgb(theme.accent))
                                                                    .child(issue.display_id),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(rgb(theme.text_muted))
                                                                    .child(issue.source_label),
                                                            ),
                                                    )
                                                    .child(
                                                        modal_preview_line(
                                                            modal_text_preview(&issue.title),
                                                            theme.text_primary,
                                                            false,
                                                        ),
                                                    )
                                                    .when_some(issue.url, |this, url| {
                                                        this.child(
                                                            modal_preview_line(
                                                                modal_mono_preview(&url),
                                                                theme.text_muted,
                                                                true,
                                                            )
                                                            .text_xs(),
                                                        )
                                                    }),
                                            )
                                        })
                                        .when(!daemon_managed_worktree, |this| {
                                            this.child(
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
                                        })
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
                                            modal_preview_box(
                                                theme,
                                                "Branch",
                                                modal_mono_preview(&branch_name),
                                            ),
                                        )
                                        .child(
                                            modal_preview_box(
                                                theme,
                                                "Path",
                                                modal_mono_preview(&target_path_preview),
                                            ),
                                        )
                                        .when_some(modal.managed_preview_error.clone(), |this, error| {
                                            this.child(
                                                div()
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
                                    })
                                    // Review PR tab content
                                    .when(is_review_pr_tab, |this| {
                                        this.child(
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(rgb(theme.text_muted))
                                                .child("Paste a GitHub PR number, `#123`, or full pull-request URL."),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .id("create-review-pr-discrete-clone-checkbox")
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
                                                "review-pr-repo-input",
                                                "Repository",
                                                &modal.repository_path,
                                                modal.repository_path_cursor,
                                                "Path to git repository",
                                                review_repository_active,
                                            )
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.update_create_review_pr_modal_input(
                                                    ReviewPrModalInputEvent::SetActiveField(
                                                        CreateReviewPrField::RepositoryPath,
                                                    ),
                                                    cx,
                                                );
                                            })),
                                        )
                                        .child(
                                            modal_input_field(
                                                theme,
                                                "review-pr-reference-input",
                                                "Pull Request",
                                                &modal.pr_reference,
                                                modal.pr_reference_cursor,
                                                "e.g. 42, #42, or https://github.com/org/repo/pull/42",
                                                review_pr_active,
                                            )
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.update_create_review_pr_modal_input(
                                                    ReviewPrModalInputEvent::SetActiveField(
                                                        CreateReviewPrField::PullRequestReference,
                                                    ),
                                                    cx,
                                                );
                                            })),
                                        )
                                        .child(
                                            modal_input_field(
                                                theme,
                                                "review-pr-name-input",
                                                "Worktree Name",
                                                &modal.worktree_name,
                                                modal.worktree_name_cursor,
                                                "Optional. Defaults from the pull request title.",
                                                review_name_active,
                                            )
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.update_create_review_pr_modal_input(
                                                    ReviewPrModalInputEvent::SetActiveField(
                                                        CreateReviewPrField::WorktreeName,
                                                    ),
                                                    cx,
                                                );
                                            })),
                                        )
                                        .child(
                                            modal_preview_box(
                                                theme,
                                                "Branch",
                                                modal_mono_preview(&review_branch_preview),
                                            ),
                                        )
                                        .child(
                                            modal_preview_box(
                                                theme,
                                                "Path",
                                                modal_mono_preview(&review_path_preview),
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
                                            modal_preview_box(
                                                theme,
                                                "Branch",
                                                modal_mono_preview(&outpost_branch_preview),
                                            ),
                                        )
                                        .child(
                                            modal_preview_box(
                                                theme,
                                                "Remote Path",
                                                modal_mono_preview(&remote_preview),
                                            ),
                                        )
                                    }),
                            ),
                    )
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
                                                CreateModalTab::ReviewPullRequest => {
                                                    this.submit_create_review_pr_modal(cx);
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

    pub(crate) fn render_delete_modal(&mut self, cx: &mut Context<Self>) -> Div {
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
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_delete_modal(cx);
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(format!("{}: {}", label_prefix, modal.label)),
                    )
                    .when(is_worktree, |this| match modal.has_unpushed {
                        None => this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_muted))
                                .child("Checking for unpushed commits..."),
                        ),
                        Some(true) => this.child(div().text_xs().text_color(rgb(0xe5c07b)).child(
                            "\u{f071} This worktree has unpushed commits that will be lost.",
                        )),
                        Some(false) => this,
                    })
                    .when(
                        is_worktree && !is_discrete_clone && !modal.branch.is_empty(),
                        |this| {
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
                                                this.bg(rgb(theme.accent)).child(
                                                    div()
                                                        .font_family(FONT_MONO)
                                                        .text_size(px(10.))
                                                        .text_color(rgb(theme.sidebar_bg))
                                                        .child("\u{f00c}"),
                                                )
                                            }),
                                    )
                                    .child(
                                        div().text_xs().text_color(rgb(theme.text_primary)).child(
                                            format!("Also delete branch `{}`", modal.branch),
                                        ),
                                    ),
                            )
                        },
                    )
                    .when_some(modal.error.clone(), |this, error| {
                        this.child(div().text_xs().text_color(rgb(0xeb6f92)).child(error))
                    })
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
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_delete_modal(cx);
                                    },
                                )),
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
                                        this.hover(|surface| {
                                            surface.bg(rgb(0xeb6f92)).text_color(rgb(theme.app_bg))
                                        })
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
}

pub(crate) const CREATE_MODAL_MONO_PREVIEW_MAX_CHARS: usize = 56;
pub(crate) const CREATE_MODAL_TEXT_PREVIEW_MAX_CHARS: usize = 72;

pub(crate) fn modal_mono_preview(value: &str) -> String {
    truncate_middle_text(value, CREATE_MODAL_MONO_PREVIEW_MAX_CHARS)
}

pub(crate) fn modal_text_preview(value: &str) -> String {
    truncate_with_ellipsis(value, CREATE_MODAL_TEXT_PREVIEW_MAX_CHARS)
}

pub(crate) fn modal_preview_line(value: String, color: u32, mono: bool) -> Div {
    div()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_sm()
        .text_color(rgb(color))
        .when(mono, |this| this.font_family(FONT_MONO))
        .child(value)
}

pub(crate) fn modal_preview_box(theme: ThemePalette, label: &'static str, value: String) -> Div {
    div()
        .flex_none()
        .w_full()
        .min_w_0()
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.panel_bg))
        .p_2()
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(modal_preview_line(value, theme.text_primary, true))
}

pub(crate) fn preview_managed_worktree_path(
    repository_path: &str,
    worktree_name: &str,
) -> Result<String, WorktreeError> {
    let repository_path = expand_home_path(repository_path)?;
    let repository_name = repository_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            WorktreeError::InvalidInput("repository name cannot be determined".to_owned())
        })?;
    let sanitized_worktree = sanitize_worktree_name(worktree_name);
    if sanitized_worktree.is_empty() {
        return Err(WorktreeError::InvalidInput(
            "invalid worktree name".to_owned(),
        ));
    }

    let path = build_managed_worktree_path(repository_name, &sanitized_worktree)?;
    Ok(path.display().to_string())
}

pub(crate) fn review_worktree_name_preview(
    pr_reference: &str,
    explicit_worktree_name: &str,
) -> Option<String> {
    let explicit = sanitize_worktree_name(explicit_worktree_name);
    if !explicit.is_empty() {
        return Some(explicit);
    }

    github_service::parse_pull_request_number(pr_reference).map(|number| format!("pr-{number}"))
}

pub(crate) struct CreateModalBranchPreviews {
    pub(crate) local_branch_preview: String,
    pub(crate) review_branch_preview: String,
    pub(crate) outpost_branch_preview: String,
}

pub(crate) fn resolve_create_modal_branch_previews(
    repository_path: &str,
    worktree_name: &str,
    review_worktree_name: Option<&str>,
    outpost_name: &str,
    outpost_repo_root: &Path,
    github_login: Option<&str>,
) -> CreateModalBranchPreviews {
    let repository_root = Path::new(repository_path.trim());
    let review_branch_preview = review_worktree_name
        .map(|name| derive_branch_name_for_repo_with_login(repository_root, name, github_login))
        .unwrap_or_else(|| "Will derive from pull request".to_owned());

    CreateModalBranchPreviews {
        local_branch_preview: derive_branch_name_for_repo_with_login(
            repository_root,
            worktree_name,
            github_login,
        ),
        review_branch_preview,
        outpost_branch_preview: derive_branch_name_for_repo_with_login(
            outpost_repo_root,
            outpost_name,
            github_login,
        ),
    }
}

pub(crate) fn create_managed_worktree(
    repository_path_input: String,
    worktree_name_input: String,
    checkout_kind: CheckoutKind,
    github_login: Option<String>,
) -> Result<CreatedWorktree, WorktreeError> {
    let repository_path = expand_home_path(&repository_path_input)?;
    if !repository_path.exists() {
        return Err(WorktreeError::InvalidInput(format!(
            "repository path does not exist: {}",
            repository_path.display()
        )));
    }

    let repository_root = worktree::repo_root(&repository_path).map_err(|error| {
        WorktreeError::GitOperation(format!("failed to resolve repository root: {error}"))
    })?;
    let repository_name = repository_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            WorktreeError::InvalidInput("repository root has no terminal directory name".to_owned())
        })?;

    let sanitized_worktree_name = sanitize_worktree_name(&worktree_name_input);
    if sanitized_worktree_name.is_empty() {
        return Err(WorktreeError::InvalidInput(
            "worktree name contains no usable characters".to_owned(),
        ));
    }

    let branch_name = derive_branch_name_with_repo_config(
        &repository_root,
        &worktree_name_input,
        github_login.as_deref(),
    );
    let worktree_path = build_managed_worktree_path(repository_name, &sanitized_worktree_name)?;
    if worktree_path.exists() {
        return Err(WorktreeError::InvalidInput(format!(
            "worktree path already exists: {}",
            worktree_path.display()
        )));
    }

    let Some(parent_directory) = worktree_path.parent() else {
        return Err(WorktreeError::InvalidInput(
            "invalid worktree path".to_owned(),
        ));
    };
    fs::create_dir_all(parent_directory).map_err(|error| {
        WorktreeError::Io(format!(
            "failed to create worktree parent directory `{}`: {error}",
            parent_directory.display()
        ))
    })?;

    match checkout_kind {
        CheckoutKind::LinkedWorktree => worktree::add(
            &repository_root,
            &worktree_path,
            worktree::AddWorktreeOptions {
                branch: Some(&branch_name),
                detach: false,
                force: false,
            },
        )
        .map_err(|error| {
            WorktreeError::GitOperation(format!("failed to create worktree: {error}"))
        })?,
        CheckoutKind::DiscreteClone => {
            create_discrete_clone(&repository_root, &worktree_path, &branch_name)?
        },
    }

    let script_context =
        WorktreeScriptContext::new(&repository_root, &worktree_path, Some(&branch_name));
    if let Err(error) = run_worktree_scripts(
        &repository_root,
        WorktreeScriptPhase::Setup,
        &script_context,
    ) {
        rollback_created_checkout(
            &repository_root,
            &worktree_path,
            checkout_kind,
            &branch_name,
        )
        .map_err(|rollback_error| WorktreeError::ScriptWithRollbackFailure {
            message: error.to_string(),
            rollback: rollback_error.to_string(),
        })?;
        return Err(WorktreeError::Script(error.to_string()));
    }

    Ok(CreatedWorktree {
        worktree_name: sanitized_worktree_name,
        branch_name,
        worktree_path,
        checkout_kind,
        source_repo_root: repository_root,
        review_pull_request_number: None,
    })
}

pub(crate) fn create_review_worktree(
    repository_path_input: String,
    pr_reference_input: String,
    worktree_name_input: String,
    checkout_kind: CheckoutKind,
    github_token: Option<String>,
    github_login: Option<String>,
) -> Result<CreatedWorktree, WorktreeError> {
    let repository_path = expand_home_path(&repository_path_input)?;
    if !repository_path.exists() {
        return Err(WorktreeError::InvalidInput(format!(
            "repository path does not exist: {}",
            repository_path.display()
        )));
    }

    let repository_root = worktree::repo_root(&repository_path).map_err(|error| {
        WorktreeError::GitOperation(format!("failed to resolve repository root: {error}"))
    })?;
    let repository_name = repository_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            WorktreeError::InvalidInput("repository root has no terminal directory name".to_owned())
        })?;
    let repo_slug = github_repo_slug_for_repo(&repository_root).ok_or_else(|| {
        WorktreeError::GitHub(format!(
            "repository `{}` does not have a GitHub origin remote",
            repository_root.display()
        ))
    })?;
    let pull_request = github_service::resolve_pull_request_for_review(
        &repo_slug,
        &pr_reference_input,
        github_token.as_deref(),
    )
    .map_err(|e| WorktreeError::GitHub(e.to_string()))?;

    let requested_name = if worktree_name_input.trim().is_empty() {
        default_review_worktree_name(&pull_request)
    } else {
        worktree_name_input
    };
    let sanitized_worktree_name = sanitize_worktree_name(&requested_name);
    if sanitized_worktree_name.is_empty() {
        return Err(WorktreeError::InvalidInput(
            "worktree name contains no usable characters".to_owned(),
        ));
    }

    let branch_name = derive_branch_name_with_repo_config(
        &repository_root,
        &requested_name,
        github_login.as_deref(),
    );
    let worktree_path = build_managed_worktree_path(repository_name, &sanitized_worktree_name)?;
    if worktree_path.exists() {
        return Err(WorktreeError::InvalidInput(format!(
            "worktree path already exists: {}",
            worktree_path.display()
        )));
    }

    let Some(parent_directory) = worktree_path.parent() else {
        return Err(WorktreeError::InvalidInput(
            "invalid worktree path".to_owned(),
        ));
    };
    fs::create_dir_all(parent_directory).map_err(|error| {
        WorktreeError::Io(format!(
            "failed to create worktree parent directory `{}`: {error}",
            parent_directory.display()
        ))
    })?;

    match checkout_kind {
        CheckoutKind::LinkedWorktree => {
            fetch_pull_request_head_into_branch(
                &repository_root,
                pull_request.number,
                &branch_name,
            )?;
            worktree::add(
                &repository_root,
                &worktree_path,
                worktree::AddWorktreeOptions {
                    branch: Some(&branch_name),
                    detach: false,
                    force: false,
                },
            )
            .map_err(|error| {
                WorktreeError::GitOperation(format!("failed to create worktree: {error}"))
            })?;
        },
        CheckoutKind::DiscreteClone => create_discrete_clone_from_pull_request(
            &repository_root,
            &worktree_path,
            pull_request.number,
            &branch_name,
        )?,
    }

    let script_context =
        WorktreeScriptContext::new(&repository_root, &worktree_path, Some(&branch_name));
    if let Err(error) = run_worktree_scripts(
        &repository_root,
        WorktreeScriptPhase::Setup,
        &script_context,
    ) {
        rollback_created_checkout(
            &repository_root,
            &worktree_path,
            checkout_kind,
            &branch_name,
        )
        .map_err(|rollback_error| WorktreeError::ScriptWithRollbackFailure {
            message: error.to_string(),
            rollback: rollback_error.to_string(),
        })?;
        return Err(WorktreeError::Script(error.to_string()));
    }

    Ok(CreatedWorktree {
        worktree_name: sanitized_worktree_name,
        branch_name,
        worktree_path,
        checkout_kind,
        source_repo_root: repository_root,
        review_pull_request_number: Some(pull_request.number),
    })
}

pub(crate) fn default_review_worktree_name(
    pull_request: &github_service::ReviewPullRequest,
) -> String {
    let title_slug = sanitize_worktree_name(&pull_request.title);
    if title_slug.is_empty() {
        format!("pr-{}", pull_request.number)
    } else {
        format!("pr-{}-{title_slug}", pull_request.number)
    }
}

pub(crate) fn fetch_pull_request_head_into_branch(
    repository_root: &Path,
    pull_request_number: u64,
    branch_name: &str,
) -> Result<(), WorktreeError> {
    let fetch_ref = format!("+refs/pull/{pull_request_number}/head:refs/heads/{branch_name}");
    let mut command = create_command("git");
    command
        .arg("-C")
        .arg(repository_root)
        .args(["fetch", "origin", &fetch_ref]);

    let output = run_command_output(&mut command, "fetch pull request")?;
    if !output.status.success() {
        return Err(WorktreeError::CommandFailed(command_failure_message(
            "fetch pull request",
            &output,
        )));
    }

    Ok(())
}

pub(crate) fn create_discrete_clone(
    source_repo_root: &Path,
    checkout_path: &Path,
    branch_name: &str,
) -> Result<(), WorktreeError> {
    let clone_source = source_repo_root.to_str().ok_or_else(|| {
        WorktreeError::InvalidInput("repository path contains invalid UTF-8".to_owned())
    })?;
    let checkout_target = checkout_path.to_str().ok_or_else(|| {
        WorktreeError::InvalidInput("checkout path contains invalid UTF-8".to_owned())
    })?;

    let source_repo = git2::Repository::open(source_repo_root).map_err(|error| {
        WorktreeError::GitOperation(format!(
            "failed to open source repository `{}`: {error}",
            source_repo_root.display()
        ))
    })?;
    let origin_url = source_repo
        .find_remote("origin")
        .ok()
        .and_then(|remote| remote.url().map(str::to_owned));

    let cloned_repo = git2::Repository::clone(clone_source, checkout_target).map_err(|error| {
        WorktreeError::GitOperation(format!(
            "failed to clone `{}` into `{}`: {error}",
            source_repo_root.display(),
            checkout_path.display()
        ))
    })?;

    if let Some(origin_url) = origin_url.as_deref() {
        let _ = cloned_repo.remote_set_url("origin", origin_url);
    }

    let head_commit = cloned_repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| {
            WorktreeError::GitOperation(format!("failed to resolve cloned HEAD: {error}"))
        })?;
    cloned_repo
        .branch(branch_name, &head_commit, false)
        .map_err(|error| {
            WorktreeError::GitOperation(format!("failed to create branch `{branch_name}`: {error}"))
        })?;

    let branch_ref = format!("refs/heads/{branch_name}");
    cloned_repo.set_head(&branch_ref).map_err(|error| {
        WorktreeError::GitOperation(format!("failed to set HEAD to `{branch_name}`: {error}"))
    })?;

    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    cloned_repo
        .checkout_head(Some(&mut checkout))
        .map_err(|error| {
            WorktreeError::GitOperation(format!("failed to check out `{branch_name}`: {error}"))
        })?;

    Ok(())
}

pub(crate) fn create_discrete_clone_from_pull_request(
    source_repo_root: &Path,
    checkout_path: &Path,
    pull_request_number: u64,
    branch_name: &str,
) -> Result<(), WorktreeError> {
    let clone_source = source_repo_root.to_str().ok_or_else(|| {
        WorktreeError::InvalidInput("repository path contains invalid UTF-8".to_owned())
    })?;
    let checkout_target = checkout_path.to_str().ok_or_else(|| {
        WorktreeError::InvalidInput("checkout path contains invalid UTF-8".to_owned())
    })?;

    let source_repo = git2::Repository::open(source_repo_root).map_err(|error| {
        WorktreeError::GitOperation(format!(
            "failed to open source repository `{}`: {error}",
            source_repo_root.display()
        ))
    })?;
    let origin_url = source_repo
        .find_remote("origin")
        .ok()
        .and_then(|remote| remote.url().map(str::to_owned));

    let cloned_repo = git2::Repository::clone(clone_source, checkout_target).map_err(|error| {
        WorktreeError::GitOperation(format!(
            "failed to clone `{}` into `{}`: {error}",
            source_repo_root.display(),
            checkout_path.display()
        ))
    })?;

    if let Some(origin_url) = origin_url.as_deref() {
        let _ = cloned_repo.remote_set_url("origin", origin_url);
    }

    fetch_pull_request_head_into_branch(checkout_path, pull_request_number, branch_name)?;

    let branch_ref = format!("refs/heads/{branch_name}");
    cloned_repo.set_head(&branch_ref).map_err(|error| {
        WorktreeError::GitOperation(format!("failed to set HEAD to `{branch_name}`: {error}"))
    })?;

    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    cloned_repo
        .checkout_head(Some(&mut checkout))
        .map_err(|error| {
            WorktreeError::GitOperation(format!("failed to check out `{branch_name}`: {error}"))
        })?;

    Ok(())
}

pub(crate) fn rollback_created_checkout(
    repo_root: &Path,
    worktree_path: &Path,
    checkout_kind: CheckoutKind,
    branch_name: &str,
) -> Result<(), WorktreeError> {
    match checkout_kind {
        CheckoutKind::LinkedWorktree => {
            worktree::remove(repo_root, worktree_path, true)
                .map_err(|error| WorktreeError::GitOperation(error.to_string()))?;
            if !branch_name.trim().is_empty() {
                worktree::delete_branch(repo_root, branch_name).map_err(|error| {
                    WorktreeError::GitOperation(format!(
                        "failed to delete branch `{branch_name}`: {error}"
                    ))
                })?;
            }
        },
        CheckoutKind::DiscreteClone => {
            if worktree_path.exists() {
                fs::remove_dir_all(worktree_path).map_err(|error| {
                    WorktreeError::Io(format!(
                        "failed to remove checkout `{}` during rollback: {error}",
                        worktree_path.display()
                    ))
                })?;
            }
        },
    }

    Ok(())
}

pub(crate) fn managed_preview_request_matches(
    modal: &CreateModal,
    modal_instance_id: u64,
    generation: u64,
) -> bool {
    modal.instance_id == modal_instance_id && modal.managed_preview_generation == generation
}

pub(crate) fn create_modal_branch_preview_matches(
    modal: &CreateModal,
    modal_instance_id: u64,
    generation: u64,
) -> bool {
    modal.instance_id == modal_instance_id && modal.branch_preview_generation == generation
}

#[cfg(test)]
mod worktree_lifecycle_tests {
    use super::*;

    fn sample_create_modal() -> CreateModal {
        CreateModal {
            instance_id: 7,
            tab: CreateModalTab::LocalWorktree,
            repository_path: "/tmp/repo".to_owned(),
            repository_path_cursor: 9,
            worktree_name: "issue-42".to_owned(),
            worktree_name_cursor: 8,
            checkout_kind: CheckoutKind::LinkedWorktree,
            worktree_active_field: CreateWorktreeField::WorktreeName,
            pr_reference: String::new(),
            pr_reference_cursor: 0,
            review_active_field: CreateReviewPrField::PullRequestReference,
            host_index: 0,
            host_dropdown_open: false,
            clone_url: String::new(),
            clone_url_cursor: 0,
            outpost_name: String::new(),
            outpost_name_cursor: 0,
            outpost_active_field: CreateOutpostField::CloneUrl,
            daemon_managed_target: Some(ManagedDaemonTarget::Primary),
            managed_preview: None,
            managed_preview_loading: false,
            managed_preview_error: None,
            managed_preview_generation: 3,
            branch_preview_generation: 5,
            local_branch_preview: "codex/issue-42".to_owned(),
            review_branch_preview: "codex/pr-42".to_owned(),
            outpost_branch_preview: "codex/outpost".to_owned(),
            issue_context: None,
            is_creating: false,
            creating_status: None,
            error: None,
        }
    }

    #[test]
    fn review_worktree_name_preview_prefers_explicit_name() {
        assert_eq!(
            review_worktree_name_preview("#42", "review auth fix"),
            Some("review-auth-fix".to_owned())
        );
    }

    #[test]
    fn review_worktree_name_preview_falls_back_to_pr_number() {
        assert_eq!(
            review_worktree_name_preview("https://github.com/penso/arbor/pull/42", ""),
            Some("pr-42".to_owned())
        );
    }

    #[test]
    fn default_review_worktree_name_uses_pr_number_and_title() {
        let pull_request = github_service::ReviewPullRequest {
            number: 42,
            title: "Fix auth callback race".to_owned(),
        };

        assert_eq!(
            default_review_worktree_name(&pull_request),
            "pr-42-fix-auth-callback-race"
        );
    }

    #[test]
    fn managed_preview_request_matches_current_modal_instance() {
        let modal = sample_create_modal();

        assert!(managed_preview_request_matches(&modal, 7, 3));
        assert!(!managed_preview_request_matches(&modal, 8, 3));
        assert!(!managed_preview_request_matches(&modal, 7, 4));
    }

    #[test]
    fn create_modal_branch_preview_matches_current_modal_instance() {
        let modal = sample_create_modal();

        assert!(create_modal_branch_preview_matches(&modal, 7, 5));
        assert!(!create_modal_branch_preview_matches(&modal, 8, 5));
        assert!(!create_modal_branch_preview_matches(&modal, 7, 6));
    }
}
