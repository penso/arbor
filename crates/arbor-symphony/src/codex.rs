use {
    crate::{
        domain::{CodexRateLimits, CodexTotals, Issue},
        tracker::IssueTracker,
        workflow::TypedWorkflowConfig,
    },
    async_trait::async_trait,
    serde_json::{Value, json},
    std::{path::PathBuf, process::Stdio, sync::Arc},
    thiserror::Error,
    tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
        process::{Child, ChildStdin, ChildStdout, Command},
        sync::mpsc,
        time::{Duration, Instant},
    },
};

#[derive(Clone)]
pub struct RunAttemptRequest {
    pub issue: Issue,
    pub attempt: Option<u32>,
    pub prompt: String,
    pub workspace_path: PathBuf,
    pub config: TypedWorkflowConfig,
    pub tracker: Arc<dyn IssueTracker>,
}

#[derive(Debug, Clone, Default)]
pub struct RunResult {
    pub outcome: RunOutcome,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_count: u32,
    pub totals: CodexTotals,
    pub rate_limits: Option<CodexRateLimits>,
    pub last_event: Option<String>,
    pub last_message: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    InputRequired,
    #[default]
    StartupFailed,
}

#[derive(Debug, Clone, Default)]
pub struct RunnerEvent {
    pub event: String,
    pub session_id: Option<String>,
    pub message: Option<String>,
    pub totals: Option<CodexTotals>,
    pub rate_limits: Option<CodexRateLimits>,
    pub at: String,
}

#[derive(Debug, Error, Clone)]
pub enum RunnerError {
    #[error("codex_not_found: {0}")]
    CodexNotFound(String),
    #[error("invalid_workspace_cwd: {0}")]
    InvalidWorkspaceCwd(String),
    #[error("response_timeout")]
    ResponseTimeout,
    #[error("turn_timeout")]
    TurnTimeout,
    #[error("port_exit")]
    PortExit,
    #[error("response_error: {0}")]
    ResponseError(String),
    #[error("turn_failed: {0}")]
    TurnFailed(String),
    #[error("turn_cancelled: {0}")]
    TurnCancelled(String),
    #[error("turn_input_required")]
    TurnInputRequired,
    #[error("io: {0}")]
    Io(String),
}

impl From<std::io::Error> for RunnerError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for RunnerError {
    fn from(error: serde_json::Error) -> Self {
        Self::ResponseError(error.to_string())
    }
}

#[async_trait]
pub trait Runner: Send + Sync {
    async fn run_attempt(
        &self,
        request: RunAttemptRequest,
        events: mpsc::UnboundedSender<RunnerEvent>,
    ) -> Result<RunResult, RunnerError>;
}

#[derive(Debug, Clone, Default)]
pub struct AppServerRunner;

