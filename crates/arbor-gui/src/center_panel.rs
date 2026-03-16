use super::*;

/// Handler for the "+" new tab button.  When `agent-chat` is enabled this opens
/// a dropdown menu; otherwise it spawns a terminal directly.
#[cfg(feature = "agent-chat")]
fn new_tab_button_handler(
    this: &mut ArborWindow,
    event: &MouseDownEvent,
    _window: &mut Window,
    cx: &mut Context<ArborWindow>,
) {
    this.new_tab_menu_position = if this.new_tab_menu_position.is_some() {
        None
    } else {
        Some(event.position)
    };
    cx.stop_propagation();
    cx.notify();
}

#[cfg(not(feature = "agent-chat"))]
fn new_tab_button_handler(
    this: &mut ArborWindow,
    _event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<ArborWindow>,
) {
    cx.stop_propagation();
    this.spawn_terminal_session(window, cx);
}

impl ArborWindow {
    pub(crate) fn render_terminal_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cell_width = diff_cell_width_px(cx);
        let wrap_columns = if let Some(list_width) = self.live_diff_list_width_px() {
            self.estimated_diff_wrap_columns_for_list_width(list_width, cell_width)
        } else {
            let window_width = f32::from(window.window_bounds().get_bounds().size.width).max(600.);
            self.estimated_diff_wrap_columns_for_window_width(window_width, cell_width)
        };
        self.rewrap_diff_sessions_if_needed(wrap_columns);

        let theme = self.theme();
        let terminals = self.selected_worktree_terminals();
        let diff_sessions = self.selected_worktree_diff_sessions();
        let file_view_sessions = self.selected_worktree_file_view_sessions();
        // Collect all current tabs for this worktree (unordered)
        let mut current_tabs: Vec<CenterTab> = terminals
            .iter()
            .map(|session| CenterTab::Terminal(session.id))
            .collect();
        current_tabs.extend(
            diff_sessions
                .iter()
                .map(|session| CenterTab::Diff(session.id)),
        );
        current_tabs.extend(
            file_view_sessions
                .iter()
                .map(|session| CenterTab::FileView(session.id)),
        );
        if let Some(worktree_path) = self.selected_worktree_path() {
            let worktree_path = worktree_path.to_path_buf();
            current_tabs.extend(
                self.agent_chat_sessions
                    .iter()
                    .filter(|s| s.workspace_path == worktree_path)
                    .map(|s| CenterTab::AgentChat(s.local_id)),
            );
        }
        if self.logs_tab_open {
            current_tabs.push(CenterTab::Logs);
        }

        // Order tabs by creation order: known tabs keep their position, new ones are appended
        let mut tabs: Vec<CenterTab> = self
            .center_tab_order
            .iter()
            .copied()
            .filter(|tab| current_tabs.contains(tab))
            .collect();
        for tab in &current_tabs {
            if !tabs.contains(tab) {
                tabs.push(*tab);
            }
        }
        // Update the stored order
        self.center_tab_order = tabs.clone();

        let mut active_tab = self.active_center_tab_for_selected_worktree();
        if active_tab.is_some_and(|tab| !tabs.contains(&tab)) {
            active_tab = None;
        }
        if active_tab.is_none() {
            active_tab = tabs.first().copied();
            self.active_diff_session_id = match active_tab {
                Some(CenterTab::Diff(diff_id)) => Some(diff_id),
                _ => None,
            };
            self.active_file_view_session_id = match active_tab {
                Some(CenterTab::FileView(fv_id)) => Some(fv_id),
                _ => None,
            };
        }

        let active_tab_index =
            active_tab.and_then(|tab| tabs.iter().position(|entry| *entry == tab));
        if let Some(index) = active_tab_index {
            self.center_tabs_scroll_handle.scroll_to_item(index);
        }
        let active_terminal = match active_tab {
            Some(CenterTab::Terminal(session_id)) => self
                .terminals
                .iter()
                .find(|session| session.id == session_id)
                .cloned(),
            _ => None,
        };
        let active_diff_session = match active_tab {
            Some(CenterTab::Diff(diff_id)) => self
                .diff_sessions
                .iter()
                .find(|session| session.id == diff_id)
                .cloned(),
            _ => None,
        };
        let active_file_view_session = match active_tab {
            Some(CenterTab::FileView(fv_id)) => self
                .file_view_sessions
                .iter()
                .find(|session| session.id == fv_id)
                .cloned(),
            _ => None,
        };
        let active_agent_chat = match active_tab {
            Some(CenterTab::AgentChat(local_id)) => self
                .agent_chat_sessions
                .iter()
                .find(|s| s.local_id == local_id)
                .cloned(),
            _ => None,
        };
        let is_empty_state = active_terminal.is_none()
            && active_diff_session.is_none()
            && active_file_view_session.is_none()
            && active_agent_chat.is_none()
            && active_tab != Some(CenterTab::Logs);

