mod github_service;
mod mdns;
mod process_manager;
mod repository_store;
mod terminal_daemon;

use {
    crate::{
        github_service::GitHubPrService,
        process_manager::ProcessManager,
        terminal_daemon::{LocalTerminalDaemon, LocalTerminalDaemonError, SessionEvent},
    },
    arbor_core::{
        agent::AgentState,
        changes,
        daemon::{
            CreateOrAttachRequest, DaemonSessionRecord, DetachRequest, JsonDaemonSessionStore,
            KillRequest, ResizeRequest, SignalRequest, SnapshotRequest, TerminalDaemon,
            TerminalSignal, TerminalSnapshot, WriteRequest,
        },
        process::ProcessInfo,
        worktree,
    },
    axum::{
        Json, Router,
        extract::{
            Path as AxumPath, Query, State,
            ws::{Message, WebSocket, WebSocketUpgrade},
        },
        handler::HandlerWithoutStateExt,
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::{delete, get, post},
    },
    futures_util::StreamExt,
    serde::{Deserialize, Serialize},
    std::{
        collections::HashMap,
        net::SocketAddr,
        path::{Path, PathBuf},
        process::Command,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    },
    tokio::sync::Mutex,
    tower_http::services::ServeDir,
};

const HTTPD_VERSION: &str = match option_env!("ARBOR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

const AGENT_SESSION_EXPIRY_SECS: u64 = 300;

/// Cached PR lookup result with expiry.
#[derive(Clone)]
struct PrCacheEntry {
    pr_number: Option<u64>,
    pr_url: Option<String>,
    fetched_at: std::time::Instant,
}

const PR_CACHE_TTL_SECS: u64 = 300;

/// Cached repository metadata (GitHub slug & avatar).
#[derive(Clone)]
struct RepoCacheEntry {
    github_repo_slug: Option<String>,
    avatar_url: Option<String>,
    fetched_at: std::time::Instant,
}

const REPO_CACHE_TTL_SECS: u64 = 600;

#[derive(Clone)]
struct AppState {
    repository_store: Arc<dyn repository_store::RepositoryStore>,
    daemon: Arc<Mutex<LocalTerminalDaemon>>,
    process_manager: Arc<Mutex<ProcessManager>>,
    github_service: Arc<dyn GitHubPrService>,
    agent_sessions: Arc<Mutex<HashMap<String, AgentSession>>>,
    agent_broadcast: tokio::sync::broadcast::Sender<AgentWsEvent>,
    pr_cache: Arc<Mutex<HashMap<String, PrCacheEntry>>>,
    repo_cache: Arc<Mutex<HashMap<String, RepoCacheEntry>>>,
    shutdown_signal: Arc<tokio::sync::Notify>,
}

#[derive(Debug, Clone)]
struct AgentSession {
    cwd: String,
    state: AgentState,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct AgentNotifyRequest {
    hook_event_name: String,
    session_id: String,
    cwd: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum AgentWsEvent {
    Snapshot { sessions: Vec<AgentSessionDto> },
    Update { session: AgentSessionDto },
}

#[derive(Debug, Clone, Serialize)]
struct AgentSessionDto {
    cwd: String,
    state: String,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;
type ApiResponse = Result<Response, (StatusCode, Json<ApiError>)>;

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

#[derive(Debug, Serialize)]
struct RepositoryDto {
    root: String,
    label: String,
    github_repo_slug: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorktreeDto {
    repo_root: String,
    path: String,
    branch: String,
    is_primary_checkout: bool,
    last_activity_unix_ms: Option<u64>,
    diff_additions: Option<usize>,
    diff_deletions: Option<usize>,
    pr_number: Option<u64>,
    pr_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorktreeQuery {
    repo_root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChangesQuery {
    path: String,
}

#[derive(Debug, Serialize)]
struct ChangedFileDto {
    path: String,
    kind: String,
    additions: usize,
    deletions: usize,
}

#[derive(Debug, Deserialize)]
struct CreateTerminalRequest {
    session_id: Option<String>,
    workspace_id: Option<String>,
    cwd: String,
    shell: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    title: Option<String>,
    command: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateTerminalResponse {
    is_new_session: bool,
    session: DaemonSessionRecord,
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    max_lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TerminalWriteRequest {
    data: String,
}

#[derive(Debug, Deserialize)]
struct TerminalResizeRequest {
    cols: u16,
    rows: u16,
}

#[derive(Debug, Deserialize)]
struct TerminalSignalRequest {
    signal: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum WsClientEvent {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
    Signal { signal: String },
    Detach,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum WsServerEvent {
    Snapshot {
        output_tail: String,
        state: arbor_core::daemon::TerminalSessionState,
        exit_code: Option<i32>,
        updated_at_unix_ms: Option<u64>,
    },
    Output {
        data: String,
    },
    Exit {
        state: arbor_core::daemon::TerminalSessionState,
        exit_code: Option<i32>,
    },
    Error {
        message: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind_addr = resolve_bind_addr()?;
    let web_ui_result = ensure_web_ui_assets();
    if let Err(error) = &web_ui_result {
        eprintln!("web-ui build skipped: {error}");
    }

    let daemon_store = JsonDaemonSessionStore::default();
    let (agent_broadcast, _) = tokio::sync::broadcast::channel::<AgentWsEvent>(64);

    // Initialize process manager — scan repository roots for arbor.toml files
    let repository_store = repository_store::default_repository_store();
    let process_manager = {
        let roots = repository_store.load_roots().unwrap_or_default();
        let resolved = repository_store::resolve_repository_roots(roots);
        let repo_root = resolved
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("."));
        let mut pm = ProcessManager::new(repo_root.clone());
        let configs = process_manager::load_process_configs(&repo_root);
        if !configs.is_empty() {
            println!(
                "loaded {} process config(s) from {}/arbor.toml",
                configs.len(),
                repo_root.display()
            );
        }
        pm.load_configs(configs);
        pm
    };

    let state = AppState {
        repository_store: repository_store.clone(),
        daemon: Arc::new(Mutex::new(LocalTerminalDaemon::new(daemon_store))),
        process_manager: Arc::new(Mutex::new(process_manager)),
        github_service: github_service::default_github_pr_service(),
        agent_sessions: Arc::new(Mutex::new(HashMap::new())),
        agent_broadcast,
        pr_cache: Arc::new(Mutex::new(HashMap::new())),
        repo_cache: Arc::new(Mutex::new(HashMap::new())),
        shutdown_signal: Arc::new(tokio::sync::Notify::new()),
    };

    // Spawn background task to monitor process lifecycle
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
            loop {
                interval.tick().await;
                let restart_schedule = {
                    let mut pm = state.process_manager.lock().await;
                    let mut daemon = state.daemon.lock().await;
                    pm.check_and_update(&mut *daemon)
                };
                for (name, delay) in restart_schedule {
                    let state = state.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let mut pm = state.process_manager.lock().await;
                        let mut daemon = state.daemon.lock().await;
                        let _ = pm.start_process(&name, &mut *daemon);
                    });
                }
            }
        });
    }

    let shutdown_signal = state.shutdown_signal.clone();
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    println!("arbor-httpd listening on http://{local_addr}");

    // Announce on the local network via mDNS — hold handle to keep registration alive
    let _mdns = match mdns::register_service(local_addr.port(), false, false) {
        Ok(registration) => {
            println!(
                "  mDNS: announcing _arbor._tcp on port {}",
                local_addr.port()
            );
            Some(registration)
        },
        Err(e) => {
            eprintln!("  mDNS: failed to register ({e}), LAN discovery disabled");
            None
        },
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown_signal.notified().await })
        .await?;

    Ok(())
}

fn resolve_bind_addr() -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let raw = std::env::var("ARBOR_HTTPD_BIND").unwrap_or_else(|_| "0.0.0.0:8787".to_owned());
    let parsed: SocketAddr = raw.parse()?;
    Ok(parsed)
}

fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/repositories", get(list_repositories))
        .route("/worktrees", get(list_worktrees))
        .route("/worktrees/changes", get(list_worktree_changes))
        .route("/terminals", get(list_terminals).post(create_terminal))
        .route(
            "/terminals/{session_id}/snapshot",
            get(get_terminal_snapshot),
        )
        .route("/terminals/{session_id}/write", post(write_terminal))
        .route("/terminals/{session_id}/resize", post(resize_terminal))
        .route("/terminals/{session_id}/signal", post(signal_terminal))
        .route("/terminals/{session_id}/detach", post(detach_terminal))
        .route("/terminals/{session_id}", delete(kill_terminal))
        .route("/terminals/{session_id}/ws", get(terminal_ws))
        .route("/agent/notify", post(agent_notify))
        .route("/agent/activity/ws", get(agent_activity_ws))
        .route("/processes", get(list_processes))
        .route("/processes/start-all", post(start_all_processes))
        .route("/processes/stop-all", post(stop_all_processes))
        .route("/processes/{name}/start", post(start_process))
        .route("/processes/{name}/stop", post(stop_process))
        .route("/processes/{name}/restart", post(restart_process))
        .route("/processes/ws", get(process_status_ws))
        .route("/shutdown", post(shutdown_daemon));

    let with_state = Router::new().nest("/api/v1", api).with_state(state);

    // Always set up ServeDir — check for assets dynamically per-request so
    // that a long-running daemon picks up assets installed after startup
    // (e.g. an app update while the detached httpd process is still running).
    let dist_dir = arbor_web_ui::dist_dir();
    with_state.fallback_service(
        ServeDir::new(dist_dir).not_found_service(web_ui_spa_or_unavailable.into_service()),
    )
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: HTTPD_VERSION,
    })
}

async fn shutdown_daemon(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    if !addr.ip().is_loopback() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError {
                error: "shutdown is only allowed from localhost".to_owned(),
            }),
        ));
    }
    eprintln!("shutdown requested from {addr}, shutting down");
    state.shutdown_signal.notify_one();
    Ok(StatusCode::OK)
}

