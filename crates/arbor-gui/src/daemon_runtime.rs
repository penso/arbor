use {super::*, serde::Deserialize};

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
    rows: u16,
    cols: u16,
    poll_notify: Option<std::sync::mpsc::Sender<()>>,
) -> SharedTerminalRuntime {
    let ws_state = Arc::new(DaemonTerminalWsState::new(poll_notify, rows, cols));
    spawn_daemon_terminal_ws_watcher(daemon.clone(), session_id.clone(), &ws_state);

    Arc::new(DaemonTerminalRuntime {
        daemon,
        ws_state,
        last_synced_ws_generation: std::sync::atomic::AtomicU64::new(0),
        snapshot_request_in_flight: Arc::new(AtomicBool::new(false)),
        kind: TerminalRuntimeKind::Local,
        resize_error_label: "failed to resize terminal",
        exit_labels: Some(RuntimeExitLabels {
            completed_title: "Terminal completed",
            failed_title: "Terminal failed",
            failed_notice_prefix: "terminal tab",
        }),
        clear_global_daemon_on_connection_refused: true,
    })
}

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

pub(crate) const DAEMON_TERMINAL_WS_MAX_LINES: usize = 220;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum DaemonTerminalWsServerEvent {
    Snapshot {
        output_tail: String,
        state: TerminalSessionState,
        exit_code: Option<i32>,
        updated_at_unix_ms: Option<u64>,
    },
    Exit {
        state: TerminalSessionState,
        exit_code: Option<i32>,
    },
    Error {
        message: String,
    },
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

pub(crate) fn trim_terminal_snapshot(
    mut snapshot: arbor_terminal_emulator::TerminalSnapshot,
    max_lines: usize,
) -> arbor_terminal_emulator::TerminalSnapshot {
    snapshot.output = trim_to_last_lines(snapshot.output, max_lines);

    let keep_from = snapshot.styled_lines.len().saturating_sub(max_lines);
    snapshot.cursor = snapshot.cursor.and_then(|cursor| {
        (cursor.line >= keep_from).then_some(TerminalCursor {
            line: cursor.line - keep_from,
            column: cursor.column,
        })
    });

    if keep_from > 0 {
        snapshot.styled_lines.drain(..keep_from);
    }

    snapshot
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
) -> Result<tungstenite::http::Request<()>, ConnectionError> {
    use tungstenite::client::IntoClientRequest;

    let mut request = connect_config
        .url
        .as_str()
        .into_client_request()
        .map_err(|error| {
            ConnectionError::Parse(format!("failed to create websocket request: {error}"))
        })?;

    if let Some(token) = connect_config.auth_token.as_ref() {
        let header_value = tungstenite::http::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|error| {
                ConnectionError::Parse(format!("failed to encode websocket auth token: {error}"))
            })?;
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
                            Ok(tungstenite::Message::Binary(bytes)) => {
                                ws_state.apply_output_bytes(&bytes);
                            },
                            Ok(tungstenite::Message::Text(payload)) => {
                                match serde_json::from_str::<DaemonTerminalWsServerEvent>(&payload)
                                {
                                    Ok(DaemonTerminalWsServerEvent::Snapshot {
                                        output_tail,
                                        state,
                                        exit_code,
                                        updated_at_unix_ms,
                                    }) => {
                                        ws_state.apply_snapshot_text(
                                            &output_tail,
                                            terminal_state_from_daemon_state(state),
                                            exit_code,
                                            updated_at_unix_ms,
                                        );
                                    },
                                    Ok(DaemonTerminalWsServerEvent::Exit { state, exit_code }) => {
                                        ws_state.apply_exit(
                                            terminal_state_from_daemon_state(state),
                                            exit_code,
                                        );
                                    },
                                    Ok(DaemonTerminalWsServerEvent::Error { message }) => {
                                        tracing::debug!(
                                            session_id = %session_id,
                                            %message,
                                            "daemon terminal websocket reported an error"
                                        );
                                    },
                                    Err(error) => {
                                        tracing::debug!(
                                            session_id = %session_id,
                                            %error,
                                            "failed to decode daemon terminal websocket event"
                                        );
                                    },
                                }
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
                                if daemon_error_is_connection_refused(&error.to_string()) {
                                    ws_state.note_connection_refused();
                                }
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
                    if daemon_error_is_connection_refused(&error.to_string()) {
                        ws_state.note_connection_refused();
                    }
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

pub(crate) fn request_async_daemon_snapshot(
    daemon: terminal_daemon_http::SharedTerminalDaemonClient,
    session_id: String,
    ws_state: Arc<DaemonTerminalWsState>,
    in_flight: Arc<AtomicBool>,
) {
    if in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    std::thread::spawn(move || {
        let result = daemon.snapshot(daemon::SnapshotRequest {
            session_id: session_id.clone().into(),
            max_lines: DAEMON_TERMINAL_WS_MAX_LINES,
        });

        match result {
            Ok(Some(snapshot)) => {
                if ws_state.snapshot().is_none() {
                    ws_state.apply_snapshot_text(
                        &snapshot.output_tail,
                        terminal_state_from_daemon_state(snapshot.state),
                        snapshot.exit_code,
                        snapshot.updated_at_unix_ms,
                    );
                }
            },
            Ok(None) => {},
            Err(error) => {
                tracing::debug!(
                    session_id = %session_id,
                    %error,
                    "failed to load daemon terminal snapshot asynchronously"
                );
                if daemon_error_is_connection_refused(&error.to_string()) {
                    ws_state.note_connection_refused();
                }
            },
        }

        in_flight.store(false, Ordering::Release);
    });
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

#[cfg(test)]
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

#[cfg(test)]
pub(crate) fn daemon_cursor_to_terminal_cursor(
    cursor: daemon::DaemonTerminalCursor,
) -> TerminalCursor {
    TerminalCursor {
        line: cursor.line,
        column: cursor.column,
    }
}

#[cfg(test)]
pub(crate) fn daemon_modes_to_terminal_modes(modes: daemon::DaemonTerminalModes) -> TerminalModes {
    TerminalModes {
        app_cursor: modes.app_cursor,
        alt_screen: modes.alt_screen,
    }
}

#[cfg(test)]
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

#[cfg(test)]
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

pub(crate) fn terminal_state_from_daemon_state(state: TerminalSessionState) -> TerminalState {
    match state {
        TerminalSessionState::Running => TerminalState::Running,
        TerminalSessionState::Completed => TerminalState::Completed,
        TerminalSessionState::Failed => TerminalState::Failed,
    }
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

pub(crate) fn cleanup_orphaned_daemon_session(
    daemon: terminal_daemon_http::SharedTerminalDaemonClient,
    record: DaemonSessionRecord,
) -> Result<(), ConnectionError> {
    let result = if orphaned_daemon_session_should_kill(&record) {
        daemon.kill(KillRequest {
            session_id: record.session_id.clone(),
        })
    } else {
        daemon.detach(DetachRequest {
            session_id: record.session_id.clone(),
        })
    };

    result.map_err(|error| {
        ConnectionError::Io(format!(
            "failed to clean up orphaned daemon session `{}`: {error}",
            record.session_id
        ))
    })
}

pub(crate) fn orphaned_daemon_session_should_kill(record: &DaemonSessionRecord) -> bool {
    terminal_state_from_daemon_record(record) == TerminalState::Running
}

pub(crate) fn schedule_orphaned_daemon_session_cleanup<C>(
    cx: &C,
    daemon: terminal_daemon_http::SharedTerminalDaemonClient,
    record: DaemonSessionRecord,
) where
    C: gpui::AppContext,
{
    let session_id = record.session_id.to_string();
    cx.background_spawn(async move {
        if let Err(error) = cleanup_orphaned_daemon_session(daemon, record) {
            tracing::warn!(
                session_id = %session_id,
                %error,
                "failed to clean up orphaned daemon session"
            );
        }
    })
    .detach();
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

pub(crate) fn parse_connect_host_target(raw: &str) -> Result<ConnectHostTarget, ConnectionError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(ConnectionError::Parse("Address cannot be empty".to_owned()));
    }

    if value.starts_with("ssh://") {
        let target = parse_ssh_daemon_target(value)?;
        let auth_key = format_ssh_auth_key(&target);
        return Ok(ConnectHostTarget::Ssh { target, auth_key });
    }

    if value.starts_with("https://") {
        return Err(ConnectionError::Parse(
            "https:// is not supported by arbor-httpd; use http://HOST:PORT or ssh://HOST/"
                .to_owned(),
        ));
    }

    if value.starts_with("http://") {
        return Ok(ConnectHostTarget::Http {
            url: value.to_owned(),
            auth_key: value.to_owned(),
        });
    }

    if value.contains("://") {
        return Err(ConnectionError::Parse(
            "unsupported scheme; use http://HOST:PORT or ssh://[user@]HOST[:ssh_port]/".to_owned(),
        ));
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

pub(crate) fn parse_ssh_daemon_target(raw: &str) -> Result<SshDaemonTarget, ConnectionError> {
    let Some(without_scheme) = raw.trim().strip_prefix("ssh://") else {
        return Err(ConnectionError::Parse(
            "ssh address must start with ssh://".to_owned(),
        ));
    };
    if without_scheme.is_empty() {
        return Err(ConnectionError::Parse(
            "ssh address is missing a host".to_owned(),
        ));
    }

    let (authority, path_tail) = match without_scheme.split_once('/') {
        Some((authority, tail)) => (authority, tail),
        None => (without_scheme, ""),
    };

    if authority.trim().is_empty() {
        return Err(ConnectionError::Parse(
            "ssh address is missing a host".to_owned(),
        ));
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
) -> Result<(Option<String>, String, u16), ConnectionError> {
    let (user, host_port) = match authority.rsplit_once('@') {
        Some((candidate_user, host_port))
            if !candidate_user.trim().is_empty() && !host_port.trim().is_empty() =>
        {
            (Some(candidate_user.trim().to_owned()), host_port.trim())
        },
        Some(_) => {
            return Err(ConnectionError::Parse(
                "invalid ssh address: malformed user@host section".to_owned(),
            ));
        },
        None => (None, authority.trim()),
    };

    let (host, port) = parse_host_and_optional_port(host_port, DEFAULT_SSH_PORT)?;
    Ok((user, host, port))
}

pub(crate) fn parse_ssh_daemon_port(path_tail: &str) -> Result<u16, ConnectionError> {
    let trimmed = path_tail.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(DEFAULT_DAEMON_PORT);
    }
    if trimmed.contains('/') || trimmed.contains('?') || trimmed.contains('#') {
        return Err(ConnectionError::Parse(
            "invalid ssh address path: only an optional daemon port is allowed, for example ssh://host/8787"
                .to_owned(),
        ));
    }

    trimmed.parse::<u16>().map_err(|error| {
        ConnectionError::Parse(format!("invalid daemon port `{trimmed}`: {error}"))
    })
}

pub(crate) fn parse_host_and_optional_port(
    value: &str,
    default_port: u16,
) -> Result<(String, u16), ConnectionError> {
    if let Some(rest) = value.strip_prefix('[') {
        let Some((host, suffix)) = rest.split_once(']') else {
            return Err(ConnectionError::Parse(
                "invalid host: missing closing `]` for IPv6 address".to_owned(),
            ));
        };
        if host.trim().is_empty() {
            return Err(ConnectionError::Parse("host is empty".to_owned()));
        }
        if suffix.is_empty() {
            return Ok((host.to_owned(), default_port));
        }
        let Some(port_text) = suffix.strip_prefix(':') else {
            return Err(ConnectionError::Parse(
                "invalid host: unexpected characters after IPv6 address".to_owned(),
            ));
        };
        let port = port_text.parse::<u16>().map_err(|error| {
            ConnectionError::Parse(format!("invalid port `{port_text}`: {error}"))
        })?;
        return Ok((host.to_owned(), port));
    }

    let Some((host, port_text)) = value.rsplit_once(':') else {
        return Ok((value.to_owned(), default_port));
    };

    if host.contains(':') {
        return Err(ConnectionError::Parse(
            "IPv6 hosts must be wrapped in brackets, for example [::1]".to_owned(),
        ));
    }
    if host.trim().is_empty() {
        return Err(ConnectionError::Parse("host is empty".to_owned()));
    }
    let port = port_text
        .parse::<u16>()
        .map_err(|error| ConnectionError::Parse(format!("invalid port `{port_text}`: {error}")))?;
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

pub(crate) fn reserve_local_loopback_port() -> Result<u16, ConnectionError> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| ConnectionError::Io(format!("failed to reserve local port: {error}")))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|error| {
            ConnectionError::Io(format!("failed to resolve local tunnel port: {error}"))
        })
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

    let home = env::var("HOME").ok().map(PathBuf::from)?;
    let log_dir = home.join(".arbor/daemon");
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
    if let Ok(exe) = env::current_exe() {
        let sibling = exe.with_file_name("arbor-httpd");
        if sibling.is_file() {
            return Some(sibling);
        }
    }

    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|dir| dir.join("arbor-httpd"))
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
