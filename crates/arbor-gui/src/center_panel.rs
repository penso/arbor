use super::*;

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
        let mut tabs: Vec<CenterTab> = terminals
            .iter()
            .map(|session| CenterTab::Terminal(session.id))
            .collect();
        tabs.extend(
            diff_sessions
                .iter()
                .map(|session| CenterTab::Diff(session.id)),
        );
        tabs.extend(
            file_view_sessions
                .iter()
                .map(|session| CenterTab::FileView(session.id)),
        );
        if self.logs_tab_open {
            tabs.push(CenterTab::Logs);
        }

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
        let preset_button = |kind: AgentPresetKind| {
            let text_color = theme.text_muted;
            div()
                .id(ElementId::Name(
                    format!("terminal-preset-tab-{}", kind.key()).into(),
                ))
                .cursor_pointer()
                .h(px(22.))
                .px_2()
                .flex()
                .items_center()
                .rounded_sm()
                .border_b_1()
                .border_color(rgb(theme.tab_bg))
                .text_color(rgb(text_color))
                .hover(|s| {
                    s.bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(theme.text_primary))
                })
                .child(agent_preset_button_content(kind, text_color))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.active_preset_tab = Some(kind);
                        this.launch_agent_preset(kind, window, cx);
                    }),
                )
        };
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
            .child(
                div()
                    .h(px(32.))
                    .bg(rgb(theme.tab_bg))
                    .flex()
                    .items_center()
                    .justify_between()
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
                                        let is_active = active_tab == Some(tab);
                                        let tab_count = tabs.len();
                                        let relation =
                                            active_tab_index.map(|active_index| index.cmp(&active_index));
                                        let (tab_icon, tab_label, terminal_icon) = match tab {
                                            CenterTab::Terminal(session_id) => (
                                                terminal_tab_icon_element(
                                                    is_active,
                                                    if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    },
                                                    16.0,
                                                )
                                                .into_any_element(),
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
                                                    .text_color(rgb(if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    }))
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
                                                    .text_color(rgb(if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    }))
                                                    .child(TAB_ICON_FILE)
                                                    .into_any_element(),
                                                self.file_view_sessions
                                                    .iter()
                                                    .find(|session| session.id == fv_id)
                                                    .map(|s| s.title.clone())
                                                    .unwrap_or_else(|| "file".to_owned()),
                                                false,
                                            ),
                                            CenterTab::Logs => (
                                                logs_tab_icon_element(
                                                    is_active,
                                                    if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    },
                                                    16.0,
                                                )
                                                .into_any_element(),
                                                "Logs".to_owned(),
                                                true,
                                            ),
                                        };
                                        let tab_id = match tab {
                                            CenterTab::Terminal(id) => ("center-tab-terminal", id),
                                            CenterTab::Diff(id) => ("center-tab-diff", id),
                                            CenterTab::FileView(id) => ("center-tab-fileview", id),
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
                                                    .text_color(rgb(if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    }))
                                                    .child(tab_icon),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(rgb(if is_active {
                                                        theme.text_primary
                                                    } else {
                                                        theme.text_muted
                                                    }))
                                                    .child(tab_label),
                                            )
                                            .child(
                                                div()
                                                    .id(match tab {
                                                        CenterTab::Terminal(id) => ("tab-close-terminal", id),
                                                        CenterTab::Diff(id) => ("tab-close-diff", id),
                                                        CenterTab::FileView(id) => ("tab-close-fileview", id),
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
                                                    .child("×")
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
                                                    if index == 0 { el } else { el.border_l_1() }
                                                },
                                                Some(std::cmp::Ordering::Less) => {
                                                    let el = this.border_b_1();
                                                    if index == 0 { el } else { el.border_l_1() }
                                                },
                                                Some(std::cmp::Ordering::Greater) => {
                                                    this.border_r_1().border_b_1()
                                                },
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
                                                CenterTab::Logs => {
                                                    this.logs_tab_active = true;
                                                    this.active_diff_session_id = None;
                                                    this.sync_navigation_ui_state_store(cx);
                                                    cx.notify();
                                                },
                                            }))
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
                                                cx.listener(
                                                    |this, _: &MouseDownEvent, window, cx| {
                                                        this.spawn_terminal_session(window, cx)
                                                    },
                                                ),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .border_l_1()
                            .border_color(rgb(theme.border))
                            .border_b_1()
                            .children(
                                AgentPresetKind::ORDER
                                    .iter()
                                    .copied()
                                    .filter(|kind| installed_preset_kinds().contains(kind))
                                    .map(&preset_button),
                            )
                            .children(self.repo_presets.iter().enumerate().map(|(index, preset)| {
                                let icon_text = preset.icon.clone();
                                let name_text = preset.name.clone();
                                div()
                                    .id(ElementId::Name(
                                        format!("terminal-repo-preset-tab-{index}").into(),
                                    ))
                                    .cursor_pointer()
                                    .h(px(22.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded_sm()
                                    .border_b_1()
                                    .border_color(rgb(theme.tab_bg))
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
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .line_height(px(14.))
                                                    .child(if icon_text.is_empty() {
                                                        "\u{f013}".to_owned()
                                                    } else {
                                                        icon_text
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .line_height(px(14.))
                                                    .child(name_text),
                                            ),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                            this.launch_repo_preset(index, window, cx);
                                        }),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                            this.open_manage_repo_presets_modal(Some(index), cx);
                                        }),
                                    )
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(rgb(theme.terminal_bg))
                    .when(
                        active_terminal.is_none() && active_diff_session.is_none() && active_file_view_session.is_none() && active_tab != Some(CenterTab::Logs),
                        |this| {
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
                        },
                    )
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
                    .when(active_tab == Some(CenterTab::Logs), |this| {
                        this.child(self.render_logs_content(cx))
                    }),
            )
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
