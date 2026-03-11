//! Free helper functions extracted from the monolithic main.rs.

#[allow(unused_imports)]
use crate::*;
use {
    crate::{
        app_config,
        checkout::CheckoutKind,
        constants::*,
        repository_store,
        terminal_backend::{
            EmbeddedTerminal, TerminalCursor, TerminalModes, TerminalStyledCell,
            TerminalStyledLine, TerminalStyledRun,
        },
        terminal_daemon_http,
        terminal_runtime::{
            DaemonTerminalRuntime, DaemonTerminalWsState, EmulatorTerminalRuntime,
            RuntimeExitLabels, SharedTerminalRuntime, TerminalRuntimeKind,
        },
        theme::ThemePalette,
        types::{
            AgentPreset, AgentPresetKind, ConnectHostTarget, OutpostSummary, RepoPreset,
            RepositorySummary, SshDaemonTarget, TerminalSession, TerminalState, WorktreeSummary,
        },
    },
    arbor_core::{
        changes::ChangeKind,
        daemon::{self, DaemonSessionRecord, TerminalSessionState},
        worktree,
    },
    gpui::{Keystroke, Pixels, px},
    ropey::Rope,
    std::{
        collections::{HashMap, HashSet},
        env, fs,
        net::TcpListener,
        path::{Path, PathBuf},
        process::{Command, Stdio},
        sync::{Arc, OnceLock},
        time::{Duration, Instant, SystemTime},
    },
    syntect::{easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet},
};

pub(crate) fn is_command_in_path(command: &str) -> bool {
    use std::env;
    let path_var = env::var_os("PATH").unwrap_or_default();
    env::split_paths(&path_var).any(|dir| dir.join(command).is_file())
}

/// Returns `true` when the platform is expected to support native file-picker
/// dialogs. On macOS/Windows this is always true. On Linux it checks whether
/// `xdg-desktop-portal` is installed (required by the ashpd-based file picker
/// in GPUI for both X11 and Wayland).
pub(crate) fn has_native_file_picker() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        true
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        is_command_in_path("xdg-desktop-portal")
    }
}

/// Return the set of `AgentPresetKind` variants whose CLI is found in PATH.
/// Cached for the lifetime of the process (the set of installed tools is
/// unlikely to change while the app is running).
pub(crate) fn installed_preset_kinds() -> &'static HashSet<AgentPresetKind> {
    static INSTALLED: OnceLock<HashSet<AgentPresetKind>> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        AgentPresetKind::ORDER
            .iter()
            .copied()
            .filter(|kind| kind.is_installed())
            .collect()
    })
}

pub(crate) fn default_agent_presets() -> Vec<AgentPreset> {
    AgentPresetKind::ORDER
        .iter()
        .copied()
        .map(|kind| AgentPreset {
            kind,
            command: kind.default_command().to_owned(),
        })
        .collect()
}

pub(crate) fn normalize_agent_presets(
    configured: &[app_config::AgentPresetConfig],
) -> Vec<AgentPreset> {
    let mut presets = default_agent_presets();

    for configured_preset in configured {
        let Some(kind) = AgentPresetKind::from_key(&configured_preset.key) else {
            continue;
        };
        let command = configured_preset.command.trim();
        if command.is_empty() {
            continue;
        }
        if let Some(preset) = presets.iter_mut().find(|preset| preset.kind == kind) {
            preset.command = command.to_owned();
        }
    }

    presets
}

pub(crate) fn load_repo_presets(
    store: &dyn app_config::AppConfigStore,
    repo_root: &Path,
) -> Vec<RepoPreset> {
    let Some(config) = store.load_repo_config(repo_root) else {
        return Vec::new();
    };
    config
        .presets
        .into_iter()
        .filter(|p| !p.name.trim().is_empty() && !p.command.trim().is_empty())
        .map(|p| RepoPreset {
            name: p.name.trim().to_owned(),
            icon: p.icon.trim().to_owned(),
            command: p.command.trim().to_owned(),
        })
        .collect()
}

pub(crate) fn local_embedded_runtime(runtime: EmbeddedTerminal) -> SharedTerminalRuntime {
    Arc::new(EmulatorTerminalRuntime {
        backend: runtime,
        kind: TerminalRuntimeKind::Local,
        resize_error_label: "failed to resize terminal",
        exit_labels: RuntimeExitLabels {
            completed_title: "Terminal completed",
            failed_title: "Terminal failed",
            failed_notice_prefix: "terminal tab",
        },
    })
}

pub(crate) fn local_daemon_runtime(
    daemon: terminal_daemon_http::SharedTerminalDaemonClient,
    session_id: String,
    poll_notify: Option<std::sync::mpsc::Sender<()>>,
) -> SharedTerminalRuntime {
    let ws_state = Arc::new(DaemonTerminalWsState::new(poll_notify));
    spawn_daemon_terminal_ws_watcher(daemon.clone(), session_id.clone(), &ws_state);

    Arc::new(DaemonTerminalRuntime {
        daemon,
        ws_state,
        last_synced_ws_generation: std::sync::atomic::AtomicU64::new(0),
        kind: TerminalRuntimeKind::Local,
        resize_error_label: "failed to resize terminal",
        snapshot_error_label: "daemon snapshot",
        exit_labels: Some(RuntimeExitLabels {
            completed_title: "Terminal completed",
            failed_title: "Terminal failed",
            failed_notice_prefix: "terminal tab",
        }),
        clear_global_daemon_on_connection_refused: true,
    })
}

#[cfg(feature = "ssh")]
pub(crate) fn outpost_ssh_runtime(ssh: SshTerminalShell) -> SharedTerminalRuntime {
    Arc::new(EmulatorTerminalRuntime {
        backend: ssh,
        kind: TerminalRuntimeKind::Outpost,
        resize_error_label: "failed to resize SSH terminal",
        exit_labels: RuntimeExitLabels {
            completed_title: "SSH terminal completed",
            failed_title: "SSH terminal failed",
            failed_notice_prefix: "SSH terminal tab",
        },
    })
}

#[cfg(feature = "mosh")]
pub(crate) fn outpost_mosh_runtime(mosh: arbor_mosh::MoshShell) -> SharedTerminalRuntime {
    Arc::new(EmulatorTerminalRuntime {
        backend: mosh,
        kind: TerminalRuntimeKind::Outpost,
        resize_error_label: "failed to resize mosh terminal",
        exit_labels: RuntimeExitLabels {
            completed_title: "Mosh terminal completed",
            failed_title: "Mosh terminal failed",
            failed_notice_prefix: "mosh terminal tab",
        },
    })
}

pub(crate) fn apply_terminal_emulator_snapshot(
    session: &mut TerminalSession,
    snapshot: arbor_terminal_emulator::TerminalSnapshot,
) -> bool {
    let mut changed = false;

    if session.output != snapshot.output
        || session.styled_output != snapshot.styled_lines
        || session.cursor != snapshot.cursor
        || session.modes != snapshot.modes
    {
        session.output = snapshot.output;
        session.styled_output = snapshot.styled_lines;
        session.cursor = snapshot.cursor;
        session.modes = snapshot.modes;
        session.updated_at_unix_ms = current_unix_timestamp_millis();
        changed = true;
    }

    if session.exit_code != snapshot.exit_code {
        session.exit_code = snapshot.exit_code;
        session.updated_at_unix_ms = current_unix_timestamp_millis();
        changed = true;
    }

    changed
}

pub(crate) fn track_terminal_command_keystroke(
    session: &mut TerminalSession,
    keystroke: &Keystroke,
) {
    if keystroke.modifiers.platform {
        return;
    }

    if keystroke.modifiers.control {
        if keystroke.key.eq_ignore_ascii_case("u") {
            session.pending_command.clear();
        }
        return;
    }

    if keystroke.modifiers.alt {
        return;
    }

    match keystroke.key.as_str() {
        "enter" | "return" if keystroke.modifiers.shift => {
            session.pending_command.push('\n');
        },
        "enter" | "return" => {
            let command = session.pending_command.trim();
            if !command.is_empty() {
                session.last_command = Some(command.to_owned());
            }
            session.pending_command.clear();
        },
        "backspace" => {
            session.pending_command.pop();
        },
        "tab" => session.pending_command.push('\t'),
        "space" => session.pending_command.push(' '),
        _ => {
            if let Some(key_char) = keystroke.key_char.as_ref() {
                session.pending_command.push_str(key_char);
            } else if keystroke.key.len() == 1 {
                session.pending_command.push_str(&keystroke.key);
            }
        },
    }
}

pub(crate) fn daemon_terminal_sync_interval(
    is_active: bool,
    session_state: TerminalState,
) -> Duration {
    if is_active {
        return ACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL;
    }

    match session_state {
        TerminalState::Running => INACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL,
        TerminalState::Completed | TerminalState::Failed => IDLE_DAEMON_TERMINAL_SYNC_INTERVAL,
    }
}

pub(crate) fn runtime_sync_interval_elapsed(
    last_runtime_sync_at: Option<Instant>,
    sync_interval: Duration,
    now: Instant,
) -> bool {
    if sync_interval == Duration::ZERO {
        return true;
    }

    match last_runtime_sync_at {
        Some(last_sync) => now.saturating_duration_since(last_sync) >= sync_interval,
        None => true,
    }
}

pub(crate) fn daemon_websocket_request(
    connect_config: &terminal_daemon_http::WebsocketConnectConfig,
) -> Result<tungstenite::http::Request<()>, String> {
    use tungstenite::client::IntoClientRequest;

    let mut request = connect_config
        .url
        .as_str()
        .into_client_request()
        .map_err(|error| format!("failed to create websocket request: {error}"))?;

    if let Some(token) = connect_config.auth_token.as_ref() {
        let header_value = tungstenite::http::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|error| format!("failed to encode websocket auth token: {error}"))?;
        request
            .headers_mut()
            .insert(tungstenite::http::header::AUTHORIZATION, header_value);
    }

    Ok(request)
}

/// Set `TCP_NODELAY` and a short read timeout on the WebSocket's underlying TCP stream
/// so the read loop can periodically check the write channel without blocking forever.
pub(crate) fn configure_ws_socket_for_low_latency(
    socket: &tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
) {
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = socket.get_ref() {
        let _ = tcp.set_nodelay(true);
        let _ = tcp.set_read_timeout(Some(Duration::from_millis(5)));
    }
}

pub(crate) fn spawn_daemon_terminal_ws_watcher(
    daemon: terminal_daemon_http::SharedTerminalDaemonClient,
    session_id: String,
    ws_state: &Arc<DaemonTerminalWsState>,
) {
    let ws_state = Arc::downgrade(ws_state);
    std::thread::spawn(move || {
        let mut reconnect_delay = DAEMON_TERMINAL_WS_RECONNECT_BASE_DELAY;

        loop {
            let Some(ws_state) = ws_state.upgrade() else {
                break;
            };
            if ws_state.is_closed() {
                break;
            }

            let connect_config = match daemon.terminal_websocket_config(&session_id) {
                Ok(config) => config,
                Err(error) => {
                    tracing::debug!(
                        session_id = %session_id,
                        %error,
                        "failed to build daemon terminal websocket config"
                    );
                    std::thread::sleep(reconnect_delay);
                    reconnect_delay = daemon_terminal_ws_next_backoff(reconnect_delay);
                    continue;
                },
            };
            let request = match daemon_websocket_request(&connect_config) {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!(
                        session_id = %session_id,
                        %error,
                        "failed to create daemon terminal websocket request"
                    );
                    std::thread::sleep(reconnect_delay);
                    reconnect_delay = daemon_terminal_ws_next_backoff(reconnect_delay);
                    continue;
                },
            };

            match tungstenite::connect(request) {
                Ok((mut socket, _)) => {
                    configure_ws_socket_for_low_latency(&socket);
                    ws_state.note_event();
                    reconnect_delay = DAEMON_TERMINAL_WS_RECONNECT_BASE_DELAY;

                    // Set up write channel for low-latency keystroke delivery
                    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
                    ws_state.set_writer(Some(tx));
                    tracing::debug!(session_id = %session_id, "WS write channel established for keystroke delivery");

                    loop {
                        if ws_state.is_closed() {
                            ws_state.set_writer(None);
                            let _ = socket.close(None);
                            return;
                        }

                        // Drain outgoing keystrokes and send as binary frames
                        while let Ok(bytes) = rx.try_recv() {
                            if socket
                                .send(tungstenite::Message::Binary(bytes.into()))
                                .is_err()
                            {
                                ws_state.set_writer(None);
                                break;
                            }
                        }

                        // Read incoming messages (may timeout quickly due to read timeout)
                        match socket.read() {
                            Ok(tungstenite::Message::Binary(_))
                            | Ok(tungstenite::Message::Text(_)) => {
                                ws_state.note_event();
                            },
                            Ok(tungstenite::Message::Ping(_))
                            | Ok(tungstenite::Message::Pong(_))
                            | Ok(tungstenite::Message::Frame(_)) => {},
                            Ok(tungstenite::Message::Close(_)) => {
                                ws_state.set_writer(None);
                                break;
                            },
                            Err(tungstenite::Error::Io(ref e))
                                if e.kind() == std::io::ErrorKind::WouldBlock
                                    || e.kind() == std::io::ErrorKind::TimedOut =>
                            {
                                // Read timeout — expected, loop back to check writes
                            },
                            Err(error) => {
                                tracing::debug!(
                                    session_id = %session_id,
                                    %error,
                                    "daemon terminal websocket disconnected"
                                );
                                ws_state.set_writer(None);
                                break;
                            },
                        }
                    }
                },
                Err(error) => {
                    tracing::debug!(
                        session_id = %session_id,
                        %error,
                        "failed to connect daemon terminal websocket"
                    );
                },
            }

            if ws_state.is_closed() {
                break;
            }

            std::thread::sleep(reconnect_delay);
            reconnect_delay = daemon_terminal_ws_next_backoff(reconnect_delay);
        }
    });
}

pub(crate) fn daemon_terminal_ws_next_backoff(current: Duration) -> Duration {
    current
        .checked_mul(2)
        .unwrap_or(DAEMON_TERMINAL_WS_RECONNECT_MAX_DELAY)
        .min(DAEMON_TERMINAL_WS_RECONNECT_MAX_DELAY)
}

pub(crate) fn ordered_terminal_sync_indices(
    terminals: &[TerminalSession],
    active_terminal_id: Option<u64>,
) -> Vec<usize> {
    let mut indices = (0..terminals.len()).collect::<Vec<_>>();
    indices.sort_by_key(|&index| active_terminal_id != Some(terminals[index].id));
    indices
}

pub(crate) fn daemon_state_from_terminal_state(state: TerminalState) -> TerminalSessionState {
    match state {
        TerminalState::Running => TerminalSessionState::Running,
        TerminalState::Completed => TerminalSessionState::Completed,
        TerminalState::Failed => TerminalSessionState::Failed,
    }
}

pub(crate) fn emulate_raw_output(
    raw: &str,
    rows: u16,
    cols: u16,
) -> (
    Vec<TerminalStyledLine>,
    Option<TerminalCursor>,
    TerminalModes,
) {
    let mut emulator = arbor_terminal_emulator::TerminalEmulator::with_size(rows, cols);
    emulator.process(raw.as_bytes());
    (
        emulator.collect_styled_lines(),
        emulator.snapshot_cursor(),
        emulator.snapshot_modes(),
    )
}

pub(crate) fn daemon_cursor_to_terminal_cursor(
    cursor: daemon::DaemonTerminalCursor,
) -> TerminalCursor {
    TerminalCursor {
        line: cursor.line,
        column: cursor.column,
    }
}

pub(crate) fn daemon_modes_to_terminal_modes(modes: daemon::DaemonTerminalModes) -> TerminalModes {
    TerminalModes {
        app_cursor: modes.app_cursor,
        alt_screen: modes.alt_screen,
    }
}

pub(crate) fn daemon_styled_line_to_terminal_line(
    line: daemon::DaemonTerminalStyledLine,
) -> TerminalStyledLine {
    TerminalStyledLine {
        cells: line
            .cells
            .into_iter()
            .map(|cell| TerminalStyledCell {
                column: cell.column,
                text: cell.text,
                fg: cell.fg,
                bg: cell.bg,
            })
            .collect(),
        runs: line
            .runs
            .into_iter()
            .map(|run| TerminalStyledRun {
                text: run.text,
                fg: run.fg,
                bg: run.bg,
            })
            .collect(),
    }
}

pub(crate) fn apply_daemon_snapshot(
    session: &mut TerminalSession,
    snapshot: &daemon::TerminalSnapshot,
) -> bool {
    let mut changed = false;

    if session.output != snapshot.output_tail {
        session.output = snapshot.output_tail.clone();
        changed = true;
    }

    let (styled_output, cursor, modes) = if snapshot.styled_lines.is_empty() {
        emulate_raw_output(&snapshot.output_tail, session.rows, session.cols)
    } else {
        (
            snapshot
                .styled_lines
                .iter()
                .cloned()
                .map(daemon_styled_line_to_terminal_line)
                .collect(),
            snapshot.cursor.map(daemon_cursor_to_terminal_cursor),
            daemon_modes_to_terminal_modes(snapshot.modes),
        )
    };

    if session.styled_output != styled_output || session.cursor != cursor || session.modes != modes
    {
        session.styled_output = styled_output;
        session.cursor = cursor;
        session.modes = modes;
        changed = true;
    }

    if changed {
        session.updated_at_unix_ms = current_unix_timestamp_millis();
    }

    changed
}

pub(crate) fn terminal_state_from_daemon_record(record: &DaemonSessionRecord) -> TerminalState {
    if let Some(state) = record.state {
        return terminal_state_from_daemon_state(state);
    }

    match record.exit_code {
        Some(0) => TerminalState::Completed,
        Some(_) => TerminalState::Failed,
        None => TerminalState::Running,
    }
}

pub(crate) fn terminal_output_tail_for_metadata(
    session: &TerminalSession,
    max_lines: usize,
    max_chars: usize,
) -> String {
    let lines = terminal_display_lines(session);
    if lines.is_empty() {
        return String::new();
    }

    let start = lines.len().saturating_sub(max_lines);
    let mut tail = lines
        .into_iter()
        .skip(start)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();

    let char_count = tail.chars().count();
    if char_count > max_chars {
        let skip = char_count.saturating_sub(max_chars);
        tail = tail.chars().skip(skip).collect::<String>();
    }

    tail
}

pub(crate) fn current_unix_timestamp_millis() -> Option<u64> {
    daemon::current_unix_timestamp_millis()
}

pub(crate) fn daemon_base_url_from_config(raw: Option<&str>) -> String {
    if let Ok(env_url) = env::var("ARBOR_DAEMON_URL") {
        let trimmed = env_url.trim().to_owned();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DAEMON_BASE_URL)
        .to_owned()
}

pub(crate) fn parse_connect_host_target(raw: &str) -> Result<ConnectHostTarget, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("Address cannot be empty".to_owned());
    }

    if value.starts_with("ssh://") {
        let target = parse_ssh_daemon_target(value)?;
        let auth_key = format_ssh_auth_key(&target);
        return Ok(ConnectHostTarget::Ssh { target, auth_key });
    }

    if value.starts_with("https://") {
        return Err(
            "https:// is not supported by arbor-httpd; use http://HOST:PORT or ssh://HOST/"
                .to_owned(),
        );
    }

    if value.starts_with("http://") {
        return Ok(ConnectHostTarget::Http {
            url: value.to_owned(),
            auth_key: value.to_owned(),
        });
    }

    if value.contains("://") {
        return Err(
            "unsupported scheme; use http://HOST:PORT or ssh://[user@]HOST[:ssh_port]/".to_owned(),
        );
    }

    let normalized = if value.contains(':') {
        format!("http://{value}")
    } else {
        format!("http://{value}:{DEFAULT_DAEMON_PORT}")
    };

    Ok(ConnectHostTarget::Http {
        url: normalized.clone(),
        auth_key: normalized,
    })
}

pub(crate) fn parse_ssh_daemon_target(raw: &str) -> Result<SshDaemonTarget, String> {
    let Some(without_scheme) = raw.trim().strip_prefix("ssh://") else {
        return Err("ssh address must start with ssh://".to_owned());
    };
    if without_scheme.is_empty() {
        return Err("ssh address is missing a host".to_owned());
    }

    let (authority, path_tail) = match without_scheme.split_once('/') {
        Some((authority, tail)) => (authority, tail),
        None => (without_scheme, ""),
    };

    if authority.trim().is_empty() {
        return Err("ssh address is missing a host".to_owned());
    }

    let (user, host, ssh_port) = parse_ssh_authority(authority)?;
    let daemon_port = parse_ssh_daemon_port(path_tail)?;

    Ok(SshDaemonTarget {
        user,
        host,
        ssh_port,
        daemon_port,
    })
}

pub(crate) fn parse_ssh_authority(
    authority: &str,
) -> Result<(Option<String>, String, u16), String> {
    let (user, host_port) = match authority.rsplit_once('@') {
        Some((candidate_user, host_port))
            if !candidate_user.trim().is_empty() && !host_port.trim().is_empty() =>
        {
            (Some(candidate_user.trim().to_owned()), host_port.trim())
        },
        Some(_) => return Err("invalid ssh address: malformed user@host section".to_owned()),
        None => (None, authority.trim()),
    };

    let (host, port) = parse_host_and_optional_port(host_port, DEFAULT_SSH_PORT)?;
    Ok((user, host, port))
}

pub(crate) fn parse_ssh_daemon_port(path_tail: &str) -> Result<u16, String> {
    let trimmed = path_tail.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(DEFAULT_DAEMON_PORT);
    }
    if trimmed.contains('/') || trimmed.contains('?') || trimmed.contains('#') {
        return Err(
            "invalid ssh address path: only an optional daemon port is allowed, for example ssh://host/8787"
                .to_owned(),
        );
    }

    trimmed
        .parse::<u16>()
        .map_err(|error| format!("invalid daemon port `{trimmed}`: {error}"))
}

