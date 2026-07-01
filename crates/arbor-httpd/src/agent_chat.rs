//! Interactive agent chat session manager.
//!
//! Supports two transport modes:
//! - **ACP** (Agent Client Protocol): spawns agent CLI processes via `acpx`,
//!   parses their JSONL stdout into structured events.
//! - **OpenAI-compatible**: sends HTTP requests to `/v1/chat/completions`
//!   endpoints (Ollama, LM Studio, OpenRouter, Requesty, OpenAI, etc.) and streams SSE
//!   responses.
//!
//! Events from both transports are broadcast over a `tokio::sync::broadcast`
//! channel for WebSocket consumers.

use {
    futures_util::StreamExt,
    serde::{Deserialize, Serialize},
    serde_json::Value,
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::Arc,
    },
    tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        process::Command,
        sync::{Mutex, broadcast},
    },
};

/// Relative path under `$HOME` for the persistent agent chat store.
const AGENT_CHAT_STORE_RELATIVE_PATH: &str = ".arbor/daemon/agent-chats.json";

// ── Public types ─────────────────────────────────────────────────────

/// Status of an agent chat session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentChatStatus {
    /// Waiting for user input.
    Idle,
    /// Agent is processing a turn.
    Working,
    /// Agent process has exited (session ended or error).
    Exited,
}

/// Transport used by an agent chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AgentChatTransport {
    /// ACP agent via acpx CLI subprocess.
    Acp,
    /// OpenAI-compatible HTTP API (Ollama, LM Studio, OpenRouter, Requesty, etc.).
    OpenAiChat {
        base_url: String,
        api_key: Option<String>,
    },
}

/// A structured event emitted by an agent session, streamed to the web UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum AgentChatEvent {
    /// A chunk of the assistant's text response (streamed).
    MessageChunk { content: String },
    /// A chunk of the agent's internal reasoning/thinking.
    ThoughtChunk { content: String },
    /// A tool invocation by the agent.
    ToolCall { name: String, status: String },
    /// Agent started processing a turn.
    TurnStarted,
    /// Agent finished processing a turn.
    TurnCompleted,
    /// Token usage update.
    UsageUpdate {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// Error from the agent.
    Error { message: String },
    /// The agent process exited.
    SessionExited { exit_code: Option<i32> },
    /// Snapshot of the full conversation history (sent on WebSocket connect).
    Snapshot {
        messages: Vec<ChatMessage>,
        status: AgentChatStatus,
        input_tokens: u64,
        output_tokens: u64,
        /// Transport label for the session (e.g. "acp:claude", "openai:http://…").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transport_label: Option<String>,
    },
    /// A complete user message (for history reconstruction).
    UserMessage { content: String },
    /// Status update (mode changes, config updates, etc.).
    StatusUpdate { message: String },
}

/// A message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatMessage {
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) tool_calls: Vec<String>,
    /// Per-turn input tokens (only set on assistant messages).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) input_tokens: u64,
    /// Per-turn output tokens (only set on assistant messages).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) output_tokens: u64,
    /// Model used for this turn (e.g. "claude-sonnet-4-20250514", "llama3.1:70b").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model_id: Option<String>,
    /// Transport label for debugging (e.g. "acp:claude", "openai:http://localhost:11434/v1").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transport_label: Option<String>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// DTO for the agent chat session list endpoint.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AgentChatSessionDto {
    pub(crate) id: String,
    pub(crate) agent_kind: String,
    pub(crate) workspace_path: String,
    pub(crate) status: AgentChatStatus,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    /// Human-readable transport label (e.g. "acp:claude", "openai:http://…").
    pub(crate) transport_label: String,
}

/// Request to create a new agent chat session.
#[derive(Debug, Deserialize)]
pub(crate) struct CreateAgentChatRequest {
    pub(crate) workspace_path: String,
    pub(crate) agent_kind: String,
    pub(crate) initial_prompt: Option<String>,
    /// Model identifier to pass via `--model` to acpx (e.g. "claude-sonnet-4-20250514").
    pub(crate) model_id: Option<String>,
    /// Transport configuration. Defaults to ACP if omitted.
    #[serde(default)]
    pub(crate) transport: Option<AgentChatTransport>,
}

/// Response from creating an agent chat session.
#[derive(Debug, Serialize)]
pub(crate) struct CreateAgentChatResponse {
    pub(crate) session_id: String,
}

/// Request to send a message to an agent.
#[derive(Debug, Deserialize)]
pub(crate) struct SendAgentMessageRequest {
    pub(crate) message: String,
}

/// A discovered model from an OpenAI-compatible provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DiscoveredModel {
    pub(crate) id: String,
    pub(crate) display_name: Option<String>,
}

/// Request to discover models from an OpenAI-compatible provider.
#[derive(Debug, Deserialize)]
pub(crate) struct DiscoverModelsRequest {
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
}

/// Response from model discovery.
#[derive(Debug, Serialize)]
pub(crate) struct DiscoverModelsResponse {
    pub(crate) models: Vec<DiscoveredModel>,
}

// ── Persistent session record ────────────────────────────────────────

/// A serializable snapshot of an agent chat session for persistence across
/// daemon restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentChatRecord {
    id: String,
    agent_kind: String,
    workspace_path: PathBuf,
    session_name: String,
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default = "default_transport")]
    transport: AgentChatTransport,
    messages: Vec<ChatMessage>,
    input_tokens: u64,
    output_tokens: u64,
}

fn default_transport() -> AgentChatTransport {
    AgentChatTransport::Acp
}

// ── Internal session state ───────────────────────────────────────────

struct AgentChatSession {
    id: String,
    agent_kind: String,
    workspace_path: PathBuf,
    session_name: String,
    /// Model identifier passed via `--model` to acpx or used in OpenAI requests.
    model_id: Option<String>,
    /// Transport used for this session.
    transport: AgentChatTransport,
    event_tx: broadcast::Sender<AgentChatEvent>,
    messages: Vec<ChatMessage>,
    /// Text being streamed for the current assistant turn (not yet finalized).
    pending_assistant_text: String,
    /// Tool calls accumulated during the current turn.
    pending_tool_calls: Vec<String>,
    status: AgentChatStatus,
    /// Cumulative input tokens across all turns.
    input_tokens: u64,
    /// Cumulative output tokens across all turns.
    output_tokens: u64,
    /// Cumulative tokens at the start of the current turn (for per-turn delta).
    turn_start_input_tokens: u64,
    turn_start_output_tokens: u64,
    /// Handle to cancel a running turn.
    turn_cancel: Option<tokio::sync::watch::Sender<bool>>,
}

