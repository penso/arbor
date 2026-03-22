pub use arbor_terminal_emulator::{
    TerminalCursor, TerminalModes, TerminalStyledCell, TerminalStyledLine, TerminalStyledRun,
};
use {
    crate::TerminalError,
    arbor_terminal_emulator::{
        self, TERMINAL_COLS, TERMINAL_DEFAULT_BG, TERMINAL_DEFAULT_FG, TERMINAL_ROWS,
        TerminalEmulator, TerminalSnapshot,
    },
    portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system},
    std::{
        env,
        io::{Read, Write},
        path::Path,
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
}

pub enum TerminalLaunch {
    Embedded(EmbeddedTerminal),
}

#[derive(Clone)]
pub struct EmbeddedTerminal {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    emulator: Arc<Mutex<TerminalEmulator>>,
    snapshot: Arc<Mutex<EmbeddedSnapshot>>,
    snapshot_generation: Arc<AtomicU64>,
    exit_code: Arc<Mutex<Option<i32>>>,
    root_pid: Option<u32>,
    generation: Arc<AtomicU64>,
    killer: Arc<Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>>,
    size: Arc<Mutex<(u16, u16, u16, u16)>>,
    notify: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
}

pub type EmbeddedSnapshot = Arc<TerminalSnapshot>;

pub fn launch_backend(
    kind: TerminalBackendKind,
    cwd: &Path,
    initial_rows: u16,
    initial_cols: u16,
) -> Result<TerminalLaunch, TerminalError> {
    match kind {
        TerminalBackendKind::Embedded => {
            EmbeddedTerminal::spawn(cwd, initial_rows, initial_cols).map(TerminalLaunch::Embedded)
        },
    }
}

impl EmbeddedTerminal {
    pub fn spawn(cwd: &Path, initial_rows: u16, initial_cols: u16) -> Result<Self, TerminalError> {
        Self::spawn_with_command(cwd, initial_rows, initial_cols, None)
    }

    pub fn spawn_command(
        cwd: &Path,
        command: &str,
        initial_rows: u16,
        initial_cols: u16,
    ) -> Result<Self, TerminalError> {
        Self::spawn_with_command(cwd, initial_rows, initial_cols, Some(command))
    }

