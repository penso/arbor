use super::*;

impl ArborWindow {
    pub(crate) fn refresh_worktree_pull_requests(&mut self, cx: &mut Context<Self>) {
        if self.worktree_prs_loading {
            return;
        }

        let rate_limit_expired = self.clear_expired_github_rate_limit();

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

        let tracked_branches: Vec<(PathBuf, String, String)> = self
            .worktrees
            .iter()
            .filter(|worktree| should_lookup_pull_request_for_worktree(worktree))
            .filter_map(|worktree| {
                repository_slug_by_group_key
                    .get(&worktree.group_key)
                    .cloned()
                    .map(|slug| (worktree.path.clone(), worktree.branch.clone(), slug))
            })
            .collect();
        let github_token = self.github_access_token();
        let github_service = self.github_service.clone();
        let tracked_paths: HashSet<PathBuf> = tracked_branches
            .iter()
            .map(|(path, ..)| path.clone())
            .collect();
        let rate_limit_remaining = self.github_rate_limit_remaining();

        let mut changed = rate_limit_expired;
        for worktree in &mut self.worktrees {
            let next_pr_loading =
                rate_limit_remaining.is_none() && tracked_paths.contains(&worktree.path);

            if worktree.pr_loading != next_pr_loading {
                worktree.pr_loading = next_pr_loading;
                changed = true;
            }
        }
        let cleared_untracked =
            clear_pull_request_data_for_untracked_worktrees(&mut self.worktrees, &tracked_paths);
        if cleared_untracked {
            changed = true;
        }

        let next_prs_loading = rate_limit_remaining.is_none() && !tracked_branches.is_empty();
        if self.worktree_prs_loading != next_prs_loading {
            self.worktree_prs_loading = next_prs_loading;
            changed = true;
        }

        if let Some(remaining) = rate_limit_remaining {
            if changed {
                self.sync_pull_request_cache_store(cx);
                cx.notify();
            }
            tracing::info!(
                remaining_seconds = remaining.as_secs(),
                tracked_worktrees = tracked_branches.len(),
                "skipping GitHub PR refresh because GitHub is rate limited"
            );
            return;
        }

        if tracked_branches.is_empty() {
            if changed {
                self.sync_pull_request_cache_store(cx);
                cx.notify();
            }
            return;
        }

        if changed {
            self.sync_pull_request_cache_store(cx);
            cx.notify();
        }

        tracing::info!(
            tracked_worktrees = tracked_branches.len(),
            refresh_interval_seconds = GITHUB_PR_REFRESH_INTERVAL.as_secs(),
            "refreshing GitHub PR details"
        );

        self.ensure_loading_animation(cx);

        cx.spawn(async move |this, cx| {
            let worker_count = tracked_branches.len().min(GITHUB_PR_REFRESH_CONCURRENCY);
            let (work_tx, work_rx) = smol::channel::unbounded::<(PathBuf, String, String)>();
            let (result_tx, result_rx) = smol::channel::unbounded::<(
                PathBuf,
                String,
                Option<u64>,
                Option<String>,
                Option<github_service::PrDetails>,
                Option<SystemTime>,
            )>();
            let stop_due_to_rate_limit = Arc::new(AtomicBool::new(false));

            for work_item in tracked_branches {
                if work_tx.send(work_item).await.is_err() {
                    break;
                }
            }
            drop(work_tx);

            for worker_index in 0..worker_count {
                let work_rx = work_rx.clone();
                let result_tx = result_tx.clone();
                let github_service = github_service.clone();
                let github_token = github_token.clone();
                let stop_due_to_rate_limit = stop_due_to_rate_limit.clone();

                cx.background_spawn(async move {
                    if let Some(delay) =
                        GITHUB_PR_REFRESH_WORKER_STAGGER.checked_mul(worker_index as u32)
                        && !delay.is_zero()
                    {
                        smol::Timer::after(delay).await;
                    }

                    while !stop_due_to_rate_limit.load(Ordering::Relaxed) {
                        let Ok((path, branch, repo_slug)) = work_rx.recv().await else {
                            break;
                        };
                        if stop_due_to_rate_limit.load(Ordering::Relaxed) {
                            break;
                        }

                        let lookup_branch = branch.clone();
                        let result = Self::lookup_worktree_pull_request(
                            github_service.as_ref(),
                            github_token.as_deref(),
                            path,
                            lookup_branch,
                            Some(repo_slug),
                        );
                        if result.4.is_some() {
                            stop_due_to_rate_limit.store(true, Ordering::Relaxed);
                        }

                        if result_tx
                            .send((result.0, branch, result.1, result.2, result.3, result.4))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
            }
            drop(result_tx);

            while let Ok((
                path_for_update,
                branch_for_update,
                next_num,
                next_url,
                next_details,
                rate_limited_until,
            )) = result_rx.recv().await
            {
                let _ = this.update(cx, |this, cx| {
                    let Some(worktree) = this
                        .worktrees
                        .iter_mut()
                        .find(|worktree| worktree.path == path_for_update)
                    else {
                        return;
                    };
                    if worktree.branch != branch_for_update {
                        return;
                    }

                    let preserve_cached_pr_data = should_preserve_cached_pr_data_on_rate_limit(
                        next_num,
                        next_url.as_deref(),
                        next_details.as_ref(),
                        rate_limited_until,
                    );
                    let mut changed = false;

                    if worktree.pr_loading {
                        worktree.pr_loading = false;
                        changed = true;
                    }
                    if !preserve_cached_pr_data && !worktree.pr_loaded {
                        worktree.pr_loaded = true;
                        changed = true;
                    }
                    if !preserve_cached_pr_data
                        && (worktree.pr_number != next_num
                            || worktree.pr_url != next_url
                            || worktree.pr_details != next_details)
                    {
                        worktree.pr_number = next_num;
                        worktree.pr_url = next_url;
                        worktree.pr_details = next_details;
                        changed = true;
                    }

                    if this.extend_github_rate_limit(rate_limited_until) {
                        changed = true;
                    }

                    let still_loading = this.worktrees.iter().any(|worktree| worktree.pr_loading);
                    if this.worktree_prs_loading != still_loading {
                        this.worktree_prs_loading = still_loading;
                        changed = true;
                    }

                    if changed {
                        this.sync_pull_request_cache_store(cx);
                        cx.notify();
                    }
                });
            }

            let _ = this.update(cx, |this, cx| {
                let mut changed = false;
                for worktree in &mut this.worktrees {
                    if worktree.pr_loading {
                        worktree.pr_loading = false;
                        changed = true;
                    }
                }
                if this.worktree_prs_loading {
                    this.worktree_prs_loading = false;
                    changed = true;
                }
                if changed {
                    this.sync_pull_request_cache_store(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn lookup_worktree_pull_request(
        github_service: &dyn github_service::GitHubService,
        github_token: Option<&str>,
        path: PathBuf,
        branch: String,
        repo_slug: Option<String>,
    ) -> (
        PathBuf,
        Option<u64>,
        Option<String>,
        Option<github_service::PrDetails>,
        Option<SystemTime>,
    ) {
        let (details, rate_limited_until) = repo_slug
            .as_ref()
            .map(|slug| github_service::pull_request_details(slug, &branch, github_token))
            .map(|outcome| (outcome.details, outcome.rate_limited_until))
            .unwrap_or((None, None));

        let (pr_number, pr_url) = if let Some(ref details) = details {
            (Some(details.number), Some(details.url.clone()))
        } else if rate_limited_until.is_some() {
            (None, None)
        } else {
            let pr_number = repo_slug.as_ref().and_then(|_| {
                github_pr_number_for_worktree(github_service, &path, &branch, github_token)
            });
            let pr_url = pr_number
                .and_then(|number| repo_slug.as_ref().map(|slug| github_pr_url(slug, number)));
            (pr_number, pr_url)
        };

        (path, pr_number, pr_url, details, rate_limited_until)
    }

    pub(crate) fn github_rate_limit_remaining(&self) -> Option<Duration> {
        self.github_rate_limited_until?
            .duration_since(SystemTime::now())
            .ok()
            .filter(|remaining| !remaining.is_zero())
    }

    pub(crate) fn clear_expired_github_rate_limit(&mut self) -> bool {
        if self.github_rate_limited_until.is_some() && self.github_rate_limit_remaining().is_none()
        {
            self.github_rate_limited_until = None;
            return true;
        }
        false
    }

    pub(crate) fn extend_github_rate_limit(
        &mut self,
        rate_limited_until: Option<SystemTime>,
    ) -> bool {
        let Some(rate_limited_until) = rate_limited_until else {
            return false;
        };
        if rate_limited_until <= SystemTime::now() {
            return false;
        }

        let next = match self.github_rate_limited_until {
            Some(current) if current >= rate_limited_until => current,
            _ => rate_limited_until,
        };
        if self.github_rate_limited_until == Some(next) {
            return false;
        }
        self.github_rate_limited_until = Some(next);
        true
    }
}

pub(crate) fn worktree_pull_request_cache_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn cached_pull_request_state_for_worktree<'a>(
    worktree: &WorktreeSummary,
    cache: &'a HashMap<String, ui_state_store::CachedPullRequestState>,
) -> Option<&'a ui_state_store::CachedPullRequestState> {
    cache
        .get(&worktree_pull_request_cache_key(&worktree.path))
        .filter(|cached| cached.branch == worktree.branch)
}

pub(crate) fn should_refresh_pull_requests_after_worktree_refresh(
    previous: &[WorktreeSummary],
    next: &[WorktreeSummary],
) -> bool {
    let previous_tracked: HashMap<&Path, &str> = previous
        .iter()
        .filter(|worktree| should_lookup_pull_request_for_worktree(worktree))
        .map(|worktree| (worktree.path.as_path(), worktree.branch.as_str()))
        .collect();

    let mut next_tracked_count = 0usize;
    for worktree in next
        .iter()
        .filter(|worktree| should_lookup_pull_request_for_worktree(worktree))
    {
        next_tracked_count += 1;

        if !worktree.pr_loaded {
            return true;
        }

        match previous_tracked.get(worktree.path.as_path()) {
            Some(previous_branch) if previous_branch == &worktree.branch.as_str() => {},
            _ => return true,
        }
    }

    next_tracked_count != previous_tracked.len()
}

pub(crate) fn should_show_worktree_pr_loading_indicator(worktree: &WorktreeSummary) -> bool {
    worktree.pr_loading && !worktree.pr_loaded
}

pub(crate) fn should_preserve_cached_pr_data_on_rate_limit(
    next_num: Option<u64>,
    next_url: Option<&str>,
    next_details: Option<&github_service::PrDetails>,
    rate_limited_until: Option<SystemTime>,
) -> bool {
    rate_limited_until.is_some()
        && next_num.is_none()
        && next_url.is_none()
        && next_details.is_none()
}

pub(crate) fn clear_pull_request_data_for_untracked_worktrees(
    worktrees: &mut [WorktreeSummary],
    tracked_paths: &HashSet<PathBuf>,
) -> bool {
    let mut cleared = false;

    for worktree in worktrees {
        if tracked_paths.contains(&worktree.path) {
            continue;
        }
        let had_pr_number = worktree.pr_number.take().is_some();
        let had_pr_url = worktree.pr_url.take().is_some();
        let had_pr_details = worktree.pr_details.take().is_some();
        if had_pr_number || had_pr_url || had_pr_details {
            cleared = true;
        }
    }

    cleared
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_worktree_summary() -> WorktreeSummary {
        WorktreeSummary {
            group_key: "/tmp/repo".to_owned(),
            checkout_kind: CheckoutKind::LinkedWorktree,
            repo_root: "/tmp/repo".into(),
            path: "/tmp/repo/wt-1".into(),
            label: "wt-1".to_owned(),
            branch: "feature/test".to_owned(),
            is_primary_checkout: false,
            pr_loading: false,
            pr_loaded: false,
            pr_number: None,
            pr_url: None,
            pr_details: None,
            branch_divergence: None,
            diff_summary: None,
            detected_ports: vec![],
            managed_processes: vec![],
            recent_turns: vec![],
            stuck_turn_count: 0,
            recent_agent_sessions: vec![],
            agent_state: None,
            agent_task: None,
            last_activity_unix_ms: None,
        }
    }

    #[test]
    fn pull_request_refresh_only_restarts_when_tracked_worktrees_change() {
        let mut previous = sample_worktree_summary();
        previous.pr_loaded = true;

        let mut next = sample_worktree_summary();
        next.pr_loaded = true;

        assert!(!should_refresh_pull_requests_after_worktree_refresh(
            &[previous],
            &[next]
        ));
    }

    #[test]
    fn pull_request_refresh_restarts_for_unresolved_or_changed_worktrees() {
        let mut previous = sample_worktree_summary();
        previous.pr_loaded = true;

        let unresolved = sample_worktree_summary();
        assert!(should_refresh_pull_requests_after_worktree_refresh(
            &[previous.clone()],
            &[unresolved]
        ));

        let mut changed_branch = sample_worktree_summary();
        changed_branch.pr_loaded = true;
        changed_branch.branch = "feature/other".to_owned();
        assert!(should_refresh_pull_requests_after_worktree_refresh(
            &[previous],
            &[changed_branch]
        ));
    }

    #[test]
    fn preserve_cached_pr_data_only_when_rate_limited_without_fresh_pr_data() {
        let pr = github_service::PrDetails {
            number: 42,
            title: "Keep the old PR metadata".to_owned(),
            url: "https://github.com/penso/arbor/pull/42".to_owned(),
            state: github_service::PrState::Open,
            additions: 1,
            deletions: 1,
            review_decision: github_service::ReviewDecision::Pending,
            mergeable: github_service::MergeableState::Mergeable,
            merge_state_status: github_service::MergeStateStatus::Clean,
            passed_checks: 0,
            checks_status: github_service::CheckStatus::Pending,
            checks: Vec::new(),
        };

        assert!(should_preserve_cached_pr_data_on_rate_limit(
            None,
            None,
            None,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(60)),
        ));
        assert!(!should_preserve_cached_pr_data_on_rate_limit(
            Some(pr.number),
            Some(pr.url.as_str()),
            Some(&pr),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(60)),
        ));
        assert!(!should_preserve_cached_pr_data_on_rate_limit(
            None, None, None, None,
        ));
    }

    #[test]
    fn clear_pull_request_data_for_untracked_worktrees_only_clears_stale_rows() {
        let mut tracked = sample_worktree_summary();
        tracked.pr_number = Some(7);
        tracked.pr_url = Some("https://github.com/penso/arbor/pull/7".to_owned());
        tracked.pr_details = Some(github_service::PrDetails {
            number: 7,
            title: "Tracked".to_owned(),
            url: "https://github.com/penso/arbor/pull/7".to_owned(),
            state: github_service::PrState::Open,
            additions: 1,
            deletions: 1,
            review_decision: github_service::ReviewDecision::Pending,
            mergeable: github_service::MergeableState::Mergeable,
            merge_state_status: github_service::MergeStateStatus::Clean,
            passed_checks: 0,
            checks_status: github_service::CheckStatus::Pending,
            checks: Vec::new(),
        });

        let mut stale = sample_worktree_summary();
        stale.path = "/tmp/repo/wt-stale".into();
        stale.label = "wt-stale".to_owned();
        stale.branch = "main".to_owned();
        stale.pr_number = Some(8);
        stale.pr_url = Some("https://github.com/penso/arbor/pull/8".to_owned());
        stale.pr_details = Some(github_service::PrDetails {
            number: 8,
            title: "Stale".to_owned(),
            url: "https://github.com/penso/arbor/pull/8".to_owned(),
            state: github_service::PrState::Open,
            additions: 1,
            deletions: 1,
            review_decision: github_service::ReviewDecision::Pending,
            mergeable: github_service::MergeableState::Mergeable,
            merge_state_status: github_service::MergeStateStatus::Clean,
            passed_checks: 0,
            checks_status: github_service::CheckStatus::Pending,
            checks: Vec::new(),
        });

        let tracked_path = tracked.path.clone();
        let mut worktrees = vec![tracked, stale];
        let tracked_paths = HashSet::from([tracked_path]);

        assert!(clear_pull_request_data_for_untracked_worktrees(
            &mut worktrees,
            &tracked_paths,
        ));
        assert_eq!(worktrees[0].pr_number, Some(7));
        assert!(worktrees[0].pr_details.is_some());
        assert_eq!(worktrees[1].pr_number, None);
        assert_eq!(worktrees[1].pr_url, None);
        assert_eq!(worktrees[1].pr_details, None);
    }
}
