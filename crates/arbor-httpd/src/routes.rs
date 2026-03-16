use {
    crate::{
        error::*,
        github_service::GitHubPrService,
        issue_linking::{self, LinkedIssueWorktree},
        managed_worktree::preview_managed_worktree as build_managed_worktree_preview,
        process_manager::ProcessEvent,
        repository_store, task_scheduler,
        terminal_daemon::{LocalTerminalDaemonError, SessionEvent, TerminalActivityEvent},
        types::*,
    },
    arbor_core::{
        SessionId,
        agent::AgentState,
        changes,
        daemon::{
            CreateOrAttachRequest, DaemonSessionRecord, DetachRequest, KillRequest, ResizeRequest,
            SignalRequest, SnapshotRequest, TerminalDaemon, TerminalSignal, TerminalSnapshot,
            WriteRequest,
        },
        process::ProcessInfo,
        repo_config,
        task::{TaskExecution, TaskInfo},
        worktree,
        worktree_scripts::{WorktreeScriptContext, WorktreeScriptPhase, run_worktree_scripts},
    },
    arbor_daemon_client::{
        AgentSessionDto, ChangedFileDto, CommitWorktreeRequest, CreateManagedWorktreeRequest,
        CreateTerminalRequest, CreateTerminalResponse, CreateWorktreeRequest,
        DeleteWorktreeRequest, GitActionResponse, HealthResponse, IssueListResponse,
        ManagedWorktreePreviewRequest, ManagedWorktreePreviewResponse, PushWorktreeRequest,
        RepositoryDto, TerminalResizeRequest, TerminalSignalRequest, WorktreeDto,
        WorktreeMutationResponse,
    },
    axum::{
        Json, Router,
        body::Bytes,
        extract::{
            Path as AxumPath, Query, State,
            ws::{Message, WebSocket, WebSocketUpgrade},
        },
        handler::HandlerWithoutStateExt,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{delete, get, post},
    },
    futures_util::StreamExt,
    serde::Serialize,
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
        process::Command,
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    },
    tokio::sync::{Mutex, broadcast},
    tower_http::services::ServeDir,
};

// ── Router ──────────────────────────────────────────────────────────

pub(crate) fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/repositories", get(list_repositories))
        .route("/issues", get(list_repository_issues))
        .route("/worktrees", get(list_worktrees).post(create_worktree))
        .route("/worktrees/managed/preview", post(preview_managed_worktree))
        .route("/worktrees/managed", post(create_managed_worktree))
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
        .route("/tasks", get(list_tasks))
        .route("/tasks/{name}/run", post(run_task))
        .route("/tasks/{name}/history", get(task_history))
        .route("/tasks/ws", get(task_status_ws))
        .route("/shutdown", post(shutdown_daemon))
        .route("/config/bind", post(set_bind_mode).get(get_bind_mode))
        .route("/config/theme", get(get_theme))
        .route("/logs/ws", get(logs_ws));

    #[cfg(feature = "agent-chat")]
    let api = api
        .route("/agent/chat", get(list_agent_chats).post(create_agent_chat))
        .route("/agent/chat/{session_id}", delete(kill_agent_chat))
        .route("/agent/chat/{session_id}/send", post(send_agent_message))
        .route("/agent/chat/{session_id}/cancel", post(cancel_agent_chat))
        .route("/agent/chat/{session_id}/history", get(agent_chat_history))
        .route("/agent/chat/{session_id}/ws", get(agent_chat_ws))
        .route("/agent/chat/discover-models", post(discover_models));

    #[cfg(feature = "symphony")]
    let api = api
        .route("/symphony/state", get(symphony_state))
        .route("/symphony/refresh", post(symphony_refresh))
        .route("/symphony/{issue_identifier}", get(symphony_issue));

    let with_state = Router::new().nest("/api/v1", api).with_state(state);

    // Always set up ServeDir — check for assets dynamically per-request so
    // that a long-running daemon picks up assets installed after startup
    // (e.g. an app update while the detached httpd process is still running).
    let dist_dir = arbor_web_ui::dist_dir();
    with_state.fallback_service(
        ServeDir::new(dist_dir).not_found_service(web_ui_spa_or_unavailable.into_service()),
    )
}

// ── Health / shutdown / bind-mode ────────────────────────────────────

pub(crate) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
        version: HTTPD_VERSION.to_owned(),
    })
}

#[cfg(feature = "symphony")]
pub(crate) async fn symphony_state(
    State(state): State<AppState>,
) -> ApiResult<arbor_symphony::RuntimeSnapshot> {
    let Some(service) = state.symphony.clone() else {
        return Err(internal_error("symphony service is disabled"));
    };
    Ok(Json(service.snapshot().await))
}

#[cfg(feature = "symphony")]
pub(crate) async fn symphony_issue(
    State(state): State<AppState>,
    AxumPath(issue_identifier): AxumPath<String>,
) -> ApiResult<arbor_symphony::IssueRuntimeSnapshot> {
    let Some(service) = state.symphony.clone() else {
        return Err(internal_error("symphony service is disabled"));
    };
    service
        .issue_snapshot(&issue_identifier)
        .await
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "issue_not_found".to_owned(),
                }),
            )
        })
}

#[cfg(feature = "symphony")]
pub(crate) async fn symphony_refresh(
    State(state): State<AppState>,
) -> ApiResult<serde_json::Value> {
    let Some(service) = state.symphony.clone() else {
        return Err(internal_error("symphony service is disabled"));
    };
    service
        .refresh()
        .map_err(|e| internal_error(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "queued": true,
        "requested_at": current_unix_timestamp_millis(),
        "operations": ["poll", "reconcile"],
    })))
}

pub(crate) async fn shutdown_daemon(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
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

pub(crate) async fn get_bind_mode(State(state): State<AppState>) -> Json<BindModeResponse> {
    Json(BindModeResponse {
        allow_remote: state
            .auth_state
            .allow_remote
            .load(std::sync::atomic::Ordering::Relaxed),
    })
}

pub(crate) async fn set_bind_mode(
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    State(state): State<AppState>,
    Json(body): Json<SetBindModeRequest>,
) -> Result<Json<BindModeResponse>, (StatusCode, Json<ApiError>)> {
    if !addr.ip().is_loopback() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError {
                error: "bind mode can only be changed from localhost".to_owned(),
            }),
        ));
    }
    state
        .auth_state
        .allow_remote
        .store(body.allow_remote, std::sync::atomic::Ordering::Relaxed);
    eprintln!("bind mode changed: allow_remote={}", body.allow_remote);
    Ok(Json(BindModeResponse {
        allow_remote: body.allow_remote,
    }))
}

pub(crate) async fn get_theme() -> Json<arbor_core::theme::ThemeResponse> {
    let theme_slug = load_theme_slug();
    let theme_kind = theme_slug
        .as_deref()
        .and_then(arbor_core::theme::ThemeKind::from_slug)
        .unwrap_or(arbor_core::theme::ThemeKind::One);
    Json(theme_kind.to_response())
}