pub(crate) fn parse_host_and_optional_port(
    value: &str,
    default_port: u16,
) -> Result<(String, u16), String> {
    if let Some(rest) = value.strip_prefix('[') {
        let Some((host, suffix)) = rest.split_once(']') else {
            return Err("invalid host: missing closing `]` for IPv6 address".to_owned());
        };
        if host.trim().is_empty() {
            return Err("host is empty".to_owned());
        }
        if suffix.is_empty() {
            return Ok((host.to_owned(), default_port));
        }
        let Some(port_text) = suffix.strip_prefix(':') else {
            return Err("invalid host: unexpected characters after IPv6 address".to_owned());
        };
        let port = port_text
            .parse::<u16>()
            .map_err(|error| format!("invalid port `{port_text}`: {error}"))?;
        return Ok((host.to_owned(), port));
    }

    let Some((host, port_text)) = value.rsplit_once(':') else {
        return Ok((value.to_owned(), default_port));
    };

    if host.contains(':') {
        return Err("IPv6 hosts must be wrapped in brackets, for example [::1]".to_owned());
    }
    if host.trim().is_empty() {
        return Err("host is empty".to_owned());
    }
    let port = port_text
        .parse::<u16>()
        .map_err(|error| format!("invalid port `{port_text}`: {error}"))?;
    Ok((host.to_owned(), port))
}

pub(crate) fn format_ssh_auth_key(target: &SshDaemonTarget) -> String {
    let host = if target.host.contains(':') {
        format!("[{}]", target.host)
    } else {
        target.host.clone()
    };
    let authority = match target.user.as_deref() {
        Some(user) if !user.trim().is_empty() => {
            format!("{user}@{host}:{}", target.ssh_port)
        },
        _ => format!("{host}:{}", target.ssh_port),
    };

    format!("ssh://{authority}/{}", target.daemon_port)
}

pub(crate) fn reserve_local_loopback_port() -> Result<u16, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("failed to reserve local port: {error}"))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|error| format!("failed to resolve local tunnel port: {error}"))
}

pub(crate) fn paths_equivalent(left: &Path, right: &Path) -> bool {
    worktree::paths_equivalent(left, right)
}

pub(crate) fn porcelain_status_to_change_kind(xy: &str) -> ChangeKind {
    let bytes = xy.as_bytes();
    let x = bytes.first().copied().unwrap_or(b' ');
    let y = bytes.get(1).copied().unwrap_or(b' ');

    match (x, y) {
        (b'?', b'?') => ChangeKind::Added,
        (b'A', _) | (_, b'A') => ChangeKind::Added,
        (b'D', _) | (_, b'D') => ChangeKind::Removed,
        (b'R', _) | (_, b'R') => ChangeKind::Renamed,
        (b'C', _) | (_, b'C') => ChangeKind::Copied,
        (b'T', _) | (_, b'T') => ChangeKind::TypeChange,
        (b'U', _) | (_, b'U') => ChangeKind::Conflict,
        (b'M', _) | (_, b'M') => ChangeKind::Modified,
        _ => ChangeKind::Modified,
    }
}

pub(crate) fn parse_remote_numstat_output(output: &str) -> HashMap<PathBuf, (usize, usize)> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let mut columns = line.split('\t');
        let Some(added) = columns.next() else {
            continue;
        };
        let Some(removed) = columns.next() else {
            continue;
        };
        let Some(path_str) = columns.next() else {
            continue;
        };
        let additions = added.parse::<usize>().unwrap_or(0);
        let deletions = removed.parse::<usize>().unwrap_or(0);
        if additions > 0 || deletions > 0 {
            map.insert(PathBuf::from(path_str), (additions, deletions));
        }
    }
    map
}

pub(crate) fn daemon_error_is_connection_refused(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("connection refused")
        || lower.contains("failed to connect")
        || lower.contains("actively refused")
}

pub(crate) fn daemon_url_is_local(url: &str) -> bool {
    let authority = url
        .strip_prefix("http://")
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("");
    let host = authority
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(authority);
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// If the running daemon's version differs from the GUI, shut it down and
/// restart a fresh one. Returns `Some((records, new_daemon))` when a restart
/// happened, or `None` when versions match (caller keeps the original daemon).
pub(crate) fn check_daemon_version_and_restart(
    daemon: &terminal_daemon_http::SharedTerminalDaemonClient,
    daemon_base_url: &str,
) -> Option<(
    Vec<DaemonSessionRecord>,
    Option<terminal_daemon_http::SharedTerminalDaemonClient>,
)> {
    let health = match daemon.health() {
        Ok(h) => h,
        Err(error) => {
            tracing::warn!(%error, "failed to query daemon health, skipping version check");
            return None;
        },
    };

    if health.version == APP_VERSION {
        tracing::debug!(version = APP_VERSION, "daemon version matches");
        return None;
    }

    tracing::warn!(
        daemon_version = %health.version,
        gui_version = APP_VERSION,
        "daemon version mismatch, restarting"
    );

    if let Err(error) = daemon.shutdown() {
        tracing::warn!(%error, "failed to request daemon shutdown");
    }

    // Give the old process a moment to exit.
    std::thread::sleep(Duration::from_millis(500));

    match try_auto_start_daemon(daemon_base_url) {
        Some(new_daemon) => {
            let records = new_daemon.list_sessions().unwrap_or_default();
            Some((records, Some(new_daemon)))
        },
        None => {
            tracing::warn!("failed to restart daemon after version mismatch");
            Some((Vec::new(), None))
        },
    }
}

/// Attempt to locate and spawn `arbor-httpd` as a detached background process,
/// then poll until it becomes reachable. Returns `Some(daemon)` on success.
/// Only works for localhost URLs — auto-starting a remote daemon makes no sense.
pub(crate) fn try_auto_start_daemon(
    daemon_base_url: &str,
) -> Option<terminal_daemon_http::SharedTerminalDaemonClient> {
    if !is_localhost_url(daemon_base_url) {
        tracing::debug!(url = %daemon_base_url, "skipping auto-start for non-localhost daemon");
        return None;
    }
    let binary = find_arbor_httpd_binary()?;
    tracing::info!(path = %binary.display(), "auto-starting arbor-httpd");

    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)?;
    let log_dir = home.join(".arbor").join("daemon");
    if let Err(error) = fs::create_dir_all(&log_dir) {
        tracing::warn!(%error, "failed to create daemon log directory");
    }
    let log_file = log_dir.join("daemon.log");

    let log_out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file);
    let (stdout_file, stderr_file) = match log_out {
        Ok(file) => {
            let dup = file.try_clone().ok()?;
            (Stdio::from(file), Stdio::from(dup))
        },
        Err(error) => {
            tracing::warn!(%error, path = %log_file.display(), "cannot open daemon log file");
            (Stdio::null(), Stdio::null())
        },
    };

    // Let arbor-httpd choose its default bind host based on whether auth is
    // enabled, while still honoring the requested port.
    let port = daemon_base_url
        .strip_prefix("http://")
        .and_then(|s| s.rsplit_once(':'))
        .and_then(|(_, p)| p.parse::<u16>().ok())
        .unwrap_or(8787);

    let mut cmd = Command::new(&binary);
    if let Some(path) = AUGMENTED_PATH.get() {
        cmd.env("PATH", path);
    }
    cmd.env("ARBOR_HTTPD_PORT", port.to_string())
        .stdin(Stdio::null())
        .stdout(stdout_file)
        .stderr(stderr_file);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    if let Err(error) = cmd.spawn() {
        tracing::warn!(%error, path = %binary.display(), "failed to spawn arbor-httpd");
        return None;
    }

    let daemon = terminal_daemon_http::default_terminal_daemon_client(daemon_base_url).ok()?;

    const MAX_ATTEMPTS: u32 = 20;
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    for attempt in 1..=MAX_ATTEMPTS {
        std::thread::sleep(POLL_INTERVAL);
        match daemon.list_sessions() {
            Ok(_) => {
                tracing::info!(attempt, "daemon is ready");
                return Some(daemon);
            },
            Err(_) if attempt < MAX_ATTEMPTS => continue,
            Err(error) => {
                tracing::warn!(%error, "daemon did not become ready after auto-start");
            },
        }
    }
    None
}

/// Search for the `arbor-httpd` binary next to the current executable,
/// then fall back to `PATH` lookup.
pub(crate) fn find_arbor_httpd_binary() -> Option<PathBuf> {
    let binary_name = if cfg!(windows) {
        "arbor-httpd.exe"
    } else {
        "arbor-httpd"
    };

    if let Ok(exe) = env::current_exe() {
        let sibling = exe.with_file_name(binary_name);
        if sibling.is_file() {
            return Some(sibling);
        }
    }

    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|dir| dir.join(binary_name))
            .find(|candidate| candidate.is_file())
    })
}

pub(crate) fn is_localhost_url(url: &str) -> bool {
    let host = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let host = host.split(':').next().unwrap_or(host);
    host == "127.0.0.1" || host == "localhost" || host == "[::1]"
}

pub(crate) fn load_outpost_summaries(
    store: &dyn arbor_core::outpost_store::OutpostStore,
    remote_hosts: &[arbor_core::outpost::RemoteHost],
) -> Vec<OutpostSummary> {
    let records = match store.load() {
        Ok(records) => records,
        Err(_) => return Vec::new(),
    };

    records
        .into_iter()
        .map(|record| {
            let hostname = remote_hosts
                .iter()
                .find(|host| host.name == record.host_name)
                .map(|host| host.hostname.clone())
                .unwrap_or_else(|| record.host_name.clone());

            OutpostSummary {
                outpost_id: record.id,
                repo_root: PathBuf::from(&record.local_repo_root),
                remote_path: record.remote_path,
                label: record.label,
                branch: record.branch,
                host_name: record.host_name,
                hostname,
                status: arbor_core::outpost::OutpostStatus::default(),
            }
        })
        .collect()
}

impl Drop for ArborWindow {
    fn drop(&mut self) {
        self.stop_active_ssh_daemon_tunnel();
        remove_claude_code_hooks();
        remove_pi_agent_extension();
    }
}

impl WorktreeSummary {
    pub(crate) fn from_worktree(
        entry: &worktree::Worktree,
        repo_root: &Path,
        group_key: &str,
        checkout_kind: CheckoutKind,
    ) -> Self {
        let label = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| entry.path.display().to_string());

        let branch = entry
            .branch
            .as_deref()
            .map(short_branch)
            .unwrap_or_else(|| "-".to_owned());
        let is_primary_checkout = entry.path.as_path() == repo_root;

        let last_activity_unix_ms = worktree::last_git_activity_ms(&entry.path);

        Self {
            group_key: group_key.to_owned(),
            checkout_kind,
            repo_root: repo_root.to_path_buf(),
            path: entry.path.clone(),
            label,
            branch,
            is_primary_checkout,
            pr_number: None,
            pr_url: None,
            pr_details: None,
            diff_summary: None,
            agent_state: None,
            agent_task: None,
            last_activity_unix_ms,
        }
    }
}

impl RepositorySummary {
    pub(crate) fn from_checkout_roots(
        root: PathBuf,
        group_key: String,
        checkout_roots: Vec<repository_store::RepositoryCheckoutRoot>,
    ) -> Self {
        let label = repository_display_name(&root);
        let github_repo_slug = github_repo_slug_for_repo(&root);
        let avatar_url = github_repo_slug
            .as_ref()
            .and_then(|repo_slug| github_avatar_url_for_repo_slug(repo_slug));

        Self {
            group_key,
            root,
            checkout_roots,
            label,
            avatar_url,
            github_repo_slug,
        }
    }

    pub(crate) fn contains_checkout_root(&self, root: &Path) -> bool {
        self.checkout_roots
            .iter()
            .any(|checkout_root| checkout_root.path == root)
    }
}

impl EntityInputHandler for ArborWindow {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.ime_marked_text.as_ref().map(|text| {
            let len: usize = text.encode_utf16().count();
            0..len
        })
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime_marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = None;
        if text.is_empty() {
            cx.notify();
            return;
        }
        // Suppress all text input while the quit overlay is showing.
        if self.quit_overlay_until.is_some() {
            return;
        }
        // When a modal with a text field is open, route IME text there instead
        if let Some(ref mut modal) = self.daemon_auth_modal {
            modal.token.push_str(text);
            modal.error = None;
            cx.notify();
            return;
        }
        if let Some(ref mut modal) = self.connect_to_host_modal {
            modal.address.push_str(text);
            modal.error = None;
            cx.notify();
            return;
        }
        if self.welcome_clone_url_active {
            self.welcome_clone_url.push_str(text);
            self.welcome_clone_error = None;
            cx.notify();
            return;
        }
        if self.welcome_local_path_active {
            self.welcome_local_path.push_str(text);
            self.welcome_local_path_error = None;
            cx.notify();
            return;
        }
        let Some(session_id) = self.active_terminal_id_for_selected_worktree() else {
            return;
        };
        self.append_pasted_text_to_pending_command(session_id, text);
        if let Err(error) = self.write_input_to_terminal(session_id, text.as_bytes()) {
            self.notice = Some(format!("failed to write to terminal: {error}"));
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked_text = if new_text.is_empty() {
            None
        } else {
            Some(new_text.to_owned())
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for ArborWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Update window title to reflect connected daemon
        let title = match &self.connected_daemon_label {
            Some(label) => format!("Arbor \u{2014} {label}"),
            None => "Arbor".to_owned(),
        };
        window.set_window_title(&title);

        self.window_is_active = window.is_window_active();
        if self.focus_terminal_on_next_render && self.active_terminal().is_some() {
            window.focus(&self.terminal_focus);
            self.focus_terminal_on_next_render = false;
        }
        let workspace_width = f32::from(window.window_bounds().get_bounds().size.width);
        self.clamp_pane_widths_for_workspace(workspace_width);
        self.sync_ui_state_store(window);

        let theme = self.theme();
        div()
            .size_full()
            .bg(rgb(theme.app_bg))
            .text_color(rgb(theme.text_primary))
            .font_family(FONT_UI)
            .relative()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(Self::handle_global_key_down))
            .on_action(cx.listener(Self::action_spawn_terminal))
            .on_action(cx.listener(Self::action_close_active_terminal))
            .on_action(cx.listener(Self::action_open_manage_presets))
            .on_action(cx.listener(Self::action_open_manage_repo_presets))
            .on_action(cx.listener(Self::action_refresh_worktrees))
            .on_action(cx.listener(Self::action_refresh_changes))
            .on_action(cx.listener(Self::action_open_add_repository))
            .on_action(cx.listener(Self::action_open_create_worktree))
            .on_action(cx.listener(Self::action_use_embedded_backend))
            .on_action(cx.listener(Self::action_use_alacritty_backend))
            .on_action(cx.listener(Self::action_use_ghostty_backend))
            .on_action(cx.listener(Self::action_toggle_left_pane))
            .on_action(cx.listener(Self::action_navigate_worktree_back))
            .on_action(cx.listener(Self::action_navigate_worktree_forward))
            .on_action(cx.listener(Self::action_collapse_all_repositories))
            .on_action(cx.listener(Self::action_view_logs))
            .on_action(cx.listener(Self::action_show_about))
            .on_action(cx.listener(Self::action_open_theme_picker))
            .on_action(cx.listener(Self::action_open_settings))
            .on_action(cx.listener(Self::action_open_manage_hosts))
            .on_action(cx.listener(Self::action_connect_to_lan_daemon))
            .on_action(cx.listener(Self::action_connect_to_host))
            .on_action(cx.listener(Self::action_request_quit))
            .on_action(cx.listener(Self::action_immediate_quit))
            .child(self.render_top_bar(cx))
            .child(div().h(px(1.)).bg(rgb(theme.chrome_border)))
            .when(self.repositories.is_empty(), |this| {
                this.child(self.render_welcome_pane(cx))
            })
            .when(!self.repositories.is_empty(), |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .on_drag_move(cx.listener(Self::handle_pane_divider_drag_move))
                        .child(self.render_left_pane(cx))
                        .when(self.left_pane_visible, |this| {
                            this.child(self.render_pane_resize_handle(
                                "left-pane-resize",
                                DraggedPaneDivider::Left,
                                theme,
                            ))
                        })
                        .child(self.render_center_pane(window, cx))
                        .child(self.render_pane_resize_handle(
                            "right-pane-resize",
                            DraggedPaneDivider::Right,
                            theme,
                        ))
                        .child(self.render_right_pane(cx)),
                )
            })
            .child(self.render_status_bar())
            .child(self.render_top_bar_worktree_quick_actions_menu(cx))
            .child(self.render_notice_toast(cx))
            .child(self.render_create_modal(cx))
            .child(self.render_github_auth_modal(cx))
            .child(self.render_repository_context_menu(cx))
            .child(self.render_worktree_context_menu(cx))
            .child(self.render_worktree_hover_popover(cx))
            .child(self.render_outpost_context_menu(cx))
            .child(self.render_delete_modal(cx))
            .child(self.render_manage_hosts_modal(cx))
            .child(self.render_manage_presets_modal(cx))
            .child(self.render_manage_repo_presets_modal(cx))
            .child(self.render_about_modal(cx))
            .child(self.render_theme_picker_modal(cx))
            .child(self.render_settings_modal(cx))
            .child(self.render_daemon_auth_modal(cx))
            .child(self.render_start_daemon_modal(cx))
            .child(self.render_connect_to_host_modal(cx))
            .child(div().when_some(self.theme_toast.clone(), |this, toast| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_end()
                        .justify_end()
                        .px_3()
                        .pb(px(34.))
                        .child(
                            div()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(theme.accent))
                                .bg(rgb(theme.panel_active_bg))
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(rgb(theme.text_primary))
                                .child(toast),
                        ),
                )
            }))
            .when(self.quit_overlay_until.is_some(), |this| {
                this.child(
                    div()
                        .id("quit-backdrop")
                        .absolute()
                        .inset_0()
                        .bg(rgb(0x000000))
                        .opacity(0.5)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.action_dismiss_quit(window, cx);
                        })),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .child(
                            div()
                                .px_6()
                                .py_4()
                                .rounded_lg()
                                .bg(rgb(theme.chrome_bg))
                                .border_1()
                                .border_color(rgb(theme.border))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(theme.text_primary))
                                        .child("Are you sure you want to quit Arbor?"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            action_button(
                                                theme,
                                                "quit-cancel",
                                                "Cancel",
                                                ActionButtonStyle::Secondary,
                                                true,
                                            )
                                            .min_w(px(64.))
                                            .flex()
                                            .justify_center()
                                            .on_click(
                                                cx.listener(|this, _, window, cx| {
                                                    this.action_dismiss_quit(window, cx);
                                                }),
                                            ),
                                        )
                                        .child(
                                            div()
                                                .id("quit-confirm")
                                                .cursor_pointer()
                                                .rounded_sm()
                                                .border_1()
                                                .border_color(rgb(0xc94040))
                                                .bg(rgb(0xc94040))
                                                .min_w(px(64.))
                                                .flex()
                                                .justify_center()
                                                .px_2()
                                                .py_1()
                                                .text_xs()
                                                .text_color(rgb(0xffffff))
                                                .child("Quit")
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.action_confirm_quit(window, cx);
                                                })),
                                        ),
                                ),
                        ),
                )
            })
    }
}

pub(crate) fn process_agent_ws_message(
    this: &gpui::WeakEntity<ArborWindow>,
    cx: &mut gpui::AsyncApp,
    text: &str,
) {
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(text);
    let Ok(value) = parsed else {
        tracing::warn!(raw = text, "agent WS: failed to parse message");
        return;
    };

    let msg_type = value.get("type").and_then(|v| v.as_str());
    match msg_type {
        Some("snapshot") => {
            let sessions = value
                .get("sessions")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let entries: Vec<(String, AgentState, Option<u64>)> = sessions
                .iter()
                .filter_map(|s| {
                    let cwd = s.get("cwd")?.as_str()?;
                    let state_str = s.get("state")?.as_str()?;
                    let state = match state_str {
                        "working" => AgentState::Working,
                        "waiting" => AgentState::Waiting,
                        _ => return None,
                    };
                    let updated_at = s.get("updated_at_unix_ms").and_then(|v| v.as_u64());
                    Some((cwd.to_owned(), state, updated_at))
                })
                .collect();
            tracing::info!(count = entries.len(), "agent WS snapshot received");
            for (cwd, state, _) in &entries {
                tracing::info!(cwd = cwd.as_str(), ?state, "  snapshot entry");
            }
            let _ = this.update(cx, |this, cx| {
                apply_agent_ws_snapshot(this, &entries);
                cx.notify();
            });
        },
        Some("update") => {
            if let Some(session) = value.get("session") {
                let cwd = session.get("cwd").and_then(|v| v.as_str());
                let state_str = session.get("state").and_then(|v| v.as_str());
                if let (Some(cwd), Some(state_str)) = (cwd, state_str) {
                    tracing::info!(cwd, state = state_str, "agent WS update received");
                    let state = match state_str {
                        "working" => AgentState::Working,
                        "waiting" => AgentState::Waiting,
                        _ => return,
                    };
                    let updated_at = session.get("updated_at_unix_ms").and_then(|v| v.as_u64());
                    let entries = vec![(cwd.to_owned(), state, updated_at)];
                    let _ = this.update(cx, |this, cx| {
                        apply_agent_ws_update(this, &entries);
                        cx.notify();
                    });
                }
            }
        },
        _ => {},
    }
}

pub(crate) fn apply_agent_ws_snapshot(
    app: &mut ArborWindow,
    entries: &[(String, AgentState, Option<u64>)],
) {
    tracing::info!(
        count = entries.len(),
        "agent WS snapshot: resetting all worktree states"
    );
    for worktree in &mut app.worktrees {
        worktree.agent_state = None;
    }
    apply_agent_ws_update(app, entries);
}

pub(crate) fn apply_agent_ws_update(
    app: &mut ArborWindow,
    entries: &[(String, AgentState, Option<u64>)],
) {
    let worktree_paths: Vec<PathBuf> = app.worktrees.iter().map(|w| w.path.clone()).collect();

    for (cwd, state, updated_at) in entries {
        let cwd_path = Path::new(cwd);
        // Find the most specific (longest) worktree path that is a prefix of this cwd.
        let best_match = worktree_paths
            .iter()
            .filter(|wt_path| cwd_path.starts_with(wt_path))
            .max_by_key(|wt_path| wt_path.as_os_str().len());

        if let Some(matched_path) = best_match
            && let Some(worktree) = app.worktrees.iter_mut().find(|w| &w.path == matched_path)
        {
            tracing::info!(
                cwd = %cwd,
                worktree = %worktree.path.display(),
                ?state,
                "agent activity matched to worktree"
            );
            worktree.agent_state = Some(*state);
            if let Some(ts) = updated_at {
                worktree.last_activity_unix_ms =
                    Some(worktree.last_activity_unix_ms.unwrap_or(0).max(*ts));
            }
        } else {
            tracing::warn!(
                cwd = %cwd,
                ?state,
                "agent activity did not match any worktree"
            );
        }
    }
}