        div()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .bg(rgb(theme.terminal_bg))
            .border_l_1()
            .border_r_1()
            .border_color(rgb(theme.border))
            .flex()
            .flex_col()
            .track_focus(&self.terminal_focus)
            .on_any_mouse_down(cx.listener(Self::focus_terminal_panel))
            .on_key_down(cx.listener(Self::handle_terminal_key_down))
            .child({
                let entity = cx.entity().clone();
                let focus = self.terminal_focus.clone();
                canvas(
                    move |_, _, _| {},
                    move |_, _, window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(
                                Bounds {
                                    origin: point(px(0.), px(0.)),
                                    size: size(px(0.), px(0.)),
                                },
                                entity.clone(),
                            ),
                            cx,
                        );
                    },
                )
                .size(px(0.))
            })
            // Tab bar header — tabs + "+" button only (presets moved inside terminal)
            .child(
                div()
                    .h(px(32.))
                    .bg(rgb(theme.tab_bg))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("center-tabs-scroll")
                            .track_scroll(&self.center_tabs_scroll_handle)
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .h_full()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .overflow_hidden()
                                    .when(tabs.is_empty(), |this| {
                                        this.child(
                                            div()
                                                .px_3()
                                                .text_xs()
                                                .text_color(rgb(theme.text_muted))
                                                .child("No tabs"),
                                        )
                                    })
                                    .children(tabs.iter().copied().enumerate().map(|(index, tab)| {
                                        self.render_center_tab(tab, index, &tabs, active_tab, active_tab_index, theme, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .h_full()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .px_2()
                                    .border_l_1()
                                    .border_color(rgb(theme.border))
                                    .border_b_1()
                                    .child(
                                        div()
                                            .id("terminal-tab-new")
                                            .relative()
                                            .size(px(20.))
                                            .cursor_pointer()
                                            .rounded_sm()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_sm()
                                            .text_color(rgb(theme.text_muted))
                                            .hover(|this| this.text_color(rgb(theme.text_primary)))
                                            .child("+")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(new_tab_button_handler),
                                            ),
                                    ),
                            ),
                    ),
            )
            // Content area
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(rgb(theme.terminal_bg))
                    .when(is_empty_state, |this| {
                        this.child(
                            div()
                                .h_full()
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .text_center()
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(theme.text_primary))
                                        .child("Workspace"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme.text_muted))
                                        .child("Press Cmd-T to open a terminal tab."),
                                )
                                .child(
                                    action_button(
                                        theme,
                                        "spawn-terminal-empty-state",
                                        "Open Terminal Tab",
                                        ActionButtonStyle::Secondary,
                                        true,
                                    )
                                    .on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.spawn_terminal_session(window, cx)
                                        }),
                                    ),
                                ),
                        )
                    })
                    .when_some(active_terminal, |this, session| {
                        let selection = self.terminal_selection_for_session(session.id);
                        let ime_text = self.ime_marked_text.as_deref();
                        let styled_lines =
                            styled_lines_for_session(&session, theme, true, selection, ime_text);
                        let mono_font = terminal_mono_font(cx);
                        let cell_width = terminal_cell_width_px(cx);
                        let line_height = terminal_line_height_px(cx);

                        this.child(
                            div()
                                .h_full()
                                .w_full()
                                .min_w_0()
                                .min_h_0()
                                .overflow_hidden()
                                .flex()
                                .flex_col()
                                .gap_0()
                                // Preset sub-bar inside the terminal tab
                                .child(self.render_preset_sub_bar(theme, cx))
                                // Terminal output
                                .child(
                                    div()
                                        .flex_1()
                                        .w_full()
                                        .min_w_0()
                                        .min_h_0()
                                        .overflow_hidden()
                                        .font(mono_font.clone())
                                        .text_size(px(TERMINAL_FONT_SIZE_PX))
                                        .line_height(px(line_height))
                                        .px_2()
                                        .pt_1()
                                        .flex()
                                        .flex_col()
                                        .gap_0()
                                        .child(
                                            div()
                                                .id("terminal-output-scroll")
                                                .flex_1()
                                                .w_full()
                                                .min_w_0()
                                                .min_h_0()
                                                .overflow_x_hidden()
                                                .overflow_y_scroll()
                                                .scrollbar_width(px(12.))
                                                .track_scroll(&self.terminal_scroll_handle)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(Self::handle_terminal_output_mouse_down),
                                                )
                                                .on_mouse_move(
                                                    cx.listener(Self::handle_terminal_output_mouse_move),
                                                )
                                                .on_mouse_up(
                                                    MouseButton::Left,
                                                    cx.listener(Self::handle_terminal_output_mouse_up),
                                                )
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .min_w_0()
                                                        .flex_none()
                                                        .flex()
                                                        .flex_col()
                                                        .gap_0()
                                                        .children(styled_lines.into_iter().map(|line| {
                                                            render_terminal_line(
                                                                line,
                                                                theme,
                                                                cell_width,
                                                                line_height,
                                                                mono_font.clone(),
                                                            )
                                                        })),
                                                ),
                                        ),
                                ),
                        )
                    })
                    .when_some(active_diff_session, |this, session| {
                        let mono_font = terminal_mono_font(cx);
                        let diff_cell_width = diff_cell_width_px(cx);
                        this.child(render_diff_session(
                            session,
                            theme,
                            &self.diff_scroll_handle,
                            mono_font,
                            diff_cell_width,
                        ))
                    })
                    .when_some(active_file_view_session, |this, session| {
                        let mono_font = terminal_mono_font(cx);
                        let editing = self.file_view_editing;
                        this.child(render_file_view_session(
                            session,
                            theme,
                            &self.file_view_scroll_handle,
                            mono_font,
                            editing,
                            cx,
                        ))
                    })
                    .when_some(active_agent_chat, |this, session| {
                        this.child(render_agent_chat_content(
                            &session,
                            self.agent_selector_open_for,
                            self.chat_mode_selector_open_for,
                            &self.agent_selector_search,
                            self.agent_selector_search_cursor,
                            &self.configured_providers,
                            theme,
                            &self.agent_chat_scroll_handle,
                            cx,
                        ))
                    })
                    .when(active_tab == Some(CenterTab::Logs), |this| {
                        this.child(self.render_logs_content(cx))
                    }),
            )
    }

    /// Render a single tab button in the center tab bar.
    fn render_center_tab(
        &self,
        tab: CenterTab,
        index: usize,
        tabs: &[CenterTab],
        active_tab: Option<CenterTab>,
        active_tab_index: Option<usize>,
        theme: ThemePalette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let is_active = active_tab == Some(tab);
        let tab_count = tabs.len();
        let relation = active_tab_index.map(|active_index| index.cmp(&active_index));
        let text_color = if is_active {
            theme.text_primary
        } else {
            theme.text_muted
        };
        let (tab_icon, tab_label, terminal_icon) = match tab {
            CenterTab::Terminal(session_id) => (
                terminal_tab_icon_element(is_active, text_color, 16.0).into_any_element(),
                self.terminals
                    .iter()
                    .find(|session| session.id == session_id)
                    .map(terminal_tab_title)
                    .unwrap_or_else(|| "terminal".to_owned()),
                true,
            ),
            CenterTab::Diff(diff_id) => (
                div()
                    .font_family(FONT_MONO)
                    .text_xs()
                    .text_color(rgb(text_color))
                    .child(TAB_ICON_DIFF)
                    .into_any_element(),
                self.diff_sessions
                    .iter()
                    .find(|session| session.id == diff_id)
                    .map(diff_tab_title)
                    .unwrap_or_else(|| "diff".to_owned()),
                false,
            ),
            CenterTab::FileView(fv_id) => (
                div()
                    .font_family(FONT_MONO)
                    .text_xs()
                    .text_color(rgb(text_color))
                    .child(TAB_ICON_FILE)
                    .into_any_element(),
                self.file_view_sessions
                    .iter()
                    .find(|session| session.id == fv_id)
                    .map(|s| s.title.clone())
                    .unwrap_or_else(|| "file".to_owned()),
                false,
            ),
            CenterTab::AgentChat(local_id) => {
                let session = self
                    .agent_chat_sessions
                    .iter()
                    .find(|s| s.local_id == local_id);
                let preset_kind = session.and_then(|s| AgentPresetKind::from_key(&s.agent_kind));
                let icon = if let Some(kind) = preset_kind {
                    agent_chat_tab_icon_element(kind, text_color, 20.0).into_any_element()
                } else {
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(14.))
                        .text_color(rgb(text_color))
                        .child("\u{f075}")
                        .into_any_element()
                };
                let label = session
                    .map(|s| {
                        let mut label = s.agent_kind.clone();
                        label[..1].make_ascii_uppercase();
                        label
                    })
                    .unwrap_or_else(|| "Agent".to_owned());
                (icon, label, true)
            },
            CenterTab::Logs => (
                logs_tab_icon_element(is_active, text_color, 16.0).into_any_element(),
                "Logs".to_owned(),
                true,
            ),
        };
        let tab_id = match tab {
            CenterTab::Terminal(id) => ("center-tab-terminal", id),
            CenterTab::Diff(id) => ("center-tab-diff", id),
            CenterTab::FileView(id) => ("center-tab-fileview", id),
            CenterTab::AgentChat(id) => ("center-tab-agent-chat", id),
            CenterTab::Logs => ("center-tab-logs", 0),
        };

        div()
            .id(tab_id)
            .group("tab")
            .relative()
            .h_full()
            .cursor_pointer()
            .w(px(160.))
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .border_color(rgb(theme.border))
            .bg(rgb(if is_active {
                theme.tab_active_bg
            } else {
                theme.tab_bg
            }))
            .when(!is_active, |this| {
                this.hover(|this| this.bg(rgb(theme.panel_active_bg)))
            })
            .child(
                div()
                    .font_family(FONT_MONO)
                    .when(terminal_icon, |this| this.text_size(px(24.)))
                    .when(!terminal_icon, |this| this.text_xs())
                    .text_color(rgb(text_color))
                    .child(tab_icon),
            )
            .child(div().text_sm().text_color(rgb(text_color)).child(tab_label))
            .child(
                div()
                    .id(match tab {
                        CenterTab::Terminal(id) => ("tab-close-terminal", id),
                        CenterTab::Diff(id) => ("tab-close-diff", id),
                        CenterTab::FileView(id) => ("tab-close-fileview", id),
                        CenterTab::AgentChat(id) => ("tab-close-agent-chat", id),
                        CenterTab::Logs => ("tab-close-logs", 0),
                    })
                    .absolute()
                    .right(px(4.))
                    .top_0()
                    .bottom_0()
                    .w(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_family(FONT_MONO)
                    .text_size(px(24.))
                    .text_color(rgb(theme.text_muted))
                    .invisible()
                    .group_hover("tab", |s| s.visible())
                    .child("\u{00d7}")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            match tab {
                                CenterTab::Terminal(session_id) => {
                                    if this.close_terminal_session_by_id(session_id) {
                                        this.sync_daemon_session_store(cx);
                                        this.terminal_scroll_handle.scroll_to_bottom();
                                        window.focus(&this.terminal_focus);
                                        this.focus_terminal_on_next_render = false;
                                        cx.notify();
                                    }
                                },
                                CenterTab::Diff(diff_id) => {
                                    if this.close_diff_session_by_id(diff_id) {
                                        cx.notify();
                                    }
                                },
                                CenterTab::FileView(fv_id) => {
                                    if this.close_file_view_session_by_id(fv_id) {
                                        cx.notify();
                                    }
                                },
                                CenterTab::AgentChat(local_id) => {
                                    this.close_agent_chat_by_local_id(local_id, cx);
                                },
                                CenterTab::Logs => {
                                    this.logs_tab_open = false;
                                    this.logs_tab_active = false;
                                    this.sync_navigation_ui_state_store(cx);
                                    cx.notify();
                                },
                            }
                        }),
                    ),
            )
            .when(index + 1 == tab_count, |this| this.border_r_1())
            .map(|this| match relation {
                Some(std::cmp::Ordering::Equal) => {
                    let el = this.border_r_1();
                    if index == 0 {
                        el
                    } else {
                        el.border_l_1()
                    }
                },
                Some(std::cmp::Ordering::Less) => {
                    let el = this.border_b_1();
                    if index == 0 {
                        el
                    } else {
                        el.border_l_1()
                    }
                },
                Some(std::cmp::Ordering::Greater) => this.border_r_1().border_b_1(),
                None => this.border_b_1(),
            })
            .on_click(cx.listener(move |this, _, window, cx| match tab {
                CenterTab::Terminal(session_id) => {
                    this.logs_tab_active = false;
                    this.select_terminal(session_id, window, cx);
                },
                CenterTab::Diff(diff_id) => {
                    this.logs_tab_active = false;
                    this.select_diff_tab(diff_id, cx);
                },
                CenterTab::FileView(fv_id) => {
                    this.logs_tab_active = false;
                    this.select_file_view_tab(fv_id, cx);
                },
                CenterTab::AgentChat(local_id) => {
                    this.logs_tab_active = false;
                    this.select_agent_chat_tab(local_id, cx);
                },
                CenterTab::Logs => {
                    this.logs_tab_active = true;
                    this.active_diff_session_id = None;
                    this.sync_navigation_ui_state_store(cx);
                    cx.notify();
                },
            }))
    }

    /// Render the preset sub-bar inside the active terminal tab.
    fn render_preset_sub_bar(&self, theme: ThemePalette, cx: &mut Context<Self>) -> Div {
        let installed = installed_preset_kinds();
        let has_presets = AgentPresetKind::ORDER.iter().any(|k| installed.contains(k))
            || !self.repo_presets.is_empty();

        if !has_presets {
            return div();
        }

        div()
            .h(px(28.))
            .flex_none()
            .w_full()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(rgb(theme.border))
            .bg(rgb(theme.tab_bg))
            .children(
                AgentPresetKind::ORDER
                    .iter()
                    .copied()
                    .filter(|kind| installed.contains(kind))
                    .map(|kind| {
                        let text_color = theme.text_muted;
                        div()
                            .id(ElementId::Name(
                                format!("terminal-inline-preset-{}", kind.key()).into(),
                            ))
                            .cursor_pointer()
                            .h(px(22.))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded_sm()
                            .text_color(rgb(text_color))
                            .hover(|s| {
                                s.bg(rgb(theme.panel_active_bg))
                                    .text_color(rgb(theme.text_primary))
                            })
                            .child(agent_preset_button_content(kind, text_color))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.run_preset_in_active_terminal(kind, cx);
                                }),
                            )
                    }),
            )
            .children(self.repo_presets.iter().enumerate().map(|(index, preset)| {
                let icon_text = preset.icon.clone();
                let name_text = preset.name.clone();
                div()
                    .id(ElementId::Name(
                        format!("terminal-inline-repo-preset-{index}").into(),
                    ))
                    .cursor_pointer()
                    .h(px(22.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .text_color(rgb(theme.text_muted))
                    .hover(|s| {
                        s.bg(rgb(theme.panel_active_bg))
                            .text_color(rgb(theme.text_primary))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(div().text_size(px(12.)).line_height(px(14.)).child(
                                if icon_text.is_empty() {
                                    "\u{f013}".to_owned()
                                } else {
                                    icon_text
                                },
                            ))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(14.))
                                    .child(name_text),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.run_repo_preset_in_active_terminal(index, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.open_manage_repo_presets_modal(Some(index), cx);
                        }),
                    )
            }))
    }

    /// Render the "+" new tab dropdown menu.
    ///
    /// Shows "Terminal" and agent chat options when the `agent-chat` feature is
    /// enabled.  Returns an empty div otherwise (the "+" button spawns a
    /// terminal directly).
    #[cfg(feature = "agent-chat")]
    pub(crate) fn render_new_tab_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(position) = self.new_tab_menu_position else {
            return div();
        };

        let theme = self.theme();

        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.new_tab_menu_position = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.new_tab_menu_position = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(position.x)
                    .top(position.y + px(8.))
                    .w(px(200.))
                    .py(px(4.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.chrome_bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    // Terminal option
                    .child(
                        div()
                            .id("new-tab-terminal")
                            .h(px(30.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                terminal_tab_icon_element(false, theme.text_muted, 14.0),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(theme.text_primary))
                                    .child("Terminal"),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_tab_menu_position = None;
                                this.spawn_terminal_session(window, cx);
                            })),
                    )
                    // Separator
                    .child(
                        div()
                            .h(px(1.))
                            .mx(px(8.))
                            .my(px(4.))
                            .bg(rgb(theme.border)),
                    )
                    // Agent Chat option — agent choice happens in the composer toolbar
                    .child(
                        div()
                            .id("new-tab-agent-chat")
                            .h(px(30.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(FONT_MONO)
                                    .text_color(rgb(theme.text_muted))
                                    .child("󰭹"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(theme.text_primary))
                                    .child("Agent Chat (alpha)"),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_tab_menu_position = None;
                                // Default to Claude; user can change agent in the composer
                                this.spawn_agent_chat(AgentPresetKind::Claude, None, cx);
                            })),
                    ),
            )
    }

    #[cfg(not(feature = "agent-chat"))]
    pub(crate) fn render_new_tab_menu(&mut self, _cx: &mut Context<Self>) -> Div {
        div()
    }

    pub(crate) fn render_center_pane(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = self.theme();
        div()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .bg(rgb(theme.app_bg))
            .flex()
            .flex_col()
            .child(self.render_terminal_panel(window, cx))
    }
}

