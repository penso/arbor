//! Authentication middleware for arbor-httpd.
//!
//! - Localhost connections bypass authentication entirely.
//! - Remote connections require either:
//!   - A `Bearer <token>` header matching the configured auth token, or
//!   - A valid session cookie obtained by posting the token to `/login`.
//! - If no auth token is configured, remote connections are refused with
//!   a message telling the user to set `[daemon] auth_token` in their config.
//! - Repeated failed remote authentication attempts are temporarily blocked
//!   per source IP to limit brute-force and websocket reconnect abuse.

use {
    axum::{
        Router,
        body::Body,
        extract::{ConnectInfo, State},
        http::{Request, StatusCode, header},
        middleware::{self, Next},
        response::{Html, IntoResponse, Response},
        routing::{get, post},
    },
    hmac::{Hmac, Mac},
    secrecy::{ExposeSecret, SecretString},
    sha2::Sha256,
    std::{
        collections::HashMap,
        net::{IpAddr, SocketAddr},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    },
};

type HmacSha256 = Hmac<Sha256>;

const SESSION_COOKIE_NAME: &str = "arbor_session";
const SESSION_MAX_AGE_SECS: u64 = 86400 * 7; // 7 days
const AUTH_FAILURE_LIMIT: u32 = 8;
const AUTH_FAILURE_WINDOW: Duration = Duration::from_secs(60);
const AUTH_BLOCK_DURATION: Duration = Duration::from_secs(300);
const AUTH_ATTEMPT_TTL: Duration = Duration::from_secs(900);
const MAX_TRACKED_REMOTE_IPS: usize = 4096;

/// Shared auth state embedded in the app.
#[derive(Clone)]
pub struct AuthState {
    /// The configured auth token. If `None`, remote access is blocked entirely.
    /// Wrapped in `SecretString` to prevent accidental logging or display.
    pub auth_token: Option<SecretString>,
    /// Random secret generated at launch for signing session cookies.
    pub session_secret: Arc<[u8; 32]>,
    /// When `false`, non-localhost requests are rejected regardless of auth.
    /// Toggled at runtime via the `/api/v1/config/bind` endpoint.
    pub allow_remote: Arc<AtomicBool>,
    failed_attempts: Arc<Mutex<HashMap<IpAddr, FailedAuthState>>>,
}

