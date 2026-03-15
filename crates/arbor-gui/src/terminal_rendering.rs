use super::*;

pub(crate) fn styled_lines_for_session(
    session: &TerminalSession,
    theme: ThemePalette,
    show_cursor: bool,
    selection: Option<&TerminalSelection>,
    ime_marked_text: Option<&str>,
) -> Vec<TerminalStyledLine> {
    let mut lines = if !session.styled_output.is_empty() {
        session.styled_output.clone()
    } else {
        plain_lines_to_styled(lines_for_display(&session.output), theme)
    };

    for line in &mut lines {
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        } else if line.runs.is_empty() && !line.cells.is_empty() {
            line.runs = runs_from_cells(&line.cells);
        }

        let mut changed = false;
        for cell in &mut line.cells {
            if cell.bg == EMBEDDED_TERMINAL_DEFAULT_BG {
                cell.bg = theme.terminal_bg;
                changed = true;
            }
            if cell.fg == EMBEDDED_TERMINAL_DEFAULT_FG {
                cell.fg = theme.text_primary;
                changed = true;
            }
        }

        if changed {
            line.runs = runs_from_cells(&line.cells);
        }
    }

    if show_cursor
        && session.state == TerminalState::Running
        && let Some(cursor) = session.cursor
    {
        if let Some(marked) = ime_marked_text {
            apply_ime_marked_text_to_lines(&mut lines, cursor, marked, theme);
        } else {
            apply_cursor_to_lines(&mut lines, cursor, theme);
        }
    }

    if let Some(selection) = selection.filter(|selection| selection.session_id == session.id) {
        apply_selection_to_lines(&mut lines, selection, theme);
    }

    lines
}

pub(crate) fn apply_cursor_to_lines(
    lines: &mut Vec<TerminalStyledLine>,
    cursor: TerminalCursor,
    theme: ThemePalette,
) {
    while lines.len() <= cursor.line {
        lines.push(TerminalStyledLine {
            cells: Vec::new(),
            runs: Vec::new(),
        });
    }

    if let Some(line) = lines.get_mut(cursor.line) {
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        }

        let insert_index = line
            .cells
            .iter()
            .position(|cell| cell.column >= cursor.column)
            .unwrap_or(line.cells.len());

        if line
            .cells
            .get(insert_index)
            .is_none_or(|cell| cell.column != cursor.column)
        {
            line.cells.insert(insert_index, TerminalStyledCell {
                column: cursor.column,
                text: " ".to_owned(),
                fg: theme.text_primary,
                bg: theme.terminal_bg,
            });
        }

        if let Some(cell) = line.cells.get_mut(insert_index) {
            if cell.text.is_empty() {
                cell.text = " ".to_owned();
            }

            if cell.text.chars().all(|character| character == ' ') {
                cell.fg = theme.text_primary;
            }
            cell.bg = theme.terminal_cursor;
        }

        line.runs = runs_from_cells(&line.cells);
    }
}

pub(crate) fn apply_ime_marked_text_to_lines(
    lines: &mut [TerminalStyledLine],
    cursor: TerminalCursor,
    marked_text: &str,
    theme: ThemePalette,
) {
    if lines.len() <= cursor.line {
        return;
    }

    let Some(line) = lines.get_mut(cursor.line) else {
        return;
    };

    if line.cells.is_empty() && !line.runs.is_empty() {
        line.cells = cells_from_runs(&line.runs);
    }

    let insert_index = line
        .cells
        .iter()
        .position(|cell| cell.column >= cursor.column)
        .unwrap_or(line.cells.len());

    // Insert marked text cell at cursor position with cursor highlight
    if line
        .cells
        .get(insert_index)
        .is_some_and(|cell| cell.column == cursor.column)
    {
        line.cells[insert_index] = TerminalStyledCell {
            column: cursor.column,
            text: marked_text.to_owned(),
            fg: theme.text_primary,
            bg: theme.terminal_cursor,
        };
    } else {
        line.cells.insert(insert_index, TerminalStyledCell {
            column: cursor.column,
            text: marked_text.to_owned(),
            fg: theme.text_primary,
            bg: theme.terminal_cursor,
        });
    }

    line.runs = runs_from_cells(&line.cells);
}