pub(crate) fn inject_daemon_log_entry(log_buffer: &log_layer::LogBuffer, text: &str) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let level = match value
        .get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("INFO")
    {
        "ERROR" => tracing::Level::ERROR,
        "WARN" => tracing::Level::WARN,
        "DEBUG" => tracing::Level::DEBUG,
        "TRACE" => tracing::Level::TRACE,
        _ => tracing::Level::INFO,
    };
    let target = value
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("arbor_httpd");
    let message = value
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let fields_str = value.get("fields").and_then(|v| v.as_str()).unwrap_or("");
    let ts_ms = value.get("ts").and_then(|v| v.as_u64()).unwrap_or(0);
    let timestamp = SystemTime::UNIX_EPOCH + Duration::from_millis(ts_ms);

    let mut fields = Vec::new();
    if !fields_str.is_empty() {
        for part in fields_str.split(' ') {
            if let Some((k, v)) = part.split_once('=') {
                fields.push((k.to_owned(), v.to_owned()));
            }
        }
    }

    log_buffer.push(log_layer::LogEntry {
        timestamp,
        level,
        target: format!("[daemon] {target}"),
        message,
        fields,
    });
}

pub(crate) fn install_claude_code_hooks(daemon_base_url: &str) -> Result<(), String> {
    let home = env::var("HOME").map_err(|_| "HOME not set".to_owned())?;
    let claude_dir = PathBuf::from(&home).join(".claude");
    let settings_path = claude_dir.join("settings.json");

    let mut settings: serde_json::Value = if settings_path.exists() {
        let content = fs::read_to_string(&settings_path)
            .map_err(|e| format!("failed to read settings.json: {e}"))?;
        serde_json::from_str(&content).map_err(|e| format!("failed to parse settings.json: {e}"))?
    } else {
        if !claude_dir.exists() {
            fs::create_dir_all(&claude_dir)
                .map_err(|e| format!("failed to create .claude dir: {e}"))?;
        }
        serde_json::json!({})
    };

    let notify_url = format!("{daemon_base_url}/api/v1/agent/notify");

    // Check if our hooks are already present
    if let Some(hooks) = settings.get("hooks") {
        let hooks_str = hooks.to_string();
        if hooks_str.contains("/api/v1/agent/notify") {
            tracing::debug!("Claude Code hooks already installed");
            return Ok(());
        }
    }

    let hook_entry = serde_json::json!([
        {
            "matcher": "",
            "hooks": [
                {
                    "type": "http",
                    "url": notify_url,
                    "timeout": 2
                }
            ]
        }
    ]);

    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let hooks_obj = hooks.as_object_mut().ok_or("hooks is not an object")?;

    if !hooks_obj.contains_key("UserPromptSubmit") {
        hooks_obj.insert("UserPromptSubmit".to_owned(), hook_entry.clone());
    }
    if !hooks_obj.contains_key("Stop") {
        hooks_obj.insert("Stop".to_owned(), hook_entry);
    }

    let serialized = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("failed to serialize settings: {e}"))?;
    fs::write(&settings_path, serialized)
        .map_err(|e| format!("failed to write settings.json: {e}"))?;

    tracing::info!(path = %settings_path.display(), "installed Claude Code hooks");
    Ok(())
}

const PI_AGENT_EXTENSION_FILENAME: &str = "arbor-activity.ts";
const PI_AGENT_EXTENSION_MARKER: &str = "Managed by Arbor: Pi activity bridge";

pub(crate) fn install_pi_agent_extension(daemon_base_url: &str) -> Result<(), String> {
    let home = env::var("HOME").map_err(|_| "HOME not set".to_owned())?;
    let extensions_dir = PathBuf::from(&home)
        .join(".pi")
        .join("agent")
        .join("extensions");
    fs::create_dir_all(&extensions_dir)
        .map_err(|e| format!("failed to create Pi extensions dir: {e}"))?;

    let extension_path = extensions_dir.join(PI_AGENT_EXTENSION_FILENAME);
    let next_content = render_pi_agent_extension(daemon_base_url);

    if extension_path.exists() {
        let existing = fs::read_to_string(&extension_path)
            .map_err(|e| format!("failed to read Pi extension: {e}"))?;
        if !existing.contains(PI_AGENT_EXTENSION_MARKER) {
            return Err(format!(
                "refusing to overwrite existing Pi extension `{}`",
                extension_path.display()
            ));
        }
        if existing == next_content {
            tracing::debug!("Pi activity extension already installed");
            return Ok(());
        }
    }

    fs::write(&extension_path, next_content)
        .map_err(|e| format!("failed to write Pi extension: {e}"))?;
    tracing::info!(path = %extension_path.display(), "installed Pi activity extension");
    Ok(())
}

pub(crate) fn render_pi_agent_extension(daemon_base_url: &str) -> String {
    let notify_url = format!("{daemon_base_url}/api/v1/agent/notify");
    format!(
        r#"// {PI_AGENT_EXTENSION_MARKER}
import type {{ ExtensionAPI }} from "@mariozechner/pi-coding-agent";

const NOTIFY_URL = {notify_url:?};

async function notify(hookEventName: "UserPromptSubmit" | "Stop", sessionId: string, cwd: string) {{
  try {{
    await fetch(NOTIFY_URL, {{
      method: "POST",
      headers: {{ "content-type": "application/json" }},
      body: JSON.stringify({{
        hook_event_name: hookEventName,
        session_id: sessionId,
        cwd,
      }}),
    }});
  }} catch {{
    // Ignore daemon reachability errors.
  }}
}}

export default function (pi: ExtensionAPI) {{
  pi.on("before_agent_start", async (_event, ctx) => {{
    await notify("UserPromptSubmit", ctx.sessionManager.getSessionId(), ctx.cwd);
  }});

  pi.on("agent_end", async (_event, ctx) => {{
    await notify("Stop", ctx.sessionManager.getSessionId(), ctx.cwd);
  }});
}}
"#
    )
}

pub(crate) fn remove_pi_agent_extension() {
    let Ok(home) = env::var("HOME") else {
        return;
    };
    let extension_path = PathBuf::from(&home)
        .join(".pi")
        .join("agent")
        .join("extensions")
        .join(PI_AGENT_EXTENSION_FILENAME);
    if !extension_path.exists() {
        return;
    }

    let Ok(content) = fs::read_to_string(&extension_path) else {
        return;
    };
    if !content.contains(PI_AGENT_EXTENSION_MARKER) {
        return;
    }

    match fs::remove_file(&extension_path) {
        Ok(()) => tracing::info!(path = %extension_path.display(), "removed Pi activity extension"),
        Err(error) => {
            tracing::warn!(path = %extension_path.display(), %error, "failed to remove Pi activity extension")
        },
    }
}

pub(crate) fn remove_claude_code_hooks() {
    let Ok(home) = env::var("HOME") else {
        return;
    };
    let settings_path = PathBuf::from(&home).join(".claude").join("settings.json");
    if !settings_path.exists() {
        return;
    }

    let Ok(content) = fs::read_to_string(&settings_path) else {
        return;
    };
    let Ok(mut settings) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };

    let Some(hooks) = settings.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return;
    };

    // Check if any hook references our notify endpoint
    if !hooks
        .values()
        .any(|v| v.to_string().contains("/api/v1/agent/notify"))
    {
        return;
    }

    // Remove entries containing our notify URL from each hook array
    let hook_keys: Vec<String> = hooks.keys().cloned().collect();
    for key in hook_keys {
        if let Some(arr) = hooks.get_mut(&key).and_then(|v| v.as_array_mut()) {
            arr.retain(|entry| !entry.to_string().contains("/api/v1/agent/notify"));
            if arr.is_empty() {
                hooks.remove(&key);
            }
        }
    }

    if hooks.is_empty()
        && let Some(obj) = settings.as_object_mut()
    {
        obj.remove("hooks");
    }

    match serde_json::to_string_pretty(&settings) {
        Ok(serialized) => {
            if let Err(e) = fs::write(&settings_path, serialized) {
                tracing::warn!(error = %e, "failed to write settings.json during hook removal");
            } else {
                tracing::info!(path = %settings_path.display(), "removed Claude Code hooks");
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize settings during hook removal");
        },
    }
}

pub(crate) fn worktree_rows_changed(
    previous: &[WorktreeSummary],
    next: &[WorktreeSummary],
) -> bool {
    if previous.len() != next.len() {
        return true;
    }

    previous.iter().zip(next.iter()).any(|(left, right)| {
        left.group_key != right.group_key
            || left.checkout_kind != right.checkout_kind
            || left.repo_root != right.repo_root
            || left.path != right.path
            || left.label != right.label
            || left.branch != right.branch
            || left.is_primary_checkout != right.is_primary_checkout
    })
}

pub(crate) fn estimated_worktree_hover_popover_card_height(
    worktree: &WorktreeSummary,
    checks_expanded: bool,
) -> Pixels {
    let mut height = 72.;

    if worktree
        .diff_summary
        .is_some_and(|summary| summary.additions > 0 || summary.deletions > 0)
    {
        height += 18.;
    }

    if worktree.agent_state.is_some() {
        height += 18.;
    }

    if let Some(pr) = worktree.pr_details.as_ref() {
        height += 110.;
        if checks_expanded
            && !pr.checks.is_empty()
            && matches!(
                pr.state,
                github_service::PrState::Open | github_service::PrState::Draft
            )
        {
            height += pr.checks.len() as f32 * 18.;
        }
    }

    px(height)
}

pub(crate) fn worktree_hover_popover_zone_bounds(
    left_pane_width: f32,
    popover: &WorktreeHoverPopover,
    worktree: &WorktreeSummary,
) -> Bounds<Pixels> {
    let padding = px(WORKTREE_HOVER_POPOVER_ZONE_PADDING_PX);
    Bounds::new(
        point(
            px(left_pane_width) + px(4.) - padding,
            popover.mouse_y - px(8.) - padding,
        ),
        size(
            px(WORKTREE_HOVER_POPOVER_CARD_WIDTH_PX) + padding * 2.,
            estimated_worktree_hover_popover_card_height(worktree, popover.checks_expanded)
                + padding * 2.,
        ),
    )
}

pub(crate) fn worktree_hover_trigger_zone_bounds(
    left_pane_width: f32,
    mouse_y: Pixels,
) -> Bounds<Pixels> {
    let height = px(WORKTREE_HOVER_TRIGGER_ZONE_HEIGHT_PX);
    Bounds::new(
        point(px(0.), mouse_y - height / 2.),
        size(px(left_pane_width), height),
    )
}

pub(crate) fn worktree_hover_safe_zone_contains(
    left_pane_width: f32,
    popover: &WorktreeHoverPopover,
    worktree: &WorktreeSummary,
    position: gpui::Point<Pixels>,
) -> bool {
    worktree_hover_popover_zone_bounds(left_pane_width, popover, worktree).contains(&position)
        || worktree_hover_trigger_zone_bounds(left_pane_width, popover.mouse_y).contains(&position)
}

pub(crate) fn format_relative_time(unix_ms: u64) -> String {
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let age_secs = now_ms.saturating_sub(unix_ms) / 1000;

    if age_secs < 60 {
        return "just now".to_owned();
    }
    let minutes = age_secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

pub(crate) fn terminal_tab_title(session: &TerminalSession) -> String {
    if let Some(last_command) = session
        .last_command
        .as_ref()
        .filter(|command| !command.trim().is_empty())
    {
        return truncate_with_ellipsis(last_command.trim(), TERMINAL_TAB_COMMAND_MAX_CHARS);
    }

    if !session.title.is_empty() && !session.title.starts_with("term-") {
        return truncate_with_ellipsis(&session.title, TERMINAL_TAB_COMMAND_MAX_CHARS);
    }

    String::new()
}

pub(crate) fn diff_tab_title(session: &DiffSession) -> String {
    truncate_with_ellipsis(&session.title, TERMINAL_TAB_COMMAND_MAX_CHARS)
}

pub(crate) fn build_worktree_diff_document(
    worktree_path: &Path,
    changed_files: &[ChangedFile],
) -> Result<(Vec<DiffLine>, HashMap<PathBuf, usize>), String> {
    let mut lines = Vec::new();
    let mut file_row_indices = HashMap::new();

    for changed_file in changed_files {
        file_row_indices.insert(changed_file.path.clone(), lines.len());
        lines.push(DiffLine {
            left_line_number: None,
            right_line_number: None,
            left_text: format!(
                "{} {}",
                change_code(changed_file.kind),
                changed_file.path.display()
            ),
            right_text: String::new(),
            kind: DiffLineKind::FileHeader,
        });

        let file_lines = build_file_diff_lines(
            worktree_path,
            changed_file.path.as_path(),
            changed_file.kind,
        )?;
        if file_lines.is_empty() {
            lines.push(DiffLine {
                left_line_number: None,
                right_line_number: None,
                left_text: "  no textual changes".to_owned(),
                right_text: String::new(),
                kind: DiffLineKind::Context,
            });
        } else {
            lines.extend(file_lines);
        }
    }

    Ok((lines, file_row_indices))
}

pub(crate) fn build_file_diff_lines(
    worktree_path: &Path,
    file_path: &Path,
    change_kind: ChangeKind,
) -> Result<Vec<DiffLine>, String> {
    let head_bytes = match change_kind {
        ChangeKind::Added | ChangeKind::IntentToAdd => Vec::new(),
        _ => read_head_file_bytes(worktree_path, file_path)?,
    };
    let worktree_bytes = match change_kind {
        ChangeKind::Removed => Vec::new(),
        _ => read_worktree_file_bytes(worktree_path, file_path)?,
    };
    let head_text = String::from_utf8_lossy(&head_bytes).into_owned();
    let worktree_text = String::from_utf8_lossy(&worktree_bytes).into_owned();
    Ok(build_side_by_side_diff_lines(&head_text, &worktree_text))
}

pub(crate) fn read_head_file_bytes(
    worktree_path: &Path,
    file_path: &Path,
) -> Result<Vec<u8>, String> {
    let relative = git_relative_path(file_path)?;
    let object_spec = format!("HEAD:{relative}");

    let repo = gix::open(worktree_path).map_err(|error| {
        format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        )
    })?;

    let object_id = match repo.rev_parse_single(object_spec.as_str()) {
        Ok(id) => id,
        Err(_) => return Ok(Vec::new()), // file does not exist at HEAD
    };

    let object = object_id.object().map_err(|error| {
        format!(
            "failed to read `{relative}` at HEAD in `{}`: {error}",
            worktree_path.display()
        )
    })?;

    Ok(object.data.to_vec())
}

pub(crate) fn read_worktree_file_bytes(
    worktree_path: &Path,
    file_path: &Path,
) -> Result<Vec<u8>, String> {
    let absolute = worktree_path.join(file_path);
    match fs::read(&absolute) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!(
            "failed to read worktree file `{}`: {error}",
            absolute.display()
        )),
    }
}

pub(crate) fn git_relative_path(file_path: &Path) -> Result<String, String> {
    let path_text = file_path.to_string_lossy();
    if path_text.trim().is_empty() {
        return Err("cannot diff an empty path".to_owned());
    }

    Ok(path_text.replace('\\', "/"))
}

pub(crate) fn build_side_by_side_diff_lines(before_text: &str, after_text: &str) -> Vec<DiffLine> {
    let before_rope = Rope::from_str(before_text);
    let after_rope = Rope::from_str(after_text);
    let input = BlobInternedInput::new(before_text.as_bytes(), after_text.as_bytes());
    let mut diff = BlobDiff::compute(DiffAlgorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let hunks = diff.hunks().collect::<Vec<_>>();

    if hunks.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut before_cursor = 0_usize;
    let mut after_cursor = 0_usize;
    let hunk_count = hunks.len();

    for (hunk_index, hunk) in hunks.iter().enumerate() {
        let before_start = hunk.before.start as usize;
        let before_end = hunk.before.end as usize;
        let after_start = hunk.after.start as usize;
        let after_end = hunk.after.end as usize;

        let (leading_context, trailing_context) = if hunk_index == 0 {
            (0, DIFF_HUNK_CONTEXT_LINES)
        } else {
            (DIFF_HUNK_CONTEXT_LINES, DIFF_HUNK_CONTEXT_LINES)
        };
        push_hunk_context_lines(
            &mut lines,
            &before_rope,
            &after_rope,
            before_cursor,
            before_start,
            after_cursor,
            after_start,
            leading_context,
            trailing_context,
        );

        let removed_count = before_end.saturating_sub(before_start);
        let added_count = after_end.saturating_sub(after_start);
        let changed_count = removed_count.max(added_count);

        for offset in 0..changed_count {
            let left_index = (offset < removed_count).then_some(before_start + offset);
            let right_index = (offset < added_count).then_some(after_start + offset);
            let kind = match (left_index.is_some(), right_index.is_some()) {
                (true, true) => DiffLineKind::Modified,
                (true, false) => DiffLineKind::Removed,
                (false, true) => DiffLineKind::Added,
                (false, false) => DiffLineKind::Context,
            };
            push_diff_line(
                &mut lines,
                &before_rope,
                &after_rope,
                left_index,
                right_index,
                kind,
            );
        }

        before_cursor = before_end;
        after_cursor = after_end;

        if hunk_index + 1 == hunk_count {
            push_hunk_context_lines(
                &mut lines,
                &before_rope,
                &after_rope,
                before_cursor,
                input.before.len(),
                after_cursor,
                input.after.len(),
                DIFF_HUNK_CONTEXT_LINES,
                0,
            );
        }
    }

    lines
}

pub(crate) fn push_hunk_context_lines(
    output: &mut Vec<DiffLine>,
    before_rope: &Rope,
    after_rope: &Rope,
    before_start: usize,
    before_end: usize,
    after_start: usize,
    after_end: usize,
    leading_context: usize,
    trailing_context: usize,
) {
    let before_count = before_end.saturating_sub(before_start);
    let after_count = after_end.saturating_sub(after_start);
    if before_count == 0 && after_count == 0 {
        return;
    }

    let leading_before_count = leading_context.min(before_count);
    let leading_after_count = leading_context.min(after_count);
    let leading_before_end = before_start.saturating_add(leading_before_count);
    let leading_after_end = after_start.saturating_add(leading_after_count);

    let trailing_before_available = before_end.saturating_sub(leading_before_end);
    let trailing_after_available = after_end.saturating_sub(leading_after_end);
    let trailing_before_count = trailing_context.min(trailing_before_available);
    let trailing_after_count = trailing_context.min(trailing_after_available);
    let trailing_before_start = before_end.saturating_sub(trailing_before_count);
    let trailing_after_start = after_end.saturating_sub(trailing_after_count);

    if leading_before_end > before_start || leading_after_end > after_start {
        push_context_diff_lines(
            output,
            before_rope,
            after_rope,
            before_start,
            leading_before_end,
            after_start,
            leading_after_end,
        );
    }

    let hidden_before_count = trailing_before_start.saturating_sub(leading_before_end);
    let hidden_after_count = trailing_after_start.saturating_sub(leading_after_end);
    if hidden_before_count > 0 || hidden_after_count > 0 {
        push_collapsed_gap_line(output, hidden_before_count, hidden_after_count);
    }

    if trailing_before_start < before_end || trailing_after_start < after_end {
        push_context_diff_lines(
            output,
            before_rope,
            after_rope,
            trailing_before_start,
            before_end,
            trailing_after_start,
            after_end,
        );
    }
}

pub(crate) fn push_collapsed_gap_line(
    output: &mut Vec<DiffLine>,
    hidden_before_count: usize,
    hidden_after_count: usize,
) {
    output.push(DiffLine {
        left_line_number: None,
        right_line_number: None,
        left_text: format!("… {hidden_before_count} unchanged lines hidden"),
        right_text: format!("… {hidden_after_count} unchanged lines hidden"),
        kind: DiffLineKind::Context,
    });
}

pub(crate) fn push_context_diff_lines(
    output: &mut Vec<DiffLine>,
    before_rope: &Rope,
    after_rope: &Rope,
    before_start: usize,
    before_end: usize,
    after_start: usize,
    after_end: usize,
) {
    let before_count = before_end.saturating_sub(before_start);
    let after_count = after_end.saturating_sub(after_start);
    let paired_count = before_count.min(after_count);

    for offset in 0..paired_count {
        push_diff_line(
            output,
            before_rope,
            after_rope,
            Some(before_start + offset),
            Some(after_start + offset),
            DiffLineKind::Context,
        );
    }

    for offset in paired_count..before_count {
        push_diff_line(
            output,
            before_rope,
            after_rope,
            Some(before_start + offset),
            None,
            DiffLineKind::Removed,
        );
    }

    for offset in paired_count..after_count {
        push_diff_line(
            output,
            before_rope,
            after_rope,
            None,
            Some(after_start + offset),
            DiffLineKind::Added,
        );
    }
}

pub(crate) fn push_diff_line(
    output: &mut Vec<DiffLine>,
    before_rope: &Rope,
    after_rope: &Rope,
    left_index: Option<usize>,
    right_index: Option<usize>,
    kind: DiffLineKind,
) {
    output.push(DiffLine {
        left_line_number: left_index.map(|index| index + 1),
        right_line_number: right_index.map(|index| index + 1),
        left_text: left_index
            .map(|index| rope_display_line(before_rope, index))
            .unwrap_or_default(),
        right_text: right_index
            .map(|index| rope_display_line(after_rope, index))
            .unwrap_or_default(),
        kind,
    });
}

pub(crate) fn rope_display_line(rope: &Rope, line_index: usize) -> String {
    if line_index >= rope.len_lines() {
        return String::new();
    }

    let mut text = rope.line(line_index).to_string();
    while text.ends_with('\n') || text.ends_with('\r') {
        let _ = text.pop();
    }
    text.replace('\t', "    ")
}

pub(crate) fn format_log_entry(entry: &log_layer::LogEntry) -> String {
    let timestamp = entry
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = timestamp.as_secs();
    let millis = timestamp.subsec_millis();
    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    let level_str = match entry.level {
        tracing::Level::ERROR => "ERROR",
        tracing::Level::WARN => "WARN ",
        tracing::Level::INFO => "INFO ",
        tracing::Level::DEBUG => "DEBUG",
        tracing::Level::TRACE => "TRACE",
    };
    let message = if entry.fields.is_empty() {
        entry.message.clone()
    } else {
        let fields_str: Vec<String> = entry
            .fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        format!("{} {}", entry.message, fields_str.join(" "))
    };
    format!(
        "{hours:02}:{minutes:02}:{seconds:02}.{millis:03} {level_str} {} {message}",
        entry.target
    )
}

pub(crate) fn render_log_row(
    entry: &log_layer::LogEntry,
    index: usize,
    theme: ThemePalette,
) -> Div {
    let timestamp = entry
        .timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = timestamp.as_secs();
    let millis = timestamp.subsec_millis();
    let hours = (total_secs / 3600) % 24;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    let time_str = format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}");

    let (level_str, level_color) = match entry.level {
        tracing::Level::ERROR => ("ERROR", 0xf38ba8_u32),
        tracing::Level::WARN => ("WARN ", 0xf9e2af),
        tracing::Level::INFO => ("INFO ", 0xa6e3a1),
        tracing::Level::DEBUG => ("DEBUG", 0x89b4fa),
        tracing::Level::TRACE => ("TRACE", 0x9399b2),
    };

    let target = truncate_with_ellipsis(&entry.target, 30);
    let bg = if index.is_multiple_of(2) {
        theme.terminal_bg
    } else {
        theme.sidebar_bg
    };

    div()
        .py(px(2.))
        .w_full()
        .flex()
        .items_start()
        .gap_2()
        .px_2()
        .font_family(FONT_MONO)
        .text_size(px(DIFF_FONT_SIZE_PX))
        .bg(rgb(bg))
        .child(
            div()
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(time_str),
        )
        .child(
            div()
                .flex_none()
                .w(px(40.))
                .text_color(rgb(level_color))
                .child(level_str),
        )
        .child(
            div()
                .flex_none()
                .w(px(200.))
                .text_color(rgb(theme.text_muted))
                .overflow_hidden()
                .child(target),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(rgb(theme.text_primary))
                .child(if entry.fields.is_empty() {
                    entry.message.clone()
                } else {
                    let fields_str: Vec<String> = entry
                        .fields
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect();
                    format!("{} {}", entry.message, fields_str.join(" "))
                }),
        )
}

