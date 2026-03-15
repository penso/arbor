use super::*;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WorktreeInventoryRefreshResult {
    pub(crate) rows_changed: bool,
    pub(crate) created_terminal: bool,
}

impl WorktreeInventoryRefreshResult {
    pub(crate) fn visible_change(self) -> bool {
        self.rows_changed || self.created_terminal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorktreeInventoryRefreshMode {
    PreserveTerminalState,
    EnsureSelectedTerminal,
}

impl WorktreeInventoryRefreshMode {
    pub(crate) fn created_terminal<F>(self, ensure_selected_terminal: F) -> bool
    where
        F: FnOnce() -> bool,
    {
        match self {
            Self::PreserveTerminalState => false,
            Self::EnsureSelectedTerminal => ensure_selected_terminal(),
        }
    }
}

#[cfg(test)]
pub(crate) fn selected_worktree_terminal_was_created<F>(
    has_existing_terminal: bool,
    ensure_selected_terminal: F,
) -> bool
where
    F: FnOnce() -> bool,
{
    if has_existing_terminal {
        false
    } else {
        ensure_selected_terminal()
    }
}

pub(crate) fn worktree_rows_changed(
    previous: &[WorktreeSummary],
    next: &[WorktreeSummary],
) -> bool {
    if previous.len() != next.len() {
        return true;
    }

    previous.iter().zip(next.iter()).any(|(left, right)| {
        left.group_key != right.group_key
            || left.checkout_kind != right.checkout_kind
            || left.repo_root != right.repo_root
            || left.path != right.path
            || left.label != right.label
            || left.branch != right.branch
            || left.is_primary_checkout != right.is_primary_checkout
            || left.branch_divergence != right.branch_divergence
            || left.detected_ports != right.detected_ports
            || left.managed_processes != right.managed_processes
    })
}

pub(crate) fn next_active_worktree_index(
    previous_local_selection: Option<&Path>,
    active_repository_group_key: Option<&str>,
    worktrees: &[WorktreeSummary],
    preserve_non_local_selection: bool,
) -> Option<usize> {
    if preserve_non_local_selection {
        return None;
    }

    previous_local_selection
        .and_then(|path| worktrees.iter().position(|worktree| worktree.path == path))
        .or_else(|| {
            active_repository_group_key.and_then(|group_key| {
                worktrees
                    .iter()
                    .position(|worktree| worktree.group_key == group_key)
            })
        })
        .or_else(|| (!worktrees.is_empty()).then_some(0))
}

impl ArborWindow {
    pub(crate) fn refresh_worktree_inventory(
        &mut self,
        cx: &mut Context<Self>,
        mode: WorktreeInventoryRefreshMode,
    ) -> WorktreeInventoryRefreshResult {
        let queued_ui_state = self.queued_ui_state_base();
        let previous_local_selection = refresh_worktree_previous_local_selection(
            self.pending_local_worktree_selection.as_deref(),
            self.selected_local_worktree_path(),
            queued_ui_state.selected_sidebar_selection.as_ref(),
        );
        let active_repository_group_key = self
            .active_repository_index
            .and_then(|repository_index| self.repositories.get(repository_index))
            .map(|repository| repository.group_key.clone());
        let preserve_non_local_selection =
            self.active_outpost_index.is_some() || self.active_remote_worktree.is_some();
        let repositories = self.repositories.clone();
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
        let previous_branches: HashMap<PathBuf, String> = self
            .worktrees
            .iter()
            .map(|worktree| (worktree.path.clone(), worktree.branch.clone()))
            .collect();
        let previous_pr_loading: HashMap<PathBuf, bool> = self
            .worktrees
            .iter()
            .map(|worktree| (worktree.path.clone(), worktree.pr_loading))
            .collect();
        let previous_pr_loaded: HashMap<PathBuf, bool> = self
            .worktrees
            .iter()
            .map(|worktree| (worktree.path.clone(), worktree.pr_loaded))
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
        let previous_recent_turns: HashMap<PathBuf, Vec<AgentTurnSnapshot>> = self
            .worktrees
            .iter()
            .map(|worktree| (worktree.path.clone(), worktree.recent_turns.clone()))
            .collect();
        let previous_detected_ports: HashMap<PathBuf, Vec<DetectedPort>> = self
            .worktrees
            .iter()
            .map(|worktree| (worktree.path.clone(), worktree.detected_ports.clone()))
            .collect();
        let previous_recent_agent_sessions: HashMap<
            PathBuf,
            Vec<arbor_core::session::AgentSessionSummary>,
        > = self
            .worktrees
            .iter()
            .map(|worktree| {
                (
                    worktree.path.clone(),
                    worktree.recent_agent_sessions.clone(),
                )
            })
            .collect();
        let previous_stuck_turn_counts: HashMap<PathBuf, usize> = self
            .worktrees
            .iter()
            .map(|worktree| (worktree.path.clone(), worktree.stuck_turn_count))
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
        let persisted_pr_cache = self.last_persisted_ui_state.pull_request_cache.clone();
        let next_epoch = self.worktree_refresh_epoch.wrapping_add(1);
        self.worktree_refresh_epoch = next_epoch;
        self._worktree_refresh_task = Some(cx.spawn(async move |this, cx| {
            let (mut next_worktrees, refresh_errors) = cx
                .background_spawn(async move {
                    let mut refresh_errors = Vec::new();
                    let mut next_worktrees = Vec::new();
                    let mut seen_worktree_paths = HashSet::new();
                    for repository in &repositories {
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
                                Err(error) => refresh_errors.push(format!(
                                    "{} ({}): {error}",
                                    repository.label,
                                    checkout_root.path.display()
                                )),
                            }
                        }
                    }
                    (next_worktrees, refresh_errors)
                })
                .await;

            for worktree in &mut next_worktrees {
                let branch_unchanged = previous_branches
                    .get(&worktree.path)
                    .is_some_and(|previous_branch| previous_branch == &worktree.branch);
                worktree.pr_loading = branch_unchanged
                    && previous_pr_loading
                        .get(&worktree.path)
                        .copied()
                        .unwrap_or(false);
                worktree.pr_loaded = branch_unchanged
                    && previous_pr_loaded
                        .get(&worktree.path)
                        .copied()
                        .unwrap_or(false);
                worktree.diff_summary = previous_summaries.get(&worktree.path).copied();
                if branch_unchanged {
                    worktree.pr_number = previous_pr_numbers.get(&worktree.path).copied();
                    worktree.pr_url = previous_pr_urls.get(&worktree.path).cloned();
                    worktree.pr_details = previous_pr_details.get(&worktree.path).cloned();
                } else if let Some(cached) =
                    cached_pull_request_state_for_worktree(worktree, &persisted_pr_cache)
                {
                    worktree.apply_cached_pull_request_state(cached);
                }
                worktree.agent_state = previous_agent_states.get(&worktree.path).copied();
                worktree.agent_task = previous_agent_tasks.get(&worktree.path).cloned();
                worktree.detected_ports = previous_detected_ports
                    .get(&worktree.path)
                    .cloned()
                    .unwrap_or_default();
                worktree.recent_turns = previous_recent_turns
                    .get(&worktree.path)
                    .cloned()
                    .unwrap_or_default();
                worktree.recent_agent_sessions = previous_recent_agent_sessions
                    .get(&worktree.path)
                    .cloned()
                    .unwrap_or_default();
                worktree.stuck_turn_count = previous_stuck_turn_counts
                    .get(&worktree.path)
                    .copied()
                    .unwrap_or_default();
                let previous = previous_activity.get(&worktree.path).copied();
                worktree.last_activity_unix_ms = match (worktree.last_activity_unix_ms, previous) {
                    (Some(left), Some(right)) => Some(left.max(right)),
                    (left, right) => left.or(right),
                };
            }

            let _ =
                this.update(cx, |this, cx| {
                    if this.worktree_refresh_epoch != next_epoch {
                        return;
                    }

                    let should_refresh_pull_requests =
                        should_refresh_pull_requests_after_worktree_refresh(
                            &this.worktrees,
                            &next_worktrees,
                        );
                    let rows_changed = worktree_rows_changed(&this.worktrees, &next_worktrees);
                    this.worktrees = next_worktrees;
                    reconcile_worktree_agent_activity(this, false, cx);
                    this.worktree_stats_loading = this
                        .worktrees
                        .iter()
                        .any(|worktree| worktree.diff_summary.is_none());

                    this.active_worktree_index = next_active_worktree_index(
                        previous_local_selection.as_deref(),
                        active_repository_group_key.as_deref(),
                        &this.worktrees,
                        preserve_non_local_selection,
                    );
                    if this
                        .pending_local_worktree_selection
                        .as_ref()
                        .is_some_and(|path| {
                            this.worktrees
                                .iter()
                                .any(|worktree| worktree.path.as_path() == path.as_path())
                        })
                    {
                        this.pending_local_worktree_selection = None;
                    }
                    if this.pending_startup_worktree_restore
                        && (this.active_worktree().is_some() || refresh_errors.is_empty())
                    {
                        this.pending_startup_worktree_restore = false;
                    }
                    if this.right_pane_tab == RightPaneTab::FileTree
                        && this.file_tree_entries.is_empty()
                    {
                        this.rebuild_file_tree(cx);
                    }

                    this.active_terminal_by_worktree.retain(|path, _| {
                        this.worktrees
                            .iter()
                            .any(|worktree| worktree.path.as_path() == path.as_path())
                    });
                    this.diff_sessions.retain(|session| {
                        this.worktrees
                            .iter()
                            .any(|worktree| worktree.path == session.worktree_path)
                    });
                    if this.active_diff_session_id.is_some_and(|diff_id| {
                        !this
                            .diff_sessions
                            .iter()
                            .any(|session| session.id == diff_id)
                    }) {
                        this.active_diff_session_id = None;
                    }

                    this.sync_active_repository_from_selected_worktree();
                    this.sync_visible_repository_issue_tabs(cx);
                    this.sync_issue_cache_store(cx);
                    this.sync_pull_request_cache_store(cx);
                    this.sync_navigation_ui_state_store(cx);

                    if refresh_errors.is_empty() {
                        if this.notice.as_deref().is_some_and(|notice| {
                            notice.starts_with("failed to refresh worktrees:")
                        }) {
                            this.notice = None;
                        }
                    } else {
                        this.worktree_stats_loading = false;
                        this.notice = Some(format!(
                            "failed to refresh worktrees: {}",
                            refresh_errors.join(", ")
                        ));
                    }

                    this.refresh_worktree_diff_summaries(cx);
                    this.refresh_worktree_ports(cx);
                    this.refresh_agent_tasks(cx);
                    this.refresh_agent_sessions(cx);
                    if should_refresh_pull_requests {
                        this.refresh_worktree_pull_requests(cx);
                    }
                    if this.active_outpost_index.is_some() {
                        this.refresh_remote_changed_files(cx);
                    } else {
                        this.refresh_changed_files(cx);
                    }
                    this.sync_selected_worktree_notes(cx);
                    let created_terminal =
                        mode.created_terminal(|| this.ensure_selected_worktree_terminal(cx));
                    if created_terminal {
                        this.sync_daemon_session_store(cx);
                    }
                    if rows_changed || created_terminal {
                        cx.notify();
                    }
                });
        }));

        WorktreeInventoryRefreshResult::default()
    }

