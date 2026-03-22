use super::*;

struct AgentWsSessionEntry {
    session_id: String,
    cwd: String,
    state: AgentState,
    updated_at_unix_ms: Option<u64>,
}

fn legacy_agent_ws_session_id(cwd: &str) -> String {
    format!("legacy-cwd:{cwd}")
}

fn parse_agent_ws_session_entry(value: &serde_json::Value) -> Option<AgentWsSessionEntry> {
    let cwd = value.get("cwd")?.as_str()?;
    let session_id = match value.get("session_id").and_then(|v| v.as_str()) {
        Some(session_id) => session_id.to_owned(),
        None => {
            tracing::info!(
                cwd,
                "agent WS entry missing session_id, using legacy cwd fallback"
            );
            legacy_agent_ws_session_id(cwd)
        },
    };
    let state_str = value.get("state")?.as_str()?;
    let state = match state_str {
        "working" => AgentState::Working,
        "waiting" => AgentState::Waiting,
        _ => return None,
    };
    let updated_at = value.get("updated_at_unix_ms").and_then(|v| v.as_u64());
    Some(AgentWsSessionEntry {
        session_id,
        cwd: cwd.to_owned(),
        state,
        updated_at_unix_ms: updated_at,
    })
}

pub(crate) fn process_agent_ws_message(
    this: &gpui::WeakEntity<ArborWindow>,
    cx: &mut gpui::AsyncApp,
    text: &str,
) {
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(text);
    let Ok(value) = parsed else {
        tracing::warn!(raw = text, "agent WS: failed to parse message");
        return;
    };

    let msg_type = value.get("type").and_then(|v| v.as_str());
    match msg_type {
        Some("snapshot") => {
            let sessions = value
                .get("sessions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let entries: Vec<AgentWsSessionEntry> = sessions
                .iter()
                .filter_map(parse_agent_ws_session_entry)
                .collect();
            tracing::info!(count = entries.len(), "agent WS snapshot received");
            for entry in &entries {
                tracing::info!(
                    session_id = entry.session_id.as_str(),
                    cwd = entry.cwd.as_str(),
                    state = ?entry.state,
                    "  snapshot entry"
                );
            }
            let _ = this.update(cx, |this, cx| {
                if apply_agent_ws_snapshot(this, &entries, cx) {
                    cx.notify();
                }
            });
        },
        Some("update") => {
            if let Some(session) = value.get("session")
                && let Some(entry) = parse_agent_ws_session_entry(session)
            {
                tracing::info!(
                    session_id = entry.session_id.as_str(),
                    cwd = entry.cwd.as_str(),
                    state = ?entry.state,
                    "agent WS update received"
                );
                let entries = vec![entry];
                let _ = this.update(cx, |this, cx| {
                    if apply_agent_ws_update(this, &entries, cx) {
                        cx.notify();
                    }
                });
            }
        },
        Some("clear") => {
            if let Some(session_id) = value.get("session_id").and_then(|v| v.as_str()) {
                tracing::info!(session_id, "agent WS clear received");
                let session_id = session_id.to_owned();
                let _ = this.update(cx, |this, cx| {
                    if apply_agent_ws_clear(this, &session_id, cx) {
                        cx.notify();
                    }
                });
            }
        },
        _ => {},
    }
}

fn apply_agent_ws_snapshot(
    app: &mut ArborWindow,
    entries: &[AgentWsSessionEntry],
    cx: &mut Context<ArborWindow>,
) -> bool {
    tracing::info!(count = entries.len(), "agent WS snapshot received");
    app.agent_activity_sessions = entries
        .iter()
        .map(|entry| {
            (entry.session_id.clone(), AgentActivitySessionRecord {
                cwd: entry.cwd.clone(),
                state: entry.state,
                updated_at_unix_ms: entry.updated_at_unix_ms,
            })
        })
        .collect();
    reconcile_worktree_agent_activity(app, false, cx)
}

fn apply_agent_ws_update(
    app: &mut ArborWindow,
    entries: &[AgentWsSessionEntry],
    cx: &mut Context<ArborWindow>,
) -> bool {
    for entry in entries {
        app.agent_activity_sessions
            .insert(entry.session_id.clone(), AgentActivitySessionRecord {
                cwd: entry.cwd.clone(),
                state: entry.state,
                updated_at_unix_ms: entry.updated_at_unix_ms,
            });
    }
    reconcile_worktree_agent_activity(app, true, cx)
}

fn apply_agent_ws_clear(
    app: &mut ArborWindow,
    session_id: &str,
    cx: &mut Context<ArborWindow>,
) -> bool {
    remove_agent_activity_session(&mut app.agent_activity_sessions, session_id);
    reconcile_worktree_agent_activity(app, false, cx)
}

pub(crate) fn reconcile_worktree_agent_activity(
    app: &mut ArborWindow,
    allow_waiting_transitions: bool,
    cx: &mut Context<ArborWindow>,
) -> bool {
    let worktree_paths: Vec<PathBuf> = app.worktrees.iter().map(|w| w.path.clone()).collect();
    let allow_auto_checkpoint = app.active_outpost_index.is_none();
    let mut derived_states = HashMap::<PathBuf, (AgentState, Option<u64>)>::new();
    let mut visible_changed = false;

    for (session_id, session) in &app.agent_activity_sessions {
        let cwd_path = Path::new(&session.cwd);
        let best_match = worktree_paths
            .iter()
            .filter(|wt_path| cwd_path.starts_with(wt_path))
            .max_by_key(|wt_path| wt_path.as_os_str().len());

        let Some(matched_path) = best_match else {
            tracing::warn!(
                session_id = session_id.as_str(),
                cwd = session.cwd.as_str(),
                state = ?session.state,
                "agent activity did not match any worktree"
            );
            continue;
        };

        tracing::info!(
            session_id = session_id.as_str(),
            cwd = session.cwd.as_str(),
            worktree = %matched_path.display(),
            state = ?session.state,
            "agent activity matched to worktree"
        );

        let entry = derived_states
            .entry(matched_path.clone())
            .or_insert((session.state, session.updated_at_unix_ms));
        merge_agent_activity_state(entry, session.state, session.updated_at_unix_ms);
    }

    let mut waiting_transitions = Vec::new();
    for worktree in &mut app.worktrees {
        let previous_state = worktree.agent_state;
        let previous_recent = worktree_recent_activity_visible(worktree.last_activity_unix_ms);
        let (next_state, updated_at) = derived_states
            .remove(&worktree.path)
            .map(|(state, updated_at)| (Some(state), updated_at))
            .unwrap_or((None, None));

        let transition_epoch = if previous_state != next_state {
            Some(advance_agent_activity_epoch(
                app.agent_activity_epochs.as_ref(),
                &worktree.path,
            ))
        } else {
            None
        };

        worktree.agent_state = next_state;
        if let Some(ts) = updated_at {
            worktree.last_activity_unix_ms =
                Some(worktree.last_activity_unix_ms.unwrap_or(0).max(ts));
        }
        let next_recent = worktree_recent_activity_visible(worktree.last_activity_unix_ms);
        visible_changed |= previous_state != next_state
            || (previous_state.is_none() && next_state.is_none() && previous_recent != next_recent);

        if allow_waiting_transitions
            && agent_waiting_transition_detected(previous_state, next_state)
            && let Some(epoch) = transition_epoch
        {
            waiting_transitions.push(AgentWaitingTransitionRequest {
                path: worktree.path.clone(),
                repo_root: worktree.repo_root.clone(),
                agent_task: worktree.agent_task.clone(),
                updated_at,
                epoch,
                allow_auto_checkpoint,
            });
        }
    }

    for request in waiting_transitions {
        app.spawn_agent_waiting_transition(request, cx);
    }

    visible_changed
}

fn worktree_recent_activity_visible(last_activity_unix_ms: Option<u64>) -> bool {
    last_activity_unix_ms.is_some_and(|timestamp| {
        current_unix_timestamp_millis()
            .unwrap_or(0)
            .saturating_sub(timestamp)
            <= 15 * 60 * 1000
    })
}

pub(crate) fn agent_waiting_transition_detected(
    previous_state: Option<AgentState>,
    next_state: Option<AgentState>,
) -> bool {
    previous_state == Some(AgentState::Working) && next_state == Some(AgentState::Waiting)
}

pub(crate) fn merge_agent_activity_state(
    entry: &mut (AgentState, Option<u64>),
    state: AgentState,
    updated_at_unix_ms: Option<u64>,
) {
    entry.0 = merge_agent_activity_status(entry.0, state);
    entry.1 = merge_agent_activity_timestamp(entry.1, updated_at_unix_ms);
}

pub(crate) fn merge_agent_activity_status(current: AgentState, next: AgentState) -> AgentState {
    if current == AgentState::Working || next == AgentState::Working {
        AgentState::Working
    } else {
        AgentState::Waiting
    }
}

pub(crate) fn merge_agent_activity_timestamp(
    current: Option<u64>,
    next: Option<u64>,
) -> Option<u64> {
    match (current, next) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}

pub(crate) fn remove_agent_activity_session(
    sessions: &mut HashMap<String, AgentActivitySessionRecord>,
    session_id: &str,
) {
    sessions.remove(session_id);
}

#[derive(Clone)]
pub(crate) struct AgentWaitingTransitionRequest {
    pub(crate) path: PathBuf,
    pub(crate) repo_root: PathBuf,
    pub(crate) agent_task: Option<String>,
    pub(crate) updated_at: Option<u64>,
    pub(crate) epoch: u64,
    pub(crate) allow_auto_checkpoint: bool,
}

pub(crate) struct AgentWaitingTransitionOutcome {
    pub(crate) path: PathBuf,
    pub(crate) updated_at: Option<u64>,
    pub(crate) epoch: u64,
    pub(crate) diff_summary: Option<changes::DiffLineSummary>,
    pub(crate) notifications_allowed: bool,
    pub(crate) auto_checkpoint: Option<AgentAutoCheckpointResult>,
}

pub(crate) struct AgentAutoCheckpointResult {
    pub(crate) notice: Option<String>,
    pub(crate) committed: bool,
    pub(crate) diff_summary: Option<changes::DiffLineSummary>,
    pub(crate) branch_divergence: Option<BranchDivergenceSummary>,
}

pub(crate) fn evaluate_agent_waiting_transition(
    request: AgentWaitingTransitionRequest,
    app_config_store: Arc<dyn app_config::AppConfigStore>,
    auto_checkpoint_in_flight: Arc<Mutex<HashSet<PathBuf>>>,
    agent_activity_epochs: Arc<Mutex<HashMap<PathBuf, u64>>>,
) -> AgentWaitingTransitionOutcome {
    let repo_config = app_config_store.load_repo_config(&request.repo_root);
    let notifications_allowed =
        repo_notifications_allow_event(repo_config.as_ref(), "agent_finished");
    let diff_summary = changes::diff_line_summary(&request.path).ok();
    let auto_checkpoint_enabled = request.allow_auto_checkpoint
        && repo_config
            .as_ref()
            .and_then(|config| config.agent.auto_checkpoint)
            .unwrap_or(false);
    let auto_checkpoint = auto_checkpoint_enabled.then(|| {
        run_agent_auto_checkpoint(
            &request,
            auto_checkpoint_in_flight.as_ref(),
            agent_activity_epochs.as_ref(),
        )
    });

    AgentWaitingTransitionOutcome {
        path: request.path,
        updated_at: request.updated_at,
        epoch: request.epoch,
        diff_summary,
        notifications_allowed,
        auto_checkpoint: auto_checkpoint.flatten(),
    }
}

fn run_agent_auto_checkpoint(
    request: &AgentWaitingTransitionRequest,
    auto_checkpoint_in_flight: &Mutex<HashSet<PathBuf>>,
    agent_activity_epochs: &Mutex<HashMap<PathBuf, u64>>,
) -> Option<AgentAutoCheckpointResult> {
    if !agent_activity_epoch_is_current(agent_activity_epochs, &request.path, request.epoch) {
        return None;
    }

    let inserted = {
        let mut in_flight = lock_mutex(auto_checkpoint_in_flight);
        in_flight.insert(request.path.clone())
    };
    if !inserted {
        return None;
    }

    let result = run_agent_auto_checkpoint_inner(request, agent_activity_epochs);
    let mut in_flight = lock_mutex(auto_checkpoint_in_flight);
    in_flight.remove(&request.path);
    result
}

fn run_agent_auto_checkpoint_inner(
    request: &AgentWaitingTransitionRequest,
    agent_activity_epochs: &Mutex<HashMap<PathBuf, u64>>,
) -> Option<AgentAutoCheckpointResult> {
    let changed_files = match changes::changed_files(&request.path) {
        Ok(files) => files,
        Err(error) => {
            return Some(AgentAutoCheckpointResult {
                notice: Some(format!(
                    "failed to inspect auto-checkpoint changes: {error}"
                )),
                committed: false,
                diff_summary: None,
                branch_divergence: None,
            });
        },
    };
    if changed_files.is_empty()
        || !agent_activity_epoch_is_current(agent_activity_epochs, &request.path, request.epoch)
    {
        return None;
    }

    let message = auto_checkpoint_commit_message(&changed_files, request.agent_task.as_deref());
    match run_git_commit_for_worktree(&request.path, &changed_files, &message) {
        Ok(summary) => Some(AgentAutoCheckpointResult {
            notice: Some(summary),
            committed: true,
            diff_summary: changes::diff_line_summary(&request.path).ok(),
            branch_divergence: branch_divergence_summary(&request.path),
        }),
        Err(GitError::Operation(ref msg)) if msg == "nothing to commit" => None,
        Err(error) => Some(AgentAutoCheckpointResult {
            notice: Some(format!("auto-checkpoint failed: {error}")),
            committed: false,
            diff_summary: None,
            branch_divergence: None,
        }),
    }
}

pub(crate) fn repo_notifications_allow_event(
    config: Option<&app_config::RepoConfig>,
    event_name: &str,
) -> bool {
    let Some(config) = config else {
        return true;
    };

    if config.notifications.desktop == Some(false) {
        return false;
    }

    config.notifications.events.is_empty()
        || config
            .notifications
            .events
            .iter()
            .any(|event| event == event_name)
}

pub(crate) fn lock_mutex<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn advance_agent_activity_epoch(
    epochs: &Mutex<HashMap<PathBuf, u64>>,
    path: &Path,
) -> u64 {
    let mut epochs = lock_mutex(epochs);
    let next = epochs.get(path).copied().unwrap_or(0).saturating_add(1);
    epochs.insert(path.to_path_buf(), next);
    next
}

pub(crate) fn agent_activity_epoch_is_current(
    epochs: &Mutex<HashMap<PathBuf, u64>>,
    path: &Path,
    epoch: u64,
) -> bool {
    lock_mutex(epochs).get(path).copied().unwrap_or(0) == epoch
}

pub(crate) fn inject_daemon_log_entry(log_buffer: &log_layer::LogBuffer, text: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let level = match value
        .get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("INFO")
    {
        "ERROR" => tracing::Level::ERROR,
        "WARN" => tracing::Level::WARN,
        "DEBUG" => tracing::Level::DEBUG,
        "TRACE" => tracing::Level::TRACE,
        _ => tracing::Level::INFO,
    };
    let target = value
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("arbor_httpd");
    let message = value
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let fields_str = value.get("fields").and_then(|v| v.as_str()).unwrap_or("");
    let ts_ms = value.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
    let timestamp = SystemTime::UNIX_EPOCH + Duration::from_millis(ts_ms);

    let mut fields = Vec::new();
    if !fields_str.is_empty() {
        for part in fields_str.split(' ') {
            if let Some((k, v)) = part.split_once('=') {
                fields.push((k.to_owned(), v.to_owned()));
            }
        }
    }

    log_buffer.push(log_layer::LogEntry {
        timestamp,
        level,
        target: format!("[daemon] {target}"),
        message,
        fields,
    });
}

pub(crate) fn should_emit_agent_finished_notification(
    notifications: &mut HashMap<PathBuf, u64>,
    worktree_path: &Path,
    updated_at: Option<u64>,
) -> bool {
    let notification_timestamp = updated_at.unwrap_or_default();
    if notifications
        .get(worktree_path)
        .copied()
        .is_some_and(|previous| previous >= notification_timestamp)
    {
        return false;
    }

    notifications.insert(worktree_path.to_path_buf(), notification_timestamp);
    true
}

pub(crate) fn install_claude_code_hooks(daemon_base_url: &str) -> Result<(), StoreError> {
    let home = env::var("HOME").map_err(|_| StoreError::Other("HOME not set".to_owned()))?;
    let claude_dir = PathBuf::from(&home).join(".claude");
    let settings_path = claude_dir.join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path).map_err(|source| StoreError::Read {
            path: settings_path.display().to_string(),
            source,
        })?;
        serde_json::from_str(&content).map_err(|source| StoreError::JsonParse {
            path: settings_path.display().to_string(),
            source,
        })?
    } else {
        if !claude_dir.exists() {
            fs::create_dir_all(&claude_dir).map_err(|source| StoreError::CreateDir {
                path: claude_dir.display().to_string(),
                source,
            })?;
        }
        serde_json::json!({})
    };

    let notify_url = format!("{daemon_base_url}/api/v1/agent/notify");

    // Check if our hooks are already present
    if let Some(hooks) = settings.get("hooks") {
        let hooks_str = hooks.to_string();
        if hooks_str.contains("/api/v1/agent/notify") {
            tracing::debug!("Claude Code hooks already installed");
            return Ok(());
        }
    }

    let hook_entry = serde_json::json!([
        {
            "matcher": "",
            "hooks": [
                {
                    "type": "http",
                    "url": notify_url,
                    "timeout": 2
                }
            ]
        }
    ]);

    let hooks = settings
        .as_object_mut()
        .ok_or_else(|| StoreError::Other("settings.json is not an object".to_owned()))?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_obj = hooks
        .as_object_mut()
        .ok_or_else(|| StoreError::Other("hooks is not an object".to_owned()))?;

    if !hooks_obj.contains_key("UserPromptSubmit") {
        hooks_obj.insert("UserPromptSubmit".to_owned(), hook_entry.clone());
    }
    if !hooks_obj.contains_key("Stop") {
        hooks_obj.insert("Stop".to_owned(), hook_entry);
    }

    let serialized =
        serde_json::to_string_pretty(&settings).map_err(|source| StoreError::JsonSerialize {
            path: settings_path.display().to_string(),
            source,
        })?;
    fs::write(&settings_path, serialized).map_err(|source| StoreError::Write {
        path: settings_path.display().to_string(),
        source,
    })?;

    tracing::info!(path = %settings_path.display(), "installed Claude Code hooks");
    Ok(())
}