fn render_agent_chat_content(
    session: &NativeAgentChatSession,
    agent_selector_open_for: Option<u64>,
    chat_mode_selector_open_for: Option<u64>,
    agent_selector_search: &str,
    agent_selector_search_cursor: usize,
    configured_providers: &[ConfiguredProvider],
    theme: ThemePalette,
    scroll_handle: &ScrollHandle,
    cx: &mut Context<ArborWindow>,
) -> Div {
    let local_id = session.local_id;
    let is_working = session.status == "working";
    let is_exited = session.status == "exited";
    let agent_kind = AgentPresetKind::from_key(&session.agent_kind);
    let selected_model_id = session.selected_model_id.clone();
    let model_label: String = if let Some(ref model_id) = selected_model_id {
        // Find the label for the selected model
        agent_kind
            .and_then(|k| k.models().iter().find(|m| m.id == model_id.as_str()))
            .map(|m| m.label.to_owned())
            .unwrap_or_else(|| model_id.clone())
    } else {
        agent_kind
            .map(|k| k.label().to_owned())
            .unwrap_or_else(|| session.agent_kind.clone())
    };
    // Sum per-message tokens for accurate totals (includes char-based estimates).
    // Fall back to session-level cumulative if no per-message data exists.
    let (total_in, total_out) = {
        let (msg_in, msg_out) = session
            .messages
            .iter()
            .filter(|m| m.role == "assistant")
            .fold((0u64, 0u64), |(ai, ao), m| {
                (ai + m.input_tokens, ao + m.output_tokens)
            });
        if msg_in > 0 || msg_out > 0 {
            (msg_in, msg_out)
        } else {
            (session.input_tokens, session.output_tokens)
        }
    };
    let token_text = format_token_count_with_total(total_in, total_out);
    let show_agent_selector = agent_selector_open_for == Some(local_id);
    let show_mode_selector = chat_mode_selector_open_for == Some(local_id);

    div()
        .h_full()
        .w_full()
        .min_w_0()
        .min_h_0()
        .relative()
        .flex()
        .flex_col()
        // Message list (flex-1, scrollable)
        .child(
            div()
                .id("agent-chat-messages")
                .flex_1()
                .w_full()
                .min_h_0()
                .overflow_y_scroll()
                .overflow_x_hidden()
                .track_scroll(scroll_handle)
                .px_4()
                .py_3()
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .children({
                            let msg_count = session.messages.len();
                            // Find the index of the last assistant message
                            let last_assistant_idx = session
                                .messages
                                .iter()
                                .rposition(|m| m.role == "assistant");
                            let session_input = total_in;
                            let session_output = total_out;
                            let streaming_info = if is_working {
                                Some((session.turn_streamed_chars, session.turn_start_time))
                            } else {
                                None
                            };
                            session.messages.iter().enumerate().map(
                                move |(i, msg)| {
                                    let is_last_assistant =
                                        last_assistant_idx == Some(i);
                                    render_chat_message(
                                        msg,
                                        i,
                                        msg_count,
                                        is_last_assistant,
                                        session_input,
                                        session_output,
                                        streaming_info,
                                        theme,
                                    )
                                },
                            )
                        })
                        .when(is_working, |this| {
                            this.child(render_thinking_indicator(theme))
                        })
                        .when(session.messages.is_empty() && !is_working, |this| {
                            this.child(render_empty_chat_state(&model_label, theme))
                        }),
                ),
        )
        // Composer area (bottom, fixed)
        .child(render_composer(
            local_id,
            session,
            is_working,
            is_exited,
            &model_label,
            &token_text,
            agent_kind,
            theme,
            cx,
        ))
        // Dismiss overlay: click anywhere outside popup to close it.
        // Rendered before the popup so GPUI paints the overlay first; the popup
        // is a later sibling and therefore appears on top.
        .when(show_agent_selector || show_mode_selector, |this| {
            this.child(
                div()
                    .id("popup-dismiss-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.agent_selector_open_for = None;
                        this.agent_selector_search.clear();
                        this.agent_selector_search_cursor = 0;
                        this.chat_mode_selector_open_for = None;
                        cx.notify();
                    })),
            )
        })
        // Agent selector popup (anchored to bottom-right above composer)
        .when(show_agent_selector, |this| {
            this.child(render_agent_selector_popup(
                local_id,
                agent_kind,
                selected_model_id.as_deref(),
                agent_selector_search,
                agent_selector_search_cursor,
                configured_providers,
                theme,
                cx,
            ))
        })
        // Chat mode selector popup
        .when(show_mode_selector, |this| {
            this.child(render_chat_mode_selector_popup(
                local_id,
                session.chat_mode,
                theme,
                cx,
            ))
        })
}

