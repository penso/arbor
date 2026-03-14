use {
    arbor_core::{
        daemon::{
            CreateOrAttachRequest, KillRequest, TerminalDaemon, TerminalSessionState, default_shell,
        },
        process::{ProcessInfo, ProcessSource, ProcessStatus},
        procfile, repo_config, worktree,
    },
    serde::Serialize,
    sha2::{Digest, Sha256},
    std::{
        collections::{HashMap, HashSet},
        path::{Path, PathBuf},
        time::{Duration, Instant},
    },
    tokio::sync::broadcast,
};

const MAX_BACKOFF_SECS: u64 = 30;
const BACKOFF_RESET_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) repo_root: PathBuf,
    pub(crate) workspace_path: PathBuf,
    pub(crate) working_dir: PathBuf,
    pub(crate) source: ProcessSource,
    pub(crate) auto_start: bool,
    pub(crate) auto_restart: bool,
}

impl ProcessDefinition {
    fn to_process_info(
        &self,
        status: ProcessStatus,
        exit_code: Option<i32>,
        restart_count: u32,
        session_id: Option<String>,
    ) -> ProcessInfo {
        ProcessInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            command: self.command.clone(),
            repo_root: self.repo_root.display().to_string(),
            workspace_id: self.workspace_path.display().to_string(),
            source: self.source,
            status,
            exit_code,
            restart_count,
            memory_bytes: None,
            session_id,
        }
    }
}

struct ManagedProcess {
    definition: ProcessDefinition,
    status: ProcessStatus,
    session_id: Option<String>,
    exit_code: Option<i32>,
    restart_count: u32,
    last_start: Option<Instant>,
    current_backoff_secs: u64,
}

impl ManagedProcess {
    fn from_definition(definition: ProcessDefinition) -> Self {
        Self {
            definition,
            status: ProcessStatus::Stopped,
            session_id: None,
            exit_code: None,
            restart_count: 0,
            last_start: None,
            current_backoff_secs: 1,
        }
    }

    fn info(&self) -> ProcessInfo {
        self.definition.to_process_info(
            self.status,
            self.exit_code,
            self.restart_count,
            self.session_id.clone(),
        )
    }