#[async_trait]
impl Runner for AppServerRunner {
    async fn run_attempt(
        &self,
        request: RunAttemptRequest,
        events: mpsc::UnboundedSender<RunnerEvent>,
    ) -> Result<RunResult, RunnerError> {
        if !request.workspace_path.is_dir() {
            return Err(RunnerError::InvalidWorkspaceCwd(
                request.workspace_path.display().to_string(),
            ));
        }

        let mut child = spawn_process(&request).await?;
        let run_result = async {
            let stdin = child
                .stdin
                .take()
                .ok_or_else(|| RunnerError::Io("missing stdin".to_owned()))?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| RunnerError::Io("missing stdout".to_owned()))?;
            if let Some(stderr) = child.stderr.take() {
                tokio::spawn(log_stderr(stderr));
            }

            let mut process = AppServerProcess::new(stdin, stdout);
            process.initialize(&request).await?;
            let thread_id = process.start_thread(&request).await?;

            let mut result = RunResult {
                thread_id: Some(thread_id.clone()),
                ..RunResult::default()
            };
            let mut current_issue = request.issue.clone();

            for turn_index in 0..request.config.agent.max_turns {
                let turn_prompt = if turn_index == 0 {
                    request.prompt.clone()
                } else {
                    "Continue working on the same issue. Use the existing thread context and focus only on the remaining work.".to_owned()
                };

                let turn_id = process
                    .start_turn(&thread_id, &turn_prompt, &request)
                    .await?;
                let session_id = format!("{thread_id}-{turn_id}");
                result.session_id = Some(session_id.clone());
                result.turn_count = turn_index + 1;
                let _ = events.send(RunnerEvent {
                    event: "session_started".to_owned(),
                    session_id: Some(session_id.clone()),
                    message: None,
                    totals: None,
                    rate_limits: None,
                    at: now_rfc3339(),
                });

                let turn_outcome = process
                    .stream_turn(&session_id, &mut result, &events, &request.config)
                    .await?;

                match turn_outcome {
                    RunOutcome::Completed => {
                        let refreshed = request
                            .tracker
                            .fetch_issue_states_by_ids(&[current_issue.id.clone()])
                            .await
                            .map_err(|error| RunnerError::ResponseError(error.to_string()))?;
                        if let Some(issue) = refreshed.into_iter().next() {
                            current_issue = issue;
                        }
                        result.outcome = RunOutcome::Completed;
                        if !request
                            .config
                            .tracker
                            .active_states
                            .iter()
                            .any(|state| state.eq_ignore_ascii_case(&current_issue.state))
                        {
                            break;
                        }
                    },
                    outcome => {
                        result.outcome = outcome;
                        break;
                    },
                }
            }

            Ok(result)
        }
        .await;
        stop_child(&mut child).await;
        run_result
    }
}

struct AppServerProcess {
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_request_id: u64,
}

impl AppServerProcess {
    fn new(stdin: ChildStdin, stdout: ChildStdout) -> Self {
        Self {
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_request_id: 1,
        }
    }

    async fn initialize(&mut self, request: &RunAttemptRequest) -> Result<(), RunnerError> {
        let initialize_id = self.next_request_id();
        self.send_message(&json!({
            "id": initialize_id,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "arbor-symphony",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {}
            }
        }))
        .await?;
        let _ = self
            .read_response(initialize_id, request.config.codex.read_timeout_ms)
            .await?;
        self.send_message(&json!({
            "method": "initialized",
            "params": {}
        }))
        .await?;
        Ok(())
    }

    async fn start_thread(&mut self, request: &RunAttemptRequest) -> Result<String, RunnerError> {
        let request_id = self.next_request_id();
        self.send_message(&json!({
            "id": request_id,
            "method": "thread/start",
            "params": {
                "approvalPolicy": request.config.codex.approval_policy,
                "sandbox": request.config.codex.thread_sandbox,
                "cwd": request.workspace_path,
            }
        }))
        .await?;
        let response = self
            .read_response(request_id, request.config.codex.read_timeout_ms)
            .await?;

        response
            .pointer("/result/thread/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| RunnerError::ResponseError("missing thread id".to_owned()))
    }

    async fn start_turn(
        &mut self,
        thread_id: &str,
        prompt: &str,
        request: &RunAttemptRequest,
    ) -> Result<String, RunnerError> {
        let request_id = self.next_request_id();
        self.send_message(&json!({
            "id": request_id,
            "method": "turn/start",
            "params": {
                "threadId": thread_id,
                "input": [{ "type": "text", "text": prompt }],
                "cwd": request.workspace_path,
                "title": format!("{}: {}", request.issue.identifier, request.issue.title),
                "approvalPolicy": request.config.codex.approval_policy,
                "sandboxPolicy": request.config.codex.turn_sandbox_policy,
            }
        }))
        .await?;
        let response = self
            .read_response(request_id, request.config.codex.read_timeout_ms)
            .await?;

        response
            .pointer("/result/turn/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| RunnerError::ResponseError("missing turn id".to_owned()))
    }

    async fn stream_turn(
        &mut self,
        session_id: &str,
        result: &mut RunResult,
        events: &mpsc::UnboundedSender<RunnerEvent>,
        config: &TypedWorkflowConfig,
    ) -> Result<RunOutcome, RunnerError> {
        let started = Instant::now();
        loop {
            let line = tokio::time::timeout(
                Duration::from_millis(config.codex.turn_timeout_ms),
                self.stdout.next_line(),
            )
            .await
            .map_err(|_| RunnerError::TurnTimeout)??;

            let Some(line) = line else {
                return Err(RunnerError::PortExit);
            };
            if line.trim().is_empty() {
                continue;
            }

            let message: Value = serde_json::from_str(&line)?;
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                if let Some(outcome) = handle_server_method(
                    method,
                    &message,
                    &mut self.stdin,
                    result,
                    events,
                    session_id,
                )
                .await?
                {
                    return Ok(outcome);
                }
            } else if message.get("error").is_some() {
                return Err(RunnerError::ResponseError(message.to_string()));
            }

            if started.elapsed() > Duration::from_millis(config.codex.turn_timeout_ms) {
                return Err(RunnerError::TurnTimeout);
            }
        }
    }

    async fn read_response(
        &mut self,
        request_id: u64,
        timeout_ms: u64,
    ) -> Result<Value, RunnerError> {
        loop {
            let line =
                tokio::time::timeout(Duration::from_millis(timeout_ms), self.stdout.next_line())
                    .await
                    .map_err(|_| RunnerError::ResponseTimeout)??;

            let Some(line) = line else {
                return Err(RunnerError::PortExit);
            };
            if line.trim().is_empty() {
                continue;
            }

            let value: Value = serde_json::from_str(&line)?;
            if value.get("id").and_then(Value::as_u64) == Some(request_id) {
                if value.get("error").is_some() {
                    return Err(RunnerError::ResponseError(value.to_string()));
                }
                return Ok(value);
            }

            if let Some(method) = value.get("method").and_then(Value::as_str) {
                let mut sink = RunResult::default();
                let (tx, _rx) = mpsc::unbounded_channel();
                let _ = handle_server_method(method, &value, &mut self.stdin, &mut sink, &tx, "")
                    .await?;
            }
        }
    }

    async fn send_message(&mut self, value: &Value) -> Result<(), RunnerError> {
        let mut line = serde_json::to_vec(value)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }
}

