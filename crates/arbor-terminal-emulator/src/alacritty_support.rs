use {
    crate::{
        TERMINAL_ANSI_16, TERMINAL_ANSI_DIM_8, TERMINAL_BRIGHT_FG, TERMINAL_CURSOR,
        TERMINAL_DEFAULT_BG, TERMINAL_DEFAULT_FG, TERMINAL_DIM_FG, TerminalCursor, TerminalModes,
        TerminalProcessReport, TerminalStyledCell, TerminalStyledLine, TerminalStyledRun,
        default_terminal_scrollback_lines,
    },
    alacritty_terminal::{
        Term,
        event::{Event, EventListener},
        grid::Dimensions,
        index::{Column, Line, Point},
        term::{
            Config, TermMode,
            cell::{Cell, Flags},
            color::Colors,
        },
        vte::ansi::{Color, NamedColor, Processor, StdSyncHandler},
    },
    std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) struct AlacrittyState {
    pub(crate) term: Term<AlacrittyEventListener>,
    pub(crate) processor: Processor<StdSyncHandler>,
    pub(crate) event_listener: AlacrittyEventListener,
}

pub(crate) struct TerminalDimensions {
    pub(crate) rows: usize,
    pub(crate) cols: usize,
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub(crate) fn new_state(rows: u16, cols: u16) -> AlacrittyState {
    let dimensions = TerminalDimensions {
        rows: usize::from(rows),
        cols: usize::from(cols),
    };
    let config = Config {
        scrolling_history: default_terminal_scrollback_lines(),
        ..Config::default()
    };
    let event_listener = AlacrittyEventListener::default();

    AlacrittyState {
        term: Term::new(config, &dimensions, event_listener.clone()),
        processor: Processor::<StdSyncHandler>::new(),
        event_listener,
    }
}

pub fn process_terminal_bytes(
    emulator: &Arc<Mutex<crate::TerminalEmulator>>,
    generation: &Arc<AtomicU64>,
    bytes: &[u8],
) {
    let mut guard = match emulator.lock() {
        Ok(lock) => lock,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.process(bytes);
    generation.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Default)]
pub(crate) struct AlacrittyEventListener {
    bell_count: Arc<AtomicU64>,
}

impl AlacrittyEventListener {
    pub(crate) fn take_process_report(&self) -> TerminalProcessReport {
        TerminalProcessReport {
            bell_count: self.bell_count.swap(0, Ordering::Relaxed) as usize,
        }
    }
}

impl EventListener for AlacrittyEventListener {
    fn send_event(&self, event: Event) {
        if matches!(event, Event::Bell) {
            self.bell_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg_attr(feature = "ghostty-vt-experimental", allow(dead_code))]
pub(crate) fn snapshot_output(term: &Term<AlacrittyEventListener>) -> String {
    let start = Point::new(term.topmost_line(), Column(0));
    let end = Point::new(term.bottommost_line(), term.last_column());
    term.bounds_to_string(start, end)
}

pub(crate) fn snapshot_output_tail(
    term: &Term<AlacrittyEventListener>,
    max_lines: usize,
) -> String {
    if max_lines == 0 {
        return String::new();
    }

    let top = term.topmost_line().0;
    let bottom = term.bottommost_line().0;
    let start_line = (bottom - (max_lines.saturating_sub(1) as i32)).max(top);
    let start = Point::new(Line(start_line), Column(0));
    let end = Point::new(term.bottommost_line(), term.last_column());
    term.bounds_to_string(start, end)
}

pub(crate) fn snapshot_cursor(term: &Term<AlacrittyEventListener>) -> Option<TerminalCursor> {
    if !term.mode().contains(TermMode::SHOW_CURSOR) {
        return None;
    }

    let grid = term.grid();
    let top = grid.topmost_line().0;
    let bottom = grid.bottommost_line().0;
    let cursor = grid.cursor.point;

    if cursor.line.0 < top || cursor.line.0 > bottom {
        return None;
    }

    let line = usize::try_from(cursor.line.0 - top).ok()?;
    let column = cursor.column.0;
    Some(TerminalCursor { line, column })
}

pub(crate) fn snapshot_cursor_tail(
    term: &Term<AlacrittyEventListener>,
    max_lines: usize,
) -> Option<TerminalCursor> {
    if max_lines == 0 {
        return None;
    }
    if !term.mode().contains(TermMode::SHOW_CURSOR) {
        return None;
    }

    let grid = term.grid();
    let top = grid.topmost_line().0;
    let bottom = grid.bottommost_line().0;
    let start_line = (bottom - (max_lines.saturating_sub(1) as i32)).max(top);
    let cursor = grid.cursor.point;

    if cursor.line.0 < start_line || cursor.line.0 > bottom {
        return None;
    }

    let line = usize::try_from(cursor.line.0 - start_line).ok()?;
    let column = cursor.column.0;
    Some(TerminalCursor { line, column })
}

#[cfg_attr(feature = "ghostty-vt-experimental", allow(dead_code))]
pub(crate) fn snapshot_modes(term: &Term<AlacrittyEventListener>) -> TerminalModes {
    let mode = term.mode();
    TerminalModes {
        app_cursor: mode.contains(TermMode::APP_CURSOR),
        alt_screen: mode.contains(TermMode::ALT_SCREEN),
    }
}

pub(crate) fn collect_styled_lines(term: &Term<AlacrittyEventListener>) -> Vec<TerminalStyledLine> {
    let grid = term.grid();
    let colors = term.colors();
    let top_line = grid.topmost_line().0;
    let bottom_line = grid.bottommost_line().0;
    collect_styled_lines_between(term, colors, top_line, bottom_line)
}

pub(crate) fn collect_styled_lines_tail(
    term: &Term<AlacrittyEventListener>,
    max_lines: usize,
) -> Vec<TerminalStyledLine> {
    if max_lines == 0 {
        return vec![finalize_styled_line(Vec::new())];
    }

    let grid = term.grid();
    let colors = term.colors();
    let top_line = grid.topmost_line().0;
    let bottom_line = grid.bottommost_line().0;
    let start_line = (bottom_line - (max_lines.saturating_sub(1) as i32)).max(top_line);
    collect_styled_lines_between(term, colors, start_line, bottom_line)
}

fn collect_styled_lines_between(
    term: &Term<AlacrittyEventListener>,
    colors: &Colors,
    top_line: i32,
    bottom_line: i32,
) -> Vec<TerminalStyledLine> {
    let grid = term.grid();
    let columns = grid.columns();

    let mut lines = Vec::new();

    for line_index in top_line..=bottom_line {
        let row = &grid[Line(line_index)];
        let mut cells: Vec<TerminalStyledCell> = Vec::with_capacity(columns);
        let mut previous_cell_had_extras = false;

        for column_index in 0..columns {
            let cell = &row[Column(column_index)];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            if cell.c == ' ' && previous_cell_had_extras {
                previous_cell_had_extras = false;
                continue;
            }
            previous_cell_had_extras = matches!(cell.zerowidth(), Some(chars) if !chars.is_empty());

            let style = resolve_cell_color(cell, colors);
            let text = cell_text(cell);
            cells.push(TerminalStyledCell {
                column: column_index,
                text,
                fg: style.fg,
                bg: style.bg,
            });
        }

        lines.push(finalize_styled_line(cells));
    }

    while lines.last().is_some_and(|line| line.cells.is_empty()) {
        lines.pop();
    }

    if lines.is_empty() {
        lines.push(TerminalStyledLine {
            cells: Vec::new(),
            runs: Vec::new(),
        });
    }

    lines
}

fn cell_text(cell: &Cell) -> String {
    let mut text = String::new();
    text.push(cell.c);
    if let Some(extra) = cell.zerowidth() {
        for character in extra {
            text.push(*character);
        }
    }
    text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StyledColor {
    fg: u32,
    bg: u32,
}

fn resolve_cell_color(cell: &Cell, colors: &Colors) -> StyledColor {
    let mut fg = color_to_rgb(cell.fg, colors, TERMINAL_DEFAULT_FG);
    let mut bg = color_to_rgb(cell.bg, colors, TERMINAL_DEFAULT_BG);

    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }

    StyledColor { fg, bg }
}

fn color_to_rgb(color: Color, colors: &Colors, default: u32) -> u32 {
    match color {
        Color::Spec(rgb) => (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b),
        Color::Indexed(index) => colors[usize::from(index)]
            .map(rgb_to_u32)
            .unwrap_or_else(|| ansi_256_to_rgb(index)),
        Color::Named(named) => {
            let index = named as usize;
            colors[index]
                .map(rgb_to_u32)
                .unwrap_or_else(|| named_color_to_rgb(named, default))
        },
    }
}

fn rgb_to_u32(rgb: alacritty_terminal::vte::ansi::Rgb) -> u32 {
    (u32::from(rgb.r) << 16) | (u32::from(rgb.g) << 8) | u32::from(rgb.b)
}

fn named_color_to_rgb(color: NamedColor, default: u32) -> u32 {
    match color {
        NamedColor::Black => TERMINAL_ANSI_16[0],
        NamedColor::Red => TERMINAL_ANSI_16[1],
        NamedColor::Green => TERMINAL_ANSI_16[2],
        NamedColor::Yellow => TERMINAL_ANSI_16[3],
        NamedColor::Blue => TERMINAL_ANSI_16[4],
        NamedColor::Magenta => TERMINAL_ANSI_16[5],
        NamedColor::Cyan => TERMINAL_ANSI_16[6],
        NamedColor::White => TERMINAL_ANSI_16[7],
        NamedColor::BrightBlack => TERMINAL_ANSI_16[8],
        NamedColor::BrightRed => TERMINAL_ANSI_16[9],
        NamedColor::BrightGreen => TERMINAL_ANSI_16[10],
        NamedColor::BrightYellow => TERMINAL_ANSI_16[11],
        NamedColor::BrightBlue => TERMINAL_ANSI_16[12],
        NamedColor::BrightMagenta => TERMINAL_ANSI_16[13],
        NamedColor::BrightCyan => TERMINAL_ANSI_16[14],
        NamedColor::BrightWhite => TERMINAL_ANSI_16[15],
        NamedColor::Foreground => default,
        NamedColor::Background => TERMINAL_DEFAULT_BG,
        NamedColor::Cursor => TERMINAL_CURSOR,
        NamedColor::DimBlack => TERMINAL_ANSI_DIM_8[0],
        NamedColor::DimRed => TERMINAL_ANSI_DIM_8[1],
        NamedColor::DimGreen => TERMINAL_ANSI_DIM_8[2],
        NamedColor::DimYellow => TERMINAL_ANSI_DIM_8[3],
        NamedColor::DimBlue => TERMINAL_ANSI_DIM_8[4],
        NamedColor::DimMagenta => TERMINAL_ANSI_DIM_8[5],
        NamedColor::DimCyan => TERMINAL_ANSI_DIM_8[6],
        NamedColor::DimWhite => TERMINAL_ANSI_DIM_8[7],
        NamedColor::BrightForeground => TERMINAL_BRIGHT_FG,
        NamedColor::DimForeground => TERMINAL_DIM_FG,
    }
}

pub(crate) fn finalize_styled_line(mut cells: Vec<TerminalStyledCell>) -> TerminalStyledLine {
    trim_trailing_whitespace_cells(&mut cells);
    let runs = runs_from_cells(&cells);
    TerminalStyledLine { cells, runs }
}

pub(crate) fn render_ansi_from_styled_lines(
    lines: &[TerminalStyledLine],
    cursor: Option<TerminalCursor>,
    max_lines: usize,
) -> String {
    let keep_from = if max_lines == 0 {
        0
    } else {
        lines.len().saturating_sub(max_lines)
    };

    let mut output = String::new();
    for (index, line) in lines[keep_from..].iter().enumerate() {
        if index > 0 {
            output.push_str("\r\n");
        }
        for run in &line.runs {
            let r_fg = (run.fg >> 16) & 0xFF;
            let g_fg = (run.fg >> 8) & 0xFF;
            let b_fg = run.fg & 0xFF;

            if run.bg != TERMINAL_DEFAULT_BG {
                let r_bg = (run.bg >> 16) & 0xFF;
                let g_bg = (run.bg >> 8) & 0xFF;
                let b_bg = run.bg & 0xFF;
                output.push_str(&format!(
                    "\x1b[38;2;{r_fg};{g_fg};{b_fg};48;2;{r_bg};{g_bg};{b_bg}m"
                ));
            } else {
                output.push_str(&format!("\x1b[38;2;{r_fg};{g_fg};{b_fg}m"));
            }
            output.push_str(&run.text);
        }
        output.push_str("\x1b[0m");
    }

    if let Some(cursor) = cursor {
        let emitted_lines = lines.len().saturating_sub(keep_from);
        let cursor_line_in_emitted = cursor.line.saturating_sub(keep_from);
        let lines_up = emitted_lines
            .saturating_sub(1)
            .saturating_sub(cursor_line_in_emitted);
        if lines_up > 0 {
            output.push_str(&format!("\x1b[{lines_up}A"));
        }
        output.push_str(&format!("\x1b[{}G", cursor.column + 1));
    }

    output
}

pub(crate) fn trim_trailing_whitespace_cells(cells: &mut Vec<TerminalStyledCell>) {
    while let Some(last_cell) = cells.last() {
        if last_cell.bg != TERMINAL_DEFAULT_BG {
            break;
        }

        if last_cell.text.chars().all(|character| character == ' ') {
            cells.pop();
            continue;
        }
        break;
    }
}

pub(crate) fn runs_from_cells(cells: &[TerminalStyledCell]) -> Vec<TerminalStyledRun> {
    let mut runs = Vec::new();
    let mut current_style: Option<StyledColor> = None;
    let mut current_text = String::new();
    let mut next_expected_column: Option<usize> = None;

    for cell in cells {
        let style = StyledColor {
            fg: cell.fg,
            bg: cell.bg,
        };

        let gap_breaks_run = next_expected_column != Some(cell.column);
        if current_style != Some(style) || gap_breaks_run {
            if let Some(previous_style) = current_style.take()
                && !current_text.is_empty()
            {
                runs.push(TerminalStyledRun {
                    text: std::mem::take(&mut current_text),
                    fg: previous_style.fg,
                    bg: previous_style.bg,
                });
            }
            current_style = Some(style);
        }

        current_text.push_str(&cell.text);
        next_expected_column = Some(cell.column.saturating_add(1));
    }

    if let Some(style) = current_style
        && !current_text.is_empty()
    {
        runs.push(TerminalStyledRun {
            text: current_text,
            fg: style.fg,
            bg: style.bg,
        });
    }

    runs
}

fn ansi_256_to_rgb(index: u8) -> u32 {
    if usize::from(index) < TERMINAL_ANSI_16.len() {
        return TERMINAL_ANSI_16[usize::from(index)];
    }

    if (16..=231).contains(&index) {
        let index = index - 16;
        let red = index / 36;
        let green = (index % 36) / 6;
        let blue = index % 6;
        let channel = |value: u8| -> u8 {
            if value == 0 {
                0
            } else {
                value.saturating_mul(40).saturating_add(55)
            }
        };

        let red = channel(red);
        let green = channel(green);
        let blue = channel(blue);
        return (u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue);
    }

    let gray = 8_u8.saturating_add(index.saturating_sub(232).saturating_mul(10));
    (u32::from(gray) << 16) | (u32::from(gray) << 8) | u32::from(gray)
}
