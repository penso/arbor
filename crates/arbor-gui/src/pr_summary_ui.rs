use super::*;

pub(crate) const MAX_VISIBLE_PR_CHECKS: usize = 6;

impl ArborWindow {
    pub(crate) fn render_changes_worktree_summary(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let worktree = self.active_worktree()?;
        let theme = self.theme();
        let attention = worktree_attention_indicator(worktree);
        let repo_slug = self.github_repo_slug.clone();
        let relative_time = worktree.last_activity_unix_ms.map(format_relative_time);
        let diff_summary = worktree.diff_summary;
        let branch_divergence = worktree.branch_divergence;
        let has_ports = !worktree.detected_ports.is_empty();
        let show_pr_loading_indicator = should_show_worktree_pr_loading_indicator(worktree);
        let checks_expanded = self
            .expanded_pr_checks_worktree
            .as_ref()
            .is_some_and(|path| path == &worktree.path);

        let mut card = div()
            .id("changes-worktree-summary")
            .mx_1()
            .mt_1()
            .mb_2()
            .p_2()
            .font_family(FONT_MONO)
            .bg(rgb(theme.panel_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .rounded_md()
            .flex()
            .flex_col()
            .gap_1();

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
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme.text_primary))
                        .child(worktree.branch.clone()),
                )
                .when_some(relative_time, |this, relative_time| {
                    this.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(rgb(theme.text_disabled))
                            .child(relative_time),
                    )
                }),
        );

        let subtitle = repo_slug
            .map(|slug| format!("{} · {}", worktree.label, slug))
            .unwrap_or_else(|| worktree.label.clone());
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(theme.text_muted))
                .child(subtitle),
        );

        let mut meta_row = div().flex().items_center().flex_wrap().gap_1();
        meta_row = meta_row.child(
            div()
                .px_1()
                .py(px(2.))
                .rounded_sm()
                .text_xs()
                .text_color(rgb(attention.color))
                .bg(rgb(theme.sidebar_bg))
                .child(attention.label),
        );

        if let Some(summary) = diff_summary
            && (summary.additions > 0 || summary.deletions > 0)
        {
            let mut diff_chip = div()
                .px_1()
                .py(px(2.))
                .rounded_sm()
                .bg(rgb(theme.sidebar_bg))
                .flex()
                .items_center()
                .gap_1();
            if summary.additions > 0 {
                diff_chip = diff_chip.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x72d69c))
                        .child(format!("+{}", summary.additions)),
                );
            }
            if summary.deletions > 0 {
                diff_chip = diff_chip.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xeb6f92))
                        .child(format!("-{}", summary.deletions)),
                );
            }
            meta_row = meta_row.child(diff_chip);
        }

        if let Some(divergence) = branch_divergence {
            if divergence.ahead > 0 {
                meta_row = meta_row.child(
                    div()
                        .px_1()
                        .py(px(2.))
                        .rounded_sm()
                        .bg(rgb(theme.sidebar_bg))
                        .text_xs()
                        .text_color(rgb(0x72d69c))
                        .child(format!("\u{2191}{}", divergence.ahead)),
                );
            }
            if divergence.behind > 0 {
                meta_row = meta_row.child(
                    div()
                        .px_1()
                        .py(px(2.))
                        .rounded_sm()
                        .bg(rgb(theme.sidebar_bg))
                        .text_xs()
                        .text_color(rgb(0xe5c07b))
                        .child(format!("\u{2193}{}", divergence.behind)),
                );
            }
        }

        if has_ports {
            meta_row = meta_row.child(
                div()
                    .px_1()
                    .py(px(2.))
                    .rounded_sm()
                    .bg(rgb(theme.sidebar_bg))
                    .text_xs()
                    .text_color(rgb(0x72d69c))
                    .child(format!(
                        "{} port{}",
                        worktree.detected_ports.len(),
                        if worktree.detected_ports.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    )),
            );
        }

        if show_pr_loading_indicator {
            meta_row = meta_row.child(pr_loading_chip(&theme, "Refreshing PR"));
        }

        card = card.child(meta_row);

        if let Some(pr) = worktree.pr_details.as_ref() {
            let (state_label, state_color) = pr_state_presentation(&theme, pr.state);
            let (passed_checks, total_checks) = pr_check_counts(pr);
            let review = review_status_presentation(pr.review_decision);
            let merge_status = merge_status_presentation(&theme, pr);
            let visible_checks = if checks_expanded {
                sorted_pr_checks_for_display(pr)
            } else {
                prioritized_pr_checks_for_display(pr)
            };
            let hidden_check_count = pr.checks.len().saturating_sub(visible_checks.len());
            let can_toggle_checks = pr.checks.len() > MAX_VISIBLE_PR_CHECKS || checks_expanded;
            let pr_url = pr.url.clone();

            card = card.child(div().h(px(1.)).bg(rgb(theme.border)).my_1());
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("changes-summary-pr-link")
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
                                    .px_1()
                                    .rounded_sm()
                                    .text_xs()
                                    .text_color(rgb(state_color))
                                    .child(state_label),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0x72d69c))
                                    .child(format!("+{}", pr.additions)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xeb6f92))
                                    .child(format!("-{}", pr.deletions)),
                            ),
                    ),
            );
            card = card.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.text_muted))
                    .child(pr.title.clone()),
            );

            let mut status_row = div().flex().items_center().flex_wrap().gap_2();
            if total_checks > 0 {
                let (icon, color) = check_status_presentation(pr.checks_status);
                let worktree_path = worktree.path.clone();
                let mut checks_summary = div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(div().text_xs().text_color(rgb(color)).child(icon))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(format!("{passed_checks}/{total_checks} checks")),
                    );

                if can_toggle_checks {
                    checks_summary = checks_summary
                        .child(div().text_xs().text_color(rgb(theme.text_disabled)).child(
                            if checks_expanded {
                                "\u{f078}"
                            } else {
                                "\u{f054}"
                            },
                        ))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(theme.sidebar_bg)))
                        .rounded_sm()
                        .px_1()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                this.toggle_changes_pr_checks_for_worktree(
                                    worktree_path.clone(),
                                    cx,
                                );
                            }),
                        );
                }

                status_row = status_row.child(checks_summary);
            }
            status_row = status_row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(div().text_xs().text_color(rgb(review.2)).child(review.0))
                    .child(div().text_xs().text_color(rgb(review.2)).child(review.1)),
            );
            status_row = status_row.child(
                div()
                    .text_xs()
                    .text_color(rgb(merge_status.1))
                    .child(merge_status.0),
            );
            card = card.child(status_row);

            if !visible_checks.is_empty() {
                let mut checks_list = div().flex().flex_col().gap(px(3.)).pl_1();
                for (name, status) in visible_checks.iter() {
                    let (icon, color) = check_status_presentation(*status);
                    checks_list = checks_list.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(div().text_xs().text_color(rgb(color)).child(icon))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child(name.clone()),
                            ),
                    );
                }
                if can_toggle_checks {
                    let worktree_path = worktree.path.clone();
                    let toggle_label = if checks_expanded {
                        "Hide checks".to_owned()
                    } else {
                        format!("+{hidden_check_count} more checks")
                    };
                    checks_list = checks_list.child(
                        div()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.sidebar_bg)))
                            .rounded_sm()
                            .px_1()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(div().text_xs().text_color(rgb(theme.text_disabled)).child(
                                if checks_expanded {
                                    "\u{f078}"
                                } else {
                                    "\u{f054}"
                                },
                            ))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child(toggle_label),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.toggle_changes_pr_checks_for_worktree(
                                        worktree_path.clone(),
                                        cx,
                                    );
                                }),
                            ),
                    );
                }
                card = card.child(checks_list);
            }
        } else {
            card = card.child(div().h(px(1.)).bg(rgb(theme.border)).my_1());

            if show_pr_loading_indicator {
                card = card.child(pr_loading_row(&theme, "Refreshing PR details"));
            }

            if let Some(pr_url) = worktree.pr_url.clone() {
                let pr_number = worktree.pr_number;
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("changes-summary-pr-fallback-link")
                                .cursor_pointer()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(theme.accent))
                                .hover(|this| this.text_color(rgb(theme.text_primary)))
                                .child(match pr_number {
                                    Some(number) => format!("#{}", number),
                                    None => "Pull request".to_owned(),
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_external_url(&pr_url, cx);
                                })),
                        )
                        .child(div().text_xs().text_color(rgb(theme.text_disabled)).child(
                            if show_pr_loading_indicator {
                                "Fetching GitHub details"
                            } else {
                                "GitHub details unavailable"
                            },
                        )),
                );
            } else if !show_pr_loading_indicator {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_disabled))
                        .child("No pull request for this branch"),
                );
            }
        }

        Some(card.into_any_element())
    }

    pub(crate) fn toggle_changes_pr_checks_for_worktree(
        &mut self,
        worktree_path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.expanded_pr_checks_worktree.as_ref() == Some(&worktree_path) {
            self.expanded_pr_checks_worktree = None;
        } else {
            self.expanded_pr_checks_worktree = Some(worktree_path);
        }
        cx.notify();
    }
}

