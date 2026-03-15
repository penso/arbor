use super::*;

pub(crate) fn render_diff_session(
    session: DiffSession,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> Div {
    let path_label = truncate_middle_text(&session.title, 84);
    let line_count = session.lines.len();
    let is_loading = session.is_loading;
    let session_id = session.id;

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
                        .font(mono_font.clone())
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(theme.text_muted))
                        .child(path_label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_disabled))
                        .child(if is_loading {
                            "loading...".to_owned()
                        } else {
                            format!("{line_count} rows")
                        }),
                ),
        )
        .child(
            div()
                .id(("diff-scroll", session_id))
                .flex_1()
                .min_h_0()
                .bg(rgb(theme.terminal_bg))
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
                            .child("Computing diff..."),
                    )
                })
                .when(!is_loading, |this| {
                    let lines = session.lines.clone();
                    let zonemap_lines = lines.clone();
                    let scroll_handle = scroll_handle.clone();
                    let mono_font = mono_font.clone();
                    this.child(
                        div()
                            .size_full()
                            .min_w_0()
                            .flex()
                            .child(
                                uniform_list(
                                    ("diff-list", session_id),
                                    lines.len(),
                                    move |range, _, _| {
                                        range
                                            .map(|index| {
                                                render_diff_row(
                                                    session_id,
                                                    index,
                                                    lines[index].clone(),
                                                    theme,
                                                    mono_font.clone(),
                                                    diff_cell_width,
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                )
                                .h_full()
                                .flex_1()
                                .min_w_0()
                                .track_scroll(scroll_handle.clone()),
                            )
                            .child(render_diff_zonemap(zonemap_lines, theme, &scroll_handle)),
                    )
                }),
        )
}

pub(crate) fn render_diff_row(
    session_id: u64,
    row_index: usize,
    line: DiffLine,
    theme: ThemePalette,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> impl IntoElement {
    if line.kind == DiffLineKind::FileHeader {
        return div()
            .id(diff_row_element_id(
                "diff-row-header",
                session_id,
                row_index,
            ))
            .w_full()
            .h(px(DIFF_ROW_HEIGHT_PX))
            .min_h(px(DIFF_ROW_HEIGHT_PX))
            .bg(rgb(theme.tab_active_bg))
            .px_2()
            .flex()
            .items_center()
            .child(
                div()
                    .min_w_0()
                    .font(mono_font)
                    .text_size(px(DIFF_FONT_SIZE_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .text_color(rgb(theme.text_primary))
                    .child(line.left_text),
            );
    }

    let (left_bg, right_bg) = diff_line_backgrounds(line.kind, theme);
    let (left_marker, right_marker) = diff_line_markers(line.kind);
    let (left_text_color, right_text_color) = diff_line_text_colors(line.kind, theme);
    div()
        .id(diff_row_element_id("diff-row", session_id, row_index))
        .w_full()
        .min_w_0()
        .h(px(DIFF_ROW_HEIGHT_PX))
        .min_h(px(DIFF_ROW_HEIGHT_PX))
        .flex()
        .child(render_diff_column(
            session_id,
            row_index,
            0,
            line.left_line_number,
            line.left_text,
            left_marker,
            left_bg,
            left_text_color,
            theme,
            mono_font.clone(),
            diff_cell_width,
        ))
        .child(render_diff_column(
            session_id,
            row_index,
            1,
            line.right_line_number,
            line.right_text,
            right_marker,
            right_bg,
            right_text_color,
            theme,
            mono_font,
            diff_cell_width,
        ))
}

pub(crate) fn render_diff_column(
    session_id: u64,
    row_index: usize,
    side: usize,
    line_number: Option<usize>,
    text: String,
    marker: char,
    background: u32,
    text_color: u32,
    theme: ThemePalette,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> impl IntoElement {
    let number_width = px((DIFF_LINE_NUMBER_WIDTH_CHARS as f32 * diff_cell_width) + 12.);

    let column_id = diff_row_side_element_id("diff-column", session_id, row_index, side);
    let marker_id = diff_row_side_element_id("diff-marker", session_id, row_index, side);
    let text_id = diff_row_side_element_id("diff-text", session_id, row_index, side);

    div()
        .id(column_id)
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(rgb(background))
        .child(
            div()
                .h_full()
                .min_w_0()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(number_width)
                        .flex_none()
                        .text_right()
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(theme.text_disabled))
                        .child(line_number.map_or(String::new(), |line| line.to_string())),
                )
                .child(
                    div()
                        .id(marker_id)
                        .w(px(10.))
                        .flex_none()
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(diff_marker_color(marker)))
                        .child(marker.to_string()),
                )
                .child(
                    div()
                        .id(text_id)
                        .min_w_0()
                        .flex_1()
                        .font(mono_font)
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .whitespace_nowrap()
                        .text_color(rgb(text_color))
                        .child(if text.is_empty() {
                            " ".to_owned()
                        } else {
                            text
                        }),
                ),
        )
}

pub(crate) fn render_diff_zonemap(
    lines: Arc<[DiffLine]>,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
) -> Div {
    let scroll_handle_for_draw = scroll_handle.clone();
    let scroll_handle_for_click = scroll_handle.clone();
    let scroll_handle_for_drag = scroll_handle.clone();
    let total_rows = lines.len();
    let marker_spans = build_zonemap_marker_spans(lines.as_ref());

    div()
        .h_full()
        .w(px(DIFF_ZONEMAP_WIDTH_PX + (DIFF_ZONEMAP_MARGIN_PX * 2.)))
        .pt(px(DIFF_ZONEMAP_MARGIN_PX))
        .pb(px(DIFF_ZONEMAP_MARGIN_PX))
        .pl(px(DIFF_ZONEMAP_MARGIN_PX))
        .pr(px(DIFF_ZONEMAP_MARGIN_PX))
        .flex_none()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, _| {
            if total_rows == 0 {
                return;
            }

            let bounds = scroll_handle_for_click.0.borrow().base_handle.bounds();
            let height = bounds.size.height.to_f64() as f32;
            if !height.is_finite() || height <= 0. {
                return;
            }

            let relative_y = (f32::from(event.position.y - bounds.top()) / height).clamp(0., 1.);
            let mut target_row = (relative_y * total_rows as f32).floor() as usize;
            if target_row >= total_rows {
                target_row = total_rows.saturating_sub(1);
            }
            scroll_handle_for_click.scroll_to_item(target_row, ScrollStrategy::Center);
        })
        .on_mouse_move(move |event: &MouseMoveEvent, _, _| {
            if event.pressed_button != Some(MouseButton::Left) || total_rows == 0 {
                return;
            }

            let bounds = scroll_handle_for_drag.0.borrow().base_handle.bounds();
            let height = bounds.size.height.to_f64() as f32;
            if !height.is_finite() || height <= 0. {
                return;
            }

            let relative_y = (f32::from(event.position.y - bounds.top()) / height).clamp(0., 1.);
            let mut target_row = (relative_y * total_rows as f32).floor() as usize;
            if target_row >= total_rows {
                target_row = total_rows.saturating_sub(1);
            }
            scroll_handle_for_drag.scroll_to_item(target_row, ScrollStrategy::Center);
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    window.paint_quad(fill(bounds, rgb(theme.app_bg)));

                    let track_origin = point(bounds.origin.x + px(1.), bounds.origin.y + px(1.));
                    let track_size = size(
                        (bounds.size.width - px(2.)).max(px(1.)),
                        (bounds.size.height - px(2.)).max(px(1.)),
                    );
                    let track_bounds = Bounds::new(track_origin, track_size);
                    window.paint_quad(fill(track_bounds, rgb(theme.panel_bg)));

                    if total_rows == 0 {
                        return;
                    }

                    let height = track_bounds.size.height.to_f64() as f32;
                    if !height.is_finite() || height <= 0. {
                        return;
                    }

                    let marker_origin_x = track_bounds.origin.x + px(1.);
                    let marker_width = (track_bounds.size.width - px(2.)).max(px(1.));

                    for span in &marker_spans {
                        let start_ratio = span.start_row as f32 / total_rows as f32;
                        let end_ratio = span.end_row.saturating_add(1) as f32 / total_rows as f32;
                        let y = track_bounds.origin.y + px(start_ratio * height);
                        let marker_height =
                            px(((end_ratio - start_ratio) * height)
                                .max(DIFF_ZONEMAP_MARKER_HEIGHT_PX));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(marker_origin_x, y),
                                size(marker_width, marker_height),
                            ),
                            rgb(span.color),
                        ));
                    }

                    let (visible_top, visible_bottom) =
                        diff_visible_row_range(&scroll_handle_for_draw, total_rows);
                    let visible_count =
                        visible_bottom.saturating_sub(visible_top).saturating_add(1);
                    let thumb_top_ratio = visible_top as f32 / total_rows as f32;
                    let thumb_height_ratio = visible_count as f32 / total_rows as f32;
                    let thumb_height = px((thumb_height_ratio * height)
                        .max(DIFF_ZONEMAP_MIN_THUMB_HEIGHT_PX)
                        .min(height));
                    let max_thumb_top =
                        track_bounds.origin.y + track_bounds.size.height - thumb_height;
                    let thumb_top = (track_bounds.origin.y + px(thumb_top_ratio * height))
                        .min(max_thumb_top)
                        .max(track_bounds.origin.y);

                    window.paint_quad(fill(
                        Bounds::new(
                            point(track_bounds.origin.x, thumb_top),
                            size(track_bounds.size.width, thumb_height),
                        ),
                        rgb(theme.accent),
                    ));
                },
            )
            .size_full(),
        )
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ZonemapMarkerSpan {
    pub(crate) start_row: usize,
    pub(crate) end_row: usize,
    pub(crate) color: u32,
}

