mod repository_store;
mod terminal_daemon;

use {
    crate::terminal_daemon::{LocalTerminalDaemon, LocalTerminalDaemonError, SessionEvent},
    arbor_core::{
        daemon::{
            CreateOrAttachRequest, DaemonSessionRecord, DetachRequest, JsonDaemonSessionStore,
            KillRequest, ResizeRequest, SignalRequest, SnapshotRequest, TerminalDaemon,
            TerminalSignal, TerminalSnapshot, WriteRequest,
        },
        worktree,
    },
    axum::{
        Json, Router,
        extract::{
            Path as AxumPath, Query, State,
            ws::{Message, WebSocket, WebSocketUpgrade},
        },
        http::StatusCode,
        response::{Html, IntoResponse, Response},
        routing::{delete, get, post},
    },
    futures_util::StreamExt,
    serde::{Deserialize, Serialize},
    std::{
        net::SocketAddr,
        path::{Path, PathBuf},
        process::Command,
        sync::Arc,
    },
    tokio::sync::Mutex,
    tower_http::services::{ServeDir, ServeFile},
};

#[derive(Clone)]
struct AppState {
    repository_store_path: PathBuf,
    daemon: Arc<Mutex<LocalTerminalDaemon>>,
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
}

#[derive(Debug, Serialize)]
struct RepositoryDto {
    root: String,
    label: String,
}

#[derive(Debug, Serialize)]
struct WorktreeDto {
    repo_root: String,
    path: String,
    branch: String,
    is_primary_checkout: bool,
}

#[derive(Debug, Deserialize)]
struct WorktreeQuery {
    repo_root: Option<String>,
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
    let state = AppState {
        repository_store_path: repository_store::default_repository_store_path(),
        daemon: Arc::new(Mutex::new(LocalTerminalDaemon::new(daemon_store))),
    };

    let app = router(
        state,
        web_ui_result.is_ok() && arbor_web_ui::dist_is_built(),
    );

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    println!("arbor-httpd listening on http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;

    Ok(())
}

fn resolve_bind_addr() -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let raw = std::env::var("ARBOR_HTTPD_BIND").unwrap_or_else(|_| "0.0.0.0:8787".to_owned());
    let parsed: SocketAddr = raw.parse()?;
    Ok(parsed)
}

fn router(state: AppState, web_ui_available: bool) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/repositories", get(list_repositories))
        .route("/worktrees", get(list_worktrees))
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
        .route("/terminals/{session_id}/ws", get(terminal_ws));

    let with_state = Router::new().nest("/api/v1", api).with_state(state);

    if !web_ui_available {
        return with_state.fallback(web_ui_unavailable);
    }

    let dist_dir = arbor_web_ui::dist_dir();
    let index_path = arbor_web_ui::dist_index_path();
    with_state
        .fallback_service(ServeDir::new(dist_dir).not_found_service(ServeFile::new(index_path)))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn list_repositories(State(state): State<AppState>) -> ApiResult<Vec<RepositoryDto>> {
    let roots = repository_store::load_repository_roots(&state.repository_store_path)
        .map_err(internal_error)?;
    let resolved = repository_store::resolve_repository_roots(roots);

    let repositories = resolved
        .into_iter()
        .map(|root| RepositoryDto {
            label: repository_display_name(&root),
            root: root.display().to_string(),
        })
        .collect();

    Ok(Json(repositories))
}

async fn list_worktrees(
    State(state): State<AppState>,
    Query(query): Query<WorktreeQuery>,
) -> ApiResult<Vec<WorktreeDto>> {
    let roots = repository_store::load_repository_roots(&state.repository_store_path)
        .map_err(internal_error)?;
    let resolved = repository_store::resolve_repository_roots(roots);
    let filter = query.repo_root.as_deref().map(PathBuf::from);

    let mut worktrees = Vec::new();

    for repository_root in resolved {
        if let Some(filter_root) = filter.as_ref()
            && !paths_equivalent(&repository_root, filter_root)
        {
            continue;
        }

        match worktree::list(&repository_root) {
            Ok(entries) => {
                for entry in entries {
                    worktrees.push(WorktreeDto {
                        repo_root: repository_root.display().to_string(),
                        path: entry.path.display().to_string(),
                        branch: entry
                            .branch
                            .as_deref()
                            .map(short_branch)
                            .unwrap_or_else(|| "-".to_owned()),
                        is_primary_checkout: entry.path.as_path() == repository_root,
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

    worktrees.sort_by(|left, right| {
        left.repo_root
            .cmp(&right.repo_root)
            .then_with(|| left.path.cmp(&right.path))
    });

    Ok(Json(worktrees))
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

async fn web_ui_unavailable() -> (StatusCode, Html<&'static str>) {
    (
        StatusCode::NOT_FOUND,
        Html(
            "<h1>Arbor Web UI assets are not built</h1><p>Run npm install && npm run build in crates/arbor-web-ui/app.</p>",
        ),
    )
}

fn ensure_web_ui_assets() -> Result<(), String> {
    if arbor_web_ui::dist_is_built() {
        return Ok(());
    }

    let app_dir = arbor_web_ui::app_dir();
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
    value
        .strip_prefix("refs/heads/")
        .unwrap_or(value)
        .to_owned()
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    let left_canonical = left.canonicalize().ok();
    let right_canonical = right.canonicalize().ok();

    left_canonical
        .zip(right_canonical)
        .is_some_and(|(left, right)| left == right)
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
