pub use arbor_terminal_emulator::{
    TerminalCursor, TerminalStyledCell, TerminalStyledLine, TerminalStyledRun,
};
use {
    arbor_terminal_emulator::{
        self, TERMINAL_COLS, TERMINAL_DEFAULT_BG, TERMINAL_DEFAULT_FG, TERMINAL_ROWS,
        TerminalEmulator, TerminalSnapshot, process_terminal_bytes,
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

pub type EmbeddedSnapshot = TerminalSnapshot;

pub fn launch_backend(
    kind: TerminalBackendKind,
    cwd: &Path,
    initial_rows: u16,
    initial_cols: u16,
) -> Result<TerminalLaunch, String> {
    match kind {
        TerminalBackendKind::Embedded => {
            EmbeddedTerminal::spawn(cwd, initial_rows, initial_cols).map(TerminalLaunch::Embedded)
        },
        TerminalBackendKind::Alacritty => launch_alacritty(cwd).map(TerminalLaunch::External),
        TerminalBackendKind::Ghostty => launch_ghostty(cwd).map(TerminalLaunch::External),
    }
}

impl EmbeddedTerminal {
    pub fn spawn(cwd: &Path, initial_rows: u16, initial_cols: u16) -> Result<Self, String> {
        let rows = if initial_rows > 0 {
            initial_rows
        } else {
            TERMINAL_ROWS
        };
        let cols = if initial_cols > 1 {
            initial_cols
        } else {
            TERMINAL_COLS
        };
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
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

        let emulator = Arc::new(Mutex::new(TerminalEmulator::with_size(rows, cols)));
        let exit_code = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(1));
        let killer = Arc::new(Mutex::new(Some(killer)));
        let size = Arc::new(Mutex::new((rows, cols, 0, 0)));

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
                emulator.snapshot_output(),
                emulator.collect_styled_lines(),
                emulator.snapshot_cursor(),
            ),
            Err(poisoned) => {
                let emulator = poisoned.into_inner();
                (
                    emulator.snapshot_output(),
                    emulator.collect_styled_lines(),
                    emulator.snapshot_cursor(),
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

    pub fn close(&self) {
        let mut killer_guard = match self.killer.lock() {
            Ok(lock) => lock,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(killer) = killer_guard.as_mut() {
            let _ = killer.kill();
        }
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
    use {super::*, arbor_terminal_emulator::TerminalEmulator};

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
