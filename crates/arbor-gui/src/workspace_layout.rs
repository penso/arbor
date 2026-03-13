impl ArborWindow {
    fn clamp_pane_widths_for_workspace(&mut self, workspace_width: f32) {
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

    fn estimated_diff_wrap_columns(&self, cell_width_px: f32) -> usize {
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

    fn estimated_diff_wrap_columns_for_window_width(
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

    fn estimated_diff_wrap_columns_for_list_width(
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

    fn live_diff_list_width_px(&self) -> Option<f32> {
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

    fn rewrap_diff_sessions_if_needed(&mut self, wrap_columns: usize) {
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

    fn ui_state_snapshot(&self, window: &Window) -> ui_state_store::UiState {
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
            sidebar_order: self.last_persisted_ui_state.sidebar_order.clone(),
        }
    }

    fn sync_ui_state_store(&mut self, window: &Window, cx: &mut Context<Self>) {
        let next_state = self.ui_state_snapshot(window);
        if self.last_persisted_ui_state == next_state {
            return;
        }

        self.last_persisted_ui_state = next_state.clone();
        self.last_ui_state_error = None;

        let store = self.ui_state_store.clone();
        self._ui_state_save_task = Some(cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { store.save(&next_state) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result
                    && this.last_ui_state_error.as_deref() != Some(error.as_str())
                {
                    this.notice = Some(format!("failed to persist UI state: {error}"));
                    this.last_ui_state_error = Some(error);
                    cx.notify();
                }
            });
        }));
    }

    fn handle_pane_divider_drag_move(
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

    fn render_pane_resize_handle(
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

    fn render_notice_toast(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_status_bar(&self) -> impl IntoElement {
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
                    .child(status_text(theme, "•"))
                    .child(status_text(
                        theme,
                        format!("theme {}", self.theme_kind.label()),
                    ))
                    .child(
                        if self.worktree_stats_loading || self.worktree_prs_loading {
                            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                            let frame_index = (SystemTime::now()
                                .duration_since(SystemTime::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis()
                                / 100) as usize
                                % frames.len();
                            status_text(theme, format!("{} loading", frames[frame_index]))
                        } else {
                            status_text(theme, "ready")
                        },
                    ),
            )
    }
}
