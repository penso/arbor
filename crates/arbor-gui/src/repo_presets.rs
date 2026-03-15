use super::*;

impl ArborWindow {
    pub(crate) fn open_manage_repo_presets_modal(
        &mut self,
        editing_index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let (icon, name, command) = if let Some(index) = editing_index {
            if let Some(preset) = self.repo_presets.get(index) {
                (
                    preset.icon.clone(),
                    preset.name.clone(),
                    preset.command.clone(),
                )
            } else {
                return;
            }
        } else {
            (String::new(), String::new(), String::new())
        };

        self.manage_repo_presets_modal = Some(ManageRepoPresetsModal {
            editing_index,
            icon_cursor: char_count(&icon),
            icon,
            name_cursor: char_count(&name),
            name,
            command_cursor: char_count(&command),
            command,
            active_tab: RepoPresetModalTab::Edit,
            active_field: RepoPresetModalField::Icon,
            error: None,
            saving: false,
        });
        cx.notify();
    }

    pub(crate) fn close_manage_repo_presets_modal(&mut self, cx: &mut Context<Self>) {
        self.manage_repo_presets_modal = None;
        cx.notify();
    }

    pub(crate) fn update_manage_repo_presets_modal_input(
        &mut self,
        input: RepoPresetsModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(mut modal) = self.manage_repo_presets_modal.clone() else {
            return;
        };

        match input {
            RepoPresetsModalInputEvent::SetActiveTab(tab) => {
                modal.active_tab = tab;
            },
            RepoPresetsModalInputEvent::SetActiveField(field) => {
                if modal.active_tab != RepoPresetModalTab::Edit {
                    self.manage_repo_presets_modal = Some(modal);
                    cx.notify();
                    return;
                }
                modal.active_field = field;
                match field {
                    RepoPresetModalField::Icon => modal.icon_cursor = char_count(&modal.icon),
                    RepoPresetModalField::Name => modal.name_cursor = char_count(&modal.name),
                    RepoPresetModalField::Command => {
                        modal.command_cursor = char_count(&modal.command);
                    },
                }
            },
            RepoPresetsModalInputEvent::MoveActiveField(reverse) => {
                if modal.active_tab != RepoPresetModalTab::Edit {
                    self.manage_repo_presets_modal = Some(modal);
                    cx.notify();
                    return;
                }
                modal.active_field = if reverse {
                    modal.active_field.prev()
                } else {
                    modal.active_field.next()
                };
            },
            RepoPresetsModalInputEvent::Edit(action) => {
                if modal.active_tab != RepoPresetModalTab::Edit {
                    self.manage_repo_presets_modal = Some(modal);
                    cx.notify();
                    return;
                }
                match modal.active_field {
                    RepoPresetModalField::Icon => {
                        apply_text_edit_action(&mut modal.icon, &mut modal.icon_cursor, &action);
                    },
                    RepoPresetModalField::Name => {
                        apply_text_edit_action(&mut modal.name, &mut modal.name_cursor, &action);
                    },
                    RepoPresetModalField::Command => {
                        apply_text_edit_action(
                            &mut modal.command,
                            &mut modal.command_cursor,
                            &action,
                        );
                    },
                }
            },
            RepoPresetsModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        self.manage_repo_presets_modal = Some(modal);
        cx.notify();
    }

    pub(crate) fn submit_manage_repo_presets_modal(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.manage_repo_presets_modal.clone() else {
            return;
        };
        if modal.saving {
            return;
        }

        let name = modal.name.trim().to_owned();
        let command = modal.command.trim().to_owned();
        let icon = modal.icon.trim().to_owned();

        if name.is_empty() {
            if let Some(m) = self.manage_repo_presets_modal.as_mut() {
                m.error = Some("Name is required.".to_owned());
            }
            cx.notify();
            return;
        }
        if command.is_empty() {
            if let Some(m) = self.manage_repo_presets_modal.as_mut() {
                m.error = Some("Command is required.".to_owned());
            }
            cx.notify();
            return;
        }

        let new_preset = RepoPreset {
            name: name.clone(),
            icon,
            command,
        };

        let previous_repo_presets = self.repo_presets.clone();
        if let Some(index) = modal.editing_index {
            if let Some(preset) = self.repo_presets.get_mut(index) {
                *preset = new_preset;
            }
        } else {
            self.repo_presets.push(new_preset);
        }

        if let Some(m) = self.manage_repo_presets_modal.as_mut() {
            m.error = None;
            m.saving = true;
        }
        let action = if modal.editing_index.is_some() {
            "updated"
        } else {
            "added"
        };
        let save_dir = self.active_arbor_toml_dir();
        let presets: Vec<app_config::RepoPresetConfig> = self
            .repo_presets
            .iter()
            .map(|preset| app_config::RepoPresetConfig {
                name: preset.name.clone(),
                icon: preset.icon.clone(),
                command: preset.command.clone(),
            })
            .collect();
        let store = self.app_config_store.clone();
        self.begin_background_config_save();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.save_repo_presets(&save_dir, &presets) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.manage_repo_presets_modal = None;
                        this.notice = Some(format!("Preset \"{name}\" {action}."));
                    },
                    Err(error) => {
                        let error = error.to_string();
                        this.repo_presets = previous_repo_presets;
                        if let Some(m) = this.manage_repo_presets_modal.as_mut() {
                            m.error = Some(error.clone());
                            m.saving = false;
                        } else {
                            this.notice = Some(error);
                        }
                    },
                }
                this.finish_background_config_save(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn delete_repo_preset(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.manage_repo_presets_modal.as_ref() else {
            return;
        };
        let Some(index) = modal.editing_index else {
            return;
        };
        let Some(preset) = self.repo_presets.get(index) else {
            return;
        };
        let name = preset.name.clone();
        if let Some(m) = self.manage_repo_presets_modal.as_mut() {
            m.error = None;
            m.saving = true;
        }
        let save_dir = self.active_arbor_toml_dir();
        let store = self.app_config_store.clone();
        let name_for_delete = name.clone();
        self.begin_background_config_save();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { store.remove_repo_preset(&save_dir, &name_for_delete) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.repo_presets.remove(index);
                        this.manage_repo_presets_modal = None;
                        this.notice = Some(format!("Preset \"{name}\" removed."));
                    },
                    Err(error) => {
                        let error = error.to_string();
                        if let Some(m) = this.manage_repo_presets_modal.as_mut() {
                            m.error = Some(error.clone());
                            m.saving = false;
                        } else {
                            this.notice = Some(error);
                        }
                    },
                }
                this.finish_background_config_save(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render_manage_repo_presets_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.manage_repo_presets_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let is_editing = modal.editing_index.is_some();
        let is_edit_tab = modal.active_tab == RepoPresetModalTab::Edit;
        let title = if is_editing {
            "Edit Custom Preset"
        } else {
            "Add Custom Preset"
        };
        let save_disabled = modal.name.trim().is_empty() || modal.command.trim().is_empty();
        let local_preset_path = self.active_arbor_toml_dir().join("arbor.toml");
        let local_preset_example = format!(
            "[[presets]]\nname = \"{}\"\nicon = \"{}\"\ncommand = \"{}\"",
            if modal.name.trim().is_empty() {
                "dev"
            } else {
                modal.name.trim()
            },
            if modal.icon.trim().is_empty() {
                "\u{f013}"
            } else {
                modal.icon.trim()
            },
            if modal.command.trim().is_empty() {
                "just run"
            } else {
                modal.command.trim()
            }
        );
        let tab_button = |tab: RepoPresetModalTab, label: &'static str| {
            let is_active = modal.active_tab == tab;
            div()
                .cursor_pointer()
                .px_3()
                .py_1()
                .flex()
                .items_center()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(if is_active {
                    theme.text_primary
                } else {
                    theme.text_muted
                }))
                .when(is_active, |this| {
                    this.border_b_2().border_color(rgb(theme.accent))
                })
                .hover(|s| {
                    s.bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(theme.text_primary))
                })
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.update_manage_repo_presets_modal_input(
                            RepoPresetsModalInputEvent::SetActiveTab(tab),
                            cx,
                        );
                    }),
                )
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
                    this.close_manage_repo_presets_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_repo_presets_modal(cx);
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
                                .child(title),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_0()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .child(tab_button(RepoPresetModalTab::Edit, "Edit"))
                            .child(tab_button(RepoPresetModalTab::LocalPreset, "Local Preset")),
                    )
                    .when(is_edit_tab, |this| {
                        this.child(
                            modal_input_field(
                                theme,
                                "repo-preset-icon-input",
                                "Icon (emoji)",
                                &modal.icon,
                                modal.icon_cursor,
                                "\u{f013}",
                                modal.active_field == RepoPresetModalField::Icon,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_repo_presets_modal_input(
                                    RepoPresetsModalInputEvent::SetActiveField(
                                        RepoPresetModalField::Icon,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "repo-preset-name-input",
                                "Name",
                                &modal.name,
                                modal.name_cursor,
                                "my preset",
                                modal.active_field == RepoPresetModalField::Name,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_repo_presets_modal_input(
                                    RepoPresetsModalInputEvent::SetActiveField(
                                        RepoPresetModalField::Name,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "repo-preset-command-input",
                                "Command",
                                &modal.command,
                                modal.command_cursor,
                                "just run",
                                modal.active_field == RepoPresetModalField::Command,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_repo_presets_modal_input(
                                    RepoPresetsModalInputEvent::SetActiveField(
                                        RepoPresetModalField::Command,
                                    ),
                                    cx,
                                );
                            })),
                        )
                    })
                    .when(!is_edit_tab, |this| {
                        this.child(
                            div()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(theme.border))
                                .bg(rgb(theme.panel_bg))
                                .p_3()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme.text_primary))
                                        .child("Add repo-local presets directly in `arbor.toml`."),
                                )
                                .child(div().text_xs().text_color(rgb(theme.text_muted)).child(
                                    format!(
                                        "Arbor reads local presets from {}",
                                        local_preset_path.display()
                                    ),
                                ))
                                .child(
                                    div()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .bg(rgb(theme.terminal_bg))
                                        .p_2()
                                        .font_family(FONT_MONO)
                                        .text_xs()
                                        .text_color(rgb(theme.text_primary))
                                        .children(
                                            local_preset_example
                                                .lines()
                                                .map(|line| div().child(line.to_owned())),
                                        ),
                                ),
                        )
                    })
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
                            .when(is_edit_tab && is_editing, |this| {
                                this.child(
                                    action_button(
                                        theme,
                                        "repo-preset-new",
                                        "New Preset",
                                        ActionButtonStyle::Secondary,
                                        !modal.saving,
                                    )
                                    .when(
                                        !modal.saving,
                                        |this| {
                                            this.on_click(cx.listener(|this, _, _, cx| {
                                                this.open_manage_repo_presets_modal(None, cx);
                                            }))
                                        },
                                    ),
                                )
                                .child(
                                    action_button(
                                        theme,
                                        "repo-preset-delete",
                                        "Delete",
                                        ActionButtonStyle::Secondary,
                                        !modal.saving,
                                    )
                                    .when(
                                        !modal.saving,
                                        |this| {
                                            this.on_click(cx.listener(|this, _, _, cx| {
                                                this.delete_repo_preset(cx);
                                            }))
                                        },
                                    ),
                                )
                            })
                            .child(
                                action_button(
                                    theme,
                                    "repo-preset-cancel",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    !modal.saving,
                                )
                                .when(!modal.saving, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.close_manage_repo_presets_modal(cx);
                                    }))
                                }),
                            )
                            .when(is_edit_tab, |this| {
                                this.child(
                                    action_button(
                                        theme,
                                        "repo-preset-save",
                                        "Save",
                                        ActionButtonStyle::Primary,
                                        !save_disabled && !modal.saving,
                                    )
                                    .when(
                                        !save_disabled && !modal.saving,
                                        |this| {
                                            this.on_click(cx.listener(|this, _, _, cx| {
                                                this.submit_manage_repo_presets_modal(cx);
                                            }))
                                        },
                                    ),
                                )
                            }),
                    ),
            )
    }
}

pub(crate) fn load_repo_presets(
    store: &dyn app_config::AppConfigStore,
    repo_root: &Path,
) -> Vec<RepoPreset> {
    let Some(config) = store.load_repo_config(repo_root) else {
        return Vec::new();
    };
    config
        .presets
        .into_iter()
        .filter(|p| !p.name.trim().is_empty() && !p.command.trim().is_empty())
        .map(|p| RepoPreset {
            name: p.name.trim().to_owned(),
            icon: p.icon.trim().to_owned(),
            command: p.command.trim().to_owned(),
        })
        .collect()
}
