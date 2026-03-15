use super::*;

pub(crate) fn ui_state_save_has_work(
    pending_ui_state_save: Option<&ui_state_store::UiState>,
    ui_state_save_in_flight: Option<&ui_state_store::UiState>,
) -> bool {
    pending_ui_state_save.is_some() || ui_state_save_in_flight.is_some()
}

pub(crate) fn next_pending_ui_state_save(
    last_persisted_ui_state: &ui_state_store::UiState,
    pending_ui_state_save: Option<&ui_state_store::UiState>,
    ui_state_save_in_flight: Option<&ui_state_store::UiState>,
    next_state: &ui_state_store::UiState,
) -> Option<ui_state_store::UiState> {
    if pending_ui_state_save == Some(next_state) || ui_state_save_in_flight == Some(next_state) {
        return pending_ui_state_save.cloned();
    }

    if last_persisted_ui_state == next_state && ui_state_save_in_flight.is_none() {
        return None;
    }

    Some(next_state.clone())
}

pub(crate) fn issue_cache_save_has_work(
    pending_issue_cache_save: Option<&issue_cache_store::IssueCache>,
    issue_cache_save_in_flight: Option<&issue_cache_store::IssueCache>,
) -> bool {
    pending_issue_cache_save.is_some() || issue_cache_save_in_flight.is_some()
}

pub(crate) fn next_pending_issue_cache_save(
    last_persisted_issue_cache: &issue_cache_store::IssueCache,
    pending_issue_cache_save: Option<&issue_cache_store::IssueCache>,
    issue_cache_save_in_flight: Option<&issue_cache_store::IssueCache>,
    next_cache: &issue_cache_store::IssueCache,
) -> Option<issue_cache_store::IssueCache> {
    if pending_issue_cache_save == Some(next_cache)
        || issue_cache_save_in_flight == Some(next_cache)
    {
        return pending_issue_cache_save.cloned();
    }

    if last_persisted_issue_cache == next_cache && issue_cache_save_in_flight.is_none() {
        return None;
    }

    Some(next_cache.clone())
}

pub(crate) fn persisted_sidebar_selection_for_ui_state(
    current_selection: Option<ui_state_store::PersistedSidebarSelection>,
    queued_selection: Option<ui_state_store::PersistedSidebarSelection>,
    pending_startup_worktree_restore: bool,
) -> Option<ui_state_store::PersistedSidebarSelection> {
    if !pending_startup_worktree_restore {
        return current_selection;
    }

    match (current_selection, queued_selection) {
        (
            Some(ui_state_store::PersistedSidebarSelection::Repository { root }),
            Some(
                ref saved_selection @ ui_state_store::PersistedSidebarSelection::Worktree {
                    ref repo_root,
                    ..
                },
            ),
        ) if root == *repo_root => Some(saved_selection.clone()),
        (current_selection, _) => current_selection,
    }
}

impl ArborWindow {
    pub(crate) fn clamp_pane_widths_for_workspace(&mut self, workspace_width: f32) {
        let available_side_width =
            (workspace_width - (2. * PANE_RESIZE_HANDLE_WIDTH) - PANE_CENTER_MIN_WIDTH).max(0.);

        self.left_pane_width = self
            .left_pane_width
            .clamp(LEFT_PANE_MIN_WIDTH, LEFT_PANE_MAX_WIDTH);
        self.right_pane_width = self
            .right_pane_width
            .clamp(RIGHT_PANE_MIN_WIDTH, RIGHT_PANE_MAX_WIDTH);

        let side_total = self.left_pane_width + self.right_pane_width;
        if side_total <= available_side_width {
            return;
        }

        let mut overflow = side_total - available_side_width;

        let right_reducible = (self.right_pane_width - RIGHT_PANE_MIN_WIDTH).max(0.);
        let right_reduction = overflow.min(right_reducible);
        self.right_pane_width -= right_reduction;
        overflow -= right_reduction;

        if overflow <= 0. {
            return;
        }

        let left_reducible = (self.left_pane_width - LEFT_PANE_MIN_WIDTH).max(0.);
        let left_reduction = overflow.min(left_reducible);
        self.left_pane_width -= left_reduction;
    }