pub(crate) fn render_file_view_session(
    session: FileViewSession,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
    mono_font: gpui::Font,
    editing: bool,
    cx: &mut Context<ArborWindow>,
) -> Div {
    let path_label = session.file_path.to_string_lossy().into_owned();
    let is_loading = session.is_loading;
    let session_id = session.id;
    let cursor = session.cursor;

    let (status_text, is_dirty, body) = match &session.content {
        FileViewContent::Image(image_path) => {
            let path = image_path.clone();
            (
                "image".to_owned(),
                false,
                div()
                    .id(("file-view-scroll", session_id))
                    .flex_1()
                    .min_h_0()
                    .bg(rgb(theme.terminal_bg))
                    .overflow_y_scroll()
                    .flex()
                    .justify_center()
                    .p_4()
                    .child(img(path).max_w_full().h_auto().with_fallback(move || {
                        div()
                            .text_sm()
                            .text_color(rgb(theme.text_muted))
                            .child("Failed to load image")
                            .into_any_element()
                    })),
            )
        },
        FileViewContent::Text {
            highlighted,
            raw_lines,
            dirty,
        } => {
            let line_count = raw_lines.len().max(highlighted.len());
            let status = if is_loading {
                "loading...".to_owned()
            } else {
                format!("{line_count} lines")
            };
            let highlighted = highlighted.clone();
            let raw_lines_clone = raw_lines.clone();
            let click_raw_lines = raw_lines.clone();
            let click_line_count = line_count;
            let click_scroll_handle = scroll_handle.clone();
            let line_number_width = line_count.to_string().len().max(3);
            let gutter_px = (line_number_width + 2) as f32 * DIFF_FONT_SIZE_PX * 0.6 + 8.0; // +8 for pl_2
            let body = div()
                .id(("file-view-scroll", session_id))
                .flex_1()
                .min_h_0()
                .bg(rgb(theme.terminal_bg))
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.file_view_editing = true;
                        this.right_pane_search_active = false;

                        // Compute clicked line and column from mouse position
                        let state = click_scroll_handle.0.borrow();
                        let bounds = state.base_handle.bounds();
                        let offset = state.base_handle.offset();
                        drop(state);

                        let local_y = f32::from(event.position.y - bounds.top()).max(0.);
                        let content_y = (local_y - f32::from(offset.y)).max(0.);
                        let clicked_line = ((content_y / DIFF_ROW_HEIGHT_PX).floor() as usize)
                            .min(click_line_count.saturating_sub(1));

                        let local_x =
                            (f32::from(event.position.x - bounds.left()) - gutter_px).max(0.);
                        let char_width = DIFF_FONT_SIZE_PX * 0.6;
                        let clicked_col = (local_x / char_width).floor() as usize;

                        let max_col = click_raw_lines
                            .get(clicked_line)
                            .map(|l| l.chars().count())
                            .unwrap_or(0);

                        if let Some(session) = this
                            .file_view_sessions
                            .iter_mut()
                            .find(|s| s.id == session_id)
                        {
                            session.cursor.line = clicked_line;
                            session.cursor.col = clicked_col.min(max_col);
                        }
                        cx.notify();
                    }),
                )
                .when(is_loading, |this| {
                    this.child(
                        div()
                            .h_full()
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .text_sm()
                            .text_color(rgb(theme.text_muted))
                            .child("Loading file..."),
                    )
                })
                .when(!is_loading, |this| {
                    let scroll_handle = scroll_handle.clone();
                    let mono_font = mono_font.clone();
                    let line_number_width = line_count.to_string().len().max(3);
                    let show_cursor = editing;
                    this.child(
                        div().size_full().min_w_0().flex().child(
                            uniform_list(
                                ("file-view-list", session_id),
                                line_count,
                                move |range, _, _| {
                                    range
                                        .map(|index| {
                                            let line_num = index + 1;
                                            let is_cursor_line =
                                                show_cursor && cursor.line == index;

                                            let mut content_div = div()
                                                .pl_2()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_hidden()
                                                .flex();

                                            if show_cursor {
                                                // When editing, show raw text with cursor
                                                let raw = raw_lines_clone
                                                    .get(index)
                                                    .cloned()
                                                    .unwrap_or_default();
                                                if is_cursor_line {
                                                    let byte_pos =
                                                        char_to_byte_offset(&raw, cursor.col);
                                                    let before = &raw[..byte_pos];
                                                    let after = &raw[byte_pos..];
                                                    let cursor_char =
                                                        after.chars().next().unwrap_or(' ');
                                                    let after_cursor = if after.is_empty() {
                                                        String::new()
                                                    } else {
                                                        after.chars().skip(1).collect()
                                                    };
                                                    content_div = content_div
                                                        .child(
                                                            div()
                                                                .text_color(rgb(theme.text_primary))
                                                                .child(before.to_owned()),
                                                        )
                                                        .child(
                                                            div()
                                                                .bg(rgb(theme.accent))
                                                                .text_color(rgb(theme.terminal_bg))
                                                                .child(cursor_char.to_string()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_color(rgb(theme.text_primary))
                                                                .child(after_cursor),
                                                        );
                                                } else {
                                                    content_div = content_div.child(
                                                        div()
                                                            .text_color(rgb(theme.text_primary))
                                                            .child(if raw.is_empty() {
                                                                " ".to_owned()
                                                            } else {
                                                                raw
                                                            }),
                                                    );
                                                }
                                            } else {
                                                // Not editing: show highlighted spans
                                                if let Some(spans) = highlighted.get(index) {
                                                    for span in spans {
                                                        content_div = content_div.child(
                                                            div()
                                                                .text_color(rgb(span.color))
                                                                .child(span.text.clone()),
                                                        );
                                                    }
                                                }
                                            }

                                            div()
                                                .id(("fv-row", index))
                                                .h(px(DIFF_ROW_HEIGHT_PX))
                                                .w_full()
                                                .min_w_0()
                                                .flex()
                                                .items_center()
                                                .font(mono_font.clone())
                                                .text_size(px(DIFF_FONT_SIZE_PX))
                                                .child(
                                                    div()
                                                        .w(px((line_number_width + 2) as f32
                                                            * DIFF_FONT_SIZE_PX
                                                            * 0.6))
                                                        .flex_none()
                                                        .text_color(rgb(theme.text_disabled))
                                                        .text_size(px(DIFF_FONT_SIZE_PX))
                                                        .px_1()
                                                        .flex()
                                                        .justify_end()
                                                        .child(format!("{line_num}")),
                                                )
                                                .child(content_div)
                                                .into_any_element()
                                        })
                                        .collect::<Vec<_>>()
                                },
                            )
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .track_scroll(scroll_handle.clone()),
                        ),
                    )
                });
            (status, *dirty, body)
        },
    };

    div()
        .h_full()
        .w_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(28.))
                .px_3()
                .bg(rgb(theme.tab_active_bg))
                .border_b_1()
                .border_color(rgb(theme.border))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .font(mono_font.clone())
                                .text_size(px(DIFF_FONT_SIZE_PX))
                                .text_color(rgb(theme.text_muted))
                                .child(path_label),
                        )
                        .when(is_dirty, |this| {
                            this.child(
                                div()
                                    .text_size(px(DIFF_FONT_SIZE_PX))
                                    .text_color(rgb(theme.accent))
                                    .child("\u{2022}"),
                            )
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(is_dirty, |this| {
                            this.child(
                                div()
                                    .id(("fv-save", session_id))
                                    .cursor_pointer()
                                    .px_2()
                                    .rounded_sm()
                                    .bg(rgb(theme.accent))
                                    .hover(|this| this.opacity(0.85))
                                    .text_xs()
                                    .text_color(rgb(theme.terminal_bg))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Save")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                            this.save_active_file_view(cx);
                                        }),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.text_disabled))
                                .child(status_text),
                        ),
                ),
        )
        .child(body)
}

pub(crate) fn render_diff_session(
    session: DiffSession,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> Div {
    let path_label = truncate_middle_text(&session.title, 84);
    let line_count = session.lines.len();
    let is_loading = session.is_loading;
    let session_id = session.id;

    div()
        .h_full()
        .w_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(28.))
                .px_3()
                .bg(rgb(theme.tab_active_bg))
                .border_b_1()
                .border_color(rgb(theme.border))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .font(mono_font.clone())
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(theme.text_muted))
                        .child(path_label),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.text_disabled))
                        .child(if is_loading {
                            "loading...".to_owned()
                        } else {
                            format!("{line_count} rows")
                        }),
                ),
        )
        .child(
            div()
                .id(("diff-scroll", session_id))
                .flex_1()
                .min_h_0()
                .bg(rgb(theme.terminal_bg))
                .when(is_loading, |this| {
                    this.child(
                        div()
                            .h_full()
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .text_sm()
                            .text_color(rgb(theme.text_muted))
                            .child("Computing diff..."),
                    )
                })
                .when(!is_loading, |this| {
                    let lines = session.lines.clone();
                    let zonemap_lines = lines.clone();
                    let scroll_handle = scroll_handle.clone();
                    let mono_font = mono_font.clone();
                    this.child(
                        div()
                            .size_full()
                            .min_w_0()
                            .flex()
                            .child(
                                uniform_list(
                                    ("diff-list", session_id),
                                    lines.len(),
                                    move |range, _, _| {
                                        range
                                            .map(|index| {
                                                render_diff_row(
                                                    session_id,
                                                    index,
                                                    lines[index].clone(),
                                                    theme,
                                                    mono_font.clone(),
                                                    diff_cell_width,
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                    },
                                )
                                .h_full()
                                .flex_1()
                                .min_w_0()
                                .track_scroll(scroll_handle.clone()),
                            )
                            .child(render_diff_zonemap(zonemap_lines, theme, &scroll_handle)),
                    )
                }),
        )
}

pub(crate) fn render_diff_row(
    session_id: u64,
    row_index: usize,
    line: DiffLine,
    theme: ThemePalette,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> impl IntoElement {
    if line.kind == DiffLineKind::FileHeader {
        return div()
            .id(diff_row_element_id(
                "diff-row-header",
                session_id,
                row_index,
            ))
            .w_full()
            .h(px(DIFF_ROW_HEIGHT_PX))
            .min_h(px(DIFF_ROW_HEIGHT_PX))
            .bg(rgb(theme.tab_active_bg))
            .px_2()
            .flex()
            .items_center()
            .child(
                div()
                    .min_w_0()
                    .font(mono_font)
                    .text_size(px(DIFF_FONT_SIZE_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .text_color(rgb(theme.text_primary))
                    .child(line.left_text),
            );
    }

    let (left_bg, right_bg) = diff_line_backgrounds(line.kind, theme);
    let (left_marker, right_marker) = diff_line_markers(line.kind);
    let (left_text_color, right_text_color) = diff_line_text_colors(line.kind, theme);
    div()
        .id(diff_row_element_id("diff-row", session_id, row_index))
        .w_full()
        .min_w_0()
        .h(px(DIFF_ROW_HEIGHT_PX))
        .min_h(px(DIFF_ROW_HEIGHT_PX))
        .flex()
        .child(render_diff_column(
            session_id,
            row_index,
            0,
            line.left_line_number,
            line.left_text,
            left_marker,
            left_bg,
            left_text_color,
            theme,
            mono_font.clone(),
            diff_cell_width,
        ))
        .child(render_diff_column(
            session_id,
            row_index,
            1,
            line.right_line_number,
            line.right_text,
            right_marker,
            right_bg,
            right_text_color,
            theme,
            mono_font,
            diff_cell_width,
        ))
}

pub(crate) fn render_diff_column(
    session_id: u64,
    row_index: usize,
    side: usize,
    line_number: Option<usize>,
    text: String,
    marker: char,
    background: u32,
    text_color: u32,
    theme: ThemePalette,
    mono_font: gpui::Font,
    diff_cell_width: f32,
) -> impl IntoElement {
    let number_width = px((DIFF_LINE_NUMBER_WIDTH_CHARS as f32 * diff_cell_width) + 12.);

    let column_id = diff_row_side_element_id("diff-column", session_id, row_index, side);
    let marker_id = diff_row_side_element_id("diff-marker", session_id, row_index, side);
    let text_id = diff_row_side_element_id("diff-text", session_id, row_index, side);

    div()
        .id(column_id)
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(rgb(background))
        .child(
            div()
                .h_full()
                .min_w_0()
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(number_width)
                        .flex_none()
                        .text_right()
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(theme.text_disabled))
                        .child(line_number.map_or(String::new(), |line| line.to_string())),
                )
                .child(
                    div()
                        .id(marker_id)
                        .w(px(10.))
                        .flex_none()
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .text_color(rgb(diff_marker_color(marker)))
                        .child(marker.to_string()),
                )
                .child(
                    div()
                        .id(text_id)
                        .min_w_0()
                        .flex_1()
                        .font(mono_font)
                        .text_size(px(DIFF_FONT_SIZE_PX))
                        .whitespace_nowrap()
                        .text_color(rgb(text_color))
                        .child(if text.is_empty() {
                            " ".to_owned()
                        } else {
                            text
                        }),
                ),
        )
}

pub(crate) fn render_diff_zonemap(
    lines: Arc<[DiffLine]>,
    theme: ThemePalette,
    scroll_handle: &UniformListScrollHandle,
) -> Div {
    let scroll_handle_for_draw = scroll_handle.clone();
    let scroll_handle_for_click = scroll_handle.clone();
    let scroll_handle_for_drag = scroll_handle.clone();
    let total_rows = lines.len();
    let marker_spans = build_zonemap_marker_spans(lines.as_ref());

    div()
        .h_full()
        .w(px(DIFF_ZONEMAP_WIDTH_PX + (DIFF_ZONEMAP_MARGIN_PX * 2.)))
        .pt(px(DIFF_ZONEMAP_MARGIN_PX))
        .pb(px(DIFF_ZONEMAP_MARGIN_PX))
        .pl(px(DIFF_ZONEMAP_MARGIN_PX))
        .pr(px(DIFF_ZONEMAP_MARGIN_PX))
        .flex_none()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, _| {
            if total_rows == 0 {
                return;
            }

            let bounds = scroll_handle_for_click.0.borrow().base_handle.bounds();
            let height = bounds.size.height.to_f64() as f32;
            if !height.is_finite() || height <= 0. {
                return;
            }

            let relative_y = (f32::from(event.position.y - bounds.top()) / height).clamp(0., 1.);
            let mut target_row = (relative_y * total_rows as f32).floor() as usize;
            if target_row >= total_rows {
                target_row = total_rows.saturating_sub(1);
            }
            scroll_handle_for_click.scroll_to_item(target_row, ScrollStrategy::Center);
        })
        .on_mouse_move(move |event: &MouseMoveEvent, _, _| {
            if event.pressed_button != Some(MouseButton::Left) || total_rows == 0 {
                return;
            }

            let bounds = scroll_handle_for_drag.0.borrow().base_handle.bounds();
            let height = bounds.size.height.to_f64() as f32;
            if !height.is_finite() || height <= 0. {
                return;
            }

            let relative_y = (f32::from(event.position.y - bounds.top()) / height).clamp(0., 1.);
            let mut target_row = (relative_y * total_rows as f32).floor() as usize;
            if target_row >= total_rows {
                target_row = total_rows.saturating_sub(1);
            }
            scroll_handle_for_drag.scroll_to_item(target_row, ScrollStrategy::Center);
        })
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _cx| {
                    window.paint_quad(fill(bounds, rgb(theme.app_bg)));

                    let track_origin = point(bounds.origin.x + px(1.), bounds.origin.y + px(1.));
                    let track_size = size(
                        (bounds.size.width - px(2.)).max(px(1.)),
                        (bounds.size.height - px(2.)).max(px(1.)),
                    );
                    let track_bounds = Bounds::new(track_origin, track_size);
                    window.paint_quad(fill(track_bounds, rgb(theme.panel_bg)));

                    if total_rows == 0 {
                        return;
                    }

                    let height = track_bounds.size.height.to_f64() as f32;
                    if !height.is_finite() || height <= 0. {
                        return;
                    }

                    let marker_origin_x = track_bounds.origin.x + px(1.);
                    let marker_width = (track_bounds.size.width - px(2.)).max(px(1.));

                    for span in &marker_spans {
                        let start_ratio = span.start_row as f32 / total_rows as f32;
                        let end_ratio = span.end_row.saturating_add(1) as f32 / total_rows as f32;
                        let y = track_bounds.origin.y + px(start_ratio * height);
                        let marker_height =
                            px(((end_ratio - start_ratio) * height)
                                .max(DIFF_ZONEMAP_MARKER_HEIGHT_PX));
                        window.paint_quad(fill(
                            Bounds::new(
                                point(marker_origin_x, y),
                                size(marker_width, marker_height),
                            ),
                            rgb(span.color),
                        ));
                    }

                    let (visible_top, visible_bottom) =
                        diff_visible_row_range(&scroll_handle_for_draw, total_rows);
                    let visible_count =
                        visible_bottom.saturating_sub(visible_top).saturating_add(1);
                    let thumb_top_ratio = visible_top as f32 / total_rows as f32;
                    let thumb_height_ratio = visible_count as f32 / total_rows as f32;
                    let thumb_height = px((thumb_height_ratio * height)
                        .max(DIFF_ZONEMAP_MIN_THUMB_HEIGHT_PX)
                        .min(height));
                    let max_thumb_top =
                        track_bounds.origin.y + track_bounds.size.height - thumb_height;
                    let thumb_top = (track_bounds.origin.y + px(thumb_top_ratio * height))
                        .min(max_thumb_top)
                        .max(track_bounds.origin.y);

                    window.paint_quad(fill(
                        Bounds::new(
                            point(track_bounds.origin.x, thumb_top),
                            size(track_bounds.size.width, thumb_height),
                        ),
                        rgb(theme.accent),
                    ));
                },
            )
            .size_full(),
        )
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ZonemapMarkerSpan {
    pub(crate) start_row: usize,
    pub(crate) end_row: usize,
    pub(crate) color: u32,
}

pub(crate) fn build_zonemap_marker_spans(lines: &[DiffLine]) -> Vec<ZonemapMarkerSpan> {
    let mut spans: Vec<ZonemapMarkerSpan> = Vec::new();

    for (row, line) in lines.iter().enumerate() {
        let Some(color) = zonemap_marker_color(line.kind) else {
            continue;
        };

        if let Some(last) = spans.last_mut()
            && last.color == color
            && row == last.end_row.saturating_add(1)
        {
            last.end_row = row;
            continue;
        }

        spans.push(ZonemapMarkerSpan {
            start_row: row,
            end_row: row,
            color,
        });
    }

    spans
}

pub(crate) fn diff_visible_row_range(
    scroll_handle: &UniformListScrollHandle,
    total_rows: usize,
) -> (usize, usize) {
    if total_rows == 0 {
        return (0, 0);
    }

    let state = scroll_handle.0.borrow();
    let max_row = total_rows.saturating_sub(1);
    let viewport_height = f32::from(state.base_handle.bounds().size.height).max(0.);
    let scroll_offset_y = (-f32::from(state.base_handle.offset().y)).max(0.);

    let top = (scroll_offset_y / DIFF_ROW_HEIGHT_PX).floor() as usize;
    let visible_rows = ((viewport_height / DIFF_ROW_HEIGHT_PX).ceil() as usize).max(1);
    let bottom = top.saturating_add(visible_rows.saturating_sub(1));

    let clamped_top = top.min(max_row);
    let clamped_bottom = bottom.min(max_row);
    (clamped_top, clamped_bottom.max(clamped_top))
}

pub(crate) fn zonemap_marker_color(kind: DiffLineKind) -> Option<u32> {
    match kind {
        DiffLineKind::FileHeader => Some(0x6d88a6),
        DiffLineKind::Added => Some(0x72d69c),
        DiffLineKind::Removed => Some(0xeb6f92),
        DiffLineKind::Modified => Some(0xf9e2af),
        DiffLineKind::Context => None,
    }
}

pub(crate) fn diff_line_backgrounds(kind: DiffLineKind, theme: ThemePalette) -> (u32, u32) {
    match kind {
        DiffLineKind::FileHeader => (theme.tab_active_bg, theme.tab_active_bg),
        DiffLineKind::Context
        | DiffLineKind::Added
        | DiffLineKind::Removed
        | DiffLineKind::Modified => (theme.terminal_bg, theme.terminal_bg),
    }
}

pub(crate) fn diff_line_text_colors(kind: DiffLineKind, theme: ThemePalette) -> (u32, u32) {
    match kind {
        DiffLineKind::FileHeader => (theme.text_primary, theme.text_primary),
        DiffLineKind::Context => (theme.text_primary, theme.text_primary),
        DiffLineKind::Added => (theme.text_disabled, 0x8fd7ad),
        DiffLineKind::Removed => (0xf2a4b7, theme.text_disabled),
        DiffLineKind::Modified => (0xf2a4b7, 0x8fd7ad),
    }
}

pub(crate) fn diff_line_markers(kind: DiffLineKind) -> (char, char) {
    match kind {
        DiffLineKind::FileHeader => (' ', ' '),
        DiffLineKind::Context => (' ', ' '),
        DiffLineKind::Added => (' ', '+'),
        DiffLineKind::Removed => ('-', ' '),
        DiffLineKind::Modified => ('-', '+'),
    }
}

pub(crate) fn diff_marker_color(marker: char) -> u32 {
    match marker {
        '+' => 0x72d69c,
        '-' => 0xeb6f92,
        '~' => 0xf9e2af,
        _ => 0x7c8599,
    }
}

pub(crate) fn wrap_diff_document_lines(
    raw_lines: &[DiffLine],
    raw_file_row_indices: &HashMap<PathBuf, usize>,
    wrap_columns: usize,
) -> (Vec<DiffLine>, HashMap<PathBuf, usize>) {
    let mut wrapped_lines = Vec::new();
    let mut raw_to_wrapped_index = Vec::with_capacity(raw_lines.len());

    for raw_line in raw_lines {
        raw_to_wrapped_index.push(wrapped_lines.len());
        wrapped_lines.extend(wrap_diff_line(raw_line.clone(), wrap_columns));
    }

    let wrapped_file_row_indices = raw_file_row_indices
        .iter()
        .map(|(path, raw_index)| {
            let wrapped_index = raw_to_wrapped_index.get(*raw_index).copied().unwrap_or(0);
            (path.clone(), wrapped_index)
        })
        .collect::<HashMap<_, _>>();

    (wrapped_lines, wrapped_file_row_indices)
}

pub(crate) fn wrap_diff_line(line: DiffLine, wrap_columns: usize) -> Vec<DiffLine> {
    let wrap_columns = wrap_columns.max(1);
    if line.kind == DiffLineKind::FileHeader {
        return split_diff_text_chunks(line.left_text, wrap_columns.saturating_mul(2))
            .into_iter()
            .map(|chunk| DiffLine {
                left_line_number: None,
                right_line_number: None,
                left_text: chunk,
                right_text: String::new(),
                kind: DiffLineKind::FileHeader,
            })
            .collect();
    }

    let left_chunks = split_diff_text_chunks(line.left_text, wrap_columns);
    let right_chunks = split_diff_text_chunks(line.right_text, wrap_columns);
    let chunk_count = left_chunks.len().max(right_chunks.len()).max(1);
    let mut wrapped = Vec::with_capacity(chunk_count);

    for index in 0..chunk_count {
        wrapped.push(DiffLine {
            left_line_number: (index == 0).then_some(line.left_line_number).flatten(),
            right_line_number: (index == 0).then_some(line.right_line_number).flatten(),
            left_text: left_chunks.get(index).cloned().unwrap_or_default(),
            right_text: right_chunks.get(index).cloned().unwrap_or_default(),
            kind: line.kind,
        });
    }

    wrapped
}

pub(crate) fn split_diff_text_chunks(text: String, wrap_columns: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let wrap_columns = wrap_columns.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0_usize;

    for ch in text.chars() {
        current.push(ch);
        current_len += 1;

        if current_len >= wrap_columns {
            chunks.push(current);
            current = String::new();
            current_len = 0;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    if chunks.is_empty() {
        chunks.push(String::new());
    }

    chunks
}

pub(crate) fn diff_row_element_id(
    prefix: &'static str,
    session_id: u64,
    row_index: usize,
) -> ElementId {
    let session_scope = ElementId::from((prefix, session_id));
    ElementId::from((session_scope, row_index.to_string()))
}

pub(crate) fn diff_row_side_element_id(
    prefix: &'static str,
    session_id: u64,
    row_index: usize,
    side: usize,
) -> ElementId {
    let row_scope = diff_row_element_id(prefix, session_id, row_index);
    ElementId::from((row_scope, side.to_string()))
}

pub(crate) fn truncate_with_ellipsis(value: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_owned();
    }

    // Take max_chars - 1 characters + "…" so total stays within budget
    let truncated: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}\u{2026}")
}

pub(crate) fn notice_looks_like_error(notice: &str) -> bool {
    let lower = notice.to_ascii_lowercase();
    [
        "error",
        "failed",
        "invalid",
        "cannot",
        "could not",
        "missing",
        "not found",
        "denied",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

pub(crate) fn action_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    style: ActionButtonStyle,
    enabled: bool,
) -> Stateful<Div> {
    let background = if enabled && style == ActionButtonStyle::Primary {
        theme.panel_active_bg
    } else {
        theme.panel_bg
    };
    let text_color = if enabled {
        theme.text_primary
    } else {
        theme.text_disabled
    };

    div()
        .id(id)
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
        })
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(background))
        .px_2()
        .py_1()
        .text_xs()
        .text_color(rgb(text_color))
        .child(label.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionButtonStyle {
    Primary,
    Secondary,
}

pub(crate) fn preset_icon_image(kind: AgentPresetKind) -> Arc<Image> {
    static CLAUDE_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static CODEX_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static PI_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static OPENCODE_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    static COPILOT_ICON: OnceLock<Arc<Image>> = OnceLock::new();

    let lock = match kind {
        AgentPresetKind::Codex => &CODEX_ICON,
        AgentPresetKind::Claude => &CLAUDE_ICON,
        AgentPresetKind::Pi => &PI_ICON,
        AgentPresetKind::OpenCode => &OPENCODE_ICON,
        AgentPresetKind::Copilot => &COPILOT_ICON,
    };

    lock.get_or_init(|| {
        tracing::info!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            bytes = preset_icon_bytes(kind).len(),
            "loading preset icon asset"
        );
        Arc::new(Image::from_bytes(
            preset_icon_format(kind),
            preset_icon_bytes(kind).to_vec(),
        ))
    })
    .clone()
}