    fn update_definition(&mut self, definition: &ProcessDefinition) {
        self.definition = definition.clone();
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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ProcessEvent {
    Snapshot { processes: Vec<ProcessInfo> },
    Update { process: ProcessInfo },
}

pub struct ProcessManager {
    processes: HashMap<String, ManagedProcess>,
    broadcast: broadcast::Sender<ProcessEvent>,
}

impl ProcessManager {
    pub fn new() -> Self {
        let (broadcast, _) = broadcast::channel(64);
        Self {
            processes: HashMap::new(),
            broadcast,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ProcessEvent> {
        self.broadcast.subscribe()
    }

    pub fn sync_definitions(&mut self, definitions: &[ProcessDefinition]) {
        let definition_ids: HashSet<&str> = definitions
            .iter()
            .map(|definition| definition.id.as_str())
            .collect();

        self.processes.retain(|id, process| {
            if definition_ids.contains(id.as_str()) {
                return true;
            }

            !matches!(process.status, ProcessStatus::Stopped)
        });

        for definition in definitions {
            if let Some(process) = self.processes.get_mut(&definition.id) {
                process.update_definition(definition);
            }
        }
    }

    pub fn list_processes(&self, definitions: &[ProcessDefinition]) -> Vec<ProcessInfo> {
        self.list_processes_internal(None, definitions)
    }

    pub fn list_processes_for_workspace(
        &self,
        workspace_path: &Path,
        definitions: &[ProcessDefinition],
    ) -> Vec<ProcessInfo> {
        self.list_processes_internal(Some(workspace_path), definitions)
    }

    pub fn start_process<D>(
        &mut self,
        identifier: &str,
        definitions: &[ProcessDefinition],
        daemon: &mut D,
    ) -> Result<ProcessInfo, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let process_id = self.resolve_process_id(identifier, definitions)?;
        let definition = definitions
            .iter()
            .find(|definition| definition.id == process_id)
            .cloned()
            .ok_or_else(|| format!("process `{identifier}` is not defined"))?;

        self.start_definition(definition, daemon)
    }

    pub fn stop_process<D>(
        &mut self,
        identifier: &str,
        definitions: &[ProcessDefinition],
        daemon: &mut D,
    ) -> Result<ProcessInfo, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let process_id = self.resolve_process_id(identifier, definitions)?;
        if let Some(info) = self.stop_process_by_id(&process_id, daemon)? {
            return Ok(info);
        }

        let definition = definitions
            .iter()
            .find(|definition| definition.id == process_id)
            .ok_or_else(|| format!("process `{identifier}` is not defined"))?;
        Ok(definition.to_process_info(ProcessStatus::Stopped, None, 0, None))
    }

    pub fn restart_process<D>(
        &mut self,
        identifier: &str,
        definitions: &[ProcessDefinition],
        daemon: &mut D,
    ) -> Result<ProcessInfo, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let process_id = self.resolve_process_id(identifier, definitions)?;
        let _ = self.stop_process_by_id(&process_id, daemon)?;
        let definition = definitions
            .iter()
            .find(|definition| definition.id == process_id)
            .cloned()
            .ok_or_else(|| format!("process `{identifier}` is not defined"))?;

        self.start_definition(definition, daemon)
    }

    pub fn restart_tracked_process<D>(
        &mut self,
        process_id: &str,
        daemon: &mut D,
    ) -> Result<ProcessInfo, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let definition = self
            .processes
            .get(process_id)
            .map(|process| process.definition.clone())
            .ok_or_else(|| format!("process `{process_id}` is not tracked"))?;

        self.start_definition(definition, daemon)
    }

    pub fn start_all<D>(
        &mut self,
        definitions: &[ProcessDefinition],
        daemon: &mut D,
    ) -> Vec<(String, Result<ProcessInfo, String>)>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let process_ids: Vec<String> = definitions
            .iter()
            .filter(|definition| {
                definition.auto_start || definition.source == ProcessSource::Procfile
            })
            .map(|definition| definition.id.clone())
            .collect();

