use super::*;

impl ArborWindow {
    pub(crate) fn open_manage_hosts_modal(&mut self, cx: &mut Context<Self>) {
        self.manage_hosts_modal = Some(ManageHostsModal {
            adding: false,
            name: String::new(),
            name_cursor: 0,
            hostname: String::new(),
            hostname_cursor: 0,
            user: String::new(),
            user_cursor: 0,
            active_field: ManageHostsField::Name,
            error: None,
            saving: false,
        });
        cx.notify();
    }

    pub(crate) fn close_manage_hosts_modal(&mut self, cx: &mut Context<Self>) {
        self.manage_hosts_modal = None;
        cx.notify();
    }

    pub(crate) fn update_manage_hosts_modal_input(
        &mut self,
        input: HostsModalInputEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(modal) = self.manage_hosts_modal.as_mut() else {
            return;
        };

        match input {
            HostsModalInputEvent::SetActiveField(field) => {
                modal.active_field = field;
                match field {
                    ManageHostsField::Name => modal.name_cursor = char_count(&modal.name),
                    ManageHostsField::Hostname => {
                        modal.hostname_cursor = char_count(&modal.hostname);
                    },
                    ManageHostsField::User => modal.user_cursor = char_count(&modal.user),
                }
            },
            HostsModalInputEvent::MoveActiveField(reverse) => {
                modal.active_field = match (modal.active_field, reverse) {
                    (ManageHostsField::Name, false) => ManageHostsField::Hostname,
                    (ManageHostsField::Hostname, false) => ManageHostsField::User,
                    (ManageHostsField::User, false) => ManageHostsField::Name,
                    (ManageHostsField::Name, true) => ManageHostsField::User,
                    (ManageHostsField::Hostname, true) => ManageHostsField::Name,
                    (ManageHostsField::User, true) => ManageHostsField::Hostname,
                };
            },
            HostsModalInputEvent::Edit(action) => match modal.active_field {
                ManageHostsField::Name => {
                    apply_text_edit_action(&mut modal.name, &mut modal.name_cursor, &action);
                },
                ManageHostsField::Hostname => {
                    apply_text_edit_action(
                        &mut modal.hostname,
                        &mut modal.hostname_cursor,
                        &action,
                    );
                },
                ManageHostsField::User => {
                    apply_text_edit_action(&mut modal.user, &mut modal.user_cursor, &action);
                },
            },
            HostsModalInputEvent::ClearError => {
                modal.error = None;
            },
        }

        cx.notify();
    }