pub(crate) fn preset_icon_bytes(kind: AgentPresetKind) -> &'static [u8] {
    match kind {
        AgentPresetKind::Codex => PRESET_ICON_CODEX_SVG,
        AgentPresetKind::Claude => PRESET_ICON_CLAUDE_PNG,
        AgentPresetKind::Pi => PRESET_ICON_PI_SVG,
        AgentPresetKind::OpenCode => PRESET_ICON_OPENCODE_SVG,
        AgentPresetKind::Copilot => PRESET_ICON_COPILOT_SVG,
    }
}

pub(crate) fn preset_icon_format(kind: AgentPresetKind) -> ImageFormat {
    match kind {
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => ImageFormat::Svg,
        AgentPresetKind::Claude => ImageFormat::Png,
    }
}

pub(crate) fn preset_icon_asset_path(kind: AgentPresetKind) -> &'static str {
    match kind {
        AgentPresetKind::Codex => "assets/preset-icons/codex-white.svg",
        AgentPresetKind::Claude => "assets/preset-icons/claude.png",
        AgentPresetKind::Pi => "assets/preset-icons/pi-white.svg",
        AgentPresetKind::OpenCode => "assets/preset-icons/opencode-white.svg",
        AgentPresetKind::Copilot => "assets/preset-icons/copilot-white.svg",
    }
}

pub(crate) fn log_preset_icon_fallback_once(kind: AgentPresetKind, fallback_glyph: &'static str) {
    static CLAUDE_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static CODEX_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static PI_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static OPENCODE_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();
    static COPILOT_FALLBACK_LOGGED: OnceLock<()> = OnceLock::new();

    let once = match kind {
        AgentPresetKind::Codex => &CODEX_FALLBACK_LOGGED,
        AgentPresetKind::Claude => &CLAUDE_FALLBACK_LOGGED,
        AgentPresetKind::Pi => &PI_FALLBACK_LOGGED,
        AgentPresetKind::OpenCode => &OPENCODE_FALLBACK_LOGGED,
        AgentPresetKind::Copilot => &COPILOT_FALLBACK_LOGGED,
    };

    once.get_or_init(|| {
        tracing::warn!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            bytes = preset_icon_bytes(kind).len(),
            fallback = fallback_glyph,
            "preset icon asset could not be rendered, using fallback glyph"
        );
        eprintln!(
            "WARN preset icon fallback preset={} asset={} bytes={} fallback={}",
            kind.key(),
            preset_icon_asset_path(kind),
            preset_icon_bytes(kind).len(),
            fallback_glyph
        );
    });
}

pub(crate) fn log_preset_icon_render_once(kind: AgentPresetKind) {
    static CLAUDE_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static CODEX_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static PI_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static OPENCODE_RENDER_LOGGED: OnceLock<()> = OnceLock::new();
    static COPILOT_RENDER_LOGGED: OnceLock<()> = OnceLock::new();

    let once = match kind {
        AgentPresetKind::Codex => &CODEX_RENDER_LOGGED,
        AgentPresetKind::Claude => &CLAUDE_RENDER_LOGGED,
        AgentPresetKind::Pi => &PI_RENDER_LOGGED,
        AgentPresetKind::OpenCode => &OPENCODE_RENDER_LOGGED,
        AgentPresetKind::Copilot => &COPILOT_RENDER_LOGGED,
    };

    once.get_or_init(|| {
        tracing::info!(
            preset = kind.key(),
            asset = preset_icon_asset_path(kind),
            "preset icon render path active"
        );
    });
}

pub(crate) fn preset_icon_render_size_px(kind: AgentPresetKind) -> f32 {
    match kind {
        AgentPresetKind::Codex => 20.,
        AgentPresetKind::Claude
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => 14.,
    }
}

pub(crate) fn agent_preset_button_content(kind: AgentPresetKind, text_color: u32) -> Div {
    log_preset_icon_render_once(kind);
    let icon = preset_icon_image(kind);
    let icon_size = preset_icon_render_size_px(kind);
    // Use consistent slot size for all icons to ensure vertical alignment
    let icon_slot_size = 20_f32;
    let fallback_color = match kind {
        AgentPresetKind::Claude => 0xD97757,
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => text_color,
    };
    let fallback_glyph = match kind {
        AgentPresetKind::Claude => "C",
        AgentPresetKind::Codex
        | AgentPresetKind::Pi
        | AgentPresetKind::OpenCode
        | AgentPresetKind::Copilot => kind.fallback_icon(),
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .w(px(icon_slot_size))
                .h(px(icon_slot_size))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(img(icon).size(px(icon_size)).with_fallback(move || {
                    log_preset_icon_fallback_once(kind, fallback_glyph);
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(12.))
                        .line_height(px(12.))
                        .text_color(rgb(fallback_color))
                        .child(fallback_glyph)
                        .into_any_element()
                })),
        )
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(14.))
                .text_color(rgb(text_color))
                .child(kind.label()),
        )
}

pub(crate) fn git_action_button(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    icon: &'static str,
    label: &'static str,
    enabled: bool,
    active: bool,
) -> Stateful<Div> {
    let background = if active {
        theme.panel_active_bg
    } else {
        theme.panel_bg
    };
    let icon_color = if active {
        theme.accent
    } else if enabled {
        theme.text_muted
    } else {
        theme.text_disabled
    };
    let text_color = if enabled || active {
        theme.text_primary
    } else {
        theme.text_disabled
    };

    div()
        .id(id)
        .h(px(24.))
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme.border))
        .bg(rgb(background))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .when(enabled, |this| {
            this.cursor_pointer()
                .hover(|this| this.bg(rgb(theme.panel_active_bg)))
        })
        .child(
            div()
                .font_family(FONT_MONO)
                .text_size(px(13.))
                .text_color(rgb(icon_color))
                .child(icon),
        )
        .child(div().text_xs().text_color(rgb(text_color)).child(label))
}

pub(crate) fn modal_backdrop() -> Div {
    div().absolute().inset_0().bg(rgb(0x000000)).opacity(0.28)
}

pub(crate) fn modal_input_field(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    label: impl Into<String>,
    value: &str,
    cursor: usize,
    placeholder: impl Into<String>,
    active: bool,
) -> Stateful<Div> {
    let label = label.into();
    let placeholder = placeholder.into();

    div()
        .id(id)
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme.text_muted))
                .child(label),
        )
        .child(
            div()
                .overflow_hidden()
                .cursor_pointer()
                .rounded_sm()
                .border_1()
                .border_color(rgb(if active {
                    theme.accent
                } else {
                    theme.border
                }))
                .bg(rgb(theme.panel_bg))
                .px_2()
                .py_1()
                .text_sm()
                .font_family(FONT_MONO)
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(if active {
                    if value.is_empty() {
                        active_input_display(theme, "", &placeholder, theme.text_disabled, 0, 48)
                    } else {
                        active_input_display(
                            theme,
                            value,
                            &placeholder,
                            theme.text_primary,
                            cursor,
                            56,
                        )
                    }
                } else if value.is_empty() {
                    div()
                        .text_color(rgb(theme.text_disabled))
                        .child(placeholder)
                        .into_any_element()
                } else {
                    div()
                        .text_color(rgb(theme.text_primary))
                        .child(value.to_owned())
                        .into_any_element()
                }),
        )
}

pub(crate) fn single_line_input_field(
    theme: ThemePalette,
    id: impl Into<ElementId>,
    value: &str,
    cursor: usize,
    placeholder: impl Into<String>,
    active: bool,
) -> Stateful<Div> {
    let placeholder = placeholder.into();

    div()
        .id(id)
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .h(px(30.))
        .cursor_text()
        .rounded_sm()
        .border_1()
        .border_color(rgb(if active {
            theme.accent
        } else {
            theme.border
        }))
        .bg(rgb(theme.panel_bg))
        .px_2()
        .text_sm()
        .font_family(FONT_MONO)
        .flex()
        .items_center()
        .child(if active {
            if value.is_empty() {
                active_input_display(theme, "", &placeholder, theme.text_disabled, 0, 48)
            } else {
                active_input_display(theme, value, &placeholder, theme.text_primary, cursor, 48)
            }
        } else {
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(rgb(if value.is_empty() {
                    theme.text_disabled
                } else {
                    theme.text_primary
                }))
                .child(if value.is_empty() {
                    placeholder
                } else {
                    value.to_owned()
                })
                .into_any_element()
        })
}

pub(crate) fn active_input_display(
    theme: ThemePalette,
    value: &str,
    placeholder: &str,
    text_color: u32,
    cursor: usize,
    max_chars: usize,
) -> AnyElement {
    if value.is_empty() {
        return div()
            .relative()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(
                div()
                    .text_color(rgb(text_color))
                    .child(placeholder.to_owned()),
            )
            .child(
                input_caret(theme)
                    .flex_none()
                    .absolute()
                    .left(px(0.))
                    .top(px(2.)),
            )
            .into_any_element();
    }

    div()
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .justify_start()
        .gap(px(0.))
        .child({
            let (before_cursor, after_cursor) = visible_input_segments(value, cursor, max_chars);
            div()
                .flex()
                .items_center()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(text_color))
                        .child(before_cursor),
                )
                .child(input_caret(theme).flex_none())
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(text_color))
                        .child(after_cursor),
                )
        })
        .into_any_element()
}

pub(crate) fn visible_input_segments(
    value: &str,
    cursor: usize,
    max_chars: usize,
) -> (String, String) {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();
    let cursor = cursor.min(len);
    if len <= max_chars {
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars[cursor..].iter().collect();
        return (before, after);
    }

    let window = max_chars.max(1);
    let preferred_left = window.saturating_sub(8);
    let mut start = cursor.saturating_sub(preferred_left);
    start = start.min(len.saturating_sub(window));
    let end = (start + window).min(len);

    let mut before: String = chars[start..cursor].iter().collect();
    let mut after: String = chars[cursor..end].iter().collect();
    if start > 0 {
        before.insert(0, '\u{2026}');
    }
    if end < len {
        after.push('\u{2026}');
    }
    (before, after)
}

pub(crate) fn input_caret(theme: ThemePalette) -> Div {
    div().w(px(1.)).h(px(14.)).bg(rgb(theme.accent)).mt(px(1.))
}

pub(crate) fn status_text(theme: ThemePalette, text: impl Into<String>) -> Div {
    div()
        .text_xs()
        .text_color(rgb(theme.text_muted))
        .child(text.into())
}

pub(crate) fn is_gui_editor(editor: &str) -> bool {
    let basename = Path::new(editor)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(editor);
    matches!(
        basename,
        "code"
            | "codium"
            | "subl"
            | "atom"
            | "gedit"
            | "kate"
            | "mousepad"
            | "xed"
            | "pluma"
            | "gvim"
            | "mvim"
            | "mate"
            | "bbedit"
            | "nova"
            | "zed"
            | "cursor"
            | "fleet"
            | "lite-xl"
    )
}

pub(crate) fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_alphanumeric() || c == '/' || c == '.' || c == '-' || c == '_')
    {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

pub(crate) fn char_to_byte_offset(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(byte, _)| byte)
        .unwrap_or(s.len())
}

pub(crate) fn char_count(s: &str) -> usize {
    s.chars().count()
}

pub(crate) fn apply_text_edit_action(
    text: &mut String,
    cursor: &mut usize,
    action: &TextEditAction,
) {
    *cursor = (*cursor).min(char_count(text));
    match action {
        TextEditAction::Insert(insert_text) => {
            let byte_offset = char_to_byte_offset(text, *cursor);
            text.insert_str(byte_offset, insert_text);
            *cursor += insert_text.chars().count();
        },
        TextEditAction::Backspace => {
            if *cursor == 0 {
                return;
            }
            let end = char_to_byte_offset(text, *cursor);
            let start = char_to_byte_offset(text, *cursor - 1);
            text.replace_range(start..end, "");
            *cursor -= 1;
        },
        TextEditAction::Delete => {
            let len = char_count(text);
            if *cursor >= len {
                return;
            }
            let start = char_to_byte_offset(text, *cursor);
            let end = char_to_byte_offset(text, *cursor + 1);
            text.replace_range(start..end, "");
        },
        TextEditAction::MoveLeft => {
            *cursor = (*cursor).saturating_sub(1);
        },
        TextEditAction::MoveRight => {
            *cursor = (*cursor + 1).min(char_count(text));
        },
        TextEditAction::MoveHome => {
            *cursor = 0;
        },
        TextEditAction::MoveEnd => {
            *cursor = char_count(text);
        },
    }
}

pub(crate) fn typed_text_for_keystroke(event: &KeyDownEvent) -> Option<String> {
    event
        .keystroke
        .key_char
        .as_deref()
        .or_else(|| {
            let key = event.keystroke.key.as_str();
            if key.chars().count() == 1 {
                Some(key)
            } else {
                None
            }
        })
        .map(ToOwned::to_owned)
}

pub(crate) fn text_edit_action_for_event(
    event: &KeyDownEvent,
    cx: &mut Context<ArborWindow>,
) -> Option<TextEditAction> {
    match event.keystroke.key.as_str() {
        "backspace" => return Some(TextEditAction::Backspace),
        "delete" => return Some(TextEditAction::Delete),
        "left" => return Some(TextEditAction::MoveLeft),
        "right" => return Some(TextEditAction::MoveRight),
        "home" => return Some(TextEditAction::MoveHome),
        "end" => return Some(TextEditAction::MoveEnd),
        _ => {},
    }

    if event.keystroke.modifiers.platform {
        if event.keystroke.key.as_str() == "v"
            && let Some(clipboard) = cx.read_from_clipboard()
        {
            let text = clipboard.text().unwrap_or_default();
            if !text.is_empty() {
                return Some(TextEditAction::Insert(text));
            }
        }
        return None;
    }

    if event.keystroke.modifiers.control || event.keystroke.modifiers.alt {
        return None;
    }

    typed_text_for_keystroke(event).map(TextEditAction::Insert)
}