async fn list_repositories(State(state): State<AppState>) -> ApiResult<Vec<RepositoryDto>> {
    let roots = state
        .repository_store
        .load_roots()
        .map_err(internal_error)?;
    let resolved = repository_store::resolve_repository_roots(roots);

    let mut cache = state.repo_cache.lock().await;
    let repositories = resolved
        .into_iter()
        .map(|root| {
            let (slug, avatar_url) = github_repo_slug_cached(&mut cache, &root);
            RepositoryDto {
                label: repository_display_name(&root),
                root: root.display().to_string(),
                github_repo_slug: slug,
                avatar_url,
            }
        })
        .collect();

    Ok(Json(repositories))
}

async fn list_worktrees(
    State(state): State<AppState>,
    Query(query): Query<WorktreeQuery>,
) -> ApiResult<Vec<WorktreeDto>> {
    let roots = state
        .repository_store
        .load_roots()
        .map_err(internal_error)?;
    let resolved = repository_store::resolve_repository_roots(roots);
    let filter = query.repo_root.as_deref().map(PathBuf::from);

    // Phase 1: collect all worktree data (sync, fast)
    struct WorktreeData {
        repo_root: String,
        path: String,
        branch: String,
        is_primary: bool,
        last_activity_unix_ms: Option<u64>,
        diff_additions: Option<usize>,
        diff_deletions: Option<usize>,
        repo_slug: Option<String>,
    }

    let mut entries_data: Vec<WorktreeData> = Vec::new();

    {
        let mut repo_cache = state.repo_cache.lock().await;
        for repository_root in &resolved {
            if let Some(filter_root) = filter.as_ref()
                && !paths_equivalent(repository_root, filter_root)
            {
                continue;
            }

            let (repo_slug, _) = github_repo_slug_cached(&mut repo_cache, repository_root);

            match worktree::list(repository_root) {
                Ok(entries) => {
                    for entry in entries {
                        let last_activity_unix_ms = worktree::last_git_activity_ms(&entry.path);
                        let diff_summary = changes::diff_line_summary(&entry.path).ok();
                        let branch_name = entry
                            .branch
                            .as_deref()
                            .map(short_branch)
                            .unwrap_or_else(|| "-".to_owned());
                        let is_primary = entry.path.as_path() == repository_root.as_path();
                        entries_data.push(WorktreeData {
                            repo_root: repository_root.display().to_string(),
                            path: entry.path.display().to_string(),
                            branch: branch_name,
                            is_primary,
                            last_activity_unix_ms,
                            diff_additions: diff_summary.as_ref().map(|d| d.additions),
                            diff_deletions: diff_summary.as_ref().map(|d| d.deletions),
                            repo_slug: repo_slug.clone(),
                        });
                    }
                },
                Err(error) => {
                    return Err(internal_error(format!(
                        "failed to list worktrees for `{}`: {error}",
                        repository_root.display()
                    )));
                },
            }
        }
    } // drop repo_cache lock before async PR lookups

    // Phase 2: look up PRs concurrently with caching
    let pr_futures: Vec<_> = entries_data
        .iter()
        .map(|wd| {
            let cache = state.pr_cache.clone();
            let github_service = state.github_service.clone();
            let slug = wd.repo_slug.clone();
            let branch = wd.branch.clone();
            let is_primary = wd.is_primary;
            async move {
                lookup_pr_cached(cache, github_service, slug.as_deref(), &branch, is_primary).await
            }
        })
        .collect();

    let pr_results = futures_util::future::join_all(pr_futures).await;

    // Phase 3: assemble DTOs
    let mut worktrees: Vec<WorktreeDto> = entries_data
        .into_iter()
        .zip(pr_results)
        .map(|(wd, (pr_number, pr_url))| WorktreeDto {
            repo_root: wd.repo_root,
            path: wd.path,
            branch: wd.branch,
            is_primary_checkout: wd.is_primary,
            last_activity_unix_ms: wd.last_activity_unix_ms,
            diff_additions: wd.diff_additions,
            diff_deletions: wd.diff_deletions,
            pr_number,
            pr_url,
        })
        .collect();

    worktrees.sort_by(|left, right| {
        left.repo_root
            .cmp(&right.repo_root)
            .then_with(|| left.path.cmp(&right.path))
    });

    Ok(Json(worktrees))
}

