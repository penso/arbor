mod auth;
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
    arbor_daemon_client::{
        AgentSessionDto, ChangedFileDto, CommitWorktreeRequest, CreateTerminalRequest,
        CreateTerminalResponse, CreateWorktreeRequest, DeleteWorktreeRequest, GitActionResponse,
        HealthResponse, PushWorktreeRequest, RepositoryDto, TerminalResizeRequest,
        TerminalSignalRequest, WorktreeDto, WorktreeMutationResponse,
    },
    axum::{
        Json, Router,
        body::Bytes,
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

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
}

type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;
type ApiResponse = Result<Response, (StatusCode, Json<ApiError>)>;

#[derive(Debug, Deserialize)]
struct WorktreeQuery {
    repo_root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChangesQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
struct SnapshotQuery {
    max_lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum WsClientEvent {
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
    Exit {
        state: arbor_core::daemon::TerminalSessionState,
        exit_code: Option<i32>,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct DaemonConfig {
    auth_token: Option<String>,
    bind: Option<String>,
}

fn daemon_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join(".config/arbor/config.toml")
}

fn load_daemon_config() -> DaemonConfig {
    let path = daemon_config_path();
    if !path.exists() {
        return DaemonConfig::default();
    }
    let settings = config::Config::builder()
        .add_source(config::File::from(path.as_path()).required(false))
        .build();
    match settings {
        Ok(s) => {
            // Try to extract just the [daemon] section
            let mut config = s.get::<DaemonConfig>("daemon").unwrap_or_default();
            config.auth_token = normalize_daemon_auth_token(config.auth_token);
            config
        },
        Err(_) => DaemonConfig::default(),
    }
}

fn ensure_auth_token(config: &mut DaemonConfig) {
    config.auth_token = normalize_daemon_auth_token(config.auth_token.take());
    if config.auth_token.is_some() {
        return;
    }

    use rand::Rng;
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let path = daemon_config_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Read existing file or start empty
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    match content.parse::<toml_edit::DocumentMut>() {
        Ok(mut doc) => {
            let daemon_table = doc
                .entry("daemon")
                .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut();
            if let Some(table) = daemon_table {
                table.insert("auth_token", toml_edit::value(&token));
            }
            if let Err(e) = std::fs::write(&path, doc.to_string()) {
                eprintln!(
                    "warning: failed to write auth token to {}: {e}",
                    path.display()
                );
            }
        },
        Err(e) => {
            eprintln!("warning: failed to parse {}: {e}", path.display());
        },
    }

    println!("generated daemon auth token: {token}");
    config.auth_token = Some(token);
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let web_ui_result = ensure_web_ui_assets();
    if let Err(error) = &web_ui_result {
        eprintln!("web-ui build skipped: {error}");
    }

    // Load daemon config and ensure auth token exists
    let mut daemon_config = load_daemon_config();
    ensure_auth_token(&mut daemon_config);
    let bind_addr = resolve_bind_addr(
        daemon_config.auth_token.as_deref(),
        daemon_config.bind.as_deref(),
    )?;
    let auth_state = auth::AuthState::new(daemon_config.auth_token);

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
    let app = auth::with_auth(router(state), auth_state);

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

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { shutdown_signal.notified().await })
    .await?;

    Ok(())
}

fn resolve_bind_addr(
    auth_token: Option<&str>,
    configured_bind: Option<&str>,
) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    if let Ok(raw) = std::env::var("ARBOR_HTTPD_BIND") {
        let parsed: SocketAddr = raw.parse()?;
        return Ok(parsed);
    }

    let port = match std::env::var("ARBOR_HTTPD_PORT") {
        Ok(raw) => raw.parse::<u16>()?,
        Err(_) => 8787,
    };

    Ok(configured_bind_addr(configured_bind, auth_token, port))
}

fn configured_bind_addr(
    configured_bind: Option<&str>,
    auth_token: Option<&str>,
    port: u16,
) -> SocketAddr {
    match configured_bind.and_then(parse_bind_host) {
        Some(host) => format!("{host}:{port}")
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], port))),
        None => default_bind_addr(auth_token, port),
    }
}

fn parse_bind_host(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "localhost" | "local" | "loopback" | "127.0.0.1" => Some("127.0.0.1"),
        "all" | "all-interfaces" | "public" | "0.0.0.0" => Some("0.0.0.0"),
        _ => None,
    }
}

