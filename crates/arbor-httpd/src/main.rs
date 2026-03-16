#[cfg(feature = "agent-chat")]
pub(crate) mod agent_chat;
mod auth;
mod error;
mod github_service;
mod issue_linking;
mod issue_provider;
mod managed_worktree;
#[cfg(feature = "mdns")]
mod mdns;
mod process_manager;
mod process_metrics;
mod repository_store;
mod routes;
mod startup;
pub(crate) mod task_scheduler;
mod terminal_daemon;
mod types;

use std::net::SocketAddr;
pub(crate) use {error::*, types::*};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Set up tracing with a broadcast layer so logs can be streamed to the GUI.
    let (log_broadcast, _) = tokio::sync::broadcast::channel::<String>(LOG_BROADCAST_CAPACITY);
    {
        use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};
        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let broadcast_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);
        let broadcast_layer = BroadcastLogLayer {
            sender: log_broadcast.clone(),
        }
        .with_filter(broadcast_filter);
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(broadcast_layer)
            .init();
    }

    let web_ui_result = routes::ensure_web_ui_assets();
    if let Err(error) = &web_ui_result {
        eprintln!("web-ui build skipped: {error}");
    }

    let (state, auth_state, has_auth, bind_addr) = startup::build_app_state(log_broadcast).await?;

    let shutdown_signal = state.shutdown_signal.clone();
    let app = auth::with_auth(routes::router(state), auth_state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(
        terminal_engine = arbor_terminal_emulator::default_terminal_engine().as_str(),
        "arbor-httpd listening on http://{local_addr}",
    );

    // Announce on the local network via mDNS — hold handle to keep registration alive
    #[cfg(feature = "mdns")]
    let _mdns = match mdns::register_service(local_addr.port(), false, has_auth) {
        Ok(registration) => {
            tracing::info!(port = local_addr.port(), "mDNS: announcing _arbor._tcp");
            Some(registration)
        },
        Err(e) => {
            tracing::warn!(%e, "mDNS: failed to register, LAN discovery disabled");
            None
        },
    };

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { shutdown_signal.notified().await })
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            process_manager::ProcessManager,
            repository_store::JsonRepositoryStore,
            routes::{apply_terminal_activity_event, process_ws_client_message, write_terminal},
            task_scheduler::TaskScheduler,
            terminal_daemon::{LocalTerminalDaemon, SessionEvent, TerminalActivityEvent},
        },
        arbor_core::{
            agent::AgentState,
            daemon::{CreateOrAttachRequest, JsonDaemonSessionStore, KillRequest, TerminalDaemon},
        },
        axum::{
            Json,
            body::Bytes,
            extract::{Path as AxumPath, State, ws::Message},
            http::StatusCode,
        },
        std::{
            collections::HashMap, env, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration,
        },
        tokio::sync::Mutex,
    };
    #[cfg(feature = "symphony")]
    use {
        crate::routes::router,
        arbor_symphony::{
            Issue, IssueRuntimeSnapshot, IssueTracker, RuntimeSnapshot, ServiceOptions,
            SymphonyService, TrackerError,
            codex::{RunAttemptRequest, RunOutcome, RunResult, Runner, RunnerError, RunnerEvent},
        },
        async_trait::async_trait,
        axum::{
            body::{Body, to_bytes},
            http::Request,
        },
        std::sync::Mutex as StdMutex,
        tower::ServiceExt,
    };

    #[tokio::test]
    #[cfg_attr(windows, ignore = "requires Unix shell (stty/cat)")]
    async fn write_terminal_accepts_raw_request_bytes() {
        let temp = match tempfile::tempdir() {
            Ok(temp) => temp,
            Err(error) => panic!("failed to create temp dir: {error}"),
        };
        let state = test_app_state(temp.path().to_path_buf());
        let session_id = create_raw_echo_session(&state, "rest-binary").await;
        let mut subscription = subscribe_to_session(&state, &session_id).await;
        let payload = vec![0xff, 0x00, 0x1b, b'[', b'6', b'n'];

        let response = write_terminal(
            State(state.clone()),
            AxumPath(session_id.clone()),
            Bytes::from(payload.clone()),
        )
        .await;
        let status = match response {
            Ok(status) => status,
            Err((status, Json(error))) => {
                panic!("write handler failed with {status}: {}", error.error)
            },
        };
        assert_eq!(status, StatusCode::NO_CONTENT);

        let echoed = collect_output_bytes(&mut subscription, payload.len()).await;
        assert_eq!(echoed, payload);

        kill_session(&state, &session_id).await;
    }

    #[tokio::test]
    #[cfg_attr(windows, ignore = "requires Unix shell (stty/cat)")]
    async fn websocket_binary_frames_write_raw_terminal_bytes() {
        let temp = match tempfile::tempdir() {
            Ok(temp) => temp,
            Err(error) => panic!("failed to create temp dir: {error}"),
        };
        let state = test_app_state(temp.path().to_path_buf());
        let session_id = create_raw_echo_session(&state, "ws-binary").await;
        let mut subscription = subscribe_to_session(&state, &session_id).await;
        let payload = vec![0xde, 0xad, 0x00, 0xbe, 0xef];

        let keep_open = match process_ws_client_message(
            &state,
            &session_id,
            Message::Binary(payload.clone().into()),
        )
        .await
        {
            Ok(keep_open) => keep_open,
            Err(error) => panic!("binary websocket write failed: {error}"),
        };
        assert!(
            keep_open,
            "binary terminal input unexpectedly closed the socket"
        );

        let echoed = collect_output_bytes(&mut subscription, payload.len()).await;
        assert_eq!(echoed, payload);

        kill_session(&state, &session_id).await;
    }

    #[tokio::test]
    async fn terminal_activity_events_are_keyed_by_session_id() {
        let temp = match tempfile::tempdir() {
            Ok(temp) => temp,
            Err(error) => panic!("failed to create temp dir: {error}"),
        };
        let state = test_app_state(temp.path().to_path_buf());

        apply_terminal_activity_event(&state, TerminalActivityEvent::Update {
            session_id: "daemon-1".into(),
            cwd: PathBuf::from("/tmp/repo/worktree"),
            state: AgentState::Waiting,
        })
        .await;
        apply_terminal_activity_event(&state, TerminalActivityEvent::Update {
            session_id: "daemon-2".into(),
            cwd: PathBuf::from("/tmp/repo/worktree"),
            state: AgentState::Waiting,
        })
        .await;

        {
            let sessions = state.agent_sessions.lock().await;
            assert_eq!(sessions.len(), 2);
            assert!(sessions.contains_key("terminal:daemon-1"));
            assert!(sessions.contains_key("terminal:daemon-2"));
        }

        apply_terminal_activity_event(&state, TerminalActivityEvent::Clear {
            session_id: "daemon-1".into(),
        })
        .await;

        let sessions = state.agent_sessions.lock().await;
        assert_eq!(sessions.len(), 1);
        assert!(!sessions.contains_key("terminal:daemon-1"));
        assert!(sessions.contains_key("terminal:daemon-2"));
    }

    #[test]
    fn default_bind_addr_uses_public_interface_only_when_auth_is_enabled() {
        assert_eq!(
            default_bind_addr(Some("secret-token"), 8787),
            SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 8787))
        );
        assert_eq!(
            default_bind_addr(None, 8787),
            SocketAddr::from(([127, 0, 0, 1], 8787))
        );
        assert_eq!(
            default_bind_addr(Some("   "), 8787),
            SocketAddr::from(([127, 0, 0, 1], 8787))
        );
    }

    #[test]
    fn configured_bind_addr_overrides_default_bind_mode() {
        assert_eq!(
            configured_bind_addr(Some("localhost"), Some("secret-token"), 8787),
            SocketAddr::from(([127, 0, 0, 1], 8787))
        );
        assert_eq!(
            configured_bind_addr(Some("all-interfaces"), None, 8787),
            SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 8787))
        );
    }

    #[cfg(feature = "symphony")]
    #[derive(Default)]
    struct MockSymphonyTracker {
        issues: StdMutex<Vec<Issue>>,
    }

    #[cfg(feature = "symphony")]
    #[async_trait]
    impl IssueTracker for MockSymphonyTracker {
        async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
            match self.issues.lock() {
                Ok(issues) => Ok(issues.clone()),
                Err(error) => Err(TrackerError::LinearApiRequest(error.to_string())),
            }
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
            match self.issues.lock() {
                Ok(issues) => Ok(issues
                    .iter()
                    .filter(|issue| issue_ids.contains(&issue.id))
                    .cloned()
                    .collect()),
                Err(error) => Err(TrackerError::LinearApiRequest(error.to_string())),
            }
        }
    }

    #[cfg(feature = "symphony")]
    #[derive(Default)]
    struct MockSymphonyRunner;

    #[cfg(feature = "symphony")]
    #[async_trait]
    impl Runner for MockSymphonyRunner {
        async fn run_attempt(
            &self,
            _request: RunAttemptRequest,
            events: tokio::sync::mpsc::UnboundedSender<RunnerEvent>,
        ) -> Result<RunResult, RunnerError> {
            let _ = events.send(RunnerEvent {
                event: "turn/completed".to_owned(),
                at: "test".to_owned(),
                ..RunnerEvent::default()
            });
            Ok(RunResult {
                outcome: RunOutcome::Completed,
                ..RunResult::default()
            })
        }
    }

    #[cfg(feature = "symphony")]
    #[tokio::test]
    async fn symphony_routes_return_state_and_issue_snapshots() {
        let temp = match tempfile::tempdir() {
            Ok(temp) => temp,
            Err(error) => panic!("failed to create temp dir: {error}"),
        };
        let workflow_path = temp.path().join("WORKFLOW.md");
        let workflow = format!(
            "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: arbor\nworkspace:\n  root: {}\ncodex:\n  command: codex app-server\n---\nIssue {{{{ issue.identifier }}}}",
            temp.path().join("workspaces").display()
        );
        if let Err(error) = std::fs::write(&workflow_path, workflow) {
            panic!("failed to write workflow: {error}");
        }

        let symphony = match SymphonyService::start(ServiceOptions {
            workflow_path: Some(workflow_path),
            runner: Arc::new(MockSymphonyRunner),
            tracker: Some(Arc::new(MockSymphonyTracker {
                issues: StdMutex::new(vec![Issue {
                    id: "1".to_owned(),
                    identifier: "ARB-1".to_owned(),
                    title: "Ready".to_owned(),
                    state: "Todo".to_owned(),
                    ..Issue::default()
                }]),
            })),
        })
        .await
        {
            Ok(handle) => handle,
            Err(error) => panic!("failed to start symphony service: {error}"),
        };

        let state = test_app_state(temp.path().to_path_buf()).with_symphony(symphony.clone());
        let app = router(state);

        let refresh_request = match Request::builder()
            .method("POST")
            .uri("/api/v1/symphony/refresh")
            .body(Body::empty())
        {
            Ok(request) => request,
            Err(error) => panic!("failed to build refresh request: {error}"),
        };
        let refresh_response = match app.clone().oneshot(refresh_request).await {
            Ok(response) => response,
            Err(error) => panic!("refresh route failed: {error}"),
        };
        assert_eq!(refresh_response.status(), StatusCode::OK);

        tokio::time::sleep(Duration::from_millis(50)).await;

        let state_request = match Request::builder()
            .uri("/api/v1/symphony/state")
            .body(Body::empty())
        {
            Ok(request) => request,
            Err(error) => panic!("failed to build state request: {error}"),
        };
        let state_response = match app.clone().oneshot(state_request).await {
            Ok(response) => response,
            Err(error) => panic!("state route failed: {error}"),
        };
        assert_eq!(state_response.status(), StatusCode::OK);
        let state_body = match to_bytes(state_response.into_body(), usize::MAX).await {
            Ok(body) => body,
            Err(error) => panic!("failed to read state body: {error}"),
        };
        let snapshot: RuntimeSnapshot = match serde_json::from_slice(&state_body) {
            Ok(snapshot) => snapshot,
            Err(error) => panic!("failed to decode state snapshot: {error}"),
        };
        assert!(
            !snapshot.retrying.is_empty(),
            "expected retrying entry after run"
        );

        let issue_request = match Request::builder()
            .uri("/api/v1/symphony/ARB-1")
            .body(Body::empty())
        {
            Ok(request) => request,
            Err(error) => panic!("failed to build issue request: {error}"),
        };
        let issue_response = match app.oneshot(issue_request).await {
            Ok(response) => response,
            Err(error) => panic!("issue route failed: {error}"),
        };
        assert_eq!(issue_response.status(), StatusCode::OK);
        let issue_body = match to_bytes(issue_response.into_body(), usize::MAX).await {
            Ok(body) => body,
            Err(error) => panic!("failed to read issue body: {error}"),
        };
        let issue: IssueRuntimeSnapshot = match serde_json::from_slice(&issue_body) {
            Ok(issue) => issue,
            Err(error) => panic!("failed to decode issue snapshot: {error}"),
        };
        assert_eq!(issue.issue_identifier, "ARB-1");

        let _ = symphony.stop();
    }

    #[cfg(feature = "symphony")]
    #[tokio::test]
    async fn symphony_issue_route_returns_not_found_for_unknown_issue() {
        let temp = match tempfile::tempdir() {
            Ok(temp) => temp,
            Err(error) => panic!("failed to create temp dir: {error}"),
        };
        let workflow_path = temp.path().join("WORKFLOW.md");
        let workflow = format!(
            "---\ntracker:\n  kind: linear\n  api_key: token\n  project_slug: arbor\nworkspace:\n  root: {}\ncodex:\n  command: codex app-server\n---\nIssue {{{{ issue.identifier }}}}",
            temp.path().join("workspaces").display()
        );
        if let Err(error) = std::fs::write(&workflow_path, workflow) {
            panic!("failed to write workflow: {error}");
        }

        let symphony = match SymphonyService::start(ServiceOptions {
            workflow_path: Some(workflow_path),
            runner: Arc::new(MockSymphonyRunner),
            tracker: Some(Arc::new(MockSymphonyTracker::default())),
        })
        .await
        {
            Ok(handle) => handle,
            Err(error) => panic!("failed to start symphony service: {error}"),
        };

        let state = test_app_state(temp.path().to_path_buf()).with_symphony(symphony.clone());
        let app = router(state);
        let request = match Request::builder()
            .uri("/api/v1/symphony/ARB-404")
            .body(Body::empty())
        {
            Ok(request) => request,
            Err(error) => panic!("failed to build issue request: {error}"),
        };

        let response = match app.oneshot(request).await {
            Ok(response) => response,
            Err(error) => panic!("issue route failed: {error}"),
        };
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let _ = symphony.stop();
    }

    fn test_app_state(repo_root: PathBuf) -> AppState {
        let daemon_store = JsonDaemonSessionStore::new(repo_root.join("daemon-sessions.json"));
        let repository_store = Arc::new(JsonRepositoryStore::new(
            repo_root.join("repositories.json"),
        ));
        let (agent_broadcast, _) = tokio::sync::broadcast::channel(16);
        let (terminal_activity_tx, _terminal_activity_rx) = tokio::sync::mpsc::unbounded_channel();
        let (log_broadcast, _) = tokio::sync::broadcast::channel(16);

        AppState {
            repository_store,
            daemon: Arc::new(Mutex::new(LocalTerminalDaemon::new(
                daemon_store,
                Some(terminal_activity_tx),
            ))),
            process_manager: Arc::new(Mutex::new(ProcessManager::new())),
            task_scheduler: Arc::new(Mutex::new(TaskScheduler::new(repo_root))),
            #[cfg(feature = "symphony")]
            symphony: None,
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
            auth_state: auth::AuthState::new(None, false),
        }
    }

    #[cfg(feature = "symphony")]
    trait TestAppStateExt {
        fn with_symphony(self, symphony: arbor_symphony::ServiceHandle) -> Self;
    }

    #[cfg(feature = "symphony")]
    impl TestAppStateExt for AppState {
        fn with_symphony(mut self, symphony: arbor_symphony::ServiceHandle) -> Self {
            self.symphony = Some(symphony);
            self
        }
    }

    async fn create_raw_echo_session(state: &AppState, session_id: &str) -> String {
        let cwd = match env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => panic!("failed to read current directory: {error}"),
        };
        let response = {
            let mut daemon = state.daemon.lock().await;
            daemon.create_or_attach(CreateOrAttachRequest {
                session_id: session_id.into(),
                workspace_id: cwd.display().to_string().into(),
                cwd,
                shell: String::new(),
                cols: 120,
                rows: 35,
                title: Some("binary-test".to_owned()),
                command: Some("stty raw -echo; cat".to_owned()),
            })
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => panic!("failed to create test terminal session: {error}"),
        };

        tokio::time::sleep(Duration::from_millis(100)).await;
        response.session.session_id.to_string()
    }

    async fn subscribe_to_session(
        state: &AppState,
        session_id: &str,
    ) -> tokio::sync::broadcast::Receiver<SessionEvent> {
        let daemon = state.daemon.lock().await;
        match daemon.subscribe(session_id) {
            Ok(subscription) => subscription,
            Err(error) => panic!("failed to subscribe to session `{session_id}`: {error}"),
        }
    }

    async fn collect_output_bytes(
        subscription: &mut tokio::sync::broadcast::Receiver<SessionEvent>,
        expected_len: usize,
    ) -> Vec<u8> {
        let mut output = Vec::new();

        while output.len() < expected_len {
            let event =
                match tokio::time::timeout(Duration::from_secs(3), subscription.recv()).await {
                    Ok(Ok(event)) => event,
                    Ok(Err(error)) => panic!("terminal event stream failed: {error}"),
                    Err(_) => panic!("timed out waiting for terminal output"),
                };

            match event {
                SessionEvent::Output(bytes) => output.extend_from_slice(&bytes),
                SessionEvent::Exit { exit_code, state } => {
                    panic!(
                        "terminal session exited before output arrived: state={state:?} exit_code={exit_code:?}"
                    )
                },
                SessionEvent::Error(message) => {
                    panic!("terminal session reported an error: {message}")
                },
            }
        }

        output
    }

    async fn kill_session(state: &AppState, session_id: &str) {
        let result = {
            let mut daemon = state.daemon.lock().await;
            daemon.kill(KillRequest {
                session_id: session_id.into(),
            })
        };

        if let Err(error) = result {
            panic!("failed to kill test session `{session_id}`: {error}");
        }
    }
}