pub(crate) fn pr_state_presentation(
    theme: &ThemePalette,
    state: github_service::PrState,
) -> (&'static str, u32) {
    match state {
        github_service::PrState::Open => ("Open", 0x72d69c_u32),
        github_service::PrState::Draft => ("Draft", theme.text_disabled),
        github_service::PrState::Merged => ("Merged", 0xbb9af7_u32),
        github_service::PrState::Closed => ("Closed", 0xeb6f92_u32),
    }
}

pub(crate) fn review_status_presentation(
    review_decision: github_service::ReviewDecision,
) -> (&'static str, &'static str, u32) {
    match review_decision {
        github_service::ReviewDecision::Approved => ("\u{f00c}", "Approved", 0x72d69c_u32),
        github_service::ReviewDecision::ChangesRequested => {
            ("\u{f00d}", "Changes requested", 0xeb6f92_u32)
        },
        github_service::ReviewDecision::Pending => ("\u{f192}", "Needs review", 0xe5c07b_u32),
    }
}

pub(crate) fn merge_status_presentation(
    theme: &ThemePalette,
    pr: &github_service::PrDetails,
) -> (&'static str, u32) {
    use crate::github_service::{MergeStateStatus, MergeableState, PrState};

    match pr.state {
        PrState::Merged => ("Merged", 0xbb9af7_u32),
        PrState::Closed => ("Closed", 0xeb6f92_u32),
        PrState::Draft => ("Draft", theme.text_disabled),
        PrState::Open => {
            if pr.mergeable == MergeableState::Conflicting
                || pr.merge_state_status == MergeStateStatus::Dirty
            {
                return ("Merge conflicts", 0xeb6f92_u32);
            }

            match pr.merge_state_status {
                MergeStateStatus::Behind => ("Update branch", 0xe5c07b_u32),
                MergeStateStatus::Blocked => ("Merge blocked", 0xe5c07b_u32),
                MergeStateStatus::Clean | MergeStateStatus::HasHooks => {
                    ("Ready to merge", 0x72d69c_u32)
                },
                MergeStateStatus::Dirty => ("Merge conflicts", 0xeb6f92_u32),
                MergeStateStatus::Draft => ("Draft", theme.text_disabled),
                MergeStateStatus::Unknown => match pr.mergeable {
                    MergeableState::Conflicting => ("Merge conflicts", 0xeb6f92_u32),
                    MergeableState::Mergeable => ("Ready to merge", 0x72d69c_u32),
                    MergeableState::Unknown => ("Checking mergeability", theme.text_disabled),
                },
                MergeStateStatus::Unstable => ("Checks failing", 0xeb6f92_u32),
            }
        },
    }
}

