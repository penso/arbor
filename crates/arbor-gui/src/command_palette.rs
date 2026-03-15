use super::*;

impl ArborWindow {
    pub(crate) fn open_command_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette_modal = Some(CommandPaletteModal {
            scope: CommandPaletteScope::Actions,
            query: String::new(),
            query_cursor: 0,
            selected_index: 0,
        });
        self.command_palette_scroll_handle.scroll_to_item(0);
        self.refresh_repo_config_if_changed(cx);
        cx.notify();
    }

    pub(crate) fn close_command_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette_modal = None;
        cx.notify();
    }

    pub(crate) fn command_palette_action_items(&self) -> Vec<CommandPaletteItem> {
        let mut items = vec![
            CommandPaletteItem {
                title: "New Worktree".to_owned(),
                subtitle: "Create a local worktree".to_owned(),
                search_text: "new worktree create".to_owned(),
                action: CommandPaletteAction::OpenCreateWorktree,
            },
            CommandPaletteItem {
                title: "Review Pull Request".to_owned(),
                subtitle: "Create a worktree from a GitHub PR".to_owned(),
                search_text: "review pull request pr github worktree".to_owned(),
                action: CommandPaletteAction::OpenReviewPullRequest,
            },
            CommandPaletteItem {
                title: "View Issues".to_owned(),
                subtitle: "Browse repository issues in the command palette".to_owned(),
                search_text: "issues github gitlab linear tickets bugs".to_owned(),
                action: CommandPaletteAction::BrowseIssues,
            },
            CommandPaletteItem {
                title: "Refresh Worktrees".to_owned(),
                subtitle: "Reload repositories and worktrees".to_owned(),
                search_text: "refresh worktrees reload repos".to_owned(),
                action: CommandPaletteAction::RefreshWorktrees,
            },
            CommandPaletteItem {
                title: if self.compact_sidebar {
                    "Disable Compact Sidebar".to_owned()
                } else {
                    "Enable Compact Sidebar".to_owned()
                },
                subtitle: "Toggle dense sidebar rows".to_owned(),
                search_text: "compact sidebar list dense toggle".to_owned(),
                action: CommandPaletteAction::ToggleCompactSidebar,
            },
            CommandPaletteItem {
                title: "Open Settings".to_owned(),
                subtitle: "Edit Arbor settings".to_owned(),
                search_text: "settings preferences config".to_owned(),
                action: CommandPaletteAction::OpenSettings,
            },
            CommandPaletteItem {
                title: "Choose Theme".to_owned(),
                subtitle: "Switch the application theme".to_owned(),
                search_text: "theme appearance colors".to_owned(),
                action: CommandPaletteAction::OpenThemePicker,
            },
        ];

        for mode in ExecutionMode::ORDER {
            items.push(CommandPaletteItem {
                title: format!("Execution Mode: {}", mode.label()),
                subtitle: format!(
                    "{}{}",
                    mode.subtitle(),
                    if self.execution_mode == mode {
                        " · Active"
                    } else {
                        ""
                    }
                ),
                search_text: format!(
                    "execution mode {} {}",
                    mode.label().to_ascii_lowercase(),
                    mode.subtitle().to_ascii_lowercase()
                ),
                action: CommandPaletteAction::SetExecutionMode(mode),
            });
        }

        for preset in AgentPresetKind::ORDER.iter().copied() {
            items.push(CommandPaletteItem {
                title: format!("Launch {}", preset.label()),
                subtitle: "Start an agent terminal".to_owned(),
                search_text: format!("launch agent {}", preset.label().to_ascii_lowercase()),
                action: CommandPaletteAction::LaunchAgentPreset(preset),
            });
        }

        for (index, preset) in self.repo_presets.iter().enumerate() {
            items.push(CommandPaletteItem {
                title: format!("Run {}", preset.name),
                subtitle: "Run repo preset".to_owned(),
                search_text: format!(
                    "preset repo {} {}",
                    preset.name.to_ascii_lowercase(),
                    preset.command.to_ascii_lowercase()
                ),
                action: CommandPaletteAction::LaunchRepoPreset(index),
            });
        }

        for (index, repository) in self.repositories.iter().enumerate() {
            items.push(CommandPaletteItem {
                title: repository.label.clone(),
                subtitle: "Repository".to_owned(),
                search_text: format!(
                    "repository repo {} {}",
                    repository.label.to_ascii_lowercase(),
                    repository.root.display()
                ),
                action: CommandPaletteAction::SelectRepository(index),
            });
        }

        for (index, worktree) in self.worktrees.iter().enumerate() {
            items.push(CommandPaletteItem {
                title: worktree.label.clone(),
                subtitle: format!("Worktree · {}", worktree.branch),
                search_text: format!(
                    "worktree {} {} {}",
                    worktree.label.to_ascii_lowercase(),
                    worktree.branch.to_ascii_lowercase(),
                    worktree.path.display()
                ),
                action: CommandPaletteAction::SelectWorktree(index),
            });
        }

        for task in self.command_palette_task_templates.clone() {
            let agent_label = task
                .agent
                .map(|agent| agent.label().to_owned())
                .unwrap_or_else(|| "Default agent".to_owned());
            items.push(CommandPaletteItem {
                title: task.name.clone(),
                subtitle: format!(
                    "Task · {} · {}",
                    repository_display_name(&task.repo_root),
                    agent_label
                ),
                search_text: format!(
                    "task {} {} {} {}",
                    task.name.to_ascii_lowercase(),
                    task.description.to_ascii_lowercase(),
                    task.prompt.to_ascii_lowercase(),
                    task.path.display()
                ),
                action: CommandPaletteAction::LaunchTaskTemplate(task),
            });
        }

        items
    }

    pub(crate) fn command_palette_issue_items(&self) -> Vec<CommandPaletteItem> {
        let Some(target) = self.issue_target_for_current_selection() else {
            return Vec::new();
        };

        self.issue_list_state(&target)
            .map(|state| state.issues.iter())
            .into_iter()
            .flatten()
            .map(|issue| {
                let mut subtitle_parts = vec![issue.state.clone()];
                if let Some(review) = issue.linked_review.as_ref() {
                    subtitle_parts.push(review.label.clone());
                }
                if let Some(branch) = issue.linked_branch.as_ref() {
                    subtitle_parts.push(branch.clone());
                }
                if subtitle_parts.len() == 1 {
                    subtitle_parts.push("Create worktree".to_owned());
                }

                CommandPaletteItem {
                    title: format!("{} {}", issue.display_id, issue.title),
                    subtitle: subtitle_parts.join(" · "),
                    search_text: format!(
                        "issue issues {} {} {} {} {} {} {}",
                        issue.display_id.to_ascii_lowercase(),
                        issue.id.to_ascii_lowercase(),
                        issue.title.to_ascii_lowercase(),
                        issue.state.to_ascii_lowercase(),
                        issue.suggested_worktree_name.to_ascii_lowercase(),
                        issue
                            .linked_branch
                            .as_deref()
                            .unwrap_or_default()
                            .to_ascii_lowercase(),
                        issue
                            .linked_review
                            .as_ref()
                            .map(|review| review.label.to_ascii_lowercase())
                            .unwrap_or_default(),
                    ),
                    action: CommandPaletteAction::OpenIssueCreateModal(Box::new(issue.clone())),
                }
            })
            .collect()
    }

    pub(crate) fn filtered_command_palette_items(&self) -> Vec<CommandPaletteItem> {
        let Some(modal) = self.command_palette_modal.as_ref() else {
            return Vec::new();
        };

        let query = modal.query.trim().to_ascii_lowercase();
        let items = match modal.scope {
            CommandPaletteScope::Actions => self.command_palette_action_items(),
            CommandPaletteScope::Issues => self.command_palette_issue_items(),
        };

        if query.is_empty() {
            return items;
        }

        let mut matches = items
            .into_iter()
            .filter_map(|item| {
                command_palette_match_score(&item, &query)
                    .map(|score| (self.command_palette_sort_rank(&item), score, item))
            })
            .collect::<Vec<_>>();
        matches.sort_by(
            |(left_rank, left_score, left_item), (right_rank, right_score, right_item)| {
                left_rank
                    .cmp(right_rank)
                    .then_with(|| left_score.cmp(right_score))
                    .then_with(|| {
                        left_item
                            .title
                            .to_ascii_lowercase()
                            .cmp(&right_item.title.to_ascii_lowercase())
                    })
            },
        );
        matches.into_iter().map(|(_, _, item)| item).collect()
    }

    pub(crate) fn command_palette_sort_rank(&self, item: &CommandPaletteItem) -> (usize, usize) {
        (
            self.command_palette_recent_rank(item),
            self.command_palette_context_rank(item),
        )
    }

    pub(crate) fn command_palette_recent_rank(&self, item: &CommandPaletteItem) -> usize {
        let action_key = command_palette_action_key(item);
        self.command_palette_recent_actions
            .iter()
            .position(|recent| recent == &action_key)
            .unwrap_or(usize::MAX)
    }

    pub(crate) fn command_palette_context_rank(&self, item: &CommandPaletteItem) -> usize {
        match &item.action {
            CommandPaletteAction::SelectWorktree(index) => {
                if self.active_worktree_index == Some(*index) {
                    0
                } else {
                    2
                }
            },
            CommandPaletteAction::SelectRepository(index) => {
                if self.active_repository_index == Some(*index) {
                    0
                } else {
                    2
                }
            },
            CommandPaletteAction::OpenIssueCreateModal(_) => 0,
            CommandPaletteAction::LaunchRepoPreset(_) => 1,
            CommandPaletteAction::LaunchTaskTemplate(task) => self
                .active_repository_index
                .and_then(|index| self.repositories.get(index))
                .map(|repository| usize::from(repository.root != task.repo_root) + 1)
                .unwrap_or(2),
            CommandPaletteAction::LaunchAgentPreset(kind) => {
                usize::from(self.active_preset_tab != Some(*kind)) + 1
            },
            CommandPaletteAction::OpenCreateWorktree
            | CommandPaletteAction::BrowseIssues
            | CommandPaletteAction::OpenReviewPullRequest
            | CommandPaletteAction::RefreshWorktrees
            | CommandPaletteAction::ToggleCompactSidebar
            | CommandPaletteAction::OpenSettings
            | CommandPaletteAction::OpenThemePicker
            | CommandPaletteAction::SetExecutionMode(_) => 3,
        }
    }

    pub(crate) fn show_command_palette_issues(&mut self, cx: &mut Context<Self>) {
        if let Some(modal) = self.command_palette_modal.as_mut() {
            modal.scope = CommandPaletteScope::Issues;
            modal.query.clear();
            modal.query_cursor = 0;
            modal.selected_index = 0;
        }
        self.command_palette_scroll_handle.scroll_to_item(0);

        if let Some(target) = self.issue_target_for_current_selection() {
            if !self.command_palette_has_issue_snapshot_for_current_selection() {
                self.ensure_issues_loaded_for_target(target, cx);
            } else {
                cx.notify();
            }
        } else {
            cx.notify();
        }
    }

    pub(crate) fn command_palette_has_issue_snapshot_for_current_selection(&self) -> bool {
        let Some(target) = self.issue_target_for_current_selection() else {
            return false;
        };

        self.issue_list_state(&target).is_some_and(|state| {
            state.loaded
                || !state.issues.is_empty()
                || state.source.is_some()
                || state.notice.is_some()
                || state.error.is_some()
        })
    }

    pub(crate) fn remember_command_palette_action(&mut self, item: &CommandPaletteItem) {
        let action_key = command_palette_action_key(item);
        self.command_palette_recent_actions
            .retain(|recent| recent != &action_key);
        self.command_palette_recent_actions.insert(0, action_key);
        self.command_palette_recent_actions.truncate(20);
    }

    pub(crate) fn execute_command_palette_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected_index = self
            .command_palette_modal
            .as_ref()
            .map(|modal| modal.selected_index)
            .unwrap_or(0);
        let items = self.filtered_command_palette_items();
        let Some(item) = items.get(selected_index).cloned() else {
            return;
        };

        self.remember_command_palette_action(&item);
        match item.action {
            CommandPaletteAction::BrowseIssues => {
                self.show_command_palette_issues(cx);
                return;
            },
            CommandPaletteAction::OpenCreateWorktree => {
                self.command_palette_modal = None;
                let repo_index = self.active_repository_index.unwrap_or(0);
                self.open_create_modal(repo_index, CreateModalTab::LocalWorktree, cx);
            },
            CommandPaletteAction::OpenReviewPullRequest => {
                self.command_palette_modal = None;
                let repo_index = self.active_repository_index.unwrap_or(0);
                self.open_create_modal(repo_index, CreateModalTab::ReviewPullRequest, cx);
            },
            CommandPaletteAction::RefreshWorktrees => {
                self.command_palette_modal = None;
                self.refresh_worktrees(cx);
            },
            CommandPaletteAction::ToggleCompactSidebar => {
                self.command_palette_modal = None;
                self.compact_sidebar = !self.compact_sidebar;
                self.notice = Some(if self.compact_sidebar {
                    "compact sidebar enabled".to_owned()
                } else {
                    "compact sidebar disabled".to_owned()
                });
                self.sync_ui_state_store(window, cx);
            },
            CommandPaletteAction::OpenSettings => {
                self.command_palette_modal = None;
                self.open_settings_modal(cx);
            },
            CommandPaletteAction::OpenThemePicker => {
                self.command_palette_modal = None;
                self.open_theme_picker_modal(cx);
            },
            CommandPaletteAction::SetExecutionMode(mode) => {
                self.command_palette_modal = None;
                self.set_execution_mode(mode, window, cx);
            },
            CommandPaletteAction::LaunchAgentPreset(preset) => {
                self.command_palette_modal = None;
                self.launch_agent_preset(preset, window, cx);
            },
            CommandPaletteAction::LaunchRepoPreset(index) => {
                self.command_palette_modal = None;
                self.launch_repo_preset(index, window, cx);
            },
            CommandPaletteAction::SelectRepository(index) => {
                self.command_palette_modal = None;
                self.select_repository(index, cx);
            },
            CommandPaletteAction::SelectWorktree(index) => {
                self.command_palette_modal = None;
                self.select_worktree(index, window, cx);
            },
            CommandPaletteAction::OpenIssueCreateModal(issue) => {
                self.command_palette_modal = None;
                self.open_issue_create_modal(*issue, cx);
            },
            CommandPaletteAction::LaunchTaskTemplate(task) => {
                self.command_palette_modal = None;
                self.launch_task_template(&task, window, cx);
            },
        }
        cx.notify();
    }

    pub(crate) fn move_command_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let item_count = self.filtered_command_palette_items().len();
        let Some(modal) = self.command_palette_modal.as_mut() else {
            return;
        };
        if item_count == 0 {
            modal.selected_index = 0;
            cx.notify();
            return;
        }

        let current = modal.selected_index.min(item_count - 1) as isize;
        let next = (current + delta).rem_euclid(item_count as isize) as usize;
        modal.selected_index = next;
        self.command_palette_scroll_handle.scroll_to_item(next);
        cx.notify();
    }

    pub(crate) fn launch_task_template(
        &mut self,
        task: &TaskTemplate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_index) = self
            .repositories
            .iter()
            .position(|repository| repository.root == task.repo_root)
        else {
            self.notice = Some(format!(
                "task repository is not available: {}",
                task.repo_root.display()
            ));
            cx.notify();
            return;
        };

        if self.active_repository_index != Some(repo_index) {
            self.select_repository(repo_index, cx);
        }

        let group_key = self
            .repositories
            .get(repo_index)
            .map(|repository| repository.group_key.clone());
        let worktree_index = group_key.and_then(|group_key| {
            self.worktrees
                .iter()
                .position(|worktree| worktree.group_key == group_key)
        });
        let Some(worktree_index) = worktree_index else {
            self.notice = Some(format!(
                "no worktree is available for {}",
                repository_display_name(&task.repo_root)
            ));
            cx.notify();
            return;
        };

        if self.active_worktree_index != Some(worktree_index) {
            self.select_worktree(worktree_index, window, cx);
        }

        let preset = task
            .agent
            .unwrap_or_else(|| self.selected_agent_preset_or_default());
        let command = self.preset_command_for_kind(preset).trim().to_owned();
        if command.is_empty() {
            self.notice = Some(format!("{} preset command is empty", preset.label()));
            cx.notify();
            return;
        }

        let invocation =
            match prompt_terminal_invocation(preset, &command, &task.prompt, self.execution_mode) {
                Ok(invocation) => format!("{invocation}\n"),
                Err(error) => {
                    self.notice = Some(format!("failed to run task {}: {error}", task.name));
                    cx.notify();
                    return;
                },
            };
        let terminal_count_before = self.terminals.len();
        self.spawn_terminal_session(window, cx);
        if self.terminals.len() <= terminal_count_before {
            return;
        }

        let Some(session_id) = self.terminals.last().map(|session| session.id) else {
            return;
        };

        if let Err(error) = self.write_input_to_terminal(session_id, invocation.as_bytes()) {
            self.notice = Some(format!("failed to run task {}: {error}", task.name));
            cx.notify();
            return;
        }

        if let Some(session) = self
            .terminals
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.agent_preset = Some(preset);
            session.execution_mode = Some(self.execution_mode);
            session.last_command = Some(invocation.trim().to_owned());
            session.pending_command.clear();
            session.updated_at_unix_ms = current_unix_timestamp_millis();
        }

        self.notice = Some(format!("launched task {}", task.name));
        self.sync_daemon_session_store(cx);
        cx.notify();
    }

    pub(crate) fn render_command_palette_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.command_palette_modal.clone() else {
            return div();
        };
        let theme = self.theme();
        let items = self.filtered_command_palette_items();
        let item_count = items.len();
        let empty_message = self.command_palette_empty_message(&modal);
        let selected_index = if items.is_empty() {
            0
        } else {
            modal.selected_index.min(items.len() - 1)
        };
        let mut list = div()
            .id("command-palette-results")
            .w_full()
            .max_h(px(COMMAND_PALETTE_MAX_HEIGHT_PX))
            .pr(px(18.))
            .overflow_y_scroll()
            .scrollbar_width(px(10.))
            .track_scroll(&self.command_palette_scroll_handle)
            .flex()
            .flex_col();

        if items.is_empty() {
            list = list.child(
                div()
                    .px_3()
                    .py_3()
                    .text_sm()
                    .text_color(rgb(theme.text_muted))
                    .child(empty_message),
            );
        }

        list = list.children(items.into_iter().enumerate().map(|(index, item)| {
            let is_selected = index == selected_index;
            div()
                .id(("command-palette-item", index))
                .cursor_pointer()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_3()
                .bg(rgb(if is_selected {
                    theme.panel_active_bg
                } else {
                    theme.sidebar_bg
                }))
                .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                    if let Some(modal) = this.command_palette_modal.as_mut()
                        && modal.selected_index != index
                    {
                        modal.selected_index = index;
                        this.command_palette_scroll_handle.scroll_to_item(index);
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(modal) = this.command_palette_modal.as_mut() {
                        modal.selected_index = index;
                    }
                    this.execute_command_palette_selection(window, cx);
                }))
                .child(command_palette_icon(&item.action, theme))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(theme.text_primary))
                                .child(item.title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_muted))
                                .child(item.subtitle),
                        ),
                )
        }));

        let mut results = div().relative().child(list);
        if let Some(scrollbar) =
            command_palette_scrollbar_indicator(theme, item_count, selected_index)
        {
            results = results.child(scrollbar);
        }

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(72.))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_command_palette(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(640.))
                    .max_w(px(640.))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(div().font_family(FONT_UI).text_sm().child(
                                active_input_display(
                                    theme,
                                    &modal.query,
                                    match modal.scope {
                                        CommandPaletteScope::Actions => {
                                            "Search actions, worktrees, presets..."
                                        },
                                        CommandPaletteScope::Issues => "Search issues...",
                                    },
                                    theme.text_primary,
                                    modal.query_cursor,
                                    72,
                                ),
                            ))
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child(match modal.scope {
                                        CommandPaletteScope::Actions => {
                                            format!("{item_count} results")
                                        },
                                        CommandPaletteScope::Issues => {
                                            format!("{item_count} issues")
                                        },
                                    }),
                            ),
                    )
                    .child(results),
            )
    }
}