#[derive(Debug, Clone, Copy)]
struct FailedAuthState {
    first_failure_at: Instant,
    last_failure_at: Instant,
    failures: u32,
    blocked_until: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailedAuthOutcome {
    NotBlocked,
    Blocked { retry_after: Duration },
}

impl AuthState {
    pub fn new(auth_token: Option<String>, allow_remote: bool) -> Self {
        let mut secret = [0u8; 32];
        use rand::RngExt;
        rand::rng().fill(&mut secret);
        Self {
            auth_token: auth_token
                .and_then(|token| normalize_auth_token(&token))
                .map(SecretString::from),
            session_secret: Arc::new(secret),
            allow_remote: Arc::new(AtomicBool::new(allow_remote)),
            failed_attempts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create an HMAC session cookie value from the auth token.
    fn sign_session(&self, token: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(self.session_secret.as_ref())
            .unwrap_or_else(|_| unreachable!());
        mac.update(token.as_bytes());
        let result = mac.finalize();
        hex::encode(result.into_bytes())
    }

    /// Verify a session cookie value.
    fn verify_session(&self, cookie_value: &str, token: &str) -> bool {
        let mut mac = HmacSha256::new_from_slice(self.session_secret.as_ref())
            .unwrap_or_else(|_| unreachable!());
        mac.update(token.as_bytes());
        let Ok(expected) = hex::decode(cookie_value) else {
            return false;
        };
        mac.verify_slice(&expected).is_ok()
    }

    fn blocked_retry_after(&self, ip: IpAddr) -> Option<Duration> {
        let now = Instant::now();
        let Ok(mut attempts) = self.failed_attempts.lock() else {
            return None;
        };
        prune_failed_attempts(&mut attempts, now);
        let state = attempts.get_mut(&ip)?;
        let blocked_until = state.blocked_until?;
        if blocked_until <= now {
            state.blocked_until = None;
            state.failures = 0;
            state.first_failure_at = now;
            return None;
        }
        Some(blocked_until.saturating_duration_since(now))
    }

    fn clear_failures(&self, ip: IpAddr) {
        if let Ok(mut attempts) = self.failed_attempts.lock() {
            attempts.remove(&ip);
        }
    }

    fn record_failure(&self, ip: IpAddr) -> FailedAuthOutcome {
        let now = Instant::now();
        let Ok(mut attempts) = self.failed_attempts.lock() else {
            return FailedAuthOutcome::NotBlocked;
        };

        prune_failed_attempts(&mut attempts, now);
        let state = attempts.entry(ip).or_insert(FailedAuthState {
            first_failure_at: now,
            last_failure_at: now,
            failures: 0,
            blocked_until: None,
        });

        if let Some(blocked_until) = state.blocked_until {
            if blocked_until > now {
                return FailedAuthOutcome::Blocked {
                    retry_after: blocked_until.saturating_duration_since(now),
                };
            }

            state.blocked_until = None;
            state.failures = 0;
            state.first_failure_at = now;
        }

        if now.saturating_duration_since(state.first_failure_at) > AUTH_FAILURE_WINDOW {
            state.first_failure_at = now;
            state.failures = 0;
            state.blocked_until = None;
        }

        state.last_failure_at = now;
        state.failures += 1;
        if state.failures >= AUTH_FAILURE_LIMIT {
            let blocked_until = now + AUTH_BLOCK_DURATION;
            state.blocked_until = Some(blocked_until);
            return FailedAuthOutcome::Blocked {
                retry_after: AUTH_BLOCK_DURATION,
            };
        }

        FailedAuthOutcome::NotBlocked
    }
}

fn is_loopback(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        },
    }
}

/// Axum middleware that enforces authentication on non-localhost requests.
pub async fn auth_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(auth): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let request_path = request.uri().path().to_owned();

    // Localhost always passes through
    if is_loopback(&addr) {
        return next.run(request).await;
    }

    // Remote access can be disabled at runtime (localhost-only mode).
    if !auth.allow_remote.load(Ordering::Relaxed) {
        return (
            StatusCode::FORBIDDEN,
            Html(
                "<h1>Remote access is disabled</h1>\
                 <p>This Arbor instance is configured for localhost-only access.</p>",
            ),
        )
            .into_response();
    }

    let Some(ref configured_secret) = auth.auth_token else {
        // No token configured — refuse all remote access
        tracing::warn!(
            ip = %addr.ip(),
            path = %request_path,
            "denied remote request: no auth token configured"
        );
        return (
            StatusCode::FORBIDDEN,
            Html(
                "<h1>Remote access requires authentication</h1>\
                 <p>Set <code>[daemon] auth_token = \"your-secret\"</code> in \
                 <code>~/.config/arbor/config.toml</code> to enable remote access.</p>",
            ),
        )
            .into_response();
    };

    let configured_token = configured_secret.expose_secret();

    if let Some(retry_after) = auth.blocked_retry_after(addr.ip()) {
        tracing::warn!(
            ip = %addr.ip(),
            path = %request_path,
            retry_after_secs = retry_after.as_secs().max(1),
            "blocked remote request (rate-limited)"
        );
        return blocked_response(retry_after);
    }

    // Check Bearer token header
    if let Some(auth_header) = request.headers().get("authorization")
        && let Ok(value) = auth_header.to_str()
        && let Some(bearer_token) = value.strip_prefix("Bearer ")
        && constant_time_eq(bearer_token.trim(), configured_token)
    {
        auth.clear_failures(addr.ip());
        return next.run(request).await;
    }