impl AgentChatSession {
    /// Human-readable transport label for debugging.
    fn transport_label(&self) -> String {
        match &self.transport {
            AgentChatTransport::Acp => format!("acp:{}", self.agent_kind),
            AgentChatTransport::OpenAiChat { base_url, .. } => {
                format!("openai:{base_url}")
            },
        }
    }
}

// ── Manager ──────────────────────────────────────────────────────────

/// Manages interactive agent chat sessions.
pub(crate) struct AgentChatManager {
    sessions: HashMap<String, AgentChatSession>,
    http_client: reqwest::Client,
    next_id: u64,
}

impl AgentChatManager {
    pub(crate) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            http_client: reqwest::Client::new(),
            next_id: 0,
        }
    }

    /// Load previously persisted agent chat sessions from disk.
    /// Restored sessions are idle (no running process) but can accept new
    /// messages which will resume the underlying session.
    ///
    /// Returns the list of `(session_id, event_rx)` pairs so the caller can
    /// spawn background listeners for each restored session.
    pub(crate) fn load_persisted_sessions(
        &mut self,
    ) -> Vec<(String, broadcast::Receiver<AgentChatEvent>)> {
        let path = agent_chat_store_path();
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                tracing::warn!(%e, "failed to read agent chat store");
                return Vec::new();
            },
        };

        let records: Vec<AgentChatRecord> = match serde_json::from_str(&data) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, "failed to parse agent chat store");
                return Vec::new();
            },
        };

        let mut restored = Vec::new();

        for record in records {
            // Skip sessions that are already loaded (shouldn't happen, but guard)
            if self.sessions.contains_key(&record.id) {
                continue;
            }

            let (event_tx, event_rx) = broadcast::channel::<AgentChatEvent>(256);
            let session_id = record.id.clone();
            let session = AgentChatSession {
                id: record.id.clone(),
                agent_kind: record.agent_kind.clone(),
                workspace_path: record.workspace_path,
                session_name: record.session_name,
                model_id: record.model_id,
                transport: record.transport,
                event_tx,
                messages: record.messages,
                pending_assistant_text: String::new(),
                pending_tool_calls: Vec::new(),
                status: AgentChatStatus::Idle,
                input_tokens: record.input_tokens,
                output_tokens: record.output_tokens,
                turn_start_input_tokens: record.input_tokens,
                turn_start_output_tokens: record.output_tokens,
                turn_cancel: None,
            };
            self.sessions.insert(record.id, session);
            restored.push((session_id, event_rx));
        }

        tracing::info!(
            count = self.sessions.len(),
            "restored agent chat sessions from disk"
        );
        restored
    }

    /// Persist all non-exited sessions to disk.
    pub(crate) fn persist(&self) {
        let records: Vec<AgentChatRecord> = self
            .sessions
            .values()
            .filter(|s| s.status != AgentChatStatus::Exited)
            .map(|s| {
                let mut messages = s.messages.clone();
                // Include any pending assistant text as a finalized message in
                // the persisted record so it isn't lost.
                if !s.pending_assistant_text.is_empty() {
                    messages.push(ChatMessage {
                        role: "assistant".to_owned(),
                        content: s.pending_assistant_text.clone(),
                        tool_calls: s.pending_tool_calls.clone(),
                        input_tokens: 0,
                        output_tokens: 0,
                        model_id: s.model_id.clone(),
                        transport_label: Some(s.transport_label()),
                    });
                }
                AgentChatRecord {
                    id: s.id.clone(),
                    agent_kind: s.agent_kind.clone(),
                    workspace_path: s.workspace_path.clone(),
                    session_name: s.session_name.clone(),
                    model_id: s.model_id.clone(),
                    transport: s.transport.clone(),
                    messages,
                    input_tokens: s.input_tokens,
                    output_tokens: s.output_tokens,
                }
            })
            .collect();

        let path = agent_chat_store_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        match serde_json::to_string_pretty(&records) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, format!("{json}\n")) {
                    tracing::warn!(%e, "failed to write agent chat store");
                }
            },
            Err(e) => {
                tracing::warn!(%e, "failed to serialize agent chat store");
            },
        }
    }

    /// Create a new agent chat session. Optionally starts the first turn with
    /// an initial prompt.
    pub(crate) fn create_session(
        &mut self,
        agent_kind: String,
        workspace_path: PathBuf,
        initial_prompt: Option<String>,
        model_id: Option<String>,
        transport: Option<AgentChatTransport>,
    ) -> (String, broadcast::Receiver<AgentChatEvent>) {
        let id_counter = self.next_id;
        self.next_id += 1;
        let session_id = format!(
            "agent-chat-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            id_counter
        );
        let session_name = format!("arbor-{session_id}");
        let (event_tx, event_rx) = broadcast::channel::<AgentChatEvent>(256);

        let transport = transport.unwrap_or(AgentChatTransport::Acp);
        let http_client = self.http_client.clone();

        let session = AgentChatSession {
            id: session_id.clone(),
            agent_kind: agent_kind.clone(),
            workspace_path: workspace_path.clone(),
            session_name: session_name.clone(),
            model_id,
            transport,
            event_tx: event_tx.clone(),
            messages: Vec::new(),
            pending_assistant_text: String::new(),
            pending_tool_calls: Vec::new(),
            status: AgentChatStatus::Idle,
            input_tokens: 0,
            output_tokens: 0,
            turn_start_input_tokens: 0,
            turn_start_output_tokens: 0,
            turn_cancel: None,
        };

        let label = session.transport_label();
        tracing::info!(
            session_id,
            agent_kind,
            workspace_path = %workspace_path.display(),
            transport = %label,
            model = ?session.model_id,
            "created agent chat session"
        );

        self.sessions.insert(session_id.clone(), session);

        // If there's an initial prompt, start the first turn immediately
        if let Some(prompt) = initial_prompt
            && let Some(session) = self.sessions.get_mut(&session_id)
        {
            session.messages.push(ChatMessage {
                role: "user".to_owned(),
                content: prompt.clone(),
                tool_calls: Vec::new(),
                input_tokens: 0,
                output_tokens: 0,
                model_id: None,
                transport_label: None,
            });
            let _ = event_tx.send(AgentChatEvent::UserMessage {
                content: prompt.clone(),
            });
            start_turn(session, prompt, &http_client);
        }

        (session_id, event_rx)
    }

    /// Send a follow-up message in an existing session.
    pub(crate) fn send_message(&mut self, session_id: &str, message: String) -> Result<(), String> {
        let http_client = self.http_client.clone();
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;

        if session.status == AgentChatStatus::Working {
            return Err("agent is already processing a turn".to_owned());
        }

        tracing::info!(
            session_id,
            transport = %session.transport_label(),
            model = ?session.model_id,
            message_len = message.len(),
            "sending message to agent chat"
        );

        session.messages.push(ChatMessage {
            role: "user".to_owned(),
            content: message.clone(),
            tool_calls: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            model_id: None,
            transport_label: None,
        });
        let _ = session.event_tx.send(AgentChatEvent::UserMessage {
            content: message.clone(),
        });

        start_turn(session, message, &http_client);
        self.persist();
        Ok(())
    }

    /// Cancel a running turn (sends SIGINT to the child process).
    pub(crate) fn cancel(&mut self, session_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;

        if let Some(cancel_tx) = session.turn_cancel.take() {
            let _ = cancel_tx.send(true);
        }
        Ok(())
    }

    /// Kill a session entirely.
    pub(crate) fn kill(&mut self, session_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;

        if let Some(cancel_tx) = session.turn_cancel.take() {
            let _ = cancel_tx.send(true);
        }

        session.status = AgentChatStatus::Exited;
        let _ = session
            .event_tx
            .send(AgentChatEvent::SessionExited { exit_code: None });
        Ok(())
    }

    /// Remove a session from the manager.
    pub(crate) fn remove(&mut self, session_id: &str) {
        if let Some(mut session) = self.sessions.remove(session_id)
            && let Some(cancel_tx) = session.turn_cancel.take()
        {
            let _ = cancel_tx.send(true);
        }
    }

    /// List all active sessions.
    pub(crate) fn list(&self) -> Vec<AgentChatSessionDto> {
        self.sessions
            .values()
            .map(|s| AgentChatSessionDto {
                id: s.id.clone(),
                agent_kind: s.agent_kind.clone(),
                workspace_path: s.workspace_path.display().to_string(),
                status: s.status,
                input_tokens: s.input_tokens,
                output_tokens: s.output_tokens,
                transport_label: s.transport_label(),
            })
            .collect()
    }

    /// Get the conversation history for a session.
    /// Includes any in-progress assistant text as a partial message.
    pub(crate) fn history(&self, session_id: &str) -> Result<Vec<ChatMessage>, String> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;
        let mut messages = session.messages.clone();
        // Append the in-progress assistant response so the GUI can stream it
        if !session.pending_assistant_text.is_empty() {
            messages.push(ChatMessage {
                role: "assistant".to_owned(),
                content: session.pending_assistant_text.clone(),
                tool_calls: session.pending_tool_calls.clone(),
                input_tokens: 0,
                output_tokens: 0,
                model_id: session.model_id.clone(),
                transport_label: Some(session.transport_label()),
            });
        }
        Ok(messages)
    }

    /// Get a broadcast receiver for a session's events.
    pub(crate) fn subscribe(
        &self,
        session_id: &str,
    ) -> Result<
        (
            broadcast::Receiver<AgentChatEvent>,
            AgentChatSessionDto,
            Vec<ChatMessage>,
        ),
        String,
    > {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("session not found: {session_id}"))?;
        Ok((
            session.event_tx.subscribe(),
            AgentChatSessionDto {
                id: session.id.clone(),
                agent_kind: session.agent_kind.clone(),
                workspace_path: session.workspace_path.display().to_string(),
                status: session.status,
                input_tokens: session.input_tokens,
                output_tokens: session.output_tokens,
                transport_label: session.transport_label(),
            },
            session.messages.clone(),
        ))
    }
}