pub(crate) fn highlight_lines_with_syntect(
    raw_lines: &[String],
    ext: &str,
    default_color: u32,
) -> Vec<Vec<FileViewSpan>> {
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();
    let theme = &theme_set.themes["base16-ocean.dark"];
    if let Some(syntax) = syntax_set.find_syntax_by_extension(ext) {
        let mut highlighter = HighlightLines::new(syntax, theme);
        raw_lines
            .iter()
            .map(|line| {
                // Syntect grammars loaded with load_defaults_newlines() require
                // newline-terminated lines for correct tokenisation.
                let line_nl = format!("{line}\n");
                match highlighter.highlight_line(&line_nl, &syntax_set) {
                    Ok(ranges) => ranges
                        .into_iter()
                        .filter_map(|(style, text)| {
                            let trimmed = text.trim_end_matches('\n');
                            if trimmed.is_empty() {
                                return None;
                            }
                            let c = style.foreground;
                            Some(FileViewSpan {
                                text: trimmed.to_owned(),
                                color: (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32,
                            })
                        })
                        .collect(),
                    Err(_) => vec![FileViewSpan {
                        text: line.to_owned(),
                        color: default_color,
                    }],
                }
            })
            .collect()
    } else {
        raw_lines
            .iter()
            .map(|line| {
                vec![FileViewSpan {
                    text: line.to_owned(),
                    color: default_color,
                }]
            })
            .collect()
    }
}

pub(crate) fn file_icon_and_color(name: &str, is_dir: bool) -> (&'static str, u32) {
    if is_dir {
        return ("\u{f07b}", 0xe5c07b);
    }

    // Check full filename first
    match name {
        "Dockerfile" | ".dockerignore" => return ("\u{e7b0}", 0x61afef),
        "Makefile" | "Justfile" => return ("\u{e615}", 0x98c379),
        ".gitignore" | ".env" => return ("\u{e615}", 0x838994),
        _ => {},
    }

    // Check extension
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "rs" => ("\u{e7a8}", 0xe06c75),
        "toml" => ("\u{e615}", 0x838994),
        "py" => ("\u{e73c}", 0x61afef),
        "js" => ("\u{e74e}", 0xe5c07b),
        "ts" => ("\u{e628}", 0x61afef),
        "jsx" | "tsx" => ("\u{e7ba}", 0x56b6c2),
        "json" => ("\u{e60b}", 0xe5c07b),
        "html" => ("\u{e736}", 0xe06c75),
        "css" | "scss" | "sass" => ("\u{e749}", 0x56b6c2),
        "md" | "mdx" => ("\u{e73e}", 0x61afef),
        "yaml" | "yml" => ("\u{e615}", 0xc678dd),
        "sh" | "bash" | "zsh" => ("\u{e795}", 0x98c379),
        "go" => ("\u{e627}", 0x56b6c2),
        "c" | "h" => ("\u{e61e}", 0x61afef),
        "cpp" | "hpp" | "cc" => ("\u{e61d}", 0xe06c75),
        "java" => ("\u{e738}", 0xe06c75),
        "rb" => ("\u{e739}", 0xe06c75),
        "swift" => ("\u{e755}", 0xe06c75),
        "lock" => ("\u{f023}", 0x838994),
        "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" => ("\u{f1c5}", 0xc678dd),
        "txt" | "log" => ("\u{f15c}", 0x838994),
        "xml" => ("\u{e619}", 0xe5c07b),
        "sql" => ("\u{f1c0}", 0xe5c07b),
        _ => ("\u{f15c}", 0x838994),
    }
}

pub(crate) fn change_code(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "A",
        ChangeKind::Modified => "M",
        ChangeKind::Removed => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Copied => "C",
        ChangeKind::TypeChange => "T",
        ChangeKind::Conflict => "U",
        ChangeKind::IntentToAdd => "I",
    }
}

pub(crate) fn truncate_middle_path_for_width(path: &Path, right_pane_width: f32) -> String {
    let path_text = path.display().to_string();
    let available_width = (right_pane_width - 110.).max(120.);
    let max_chars = ((available_width / 7.3).floor() as usize).clamp(18, 96);
    truncate_middle_text(&path_text, max_chars)
}

pub(crate) fn truncate_middle_text(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_owned();
    }

    if max_chars <= 1 {
        return "…".to_owned();
    }

    let keep = max_chars - 1;
    let tail_keep = (keep * 3) / 5;
    let head_keep = keep.saturating_sub(tail_keep);
    let tail_start = chars.len().saturating_sub(tail_keep);

    let mut output = String::with_capacity(max_chars);
    output.extend(chars.iter().take(head_keep));
    output.push('…');
    output.extend(chars.iter().skip(tail_start));
    output
}

pub(crate) fn run_launch_command(command: &mut Command, operation: &str) -> Result<(), String> {
    let output = run_command_output(command, operation)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failure_message(operation, &output))
    }
}

pub(crate) fn open_worktree_in_file_manager(worktree_path: &Path) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let mut command = create_command("open");
        command.arg(worktree_path);
        run_launch_command(&mut command, "open worktree in Finder")?;
        return Ok("opened worktree in Finder".to_owned());
    }

    #[cfg(target_os = "linux")]
    {
        let mut command = create_command("xdg-open");
        command.arg(worktree_path);
        run_launch_command(&mut command, "open worktree in file manager")?;
        return Ok("opened worktree in file manager".to_owned());
    }

    #[cfg(target_os = "windows")]
    {
        let mut command = create_command("explorer");
        command.arg(worktree_path);
        run_launch_command(&mut command, "open worktree in File Explorer")?;
        return Ok("opened worktree in File Explorer".to_owned());
    }

    #[allow(unreachable_code)]
    Err("opening this worktree in a file manager is not supported on this platform".to_owned())
}

pub(crate) fn open_worktree_with_external_launcher(
    worktree_path: &Path,
    launcher: ExternalLauncher,
) -> Result<String, String> {
    match launcher.kind {
        ExternalLauncherKind::Command(command_name) => {
            let mut command = create_command(command_name);
            command.arg(worktree_path);
            run_launch_command(
                &mut command,
                &format!("open worktree with {}", launcher.label),
            )?;
        },
        ExternalLauncherKind::MacApp(app_name) => {
            let mut command = create_command("open");
            command.arg("-a").arg(app_name).arg(worktree_path);
            run_launch_command(
                &mut command,
                &format!("open worktree in {}", launcher.label),
            )?;
        },
    }

    Ok(format!("opened worktree in {}", launcher.label))
}

pub(crate) fn command_exists_on_path(command_name: &str) -> bool {
    let path_env = AUGMENTED_PATH
        .get()
        .map(|p| std::ffi::OsString::from(p.as_str()))
        .or_else(|| env::var_os("PATH"));

    let Some(path_env) = path_env else {
        return false;
    };

    env::split_paths(&path_env).any(|directory| directory.join(command_name).is_file())
}

#[cfg(target_os = "macos")]
pub(crate) fn mac_app_bundle_exists(app_name: &str) -> bool {
    let bundle = format!("{app_name}.app");
    [
        "/Applications",
        "/System/Applications",
        "/System/Applications/Utilities",
    ]
    .iter()
    .map(PathBuf::from)
    .chain(
        env::var_os("HOME")
            .map(PathBuf::from)
            .into_iter()
            .map(|home| home.join("Applications")),
    )
    .any(|base| base.join(&bundle).exists())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn mac_app_bundle_exists(_: &str) -> bool {
    false
}

pub(crate) fn detect_external_launcher(
    label: &'static str,
    icon: &'static str,
    icon_color: u32,
    mac_app: Option<&'static str>,
    command: Option<&'static str>,
) -> Option<ExternalLauncher> {
    if let Some(app_name) = mac_app
        && mac_app_bundle_exists(app_name)
    {
        return Some(ExternalLauncher {
            label,
            icon,
            icon_color,
            kind: ExternalLauncherKind::MacApp(app_name),
        });
    }

    if let Some(command_name) = command
        && command_exists_on_path(command_name)
    {
        return Some(ExternalLauncher {
            label,
            icon,
            icon_color,
            kind: ExternalLauncherKind::Command(command_name),
        });
    }

    None
}

pub(crate) fn detect_ide_launchers() -> Vec<ExternalLauncher> {
    [
        (
            "VS Code",
            "\u{e70c}",
            0x2f80ed,
            Some("Visual Studio Code"),
            Some("code"),
        ),
        (
            "VS Code Insiders",
            "\u{e70c}",
            0x4f9fff,
            Some("Visual Studio Code - Insiders"),
            Some("code-insiders"),
        ),
        ("Cursor", "Cu", 0x6ca6ff, Some("Cursor"), Some("cursor")),
        ("Zed", "Ze", 0x59a6ff, Some("Zed"), Some("zed")),
        (
            "Windsurf",
            "Ws",
            0x3cb9fc,
            Some("Windsurf"),
            Some("windsurf"),
        ),
        ("VSCodium", "Vc", 0x23a8f2, Some("VSCodium"), Some("codium")),
    ]
    .into_iter()
    .filter_map(|(label, icon, icon_color, mac_app, command)| {
        detect_external_launcher(label, icon, icon_color, mac_app, command)
    })
    .collect()
}

pub(crate) fn detect_terminal_launchers() -> Vec<ExternalLauncher> {
    [
        ("Terminal", "Tm", 0x7ecf95, Some("Terminal"), None),
        ("iTerm", "iT", 0x8ad1ec, Some("iTerm"), Some("iterm2")),
        ("iTerm2", "i2", 0x8ad1ec, Some("iTerm2"), Some("iterm2")),
        ("Ghostty", "Gh", 0xbf8cf8, Some("Ghostty"), Some("ghostty")),
        (
            "Alacritty",
            "Al",
            0xf0a168,
            Some("Alacritty"),
            Some("alacritty"),
        ),
        ("Warp", "Wp", 0x6f8dff, Some("Warp"), Some("warp")),
        ("WezTerm", "Wz", 0x6dc5ff, Some("WezTerm"), Some("wezterm")),
        ("Kitty", "Kt", 0xc89fff, Some("kitty"), Some("kitty")),
    ]
    .into_iter()
    .filter_map(|(label, icon, icon_color, mac_app, command)| {
        detect_external_launcher(label, icon, icon_color, mac_app, command)
    })
    .collect()
}

pub(crate) fn run_command_output(
    command: &mut Command,
    operation: &str,
) -> Result<std::process::Output, String> {
    command
        .output()
        .map_err(|error| format!("failed to run {operation}: {error}"))
}

pub(crate) fn command_failure_message(operation: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        return format!("{operation} failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stdout.is_empty() {
        return format!("{operation} failed: {stdout}");
    }

    match output.status.code() {
        Some(code) => format!("{operation} failed with exit code {code}"),
        None => format!("{operation} failed"),
    }
}

pub(crate) fn auto_commit_subject(changed_files: &[ChangedFile]) -> String {
    if changed_files.len() == 1 {
        let file_label = changed_files[0]
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| changed_files[0].path.display().to_string());
        return format!("chore: update {file_label}");
    }

    let has_added = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Added | ChangeKind::IntentToAdd));
    let has_removed = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Removed));
    let has_renamed = changed_files
        .iter()
        .any(|change| matches!(change.kind, ChangeKind::Renamed));
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

pub(crate) fn auto_commit_body(changed_files: &[ChangedFile]) -> String {
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

pub(crate) fn run_git_commit_for_worktree(
    worktree_path: &Path,
    changed_files: &[ChangedFile],
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

    // Stage all changes (equivalent to `git add -A`).
    let mut index = repo
        .index()
        .map_err(|error| format!("failed to read index: {error}"))?;
    index
        .add_all(["."], git2::IndexAddOption::DEFAULT, None)
        .map_err(|error| format!("failed to stage changes: {error}"))?;
    // Also remove files that were deleted from the worktree.
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

    // Check if there's actually anything to commit.
    if let Ok(head_commit) = repo.head().and_then(|h| h.peel_to_commit())
        && head_commit.tree_id() == tree_oid
    {
        return Err("nothing to commit".to_owned());
    }

    let subject = auto_commit_subject(changed_files);
    let body = auto_commit_body(changed_files);
    let message = format!("{subject}\n\n{body}");

    let sig = repo
        .signature()
        .map_err(|error| format!("failed to create signature: {error}"))?;

    let parent_commits: Vec<git2::Commit<'_>> = match repo.head().and_then(|h| h.peel_to_commit()) {
        Ok(commit) => vec![commit],
        Err(_) => vec![], // initial commit
    };
    let parents: Vec<&git2::Commit<'_>> = parent_commits.iter().collect();

    repo.commit(Some("HEAD"), &sig, &sig, &message, &tree, &parents)
        .map_err(|error| format!("failed to create commit: {error}"))?;

    Ok(format!("commit complete: {subject}"))
}

pub(crate) fn run_git_push_for_worktree(worktree_path: &Path) -> Result<String, String> {
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
        .map_err(|error| format!("failed to find remote 'origin': {error}"))?;

    // Set up SSH authentication callbacks.
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

    // Set upstream tracking branch.
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

pub(crate) fn git_branch_name_for_worktree(worktree_path: &Path) -> Result<String, String> {
    let repo = gix::open(worktree_path).map_err(|error| {
        format!(
            "failed to open repository at `{}`: {error}",
            worktree_path.display()
        )
    })?;

    let head_ref = repo
        .head_ref()
        .map_err(|error| format!("failed to read HEAD: {error}"))?;

    match head_ref {
        Some(reference) => {
            let name = reference.name().shorten().to_string();
            if name.is_empty() {
                return Err("cannot create a PR from detached HEAD".to_owned());
            }
            Ok(name)
        },
        None => Err("cannot create a PR from detached HEAD".to_owned()),
    }
}

pub(crate) fn git_has_tracking_branch(worktree_path: &Path) -> bool {
    let Ok(repo) = gix::open(worktree_path) else {
        return false;
    };
    let Ok(Some(head_ref)) = repo.head_ref() else {
        return false;
    };

    let branch_name = head_ref.name().shorten().to_string();
    let config = repo.config_snapshot();
    config
        .string(format!("branch.{branch_name}.remote"))
        .is_some()
        && config
            .string(format!("branch.{branch_name}.merge"))
            .is_some()
}

pub(crate) fn git_default_base_branch(worktree_path: &Path) -> Option<String> {
    let repo = gix::open(worktree_path).ok()?;
    let reference = repo.find_reference("refs/remotes/origin/HEAD").ok()?;
    let target = reference.target();
    let target_name = target.try_name()?.to_string();
    let short = target_name
        .strip_prefix("refs/remotes/origin/")
        .unwrap_or(&target_name);

    if short.is_empty() {
        return None;
    }

    Some(short.to_owned())
}

pub(crate) fn run_create_pr_for_worktree(
    github_service: &dyn github_service::GitHubService,
    worktree_path: &Path,
    repo_slug: Option<&str>,
    github_token: Option<&str>,
) -> Result<String, String> {
    if !git_has_tracking_branch(worktree_path) {
        return Err("push the branch before creating a PR".to_owned());
    }

    let branch = git_branch_name_for_worktree(worktree_path)?;
    let base_branch = git_default_base_branch(worktree_path).unwrap_or_else(|| "main".to_owned());

    let slug = repo_slug
        .map(str::to_owned)
        .or_else(|| github_repo_slug_for_repo(worktree_path))
        .ok_or_else(|| "could not determine GitHub repository slug".to_owned())?;

    // Read the first commit message on the branch as PR title.
    let title = branch.replace(['-', '_'], " ");

    let token = resolve_github_access_token(github_token)
        .ok_or_else(|| "GitHub authentication required, click GitHub Sign in first".to_owned())?;

    github_service.create_pull_request(&slug, &title, &branch, &base_branch, &token)
}

pub(crate) fn extract_first_url(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|token| {
        let trimmed =
            token.trim_matches(|character: char| matches!(character, '"' | '\'' | ',' | '.'));
        if trimmed.starts_with("https://") {
            Some(trimmed.to_owned())
        } else {
            None
        }
    })
}

pub(crate) fn github_repo_slug_for_repo(repo_root: &Path) -> Option<String> {
    let remote_url = git_origin_remote_url(repo_root)?;
    github_repo_slug_from_remote_url(remote_url.trim())
}

pub(crate) fn github_avatar_url_for_repo_slug(repo_slug: &str) -> Option<String> {
    let (owner, _) = repo_slug.split_once('/')?;
    Some(format!(
        "https://avatars.githubusercontent.com/{owner}?size=96"
    ))
}

pub(crate) fn github_repo_url(repo_slug: &str) -> String {
    format!("https://github.com/{repo_slug}")
}

pub(crate) fn git_origin_remote_url(repo_root: &Path) -> Option<String> {
    let repo = gix::open(repo_root).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    let url = remote.url(gix::remote::Direction::Fetch)?;
    let url_str = url.to_bstring().to_string();
    if url_str.is_empty() {
        return None;
    }
    Some(url_str)
}

pub(crate) fn github_repo_slug_from_remote_url(remote_url: &str) -> Option<String> {
    if let Some(path) = remote_url.strip_prefix("git@github.com:") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("https://github.com/") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("http://github.com/") {
        return github_repo_slug_from_path(path);
    }

    if let Some(path) = remote_url.strip_prefix("ssh://git@github.com/") {
        return github_repo_slug_from_path(path);
    }

    None
}

pub(crate) fn github_repo_slug_from_path(path: &str) -> Option<String> {
    let normalized = path.trim_end_matches('/');
    let repository_path = normalized.strip_suffix(".git").unwrap_or(normalized);
    let (owner, repository) = repository_path.split_once('/')?;
    if owner.is_empty() || repository.is_empty() {
        return None;
    }

    Some(format!("{owner}/{repository}"))
}

pub(crate) fn github_pr_number_for_worktree(
    github_service: &dyn github_service::GitHubService,
    worktree_path: &Path,
    branch: &str,
    github_token: Option<&str>,
) -> Option<u64> {
    if branch.trim().is_empty() || branch == "-" {
        return None;
    }

    github_pr_number_by_tracking_branch(github_service, worktree_path, github_token).or_else(|| {
        github_pr_number_by_head_branch(github_service, worktree_path, branch, github_token)
    })
}

pub(crate) fn should_lookup_pull_request_for_worktree(worktree: &WorktreeSummary) -> bool {
    if worktree.is_primary_checkout {
        return false;
    }

    let branch = worktree.branch.as_str();
    if branch == "-" || branch.is_empty() {
        return false;
    }

    !(branch.eq_ignore_ascii_case("main")
        || branch.eq_ignore_ascii_case("master")
        || branch.eq_ignore_ascii_case("develop")
        || branch.eq_ignore_ascii_case("dev")
        || branch.eq_ignore_ascii_case("trunk"))
}

pub(crate) fn github_pr_number_by_tracking_branch(
    github_service: &dyn github_service::GitHubService,
    worktree_path: &Path,
    github_token: Option<&str>,
) -> Option<u64> {
    let branch = git_branch_name_for_worktree(worktree_path).ok()?;
    github_pr_number_by_head_branch(github_service, worktree_path, &branch, github_token)
}

pub(crate) fn github_pr_number_by_head_branch(
    github_service: &dyn github_service::GitHubService,
    worktree_path: &Path,
    branch: &str,
    github_token: Option<&str>,
) -> Option<u64> {
    let slug = github_repo_slug_for_repo(worktree_path)?;
    let token = resolve_github_access_token(github_token)?;
    github_service.pull_request_number(&slug, branch, &token)
}

pub(crate) fn github_pr_url(repo_slug: &str, pr_number: u64) -> String {
    format!("https://github.com/{repo_slug}/pull/{pr_number}")
}

pub(crate) fn non_empty_trimmed_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub(crate) fn github_access_token_from_env() -> Option<String> {
    env::var("GITHUB_TOKEN")
        .ok()
        .and_then(|value| non_empty_trimmed_str(&value).map(str::to_owned))
}

pub(crate) fn resolve_github_access_token(saved_token: Option<&str>) -> Option<String> {
    let env_token = github_access_token_from_env();
    resolve_github_access_token_from_sources(saved_token, env_token.as_deref())
        .or_else(github_service::github_access_token_from_gh_cli)
}

pub(crate) fn resolve_github_access_token_from_sources(
    saved_token: Option<&str>,
    env_token: Option<&str>,
) -> Option<String> {
    saved_token
        .and_then(non_empty_trimmed_str)
        .map(str::to_owned)
        .or_else(|| env_token.and_then(non_empty_trimmed_str).map(str::to_owned))
}

