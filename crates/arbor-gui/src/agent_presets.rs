use super::*;

impl ArborWindow {
    pub(crate) fn preset_command_for_kind(&self, kind: AgentPresetKind) -> String {
        self.agent_presets
            .iter()
            .find(|preset| preset.kind == kind)
            .map(|preset| preset.command.clone())
            .unwrap_or_else(|| kind.default_command().to_owned())
    }

    pub(crate) fn set_preset_command_for_kind(&mut self, kind: AgentPresetKind, command: String) {
        if let Some(preset) = self
            .agent_presets
            .iter_mut()
            .find(|preset| preset.kind == kind)
        {
            preset.command = command;
            return;
        }

        self.agent_presets.push(AgentPreset { kind, command });
        self.agent_presets.sort_by_key(|preset| {
            AgentPresetKind::ORDER
                .iter()
                .position(|kind| *kind == preset.kind)
                .unwrap_or(usize::MAX)
        });
    }

    pub(crate) fn save_agent_presets(&self) -> Result<(), StoreError> {
        let presets = self
            .agent_presets
            .iter()
            .map(|preset| app_config::AgentPresetConfig {
                key: preset.kind.key().to_owned(),
                command: preset.command.clone(),
            })
            .collect::<Vec<_>>();
        self.app_config_store.save_agent_presets(&presets)
    }