// ── Turn execution ───────────────────────────────────────────────────

/// Start a new turn for the session, dispatching based on transport type.
fn start_turn(session: &mut AgentChatSession, prompt: String, http_client: &reqwest::Client) {
    session.status = AgentChatStatus::Working;
    let _ = session.event_tx.send(AgentChatEvent::TurnStarted);

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    session.turn_cancel = Some(cancel_tx);

    let session_id = session.id.clone();
    let event_tx = session.event_tx.clone();

    match &session.transport {
        AgentChatTransport::Acp => {
            let agent_kind = session.agent_kind.clone();
            let workspace_path = session.workspace_path.clone();
            let session_name = session.session_name.clone();
            let model_id = session.model_id.clone();

            tokio::spawn(async move {
                let result = run_turn_acpx(
                    &agent_kind,
                    &workspace_path,
                    &session_name,
                    &prompt,
                    model_id.as_deref(),
                    &event_tx,
                    cancel_rx,
                )
                .await;

                match result {
                    Ok(()) => {
                        let _ = event_tx.send(AgentChatEvent::TurnCompleted);
                    },
                    Err(error) => {
                        let _ = event_tx.send(AgentChatEvent::Error {
                            message: error.clone(),
                        });
                        tracing::warn!(session_id, %error, "agent turn failed");
                    },
                }
            });
        },
        AgentChatTransport::OpenAiChat { base_url, api_key } => {
            let base_url = base_url.clone();
            let api_key = api_key.clone();
            let model_id = session.model_id.clone();
            let messages = session.messages.clone();
            let client = http_client.clone();

            tokio::spawn(async move {
                let result = run_turn_openai(
                    &client,
                    &base_url,
                    api_key.as_deref(),
                    &model_id,
                    &messages,
                    &event_tx,
                    cancel_rx,
                )
                .await;

                match result {
                    Ok(()) => {
                        let _ = event_tx.send(AgentChatEvent::TurnCompleted);
                    },
                    Err(error) => {
                        let _ = event_tx.send(AgentChatEvent::Error {
                            message: error.clone(),
                        });
                        tracing::warn!(session_id, %error, "openai turn failed");
                    },
                }
            });
        },
    }
}