pub(crate) fn apply_selection_to_lines(
    lines: &mut Vec<TerminalStyledLine>,
    selection: &TerminalSelection,
    theme: ThemePalette,
) {
    let Some((start, end)) = normalized_terminal_selection(selection) else {
        return;
    };

    while lines.len() <= end.line {
        lines.push(TerminalStyledLine {
            cells: Vec::new(),
            runs: Vec::new(),
        });
    }

    for line_index in start.line..=end.line {
        let Some(line) = lines.get_mut(line_index) else {
            continue;
        };
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        }

        let line_start = if line_index == start.line {
            start.column
        } else {
            0
        };
        let line_end_exclusive = if line_index == end.line {
            end.column
        } else {
            usize::MAX
        };
        if line_end_exclusive <= line_start {
            continue;
        }

        let mut changed = false;
        for cell in &mut line.cells {
            if cell.column >= line_start && cell.column < line_end_exclusive {
                cell.fg = theme.terminal_selection_fg;
                cell.bg = theme.terminal_selection_bg;
                changed = true;
            }
        }

        if changed {
            line.runs = runs_from_cells(&line.cells);
        }
    }
}

pub(crate) fn normalized_terminal_selection(
    selection: &TerminalSelection,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let (start, end) = if selection.anchor.line < selection.head.line
        || (selection.anchor.line == selection.head.line
            && selection.anchor.column <= selection.head.column)
    {
        (selection.anchor, selection.head)
    } else {
        (selection.head, selection.anchor)
    };

    if start == end {
        return None;
    }

    Some((start, end))
}

pub(crate) fn cells_from_runs(runs: &[TerminalStyledRun]) -> Vec<TerminalStyledCell> {
    let mut cells = Vec::new();
    let mut column = 0_usize;
    for run in runs {
        for character in run.text.chars() {
            cells.push(TerminalStyledCell {
                column,
                text: character.to_string(),
                fg: run.fg,
                bg: run.bg,
            });
            column = column.saturating_add(1);
        }
    }
    cells
}

pub(crate) fn runs_from_cells(cells: &[TerminalStyledCell]) -> Vec<TerminalStyledRun> {
    let mut runs = Vec::new();
    let mut current_fg = None;
    let mut current_bg = None;
    let mut current_text = String::new();
    let mut next_expected_column: Option<usize> = None;
    let mut current_contains_complex_cell = false;
    let mut current_contains_decorative_cell = false;

    for cell in cells {
        let cell_is_complex = cell.text.chars().count() != 1;
        let cell_is_powerline = cell
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
            && cell.text.chars().count() == 1;
        let style_changed = current_fg != Some(cell.fg) || current_bg != Some(cell.bg);
        let gap_breaks_run = next_expected_column != Some(cell.column);
        let complex_breaks_run = current_contains_complex_cell || cell_is_complex;
        let decorative_breaks_run = current_contains_decorative_cell || cell_is_powerline;
        if style_changed || gap_breaks_run || complex_breaks_run || decorative_breaks_run {
            if let (Some(fg), Some(bg)) = (current_fg.take(), current_bg.take())
                && !current_text.is_empty()
            {
                runs.push(TerminalStyledRun {
                    text: std::mem::take(&mut current_text),
                    fg,
                    bg,
                });
            }

            current_fg = Some(cell.fg);
            current_bg = Some(cell.bg);
            current_contains_complex_cell = cell_is_complex;
            current_contains_decorative_cell = cell_is_powerline;
        }

        current_text.push_str(&cell.text);
        next_expected_column = Some(cell.column.saturating_add(1));
        current_contains_decorative_cell |= cell_is_powerline;
    }

    if let (Some(fg), Some(bg)) = (current_fg, current_bg)
        && !current_text.is_empty()
    {
        runs.push(TerminalStyledRun {
            text: current_text,
            fg,
            bg,
        });
    }

    runs
}

#[derive(Clone)]
pub(crate) struct PositionedTerminalRun {
    pub(crate) text: String,
    pub(crate) fg: u32,
    pub(crate) bg: u32,
    pub(crate) start_column: usize,
    pub(crate) cell_count: usize,
    pub(crate) force_cell_width: bool,
}