async fn handle_server_method(
    method: &str,
    message: &Value,
    stdin: &mut ChildStdin,
    result: &mut RunResult,
    events: &mpsc::UnboundedSender<RunnerEvent>,
    session_id: &str,
) -> Result<Option<RunOutcome>, RunnerError> {
    if method == "turn/completed" {
        let event = "turn_completed".to_owned();
        result.last_event = Some(event.clone());
        let _ = events.send(RunnerEvent {
            event,
            session_id: Some(session_id.to_owned()),
            message: None,
            totals: Some(result.totals.clone()),
            rate_limits: result.rate_limits.clone(),
            at: now_rfc3339(),
        });
        return Ok(Some(RunOutcome::Completed));
    }

    if method == "turn/failed" {
        let error = message.to_string();
        result.last_event = Some("turn_failed".to_owned());
        result.last_message = Some(error.clone());
        return Ok(Some(RunOutcome::Failed));
    }

    if method == "turn/cancelled" {
        result.last_event = Some("turn_cancelled".to_owned());
        return Ok(Some(RunOutcome::Cancelled));
    }

    if is_approval_method(method) {
        respond(
            stdin,
            message.get("id").and_then(Value::as_u64),
            json!({ "approved": true }),
        )
        .await?;
        let _ = events.send(RunnerEvent {
            event: "approval_auto_approved".to_owned(),
            session_id: Some(session_id.to_owned()),
            message: Some(method.to_owned()),
            totals: None,
            rate_limits: None,
            at: now_rfc3339(),
        });
        return Ok(None);
    }

    if method == "item/tool/requestUserInput" {
        respond(
            stdin,
            message.get("id").and_then(Value::as_u64),
            json!({ "approved": false, "reason": "user input disabled" }),
        )
        .await?;
        return Ok(Some(RunOutcome::InputRequired));
    }

    if method == "item/tool/call" {
        respond(
            stdin,
            message.get("id").and_then(Value::as_u64),
            json!({ "success": false, "error": "unsupported_tool_call" }),
        )
        .await?;
        let _ = events.send(RunnerEvent {
            event: "unsupported_tool_call".to_owned(),
            session_id: Some(session_id.to_owned()),
            message: message
                .pointer("/params/name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            totals: None,
            rate_limits: None,
            at: now_rfc3339(),
        });
        return Ok(None);
    }

    if let Some(totals) = extract_totals(message) {
        result.totals = totals.clone();
    }
    if let Some(rate_limits) = extract_rate_limits(message) {
        result.rate_limits = Some(rate_limits.clone());
    }

    result.last_event = Some(method.to_owned());
    result.last_message = summarize_message(message);
    let _ = events.send(RunnerEvent {
        event: method.to_owned(),
        session_id: Some(session_id.to_owned()),
        message: result.last_message.clone(),
        totals: Some(result.totals.clone()),
        rate_limits: result.rate_limits.clone(),
        at: now_rfc3339(),
    });
    Ok(None)
}

fn extract_totals(message: &Value) -> Option<CodexTotals> {
    let input_tokens = find_u64(message, &["input_tokens", "inputTokens"])?;
    let output_tokens = find_u64(message, &["output_tokens", "outputTokens"]).unwrap_or(0);
    let total_tokens =
        find_u64(message, &["total_tokens", "totalTokens"]).unwrap_or(input_tokens + output_tokens);
    Some(CodexTotals {
        input_tokens,
        output_tokens,
        total_tokens,
        seconds_running: 0,
    })
}

fn extract_rate_limits(message: &Value) -> Option<CodexRateLimits> {
    let rate_limits = message
        .pointer("/params/rateLimits")
        .or_else(|| message.pointer("/params/rate_limits"))
        .or_else(|| message.pointer("/result/rateLimits"))
        .or_else(|| message.pointer("/result/rate_limits"))?;

    let mut values = std::collections::HashMap::new();
    if let Some(object) = rate_limits.as_object() {
        for (key, value) in object {
            values.insert(key.clone(), value.to_string());
        }
    }
    Some(CodexRateLimits { values })
}

fn find_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(found) = object.get(*key).and_then(Value::as_u64) {
                    return Some(found);
                }
            }
            object.values().find_map(|value| find_u64(value, keys))
        },
        Value::Array(items) => items.iter().find_map(|value| find_u64(value, keys)),
        _ => None,
    }
}

