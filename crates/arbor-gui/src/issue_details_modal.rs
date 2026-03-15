use super::*;

impl ArborWindow {
    pub(crate) fn open_issue_details_modal_for_target(
        &mut self,
        target: IssueTarget,
        source_label: String,
        issue: terminal_daemon_http::IssueDto,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw_body = issue.body.as_deref();
        let normalized_body = issue_body_text(raw_body);
        let uses_fallback = normalized_body == ISSUE_DESCRIPTION_FALLBACK;
        let preview = issue_body_log_preview(&normalized_body);
        if uses_fallback {
            tracing::warn!(
                repo_root = %target.repo_root,
                daemon_target = ?target.daemon_target,
                issue_id = %issue.display_id,
                title = %issue.title,
                raw_body_present = raw_body.is_some(),
                raw_body_len = raw_body.map_or(0, str::len),
                normalized_body_len = normalized_body.len(),
                preview = %preview,
                "opening issue details modal without issue body content"
            );
        } else {
            tracing::info!(
                repo_root = %target.repo_root,
                daemon_target = ?target.daemon_target,
                issue_id = %issue.display_id,
                title = %issue.title,
                raw_body_present = raw_body.is_some(),
                raw_body_len = raw_body.map_or(0, str::len),
                normalized_body_len = normalized_body.len(),
                preview = %preview,
                "opening issue details modal"
            );
        }

        self.create_modal = None;
        self.issue_details_modal = Some(IssueDetailsModal {
            target,
            source_label,
            issue,
        });
        window.focus(&self.issue_details_focus);
        cx.notify();
    }

    pub(crate) fn close_issue_details_modal(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        self.issue_details_modal = None;
        self.issue_details_scrollbar_drag_offset = None;
        if let Some(window) = window {
            window.focus(&self.terminal_focus);
        }
        cx.notify();
    }

    pub(crate) fn open_create_modal_from_issue_details(&mut self, cx: &mut Context<Self>) {
        let Some(modal) = self.issue_details_modal.clone() else {
            return;
        };
        self.issue_details_modal = None;
        self.open_issue_create_modal_for_target(modal.target, modal.source_label, modal.issue, cx);
    }

