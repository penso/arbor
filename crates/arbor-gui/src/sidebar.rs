use super::*;

impl ArborWindow {
    pub(crate) fn render_left_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.left_pane_visible {
            let theme = self.theme();
            let repositories = self.repositories.clone();
            let worktrees = self.worktrees.clone();
            let mut pane = div()
                .id("collapsed-left-pane")
                .w(px(40.))
                .h_full()
                .flex_none()
                .bg(rgb(theme.sidebar_bg))
                .flex()
                .flex_col()
                .items_center()
                .pt_2()
                .gap_1()
                .overflow_y_scroll();

            for (repo_index, repository) in repositories.iter().enumerate() {
                let repository_github_url = repository
                    .github_repo_slug
                    .as_ref()
                    .map(|repo_slug| github_repo_url(repo_slug));
                let repo_worktrees: Vec<(usize, &WorktreeSummary)> = worktrees
                    .iter()
                    .enumerate()
                    .filter(|(_, w)| w.group_key == repository.group_key)
                    .collect();

                // Add spacing between repo groups (not before the first)
                if repo_index > 0 {
                    pane = pane.child(div().h(px(4.)));
                }

                // Repo icon row: circular avatar or GitHub icon
                let repo_icon = match (repository.avatar_url.clone(), repository_github_url.clone())
                {
                    (Some(url), Some(github_url)) => div()
                        .id(("collapsed-repository-github-link", repo_index))
                        .size(px(32.))
                        .rounded_md()
                        .overflow_hidden()
                        .cursor_pointer()
                        .hover(|this| this.opacity(0.9))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_external_url(&github_url, cx);
                            cx.stop_propagation();
                        }))
                        .child(img(url).size_full().rounded_md().with_fallback(move || {
                            div()
                                .size_full()
                                .font_family(FONT_MONO)
                                .text_size(px(14.))
                                .text_color(rgb(theme.text_muted))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child("\u{f09b}")
                                .into_any_element()
                        }))
                        .into_any_element(),
                    (Some(url), None) => div()
                        .size(px(32.))
                        .rounded_md()
                        .overflow_hidden()
                        .child(img(url).size_full().rounded_md().with_fallback(move || {
                            div()
                                .size_full()
                                .font_family(FONT_MONO)
                                .text_size(px(14.))
                                .text_color(rgb(theme.text_muted))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child("\u{f09b}")
                                .into_any_element()
                        }))
                        .into_any_element(),
                    (None, Some(github_url)) => div()
                        .id(("collapsed-repository-github-link", repo_index))
                        .size(px(24.))
                        .font_family(FONT_MONO)
                        .text_size(px(14.))
                        .text_color(rgb(theme.text_muted))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|this| this.opacity(0.9))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_external_url(&github_url, cx);
                            cx.stop_propagation();
                        }))
                        .child("\u{f09b}")
                        .into_any_element(),
                    (None, None) => div()
                        .size(px(24.))
                        .font_family(FONT_MONO)
                        .text_size(px(14.))
                        .text_color(rgb(theme.text_muted))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("\u{f09b}")
                        .into_any_element(),
                };
                pane = pane.child(repo_icon);

                let selection_epoch = self.worktree_selection_epoch;
                for (wt_index, worktree) in repo_worktrees {
                    let is_active = self.active_worktree_index == Some(wt_index);
                    let first_char: String = worktree
                        .branch
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .collect();

                    let cell = div()
                        .id(("collapsed-worktree", wt_index))
                        .cursor_pointer()
                        .size(px(30.))
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(if is_active {
                            theme.accent
                        } else {
                            theme.border
                        }))
                        .bg(rgb(if is_active {
                            theme.panel_active_bg
                        } else {
                            theme.panel_bg
                        }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if is_active {
                            theme.text_primary
                        } else {
                            theme.text_muted
                        }))
                        .child(first_char)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_worktree(wt_index, window, cx);
                        }));
                    if is_active {
                        pane = pane.child(cell.with_animation(
                            ("collapsed-wt-select", selection_epoch),
                            Animation::new(Duration::from_millis(150)).with_easing(ease_in_out),
                            |el, delta| el.opacity(0.8 + 0.2 * delta),
                        ));
                    } else {
                        pane = pane.child(cell.opacity(0.8));
                    }
                }
            }

            return pane;
        }
        let theme = self.theme();
        let repositories = self.repositories.clone();
        let worktrees = self.worktrees.clone();
        div()
            .id("left-pane")
            .w(px(self.left_pane_width))
            .h_full()
            .bg(rgb(theme.sidebar_bg))
            .flex()
            .flex_col()
            .child(
                div()
                    .id("worktrees-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .px_2()
                    .pt_2()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(repositories.into_iter().enumerate().map(
                        |(repository_index, repository)| {
                            let is_collapsed =
                                self.collapsed_repositories.contains(&repository_index);
                            let repository_avatar_url = repository.avatar_url.clone();
                            let repository_github_url = repository
                                .github_repo_slug
                                .as_ref()
                                .map(|repo_slug| github_repo_url(repo_slug));
                            let repo_worktrees: Vec<(usize, WorktreeSummary)> = worktrees
                                .iter()
                                .cloned()
                                .enumerate()
                                .filter(|(_, worktree)| {
                                    worktree.group_key == repository.group_key
                                })
                                .collect();
                            let repo_agent_dot_color = if is_collapsed {
                                if repo_worktrees
                                    .iter()
                                    .any(|(_, wt)| wt.agent_state == Some(AgentState::Working))
                                {
                                    Some(0xe5c07b_u32)
                                } else if repo_worktrees
                                    .iter()
                                    .any(|(_, wt)| wt.agent_state == Some(AgentState::Waiting))
                                {
                                    Some(0x61afef_u32)
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            let repo_outposts: Vec<(usize, OutpostSummary)> = self
                                .outposts
                                .iter()
                                .cloned()
                                .enumerate()
                                .filter(|(_, outpost)| outpost.repo_root == repository.root)
                                .collect();
                            let repository_sidebar_tab =
                                self.repository_sidebar_tab_for_group(&repository.group_key);
                            let repository_issue_target =
                                self.issue_target_for_repository(&repository);
                            let repository_group_key = repository.group_key.clone();
                                    let chevron_repository_issue_target =
                                        repository_issue_target.clone();

                            div()
                                .id(("repository-group", repository_index))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("repository-row", repository_index))
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .h(px(32.))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.select_repository(repository_index, cx);
                                        }))
                                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            this.repository_context_menu = Some(RepositoryContextMenu {
                                                repository_index,
                                                position: event.position,
                                            });
                                            cx.notify();
                                        }))
                                        // GitHub icon or avatar outside the cell
                                        .child(
                                            match (
                                                repository_avatar_url.clone(),
                                                repository_github_url.clone(),
                                            ) {
                                                (Some(url), Some(github_url)) => div()
                                                    .id((
                                                        "repository-github-link",
                                                        repository_index,
                                                    ))
                                                    .flex_none()
                                                    .size(px(20.))
                                                    .rounded_sm()
                                                    .overflow_hidden()
                                                    .cursor_pointer()
                                                    .hover(|this| this.opacity(0.9))
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.open_external_url(
                                                                &github_url,
                                                                cx,
                                                            );
                                                            cx.stop_propagation();
                                                        },
                                                    ))
                                                    .child(
                                                        img(url)
                                                            .size_full()
                                                            .rounded_sm()
                                                            .with_fallback(move || {
                                                                div()
                                                                    .size_full()
                                                                    .font_family(FONT_MONO)
                                                                    .text_size(px(12.))
                                                                    .text_color(rgb(
                                                                        theme.text_muted,
                                                                    ))
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .child("\u{f09b}")
                                                                    .into_any_element()
                                                            }),
                                                    )
                                                    .into_any_element(),
                                                (Some(url), None) => div()
                                                    .flex_none()
                                                    .size(px(20.))
                                                    .rounded_sm()
                                                    .overflow_hidden()
                                                    .child(
                                                        img(url)
                                                            .size_full()
                                                            .rounded_sm()
                                                            .with_fallback(move || {
                                                                div()
                                                                    .size_full()
                                                                    .font_family(FONT_MONO)
                                                                    .text_size(px(12.))
                                                                    .text_color(rgb(
                                                                        theme.text_muted,
                                                                    ))
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .child("\u{f09b}")
                                                                    .into_any_element()
                                                            }),
                                                    )
                                                    .into_any_element(),
                                                (None, Some(github_url)) => div()
                                                    .id((
                                                        "repository-github-link",
                                                        repository_index,
                                                    ))
                                                    .flex_none()
                                                    .font_family(FONT_MONO)
                                                    .text_size(px(12.))
                                                    .text_color(rgb(theme.text_muted))
                                                    .cursor_pointer()
                                                    .hover(|this| this.opacity(0.9))
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.open_external_url(
                                                                &github_url,
                                                                cx,
                                                            );
                                                            cx.stop_propagation();
                                                        },
                                                    ))
                                                    .child("\u{f09b}")
                                                    .into_any_element(),
                                                (None, None) => div()
                                                    .flex_none()
                                                    .font_family(FONT_MONO)
                                                    .text_size(px(12.))
                                                    .text_color(rgb(theme.text_muted))
                                                    .child("\u{f09b}")
                                                    .into_any_element(),
                                            },
                                        )
                                        // Cell with chevron, name, count, etc.
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .flex_1()
                                                        .flex()
                                                        .items_center()
                                                        .gap_1()
                                                        // Chevron toggle
                                                        .child(
                                                            div()
                                                                .id(("repo-chevron", repository_index))
                                                                .cursor_pointer()
                                                                .hover(|this| this.text_color(rgb(theme.text_primary)))
                                                                .text_size(px(16.))
                                                                .text_color(rgb(theme.text_muted))
                                                                .w(px(14.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .child(if is_collapsed {
                                                                    "\u{25B8}"
                                                                } else {
                                                                    "\u{25BE}"
                                                                })
                                                                .on_click(cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        let was_collapsed = this
                                                                            .collapsed_repositories
                                                                            .contains(&repository_index);
                                                                        if was_collapsed {
                                                                            this.collapsed_repositories
                                                                                .remove(&repository_index);
                                                                        } else {
                                                                            this.collapsed_repositories
                                                                                .insert(repository_index);
                                                                        }
                                                                        if was_collapsed {
                                                                            this.ensure_issues_loaded_for_target(
                                                                                chevron_repository_issue_target
                                                                                    .clone(),
                                                                                cx,
                                                                            );
                                                                        }
                                                                        this.sync_collapsed_repositories_store(cx);
                                                                        cx.stop_propagation();
                                                                        cx.notify();
                                                                    },
                                                                )),
                                                        )
                                                        // Repository name
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .text_ellipsis()
                                                        .text_sm()
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .text_color(rgb(theme.text_primary))
                                                        .child(repository.label.clone()),
                                                ),
                                        )
                                        .when_some(repo_agent_dot_color, |this, color| {
                                            this.child(
                                                div()
                                                    .flex_none()
                                                    .size(px(6.))
                                                    .rounded_full()
                                                    .bg(rgb(color)),
                                            )
                                        })
                                        .child(
                                            repository_add_worktree_button(
                                                &theme,
                                                ("repository-add-worktree", repository_index),
                                            )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    if this.active_repository_index
                                                        != Some(repository_index)
                                                    {
                                                        this.select_repository(repository_index, cx);
                                                    }
                                                    this.open_create_modal(
                                                        repository_index,
                                                        CreateModalTab::LocalWorktree,
                                                        cx,
                                                    );
                                                    cx.stop_propagation();
                                                })),
                                        ),
                                        )
                                )
                                .when(!is_collapsed, |this| {
                                    let selection_epoch = self.worktree_selection_epoch;
                                    let compact_sidebar = self.compact_sidebar;
                                    this.child(self.render_repository_sidebar_subtabs(
                                        repository_index,
                                        repository_group_key.clone(),
                                        repository_sidebar_tab,
                                        repository_issue_target.clone(),
                                        repo_worktrees
                                            .iter()
                                            .filter(|(_, worktree)| !worktree.is_primary_checkout)
                                            .count(),
                                        cx,
                                    ))
                                    .when(
                                        repository_sidebar_tab == RepositorySidebarTab::Worktrees,
                                        |this| {
                                            this.child(self.render_repository_worktree_sidebar(
                                                repository_index,
                                                &repository,
                                                &repo_worktrees,
                                                &repo_outposts,
                                                selection_epoch,
                                                compact_sidebar,
                                                cx,
                                            ))
                                        },
                                    )
                                    .when(
                                        repository_sidebar_tab == RepositorySidebarTab::Issues,
                                        |this| {
                                            this.child(self.render_repository_issue_sidebar(
                                                repository_index,
                                                repository_issue_target.clone(),
                                                cx,
                                            ))
                                        },
                                    )
                                })
                        },
                    ))
                    // ── Remote repos from expanded LAN daemons ───────────
                    .children({
                        // Build remote repo group elements imperatively so we can use cx.listener()
                        let mut remote_elements: Vec<AnyElement> = Vec::new();
                        let mut remote_wt_id = 0_usize;
                        for (&daemon_index, state) in &self.remote_daemon_states {
                            if !state.expanded {
                                continue;
                            }
                            let daemon_url = state.client.base_url();
                            // Show loading placeholder in the repo list
                            if state.loading {
                                remote_elements.push(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .h(px(32.))
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .font_family(FONT_MONO)
                                                        .text_size(px(12.))
                                                        .text_color(rgb(theme.text_muted))
                                                        .child("\u{f233}"),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(theme.text_disabled))
                                                        .child(format!("{}@{} — loading…", "", state.hostname)),
                                                ),
                                        )
                                        .into_any_element(),
                                );
                                continue;
                            }
                            if let Some(ref err) = state.error {
                                remote_elements.push(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0xe06c75_u32))
                                                .child(format!("{}@{}: {err}", "", state.hostname)),
                                        )
                                        .into_any_element(),
                                );
                                continue;
                            }
                            for repo in &state.repositories {
                                let repo_label = format!("{}@{}", repo.label, state.hostname);
                                let repo_wts: Vec<_> = state
                                    .worktrees
                                    .iter()
                                    .filter(|w| w.repo_root == repo.root)
                                    .collect();
                                let wt_count = repo_wts.len();

                                // Build worktree row elements
                                let mut wt_rows: Vec<AnyElement> = Vec::new();
                                for wt in &repo_wts {
                                    let branch = wt.branch.clone();
                                    let dir_label = wt.path.rsplit('/').next()
                                        .unwrap_or(&wt.path).to_owned();
                                    let additions = wt.diff_additions.unwrap_or(0);
                                    let deletions = wt.diff_deletions.unwrap_or(0);
                                    let has_diff = additions > 0 || deletions > 0;
                                    let pr_number = wt.pr_number;
                                    let last_activity = wt.last_activity_unix_ms;
                                    let click_path = wt.path.clone();
                                    let row_id = remote_wt_id;
                                    remote_wt_id += 1;
                                    let is_active = self.active_remote_worktree.as_ref().is_some_and(
                                        |arw| arw.daemon_index == daemon_index && arw.worktree_path == Path::new(&wt.path),
                                    );

                                    wt_rows.push(
                                        div()
                                            .id(("remote-wt-row", row_id))
                                            .font_family(FONT_MONO)
                                            .cursor_pointer()
                                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                            .flex()
                                            .items_center()
                                            .on_click(cx.listener(
                                                move |this, _, window, cx| {
                                                    this.select_remote_worktree(
                                                        daemon_index,
                                                        click_path.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                },
                                            ))
                                            .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(if is_active { theme.accent } else { theme.border }))
                                                .bg(rgb(theme.panel_bg))
                                                .px_2()
                                                .py_1()
                                                .flex()
                                                .flex_row()
                                                .items_center()
                                                .gap(px(4.))
                                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                                .when(is_active, |this| {
                                                    this.bg(rgb(theme.panel_active_bg))
                                                        .border_color(rgb(theme.accent))
                                                })
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .w(px(18.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .text_size(px(16.))
                                                    .text_color(rgb(theme.text_muted))
                                                    .child("\u{e725}"),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(px(1.))
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(2.))
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex_1()
                                                            .overflow_hidden()
                                                            .whitespace_nowrap()
                                                            .text_ellipsis()
                                                            .text_xs()
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .text_color(rgb(theme.text_primary))
                                                            .child(branch),
                                                    )
                                                    .child({
                                                        let mut right = div()
                                                            .flex_none()
                                                            .flex()
                                                            .items_center()
                                                            .gap_1();
                                                        if has_diff {
                                                            if additions > 0 {
                                                                right = right.child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(rgb(0x72d69c))
                                                                        .child(format!("+{additions}")),
                                                                );
                                                            }
                                                            if deletions > 0 {
                                                                right = right.child(
                                                                    div()
                                                                        .text_xs()
                                                                        .text_color(rgb(0xeb6f92))
                                                                        .child(format!("-{deletions}")),
                                                                );
                                                            }
                                                        }
                                                        if let Some(activity_ms) = last_activity {
                                                            right = right.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(rgb(theme.text_disabled))
                                                                    .child(format_relative_time(activity_ms)),
                                                            );
                                                        }
                                                        right
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap_2()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .flex_1()
                                                            .overflow_hidden()
                                                            .whitespace_nowrap()
                                                            .text_ellipsis()
                                                            .text_xs()
                                                            .text_color(rgb(theme.text_disabled))
                                                            .child(dir_label),
                                                    )
                                                    .when_some(pr_number, |el, pr_num| {
                                                        el.child(
                                                            div()
                                                                .flex_none()
                                                                .text_xs()
                                                                .text_color(rgb(theme.accent))
                                                                .child(format!("#{pr_num}")),
                                                        )
                                                    }),
                                            )
                                            ) // text column
                                            ) // bordered cell
                                            .when(!is_active, |el| el.opacity(0.8))
                                            .into_any_element(),
                                    );
                                }

                                // Repo header + worktree rows
                                let avatar_url = repo.avatar_url.clone();
                                let icon: AnyElement = if let Some(url) = avatar_url {
                                    div()
                                        .flex_none()
                                        .size(px(20.))
                                        .rounded_sm()
                                        .overflow_hidden()
                                        .child(
                                            img(url)
                                                .size_full()
                                                .rounded_sm()
                                                .with_fallback(move || {
                                                    div()
                                                        .size_full()
                                                        .font_family(FONT_MONO)
                                                        .text_size(px(12.))
                                                        .text_color(rgb(theme.text_muted))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .child("\u{f233}")
                                                        .into_any_element()
                                                }),
                                        )
                                        .into_any_element()
                                } else {
                                    div()
                                        .flex_none()
                                        .font_family(FONT_MONO)
                                        .text_size(px(12.))
                                        .text_color(rgb(theme.text_muted))
                                        .child("\u{f233}")
                                        .into_any_element()
                                };

                                // "+" button to create worktree on remote
                                let plus_url = daemon_url.clone();
                                let plus_hostname = state.hostname.clone();
                                let plus_repo_root = repo.root.clone();
                                let plus_id = remote_wt_id;
                                remote_wt_id += 1;

                                remote_elements.push(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .h(px(32.))
                                                .child(icon)
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .child(
                                                            div()
                                                                .min_w_0()
                                                                .flex_1()
                                                                .flex()
                                                                .items_center()
                                                                .gap_1()
                                                                .child(
                                                                    div()
                                                                        .text_size(px(16.))
                                                                        .text_color(rgb(theme.text_muted))
                                                                        .w(px(14.))
                                                                        .flex()
                                                                        .items_center()
                                                                        .justify_center()
                                                                        .child("\u{25BE}"),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .min_w_0()
                                                                        .overflow_hidden()
                                                                        .whitespace_nowrap()
                                                                        .text_ellipsis()
                                                                        .text_sm()
                                                                        .font_weight(FontWeight::MEDIUM)
                                                                        .text_color(rgb(theme.text_primary))
                                                                        .child(repo_label),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .text_color(rgb(theme.text_disabled))
                                                                        .child(format!("{wt_count}")),
                                                                ),
                                                        )
                                                        // "+" button
                                                        .child(
                                                            repository_add_worktree_button(
                                                                &theme,
                                                                ("remote-repo-add-wt", plus_id),
                                                            )
                                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                                    this.open_remote_create_modal(
                                                                        plus_url.clone(),
                                                                        plus_hostname.clone(),
                                                                        plus_repo_root.clone(),
                                                                        cx,
                                                                    );
                                                                    cx.stop_propagation();
                                                                })),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .flex_col()
                                                .gap(px(6.))
                                                .children(wt_rows),
                                        )
                                        .into_any_element(),
                                );
                            }
                        }
                        remote_elements
                    }),
            )
            // ── LAN Daemons section ──────────────────────────────────────
            .when(!self.discovered_daemons.is_empty(), |pane| {
                let daemons = self.discovered_daemons.clone();
                pane.child(div().h(px(1.)).bg(rgb(theme.border)))
                    .child(
                        div()
                            .px_2()
                            .pt_2()
                            .pb_1()
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
                                            .text_size(px(14.))
                                            .text_color(rgb(theme.text_muted))
                                            .child("\u{f0ac}"), // globe/network icon
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(theme.text_muted))
                                            .child("LAN Daemons"),
                                    ),
                            )
                            .children(daemons.into_iter().enumerate().map(
                                |(daemon_index, daemon)| {
                                    let remote_state = self.remote_daemon_states.get(&daemon_index);
                                    let is_expanded = remote_state.is_some_and(|s| s.expanded);
                                    let is_loading = remote_state.is_some_and(|s| s.loading);
                                    let display_name = daemon.display_name().to_owned();
                                    let chevron = if is_expanded { "\u{f078}" } else { "\u{f054}" };
                                    let mut col = div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .id(("lan-daemon-row", daemon_index))
                                                .cursor_pointer()
                                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                                .flex()
                                                .items_center()
                                                .on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        this.toggle_discovered_daemon(
                                                            daemon_index,
                                                            cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .rounded_sm()
                                                        .border_1()
                                                        .border_color(rgb(if is_expanded {
                                                            theme.accent
                                                        } else {
                                                            theme.border
                                                        }))
                                                        .bg(rgb(if is_expanded {
                                                            theme.panel_active_bg
                                                        } else {
                                                            theme.panel_bg
                                                        }))
                                                        .px_2()
                                                        .py_1()
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap(px(4.))
                                                        .child(
                                                            div()
                                                                .flex_none()
                                                                .w(px(12.))
                                                                .font_family(FONT_MONO)
                                                                .text_size(px(10.))
                                                                .text_color(rgb(theme.text_muted))
                                                                .child(chevron),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_none()
                                                                .w(px(18.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .font_family(FONT_MONO)
                                                                .text_size(px(18.))
                                                                .text_color(rgb(theme.accent))
                                                                .child("\u{f233}"),
                                                        )
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .text_xs()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(theme.text_primary))
                                                                .child(display_name),
                                                        ),
                                                ),
                                        );

                                    // Loading/error status below the toggle
                                    if let Some(state) = remote_state
                                        && state.expanded
                                    {
                                        if is_loading {
                                            col = col.child(
                                                div()
                                                    .pl(px(30.))
                                                    .py_1()
                                                    .text_xs()
                                                    .text_color(rgb(theme.text_disabled))
                                                    .child("Loading…"),
                                            );
                                        } else if let Some(ref err) = state.error {
                                            col = col.child(
                                                div()
                                                    .pl(px(30.))
                                                    .py_1()
                                                    .text_xs()
                                                    .text_color(rgb(0xe06c75_u32))
                                                    .child(err.clone()),
                                            );
                                        }
                                    }

                                    col
                                },
                            )),
                    )
            })
            // ── Bottom bar ───────────────────────────────────────────────
            .child(div().h(px(1.)).bg(rgb(theme.border)))
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("open-add-repository")
                            .cursor_pointer()
                            .h(px(24.))
                            .w_full()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.panel_bg))
                            .hover(|s| {
                                s.bg(rgb(theme.panel_active_bg))
                                    .border_color(rgb(theme.accent))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_add_repository_picker(cx);
                            }))
                            .child(
                                div()
                                    .h_full()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap_1()
                                    .child(
                                        div()
                                            .font_family(FONT_MONO)
                                            .text_size(px(11.))
                                            .text_color(rgb(theme.accent))
                                            .child("\u{f067}"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(theme.text_primary))
                                            .child("Add Repository"),
                                    ),
                            ),
                    ),
            )
    }

    pub(crate) fn build_sidebar_order_for_group(
        &self,
        group_key: &str,
        repo_root: &Path,
    ) -> Vec<SidebarItemId> {
        let worktree_ids: Vec<SidebarItemId> = self
            .worktrees
            .iter()
            .filter(|worktree| worktree.group_key == group_key)
            .map(|worktree| SidebarItemId::Worktree(worktree.path.clone()))
            .collect();
        let outpost_ids: Vec<SidebarItemId> = self
            .outposts
            .iter()
            .filter(|outpost| outpost.repo_root == repo_root)
            .map(|outpost| SidebarItemId::Outpost(outpost.outpost_id.clone()))
            .collect();

        normalized_sidebar_order(
            self.sidebar_order.get(group_key).map(Vec::as_slice),
            worktree_ids,
            outpost_ids,
        )
    }

    pub(crate) fn handle_sidebar_item_drop(
        &mut self,
        source_id: &SidebarItemId,
        insert_before: usize,
        group_key: &str,
        repo_root: &Path,
        cx: &mut Context<Self>,
    ) {
        let items = self.build_sidebar_order_for_group(group_key, repo_root);
        let Some(reordered) = reordered_sidebar_items(&items, source_id, insert_before) else {
            return;
        };

        self.sidebar_order.insert(group_key.to_owned(), reordered);
        self.sync_sidebar_order_store(cx);
        cx.notify();
    }

    pub(crate) fn render_repository_worktree_sidebar(
        &mut self,
        repository_index: usize,
        repository: &RepositorySummary,
        repo_worktrees: &[(usize, WorktreeSummary)],
        repo_outposts: &[(usize, OutpostSummary)],
        selection_epoch: usize,
        compact_sidebar: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let group_key = repository.group_key.clone();
        let repo_root = repository.root.clone();
        let worktree_map: HashMap<PathBuf, (usize, WorktreeSummary)> = repo_worktrees
            .iter()
            .cloned()
            .map(|(index, worktree)| (worktree.path.clone(), (index, worktree)))
            .collect();
        let outpost_map: HashMap<String, (usize, OutpostSummary)> = repo_outposts
            .iter()
            .cloned()
            .map(|(index, outpost)| (outpost.outpost_id.clone(), (index, outpost)))
            .collect();
        let sidebar_order = self.build_sidebar_order_for_group(&group_key, &repo_root);
        let item_count = sidebar_order.len();
        let mut elements: Vec<AnyElement> = Vec::with_capacity(item_count.saturating_mul(2) + 1);

        for (slot, item_id) in sidebar_order.iter().cloned().enumerate() {
            elements.push(
                self.render_repository_sidebar_drop_zone(
                    repository_index,
                    slot,
                    group_key.clone(),
                    repo_root.clone(),
                    cx,
                )
                .into_any_element(),
            );

            match item_id {
                SidebarItemId::Worktree(path) => {
                    let Some((index, worktree)) = worktree_map.get(&path).cloned() else {
                        continue;
                    };
                    elements.push(self.render_repository_worktree_row(
                        slot,
                        group_key.as_str(),
                        repo_root.as_path(),
                        index,
                        worktree,
                        selection_epoch,
                        compact_sidebar,
                        cx,
                    ));
                },
                SidebarItemId::Outpost(outpost_id) => {
                    let Some((index, outpost)) = outpost_map.get(&outpost_id).cloned() else {
                        continue;
                    };
                    elements.push(self.render_repository_outpost_row(
                        slot,
                        group_key.as_str(),
                        repo_root.as_path(),
                        index,
                        outpost,
                        cx,
                    ));
                },
            }
        }

        elements.push(
            self.render_repository_sidebar_drop_zone(
                repository_index,
                item_count,
                group_key,
                repo_root,
                cx,
            )
            .into_any_element(),
        );

        div().flex().flex_col().children(elements)
    }

    pub(crate) fn render_repository_sidebar_drop_zone(
        &self,
        repository_index: usize,
        slot: usize,
        group_key: String,
        repo_root: PathBuf,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = self.theme();
        let can_drop_group_key = group_key.clone();
        let drop_group_key = group_key.clone();
        div()
            .id(ElementId::Name(
                format!("sidebar-drop-zone-{repository_index}-{slot}").into(),
            ))
            .h(px(6.))
            .mx(px(4.))
            .rounded_sm()
            .can_drop(move |value, _, _| {
                value
                    .downcast_ref::<DraggedSidebarItem>()
                    .is_some_and(|dragged| dragged.group_key == can_drop_group_key)
            })
            .drag_over::<DraggedSidebarItem>({
                let accent = theme.accent;
                move |style, _, _, _| style.bg(rgb(accent)).h(px(3.)).my(px(1.5))
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedSidebarItem, _, cx| {
                    if dragged.group_key == drop_group_key {
                        this.handle_sidebar_item_drop(
                            &dragged.item_id,
                            slot,
                            &drop_group_key,
                            &repo_root,
                            cx,
                        );
                    }
                }),
            )
    }

    pub(crate) fn render_repository_worktree_row(
        &self,
        slot: usize,
        group_key: &str,
        repo_root: &Path,
        index: usize,
        worktree: WorktreeSummary,
        selection_epoch: usize,
        compact_sidebar: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme();
        let is_active = self.active_worktree_index == Some(index);
        let diff_summary = worktree.diff_summary;
        let show_pr_loading_indicator = should_show_worktree_pr_loading_indicator(&worktree);
        let pr_number = worktree.pr_number;
        let pr_url = worktree.pr_url.clone();
        let is_merged_pr = worktree
            .pr_details
            .as_ref()
            .is_some_and(|pr| pr.state == github_service::PrState::Merged);
        let pr_badge_color = if is_merged_pr {
            0xbb9af7_u32
        } else {
            theme.accent
        };
        let branch_divergence = worktree.branch_divergence;
        let pr_details = worktree.pr_details.clone();
        let is_stuck = worktree.stuck_turn_count >= 2;
        let is_primary = worktree.is_primary_checkout;
        let attention = worktree_attention_indicator(&worktree);
        let activity_sparkline = worktree_activity_sparkline(&worktree);
        let detected_ports = worktree.detected_ports.clone();
        let agent_dot_color = match worktree.agent_state {
            Some(AgentState::Working) => Some(0xe5c07b_u32),
            Some(AgentState::Waiting) => Some(0x61afef_u32),
            None => None,
        };
        let drag_item_id = SidebarItemId::Worktree(worktree.path.clone());
        let drag_group_key = group_key.to_owned();
        let drop_group_key = group_key.to_owned();
        let drop_repo_root = repo_root.to_path_buf();
        let drag_label = worktree.branch.clone();
        let drag_icon = worktree.checkout_kind.icon().to_owned();
        let can_drop_group_key = drop_group_key.clone();

        let row = div()
            .id(("worktree-row", index))
            .font_family(FONT_MONO)
            .cursor_pointer()
            .rounded_sm()
            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
            .flex()
            .items_center()
            .on_drag(
                DraggedSidebarItem {
                    item_id: drag_item_id,
                    group_key: drag_group_key,
                    label: drag_label,
                    icon: drag_icon,
                    icon_color: theme.text_muted,
                    bg_color: theme.panel_active_bg,
                    border_color: theme.accent,
                    text_color: theme.text_primary,
                },
                |dragged, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| dragged.clone())
                },
            )
            .can_drop(move |value, _, _| {
                value
                    .downcast_ref::<DraggedSidebarItem>()
                    .is_some_and(|dragged| dragged.group_key == can_drop_group_key)
            })
            .drag_over::<DraggedSidebarItem>({
                let accent = theme.accent;
                move |style, _, _, _| style.border_color(rgb(accent)).border_t_2()
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedSidebarItem, _, cx| {
                    if dragged.group_key == drop_group_key {
                        this.handle_sidebar_item_drop(
                            &dragged.item_id,
                            slot,
                            &drop_group_key,
                            &drop_repo_root,
                            cx,
                        );
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                this.update_worktree_hover_mouse_position(event.position);
            }))
            .on_click(
                cx.listener(move |this, _, window, cx| this.select_worktree(index, window, cx)),
            )
            .when(
                !is_primary || worktree.checkout_kind == CheckoutKind::DiscreteClone,
                |this| {
                    this.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.worktree_context_menu = Some(WorktreeContextMenu {
                                worktree_index: index,
                                position: event.position,
                            });
                            this.worktree_hover_popover = None;
                            this._hover_show_task = None;
                            cx.notify();
                        }),
                    )
                },
            )
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                this.update_worktree_hover_mouse_position(window.mouse_position());
                if *hovered {
                    let mouse_position = window.mouse_position();
                    this.schedule_worktree_hover_popover_show(index, mouse_position.y, cx);
                } else if this
                    .worktree_hover_popover
                    .as_ref()
                    .is_some_and(|popover| popover.worktree_index == index)
                {
                    this.schedule_worktree_hover_popover_dismiss(index, cx);
                } else {
                    this.cancel_worktree_hover_popover_show();
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(if is_active {
                        theme.accent
                    } else {
                        theme.border
                    }))
                    .bg(rgb(theme.panel_bg))
                    .px_2()
                    .py(px(if compact_sidebar {
                        4.
                    } else {
                        6.
                    }))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                    .when(is_active, |this| {
                        this.bg(rgb(theme.panel_active_bg))
                            .border_color(rgb(theme.accent))
                    })
                    .when(is_merged_pr && !is_active, |this| this.opacity(0.72))
                    .child(
                        div()
                            .flex_none()
                            .w(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(if show_pr_loading_indicator {
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.accent))
                                    .child(loading_spinner_frame(self.loading_animation_frame))
                            } else {
                                div()
                                    .text_size(px(16.))
                                    .text_color(rgb(theme.text_muted))
                                    .child(worktree.checkout_kind.icon())
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(2.))
                                    .when_some(agent_dot_color, |this, color| {
                                        this.child(
                                            div()
                                                .flex_none()
                                                .size(px(6.))
                                                .rounded_full()
                                                .bg(rgb(color)),
                                        )
                                    })
                                    .when(is_stuck, |this| {
                                        this.child(
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(rgb(0xeb6f92))
                                                .child("\u{f071}"),
                                        )
                                    })
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(theme.text_primary))
                                            .child(if compact_sidebar {
                                                format!("{} · {}", worktree.label, worktree.branch)
                                            } else {
                                                worktree.branch.clone()
                                            }),
                                    )
                                    .child({
                                        let summary = diff_summary.unwrap_or_default();
                                        let show_diff_summary =
                                            summary.additions > 0 || summary.deletions > 0;
                                        let mut right =
                                            div().flex_none().flex().items_center().gap_1();

                                        if compact_sidebar {
                                            right = right.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(attention.color))
                                                    .child(attention.short_label),
                                            );
                                        }

                                        if self.worktree_stats_loading
                                            && diff_summary.is_none()
                                            && !compact_sidebar
                                        {
                                            right = right.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme.text_muted))
                                                    .child("..."),
                                            );
                                        } else if show_diff_summary && !compact_sidebar {
                                            if summary.additions > 0 {
                                                right = right.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0x72d69c))
                                                        .child(format!("+{}", summary.additions)),
                                                );
                                            }
                                            if summary.deletions > 0 {
                                                right = right.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0xeb6f92))
                                                        .child(format!("-{}", summary.deletions)),
                                                );
                                            }
                                        }

                                        if let Some(activity_ms) = worktree.last_activity_unix_ms {
                                            right = right.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme.text_disabled))
                                                    .child(format_relative_time(activity_ms)),
                                            );
                                        }

                                        if let Some(divergence) = branch_divergence
                                            && !compact_sidebar
                                        {
                                            if divergence.ahead > 0 {
                                                right = right.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0x72d69c))
                                                        .child(format!(
                                                            "\u{2191}{}",
                                                            divergence.ahead
                                                        )),
                                                );
                                            }
                                            if divergence.behind > 0 {
                                                right = right.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(rgb(0xe5c07b))
                                                        .child(format!(
                                                            "\u{2193}{}",
                                                            divergence.behind
                                                        )),
                                                );
                                            }
                                        }

                                        right
                                    }),
                            )
                            .when(!compact_sidebar, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .overflow_hidden()
                                                .whitespace_nowrap()
                                                .text_ellipsis()
                                                .text_xs()
                                                .text_color(rgb(theme.text_disabled))
                                                .child({
                                                    let task_or_label = worktree
                                                        .agent_task
                                                        .clone()
                                                        .unwrap_or_else(|| worktree.label.clone());
                                                    if activity_sparkline.is_empty() {
                                                        format!(
                                                            "{} · {}",
                                                            attention.label, task_or_label
                                                        )
                                                    } else {
                                                        format!(
                                                            "{} {} · {}",
                                                            attention.label,
                                                            activity_sparkline,
                                                            task_or_label
                                                        )
                                                    }
                                                }),
                                        )
                                        .when_some(pr_details.clone(), |this, pr| {
                                            let (checks_icon, checks_color) = match pr.checks_status
                                            {
                                                github_service::CheckStatus::Success => {
                                                    ("\u{f00c}", 0x72d69c_u32)
                                                },
                                                github_service::CheckStatus::Failure => {
                                                    ("\u{f00d}", 0xeb6f92_u32)
                                                },
                                                github_service::CheckStatus::Pending => {
                                                    ("\u{f192}", 0xe5c07b_u32)
                                                },
                                            };
                                            let (review_icon, _, review_color) =
                                                review_status_presentation(pr.review_decision);

                                            let mut badges = this
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(rgb(checks_color))
                                                        .child(checks_icon),
                                                )
                                                .child(
                                                    div()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(rgb(review_color))
                                                        .child(review_icon),
                                                );

                                            if pr.additions > 0 {
                                                badges = badges.child(
                                                    div()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(rgb(0x72d69c))
                                                        .child(format!("+{}", pr.additions)),
                                                );
                                            }
                                            if pr.deletions > 0 {
                                                badges = badges.child(
                                                    div()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(rgb(0xeb6f92))
                                                        .child(format!("-{}", pr.deletions)),
                                                );
                                            }

                                            let mut badges = badges;
                                            for port in detected_ports.iter().take(2) {
                                                let port_url = worktree_port_url(port);
                                                let port_id = format!(
                                                    "worktree-port-link-{index}-{}",
                                                    port.port
                                                );
                                                badges = badges.child(
                                                    div()
                                                        .id(ElementId::Name(port_id.into()))
                                                        .cursor_pointer()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(rgb(0x72d69c))
                                                        .hover(|this| {
                                                            this.text_color(rgb(theme.text_primary))
                                                        })
                                                        .child(worktree_port_badge_text(port))
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.open_external_url(
                                                                    &port_url, cx,
                                                                );
                                                                cx.stop_propagation();
                                                            },
                                                        )),
                                                );
                                            }

                                            badges
                                        })
                                        .when_some(pr_number, |this, pr_num| {
                                            let pr_text = format!("#{pr_num}");
                                            if let Some(pr_url) = pr_url.clone() {
                                                this.child(
                                                    div()
                                                        .id(("worktree-pr-link", index))
                                                        .cursor_pointer()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(rgb(pr_badge_color))
                                                        .hover(|this| {
                                                            this.text_color(rgb(theme.text_primary))
                                                        })
                                                        .child(pr_text)
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.open_external_url(&pr_url, cx);
                                                                cx.stop_propagation();
                                                            },
                                                        )),
                                                )
                                            } else {
                                                this.child(
                                                    div()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(rgb(pr_badge_color))
                                                        .child(pr_text),
                                                )
                                            }
                                        })
                                        .when(
                                            pr_details.is_none() && !detected_ports.is_empty(),
                                            |this| {
                                                detected_ports.iter().take(2).fold(
                                                    this,
                                                    |this, port| {
                                                        let port_url = worktree_port_url(port);
                                                        let port_id = format!(
                                                            "worktree-port-link-{index}-{}",
                                                            port.port
                                                        );
                                                        this.child(
                                                            div()
                                                                .id(ElementId::Name(port_id.into()))
                                                                .cursor_pointer()
                                                                .flex_none()
                                                                .text_xs()
                                                                .text_color(rgb(0x72d69c))
                                                                .hover(|this| {
                                                                    this.text_color(rgb(
                                                                        theme.text_primary
                                                                    ))
                                                                })
                                                                .child(worktree_port_badge_text(
                                                                    port,
                                                                ))
                                                                .on_click(cx.listener(
                                                                    move |this, _, _, cx| {
                                                                        this.open_external_url(
                                                                            &port_url, cx,
                                                                        );
                                                                        cx.stop_propagation();
                                                                    },
                                                                )),
                                                        )
                                                    },
                                                )
                                            },
                                        ),
                                )
                            }),
                    ),
            );

        if is_active {
            row.with_animation(
                ("worktree-select", selection_epoch),
                Animation::new(Duration::from_millis(150)).with_easing(ease_in_out),
                |el, delta| el.opacity(0.8 + 0.2 * delta),
            )
            .into_any_element()
        } else {
            row.opacity(0.8).into_any_element()
        }
    }

    pub(crate) fn render_repository_outpost_row(
        &self,
        slot: usize,
        group_key: &str,
        repo_root: &Path,
        outpost_index: usize,
        outpost: OutpostSummary,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme();
        let is_active = self.active_outpost_index == Some(outpost_index);
        let status_color = match outpost.status {
            arbor_core::outpost::OutpostStatus::Available => theme.accent,
            arbor_core::outpost::OutpostStatus::Unreachable => 0xeb6f92,
            arbor_core::outpost::OutpostStatus::NotCloned
            | arbor_core::outpost::OutpostStatus::Provisioning => theme.text_muted,
        };
        let drag_item_id = SidebarItemId::Outpost(outpost.outpost_id.clone());
        let drag_group_key = group_key.to_owned();
        let drop_group_key = group_key.to_owned();
        let drop_repo_root = repo_root.to_path_buf();
        let drag_label = format!("{}@{}", outpost.branch, outpost.hostname);
        let can_drop_group_key = drop_group_key.clone();

        div()
            .id(("outpost-row", outpost_index))
            .font_family(FONT_MONO)
            .cursor_pointer()
            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
            .flex()
            .items_center()
            .on_drag(
                DraggedSidebarItem {
                    item_id: drag_item_id,
                    group_key: drag_group_key,
                    label: drag_label,
                    icon: "\u{f0ac}".to_owned(),
                    icon_color: status_color,
                    bg_color: theme.panel_active_bg,
                    border_color: theme.accent,
                    text_color: theme.text_primary,
                },
                |dragged, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| dragged.clone())
                },
            )
            .can_drop(move |value, _, _| {
                value
                    .downcast_ref::<DraggedSidebarItem>()
                    .is_some_and(|dragged| dragged.group_key == can_drop_group_key)
            })
            .drag_over::<DraggedSidebarItem>({
                let accent = theme.accent;
                move |style, _, _, _| style.border_color(rgb(accent)).border_t_2()
            })
            .on_drop(
                cx.listener(move |this, dragged: &DraggedSidebarItem, _, cx| {
                    if dragged.group_key == drop_group_key {
                        this.handle_sidebar_item_drop(
                            &dragged.item_id,
                            slot,
                            &drop_group_key,
                            &drop_repo_root,
                            cx,
                        );
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_outpost(outpost_index, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.outpost_context_menu = Some(OutpostContextMenu {
                        outpost_index,
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(if is_active {
                        theme.accent
                    } else {
                        theme.border
                    }))
                    .bg(rgb(theme.panel_bg))
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .when(is_active, |this| this.bg(rgb(theme.panel_active_bg)))
                    .child(
                        div()
                            .flex_none()
                            .w(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(18.))
                            .text_color(rgb(status_color))
                            .child("\u{f0ac}"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div().flex().items_center().child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(theme.text_primary))
                                        .child(format!("{}@{}", outpost.branch, outpost.hostname)),
                                ),
                            )
                            .child(
                                div().flex().items_center().gap_2().child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_xs()
                                        .text_color(rgb(theme.text_disabled))
                                        .child(outpost.label.clone()),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn repository_sidebar_tab_for_group(&self, group_key: &str) -> RepositorySidebarTab {
        self.repository_sidebar_tabs
            .get(group_key)
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn set_repository_sidebar_tab_for_group(
        &mut self,
        group_key: &str,
        tab: RepositorySidebarTab,
        cx: &mut Context<Self>,
    ) {
        if tab == RepositorySidebarTab::Worktrees {
            self.repository_sidebar_tabs.remove(group_key);
        } else {
            self.repository_sidebar_tabs
                .insert(group_key.to_owned(), tab);
        }
        self.sync_repository_sidebar_tabs_store(cx);
    }

    pub(crate) fn render_repository_sidebar_subtabs(
        &self,
        repository_index: usize,
        repository_group_key: String,
        active_tab: RepositorySidebarTab,
        issue_target: IssueTarget,
        worktree_count: usize,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme();
        let issue_badge = self.issue_list_state(&issue_target).and_then(|state| {
            if state.loading && !state.loaded {
                Some("...".to_owned())
            } else if state.loaded || state.notice.is_some() || state.error.is_some() {
                Some(state.issues.len().to_string())
            } else {
                None
            }
        });
        let tab_button =
            |label: &'static str, tab: RepositorySidebarTab, badge_label: Option<String>| {
                let is_active = active_tab == tab;
                let group_key = repository_group_key.clone();
                let issue_target = issue_target.clone();
                div()
                    .id(ElementId::Name(
                        format!(
                            "repository-sidebar-tab-{repository_index}-{}",
                            label.to_ascii_lowercase()
                        )
                        .into(),
                    ))
                    .flex_1()
                    .h(px(24.))
                    .rounded_sm()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .bg(rgb(if is_active {
                        theme.panel_active_bg
                    } else {
                        theme.panel_bg
                    }))
                    .text_color(rgb(if is_active {
                        theme.text_primary
                    } else {
                        theme.text_muted
                    }))
                    .border_1()
                    .border_color(rgb(if is_active {
                        theme.accent
                    } else {
                        theme.border
                    }))
                    .hover(|this| this.text_color(rgb(theme.text_primary)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.active_repository_index != Some(repository_index) {
                            this.select_repository(repository_index, cx);
                        }
                        this.set_repository_sidebar_tab_for_group(&group_key, tab, cx);
                        if tab == RepositorySidebarTab::Issues {
                            this.ensure_issues_loaded_for_target(issue_target.clone(), cx);
                        } else {
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(4.))
                            .child(label)
                            .when_some(badge_label, |this, badge_label| {
                                this.child(
                                    div()
                                        .rounded_full()
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .bg(rgb(theme.panel_bg))
                                        .min_w(px(14.))
                                        .h(px(14.))
                                        .px_1()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(10.))
                                        .font_family(FONT_MONO)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(if is_active {
                                            theme.text_muted
                                        } else {
                                            theme.text_disabled
                                        }))
                                        .child(badge_label),
                                )
                            }),
                    )
            };

        div()
            .w_full()
            .px_1()
            .flex()
            .gap_1()
            .child(tab_button(
                "Worktrees",
                RepositorySidebarTab::Worktrees,
                Some(worktree_count.to_string()),
            ))
            .child(tab_button(
                "Issues",
                RepositorySidebarTab::Issues,
                issue_badge,
            ))
    }

    pub(crate) fn render_repository_issue_sidebar(
        &mut self,
        repository_index: usize,
        issue_target: IssueTarget,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme();
        let issue_state = self
            .issue_list_state(&issue_target)
            .cloned()
            .unwrap_or_default();
        let issue_loading = issue_state.loading;
        let issue_error = issue_state.error.clone();
        let issue_notice = issue_state.notice.clone();
        let issue_rows = issue_state.issues.clone();
        let modal_source_label = issue_state
            .source
            .as_ref()
            .map(issue_modal_source_label)
            .unwrap_or_else(|| "Issue".to_owned());
        let mut content = div().w_full().min_w_0().flex().flex_col().gap_1();

        if let Some(error) = issue_error.clone() {
            content = content.child(
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
            );
        }

        if let Some(notice) = issue_notice.clone() {
            content = content.child(
                div()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.panel_bg))
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child(notice),
            );
        }

        if issue_loading && issue_rows.is_empty() {
            content = content.child(
                div()
                    .px_2()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child("Loading issues…"),
            );
        } else if issue_error.is_none() && issue_notice.is_none() && issue_rows.is_empty() {
            content = content.child(
                div()
                    .px_2()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child("No issues found."),
            );
        }

        for (row_id, issue) in issue_rows.into_iter().enumerate() {
            let issue_target = issue_target.clone();
            let issue_source_label = modal_source_label.clone();
            let issue_context = issue.clone();
            let issue_url = issue.url.clone();
            let has_issue_url = issue_url.is_some();
            let linked_review = issue.linked_review.clone();
            let issue_relative_updated_at = issue
                .updated_at
                .as_deref()
                .and_then(relative_time_from_rfc3339_utc);
            let issue_status_color = if linked_review.is_some() {
                theme.accent
            } else if issue.linked_branch.is_some() {
                0xe5c07b
            } else {
                theme.text_disabled
            };
            let (title_badge_label, issue_title_text) = split_issue_title_badge(&issue.title);
            let issue_type_label = issue
                .issue_type
                .as_ref()
                .map(|issue_type| issue_type.name.clone())
                .or(title_badge_label.clone());
            let issue_type_color = issue
                .issue_type
                .as_ref()
                .and_then(|issue_type| issue_type.color.as_deref())
                .map(str::to_owned);
            let type_key = issue_type_label
                .as_deref()
                .map(normalize_issue_badge_key)
                .unwrap_or_default();
            let issue_labels: Vec<_> = issue
                .labels
                .iter()
                .filter(|label| normalize_issue_badge_key(&label.name) != type_key)
                .take(2)
                .cloned()
                .collect();

            content = content.child(
                div()
                    .id(ElementId::Name(
                        format!("repository-issue-row-{repository_index}-{row_id}").into(),
                    ))
                    .w_full()
                    .min_w_0()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_issue_details_modal_for_target(
                            issue_target.clone(),
                            issue_source_label.clone(),
                            issue_context.clone(),
                            window,
                            cx,
                        );
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .w_full()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.panel_bg))
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .px_2()
                            .py(px(6.))
                            .flex()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .mt(px(5.))
                                    .size(px(6.))
                                    .rounded_full()
                                    .bg(rgb(issue_status_color)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .w_full()
                                    .flex()
                                    .flex_1()
                                    .flex_col()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .items_start()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .flex()
                                                    .items_center()
                                                    .flex_wrap()
                                                    .gap_2()
                                                    .child(div().min_w_0().when_some(
                                                        issue_url.clone(),
                                                        |this, issue_url| {
                                                            this.cursor_pointer()
                                                                .text_xs()
                                                                .font_family(FONT_MONO)
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .whitespace_nowrap()
                                                                .text_color(rgb(theme.accent))
                                                                .hover(|this| {
                                                                    this.text_color(rgb(
                                                                        theme.text_primary,
                                                                    ))
                                                                })
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    cx.listener(move |this, _, _, cx| {
                                                                        this.open_external_url(
                                                                            &issue_url,
                                                                            cx,
                                                                        );
                                                                        cx.stop_propagation();
                                                                    }),
                                                                )
                                                                .child(issue.display_id.clone())
                                                        },
                                                    )
                                                    .when(!has_issue_url, |this| {
                                                        this.text_xs()
                                                            .font_family(FONT_MONO)
                                                            .font_weight(FontWeight::SEMIBOLD)
                                                            .whitespace_nowrap()
                                                            .text_color(rgb(theme.accent))
                                                            .child(issue.display_id.clone())
                                                    }))
                                                    .when_some(issue_type_label.clone(), |this, issue_type_label| {
                                                        let (badge_text_color, badge_border_color, badge_bg_color) =
                                                            issue_type_badge_colors(
                                                                theme,
                                                                &issue_type_label,
                                                                issue_type_color.as_deref(),
                                                            );
                                                        this.child(
                                                            div()
                                                                .flex_none()
                                                                .rounded_full()
                                                                .border_1()
                                                                .border_color(rgb(badge_border_color))
                                                                .bg(rgb(badge_bg_color))
                                                                .px(px(6.))
                                                                .h(px(16.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .text_size(px(10.))
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .text_color(rgb(badge_text_color))
                                                                .child(issue_type_label),
                                                        )
                                                    })
                                                    .children(issue_labels.into_iter().map(|label| {
                                                        let (badge_text_color, badge_border_color, badge_bg_color) =
                                                            issue_label_badge_colors(
                                                                theme,
                                                                label.color.as_deref(),
                                                            );
                                                        div()
                                                            .flex_none()
                                                            .rounded_full()
                                                            .border_1()
                                                            .border_color(rgb(badge_border_color))
                                                            .bg(rgb(badge_bg_color))
                                                            .px(px(6.))
                                                            .h(px(16.))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .text_size(px(10.))
                                                            .font_weight(FontWeight::MEDIUM)
                                                            .text_color(rgb(badge_text_color))
                                                            .child(label.name)
                                                    }))
                                                    .when_some(linked_review.clone(), |this, linked_review| {
                                                        this.child(
                                                            div()
                                                                .flex_none()
                                                                .rounded_full()
                                                                .border_1()
                                                                .border_color(rgb(theme.border))
                                                                .bg(rgb(theme.panel_bg))
                                                                .px(px(6.))
                                                                .h(px(16.))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .text_size(px(10.))
                                                                .font_family(FONT_MONO)
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(theme.accent))
                                                                .child(linked_review.label),
                                                        )
                                                    }),
                                            )
                                            .when_some(issue_relative_updated_at, |this, updated_at| {
                                                this.child(
                                                    div()
                                                        .flex_none()
                                                        .text_xs()
                                                        .text_color(rgb(theme.text_disabled))
                                                        .child(updated_at),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .w_full()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(theme.text_primary))
                                            .child(issue_title_text),
                                    ),
                            ),
                    ),
            );
        }

        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_1()
            .flex_col()
            .gap_1()
            .child(content)
    }

    pub(crate) fn render_repository_context_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(menu) = self.repository_context_menu.as_ref() else {
            return div();
        };

        let theme = self.theme();
        let index = menu.repository_index;
        let position = menu.position;

        // Full-screen invisible overlay to dismiss on click outside,
        // with the menu as a child — same pattern as render_top_bar_worktree_quick_actions_overlay
        div()
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.repository_context_menu = None;
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                this.repository_context_menu = None;
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                this.repository_context_menu = None;
                cx.notify();
            }))
            // Absolutely-positioned menu at cursor position
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
                            .id("repository-context-remove")
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
                                let label = this
                                    .repositories
                                    .get(index)
                                    .map(|r| r.label.clone())
                                    .unwrap_or_default();
                                this.repository_context_menu = None;
                                this.delete_modal = Some(DeleteModal {
                                    target: DeleteTarget::Repository(index),
                                    label,
                                    branch: String::new(),
                                    has_unpushed: None,
                                    delete_branch: false,
                                    is_deleting: false,
                                    error: None,
                                });
                                cx.notify();
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
                                    .child("Remove"),
                            ),
                    ),
            )
    }

    pub(crate) fn render_worktree_context_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(menu) = self.worktree_context_menu.as_ref() else {
            return div();
        };

        let theme = self.theme();
        let index = menu.worktree_index;
        let position = menu.position;

        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.worktree_context_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.worktree_context_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, _, _, cx| {
                this.worktree_context_menu = None;
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
                            .id("worktree-context-delete")
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
                                this.worktree_context_menu = None;
                                let wt_label = this
                                    .worktrees
                                    .get(index)
                                    .map(|wt| wt.label.clone())
                                    .unwrap_or_default();
                                let wt_branch = this
                                    .worktrees
                                    .get(index)
                                    .map(|wt| wt.branch.clone())
                                    .unwrap_or_default();
                                this.open_delete_modal(
                                    DeleteTarget::Worktree(index),
                                    wt_label,
                                    wt_branch,
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

    pub(crate) fn render_worktree_hover_popover(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(popover) = self.worktree_hover_popover.as_ref() else {
            return div();
        };
        let Some(worktree) = self.worktrees.get(popover.worktree_index) else {
            return div();
        };

        let theme = self.theme();
        let checks_expanded = popover.checks_expanded;
        let popover_zone_bounds =
            worktree_hover_popover_zone_bounds(self.left_pane_width, popover, worktree);
        let attention = worktree_attention_indicator(worktree);
        let activity_sparkline = worktree_activity_sparkline(worktree);

        // Build popover card content
        let popover_wt_index = popover.worktree_index;
        let mut card = div()
            .id("worktree-hover-popover-card")
            .font_family(FONT_MONO)
            .w(px(WORKTREE_HOVER_POPOVER_CARD_WIDTH_PX))
            .bg(rgb(theme.panel_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .rounded_md()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.cancel_worktree_hover_popover_dismiss();
                } else {
                    this.schedule_worktree_hover_popover_dismiss(popover_wt_index, cx);
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            });

        // Header: branch name + relative time (top-right), then directory label
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(theme.text_primary))
                                .child(worktree.branch.clone()),
                        )
                        .when_some(worktree.last_activity_unix_ms, |el, ms| {
                            el.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child(format_relative_time(ms)),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_muted))
                        .child(worktree.label.clone()),
                ),
        );

        // Diff summary
        if let Some(summary) = worktree.diff_summary
            && (summary.additions > 0 || summary.deletions > 0)
        {
            let mut diff_row = div().flex().items_center().gap_1();
            if summary.additions > 0 {
                diff_row = diff_row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x72d69c))
                        .child(format!("+{}", summary.additions)),
                );
            }
            if summary.deletions > 0 {
                diff_row = diff_row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xeb6f92))
                        .child(format!("-{}", summary.deletions)),
                );
            }
            card = card.child(diff_row);
        }

        // Agent section
        let mut agent_row = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex_none()
                    .size(px(6.))
                    .rounded_full()
                    .bg(rgb(attention.color)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_primary))
                    .child(attention.label),
            );

        if !activity_sparkline.is_empty() {
            agent_row = agent_row.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_disabled))
                    .child(activity_sparkline),
            );
        }

        if let Some(ref task) = worktree.agent_task {
            agent_row = agent_row.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(task.clone()),
            );
        }
        card = card.child(agent_row);

        if !worktree.detected_ports.is_empty() {
            let ports = worktree.detected_ports.clone();
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child("Ports"),
                    )
                    .children(ports.into_iter().take(3).map(|port| {
                        let url = worktree_port_url(&port);
                        let detail = worktree_port_detail_text(&port);
                        div()
                            .id(ElementId::Name(
                                format!("hover-port-link-{popover_wt_index}-{}", port.port).into(),
                            ))
                            .cursor_pointer()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .px_1()
                            .max_w(px(WORKTREE_HOVER_POPOVER_CARD_WIDTH_PX - 64.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(rgb(0x72d69c))
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .child(detail)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_external_url(&url, cx);
                                cx.stop_propagation();
                            }))
                    })),
            );
        }

        if !worktree.recent_turns.is_empty() {
            card = card.child(div().h(px(1.)).bg(rgb(theme.border)).my_1());

            let heading = if worktree.stuck_turn_count >= 2 {
                format!(
                    "Recent turns · stuck for {} turns",
                    worktree.stuck_turn_count + 1
                )
            } else {
                "Recent turns".to_owned()
            };
            card = card.child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(if worktree.stuck_turn_count >= 2 {
                        0xeb6f92
                    } else {
                        theme.text_primary
                    }))
                    .child(heading),
            );

            for snapshot in worktree.recent_turns.iter().take(3) {
                let summary_text = match snapshot.diff_summary {
                    Some(summary) if summary.additions > 0 || summary.deletions > 0 => {
                        format!("Changed +{} -{}", summary.additions, summary.deletions)
                    },
                    _ => "No file changes".to_owned(),
                };

                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_muted))
                                .child(summary_text),
                        )
                        .child(
                            div().text_xs().text_color(rgb(theme.text_disabled)).child(
                                snapshot
                                    .timestamp_unix_ms
                                    .map(format_relative_time)
                                    .unwrap_or_else(|| "-".to_owned()),
                            ),
                        ),
                );
            }
        }

        if !worktree.recent_agent_sessions.is_empty() {
            card = card.child(div().h(px(1.)).bg(rgb(theme.border)).my_1());
            card = card.child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme.text_primary))
                    .child("Recent sessions"),
            );

            let mut current_provider = None;
            for session in worktree.recent_agent_sessions.iter().take(4) {
                if current_provider != Some(session.provider) {
                    current_provider = Some(session.provider);
                    card = card.child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_disabled))
                            .child(session.provider.label()),
                    );
                }

                let mut meta = Vec::new();
                if session.message_count > 0 {
                    meta.push(format!("{} msgs", session.message_count));
                }
                if let Some(timestamp) = session.timestamp_unix_ms {
                    meta.push(format_relative_time(timestamp));
                }

                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_xs()
                                .text_color(rgb(theme.text_muted))
                                .child(session.title.clone()),
                        )
                        .when(!meta.is_empty(), |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child(meta.join(" · ")),
                            )
                        }),
                );
            }
        }

        // PR section
        if let Some(ref pr) = worktree.pr_details {
            card = card.child(div().h(px(1.)).bg(rgb(theme.border)).my_1());

            let (state_label, state_color) = pr_state_presentation(&theme, pr.state);

            let pr_url = pr.url.clone();
            let mut pr_header = div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .id("popover-pr-link")
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme.accent))
                        .hover(|this| this.text_color(rgb(theme.text_primary)))
                        .child(format!("#{}", pr.number))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_external_url(&pr_url, cx);
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .px_1()
                        .rounded_sm()
                        .text_color(rgb(state_color))
                        .child(state_label),
                );

            if pr.additions > 0 || pr.deletions > 0 {
                if pr.additions > 0 {
                    pr_header = pr_header.child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x72d69c))
                            .child(format!("+{}", pr.additions)),
                    );
                }
                if pr.deletions > 0 {
                    pr_header = pr_header.child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xeb6f92))
                            .child(format!("-{}", pr.deletions)),
                    );
                }
            }
            card = card.child(pr_header);

            // PR title
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child(pr.title.clone()),
            );

            // Checks + review (only for open/draft PRs)
            if pr.state == github_service::PrState::Open
                || pr.state == github_service::PrState::Draft
            {
                let mut status_row = div().flex().items_center().gap_1();

                if !pr.checks.is_empty() {
                    let (passed, total) = pr_check_counts(pr);
                    let (check_icon, check_color) = check_status_presentation(pr.checks_status);
                    let chevron = if checks_expanded {
                        "\u{f078}"
                    } else {
                        "\u{f054}"
                    };
                    status_row = status_row.child(
                        div()
                            .id("popover-checks-toggle")
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(check_color))
                                    .child(check_icon),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child(format!("{passed}/{total} checks")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child(chevron),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(ref mut p) = this.worktree_hover_popover {
                                    p.checks_expanded = !p.checks_expanded;
                                }
                                cx.notify();
                            })),
                    );
                }

                let (review_icon, review_label, review_color) =
                    review_status_presentation(pr.review_decision);
                status_row = status_row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(review_color))
                                .child(review_icon),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(review_color))
                                .child(review_label),
                        ),
                );

                card = card.child(status_row);

                // Expanded checks list
                if checks_expanded {
                    let mut checks_list = div().flex().flex_col().gap(px(2.)).pl_2();
                    for (name, status) in sorted_pr_checks_for_display(pr) {
                        let (icon, color) = check_status_presentation(*status);
                        checks_list = checks_list.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(div().text_xs().text_color(rgb(color)).child(icon))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .child(name.clone()),
                                ),
                        );
                    }
                    card = card.child(checks_list);
                }
            }
        }

        div().absolute().inset_0().child(
            div()
                .id("worktree-hover-popover-zone")
                .absolute()
                .left(popover_zone_bounds.origin.x)
                .top(popover_zone_bounds.origin.y)
                .p(px(WORKTREE_HOVER_POPOVER_ZONE_PADDING_PX))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                    this.update_worktree_hover_mouse_position(event.position);
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    this.update_worktree_hover_mouse_position(window.mouse_position());
                    if *hovered {
                        this.cancel_worktree_hover_popover_dismiss();
                    } else {
                        this.schedule_worktree_hover_popover_dismiss(popover_wt_index, cx);
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(card),
        )
    }
}

