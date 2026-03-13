impl ArborWindow {
    fn open_command_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette_modal = Some(CommandPaletteModal {
            query: String::new(),
            query_cursor: 0,
            selected_index: 0,
        });
        self.command_palette_scroll_handle.scroll_to_item(0);
        self.refresh_repo_config_if_changed(cx);
        cx.notify();
    }

    fn close_command_palette(&mut self, cx: &mut Context<Self>) {
        self.command_palette_modal = None;
        cx.notify();
    }

    fn command_palette_items(&self) -> Vec<CommandPaletteItem> {
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

    fn filtered_command_palette_items(&self) -> Vec<CommandPaletteItem> {
        let Some(modal) = self.command_palette_modal.as_ref() else {
            return Vec::new();
        };

        let query = modal.query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return self.command_palette_items();
        }

        let mut matches = self
            .command_palette_items()
            .into_iter()
            .filter_map(|item| {
                command_palette_match_score(&item, &query).map(|score| {
                    (
                        self.command_palette_recent_rank(&item),
                        self.command_palette_context_rank(&item),
                        score,
                        item,
                    )
                })
            })
            .collect::<Vec<_>>();
        matches.sort_by(
            |(left_recent, left_context, left_score, left_item),
             (right_recent, right_context, right_score, right_item)| {
                left_recent
                    .cmp(right_recent)
                    .then_with(|| left_context.cmp(right_context))
                    .then_with(|| left_score.cmp(right_score))
                    .then_with(|| {
                        left_item
                            .title
                            .to_ascii_lowercase()
                            .cmp(&right_item.title.to_ascii_lowercase())
                    })
            },
        );
        matches.into_iter().map(|(_, _, _, item)| item).collect()
    }

    fn command_palette_recent_rank(&self, item: &CommandPaletteItem) -> usize {
        let action_key = command_palette_action_key(item);
        self.command_palette_recent_actions
            .iter()
            .position(|recent| recent == &action_key)
            .unwrap_or(usize::MAX)
    }

    fn command_palette_context_rank(&self, item: &CommandPaletteItem) -> usize {
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
            | CommandPaletteAction::OpenReviewPullRequest
            | CommandPaletteAction::RefreshWorktrees
            | CommandPaletteAction::ToggleCompactSidebar
            | CommandPaletteAction::OpenSettings
            | CommandPaletteAction::OpenThemePicker
            | CommandPaletteAction::SetExecutionMode(_) => 3,
        }
    }

    fn remember_command_palette_action(&mut self, item: &CommandPaletteItem) {
        let action_key = command_palette_action_key(item);
        self.command_palette_recent_actions
            .retain(|recent| recent != &action_key);
        self.command_palette_recent_actions.insert(0, action_key);
        self.command_palette_recent_actions.truncate(20);
    }

    fn execute_command_palette_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected_index = self
            .command_palette_modal
            .as_ref()
            .map(|modal| modal.selected_index)
            .unwrap_or(0);
        let items = self.filtered_command_palette_items();
        let Some(item) = items.get(selected_index).cloned() else {
            return;
        };

        self.command_palette_modal = None;
        self.remember_command_palette_action(&item);
        match item.action {
            CommandPaletteAction::OpenCreateWorktree => {
                let repo_index = self.active_repository_index.unwrap_or(0);
                self.open_create_modal(repo_index, CreateModalTab::LocalWorktree, cx);
            },
            CommandPaletteAction::OpenReviewPullRequest => {
                let repo_index = self.active_repository_index.unwrap_or(0);
                self.open_create_modal(repo_index, CreateModalTab::ReviewPullRequest, cx);
            },
            CommandPaletteAction::RefreshWorktrees => self.refresh_worktrees(cx),
            CommandPaletteAction::ToggleCompactSidebar => {
                self.compact_sidebar = !self.compact_sidebar;
                self.notice = Some(if self.compact_sidebar {
                    "compact sidebar enabled".to_owned()
                } else {
                    "compact sidebar disabled".to_owned()
                });
                self.sync_ui_state_store(window, cx);
            },
            CommandPaletteAction::OpenSettings => self.open_settings_modal(cx),
            CommandPaletteAction::OpenThemePicker => self.open_theme_picker_modal(cx),
            CommandPaletteAction::SetExecutionMode(mode) => {
                self.set_execution_mode(mode, window, cx);
            },
            CommandPaletteAction::LaunchAgentPreset(preset) => {
                self.launch_agent_preset(preset, window, cx);
            },
            CommandPaletteAction::LaunchRepoPreset(index) => {
                self.launch_repo_preset(index, window, cx);
            },
            CommandPaletteAction::SelectRepository(index) => self.select_repository(index, cx),
            CommandPaletteAction::SelectWorktree(index) => self.select_worktree(index, window, cx),
            CommandPaletteAction::LaunchTaskTemplate(task) => {
                self.launch_task_template(&task, window, cx);
            },
        }
        cx.notify();
    }

    fn move_command_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
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

    fn launch_task_template(
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

        let invocation = match prompt_terminal_invocation(
            preset,
            &command,
            &task.prompt,
            self.execution_mode,
        ) {
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

    fn render_command_palette_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.command_palette_modal.clone() else {
            return div();
        };
        let theme = self.theme();
        let items = self.filtered_command_palette_items();
        let item_count = items.len();
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
                    .child("No results"),
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
                                    "Search actions, worktrees, presets...",
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
                                    .child(format!("{item_count} results")),
                            ),
                    )
                    .child(results),
            )
    }
}

fn command_palette_icon(action: &CommandPaletteAction, theme: ThemePalette) -> AnyElement {
    match action {
        CommandPaletteAction::OpenCreateWorktree => {
            command_palette_glyph_icon("\u{f055}", 0x98c379, theme)
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

fn command_palette_glyph_icon(glyph: &'static str, color: u32, _theme: ThemePalette) -> AnyElement {
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

fn command_palette_preset_icon(kind: AgentPresetKind, theme: ThemePalette) -> AnyElement {
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

fn command_palette_scrollbar_indicator(
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

fn command_palette_action_key(item: &CommandPaletteItem) -> String {
    format!("{}|{}", item.title, item.subtitle)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CommandPaletteMatchScore {
    bucket: u8,
    title_position: usize,
    search_position: usize,
    title_len: usize,
}

fn command_palette_match_score(
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

fn command_palette_query_tokens(query: &str) -> Vec<&str> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect()
}

fn command_palette_title_word_prefix_match(title: &str, tokens: &[&str]) -> bool {
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

fn command_palette_tokens_match(tokens: &[&str], fields: &[&str]) -> bool {
    tokens.iter().all(|token| fields.iter().any(|field| field.contains(token)))
}

fn command_palette_initialism(title: &str) -> String {
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
            &palette_item("New Worktree", "Create a local worktree", "new worktree create"),
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
