#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TopBarIconKind {
    RemoteControl,
    GitHub,
    WorktreeActions,
    ReportIssue,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TopBarIconTone {
    Muted,
    Disabled,
    Connected,
    Busy,
}

impl ArborWindow {
    fn render_top_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let repository = self.selected_repository_label();
        let branch = self
            .active_worktree()
            .map(|worktree| worktree.branch.clone())
            .unwrap_or_else(|| "no-worktree".to_owned());
        let centered_title = format!("{repository} · {branch}");
        let back_enabled = !self.worktree_nav_back.is_empty();
        let forward_enabled = !self.worktree_nav_forward.is_empty();
        let sidebar_hidden = !self.left_pane_visible;
        let worktree_quick_actions_enabled = self.selected_local_worktree_path().is_some();
        let worktree_quick_actions_open =
            worktree_quick_actions_enabled && self.top_bar_quick_actions_open;
        let github_saved_token = self.has_persisted_github_token();
        let github_env_token = github_access_token_from_env().is_some();
        let github_auth_busy = self.github_auth_in_progress;
        let github_auth_label = if github_auth_busy {
            "Authorizing"
        } else if github_saved_token {
            "Disconnect"
        } else if github_env_token {
            "Connected (env)"
        } else {
            "Sign in"
        };
        let github_auth_icon_color = if github_auth_busy {
            theme.accent
        } else if github_saved_token || github_env_token {
            0x68c38d
        } else {
            theme.text_muted
        };
        let github_auth_text_color = if github_auth_busy || github_saved_token || github_env_token {
            theme.text_primary
        } else {
            theme.text_muted
        };
        let github_avatar_url = self.github_auth_state.user_avatar_url.clone();