fn default_bind_addr(auth_token: Option<&str>, port: u16) -> SocketAddr {
    let host = if auth_token.is_some_and(|token| !token.trim().is_empty()) {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };

    format!("{host}:{port}")
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], port)))
}

fn normalize_daemon_auth_token(raw: Option<String>) -> Option<String> {
    raw.and_then(|token| {
        let trimmed = token.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_owned())
    })
}

fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/repositories", get(list_repositories))
        .route("/worktrees", get(list_worktrees).post(create_worktree))
        .route("/worktrees/delete", post(delete_worktree))
        .route("/worktrees/changes", get(list_worktree_changes))
        .route("/worktrees/commit", post(commit_worktree))
        .route("/worktrees/push", post(push_worktree))
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
        .route("/agent/activity", get(list_agent_activity))
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
        status: "ok".to_owned(),
        version: HTTPD_VERSION.to_owned(),
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

async fn create_worktree(
    Json(request): Json<CreateWorktreeRequest>,
) -> ApiResult<WorktreeMutationResponse> {
    let repo_root = PathBuf::from(&request.repo_root);
    let worktree_path = PathBuf::from(&request.path);
    let branch = request
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    if paths_equivalent(&repo_root, &worktree_path) {
        return Err(internal_error(
            "refusing to create a worktree over the primary checkout",
        ));
    }

    if worktree_path.exists() {
        return Err(internal_error(format!(
            "worktree path already exists: {}",
            worktree_path.display()
        )));
    }

    let Some(parent) = worktree_path.parent() else {
        return Err(internal_error("invalid worktree path"));
    };
    std::fs::create_dir_all(parent).map_err(|error| {
        internal_error(format!(
            "failed to create worktree parent directory `{}`: {error}",
            parent.display()
        ))
    })?;

    worktree::add(&repo_root, &worktree_path, worktree::AddWorktreeOptions {
        branch: branch.as_deref(),
        detach: request.detach.unwrap_or(false),
        force: request.force.unwrap_or(false),
    })
    .map_err(|error| internal_error(format!("failed to create worktree: {error}")))?;

    let resolved_branch = git_branch_name_for_worktree(&worktree_path).ok();
    Ok(Json(WorktreeMutationResponse {
        repo_root: repo_root.display().to_string(),
        path: worktree_path.display().to_string(),
        branch: resolved_branch,
        deleted_branch: None,
        message: format!("created worktree at {}", worktree_path.display()),
    }))
}