async fn list_worktree_changes(
    Query(query): Query<ChangesQuery>,
) -> ApiResult<Vec<ChangedFileDto>> {
    let worktree_path = PathBuf::from(&query.path);
    let files = changes::changed_files(&worktree_path).map_err(|error| {
        internal_error(format!(
            "failed to get changes for `{}`: {error}",
            worktree_path.display()
        ))
    })?;

    let dtos = files
        .into_iter()
        .map(|file| ChangedFileDto {
            path: file.path.display().to_string(),
            kind: file.kind.to_string(),
            additions: file.additions,
            deletions: file.deletions,
        })
        .collect();

    Ok(Json(dtos))
}

async fn list_terminals(State(state): State<AppState>) -> ApiResult<Vec<DaemonSessionRecord>> {
    let daemon = state.daemon.lock().await;
    let mut sessions = daemon.list_sessions().map_err(map_daemon_error)?;

    sessions.sort_by(|left, right| {
        right
            .updated_at_unix_ms
            .unwrap_or(0)
            .cmp(&left.updated_at_unix_ms.unwrap_or(0))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });

    Ok(Json(sessions))
}

async fn create_terminal(
    State(state): State<AppState>,
    Json(request): Json<CreateTerminalRequest>,
) -> ApiResult<CreateTerminalResponse> {
    let cwd = PathBuf::from(request.cwd.clone());
    let cols = request.cols.unwrap_or(120);
    let rows = request.rows.unwrap_or(35);
    let workspace_id = request.workspace_id.unwrap_or_else(|| request.cwd.clone());

    let mut daemon = state.daemon.lock().await;
    let response = daemon
        .create_or_attach(CreateOrAttachRequest {
            session_id: request.session_id.unwrap_or_default(),
            workspace_id,
            cwd,
            shell: request.shell.unwrap_or_default(),
            cols,
            rows,
            title: request.title,
            command: request.command,
        })
        .map_err(map_daemon_error)?;

    Ok(Json(CreateTerminalResponse {
        is_new_session: response.is_new_session,
        session: response.session,
    }))
}