pub(crate) fn github_oauth_client_id() -> Option<String> {
    env::var("ARBOR_GITHUB_OAUTH_CLIENT_ID")
        .ok()
        .or_else(|| env::var("GITHUB_OAUTH_CLIENT_ID").ok())
        .or_else(|| BUILT_IN_GITHUB_OAUTH_CLIENT_ID.map(str::to_owned))
        .and_then(|value| non_empty_trimmed_str(&value).map(str::to_owned))
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct GitHubDeviceCode {
    pub(crate) device_code: String,
    pub(crate) user_code: String,
    pub(crate) verification_uri: String,
    pub(crate) verification_uri_complete: Option<String>,
    pub(crate) expires_in: u64,
    pub(crate) interval: Option<u64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct GitHubDeviceCodeResponse {
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    user_code: String,
    #[serde(default)]
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct GitHubTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct GitHubAccessToken {
    pub(crate) access_token: String,
    pub(crate) token_type: Option<String>,
    pub(crate) scope: Option<String>,
}

pub(crate) fn github_oauth_http_agent() -> ureq::Agent {
    let config = ureq::config::Config::builder()
        .http_status_as_error(false)
        .build();
    ureq::Agent::new_with_config(config)
}

pub(crate) fn github_request_device_code(client_id: &str) -> Result<GitHubDeviceCode, String> {
    let response = github_oauth_http_agent()
        .post(GITHUB_OAUTH_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "Arbor")
        .send_form([("client_id", client_id), ("scope", GITHUB_OAUTH_SCOPE)])
        .map_err(|error| format!("failed to start GitHub OAuth flow: {error}"))?;

    let status = response.status();
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("failed to read GitHub OAuth response: {error}"))?;
    let payload: GitHubDeviceCodeResponse = serde_json::from_str(&body)
        .map_err(|error| format!("failed to parse GitHub OAuth response: {error}"))?;

    if !status.is_success() {
        let reason = payload
            .error
            .unwrap_or_else(|| "request_rejected".to_owned());
        let description = payload
            .error_description
            .unwrap_or_else(|| "request was rejected".to_owned());
        return Err(format!(
            "failed to start GitHub OAuth flow: {reason} ({description})"
        ));
    }

    let device_code = non_empty_trimmed_str(&payload.device_code)
        .map(str::to_owned)
        .ok_or_else(|| "GitHub OAuth response was missing device_code".to_owned())?;
    let user_code = non_empty_trimmed_str(&payload.user_code)
        .map(str::to_owned)
        .ok_or_else(|| "GitHub OAuth response was missing user_code".to_owned())?;
    let verification_uri = non_empty_trimmed_str(&payload.verification_uri)
        .map(str::to_owned)
        .ok_or_else(|| "GitHub OAuth response was missing verification_uri".to_owned())?;
    let expires_in = if payload.expires_in == 0 {
        return Err("GitHub OAuth response was missing expires_in".to_owned());
    } else {
        payload.expires_in
    };

    Ok(GitHubDeviceCode {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete: payload
            .verification_uri_complete
            .as_deref()
            .and_then(non_empty_trimmed_str)
            .map(str::to_owned),
        expires_in,
        interval: payload.interval,
    })
}

pub(crate) fn github_poll_device_access_token(
    client_id: &str,
    device_code: &GitHubDeviceCode,
) -> Result<GitHubAccessToken, String> {
    let deadline = Instant::now() + Duration::from_secs(device_code.expires_in.max(5));
    let mut poll_interval = Duration::from_secs(
        device_code
            .interval
            .unwrap_or(GITHUB_DEVICE_FLOW_POLL_MIN_INTERVAL.as_secs())
            .max(GITHUB_DEVICE_FLOW_POLL_MIN_INTERVAL.as_secs()),
    );

    loop {
        if Instant::now() >= deadline {
            return Err("GitHub authorization timed out before completion".to_owned());
        }

        std::thread::sleep(poll_interval);

        let payload = github_request_access_token(client_id, &device_code.device_code)?;
        if let Some(access_token) = payload
            .access_token
            .as_deref()
            .and_then(non_empty_trimmed_str)
            .map(str::to_owned)
        {
            return Ok(GitHubAccessToken {
                access_token,
                token_type: payload.token_type,
                scope: payload.scope,
            });
        }

        match payload.error.as_deref() {
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                poll_interval += Duration::from_secs(5);
                continue;
            },
            Some("access_denied") => {
                return Err("GitHub authorization was denied".to_owned());
            },
            Some("expired_token") => {
                return Err("GitHub authorization code expired".to_owned());
            },
            Some(other) => {
                let description = payload
                    .error_description
                    .as_deref()
                    .and_then(non_empty_trimmed_str)
                    .unwrap_or("request failed");
                return Err(format!("GitHub OAuth failed: {other} ({description})"));
            },
            None => {
                return Err("GitHub OAuth response was missing an access token".to_owned());
            },
        }
    }
}

pub(crate) fn github_request_access_token(
    client_id: &str,
    device_code: &str,
) -> Result<GitHubTokenResponse, String> {
    let response = github_oauth_http_agent()
        .post(GITHUB_OAUTH_ACCESS_TOKEN_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "Arbor")
        .send_form([
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .map_err(|error| format!("failed to poll GitHub OAuth status: {error}"))?;

    let status = response.status();
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|error| format!("failed to read GitHub OAuth token response: {error}"))?;
    let payload: GitHubTokenResponse = serde_json::from_str(&body)
        .map_err(|error| format!("failed to parse GitHub OAuth token response: {error}"))?;

    if status.is_success() || payload.error.is_some() || payload.access_token.is_some() {
        return Ok(payload);
    }

    Err("GitHub OAuth token request failed".to_owned())
}

pub(crate) fn extract_repo_name_from_url(url: &str) -> String {
    let url = url.trim();
    // Strip trailing .git
    let url = url.strip_suffix(".git").unwrap_or(url);
    // Strip trailing /
    let url = url.strip_suffix('/').unwrap_or(url);
    // Get the last path component
    if let Some(pos) = url.rfind('/') {
        url[pos + 1..].to_owned()
    } else if let Some(pos) = url.rfind(':') {
        // SSH-style: git@github.com:user/repo
        let after_colon = &url[pos + 1..];
        if let Some(slash_pos) = after_colon.rfind('/') {
            after_colon[slash_pos + 1..].to_owned()
        } else {
            after_colon.to_owned()
        }
    } else {
        String::new()
    }
}

pub(crate) fn repository_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn should_seed_repo_root_from_cwd(
    store_file_exists: bool,
    loaded_roots_were_empty: bool,
) -> bool {
    // Seed from CWD on first run (no store file), or when there are existing
    // saved roots and CWD is simply not listed yet. If the store exists and is
    // explicitly empty, preserve that empty state across restarts.
    !store_file_exists || !loaded_roots_were_empty
}

pub(crate) fn short_branch(value: &str) -> String {
    worktree::short_branch(value)
}

pub(crate) fn expand_home_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("repository path cannot be empty".to_owned());
    }

    if trimmed == "~" {
        return user_home_dir();
    }

    if let Some(suffix) = trimmed.strip_prefix("~/") {
        return user_home_dir().map(|home| home.join(suffix));
    }

    Ok(PathBuf::from(trimmed))
}

pub(crate) fn user_home_dir() -> Result<PathBuf, String> {
    env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "HOME environment variable is not set".to_owned())
}

pub(crate) fn sanitize_worktree_name(value: &str) -> String {
    let mut sanitized = String::new();
    let mut previous_dash = false;

    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            sanitized.push(character.to_ascii_lowercase());
            previous_dash = false;
            continue;
        }

        if character == '-' || character == '_' || character == '.' {
            sanitized.push(character);
            previous_dash = false;
            continue;
        }

        if !previous_dash && !sanitized.is_empty() {
            sanitized.push('-');
            previous_dash = true;
        }
    }

    while sanitized.ends_with('-') {
        let _ = sanitized.pop();
    }

    sanitized
}

pub(crate) fn derive_branch_name(worktree_name: &str) -> String {
    let sanitized = sanitize_worktree_name(worktree_name);
    if sanitized.is_empty() {
        "worktree".to_owned()
    } else {
        sanitized
    }
}

pub(crate) fn build_managed_worktree_path(
    repo_name: &str,
    worktree_name: &str,
) -> Result<PathBuf, String> {
    let home_dir = user_home_dir()?;
    Ok(home_dir
        .join(".arbor")
        .join("worktrees")
        .join(repo_name)
        .join(worktree_name))
}

pub(crate) fn preview_managed_worktree_path(
    repository_path: &str,
    worktree_name: &str,
) -> Result<String, String> {
    let repository_path = expand_home_path(repository_path)?;
    let repository_name = repository_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "repository name cannot be determined".to_owned())?;
    let sanitized_worktree = sanitize_worktree_name(worktree_name);
    if sanitized_worktree.is_empty() {
        return Err("invalid worktree name".to_owned());
    }

    let path = build_managed_worktree_path(repository_name, &sanitized_worktree)?;
    Ok(path.display().to_string())
}

pub(crate) fn create_managed_worktree(
    repository_path_input: String,
    worktree_name_input: String,
    checkout_kind: CheckoutKind,
) -> Result<CreatedWorktree, String> {
    let repository_path = expand_home_path(&repository_path_input)?;
    if !repository_path.exists() {
        return Err(format!(
            "repository path does not exist: {}",
            repository_path.display()
        ));
    }

    let repository_root = worktree::repo_root(&repository_path)
        .map_err(|error| format!("failed to resolve repository root: {error}"))?;
    let repository_name = repository_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "repository root has no terminal directory name".to_owned())?;

    let sanitized_worktree_name = sanitize_worktree_name(&worktree_name_input);
    if sanitized_worktree_name.is_empty() {
        return Err("worktree name contains no usable characters".to_owned());
    }

    let branch_name = derive_branch_name(&worktree_name_input);
    let worktree_path = build_managed_worktree_path(repository_name, &sanitized_worktree_name)?;
    if worktree_path.exists() {
        return Err(format!(
            "worktree path already exists: {}",
            worktree_path.display()
        ));
    }

    let Some(parent_directory) = worktree_path.parent() else {
        return Err("invalid worktree path".to_owned());
    };
    fs::create_dir_all(parent_directory).map_err(|error| {
        format!(
            "failed to create worktree parent directory `{}`: {error}",
            parent_directory.display()
        )
    })?;

    match checkout_kind {
        CheckoutKind::LinkedWorktree => worktree::add(
            &repository_root,
            &worktree_path,
            worktree::AddWorktreeOptions {
                branch: Some(&branch_name),
                detach: false,
                force: false,
            },
        )
        .map_err(|error| format!("failed to create worktree: {error}"))?,
        CheckoutKind::DiscreteClone => {
            create_discrete_clone(&repository_root, &worktree_path, &branch_name)?
        },
    }

    Ok(CreatedWorktree {
        worktree_name: sanitized_worktree_name,
        branch_name,
        worktree_path,
        checkout_kind,
        source_repo_root: repository_root,
    })
}

pub(crate) fn create_discrete_clone(
    source_repo_root: &Path,
    checkout_path: &Path,
    branch_name: &str,
) -> Result<(), String> {
    let clone_source = source_repo_root
        .to_str()
        .ok_or_else(|| "repository path contains invalid UTF-8".to_owned())?;
    let checkout_target = checkout_path
        .to_str()
        .ok_or_else(|| "checkout path contains invalid UTF-8".to_owned())?;

    let source_repo = git2::Repository::open(source_repo_root).map_err(|error| {
        format!(
            "failed to open source repository `{}`: {error}",
            source_repo_root.display()
        )
    })?;
    let origin_url = source_repo
        .find_remote("origin")
        .ok()
        .and_then(|remote| remote.url().map(str::to_owned));

    let cloned_repo = git2::Repository::clone(clone_source, checkout_target).map_err(|error| {
        format!(
            "failed to clone `{}` into `{}`: {error}",
            source_repo_root.display(),
            checkout_path.display()
        )
    })?;

    if let Some(origin_url) = origin_url.as_deref() {
        let _ = cloned_repo.remote_set_url("origin", origin_url);
    }

    let head_commit = cloned_repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| format!("failed to resolve cloned HEAD: {error}"))?;
    cloned_repo
        .branch(branch_name, &head_commit, false)
        .map_err(|error| format!("failed to create branch `{branch_name}`: {error}"))?;

    let branch_ref = format!("refs/heads/{branch_name}");
    cloned_repo
        .set_head(&branch_ref)
        .map_err(|error| format!("failed to set HEAD to `{branch_name}`: {error}"))?;

    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force();
    cloned_repo
        .checkout_head(Some(&mut checkout))
        .map_err(|error| format!("failed to check out `{branch_name}`: {error}"))?;

    Ok(())
}

pub(crate) fn styled_lines_for_session(
    session: &TerminalSession,
    theme: ThemePalette,
    show_cursor: bool,
    selection: Option<&TerminalSelection>,
    ime_marked_text: Option<&str>,
) -> Vec<TerminalStyledLine> {
    let mut lines = if !session.styled_output.is_empty() {
        session.styled_output.clone()
    } else {
        plain_lines_to_styled(lines_for_display(&session.output), theme)
    };

    for line in &mut lines {
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        } else if line.runs.is_empty() && !line.cells.is_empty() {
            line.runs = runs_from_cells(&line.cells);
        }

        let mut changed = false;
        for cell in &mut line.cells {
            if cell.bg == EMBEDDED_TERMINAL_DEFAULT_BG {
                cell.bg = theme.terminal_bg;
                changed = true;
            }
            if cell.fg == EMBEDDED_TERMINAL_DEFAULT_FG {
                cell.fg = theme.text_primary;
                changed = true;
            }
        }

        if changed {
            line.runs = runs_from_cells(&line.cells);
        }
    }

    if show_cursor
        && session.state == TerminalState::Running
        && let Some(cursor) = session.cursor
    {
        if let Some(marked) = ime_marked_text {
            apply_ime_marked_text_to_lines(&mut lines, cursor, marked, theme);
        } else {
            apply_cursor_to_lines(&mut lines, cursor, theme);
        }
    }

    if let Some(selection) = selection.filter(|selection| selection.session_id == session.id) {
        apply_selection_to_lines(&mut lines, selection, theme);
    }

    lines
}

pub(crate) fn apply_cursor_to_lines(
    lines: &mut Vec<TerminalStyledLine>,
    cursor: TerminalCursor,
    theme: ThemePalette,
) {
    while lines.len() <= cursor.line {
        lines.push(TerminalStyledLine {
            cells: Vec::new(),
            runs: Vec::new(),
        });
    }

    if let Some(line) = lines.get_mut(cursor.line) {
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        }

        let insert_index = line
            .cells
            .iter()
            .position(|cell| cell.column >= cursor.column)
            .unwrap_or(line.cells.len());

        if line
            .cells
            .get(insert_index)
            .is_none_or(|cell| cell.column != cursor.column)
        {
            line.cells.insert(insert_index, TerminalStyledCell {
                column: cursor.column,
                text: " ".to_owned(),
                fg: theme.text_primary,
                bg: theme.terminal_bg,
            });
        }

        if let Some(cell) = line.cells.get_mut(insert_index) {
            if cell.text.is_empty() {
                cell.text = " ".to_owned();
            }

            if cell.text.chars().all(|character| character == ' ') {
                cell.fg = theme.text_primary;
            }
            cell.bg = theme.terminal_cursor;
        }

        line.runs = runs_from_cells(&line.cells);
    }
}

pub(crate) fn apply_ime_marked_text_to_lines(
    lines: &mut [TerminalStyledLine],
    cursor: TerminalCursor,
    marked_text: &str,
    theme: ThemePalette,
) {
    if lines.len() <= cursor.line {
        return;
    }

    let Some(line) = lines.get_mut(cursor.line) else {
        return;
    };

    if line.cells.is_empty() && !line.runs.is_empty() {
        line.cells = cells_from_runs(&line.runs);
    }

    let insert_index = line
        .cells
        .iter()
        .position(|cell| cell.column >= cursor.column)
        .unwrap_or(line.cells.len());

    // Insert marked text cell at cursor position with cursor highlight
    if line
        .cells
        .get(insert_index)
        .is_some_and(|cell| cell.column == cursor.column)
    {
        line.cells[insert_index] = TerminalStyledCell {
            column: cursor.column,
            text: marked_text.to_owned(),
            fg: theme.text_primary,
            bg: theme.terminal_cursor,
        };
    } else {
        line.cells.insert(insert_index, TerminalStyledCell {
            column: cursor.column,
            text: marked_text.to_owned(),
            fg: theme.text_primary,
            bg: theme.terminal_cursor,
        });
    }

    line.runs = runs_from_cells(&line.cells);
}

pub(crate) fn apply_selection_to_lines(
    lines: &mut Vec<TerminalStyledLine>,
    selection: &TerminalSelection,
    theme: ThemePalette,
) {
    let Some((start, end)) = normalized_terminal_selection(selection) else {
        return;
    };

    while lines.len() <= end.line {
        lines.push(TerminalStyledLine {
            cells: Vec::new(),
            runs: Vec::new(),
        });
    }

    for line_index in start.line..=end.line {
        let Some(line) = lines.get_mut(line_index) else {
            continue;
        };
        if line.cells.is_empty() && !line.runs.is_empty() {
            line.cells = cells_from_runs(&line.runs);
        }

        let line_start = if line_index == start.line {
            start.column
        } else {
            0
        };
        let line_end_exclusive = if line_index == end.line {
            end.column
        } else {
            usize::MAX
        };
        if line_end_exclusive <= line_start {
            continue;
        }

        let mut changed = false;
        for cell in &mut line.cells {
            if cell.column >= line_start && cell.column < line_end_exclusive {
                cell.fg = theme.terminal_selection_fg;
                cell.bg = theme.terminal_selection_bg;
                changed = true;
            }
        }

        if changed {
            line.runs = runs_from_cells(&line.cells);
        }
    }
}

pub(crate) fn normalized_terminal_selection(
    selection: &TerminalSelection,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let (start, end) = if selection.anchor.line < selection.head.line
        || (selection.anchor.line == selection.head.line
            && selection.anchor.column <= selection.head.column)
    {
        (selection.anchor, selection.head)
    } else {
        (selection.head, selection.anchor)
    };

    if start == end {
        return None;
    }

    Some((start, end))
}

pub(crate) fn cells_from_runs(runs: &[TerminalStyledRun]) -> Vec<TerminalStyledCell> {
    let mut cells = Vec::new();
    let mut column = 0_usize;
    for run in runs {
        for character in run.text.chars() {
            cells.push(TerminalStyledCell {
                column,
                text: character.to_string(),
                fg: run.fg,
                bg: run.bg,
            });
            column = column.saturating_add(1);
        }
    }
    cells
}

pub(crate) fn runs_from_cells(cells: &[TerminalStyledCell]) -> Vec<TerminalStyledRun> {
    let mut runs = Vec::new();
    let mut current_fg = None;
    let mut current_bg = None;
    let mut current_text = String::new();
    let mut next_expected_column: Option<usize> = None;
    let mut current_contains_complex_cell = false;
    let mut current_contains_decorative_cell = false;

    for cell in cells {
        let cell_is_complex = cell.text.chars().count() != 1;
        let cell_is_powerline = cell
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
            && cell.text.chars().count() == 1;
        let style_changed = current_fg != Some(cell.fg) || current_bg != Some(cell.bg);
        let gap_breaks_run = next_expected_column != Some(cell.column);
        let complex_breaks_run = current_contains_complex_cell || cell_is_complex;
        let decorative_breaks_run = current_contains_decorative_cell || cell_is_powerline;
        if style_changed || gap_breaks_run || complex_breaks_run || decorative_breaks_run {
            if let (Some(fg), Some(bg)) = (current_fg.take(), current_bg.take())
                && !current_text.is_empty()
            {
                runs.push(TerminalStyledRun {
                    text: std::mem::take(&mut current_text),
                    fg,
                    bg,
                });
            }

            current_fg = Some(cell.fg);
            current_bg = Some(cell.bg);
            current_contains_complex_cell = cell_is_complex;
            current_contains_decorative_cell = cell_is_powerline;
        }

        current_text.push_str(&cell.text);
        next_expected_column = Some(cell.column.saturating_add(1));
        current_contains_decorative_cell |= cell_is_powerline;
    }

    if let (Some(fg), Some(bg)) = (current_fg, current_bg)
        && !current_text.is_empty()
    {
        runs.push(TerminalStyledRun {
            text: current_text,
            fg,
            bg,
        });
    }

    runs
}

#[derive(Clone)]
pub(crate) struct PositionedTerminalRun {
    pub(crate) text: String,
    pub(crate) fg: u32,
    pub(crate) bg: u32,
    pub(crate) start_column: usize,
    pub(crate) cell_count: usize,
    pub(crate) force_cell_width: bool,
}

pub(crate) fn positioned_runs_from_cells(
    cells: &[TerminalStyledCell],
) -> Vec<PositionedTerminalRun> {
    let mut runs = Vec::new();
    let mut current_fg: Option<u32> = None;
    let mut current_bg: Option<u32> = None;
    let mut current_start_column = 0_usize;
    let mut current_text = String::new();
    let mut next_expected_column: Option<usize> = None;
    let mut current_contains_complex_cell = false;
    let mut current_contains_decorative_cell = false;
    let mut current_cell_count = 0_usize;

    for cell in cells {
        let cell_is_complex = cell.text.chars().count() != 1;
        let cell_is_powerline = cell
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
            && cell.text.chars().count() == 1;
        let style_changed = current_fg != Some(cell.fg) || current_bg != Some(cell.bg);
        let gap_breaks_run = next_expected_column != Some(cell.column);
        let complex_breaks_run = current_contains_complex_cell || cell_is_complex;
        let decorative_breaks_run = current_contains_decorative_cell || cell_is_powerline;
        if style_changed || gap_breaks_run || complex_breaks_run || decorative_breaks_run {
            if let (Some(fg), Some(bg)) = (current_fg.take(), current_bg.take())
                && !current_text.is_empty()
            {
                runs.push(PositionedTerminalRun {
                    text: std::mem::take(&mut current_text),
                    fg,
                    bg,
                    start_column: current_start_column,
                    cell_count: current_cell_count,
                    force_cell_width: !current_contains_complex_cell
                        && !current_contains_decorative_cell,
                });
            }

            current_fg = Some(cell.fg);
            current_bg = Some(cell.bg);
            current_start_column = cell.column;
            current_contains_complex_cell = cell_is_complex;
            current_contains_decorative_cell = cell_is_powerline;
            current_cell_count = 0;
        }

        current_text.push_str(&cell.text);
        current_cell_count = current_cell_count.saturating_add(1);
        current_contains_complex_cell |= cell_is_complex;
        current_contains_decorative_cell |= cell_is_powerline;
        next_expected_column = Some(cell.column.saturating_add(1));
    }

    if let (Some(fg), Some(bg)) = (current_fg, current_bg)
        && !current_text.is_empty()
    {
        runs.push(PositionedTerminalRun {
            text: current_text,
            fg,
            bg,
            start_column: current_start_column,
            cell_count: current_cell_count,
            force_cell_width: !current_contains_complex_cell && !current_contains_decorative_cell,
        });
    }

    runs
}

pub(crate) fn is_terminal_powerline_character(ch: char) -> bool {
    matches!(ch as u32, 0xE0B0..=0xE0D7)
}

pub(crate) fn plain_lines_to_styled(
    lines: Vec<String>,
    theme: ThemePalette,
) -> Vec<TerminalStyledLine> {
    lines
        .into_iter()
        .map(|line| {
            let cells: Vec<TerminalStyledCell> = line
                .chars()
                .enumerate()
                .map(|(column, character)| TerminalStyledCell {
                    column,
                    text: character.to_string(),
                    fg: theme.text_primary,
                    bg: theme.terminal_bg,
                })
                .collect();

            let runs = if line.is_empty() {
                Vec::new()
            } else {
                vec![TerminalStyledRun {
                    text: line,
                    fg: theme.text_primary,
                    bg: theme.terminal_bg,
                }]
            };

            TerminalStyledLine { cells, runs }
        })
        .collect()
}