        div()
            .h(px(TITLEBAR_HEIGHT))
            .bg(rgb(theme.chrome_bg))
            .window_control_area(WindowControlArea::Drag)
            .relative()
            .flex()
            .items_center()
            .child(
                div()
                    .absolute()
                    .left(px(TOP_BAR_LEFT_OFFSET))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .child(
                        div()
                            .id("toggle-sidebar")
                            .cursor_pointer()
                            .font_family(FONT_MONO)
                            .text_size(px(20.))
                            .text_color(rgb(if sidebar_hidden {
                                theme.accent
                            } else {
                                theme.text_muted
                            }))
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(theme.border))
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.action_toggle_left_pane(&ToggleLeftPane, window, cx);
                            }))
                            .child("\u{f0c9}"),
                    )
                    .child(
                        div()
                            .id("nav-back")
                            .cursor_pointer()
                            .font_family(FONT_MONO)
                            .text_size(px(20.))
                            .text_color(rgb(if back_enabled {
                                theme.text_primary
                            } else {
                                theme.text_disabled
                            }))
                            .hover(|this| this.text_color(rgb(theme.accent)))
                            .when(back_enabled, |this| {
                                this.on_click(cx.listener(|this, _, window, cx| {
                                    this.action_navigate_worktree_back(
                                        &NavigateWorktreeBack,
                                        window,
                                        cx,
                                    );
                                }))
                            })
                            .child("\u{f053}"),
                    )
                    .child(
                        div()
                            .id("nav-forward")
                            .cursor_pointer()
                            .font_family(FONT_MONO)
                            .text_size(px(20.))
                            .text_color(rgb(if forward_enabled {
                                theme.text_primary
                            } else {
                                theme.text_disabled
                            }))
                            .hover(|this| this.text_color(rgb(theme.accent)))
                            .when(forward_enabled, |this| {
                                this.on_click(cx.listener(|this, _, window, cx| {
                                    this.action_navigate_worktree_forward(
                                        &NavigateWorktreeForward,
                                        window,
                                        cx,
                                    );
                                }))
                            })
                            .child("\u{f054}"),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_primary))
                            .child(centered_title),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .right(px(16.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child({
                        let daemon_connected = self.terminal_daemon.is_some();
                        let web_ui_url = self.daemon_base_url.clone();
                        top_bar_button(
                            theme,
                            "web-ui-link",
                            true,
                            if daemon_connected {
                                theme.text_muted
                            } else {
                                theme.text_disabled
                            },
                            theme.text_primary,
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(11.))
                                .child(top_bar_icon_element(
                                    TopBarIconKind::RemoteControl,
                                    if daemon_connected {
                                        TopBarIconTone::Connected
                                    } else {
                                        TopBarIconTone::Disabled
                                    },
                                    if daemon_connected {
                                        0x68c38d
                                    } else {
                                        theme.text_disabled
                                    },
                                    "\u{f0ac}",
                                ))
                                .child("Remote Control"),
                        )
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            if this.terminal_daemon.is_some() {
                                this.open_external_url(&web_ui_url, cx);
                            } else {
                                this.start_daemon_modal = true;
                                cx.notify();
                            }
                        }))
                    })
                    .child(
                        top_bar_button(
                            theme,
                            "github-auth",
                            !github_auth_busy,
                            github_auth_text_color,
                            theme.text_primary,
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(11.))
                                .child(match github_avatar_url {
                                    Some(url) => div()
                                        .size(px(12.))
                                        .rounded_full()
                                        .overflow_hidden()
                                        .child(img(url).size_full().rounded_full().with_fallback(
                                            move || {
                                                top_bar_icon_element(
                                                    TopBarIconKind::GitHub,
                                                    if github_auth_busy {
                                                        TopBarIconTone::Busy
                                                    } else if github_saved_token || github_env_token {
                                                        TopBarIconTone::Connected
                                                    } else {
                                                        TopBarIconTone::Muted
                                                    },
                                                    github_auth_icon_color,
                                                    "\u{f09b}",
                                                )
                                                .into_any_element()
                                            },
                                        ))
                                        .into_any_element(),
                                    None => top_bar_icon_element(
                                        TopBarIconKind::GitHub,
                                        if github_auth_busy {
                                            TopBarIconTone::Busy
                                        } else if github_saved_token || github_env_token {
                                            TopBarIconTone::Connected
                                        } else {
                                            TopBarIconTone::Muted
                                        },
                                        github_auth_icon_color,
                                        "\u{f09b}",
                                    )
                                    .into_any_element(),
                                })
                                .child(github_auth_label),
                        )
                        .when(!github_auth_busy, |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.run_github_auth_button_action(cx);
                            }))
                        }),
                    )
                    .child(
                        top_bar_button(
                            theme,
                            "worktree-quick-actions",
                            worktree_quick_actions_enabled,
                            if worktree_quick_actions_enabled {
                                theme.text_muted
                            } else {
                                theme.text_disabled
                            },
                            theme.text_primary,
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(top_bar_icon_element(
                                    TopBarIconKind::WorktreeActions,
                                    if worktree_quick_actions_enabled {
                                        TopBarIconTone::Muted
                                    } else {
                                        TopBarIconTone::Disabled
                                    },
                                    if worktree_quick_actions_enabled {
                                        theme.text_muted
                                    } else {
                                        theme.text_disabled
                                    },
                                    "\u{f0e7}",
                                ))
                                .child(div().text_size(px(11.)).child("Action"))
                                .child(
                                    div()
                                        .font_family(FONT_MONO)
                                        .text_size(px(9.))
                                        .child(if worktree_quick_actions_open {
                                            "\u{f077}"
                                        } else {
                                            "\u{f078}"
                                        }),
                                ),
                        )
                        .when(worktree_quick_actions_enabled, |this| {
                            this.on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_top_bar_worktree_quick_actions_menu(cx);
                            }))
                        }),
                    )
                    .child(
                        top_bar_button(
                            theme,
                            "report-issue",
                            true,
                            theme.text_muted,
                            theme.text_primary,
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(11.))
                                .child(top_bar_icon_element(
                                    TopBarIconKind::ReportIssue,
                                    TopBarIconTone::Muted,
                                    theme.text_muted,
                                    "\u{f188}",
                                ))
                                .child("Report issue"),
                        )
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.close_top_bar_worktree_quick_actions();
                            cx.open_url("https://github.com/penso/arbor/issues/new");
                        })),
                    ),
            )
    }

    fn render_top_bar_worktree_quick_actions_menu(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let menu_open =
            self.top_bar_quick_actions_open && self.selected_local_worktree_path().is_some();

        if !menu_open {
            return div();
        }

        let ide_has_launchers = !self.ide_launchers.is_empty();
        let submenu = self.top_bar_quick_actions_submenu;
        let ide_row_active = submenu == Some(QuickActionSubmenu::Ide);

        let mut overlay = div()
            .absolute()
            .right(px(16.))
            .top(px(TITLEBAR_HEIGHT))
            .mt(px(4.))
            .child(
                div()
                    .w(px(192.))
                    .py(px(4.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.chrome_bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .id("quick-action-open-finder")
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_worktree_quick_action(WorktreeQuickAction::OpenFinder, cx);
                            }))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_color(rgb(0xe5c07b))
                                    .child("\u{f07b}"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Open in Finder"),
                            ),
                    )
                    .child(
                        div()
                            .id("quick-action-open-ide-submenu")
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .text_color(rgb(if ide_has_launchers {
                                theme.text_primary
                            } else {
                                theme.text_disabled
                            }))
                            .when(ide_has_launchers, |this| {
                                this.cursor_pointer()
                                    .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_top_bar_worktree_quick_actions_submenu(
                                            QuickActionSubmenu::Ide,
                                            cx,
                                        );
                                    }))
                            })
                            .when(ide_row_active, |this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .font_family(FONT_MONO)
                                            .text_size(px(12.))
                                            .text_color(rgb(0x39a0ed))
                                            .child("\u{f121}"),
                                    )
                                    .child(div().text_size(px(11.)).child("IDE")),
                            )
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(10.))
                                    .text_color(rgb(if ide_has_launchers {
                                        theme.text_muted
                                    } else {
                                        theme.text_disabled
                                    }))
                                    .child("\u{f054}"),
                            ),
                    )
                    .child(div().h(px(1.)).mx(px(8.)).my(px(4.)).bg(rgb(theme.border)))
                    .child(
                        div()
                            .id("quick-action-copy-path")
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.run_worktree_quick_action(WorktreeQuickAction::CopyPath, cx);
                            }))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_color(rgb(theme.text_muted))
                                    .child("\u{f0c5}"),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme.text_primary))
                                    .child("Copy path"),
                            ),
                    ),
            );

        if let Some(submenu) = submenu {
            let launchers: &[ExternalLauncher] = match submenu {
                QuickActionSubmenu::Ide => &self.ide_launchers,
            };
            if launchers.is_empty() {
                return overlay;
            }
            let submenu_top = match submenu {
                QuickActionSubmenu::Ide => px(28.),
            };

            overlay = overlay.child(
                div()
                    .id("quick-action-launcher-submenu")
                    .absolute()
                    .right(px(200.))
                    .top(submenu_top)
                    .w(px(220.))
                    .py(px(4.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.chrome_bg))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .children(launchers.iter().enumerate().map(|(index, launcher)| {
                        let launcher = *launcher;
                        div()
                            .id(ElementId::NamedInteger(
                                "quick-action-launcher-item".into(),
                                index as u64,
                            ))
                            .h(px(24.))
                            .mx(px(4.))
                            .px(px(8.))
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_worktree_external_launcher(index, cx);
                            }))
                            .child(
                                div()
                                    .w(px(20.))
                                    .flex_none()
                                    .font_family(FONT_MONO)
                                    .text_size(px(12.))
                                    .text_center()
                                    .text_color(rgb(launcher.icon_color))
                                    .child(launcher.icon),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme.text_primary))
                                    .child(launcher.label),
                            )
                    })),
            );
        }

        div()
            .absolute()
            .inset_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.close_top_bar_worktree_quick_actions();
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(overlay)
    }
}