const PI_AGENT_EXTENSION_FILENAME: &str = "arbor-activity.ts";
const PI_AGENT_EXTENSION_MARKER: &str = "Managed by Arbor: Pi activity bridge";

pub(crate) fn install_pi_agent_extension(daemon_base_url: &str) -> Result<(), StoreError> {
    let home = env::var("HOME").map_err(|_| StoreError::Other("HOME not set".to_owned()))?;
    let extensions_dir = PathBuf::from(&home)
        .join(".pi")
        .join("agent")
        .join("extensions");
    fs::create_dir_all(&extensions_dir).map_err(|source| StoreError::CreateDir {
        path: extensions_dir.display().to_string(),
        source,
    })?;

    let extension_path = extensions_dir.join(PI_AGENT_EXTENSION_FILENAME);
    let next_content = render_pi_agent_extension(daemon_base_url);

    if extension_path.exists() {
        let existing = fs::read_to_string(&extension_path).map_err(|source| StoreError::Read {
            path: extension_path.display().to_string(),
            source,
        })?;
        if !existing.contains(PI_AGENT_EXTENSION_MARKER) {
            return Err(StoreError::Other(format!(
                "refusing to overwrite existing Pi extension `{}`",
                extension_path.display()
            )));
        }
        if existing == next_content {
            tracing::debug!("Pi activity extension already installed");
            return Ok(());
        }
    }

    fs::write(&extension_path, next_content).map_err(|source| StoreError::Write {
        path: extension_path.display().to_string(),
        source,
    })?;
    tracing::info!(path = %extension_path.display(), "installed Pi activity extension");
    Ok(())
}

