use super::*;

impl Drop for ArborWindow {
    fn drop(&mut self) {
        self.stop_active_ssh_daemon_tunnel();
        remove_claude_code_hooks();
        remove_pi_agent_extension();
    }
}

impl EntityInputHandler for ArborWindow {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.ime_marked_text.as_ref().map(|text| {
            let len: usize = text.encode_utf16().count();
            0..len
        })
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime_marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = None;
        if text.is_empty() {
            cx.notify();
            return;
        }
        // Suppress all text input while the quit overlay is showing.
        if self.quit_overlay_until.is_some() {
            return;
        }
        // When a modal with a text field is open, route IME text there instead
        if let Some(ref mut modal) = self.daemon_auth_modal {
            modal.token.push_str(text);
            modal.error = None;
            cx.notify();
            return;
        }
        if let Some(ref mut modal) = self.connect_to_host_modal {
            modal.address.push_str(text);
            modal.error = None;
            cx.notify();
            return;
        }
        if let Some(ref mut modal) = self.command_palette_modal {
            modal.query.push_str(text);
            modal.selected_index = 0;
            self.command_palette_scroll_handle.scroll_to_item(0);
            cx.notify();
            return;
        }
        if let Some(ref mut modal) = self.commit_modal {
            modal.message.push_str(text);
            modal.error = None;
            cx.notify();
            return;
        }
        if self.welcome_clone_url_active {
            self.welcome_clone_url.push_str(text);
            self.welcome_clone_error = None;
            cx.notify();
            return;
        }
        if self.worktree_notes_active && self.right_pane_tab == RightPaneTab::Notes {
            self.insert_text_into_selected_worktree_notes(text, cx);
            cx.notify();
            return;
        }
        // Route text input to agent chat when an agent chat tab is active
        if let Some(CenterTab::AgentChat(local_id)) = self.active_center_tab_for_selected_worktree()
        {
            if let Some(session) = self
                .agent_chat_sessions
                .iter_mut()
                .find(|s| s.local_id == local_id)
            {
                let cursor = session.input_cursor;
                session.input_text.insert_str(cursor, text);
                session.input_cursor += text.len();
            }
            cx.notify();
            return;
        }
        let Some(session_id) = self.active_terminal_id_for_selected_worktree() else {
            return;
        };
        if let Some(input) = self.take_terminal_text_input_fallback(text) {
            if let Err(error) = self.write_input_to_terminal(session_id, &input) {
                self.notice = Some(format!("failed to write to terminal: {error}"));
                cx.notify();
                return;
            }
            self.notify_after_terminal_input(session_id, cx);
            return;
        }
        self.append_text_to_pending_input_buffers(session_id, text);
        if let Err(error) = self.write_input_to_terminal(session_id, text.as_bytes()) {
            self.notice = Some(format!("failed to write to terminal: {error}"));
            cx.notify();
            return;
        }
        self.notify_after_terminal_input(session_id, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = if new_text.is_empty() {
            None
        } else {
            Some(new_text.to_owned())
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for ArborWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update window title to reflect connected daemon
        let title = app_window_title(self.connected_daemon_label.as_deref());
        window.set_window_title(&title);

        self.window_is_active = window.is_window_active();
        if self.focus_terminal_on_next_render && self.active_terminal().is_some() {
            window.focus(&self.terminal_focus);
            self.focus_terminal_on_next_render = false;
        }
        let bounds = window.window_bounds().get_bounds();
        let current_window_geometry = ui_state_store::WindowGeometry {
            x: f32::from(bounds.origin.x).round() as i32,
            y: f32::from(bounds.origin.y).round() as i32,
            width: f32::from(bounds.size.width).round().max(1.) as u32,
            height: f32::from(bounds.size.height).round().max(1.) as u32,
        };
        if self.last_window_geometry != Some(current_window_geometry) {
            let workspace_width = current_window_geometry.width as f32;
            self.clamp_pane_widths_for_workspace(workspace_width);
            self.sync_ui_state_store(window, cx);
            self.last_window_geometry = Some(current_window_geometry);
        }

        let theme = self.theme();
        let left_pane_width = if self.left_pane_visible {
            self.left_pane_width
        } else {
            40.
        };
        let left_handle_width = if self.left_pane_visible {
            PANE_RESIZE_HANDLE_WIDTH
        } else {
            0.
        };
        let right_handle_width = PANE_RESIZE_HANDLE_WIDTH;
        let right_pane_width = self.right_pane_width;
        let center_left = left_pane_width + left_handle_width;
        let center_right = right_pane_width + right_handle_width;
        div()
            .size_full()
            .bg(rgb(theme.app_bg))
            .text_color(rgb(theme.text_primary))
            .font_family(FONT_UI)
            .relative()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(Self::handle_global_key_down))
            .on_action(cx.listener(Self::action_spawn_terminal))
            .on_action(cx.listener(Self::action_close_active_terminal))
            .on_action(cx.listener(Self::action_open_manage_presets))
            .on_action(cx.listener(Self::action_open_manage_repo_presets))
            .on_action(cx.listener(Self::action_open_command_palette))
            .on_action(cx.listener(Self::action_refresh_worktrees))
            .on_action(cx.listener(Self::action_refresh_changes))
            .on_action(cx.listener(Self::action_open_add_repository))
            .on_action(cx.listener(Self::action_open_create_worktree))
            .on_action(cx.listener(Self::action_toggle_left_pane))
            .on_action(cx.listener(Self::action_navigate_worktree_back))
            .on_action(cx.listener(Self::action_navigate_worktree_forward))
            .on_action(cx.listener(Self::action_collapse_all_repositories))
            .on_action(cx.listener(Self::action_view_logs))
            .on_action(cx.listener(Self::action_show_about))
            .on_action(cx.listener(Self::action_open_theme_picker))
            .on_action(cx.listener(Self::action_open_settings))
            .on_action(cx.listener(Self::action_open_manage_hosts))
            .on_action(cx.listener(Self::action_connect_to_lan_daemon))
            .on_action(cx.listener(Self::action_connect_to_host))
            .on_action(cx.listener(Self::action_request_quit))
            .on_action(cx.listener(Self::action_immediate_quit))
            .child(self.render_top_bar(cx))
            .child(div().h(px(1.)).bg(rgb(theme.chrome_border)))
            .when(self.repositories.is_empty(), |this| {
                this.child(self.render_welcome_pane(cx))
            })
            .when(!self.repositories.is_empty(), |this| {
                this.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .on_drag_move(cx.listener(Self::handle_pane_divider_drag_move))
                        .when(self.left_pane_visible, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(px(left_pane_width))
                                    .child(self.render_pane_resize_handle(
                                        "left-pane-resize",
                                        DraggedPaneDivider::Left,
                                        theme,
                                    )),
                            )
                        })
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(px(left_pane_width))
                                .child(self.render_left_pane(cx)),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(px(center_left))
                                .right(px(center_right))
                                .child(
                                    div().size_full().child(self.render_center_pane(window, cx)),
                                ),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right(px(right_pane_width))
                                .child(self.render_pane_resize_handle(
                                    "right-pane-resize",
                                    DraggedPaneDivider::Right,
                                    theme,
                                )),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(right_pane_width))
                                .child(self.render_right_pane(cx)),
                        ),
                )
            })
            .child(self.render_status_bar())
            .child(self.render_top_bar_worktree_quick_actions_menu(cx))
            .child(self.render_notice_toast(cx))
            .child(self.render_issue_details_modal(cx))
            .child(self.render_create_modal(cx))
            .child(self.render_github_auth_modal(cx))
            .child(self.render_new_tab_menu(cx))
            .child(self.render_repository_context_menu(cx))
            .child(self.render_worktree_context_menu(cx))
            .child(self.render_worktree_hover_popover(cx))
            .child(self.render_outpost_context_menu(cx))
            .child(self.render_delete_modal(cx))
            .child(self.render_manage_hosts_modal(cx))
            .child(self.render_manage_presets_modal(cx))
            .child(self.render_manage_repo_presets_modal(cx))
            .child(self.render_commit_modal(cx))
            .child(self.render_command_palette_modal(cx))
            .child(self.render_about_modal(cx))
            .child(self.render_theme_picker_modal(cx))
            .child(self.render_settings_modal(cx))
            .child(self.render_daemon_auth_modal(cx))
            .child(self.render_start_daemon_modal(cx))
            .child(self.render_connect_to_host_modal(cx))
            .child(div().when_some(self.theme_toast.clone(), |this, toast| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_end()
                        .justify_end()
                        .px_3()
                        .pb(px(34.))
                        .child(
                            div()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(theme.accent))
                                .bg(rgb(theme.panel_active_bg))
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(rgb(theme.text_primary))
                                .child(toast),
                        ),
                )
            }))
            .when(self.quit_overlay_until.is_some(), |this| {
                this.child(
                    div()
                        .id("quit-backdrop")
                        .absolute()
                        .inset_0()
                        .bg(rgb(0x000000))
                        .opacity(0.5)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.action_dismiss_quit(window, cx);
                        })),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(
                            div()
                                .px_6()
                                .py_4()
                                .rounded_lg()
                                .bg(rgb(theme.chrome_bg))
                                .border_1()
                                .border_color(rgb(theme.border))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme.text_primary))
                                        .child("Are you sure you want to quit Arbor?"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            action_button(
                                                theme,
                                                "quit-cancel",
                                                "Cancel",
                                                ActionButtonStyle::Secondary,
                                                true,
                                            )
                                            .min_w(px(64.))
                                            .flex()
                                            .justify_center()
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.action_dismiss_quit(window, cx);
                                                }),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .id("quit-confirm")
                                                .cursor_pointer()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(0xc94040))
                                                .bg(rgb(0xc94040))
                                                .min_w(px(64.))
                                                .flex()
                                                .justify_center()
                                                .px_2()
                                                .py_1()
                                                .text_xs()
                                                .text_color(rgb(0xffffff))
                                                .child("Quit")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.action_confirm_quit(window, cx);
                                                })),
                                        ),
                                ),
                        ),
                )
            })
    }
}

pub(crate) fn terminal_tab_title(session: &TerminalSession) -> String {
    if let Some(last_command) = session
        .last_command
        .as_ref()
        .filter(|command| !command.trim().is_empty())
    {
        return truncate_with_ellipsis(last_command.trim(), TERMINAL_TAB_COMMAND_MAX_CHARS);
    }

    if !session.title.is_empty() && !session.title.starts_with("term-") {
        return truncate_with_ellipsis(&session.title, TERMINAL_TAB_COMMAND_MAX_CHARS);
    }

    String::new()
}
