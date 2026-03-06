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
    portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system},
    std::{
        env,
        io::{Read, Write},
        path::Path,
        process::{Command, Stdio},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        thread,
    },
};

const TERMINAL_ROWS: u16 = 56;
const TERMINAL_COLS: u16 = 180;
const TERMINAL_SCROLLBACK: usize = 8_000;

const TERMINAL_DEFAULT_FG: u32 = 0xabb2bf;
const TERMINAL_DEFAULT_BG: u32 = 0x282c34;
const TERMINAL_CURSOR: u32 = 0x74ade8;
const TERMINAL_BRIGHT_FG: u32 = 0xdce0e5;
const TERMINAL_DIM_FG: u32 = 0x636d83;
const TERMINAL_ANSI_16: [u32; 16] = [
    0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, 0x636d83,
    0xea858b, 0xaad581, 0xffd885, 0x85c1ff, 0xd398eb, 0x6ed5de, 0xfafafa,
];
const TERMINAL_ANSI_DIM_8: [u32; 8] = [
    0x3b3f4a, 0xa7545a, 0x6d8f59, 0xb8985b, 0x457cad, 0x8d54a0, 0x3c818a, 0x8f969b,
];

pub const EMBEDDED_TERMINAL_DEFAULT_FG: u32 = TERMINAL_DEFAULT_FG;
pub const EMBEDDED_TERMINAL_DEFAULT_BG: u32 = TERMINAL_DEFAULT_BG;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalBackendKind {
    Embedded,
    Alacritty,
    Ghostty,
}

#[derive(Debug, Clone)]
pub struct TerminalRunResult {
    pub command: String,
    pub output: String,
    pub success: bool,
    pub code: Option<i32>,
}

pub enum TerminalLaunch {
    Embedded(EmbeddedTerminal),
    External(TerminalRunResult),
}

#[derive(Clone)]
pub struct EmbeddedTerminal {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    emulator: Arc<Mutex<TerminalEmulator>>,
    exit_code: Arc<Mutex<Option<i32>>>,
    generation: Arc<AtomicU64>,
    killer: Arc<Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>>,
    size: Arc<Mutex<(u16, u16, u16, u16)>>,
}

#[derive(Debug, Clone)]
pub struct EmbeddedSnapshot {
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

struct TerminalDimensions {
    rows: usize,
    cols: usize,
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

struct TerminalEmulator {
    term: Term<VoidListener>,
    processor: Processor<StdSyncHandler>,
}

impl TerminalEmulator {
    fn new() -> Self {
        let dimensions = TerminalDimensions {
            rows: usize::from(TERMINAL_ROWS),
            cols: usize::from(TERMINAL_COLS),
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

    fn process(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        let dimensions = TerminalDimensions {
            rows: usize::from(rows),
            cols: usize::from(cols),
        };
        self.term.resize(dimensions);
    }
}

pub fn launch_backend(kind: TerminalBackendKind, cwd: &Path) -> Result<TerminalLaunch, String> {
    match kind {
        TerminalBackendKind::Embedded => EmbeddedTerminal::spawn(cwd).map(TerminalLaunch::Embedded),
        TerminalBackendKind::Alacritty => launch_alacritty(cwd).map(TerminalLaunch::External),
        TerminalBackendKind::Ghostty => launch_ghostty(cwd).map(TerminalLaunch::External),
    }
}

impl EmbeddedTerminal {
    pub fn spawn(cwd: &Path) -> Result<Self, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: TERMINAL_ROWS,
                cols: TERMINAL_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("failed to create PTY: {error}"))?;

        let mut command = CommandBuilder::new(default_shell());
        command.arg("-l");
        command.cwd(cwd.as_os_str());

        if env::var_os("TERM").is_none() {
            command.env("TERM", "xterm-256color");
        }
        if env::var_os("COLORTERM").is_none() {
            command.env("COLORTERM", "truecolor");
        }

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("failed to spawn shell in PTY: {error}"))?;
        let killer = child.clone_killer();

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("failed to clone PTY reader: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("failed to open PTY writer: {error}"))?;
        let master = pair.master;

        let emulator = Arc::new(Mutex::new(TerminalEmulator::new()));
        let exit_code = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(1));
        let killer = Arc::new(Mutex::new(Some(killer)));
        let size = Arc::new(Mutex::new((TERMINAL_ROWS, TERMINAL_COLS, 0, 0)));