    pub(crate) fn render_issue_details_modal(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(modal) = self.issue_details_modal.clone() else {
            return div();
        };

        let theme = self.theme();
        let issue = modal.issue;
        let issue_url = issue.url.clone();
        let issue_number = issue.display_id.clone();
        let issue_state = issue.state.clone();
        let updated_at = issue.updated_at.clone();
        let linked_review = issue.linked_review.clone();
        let linked_branch = issue.linked_branch.clone();
        let title = issue.title.clone();
        let issue_has_body = issue
            .body
            .as_deref()
            .is_some_and(|body| !body.trim().is_empty());
        let source_label = modal.source_label;
        let issue_heading = format!("Issue {issue_number}");
        let issue_description = issue_body_text(issue.body.as_deref());
        let issue_description_lines: Vec<String> = issue_description
            .lines()
            .map(|line| {
                if line.is_empty() {
                    " ".to_owned()
                } else {
                    line.to_owned()
                }
            })
            .collect();
        let mut description_body =
            div().relative().w_full().h_full().child(
                div()
                    .id("issue-details-description-body")
                    .w_full()
                    .h_full()
                    .overflow_y_scroll()
                    .scrollbar_width(px(10.))
                    .track_scroll(&self.issue_details_scroll_handle)
                    .pr(px(18.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(issue_description_lines.into_iter().enumerate().map(
                        |(index, line)| {
                            div()
                                .id(("issue-details-line", index))
                                .text_sm()
                                .whitespace_normal()
                                .text_color(rgb(if issue_has_body {
                                    theme.text_primary
                                } else {
                                    theme.text_muted
                                }))
                                .child(line)
                        },
                    )),
            );

        if let Some(scrollbar) =
            issue_details_scrollbar_indicator(&self.issue_details_scroll_handle, theme, cx)
        {
            description_body = description_body.child(scrollbar);
        }

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_issue_details_modal(Some(window), cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    this.close_issue_details_modal(Some(window), cx);
                    cx.stop_propagation();
                }),
            )
            .child(modal_backdrop())
            .child(
                div()
                    .w(px(680.))
                    .max_w(px(680.))
                    .h(px(640.))
                    .max_h(px(640.))
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
                    .track_focus(&self.issue_details_focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                this.close_issue_details_modal(Some(window), cx);
                                cx.stop_propagation();
                            },
                            "enter" | "return" => {
                                this.open_create_modal_from_issue_details(cx);
                                cx.stop_propagation();
                            },
                            _ => {},
                        }
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_mouse_down(MouseButton::Right, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div().flex_none().flex().items_start().gap_3().child(
                            div()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(theme.text_muted))
                                        .child(issue_heading),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_lg()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .line_clamp(3)
                                        .text_color(rgb(theme.text_primary))
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .flex_wrap()
                                        .child(issue_meta_chip(
                                            issue_state,
                                            theme.text_primary,
                                            theme.panel_bg,
                                            false,
                                            None,
                                            cx,
                                        ))
                                        .child(issue_meta_chip(
                                            source_label,
                                            theme.text_muted,
                                            theme.panel_bg,
                                            false,
                                            None,
                                            cx,
                                        ))
                                        .when_some(updated_at.clone(), |this, updated_at| {
                                            this.child(issue_meta_chip(
                                                issue_updated_label(&updated_at),
                                                theme.text_muted,
                                                theme.panel_bg,
                                                false,
                                                None,
                                                cx,
                                            ))
                                        }),
                                ),
                        ),
                    )
                    .when(linked_review.is_some() || linked_branch.is_some(), |this| {
                        this.child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_2()
                                .flex_wrap()
                                .when_some(linked_review.clone(), |this, review| {
                                    let review_color = match review.kind {
                                        terminal_daemon_http::IssueReviewKind::PullRequest => {
                                            theme.accent
                                        },
                                        terminal_daemon_http::IssueReviewKind::MergeRequest => {
                                            0x72d69c
                                        },
                                    };
                                    this.child(issue_meta_chip(
                                        review.label,
                                        review_color,
                                        theme.panel_active_bg,
                                        review.url.is_some(),
                                        review.url,
                                        cx,
                                    ))
                                })
                                .when_some(linked_branch.clone(), |this, branch| {
                                    this.child(issue_meta_chip(
                                        branch,
                                        theme.text_primary,
                                        theme.panel_bg,
                                        false,
                                        None,
                                        cx,
                                    ))
                                }),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .h(px(380.))
                            .w_full()
                            .child(description_body),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("issue-details-cancel")
                                    .cursor_pointer()
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(rgb(theme.border))
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(rgb(theme.text_primary))
                                    .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_issue_details_modal(Some(window), cx);
                                        cx.stop_propagation();
                                    })),
                            )
                            .when_some(issue_url, |this, issue_url| {
                                this.child(
                                    div()
                                        .id("issue-details-open-browser")
                                        .cursor_pointer()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .px_2()
                                        .py_1()
                                        .text_xs()
                                        .text_color(rgb(theme.text_primary))
                                        .hover(|this| this.bg(rgb(theme.panel_active_bg)))
                                        .child("Open in Browser")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_external_url(&issue_url, cx);
                                            cx.stop_propagation();
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("issue-details-create-worktree")
                                    .cursor_pointer()
                                    .rounded_sm()
                                    .bg(rgb(theme.accent))
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme.sidebar_bg))
                                    .hover(|this| this.opacity(0.92))
                                    .child("Create Worktree")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_create_modal_from_issue_details(cx);
                                        cx.stop_propagation();
                                    })),
                            ),
                    ),
            )
    }
}