pub(crate) fn render_pi_agent_extension(daemon_base_url: &str) -> String {
    let notify_url = format!("{daemon_base_url}/api/v1/agent/notify");
    format!(
        r#"// {PI_AGENT_EXTENSION_MARKER}
import type {{ ExtensionAPI }} from "@mariozechner/pi-coding-agent";

const NOTIFY_URL = {notify_url:?};

async function notify(hookEventName: "UserPromptSubmit" | "Stop", sessionId: string, cwd: string) {{
  try {{
    await fetch(NOTIFY_URL, {{
      method: "POST",
      headers: {{ "content-type": "application/json" }},
      body: JSON.stringify({{
        hook_event_name: hookEventName,
        session_id: sessionId,
        cwd,
      }}),
    }});
  }} catch {{
    // Ignore daemon reachability errors.
  }}
}}

export default function (pi: ExtensionAPI) {{
  pi.on("before_agent_start", async (_event, ctx) => {{
    await notify("UserPromptSubmit", ctx.sessionManager.getSessionId(), ctx.cwd);
  }});

  pi.on("agent_end", async (_event, ctx) => {{
    await notify("Stop", ctx.sessionManager.getSessionId(), ctx.cwd);
  }});
}}
"#
    )
}

pub(crate) fn remove_pi_agent_extension() {
    let Ok(home) = env::var("HOME") else {
        return;
    };
    let extension_path = PathBuf::from(&home)
        .join(".pi")
        .join("agent")
        .join("extensions")
        .join(PI_AGENT_EXTENSION_FILENAME);
    if !extension_path.exists() {
        return;
    }

    let Ok(content) = fs::read_to_string(&extension_path) else {
        return;
    };
    if !content.contains(PI_AGENT_EXTENSION_MARKER) {
        return;
    }

    match fs::remove_file(&extension_path) {
        Ok(()) => tracing::info!(path = %extension_path.display(), "removed Pi activity extension"),
        Err(error) => {
            tracing::warn!(path = %extension_path.display(), %error, "failed to remove Pi activity extension")
        },
    }
}