async fn get_terminal_snapshot(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<SnapshotQuery>,
) -> ApiResult<TerminalSnapshot> {
    let max_lines = query.max_lines.unwrap_or(180).clamp(1, 2000);

    let daemon = state.daemon.lock().await;
    let snapshot = daemon
        .snapshot(SnapshotRequest {
            session_id: session_id.clone(),
            max_lines,
        })
        .map_err(map_daemon_error)?;

    let Some(snapshot) = snapshot else {
        return Err(not_found_error(format!(
            "terminal session `{session_id}` was not found"
        )));
    };

    Ok(Json(snapshot))
}

async fn write_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<TerminalWriteRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .write(WriteRequest {
            session_id,
            bytes: request.data.into_bytes(),
        })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn resize_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<TerminalResizeRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .resize(ResizeRequest {
            session_id,
            cols: request.cols,
            rows: request.rows,
        })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn signal_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<TerminalSignalRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let signal = parse_terminal_signal(&request.signal).ok_or_else(|| {
        bad_request_error("signal must be one of: interrupt, terminate, kill".to_owned())
    })?;

    let mut daemon = state.daemon.lock().await;
    daemon
        .signal(SignalRequest { session_id, signal })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn kill_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .kill(KillRequest { session_id })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn detach_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .detach(DetachRequest { session_id })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn terminal_ws(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    ws: WebSocketUpgrade,
) -> ApiResponse {
    {
        let daemon = state.daemon.lock().await;
        let exists = daemon
            .snapshot(SnapshotRequest {
                session_id: session_id.clone(),
                max_lines: 1,
            })
            .map_err(map_daemon_error)?
            .is_some();
        if !exists {
            return Err(not_found_error(format!(
                "terminal session `{session_id}` was not found"
            )));
        }
    }

    Ok(ws
        .on_upgrade(move |socket| handle_terminal_ws(state, session_id, socket))
        .into_response())
}