/// Render the agent selector popup above the model pill.
fn render_agent_selector_popup(
    local_id: u64,
    current_kind: Option<AgentPresetKind>,
    current_model_id: Option<&str>,
    search_text: &str,
    search_cursor: usize,
    configured_providers: &[ConfiguredProvider],
    theme: ThemePalette,
    cx: &mut Context<ArborWindow>,
) -> Div {
    let installed = installed_preset_kinds();
    let query = search_text.trim().to_ascii_lowercase();

    // Build entries: ACP provider sections first, then configured providers.
    let mut entries: Vec<ModelSelectorEntry> = Vec::new();

    // ACP agents (via acpx)
    for &kind in &AgentPresetKind::ORDER {
        if !installed.contains(&kind) {
            continue;
        }
        let models: Vec<&AgentModel> = kind
            .models()
            .iter()
            .filter(|m| query.is_empty() || m.label.to_ascii_lowercase().contains(&query))
            .collect();
        if models.is_empty() {
            continue;
        }
        entries.push(ModelSelectorEntry::Separator(
            kind.provider_name().to_owned(),
        ));
        for model in models {
            entries.push(ModelSelectorEntry::AcpModel(model.clone()));
        }
    }

    // Configured OpenAI-compatible providers (from config.toml)
    for provider in configured_providers {
        let models: Vec<&ConfiguredModel> = provider
            .models
            .iter()
            .filter(|m| query.is_empty() || m.label.to_ascii_lowercase().contains(&query))
            .collect();
        if models.is_empty() {
            continue;
        }
        entries.push(ModelSelectorEntry::Separator(provider.name.clone()));
        for model in models {
            entries.push(ModelSelectorEntry::ApiModel(model.clone()));
        }
    }

    let search_text = search_text.to_owned();
    let search_before = search_text[..search_cursor.min(search_text.len())].to_owned();
    let search_after = search_text[search_cursor.min(search_text.len())..].to_owned();

    div()
        .absolute()
        .bottom(px(60.))
        .right(px(12.))
        .w(px(280.))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.chrome_bg))
        .shadow_lg()
        .flex()
        .flex_col()
        // Search bar
        .child(
            div()
                .px(px(8.))
                .py(px(8.))
                .border_b_1()
                .border_color(rgb(theme.border))
                .child(
                    div()
                        .w_full()
                        .px(px(8.))
                        .py(px(5.))
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.terminal_bg))
                        .text_sm()
                        .text_color(rgb(theme.text_primary))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_xs()
                                .text_color(rgb(theme.text_disabled))
                                .mr(px(6.))
                                .child("\u{f002}"), // nf-fa-search
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .relative()
                                .flex()
                                .items_center()
                                .when(search_text.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .absolute()
                                            .text_color(rgb(theme.text_disabled))
                                            .child("Select a model…"),
                                    )
                                })
                                .child(search_before)
                                .child(
                                    div()
                                        .w(px(1.5))
                                        .h(px(16.))
                                        .bg(rgb(theme.accent))
                                        .flex_none(),
                                )
                                .child(search_after),
                        ),
                ),
        )
        // Model list
        .child(
            div()
                .id(ElementId::Name("model-selector-list".into()))
                .py(px(4.))
                .max_h(px(320.))
                .overflow_y_scroll()
                .children(entries.into_iter().enumerate().map(|(ix, entry)| {
                    match entry {
                        ModelSelectorEntry::Separator(label) => div()
                            .px(px(10.))
                            .pb(px(2.))
                            .when(ix > 0, |this| {
                                this.mt(px(4.))
                                    .pt(px(6.))
                                    .border_t_1()
                                    .border_color(rgb(theme.border))
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child(label),
                            )
                            .into_any_element(),
                        ModelSelectorEntry::AcpModel(model) => {
                            let is_current = current_kind == Some(model.provider)
                                && match current_model_id {
                                    Some(id) => id == model.id,
                                    // No explicit model → highlight the first model of the provider
                                    None => model.provider.models().first().map(|m| m.id) == Some(model.id),
                                };
                            let model_provider = model.provider;
                            let model_id = model.id;
                            div()
                                .id(ElementId::Name(
                                    format!("model-select-{}", model.id).into(),
                                ))
                                .mx(px(6.))
                                .px(px(8.))
                                .py(px(6.))
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                .when(is_current, |this| this.bg(rgb(theme.panel_active_bg)))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(agent_chat_tab_icon_element(
                                    model.provider,
                                    if is_current {
                                        theme.text_primary
                                    } else {
                                        theme.text_muted
                                    },
                                    16.0,
                                ))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme.text_primary))
                                        .child(model.label),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.agent_selector_open_for = None;
                                    this.agent_selector_search.clear();
                                    this.agent_selector_search_cursor = 0;
                                    // Kill the current session and create a new
                                    // one with the correct agent kind + model so
                                    // the daemon actually uses the right CLI.
                                    this.close_agent_chat_by_local_id(local_id, cx);
                                    this.spawn_agent_chat(
                                        model_provider,
                                        Some(model_id.to_owned()),
                                        cx,
                                    );
                                }))
                                .into_any_element()
                        },
                        ModelSelectorEntry::ApiModel(model) => {
                            let model_label = model.label.clone();
                            let model_id = model.id.clone();
                            let provider_name = model.provider_name.clone();
                            div()
                                .id(ElementId::Name(
                                    format!("model-select-api-{}", model.id).into(),
                                ))
                                .mx(px(6.))
                                .px(px(8.))
                                .py(px(6.))
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                // API icon — use a network/globe glyph to distinguish from ACP
                                .child(
                                    div()
                                        .font_family(FONT_MONO)
                                        .text_color(rgb(theme.text_muted))
                                        .text_size(px(14.))
                                        .flex_none()
                                        .w(px(18.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child("\u{f0ac}"), // nf-fa-globe
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme.text_primary))
                                        .child(model_label),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.agent_selector_open_for = None;
                                    this.agent_selector_search.clear();
                                    this.agent_selector_search_cursor = 0;

                                    // Close the current session first
                                    this.close_agent_chat_by_local_id(local_id, cx);

                                    // Find the provider to get base_url and api_key
                                    let provider = this.configured_providers.iter()
                                        .find(|p| p.name == provider_name);

                                    if let Some(provider) = provider {
                                        // Create a new session with OpenAI transport
                                        let transport = terminal_daemon_http::AgentChatTransport::OpenAiChat {
                                            base_url: provider.base_url.clone(),
                                            api_key: provider.api_key.clone(),
                                        };
                                        this.spawn_api_agent_chat(
                                            &provider_name,
                                            &model_id,
                                            transport,
                                            cx,
                                        );
                                    } else {
                                        this.notice = Some(format!("Provider '{}' not found", provider_name));
                                    }
                                    cx.notify();
                                }))
                                .into_any_element()
                        },
                    }
                })),
        )
}