async fn delete_worktree(
    Json(request): Json<DeleteWorktreeRequest>,
) -> ApiResult<WorktreeMutationResponse> {
    let repo_root = PathBuf::from(&request.repo_root);
    let worktree_path = PathBuf::from(&request.path);

    if paths_equivalent(&repo_root, &worktree_path) {
        return Err(internal_error("refusing to delete the primary checkout"));
    }

    let branch = git_branch_name_for_worktree(&worktree_path).ok();
    worktree::remove(&repo_root, &worktree_path, request.force.unwrap_or(false))
        .map_err(|error| internal_error(format!("failed to delete worktree: {error}")))?;

    let deleted_branch = if request.delete_branch.unwrap_or(false)
        && branch.as_deref().is_some_and(|name| !name.is_empty())
    {
        let branch_name = branch.clone().unwrap_or_default();
        worktree::delete_branch(&repo_root, &branch_name).map_err(|error| {
            internal_error(format!("failed to delete branch `{branch_name}`: {error}"))
        })?;
        Some(branch_name)
    } else {
        None
    };

    Ok(Json(WorktreeMutationResponse {
        repo_root: repo_root.display().to_string(),
        path: worktree_path.display().to_string(),
        branch,
        deleted_branch,
        message: format!("deleted worktree at {}", worktree_path.display()),
    }))
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

async fn commit_worktree(
    Json(request): Json<CommitWorktreeRequest>,
) -> ApiResult<GitActionResponse> {
    let worktree_path = PathBuf::from(&request.path);
    let changed_files = changes::changed_files(&worktree_path).map_err(|error| {
        internal_error(format!(
            "failed to gather changed files for `{}`: {error}",
            worktree_path.display()
        ))
    })?;

    let commit_message =
        run_git_commit_for_worktree(&worktree_path, &changed_files, request.message.as_deref())
            .map_err(internal_error)?;

    Ok(Json(GitActionResponse {
        path: worktree_path.display().to_string(),
        branch: git_branch_name_for_worktree(&worktree_path).ok(),
        message: "commit complete".to_owned(),
        commit_message: Some(commit_message),
    }))
}

async fn push_worktree(Json(request): Json<PushWorktreeRequest>) -> ApiResult<GitActionResponse> {
    let worktree_path = PathBuf::from(&request.path);
    let push_message = run_git_push_for_worktree(&worktree_path).map_err(internal_error)?;

    Ok(Json(GitActionResponse {
        path: worktree_path.display().to_string(),
        branch: git_branch_name_for_worktree(&worktree_path).ok(),
        message: push_message,
        commit_message: None,
    }))
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
    body: Bytes,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .write(WriteRequest {
            session_id,
            bytes: body.to_vec(),
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
    Query(query): Query<HashMap<String, String>>,
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

    let initial_size = match (query.get("cols"), query.get("rows")) {
        (Some(cols_str), Some(rows_str)) => {
            if let (Ok(cols), Ok(rows)) = (cols_str.parse::<u16>(), rows_str.parse::<u16>()) {
                if cols > 0 && rows > 0 {
                    Some((cols, rows))
                } else {
                    None
                }
            } else {
                None
            }
        },
        _ => None,
    };

    Ok(ws
        .on_upgrade(move |socket| handle_terminal_ws(state, session_id, socket, initial_size))
        .into_response())
}

async fn handle_terminal_ws(
    state: AppState,
    session_id: String,
    mut socket: WebSocket,
    initial_size: Option<(u16, u16)>,
) {
    let (ansi_output, snapshot_state, snapshot_exit_code, snapshot_updated_at, mut subscription) = {
        let mut daemon = state.daemon.lock().await;

        // Resize the PTY before generating the snapshot so the emulator
        // reflows content to the client's actual terminal dimensions.
        if let Some((cols, rows)) = initial_size {
            let _ = daemon.resize(ResizeRequest {
                session_id: session_id.clone(),
                cols,
                rows,
            });
        }

        let snapshot = match daemon.snapshot(SnapshotRequest {
            session_id: session_id.clone(),
            max_lines: 1,
        }) {
            Ok(Some(snapshot)) => snapshot,
            _ => return,
        };

        // Render the emulator's visual state to ANSI instead of replaying
        // raw output_tail bytes which were captured at potentially different
        // terminal dimensions.
        let ansi_output = daemon
            .render_ansi_snapshot(&session_id, 180)
            .ok()
            .flatten()
            .unwrap_or_default();

        let subscription = match daemon.subscribe(&session_id) {
            Ok(subscription) => subscription,
            Err(_) => return,
        };

        (
            ansi_output,
            snapshot.state,
            snapshot.exit_code,
            snapshot.updated_at_unix_ms,
            subscription,
        )
    };

    if send_ws_event(&mut socket, WsServerEvent::Snapshot {
        output_tail: ansi_output,
        state: snapshot_state,
        exit_code: snapshot_exit_code,
        updated_at_unix_ms: snapshot_updated_at,
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
                        if send_ws_binary(&mut socket, &data).await.is_err() {
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

async fn send_ws_binary(socket: &mut WebSocket, payload: &[u8]) -> Result<(), ()> {
    socket
        .send(Message::Binary(payload.to_vec().into()))
        .await
        .map_err(|_| ())
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

async fn list_agent_activity(State(state): State<AppState>) -> ApiResult<Vec<AgentSessionDto>> {
    let mut sessions = state.agent_sessions.lock().await;
    Ok(Json(agent_session_snapshot(&mut sessions)))
}

async fn agent_activity_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_agent_activity_ws(state, socket))
        .into_response()
}

async fn handle_agent_activity_ws(state: AppState, mut socket: WebSocket) {
    let snapshot = {
        let mut sessions = state.agent_sessions.lock().await;
        let dtos = agent_session_snapshot(&mut sessions);
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

fn agent_session_snapshot(sessions: &mut HashMap<String, AgentSession>) -> Vec<AgentSessionDto> {
    let cutoff = current_unix_timestamp_millis().saturating_sub(AGENT_SESSION_EXPIRY_SECS * 1000);
    sessions.retain(|_, session| session.updated_at_unix_ms > cutoff);

    let mut snapshot: Vec<AgentSessionDto> = sessions
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
    snapshot.sort_by(|left, right| left.cwd.cmp(&right.cwd));
    snapshot
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

fn auto_commit_subject(changed_files: &[changes::ChangedFile]) -> String {
    if changed_files.len() == 1 {
        let file_label = changed_files[0]
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| changed_files[0].path.display().to_string());
        return format!("chore: update {file_label}");
    }

    let has_added = changed_files.iter().any(|change| {
        matches!(
            change.kind,
            changes::ChangeKind::Added | changes::ChangeKind::IntentToAdd
        )
    });
    let has_removed = changed_files
        .iter()
        .any(|change| matches!(change.kind, changes::ChangeKind::Removed));
    let has_renamed = changed_files
        .iter()
        .any(|change| matches!(change.kind, changes::ChangeKind::Renamed));
    let verb = if has_added && !has_removed && !has_renamed {
        "add"
    } else if has_removed && !has_added && !has_renamed {
        "remove"
    } else if has_renamed && !has_added && !has_removed {
        "rename"
    } else {
        "update"
    };

    format!("chore: {verb} {} files", changed_files.len())
}

fn auto_commit_body(changed_files: &[changes::ChangedFile]) -> String {
    let mut lines = vec!["Auto-generated by Arbor.".to_owned(), String::new()];

    for change in changed_files.iter().take(12) {
        let mut line = format!("- {} {}", change_code(change.kind), change.path.display());
        if change.additions > 0 || change.deletions > 0 {
            line.push_str(&format!(" (+{} -{})", change.additions, change.deletions));
        }
        lines.push(line);
    }

    if changed_files.len() > 12 {
        lines.push(format!("- ... and {} more", changed_files.len() - 12));
    }

    lines.join("\n")
}

fn change_code(kind: changes::ChangeKind) -> &'static str {
    match kind {
        changes::ChangeKind::Added => "A",
        changes::ChangeKind::Modified => "M",
        changes::ChangeKind::Removed => "D",
        changes::ChangeKind::Renamed => "R",
        changes::ChangeKind::Copied => "C",
        changes::ChangeKind::TypeChange => "T",
        changes::ChangeKind::Conflict => "U",
        changes::ChangeKind::IntentToAdd => "I",
    }
}

fn run_git_commit_for_worktree(
    worktree_path: &Path,
    changed_files: &[changes::ChangedFile],
    message_override: Option<&str>,
) -> Result<String, String> {
    if changed_files.is_empty() {
        return Err("nothing to commit".to_owned());
    }

    let repo = git2::Repository::open(worktree_path).map_err(|error| {
        format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        )
    })?;

    let mut index = repo
        .index()
        .map_err(|error| format!("failed to read index: {error}"))?;
    index
        .add_all(["."], git2::IndexAddOption::DEFAULT, None)
        .map_err(|error| format!("failed to stage changes: {error}"))?;
    index
        .update_all(["."], None)
        .map_err(|error| format!("failed to update index: {error}"))?;
    index
        .write()
        .map_err(|error| format!("failed to write index: {error}"))?;

    let tree_oid = index
        .write_tree()
        .map_err(|error| format!("failed to write tree: {error}"))?;
    let tree = repo
        .find_tree(tree_oid)
        .map_err(|error| format!("failed to find tree: {error}"))?;

    if let Ok(head) = repo.head()
        && let Ok(head_commit) = head.peel_to_commit()
        && head_commit.tree_id() == tree_oid
    {
        return Err("nothing to commit".to_owned());
    }

    let message = message_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let subject = auto_commit_subject(changed_files);
            let body = auto_commit_body(changed_files);
            format!("{subject}\n\n{body}")
        });

    let signature = repo
        .signature()
        .map_err(|error| format!("failed to create signature: {error}"))?;
    let parent_commits: Vec<git2::Commit<'_>> = match repo.head() {
        Ok(head) => match head.peel_to_commit() {
            Ok(commit) => vec![commit],
            Err(_) => vec![],
        },
        Err(_) => vec![],
    };
    let parents: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        &message,
        &tree,
        &parents,
    )
    .map_err(|error| format!("failed to create commit: {error}"))?;

    Ok(message)
}

fn run_git_push_for_worktree(worktree_path: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(worktree_path).map_err(|error| {
        format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        )
    })?;

    let head_ref = repo
        .head()
        .map_err(|error| format!("failed to read HEAD: {error}"))?;
    let branch_name = head_ref
        .shorthand()
        .ok_or_else(|| "cannot push detached HEAD".to_owned())?
        .to_owned();
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");

    let mut remote = repo
        .find_remote("origin")
        .map_err(|error| format!("failed to find remote `origin`: {error}"))?;

    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(|_url, username_from_url, allowed_types| {
        if allowed_types.contains(git2::CredentialType::SSH_KEY) {
            let username = username_from_url.unwrap_or("git");
            git2::Cred::ssh_key_from_agent(username)
        } else if allowed_types.contains(git2::CredentialType::DEFAULT) {
            git2::Cred::default()
        } else {
            Err(git2::Error::from_str(
                "no suitable credential type available",
            ))
        }
    });

    let mut push_options = git2::PushOptions::new();
    push_options.remote_callbacks(callbacks);

    remote
        .push(&[&refspec], Some(&mut push_options))
        .map_err(|error| format!("push failed: {error}"))?;

    let mut config = repo
        .config()
        .map_err(|error| format!("failed to read config: {error}"))?;
    let _ = config.set_str(&format!("branch.{branch_name}.remote"), "origin");
    let _ = config.set_str(
        &format!("branch.{branch_name}.merge"),
        &format!("refs/heads/{branch_name}"),
    );

    Ok(format!(
        "push complete: {branch_name} -> origin/{branch_name}"
    ))
}

fn git_branch_name_for_worktree(worktree_path: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(worktree_path).map_err(|error| {
        format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        )
    })?;

    let head_ref = repo
        .head()
        .map_err(|error| format!("failed to read HEAD: {error}"))?;

    head_ref
        .shorthand()
        .map(str::to_owned)
        .ok_or_else(|| "worktree has detached HEAD".to_owned())
}

