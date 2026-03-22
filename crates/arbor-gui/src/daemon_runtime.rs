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
    let snapshot_request_in_flight = Arc::new(AtomicBool::new(false));
    let snapshot_request_pending = Arc::new(AtomicBool::new(false));
    spawn_daemon_terminal_ws_watcher(
        daemon.clone(),
        session_id.clone(),
        &ws_state,
        snapshot_request_in_flight.clone(),
        snapshot_request_pending.clone(),
    );

    Arc::new(DaemonTerminalRuntime {
        daemon,
        ws_state,
        last_synced_ws_generation: std::sync::atomic::AtomicU64::new(0),
        snapshot_request_in_flight,
        snapshot_request_pending,
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

pub(crate) fn daemon_terminal_ws_max_lines() -> usize {
    arbor_terminal_emulator::default_terminal_scrollback_lines()
}

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
    snapshot: &arbor_terminal_emulator::TerminalSnapshot,
) -> bool {
    let mut changed = false;

    if session.output != snapshot.output
        || session.styled_output != snapshot.styled_lines
        || session.cursor != snapshot.cursor
        || session.modes != snapshot.modes
    {
        session.output = snapshot.output.clone();
        session.styled_output = snapshot.styled_lines.clone();
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

pub(crate) fn event_driven_terminal_sync_interval(
    is_active: bool,
    session_state: TerminalState,
) -> Option<Duration> {
    (session_state == TerminalState::Running).then_some(if is_active {
        ACTIVE_EVENT_DRIVEN_TERMINAL_SYNC_INTERVAL
    } else {
        INACTIVE_EVENT_DRIVEN_TERMINAL_SYNC_INTERVAL
    })
}

pub(crate) fn ssh_terminal_sync_interval(
    is_active: bool,
    session_state: TerminalState,
) -> Option<Duration> {
    match session_state {
        TerminalState::Running => Some(if is_active {
            ACTIVE_SSH_TERMINAL_SYNC_INTERVAL
        } else {
            INACTIVE_SSH_TERMINAL_SYNC_INTERVAL
        }),
        TerminalState::Completed | TerminalState::Failed => None,
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

pub(crate) fn terminal_sync_interval_for_session(
    session: &TerminalSession,
    default_interval: Duration,
    now: Instant,
) -> Duration {
    if session.state == TerminalState::Running
        && session
            .interactive_sync_until
            .is_some_and(|deadline| deadline > now)
    {
        return default_interval.min(INTERACTIVE_TERMINAL_SYNC_INTERVAL);
    }

    default_interval
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
    snapshot_request_in_flight: Arc<AtomicBool>,
    snapshot_request_pending: Arc<AtomicBool>,
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
                    if !ws_state.has_ready_snapshot() {
                        ws_state.request_snapshot_refresh();
                        request_async_daemon_snapshot(
                            daemon.clone(),
                            session_id.clone(),
                            ws_state.clone(),
                            snapshot_request_in_flight.clone(),
                            snapshot_request_pending.clone(),
                        );
                    }
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
                                if !ws_state.apply_output_bytes(&bytes) {
                                    schedule_daemon_ws_snapshot_rebuild(ws_state.clone());
                                }
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
    pending: Arc<AtomicBool>,
) {
    if in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        pending.store(true, Ordering::Release);
        return;
    }

    std::thread::spawn(move || {
        loop {
            let requested_generation = ws_state.event_generation();
            let result = daemon.snapshot(daemon::SnapshotRequest {
                session_id: session_id.clone().into(),
                max_lines: daemon_terminal_ws_max_lines(),
            });

            match result {
                Ok(Some(snapshot)) => {
                    let cached_updated_at_unix_ms = ws_state
                        .snapshot
                        .lock()
                        .ok()
                        .and_then(|cached| cached.updated_at_unix_ms);
                    if should_apply_async_daemon_snapshot(
                        requested_generation,
                        ws_state.event_generation(),
                        snapshot.updated_at_unix_ms,
                        cached_updated_at_unix_ms,
                    ) {
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

            if !pending.swap(false, Ordering::AcqRel) {
                break;
            }

            if in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                break;
            }
        }
    });
}

pub(crate) fn should_apply_async_daemon_snapshot(
    requested_generation: u64,
    current_generation: u64,
    snapshot_updated_at_unix_ms: Option<u64>,
    cached_updated_at_unix_ms: Option<u64>,
) -> bool {
    if current_generation == requested_generation {
        return true;
    }

    match (snapshot_updated_at_unix_ms, cached_updated_at_unix_ms) {
        (Some(snapshot_updated_at), Some(cached_updated_at)) => {
            snapshot_updated_at >= cached_updated_at
        },
        (Some(_), None) => true,
        (None, _) => false,
    }
}

pub(crate) fn schedule_daemon_ws_snapshot_rebuild(ws_state: Arc<DaemonTerminalWsState>) {
    if ws_state
        .snapshot_build_in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        ws_state
            .snapshot_build_pending
            .store(true, Ordering::Release);
        return;
    }

    std::thread::spawn(move || {
        loop {
            let exit_code = ws_state
                .snapshot
                .lock()
                .ok()
                .map(|cached| cached.terminal.exit_code)
                .unwrap_or(None);
            let mut terminal_snapshot = {
                let emulator = match ws_state.emulator.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                emulator.snapshot_tail(daemon_terminal_ws_max_lines())
            };
            terminal_snapshot.exit_code = exit_code;

            let mut cached = match ws_state.snapshot.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            cached.terminal = Arc::new(terminal_snapshot);
            cached.updated_at_unix_ms = current_unix_timestamp_millis();
            cached.ready = true;
            drop(cached);
            ws_state.note_event_with_cached_snapshot();

            ws_state
                .snapshot_build_in_flight
                .store(false, Ordering::Release);

            if !ws_state
                .snapshot_build_pending
                .swap(false, Ordering::AcqRel)
            {
                break;
            }

            if ws_state
                .snapshot_build_in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                break;
            }
        }
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
    let lines = terminal_display_tail_lines(session, max_lines);
    if lines.is_empty() {
        return String::new();
    }
    let mut tail = lines
        .into_iter()
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

#[cfg(test)]
pub(crate) fn session_with_styled_line(
    text: &str,
    fg: u32,
    bg: u32,
    cursor: Option<TerminalCursor>,
) -> TerminalSession {
    TerminalSession {
        id: 1,
        daemon_session_id: "daemon-test-1".to_owned(),
        worktree_path: PathBuf::from("/tmp/worktree"),
        managed_process_id: None,
        title: "term-1".to_owned(),
        last_command: None,
        pending_command: String::new(),
        command: "zsh".to_owned(),
        agent_preset: None,
        execution_mode: None,
        state: TerminalState::Running,
        exit_code: None,
        updated_at_unix_ms: None,
        root_pid: None,
        cols: 120,
        rows: 35,
        generation: 0,
        output: text.to_owned(),
        styled_output: vec![TerminalStyledLine {
            cells: text
                .chars()
                .enumerate()
                .map(|(column, character)| TerminalStyledCell {
                    column,
                    text: character.to_string(),
                    fg,
                    bg,
                })
                .collect(),
            runs: vec![TerminalStyledRun {
                text: text.to_owned(),
                fg,
                bg,
            }],
        }],
        cursor,
        modes: TerminalModes::default(),
        last_runtime_sync_at: None,
        interactive_sync_until: None,
        last_port_hint_scan_at: None,
        queued_input: Vec::new(),
        is_initializing: false,
        runtime: None,
    }
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

#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) mod tests {
    use {
        super::*,
        crate::terminal_daemon_http::{HttpTerminalDaemon, WebsocketConnectConfig},
        std::{
            sync::{
                Arc,
                atomic::{AtomicU64, Ordering},
            },
            time::Instant,
        },
    };

    #[derive(Clone)]
    struct TestEmbeddedBackend {
        generation: Arc<AtomicU64>,
    }

    impl TestEmbeddedBackend {
        fn new(generation: u64) -> Self {
            Self {
                generation: Arc::new(AtomicU64::new(generation)),
            }
        }
    }

    impl EmulatorRuntimeBackend for TestEmbeddedBackend {
        fn poll(&self) {}

        fn write_input(&self, _input: &[u8]) -> Result<(), TerminalError> {
            Ok(())
        }

        fn snapshot(&self) -> arbor_terminal_emulator::TerminalSnapshot {
            arbor_terminal_emulator::TerminalSnapshot {
                output: String::new(),
                styled_lines: Vec::new(),
                cursor: None,
                modes: TerminalModes::default(),
                exit_code: None,
            }
        }

        fn resize(
            &self,
            _rows: u16,
            _cols: u16,
            _pixel_width: u16,
            _pixel_height: u16,
        ) -> Result<(), TerminalError> {
            Ok(())
        }

        fn generation(&self) -> u64 {
            self.generation.load(Ordering::Relaxed)
        }

        fn close(&self) {}

        fn background_sync_interval(
            &self,
            is_active: bool,
            session_state: TerminalState,
        ) -> Option<Duration> {
            event_driven_terminal_sync_interval(is_active, session_state)
        }
    }

    fn embedded_runtime_for_test(generation: u64) -> EmulatorTerminalRuntime<TestEmbeddedBackend> {
        EmulatorTerminalRuntime {
            backend: TestEmbeddedBackend::new(generation),
            kind: TerminalRuntimeKind::Local,
            resize_error_label: "resize",
            exit_labels: RuntimeExitLabels {
                completed_title: "done",
                failed_title: "failed",
                failed_notice_prefix: "terminal",
            },
        }
    }

    pub(crate) fn daemon_runtime_for_test() -> DaemonTerminalRuntime {
        let daemon = match HttpTerminalDaemon::new("http://127.0.0.1:1") {
            Ok(daemon) => daemon,
            Err(error) => panic!("failed to create daemon client: {error}"),
        };

        DaemonTerminalRuntime {
            daemon: Arc::new(daemon),
            ws_state: Arc::new(DaemonTerminalWsState::default()),
            last_synced_ws_generation: AtomicU64::new(0),
            snapshot_request_in_flight: Arc::new(AtomicBool::new(false)),
            snapshot_request_pending: Arc::new(AtomicBool::new(false)),
            kind: TerminalRuntimeKind::Local,
            resize_error_label: "resize",
            exit_labels: None,
            clear_global_daemon_on_connection_refused: false,
        }
    }

    #[test]
    fn active_terminal_sync_is_prioritized() {
        let mut first = session_with_styled_line("one", 0xffffff, 0x000000, None);
        first.id = 10;
        let mut second = session_with_styled_line("two", 0xffffff, 0x000000, None);
        second.id = 20;
        let mut third = session_with_styled_line("three", 0xffffff, 0x000000, None);
        third.id = 30;

        let indices = ordered_terminal_sync_indices(&[first, second, third], Some(30));

        assert_eq!(indices, vec![2, 0, 1]);
    }

    #[test]
    fn daemon_terminal_sync_interval_uses_active_fallback() {
        assert_eq!(
            daemon_terminal_sync_interval(true, TerminalState::Running),
            ACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL
        );
        assert_eq!(
            daemon_terminal_sync_interval(false, TerminalState::Running),
            INACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL
        );
        assert_eq!(
            daemon_terminal_sync_interval(false, TerminalState::Completed),
            IDLE_DAEMON_TERMINAL_SYNC_INTERVAL
        );
        assert_eq!(
            daemon_terminal_sync_interval(false, TerminalState::Failed),
            IDLE_DAEMON_TERMINAL_SYNC_INTERVAL
        );
    }

    #[test]
    fn event_driven_terminal_sync_interval_coalesces_running_sessions() {
        assert_eq!(
            event_driven_terminal_sync_interval(true, TerminalState::Running),
            Some(ACTIVE_EVENT_DRIVEN_TERMINAL_SYNC_INTERVAL)
        );
        assert_eq!(
            event_driven_terminal_sync_interval(false, TerminalState::Running),
            Some(INACTIVE_EVENT_DRIVEN_TERMINAL_SYNC_INTERVAL)
        );
        assert_eq!(
            event_driven_terminal_sync_interval(true, TerminalState::Completed),
            None
        );
    }

    #[test]
    fn ssh_terminal_sync_interval_polls_only_running_sessions() {
        assert_eq!(
            ssh_terminal_sync_interval(true, TerminalState::Running),
            Some(ACTIVE_SSH_TERMINAL_SYNC_INTERVAL)
        );
        assert_eq!(
            ssh_terminal_sync_interval(false, TerminalState::Running),
            Some(INACTIVE_SSH_TERMINAL_SYNC_INTERVAL)
        );
        assert_eq!(
            ssh_terminal_sync_interval(true, TerminalState::Completed),
            None
        );
    }

    #[test]
    fn daemon_runtime_coalesces_active_session_ws_bursts() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        assert!(!runtime.should_sync(&session, true, None, now));

        runtime.ws_state.note_event();

        assert!(!runtime.should_sync(&session, true, None, now));
        assert!(runtime.should_sync(
            &session,
            true,
            None,
            now + ACTIVE_DAEMON_EVENT_COALESCE_INTERVAL
        ));
    }

    #[test]
    fn async_daemon_snapshot_applies_when_snapshot_is_newer_than_cached_ws_state() {
        assert!(should_apply_async_daemon_snapshot(
            1,
            2,
            Some(200),
            Some(150)
        ));
        assert!(should_apply_async_daemon_snapshot(1, 2, Some(200), None));
    }

    #[test]
    fn async_daemon_snapshot_skips_stale_snapshot_after_newer_ws_state() {
        assert!(!should_apply_async_daemon_snapshot(
            1,
            2,
            Some(150),
            Some(200)
        ));
        assert!(!should_apply_async_daemon_snapshot(1, 2, None, Some(200)));
    }

    #[test]
    fn daemon_runtime_uses_interactive_sync_interval_after_input() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);
        session.interactive_sync_until = Some(now + INTERACTIVE_TERMINAL_SYNC_WINDOW);

        runtime.ws_state.note_event();

        assert!(!runtime.should_sync(
            &session,
            true,
            None,
            now + INTERACTIVE_TERMINAL_SYNC_INTERVAL.saturating_sub(Duration::from_millis(1))
        ));
        assert!(runtime.should_sync(
            &session,
            true,
            None,
            now + INTERACTIVE_TERMINAL_SYNC_INTERVAL
        ));
    }

    #[test]
    fn daemon_runtime_coalesces_refresh_requests_for_active_sessions() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        runtime.ws_state.request_snapshot_refresh();

        assert!(!runtime.should_sync(&session, true, None, now));
        assert!(runtime.should_sync(
            &session,
            true,
            None,
            now + ACTIVE_DAEMON_EVENT_COALESCE_INTERVAL
        ));
    }

    #[test]
    fn embedded_runtime_coalesces_active_generation_bursts() {
        let runtime = embedded_runtime_for_test(1);
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        assert!(!runtime.should_sync(&session, true, None, now));
        assert!(runtime.should_sync(
            &session,
            true,
            None,
            now + ACTIVE_EVENT_DRIVEN_TERMINAL_SYNC_INTERVAL
        ));
    }

    #[test]
    fn embedded_runtime_uses_interactive_sync_interval_after_input() {
        let runtime = embedded_runtime_for_test(1);
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);
        session.interactive_sync_until = Some(now + INTERACTIVE_TERMINAL_SYNC_WINDOW);

        assert!(!runtime.should_sync(
            &session,
            true,
            None,
            now + INTERACTIVE_TERMINAL_SYNC_INTERVAL.saturating_sub(Duration::from_millis(1))
        ));
        assert!(runtime.should_sync(
            &session,
            true,
            None,
            now + INTERACTIVE_TERMINAL_SYNC_INTERVAL
        ));
    }

    #[test]
    fn embedded_runtime_throttles_inactive_generation_bursts() {
        let runtime = embedded_runtime_for_test(1);
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        assert!(!runtime.should_sync(&session, false, None, now));
        assert!(runtime.should_sync(
            &session,
            false,
            None,
            now + INACTIVE_EVENT_DRIVEN_TERMINAL_SYNC_INTERVAL
        ));
    }

    #[test]
    fn daemon_runtime_throttles_inactive_sessions_even_when_ws_is_dirty() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("background", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        runtime.ws_state.note_event();

        assert!(!runtime.should_sync(&session, false, None, now));
        assert!(runtime.should_sync(
            &session,
            false,
            None,
            now + INACTIVE_DAEMON_TERMINAL_SYNC_INTERVAL
        ));
    }

    #[test]
    fn daemon_runtime_syncs_active_resize_without_waiting_for_ws() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("prompt", 0xffffff, 0x000000, None);
        let now = Instant::now();
        session.last_runtime_sync_at = Some(now);

        assert!(runtime.should_sync(
            &session,
            true,
            Some((session.rows + 1, session.cols, 0, 0)),
            now
        ));
    }

    #[test]
    fn orphaned_daemon_session_cleanup_kills_only_running_sessions() {
        let mut record = DaemonSessionRecord {
            session_id: "daemon-test-1".into(),
            workspace_id: "/tmp/worktree".into(),
            cwd: PathBuf::from("/tmp/worktree"),
            shell: "zsh".to_owned(),
            ..Default::default()
        };

        assert!(orphaned_daemon_session_should_kill(&record));

        record.state = Some(TerminalSessionState::Completed);
        assert!(!orphaned_daemon_session_should_kill(&record));

        record.state = Some(TerminalSessionState::Failed);
        assert!(!orphaned_daemon_session_should_kill(&record));
    }

    #[test]
    fn daemon_runtime_without_cached_snapshot_returns_without_sync_error() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.output.clear();
        session.styled_output.clear();
        session.cursor = None;
        session.last_runtime_sync_at = Some(Instant::now());

        let outcome = runtime.sync(&mut session, true, None);

        assert!(!outcome.changed);
        assert!(outcome.notice.is_none());
        assert_eq!(session.state, TerminalState::Running);
        assert!(session.output.is_empty());
    }

    #[test]
    fn daemon_ws_state_rehydrates_trimmed_snapshot_from_ansi_output() {
        let ws_state = DaemonTerminalWsState::default();
        ws_state.apply_snapshot_text("hello\r\nworld\r\n", TerminalState::Running, None, Some(42));

        let snapshot = ws_state
            .snapshot()
            .unwrap_or_else(|| panic!("expected websocket snapshot to be available"));

        assert_eq!(snapshot.state, TerminalState::Running);
        assert_eq!(snapshot.updated_at_unix_ms, Some(42));
        assert!(snapshot.terminal.output.contains("hello"));
        assert!(snapshot.terminal.output.contains("world"));
        assert_eq!(snapshot.terminal.styled_lines.len(), 2);
    }

    #[test]
    fn interactive_ws_output_publishes_snapshot_immediately() {
        let ws_state = DaemonTerminalWsState::default();
        ws_state.enter_interactive_output_window();

        assert!(ws_state.apply_output_bytes(b"echo"));

        let snapshot = ws_state
            .snapshot()
            .unwrap_or_else(|| panic!("expected inline websocket snapshot to be available"));
        assert!(snapshot.terminal.output.contains("echo"));
    }

    #[test]
    fn interactive_ws_large_shell_redraw_still_publishes_snapshot_immediately() {
        let ws_state = DaemonTerminalWsState::default();
        ws_state.enter_interactive_output_window();

        let redraw = "$ ".to_owned() + &"x".repeat(2_048);
        assert!(ws_state.apply_output_bytes(redraw.as_bytes()));

        let snapshot = ws_state
            .snapshot()
            .unwrap_or_else(|| panic!("expected inline websocket snapshot for large redraw"));
        assert!(snapshot.terminal.output.contains(&"x".repeat(128)));
    }

    #[test]
    fn daemon_runtime_write_input_does_not_mutate_snapshot_before_echo() {
        let runtime = daemon_runtime_for_test();
        let (tx, _rx) = std::sync::mpsc::channel();
        runtime.ws_state.set_writer(Some(tx));
        runtime
            .ws_state
            .apply_snapshot_text("$ ", TerminalState::Running, None, Some(1));

        let session = session_with_styled_line("$ ", 0xffffff, 0x000000, None);
        let before = runtime
            .ws_state
            .snapshot()
            .unwrap_or_else(|| panic!("expected snapshot before local input"));

        if let Err(error) = runtime.write_input(&session, b" ") {
            panic!("expected websocket write to succeed: {error}");
        }

        let after = runtime
            .ws_state
            .snapshot()
            .unwrap_or_else(|| panic!("expected snapshot after local input"));
        assert_eq!(after.terminal.output, before.terminal.output);
        assert_eq!(after.terminal.cursor, before.terminal.cursor);
    }

    #[test]
    fn daemon_ws_output_redraw_replaces_line_without_duplicate_local_echo() {
        let ws_state = DaemonTerminalWsState::default();
        ws_state.apply_snapshot_text("$ ", TerminalState::Running, None, Some(1));

        ws_state.enter_interactive_output_window();
        assert!(ws_state.apply_output_bytes(b"star"));
        assert!(ws_state.apply_output_bytes(b"\r\x1b[2K$ starship"));
        let snapshot = ws_state
            .snapshot()
            .unwrap_or_else(|| panic!("expected snapshot after redraw output"));
        assert!(
            snapshot
                .terminal
                .output
                .trim_end_matches('\n')
                .ends_with("$ starship"),
            "unexpected redraw output: {:?}",
            snapshot.terminal.output
        );
    }

    #[test]
    fn daemon_runtime_sync_applies_cached_ws_snapshot_without_http_roundtrip() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.output.clear();
        session.styled_output.clear();
        session.cursor = None;
        session.exit_code = None;
        runtime.ws_state.apply_snapshot_text(
            "codex> working\r\n",
            TerminalState::Running,
            None,
            Some(99),
        );

        let outcome = runtime.sync(&mut session, true, None);

        assert!(outcome.changed);
        assert_eq!(session.state, TerminalState::Running);
        assert_eq!(session.updated_at_unix_ms, Some(99));
        assert!(session.output.contains("codex> working"));
        assert_eq!(session.exit_code, None);
    }

    #[test]
    fn daemon_runtime_timestamp_only_ws_update_does_not_mark_session_changed() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.output.clear();
        session.styled_output.clear();
        session.cursor = None;

        runtime.ws_state.apply_snapshot_text(
            "codex> working\r\n",
            TerminalState::Running,
            None,
            Some(42),
        );
        let first = runtime.sync(&mut session, true, None);
        assert!(first.changed);
        assert_eq!(session.updated_at_unix_ms, Some(42));

        runtime.ws_state.apply_snapshot_text(
            "codex> working\r\n",
            TerminalState::Running,
            None,
            Some(99),
        );
        let second = runtime.sync(&mut session, true, None);

        assert!(!second.changed);
        assert_eq!(session.updated_at_unix_ms, Some(99));
        assert!(session.output.contains("codex> working"));
    }

    #[test]
    fn active_daemon_sync_repaints_without_recopying_session_buffers() {
        let runtime = daemon_runtime_for_test();
        let mut session = session_with_styled_line("stale", 0xffffff, 0x000000, None);
        session.updated_at_unix_ms = Some(10);

        runtime.ws_state.apply_snapshot_text(
            "codex> refreshed\r\n",
            TerminalState::Running,
            None,
            Some(99),
        );

        let outcome = runtime.sync(&mut session, true, None);
        let render_snapshot = runtime
            .render_snapshot(&session)
            .unwrap_or_else(|| panic!("expected daemon render snapshot"));

        assert!(!outcome.changed);
        assert!(outcome.repaint);
        assert_eq!(session.updated_at_unix_ms, Some(99));
        assert!(session.output.contains("stale"));
        assert!(render_snapshot.terminal.output.contains("codex> refreshed"));
    }

    #[test]
    fn daemon_websocket_request_adds_bearer_auth_header() {
        let request = match daemon_websocket_request(&WebsocketConnectConfig {
            url: "ws://127.0.0.1:8787/api/v1/agent/activity/ws".to_owned(),
            auth_token: Some("secret-token".to_owned()),
        }) {
            Ok(request) => request,
            Err(error) => panic!("failed to build websocket request: {error}"),
        };

        assert_eq!(
            request
                .headers()
                .get(tungstenite::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret-token")
        );
    }

    #[test]
    fn daemon_snapshot_applies_structured_terminal_state() {
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.output.clear();
        session.styled_output.clear();
        session.cursor = None;
        session.modes = TerminalModes::default();

        let changed = apply_daemon_snapshot(&mut session, &daemon::TerminalSnapshot {
            session_id: "daemon-test-1".to_owned().into(),
            output_tail: "READY".to_owned(),
            styled_lines: vec![daemon::DaemonTerminalStyledLine {
                cells: vec![daemon::DaemonTerminalStyledCell {
                    column: 0,
                    text: "READY".to_owned(),
                    fg: 0x123456,
                    bg: 0x654321,
                }],
                runs: vec![daemon::DaemonTerminalStyledRun {
                    text: "READY".to_owned(),
                    fg: 0x123456,
                    bg: 0x654321,
                }],
            }],
            cursor: Some(daemon::DaemonTerminalCursor { line: 0, column: 5 }),
            modes: daemon::DaemonTerminalModes {
                app_cursor: true,
                alt_screen: true,
            },
            exit_code: None,
            state: TerminalSessionState::Running,
            updated_at_unix_ms: Some(1),
        });

        assert!(changed);
        assert_eq!(session.output, "READY");
        assert_eq!(session.cursor, Some(TerminalCursor { line: 0, column: 5 }));
        assert_eq!(session.modes, TerminalModes {
            app_cursor: true,
            alt_screen: true,
        });
        assert_eq!(session.styled_output.len(), 1);
        assert_eq!(session.styled_output[0].runs[0].text, "READY");
        assert_eq!(session.styled_output[0].runs[0].fg, 0x123456);
        assert_eq!(session.styled_output[0].runs[0].bg, 0x654321);
    }

    #[test]
    fn shift_enter_does_not_submit_pending_terminal_command() {
        let mut session = session_with_styled_line("", 0xffffff, 0x000000, None);
        session.pending_command = "hello".to_owned();

        track_terminal_command_keystroke(
            &mut session,
            &Keystroke::parse("shift-enter").expect("valid keystroke"),
        );

        assert_eq!(session.pending_command, "hello\n");
        assert_eq!(session.last_command, None);
    }

    #[test]
    fn parse_connect_host_target_normalizes_bare_http_host() {
        let target = parse_connect_host_target("10.0.0.5")
            .expect("bare host should parse as http daemon target");

        match target {
            ConnectHostTarget::Http { url, auth_key } => {
                assert_eq!(url, "http://10.0.0.5:8787");
                assert_eq!(auth_key, url);
            },
            ConnectHostTarget::Ssh { .. } => panic!("expected http target"),
        }
    }

    #[test]
    fn parse_connect_host_target_supports_ssh_scheme() {
        let target = parse_connect_host_target("ssh://dev@example.com:2222/9001")
            .expect("ssh address should parse");

        match target {
            ConnectHostTarget::Ssh { target, auth_key } => {
                assert_eq!(target.user.as_deref(), Some("dev"));
                assert_eq!(target.host, "example.com");
                assert_eq!(target.ssh_port, 2222);
                assert_eq!(target.daemon_port, 9001);
                assert_eq!(auth_key, "ssh://dev@example.com:2222/9001");
            },
            ConnectHostTarget::Http { .. } => panic!("expected ssh target"),
        }
    }
}