pub(crate) fn build_zonemap_marker_spans(lines: &[DiffLine]) -> Vec<ZonemapMarkerSpan> {
    let mut spans: Vec<ZonemapMarkerSpan> = Vec::new();

    for (row, line) in lines.iter().enumerate() {
        let Some(color) = zonemap_marker_color(line.kind) else {
            continue;
        };

        if let Some(last) = spans.last_mut()
            && last.color == color
            && row == last.end_row.saturating_add(1)
        {
            last.end_row = row;
            continue;
        }

        spans.push(ZonemapMarkerSpan {
            start_row: row,
            end_row: row,
            color,
        });
    }

    spans
}

pub(crate) fn diff_visible_row_range(
    scroll_handle: &UniformListScrollHandle,
    total_rows: usize,
) -> (usize, usize) {
    if total_rows == 0 {
        return (0, 0);
    }

    let state = scroll_handle.0.borrow();
    let max_row = total_rows.saturating_sub(1);
    let viewport_height = f32::from(state.base_handle.bounds().size.height).max(0.);
    let scroll_offset_y = (-f32::from(state.base_handle.offset().y)).max(0.);

    let top = (scroll_offset_y / DIFF_ROW_HEIGHT_PX).floor() as usize;
    let visible_rows = ((viewport_height / DIFF_ROW_HEIGHT_PX).ceil() as usize).max(1);
    let bottom = top.saturating_add(visible_rows.saturating_sub(1));

    let clamped_top = top.min(max_row);
    let clamped_bottom = bottom.min(max_row);
    (clamped_top, clamped_bottom.max(clamped_top))
}