pub(crate) fn issue_details_scrollbar_indicator(
    scroll_handle: &ScrollHandle,
    theme: ThemePalette,
    cx: &mut Context<ArborWindow>,
) -> Option<Div> {
    let scroll_handle_for_draw = scroll_handle.clone();
    let scroll_handle_for_click = scroll_handle.clone();
    let scroll_handle_for_drag = scroll_handle.clone();
    let entity = cx.entity();

    Some(
        div()
            .absolute()
            .top(px(4.))
            .right(px(4.))
            .bottom(px(4.))
            .w(px(8.))
            .cursor_pointer()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let Some(metrics) =
                            issue_details_scrollbar_metrics(&scroll_handle_for_draw, bounds)
                        else {
                            return;
                        };

                        window.paint_quad(fill(metrics.track_bounds, rgb(theme.panel_bg)));
                        window.paint_quad(fill(metrics.thumb_bounds, rgb(theme.accent)));

                        window.on_mouse_event({
                            let entity = entity.clone();
                            let scroll_handle = scroll_handle_for_click.clone();
                            move |event: &MouseDownEvent, _, _, cx| {
                                let Some(metrics) =
                                    issue_details_scrollbar_metrics(&scroll_handle, bounds)
                                else {
                                    return;
                                };
                                if !metrics.track_bounds.contains(&event.position) {
                                    return;
                                }

                                if metrics.thumb_bounds.contains(&event.position) {
                                    let anchor = event.position.y - metrics.thumb_bounds.origin.y;
                                    entity.update(cx, |this, cx| {
                                        this.issue_details_scrollbar_drag_offset = Some(anchor);
                                        cx.notify();
                                    });
                                } else {
                                    let centered_anchor =
                                        px(metrics.thumb_bounds.size.height.to_f64() as f32 / 2.);
                                    issue_details_set_scroll_offset(
                                        &scroll_handle,
                                        metrics.track_bounds,
                                        metrics.thumb_bounds.size.height,
                                        event.position.y,
                                        centered_anchor,
                                    );
                                    entity.update(cx, |this, cx| {
                                        this.issue_details_scrollbar_drag_offset =
                                            Some(centered_anchor);
                                        cx.notify();
                                    });
                                }
                            }
                        });

                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |_: &MouseUpEvent, _, _, cx| {
                                entity.update(cx, |this, cx| {
                                    if this.issue_details_scrollbar_drag_offset.take().is_some() {
                                        cx.notify();
                                    }
                                });
                            }
                        });

                        window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
                            if !event.dragging() {
                                return;
                            }

                            let Some(anchor) = entity.read(cx).issue_details_scrollbar_drag_offset
                            else {
                                return;
                            };

                            let Some(metrics) =
                                issue_details_scrollbar_metrics(&scroll_handle_for_drag, bounds)
                            else {
                                return;
                            };

                            issue_details_set_scroll_offset(
                                &scroll_handle_for_drag,
                                metrics.track_bounds,
                                metrics.thumb_bounds.size.height,
                                event.position.y,
                                anchor,
                            );
                            cx.notify(entity.entity_id());
                        });
                    },
                )
                .size_full(),
            ),
    )
}

#[derive(Clone, Copy)]
pub(crate) struct IssueDetailsScrollbarMetrics {
    pub(crate) track_bounds: Bounds<Pixels>,
    pub(crate) thumb_bounds: Bounds<Pixels>,
}

pub(crate) fn issue_details_scrollbar_metrics(
    scroll_handle: &ScrollHandle,
    bounds: Bounds<Pixels>,
) -> Option<IssueDetailsScrollbarMetrics> {
    let viewport_height = scroll_handle.bounds().size.height;
    if viewport_height <= px(0.) {
        return None;
    }

    let max_offset = scroll_handle.max_offset();
    if max_offset.height <= px(0.) {
        return None;
    }

    let track_bounds = Bounds::new(
        point(bounds.origin.x, bounds.origin.y),
        size(bounds.size.width, bounds.size.height),
    );
    let track_height = track_bounds.size.height;
    let track_height_px = track_height.to_f64() as f32;
    let viewport_height_px = viewport_height.to_f64() as f32;
    let max_offset_px = max_offset.height.to_f64() as f32;
    let thumb_height = px(((track_height_px
        * (viewport_height_px / (viewport_height_px + max_offset_px)))
        .max(36.))
    .min(track_height_px));
    let thumb_travel = (track_height - thumb_height).max(px(0.));
    let current_offset_y = (-scroll_handle.offset().y).clamp(px(0.), max_offset.height);
    let thumb_top = if max_offset.height <= px(0.) || thumb_travel <= px(0.) {
        px(0.)
    } else {
        px((thumb_travel.to_f64() as f32)
            * ((current_offset_y.to_f64() as f32) / (max_offset.height.to_f64() as f32)))
    };
    let thumb_bounds = Bounds::new(
        point(
            track_bounds.origin.x + px(1.),
            track_bounds.origin.y + thumb_top,
        ),
        size((track_bounds.size.width - px(2.)).max(px(1.)), thumb_height),
    );

    Some(IssueDetailsScrollbarMetrics {
        track_bounds,
        thumb_bounds,
    })
}