pub(crate) fn remove_claude_code_hooks() {
    let Ok(home) = env::var("HOME") else {
        return;
    };
    let settings_path = PathBuf::from(&home).join(".claude").join("settings.json");
    if !settings_path.exists() {
        return;
    }

    let Ok(content) = fs::read_to_string(&settings_path) else {
        return;
    };
    let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };

    let Some(hooks) = settings.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return;
    };

    // Check if any hook references our notify endpoint
    if !hooks
        .values()
        .any(|v| v.to_string().contains("/api/v1/agent/notify"))
    {
        return;
    }

    // Remove entries containing our notify URL from each hook array
    let hook_keys: Vec<String> = hooks.keys().cloned().collect();
    for key in hook_keys {
        if let Some(arr) = hooks.get_mut(&key).and_then(|v| v.as_array_mut()) {
            arr.retain(|entry| !entry.to_string().contains("/api/v1/agent/notify"));
            if arr.is_empty() {
                hooks.remove(&key);
            }
        }
    }

    if hooks.is_empty()
        && let Some(obj) = settings.as_object_mut()
    {
        obj.remove("hooks");
    }

    match serde_json::to_string_pretty(&settings) {
        Ok(serialized) => {
            if let Err(e) = fs::write(&settings_path, serialized) {
                tracing::warn!(error = %e, "failed to write settings.json during hook removal");
            } else {
                tracing::info!(path = %settings_path.display(), "removed Claude Code hooks");
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize settings during hook removal");
        },
    }
}