fn load_theme_slug() -> Option<String> {
    let path = daemon_config_path();
    if !path.exists() {
        return None;
    }
    let settings = config::Config::builder()
        .add_source(config::File::from(path.as_path()).required(false))
        .build()
        .ok()?;
    settings.get_string("theme").ok().and_then(|s| {
        let trimmed = s.trim().to_owned();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

// ── Repository / worktree routes ─────────────────────────────────────

pub(crate) async fn list_repositories(
    State(state): State<AppState>,
) -> ApiResult<Vec<RepositoryDto>> {
    let roots = state
        .repository_store
        .load_roots()
        .map_err(|e| internal_error(e.to_string()))?;
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

pub(crate) async fn list_repository_issues(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<IssuesQuery>,
) -> ApiResult<IssueListResponse> {
    let repo_root = PathBuf::from(query.repo_root);
    let issue_service = state.issue_service.clone();
    let repo_root_for_issue_fetch = repo_root.clone();
    let github_token = github_token_from_headers(&headers);
    let mut issues = tokio::task::spawn_blocking(move || {
        issue_service.list_repository_issues(&repo_root_for_issue_fetch, github_token)
    })
    .await
    .map_err(|error| internal_error(format!("failed to join issue fetch task: {error}")))?
    .map_err(|e| internal_error(e.to_string()))?;
    if let Err(error) = enrich_issue_list_with_local_links(&state, &repo_root, &mut issues).await {
        tracing::warn!(
            repo_root = %repo_root.display(),
            %error,
            "failed to enrich repository issues with local worktree links"
        );
    }
    Ok(Json(issues))
}

fn github_token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("X-Arbor-GitHub-Token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

async fn enrich_issue_list_with_local_links(
    state: &AppState,
    repo_root: &Path,
    issues_response: &mut IssueListResponse,
) -> Result<(), RouteError> {
    let linked_worktrees = collect_linked_issue_worktrees(state, repo_root).await?;
    issue_linking::enrich_issues_with_worktree_links(
        repo_root,
        &mut issues_response.issues,
        &linked_worktrees,
    );
    Ok(())
}

async fn collect_linked_issue_worktrees(
    state: &AppState,
    repo_root: &Path,
) -> Result<Vec<LinkedIssueWorktree>, RouteError> {
    let entries = worktree::list(repo_root).map_err(|error| {
        RouteError::Internal(format!(
            "failed to list worktrees for `{}`: {error}",
            repo_root.display()
        ))
    })?;

    let repo_slug = {
        let mut repo_cache = state.repo_cache.lock().await;
        let (repo_slug, _) = github_repo_slug_cached(&mut repo_cache, repo_root);
        repo_slug
    };

    let worktrees = issue_worktree_data_from_entries(entries);

    let pr_futures: Vec<_> = worktrees
        .iter()
        .map(|worktree| {
            let cache = state.pr_cache.clone();
            let github_service = state.github_service.clone();
            let repo_slug = repo_slug.clone();
            let branch = worktree.branch.clone();
            async move {
                lookup_pr_cached(cache, github_service, repo_slug.as_deref(), &branch, false).await
            }
        })
        .collect();

    let pr_results = futures_util::future::join_all(pr_futures).await;

    Ok(worktrees
        .into_iter()
        .zip(pr_results)
        .map(|(worktree, (pr_number, pr_url))| LinkedIssueWorktree {
            path: worktree.path,
            branch: worktree.branch,
            pr_number,
            pr_url,
            last_activity_unix_ms: worktree.last_activity_unix_ms,
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IssueWorktreeData {
    path: PathBuf,
    branch: String,
    last_activity_unix_ms: Option<u64>,
}

fn issue_worktree_data_from_entries(entries: Vec<worktree::Worktree>) -> Vec<IssueWorktreeData> {
    entries
        .into_iter()
        .map(|entry| IssueWorktreeData {
            last_activity_unix_ms: worktree::last_git_activity_ms(&entry.path),
            branch: entry
                .branch
                .as_deref()
                .map(short_branch)
                .unwrap_or_else(|| "-".to_owned()),
            path: entry.path,
        })
        .collect()
}

pub(crate) async fn list_worktrees(
    State(state): State<AppState>,
    Query(query): Query<WorktreeQuery>,
) -> ApiResult<Vec<WorktreeDto>> {
    let roots = state
        .repository_store
        .load_roots()
        .map_err(|e| internal_error(e.to_string()))?;
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
        process_definitions: Vec<crate::process_manager::ProcessDefinition>,
    }

    let mut entries_data: Vec<WorktreeData> = Vec::new();
    let all_process_definitions =
        crate::process_manager::discover_process_definitions_for_roots(&resolved);

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
                        let process_definitions =
                            crate::process_manager::discover_process_definitions_for_worktree(
                                repository_root,
                                &entry.path,
                            );
                        entries_data.push(WorktreeData {
                            repo_root: repository_root.display().to_string(),
                            path: entry.path.display().to_string(),
                            branch: branch_name,
                            is_primary,
                            last_activity_unix_ms,
                            diff_additions: diff_summary.as_ref().map(|d| d.additions),
                            diff_deletions: diff_summary.as_ref().map(|d| d.deletions),
                            repo_slug: repo_slug.clone(),
                            process_definitions,
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
    let session_memory_bytes = collect_process_memory_bytes(&state).await;
    let mut process_manager = state.process_manager.lock().await;
    process_manager.sync_definitions(&all_process_definitions);
    let mut worktrees: Vec<WorktreeDto> = entries_data
        .into_iter()
        .zip(pr_results)
        .map(|(wd, (pr_number, pr_url))| {
            let mut processes = process_manager
                .list_processes_for_workspace(Path::new(&wd.path), &wd.process_definitions);
            apply_process_memory(&mut processes, &session_memory_bytes);
            WorktreeDto {
                repo_root: wd.repo_root,
                path: wd.path,
                branch: wd.branch,
                is_primary_checkout: wd.is_primary,
                last_activity_unix_ms: wd.last_activity_unix_ms,
                diff_additions: wd.diff_additions,
                diff_deletions: wd.diff_deletions,
                pr_number,
                pr_url,
                processes,
            }
        })
        .collect();

    worktrees.sort_by(|left, right| {
        left.repo_root
            .cmp(&right.repo_root)
            .then_with(|| left.path.cmp(&right.path))
    });

    Ok(Json(worktrees))
}

pub(crate) async fn create_worktree(
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

    create_worktree_at(
        repo_root,
        worktree_path,
        branch,
        request.detach,
        request.force,
    )
}

pub(crate) async fn preview_managed_worktree(
    Json(request): Json<ManagedWorktreePreviewRequest>,
) -> ApiResult<ManagedWorktreePreviewResponse> {
    let repo_root = PathBuf::from(&request.repo_root);
    let repo_root = worktree::repo_root(&repo_root)
        .map_err(|error| internal_error(format!("failed to resolve repository root: {error}")))?;
    let preview = build_managed_worktree_preview(&repo_root, &request.worktree_name)
        .map_err(|e| internal_error(e.to_string()))?;

    Ok(Json(ManagedWorktreePreviewResponse {
        sanitized_worktree_name: preview.sanitized_worktree_name,
        branch: preview.branch_name,
        path: preview.worktree_path.display().to_string(),
    }))
}

pub(crate) async fn create_managed_worktree(
    Json(request): Json<CreateManagedWorktreeRequest>,
) -> ApiResult<WorktreeMutationResponse> {
    let repo_root = PathBuf::from(&request.repo_root);
    let repo_root = worktree::repo_root(&repo_root)
        .map_err(|error| internal_error(format!("failed to resolve repository root: {error}")))?;
    let preview = build_managed_worktree_preview(&repo_root, &request.worktree_name)
        .map_err(|e| internal_error(e.to_string()))?;

    create_worktree_at(
        repo_root,
        preview.worktree_path,
        Some(preview.branch_name),
        Some(false),
        Some(false),
    )
}

fn create_worktree_at(
    repo_root: PathBuf,
    worktree_path: PathBuf,
    branch: Option<String>,
    detach: Option<bool>,
    force: Option<bool>,
) -> ApiResult<WorktreeMutationResponse> {
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
        detach: detach.unwrap_or(false),
        force: force.unwrap_or(false),
    })
    .map_err(|error| internal_error(format!("failed to create worktree: {error}")))?;

    let resolved_branch = git_branch_name_for_worktree(&worktree_path).ok();
    let script_context =
        WorktreeScriptContext::new(&repo_root, &worktree_path, resolved_branch.as_deref());
    if let Err(error) =
        run_worktree_scripts(&repo_root, WorktreeScriptPhase::Setup, &script_context)
    {
        rollback_created_worktree_http(&repo_root, &worktree_path, branch.as_deref()).map_err(
            |rollback_error| {
                internal_error(format!("{error}. rollback also failed: {rollback_error}"))
            },
        )?;
        return Err(internal_error(error.to_string()));
    }

    Ok(Json(WorktreeMutationResponse {
        repo_root: repo_root.display().to_string(),
        path: worktree_path.display().to_string(),
        branch: resolved_branch,
        deleted_branch: None,
        message: format!("created worktree at {}", worktree_path.display()),
    }))
}

pub(crate) async fn delete_worktree(
    Json(request): Json<DeleteWorktreeRequest>,
) -> ApiResult<WorktreeMutationResponse> {
    let repo_root = PathBuf::from(&request.repo_root);
    let worktree_path = PathBuf::from(&request.path);

    if paths_equivalent(&repo_root, &worktree_path) {
        return Err(internal_error("refusing to delete the primary checkout"));
    }

    let branch = git_branch_name_for_worktree(&worktree_path).ok();
    let script_context = WorktreeScriptContext::new(&repo_root, &worktree_path, branch.as_deref());
    run_worktree_scripts(&repo_root, WorktreeScriptPhase::Teardown, &script_context)
        .map_err(|error| internal_error(error.to_string()))?;
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

fn rollback_created_worktree_http(
    repo_root: &Path,
    worktree_path: &Path,
    created_branch: Option<&str>,
) -> Result<(), RouteError> {
    worktree::remove(repo_root, worktree_path, true)
        .map_err(|error| RouteError::Internal(error.to_string()))?;
    if let Some(branch_name) = created_branch.filter(|value| !value.trim().is_empty()) {
        worktree::delete_branch(repo_root, branch_name).map_err(|error| {
            RouteError::Internal(format!("failed to delete branch `{branch_name}`: {error}"))
        })?;
    }
    Ok(())
}

pub(crate) async fn list_worktree_changes(
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

pub(crate) async fn commit_worktree(
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
            .map_err(|e| internal_error(e.to_string()))?;

    Ok(Json(GitActionResponse {
        path: worktree_path.display().to_string(),
        branch: git_branch_name_for_worktree(&worktree_path).ok(),
        message: "commit complete".to_owned(),
        commit_message: Some(commit_message),
    }))
}

pub(crate) async fn push_worktree(
    Json(request): Json<PushWorktreeRequest>,
) -> ApiResult<GitActionResponse> {
    let worktree_path = PathBuf::from(&request.path);
    let push_message =
        run_git_push_for_worktree(&worktree_path).map_err(|e| internal_error(e.to_string()))?;

    Ok(Json(GitActionResponse {
        path: worktree_path.display().to_string(),
        branch: git_branch_name_for_worktree(&worktree_path).ok(),
        message: push_message,
        commit_message: None,
    }))
}

// ── Terminal routes ──────────────────────────────────────────────────

pub(crate) async fn list_terminals(
    State(state): State<AppState>,
) -> ApiResult<Vec<DaemonSessionRecord>> {
    let daemon = state.daemon.lock().await;
    let mut sessions = daemon.list_sessions().map_err(map_daemon_error)?;

    sessions.sort_by(|left, right| {
        right
            .updated_at_unix_ms
            .unwrap_or(0)
            .cmp(&left.updated_at_unix_ms.unwrap_or(0))
            .then_with(|| left.session_id.as_str().cmp(right.session_id.as_str()))
    });

    Ok(Json(sessions))
}

pub(crate) async fn create_terminal(
    State(state): State<AppState>,
    Json(request): Json<CreateTerminalRequest>,
) -> ApiResult<CreateTerminalResponse> {
    let cwd = PathBuf::from(request.cwd.clone());
    let cols = request.cols.unwrap_or(120);
    let rows = request.rows.unwrap_or(35);
    let workspace_id = request
        .workspace_id
        .unwrap_or_else(|| request.cwd.clone().into());

    tracing::info!(
        session_id = ?request.session_id,
        workspace_id = %workspace_id,
        cwd = %cwd.display(),
        title = ?request.title,
        command = ?request.command,
        "create_terminal request received"
    );

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

    tracing::info!(
        session_id = %response.session.session_id,
        is_new = response.is_new_session,
        "create_terminal response"
    );

    Ok(Json(CreateTerminalResponse {
        is_new_session: response.is_new_session,
        session: response.session,
    }))
}

pub(crate) async fn get_terminal_snapshot(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<SnapshotQuery>,
) -> ApiResult<TerminalSnapshot> {
    let max_lines = query.max_lines.unwrap_or(180).clamp(1, 2000);

    let daemon = state.daemon.lock().await;
    let snapshot = daemon
        .snapshot(SnapshotRequest {
            session_id: session_id.clone().into(),
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

pub(crate) async fn write_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    body: Bytes,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .write(WriteRequest {
            session_id: session_id.into(),
            bytes: body.to_vec(),
        })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn resize_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<TerminalResizeRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .resize(ResizeRequest {
            session_id: session_id.into(),
            cols: request.cols,
            rows: request.rows,
        })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn signal_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<TerminalSignalRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let signal = parse_terminal_signal(&request.signal).ok_or_else(|| {
        bad_request_error("signal must be one of: interrupt, terminate, kill".to_owned())
    })?;

    let mut daemon = state.daemon.lock().await;
    daemon
        .signal(SignalRequest {
            session_id: session_id.into(),
            signal,
        })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn kill_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .kill(KillRequest {
            session_id: session_id.into(),
        })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn detach_terminal(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let mut daemon = state.daemon.lock().await;
    daemon
        .detach(DetachRequest {
            session_id: session_id.into(),
        })
        .map_err(map_daemon_error)?;

    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn terminal_ws(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> ApiResponse {
    {
        let daemon = state.daemon.lock().await;
        let exists = daemon
            .snapshot(SnapshotRequest {
                session_id: session_id.clone().into(),
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
                session_id: session_id.clone().into(),
                cols,
                rows,
            });
        }

        let snapshot = match daemon.snapshot(SnapshotRequest {
            session_id: session_id.clone().into(),
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
                                let _ = send_ws_event(&mut socket, WsServerEvent::Error { message: error.to_string() }).await;
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
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        if send_ws_event(&mut socket, WsServerEvent::Error { message: format!("dropped {skipped} terminal events") }).await.is_err() {
                            break;
                        }
                    },
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    },
                }
            }
        }
    }

    let mut daemon = state.daemon.lock().await;
    let _ = daemon.detach(DetachRequest {
        session_id: session_id.into(),
    });
}

pub(crate) async fn process_ws_client_message(
    state: &AppState,
    session_id: &str,
    message: Message,
) -> Result<bool, WsClientError> {
    match message {
        Message::Text(text) => {
            let parsed = serde_json::from_str::<WsClientEvent>(&text)
                .map_err(|error| WsClientError::InvalidPayload(error.to_string()))?;

            match parsed {
                WsClientEvent::Resize { cols, rows } => {
                    let mut daemon = state.daemon.lock().await;
                    daemon
                        .resize(ResizeRequest {
                            session_id: session_id.into(),
                            cols,
                            rows,
                        })
                        .map_err(|error| WsClientError::Daemon(error.to_string()))?;
                },
                WsClientEvent::Signal { signal } => {
                    let Some(signal) = parse_terminal_signal(&signal) else {
                        return Err(WsClientError::InvalidSignal);
                    };

                    let mut daemon = state.daemon.lock().await;
                    daemon
                        .signal(SignalRequest {
                            session_id: session_id.into(),
                            signal,
                        })
                        .map_err(|error| WsClientError::Daemon(error.to_string()))?;
                },
                WsClientEvent::Detach => {
                    let mut daemon = state.daemon.lock().await;
                    daemon
                        .detach(DetachRequest {
                            session_id: session_id.into(),
                        })
                        .map_err(|error| WsClientError::Daemon(error.to_string()))?;
                    return Ok(false);
                },
            }
        },
        Message::Binary(bytes) => {
            let mut daemon = state.daemon.lock().await;
            daemon
                .write(WriteRequest {
                    session_id: session_id.into(),
                    bytes: bytes.to_vec(),
                })
                .map_err(|error| WsClientError::Daemon(error.to_string()))?;
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

// ── Agent routes ─────────────────────────────────────────────────────

pub(crate) async fn agent_notify(
    State(state): State<AppState>,
    Json(request): Json<AgentNotifyRequest>,
) -> StatusCode {
    tracing::info!(
        hook_event = request.hook_event_name.as_str(),
        session_id = request.session_id.as_str(),
        cwd = request.cwd.as_str(),
        "agent notify received"
    );
    let agent_state = match request.hook_event_name.as_str() {
        "UserPromptSubmit" => AgentState::Working,
        "Stop" => AgentState::Waiting,
        _ => {
            tracing::info!(
                hook_event = request.hook_event_name.as_str(),
                "agent notify ignored: unknown hook event"
            );
            return StatusCode::OK;
        },
    };

    upsert_agent_session(
        &state,
        request.session_id.clone(),
        request.cwd.clone(),
        agent_state,
        AgentSessionUpdateSource::Hook,
    )
    .await;

    StatusCode::OK
}

pub(crate) async fn apply_terminal_activity_event(state: &AppState, event: TerminalActivityEvent) {
    match event {
        TerminalActivityEvent::Update {
            session_id,
            cwd,
            state: agent_state,
        } => {
            upsert_agent_session(
                state,
                terminal_agent_session_key(&session_id),
                cwd.display().to_string(),
                agent_state,
                AgentSessionUpdateSource::TerminalActivity,
            )
            .await;
        },
        TerminalActivityEvent::Clear { session_id } => {
            remove_agent_session(state, &terminal_agent_session_key(&session_id)).await;
        },
    }
}

async fn upsert_agent_session(
    state: &AppState,
    session_id: String,
    cwd: String,
    agent_state: AgentState,
    source: AgentSessionUpdateSource,
) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let cwd_path = PathBuf::from(&cwd);

    let (dto, previous_state) = {
        let mut sessions = state.agent_sessions.lock().await;

        let cutoff = now_ms.saturating_sub(AGENT_SESSION_EXPIRY_SECS * 1000);
        sessions.retain(|_, session| session.updated_at_unix_ms > cutoff);

        let previous_state = sessions.get(&session_id).map(|session| session.state);

        sessions.insert(session_id.clone(), AgentSession {
            cwd: cwd.clone(),
            state: agent_state,
            updated_at_unix_ms: now_ms,
        });

        (
            AgentSessionDto {
                session_id: session_id.clone(),
                cwd: cwd.clone(),
                state: agent_state_label(agent_state).to_owned(),
                updated_at_unix_ms: now_ms,
            },
            previous_state,
        )
    };

    tracing::info!(
        session_id = dto.session_id.as_str(),
        cwd = dto.cwd.as_str(),
        state = dto.state.as_str(),
        "agent session updated, broadcasting"
    );
    if let Some(event_name) =
        notification_event_name_for_agent_transition(source, previous_state, agent_state)
        && let Ok(repo_root) = worktree::repo_root(&cwd_path)
    {
        let branch = git_branch_name_for_worktree(&cwd_path).ok();
        spawn_notification_webhooks(
            repo_root.clone(),
            event_name,
            serde_json::json!({
                "event": event_name,
                "repo_root": repo_root,
                "worktree_path": cwd_path,
                "cwd": dto.cwd.clone(),
                "branch": branch,
                "session_id": session_id,
                "state": agent_state_label(agent_state),
                "previous_state": previous_state.map(agent_state_label),
                "timestamp_unix_ms": now_ms,
            }),
        );
    }
    let _ = state
        .agent_broadcast
        .send(AgentWsEvent::Update { session: dto });
}

async fn remove_agent_session(state: &AppState, session_id: &str) {
    let removed = {
        let mut sessions = state.agent_sessions.lock().await;
        sessions.remove(session_id).is_some()
    };
    if !removed {
        return;
    }

    tracing::info!(session_id, "agent session cleared, broadcasting clear");
    let _ = state.agent_broadcast.send(AgentWsEvent::Clear {
        session_id: session_id.to_owned(),
    });
}

fn terminal_agent_session_key(session_id: &SessionId) -> String {
    format!("terminal:{session_id}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentSessionUpdateSource {
    Hook,
    TerminalActivity,
}

pub(crate) async fn list_agent_activity(
    State(state): State<AppState>,
) -> ApiResult<Vec<AgentSessionDto>> {
    let mut sessions = state.agent_sessions.lock().await;
    Ok(Json(agent_session_snapshot(&mut sessions)))
}

pub(crate) async fn agent_activity_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_agent_activity_ws(state, socket))
        .into_response()
}

async fn handle_agent_activity_ws(state: AppState, mut socket: WebSocket) {
    let (snapshot, mut rx) = agent_activity_snapshot_and_subscription(&state).await;

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
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let (snapshot, next_rx) = agent_activity_snapshot_and_subscription(&state).await;
                if send_ws_json(&mut socket, &snapshot).await.is_err() {
                    break;
                }
                rx = next_rx;
            },
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn agent_activity_snapshot_and_subscription(
    state: &AppState,
) -> (AgentWsEvent, broadcast::Receiver<AgentWsEvent>) {
    let snapshot = {
        let mut sessions = state.agent_sessions.lock().await;
        let dtos = agent_session_snapshot(&mut sessions);
        AgentWsEvent::Snapshot { sessions: dtos }
    };

    (snapshot, state.agent_broadcast.subscribe())
}

pub(crate) async fn send_ws_json(socket: &mut WebSocket, value: &impl Serialize) -> Result<(), ()> {
    let payload = serde_json::to_string(value).map_err(|_| ())?;
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}

fn repo_webhook_urls_for_event(repo_root: &Path, event_name: &str) -> Vec<String> {
    let Some(config) = repo_config::load_repo_config(repo_root) else {
        return Vec::new();
    };

    let notifications = config.notifications;
    if !notifications.events.is_empty()
        && !notifications.events.iter().any(|event| event == event_name)
    {
        return Vec::new();
    }

    notifications
        .webhook_urls
        .into_iter()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .collect()
}

pub(crate) fn spawn_notification_webhooks(
    repo_root: PathBuf,
    event_name: &'static str,
    payload: serde_json::Value,
) {
    let urls = repo_webhook_urls_for_event(&repo_root, event_name);
    if urls.is_empty() {
        return;
    }

    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            let http: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(10)))
                .build()
                .into();
            for url in urls {
                let body = match notification_webhook_request_body(&url, event_name, &payload) {
                    Ok(body) => body,
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            %url,
                            event = event_name,
                            "failed to serialize notification webhook request"
                        );
                        continue;
                    },
                };

                send_notification_webhook_with_retries(&http, &url, event_name, &body);
            }
        })
        .await;
    });
}

const NOTIFICATION_WEBHOOK_MAX_ATTEMPTS: usize = 3;
const NOTIFICATION_WEBHOOK_RETRY_DELAYS_MS: [u64; NOTIFICATION_WEBHOOK_MAX_ATTEMPTS - 1] =
    [300, 1_000];

fn send_notification_webhook_with_retries(
    http: &ureq::Agent,
    url: &str,
    event_name: &str,
    body: &str,
) {
    for attempt in 0..NOTIFICATION_WEBHOOK_MAX_ATTEMPTS {
        match send_notification_webhook_request(http, url, body) {
            Ok(()) => return,
            Err(error) => {
                let Some(delay) = notification_webhook_retry_delay(attempt, &error) else {
                    tracing::warn!(
                        %error,
                        %url,
                        event = event_name,
                        attempt = attempt + 1,
                        "notification webhook failed"
                    );
                    return;
                };

                tracing::warn!(
                    %error,
                    %url,
                    event = event_name,
                    attempt = attempt + 1,
                    retry_in_ms = delay.as_millis() as u64,
                    "notification webhook failed; retrying"
                );
                std::thread::sleep(delay);
            },
        }
    }
}

fn send_notification_webhook_request(
    http: &ureq::Agent,
    url: &str,
    body: &str,
) -> Result<(), ureq::Error> {
    http.post(url)
        .header("content-type", "application/json")
        .send(body)
        .map(|_| ())
}

fn notification_webhook_retry_delay(attempt: usize, error: &ureq::Error) -> Option<Duration> {
    if !should_retry_notification_webhook(error) {
        return None;
    }

    NOTIFICATION_WEBHOOK_RETRY_DELAYS_MS
        .get(attempt)
        .copied()
        .map(Duration::from_millis)
}

fn should_retry_notification_webhook(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::StatusCode(status) => {
            *status == 408 || *status == 409 || *status == 425 || *status == 429 || *status >= 500
        },
        ureq::Error::Timeout(_)
        | ureq::Error::Io(_)
        | ureq::Error::HostNotFound
        | ureq::Error::Protocol(_)
        | ureq::Error::ConnectionFailed => true,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotificationWebhookFormat {
    GenericJson,
    SlackIncomingWebhook,
    DiscordWebhook,
}

fn notification_webhook_format(url: &str) -> NotificationWebhookFormat {
    let url = url.to_ascii_lowercase();
    if url.contains("hooks.slack.com/") {
        NotificationWebhookFormat::SlackIncomingWebhook
    } else if url.contains("discord.com/api/webhooks/")
        || url.contains("discordapp.com/api/webhooks/")
    {
        NotificationWebhookFormat::DiscordWebhook
    } else {
        NotificationWebhookFormat::GenericJson
    }
}

fn notification_webhook_request_body(
    url: &str,
    event_name: &str,
    payload: &serde_json::Value,
) -> Result<String, serde_json::Error> {
    match notification_webhook_format(url) {
        NotificationWebhookFormat::GenericJson => serde_json::to_string(payload),
        NotificationWebhookFormat::SlackIncomingWebhook => serde_json::to_string(
            &serde_json::json!({ "text": notification_webhook_text(event_name, payload) }),
        ),
        NotificationWebhookFormat::DiscordWebhook => serde_json::to_string(
            &serde_json::json!({ "content": notification_webhook_text(event_name, payload) }),
        ),
    }
}

fn notification_webhook_text(event_name: &str, payload: &serde_json::Value) -> String {
    let repo_root = notification_payload_field(payload, "repo_root");
    let worktree_path = notification_payload_field(payload, "worktree_path");
    let branch = notification_payload_field(payload, "branch");
    let cwd = notification_payload_field(payload, "cwd");
    let process_name = notification_payload_field(payload, "process_name");
    let command = notification_payload_field(payload, "command");
    let exit_code = payload.get("exit_code").and_then(serde_json::Value::as_i64);

    match event_name {
        "agent_started" => {
            let mut parts = vec!["Arbor agent started".to_owned()];
            if let Some(branch) = branch {
                parts.push(format!("branch `{branch}`"));
            }
            if let Some(worktree_path) = worktree_path.or(cwd) {
                parts.push(format!("worktree `{worktree_path}`"));
            }
            if let Some(repo_root) = repo_root {
                parts.push(format!("repo `{repo_root}`"));
            }
            parts.join(" · ")
        },
        "agent_finished" => {
            let mut parts = vec!["Arbor agent finished".to_owned()];
            if let Some(branch) = branch {
                parts.push(format!("branch `{branch}`"));
            }
            if let Some(worktree_path) = worktree_path.or(cwd) {
                parts.push(format!("worktree `{worktree_path}`"));
            }
            if let Some(repo_root) = repo_root {
                parts.push(format!("repo `{repo_root}`"));
            }
            parts.join(" · ")
        },
        "agent_error" => {
            let mut parts = vec!["Arbor process error".to_owned()];
            if let Some(process_name) = process_name {
                parts.push(format!("process `{process_name}`"));
            }
            if let Some(command) = command {
                parts.push(format!("command `{command}`"));
            }
            if let Some(exit_code) = exit_code {
                parts.push(format!("exit {exit_code}"));
            }
            if let Some(repo_root) = repo_root {
                parts.push(format!("repo `{repo_root}`"));
            }
            parts.join(" · ")
        },
        _ => {
            let mut parts = vec![format!("Arbor event `{event_name}`")];
            if let Some(repo_root) = repo_root {
                parts.push(format!("repo `{repo_root}`"));
            }
            parts.join(" · ")
        },
    }
}

fn notification_payload_field<'a>(payload: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(serde_json::Value::as_str)
}

fn notification_event_name_for_agent_transition(
    source: AgentSessionUpdateSource,
    previous_state: Option<AgentState>,
    current_state: AgentState,
) -> Option<&'static str> {
    if source == AgentSessionUpdateSource::TerminalActivity {
        return None;
    }

    match (previous_state, current_state) {
        (Some(AgentState::Working), AgentState::Working)
        | (Some(AgentState::Waiting), AgentState::Waiting) => None,
        (_, AgentState::Working) => Some("agent_started"),
        (_, AgentState::Waiting) => Some("agent_finished"),
    }
}

fn agent_state_label(state: AgentState) -> &'static str {
    match state {
        AgentState::Working => "working",
        AgentState::Waiting => "waiting",
    }
}

fn agent_session_snapshot(sessions: &mut HashMap<String, AgentSession>) -> Vec<AgentSessionDto> {
    let cutoff = current_unix_timestamp_millis().saturating_sub(AGENT_SESSION_EXPIRY_SECS * 1000);
    sessions.retain(|_, session| session.updated_at_unix_ms > cutoff);

    let mut snapshot: Vec<AgentSessionDto> = sessions
        .iter()
        .map(|(session_id, session)| AgentSessionDto {
            session_id: session_id.clone(),
            cwd: session.cwd.clone(),
            state: match session.state {
                AgentState::Working => "working".to_owned(),
                AgentState::Waiting => "waiting".to_owned(),
            },
            updated_at_unix_ms: session.updated_at_unix_ms,
        })
        .collect();
    snapshot.sort_by(|left, right| {
        left.cwd
            .cmp(&right.cwd)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    snapshot
}

// ── Process management handlers ──────────────────────────────────────

pub(crate) async fn list_processes(State(state): State<AppState>) -> ApiResult<Vec<ProcessInfo>> {
    let definitions =
        discover_process_definitions(&state).map_err(|e| internal_error(e.to_string()))?;
    let session_memory_bytes = collect_process_memory_bytes(&state).await;
    let mut pm = state.process_manager.lock().await;
    pm.sync_definitions(&definitions);
    let mut infos = pm.list_processes(&definitions);
    apply_process_memory(&mut infos, &session_memory_bytes);
    Ok(Json(infos))
}

pub(crate) async fn start_all_processes(
    State(state): State<AppState>,
) -> ApiResult<Vec<ProcessInfo>> {
    let definitions =
        discover_process_definitions(&state).map_err(|e| internal_error(e.to_string()))?;
    let mut pm = state.process_manager.lock().await;
    pm.sync_definitions(&definitions);
    let mut daemon = state.daemon.lock().await;
    let results = pm.start_all(&definitions, &mut *daemon);
    let mut infos: Vec<ProcessInfo> = results
        .into_iter()
        .filter_map(|(_, result)| result.ok())
        .collect();
    let session_memory_bytes = process_memory_bytes(&*daemon);
    apply_process_memory(&mut infos, &session_memory_bytes);
    Ok(Json(infos))
}

pub(crate) async fn stop_all_processes(
    State(state): State<AppState>,
) -> ApiResult<Vec<ProcessInfo>> {
    let definitions =
        discover_process_definitions(&state).map_err(|e| internal_error(e.to_string()))?;
    let mut pm = state.process_manager.lock().await;
    pm.sync_definitions(&definitions);
    let mut daemon = state.daemon.lock().await;
    let results = pm.stop_all(&mut *daemon);
    let mut infos: Vec<ProcessInfo> = results
        .into_iter()
        .filter_map(|(_, result)| result.ok())
        .collect();
    let session_memory_bytes = process_memory_bytes(&*daemon);
    apply_process_memory(&mut infos, &session_memory_bytes);
    Ok(Json(infos))
}

pub(crate) async fn start_process(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<ProcessInfo> {
    let definitions =
        discover_process_definitions(&state).map_err(|e| internal_error(e.to_string()))?;
    let mut pm = state.process_manager.lock().await;
    pm.sync_definitions(&definitions);
    let mut daemon = state.daemon.lock().await;
    let mut info = pm
        .start_process(&name, &definitions, &mut *daemon)
        .map_err(|e| internal_error(e.to_string()))?;
    let session_memory_bytes = process_memory_bytes(&*daemon);
    apply_process_memory(std::slice::from_mut(&mut info), &session_memory_bytes);
    Ok(Json(info))
}

pub(crate) async fn stop_process(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<ProcessInfo> {
    let definitions =
        discover_process_definitions(&state).map_err(|e| internal_error(e.to_string()))?;
    let mut pm = state.process_manager.lock().await;
    pm.sync_definitions(&definitions);
    let mut daemon = state.daemon.lock().await;
    let mut info = pm
        .stop_process(&name, &definitions, &mut *daemon)
        .map_err(|e| internal_error(e.to_string()))?;
    let session_memory_bytes = process_memory_bytes(&*daemon);
    apply_process_memory(std::slice::from_mut(&mut info), &session_memory_bytes);
    Ok(Json(info))
}

pub(crate) async fn restart_process(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<ProcessInfo> {
    let definitions =
        discover_process_definitions(&state).map_err(|e| internal_error(e.to_string()))?;
    let mut pm = state.process_manager.lock().await;
    pm.sync_definitions(&definitions);
    let mut daemon = state.daemon.lock().await;
    let mut info = pm
        .restart_process(&name, &definitions, &mut *daemon)
        .map_err(|e| internal_error(e.to_string()))?;
    let session_memory_bytes = process_memory_bytes(&*daemon);
    apply_process_memory(std::slice::from_mut(&mut info), &session_memory_bytes);
    Ok(Json(info))
}

pub(crate) async fn process_status_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_process_status_ws(state, socket))
        .into_response()
}

async fn handle_process_status_ws(state: AppState, mut socket: WebSocket) {
    let (snapshot, mut rx) = process_status_snapshot_and_subscription(&state).await;

    if send_ws_json(&mut socket, &snapshot).await.is_err() {
        return;
    }

    loop {
        match rx.recv().await {
            Ok(mut event) => {
                enrich_process_event_memory(&state, &mut event).await;
                if send_ws_json(&mut socket, &event).await.is_err() {
                    break;
                }
            },
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let (snapshot, next_rx) = process_status_snapshot_and_subscription(&state).await;
                if send_ws_json(&mut socket, &snapshot).await.is_err() {
                    break;
                }
                rx = next_rx;
            },
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn process_status_snapshot_and_subscription(
    state: &AppState,
) -> (ProcessEvent, broadcast::Receiver<ProcessEvent>) {
    let definitions = discover_process_definitions(state).unwrap_or_else(|error| {
        tracing::warn!(%error, "failed to discover process definitions for process WebSocket");
        Vec::new()
    });
    let session_memory_bytes = collect_process_memory_bytes(state).await;
    let mut pm = state.process_manager.lock().await;
    pm.sync_definitions(&definitions);
    let mut snapshot = pm.snapshot_event(&definitions);
    apply_process_memory_to_event(&mut snapshot, &session_memory_bytes);
    (snapshot, pm.subscribe())
}

fn discover_process_definitions(
    state: &AppState,
) -> Result<Vec<crate::process_manager::ProcessDefinition>, RepositoryStoreError> {
    let roots = state.repository_store.load_roots()?;
    let resolved = repository_store::resolve_repository_roots(roots);
    Ok(crate::process_manager::discover_process_definitions_for_roots(&resolved))
}

async fn collect_process_memory_bytes(state: &AppState) -> HashMap<String, u64> {
    let daemon = state.daemon.lock().await;
    process_memory_bytes(&*daemon)
}

fn process_memory_bytes<D>(daemon: &D) -> HashMap<String, u64>
where
    D: TerminalDaemon,
    D::Error: ToString,
{
    match crate::process_metrics::collect_session_memory_bytes(daemon) {
        Ok(session_memory_bytes) => session_memory_bytes,
        Err(error) => {
            tracing::warn!(%error, "failed to collect process memory");
            HashMap::new()
        },
    }
}

fn apply_process_memory(
    processes: &mut [ProcessInfo],
    session_memory_bytes: &HashMap<String, u64>,
) {
    crate::process_metrics::attach_process_memory(processes, session_memory_bytes);
}

async fn enrich_process_event_memory(state: &AppState, event: &mut ProcessEvent) {
    let session_memory_bytes = collect_process_memory_bytes(state).await;
    apply_process_memory_to_event(event, &session_memory_bytes);
}

fn apply_process_memory_to_event(
    event: &mut ProcessEvent,
    session_memory_bytes: &HashMap<String, u64>,
) {
    match event {
        ProcessEvent::Snapshot { processes } => {
            apply_process_memory(processes, session_memory_bytes);
        },
        ProcessEvent::Update { process } => {
            apply_process_memory(std::slice::from_mut(process), session_memory_bytes);
        },
    }
}

// ── Log streaming ────────────────────────────────────────────────────

pub(crate) async fn logs_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_logs_ws(state, socket))
        .into_response()
}

async fn handle_logs_ws(state: AppState, mut socket: WebSocket) {
    let mut rx = state.log_broadcast.subscribe();
    loop {
        match rx.recv().await {
            Ok(line) => {
                if socket.send(Message::Text(line.into())).await.is_err() {
                    break;
                }
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                // Tell the client entries were skipped, then continue.
                let msg = serde_json::json!({
                    "ts": current_unix_timestamp_millis(),
                    "level": "WARN",
                    "target": "arbor_httpd",
                    "message": format!("log stream lagged, skipped {n} entries"),
                    "fields": "",
                });
                if socket
                    .send(Message::Text(msg.to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                rx = state.log_broadcast.subscribe();
            },
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

// ── Web UI fallback ──────────────────────────────────────────────────

/// Dynamic SPA fallback: serves `index.html` if it exists (for client-side
/// routing), otherwise returns a helpful "not built" message. Checked
/// per-request so a long-running process picks up assets installed later.
pub(crate) async fn web_ui_spa_or_unavailable() -> Response {
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

pub(crate) fn ensure_web_ui_assets() -> Result<(), RouteError> {
    if arbor_web_ui::dist_is_built() {
        return Ok(());
    }

    // Only attempt an automatic npm build during development, when the source
    // directory exists. In release/packaged builds the assets should already
    // be bundled alongside the binary.
    let app_dir = arbor_web_ui::app_dir();
    if !app_dir.join("package.json").is_file() {
        return Err(RouteError::Internal(
            "web-ui assets not found (packaged build without bundled assets?)".to_owned(),
        ));
    }

    let package_manager = detect_npm_binary().ok_or_else(|| {
        RouteError::Internal(
            "`npm` is not installed or not in PATH; skipping web-ui build".to_owned(),
        )
    })?;

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

    Err(RouteError::Internal(format!(
        "web-ui build completed but `{}` is missing",
        arbor_web_ui::dist_index_path().display()
    )))
}

// ── Helper functions ─────────────────────────────────────────────────

// ── Scheduled tasks ──────────────────────────────────────────────────

pub(crate) async fn list_tasks(State(state): State<AppState>) -> ApiResult<Vec<TaskInfo>> {
    let ts = state.task_scheduler.lock().await;
    Ok(Json(ts.list_tasks()))
}

pub(crate) async fn run_task(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<TaskInfo> {
    let task_request = {
        let mut ts = state.task_scheduler.lock().await;
        ts.mark_running(&name)
            .map_err(|e| internal_error(e.to_string()))?
    };

    let scheduler = state.task_scheduler.clone();

    tokio::spawn(async move {
        let started_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let (exit_code, stdout) =
            task_scheduler::execute_task(&task_request.command, &task_request.working_dir).await;

        let mut agent_spawned = false;
        if let Some(ref trigger) = task_request.trigger
            && task_scheduler::should_trigger(trigger, exit_code, &stdout)
            && let Some((program, args)) =
                task_scheduler::build_agent_command(trigger, &stdout, &task_request.repo_root)
            && task_scheduler::spawn_agent(&program, &args, &task_request.working_dir)
                .await
                .is_ok()
        {
            agent_spawned = true;
        }

        let stdout_tail = if stdout.is_empty() {
            None
        } else {
            Some(stdout)
        };

        let mut ts = scheduler.lock().await;
        ts.record_completion(
            &task_request.name,
            exit_code,
            stdout_tail,
            started_at_ms,
            agent_spawned,
        );
    });

    let ts = state.task_scheduler.lock().await;
    let info = ts
        .list_tasks()
        .into_iter()
        .find(|t| t.name == name)
        .ok_or_else(|| internal_error(format!("task `{name}` not found")))?;
    Ok(Json(info))
}

pub(crate) async fn task_history(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> ApiResult<Vec<TaskExecution>> {
    let ts = state.task_scheduler.lock().await;
    let history = ts
        .task_history(&name)
        .map_err(|e| not_found_error(e.to_string()))?;
    Ok(Json(history))
}

pub(crate) async fn task_status_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_task_status_ws(state, socket))
        .into_response()
}

async fn handle_task_status_ws(state: AppState, mut socket: WebSocket) {
    let (snapshot, mut rx) = {
        let ts = state.task_scheduler.lock().await;
        (ts.snapshot_event(), ts.subscribe())
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
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let (snapshot, next_rx) = {
                    let ts = state.task_scheduler.lock().await;
                    (ts.snapshot_event(), ts.subscribe())
                };
                if send_ws_json(&mut socket, &snapshot).await.is_err() {
                    break;
                }
                rx = next_rx;
            },
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

pub(crate) fn internal_error(message: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            error: message.into(),
        }),
    )
}

pub(crate) fn bad_request_error(message: String) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: message }))
}

pub(crate) fn not_found_error(message: String) -> (StatusCode, Json<ApiError>) {
    (StatusCode::NOT_FOUND, Json(ApiError { error: message }))
}

#[cfg(feature = "agent-chat")]
fn bad_gateway_error(message: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(ApiError {
            error: message.to_owned(),
        }),
    )
}

pub(crate) fn map_daemon_error(error: LocalTerminalDaemonError) -> (StatusCode, Json<ApiError>) {
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

fn run_command(program: &str, args: &[&str], cwd: &Path) -> Result<(), RouteError> {
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|error| {
            RouteError::Internal(format!(
                "failed to run `{program} {}` in `{}`: {error}",
                args.join(" "),
                cwd.display()
            ))
        })?;

    if status.success() {
        return Ok(());
    }

    Err(RouteError::Internal(format!(
        "command `{program} {}` failed with status {status}",
        args.join(" "),
    )))
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
) -> Result<String, GitCommitError> {
    if changed_files.is_empty() {
        return Err(GitCommitError::NothingToCommit);
    }

    let repo =
        git2::Repository::open(worktree_path).map_err(|source| GitCommitError::OpenRepository {
            path: worktree_path.display().to_string(),
            source,
        })?;

    let mut index = repo.index().map_err(GitCommitError::ReadIndex)?;
    index
        .add_all(["."], git2::IndexAddOption::DEFAULT, None)
        .map_err(GitCommitError::StageChanges)?;
    index
        .update_all(["."], None)
        .map_err(GitCommitError::UpdateIndex)?;
    index.write().map_err(GitCommitError::WriteIndex)?;

    let tree_oid = index.write_tree().map_err(GitCommitError::WriteTree)?;
    let tree = repo.find_tree(tree_oid).map_err(GitCommitError::FindTree)?;

    if let Ok(head) = repo.head()
        && let Ok(head_commit) = head.peel_to_commit()
        && head_commit.tree_id() == tree_oid
    {
        return Err(GitCommitError::NothingToCommit);
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

    let signature = repo.signature().map_err(GitCommitError::CreateSignature)?;
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
    .map_err(GitCommitError::CreateCommit)?;

    Ok(message)
}

fn run_git_push_for_worktree(worktree_path: &Path) -> Result<String, RouteError> {
    let repo = git2::Repository::open(worktree_path).map_err(|error| {
        RouteError::Internal(format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        ))
    })?;

    let head_ref = repo
        .head()
        .map_err(|error| RouteError::Internal(format!("failed to read HEAD: {error}")))?;
    let branch_name = head_ref
        .shorthand()
        .ok_or_else(|| RouteError::Internal("cannot push detached HEAD".to_owned()))?
        .to_owned();
    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");

    let mut remote = repo.find_remote("origin").map_err(|error| {
        RouteError::Internal(format!("failed to find remote `origin`: {error}"))
    })?;

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
        .map_err(|error| RouteError::Internal(format!("push failed: {error}")))?;

    let mut config = repo
        .config()
        .map_err(|error| RouteError::Internal(format!("failed to read config: {error}")))?;
    let _ = config.set_str(&format!("branch.{branch_name}.remote"), "origin");
    let _ = config.set_str(
        &format!("branch.{branch_name}.merge"),
        &format!("refs/heads/{branch_name}"),
    );

    Ok(format!(
        "push complete: {branch_name} -> origin/{branch_name}"
    ))
}

fn git_branch_name_for_worktree(worktree_path: &Path) -> Result<String, RouteError> {
    let repo = git2::Repository::open(worktree_path).map_err(|error| {
        RouteError::Internal(format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        ))
    })?;

    let head_ref = repo
        .head()
        .map_err(|error| RouteError::Internal(format!("failed to read HEAD: {error}")))?;

    head_ref
        .shorthand()
        .map(str::to_owned)
        .ok_or_else(|| RouteError::Internal("worktree has detached HEAD".to_owned()))
}

fn short_branch(value: &str) -> String {
    worktree::short_branch(value)
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    worktree::paths_equivalent(left, right)
}

pub(crate) fn current_unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn github_repo_slug_for_path(repo_root: &Path) -> Option<String> {
    let repo = gix::open(repo_root).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    let url = remote.url(gix::remote::Direction::Fetch)?;
    let url_str = url.to_bstring().to_string();
    if url_str.is_empty() {
        return None;
    }
    github_repo_slug_from_remote_url(url_str.trim())
}

fn github_repo_slug_cached(
    cache: &mut HashMap<String, RepoCacheEntry>,
    repo_root: &Path,
) -> (Option<String>, Option<String>) {
    // Evict expired entries to prevent unbounded growth.
    cache.retain(|_, entry| entry.fetched_at.elapsed().as_secs() < REPO_CACHE_TTL_SECS);

    let key = repo_root.display().to_string();
    if let Some(entry) = cache.get(&key) {
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

    let cache_key = pr_cache_key(slug, branch, is_primary);

    // Check cache and evict expired entries
    {
        let mut cache_map = cache.lock().await;
        cache_map.retain(|_, entry| entry.fetched_at.elapsed().as_secs() < PR_CACHE_TTL_SECS);
        if let Some(entry) = cache_map.get(&cache_key) {
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

fn pr_cache_key(slug: &str, branch: &str, is_primary: bool) -> String {
    format!(
        "{slug}:{branch}:{}",
        if is_primary {
            "primary"
        } else {
            "linked"
        }
    )
}

// ── Agent Chat routes ────────────────────────────────────────────────

#[cfg(feature = "agent-chat")]
async fn create_agent_chat(
    State(state): State<AppState>,
    Json(request): Json<crate::agent_chat::CreateAgentChatRequest>,
) -> ApiResult<crate::agent_chat::CreateAgentChatResponse> {
    let workspace_path = PathBuf::from(&request.workspace_path);
    let mut manager = state.agent_chat.lock().await;
    let (session_id, event_rx) = manager.create_session(
        request.agent_kind,
        workspace_path,
        request.initial_prompt,
        request.model_id,
        request.transport,
    );
    manager.persist();

    // Spawn a background listener to track conversation state
    crate::agent_chat::spawn_session_listener(
        state.agent_chat.clone(),
        session_id.clone(),
        event_rx,
    );

    Ok(Json(crate::agent_chat::CreateAgentChatResponse {
        session_id,
    }))
}

#[cfg(feature = "agent-chat")]
async fn list_agent_chats(
    State(state): State<AppState>,
) -> ApiResult<Vec<crate::agent_chat::AgentChatSessionDto>> {
    let manager = state.agent_chat.lock().await;
    Ok(Json(manager.list()))
}

#[cfg(feature = "agent-chat")]
async fn send_agent_message(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<crate::agent_chat::SendAgentMessageRequest>,
) -> ApiResult<serde_json::Value> {
    let mut manager = state.agent_chat.lock().await;
    manager
        .send_message(&session_id, request.message)
        .map_err(bad_request_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(feature = "agent-chat")]
async fn cancel_agent_chat(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> ApiResult<serde_json::Value> {
    let mut manager = state.agent_chat.lock().await;
    manager.cancel(&session_id).map_err(not_found_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(feature = "agent-chat")]
async fn kill_agent_chat(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> ApiResult<serde_json::Value> {
    let mut manager = state.agent_chat.lock().await;
    manager.kill(&session_id).map_err(not_found_error)?;
    manager.remove(&session_id);
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(feature = "agent-chat")]
async fn agent_chat_history(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> ApiResult<Vec<crate::agent_chat::ChatMessage>> {
    let manager = state.agent_chat.lock().await;
    let messages = manager.history(&session_id).map_err(not_found_error)?;
    Ok(Json(messages))
}

#[cfg(feature = "agent-chat")]
async fn agent_chat_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> Response {
    ws.on_upgrade(move |socket| handle_agent_chat_ws(socket, state, session_id))
}

#[cfg(feature = "agent-chat")]
async fn handle_agent_chat_ws(mut socket: WebSocket, state: AppState, session_id: String) {
    // Subscribe to session events
    let (mut event_rx, session_dto, messages) = {
        let manager = state.agent_chat.lock().await;
        match manager.subscribe(&session_id) {
            Ok(result) => result,
            Err(error) => {
                let _ = socket
                    .send(Message::Text(
                        serde_json::to_string(&crate::agent_chat::AgentChatEvent::Error {
                            message: error,
                        })
                        .unwrap_or_default()
                        .into(),
                    ))
                    .await;
                return;
            },
        }
    };

    // Send initial snapshot with conversation history
    let snapshot = crate::agent_chat::AgentChatEvent::Snapshot {
        messages,
        status: session_dto.status,
        input_tokens: session_dto.input_tokens,
        output_tokens: session_dto.output_tokens,
        transport_label: Some(session_dto.transport_label.clone()),
    };
    if let Ok(json) = serde_json::to_string(&snapshot) {
        let _ = socket.send(Message::Text(json.into())).await;
    }

    // Stream events to client, handle client messages
    loop {
        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Ok(chat_event) => {
                        if let Ok(json) = serde_json::to_string(&chat_event)
                            && socket.send(Message::Text(json.into())).await.is_err()
                        {
                            break;
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            },
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Client can send messages over WebSocket too
                        if let Ok(request) = serde_json::from_str::<crate::agent_chat::SendAgentMessageRequest>(&text) {
                            let mut manager = state.agent_chat.lock().await;
                            let _ = manager.send_message(&session_id, request.message);
                        }
                    },
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {},
                }
            },
        }
    }
}

#[cfg(feature = "agent-chat")]
async fn discover_models(
    Json(request): Json<crate::agent_chat::DiscoverModelsRequest>,
) -> ApiResult<crate::agent_chat::DiscoverModelsResponse> {
    let models =
        crate::agent_chat::discover_openai_models(&request.base_url, request.api_key.as_deref())
            .await
            .map_err(|e| bad_gateway_error(&e))?;
    Ok(Json(crate::agent_chat::DiscoverModelsResponse { models }))
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        std::sync::atomic::{AtomicUsize, Ordering},
    };

    struct MockGitHubPrService {
        lookup_count: AtomicUsize,
    }

    impl MockGitHubPrService {
        fn new() -> Self {
            Self {
                lookup_count: AtomicUsize::new(0),
            }
        }
    }

    impl GitHubPrService for MockGitHubPrService {
        fn lookup_pr_for_branch(
            &self,
            _repo_slug: Option<String>,
            branch: String,
            is_primary: bool,
        ) -> futures_util::future::BoxFuture<'static, (Option<u64>, Option<String>)> {
            self.lookup_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if is_primary {
                    (None, None)
                } else {
                    let url = format!("https://github.com/penso/arbor/pull/{}", 55);
                    let _ = branch;
                    (Some(55), Some(url))
                }
            })
        }
    }

    #[test]
    fn issue_worktree_data_from_entries_keeps_primary_checkout() {
        let repo_root = PathBuf::from("/tmp/arbor");
        let entries = vec![
            worktree::Worktree {
                path: repo_root.clone(),
                branch: Some("refs/heads/issue-55".to_owned()),
                ..Default::default()
            },
            worktree::Worktree {
                path: repo_root.join("../issue-55-linked"),
                branch: Some("refs/heads/issue-55-linked".to_owned()),
                ..Default::default()
            },
        ];

        let worktrees = issue_worktree_data_from_entries(entries);

        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path, repo_root);
        assert_eq!(worktrees[0].branch, "issue-55");
    }

    #[tokio::test]
    async fn lookup_pr_cached_distinguishes_primary_and_linked_results() {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let github_service = Arc::new(MockGitHubPrService::new());

        let primary = lookup_pr_cached(
            cache.clone(),
            github_service.clone(),
            Some("penso/arbor"),
            "issue-55",
            true,
        )
        .await;
        let linked = lookup_pr_cached(
            cache,
            github_service.clone(),
            Some("penso/arbor"),
            "issue-55",
            false,
        )
        .await;

        assert_eq!(primary, (None, None));
        assert_eq!(
            linked,
            (
                Some(55),
                Some("https://github.com/penso/arbor/pull/55".to_owned()),
            )
        );
        assert_eq!(github_service.lookup_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn notification_webhook_format_detects_provider_specific_urls() {
        assert_eq!(
            notification_webhook_format("https://hooks.slack.com/services/T000/B000/abc"),
            NotificationWebhookFormat::SlackIncomingWebhook
        );
        assert_eq!(
            notification_webhook_format("https://discord.com/api/webhooks/123/abc"),
            NotificationWebhookFormat::DiscordWebhook
        );
        assert_eq!(
            notification_webhook_format("https://example.com/hooks/arbor"),
            NotificationWebhookFormat::GenericJson
        );
    }

    #[test]
    fn notification_webhook_request_body_uses_slack_text_payload() {
        let payload = serde_json::json!({
            "event": "agent_finished",
            "repo_root": "/tmp/repo",
            "worktree_path": "/tmp/repo-feature",
            "branch": "feature/test"
        });
        let body = notification_webhook_request_body(
            "https://hooks.slack.com/services/T000/B000/abc",
            "agent_finished",
            &payload,
        )
        .unwrap_or_else(|error| panic!("request body should serialize: {error}"));
        let json: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|error| panic!("request body should parse: {error}"));

        assert_eq!(
            json.get("text").and_then(serde_json::Value::as_str),
            Some(
                "Arbor agent finished · branch `feature/test` · worktree `/tmp/repo-feature` · repo `/tmp/repo`"
            )
        );
    }

    #[test]
    fn notification_webhook_request_body_uses_discord_content_payload() {
        let payload = serde_json::json!({
            "event": "agent_error",
            "repo_root": "/tmp/repo",
            "process_name": "web",
            "command": "npm test",
            "exit_code": 1
        });
        let body = notification_webhook_request_body(
            "https://discord.com/api/webhooks/123/abc",
            "agent_error",
            &payload,
        )
        .unwrap_or_else(|error| panic!("request body should serialize: {error}"));
        let json: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|error| panic!("request body should parse: {error}"));

        assert_eq!(
            json.get("content").and_then(serde_json::Value::as_str),
            Some(
                "Arbor process error · process `web` · command `npm test` · exit 1 · repo `/tmp/repo`"
            )
        );
    }

    #[test]
    fn notification_webhook_text_formats_agent_started() {
        let payload = serde_json::json!({
            "event": "agent_started",
            "repo_root": "/tmp/repo",
            "worktree_path": "/tmp/repo-feature",
            "branch": "feature/test"
        });

        assert_eq!(
            notification_webhook_text("agent_started", &payload),
            "Arbor agent started · branch `feature/test` · worktree `/tmp/repo-feature` · repo `/tmp/repo`"
        );
    }

    #[test]
    fn notification_event_name_tracks_agent_state_transitions() {
        assert_eq!(
            notification_event_name_for_agent_transition(
                AgentSessionUpdateSource::Hook,
                None,
                AgentState::Working,
            ),
            Some("agent_started")
        );
        assert_eq!(
            notification_event_name_for_agent_transition(
                AgentSessionUpdateSource::Hook,
                Some(AgentState::Working),
                AgentState::Waiting,
            ),
            Some("agent_finished")
        );
        assert_eq!(
            notification_event_name_for_agent_transition(
                AgentSessionUpdateSource::Hook,
                Some(AgentState::Waiting),
                AgentState::Waiting,
            ),
            None
        );
    }

    #[test]
    fn terminal_activity_transitions_do_not_emit_notification_events() {
        assert_eq!(
            notification_event_name_for_agent_transition(
                AgentSessionUpdateSource::TerminalActivity,
                None,
                AgentState::Waiting,
            ),
            None
        );
        assert_eq!(
            notification_event_name_for_agent_transition(
                AgentSessionUpdateSource::TerminalActivity,
                Some(AgentState::Working),
                AgentState::Waiting,
            ),
            None
        );
    }

    #[test]
    fn agent_ws_clear_event_serializes_session_id() {
        let json = serde_json::to_value(AgentWsEvent::Clear {
            session_id: "terminal:daemon-1".to_owned(),
        })
        .unwrap_or_else(|error| panic!("clear event should serialize: {error}"));

        assert_eq!(
            json,
            serde_json::json!({
                "type": "clear",
                "session_id": "terminal:daemon-1",
            })
        );
    }

    #[test]
    fn notification_webhook_retry_policy_retries_transient_status_codes() {
        assert!(should_retry_notification_webhook(&ureq::Error::StatusCode(
            429
        )));
        assert!(should_retry_notification_webhook(&ureq::Error::StatusCode(
            503
        )));
        assert!(!should_retry_notification_webhook(
            &ureq::Error::StatusCode(400)
        ));
    }

    #[test]
    fn notification_webhook_retry_delay_is_bounded() {
        assert_eq!(
            notification_webhook_retry_delay(0, &ureq::Error::StatusCode(503)),
            Some(Duration::from_millis(300))
        );
        assert_eq!(
            notification_webhook_retry_delay(1, &ureq::Error::StatusCode(503)),
            Some(Duration::from_millis(1_000))
        );
        assert_eq!(
            notification_webhook_retry_delay(2, &ureq::Error::StatusCode(503)),
            None
        );
    }
}