pub(crate) fn check_status_presentation(
    status: github_service::CheckStatus,
) -> (&'static str, u32) {
    match status {
        github_service::CheckStatus::Success => ("\u{f00c}", 0x72d69c_u32),
        github_service::CheckStatus::Failure => ("\u{f00d}", 0xeb6f92_u32),
        github_service::CheckStatus::Pending => ("\u{f192}", 0xe5c07b_u32),
    }
}

pub(crate) fn pr_check_counts(pr: &github_service::PrDetails) -> (usize, usize) {
    (pr.passed_checks, pr.checks.len())
}

pub(crate) fn prioritized_pr_checks_for_display(
    pr: &github_service::PrDetails,
) -> &[(String, github_service::CheckStatus)] {
    let visible_check_count = pr.checks.len().min(MAX_VISIBLE_PR_CHECKS);
    &pr.checks[..visible_check_count]
}

pub(crate) fn sorted_pr_checks_for_display(
    pr: &github_service::PrDetails,
) -> &[(String, github_service::CheckStatus)] {
    pr.checks.as_slice()
}

pub(crate) fn pr_loading_chip(theme: &ThemePalette, label: &'static str) -> AnyElement {
    div()
        .px_1()
        .py(px(2.))
        .rounded_sm()
        .bg(rgb(theme.sidebar_bg))
        .flex()
        .items_center()
        .gap(px(4.))
        .child(pr_loading_icon(theme, "changes-summary-pr-loading-chip"))
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme.text_disabled))
                .child(label),
        )
        .into_any_element()
}

pub(crate) fn pr_loading_row(theme: &ThemePalette, label: &'static str) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(pr_loading_icon(theme, "changes-summary-pr-loading-row"))
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme.text_disabled))
                .child(label),
        )
        .into_any_element()
}

pub(crate) fn pr_loading_icon(theme: &ThemePalette, animation_key: &'static str) -> AnyElement {
    div()
        .font_family(FONT_MONO)
        .text_xs()
        .text_color(rgb(theme.text_disabled))
        .child("\u{f110}")
        .with_animation(
            animation_key,
            Animation::new(Duration::from_millis(900))
                .repeat()
                .with_easing(ease_in_out),
            |this, delta| this.opacity(0.3 + (0.7 * delta)),
        )
        .into_any_element()
}