    // Check session cookie
    if let Some(cookie_header) = request.headers().get("cookie")
        && let Ok(cookies) = cookie_header.to_str()
    {
        for cookie in cookies.split(';') {
            let cookie = cookie.trim();
            if let Some(value) = cookie.strip_prefix(&format!("{SESSION_COOKIE_NAME}="))
                && auth.verify_session(value.trim(), configured_token)
            {
                auth.clear_failures(addr.ip());
                return next.run(request).await;
            }
        }
    }

    let has_credentials =
        request.headers().contains_key("authorization") || request.headers().contains_key("cookie");

    // Only count failures when credentials were actually provided but wrong.
    // Missing credentials (e.g. first browser visit) should not penalise the IP.
    if has_credentials {
        let failure_reason = if request.headers().contains_key("authorization") {
            "invalid bearer token"
        } else {
            "invalid session cookie"
        };

        match auth.record_failure(addr.ip()) {
            FailedAuthOutcome::Blocked { retry_after } => {
                tracing::warn!(
                    ip = %addr.ip(),
                    path = %request_path,
                    reason = failure_reason,
                    retry_after_secs = retry_after.as_secs().max(1),
                    "blocked after repeated auth failures"
                );
                return blocked_response(retry_after);
            },
            FailedAuthOutcome::NotBlocked => {
                tracing::warn!(
                    ip = %addr.ip(),
                    path = %request_path,
                    reason = failure_reason,
                    "unauthorized remote request"
                );
            },
        }
    } else {
        tracing::warn!(
            ip = %addr.ip(),
            path = %request_path,
            "unauthorized remote request (missing credentials)"
        );
    }

    StatusCode::UNAUTHORIZED.into_response()
}

/// Build a router for auth-related endpoints (login page + login POST).
/// These routes are NOT protected by the auth middleware.
pub fn auth_routes() -> Router<AuthState> {
    Router::new()
        .route("/login", get(login_page))
        .route("/login", post(handle_login))
}

async fn login_page() -> Html<&'static str> {
    Html(LOGIN_PAGE_HTML)
}

#[derive(serde::Deserialize)]
struct LoginRequest {
    token: String,
}

async fn handle_login(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(auth): State<AuthState>,
    axum::Form(form): axum::Form<LoginRequest>,
) -> Response {
    if !is_loopback(&addr)
        && let Some(retry_after) = auth.blocked_retry_after(addr.ip())
    {
        tracing::warn!(
            ip = %addr.ip(),
            retry_after_secs = retry_after.as_secs().max(1),
            "blocked login attempt (rate-limited)"
        );
        return blocked_response(retry_after);
    }

    let Some(ref configured_secret) = auth.auth_token else {
        if !is_loopback(&addr) {
            tracing::warn!(
                ip = %addr.ip(),
                "denied login attempt: no auth token configured"
            );
        }
        return (StatusCode::FORBIDDEN, "No auth token configured").into_response();
    };

    let configured_token = configured_secret.expose_secret();

    if !constant_time_eq(&form.token, configured_token) {
        if !is_loopback(&addr) {
            match auth.record_failure(addr.ip()) {
                FailedAuthOutcome::Blocked { retry_after } => {
                    tracing::warn!(
                        ip = %addr.ip(),
                        retry_after_secs = retry_after.as_secs().max(1),
                        "blocked after repeated failed login attempts"
                    );
                    return blocked_response(retry_after);
                },
                FailedAuthOutcome::NotBlocked => {
                    tracing::warn!(
                        ip = %addr.ip(),
                        "rejected login attempt: invalid token"
                    );
                },
            }
        }
        return (StatusCode::UNAUTHORIZED, Html(LOGIN_ERROR_HTML)).into_response();
    }

    if !is_loopback(&addr) {
        auth.clear_failures(addr.ip());
    }

    let session_value = auth.sign_session(configured_token);
    // No `Secure` flag — arbor-httpd serves plain HTTP. On a LAN this is
    // acceptable; if placed behind a TLS reverse proxy the proxy can add
    // the Secure attribute via its own Set-Cookie rewriting.
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={session_value}; Path=/; HttpOnly; SameSite=Strict; \
         Max-Age={SESSION_MAX_AGE_SECS}"
    );

    (
        StatusCode::SEE_OTHER,
        [("set-cookie", cookie.as_str()), ("location", "/")],
        "",
    )
        .into_response()
}

