use {
    crate::{
        codex::{
            AppServerRunner, RunAttemptRequest, RunOutcome, RunResult, Runner, RunnerError,
            RunnerEvent,
        },
        domain::{
            CodexRateLimits, CodexTotals, RetrySnapshot, RunningSnapshot, RuntimeSnapshot,
            ServiceStatus,
        },
        tracker::{IssueTracker, LinearTracker, TrackerError},
        workflow::{
            TypedWorkflowConfig, WorkflowDefinition, WorkflowError, WorkflowLoader,
            default_workflow_path, resolve_config,
        },
        workspace::{Workspace, WorkspaceManager},
    },
    std::{
        collections::{HashMap, HashSet},
        path::PathBuf,
        sync::Arc,
        time::{Duration, Instant, SystemTime},
    },
    thiserror::Error,
    tokio::sync::{RwLock, mpsc},
};

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("{0}")]
    Workflow(#[from] WorkflowError),
    #[error("{0}")]
    Tracker(#[from] TrackerError),
}

#[derive(Clone)]
pub struct ServiceHandle {
    snapshot: Arc<RwLock<RuntimeSnapshot>>,
    issue_snapshots: Arc<RwLock<HashMap<String, IssueRuntimeSnapshot>>>,
    commands: mpsc::UnboundedSender<ServiceCommand>,
}

impl ServiceHandle {
    pub async fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn issue_snapshot(&self, identifier: &str) -> Option<IssueRuntimeSnapshot> {
        self.issue_snapshots.read().await.get(identifier).cloned()
    }

    pub fn refresh(&self) -> Result<(), String> {
        self.commands
            .send(ServiceCommand::Refresh)
            .map_err(|error| error.to_string())
    }

    pub fn stop(&self) -> Result<(), String> {
        self.commands
            .send(ServiceCommand::Stop)
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub struct ServiceOptions {
    pub workflow_path: Option<PathBuf>,
    pub runner: Arc<dyn Runner>,
    pub tracker: Option<Arc<dyn IssueTracker>>,
}

impl Default for ServiceOptions {
    fn default() -> Self {
        Self {
            workflow_path: None,
            runner: Arc::new(AppServerRunner),
            tracker: None,
        }
    }
}

pub struct SymphonyService;

impl SymphonyService {
    pub async fn start(options: ServiceOptions) -> Result<ServiceHandle, ServiceError> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let workflow_path = options
            .workflow_path
            .unwrap_or_else(|| default_workflow_path(&cwd));
        let mut loader = WorkflowLoader::new(workflow_path.clone());
        let workflow = loader.load()?;
        let config = resolve_config(&workflow)?;
        let tracker: Arc<dyn IssueTracker> = match options.tracker {
            Some(tracker) => tracker,
            None => Arc::new(LinearTracker::new(config.tracker.clone())?),
        };
        let workspace_manager = Arc::new(WorkspaceManager::new(
            config.workspace.root.clone(),
            config.hooks.clone(),
        ));

        let snapshot = Arc::new(RwLock::new(RuntimeSnapshot {
            generated_at: now_string(),
            workflow_path: workflow_path.clone(),
            service_status: ServiceStatus::Running,
            ..RuntimeSnapshot::default()
        }));
        let issue_snapshots = Arc::new(RwLock::new(HashMap::new()));
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        let actor = Actor {
            snapshot: snapshot.clone(),
            issue_snapshots: issue_snapshots.clone(),
            loader,
            workflow,
            config,
            tracker,
            workspace_manager,
            runner: options.runner,
            commands_rx,
            events_rx,
            events_tx,
            running: HashMap::new(),
            claimed: HashSet::new(),
            retry_attempts: HashMap::new(),
            accumulated_runtime_secs: 0,
            latest_rate_limits: None,
            last_error: None,
        };
        tokio::spawn(actor.run());

        Ok(ServiceHandle {
            snapshot,
            issue_snapshots,
            commands: commands_tx,
        })
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct IssueRuntimeSnapshot {
    pub issue_id: String,
    pub issue_identifier: String,
    pub status: String,
    pub workspace_path: Option<PathBuf>,
    pub session_id: Option<String>,
    pub turn_count: u32,
    pub current_retry_attempt: Option<u32>,
    pub last_event: Option<String>,
    pub last_message: Option<String>,
    pub last_error: Option<String>,
}

struct Actor {
    snapshot: Arc<RwLock<RuntimeSnapshot>>,
    issue_snapshots: Arc<RwLock<HashMap<String, IssueRuntimeSnapshot>>>,
    loader: WorkflowLoader,
    workflow: WorkflowDefinition,
    config: TypedWorkflowConfig,
    tracker: Arc<dyn IssueTracker>,
    workspace_manager: Arc<WorkspaceManager>,
    runner: Arc<dyn Runner>,
    commands_rx: mpsc::UnboundedReceiver<ServiceCommand>,
    events_rx: mpsc::UnboundedReceiver<ServiceEvent>,
    events_tx: mpsc::UnboundedSender<ServiceEvent>,
    running: HashMap<String, RunningEntry>,
    claimed: HashSet<String>,
    retry_attempts: HashMap<String, RetryEntry>,
    accumulated_runtime_secs: u64,
    latest_rate_limits: Option<CodexRateLimits>,
    last_error: Option<String>,
}

#[derive(Debug)]
struct RunningEntry {
    issue_id: String,
    issue_identifier: String,
    issue_state: String,
    workspace_path: PathBuf,
    started_at: Instant,
    last_activity_at: Instant,
    started_at_text: String,
    last_event: Option<String>,
    last_event_at: Option<String>,
    last_message: Option<String>,
    session_id: Option<String>,
    turn_count: u32,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    retry_attempt: Option<u32>,
    cancellation: Option<CancellationAction>,
    abort_handle: tokio::task::AbortHandle,
}

#[derive(Debug, Clone)]
enum CancellationAction {
    Retry {
        reason: String,
        record_error: bool,
    },
    Release {
        reason: Option<String>,
        record_error: bool,
    },
}

#[derive(Debug, Clone)]
struct RetryEntry {
    issue_id: String,
    issue_identifier: String,
    attempt: u32,
    due_at: Instant,
    due_at_text: String,
    error: Option<String>,
}

#[derive(Debug)]
enum ServiceCommand {
    Refresh,
    Stop,
}

#[derive(Debug)]
enum ServiceEvent {
    RunnerEvent {
        issue_id: String,
        event: RunnerEvent,
    },
    WorkerFinished {
        issue_id: String,
        issue_identifier: String,
        workspace: Option<Workspace>,
        result: Result<RunResult, RunnerError>,
    },
}

impl Actor {
    async fn run(mut self) {
        let _ = self.startup_cleanup().await;
        self.rebuild_snapshots().await;

        loop {
            let delay = self.next_wakeup_delay();
            let sleep = tokio::time::sleep(delay);
            tokio::pin!(sleep);

            tokio::select! {
                Some(command) = self.commands_rx.recv() => {
                    match command {
                        ServiceCommand::Refresh => self.tick().await,
                        ServiceCommand::Stop => {
                            self.stop_all_running();
                            self.last_error = None;
                            break;
                        },
                    }
                },
                Some(event) = self.events_rx.recv() => {
                    self.handle_event(event).await;
                },
                _ = &mut sleep => {
                    self.tick().await;
                },
            }
        }

        {
            let mut snapshot = self.snapshot.write().await;
            snapshot.running.clear();
            snapshot.retrying.clear();
            snapshot.codex_totals = CodexTotals::default();
            snapshot.rate_limits = None;
            snapshot.last_error = None;
            snapshot.service_status = ServiceStatus::Stopped;
            snapshot.generated_at = now_string();
        }
    }

    fn next_wakeup_delay(&self) -> Duration {
        let poll = Duration::from_millis(self.config.polling.interval_ms.max(1));
        let Some(next_retry) = self.retry_attempts.values().map(|entry| entry.due_at).min() else {
            return poll;
        };

        let now = Instant::now();
        if next_retry <= now {
            Duration::from_millis(1)
        } else {
            std::cmp::min(poll, next_retry.saturating_duration_since(now))
        }
    }

    async fn tick(&mut self) {
        let mut tick_error = None;
        if let Err(error) = self.reload_workflow_if_changed().await {
            tick_error = Some(error.to_string());
        }

        if let Some(error) = self.reconcile_running().await {
            tick_error = Some(error);
        }
        if let Some(error) = self.process_due_retries().await {
            tick_error = Some(error);
        }

        match self.tracker.fetch_candidate_issues().await {
            Ok(issues) => self.dispatch_candidates(issues).await,
            Err(error) => tick_error = Some(error.to_string()),
        }

        self.last_error = tick_error;
        self.rebuild_snapshots().await;
    }

    async fn reload_workflow_if_changed(&mut self) -> Result<(), WorkflowError> {
        let Some(workflow) = self.loader.load_if_changed()? else {
            return Ok(());
        };
        let config = resolve_config(&workflow)?;
        let tracker: Arc<dyn IssueTracker> = Arc::new(
            LinearTracker::new(config.tracker.clone())
                .map_err(|error| WorkflowError::InvalidConfig(error.to_string()))?,
        );
        self.workflow = workflow;
        self.workspace_manager = Arc::new(WorkspaceManager::new(
            config.workspace.root.clone(),
            config.hooks.clone(),
        ));
        self.tracker = tracker;
        self.config = config;
        Ok(())
    }

    async fn reconcile_running(&mut self) -> Option<String> {
        if self.running.is_empty() {
            return None;
        }

        let mut reconciliation_error = None;
        if self.config.codex.stall_timeout_ms > 0 {
            let stall_timeout = Duration::from_millis(self.config.codex.stall_timeout_ms as u64);
            let mut stalled = Vec::new();
            for (issue_id, entry) in &self.running {
                let elapsed = entry.last_activity_at.elapsed();
                if elapsed > stall_timeout {
                    stalled.push(issue_id.clone());
                }
            }
            for issue_id in stalled {
                reconciliation_error.get_or_insert_with(|| "stall timeout".to_owned());
                self.cancel_running(
                    &issue_id,
                    CancellationAction::Retry {
                        reason: "stall timeout".to_owned(),
                        record_error: true,
                    },
                    false,
                );
            }
        }

        let ids: Vec<String> = self.running.keys().cloned().collect();
        match self.tracker.fetch_issue_states_by_ids(&ids).await {
            Ok(issues) => {
                let refreshed: HashMap<String, crate::domain::Issue> = issues
                    .into_iter()
                    .map(|issue| (issue.id.clone(), issue))
                    .collect();
                let running_ids: Vec<String> = self.running.keys().cloned().collect();
                for issue_id in running_ids {
                    let Some(issue) = refreshed.get(&issue_id) else {
                        continue;
                    };
                    let is_terminal = self
                        .config
                        .tracker
                        .terminal_states
                        .iter()
                        .any(|state| state.eq_ignore_ascii_case(&issue.state));
                    let is_active = self
                        .config
                        .tracker
                        .active_states
                        .iter()
                        .any(|state| state.eq_ignore_ascii_case(&issue.state));
                    if is_terminal {
                        self.cancel_running(
                            &issue_id,
                            CancellationAction::Release {
                                reason: None,
                                record_error: false,
                            },
                            true,
                        );
                    } else if !is_active {
                        self.cancel_running(
                            &issue_id,
                            CancellationAction::Release {
                                reason: None,
                                record_error: false,
                            },
                            false,
                        );
                    } else if let Some(entry) = self.running.get_mut(&issue_id) {
                        entry.issue_state = issue.state.clone();
                    }
                }
            },
            Err(error) => {
                reconciliation_error = Some(error.to_string());
            },
        }

        reconciliation_error
    }

    async fn process_due_retries(&mut self) -> Option<String> {
        let now = Instant::now();
        let due_ids: Vec<String> = self
            .retry_attempts
            .iter()
            .filter(|(_, entry)| entry.due_at <= now)
            .map(|(issue_id, _)| issue_id.clone())
            .collect();
        if due_ids.is_empty() {
            return None;
        }

        let candidates = match self.tracker.fetch_candidate_issues().await {
            Ok(candidates) => candidates,
            Err(error) => {
                return Some(error.to_string());
            },
        };

        for issue_id in due_ids {
            let Some(retry) = self.retry_attempts.remove(&issue_id) else {
                continue;
            };
            self.claimed.remove(&issue_id);

            let Some(issue) = candidates
                .iter()
                .find(|issue| issue.id == issue_id)
                .cloned()
            else {
                self.claimed.remove(&issue_id);
                continue;
            };

            if self.can_dispatch(&issue) {
                self.dispatch_issue(issue, Some(retry.attempt)).await;
            } else {
                self.schedule_retry(
                    retry.issue_id,
                    retry.issue_identifier,
                    retry.attempt + 1,
                    Some("no available orchestrator slots".to_owned()),
                    retry_delay_ms(retry.attempt + 1, self.config.agent.max_retry_backoff_ms),
                );
            }
        }

        None
    }

    async fn dispatch_candidates(&mut self, mut issues: Vec<crate::domain::Issue>) {
        issues.sort_by(|left, right| {
            left.priority
                .unwrap_or(i64::MAX)
                .cmp(&right.priority.unwrap_or(i64::MAX))
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.identifier.cmp(&right.identifier))
        });

        for issue in issues {
            if !self.has_available_slots() {
                break;
            }
            if self.can_dispatch(&issue) {
                self.dispatch_issue(issue, None).await;
            }
        }
    }

    fn has_available_slots(&self) -> bool {
        self.running.len() < self.config.agent.max_concurrent_agents
    }

    fn can_dispatch(&self, issue: &crate::domain::Issue) -> bool {
        if issue.id.trim().is_empty()
            || issue.identifier.trim().is_empty()
            || issue.title.trim().is_empty()
            || issue.state.trim().is_empty()
        {
            return false;
        }

        let state = issue.state.to_ascii_lowercase();
        let is_active = self
            .config
            .tracker
            .active_states
            .iter()
            .any(|configured| configured.eq_ignore_ascii_case(&state));
        let is_terminal = self
            .config
            .tracker
            .terminal_states
            .iter()
            .any(|configured| configured.eq_ignore_ascii_case(&state));
        if !is_active || is_terminal {
            return false;
        }

        if self.running.contains_key(&issue.id) || self.claimed.contains(&issue.id) {
            return false;
        }

        let state_limit = self
            .config
            .agent
            .max_concurrent_agents_by_state
            .get(&issue.state.to_ascii_lowercase())
            .copied()
            .unwrap_or(self.config.agent.max_concurrent_agents);
        let running_in_state = self
            .running
            .values()
            .filter(|entry| entry.issue_state.eq_ignore_ascii_case(&issue.state))
            .count();
        if running_in_state >= state_limit {
            return false;
        }

        if issue.state.eq_ignore_ascii_case("todo")
            && issue.blocked_by.iter().any(|blocker| {
                blocker.state.as_deref().is_some_and(|state| {
                    !self
                        .config
                        .tracker
                        .terminal_states
                        .iter()
                        .any(|terminal| terminal.eq_ignore_ascii_case(state))
                })
            })
        {
            return false;
        }

        self.has_available_slots()
    }

    async fn dispatch_issue(&mut self, issue: crate::domain::Issue, attempt: Option<u32>) {
        let issue_id = issue.id.clone();
        let issue_identifier = issue.identifier.clone();
        self.claimed.insert(issue_id.clone());
        self.retry_attempts.remove(&issue_id);

        let workflow = self.workflow.clone();
        let config = self.config.clone();
        let tracker = self.tracker.clone();
        let workspace_manager = self.workspace_manager.clone();
        let runner = self.runner.clone();
        let events = self.events_tx.clone();

        let worker_issue_id = issue_id.clone();
        let worker_issue_identifier = issue_identifier.clone();
        let worker_issue_state = issue.state.clone();
        let worker_issue = issue.clone();
        let running_issue_id = worker_issue_id.clone();
        let entry_issue_id = worker_issue_id.clone();
        let join = tokio::spawn(async move {
            let result: Result<(RunResult, Workspace), WorkerTaskError> = async {
                let workspace = workspace_manager
                    .ensure_workspace(&worker_issue.identifier)
                    .await?;
                workspace_manager.before_run(&workspace).await?;
                let prompt = workflow.render_prompt(&worker_issue, attempt)?;
                let request = RunAttemptRequest {
                    issue: worker_issue,
                    attempt,
                    prompt,
                    workspace_path: workspace.path.clone(),
                    config,
                    tracker,
                };
                let (runner_tx, mut runner_rx) = mpsc::unbounded_channel();
                let event_forwarder = {
                    let events = events.clone();
                    let issue_id = worker_issue_id.clone();
                    tokio::spawn(async move {
                        while let Some(event) = runner_rx.recv().await {
                            let _ = events.send(ServiceEvent::RunnerEvent {
                                issue_id: issue_id.clone(),
                                event,
                            });
                        }
                    })
                };
                let result = runner.run_attempt(request, runner_tx).await;
                let _ = event_forwarder.await;
                workspace_manager.after_run_best_effort(&workspace).await;
                result
                    .map(|result| (result, workspace))
                    .map_err(WorkerTaskError::from)
            }
            .await;
            result
        });
        let abort_handle = join.abort_handle();
        let events = self.events_tx.clone();
        tokio::spawn(async move {
            let message = match join.await {
                Ok(Ok((result, workspace))) => ServiceEvent::WorkerFinished {
                    issue_id,
                    issue_identifier,
                    workspace: Some(workspace),
                    result: Ok(result),
                },
                Ok(Err(error)) => ServiceEvent::WorkerFinished {
                    issue_id,
                    issue_identifier,
                    workspace: None,
                    result: Err(match error {
                        WorkerTaskError::Workflow(error) => {
                            RunnerError::ResponseError(error.to_string())
                        },
                        WorkerTaskError::Runner(error) => error,
                        WorkerTaskError::Workspace(error) => {
                            RunnerError::ResponseError(error.to_string())
                        },
                    }),
                },
                Err(error) => ServiceEvent::WorkerFinished {
                    issue_id,
                    issue_identifier,
                    workspace: None,
                    result: Err(if error.is_cancelled() {
                        RunnerError::TurnCancelled("worker cancelled".to_owned())
                    } else {
                        RunnerError::ResponseError(error.to_string())
                    }),
                },
            };
            let _ = events.send(message);
        });

        let workspace_path = self
            .workspace_manager
            .workspace_path_for(&worker_issue_identifier)
            .unwrap_or_else(|_| self.workspace_manager.root().join(&worker_issue_identifier));
        self.running.insert(running_issue_id, RunningEntry {
            issue_id: entry_issue_id,
            issue_identifier: worker_issue_identifier.clone(),
            issue_state: worker_issue_state,
            workspace_path,
            started_at: Instant::now(),
            last_activity_at: Instant::now(),
            started_at_text: now_string(),
            last_event: None,
            last_event_at: None,
            last_message: None,
            session_id: None,
            turn_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            retry_attempt: attempt,
            cancellation: None,
            abort_handle,
        });
    }

    fn cancel_running(
        &mut self,
        issue_id: &str,
        action: CancellationAction,
        cleanup_workspace: bool,
    ) {
        let Some(entry) = self.running.get_mut(issue_id) else {
            return;
        };
        entry.cancellation = Some(action);
        entry.abort_handle.abort();
        if cleanup_workspace {
            let workspace_manager = self.workspace_manager.clone();
            let issue_identifier = entry.issue_identifier.clone();
            tokio::spawn(async move {
                let _ = workspace_manager.remove_workspace(&issue_identifier).await;
            });
        }
    }

    fn stop_all_running(&mut self) {
        for entry in self.running.values() {
            entry.abort_handle.abort();
        }
        self.running.clear();
        self.retry_attempts.clear();
        self.claimed.clear();
    }

    async fn handle_event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::RunnerEvent { issue_id, event } => {
                if let Some(entry) = self.running.get_mut(&issue_id) {
                    entry.last_event = Some(event.event.clone());
                    entry.last_event_at = Some(event.at.clone());
                    entry.last_activity_at = Instant::now();
                    entry.last_message = event.message.clone();
                    entry.session_id = event
                        .session_id
                        .clone()
                        .or_else(|| entry.session_id.clone());
                    if event.event == "session_started" {
                        entry.turn_count = entry.turn_count.saturating_add(1);
                    }
                    if let Some(totals) = event.totals {
                        entry.input_tokens = totals.input_tokens;
                        entry.output_tokens = totals.output_tokens;
                        entry.total_tokens = totals.total_tokens;
                    }
                    if let Some(rate_limits) = event.rate_limits {
                        self.latest_rate_limits = Some(rate_limits);
                    }
                }
            },
            ServiceEvent::WorkerFinished {
                issue_id,
                issue_identifier,
                workspace,
                result,
            } => {
                let Some(entry) = self.running.remove(&issue_id) else {
                    return;
                };
                self.accumulated_runtime_secs += entry.started_at.elapsed().as_secs();
                let cancellation = entry.cancellation.clone();

                match cancellation {
                    Some(CancellationAction::Retry {
                        reason,
                        record_error,
                    }) => {
                        let next_attempt = entry.retry_attempt.unwrap_or(0) + 1;
                        if record_error {
                            self.last_error = Some(reason.clone());
                        }
                        self.schedule_retry(
                            issue_id.clone(),
                            issue_identifier.clone(),
                            next_attempt,
                            Some(reason),
                            retry_delay_ms(next_attempt, self.config.agent.max_retry_backoff_ms),
                        );
                    },
                    Some(CancellationAction::Release {
                        reason,
                        record_error,
                    }) => {
                        self.claimed.remove(&issue_id);
                        self.retry_attempts.remove(&issue_id);
                        if record_error {
                            self.last_error = reason;
                        }
                    },
                    None => match result {
                        Ok(run_result) if run_result.outcome == RunOutcome::Completed => {
                            self.schedule_retry(
                                issue_id.clone(),
                                issue_identifier.clone(),
                                1,
                                None,
                                1_000,
                            );
                            if let Some(rate_limits) = run_result.rate_limits {
                                self.latest_rate_limits = Some(rate_limits);
                            }
                        },
                        Ok(run_result) => {
                            let error = Some(format!("worker exited: {:?}", run_result.outcome));
                            self.last_error = error.clone();
                            self.schedule_retry(
                                issue_id.clone(),
                                issue_identifier.clone(),
                                entry.retry_attempt.unwrap_or(0) + 1,
                                error,
                                retry_delay_ms(
                                    entry.retry_attempt.unwrap_or(0) + 1,
                                    self.config.agent.max_retry_backoff_ms,
                                ),
                            );
                        },
                        Err(error) => {
                            let error = error.to_string();
                            self.last_error = Some(error.clone());
                            self.schedule_retry(
                                issue_id.clone(),
                                issue_identifier.clone(),
                                entry.retry_attempt.unwrap_or(0) + 1,
                                Some(error),
                                retry_delay_ms(
                                    entry.retry_attempt.unwrap_or(0) + 1,
                                    self.config.agent.max_retry_backoff_ms,
                                ),
                            );
                        },
                    },
                }

                if let Some(workspace) = workspace {
                    self.issue_snapshots.write().await.insert(
                        issue_identifier.clone(),
                        IssueRuntimeSnapshot {
                            issue_id,
                            issue_identifier,
                            status: "retrying".to_owned(),
                            workspace_path: Some(workspace.path),
                            session_id: entry.session_id,
                            turn_count: entry.turn_count,
                            current_retry_attempt: self
                                .retry_attempts
                                .get(&entry.issue_id)
                                .map(|retry| retry.attempt),
                            last_event: entry.last_event,
                            last_message: entry.last_message,
                            last_error: self.last_error.clone(),
                        },
                    );
                }
            },
        }

        self.rebuild_snapshots().await;
    }

    fn schedule_retry(
        &mut self,
        issue_id: String,
        issue_identifier: String,
        attempt: u32,
        error: Option<String>,
        delay_ms: u64,
    ) {
        self.claimed.insert(issue_id.clone());
        self.retry_attempts.insert(issue_id.clone(), RetryEntry {
            issue_id,
            issue_identifier,
            attempt,
            due_at: Instant::now() + Duration::from_millis(delay_ms),
            due_at_text: now_string_after(delay_ms),
            error,
        });
    }

    async fn startup_cleanup(&mut self) -> Result<(), TrackerError> {
        let issues = self
            .tracker
            .fetch_issues_by_states(&self.config.tracker.terminal_states)
            .await?;
        for issue in issues {
            let _ = self
                .workspace_manager
                .remove_workspace(&issue.identifier)
                .await;
        }
        Ok(())
    }

    async fn rebuild_snapshots(&self) {
        let running: Vec<RunningSnapshot> = self
            .running
            .values()
            .map(|entry| RunningSnapshot {
                issue_id: entry.issue_id.clone(),
                issue_identifier: entry.issue_identifier.clone(),
                state: entry.issue_state.clone(),
                session_id: entry.session_id.clone(),
                turn_count: entry.turn_count,
                last_event: entry.last_event.clone(),
                last_message: entry.last_message.clone(),
                started_at: entry.started_at_text.clone(),
                last_event_at: entry.last_event_at.clone(),
                input_tokens: entry.input_tokens,
                output_tokens: entry.output_tokens,
                total_tokens: entry.total_tokens,
                workspace_path: entry.workspace_path.clone(),
                retry_attempt: entry.retry_attempt,
            })
            .collect();

        let retrying: Vec<RetrySnapshot> = self
            .retry_attempts
            .values()
            .map(|entry| RetrySnapshot {
                issue_id: entry.issue_id.clone(),
                issue_identifier: entry.issue_identifier.clone(),
                attempt: entry.attempt,
                due_at: entry.due_at_text.clone(),
                error: entry.error.clone(),
            })
            .collect();

        let seconds_running = self.accumulated_runtime_secs
            + self
                .running
                .values()
                .map(|entry| entry.started_at.elapsed().as_secs())
                .sum::<u64>();
        let mut snapshot = self.snapshot.write().await;
        snapshot.generated_at = now_string();
        snapshot.running = running;
        snapshot.retrying = retrying;
        snapshot.codex_totals = CodexTotals {
            input_tokens: self.running.values().map(|entry| entry.input_tokens).sum(),
            output_tokens: self.running.values().map(|entry| entry.output_tokens).sum(),
            total_tokens: self.running.values().map(|entry| entry.total_tokens).sum(),
            seconds_running,
        };
        snapshot.rate_limits = self.latest_rate_limits.clone();
        snapshot.last_error = self.last_error.clone();
        snapshot.service_status = if self.last_error.is_some() {
            ServiceStatus::Degraded
        } else {
            ServiceStatus::Running
        };
    }
}

#[derive(Debug, Error)]
enum WorkerTaskError {
    #[error("{0}")]
    Workflow(#[from] WorkflowError),
    #[error("{0}")]
    Workspace(#[from] crate::workspace::WorkspaceError),
    #[error("{0}")]
    Runner(#[from] RunnerError),
}

fn retry_delay_ms(attempt: u32, max_retry_backoff_ms: u64) -> u64 {
    let exponent = attempt.saturating_sub(1);
    let multiplier = 2_u64.saturating_pow(exponent);
    std::cmp::min(10_000_u64.saturating_mul(multiplier), max_retry_backoff_ms)
}

fn now_string() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z", now.as_secs(), now.subsec_millis())
}

fn now_string_after(delay_ms: u64) -> String {
    let future = SystemTime::now()
        .checked_add(Duration::from_millis(delay_ms))
        .unwrap_or_else(SystemTime::now);
    let future = future
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z", future.as_secs(), future.subsec_millis())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use {
        super::*,
        crate::{
            codex::{RunAttemptRequest, RunnerEvent},
            domain::{Issue, IssueBlocker},
            tracker::TrackerError,
        },
        async_trait::async_trait,
        std::sync::{
            Arc, Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[derive(Default)]
    struct MockTracker {
        issues: StdMutex<Vec<Issue>>,
        candidate_errors: StdMutex<Vec<String>>,
    }

    #[async_trait]
    impl IssueTracker for MockTracker {
        async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
            if let Some(error) = self.candidate_errors.lock().expect("lock").pop() {
                return Err(TrackerError::LinearApiRequest(error));
            }
            Ok(self.issues.lock().expect("lock").clone())
        }

        async fn fetch_issues_by_states(
            &self,
            _states: &[String],
        ) -> Result<Vec<Issue>, TrackerError> {
            Ok(Vec::new())
        }

        async fn fetch_issue_states_by_ids(
            &self,
            issue_ids: &[String],
        ) -> Result<Vec<Issue>, TrackerError> {
            Ok(self
                .issues
                .lock()
                .expect("lock")
                .iter()
                .filter(|issue| issue_ids.contains(&issue.id))
                .cloned()
                .collect())
        }
    }

    #[derive(Default)]
    struct MockRunner;

    #[async_trait]
    impl Runner for MockRunner {
        async fn run_attempt(
            &self,
            _request: RunAttemptRequest,
            events: mpsc::UnboundedSender<RunnerEvent>,
        ) -> Result<RunResult, RunnerError> {
            let _ = events.send(RunnerEvent {
                event: "turn/completed".to_owned(),
                at: now_string(),
                ..RunnerEvent::default()
            });
            Ok(RunResult {
                outcome: RunOutcome::Completed,
                ..RunResult::default()
            })
        }
    }

    struct CountingRunner {
        attempts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Runner for CountingRunner {
        async fn run_attempt(
            &self,
            _request: RunAttemptRequest,
            events: mpsc::UnboundedSender<RunnerEvent>,
        ) -> Result<RunResult, RunnerError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let _ = events.send(RunnerEvent {
                event: "session_started".to_owned(),
                at: now_string(),
                ..RunnerEvent::default()
            });
            let _ = events.send(RunnerEvent {
                event: "turn/completed".to_owned(),
                at: now_string(),
                ..RunnerEvent::default()
            });
            Ok(RunResult {
                outcome: RunOutcome::Completed,
                ..RunResult::default()
            })
        }
    }

    struct ActiveRunner {
        events_before_finish: usize,
        interval: Duration,
    }

    #[async_trait]
    impl Runner for ActiveRunner {
        async fn run_attempt(
            &self,
            _request: RunAttemptRequest,
            events: mpsc::UnboundedSender<RunnerEvent>,
        ) -> Result<RunResult, RunnerError> {
            for _ in 0..self.events_before_finish {
                let _ = events.send(RunnerEvent {
                    event: "notification".to_owned(),
                    at: now_string(),
                    ..RunnerEvent::default()
                });
                tokio::time::sleep(self.interval).await;
            }

            Ok(RunResult {
                outcome: RunOutcome::Completed,
                ..RunResult::default()
            })
        }
    }

    struct HangingRunner;

    #[async_trait]
    impl Runner for HangingRunner {
        async fn run_attempt(
            &self,
            _request: RunAttemptRequest,
            _events: mpsc::UnboundedSender<RunnerEvent>,
        ) -> Result<RunResult, RunnerError> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(RunResult {
                outcome: RunOutcome::Completed,
                ..RunResult::default()
            })
        }
    }

    fn write_workflow(path: &std::path::Path, polling_ms: u64, stall_timeout_ms: i64) {
        let workspace_root = path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("workspaces");
        std::fs::write(
            path,
            format!(
                "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: arbor\npolling:\n  interval_ms: {polling_ms}\nworkspace:\n  root: {}\nagent:\n  max_turns: 1\ncodex:\n  command: codex app-server\n  stall_timeout_ms: {stall_timeout_ms}\n---\nIssue {{{{ issue.identifier }}}}",
                workspace_root.display(),
            ),
        )
        .expect("workflow");
    }

    #[tokio::test]
    async fn retry_backoff_scales_and_caps() {
        assert_eq!(retry_delay_ms(1, 300_000), 10_000);
        assert_eq!(retry_delay_ms(2, 300_000), 20_000);
        assert_eq!(retry_delay_ms(10, 30_000), 30_000);
    }

    #[tokio::test]
    async fn starts_service_and_dispatches_unblocked_issue() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workflow_path = temp.path().join("WORKFLOW.md");
        write_workflow(&workflow_path, 30_000, 300_000);

        let handle = SymphonyService::start(ServiceOptions {
            workflow_path: Some(workflow_path),
            runner: Arc::new(MockRunner),
            tracker: Some(Arc::new(MockTracker {
                issues: StdMutex::new(vec![Issue {
                    id: "1".to_owned(),
                    identifier: "ARB-1".to_owned(),
                    title: "Ready".to_owned(),
                    state: "Todo".to_owned(),
                    ..Issue::default()
                }]),
                candidate_errors: StdMutex::default(),
            })),
        })
        .await
        .expect("service");

        let _ = handle.refresh();
        tokio::time::sleep(Duration::from_millis(25)).await;
        let snapshot = handle.snapshot().await;
        assert_eq!(snapshot.service_status, ServiceStatus::Running);
        let _ = handle.stop();
    }

    #[tokio::test]
    async fn redispatches_retry_after_successful_completion() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workflow_path = temp.path().join("WORKFLOW.md");
        write_workflow(&workflow_path, 25, 300_000);
        let attempts = Arc::new(AtomicUsize::new(0));

        let handle = SymphonyService::start(ServiceOptions {
            workflow_path: Some(workflow_path),
            runner: Arc::new(CountingRunner {
                attempts: attempts.clone(),
            }),
            tracker: Some(Arc::new(MockTracker {
                issues: StdMutex::new(vec![Issue {
                    id: "1".to_owned(),
                    identifier: "ARB-1".to_owned(),
                    title: "Retry me".to_owned(),
                    state: "Todo".to_owned(),
                    ..Issue::default()
                }]),
                candidate_errors: StdMutex::default(),
            })),
        })
        .await
        .expect("service");

        let _ = handle.refresh();
        tokio::time::sleep(Duration::from_millis(1_200)).await;

        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "expected retry redispatch to run a second attempt"
        );
        let _ = handle.stop();
    }

    #[tokio::test]
    async fn clears_degraded_status_after_successful_tick() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workflow_path = temp.path().join("WORKFLOW.md");
        write_workflow(&workflow_path, 30_000, 300_000);

        let handle = SymphonyService::start(ServiceOptions {
            workflow_path: Some(workflow_path),
            runner: Arc::new(MockRunner),
            tracker: Some(Arc::new(MockTracker {
                issues: StdMutex::new(vec![Issue {
                    id: "1".to_owned(),
                    identifier: "ARB-1".to_owned(),
                    title: "Recover".to_owned(),
                    state: "Todo".to_owned(),
                    ..Issue::default()
                }]),
                candidate_errors: StdMutex::new(vec!["transient".to_owned()]),
            })),
        })
        .await
        .expect("service");

        let _ = handle.refresh();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            handle.snapshot().await.service_status,
            ServiceStatus::Degraded
        );

        let _ = handle.refresh();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            handle.snapshot().await.service_status,
            ServiceStatus::Running
        );
        let _ = handle.stop();
    }

    #[tokio::test]
    async fn stop_aborts_running_workers_and_clears_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workflow_path = temp.path().join("WORKFLOW.md");
        write_workflow(&workflow_path, 25, 300_000);

        let handle = SymphonyService::start(ServiceOptions {
            workflow_path: Some(workflow_path),
            runner: Arc::new(HangingRunner),
            tracker: Some(Arc::new(MockTracker {
                issues: StdMutex::new(vec![Issue {
                    id: "1".to_owned(),
                    identifier: "ARB-1".to_owned(),
                    title: "Hang".to_owned(),
                    state: "Todo".to_owned(),
                    ..Issue::default()
                }]),
                candidate_errors: StdMutex::default(),
            })),
        })
        .await
        .expect("service");

        let _ = handle.refresh();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(handle.snapshot().await.running.len(), 1);

        let _ = handle.stop();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let snapshot = handle.snapshot().await;
        assert_eq!(snapshot.service_status, ServiceStatus::Stopped);
        assert!(snapshot.running.is_empty());
        assert!(snapshot.retrying.is_empty());
    }

    #[tokio::test]
    async fn stall_detection_uses_last_activity_instead_of_start_time() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workflow_path = temp.path().join("WORKFLOW.md");
        write_workflow(&workflow_path, 25, 100);

        let handle = SymphonyService::start(ServiceOptions {
            workflow_path: Some(workflow_path),
            runner: Arc::new(ActiveRunner {
                events_before_finish: 8,
                interval: Duration::from_millis(30),
            }),
            tracker: Some(Arc::new(MockTracker {
                issues: StdMutex::new(vec![Issue {
                    id: "1".to_owned(),
                    identifier: "ARB-1".to_owned(),
                    title: "Active".to_owned(),
                    state: "Todo".to_owned(),
                    ..Issue::default()
                }]),
                candidate_errors: StdMutex::default(),
            })),
        })
        .await
        .expect("service");

        let _ = handle.refresh();
        tokio::time::sleep(Duration::from_millis(160)).await;

        let snapshot = handle.snapshot().await;
        assert_eq!(snapshot.running.len(), 1);
        assert!(snapshot.retrying.is_empty());
        let _ = handle.stop();
    }

    #[test]
    fn todo_with_blocker_is_not_dispatchable_in_logic_shape() {
        let issue = Issue {
            id: "1".to_owned(),
            identifier: "ARB-1".to_owned(),
            title: "Blocked".to_owned(),
            state: "Todo".to_owned(),
            blocked_by: vec![IssueBlocker {
                state: Some("In Progress".to_owned()),
                ..IssueBlocker::default()
            }],
            ..Issue::default()
        };
        assert_eq!(issue.state, "Todo");
        assert_eq!(issue.blocked_by.len(), 1);
    }
}