        spawn_reader_thread(reader, emulator.clone(), generation.clone());
        spawn_wait_thread(
            child,
            emulator.clone(),
            exit_code.clone(),
            killer.clone(),
            generation.clone(),
        );

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            master: Arc::new(Mutex::new(master)),
            emulator,
            exit_code,
            generation,
            killer,
            size,
        })
    }

    pub fn write_input(&self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() {
            return Ok(());
        }

        let mut writer = self
            .writer
            .lock()
            .map_err(|_| "failed to acquire PTY writer lock".to_owned())?;
        writer
            .write_all(bytes)
            .map_err(|error| format!("failed to write to PTY: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("failed to flush PTY writer: {error}"))
    }

    pub fn snapshot(&self) -> EmbeddedSnapshot {
        let (output, styled_lines, cursor) = match self.emulator.lock() {
            Ok(emulator) => (
                snapshot_output(&emulator.term),
                collect_styled_lines(&emulator.term),
                snapshot_cursor(&emulator.term),
            ),
            Err(poisoned) => {
                let emulator = poisoned.into_inner();
                (
                    snapshot_output(&emulator.term),
                    collect_styled_lines(&emulator.term),
                    snapshot_cursor(&emulator.term),
                )
            },
        };
        let exit_code = match self.exit_code.lock() {
            Ok(code) => *code,
            Err(poisoned) => *poisoned.into_inner(),
        };

        EmbeddedSnapshot {
            output,
            styled_lines,
            cursor,
            exit_code,
        }
    }

    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), String> {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let pixel_width = pixel_width.max(1);
        let pixel_height = pixel_height.max(1);

        {
            let size = self
                .size
                .lock()
                .map_err(|_| "failed to acquire terminal size lock".to_owned())?;
            if *size == (rows, cols, pixel_width, pixel_height) {
                return Ok(());
            }
        }

        {
            let mut emulator = self
                .emulator
                .lock()
                .map_err(|_| "failed to acquire emulator lock for resize".to_owned())?;
            emulator.resize(rows, cols);
        }

        {
            let master = self
                .master
                .lock()
                .map_err(|_| "failed to acquire PTY master lock for resize".to_owned())?;
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width,
                    pixel_height,
                })
                .map_err(|error| format!("failed to resize PTY: {error}"))?;
        }

        {
            let mut size = self
                .size
                .lock()
                .map_err(|_| "failed to update terminal size lock".to_owned())?;
            *size = (rows, cols, pixel_width, pixel_height);
        }

        self.generation.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

impl Drop for EmbeddedTerminal {
    fn drop(&mut self) {
        if Arc::strong_count(&self.killer) != 1 {
            return;
        }

        let mut killer_guard = match self.killer.lock() {
            Ok(lock) => lock,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(killer) = killer_guard.as_mut() {
            let _ = killer.kill();
        }
    }
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    emulator: Arc<Mutex<TerminalEmulator>>,
    generation: Arc<AtomicU64>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => process_terminal_bytes(&emulator, &generation, &buffer[..read]),
                Err(error) => {
                    process_terminal_bytes(
                        &emulator,
                        &generation,
                        format!("\r\n[terminal reader error: {error}]\r\n").as_bytes(),
                    );
                    break;
                },
            }
        }
    });
}

fn spawn_wait_thread(
    child: Box<dyn Child + Send + Sync>,
    emulator: Arc<Mutex<TerminalEmulator>>,
    exit_code: Arc<Mutex<Option<i32>>>,
    killer: Arc<Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>>,
    generation: Arc<AtomicU64>,
) {
    thread::spawn(move || {
        let mut child = child;
        let status = child.wait();

        let (final_code, exit_message) = match status {
            Ok(status) => {
                let code = i32::try_from(status.exit_code()).unwrap_or(i32::MAX);
                let message = format!("\n\n[session exited with code {code}]\n");
                (Some(code), message)
            },
            Err(error) => (
                Some(1),
                format!("\n\n[session failed to wait for process exit: {error}]\n"),
            ),
        };

        {
            let mut exit_guard = match exit_code.lock() {
                Ok(lock) => lock,
                Err(poisoned) => poisoned.into_inner(),
            };
            *exit_guard = final_code;
        }

        {
            let mut killer_guard = match killer.lock() {
                Ok(lock) => lock,
                Err(poisoned) => poisoned.into_inner(),
            };
            *killer_guard = None;
        }

        process_terminal_bytes(&emulator, &generation, exit_message.as_bytes());
    });
}