/// Outer middleware that redirects browser requests to `/login` when the inner
/// handler returns 401.  API clients (JSON, Bearer token) receive the raw 401
/// so they can handle it programmatically.
async fn redirect_unauthorized(request: Request<Body>, next: Next) -> Response {
    let accepts_html = request
        .headers()
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));

    let response = next.run(request).await;

    if response.status() == StatusCode::UNAUTHORIZED && accepts_html {
        return (StatusCode::SEE_OTHER, [(header::LOCATION, "/login")]).into_response();
    }

    response
}

/// Apply auth middleware to a router, adding login routes that bypass auth.
pub fn with_auth(app: Router, auth_state: AuthState) -> Router {
    let login_routes = auth_routes().with_state(auth_state.clone());

    // `.layer()` (not `.route_layer()`) so auth covers both matched routes
    // AND fallback services (e.g. ServeDir for the web UI static assets).
    let protected = app.layer(middleware::from_fn_with_state(auth_state, auth_middleware));

    // Merge: login routes first (unprotected), then protected routes.
    // The outer redirect layer catches 401s from the auth middleware and
    // sends browsers to /login instead.
    login_routes
        .merge(protected)
        .layer(middleware::from_fn(redirect_unauthorized))
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn normalize_auth_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then_some(trimmed.to_owned())
}

fn prune_failed_attempts(attempts: &mut HashMap<IpAddr, FailedAuthState>, now: Instant) {
    attempts.retain(|_, state| {
        state
            .blocked_until
            .is_some_and(|blocked_until| blocked_until > now)
            || now.saturating_duration_since(state.last_failure_at) <= AUTH_ATTEMPT_TTL
    });

    if attempts.len() <= MAX_TRACKED_REMOTE_IPS {
        return;
    }

    let mut by_last_failure = attempts
        .iter()
        .map(|(ip, state)| (*ip, state.last_failure_at))
        .collect::<Vec<_>>();
    by_last_failure.sort_by_key(|(_, last_failure_at)| *last_failure_at);

    for (ip, _) in by_last_failure
        .into_iter()
        .take(attempts.len().saturating_sub(MAX_TRACKED_REMOTE_IPS))
    {
        attempts.remove(&ip);
    }
}

fn blocked_response(retry_after: Duration) -> Response {
    let retry_after_secs = retry_after.as_secs().max(1);
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Html(format!(
            "<h1>Too many authentication failures</h1>\
             <p>Remote access from this IP is temporarily blocked. Try again in {retry_after_secs} seconds.</p>"
        )),
    )
        .into_response();

    if let Ok(header_value) = header::HeaderValue::from_str(&retry_after_secs.to_string()) {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, header_value);
    }

    response
}