async fn handle_terminal_ws(state: AppState, session_id: String, mut socket: WebSocket) {
    let (snapshot, mut subscription) = {
        let daemon = state.daemon.lock().await;
        let snapshot = match daemon.snapshot(SnapshotRequest {
            session_id: session_id.clone(),
            max_lines: 180,
        }) {
            Ok(Some(snapshot)) => snapshot,
            _ => return,
        };
        let subscription = match daemon.subscribe(&session_id) {
            Ok(subscription) => subscription,
            Err(_) => return,
        };

        (snapshot, subscription)
    };

    if send_ws_event(&mut socket, WsServerEvent::Snapshot {
        output_tail: snapshot.output_tail,
        state: snapshot.state,
        exit_code: snapshot.exit_code,
        updated_at_unix_ms: snapshot.updated_at_unix_ms,
    })
    .await
    .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(message)) => {
                        match process_ws_client_message(&state, &session_id, message).await {
                            Ok(true) => {},
                            Ok(false) => {
                                break;
                            },
                            Err(error) => {
                                let _ = send_ws_event(&mut socket, WsServerEvent::Error { message: error }).await;
                            },
                        }
                    },
                    Some(Err(error)) => {
                        let _ = send_ws_event(&mut socket, WsServerEvent::Error { message: format!("websocket receive error: {error}") }).await;
                        break;
                    },
                    None => {
                        break;
                    },
                }
            },
            event = subscription.recv() => {
                match event {
                    Ok(SessionEvent::Output(data)) => {
                        if send_ws_event(&mut socket, WsServerEvent::Output { data }).await.is_err() {
                            break;
                        }
                    },
                    Ok(SessionEvent::Exit { exit_code, state }) => {
                        if send_ws_event(&mut socket, WsServerEvent::Exit { exit_code, state }).await.is_err() {
                            break;
                        }
                    },
                    Ok(SessionEvent::Error(message)) => {
                        if send_ws_event(&mut socket, WsServerEvent::Error { message }).await.is_err() {
                            break;
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        if send_ws_event(&mut socket, WsServerEvent::Error { message: format!("dropped {skipped} terminal events") }).await.is_err() {
                            break;
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    },
                }
            }
        }
    }

    let mut daemon = state.daemon.lock().await;
    let _ = daemon.detach(DetachRequest { session_id });
}

async fn process_ws_client_message(
    state: &AppState,
    session_id: &str,
    message: Message,
) -> Result<bool, String> {
    match message {
        Message::Text(text) => {
            let parsed = serde_json::from_str::<WsClientEvent>(&text)
                .map_err(|error| format!("invalid websocket payload: {error}"))?;

            match parsed {
                WsClientEvent::Input { data } => {
                    let mut daemon = state.daemon.lock().await;
                    daemon
                        .write(WriteRequest {
                            session_id: session_id.to_owned(),
                            bytes: data.into_bytes(),
                        })
                        .map_err(|error| error.to_string())?;
                },
                WsClientEvent::Resize { cols, rows } => {
                    let mut daemon = state.daemon.lock().await;
                    daemon
                        .resize(ResizeRequest {
                            session_id: session_id.to_owned(),
                            cols,
                            rows,
                        })
                        .map_err(|error| error.to_string())?;
                },
                WsClientEvent::Signal { signal } => {
                    let Some(signal) = parse_terminal_signal(&signal) else {
                        return Err("invalid signal, expected interrupt|terminate|kill".to_owned());
                    };

                    let mut daemon = state.daemon.lock().await;
                    daemon
                        .signal(SignalRequest {
                            session_id: session_id.to_owned(),
                            signal,
                        })
                        .map_err(|error| error.to_string())?;
                },
                WsClientEvent::Detach => {
                    let mut daemon = state.daemon.lock().await;
                    daemon
                        .detach(DetachRequest {
                            session_id: session_id.to_owned(),
                        })
                        .map_err(|error| error.to_string())?;
                    return Ok(false);
                },
            }
        },
        Message::Binary(bytes) => {
            let mut daemon = state.daemon.lock().await;
            daemon
                .write(WriteRequest {
                    session_id: session_id.to_owned(),
                    bytes: bytes.to_vec(),
                })
                .map_err(|error| error.to_string())?;
        },
        Message::Ping(_) => {},
        Message::Pong(_) => {},
        Message::Close(_) => {
            return Ok(false);
        },
    }

    Ok(true)
}