fn top_bar_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    enabled: bool,
    base_text_color: u32,
    hover_text_color: u32,
    content: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(22.))
        .px(px(6.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(theme.chrome_bg))
        .text_color(rgb(base_text_color))
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| {
                    this.bg(rgb(theme.panel_bg))
                        .text_color(rgb(hover_text_color))
                        .border_color(rgb(theme.panel_active_bg))
                })
                .active(|this| {
                    this.bg(rgb(theme.panel_active_bg))
                        .text_color(rgb(hover_text_color))
                })
        })
        .child(content)
}

fn top_bar_icon_asset_path(kind: TopBarIconKind, tone: TopBarIconTone) -> Option<PathBuf> {
    let file_name = match (kind, tone) {
        (TopBarIconKind::RemoteControl, TopBarIconTone::Connected) => {
            "remote-control-connected.svg"
        },
        (TopBarIconKind::RemoteControl, TopBarIconTone::Disabled) => "remote-control-disabled.svg",
        (TopBarIconKind::GitHub, TopBarIconTone::Muted) => "github-muted.svg",
        (TopBarIconKind::GitHub, TopBarIconTone::Connected) => "github-connected.svg",
        (TopBarIconKind::GitHub, TopBarIconTone::Busy) => "github-busy.svg",
        (TopBarIconKind::WorktreeActions, TopBarIconTone::Muted) => "worktree-actions-enabled.svg",
        (TopBarIconKind::WorktreeActions, TopBarIconTone::Disabled) => {
            "worktree-actions-disabled.svg"
        },
        (TopBarIconKind::ReportIssue, TopBarIconTone::Muted) => "report-issue.svg",
        _ => return None,
    };

    find_top_bar_icons_dir().map(|dir| dir.join(file_name))
}

fn top_bar_icon_size_px(kind: TopBarIconKind) -> f32 {
    match kind {
        TopBarIconKind::GitHub => 10.5,
        TopBarIconKind::RemoteControl
        | TopBarIconKind::WorktreeActions
        | TopBarIconKind::ReportIssue => 12.0,
    }
}

fn top_bar_icon_element(
    kind: TopBarIconKind,
    tone: TopBarIconTone,
    fallback_color: u32,
    fallback_glyph: &'static str,
) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(match top_bar_icon_asset_path(kind, tone) {
            Some(path) => img(path)
                .size(px(top_bar_icon_size_px(kind)))
                .with_fallback(move || {
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .line_height(px(12.))
                        .text_color(rgb(fallback_color))
                        .child(fallback_glyph)
                        .into_any_element()
                })
                .into_any_element(),
            None => div()
                .font_family(FONT_MONO)
                .text_size(px(12.))
                .line_height(px(12.))
                .text_color(rgb(fallback_color))
                .child(fallback_glyph)
                .into_any_element(),
        })
}
