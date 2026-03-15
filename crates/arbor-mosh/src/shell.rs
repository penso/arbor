use {
    crate::{MoshError, MoshHandshakeResult},
    arbor_terminal_emulator::{
        TERMINAL_COLS, TERMINAL_ROWS, TerminalEmulator, TerminalSnapshot, process_terminal_bytes,
    },
    portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system},
    std::{
        io::{Read, Write},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        thread,
    },
};

#[derive(Clone)]
pub struct MoshShell {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    emulator: Arc<Mutex<TerminalEmulator>>,
    exit_code: Arc<Mutex<Option<i32>>>,
    generation: Arc<AtomicU64>,
    killer: Arc<Mutex<Option<Box<dyn ChildKiller + Send + Sync>>>>,
    size: Arc<Mutex<(u16, u16, u16, u16)>>,
    notify: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>>,
}

impl MoshShell {
    pub fn spawn(handshake: MoshHandshakeResult, cols: u16, rows: u16) -> Result<Self, MoshError> {
        let cols = if cols == 0 {
            TERMINAL_COLS
        } else {
            cols
        };
        let rows = if rows == 0 {
            TERMINAL_ROWS
        } else {
            rows
        };

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| MoshError::Pty(format!("failed to create PTY: {error}")))?;

        let mut command = CommandBuilder::new("mosh-client");
        command.arg(&handshake.hostname);
        command.arg(handshake.port.to_string());
        command.env("MOSH_KEY", &handshake.key);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| MoshError::Pty(format!("failed to spawn mosh-client: {error}")))?;
        let killer = child.clone_killer();

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| MoshError::Pty(format!("failed to clone PTY reader: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| MoshError::Pty(format!("failed to open PTY writer: {error}")))?;
        let master = pair.master;

        let emulator = Arc::new(Mutex::new(TerminalEmulator::with_size(rows, cols)));
        let exit_code = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(1));
        let killer = Arc::new(Mutex::new(Some(killer)));
        let size = Arc::new(Mutex::new((rows, cols, 0, 0)));
        let notify: Arc<Mutex<Option<std::sync::mpsc::Sender<()>>>> = Arc::new(Mutex::new(None));

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
            exit_code,
            generation,
            killer,
            size,
            notify,
        })
    }

    pub fn write_input(&self, bytes: &[u8]) -> Result<(), MoshError> {
        if bytes.is_empty() {
            return Ok(());
        }

        let mut writer = self
            .writer
            .lock()
            .map_err(|_| MoshError::Io("failed to acquire mosh PTY writer lock".to_owned()))?;
        writer
            .write_all(bytes)
            .map_err(|error| MoshError::Io(format!("failed to write to mosh PTY: {error}")))?;
        writer
            .flush()
            .map_err(|error| MoshError::Io(format!("failed to flush mosh PTY writer: {error}")))
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let mut snapshot = match self.emulator.lock() {
            Ok(emulator) => emulator.snapshot(),
            Err(poisoned) => poisoned.into_inner().snapshot(),
        };
        let exit_code = match self.exit_code.lock() {
            Ok(code) => *code,
            Err(poisoned) => *poisoned.into_inner(),
        };

        snapshot.exit_code = exit_code;
        snapshot
    }

    pub fn resize(
        &self,
        rows: u16,
        cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), MoshError> {
        let rows = rows.max(1);
        let cols = cols.max(2);
        let pixel_width = pixel_width.max(1);
        let pixel_height = pixel_height.max(1);

        {
            let size = self.size.lock().map_err(|_| {
                MoshError::Io("failed to acquire mosh terminal size lock".to_owned())
            })?;
            if *size == (rows, cols, pixel_width, pixel_height) {
                return Ok(());
            }
        }

        {
            let mut emulator = self.emulator.lock().map_err(|_| {
                MoshError::Io("failed to acquire mosh emulator lock for resize".to_owned())
            })?;
            emulator.resize(rows, cols);
        }

        {
            let master = self.master.lock().map_err(|_| {
                MoshError::Io("failed to acquire mosh PTY master lock for resize".to_owned())
            })?;
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width,
                    pixel_height,
                })
                .map_err(|error| MoshError::Pty(format!("failed to resize mosh PTY: {error}")))?;
        }

        {
            let mut size = self.size.lock().map_err(|_| {
                MoshError::Io("failed to update mosh terminal size lock".to_owned())
            })?;
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

impl Drop for MoshShell {
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
                    process_terminal_bytes(&emulator, &generation, &buffer[..read]);
                    send_notify(&notify);
                },
                Err(error) => {
                    process_terminal_bytes(
                        &emulator,
                        &generation,
                        format!("\r\n[mosh reader error: {error}]\r\n").as_bytes(),
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
                let message = format!("\n\n[mosh session exited with code {code}]\n");
                (Some(code), message)
            },
            Err(error) => (
                Some(1),
                format!("\n\n[mosh session failed to wait for process exit: {error}]\n"),
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
        send_notify(&notify);
    });
}
