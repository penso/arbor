use {
    arbor_core::daemon::{
        CreateOrAttachRequest, CreateOrAttachResponse, DaemonSessionRecord, DetachRequest,
        KillRequest, ResizeRequest, SignalRequest, SnapshotRequest, TerminalSignal,
        TerminalSnapshot, WriteRequest,
    },
    serde::{Deserialize, Serialize, de::DeserializeOwned},
    std::{
        fmt,
        io::{Read, Write},
        net::{TcpStream, ToSocketAddrs},
        sync::{Arc, Mutex},
        time::Duration,
    },
};

const API_PATH_PREFIX: &str = "/api/v1";
const DEFAULT_DAEMON_PORT: u16 = 8787;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct HttpTerminalDaemon {
    endpoint: HttpEndpoint,
    auth_token: Arc<Mutex<Option<String>>>,
}

#[derive(Debug, Clone)]
pub struct HttpTerminalDaemonError {
    message: String,
}

impl HttpTerminalDaemonError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn is_unauthorized(&self) -> bool {
        self.message.contains("status 401")
    }

    pub fn is_forbidden(&self) -> bool {
        self.message.contains("status 403")
    }
}

impl fmt::Display for HttpTerminalDaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HttpTerminalDaemonError {}

#[derive(Debug, Clone)]
struct HttpEndpoint {
    host: String,
    port: u16,
    base_path: String,
}

#[derive(Debug)]
struct HttpResponse {
    status_code: u16,
    body: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: String,
}

