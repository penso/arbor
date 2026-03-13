impl ArborWindow {
    fn handle_file_view_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        // Always handle Cmd+S for save, even when not in editing mode
        if event.keystroke.modifiers.platform && event.keystroke.key.as_str() == "s" {
            self.save_active_file_view(cx);
            return true;
        }
        if !self.file_view_editing {
            return false;
        }
        let Some(session_id) = self.active_file_view_session_id else {
            return false;
        };
        let Some(session) = self
            .file_view_sessions
            .iter_mut()
            .find(|s| s.id == session_id)
        else {
            return false;
        };
        let FileViewContent::Text {
            raw_lines, dirty, ..
        } = &mut session.content
        else {
            return false;
        };
        if raw_lines.is_empty() {
            return false;
        }

        let cursor = &mut session.cursor;

        // Skip platform combos (Cmd+S handled above)
        if event.keystroke.modifiers.platform {
            return false;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.file_view_editing = false;
                cx.notify();
                return true;
            },
            "backspace" => {
                if cursor.col > 0 {
                    let line = &mut raw_lines[cursor.line];
                    let byte_pos = char_to_byte_offset(line, cursor.col);
                    let prev_byte = char_to_byte_offset(line, cursor.col - 1);
                    line.replace_range(prev_byte..byte_pos, "");
                    cursor.col -= 1;
                } else if cursor.line > 0 {
                    let removed = raw_lines.remove(cursor.line);
                    cursor.line -= 1;
                    cursor.col = raw_lines[cursor.line].chars().count();
                    raw_lines[cursor.line].push_str(&removed);
                }
                *dirty = true;
                cx.notify();
                return true;
            },
            "delete" => {
                let line_char_count = raw_lines[cursor.line].chars().count();
                if cursor.col < line_char_count {
                    let line = &mut raw_lines[cursor.line];
                    let byte_pos = char_to_byte_offset(line, cursor.col);
                    let next_byte = char_to_byte_offset(line, cursor.col + 1);
                    line.replace_range(byte_pos..next_byte, "");
                } else if cursor.line + 1 < raw_lines.len() {
                    let next = raw_lines.remove(cursor.line + 1);
                    raw_lines[cursor.line].push_str(&next);
                }
                *dirty = true;
                cx.notify();
                return true;
            },
            "enter" | "return" => {
                let line = &raw_lines[cursor.line];
                let byte_pos = char_to_byte_offset(line, cursor.col);
                let rest = line[byte_pos..].to_owned();
                raw_lines[cursor.line].truncate(byte_pos);
                cursor.line += 1;
                cursor.col = 0;
                raw_lines.insert(cursor.line, rest);
                *dirty = true;
                cx.notify();
                return true;
            },
            "left" => {
                if cursor.col > 0 {
                    cursor.col -= 1;
                } else if cursor.line > 0 {
                    cursor.line -= 1;
                    cursor.col = raw_lines[cursor.line].chars().count();
                }
                cx.notify();
                return true;
            },
            "right" => {
                let line_len = raw_lines[cursor.line].chars().count();
                if cursor.col < line_len {
                    cursor.col += 1;
                } else if cursor.line + 1 < raw_lines.len() {
                    cursor.line += 1;
                    cursor.col = 0;
                }
                cx.notify();
                return true;
            },
            "up" => {
                if cursor.line > 0 {
                    cursor.line -= 1;
                    let line_len = raw_lines[cursor.line].chars().count();
                    cursor.col = cursor.col.min(line_len);
                }
                cx.notify();
                return true;
            },
            "down" => {
                if cursor.line + 1 < raw_lines.len() {
                    cursor.line += 1;
                    let line_len = raw_lines[cursor.line].chars().count();
                    cursor.col = cursor.col.min(line_len);
                }
                cx.notify();
                return true;
            },
            "tab" => {
                let line = &mut raw_lines[cursor.line];
                let byte_pos = char_to_byte_offset(line, cursor.col);
                line.insert_str(byte_pos, "    ");
                cursor.col += 4;
                *dirty = true;
                cx.notify();
                return true;
            },
            "home" => {
                cursor.col = 0;
                cx.notify();
                return true;
            },
            "end" => {
                cursor.col = raw_lines[cursor.line].chars().count();
                cx.notify();
                return true;
            },
            _ => {},
        }

        if event.keystroke.modifiers.control || event.keystroke.modifiers.alt {
            return false;
        }

        // Character input
        if let Some(key_char) = event.keystroke.key_char.as_ref() {
            let line = &mut raw_lines[cursor.line];
            let byte_pos = char_to_byte_offset(line, cursor.col);
            line.insert_str(byte_pos, key_char);
            cursor.col += key_char.chars().count();
            *dirty = true;
            cx.notify();
            return true;
        }

        false
    }

    fn save_active_file_view(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_file_view_session_id else {
            return;
        };
        let Some(session) = self.file_view_sessions.iter().find(|s| s.id == session_id) else {
            return;
        };
        let FileViewContent::Text {
            raw_lines, dirty, ..
        } = &session.content
        else {
            return;
        };
        if !dirty {
            return;
        }
        let content = raw_lines.join("\n");
        let full_path = session.worktree_path.join(&session.file_path);
        let ext = session
            .file_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let raw_clone = raw_lines.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    fs::write(&full_path, &content)
                        .map_err(|error| format!("Failed to save: {error}"))?;
                    let highlighted = highlight_lines_with_syntect(&raw_clone, &ext, 0xc8ccd4);
                    Ok::<Arc<[Vec<FileViewSpan>]>, String>(Arc::from(highlighted))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(highlighted) => {
                        if let Some(s) = this
                            .file_view_sessions
                            .iter_mut()
                            .find(|s| s.id == session_id)
                            && let FileViewContent::Text {
                                highlighted: current,
                                dirty,
                                ..
                            } = &mut s.content
                        {
                            *current = highlighted;
                            *dirty = false;
                        }
                    },
                    Err(error) => {
                        this.notice = Some(error);
                    },
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn render_file_view_session(
    session: FileViewSession,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
    mono_font: gpui::Font,
    editing: bool,
    cx: &mut Context<ArborWindow>,
) -> Div {
    let path_label = session.file_path.to_string_lossy().into_owned();
    let is_loading = session.is_loading;
    let session_id = session.id;
    let cursor = session.cursor;

    let (status_text, is_dirty, body) = match &session.content {
        FileViewContent::Image(image_path) => {
            let path = image_path.clone();
            (
                "image".to_owned(),
                false,
                div()
                    .id(("file-view-scroll", session_id))
                    .flex_1()
                    .min_h_0()
                    .bg(rgb(theme.terminal_bg))
                    .overflow_y_scroll()
                    .flex()
                    .justify_center()
                    .p_4()
                    .child(img(path).max_w_full().h_auto().with_fallback(move || {
                        div()
                            .text_sm()
                            .text_color(rgb(theme.text_muted))
                            .child("Failed to load image")
                            .into_any_element()
                    })),
            )
        },
        FileViewContent::Text {
            highlighted,
            raw_lines,
            dirty,
        } => {
            let line_count = raw_lines.len().max(highlighted.len());
            let status = if is_loading {
                "loading...".to_owned()
            } else {
                format!("{line_count} lines")
            };
            let highlighted = highlighted.clone();
            let raw_lines_clone = raw_lines.clone();
            let click_raw_lines = raw_lines.clone();
            let click_line_count = line_count;
            let click_scroll_handle = scroll_handle.clone();
            let line_number_width = line_count.to_string().len().max(3);
            let gutter_px = (line_number_width + 2) as f32 * DIFF_FONT_SIZE_PX * 0.6 + 8.0; // +8 for pl_2
            let body = div()
                .id(("file-view-scroll", session_id))
                .flex_1()
                .min_h_0()
                .bg(rgb(theme.terminal_bg))
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.file_view_editing = true;
                        this.right_pane_search_active = false;

                        // Compute clicked line and column from mouse position
                        let state = click_scroll_handle.0.borrow();
                        let bounds = state.base_handle.bounds();
                        let offset = state.base_handle.offset();
                        drop(state);

                        let local_y = f32::from(event.position.y - bounds.top()).max(0.);
                        let content_y = (local_y - f32::from(offset.y)).max(0.);
                        let clicked_line = ((content_y / DIFF_ROW_HEIGHT_PX).floor() as usize)
                            .min(click_line_count.saturating_sub(1));

                        let local_x =
                            (f32::from(event.position.x - bounds.left()) - gutter_px).max(0.);
                        let char_width = DIFF_FONT_SIZE_PX * 0.6;
                        let clicked_col = (local_x / char_width).floor() as usize;

                        let max_col = click_raw_lines
                            .get(clicked_line)
                            .map(|l| l.chars().count())
                            .unwrap_or(0);

                        if let Some(session) = this
                            .file_view_sessions
                            .iter_mut()
                            .find(|s| s.id == session_id)
                        {
                            session.cursor.line = clicked_line;
                            session.cursor.col = clicked_col.min(max_col);
                        }
                        cx.notify();
                    }),
                )
                .when(is_loading, |this| {
                    this.child(
                        div()
                            .h_full()
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .text_sm()
                            .text_color(rgb(theme.text_muted))
                            .child("Loading file..."),
                    )
                })
                .when(!is_loading, |this| {
                    let scroll_handle = scroll_handle.clone();
                    let mono_font = mono_font.clone();
                    let line_number_width = line_count.to_string().len().max(3);
                    let show_cursor = editing;
                    this.child(
                        div().size_full().min_w_0().flex().child(
                            uniform_list(
                                ("file-view-list", session_id),
                                line_count,
                                move |range, _, _| {
                                    range
                                        .map(|index| {
                                            let line_num = index + 1;
                                            let is_cursor_line =
                                                show_cursor && cursor.line == index;

                                            let mut content_div = div()
                                                .pl_2()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .flex();

                                            if show_cursor {
                                                // When editing, show raw text with cursor
                                                let raw = raw_lines_clone
                                                    .get(index)
                                                    .cloned()
                                                    .unwrap_or_default();
                                                if is_cursor_line {
                                                    let byte_pos =
                                                        char_to_byte_offset(&raw, cursor.col);
                                                    let before = &raw[..byte_pos];
                                                    let after = &raw[byte_pos..];
                                                    let cursor_char =
                                                        after.chars().next().unwrap_or(' ');
                                                    let after_cursor = if after.is_empty() {
                                                        String::new()
                                                    } else {
                                                        after.chars().skip(1).collect()
                                                    };
                                                    content_div = content_div
                                                        .child(
                                                            div()
                                                                .text_color(rgb(theme.text_primary))
                                                                .child(before.to_owned()),
                                                        )
                                                        .child(
                                                            div()
                                                                .bg(rgb(theme.accent))
                                                                .text_color(rgb(theme.terminal_bg))
                                                                .child(cursor_char.to_string()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_color(rgb(theme.text_primary))
                                                                .child(after_cursor),
                                                        );
                                                } else {
                                                    content_div = content_div.child(
                                                        div()
                                                            .text_color(rgb(theme.text_primary))
                                                            .child(if raw.is_empty() {
                                                                " ".to_owned()
                                                            } else {
                                                                raw
                                                            }),
                                                    );
                                                }
                                            } else {
                                                // Not editing: show highlighted spans
                                                if let Some(spans) = highlighted.get(index) {
                                                    for span in spans {
                                                        content_div = content_div.child(
                                                            div()
                                                                .text_color(rgb(span.color))
                                                                .child(span.text.clone()),
                                                        );
                                                    }
                                                }
                                            }

                                            div()
                                                .id(("fv-row", index))
                                                .h(px(DIFF_ROW_HEIGHT_PX))
                                                .w_full()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .font(mono_font.clone())
                                                .text_size(px(DIFF_FONT_SIZE_PX))
                                                .child(
                                                    div()
                                                        .w(px((line_number_width + 2) as f32
                                                            * DIFF_FONT_SIZE_PX
                                                            * 0.6))
                                                        .flex_none()
                                                        .text_color(rgb(theme.text_disabled))
                                                        .text_size(px(DIFF_FONT_SIZE_PX))
                                                        .px_1()
                                                        .flex()
                                                        .justify_end()
                                                        .child(format!("{line_num}")),
                                                )
                                                .child(content_div)
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .track_scroll(scroll_handle.clone()),
                        ),
                    )
                });
            (status, *dirty, body)
        },
    };

    div()
        .h_full()
        .w_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(28.))
                .px_3()
                .bg(rgb(theme.tab_active_bg))
                .border_b_1()
                .border_color(rgb(theme.border))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .font(mono_font.clone())
                                .text_size(px(DIFF_FONT_SIZE_PX))
                                .text_color(rgb(theme.text_muted))
                                .child(path_label),
                        )
                        .when(is_dirty, |this| {
                            this.child(
                                div()
                                    .text_size(px(DIFF_FONT_SIZE_PX))
                                    .text_color(rgb(theme.accent))
                                    .child("\u{2022}"),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(is_dirty, |this| {
                            this.child(
                                div()
                                    .id(("fv-save", session_id))
                                    .cursor_pointer()
                                    .px_2()
                                    .rounded_sm()
                                    .bg(rgb(theme.accent))
                                    .hover(|this| this.opacity(0.85))
                                    .text_xs()
                                    .text_color(rgb(theme.terminal_bg))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Save")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                            this.save_active_file_view(cx);
                                        }),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_disabled))
                                .child(status_text),
                        ),
                ),
        )
        .child(body)
}