pub(crate) fn render_terminal_line(
    line: TerminalStyledLine,
    theme: ThemePalette,
    cell_width: f32,
    line_height: f32,
    mono_font: gpui::Font,
) -> Div {
    let cells = if line.cells.is_empty() {
        cells_from_runs(&line.runs)
    } else {
        line.cells
    };

    if cells.is_empty() {
        return div()
            .flex_none()
            .w_full()
            .min_w_0()
            .h(px(line_height))
            .overflow_x_hidden()
            .whitespace_nowrap()
            .font(mono_font)
            .text_size(px(TERMINAL_FONT_SIZE_PX))
            .line_height(px(line_height))
            .bg(rgb(theme.terminal_bg))
            .text_color(rgb(theme.text_primary))
            .child(" ");
    }

    let line_height = px(line_height);
    let font_size = px(TERMINAL_FONT_SIZE_PX);
    let positioned_runs = positioned_runs_from_cells(&cells);

    div()
        .flex_none()
        .w_full()
        .min_w_0()
        .h(line_height)
        .overflow_hidden()
        .bg(rgb(theme.terminal_bg))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let scale_factor = window.scale_factor();
                    for run in &positioned_runs {
                        if run.text.is_empty() {
                            continue;
                        }

                        if run.cell_count > 0 {
                            let start_x = snap_pixels_floor(
                                bounds.origin.x + px(run.start_column as f32 * cell_width),
                                scale_factor,
                            );
                            let end_x = snap_pixels_ceil(
                                bounds.origin.x
                                    + px((run.start_column + run.cell_count) as f32 * cell_width),
                                scale_factor,
                            );
                            let background_origin = point(start_x, bounds.origin.y);
                            let background_size = size((end_x - start_x).max(px(0.)), line_height);
                            window.paint_quad(fill(
                                Bounds::new(background_origin, background_size),
                                rgb(run.bg),
                            ));
                        }

                        let is_powerline = should_force_powerline(run);
                        let force_cell_width = run.force_cell_width || is_powerline;
                        let force_width = if force_cell_width {
                            Some(px(cell_width))
                        } else {
                            None
                        };

                        let shaped_line = window.text_system().shape_line(
                            run.text.clone().into(),
                            font_size,
                            &[TextRun {
                                len: run.text.len(),
                                font: mono_font.clone(),
                                color: rgb(run.fg).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            force_width,
                        );

                        let run_origin = bounds.origin.x + px(run.start_column as f32 * cell_width);
                        let run_x = if is_powerline || force_cell_width {
                            run_origin
                        } else {
                            run_origin.floor()
                        };

                        let _ = shaped_line.paint(
                            point(run_x, bounds.origin.y),
                            line_height,
                            window,
                            cx,
                        );
                    }
                },
            )
            .size_full(),
        )
}

pub(crate) fn should_force_powerline(run: &PositionedTerminalRun) -> bool {
    run.text.chars().count() == 1
        && run
            .text
            .chars()
            .next()
            .is_some_and(is_terminal_powerline_character)
}

pub(crate) fn snap_pixels_floor(value: Pixels, scale_factor: f32) -> Pixels {
    if !(scale_factor.is_finite() && scale_factor > 0.) {
        return value.floor();
    }

    let scaled = value.to_f64() as f32 * scale_factor;
    px(scaled.floor() / scale_factor)
}

pub(crate) fn snap_pixels_ceil(value: Pixels, scale_factor: f32) -> Pixels {
    if !(scale_factor.is_finite() && scale_factor > 0.) {
        return value.ceil();
    }

    let scaled = value.to_f64() as f32 * scale_factor;
    px(scaled.ceil() / scale_factor)
}

pub(crate) fn lines_for_display(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec!["<no output yet>".to_owned()];
    }

    text.lines().map(ToOwned::to_owned).collect()
}

pub(crate) fn terminal_display_lines(session: &TerminalSession) -> Vec<String> {
    if !session.styled_output.is_empty() {
        return session
            .styled_output
            .iter()
            .map(styled_line_to_string)
            .collect();
    }

    if session.output.is_empty() {
        return vec![String::new()];
    }

    session.output.lines().map(ToOwned::to_owned).collect()
}

pub(crate) fn styled_line_to_string(line: &TerminalStyledLine) -> String {
    let mut cells = if line.cells.is_empty() {
        cells_from_runs(&line.runs)
    } else {
        line.cells.clone()
    };
    if cells.is_empty() {
        return String::new();
    }

    cells.sort_by_key(|cell| cell.column);
    let mut output = String::new();
    let mut current_column = 0_usize;

    for cell in cells {
        while current_column < cell.column {
            output.push(' ');
            current_column = current_column.saturating_add(1);
        }
        output.push_str(&cell.text);
        current_column = current_column.saturating_add(1);
    }

    output
}

pub(crate) fn terminal_grid_position_from_pointer(
    position: gpui::Point<Pixels>,
    bounds: Bounds<Pixels>,
    scroll_offset: gpui::Point<Pixels>,
    line_height: f32,
    cell_width: f32,
    line_count: usize,
) -> Option<TerminalGridPosition> {
    if line_height <= 0. || cell_width <= 0. || line_count == 0 {
        return None;
    }

    let local_x = f32::from(position.x - bounds.left()).max(0.);
    let local_y = f32::from(position.y - bounds.top()).max(0.);
    let content_y = (local_y - f32::from(scroll_offset.y)).max(0.);

    let max_line = line_count.saturating_sub(1);
    let line = ((content_y / line_height).floor() as usize).min(max_line);
    let column = (local_x / cell_width).floor().max(0.) as usize;

    Some(TerminalGridPosition { line, column })
}

pub(crate) fn terminal_token_bounds(
    lines: &[String],
    point: TerminalGridPosition,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let line = lines.get(point.line)?;
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let index = point.column.min(chars.len().saturating_sub(1));
    if chars
        .get(index)
        .is_none_or(|character| character.is_whitespace())
    {
        return None;
    }

    let mut start = index;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }

    let mut end = index.saturating_add(1);
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }

    Some((
        TerminalGridPosition {
            line: point.line,
            column: start,
        },
        TerminalGridPosition {
            line: point.line,
            column: end,
        },
    ))
}

pub(crate) fn terminal_line_bounds(
    lines: &[String],
    point: TerminalGridPosition,
) -> Option<(TerminalGridPosition, TerminalGridPosition)> {
    let line = lines.get(point.line)?;
    let width = line.chars().count();
    if width == 0 {
        return None;
    }

    Some((
        TerminalGridPosition {
            line: point.line,
            column: 0,
        },
        TerminalGridPosition {
            line: point.line,
            column: width,
        },
    ))
}

pub(crate) fn terminal_selection_text(lines: &[String], selection: &TerminalSelection) -> String {
    let Some((start, end)) = normalized_terminal_selection(selection) else {
        return String::new();
    };

    let mut output = String::new();
    for line_index in start.line..=end.line {
        let line = lines.get(line_index).map_or("", String::as_str);
        let chars: Vec<char> = line.chars().collect();

        let from = if line_index == start.line {
            start.column.min(chars.len())
        } else {
            0
        };
        let to = if line_index == end.line {
            end.column.min(chars.len())
        } else {
            chars.len()
        };

        if from < to {
            output.extend(chars[from..to].iter());
        }

        if line_index != end.line {
            output.push('\n');
        }
    }

    output
}

pub(crate) fn trim_to_last_lines(text: String, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text;
    }

    let mut trimmed = String::new();
    let start = lines.len().saturating_sub(max_lines);
    for line in lines.iter().skip(start) {
        trimmed.push_str(line);
        trimmed.push('\n');
    }
    trimmed
}

pub(crate) fn terminal_scroll_is_near_bottom(scroll_handle: &ScrollHandle) -> bool {
    let max_offset = scroll_handle.max_offset();
    if max_offset.height <= px(0.) {
        return true;
    }

    let offset = scroll_handle.offset();
    let distance_from_bottom = (offset.y + max_offset.height).abs();
    distance_from_bottom <= px(6.)
}

pub(crate) fn terminal_grid_size_from_scroll_handle(
    scroll_handle: &ScrollHandle,
    cx: &App,
) -> Option<(u16, u16, u16, u16)> {
    let bounds = scroll_handle.bounds();
    let width = (bounds.size.width.to_f64() as f32 - TERMINAL_SCROLLBAR_WIDTH_PX).max(1.);
    let height = bounds.size.height.to_f64() as f32;
    let cell_width = terminal_cell_width_px(cx);
    let line_height = terminal_line_height_px(cx);
    let (rows, cols) = terminal_grid_size_for_viewport(width, height, cell_width, line_height)?;
    let pixel_width = width.floor().clamp(1., f32::from(u16::MAX)) as u16;
    let pixel_height = height.floor().clamp(1., f32::from(u16::MAX)) as u16;
    Some((rows, cols, pixel_width, pixel_height))
}

pub(crate) fn terminal_cell_width_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);

    text_system
        .advance(font_id, px(TERMINAL_FONT_SIZE_PX), 'm')
        .map(|size| size.width.to_f64() as f32)
        .ok()
        .filter(|width| width.is_finite() && *width > 0.)
        .unwrap_or(TERMINAL_CELL_WIDTH_PX)
}

pub(crate) fn diff_cell_width_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);
    let fallback = (TERMINAL_CELL_WIDTH_PX * (DIFF_FONT_SIZE_PX / TERMINAL_FONT_SIZE_PX)).max(1.);

    text_system
        .advance(font_id, px(DIFF_FONT_SIZE_PX), 'm')
        .map(|size| size.width.to_f64() as f32)
        .ok()
        .filter(|width| width.is_finite() && *width > 0.)
        .unwrap_or(fallback)
}

pub(crate) fn terminal_line_height_px(cx: &App) -> f32 {
    let text_system = cx.text_system();
    let mono_font = terminal_mono_font(cx);
    let font_id = text_system.resolve_font(&mono_font);
    let font_size = px(TERMINAL_FONT_SIZE_PX);

    let ascent = text_system.ascent(font_id, font_size).to_f64() as f32;
    let descent = text_system.descent(font_id, font_size).to_f64() as f32;
    let measured_height = if descent.is_sign_negative() {
        ascent - descent
    } else {
        ascent + descent
    };

    if measured_height.is_finite() && measured_height > 0. {
        return measured_height.ceil().max(TERMINAL_FONT_SIZE_PX).max(1.);
    }

    TERMINAL_CELL_HEIGHT_PX
}

pub(crate) fn terminal_grid_size_for_viewport(
    width: f32,
    height: f32,
    cell_width: f32,
    cell_height: f32,
) -> Option<(u16, u16)> {
    if width <= 0. || height <= 0. || cell_width <= 0. || cell_height <= 0. {
        return None;
    }

    let cols = (width / cell_width).floor() as i32;
    let rows = (height / cell_height).floor() as i32;
    if cols <= 0 || rows <= 0 {
        return None;
    }

    let cols = cols.clamp(2, i32::from(u16::MAX)) as u16;
    let rows = rows.clamp(1, i32::from(u16::MAX)) as u16;
    Some((rows, cols))
}

pub(crate) fn should_auto_follow_terminal_output(changed: bool, was_near_bottom: bool) -> bool {
    changed && was_near_bottom
}

pub(crate) fn parse_terminal_backend_kind(
    terminal_backend: Option<&str>,
) -> Result<TerminalBackendKind, String> {
    let Some(value) = terminal_backend
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(TerminalBackendKind::Embedded);
    };

    match value.to_ascii_lowercase().as_str() {
        "embedded" => Ok(TerminalBackendKind::Embedded),
        "alacritty" => Ok(TerminalBackendKind::Alacritty),
        "ghostty" => Ok(TerminalBackendKind::Ghostty),
        _ => Err(format!(
            "invalid terminal_backend `{value}` in config, expected embedded/alacritty/ghostty"
        )),
    }
}

pub(crate) fn parse_theme_kind(theme: Option<&str>) -> Result<ThemeKind, String> {
    let Some(value) = theme.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(ThemeKind::One);
    };

    match value.to_ascii_lowercase().as_str() {
        "one-dark" | "onedark" => Ok(ThemeKind::One),
        "ayu-dark" | "ayu" => Ok(ThemeKind::Ayu),
        "gruvbox-dark" | "gruvbox" => Ok(ThemeKind::Gruvbox),
        "dracula" => Ok(ThemeKind::Dracula),
        "solarized-light" | "solarized" => Ok(ThemeKind::SolarizedLight),
        "everforest-dark" | "everforest" => Ok(ThemeKind::Everforest),
        "catppuccin" => Ok(ThemeKind::Catppuccin),
        "catppuccin-latte" => Ok(ThemeKind::CatppuccinLatte),
        "ethereal" => Ok(ThemeKind::Ethereal),
        "flexoki-light" | "flexoki" => Ok(ThemeKind::FlexokiLight),
        "hackerman" => Ok(ThemeKind::Hackerman),
        "kanagawa" => Ok(ThemeKind::Kanagawa),
        "matte-black" | "matteblack" => Ok(ThemeKind::MatteBlack),
        "miasma" => Ok(ThemeKind::Miasma),
        "nord" => Ok(ThemeKind::Nord),
        "osaka-jade" | "osakajade" => Ok(ThemeKind::OsakaJade),
        "ristretto" => Ok(ThemeKind::Ristretto),
        "rose-pine" | "rosepine" => Ok(ThemeKind::RosePine),
        "tokyo-night" | "tokyonight" => Ok(ThemeKind::TokyoNight),
        "vantablack" => Ok(ThemeKind::Vantablack),
        "white" => Ok(ThemeKind::White),
        "retrobox-classic" | "retrobox" => Ok(ThemeKind::RetroboxClassic),
        "tokyonight-day" | "tokionight-day" => Ok(ThemeKind::TokyoNightDay),
        "tokyonight-classic" | "tokionight-classic" => Ok(ThemeKind::TokyoNightClassic),
        "zellner" => Ok(ThemeKind::Zellner),
        _ => Err(format!(
            "invalid theme `{value}` in config, expected one-dark/ayu-dark/gruvbox-dark/dracula/solarized-light/everforest-dark/catppuccin/catppuccin-latte/ethereal/flexoki-light/hackerman/kanagawa/matte-black/miasma/nord/osaka-jade/ristretto/rose-pine/tokyo-night/vantablack/white/retrobox-classic/tokyonight-day/tokyonight-classic/zellner"
        )),
    }
}

pub(crate) fn open_arbor_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1460.), px(900.)), cx);
    if let Err(error) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(1180.), px(760.))),
            app_id: Some("so.pen.arbor".to_owned()),
            titlebar: Some(TitlebarOptions {
                title: Some("Arbor".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(9.), px(9.))),
            }),
            window_decorations: Some(WindowDecorations::Client),
            ..Default::default()
        },
        |_, cx| {
            cx.new(|cx| {
                ArborWindow::load_with_daemon_store::<daemon::JsonDaemonSessionStore>(
                    ui_state_store::UiState::default(),
                    log_layer::LogBuffer::new(),
                    cx,
                )
            })
        },
    ) {
        tracing::error!(%error, "failed to open Arbor window");
    }
}

pub(crate) fn new_window(_: &NewWindow, cx: &mut App) {
    open_arbor_window(cx);
}

pub(crate) fn install_app_menu_and_keys(cx: &mut App) {
    cx.on_action(new_window);
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-q", RequestQuit, None),
        KeyBinding::new("cmd-t", SpawnTerminal, None),
        KeyBinding::new("cmd-w", CloseActiveTerminal, None),
        KeyBinding::new("cmd-shift-o", OpenAddRepository, None),
        KeyBinding::new("cmd-shift-n", OpenCreateWorktree, None),
        KeyBinding::new("cmd-shift-r", RefreshWorktrees, None),
        KeyBinding::new("cmd-alt-r", RefreshChanges, None),
        KeyBinding::new("cmd-1", UseEmbeddedBackend, None),
        KeyBinding::new("cmd-2", UseAlacrittyBackend, None),
        KeyBinding::new("cmd-3", UseGhosttyBackend, None),
        KeyBinding::new("cmd-\\", ToggleLeftPane, None),
        KeyBinding::new("cmd-[", NavigateWorktreeBack, None),
        KeyBinding::new("cmd-]", NavigateWorktreeForward, None),
        KeyBinding::new("cmd-shift-l", ViewLogs, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
    ]);
    cx.set_menus(build_app_menus(&[]));
}

pub(crate) fn build_app_menus(discovered_daemons: &[mdns_browser::DiscoveredDaemon]) -> Vec<Menu> {
    let mut host_items = vec![
        MenuItem::action("Connect to Host...", ConnectToHost),
        MenuItem::action("Manage Hosts...", OpenManageHosts),
    ];

    if !discovered_daemons.is_empty() {
        host_items.push(MenuItem::separator());
        for (index, daemon) in discovered_daemons.iter().enumerate() {
            let addr = daemon
                .addresses
                .first()
                .cloned()
                .unwrap_or_else(|| daemon.host.clone());
            let label = format!("{} ({addr}:{})", daemon.display_name(), daemon.port);
            host_items.push(MenuItem::action(label, ConnectToLanDaemon { index }));
        }
    }

    vec![
        Menu {
            name: "Arbor".into(),
            items: vec![
                MenuItem::action("About Arbor", ShowAbout),
                MenuItem::action("Settings...", OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Arbor", ImmediateQuit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Window", NewWindow),
                MenuItem::separator(),
                MenuItem::action("Add Repository...", OpenAddRepository),
                MenuItem::separator(),
                MenuItem::action("New Terminal Tab", SpawnTerminal),
                MenuItem::action("Close Terminal Tab", CloseActiveTerminal),
                MenuItem::action("New Worktree", OpenCreateWorktree),
            ],
        },
        Menu {
            name: "Terminal".into(),
            items: vec![
                MenuItem::action("New Terminal Tab", SpawnTerminal),
                MenuItem::action("Close Terminal Tab", CloseActiveTerminal),
                MenuItem::separator(),
                MenuItem::action("Edit Presets...", OpenManagePresets),
                MenuItem::action("Custom Presets...", OpenManageRepoPresets),
                MenuItem::separator(),
                MenuItem::action("Use Embedded Backend", UseEmbeddedBackend),
            ],
        },
        Menu {
            name: "Hosts".into(),
            items: host_items,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Toggle Sidebar", ToggleLeftPane),
                MenuItem::action("Collapse All Repositories", CollapseAllRepositories),
                MenuItem::separator(),
                MenuItem::action("Theme Picker...", OpenThemePicker),
                MenuItem::separator(),
                MenuItem::action("View Logs", ViewLogs),
            ],
        },
        Menu {
            name: "Worktree".into(),
            items: vec![
                MenuItem::action("Add Repository...", OpenAddRepository),
                MenuItem::separator(),
                MenuItem::action("New Worktree", OpenCreateWorktree),
                MenuItem::separator(),
                MenuItem::action("Navigate Back", NavigateWorktreeBack),
                MenuItem::action("Navigate Forward", NavigateWorktreeForward),
                MenuItem::separator(),
                MenuItem::action("Refresh Worktrees", RefreshWorktrees),
                MenuItem::action("Refresh Changes", RefreshChanges),
            ],
        },
    ]
}

pub(crate) fn bounds_from_window_geometry(
    geometry: ui_state_store::WindowGeometry,
) -> Option<Bounds<Pixels>> {
    if geometry.width == 0 || geometry.height == 0 {
        return None;
    }

    let width = geometry.width as f32;
    let height = geometry.height as f32;
    if !width.is_finite() || !height.is_finite() {
        return None;
    }

    Some(Bounds::new(
        point(px(geometry.x as f32), px(geometry.y as f32)),
        size(px(width), px(height)),
    ))
}

/// The augmented PATH computed at startup, merging the user's login-shell PATH
/// with the process PATH.  Stored once, read by [`create_command`].
pub(crate) static AUGMENTED_PATH: OnceLock<String> = OnceLock::new();

/// When launched as a macOS `.app` bundle the process inherits a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`).  This function sources the user's login
/// shell to obtain their real PATH and merges it with the current one so that
/// tools like `gh` and `git` installed via Homebrew are found.
///
/// The result is stored in [`AUGMENTED_PATH`] and applied per-command via
/// [`create_command`] rather than mutating the global environment.
pub(crate) fn augment_path_from_login_shell() {
    if !cfg!(target_os = "macos") {
        return;
    }

    let current_path = env::var("PATH").unwrap_or_default();

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
    let marker_start = "__PATH_START__";
    let marker_end = "__PATH_END__";

    let shell_path = match Command::new(&shell)
        .args(["-lic", &format!("echo {marker_start}${{PATH}}{marker_end}")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .find_map(|line| {
                    let start = line.find(marker_start)?;
                    let after_start = start + marker_start.len();
                    let end = line[after_start..].find(marker_end)?;
                    Some(line[after_start..after_start + end].to_owned())
                })
                .unwrap_or_default()
        },
        _ => String::new(),
    };

    // Merge: login-shell paths first, then current PATH, deduplicated.
    let mut seen = HashSet::new();
    let mut merged: Vec<&str> = Vec::new();

    let paths_to_add = if shell_path.is_empty() {
        let home = env::var("HOME").unwrap_or_default();
        vec![
            "/opt/homebrew/bin".to_owned(),
            "/opt/homebrew/sbin".to_owned(),
            "/usr/local/bin".to_owned(),
            format!("{home}/.local/bin"),
        ]
    } else {
        shell_path.split(':').map(|s| s.to_owned()).collect()
    };

    for dir in &paths_to_add {
        if !dir.is_empty() && seen.insert(dir.as_str()) {
            merged.push(dir.as_str());
        }
    }
    for dir in current_path.split(':') {
        if !dir.is_empty() && seen.insert(dir) {
            merged.push(dir);
        }
    }

    AUGMENTED_PATH.set(merged.join(":")).ok();
}

/// Create a [`Command`] with the augmented PATH applied.  Use this instead of
/// [`Command::new`] so that Homebrew-installed tools are found when running as
/// a macOS `.app` bundle.
pub(crate) fn create_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    if let Some(path) = AUGMENTED_PATH.get() {
        cmd.env("PATH", path);
    }
    cmd
}

/// Explicitly set the dock icon.
///
/// When running inside a `.app` bundle, loads the icon from the bundle resources.
/// When running via `cargo run` (no bundle), falls back to loading the source PNG
/// from the `assets/` directory so the dock shows the real icon instead of a folder.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub(crate) fn set_dock_icon() {
    use cocoa::{
        appkit::{NSApp, NSApplication, NSImage},
        base::{id, nil},
        foundation::NSString as _,
    };

    // SAFETY: Cocoa FFI – we call well-known AppKit selectors on the shared
    // NSApplication. GPUI has already initialised the NSApplication before
    // our `run` callback executes.
    unsafe {
        let icon_name = cocoa::foundation::NSString::alloc(nil).init_str("NSApplicationIcon");
        let icon: id = NSImage::imageNamed_(nil, icon_name);
        if icon != nil {
            NSApp().setApplicationIconImage_(icon);
            return;
        }

        // Fallback for development: load the icon PNG from the source tree.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let icon_path = format!("{manifest_dir}/../../assets/icons/arbor-icon-1024.png");
        if let Ok(canonical) = fs::canonicalize(&icon_path) {
            let path_str = canonical.to_string_lossy();
            let ns_path = cocoa::foundation::NSString::alloc(nil).init_str(&path_str);
            let icon: id = NSImage::alloc(nil).initWithContentsOfFile_(ns_path);
            if icon != nil {
                NSApp().setApplicationIconImage_(icon);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn set_dock_icon() {}