impl ArborWindow {
    pub(crate) fn spawn_agent_waiting_transition(
        &mut self,
        request: AgentWaitingTransitionRequest,
        cx: &mut Context<Self>,
    ) {
        let app_config_store = self.app_config_store.clone();
        let auto_checkpoint_in_flight = Arc::clone(&self.auto_checkpoint_in_flight);
        let agent_activity_epochs = Arc::clone(&self.agent_activity_epochs);

        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    evaluate_agent_waiting_transition(
                        request,
                        app_config_store,
                        auto_checkpoint_in_flight,
                        agent_activity_epochs,
                    )
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                this.apply_agent_waiting_transition_result(outcome, cx);
            });
        })
        .detach();
    }

    pub(crate) fn apply_agent_waiting_transition_result(
        &mut self,
        outcome: AgentWaitingTransitionOutcome,
        cx: &mut Context<Self>,
    ) {
        let AgentWaitingTransitionOutcome {
            path,
            epoch,
            updated_at,
            diff_summary,
            notifications_allowed,
            auto_checkpoint,
        } = outcome;

        if !agent_activity_epoch_is_current(self.agent_activity_epochs.as_ref(), &path, epoch) {
            return;
        }

        let mut notification_worktree = None;
        if let Some(worktree) = self
            .worktrees
            .iter_mut()
            .find(|candidate| candidate.path == path)
        {
            if worktree.agent_state != Some(AgentState::Waiting) {
                return;
            }

            let next_snapshot = AgentTurnSnapshot {
                timestamp_unix_ms: updated_at.or(worktree.last_activity_unix_ms),
                diff_summary,
            };

            if worktree
                .recent_turns
                .first()
                .is_some_and(|previous| previous.diff_summary == next_snapshot.diff_summary)
            {
                worktree.stuck_turn_count += 1;
            } else {
                worktree.stuck_turn_count = 0;
            }

            worktree.recent_turns.insert(0, next_snapshot);
            worktree.recent_turns.truncate(5);

            if let Some(auto_checkpoint) = auto_checkpoint.as_ref()
                && auto_checkpoint.committed
            {
                worktree.diff_summary = auto_checkpoint.diff_summary;
                worktree.branch_divergence = auto_checkpoint.branch_divergence;
            }

            if notifications_allowed {
                notification_worktree = Some(worktree.clone());
            }
        } else {
            return;
        }

        if let Some(worktree) = notification_worktree.as_ref() {
            self.maybe_notify_agent_finished(worktree, updated_at);
        }

        if let Some(auto_checkpoint) = auto_checkpoint {
            if auto_checkpoint.committed
                && self
                    .selected_local_worktree_path()
                    .is_some_and(|selected| selected == path.as_path())
            {
                self.changed_files.clear();
                self.selected_changed_file = None;
                self.refresh_changed_files(cx);
            }

            if let Some(notice) = auto_checkpoint.notice {
                self.notice = Some(notice);
            }
        }

        cx.notify();
    }

    pub(crate) fn start_agent_activity_ws(&mut self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        let daemon = self.terminal_daemon.clone();
        cx.spawn(async move |this, cx| {
            let mut backoff_secs = 3u64;

            loop {
                let connect_config = daemon
                    .as_ref()
                    .and_then(|daemon| {
                        daemon
                            .websocket_connect_config("/api/v1/agent/activity/ws")
                            .ok()
                    })
                    .or_else(|| {
                        daemon_url_is_local(&daemon_base_url).then(|| {
                            terminal_daemon_http::WebsocketConnectConfig {
                                url: daemon_base_url
                                    .replace("http://", "ws://")
                                    .replace("https://", "wss://")
                                    + "/api/v1/agent/activity/ws",
                                auth_token: None,
                            }
                        })
                    });
                let (tx, rx) = smol::channel::unbounded::<Option<String>>();

                cx.background_spawn(async move {
                    let Some(connect_config) = connect_config else {
                        let _ = tx.send(None).await;
                        return;
                    };
                    let request = match daemon_websocket_request(&connect_config) {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::debug!(%error, "failed to build agent activity websocket request");
                            let _ = tx.send(None).await;
                            return;
                        },
                    };

                    let Ok((mut ws, _)) = tungstenite::connect(request) else {
                        let _ = tx.send(None).await;
                        return;
                    };
                    loop {
                        match ws.read() {
                            Ok(tungstenite::Message::Text(text)) => {
                                if tx.send(Some(text.to_string())).await.is_err() {
                                    break;
                                }
                            },
                            Ok(tungstenite::Message::Ping(_))
                            | Ok(tungstenite::Message::Pong(_)) => {},
                            Ok(tungstenite::Message::Close(_)) | Err(_) => {
                                let _ = tx.send(None).await;
                                break;
                            },
                            _ => {},
                        }
                    }
                })
                .detach();

                let first = rx.recv().await;
                if let Ok(Some(text)) = first {
                    tracing::info!("agent activity WS connected");
                    backoff_secs = 3;
                    // Process the first message
                    process_agent_ws_message(&this, cx, &text);

                    // Process subsequent messages
                    while let Ok(Some(text)) = rx.recv().await {
                        process_agent_ws_message(&this, cx, &text);
                    }
                }

                tracing::debug!("agent activity WS disconnected, will retry");

                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_secs(backoff_secs));
                })
                .await;
                backoff_secs = (backoff_secs * 2).min(30);
            }
        })
        .detach();
    }

    pub(crate) fn start_daemon_log_ws(&mut self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        let daemon = self.terminal_daemon.clone();
        let log_buffer = self.log_buffer.clone();
        cx.spawn(async move |_this, cx| {
            let mut backoff_secs = 3u64;

            loop {
                let connect_config = daemon
                    .as_ref()
                    .and_then(|daemon| daemon.websocket_connect_config("/api/v1/logs/ws").ok())
                    .or_else(|| {
                        daemon_url_is_local(&daemon_base_url).then(|| {
                            terminal_daemon_http::WebsocketConnectConfig {
                                url: daemon_base_url
                                    .replace("http://", "ws://")
                                    .replace("https://", "wss://")
                                    + "/api/v1/logs/ws",
                                auth_token: None,
                            }
                        })
                    });
                let (tx, rx) = smol::channel::unbounded::<Option<String>>();

                cx.background_spawn(async move {
                    let Some(connect_config) = connect_config else {
                        let _ = tx.send(None).await;
                        return;
                    };
                    let request = match daemon_websocket_request(&connect_config) {
                        Ok(request) => request,
                        Err(_) => {
                            let _ = tx.send(None).await;
                            return;
                        },
                    };

                    let Ok((mut ws, _)) = tungstenite::connect(request) else {
                        let _ = tx.send(None).await;
                        return;
                    };
                    loop {
                        match ws.read() {
                            Ok(tungstenite::Message::Text(text)) => {
                                if tx.send(Some(text.to_string())).await.is_err() {
                                    break;
                                }
                            },
                            Ok(tungstenite::Message::Ping(_))
                            | Ok(tungstenite::Message::Pong(_)) => {},
                            Ok(tungstenite::Message::Close(_)) | Err(_) => {
                                let _ = tx.send(None).await;
                                break;
                            },
                            _ => {},
                        }
                    }
                })
                .detach();

                let first = rx.recv().await;
                if let Ok(Some(text)) = first {
                    tracing::info!("daemon log WS connected");
                    backoff_secs = 3;
                    inject_daemon_log_entry(&log_buffer, &text);

                    while let Ok(Some(text)) = rx.recv().await {
                        inject_daemon_log_entry(&log_buffer, &text);
                    }
                }

                tracing::debug!("daemon log WS disconnected, will retry");

                cx.background_spawn(async move {
                    std::thread::sleep(Duration::from_secs(backoff_secs));
                })
                .await;
                backoff_secs = (backoff_secs * 2).min(30);
            }
        })
        .detach();
    }

    pub(crate) fn ensure_claude_code_hooks(&self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        cx.spawn(async move |_this, cx| {
            cx.background_spawn(async move {
                if let Err(error) = install_claude_code_hooks(&daemon_base_url) {
                    tracing::warn!(%error, "failed to install Claude Code hooks");
                }
            })
            .await;
        })
        .detach();
    }

    pub(crate) fn ensure_pi_agent_extension(&self, cx: &mut Context<Self>) {
        let daemon_base_url = self.daemon_base_url.clone();
        cx.spawn(async move |_this, cx| {
            cx.background_spawn(async move {
                if let Err(error) = install_pi_agent_extension(&daemon_base_url) {
                    tracing::warn!(%error, "failed to install Pi activity extension");
                }
            })
            .await;
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unwrap_used, clippy::expect_used)]
    mod agent_activity_tests {
        use super::*;

        #[test]
        fn agent_waiting_transition_requires_prior_working_state() {
            assert!(agent_waiting_transition_detected(
                Some(AgentState::Working),
                Some(AgentState::Waiting),
            ));
            assert!(!agent_waiting_transition_detected(
                None,
                Some(AgentState::Waiting),
            ));
            assert!(!agent_waiting_transition_detected(
                Some(AgentState::Waiting),
                Some(AgentState::Waiting),
            ));
        }

        #[test]
        fn merge_agent_activity_state_keeps_working_and_latest_timestamp() {
            let mut merged = (AgentState::Working, Some(200));
            merge_agent_activity_state(&mut merged, AgentState::Waiting, Some(100));
            assert_eq!(merged, (AgentState::Working, Some(200)));

            let mut merged = (AgentState::Waiting, Some(100));
            merge_agent_activity_state(&mut merged, AgentState::Working, Some(200));
            assert_eq!(merged, (AgentState::Working, Some(200)));
        }

        #[test]
        fn merge_agent_activity_state_waiting_only_when_no_session_is_working() {
            let mut merged = (AgentState::Waiting, Some(100));
            merge_agent_activity_state(&mut merged, AgentState::Waiting, Some(200));
            assert_eq!(merged, (AgentState::Waiting, Some(200)));
        }

        #[test]
        fn parse_agent_ws_session_entry_uses_legacy_cwd_id_when_missing() {
            let entry = parse_agent_ws_session_entry(&serde_json::json!({
                "cwd": "/tmp/repo/worktree",
                "state": "working",
                "updated_at_unix_ms": 42_u64,
            }))
            .expect("expected agent ws entry");

            assert_eq!(entry.session_id, "legacy-cwd:/tmp/repo/worktree");
            assert_eq!(entry.cwd, "/tmp/repo/worktree");
            assert_eq!(entry.state, AgentState::Working);
            assert_eq!(entry.updated_at_unix_ms, Some(42));
        }

        #[test]
        fn parse_agent_ws_session_entry_preserves_explicit_session_id() {
            let entry = parse_agent_ws_session_entry(&serde_json::json!({
                "session_id": "terminal:daemon-1",
                "cwd": "/tmp/repo/worktree",
                "state": "waiting",
                "updated_at_unix_ms": 99_u64,
            }))
            .expect("expected agent ws entry");

            assert_eq!(entry.session_id, "terminal:daemon-1");
            assert_eq!(entry.cwd, "/tmp/repo/worktree");
            assert_eq!(entry.state, AgentState::Waiting);
            assert_eq!(entry.updated_at_unix_ms, Some(99));
        }

        #[test]
        fn apply_agent_ws_clear_removes_matching_session() {
            let mut sessions = HashMap::from([
                ("terminal:daemon-1".to_owned(), AgentActivitySessionRecord {
                    cwd: "/tmp/repo/worktree".to_owned(),
                    state: AgentState::Waiting,
                    updated_at_unix_ms: Some(42),
                }),
                ("terminal:daemon-2".to_owned(), AgentActivitySessionRecord {
                    cwd: "/tmp/repo/worktree".to_owned(),
                    state: AgentState::Working,
                    updated_at_unix_ms: Some(99),
                }),
            ]);

            remove_agent_activity_session(&mut sessions, "terminal:daemon-1");

            assert!(!sessions.contains_key("terminal:daemon-1"));
            assert!(sessions.contains_key("terminal:daemon-2"));
        }

        #[test]
        fn agent_finished_notifications_are_deduped_by_timestamp() {
            let path = Path::new("/tmp/repo/worktree");
            let mut notifications = HashMap::new();

            assert!(should_emit_agent_finished_notification(
                &mut notifications,
                path,
                Some(10),
            ));
            assert!(!should_emit_agent_finished_notification(
                &mut notifications,
                path,
                Some(10),
            ));
            assert!(!should_emit_agent_finished_notification(
                &mut notifications,
                path,
                Some(9),
            ));
            assert!(should_emit_agent_finished_notification(
                &mut notifications,
                path,
                Some(11),
            ));
        }

        #[test]
        fn agent_activity_epoch_advances_and_invalidates_previous_work() {
            let epochs = Mutex::new(HashMap::new());
            let path = Path::new("/tmp/repo/worktree");

            let first = advance_agent_activity_epoch(&epochs, path);
            let second = advance_agent_activity_epoch(&epochs, path);

            assert_eq!(first, 1);
            assert_eq!(second, 2);
            assert!(!agent_activity_epoch_is_current(&epochs, path, first));
            assert!(agent_activity_epoch_is_current(&epochs, path, second));
        }

        #[test]
        fn repo_notifications_allow_event_honors_filters() {
            let mut config = app_config::RepoConfig::default();
            assert!(repo_notifications_allow_event(
                Some(&config),
                "agent_finished"
            ));

            config.notifications.desktop = Some(false);
            assert!(!repo_notifications_allow_event(
                Some(&config),
                "agent_finished",
            ));

            config.notifications.desktop = Some(true);
            config.notifications.events = vec!["build_finished".to_owned()];
            assert!(!repo_notifications_allow_event(
                Some(&config),
                "agent_finished",
            ));

            config
                .notifications
                .events
                .push("agent_finished".to_owned());
            assert!(repo_notifications_allow_event(
                Some(&config),
                "agent_finished",
            ));
        }
    }
}
