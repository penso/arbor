fn open_arbor_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1460.), px(900.)), cx);
    if let Err(error) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(1180.), px(760.))),
            app_id: Some("so.pen.arbor".to_owned()),
            titlebar: Some(default_titlebar_options(None)),
            window_decorations: Some(DEFAULT_WINDOW_DECORATIONS),
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

fn new_window(_: &NewWindow, cx: &mut App) {
    open_arbor_window(cx);
}

fn install_app_menu_and_keys(cx: &mut App) {
    cx.on_action(new_window);
    cx.bind_keys([
        KeyBinding::new("cmd-n", NewWindow, None),
        KeyBinding::new("cmd-q", RequestQuit, None),
        KeyBinding::new("cmd-t", SpawnTerminal, None),
        KeyBinding::new("cmd-w", CloseActiveTerminal, None),
        KeyBinding::new("cmd-k", OpenCommandPalette, None),
        KeyBinding::new("cmd-shift-o", OpenAddRepository, None),
        KeyBinding::new("cmd-shift-n", OpenCreateWorktree, None),
        KeyBinding::new("cmd-shift-r", RefreshWorktrees, None),
        KeyBinding::new("cmd-alt-r", RefreshChanges, None),
        KeyBinding::new("cmd-\\", ToggleLeftPane, None),
        KeyBinding::new("cmd-[", NavigateWorktreeBack, None),
        KeyBinding::new("cmd-]", NavigateWorktreeForward, None),
        KeyBinding::new("cmd-shift-l", ViewLogs, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
    ]);
    cx.set_menus(build_app_menus(&[]));
}

fn build_app_menus(discovered_daemons: &[mdns_browser::DiscoveredDaemon]) -> Vec<Menu> {
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
                MenuItem::action("Command Palette...", OpenCommandPalette),
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

fn bounds_from_window_geometry(geometry: ui_state_store::WindowGeometry) -> Option<Bounds<Pixels>> {
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
static AUGMENTED_PATH: OnceLock<String> = OnceLock::new();

/// When launched as a macOS `.app` bundle the process inherits a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`).  This function sources the user's login
/// shell to obtain their real PATH and merges it with the current one so that
/// tools like `gh` and `git` installed via Homebrew are found.
///
/// The result is stored in [`AUGMENTED_PATH`] and applied per-command via
/// [`create_command`] rather than mutating the global environment.
fn augment_path_from_login_shell() {
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
fn create_command(program: &str) -> Command {
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
fn set_dock_icon() {
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
fn set_dock_icon() {}

enum LaunchMode {
    Gui,
    Daemon { bind_addr: Option<String> },
    Help,
}

fn parse_launch_mode(args: impl IntoIterator<Item = String>) -> Result<LaunchMode, CliError> {
    let mut daemon_mode = false;
    let mut bind_addr: Option<String> = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--daemon" | "--daemon-only" | "daemon" => {
                daemon_mode = true;
            },
            "--bind" | "--daemon-bind" => {
                let Some(value) = args.next() else {
                    return Err(CliError::InvalidArg(format!("missing value for `{arg}`")));
                };
                if value.trim().is_empty() {
                    return Err(CliError::InvalidArg(format!(
                        "`{arg}` requires a non-empty address"
                    )));
                }
                bind_addr = Some(value);
            },
            "-h" | "--help" => return Ok(LaunchMode::Help),
            unknown => {
                return Err(CliError::InvalidArg(format!(
                    "unknown argument `{unknown}`"
                )));
            },
        }
    }

    if daemon_mode {
        Ok(LaunchMode::Daemon { bind_addr })
    } else {
        Ok(LaunchMode::Gui)
    }
}

fn daemon_cli_usage(program_name: &str) -> String {
    format!(
        "Usage:\n  {program_name}\n  {program_name} --daemon [--bind ADDR]\n\nExamples:\n  {program_name} --daemon\n  {program_name} --daemon --bind 0.0.0.0:8787"
    )
}

fn themed_ui_svg_icon(
    path: &'static str,
    color: u32,
    size_px: f32,
    fallback_glyph: &'static str,
) -> Div {
    div()
        .size(px(size_px))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(match find_assets_root_dir().map(|dir| dir.join(path)) {
            Some(path) => img(path)
                .size(px(size_px))
                .with_fallback(move || {
                    div()
                        .font_family(FONT_MONO)
                        .text_size(px(size_px))
                        .line_height(px(size_px))
                        .text_color(rgb(color))
                        .child(fallback_glyph)
                        .into_any_element()
                })
                .into_any_element(),
            None => div()
                .font_family(FONT_MONO)
                .text_size(px(size_px))
                .line_height(px(size_px))
                .text_color(rgb(color))
                .child(fallback_glyph)
                .into_any_element(),
        })
}

fn terminal_tab_icon_element(is_active: bool, color: u32, size_px: f32) -> Div {
    themed_ui_svg_icon(
        if is_active {
            "icons/ui/terminal-active.svg"
        } else {
            "icons/ui/terminal-muted.svg"
        },
        color,
        size_px,
        "\u{f120}",
    )
}

fn logs_tab_icon_element(is_active: bool, color: u32, size_px: f32) -> Div {
    themed_ui_svg_icon(
        if is_active {
            "icons/ui/logs-active.svg"
        } else {
            "icons/ui/logs-muted.svg"
        },
        color,
        size_px,
        "\u{f4ed}",
    )
}
fn run_daemon_mode(bind_addr: Option<String>) -> Result<(), DaemonLaunchError> {
    let binary = find_arbor_httpd_binary().ok_or_else(|| {
        DaemonLaunchError::Failed(
            "could not find `arbor-httpd` in PATH or next to the current executable".to_owned(),
        )
    })?;

    let mut command = Command::new(&binary);
    if let Some(path) = AUGMENTED_PATH.get() {
        command.env("PATH", path);
    }
    if let Some(bind_addr) = bind_addr {
        command.env("ARBOR_HTTPD_BIND", bind_addr);
    }

    let binary_name = binary
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("arbor-httpd");
    let status = command.status().map_err(|error| {
        DaemonLaunchError::Failed(format!("failed to start `{binary_name}`: {error}"))
    })?;

    if status.success() {
        return Ok(());
    }

    Err(DaemonLaunchError::Failed(format!(
        "arbor-httpd exited with status {status}"
    )))
}

fn main() {
    let program_name = env::args().next().unwrap_or_else(|| "arbor".to_owned());
    let launch_mode = match parse_launch_mode(env::args().skip(1)) {
        Ok(mode) => mode,
        Err(error) => {
            eprintln!("{error}\n\n{}", daemon_cli_usage(&program_name));
            std::process::exit(2);
        },
    };

    if matches!(launch_mode, LaunchMode::Help) {
        println!("{}", daemon_cli_usage(&program_name));
        return;
    }

    augment_path_from_login_shell();

    if let LaunchMode::Daemon { bind_addr } = launch_mode {
        if let Err(error) = run_daemon_mode(bind_addr) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }

    force_x11_on_wayland();

    let log_buffer = log_layer::LogBuffer::new();

    {
        use tracing_subscriber::{
            EnvFilter, Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt,
        };

        let env_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let in_memory_layer =
            log_layer::InMemoryLayer::new(log_buffer.clone()).with_filter(env_filter);

        Registry::default().with(in_memory_layer).init();
    }

    tracing::info!("Arbor starting");

    let mut application = Application::new();
    if let Some(assets_base) = find_assets_root_dir() {
        application = application.with_assets(ArborAssets { base: assets_base });
    }

    application.run(move |cx: &mut App| {
        register_bundled_fonts(cx);
        set_dock_icon();
        cx.set_http_client(simple_http_client::create_http_client());
        install_app_menu_and_keys(cx);
        let startup_ui_state = ui_state_store::load_startup_state();
        let default_bounds = Bounds::centered(None, size(px(1460.), px(900.)), cx);
        let bounds = startup_ui_state
            .window
            .and_then(bounds_from_window_geometry)
            .unwrap_or(default_bounds);
        let startup_ui_state_for_window = startup_ui_state.clone();
        let log_buffer_for_window = log_buffer.clone();

        if let Err(error) = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(1180.), px(760.))),
                app_id: Some("so.pen.arbor".to_owned()),
                titlebar: Some(default_titlebar_options(None)),
                window_decorations: Some(DEFAULT_WINDOW_DECORATIONS),
                ..Default::default()
            },
            move |_, cx| {
                let startup_ui_state = startup_ui_state_for_window.clone();
                let log_buffer = log_buffer_for_window.clone();
                cx.new(move |cx| {
                    ArborWindow::load_with_daemon_store::<daemon::JsonDaemonSessionStore>(
                        startup_ui_state,
                        log_buffer,
                        cx,
                    )
                })
            },
        ) {
            eprintln!("failed to open Arbor window: {error:#}");
            cx.quit();
            return;
        }

        cx.activate(true);
    });
}

/// Force X11 (via XWayland) on Wayland sessions. GPUI's Wayland backend does not
/// yet support server-side decoration features (resize handles, drag, close/min/max
/// buttons), so we fall back to X11 where these work correctly.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn force_x11_on_wayland() {
    if env::var("WAYLAND_DISPLAY").is_ok_and(|v| !v.is_empty()) {
        // SAFETY: called at the very start of main before any threads are spawned,
        // and only when running in GUI mode (after the daemon early-return).
        unsafe { env::remove_var("WAYLAND_DISPLAY") };
    }
}

#[cfg(not(target_os = "linux"))]
fn force_x11_on_wayland() {}