        let mut results = Vec::new();
        for process_id in process_ids {
            let result = self.start_process(&process_id, definitions, daemon);
            results.push((process_id, result));
        }
        results
    }

    pub fn stop_all<D>(&mut self, daemon: &mut D) -> Vec<(String, Result<ProcessInfo, String>)>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let process_ids: Vec<String> = self.processes.keys().cloned().collect();

        let mut results = Vec::new();
        for process_id in process_ids {
            let result = self.stop_process_by_id(&process_id, daemon).map(|info| {
                info.unwrap_or_else(|| ProcessInfo {
                    id: process_id.clone(),
                    name: process_id.clone(),
                    command: String::new(),
                    repo_root: String::new(),
                    workspace_id: String::new(),
                    source: ProcessSource::Procfile,
                    status: ProcessStatus::Stopped,
                    exit_code: None,
                    restart_count: 0,
                    memory_bytes: None,
                    session_id: None,
                })
            });
            results.push((process_id, result));
        }
        results
    }

    pub fn check_and_update<D>(&mut self, daemon: &mut D) -> Vec<(String, Duration)>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let sessions = match daemon.list_sessions() {
            Ok(sessions) => sessions,
            Err(_) => return Vec::new(),
        };

        let mut restart_schedule = Vec::new();

        let process_ids: Vec<String> = self.processes.keys().cloned().collect();
        for process_id in process_ids {
            let Some(process) = self.processes.get_mut(&process_id) else {
                continue;
            };

            if process.status != ProcessStatus::Running {
                continue;
            }

            let Some(session_id) = process.session_id.as_ref() else {
                continue;
            };

            let session = sessions
                .iter()
                .find(|session| session.session_id.as_str() == session_id);

            let is_exited = match session {
                Some(session) => matches!(
                    session.state,
                    Some(TerminalSessionState::Completed) | Some(TerminalSessionState::Failed)
                ),
                None => true,
            };

            if !is_exited {
                continue;
            }

            process.exit_code = session.and_then(|session| session.exit_code);
            process.status = ProcessStatus::Crashed;
            process.session_id = None;

            let _ = self.broadcast.send(ProcessEvent::Update {
                process: process.info(),
            });

            if process.definition.auto_restart {
                if process.should_reset_backoff() {
                    process.reset_backoff();
                }

                let delay = process.next_backoff();
                process.restart_count += 1;
                process.status = ProcessStatus::Restarting;

                let _ = self.broadcast.send(ProcessEvent::Update {
                    process: process.info(),
                });

                restart_schedule.push((process_id.clone(), delay));
            }
        }

        restart_schedule
    }

    pub fn snapshot_event(&self, definitions: &[ProcessDefinition]) -> ProcessEvent {
        ProcessEvent::Snapshot {
            processes: self.list_processes(definitions),
        }
    }

    fn list_processes_internal(
        &self,
        workspace_path: Option<&Path>,
        definitions: &[ProcessDefinition],
    ) -> Vec<ProcessInfo> {
        let mut infos = Vec::new();
        let mut seen_ids = HashSet::new();

        for definition in definitions {
            if workspace_path.is_some_and(|path| definition.workspace_path != path) {
                continue;
            }

            seen_ids.insert(definition.id.as_str());
            infos.push(self.info_for_definition(definition));
        }

        for (process_id, process) in &self.processes {
            if seen_ids.contains(process_id.as_str()) {
                continue;
            }
            if workspace_path.is_some_and(|path| process.definition.workspace_path != path) {
                continue;
            }

            infos.push(process.info());
        }

        infos.sort_by(|left, right| {
            left.workspace_id
                .cmp(&right.workspace_id)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.command.cmp(&right.command))
        });
        infos
    }

    fn info_for_definition(&self, definition: &ProcessDefinition) -> ProcessInfo {
        match self.processes.get(&definition.id) {
            Some(process) => process.info(),
            None => definition.to_process_info(ProcessStatus::Stopped, None, 0, None),
        }
    }

    fn resolve_process_id(
        &self,
        identifier: &str,
        definitions: &[ProcessDefinition],
    ) -> Result<String, String> {
        if self.processes.contains_key(identifier)
            || definitions
                .iter()
                .any(|definition| definition.id == identifier)
        {
            return Ok(identifier.to_owned());
        }

        let mut matches = Vec::new();
        let mut seen_ids = HashSet::new();

        for definition in definitions {
            if definition.name == identifier && seen_ids.insert(definition.id.clone()) {
                matches.push(definition.id.clone());
            }
        }

        for process in self.processes.values() {
            if process.definition.name == identifier
                && seen_ids.insert(process.definition.id.clone())
            {
                matches.push(process.definition.id.clone());
            }
        }

        match matches.len() {
            0 => Err(format!("process `{identifier}` not found")),
            1 => Ok(matches.remove(0)),
            _ => Err(format!(
                "process name `{identifier}` is ambiguous, use a process id instead"
            )),
        }
    }

    fn start_definition<D>(
        &mut self,
        definition: ProcessDefinition,
        daemon: &mut D,
    ) -> Result<ProcessInfo, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        if self
            .processes
            .get(&definition.id)
            .is_some_and(|process| process.status == ProcessStatus::Running)
        {
            return self
                .processes
                .get(&definition.id)
                .map(ManagedProcess::info)
                .ok_or_else(|| format!("process `{}` not found", definition.id));
        }

        let session_id = session_id_for_process(&definition);
        let result = daemon.create_or_attach(CreateOrAttachRequest {
            session_id: session_id.clone().into(),
            workspace_id: definition.workspace_path.display().to_string().into(),
            cwd: definition.working_dir.clone(),
            shell: default_shell(),
            cols: 120,
            rows: 35,
            title: Some(format!(
                "[{}] {}",
                process_source_label(definition.source),
                definition.name
            )),
            command: Some(definition.command.clone()),
        });

        match result {
            Ok(response) => {
                let process = self
                    .processes
                    .entry(definition.id.clone())
                    .or_insert_with(|| ManagedProcess::from_definition(definition.clone()));
                process.update_definition(&definition);
                process.reset_backoff();
                process.status = ProcessStatus::Running;
                process.session_id = Some(response.session.session_id.to_string());
                process.exit_code = None;
                process.last_start = Some(Instant::now());

                let info = process.info();
                let _ = self.broadcast.send(ProcessEvent::Update {
                    process: info.clone(),
                });
                Ok(info)
            },
            Err(error) => {
                let process = self
                    .processes
                    .entry(definition.id.clone())
                    .or_insert_with(|| ManagedProcess::from_definition(definition.clone()));
                process.update_definition(&definition);
                process.status = ProcessStatus::Crashed;
                process.session_id = None;
                let _ = self.broadcast.send(ProcessEvent::Update {
                    process: process.info(),
                });
                Err(error.to_string())
            },
        }
    }

    fn stop_process_by_id<D>(
        &mut self,
        process_id: &str,
        daemon: &mut D,
    ) -> Result<Option<ProcessInfo>, String>
    where
        D: TerminalDaemon,
        D::Error: ToString,
    {
        let Some(process) = self.processes.get_mut(process_id) else {
            return Ok(None);
        };

        if let Some(session_id) = process.session_id.clone() {
            daemon
                .kill(KillRequest {
                    session_id: session_id.into(),
                })
                .map_err(|error| error.to_string())?;
        }

        process.status = ProcessStatus::Stopped;
        process.session_id = None;
        process.exit_code = None;
        process.reset_backoff();

        let info = process.info();
        let _ = self.broadcast.send(ProcessEvent::Update {
            process: info.clone(),
        });
        Ok(Some(info))
    }
}

