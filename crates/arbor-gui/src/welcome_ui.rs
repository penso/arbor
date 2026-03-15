use super::*;

impl ArborWindow {
    pub(crate) fn open_add_repository_picker(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select Git Repository".into()),
        });

        cx.spawn(async move |this, cx| {
            let Ok(selection) = picker.await else {
                return;
            };

            let _ = this.update(cx, |this, cx| match selection {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.into_iter().next() {
                        this.add_repository_from_path(path, cx);
                    }
                },
                Ok(None) => {},
                Err(error) => {
                    this.notice = Some(format!("failed to open repository picker: {error}"));
                    cx.notify();
                },
            });
        })
        .detach();
    }

    pub(crate) fn submit_welcome_clone(&mut self, cx: &mut Context<Self>) {
        let url = self.welcome_clone_url.trim().to_owned();
        if url.is_empty() {
            self.welcome_clone_error = Some("Please enter a repository URL".to_owned());
            cx.notify();
            return;
        }
        if self.welcome_cloning {
            return;
        }

        let repo_name = extract_repo_name_from_url(&url);
        if repo_name.is_empty() {
            self.welcome_clone_error =
                Some("Could not determine repository name from URL".to_owned());
            cx.notify();
            return;
        }

        let clone_dir = match user_home_dir() {
            Ok(home) => home.join(".arbor").join("repos").join(&repo_name),
            Err(error) => {
                self.welcome_clone_error = Some(error.to_string());
                cx.notify();
                return;
            },
        };

        if clone_dir.exists() {
            self.add_repository_from_path(clone_dir, cx);
            self.welcome_clone_url.clear();
            self.welcome_clone_url_active = false;
            self.welcome_clone_error = None;
            return;
        }

        self.welcome_cloning = true;
        self.welcome_clone_error = None;
        cx.notify();

        let clone_url = url.clone();
        let target = clone_dir.clone();
        cx.spawn(async move |this, cx| {
            let result = std::thread::spawn(move || {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!("failed to create directory `{}`: {error}", parent.display())
                    })?;
                }
                let output = Command::new("git")
                    .arg("clone")
                    .arg(&clone_url)
                    .arg(&target)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
                    .map_err(|error| format!("failed to run git clone: {error}"))?;

                if output.status.success() {
                    Ok(target)
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Err(format!("git clone failed: {}", stderr.trim()))
                }
            })
            .join()
            .unwrap_or_else(|_| Err("git clone thread panicked".to_owned()));

            let _ = this.update(cx, |this, cx| match result {
                Ok(cloned_path) => {
                    this.welcome_cloning = false;
                    this.welcome_clone_url.clear();
                    this.welcome_clone_url_active = false;
                    this.welcome_clone_error = None;
                    this.add_repository_from_path(cloned_path, cx);
                },
                Err(error) => {
                    this.welcome_cloning = false;
                    this.welcome_clone_error = Some(error);
                    cx.notify();
                },
            });
        })
        .detach();
    }

    pub(crate) fn render_welcome_pane(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let clone_url_active = self.welcome_clone_url_active;
        let cloning = self.welcome_cloning;
        let clone_error = self.welcome_clone_error.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(rgb(theme.app_bg))
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(theme.text_primary))
                    .child("Welcome to Arbor"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(theme.text_muted))
                    .text_center()
                    .max_w(px(460.))
                    .child("Get started by adding a repository. You can open a local git repository or clone one from a URL."),
            )
            .child(
                div()
                    .mt_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(420.))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_muted))
                            .child("CLONE FROM URL"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                single_line_input_field(
                                    theme,
                                    "welcome-clone-url",
                                    &self.welcome_clone_url,
                                    self.welcome_clone_url_cursor,
                                    "https://github.com/user/repo or git@github.com:user/repo.git",
                                    clone_url_active,
                                )
                                .track_focus(&self.welcome_clone_focus)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                        window.focus(&this.welcome_clone_focus);
                                        this.welcome_clone_url_active = true;
                                        this.welcome_clone_url_cursor =
                                            char_count(&this.welcome_clone_url);
                                        cx.notify();
                                    }),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.welcome_clone_url_active = true;
                                    this.welcome_clone_url_cursor =
                                        char_count(&this.welcome_clone_url);
                                    cx.notify();
                                })),
                            )
                            .when_some(clone_error, |this, error| {
                                this.child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(theme.notice_text))
                                        .child(error),
                                )
                            })
                            .child(
                                action_button(
                                    theme,
                                    "welcome-clone-button",
                                    if cloning { "Cloning..." } else { "Clone Repository" },
                                    ActionButtonStyle::Primary,
                                    !cloning,
                                )
                                .when(!cloning, |this| {
                                    this.on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_welcome_clone(cx);
                                    }))
                                }),
                            ),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().h(px(1.)).bg(rgb(theme.border)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(theme.text_disabled))
                                    .child("or"),
                            )
                            .child(div().flex_1().h(px(1.)).bg(rgb(theme.border))),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text_muted))
                            .child("LOCAL REPOSITORY"),
                    )
                    .child(
                        action_button(
                            theme,
                            "welcome-add-local",
                            "Open Local Repository",
                            ActionButtonStyle::Secondary,
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_add_repository_picker(cx);
                        })),
                    ),
            )
    }
}