impl ArborWindow {
    pub(crate) fn command_palette_empty_message(&self, modal: &CommandPaletteModal) -> String {
        match modal.scope {
            CommandPaletteScope::Actions => "No results".to_owned(),
            CommandPaletteScope::Issues => {
                if self.issue_target_for_current_selection().is_none() {
                    if self.active_outpost_index.is_some() {
                        "Issues are unavailable for SSH outposts.".to_owned()
                    } else {
                        "Select a daemon-backed repository to browse issues.".to_owned()
                    }
                } else {
                    let Some(target) = self.issue_target_for_current_selection() else {
                        return "Select a daemon-backed repository to browse issues.".to_owned();
                    };
                    let Some(state) = self.issue_list_state(&target) else {
                        return "Loading issues…".to_owned();
                    };

                    if state.loading && state.issues.is_empty() {
                        "Loading issues…".to_owned()
                    } else if let Some(error) = state.error.as_ref() {
                        error.clone()
                    } else if let Some(notice) = state.notice.as_ref() {
                        notice.clone()
                    } else if modal.query.trim().is_empty() {
                        "No issues found for this repository.".to_owned()
                    } else {
                        "No matching issues.".to_owned()
                    }
                }
            },
        }
    }
}

pub(crate) fn command_palette_icon(
    action: &CommandPaletteAction,
    theme: ThemePalette,
) -> AnyElement {
    match action {
        CommandPaletteAction::OpenCreateWorktree => {
            command_palette_glyph_icon("\u{f055}", 0x98c379, theme)
        },
        CommandPaletteAction::BrowseIssues | CommandPaletteAction::OpenIssueCreateModal(_) => {
            command_palette_glyph_icon("\u{f145}", 0x61afef, theme)
        },
        CommandPaletteAction::OpenReviewPullRequest => {
            command_palette_glyph_icon("\u{f0ea}", 0xc678dd, theme)
        },
        CommandPaletteAction::RefreshWorktrees => {
            command_palette_glyph_icon("\u{f021}", 0x61afef, theme)
        },
        CommandPaletteAction::ToggleCompactSidebar => {
            command_palette_glyph_icon("\u{f03a}", 0x56b6c2, theme)
        },
        CommandPaletteAction::OpenSettings => {
            command_palette_glyph_icon("\u{f013}", 0xd19a66, theme)
        },
        CommandPaletteAction::OpenThemePicker => {
            command_palette_glyph_icon("\u{f53f}", 0xc678dd, theme)
        },
        CommandPaletteAction::SetExecutionMode(mode) => match mode {
            ExecutionMode::Plan => command_palette_glyph_icon("\u{f19c}", 0xe5c07b, theme),
            ExecutionMode::Build => command_palette_glyph_icon("\u{f085}", 0x72d69c, theme),
            ExecutionMode::Yolo => command_palette_glyph_icon("\u{f06d}", 0xeb6f92, theme),
        },
        CommandPaletteAction::LaunchAgentPreset(kind) => command_palette_preset_icon(*kind, theme),
        CommandPaletteAction::LaunchRepoPreset(_) => {
            command_palette_glyph_icon("\u{f04b}", 0xa6e3a1, theme)
        },
        CommandPaletteAction::SelectRepository(_) => {
            command_palette_glyph_icon("\u{f07b}", 0xe5c07b, theme)
        },
        CommandPaletteAction::SelectWorktree(_) => {
            command_palette_glyph_icon("\u{e725}", 0x89b4fa, theme)
        },
        CommandPaletteAction::LaunchTaskTemplate(task) => task
            .agent
            .map(|kind| command_palette_preset_icon(kind, theme))
            .unwrap_or_else(|| command_palette_glyph_icon("\u{f0ae}", 0x56b6c2, theme)),
    }
}