    fn spawn_with_command(
        cwd: &Path,
        initial_rows: u16,
        initial_cols: u16,
        command_text: Option<&str>,
    ) -> Result<Self, TerminalError> {
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
            .map_err(|error| TerminalError::Pty(format!("failed to create PTY: {error}")))?;

        let mut command = CommandBuilder::new(default_shell());
        if let Some(command_text) = command_text {
            command.arg("-c");
            command.arg(command_text);
        } else {
            command.arg("-l");
        }
        command.cwd(cwd.as_os_str());

        if env::var_os("TERM").is_none() {
            command.env("TERM", "xterm-256color");
        }
        if env::var_os("COLORTERM").is_none() {
            command.env("COLORTERM", "truecolor");
        }

        let child = pair.slave.spawn_command(command).map_err(|error| {
            TerminalError::Pty(format!("failed to spawn shell in PTY: {error}"))
        })?;
        let root_pid = child.process_id();
        let killer = child.clone_killer();

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalError::Pty(format!("failed to clone PTY reader: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalError::Pty(format!("failed to open PTY writer: {error}")))?;
        let master = pair.master;

        let emulator = Arc::new(Mutex::new(TerminalEmulator::with_size(rows, cols)));
        let snapshot = Arc::new(Mutex::new(empty_embedded_snapshot()));
        let snapshot_generation = Arc::new(AtomicU64::new(0));
        let exit_code = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(1));
        let killer = Arc::new(Mutex::new(Some(killer)));
        let size = Arc::new(Mutex::new((rows, cols, 0, 0)));
        let notify: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>> = Arc::new(Mutex::new(None));

        refresh_embedded_snapshot_cache(&emulator, &snapshot, None);
        snapshot_generation.store(1, Ordering::Relaxed);

        spawn_reader_thread(reader, emulator.clone(), generation.clone(), notify.clone());
        spawn_wait_thread(
            child,
            emulator.clone(),
            exit_code.clone(),
            killer.clone(),
            generation.clone(),
            notify.clone(),
        );

        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
            master: Arc::new(Mutex::new(master)),
            emulator,
            snapshot,
            snapshot_generation,
            exit_code,
            root_pid,
            generation,
            killer,
            size,
            notify,
        })
    }

    pub fn write_input(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        if bytes.is_empty() {
            return Ok(());
        }

        let mut writer = self
            .writer
            .lock()
            .map_err(|_| TerminalError::LockPoisoned("PTY writer"))?;
        writer
            .write_all(bytes)
            .map_err(|error| TerminalError::Pty(format!("failed to write to PTY: {error}")))?;
        writer
            .flush()
            .map_err(|error| TerminalError::Pty(format!("failed to flush PTY writer: {error}")))
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let current_generation = self.generation.load(Ordering::Relaxed);
        if self.snapshot_generation.load(Ordering::Relaxed) != current_generation {
            let exit_code = self
                .exit_code
                .lock()
                .ok()
                .map(|guard| *guard)
                .unwrap_or(None);
            refresh_embedded_snapshot_cache(&self.emulator, &self.snapshot, exit_code);
            self.snapshot_generation
                .store(current_generation, Ordering::Relaxed);
        }

        match self.snapshot.lock() {
            Ok(snapshot) => (*snapshot).as_ref().clone(),
            Err(poisoned) => poisoned.into_inner().as_ref().clone(),
        }
    }

    pub fn shared_snapshot(&self) -> Arc<TerminalSnapshot> {
        let current_generation = self.generation.load(Ordering::Relaxed);
        if self.snapshot_generation.load(Ordering::Relaxed) != current_generation {
            let exit_code = self
                .exit_code
                .lock()
                .ok()
                .map(|guard| *guard)
                .unwrap_or(None);
            refresh_embedded_snapshot_cache(&self.emulator, &self.snapshot, exit_code);
            self.snapshot_generation
                .store(current_generation, Ordering::Relaxed);
        }

        match self.snapshot.lock() {
            Ok(snapshot) => Arc::clone(&snapshot),
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), TerminalError> {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let pixel_width = pixel_width.max(1);
        let pixel_height = pixel_height.max(1);

        {
            let size = self
                .size
                .lock()
                .map_err(|_| TerminalError::LockPoisoned("terminal size"))?;
            if *size == (rows, cols, pixel_width, pixel_height) {
                return Ok(());
            }
        }

        {
            let mut emulator = self
                .emulator
                .lock()
                .map_err(|_| TerminalError::LockPoisoned("emulator"))?;
            emulator.resize(rows, cols);
        }

        {
            let master = self
                .master
                .lock()
                .map_err(|_| TerminalError::LockPoisoned("PTY master"))?;
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width,
                    pixel_height,
                })
                .map_err(|error| TerminalError::Pty(format!("failed to resize PTY: {error}")))?;
        }

        {
            let mut size = self
                .size
                .lock()
                .map_err(|_| TerminalError::LockPoisoned("terminal size"))?;
            *size = (rows, cols, pixel_width, pixel_height);
        }

        self.generation.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    pub fn set_notify(&self, sender: std::sync::mpsc::Sender<()>) {
        if let Ok(mut guard) = self.notify.lock() {
            *guard = Some(sender);
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn root_pid(&self) -> Option<u32> {
        self.root_pid
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

fn send_notify(notify: &Mutex<Option<std::sync::mpsc::Sender<()>>>) {
    if let Ok(guard) = notify.lock()
        && let Some(ref tx) = *guard
    {
        let _ = tx.send(());
    }
}

fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    emulator: Arc<Mutex<TerminalEmulator>>,
    generation: Arc<AtomicU64>,
    notify: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];

        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    process_embedded_terminal_bytes(&emulator, &generation, &buffer[..read]);
                    send_notify(&notify);
                },
                Err(error) => {
                    process_embedded_terminal_bytes(
                        &emulator,
                        &generation,
                        format!("\r\n[terminal reader error: {error}]\r\n").as_bytes(),
                    );
                    send_notify(&notify);
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
    notify: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
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

        process_embedded_terminal_bytes(&emulator, &generation, exit_message.as_bytes());
        send_notify(&notify);
    });
}

fn empty_embedded_snapshot() -> EmbeddedSnapshot {
    Arc::new(TerminalSnapshot {
        output: String::new(),
        styled_lines: vec![TerminalStyledLine {
            cells: Vec::new(),
            runs: Vec::new(),
        }],
        cursor: None,
        modes: TerminalModes::default(),
        exit_code: None,
    })
}

fn refresh_embedded_snapshot_cache(
    emulator: &Mutex<TerminalEmulator>,
    snapshot: &Mutex<EmbeddedSnapshot>,
    exit_code: Option<i32>,
) {
    let mut next_snapshot = match emulator.lock() {
        Ok(emulator) => emulator.snapshot(),
        Err(poisoned) => poisoned.into_inner().snapshot(),
    };
    next_snapshot.exit_code = exit_code;

    match snapshot.lock() {
        Ok(mut cached) => *cached = Arc::new(next_snapshot),
        Err(poisoned) => *poisoned.into_inner() = Arc::new(next_snapshot),
    }
}

fn process_embedded_terminal_bytes(
    emulator: &Mutex<TerminalEmulator>,
    generation: &AtomicU64,
    bytes: &[u8],
) {
    match emulator.lock() {
        Ok(mut emulator) => emulator.process(bytes),
        Err(poisoned) => poisoned.into_inner().process(bytes),
    }

    generation.fetch_add(1, Ordering::Relaxed);
}

fn default_shell() -> String {
    arbor_core::daemon::default_shell()
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

    #[test]
    fn embedded_snapshot_cache_tracks_exit_code() {
        let emulator = Mutex::new(TerminalEmulator::new());
        let snapshot = Mutex::new(empty_embedded_snapshot());

        process_embedded_terminal_bytes(&emulator, &AtomicU64::new(0), b"hello\r\n");
        refresh_embedded_snapshot_cache(&emulator, &snapshot, Some(7));

        let cached = snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(cached.exit_code, Some(7));
        assert!(cached.output.contains("hello"));
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