    pub(crate) fn open_manage_presets_modal(&mut self, cx: &mut Context<Self>) {
        let active_preset = self.selected_agent_preset_or_default();
        let command = self.preset_command_for_kind(active_preset);
        self.manage_presets_modal = Some(ManagePresetsModal {
            active_preset,
            command_cursor: char_count(&command),
            command,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_manage_presets_modal(&mut self, cx: &mut Context<Self>) {
        self.manage_presets_modal = None;
        cx.notify();
    }

    pub(crate) fn update_manage_presets_modal_input(
        &mut self,
        input: PresetsModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(mut modal) = self.manage_presets_modal.clone() else {
            return;
        };

        match input {
            PresetsModalInputEvent::SetActivePreset(kind) => {
                modal.active_preset = kind;
                modal.command = self.preset_command_for_kind(kind);
                modal.command_cursor = char_count(&modal.command);
            },
            PresetsModalInputEvent::CycleActivePreset(reverse) => {
                modal.active_preset = modal.active_preset.cycle(reverse);
                modal.command = self.preset_command_for_kind(modal.active_preset);
                modal.command_cursor = char_count(&modal.command);
            },
            PresetsModalInputEvent::Edit(action) => {
                apply_text_edit_action(&mut modal.command, &mut modal.command_cursor, &action);
            },
            PresetsModalInputEvent::RestoreDefault => {
                modal.command = modal.active_preset.default_command().to_owned();
                modal.command_cursor = char_count(&modal.command);
            },
            PresetsModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        self.manage_presets_modal = Some(modal);
        cx.notify();
    }

    pub(crate) fn submit_manage_presets_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.manage_presets_modal.clone() else {
            return;
        };

        let command = modal.command.trim().to_owned();
        if command.is_empty() {
            if let Some(modal_state) = self.manage_presets_modal.as_mut() {
                modal_state.error = Some("Command is required.".to_owned());
            }
            cx.notify();
            return;
        }

        self.set_preset_command_for_kind(modal.active_preset, command);
        if let Err(error) = self.save_agent_presets() {
            if let Some(modal_state) = self.manage_presets_modal.as_mut() {
                modal_state.error = Some(error.to_string());
            }
            cx.notify();
            return;
        }

        self.config_last_modified = None;
        self.refresh_config_if_changed(cx);
        self.manage_presets_modal = None;
        self.notice = Some(format!("{} preset updated", modal.active_preset.label()));
        cx.notify();
    }

    pub(crate) fn launch_agent_preset(
        &mut self,
        preset: AgentPresetKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = match command_for_execution_mode(
            preset,
            &self.preset_command_for_kind(preset),
            self.execution_mode,
        ) {
            Ok(command) => command,
            Err(error) => {
                self.notice = Some(error.to_string());
                cx.notify();
                return;
            },
        };
        self.active_preset_tab = Some(preset);
        if command.is_empty() {
            self.notice = Some(format!("{} preset command is empty", preset.label()));
            cx.notify();
            return;
        }

        let terminal_count_before = self.terminals.len();
        self.spawn_terminal_session(window, cx);
        if self.terminals.len() <= terminal_count_before {
            return;
        }

        let Some(session_id) = self.terminals.last().map(|session| session.id) else {
            return;
        };

        let input = format!("{command}\n");
        if let Err(error) = self.write_input_to_terminal(session_id, input.as_bytes()) {
            self.notice = Some(format!("failed to run {} preset: {error}", preset.label()));
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
            session.last_command = Some(command);
            session.pending_command.clear();
            session.updated_at_unix_ms = current_unix_timestamp_millis();
        }

        self.sync_daemon_session_store(cx);
        cx.notify();
    }

    pub(crate) fn set_execution_mode(
        &mut self,
        mode: ExecutionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.execution_mode == mode {
            return;
        }

        self.execution_mode = mode;
        if let Some(session_id) = self
            .active_center_tab_for_selected_worktree()
            .and_then(|tab| match tab {
                CenterTab::Terminal(session_id) => Some(session_id),
                _ => None,
            })
            && let Some(session) = self
                .terminals
                .iter_mut()
                .find(|session| session.id == session_id)
        {
            session.execution_mode = Some(mode);
        }

        self.notice = Some(format!("execution mode set to {}", mode.label()));
        self.sync_ui_state_store(window, cx);
        cx.notify();
    }

    pub(crate) fn render_manage_presets_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.manage_presets_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let save_disabled = modal.command.trim().is_empty();
        let tab_button = |kind: AgentPresetKind| {
            let is_active = modal.active_preset == kind;
            let text_color = if is_active {
                theme.text_primary
            } else {
                theme.text_muted
            };
            div()
                .id(ElementId::Name(
                    format!("preset-modal-tab-{}", kind.key()).into(),
                ))
                .cursor_pointer()
                .px_3()
                .py_1()
                .flex()
                .items_center()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(text_color))
                .when(is_active, |this| {
                    this.border_b_2().border_color(rgb(theme.accent))
                })
                .hover(|surface| {
                    surface
                        .bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(theme.text_primary))
                })
                .child(agent_preset_button_content(kind, text_color))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.update_manage_presets_modal_input(
                        PresetsModalInputEvent::SetActivePreset(kind),
                        cx,
                    );
                }))
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_presets_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_presets_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(620.))
                    .max_w(px(620.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div().flex().items_center().child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(theme.text_primary))
                                .child("Edit Agent Preset"),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_0()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .children(AgentPresetKind::ORDER.iter().copied().map(&tab_button)),
                    )
                    .child(
                        modal_input_field(
                            theme,
                            "preset-command-input",
                            "Command",
                            &modal.command,
                            modal.command_cursor,
                            modal.active_preset.default_command(),
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.update_manage_presets_modal_input(
                                PresetsModalInputEvent::ClearError,
                                cx,
                            );
                        })),
                    )
                    .child(div().when_some(modal.error.clone(), |this, error| {
                        this.rounded_sm()
                            .border_1()
                            .border_color(rgb(0xa44949))
                            .bg(rgb(0x4d2a2a))
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(rgb(0xffd7d7))
                            .child(error)
                    }))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "preset-restore-default",
                                    "Restore Default",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.update_manage_presets_modal_input(
                                            PresetsModalInputEvent::RestoreDefault,
                                            cx,
                                        );
                                    },
                                )),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "preset-cancel",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_manage_presets_modal(cx);
                                    },
                                )),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "preset-save",
                                    "Save",
                                    ActionButtonStyle::Primary,
                                    !save_disabled,
                                )
                                .when(!save_disabled, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_manage_presets_modal(cx);
                                    }))
                                }),
                            ),
                    ),
            )
    }
}