pub(crate) fn issue_details_set_scroll_offset(
    scroll_handle: &ScrollHandle,
    track_bounds: Bounds<Pixels>,
    thumb_height: Pixels,
    pointer_y: Pixels,
    drag_anchor: Pixels,
) {
    let max_offset = scroll_handle.max_offset();
    if max_offset.height <= px(0.) {
        return;
    }

    let thumb_travel = (track_bounds.size.height - thumb_height).max(px(0.));
    if thumb_travel <= px(0.) {
        return;
    }

    let desired_thumb_top =
        (pointer_y - track_bounds.origin.y - drag_anchor).clamp(px(0.), thumb_travel);
    let ratio = (desired_thumb_top.to_f64() as f32) / (thumb_travel.to_f64() as f32);
    let target_offset_y = px((max_offset.height.to_f64() as f32) * ratio);
    let current_offset = scroll_handle.offset();
    scroll_handle.set_offset(point(current_offset.x, -target_offset_y));
}

pub(crate) const ISSUE_DESCRIPTION_FALLBACK: &str =
    "No issue description is available for this issue.";

pub(crate) fn issue_meta_chip(
    label: String,
    text_color: u32,
    background: u32,
    is_interactive: bool,
    url: Option<String>,
    cx: &mut Context<ArborWindow>,
) -> Div {
    div()
        .rounded_full()
        .border_1()
        .border_color(rgb(text_color))
        .bg(rgb(background))
        .px(px(8.))
        .py(px(3.))
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .font_family(FONT_MONO)
        .text_color(rgb(text_color))
        .when(is_interactive, |this| {
            this.cursor_pointer().hover(|this| this.opacity(0.9))
        })
        .when_some(url, |this, url| {
            this.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.open_external_url(&url, cx);
                    cx.stop_propagation();
                }),
            )
        })
        .child(label)
}

pub(crate) fn issue_body_text(body: Option<&str>) -> String {
    let Some(body) = body else {
        return ISSUE_DESCRIPTION_FALLBACK.to_owned();
    };

    let plain_text = issue_markdown_to_text(body);
    if plain_text.trim().is_empty() {
        ISSUE_DESCRIPTION_FALLBACK.to_owned()
    } else {
        plain_text
    }
}

pub(crate) fn issue_body_log_preview(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview: String = normalized.chars().take(160).collect();
    if normalized.chars().count() > 160 {
        preview.push_str("...");
    }
    preview
}

pub(crate) fn issue_markdown_to_text(markdown: &str) -> String {
    let mut plain_lines = Vec::new();
    let mut in_code_block = false;

    for raw_line in markdown.lines() {
        let trimmed = raw_line.trim_end();
        if trimmed.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        let plain_line = if in_code_block {
            trimmed.to_owned()
        } else {
            markdown_line_to_text(trimmed)
        };
        plain_lines.push(plain_line);
    }

    let mut normalized = Vec::new();
    let mut previous_blank = true;
    for line in plain_lines {
        let is_blank = line.trim().is_empty();
        if is_blank {
            if !previous_blank {
                normalized.push(String::new());
            }
        } else {
            normalized.push(line);
        }
        previous_blank = is_blank;
    }

    while normalized.last().is_some_and(|line| line.is_empty()) {
        normalized.pop();
    }

    normalized.join("\n")
}

pub(crate) fn markdown_line_to_text(line: &str) -> String {
    let mut text = line.trim_start();

    if let Some(stripped) = text.strip_prefix('>') {
        text = stripped.trim_start();
    }

    if let Some(stripped) = strip_markdown_heading(text) {
        text = stripped;
    }

    if let Some(stripped) = strip_markdown_list_marker(text) {
        text = stripped;
    }

    if let Some(stripped) = strip_markdown_task_marker(text) {
        text = stripped;
    }

    let mut plain = markdown_inline_to_text(text);
    plain = plain
        .replace("**", "")
        .replace("__", "")
        .replace("~~", "")
        .replace(['`', '*', '_'], "");
    plain.trim().to_owned()
}

