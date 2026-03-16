#[cfg(feature = "agent-chat")]
use crate::agent_chat;
#[cfg(feature = "symphony")]
use arbor_symphony::{ServiceOptions, SymphonyService};
use {
    crate::{
        auth, github_service, issue_provider,
        process_manager::ProcessManager,
        repository_store,
        routes::spawn_notification_webhooks,
        task_scheduler::{self, TaskScheduler},
        terminal_daemon::LocalTerminalDaemon,
        types::*,
    },
    arbor_core::{daemon::JsonDaemonSessionStore, process::ProcessStatus},
    std::{collections::HashMap, env, net::SocketAddr, path::PathBuf, sync::Arc},
    tokio::sync::Mutex,
};

pub(crate) fn configure_embedded_terminal_engine() {
    let requested = env::var("ARBOR_TERMINAL_ENGINE")
        .ok()
        .or_else(load_embedded_terminal_engine_setting);
    match arbor_terminal_emulator::parse_terminal_engine_kind(requested.as_deref()) {
        Ok(engine) => arbor_terminal_emulator::set_default_terminal_engine(engine),
        Err(error) => {
            tracing::warn!(%error, "invalid embedded terminal engine configuration");
            arbor_terminal_emulator::set_default_terminal_engine(
                arbor_terminal_emulator::TerminalEngineKind::default(),
            );
        },
    }
}

#[cfg(feature = "symphony")]
pub(crate) async fn start_symphony_if_configured() -> Option<arbor_symphony::ServiceHandle> {
    let workflow_path = env::var("ARBOR_SYMPHONY_WORKFLOW")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            env::current_dir()
                .ok()
                .map(|cwd| cwd.join("WORKFLOW.md"))
                .filter(|path| path.exists())
        });

    let Some(workflow_path) = workflow_path else {
        tracing::info!("symphony workflow not found; service disabled");
        return None;
    };

    match SymphonyService::start(ServiceOptions {
        workflow_path: Some(workflow_path.clone()),
        ..ServiceOptions::default()
    })
    .await
    {
        Ok(handle) => {
            tracing::info!(path = %workflow_path.display(), "symphony service started");
            Some(handle)
        },
        Err(error) => {
            tracing::error!(%error, path = %workflow_path.display(), "failed to start symphony service");
            None
        },
    }
}