pub(crate) fn positioned_runs_from_cells(
    cells: &[TerminalStyledCell],
) -> Vec<PositionedTerminalRun> {
    let mut runs = Vec::new();
    let mut current_fg: Option<u32> = None;
    let mut current_bg: Option<u32> = None;
    let mut current_start_column = 0_usize;
    let mut current_text = String::new();
    let mut next_expected_column: Option<usize> = None;
    let mut current_contains_complex_cell = false;
    let mut current_contains_decorative_cell = false;
    let mut current_cell_count = 0_usize;

    for cell in cells {
        let cell_is_complex = cell.text.chars().count() != 1;
        let cell_is_powerline = cell
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
            && cell.text.chars().count() == 1;
        let style_changed = current_fg != Some(cell.fg) || current_bg != Some(cell.bg);
        let gap_breaks_run = next_expected_column != Some(cell.column);
        let complex_breaks_run = current_contains_complex_cell || cell_is_complex;
        let decorative_breaks_run = current_contains_decorative_cell || cell_is_powerline;
        if style_changed || gap_breaks_run || complex_breaks_run || decorative_breaks_run {
            if let (Some(fg), Some(bg)) = (current_fg.take(), current_bg.take())
                && !current_text.is_empty()
            {
                runs.push(PositionedTerminalRun {
                    text: std::mem::take(&mut current_text),
                    fg,
                    bg,
                    start_column: current_start_column,
                    cell_count: current_cell_count,
                    force_cell_width: !current_contains_complex_cell
                        && !current_contains_decorative_cell,
                });
            }

            current_fg = Some(cell.fg);
            current_bg = Some(cell.bg);
            current_start_column = cell.column;
            current_contains_complex_cell = cell_is_complex;
            current_contains_decorative_cell = cell_is_powerline;
            current_cell_count = 0;
        }

        current_text.push_str(&cell.text);
        current_cell_count = current_cell_count.saturating_add(1);
        current_contains_complex_cell |= cell_is_complex;
        current_contains_decorative_cell |= cell_is_powerline;
        next_expected_column = Some(cell.column.saturating_add(1));
    }

    if let (Some(fg), Some(bg)) = (current_fg, current_bg)
        && !current_text.is_empty()
    {
        runs.push(PositionedTerminalRun {
            text: current_text,
            fg,
            bg,
            start_column: current_start_column,
            cell_count: current_cell_count,
            force_cell_width: !current_contains_complex_cell && !current_contains_decorative_cell,
        });
    }

    runs
}

pub(crate) fn is_terminal_powerline_character(ch: char) -> bool {
    matches!(ch as u32, 0xE0B0..=0xE0D7)
}

pub(crate) fn plain_lines_to_styled(
    lines: Vec<String>,
    theme: ThemePalette,
) -> Vec<TerminalStyledLine> {
    lines
        .into_iter()
        .map(|line| {
            let cells: Vec<TerminalStyledCell> = line
                .chars()
                .enumerate()
                .map(|(column, character)| TerminalStyledCell {
                    column,
                    text: character.to_string(),
                    fg: theme.text_primary,
                    bg: theme.terminal_bg,
                })
                .collect();

            let runs = if line.is_empty() {
                Vec::new()
            } else {
                vec![TerminalStyledRun {
                    text: line,
                    fg: theme.text_primary,
                    bg: theme.terminal_bg,
                }]
            };

            TerminalStyledLine { cells, runs }
        })
        .collect()
}