pub fn discover_process_definitions_for_roots(
    repository_roots: &[PathBuf],
) -> Vec<ProcessDefinition> {
    let mut definitions = Vec::new();
    for repo_root in repository_roots {
        definitions.extend(discover_process_definitions_for_repo(repo_root));
    }
    definitions
}

pub fn discover_process_definitions_for_repo(repo_root: &Path) -> Vec<ProcessDefinition> {
    let mut definitions = discover_arbor_toml_process_definitions(repo_root);

    match worktree::list(repo_root) {
        Ok(worktrees) => {
            for worktree in worktrees {
                definitions.extend(discover_procfile_process_definitions(
                    repo_root,
                    &worktree.path,
                ));
            }
        },
        Err(error) => {
            tracing::warn!(repo_root = %repo_root.display(), %error, "failed to enumerate worktrees for process discovery");
        },
    }

    dedupe_process_definitions(definitions)
}

pub fn discover_process_definitions_for_worktree(
    repo_root: &Path,
    worktree_path: &Path,
) -> Vec<ProcessDefinition> {
    let mut definitions = Vec::new();
    if worktree_path == repo_root {
        definitions.extend(discover_arbor_toml_process_definitions(repo_root));
    }
    definitions.extend(discover_procfile_process_definitions(
        repo_root,
        worktree_path,
    ));
    dedupe_process_definitions(definitions)
}

fn discover_arbor_toml_process_definitions(repo_root: &Path) -> Vec<ProcessDefinition> {
    let Some(config) = repo_config::load_repo_config(repo_root) else {
        return Vec::new();
    };

    config
        .processes
        .into_iter()
        .filter(|process| !process.name.trim().is_empty() && !process.command.trim().is_empty())
        .map(|process| {
            let working_dir = process
                .working_dir
                .as_deref()
                .map(|dir| repo_root.join(dir))
                .unwrap_or_else(|| repo_root.to_path_buf());
            build_process_definition(
                ProcessSource::ArborToml,
                repo_root,
                repo_root,
                process.name,
                process.command,
                working_dir,
                process.auto_start.unwrap_or(false),
                process.auto_restart.unwrap_or(false),
            )
        })
        .collect()
}