pub(crate) fn split_issue_title_badge(title: &str) -> (Option<String>, String) {
    let trimmed = title.trim();
    let Some(rest) = trimmed.strip_prefix('[') else {
        return (None, trimmed.to_owned());
    };
    let Some((badge, remainder)) = rest.split_once(']') else {
        return (None, trimmed.to_owned());
    };
    let badge = badge.trim();
    if badge.is_empty() {
        return (None, trimmed.to_owned());
    }

    let clean_title = remainder
        .trim_start_matches(':')
        .trim_start_matches('-')
        .trim();
    let clean_title = if clean_title.is_empty() {
        trimmed
    } else {
        clean_title
    };
    (Some(badge.to_owned()), clean_title.to_owned())
}

pub(crate) fn normalize_issue_badge_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '_', '-'], "")
}

pub(crate) fn issue_type_badge_colors(
    theme: ThemePalette,
    label: &str,
    metadata_color: Option<&str>,
) -> (u32, u32, u32) {
    if let Some(color) = metadata_color.and_then(parse_issue_badge_hex_color) {
        return (color, color, theme.panel_bg);
    }

    match normalize_issue_badge_key(label).as_str() {
        "bug" => (0xeb6f92, 0xeb6f92, theme.panel_bg),
        "feature" | "enhancement" => (theme.accent, theme.accent, theme.panel_bg),
        "task" | "chore" => (0xe5c07b, 0xe5c07b, theme.panel_bg),
        "docs" | "documentation" => (0x72d69c, 0x72d69c, theme.panel_bg),
        _ => (theme.text_muted, theme.border, theme.panel_bg),
    }
}

