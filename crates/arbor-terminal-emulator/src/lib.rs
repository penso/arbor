use {
    alacritty_terminal::{
        Term,
        event::VoidListener,
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

pub const TERMINAL_ROWS: u16 = 24;
pub const TERMINAL_COLS: u16 = 80;
pub const TERMINAL_SCROLLBACK: usize = 8_000;

pub const TERMINAL_DEFAULT_FG: u32 = 0xabb2bf;
pub const TERMINAL_DEFAULT_BG: u32 = 0x282c34;
pub const TERMINAL_CURSOR: u32 = 0x74ade8;
pub const TERMINAL_BRIGHT_FG: u32 = 0xdce0e5;
pub const TERMINAL_DIM_FG: u32 = 0x636d83;
pub const TERMINAL_ANSI_16: [u32; 16] = [
    0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x636d83,
    0xea858b, 0xaad581, 0xffd885, 0x85c1ff, 0xd398eb, 0x6ed5de, 0xfafafa,
];
pub const TERMINAL_ANSI_DIM_8: [u32; 8] = [
    0x3b3f4a, 0xa7545a, 0x6d8f59, 0xb8985b, 0x457cad, 0x8d54a0, 0x3c818a, 0x8f969b,
];

#[derive(Debug, Clone)]
pub struct TerminalSnapshot {
    pub output: String,
    pub styled_lines: Vec<TerminalStyledLine>,
    pub cursor: Option<TerminalCursor>,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursor {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StyledColor {
    fg: u32,
    bg: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStyledLine {
    pub cells: Vec<TerminalStyledCell>,
    pub runs: Vec<TerminalStyledRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStyledCell {
    pub column: usize,
    pub text: String,
    pub fg: u32,
    pub bg: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalStyledRun {
    pub text: String,
    pub fg: u32,
    pub bg: u32,
}

pub struct TerminalDimensions {
    rows: usize,
    cols: usize,
}

impl TerminalDimensions {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self { rows, cols }
    }
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

pub struct TerminalEmulator {
    term: Term<VoidListener>,
    processor: Processor<StdSyncHandler>,
}

impl TerminalEmulator {
    pub fn new() -> Self {
        Self::with_size(TERMINAL_ROWS, TERMINAL_COLS)
    }

    pub fn with_size(rows: u16, cols: u16) -> Self {
        let dimensions = TerminalDimensions {
            rows: usize::from(rows),
            cols: usize::from(cols),
        };
        let config = Config {
            scrolling_history: TERMINAL_SCROLLBACK,
            ..Config::default()
        };

        Self {
            term: Term::new(config, &dimensions, VoidListener),
            processor: Processor::<StdSyncHandler>::new(),
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let dimensions = TerminalDimensions {
            rows: usize::from(rows),
            cols: usize::from(cols),
        };
        self.term.resize(dimensions);
    }

    pub fn snapshot_output(&self) -> String {
        snapshot_output(&self.term)
    }

    pub fn snapshot_cursor(&self) -> Option<TerminalCursor> {
        snapshot_cursor(&self.term)
    }

    pub fn collect_styled_lines(&self) -> Vec<TerminalStyledLine> {
        collect_styled_lines(&self.term)
    }
}

impl Default for TerminalEmulator {
    fn default() -> Self {
        Self::new()
    }
}

pub fn process_terminal_bytes(
    emulator: &Arc<Mutex<TerminalEmulator>>,
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

fn snapshot_output(term: &Term<VoidListener>) -> String {
    let start = Point::new(term.topmost_line(), Column(0));
    let end = Point::new(term.bottommost_line(), term.last_column());
    term.bounds_to_string(start, end)
}

fn snapshot_cursor(term: &Term<VoidListener>) -> Option<TerminalCursor> {
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

fn collect_styled_lines(term: &Term<VoidListener>) -> Vec<TerminalStyledLine> {
    let grid = term.grid();
    let colors = term.colors();
    let top_line = grid.topmost_line().0;
    let bottom_line = grid.bottommost_line().0;
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

        trim_trailing_whitespace_cells(&mut cells);
        let runs = runs_from_cells(&cells);
        lines.push(TerminalStyledLine { cells, runs });
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

fn trim_trailing_whitespace_cells(cells: &mut Vec<TerminalStyledCell>) {
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

fn runs_from_cells(cells: &[TerminalStyledCell]) -> Vec<TerminalStyledRun> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_lines_include_scrollback_content() {
        let mut emulator = TerminalEmulator::new();

        for line_index in 0..120 {
            let line = format!("line-{line_index:03}\r\n");
            emulator.process(line.as_bytes());
        }

        let styled_lines = emulator.collect_styled_lines();
        assert!(
            styled_lines.len() > 60,
            "expected many lines from scrollback, got {}",
            styled_lines.len()
        );

        let first = styled_line_to_string(styled_lines.first());
        let last = styled_line_to_string(styled_lines.last());

        assert!(
            first.contains("line-000"),
            "expected first scrollback line to be present, got `{first}`"
        );
        assert!(
            last.contains("line-119"),
            "expected final line to be present, got `{last}`"
        );
    }

    #[test]
    fn plain_snapshot_output_includes_scrollback_content() {
        let mut emulator = TerminalEmulator::new();

        for line_index in 0..220 {
            let line = format!("output-{line_index:03}\r\n");
            emulator.process(line.as_bytes());
        }

        let snapshot = emulator.snapshot_output();
        assert!(
            snapshot.contains("output-000"),
            "expected oldest visible scrollback in snapshot output"
        );
        assert!(
            snapshot.contains("output-219"),
            "expected latest output in snapshot output"
        );

        let snapshot_line_count = snapshot.lines().count();
        assert!(
            snapshot_line_count > usize::from(TERMINAL_ROWS),
            "expected snapshot line count ({snapshot_line_count}) to exceed viewport rows ({TERMINAL_ROWS})",
        );
    }

    #[test]
    fn styled_lines_skip_space_after_zero_width_sequence() {
        let mut emulator = TerminalEmulator::new();
        emulator.process("A\u{2600}\u{fe0f}B\r\n".as_bytes());

        let styled_lines = emulator.collect_styled_lines();
        let rendered = styled_line_to_string(styled_lines.first());

        assert_eq!(rendered, "A\u{2600}\u{fe0f}B");
    }

    #[test]
    fn snapshot_cursor_respects_cursor_visibility_mode() {
        let mut emulator = TerminalEmulator::new();
        assert!(emulator.snapshot_cursor().is_some());

        emulator.process("\u{1b}[?25l".as_bytes());
        assert!(emulator.snapshot_cursor().is_none());

        emulator.process("\u{1b}[?25h".as_bytes());
        assert!(emulator.snapshot_cursor().is_some());
    }

    #[test]
    fn osc_1337_bel_terminated_silently_consumed() {
        let mut emulator = TerminalEmulator::new();
        let seq =
            "\x1b]1337;RemoteHost=penso@m4max\x07\x1b]1337;CurrentDir=/home\x07\x1b]133;C\x07";
        emulator.process(seq.as_bytes());
        let rendered = styled_lines_to_string(&emulator.collect_styled_lines());
        assert!(
            !rendered.contains("1337"),
            "BEL-terminated OSC leaked: {rendered:?}"
        );
    }

    fn styled_line_to_string(line: Option<&TerminalStyledLine>) -> String {
        line.map(|line| {
            line.runs
                .iter()
                .map(|run| run.text.as_str())
                .collect::<String>()
        })
        .unwrap_or_default()
    }

    fn styled_lines_to_string(lines: &[TerminalStyledLine]) -> String {
        lines
            .iter()
            .flat_map(|line| line.runs.iter())
            .map(|run| run.text.as_str())
            .collect()
    }
}