fn summarize_message(message: &Value) -> Option<String> {
    message
        .pointer("/params/message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            message
                .pointer("/params/text")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
}

fn is_approval_method(method: &str) -> bool {
    method.contains("approval") || method.ends_with("/requestPermission")
}

async fn respond(
    stdin: &mut ChildStdin,
    request_id: Option<u64>,
    result: Value,
) -> Result<(), RunnerError> {
    let Some(request_id) = request_id else {
        return Ok(());
    };
    let mut payload = serde_json::to_vec(&json!({
        "id": request_id,
        "result": result,
    }))?;
    payload.push(b'\n');
    stdin.write_all(&payload).await?;
    stdin.flush().await?;
    Ok(())
}

async fn spawn_process(request: &RunAttemptRequest) -> Result<Child, RunnerError> {
    let mut command = Command::new("bash");
    command
        .arg("-lc")
        .arg(&request.config.codex.command)
        .current_dir(&request.workspace_path)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            RunnerError::CodexNotFound(error.to_string())
        } else {
            RunnerError::from(error)
        }
    })
}

async fn stop_child(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

async fn log_stderr(stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => tracing::debug!(line, "codex stderr"),
            Ok(None) => break,
            Err(error) => {
                tracing::debug!(%error, "codex stderr stream failed");
                break;
            },
        }
    }
}

fn now_rfc3339() -> String {
    let now = std::time::SystemTime::now();
    format!(
        "{:?}",
        now.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
    )
}

pub type AppServerRunnerType = AppServerRunner;