pub(crate) fn render_terminal_line(
    line: TerminalStyledLine,
    theme: ThemePalette,
    cell_width: f32,
    line_height: f32,
    mono_font: gpui::Font,
) -> Div {
    let cells = if line.cells.is_empty() {
        cells_from_runs(&line.runs)
    } else {
        line.cells
    };

    if cells.is_empty() {
        return div()
            .flex_none()
            .w_full()
            .min_w_0()
            .h(px(line_height))
            .overflow_x_hidden()
            .whitespace_nowrap()
            .font(mono_font)
            .text_size(px(TERMINAL_FONT_SIZE_PX))
            .line_height(px(line_height))
            .bg(rgb(theme.terminal_bg))
            .text_color(rgb(theme.text_primary))
            .child(" ");
    }

    let line_height = px(line_height);
    let font_size = px(TERMINAL_FONT_SIZE_PX);
    let positioned_runs = positioned_runs_from_cells(&cells);

    div()
        .flex_none()
        .w_full()
        .min_w_0()
        .h(line_height)
        .overflow_hidden()
        .bg(rgb(theme.terminal_bg))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let scale_factor = window.scale_factor();
                    for run in &positioned_runs {
                        if run.text.is_empty() {
                            continue;
                        }

                        if run.cell_count > 0 {
                            let start_x = snap_pixels_floor(
                                bounds.origin.x + px(run.start_column as f32 * cell_width),
                                scale_factor,
                            );
                            let end_x = snap_pixels_ceil(
                                bounds.origin.x
                                    + px((run.start_column + run.cell_count) as f32 * cell_width),
                                scale_factor,
                            );
                            let background_origin = point(start_x, bounds.origin.y);
                            let background_size = size((end_x - start_x).max(px(0.)), line_height);
                            window.paint_quad(fill(
                                Bounds::new(background_origin, background_size),
                                rgb(run.bg),
                            ));
                        }

                        let is_powerline = should_force_powerline(run);
                        let force_cell_width = run.force_cell_width || is_powerline;
                        let force_width = if force_cell_width {
                            Some(px(cell_width))
                        } else {
                            None
                        };

                        let shaped_line = window.text_system().shape_line(
                            run.text.clone().into(),
                            font_size,
                            &[TextRun {
                                len: run.text.len(),
                                font: mono_font.clone(),
                                color: rgb(run.fg).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            force_width,
                        );

                        let run_origin = bounds.origin.x + px(run.start_column as f32 * cell_width);
                        let run_x = if is_powerline || force_cell_width {
                            run_origin
                        } else {
                            run_origin.floor()
                        };

                        let _ = shaped_line.paint(
                            point(run_x, bounds.origin.y),
                            line_height,
                            window,
                            cx,
                        );
                    }
                },
            )
            .size_full(),
        )
}

pub(crate) fn should_force_powerline(run: &PositionedTerminalRun) -> bool {
    run.text.chars().count() == 1
        && run
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
}

pub(crate) fn snap_pixels_floor(value: Pixels, scale_factor: f32) -> Pixels {
    if !(scale_factor.is_finite() && scale_factor > 0.) {
        return value.floor();
    }

    let scaled = value.to_f64() as f32 * scale_factor;
    px(scaled.floor() / scale_factor)
}

pub(crate) fn snap_pixels_ceil(value: Pixels, scale_factor: f32) -> Pixels {
    if !(scale_factor.is_finite() && scale_factor > 0.) {
        return value.ceil();
    }

    let scaled = value.to_f64() as f32 * scale_factor;
    px(scaled.ceil() / scale_factor)
}

pub(crate) fn lines_for_display(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec!["<no output yet>".to_owned()];
    }

    text.lines().map(ToOwned::to_owned).collect()
}

pub(crate) fn terminal_display_lines(session: &TerminalSession) -> Vec<String> {
    if !session.styled_output.is_empty() {
        return session
            .styled_output
            .iter()
            .map(styled_line_to_string)
            .collect();
    }

    if session.output.is_empty() {
        return vec![String::new()];
    }

    session.output.lines().map(ToOwned::to_owned).collect()
}

pub(crate) fn styled_line_to_string(line: &TerminalStyledLine) -> String {
    let mut cells = if line.cells.is_empty() {
        cells_from_runs(&line.runs)
    } else {
        line.cells.clone()
    };
    if cells.is_empty() {
        return String::new();
    }

    cells.sort_by_key(|cell| cell.column);
    let mut output = String::new();
    let mut current_column = 0_usize;

    for cell in cells {
        while current_column < cell.column {
            output.push(' ');
            current_column = current_column.saturating_add(1);
        }
        output.push_str(&cell.text);
        current_column = current_column.saturating_add(1);
    }

    output
}

pub(crate) fn terminal_grid_position_from_pointer(
    position: gpui::Point<Pixels>,
    bounds: Bounds<Pixels>,
    scroll_offset: gpui::Point<Pixels>,
    line_height: f32,
    cell_width: f32,
    line_count: usize,
) -> Option<TerminalGridPosition> {
    if line_height <= 0. || cell_width <= 0. || line_count == 0 {
        return None;
    }

    let local_x = f32::from(position.x - bounds.left()).max(0.);
    let local_y = f32::from(position.y - bounds.top()).max(0.);
    let content_y = (local_y - f32::from(scroll_offset.y)).max(0.);

    let max_line = line_count.saturating_sub(1);
    let line = ((content_y / line_height).floor() as usize).min(max_line);
    let column = (local_x / cell_width).floor().max(0.) as usize;

    Some(TerminalGridPosition { line, column })
}

