use super::*;

pub(crate) fn background_config_save_has_work(pending_app_config_save_count: usize) -> bool {
    pending_app_config_save_count > 0
}

impl ArborWindow {
    pub(crate) fn begin_background_config_save(&mut self) {
        self.pending_app_config_save_count = self.pending_app_config_save_count.saturating_add(1);
    }

    pub(crate) fn finish_background_config_save(&mut self, cx: &mut Context<Self>) {
        self.pending_app_config_save_count = self.pending_app_config_save_count.saturating_sub(1);
        self.maybe_finish_quit_after_persistence_flush(cx);
    }

    pub(crate) fn open_settings_modal(&mut self, cx: &mut Context<Self>) {
        self.settings_modal = Some(SettingsModal {
            active_control: SettingsControl::DaemonBindMode,
            daemon_bind_mode: DaemonBindMode::AllInterfaces,
            initial_daemon_bind_mode: DaemonBindMode::AllInterfaces,
            notifications: self.notifications_enabled,
            daemon_auth_token: String::new(),
            loading: true,
            error: None,
        });
        let store = self.app_config_store.clone();
        let notifications_enabled = self.notifications_enabled;
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move { store.load_or_create_config() })
                .await;

            let _ = this.update(cx, |this, cx| {
                let Some(modal) = this.settings_modal.as_mut() else {
                    return;
                };
                let daemon_auth_token = loaded
                    .config
                    .daemon
                    .as_ref()
                    .and_then(|daemon| daemon.auth_token.clone())
                    .unwrap_or_default();
                let daemon_bind_mode = DaemonBindMode::from_config(
                    loaded
                        .config
                        .daemon
                        .as_ref()
                        .and_then(|daemon| daemon.bind.as_deref()),
                );
                modal.daemon_bind_mode = daemon_bind_mode;
                modal.initial_daemon_bind_mode = daemon_bind_mode;
                modal.notifications = notifications_enabled;
                modal.daemon_auth_token = daemon_auth_token;
                modal.loading = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn close_settings_modal(&mut self, cx: &mut Context<Self>) {
        self.settings_modal = None;
        cx.notify();
    }

    pub(crate) fn update_settings_modal_input(
        &mut self,
        input: SettingsModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(mut modal) = self.settings_modal.clone() else {
            return;
        };

        modal.error = None;
        match input {
            SettingsModalInputEvent::CycleControl(reverse) => {
                modal.active_control = modal.active_control.cycle(reverse);
            },
            SettingsModalInputEvent::SelectDaemonBindMode(bind_mode) => {
                modal.active_control = SettingsControl::DaemonBindMode;
                modal.daemon_bind_mode = bind_mode;
            },
            SettingsModalInputEvent::ToggleActiveControl => match modal.active_control {
                SettingsControl::DaemonBindMode => {
                    modal.daemon_bind_mode = match modal.daemon_bind_mode {
                        DaemonBindMode::Localhost => DaemonBindMode::AllInterfaces,
                        DaemonBindMode::AllInterfaces => DaemonBindMode::Localhost,
                    };
                },
                SettingsControl::Notifications => {
                    modal.notifications = !modal.notifications;
                },
            },
            SettingsModalInputEvent::ToggleNotifications => {
                modal.active_control = SettingsControl::Notifications;
                modal.notifications = !modal.notifications;
            },
        }

        self.settings_modal = Some(modal);
        cx.notify();
    }

    pub(crate) fn submit_settings_modal(&mut self, cx: &mut Context<Self>) {
        let Some(mut modal) = self.settings_modal.clone() else {
            return;
        };
        if modal.loading {
            return;
        }
        modal.loading = true;
        self.settings_modal = Some(modal.clone());
        cx.notify();

        let notifications_str = if modal.notifications {
            "true"
        } else {
            "false"
        };
        let theme_slug = self.theme_kind.slug();
        let daemon_bind_changed = modal.daemon_bind_mode != modal.initial_daemon_bind_mode;
        let store = self.app_config_store.clone();
        let daemon = self.terminal_daemon.clone();
        let daemon_base_url = self.daemon_base_url.clone();
        self.begin_background_config_save();
        cx.spawn(async move |this, cx| {
            enum SettingsSaveOutcome {
                Saved(Option<bool>),
                RestartNeeded,
            }

            let result = cx
                .background_spawn(async move {
                    store.save_scalar_settings(&[
                        ("notifications", Some(notifications_str)),
                        ("theme", Some(theme_slug)),
                    ])?;
                    store.save_daemon_bind_mode(Some(modal.daemon_bind_mode.as_config_value()))?;

                    let daemon_bind_result =
                        if daemon_bind_changed && daemon_url_is_local(&daemon_base_url) {
                        let allow_remote = modal.daemon_bind_mode == DaemonBindMode::AllInterfaces;
                        match daemon
                            .as_ref()
                            .map(|daemon| daemon.set_bind_mode(allow_remote).map(|()| allow_remote))
                            .transpose()
                        {
                            Ok(result) => SettingsSaveOutcome::Saved(result),
                            Err(error) => {
                                tracing::warn!(%error, "failed to update daemon bind mode, restarting");
                                SettingsSaveOutcome::RestartNeeded
                            },
                        }
                    } else {
                        SettingsSaveOutcome::Saved(None)
                    };

                    Ok::<SettingsSaveOutcome, StoreError>(daemon_bind_result)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(SettingsSaveOutcome::Saved(Some(allow_remote))) => {
                        this.config_last_modified = None;
                        this.refresh_config_if_changed(cx);
                        this.settings_modal = None;
                        let mode = if allow_remote {
                            "all interfaces"
                        } else {
                            "localhost only"
                        };
                        this.notice =
                            Some(format!("Settings saved. Daemon now listening on {mode}."));
                    },
                    Ok(SettingsSaveOutcome::Saved(None)) => {
                        this.config_last_modified = None;
                        this.refresh_config_if_changed(cx);
                        this.settings_modal = None;
                        this.notice = Some("Settings saved".to_owned());
                    },
                    Ok(SettingsSaveOutcome::RestartNeeded) => {
                        this.config_last_modified = None;
                        this.settings_modal = None;
                        this.restart_local_daemon_after_settings_save(cx);
                        this.finish_background_config_save(cx);
                        return;
                    },
                    Err(error) => {
                        tracing::warn!(%error, "failed to update settings");
                        if let Some(modal_state) = this.settings_modal.as_mut() {
                            modal_state.loading = false;
                            modal_state.error = Some(error.to_string());
                        }
                    },
                }
                this.finish_background_config_save(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn restart_local_daemon_after_settings_save(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.terminal_daemon.clone() else {
            self.notice = Some("Settings saved".to_owned());
            cx.notify();
            return;
        };
        let daemon_base_url = self.daemon_base_url.clone();
        self.notice = Some("Settings saved. Restarting daemon…".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let _ = daemon.shutdown();
                    std::thread::sleep(Duration::from_millis(500));
                    try_auto_start_daemon(&daemon_base_url)
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                if let Some(client) = result {
                    let records = client.list_sessions().unwrap_or_default();
                    this.terminal_daemon = Some(client);
                    this.restore_terminal_sessions_from_records(records, true);
                    this.refresh_worktrees(cx);
                    this.notice = Some("Settings saved".to_owned());
                } else {
                    this.notice = Some(
                        "Settings saved, but Arbor could not restart the daemon automatically."
                            .to_owned(),
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_settings_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.settings_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let daemon_auth_token_empty = modal.daemon_auth_token.trim().is_empty();
        let loading = modal.loading;
        let section_card = |div: Div| {
            div.rounded_sm()
                .border_1()
                .border_color(rgb(theme.border))
                .bg(rgb(theme.panel_bg))
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
        };
        let section_heading = |title: &str| {
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme.text_primary))
                .child(title.to_owned())
        };
        let bind_mode_button = |mode: DaemonBindMode, title: &str, detail: &str| {
            let selected = modal.daemon_bind_mode == mode;
            div()
                .flex_1()
                .min_w_0()
                .cursor_pointer()
                .rounded_sm()
                .border_1()
                .border_color(rgb(if selected {
                    theme.accent
                } else {
                    theme.border
                }))
                .bg(rgb(if selected {
                    theme.panel_active_bg
                } else {
                    theme.sidebar_bg
                }))
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap(px(3.))
                .hover(|style| style.opacity(0.92))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if selected {
                            theme.text_primary
                        } else {
                            theme.text_muted
                        }))
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_muted))
                        .child(detail.to_owned()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.update_settings_modal_input(
                            SettingsModalInputEvent::SelectDaemonBindMode(mode),
                            cx,
                        );
                    }),
                )
        };
        let daemon_helper_text = match modal.daemon_bind_mode {
            DaemonBindMode::Localhost => "Only this machine can connect to the daemon.",
            DaemonBindMode::AllInterfaces => {
                "Other Arbor instances can connect with your host IP and the token below."
            },
        };
        let notifications_enabled = modal.notifications;
        let notifications_active = modal.active_control == SettingsControl::Notifications;
        let notifications_toggle = div()
            .id("settings-notifications-toggle")
            .cursor_pointer()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(rgb(if notifications_active {
                theme.accent
            } else {
                theme.border
            }))
            .bg(rgb(if notifications_enabled {
                theme.accent
            } else {
                theme.sidebar_bg
            }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if notifications_enabled {
                theme.app_bg
            } else {
                theme.text_muted
            }))
            .hover(|style| style.opacity(0.85))
            .on_click(cx.listener(|this, _, _, cx| {
                this.update_settings_modal_input(SettingsModalInputEvent::ToggleNotifications, cx);
            }))
            .child(if notifications_enabled {
                "Enabled"
            } else {
                "Disabled"
            });

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_settings_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_settings_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(500.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.sidebar_bg))
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child("Settings"),
                    )
                    .child(
                        section_card(div())
                            .child(section_heading("Daemon settings"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child("Choose who can reach this Arbor daemon."),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(bind_mode_button(
                                        DaemonBindMode::Localhost,
                                        "Localhost only",
                                        "Keep the daemon private to this machine.",
                                    ))
                                    .child(bind_mode_button(
                                        DaemonBindMode::AllInterfaces,
                                        "All interfaces",
                                        "Allow other machines to connect with a token.",
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child(daemon_helper_text),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(theme.text_muted))
                                            .child("Auth token"),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .h(px(30.))
                                                    .px_2()
                                                    .flex()
                                                    .items_center()
                                                    .rounded_sm()
                                                    .border_1()
                                                    .border_color(rgb(theme.border))
                                                    .bg(rgb(theme.sidebar_bg))
                                                    .text_sm()
                                                    .font_family(FONT_MONO)
                                                    .text_color(rgb(theme.text_disabled))
                                                    .overflow_hidden()
                                                    .whitespace_nowrap()
                                                    .text_ellipsis()
                                                    .child(if modal.daemon_auth_token.is_empty() {
                                                        "(not configured)".to_owned()
                                                    } else {
                                                        modal.daemon_auth_token.clone()
                                                    }),
                                            )
                                            .child(
                                                action_button(
                                                    theme,
                                                    "settings-copy-daemon-auth-token",
                                                    "Copy",
                                                    ActionButtonStyle::Secondary,
                                                    !daemon_auth_token_empty,
                                                )
                                                .when(!daemon_auth_token_empty, |this| {
                                                    this.on_click(cx.listener(|this, _, _, cx| {
                                                        this.copy_settings_daemon_auth_token_to_clipboard(cx);
                                                    }))
                                                }),
                                            ),
                                    ),
                            ),
                    )
                    .child(
                        section_card(div())
                            .child(section_heading("Notifications"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(theme.text_muted))
                                                    .child("Desktop notifications"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(rgb(theme.text_muted))
                                                    .child(
                                                        "Show notices for daemon status and background activity.",
                                                    ),
                                            ),
                                    )
                                    .child(notifications_toggle),
                            ),
                    )
                    .when_some(modal.error.clone(), |this, error| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.notice_text))
                                .bg(rgb(theme.notice_bg))
                                .rounded_sm()
                                .px_2()
                                .py_1()
                                .child(error),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                action_button(
                                    theme,
                                    "settings-cancel",
                                    "Cancel",
                                    ActionButtonStyle::Secondary,
                                    !loading,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_settings_modal(cx);
                                })),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "settings-save",
                                    if loading { "Saving..." } else { "Save" },
                                    ActionButtonStyle::Primary,
                                    !loading,
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.submit_settings_modal(cx);
                                })),
                            ),
                    ),
            )
    }

    pub(crate) fn render_about_modal(&mut self, cx: &mut Context<Self>) -> Div {
        if !self.show_about {
            return div();
        }

        let theme = self.theme();
        let version = APP_VERSION;

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_about = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.show_about = false;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(340.))
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
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_primary))
                                    .child("About Arbor"),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "close-about",
                                    "Close",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.show_about = false;
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(theme.text_primary))
                                    .child(format!("Arbor {version}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child("Git worktree manager"),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_config_save_has_work_when_count_is_nonzero() {
        assert!(!background_config_save_has_work(0));
        assert!(background_config_save_has_work(1));
        assert!(background_config_save_has_work(3));
    }
}
