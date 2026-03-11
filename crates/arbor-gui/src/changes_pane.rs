impl ArborWindow {
    fn render_right_pane(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let content: Div = match self.right_pane_tab {
            RightPaneTab::Changes => self.render_changes_content(cx),
            RightPaneTab::FileTree => self.render_file_tree(cx),
            RightPaneTab::Notes => self.render_notes_content(cx),
        };
        let search_active = self.right_pane_search_active;
        let search_text = self.right_pane_search.clone();
        let show_search = self.right_pane_tab != RightPaneTab::Notes;

        div()
            .w(px(self.right_pane_width))
            .h_full()
            .min_h_0()
            .bg(rgb(theme.sidebar_bg))
            .flex()
            .flex_col()
            .child(self.render_right_pane_tabs(cx))
            .when(show_search, |this| this.child(
                div()
                    .id("right-pane-search")
                    .h(px(28.))
                    .mx_1()
                    .my(px(4.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(if search_active {
                        theme.accent
                    } else {
                        theme.border
                    }))
                    .bg(rgb(theme.panel_bg))
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.right_pane_search_active = true;
                            this.right_pane_search_cursor = char_count(&this.right_pane_search);
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_xs()
                            .min_w_0()
                            .flex_1()
                            .child(if search_active {
                                if search_text.is_empty() {
                                    active_input_display(
                                        theme,
                                        "",
                                        "Filter files…",
                                        theme.text_disabled,
                                        self.right_pane_search_cursor,
                                        28,
                                    )
                                } else {
                                    active_input_display(
                                        theme,
                                        &search_text,
                                        "Filter files…",
                                        theme.text_primary,
                                        self.right_pane_search_cursor,
                                        28,
                                    )
                                }
                            } else if search_text.is_empty() {
                                div()
                                    .text_color(rgb(theme.text_disabled))
                                    .child("Filter files…")
                                    .into_any_element()
                            } else {
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(rgb(theme.text_primary))
                                    .child(search_text)
                                    .into_any_element()
                            }),
                    ),
            ))
            .child(content)
    }

    fn render_right_pane_tabs(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let active_tab = self.right_pane_tab;

        let tab_button = |label: &'static str, tab: RightPaneTab| {
            let is_active = active_tab == tab;
            div()
                .id(ElementId::Name(
                    format!("right-tab-{label}").to_lowercase().into(),
                ))
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_xs()
                .font_family(FONT_UI)
                .bg(rgb(if is_active {
                    theme.tab_active_bg
                } else {
                    theme.tab_bg
                }))
                .text_color(rgb(if is_active {
                    theme.text_primary
                } else {
                    theme.text_muted
                }))
                .when(is_active, |this| {
                    this.border_b_2().border_color(rgb(theme.accent))
                })
                .when(!is_active, |this| {
                    this.hover(|this| {
                        this.bg(rgb(theme.panel_active_bg))
                            .text_color(rgb(theme.text_primary))
                    })
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        this.set_right_pane_tab(tab, cx);
                    }),
                )
                .child(label)
        };

        div()
            .h(px(28.))
            .flex()
            .flex_row()
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(tab_button("Changes", RightPaneTab::Changes))
            .child(tab_button("Files", RightPaneTab::FileTree))
            .child(tab_button("Notes", RightPaneTab::Notes))
    }

    fn render_changes_content(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let selected_path = self.selected_changed_file.clone();
        let can_run_actions = self.can_run_local_git_actions();
        let is_busy = self.git_action_in_flight.is_some();
        let commit_enabled = can_run_actions && !is_busy && !self.changed_files.is_empty();
        let stacked_pr_enabled = can_run_actions && !is_busy && !self.changed_files.is_empty();
        let push_enabled = can_run_actions && !is_busy;
        let pr_enabled = can_run_actions && !is_busy;
        let search_lower = self.right_pane_search.to_lowercase();
        let filtered_changes: Vec<_> = self
            .changed_files
            .iter()
            .filter(|change| {
                search_lower.is_empty()
                    || change
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&search_lower)
            })
            .cloned()
            .collect();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(32.))
                    .px_1()
                    .gap_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                git_action_button(
                                    theme,
                                    "changes-action-commit",
                                    GIT_ACTION_ICON_COMMIT,
                                    "Commit",
                                    commit_enabled,
                                    self.git_action_in_flight == Some(GitActionKind::Commit),
                                )
                                .when(commit_enabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.open_commit_modal(cx);
                                    }))
                                }),
                            )
                            .child(
                                git_action_button(
                                    theme,
                                    "changes-action-stacked-pr",
                                    GIT_ACTION_ICON_PR,
                                    "Ship PR",
                                    stacked_pr_enabled,
                                    self.git_action_in_flight
                                        == Some(GitActionKind::CommitPushCreatePullRequest),
                                )
                                .when(stacked_pr_enabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.open_commit_modal(cx);
                                    }))
                                }),
                            )
                            .child(
                                git_action_button(
                                    theme,
                                    "changes-action-push",
                                    GIT_ACTION_ICON_PUSH,
                                    "Push",
                                    push_enabled,
                                    self.git_action_in_flight == Some(GitActionKind::Push),
                                )
                                .when(push_enabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.run_push_action(cx);
                                    }))
                                }),
                            )
                            .child(
                                git_action_button(
                                    theme,
                                    "changes-action-pr",
                                    GIT_ACTION_ICON_PR,
                                    "Create PR",
                                    pr_enabled,
                                    self.git_action_in_flight
                                        == Some(GitActionKind::CreatePullRequest),
                                )
                                .when(pr_enabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.run_create_pr_action(cx);
                                    }))
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("changes-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .scrollbar_width(px(10.))
                    .flex()
                    .flex_col()
                    .font_family(FONT_MONO)
                    .p_1()
                    .children(filtered_changes.iter().map(|change| {
                        let is_selected = selected_path
                            .as_ref()
                            .is_some_and(|selected| selected.as_path() == change.path.as_path());
                        let status_color = match change.kind {
                            ChangeKind::Added => 0xa6e3a1,
                            ChangeKind::Modified => 0xf9e2af,
                            ChangeKind::Removed => 0xf38ba8,
                            ChangeKind::Renamed => 0x89dceb,
                            ChangeKind::Copied => 0x74c7ec,
                            ChangeKind::TypeChange => 0xcba6f7,
                            ChangeKind::Conflict => 0xf38ba8,
                            ChangeKind::IntentToAdd => 0x94e2d5,
                        };
                        let path_color = match change.kind {
                            ChangeKind::Added => 0x8fd7ad,
                            ChangeKind::Removed => 0xf2a4b7,
                            ChangeKind::Modified => 0xd9d7cf,
                            ChangeKind::Renamed => 0x8ecae6,
                            ChangeKind::Copied => 0x91d7e3,
                            ChangeKind::TypeChange => 0xc4b1ee,
                            ChangeKind::Conflict => 0xf38ba8,
                            ChangeKind::IntentToAdd => 0x94e2d5,
                        };
                        let display_path = truncate_middle_path_for_width(
                            change.path.as_path(),
                            self.right_pane_width,
                        );
                        let file_path = change.path.clone();

                        div()
                            .id(ElementId::Name(
                                format!("changed-file-{}", display_path).into(),
                            ))
                            .h(px(24.))
                            .pl(px(4.))
                            .pr_1()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when(is_selected, |this| this.bg(rgb(theme.panel_active_bg)))
                            .when(!is_selected, |this| {
                                this.hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                    this.select_changed_file(file_path.clone(), cx);
                                    this.open_diff_tab_for_selected_file(cx);
                                }),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.))
                                    .text_color(rgb(status_color))
                                    .child(change_code(change.kind)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_xs()
                                    .text_color(rgb(path_color))
                                    .child(display_path),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .gap_1()
                                    .when(change.additions > 0, |this| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0x72d69c))
                                                .child(format!("+{}", change.additions)),
                                        )
                                    })
                                    .when(change.deletions > 0, |this| {
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(0xeb6f92))
                                                .child(format!("-{}", change.deletions)),
                                        )
                                    }),
                            )
                    })),
            )
    }

    fn render_notes_content(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();

        let Some(notes_path) = self.worktree_notes_path.clone() else {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(theme.text_muted))
                        .child("Notes are available for local worktrees."),
                );
        };

        let cursor_line = self
            .worktree_notes_cursor
            .line
            .min(self.worktree_notes_lines.len().saturating_sub(1));
        let cursor_col = self.worktree_notes_cursor.col;
        let notes_active = self.worktree_notes_active;
        let notes_error = self.worktree_notes_error.clone();
        let notes_path_label = notes_path
            .strip_prefix(self.selected_local_worktree_path().unwrap_or(notes_path.as_path()))
            .unwrap_or(notes_path.as_path())
            .display()
            .to_string();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(notes_path_label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if notes_error.is_some() {
                                0xeb6f92
                            } else {
                                theme.text_disabled
                            }))
                            .child(notes_error.unwrap_or_else(|| "Autosaves on edit".to_owned())),
                    ),
            )
            .child(
                div()
                    .id("worktree-notes-editor")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .cursor_text()
                    .bg(rgb(theme.panel_bg))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            this.worktree_notes_active = true;
                            if this.worktree_notes_lines.is_empty() {
                                this.worktree_notes_lines.push(String::new());
                            }
                            let last_line = this.worktree_notes_lines.len().saturating_sub(1);
                            this.worktree_notes_cursor.line =
                                this.worktree_notes_cursor.line.min(last_line);
                            this.worktree_notes_cursor.col = this.worktree_notes_cursor.col.min(
                                this.worktree_notes_lines[this.worktree_notes_cursor.line]
                                    .chars()
                                    .count(),
                            );
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .min_h_full()
                            .px_2()
                            .py_2()
                            .font_family(FONT_MONO)
                            .text_xs()
                            .children(self.worktree_notes_lines.iter().enumerate().map(
                                |(line_index, line)| {
                                    let is_cursor_line = notes_active && line_index == cursor_line;
                                    let line_len = line.chars().count();
                                    let cursor_col = cursor_col.min(line_len);
                                    let before = if is_cursor_line {
                                        line.chars().take(cursor_col).collect::<String>()
                                    } else {
                                        String::new()
                                    };
                                    let after = if is_cursor_line {
                                        line.chars().skip(cursor_col).collect::<String>()
                                    } else {
                                        String::new()
                                    };

                                    div()
                                        .min_h(px(18.))
                                        .flex()
                                        .items_start()
                                        .child(
                                            div()
                                                .w(px(28.))
                                                .flex_none()
                                                .text_color(rgb(theme.text_disabled))
                                                .child(format!("{:>2}", line_index + 1)),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .w_full()
                                                .text_color(rgb(theme.text_primary))
                                                .when(is_cursor_line, |this| {
                                                    this.flex()
                                                        .flex_wrap()
                                                        .w_full()
                                                        .items_center()
                                                        .child(before)
                                                        .child(input_caret(theme).flex_none())
                                                        .child(after)
                                                })
                                                .when(!is_cursor_line, |this| {
                                                    if line.is_empty() {
                                                        this.child(" ")
                                                    } else {
                                                        this.flex().flex_wrap().w_full().child(line.clone())
                                                    }
                                                }),
                                        )
                                },
                            )),
                    ),
            )
    }

    fn render_file_tree(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let selected_entry = self.selected_file_tree_entry.clone();
        let expanded_dirs = &self.expanded_dirs;
        let search_lower = self.right_pane_search.to_lowercase();
        let is_filtering = !search_lower.is_empty();
        let filtered_entries: Vec<_> = self
            .file_tree_entries
            .iter()
            .filter(|entry| {
                if !is_filtering {
                    return true;
                }
                if entry.is_dir {
                    return false;
                }
                entry
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&search_lower)
            })
            .collect();

        div().flex_1().min_h_0().flex().flex_col().child(
            div()
                .id("file-tree-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .scrollbar_width(px(10.))
                .flex()
                .flex_col()
                .font_family(FONT_MONO)
                .p_1()
                .children(filtered_entries.iter().map(|entry| {
                    let is_selected = selected_entry
                        .as_ref()
                        .is_some_and(|selected| selected == &entry.path);
                    let indent = if is_filtering {
                        4.
                    } else {
                        entry.depth as f32 * 16. + 4.
                    };
                    let entry_path = entry.path.clone();
                    let is_dir = entry.is_dir;

                    let chevron = if is_dir {
                        if expanded_dirs.contains(&entry.path) {
                            "\u{f078}" // chevron down
                        } else {
                            "\u{f054}" // chevron right
                        }
                    } else {
                        " "
                    };

                    let (file_icon, icon_color) = file_icon_and_color(&entry.name, is_dir);

                    div()
                        .id(ElementId::Name(
                            format!("ft-{}", entry.path.display()).into(),
                        ))
                        .h(px(24.))
                        .pl(px(indent))
                        .pr_1()
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap_1()
                        .bg(rgb(if is_selected {
                            theme.panel_active_bg
                        } else {
                            theme.sidebar_bg
                        }))
                        .when(!is_selected, |this| {
                            this.hover(|this| this.bg(rgb(theme.panel_active_bg)))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                                if is_dir {
                                    this.toggle_file_tree_dir(entry_path.clone(), cx);
                                } else {
                                    this.select_file_tree_entry(entry_path.clone(), cx);
                                }
                            }),
                        )
                        .child(
                            div()
                                .w(px(12.))
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(rgb(theme.text_muted))
                                .child(chevron),
                        )
                        .child(
                            div()
                                .w(px(20.))
                                .flex_none()
                                .text_size(px(18.))
                                .text_color(rgb(icon_color))
                                .child(file_icon),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_xs()
                                .text_color(rgb(icon_color))
                                .when(is_dir, |this| this.font_weight(FontWeight::SEMIBOLD))
                                .child(if is_filtering {
                                    entry.path.to_string_lossy().into_owned()
                                } else {
                                    entry.name.clone()
                                }),
                        )
                })),
        )
    }
}