#[derive(Debug, Deserialize)]
pub struct HealthInfo {
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct RemoteRepositoryDto {
    pub root: String,
    pub label: String,
    pub github_repo_slug: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct RemoteWorktreeDto {
    pub repo_root: String,
    pub path: String,
    pub branch: String,
    pub is_primary_checkout: bool,
    pub last_activity_unix_ms: Option<u64>,
    pub diff_additions: Option<usize>,
    pub diff_deletions: Option<usize>,
    pub pr_number: Option<u64>,
    pub pr_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct SetBindModeRequest {
    allow_remote: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebsocketConnectConfig {
    pub url: String,
    pub auth_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateTerminalRequest {
    session_id: Option<String>,
    workspace_id: String,
    cwd: String,
    shell: Option<String>,
    cols: u16,
    rows: u16,
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateTerminalResponse {
    is_new_session: bool,
    session: DaemonSessionRecord,
}

#[derive(Debug, Serialize)]
struct TerminalResizeRequest {
    cols: u16,
    rows: u16,
}

#[derive(Debug, Serialize)]
struct TerminalSignalRequest {
    signal: &'static str,
}

impl HttpTerminalDaemon {
    pub fn new(base_url: &str) -> Result<Self, HttpTerminalDaemonError> {
        let endpoint = HttpEndpoint::parse(base_url)?;
        Ok(Self {
            endpoint,
            auth_token: Arc::new(Mutex::new(None)),
        })
    }

    pub fn set_auth_token(&self, token: Option<String>) {
        if let Ok(mut auth_token) = self.auth_token.lock() {
            *auth_token = token;
        }
    }

    pub fn base_url(&self) -> String {
        self.endpoint.display_url()
    }

    pub fn websocket_connect_config(
        &self,
        path: &str,
    ) -> Result<WebsocketConnectConfig, HttpTerminalDaemonError> {
        Ok(WebsocketConnectConfig {
            url: self.endpoint.websocket_url(path),
            auth_token: self.auth_token.lock().ok().and_then(|guard| guard.clone()),
        })
    }

    pub fn terminal_websocket_config(
        &self,
        session_id: &str,
    ) -> Result<WebsocketConnectConfig, HttpTerminalDaemonError> {
        let path = format!(
            "{API_PATH_PREFIX}/terminals/{}/ws",
            encode_path_segment(session_id)
        );
        self.websocket_connect_config(&path)
    }

    pub fn create_or_attach(
        &self,
        request: CreateOrAttachRequest,
    ) -> Result<CreateOrAttachResponse, HttpTerminalDaemonError> {
        let payload = CreateTerminalRequest {
            session_id: (!request.session_id.trim().is_empty()).then_some(request.session_id),
            workspace_id: request.workspace_id,
            cwd: request.cwd.display().to_string(),
            shell: (!request.shell.trim().is_empty()).then_some(request.shell),
            cols: request.cols,
            rows: request.rows,
            title: request.title.filter(|value| !value.trim().is_empty()),
            command: request.command,
        };

        let response = self.send_json("POST", &format!("{API_PATH_PREFIX}/terminals"), &payload)?;
        self.decode_json_response::<CreateTerminalResponse>(response, &[200])
            .map(|value| CreateOrAttachResponse {
                is_new_session: value.is_new_session,
                session: value.session,
            })
    }

    pub fn write(&self, request: WriteRequest) -> Result<(), HttpTerminalDaemonError> {
        let path = format!(
            "{API_PATH_PREFIX}/terminals/{}/write",
            encode_path_segment(&request.session_id)
        );
        let response = self.send_bytes("POST", &path, &request.bytes)?;
        self.expect_status(response, &[204])
    }

    pub fn resize(&self, request: ResizeRequest) -> Result<(), HttpTerminalDaemonError> {
        let payload = TerminalResizeRequest {
            cols: request.cols,
            rows: request.rows,
        };
        let path = format!(
            "{API_PATH_PREFIX}/terminals/{}/resize",
            encode_path_segment(&request.session_id)
        );
        let response = self.send_json("POST", &path, &payload)?;
        self.expect_status(response, &[204])
    }

    pub fn signal(&self, request: SignalRequest) -> Result<(), HttpTerminalDaemonError> {
        let signal = match request.signal {
            TerminalSignal::Interrupt => "interrupt",
            TerminalSignal::Terminate => "terminate",
            TerminalSignal::Kill => "kill",
        };
        let payload = TerminalSignalRequest { signal };
        let path = format!(
            "{API_PATH_PREFIX}/terminals/{}/signal",
            encode_path_segment(&request.session_id)
        );
        let response = self.send_json("POST", &path, &payload)?;
        self.expect_status(response, &[204])
    }

    pub fn detach(&self, request: DetachRequest) -> Result<(), HttpTerminalDaemonError> {
        let path = format!(
            "{API_PATH_PREFIX}/terminals/{}/detach",
            encode_path_segment(&request.session_id)
        );
        let response = self.send_empty("POST", &path)?;
        self.expect_status(response, &[204])
    }

    pub fn kill(&self, request: KillRequest) -> Result<(), HttpTerminalDaemonError> {
        let path = format!(
            "{API_PATH_PREFIX}/terminals/{}",
            encode_path_segment(&request.session_id)
        );
        let response = self.send_empty("DELETE", &path)?;
        self.expect_status(response, &[204])
    }

    pub fn snapshot(
        &self,
        request: SnapshotRequest,
    ) -> Result<Option<TerminalSnapshot>, HttpTerminalDaemonError> {
        let path = format!(
            "{API_PATH_PREFIX}/terminals/{}/snapshot?max_lines={}",
            encode_path_segment(&request.session_id),
            request.max_lines.clamp(1, 2_000)
        );
        let response = self.send_empty("GET", &path)?;
        if response.status_code == 404 {
            return Ok(None);
        }

        self.decode_json_response(response, &[200]).map(Some)
    }

    pub fn list_sessions(&self) -> Result<Vec<DaemonSessionRecord>, HttpTerminalDaemonError> {
        let response = self.send_empty("GET", &format!("{API_PATH_PREFIX}/terminals"))?;
        self.decode_json_response(response, &[200])
    }

    pub fn health(&self) -> Result<HealthInfo, HttpTerminalDaemonError> {
        let response = self.send_empty("GET", &format!("{API_PATH_PREFIX}/health"))?;
        self.decode_json_response(response, &[200])
    }

    pub fn list_repositories(&self) -> Result<Vec<RemoteRepositoryDto>, HttpTerminalDaemonError> {
        let response = self.send_empty("GET", &format!("{API_PATH_PREFIX}/repositories"))?;
        self.decode_json_response(response, &[200])
    }

    pub fn list_worktrees(&self) -> Result<Vec<RemoteWorktreeDto>, HttpTerminalDaemonError> {
        let response = self.send_empty("GET", &format!("{API_PATH_PREFIX}/worktrees"))?;
        self.decode_json_response(response, &[200])
    }

    pub fn shutdown(&self) -> Result<(), HttpTerminalDaemonError> {
        let response = self.send_empty("POST", &format!("{API_PATH_PREFIX}/shutdown"))?;
        self.expect_status(response, &[200])
    }

    pub fn set_bind_mode(&self, allow_remote: bool) -> Result<(), HttpTerminalDaemonError> {
        let response = self.send_json(
            "POST",
            &format!("{API_PATH_PREFIX}/config/bind"),
            &SetBindModeRequest { allow_remote },
        )?;
        self.expect_status(response, &[200])
    }

    fn send_empty(
        &self,
        method: &str,
        path: &str,
    ) -> Result<HttpResponse, HttpTerminalDaemonError> {
        self.send_request(method, path, None, None)
    }

    fn send_json<T: Serialize>(
        &self,
        method: &str,
        path: &str,
        payload: &T,
    ) -> Result<HttpResponse, HttpTerminalDaemonError> {
        let body = serde_json::to_vec(payload).map_err(|error| {
            HttpTerminalDaemonError::new(format!("failed to encode request payload: {error}"))
        })?;
        self.send_request(
            method,
            path,
            Some(body.as_slice()),
            Some("application/json"),
        )
    }

    fn send_bytes(
        &self,
        method: &str,
        path: &str,
        payload: &[u8],
    ) -> Result<HttpResponse, HttpTerminalDaemonError> {
        self.send_request(
            method,
            path,
            Some(payload),
            Some("application/octet-stream"),
        )
    }

    fn send_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Result<HttpResponse, HttpTerminalDaemonError> {
        let mut stream = self.endpoint.connect()?;
        let request_path = self.endpoint.request_path(path);
        let host_header = self.endpoint.host_header();
        let body = body.unwrap_or_default();

        let mut headers = format!(
            "{method} {request_path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\nAccept: application/json\r\n"
        );

        if let Some(token) = self.auth_token.lock().ok().and_then(|guard| guard.clone()) {
            headers.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        if !body.is_empty() {
            if let Some(content_type) = content_type {
                headers.push_str(&format!("Content-Type: {content_type}\r\n"));
            }
            headers.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        headers.push_str("\r\n");

        stream.write_all(headers.as_bytes()).map_err(|error| {
            HttpTerminalDaemonError::new(format!("failed to write request: {error}"))
        })?;
        if !body.is_empty() {
            stream.write_all(body).map_err(|error| {
                HttpTerminalDaemonError::new(format!("failed to write request body: {error}"))
            })?;
        }
        stream.flush().map_err(|error| {
            HttpTerminalDaemonError::new(format!("failed to flush request: {error}"))
        })?;

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(|error| {
            HttpTerminalDaemonError::new(format!("failed to read response: {error}"))
        })?;

        parse_http_response(raw)
    }

    fn expect_status(
        &self,
        response: HttpResponse,
        expected: &[u16],
    ) -> Result<(), HttpTerminalDaemonError> {
        if expected.contains(&response.status_code) {
            return Ok(());
        }

        Err(self.error_from_response(response, expected))
    }

    fn decode_json_response<T: DeserializeOwned>(
        &self,
        response: HttpResponse,
        expected: &[u16],
    ) -> Result<T, HttpTerminalDaemonError> {
        if !expected.contains(&response.status_code) {
            return Err(self.error_from_response(response, expected));
        }

        serde_json::from_slice(&response.body).map_err(|error| {
            HttpTerminalDaemonError::new(format!(
                "failed to decode daemon response as JSON: {error}"
            ))
        })
    }

    fn error_from_response(
        &self,
        response: HttpResponse,
        expected: &[u16],
    ) -> HttpTerminalDaemonError {
        let expected_codes = expected
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ");

        let api_error = serde_json::from_slice::<ApiError>(&response.body)
            .ok()
            .map(|error| error.error);
        let fallback_body = String::from_utf8(response.body)
            .ok()
            .map(|body| body.trim().to_owned())
            .filter(|body| !body.is_empty());
        let message = api_error
            .or(fallback_body)
            .unwrap_or_else(|| "daemon request failed".to_owned());

        HttpTerminalDaemonError::new(format!(
            "daemon request failed with status {} (expected {expected_codes}): {message}",
            response.status_code
        ))
    }
}

pub trait TerminalDaemonClient: Send + Sync {
    fn base_url(&self) -> String;
    fn set_auth_token(&self, token: Option<String>);
    fn websocket_connect_config(
        &self,
        path: &str,
    ) -> Result<WebsocketConnectConfig, HttpTerminalDaemonError>;
    fn terminal_websocket_config(
        &self,
        session_id: &str,
    ) -> Result<WebsocketConnectConfig, HttpTerminalDaemonError>;
    fn create_or_attach(
        &self,
        request: CreateOrAttachRequest,
    ) -> Result<CreateOrAttachResponse, HttpTerminalDaemonError>;
    fn write(&self, request: WriteRequest) -> Result<(), HttpTerminalDaemonError>;
    fn resize(&self, request: ResizeRequest) -> Result<(), HttpTerminalDaemonError>;
    fn signal(&self, request: SignalRequest) -> Result<(), HttpTerminalDaemonError>;
    fn detach(&self, request: DetachRequest) -> Result<(), HttpTerminalDaemonError>;
    fn kill(&self, request: KillRequest) -> Result<(), HttpTerminalDaemonError>;
    fn snapshot(
        &self,
        request: SnapshotRequest,
    ) -> Result<Option<TerminalSnapshot>, HttpTerminalDaemonError>;
    fn list_sessions(&self) -> Result<Vec<DaemonSessionRecord>, HttpTerminalDaemonError>;
    fn health(&self) -> Result<HealthInfo, HttpTerminalDaemonError>;
    fn shutdown(&self) -> Result<(), HttpTerminalDaemonError>;
    fn set_bind_mode(&self, allow_remote: bool) -> Result<(), HttpTerminalDaemonError>;
}

impl TerminalDaemonClient for HttpTerminalDaemon {
    fn base_url(&self) -> String {
        HttpTerminalDaemon::base_url(self)
    }

    fn set_auth_token(&self, token: Option<String>) {
        HttpTerminalDaemon::set_auth_token(self, token);
    }

    fn websocket_connect_config(
        &self,
        path: &str,
    ) -> Result<WebsocketConnectConfig, HttpTerminalDaemonError> {
        HttpTerminalDaemon::websocket_connect_config(self, path)
    }

    fn terminal_websocket_config(
        &self,
        session_id: &str,
    ) -> Result<WebsocketConnectConfig, HttpTerminalDaemonError> {
        HttpTerminalDaemon::terminal_websocket_config(self, session_id)
    }

    fn create_or_attach(
        &self,
        request: CreateOrAttachRequest,
    ) -> Result<CreateOrAttachResponse, HttpTerminalDaemonError> {
        HttpTerminalDaemon::create_or_attach(self, request)
    }

    fn write(&self, request: WriteRequest) -> Result<(), HttpTerminalDaemonError> {
        HttpTerminalDaemon::write(self, request)
    }

    fn resize(&self, request: ResizeRequest) -> Result<(), HttpTerminalDaemonError> {
        HttpTerminalDaemon::resize(self, request)
    }

    fn signal(&self, request: SignalRequest) -> Result<(), HttpTerminalDaemonError> {
        HttpTerminalDaemon::signal(self, request)
    }

    fn detach(&self, request: DetachRequest) -> Result<(), HttpTerminalDaemonError> {
        HttpTerminalDaemon::detach(self, request)
    }

    fn kill(&self, request: KillRequest) -> Result<(), HttpTerminalDaemonError> {
        HttpTerminalDaemon::kill(self, request)
    }

    fn snapshot(
        &self,
        request: SnapshotRequest,
    ) -> Result<Option<TerminalSnapshot>, HttpTerminalDaemonError> {
        HttpTerminalDaemon::snapshot(self, request)
    }

    fn list_sessions(&self) -> Result<Vec<DaemonSessionRecord>, HttpTerminalDaemonError> {
        HttpTerminalDaemon::list_sessions(self)
    }

    fn health(&self) -> Result<HealthInfo, HttpTerminalDaemonError> {
        HttpTerminalDaemon::health(self)
    }

    fn shutdown(&self) -> Result<(), HttpTerminalDaemonError> {
        HttpTerminalDaemon::shutdown(self)
    }

    fn set_bind_mode(&self, allow_remote: bool) -> Result<(), HttpTerminalDaemonError> {
        HttpTerminalDaemon::set_bind_mode(self, allow_remote)
    }
}

pub type SharedTerminalDaemonClient = Arc<dyn TerminalDaemonClient>;

pub fn default_terminal_daemon_client(
    base_url: &str,
) -> Result<SharedTerminalDaemonClient, HttpTerminalDaemonError> {
    Ok(Arc::new(HttpTerminalDaemon::new(base_url)?))
}

impl HttpEndpoint {
    fn parse(raw: &str) -> Result<Self, HttpTerminalDaemonError> {
        let trimmed = raw.trim();
        let without_scheme = trimmed.strip_prefix("http://").ok_or_else(|| {
            HttpTerminalDaemonError::new(
                "daemon URL must use the `http://` scheme (for example http://127.0.0.1:8787)",
            )
        })?;
        if without_scheme.trim().is_empty() {
            return Err(HttpTerminalDaemonError::new("daemon URL is empty"));
        }

        let (authority, tail) = match without_scheme.split_once('/') {
            Some((authority, tail)) => (authority, tail),
            None => (without_scheme, ""),
        };
        if authority.trim().is_empty() {
            return Err(HttpTerminalDaemonError::new("daemon URL is missing a host"));
        }

        let (host, port) = parse_host_and_port(authority)?;
        let base_path = normalize_base_path(tail);

        Ok(Self {
            host,
            port,
            base_path,
        })
    }

    fn connect(&self) -> Result<TcpStream, HttpTerminalDaemonError> {
        let address = self.host_header();
        tracing::debug!(host = %self.host, port = self.port, %address, "resolving daemon address");

        let addrs: Vec<_> = address
            .to_socket_addrs()
            .map_err(|error| {
                tracing::warn!(host = %self.host, %error, "DNS resolution failed");
                HttpTerminalDaemonError::new(format!(
                    "failed to resolve daemon host `{}`: {error}",
                    self.host
                ))
            })?
            .collect();

        if addrs.is_empty() {
            tracing::warn!(host = %self.host, "DNS resolution returned no addresses");
            return Err(HttpTerminalDaemonError::new(format!(
                "daemon host `{}` did not resolve to an address",
                self.host
            )));
        }

        tracing::debug!(
            host = %self.host,
            addresses = %addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", "),
            "resolved {} address(es)",
            addrs.len()
        );

        // Try all resolved addresses (IPv4 and IPv6) — `.local` hostnames
        // often resolve to IPv6 first, but the daemon may only listen on IPv4.
        let mut last_error = None;
        for (i, socket_addr) in addrs.iter().enumerate() {
            tracing::debug!(
                addr = %socket_addr,
                attempt = i + 1,
                total = addrs.len(),
                "attempting TCP connection"
            );
            match TcpStream::connect_timeout(socket_addr, CONNECT_TIMEOUT) {
                Ok(stream) => {
                    tracing::debug!(
                        addr = %socket_addr,
                        host = %self.host,
                        "connected to daemon"
                    );
                    let _ = stream.set_nodelay(true);
                    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(|error| {
                        HttpTerminalDaemonError::new(format!("failed to set read timeout: {error}"))
                    })?;
                    stream
                        .set_write_timeout(Some(IO_TIMEOUT))
                        .map_err(|error| {
                            HttpTerminalDaemonError::new(format!(
                                "failed to set write timeout: {error}"
                            ))
                        })?;
                    return Ok(stream);
                },
                Err(error) => {
                    tracing::debug!(
                        addr = %socket_addr,
                        %error,
                        "connection attempt failed, trying next address"
                    );
                    last_error = Some(error);
                },
            }
        }

        let err_msg = last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no addresses resolved".to_owned());
        tracing::warn!(
            host = %self.host,
            port = self.port,
            error = %err_msg,
            "all connection attempts failed"
        );
        Err(HttpTerminalDaemonError::new(format!(
            "failed to connect to daemon at {}:{}: {err_msg}",
            self.host, self.port,
        )))
    }

    fn request_path(&self, path: &str) -> String {
        let normalized = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };

        if self.base_path.is_empty() {
            return normalized;
        }

        if normalized == "/" {
            return self.base_path.clone();
        }

        format!("{}{}", self.base_path, normalized)
    }

    fn host_header(&self) -> String {
        if self.host.contains(':') {
            return format!("[{}]:{}", self.host, self.port);
        }

        format!("{}:{}", self.host, self.port)
    }

    fn display_url(&self) -> String {
        let authority = self.host_header();
        if self.base_path.is_empty() {
            return format!("http://{authority}");
        }

        format!("http://{authority}{}", self.base_path)
    }

    fn websocket_url(&self, path: &str) -> String {
        let request_path = self.request_path(path);
        format!("ws://{}{}", self.host_header(), request_path)
    }
}

fn parse_host_and_port(value: &str) -> Result<(String, u16), HttpTerminalDaemonError> {
    if let Some(rest) = value.strip_prefix('[') {
        let Some((host, suffix)) = rest.split_once(']') else {
            return Err(HttpTerminalDaemonError::new(
                "invalid daemon URL host: missing closing `]` for IPv6 address",
            ));
        };
        if host.trim().is_empty() {
            return Err(HttpTerminalDaemonError::new("daemon URL host is empty"));
        }

        if suffix.is_empty() {
            return Ok((host.to_owned(), DEFAULT_DAEMON_PORT));
        }

        let port_text = suffix.strip_prefix(':').ok_or_else(|| {
            HttpTerminalDaemonError::new("invalid daemon URL: unexpected characters after host")
        })?;
        let port = parse_port(port_text)?;
        return Ok((host.to_owned(), port));
    }

    let Some((host, port_text)) = value.rsplit_once(':') else {
        return Ok((value.to_owned(), DEFAULT_DAEMON_PORT));
    };

    if host.contains(':') {
        return Err(HttpTerminalDaemonError::new(
            "IPv6 daemon hosts must be wrapped with brackets, for example http://[::1]:8787",
        ));
    }
    if host.trim().is_empty() {
        return Err(HttpTerminalDaemonError::new("daemon URL host is empty"));
    }

    let port = parse_port(port_text)?;
    Ok((host.to_owned(), port))
}

fn parse_port(value: &str) -> Result<u16, HttpTerminalDaemonError> {
    value.parse::<u16>().map_err(|error| {
        HttpTerminalDaemonError::new(format!("invalid daemon URL port `{value}`: {error}"))
    })
}

fn normalize_base_path(raw: &str) -> String {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }

    format!("/{}", trimmed)
}

fn parse_http_response(raw: Vec<u8>) -> Result<HttpResponse, HttpTerminalDaemonError> {
    let Some(header_end) = find_subslice(&raw, b"\r\n\r\n") else {
        return Err(HttpTerminalDaemonError::new(
            "invalid daemon response: missing HTTP header separator",
        ));
    };

    let header_bytes = &raw[..header_end];
    let body_bytes = &raw[header_end + 4..];
    let headers = String::from_utf8(header_bytes.to_vec()).map_err(|error| {
        HttpTerminalDaemonError::new(format!("invalid daemon response header encoding: {error}"))
    })?;
    let mut lines = headers.lines();
    let Some(status_line) = lines.next() else {
        return Err(HttpTerminalDaemonError::new(
            "invalid daemon response: missing status line",
        ));
    };

    let status_code = parse_status_code(status_line)?;
    let is_chunked = lines.any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
        })
    });

    let body = if is_chunked {
        decode_chunked_body(body_bytes)?
    } else {
        body_bytes.to_vec()
    };

    Ok(HttpResponse { status_code, body })
}

fn parse_status_code(status_line: &str) -> Result<u16, HttpTerminalDaemonError> {
    let mut parts = status_line.split_whitespace();
    let _http_version = parts.next();
    let code = parts.next().ok_or_else(|| {
        HttpTerminalDaemonError::new("invalid daemon response: missing HTTP status code")
    })?;

    code.parse::<u16>().map_err(|error| {
        HttpTerminalDaemonError::new(format!(
            "invalid daemon response status code `{code}`: {error}"
        ))
    })
}

fn decode_chunked_body(bytes: &[u8]) -> Result<Vec<u8>, HttpTerminalDaemonError> {
    let mut cursor = 0;
    let mut output = Vec::new();

    loop {
        let Some(line_end) = find_subslice_from(bytes, b"\r\n", cursor) else {
            return Err(HttpTerminalDaemonError::new(
                "invalid chunked response: missing chunk size delimiter",
            ));
        };

        let size_line = &bytes[cursor..line_end];
        let size_text = String::from_utf8(size_line.to_vec()).map_err(|error| {
            HttpTerminalDaemonError::new(format!("invalid chunk size encoding: {error}"))
        })?;
        let size_hex = size_text
            .split(';')
            .next()
            .map(str::trim)
            .unwrap_or_default();
        let chunk_size = usize::from_str_radix(size_hex, 16).map_err(|error| {
            HttpTerminalDaemonError::new(format!("invalid chunk size `{size_hex}`: {error}"))
        })?;
        cursor = line_end + 2;

        if chunk_size == 0 {
            break;
        }

        if cursor.saturating_add(chunk_size) > bytes.len() {
            return Err(HttpTerminalDaemonError::new(
                "invalid chunked response: chunk exceeds response length",
            ));
        }

        output.extend_from_slice(&bytes[cursor..cursor + chunk_size]);
        cursor += chunk_size;

        if cursor.saturating_add(2) > bytes.len() || &bytes[cursor..cursor + 2] != b"\r\n" {
            return Err(HttpTerminalDaemonError::new(
                "invalid chunked response: missing chunk terminator",
            ));
        }
        cursor += 2;
    }

    Ok(output)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    find_subslice_from(haystack, needle, 0)
}

fn find_subslice_from(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if needle.is_empty() || start >= haystack.len() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| start + offset)
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(*byte));
            },
            _ => {
                encoded.push('%');
                encoded.push(hex_upper(byte >> 4));
                encoded.push(hex_upper(byte & 0x0f));
            },
        }
    }
    encoded
}