    pub(crate) fn submit_add_host(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.manage_hosts_modal.clone() else {
            return;
        };
        if modal.saving {
            return;
        }
        let name = modal.name.trim().to_owned();
        let hostname = modal.hostname.trim().to_owned();
        let user = modal.user.trim().to_owned();

        if name.is_empty() || hostname.is_empty() || user.is_empty() {
            if let Some(modal) = self.manage_hosts_modal.as_mut() {
                modal.error = Some("All fields are required.".to_owned());
            }
            cx.notify();
            return;
        }

        if self.remote_hosts.iter().any(|host| host.name == name) {
            if let Some(modal) = self.manage_hosts_modal.as_mut() {
                modal.error = Some(format!("Host \"{name}\" already exists."));
            }
            cx.notify();
            return;
        }

        let host_config = app_config::RemoteHostConfig {
            name: name.clone(),
            hostname,
            user,
            port: 22,
            identity_file: None,
            remote_base_path: "~/arbor-outposts".to_owned(),
            daemon_port: None,
            mosh: None,
            mosh_server_path: None,
        };

        if let Some(modal) = self.manage_hosts_modal.as_mut() {
            modal.error = None;
            modal.saving = true;
        }
        let store = self.app_config_store.clone();
        self.begin_background_config_save();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.append_remote_host(&host_config) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.config_last_modified = None;
                        this.refresh_config_if_changed(cx);
                        this.notice = Some(format!("Host \"{name}\" added."));
                        if let Some(modal) = this.manage_hosts_modal.as_mut() {
                            modal.adding = false;
                            modal.name.clear();
                            modal.name_cursor = 0;
                            modal.hostname.clear();
                            modal.hostname_cursor = 0;
                            modal.user.clear();
                            modal.user_cursor = 0;
                            modal.error = None;
                            modal.saving = false;
                        }
                    },
                    Err(error) => {
                        let error = error.to_string();
                        if let Some(modal) = this.manage_hosts_modal.as_mut() {
                            modal.error = Some(error);
                            modal.saving = false;
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

    pub(crate) fn remove_host_at(&mut self, host_name: String, cx: &mut Context<Self>) {
        let store = self.app_config_store.clone();
        let host_name_for_save = host_name.clone();
        self.begin_background_config_save();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { store.remove_remote_host(&host_name_for_save) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.config_last_modified = None;
                        this.refresh_config_if_changed(cx);
                        this.notice = Some(format!("Host \"{host_name}\" removed."));
                    },
                    Err(error) => {
                        this.notice = Some(error.to_string());
                    },
                }
                this.finish_background_config_save(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_manage_hosts_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.manage_hosts_modal.clone() else {
            return div();
        };

        let theme = self.theme();

        if modal.adding {
            let name_active = modal.active_field == ManageHostsField::Name;
            let hostname_active = modal.active_field == ManageHostsField::Hostname;
            let user_active = modal.active_field == ManageHostsField::User;
            let add_disabled = modal.name.trim().is_empty()
                || modal.hostname.trim().is_empty()
                || modal.user.trim().is_empty()
                || modal.saving;

            return div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_manage_hosts_modal(cx);
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.close_manage_hosts_modal(cx);
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
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(theme.text_primary))
                                        .child("Add Host"),
                                )
                                .child(
                                    action_button(
                                        theme,
                                        "back-manage-hosts",
                                        "Back",
                                        ActionButtonStyle::Secondary,
                                        !modal.saving,
                                    )
                                    .when(
                                        !modal.saving,
                                        |this| {
                                            this.on_click(cx.listener(|this, _, _, cx| {
                                                if let Some(modal) =
                                                    this.manage_hosts_modal.as_mut()
                                                {
                                                    modal.adding = false;
                                                    modal.error = None;
                                                    cx.notify();
                                                }
                                            }))
                                        },
                                    ),
                                ),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "hosts-name-input",
                                "Name",
                                &modal.name,
                                modal.name_cursor,
                                "e.g. build-server",
                                name_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_hosts_modal_input(
                                    HostsModalInputEvent::SetActiveField(ManageHostsField::Name),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "hosts-hostname-input",
                                "Hostname",
                                &modal.hostname,
                                modal.hostname_cursor,
                                "e.g. build.example.com",
                                hostname_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_hosts_modal_input(
                                    HostsModalInputEvent::SetActiveField(
                                        ManageHostsField::Hostname,
                                    ),
                                    cx,
                                );
                            })),
                        )
                        .child(
                            modal_input_field(
                                theme,
                                "hosts-user-input",
                                "User",
                                &modal.user,
                                modal.user_cursor,
                                "e.g. dev",
                                user_active,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_manage_hosts_modal_input(
                                    HostsModalInputEvent::SetActiveField(ManageHostsField::User),
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
                                        "cancel-add-host",
                                        "Cancel",
                                        ActionButtonStyle::Secondary,
                                        !modal.saving,
                                    )
                                    .when(
                                        !modal.saving,
                                        |this| {
                                            this.on_click(cx.listener(|this, _, _, cx| {
                                                if let Some(modal) =
                                                    this.manage_hosts_modal.as_mut()
                                                {
                                                    modal.adding = false;
                                                    modal.error = None;
                                                    cx.notify();
                                                }
                                            }))
                                        },
                                    ),
                                )
                                .child(
                                    action_button(
                                        theme,
                                        "submit-add-host",
                                        "Add Host",
                                        ActionButtonStyle::Primary,
                                        !add_disabled,
                                    )
                                    .when(
                                        !add_disabled,
                                        |this| {
                                            this.on_click(cx.listener(|this, _, _, cx| {
                                                this.submit_add_host(cx);
                                            }))
                                        },
                                    ),
                                ),
                        ),
                );
        }

        let hosts = self.remote_hosts.clone();
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_hosts_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_manage_hosts_modal(cx);
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
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_primary))
                                    .child("Manage Hosts"),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "close-manage-hosts",
                                    "Close",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_manage_hosts_modal(cx);
                                    },
                                )),
                            ),
                    )
                    .child(if hosts.is_empty() {
                        div()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.panel_bg))
                            .p_3()
                            .text_sm()
                            .text_color(rgb(theme.text_muted))
                            .child("No remote hosts configured.")
                            .into_any_element()
                    } else {
                        let mut list = div()
                            .id("manage-hosts-list")
                            .flex()
                            .flex_col()
                            .gap_1()
                            .max_h(px(300.))
                            .overflow_y_scroll();
                        for (i, host) in hosts.iter().enumerate() {
                            let host_name = host.name.clone();
                            let display = format!("{}@{}", host.user, host.hostname);
                            list = list.child(
                                div()
                                    .id(ElementId::NamedInteger("host-row".into(), i as u64))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(theme.border))
                                    .bg(rgb(theme.panel_bg))
                                    .px_2()
                                    .py_1()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(rgb(theme.text_primary))
                                                    .child(host.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_family(FONT_MONO)
                                                    .text_color(rgb(theme.text_muted))
                                                    .child(display),
                                            ),
                                    )
                                    .child(
                                        action_button(
                                            theme,
                                            ElementId::NamedInteger("remove-host".into(), i as u64),
                                            "Remove",
                                            ActionButtonStyle::Secondary,
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.remove_host_at(host_name.clone(), cx);
                                            }),
                                        ),
                                    ),
                            );
                        }
                        list.into_any_element()
                    })
                    .child(
                        div().flex().items_center().justify_end().child(
                            action_button(
                                theme,
                                "open-add-host-form",
                                "+ Add Host",
                                ActionButtonStyle::Primary,
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(modal) = this.manage_hosts_modal.as_mut() {
                                    modal.adding = true;
                                    modal.name.clear();
                                    modal.hostname.clear();
                                    modal.user.clear();
                                    modal.active_field = ManageHostsField::Name;
                                    modal.error = None;
                                    cx.notify();
                                }
                            })),
                        ),
                    ),
            )
    }
}
