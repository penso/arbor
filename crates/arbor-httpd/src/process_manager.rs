use {
    arbor_core::{
        daemon::{
            CreateOrAttachRequest, KillRequest, TerminalDaemon, TerminalSessionState, default_shell,
        },
        process::{ProcessInfo, ProcessStatus},
        repo_config,
    },
    serde::Serialize,
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    },
    tokio::sync::broadcast,
};

const MAX_BACKOFF_SECS: u64 = 30;
const BACKOFF_RESET_SECS: u64 = 60;

pub type ProcessConfig = repo_config::ProcessConfig;

/// Internal state for each managed process.
struct ManagedProcess {
    config: ProcessConfig,
    status: ProcessStatus,
    session_id: Option<String>,
    exit_code: Option<i32>,
    restart_count: u32,
    last_start: Option<Instant>,
    current_backoff_secs: u64,
}

impl ManagedProcess {
    fn from_config(config: ProcessConfig) -> Self {
        Self {
            config,
            status: ProcessStatus::Stopped,
            session_id: None,
            exit_code: None,
            restart_count: 0,
            last_start: None,
            current_backoff_secs: 1,
        }
    }

    fn info(&self) -> ProcessInfo {
        ProcessInfo {
            name: self.config.name.clone(),
            command: self.config.command.clone(),
            status: self.status,
            exit_code: self.exit_code,
            restart_count: self.restart_count,
            session_id: self.session_id.clone(),
        }
    }

    fn reset_backoff(&mut self) {
        self.current_backoff_secs = 1;
    }

    fn next_backoff(&mut self) -> Duration {
        let delay = Duration::from_secs(self.current_backoff_secs);
        self.current_backoff_secs = (self.current_backoff_secs * 2).min(MAX_BACKOFF_SECS);
        delay
    }

    fn should_reset_backoff(&self) -> bool {
        self.last_start
            .is_some_and(|start| start.elapsed() > Duration::from_secs(BACKOFF_RESET_SECS))
    }
}

/// Real-time process status event.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProcessEvent {
    Snapshot { processes: Vec<ProcessInfo> },
    Update { process: ProcessInfo },
}

/// Manages `[[processes]]` from `arbor.toml`, creating terminal sessions for
/// each and tracking their lifecycle (auto-restart, status, backoff).
pub struct ProcessManager {
    processes: HashMap<String, ManagedProcess>,
    repo_root: PathBuf,
    broadcast: broadcast::Sender<ProcessEvent>,
}

impl ProcessManager {
    pub fn new(repo_root: PathBuf) -> Self {
        let (broadcast, _) = broadcast::channel(64);
        Self {
            processes: HashMap::new(),
            repo_root,
            broadcast,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.broadcast.subscribe()
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Load process configs from parsed `arbor.toml`.
    pub fn load_configs(&mut self, configs: Vec<ProcessConfig>) {
        // Remove processes that are no longer in config
        let new_names: std::collections::HashSet<String> =
            configs.iter().map(|c| c.name.clone()).collect();
        self.processes.retain(|name, _| new_names.contains(name));

        // Add or update processes
        for config in configs {
            let name = config.name.clone();
            self.processes
                .entry(name)
                .and_modify(|p| p.config = config.clone())
                .or_insert_with(|| ManagedProcess::from_config(config));
        }
    }

    /// List all managed processes with their current status.
    pub fn list_processes(&self) -> Vec<ProcessInfo> {
        self.processes.values().map(|p| p.info()).collect()
    }

    /// Start a single process by name. Creates a terminal session for it.
    pub fn start_process<D>(&mut self, name: &str, daemon: &mut D) -> Result<ProcessInfo, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let process = self
            .processes
            .get_mut(name)
            .ok_or_else(|| format!("process `{name}` not found"))?;

        if process.status == ProcessStatus::Running {
            return Ok(process.info());
        }

        let cwd = process
            .config
            .working_dir
            .as_ref()
            .map(|dir| self.repo_root.join(dir))
            .unwrap_or_else(|| self.repo_root.clone());

        let session_id = format!("process-{}", name);

        let result = daemon.create_or_attach(CreateOrAttachRequest {
            session_id: session_id.clone(),
            workspace_id: cwd.display().to_string(),
            cwd,
            shell: default_shell(),
            cols: 120,
            rows: 35,
            title: Some(format!("[process] {}", name)),
            command: Some(process.config.command.clone()),
        });

        match result {
            Ok(response) => {
                process.status = ProcessStatus::Running;
                process.session_id = Some(response.session.session_id.clone());
                process.exit_code = None;
                process.last_start = Some(Instant::now());
                let info = process.info();
                let _ = self.broadcast.send(ProcessEvent::Update {
                    process: info.clone(),
                });
                Ok(info)
            },
            Err(error) => {
                process.status = ProcessStatus::Crashed;
                let _ = self.broadcast.send(ProcessEvent::Update {
                    process: process.info(),
                });
                Err(error.to_string())
            },
        }
    }

    /// Stop a single process by name.
    pub fn stop_process<D>(&mut self, name: &str, daemon: &mut D) -> Result<ProcessInfo, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let process = self
            .processes
            .get_mut(name)
            .ok_or_else(|| format!("process `{name}` not found"))?;

        if let Some(ref session_id) = process.session_id {
            let _ = daemon.kill(KillRequest {
                session_id: session_id.clone(),
            });
        }

        process.status = ProcessStatus::Stopped;
        process.session_id = None;
        process.exit_code = None;
        process.reset_backoff();

        let info = process.info();
        let _ = self.broadcast.send(ProcessEvent::Update {
            process: info.clone(),
        });
        Ok(info)
    }