    pub(crate) fn refresh_worktrees(&mut self, cx: &mut Context<Self>) {
        tracing::debug!("refreshing worktrees");
        let refresh = self
            .refresh_worktree_inventory(cx, WorktreeInventoryRefreshMode::EnsureSelectedTerminal);
        if refresh.visible_change() {
            cx.notify();
        }
    }

    pub(crate) fn refresh_worktree_diff_summaries(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn refresh_agent_tasks(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn refresh_agent_sessions(&mut self, cx: &mut Context<Self>) {
        let worktree_paths: Vec<PathBuf> = self
            .worktrees
            .iter()
            .filter(|worktree| worktree.recent_agent_sessions.is_empty())
            .map(|worktree| worktree.path.clone())
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
                            let sessions = arbor_core::session::recent_agent_sessions(&path, 6);
                            (path, sessions)
                        })
                        .collect::<Vec<_>>()
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let mut changed = false;
                for (path, sessions) in results {
                    if let Some(worktree) = this
                        .worktrees
                        .iter_mut()
                        .find(|worktree| worktree.path == path)
                        && worktree.recent_agent_sessions != sessions
                    {
                        worktree.recent_agent_sessions = sessions;
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
}

#[cfg(test)]
mod tests {
    use {super::*, std::cell::Cell};

    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn sample_worktree_summary() -> WorktreeSummary {
        crate::tests::sample_worktree_summary()
    }

    #[test]
    fn worktree_rows_changed_detects_external_worktree_addition() {
        let previous = sample_worktree_summary();
        let current = sample_worktree_summary();
        let mut external = sample_worktree_summary();
        external.path = "/tmp/repo/wt-external".into();
        external.label = "wt-external".to_owned();
        external.branch = "feature/external".to_owned();

        assert!(worktree_rows_changed(&[previous], &[current, external]));
    }

    #[test]
    fn selected_worktree_terminal_existing_session_is_not_reported_as_created() {
        let spawn_called = Cell::new(false);

        let created = selected_worktree_terminal_was_created(true, || {
            spawn_called.set(true);
            true
        });

        assert!(!created);
        assert!(!spawn_called.get());
    }

    #[test]
    fn selected_worktree_terminal_reports_spawn_result_when_missing() {
        assert!(selected_worktree_terminal_was_created(false, || { true }));
        assert!(!selected_worktree_terminal_was_created(false, || false));
    }

    #[test]
    fn background_inventory_refresh_does_not_recreate_selected_terminal() {
        let ensure_called = Cell::new(false);

        let created = WorktreeInventoryRefreshMode::PreserveTerminalState.created_terminal(|| {
            ensure_called.set(true);
            true
        });

        assert!(!created);
        assert!(!ensure_called.get());
    }

    #[test]
    fn explicit_inventory_refresh_reports_selected_terminal_creation() {
        assert!(WorktreeInventoryRefreshMode::EnsureSelectedTerminal.created_terminal(|| true));
        assert!(!WorktreeInventoryRefreshMode::EnsureSelectedTerminal.created_terminal(|| false));
    }

    #[test]
    fn next_active_worktree_index_preserves_non_local_selection() {
        let worktree = sample_worktree_summary();
        let group_key = worktree.group_key.clone();

        assert_eq!(
            next_active_worktree_index(None, Some(group_key.as_str()), &[worktree], true),
            None
        );
    }

    #[test]
    fn next_active_worktree_index_restores_previous_local_selection() {
        let first = sample_worktree_summary();
        let mut second = sample_worktree_summary();
        second.path = "/tmp/repo/wt-two".into();
        second.label = "wt-two".to_owned();
        second.branch = "feature/two".to_owned();
        let second_path = second.path.clone();
        let first_group_key = first.group_key.clone();

        assert_eq!(
            next_active_worktree_index(
                Some(second_path.as_path()),
                Some(first_group_key.as_str()),
                &[first, second],
                false,
            ),
            Some(1)
        );
    }
}