/// Render the chat mode selector popup (Ask permissions, Auto accept, Plan, Bypass).
fn render_chat_mode_selector_popup(
    local_id: u64,
    current_mode: AgentChatMode,
    theme: ThemePalette,
    cx: &mut Context<ArborWindow>,
) -> Div {
    div()
        .absolute()
        .bottom(px(60.))
        .left(px(12.))
        .w(px(280.))
        .py(px(6.))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.chrome_bg))
        .shadow_lg()
        .children(AgentChatMode::ORDER.iter().copied().map(|mode| {
            let is_current = current_mode == mode;
            div()
                .id(ElementId::Name(
                    format!("mode-select-{}", mode.label()).into(),
                ))
                .mx(px(6.))
                .px(px(10.))
                .py(px(8.))
                .rounded_md()
                .cursor_pointer()
                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                .when(is_current, |this| this.bg(rgb(theme.panel_active_bg)))
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .font_family(FONT_MONO)
                        .text_base()
                        .w(px(24.))
                        .flex_shrink_0()
                        .text_color(if is_current {
                            rgb(theme.text_primary)
                        } else {
                            rgb(theme.text_muted)
                        })
                        .child(mode.icon()),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(theme.text_primary))
                                .child(mode.label()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_disabled))
                                .child(mode.subtitle()),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.chat_mode_selector_open_for = None;
                    if let Some(session) = this
                        .agent_chat_sessions
                        .iter_mut()
                        .find(|s| s.local_id == local_id)
                    {
                        session.chat_mode = mode;
                    }
                    cx.notify();
                }))
        }))
}