async fn send_ws_event(socket: &mut WebSocket, event: WsServerEvent) -> Result<(), ()> {
    let payload = serde_json::to_string(&event).map_err(|_| ())?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}

async fn agent_notify(
    State(state): State<AppState>,
    Json(request): Json<AgentNotifyRequest>,
) -> StatusCode {
    let agent_state = match request.hook_event_name.as_str() {
        "UserPromptSubmit" => AgentState::Working,
        "Stop" => AgentState::Waiting,
        _ => return StatusCode::OK,
    };

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let dto = {
        let mut sessions = state.agent_sessions.lock().await;

        // Expire stale sessions
        let cutoff = now_ms.saturating_sub(AGENT_SESSION_EXPIRY_SECS * 1000);
        sessions.retain(|_, session| session.updated_at_unix_ms > cutoff);

        sessions.insert(request.session_id, AgentSession {
            cwd: request.cwd.clone(),
            state: agent_state,
            updated_at_unix_ms: now_ms,
        });

        AgentSessionDto {
            cwd: request.cwd,
            state: match agent_state {
                AgentState::Working => "working".to_owned(),
                AgentState::Waiting => "waiting".to_owned(),
            },
            updated_at_unix_ms: now_ms,
        }
    };

    let _ = state
        .agent_broadcast
        .send(AgentWsEvent::Update { session: dto });

    StatusCode::OK
}

async fn agent_activity_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_agent_activity_ws(state, socket))
        .into_response()
}

async fn handle_agent_activity_ws(state: AppState, mut socket: WebSocket) {
    let snapshot = {
        let sessions = state.agent_sessions.lock().await;
        let dtos: Vec<AgentSessionDto> = sessions
            .values()
            .map(|session| AgentSessionDto {
                cwd: session.cwd.clone(),
                state: match session.state {
                    AgentState::Working => "working".to_owned(),
                    AgentState::Waiting => "waiting".to_owned(),
                },
                updated_at_unix_ms: session.updated_at_unix_ms,
            })
            .collect();
        AgentWsEvent::Snapshot { sessions: dtos }
    };

    if send_ws_json(&mut socket, &snapshot).await.is_err() {
        return;
    }

    let mut rx = state.agent_broadcast.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                if send_ws_json(&mut socket, &event).await.is_err() {
                    break;
                }
            },
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn send_ws_json(socket: &mut WebSocket, value: &impl Serialize) -> Result<(), ()> {
    let payload = serde_json::to_string(value).map_err(|_| ())?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}

/// Dynamic SPA fallback: serves `index.html` if it exists (for client-side
/// routing), otherwise returns a helpful "not built" message. Checked
/// per-request so a long-running process picks up assets installed later.
async fn web_ui_spa_or_unavailable() -> Response {
    let index_path = arbor_web_ui::dist_index_path();
    if index_path.is_file()
        && let Ok(body) = tokio::fs::read(&index_path).await
        && let Ok(response) = Response::builder()
            .header("content-type", "text/html; charset=utf-8")
            .body(axum::body::Body::from(body))
    {
        return response;
    }
    web_ui_unavailable_response()
}

