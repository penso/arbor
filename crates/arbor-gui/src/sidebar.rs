impl ArborWindow {
    fn render_left_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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

                            div()
                                .id(("repository-group", repository_index))
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .id(("repository-row", repository_index))
                                        .cursor_pointer()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .h(px(32.))
                                        .hover(|this| this.bg(rgb(theme.panel_active_bg)))
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
                                                                        if this
                                                                            .collapsed_repositories
                                                                            .contains(&repository_index)
                                                                        {
                                                                            this.collapsed_repositories
                                                                                .remove(&repository_index);
                                                                        } else {
                                                                            this.collapsed_repositories
                                                                                .insert(repository_index);
                                                                        }
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
                                                )
                                                // Worktree count badge
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(theme.text_disabled))
                                                        .child(format!(
                                                            "{}",
                                                            repo_worktrees.len()
                                                        )),
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
                                            div()
                                                .id(("repository-add-worktree", repository_index))
                                                .size(px(20.))
                                                .rounded_sm()
                                                .cursor_pointer()
                                                .flex_none()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(theme.text_muted))
                                                .hover(|this| this.text_color(rgb(theme.text_primary)))
                                                .child("+")
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
                                    this.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.))
                                        .children(
                                            repo_worktrees.into_iter().map(|(index, worktree)| {
                                                let is_active =
                                                    self.active_worktree_index == Some(index);
                                                let diff_summary = worktree.diff_summary;
                                                let pr_number = worktree.pr_number;
                                                let pr_url = worktree.pr_url.clone();
                                                let is_merged_pr = worktree
                                                    .pr_details
                                                    .as_ref()
                                                    .is_some_and(|pr| {
                                                        pr.state == github_service::PrState::Merged
                                                    });
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
                                                let row = div()
                                                    .id(("worktree-row", index))
                                                    .font_family(FONT_MONO)
                                                    .cursor_pointer()
                                                    .rounded_sm()
                                                    .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                                    .flex()
                                                    .items_center()
                                                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                                                        this.update_worktree_hover_mouse_position(event.position);
                                                    }))
                                                    .on_click(
                                                        cx.listener(move |this, _, window, cx| {
                                                            this.select_worktree(index, window, cx)
                                                        }),
                                                    )
                                                    .when(
                                                        !is_primary
                                                            || worktree.checkout_kind
                                                                == CheckoutKind::DiscreteClone,
                                                        |this| {
                                                        this.on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            this.worktree_context_menu = Some(WorktreeContextMenu {
                                                                worktree_index: index,
                                                                position: event.position,
                                                            });
                                                            this.worktree_hover_popover = None;
                                                            this._hover_show_task = None;
                                                            cx.notify();
                                                        }))
                                                    },
                                                    )
                                                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                                        this.update_worktree_hover_mouse_position(window.mouse_position());
                                                        if *hovered {
                                                            let mouse_position = window.mouse_position();
                                                            this.schedule_worktree_hover_popover_show(index, mouse_position.y, cx);
                                                        } else if this.worktree_hover_popover.as_ref().is_some_and(|p| p.worktree_index == index) {
                                                            this.schedule_worktree_hover_popover_dismiss(index, cx);
                                                        } else {
                                                            this.cancel_worktree_hover_popover_show();
                                                        }
                                                    }))
                                                    // Bordered cell
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
                                                        .py(px(if compact_sidebar { 4. } else { 6. }))
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .gap(px(4.))
                                                        .hover(|this| {
                                                            this.bg(rgb(theme.panel_active_bg))
                                                        })
                                                        .when(is_active, |this| {
                                                            this.bg(rgb(theme.panel_active_bg))
                                                                .border_color(rgb(theme.accent))
                                                        })
                                                        .when(is_merged_pr && !is_active, |this| {
                                                            this.opacity(0.72)
                                                        })
                                                    // Git branch icon — vertically centered
                                                    .child(
                                                        div()
                                                            .flex_none()
                                                            .w(px(18.))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .text_size(px(16.))
                                                            .text_color(rgb(theme.text_muted))
                                                            .child(worktree.checkout_kind.icon()),
                                                    )
                                                    // Two-line text column
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .min_w_0()
                                                            .flex()
                                                            .flex_col()
                                                            .gap(px(1.))
                                                    // Line 1: [spinner] [name] ... [+- lines] [time ago]
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(2.))
                                                            // Activity spinner dot
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
                                                            // Name/label
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
                                                                        format!(
                                                                            "{} · {}",
                                                                            worktree.label,
                                                                            worktree.branch
                                                                        )
                                                                    } else {
                                                                        worktree.branch.clone()
                                                                    }),
                                                            )
                                                            // Right side: [+- lines] [time ago]
                                                            .child({
                                                                let summary =
                                                                    diff_summary.unwrap_or_default();
                                                                let show_diff_summary =
                                                                    summary.additions > 0
                                                                        || summary.deletions > 0;
                                                                let mut right = div()
                                                                    .flex_none()
                                                                    .flex()
                                                                    .items_center()
                                                                    .gap_1();

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
                                                                            .text_color(rgb(
                                                                                theme.text_muted,
                                                                            ))
                                                                            .child("..."),
                                                                    );
                                                                } else if show_diff_summary
                                                                    && !compact_sidebar
                                                                {
                                                                    if summary.additions > 0 {
                                                                        right = right.child(
                                                                            div()
                                                                                .text_xs()
                                                                                .text_color(rgb(
                                                                                    0x72d69c,
                                                                                ))
                                                                                .child(format!(
                                                                                    "+{}",
                                                                                    summary
                                                                                        .additions
                                                                                )),
                                                                        );
                                                                    }
                                                                    if summary.deletions > 0 {
                                                                        right = right.child(
                                                                            div()
                                                                                .text_xs()
                                                                                .text_color(rgb(
                                                                                    0xeb6f92,
                                                                                ))
                                                                                .child(format!(
                                                                                    "-{}",
                                                                                    summary
                                                                                        .deletions
                                                                                )),
                                                                        );
                                                                    }
                                                                }

                                                                if let Some(activity_ms) = worktree.last_activity_unix_ms {
                                                                    right = right.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(rgb(
                                                                                theme.text_disabled,
                                                                            ))
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
                                                    // Line 2: [agent task or dir name] ... [PR number]
                                                    .when(!compact_sidebar, |this| this.child(
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
                                                                                attention.label,
                                                                                task_or_label
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
                                                                let (checks_icon, checks_color) = match pr.checks_status {
                                                                    github_service::CheckStatus::Success => ("\u{f00c}", 0x72d69c_u32),
                                                                    github_service::CheckStatus::Failure => ("\u{f00d}", 0xeb6f92_u32),
                                                                    github_service::CheckStatus::Pending => ("\u{f192}", 0xe5c07b_u32),
                                                                };
                                                                let (review_icon, review_color) = match pr.review_decision {
                                                                    github_service::ReviewDecision::Approved => ("\u{f00c}", 0x72d69c_u32),
                                                                    github_service::ReviewDecision::ChangesRequested => ("\u{f071}", 0xeb6f92_u32),
                                                                    github_service::ReviewDecision::Pending => ("\u{f128}", theme.text_disabled),
                                                                };

                                                                let mut badges = this.child(
                                                                    div()
                                                                        .flex_none()
                                                                        .text_xs()
                                                                        .text_color(rgb(checks_color))
                                                                        .child(checks_icon),
                                                                ).child(
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
                                                                            .id(ElementId::Name(
                                                                                port_id.into(),
                                                                            ))
                                                                            .cursor_pointer()
                                                                            .flex_none()
                                                                            .text_xs()
                                                                            .text_color(rgb(0x72d69c))
                                                                            .hover(|this| this.text_color(rgb(theme.text_primary)))
                                                                            .child(worktree_port_badge_text(port))
                                                                            .on_click(cx.listener(
                                                                                move |this, _, _, cx| {
                                                                                    this.open_external_url(
                                                                                        &port_url,
                                                                                        cx,
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
                                                                            .hover(|this| this.text_color(rgb(theme.text_primary)))
                                                                            .child(pr_text)
                                                                            .on_click(cx.listener(
                                                                                move |this, _, _, cx| {
                                                                                    this.open_external_url(
                                                                                        &pr_url,
                                                                                        cx,
                                                                                    );
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
                                                            .when(pr_details.is_none() && !detected_ports.is_empty(), |this| {
                                                                detected_ports.iter().take(2).fold(this, |this, port| {
                                                                    let port_url = worktree_port_url(port);
                                                                    let port_id = format!(
                                                                        "worktree-port-link-{index}-{}",
                                                                        port.port
                                                                    );
                                                                    this.child(
                                                                        div()
                                                                            .id(ElementId::Name(
                                                                                port_id.into(),
                                                                            ))
                                                                            .cursor_pointer()
                                                                            .flex_none()
                                                                            .text_xs()
                                                                            .text_color(rgb(0x72d69c))
                                                                            .hover(|this| this.text_color(rgb(theme.text_primary)))
                                                                            .child(worktree_port_badge_text(port))
                                                                            .on_click(cx.listener(
                                                                                move |this, _, _, cx| {
                                                                                    this.open_external_url(
                                                                                        &port_url,
                                                                                        cx,
                                                                                    );
                                                                                    cx.stop_propagation();
                                                                                },
                                                                            )),
                                                                    )
                                                                })
                                                            }),
                                                    ))
                                                    ) // text column
                                                    ); // bordered cell
                                                if is_active {
                                                    row.with_animation(
                                                        ("worktree-select", selection_epoch),
                                                        Animation::new(Duration::from_millis(150))
                                                            .with_easing(ease_in_out),
                                                        |el, delta| {
                                                            el.opacity(0.8 + 0.2 * delta)
                                                        },
                                                    )
                                                    .into_any_element()
                                                } else {
                                                    row.opacity(0.8).into_any_element()
                                                }
                                            }),
                                        ),
                                )
                                })
                                .when(!repo_outposts.is_empty(), |group| {
                                    group.child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .children(
                                                repo_outposts.into_iter().map(|(outpost_index, outpost)| {
                                                    let is_active = self.active_outpost_index == Some(outpost_index);
                                                    let status_color = match outpost.status {
                                                        arbor_core::outpost::OutpostStatus::Available => theme.accent,
                                                        arbor_core::outpost::OutpostStatus::Unreachable => 0xeb6f92,
                                                        arbor_core::outpost::OutpostStatus::NotCloned | arbor_core::outpost::OutpostStatus::Provisioning => theme.text_muted,
                                                    };
                                                    div()
                                                        .id(("outpost-row", outpost_index))
                                                        .font_family(FONT_MONO)
                                                        .cursor_pointer()
                                                        .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                                        .flex()
                                                        .items_center()
                                                        .on_click(cx.listener(move |this, _, window, cx| {
                                                            this.select_outpost(outpost_index, window, cx);
                                                        }))
                                                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                                            cx.stop_propagation();
                                                            this.outpost_context_menu = Some(OutpostContextMenu {
                                                                outpost_index,
                                                                position: event.position,
                                                            });
                                                            cx.notify();
                                                        }))
                                                        // Bordered cell
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
                                                            .when(is_active, |this| this.bg(rgb(theme.panel_active_bg)))
                                                        // Globe icon — vertically centered
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
                                                        // Two-line text column
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .min_w_0()
                                                                .flex()
                                                                .flex_col()
                                                                .gap(px(1.))
                                                        // Line 1: branch@host
                                                        .child(
                                                            div()
                                                                .flex()
                                                                .items_center()
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
                                                                        .child(format!("{}@{}", outpost.branch, outpost.hostname)),
                                                                ),
                                                        )
                                                        // Line 2: outpost label
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
                                                                        .child(outpost.label.clone()),
                                                                ),
                                                        )
                                                        )
                                                        )
                                                }),
                                            ),
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
                                                            div()
                                                                .id(("remote-repo-add-wt", plus_id))
                                                                .size(px(20.))
                                                                .rounded_sm()
                                                                .cursor_pointer()
                                                                .flex_none()
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .text_sm()
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(rgb(theme.text_muted))
                                                                .hover(|this| this.text_color(rgb(theme.text_primary)))
                                                                .child("+")
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

    fn render_repository_context_menu(&mut self, cx: &mut Context<Self>) -> Div {
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

    fn render_worktree_context_menu(&mut self, cx: &mut Context<Self>) -> Div {
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

    fn render_worktree_hover_popover(&mut self, cx: &mut Context<Self>) -> Div {
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
                format!("Recent turns · stuck for {} turns", worktree.stuck_turn_count + 1)
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
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_disabled))
                                .child(
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

            let (state_label, state_color) = match pr.state {
                github_service::PrState::Open => ("Open", 0x72d69c_u32),
                github_service::PrState::Draft => ("Draft", theme.text_disabled),
                github_service::PrState::Merged => ("Merged", 0xbb9af7_u32),
                github_service::PrState::Closed => ("Closed", 0xeb6f92_u32),
            };

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
                    let passed = pr
                        .checks
                        .iter()
                        .filter(|(_, s)| *s == github_service::CheckStatus::Success)
                        .count();
                    let total = pr.checks.len();
                    let (check_icon, check_color) = match pr.checks_status {
                        github_service::CheckStatus::Success => ("\u{f00c}", 0x72d69c_u32),
                        github_service::CheckStatus::Failure => ("\u{f00d}", 0xeb6f92_u32),
                        github_service::CheckStatus::Pending => ("\u{f192}", 0xe5c07b_u32),
                    };
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

                let (review_label, review_color) = match pr.review_decision {
                    github_service::ReviewDecision::Approved => ("Approved", 0x72d69c_u32),
                    github_service::ReviewDecision::ChangesRequested => {
                        ("Changes requested", 0xeb6f92_u32)
                    },
                    github_service::ReviewDecision::Pending => {
                        ("Review pending", theme.text_disabled)
                    },
                };
                status_row = status_row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(review_color))
                        .child(review_label),
                );

                card = card.child(status_row);

                // Expanded checks list
                if checks_expanded {
                    let mut checks_list = div().flex().flex_col().gap(px(2.)).pl_2();
                    let mut sorted_checks: Vec<_> = pr.checks.iter().collect();
                    sorted_checks.sort_by_key(|(_, status)| match status {
                        github_service::CheckStatus::Failure => 0,
                        github_service::CheckStatus::Pending => 1,
                        github_service::CheckStatus::Success => 2,
                    });
                    for (name, status) in sorted_checks {
                        let (icon, color) = match status {
                            github_service::CheckStatus::Success => ("\u{f00c}", 0x72d69c_u32),
                            github_service::CheckStatus::Failure => ("\u{f00d}", 0xeb6f92_u32),
                            github_service::CheckStatus::Pending => ("\u{f192}", 0xe5c07b_u32),
                        };
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