/// Render the bottom composer area: text editor + toolbar.
fn render_composer(
    local_id: u64,
    session: &NativeAgentChatSession,
    is_working: bool,
    is_exited: bool,
    model_label: &str,
    token_text: &str,
    agent_kind: Option<AgentPresetKind>,
    theme: ThemePalette,
    cx: &mut Context<ArborWindow>,
) -> Div {
    let model_label = model_label.to_owned();
    let token_text = token_text.to_owned();
    let chat_mode = session.chat_mode;

    // Calculate textarea height: 3 lines minimum, expand up to 10 lines, then scroll.
    // line_height = 20px, py = 8px top + 8px bottom = 16px
    let line_count = session.input_text.lines().count().max(1);
    let visible_lines = line_count.clamp(3, 10);
    let textarea_height = (visible_lines as f32) * 20.0 + 16.0;

    div()
        .flex_none()
        .w_full()
        .border_t_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.chrome_bg))
        .flex()
        .flex_col()
        // Text editor area
        .child(
            div()
                .w_full()
                .px_3()
                .pt_3()
                .pb_1()
                .child(
                    div()
                        .id(ElementId::Name(
                            format!("agent-chat-input-{local_id}").into(),
                        ))
                        .w_full()
                        .h(px(textarea_height))
                        .overflow_y_scroll()
                        .relative()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgb(theme.border))
                        .bg(rgb(theme.terminal_bg))
                        .text_sm()
                        .line_height(px(20.))
                        .text_color(rgb(theme.text_primary))
                        .when(is_exited, |this| this.opacity(0.5))
                        .child({
                            let cursor = session.input_cursor.min(session.input_text.len());
                            let before = &session.input_text[..cursor];
                            let after = &session.input_text[cursor..];
                            let is_empty = session.input_text.is_empty();

                            // Split text into lines, placing the cursor on the
                            // correct line so it follows newlines properly.
                            let before_lines: Vec<&str> = before.split('\n').collect();
                            let after_lines: Vec<&str> = after.split('\n').collect();

                            let cursor_element = div()
                                .w(px(1.5))
                                .h(px(18.))
                                .bg(rgb(theme.accent))
                                .flex_none();

                            let mut container = div()
                                .w_full()
                                .relative()
                                .flex()
                                .flex_col();

                            if is_empty {
                                container = container.child(
                                    div()
                                        .absolute()
                                        .text_color(rgb(theme.text_muted))
                                        .child("Message the agent…"),
                                );
                            }

                            // Lines before the cursor line
                            for line in &before_lines[..before_lines.len().saturating_sub(1)] {
                                container = container.child(
                                    div()
                                        .w_full()
                                        .min_h(px(20.))
                                        .child((*line).to_owned()),
                                );
                            }

                            // The cursor line: last before-line + cursor + first after-line
                            let cursor_line_before =
                                before_lines.last().copied().unwrap_or("");
                            let cursor_line_after =
                                after_lines.first().copied().unwrap_or("");
                            container = container.child(
                                div()
                                    .w_full()
                                    .min_h(px(20.))
                                    .flex()
                                    .items_center()
                                    .child(cursor_line_before.to_owned())
                                    .child(cursor_element)
                                    .child(cursor_line_after.to_owned()),
                            );

                            // Lines after the cursor line
                            for line in &after_lines[1..] {
                                container = container.child(
                                    div()
                                        .w_full()
                                        .min_h(px(20.))
                                        .child((*line).to_owned()),
                                );
                            }

                            container
                        }),
                ),
        )
        // Toolbar row
        .child(
            div()
                .w_full()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .justify_between()
                // Left side: mode selector + workspace path
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        // Mode selector pill
                        .child(
                            render_toolbar_pill(
                                "agent-chat-mode",
                                vec![
                                    div()
                                        .font_family(FONT_MONO)
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .child(chat_mode.icon())
                                        .into_any_element(),
                                    div()
                                        .text_color(rgb(theme.text_primary))
                                        .child(chat_mode.label())
                                        .into_any_element(),
                                    div()
                                        .font_family(FONT_MONO)
                                        .text_xs()
                                        .text_color(rgb(theme.text_muted))
                                        .child("▾")
                                        .into_any_element(),
                                ],
                                theme,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.chat_mode_selector_open_for == Some(local_id) {
                                    this.chat_mode_selector_open_for = None;
                                } else {
                                    this.chat_mode_selector_open_for = Some(local_id);
                                    this.agent_selector_open_for = None;
                                }
                                cx.notify();
                            })),
                        )
                        // Workspace path
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_xs()
                                .text_color(rgb(theme.text_disabled))
                                .child(
                                    div()
                                        .font_family(FONT_MONO)
                                        .child("\u{f07b}"), // nf-fa-folder
                                )
                                .child({
                                    let path_str =
                                        session.workspace_path.display().to_string();
                                    if let Ok(home) = env::var("HOME") {
                                        if let Some(rest) =
                                            path_str.strip_prefix(home.as_str())
                                        {
                                            format!("~{rest}")
                                        } else {
                                            path_str
                                        }
                                    } else {
                                        path_str
                                    }
                                }),
                        )
                        // Token count (when non-zero)
                        .when(!token_text.is_empty(), |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child(token_text.clone()),
                            )
                        })
                        // Status indicator when working
                        .when(is_working, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.accent))
                                    .child("thinking…"),
                            )
                        }),
                )
                // Right side: model selector + stop + send buttons
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        // Model pill (clickable to switch agent)
                        .child(
                            render_toolbar_pill(
                                "agent-chat-model",
                                {
                                    let mut parts: Vec<AnyElement> = Vec::new();
                                    if let Some(kind) = agent_kind {
                                        parts.push(
                                            agent_chat_tab_icon_element(
                                                kind,
                                                theme.text_muted,
                                                14.0,
                                            )
                                            .into_any_element(),
                                        );
                                    }
                                    parts.push(
                                        div()
                                            .text_color(rgb(theme.text_primary))
                                            .child(model_label.clone())
                                            .into_any_element(),
                                    );
                                    parts.push(
                                        div()
                                            .font_family(FONT_MONO)
                                            .text_xs()
                                            .text_color(rgb(theme.text_muted))
                                            .child("▾")
                                            .into_any_element(),
                                    );
                                    parts
                                },
                                theme,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.agent_selector_open_for == Some(local_id) {
                                    this.agent_selector_open_for = None;
                                    this.agent_selector_search.clear();
                                    this.agent_selector_search_cursor = 0;
                                } else {
                                    this.agent_selector_open_for = Some(local_id);
                                    this.agent_selector_search.clear();
                                    this.agent_selector_search_cursor = 0;
                                    this.chat_mode_selector_open_for = None;
                                }
                                cx.notify();
                            })),
                        )
                        // Stop button (only when working)
                        .when(is_working, |this| {
                            this.child(
                                div()
                                    .id(ElementId::Name(
                                        format!("agent-chat-stop-{local_id}").into(),
                                    ))
                                    .cursor_pointer()
                                    .size(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(rgb(theme.panel_active_bg))
                                    .hover(|s| s.bg(rgb(theme.border)))
                                    .child(
                                        div()
                                            .size(px(10.))
                                            .rounded_sm()
                                            .bg(rgb(theme.text_primary)),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.cancel_agent_chat(local_id, cx);
                                    })),
                            )
                        })
                        // Send button (up arrow)
                        .child(
                            div()
                                .id(ElementId::Name(
                                    format!("agent-chat-send-{local_id}").into(),
                                ))
                                .cursor_pointer()
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .when(!is_exited && !is_working, |this| {
                                    this.bg(rgb(theme.accent))
                                        .text_color(rgb(0xffffff))
                                        .hover(|s| s.opacity(0.9))
                                })
                                .when(is_exited || is_working, |this| {
                                    this.bg(rgb(theme.panel_active_bg))
                                        .text_color(rgb(theme.text_muted))
                                        .opacity(0.5)
                                })
                                .child(
                                    div()
                                        .font_family(FONT_MONO)
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .child("↑"),
                                )
                                .when(!is_exited && !is_working, |this| {
                                    this.on_click(cx.listener(move |this, _, _, cx| {
                                        this.send_agent_chat_message(local_id, cx);
                                    }))
                                }),
                        ),
                ),
        )
}