fn hex_upper(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'A' + (value - 10)),
        _ => '0',
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        std::{net::TcpListener, sync::mpsc, thread},
    };

    #[derive(Debug)]
    struct CapturedRequest {
        headers: String,
        body: Vec<u8>,
    }

    #[test]
    fn write_sends_raw_octets_with_binary_content_type() {
        let (base_url, receiver, handle) = spawn_capture_server();
        let daemon = match HttpTerminalDaemon::new(&base_url) {
            Ok(daemon) => daemon,
            Err(error) => panic!("failed to create daemon client: {error}"),
        };
        let payload = vec![0xff, 0x00, 0x1b, b'[', b'2', b'J'];

        if let Err(error) = daemon.write(WriteRequest {
            session_id: "daemon-raw".to_owned(),
            bytes: payload.clone(),
        }) {
            panic!("write request failed: {error}");
        }

        let captured = match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(captured) => captured,
            Err(error) => panic!("did not capture HTTP request: {error}"),
        };
        assert!(
            captured
                .headers
                .starts_with("POST /api/v1/terminals/daemon-raw/write HTTP/1.1\r\n"),
            "unexpected request line: {:?}",
            captured.headers
        );
        assert!(
            captured
                .headers
                .to_ascii_lowercase()
                .contains("content-type: application/octet-stream\r\n"),
            "missing binary content type: {:?}",
            captured.headers
        );
        assert_eq!(captured.body, payload);

        match handle.join() {
            Ok(()) => {},
            Err(_) => panic!("capture server thread panicked"),
        }
    }

    #[test]
    fn terminal_websocket_config_uses_encoded_session_id_and_auth_token() {
        let daemon = match HttpTerminalDaemon::new("http://127.0.0.1:8787/arbor") {
            Ok(daemon) => daemon,
            Err(error) => panic!("failed to create daemon client: {error}"),
        };
        daemon.set_auth_token(Some("secret-token".to_owned()));

        let config = match daemon.terminal_websocket_config("tmux/demo 1") {
            Ok(config) => config,
            Err(error) => panic!("failed to build websocket config: {error}"),
        };

        assert_eq!(
            config.url,
            "ws://127.0.0.1:8787/arbor/api/v1/terminals/tmux%2Fdemo%201/ws"
        );
        assert_eq!(config.auth_token.as_deref(), Some("secret-token"));
    }

    #[test]
    fn websocket_connect_config_preserves_auth_token_for_agent_paths() {
        let daemon = match HttpTerminalDaemon::new("http://127.0.0.1:8787") {
            Ok(daemon) => daemon,
            Err(error) => panic!("failed to create daemon client: {error}"),
        };
        daemon.set_auth_token(Some("agent-secret".to_owned()));

        let config = match daemon.websocket_connect_config("/api/v1/agent/activity/ws") {
            Ok(config) => config,
            Err(error) => panic!("failed to build websocket config: {error}"),
        };

        assert_eq!(config.url, "ws://127.0.0.1:8787/api/v1/agent/activity/ws");
        assert_eq!(config.auth_token.as_deref(), Some("agent-secret"));
    }

    fn spawn_capture_server() -> (
        String,
        mpsc::Receiver<CapturedRequest>,
        thread::JoinHandle<()>,
    ) {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) => panic!("failed to bind capture server: {error}"),
        };
        let local_addr = match listener.local_addr() {
            Ok(local_addr) => local_addr,
            Err(error) => panic!("failed to read capture server address: {error}"),
        };
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) => panic!("failed to accept capture connection: {error}"),
            };
            if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                panic!("failed to set capture read timeout: {error}");
            }

            let request = match read_http_request(&mut stream) {
                Ok(request) => request,
                Err(error) => panic!("failed to read HTTP request: {error}"),
            };
            if let Err(error) = sender.send(request) {
                panic!("failed to send captured request: {error}");
            }
            if let Err(error) =
                stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            {
                panic!("failed to write HTTP response: {error}");
            }
            if let Err(error) = stream.flush() {
                panic!("failed to flush HTTP response: {error}");
            }
        });

        (
            format!("http://127.0.0.1:{}", local_addr.port()),
            receiver,
            handle,
        )
    }

    fn read_http_request(stream: &mut TcpStream) -> Result<CapturedRequest, String> {
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 1024];
        let (header_end, content_length) = loop {
            let bytes_read = stream
                .read(&mut buffer)
                .map_err(|error| format!("failed to read request bytes: {error}"))?;
            if bytes_read == 0 {
                return Err("unexpected EOF while reading request".to_owned());
            }
            raw.extend_from_slice(&buffer[..bytes_read]);

            let Some(header_end) = find_subslice(&raw, b"\r\n\r\n") else {
                continue;
            };
            let content_length = parse_content_length(&raw[..header_end])?;
            if raw.len() >= header_end + 4 + content_length {
                break (header_end, content_length);
            }
        };

        let headers = String::from_utf8(raw[..header_end].to_vec())
            .map_err(|error| format!("request headers were not valid UTF-8: {error}"))?;
        let body_start = header_end + 4;
        let body_end = body_start + content_length;
        Ok(CapturedRequest {
            headers,
            body: raw[body_start..body_end].to_vec(),
        })
    }

    fn parse_content_length(header_bytes: &[u8]) -> Result<usize, String> {
        let headers = String::from_utf8(header_bytes.to_vec())
            .map_err(|error| format!("request headers were not valid UTF-8: {error}"))?;
        for line in headers.lines() {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if !name.trim().eq_ignore_ascii_case("content-length") {
                continue;
            }

            return value
                .trim()
                .parse::<usize>()
                .map_err(|error| format!("invalid content length: {error}"));
        }

        Ok(0)
    }
}