fn web_ui_unavailable_response() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("content-type", "text/html; charset=utf-8")
        .body(axum::body::Body::from(
            "<h1>Arbor Web UI assets are not built</h1>\
             <p>Run npm install && npm run build in crates/arbor-web-ui/app.</p>",
        ))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn ensure_web_ui_assets() -> Result<(), String> {
    if arbor_web_ui::dist_is_built() {
        return Ok(());
    }

    // Only attempt an automatic npm build during development, when the source
    // directory exists. In release/packaged builds the assets should already
    // be bundled alongside the binary.
    let app_dir = arbor_web_ui::app_dir();
    if !app_dir.join("package.json").is_file() {
        return Err("web-ui assets not found (packaged build without bundled assets?)".to_owned());
    }

    let package_manager = detect_npm_binary()
        .ok_or_else(|| "`npm` is not installed or not in PATH; skipping web-ui build".to_owned())?;

    let install_args = if app_dir.join("package-lock.json").exists() {
        vec!["ci", "--no-audit", "--no-fund"]
    } else {
        vec!["install", "--no-audit", "--no-fund"]
    };

    run_command(package_manager, &install_args, &app_dir)?;
    run_command(package_manager, &["run", "build"], &app_dir)?;

    if arbor_web_ui::dist_is_built() {
        return Ok(());
    }

    Err(format!(
        "web-ui build completed but `{}` is missing",
        arbor_web_ui::dist_index_path().display()
    ))
}

fn run_command(program: &str, args: &[&str], cwd: &Path) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|error| {
            format!(
                "failed to run `{program} {}` in `{}`: {error}",
                args.join(" "),
                cwd.display()
            )
        })?;

    if status.success() {
        return Ok(());
    }

    Err(format!(
        "command `{program} {}` failed with status {status}",
        args.join(" "),
    ))
}

fn detect_npm_binary() -> Option<&'static str> {
    let status = Command::new("npm").arg("--version").status();
    if status.ok().is_some_and(|status| status.success()) {
        return Some("npm");
    }

    None
}

fn parse_terminal_signal(raw: &str) -> Option<TerminalSignal> {
    if raw.eq_ignore_ascii_case("interrupt") {
        return Some(TerminalSignal::Interrupt);
    }
    if raw.eq_ignore_ascii_case("terminate") {
        return Some(TerminalSignal::Terminate);
    }
    if raw.eq_ignore_ascii_case("kill") {
        return Some(TerminalSignal::Kill);
    }

    None
}

fn repository_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn short_branch(value: &str) -> String {
    worktree::short_branch(value)
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    worktree::paths_equivalent(left, right)
}

fn github_repo_slug_for_path(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout);
    github_repo_slug_from_remote_url(url.trim())
}

fn github_repo_slug_cached(
    cache: &mut HashMap<String, RepoCacheEntry>,
    repo_root: &Path,
) -> (Option<String>, Option<String>) {
    let key = repo_root.display().to_string();
    if let Some(entry) = cache.get(&key)
        && entry.fetched_at.elapsed().as_secs() < REPO_CACHE_TTL_SECS
    {
        return (entry.github_repo_slug.clone(), entry.avatar_url.clone());
    }
    let slug = github_repo_slug_for_path(repo_root);
    let avatar_url = slug.as_deref().and_then(github_avatar_url);
    cache.insert(key, RepoCacheEntry {
        github_repo_slug: slug.clone(),
        avatar_url: avatar_url.clone(),
        fetched_at: std::time::Instant::now(),
    });
    (slug, avatar_url)
}

fn github_repo_slug_from_remote_url(remote_url: &str) -> Option<String> {
    let path = remote_url
        .strip_prefix("git@github.com:")
        .or_else(|| remote_url.strip_prefix("https://github.com/"))
        .or_else(|| remote_url.strip_prefix("http://github.com/"))
        .or_else(|| remote_url.strip_prefix("ssh://git@github.com/"))?;

    let normalized = path.trim_end_matches('/');
    let repo_path = normalized.strip_suffix(".git").unwrap_or(normalized);
    let (owner, repo_name) = repo_path.split_once('/')?;
    if owner.is_empty() || repo_name.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo_name}"))
}

fn github_avatar_url(repo_slug: &str) -> Option<String> {
    let (owner, _) = repo_slug.split_once('/')?;
    Some(format!(
        "https://avatars.githubusercontent.com/{owner}?size=96"
    ))
}