/// Run a single turn via acpx: spawn subprocess, write prompt to stdin, parse JSONL.
async fn run_turn_acpx(
    agent_kind: &str,
    workspace_path: &Path,
    session_name: &str,
    prompt: &str,
    model_id: Option<&str>,
    event_tx: &broadcast::Sender<AgentChatEvent>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let acpx_path = which_acpx();
    let cwd_str = workspace_path.display().to_string();

    // Ensure the named session exists before prompting.
    let ensure_result = Command::new(&acpx_path)
        .args([
            "--cwd",
            &cwd_str,
            agent_kind,
            "sessions",
            "ensure",
            "--name",
            session_name,
        ])
        .current_dir(workspace_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_remove("CLAUDECODE")
        .output()
        .await
        .map_err(|e| format!("failed to ensure acpx session: {e}"))?;

    if !ensure_result.status.success() {
        let stderr = String::from_utf8_lossy(&ensure_result.stderr);
        tracing::warn!(session_name, %stderr, "acpx sessions ensure failed, continuing anyway");
    }

    let mut args: Vec<String> = vec![
        "--format".into(),
        "json".into(),
        "--json-strict".into(),
        "--cwd".into(),
        cwd_str.clone(),
    ];
    // Pass the model identifier if specified (e.g. "claude-sonnet-4-20250514").
    if let Some(model) = model_id {
        args.push("--model".into());
        args.push(model.to_owned());
    }
    args.extend([
        agent_kind.into(),
        "prompt".into(),
        "--session".into(),
        session_name.into(),
        "--file".into(),
        "-".into(),
    ]);

    let mut child = Command::new(&acpx_path)
        .args(&args)
        .current_dir(workspace_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env_remove("CLAUDECODE")
        .spawn()
        .map_err(|e| format!("failed to spawn acpx: {e}"))?;

    // Write the prompt to stdin and close it.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| format!("failed to write to acpx stdin: {e}"))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| format!("failed to close acpx stdin: {e}"))?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "acpx stdout unavailable".to_owned())?;

    let mut lines = BufReader::new(stdout).lines();
    let mut assistant_text = String::new();

    loop {
        tokio::select! {
            line_result = lines.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        let events = parse_acpx_events(&line);
                        if events.is_empty() {
                            tracing::trace!(line, "acpx line produced no events");
                        }
                        for event in events {
                            tracing::trace!(?event, "acpx event");
                            // Accumulate assistant text for history
                            if let AgentChatEvent::MessageChunk { ref content } = event {
                                assistant_text.push_str(content);
                            }
                            let _ = event_tx.send(event);
                        }
                    },
                    Ok(None) => break, // EOF
                    Err(error) => {
                        let _ = event_tx.send(AgentChatEvent::Error {
                            message: format!("read error: {error}"),
                        });
                        break;
                    },
                }
            },
            _ = cancel_rx.changed() => {
                if *cancel_rx.borrow() {
                    let _ = child.kill().await;
                    return Err("turn cancelled".to_owned());
                }
            },
        }
    }

    // Wait for the process to exit
    let exit_status = child
        .wait()
        .await
        .map_err(|e| format!("failed to wait for acpx: {e}"))?;

    if !exit_status.success() {
        let code = exit_status.code();
        // Read stderr for diagnostics
        let stderr_text = if let Some(mut stderr) = child.stderr.take() {
            let mut buf = String::new();
            let _ = tokio::io::AsyncReadExt::read_to_string(&mut stderr, &mut buf).await;
            buf
        } else {
            String::new()
        };

        if code == Some(127) {
            return Err("acpx not found in PATH".to_owned());
        }
        // If we got no output at all, report the failure
        if assistant_text.is_empty() {
            let detail = if stderr_text.trim().is_empty() {
                format!("agent exited with code {}", code.unwrap_or(-1))
            } else {
                stderr_text.trim().to_owned()
            };
            return Err(detail);
        }
    }

    Ok(())
}

// ── OpenAI-compatible HTTP transport ─────────────────────────────────

/// Run a single turn via OpenAI-compatible HTTP API with SSE streaming.
///
/// Sends the full conversation history plus the latest user message to
/// `{base_url}/chat/completions` with `stream: true` and parses SSE events.
async fn run_turn_openai(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    model_id: &Option<String>,
    messages: &[ChatMessage],
    event_tx: &broadcast::Sender<AgentChatEvent>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let model = model_id
        .as_deref()
        .ok_or_else(|| "no model selected for OpenAI-compatible session".to_owned())?;

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    // Convert conversation history to OpenAI message format.
    let openai_messages: Vec<Value> = messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| {
            serde_json::json!({
                "role": m.role,
                "content": m.content,
            })
        })
        .collect();

    if openai_messages.is_empty() {
        return Err("no messages to send".to_owned());
    }

    tracing::info!(
        model,
        base_url,
        message_count = openai_messages.len(),
        "starting OpenAI-compatible chat request"
    );

    let body = serde_json::json!({
        "model": model,
        "messages": openai_messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });

    let mut request = client.post(&url).header("User-Agent", "arbor").json(&body);

    if let Some(key) = api_key
        && !key.is_empty()
    {
        request = request.bearer_auth(key);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable>".to_owned());
        return Err(format!("API error {status}: {body_text}"));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();

    if content_type.contains("text/event-stream") {
        consume_sse_stream(response, event_tx, &mut cancel_rx).await
    } else {
        // Non-streaming JSON response (some providers may not support streaming)
        consume_json_response(response, event_tx).await
    }
}