pub(crate) fn zonemap_marker_color(kind: DiffLineKind) -> Option<u32> {
    match kind {
        DiffLineKind::FileHeader => Some(0x6d88a6),
        DiffLineKind::Added => Some(0x72d69c),
        DiffLineKind::Removed => Some(0xeb6f92),
        DiffLineKind::Modified => Some(0xf9e2af),
        DiffLineKind::Context => None,
    }
}

pub(crate) fn diff_line_backgrounds(kind: DiffLineKind, theme: ThemePalette) -> (u32, u32) {
    match kind {
        DiffLineKind::FileHeader => (theme.tab_active_bg, theme.tab_active_bg),
        DiffLineKind::Context
        | DiffLineKind::Added
        | DiffLineKind::Removed
        | DiffLineKind::Modified => (theme.terminal_bg, theme.terminal_bg),
    }
}

pub(crate) fn diff_line_text_colors(kind: DiffLineKind, theme: ThemePalette) -> (u32, u32) {
    match kind {
        DiffLineKind::FileHeader => (theme.text_primary, theme.text_primary),
        DiffLineKind::Context => (theme.text_primary, theme.text_primary),
        DiffLineKind::Added => (theme.text_disabled, 0x8fd7ad),
        DiffLineKind::Removed => (0xf2a4b7, theme.text_disabled),
        DiffLineKind::Modified => (0xf2a4b7, 0x8fd7ad),
    }
}

