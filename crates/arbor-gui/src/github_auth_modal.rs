use super::*;

impl ArborWindow {
    pub(crate) fn close_github_auth_modal(&mut self, cx: &mut Context<Self>) {
        self.github_auth_copy_feedback_active = false;
        if self.github_auth_modal.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn copy_github_auth_code_to_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.github_auth_modal.as_ref() else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(modal.user_code.clone()));
        self.github_auth_copy_feedback_active = true;
        self.github_auth_copy_feedback_generation =
            self.github_auth_copy_feedback_generation.saturating_add(1);
        let generation = self.github_auth_copy_feedback_generation;
        self.notice = Some("GitHub device code copied to clipboard".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                std::thread::sleep(GITHUB_AUTH_COPY_FEEDBACK_DURATION);
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                if this.github_auth_copy_feedback_generation == generation
                    && this.github_auth_copy_feedback_active
                {
                    this.github_auth_copy_feedback_active = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn open_github_auth_verification_page(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.github_auth_modal.as_ref() else {
            return;
        };

        let url = modal.verification_url.clone();
        self.open_external_url(&url, cx);
    }

    pub(crate) fn render_github_auth_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.github_auth_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let copy_feedback_active = self.github_auth_copy_feedback_active;
        let copy_label = if copy_feedback_active {
            "Copied"
        } else {
            "Copy code"
        };
        let status_line = if self.github_auth_in_progress {
            "Waiting for GitHub authorization..."
        } else {
            "Authorization complete."
        };
        let detail_line = if self.github_auth_in_progress {
            "Arbor will continue automatically after you approve access."
        } else {
            "You can close this dialog."
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
                    this.close_github_auth_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.close_github_auth_modal(cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(560.))
                    .max_w(px(560.))
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
                                    .child("GitHub Sign In"),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "close-github-auth-modal",
                                    "Close",
                                    ActionButtonStyle::Secondary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.close_github_auth_modal(cx);
                                    },
                                )),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child("1. Open GitHub and enter this device code."),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child("2. Return here after approving Arbor."),
                    )
                    .child(
                        div()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.panel_bg))
                            .p_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child("Device code"),
                            )
                            .child(
                                div()
                                    .pt_1()
                                    .text_lg()
                                    .font_family(FONT_MONO)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.text_primary))
                                    .child(modal.user_code),
                            ),
                    )
                    .child(
                        div()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.panel_bg))
                            .p_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .child("Verification URL"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family(FONT_MONO)
                                    .text_color(rgb(theme.text_primary))
                                    .child(modal.verification_url),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if copy_feedback_active {
                                0x68c38d
                            } else {
                                theme.accent
                            }))
                            .child(if copy_feedback_active {
                                "Code copied to clipboard"
                            } else {
                                status_line
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(detail_line),
                    )
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
                                    "github-auth-copy-code",
                                    copy_label,
                                    if copy_feedback_active {
                                        ActionButtonStyle::Primary
                                    } else {
                                        ActionButtonStyle::Secondary
                                    },
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.copy_github_auth_code_to_clipboard(cx);
                                    },
                                )),
                            )
                            .child(
                                action_button(
                                    theme,
                                    "github-auth-open",
                                    "Open GitHub",
                                    ActionButtonStyle::Primary,
                                    true,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.open_github_auth_verification_page(cx);
                                    },
                                )),
                            ),
                    ),
            )
    }
}
