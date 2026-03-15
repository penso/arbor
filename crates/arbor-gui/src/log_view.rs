use super::*;

impl ArborWindow {
    pub(crate) fn render_logs_content(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let entry_count = self.log_entries.len();
        let auto_scroll = self.log_auto_scroll;

        div()
            .h_full()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.tab_bg))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.text_muted))
                            .child(format!("{entry_count} entries")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .id("log-copy-all")
                                    .cursor_pointer()
                                    .my(px(2.))
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .text_xs()
                                    .text_color(rgb(theme.text_muted))
                                    .hover(|this| {
                                        this.bg(rgb(theme.panel_active_bg))
                                            .text_color(rgb(theme.text_primary))
                                    })
                                    .child("Copy All")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                            let text = this
                                                .log_entries
                                                .iter()
                                                .map(format_log_entry)
                                                .collect::<Vec<_>>()
                                                .join("\n");
                                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("log-auto-scroll-toggle")
                                    .cursor_pointer()
                                    .text_xs()
                                    .text_color(rgb(if auto_scroll {
                                        theme.accent
                                    } else {
                                        theme.text_muted
                                    }))
                                    .hover(|this| this.text_color(rgb(theme.text_primary)))
                                    .child(if auto_scroll {
                                        "Auto-scroll: ON"
                                    } else {
                                        "Auto-scroll: OFF"
                                    })
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                            this.log_auto_scroll = !this.log_auto_scroll;
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    ),
            )
            .child(div().flex_1().min_h_0().child(if entry_count > 0 {
                let entries = self.log_entries.clone();
                div()
                    .id("log-entries")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.log_scroll_handle)
                    .children(
                        entries
                            .iter()
                            .enumerate()
                            .map(|(ix, entry)| render_log_row(entry, ix, theme)),
                    )
                    .into_any_element()
            } else {
                div()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(theme.text_muted))
                    .child("No log entries yet")
                    .into_any_element()
            }))
    }
}

pub(crate) fn render_log_row(
    entry: &log_layer::LogEntry,
    index: usize,
    theme: ThemePalette,
) -> Div {
    let timestamp = entry
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = timestamp.as_secs();
    let millis = timestamp.subsec_millis();
    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    let time_str = format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}");

    let (level_str, level_color) = match entry.level {
        tracing::Level::ERROR => ("ERROR", 0xf38ba8_u32),
        tracing::Level::WARN => ("WARN ", 0xf9e2af),
        tracing::Level::INFO => ("INFO ", 0xa6e3a1),
        tracing::Level::DEBUG => ("DEBUG", 0x89b4fa),
        tracing::Level::TRACE => ("TRACE", 0x9399b2),
    };

    let target = truncate_with_ellipsis(&entry.target, 30);
    let bg = if index.is_multiple_of(2) {
        theme.terminal_bg
    } else {
        theme.sidebar_bg
    };

    div()
        .py(px(2.))
        .w_full()
        .flex()
        .items_start()
        .gap_2()
        .px_2()
        .font_family(FONT_MONO)
        .text_size(px(DIFF_FONT_SIZE_PX))
        .bg(rgb(bg))
        .child(
            div()
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(time_str),
        )
        .child(
            div()
                .flex_none()
                .w(px(40.))
                .text_color(rgb(level_color))
                .child(level_str),
        )
        .child(
            div()
                .flex_none()
                .w(px(200.))
                .text_color(rgb(theme.text_muted))
                .overflow_hidden()
                .child(target),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(rgb(theme.text_primary))
                .child(if entry.fields.is_empty() {
                    entry.message.clone()
                } else {
                    let fields_str: Vec<String> = entry
                        .fields
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect();
                    format!("{} {}", entry.message, fields_str.join(" "))
                }),
        )
}
