use {
    crate::auth,
    arbor_core::agent::AgentState,
    axum::{Json, http::StatusCode, response::Response},
    serde::{Deserialize, Serialize},
    std::{collections::HashMap, path::PathBuf, sync::Arc},
    tokio::sync::Mutex,
};

pub(crate) const HTTPD_VERSION: &str = match option_env!("ARBOR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub(crate) const AGENT_SESSION_EXPIRY_SECS: u64 = 300;
pub(crate) const LOG_BROADCAST_CAPACITY: usize = 256;

pub(crate) const PR_CACHE_TTL_SECS: u64 = 300;
pub(crate) const REPO_CACHE_TTL_SECS: u64 = 600;

/// Cached PR lookup result with expiry.
#[derive(Clone)]
pub(crate) struct PrCacheEntry {
    pub(crate) pr_number: Option<u64>,
    pub(crate) pr_url: Option<String>,
    pub(crate) fetched_at: std::time::Instant,
}

/// Cached repository metadata (GitHub slug & avatar).
#[derive(Clone)]
pub(crate) struct RepoCacheEntry {
    pub(crate) github_repo_slug: Option<String>,
    pub(crate) avatar_url: Option<String>,
    pub(crate) fetched_at: std::time::Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct AgentSession {
    pub(crate) cwd: String,
    pub(crate) state: AgentState,
    pub(crate) updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AgentNotifyRequest {
    pub(crate) hook_event_name: String,
    pub(crate) session_id: String,
    pub(crate) cwd: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum AgentWsEvent {
    Snapshot {
        sessions: Vec<arbor_daemon_client::AgentSessionDto>,
    },
    Update {
        session: arbor_daemon_client::AgentSessionDto,
    },
    Clear {
        session_id: String,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct ApiError {
    pub(crate) error: String,
}

pub(crate) type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;
pub(crate) type ApiResponse = Result<Response, (StatusCode, Json<ApiError>)>;

#[derive(Debug, Deserialize)]
pub(crate) struct WorktreeQuery {
    pub(crate) repo_root: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChangesQuery {
    pub(crate) path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SnapshotQuery {
    pub(crate) max_lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum WsClientEvent {
    Resize { cols: u16, rows: u16 },
    Signal { signal: String },
    Detach,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum WsServerEvent {
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

#[derive(Debug, Serialize)]
pub(crate) struct BindModeResponse {
    pub(crate) allow_remote: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetBindModeRequest {
    pub(crate) allow_remote: bool,
}

/// A tracing layer that formats each event as a single-line string and
/// broadcasts it over a tokio channel.  Receivers that fall behind simply
/// skip missed entries (lagged), so a slow GUI client never blocks the
/// daemon.
pub(crate) struct BroadcastLogLayer {
    pub(crate) sender: tokio::sync::broadcast::Sender<String>,
}

impl<S> tracing_subscriber::Layer<S> for BroadcastLogLayer
where
    S: tracing::Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        use std::time::{SystemTime, UNIX_EPOCH};

        let metadata = event.metadata();
        let mut visitor = LogFieldVisitor::default();
        event.record(&mut visitor);

        let level = match *metadata.level() {
            tracing::Level::ERROR => "ERROR",
            tracing::Level::WARN => "WARN",
            tracing::Level::INFO => "INFO",
            tracing::Level::DEBUG => "DEBUG",
            tracing::Level::TRACE => "TRACE",
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let ts_ms = now.as_millis() as u64;

        let fields_str = if visitor.fields.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = visitor
                .fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            format!(" {}", parts.join(" "))
        };

        let line = serde_json::json!({
            "ts": ts_ms,
            "level": level,
            "target": metadata.target(),
            "message": visitor.message,
            "fields": fields_str.trim(),
        });

        // Fire-and-forget — if no receivers are listening, this is a no-op.
        let _ = self.sender.send(line.to_string());
    }
}

#[derive(Default)]
pub(crate) struct LogFieldVisitor {
    pub(crate) message: String,
    pub(crate) fields: Vec<(String, String)>,
}

impl tracing::field::Visit for LogFieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .push((field.name().to_owned(), format!("{value:?}")));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields
                .push((field.name().to_owned(), value.to_owned()));
        }
    }
}

// ── AppState ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) repository_store: Arc<dyn crate::repository_store::RepositoryStore>,
    pub(crate) daemon: Arc<Mutex<crate::terminal_daemon::LocalTerminalDaemon>>,
    pub(crate) process_manager: Arc<Mutex<crate::process_manager::ProcessManager>>,
    #[cfg(feature = "symphony")]
    pub(crate) symphony: Option<arbor_symphony::ServiceHandle>,
    pub(crate) task_scheduler: Arc<Mutex<crate::task_scheduler::TaskScheduler>>,
    pub(crate) github_service: Arc<dyn crate::github_service::GitHubPrService>,
    pub(crate) agent_sessions: Arc<Mutex<HashMap<String, AgentSession>>>,
    pub(crate) agent_broadcast: tokio::sync::broadcast::Sender<AgentWsEvent>,
    pub(crate) log_broadcast: tokio::sync::broadcast::Sender<String>,
    pub(crate) pr_cache: Arc<Mutex<HashMap<String, PrCacheEntry>>>,
    pub(crate) repo_cache: Arc<Mutex<HashMap<String, RepoCacheEntry>>>,
    pub(crate) shutdown_signal: Arc<tokio::sync::Notify>,
    pub(crate) auth_state: auth::AuthState,
}

// ── Config ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DaemonConfig {
    pub(crate) auth_token: Option<String>,
    pub(crate) bind: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RootConfig {
    embedded_terminal_engine: Option<String>,
}

pub(crate) fn daemon_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join(".config/arbor/config.toml")
}

pub(crate) fn load_daemon_config() -> DaemonConfig {
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

pub(crate) fn load_embedded_terminal_engine_setting() -> Option<String> {
    let path = daemon_config_path();
    if !path.exists() {
        return None;
    }

    let settings = config::Config::builder()
        .add_source(config::File::from(path.as_path()).required(false))
        .build();
    match settings {
        Ok(s) => s
            .try_deserialize::<RootConfig>()
            .ok()
            .and_then(|config| config.embedded_terminal_engine)
            .and_then(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then_some(trimmed.to_owned())
            }),
        Err(_) => None,
    }
}

pub(crate) fn ensure_auth_token(config: &mut DaemonConfig) {
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

pub(crate) fn normalize_daemon_auth_token(raw: Option<String>) -> Option<String> {
    raw.and_then(|token| {
        let trimmed = token.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_owned())
    })
}

pub(crate) fn resolve_bind_addr(
    auth_token: Option<&str>,
    configured_bind: Option<&str>,
) -> Result<std::net::SocketAddr, Box<dyn std::error::Error>> {
    if let Ok(raw) = std::env::var("ARBOR_HTTPD_BIND") {
        let parsed: std::net::SocketAddr = raw.parse()?;
        return Ok(parsed);
    }

    let port = match std::env::var("ARBOR_HTTPD_PORT") {
        Ok(raw) => raw.parse::<u16>()?,
        Err(_) => 8787,
    };

    Ok(configured_bind_addr(configured_bind, auth_token, port))
}

pub(crate) fn configured_bind_addr(
    configured_bind: Option<&str>,
    auth_token: Option<&str>,
    port: u16,
) -> std::net::SocketAddr {
    match configured_bind.and_then(parse_bind_host) {
        Some(host) => format!("{host}:{port}")
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([127, 0, 0, 1], port))),
        None => default_bind_addr(auth_token, port),
    }
}

pub(crate) fn parse_bind_host(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "localhost" | "local" | "loopback" | "127.0.0.1" => Some("127.0.0.1"),
        "all" | "all-interfaces" | "public" | "0.0.0.0" | "[::]" => Some("[::]"),
        _ => None,
    }
}

pub(crate) fn default_bind_addr(auth_token: Option<&str>, port: u16) -> std::net::SocketAddr {
    if auth_token.is_some_and(|token| !token.trim().is_empty()) {
        // Bind on IPv6 wildcard — dual-stack, accepts both IPv4 and IPv6.
        std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port))
    } else {
        std::net::SocketAddr::from(([127, 0, 0, 1], port))
    }
}

/// Whether the resolved bind configuration allows remote access.
pub(crate) fn is_public_bind(auth_token: Option<&str>, configured_bind: Option<&str>) -> bool {
    match configured_bind.and_then(parse_bind_host) {
        Some("[::]") | Some("0.0.0.0") => true,
        Some(_) => false,
        None => auth_token.is_some_and(|t| !t.trim().is_empty()),
    }
}