/// Render a small pill-shaped button for the toolbar.
fn render_toolbar_pill(
    id: &'static str,
    children: Vec<AnyElement>,
    theme: ThemePalette,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded_md()
        .bg(rgb(theme.panel_active_bg))
        .text_xs()
        .cursor_pointer()
        .hover(|this| this.bg(rgb(theme.border)))
        .children(children)
}

/// Render the empty state when no messages have been sent.
fn render_empty_chat_state(model_label: &str, theme: ThemePalette) -> Div {
    div()
        .h_full()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme.text_primary))
                .child(format!("{model_label} Chat (alpha)")),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(theme.text_muted))
                .text_center()
                .child("Ask a question, request code changes, or start a task."),
        )
        .child(
            div()
                .mt_2()
                .text_xs()
                .text_color(rgb(theme.text_muted))
                .child("Enter to send · Shift+Enter for newline"),
        )
}

/// Render a thinking/loading indicator.
fn render_thinking_indicator(theme: ThemePalette) -> Div {
    div().w_full().px_1().py_2().child(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                // Pulsing dot
                div()
                    .w(px(6.))
                    .h(px(6.))
                    .rounded_full()
                    .bg(rgb(theme.accent))
                    .flex_none(),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(theme.text_muted))
                    .child("Thinking…"),
            ),
    )
}