pub(crate) fn terminal_token_bounds(
    lines: &[String],
    point: TerminalGridPosition,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let line = lines.get(point.line)?;
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let index = point.column.min(chars.len().saturating_sub(1));
    if chars
        .get(index)
        .is_none_or(|character| character.is_whitespace())
    {
        return None;
    }

    let mut start = index;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }

    let mut end = index.saturating_add(1);
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }

    Some((
        TerminalGridPosition {
            line: point.line,
            column: start,
        },
        TerminalGridPosition {
            line: point.line,
            column: end,
        },
    ))
}

pub(crate) fn terminal_line_bounds(
    lines: &[String],
    point: TerminalGridPosition,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let line = lines.get(point.line)?;
    let width = line.chars().count();
    if width == 0 {
        return None;
    }

    Some((
        TerminalGridPosition {
            line: point.line,
            column: 0,
        },
        TerminalGridPosition {
            line: point.line,
            column: width,
        },
    ))
}

pub(crate) fn terminal_selection_text(lines: &[String], selection: &TerminalSelection) -> String {
    let Some((start, end)) = normalized_terminal_selection(selection) else {
        return String::new();
    };

    let mut output = String::new();
    for line_index in start.line..=end.line {
        let line = lines.get(line_index).map_or("", String::as_str);
        let chars: Vec<char> = line.chars().collect();

        let from = if line_index == start.line {
            start.column.min(chars.len())
        } else {
            0
        };
        let to = if line_index == end.line {
            end.column.min(chars.len())
        } else {
            chars.len()
        };

        if from < to {
            output.extend(chars[from..to].iter());
        }

        if line_index != end.line {
            output.push('\n');
        }
    }

    output
}

pub(crate) fn trim_to_last_lines(text: String, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text;
    }

    let mut trimmed = String::new();
    let start = lines.len().saturating_sub(max_lines);
    for line in lines.iter().skip(start) {
        trimmed.push_str(line);
        trimmed.push('\n');
    }
    trimmed
}

pub(crate) fn terminal_scroll_is_near_bottom(scroll_handle: &ScrollHandle) -> bool {
    let max_offset = scroll_handle.max_offset();
    if max_offset.height <= px(0.) {
        return true;
    }

    let offset = scroll_handle.offset();
    let distance_from_bottom = (offset.y + max_offset.height).abs();
    distance_from_bottom <= px(6.)
}

pub(crate) fn terminal_grid_size_from_scroll_handle(
    scroll_handle: &ScrollHandle,
    cx: &App,
) -> Option<(u16, u16, u16, u16)> {
    let bounds = scroll_handle.bounds();
    let width = (bounds.size.width.to_f64() as f32 - TERMINAL_SCROLLBAR_WIDTH_PX).max(1.);
    let height = bounds.size.height.to_f64() as f32;
    let cell_width = terminal_cell_width_px(cx);
    let line_height = terminal_line_height_px(cx);
    let (rows, cols) = terminal_grid_size_for_viewport(width, height, cell_width, line_height)?;
    let pixel_width = width.floor().clamp(1., f32::from(u16::MAX)) as u16;
    let pixel_height = height.floor().clamp(1., f32::from(u16::MAX)) as u16;
    Some((rows, cols, pixel_width, pixel_height))
}

pub(crate) fn terminal_cell_width_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);

    text_system
        .advance(font_id, px(TERMINAL_FONT_SIZE_PX), 'm')
        .map(|size| size.width.to_f64() as f32)
        .ok()
        .filter(|width| width.is_finite() && *width > 0.)
        .unwrap_or(TERMINAL_CELL_WIDTH_PX)
}

pub(crate) fn diff_cell_width_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);
    let fallback = (TERMINAL_CELL_WIDTH_PX * (DIFF_FONT_SIZE_PX / TERMINAL_FONT_SIZE_PX)).max(1.);

    text_system
        .advance(font_id, px(DIFF_FONT_SIZE_PX), 'm')
        .map(|size| size.width.to_f64() as f32)
        .ok()
        .filter(|width| width.is_finite() && *width > 0.)
        .unwrap_or(fallback)
}