    pub(crate) fn estimated_diff_wrap_columns(&self, cell_width_px: f32) -> usize {
        let fallback_window_width = self.left_pane_width
            + self.right_pane_width
            + PANE_CENTER_MIN_WIDTH
            + (2. * PANE_RESIZE_HANDLE_WIDTH);
        let window_width = self
            .last_persisted_ui_state
            .window
            .map(|window| window.width as f32)
            .unwrap_or(fallback_window_width)
            .max(600.);
        self.estimated_diff_wrap_columns_for_window_width(window_width, cell_width_px)
    }

    pub(crate) fn estimated_diff_wrap_columns_for_window_width(
        &self,
        window_width: f32,
        cell_width_px: f32,
    ) -> usize {
        let center_width = (window_width
            - self.left_pane_width
            - self.right_pane_width
            - (2. * PANE_RESIZE_HANDLE_WIDTH))
            .max(PANE_CENTER_MIN_WIDTH);
        let list_width =
            (center_width - DIFF_ZONEMAP_WIDTH_PX - (DIFF_ZONEMAP_MARGIN_PX * 2.)).max(80.);
        self.estimated_diff_wrap_columns_for_list_width(list_width, cell_width_px)
    }

    pub(crate) fn estimated_diff_wrap_columns_for_list_width(
        &self,
        list_width: f32,
        cell_width_px: f32,
    ) -> usize {
        let column_width = (list_width / 2.).max(40.);
        let safe_cell_width = cell_width_px.max(1.);
        let line_number_width = (DIFF_LINE_NUMBER_WIDTH_CHARS as f32 * safe_cell_width) + 12.;
        let marker_width = 10.;
        let horizontal_padding = 16.;
        let horizontal_gaps = 16.;
        let text_width = (column_width
            - line_number_width
            - marker_width
            - horizontal_padding
            - horizontal_gaps)
            .max(safe_cell_width);
        let estimated_columns = (text_width / safe_cell_width).floor() as usize;
        estimated_columns.saturating_add(2).clamp(12, 320)
    }

    pub(crate) fn live_diff_list_width_px(&self) -> Option<f32> {
        let width = self
            .diff_scroll_handle
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .width
            .to_f64() as f32;
        (width.is_finite() && width >= 80.).then_some(width)
    }

    pub(crate) fn rewrap_diff_sessions_if_needed(&mut self, wrap_columns: usize) {
        for session in &mut self.diff_sessions {
            if session.is_loading
                || session.raw_lines.is_empty()
                || session.wrapped_columns == wrap_columns
            {
                continue;
            }

            let (wrapped_lines, wrapped_indices) = wrap_diff_document_lines(
                session.raw_lines.as_ref(),
                &session.raw_file_row_indices,
                wrap_columns,
            );
            session.lines = Arc::<[DiffLine]>::from(wrapped_lines);
            session.file_row_indices = wrapped_indices;
            session.wrapped_columns = wrap_columns;
        }
    }

    pub(crate) fn ui_state_snapshot(&self, window: &Window) -> ui_state_store::UiState {
        let bounds = window.window_bounds().get_bounds();
        let x = f32::from(bounds.origin.x).round() as i32;
        let y = f32::from(bounds.origin.y).round() as i32;
        let width = f32::from(bounds.size.width).round().max(1.) as u32;
        let height = f32::from(bounds.size.height).round().max(1.) as u32;

        ui_state_store::UiState {
            left_pane_width: Some(self.left_pane_width.round() as i32),
            right_pane_width: Some(self.right_pane_width.round() as i32),
            window: Some(ui_state_store::WindowGeometry {
                x,
                y,
                width,
                height,
            }),
            left_pane_visible: Some(self.left_pane_visible),
            compact_sidebar: Some(self.compact_sidebar),
            execution_mode: Some(self.execution_mode),
            preferred_checkout_kind: Some(self.preferred_checkout_kind),
            collapsed_repository_group_keys: self.collapsed_repository_group_keys_snapshot(),
            sidebar_order: self.sidebar_order.clone(),
            repository_sidebar_tabs: self.repository_sidebar_tabs_snapshot(),
            selected_sidebar_selection: self.sidebar_selection_snapshot_for_persistence(),
            right_pane_tab: Some(persisted_right_pane_tab(self.right_pane_tab)),
            logs_tab_open: Some(self.logs_tab_open),
            logs_tab_active: Some(self.logs_tab_active),
            pull_request_cache: self.pull_request_cache_snapshot(),
        }
    }