/// Build the [`AppState`] and spawn all background tasks.
///
/// Returns the fully-initialised state, the [`auth::AuthState`] needed by the
/// auth middleware, whether auth is configured, and the resolved bind address.
pub(crate) async fn build_app_state(
    log_broadcast: tokio::sync::broadcast::Sender<String>,
) -> Result<(AppState, auth::AuthState, bool, SocketAddr), Box<dyn std::error::Error>> {
    let mut daemon_config = load_daemon_config();
    ensure_auth_token(&mut daemon_config);
    configure_embedded_terminal_engine();

    let allow_remote = is_public_bind(
        daemon_config.auth_token.as_deref(),
        daemon_config.bind.as_deref(),
    );
    let bind_addr = resolve_bind_addr(
        daemon_config.auth_token.as_deref(),
        daemon_config.bind.as_deref(),
    )?;
    let has_auth = daemon_config.auth_token.is_some();
    let auth_state = auth::AuthState::new(daemon_config.auth_token, allow_remote);

    let daemon_store = JsonDaemonSessionStore::default();
    let (agent_broadcast, _) = tokio::sync::broadcast::channel::<AgentWsEvent>(64);
    let (terminal_activity_tx, mut terminal_activity_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::terminal_daemon::TerminalActivityEvent>();

    let repository_store = repository_store::default_repository_store();
    let process_manager = ProcessManager::new();

    #[cfg(feature = "symphony")]
    let symphony = start_symphony_if_configured().await;

    // Initialize task scheduler — load [[tasks]] from arbor.toml
    let task_scheduler = {
        let roots = repository_store.load_roots().unwrap_or_default();
        let resolved = repository_store::resolve_repository_roots(roots);
        let repo_root = resolved
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("."));
        let mut ts = TaskScheduler::new(repo_root.clone());
        let configs = task_scheduler::load_task_configs(&repo_root);
        if !configs.is_empty() {
            println!(
                "loaded {} task config(s) from {}/arbor.toml",
                configs.len(),
                repo_root.display()
            );
        }
        ts.load_configs(configs);
        ts
    };

    let state = AppState {
        repository_store: repository_store.clone(),
        daemon: Arc::new(Mutex::new(LocalTerminalDaemon::new(
            daemon_store,
            Some(terminal_activity_tx),
        ))),
        process_manager: Arc::new(Mutex::new(process_manager)),
        #[cfg(feature = "symphony")]
        symphony,
        task_scheduler: Arc::new(Mutex::new(task_scheduler)),
        github_service: github_service::default_github_pr_service(),
        issue_service: Arc::new(issue_provider::RepositoryIssueService::default()),
        agent_sessions: Arc::new(Mutex::new(HashMap::new())),
        agent_broadcast,
        #[cfg(feature = "agent-chat")]
        agent_chat: Arc::new(Mutex::new(agent_chat::AgentChatManager::new())),
        log_broadcast,
        pr_cache: Arc::new(Mutex::new(HashMap::new())),
        repo_cache: Arc::new(Mutex::new(HashMap::new())),
        shutdown_signal: Arc::new(tokio::sync::Notify::new()),
        auth_state: auth_state.clone(),
    };

    // Restore persisted agent chat sessions and spawn listeners
    #[cfg(feature = "agent-chat")]
    {
        let mut mgr = state.agent_chat.lock().await;
        let restored = mgr.load_persisted_sessions();
        drop(mgr);
        for (session_id, event_rx) in restored {
            agent_chat::spawn_session_listener(state.agent_chat.clone(), session_id, event_rx);
        }
    }

    // Forward terminal activity events to agent sessions
    {
        let state = state.clone();
        tokio::spawn(async move {
            while let Some(event) = terminal_activity_rx.recv().await {
                crate::routes::apply_terminal_activity_event(&state, event).await;
            }
        });
    }

    // Spawn background task to monitor process lifecycle
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
            let mut ticks_since_reap: u32 = 0;
            let mut ticks_since_process_check: u32 = 0;
            loop {
                interval.tick().await;
                let (restart_schedule, crashed_processes) = {
                    let mut pm = state.process_manager.lock().await;
                    let mut daemon = state.daemon.lock().await;
                    let previous = pm.list_processes(&[]);

                    // Periodically reap exited terminal sessions to free memory
                    // (~23 MB per dead session from scrollback buffers).
                    ticks_since_reap += 1;
                    if ticks_since_reap >= 30 {
                        // Every ~60 seconds
                        daemon.reap_exited_sessions();
                        ticks_since_reap = 0;
                    }

                    let restart_schedule = pm.check_and_update(&mut *daemon);
                    let current = pm.list_processes(&[]);
                    let crashed_processes = current
                        .into_iter()
                        .filter(|process| {
                            process.status == ProcessStatus::Crashed
                                && previous
                                    .iter()
                                    .find(|candidate| candidate.id == process.id)
                                    .map(|candidate| candidate.status)
                                    != Some(ProcessStatus::Crashed)
                        })
                        .collect::<Vec<_>>();

                    (restart_schedule, crashed_processes)
                };
                for process in crashed_processes {
                    spawn_notification_webhooks(
                        PathBuf::from(&process.repo_root),
                        "agent_error",
                        serde_json::json!({
                            "event": "agent_error",
                            "repo_root": process.repo_root,
                            "workspace_id": process.workspace_id,
                            "process_name": process.name,
                            "command": process.command,
                            "exit_code": process.exit_code,
                            "timestamp_unix_ms": std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        }),
                    );
                }
                for (name, delay) in restart_schedule {
                    let state = state.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let mut pm = state.process_manager.lock().await;
                        let mut daemon = state.daemon.lock().await;
                        let _ = pm.restart_tracked_process(&name, &mut *daemon);
                    });
                }

                // Periodically check session process trees for runaway spawning
                ticks_since_process_check += 1;
                if ticks_since_process_check >= 5 {
                    ticks_since_process_check = 0;
                    let daemon = state.daemon.lock().await;
                    if let Ok(metrics) =
                        crate::process_metrics::collect_session_process_metrics(&*daemon)
                    {
                        let mut total_processes: u32 = 0;
                        for (session_id, session_metrics) in &metrics {
                            total_processes =
                                total_processes.saturating_add(session_metrics.process_count);
                            let memory_mb = session_metrics.memory_bytes as f64 / (1024.0 * 1024.0);
                            if session_metrics.process_count > 200 {
                                tracing::error!(
                                    session_id = %session_id,
                                    process_count = session_metrics.process_count,
                                    memory_mb = format!("{memory_mb:.1}"),
                                    "session has excessive descendant processes"
                                );
                            } else if session_metrics.process_count > 50 {
                                tracing::warn!(
                                    session_id = %session_id,
                                    process_count = session_metrics.process_count,
                                    memory_mb = format!("{memory_mb:.1}"),
                                    "session has many descendant processes"
                                );
                            }
                        }
                        if total_processes > 100 {
                            tracing::info!(
                                total_process_count = total_processes,
                                session_count = metrics.len(),
                                "high total process count across all sessions"
                            );
                        }
                    }
                }
            }
        });
    }

    // Spawn background task to run scheduled tasks
    {
        let scheduler = state.task_scheduler.clone();
        tokio::spawn(task_scheduler::run_task_loop(scheduler));
    }

    Ok((state, auth_state, has_auth, bind_addr))
}