    /// Restart a single process by name.
    pub fn restart_process<D>(&mut self, name: &str, daemon: &mut D) -> Result<ProcessInfo, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        self.stop_process(name, daemon)?;
        self.start_process(name, daemon)
    }

    /// Start all processes that have `auto_start = true`.
    pub fn start_all<D>(&mut self, daemon: &mut D) -> Vec<(String, Result<ProcessInfo, String>)>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let auto_start_names: Vec<String> = self
            .processes
            .iter()
            .filter(|(_, p)| p.config.auto_start.unwrap_or(false))
            .map(|(name, _)| name.clone())
            .collect();

        let mut results = Vec::new();
        for name in auto_start_names {
            let result = self.start_process(&name, daemon);
            results.push((name, result));
        }
        results
    }

    /// Stop all running processes.
    pub fn stop_all<D>(&mut self, daemon: &mut D) -> Vec<(String, Result<ProcessInfo, String>)>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let running_names: Vec<String> = self
            .processes
            .iter()
            .filter(|(_, p)| p.status == ProcessStatus::Running)
            .map(|(name, _)| name.clone())
            .collect();

        let mut results = Vec::new();
        for name in running_names {
            let result = self.stop_process(&name, daemon);
            results.push((name, result));
        }
        results
    }

    /// Check all running processes for exit, handle auto-restart.
    /// Returns names of processes that need to be restarted (after backoff delay).
    pub fn check_and_update<D>(&mut self, daemon: &mut D) -> Vec<(String, Duration)>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let mut restart_schedule: Vec<(String, Duration)> = Vec::new();

        let names: Vec<String> = self.processes.keys().cloned().collect();
        for name in names {
            let process = match self.processes.get_mut(&name) {
                Some(p) => p,
                None => continue,
            };

            if process.status != ProcessStatus::Running {
                continue;
            }

            let Some(ref session_id) = process.session_id else {
                continue;
            };

            // Check session state from daemon
            let sessions = match daemon.list_sessions() {
                Ok(s) => s,
                Err(_) => continue,
            };

            let session = sessions.iter().find(|s| s.session_id == *session_id);

            let is_exited = match session {
                Some(s) => matches!(
                    s.state,
                    Some(TerminalSessionState::Completed) | Some(TerminalSessionState::Failed)
                ),
                None => true, // Session disappeared
            };

            if !is_exited {
                continue;
            }

            // Process has exited
            let exit_code = session.and_then(|s| s.exit_code);
            process.exit_code = exit_code;
            process.status = ProcessStatus::Crashed;

            let _ = self.broadcast.send(ProcessEvent::Update {
                process: process.info(),
            });

            // Check if auto-restart is enabled
            if process.config.auto_restart.unwrap_or(false) {
                if process.should_reset_backoff() {
                    process.reset_backoff();
                }

                let delay = process.next_backoff();
                process.restart_count += 1;
                process.status = ProcessStatus::Restarting;

                let _ = self.broadcast.send(ProcessEvent::Update {
                    process: process.info(),
                });

                restart_schedule.push((name.clone(), delay));
            }
        }

        restart_schedule
    }

    /// Get a snapshot event for broadcasting to WebSocket clients.
    pub fn snapshot_event(&self) -> ProcessEvent {
        ProcessEvent::Snapshot {
            processes: self.list_processes(),
        }
    }
}

/// Load `[[processes]]` from an `arbor.toml` file.
pub fn load_process_configs(repo_root: &Path) -> Vec<ProcessConfig> {
    repo_config::load_repo_config(repo_root)
        .map(|config| config.processes)
        .unwrap_or_default()
}