    pub(crate) fn queued_ui_state_base(&self) -> ui_state_store::UiState {
        self.pending_ui_state_save
            .clone()
            .or_else(|| self.ui_state_save_in_flight.clone())
            .unwrap_or_else(|| self.last_persisted_ui_state.clone())
    }

    pub(crate) fn repository_sidebar_tabs_snapshot(&self) -> HashMap<String, RepositorySidebarTab> {
        self.repository_sidebar_tabs
            .iter()
            .filter(|(_, tab)| **tab != RepositorySidebarTab::Worktrees)
            .map(|(group_key, tab)| (group_key.clone(), *tab))
            .collect()
    }

    pub(crate) fn collapsed_repository_group_keys_snapshot(&self) -> Vec<String> {
        let mut group_keys: Vec<String> = self
            .collapsed_repositories
            .iter()
            .filter_map(|index| self.repositories.get(*index))
            .map(|repository| repository.group_key.clone())
            .collect();
        group_keys.sort();
        group_keys.dedup();
        group_keys
    }

    pub(crate) fn sidebar_selection_snapshot(
        &self,
    ) -> Option<ui_state_store::PersistedSidebarSelection> {
        if let Some(outpost_index) = self.active_outpost_index {
            return self.outposts.get(outpost_index).map(|outpost| {
                ui_state_store::PersistedSidebarSelection::Outpost {
                    repo_root: outpost.repo_root.display().to_string(),
                    outpost_id: outpost.outpost_id.clone(),
                }
            });
        }

        if let Some(worktree) = self.active_worktree() {
            return Some(ui_state_store::PersistedSidebarSelection::Worktree {
                repo_root: worktree.repo_root.display().to_string(),
                path: worktree.path.display().to_string(),
            });
        }

        self.selected_repository().map(|repository| {
            ui_state_store::PersistedSidebarSelection::Repository {
                root: repository.root.display().to_string(),
            }
        })
    }

    pub(crate) fn sidebar_selection_snapshot_for_persistence(
        &self,
    ) -> Option<ui_state_store::PersistedSidebarSelection> {
        persisted_sidebar_selection_for_ui_state(
            self.sidebar_selection_snapshot(),
            self.queued_ui_state_base().selected_sidebar_selection,
            self.pending_startup_worktree_restore,
        )
    }

    pub(crate) fn pull_request_cache_snapshot(
        &self,
    ) -> HashMap<String, ui_state_store::CachedPullRequestState> {
        self.worktrees
            .iter()
            .filter_map(|worktree| {
                worktree
                    .cached_pull_request_state()
                    .map(|cached| (worktree_pull_request_cache_key(&worktree.path), cached))
            })
            .collect()
    }

    pub(crate) fn queue_ui_state_save(
        &mut self,
        next_state: ui_state_store::UiState,
        cx: &mut Context<Self>,
    ) {
        let queued_ui_state_save = next_pending_ui_state_save(
            &self.last_persisted_ui_state,
            self.pending_ui_state_save.as_ref(),
            self.ui_state_save_in_flight.as_ref(),
            &next_state,
        );
        let should_start_save =
            queued_ui_state_save.is_some() && self.ui_state_save_in_flight.is_none();
        self.pending_ui_state_save = queued_ui_state_save;
        if !should_start_save {
            return;
        }

        self.start_pending_ui_state_save(cx);
    }

    pub(crate) fn sync_ui_state_store(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.queue_ui_state_save(self.ui_state_snapshot(window), cx);
    }