const LOGIN_PAGE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Arbor – Login</title>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    background: #0f1115; color: #e4e4e7;
    display: flex; align-items: center; justify-content: center;
    min-height: 100vh;
  }
  .card {
    background: #1a1b1e; border: 1px solid #2a2b2e; border-radius: 12px;
    padding: 32px; width: 100%; max-width: 380px;
  }
  h1 { font-size: 20px; margin-bottom: 8px; }
  p { font-size: 13px; color: #71717a; margin-bottom: 24px; }
  label { display: block; font-size: 13px; font-weight: 500; margin-bottom: 6px; }
  input[type="password"] {
    width: 100%; padding: 10px 12px; border: 1px solid #2a2b2e; border-radius: 6px;
    background: #0f1115; color: #e4e4e7; font-size: 14px;
    font-family: ui-monospace, monospace;
  }
  input:focus { outline: none; border-color: #4ade80; }
  button {
    width: 100%; margin-top: 16px; padding: 10px; border: none; border-radius: 6px;
    background: #4ade80; color: #0f1115; font-size: 14px; font-weight: 600;
    cursor: pointer;
  }
  button:hover { background: #22c55e; }
</style>
</head>
<body>
<div class="card">
  <h1>Arbor</h1>
  <p>Enter your authentication token to access this instance remotely.</p>
  <form method="POST" action="/login">
    <label for="token">Auth Token</label>
    <input type="password" id="token" name="token" autofocus required>
    <button type="submit">Sign in</button>
  </form>
</div>
</body>
</html>"#;

const LOGIN_ERROR_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Arbor – Login Failed</title>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  body {
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    background: #0f1115; color: #e4e4e7;
    display: flex; align-items: center; justify-content: center;
    min-height: 100vh;
  }
  .card {
    background: #1a1b1e; border: 1px solid #2a2b2e; border-radius: 12px;
    padding: 32px; width: 100%; max-width: 380px;
  }
  h1 { font-size: 20px; margin-bottom: 8px; }
  .error { color: #f38ba8; font-size: 13px; margin-bottom: 16px; }
  p { font-size: 13px; color: #71717a; margin-bottom: 24px; }
  label { display: block; font-size: 13px; font-weight: 500; margin-bottom: 6px; }
  input[type="password"] {
    width: 100%; padding: 10px 12px; border: 1px solid #2a2b2e; border-radius: 6px;
    background: #0f1115; color: #e4e4e7; font-size: 14px;
    font-family: ui-monospace, monospace;
  }
  input:focus { outline: none; border-color: #4ade80; }
  button {
    width: 100%; margin-top: 16px; padding: 10px; border: none; border-radius: 6px;
    background: #4ade80; color: #0f1115; font-size: 14px; font-weight: 600;
    cursor: pointer;
  }
  button:hover { background: #22c55e; }
</style>
</head>
<body>
<div class="card">
  <h1>Arbor</h1>
  <div class="error">Invalid token. Please try again.</div>
  <form method="POST" action="/login">
    <label for="token">Auth Token</label>
    <input type="password" id="token" name="token" autofocus required>
    <button type="submit">Sign in</button>
  </form>
</div>
</body>
</html>"#;

/// Hex encoding/decoding helpers to avoid adding a `hex` dependency.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, ()> {
        if !s.len().is_multiple_of(2) {
            return Err(());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn trims_auth_tokens_on_creation() {
        let state = AuthState::new(Some("  secret-token  ".to_owned()), true);
        assert!(state.auth_token.is_some());
        assert_eq!(state.auth_token.unwrap().expose_secret(), "secret-token");

        let empty = AuthState::new(Some("   ".to_owned()), true);
        assert!(empty.auth_token.is_none());
    }

    #[test]
    fn repeated_remote_failures_temporarily_block_an_ip() {
        let state = AuthState::new(Some("secret".to_owned()), true);
        let ip = "203.0.113.5"
            .parse::<IpAddr>()
            .unwrap_or_else(|error| panic!("invalid IP literal: {error}"));

        for _ in 0..AUTH_FAILURE_LIMIT.saturating_sub(1) {
            assert_eq!(state.record_failure(ip), FailedAuthOutcome::NotBlocked);
        }

        let blocked = state.record_failure(ip);
        assert!(matches!(
            blocked,
            FailedAuthOutcome::Blocked { retry_after } if retry_after > Duration::ZERO
        ));
        assert!(state.blocked_retry_after(ip).is_some());

        state.clear_failures(ip);
        assert_eq!(state.blocked_retry_after(ip), None);
    }

    #[test]
    fn blocked_response_sets_retry_after_header() {
        let response = blocked_response(Duration::from_secs(12));
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("12")
        );
    }

    // ── Integration tests for auth middleware ─────────────────────────

    use tower::ServiceExt;

    /// Build a minimal Axum app with auth middleware protecting a single
    /// `/test` endpoint that returns 200 OK.
    fn test_app(auth_state: AuthState) -> Router {
        let inner = Router::new().route("/test", get(|| async { StatusCode::OK }));
        with_auth(inner, auth_state)
    }

    fn remote_addr() -> SocketAddr {
        "203.0.113.5:12345".parse().unwrap()
    }

    fn loopback_addr() -> SocketAddr {
        "127.0.0.1:12345".parse().unwrap()
    }

    fn make_request(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    fn make_request_with_bearer(uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    async fn oneshot_with_addr(app: Router, addr: SocketAddr, request: Request<Body>) -> Response {
        let app = app.layer(axum::extract::connect_info::MockConnectInfo(addr));
        app.oneshot(request).await.unwrap()
    }

    #[tokio::test]
    async fn localhost_only_mode_rejects_remote_requests() {
        let state = AuthState::new(Some("secret-token".to_owned()), false);
        let app = test_app(state);

        let response = oneshot_with_addr(app, remote_addr(), make_request("/test")).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn localhost_only_mode_allows_localhost() {
        let state = AuthState::new(Some("secret-token".to_owned()), false);
        let app = test_app(state);

        let response = oneshot_with_addr(app, loopback_addr(), make_request("/test")).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ipv4_mapped_loopback_is_treated_as_localhost() {
        let state = AuthState::new(Some("secret-token".to_owned()), false);
        let app = test_app(state);

        // ::ffff:127.0.0.1 is what arrives when binding on [::] and a client connects via IPv4
        let mapped_loopback: SocketAddr = "[::ffff:127.0.0.1]:12345".parse().unwrap();
        let response = oneshot_with_addr(app, mapped_loopback, make_request("/test")).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_auth_token_rejects_remote_requests() {
        let state = AuthState::new(None, true);
        let app = test_app(state);

        let response = oneshot_with_addr(app, remote_addr(), make_request("/test")).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn no_auth_token_allows_localhost() {
        let state = AuthState::new(None, true);
        let app = test_app(state);

        let response = oneshot_with_addr(app, loopback_addr(), make_request("/test")).await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn valid_bearer_token_passes_when_remote_allowed() {
        let state = AuthState::new(Some("secret-token".to_owned()), true);
        let app = test_app(state);

        let response = oneshot_with_addr(
            app,
            remote_addr(),
            make_request_with_bearer("/test", "secret-token"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_bearer_token_returns_401() {
        let state = AuthState::new(Some("secret-token".to_owned()), true);
        let app = test_app(state);

        let response = oneshot_with_addr(
            app,
            remote_addr(),
            make_request_with_bearer("/test", "wrong-token"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn missing_credentials_returns_401_not_rate_limited() {
        let state = AuthState::new(Some("secret-token".to_owned()), true);

        // Send many requests with missing credentials — should never block
        for _ in 0..(AUTH_FAILURE_LIMIT + 5) {
            let app = test_app(state.clone());
            let response = oneshot_with_addr(app, remote_addr(), make_request("/test")).await;

            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "missing credentials should return 401, not 429"
            );
        }
    }

    #[tokio::test]
    async fn allow_remote_toggle_takes_effect_immediately() {
        let state = AuthState::new(Some("secret-token".to_owned()), false);

        // Initially blocked
        let app = test_app(state.clone());
        let response = oneshot_with_addr(
            app,
            remote_addr(),
            make_request_with_bearer("/test", "secret-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        // Toggle to allow remote
        state.allow_remote.store(true, Ordering::Relaxed);

        let app = test_app(state.clone());
        let response = oneshot_with_addr(
            app,
            remote_addr(),
            make_request_with_bearer("/test", "secret-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // Toggle back to localhost-only
        state.allow_remote.store(false, Ordering::Relaxed);

        let app = test_app(state.clone());
        let response = oneshot_with_addr(
            app,
            remote_addr(),
            make_request_with_bearer("/test", "secret-token"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn login_page_accessible_without_auth() {
        let state = AuthState::new(Some("secret-token".to_owned()), true);
        let app = test_app(state);

        let response = oneshot_with_addr(app, remote_addr(), make_request("/login")).await;

        assert_eq!(response.status(), StatusCode::OK);
    }
}