pub(crate) fn strip_markdown_heading(line: &str) -> Option<&str> {
    let hashes = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if hashes == 0 {
        return None;
    }

    line[hashes..].strip_prefix(' ').or(Some(&line[hashes..]))
}

pub(crate) fn strip_markdown_list_marker(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(stripped) = line.strip_prefix(marker) {
            return Some(stripped);
        }
    }

    let digit_count = line
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if digit_count == 0 {
        return None;
    }

    let rest = &line[digit_count..];
    rest.strip_prefix(". ")
        .or_else(|| rest.strip_prefix(") "))
        .or(None)
}

pub(crate) fn strip_markdown_task_marker(line: &str) -> Option<&str> {
    ["[ ] ", "[x] ", "[X] "]
        .into_iter()
        .find_map(|marker| line.strip_prefix(marker))
}

pub(crate) fn markdown_inline_to_text(text: &str) -> String {
    let mut output = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        if character == '!'
            && chars.get(index + 1) == Some(&'[')
            && let Some((alt_text, next_index)) =
                markdown_bracket_and_url_to_text(&chars, index + 1, false)
        {
            output.push_str(&alt_text);
            index = next_index;
            continue;
        }

        if character == '['
            && let Some((link_text, next_index)) =
                markdown_bracket_and_url_to_text(&chars, index, true)
        {
            output.push_str(&link_text);
            index = next_index;
            continue;
        }

        if character == '<'
            && let Some(close_index) = chars[index + 1..]
                .iter()
                .position(|candidate| *candidate == '>')
        {
            let end = index + 1 + close_index;
            let inner: String = chars[index + 1..end].iter().collect();
            if inner.starts_with("http://") || inner.starts_with("https://") {
                output.push_str(&inner);
                index = end + 1;
                continue;
            }
        }

        output.push(character);
        index += 1;
    }

    output
}

pub(crate) fn markdown_bracket_and_url_to_text(
    chars: &[char],
    bracket_index: usize,
    include_url: bool,
) -> Option<(String, usize)> {
    let close_bracket_offset = chars[bracket_index + 1..]
        .iter()
        .position(|character| *character == ']')?;
    let close_bracket_index = bracket_index + 1 + close_bracket_offset;
    if chars.get(close_bracket_index + 1) != Some(&'(') {
        return None;
    }

    let close_paren_offset = chars[close_bracket_index + 2..]
        .iter()
        .position(|character| *character == ')')?;
    let close_paren_index = close_bracket_index + 2 + close_paren_offset;
    let label: String = chars[bracket_index + 1..close_bracket_index]
        .iter()
        .collect();
    let url: String = chars[close_bracket_index + 2..close_paren_index]
        .iter()
        .collect();

    let rendered = if include_url && !url.trim().is_empty() && label.trim() != url.trim() {
        format!("{label} ({url})")
    } else {
        label
    };
    Some((rendered, close_paren_index + 1))
}

pub(crate) fn issue_updated_label(updated_at: &str) -> String {
    format!("updated {updated_at}")
}

pub(crate) fn issue_modal_source_label(source: &terminal_daemon_http::IssueSourceDto) -> String {
    source.provider.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_markdown_to_text_produces_readable_plain_text() {
        let markdown = r#"# Summary

- [x] shipped **bold** change
- see [docs](https://example.com/docs)

> quoted _note_

```rs
let answer = 42;
```
"#;

        let plain_text = issue_markdown_to_text(markdown);
        assert_eq!(
            plain_text,
            "Summary\n\nshipped bold change\nsee docs (https://example.com/docs)\n\nquoted note\n\nlet answer = 42;"
        );
    }

    #[test]
    fn issue_body_text_falls_back_when_body_is_missing_or_empty() {
        assert_eq!(issue_body_text(None), ISSUE_DESCRIPTION_FALLBACK);
        assert_eq!(issue_body_text(Some("   \n\n")), ISSUE_DESCRIPTION_FALLBACK);
    }
}