fn process_terminal_bytes(
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

fn default_shell() -> String {
    arbor_core::daemon::default_shell()
}

fn launch_alacritty(cwd: &Path) -> Result<TerminalRunResult, String> {
    let shell = default_shell();
    let script = "printf 'Arbor external terminal session\\n'; exec $SHELL -l";
    let cwd_display = cwd.display().to_string();

    let direct_args = vec![
        "--working-directory".to_owned(),
        cwd_display.clone(),
        "-e".to_owned(),
        shell.clone(),
        "-lc".to_owned(),
        script.to_owned(),
    ];

    let launched_command = match run_detached("alacritty", &direct_args, cwd) {
        Ok(()) => format!("alacritty {}", render_args(&direct_args)),
        Err(direct_error) => {
            #[cfg(target_os = "macos")]
            {
                let app_args = vec![
                    "-na".to_owned(),
                    "Alacritty.app".to_owned(),
                    "--args".to_owned(),
                    "--working-directory".to_owned(),
                    cwd_display,
                    "-e".to_owned(),
                    shell,
                    "-lc".to_owned(),
                    script.to_owned(),
                ];

                match run_detached("open", &app_args, cwd) {
                    Ok(()) => format!("open {}", render_args(&app_args)),
                    Err(bundle_error) => {
                        return Err(format!(
                            "unable to launch Alacritty directly ({direct_error}) or via app bundle ({bundle_error})",
                        ));
                    },
                }
            }

            #[cfg(not(target_os = "macos"))]
            {
                return Err(format!("unable to launch Alacritty: {direct_error}"));
            }
        },
    };

    Ok(external_launch_result("Alacritty", launched_command))
}

fn launch_ghostty(cwd: &Path) -> Result<TerminalRunResult, String> {
    let shell = default_shell();
    let script = "printf 'Arbor external terminal session\\n'; exec $SHELL -l";
    let cwd_flag = format!("--working-directory={}", cwd.display());

    #[cfg(target_os = "macos")]
    {
        let app_args = vec![
            "-na".to_owned(),
            "Ghostty.app".to_owned(),
            "--args".to_owned(),
            cwd_flag,
            "-e".to_owned(),
            shell,
            "-lc".to_owned(),
            script.to_owned(),
        ];

        run_detached("open", &app_args, cwd).map_err(|error| {
            format!(
                "unable to launch Ghostty via app bundle. Install Ghostty.app in /Applications or adjust PATH: {error}",
            )
        })?;

        Ok(external_launch_result(
            "Ghostty",
            format!("open {}", render_args(&app_args)),
        ))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let args = vec![
            cwd_flag,
            "-e".to_owned(),
            shell,
            "-lc".to_owned(),
            script.to_owned(),
        ];
        run_detached("ghostty", &args, cwd)
            .map_err(|error| format!("unable to launch Ghostty: {error}"))?;

        Ok(external_launch_result(
            "Ghostty",
            format!("ghostty {}", render_args(&args)),
        ))
    }
}

fn run_detached(program: &str, args: &[String], cwd: &Path) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    command.spawn().map(|_| ()).map_err(|error| {
        format!(
            "failed to spawn `{program}` with args [{}]: {error}",
            render_args(args),
        )
    })
}

fn render_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| shell_escape(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_escape(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_owned();
    }

    let needs_quotes = arg
        .chars()
        .any(|ch| ch.is_whitespace() || ch == '\'' || ch == '"');
    if !needs_quotes {
        return arg.to_owned();
    }

    let escaped = arg.replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn external_launch_result(backend_label: &str, command: String) -> TerminalRunResult {
    TerminalRunResult {
        command,
        output: format!(
            "{backend_label} opened in an external window.\nUse that window for interactive work.",
        ),
        success: true,
        code: Some(0),
    }
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

        let styled_lines = collect_styled_lines(&emulator.term);
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

        let snapshot = snapshot_output(&emulator.term);
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
        emulator.process("A☀️B\r\n".as_bytes());

        let styled_lines = collect_styled_lines(&emulator.term);
        let rendered = styled_line_to_string(styled_lines.first());

        assert_eq!(rendered, "A☀️B");
    }

    #[test]
    fn snapshot_cursor_respects_cursor_visibility_mode() {
        let mut emulator = TerminalEmulator::new();
        assert!(snapshot_cursor(&emulator.term).is_some());

        emulator.process("\u{1b}[?25l".as_bytes());
        assert!(snapshot_cursor(&emulator.term).is_none());

        emulator.process("\u{1b}[?25h".as_bytes());
        assert!(snapshot_cursor(&emulator.term).is_some());
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
}