/// Consume an SSE (Server-Sent Events) stream from an OpenAI-compatible API.
async fn consume_sse_stream(
    response: reqwest::Response,
    event_tx: &broadcast::Sender<AgentChatEvent>,
    cancel_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk_result) = chunk else { break };
                let chunk_bytes = chunk_result
                    .map_err(|e| format!("stream read error: {e}"))?;
                buffer.push_str(&String::from_utf8_lossy(&chunk_bytes));

                // Process complete lines from the buffer
                while let Some(pos) = buffer.find('\n') {
                    let line = buffer[..pos].trim().to_string();
                    buffer = buffer[pos + 1..].to_string();

                    if line.is_empty() || !line.starts_with("data:") {
                        continue;
                    }

                    let data = line.trim_start_matches("data:").trim();
                    if data == "[DONE]" {
                        return Ok(());
                    }

                    let value: Value = serde_json::from_str(data)
                        .map_err(|e| format!("SSE JSON parse error: {e}"))?;

                    // Extract text content delta
                    if let Some(delta) = value
                        .pointer("/choices/0/delta/content")
                        .and_then(Value::as_str)
                        && !delta.is_empty()
                    {
                        let _ = event_tx.send(AgentChatEvent::MessageChunk {
                            content: delta.to_string(),
                        });
                    }

                    // Extract usage info
                    if let Some((input, output)) = parse_openai_usage(&value) {
                        let _ = event_tx.send(AgentChatEvent::UsageUpdate {
                            input_tokens: input,
                            output_tokens: output,
                        });
                    }

                    // Extract tool calls (report as status, since we don't execute them)
                    if let Some(tool_calls) = value
                        .pointer("/choices/0/delta/tool_calls")
                        .and_then(Value::as_array)
                    {
                        for call in tool_calls {
                            if let Some(name) = call
                                .pointer("/function/name")
                                .and_then(Value::as_str)
                            {
                                let _ = event_tx.send(AgentChatEvent::ToolCall {
                                    name: name.to_owned(),
                                    status: "requested".to_owned(),
                                });
                            }
                        }
                    }
                }
            },
            _ = cancel_rx.changed() => {
                if *cancel_rx.borrow() {
                    return Err("turn cancelled".to_owned());
                }
            },
        }
    }

    Ok(())
}

/// Consume a non-streaming JSON response from an OpenAI-compatible API.
async fn consume_json_response(
    response: reqwest::Response,
    event_tx: &broadcast::Sender<AgentChatEvent>,
) -> Result<(), String> {
    let payload: Value = response
        .json()
        .await
        .map_err(|e| format!("JSON parse error: {e}"))?;

    // Extract the assistant message content
    let content = payload["choices"][0]["message"]["content"]
        .as_str()
        .or_else(|| payload["output_text"].as_str())
        .unwrap_or_default();

    if !content.is_empty() {
        let _ = event_tx.send(AgentChatEvent::MessageChunk {
            content: content.to_owned(),
        });
    }

    // Extract usage
    if let Some((input, output)) = parse_openai_usage(&payload) {
        let _ = event_tx.send(AgentChatEvent::UsageUpdate {
            input_tokens: input,
            output_tokens: output,
        });
    }

    Ok(())
}

/// Parse OpenAI usage fields from a JSON payload.
/// Handles both `prompt_tokens`/`completion_tokens` and `input_tokens`/`output_tokens`.
fn parse_openai_usage(payload: &Value) -> Option<(u64, u64)> {
    let usage = payload.get("usage")?;
    let input = usage
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("input_tokens").and_then(Value::as_u64))?;
    let output = usage
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .or_else(|| usage.get("output_tokens").and_then(Value::as_u64))
        .unwrap_or(0);
    Some((input, output))
}

// ── Model discovery ──────────────────────────────────────────────────

/// Discover available models from an OpenAI-compatible `/v1/models` endpoint.
pub(crate) async fn discover_openai_models(
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<DiscoveredModel>, String> {
    let client = reqwest::Client::new();
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    tracing::info!(base_url, "discovering OpenAI-compatible models");

    let mut request = client.get(&url).header("User-Agent", "arbor");

    if let Some(key) = api_key
        && !key.is_empty()
    {
        request = request.bearer_auth(key);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("model discovery failed: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable>".to_owned());
        return Err(format!("model discovery failed with {status}: {body}"));
    }

    let payload: Value = response
        .json()
        .await
        .map_err(|e| format!("model discovery JSON error: {e}"))?;

    let models = payload
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("id")?.as_str()?.to_owned();
            let display_name = model
                .get("name")
                .or_else(|| model.get("display_name"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            Some(DiscoveredModel { id, display_name })
        })
        .collect::<Vec<_>>();

    tracing::info!(
        base_url,
        count = models.len(),
        "discovered OpenAI-compatible models"
    );

    Ok(models)
}

/// Find the acpx binary.
fn which_acpx() -> String {
    // Check if acpx is in PATH
    if let Ok(output) = std::process::Command::new("which").arg("acpx").output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return path;
        }
    }
    // Fallback to bare command name (will fail at spawn time if not found)
    "acpx".to_owned()
}

// ── JSONL event parsing ──────────────────────────────────────────────

/// Parse a single line of JSONL output from acpx into an `AgentChatEvent`.
///
/// Mirrors polyphony's `parse_acpx_prompt_event_line` in
/// `crates/agent-acpx/src/lib.rs:479-526`.
/// Parse a single JSONL line from acpx output into zero or more events.
///
/// Returns a primary event (if recognized) and optionally a `UsageUpdate` if
/// token counts are found anywhere in the line's JSON (they can appear embedded
/// in various events, not just `usage_update`).
fn parse_acpx_events(line: &str) -> Vec<AgentChatEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let parsed: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(object) = parsed.as_object() else {
        return Vec::new();
    };

    // Handle JSON-RPC error responses (e.g. "no session found")
    if let Some(error_obj) = object.get("error").and_then(Value::as_object) {
        let message = error_obj
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown JSON-RPC error")
            .to_owned();
        return vec![AgentChatEvent::Error { message }];
    }

    // Handle ACP session/update wrapper
    let payload = if object.get("method").and_then(Value::as_str) == Some("session/update") {
        match object
            .get("params")
            .and_then(|p| p.get("update"))
            .and_then(Value::as_object)
        {
            Some(p) => p.clone(),
            None => return Vec::new(),
        }
    } else {
        object.clone()
    };

    let tag = payload
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .or_else(|| payload.get("type").and_then(Value::as_str))
        .unwrap_or_default();

    let mut events = Vec::new();

    match tag {
        "text" | "agent_message_chunk" => {
            if let Some(content) = extract_text(&payload) {
                events.push(AgentChatEvent::MessageChunk { content });
            }
        },
        "thought" | "agent_thought_chunk" => {
            if let Some(content) = extract_text(&payload) {
                events.push(AgentChatEvent::ThoughtChunk { content });
            }
        },
        "tool_call" | "tool_call_update" => {
            let name = payload
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("tool call")
                .to_owned();
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            events.push(AgentChatEvent::ToolCall { name, status });
        },
        "usage_update" => {
            // Log the raw payload so we can see what acpx actually sends
            let raw = serde_json::to_string(&Value::Object(payload.clone())).unwrap_or_default();
            tracing::debug!(raw, "acpx usage_update raw payload");
            // Extraction handled below
        },
        "done" => {
            // Handled by process exit — but may contain usage, checked below
        },
        "error" => {
            let message = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("agent error")
                .to_owned();
            events.push(AgentChatEvent::Error { message });
        },
        "current_mode_update"
        | "config_option_update"
        | "available_commands_update"
        | "session_info_update"
        | "plan"
        | "client_operation"
        | "update" => {
            let message = extract_text(&payload).unwrap_or_else(|| tag.replace('_', " "));
            events.push(AgentChatEvent::StatusUpdate { message });
        },
        _ => {},
    }

    // Try to extract usage from anywhere in the full JSON (not just the
    // unwrapped payload). Token counts can appear at many different paths
    // depending on the agent and protocol version.
    let usage = extract_usage_from_value(&parsed)
        .or_else(|| extract_usage_from_value(&Value::Object(payload)));
    if let Some((input_tokens, output_tokens)) = usage {
        events.push(AgentChatEvent::UsageUpdate {
            input_tokens,
            output_tokens,
        });
    }

    events
}