fn short_branch(value: &str) -> String {
    worktree::short_branch(value)
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    worktree::paths_equivalent(left, right)
}

fn current_unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use {super::*, crate::repository_store::JsonRepositoryStore, std::time::Duration};

    #[tokio::test]
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

    #[test]
    fn default_bind_addr_uses_public_interface_only_when_auth_is_enabled() {
        assert_eq!(
            default_bind_addr(Some("secret-token"), 8787),
            SocketAddr::from(([0, 0, 0, 0], 8787))
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
            SocketAddr::from(([0, 0, 0, 0], 8787))
        );
    }

    fn test_app_state(repo_root: PathBuf) -> AppState {
        let daemon_store = JsonDaemonSessionStore::new(repo_root.join("daemon-sessions.json"));
        let repository_store = Arc::new(JsonRepositoryStore::new(
            repo_root.join("repositories.json"),
        ));
        let (agent_broadcast, _) = tokio::sync::broadcast::channel(16);

        AppState {
            repository_store,
            daemon: Arc::new(Mutex::new(LocalTerminalDaemon::new(daemon_store))),
            process_manager: Arc::new(Mutex::new(ProcessManager::new(repo_root))),
            github_service: github_service::default_github_pr_service(),
            agent_sessions: Arc::new(Mutex::new(HashMap::new())),
            agent_broadcast,
            pr_cache: Arc::new(Mutex::new(HashMap::new())),
            repo_cache: Arc::new(Mutex::new(HashMap::new())),
            shutdown_signal: Arc::new(tokio::sync::Notify::new()),
        }
    }

    async fn create_raw_echo_session(state: &AppState, session_id: &str) -> String {
        let cwd = match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(error) => panic!("failed to read current directory: {error}"),
        };
        let response = {
            let mut daemon = state.daemon.lock().await;
            daemon.create_or_attach(CreateOrAttachRequest {
                session_id: session_id.to_owned(),
                workspace_id: cwd.display().to_string(),
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
        response.session.session_id
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
                session_id: session_id.to_owned(),
            })
        };

        if let Err(error) = result {
            panic!("failed to kill test session `{session_id}`: {error}");
        }
    }
}