/// Cached wrapper around `lookup_pr_for_branch`.
async fn lookup_pr_cached(
    cache: Arc<Mutex<HashMap<String, PrCacheEntry>>>,
    github_service: Arc<dyn GitHubPrService>,
    repo_slug: Option<&str>,
    branch: &str,
    is_primary: bool,
) -> (Option<u64>, Option<String>) {
    let slug = match repo_slug {
        Some(s) => s,
        None => return (None, None),
    };

    let cache_key = format!("{slug}:{branch}");

    // Check cache
    {
        let cache_map = cache.lock().await;
        if let Some(entry) = cache_map.get(&cache_key)
            && entry.fetched_at.elapsed().as_secs() < PR_CACHE_TTL_SECS
        {
            return (entry.pr_number, entry.pr_url.clone());
        }
    }

    let (pr_number, pr_url) = github_service
        .lookup_pr_for_branch(repo_slug.map(str::to_owned), branch.to_owned(), is_primary)
        .await;

    // Store in cache
    {
        let mut cache_map = cache.lock().await;
        cache_map.insert(cache_key, PrCacheEntry {
            pr_number,
            pr_url: pr_url.clone(),
            fetched_at: std::time::Instant::now(),
        });
    }

    (pr_number, pr_url)
}

fn internal_error(message: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            error: message.into(),
        }),
    )
}

fn bad_request_error(message: String) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: message }))
}

fn not_found_error(message: String) -> (StatusCode, Json<ApiError>) {
    (StatusCode::NOT_FOUND, Json(ApiError { error: message }))
}

fn map_daemon_error(error: LocalTerminalDaemonError) -> (StatusCode, Json<ApiError>) {
    match error {
        LocalTerminalDaemonError::SessionNotFound { session_id } => {
            not_found_error(format!("terminal session `{session_id}` was not found"))
        },
        LocalTerminalDaemonError::Message { message } => internal_error(message),
        LocalTerminalDaemonError::SessionStore(store_error) => {
            internal_error(store_error.to_string())
        },
    }
}

// ── Process management handlers ──────────────────────────────────────

async fn list_processes(State(state): State<AppState>) -> ApiResult<Vec<ProcessInfo>> {
    let pm = state.process_manager.lock().await;
    Ok(Json(pm.list_processes()))
}

async fn start_all_processes(State(state): State<AppState>) -> ApiResult<Vec<ProcessInfo>> {
    let mut pm = state.process_manager.lock().await;
    let mut daemon = state.daemon.lock().await;
    let results = pm.start_all(&mut *daemon);
    let infos: Vec<ProcessInfo> = results
        .into_iter()
        .filter_map(|(_, result)| result.ok())
        .collect();
    Ok(Json(infos))
}

async fn stop_all_processes(State(state): State<AppState>) -> ApiResult<Vec<ProcessInfo>> {
    let mut pm = state.process_manager.lock().await;
    let mut daemon = state.daemon.lock().await;
    let results = pm.stop_all(&mut *daemon);
    let infos: Vec<ProcessInfo> = results
        .into_iter()
        .filter_map(|(_, result)| result.ok())
        .collect();
    Ok(Json(infos))
}

async fn start_process(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<ProcessInfo> {
    let mut pm = state.process_manager.lock().await;
    let mut daemon = state.daemon.lock().await;
    let info = pm
        .start_process(&name, &mut *daemon)
        .map_err(internal_error)?;
    Ok(Json(info))
}

async fn stop_process(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<ProcessInfo> {
    let mut pm = state.process_manager.lock().await;
    let mut daemon = state.daemon.lock().await;
    let info = pm
        .stop_process(&name, &mut *daemon)
        .map_err(internal_error)?;
    Ok(Json(info))
}

async fn restart_process(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<ProcessInfo> {
    let mut pm = state.process_manager.lock().await;
    let mut daemon = state.daemon.lock().await;
    let info = pm
        .restart_process(&name, &mut *daemon)
        .map_err(internal_error)?;
    Ok(Json(info))
}

async fn process_status_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_process_status_ws(state, socket))
        .into_response()
}

async fn handle_process_status_ws(state: AppState, mut socket: WebSocket) {
    let (snapshot, mut rx) = {
        let pm = state.process_manager.lock().await;
        (pm.snapshot_event(), pm.subscribe())
    };

    if send_ws_json(&mut socket, &snapshot).await.is_err() {
        return;
    }

    loop {
        match rx.recv().await {
            Ok(event) => {
                if send_ws_json(&mut socket, &event).await.is_err() {
                    break;
                }
            },
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}