/// Extract text content from a payload, checking multiple field paths.
fn extract_text(payload: &serde_json::Map<String, Value>) -> Option<String> {
    payload
        .get("content")
        .and_then(|content| {
            if let Some(text) = content.as_str() {
                return Some(text.to_string());
            }
            content
                .as_object()
                .and_then(|obj| obj.get("text").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            payload
                .get("text")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| {
            payload
                .get("summary")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

/// Try to extract `(input_tokens, output_tokens)` from a JSON value by
/// probing many possible paths that different agents use.
///
/// Mirrors the extraction logic from polyphony's `agent-codex` module which
/// checks paths like `params/token_counts/total_token_usage`,
/// `params/tokenUsage`, `params/usage`, `usage`, and both snake_case and
/// camelCase field names.
fn extract_usage_from_value(value: &Value) -> Option<(u64, u64)> {
    // JSON Pointer paths where absolute token usage may appear
    static USAGE_POINTERS: &[&str] = &[
        "/params/token_counts/total_token_usage",
        "/params/token_counts/totalTokenUsage",
        "/result/token_counts/total_token_usage",
        "/result/token_counts/totalTokenUsage",
        "/params/total_token_usage",
        "/params/totalTokenUsage",
        "/result/total_token_usage",
        "/result/totalTokenUsage",
        "/params/token_usage",
        "/params/tokenUsage",
        "/result/token_usage",
        "/result/tokenUsage",
        "/params/usage",
        "/result/usage",
        "/usage",
        // Inside the unwrapped payload (for session/update wrapper)
        "/tokenUsage",
        "/token_usage",
    ];

    for pointer in USAGE_POINTERS {
        if let Some(usage_obj) = value.pointer(pointer)
            && let Some(pair) = parse_token_pair(usage_obj)
        {
            return Some(pair);
        }
    }
    // As a last resort, try parsing the value itself (top-level fields)
    parse_token_pair(value)
}

/// Parse `(input_tokens, output_tokens)` from a usage object, accepting both
/// snake_case and camelCase field names.
fn parse_token_pair(obj: &Value) -> Option<(u64, u64)> {
    let input = obj
        .get("input_tokens")
        .or_else(|| obj.get("inputTokens"))
        .and_then(Value::as_u64)?;
    let output = obj
        .get("output_tokens")
        .or_else(|| obj.get("outputTokens"))
        .and_then(Value::as_u64)?;
    Some((input, output))
}

// ── Background event listener ────────────────────────────────────────

/// Spawn a background task that listens to a session's events and updates the
/// manager's state (accumulates messages, tracks status, updates tokens).
///
/// This is called after creating a session to keep the in-memory state in sync.
pub(crate) fn spawn_session_listener(
    manager: Arc<Mutex<AgentChatManager>>,
    session_id: String,
    mut event_rx: broadcast::Receiver<AgentChatEvent>,
) {
    tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    let mut mgr = manager.lock().await;
                    let Some(session) = mgr.sessions.get_mut(&session_id) else {
                        break;
                    };

                    match &event {
                        AgentChatEvent::MessageChunk { content } => {
                            // Append to pending text — history() includes this
                            // as a streaming partial message.
                            session.pending_assistant_text.push_str(content);
                        },
                        AgentChatEvent::ToolCall { name, status } => {
                            session
                                .pending_tool_calls
                                .push(format!("{name} ({status})"));
                        },
                        AgentChatEvent::TurnCompleted => {
                            // Finalize: move pending text into permanent messages
                            if !session.pending_assistant_text.is_empty() {
                                let text = std::mem::take(&mut session.pending_assistant_text);
                                let tools = std::mem::take(&mut session.pending_tool_calls);
                                let turn_input = session
                                    .input_tokens
                                    .saturating_sub(session.turn_start_input_tokens);
                                let mut turn_output = session
                                    .output_tokens
                                    .saturating_sub(session.turn_start_output_tokens);
                                // If usage delta is zero, estimate from text length
                                // (~4 chars per token).
                                if turn_output == 0 && !text.is_empty() {
                                    turn_output = (text.len() as u64).div_ceil(4);
                                }
                                tracing::debug!(
                                    session_id,
                                    cumulative_in = session.input_tokens,
                                    cumulative_out = session.output_tokens,
                                    turn_start_in = session.turn_start_input_tokens,
                                    turn_start_out = session.turn_start_output_tokens,
                                    turn_input,
                                    turn_output,
                                    text_len = text.len(),
                                    "finalizing assistant message with per-turn tokens"
                                );
                                session.messages.push(ChatMessage {
                                    role: "assistant".to_owned(),
                                    content: text,
                                    tool_calls: tools,
                                    input_tokens: turn_input,
                                    output_tokens: turn_output,
                                    model_id: session.model_id.clone(),
                                    transport_label: Some(session.transport_label()),
                                });
                            }
                            session.status = AgentChatStatus::Idle;
                            mgr.persist();
                        },
                        AgentChatEvent::Error { message } => {
                            // Finalize any partial text, then record the error
                            if !session.pending_assistant_text.is_empty() {
                                let text = std::mem::take(&mut session.pending_assistant_text);
                                let tools = std::mem::take(&mut session.pending_tool_calls);
                                let turn_input = session
                                    .input_tokens
                                    .saturating_sub(session.turn_start_input_tokens);
                                let mut turn_output = session
                                    .output_tokens
                                    .saturating_sub(session.turn_start_output_tokens);
                                if turn_output == 0 && !text.is_empty() {
                                    turn_output = (text.len() as u64).div_ceil(4);
                                }
                                session.messages.push(ChatMessage {
                                    role: "assistant".to_owned(),
                                    content: text,
                                    tool_calls: tools,
                                    input_tokens: turn_input,
                                    output_tokens: turn_output,
                                    model_id: session.model_id.clone(),
                                    transport_label: Some(session.transport_label()),
                                });
                            } else {
                                session.pending_assistant_text.clear();
                                session.pending_tool_calls.clear();
                            }
                            session.messages.push(ChatMessage {
                                role: "error".to_owned(),
                                content: message.clone(),
                                tool_calls: Vec::new(),
                                input_tokens: 0,
                                output_tokens: 0,
                                model_id: None,
                                transport_label: None,
                            });
                            session.status = AgentChatStatus::Idle;
                            mgr.persist();
                        },
                        AgentChatEvent::SessionExited { .. } => {
                            session.status = AgentChatStatus::Exited;
                            mgr.persist();
                            break;
                        },
                        AgentChatEvent::UsageUpdate {
                            input_tokens,
                            output_tokens,
                        } => {
                            tracing::debug!(
                                session_id,
                                input_tokens,
                                output_tokens,
                                prev_input = session.input_tokens,
                                prev_output = session.output_tokens,
                                "usage update received (cumulative overwrite)"
                            );
                            // Values are cumulative totals (e.g. from acpx
                            // total_token_usage). Overwrite, not accumulate.
                            session.input_tokens = *input_tokens;
                            session.output_tokens = *output_tokens;
                        },
                        AgentChatEvent::TurnStarted => {
                            session.pending_assistant_text.clear();
                            session.pending_tool_calls.clear();
                            session.turn_start_input_tokens = session.input_tokens;
                            session.turn_start_output_tokens = session.output_tokens;
                            session.status = AgentChatStatus::Working;
                        },
                        _ => {},
                    }
                },
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(session_id, skipped, "session listener lagged");
                },
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Resolve the path to the persistent agent chat store file.
fn agent_chat_store_path() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) => PathBuf::from(home).join(AGENT_CHAT_STORE_RELATIVE_PATH),
        Err(_) => PathBuf::from(AGENT_CHAT_STORE_RELATIVE_PATH),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_chunk() {
        let line = r#"{"type":"agent_message_chunk","content":{"type":"text","text":"Hello"}}"#;
        let event = parse_acpx_events(line).into_iter().next().unwrap();
        match event {
            AgentChatEvent::MessageChunk { content } => assert_eq!(content, "Hello"),
            other => panic!("expected MessageChunk, got {other:?}"),
        }
    }

    #[test]
    fn parse_thought_chunk() {
        let line =
            r#"{"type":"agent_thought_chunk","content":{"type":"text","text":"thinking..."}}"#;
        let event = parse_acpx_events(line).into_iter().next().unwrap();
        match event {
            AgentChatEvent::ThoughtChunk { content } => assert_eq!(content, "thinking..."),
            other => panic!("expected ThoughtChunk, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_call() {
        let line = r#"{"type":"tool_call","title":"Read file","status":"completed"}"#;
        let event = parse_acpx_events(line).into_iter().next().unwrap();
        match event {
            AgentChatEvent::ToolCall { name, status } => {
                assert_eq!(name, "Read file");
                assert_eq!(status, "completed");
            },
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn parse_error() {
        let line = r#"{"type":"error","message":"rate limited"}"#;
        let event = parse_acpx_events(line).into_iter().next().unwrap();
        match event {
            AgentChatEvent::Error { message } => assert_eq!(message, "rate limited"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn parse_done_returns_none() {
        let line = r#"{"type":"done"}"#;
        assert!(parse_acpx_events(line).is_empty());
    }

    #[test]
    fn parse_empty_line_returns_none() {
        assert!(parse_acpx_events("").is_empty());
        assert!(parse_acpx_events("  ").is_empty());
    }

    #[test]
    fn parse_session_update_wrapper() {
        let line = r#"{"method":"session/update","params":{"update":{"type":"agent_message_chunk","content":"hi"}}}"#;
        let event = parse_acpx_events(line).into_iter().next().unwrap();
        match event {
            AgentChatEvent::MessageChunk { content } => assert_eq!(content, "hi"),
            other => panic!("expected MessageChunk, got {other:?}"),
        }
    }

    #[test]
    fn parse_acpx_usage_in_params() {
        // Token usage embedded in params/tokenUsage (Claude-style)
        let line = r#"{"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"inputTokens":21,"outputTokens":13,"totalTokens":34}}}"#;
        let events = parse_acpx_events(line);
        let usage = events
            .iter()
            .find(|e| matches!(e, AgentChatEvent::UsageUpdate { .. }))
            .expect("expected UsageUpdate");
        match usage {
            AgentChatEvent::UsageUpdate {
                input_tokens,
                output_tokens,
            } => {
                assert_eq!(*input_tokens, 21);
                assert_eq!(*output_tokens, 13);
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn parse_acpx_usage_total_token_usage() {
        // Token usage at params/token_counts/total_token_usage (Codex-style)
        let line = r#"{"method":"notification","params":{"token_counts":{"total_token_usage":{"input_tokens":55,"output_tokens":34,"total_tokens":89}}}}"#;
        let events = parse_acpx_events(line);
        let usage = events
            .iter()
            .find(|e| matches!(e, AgentChatEvent::UsageUpdate { .. }))
            .expect("expected UsageUpdate");
        match usage {
            AgentChatEvent::UsageUpdate {
                input_tokens,
                output_tokens,
            } => {
                assert_eq!(*input_tokens, 55);
                assert_eq!(*output_tokens, 34);
            },
            _ => unreachable!(),
        }
    }

    #[test]
    fn parse_openai_usage_standard() {
        let payload = serde_json::json!({
            "usage": {"prompt_tokens": 12, "completion_tokens": 8}
        });
        let (input, output) = parse_openai_usage(&payload).unwrap();
        assert_eq!(input, 12);
        assert_eq!(output, 8);
    }

    #[test]
    fn parse_openai_usage_anthropic_style() {
        let payload = serde_json::json!({
            "usage": {"input_tokens": 100, "output_tokens": 50}
        });
        let (input, output) = parse_openai_usage(&payload).unwrap();
        assert_eq!(input, 100);
        assert_eq!(output, 50);
    }

    #[test]
    fn transport_serialization() {
        let acp = AgentChatTransport::Acp;
        let json = serde_json::to_string(&acp).unwrap();
        assert!(json.contains("\"type\":\"acp\""));

        let openai = AgentChatTransport::OpenAiChat {
            base_url: "http://localhost:11434/v1".to_owned(),
            api_key: None,
        };
        let json = serde_json::to_string(&openai).unwrap();
        assert!(json.contains("\"type\":\"open_ai_chat\""));
        assert!(json.contains("localhost:11434"));
    }

    #[test]
    fn transport_label_acp() {
        let session = AgentChatSession {
            id: "test-1".to_owned(),
            agent_kind: "copilot".to_owned(),
            workspace_path: PathBuf::from("/tmp"),
            session_name: "test-session".to_owned(),
            model_id: Some("copilot".to_owned()),
            transport: AgentChatTransport::Acp,
            event_tx: broadcast::channel::<AgentChatEvent>(16).0,
            messages: Vec::new(),
            pending_assistant_text: String::new(),
            pending_tool_calls: Vec::new(),
            status: AgentChatStatus::Idle,
            input_tokens: 0,
            output_tokens: 0,
            turn_start_input_tokens: 0,
            turn_start_output_tokens: 0,
            turn_cancel: None,
        };
        assert_eq!(session.transport_label(), "acp:copilot");
    }

    #[test]
    fn transport_label_openai() {
        let session = AgentChatSession {
            id: "test-2".to_owned(),
            agent_kind: "ollama".to_owned(),
            workspace_path: PathBuf::from("/tmp"),
            session_name: "test-session".to_owned(),
            model_id: Some("llama3:8b".to_owned()),
            transport: AgentChatTransport::OpenAiChat {
                base_url: "http://localhost:11434/v1".to_owned(),
                api_key: None,
            },
            event_tx: broadcast::channel::<AgentChatEvent>(16).0,
            messages: Vec::new(),
            pending_assistant_text: String::new(),
            pending_tool_calls: Vec::new(),
            status: AgentChatStatus::Idle,
            input_tokens: 0,
            output_tokens: 0,
            turn_start_input_tokens: 0,
            turn_start_output_tokens: 0,
            turn_cancel: None,
        };
        assert_eq!(
            session.transport_label(),
            "openai:http://localhost:11434/v1"
        );
    }

    #[test]
    fn create_session_sets_correct_agent_kind() {
        let mut manager = AgentChatManager::new();

        // Create a session with copilot agent kind
        let (session_id, _rx) = manager.create_session(
            "copilot".to_owned(),
            PathBuf::from("/tmp/test"),
            None,
            Some("copilot".to_owned()),
            None, // defaults to ACP
        );

        let sessions = manager.list();
        let session = sessions.iter().find(|s| s.id == session_id).unwrap();
        assert_eq!(session.agent_kind, "copilot");
        assert_eq!(session.transport_label, "acp:copilot");
    }

    #[test]
    fn create_session_with_openai_transport() {
        let mut manager = AgentChatManager::new();

        let (session_id, _rx) = manager.create_session(
            "ollama".to_owned(),
            PathBuf::from("/tmp/test"),
            None,
            Some("llama3:8b".to_owned()),
            Some(AgentChatTransport::OpenAiChat {
                base_url: "http://localhost:11434/v1".to_owned(),
                api_key: None,
            }),
        );

        let sessions = manager.list();
        let session = sessions.iter().find(|s| s.id == session_id).unwrap();
        assert_eq!(session.agent_kind, "ollama");
        assert_eq!(session.transport_label, "openai:http://localhost:11434/v1");
    }

    #[test]
    fn create_session_default_transport_is_acp() {
        let mut manager = AgentChatManager::new();

        let (session_id, _rx) = manager.create_session(
            "claude".to_owned(),
            PathBuf::from("/tmp/test"),
            None,
            None,
            None, // no transport → default ACP
        );

        let sessions = manager.list();
        let session = sessions.iter().find(|s| s.id == session_id).unwrap();
        assert_eq!(session.transport_label, "acp:claude");
    }

    #[test]
    fn switching_agent_creates_separate_sessions() {
        let mut manager = AgentChatManager::new();

        // First session: claude
        let (id1, _rx1) = manager.create_session(
            "claude".to_owned(),
            PathBuf::from("/tmp/test"),
            None,
            None,
            None,
        );

        // Second session: copilot (simulates what happens when user switches)
        let (id2, _rx2) = manager.create_session(
            "copilot".to_owned(),
            PathBuf::from("/tmp/test"),
            None,
            Some("copilot".to_owned()),
            None,
        );

        assert_ne!(id1, id2);

        let sessions = manager.list();
        let s1 = sessions.iter().find(|s| s.id == id1).unwrap();
        let s2 = sessions.iter().find(|s| s.id == id2).unwrap();

        assert_eq!(s1.agent_kind, "claude");
        assert_eq!(s1.transport_label, "acp:claude");
        assert_eq!(s2.agent_kind, "copilot");
        assert_eq!(s2.transport_label, "acp:copilot");
    }

    #[test]
    fn kill_and_remove_session() {
        let mut manager = AgentChatManager::new();

        let (session_id, _rx) = manager.create_session(
            "claude".to_owned(),
            PathBuf::from("/tmp/test"),
            None,
            None,
            None,
        );

        assert_eq!(manager.list().len(), 1);

        // Kill marks it exited
        manager.kill(&session_id).unwrap();
        let sessions = manager.list();
        assert_eq!(sessions[0].status, AgentChatStatus::Exited);

        // Remove deletes it
        manager.remove(&session_id);
        assert!(manager.list().is_empty());
    }

    #[test]
    fn session_snapshot_includes_transport_label() {
        let mut manager = AgentChatManager::new();

        let (session_id, _rx) = manager.create_session(
            "copilot".to_owned(),
            PathBuf::from("/tmp/test"),
            None,
            Some("copilot".to_owned()),
            None,
        );

        let (_rx, dto, _messages) = manager.subscribe(&session_id).unwrap();
        assert_eq!(dto.transport_label, "acp:copilot");
        assert_eq!(dto.agent_kind, "copilot");
    }
}