pub(crate) fn terminal_line_height_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);
    let font_size = px(TERMINAL_FONT_SIZE_PX);

    let ascent = text_system.ascent(font_id, font_size).to_f64() as f32;
    let descent = text_system.descent(font_id, font_size).to_f64() as f32;
    let measured_height = if descent.is_sign_negative() {
        ascent - descent
    } else {
        ascent + descent
    };

    if measured_height.is_finite() && measured_height > 0. {
        return measured_height.ceil().max(TERMINAL_FONT_SIZE_PX).max(1.);
    }

    TERMINAL_CELL_HEIGHT_PX
}

pub(crate) fn terminal_grid_size_for_viewport(
    width: f32,
    height: f32,
    cell_width: f32,
    cell_height: f32,
) -> Option<(u16, u16)> {
    if width <= 0. || height <= 0. || cell_width <= 0. || cell_height <= 0. {
        return None;
    }

    let cols = (width / cell_width).floor() as i32;
    let rows = (height / cell_height).floor() as i32;
    if cols <= 0 || rows <= 0 {
        return None;
    }

    let cols = cols.clamp(2, i32::from(u16::MAX)) as u16;
    let rows = rows.clamp(1, i32::from(u16::MAX)) as u16;
    Some((rows, cols))
}

pub(crate) fn should_auto_follow_terminal_output(changed: bool, was_near_bottom: bool) -> bool {
    changed && was_near_bottom
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use {
        super::*,
        crate::{daemon_runtime::session_with_styled_line, theme::ThemeKind},
    };

    #[test]
    fn cursor_is_painted_at_terminal_column_instead_of_line_end() {
        let theme = ThemeKind::One.palette();
        let session = session_with_styled_line(
            "abcdef",
            0x112233,
            0x445566,
            Some(TerminalCursor { line: 0, column: 2 }),
        );

        let lines = styled_lines_for_session(&session, theme, true, None, None);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].runs.len(), 3);
        assert_eq!(lines[0].runs[0].text, "ab");
        assert_eq!(lines[0].runs[1].text, "c");
        assert_eq!(lines[0].runs[1].fg, 0x112233);
        assert_eq!(lines[0].runs[1].bg, theme.terminal_cursor);
        assert_eq!(lines[0].runs[2].text, "def");
    }

    #[test]
    fn cursor_pads_to_column_when_it_is_after_line_content() {
        let theme = ThemeKind::One.palette();
        let session = session_with_styled_line(
            "abc",
            0x112233,
            0x445566,
            Some(TerminalCursor { line: 0, column: 5 }),
        );

        let lines = styled_lines_for_session(&session, theme, true, None, None);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].runs.len(), 2);
        assert_eq!(lines[0].runs[0].text, "abc");
        assert_eq!(lines[0].runs[1].text, " ");
        assert_eq!(lines[0].runs[1].fg, theme.text_primary);
        assert_eq!(lines[0].runs[1].bg, theme.terminal_cursor);
        assert!(lines[0].cells.iter().any(|cell| {
            cell.column == 5 && cell.text == " " && cell.bg == theme.terminal_cursor
        }));
    }

    #[test]
    fn positioned_runs_split_cells_with_zero_width_sequences() {
        let cells = vec![
            TerminalStyledCell {
                column: 0,
                text: "A".to_owned(),
                fg: 0x112233,
                bg: 0x445566,
            },
            TerminalStyledCell {
                column: 1,
                text: "\u{2600}\u{fe0f}".to_owned(),
                fg: 0x112233,
                bg: 0x445566,
            },
            TerminalStyledCell {
                column: 2,
                text: "B".to_owned(),
                fg: 0x112233,
                bg: 0x445566,
            },
        ];

        let runs = positioned_runs_from_cells(&cells);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "A");
        assert_eq!(runs[0].start_column, 0);
        assert_eq!(runs[0].cell_count, 1);
        assert!(runs[0].force_cell_width);
        assert_eq!(runs[1].text, "\u{2600}\u{fe0f}");
        assert_eq!(runs[1].start_column, 1);
        assert_eq!(runs[1].cell_count, 1);
        assert!(!runs[1].force_cell_width);
        assert_eq!(runs[2].text, "B");
        assert_eq!(runs[2].start_column, 2);
        assert_eq!(runs[2].cell_count, 1);
        assert!(runs[2].force_cell_width);
    }

    #[test]
    fn positioned_runs_do_not_force_cell_width_for_powerline_symbols() {
        let cells = vec![
            TerminalStyledCell {
                column: 0,
                text: "\u{e0b0}".to_owned(),
                fg: 0xaabbcc,
                bg: 0x112233,
            },
            TerminalStyledCell {
                column: 1,
                text: "X".to_owned(),
                fg: 0xaabbcc,
                bg: 0x112233,
            },
        ];

        let runs = positioned_runs_from_cells(&cells);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "\u{e0b0}");
        assert!(!runs[0].force_cell_width);
        assert_eq!(runs[1].text, "X");
        assert!(runs[1].force_cell_width);
    }

    #[test]
    fn positioned_runs_keep_cell_width_for_box_drawing_symbols() {
        let cells = vec![
            TerminalStyledCell {
                column: 0,
                text: "\u{2502}".to_owned(),
                fg: 0xaabbcc,
                bg: 0x112233,
            },
            TerminalStyledCell {
                column: 1,
                text: "X".to_owned(),
                fg: 0xaabbcc,
                bg: 0x112233,
            },
        ];

        let runs = positioned_runs_from_cells(&cells);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "\u{2502}X");
        assert!(runs[0].force_cell_width);
    }

    #[test]
    fn powerline_glyph_is_forced_to_cell_width() {
        let run = PositionedTerminalRun {
            text: "\u{e0b6}".to_owned(),
            fg: 0,
            bg: 0,
            start_column: 7,
            cell_count: 1,
            force_cell_width: false,
        };

        assert!(should_force_powerline(&run));
    }

    #[test]
    fn token_bounds_capture_full_url() {
        let lines = vec!["visit https://example.com/path?q=1 please".to_owned()];
        let point = TerminalGridPosition {
            line: 0,
            column: 12,
        };

        let bounds = terminal_token_bounds(&lines, point);
        assert!(bounds.is_some());
        let (start, end) = bounds.expect("token bounds");
        let selection = TerminalSelection {
            session_id: 1,
            anchor: start,
            head: end,
        };
        let selected = terminal_selection_text(&lines, &selection);
        assert_eq!(selected, "https://example.com/path?q=1");
    }

    #[test]
    fn selection_text_spans_multiple_lines() {
        let lines = vec!["abc".to_owned(), "def".to_owned(), "ghi".to_owned()];
        let selection = TerminalSelection {
            session_id: 1,
            anchor: TerminalGridPosition { line: 0, column: 1 },
            head: TerminalGridPosition { line: 2, column: 2 },
        };

        let selected = terminal_selection_text(&lines, &selection);
        assert_eq!(selected, "bc\ndef\ngh");
    }

    #[test]
    fn line_bounds_capture_entire_line_on_triple_click() {
        let lines = vec!["hello world".to_owned()];
        let point = TerminalGridPosition { line: 0, column: 3 };

        let bounds = terminal_line_bounds(&lines, point);
        assert!(bounds.is_some());
        let (start, end) = bounds.expect("line bounds");
        assert_eq!(start.line, 0);
        assert_eq!(start.column, 0);
        assert_eq!(end.line, 0);
        assert_eq!(end.column, 11);
    }

    #[test]
    fn styled_lines_remap_embedded_default_palette_to_active_theme() {
        let theme = ThemeKind::Gruvbox.palette();
        let session = session_with_styled_line(
            "abc",
            EMBEDDED_TERMINAL_DEFAULT_FG,
            EMBEDDED_TERMINAL_DEFAULT_BG,
            None,
        );

        let lines = styled_lines_for_session(&session, theme, false, None, None);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0]
                .cells
                .iter()
                .all(|cell| cell.bg == theme.terminal_bg)
        );
        assert!(
            lines[0]
                .cells
                .iter()
                .all(|cell| cell.fg == theme.text_primary)
        );
    }

    #[test]
    fn auto_follow_requires_new_output_and_bottom_position() {
        assert!(should_auto_follow_terminal_output(true, true));
        assert!(!should_auto_follow_terminal_output(true, false));
        assert!(!should_auto_follow_terminal_output(false, true));
    }

    #[test]
    fn auto_follow_is_disabled_without_new_output() {
        assert!(!should_auto_follow_terminal_output(false, false));
    }

    #[test]
    fn computes_terminal_grid_size_from_viewport() {
        let result = terminal_grid_size_for_viewport(
            900.,
            380.,
            TERMINAL_CELL_WIDTH_PX,
            TERMINAL_CELL_HEIGHT_PX,
        );
        assert_eq!(result, Some((20, 100)));
    }
}