pub(crate) fn diff_line_markers(kind: DiffLineKind) -> (char, char) {
    match kind {
        DiffLineKind::FileHeader => (' ', ' '),
        DiffLineKind::Context => (' ', ' '),
        DiffLineKind::Added => (' ', '+'),
        DiffLineKind::Removed => ('-', ' '),
        DiffLineKind::Modified => ('-', '+'),
    }
}

pub(crate) fn diff_marker_color(marker: char) -> u32 {
    match marker {
        '+' => 0x72d69c,
        '-' => 0xeb6f92,
        '~' => 0xf9e2af,
        _ => 0x7c8599,
    }
}

pub(crate) fn wrap_diff_document_lines(
    raw_lines: &[DiffLine],
    raw_file_row_indices: &HashMap<PathBuf, usize>,
    wrap_columns: usize,
) -> (Vec<DiffLine>, HashMap<PathBuf, usize>) {
    let mut wrapped_lines = Vec::new();
    let mut raw_to_wrapped_index = Vec::with_capacity(raw_lines.len());

    for raw_line in raw_lines {
        raw_to_wrapped_index.push(wrapped_lines.len());
        wrapped_lines.extend(wrap_diff_line(raw_line.clone(), wrap_columns));
    }

    let wrapped_file_row_indices = raw_file_row_indices
        .iter()
        .map(|(path, raw_index)| {
            let wrapped_index = raw_to_wrapped_index.get(*raw_index).copied().unwrap_or(0);
            (path.clone(), wrapped_index)
        })
        .collect::<HashMap<_, _>>();

    (wrapped_lines, wrapped_file_row_indices)
}

pub(crate) fn wrap_diff_line(line: DiffLine, wrap_columns: usize) -> Vec<DiffLine> {
    let wrap_columns = wrap_columns.max(1);
    if line.kind == DiffLineKind::FileHeader {
        return split_diff_text_chunks(line.left_text, wrap_columns.saturating_mul(2))
            .into_iter()
            .map(|chunk| DiffLine {
                left_line_number: None,
                right_line_number: None,
                left_text: chunk,
                right_text: String::new(),
                kind: DiffLineKind::FileHeader,
            })
            .collect();
    }

    let left_chunks = split_diff_text_chunks(line.left_text, wrap_columns);
    let right_chunks = split_diff_text_chunks(line.right_text, wrap_columns);
    let chunk_count = left_chunks.len().max(right_chunks.len()).max(1);
    let mut wrapped = Vec::with_capacity(chunk_count);

    for index in 0..chunk_count {
        wrapped.push(DiffLine {
            left_line_number: (index == 0).then_some(line.left_line_number).flatten(),
            right_line_number: (index == 0).then_some(line.right_line_number).flatten(),
            left_text: left_chunks.get(index).cloned().unwrap_or_default(),
            right_text: right_chunks.get(index).cloned().unwrap_or_default(),
            kind: line.kind,
        });
    }

    wrapped
}

pub(crate) fn split_diff_text_chunks(text: String, wrap_columns: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let wrap_columns = wrap_columns.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0_usize;

    for ch in text.chars() {
        current.push(ch);
        current_len += 1;

        if current_len >= wrap_columns {
            chunks.push(current);
            current = String::new();
            current_len = 0;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    if chunks.is_empty() {
        chunks.push(String::new());
    }

    chunks
}

pub(crate) fn diff_row_element_id(
    prefix: &'static str,
    session_id: u64,
    row_index: usize,
) -> ElementId {
    let session_scope = ElementId::from((prefix, session_id));
    ElementId::from((session_scope, row_index.to_string()))
}

pub(crate) fn diff_row_side_element_id(
    prefix: &'static str,
    session_id: u64,
    row_index: usize,
    side: usize,
) -> ElementId {
    let row_scope = diff_row_element_id(prefix, session_id, row_index);
    ElementId::from((row_scope, side.to_string()))
}
