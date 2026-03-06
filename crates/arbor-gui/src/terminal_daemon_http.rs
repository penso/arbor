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

#[derive(Debug, Serialize)]
struct CreateTerminalRequest {
    session_id: Option<String>,
    workspace_id: String,
    cwd: String,
    shell: Option<String>,
    cols: u16,
    rows: u16,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateTerminalResponse {
    is_new_session: bool,
    session: DaemonSessionRecord,
}

#[derive(Debug, Serialize)]
struct TerminalWriteRequest {
    data: String,
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
        Ok(Self { endpoint })
    }

    pub fn base_url(&self) -> String {
        self.endpoint.display_url()
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
        };

        let response = self.send_json("POST", &format!("{API_PATH_PREFIX}/terminals"), &payload)?;
        self.decode_json_response::<CreateTerminalResponse>(response, &[200])
            .map(|value| CreateOrAttachResponse {
                is_new_session: value.is_new_session,
                session: value.session,
            })
    }

    pub fn write(&self, request: WriteRequest) -> Result<(), HttpTerminalDaemonError> {
        let data = String::from_utf8(request.bytes).map_err(|error| {
            HttpTerminalDaemonError::new(format!(
                "terminal input for session `{}` is not valid UTF-8: {error}",
                request.session_id
            ))
        })?;
        let payload = TerminalWriteRequest { data };

        let path = format!(
            "{API_PATH_PREFIX}/terminals/{}/write",
            encode_path_segment(&request.session_id)
        );
        let response = self.send_json("POST", &path, &payload)?;
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

    fn send_empty(
        &self,
        method: &str,
        path: &str,
    ) -> Result<HttpResponse, HttpTerminalDaemonError> {
        self.send_request(method, path, None)
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
        self.send_request(method, path, Some(body.as_slice()))
    }

    fn send_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, HttpTerminalDaemonError> {
        let mut stream = self.endpoint.connect()?;
        let request_path = self.endpoint.request_path(path);
        let host_header = self.endpoint.host_header();
        let body = body.unwrap_or_default();

        let mut headers = format!(
            "{method} {request_path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\nAccept: application/json\r\n"
        );

        if !body.is_empty() {
            headers.push_str("Content-Type: application/json\r\n");
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
        let mut addrs = address.to_socket_addrs().map_err(|error| {
            HttpTerminalDaemonError::new(format!(
                "failed to resolve daemon host `{}`: {error}",
                self.host
            ))
        })?;
        let Some(socket_addr) = addrs.next() else {
            return Err(HttpTerminalDaemonError::new(format!(
                "daemon host `{}` did not resolve to an address",
                self.host
            )));
        };

        let stream =
            TcpStream::connect_timeout(&socket_addr, CONNECT_TIMEOUT).map_err(|error| {
                HttpTerminalDaemonError::new(format!(
                    "failed to connect to daemon at {}:{}: {error}",
                    self.host, self.port
                ))
            })?;
        stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(|error| {
            HttpTerminalDaemonError::new(format!("failed to set read timeout: {error}"))
        })?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(|error| {
                HttpTerminalDaemonError::new(format!("failed to set write timeout: {error}"))
            })?;

        Ok(stream)
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