fn discover_procfile_process_definitions(
    repo_root: &Path,
    worktree_path: &Path,
) -> Vec<ProcessDefinition> {
    let entries = match procfile::read_procfile(worktree_path) {
        Ok(Some(entries)) => entries,
        Ok(None) => return Vec::new(),
        Err(error) => {
            tracing::warn!(worktree = %worktree_path.display(), %error, "failed to read Procfile");
            return Vec::new();
        },
    };

    entries
        .into_iter()
        .map(|entry| {
            build_process_definition(
                ProcessSource::Procfile,
                repo_root,
                worktree_path,
                entry.name,
                entry.command,
                worktree_path.to_path_buf(),
                false,
                false,
            )
        })
        .collect()
}

fn build_process_definition(
    source: ProcessSource,
    repo_root: &Path,
    workspace_path: &Path,
    name: String,
    command: String,
    working_dir: PathBuf,
    auto_start: bool,
    auto_restart: bool,
) -> ProcessDefinition {
    ProcessDefinition {
        id: process_definition_id(source, workspace_path, &name),
        name,
        command,
        repo_root: repo_root.to_path_buf(),
        workspace_path: workspace_path.to_path_buf(),
        working_dir,
        source,
        auto_start,
        auto_restart,
    }
}

fn dedupe_process_definitions(definitions: Vec<ProcessDefinition>) -> Vec<ProcessDefinition> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for definition in definitions {
        if seen.insert(definition.id.clone()) {
            deduped.push(definition);
        }
    }

    deduped
}

fn process_definition_id(source: ProcessSource, workspace_path: &Path, name: &str) -> String {
    format!(
        "{}:{}:{}",
        process_source_label(source),
        workspace_path.display(),
        name
    )
}

fn session_id_for_process(definition: &ProcessDefinition) -> String {
    let digest = Sha256::digest(definition.id.as_bytes());
    let suffix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("process-{suffix}")
}

fn process_source_label(source: ProcessSource) -> &'static str {
    match source {
        ProcessSource::ArborToml => "arbor-toml",
        ProcessSource::Procfile => "procfile",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procfile_definitions_are_worktree_scoped() {
        let repo_root = PathBuf::from("/tmp/repo");
        let first = build_process_definition(
            ProcessSource::Procfile,
            &repo_root,
            Path::new("/tmp/repo"),
            "web".to_owned(),
            "cargo run".to_owned(),
            PathBuf::from("/tmp/repo"),
            false,
            false,
        );
        let second = build_process_definition(
            ProcessSource::Procfile,
            &repo_root,
            Path::new("/tmp/repo-worktrees/feature"),
            "web".to_owned(),
            "cargo run".to_owned(),
            PathBuf::from("/tmp/repo-worktrees/feature"),
            false,
            false,
        );

        assert_ne!(first.id, second.id);
    }

    #[test]
    fn list_processes_keeps_runtime_orphans_for_workspace() {
        let repo_root = PathBuf::from("/tmp/repo");
        let workspace_path = PathBuf::from("/tmp/repo");
        let definition = build_process_definition(
            ProcessSource::Procfile,
            &repo_root,
            &workspace_path,
            "web".to_owned(),
            "cargo run".to_owned(),
            workspace_path.clone(),
            false,
            false,
        );
        let orphan = build_process_definition(
            ProcessSource::Procfile,
            &repo_root,
            &workspace_path,
            "worker".to_owned(),
            "just jobs".to_owned(),
            workspace_path.clone(),
            false,
            false,
        );

        let mut manager = ProcessManager::new();
        manager
            .processes
            .insert(orphan.id.clone(), ManagedProcess::from_definition(orphan));

        let infos = manager.list_processes_for_workspace(&workspace_path, &[definition]);
        assert_eq!(infos.len(), 2);
    }
}