pub(crate) fn command_palette_glyph_icon(
    glyph: &'static str,
    color: u32,
    _theme: ThemePalette,
) -> AnyElement {
    div()
        .w(px(34.))
        .h(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(px(18.))
                .line_height(px(18.))
                .text_color(rgb(color))
                .child(glyph),
        )
        .into_any_element()
}

pub(crate) fn command_palette_preset_icon(
    kind: AgentPresetKind,
    theme: ThemePalette,
) -> AnyElement {
    log_preset_icon_render_once(kind);
    let icon = preset_icon_image(kind);
    let icon_size = match kind {
        AgentPresetKind::Codex => 22.,
        AgentPresetKind::Claude
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => 17.,
    };
    let fallback_glyph = kind.fallback_icon();
    let fallback_color = match kind {
        AgentPresetKind::Claude => 0xD97757,
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => theme.text_primary,
    };

    div()
        .w(px(34.))
        .h(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(img(icon).size(px(icon_size)).with_fallback(move || {
            log_preset_icon_fallback_once(kind, fallback_glyph);
            div()
                .font_family(FONT_MONO)
                .text_size(px(14.))
                .line_height(px(14.))
                .text_color(rgb(fallback_color))
                .child(fallback_glyph)
                .into_any_element()
        }))
        .into_any_element()
}

pub(crate) fn command_palette_scrollbar_indicator(
    theme: ThemePalette,
    item_count: usize,
    selected_index: usize,
) -> Option<Div> {
    let visible_rows = (COMMAND_PALETTE_MAX_HEIGHT_PX / COMMAND_PALETTE_ROW_ESTIMATE_PX)
        .floor()
        .max(1.0) as usize;
    if item_count <= visible_rows {
        return None;
    }

    let track_height = COMMAND_PALETTE_SCROLLBAR_TRACK_HEIGHT_PX;
    let thumb_height = (track_height * (visible_rows as f32 / item_count as f32)).max(32.);
    let max_offset = item_count.saturating_sub(visible_rows);
    let offset = selected_index
        .saturating_sub(visible_rows / 2)
        .min(max_offset);
    let thumb_top = if max_offset == 0 {
        0.
    } else {
        (track_height - thumb_height) * (offset as f32 / max_offset as f32)
    };

    Some(
        div()
            .absolute()
            .top(px(12.))
            .right(px(5.))
            .w(px(8.))
            .h(px(track_height))
            .rounded_full()
            .bg(rgb(theme.panel_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .child(
                div()
                    .absolute()
                    .left(px(1.))
                    .top(px(thumb_top))
                    .w(px(4.))
                    .h(px(thumb_height))
                    .rounded_full()
                    .bg(rgb(theme.accent)),
            ),
    )
}

pub(crate) fn command_palette_action_key(item: &CommandPaletteItem) -> String {
    match &item.action {
        CommandPaletteAction::BrowseIssues => "command-palette:browse-issues".to_owned(),
        CommandPaletteAction::OpenIssueCreateModal(issue) => {
            format!("command-palette:issue:{}", issue.id)
        },
        _ => format!("{}|{}", item.title, item.subtitle),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CommandPaletteMatchScore {
    pub(crate) bucket: u8,
    pub(crate) title_position: usize,
    pub(crate) search_position: usize,
    pub(crate) title_len: usize,
}

pub(crate) fn command_palette_match_score(
    item: &CommandPaletteItem,
    query: &str,
) -> Option<CommandPaletteMatchScore> {
    let title = item.title.to_ascii_lowercase();
    let subtitle = item.subtitle.to_ascii_lowercase();
    let search_text = item.search_text.to_ascii_lowercase();
    let tokens = command_palette_query_tokens(query);
    if tokens.is_empty() {
        return Some(CommandPaletteMatchScore {
            bucket: 0,
            title_position: 0,
            search_position: 0,
            title_len: title.len(),
        });
    }

    let exact_title = title == query;
    let title_starts = title.starts_with(query);
    let acronym_match = command_palette_initialism(&title) == query;
    let word_prefix_match = command_palette_title_word_prefix_match(&title, &tokens);
    let title_position = title.find(query).unwrap_or(usize::MAX);
    let search_position = search_text.find(query).unwrap_or(usize::MAX);
    let token_match = command_palette_tokens_match(&tokens, &[&title, &subtitle, &search_text]);

    let bucket = if exact_title {
        0
    } else if title_starts {
        1
    } else if acronym_match {
        2
    } else if word_prefix_match {
        3
    } else if title_position != usize::MAX {
        4
    } else if token_match {
        5
    } else if search_position != usize::MAX {
        6
    } else {
        return None;
    };

    Some(CommandPaletteMatchScore {
        bucket,
        title_position,
        search_position,
        title_len: title.len(),
    })
}

pub(crate) fn command_palette_query_tokens(query: &str) -> Vec<&str> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect()
}

pub(crate) fn command_palette_title_word_prefix_match(title: &str, tokens: &[&str]) -> bool {
    if tokens.is_empty() {
        return true;
    }

    let words = title
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return false;
    }

    let mut word_index = 0usize;
    for token in tokens {
        let mut matched = false;
        while let Some(word) = words.get(word_index) {
            word_index += 1;
            if word.starts_with(token) {
                matched = true;
                break;
            }
        }
        if !matched {
            return false;
        }
    }

    true
}

pub(crate) fn command_palette_tokens_match(tokens: &[&str], fields: &[&str]) -> bool {
    tokens
        .iter()
        .all(|token| fields.iter().any(|field| field.contains(token)))
}

pub(crate) fn command_palette_initialism(title: &str) -> String {
    title
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .filter_map(|word| word.chars().next())
        .collect()
}

#[cfg(test)]
mod command_palette_tests {
    use super::*;

    fn palette_item(title: &str, subtitle: &str, search_text: &str) -> CommandPaletteItem {
        CommandPaletteItem {
            title: title.to_owned(),
            subtitle: subtitle.to_owned(),
            search_text: search_text.to_owned(),
            action: CommandPaletteAction::RefreshWorktrees,
        }
    }

    #[test]
    fn command_palette_scores_exact_matches_first() {
        let exact = command_palette_match_score(
            &palette_item("Open Settings", "Settings", "open settings config"),
            "open settings",
        )
        .unwrap_or_else(|| panic!("exact match should score"));
        let fuzzy = command_palette_match_score(
            &palette_item("Settings", "Preferences", "open settings config"),
            "open settings",
        )
        .unwrap_or_else(|| panic!("token match should score"));

        assert!(exact < fuzzy);
    }

    #[test]
    fn command_palette_supports_initialism_matching() {
        let score = command_palette_match_score(
            &palette_item(
                "New Worktree",
                "Create a local worktree",
                "new worktree create",
            ),
            "nw",
        );
        assert!(score.is_some());
    }

    #[test]
    fn command_palette_supports_multi_token_word_prefix_matching() {
        let score = command_palette_match_score(
            &palette_item(
                "Refresh Worktrees",
                "Reload repositories and worktrees",
                "refresh worktrees reload repos",
            ),
            "ref work",
        );
        assert!(score.is_some());
    }
}