    pub(crate) fn start_pending_ui_state_save(&mut self, cx: &mut Context<Self>) {
        if self.ui_state_save_in_flight.is_some() {
            return;
        }

        let Some(next_state) = self.pending_ui_state_save.take() else {
            self.maybe_finish_quit_after_persistence_flush(cx);
            return;
        };

        self.ui_state_save_in_flight = Some(next_state.clone());
        let store = self.ui_state_store.clone();
        let state_to_save = next_state.clone();
        self._ui_state_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.save(&state_to_save) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.ui_state_save_in_flight = None;
                match result {
                    Ok(()) => {
                        this.last_persisted_ui_state = next_state.clone();
                        this.last_ui_state_error = None;
                    },
                    Err(error) => {
                        let message = error.to_string();
                        if this.last_ui_state_error.as_deref() != Some(message.as_str()) {
                            this.notice = Some(format!("failed to persist UI state: {error}"));
                            this.last_ui_state_error = Some(message);
                            cx.notify();
                        }
                    },
                }

                this.start_pending_ui_state_save(cx);
                this.maybe_finish_quit_after_persistence_flush(cx);
            });
        }));
    }

    pub(crate) fn sync_pull_request_cache_store(&mut self, cx: &mut Context<Self>) {
        let mut next_state = self.queued_ui_state_base();
        next_state.pull_request_cache = self.pull_request_cache_snapshot();
        self.queue_ui_state_save(next_state, cx);
    }

    pub(crate) fn sync_repository_sidebar_tabs_store(&mut self, cx: &mut Context<Self>) {
        let mut next_state = self.queued_ui_state_base();
        next_state.repository_sidebar_tabs = self.repository_sidebar_tabs_snapshot();
        self.queue_ui_state_save(next_state, cx);
    }

    pub(crate) fn sync_sidebar_order_store(&mut self, cx: &mut Context<Self>) {
        let mut next_state = self.queued_ui_state_base();
        next_state.sidebar_order = self.sidebar_order.clone();
        self.queue_ui_state_save(next_state, cx);
    }

    pub(crate) fn sync_collapsed_repositories_store(&mut self, cx: &mut Context<Self>) {
        let mut next_state = self.queued_ui_state_base();
        next_state.collapsed_repository_group_keys =
            self.collapsed_repository_group_keys_snapshot();
        self.queue_ui_state_save(next_state, cx);
    }

    pub(crate) fn sync_navigation_ui_state_store(&mut self, cx: &mut Context<Self>) {
        let mut next_state = self.queued_ui_state_base();
        next_state.selected_sidebar_selection = self.sidebar_selection_snapshot_for_persistence();
        next_state.right_pane_tab = Some(persisted_right_pane_tab(self.right_pane_tab));
        next_state.logs_tab_open = Some(self.logs_tab_open);
        next_state.logs_tab_active = Some(self.logs_tab_active);
        self.queue_ui_state_save(next_state, cx);
    }

    pub(crate) fn queued_issue_cache_base(&self) -> issue_cache_store::IssueCache {
        self.pending_issue_cache_save
            .clone()
            .or_else(|| self.issue_cache_save_in_flight.clone())
            .unwrap_or_else(|| self.last_persisted_issue_cache.clone())
    }

    pub(crate) fn issue_cache_snapshot(&self) -> issue_cache_store::IssueCache {
        issue_cache_store::issue_cache_snapshot(
            &self.repositories,
            &self.queued_issue_cache_base(),
            &self.issue_lists,
        )
    }

    pub(crate) fn queue_issue_cache_save(
        &mut self,
        next_cache: issue_cache_store::IssueCache,
        cx: &mut Context<Self>,
    ) {
        let queued_issue_cache_save = next_pending_issue_cache_save(
            &self.last_persisted_issue_cache,
            self.pending_issue_cache_save.as_ref(),
            self.issue_cache_save_in_flight.as_ref(),
            &next_cache,
        );
        let should_start_save =
            queued_issue_cache_save.is_some() && self.issue_cache_save_in_flight.is_none();
        self.pending_issue_cache_save = queued_issue_cache_save;
        if !should_start_save {
            return;
        }

        self.start_pending_issue_cache_save(cx);
    }

    pub(crate) fn start_pending_issue_cache_save(&mut self, cx: &mut Context<Self>) {
        if self.issue_cache_save_in_flight.is_some() {
            return;
        }

        let Some(next_cache) = self.pending_issue_cache_save.take() else {
            self.maybe_finish_quit_after_persistence_flush(cx);
            return;
        };

        self.issue_cache_save_in_flight = Some(next_cache.clone());
        let store = self.issue_cache_store.clone();
        let cache_to_save = next_cache.clone();
        self._issue_cache_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.save(&cache_to_save) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.issue_cache_save_in_flight = None;
                match result {
                    Ok(()) => {
                        this.last_persisted_issue_cache = next_cache.clone();
                        this.last_issue_cache_error = None;
                    },
                    Err(error) => {
                        let message = error.to_string();
                        if this.last_issue_cache_error.as_deref() != Some(message.as_str()) {
                            this.notice = Some(format!("failed to persist issue cache: {error}"));
                            this.last_issue_cache_error = Some(message);
                            cx.notify();
                        }
                    },
                }

                this.start_pending_issue_cache_save(cx);
                this.maybe_finish_quit_after_persistence_flush(cx);
            });
        }));
    }

    pub(crate) fn sync_issue_cache_store(&mut self, cx: &mut Context<Self>) {
        self.queue_issue_cache_save(self.issue_cache_snapshot(), cx);
    }

    pub(crate) fn handle_pane_divider_drag_move(
        &mut self,
        event: &DragMoveEvent<DraggedPaneDivider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace_width = f32::from(event.bounds.size.width);
        let available_side_width =
            (workspace_width - (2. * PANE_RESIZE_HANDLE_WIDTH) - PANE_CENTER_MIN_WIDTH).max(0.);

        match event.drag(cx) {
            DraggedPaneDivider::Left => {
                let proposed = f32::from(event.event.position.x - event.bounds.left());
                let max_width =
                    (available_side_width - self.right_pane_width).min(LEFT_PANE_MAX_WIDTH);
                let min_width = LEFT_PANE_MIN_WIDTH.min(max_width);
                self.left_pane_width = proposed.clamp(min_width, max_width);
            },
            DraggedPaneDivider::Right => {
                let proposed = f32::from(event.bounds.right() - event.event.position.x);
                let max_width =
                    (available_side_width - self.left_pane_width).min(RIGHT_PANE_MAX_WIDTH);
                let min_width = RIGHT_PANE_MIN_WIDTH.min(max_width);
                self.right_pane_width = proposed.clamp(min_width, max_width);
            },
        }

        self.clamp_pane_widths_for_workspace(workspace_width);
        self.sync_ui_state_store(window, cx);
        cx.stop_propagation();
        cx.notify();
    }

    pub(crate) fn render_pane_resize_handle(
        &self,
        id: &'static str,
        divider: DraggedPaneDivider,
        theme: ThemePalette,
    ) -> impl IntoElement {
        div()
            .id(id)
            .w(px(PANE_RESIZE_HANDLE_WIDTH))
            .h_full()
            .flex_none()
            .cursor_col_resize()
            .on_drag(divider, |dragged_divider, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| *dragged_divider)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .child(div().w(px(1.)).h_full().mx_auto().bg(rgb(theme.border)))
            .occlude()
    }

    pub(crate) fn render_notice_toast(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(notice) = self.notice.clone() else {
            return div();
        };

        let theme = self.theme();
        let is_error = notice_looks_like_error(&notice);
        let background = if is_error {
            theme.notice_bg
        } else {
            theme.chrome_bg
        };
        let text_color = if is_error {
            theme.notice_text
        } else {
            theme.text_primary
        };
        let border_color = if is_error {
            0xb95d5d
        } else {
            theme.accent
        };
        let icon = if is_error {
            "\u{f06a}"
        } else {
            "\u{f05a}"
        };
        let icon_color = if is_error {
            theme.notice_text
        } else {
            theme.accent
        };

        div()
            .absolute()
            .right(px(16.))
            .bottom(px(36.))
            .w(px(420.))
            .max_w(px(420.))
            .rounded_sm()
            .border_1()
            .border_color(rgb(border_color))
            .bg(rgb(background))
            .px_2()
            .py(px(8.))
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(12.))
                            .text_color(rgb(icon_color))
                            .child(icon),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(rgb(text_color))
                            .child(notice),
                    ),
            )
            .child(
                div()
                    .id("notice-toast-dismiss")
                    .cursor_pointer()
                    .font_family(FONT_MONO)
                    .text_size(px(11.))
                    .text_color(rgb(theme.text_muted))
                    .hover(|this| this.text_color(rgb(theme.text_primary)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    }))
                    .child("\u{f00d}"),
            )
    }

    pub(crate) fn render_status_bar(&self) -> impl IntoElement {
        let theme = self.theme();
        let repo_name = self
            .repo_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.repo_root.display().to_string());
        let worktree = self
            .active_worktree()
            .map(|entry| entry.label.clone())
            .unwrap_or_else(|| "none".to_owned());
        let terminal_count = self.selected_worktree_path().map_or(0, |worktree_path| {
            self.terminals
                .iter()
                .filter(|session| session.worktree_path.as_path() == worktree_path)
                .count()
        });

        div()
            .h(px(26.))
            .bg(rgb(theme.chrome_bg))
            .border_t_1()
            .border_color(rgb(theme.chrome_border))
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_text(theme, "●"))
                    .child(status_text(theme, format!("repo {repo_name}")))
                    .child(status_text(theme, "•"))
                    .child(status_text(theme, format!("worktree {worktree}"))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_text(
                        theme,
                        format!("changes {}", self.changed_files.len()),
                    ))
                    .child(status_text(theme, "•"))
                    .child(status_text(theme, format!("terminals {terminal_count}")))
                    .when_some(self.update_available.clone(), |this, version| {
                        this.child(status_text(theme, "•")).child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.accent))
                                .child(format!("update available: v{version}")),
                        )
                    })
                    .when_some(self.github_rate_limit_remaining(), |this, remaining| {
                        this.child(status_text(theme, "•")).child(
                            div().text_xs().text_color(rgb(theme.accent)).child(format!(
                                "GitHub rate limited: {}",
                                format_countdown(remaining)
                            )),
                        )
                    })
                    .child(if self.github_rate_limit_remaining().is_some() {
                        loading_status_text(theme, "waiting")
                    } else if let Some(label) = workspace_loading_status_label(
                        if self.worktree_stats_loading {
                            self.worktrees
                                .iter()
                                .filter(|worktree| worktree.diff_summary.is_none())
                                .count()
                        } else {
                            0
                        },
                        self.worktrees
                            .iter()
                            .filter(|worktree| worktree.pr_loading)
                            .count(),
                        self.worktrees.iter().any(|worktree| worktree.pr_loaded),
                    ) {
                        loading_status_text(
                            theme,
                            format!(
                                "{} {label}",
                                loading_spinner_frame(self.loading_animation_frame)
                            ),
                        )
                    } else {
                        status_text(theme, "ready")
                    }),
            )
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use {super::*, crate::ui_state_store};

    #[test]
    fn pending_save_coalesces_to_latest_value_after_inflight_write() {
        let mut pending = PendingSave::default();

        pending.queue("first");
        assert_eq!(pending.begin_next(), Some("first"));
        assert!(pending.has_work());

        pending.queue("second");
        pending.queue("third");
        assert!(pending.begin_next().is_none());

        pending.finish();

        assert_eq!(pending.begin_next(), Some("third"));
        pending.finish();
        assert!(!pending.has_work());
    }

    #[test]
    fn pending_save_reports_work_for_pending_and_inflight_states() {
        let mut pending = PendingSave::default();
        assert!(!pending.has_work());

        pending.queue(1_u8);
        assert!(pending.has_work());

        let _ = pending.begin_next();
        assert!(pending.has_work());

        pending.finish();
        assert!(!pending.has_work());
    }

    #[test]
    fn ui_state_save_has_work_for_pending_and_inflight_states() {
        let state = ui_state_store::UiState::default();

        assert!(!ui_state_save_has_work(None, None));
        assert!(ui_state_save_has_work(Some(&state), None));
        assert!(ui_state_save_has_work(None, Some(&state)));
    }

    #[test]
    fn next_pending_ui_state_save_keeps_reverted_state_queued_while_other_save_is_in_flight() {
        let persisted = ui_state_store::UiState {
            left_pane_width: Some(240),
            ..ui_state_store::UiState::default()
        };
        let in_flight = ui_state_store::UiState {
            left_pane_width: Some(320),
            ..ui_state_store::UiState::default()
        };

        assert_eq!(
            next_pending_ui_state_save(&persisted, None, Some(&in_flight), &persisted),
            Some(persisted),
        );
    }

    #[test]
    fn next_pending_ui_state_save_does_not_duplicate_inflight_state() {
        let state = ui_state_store::UiState {
            left_pane_width: Some(320),
            ..ui_state_store::UiState::default()
        };

        assert_eq!(
            next_pending_ui_state_save(
                &ui_state_store::UiState::default(),
                None,
                Some(&state),
                &state,
            ),
            None,
        );
    }
}