/// Render a single chat message (user or assistant).
fn render_chat_message(
    msg: &AgentChatMessage,
    index: usize,
    _msg_count: usize,
    is_last_assistant: bool,
    session_input_tokens: u64,
    session_output_tokens: u64,
    // When the session is working: `Some((turn_streamed_chars, turn_start_time))`.
    streaming_info: Option<(usize, Option<Instant>)>,
    theme: ThemePalette,
) -> Stateful<Div> {
    let is_user = msg.role == "user";
    let is_error = msg.role == "error";
    let is_assistant = !is_user && !is_error;
    let is_streaming = streaming_info.is_some();

    // Build footer parts for assistant messages (text + optional colored speed)
    let footer_parts: Option<(String, Option<(String, u32)>)> = if is_assistant {
        if msg.input_tokens > 0 || msg.output_tokens > 0 {
            // Completed message with per-message token data
            let mut parts: Vec<String> = Vec::new();
            if let Some(label) = &msg.transport_label {
                parts.push(label.clone());
            }
            if let Some(model) = &msg.model_id {
                parts.push(model.clone());
            }
            parts.push(format_token_count(msg.input_tokens, msg.output_tokens));
            let speed = msg.tokens_per_sec.map(format_speed);
            Some((parts.join(" · "), speed))
        } else if is_last_assistant && is_streaming {
            // Currently streaming — show live estimated token count
            // Safe: we checked is_streaming above
            let (streamed_chars, start_time) = streaming_info.unwrap_or((0, None));
            let estimated_tokens = (streamed_chars / 4) as u64;
            if estimated_tokens > 0 {
                let mut parts: Vec<String> = Vec::new();
                if let Some(label) = &msg.transport_label {
                    parts.push(label.clone());
                }
                if let Some(model) = &msg.model_id {
                    parts.push(model.clone());
                }
                parts.push(format!("~{} out", format_tokens(estimated_tokens)));
                // Live speed estimate
                let speed = start_time.and_then(|t| {
                    let elapsed = t.elapsed().as_secs_f64();
                    if elapsed > 0.5 {
                        Some(format_speed(estimated_tokens as f64 / elapsed))
                    } else {
                        None
                    }
                });
                Some((parts.join(" · "), speed))
            } else {
                None
            }
        } else if is_last_assistant && (session_input_tokens > 0 || session_output_tokens > 0) {
            // Idle with session-level cumulative tokens (no per-message data yet)
            let mut parts: Vec<String> = Vec::new();
            if let Some(label) = &msg.transport_label {
                parts.push(label.clone());
            }
            if let Some(model) = &msg.model_id {
                parts.push(model.clone());
            }
            parts.push(format_token_count_with_total(
                session_input_tokens,
                session_output_tokens,
            ));
            let speed = msg.tokens_per_sec.map(format_speed);
            Some((parts.join(" · "), speed))
        } else {
            None
        }
    } else {
        None
    };

    div()
        .id(ElementId::Name(format!("chat-msg-{index}").into()))
        .w_full()
        .flex()
        .flex_col()
        // Align user bubbles to the right, assistant to the left
        .when(is_user, |this| this.items_end())
        .when(!is_user, |this| this.items_start())
        .gap_1()
        // Message bubble
        .child(
            div()
                .max_w(px(600.))
                .text_sm()
                .line_height(px(22.))
                .px_3()
                .py_2()
                .rounded_lg()
                .when(is_user, |this| {
                    this.bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(theme.text_primary))
                })
                .when(is_assistant, |this| {
                    this.text_color(rgb(theme.text_primary))
                })
                .when(is_error, |this| {
                    this.border_1()
                        .border_color(rgb(0xc94040))
                        .bg(rgb(0x3a1515))
                        .text_color(rgb(0xc94040))
                })
                .child(msg.content.clone()),
        )
        // Tool calls
        .when(!msg.tool_calls.is_empty(), |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(msg.tool_calls.iter().enumerate().map(|(i, tc)| {
                        div()
                            .id(ElementId::Name(
                                format!("chat-msg-{index}-tool-{i}").into(),
                            ))
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.tab_bg))
                            .text_xs()
                            .font_family(FONT_MONO)
                            .text_color(rgb(theme.text_muted))
                            .child(tc.clone())
                    })),
            )
        })
        // Per-message token/transport footer on assistant messages
        .children(footer_parts.map(|(text, speed)| {
            div()
                .flex()
                .gap_1()
                .text_xs()
                .text_color(rgb(theme.text_muted))
                .pt_1()
                .child(text)
                .children(speed.map(|(label, color)| {
                    div()
                        .text_xs()
                        .text_color(rgb(color))
                        .child(label)
                }))
        }))
}

/// Format speed as "X.Y tok/s" with a color based on performance.
/// Green (≥50 tok/s), gray (20–50), red (<20).
fn format_speed(tps: f64) -> (String, u32) {
    let label = format!("{:.1} tok/s", tps);
    let color = if tps >= 50.0 {
        0x4ec96e // green — fast
    } else if tps >= 20.0 {
        0x888888 // gray — normal
    } else {
        0xc94040 // red — slow
    };
    (label, color)
}

fn format_token_count(input: u64, output: u64) -> String {
    if input == 0 && output == 0 {
        String::new()
    } else {
        format!(
            "{} in / {} out",
            format_tokens(input),
            format_tokens(output)
        )
    }
}

fn format_token_count_with_total(input: u64, output: u64) -> String {
    if input == 0 && output == 0 {
        String::new()
    } else {
        let total = input + output;
        format!(
            "{} in / {} out · {} tokens",
            format_tokens(input),
            format_tokens(output),
            format_tokens(total),
        )
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