pub(crate) fn issue_label_badge_colors(
    theme: ThemePalette,
    metadata_color: Option<&str>,
) -> (u32, u32, u32) {
    if let Some(color) = metadata_color.and_then(parse_issue_badge_hex_color) {
        return (color, color, theme.panel_bg);
    }

    (theme.text_muted, theme.border, theme.panel_bg)
}

pub(crate) fn parse_issue_badge_hex_color(value: &str) -> Option<u32> {
    let trimmed = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if trimmed.len() != 6 {
        return None;
    }
    u32::from_str_radix(trimmed, 16).ok()
}

pub(crate) fn normalized_sidebar_order(
    saved: Option<&[SidebarItemId]>,
    worktree_ids: Vec<SidebarItemId>,
    outpost_ids: Vec<SidebarItemId>,
) -> Vec<SidebarItemId> {
    let all_current: HashSet<_> = worktree_ids.iter().chain(&outpost_ids).cloned().collect();

    if let Some(saved) = saved {
        let mut ordered: Vec<SidebarItemId> = saved
            .iter()
            .filter(|id| all_current.contains(id))
            .cloned()
            .collect();
        let ordered_set: HashSet<_> = ordered.iter().cloned().collect();
        for id in worktree_ids.into_iter().chain(outpost_ids) {
            if !ordered_set.contains(&id) {
                ordered.push(id);
            }
        }
        ordered
    } else {
        worktree_ids.into_iter().chain(outpost_ids).collect()
    }
}

pub(crate) fn reordered_sidebar_items(
    items: &[SidebarItemId],
    source_id: &SidebarItemId,
    insert_before: usize,
) -> Option<Vec<SidebarItemId>> {
    let mut items = items.to_vec();
    let source_pos = items.iter().position(|id| id == source_id)?;
    let target_pos = if insert_before > source_pos {
        insert_before.saturating_sub(1)
    } else {
        insert_before
    };

    if source_pos == target_pos || source_pos + 1 == insert_before {
        return None;
    }

    let item = items.remove(source_pos);
    let target_pos = target_pos.min(items.len());
    items.insert(target_pos, item);
    Some(items)
}

pub(crate) fn repository_add_worktree_button<I>(theme: &ThemePalette, id: I) -> Stateful<Div>
where
    I: Into<ElementId>,
{
    div()
        .id(id)
        .size(px(20.))
        .rounded_sm()
        .cursor_pointer()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.sidebar_bg))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(theme.text_muted))
        .hover(|this| {
            this.text_color(rgb(theme.text_primary))
                .bg(rgb(theme.panel_active_bg))
        })
        .child("+")
}
